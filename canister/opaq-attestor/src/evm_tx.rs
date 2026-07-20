// P5.1 (OPAQ.md B.14.6): EVM leg, write side — build + threshold-ECDSA-sign a
// legacy (EIP-155) Ethereum transaction calling `addPending`, entirely inside
// the canister (no external signer), and submit it via evm-rpc-canister.
use std::str::FromStr;

use evm_rpc_types::{Hex, MultiRpcResult};
use ic_cdk::management_canister::{sign_with_ecdsa, SignWithEcdsaArgs};

use crate::ecdsa_key_id;
use crate::evm::{client_for, evm_pubkey_sec1};

const SEL_ADD_PENDING: &str = "562b80a1"; // cast sig "addPending(bytes32,uint256,uint256)"

// ---- minimal RLP (just enough for a legacy tx: byte strings + lists) ----

fn rlp_bytes(b: &[u8]) -> Vec<u8> {
    if b.len() == 1 && b[0] < 0x80 {
        vec![b[0]]
    } else if b.len() < 56 {
        let mut out = vec![0x80 + b.len() as u8];
        out.extend_from_slice(b);
        out
    } else {
        let len_bytes = be_bytes_minimal(b.len() as u64);
        let mut out = vec![0xb7 + len_bytes.len() as u8];
        out.extend_from_slice(&len_bytes);
        out.extend_from_slice(b);
        out
    }
}

fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.concat();
    if payload.len() < 56 {
        let mut out = vec![0xc0 + payload.len() as u8];
        out.extend_from_slice(&payload);
        out
    } else {
        let len_bytes = be_bytes_minimal(payload.len() as u64);
        let mut out = vec![0xf7 + len_bytes.len() as u8];
        out.extend_from_slice(&len_bytes);
        out.extend_from_slice(&payload);
        out
    }
}

fn be_bytes_minimal(v: u64) -> Vec<u8> {
    let b = v.to_be_bytes();
    let start = b.iter().position(|&x| x != 0).unwrap_or(b.len());
    b[start..].to_vec()
}

/// RLP "quantity" encoding: big-endian bytes with leading zeros stripped
/// (zero itself encodes as the empty string, per the Ethereum Yellow Paper).
fn rlp_quantity(be: &[u8]) -> Vec<u8> {
    let start = be.iter().position(|&b| b != 0).unwrap_or(be.len());
    rlp_bytes(&be[start..])
}

fn hex_to_be_bytes(s: &str) -> Result<Vec<u8>, String> {
    let clean = s.trim_start_matches("0x");
    let padded = if clean.len() % 2 == 1 {
        format!("0{clean}")
    } else {
        clean.to_string()
    };
    hex::decode(&padded).map_err(|e| format!("bad hex {s}: {e}"))
}

fn parse_u256_hex(s: &str) -> Result<[u8; 32], String> {
    let bytes = hex_to_be_bytes(s)?;
    if bytes.len() > 32 {
        return Err(format!("value {s} longer than 32 bytes"));
    }
    let mut out = [0u8; 32];
    out[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(out)
}

async fn raw_json_rpc(
    chain_id: u64,
    rpc_url: String,
    method: &str,
    params: serde_json::Value,
) -> Result<String, String> {
    let json = serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params, "id": 1});
    let result = client_for(chain_id, rpc_url).multi_request(json).send().await;
    match result {
        MultiRpcResult::Consistent(Ok(s)) => Ok(s),
        MultiRpcResult::Consistent(Err(e)) => Err(format!("{method}: {e:?}")),
        MultiRpcResult::Inconsistent(r) => Err(format!("{method}: providers disagreed: {r:?}")),
    }
}

/// Builds the RLP payload for a legacy (type-0) EIP-155 tx. `signature` is
/// `None` for the unsigned (to-be-hashed-and-signed) form, `Some((v, r, s))`
/// for the final signed form.
fn legacy_tx_rlp(
    nonce: u64,
    gas_price: &[u8],
    gas_limit: u64,
    to: &[u8; 20],
    data: &[u8],
    chain_id: u64,
    signature: Option<(u64, &[u8; 32], &[u8; 32])>,
) -> Vec<u8> {
    let (v, r, s): (Vec<u8>, Vec<u8>, Vec<u8>) = match signature {
        Some((v, r, s)) => (be_bytes_minimal(v), r.to_vec(), s.to_vec()),
        None => (be_bytes_minimal(chain_id), vec![], vec![]),
    };
    rlp_list(&[
        rlp_quantity(&nonce.to_be_bytes()),
        rlp_quantity(gas_price),
        rlp_quantity(&gas_limit.to_be_bytes()),
        rlp_bytes(to),
        rlp_quantity(&[0u8; 32]), // value: always 0, addPending takes no ETH
        rlp_bytes(data),
        rlp_quantity(&v),
        rlp_quantity(&r),
        rlp_quantity(&s),
    ])
}

/// *Accept (P5.1, write half):* the tx lands (positive receipt status), and
/// `read_pending_mint` on the exact same `eth_call` path then reflects it —
/// not merely "a receipt existed", but "the chain's own authoritative state
/// changed", per B.14.2's read-real-state design.
#[ic_cdk::update]
async fn submit_add_pending(
    chain_id: u64,
    rpc_url: String,
    pool_address: String,
    nullifier_hex: String,
    dest_chain_hex: String,
    out_commitment_hex: String,
) -> Result<String, String> {
    let to: [u8; 20] = {
        let clean = pool_address.trim_start_matches("0x");
        let bytes = hex::decode(clean).map_err(|e| format!("bad pool address: {e}"))?;
        bytes
            .try_into()
            .map_err(|_| "pool address must be 20 bytes".to_string())?
    };
    let nullifier = parse_u256_hex(&nullifier_hex)?;
    let dest_chain = parse_u256_hex(&dest_chain_hex)?;
    let out_commitment = parse_u256_hex(&out_commitment_hex)?;

    let mut data = hex::decode(SEL_ADD_PENDING).unwrap();
    data.extend_from_slice(&nullifier);
    data.extend_from_slice(&dest_chain);
    data.extend_from_slice(&out_commitment);

    let (pubkey, from_addr) = evm_pubkey_sec1().await?;
    let from_hex = format!("0x{}", hex::encode(from_addr));

    let nonce_hex = raw_json_rpc(
        chain_id,
        rpc_url.clone(),
        "eth_getTransactionCount",
        serde_json::json!([from_hex, "latest"]),
    )
    .await?;
    let nonce = u64::from_str_radix(nonce_hex.trim_start_matches("0x"), 16)
        .map_err(|e| format!("bad nonce {nonce_hex}: {e}"))?;

    let gas_price_hex = raw_json_rpc(chain_id, rpc_url.clone(), "eth_gasPrice", serde_json::json!([]))
        .await?;
    let gas_price = hex_to_be_bytes(&gas_price_hex)?;

    const GAS_LIMIT: u64 = 200_000; // addPending is one SSTORE + guard reads; generous headroom

    let unsigned = legacy_tx_rlp(nonce, &gas_price, GAS_LIMIT, &to, &data, chain_id, None);
    let digest: [u8; 32] = {
        use sha3::{Digest, Keccak256};
        Keccak256::digest(&unsigned).into()
    };

    let sig = sign_with_ecdsa(&SignWithEcdsaArgs {
        message_hash: digest.to_vec(),
        derivation_path: vec![],
        key_id: ecdsa_key_id(),
    })
    .await
    .map_err(|e| format!("sign_with_ecdsa: {e:?}"))?
    .signature;
    let r: [u8; 32] = sig[0..32].try_into().unwrap();
    let s: [u8; 32] = sig[32..64].try_into().unwrap();

    let recovery_id = recovery_id_for(&digest, &r, &s, &pubkey)?;
    let v = chain_id * 2 + 35 + recovery_id as u64; // EIP-155

    let signed = legacy_tx_rlp(nonce, &gas_price, GAS_LIMIT, &to, &data, chain_id, Some((v, &r, &s)));
    let signed_hex = format!("0x{}", hex::encode(&signed));

    let result = client_for(chain_id, rpc_url)
        .send_raw_transaction(Hex::from_str(&signed_hex).map_err(|e| format!("bad signed tx: {e}"))?)
        .send()
        .await;
    match result {
        MultiRpcResult::Consistent(Ok(status)) => Ok(format!("{status:?}")),
        MultiRpcResult::Consistent(Err(e)) => Err(format!("send_raw_transaction: {e:?}")),
        MultiRpcResult::Inconsistent(r) => Err(format!("providers disagreed: {r:?}")),
    }
}

/// The IC's `sign_with_ecdsa` doesn't return a recovery id, so — as is
/// standard for tECDSA canisters — try both candidates and keep whichever
/// recovers back to our own known pubkey.
fn recovery_id_for(
    digest: &[u8; 32],
    r: &[u8; 32],
    s: &[u8; 32],
    expected_pubkey_sec1: &[u8],
) -> Result<u8, String> {
    use k256::ecdsa::signature::hazmat::PrehashVerifier;
    let signature = k256::ecdsa::Signature::from_scalars(*r, *s).map_err(|e| e.to_string())?;
    for id in 0u8..2 {
        let recid = k256::ecdsa::RecoveryId::from_byte(id).unwrap();
        if let Ok(candidate) = k256::ecdsa::VerifyingKey::recover_from_prehash(digest, &signature, recid) {
            if candidate.to_encoded_point(true).as_bytes() == expected_pubkey_sec1 {
                // sanity: the recovered key must also verify the signature.
                candidate
                    .verify_prehash(digest, &signature)
                    .map_err(|e| format!("recovered key failed to verify: {e}"))?;
                return Ok(id);
            }
        }
    }
    Err("neither recovery id matched our own pubkey".to_string())
}

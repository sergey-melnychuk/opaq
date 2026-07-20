// P5.2 (OPAQ.md B.14.6): Solana leg, write side — build + threshold-Ed25519
// (Schnorr) sign a legacy Solana transaction calling `add_pending_xburn`
// (tag 6), entirely inside the canister, and submit it via `sendTransaction`.
use ic_cdk::management_canister::{
    http_request, schnorr_public_key, sign_with_schnorr, HttpHeader, HttpMethod, HttpRequestArgs,
    HttpRequestResult, SchnorrPublicKeyArgs, SignWithSchnorrArgs, TransformArgs, TransformContext,
    TransformFunc,
};

use crate::schnorr_key_id;

const TAG_ADD_PENDING_XBURN: u8 = 6;
const SYSTEM_PROGRAM_ID: [u8; 32] = [0u8; 32];

#[ic_cdk::query(hidden = true)]
fn transform_solana_rpc(args: TransformArgs) -> HttpRequestResult {
    HttpRequestResult {
        status: args.response.status,
        body: args.response.body,
        headers: vec![],
    }
}

async fn rpc_call(rpc_url: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let request = HttpRequestArgs {
        url: rpc_url.to_string(),
        max_response_bytes: Some(50_000),
        method: HttpMethod::POST,
        headers: vec![HttpHeader {
            name: "Content-Type".to_string(),
            value: "application/json".to_string(),
        }],
        body: Some(body.to_string().into_bytes()),
        transform: Some(TransformContext {
            function: TransformFunc::new(ic_cdk::api::canister_self(), "transform_solana_rpc".to_string()),
            context: vec![],
        }),
        is_replicated: None,
    };
    let response = http_request(&request).await.map_err(|e| format!("{method} outcall: {e:?}"))?;
    let json: serde_json::Value =
        serde_json::from_slice(&response.body).map_err(|e| format!("{method}: bad JSON: {e}"))?;
    if let Some(err) = json.get("error") {
        return Err(format!("{method}: RPC error: {err}"));
    }
    Ok(json)
}

fn shortvec_encode(mut n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            byte |= 0x80;
            out.push(byte);
        } else {
            out.push(byte);
            break;
        }
    }
    out
}

/// Program-derived address for seed `xpending` under `program_id` — the same
/// derivation `Pubkey::find_program_address(&[XBURN_PENDING_SEED], program_id)`
/// computes on-chain (programs/opaq/src/lib.rs). PDAs are simply SHA-256(seeds
/// ‖ bump ‖ program_id ‖ "ProgramDerivedAddress") checked to fall off-curve;
/// re-implemented here since this canister has no solana-sdk dependency.
fn find_program_address(seed: &[u8], program_id: &[u8; 32]) -> Result<[u8; 32], String> {
    use sha2::{Digest, Sha256};
    for bump in (0u8..=255).rev() {
        let mut hasher = Sha256::new();
        hasher.update(seed);
        hasher.update([bump]);
        hasher.update(program_id);
        hasher.update(b"ProgramDerivedAddress");
        let candidate: [u8; 32] = hasher.finalize().into();
        if !is_on_curve(&candidate) {
            return Ok(candidate);
        }
    }
    Err("no valid PDA bump found".to_string())
}

/// A point is a valid Ed25519 curve point iff its compressed-y bytes decode
/// successfully — this is exactly what Solana's `PodU8Array`/off-curve check
/// tests (curve25519-dalek's `CompressedEdwardsY::decompress`).
fn is_on_curve(bytes: &[u8; 32]) -> bool {
    curve25519_dalek::edwards::CompressedEdwardsY(*bytes)
        .decompress()
        .is_some()
}

/// *Accept (P5.2, write half):* the tx lands, and the on-chain `XburnPending`
/// account's own state (read back via `getAccountInfo`, not just a receipt)
/// reflects the new entry — mirrors P5.1's `read_pending_mint` check.
#[ic_cdk::update]
async fn submit_add_pending_xburn(
    rpc_url: String,
    program_id_b58: String,
    nullifier_hex: String,
    dest_chain_hex: String,
    out_commitment_hex: String,
) -> Result<String, String> {
    let program_id: [u8; 32] = bs58::decode(&program_id_b58)
        .into_vec()
        .map_err(|e| format!("bad program id: {e}"))?
        .try_into()
        .map_err(|_| "program id must be 32 bytes".to_string())?;
    let pda = find_program_address(b"xpending", &program_id)?;

    let parse32 = |s: &str| -> Result<[u8; 32], String> {
        hex::decode(s.trim_start_matches("0x"))
            .map_err(|e| e.to_string())?
            .try_into()
            .map_err(|_| "expected 32 bytes".to_string())
    };
    let nullifier = parse32(&nullifier_hex)?;
    let dest_chain = parse32(&dest_chain_hex)?;
    let out_commitment = parse32(&out_commitment_hex)?;

    let mut data = vec![TAG_ADD_PENDING_XBURN];
    data.extend_from_slice(&nullifier);
    data.extend_from_slice(&dest_chain);
    data.extend_from_slice(&out_commitment);

    let pk = schnorr_public_key(&SchnorrPublicKeyArgs {
        canister_id: None,
        derivation_path: vec![],
        key_id: schnorr_key_id(),
    })
    .await
    .map_err(|e| format!("schnorr_public_key: {e:?}"))?;
    let fee_payer: [u8; 32] = pk
        .public_key
        .try_into()
        .map_err(|_| "unexpected pubkey length".to_string())?;

    let blockhash_resp = rpc_call(&rpc_url, "getLatestBlockhash", serde_json::json!([{"commitment": "finalized"}])).await?;
    let blockhash_b58 = blockhash_resp["result"]["value"]["blockhash"]
        .as_str()
        .ok_or("missing blockhash in response")?;
    let recent_blockhash: [u8; 32] = bs58::decode(blockhash_b58)
        .into_vec()
        .map_err(|e| format!("bad blockhash: {e}"))?
        .try_into()
        .map_err(|_| "blockhash must be 32 bytes".to_string())?;

    let message = build_message(&fee_payer, &pda, &program_id, &data, &recent_blockhash);

    let sig = sign_with_schnorr(&SignWithSchnorrArgs {
        message: message.clone(),
        derivation_path: vec![],
        key_id: schnorr_key_id(),
        aux: None,
    })
    .await
    .map_err(|e| format!("sign_with_schnorr: {e:?}"))?
    .signature;

    let mut wire = shortvec_encode(1);
    wire.extend_from_slice(&sig);
    wire.extend_from_slice(&message);

    use base64::Engine;
    let wire_b64 = base64::engine::general_purpose::STANDARD.encode(&wire);
    let submit = rpc_call(
        &rpc_url,
        "sendTransaction",
        serde_json::json!([wire_b64, {"encoding": "base64"}]),
    )
    .await?;
    let tx_sig = submit["result"]
        .as_str()
        .ok_or_else(|| format!("sendTransaction: unexpected response: {submit}"))?;
    Ok(tx_sig.to_string())
}

/// Builds a legacy (non-versioned) Solana message for a single instruction
/// with accounts `[fee_payer(signer,w), pda(w), system_program(readonly),
/// opaq_program(readonly)]` — exactly what `add_pending_xburn` expects.
fn build_message(
    fee_payer: &[u8; 32],
    pda: &[u8; 32],
    program_id: &[u8; 32],
    data: &[u8],
    recent_blockhash: &[u8; 32],
) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.push(1u8); // numRequiredSignatures
    msg.push(0u8); // numReadonlySignedAccounts
    msg.push(2u8); // numReadonlyUnsignedAccounts (system_program, opaq_program)

    let account_keys: [&[u8; 32]; 4] = [fee_payer, pda, &SYSTEM_PROGRAM_ID, program_id];
    msg.extend_from_slice(&shortvec_encode(account_keys.len()));
    for k in account_keys {
        msg.extend_from_slice(k);
    }

    msg.extend_from_slice(recent_blockhash);

    msg.extend_from_slice(&shortvec_encode(1)); // 1 instruction
    msg.push(3); // programIdIndex = 3 (opaq program)
    msg.extend_from_slice(&shortvec_encode(3));
    msg.push(0); // fee_payer (operator, signer+writable)
    msg.push(1); // pda (writable)
    msg.push(2); // system_program (readonly)
    msg.extend_from_slice(&shortvec_encode(data.len()));
    msg.extend_from_slice(data);
    msg
}

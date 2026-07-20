// P5.1 (OPAQ.md B.14.6): EVM leg via the DFINITY evm-rpc-canister — read side.
// Checks OpaqPool's own authoritative state (`nullifierSpent` / `pendingMint`),
// never event logs (B.14.2's "log != transfer" fix).
use std::str::FromStr;

use candid::Principal;
use evm_rpc_client::{CandidResponseConverter, EvmRpcClient, NoRetry};
use evm_rpc_types::{
    BlockTag, CallArgs, Hex, Hex20, MultiRpcResult, RpcApi, RpcServices, TransactionRequest,
};
use ic_canister_runtime::IcRuntime;
use ic_cdk::management_canister::{ecdsa_public_key, EcdsaPublicKeyArgs};
use k256::elliptic_curve::sec1::ToEncodedPoint;

use crate::ecdsa_key_id;

// nullifierSpent(bytes32) / pendingMint(bytes32) — both public mapping getters
// on OpaqPool.sol, selectors via `cast sig`.
const SEL_NULLIFIER_SPENT: &str = "38c86911";
const SEL_PENDING_MINT: &str = "87488892";

fn encode_call(selector: &str, arg32: &[u8; 32]) -> String {
    format!("0x{selector}{}", hex::encode(arg32))
}

fn evm_rpc_canister_id() -> Principal {
    let raw = ic_cdk::api::env_var_value("PUBLIC_CANISTER_ID:evm_rpc");
    Principal::from_text(raw).expect("invalid/missing PUBLIC_CANISTER_ID:evm_rpc principal")
}

/// `rpc_url` + `chain_id` name a Custom provider so this can be pointed at a
/// local anvil during development (P5.1) as well as a real testnet/mainnet
/// (P5.3+) — which predefined chain/provider set to use in production is a
/// P5.5 config concern, not decided here.
pub(crate) fn client_for(
    chain_id: u64,
    rpc_url: String,
) -> EvmRpcClient<IcRuntime, CandidResponseConverter, NoRetry> {
    EvmRpcClient::builder(IcRuntime::new(), evm_rpc_canister_id())
        .with_rpc_sources(RpcServices::Custom {
            chain_id,
            services: vec![RpcApi {
                url: rpc_url,
                headers: None,
            }],
        })
        .build()
}

/// Fetches this canister's own threshold-ECDSA pubkey, returning both the raw
/// SEC1-compressed bytes (needed to pick the right ECDSA recovery id when
/// signing) and the derived 20-byte EVM address.
pub(crate) async fn evm_pubkey_sec1() -> Result<(Vec<u8>, [u8; 20]), String> {
    let res = ecdsa_public_key(&EcdsaPublicKeyArgs {
        canister_id: None,
        derivation_path: vec![],
        key_id: ecdsa_key_id(),
    })
    .await
    .map_err(|e| format!("ecdsa_public_key: {e:?}"))?;
    let point =
        k256::PublicKey::from_sec1_bytes(&res.public_key).map_err(|e| format!("bad pubkey: {e}"))?;
    let uncompressed = point.to_encoded_point(false);
    use sha3::{Digest, Keccak256};
    let hash = Keccak256::digest(&uncompressed.as_bytes()[1..]);
    let addr: [u8; 20] = hash[12..].try_into().unwrap();
    Ok((res.public_key, addr))
}

fn parse_bytes32(hex_str: &str) -> [u8; 32] {
    let clean = hex_str.trim_start_matches("0x");
    let bytes = hex::decode(clean).expect("nullifier must be valid hex");
    bytes.try_into().expect("nullifier must be exactly 32 bytes")
}

// `finalized` should be `true` for every real deployment (B.14.3 — checking
// anything less than Finalized risks attesting a burn a reorg later erases).
// It's a caller-supplied bool rather than hardcoded because there's no
// governance/config layer yet (P5.5) to stop someone passing `false`, AND
// because anvil (used for local dev, e.g. P5.1's own local proof) pins
// `finalized`/`safe` to genesis block 0 unconditionally — a single-node dev
// chain has no real consensus finality to track — so a hardcoded `Finalized`
// would make this untestable against anvil at all. Real networks (Sepolia,
// mainnet) resolve `Finalized` correctly.
async fn eth_call_bytes32(
    chain_id: u64,
    rpc_url: String,
    pool_address: &str,
    selector: &str,
    arg32: &[u8; 32],
    finalized: bool,
) -> Result<[u8; 32], String> {
    let calldata = encode_call(selector, arg32);
    let args = CallArgs {
        transaction: TransactionRequest {
            to: Some(Hex20::from_str(pool_address).map_err(|e| format!("bad pool address: {e}"))?),
            input: Some(Hex::from_str(&calldata).map_err(|e| format!("bad calldata: {e}"))?),
            ..Default::default()
        },
        block: Some(if finalized {
            BlockTag::Finalized
        } else {
            BlockTag::Latest
        }),
    };

    let result = client_for(chain_id, rpc_url).call(args).send().await;
    let hex_result = match result {
        MultiRpcResult::Consistent(Ok(r)) => r,
        MultiRpcResult::Consistent(Err(e)) => return Err(format!("eth_call error: {e:?}")),
        MultiRpcResult::Inconsistent(r) => return Err(format!("providers disagreed: {r:?}")),
    };
    let raw = hex::decode(hex_result.to_string().trim_start_matches("0x"))
        .map_err(|e| format!("bad eth_call response hex: {e}"))?;
    raw.try_into()
        .map_err(|r: Vec<u8>| format!("expected 32-byte word, got {} bytes", r.len()))
}

/// *Accept (P5.1, read half):* a known-unspent nullifier returns `false`; a
/// known-spent one (after a real `xburn()` call) returns `true`.
#[ic_cdk::update]
async fn check_nullifier_spent(
    chain_id: u64,
    rpc_url: String,
    pool_address: String,
    nullifier_hex: String,
    finalized: bool,
) -> Result<bool, String> {
    let word = eth_call_bytes32(
        chain_id,
        rpc_url,
        &pool_address,
        SEL_NULLIFIER_SPENT,
        &parse_bytes32(&nullifier_hex),
        finalized,
    )
    .await?;
    // Solidity bool is ABI-encoded as a full 32-byte word, 0 or 1.
    Ok(word[31] == 1 && word[..31].iter().all(|b| *b == 0))
}

/// Reads `pendingMint[nullifier]` (the attested-destination hash, B.14.2's
/// tuple-binding fix) — used to confirm a submitted `addPending` landed and
/// is queryable back through the exact same `eth_call` path used to check it
/// (P5.1's second accept criterion), not just via receipt inspection.
#[ic_cdk::update]
async fn read_pending_mint(
    chain_id: u64,
    rpc_url: String,
    pool_address: String,
    nullifier_hex: String,
    finalized: bool,
) -> Result<String, String> {
    let word = eth_call_bytes32(
        chain_id,
        rpc_url,
        &pool_address,
        SEL_PENDING_MINT,
        &parse_bytes32(&nullifier_hex),
        finalized,
    )
    .await?;
    Ok(format!("0x{}", hex::encode(word)))
}

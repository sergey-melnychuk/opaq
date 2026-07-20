// P5.2 (OPAQ.md B.14.6): Solana leg — read side. No DFINITY-maintained
// "Solana RPC canister" exists (unlike the EVM leg's evm-rpc-canister), so
// this canister makes its own HTTPS outcalls against a small RPC set and
// does its own ≥2-of-3 agreement across independent providers.
//
// Verifies a NAMED transaction's own instruction data (B.14.2's fix for two
// real bugs found while scoping this canister): checking bare nullifier-set
// membership can't tell a genuine `xburn` (tag 8) from any other instruction
// that happens to touch the same nullifier (withdraw/transfer/burn all write
// to the same NullifierSet) — "log != transfer". So this checks: the named
// tx succeeded, ITS OWN instruction data starts with tag 8, and that
// instruction's own (nullifier, dest_chain, out_commitment) — not just
// "some pending record exists" — match what the requester claims.
use ic_cdk::management_canister::{
    http_request, HttpHeader, HttpMethod, HttpRequestArgs, HttpRequestResult, TransformArgs,
    TransformContext, TransformFunc,
};

const TAG_XBURN: u8 = 8;
// xburn instruction data layout (programs/opaq/src/lib.rs `fn xburn`):
// tag(1) proof_a(64) proof_b(128) proof_c(64) src_merkle_root(32)
// src_nullifier(32) dest_chain(32) out_commitment(32) = 385 bytes.
const OFF_NULLIFIER: usize = 1 + 64 + 128 + 64 + 32;
const OFF_DEST_CHAIN: usize = OFF_NULLIFIER + 32;
const OFF_OUT_COMMITMENT: usize = OFF_DEST_CHAIN + 32;
const XBURN_DATA_LEN: usize = OFF_OUT_COMMITMENT + 32;

#[ic_cdk::query(hidden = true)]
fn transform_solana_response(args: TransformArgs) -> HttpRequestResult {
    HttpRequestResult {
        status: args.response.status,
        body: args.response.body,
        headers: vec![], // strip Date/etc — must be byte-identical across subnet replicas
    }
}

async fn get_transaction_json(rpc_url: &str, signature: &str) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        // "json" (not "jsonParsed") is enough: instructions come back with
        // programIdIndex + base58 `data` regardless of whether the RPC node
        // recognizes our program — no need to parse the raw wire format.
        "params": [signature, {"encoding": "json", "commitment": "finalized", "maxSupportedTransactionVersion": 0}]
    });
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
            function: TransformFunc::new(
                ic_cdk::api::canister_self(),
                "transform_solana_response".to_string(),
            ),
            context: vec![],
        }),
        is_replicated: None,
    };
    let response = http_request(&request).await.map_err(|e| format!("outcall to {rpc_url}: {e:?}"))?;
    serde_json::from_slice(&response.body).map_err(|e| format!("bad JSON from {rpc_url}: {e}"))
}

/// Queries every URL in `rpc_urls`, requires at least `agree_threshold` of
/// them to return byte-identical JSON (structural equality), and returns
/// that agreed-upon value. A finalized transaction's content is static, so
/// honest providers should always agree exactly — any real disagreement
/// means a stale/lying/malfunctioning provider.
async fn get_transaction_with_agreement(
    rpc_urls: &[String],
    agree_threshold: usize,
    signature: &str,
) -> Result<serde_json::Value, String> {
    let mut responses = Vec::with_capacity(rpc_urls.len());
    for url in rpc_urls {
        match get_transaction_json(url, signature).await {
            Ok(v) => responses.push(v),
            Err(e) => ic_cdk::api::debug_print(format!("solana leg: {url} failed: {e}")),
        }
    }
    for (i, candidate) in responses.iter().enumerate() {
        let agree_count = responses.iter().filter(|r| *r == candidate).count();
        if agree_count >= agree_threshold {
            return Ok(responses[i].clone());
        }
    }
    Err(format!(
        "no {agree_threshold}-of-{} provider agreement ({} responses received)",
        rpc_urls.len(),
        responses.len()
    ))
}

fn parse_bytes32(hex_str: &str) -> Result<[u8; 32], String> {
    let clean = hex_str.trim_start_matches("0x");
    let bytes = hex::decode(clean).map_err(|e| format!("bad hex: {e}"))?;
    bytes.try_into().map_err(|_| "expected exactly 32 bytes".to_string())
}

/// *Accept (P5.2):* a genuine finalized `xburn` tx with matching claims
/// attests; a real, finalized, but wrong-tag tx (e.g. the deposit that
/// preceded it) is rejected regardless of what its data contains; claims
/// that don't match what the named tx actually recorded are rejected; and
/// (via `get_transaction_with_agreement`) a provider that disagrees with the
/// rest is out-voted rather than trusted.
#[ic_cdk::update]
async fn verify_xburn_transaction(
    rpc_urls: Vec<String>,
    agree_threshold: u32,
    program_id_b58: String,
    signature: String,
    nullifier_hex: String,
    dest_chain_hex: String,
    out_commitment_hex: String,
) -> Result<String, String> {
    let expected_nullifier = parse_bytes32(&nullifier_hex)?;
    let expected_dest_chain = parse_bytes32(&dest_chain_hex)?;
    let expected_out_commitment = parse_bytes32(&out_commitment_hex)?;

    let tx = get_transaction_with_agreement(&rpc_urls, agree_threshold as usize, &signature).await?;
    let result = tx.get("result").filter(|r| !r.is_null()).ok_or("transaction not found")?;

    let err = &result["meta"]["err"];
    if !err.is_null() {
        return Err(format!("named transaction did not succeed: {err}"));
    }

    let account_keys: Vec<String> = result["transaction"]["message"]["accountKeys"]
        .as_array()
        .ok_or("missing accountKeys")?
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    let program_index = account_keys
        .iter()
        .position(|k| k == &program_id_b58)
        .ok_or("named transaction never references the opaq program at all")?;

    let instructions = result["transaction"]["message"]["instructions"]
        .as_array()
        .ok_or("missing instructions")?;
    let opaq_ix = instructions
        .iter()
        .find(|ix| ix["programIdIndex"].as_u64() == Some(program_index as u64))
        .ok_or("no instruction in this transaction targets the opaq program")?;

    let data_b58 = opaq_ix["data"].as_str().ok_or("instruction has no data")?;
    let data = bs58::decode(data_b58)
        .into_vec()
        .map_err(|e| format!("bad base58 instruction data: {e}"))?;

    // The specific bug this whole design closes: do NOT accept on bare
    // nullifier-set membership. A withdraw/transfer/burn touching the SAME
    // nullifier is a DIFFERENT instruction (different tag) and must reject
    // here, even though a nullifier-set lookup alone couldn't tell them apart.
    if data.first() != Some(&TAG_XBURN) {
        return Err(format!(
            "opaq instruction in this transaction is tag {:?}, not xburn (tag {TAG_XBURN})",
            data.first()
        ));
    }
    if data.len() != XBURN_DATA_LEN {
        return Err(format!("xburn instruction data wrong length: {}", data.len()));
    }

    let actual_nullifier = &data[OFF_NULLIFIER..OFF_NULLIFIER + 32];
    let actual_dest_chain = &data[OFF_DEST_CHAIN..OFF_DEST_CHAIN + 32];
    let actual_out_commitment = &data[OFF_OUT_COMMITMENT..OFF_OUT_COMMITMENT + 32];

    if actual_nullifier != expected_nullifier {
        return Err("claimed nullifier does not match what this tx actually recorded".to_string());
    }
    if actual_dest_chain != expected_dest_chain {
        return Err("claimed dest_chain does not match what this tx actually recorded".to_string());
    }
    if actual_out_commitment != expected_out_commitment {
        return Err("claimed out_commitment does not match what this tx actually recorded".to_string());
    }

    Ok(format!(
        "attested: {signature} is a finalized xburn recording (nullifier={nullifier_hex}, dest_chain={dest_chain_hex}, out_commitment={out_commitment_hex})"
    ))
}

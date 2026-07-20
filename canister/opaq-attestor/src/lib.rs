// P5.0 accept criterion (OPAQ.md B.14.6): derive an EVM address (threshold-ECDSA,
// secp256k1) and a Solana address (threshold-Schnorr, Ed25519), then produce and
// verify a signature for both key types entirely from within the canister.

mod evm;
mod evm_tx;
mod solana;
mod solana_tx;

use k256::elliptic_curve::sec1::ToEncodedPoint;

use ic_cdk::management_canister::{
    ecdsa_public_key, schnorr_public_key, sign_with_ecdsa, sign_with_schnorr, EcdsaCurve,
    EcdsaKeyId, EcdsaPublicKeyArgs, SchnorrAlgorithm, SchnorrKeyId, SchnorrPublicKeyArgs,
    SignWithEcdsaArgs, SignWithSchnorrArgs,
};

// "dfx_test_key" is the conventional insecure single-node key available on a
// local replica; production deploys must use "key_1" (mainnet, 34-node subnet)
// instead — see OPAQ.md B.14.4.
const LOCAL_KEY_NAME: &str = "dfx_test_key";

// k256/ed25519-dalek pull in getrandom transitively even though every path this
// canister uses (sign_with_ecdsa/sign_with_schnorr via the mgmt canister, plus
// verify) is deterministic and never calls it — wasm32-unknown-unknown has no
// OS RNG, so getrandom needs an explicit (unreachable) stub to link at all.
fn unreachable_getrandom(_buf: &mut [u8]) -> Result<(), getrandom::Error> {
    unreachable!("no code path in this canister calls into getrandom")
}
getrandom::register_custom_getrandom!(unreachable_getrandom);

pub(crate) fn ecdsa_key_id() -> EcdsaKeyId {
    EcdsaKeyId {
        curve: EcdsaCurve::Secp256k1,
        name: LOCAL_KEY_NAME.to_string(),
    }
}

pub(crate) fn schnorr_key_id() -> SchnorrKeyId {
    SchnorrKeyId {
        algorithm: SchnorrAlgorithm::Ed25519,
        name: LOCAL_KEY_NAME.to_string(),
    }
}

/// keccak256(pubkey)[12..] as a 0x-prefixed hex string, same derivation EVM uses.
fn evm_address_from_pubkey(uncompressed_no_prefix: &[u8]) -> String {
    use sha3::{Digest, Keccak256};
    let hash = Keccak256::digest(uncompressed_no_prefix);
    format!("0x{}", hex::encode(&hash[12..]))
}

#[ic_cdk::update]
async fn evm_address() -> String {
    let res = ecdsa_public_key(&EcdsaPublicKeyArgs {
        canister_id: None,
        derivation_path: vec![],
        key_id: ecdsa_key_id(),
    })
    .await
    .expect("ecdsa_public_key failed");
    // SEC1 compressed pubkey -> decompress -> strip the 0x04 prefix for keccak.
    let point = k256::PublicKey::from_sec1_bytes(&res.public_key).expect("bad pubkey");
    let uncompressed = point.to_encoded_point(false);
    evm_address_from_pubkey(&uncompressed.as_bytes()[1..])
}

#[ic_cdk::update]
async fn solana_address() -> String {
    let res = schnorr_public_key(&SchnorrPublicKeyArgs {
        canister_id: None,
        derivation_path: vec![],
        key_id: schnorr_key_id(),
    })
    .await
    .expect("schnorr_public_key failed");
    bs58::encode(&res.public_key).into_string()
}

/// Signs `message` with threshold-ECDSA, then verifies the signature recovers
/// the same address returned by `evm_address()` — proving the mgmt canister's
/// reported key and its signing key are the same key.
#[ic_cdk::update]
async fn evm_sign_and_verify(message: String) -> Result<String, String> {
    use sha3::{Digest, Keccak256};
    let digest = Keccak256::digest(message.as_bytes());

    let pk = ecdsa_public_key(&EcdsaPublicKeyArgs {
        canister_id: None,
        derivation_path: vec![],
        key_id: ecdsa_key_id(),
    })
    .await
    .map_err(|e| format!("ecdsa_public_key: {e:?}"))?;

    let sig = sign_with_ecdsa(&SignWithEcdsaArgs {
        message_hash: digest.to_vec(),
        derivation_path: vec![],
        key_id: ecdsa_key_id(),
    })
    .await
    .map_err(|e| format!("sign_with_ecdsa: {e:?}"))?;

    let verifying_key = k256::ecdsa::VerifyingKey::from_sec1_bytes(&pk.public_key)
        .map_err(|e| format!("bad pubkey: {e}"))?;
    let signature = k256::ecdsa::Signature::from_slice(&sig.signature)
        .map_err(|e| format!("bad signature: {e}"))?;

    // sign_with_ecdsa signs the given 32-byte hash directly (no internal
    // re-hashing) — verify_prehash matches that; plain Verifier::verify would
    // wrongly re-hash `digest` with SHA-256 before checking.
    use k256::ecdsa::signature::hazmat::PrehashVerifier;
    verifying_key
        .verify_prehash(&digest, &signature)
        .map_err(|e| format!("signature did NOT verify: {e}"))?;

    let point = k256::PublicKey::from_sec1_bytes(&pk.public_key).map_err(|e| e.to_string())?;
    let uncompressed = point.to_encoded_point(false);
    let addr = evm_address_from_pubkey(&uncompressed.as_bytes()[1..]);
    Ok(format!(
        "OK: signature verifies against key for address {addr}"
    ))
}

/// Same accept criterion as above, for threshold-Ed25519 (the Solana leg).
#[ic_cdk::update]
async fn solana_sign_and_verify(message: String) -> Result<String, String> {
    let pk = schnorr_public_key(&SchnorrPublicKeyArgs {
        canister_id: None,
        derivation_path: vec![],
        key_id: schnorr_key_id(),
    })
    .await
    .map_err(|e| format!("schnorr_public_key: {e:?}"))?;

    let sig = sign_with_schnorr(&SignWithSchnorrArgs {
        message: message.as_bytes().to_vec(),
        derivation_path: vec![],
        key_id: schnorr_key_id(),
        aux: None,
    })
    .await
    .map_err(|e| format!("sign_with_schnorr: {e:?}"))?;

    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(
        pk.public_key
            .as_slice()
            .try_into()
            .map_err(|_| "bad pubkey length".to_string())?,
    )
    .map_err(|e| format!("bad pubkey: {e}"))?;
    let signature = ed25519_dalek::Signature::from_slice(&sig.signature)
        .map_err(|e| format!("bad signature: {e}"))?;

    use ed25519_dalek::Verifier;
    verifying_key
        .verify(message.as_bytes(), &signature)
        .map_err(|e| format!("signature did NOT verify: {e}"))?;

    Ok(format!(
        "OK: signature verifies against pubkey {}",
        bs58::encode(&pk.public_key).into_string()
    ))
}

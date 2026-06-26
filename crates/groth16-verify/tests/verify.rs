//! M3 off-chain isolation test (OPAQ.md B.6): the same groth16-solana verifier
//! used on-chain must accept a valid deposit proof and reject a tampered one.
//! Fixtures are real snarkjs artifacts from the deposit circuit (deterministic
//! ptau), regenerable via scripts/groth16-prove.sh.

use groth16_solana::groth16::{Groth16Verifier, Groth16Verifyingkey};
use groth16_verify::{proof_from_json, public_from_json, vk_from_json};
use serde_json::Value;

fn load() -> (Value, Value, Value) {
    (
        serde_json::from_str(include_str!("fixtures/deposit/proof.json")).unwrap(),
        serde_json::from_str(include_str!("fixtures/deposit/verification_key.json")).unwrap(),
        serde_json::from_str(include_str!("fixtures/deposit/public.json")).unwrap(),
    )
}

fn verify(proof_a: &[u8; 64], proof_b: &[u8; 128], proof_c: &[u8; 64]) -> bool {
    let (_, vk_json, pub_json) = load();
    let vk = vk_from_json(&vk_json);
    let ic = vk.ic.clone();
    let vk = Groth16Verifyingkey {
        nr_pubinputs: ic.len() - 1,
        vk_alpha_g1: vk.alpha,
        vk_beta_g2: vk.beta,
        vk_gamme_g2: vk.gamma, // sic: upstream field name is misspelled
        vk_delta_g2: vk.delta,
        vk_ic: &ic,
    };
    let pubs = public_from_json(&pub_json);
    let pubs: [[u8; 32]; 3] = [pubs[0], pubs[1], pubs[2]];

    match Groth16Verifier::new(proof_a, proof_b, proof_c, &pubs, &vk) {
        Ok(mut v) => match v.verify() {
            Ok(()) => true,
            Err(e) => {
                eprintln!("verify() error: {e:?}");
                false
            }
        },
        Err(e) => {
            eprintln!("new() error: {e:?}");
            false
        }
    }
}

#[test]
fn valid_deposit_proof_accepts() {
    let (proof_json, _, _) = load();
    let p = proof_from_json(&proof_json);
    assert!(verify(&p.a, &p.b, &p.c), "valid deposit proof must verify on the groth16-solana path");
}

#[test]
fn tampered_proof_rejects() {
    let (proof_json, _, _) = load();
    let p = proof_from_json(&proof_json);
    let mut bad_c = p.c;
    bad_c[63] ^= 0x01; // flip a low bit of C.y
    assert!(!verify(&p.a, &p.b, &bad_c), "tampered proof must be rejected");
}

//! Build an opaq program instruction blob from a circuit's snarkjs proof + the
//! gen_witness sidecar (e2e_values.json). Layout the on-chain program expects:
//!   deposit  (tag 1): proof_a(64) proof_b(128) proof_c(64) mint(32) amount(8 LE) commitment(32)
//!   withdraw (tag 2): proof… merkle_root(32) nullifier(32) mint(32) amount(8 LE) recipient(32)
//!
//! Usage: emit_opaq_instruction <deposit|withdraw> <proof_dir> <e2e_values.json> <out.bin>

use std::{fs, path::PathBuf};

use groth16_verify::proof_from_json;
use serde_json::Value;

fn hex32(s: &str) -> [u8; 32] {
    hex::decode(s).unwrap().try_into().unwrap()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let circuit = a[1].as_str();
    let proof_dir = PathBuf::from(&a[2]);
    let sidecar: Value = serde_json::from_str(&fs::read_to_string(&a[3]).unwrap()).unwrap();
    let out = PathBuf::from(&a[4]);

    let proof_json: Value =
        serde_json::from_str(&fs::read_to_string(proof_dir.join("proof.json")).unwrap()).unwrap();
    let p = proof_from_json(&proof_json);

    let mint = hex32(sidecar["mint_hex"].as_str().unwrap());
    let amount = sidecar["amount"].as_u64().unwrap();
    let commitment = hex32(sidecar["commitment"].as_str().unwrap());
    let nullifier = hex32(sidecar["nullifier"].as_str().unwrap());
    let merkle_root = hex32(sidecar["merkle_root"].as_str().unwrap());
    let recipient = hex32(sidecar["recipient_hex"].as_str().unwrap());

    let mut data = Vec::new();
    match circuit {
        "deposit" => {
            data.push(1u8);
            data.extend_from_slice(&p.a);
            data.extend_from_slice(&p.b);
            data.extend_from_slice(&p.c);
            data.extend_from_slice(&mint);
            data.extend_from_slice(&amount.to_le_bytes());
            data.extend_from_slice(&commitment);
        }
        "withdraw" => {
            data.push(2u8);
            data.extend_from_slice(&p.a);
            data.extend_from_slice(&p.b);
            data.extend_from_slice(&p.c);
            data.extend_from_slice(&merkle_root);
            data.extend_from_slice(&nullifier);
            data.extend_from_slice(&mint);
            data.extend_from_slice(&amount.to_le_bytes());
            data.extend_from_slice(&recipient);
        }
        _ => panic!("circuit must be deposit|withdraw"),
    }
    fs::write(&out, &data).unwrap();
    println!("wrote {} ({} bytes)", out.display(), data.len());
}

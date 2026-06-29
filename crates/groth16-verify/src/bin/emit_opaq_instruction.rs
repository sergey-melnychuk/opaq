//! Build an opaq program instruction blob from a circuit's snarkjs proof + the
//! gen_witness sidecar (e2e_values.json). Layout the on-chain program expects:
//!   deposit  (tag 1): proof_a(64) proof_b(128) proof_c(64) mint(32) amount(8 LE) commitment(32)
//!   withdraw (tag 2): proof… merkle_root(32) nullifier(32) mint(32) amount(8 LE) recipient(32)
//!   transfer (tag 3): proof… merkle_root(32) nullifier0(32) nullifier1(32) commitment0(32) commitment1(32)
//!
//! Usage: emit_opaq_instruction <deposit|withdraw|transfer> <proof_dir> <e2e_values.json> <out.bin>
//! For transfer the sidecar arg is unused — the 5 public inputs ARE the args, read
//! straight from proof_dir/public.json (guaranteed to match what was proven).

use std::{fs, path::PathBuf};

use groth16_verify::{opaq_instruction, proof_from_json, public_from_json, OpaqFields};
use serde_json::Value;

fn hex32(s: &str) -> [u8; 32] {
    hex::decode(s).unwrap().try_into().unwrap()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let circuit = a[1].as_str();
    let proof_dir = PathBuf::from(&a[2]);
    let out = PathBuf::from(&a[4]);

    let proof_json: Value =
        serde_json::from_str(&fs::read_to_string(proof_dir.join("proof.json")).unwrap()).unwrap();
    let p = proof_from_json(&proof_json);

    // transfer's args are exactly its 5 public inputs (merkle_root, nullifier[2],
    // out_commitment[2]) — take them from public.json, no sidecar needed.
    if circuit == "transfer" {
        let public_json: Value =
            serde_json::from_str(&fs::read_to_string(proof_dir.join("public.json")).unwrap()).unwrap();
        let public = public_from_json(&public_json);
        assert_eq!(public.len(), 5, "transfer expects 5 public inputs, got {}", public.len());
        let mut data = Vec::with_capacity(1 + 256 + 5 * 32);
        data.push(3u8);
        data.extend_from_slice(&p.a);
        data.extend_from_slice(&p.b);
        data.extend_from_slice(&p.c);
        for x in &public {
            data.extend_from_slice(x);
        }
        fs::write(&out, &data).unwrap();
        println!("wrote {} ({} bytes)", out.display(), data.len());
        return;
    }

    let sidecar: Value = serde_json::from_str(&fs::read_to_string(&a[3]).unwrap()).unwrap();
    let fields = OpaqFields {
        mint: hex32(sidecar["mint_hex"].as_str().unwrap()),
        amount: sidecar["amount"].as_u64().unwrap(),
        commitment: hex32(sidecar["commitment"].as_str().unwrap()),
        nullifier: hex32(sidecar["nullifier"].as_str().unwrap()),
        merkle_root: hex32(sidecar["merkle_root"].as_str().unwrap()),
        recipient: hex32(sidecar["recipient_hex"].as_str().unwrap()), // burn: dest_address
        dest_chain: sidecar["dest_chain"].as_str().map(hex32).unwrap_or_default(),
    };

    let data = opaq_instruction(circuit, &p, &fields).unwrap();
    fs::write(&out, &data).unwrap();
    println!("wrote {} ({} bytes)", out.display(), data.len());
}

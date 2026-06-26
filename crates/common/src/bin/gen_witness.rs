//! M1 witness generator: writes deterministic `Prover.toml` files for the
//! deposit and withdraw circuits so they can be proved/verified locally with bb.
//!
//! All field values are computed with `light-poseidon` — already proven
//! byte-identical to the circuit's Poseidon in M0 — so a successful
//! `nargo execute` (which re-checks every assert) is itself cross-validation
//! that the off-chain witness math matches the in-circuit math.
//!
//! Usage: cargo run -p opaq-common --bin gen_witness -- <circuits_dir>

use std::{fs, path::PathBuf};

use opaq_common::{be32, field_hex, merkle_root_be, poseidon_be, to_field_be};

const TREE_DEPTH: usize = 24;

fn main() {
    let circuits_dir = PathBuf::from(
        std::env::args().nth(1).unwrap_or_else(|| "circuits".to_string()),
    );

    // Deterministic note secrets (M1 fixtures, not real randomness).
    let spend_key = be32(987_654_321);
    let blinding = be32(123_456_789);
    let amount = be32(100);
    let token_id = to_field_be(&dummy_pubkey(0xAB)); // canonical mint encoding
    let recipient = to_field_be(&dummy_pubkey(0xCD)); // canonical recipient encoding

    // Derivations matching the circuits.
    let owner_pubkey = poseidon_be(&[spend_key]); // hash_1([spend_key])
    let commitment = poseidon_be(&[token_id, amount, owner_pubkey, blinding]); // hash_4
    let nullifier = poseidon_be(&[commitment, spend_key]); // hash_2([commitment, spend_key])

    // --- deposit/Prover.toml ---
    let deposit = format!(
        "token_id = \"{}\"\n\
         amount = \"{}\"\n\
         new_commitment = \"{}\"\n\
         owner_pubkey = \"{}\"\n\
         blinding_factor = \"{}\"\n",
        field_hex(&token_id),
        field_hex(&amount),
        field_hex(&commitment),
        field_hex(&owner_pubkey),
        field_hex(&blinding),
    );
    write(&circuits_dir, "deposit", &deposit);
    write_json(
        &circuits_dir,
        "deposit",
        &[
            ("token_id", field_hex(&token_id)),
            ("amount", field_hex(&amount)),
            ("new_commitment", field_hex(&commitment)),
            ("owner_pubkey", field_hex(&owner_pubkey)),
            ("blinding_factor", field_hex(&blinding)),
        ],
        &[],
    );

    // --- withdraw/Prover.toml ---
    // Real empty-tree state: leaf at index 0, all-left path, siblings = the
    // empty-subtree hashes (zero_hashes), NOT all-zero. This makes the proof's
    // merkle_root equal the root CommitmentTree produces after inserting the
    // commitment at index 0 (cross-checked by the M4 tree tests) — so the
    // circuit and the on-chain tree agree (needed for M6/M7).
    let siblings = opaq_common::tree::zero_hashes(&|a: &[u8; 32], b: &[u8; 32]| {
        opaq_common::poseidon_hash2_be(a, b)
    });
    let right = [false; TREE_DEPTH];
    let merkle_root = merkle_root_be(commitment, &siblings, &right);

    let path_list = siblings
        .iter()
        .map(|s| format!("\"{}\"", field_hex(s)))
        .collect::<Vec<_>>()
        .join(", ");
    let idx_list = right
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let withdraw = format!(
        "merkle_root = \"{}\"\n\
         nullifier = \"{}\"\n\
         token_id = \"{}\"\n\
         amount = \"{}\"\n\
         recipient = \"{}\"\n\
         spend_key = \"{}\"\n\
         blinding_factor = \"{}\"\n\
         merkle_path = [{}]\n\
         merkle_path_indices = [{}]\n",
        field_hex(&merkle_root),
        field_hex(&nullifier),
        field_hex(&token_id),
        field_hex(&amount),
        field_hex(&recipient),
        field_hex(&spend_key),
        field_hex(&blinding),
        path_list,
        idx_list,
    );
    write(&circuits_dir, "withdraw", &withdraw);
    write_json(
        &circuits_dir,
        "withdraw",
        &[
            ("merkle_root", field_hex(&merkle_root)),
            ("nullifier", field_hex(&nullifier)),
            ("token_id", field_hex(&token_id)),
            ("amount", field_hex(&amount)),
            ("recipient", field_hex(&recipient)),
            ("spend_key", field_hex(&spend_key)),
            ("blinding_factor", field_hex(&blinding)),
        ],
        &[
            // pre-formatted JSON: Field elements are quoted "0x.." strings,
            // bools are raw JSON true/false (the ABI parser wants real booleans).
            ("merkle_path", siblings.iter().map(|s| format!("\"{}\"", field_hex(s))).collect()),
            ("merkle_path_indices", right.iter().map(|b| b.to_string()).collect()),
        ],
    );

    println!("commitment = {}", field_hex(&commitment));
    println!("nullifier  = {}", field_hex(&nullifier));
    println!("root       = {}", field_hex(&merkle_root));
}

fn dummy_pubkey(seed: u8) -> [u8; 32] {
    let mut b = [0u8; 32];
    for (i, x) in b.iter_mut().enumerate() {
        *x = seed.wrapping_add(i as u8);
    }
    b
}

fn write(circuits_dir: &std::path::Path, circuit: &str, contents: &str) {
    let path = circuits_dir.join(circuit).join("Prover.toml");
    fs::write(&path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    println!("wrote {}", path.display());
}

/// ABI-shaped `inputs.json` for the Noir->Groth16 pipeline (noir-cli interop):
/// a flat map of input name -> "0x.." string, with array inputs as JSON arrays.
fn write_json(
    circuits_dir: &std::path::Path,
    circuit: &str,
    scalars: &[(&str, String)],
    arrays: &[(&str, Vec<String>)],
) {
    let mut entries: Vec<String> = scalars
        .iter()
        .map(|(k, v)| format!("\"{k}\":\"{v}\""))
        .collect();
    for (k, vs) in arrays {
        // elements are already JSON-formatted by the caller (quoted or raw)
        let arr = vs.join(",");
        entries.push(format!("\"{k}\":[{arr}]"));
    }
    let json = format!("{{{}}}", entries.join(","));
    let path = circuits_dir.join(circuit).join("inputs.json");
    fs::write(&path, json).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    println!("wrote {}", path.display());
}

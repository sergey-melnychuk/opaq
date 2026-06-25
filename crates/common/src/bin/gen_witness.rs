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

    // --- withdraw/Prover.toml ---
    // Simple valid path: leaf at index 0, all-left, zero siblings. The root is
    // whatever this folds to — a perfectly valid membership witness for M1.
    let siblings = [[0u8; 32]; TREE_DEPTH];
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

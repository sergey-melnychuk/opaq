// Opaq xburn circuit (OPAQ.md B.12.2, Phase 4) — symmetric cross-chain burn.
//
// Generalizes `burn.nr` for the symmetric shielded bridge: instead of binding a
// public `(dest_chain, dest_address)` and leaving value locked for a balance-
// ledger mint (OpaqMint), it binds a public `out_commitment` — a full note ready
// to be inserted directly into the DESTINATION chain's shielded pool tree. The
// source chain records `src_nullifier` (this circuit's proof, verified there);
// the destination chain verifies the SAME proof and inserts `out_commitment`
// (B.12.3). token_id/amount stay private end-to-end (only conserved, never
// revealed) and `dest_owner_pubkey`/`dest_blinding` let the destination note
// have a fresh owner distinct from the source spend_key.
use poseidon::poseidon::bn254::{hash_1, hash_2, hash_4};

global TREE_DEPTH: u32 = 24;

fn main(
    src_merkle_root: pub Field, // a known recent root of the SOURCE tree
    src_nullifier: pub Field,   // recorded on source; attested to the dest
    dest_chain: pub Field,      // destination chain id (bound; can't be redirected)
    out_commitment: pub Field,  // note to insert on the DESTINATION tree
    token_id: Field,
    amount: Field,
    src_spend_key: Field,
    src_blinding: Field,
    src_merkle_path: [Field; TREE_DEPTH],
    src_merkle_path_indices: [bool; TREE_DEPTH], // false = current is left, true = right
    dest_owner_pubkey: Field,
    dest_blinding: Field,
) {
    // Load-bearing for soundness (B.4.1): field arithmetic wraps mod p, so an
    // unconstrained amount could forge value once conserved across the bridge.
    amount.assert_max_bit_size::<64>();

    let src_owner = hash_1([src_spend_key]);
    let src_commitment = hash_4([token_id, amount, src_owner, src_blinding]);

    // Merkle membership on the source tree: fold the commitment up to the root.
    let mut current = src_commitment;
    for i in 0..TREE_DEPTH {
        let sibling = src_merkle_path[i];
        current = if src_merkle_path_indices[i] {
            hash_2([sibling, current])
        } else {
            hash_2([current, sibling])
        };
    }
    assert(current == src_merkle_root);

    // Nullifier binds the burned source note; recorded on the source chain to
    // prevent double-burn, and re-checked (via attestation) on the destination
    // to gate the mint — the two chains enforce it independently (A.9).
    let computed_nullifier = hash_2([src_commitment, src_spend_key]);
    assert(computed_nullifier == src_nullifier);

    // Same token_id + amount as the source note ⇒ asset and value are
    // conserved across the bridge, both still hidden from the public inputs.
    let computed_out = hash_4([token_id, amount, dest_owner_pubkey, dest_blinding]);
    assert(computed_out == out_commitment);

    // Keep dest_chain in the witness so it stays a verified public input (a
    // relayer/submitter can't redirect the mint to a different chain).
    std::as_witness(dest_chain);
}

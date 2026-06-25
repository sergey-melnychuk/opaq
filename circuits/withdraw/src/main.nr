// Opaq withdraw circuit (OPAQ.md B.4.2).
//
// Proves: a commitment encoding (token_id, amount, owner_pubkey, blinding) is a
// leaf in the tree at `merkle_root`, the prover knows the spend_key behind
// owner_pubkey, and `nullifier` is the correct nullifier for that note — all
// without revealing which leaf. `recipient` is a bound public input so a
// submitter/relayer can't redirect funds.
//
// Field-encoding note (B.4.2): `token_id` and `recipient` are canonical
// to_field(32-byte) Poseidon encodings, computed identically off-chain and
// on-chain — NOT raw casts (a 32-byte Pubkey overflows the BN254 field).
use poseidon::poseidon::bn254::{hash_1, hash_2, hash_4};

global TREE_DEPTH: u32 = 24;

fn main(
    merkle_root: pub Field,
    nullifier: pub Field,
    token_id: pub Field,
    amount: pub Field,
    recipient: pub Field, // to_field(recipient_pubkey); bound, not algebraically constrained
    spend_key: Field,
    blinding_factor: Field,
    merkle_path: [Field; TREE_DEPTH],
    merkle_path_indices: [bool; TREE_DEPTH], // false = current is left, true = current is right
) {
    amount.assert_max_bit_size::<64>();

    let owner_pubkey = hash_1([spend_key]);
    let commitment = hash_4([token_id, amount, owner_pubkey, blinding_factor]);

    // Merkle membership: fold the commitment up to the root.
    let mut current = commitment;
    for i in 0..TREE_DEPTH {
        let sibling = merkle_path[i];
        current = if merkle_path_indices[i] {
            hash_2([sibling, current])
        } else {
            hash_2([current, sibling])
        };
    }
    assert(current == merkle_root);

    // Nullifier binds the spent note; recorded on-chain to prevent double-spend.
    let computed_nullifier = hash_2([commitment, spend_key]);
    assert(computed_nullifier == nullifier);

    // Keep `recipient` in the witness so it remains a verified public input.
    std::as_witness(recipient);
}

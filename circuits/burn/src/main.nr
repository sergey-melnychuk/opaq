// Opaq burn circuit (OPAQ.md A.6 Phase 3) — cross-chain burn.
//
// Like `withdraw`, but instead of releasing SPL to a Solana recipient it BURNS
// the note (records the nullifier on Solana, value stays locked in the vault) and
// binds an EVM mint destination — `(dest_chain, dest_address)` — as public inputs.
// An EVM mint contract verifies this same Groth16 proof and mints the equivalent
// asset to `dest_address`, checking the nullifier against ITS OWN set (the two
// chains don't share state, so the nullifier is enforced independently on each —
// OPAQ.md A.9). `dest_chain`/`dest_address` are bound so a relayer can't redirect
// the mint.
//
// Field encoding (B.4.2): `token_id` is the canonical to_field(mint). An EVM
// address is 20 bytes (< 254 bits) so `dest_address` fits a Field directly;
// `dest_chain` is a small chain id. The proof binds them as public inputs.
use poseidon::poseidon::bn254::{hash_1, hash_2, hash_4};

global TREE_DEPTH: u32 = 24;

fn main(
    merkle_root: pub Field,
    nullifier: pub Field,
    token_id: pub Field,
    amount: pub Field,
    dest_chain: pub Field,   // EVM chain id (bound)
    dest_address: pub Field, // EVM recipient address as a field (bound)
    spend_key: Field,
    blinding_factor: Field,
    merkle_path: [Field; TREE_DEPTH],
    merkle_path_indices: [bool; TREE_DEPTH], // false = current is left, true = right
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

    // Nullifier binds the burned note; recorded on-chain to prevent double-burn,
    // and re-checked on the EVM side to prevent double-mint.
    let computed_nullifier = hash_2([commitment, spend_key]);
    assert(computed_nullifier == nullifier);

    // Keep the EVM destination in the witness so they remain verified public
    // inputs (a relayer/submitter can't redirect the mint).
    std::as_witness(dest_chain);
    std::as_witness(dest_address);
}

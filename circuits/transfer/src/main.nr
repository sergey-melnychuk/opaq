// Opaq transfer circuit (OPAQ.md B.4.3 / A.6 Phase 2) — 2-in/2-out join-split.
//
// Spends 2 input notes and creates 2 output notes ENTIRELY INSIDE the pool (no
// SPL movement), hiding BOTH the amounts and the token: `token_id` and every
// amount are private. The only public inputs are the Merkle root, the 2 input
// nullifiers, and the 2 output commitments — so the anonymity set is every
// transfer, not just identical-(token,amount) ones (closes A.12).
//
// Conserves value: sum(inputs) == sum(outputs), with every amount range-checked
// to 64 bits. The range checks are load-bearing for SOUNDNESS — field arithmetic
// wraps mod p, so an unchecked amount near p could forge value via underflow
// (B.4.1). All 4 notes must share one private `token_id` (enforced by reuse in
// every commitment), so a transfer can't mint one token from another.
//
// A "dummy" input (is_dummy = true) lets a transfer spend fewer than 2 real notes
// (e.g. send a single note): it must have amount 0 and skips Merkle membership.
// Its nullifier is still bound to its (caller-chosen, fresh-blinded) commitment,
// so it can't collide with or pre-empt any real note's nullifier (Poseidon
// preimage resistance), and recording it on-chain is harmless.
use poseidon::poseidon::bn254::{hash_1, hash_2, hash_4};

global TREE_DEPTH: u32 = 24;
global N_IN: u32 = 2;
global N_OUT: u32 = 2;

fn main(
    merkle_root: pub Field,
    nullifiers: pub [Field; N_IN],
    out_commitments: pub [Field; N_OUT],
    token_id: Field, // private: shared by all 4 notes
    in_amount: [Field; N_IN],
    in_spend_key: [Field; N_IN],
    in_blinding: [Field; N_IN],
    in_is_dummy: [bool; N_IN],
    in_merkle_path: [[Field; TREE_DEPTH]; N_IN],
    in_merkle_path_indices: [[bool; TREE_DEPTH]; N_IN],
    out_amount: [Field; N_OUT],
    out_owner_pubkey: [Field; N_OUT], // recipient's hash_1(spend_key); sender knows it
    out_blinding: [Field; N_OUT],
) {
    let mut in_sum: Field = 0;
    for i in 0..N_IN {
        in_amount[i].assert_max_bit_size::<64>();

        let owner = hash_1([in_spend_key[i]]);
        let commitment = hash_4([token_id, in_amount[i], owner, in_blinding[i]]);

        // Fold the commitment up to the root along the supplied path.
        let mut current = commitment;
        for d in 0..TREE_DEPTH {
            let sibling = in_merkle_path[i][d];
            current = if in_merkle_path_indices[i][d] {
                hash_2([sibling, current])
            } else {
                hash_2([current, sibling])
            };
        }

        // Real inputs must be a leaf at merkle_root; dummies must be value 0 and
        // skip membership (a 0-note isn't in the tree).
        if in_is_dummy[i] {
            assert(in_amount[i] == 0);
        } else {
            assert(current == merkle_root);
        }

        // Nullifier binds the spent note (real or dummy) and is recorded on-chain.
        let computed_nullifier = hash_2([commitment, in_spend_key[i]]);
        assert(computed_nullifier == nullifiers[i]);

        in_sum += in_amount[i];
    }

    let mut out_sum: Field = 0;
    for j in 0..N_OUT {
        out_amount[j].assert_max_bit_size::<64>();
        let commitment = hash_4([token_id, out_amount[j], out_owner_pubkey[j], out_blinding[j]]);
        assert(commitment == out_commitments[j]);
        out_sum += out_amount[j];
    }

    // Value conservation. Both sums are <= 2*2^64 < p (no field wraparound given
    // the range checks above), so this equality is exact.
    assert(in_sum == out_sum);
}

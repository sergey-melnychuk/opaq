// Opaq deposit circuit (OPAQ.md B.4.1).
//
// Binds the publicly-deposited (token_id, amount) to the commitment inserted
// into the tree, while keeping owner_pubkey and blinding_factor private. The
// on-chain `deposit` instruction MUST verify this proof and assert its public
// token_id/amount/new_commitment equal the instruction args + transferred
// amount — otherwise a depositor can commit to more than they transferred and
// drain the vault (see B.5.2).
//
// Field-encoding note (B.4.2): `token_id` is the canonical to_field(mint)
// 2-limb Poseidon encoding, computed identically off-chain and on-chain — NOT a
// raw 32-byte cast (a Pubkey overflows the BN254 field).
use poseidon::poseidon::bn254::hash_4;

fn main(
    token_id: pub Field,
    amount: pub Field,
    new_commitment: pub Field,
    owner_pubkey: Field,
    blinding_factor: Field,
) {
    // u64 range: on-chain amounts are u64 and field arithmetic wraps mod p.
    amount.assert_max_bit_size::<64>();

    let computed = hash_4([token_id, amount, owner_pubkey, blinding_factor]);
    assert(computed == new_commitment);
}

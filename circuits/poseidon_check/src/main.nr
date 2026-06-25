// M0 Poseidon parity spike — OPAQ.md B.0.
// Hash fixed field elements with poseidon::bn254::hash_2 (ORIGINAL Poseidon,
// Circom params) and print the results, so they can be compared byte-for-byte
// against light-poseidon (Rust) and the solana_poseidon syscall.
//
// Run:  nargo test --show-output
use poseidon::poseidon::bn254::hash_2;

fn main(a: Field, b: Field) -> pub Field {
    hash_2([a, b])
}

#[test]
fn print_parity_vectors() {
    // Several input combinations, not just one (B.0 / Test 0).
    println(hash_2([1, 2]));
    println(hash_2([0, 0]));
    println(hash_2([
        0x0000000000000000000000000000000000000000000000000000000000000003,
        0x0000000000000000000000000000000000000000000000000000000000000004,
    ]));
    // A large value near the field boundary to catch byte-order / reduction bugs.
    println(hash_2([
        0x1111111111111111111111111111111111111111111111111111111111111111,
        0x2222222222222222222222222222222222222222222222222222222222222222,
    ]));
}

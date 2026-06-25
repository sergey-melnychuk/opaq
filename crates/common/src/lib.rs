//! Shared Poseidon helpers for Opaq. Seeded by the M0 parity spike (OPAQ.md B.0):
//! the off-chain prover (`light-poseidon`) and the on-chain program
//! (`solana-poseidon` syscall) MUST agree byte-for-byte with the Noir circuit's
//! `poseidon::bn254::hash_2`. If they ever diverge, no root/nullifier matches.

use light_poseidon::{Poseidon, PoseidonBytesHasher};
use ark_bn254::Fr;

/// Original Poseidon (Circom BN254 params) over two 32-byte big-endian field
/// elements, returning a 32-byte big-endian field element — matching the
/// circuit's `poseidon::bn254::hash_2` and the Solana `sol_poseidon` syscall.
pub fn poseidon_hash2_be(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Poseidon::<Fr>::new_circom(2).expect("circom poseidon(2) params");
    hasher
        .hash_bytes_be(&[a, b])
        .expect("poseidon hash_bytes_be")
}

/// 32-byte big-endian representation of a small integer (test/helper).
pub fn be32(n: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&n.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_poseidon::{hashv, Endianness, Parameters};

    fn parse_hex32(s: &str) -> [u8; 32] {
        let bytes = hex::decode(s.trim_start_matches("0x")).expect("hex");
        let mut out = [0u8; 32];
        out[32 - bytes.len()..].copy_from_slice(&bytes);
        out
    }

    /// M0: light-poseidon (off-chain prover path) must equal the solana-poseidon
    /// crate (on-chain syscall path) for every vector. Prints each result so it
    /// can also be diffed against the Noir `nargo test --show-output` values.
    #[test]
    fn parity_light_vs_solana() {
        let vectors: [(&[u8; 32], &[u8; 32]); 4] = [
            // mirror circuits/poseidon_check/src/main.nr
            (&be32(1), &be32(2)),
            (&be32(0), &be32(0)),
            (&be32(3), &be32(4)),
            (
                &parse_hex32("1111111111111111111111111111111111111111111111111111111111111111"),
                &parse_hex32("2222222222222222222222222222222222222222222222222222222222222222"),
            ),
        ];

        for (a, b) in vectors {
            let light = poseidon_hash2_be(a, b);
            let sol = hashv(Parameters::Bn254X5, Endianness::BigEndian, &[a, b])
                .expect("solana poseidon")
                .to_bytes();
            assert_eq!(
                light, sol,
                "light-poseidon vs solana-poseidon mismatch for inputs {} / {}",
                hex::encode(a),
                hex::encode(b)
            );
            println!("hash2({}, {}) = 0x{}", hex::encode(a), hex::encode(b), hex::encode(light));
        }
    }
}

//! Shared Poseidon helpers for Opaq. Seeded by the M0 parity spike (OPAQ.md B.0):
//! the off-chain prover (`light-poseidon`) and the on-chain program
//! (`solana-poseidon` syscall) MUST agree byte-for-byte with the Noir circuit's
//! `poseidon::bn254::hash_2`. If they ever diverge, no root/nullifier matches.

use light_poseidon::{Poseidon, PoseidonBytesHasher};
use ark_bn254::Fr;

/// Original Poseidon (Circom BN254 params) over N 32-byte big-endian field
/// elements, returning a 32-byte big-endian field element — matching the
/// circuit's `poseidon::poseidon::bn254::hash_N` and the Solana `sol_poseidon`
/// syscall (verified byte-identical in M0, see tests + scripts/m0-onchain.sh).
pub fn poseidon_be(inputs: &[[u8; 32]]) -> [u8; 32] {
    let refs: Vec<&[u8]> = inputs.iter().map(|x| x.as_slice()).collect();
    let mut hasher = Poseidon::<Fr>::new_circom(inputs.len()).expect("circom poseidon params");
    hasher.hash_bytes_be(&refs).expect("poseidon hash_bytes_be")
}

/// Two-input Poseidon (kept for the M0 parity test's explicit name).
pub fn poseidon_hash2_be(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    poseidon_be(&[*a, *b])
}

/// Canonical field encoding of a 32-byte value (OPAQ.md B.4.2): a Pubkey/mint is
/// 256 bits and overflows the BN254 field, so split into two 128-bit big-endian
/// limbs and Poseidon-hash them. Used for `token_id` and `recipient`.
pub fn to_field_be(bytes32: &[u8; 32]) -> [u8; 32] {
    let mut hi = [0u8; 32];
    hi[16..].copy_from_slice(&bytes32[0..16]);
    let mut lo = [0u8; 32];
    lo[16..].copy_from_slice(&bytes32[16..32]);
    poseidon_be(&[hi, lo])
}

/// Fold a leaf up a Merkle path to its root. `right[i] == true` means the
/// running hash is the right child at level i (sibling on the left).
pub fn merkle_root_be(leaf: [u8; 32], siblings: &[[u8; 32]], right: &[bool]) -> [u8; 32] {
    let mut cur = leaf;
    for (sib, &is_right) in siblings.iter().zip(right) {
        cur = if is_right {
            poseidon_be(&[*sib, cur])
        } else {
            poseidon_be(&[cur, *sib])
        };
    }
    cur
}

/// Format a field element as a `0x`-prefixed hex string for Noir `Prover.toml`.
pub fn field_hex(bytes32: &[u8; 32]) -> String {
    format!("0x{}", hex::encode(bytes32))
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

//! snarkjs (circom-style) BN254 Groth16 → groth16-solana byte layout.
//!
//! Conventions required by groth16-solana / EVM (OPAQ.md B.6):
//! - field coordinates are 32-byte big-endian;
//! - G1 = x‖y (64 bytes); G2 uses imaginary-part-first ordering x_c1‖x_c0‖y_c1‖y_c0 (128);
//! - proof_a's y is negated (the pairing check uses −A; the crate expects −A pre-applied).

use ark_bn254::{Fq, Fr};
use ark_ff::{BigInteger, Field, PrimeField, Zero};
use serde_json::Value;
use std::str::FromStr;

pub struct ProofBytes {
    pub a: [u8; 64],
    pub b: [u8; 128],
    pub c: [u8; 64],
}

pub struct VkBytes {
    pub alpha: [u8; 64],
    pub beta: [u8; 128],
    pub gamma: [u8; 128],
    pub delta: [u8; 128],
    pub ic: Vec<[u8; 64]>,
}

fn to32(v: Vec<u8>) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[32 - v.len()..].copy_from_slice(&v);
    o
}

fn fq(v: &str) -> Fq {
    Fq::from_str(v).expect("Fq")
}
fn fq_bytes(f: Fq) -> [u8; 32] {
    to32(f.into_bigint().to_bytes_be())
}
fn fr_be(v: &str) -> [u8; 32] {
    to32(Fr::from_str(v).expect("Fr").into_bigint().to_bytes_be())
}
fn s(v: &Value) -> &str {
    v.as_str().expect("string field element")
}

/// G1 point `[X, Y, Z]` (snarkjs projective) → 64 bytes BE (x‖y), optionally
/// negating y. Z==0 is the point at infinity, encoded as all-zeros (EIP-197);
/// finite points are normalized x=X/Z, y=Y/Z (snarkjs emits Z=1 for these).
fn g1(p: &Value, negate_y: bool) -> [u8; 64] {
    let p = p.as_array().expect("G1 array");
    let mut o = [0u8; 64];
    let z = fq(s(&p[2]));
    if z.is_zero() {
        return o; // point at infinity
    }
    let zi = z.inverse().expect("z inverse");
    let x = fq(s(&p[0])) * zi;
    let mut y = fq(s(&p[1])) * zi;
    if negate_y {
        y = -y;
    }
    o[..32].copy_from_slice(&fq_bytes(x));
    o[32..].copy_from_slice(&fq_bytes(y));
    o
}

/// G2 point `[[x_c0,x_c1],[y_c0,y_c1],[z_c0,z_c1]]` → 128 bytes BE in EIP-197
/// (imaginary-first) order x_c1‖x_c0‖y_c1‖y_c0 — the layout Solana's alt_bn128
/// pairing decodes (verified against solana-bn254 PodG2::from_be_bytes).
/// snarkjs emits Z=[1,0] for finite points and [0,0] for infinity.
fn g2(p: &Value) -> [u8; 128] {
    let p = p.as_array().expect("G2 array");
    let mut o = [0u8; 128];
    let z = p[2].as_array().expect("G2.z");
    if fq(s(&z[0])).is_zero() && fq(s(&z[1])).is_zero() {
        return o; // point at infinity
    }
    let x = p[0].as_array().expect("G2.x");
    let y = p[1].as_array().expect("G2.y");
    o[0..32].copy_from_slice(&fq_bytes(fq(s(&x[1])))); // x_c1
    o[32..64].copy_from_slice(&fq_bytes(fq(s(&x[0])))); // x_c0
    o[64..96].copy_from_slice(&fq_bytes(fq(s(&y[1])))); // y_c1
    o[96..128].copy_from_slice(&fq_bytes(fq(s(&y[0])))); // y_c0
    o
}

pub fn proof_from_json(v: &Value) -> ProofBytes {
    ProofBytes {
        a: g1(&v["pi_a"], true), // negate y
        b: g2(&v["pi_b"]),
        c: g1(&v["pi_c"], false),
    }
}

pub fn vk_from_json(v: &Value) -> VkBytes {
    VkBytes {
        alpha: g1(&v["vk_alpha_1"], false),
        beta: g2(&v["vk_beta_2"]),
        gamma: g2(&v["vk_gamma_2"]),
        delta: g2(&v["vk_delta_2"]),
        ic: v["IC"].as_array().expect("IC").iter().map(|p| g1(p, false)).collect(),
    }
}

pub fn public_from_json(v: &Value) -> Vec<[u8; 32]> {
    v.as_array().expect("public array").iter().map(|x| fr_be(s(x))).collect()
}

/// Public-input fields for an opaq program instruction. Deposit uses
/// (mint, amount, commitment); withdraw uses (merkle_root, nullifier, mint,
/// amount, recipient); burn uses (merkle_root, nullifier, mint, amount,
/// dest_chain, recipient=dest_address). Unused fields are ignored.
#[derive(Default)]
pub struct OpaqFields {
    pub mint: [u8; 32],
    pub amount: u64,
    pub commitment: [u8; 32],
    pub nullifier: [u8; 32],
    pub merkle_root: [u8; 32],
    pub recipient: [u8; 32],
    pub dest_chain: [u8; 32],
}

/// Build the instruction data the opaq program expects (single source of truth
/// for the on-chain layout in programs/opaq/src/lib.rs):
///   deposit  (tag 1): proof_a(64) proof_b(128) proof_c(64) mint(32) amount(8 LE) commitment(32)
///   withdraw (tag 2): proof… merkle_root(32) nullifier(32) mint(32) amount(8 LE) recipient(32)
///   burn     (tag 4): proof… merkle_root(32) nullifier(32) mint(32) amount(8 LE) dest_chain(32) dest_address(32)
pub fn opaq_instruction(circuit: &str, p: &ProofBytes, f: &OpaqFields) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    match circuit {
        "deposit" => {
            data.push(1u8);
            data.extend_from_slice(&p.a);
            data.extend_from_slice(&p.b);
            data.extend_from_slice(&p.c);
            data.extend_from_slice(&f.mint);
            data.extend_from_slice(&f.amount.to_le_bytes());
            data.extend_from_slice(&f.commitment);
        }
        "withdraw" => {
            data.push(2u8);
            data.extend_from_slice(&p.a);
            data.extend_from_slice(&p.b);
            data.extend_from_slice(&p.c);
            data.extend_from_slice(&f.merkle_root);
            data.extend_from_slice(&f.nullifier);
            data.extend_from_slice(&f.mint);
            data.extend_from_slice(&f.amount.to_le_bytes());
            data.extend_from_slice(&f.recipient);
        }
        "burn" => {
            data.push(4u8);
            data.extend_from_slice(&p.a);
            data.extend_from_slice(&p.b);
            data.extend_from_slice(&p.c);
            data.extend_from_slice(&f.merkle_root);
            data.extend_from_slice(&f.nullifier);
            data.extend_from_slice(&f.mint);
            data.extend_from_slice(&f.amount.to_le_bytes());
            data.extend_from_slice(&f.dest_chain);
            data.extend_from_slice(&f.recipient); // dest_address
        }
        other => return Err(format!("circuit must be deposit|withdraw|burn, got {other}")),
    }
    Ok(data)
}

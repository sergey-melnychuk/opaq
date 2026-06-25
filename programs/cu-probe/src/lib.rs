//! M0.5 CU probe (OPAQ.md B.6). Instruction data: `[mode(1)] [count u32 LE(4)]`.
//!   mode 0: N × alt_bn128 G1 scalar multiplications
//!   mode 1: N × alt_bn128 G1 additions
//!   mode 3: N × BN254 Fr multiplications (pure BPF, ark-ff) — the Honk sumcheck cost driver
//! Measure `computeUnitsConsumed` for N and 2N; the slope is the per-op CU.

use solana_program::{
    account_info::AccountInfo, entrypoint, entrypoint::ProgramResult,
    program::set_return_data, program_error::ProgramError, pubkey::Pubkey,
};
use solana_bn254::prelude::{alt_bn128_addition, alt_bn128_multiplication};

use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};

entrypoint!(process_instruction);

fn be32(n: u64) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[24..].copy_from_slice(&n.to_be_bytes());
    b
}

pub fn process_instruction(_id: &Pubkey, _accts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    if data.len() < 5 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let mode = data[0];
    let n = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);

    // Fixed valid inputs (big-endian, EVM-style). G1 generator = (1, 2).
    let g1 = {
        let mut p = [0u8; 64];
        p[..32].copy_from_slice(&be32(1));
        p[32..].copy_from_slice(&be32(2));
        p
    };
    let mut sink = [0u8; 32];

    match mode {
        0 => {
            let mut input = [0u8; 96]; // G1 || scalar
            input[..64].copy_from_slice(&g1);
            input[64..].copy_from_slice(&be32(7));
            for _ in 0..n {
                let out = alt_bn128_multiplication(&input)
                    .map_err(|_| ProgramError::InvalidInstructionData)?;
                sink[0] ^= out[0];
            }
        }
        1 => {
            let mut input = [0u8; 128]; // G1 || G1
            input[..64].copy_from_slice(&g1);
            input[64..].copy_from_slice(&g1);
            for _ in 0..n {
                let out = alt_bn128_addition(&input)
                    .map_err(|_| ProgramError::InvalidInstructionData)?;
                sink[0] ^= out[0];
            }
        }
        3 => {
            let mut acc = Fr::from(3u64);
            let m = Fr::from(7u64);
            for _ in 0..n {
                acc = core::hint::black_box(acc * m);
            }
            let bytes = acc.into_bigint().to_bytes_be();
            sink[..bytes.len().min(32)].copy_from_slice(&bytes[..bytes.len().min(32)]);
        }
        _ => return Err(ProgramError::InvalidInstructionData),
    }

    set_return_data(&sink);
    Ok(())
}

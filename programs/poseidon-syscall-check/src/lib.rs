//! M0 syscall parity program (OPAQ.md B.0 step 4).
//!
//! Instruction data layout: `a(32) || b(32) || expected(32)` (96 bytes).
//! Computes `sol_poseidon(Bn254X5, BigEndian, [a, b])` via the real syscall and
//! returns Custom(1) if it disagrees with `expected`. The test harness passes
//! the off-chain reference hashes as `expected`, so a successful transaction is
//! proof that the on-chain syscall matches off-chain Poseidon byte-for-byte.

use solana_program::{
    account_info::AccountInfo, entrypoint, entrypoint::ProgramResult, msg,
    program::set_return_data, program_error::ProgramError, pubkey::Pubkey,
};
use solana_poseidon::{hashv, Endianness, Parameters};

entrypoint!(process_instruction);

pub fn process_instruction(
    _program_id: &Pubkey,
    _accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    if data.len() < 64 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let a = &data[0..32];
    let b = &data[32..64];

    let hash = hashv(Parameters::Bn254X5, Endianness::BigEndian, &[a, b])
        .map_err(|_| ProgramError::InvalidInstructionData)?;
    let out = hash.to_bytes();

    if data.len() >= 96 {
        let expected = &data[64..96];
        if out != expected {
            msg!("poseidon syscall output != expected");
            return Err(ProgramError::Custom(1));
        }
        msg!("poseidon syscall matches expected");
    }

    set_return_data(&out);
    Ok(())
}

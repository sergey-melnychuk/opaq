//! On-chain r/place (raw native Solana, no Anchor). One global 32×32 canvas in a
//! PDA seeded by ["canvas"] — 1024 bytes, one palette index (0..15) per pixel.
//! Anyone can paint any pixel, any time. Collaborative pixel art (and griefing).
//!
//! Instructions (data[0] = tag):
//!   0 init                      create the canvas PDA (zeroed)
//!   1 paint (x u8, y u8, color u8)   set pixel (x,y) to color & 0x0F
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    msg,
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};

pub const W: usize = 32;
pub const H: usize = 32;
const SIZE: usize = W * H;
const CANVAS_SEED: &[u8] = b"canvas";

entrypoint!(process_instruction);

pub fn process_instruction(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let (&tag, args) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let canvas = next_account_info(iter)?;

    let (canvas_pda, bump) = Pubkey::find_program_address(&[CANVAS_SEED], program_id);
    if canvas.key != &canvas_pda {
        return Err(ProgramError::InvalidSeeds);
    }

    match tag {
        0 => {
            let system = next_account_info(iter)?;
            if !payer.is_signer {
                return Err(ProgramError::MissingRequiredSignature);
            }
            let lamports = Rent::get()?.minimum_balance(SIZE);
            let mut ix_data = Vec::with_capacity(4 + 8 + 8 + 32);
            ix_data.extend_from_slice(&0u32.to_le_bytes()); // CreateAccount
            ix_data.extend_from_slice(&lamports.to_le_bytes());
            ix_data.extend_from_slice(&(SIZE as u64).to_le_bytes());
            ix_data.extend_from_slice(program_id.as_ref());
            let ix = Instruction {
                program_id: Pubkey::default(),
                accounts: vec![AccountMeta::new(*payer.key, true), AccountMeta::new(*canvas.key, true)],
                data: ix_data,
            };
            invoke_signed(
                &ix,
                &[payer.clone(), canvas.clone(), system.clone()],
                &[&[CANVAS_SEED, &[bump]]],
            )?;
            msg!("opaq-place: canvas up ({}x{})", W, H);
            Ok(())
        }
        1 => {
            if args.len() != 3 {
                return Err(ProgramError::InvalidInstructionData);
            }
            let (x, y, color) = (args[0] as usize, args[1] as usize, args[2] & 0x0F);
            if x >= W || y >= H {
                return Err(ProgramError::InvalidInstructionData);
            }
            let mut d = canvas.data.borrow_mut();
            if d.len() < SIZE {
                return Err(ProgramError::UninitializedAccount);
            }
            d[y * W + x] = color;
            Ok(())
        }
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

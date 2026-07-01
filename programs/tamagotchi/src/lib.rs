//! On-chain Tamagotchi (raw native Solana, no Anchor). One pet per owner, in a
//! PDA seeded by ["pet", owner]. The pet's stats DECAY with on-chain time: the
//! `Clock` sysvar's slot is the clock, and hunger climbs / happiness + energy
//! fall by the number of slots elapsed since the last interaction. Feed it, play
//! with it, put it to sleep — or neglect it and it starves (`alive = 0`).
//!
//! Decay is applied lazily on every instruction; a client can mirror it between
//! txs (stored stats + slots-elapsed) so the pet looks alive in real time.
//!
//! State layout (28 bytes, zero-copy):
//!   last_slot u64 LE [0..8] | hunger u8 [8] | happiness u8 [9] | energy u8 [10]
//!   | alive u8 [11] | name [u8;16] [12..28]
//! Instructions (data[0] = tag):
//!   0 init  (args: name[..16])   1 feed   2 play   3 sleep   4 tick (apply decay)
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
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

const SIZE: usize = 28;
const PET_SEED: &[u8] = b"pet";

entrypoint!(process_instruction);

pub fn process_instruction(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let (&tag, args) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    let iter = &mut accounts.iter();
    let owner = next_account_info(iter)?;
    let pet = next_account_info(iter)?;

    let (pet_pda, bump) = Pubkey::find_program_address(&[PET_SEED, owner.key.as_ref()], program_id);
    if pet.key != &pet_pda {
        return Err(ProgramError::InvalidSeeds);
    }

    if tag == 0 {
        let system = next_account_info(iter)?;
        if !owner.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }
        // Create the pet PDA via a System CreateAccount CPI (built by hand to dodge
        // the solana 3.x crate-split types).
        let lamports = Rent::get()?.minimum_balance(SIZE);
        let mut ix_data = Vec::with_capacity(4 + 8 + 8 + 32);
        ix_data.extend_from_slice(&0u32.to_le_bytes());
        ix_data.extend_from_slice(&lamports.to_le_bytes());
        ix_data.extend_from_slice(&(SIZE as u64).to_le_bytes());
        ix_data.extend_from_slice(program_id.as_ref());
        let ix = Instruction {
            program_id: Pubkey::default(),
            accounts: vec![AccountMeta::new(*owner.key, true), AccountMeta::new(*pet.key, true)],
            data: ix_data,
        };
        invoke_signed(
            &ix,
            &[owner.clone(), pet.clone(), system.clone()],
            &[&[PET_SEED, owner.key.as_ref(), &[bump]]],
        )?;

        let mut d = pet.data.borrow_mut();
        let now = Clock::get()?.slot;
        d[0..8].copy_from_slice(&now.to_le_bytes());
        d[8] = 20; // hunger
        d[9] = 80; // happiness
        d[10] = 80; // energy
        d[11] = 1; // alive
        let n = args.len().min(16);
        d[12..12 + n].copy_from_slice(&args[..n]);
        msg!("opaq-pet: hatched!");
        return Ok(());
    }

    // All other actions first age the pet by the slots elapsed.
    let now = Clock::get()?.slot;
    let mut d = pet.data.borrow_mut();
    if d.len() < SIZE {
        return Err(ProgramError::UninitializedAccount);
    }
    let last = u64::from_le_bytes(d[0..8].try_into().unwrap());
    let elapsed = now.saturating_sub(last).min(255) as u8;
    let mut hunger = d[8].saturating_add(elapsed);
    let mut happiness = d[9].saturating_sub(elapsed);
    let mut energy = d[10].saturating_sub(elapsed / 2);
    let mut alive = d[11];

    if alive == 1 && hunger >= 100 {
        alive = 0; // starved
        msg!("opaq-pet: starved :(");
    }

    if alive == 1 {
        match tag {
            1 => {
                // feed
                hunger = hunger.saturating_sub(40);
                happiness = (happiness as u16 + 5).min(100) as u8;
            }
            2 => {
                // play
                happiness = (happiness as u16 + 25).min(100) as u8;
                energy = energy.saturating_sub(20);
                hunger = (hunger as u16 + 10).min(100) as u8;
            }
            3 => energy = 100, // sleep
            4 => {}            // tick: just materialize decay
            _ => return Err(ProgramError::InvalidInstructionData),
        }
    }

    d[0..8].copy_from_slice(&now.to_le_bytes());
    d[8] = hunger;
    d[9] = happiness;
    d[10] = energy;
    d[11] = alive;
    Ok(())
}

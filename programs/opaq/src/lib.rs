//! Opaq — native Solana privacy pool program (OPAQ.md B.5). NOT Anchor.
//!
//! Instruction dispatch on the first data byte:
//!   0 = initialize_pool   (1 = deposit, 2 = withdraw — added next)
//!
//! Accounts are PDAs holding borsh-serialized state. The commitment tree and
//! nullifier set are single global PDAs (B.2's single-PDA design); vaults are
//! one PDA per SPL mint, created on first deposit.

use borsh::{BorshDeserialize, BorshSerialize};
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

mod tree_consts;
use tree_consts::{EMPTY_ROOT, ZEROS};

pub const TREE_DEPTH: usize = 24;
pub const ROOT_HISTORY: usize = 32;

pub const TREE_SEED: &[u8] = b"tree";
pub const NULLIFIER_SEED: &[u8] = b"nullifiers";

/// Incremental Poseidon Merkle tree (B.2). Fixed size: no realloc.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct CommitmentTree {
    pub next_index: u64,
    pub filled_subtrees: [[u8; 32]; TREE_DEPTH],
    pub roots: [[u8; 32]; ROOT_HISTORY],
    pub current_root_index: u8,
}

impl CommitmentTree {
    pub const SIZE: usize = 8 + TREE_DEPTH * 32 + ROOT_HISTORY * 32 + 1;
}

/// Append-only nullifier set (B.2), grown via realloc on withdraw.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct NullifierSet {
    pub count: u64,
    pub nullifiers: Vec<[u8; 32]>,
}

impl NullifierSet {
    /// Empty: count(8) + vec length prefix(4).
    pub const INIT_SIZE: usize = 8 + 4;
}

entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let (&tag, _args) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    match tag {
        0 => initialize_pool(program_id, accounts),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

/// Create a program-owned PDA account via a system-program CPI.
fn create_pda<'a>(
    payer: &AccountInfo<'a>,
    pda: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    program_id: &Pubkey,
    seed: &[u8],
    bump: u8,
    size: usize,
) -> ProgramResult {
    let lamports = Rent::get()?.minimum_balance(size);
    // SystemInstruction::CreateAccount (bincode: u32 tag 0, lamports, space, owner).
    // Built by hand to avoid the solana 3.x crate-split type mismatches.
    let mut ix_data = Vec::with_capacity(4 + 8 + 8 + 32);
    ix_data.extend_from_slice(&0u32.to_le_bytes());
    ix_data.extend_from_slice(&lamports.to_le_bytes());
    ix_data.extend_from_slice(&(size as u64).to_le_bytes());
    ix_data.extend_from_slice(program_id.as_ref());
    let ix = Instruction {
        program_id: Pubkey::default(), // System Program = all-zeros pubkey
        accounts: vec![
            AccountMeta::new(*payer.key, true),
            AccountMeta::new(*pda.key, true),
        ],
        data: ix_data,
    };
    invoke_signed(
        &ix,
        &[payer.clone(), pda.clone(), system.clone()],
        &[&[seed, &[bump]]],
    )
}

fn write_account<T: BorshSerialize>(account: &AccountInfo, value: &T) -> ProgramResult {
    let bytes = borsh::to_vec(value).map_err(|_| ProgramError::InvalidAccountData)?;
    let mut data = account.data.borrow_mut();
    if bytes.len() > data.len() {
        return Err(ProgramError::AccountDataTooSmall);
    }
    data[..bytes.len()].copy_from_slice(&bytes);
    Ok(())
}

/// One-time setup: create the commitment-tree and nullifier-set PDAs.
/// Accounts: [payer (signer, w), commitment_tree (w), nullifier_set (w), system_program]
fn initialize_pool(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let tree_ai = next_account_info(iter)?;
    let nullifier_ai = next_account_info(iter)?;
    let system = next_account_info(iter)?;

    let (tree_pda, tree_bump) = Pubkey::find_program_address(&[TREE_SEED], program_id);
    let (nf_pda, nf_bump) = Pubkey::find_program_address(&[NULLIFIER_SEED], program_id);
    if tree_ai.key != &tree_pda || nullifier_ai.key != &nf_pda {
        return Err(ProgramError::InvalidSeeds);
    }

    create_pda(payer, tree_ai, system, program_id, TREE_SEED, tree_bump, CommitmentTree::SIZE)?;
    create_pda(payer, nullifier_ai, system, program_id, NULLIFIER_SEED, nf_bump, NullifierSet::INIT_SIZE)?;

    let mut roots = [[0u8; 32]; ROOT_HISTORY];
    roots[0] = EMPTY_ROOT;
    write_account(
        tree_ai,
        &CommitmentTree { next_index: 0, filled_subtrees: ZEROS, roots, current_root_index: 0 },
    )?;
    write_account(nullifier_ai, &NullifierSet { count: 0, nullifiers: Vec::new() })?;

    msg!("opaq: pool initialized");
    Ok(())
}

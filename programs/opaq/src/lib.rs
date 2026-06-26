//! Opaq — native Solana privacy pool program (OPAQ.md B.5). NOT Anchor.
//!
//! Instruction dispatch on the first data byte:
//!   0 = initialize_pool   (1 = deposit, 2 = withdraw — added next)
//!
//! Accounts are PDAs holding borsh-serialized state. The commitment tree and
//! nullifier set are single global PDAs (B.2's single-PDA design); vaults are
//! one PDA per SPL mint, created on first deposit.

use borsh::{BorshDeserialize, BorshSerialize};
use groth16_solana::groth16::{Groth16Verifier, Groth16Verifyingkey};
use solana_poseidon::{hashv, Endianness, Parameters};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};

mod tree_consts;
mod vk_deposit;
use tree_consts::{EMPTY_ROOT, ZEROS};

pub const TREE_DEPTH: usize = 24;
pub const ROOT_HISTORY: usize = 32;

pub const TREE_SEED: &[u8] = b"tree";
pub const NULLIFIER_SEED: &[u8] = b"nullifiers";

// Custom error codes (ProgramError::Custom).
pub const E_PROOF_INVALID: u32 = 1;
pub const E_TREE_FULL: u32 = 2;
pub const E_ALREADY_SPENT: u32 = 3;
pub const E_UNKNOWN_ROOT: u32 = 4;

const DEPOSIT_VK: Groth16Verifyingkey<'static> = Groth16Verifyingkey {
    nr_pubinputs: 3,
    vk_alpha_g1: vk_deposit::VK_ALPHA_G1,
    vk_beta_g2: vk_deposit::VK_BETA_G2,
    vk_gamme_g2: vk_deposit::VK_GAMME_G2,
    vk_delta_g2: vk_deposit::VK_DELTA_G2,
    vk_ic: &vk_deposit::VK_IC,
};

/// Original Poseidon hash of two field elements (== the circuit's hash_2 and
/// the host light-poseidon, proven in M0).
fn hash2(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    hashv(Parameters::Bn254X5, Endianness::BigEndian, &[a, b])
        .expect("poseidon")
        .to_bytes()
}

/// Canonical field encoding of a 32-byte value (B.4.2): two 128-bit big-endian
/// limbs, Poseidon-hashed. Used for token_id (and recipient on withdraw).
fn to_field(bytes32: &[u8; 32]) -> [u8; 32] {
    let mut hi = [0u8; 32];
    hi[16..].copy_from_slice(&bytes32[0..16]);
    let mut lo = [0u8; 32];
    lo[16..].copy_from_slice(&bytes32[16..32]);
    hash2(&hi, &lo)
}

/// u64 amount as a 32-byte big-endian field element.
fn be32(n: u64) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[24..].copy_from_slice(&n.to_be_bytes());
    o
}

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

// Byte offsets into the borsh-serialized CommitmentTree, for zero-copy access
// (materializing the 1801-byte struct on the stack overflows SBF's 4 KB frame).
const OFF_FILLED: usize = 8;
const OFF_ROOTS: usize = 8 + TREE_DEPTH * 32;
const OFF_CRI: usize = OFF_ROOTS + ROOT_HISTORY * 32;

fn read32(data: &[u8], off: usize) -> [u8; 32] {
    data[off..off + 32].try_into().unwrap()
}

/// Zero-copy incremental insert directly on the tree account's bytes. Mirrors
/// crates/common::tree (host-tested) and CommitmentTree::SIZE layout.
fn tree_insert(data: &mut [u8], leaf: [u8; 32]) -> Result<u64, ProgramError> {
    let mut next_index = u64::from_le_bytes(data[0..8].try_into().unwrap());
    if next_index >= (1u64 << TREE_DEPTH) {
        return Err(ProgramError::Custom(E_TREE_FULL));
    }
    let leaf_index = next_index;
    let mut index = next_index;
    let mut current = leaf;
    for i in 0..TREE_DEPTH {
        let off = OFF_FILLED + i * 32;
        let (left, right) = if index & 1 == 0 {
            data[off..off + 32].copy_from_slice(&current); // filled_subtrees[i] = current
            (current, ZEROS[i])
        } else {
            (read32(data, off), current)
        };
        current = hash2(&left, &right);
        index >>= 1;
    }
    let cri = ((data[OFF_CRI] as usize + 1) % ROOT_HISTORY) as u8;
    let root_off = OFF_ROOTS + cri as usize * 32;
    data[root_off..root_off + 32].copy_from_slice(&current);
    data[OFF_CRI] = cri;
    next_index += 1;
    data[0..8].copy_from_slice(&next_index.to_le_bytes());
    Ok(leaf_index)
}

/// Whether `root` is in the recent-root ring buffer (zero-copy; ignores empty slot).
fn tree_is_known_root(data: &[u8], root: &[u8; 32]) -> bool {
    *root != [0u8; 32]
        && (0..ROOT_HISTORY).any(|j| &data[OFF_ROOTS + j * 32..OFF_ROOTS + j * 32 + 32] == root)
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
    let (&tag, args) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    match tag {
        0 => initialize_pool(program_id, accounts),
        1 => deposit(program_id, accounts, args),
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

/// Deserialize account state from the start of its data (trailing bytes, e.g.
/// realloc slack, are ignored).
#[allow(dead_code)]
fn read_account<T: BorshDeserialize>(account: &AccountInfo) -> Result<T, ProgramError> {
    T::deserialize(&mut &account.data.borrow()[..]).map_err(|_| ProgramError::InvalidAccountData)
}

/// SPL Token `Transfer` (tag 3): `amount` from `source` to `dest`, signed by
/// `authority`. Built by hand to avoid an spl-token dep + type mismatches.
fn spl_transfer<'a>(
    token_program: &AccountInfo<'a>,
    source: &AccountInfo<'a>,
    dest: &AccountInfo<'a>,
    authority: &AccountInfo<'a>,
    amount: u64,
    signer_seeds: &[&[&[u8]]],
) -> ProgramResult {
    let mut data = Vec::with_capacity(9);
    data.push(3u8);
    data.extend_from_slice(&amount.to_le_bytes());
    let ix = Instruction {
        program_id: *token_program.key,
        accounts: vec![
            AccountMeta::new(*source.key, false),
            AccountMeta::new(*dest.key, false),
            AccountMeta::new_readonly(*authority.key, signer_seeds.is_empty()),
        ],
        data,
    };
    let infos = [source.clone(), dest.clone(), authority.clone(), token_program.clone()];
    if signer_seeds.is_empty() {
        invoke(&ix, &infos)
    } else {
        invoke_signed(&ix, &infos, signer_seeds)
    }
}

/// One-time setup: create the commitment-tree and nullifier-set PDAs.
/// Accounts: [payer (signer, w), commitment_tree (w), nullifier_set (w), system_program]
#[inline(never)]
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

pub const VAULT_TOKEN_SEED: &[u8] = b"vault_token";

/// Deposit: verify the binding proof, move SPL into the canonical vault, insert
/// the commitment.
/// Accounts: [depositor (signer), depositor_token (w), vault_token (w),
///            commitment_tree (w), token_program]
/// Args (328 bytes): proof_a(64) | proof_b(128) | proof_c(64) | token_id(32) |
///                   amount(8 LE) | commitment(32)
#[inline(never)]
fn deposit(program_id: &Pubkey, accounts: &[AccountInfo], args: &[u8]) -> ProgramResult {
    if args.len() != 64 + 128 + 64 + 32 + 8 + 32 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let proof_a: [u8; 64] = args[0..64].try_into().unwrap();
    let proof_b: [u8; 128] = args[64..192].try_into().unwrap();
    let proof_c: [u8; 64] = args[192..256].try_into().unwrap();
    let token_id: [u8; 32] = args[256..288].try_into().unwrap();
    let amount = u64::from_le_bytes(args[288..296].try_into().unwrap());
    let commitment: [u8; 32] = args[296..328].try_into().unwrap();

    // Bind public (token_id, amount) to the committed note: the proof verifies
    // only if its public inputs equal these reconstructed values (B.5.2). Without
    // this a depositor could commit to more than they transfer and drain the vault.
    let public = [to_field(&token_id), be32(amount), commitment];
    let mut verifier = Groth16Verifier::new(&proof_a, &proof_b, &proof_c, &public, &DEPOSIT_VK)
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;
    verifier
        .verify()
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;

    let iter = &mut accounts.iter();
    let depositor = next_account_info(iter)?;
    let depositor_token = next_account_info(iter)?;
    let vault_token = next_account_info(iter)?;
    let tree_ai = next_account_info(iter)?;
    let token_program = next_account_info(iter)?;

    if !depositor.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (tree_pda, _) = Pubkey::find_program_address(&[TREE_SEED], program_id);
    if tree_ai.key != &tree_pda {
        return Err(ProgramError::InvalidSeeds);
    }
    // The vault MUST be the canonical per-mint PDA, else funds go elsewhere while
    // a valid commitment lets the depositor later drain the real vault.
    let (vault_pda, _) =
        Pubkey::find_program_address(&[VAULT_TOKEN_SEED, &token_id], program_id);
    if vault_token.key != &vault_pda {
        return Err(ProgramError::InvalidSeeds);
    }

    // Move exactly `amount` (the proof-bound amount) depositor -> vault.
    spl_transfer(token_program, depositor_token, vault_token, depositor, amount, &[])?;

    let leaf_index = tree_insert(&mut tree_ai.data.borrow_mut(), commitment)?;

    msg!("opaq: deposit ok, leaf_index={}", leaf_index);
    Ok(())
}

//! Opaq — native Solana privacy pool program (OPAQ.md B.5).
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
mod vk_burn;
mod vk_deposit;
mod vk_transfer;
mod vk_withdraw;
mod vk_xburn;
use tree_consts::{EMPTY_ROOT, ZEROS};

pub const TREE_DEPTH: usize = 24;
pub const ROOT_HISTORY: usize = 32;

pub const TREE_SEED: &[u8] = b"tree";
pub const NULLIFIER_SEED: &[u8] = b"nullifiers";
pub const VAULT_SEED: &[u8] = b"vault"; // vault token-account authority PDA
pub const XBURN_PENDING_SEED: &[u8] = b"xpending"; // B.12.5 mint_from_xburn attestation PDA

/// Opaq-internal destination-chain id for Solana (OPAQ.md B.12.5) — this is
/// NOT an external standard (Solana has no EIP-155-style numeric chain id);
/// it only has to agree with what a source chain's `xburn` proof binds as
/// `dest_chain` when Solana is the destination.
pub const SOLANA_CHAIN_ID: u64 = 101;

// Custom error codes (ProgramError::Custom).
pub const E_PROOF_INVALID: u32 = 1;
pub const E_TREE_FULL: u32 = 2;
pub const E_ALREADY_SPENT: u32 = 3;
pub const E_UNKNOWN_ROOT: u32 = 4;
pub const E_WRONG_VAULT: u32 = 5;
pub const E_WRONG_RECIPIENT: u32 = 6;
pub const E_WRONG_MINT: u32 = 7;
pub const E_WRONG_OPERATOR: u32 = 8; // add_pending_xburn signer != XburnPending.operator
pub const E_NOT_PENDING: u32 = 9; // mint_from_xburn: nullifier never attested (or already minted)
pub const E_WRONG_CHAIN: u32 = 10; // mint_from_xburn: proof's dest_chain != SOLANA_CHAIN_ID

/// SPL Token program id (`Tokenkeg…`). Funds only ever move via a CPI to THIS
/// program, so it must be validated: otherwise a no-op/forged "token program"
/// passed by the caller lets a deposit insert a commitment WITHOUT an actual
/// transfer into the vault — a pool-drain vector. The real token program also
/// re-validates every token account it touches, so pinning it closes the whole
/// account-confusion class.
pub const SPL_TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172,
    28, 180, 133, 237, 95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
]);

/// Associated Token Account program id (`ATokenGP…`). Used to pin the vault to
/// the ONE canonical ATA per (vault_authority, mint): without this, any token
/// account owned by the vault PDA passes `check_vault`, so deposits/withdrawals
/// could reference different vault accounts and desync the single-vault invariant
/// the pool's accounting assumes.
pub const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    140, 151, 37, 143, 78, 36, 137, 241, 187, 61, 16, 41, 20, 142, 13, 131,
    11, 90, 19, 153, 218, 255, 16, 132, 4, 142, 123, 216, 219, 233, 248, 89,
]);

/// The canonical vault token account: the ATA of `vault_authority` for `mint`.
fn canonical_vault_ata(vault_authority: &Pubkey, mint: &[u8; 32]) -> Pubkey {
    Pubkey::find_program_address(
        &[vault_authority.as_ref(), SPL_TOKEN_PROGRAM_ID.as_ref(), mint],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .0
}

const DEPOSIT_VK: Groth16Verifyingkey<'static> = Groth16Verifyingkey {
    nr_pubinputs: 3,
    vk_alpha_g1: vk_deposit::VK_ALPHA_G1,
    vk_beta_g2: vk_deposit::VK_BETA_G2,
    vk_gamme_g2: vk_deposit::VK_GAMME_G2,
    vk_delta_g2: vk_deposit::VK_DELTA_G2,
    vk_ic: &vk_deposit::VK_IC,
};

const WITHDRAW_VK: Groth16Verifyingkey<'static> = Groth16Verifyingkey {
    nr_pubinputs: 5,
    vk_alpha_g1: vk_withdraw::VK_ALPHA_G1,
    vk_beta_g2: vk_withdraw::VK_BETA_G2,
    vk_gamme_g2: vk_withdraw::VK_GAMME_G2,
    vk_delta_g2: vk_withdraw::VK_DELTA_G2,
    vk_ic: &vk_withdraw::VK_IC,
};

const TRANSFER_VK: Groth16Verifyingkey<'static> = Groth16Verifyingkey {
    nr_pubinputs: 5, // merkle_root, nullifier[2], out_commitment[2]
    vk_alpha_g1: vk_transfer::VK_ALPHA_G1,
    vk_beta_g2: vk_transfer::VK_BETA_G2,
    vk_gamme_g2: vk_transfer::VK_GAMME_G2,
    vk_delta_g2: vk_transfer::VK_DELTA_G2,
    vk_ic: &vk_transfer::VK_IC,
};

const BURN_VK: Groth16Verifyingkey<'static> = Groth16Verifyingkey {
    nr_pubinputs: 6, // merkle_root, nullifier, token_id, amount, dest_chain, dest_address
    vk_alpha_g1: vk_burn::VK_ALPHA_G1,
    vk_beta_g2: vk_burn::VK_BETA_G2,
    vk_gamme_g2: vk_burn::VK_GAMME_G2,
    vk_delta_g2: vk_burn::VK_DELTA_G2,
    vk_ic: &vk_burn::VK_IC,
};

/// Phase 4 (B.12.2/B.12.5): xburn.nr's verifier — used by `mint_from_xburn`
/// when Solana is the DESTINATION of a cross-chain shielded move.
const XBURN_VK: Groth16Verifyingkey<'static> = Groth16Verifyingkey {
    nr_pubinputs: 4, // src_merkle_root, src_nullifier, dest_chain, out_commitment
    vk_alpha_g1: vk_xburn::VK_ALPHA_G1,
    vk_beta_g2: vk_xburn::VK_BETA_G2,
    vk_gamme_g2: vk_xburn::VK_GAMME_G2,
    vk_delta_g2: vk_xburn::VK_DELTA_G2,
    vk_ic: &vk_xburn::VK_IC,
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

// NullifierSet borsh layout: count u64 [0..8] | vec len u32 [8..12] | nullifiers [12..].
const OFF_NULLIFIERS: usize = 12;

fn nullifier_contains(data: &[u8], n: &[u8; 32]) -> bool {
    let count = u64::from_le_bytes(data[0..8].try_into().unwrap()) as usize;
    (0..count).any(|i| &data[OFF_NULLIFIERS + i * 32..OFF_NULLIFIERS + i * 32 + 32] == n)
}

/// Append a nullifier: grow the account by 32 bytes (rent funded by `payer` via a
/// System Transfer), then write the entry and bump count + vec length (zero-copy).
fn nullifier_append<'a>(
    payer: &AccountInfo<'a>,
    nf: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    nullifier: [u8; 32],
) -> ProgramResult {
    let count = u64::from_le_bytes(nf.data.borrow()[0..8].try_into().unwrap());
    let new_len = OFF_NULLIFIERS + (count as usize + 1) * 32;

    let needed = Rent::get()?.minimum_balance(new_len);
    let have = nf.lamports();
    if needed > have {
        let mut d = Vec::with_capacity(12);
        d.extend_from_slice(&2u32.to_le_bytes()); // SystemInstruction::Transfer
        d.extend_from_slice(&(needed - have).to_le_bytes());
        let ix = Instruction {
            program_id: Pubkey::default(),
            accounts: vec![AccountMeta::new(*payer.key, true), AccountMeta::new(*nf.key, false)],
            data: d,
        };
        invoke(&ix, &[payer.clone(), nf.clone(), system.clone()])?;
    }
    nf.resize(new_len)?;

    let mut data = nf.data.borrow_mut();
    let off = OFF_NULLIFIERS + count as usize * 32;
    data[off..off + 32].copy_from_slice(&nullifier);
    let count2 = count + 1;
    data[0..8].copy_from_slice(&count2.to_le_bytes());
    data[8..12].copy_from_slice(&(count2 as u32).to_le_bytes());
    Ok(())
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

// XburnPending (B.12.5): mirrors OpaqMint.sol's pendingMint/minted mappings on
// the Solana side. Zero-copy layout: operator[32] | count u64 LE [32..40] |
// entries (nullifier[32] ‖ minted:u8) repeated from 40. Same append-only,
// linear-scan, realloc-grown shape as NullifierSet (B.2) — appropriate for the
// same reason (few entries relative to mainnet nullifier-set scale, A.10).
const XP_OFF_OPERATOR: usize = 0;
const XP_OFF_COUNT: usize = 32;
const XP_OFF_ENTRIES: usize = 40;
const XP_ENTRY_SIZE: usize = 33; // nullifier(32) + minted(1)
const XBURN_PENDING_INIT_SIZE: usize = XP_OFF_ENTRIES;

fn xpending_operator(data: &[u8]) -> Pubkey {
    Pubkey::new_from_array(data[XP_OFF_OPERATOR..XP_OFF_OPERATOR + 32].try_into().unwrap())
}

fn xpending_count(data: &[u8]) -> u64 {
    u64::from_le_bytes(data[XP_OFF_COUNT..XP_OFF_COUNT + 8].try_into().unwrap())
}

/// Byte offset of `nullifier`'s entry, if present.
fn xpending_find(data: &[u8], nullifier: &[u8; 32]) -> Option<usize> {
    let count = xpending_count(data) as usize;
    (0..count).map(|i| XP_OFF_ENTRIES + i * XP_ENTRY_SIZE).find(|&off| &data[off..off + 32] == nullifier)
}

/// Append a fresh pending entry (minted=false). Caller must have already
/// checked `xpending_find` returns `None` — this never dedupes itself.
fn xpending_append<'a>(
    payer: &AccountInfo<'a>,
    xp: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    nullifier: [u8; 32],
) -> ProgramResult {
    let count = xpending_count(&xp.data.borrow());
    let new_len = XP_OFF_ENTRIES + (count as usize + 1) * XP_ENTRY_SIZE;

    let needed = Rent::get()?.minimum_balance(new_len);
    let have = xp.lamports();
    if needed > have {
        let mut d = Vec::with_capacity(12);
        d.extend_from_slice(&2u32.to_le_bytes()); // SystemInstruction::Transfer
        d.extend_from_slice(&(needed - have).to_le_bytes());
        let ix = Instruction {
            program_id: Pubkey::default(),
            accounts: vec![AccountMeta::new(*payer.key, true), AccountMeta::new(*xp.key, false)],
            data: d,
        };
        invoke(&ix, &[payer.clone(), xp.clone(), system.clone()])?;
    }
    xp.resize(new_len)?;

    let mut data = xp.data.borrow_mut();
    let off = XP_OFF_ENTRIES + count as usize * XP_ENTRY_SIZE;
    data[off..off + 32].copy_from_slice(&nullifier);
    data[off + 32] = 0; // minted = false
    let count2 = count + 1;
    data[XP_OFF_COUNT..XP_OFF_COUNT + 8].copy_from_slice(&count2.to_le_bytes());
    Ok(())
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
        3 => transfer(program_id, accounts, args),
        4 => burn(program_id, accounts, args),
        2 => withdraw(program_id, accounts, args),
        5 => initialize_xburn_pending(program_id, accounts, args),
        6 => add_pending_xburn(program_id, accounts, args),
        7 => mint_from_xburn(program_id, accounts, args),
        8 => xburn(program_id, accounts, args),
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

/// Verify `vault_token` is an SPL token account for `token_id` whose authority
/// is the vault PDA — a real vault for this mint controlled by the program.
/// SPL token account layout: mint @0..32, owner(authority) @32..64.
fn check_vault(
    vault_token: &AccountInfo,
    token_id: &[u8; 32],
    vault_authority: &Pubkey,
) -> ProgramResult {
    let data = vault_token.data.borrow();
    if data.len() < 64 {
        return Err(ProgramError::InvalidAccountData);
    }
    if &data[0..32] != token_id || &data[32..64] != vault_authority.as_ref() {
        return Err(ProgramError::Custom(E_WRONG_VAULT));
    }
    Ok(())
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
            // authority signs either as a real tx signer (deposit) or via the
            // vault PDA in invoke_signed (withdraw) — signer in the meta both ways.
            AccountMeta::new_readonly(*authority.key, true),
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
    // Funds move only via this CPI: a forged token_program would let the transfer
    // be a no-op while the commitment is still inserted — a fake, unfunded deposit.
    if token_program.key != &SPL_TOKEN_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    let (tree_pda, _) = Pubkey::find_program_address(&[TREE_SEED], program_id);
    if tree_ai.key != &tree_pda {
        return Err(ProgramError::InvalidSeeds);
    }
    // The vault must be a token account for this mint owned by the vault PDA,
    // else funds go elsewhere while a valid commitment lets the depositor later
    // drain the real vault.
    let (vault_authority, _) = Pubkey::find_program_address(&[VAULT_SEED, &token_id], program_id);
    check_vault(vault_token, &token_id, &vault_authority)?;
    // Pin the ONE canonical vault (the authority's ATA), so every deposit/withdraw
    // for this mint hits the same account and the single-vault invariant holds.
    if vault_token.key != &canonical_vault_ata(&vault_authority, &token_id) {
        return Err(ProgramError::Custom(E_WRONG_VAULT));
    }
    // depositor_token must be the same mint (defense-in-depth; the token program
    // also enforces mint equality on transfer).
    if depositor_token.data.borrow().get(0..32) != Some(&token_id[..]) {
        return Err(ProgramError::Custom(E_WRONG_MINT));
    }

    // Move exactly `amount` (the proof-bound amount) depositor -> vault.
    spl_transfer(token_program, depositor_token, vault_token, depositor, amount, &[])?;

    let leaf_index = tree_insert(&mut tree_ai.data.borrow_mut(), commitment)?;

    msg!("opaq: deposit ok, leaf_index={}", leaf_index);
    Ok(())
}

/// Withdraw: verify the proof, check the root is recent, enforce the nullifier,
/// and release SPL from the vault to the recipient (vault PDA signs).
/// Accounts: [payer (signer,w), vault_authority, vault_token (w),
///            recipient_token (w), commitment_tree, nullifier_set (w),
///            token_program, system_program]
/// Args (392): proof_a(64) proof_b(128) proof_c(64) merkle_root(32) nullifier(32)
///             token_id(32) amount(8 LE) recipient(32)
#[inline(never)]
fn withdraw(program_id: &Pubkey, accounts: &[AccountInfo], args: &[u8]) -> ProgramResult {
    if args.len() != 64 + 128 + 64 + 32 + 32 + 32 + 8 + 32 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let proof_a: [u8; 64] = args[0..64].try_into().unwrap();
    let proof_b: [u8; 128] = args[64..192].try_into().unwrap();
    let proof_c: [u8; 64] = args[192..256].try_into().unwrap();
    let merkle_root: [u8; 32] = args[256..288].try_into().unwrap();
    let nullifier: [u8; 32] = args[288..320].try_into().unwrap();
    let token_id: [u8; 32] = args[320..352].try_into().unwrap();
    let amount = u64::from_le_bytes(args[352..360].try_into().unwrap());
    let recipient: [u8; 32] = args[360..392].try_into().unwrap();

    // Public inputs in circuit order (B.4.2). recipient is bound so the submitter
    // can't redirect funds; token_id/amount are bound to the released asset.
    let public = [
        merkle_root,
        nullifier,
        to_field(&token_id),
        be32(amount),
        to_field(&recipient),
    ];
    let mut verifier = Groth16Verifier::new(&proof_a, &proof_b, &proof_c, &public, &WITHDRAW_VK)
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;
    verifier
        .verify()
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;

    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let vault_authority = next_account_info(iter)?;
    let vault_token = next_account_info(iter)?;
    let recipient_token = next_account_info(iter)?;
    let tree_ai = next_account_info(iter)?;
    let nullifier_ai = next_account_info(iter)?;
    let token_program = next_account_info(iter)?;
    let system = next_account_info(iter)?;

    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    // Funds move only via this CPI (vault -> recipient); pin the token program so
    // a forged one can't intercept the release or skip the real account checks.
    if token_program.key != &SPL_TOKEN_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    let (tree_pda, _) = Pubkey::find_program_address(&[TREE_SEED], program_id);
    let (nf_pda, _) = Pubkey::find_program_address(&[NULLIFIER_SEED], program_id);
    let (vault_auth_pda, vault_bump) =
        Pubkey::find_program_address(&[VAULT_SEED, &token_id], program_id);
    if tree_ai.key != &tree_pda
        || nullifier_ai.key != &nf_pda
        || vault_authority.key != &vault_auth_pda
    {
        return Err(ProgramError::InvalidSeeds);
    }
    check_vault(vault_token, &token_id, &vault_auth_pda)?;
    if vault_token.key != &canonical_vault_ata(&vault_auth_pda, &token_id) {
        return Err(ProgramError::Custom(E_WRONG_VAULT));
    }
    // recipient_token must be owned by the proof-bound recipient — otherwise the
    // recipient public input is toothless and a submitter could redirect funds —
    // and be the same mint (defense-in-depth; the token program also enforces it).
    {
        let rt = recipient_token.data.borrow();
        if rt.len() < 64 || rt[32..64] != recipient {
            return Err(ProgramError::Custom(E_WRONG_RECIPIENT));
        }
        if rt[0..32] != token_id {
            return Err(ProgramError::Custom(E_WRONG_MINT));
        }
    }

    // Root must be in the recent ring buffer (proofs may target a moved root).
    if !tree_is_known_root(&tree_ai.data.borrow(), &merkle_root) {
        return Err(ProgramError::Custom(E_UNKNOWN_ROOT));
    }
    // Double-spend check, then record the nullifier.
    if nullifier_contains(&nullifier_ai.data.borrow(), &nullifier) {
        return Err(ProgramError::Custom(E_ALREADY_SPENT));
    }
    nullifier_append(payer, nullifier_ai, system, nullifier)?;

    // Release tokens vault -> recipient, signed by the vault authority PDA.
    spl_transfer(
        token_program,
        vault_token,
        recipient_token,
        vault_authority,
        amount,
        &[&[VAULT_SEED, &token_id, &[vault_bump]]],
    )?;

    msg!("opaq: withdraw ok");
    Ok(())
}

/// Transfer (Phase 2): a fully-private 2-in/2-out join-split — verify the proof,
/// check the root is recent, record both input nullifiers, insert both output
/// commitments. NO vault movement: value stays in the pool (the circuit enforces
/// Σin == Σout for a single private token_id), so amounts/token never appear.
/// Accounts: [payer (signer,w), commitment_tree (w), nullifier_set (w), system_program]
/// Args (>= 416): proof_a(64) proof_b(128) proof_c(64) merkle_root(32)
///             nullifier0(32) nullifier1(32) out_commitment0(32) out_commitment1(32)
///             [+ optional trailing B.13 viewing-key memo bytes — the wallet layer's
///             encrypted note-opening for a recipient output, riding along in this
///             instruction's own data. Not a circuit input, not parsed on-chain: the
///             program only ever reads the fixed prefix above and ignores anything
///             appended after it, so the memo costs no extra verification or compute
///             here — it's just present in the transaction for RPC clients to read.]
#[inline(never)]
fn transfer(program_id: &Pubkey, accounts: &[AccountInfo], args: &[u8]) -> ProgramResult {
    const FIXED_LEN: usize = 64 + 128 + 64 + 32 + 32 + 32 + 32 + 32;
    if args.len() < FIXED_LEN {
        return Err(ProgramError::InvalidInstructionData);
    }
    let proof_a: [u8; 64] = args[0..64].try_into().unwrap();
    let proof_b: [u8; 128] = args[64..192].try_into().unwrap();
    let proof_c: [u8; 64] = args[192..256].try_into().unwrap();
    let merkle_root: [u8; 32] = args[256..288].try_into().unwrap();
    let nullifier0: [u8; 32] = args[288..320].try_into().unwrap();
    let nullifier1: [u8; 32] = args[320..352].try_into().unwrap();
    let commitment0: [u8; 32] = args[352..384].try_into().unwrap();
    let commitment1: [u8; 32] = args[384..416].try_into().unwrap();

    // Public inputs in circuit order (B.4.3). Value conservation, range checks,
    // shared token_id, membership, and the nullifier/commitment bindings are all
    // enforced inside the proof — the program only records its public effects.
    let public = [merkle_root, nullifier0, nullifier1, commitment0, commitment1];
    let mut verifier = Groth16Verifier::new(&proof_a, &proof_b, &proof_c, &public, &TRANSFER_VK)
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;
    verifier
        .verify()
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;

    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let tree_ai = next_account_info(iter)?;
    let nullifier_ai = next_account_info(iter)?;
    let system = next_account_info(iter)?;

    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (tree_pda, _) = Pubkey::find_program_address(&[TREE_SEED], program_id);
    let (nf_pda, _) = Pubkey::find_program_address(&[NULLIFIER_SEED], program_id);
    if tree_ai.key != &tree_pda || nullifier_ai.key != &nf_pda {
        return Err(ProgramError::InvalidSeeds);
    }

    if !tree_is_known_root(&tree_ai.data.borrow(), &merkle_root) {
        return Err(ProgramError::Custom(E_UNKNOWN_ROOT));
    }
    // Neither input may be already spent, and the two inputs must be distinct
    // notes (equal nullifiers == spending the same note twice in one transfer).
    {
        let nf_data = nullifier_ai.data.borrow();
        if nullifier_contains(&nf_data, &nullifier0) || nullifier_contains(&nf_data, &nullifier1) {
            return Err(ProgramError::Custom(E_ALREADY_SPENT));
        }
    }
    if nullifier0 == nullifier1 {
        return Err(ProgramError::Custom(E_ALREADY_SPENT));
    }
    nullifier_append(payer, nullifier_ai, system, nullifier0)?;
    nullifier_append(payer, nullifier_ai, system, nullifier1)?;

    // Insert both output commitments (value conserved, so the pool stays balanced).
    let leaf0 = tree_insert(&mut tree_ai.data.borrow_mut(), commitment0)?;
    let leaf1 = tree_insert(&mut tree_ai.data.borrow_mut(), commitment1)?;

    msg!("opaq: transfer ok, leaves={},{}", leaf0, leaf1);
    Ok(())
}

/// Burn (Phase 3): like withdraw, but releases NO SPL — it burns the note (records
/// the nullifier on Solana, value stays locked in the vault) and binds an EVM mint
/// destination (dest_chain, dest_address). An EVM mint contract verifies this same
/// proof and mints there, checking the nullifier against its OWN set (A.9 — the
/// chains don't share state). The burn params are logged for the relayer.
/// Accounts: [payer (signer,w), commitment_tree, nullifier_set (w), system_program]
/// Args (424): proof_a(64) proof_b(128) proof_c(64) merkle_root(32) nullifier(32)
///             token_id(32) amount(8 LE) dest_chain(32) dest_address(32)
#[inline(never)]
fn burn(program_id: &Pubkey, accounts: &[AccountInfo], args: &[u8]) -> ProgramResult {
    if args.len() != 64 + 128 + 64 + 32 + 32 + 32 + 8 + 32 + 32 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let proof_a: [u8; 64] = args[0..64].try_into().unwrap();
    let proof_b: [u8; 128] = args[64..192].try_into().unwrap();
    let proof_c: [u8; 64] = args[192..256].try_into().unwrap();
    let merkle_root: [u8; 32] = args[256..288].try_into().unwrap();
    let nullifier: [u8; 32] = args[288..320].try_into().unwrap();
    let token_id: [u8; 32] = args[320..352].try_into().unwrap();
    let amount = u64::from_le_bytes(args[352..360].try_into().unwrap());
    let dest_chain: [u8; 32] = args[360..392].try_into().unwrap();
    let dest_address: [u8; 32] = args[392..424].try_into().unwrap();

    // Public inputs in circuit order (burn.nr). dest_chain/dest_address are bound,
    // so a relayer can't redirect the EVM mint.
    let public = [
        merkle_root,
        nullifier,
        to_field(&token_id),
        be32(amount),
        dest_chain,
        dest_address,
    ];
    let mut verifier = Groth16Verifier::new(&proof_a, &proof_b, &proof_c, &public, &BURN_VK)
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;
    verifier
        .verify()
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;

    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let tree_ai = next_account_info(iter)?;
    let nullifier_ai = next_account_info(iter)?;
    let system = next_account_info(iter)?;

    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (tree_pda, _) = Pubkey::find_program_address(&[TREE_SEED], program_id);
    let (nf_pda, _) = Pubkey::find_program_address(&[NULLIFIER_SEED], program_id);
    if tree_ai.key != &tree_pda || nullifier_ai.key != &nf_pda {
        return Err(ProgramError::InvalidSeeds);
    }

    if !tree_is_known_root(&tree_ai.data.borrow(), &merkle_root) {
        return Err(ProgramError::Custom(E_UNKNOWN_ROOT));
    }
    // Double-burn check, then record the nullifier. No SPL release and no tree
    // insert: the value is now claimable on the destination chain via the proof.
    if nullifier_contains(&nullifier_ai.data.borrow(), &nullifier) {
        return Err(ProgramError::Custom(E_ALREADY_SPENT));
    }
    nullifier_append(payer, nullifier_ai, system, nullifier)?;

    msg!("opaq: burn ok, amount={}, dest_chain={}", amount, chain_lo(&dest_chain));
    Ok(())
}

/// Low 32 bits of a 32-byte field — the EVM chain id for typical ids (cheap log).
fn chain_lo(b: &[u8; 32]) -> u32 {
    u32::from_be_bytes(b[28..32].try_into().unwrap())
}

/// Phase 4 (B.12.5): one-time setup for the `XburnPending` PDA — the Solana
/// mirror of OpaqMint.sol's `pendingMint`/`minted` mappings, used when Solana
/// is the DESTINATION of a cross-chain shielded move. `operator` is the
/// semi-trusted attestor (A.9 rung 1: single key today; the same slot an ICP
/// canister's threshold-signing address takes at rung 2+).
/// Accounts: [payer (signer,w), xburn_pending (w), system_program]
/// Args (32): operator (Pubkey)
fn initialize_xburn_pending(program_id: &Pubkey, accounts: &[AccountInfo], args: &[u8]) -> ProgramResult {
    if args.len() != 32 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let operator = Pubkey::new_from_array(args[0..32].try_into().unwrap());

    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let xp_ai = next_account_info(iter)?;
    let system = next_account_info(iter)?;

    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (xp_pda, xp_bump) = Pubkey::find_program_address(&[XBURN_PENDING_SEED], program_id);
    if xp_ai.key != &xp_pda {
        return Err(ProgramError::InvalidSeeds);
    }

    create_pda(payer, xp_ai, system, program_id, XBURN_PENDING_SEED, xp_bump, XBURN_PENDING_INIT_SIZE)?;
    let mut data = xp_ai.data.borrow_mut();
    data[XP_OFF_OPERATOR..XP_OFF_OPERATOR + 32].copy_from_slice(operator.as_ref());
    data[XP_OFF_COUNT..XP_OFF_COUNT + 8].copy_from_slice(&0u64.to_le_bytes());

    msg!("opaq: xburn_pending initialized, operator={}", operator);
    Ok(())
}

/// Phase 4 (B.12.5): the operator mirrors a finalized SOURCE-chain `xburn` by
/// attesting its nullifier here — the ONLY trust in the bridge (A.9): the
/// operator posts a boolean, never sees a secret (the proof itself binds the
/// nullifier to token/amount/destination note).
/// Accounts: [operator (signer,w, also funds rent), xburn_pending (w), system_program]
/// Args (32): src_nullifier
fn add_pending_xburn(program_id: &Pubkey, accounts: &[AccountInfo], args: &[u8]) -> ProgramResult {
    if args.len() != 32 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let src_nullifier: [u8; 32] = args[0..32].try_into().unwrap();

    let iter = &mut accounts.iter();
    let operator = next_account_info(iter)?;
    let xp_ai = next_account_info(iter)?;
    let system = next_account_info(iter)?;

    if !operator.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (xp_pda, _) = Pubkey::find_program_address(&[XBURN_PENDING_SEED], program_id);
    if xp_ai.key != &xp_pda {
        return Err(ProgramError::InvalidSeeds);
    }
    if xpending_operator(&xp_ai.data.borrow()) != *operator.key {
        return Err(ProgramError::Custom(E_WRONG_OPERATOR));
    }
    if xpending_find(&xp_ai.data.borrow(), &src_nullifier).is_some() {
        // Already pending (or already minted, per B.12.4's "re-add-after-mint
        // guard") — reject rather than silently accept a duplicate attestation.
        return Err(ProgramError::Custom(E_ALREADY_SPENT));
    }
    xpending_append(operator, xp_ai, system, src_nullifier)?;

    msg!("opaq: xburn pending added");
    Ok(())
}

/// Phase 4 (B.12.5): the DESTINATION-side mint — verify the SAME proof the
/// source chain's `xburn` verified, require the operator has attested
/// `src_nullifier` as a finalized (unminted) source burn, then re-shield by
/// inserting `out_commitment` into Solana's own tree. Solana never validates
/// `src_root` — it isn't Solana's root to check (B.12.3): the source chain
/// already enforced a valid root before recording its own nullifier.
/// Accounts: [payer (signer,w), commitment_tree (w), xburn_pending (w), system_program]
/// Args (256 + 4*32 = 384): proof_a(64) proof_b(128) proof_c(64)
///             src_merkle_root(32) src_nullifier(32) dest_chain(32) out_commitment(32)
#[inline(never)]
fn mint_from_xburn(program_id: &Pubkey, accounts: &[AccountInfo], args: &[u8]) -> ProgramResult {
    if args.len() != 64 + 128 + 64 + 32 + 32 + 32 + 32 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let proof_a: [u8; 64] = args[0..64].try_into().unwrap();
    let proof_b: [u8; 128] = args[64..192].try_into().unwrap();
    let proof_c: [u8; 64] = args[192..256].try_into().unwrap();
    let src_merkle_root: [u8; 32] = args[256..288].try_into().unwrap();
    let src_nullifier: [u8; 32] = args[288..320].try_into().unwrap();
    let dest_chain: [u8; 32] = args[320..352].try_into().unwrap();
    let out_commitment: [u8; 32] = args[352..384].try_into().unwrap();

    if dest_chain != be32(SOLANA_CHAIN_ID) {
        return Err(ProgramError::Custom(E_WRONG_CHAIN));
    }

    // Public inputs in circuit order (xburn.nr, B.12.2).
    let public = [src_merkle_root, src_nullifier, dest_chain, out_commitment];
    let mut verifier = Groth16Verifier::new(&proof_a, &proof_b, &proof_c, &public, &XBURN_VK)
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;
    verifier
        .verify()
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;

    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let tree_ai = next_account_info(iter)?;
    let xp_ai = next_account_info(iter)?;
    let _system = next_account_info(iter)?;

    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (tree_pda, _) = Pubkey::find_program_address(&[TREE_SEED], program_id);
    let (xp_pda, _) = Pubkey::find_program_address(&[XBURN_PENDING_SEED], program_id);
    if tree_ai.key != &tree_pda || xp_ai.key != &xp_pda {
        return Err(ProgramError::InvalidSeeds);
    }

    let mint_offset = {
        let data = xp_ai.data.borrow();
        let off = xpending_find(&data, &src_nullifier).ok_or(ProgramError::Custom(E_NOT_PENDING))?;
        if data[off + 32] != 0 {
            return Err(ProgramError::Custom(E_ALREADY_SPENT)); // already minted
        }
        off
    };

    let leaf_index = tree_insert(&mut tree_ai.data.borrow_mut(), out_commitment)?;
    xp_ai.data.borrow_mut()[mint_offset + 32] = 1; // mark minted

    msg!("opaq: mint_from_xburn ok, leaf={}", leaf_index);
    Ok(())
}

/// Phase 4 (P4.3, B.12.3/B.12.5): the SOURCE-side leg of a cross-chain move —
/// Solana's own analog of `OpaqPool.xburn` (evm/src/OpaqPool.sol). Verifies
/// the SAME xburn.nr proof `mint_from_xburn` verifies on a destination chain,
/// but here Solana IS the source: checks `src_merkle_root` against Solana's
/// OWN known-recent-root ring buffer, records `src_nullifier` in the SAME
/// `NullifierSet` every other spend draws from (a nullifier is unique
/// regardless of which instruction spent it), and inserts nothing / releases
/// no SPL — the note is now claimable on the destination via this identical
/// proof (mirrors `burn`'s shape exactly, just xburn.nr's 4 public inputs
/// instead of burn.nr's 6). Added as a new instruction (tag 8) rather than
/// migrating tag 4's `burn` in place, for the same reason P4.1 stayed
/// additive (B.12.8's P4.1 scoping note): doesn't touch the already-working
/// burn.nr/OpaqMint path.
/// Accounts: [payer (signer,w), commitment_tree, nullifier_set (w), system_program]
/// Args (256 + 4*32 = 384): proof_a(64) proof_b(128) proof_c(64)
///             src_merkle_root(32) src_nullifier(32) dest_chain(32) out_commitment(32)
#[inline(never)]
fn xburn(program_id: &Pubkey, accounts: &[AccountInfo], args: &[u8]) -> ProgramResult {
    if args.len() != 64 + 128 + 64 + 32 + 32 + 32 + 32 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let proof_a: [u8; 64] = args[0..64].try_into().unwrap();
    let proof_b: [u8; 128] = args[64..192].try_into().unwrap();
    let proof_c: [u8; 64] = args[192..256].try_into().unwrap();
    let src_merkle_root: [u8; 32] = args[256..288].try_into().unwrap();
    let src_nullifier: [u8; 32] = args[288..320].try_into().unwrap();
    let dest_chain: [u8; 32] = args[320..352].try_into().unwrap();
    let out_commitment: [u8; 32] = args[352..384].try_into().unwrap();

    // Public inputs in circuit order (xburn.nr, B.12.2).
    let public = [src_merkle_root, src_nullifier, dest_chain, out_commitment];
    let mut verifier = Groth16Verifier::new(&proof_a, &proof_b, &proof_c, &public, &XBURN_VK)
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;
    verifier
        .verify()
        .map_err(|_| ProgramError::Custom(E_PROOF_INVALID))?;

    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let tree_ai = next_account_info(iter)?;
    let nullifier_ai = next_account_info(iter)?;
    let system = next_account_info(iter)?;

    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (tree_pda, _) = Pubkey::find_program_address(&[TREE_SEED], program_id);
    let (nf_pda, _) = Pubkey::find_program_address(&[NULLIFIER_SEED], program_id);
    if tree_ai.key != &tree_pda || nullifier_ai.key != &nf_pda {
        return Err(ProgramError::InvalidSeeds);
    }

    if !tree_is_known_root(&tree_ai.data.borrow(), &src_merkle_root) {
        return Err(ProgramError::Custom(E_UNKNOWN_ROOT));
    }
    // Double-xburn check, then record the nullifier. No SPL release and no tree
    // insert: the value is now claimable on the destination chain via the proof.
    if nullifier_contains(&nullifier_ai.data.borrow(), &src_nullifier) {
        return Err(ProgramError::Custom(E_ALREADY_SPENT));
    }
    nullifier_append(payer, nullifier_ai, system, src_nullifier)?;

    msg!("opaq: xburn ok, dest_chain={}", chain_lo(&dest_chain));
    Ok(())
}

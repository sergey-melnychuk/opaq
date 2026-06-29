//! `opaq` — off-chain prover/note CLI (OPAQ.md B.7, M9).
//!
//!   opaq deposit  --token <pubkey> --amount <u64> --note <out.json>
//!   opaq withdraw --note <in.json> --recipient <pubkey>
//!
//! Owns the client-side note lifecycle: generate secrets, derive the commitment
//! (deposit) / nullifier (withdraw), encrypt the note at rest (Argon2id +
//! ChaCha20-Poly1305), and surface the A.8 / A.12 privacy warnings. Proof
//! generation + tx submission reuse the existing pipeline
//! (scripts/groth16-prove-note.sh, tests/*.mjs).
//!
//! Passphrase for note encryption is read from $OPAQ_PASSPHRASE.

use std::collections::HashMap;
use std::process::exit;

use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use ark_std::UniformRand;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use opaq_common::{be32, field_hex, poseidon_be, to_field_be, tree};
use rand::RngCore;
use serde_json::Value;

mod warn;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("deposit") => deposit(&flags(&args[2..])),
        Some("withdraw") => withdraw(&flags(&args[2..])),
        Some("transfer") => transfer(&flags(&args[2..])),
        _ => Err("usage: opaq <deposit|withdraw|transfer> ...".into()),
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        exit(1);
    }
}

type R = Result<(), String>;

fn deposit(f: &HashMap<String, String>) -> R {
    let mint = pubkey(req(f, "token")?)?;
    let amount: u64 = req(f, "amount")?.parse().map_err(|_| "amount must be u64")?;
    let note_path = req(f, "note")?;

    // Fresh secrets, then the commitment exactly as the circuit computes it.
    let spend_key = random_scalar();
    let blinding = random_scalar();
    let owner_pubkey = poseidon_be(&[spend_key]); // hash_1
    let token_id = to_field_be(&mint);
    let commitment = poseidon_be(&[token_id, be32(amount as u128), owner_pubkey, blinding]); // hash_4

    let note = serde_json::json!({
        "mint": hex::encode(mint),
        "amount": amount,
        "spend_key": hex::encode(spend_key),
        "blinding": hex::encode(blinding),
        "owner_pubkey": hex::encode(owner_pubkey),
        "commitment": hex::encode(commitment),
        "leaf_index": Value::Null, // filled in after the deposit lands on-chain
    });
    write_note(note_path, &note)?;
    println!("note written (encrypted) -> {note_path}");

    // The deposit circuit's ABI inputs for this real note (always built; written
    // to --inputs-out and/or proved with --prove below).
    let inputs = serde_json::json!({
        "token_id": opaq_common::field_hex(&token_id),
        "amount": opaq_common::field_hex(&be32(amount as u128)),
        "new_commitment": opaq_common::field_hex(&commitment),
        "owner_pubkey": opaq_common::field_hex(&owner_pubkey),
        "blinding_factor": opaq_common::field_hex(&blinding),
    })
    .to_string();
    if let Some(p) = f.get("inputs-out") {
        std::fs::write(p, &inputs).map_err(|e| format!("write {p}: {e}"))?;
        println!("circuit inputs   -> {p}");
    }
    println!("\ndeposit public inputs (submit these with a proof):");
    println!("  token_id   {}", bs58::encode(mint).into_string());
    println!("  amount     {amount}");
    println!("  commitment 0x{}", hex::encode(commitment));

    // M9(a): prove the binding proof + assemble the blob (--prove); with --submit,
    // also sign + broadcast the deposit tx (moves SPL into the vault, inserts the
    // commitment). Symmetric to withdraw, but the deposit blob carries the raw
    // mint + commitment (no merkle/nullifier), so those sidecar fields are zero.
    if f.contains_key("prove") || f.contains_key("submit") {
        let out = f.get("out").map(String::as_str).unwrap_or("deposit.bin");
        let z = "00".repeat(32);
        let sidecar = format!(
            "{{\"mint_hex\":\"{}\",\"amount\":{},\"commitment\":\"{}\",\
             \"nullifier\":\"{z}\",\"merkle_root\":\"{z}\",\"recipient_hex\":\"{z}\"}}",
            hex::encode(mint),
            amount,
            hex::encode(commitment),
        );
        prove_and_emit("deposit", &inputs, &sidecar, out, f)?;
        println!("deposit instruction blob -> {out}");

        if f.contains_key("submit") {
            let sig = submit_deposit(out, &mint, f)?;
            println!("deposit submitted ✓ tx {sig}");
        }
    }

    warn::amount(amount);
    Ok(())
}

fn withdraw(f: &HashMap<String, String>) -> R {
    let note = read_note(req(f, "note")?)?;
    let recipient = pubkey(req(f, "recipient")?)?;

    let commitment = hex32(note["commitment"].as_str().ok_or("note missing commitment")?)?;
    let spend_key = hex32(note["spend_key"].as_str().ok_or("note missing spend_key")?)?;
    let mint = hex32(note["mint"].as_str().ok_or("note missing mint")?)?;
    let amount = note["amount"].as_u64().ok_or("note missing amount")?;

    let nullifier = poseidon_be(&[commitment, spend_key]); // hash_2

    println!("withdraw public inputs (submit these with a proof):");
    println!("  nullifier  0x{}", hex::encode(nullifier));
    println!("  token_id   {}", bs58::encode(mint).into_string());
    println!("  amount     {amount}");
    println!("  recipient  {}", bs58::encode(recipient).into_string());

    // Zero-infra read path (M10 / Test 7): obtain the ordered commitment list and
    // rebuild this note's Merkle authentication path locally — no indexer, no
    // special access. Leaves come either from a pre-harvested `--leaves` file or,
    // with `--rpc <url> --program <id>`, harvested live from chain here (M9).
    let leaves = if let Some(leaves_path) = f.get("leaves") {
        Some(load_leaves(leaves_path)?)
    } else if let (Some(rpc), Some(program)) = (f.get("rpc"), f.get("program")) {
        println!("  harvesting leaves over RPC ({rpc}) …");
        Some(harvest_leaves(rpc, program)?)
    } else {
        None
    };

    if let Some(leaves) = leaves {
        let leaf_index = leaves
            .iter()
            .position(|c| *c == commitment)
            .ok_or("note commitment not found in on-chain leaves (wrong pool/passphrase?)")?
            as u64;

        let blinding = hex32(note["blinding"].as_str().ok_or("note missing blinding")?)?;
        let zeros = tree::zero_hashes(&poseidon2);
        let (siblings, right, merkle_root) =
            tree::reconstruct_path(&poseidon2, &zeros, &leaves, leaf_index);

        let token_id = to_field_be(&mint);
        let recipient_field = to_field_be(&recipient);
        println!("  leaf_index {leaf_index} (located in on-chain leaf set)");
        println!("  merkle_root 0x{}", hex::encode(merkle_root));

        // The circuit inputs for this reconstructed path (always built; written to
        // --inputs-out and/or proved with --prove below).
        let path: Vec<String> = siblings.iter().map(|s| format!("\"{}\"", field_hex(s))).collect();
        let idx: Vec<String> = right.iter().map(|b| b.to_string()).collect();
        let inputs = format!(
            "{{\"merkle_root\":\"{}\",\"nullifier\":\"{}\",\"token_id\":\"{}\",\
             \"amount\":\"{}\",\"recipient\":\"{}\",\"spend_key\":\"{}\",\
             \"blinding_factor\":\"{}\",\"merkle_path\":[{}],\
             \"merkle_path_indices\":[{}]}}",
            field_hex(&merkle_root),
            field_hex(&nullifier),
            field_hex(&token_id),
            field_hex(&be32(amount as u128)),
            field_hex(&recipient_field),
            field_hex(&spend_key),
            field_hex(&blinding),
            path.join(","),
            idx.join(","),
        );
        if let Some(p) = f.get("inputs-out") {
            std::fs::write(p, &inputs).map_err(|e| format!("write {p}: {e}"))?;
            println!("withdraw circuit inputs -> {p}");
        }

        // M9(a): generate the Groth16 proof and assemble the ready-to-submit
        // instruction blob (--prove), orchestrating the same tested toolchain the
        // test harness uses (groth16-prove-note.sh + emit_opaq_instruction). With
        // --submit, also sign + broadcast the tx via the node submit helper.
        if f.contains_key("prove") || f.contains_key("submit") {
            let out = f.get("out").map(String::as_str).unwrap_or("withdraw.bin");
            let sidecar = format!(
                "{{\"merkle_root\":\"{}\",\"nullifier\":\"{}\",\"mint_hex\":\"{}\",\
                 \"amount\":{},\"recipient_hex\":\"{}\",\"commitment\":\"{}\"}}",
                hex::encode(merkle_root),
                hex::encode(nullifier),
                hex::encode(mint),
                amount,
                hex::encode(recipient),
                "00".repeat(32), // unused by withdraw; emit_opaq_instruction still parses it
            );
            prove_and_emit("withdraw", &inputs, &sidecar, out, f)?;
            println!("withdraw instruction blob -> {out}");

            if f.contains_key("submit") {
                let sig = submit_withdraw(out, &mint, &recipient, f)?;
                println!("withdraw submitted ✓ tx {sig}");
            }
        }
    } else {
        println!(
            "  merkle_root + path: pass --rpc <url> --program <id> to harvest leaves \
             live from chain, or --leaves <file> for a pre-harvested list — zero-infra \
             read path, M9/M10"
        );
    }

    // A.8: when an RPC endpoint is available, turn the recipient warning from a
    // static advisory into a concrete fresh/not-fresh finding.
    let history = match f.get("rpc") {
        Some(rpc) => match recipient_history(rpc, &bs58::encode(recipient).into_string()) {
            Ok(h) => Some(h),
            Err(e) => {
                eprintln!("  (recipient history check skipped: {e})");
                None
            }
        },
        None => None,
    };
    warn::recipient(&recipient, history);
    warn::amount(amount);
    Ok(())
}

/// Transfer (Phase 2): spend one note privately — send `--amount` to a recipient
/// (their owner_pubkey, hex) and keep the change in a fresh self note — via a
/// 2-in/2-out join-split (the 2nd input is a dummy). Hides the amount and token.
/// Needs --rpc/--program (to harvest + reconstruct the input's Merkle path) or
/// --leaves; --prove emits the blob, --submit also broadcasts it.
fn transfer(f: &HashMap<String, String>) -> R {
    let note = read_note(req(f, "note")?)?;
    let mint = hex32(note["mint"].as_str().ok_or("note missing mint")?)?;
    let in_amount = note["amount"].as_u64().ok_or("note missing amount")?;
    let in_spend_key = hex32(note["spend_key"].as_str().ok_or("note missing spend_key")?)?;
    let in_blinding = hex32(note["blinding"].as_str().ok_or("note missing blinding")?)?;
    let in_commitment = hex32(note["commitment"].as_str().ok_or("note missing commitment")?)?;
    let token_id = to_field_be(&mint);

    let recipient_owner = hex32(req(f, "to")?)?; // recipient's owner_pubkey = hash_1(their spend_key)
    let send_amount: u64 = req(f, "amount")?.parse().map_err(|_| "amount must be u64")?;
    if send_amount > in_amount {
        return Err(format!("send amount {send_amount} exceeds note amount {in_amount}"));
    }
    let change = in_amount - send_amount;
    let in_nullifier = poseidon_be(&[in_commitment, in_spend_key]); // hash_2

    // Dummy 2nd input (amount 0, skips Merkle membership in-circuit).
    let dummy_sk = random_scalar();
    let dummy_bl = random_scalar();
    let dummy_owner = poseidon_be(&[dummy_sk]);
    let dummy_commit = poseidon_be(&[token_id, be32(0), dummy_owner, dummy_bl]);
    let dummy_nf = poseidon_be(&[dummy_commit, dummy_sk]);

    // out[0] -> recipient; out[1] -> change in a FRESH self note (fresh secrets).
    let out0_bl = random_scalar();
    let out0_amt = be32(send_amount as u128);
    let out0_commit = poseidon_be(&[token_id, out0_amt, recipient_owner, out0_bl]);
    let change_sk = random_scalar();
    let change_bl = random_scalar();
    let change_owner = poseidon_be(&[change_sk]);
    let out1_amt = be32(change as u128);
    let out1_commit = poseidon_be(&[token_id, out1_amt, change_owner, change_bl]);

    // Reconstruct the input note's Merkle path (zero-infra read path, like withdraw).
    let leaves = if let (Some(rpc), Some(program)) = (f.get("rpc"), f.get("program")) {
        println!("  harvesting leaves over RPC ({rpc}) …");
        harvest_leaves(rpc, program)?
    } else if let Some(p) = f.get("leaves") {
        load_leaves(p)?
    } else {
        return Err("transfer needs --rpc <url> --program <id> (or --leaves <file>)".to_string());
    };
    let leaf_index = leaves
        .iter()
        .position(|c| *c == in_commitment)
        .ok_or("input note commitment not found in on-chain leaves (wrong pool/passphrase?)")?
        as u64;
    let zeros = tree::zero_hashes(&poseidon2);
    let (siblings, right, merkle_root) =
        tree::reconstruct_path(&poseidon2, &zeros, &leaves, leaf_index);

    println!(
        "transfer: spend {in_amount} (leaf {leaf_index}) -> send {send_amount} to recipient, {change} change"
    );
    println!("  merkle_root 0x{}", hex::encode(merkle_root));

    // Build the transfer witness (input[0] real, input[1] dummy).
    let fh = |x: &[u8; 32]| field_hex(x);
    let path_j = siblings.iter().map(|s| format!("\"{}\"", field_hex(s))).collect::<Vec<_>>().join(",");
    let idx_j = right.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",");
    let zeros_j = vec!["\"0x00\"".to_string(); 24].join(",");
    let false_j = vec!["false".to_string(); 24].join(",");
    let inputs = format!(
        "{{\"merkle_root\":\"{}\",\"nullifiers\":[\"{}\",\"{}\"],\"out_commitments\":[\"{}\",\"{}\"],\
         \"token_id\":\"{}\",\"in_amount\":[\"{}\",\"{}\"],\"in_spend_key\":[\"{}\",\"{}\"],\
         \"in_blinding\":[\"{}\",\"{}\"],\"in_is_dummy\":[false,true],\
         \"in_merkle_path\":[[{}],[{}]],\"in_merkle_path_indices\":[[{}],[{}]],\
         \"out_amount\":[\"{}\",\"{}\"],\"out_owner_pubkey\":[\"{}\",\"{}\"],\"out_blinding\":[\"{}\",\"{}\"]}}",
        fh(&merkle_root), fh(&in_nullifier), fh(&dummy_nf), fh(&out0_commit), fh(&out1_commit),
        fh(&token_id), fh(&be32(in_amount as u128)), fh(&be32(0)),
        fh(&in_spend_key), fh(&dummy_sk), fh(&in_blinding), fh(&dummy_bl),
        path_j, zeros_j, idx_j, false_j,
        fh(&out0_amt), fh(&out1_amt), fh(&recipient_owner), fh(&change_owner), fh(&out0_bl), fh(&change_bl),
    );

    // Persist the output notes: change to self (encrypted), recipient's as plaintext
    // to hand over (they hold the spend_key behind recipient_owner).
    if let Some(p) = f.get("change-note") {
        let cn = serde_json::json!({
            "mint": hex::encode(mint), "amount": change,
            "spend_key": hex::encode(change_sk), "blinding": hex::encode(change_bl),
            "owner_pubkey": hex::encode(change_owner), "commitment": hex::encode(out1_commit),
            "leaf_index": Value::Null,
        });
        write_note(p, &cn)?;
        println!("change note (encrypted) -> {p}");
    }
    if let Some(p) = f.get("out-note") {
        let on = serde_json::json!({
            "mint": hex::encode(mint), "amount": send_amount,
            "owner_pubkey": hex::encode(recipient_owner), "blinding": hex::encode(out0_bl),
            "commitment": hex::encode(out0_commit), "leaf_index": Value::Null,
        });
        std::fs::write(p, on.to_string()).map_err(|e| format!("write {p}: {e}"))?;
        println!("recipient note (plaintext, hand to recipient) -> {p}");
    }

    if f.contains_key("prove") || f.contains_key("submit") {
        let out = f.get("out").map(String::as_str).unwrap_or("transfer.bin");
        prove_and_emit("transfer", &inputs, "{}", out, f)?; // sidecar unused for transfer
        println!("transfer instruction blob -> {out}");
        if f.contains_key("submit") {
            let sig = submit_transfer(out, f)?;
            println!("transfer submitted ✓ tx {sig}");
        }
    }

    warn::amount(send_amount);
    Ok(())
}

/// Sign + broadcast a transfer tx via the node submit helper (accounts: payer,
/// tree, nullifiers, system — no vault). Override via $OPAQ_SUBMIT_TRANSFER_SCRIPT.
fn submit_transfer(blob: &str, f: &HashMap<String, String>) -> Result<String, String> {
    let rpc = f.get("rpc").ok_or("--submit requires --rpc <url>")?;
    let program = f.get("program").ok_or("--submit requires --program <id>")?;
    let payer = f.get("payer").ok_or("--submit requires --payer <keypair.json>")?;
    let script = std::env::var("OPAQ_SUBMIT_TRANSFER_SCRIPT")
        .unwrap_or_else(|_| "tests/submit_transfer.mjs".to_string());
    let out = std::process::Command::new("node")
        .args([&script, rpc, program, blob, payer])
        .output()
        .map_err(|e| format!("spawn node {script}: {e}"))?;
    if !out.status.success() {
        return Err(format!("submit failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 2-input Poseidon over big-endian field elements — the tree's hash, proven
/// byte-identical to the circuit + on-chain syscall in M0.
fn poseidon2(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    poseidon_be(&[*a, *b])
}

/// Parse the ordered commitment (leaf) list: a JSON array of 32-byte hex
/// strings, index `i` == on-chain `leaf_index` `i`. Shared by the `--leaves`
/// file path and the `--rpc` harvest path.
fn parse_leaves(bytes: &[u8]) -> Result<Vec<[u8; 32]>, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "leaves are not valid JSON")?;
    let arr = v.as_array().ok_or("leaves must be a JSON array of hex commitments")?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, e) in arr.iter().enumerate() {
        let s = e.as_str().ok_or_else(|| format!("leaf {i} is not a string"))?;
        out.push(hex32(s)?);
    }
    Ok(out)
}

/// Load the ordered leaf list from a pre-harvested file.
fn load_leaves(path: &str) -> Result<Vec<[u8; 32]>, String> {
    parse_leaves(&std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?)
}

/// Harvest the ordered leaf list directly from chain over plain RPC, so a
/// withdraw works against ANY live pool state (closing the M11 fresh-pool
/// shortcut). RPC logic lives in the tested node read path (tests/read_path.mjs
/// via read_leaves.mjs); this orchestrates it so there's one source of truth.
/// Override the script with $OPAQ_READ_SCRIPT (default: tests/read_leaves.mjs).
fn harvest_leaves(rpc: &str, program: &str) -> Result<Vec<[u8; 32]>, String> {
    let script = std::env::var("OPAQ_READ_SCRIPT")
        .unwrap_or_else(|_| "tests/read_leaves.mjs".to_string());
    let out = std::process::Command::new("node")
        .args([&script, rpc, program])
        .output()
        .map_err(|e| format!("spawn node {script}: {e} (is node installed / cwd repo root?)"))?;
    if !out.status.success() {
        return Err(format!(
            "read path failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_leaves(&out.stdout)
}

/// Prove a circuit's reconstructed inputs and assemble the on-chain instruction
/// blob, orchestrating the tested toolchain (scripts/groth16-prove-note.sh + the
/// emit_opaq_instruction bin — kept as the single source of the on-chain layout).
/// Repo root via $OPAQ_ROOT (default "."); the fixed zkey via --zkey or
/// $OPAQ_<CIRCUIT>_ZKEY.
fn prove_and_emit(
    circuit: &str,
    inputs: &str,
    sidecar: &str,
    out: &str,
    f: &HashMap<String, String>,
) -> R {
    let root = std::env::var("OPAQ_ROOT").unwrap_or_else(|_| ".".to_string());
    let zkey = f
        .get("zkey")
        .cloned()
        .or_else(|| std::env::var(format!("OPAQ_{}_ZKEY", circuit.to_uppercase())).ok())
        .ok_or_else(|| {
            format!("--zkey <circuit.zkey> (or $OPAQ_{}_ZKEY) required for --prove", circuit.to_uppercase())
        })?;

    let tmp = std::env::temp_dir();
    let pid = std::process::id();
    let inputs_path = tmp.join(format!("opaq-{circuit}-inputs-{pid}.json"));
    let sidecar_path = tmp.join(format!("opaq-{circuit}-sidecar-{pid}.json"));
    let provedir = tmp.join(format!("opaq-{circuit}-prove-{pid}"));
    std::fs::write(&inputs_path, inputs).map_err(|e| format!("write inputs: {e}"))?;
    std::fs::write(&sidecar_path, sidecar).map_err(|e| format!("write sidecar: {e}"))?;

    run("bash", &[
        &format!("{root}/scripts/groth16-prove-note.sh"),
        circuit, &zkey, inputs_path.to_str().unwrap(), provedir.to_str().unwrap(),
    ])?;
    run("cargo", &[
        "run", "-q", "--manifest-path", &format!("{root}/crates/groth16-verify/Cargo.toml"),
        "--bin", "emit_opaq_instruction", "--",
        circuit, provedir.to_str().unwrap(), sidecar_path.to_str().unwrap(), out,
    ])
}

/// Sign + broadcast a withdraw tx via the node submit helper (chain I/O stays in
/// the node path, where the account layout lives). Needs --rpc, --program, and
/// --payer <keypair.json>. Override the script with $OPAQ_SUBMIT_SCRIPT
/// (default: tests/submit_withdraw.mjs). Returns the confirmed tx signature.
fn submit_withdraw(
    blob: &str,
    mint: &[u8; 32],
    recipient: &[u8; 32],
    f: &HashMap<String, String>,
) -> Result<String, String> {
    let rpc = f.get("rpc").ok_or("--submit requires --rpc <url>")?;
    let program = f.get("program").ok_or("--submit requires --program <id>")?;
    let payer = f.get("payer").ok_or("--submit requires --payer <keypair.json>")?;
    let script = std::env::var("OPAQ_SUBMIT_SCRIPT")
        .unwrap_or_else(|_| "tests/submit_withdraw.mjs".to_string());
    let out = std::process::Command::new("node")
        .args([
            &script, rpc, program, blob, payer,
            &hex::encode(mint), &bs58::encode(recipient).into_string(),
        ])
        .output()
        .map_err(|e| format!("spawn node {script}: {e}"))?;
    if !out.status.success() {
        return Err(format!("submit failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Sign + broadcast a deposit tx via the node submit helper. Needs --rpc,
/// --program, --payer <keypair.json>; the payer is the depositor (its token
/// account must already hold `amount` of the mint). Override the script with
/// $OPAQ_SUBMIT_DEPOSIT_SCRIPT (default: tests/submit_deposit.mjs).
fn submit_deposit(blob: &str, mint: &[u8; 32], f: &HashMap<String, String>) -> Result<String, String> {
    let rpc = f.get("rpc").ok_or("--submit requires --rpc <url>")?;
    let program = f.get("program").ok_or("--submit requires --program <id>")?;
    let payer = f.get("payer").ok_or("--submit requires --payer <keypair.json>")?;
    let script = std::env::var("OPAQ_SUBMIT_DEPOSIT_SCRIPT")
        .unwrap_or_else(|_| "tests/submit_deposit.mjs".to_string());
    let out = std::process::Command::new("node")
        .args([&script, rpc, program, blob, payer, &hex::encode(mint)])
        .output()
        .map_err(|e| format!("spawn node {script}: {e}"))?;
    if !out.status.success() {
        return Err(format!("submit failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Spawn a command inheriting stdio, erroring on non-zero exit.
fn run(cmd: &str, args: &[&str]) -> R {
    let st = std::process::Command::new(cmd)
        .args(args)
        .status()
        .map_err(|e| format!("spawn {cmd}: {e}"))?;
    if !st.success() {
        return Err(format!("`{cmd} {}` failed", args.join(" ")));
    }
    Ok(())
}

/// Look up a recipient's prior on-chain signature count over plain RPC (A.8), via
/// the tested node read path. Override the script with $OPAQ_RECIPIENT_SCRIPT
/// (default: tests/recipient_history.mjs).
fn recipient_history(rpc: &str, recipient_b58: &str) -> Result<warn::RecipientHistory, String> {
    let script = std::env::var("OPAQ_RECIPIENT_SCRIPT")
        .unwrap_or_else(|_| "tests/recipient_history.mjs".to_string());
    let out = std::process::Command::new("node")
        .args([&script, rpc, recipient_b58])
        .output()
        .map_err(|e| format!("spawn node {script}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "recipient history check failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| "recipient history is not valid JSON")?;
    Ok(warn::RecipientHistory {
        count: v["count"].as_u64().ok_or("history missing count")? as usize,
        capped: v["capped"].as_bool().unwrap_or(false),
    })
}

// --- helpers ---

fn flags(args: &[String]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        if let Some(k) = args[i].strip_prefix("--") {
            // A following non-flag token is the value; otherwise it's a boolean
            // flag (e.g. --prove / --submit), recorded with an empty value.
            let val = match args.get(i + 1) {
                Some(v) if !v.starts_with("--") => {
                    i += 1;
                    v.clone()
                }
                _ => String::new(),
            };
            m.insert(k.to_string(), val);
        }
        i += 1;
    }
    m
}

fn req<'a>(f: &'a HashMap<String, String>, k: &str) -> Result<&'a str, String> {
    f.get(k).map(String::as_str).ok_or_else(|| format!("missing --{k}"))
}

fn to32(v: Vec<u8>) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[32 - v.len()..].copy_from_slice(&v);
    o
}

fn random_scalar() -> [u8; 32] {
    to32(Fr::rand(&mut rand::thread_rng()).into_bigint().to_bytes_be())
}

fn hex32(s: &str) -> Result<[u8; 32], String> {
    hex::decode(s.trim_start_matches("0x"))
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| "expected 32-byte hex".into())
}

/// Accept a 32-byte pubkey as base58 (Solana) or 0x-hex.
fn pubkey(s: &str) -> Result<[u8; 32], String> {
    if let Ok(b) = hex::decode(s.trim_start_matches("0x")) {
        if b.len() == 32 {
            return Ok(b.try_into().unwrap());
        }
    }
    bs58::decode(s)
        .into_vec()
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| format!("invalid pubkey: {s}"))
}

fn passphrase() -> Result<String, String> {
    std::env::var("OPAQ_PASSPHRASE")
        .map_err(|_| "set $OPAQ_PASSPHRASE to encrypt/decrypt the note".into())
}

fn derive_key(pass: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(pass.as_bytes(), salt, &mut key)
        .expect("argon2");
    key
}

fn write_note(path: &str, note: &Value) -> R {
    let pass = passphrase()?;
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&derive_key(&pass, &salt)));
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), note.to_string().as_bytes())
        .map_err(|_| "encrypt failed")?;
    let env = serde_json::json!({
        "v": 1, "salt": hex::encode(salt), "nonce": hex::encode(nonce), "ct": hex::encode(ct),
    });
    std::fs::write(path, env.to_string()).map_err(|e| format!("write {path}: {e}"))
}

fn read_note(path: &str) -> Result<Value, String> {
    let pass = passphrase()?;
    let env: Value = serde_json::from_slice(
        &std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?,
    )
    .map_err(|_| "note is not valid JSON")?;
    let salt = hex::decode(env["salt"].as_str().ok_or("note missing salt")?).map_err(|_| "bad salt")?;
    let nonce = hex::decode(env["nonce"].as_str().ok_or("note missing nonce")?).map_err(|_| "bad nonce")?;
    let ct = hex::decode(env["ct"].as_str().ok_or("note missing ct")?).map_err(|_| "bad ct")?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&derive_key(&pass, &salt)));
    let pt = cipher
        .decrypt(Nonce::from_slice(&nonce), ct.as_ref())
        .map_err(|_| "decrypt failed (wrong passphrase?)")?;
    serde_json::from_slice(&pt).map_err(|_| "decrypted note is not valid JSON".into())
}

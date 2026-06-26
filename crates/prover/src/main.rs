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
use opaq_common::{be32, poseidon_be, to_field_be};
use rand::RngCore;
use serde_json::Value;

mod warn;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("deposit") => deposit(&flags(&args[2..])),
        Some("withdraw") => withdraw(&flags(&args[2..])),
        _ => Err("usage: opaq <deposit|withdraw> ...".into()),
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

    // Optional: emit the deposit circuit's ABI inputs for this real note, so it
    // can be proved directly (scripts/groth16-prove-note.sh deposit <zkey> <this>).
    if let Some(p) = f.get("inputs-out") {
        let inputs = serde_json::json!({
            "token_id": opaq_common::field_hex(&token_id),
            "amount": opaq_common::field_hex(&be32(amount as u128)),
            "new_commitment": opaq_common::field_hex(&commitment),
            "owner_pubkey": opaq_common::field_hex(&owner_pubkey),
            "blinding_factor": opaq_common::field_hex(&blinding),
        });
        std::fs::write(p, inputs.to_string()).map_err(|e| format!("write {p}: {e}"))?;
        println!("circuit inputs   -> {p}");
    }
    println!("\ndeposit public inputs (submit these with a proof):");
    println!("  token_id   {}", bs58::encode(mint).into_string());
    println!("  amount     {amount}");
    println!("  commitment 0x{}", hex::encode(commitment));
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
    println!(
        "  merkle_root + path: reconstruct from on-chain tree state for leaf_index {} \
         (zero-infra read path, M10)",
        note["leaf_index"]
    );
    warn::recipient(&recipient);
    warn::amount(amount);
    Ok(())
}

// --- helpers ---

fn flags(args: &[String]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut i = 0;
    while i + 1 < args.len() {
        if let Some(k) = args[i].strip_prefix("--") {
            m.insert(k.to_string(), args[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
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

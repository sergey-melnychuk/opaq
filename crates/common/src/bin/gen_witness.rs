//! M1 witness generator: writes deterministic `Prover.toml` files for the
//! deposit and withdraw circuits so they can be proved/verified locally with bb.
//!
//! All field values are computed with `light-poseidon` — already proven
//! byte-identical to the circuit's Poseidon in M0 — so a successful
//! `nargo execute` (which re-checks every assert) is itself cross-validation
//! that the off-chain witness math matches the in-circuit math.
//!
//! Usage: cargo run -p opaq-common --bin gen_witness -- <circuits_dir>

use std::{fs, path::PathBuf};

use opaq_common::{be32, field_hex, merkle_root_be, poseidon_be, to_field_be};

const TREE_DEPTH: usize = 24;

fn main() {
    let circuits_dir = PathBuf::from(
        std::env::args().nth(1).unwrap_or_else(|| "circuits".to_string()),
    );

    // Deterministic note secrets (M1 fixtures, not real randomness).
    let spend_key = be32(987_654_321);
    let blinding = be32(
        std::env::var("OPAQ_BLINDING").ok().and_then(|s| s.parse().ok()).unwrap_or(123_456_789),
    );
    // Real test params come from env (the M8 e2e harness); otherwise dummy fixtures.
    let amount_u64: u64 = std::env::var("OPAQ_AMOUNT").ok().and_then(|s| s.parse().ok()).unwrap_or(100);
    let amount = be32(amount_u64 as u128);
    let mint_bytes = hex32_env("OPAQ_MINT_HEX").unwrap_or_else(|| dummy_pubkey(0xAB));
    let recipient_bytes = hex32_env("OPAQ_RECIPIENT_HEX").unwrap_or_else(|| dummy_pubkey(0xCD));
    let token_id = to_field_be(&mint_bytes); // canonical mint encoding
    let recipient = to_field_be(&recipient_bytes); // canonical recipient encoding

    // Derivations matching the circuits.
    let owner_pubkey = poseidon_be(&[spend_key]); // hash_1([spend_key])
    let commitment = poseidon_be(&[token_id, amount, owner_pubkey, blinding]); // hash_4
    let nullifier = poseidon_be(&[commitment, spend_key]); // hash_2([commitment, spend_key])

    // --- deposit/Prover.toml ---
    let deposit = format!(
        "token_id = \"{}\"\n\
         amount = \"{}\"\n\
         new_commitment = \"{}\"\n\
         owner_pubkey = \"{}\"\n\
         blinding_factor = \"{}\"\n",
        field_hex(&token_id),
        field_hex(&amount),
        field_hex(&commitment),
        field_hex(&owner_pubkey),
        field_hex(&blinding),
    );
    write(&circuits_dir, "deposit", &deposit);
    write_json(
        &circuits_dir,
        "deposit",
        &[
            ("token_id", field_hex(&token_id)),
            ("amount", field_hex(&amount)),
            ("new_commitment", field_hex(&commitment)),
            ("owner_pubkey", field_hex(&owner_pubkey)),
            ("blinding_factor", field_hex(&blinding)),
        ],
        &[],
    );

    // --- withdraw/Prover.toml ---
    // Real empty-tree state: leaf at index 0, all-left path, siblings = the
    // empty-subtree hashes (zero_hashes), NOT all-zero. This makes the proof's
    // merkle_root equal the root CommitmentTree produces after inserting the
    // commitment at index 0 (cross-checked by the M4 tree tests) — so the
    // circuit and the on-chain tree agree (needed for M6/M7).
    let siblings = opaq_common::tree::zero_hashes(&|a: &[u8; 32], b: &[u8; 32]| {
        opaq_common::poseidon_hash2_be(a, b)
    });
    let right = [false; TREE_DEPTH];
    let merkle_root = merkle_root_be(commitment, &siblings, &right);

    let path_list = siblings
        .iter()
        .map(|s| format!("\"{}\"", field_hex(s)))
        .collect::<Vec<_>>()
        .join(", ");
    let idx_list = right
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let withdraw = format!(
        "merkle_root = \"{}\"\n\
         nullifier = \"{}\"\n\
         token_id = \"{}\"\n\
         amount = \"{}\"\n\
         recipient = \"{}\"\n\
         spend_key = \"{}\"\n\
         blinding_factor = \"{}\"\n\
         merkle_path = [{}]\n\
         merkle_path_indices = [{}]\n",
        field_hex(&merkle_root),
        field_hex(&nullifier),
        field_hex(&token_id),
        field_hex(&amount),
        field_hex(&recipient),
        field_hex(&spend_key),
        field_hex(&blinding),
        path_list,
        idx_list,
    );
    write(&circuits_dir, "withdraw", &withdraw);
    write_json(
        &circuits_dir,
        "withdraw",
        &[
            ("merkle_root", field_hex(&merkle_root)),
            ("nullifier", field_hex(&nullifier)),
            ("token_id", field_hex(&token_id)),
            ("amount", field_hex(&amount)),
            ("recipient", field_hex(&recipient)),
            ("spend_key", field_hex(&spend_key)),
            ("blinding_factor", field_hex(&blinding)),
        ],
        &[
            // pre-formatted JSON: Field elements are quoted "0x.." strings,
            // bools are raw JSON true/false (the ABI parser wants real booleans).
            ("merkle_path", siblings.iter().map(|s| format!("\"{}\"", field_hex(s))).collect()),
            ("merkle_path_indices", right.iter().map(|b| b.to_string()).collect()),
        ],
    );

    // --- burn/Prover.toml (Phase 3): like withdraw, EVM destination, no SPL ---
    let dest_chain = be32(1); // Ethereum mainnet chain id
    let mut dest_address = [0u8; 32]; // a dummy 20-byte EVM address as a BE field
    dest_address[12..32].copy_from_slice(&[0x11; 20]);
    let burn = format!(
        "merkle_root = \"{}\"\nnullifier = \"{}\"\ntoken_id = \"{}\"\namount = \"{}\"\n\
         dest_chain = \"{}\"\ndest_address = \"{}\"\nspend_key = \"{}\"\nblinding_factor = \"{}\"\n\
         merkle_path = [{}]\nmerkle_path_indices = [{}]\n",
        field_hex(&merkle_root), field_hex(&nullifier), field_hex(&token_id), field_hex(&amount),
        field_hex(&dest_chain), field_hex(&dest_address), field_hex(&spend_key), field_hex(&blinding),
        path_list, idx_list,
    );
    write(&circuits_dir, "burn", &burn);
    write_json(
        &circuits_dir,
        "burn",
        &[
            ("merkle_root", field_hex(&merkle_root)),
            ("nullifier", field_hex(&nullifier)),
            ("token_id", field_hex(&token_id)),
            ("amount", field_hex(&amount)),
            ("dest_chain", field_hex(&dest_chain)),
            ("dest_address", field_hex(&dest_address)),
            ("spend_key", field_hex(&spend_key)),
            ("blinding_factor", field_hex(&blinding)),
        ],
        &[
            ("merkle_path", siblings.iter().map(|s| format!("\"{}\"", field_hex(s))).collect()),
            ("merkle_path_indices", right.iter().map(|b| b.to_string()).collect()),
        ],
    );
    // burn sidecar for emit_opaq_instruction (mint/amount + the bound EVM dest).
    let burn_sidecar = format!(
        "{{\"mint_hex\":\"{}\",\"amount\":{},\"commitment\":\"{}\",\"nullifier\":\"{}\",\
         \"merkle_root\":\"{}\",\"dest_chain\":\"{}\",\"recipient_hex\":\"{}\"}}\n",
        hex::encode(mint_bytes),
        amount_u64,
        hex::encode(commitment),
        hex::encode(nullifier),
        hex::encode(merkle_root),
        hex::encode(dest_chain),
        hex::encode(dest_address),
    );
    fs::write(circuits_dir.join("burn_values.json"), burn_sidecar).unwrap();

    // --- xburn/Prover.toml (Phase 4, P4.0/P4.1): symmetric cross-chain burn ---
    // Reuses the same source note/path as withdraw/burn above (leaf 0). The
    // destination note gets a fresh owner + blinding, same token_id/amount
    // (B.12.2 conservation). dest_chain defaults to burn's Ethereum-mainnet
    // fixture but is independently overridable — an EVM-origin xburn destined
    // for SOLANA (m19/P4.1) needs SOLANA_CHAIN_ID (101, programs/opaq/src/lib.rs),
    // not Ethereum's.
    let xburn_dest_chain = std::env::var("OPAQ_XBURN_DEST_CHAIN")
        .ok()
        .and_then(|s| s.parse::<u128>().ok())
        .map(be32)
        .unwrap_or(dest_chain);
    let dest_owner_pubkey = poseidon_be(&[be32(848_484_848)]);
    let dest_blinding = be32(272_727_272);
    let out_commitment = poseidon_be(&[token_id, amount, dest_owner_pubkey, dest_blinding]);
    let xburn = format!(
        "src_merkle_root = \"{}\"\nsrc_nullifier = \"{}\"\ndest_chain = \"{}\"\nout_commitment = \"{}\"\n\
         token_id = \"{}\"\namount = \"{}\"\nsrc_spend_key = \"{}\"\nsrc_blinding = \"{}\"\n\
         src_merkle_path = [{}]\nsrc_merkle_path_indices = [{}]\n\
         dest_owner_pubkey = \"{}\"\ndest_blinding = \"{}\"\n",
        field_hex(&merkle_root), field_hex(&nullifier), field_hex(&xburn_dest_chain), field_hex(&out_commitment),
        field_hex(&token_id), field_hex(&amount), field_hex(&spend_key), field_hex(&blinding),
        path_list, idx_list,
        field_hex(&dest_owner_pubkey), field_hex(&dest_blinding),
    );
    write(&circuits_dir, "xburn", &xburn);
    write_json(
        &circuits_dir,
        "xburn",
        &[
            ("src_merkle_root", field_hex(&merkle_root)),
            ("src_nullifier", field_hex(&nullifier)),
            ("dest_chain", field_hex(&xburn_dest_chain)),
            ("out_commitment", field_hex(&out_commitment)),
            ("token_id", field_hex(&token_id)),
            ("amount", field_hex(&amount)),
            ("src_spend_key", field_hex(&spend_key)),
            ("src_blinding", field_hex(&blinding)),
            ("dest_owner_pubkey", field_hex(&dest_owner_pubkey)),
            ("dest_blinding", field_hex(&dest_blinding)),
        ],
        &[
            ("src_merkle_path", siblings.iter().map(|s| format!("\"{}\"", field_hex(s))).collect()),
            ("src_merkle_path_indices", right.iter().map(|b| b.to_string()).collect()),
        ],
    );
    // xburn sidecar for on-chain instruction assembly (P4.1): source nullifier,
    // dest_chain, and out_commitment for the destination mint.
    let xburn_sidecar = format!(
        "{{\"mint_hex\":\"{}\",\"amount\":{},\"src_commitment\":\"{}\",\"src_nullifier\":\"{}\",\
         \"src_merkle_root\":\"{}\",\"dest_chain\":\"{}\",\"out_commitment\":\"{}\"}}\n",
        hex::encode(mint_bytes),
        amount_u64,
        hex::encode(commitment),
        hex::encode(nullifier),
        hex::encode(merkle_root),
        hex::encode(xburn_dest_chain),
        hex::encode(out_commitment),
    );
    fs::write(circuits_dir.join("xburn_values.json"), xburn_sidecar).unwrap();

    // --- xburn2 (P4.3, m20 reverse leg): EVM note -> Solana note ---
    // The SAME xburn.nr circuit, a second witness for the round trip's return
    // leg. Its SOURCE note is the note the forward leg (above) just minted on
    // the destination pool: dest_owner_pubkey/dest_blinding there are exactly
    // this witness's src_spend_key/src_blinding (owner_pubkey = Poseidon(spend_key),
    // so `dest_owner_pubkey == hash_1(848_484_848)` both name the same note).
    // Its Merkle path is leaf 0 / all-left / zero-hash siblings — valid because
    // OpaqPool.sol seeds from the SAME zero-hash table (B.12.9's parity gate)
    // and the forward leg's mint is that pool's first-ever insert, so this is
    // its real authentication path, not a stand-in.
    let src_spend_key2 = be32(848_484_848); // == witness 1's dest_owner_pubkey's preimage
    let src_blinding2 = dest_blinding; // == witness 1's dest_blinding (same note)
    let src_owner2 = poseidon_be(&[src_spend_key2]);
    debug_assert_eq!(src_owner2, dest_owner_pubkey, "xburn2's source note must be xburn's destination note");
    let src_commitment2 = out_commitment; // the note the forward leg minted
    let src_merkle_root2 = merkle_root_be(src_commitment2, &siblings, &right);
    let src_nullifier2 = poseidon_be(&[src_commitment2, src_spend_key2]);
    let dest_chain2 = be32(101); // SOLANA_CHAIN_ID (programs/opaq/src/lib.rs) — separate workspace, can't import it
    let dest_owner_pubkey2 = poseidon_be(&[be32(909_090_909)]); // fresh, final Solana-side owner
    let dest_blinding2 = be32(373_737_373);
    let out_commitment2 = poseidon_be(&[token_id, amount, dest_owner_pubkey2, dest_blinding2]);

    let xburn2_json = format!(
        "{{\"src_merkle_root\":\"{}\",\"src_nullifier\":\"{}\",\"dest_chain\":\"{}\",\
         \"out_commitment\":\"{}\",\"token_id\":\"{}\",\"amount\":\"{}\",\
         \"src_spend_key\":\"{}\",\"src_blinding\":\"{}\",\
         \"src_merkle_path\":[{}],\"src_merkle_path_indices\":[{}],\
         \"dest_owner_pubkey\":\"{}\",\"dest_blinding\":\"{}\"}}",
        field_hex(&src_merkle_root2), field_hex(&src_nullifier2), field_hex(&dest_chain2), field_hex(&out_commitment2),
        field_hex(&token_id), field_hex(&amount), field_hex(&src_spend_key2), field_hex(&src_blinding2),
        siblings.iter().map(|s| format!("\"{}\"", field_hex(s))).collect::<Vec<_>>().join(","),
        right.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(","),
        field_hex(&dest_owner_pubkey2), field_hex(&dest_blinding2),
    );
    fs::write(circuits_dir.join("xburn2_inputs.json"), &xburn2_json).unwrap();
    println!("wrote {}", circuits_dir.join("xburn2_inputs.json").display());

    let xburn2_sidecar = format!(
        "{{\"src_commitment\":\"{}\",\"src_nullifier\":\"{}\",\"src_merkle_root\":\"{}\",\
         \"dest_chain\":\"{}\",\"out_commitment\":\"{}\"}}\n",
        hex::encode(src_commitment2),
        hex::encode(src_nullifier2),
        hex::encode(src_merkle_root2),
        hex::encode(dest_chain2),
        hex::encode(out_commitment2),
    );
    fs::write(circuits_dir.join("xburn2_values.json"), xburn2_sidecar).unwrap();

    // --- transfer/Prover.toml (Phase 2, P2.1): 2-in/2-out join-split ---
    // input[0]: the real note above (leaf 0, reusing the withdraw merkle setup).
    // input[1]: a dummy (amount 0). Split A into out[0]=B (to recipient) + out[1]=
    // A-B (change back to self), same token_id throughout. Conserves value.
    let a = amount_u64; // input[0] amount
    let b = a / 2; // output[0] to recipient
    let change = a - b; // output[1] back to self
    let in0_amt = be32(a as u128);

    // input[1]: dummy note (amount 0, skips Merkle membership in-circuit).
    let in1_sk = be32(111_222_333);
    let in1_bl = be32(444_555_666);
    let in1_owner = poseidon_be(&[in1_sk]);
    let in1_commit = poseidon_be(&[token_id, be32(0), in1_owner, in1_bl]);
    let in1_nf = poseidon_be(&[in1_commit, in1_sk]);

    // outputs: [0] to recipient (fresh owner), [1] change back to self (owner_pubkey).
    let out0_owner = poseidon_be(&[be32(777_000_777)]);
    let out0_bl = be32(555_111_555);
    let out0_amt = be32(b as u128);
    let out0_commit = poseidon_be(&[token_id, out0_amt, out0_owner, out0_bl]);
    let out1_owner = owner_pubkey; // change to self
    let out1_bl = be32(666_222_666);
    let out1_amt = be32(change as u128);
    let out1_commit = poseidon_be(&[token_id, out1_amt, out1_owner, out1_bl]);

    let fh = |x: &[u8; 32]| field_hex(x);
    let zeros24 = vec!["\"0x00\"".to_string(); TREE_DEPTH].join(", ");
    let false24 = vec!["false".to_string(); TREE_DEPTH].join(", ");
    let real_path = siblings.iter().map(|s| format!("\"{}\"", field_hex(s))).collect::<Vec<_>>().join(", ");
    let real_idx = right.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ");

    let transfer = format!(
        "merkle_root = \"{root}\"\n\
         nullifiers = [\"{nf0}\", \"{nf1}\"]\n\
         out_commitments = [\"{oc0}\", \"{oc1}\"]\n\
         token_id = \"{tid}\"\n\
         in_amount = [\"{ia0}\", \"{ia1}\"]\n\
         in_spend_key = [\"{isk0}\", \"{isk1}\"]\n\
         in_blinding = [\"{ib0}\", \"{ib1}\"]\n\
         in_is_dummy = [false, true]\n\
         in_merkle_path = [[{rp}], [{z}]]\n\
         in_merkle_path_indices = [[{ri}], [{f}]]\n\
         out_amount = [\"{oa0}\", \"{oa1}\"]\n\
         out_owner_pubkey = [\"{oo0}\", \"{oo1}\"]\n\
         out_blinding = [\"{ob0}\", \"{ob1}\"]\n",
        root = fh(&merkle_root), nf0 = fh(&nullifier), nf1 = fh(&in1_nf),
        oc0 = fh(&out0_commit), oc1 = fh(&out1_commit), tid = fh(&token_id),
        ia0 = fh(&in0_amt), ia1 = fh(&be32(0)),
        isk0 = fh(&spend_key), isk1 = fh(&in1_sk),
        ib0 = fh(&blinding), ib1 = fh(&in1_bl),
        rp = real_path, z = zeros24, ri = real_idx, f = false24,
        oa0 = fh(&out0_amt), oa1 = fh(&out1_amt),
        oo0 = fh(&out0_owner), oo1 = fh(&out1_owner),
        ob0 = fh(&out0_bl), ob1 = fh(&out1_bl),
    );
    write(&circuits_dir, "transfer", &transfer);

    // inputs.json (ABI for noir-cli interop): same values, JSON shape.
    let real_path_j = siblings.iter().map(|s| format!("\"{}\"", field_hex(s))).collect::<Vec<_>>().join(",");
    let zeros24_j = vec!["\"0x00\"".to_string(); TREE_DEPTH].join(",");
    let false24_j = vec!["false".to_string(); TREE_DEPTH].join(",");
    let real_idx_j = right.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
    let transfer_json = format!(
        "{{\"merkle_root\":\"{root}\",\"nullifiers\":[\"{nf0}\",\"{nf1}\"],\
         \"out_commitments\":[\"{oc0}\",\"{oc1}\"],\"token_id\":\"{tid}\",\
         \"in_amount\":[\"{ia0}\",\"{ia1}\"],\"in_spend_key\":[\"{isk0}\",\"{isk1}\"],\
         \"in_blinding\":[\"{ib0}\",\"{ib1}\"],\"in_is_dummy\":[false,true],\
         \"in_merkle_path\":[[{rp}],[{z}]],\"in_merkle_path_indices\":[[{ri}],[{f}]],\
         \"out_amount\":[\"{oa0}\",\"{oa1}\"],\"out_owner_pubkey\":[\"{oo0}\",\"{oo1}\"],\
         \"out_blinding\":[\"{ob0}\",\"{ob1}\"]}}",
        root = fh(&merkle_root), nf0 = fh(&nullifier), nf1 = fh(&in1_nf),
        oc0 = fh(&out0_commit), oc1 = fh(&out1_commit), tid = fh(&token_id),
        ia0 = fh(&in0_amt), ia1 = fh(&be32(0)),
        isk0 = fh(&spend_key), isk1 = fh(&in1_sk),
        ib0 = fh(&blinding), ib1 = fh(&in1_bl),
        rp = real_path_j, z = zeros24_j, ri = real_idx_j, f = false24_j,
        oa0 = fh(&out0_amt), oa1 = fh(&out1_amt),
        oo0 = fh(&out0_owner), oo1 = fh(&out1_owner),
        ob0 = fh(&out0_bl), ob1 = fh(&out1_bl),
    );
    fs::write(circuits_dir.join("transfer").join("inputs.json"), transfer_json).unwrap();
    println!("wrote {}", circuits_dir.join("transfer/inputs.json").display());

    // Sidecar for the M8 e2e harness: real instruction args + computed fields.
    let sidecar = format!(
        "{{\"mint_hex\":\"{}\",\"recipient_hex\":\"{}\",\"amount\":{},\
         \"commitment\":\"{}\",\"nullifier\":\"{}\",\"merkle_root\":\"{}\"}}\n",
        hex::encode(mint_bytes),
        hex::encode(recipient_bytes),
        amount_u64,
        hex::encode(commitment),
        hex::encode(nullifier),
        hex::encode(merkle_root),
    );
    fs::write(circuits_dir.join("e2e_values.json"), sidecar).unwrap();

    println!("commitment = {}", field_hex(&commitment));
    println!("nullifier  = {}", field_hex(&nullifier));
    println!("root       = {}", field_hex(&merkle_root));
}

fn hex32_env(var: &str) -> Option<[u8; 32]> {
    let s = std::env::var(var).ok()?;
    let bytes = hex::decode(s.trim_start_matches("0x")).ok()?;
    bytes.try_into().ok()
}

fn dummy_pubkey(seed: u8) -> [u8; 32] {
    let mut b = [0u8; 32];
    for (i, x) in b.iter_mut().enumerate() {
        *x = seed.wrapping_add(i as u8);
    }
    b
}

fn write(circuits_dir: &std::path::Path, circuit: &str, contents: &str) {
    let path = circuits_dir.join(circuit).join("Prover.toml");
    fs::write(&path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    println!("wrote {}", path.display());
}

/// ABI-shaped `inputs.json` for the Noir->Groth16 pipeline (noir-cli interop):
/// a flat map of input name -> "0x.." string, with array inputs as JSON arrays.
fn write_json(
    circuits_dir: &std::path::Path,
    circuit: &str,
    scalars: &[(&str, String)],
    arrays: &[(&str, Vec<String>)],
) {
    let mut entries: Vec<String> = scalars
        .iter()
        .map(|(k, v)| format!("\"{k}\":\"{v}\""))
        .collect();
    for (k, vs) in arrays {
        // elements are already JSON-formatted by the caller (quoted or raw)
        let arr = vs.join(",");
        entries.push(format!("\"{k}\":[{arr}]"));
    }
    let json = format!("{{{}}}", entries.join(","));
    let path = circuits_dir.join(circuit).join("inputs.json");
    fs::write(&path, json).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    println!("wrote {}", path.display());
}

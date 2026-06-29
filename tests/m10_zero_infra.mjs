// M10 / Test 7 (OPAQ.md B.8): zero-infra read path.
//
// Lands real deposits on a validator, then — from a FRESH connection that has
// only the public RPC URL (no indexer, no cache, no shared state) — reconstructs
// the Merkle authentication path for an existing note purely from on-chain
// account data + transaction logs, and proves it lands on a known on-chain root.
//
// The reconstruction itself runs through the real `opaq withdraw --leaves` CLI,
// so this exercises the exact client path a withdrawer would use.
//
// Usage: node m10_zero_infra.mjs <programKp> <mintKp> <recipientKp> \
//          <depositA.bin> <depositB.bin> <noteA.json> <opaqBin> [rpc]
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync, spawnSync } from "node:child_process";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo, getAccount,
} from "@solana/spl-token";
import { fetchTree, fetchLeaves } from "./read_path.mjs";

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, mintPath, recipPath, depositABin, depositBBin, noteAPath, opaqBin] =
  process.argv.slice(2);
const rpc = process.argv[9] || "http://127.0.0.1:8899";

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const recipient = kp(recipPath);
const AMOUNT = 1000n;
const conn = new Connection(rpc, "confirmed");
const payer = Keypair.generate();
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];

async function send(ix, signers = [payer]) {
  const tx = new Transaction()
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 }))
    .add(ix);
  return sendAndConfirmTransaction(conn, tx, signers, { commitment: "confirmed" });
}
const bal = async (ata) => (await getAccount(conn, ata)).amount;
const assert = (cond, msg) => { if (!cond) throw new Error(msg); };

async function main() {
  await conn.confirmTransaction(
    await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "confirmed");

  // --- SPL + pool setup, then land two real deposits (notes A, B) ---
  const mint = await createMint(conn, payer, payer.publicKey, null, 0, mintKp);
  const depositorAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, payer.publicKey)).address;
  await mintTo(conn, payer, mint, depositorAta, payer, 2n * AMOUNT);
  const vaultAuthority = pda([Buffer.from("vault"), mint.toBuffer()]);
  const vaultAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true)).address;

  const tree = pda([Buffer.from("tree")]);
  const nullifiers = pda([Buffer.from("nullifiers")]);
  console.log(`program ${programId.toBase58()}\ntree    ${tree.toBase58()}`);

  await send(new TransactionInstruction({
    programId, data: Buffer.from([0]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  const depositIx = (data) => new TransactionInstruction({
    programId, data,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: depositorAta, isSigner: false, isWritable: true },
      { pubkey: vaultAta, isSigner: false, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    ],
  });
  await send(depositIx(fs.readFileSync(depositABin))); // note A -> leaf_index 0
  await send(depositIx(fs.readFileSync(depositBBin))); // note B -> leaf_index 1
  assert(await bal(vaultAta) === 2n * AMOUNT, "vault != 2*amount after two deposits");
  console.log(`  OK  landed deposits A + B (vault ${2n * AMOUNT})`);

  // --- the clean machine: a fresh connection, only the RPC URL ---
  const clean = new Connection(rpc, "confirmed");
  const treeState = await fetchTree(clean, tree);
  const leaves = await fetchLeaves(clean, programId, tree);
  console.log(`  OK  read path (RPC only): ${leaves.length} leaves, next_index=${treeState.nextIndex}, ` +
    `${treeState.knownRoots.size} known roots`);
  assert(leaves.length === 2, `expected 2 leaves from chain, got ${leaves.length}`);
  assert(treeState.nextIndex === 2n, `tree next_index should be 2, got ${treeState.nextIndex}`);

  // Hand the harvested commitment list to the real withdraw CLI, which finds
  // note A's commitment, rebuilds its path, and emits a withdraw witness.
  const leavesFile = path.join(os.tmpdir(), `opaq-leaves-${process.pid}.json`);
  const witnessFile = path.join(os.tmpdir(), `opaq-wd-${process.pid}.json`);
  fs.writeFileSync(leavesFile, JSON.stringify(leaves));

  const out = execFileSync(opaqBin, [
    "withdraw",
    "--note", noteAPath,
    "--recipient", recipient.publicKey.toBase58(),
    "--leaves", leavesFile,
    "--inputs-out", witnessFile,
  ], { encoding: "utf8", env: process.env });

  const leafIndex = Number(out.match(/leaf_index (\d+)/)?.[1]);
  const root = out.match(/merkle_root 0x([0-9a-f]{64})/)?.[1];
  assert(leafIndex === 0, `note A should be leaf_index 0, got ${leafIndex}`);
  assert(root, "CLI did not print a reconstructed merkle_root");
  console.log(`  OK  reconstructed path for note A (leaf_index ${leafIndex})`);

  // The zero-infra acceptance: the reconstructed root is one the on-chain ring
  // buffer recognizes — so a proof built on this path would verify on-chain.
  assert(treeState.knownRoots.has(root),
    `reconstructed root ${root} is NOT in the on-chain root ring buffer`);
  assert(root === treeState.currentRoot,
    "full-tree reconstruction should match the current on-chain root");
  console.log("  OK  reconstructed root matches a known on-chain root (current root)");

  // M9: the same withdraw, but harvesting leaves LIVE over RPC (--rpc/--program)
  // instead of a pre-harvested --leaves file. Must reconstruct the identical root
  // — proving the CLI works against any live pool with no external harness.
  const rpcRun = spawnSync(opaqBin, [
    "withdraw",
    "--note", noteAPath,
    "--recipient", recipient.publicKey.toBase58(),
    "--rpc", rpc,
    "--program", programId.toBase58(),
  ], { encoding: "utf8", env: {
    ...process.env,
    OPAQ_READ_SCRIPT: `${process.env.OPAQ_ROOT}/tests/read_leaves.mjs`,
    OPAQ_RECIPIENT_SCRIPT: `${process.env.OPAQ_ROOT}/tests/recipient_history.mjs`,
  } });
  assert(rpcRun.status === 0, `--rpc withdraw failed: ${rpcRun.stderr}`);
  const rootRpc = rpcRun.stdout.match(/merkle_root 0x([0-9a-f]{64})/)?.[1];
  assert(rootRpc === root, `--rpc root ${rootRpc} != --leaves root ${root}`);
  console.log("  OK  --rpc harvest reconstructs the identical root (no leaves file)");

  // M9(c): with --rpc, the A.8 recipient warning is a concrete RPC finding. The
  // test recipient is brand-new, so it must report FRESH (no prior signatures).
  assert(/RPC check: no prior signatures seen .* looks FRESH/.test(rpcRun.stderr),
    `expected a FRESH A.8 RPC finding, got stderr:\n${rpcRun.stderr}`);
  console.log("  OK  A.8 recipient history auto-checked over RPC (fresh address)");

  // The emitted witness is a complete, well-formed withdraw input.
  const w = JSON.parse(fs.readFileSync(witnessFile, "utf8"));
  assert(w.merkle_path.length === 24 && w.merkle_path_indices.length === 24,
    "withdraw witness must carry a depth-24 path");
  assert(w.merkle_root === `0x${root}`, "witness root must match the printed root");
  console.log("  OK  emitted a complete depth-24 withdraw witness");

  // --- close the loop: M9(a) prove-only — the CLI reconstructs (via --rpc),
  // proves the withdraw, and emits the ready-to-submit blob ITSELF (no manual
  // prove+emit). Submitting that blob on-chain below is the verification. ---
  const { OPAQ_ROOT: R, OPAQ_WITHDRAW_ZKEY: WZKEY } = process.env;
  const wdBin = path.join(os.tmpdir(), `opaq-wd-${process.pid}.bin`);
  const proveRun = spawnSync(opaqBin, [
    "withdraw",
    "--note", noteAPath,
    "--recipient", recipient.publicKey.toBase58(),
    "--rpc", rpc,
    "--program", programId.toBase58(),
    "--prove",
    "--zkey", WZKEY,
    "--out", wdBin,
  ], { encoding: "utf8", env: {
    ...process.env,
    OPAQ_READ_SCRIPT: `${R}/tests/read_leaves.mjs`,
    OPAQ_RECIPIENT_SCRIPT: `${R}/tests/recipient_history.mjs`,
  } });
  assert(proveRun.status === 0, `--prove withdraw failed: ${proveRun.stderr}`);
  assert(fs.existsSync(wdBin), "--prove did not emit an instruction blob");
  console.log("  OK  CLI proved the withdraw + emitted the instruction blob (M9a)");

  const recipientAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, recipient.publicKey)).address;
  await send(new TransactionInstruction({
    programId, data: fs.readFileSync(wdBin),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: vaultAuthority, isSigner: false, isWritable: false },
      { pubkey: vaultAta, isSigner: false, isWritable: true },
      { pubkey: recipientAta, isSigner: false, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: false },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  assert(await bal(recipientAta) === AMOUNT, "recipient did not receive funds via reconstructed-path withdraw");
  assert(await bal(vaultAta) === AMOUNT, "vault should retain note B's funds");
  console.log(`  OK  withdraw via RECONSTRUCTED path: recipient got ${AMOUNT}, note B untouched`);

  [leavesFile, witnessFile, wdBin].forEach((p) => fs.rmSync(p, { force: true }));
  console.log("\nM10 PASSED — zero-infra read path: Merkle path reconstructed from a clean" +
    " RPC-only client, then USED to withdraw on-chain (funds moved).");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

// Test 5 — Root ring buffer overflow (OPAQ.md B.8).
//
// The CommitmentTree keeps only the last ROOT_HISTORY(=32) roots. A withdrawal
// whose proof targets a root that has since been evicted must fail with a CLEAR
// "unknown/stale root" error (E_UNKNOWN_ROOT = 0x4) — not a confusing generic
// failure — because this is a real thing that happens to slow withdrawers.
//
// Plan: deposit note A at leaf 0 (its withdraw proof targets root_after_1), then
// deposit 32 filler notes so 33 total deposits wrap the ring buffer and overwrite
// root_after_1. The (still perfectly valid) withdraw proof for A must then be
// rejected for an aged-out root. We also confirm the eviction over plain RPC.
//
// Usage: node test5_ringbuffer.mjs <programKp> <mintKp> <recipientKp> \
//          <deposit_a.bin> <withdraw_a.bin> [rpc]
import fs from "node:fs";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo, getAccount,
} from "@solana/spl-token";
import { fetchTree } from "./read_path.mjs";

const ROOT_HISTORY = 32;
const FILLERS = ROOT_HISTORY; // 1 (note A) + 32 fillers = 33 deposits => wrap

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, mintPath, recipPath, depositABin, withdrawABin] = process.argv.slice(2);
const rpc = process.argv[7] || "http://127.0.0.1:8899";

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const recipient = kp(recipPath);
const AMOUNT = 1000n;
const conn = new Connection(rpc, "confirmed");
const payer = Keypair.generate();
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
const hex = (buf) => Buffer.from(buf).toString("hex");
const assert = (cond, msg) => { if (!cond) throw new Error(msg); };

async function send(ix, signers = [payer]) {
  const tx = new Transaction()
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 }))
    .add(ix);
  return sendAndConfirmTransaction(conn, tx, signers, { commitment: "confirmed" });
}
const bal = async (ata) => (await getAccount(conn, ata)).amount;

// Run `ix`, expect it to fail, and return the surfaced error text + logs so the
// caller can assert the failure is the SPECIFIC, legible one.
async function captureFailure(ix) {
  try {
    await send(ix);
  } catch (e) {
    let logs = e?.logs ?? [];
    if (!logs.length && typeof e?.getLogs === "function") {
      try { logs = (await e.getLogs(conn)) ?? []; } catch { /* best effort */ }
    }
    return { message: String(e?.message ?? e), logs };
  }
  return null; // did not fail
}

async function main() {
  await conn.confirmTransaction(
    await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "confirmed");

  const mint = await createMint(conn, payer, payer.publicKey, null, 0, mintKp);
  const depositorAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, payer.publicKey)).address;
  await mintTo(conn, payer, mint, depositorAta, payer, BigInt(FILLERS + 1) * AMOUNT);
  const vaultAuthority = pda([Buffer.from("vault"), mint.toBuffer()]);
  const vaultAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true)).address;
  const recipientAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, recipient.publicKey)).address;

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

  const depositData = fs.readFileSync(depositABin);
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

  // The root note A's withdraw proof targets: withdraw arg layout (after the
  // 1-byte tag) is proof(256) ‖ merkle_root(32) ‖ ... so root is bytes [257,289).
  const withdrawData = fs.readFileSync(withdrawABin);
  const rootAfterA = hex(withdrawData.subarray(257, 289));

  // Deposit A (leaf 0). root_after_1 is now the current, known root.
  await send(depositIx(depositData));
  let t = await fetchTree(conn, tree);
  assert(t.nextIndex === 1n, `expected next_index 1 after note A, got ${t.nextIndex}`);
  assert(t.knownRoots.has(rootAfterA), "root_after_A should be a known root right after the deposit");
  assert(t.currentRoot === rootAfterA, "root_after_A should be the current root after one deposit");
  console.log(`  OK  note A deposited; its proof's root is currently known`);

  // 32 filler deposits (reuse A's proof — duplicate commitments are fine for the
  // tree and cheap; each still advances the root) => 33 total, wrapping the ring.
  for (let i = 0; i < FILLERS; i++) await send(depositIx(depositData));
  assert(await bal(vaultAta) === BigInt(FILLERS + 1) * AMOUNT, "vault balance wrong after 33 deposits");
  t = await fetchTree(conn, tree);
  assert(t.nextIndex === BigInt(FILLERS + 1), `expected next_index ${FILLERS + 1}, got ${t.nextIndex}`);
  assert(!t.knownRoots.has(rootAfterA),
    "root_after_A must be EVICTED from the ring buffer after 33 deposits");
  console.log(`  OK  ${FILLERS + 1} deposits wrapped the ring buffer; root_after_A is now evicted (RPC-verified)`);

  // Withdraw A with its (still cryptographically valid) proof. The program
  // verifies the proof + recipient binding FIRST, so reaching E_UNKNOWN_ROOT
  // proves the proof itself is fine — only the root aged out.
  const withdrawIx = new TransactionInstruction({
    programId, data: withdrawData,
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
  });
  const fail = await captureFailure(withdrawIx);
  assert(fail, "withdraw against an evicted root was ACCEPTED — ring-buffer guard is broken");

  // E_UNKNOWN_ROOT = 4. Must be THIS error, not a generic / proof-invalid (0x1) one.
  const text = [fail.message, ...fail.logs].join("\n");
  assert(/custom program error: 0x4\b/.test(text),
    `expected a clear stale-root error (0x4), got:\n${text}`);
  assert(!/custom program error: 0x1\b/.test(text),
    "got proof-invalid (0x1) — the proof should verify; only the root is stale");
  // Funds must be untouched by the rejected withdrawal.
  assert(await bal(recipientAta) === 0n, "recipient received funds on a rejected withdraw");
  assert(await bal(vaultAta) === BigInt(FILLERS + 1) * AMOUNT, "vault changed on a rejected withdraw");
  console.log(`  OK  evicted-root withdraw rejected with a clear E_UNKNOWN_ROOT (0x4); funds untouched`);

  console.log("\nTest 5 PASSED — ring-buffer overflow: a withdrawal against an evicted root" +
    " fails legibly with E_UNKNOWN_ROOT, not a generic error.");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

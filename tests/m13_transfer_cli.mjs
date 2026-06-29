// M13 / Phase 2 P2.4 (OPAQ.md B.4.3): drive the full private loop from the CLI —
// `opaq deposit` -> `opaq transfer` -> `opaq withdraw` (of the change note). This
// sets up the pool/mint/vault, then spawns the real `opaq` binary for each step
// and asserts the on-chain effects (tree growth, vault balance), proving the
// transfer CLI works against a live pool.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo, getAccount,
} from "@solana/spl-token";
import { fetchTree } from "./read_path.mjs";

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, mintPath, opaqBin] = process.argv.slice(2);
const rpc = process.argv[5] || "http://127.0.0.1:8899";
const { OPAQ_ROOT: R, OPAQ_DEPOSIT_ZKEY, OPAQ_TRANSFER_ZKEY, OPAQ_WITHDRAW_ZKEY } = process.env;

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const AMOUNT = 1000n;
const SEND = 600n; // to recipient; CHANGE = 400 back to self
const conn = new Connection(rpc, "confirmed");
const payer = Keypair.generate();
const recipient = Keypair.generate();
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
const assert = (c, m) => { if (!c) throw new Error(m); };
const bal = async (ata) => (await getAccount(conn, ata)).amount;
const send = (ix) => sendAndConfirmTransaction(conn, new Transaction()
  .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 })).add(ix), [payer], { commitment: "confirmed" });

function cli(args, label) {
  const r = spawnSync(opaqBin, args, { encoding: "utf8", env: {
    ...process.env,
    OPAQ_READ_SCRIPT: `${R}/tests/read_leaves.mjs`,
    OPAQ_RECIPIENT_SCRIPT: `${R}/tests/recipient_history.mjs`,
    OPAQ_SUBMIT_SCRIPT: `${R}/tests/submit_withdraw.mjs`,
    OPAQ_SUBMIT_DEPOSIT_SCRIPT: `${R}/tests/submit_deposit.mjs`,
    OPAQ_SUBMIT_TRANSFER_SCRIPT: `${R}/tests/submit_transfer.mjs`,
  } });
  assert(r.status === 0, `${label} failed:\n${r.stdout}\n${r.stderr}`);
  return r;
}

async function main() {
  await conn.confirmTransaction(await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "confirmed");
  const mint = await createMint(conn, payer, payer.publicKey, null, 0, mintKp);
  const mintB58 = mint.toBase58();
  const depositorAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, payer.publicKey)).address;
  await mintTo(conn, payer, mint, depositorAta, payer, AMOUNT);
  const vaultAuthority = pda([Buffer.from("vault"), mint.toBuffer()]);
  const vaultAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true)).address;
  const tree = pda([Buffer.from("tree")]);
  const nullifiers = pda([Buffer.from("nullifiers")]);

  // init pool
  await send(new TransactionInstruction({
    programId, data: Buffer.from([0]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));

  const payerFile = path.join(os.tmpdir(), `opaq-m13-payer-${process.pid}.json`);
  fs.writeFileSync(payerFile, JSON.stringify(Array.from(payer.secretKey)));
  const depNote = path.join(os.tmpdir(), `opaq-m13-dep-${process.pid}.json`);
  const changeNote = path.join(os.tmpdir(), `opaq-m13-change-${process.pid}.json`);
  const recipientAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, recipient.publicKey)).address;
  const common = ["--rpc", rpc, "--program", programId.toBase58(), "--submit", "--payer", payerFile];

  // 1) deposit a note via the CLI
  cli(["deposit", "--token", mintB58, "--amount", String(AMOUNT), "--note", depNote,
    "--zkey", OPAQ_DEPOSIT_ZKEY, ...common,
    "--out", path.join(os.tmpdir(), `opaq-m13-depbin-${process.pid}.bin`)], "opaq deposit");
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "deposit should land leaf 0");
  assert(await bal(vaultAta) === AMOUNT, "vault != amount after deposit");
  console.log("  OK  opaq deposit --submit (leaf 0, vault funded)");

  // 2) transfer: send SEND to a recipient owner, keep CHANGE in a fresh self note
  const recipientOwner = "0x0000000000000000000000000000000000000000000000000000000000000001";
  cli(["transfer", "--note", depNote, "--to", recipientOwner, "--amount", String(SEND),
    "--zkey", OPAQ_TRANSFER_ZKEY, "--change-note", changeNote,
    "--out", path.join(os.tmpdir(), `opaq-m13-xferbin-${process.pid}.bin`), ...common], "opaq transfer");
  assert((await fetchTree(conn, tree)).nextIndex === 3n, "transfer should insert 2 output commitments");
  assert(await bal(vaultAta) === AMOUNT, "transfer must NOT move vault tokens");
  console.log("  OK  opaq transfer --submit (2 outputs inserted, vault untouched)");

  // 3) withdraw the CHANGE note (proves the self output is spendable)
  cli(["withdraw", "--note", changeNote, "--recipient", recipient.publicKey.toBase58(),
    "--zkey", OPAQ_WITHDRAW_ZKEY, ...common,
    "--out", path.join(os.tmpdir(), `opaq-m13-wdbin-${process.pid}.bin`)], "opaq withdraw (change)");
  const CHANGE = AMOUNT - SEND;
  assert(await bal(recipientAta) === CHANGE, `recipient should receive the change ${CHANGE}`);
  assert(await bal(vaultAta) === AMOUNT - CHANGE, "vault should drop by the withdrawn change");
  console.log("  OK  opaq withdraw of the change note (funds released)");

  [payerFile, depNote, changeNote].forEach((p) => fs.rmSync(p, { force: true }));
  console.log("\nM13 PASSED — Phase 2 driven end-to-end from the CLI: deposit -> transfer" +
    " (hidden amount + token) -> withdraw the change, all on-chain.");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

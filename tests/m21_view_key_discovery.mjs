// M21 / Phase 2.5 P2.5.2 (OPAQ.md B.13): the closing accept criterion for
// viewing keys — Alice sends a transfer to Bob's published meta-address; Bob,
// holding ONLY (spend_key, view_key) in his identity file and ZERO
// out-of-band note info (no change-note/out-note handoff), runs `opaq
// list-unspent` and recovers exactly the note Alice sent, then withdraws it.
// This is the thing B.11 #4 said was missing: a transfer output found by the
// recipient with no side channel other than the memo riding on-chain.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  createMint, getOrCreateAssociatedTokenAccount, mintTo, getAccount,
} from "@solana/spl-token";
import { fetchTree } from "./read_path.mjs";

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, mintPath, opaqBin] = process.argv.slice(2);
const rpc = process.argv[5] || "http://127.0.0.1:8899";
const { OPAQ_ROOT: R, OPAQ_DEPOSIT_ZKEY, OPAQ_TRANSFER_ZKEY, OPAQ_WITHDRAW_ZKEY } = process.env;

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const AMOUNT = 1000n;
const SEND = 700n; // to Bob; CHANGE = 300 back to Alice (untouched by this test)
const conn = new Connection(rpc, "confirmed");
const payer = Keypair.generate();
const bobRecipient = Keypair.generate(); // where Bob withdraws TO
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
const assert = (c, m) => { if (!c) throw new Error(m); };
const bal = async (ata) => (await getAccount(conn, ata)).amount;
const send = (ix) => sendAndConfirmTransaction(conn, new Transaction()
  .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 })).add(ix), [payer], { commitment: "confirmed" });

function cli(args, label, extraEnv = {}) {
  const r = spawnSync(opaqBin, args, { encoding: "utf8", env: {
    ...process.env,
    OPAQ_READ_SCRIPT: `${R}/tests/read_leaves.mjs`,
    OPAQ_RECIPIENT_SCRIPT: `${R}/tests/recipient_history.mjs`,
    OPAQ_SUBMIT_SCRIPT: `${R}/tests/submit_withdraw.mjs`,
    OPAQ_SUBMIT_DEPOSIT_SCRIPT: `${R}/tests/submit_deposit.mjs`,
    OPAQ_SUBMIT_TRANSFER_SCRIPT: `${R}/tests/submit_transfer.mjs`,
    OPAQ_LIST_UNSPENT_SCRIPT: `${R}/tests/list_unspent.mjs`,
    ...extraEnv,
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

  const tmp = os.tmpdir(), pid = process.pid;
  const payerFile = path.join(tmp, `opaq-m21-payer-${pid}.json`);
  fs.writeFileSync(payerFile, JSON.stringify(Array.from(payer.secretKey)));
  const depNote = path.join(tmp, `opaq-m21-dep-${pid}.json`);
  const bobIdentity = path.join(tmp, `opaq-m21-bob-identity-${pid}.json`);
  const bobNotesDir = path.join(tmp, `opaq-m21-bob-notes-${pid}`);
  fs.mkdirSync(bobNotesDir, { recursive: true });
  const bobRecipientAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, bobRecipient.publicKey)).address;
  const common = ["--rpc", rpc, "--program", programId.toBase58(), "--submit", "--payer", payerFile];

  // 1) Alice deposits
  cli(["deposit", "--token", mintB58, "--amount", String(AMOUNT), "--note", depNote,
    "--zkey", OPAQ_DEPOSIT_ZKEY, ...common,
    "--out", path.join(tmp, `opaq-m21-depbin-${pid}.bin`)],
    "opaq deposit", { OPAQ_PASSPHRASE: "alice-pass" });
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "deposit should land leaf 0");
  console.log("  OK  Alice deposits (leaf 0)");

  // 2) Bob generates a fresh receiving identity + publishes his meta-address
  const addrOut = cli(["address", "--out", bobIdentity], "opaq address", { OPAQ_PASSPHRASE: "bob-pass" });
  const ownerMatch = addrOut.stdout.match(/owner_pubkey\s+(0x[0-9a-f]+)/);
  const viewMatch = addrOut.stdout.match(/viewing_pubkey\s+(0x[0-9a-f]+)/);
  assert(ownerMatch && viewMatch, "opaq address must print a meta-address");
  const [bobOwner, bobViewing] = [ownerMatch[1], viewMatch[1]];
  console.log(`  OK  Bob publishes meta-address (owner ${bobOwner.slice(0, 10)}…, viewing ${bobViewing.slice(0, 10)}…)`);

  // 3) Alice transfers SEND to Bob's meta-address, attaching the B.13 memo.
  // Alice never hands Bob anything else — no --out-note, no side channel.
  cli(["transfer", "--note", depNote, "--to", bobOwner, "--to-view", bobViewing,
    "--amount", String(SEND), "--zkey", OPAQ_TRANSFER_ZKEY,
    "--out", path.join(tmp, `opaq-m21-xferbin-${pid}.bin`), ...common],
    "opaq transfer --to-view", { OPAQ_PASSPHRASE: "alice-pass" });
  assert((await fetchTree(conn, tree)).nextIndex === 3n, "transfer should insert 2 output commitments");
  console.log("  OK  Alice transfers to Bob with an attached encrypted memo (no out-of-band handoff)");

  // 4) Bob, with ONLY his identity file, discovers the note himself.
  const scanOut = cli(["list-unspent", "--identity", bobIdentity, "--rpc", rpc,
    "--program", programId.toBase58(), "--out-dir", bobNotesDir],
    "opaq list-unspent", { OPAQ_PASSPHRASE: "bob-pass" });
  assert(/done: 1 unspent note/.test(scanOut.stdout), `expected exactly 1 discovered note:\n${scanOut.stdout}`);
  const discovered = fs.readdirSync(bobNotesDir).filter((f) => f.startsWith("note-"));
  assert(discovered.length === 1, `expected exactly 1 note file, got ${discovered.length}`);
  console.log(`  OK  Bob discovers his note via list-unspent (zero out-of-band info): ${discovered[0]}`);

  // 5) Bob withdraws the discovered note — proves it's genuinely spendable.
  const discoveredPath = path.join(bobNotesDir, discovered[0]);
  cli(["withdraw", "--note", discoveredPath, "--recipient", bobRecipient.publicKey.toBase58(),
    "--zkey", OPAQ_WITHDRAW_ZKEY, ...common,
    "--out", path.join(tmp, `opaq-m21-wdbin-${pid}.bin`)],
    "opaq withdraw (discovered note)", { OPAQ_PASSPHRASE: "bob-pass" });
  assert(await bal(bobRecipientAta) === SEND, `Bob's recipient should receive ${SEND}`);
  assert(await bal(vaultAta) === AMOUNT - SEND, "vault should drop by exactly what Bob withdrew");
  console.log("  OK  Bob withdraws the discovered note (funds released, vault matches)");

  [payerFile, depNote, bobIdentity].forEach((p) => fs.rmSync(p, { force: true }));
  fs.rmSync(bobNotesDir, { recursive: true, force: true });
  console.log("\nM21 PASSED — B.13 viewing-key discovery closed end-to-end: Alice sends to Bob's" +
    " meta-address, Bob discovers + withdraws with zero out-of-band handoff.");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

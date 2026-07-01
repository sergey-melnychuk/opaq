// M16 / Phase 3 (OPAQ.md B.11 #3): drive the cross-chain burn from the CLI —
// `opaq deposit` -> `opaq burn --submit`. Sets up the pool/mint/vault, deposits a
// note, then burns it via the CLI (self-served, no relayer) and asserts the
// on-chain effects: nullifier recorded, NO SPL released, NO tree insert, and a
// double-burn (nullifier reuse) is rejected on-chain.
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
const { OPAQ_ROOT: R, OPAQ_DEPOSIT_ZKEY, OPAQ_BURN_ZKEY } = process.env;

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const AMOUNT = 1000n;
const DEST_CHAIN = "1"; // Ethereum mainnet chain id
const DEST_ADDR = "0x1111111111111111111111111111111111111111";
const conn = new Connection(rpc, "confirmed");
const payer = Keypair.generate();
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
const assert = (c, m) => { if (!c) throw new Error(m); };
const bal = async (ata) => (await getAccount(conn, ata)).amount;
const send = (ix) => sendAndConfirmTransaction(conn, new Transaction()
  .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 })).add(ix), [payer], { commitment: "confirmed" });

const cliEnv = () => ({
  ...process.env,
  OPAQ_READ_SCRIPT: `${R}/tests/read_leaves.mjs`,
  OPAQ_SUBMIT_DEPOSIT_SCRIPT: `${R}/tests/submit_deposit.mjs`,
  OPAQ_SUBMIT_BURN_SCRIPT: `${R}/tests/submit_burn.mjs`,
});
const burnArgs = (out, payerFile, note) => [
  "burn", "--note", note, "--dest-chain", DEST_CHAIN, "--dest-address", DEST_ADDR,
  "--zkey", OPAQ_BURN_ZKEY, "--rpc", rpc, "--program", programId.toBase58(),
  "--submit", "--payer", payerFile, "--out", out,
];
function cli(args, label) {
  const r = spawnSync(opaqBin, args, { encoding: "utf8", env: cliEnv() });
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

  const payerFile = path.join(os.tmpdir(), `opaq-m16-payer-${process.pid}.json`);
  fs.writeFileSync(payerFile, JSON.stringify(Array.from(payer.secretKey)));
  const depNote = path.join(os.tmpdir(), `opaq-m16-dep-${process.pid}.json`);
  const common = ["--rpc", rpc, "--program", programId.toBase58(), "--submit", "--payer", payerFile];

  // 1) deposit a note via the CLI
  cli(["deposit", "--token", mintB58, "--amount", String(AMOUNT), "--note", depNote,
    "--zkey", OPAQ_DEPOSIT_ZKEY, ...common,
    "--out", path.join(os.tmpdir(), `opaq-m16-depbin-${process.pid}.bin`)], "opaq deposit");
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "deposit should land leaf 0");
  assert(await bal(vaultAta) === AMOUNT, "vault != amount after deposit");
  console.log("  OK  opaq deposit --submit (leaf 0, vault funded)");

  // 2) burn the note cross-chain via the CLI (self-served: no relayer)
  const burnBin = path.join(os.tmpdir(), `opaq-m16-burnbin-${process.pid}.bin`);
  cli(burnArgs(burnBin, payerFile, depNote), "opaq burn");
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "burn must NOT insert a commitment");
  assert(await bal(vaultAta) === AMOUNT, "burn must NOT release SPL (value locked on Solana)");
  console.log("  OK  opaq burn --submit (nullifier recorded, tree + vault unchanged)");

  // 3) double-burn: the same note again must be rejected (nullifier reuse guard)
  const burnBin2 = path.join(os.tmpdir(), `opaq-m16-burnbin2-${process.pid}.bin`);
  const r2 = spawnSync(opaqBin, burnArgs(burnBin2, payerFile, depNote), { encoding: "utf8", env: cliEnv() });
  assert(r2.status !== 0, `re-burn (nullifier reuse) should be rejected on-chain:\n${r2.stdout}`);
  console.log("  OK  double-burn rejected (nullifier reuse guard)");

  [payerFile, depNote, burnBin, burnBin2].forEach((p) => fs.rmSync(p, { force: true }));
  console.log("\nM16 PASSED — Phase 3 cross-chain burn driven end-to-end from the CLI:" +
    " deposit -> opaq burn --submit (self-served, no relayer), double-burn rejected.");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

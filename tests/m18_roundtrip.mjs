// M18 / Phase 3 (OPAQ.md B.11 #3): FORWARD round-trip, Solana side. Deposit a note,
// then `opaq burn --submit --prove-dir <dir>` — records the nullifier on Solana AND
// leaves the snarkjs proof.json + public.json for the EVM mint. Asserts Solana state
// (burn inserts nothing, releases nothing); the m18 driver then feeds the SAME proof
// to evm/mint.mjs on anvil.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import { createMint, getOrCreateAssociatedTokenAccount, mintTo, getAccount } from "@solana/spl-token";
import { fetchTree } from "./read_path.mjs";

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, mintPath, opaqBin, proofDir, destChain, destAddr] = process.argv.slice(2);
const rpc = process.argv[8] || "http://127.0.0.1:8899";
const { OPAQ_ROOT: R, OPAQ_DEPOSIT_ZKEY, OPAQ_BURN_ZKEY } = process.env;

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const AMOUNT = 1000n;
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
function cli(args, label) {
  const r = spawnSync(opaqBin, args, { encoding: "utf8", env: cliEnv() });
  assert(r.status === 0, `${label} failed:\n${r.stdout}\n${r.stderr}`);
  return r;
}

async function main() {
  await conn.confirmTransaction(await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "confirmed");
  const mint = await createMint(conn, payer, payer.publicKey, null, 0, mintKp);
  const depositorAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, payer.publicKey)).address;
  await mintTo(conn, payer, mint, depositorAta, payer, AMOUNT);
  const vaultAuthority = pda([Buffer.from("vault"), mint.toBuffer()]);
  const vaultAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true)).address;
  const tree = pda([Buffer.from("tree")]);
  const nullifiers = pda([Buffer.from("nullifiers")]);

  await send(new TransactionInstruction({
    programId, data: Buffer.from([0]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));

  const payerFile = path.join(os.tmpdir(), `opaq-m18-payer-${process.pid}.json`);
  fs.writeFileSync(payerFile, JSON.stringify(Array.from(payer.secretKey)));
  const depNote = path.join(os.tmpdir(), `opaq-m18-dep-${process.pid}.json`);
  const common = ["--rpc", rpc, "--program", programId.toBase58(), "--submit", "--payer", payerFile];

  cli(["deposit", "--token", mint.toBase58(), "--amount", String(AMOUNT), "--note", depNote,
    "--zkey", OPAQ_DEPOSIT_ZKEY, ...common,
    "--out", path.join(os.tmpdir(), `opaq-m18-depbin-${process.pid}.bin`)], "opaq deposit");
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "deposit should land leaf 0");
  assert(await bal(vaultAta) === AMOUNT, "vault != amount after deposit");
  console.log("  OK  opaq deposit --submit (leaf 0, vault funded)");

  // burn cross-chain: --prove-dir keeps proof.json + public.json for the EVM mint.
  cli(["burn", "--note", depNote, "--dest-chain", destChain, "--dest-address", destAddr,
    "--zkey", OPAQ_BURN_ZKEY, "--prove-dir", proofDir, ...common,
    "--out", path.join(os.tmpdir(), `opaq-m18-burnbin-${process.pid}.bin`)], "opaq burn");
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "burn must NOT insert a commitment");
  assert(await bal(vaultAta) === AMOUNT, "burn must NOT release SPL (value locked for the EVM mint)");
  assert(fs.existsSync(path.join(proofDir, "public.json")) && fs.existsSync(path.join(proofDir, "proof.json")),
    "burn --prove-dir should leave proof.json + public.json");
  console.log("  OK  opaq burn --submit --prove-dir (nullifier recorded on Solana, proof kept for EVM)");

  [payerFile, depNote].forEach((p) => fs.rmSync(p, { force: true }));
}
main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

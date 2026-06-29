// M12 / Phase 2 (OPAQ.md B.4.3): on-chain transfer (2-in/2-out join-split), e2e.
//
// Deposits note A (the gen_witness fixture, leaf 0), then submits a transfer that
// spends A (+ 1 dummy input) into 2 output notes — verifying the tag-3 instruction
// records both nullifiers, inserts both output commitments, and moves NO tokens
// (value stays in the pool). Then re-submits to confirm the nullifier double-spend
// guard rejects it.
import fs from "node:fs";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo, getAccount,
} from "@solana/spl-token";
import { fetchTree } from "./read_path.mjs";

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, mintPath, depositBin, transferBin] = process.argv.slice(2);
const rpc = process.argv[6] || "http://127.0.0.1:8899";

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const AMOUNT = 1000n;
const conn = new Connection(rpc, "confirmed");
const payer = Keypair.generate();
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
const assert = (c, m) => { if (!c) throw new Error(m); };
const send = (ix, signers = [payer]) =>
  sendAndConfirmTransaction(conn, new Transaction()
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 })).add(ix),
    signers, { commitment: "confirmed" });
const bal = async (ata) => (await getAccount(conn, ata)).amount;

async function main() {
  await conn.confirmTransaction(await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "confirmed");

  const mint = await createMint(conn, payer, payer.publicKey, null, 0, mintKp);
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

  // deposit note A (-> leaf 0)
  await send(new TransactionInstruction({
    programId, data: fs.readFileSync(depositBin),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: depositorAta, isSigner: false, isWritable: true },
      { pubkey: vaultAta, isSigner: false, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    ],
  }));
  let t = await fetchTree(conn, tree);
  assert(t.nextIndex === 1n, `tree next_index should be 1 after deposit, got ${t.nextIndex}`);
  assert(await bal(vaultAta) === AMOUNT, "vault != amount after deposit");
  console.log("  OK  deposited note A (leaf 0), vault funded");

  // transfer: spend A (+ dummy) -> 2 output notes. No vault movement.
  const transferIx = () => new TransactionInstruction({
    programId, data: fs.readFileSync(transferBin),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  });
  await send(transferIx());
  t = await fetchTree(conn, tree);
  assert(t.nextIndex === 3n, `tree next_index should be 3 after transfer (2 outputs), got ${t.nextIndex}`);
  assert(await bal(vaultAta) === AMOUNT, "transfer must NOT move vault tokens");
  console.log("  OK  transfer landed: 2 output commitments inserted, vault untouched");

  // double-spend: re-submitting the same transfer reuses A's nullifier -> reject.
  let rejected = false;
  try { await send(transferIx()); } catch { rejected = true; }
  assert(rejected, "re-submitted transfer (nullifier reuse) should be rejected");
  console.log("  OK  transfer double-spend (nullifier reuse) rejected");

  console.log("\nM12 PASSED — Phase 2 join-split on-chain: 2-in/2-out transfer verified," +
    " nullifiers recorded, output commitments inserted, no token movement.");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

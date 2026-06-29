// M14 / Phase 3 P3.1 (OPAQ.md A.6): on-chain burn (cross-chain), e2e.
//
// Deposits note A, then submits a burn that spends it — verifying the tag-4
// instruction records the nullifier but inserts NO commitment (tree unchanged)
// and releases NO SPL (vault unchanged): the value is now locked on Solana,
// claimable on the EVM side via the same proof. Then confirms the double-burn
// (nullifier reuse) guard rejects a replay.
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
const [progPath, mintPath, depositBin, burnBin] = process.argv.slice(2);
const rpc = process.argv[6] || "http://127.0.0.1:8899";

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const AMOUNT = 1000n;
const conn = new Connection(rpc, "confirmed");
const payer = Keypair.generate();
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
const assert = (c, m) => { if (!c) throw new Error(m); };
const send = (ix) => sendAndConfirmTransaction(conn, new Transaction()
  .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 })).add(ix), [payer], { commitment: "confirmed" });
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
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "deposit should land leaf 0");
  assert(await bal(vaultAta) === AMOUNT, "vault != amount after deposit");
  console.log("  OK  deposited note A (leaf 0), vault funded");

  // burn: spend A cross-chain. No tree insert, no SPL release.
  const burnIx = () => new TransactionInstruction({
    programId, data: fs.readFileSync(burnBin),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: false },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  });
  await send(burnIx());
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "burn must NOT insert a commitment");
  assert(await bal(vaultAta) === AMOUNT, "burn must NOT release SPL (value locked on Solana)");
  console.log("  OK  burn landed: nullifier recorded, tree + vault unchanged");

  let rejected = false;
  try { await send(burnIx()); } catch { rejected = true; }
  assert(rejected, "re-submitted burn (nullifier reuse) should be rejected");
  console.log("  OK  burn double-spend (nullifier reuse) rejected");

  console.log("\nM14 PASSED — Phase 3 burn on-chain: note burned (nullifier recorded)," +
    " no commitment inserted, no SPL released — value locked for the EVM mint.");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

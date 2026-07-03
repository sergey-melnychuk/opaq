// M19 / Phase 4 P4.1 (OPAQ.md B.12.5): mint_from_xburn on-chain, e2e.
//
// Solana is the DESTINATION of a cross-chain shielded move here (the reverse
// leg of the symmetric bridge) — simulated with an off-chain xburn.nr proof
// fixture standing in for a real EVM-origin burn (P4.2's OpaqPool.sol isn't
// built yet; P4.3's m20 does the live round trip once it is). Covers every
// P4.1 accept-criteria case: happy path, double-mint rejection, wrong
// dest_chain rejection, and unattested-nullifier rejection.
import fs from "node:fs";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import { fetchTree } from "./read_path.mjs";

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, valuesAPath, xburnABin, xburnBBin] = process.argv.slice(2);
const rpc = process.argv[6] || "http://127.0.0.1:8899";

const programId = kp(progPath).publicKey;
const valuesA = JSON.parse(fs.readFileSync(valuesAPath, "utf8"));
const srcNullifierA = Buffer.from(valuesA.src_nullifier, "hex");

const conn = new Connection(rpc, "confirmed");
const payer = Keypair.generate();
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
const assert = (c, m) => { if (!c) throw new Error(m); };
const send = (ix) => sendAndConfirmTransaction(conn, new Transaction()
  .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 })).add(ix), [payer], { commitment: "confirmed" });

async function main() {
  await conn.confirmTransaction(await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "confirmed");
  const tree = pda([Buffer.from("tree")]);
  const nullifiers = pda([Buffer.from("nullifiers")]);
  const xpending = pda([Buffer.from("xpending")]);

  // init pool (tree + nullifier set — mint_from_xburn only needs the tree,
  // but initialize_pool creates both together)
  await send(new TransactionInstruction({
    programId, data: Buffer.from([0]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));

  // init xburn_pending, operator = payer
  await send(new TransactionInstruction({
    programId, data: Buffer.concat([Buffer.from([5]), payer.publicKey.toBuffer()]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: xpending, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  console.log("  OK  xburn_pending initialized (operator = payer)");

  // attest fixture A's src_nullifier (simulates the operator mirroring a
  // finalized EVM-source xburn, A.9)
  await send(new TransactionInstruction({
    programId, data: Buffer.concat([Buffer.from([6]), srcNullifierA]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: xpending, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  console.log("  OK  operator attests fixture A's src_nullifier as pending");

  const mintIx = (data) => new TransactionInstruction({
    programId, data,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: xpending, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  });

  // 1) happy path: fixture A mints a note on Solana's tree
  const blobA = fs.readFileSync(xburnABin);
  await send(mintIx(blobA));
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "mint_from_xburn should insert one leaf");
  console.log("  OK  mint_from_xburn lands (EVM-origin note re-shielded on Solana, leaf 0)");

  // 2) double-mint rejected (same nullifier already marked minted)
  let doubleMintRejected = false;
  try { await send(mintIx(blobA)); } catch { doubleMintRejected = true; }
  assert(doubleMintRejected, "re-submitted mint_from_xburn (double-mint) should be rejected");
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "double-mint must not insert a second leaf");
  console.log("  OK  double-mint rejected");

  // 3) wrong dest_chain rejected: tamper blobA's dest_chain field (bytes
  // 321..353 of the instruction data: tag(1)+proof(256)+src_root(32)+src_nullifier(32))
  const tampered = Buffer.from(blobA);
  tampered[352] ^= 0xff; // flip the low byte of dest_chain
  let wrongChainRejected = false;
  try { await send(mintIx(tampered)); } catch { wrongChainRejected = true; }
  assert(wrongChainRejected, "mint_from_xburn with a wrong dest_chain should be rejected");
  console.log("  OK  wrong dest_chain rejected");

  // 4) unattested nullifier rejected: fixture B's src_nullifier was never
  // add_pending_xburn'd
  const blobB = fs.readFileSync(xburnBBin);
  let unattestedRejected = false;
  try { await send(mintIx(blobB)); } catch { unattestedRejected = true; }
  assert(unattestedRejected, "mint_from_xburn for an unattested nullifier should be rejected");
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "unattested mint must not insert a leaf");
  console.log("  OK  unattested nullifier rejected");

  console.log("\nM19 PASSED — Phase 4 mint_from_xburn on-chain: EVM-origin note re-shielded on" +
    " Solana; double-mint, wrong dest_chain, and unattested nullifier all rejected.");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

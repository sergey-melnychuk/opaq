// P5.2 fixture helper (OPAQ.md B.14.6): submit a real deposit + xburn (tag 8,
// Solana as SOURCE) on a local validator, then write out the facts the ICP
// attestor canister's Solana leg needs to verify: program id, the xburn tx's
// own signature, and the (nullifier, dest_chain, out_commitment) it recorded.
//
// Usage: p5.2-fixture.mjs <progKp> <mintKp> <depositBin> <xburnBin>
//   <proveXburnDir> <solRpc> <outJson>
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import { TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo } from "@solana/spl-token";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, mintPath, depositBin, xburnBin, proveXburnDir, solRpc, outJson] = process.argv.slice(2);

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const AMOUNT = 1000n;
const conn = new Connection(solRpc, "confirmed");
const payer = Keypair.generate();
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];

async function send(ix) {
  return sendAndConfirmTransaction(conn, new Transaction()
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 })).add(ix), [payer], { commitment: "finalized" });
}

async function main() {
  await conn.confirmTransaction(await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "finalized");
  // Belt-and-suspenders against a known solana-test-validator race: preflight
  // simulation can still see a stale bank ("no record of a prior credit")
  // right after confirmTransaction resolves — poll the balance directly.
  for (let i = 0; i < 20 && (await conn.getBalance(payer.publicKey, "finalized")) === 0; i++) {
    await new Promise((r) => setTimeout(r, 250));
  }
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
  console.log("  OK  deposit (leaf 0)");

  const xburnSig = await send(new TransactionInstruction({
    programId, data: fs.readFileSync(xburnBin),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: false },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  console.log(`  OK  xburn (tag 8), tx=${xburnSig}`);

  // Also submit a DIFFERENT, non-xburn transaction (a second, harmless
  // init_pool-shaped call fails since it's already init'd — instead just
  // reuse the deposit tx's own signature) so the canister's P5.2 test can
  // confirm it rejects a real, finalized, but wrong-tag transaction.
  const depositTxSigs = await conn.getSignaturesForAddress(programId, { limit: 10 }, "finalized");
  const depositSig = depositTxSigs.find((s) => s.signature !== xburnSig)?.signature;

  const pub = JSON.parse(fs.readFileSync(path.join(proveXburnDir, "public.json"), "utf8"));
  const hex32 = (x) => BigInt(x).toString(16).padStart(64, "0");
  const fixture = {
    rpc_url: solRpc,
    program_id: programId.toBase58(),
    xburn_tx_signature: xburnSig,
    wrong_tag_tx_signature: depositSig,
    nullifier_hex: "0x" + hex32(pub[1]),
    dest_chain_hex: "0x" + hex32(pub[2]),
    out_commitment_hex: "0x" + hex32(pub[3]),
  };
  fs.writeFileSync(outJson, JSON.stringify(fixture, null, 2));
  console.log(`  wrote ${outJson}`);
}

main().catch((e) => { console.error(e); process.exit(1); });

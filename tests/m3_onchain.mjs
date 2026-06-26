// M3 on-chain leg (OPAQ.md B.6): submit a real Groth16 proof to the deployed
// verifier program. A confirmed tx means the embedded VK verified the proof in
// the SBF VM; a tampered proof must be rejected on-chain.
//
// Usage: node m3_onchain.mjs <programKeypairJson> <instruction.bin> [rpcUrl]
import fs from "node:fs";
import {
  Connection, Keypair, Transaction, TransactionInstruction,
  ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";

const programId = Keypair.fromSecretKey(
  Uint8Array.from(JSON.parse(fs.readFileSync(process.argv[2], "utf8")))
).publicKey;
const data = fs.readFileSync(process.argv[3]); // 352-byte instruction blob
const conn = new Connection(process.argv[4] || "http://127.0.0.1:8899", "confirmed");
const payer = Keypair.generate();

async function send(buf) {
  const tx = new Transaction()
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 })) // groth16 verify ~272k CU
    .add(new TransactionInstruction({ keys: [], programId, data: buf }));
  return sendAndConfirmTransaction(conn, tx, [payer], { commitment: "confirmed" });
}

async function main() {
  await conn.confirmTransaction(
    await conn.requestAirdrop(payer.publicKey, 2 * LAMPORTS_PER_SOL), "confirmed");
  console.log(`program: ${programId.toBase58()}  (instruction ${data.length} bytes)`);

  await send(data); // throws if the program rejects
  console.log("  OK  valid deposit proof verified on-chain (SBF sol_alt_bn128)");

  const bad = Buffer.from(data);
  bad[255] ^= 0x01; // flip a low bit of proof_c.y
  let rejected = false;
  try { await send(bad); } catch { rejected = true; }
  if (!rejected) throw new Error("negative control FAILED: tampered proof accepted on-chain");
  console.log("  OK  tampered proof rejected on-chain (Custom(1))");

  console.log("\nM3 on-chain leg PASSED — groth16-solana verifies in the SBF VM.");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

// M0.5 CU-feasibility measurement (OPAQ.md B.6). Measures real Solana compute
// units per alt_bn128 op and per BN254 Fr multiply on this validator, then plugs
// them into UltraHonk vs Groth16 verifier-cost models to decide which fits one
// transaction's 1.4M-CU ceiling.
//
// Usage: node m05_cu.mjs <programKeypairJson> [rpcUrl]
import fs from "node:fs";
import {
  Connection, Keypair, Transaction, TransactionInstruction,
  ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";

const programId = Keypair.fromSecretKey(
  Uint8Array.from(JSON.parse(fs.readFileSync(process.argv[2], "utf8")))
).publicKey;
const conn = new Connection(process.argv[3] || "http://127.0.0.1:8899", "confirmed");
const payer = Keypair.generate();

const CU_CEILING = 1_400_000;          // Solana per-tx max with a budget request
const PAIR_CU = 36_364;                // per-pair alt_bn128_pairing (OPAQ.md A.10 / docs)

async function measure(mode, count) {
  const data = Buffer.alloc(5);
  data[0] = mode;
  data.writeUInt32LE(count, 1);
  const tx = new Transaction()
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: CU_CEILING }))
    .add(new TransactionInstruction({ keys: [], programId, data }));
  const sig = await sendAndConfirmTransaction(conn, tx, [payer], { commitment: "confirmed" });
  const t = await conn.getTransaction(sig, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
  return t.meta.computeUnitsConsumed;
}

async function slope(name, mode, n1, n2) {
  const c1 = await measure(mode, n1);
  const c2 = await measure(mode, n2);
  const per = (c2 - c1) / (n2 - n1);
  console.log(`  ${name.padEnd(14)} N=${n1}:${c1}cu  N=${n2}:${c2}cu  ->  ${per.toFixed(1)} CU/op`);
  return per;
}

async function main() {
  await conn.confirmTransaction(
    await conn.requestAirdrop(payer.publicKey, 2 * LAMPORTS_PER_SOL), "confirmed");

  console.log(`program: ${programId.toBase58()}\nMeasuring per-op CU:`);
  const mul = await slope("G1 mul", 0, 100, 200);
  const add = await slope("G1 add", 1, 200, 400);
  const fr = await slope("Fr mul (BPF)", 3, 100, 300);

  // ---- verifier cost models (op counts; constants are explicit so they can be tuned) ----
  // UltraHonk (bb) for the withdraw circuit: ~2^15 gates -> d=15 sumcheck rounds,
  // ~44 polynomial entities. Shplemini batch opening MSM ~ (entities + d + const).
  const d = 15, entities = 44;
  const honkMul = entities + d + 10;   // ~69 scalar muls in the batched opening
  const honkAdd = entities + d + 10;   // ~69 point adds
  const honkPairs = 2;                 // KZG-style final check (2 Miller loops)
  // Field work: sumcheck round-univariate evals + the ~26-subrelation combine.
  // Wide range — this is the differentiator Groth16 lacks. Low/high estimates:
  const honkFrLo = 2_000, honkFrHi = 8_000;

  // Groth16: L = MSM over the 5 public inputs (+1), then a 4-pairing check. ~no field work.
  const g16Mul = 5 + 1, g16Add = 5, g16Pairs = 4, g16Fr = 50;

  const ec = (m, a, pairs) => m * mul + a * add + pairs * PAIR_CU;
  const honkLo = ec(honkMul, honkAdd, honkPairs) + honkFrLo * fr;
  const honkHi = ec(honkMul, honkAdd, honkPairs) + honkFrHi * fr;
  const g16 = ec(g16Mul, g16Add, g16Pairs) + g16Fr * fr;

  const fmt = (n) => `${Math.round(n).toLocaleString()} CU`;
  const verdict = (n) => n < CU_CEILING ? "FITS" : "OVER BUDGET";
  console.log(`\nVerifier-cost estimate (withdraw circuit), ceiling = ${fmt(CU_CEILING)}:`);
  console.log(`  UltraHonk  EC=${fmt(ec(honkMul, honkAdd, honkPairs))} + field(${honkFrLo}-${honkFrHi} Fr muls)`);
  console.log(`             total ${fmt(honkLo)} – ${fmt(honkHi)}   [${verdict(honkLo)} – ${verdict(honkHi)}]`);
  console.log(`  Groth16    total ${fmt(g16)}   [${verdict(g16)}]`);
  console.log(`\nNote: field-op count is modeled, EC/Fr per-op costs are measured on-chain.`);
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

// M0 on-chain leg (OPAQ.md B.0 step 4): invoke the deployed poseidon-syscall-check
// program for each reference vector, passing the off-chain light-poseidon hash as
// `expected`. The program runs the REAL sol_poseidon syscall and returns Custom(1)
// on mismatch — so a confirmed transaction proves syscall == off-chain, byte-for-byte.
//
// Usage: node m0_syscall.mjs <programKeypairJson> [rpcUrl]
import fs from "node:fs";
import {
  Connection, Keypair, Transaction, TransactionInstruction,
  sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";

const programKeypairPath = process.argv[2];
const rpcUrl = process.argv[3] || "http://127.0.0.1:8899";

const programId = Keypair.fromSecretKey(
  Uint8Array.from(JSON.parse(fs.readFileSync(programKeypairPath, "utf8")))
).publicKey;

function be32(n) {
  const b = Buffer.alloc(32);
  b.writeBigUInt64BE(BigInt(n), 24);
  return b;
}
const hexBuf = (h) => Buffer.from(h, "hex");
const repeatByte = (byte) => Buffer.alloc(32, byte);

// Reference vectors — must match circuits/poseidon_check + crates/common (M0).
const vectors = [
  { a: be32(1), b: be32(2), expected: "115cc0f5e7d690413df64c6b9662e9cf2a3617f2743245519e19607a4417189a" },
  { a: be32(0), b: be32(0), expected: "2098f5fb9e239eab3ceac3f27b81e481dc3124d55ffed523a839ee8446b64864" },
  { a: be32(3), b: be32(4), expected: "20a3af0435914ccd84b806164531b0cd36e37d4efb93efab76913a93e1f30996" },
  { a: repeatByte(0x11), b: repeatByte(0x22), expected: "036e25235e4790f28f7dbed7eb3a0841726264a350565324e764beab84ba918b" },
];

const send = (conn, payer, data) => {
  const ix = new TransactionInstruction({ keys: [], programId, data });
  return sendAndConfirmTransaction(conn, new Transaction().add(ix), [payer], {
    commitment: "confirmed", skipPreflight: false,
  });
};

async function main() {
  const conn = new Connection(rpcUrl, "confirmed");
  const payer = Keypair.generate();
  const sig = await conn.requestAirdrop(payer.publicKey, 2 * LAMPORTS_PER_SOL);
  await conn.confirmTransaction(sig, "confirmed");

  console.log(`program: ${programId.toBase58()}`);
  let pass = 0;
  for (const { a, b, expected } of vectors) {
    const data = Buffer.concat([a, b, hexBuf(expected)]);
    await send(conn, payer, data); // throws if program rejects (syscall != expected)
    console.log(`  OK  syscall(${a.toString("hex").slice(-4)}, ${b.toString("hex").slice(-4)}) == 0x${expected.slice(0, 12)}…`);
    pass++;
  }

  // Negative control: wrong `expected` must be rejected on-chain.
  const v = vectors[0];
  const bad = Buffer.concat([v.a, v.b, hexBuf("ff".repeat(32))]);
  let rejected = false;
  try { await send(conn, payer, bad); } catch { rejected = true; }
  if (!rejected) throw new Error("negative control FAILED: bad expected was accepted on-chain");
  console.log("  OK  negative control: wrong expected rejected (Custom(1))");

  console.log(`\nM0 on-chain leg PASSED — ${pass}/${vectors.length} vectors + negative control. ` +
    `sol_poseidon syscall matches off-chain Poseidon byte-for-byte.`);
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

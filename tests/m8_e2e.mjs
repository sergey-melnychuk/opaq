// M8 end-to-end (OPAQ.md B.8): full deposit -> withdraw round-trip on a validator
// with REAL Groth16 proofs, plus double-spend rejection.
//
// Usage: node m8_e2e.mjs <programKeypair> <mintKeypair> <recipientKeypair> <deposit.bin> <withdraw.bin> [rpc]
import fs from "node:fs";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo, getAccount,
} from "@solana/spl-token";

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, mintPath, recipPath, depositBin, withdrawBin] = process.argv.slice(2);
const rpc = process.argv[7] || "http://127.0.0.1:8899";

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const recipient = kp(recipPath);
const AMOUNT = 1000n;
const conn = new Connection(rpc, "confirmed");
const payer = Keypair.generate();

const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];

async function send(ix, signers = [payer]) {
  const tx = new Transaction()
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 }))
    .add(ix);
  return sendAndConfirmTransaction(conn, tx, signers, { commitment: "confirmed" });
}
const bal = async (ata) => (await getAccount(conn, ata)).amount;

async function expectFail(ix, label) {
  let ok = false;
  try { await send(ix); } catch { ok = true; }
  if (!ok) throw new Error(`negative control FAILED: ${label} was accepted`);
  console.log(`  OK  ${label} rejected`);
}
// Overwrite the 8-byte LE amount at `off` with a different value (forge attempt).
function forgeAmount(buf, off) {
  const b = Buffer.from(buf);
  b.writeBigUInt64LE(9999n, off);
  return b;
}

async function main() {
  await conn.confirmTransaction(
    await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "confirmed");

  // --- SPL setup ---
  const mint = await createMint(conn, payer, payer.publicKey, null, 0, mintKp);
  const depositorAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, payer.publicKey)).address;
  await mintTo(conn, payer, mint, depositorAta, payer, AMOUNT);

  const vaultAuthority = pda([Buffer.from("vault"), mint.toBuffer()]);
  const vaultAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true)).address;
  const recipientAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, recipient.publicKey)).address;

  const tree = pda([Buffer.from("tree")]);
  const nullifiers = pda([Buffer.from("nullifiers")]);
  console.log(`program ${programId.toBase58()}\nmint ${mint.toBase58()}`);

  // --- initialize_pool (tag 0) ---
  await send(new TransactionInstruction({
    programId, data: Buffer.from([0]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  console.log("  OK  initialize_pool");

  // --- deposit (tag 1, bin includes tag) ---
  const depositData = fs.readFileSync(depositBin);
  const depositIx = (data) => new TransactionInstruction({
    programId, data,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },     // depositor
      { pubkey: depositorAta, isSigner: false, isWritable: true },
      { pubkey: vaultAta, isSigner: false, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    ],
  });
  // Test 3 (forged input): claim a different amount than the proof was made for.
  await expectFail(depositIx(forgeAmount(depositData, 1 + 256 + 32)), "forged-amount deposit");
  await send(depositIx(depositData));
  if (await bal(vaultAta) !== AMOUNT) throw new Error("vault balance after deposit != amount");
  console.log(`  OK  deposit: vault holds ${AMOUNT}`);

  // --- withdraw (tag 2) ---
  const withdrawData = fs.readFileSync(withdrawBin);
  const withdrawIx = (data, recipAta = recipientAta) => new TransactionInstruction({
    programId, data,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: vaultAuthority, isSigner: false, isWritable: false },
      { pubkey: vaultAta, isSigner: false, isWritable: true },
      { pubkey: recipAta, isSigner: false, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: false },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  });
  // Test 3 (withdraw): forged amount must be rejected at proof verification.
  await expectFail(withdrawIx(forgeAmount(withdrawData, 1 + 256 + 96)), "forged-amount withdraw");
  // Recipient-binding: paying a token account not owned by the bound recipient.
  await expectFail(withdrawIx(withdrawData, depositorAta), "wrong-recipient withdraw");
  await send(withdrawIx(withdrawData));
  if (await bal(recipientAta) !== AMOUNT || await bal(vaultAta) !== 0n) {
    throw new Error("balances after withdraw incorrect");
  }
  console.log(`  OK  withdraw: recipient got ${AMOUNT}, vault drained`);

  // --- double-spend: same nullifier must be rejected ---
  await expectFail(withdrawIx(withdrawData), "double-spend (nullifier reuse)");

  console.log(
    "\nM8 PASSED — round-trip + forged-input (deposit & withdraw) + wrong-recipient" +
    " + double-spend all enforced on-chain."
  );
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

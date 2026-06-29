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
const [progPath, mintPath, recipPath, depositBin, depositBBin, withdrawBin, mint2Path, depositCBin] =
  process.argv.slice(2);
const rpc = process.argv[10] || "http://127.0.0.1:8899";

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const mint2Kp = kp(mint2Path);
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
  await mintTo(conn, payer, mint, depositorAta, payer, 2n * AMOUNT); // fund notes A + B

  const vaultAuthority = pda([Buffer.from("vault"), mint.toBuffer()]);
  const vaultAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true)).address;
  const recipientAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, recipient.publicKey)).address;

  // Second token (mint2) for the multi-token isolation check (Test 6).
  const mint2 = await createMint(conn, payer, payer.publicKey, null, 0, mint2Kp);
  const depositor2Ata = (await getOrCreateAssociatedTokenAccount(conn, payer, mint2, payer.publicKey)).address;
  await mintTo(conn, payer, mint2, depositor2Ata, payer, AMOUNT);
  const vault2Authority = pda([Buffer.from("vault"), mint2.toBuffer()]);
  const vault2Ata = (await getOrCreateAssociatedTokenAccount(conn, payer, mint2, vault2Authority, true)).address;

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
  const depositIx = (data, depAta = depositorAta, vAta = vaultAta, tokenProg = TOKEN_PROGRAM_ID) =>
    new TransactionInstruction({
      programId, data,
      keys: [
        { pubkey: payer.publicKey, isSigner: true, isWritable: true },     // depositor
        { pubkey: depAta, isSigner: false, isWritable: true },
        { pubkey: vAta, isSigner: false, isWritable: true },
        { pubkey: tree, isSigner: false, isWritable: true },
        { pubkey: tokenProg, isSigner: false, isWritable: false },
      ],
    });
  // Test 3 (forged input): claim a different amount than the proof was made for.
  await expectFail(depositIx(forgeAmount(depositData, 1 + 256 + 32)), "forged-amount deposit");
  // Audit fix: a forged token_program must be rejected — otherwise a no-op
  // "transfer" would let a deposit insert a commitment WITHOUT funding the vault,
  // then drain it on withdraw. Pass the System program in the token_program slot.
  await expectFail(depositIx(depositData, depositorAta, vaultAta, SystemProgram.programId),
    "forged token_program deposit");
  // Note A, then note B (distinct commitment, same fixed VK) — B moves the root
  // so the later withdraw of A exercises stale-root tolerance (Test 4).
  await send(depositIx(depositData));
  await send(depositIx(fs.readFileSync(depositBBin)));
  if (await bal(vaultAta) !== 2n * AMOUNT) throw new Error("vault != 2*amount after two deposits");
  // Note C into the OTHER token's vault (Test 6).
  await send(depositIx(fs.readFileSync(depositCBin), depositor2Ata, vault2Ata));
  if (await bal(vault2Ata) !== AMOUNT) throw new Error("mint2 vault != amount after deposit C");
  console.log(`  OK  deposit A + B (mint1) + C (mint2): vaults ${2n * AMOUNT} / ${AMOUNT}`);

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
  // Withdraw note A against root-after-A, now STALE (B advanced the current root)
  // but still in the ring buffer (Test 4). Note B's funds must stay put.
  await send(withdrawIx(withdrawData));
  if (await bal(recipientAta) !== AMOUNT || await bal(vaultAta) !== AMOUNT) {
    throw new Error("balances after withdraw incorrect");
  }
  // Test 6: the mint2 vault must be completely untouched by a mint1 withdraw.
  if (await bal(vault2Ata) !== AMOUNT) throw new Error("mint2 vault touched by mint1 withdraw");
  console.log(`  OK  withdraw A (stale root): recipient ${AMOUNT}; note B + mint2 vault untouched`);

  // --- double-spend: same nullifier must be rejected ---
  await expectFail(withdrawIx(withdrawData), "double-spend (nullifier reuse)");

  console.log(
    "\nM8 PASSED — round-trip + stale-root (T4) + multi-token isolation (T6)" +
    " + forged-input (T3) + forged-token_program + wrong-recipient + double-spend (T2), all on-chain."
  );
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

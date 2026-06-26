// M11 — Devnet demo (OPAQ.md B.8 Test 1): deposit -> withdraw round-trip on a
// public RPC endpoint (default: Solana devnet). Uses the deploy wallet as payer
// so the same keypair that funded program deploy can drive the demo.
//
// Usage: node m11_devnet_demo.mjs <programKp> <payerKp> <mintKp> <recipientKp> \
//          <deposit.bin> <withdraw.bin> [rpc]
import fs from "node:fs";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo, getAccount,
} from "@solana/spl-token";

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, payerPath, mintPath, recipPath, depositBin, withdrawBin] = process.argv.slice(2);
const rpc = process.argv[8] || "https://api.devnet.solana.com";

const programId = kp(progPath).publicKey;
const payer = kp(payerPath);
const mintKp = kp(mintPath);
const recipient = kp(recipPath);
const AMOUNT = 1000n;
const conn = new Connection(rpc, {
  commitment: "confirmed",
  confirmTransactionInitialTimeout: 120_000,
});
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
const assert = (cond, msg) => { if (!cond) throw new Error(msg); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function withRetry(label, fn, tries = 10) {
  for (let i = 0; i < tries; i++) {
    try {
      return await fn();
    } catch (e) {
      const msg = String(e?.message ?? e);
      if (i + 1 < tries && /429|Too many requests|ECONNRESET|ETIMEDOUT/i.test(msg)) {
        const wait = Math.min(8000, 1500 * (i + 1));
        console.log(`  ..  ${label}: RPC busy (429), retry in ${wait}ms (${i + 1}/${tries})`);
        await sleep(wait);
        continue;
      }
      throw e;
    }
  }
}

async function send(ix, signers = [payer]) {
  return withRetry("send", async () => {
    const tx = new Transaction()
      .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_400_000 }))
      .add(ix);
    return sendAndConfirmTransaction(conn, tx, signers, { commitment: "confirmed" });
  });
}
const bal = async (ata) => (await getAccount(conn, ata)).amount;

async function ensureSol(minSol = 2) {
  const have = await withRetry("balance", () => conn.getBalance(payer.publicKey, "confirmed"));
  if (have >= minSol * LAMPORTS_PER_SOL) return;
  console.log(`  ..  payer low on SOL (${have / LAMPORTS_PER_SOL}), requesting devnet airdrop`);
  await withRetry("airdrop", async () => {
    const sig = await conn.requestAirdrop(payer.publicKey, minSol * LAMPORTS_PER_SOL);
    await conn.confirmTransaction(sig, "confirmed");
  });
  await sleep(2000);
}

async function main() {
  await ensureSol(2);
  await sleep(1500);

  const mint = await withRetry("createMint", () =>
    createMint(conn, payer, payer.publicKey, null, 0, mintKp));
  await sleep(1500);
  const depositorAta = await withRetry("depositor ATA", () =>
    getOrCreateAssociatedTokenAccount(conn, payer, mint, payer.publicKey).then((a) => a.address));
  await sleep(1000);
  await withRetry("mintTo", () => mintTo(conn, payer, mint, depositorAta, payer, AMOUNT));
  await sleep(1500);
  const vaultAuthority = pda([Buffer.from("vault"), mint.toBuffer()]);
  const vaultAta = await withRetry("vault ATA", () =>
    getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true).then((a) => a.address));
  await sleep(1000);
  const recipientAta = await withRetry("recipient ATA", () =>
    getOrCreateAssociatedTokenAccount(conn, payer, mint, recipient.publicKey).then((a) => a.address));
  await sleep(1500);

  const tree = pda([Buffer.from("tree")]);
  const nullifiers = pda([Buffer.from("nullifiers")]);
  console.log(`rpc     ${rpc}`);
  console.log(`program ${programId.toBase58()}`);
  console.log(`mint    ${mint.toBase58()}`);
  console.log(`tree    ${tree.toBase58()}`);

  if (!(await withRetry("tree account", () => conn.getAccountInfo(tree, "confirmed")))) {
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
  } else {
    console.log("  OK  pool already initialized (reusing existing devnet deployment)");
  }

  const depositData = fs.readFileSync(depositBin);
  await send(new TransactionInstruction({
    programId, data: depositData,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: depositorAta, isSigner: false, isWritable: true },
      { pubkey: vaultAta, isSigner: false, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    ],
  }));
  await sleep(2000);
  assert(await bal(vaultAta) === AMOUNT, "vault != amount after deposit");
  console.log(`  OK  deposit ${AMOUNT} tokens (real Groth16 proof)`);

  const withdrawData = fs.readFileSync(withdrawBin);
  await send(new TransactionInstruction({
    programId, data: withdrawData,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: vaultAuthority, isSigner: false, isWritable: false },
      { pubkey: vaultAta, isSigner: false, isWritable: true },
      { pubkey: recipientAta, isSigner: false, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: false },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  assert(await bal(recipientAta) === AMOUNT, "recipient did not receive withdrawn amount");
  assert(await bal(vaultAta) === 0n, "vault should be empty after withdraw");
  console.log(`  OK  withdraw ${AMOUNT} tokens to fresh recipient`);

  console.log(
    "\nM11 PASSED — devnet demo: real deposit + withdraw round-trip on a public RPC" +
    ` (${rpc}).`
  );
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

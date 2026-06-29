// Broadcast a withdraw (M9 full auto-submit): given the prove-only instruction
// blob + a payer keypair, derive the program accounts, sign, and send the tx.
// `opaq withdraw --submit` spawns this (chain I/O stays in the node path, the
// single place the account layout lives alongside the tests that exercise it).
//
// Usage: node submit_withdraw.mjs <rpc> <programId> <blobPath> <payerKeypair> <mintHex> <recipientBase58>
import fs from "node:fs";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, sendAndConfirmTransaction,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID, getOrCreateAssociatedTokenAccount,
} from "@solana/spl-token";

const [rpc, programArg, blobPath, payerPath, mintHex, recipientArg] = process.argv.slice(2);
if (!rpc || !programArg || !blobPath || !payerPath || !mintHex || !recipientArg) {
  console.error("usage: node submit_withdraw.mjs <rpc> <programId> <blob> <payerKeypair> <mintHex> <recipient>");
  process.exit(2);
}

try {
  const conn = new Connection(rpc, "confirmed");
  const programId = new PublicKey(programArg);
  const payer = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(payerPath, "utf8"))));
  const mint = new PublicKey(Buffer.from(mintHex, "hex"));
  const recipient = new PublicKey(recipientArg);
  const data = fs.readFileSync(blobPath);

  const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
  const vaultAuthority = pda([Buffer.from("vault"), mint.toBuffer()]);
  // vault token account is the vault PDA's ATA (off-curve owner); created at first deposit.
  const vaultAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true)).address;
  const recipientAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, recipient)).address;
  const tree = pda([Buffer.from("tree")]);
  const nullifiers = pda([Buffer.from("nullifiers")]);

  const ix = new TransactionInstruction({
    programId, data,
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
  });
  const sig = await sendAndConfirmTransaction(conn, new Transaction().add(ix), [payer], { commitment: "confirmed" });
  process.stdout.write(sig);
} catch (e) {
  console.error(`submit_withdraw: ${e?.message ?? e}`);
  process.exit(1);
}

// Broadcast a deposit (M9 full auto-submit): given the prove-only instruction
// blob + the depositor's keypair, derive the program accounts, sign, and send.
// `opaq deposit --submit` spawns this. The depositor's token account must already
// hold `amount` of the mint (a deposit moves real SPL into the vault); this helper
// creates the depositor + vault ATAs if missing but never mints.
//
// Usage: node submit_deposit.mjs <rpc> <programId> <blobPath> <payerKeypair> <mintHex>
import fs from "node:fs";
import {
  Connection, Keypair, PublicKey, Transaction, TransactionInstruction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import { TOKEN_PROGRAM_ID, getOrCreateAssociatedTokenAccount } from "@solana/spl-token";

const [rpc, programArg, blobPath, payerPath, mintHex] = process.argv.slice(2);
if (!rpc || !programArg || !blobPath || !payerPath || !mintHex) {
  console.error("usage: node submit_deposit.mjs <rpc> <programId> <blob> <payerKeypair> <mintHex>");
  process.exit(2);
}

try {
  const conn = new Connection(rpc, "confirmed");
  const programId = new PublicKey(programArg);
  const payer = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(payerPath, "utf8"))));
  const mint = new PublicKey(Buffer.from(mintHex, "hex"));
  const data = fs.readFileSync(blobPath);

  const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
  const depositorAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, payer.publicKey)).address;
  const vaultAuthority = pda([Buffer.from("vault"), mint.toBuffer()]);
  const vaultAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true)).address;
  const tree = pda([Buffer.from("tree")]);

  const ix = new TransactionInstruction({
    programId, data,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: depositorAta, isSigner: false, isWritable: true },
      { pubkey: vaultAta, isSigner: false, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    ],
  });
  const sig = await sendAndConfirmTransaction(conn, new Transaction().add(ix), [payer], { commitment: "confirmed" });
  process.stdout.write(sig);
} catch (e) {
  console.error(`submit_deposit: ${e?.message ?? e}`);
  process.exit(1);
}

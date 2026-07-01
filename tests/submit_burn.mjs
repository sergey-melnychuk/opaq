// Broadcast a burn (Phase 3 cross-chain): given the prove-only instruction blob
// + a payer keypair, derive the tree + nullifier PDAs, sign, and send the tag-4
// burn tx. Records the nullifier on Solana; releases NO SPL and inserts NO
// commitment — the value is now claimable on the EVM side via the same proof.
// `opaq burn --submit` spawns this (chain I/O stays in the node path, alongside
// the m14 test that exercises the same account layout).
//
// Usage: node submit_burn.mjs <rpc> <programId> <blobPath> <payerKeypair>
import fs from "node:fs";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction,
} from "@solana/web3.js";

const [rpc, programArg, blobPath, payerPath] = process.argv.slice(2);
if (!rpc || !programArg || !blobPath || !payerPath) {
  console.error("usage: node submit_burn.mjs <rpc> <programId> <blob> <payerKeypair>");
  process.exit(2);
}

try {
  const conn = new Connection(rpc, "confirmed");
  const programId = new PublicKey(programArg);
  const payer = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(payerPath, "utf8"))));
  const data = fs.readFileSync(blobPath);

  const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
  const tree = pda([Buffer.from("tree")]);
  const nullifiers = pda([Buffer.from("nullifiers")]);

  const ix = new TransactionInstruction({
    programId, data,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: false }, // read-only: burn checks the root, no insert
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  });
  const tx = new Transaction()
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 })) // Groth16 verify headroom
    .add(ix);
  const sig = await sendAndConfirmTransaction(conn, tx, [payer], { commitment: "confirmed" });
  process.stdout.write(sig);
} catch (e) {
  console.error(`submit_burn: ${e?.message ?? e}`);
  process.exit(1);
}

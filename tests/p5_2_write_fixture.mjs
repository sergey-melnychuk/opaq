// P5.2 write-side fixture helper: initialize XburnPending with a GIVEN
// operator pubkey (the ICP attestor canister's own threshold-Ed25519
// address) and fund that address with SOL, so the canister's own
// submit_add_pending_xburn call can pay fees/rent and sign for real.
//
// Usage: p5_2_write_fixture.mjs <progKp> <operatorB58> <solRpc>
import fs from "node:fs";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [progPath, operatorB58, solRpc] = process.argv.slice(2);

const programId = kp(progPath).publicKey;
const operator = new PublicKey(operatorB58);
const conn = new Connection(solRpc, "confirmed");
const payer = Keypair.generate();
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];

async function send(ix) {
  return sendAndConfirmTransaction(conn, new Transaction()
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 })).add(ix), [payer], { commitment: "finalized" });
}

async function main() {
  await conn.confirmTransaction(await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "finalized");
  for (let i = 0; i < 20 && (await conn.getBalance(payer.publicKey, "finalized")) === 0; i++) {
    await new Promise((r) => setTimeout(r, 250));
  }

  const xpending = pda([Buffer.from("xpending")]);
  await send(new TransactionInstruction({
    programId, data: Buffer.concat([Buffer.from([5]), operator.toBuffer()]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: xpending, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  console.log(`  OK  initialize_xburn_pending (operator=${operatorB58}), xpending=${xpending.toBase58()}`);

  // Fund the operator (canister) address so it can pay its own tx fee + rent.
  await conn.confirmTransaction(await conn.requestAirdrop(operator, 2 * LAMPORTS_PER_SOL), "finalized");
  console.log(`  OK  funded operator with 2 SOL`);
}

main().catch((e) => { console.error(e); process.exit(1); });

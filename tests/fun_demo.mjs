// Fun demo: on-chain Tamagotchi + r/place, rendered in the terminal.
// Usage: node fun_demo.mjs <petProgKp> <placeProgKp> [rpc]
import fs from "node:fs";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";

const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [petProg, placeProg] = [kp(process.argv[2]).publicKey, kp(process.argv[3]).publicKey];
const conn = new Connection(process.argv[4] || "http://127.0.0.1:8899", "confirmed");
const payer = Keypair.generate();
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const send = (ix) => sendAndConfirmTransaction(conn, new Transaction().add(ix), [payer], { commitment: "confirmed" });

// ---------- Tamagotchi ----------
const petPda = PublicKey.findProgramAddressSync([Buffer.from("pet"), payer.publicKey.toBuffer()], petProg)[0];
const petIx = (data, init = false) => new TransactionInstruction({
  programId: petProg, data,
  keys: init
    ? [{ pubkey: payer.publicKey, isSigner: true, isWritable: true },
       { pubkey: petPda, isSigner: false, isWritable: true },
       { pubkey: SystemProgram.programId, isSigner: false, isWritable: false }]
    : [{ pubkey: payer.publicKey, isSigner: false, isWritable: false },
       { pubkey: petPda, isSigner: false, isWritable: true }],
});
const bar = (v, c) => `${c.repeat(Math.round(v / 10))}${"·".repeat(10 - Math.round(v / 10))}`;

async function renderPet(label) {
  const info = await conn.getAccountInfo(petPda, "confirmed");
  const slot = await conn.getSlot("confirmed");
  const d = info.data;
  const last = Number(d.readBigUInt64LE(0));
  const el = Math.min(255, slot - last);
  // mirror the program's lazy decay so the pet looks alive between txs
  const hunger = Math.min(100, d[8] + el);
  const happy = Math.max(0, d[9] - el);
  const energy = Math.max(0, d[10] - Math.floor(el / 2));
  const alive = d[11] === 1 && hunger < 100;
  const name = Buffer.from(d.subarray(12, 28)).toString("utf8").replace(/\0+$/, "");
  const face = !alive ? "(x_x)" : hunger > 70 ? "(T_T)" : happy > 70 ? "(^ω^)" : "(・_・)";
  console.log(`\n  ${label}  —  ${name} ${face}${alive ? "" : "  R.I.P."}  (slot ${slot})`);
  console.log(`    hunger    ${bar(hunger, "█")} ${hunger}`);
  console.log(`    happiness ${bar(happy, "♥")} ${happy}`);
  console.log(`    energy    ${bar(energy, "▮")} ${energy}`);
}

// ---------- r/place ----------
const W = 32, H = 32;
const canvasPda = PublicKey.findProgramAddressSync([Buffer.from("canvas")], placeProg)[0];
const paint = (x, y, color) => send(new TransactionInstruction({
  programId: placeProg, data: Buffer.from([1, x, y, color]),
  keys: [{ pubkey: payer.publicKey, isSigner: false, isWritable: false },
         { pubkey: canvasPda, isSigner: false, isWritable: true }],
}));
const PAL = [16, 196, 208, 226, 46, 51, 21, 201, 231, 240, 88, 28, 24, 53, 130, 244]; // 256-color
async function renderCanvas() {
  const info = await conn.getAccountInfo(canvasPda, "confirmed");
  const d = info.data;
  let out = "";
  for (let y = 0; y < H; y++) {
    out += "    ";
    for (let x = 0; x < W; x++) out += `\x1b[48;5;${PAL[d[y * W + x] & 15]}m  \x1b[0m`;
    out += "\n";
  }
  process.stdout.write(out);
}

// 11x9 pixel heart (1 = red). painted centered.
const HEART = [
  "01100110",
  "11111111",
  "11111111",
  "11111111",
  "01111110",
  "00111100",
  "00011000",
];

async function main() {
  await conn.confirmTransaction(await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "confirmed");

  console.log("=".repeat(50), "\n  🐣 ON-CHAIN TAMAGOTCHI");
  await send(petIx(Buffer.concat([Buffer.from([0]), Buffer.from("Bonk")]), true));
  await renderPet("hatched");
  console.log("\n  ...neglecting it for a few slots...");
  await sleep(7000);
  await renderPet("neglected");
  await send(petIx(Buffer.from([1]))); // feed
  await send(petIx(Buffer.from([2]))); // play
  await send(petIx(Buffer.from([3]))); // sleep
  await renderPet("fed + played + rested");

  console.log("\n" + "=".repeat(50), "\n  🟦 ON-CHAIN r/place (32×32)\n");
  await send(new TransactionInstruction({
    programId: placeProg, data: Buffer.from([0]),
    keys: [{ pubkey: payer.publicKey, isSigner: true, isWritable: true },
           { pubkey: canvasPda, isSigner: false, isWritable: true },
           { pubkey: SystemProgram.programId, isSigner: false, isWritable: false }],
  }));
  // paint a heart, then scatter a few "other users'" pixels
  for (let r = 0; r < HEART.length; r++)
    for (let c = 0; c < HEART[r].length; c++)
      if (HEART[r][c] === "1") await paint(11 + c, 8 + r, 1);
  for (let i = 0; i < 40; i++)
    await paint((Math.random() * W) | 0, (Math.random() * H) | 0, 1 + ((Math.random() * 15) | 0));
  await renderCanvas();

  console.log("\n  ✅ both running on a Solana validator. gm.\n");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

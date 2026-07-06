// M20 / Phase 4 P4.3 (OPAQ.md B.12.8): the SYMMETRIC round trip, live on a
// validator + anvil simultaneously — mirrors M18's "one person, one proof"
// structure, but closes the loop both ways with real re-shielding instead of
// OpaqMint's balance ledger:
//
//   1. deposit on Solana (leaf 0)
//   2. xburn it (tag 8, Solana as SOURCE) -> nullifier1 recorded on Solana
//   3. attest + mintFromXburn on OpaqPool (EVM as DESTINATION) -> a real,
//      spendable note lands as OpaqPool's leaf 0 (forward: Solana -> EVM)
//   4. xburn THAT SAME note from OpaqPool (EVM as SOURCE) -> nullifier2
//      recorded on EVM
//   5. attest + mint_from_xburn on Solana (tag 7, Solana as DESTINATION
//      again) -> a fresh note lands as Solana's leaf 1 (reverse: EVM -> Solana)
//
// Both xburn proofs share ONE fixed zkey/VK (same circuit, xburn.nr) proven
// over the REAL PPoT so both the Solana groth16-solana verifier AND the EVM
// ecMul precompile accept them (B.6/M15's finding) — gen_witness.rs computes
// both witnesses together so leg 2's source note is provably leg 1's
// destination note (crates/common/src/bin/gen_witness.rs's xburn2 block).
//
// Usage: node m20_symmetric_roundtrip.mjs <progKp> <mintKp> <depositBin>
//   <xburn1SolanaBin> <xburn2SolanaBin> <prove1Dir> <prove2Dir>
//   <evmRpc> <opaqPoolAddr> <operatorPrivKey> [solRpc]
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, ComputeBudgetProgram, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import { TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo } from "@solana/spl-token";
import { fetchTree } from "./read_path.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const kp = (p) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
const [
  progPath, mintPath, depositBin,
  xburn1SolanaBin, xburn2SolanaBin,
  prove1Dir, prove2Dir,
  evmRpc, opaqPoolAddr, opKey,
] = process.argv.slice(2);
const solRpc = process.argv[14] || "http://127.0.0.1:8899";

const programId = kp(progPath).publicKey;
const mintKp = kp(mintPath);
const AMOUNT = 1000n;
const conn = new Connection(solRpc, "confirmed");
const payer = Keypair.generate();
const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, programId)[0];
const assert = (c, m) => { if (!c) throw new Error(m); };
const send = (ix) => sendAndConfirmTransaction(conn, new Transaction()
  .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 })).add(ix), [payer], { commitment: "confirmed" });

// `cast call ... (uint256)` pretty-prints large numbers with a trailing
// `[1.2e76]`-style scientific-notation hint — strip it before BigInt-parsing
// (never appears in tx hashes/addresses, so stripping is always safe here).
const cast = (args) => execFileSync("cast", args, { encoding: "utf8" }).trim().replace(/\s*\[.*\]$/, "");
const evmMint = (pub, proof, fnSig) => execFileSync("node", [
  path.join(HERE, "..", "evm", "mint.mjs"), evmRpc, opaqPoolAddr, opKey, pub, proof, fnSig, "4",
], { encoding: "utf8" }).trim();

async function main() {
  await conn.confirmTransaction(await conn.requestAirdrop(payer.publicKey, 5 * LAMPORTS_PER_SOL), "confirmed");
  const mint = await createMint(conn, payer, payer.publicKey, null, 0, mintKp);
  const depositorAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, payer.publicKey)).address;
  await mintTo(conn, payer, mint, depositorAta, payer, AMOUNT);
  const vaultAuthority = pda([Buffer.from("vault"), mint.toBuffer()]);
  const vaultAta = (await getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true)).address;
  const tree = pda([Buffer.from("tree")]);
  const nullifiers = pda([Buffer.from("nullifiers")]);
  const xpending = pda([Buffer.from("xpending")]);

  // init pool + xburn_pending (operator = payer, self-served single-actor demo)
  await send(new TransactionInstruction({
    programId, data: Buffer.from([0]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  await send(new TransactionInstruction({
    programId, data: Buffer.concat([Buffer.from([5]), payer.publicKey.toBuffer()]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: xpending, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));

  // 1) deposit note A on Solana (-> leaf 0)
  await send(new TransactionInstruction({
    programId, data: fs.readFileSync(depositBin),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: depositorAta, isSigner: false, isWritable: true },
      { pubkey: vaultAta, isSigner: false, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    ],
  }));
  assert((await fetchTree(conn, tree)).nextIndex === 1n, "deposit should land leaf 0");
  console.log("  OK  [Solana] deposit (leaf 0), vault funded");

  // 2) xburn note A on Solana (tag 8, Solana as SOURCE) -> nullifier1 recorded
  await send(new TransactionInstruction({
    programId, data: fs.readFileSync(xburn1SolanaBin),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: false },
      { pubkey: nullifiers, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  console.log("  OK  [Solana] xburn (nullifier1 recorded, value locked for EVM)");

  // --- forward leg: attest + mintFromXburn on OpaqPool (EVM as DESTINATION) ---
  const pub1 = JSON.parse(fs.readFileSync(path.join(prove1Dir, "public.json"), "utf8"));
  const nullifier1 = "0x" + BigInt(pub1[1]).toString(16).padStart(64, "0");

  cast(["send", opaqPoolAddr, "addPending(bytes32)", nullifier1, "--rpc-url", evmRpc, "--private-key", opKey]);
  const mintTx = evmMint(
    path.join(prove1Dir, "public.json"), path.join(prove1Dir, "proof.json"),
    "mintFromXburn(uint256[2],uint256[2][2],uint256[2],uint256[4])",
  );
  assert(mintTx.length > 0, "mintFromXburn tx should return a hash");
  console.log(`  OK  [EVM] mintFromXburn (tx ${mintTx}) — a real note lands on OpaqPool, leaf 0`);

  const nextIndexHex = cast(["call", opaqPoolAddr, "nextIndex()(uint256)", "--rpc-url", evmRpc]);
  assert(BigInt(nextIndexHex) === 1n, `OpaqPool.nextIndex should be 1 after the forward mint, got ${nextIndexHex}`);
  console.log("  OK  [EVM] OpaqPool re-shielded the Solana-origin note (forward: Solana -> EVM)");

  // --- reverse leg: xburn the SAME note back off OpaqPool (EVM as SOURCE) ---
  const pub2 = JSON.parse(fs.readFileSync(path.join(prove2Dir, "public.json"), "utf8"));
  const currentRootIdx = cast(["call", opaqPoolAddr, "currentRootIndex()(uint256)", "--rpc-url", evmRpc]);
  const onChainRoot = cast(["call", opaqPoolAddr, "roots(uint256)(uint256)", currentRootIdx, "--rpc-url", evmRpc]);
  const expectedRoot = BigInt(pub2[0]);
  assert(BigInt(onChainRoot) === expectedRoot,
    `xburn2's src_merkle_root must equal OpaqPool's actual on-chain root (${onChainRoot} != ${expectedRoot})`);
  console.log("  OK  [EVM] xburn2's witness root matches OpaqPool's real on-chain root");

  const xburnTx = evmMint(
    path.join(prove2Dir, "public.json"), path.join(prove2Dir, "proof.json"),
    "xburn(uint256[2],uint256[2][2],uint256[2],uint256[4])",
  );
  assert(xburnTx.length > 0, "xburn tx should return a hash");
  console.log(`  OK  [EVM] xburn (tx ${xburnTx}) — nullifier2 recorded on OpaqPool, note locked for Solana`);

  // --- attest + mint_from_xburn on Solana (tag 7, Solana as DESTINATION again) ---
  const nullifier2 = Buffer.from(BigInt(pub2[1]).toString(16).padStart(64, "0"), "hex");
  await send(new TransactionInstruction({
    programId, data: Buffer.concat([Buffer.from([6]), nullifier2]),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: xpending, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  await send(new TransactionInstruction({
    programId, data: fs.readFileSync(xburn2SolanaBin),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: tree, isSigner: false, isWritable: true },
      { pubkey: xpending, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  }));
  assert((await fetchTree(conn, tree)).nextIndex === 2n, "mint_from_xburn should insert leaf 1 (the round-tripped note)");
  console.log("  OK  [Solana] mint_from_xburn re-shields the EVM-origin note as leaf 1 (reverse: EVM -> Solana)");

  console.log("\nM20 PASSED — Phase 4 symmetric round trip, live: Solana note -> EVM note" +
    " (re-shielded, not a balance) -> back to a Solana note. One proof each leg, no relayer.");
}

main().catch((e) => { console.error("FAILED:", e.message || e); process.exit(1); });

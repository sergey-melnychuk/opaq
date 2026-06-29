// Recipient on-chain history check (M9 / OPAQ.md A.8) as a one-shot CLI: given a
// public RPC URL and an address, print {"count":N,"capped":bool} — N prior
// signatures for that address (capped at one RPC page). `opaq withdraw --rpc`
// spawns this to turn the static A.8 warning into a concrete fresh/not-fresh
// finding. Pure RPC, no indexer.
//
// Usage: node recipient_history.mjs <rpcUrl> <address>
import { Connection, PublicKey } from "@solana/web3.js";

const [rpc, addr] = process.argv.slice(2);
if (!rpc || !addr) {
  console.error("usage: node recipient_history.mjs <rpcUrl> <address>");
  process.exit(2);
}

const LIMIT = 1000; // one getSignaturesForAddress page; enough to answer "fresh?"
try {
  const conn = new Connection(rpc, "confirmed");
  const sigs = await conn.getSignaturesForAddress(new PublicKey(addr), { limit: LIMIT }, "confirmed");
  process.stdout.write(JSON.stringify({ count: sigs.length, capped: sigs.length >= LIMIT }));
} catch (e) {
  console.error(`recipient_history: ${e?.message ?? e}`);
  process.exit(1);
}

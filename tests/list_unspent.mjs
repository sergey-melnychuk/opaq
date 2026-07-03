// Zero-infra read path for B.13 note discovery (OPAQ.md B.13.5), as a
// one-shot CLI: given only a public RPC URL and the program id, print
// everything `opaq list-unspent` needs to find + verify a recipient's
// incoming transfer notes — decryption and verification stay in Rust (chain
// I/O lives here, same split as read_leaves.mjs / harvest_leaves).
//
// Usage: node list_unspent.mjs <rpcUrl> <programId>
// Prints JSON: { leaves: [hex...], memos: [{commitment, memo}...], nullifiers: [hex...] }
import { Connection, PublicKey } from "@solana/web3.js";
import { fetchLeaves, fetchTransferMemos, fetchNullifierSet } from "./read_path.mjs";

const [rpc, programArg] = process.argv.slice(2);
if (!rpc || !programArg) {
  console.error("usage: node list_unspent.mjs <rpcUrl> <programId>");
  process.exit(2);
}

try {
  const conn = new Connection(rpc, "confirmed");
  const programId = new PublicKey(programArg);
  const tree = PublicKey.findProgramAddressSync([Buffer.from("tree")], programId)[0];
  const nullifierSetPda = PublicKey.findProgramAddressSync([Buffer.from("nullifiers")], programId)[0];

  const [leaves, memos, nullifierSet] = await Promise.all([
    fetchLeaves(conn, programId, tree),
    fetchTransferMemos(conn, programId, tree),
    fetchNullifierSet(conn, nullifierSetPda),
  ]);

  process.stdout.write(JSON.stringify({ leaves, memos, nullifiers: [...nullifierSet] }));
} catch (e) {
  console.error(`list_unspent: ${e?.message ?? e}`);
  process.exit(1);
}

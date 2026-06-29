// Zero-infra read path as a one-shot CLI (M9): given only a public RPC URL and
// the program id, print the ordered on-chain commitment (leaf) list as a JSON
// array of hex strings to stdout — index i == on-chain leaf_index i. This is the
// single source of truth for harvesting leaves; `opaq withdraw --rpc` spawns it
// (see crates/prover/src/main.rs harvest_leaves) and the reconstruction stays in
// Rust. RPC logic itself lives in read_path.mjs and is exercised by m10.
//
// Usage: node read_leaves.mjs <rpcUrl> <programId> [treePda]
import { Connection, PublicKey } from "@solana/web3.js";
import { fetchLeaves } from "./read_path.mjs";

const [rpc, programArg, treeArg] = process.argv.slice(2);
if (!rpc || !programArg) {
  console.error("usage: node read_leaves.mjs <rpcUrl> <programId> [treePda]");
  process.exit(2);
}

try {
  const conn = new Connection(rpc, "confirmed");
  const programId = new PublicKey(programArg);
  // The commitment tree is the single PDA seeded by b"tree" (programs/opaq/src/lib.rs).
  const tree = treeArg
    ? new PublicKey(treeArg)
    : PublicKey.findProgramAddressSync([Buffer.from("tree")], programId)[0];
  const leaves = await fetchLeaves(conn, programId, tree);
  process.stdout.write(JSON.stringify(leaves));
} catch (e) {
  console.error(`read_leaves: ${e?.message ?? e}`);
  process.exit(1);
}

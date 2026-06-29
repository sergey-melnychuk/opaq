// Zero-infra read path (OPAQ.md B.7 step 2-3, Test 7 / M10).
//
// Reconstructs everything a withdrawer needs to build a Merkle proof for an
// existing note using ONLY a public RPC endpoint — no custom indexer, no
// special access, no local cache. Two RPC primitives carry the whole thing:
//
//   getAccountInfo(treePda)            -> the live tree frontier + root ring buffer
//   getSignaturesForAddress(treePda)   -> every tx that touched the tree
//   getTransaction(sig)                -> the deposit's commitment (instruction
//                                         data) + leaf_index (program log)
//
// The commitment list this yields, in leaf_index order, is exactly what
// `opaq withdraw --leaves` folds back into an authentication path.
import bs58 from "bs58";

const TREE_DEPTH = 24;
const ROOT_HISTORY = 32;
// CommitmentTree borsh layout (mirrors programs/opaq/src/lib.rs):
//   next_index u64 LE | filled_subtrees[24][32] | roots[32][32] | current_root_index u8
const OFF_ROOTS = 8 + TREE_DEPTH * 32;
const OFF_CRI = OFF_ROOTS + ROOT_HISTORY * 32;
export const TREE_SIZE = OFF_CRI + 1;

const hex = (buf) => Buffer.from(buf).toString("hex");

// Parse the raw CommitmentTree account bytes into the bits the read path needs.
export function parseTreeAccount(data) {
  if (data.length < TREE_SIZE) throw new Error(`tree account too small: ${data.length}`);
  const nextIndex = data.readBigUInt64LE(0);
  const currentRootIndex = data[OFF_CRI];
  const roots = [];
  for (let i = 0; i < ROOT_HISTORY; i++) {
    roots.push(hex(data.subarray(OFF_ROOTS + i * 32, OFF_ROOTS + i * 32 + 32)));
  }
  const ZERO = "00".repeat(32);
  return {
    nextIndex,
    currentRootIndex,
    roots,
    currentRoot: roots[currentRootIndex],
    // The set a withdraw proof's merkle_root may match (the ring buffer, minus
    // the unused all-zero slots).
    knownRoots: new Set(roots.filter((r) => r !== ZERO)),
  };
}

export async function fetchTree(conn, treePda) {
  const info = await conn.getAccountInfo(treePda, "confirmed");
  if (!info) throw new Error("commitment tree account not found (pool not initialized?)");
  return parseTreeAccount(info.data);
}

// Walk getSignaturesForAddress back to genesis (paginated), oldest first.
async function allSignatures(conn, address) {
  const out = [];
  let before;
  for (;;) {
    const page = await conn.getSignaturesForAddress(address, { before, limit: 1000 }, "confirmed");
    if (page.length === 0) break;
    out.push(...page);
    before = page[page.length - 1].signature;
    if (page.length < 1000) break;
  }
  return out.reverse(); // chronological
}

// Pull the (leaf_index -> commitment) pairs out of one transaction. Two kinds of
// opaq instruction insert leaves: a deposit (`tag=1`, 329 bytes, final 32 = the
// commitment, logs `deposit ok, leaf_index=N`) and a transfer (`tag=3`, 417 bytes,
// final 64 = the 2 output commitments, logs `transfer ok, leaves=N,M`). Both must
// be harvested or notes created by a transfer (e.g. change) can't be withdrawn.
function depositsFromTx(tx, programId) {
  if (!tx || tx.meta?.err) return [];
  const msg = tx.transaction.message;
  const keys = msg.accountKeys ?? msg.staticAccountKeys;
  const pid = programId.toBase58();

  const commitments = [];
  for (const ix of msg.instructions ?? msg.compiledInstructions ?? []) {
    // Legacy compiled instruction: { programIdIndex, data(base58) }.
    const owner = keys[ix.programIdIndex];
    if ((owner.toBase58 ? owner.toBase58() : owner.pubkey?.toBase58()) !== pid) continue;
    const raw = typeof ix.data === "string" ? Buffer.from(bs58.decode(ix.data)) : Buffer.from(ix.data);
    if (raw.length === 329 && raw[0] === 1) {
      commitments.push(hex(raw.subarray(raw.length - 32)));
    } else if (raw.length === 417 && raw[0] === 3) {
      commitments.push(hex(raw.subarray(raw.length - 64, raw.length - 32))); // out_commitment0
      commitments.push(hex(raw.subarray(raw.length - 32))); // out_commitment1
    }
  }
  if (commitments.length === 0) return [];

  const indices = [];
  for (const line of tx.meta?.logMessages ?? []) {
    const d = line.match(/opaq: deposit ok, leaf_index=(\d+)/);
    if (d) { indices.push(Number(d[1])); continue; }
    const t = line.match(/opaq: transfer ok, leaves=(\d+),(\d+)/);
    if (t) indices.push(Number(t[1]), Number(t[2]));
  }
  // Pair in order; fall back to positional if the program ever stops logging.
  return commitments.map((commitment, i) => ({
    leafIndex: indices[i],
    commitment,
  }));
}

// The full ordered commitment (leaf) list, harvested purely over RPC.
export async function fetchLeaves(conn, programId, treePda) {
  const sigs = await allSignatures(conn, treePda);
  const pairs = [];
  for (const { signature } of sigs) {
    const tx = await conn.getTransaction(signature, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });
    pairs.push(...depositsFromTx(tx, programId));
  }
  // Order by the authoritative on-chain leaf_index when available.
  const haveIdx = pairs.every((p) => Number.isInteger(p.leafIndex));
  if (haveIdx) pairs.sort((a, b) => a.leafIndex - b.leafIndex);
  return pairs.map((p) => p.commitment);
}

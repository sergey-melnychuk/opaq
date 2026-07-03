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

// Fixed byte offsets inside a transfer instruction's own data (tag=3):
//   tag(1) proof_a(64) proof_b(128) proof_c(64) merkle_root(32)
//   nullifier0(32) nullifier1(32) out_commitment0(32) out_commitment1(32)
// = 417 bytes fixed prefix. OPAQ.md B.13.4: a transfer MAY carry a trailing,
// optional encrypted memo after this prefix (opaq transfer --to-view), so
// callers must match on `raw.length >= 417`, NOT `=== 417`, and slice
// commitments from these FIXED absolute offsets — never from `raw.length`,
// which shifts once a memo is attached.
const TRANSFER_FIXED_LEN = 417;
const OFF_OUT_COMMITMENT0 = 353;
const OFF_OUT_COMMITMENT1 = 385;

// Pull the (leaf_index -> commitment) pairs out of one transaction. Two kinds of
// opaq instruction insert leaves: a deposit (`tag=1`, 329 bytes, final 32 = the
// commitment, logs `deposit ok, leaf_index=N`) and a transfer (`tag=3`, >= 417
// bytes, out_commitment0/1 at fixed offsets, logs `transfer ok, leaves=N,M`).
// Both must be harvested or notes created by a transfer (e.g. change) can't be
// withdrawn.
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
    } else if (raw.length >= TRANSFER_FIXED_LEN && raw[0] === 3) {
      commitments.push(hex(raw.subarray(OFF_OUT_COMMITMENT0, OFF_OUT_COMMITMENT0 + 32))); // out_commitment0
      commitments.push(hex(raw.subarray(OFF_OUT_COMMITMENT1, OFF_OUT_COMMITMENT1 + 32))); // out_commitment1
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

// B.13.5: pull out0's trailing memo (if any) from a transfer instruction —
// the encrypted note-opening a sender may attach for out_commitment0's owner
// (`opaq transfer --to-view`). out1 (change-to-self) never carries one.
function transferMemoFromTx(tx, programId) {
  if (!tx || tx.meta?.err) return [];
  const msg = tx.transaction.message;
  const keys = msg.accountKeys ?? msg.staticAccountKeys;
  const pid = programId.toBase58();

  const out = [];
  for (const ix of msg.instructions ?? msg.compiledInstructions ?? []) {
    const owner = keys[ix.programIdIndex];
    if ((owner.toBase58 ? owner.toBase58() : owner.pubkey?.toBase58()) !== pid) continue;
    const raw = typeof ix.data === "string" ? Buffer.from(bs58.decode(ix.data)) : Buffer.from(ix.data);
    if (raw.length > TRANSFER_FIXED_LEN && raw[0] === 3) {
      out.push({
        commitment: hex(raw.subarray(OFF_OUT_COMMITMENT0, OFF_OUT_COMMITMENT0 + 32)),
        memo: hex(raw.subarray(TRANSFER_FIXED_LEN)),
      });
    }
  }
  return out;
}

// The full set of (out_commitment0 -> memo) pairs across every transfer that
// ever carried one, harvested purely over RPC (same walk as fetchLeaves).
export async function fetchTransferMemos(conn, programId, treePda) {
  const sigs = await allSignatures(conn, treePda);
  const out = [];
  for (const { signature } of sigs) {
    const tx = await conn.getTransaction(signature, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });
    out.push(...transferMemoFromTx(tx, programId));
  }
  return out;
}

// NullifierSet borsh layout (mirrors programs/opaq/src/lib.rs):
//   count u64 LE | nullifiers: Vec<[u8;32]> (u32 LE length prefix + 32B each)
export function parseNullifierSet(data) {
  const len = data.readUInt32LE(8);
  const set = new Set();
  for (let i = 0; i < len; i++) {
    const off = 12 + i * 32;
    set.add(hex(data.subarray(off, off + 32)));
  }
  return set;
}

export async function fetchNullifierSet(conn, nullifierPda) {
  const info = await conn.getAccountInfo(nullifierPda, "confirmed");
  if (!info) throw new Error("nullifier set account not found (pool not initialized?)");
  return parseNullifierSet(info.data);
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

//! M4: incremental Poseidon Merkle tree (OPAQ.md B.2), in the Tornado
//! `MerkleTreeWithHistory` style. Pure logic, generic over the 2-input hash so
//! the same code runs in host tests (light-poseidon) and on-chain
//! (`sol_poseidon` syscall) — both proven byte-identical in M0.
//!
//! Insert is O(depth): it folds the new leaf up the frontier using the
//! precomputed empty-subtree hashes (`zero_hashes`) for the still-empty side,
//! and records the new root in a ring buffer of recent roots.

pub const TREE_DEPTH: usize = 24;
pub const ROOT_HISTORY: usize = 32;
/// Sentinel value for an empty leaf (field 0). Matches the circuit, which does
/// not special-case empty positions.
pub const ZERO_LEAF: [u8; 32] = [0u8; 32];

#[derive(Clone)]
pub struct CommitmentTree {
    pub next_index: u64,
    pub filled_subtrees: [[u8; 32]; TREE_DEPTH],
    pub roots: [[u8; 32]; ROOT_HISTORY],
    pub current_root_index: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TreeError {
    Full,
}

/// Empty-subtree hash at each level: `zeros[0]` is the empty leaf, `zeros[i] =
/// H(zeros[i-1], zeros[i-1])`. On-chain these are precomputed constants.
pub fn zero_hashes<H>(hash: &H) -> [[u8; 32]; TREE_DEPTH]
where
    H: Fn(&[u8; 32], &[u8; 32]) -> [u8; 32],
{
    let mut z = [[0u8; 32]; TREE_DEPTH];
    z[0] = ZERO_LEAF;
    for i in 1..TREE_DEPTH {
        z[i] = hash(&z[i - 1], &z[i - 1]);
    }
    z
}

/// Root of the all-empty tree (one level above the tallest empty subtree).
pub fn empty_root<H>(hash: &H, zeros: &[[u8; 32]; TREE_DEPTH]) -> [u8; 32]
where
    H: Fn(&[u8; 32], &[u8; 32]) -> [u8; 32],
{
    hash(&zeros[TREE_DEPTH - 1], &zeros[TREE_DEPTH - 1])
}

impl CommitmentTree {
    pub fn new<H>(hash: &H) -> Self
    where
        H: Fn(&[u8; 32], &[u8; 32]) -> [u8; 32],
    {
        let zeros = zero_hashes(hash);
        let mut roots = [[0u8; 32]; ROOT_HISTORY];
        roots[0] = empty_root(hash, &zeros);
        Self {
            next_index: 0,
            filled_subtrees: zeros,
            roots,
            current_root_index: 0,
        }
    }

    /// Insert `leaf` at the next free slot. Returns its leaf index. `zeros` must
    /// be `zero_hashes(hash)` (passed in so on-chain it's a constant, not
    /// recomputed every insert).
    pub fn insert<H>(
        &mut self,
        hash: &H,
        zeros: &[[u8; 32]; TREE_DEPTH],
        leaf: [u8; 32],
    ) -> Result<u64, TreeError>
    where
        H: Fn(&[u8; 32], &[u8; 32]) -> [u8; 32],
    {
        if self.next_index >= (1u64 << TREE_DEPTH) {
            return Err(TreeError::Full);
        }
        let leaf_index = self.next_index;
        let mut index = self.next_index;
        let mut current = leaf;
        for i in 0..TREE_DEPTH {
            let (left, right) = if index & 1 == 0 {
                // current is the left child; its right sibling is still empty.
                self.filled_subtrees[i] = current;
                (current, zeros[i])
            } else {
                // current is the right child; left sibling is the saved frontier.
                (self.filled_subtrees[i], current)
            };
            current = hash(&left, &right);
            index >>= 1;
        }
        self.current_root_index =
            ((self.current_root_index as usize + 1) % ROOT_HISTORY) as u8;
        self.roots[self.current_root_index as usize] = current;
        self.next_index += 1;
        Ok(leaf_index)
    }

    pub fn current_root(&self) -> [u8; 32] {
        self.roots[self.current_root_index as usize]
    }

    /// All roots currently retained in the ring buffer (ignoring the empty slot)
    /// — the set a withdraw proof's `merkle_root` is allowed to match (B.2).
    pub fn known_roots(&self) -> Vec<[u8; 32]> {
        self.roots.iter().copied().filter(|r| *r != [0u8; 32]).collect()
    }

    /// Whether `root` is in the recent-root ring buffer (proofs may be built
    /// against a root that has since moved). Ignores the all-zero slot.
    pub fn is_known_root(&self, root: &[u8; 32]) -> bool {
        *root != [0u8; 32] && self.roots.iter().any(|r| r == root)
    }
}

/// The Merkle authentication path for the leaf at `index`, reconstructed from
/// the full ordered list of inserted `leaves` (M10 / Test 7's zero-infra read
/// path). The on-chain `CommitmentTree` account only retains the rightmost
/// frontier (`filled_subtrees`) + recent roots, never the leaves — so a
/// withdrawer rebuilds the tree from the deposit history harvested over plain
/// RPC (`Deposited` logs + instruction data), with no indexer.
///
/// Returns `(siblings, right, root)` in the exact shape `merkle_root_be` and the
/// withdraw circuit consume: `right[i] == true` means the running node is the
/// right child at level `i`. Folding `leaves[index]` up `(siblings, right)`
/// reproduces `root`, which equals the incremental tree's current root once all
/// `leaves` are inserted (so it lands in the on-chain root ring buffer).
///
/// Positions past the end of `leaves` are treated as empty (`zeros[level]`),
/// matching how the incremental tree leaves unfilled slots.
pub fn reconstruct_path<H>(
    hash: &H,
    zeros: &[[u8; 32]; TREE_DEPTH],
    leaves: &[[u8; 32]],
    index: u64,
) -> ([[u8; 32]; TREE_DEPTH], [bool; TREE_DEPTH], [u8; 32])
where
    H: Fn(&[u8; 32], &[u8; 32]) -> [u8; 32],
{
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    let mut idx = index as usize;
    let mut siblings = [[0u8; 32]; TREE_DEPTH];
    let mut right = [false; TREE_DEPTH];

    for i in 0..TREE_DEPTH {
        right[i] = idx & 1 == 1;
        let sib_idx = idx ^ 1;
        // The sibling is an empty subtree if it sits past the filled prefix.
        siblings[i] = level.get(sib_idx).copied().unwrap_or(zeros[i]);

        // Fold this level into the next, using zeros[i] for a missing right child.
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut j = 0;
        while j < level.len() {
            let l = level[j];
            let r = level.get(j + 1).copied().unwrap_or(zeros[i]);
            next.push(hash(&l, &r));
            j += 2;
        }
        if next.is_empty() {
            // Empty tree: the parent of two empty subtrees is the next zero hash.
            next.push(hash(&zeros[i], &zeros[i]));
        }
        level = next;
        idx >>= 1;
    }

    (siblings, right, level[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{be32, merkle_root_be, poseidon_hash2_be};

    fn h(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
        poseidon_hash2_be(a, b)
    }

    #[test]
    fn empty_root_is_known_and_consistent() {
        let zeros = zero_hashes(&h);
        let t = CommitmentTree::new(&h);
        assert_eq!(t.current_root(), empty_root(&h, &zeros));
        assert!(t.is_known_root(&t.current_root()));
        assert_eq!(t.next_index, 0);
    }

    #[test]
    fn insert_index0_matches_direct_fold() {
        let zeros = zero_hashes(&h);
        let mut t = CommitmentTree::new(&h);
        let leaf = be32(42);
        assert_eq!(t.insert(&h, &zeros, leaf).unwrap(), 0);
        // index 0 = all-left path with empty-subtree siblings.
        let expected = merkle_root_be(leaf, &zeros, &[false; TREE_DEPTH]);
        assert_eq!(t.current_root(), expected);
    }

    #[test]
    fn insert_index1_matches_direct_fold() {
        let zeros = zero_hashes(&h);
        let mut t = CommitmentTree::new(&h);
        let (leaf0, leaf1) = (be32(42), be32(99));
        t.insert(&h, &zeros, leaf0).unwrap();
        assert_eq!(t.insert(&h, &zeros, leaf1).unwrap(), 1);
        // index 1: right child at level 0 with sibling leaf0, empty above.
        let mut siblings = zeros;
        siblings[0] = leaf0;
        let mut right = [false; TREE_DEPTH];
        right[0] = true;
        assert_eq!(t.current_root(), merkle_root_be(leaf1, &siblings, &right));
    }

    #[test]
    fn reconstructed_path_folds_to_the_live_tree_root() {
        // M10 / Test 7: rebuild each leaf's authentication path from ONLY the
        // ordered leaf list (what the zero-infra RPC read path harvests) and
        // assert it folds to the tree's actual current root — i.e. a root the
        // on-chain ring buffer recognizes. This is the read-path correctness
        // contract a withdrawer relies on.
        let zeros = zero_hashes(&h);
        let mut t = CommitmentTree::new(&h);
        let leaves: Vec<[u8; 32]> = (0..7u128).map(|i| be32(100 + i)).collect();
        for leaf in &leaves {
            t.insert(&h, &zeros, *leaf).unwrap();
        }
        let root = t.current_root();
        for (i, leaf) in leaves.iter().enumerate() {
            let (siblings, right, recon_root) =
                reconstruct_path(&h, &zeros, &leaves, i as u64);
            assert_eq!(recon_root, root, "reconstructed root mismatch at leaf {i}");
            assert_eq!(
                merkle_root_be(*leaf, &siblings, &right),
                root,
                "folding leaf {i} up its path must reach the live root"
            );
            assert!(t.is_known_root(&recon_root));
        }
    }

    #[test]
    fn reconstructed_path_for_lone_leaf_matches_empty_frontier() {
        // Single deposit: the path is all empty-subtree siblings, all-left.
        let zeros = zero_hashes(&h);
        let mut t = CommitmentTree::new(&h);
        let leaf = be32(42);
        t.insert(&h, &zeros, leaf).unwrap();
        let (siblings, right, recon_root) = reconstruct_path(&h, &zeros, &[leaf], 0);
        assert_eq!(siblings, zeros);
        assert_eq!(right, [false; TREE_DEPTH]);
        assert_eq!(recon_root, t.current_root());
    }

    #[test]
    fn root_ring_buffer_evicts_old_roots() {
        let zeros = zero_hashes(&h);
        let mut t = CommitmentTree::new(&h);
        t.insert(&h, &zeros, be32(1)).unwrap();
        let first_root = t.current_root();
        assert!(t.is_known_root(&first_root));
        // Overflow the ring buffer; the first root must be evicted.
        for i in 0..(ROOT_HISTORY as u128 + 1) {
            t.insert(&h, &zeros, be32(1000 + i)).unwrap();
        }
        assert!(!t.is_known_root(&first_root), "old root should be evicted");
        assert!(t.is_known_root(&t.current_root()));
    }
}

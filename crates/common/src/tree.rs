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

    /// Whether `root` is in the recent-root ring buffer (proofs may be built
    /// against a root that has since moved). Ignores the all-zero slot.
    pub fn is_known_root(&self, root: &[u8; 32]) -> bool {
        *root != [0u8; 32] && self.roots.iter().any(|r| r == root)
    }
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

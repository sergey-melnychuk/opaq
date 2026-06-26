//! M5: nullifier set (OPAQ.md B.2) — append-only, unsorted, linear-scan
//! membership. Accepted O(n) cost for Phase 1; a sorted/hashtable layout is the
//! documented Phase 1.5 optimization. Pure logic, unit-tested in isolation.

#[derive(Clone, Default)]
pub struct NullifierSet {
    pub nullifiers: Vec<[u8; 32]>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum NullifierError {
    /// The nullifier is already recorded — double-spend attempt.
    AlreadySpent,
}

impl NullifierSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, n: &[u8; 32]) -> bool {
        self.nullifiers.iter().any(|x| x == n)
    }

    /// Record a nullifier as spent. Rejects a duplicate (double-spend).
    pub fn try_insert(&mut self, n: [u8; 32]) -> Result<(), NullifierError> {
        if self.contains(&n) {
            return Err(NullifierError::AlreadySpent);
        }
        self.nullifiers.push(n);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.nullifiers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nullifiers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::be32;

    #[test]
    fn insert_new_then_reject_duplicate() {
        let mut set = NullifierSet::new();
        let n = be32(7);
        assert!(!set.contains(&n));
        assert_eq!(set.try_insert(n), Ok(()));
        assert!(set.contains(&n));
        assert_eq!(set.len(), 1);
        // second spend of the same nullifier is rejected, set unchanged
        assert_eq!(set.try_insert(n), Err(NullifierError::AlreadySpent));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn distinct_nullifiers_coexist() {
        let mut set = NullifierSet::new();
        for i in 0..100u128 {
            assert_eq!(set.try_insert(be32(i)), Ok(()));
        }
        assert_eq!(set.len(), 100);
        assert!(set.contains(&be32(50)));
        assert!(!set.contains(&be32(100)));
    }
}

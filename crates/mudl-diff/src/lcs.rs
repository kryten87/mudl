//! A small, generic longest-common-subsequence diff, shared by
//! [`crate::word::diff`] and [`crate::line::diff`] (and, later,
//! [`crate::block::match_blocks`]) — the "standard, well-known algorithm,
//! hand-roll it" exception noted in the implementation plan's dependency
//! table (§3), rather than a `similar`/`diff` crate dependency.
//!
//! Not part of the plan's public step list: it's the shared engine behind
//! several steps' `diff`/`match_*` functions, not a deliverable of its own.

use std::collections::HashSet;

/// Classifies every index of `old`/`new` as removed/inserted by an LCS
/// alignment. Indices absent from both returned sets are the unchanged
/// anchors, in order.
pub fn diff_indices<T: PartialEq>(old: &[T], new: &[T]) -> (HashSet<usize>, HashSet<usize>) {
    let n = old.len();
    let m = new.len();

    // dp[i][j] = length of the LCS of old[i..] and new[j..].
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if old[i] == new[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut removed_old = HashSet::new();
    let mut inserted_new = HashSet::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i] == new[j] {
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            removed_old.insert(i);
            i += 1;
        } else {
            inserted_new.insert(j);
            j += 1;
        }
    }
    while i < n {
        removed_old.insert(i);
        i += 1;
    }
    while j < m {
        inserted_new.insert(j);
        j += 1;
    }

    (removed_old, inserted_new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_sequences_have_no_changes() {
        let (removed, inserted) = diff_indices(&["a", "b", "c"], &["a", "b", "c"]);
        assert!(removed.is_empty());
        assert!(inserted.is_empty());
    }

    #[test]
    fn both_empty() {
        let (removed, inserted) = diff_indices::<&str>(&[], &[]);
        assert!(removed.is_empty());
        assert!(inserted.is_empty());
    }

    #[test]
    fn all_removed() {
        let (removed, inserted) = diff_indices(&["a", "b"], &[]);
        assert_eq!(removed, HashSet::from([0, 1]));
        assert!(inserted.is_empty());
    }

    #[test]
    fn all_inserted() {
        let (removed, inserted) = diff_indices::<&str>(&[], &["a", "b"]);
        assert!(removed.is_empty());
        assert_eq!(inserted, HashSet::from([0, 1]));
    }

    #[test]
    fn single_substitution() {
        let (removed, inserted) = diff_indices(&["a", "b", "c"], &["a", "x", "c"]);
        assert_eq!(removed, HashSet::from([1]));
        assert_eq!(inserted, HashSet::from([1]));
    }

    #[test]
    fn insertion_in_middle() {
        let (removed, inserted) = diff_indices(&["a", "c"], &["a", "b", "c"]);
        assert!(removed.is_empty());
        assert_eq!(inserted, HashSet::from([1]));
    }

    #[test]
    fn deletion_from_middle() {
        let (removed, inserted) = diff_indices(&["a", "b", "c"], &["a", "c"]);
        assert_eq!(removed, HashSet::from([1]));
        assert!(inserted.is_empty());
    }

    #[test]
    fn repeated_elements_align_positionally() {
        // "a a b" -> "a b a": the LCS is "a b" (length 2); one "a" moves.
        let (removed, inserted) = diff_indices(&["a", "a", "b"], &["a", "b", "a"]);
        assert_eq!(removed.len(), 1);
        assert_eq!(inserted.len(), 1);
    }
}

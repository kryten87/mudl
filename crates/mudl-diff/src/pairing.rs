//! Greedy line pairing by word overlap (`WordPairing.swift`; plan §7.3).
//!
//! Within a gap of deleted and inserted lines, word-level diffs are only
//! useful between lines that are actually related, so lines are paired by
//! how many distinct words they share rather than by position.

use std::cmp::Reverse;
use std::collections::HashSet;

/// Returns `(deleted_index, inserted_index)` pairs chosen by greedy
/// best-match on shared word count. Indices are offsets into the given
/// slices, not document line numbers. At most `min(deleted, inserted)` pairs
/// are returned; lines with no overlap still pair if they are left over.
///
/// **Tie-break (deviation from `mud`):** candidates with equal scores are
/// taken lowest deleted index first, then lowest inserted index (a stable
/// sort over generation order). Swift sorts with an unstable sort, so its
/// choice among ties is unspecified; a fixed rule keeps change rendering
/// reproducible across runs.
pub fn best_pairs<S: AsRef<str>>(deleted: &[S], inserted: &[S]) -> Vec<(usize, usize)> {
    let pair_count = deleted.len().min(inserted.len());
    if pair_count == 0 {
        return Vec::new();
    }
    if deleted.len() == 1 && inserted.len() == 1 {
        return vec![(0, 0)];
    }

    let del_words: Vec<HashSet<&str>> = deleted.iter().map(|s| words(s.as_ref())).collect();
    let ins_words: Vec<HashSet<&str>> = inserted.iter().map(|s| words(s.as_ref())).collect();

    let mut candidates = Vec::with_capacity(deleted.len() * inserted.len());
    for (d, dw) in del_words.iter().enumerate() {
        for (i, iw) in ins_words.iter().enumerate() {
            candidates.push((d, i, dw.intersection(iw).count()));
        }
    }
    candidates.sort_by_key(|&(_, _, score)| Reverse(score));

    let mut used_del = vec![false; deleted.len()];
    let mut used_ins = vec![false; inserted.len()];
    let mut pairs = Vec::with_capacity(pair_count);
    for (d, i, _) in candidates {
        if pairs.len() >= pair_count {
            break;
        }
        if used_del[d] || used_ins[i] {
            continue;
        }
        pairs.push((d, i));
        used_del[d] = true;
        used_ins[i] = true;
    }
    pairs
}

/// The distinct whitespace-separated words of `text`.
fn words(text: &str) -> HashSet<&str> {
    text.split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ported from `WordPairingTests.swift`.
    mod best_pairs_tests {
        use super::*;

        const NONE: [&str; 0] = [];

        #[test]
        fn single_pair() {
            assert_eq!(best_pairs(&["old line"], &["new line"]), vec![(0, 0)]);
        }

        #[test]
        fn empty_inputs() {
            assert!(best_pairs(&NONE, &NONE).is_empty());
            assert!(best_pairs(&["a"], &NONE).is_empty());
            assert!(best_pairs(&NONE, &["a"]).is_empty());
        }

        #[test]
        fn asymmetric_gap_picks_best_match() {
            let dels = [
                "completely unrelated content here",
                "another different line of text",
                "the quick brown fox jumps",
            ];
            assert_eq!(
                best_pairs(&dels, &["the quick red fox jumps"]),
                vec![(2, 0)]
            );
        }

        #[test]
        fn two_insertions_match_correct_deletions() {
            let dels = ["alpha beta gamma", "one two three"];
            let ins = ["one two four", "alpha beta delta"];
            let mut pairs = best_pairs(&dels, &ins);
            pairs.sort_by_key(|&(_, i)| i);
            assert_eq!(pairs, vec![(1, 0), (0, 1)]);
        }

        #[test]
        fn no_overlap_still_produces_pairs() {
            assert_eq!(best_pairs(&["aaa bbb", "ccc ddd"], &["xxx yyy"]).len(), 1);
        }

        #[test]
        fn greedy_does_not_double_assign() {
            let dels = ["the shared words here", "nothing in common"];
            let ins = ["the shared words there", "the shared words everywhere"];
            let pairs = best_pairs(&dels, &ins);
            assert_eq!(pairs.len(), 2);
            let d: HashSet<usize> = pairs.iter().map(|p| p.0).collect();
            let i: HashSet<usize> = pairs.iter().map(|p| p.1).collect();
            assert_eq!((d.len(), i.len()), (2, 2));
        }

        #[test]
        fn ties_break_by_lowest_deleted_then_inserted_index() {
            assert_eq!(
                best_pairs(&["aaa", "bbb"], &["xxx", "yyy"]),
                vec![(0, 0), (1, 1)]
            );
            // Both insertions score 3 against deletion 0; insertion 0 wins.
            let dels = ["the shared words here", "nothing in common"];
            let ins = ["the shared words there", "the shared words everywhere"];
            assert_eq!(best_pairs(&dels, &ins), vec![(0, 0), (1, 1)]);
        }

        #[test]
        fn duplicate_words_count_once() {
            // "a a a" shares one distinct word with "a b", and two with "a c a c".
            let pairs = best_pairs(&["a c"], &["a a a", "a c a c"]);
            assert_eq!(pairs, vec![(0, 1)]);
        }
    }
}

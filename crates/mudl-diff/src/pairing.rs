//! Greedy line pairing by word overlap (ported from `mud`'s
//! `Core/Sources/Diff/WordPairing.swift`, Phase 13.3).
//!
//! Pairs deleted and inserted lines by shared-word-set intersection count
//! rather than positional order, so word-level diffs (`crate::word::diff`)
//! compare the most similar lines within a gap.

use std::collections::HashSet;

/// Returns `(del_index, ins_index)` pairs chosen by greedy best-match on
/// shared word count. Indices are offsets into `deleted`/`inserted`, not
/// document line numbers.
///
/// Ties are broken deterministically by candidate generation order: lowest
/// `del_index` first, then lowest `ins_index`.
pub fn best_pairs(deleted: &[&str], inserted: &[&str]) -> Vec<(usize, usize)> {
    let pair_count = deleted.len().min(inserted.len());
    if pair_count == 0 {
        return Vec::new();
    }

    if deleted.len() == 1 && inserted.len() == 1 {
        return vec![(0, 0)];
    }

    let del_words: Vec<HashSet<&str>> = deleted.iter().map(|l| words_in(l)).collect();
    let ins_words: Vec<HashSet<&str>> = inserted.iter().map(|l| words_in(l)).collect();

    let mut candidates: Vec<(usize, usize, usize)> =
        Vec::with_capacity(deleted.len() * inserted.len());
    for (d, dw) in del_words.iter().enumerate() {
        for (i, iw) in ins_words.iter().enumerate() {
            let score = dw.intersection(iw).count();
            candidates.push((d, i, score));
        }
    }

    // Stable sort by descending score preserves the (d, i) generation
    // order among ties, giving the documented tie-break rule.
    candidates.sort_by_key(|c| std::cmp::Reverse(c.2));

    let mut used_del = HashSet::new();
    let mut used_ins = HashSet::new();
    let mut pairs = Vec::with_capacity(pair_count);

    for &(d, i, _) in &candidates {
        if pairs.len() >= pair_count {
            break;
        }
        if used_del.contains(&d) || used_ins.contains(&i) {
            continue;
        }
        pairs.push((d, i));
        used_del.insert(d);
        used_ins.insert(i);
    }

    pairs
}

fn words_in(text: &str) -> HashSet<&str> {
    text.split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_pair() {
        let pairs = best_pairs(&["old line"], &["new line"]);
        assert_eq!(pairs, vec![(0, 0)]);
    }

    #[test]
    fn empty_inputs() {
        assert!(best_pairs(&[], &[]).is_empty());
        assert!(best_pairs(&["a"], &[]).is_empty());
        assert!(best_pairs(&[], &["a"]).is_empty());
    }

    #[test]
    fn asymmetric_gap_picks_best_match() {
        let dels = [
            "completely unrelated content here",
            "another different line of text",
            "the quick brown fox jumps",
        ];
        let ins = ["the quick red fox jumps"];
        let pairs = best_pairs(&dels, &ins);
        assert_eq!(pairs, vec![(2, 0)]);
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
        let pairs = best_pairs(&["aaa bbb", "ccc ddd"], &["xxx yyy"]);
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn greedy_does_not_double_assign() {
        let dels = ["the shared words here", "nothing in common"];
        let ins = ["the shared words there", "the shared words everywhere"];
        let pairs = best_pairs(&dels, &ins);
        assert_eq!(pairs.len(), 2);
        let del_set: HashSet<usize> = pairs.iter().map(|&(d, _)| d).collect();
        let ins_set: HashSet<usize> = pairs.iter().map(|&(_, i)| i).collect();
        assert_eq!(del_set.len(), 2);
        assert_eq!(ins_set.len(), 2);
    }

    #[test]
    fn tie_break_prefers_lowest_indices_in_generation_order() {
        // All four pairings score 0 (no shared words) -- deterministic
        // tie-break should pick (0, 0) and (1, 1), not a shuffled match.
        let dels = ["aaa", "bbb"];
        let ins = ["xxx", "yyy"];
        let pairs = best_pairs(&dels, &ins);
        assert_eq!(pairs, vec![(0, 0), (1, 1)]);
    }
}

//! Myers O((N+M)·D) sequence diff (`_working/DIFF-IMPLEMENTATION.md` §7.1).
//!
//! Every diff in this crate — words, lines, leaf blocks — runs through here.
//! `mud` gets the same service from Swift's `CollectionDifference`
//! (`new.difference(from: old)`), which is the classic greedy forward Myers
//! with a full trace and a backtrack. This module reproduces that algorithm
//! decision for decision, including its tie-break (on equal furthest-reaching
//! paths, prefer the deletion), so that when a sequence has several minimal
//! edit scripts both ports pick the same anchors — and therefore mint the
//! same change IDs.
//!
//! Output is always ordered the way `mud`'s consumers expect: within each gap
//! between unchanged anchors, every deletion comes before every insertion.
//!
//! Two deliberate differences from a textbook implementation:
//!
//! - **Common-prefix trim.** The prefix is skipped before the main loop. This
//!   is output-identical to running Myers on the whole input (the greedy
//!   d = 0 snake consumes exactly the common prefix), and it keeps the trace
//!   small for the common "one edit somewhere" reload.
//! - **No common-suffix trim.** Plan §7.1 suggested trimming both ends, but
//!   trimming the suffix first changes which anchors are chosen when the
//!   suffix element also occurs earlier (e.g. old `[c, a, b, a]` → new `[a]`:
//!   Swift keeps the first `a`, a suffix trim would keep the last), and it
//!   buys nothing asymptotically — a common suffix is just one final snake.
//!   Parity of change IDs with `mud` wins.
//!
//! **Memory guard.** The trace costs O(D²) cells. When it would exceed
//! [`MAX_TRACE_CELLS`] (a near-total rewrite of a very large sequence), the
//! diff falls back to "everything between the common prefix and suffix was
//! replaced": still a correct edit script, just not a minimal one. Plan §12
//! asks that a reload never blocks for seconds; this is that guard.

use std::ops::Range;

/// The largest Myers trace (in cells, ~D²) computed before falling back to a
/// whole-middle replacement. 4M cells is 32 MB on 64-bit, reached at an edit
/// distance of about 2,000.
pub const MAX_TRACE_CELLS: usize = 4_000_000;

/// One step of an edit script, indexing into the `old` and `new` inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edit {
    /// `old[old]` and `new[new]` are equal and matched (an anchor).
    Unchanged { old: usize, new: usize },
    /// `old[i]` was removed.
    Deleted(usize),
    /// `new[i]` was inserted.
    Inserted(usize),
}

/// A gap (possibly empty) followed by the anchor that closes it.
///
/// Within a gap every old index is deleted and every new index inserted, so
/// both sides are contiguous ranges. Every consumer in this crate walks
/// gaps, which is why this shape exists alongside [`Edit`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Segment {
    /// Old indices deleted in this gap.
    pub deleted: Range<usize>,
    /// New indices inserted in this gap.
    pub inserted: Range<usize>,
    /// The unchanged `(old, new)` pair after the gap; `None` for the final
    /// segment, which runs to the end of both inputs.
    pub anchor: Option<(usize, usize)>,
}

/// Diffs `old` against `new` and returns an edit script in document order:
/// in each gap, deletions (ascending) then insertions (ascending), then the
/// anchor.
pub fn diff<T: Eq>(old: &[T], new: &[T]) -> Vec<Edit> {
    let mut edits = Vec::with_capacity(old.len().max(new.len()));
    for seg in segments(old, new) {
        edits.extend(seg.deleted.map(Edit::Deleted));
        edits.extend(seg.inserted.map(Edit::Inserted));
        if let Some((o, n)) = seg.anchor {
            edits.push(Edit::Unchanged { old: o, new: n });
        }
    }
    edits
}

/// Diffs `old` against `new` and returns the gaps between anchors.
///
/// There is always exactly one segment per anchor plus one final segment
/// (`anchor: None`), so empty gaps are included. Identical inputs produce
/// only empty gaps.
pub fn segments<T: Eq>(old: &[T], new: &[T]) -> Vec<Segment> {
    segments_with_limit(old, new, MAX_TRACE_CELLS)
}

fn segments_with_limit<T: Eq>(old: &[T], new: &[T], limit: usize) -> Vec<Segment> {
    let (removed, inserted) = classify(old, new, limit);

    // Pair the k-th surviving old element with the k-th surviving new one.
    // This is `mud`'s anchor walk; the gap between two anchors is exactly
    // the removed/inserted run on each side.
    let mut segs = Vec::new();
    let (mut oi, mut ni) = (0usize, 0usize);
    let (mut gap_old, mut gap_new) = (0usize, 0usize);
    loop {
        while oi < old.len() && removed[oi] {
            oi += 1;
        }
        while ni < new.len() && inserted[ni] {
            ni += 1;
        }
        if oi < old.len() && ni < new.len() {
            segs.push(Segment {
                deleted: gap_old..oi,
                inserted: gap_new..ni,
                anchor: Some((oi, ni)),
            });
            oi += 1;
            ni += 1;
            gap_old = oi;
            gap_new = ni;
        } else {
            debug_assert!(oi == old.len() && ni == new.len());
            segs.push(Segment {
                deleted: gap_old..old.len(),
                inserted: gap_new..new.len(),
                anchor: None,
            });
            return segs;
        }
    }
}

/// Marks every removed old index and inserted new index.
fn classify<T: Eq>(old: &[T], new: &[T], limit: usize) -> (Vec<bool>, Vec<bool>) {
    let mut removed = vec![false; old.len()];
    let mut inserted = vec![false; new.len()];

    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let (a, b) = (&old[prefix..], &new[prefix..]);

    match myers(a, b, limit) {
        Some(changes) => {
            for change in changes {
                match change {
                    Change::Remove(i) => removed[prefix + i] = true,
                    Change::Insert(i) => inserted[prefix + i] = true,
                }
            }
        }
        None => {
            let suffix = a
                .iter()
                .rev()
                .zip(b.iter().rev())
                .take_while(|(x, y)| x == y)
                .count();
            for flag in &mut removed[prefix..old.len() - suffix] {
                *flag = true;
            }
            for flag in &mut inserted[prefix..new.len() - suffix] {
                *flag = true;
            }
        }
    }
    (removed, inserted)
}

enum Change {
    Remove(usize),
    Insert(usize),
}

/// Greedy forward Myers with a backtrack, transcribed from Swift's
/// `_myers(from:to:using:)` (`CollectionDifference`). `x` walks `a` (old),
/// `y` walks `b` (new), `k = x - y`. Returns `None` when the trace would
/// exceed `limit` cells.
fn myers<T: Eq>(a: &[T], b: &[T], limit: usize) -> Option<Vec<Change>> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    if n == 0 && m == 0 {
        return Some(Vec::new());
    }
    let max = n + m;
    let offset = max + 1;
    let at = |k: isize| (k + offset) as usize;

    let mut v = vec![0isize; (2 * max + 3) as usize];
    // `trace[d]` holds V over k in [-d, d] as it stood *before* round d —
    // exactly the cells round d (and its backtrack step) reads.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut cells = 0usize;

    'rounds: for d in 0..=max {
        cells += (2 * d + 1) as usize;
        if cells > limit {
            return None;
        }
        trace.push(v[at(-d)..=at(d)].to_vec());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                break 'rounds;
            }
            k += 2;
        }
    }

    let mut changes = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..trace.len()).rev() {
        let row = &trace[d];
        let d = d as isize;
        let get = |k: isize| row[(k + d) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = get(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
        }
        if y != prev_y {
            changes.push(Change::Insert(prev_y as usize));
        } else {
            changes.push(Change::Remove(prev_x as usize));
        }
        x = prev_x;
        y = prev_y;
    }
    Some(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn d(old: &str, new: &str) -> Vec<Edit> {
        diff(&chars(old), &chars(new))
    }

    use Edit::{Deleted as D, Inserted as I};
    fn u(old: usize, new: usize) -> Edit {
        Edit::Unchanged { old, new }
    }

    /// Checks an edit script reproduces both sides and is ordered.
    fn assert_valid(old: &[char], new: &[char], edits: &[Edit]) {
        let (mut oi, mut ni) = (0, 0);
        let mut seen_insert_in_gap = false;
        for e in edits {
            match *e {
                Edit::Unchanged { old: o, new: n } => {
                    assert_eq!((o, n), (oi, ni));
                    assert_eq!(old[o], new[n]);
                    oi += 1;
                    ni += 1;
                    seen_insert_in_gap = false;
                }
                Edit::Deleted(o) => {
                    assert_eq!(o, oi);
                    assert!(!seen_insert_in_gap, "deletion after insertion in a gap");
                    oi += 1;
                }
                Edit::Inserted(n) => {
                    assert_eq!(n, ni);
                    ni += 1;
                    seen_insert_in_gap = true;
                }
            }
        }
        assert_eq!((oi, ni), (old.len(), new.len()));
    }

    fn lcs_len(a: &[char], b: &[char]) -> usize {
        let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                dp[i][j] = if a[i] == b[j] {
                    dp[i + 1][j + 1] + 1
                } else {
                    dp[i + 1][j].max(dp[i][j + 1])
                };
            }
        }
        dp[0][0]
    }

    mod basic_tests {
        use super::*;

        #[test]
        fn both_empty() {
            assert!(d("", "").is_empty());
            assert_eq!(
                segments::<char>(&[], &[]),
                vec![Segment {
                    deleted: 0..0,
                    inserted: 0..0,
                    anchor: None
                }]
            );
        }

        #[test]
        fn identical() {
            assert_eq!(d("abc", "abc"), vec![u(0, 0), u(1, 1), u(2, 2)]);
        }

        #[test]
        fn all_inserted() {
            assert_eq!(d("", "ab"), vec![I(0), I(1)]);
        }

        #[test]
        fn all_deleted() {
            assert_eq!(d("ab", ""), vec![D(0), D(1)]);
        }

        #[test]
        fn interleaved_changes_put_deletions_first_in_each_gap() {
            assert_eq!(
                d("axbyc", "aXbYc"),
                vec![u(0, 0), D(1), I(1), u(2, 2), D(3), I(3), u(4, 4)]
            );
        }

        #[test]
        fn replacement_run_lists_all_deletions_then_all_insertions() {
            assert_eq!(
                d("axyb", "aXYZb"),
                vec![u(0, 0), D(1), D(2), I(1), I(2), I(3), u(3, 4)]
            );
        }

        #[test]
        fn moved_line_is_a_delete_and_an_insert() {
            // "c" moves from the end to the front.
            assert_eq!(d("abc", "cab"), vec![I(0), u(0, 1), u(1, 2), D(2)]);
        }

        #[test]
        fn segments_include_empty_gaps() {
            let segs = segments(&chars("ab"), &chars("ab"));
            assert_eq!(segs.len(), 3);
            assert!(segs
                .iter()
                .all(|s| s.deleted.is_empty() && s.inserted.is_empty()));
            assert_eq!(segs[2].anchor, None);
        }
    }

    /// Goldens for inputs with several minimal edit scripts. Each expected
    /// script was derived by hand-running Swift's `_myers`; they pin the
    /// anchor choice so an algorithm tweak cannot silently renumber IDs.
    mod ambiguity_goldens {
        use super::*;

        #[test]
        fn swap_keeps_the_later_old_element() {
            // [a, b] → [b, a]: Swift removes old "a" and re-inserts it.
            assert_eq!(d("ab", "ba"), vec![D(0), u(1, 0), I(1)]);
        }

        #[test]
        fn repeated_element_anchors_on_the_earliest_match() {
            // A suffix trim would anchor on old[3] instead.
            assert_eq!(d("caba", "a"), vec![D(0), u(1, 0), D(2), D(3)]);
        }

        #[test]
        fn duplicate_insertion_goes_after_the_existing_copy() {
            assert_eq!(d("a", "aa"), vec![u(0, 0), I(1)]);
        }

        #[test]
        fn duplicate_deletion_removes_the_later_copy() {
            assert_eq!(d("aa", "a"), vec![u(0, 0), D(1)]);
        }

        #[test]
        fn substitution_against_repeated_neighbours() {
            // Two blank-ish separators either side of the edit.
            assert_eq!(
                d("x-x-x", "x-y-x"),
                vec![u(0, 0), u(1, 1), D(2), I(2), u(3, 3), u(4, 4)]
            );
        }

        #[test]
        fn inserted_block_between_identical_blocks() {
            assert_eq!(d("aba", "abba"), vec![u(0, 0), u(1, 1), I(2), u(2, 3)]);
        }
    }

    mod property_tests {
        use super::*;

        /// Tiny deterministic LCG — no `rand` dependency.
        struct Lcg(u64);
        impl Lcg {
            fn next(&mut self) -> u64 {
                self.0 = self
                    .0
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                self.0 >> 33
            }
            fn seq(&mut self, max_len: u64, alphabet: &[char]) -> Vec<char> {
                let len = self.next() % (max_len + 1);
                (0..len)
                    .map(|_| alphabet[(self.next() % alphabet.len() as u64) as usize])
                    .collect()
            }
        }

        #[test]
        fn scripts_are_valid_and_minimal() {
            let mut rng = Lcg(42);
            for _ in 0..2000 {
                let a = rng.seq(12, &['a', 'b', 'c']);
                let b = rng.seq(12, &['a', 'b', 'c']);
                let edits = diff(&a, &b);
                assert_valid(&a, &b, &edits);
                let kept = edits
                    .iter()
                    .filter(|e| matches!(e, Edit::Unchanged { .. }))
                    .count();
                assert_eq!(kept, lcs_len(&a, &b), "{a:?} → {b:?}");
            }
        }

        #[test]
        fn prefix_trim_does_not_change_the_script() {
            // The trim is an optimisation only: running the core algorithm
            // on the untrimmed input must classify every index the same way.
            let mut rng = Lcg(99);
            for _ in 0..1000 {
                let a = rng.seq(10, &['a', 'b', 'c']);
                let b = rng.seq(10, &['a', 'b', 'c']);
                let mut removed = vec![false; a.len()];
                let mut inserted = vec![false; b.len()];
                for c in myers(&a, &b, usize::MAX).unwrap() {
                    match c {
                        Change::Remove(i) => removed[i] = true,
                        Change::Insert(i) => inserted[i] = true,
                    }
                }
                assert_eq!(classify(&a, &b, usize::MAX), (removed, inserted));
            }
        }

        #[test]
        fn fallback_replaces_the_middle_and_keeps_prefix_and_suffix() {
            let old = chars("pXYZs");
            let new = chars("pABCs");
            let segs = segments_with_limit(&old, &new, 1);
            assert_eq!(
                segs,
                vec![
                    Segment {
                        deleted: 0..0,
                        inserted: 0..0,
                        anchor: Some((0, 0))
                    },
                    Segment {
                        deleted: 1..4,
                        inserted: 1..4,
                        anchor: Some((4, 4))
                    },
                    Segment {
                        deleted: 5..5,
                        inserted: 5..5,
                        anchor: None
                    },
                ]
            );
        }

        #[test]
        fn fallback_scripts_are_still_valid() {
            let mut rng = Lcg(7);
            for _ in 0..500 {
                let a = rng.seq(10, &['a', 'b']);
                let b = rng.seq(10, &['a', 'b']);
                let edits: Vec<Edit> = segments_with_limit(&a, &b, 3)
                    .into_iter()
                    .flat_map(|s| {
                        s.deleted
                            .map(Edit::Deleted)
                            .chain(s.inserted.map(Edit::Inserted))
                            .chain(s.anchor.map(|(o, n)| Edit::Unchanged { old: o, new: n }))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                assert_valid(&a, &b, &edits);
            }
        }
    }
}

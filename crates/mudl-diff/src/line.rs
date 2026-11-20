//! Line-level diffing (`LineLevelDiff.swift`; plan §7.4).
//!
//! Used for code-block pairs ([`crate::code`]) and, in Down mode, for paired
//! prose blocks shown line by line.

use crate::myers;

/// One line's diff annotation, plus the index it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineChange {
    pub annotation: Annotation,
    /// 0-based index in the source array: the new array for `Unchanged`
    /// and `Inserted`, the old array for `Deleted`.
    pub source_index: usize,
}

/// Whether a line survived, was added, or was removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Annotation {
    Unchanged,
    Inserted,
    Deleted,
}

/// Diffs two arrays of lines and returns per-line annotations in display
/// order: within each gap between unchanged anchors, deletions precede
/// insertions. Returns `None` when the arrays are identical, which callers
/// use to fall back to block-level handling.
pub fn diff<T: Eq>(old: &[T], new: &[T]) -> Option<Vec<LineChange>> {
    let edits = myers::diff(old, new);
    if edits
        .iter()
        .all(|e| matches!(e, myers::Edit::Unchanged { .. }))
    {
        return None;
    }
    Some(
        edits
            .into_iter()
            .map(|e| match e {
                myers::Edit::Unchanged { new, .. } => LineChange {
                    annotation: Annotation::Unchanged,
                    source_index: new,
                },
                myers::Edit::Inserted(i) => LineChange {
                    annotation: Annotation::Inserted,
                    source_index: i,
                },
                myers::Edit::Deleted(i) => LineChange {
                    annotation: Annotation::Deleted,
                    source_index: i,
                },
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: [&str; 0] = [];

    fn annotations(entries: &[LineChange]) -> Vec<Annotation> {
        entries.iter().map(|e| e.annotation).collect()
    }

    /// Ported from `LineLevelDiffTests.swift`.
    mod diff_tests {
        use super::*;

        #[test]
        fn identical_lines_returns_none() {
            let lines = ["alpha", "beta", "gamma"];
            assert_eq!(diff(&lines, &lines), None);
        }

        #[test]
        fn both_empty_returns_none() {
            assert_eq!(diff(&NONE, &NONE), None);
        }

        #[test]
        fn single_line_identical_returns_none() {
            assert_eq!(diff(&["same"], &["same"]), None);
        }

        #[test]
        fn single_line_replaced_produces_deleted_then_inserted() {
            let result = diff(&["a", "b", "c"], &["a", "B", "c"]).unwrap();
            assert_eq!(
                annotations(&result),
                vec![
                    Annotation::Unchanged,
                    Annotation::Deleted,
                    Annotation::Inserted,
                    Annotation::Unchanged
                ]
            );
        }

        #[test]
        fn single_line_pair_trivially_changed() {
            let result = diff(&["old"], &["new"]).unwrap();
            assert_eq!(result.len(), 2);
            assert_eq!(result[0].annotation, Annotation::Deleted);
            assert_eq!(result[1].annotation, Annotation::Inserted);
        }

        #[test]
        fn lines_inserted_at_end() {
            let result = diff(&["a", "b"], &["a", "b", "c", "d"]).unwrap();
            assert_eq!(
                result
                    .iter()
                    .filter(|e| e.annotation == Annotation::Inserted)
                    .count(),
                2
            );
            assert_eq!(
                result
                    .iter()
                    .filter(|e| e.annotation == Annotation::Deleted)
                    .count(),
                0
            );
        }

        #[test]
        fn lines_inserted_in_middle() {
            let result = diff(&["a", "c"], &["a", "b", "c"]).unwrap();
            assert_eq!(
                annotations(&result),
                vec![
                    Annotation::Unchanged,
                    Annotation::Inserted,
                    Annotation::Unchanged
                ]
            );
        }

        #[test]
        fn lines_deleted_from_middle() {
            let result = diff(&["a", "b", "c", "d"], &["a", "d"]).unwrap();
            assert_eq!(
                result
                    .iter()
                    .filter(|e| e.annotation == Annotation::Deleted)
                    .count(),
                2
            );
            assert_eq!(
                result
                    .iter()
                    .filter(|e| e.annotation == Annotation::Inserted)
                    .count(),
                0
            );
        }

        #[test]
        fn two_separate_changes() {
            let result = diff(&["a", "b", "c", "d", "e"], &["a", "B", "c", "D", "e"]).unwrap();
            assert_eq!(
                annotations(&result),
                vec![
                    Annotation::Unchanged,
                    Annotation::Deleted,
                    Annotation::Inserted,
                    Annotation::Unchanged,
                    Annotation::Deleted,
                    Annotation::Inserted,
                    Annotation::Unchanged,
                ]
            );
        }

        #[test]
        fn deletions_before_insertions_in_gap() {
            let result = diff(&["a", "x", "y", "b"], &["a", "z", "b"]).unwrap();
            let gap: Vec<&LineChange> = result
                .iter()
                .filter(|e| e.annotation != Annotation::Unchanged)
                .collect();
            assert_eq!(gap.len(), 3);
            assert_eq!(gap[0].annotation, Annotation::Deleted);
            assert_eq!(gap[1].annotation, Annotation::Deleted);
            assert_eq!(gap[2].annotation, Annotation::Inserted);
        }

        #[test]
        fn all_lines_changed() {
            let result = diff(&["a", "b", "c"], &["x", "y", "z"]).unwrap();
            assert_eq!(
                result
                    .iter()
                    .filter(|e| e.annotation == Annotation::Deleted)
                    .count(),
                3
            );
            assert_eq!(
                result
                    .iter()
                    .filter(|e| e.annotation == Annotation::Inserted)
                    .count(),
                3
            );
            assert_eq!(
                result
                    .iter()
                    .filter(|e| e.annotation == Annotation::Unchanged)
                    .count(),
                0
            );
        }

        #[test]
        fn empty_old_all_inserted() {
            let result = diff(&NONE, &["a", "b"]).unwrap();
            assert_eq!(result.len(), 2);
            assert!(result.iter().all(|e| e.annotation == Annotation::Inserted));
        }

        #[test]
        fn empty_new_all_deleted() {
            let result = diff(&["a", "b"], &NONE).unwrap();
            assert_eq!(result.len(), 2);
            assert!(result.iter().all(|e| e.annotation == Annotation::Deleted));
        }

        #[test]
        fn unchanged_entry_carries_new_index() {
            let result = diff(&["a", "b", "c"], &["a", "X", "b", "c"]).unwrap();
            let unchanged: Vec<&LineChange> = result
                .iter()
                .filter(|e| e.annotation == Annotation::Unchanged)
                .collect();
            assert_eq!(unchanged[0].source_index, 0);
            assert_eq!(unchanged[1].source_index, 2);
            assert_eq!(unchanged[2].source_index, 3);
        }

        #[test]
        fn deleted_entry_carries_old_index() {
            let result = diff(&["a", "b", "c"], &["a", "c"]).unwrap();
            let deleted: Vec<&LineChange> = result
                .iter()
                .filter(|e| e.annotation == Annotation::Deleted)
                .collect();
            assert_eq!(deleted.len(), 1);
            assert_eq!(deleted[0].source_index, 1);
        }

        #[test]
        fn inserted_entry_carries_new_index() {
            let result = diff(&["a", "c"], &["a", "b", "c"]).unwrap();
            let inserted: Vec<&LineChange> = result
                .iter()
                .filter(|e| e.annotation == Annotation::Inserted)
                .collect();
            assert_eq!(inserted.len(), 1);
            assert_eq!(inserted[0].source_index, 1);
        }

        #[test]
        fn indices_correct_with_shifted_positions() {
            let result = diff(&["a", "b", "c"], &["a", "X", "Y", "c"]).unwrap();
            let deleted: Vec<&LineChange> = result
                .iter()
                .filter(|e| e.annotation == Annotation::Deleted)
                .collect();
            let inserted: Vec<&LineChange> = result
                .iter()
                .filter(|e| e.annotation == Annotation::Inserted)
                .collect();
            assert_eq!(deleted[0].source_index, 1);
            assert_eq!(inserted[0].source_index, 1);
            assert_eq!(inserted[1].source_index, 2);
        }

        #[test]
        fn accepts_owned_strings() {
            let old = vec!["a".to_string(), "b".to_string()];
            let new = vec!["a".to_string(), "c".to_string()];
            assert_eq!(diff(&old, &new).unwrap().len(), 3);
        }

        #[test]
        fn whitespace_only_change_detected() {
            let old = ["  line one", "line two"];
            let new = ["    line one", "line two"];
            assert!(diff(&old, &new).is_some());
        }
    }
}

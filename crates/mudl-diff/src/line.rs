//! Line-level diffing (ported from `mud`'s
//! `Core/Sources/Diff/LineLevelDiff.swift`, Phase 13.4).

/// One line's diff annotation, plus the index it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineChange {
    pub annotation: Annotation,
    /// 0-based index in the source array: the new array for `Unchanged`
    /// and `Inserted`, the old array for `Deleted`.
    pub source_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Annotation {
    Unchanged,
    Inserted,
    Deleted,
}

/// Diffs two arrays of lines and returns per-line annotations. Within each
/// gap between unchanged anchors, deletions precede insertions. Returns
/// `None` when the arrays are identical.
pub fn diff(old: &[&str], new: &[&str]) -> Option<Vec<LineChange>> {
    let (removed_old, inserted_new) = crate::lcs::diff_indices(old, new);
    if removed_old.is_empty() && inserted_new.is_empty() {
        return None;
    }

    let mut anchors: Vec<(usize, usize)> = Vec::new();
    let mut oi = 0usize;
    let mut ni = 0usize;
    while oi < old.len() && ni < new.len() {
        if removed_old.contains(&oi) {
            oi += 1;
            continue;
        }
        if inserted_new.contains(&ni) {
            ni += 1;
            continue;
        }
        anchors.push((oi, ni));
        oi += 1;
        ni += 1;
    }

    let mut result = Vec::new();
    let mut prev_old: isize = -1;
    let mut prev_new: isize = -1;

    for (anchor_old, anchor_new) in &anchors {
        for i in (prev_old + 1) as usize..*anchor_old {
            result.push(LineChange {
                annotation: Annotation::Deleted,
                source_index: i,
            });
        }
        for i in (prev_new + 1) as usize..*anchor_new {
            result.push(LineChange {
                annotation: Annotation::Inserted,
                source_index: i,
            });
        }
        result.push(LineChange {
            annotation: Annotation::Unchanged,
            source_index: *anchor_new,
        });
        prev_old = *anchor_old as isize;
        prev_new = *anchor_new as isize;
    }

    for i in (prev_old + 1) as usize..old.len() {
        result.push(LineChange {
            annotation: Annotation::Deleted,
            source_index: i,
        });
    }
    for i in (prev_new + 1) as usize..new.len() {
        result.push(LineChange {
            annotation: Annotation::Inserted,
            source_index: i,
        });
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn annotations(entries: &[LineChange]) -> Vec<Annotation> {
        entries.iter().map(|e| e.annotation).collect()
    }

    #[test]
    fn identical_lines_returns_none() {
        let lines = ["alpha", "beta", "gamma"];
        assert_eq!(diff(&lines, &lines), None);
    }

    #[test]
    fn both_empty_returns_none() {
        assert_eq!(diff(&[], &[]), None);
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
        let result = diff(&[], &["a", "b"]).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.annotation == Annotation::Inserted));
    }

    #[test]
    fn empty_new_all_deleted() {
        let result = diff(&["a", "b"], &[]).unwrap();
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
    fn whitespace_only_change_detected() {
        let old = ["  line one", "line two"];
        let new = ["    line one", "line two"];
        assert!(diff(&old, &new).is_some());
    }
}

//! Line-level diff inside a paired code block (`CodeBlockDiff.swift`;
//! plan §7.6).
//!
//! When a gap deletes one code block and inserts another, `mud` shows the
//! edit line by line instead of as two whole blocks. This module produces
//! the per-line data: annotation, line text, change and group IDs, and the
//! word ranges to mark on paired lines.
//!
//! Unlike `mud`, nothing here highlights or escapes: a line carries its raw
//! text and the byte ranges of its `<ins>`/`<del>` words, and the renderer
//! (Phase 3) escapes, marks, and leaves syntax colouring to the browser.

use std::ops::Range;

use crate::line::{self, Annotation};
use crate::pairing;
use crate::word;

/// One displayed line of a code-block diff.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CodeLine {
    pub annotation: Annotation,
    /// The line's text, without its newline.
    pub text: String,
    /// 0-based line index within the block's content: into the old block
    /// for `Deleted`, the new block otherwise.
    pub source_index: usize,
    /// Shared by every line of a contiguous changed cluster; `None` for
    /// unchanged lines.
    pub change_id: Option<String>,
    /// The cluster's group; `None` for unchanged lines.
    pub group_id: Option<String>,
    /// The badge number, on the first line of each cluster only.
    pub group_index: Option<usize>,
    /// Byte ranges into `text` to mark: `<del>` on a deleted line, `<ins>`
    /// on an inserted one. Empty unless the line was paired and the pair's
    /// word diff passed the significance threshold.
    pub word_ranges: Vec<Range<usize>>,
}

impl CodeLine {
    fn new(annotation: Annotation, text: &str, source_index: usize) -> Self {
        CodeLine {
            annotation,
            text: text.to_string(),
            source_index,
            change_id: None,
            group_id: None,
            group_index: None,
            word_ranges: Vec::new(),
        }
    }

    /// True for inserted and deleted lines.
    pub fn is_changed(&self) -> bool {
        self.annotation != Annotation::Unchanged
    }
}

/// Splits code into lines, dropping the empty last line a trailing newline
/// leaves (the content of a fenced block usually ends with one).
pub fn split_code(code: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = code.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// Computes a line diff with word ranges but no IDs. Returns `None` when no
/// line changed (only the fence or language did), so the caller keeps
/// block-level handling.
///
/// Each gap between unchanged lines emits all its deleted lines, then all
/// its inserted lines. Within the gap, lines pair by word overlap
/// ([`pairing::best_pairs`]); a pair gets word ranges only when
/// [`word::has_significant_changes`] holds at `threshold`.
pub fn compute_raw(old_code: &str, new_code: &str, threshold: f64) -> Option<Vec<CodeLine>> {
    let old_lines = split_code(old_code);
    let new_lines = split_code(new_code);
    let entries = line::diff(&old_lines, &new_lines)?;

    let mut result = Vec::with_capacity(entries.len());
    let mut idx = 0;
    while idx < entries.len() {
        let entry = entries[idx];
        if entry.annotation == Annotation::Unchanged {
            result.push(CodeLine::new(
                Annotation::Unchanged,
                new_lines[entry.source_index],
                entry.source_index,
            ));
            idx += 1;
            continue;
        }
        let (mut dels, mut ins) = (Vec::new(), Vec::new());
        while idx < entries.len() && entries[idx].annotation != Annotation::Unchanged {
            match entries[idx].annotation {
                Annotation::Deleted => dels.push(entries[idx].source_index),
                _ => ins.push(entries[idx].source_index),
            }
            idx += 1;
        }
        emit_gap(&dels, &ins, &old_lines, &new_lines, threshold, &mut result);
    }
    Some(result)
}

fn emit_gap(
    dels: &[usize],
    ins: &[usize],
    old_lines: &[&str],
    new_lines: &[&str],
    threshold: f64,
    out: &mut Vec<CodeLine>,
) {
    let del_texts: Vec<&str> = dels.iter().map(|&i| old_lines[i]).collect();
    let ins_texts: Vec<&str> = ins.iter().map(|&i| new_lines[i]).collect();

    let mut del_lines: Vec<CodeLine> = dels
        .iter()
        .map(|&i| CodeLine::new(Annotation::Deleted, old_lines[i], i))
        .collect();
    let mut ins_lines: Vec<CodeLine> = ins
        .iter()
        .map(|&i| CodeLine::new(Annotation::Inserted, new_lines[i], i))
        .collect();

    for (d, i) in pairing::best_pairs(&del_texts, &ins_texts) {
        let spans = word::diff(del_texts[d], ins_texts[i]);
        if !word::has_significant_changes(&spans, threshold) {
            continue;
        }
        let (del_ranges, ins_ranges) = word::change_ranges(&spans);
        del_lines[d].word_ranges = del_ranges;
        ins_lines[i].word_ranges = ins_ranges;
    }
    out.extend(del_lines);
    out.extend(ins_lines);
}

/// Index ranges of the contiguous changed clusters in `lines`.
pub fn clusters(lines: &[CodeLine]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].is_changed() {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && lines[i].is_changed() {
            i += 1;
        }
        out.push(start..i);
    }
    out
}

/// Gives every line of each changed cluster one fresh change ID.
///
/// `plan` mints these when a code pair's gap closes, after every block-level
/// ID in the gap, so all consumers number the same edit identically.
pub fn assign_change_ids(lines: &mut [CodeLine], mut next_change_id: impl FnMut() -> String) {
    for cluster in clusters(lines) {
        let id = next_change_id();
        for line in &mut lines[cluster] {
            line.change_id = Some(id.clone());
        }
    }
}

/// Gives each changed cluster its own group, with the badge index on the
/// cluster's first line only. `next_group_id` returns `(id, index)`.
pub fn assign_group_ids(
    lines: &mut [CodeLine],
    mut next_group_id: impl FnMut() -> (String, usize),
) {
    for cluster in clusters(lines) {
        let (id, index) = next_group_id();
        for (offset, line) in lines[cluster].iter_mut().enumerate() {
            line.group_id = Some(id.clone());
            line.group_index = (offset == 0).then_some(index);
        }
    }
}

/// [`compute_raw`] plus both ID passes, for standalone use. `plan` calls the
/// passes separately because it mints change and group IDs at different
/// times.
pub fn compute(
    old_code: &str,
    new_code: &str,
    threshold: f64,
    next_change_id: impl FnMut() -> String,
    next_group_id: impl FnMut() -> (String, usize),
) -> Option<Vec<CodeLine>> {
    let mut lines = compute_raw(old_code, new_code, threshold)?;
    assign_change_ids(&mut lines, next_change_id);
    assign_group_ids(&mut lines, next_group_id);
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Annotation::{Deleted, Inserted, Unchanged};

    /// Runs `compute` with real counters; returns the lines and how many
    /// change and group IDs were minted.
    fn run(old: &str, new: &str) -> Option<(Vec<CodeLine>, usize, usize)> {
        let mut changes = 0;
        let mut groups = 0;
        let lines = compute(
            old,
            new,
            0.25,
            || {
                changes += 1;
                format!("change-{changes}")
            },
            || {
                groups += 1;
                (format!("group-{groups}"), groups)
            },
        )?;
        Some((lines, changes, groups))
    }

    fn annotations(lines: &[CodeLine]) -> Vec<Annotation> {
        lines.iter().map(|l| l.annotation).collect()
    }

    fn with(lines: &[CodeLine], a: Annotation) -> Vec<&CodeLine> {
        lines.iter().filter(|l| l.annotation == a).collect()
    }

    /// Ported from `CodeBlockDiffTests.swift` (highlighting assertions
    /// become raw-text and word-range assertions).
    mod compute_tests {
        use super::*;

        #[test]
        fn identical_content_returns_none() {
            let code = "func greet() {\n    print(\"hello\")\n}\n";
            assert!(run(code, code).is_none());
        }

        #[test]
        fn language_only_change_returns_none() {
            // Languages are not inputs here; same content is no diff.
            assert!(run("x = 1\n", "x = 1\n").is_none());
        }

        #[test]
        fn single_line_changed_produces_one_mixed_group() {
            let (lines, _, groups) = run(
                "func greet() {\n    print(\"hello\")\n}\n",
                "func greet() {\n    print(\"hi\")\n}\n",
            )
            .unwrap();
            assert_eq!(
                annotations(&lines),
                vec![Unchanged, Deleted, Inserted, Unchanged]
            );
            assert_eq!(groups, 1);
            assert_eq!(lines[1].group_id, lines[2].group_id);
        }

        #[test]
        fn lines_added_produce_an_insertion_group() {
            let (lines, _, groups) = run("a\nb\n", "a\nb\nc\nd\n").unwrap();
            assert_eq!(with(&lines, Inserted).len(), 2);
            assert_eq!(with(&lines, Deleted).len(), 0);
            assert_eq!(groups, 1);
        }

        #[test]
        fn lines_removed_produce_a_deletion_group() {
            let (lines, _, _) = run("a\nb\nc\nd\n", "a\nd\n").unwrap();
            assert_eq!(with(&lines, Deleted).len(), 2);
        }

        #[test]
        fn separate_clusters_form_separate_groups() {
            let (lines, changes, groups) = run("a\nb\nc\nd\ne\n", "a\nB\nc\nD\ne\n").unwrap();
            assert_eq!((changes, groups), (2, 2));
            assert_eq!(
                annotations(&lines),
                vec![Unchanged, Deleted, Inserted, Unchanged, Deleted, Inserted, Unchanged]
            );
            assert_eq!(lines[1].change_id.as_deref(), Some("change-1"));
            assert_eq!(lines[2].change_id.as_deref(), Some("change-1"));
            assert_eq!(lines[4].change_id.as_deref(), Some("change-2"));
            assert_eq!(lines[4].group_id.as_deref(), Some("group-2"));
        }

        #[test]
        fn deletions_come_before_insertions_in_a_gap() {
            let (lines, _, _) = run("a\nold line\nc\n", "a\nnew line\nc\n").unwrap();
            let changed: Vec<Annotation> = lines
                .iter()
                .filter(|l| l.is_changed())
                .map(|l| l.annotation)
                .collect();
            assert_eq!(changed, vec![Deleted, Inserted]);
        }

        #[test]
        fn unchanged_lines_have_no_ids() {
            let (lines, _, _) = run("a\nb\n", "a\nB\n").unwrap();
            for line in with(&lines, Unchanged) {
                assert_eq!(
                    (&line.change_id, &line.group_id, line.group_index),
                    (&None, &None, None)
                );
            }
        }

        #[test]
        fn lines_carry_their_text_and_source_index() {
            let (lines, _, _) = run("let x = 1\nlet y = 2\n", "let x = 1\nlet z = 3\n").unwrap();
            assert_eq!(lines[0].text, "let x = 1");
            assert_eq!(
                (lines[1].text.as_str(), lines[1].source_index),
                ("let y = 2", 1)
            );
            assert_eq!(
                (lines[2].text.as_str(), lines[2].source_index),
                ("let z = 3", 1)
            );
        }

        #[test]
        fn different_content_in_different_languages_still_diffs() {
            assert!(run("let x = 1\nlet y = 2\n", "x = 1\nz = 3\n").is_some());
        }

        #[test]
        fn text_is_raw_not_escaped() {
            let (lines, _, _) = run("a <b> c\n", "a <b> d\n").unwrap();
            assert!(lines.iter().all(|l| l.text.starts_with("a <b> ")));
        }

        #[test]
        fn trailing_newline_does_not_cause_a_phantom_line() {
            let (lines, _, _) =
                run("line one\nline two\n", "line one\nline two changed\n").unwrap();
            assert_eq!(lines.len(), 3);
            assert!(!lines.last().unwrap().text.is_empty());
        }

        #[test]
        fn all_lines_changed_is_one_group() {
            let (lines, _, groups) = run("a\nb\nc\n", "x\ny\nz\n").unwrap();
            assert_eq!(groups, 1);
            assert!(with(&lines, Unchanged).is_empty());
        }

        #[test]
        fn empty_old_is_all_insertions() {
            let (lines, _, _) = run("", "a\nb\n").unwrap();
            assert_eq!(annotations(&lines), vec![Inserted, Inserted]);
        }

        #[test]
        fn empty_new_is_all_deletions() {
            let (lines, _, _) = run("a\nb\n", "").unwrap();
            assert_eq!(annotations(&lines), vec![Deleted, Deleted]);
        }

        #[test]
        fn single_line_block() {
            let (lines, _, _) = run("old\n", "new\n").unwrap();
            assert_eq!(annotations(&lines), vec![Deleted, Inserted]);
        }

        #[test]
        fn group_index_is_on_the_first_line_of_each_cluster_only() {
            let (lines, _, _) = run("a\nb\nc\n", "a\nB\nc\n").unwrap();
            assert_eq!(lines[1].group_index, Some(1));
            assert_eq!(lines[2].group_index, None);
        }
    }

    mod word_range_tests {
        use super::*;

        #[test]
        fn paired_lines_get_word_ranges() {
            let (lines, _, _) = run("let x = 1\nlet y = 2\n", "let x = 1\nlet z = 3\n").unwrap();
            let del = with(&lines, Deleted)[0];
            let ins = with(&lines, Inserted)[0];
            assert_eq!(del.word_ranges, vec![4..5, 8..9]);
            assert_eq!(ins.word_ranges, vec![4..5, 8..9]);
        }

        #[test]
        fn unpaired_lines_have_no_word_ranges() {
            let (lines, _, _) = run("a\nb\nc\n", "x\n").unwrap();
            let dels = with(&lines, Deleted);
            assert_eq!(dels.len(), 3);
            assert!(dels.iter().all(|l| l.word_ranges.is_empty()));
        }

        #[test]
        fn identical_paired_neighbours_stay_unmarked() {
            let (lines, _, _) = run("a\nsame\nb\n", "x\nsame\ny\n").unwrap();
            let unchanged = with(&lines, Unchanged);
            assert_eq!(unchanged.len(), 1);
            assert!(unchanged[0].word_ranges.is_empty());
        }

        #[test]
        fn single_word_edit_marks_only_that_word() {
            let (lines, _, _) = run("let x = 1\n", "let x = 2\n").unwrap();
            let ins = with(&lines, Inserted)[0];
            assert_eq!(&ins.text[ins.word_ranges[0].clone()], "2");
        }

        #[test]
        fn below_threshold_pairs_are_unmarked() {
            let (lines, _, _) = run("alpha beta gamma\n", "one two three\n").unwrap();
            assert!(lines.iter().all(|l| l.word_ranges.is_empty()));
        }

        #[test]
        fn pairing_follows_word_overlap_not_position() {
            let (lines, _, _) = run(
                "alpha beta gamma\none two three\n",
                "one two four\nalpha beta delta\n",
            )
            .unwrap();
            // Deleted "one two three" (old 1) pairs with inserted "one two four".
            let del = lines.iter().find(|l| l.text == "one two three").unwrap();
            assert_eq!(&del.text[del.word_ranges[0].clone()], "three");
            let ins = lines.iter().find(|l| l.text == "alpha beta delta").unwrap();
            assert_eq!(&ins.text[ins.word_ranges[0].clone()], "delta");
        }
    }

    mod split_code_tests {
        use super::*;

        #[test]
        fn drops_only_one_trailing_empty_line() {
            assert_eq!(split_code("a\nb\n"), vec!["a", "b"]);
            assert_eq!(split_code("a\n\n"), vec!["a", ""]);
            assert!(split_code("").is_empty());
        }
    }
}

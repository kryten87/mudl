//! The sidebar's change list (`ChangeList.swift`, `ChangeGroup.swift`;
//! plan §7.7).
//!
//! [`compute_changes`] projects a [`ChangePlan`] into a flat, ordered list
//! of [`DocumentChange`]s; [`ChangeGroup::build`] folds that list into the
//! groups the Changes bar counts and the sidebar shows. Neither holds any
//! block references, so both are cheap to clone out of a lock.

use crate::block::{LeafBlock, LeafTag};
use crate::code::CodeLine;
use crate::line::Annotation;
use crate::plan::{ChangePlan, InsertionSlot};

/// Summaries longer than this many characters are truncated.
const SUMMARY_LIMIT: usize = 60;

/// Whether a change adds or removes content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeType {
    Insertion,
    Deletion,
}

/// One entry of the change list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentChange {
    /// The change ID (`change-N`). Every changed line of a code-block
    /// cluster repeats its cluster's ID.
    pub id: String,
    pub ty: ChangeType,
    /// Plain text, at most ~60 characters.
    pub summary: String,
    /// The line to scroll to. A deletion uses its host line: the block it
    /// is shown before.
    pub source_line: usize,
    /// True when no unchanged block separates this change from the previous
    /// one. Always false for the first change.
    pub is_consecutive: bool,
    /// `group-N`, or `""` when the plan has no group for the change.
    pub group_id: String,
    /// The badge number, or 0 (a code cluster's later lines carry 0).
    pub group_index: usize,
    /// True when the change's group holds both deletions and insertions.
    pub is_mixed: bool,
}

/// A group of related changes, for navigation and counts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChangeGroup {
    /// `group-N`: the group's identity.
    pub group_id: String,
    /// The badge number (from the group's first entry).
    pub index: usize,
    /// `Insertion` when the group holds any insertion.
    pub ty: ChangeType,
    /// Both types present, or any member flagged mixed by the plan.
    pub is_mixed: bool,
    /// Every member's change ID, in order (code clusters repeat IDs).
    pub change_ids: Vec<String>,
    /// Each member's type and summary, for the multi-line sidebar row.
    /// Collapsing long same-type runs ("and N more insertions") is left to
    /// the view.
    pub members: Vec<(ChangeType, String)>,
    /// The first member's line.
    pub source_line: usize,
}

impl ChangeGroup {
    /// Buckets `changes` by `group_id`, in first-seen order.
    pub fn build(changes: &[DocumentChange]) -> Vec<ChangeGroup> {
        let mut groups: Vec<ChangeGroup> = Vec::new();
        let mut position: std::collections::HashMap<&str, usize> = Default::default();
        for change in changes {
            let idx = *position.entry(change.group_id.as_str()).or_insert_with(|| {
                groups.push(ChangeGroup {
                    group_id: change.group_id.clone(),
                    index: change.group_index,
                    ty: ChangeType::Deletion,
                    is_mixed: false,
                    change_ids: Vec::new(),
                    members: Vec::new(),
                    source_line: change.source_line,
                });
                groups.len() - 1
            });
            let group = &mut groups[idx];
            group.change_ids.push(change.id.clone());
            group.members.push((change.ty, change.summary.clone()));
            group.is_mixed |= change.is_mixed;
        }
        for group in &mut groups {
            let has_ins = group
                .members
                .iter()
                .any(|(t, _)| *t == ChangeType::Insertion);
            let has_del = group
                .members
                .iter()
                .any(|(t, _)| *t == ChangeType::Deletion);
            group.is_mixed |= has_ins && has_del;
            if has_ins {
                group.ty = ChangeType::Insertion;
            }
        }
        groups
    }

    /// Number of member changes.
    pub fn count(&self) -> usize {
        self.members.len()
    }
}

/// Projects the plan's gaps into the change list, in document order.
///
/// Within a gap, deletions come first, then insertions and code-pair lines
/// in order. A deletion is listed at its host line: the first insertion in
/// its gap, else the following anchor, else the preceding anchor, else the
/// last surviving line seen.
pub fn compute_changes(plan: &ChangePlan) -> Vec<DocumentChange> {
    let mut changes: Vec<DocumentChange> = Vec::new();
    let mut saw_unchanged = true;
    let mut last_surviving_line = 1;

    for gap in &plan.gaps {
        let host_line = gap
            .insertion_slots
            .first()
            .map(|s| s.insertion_block().source_line)
            .or(gap.following_anchor.as_ref().map(|b| b.source_line))
            .or(gap.preceding_anchor.as_ref().map(|b| b.source_line))
            .unwrap_or(last_surviving_line);

        for del in &gap.deletions {
            let info = plan.groups.get(&del.change_id);
            changes.push(DocumentChange {
                id: del.change_id.clone(),
                ty: ChangeType::Deletion,
                summary: deletion_summary(&del.block),
                source_line: host_line,
                is_consecutive: !changes.is_empty() && !saw_unchanged,
                group_id: info.map(|g| g.group_id.clone()).unwrap_or_default(),
                group_index: info.map_or(0, |g| g.index),
                is_mixed: info.is_some_and(|g| g.is_mixed()),
            });
            saw_unchanged = false;
        }

        for slot in &gap.insertion_slots {
            match slot {
                InsertionSlot::Block(change) => {
                    let info = plan.groups.get(&change.change_id);
                    changes.push(DocumentChange {
                        id: change.change_id.clone(),
                        ty: ChangeType::Insertion,
                        summary: block_summary(&change.block),
                        source_line: change.block.source_line,
                        is_consecutive: !changes.is_empty() && !saw_unchanged,
                        group_id: info.map(|g| g.group_id.clone()).unwrap_or_default(),
                        group_index: info.map_or(0, |g| g.index),
                        is_mixed: info.is_some_and(|g| g.is_mixed()),
                    });
                    saw_unchanged = false;
                    last_surviving_line = change.block.source_line;
                }
                InsertionSlot::CodeBlockPair(pair) => {
                    let line = pair.insertion.block.source_line;
                    emit_code_changes(&pair.lines, line, &mut changes, &mut saw_unchanged);
                    last_surviving_line = line;
                }
            }
        }

        if let Some(anchor) = &gap.following_anchor {
            saw_unchanged = true;
            last_surviving_line = anchor.source_line;
        }
    }
    changes
}

/// One entry per changed line of a code pair. An unchanged line breaks the
/// consecutive run; later lines of a cluster are always consecutive.
fn emit_code_changes(
    lines: &[CodeLine],
    source_line: usize,
    changes: &mut Vec<DocumentChange>,
    saw_unchanged: &mut bool,
) {
    let mut current: Option<&str> = None;
    for line in lines {
        let (Some(change_id), Some(group_id)) = (&line.change_id, &line.group_id) else {
            current = None;
            *saw_unchanged = true;
            continue;
        };
        let is_new_cluster = current != Some(change_id.as_str());
        current = Some(change_id);
        let consecutive = !changes.is_empty() && !*saw_unchanged;
        changes.push(DocumentChange {
            id: change_id.clone(),
            ty: if line.annotation == Annotation::Deleted {
                ChangeType::Deletion
            } else {
                ChangeType::Insertion
            },
            summary: summarize(&line.text),
            source_line,
            is_consecutive: if is_new_cluster { consecutive } else { true },
            group_id: group_id.clone(),
            group_index: line.group_index.unwrap_or(0),
            is_mixed: false,
        });
        *saw_unchanged = false;
    }
}

/// A plain-text summary of a block for the sidebar.
///
/// Paragraphs, headings and list items use their inline text (Markdown
/// syntax already gone), code blocks their first line, and everything else
/// its source. See [`summarize`] for the clean-up and truncation.
pub fn block_summary(block: &LeafBlock) -> String {
    let raw = match block.tag {
        LeafTag::Paragraph | LeafTag::Heading(_) | LeafTag::ListItem => {
            block.inline_text.as_deref().unwrap_or(&block.source_text)
        }
        LeafTag::CodeBlock => block
            .code
            .as_deref()
            .unwrap_or("")
            .split('\n')
            .next()
            .unwrap_or(""),
        _ => &block.source_text,
    };
    summarize(raw)
}

/// The summary for a deleted block: a Mermaid diagram shows
/// `[revised diagram]`, matching how its deletion renders.
pub fn deletion_summary(block: &LeafBlock) -> String {
    if block.tag == LeafTag::CodeBlock && block.has_language("mermaid") {
        return "[revised diagram]".to_string();
    }
    block_summary(block)
}

/// Newlines become spaces and horizontal whitespace is trimmed. Over 60
/// characters, the text is cut at the last space within the first 60 (or at
/// 60 when there is none) and `…` is appended.
///
/// Counts `char`s where Swift counts grapheme clusters.
pub fn summarize(raw: &str) -> String {
    let text = raw.replace('\n', " ");
    let text = text.trim_matches(is_horizontal_whitespace);
    if text.chars().count() <= SUMMARY_LIMIT {
        return text.to_string();
    }
    let cut = text
        .char_indices()
        .nth(SUMMARY_LIMIT)
        .map_or(text.len(), |(i, _)| i);
    let prefix = &text[..cut];
    match prefix.rfind(' ') {
        Some(space) => format!("{}…", &prefix[..space]),
        None => format!("{prefix}…"),
    }
}

/// Foundation's `CharacterSet.whitespaces`: tab and the space separators,
/// not line breaks.
fn is_horizontal_whitespace(c: char) -> bool {
    c.is_whitespace()
        && !matches!(
            c,
            '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockKind;
    use crate::plan::DEFAULT_WORD_DIFF_THRESHOLD;
    use crate::test_support::doc;
    use ChangeType::{Deletion, Insertion};

    fn changes(old: &str, new: &str) -> Vec<DocumentChange> {
        compute_changes(&ChangePlan::build(
            &doc(old),
            &doc(new),
            DEFAULT_WORD_DIFF_THRESHOLD,
        ))
    }

    /// (id, type, line, consecutive, group, index)
    fn rows(list: &[DocumentChange]) -> Vec<(&str, ChangeType, usize, bool, &str, usize)> {
        list.iter()
            .map(|c| {
                (
                    c.id.as_str(),
                    c.ty,
                    c.source_line,
                    c.is_consecutive,
                    c.group_id.as_str(),
                    c.group_index,
                )
            })
            .collect()
    }

    fn entry(
        id: &str,
        ty: ChangeType,
        group_id: &str,
        group_index: usize,
        is_mixed: bool,
        summary: &str,
    ) -> DocumentChange {
        DocumentChange {
            id: id.to_string(),
            ty,
            summary: summary.to_string(),
            source_line: 1,
            is_consecutive: false,
            group_id: group_id.to_string(),
            group_index,
            is_mixed,
        }
    }

    /// Ported from `ChangeGroupTests.swift`.
    mod change_group_tests {
        use super::*;

        #[test]
        fn collapses_entries_sharing_a_group_id() {
            let groups = ChangeGroup::build(&[
                entry("change-1", Insertion, "group-1", 1, false, "s"),
                entry("change-2", Insertion, "group-1", 1, false, "s"),
                entry("change-3", Insertion, "group-2", 2, false, "s"),
            ]);
            assert_eq!(groups.len(), 2);
            assert_eq!(groups[0].change_ids, ["change-1", "change-2"]);
            assert_eq!(groups[0].count(), 2);
            assert_eq!(groups[1].change_ids, ["change-3"]);
        }

        #[test]
        fn preserves_first_occurrence_order() {
            let groups = ChangeGroup::build(&[
                entry("change-2", Deletion, "group-2", 2, false, "s"),
                entry("change-1", Insertion, "group-1", 1, false, "s"),
                entry("change-3", Insertion, "group-2", 2, false, "s"),
            ]);
            let ids: Vec<&str> = groups.iter().map(|g| g.group_id.as_str()).collect();
            assert_eq!(ids, ["group-2", "group-1"]);
        }

        #[test]
        fn group_is_mixed_when_both_types_present() {
            let groups = ChangeGroup::build(&[
                entry("change-1", Deletion, "group-1", 1, false, "s"),
                entry("change-2", Insertion, "group-1", 1, false, "s"),
            ]);
            assert_eq!(groups.len(), 1);
            assert!(groups[0].is_mixed);
            assert_eq!(groups[0].ty, Insertion);
        }

        #[test]
        fn member_mixed_flag_propagates() {
            let groups =
                ChangeGroup::build(&[entry("change-2", Insertion, "group-1", 1, true, "s")]);
            assert!(groups[0].is_mixed);
        }

        #[test]
        fn group_index_and_members_come_from_entries() {
            let groups = ChangeGroup::build(&[
                entry("change-1", Deletion, "group-3", 3, false, "old line"),
                entry("change-1", Insertion, "group-3", 3, false, "new line"),
            ]);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].index, 3);
            assert_eq!(
                groups[0].members,
                vec![
                    (Deletion, "old line".to_string()),
                    (Insertion, "new line".to_string())
                ]
            );
        }

        #[test]
        fn empty_input_produces_no_groups() {
            assert!(ChangeGroup::build(&[]).is_empty());
        }

        #[test]
        fn deletion_only_group_is_a_deletion() {
            let groups =
                ChangeGroup::build(&[entry("change-1", Deletion, "group-1", 1, false, "s")]);
            assert_eq!((groups[0].ty, groups[0].is_mixed), (Deletion, false));
        }
    }

    /// `ChangeIDParityTests.corpus` through the sidebar projection, plus
    /// the host-line and consecutive rules of `ChangeList.swift`.
    mod compute_changes_tests {
        use super::*;

        #[test]
        fn paragraph_edit() {
            let list = changes("Alpha one.\n\nBeta.\n", "Alpha two.\n\nBeta.\n");
            assert_eq!(
                rows(&list),
                vec![
                    ("change-1", Deletion, 1, false, "group-1", 1),
                    ("change-2", Insertion, 1, true, "group-1", 1),
                ]
            );
            assert_eq!(list[0].summary, "Alpha one.");
            assert!(list.iter().all(|c| c.is_mixed));
        }

        #[test]
        fn insertion_only() {
            let list = changes("Alpha.\n", "Alpha.\n\nAdded.\n");
            assert_eq!(
                rows(&list),
                vec![("change-1", Insertion, 3, false, "group-1", 1)]
            );
        }

        #[test]
        fn trailing_deletion_hosts_on_the_preceding_anchor() {
            let list = changes("Alpha.\n\nGone.\n", "Alpha.\n");
            assert_eq!(
                rows(&list),
                vec![("change-1", Deletion, 1, false, "group-1", 1)]
            );
        }

        #[test]
        fn deletion_hosts_on_the_following_anchor() {
            let list = changes("Alpha.\n\nGone.\n\nOmega.\n", "Alpha.\n\nOmega.\n");
            assert_eq!(list[0].source_line, 3);
        }

        #[test]
        fn deletion_with_no_anchor_hosts_on_the_last_surviving_line() {
            let list = changes("Gone.\n", "");
            assert_eq!(
                rows(&list),
                vec![("change-1", Deletion, 1, false, "group-1", 1)]
            );
        }

        #[test]
        fn code_block_edit_alone_lists_each_changed_line() {
            let list = changes("```\nkeep\nold\n```\n", "```\nkeep\nnew\n```\n");
            assert_eq!(
                rows(&list),
                vec![
                    ("change-3", Deletion, 1, false, "group-1", 1),
                    ("change-3", Insertion, 1, true, "group-1", 0),
                ]
            );
            assert_eq!(
                (list[0].summary.as_str(), list[1].summary.as_str()),
                ("old", "new")
            );
            assert!(list.iter().all(|c| !c.is_mixed));
        }

        #[test]
        fn code_block_edit_then_paragraph_edit() {
            let list = changes(
                "```\nkeep\nold\n```\n\nMiddle.\n\nTail one.\n",
                "```\nkeep\nnew\n```\n\nMiddle.\n\nTail two.\n",
            );
            assert_eq!(
                rows(&list),
                vec![
                    ("change-3", Deletion, 1, false, "group-1", 1),
                    ("change-3", Insertion, 1, true, "group-1", 0),
                    ("change-4", Deletion, 8, false, "group-2", 2),
                    ("change-5", Insertion, 8, true, "group-2", 2),
                ]
            );
        }

        #[test]
        fn code_block_edit_and_insertion_in_the_same_gap() {
            let list = changes(
                "```\nkeep\nold\n```\n\nTail.\n",
                "```\nkeep\nnew\n```\n\nAdded.\n\nTail.\n",
            );
            assert_eq!(
                rows(&list),
                vec![
                    ("change-4", Deletion, 1, false, "group-1", 1),
                    ("change-4", Insertion, 1, true, "group-1", 0),
                    ("change-3", Insertion, 6, true, "group-2", 2),
                ]
            );
        }

        #[test]
        fn multi_cluster_code_block_edit_then_paragraph_edit() {
            let list = changes(
                "```\na\nb\nc\nd\ne\n```\n\nTail one.\n",
                "```\na\nB\nc\nD\ne\n```\n\nTail two.\n",
            );
            assert_eq!(
                rows(&list),
                vec![
                    ("change-2", Deletion, 1, false, "group-1", 1),
                    ("change-5", Deletion, 1, false, "group-2", 2),
                    ("change-5", Insertion, 1, true, "group-2", 0),
                    ("change-6", Deletion, 1, false, "group-3", 3),
                    ("change-6", Insertion, 1, true, "group-3", 0),
                    ("change-4", Insertion, 9, false, "group-4", 4),
                ]
            );
            assert_eq!(ChangeGroup::build(&list).len(), 4);
        }

        #[test]
        fn paragraph_and_code_block_swap_positions_in_one_gap() {
            let list = changes(
                "Intro.\n\nAlpha beta gamma.\n\n```\nkeep\nold\n```\n\nTail.\n",
                "Intro.\n\n```\nkeep\nnew\n```\n\nDelta epsilon zeta.\n\nTail.\n",
            );
            assert_eq!(
                rows(&list),
                vec![
                    ("change-1", Deletion, 3, false, "group-1", 1),
                    ("change-5", Deletion, 3, false, "group-2", 2),
                    ("change-5", Insertion, 3, true, "group-2", 0),
                    ("change-4", Insertion, 8, true, "group-3", 3),
                ]
            );
        }

        #[test]
        fn paragraph_deleted_and_code_block_edited_in_one_gap() {
            let list = changes(
                "Gone paragraph.\n\n```\nkeep\nold\n```\n\nTail.\n",
                "```\nkeep\nnew\n```\n\nTail.\n",
            );
            assert_eq!(
                rows(&list),
                vec![
                    ("change-1", Deletion, 1, false, "group-1", 1),
                    ("change-4", Deletion, 1, false, "group-2", 2),
                    ("change-4", Insertion, 1, true, "group-2", 0),
                ]
            );
        }

        #[test]
        fn mermaid_deletion_shows_a_placeholder_and_its_insertion_is_mixed() {
            let list = changes("```mermaid\ngraph A\n```\n", "```mermaid\ngraph B\n```\n");
            assert_eq!(list[0].summary, "[revised diagram]");
            assert_eq!(list[1].summary, "graph B");
            assert!(list[1].is_mixed);
        }

        #[test]
        fn every_corpus_case_produces_changes_and_identical_input_none() {
            assert!(changes("A.\n\nB.\n", "A.\n\nB.\n").is_empty());
            assert!(!changes("```\nx\n```\n", "```\ny\n```\n").is_empty());
        }
    }

    mod summary_tests {
        use super::*;

        fn leaf(tag: LeafTag, source: &str) -> LeafBlock {
            LeafBlock::new(BlockKind::Prose, tag, 1, 0..source.len(), source)
        }

        #[test]
        fn short_text_is_kept() {
            assert_eq!(summarize("  hello world \t"), "hello world");
        }

        #[test]
        fn newlines_become_spaces() {
            assert_eq!(summarize("one\ntwo"), "one two");
        }

        #[test]
        fn exactly_sixty_characters_is_not_truncated() {
            let s = "a".repeat(60);
            assert_eq!(summarize(&s), s);
        }

        #[test]
        fn long_text_is_cut_at_the_last_space_within_sixty() {
            let s = format!("{} {}", "a".repeat(50), "b".repeat(20));
            assert_eq!(summarize(&s), format!("{}…", "a".repeat(50)));
        }

        #[test]
        fn long_text_without_spaces_is_cut_hard() {
            assert_eq!(summarize(&"x".repeat(70)), format!("{}…", "x".repeat(60)));
        }

        #[test]
        fn counts_characters_not_bytes() {
            let s = "é".repeat(60);
            assert_eq!(summarize(&s), s);
        }

        #[test]
        fn prose_uses_inline_text() {
            let b = leaf(LeafTag::Heading(2), "## The **bold** title")
                .with_inline_text("The bold title");
            assert_eq!(block_summary(&b), "The bold title");
            let b = leaf(LeafTag::ListItem, "- item *x*").with_inline_text("item x");
            assert_eq!(block_summary(&b), "item x");
        }

        #[test]
        fn prose_without_inline_text_falls_back_to_source() {
            assert_eq!(
                block_summary(&leaf(LeafTag::Paragraph, "raw *text*")),
                "raw *text*"
            );
        }

        #[test]
        fn code_uses_its_first_line() {
            let b =
                leaf(LeafTag::CodeBlock, "```\nfirst\nsecond\n```").with_code("first\nsecond\n");
            assert_eq!(block_summary(&b), "first");
        }

        #[test]
        fn other_blocks_use_their_source() {
            assert_eq!(
                block_summary(&leaf(LeafTag::TableRow, "| a | b |").with_inline_text("ab")),
                "| a | b |"
            );
            assert_eq!(block_summary(&leaf(LeafTag::ThematicBreak, "---")), "---");
        }

        #[test]
        fn mermaid_deletion_is_a_placeholder_but_its_insertion_is_not() {
            let b = leaf(LeafTag::CodeBlock, "```mermaid\ngraph\n```")
                .with_language(Some("mermaid"))
                .with_code("graph\n");
            assert_eq!(deletion_summary(&b), "[revised diagram]");
            assert_eq!(block_summary(&b), "graph");
        }
    }
}

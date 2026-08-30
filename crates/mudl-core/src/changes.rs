//! Wiring `mudl-diff`'s `ChangePlan` into `render_up`/`render_down` as an
//! optional overlay (Phase 13.8 of `docs/IMPLEMENTATION-PLAN.md`).
//!
//! Scope note: `collect_leaf_blocks` tracks paragraphs, headings, and
//! fenced/indented code blocks — the block kinds `mudl-diff`'s `BlockKind`
//! already models well (`Prose` and `CodeBlock`). Lists, tables,
//! blockquotes, thematic breaks, and raw HTML blocks render exactly as
//! they did before this phase (no change-tracking wrapper), rather than
//! porting `BlockMatcher`'s full leaf-block taxonomy — an explicit,
//! honest scope reduction from the Swift source, in the spirit of this
//! plan's own guidance that "port" means matching behavior, not
//! translating every line. Extending `collect_leaf_blocks` to more block
//! kinds later is additive: it doesn't change this module's shape.

use std::collections::HashMap;

use pulldown_cmark::{Event, Tag};

use mudl_diff::block::{fingerprint as block_fingerprint, BlockKind, LeafBlock};
use mudl_diff::line;
use mudl_diff::plan::{ChangePlan, GroupInfo, InsertionSlot};

use crate::encoding::html_escape;
use crate::options::RenderOptions;
use crate::parse::ParsedMarkdown;

/// Walks `markdown`'s event stream collecting a [`LeafBlock`] for every
/// paragraph, heading, and code block, each carrying its byte range in
/// `markdown` (a `Start` event's range already spans its whole block in
/// `pulldown-cmark`'s offset iterator, matching End included, so no
/// separate scan for the closing tag is needed).
pub fn collect_leaf_blocks(markdown: &str) -> Vec<LeafBlock> {
    let parsed = ParsedMarkdown::new(markdown);
    let mut blocks = Vec::new();
    for (event, range) in &parsed.events {
        let (kind, language) = match event {
            Event::Start(Tag::Paragraph) | Event::Start(Tag::Heading { .. }) => {
                (BlockKind::Prose, None)
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(info) if !info.is_empty() => {
                        Some(info.to_string())
                    }
                    _ => None,
                };
                (BlockKind::CodeBlock, language)
            }
            _ => continue,
        };
        let source_line = markdown[..range.start].matches('\n').count() + 1;
        blocks.push(LeafBlock {
            kind,
            source_line,
            source_text: markdown[range.start..range.end].to_string(),
            language,
            range: Some((range.start, range.end)),
        });
    }
    blocks
}

/// Change-tracking markup to wrap a block with, keyed by its byte-range
/// start in the *new* document.
struct BlockWrap {
    change_id: String,
    group_id: String,
}

/// The precomputed change-tracking overlay for one `render_up` call: where
/// to splice in deleted content, and which surviving blocks to wrap in
/// `<ins>`.
pub struct UpOverlay {
    /// New-document block start offset -> wrap info for that (inserted or
    /// paired-new) block.
    inserted_wraps: HashMap<usize, BlockWrap>,
    /// New-document block start offset -> rendered `<del>` HTML to splice
    /// immediately before that block.
    deletions_before: HashMap<usize, String>,
    /// Rendered `<del>` HTML for a gap with no following anchor (deletions
    /// at the very end of the document).
    trailing_deletions: String,
}

fn group_id_for<'a>(group_info: &'a HashMap<String, GroupInfo>, change_id: &str) -> &'a str {
    group_info
        .get(change_id)
        .map(|g| g.group_id.as_str())
        .unwrap_or("")
}

fn render_deleted_block(block: &LeafBlock, change_id: &str, group_id: &str) -> String {
    let inner = crate::render::render_up(&block.source_text, &RenderOptions::default());
    format!(
        "<del class=\"mudl-change\" data-change-id=\"{}\" data-group-id=\"{}\">{}</del>",
        html_escape(change_id),
        html_escape(group_id),
        inner
    )
}

/// Builds the Up-mode overlay from a [`ChangePlan`] already computed over
/// `old`/`new` leaf blocks.
fn build_up_overlay(plan: &ChangePlan) -> UpOverlay {
    let mut inserted_wraps: HashMap<usize, BlockWrap> = HashMap::new();
    let mut deletions_before: HashMap<usize, String> = HashMap::new();
    let mut trailing_deletions = String::new();

    for gap in &plan.gaps {
        let mut deleted_html = String::new();
        for del in &gap.deletions {
            let group_id = group_id_for(&plan.group_info, &del.change_id);
            deleted_html.push_str(&render_deleted_block(&del.block, &del.change_id, group_id));
        }
        for slot in &gap.insertion_slots {
            if let InsertionSlot::CodeBlockPair(pair) = slot {
                let group_id = group_id_for(&plan.group_info, &pair.deletion.change_id);
                deleted_html.push_str(&render_deleted_block(
                    &pair.deletion.block,
                    &pair.deletion.change_id,
                    group_id,
                ));
            }
        }
        if !deleted_html.is_empty() {
            match &gap.following_anchor {
                Some(anchor) => {
                    if let Some((start, _)) = anchor.range {
                        deletions_before
                            .entry(start)
                            .or_default()
                            .push_str(&deleted_html);
                    }
                }
                None => trailing_deletions.push_str(&deleted_html),
            }
        }

        for slot in &gap.insertion_slots {
            let (block, change_id) = match slot {
                InsertionSlot::Block(change) => (&change.block, &change.change_id),
                InsertionSlot::CodeBlockPair(pair) => {
                    (&pair.insertion.block, &pair.insertion.change_id)
                }
            };
            if let Some((start, _)) = block.range {
                let group_id = group_id_for(&plan.group_info, change_id).to_string();
                inserted_wraps.insert(
                    start,
                    BlockWrap {
                        change_id: change_id.clone(),
                        group_id,
                    },
                );
            }
        }
    }

    UpOverlay {
        inserted_wraps,
        deletions_before,
        trailing_deletions,
    }
}

/// Computes the [`ChangePlan`] for diffing `old_markdown` against
/// `new_markdown` (both already frontmatter-stripped) over the block kinds
/// [`collect_leaf_blocks`] tracks. Public so a GUI sidebar (Phase 13.9) can
/// list the plan's change groups without re-deriving this wiring —
/// `up_overlay` is just this plus [`build_up_overlay`].
pub fn up_change_plan(
    old_markdown: &str,
    new_markdown: &str,
    word_diff_threshold: f64,
) -> ChangePlan {
    let old_blocks = collect_leaf_blocks(old_markdown);
    let new_blocks = collect_leaf_blocks(new_markdown);
    let matches = mudl_diff::block::match_blocks(&old_blocks, &new_blocks);
    ChangePlan::build(&matches, word_diff_threshold)
}

/// Computes the Up-mode overlay for diffing `old_markdown` (already
/// frontmatter-stripped) against `new_markdown` (ditto).
pub fn up_overlay(old_markdown: &str, new_markdown: &str, word_diff_threshold: f64) -> UpOverlay {
    build_up_overlay(&up_change_plan(
        old_markdown,
        new_markdown,
        word_diff_threshold,
    ))
}

/// One entry in a "Changes" sidebar: a change group's ID, content type,
/// and how many changes it holds, in ascending group order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSummary {
    pub group_id: String,
    pub group_type: mudl_diff::plan::GroupType,
    pub change_count: usize,
}

/// Summarizes `plan`'s change groups (both the Up-mode block-level groups
/// and the Down-mode line-level groups a caller passes in the same
/// `ChangePlan` shape) in ascending `group_index` order, one entry per
/// distinct group ID.
pub fn group_summaries(plan: &ChangePlan) -> Vec<GroupSummary> {
    let mut by_group: HashMap<&str, (mudl_diff::plan::GroupType, usize, usize)> = HashMap::new();
    for info in plan.group_info.values() {
        let entry = by_group.entry(info.group_id.as_str()).or_insert((
            info.group_type,
            info.group_index,
            0,
        ));
        entry.2 += 1;
    }
    let mut indexed: Vec<(usize, GroupSummary)> = by_group
        .into_iter()
        .map(|(group_id, (group_type, group_index, change_count))| {
            (
                group_index,
                GroupSummary {
                    group_id: group_id.to_string(),
                    group_type,
                    change_count,
                },
            )
        })
        .collect();
    indexed.sort_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, summary)| summary).collect()
}

impl UpOverlay {
    pub(crate) fn deletions_before(&self, start: usize) -> Option<&str> {
        self.deletions_before.get(&start).map(String::as_str)
    }

    pub(crate) fn inserted_open_tag(&self, start: usize) -> Option<String> {
        self.inserted_wraps.get(&start).map(|w| {
            format!(
                "<ins class=\"mudl-change\" data-change-id=\"{}\" data-group-id=\"{}\">",
                html_escape(&w.change_id),
                html_escape(&w.group_id)
            )
        })
    }

    pub(crate) fn trailing_deletions(&self) -> &str {
        &self.trailing_deletions
    }
}

/// Down-mode change annotation for one rendered line.
pub enum LineAnnotation {
    Unchanged,
    Inserted,
    Deleted,
}

/// One line to render in Down-mode overlay order: its text, the document
/// line number to show, and its change annotation (with change/group IDs
/// when annotated).
pub struct OverlayLine<'a> {
    pub text: &'a str,
    pub display_line: usize,
    pub annotation: LineAnnotation,
    pub change_id: Option<String>,
    pub group_id: Option<String>,
}

/// Reconstructs Down mode's complete line sequence from
/// [`mudl_diff::line::diff`] (which, unlike [`ChangePlan`]'s gaps, records
/// every line — changed or not — so it alone can drive reconstruction
/// order). Change/group IDs are looked up from a `ChangePlan` built
/// separately over the same lines as `Verbatim` leaf blocks (exact-match
/// fingerprint, matching Down mode's "compare raw text exactly" semantics):
/// since both use the same `lcs` engine over equivalent comparisons, every
/// non-unchanged `LineChange` has a matching entry in that plan.
pub fn down_overlay_lines<'a>(
    old_markdown: &'a str,
    new_markdown: &'a str,
    word_diff_threshold: f64,
) -> Vec<OverlayLine<'a>> {
    let old_lines: Vec<&str> = old_markdown.lines().collect();
    let new_lines: Vec<&str> = new_markdown.lines().collect();

    let Some(line_changes) = line::diff(&old_lines, &new_lines) else {
        // Identical: no annotations needed at all.
        return new_lines
            .iter()
            .enumerate()
            .map(|(i, text)| OverlayLine {
                text,
                display_line: i + 1,
                annotation: LineAnnotation::Unchanged,
                change_id: None,
                group_id: None,
            })
            .collect();
    };

    let leaf = |text: &str, line_no: usize| LeafBlock {
        kind: BlockKind::Verbatim,
        source_line: line_no,
        source_text: text.to_string(),
        language: None,
        range: None,
    };
    let old_blocks: Vec<LeafBlock> = old_lines
        .iter()
        .enumerate()
        .map(|(i, l)| leaf(l, i + 1))
        .collect();
    let new_blocks: Vec<LeafBlock> = new_lines
        .iter()
        .enumerate()
        .map(|(i, l)| leaf(l, i + 1))
        .collect();
    debug_assert!(old_blocks
        .iter()
        .all(|b| block_fingerprint(b) == b.source_text));

    let matches = mudl_diff::block::match_blocks(&old_blocks, &new_blocks);
    let plan = ChangePlan::build(&matches, word_diff_threshold);

    // old/new 1-based line number -> (change_id, group_id).
    let mut deleted_ids: HashMap<usize, (String, String)> = HashMap::new();
    let mut inserted_ids: HashMap<usize, (String, String)> = HashMap::new();
    for gap in &plan.gaps {
        for del in &gap.deletions {
            let group_id = group_id_for(&plan.group_info, &del.change_id).to_string();
            deleted_ids.insert(del.block.source_line, (del.change_id.clone(), group_id));
        }
        for slot in &gap.insertion_slots {
            let InsertionSlot::Block(change) = slot else {
                unreachable!("line blocks are never CodeBlock-kind, so never pair as code")
            };
            let group_id = group_id_for(&plan.group_info, &change.change_id).to_string();
            inserted_ids.insert(
                change.block.source_line,
                (change.change_id.clone(), group_id),
            );
        }
    }

    line_changes
        .into_iter()
        .map(|lc| {
            let line_no = lc.source_index + 1;
            match lc.annotation {
                line::Annotation::Unchanged => OverlayLine {
                    text: new_lines[lc.source_index],
                    display_line: line_no,
                    annotation: LineAnnotation::Unchanged,
                    change_id: None,
                    group_id: None,
                },
                line::Annotation::Inserted => {
                    let ids = inserted_ids.get(&line_no);
                    OverlayLine {
                        text: new_lines[lc.source_index],
                        display_line: line_no,
                        annotation: LineAnnotation::Inserted,
                        change_id: ids.map(|(id, _)| id.clone()),
                        group_id: ids.map(|(_, gid)| gid.clone()),
                    }
                }
                line::Annotation::Deleted => {
                    let ids = deleted_ids.get(&line_no);
                    OverlayLine {
                        text: old_lines[lc.source_index],
                        display_line: line_no,
                        annotation: LineAnnotation::Deleted,
                        change_id: ids.map(|(id, _)| id.clone()),
                        group_id: ids.map(|(_, gid)| gid.clone()),
                    }
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_paragraphs_and_headings() {
        let blocks = collect_leaf_blocks("# Title\n\nFirst paragraph.\n\nSecond one.\n");
        assert_eq!(blocks.len(), 3);
        assert!(blocks.iter().all(|b| b.kind == BlockKind::Prose));
        assert_eq!(blocks[0].source_text, "# Title\n");
        assert_eq!(blocks[1].source_text, "First paragraph.\n");
    }

    #[test]
    fn collects_code_blocks_with_language() {
        let blocks = collect_leaf_blocks("```rust\nfn f() {}\n```\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::CodeBlock);
        assert_eq!(blocks[0].language.as_deref(), Some("rust"));
    }

    #[test]
    fn bare_fence_has_no_language() {
        let blocks = collect_leaf_blocks("```\nplain\n```\n");
        assert_eq!(blocks[0].language, None);
    }

    #[test]
    fn ignores_lists_and_tables() {
        let blocks = collect_leaf_blocks("- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(blocks.is_empty());
    }

    #[test]
    fn source_line_is_one_based() {
        let blocks = collect_leaf_blocks("first\n\nsecond\n\nthird\n");
        assert_eq!(blocks[0].source_line, 1);
        assert_eq!(blocks[1].source_line, 3);
        assert_eq!(blocks[2].source_line, 5);
    }

    #[test]
    fn up_overlay_wraps_an_inserted_paragraph() {
        let overlay = up_overlay("First.\n", "First.\n\nSecond.\n", 0.25);
        let blocks = collect_leaf_blocks("First.\n\nSecond.\n");
        let second_start = blocks[1].range.unwrap().0;
        assert!(overlay.inserted_open_tag(second_start).is_some());
    }

    #[test]
    fn up_overlay_records_a_deletion_before_the_following_anchor() {
        let overlay = up_overlay("First.\n\nGone.\n\nLast.\n", "First.\n\nLast.\n", 0.25);
        let blocks = collect_leaf_blocks("First.\n\nLast.\n");
        let last_start = blocks[1].range.unwrap().0;
        let del = overlay.deletions_before(last_start).unwrap();
        assert!(del.contains("Gone."));
        assert!(del.contains("mudl-change"));
    }

    #[test]
    fn up_overlay_no_changes_produces_no_wraps() {
        let overlay = up_overlay("Same.\n", "Same.\n", 0.25);
        let blocks = collect_leaf_blocks("Same.\n");
        assert!(overlay
            .inserted_open_tag(blocks[0].range.unwrap().0)
            .is_none());
    }

    #[test]
    fn down_overlay_identical_content_is_all_unchanged() {
        let lines = down_overlay_lines("a\nb\n", "a\nb\n", 0.25);
        assert_eq!(lines.len(), 2);
        assert!(lines
            .iter()
            .all(|l| matches!(l.annotation, LineAnnotation::Unchanged)));
    }

    #[test]
    fn down_overlay_marks_inserted_and_deleted_lines() {
        let lines = down_overlay_lines("a\nb\nc\n", "a\nx\nc\n", 0.25);
        let annotations: Vec<&str> = lines
            .iter()
            .map(|l| match l.annotation {
                LineAnnotation::Unchanged => "u",
                LineAnnotation::Inserted => "i",
                LineAnnotation::Deleted => "d",
            })
            .collect();
        assert_eq!(annotations, vec!["u", "d", "i", "u"]);
        assert_eq!(lines[1].text, "b");
        assert_eq!(lines[2].text, "x");
    }

    // --- group_summaries ---

    #[test]
    fn no_changes_summarizes_to_no_groups() {
        let plan = up_change_plan("Same.\n", "Same.\n", 0.25);
        assert!(group_summaries(&plan).is_empty());
    }

    #[test]
    fn adjacent_changes_summarize_to_one_mixed_group() {
        let plan = up_change_plan(
            "First.\n\nOld one.\n\nLast.\n",
            "First.\n\nNew one.\n\nLast.\n",
            0.25,
        );
        let summaries = group_summaries(&plan);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].group_type, mudl_diff::plan::GroupType::Mix);
        assert_eq!(summaries[0].change_count, 2);
    }

    #[test]
    fn separate_gaps_summarize_in_ascending_order() {
        let plan = up_change_plan(
            "Old one.\n\nAnchor.\n\nOld two.\n",
            "New one.\n\nAnchor.\n\nNew two.\n",
            0.25,
        );
        let summaries = group_summaries(&plan);
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].group_id, "group-1");
        assert_eq!(summaries[1].group_id, "group-2");
    }
}

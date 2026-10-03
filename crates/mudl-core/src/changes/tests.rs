use super::*;
use mudl_diff::block::{match_blocks, BlockKind, BlockMatch, LeafTag};
use mudl_diff::list::ChangeType::{self, Deletion, Insertion};

use DefinitionPolicy::{DescendPlainFootnotes, SkipAll};

const BOTH: [DefinitionPolicy; 2] = [SkipAll, DescendPlainFootnotes];

fn blocks(md: &str) -> Vec<LeafBlock> {
    leaves(md, SkipAll)
}

fn changes(old: &str, new: &str, policy: DefinitionPolicy) -> Vec<DocumentChange> {
    compute_changes(old, new, policy, DEFAULT_WORD_DIFF_THRESHOLD)
}

/// (kind, tag, line, source_text) per leaf.
fn shape(blocks: &[LeafBlock]) -> Vec<(BlockKind, LeafTag, usize, &str)> {
    blocks
        .iter()
        .map(|b| (b.kind, b.tag, b.source_line, b.source_text.as_str()))
        .collect()
}

/// A parser-neutral record of one match (`BlockMatcherTests.MatchRecord`):
/// (kind, old source, new source).
fn records(
    old: &str,
    new: &str,
    policy: DefinitionPolicy,
) -> Vec<(&'static str, Option<String>, Option<String>)> {
    let (o, n) = (leaves(old, policy), leaves(new, policy));
    match_blocks(&o, &n)
        .into_iter()
        .map(|m| match m {
            BlockMatch::Unchanged(a, b) => (
                "unchanged",
                Some(o[a].source_text.clone()),
                Some(n[b].source_text.clone()),
            ),
            BlockMatch::Deleted(a) => ("deleted", Some(o[a].source_text.clone()), None),
            BlockMatch::Inserted(b) => ("inserted", None, Some(n[b].source_text.clone())),
        })
        .collect()
}

fn all_unchanged(old: &str, new: &str, policy: DefinitionPolicy) -> bool {
    records(old, new, policy)
        .iter()
        .all(|(k, _, _)| *k == "unchanged")
}

fn has(records: &[(&str, Option<String>, Option<String>)], kind: &str) -> bool {
    records.iter().any(|(k, _, _)| *k == kind)
}

/// One test per row of the §8.1 collection table.
mod collection_table {
    use super::*;

    #[test]
    fn paragraph_heading_code_rule_and_html_are_leaves() {
        let md = "# Title\n\nPara one\nwraps.\n\n```rust\nfn x() {}\n```\n\n---\n\n<div>\nhi\n</div>\n\nSetext\n===\n";
        let b = blocks(md);
        assert_eq!(
            shape(&b),
            vec![
                (BlockKind::Prose, LeafTag::Heading(1), 1, "# Title"),
                (BlockKind::Prose, LeafTag::Paragraph, 3, "Para one\nwraps."),
                (
                    BlockKind::CodeBlock,
                    LeafTag::CodeBlock,
                    6,
                    "```rust\nfn x() {}\n```"
                ),
                (BlockKind::Verbatim, LeafTag::ThematicBreak, 10, "---"),
                (
                    BlockKind::Verbatim,
                    LeafTag::HtmlBlock,
                    12,
                    "<div>\nhi\n</div>"
                ),
                (BlockKind::Prose, LeafTag::Heading(1), 16, "Setext\n==="),
            ]
        );
        assert_eq!(b[0].inline_text.as_deref(), Some("Title"));
        assert_eq!(b[1].inline_text.as_deref(), Some("Para one wraps."));
        assert_eq!(b[2].language.as_deref(), Some("rust"));
        assert_eq!(b[2].code.as_deref(), Some("fn x() {}\n"));
        assert!(b[3].inline_text.is_none() && b[4].inline_text.is_none());
    }

    #[test]
    fn byte_range_is_the_start_event_range_and_source_text_is_trimmed() {
        let md = "Alpha.\n\n\n\nBeta.\n";
        let b = blocks(md);
        assert_eq!(&md[b[0].byte_range.clone()], "Alpha.\n");
        assert_eq!(b[0].source_text, "Alpha.");
        assert_eq!(b[1].source_line, 5);
        // A loose last item's range swallows trailing blank lines; the
        // source text drops them (`extractSourceText`).
        let md = "- a\n\n- b\n\n\n";
        let b = blocks(md);
        assert_eq!(b[1].source_text, "- b");
        assert!(md[b[1].byte_range.clone()].starts_with("- b"));
    }

    #[test]
    fn indented_code_has_no_language_and_keeps_whole_lines() {
        let b = blocks("Para.\n\n    let x = 1;\n    let y = 2;\n");
        assert_eq!(b[1].kind, BlockKind::CodeBlock);
        assert_eq!(b[1].language, None);
        assert_eq!(b[1].code.as_deref(), Some("let x = 1;\nlet y = 2;\n"));
        assert_eq!(b[1].source_text, "    let x = 1;\n    let y = 2;");
    }

    #[test]
    fn fence_language_is_the_full_info_string() {
        let b = blocks("```rust title=\"x\"\nfn\n```\n");
        assert_eq!(b[0].language.as_deref(), Some("rust title=\"x\""));
    }

    #[test]
    fn blockquote_table_and_list_descend() {
        let md = "> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- item\n";
        let tags: Vec<LeafTag> = blocks(md).iter().map(|b| b.tag).collect();
        assert_eq!(
            tags,
            vec![
                LeafTag::Paragraph,
                LeafTag::TableHead,
                LeafTag::TableRow,
                LeafTag::ListItem
            ]
        );
    }

    #[test]
    fn table_head_is_one_leaf_with_cell_text_concatenated() {
        let b = blocks("| a | *b* |\n|---|---|\n| 1 | `2` |\n");
        assert_eq!(
            shape(&b),
            vec![
                (BlockKind::TableRow, LeafTag::TableHead, 1, "| a | *b* |"),
                (BlockKind::TableRow, LeafTag::TableRow, 3, "| 1 | `2` |"),
            ]
        );
        assert_eq!(b[0].inline_text.as_deref(), Some("ab"));
        assert_eq!(b[1].inline_text.as_deref(), Some("12"));
    }

    #[test]
    fn table_row_padding_is_cosmetic_but_a_cell_edit_is_not() {
        let old = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";
        let padded = "| a   | b |\n|-----|---|\n| 1   | 2 |\n| 3   | 4 |\n";
        assert!(all_unchanged(old, padded, SkipAll));
        let edited = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 5 |\n";
        let r = records(old, edited, SkipAll);
        assert_eq!(r[2], ("deleted", Some("| 3 | 4 |".into()), None));
        assert_eq!(r[3], ("inserted", None, Some("| 3 | 5 |".into())));
    }

    #[test]
    fn tight_item_with_only_inline_content_is_a_leaf() {
        let b = blocks("- one *two*\n- three\n");
        assert_eq!(
            shape(&b),
            vec![
                (BlockKind::Prose, LeafTag::ListItem, 1, "- one *two*"),
                (BlockKind::Prose, LeafTag::ListItem, 2, "- three"),
            ]
        );
        assert_eq!(b[0].inline_text.as_deref(), Some("one two"));
    }

    #[test]
    fn loose_item_with_one_paragraph_is_a_leaf() {
        let b = blocks("- one\n  wrapped\n\n- two\n");
        assert_eq!(
            shape(&b),
            vec![
                (BlockKind::Prose, LeafTag::ListItem, 1, "- one\n  wrapped"),
                (BlockKind::Prose, LeafTag::ListItem, 4, "- two"),
            ]
        );
        assert_eq!(b[0].inline_text.as_deref(), Some("one wrapped"));
    }

    #[test]
    fn item_with_nested_list_descends_and_cuts_its_own_text() {
        let md = "- parent\n  - child a\n  - child b\n- sibling\n";
        let b = blocks(md);
        assert_eq!(
            shape(&b),
            vec![
                (BlockKind::Prose, LeafTag::Paragraph, 1, "- parent"),
                (BlockKind::Prose, LeafTag::ListItem, 2, "  - child a"),
                (BlockKind::Prose, LeafTag::ListItem, 3, "  - child b"),
                (BlockKind::Prose, LeafTag::ListItem, 4, "- sibling"),
            ]
        );
        // The inline run's range stops before the nested list.
        assert_eq!(&md[b[0].byte_range.clone()], "parent");
        assert!(b[0].byte_range.end <= b[1].byte_range.start);
        assert_eq!(b[0].inline_text.as_deref(), Some("parent"));
    }

    #[test]
    fn loose_item_with_several_blocks_descends() {
        let md = "- para one\n\n  para two\n\n  ```\n  code\n  ```\n";
        let b = blocks(md);
        assert_eq!(
            b.iter().map(|b| (b.tag, b.source_line)).collect::<Vec<_>>(),
            vec![
                (LeafTag::Paragraph, 1),
                (LeafTag::Paragraph, 3),
                (LeafTag::CodeBlock, 5),
            ]
        );
        assert_eq!(b[0].source_text, "- para one");
        assert_eq!(b[2].code.as_deref(), Some("code\n"));
    }

    #[test]
    fn empty_item_yields_no_leaf() {
        assert_eq!(blocks("- a\n-\n- b\n").len(), 2);
    }

    #[test]
    fn ordered_item_ignores_its_marker() {
        let b = blocks("1. one\n2. two\n");
        assert!(b.iter().all(|b| b.kind == BlockKind::OrderedListItem));
        assert_eq!(b[1].source_text, "2. two");
        // Deleting the first item renumbers the second: only the deletion
        // is a change.
        let r = records("1. one\n2. two\n3. three\n", "1. two\n2. three\n", SkipAll);
        assert_eq!(
            r.iter().map(|(k, _, _)| *k).collect::<Vec<_>>(),
            vec!["deleted", "unchanged", "unchanged"]
        );
    }

    #[test]
    fn task_list_marker_is_part_of_the_source() {
        let b = blocks("- [ ] task\n");
        assert_eq!(b[0].source_text, "- [ ] task");
        assert_eq!(b[0].inline_text.as_deref(), Some("task"));
        let r = records("- [ ] task\n", "- [x] task\n", SkipAll);
        assert!(has(&r, "deleted") && has(&r, "inserted"));
    }

    #[test]
    fn alert_contributes_its_inner_paragraph() {
        let b = blocks("> [!NOTE]\n> Body text.\n\nNote: an aside.\n");
        assert_eq!(b[0].tag, LeafTag::Paragraph);
        assert_eq!(b[0].source_text, "> [!NOTE]\n> Body text.");
        // The tag stays in the stream; the renderer skips it.
        assert_eq!(b[0].inline_text.as_deref(), Some("[!NOTE] Body text."));
        let r = records(
            "> [!NOTE]\n> Body text.\n",
            "> [!NOTE]\n> Body text edited.\n",
            SkipAll,
        );
        assert!(has(&r, "deleted") && has(&r, "inserted"));
    }

    #[test]
    fn display_math_paragraph_is_verbatim_without_inline_text() {
        let b = blocks("$$ x = 1 $$\n\n```math\ny\n```\n");
        assert_eq!(
            (b[0].kind, b[0].tag, b[0].inline_text.as_deref()),
            (BlockKind::Verbatim, LeafTag::Paragraph, None)
        );
        assert_eq!(b[1].kind, BlockKind::CodeBlock);
        assert!(b[1].has_language("math"));
        assert!(b[1].inline_text.is_none());
    }

    #[test]
    fn math_edits_never_get_word_spans() {
        let p = plan(
            "$$ a + b + c + d $$\n",
            "$$ a + b + c + e $$\n",
            SkipAll,
            DEFAULT_WORD_DIFF_THRESHOLD,
        );
        assert_eq!(p.gaps.len(), 1);
        assert!(p.word_spans.is_empty());
        let p = plan(
            "```math\na + b\n```\n",
            "```math\na + c\n```\n",
            SkipAll,
            DEFAULT_WORD_DIFF_THRESHOLD,
        );
        assert!(p.word_spans.is_empty());
        assert!(p.gaps[0]
            .insertion_slots
            .iter()
            .all(|s| matches!(s, mudl_diff::plan::InsertionSlot::Block(_))));
    }

    #[test]
    fn inline_text_follows_the_renderer_stream() {
        let b = blocks(
            "A `code` and **bold**[^1] ![alt *x*](u.png) <b>hi</b>  \nnext\nsoft\n\n[^1]: Foot.\n",
        );
        assert_eq!(
            b[0].inline_text.as_deref(),
            Some("A code and bold alt x hi\nnext soft")
        );
    }

    #[test]
    fn footnote_definitions_follow_the_policy() {
        let md = "Text[^a] and[^comment-x].\n\n[^a]: Foot one.\n\n    Foot two.\n\n[^comment-x]: A note.\n\nAfter.\n";
        let skip = leaves(md, SkipAll);
        assert_eq!(
            skip.iter()
                .map(|b| b.source_text.as_str())
                .collect::<Vec<_>>(),
            vec!["Text[^a] and[^comment-x].", "After."]
        );
        let descend = leaves(md, DescendPlainFootnotes);
        assert_eq!(
            descend
                .iter()
                .map(|b| (b.source_line, b.source_text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (1, "Text[^a] and[^comment-x]."),
                (3, "[^a]: Foot one."),
                (5, "    Foot two."),
                (9, "After."),
            ]
        );
        // Sorted by line.
        assert!(descend
            .windows(2)
            .all(|w| w[0].source_line <= w[1].source_line));
    }

    #[test]
    fn frontmatter_is_never_diffed_and_coordinates_are_body_relative() {
        let old = "---\ntitle: One\n---\n# Head\n\nBody.\n";
        let new = "---\ntitle: Two\nextra: yes\n---\n# Head\n\nBody.\n";
        for policy in BOTH {
            assert!(changes(old, new, policy).is_empty());
        }
        let b = leaves(new, SkipAll);
        assert_eq!(b[0].source_line, 1);
        assert_eq!(b[0].byte_range.start, 0);
        assert_eq!(body(new), "# Head\n\nBody.\n");
        assert_eq!(body_line_offset(new), 4);
        assert_eq!(body_line_offset("# No frontmatter\n"), 0);
        // File line of "Body." in Down mode's numbering.
        assert_eq!(
            b[1].source_line + body_line_offset(new),
            new.lines().position(|l| l == "Body.").unwrap() + 1
        );
    }

    #[test]
    fn crlf_source_text_drops_carriage_returns() {
        let b = blocks("Line one\r\nline two\r\n\r\nNext\r\n");
        assert_eq!(b[0].source_text, "Line one\nline two");
        assert_eq!(b[1].source_line, 4);
    }
}

/// `BlockMatcherTests.swift` (default policy: skip all definitions).
mod block_matcher {
    use super::*;

    #[test]
    fn footnote_definition_produces_no_leaf_blocks() {
        let b = blocks(
            "Text with a footnote[^a].\n\n[^a]: The footnote body.\n\n    A second paragraph inside the definition.",
        );
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].tag, LeafTag::Paragraph);
    }

    #[test]
    fn comment_definition_produces_no_leaf_blocks() {
        let b = blocks(
            "Text with a comment[^comment-a].\n\n[^comment-a]: > a comment\n\n    💬 {Tester @ 2026-07-08 12:00:00}:\n\n    A comment thread body.",
        );
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].tag, LeafTag::Paragraph);
    }

    #[test]
    fn footnote_definition_body_edit_classifies_as_unchanged() {
        let r = records(
            "Text with a footnote[^a].\n\n[^a]: Old body.\n",
            "Text with a footnote[^a].\n\n[^a]: New body.\n",
            SkipAll,
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "unchanged");
    }

    #[test]
    fn comment_definition_body_edit_classifies_as_unchanged() {
        let def = |body: &str| {
            format!(
                "Text with a comment[^comment-a].\n\n[^comment-a]: > a comment\n\n    💬 {{Tester @ 2026-07-08 12:00:00}}:\n\n    {body}"
            )
        };
        let r = records(&def("Old thread body."), &def("New thread body."), SkipAll);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "unchanged");
    }

    #[test]
    fn added_comment_reference_classifies_as_unchanged() {
        let r = records(
            "The quick fox.\n",
            "The quick fox[^comment-a].\n\n[^comment-a]: > quick fox\n\n    💬 {Tester @ 2026-07-08 12:00:00}:\n\n    A note.",
            SkipAll,
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "unchanged");
    }

    #[test]
    fn rewrapped_paragraph_classifies_as_unchanged() {
        assert!(all_unchanged(
            "A message attribution at the start of a paragraph **begins a new\nmessage**. The first message needs no such attribution.",
            "A message attribution at the start of a paragraph **begins a\nnew message**. The first message needs no such attribution.",
            SkipAll,
        ));
    }

    #[test]
    fn rewrapped_list_item_classifies_as_unchanged() {
        let r = records(
            "- one workaround is to put empty braces at the\n  start.",
            "- one workaround is to put empty braces\n  at the start.",
            SkipAll,
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "unchanged");
    }

    #[test]
    fn rewrapped_block_quote_classifies_as_unchanged() {
        let r = records(
            "> The quick brown fox\n> jumped over the lazy dog.",
            "> The quick brown fox jumped over the\n> lazy dog.",
            SkipAll,
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "unchanged");
    }

    #[test]
    fn rewrapped_paragraph_keeps_word_edit_visible() {
        let r = records(
            "The quick brown fox\njumped over the lazy dog.",
            "The quick red fox jumped over\nthe lazy dog.",
            SkipAll,
        );
        assert!(has(&r, "deleted") && has(&r, "inserted"));
    }

    #[test]
    fn removed_hard_line_break_classifies_as_changed() {
        let r = records(
            "The quick brown fox  \njumped over the lazy dog.",
            "The quick brown fox\njumped over the lazy dog.",
            SkipAll,
        );
        assert!(has(&r, "deleted") && has(&r, "inserted"));
    }

    #[test]
    fn deepened_quote_classifies_as_changed() {
        let r = records(
            "> The quick brown fox.\n",
            ">> The quick brown fox.\n",
            SkipAll,
        );
        assert!(has(&r, "deleted") && has(&r, "inserted"));
    }

    #[test]
    fn code_block_keeps_its_whitespace() {
        let r = records(
            "```\nlet x = 1\n```\n",
            "```\n    let x = 1\n```\n",
            SkipAll,
        );
        assert!(has(&r, "deleted") && has(&r, "inserted"));
    }
}

/// `CommentDiffInvarianceTests.swift` and `CommentChangeTrackingTests.swift`:
/// comments are metadata, invisible to change tracking under both
/// policies. (The render-level cases are checked through
/// `compute_changes`, which is what Phase 3's renderers project from; the
/// `removeComments` / `commentDefinitionLineRanges` helpers have no mudl
/// counterpart, and the `stripCommentTokens` cases live in `mudl-diff`.)
mod comments {
    use super::*;

    const PLAIN: &str = "The quick brown fox jumped.\n";
    const COMMENTED: &str = "The quick brown fox[^comment-a] jumped.\n\n[^comment-a]: A note.\n";

    #[test]
    fn adding_a_comment_leaves_all_blocks_unchanged() {
        for policy in BOTH {
            let r = records(PLAIN, COMMENTED, policy);
            assert_eq!(r.len(), 1, "{policy:?}");
            assert!(all_unchanged(PLAIN, COMMENTED, policy));
        }
    }

    #[test]
    fn removing_a_comment_leaves_all_blocks_unchanged() {
        for policy in BOTH {
            assert_eq!(records(COMMENTED, PLAIN, policy).len(), 1);
            assert!(all_unchanged(COMMENTED, PLAIN, policy));
        }
    }

    #[test]
    fn adding_or_removing_a_comment_yields_zero_changes() {
        for policy in BOTH {
            assert!(changes(PLAIN, COMMENTED, policy).is_empty());
            assert!(changes(COMMENTED, PLAIN, policy).is_empty());
        }
    }

    #[test]
    fn multi_paragraph_comment_thread_yields_zero_changes() {
        let new = "The quick brown fox[^comment-a] jumped.\n\n[^comment-a]: > brown fox\n\n    💬 {Tester @ 2026-07-08 12:00:00}:\n\n    A note.\n\n    💬 {Other @ 2026-07-08 12:05:00}:\n\n    A reply.\n";
        for policy in BOTH {
            assert!(changes(PLAIN, new, policy).is_empty());
        }
    }

    #[test]
    fn real_edit_alongside_a_comment_reports_only_the_body_change() {
        let old_plain = "The fox ran.\n\nSecond para.\n";
        let new_plain = "The fox jumped.\n\nSecond para.\n";
        let new_commented =
            "The fox[^comment-a] jumped.\n\nSecond para.\n\n[^comment-a]: A note.\n";
        let plain = records(old_plain, new_plain, DescendPlainFootnotes);
        let commented = records(old_plain, new_commented, DescendPlainFootnotes);
        assert_eq!(commented.len(), plain.len());
        let unchanged = |r: &[(&str, Option<String>, Option<String>)]| {
            r.iter().filter(|(k, _, _)| *k == "unchanged").count()
        };
        assert_eq!(unchanged(&commented), unchanged(&plain));
        assert!(unchanged(&commented) > 0);
        let ids = |list: Vec<DocumentChange>| -> Vec<(String, ChangeType)> {
            list.into_iter().map(|c| (c.id, c.ty)).collect()
        };
        for policy in BOTH {
            assert_eq!(
                ids(changes(old_plain, new_commented, policy)),
                ids(changes(old_plain, new_plain, policy))
            );
        }
    }

    #[test]
    fn leaf_block_count_unaffected_by_a_comment_definition() {
        let plain = "Para one.\n\nPara two.\n";
        let commented = "Para one.[^comment-a]\n\nPara two.\n\n[^comment-a]: A note.\n";
        for policy in BOTH {
            assert_eq!(leaves(plain, policy).len(), leaves(commented, policy).len());
        }
    }

    #[test]
    fn unchanged_comment_is_not_flagged() {
        let md =
            "The quick brown fox[^comment-a] jumped.\n\n[^comment-a]: > brown fox\n\n    A note.\n";
        for policy in BOTH {
            assert!(changes(md, md, policy).is_empty());
        }
    }

    #[test]
    fn unchanged_footnote_is_not_flagged() {
        let md = "A sentence[^1].\n\n[^1]: A footnote.\n";
        for policy in BOTH {
            assert!(changes(md, md, policy).is_empty());
        }
    }

    #[test]
    fn real_edit_still_flagged_alongside_comment() {
        let old = "The fox[^comment-a] ran.\n\n[^comment-a]: A note.\n";
        let new = "The fox[^comment-a] jumped.\n\n[^comment-a]: A note.\n";
        for policy in BOTH {
            assert!(!changes(old, new, policy).is_empty());
        }
    }

    #[test]
    fn comment_definition_edit_is_invisible_but_footnote_edit_follows_policy() {
        let c_old = "Text[^comment-a].\n\n[^comment-a]: Old.\n";
        let c_new = "Text[^comment-a].\n\n[^comment-a]: New.\n";
        for policy in BOTH {
            assert!(changes(c_old, c_new, policy).is_empty());
        }
        let f_old = "Text[^a].\n\n[^a]: Old.\n";
        let f_new = "Text[^a].\n\n[^a]: New.\n";
        assert!(changes(f_old, f_new, SkipAll).is_empty());
        assert!(!changes(f_old, f_new, DescendPlainFootnotes).is_empty());
    }

    #[test]
    fn the_rendered_comment_marker_strips_out_of_a_fingerprint() {
        // `stripCommentTokensRemovesTheMarkerTheEmitterEmits`: pin the
        // strip against the marker `render_up` actually emits.
        for label in ["comment-a", "comment-intro_2-b"] {
            let md = format!("Fox[^{label}] ran.\n\n[^{label}]: note\n");
            let html = crate::render::render_up(&md, &crate::options::RenderOptions::default());
            let start = html.find("<a class=\"mud-comment-marker\"").unwrap();
            let end = start + html[start..].find("</a>").unwrap() + "</a>".len();
            let baked = format!("Fox{} ran.", &html[start..end]);
            assert_eq!(mudl_diff::block::strip_comment_tokens(&baked), "Fox ran.");
        }
    }
}

/// `ChangeIDParityTests.corpus` through the real collector. `mudl-diff`
/// pinned these with a test-only mini collector (`list.rs`
/// `compute_changes_tests`); the real collector must agree.
mod id_parity {
    use super::*;

    type Row = (&'static str, ChangeType, usize, bool, &'static str, usize);
    type OwnedRow = (String, ChangeType, usize, bool, String, usize);

    fn rows(old: &str, new: &str, policy: DefinitionPolicy) -> Vec<OwnedRow> {
        changes(old, new, policy)
            .into_iter()
            .map(|c| {
                (
                    c.id,
                    c.ty,
                    c.source_line,
                    c.is_consecutive,
                    c.group_id,
                    c.group_index,
                )
            })
            .collect()
    }

    fn check(old: &str, new: &str, expected: Vec<Row>) {
        let expected: Vec<OwnedRow> = expected
            .into_iter()
            .map(|(id, ty, line, cons, group, index)| {
                (id.to_string(), ty, line, cons, group.to_string(), index)
            })
            .collect();
        for policy in BOTH {
            assert_eq!(rows(old, new, policy), expected, "{policy:?}");
        }
    }

    #[test]
    fn paragraph_edit() {
        check(
            "Alpha one.\n\nBeta.\n",
            "Alpha two.\n\nBeta.\n",
            vec![
                ("change-1", Deletion, 1, false, "group-1", 1),
                ("change-2", Insertion, 1, true, "group-1", 1),
            ],
        );
        let list = changes("Alpha one.\n\nBeta.\n", "Alpha two.\n\nBeta.\n", SkipAll);
        assert_eq!(list[0].summary, "Alpha one.");
        assert!(list.iter().all(|c| c.is_mixed));
    }

    #[test]
    fn insertion_only() {
        check(
            "Alpha.\n",
            "Alpha.\n\nAdded.\n",
            vec![("change-1", Insertion, 3, false, "group-1", 1)],
        );
    }

    #[test]
    fn deletion_only_trailing() {
        check(
            "Alpha.\n\nGone.\n",
            "Alpha.\n",
            vec![("change-1", Deletion, 1, false, "group-1", 1)],
        );
    }

    #[test]
    fn code_block_edit_alone() {
        check(
            "```\nkeep\nold\n```\n",
            "```\nkeep\nnew\n```\n",
            vec![
                ("change-3", Deletion, 1, false, "group-1", 1),
                ("change-3", Insertion, 1, true, "group-1", 0),
            ],
        );
    }

    #[test]
    fn code_block_edit_then_paragraph_edit() {
        check(
            "```\nkeep\nold\n```\n\nMiddle.\n\nTail one.\n",
            "```\nkeep\nnew\n```\n\nMiddle.\n\nTail two.\n",
            vec![
                ("change-3", Deletion, 1, false, "group-1", 1),
                ("change-3", Insertion, 1, true, "group-1", 0),
                ("change-4", Deletion, 8, false, "group-2", 2),
                ("change-5", Insertion, 8, true, "group-2", 2),
            ],
        );
    }

    #[test]
    fn paragraph_edit_then_code_block_edit() {
        check(
            "Head one.\n\nMiddle.\n\n```\nkeep\nold\n```\n",
            "Head two.\n\nMiddle.\n\n```\nkeep\nnew\n```\n",
            vec![
                ("change-1", Deletion, 1, false, "group-1", 1),
                ("change-2", Insertion, 1, true, "group-1", 1),
                ("change-5", Deletion, 5, false, "group-2", 2),
                ("change-5", Insertion, 5, true, "group-2", 0),
            ],
        );
    }

    #[test]
    fn two_code_block_edits() {
        check(
            "```\na\nold1\n```\n\nMid.\n\n```\nb\nold2\n```\n",
            "```\na\nnew1\n```\n\nMid.\n\n```\nb\nnew2\n```\n",
            vec![
                ("change-3", Deletion, 1, false, "group-1", 1),
                ("change-3", Insertion, 1, true, "group-1", 0),
                ("change-6", Deletion, 8, false, "group-2", 2),
                ("change-6", Insertion, 8, true, "group-2", 0),
            ],
        );
    }

    #[test]
    fn code_block_edit_and_insertion_in_the_same_gap() {
        check(
            "```\nkeep\nold\n```\n\nTail.\n",
            "```\nkeep\nnew\n```\n\nAdded.\n\nTail.\n",
            vec![
                ("change-4", Deletion, 1, false, "group-1", 1),
                ("change-4", Insertion, 1, true, "group-1", 0),
                ("change-3", Insertion, 6, true, "group-2", 2),
            ],
        );
    }

    #[test]
    fn multi_cluster_code_block_edit_then_paragraph_edit() {
        let (old, new) = (
            "```\na\nb\nc\nd\ne\n```\n\nTail one.\n",
            "```\na\nB\nc\nD\ne\n```\n\nTail two.\n",
        );
        check(
            old,
            new,
            vec![
                ("change-2", Deletion, 1, false, "group-1", 1),
                ("change-5", Deletion, 1, false, "group-2", 2),
                ("change-5", Insertion, 1, true, "group-2", 0),
                ("change-6", Deletion, 1, false, "group-3", 3),
                ("change-6", Insertion, 1, true, "group-3", 0),
                ("change-4", Insertion, 9, false, "group-4", 4),
            ],
        );
        assert_eq!(compute_groups(old, new, SkipAll, 0.25).len(), 4);
    }

    #[test]
    fn paragraph_and_code_block_swap_positions_in_one_gap() {
        check(
            "Intro.\n\nAlpha beta gamma.\n\n```\nkeep\nold\n```\n\nTail.\n",
            "Intro.\n\n```\nkeep\nnew\n```\n\nDelta epsilon zeta.\n\nTail.\n",
            vec![
                ("change-1", Deletion, 3, false, "group-1", 1),
                ("change-5", Deletion, 3, false, "group-2", 2),
                ("change-5", Insertion, 3, true, "group-2", 0),
                ("change-4", Insertion, 8, true, "group-3", 3),
            ],
        );
    }

    #[test]
    fn paragraph_deleted_and_code_block_edited_in_one_gap() {
        check(
            "Gone paragraph.\n\n```\nkeep\nold\n```\n\nTail.\n",
            "```\nkeep\nnew\n```\n\nTail.\n",
            vec![
                ("change-1", Deletion, 1, false, "group-1", 1),
                ("change-4", Deletion, 1, false, "group-2", 2),
                ("change-4", Insertion, 1, true, "group-2", 0),
            ],
        );
    }

    #[test]
    fn real_and_mini_collectors_agree_leaf_by_leaf_on_the_corpus() {
        // Spot-check the fields the mini collector fabricates.
        let b = blocks("```\nkeep\nold\n```\n\nTail one.\n");
        assert_eq!(b[0].source_text, "```\nkeep\nold\n```");
        assert_eq!(b[0].code.as_deref(), Some("keep\nold\n"));
        assert_eq!(b[0].language, None);
        assert_eq!(b[1].source_line, 6);
        assert_eq!(b[1].inline_text.as_deref(), Some("Tail one."));
    }
}

mod plan_cache {
    use super::*;

    #[test]
    fn plans_are_shared_through_a_cache() {
        let cache = Mutex::new(PlanCache::new());
        let a = plan_with_cache(&cache, "A.\n", "B.\n", SkipAll, 0.25);
        let b = plan_with_cache(&cache, "A.\n", "B.\n", SkipAll, 0.25);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(cache.lock().unwrap().len(), 1);
        // Policy and threshold are part of the key.
        plan_with_cache(&cache, "A.\n", "B.\n", DescendPlainFootnotes, 0.25);
        plan_with_cache(&cache, "A.\n", "B.\n", SkipAll, 0.5);
        assert_eq!(cache.lock().unwrap().len(), 3);
    }

    #[test]
    fn frontmatter_only_differences_share_a_plan() {
        let cache = Mutex::new(PlanCache::new());
        let a = plan_with_cache(&cache, "---\na: 1\n---\nX.\n", "Y.\n", SkipAll, 0.25);
        let b = plan_with_cache(&cache, "---\na: 2\n---\nX.\n", "Y.\n", SkipAll, 0.25);
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn waypoint_threshold_is_a_fraction() {
        let w = Waypoint {
            old_markdown: Arc::from("x"),
            show_inline_deletions: false,
            word_diff_threshold_pct: 25,
        };
        assert_eq!(w.threshold(), 0.25);
    }

    #[test]
    fn tracker_mode_picks_the_policy() {
        assert_eq!(TrackerMode::Up.policy(), SkipAll);
        assert_eq!(TrackerMode::Down.policy(), DescendPlainFootnotes);
        assert_eq!(TrackerMode::default(), TrackerMode::Up);
    }
}

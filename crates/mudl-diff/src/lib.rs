//! Change-tracking diff engine (`mudl-diff`; `_working/DIFF-IMPLEMENTATION.md`
//! §7, Phase 1).
//!
//! A port of `mud`'s `Core/Sources/Diff/*.swift`, minus anything that needs
//! a Markdown AST or a syntax highlighter. Everything here is a pure function
//! over strings and already-collected [`block::LeafBlock`]s, so the crate has
//! no dependencies at all. Collecting leaf blocks from a `pulldown-cmark`
//! event stream, and rendering the result, belong to `mudl-core::changes`
//! (the dependency direction is `mudl-core → mudl-diff`).
//!
//! The pieces, bottom-up:
//!
//! - [`myers`]: the O((N+M)·D) sequence diff every other module builds on,
//!   with the anchor choice of Swift's `CollectionDifference`.
//! - [`word`]: word-level spans, similarity, significance gating.
//! - [`pairing`]: greedy best-overlap pairing of deleted/inserted lines.
//! - [`line`]: line-level annotations (`LineLevelDiff`).
//! - [`block`]: leaf-block fingerprints and block matching (`BlockMatcher`).
//! - [`code`]: per-line diff inside a paired code block (`CodeBlockDiff`).
//! - [`plan`]: the single pass that mints change and group IDs
//!   (`ChangePlan`), plus a small LRU [`plan::PlanCache`].
//! - [`list`]: the sidebar projection (`ChangeList`, `ChangeGroup`).

pub mod block;
pub mod code;
pub mod line;
pub mod list;
pub mod myers;
pub mod pairing;
pub mod plan;
pub mod word;

const COMMENT_LABEL_PREFIX: &str = "comment-";

/// True when `label` is a comment label (`comment-` followed by one or more
/// `[\w-]` characters) rather than an authorial footnote label.
///
/// Mirrors `FootnoteProcessor.isCommentLabel`, including its Unicode-aware
/// notion of a word character. It lives here, at the bottom of the
/// dependency graph, because comments must be invisible to change tracking:
/// the fingerprints in [`block`] strip comment references, and the leaf
/// collector in `mudl-core` skips comment definitions. `mudl-core` will
/// reuse this function instead of keeping its own copy.
pub fn is_comment_label(label: &str) -> bool {
    match label.strip_prefix(COMMENT_LABEL_PREFIX) {
        Some(suffix) if !suffix.is_empty() => suffix.chars().all(is_label_char),
        _ => false,
    }
}

/// One character of a comment label's suffix: the `[\w-]` class.
pub(crate) fn is_label_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// A deliberately tiny "collector" for tests: blank-line-separated blocks,
/// with ```` ``` ```` fences, `# ` headings, `- ` and `1. ` list items.
/// Enough to port `mud`'s corpus cases without a Markdown parser; the real
/// collector is `mudl-core`'s.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::block::{BlockKind, LeafBlock, LeafTag};

    pub fn doc(md: &str) -> Vec<LeafBlock> {
        // (line text, byte offset of line start)
        let mut lines = Vec::new();
        let mut offset = 0;
        for line in md.split_inclusive('\n') {
            lines.push((line.trim_end_matches('\n'), offset));
            offset += line.len();
        }
        let mut blocks = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let (text, start) = lines[i];
            if text.trim().is_empty() {
                i += 1;
                continue;
            }
            let first = i;
            if let Some(info) = text.strip_prefix("```") {
                i += 1;
                while i < lines.len() && !lines[i].0.starts_with("```") {
                    i += 1;
                }
                let body: String = lines[first + 1..i]
                    .iter()
                    .map(|(l, _)| format!("{l}\n"))
                    .collect();
                let end = if i < lines.len() { i + 1 } else { i };
                let source: Vec<&str> = lines[first..end].iter().map(|(l, _)| *l).collect();
                let source = source.join("\n");
                blocks.push(
                    LeafBlock::new(
                        BlockKind::CodeBlock,
                        LeafTag::CodeBlock,
                        first + 1,
                        start..start + source.len(),
                        source,
                    )
                    .with_language(Some(info.trim()))
                    .with_code(body),
                );
                i = end;
                continue;
            }
            while i < lines.len() && !lines[i].0.trim().is_empty() {
                i += 1;
            }
            let source: Vec<&str> = lines[first..i].iter().map(|(l, _)| *l).collect();
            let source = source.join("\n");
            let range = start..start + source.len();
            let (kind, tag, inline) = if let Some(h) = source.strip_prefix("# ") {
                (BlockKind::Prose, LeafTag::Heading(1), h.to_string())
            } else if let Some(item) = source.strip_prefix("- ") {
                (BlockKind::Prose, LeafTag::ListItem, item.to_string())
            } else if source.starts_with(|c: char| c.is_ascii_digit()) && source.contains(". ") {
                let item = &source[source.find(". ").unwrap() + 2..];
                (
                    BlockKind::OrderedListItem,
                    LeafTag::ListItem,
                    item.to_string(),
                )
            } else {
                (BlockKind::Prose, LeafTag::Paragraph, source.clone())
            };
            let inline: Vec<&str> = inline.split('\n').map(str::trim).collect();
            blocks.push(
                LeafBlock::new(kind, tag, first + 1, range, source)
                    .with_inline_text(inline.join(" ")),
            );
        }
        blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod is_comment_label_tests {
        use super::*;

        #[test]
        fn valid_label_is_recognized() {
            assert!(is_comment_label("comment-abc123"));
        }

        #[test]
        fn empty_suffix_is_rejected() {
            assert!(!is_comment_label("comment-"));
        }

        #[test]
        fn suffix_with_space_or_period_is_rejected() {
            assert!(!is_comment_label("comment-a b"));
            assert!(!is_comment_label("comment-a.b"));
        }

        #[test]
        fn missing_prefix_is_rejected() {
            assert!(!is_comment_label("footnote-abc123"));
            assert!(!is_comment_label("xcomment-abc"));
            assert!(!is_comment_label(""));
        }

        #[test]
        fn hyphen_underscore_and_unicode_letters_are_accepted() {
            assert!(is_comment_label("comment---"));
            assert!(is_comment_label("comment-a_b"));
            assert!(is_comment_label("comment-café"));
        }
    }
}

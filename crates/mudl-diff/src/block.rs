//! Leaf blocks, fingerprints, and block matching (`BlockMatcher.swift`;
//! plan §7.5).
//!
//! Change tracking works on *leaf blocks*: paragraphs, headings, simple list
//! items, table rows, code blocks, thematic breaks and HTML blocks. Two
//! blocks are "the same" when their fingerprints match; a fingerprint is the
//! block's source with comment tokens stripped and, for prose, cosmetic
//! whitespace collapsed, so neither adding a comment nor re-wrapping a
//! paragraph counts as a change.
//!
//! Unlike the Swift original, nothing here walks a Markdown tree: leaf
//! *collection* from `pulldown-cmark` events lives in `mudl-core::changes`
//! (Phase 2), which builds [`LeafBlock`]s with [`LeafBlock::new`] and the
//! `with_*` setters. Everything below operates on those.

use std::ops::Range;

use crate::{is_label_char, myers};

/// How a block's source is normalised before comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockKind {
    /// Paragraphs, headings, unordered and task list items: re-wrapping
    /// and continuation-line quote prefixes are cosmetic.
    Prose,
    /// An ordered list item: like `Prose`, but the `N.`/`N)` marker is
    /// ignored too, so renumbering a list is not a change.
    OrderedListItem,
    /// A table row (or header): pipe-padding whitespace is cosmetic.
    TableRow,
    /// A fenced or indented code block, compared verbatim. Separate from
    /// `Verbatim` because code blocks pair line-by-line ([`crate::code`]).
    CodeBlock,
    /// HTML blocks, thematic breaks, display math: compared verbatim.
    Verbatim,
}

/// The element a leaf renders as, so later phases can put change attributes
/// on the right tag without re-deriving it from the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeafTag {
    /// `<p>` (including a paragraph inside a blockquote or alert, and a
    /// `$$…$$` display-math paragraph).
    Paragraph,
    /// `<hN>`, with N in 1..=6.
    Heading(u8),
    /// `<li>`.
    ListItem,
    /// `<tr>` in a table body.
    TableRow,
    /// `<thead>` (pulldown-cmark has no row inside the head).
    TableHead,
    /// `<pre>`.
    CodeBlock,
    /// `<hr />`.
    ThematicBreak,
    /// A raw HTML block.
    HtmlBlock,
}

/// How the leaf collector treats footnote definitions
/// (`DefinitionDiffPolicy`). Defined here so the policy can join the
/// [`crate::plan::PlanCache`] key; the collector itself is in `mudl-core`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefinitionPolicy {
    /// Every definition, footnote or comment, is skipped. Up mode: the Up
    /// renderer draws nothing at a definition, so a change there could
    /// only be misplaced.
    SkipAll,
    /// Comment definitions are skipped; plain footnote definitions are
    /// descended and their body blocks become leaves. Down mode, dedup, and
    /// counts: Down shows the raw source, definitions included.
    DescendPlainFootnotes,
}

/// A leaf-level block, already collected from a document.
///
/// Blocks are joined across consumers by `byte_range`, never by identity
/// (`mud`'s `SourceKey`): a cached plan may hold blocks from an earlier,
/// textually identical parse.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LeafBlock {
    pub kind: BlockKind,
    pub tag: LeafTag,
    /// 1-based line of the block's start in the (frontmatter-stripped) body.
    pub source_line: usize,
    /// The block's byte range in the body it was collected from.
    pub byte_range: Range<usize>,
    /// The block's source: whole lines, trailing blank lines trimmed.
    pub source_text: String,
    /// What the block is matched on; see [`fingerprint`].
    pub fingerprint: String,
    /// A code block's fence info string (`None` when empty or indented).
    /// Compared case-insensitively against `mermaid`/`math`.
    pub language: Option<String>,
    /// The block's inline text as the renderer's character stream sees it
    /// (text and code literals; soft break → `" "`; hard break → `"\n"`).
    /// Paragraphs, headings and list items (their first paragraph) carry
    /// it; so do table rows (cell text concatenated). Word diffs run on it,
    /// and `None` diffs as empty, matching `inlineText(of:)` on a node with
    /// no inline children.
    pub inline_text: Option<String>,
    /// A code block's content without fences (cmark's `literal`).
    pub code: Option<String>,
}

impl LeafBlock {
    /// Builds a leaf and computes its fingerprint from `source_text`.
    pub fn new(
        kind: BlockKind,
        tag: LeafTag,
        source_line: usize,
        byte_range: Range<usize>,
        source_text: impl Into<String>,
    ) -> Self {
        let source_text = source_text.into();
        LeafBlock {
            fingerprint: fingerprint(&source_text, kind),
            kind,
            tag,
            source_line,
            byte_range,
            source_text,
            language: None,
            inline_text: None,
            code: None,
        }
    }

    /// Sets [`LeafBlock::language`]; an empty info string becomes `None`.
    pub fn with_language(mut self, language: Option<impl Into<String>>) -> Self {
        self.language = language.map(Into::into).filter(|l: &String| !l.is_empty());
        self
    }

    /// Sets [`LeafBlock::inline_text`].
    pub fn with_inline_text(mut self, text: impl Into<String>) -> Self {
        self.inline_text = Some(text.into());
        self
    }

    /// Sets [`LeafBlock::code`].
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// The text word diffs run on: the inline text, or `""`.
    pub fn word_text(&self) -> &str {
        self.inline_text.as_deref().unwrap_or("")
    }

    /// True when the block's language is `name`, ignoring case.
    pub fn has_language(&self, name: &str) -> bool {
        self.language
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case(name))
    }
}

/// Relationship between the old and new leaf lists, as indices into them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockMatch {
    /// `Unchanged(o, n)`: `old[o]` and `new[n]` have the same fingerprint
    /// (an anchor).
    Unchanged(usize, usize),
    /// `old[i]` has no counterpart.
    Deleted(usize),
    /// `new[i]` has no counterpart.
    Inserted(usize),
}

/// Matches two leaf lists by fingerprint. The result is in document order;
/// within each gap between anchors, deletions come before insertions.
pub fn match_blocks(old: &[LeafBlock], new: &[LeafBlock]) -> Vec<BlockMatch> {
    let old_fp: Vec<&str> = old.iter().map(|b| b.fingerprint.as_str()).collect();
    let new_fp: Vec<&str> = new.iter().map(|b| b.fingerprint.as_str()).collect();
    myers::diff(&old_fp, &new_fp)
        .into_iter()
        .map(|e| match e {
            myers::Edit::Unchanged { old, new } => BlockMatch::Unchanged(old, new),
            myers::Edit::Deleted(i) => BlockMatch::Deleted(i),
            myers::Edit::Inserted(i) => BlockMatch::Inserted(i),
        })
        .collect()
}

/// Builds the fingerprint of a block's `raw` source for `kind`.
///
/// - Comment tokens are stripped first (prose, code, verbatim) — gaining a
///   comment is not a change.
/// - `Prose` collapses cosmetic whitespace ([`normalized_prose`]).
/// - `OrderedListItem` drops the list marker, then normalises as prose.
/// - `TableRow` collapses every whitespace run to one space, then strips
///   comment tokens (Swift's order).
/// - `CodeBlock` and `Verbatim` are otherwise compared verbatim.
pub fn fingerprint(raw: &str, kind: BlockKind) -> String {
    match kind {
        BlockKind::Prose => normalized_prose(&strip_comment_tokens(raw)),
        BlockKind::OrderedListItem => {
            normalized_prose(&strip_comment_tokens(strip_ordered_marker(raw)))
        }
        BlockKind::TableRow => strip_comment_tokens(&collapse_runs(raw, char::is_whitespace)),
        BlockKind::CodeBlock | BlockKind::Verbatim => strip_comment_tokens(raw),
    }
}

/// The CSS class of a baked comment marker (`mudl-core`'s renderer emits
/// `<a class="mud-comment-marker" …>💬</a>`).
const COMMENT_MARKER_OPEN: &str = "<a class=\"mud-comment-marker\"";
const COMMENT_MARKER_CLOSE: &str = ">\u{1F4AC}</a>";
const COMMENT_REF_OPEN: &str = "[^comment-";

/// Removes comment tokens — raw `[^comment-…]` references and baked marker
/// HTML — so a block that only gained a comment fingerprints like its
/// pre-comment self. A no-op (beyond the copy) on comment-free text.
pub fn strip_comment_tokens(s: &str) -> String {
    if !s.contains("[^") && !s.contains("mud-comment-marker") {
        return s.to_string();
    }
    strip_marker_html(&strip_comment_refs(s))
}

/// Removes `\[\^comment-[\w-]+\]` matches, scanning left to right.
fn strip_comment_refs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find(COMMENT_REF_OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + COMMENT_REF_OPEN.len()..];
        let label_len = after.len() - after.trim_start_matches(is_label_char).len();
        if label_len > 0 && after[label_len..].starts_with(']') {
            rest = &after[label_len + 1..];
        } else {
            // Not a comment reference: keep the `[` and rescan after it.
            out.push('[');
            rest = &rest[start + 1..];
        }
    }
    out.push_str(rest);
    out
}

/// Removes `<a class="mud-comment-marker"[^>]*>💬</a>` matches.
fn strip_marker_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find(COMMENT_MARKER_OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + COMMENT_MARKER_OPEN.len()..];
        match after.find('>') {
            Some(gt) if after[gt..].starts_with(COMMENT_MARKER_CLOSE) => {
                rest = &after[gt + COMMENT_MARKER_CLOSE.len()..];
            }
            _ => {
                out.push('<');
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Strips a leading ordered-list marker: `^[ \t>]*\d{1,9}[.)][ \t]*`, plus
/// a line break (and the next line's prefix) when the content starts on the
/// following line. What remains is what `mud` fingerprints — the item's
/// paragraph, cut at its start column. Text without a marker is returned
/// unchanged.
pub fn strip_ordered_marker(text: &str) -> &str {
    let rest = text.trim_start_matches([' ', '\t', '>']);
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 || digits > 9 {
        return text;
    }
    let rest = &rest[digits..];
    let Some(rest) = rest.strip_prefix(['.', ')']) else {
        return text;
    };
    let rest = rest.trim_start_matches([' ', '\t']);
    match rest.strip_prefix('\n') {
        Some(next) => next.trim_start_matches([' ', '\t', '>']),
        None => rest,
    }
}

/// Stands in for a hard line break while whitespace collapses. A control
/// character, so no readable source carries one.
const HARD_BREAK_MARK: char = '\u{1}';

/// The whitespace Markdown treats as cosmetic: ASCII only. A non-breaking
/// space renders as itself, so it is content and stays.
fn is_cosmetic_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r')
}

/// Collapses cosmetic whitespace out of a prose block so re-wrapping is not
/// a change: continuation lines lose their `[ \t>]*` prefix, every
/// whitespace run becomes one space, and one space is trimmed from each
/// end. Two things survive because they change what renders: the first
/// line's indent (nesting depth), and a hard break (2+ trailing spaces),
/// marked with [`HARD_BREAK_MARK`] before the collapse. A backslash hard
/// break is not whitespace and survives as itself.
fn normalized_prose(raw: &str) -> String {
    let mut lines = raw.split('\n');
    let first = lines.next().unwrap_or("");
    let indent = &first[..first.len() - first.trim_start_matches([' ', '\t']).len()];

    let mut joined = String::with_capacity(raw.len());
    joined.push_str(first);
    for line in lines {
        joined.push('\n');
        joined.push_str(line.trim_start_matches([' ', '\t', '>']));
    }
    let marked = mark_hard_breaks(&joined);
    let flat = collapse_runs(&marked, is_cosmetic_whitespace);
    let trimmed = flat.strip_prefix(' ').unwrap_or(&flat);
    let trimmed = trimmed.strip_suffix(' ').unwrap_or(trimmed);
    format!("{indent}{trimmed}")
}

/// Replaces every ` {2,}\n` with the hard-break mark plus `\n`.
fn mark_hard_breaks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut segments = s.split('\n').peekable();
    while let Some(seg) = segments.next() {
        if segments.peek().is_none() {
            out.push_str(seg);
            break;
        }
        let body = seg.trim_end_matches(' ');
        if seg.len() - body.len() >= 2 {
            out.push_str(body);
            out.push(HARD_BREAK_MARK);
        } else {
            out.push_str(seg);
        }
        out.push('\n');
    }
    out
}

/// Replaces each run of characters matching `is_ws` with a single space.
fn collapse_runs(s: &str, is_ws: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_run = false;
    for c in s.chars() {
        if is_ws(c) {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(kind: BlockKind, text: &str) -> LeafBlock {
        let tag = match kind {
            BlockKind::Prose => LeafTag::Paragraph,
            BlockKind::OrderedListItem => LeafTag::ListItem,
            BlockKind::TableRow => LeafTag::TableRow,
            BlockKind::CodeBlock => LeafTag::CodeBlock,
            BlockKind::Verbatim => LeafTag::HtmlBlock,
        };
        LeafBlock::new(kind, tag, 1, 0..text.len(), text)
    }

    fn fp(kind: BlockKind, text: &str) -> String {
        fingerprint(text, kind)
    }

    fn same(kind: BlockKind, a: &str, b: &str) -> bool {
        fp(kind, a) == fp(kind, b)
    }

    use BlockKind::{CodeBlock, OrderedListItem, Prose, TableRow, Verbatim};

    /// The fingerprint halves of `BlockMatcherTests.swift` (the definition
    /// cases need a parser and move to `mudl-core`'s collector tests).
    mod prose_fingerprint_tests {
        use super::*;

        #[test]
        fn rewrapped_paragraph_is_unchanged() {
            assert!(same(
                Prose,
                "A message attribution at the start of a paragraph **begins a new\nmessage**. The first message needs no such attribution.",
                "A message attribution at the start of a paragraph **begins a\nnew message**. The first message needs no such attribution."
            ));
        }

        #[test]
        fn rewrapped_list_item_is_unchanged() {
            assert!(same(
                Prose,
                "- one workaround is to put empty braces at the\n  start.",
                "- one workaround is to put empty braces\n  at the start."
            ));
        }

        #[test]
        fn rewrapped_blockquote_is_unchanged() {
            assert!(same(
                Prose,
                "> The quick brown fox\n> jumped over the lazy dog.",
                "> The quick brown fox jumped over the\n> lazy dog."
            ));
        }

        #[test]
        fn lazy_continuation_is_unchanged() {
            assert!(same(
                Prose,
                "> first line\n> second line",
                "> first line\nsecond line"
            ));
        }

        #[test]
        fn rewrap_keeps_a_word_edit_visible() {
            assert!(!same(
                Prose,
                "The quick brown fox\njumped over the lazy dog.",
                "The quick red fox jumped over\nthe lazy dog."
            ));
        }

        #[test]
        fn removed_hard_line_break_is_a_change() {
            assert!(!same(
                Prose,
                "The quick brown fox  \njumped over the lazy dog.",
                "The quick brown fox\njumped over the lazy dog."
            ));
        }

        #[test]
        fn hard_break_width_is_cosmetic() {
            assert!(same(Prose, "a  \nb", "a     \nb"));
        }

        #[test]
        fn backslash_hard_break_survives() {
            assert!(!same(Prose, "a\\\nb", "a\nb"));
        }

        #[test]
        fn deepened_quote_is_a_change() {
            assert!(!same(
                Prose,
                "> The quick brown fox.",
                ">> The quick brown fox."
            ));
        }

        #[test]
        fn first_line_indent_is_kept() {
            assert!(!same(Prose, "text here", "  text here"));
            assert_eq!(fp(Prose, "  a\n   b"), "  a b");
        }

        #[test]
        fn non_breaking_space_is_content() {
            assert!(!same(Prose, "a\u{a0}b", "a b"));
        }

        #[test]
        fn exact_normal_form() {
            assert_eq!(fp(Prose, "a  \n> b\tc  "), "a\u{1} b c");
        }
    }

    mod comment_token_tests {
        use super::*;

        #[test]
        fn added_comment_reference_is_unchanged() {
            assert!(same(Prose, "The quick fox.", "The quick fox[^comment-a]."));
        }

        #[test]
        fn plain_footnote_reference_is_a_change() {
            assert!(!same(Prose, "hello world", "hello[^1] world"));
        }

        #[test]
        fn baked_marker_html_is_stripped() {
            let marked = "hi<a class=\"mud-comment-marker\" id=\"cmtref-comment-a\" href=\"#cmt-comment-a\">\u{1F4AC}</a> there";
            assert_eq!(strip_comment_tokens(marked), "hi there");
        }

        #[test]
        fn nested_bracket_still_finds_the_comment() {
            assert_eq!(strip_comment_tokens("[^a [^comment-x]"), "[^a ");
        }

        #[test]
        fn malformed_references_are_kept() {
            for s in ["[^comment-]", "[^comment-a b]", "[^comment-x", "[^1]"] {
                assert_eq!(strip_comment_tokens(s), s);
            }
        }

        #[test]
        fn code_blocks_strip_comments_too() {
            assert!(same(CodeBlock, "x[^comment-a]", "x"));
        }
    }

    mod other_kind_tests {
        use super::*;

        #[test]
        fn ordered_renumbering_is_unchanged() {
            assert!(same(OrderedListItem, "5. Foo", "4. Foo"));
            assert!(same(OrderedListItem, "  9) Foo\n   bar", "10) Foo bar"));
            assert!(same(OrderedListItem, "> 1. Foo", "2. Foo"));
        }

        #[test]
        fn ordered_content_change_is_a_change() {
            assert!(!same(OrderedListItem, "5. Foo", "5. Bar"));
        }

        #[test]
        fn ordered_marker_variants() {
            assert_eq!(strip_ordered_marker("12. x"), "x");
            assert_eq!(strip_ordered_marker("1.\n   x"), "x");
            assert_eq!(strip_ordered_marker("1234567890. x"), "1234567890. x");
            assert_eq!(strip_ordered_marker("- x"), "- x");
        }

        #[test]
        fn table_padding_is_unchanged() {
            assert!(same(TableRow, "| a   | b |", "| a | b |"));
            assert!(!same(TableRow, "| a | b |", "| a | c |"));
        }

        #[test]
        fn code_block_keeps_its_whitespace() {
            assert!(!same(
                CodeBlock,
                "```\nlet x = 1\n```",
                "```\n    let x = 1\n```"
            ));
            assert!(!same(Verbatim, "fn main() {}", "fn main() {  }"));
        }
    }

    mod leaf_block_tests {
        use super::*;

        #[test]
        fn new_computes_the_fingerprint() {
            assert_eq!(leaf(Prose, "a\nb").fingerprint, "a b");
        }

        #[test]
        fn empty_language_is_none() {
            let b = leaf(CodeBlock, "```\nx\n```").with_language(Some(""));
            assert_eq!(b.language, None);
            let b = b.with_language(Some("Mermaid"));
            assert!(b.has_language("mermaid"));
        }

        #[test]
        fn word_text_defaults_to_empty() {
            assert_eq!(leaf(Verbatim, "<div>").word_text(), "");
            assert_eq!(leaf(Prose, "*a*").with_inline_text("a").word_text(), "a");
        }
    }

    mod match_blocks_tests {
        use super::*;
        use BlockMatch::{Deleted as D, Inserted as I, Unchanged as U};

        fn paras(texts: &[&str]) -> Vec<LeafBlock> {
            texts.iter().map(|t| leaf(Prose, t)).collect()
        }

        #[test]
        fn empty_documents() {
            assert!(match_blocks(&[], &[]).is_empty());
        }

        #[test]
        fn identical_blocks_are_unchanged() {
            let docs = paras(&["alpha", "beta"]);
            assert_eq!(match_blocks(&docs, &docs), vec![U(0, 0), U(1, 1)]);
        }

        #[test]
        fn rewrap_is_unchanged() {
            let old = paras(&["the quick\nbrown fox"]);
            let new = paras(&["the quick brown\nfox"]);
            assert_eq!(match_blocks(&old, &new), vec![U(0, 0)]);
        }

        #[test]
        fn insertion_and_deletion() {
            let a = paras(&["alpha"]);
            let ab = paras(&["alpha", "beta"]);
            assert_eq!(match_blocks(&a, &ab), vec![U(0, 0), I(1)]);
            assert_eq!(match_blocks(&ab, &a), vec![U(0, 0), D(1)]);
        }

        #[test]
        fn deletions_come_before_insertions_in_a_gap() {
            let old = paras(&["alpha", "old middle", "gamma"]);
            let new = paras(&["alpha", "new middle", "gamma"]);
            assert_eq!(match_blocks(&old, &new), vec![U(0, 0), D(1), I(1), U(2, 2)]);
        }
    }
}

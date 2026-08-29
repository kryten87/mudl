//! Leaf-block matching (ported from `mud`'s
//! `Core/Sources/Diff/BlockMatcher.swift`, Phase 13.5).
//!
//! Unlike the Swift original — which walks a cmark AST to *collect* leaf
//! blocks from a document — `fingerprint`/`match_blocks` here operate on
//! already-collected [`LeafBlock`]s, matching this plan's step 13.5 (the
//! `pulldown-cmark` event-stream walk that produces `LeafBlock`s from a
//! real document is Phase 13.8's concern, where it plugs into
//! `mudl-core`'s renderer).

use mudl_core::footnotes::is_comment_label;

/// The block-kind-specific normalization `fingerprint` applies before
/// comparing two blocks. Mirrors the Swift source's `isProse`/table-row/
/// ordered-list special cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockKind {
    /// Paragraphs, headings, plain list items: cosmetic whitespace
    /// (re-wrapping, continuation-line quote prefixes) is not a change.
    Prose,
    /// An ordered list item: like `Prose`, but also ignores the leading
    /// `N.`/`N)` marker, so renumbering is not a change.
    OrderedListItem,
    /// A table row: pipe-padding whitespace is not a change.
    TableRow,
    /// A fenced/indented code block: whitespace is real content, compared
    /// verbatim. Carries its own variant (rather than folding into
    /// `Verbatim`) so `plan::build` (13.6) can find code blocks to pair by
    /// position and read `LeafBlock::language` for the Mermaid/math
    /// pairing exclusion.
    CodeBlock,
    /// HTML blocks, thematic breaks: whitespace is real content, compared
    /// verbatim.
    Verbatim,
}

/// A leaf-level block, already collected from a document.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LeafBlock {
    pub kind: BlockKind,
    /// 1-based line number of the block's start in the source.
    pub source_line: usize,
    /// The block's raw source text.
    pub source_text: String,
    /// The fence info string for a `CodeBlock` (e.g. `"rust"`), or `None`
    /// for a bare fence/indented block or any other kind.
    pub language: Option<String>,
}

/// Describes the relationship between a block in the old and new documents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BlockMatch {
    Unchanged { old: LeafBlock, new: LeafBlock },
    Inserted { new: LeafBlock },
    Deleted { old: LeafBlock },
}

/// Builds a block's fingerprint from its source text: comment-footnote
/// references stripped, then, for prose kinds, cosmetic whitespace
/// collapsed. Two blocks with the same fingerprint are the same block as
/// far as change tracking is concerned.
pub fn fingerprint(block: &LeafBlock) -> String {
    let stripped = strip_comment_refs(&block.source_text);
    match block.kind {
        BlockKind::Prose => normalized_prose(&stripped),
        BlockKind::OrderedListItem => normalized_prose(strip_ordered_marker(&stripped)),
        BlockKind::TableRow => collapse_whitespace_runs(&stripped),
        BlockKind::CodeBlock | BlockKind::Verbatim => stripped,
    }
}

/// Compares the leaf blocks of two documents and returns an ordered list
/// of matches describing how blocks changed.
pub fn match_blocks(old: &[LeafBlock], new: &[LeafBlock]) -> Vec<BlockMatch> {
    if old.is_empty() && new.is_empty() {
        return Vec::new();
    }

    let old_fp: Vec<String> = old.iter().map(fingerprint).collect();
    let new_fp: Vec<String> = new.iter().map(fingerprint).collect();
    let (removed_old, inserted_new) = crate::lcs::diff_indices(&old_fp, &new_fp);

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
    let mut boundaries: Vec<(isize, isize)> = vec![(-1, -1)];
    boundaries.extend(anchors.iter().map(|&(o, n)| (o as isize, n as isize)));
    boundaries.push((old.len() as isize, new.len() as isize));

    for i in 0..(boundaries.len() - 1) {
        let (prev_old, prev_new) = boundaries[i];
        let (next_old, next_new) = boundaries[i + 1];

        result.extend(
            ((prev_old + 1) as usize..next_old as usize)
                .filter(|oi| removed_old.contains(oi))
                .map(|oi| BlockMatch::Deleted {
                    old: old[oi].clone(),
                }),
        );
        result.extend(
            ((prev_new + 1) as usize..next_new as usize)
                .filter(|ni| inserted_new.contains(ni))
                .map(|ni| BlockMatch::Inserted {
                    new: new[ni].clone(),
                }),
        );

        if i + 1 < boundaries.len() - 1 {
            result.push(BlockMatch::Unchanged {
                old: old[next_old as usize].clone(),
                new: new[next_new as usize].clone(),
            });
        }
    }

    result
}

/// Strips `[^comment-...]` footnote references — comments are invisible to
/// change tracking everywhere, so gaining/losing one is not a change.
fn strip_comment_refs(text: &str) -> String {
    if !text.contains("[^") {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[^") {
        result.push_str(&rest[..start]);
        let after_marker = &rest[start + 2..];
        match after_marker.find(']') {
            Some(end) => {
                let label = &after_marker[..end];
                if !is_comment_label(label) {
                    result.push_str(&rest[start..start + 2 + end + 1]);
                }
                rest = &after_marker[end + 1..];
            }
            None => {
                result.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    result.push_str(rest);
    result
}

/// Strips a leading ordered-list marker (`N.` or `N)` plus following
/// whitespace) so renumbering a list does not change its fingerprint.
fn strip_ordered_marker(text: &str) -> &str {
    let digits_end = text
        .char_indices()
        .find(|&(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    if digits_end == 0 {
        return text;
    }
    let mut chars = text[digits_end..].char_indices();
    let Some((_, marker_char)) = chars.next() else {
        return text;
    };
    if marker_char != '.' && marker_char != ')' {
        return text;
    }
    let after_marker = digits_end + marker_char.len_utf8();
    let ws_end = text[after_marker..]
        .char_indices()
        .find(|&(_, c)| c != ' ' && c != '\t')
        .map(|(i, _)| after_marker + i)
        .unwrap_or(text.len());
    &text[ws_end..]
}

/// Stands in for a hard line break (2+ trailing spaces before a newline)
/// while the whitespace around it collapses. A control character, so no
/// readable source carries one.
const HARD_BREAK_MARK: char = '\u{1}';

/// Collapses cosmetic whitespace out of a prose block's fingerprint, so
/// re-wrapping a paragraph is not a change: a soft line break renders as a
/// space, and a wrapping tool moves them on almost every edit. Runs of
/// whitespace become one space, and continuation lines lose the
/// blockquote/indent prefix cmark strips before parsing the block's text.
///
/// Two things survive on purpose, because each of them changes what
/// renders: the first line's own indent, and a hard line break (2+
/// trailing spaces, which render as `<br>` — marked so the collapse below
/// cannot erase it; the other hard-break form, a trailing backslash, is
/// not whitespace and survives the collapse as itself).
fn normalized_prose(raw: &str) -> String {
    let mut lines: Vec<&str> = raw.split('\n').collect();
    let Some(&first) = lines.first() else {
        return raw.to_string();
    };
    let indent_end = first
        .char_indices()
        .find(|&(_, c)| c != ' ' && c != '\t')
        .map(|(i, _)| i)
        .unwrap_or(first.len());
    let indent = &first[..indent_end];

    for line in lines.iter_mut().skip(1) {
        let strip_end = line
            .char_indices()
            .find(|&(_, c)| c != ' ' && c != '\t' && c != '>')
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        *line = &line[strip_end..];
    }

    let joined = mark_hard_breaks(&lines);
    let flat = collapse_whitespace_runs(&joined);
    let no_prefix = flat.strip_prefix(' ').unwrap_or(&flat);
    let trimmed = no_prefix.strip_suffix(' ').unwrap_or(no_prefix);

    format!("{indent}{trimmed}")
}

/// Joins `lines` with `\n`, replacing each line's trailing run of 2+
/// spaces (which would otherwise be swallowed by whitespace collapse) with
/// [`HARD_BREAK_MARK`].
fn mark_hard_breaks(lines: &[&str]) -> String {
    let last = lines.len().saturating_sub(1);
    let mut result = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        if i < last {
            let trailing_spaces = line.len() - line.trim_end_matches(' ').len();
            if trailing_spaces >= 2 {
                result.push_str(&line[..line.len() - trailing_spaces]);
                result.push(HARD_BREAK_MARK);
                continue;
            }
        }
        result.push_str(line);
    }
    result
}

/// The whitespace Markdown itself treats as cosmetic — ASCII only.
/// Deliberately narrower than Unicode whitespace, which also covers a
/// non-breaking space; that renders as itself, so it is content and stays.
fn is_cosmetic_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r')
}

fn collapse_whitespace_runs(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_run = false;
    for c in s.chars() {
        if is_cosmetic_whitespace(c) {
            if !in_run {
                result.push(' ');
                in_run = true;
            }
        } else {
            result.push(c);
            in_run = false;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prose(line: usize, text: &str) -> LeafBlock {
        LeafBlock {
            kind: BlockKind::Prose,
            source_line: line,
            source_text: text.to_string(),
            language: None,
        }
    }

    // --- fingerprint: prose re-wrapping ---

    #[test]
    fn rewrapped_paragraph_is_not_a_change() {
        let old = prose(1, "the quick\nbrown fox");
        let new = prose(1, "the quick brown\nfox");
        assert_eq!(fingerprint(&old), fingerprint(&new));
    }

    #[test]
    fn actually_different_paragraph_is_a_change() {
        let old = prose(1, "the quick brown fox");
        let new = prose(1, "the slow brown fox");
        assert_ne!(fingerprint(&old), fingerprint(&new));
    }

    #[test]
    fn first_line_indent_is_preserved() {
        let shallow = prose(1, "text here");
        let deep = prose(1, "  text here");
        assert_ne!(fingerprint(&shallow), fingerprint(&deep));
    }

    #[test]
    fn hard_break_is_preserved() {
        let with_break = prose(1, "line one  \nline two");
        let without_break = prose(1, "line one\nline two");
        assert_ne!(fingerprint(&with_break), fingerprint(&without_break));
    }

    // --- fingerprint: blockquote continuation prefix ---

    #[test]
    fn blockquote_continuation_prefix_differences_are_not_a_change() {
        let with_marker = prose(1, "> first line\n> second line");
        let lazy_continuation = prose(1, "> first line\nsecond line");
        assert_eq!(fingerprint(&with_marker), fingerprint(&lazy_continuation));
    }

    // --- fingerprint: table pipe padding ---

    #[test]
    fn table_pipe_padding_differences_are_not_a_change() {
        let padded = LeafBlock {
            kind: BlockKind::TableRow,
            source_line: 1,
            source_text: "| a   | b |".to_string(),
            language: None,
        };
        let tight = LeafBlock {
            kind: BlockKind::TableRow,
            source_line: 1,
            source_text: "| a | b |".to_string(),
            language: None,
        };
        assert_eq!(fingerprint(&padded), fingerprint(&tight));
    }

    // --- fingerprint: ordered list renumbering ---

    #[test]
    fn ordered_list_renumbering_is_not_a_change() {
        let five = LeafBlock {
            kind: BlockKind::OrderedListItem,
            source_line: 1,
            source_text: "5. Foo".to_string(),
            language: None,
        };
        let four = LeafBlock {
            kind: BlockKind::OrderedListItem,
            source_line: 1,
            source_text: "4. Foo".to_string(),
            language: None,
        };
        assert_eq!(fingerprint(&five), fingerprint(&four));
    }

    #[test]
    fn ordered_list_item_content_change_is_a_change() {
        let a = LeafBlock {
            kind: BlockKind::OrderedListItem,
            source_line: 1,
            source_text: "5. Foo".to_string(),
            language: None,
        };
        let b = LeafBlock {
            kind: BlockKind::OrderedListItem,
            source_line: 1,
            source_text: "5. Bar".to_string(),
            language: None,
        };
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    // --- fingerprint: verbatim (code/html) ---

    #[test]
    fn verbatim_whitespace_change_is_a_change() {
        let a = LeafBlock {
            kind: BlockKind::Verbatim,
            source_line: 1,
            source_text: "fn main() {}".to_string(),
            language: None,
        };
        let b = LeafBlock {
            kind: BlockKind::Verbatim,
            source_line: 1,
            source_text: "fn main() {  }".to_string(),
            language: None,
        };
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    // --- fingerprint: comment references ---

    #[test]
    fn comment_footnote_reference_is_stripped() {
        let without = prose(1, "hello world");
        let with_comment = prose(1, "hello[^comment-abc123] world");
        assert_eq!(fingerprint(&without), fingerprint(&with_comment));
    }

    #[test]
    fn non_comment_footnote_reference_is_kept() {
        let without = prose(1, "hello world");
        let with_footnote = prose(1, "hello[^1] world");
        assert_ne!(fingerprint(&without), fingerprint(&with_footnote));
    }

    // --- match_blocks ---

    #[test]
    fn empty_documents_produce_no_matches() {
        assert!(match_blocks(&[], &[]).is_empty());
    }

    #[test]
    fn identical_blocks_are_all_unchanged() {
        let old = vec![prose(1, "alpha"), prose(2, "beta")];
        let new = old.clone();
        let matches = match_blocks(&old, &new);
        assert_eq!(matches.len(), 2);
        assert!(matches
            .iter()
            .all(|m| matches!(m, BlockMatch::Unchanged { .. })));
    }

    #[test]
    fn rewrap_does_not_produce_a_match_change() {
        let old = vec![prose(1, "the quick\nbrown fox")];
        let new = vec![prose(1, "the quick brown\nfox")];
        let matches = match_blocks(&old, &new);
        assert_eq!(matches.len(), 1);
        assert!(matches!(matches[0], BlockMatch::Unchanged { .. }));
    }

    #[test]
    fn inserted_block_is_reported() {
        let old = vec![prose(1, "alpha")];
        let new = vec![prose(1, "alpha"), prose(2, "beta")];
        let matches = match_blocks(&old, &new);
        assert_eq!(matches.len(), 2);
        assert!(matches!(matches[0], BlockMatch::Unchanged { .. }));
        assert!(matches!(matches[1], BlockMatch::Inserted { .. }));
    }

    #[test]
    fn deleted_block_is_reported() {
        let old = vec![prose(1, "alpha"), prose(2, "beta")];
        let new = vec![prose(1, "alpha")];
        let matches = match_blocks(&old, &new);
        assert_eq!(matches.len(), 2);
        assert!(matches!(matches[0], BlockMatch::Unchanged { .. }));
        assert!(matches!(matches[1], BlockMatch::Deleted { .. }));
    }

    #[test]
    fn deletions_come_before_insertions_in_a_gap() {
        let old = vec![prose(1, "alpha"), prose(2, "old middle"), prose(3, "gamma")];
        let new = vec![prose(1, "alpha"), prose(2, "new middle"), prose(3, "gamma")];
        let matches = match_blocks(&old, &new);
        assert_eq!(matches.len(), 4);
        assert!(matches!(matches[0], BlockMatch::Unchanged { .. }));
        assert!(matches!(matches[1], BlockMatch::Deleted { .. }));
        assert!(matches!(matches[2], BlockMatch::Inserted { .. }));
        assert!(matches!(matches[3], BlockMatch::Unchanged { .. }));
    }
}

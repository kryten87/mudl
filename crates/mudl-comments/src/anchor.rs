//! Locating a comment's anchor point in the current document (Phase 14.3).
//! A scoped-down port of `mud`'s `Core/Sources/Comments/CommentAnchor.swift`.
//!
//! `mud`'s version maps an arbitrary offset *inside* a matched block (the
//! live WebKit selection's end, which can land mid-paragraph) and separately
//! special-cases GFM alert/DocC-aside title stripping, inline math spans,
//! and smart-typography folding. This port narrows the contract to what
//! `mudl-gui`'s selection flow actually needs: the caller always has the
//! **whole** quoted text (a captured, whitespace-collapsed selection), so
//! the only offset worth returning is the one anchoring point the design
//! calls out as sufficient -- the quotation's end -- and the extra
//! special-casing (which exists in `mud` only to resolve an *interior*
//! offset) doesn't apply.
//!
//! Always re-locates fresh from the current content (never position-tracked
//! through edits): parses `markdown`, finds the leaf block (paragraph,
//! heading, or GFM table cell) whose whitespace-collapsed text matches
//! `quotation`, disambiguated by `occurrence` for duplicate text, and
//! returns the byte offset just past that block's text -- ready for
//! `mudl_comments::editor::insert`. A block inside a footnote (or comment)
//! definition is never a match: those are the hidden bottom section, never
//! the selection's source.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

fn parser_options() -> Options {
    Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
}

/// A block whose children are inline content and so can be text-matched: a
/// paragraph, a heading, or a GFM table cell.
fn is_leaf_block(tag: &Tag) -> bool {
    matches!(tag, Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell)
}

/// Locates the byte offset just past the `occurrence`-th (0-based) block in
/// `markdown` whose whitespace-collapsed text equals `quotation`
/// (itself whitespace-collapsed before comparing). `None` when `quotation`
/// is empty, `occurrence` has no match, or the source has no such block at
/// all -- the caller treats that as "the quoted text no longer matches the
/// source" and refuses the edit rather than guessing.
pub fn locate(markdown: &str, quotation: &str, occurrence: usize) -> Option<usize> {
    let target = collapse(quotation);
    if target.is_empty() {
        return None;
    }

    let events: Vec<(Event, Range<usize>)> = Parser::new_ext(markdown, parser_options())
        .into_offset_iter()
        .collect();

    let mut in_definition: usize = 0;
    let mut match_index: usize = 0;
    let mut i = 0;
    while i < events.len() {
        match &events[i].0 {
            Event::Start(Tag::FootnoteDefinition(_)) => {
                in_definition += 1;
                i += 1;
                continue;
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                in_definition = in_definition.saturating_sub(1);
                i += 1;
                continue;
            }
            _ => {}
        }

        if in_definition == 0 {
            if let Event::Start(tag) = &events[i].0 {
                if is_leaf_block(tag) {
                    let (end_idx, end_byte) = block_end(&events, i);
                    let text = collapse(&plain_text(&events[i + 1..end_idx]));
                    if text == target {
                        if match_index == occurrence {
                            return Some(trim_trailing_newline(markdown, end_byte));
                        }
                        match_index += 1;
                    }
                    i = end_idx + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    None
}

/// Given the index of a `Start` event, walks forward (depth-counting every
/// nested `Start`/`End` regardless of kind, since a leaf block can contain
/// inline containers like emphasis or links but never another leaf block)
/// to find its matching `End`. Returns that `End`'s index and the byte just
/// past the block's own source span.
fn block_end(events: &[(Event, Range<usize>)], start_idx: usize) -> (usize, usize) {
    let mut depth = 1;
    let mut j = start_idx + 1;
    loop {
        match &events[j].0 {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return (j, events[j].1.end);
        }
        j += 1;
    }
}

/// `pulldown-cmark`'s block ranges include the newline terminating a
/// block's last source line whenever more source follows (see
/// `serialization::slice_body`'s doc comment for the same quirk) -- without
/// correcting for it, a marker anchored at a block's end would land one
/// byte into the next line rather than immediately after the quoted text.
fn trim_trailing_newline(source: &str, byte_offset: usize) -> usize {
    if source.as_bytes().get(byte_offset.wrapping_sub(1)) == Some(&b'\n') {
        byte_offset - 1
    } else {
        byte_offset
    }
}

/// The plain text of a run of events: `Text`/`Code` contribute their
/// literal, a soft or hard break contributes a space. A footnote reference
/// is zero-width (it renders as a marker glyph, not text) and simply
/// doesn't match any of those patterns, so it -- like every container's own
/// `Start`/`End` -- is already skipped without a special case.
fn plain_text(events: &[(Event, Range<usize>)]) -> String {
    let mut s = String::new();
    for (event, _) in events {
        match event {
            Event::Text(t) | Event::Code(t) => s.push_str(t),
            Event::SoftBreak | Event::HardBreak => s.push(' '),
            _ => {}
        }
    }
    s
}

/// Collapses every run of whitespace to a single space and trims the ends.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_anchors_at_paragraph_end() {
        let source = "The quick brown fox.\n";
        let offset = locate(source, "The quick brown fox.", 0).unwrap();
        assert_eq!(&source[..offset], "The quick brown fox.");
    }

    #[test]
    fn exact_match_not_at_end_of_file() {
        let source = "The quick brown fox.\n\nAnother paragraph.\n";
        let offset = locate(source, "The quick brown fox.", 0).unwrap();
        assert_eq!(&source[..offset], "The quick brown fox.");
    }

    #[test]
    fn whitespace_normalized_match() {
        let source = "The quick\nbrown   fox.\n";
        // A soft-wrapped, multiply-spaced quotation still matches after
        // collapsing whitespace on both sides.
        let offset = locate(source, "The   quick  brown fox.", 0).unwrap();
        assert_eq!(collapse(&source[..offset]), "The quick brown fox.");
    }

    #[test]
    fn duplicate_text_disambiguated_by_occurrence() {
        let source = "Same text.\n\nSame text.\n";
        let first = locate(source, "Same text.", 0).unwrap();
        let second = locate(source, "Same text.", 1).unwrap();
        assert!(first < second);
        assert_eq!(&source[..first], "Same text.");
    }

    #[test]
    fn occurrence_past_the_last_match_is_none() {
        let source = "Same text.\n\nSame text.\n";
        assert_eq!(locate(source, "Same text.", 2), None);
    }

    #[test]
    fn quotation_no_longer_present_is_none() {
        let source = "The quick brown fox.\n";
        assert_eq!(locate(source, "A sentence that is not here.", 0), None);
    }

    #[test]
    fn empty_quotation_is_none() {
        let source = "The quick brown fox.\n";
        assert_eq!(locate(source, "", 0), None);
        assert_eq!(locate(source, "   ", 0), None);
    }

    #[test]
    fn matches_a_heading() {
        let source = "# The Quick Brown Fox\n\nBody text.\n";
        let offset = locate(source, "The Quick Brown Fox", 0).unwrap();
        assert_eq!(&source[..offset], "# The Quick Brown Fox");
    }

    #[test]
    fn matches_a_paragraph_inside_a_list_item() {
        // A *loose* list (blank line between items): pulldown-cmark wraps
        // loose-item content in a real `Paragraph`, unlike a tight list's
        // bare inline text (out of scope here, matching the plan's leaf-
        // block set of paragraph/heading/table-cell).
        let source = "- The quick brown fox.\n\n- Another item.\n";
        let offset = locate(source, "The quick brown fox.", 0).unwrap();
        assert_eq!(&source[..offset], "- The quick brown fox.");
    }

    #[test]
    fn skips_blocks_inside_a_footnote_definition() {
        let source = "The quick brown fox.\n\n[^comment-a]:\n    The quick brown fox.\n";
        // Both the body paragraph and the footnote definition's body share
        // the same text; only the body copy (occurrence 0, and the only
        // match) is reachable -- the definition's copy is skipped.
        let offset = locate(source, "The quick brown fox.", 0).unwrap();
        assert_eq!(&source[..offset], "The quick brown fox.");
        assert_eq!(locate(source, "The quick brown fox.", 1), None);
    }
}

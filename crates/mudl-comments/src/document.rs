//! Collecting every comment out of a whole document. Ported from the
//! comment-classifying half of `mud`'s
//! `Core/Sources/Rendering/FootnoteProcessor.swift` (`process`/
//! `isCommentLabel`).
//!
//! Used both by the impure write flow ([`crate::write`], Phase 14.5 -- to
//! fetch a comment's current quotation/messages before replying to or
//! editing it) and, later, by `mudl-core`'s renderer (Phase 14's "wire
//! comments into `render_up`/`render_down`" step) to build the bottom
//! Comments section.

use pulldown_cmark::{Event, Options, Parser, Tag};
use std::collections::HashMap;
use std::ops::Range;

use crate::serialization::{self, Comment};

fn parser_options() -> Options {
    Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
}

/// True when `label` is a comment label (`comment-` followed by one or more
/// `[\w-]` characters) rather than an authorial footnote label. Mirrors
/// `mudl_core::footnotes::is_comment_label` -- duplicated rather than
/// imported, since `mudl-core` depends on this crate, not the other way
/// around (the same shape as `mudl-diff`'s duplicate of the same
/// predicate).
fn is_comment_label(label: &str) -> bool {
    match label.strip_prefix("comment-") {
        Some(suffix) if !suffix.is_empty() => suffix
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-'),
        _ => false,
    }
}

/// Collects every comment in `source`, parsed and ordered the way `mud`
/// orders the bottom Comments section: by first-reference position (the
/// byte of the first `[^label]` that points to it), falling back to
/// definition order for a comment whose reference cmark didn't surface (an
/// orphaned or reference-less definition). `ordinal` is assigned 1-based in
/// that order. Empty when `source` has no comments at all.
pub fn parse_comments(source: &str) -> Vec<Comment> {
    if !source.contains("[^") {
        return Vec::new();
    }
    let events: Vec<(Event, Range<usize>)> = Parser::new_ext(source, parser_options())
        .into_offset_iter()
        .collect();

    let mut first_ref: HashMap<String, usize> = HashMap::new();
    for (event, range) in &events {
        if let Event::FootnoteReference(label) = event {
            if is_comment_label(label) {
                first_ref.entry(label.to_string()).or_insert(range.start);
            }
        }
    }

    struct Draft {
        label: String,
        def_start: usize,
        quotation: Option<String>,
        messages: Vec<serialization::CommentMessage>,
    }
    let mut drafts: Vec<Draft> = Vec::new();

    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::FootnoteDefinition(label)) = &events[i].0 {
            let label = label.to_string();
            let def_start = events[i].1.start;
            let end_idx = block_end_index(&events, i);
            if is_comment_label(&label) {
                let children = direct_child_ranges(&events, i + 1, end_idx);
                let body = join_body_markdown(source, &children);
                let (quotation, messages) = serialization::parse(&body);
                drafts.push(Draft {
                    label,
                    def_start,
                    quotation,
                    messages,
                });
            }
            i = end_idx + 1;
            continue;
        }
        i += 1;
    }

    drafts.sort_by_key(|d| {
        let ref_pos = first_ref.get(&d.label).copied().unwrap_or(usize::MAX);
        (ref_pos, d.def_start)
    });

    drafts
        .into_iter()
        .enumerate()
        .map(|(idx, d)| Comment {
            label: d.label,
            ordinal: idx + 1,
            quotation: d.quotation,
            messages: d.messages,
        })
        .collect()
}

/// An authorial (non-comment) footnote definition, for `mudl-core`'s bottom
/// Footnotes section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FootnoteEntry {
    pub label: String,
    /// De-indented body Markdown, ready to render.
    pub body_markdown: String,
}

/// Collects every *authorial* footnote definition in `source` -- everything
/// `[^label]:` defines except a comment (`is_comment_label`). Definition
/// order; `mudl-core` assigns display numbers itself from first-reference
/// order (the same walk it already does to render markers), dropping any
/// entry that numbering doesn't reach (no reference anywhere resolves to
/// it), matching `mud`'s "an unreferenced footnote definition doesn't
/// appear in the section" rule.
pub fn parse_footnotes(source: &str) -> Vec<FootnoteEntry> {
    if !source.contains("[^") {
        return Vec::new();
    }
    let events: Vec<(Event, Range<usize>)> = Parser::new_ext(source, parser_options())
        .into_offset_iter()
        .collect();

    let mut entries = Vec::new();
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::FootnoteDefinition(label)) = &events[i].0 {
            let label = label.to_string();
            let end_idx = block_end_index(&events, i);
            if !is_comment_label(&label) {
                let children = direct_child_ranges(&events, i + 1, end_idx);
                let body_markdown = join_body_markdown(source, &children);
                entries.push(FootnoteEntry {
                    label,
                    body_markdown,
                });
            }
            i = end_idx + 1;
            continue;
        }
        i += 1;
    }
    entries
}

/// The index of the `End` event matching the `Start` at `start_idx`
/// (depth-counting every nested `Start`/`End` regardless of kind).
fn block_end_index(events: &[(Event, Range<usize>)], start_idx: usize) -> usize {
    let mut depth = 1;
    let mut j = start_idx + 1;
    while depth > 0 {
        match &events[j].0 {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        if depth > 0 {
            j += 1;
        }
    }
    j
}

/// The byte ranges of the direct child blocks in `events[start_idx..end_idx)`
/// (a `FootnoteDefinition`'s immediate children). Each range points to that
/// child's own verbatim source span -- for an indented continuation line,
/// `pulldown-cmark` already points past the leading indent, so no manual
/// de-indentation is needed.
fn direct_child_ranges(
    events: &[(Event, Range<usize>)],
    start_idx: usize,
    end_idx: usize,
) -> Vec<Range<usize>> {
    let mut children = Vec::new();
    let mut i = start_idx;
    while i < end_idx {
        if let Event::Start(_) = &events[i].0 {
            let start = events[i].1.start;
            let child_end_idx = block_end_index(events, i);
            children.push(start..events[child_end_idx].1.end);
            i = child_end_idx + 1;
        } else {
            i += 1;
        }
    }
    children
}

/// Joins a definition's direct-child source spans into de-indented body
/// Markdown ready for `serialization::parse`: each child's trailing newline
/// (see `serialization::slice_body`'s doc comment for that quirk) is
/// trimmed, and a fresh `"\n\n"` separates siblings -- semantically
/// equivalent to the original spacing regardless of how many blank lines
/// actually separated them in the source.
fn join_body_markdown(source: &str, children: &[Range<usize>]) -> String {
    children
        .iter()
        .map(|r| source[r.clone()].trim_end_matches('\n'))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_comments_is_empty() {
        assert!(parse_comments("Plain text, no comments.\n").is_empty());
    }

    #[test]
    fn single_general_comment() {
        let source = "Text.\n\n[^comment-a]: A note.\n";
        let comments = parse_comments(source);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].label, "comment-a");
        assert_eq!(comments[0].ordinal, 1);
        assert_eq!(comments[0].quotation, None);
        assert_eq!(comments[0].messages.len(), 1);
        assert_eq!(comments[0].messages[0].body, "A note.");
    }

    #[test]
    fn quoted_comment_with_attribution() {
        let source = "The quick brown fox.[^comment-a]\n\n\
             [^comment-a]: > quick brown\n\n    {JP}: Nice.\n";
        let comments = parse_comments(source);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].quotation.as_deref(), Some("quick brown"));
        assert_eq!(comments[0].messages[0].author.as_deref(), Some("JP"));
        assert_eq!(comments[0].messages[0].body, "Nice.");
    }

    #[test]
    fn thread_with_reply() {
        let source = "Text.[^comment-a]\n\n\
             [^comment-a]: 💬 {JP}: First.\n\n    💬 {Claude}: Second.\n";
        let comments = parse_comments(source);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].messages.len(), 2);
        assert_eq!(comments[0].messages[0].body, "First.");
        assert_eq!(comments[0].messages[1].body, "Second.");
    }

    #[test]
    fn footnote_definitions_are_not_comments() {
        let source = "Text.[^1]\n\n[^1]: An authorial footnote.\n";
        assert!(parse_comments(source).is_empty());
    }

    #[test]
    fn ordinal_follows_first_reference_order_not_definition_order() {
        // Definitions appear b-then-a, but "a" is referenced first in the
        // body, so it gets ordinal 1.
        let source = "Alpha[^comment-a] then beta[^comment-b].\n\n\
             [^comment-b]: Second definition, first? no.\n\n\
             [^comment-a]: First definition, but referenced first too.\n";
        let comments = parse_comments(source);
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].label, "comment-a");
        assert_eq!(comments[0].ordinal, 1);
        assert_eq!(comments[1].label, "comment-b");
        assert_eq!(comments[1].ordinal, 2);
    }

    #[test]
    fn unreferenced_comment_falls_back_to_definition_order() {
        let source = "Referenced[^comment-a].\n\n\
             [^comment-a]: Has a reference.\n\n\
             [^comment-b]: No reference anywhere.\n";
        let comments = parse_comments(source);
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].label, "comment-a");
        assert_eq!(comments[1].label, "comment-b");
    }

    // MARK: - parse_footnotes

    #[test]
    fn no_footnotes_is_empty() {
        assert!(parse_footnotes("Plain text, no footnotes.\n").is_empty());
    }

    #[test]
    fn authorial_footnote_is_collected() {
        let source = "Text.[^1]\n\n[^1]: An authorial footnote.\n";
        let footnotes = parse_footnotes(source);
        assert_eq!(footnotes.len(), 1);
        assert_eq!(footnotes[0].label, "1");
        assert_eq!(footnotes[0].body_markdown, "An authorial footnote.");
    }

    #[test]
    fn comment_definitions_are_not_footnotes() {
        let source = "Text.[^comment-a]\n\n[^comment-a]: A comment.\n";
        assert!(parse_footnotes(source).is_empty());
    }

    #[test]
    fn footnotes_and_comments_coexist() {
        let source = "Text.[^1][^comment-a]\n\n\
             [^1]: A footnote.\n\n\
             [^comment-a]: A comment.\n";
        assert_eq!(parse_footnotes(source).len(), 1);
        assert_eq!(parse_comments(source).len(), 1);
    }

    #[test]
    fn multiple_comments_are_all_collected() {
        let source = "Alpha[^comment-a] and beta[^comment-b].\n\n\
             [^comment-a]: First comment.\n\n\
             [^comment-b]: Second comment.\n";
        let comments = parse_comments(source);
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].messages[0].body, "First comment.");
        assert_eq!(comments[1].messages[0].body, "Second comment.");
    }
}

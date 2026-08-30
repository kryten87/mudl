//! Pure source rewriting for comments (Phase 14.4) -- no IO. Ported from
//! `mud`'s `Core/Sources/Comments/CommentEditor.swift`. Every edit is
//! **byte-surgical**: untouched bytes (line endings, indentation, the bytes
//! of unrelated blocks) are preserved exactly, so diffs stay minimal. The
//! one deliberate exception is trailing newlines at the foot of the file: an
//! edit that touches the file's last comment normalizes the end to exactly
//! one trailing newline, so the file stays git-clean and an add/delete
//! round-trip restores the original bytes.
//!
//! Locating an existing definition/its references goes through
//! `locate_definition`, which parses `source` with `pulldown-cmark`'s
//! footnote-aware mode -- the Rust analogue of `mud`'s
//! `FootnoteProcessor.locateComments`, which re-parses with `cmark-gfm` for
//! the same reason: a `[^comment-x]:` inside a code block is never mistaken
//! for a real definition.

use pulldown_cmark::{Event, Options, Parser, Tag};
use std::ops::Range;

use crate::labels;
use crate::serialization::{self, Comment, CommentMessage};

fn parser_options() -> Options {
    Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
}

/// The byte geometry of one comment's definition, for byte-surgical rewrites.
struct Location {
    /// Start byte of the definition's opener line (`[^label]:`).
    def_start: usize,
    /// End byte of the definition body's last line, before its newline --
    /// the span `rewrite` replaces.
    def_content_end: usize,
    /// End byte past the definition block *and* its trailing blank lines --
    /// the span `delete` removes.
    def_delete_end: usize,
    /// Byte ranges of each `[^label]` reference marker.
    ref_ranges: Vec<Range<usize>>,
}

/// Locates the `label` comment's definition and every reference to it, by
/// byte range. `None` when no such definition exists.
fn locate_definition(source: &str, label: &str) -> Option<Location> {
    if !source.contains("[^") {
        return None;
    }
    let events: Vec<(Event, Range<usize>)> = Parser::new_ext(source, parser_options())
        .into_offset_iter()
        .collect();

    let ref_ranges: Vec<Range<usize>> = events
        .iter()
        .filter_map(|(event, range)| match event {
            Event::FootnoteReference(lbl) if lbl.as_ref() == label => Some(range.clone()),
            _ => None,
        })
        .collect();

    let def_start_idx = events.iter().position(|(event, _)| {
        matches!(event, Event::Start(Tag::FootnoteDefinition(lbl)) if lbl.as_ref() == label)
    })?;
    let def_start = events[def_start_idx].1.start;

    // Depth-count forward from the definition's `Start` to its matching
    // `End` -- the event immediately before it is always the closing `End`
    // of the definition's last direct child block, whatever it is.
    let mut depth = 1;
    let mut j = def_start_idx + 1;
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
    let def_delete_end = events[j].1.end;
    let def_content_end = trim_trailing_newline(source, events[j - 1].1.end);

    Some(Location {
        def_start,
        def_content_end,
        def_delete_end,
        ref_ranges,
    })
}

/// See `serialization::slice_body`'s doc comment: a `pulldown-cmark` block
/// range includes the newline terminating its last source line whenever
/// more source follows.
fn trim_trailing_newline(source: &str, byte_offset: usize) -> usize {
    if source.as_bytes().get(byte_offset.wrapping_sub(1)) == Some(&b'\n') {
        byte_offset - 1
    } else {
        byte_offset
    }
}

/// Inserts a new comment: splices the `[^label]` marker at
/// `marker_byte_offset` (the selection end, mapped to a source byte by the
/// caller -- see `anchor::locate`) and appends a canonical definition at the
/// end of the document. Returns the rewritten source and the new `Comment`
/// (its `ordinal` is assigned at render time).
pub fn insert(
    source: &str,
    marker_byte_offset: usize,
    quotation: Option<&str>,
    message: CommentMessage,
) -> (String, Comment) {
    let label = labels::next_label(&labels::existing_labels(source));

    let mut bytes = source.as_bytes().to_vec();
    let clamped = marker_byte_offset.min(bytes.len());
    let marker = format!("[^{label}]");
    bytes.splice(clamped..clamped, marker.into_bytes());
    let with_marker = String::from_utf8_lossy(&bytes).into_owned();

    let body = serialization::serialize(quotation, std::slice::from_ref(&message));
    let definition = format!("[^{label}]:\n{}", indent_body(&body));
    let with_definition = append_definition(&with_marker, &definition);

    let comment = Comment {
        label,
        ordinal: 0,
        quotation: quotation.map(str::to_string),
        messages: vec![message],
    };
    (with_definition, comment)
}

/// Replaces the body of the `label` definition with a freshly serialized
/// quotation + messages. Covers edit, reply (caller passes `existing +
/// [newMessage]`), and removing one message of a thread. The marker and the
/// quotation's anchor are untouched. Returns `None` when the label has no
/// definition (the comment changed or was removed on disk) -- writing the
/// source back unchanged would falsely report success.
pub fn rewrite(
    source: &str,
    label: &str,
    quotation: Option<&str>,
    messages: &[CommentMessage],
) -> Option<String> {
    let loc = locate_definition(source, label)?;

    let body = serialization::serialize(quotation, messages);
    let rebuilt = format!("[^{label}]:\n{}", indent_body(&body));

    let bytes = source.as_bytes();
    // When everything past the definition's content is just trailing
    // newlines, the comment is the file's last content: replace through
    // end-of-file and end with a single newline (git-clean). Otherwise keep
    // the bytes after the definition (the blank line and the block that
    // follows it) untouched.
    let tail_is_all_newlines = bytes[loc.def_content_end..]
        .iter()
        .all(|&b| b == b'\n' || b == b'\r');

    let mut new_bytes = bytes.to_vec();
    let mut replacement = rebuilt.into_bytes();
    let replace_end = if tail_is_all_newlines {
        replacement.push(b'\n');
        new_bytes.len()
    } else {
        loc.def_content_end
    };
    new_bytes.splice(loc.def_start..replace_end, replacement);
    Some(String::from_utf8_lossy(&new_bytes).into_owned())
}

/// Removes a comment entirely -- its definition (and trailing blank lines)
/// plus every `[^label]` marker -- leaving the label gap rather than
/// renumbering later labels, and normalizing the foot of the file to a
/// single trailing newline when the removal reaches it. Returns `None` when
/// the label has no definition (the comment changed or was removed on
/// disk).
pub fn delete(source: &str, label: &str) -> Option<String> {
    let loc = locate_definition(source, label)?;

    let mut ranges: Vec<(usize, usize)> = loc.ref_ranges.iter().map(|r| (r.start, r.end)).collect();
    ranges.push((loc.def_start, loc.def_delete_end));
    ranges.sort_by_key(|&(start, _)| std::cmp::Reverse(start));

    let mut bytes = source.as_bytes().to_vec();
    // Did this comment end the file? Only then does its removal expose
    // dangling blank lines to trim -- a comment deleted from the middle of
    // the document leaves the file's own trailing newline untouched.
    let reached_end = loc.def_delete_end == bytes.len();
    for (start, end) in ranges {
        bytes.drain(start..end);
    }
    if reached_end {
        while matches!(bytes.last(), Some(b'\n') | Some(b'\r')) {
            bytes.pop();
        }
        if !bytes.is_empty() {
            bytes.push(b'\n');
        }
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Indents every line of a serialized body by four spaces (the GFM footnote
/// continuation indent); blank lines stay empty.
fn indent_body(body: &str) -> String {
    body.split('\n')
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("    {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Appends a definition to `source` after exactly one blank line,
/// normalizing any trailing newlines so the diff is a clean append. The
/// result ends with a single trailing newline (git-clean).
fn append_definition(source: &str, definition: &str) -> String {
    let trimmed = source.trim_end_matches('\n');
    if trimmed.is_empty() {
        return format!("{definition}\n");
    }
    format!("{trimmed}\n\n{definition}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialization::parse_timestamp;

    fn ts(s: &str) -> Option<crate::serialization::Timestamp> {
        parse_timestamp(s)
    }

    fn msg(author: Option<&str>, created: Option<&str>, body: &str) -> CommentMessage {
        CommentMessage {
            author: author.map(str::to_string),
            created: created.and_then(ts),
            body: body.to_string(),
        }
    }

    // MARK: - insert

    #[test]
    fn insert_splices_marker_and_appends_definition() {
        let (source, comment) = insert("Hello world.\n", 11, None, msg(None, None, "Note."));

        assert_eq!(comment.label, "comment-a");
        assert_eq!(comment.quotation, None);
        assert_eq!(comment.messages.len(), 1);
        assert!(source.contains("Hello world[^comment-a]."));
        assert!(source.contains("[^comment-a]:\n    Note."));
    }

    #[test]
    fn insert_at_end_of_file_ends_with_single_newline() {
        let (source, _) = insert("Hello world.\n", 11, None, msg(None, None, "Note."));
        assert!(source.ends_with("    Note.\n"));
        assert!(!source.ends_with("\n\n"));
    }

    #[test]
    fn insert_then_delete_restores_original() {
        // The README round-trip: adding then removing a comment must leave
        // the file byte-for-byte unchanged, trailing newline and all.
        let original = "# Title\n\nBody text.\n";
        let (inserted, comment) = insert(original, 18, None, msg(None, None, "Note."));
        let restored = delete(&inserted, &comment.label).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn insert_with_quotation_writes_leading_blockquote() {
        let (source, _) = insert(
            "The quick brown fox.\n",
            19,
            Some("quick brown"),
            msg(Some("JP"), Some("2026-06-01 18:33:00"), "Nice."),
        );

        assert!(source.contains("brown fox[^comment-a]."));
        assert!(source.contains("[^comment-a]:\n    > quick brown"));
        assert!(source.contains("💬 {JP @ 2026-06-01 18:33:00}:"));
        assert!(source.contains("Nice."));
    }

    // MARK: - rewrite

    #[test]
    fn rewrite_replaces_body_keeps_marker() {
        let (inserted, _) = insert("Hello world.\n", 11, None, msg(None, None, "Note."));

        let rewritten = rewrite(
            &inserted,
            "comment-a",
            None,
            &[
                msg(Some("JP"), Some("2026-06-01 18:33:00"), "First."),
                msg(Some("Claude"), Some("2026-06-01 18:34:00"), "Second."),
            ],
        )
        .unwrap();

        assert!(rewritten.contains("Hello world[^comment-a].")); // marker intact
        assert!(rewritten.contains("💬 {JP @ 2026-06-01 18:33:00}:"));
        assert!(rewritten.contains("💬 {Claude @ 2026-06-01 18:34:00}:"));
        assert!(rewritten.contains("First."));
        assert!(rewritten.contains("Second."));
        assert!(!rewritten.contains("Note.")); // old body replaced
    }

    #[test]
    fn rewrite_keeps_blank_line_before_following_block() {
        // A comment definition mid-document, with a following paragraph.
        // Rewriting its body must not swallow the blank line that separates
        // the two.
        let source = "Text[^comment-a] here.\n\n\
             [^comment-a]: > here\n\n\
             \x20\x20\x20\x20A note.\n\n\
             Following paragraph.\n";

        let rewritten = rewrite(
            source,
            "comment-a",
            Some("here"),
            &[msg(None, None, "A note."), msg(None, None, "A reply.")],
        )
        .unwrap();

        // The reply is a distinct message (bare `💬`), and the following
        // paragraph stays separated by a blank line rather than merging
        // into the definition.
        assert!(rewritten.contains("    💬 A reply."));
        assert!(rewritten.contains("    💬 A reply.\n\nFollowing paragraph."));
    }

    #[test]
    fn rewrite_at_end_of_file_ends_with_single_newline() {
        let (inserted, _) = insert("Hello world.\n", 11, None, msg(None, None, "Note."));

        let rewritten = rewrite(
            &inserted,
            "comment-a",
            None,
            &[msg(None, None, "Note."), msg(None, None, "Reply.")],
        )
        .unwrap();

        assert!(rewritten.ends_with("    💬 Reply.\n"));
        assert!(!rewritten.ends_with("\n\n"));
    }

    // MARK: - delete

    #[test]
    fn delete_removes_one_comment_leaves_others() {
        let source = "Alpha[^comment-a] and beta[^comment-b].\n\n\
             [^comment-a]: First comment.\n\n\
             [^comment-b]: Second comment.\n";

        let after = delete(source, "comment-a").unwrap();

        assert!(!after.contains("comment-a")); // marker + definition gone
        assert!(after.contains("beta[^comment-b].")); // other marker intact
        assert!(after.contains("[^comment-b]: Second comment.")); // other def intact
        assert!(after.contains("Alpha and beta")); // surrounding text intact
    }

    #[test]
    fn delete_collapses_trailing_newlines_to_one() {
        let source = "Body[^comment-a] text.\n\n\
             [^comment-a]: A trailing comment.\n\n\n";

        let after = delete(source, "comment-a").unwrap();

        assert!(after.ends_with("Body text.\n"));
        assert!(!after.ends_with("\n\n"));
    }

    #[test]
    fn delete_keeps_trailing_newline_when_comment_not_last() {
        // The definition is followed by more content, so its removal does
        // not reach end-of-file: the file's own trailing newline must
        // survive.
        let source = "Body[^comment-a] text.\n\n\
             [^comment-a]: A comment.\n\n\
             More content.\n";

        let after = delete(source, "comment-a").unwrap();

        assert!(!after.contains("comment-a"));
        assert!(after.contains("Body text."));
        assert!(after.ends_with("More content.\n"));
    }

    // MARK: - Missing label

    #[test]
    fn rewrite_missing_label_returns_none() {
        // A vanished definition must be reported, not papered over:
        // returning the source unchanged would let the caller write it
        // back and claim success.
        let source = "Plain text, no comments.\n";
        let result = rewrite(source, "comment-a", None, &[msg(None, None, "Note.")]);
        assert_eq!(result, None);
    }

    #[test]
    fn delete_missing_label_returns_none() {
        let source = "Plain text, no comments.\n";
        assert_eq!(delete(source, "comment-a"), None);
    }
}

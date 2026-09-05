//! The read/write codec for a comment definition's body (Phase 14.1). Ported
//! from `mud`'s `Core/Sources/Comments/CommentSerialization.swift` (see also
//! `Comment.swift` for the `Comment`/`CommentMessage` model).
//!
//! `parse` takes a footnote definition's **already de-indented** body
//! Markdown (`mudl-comments::editor`, Phase 14.4, is responsible for
//! stripping the four-space continuation indent before calling this) and
//! structures it into a root quotation plus ordered messages. `serialize` is
//! the strict inverse, with the round-trip invariant
//! `parse(serialize(quotation, messages)) == (quotation, messages)`.
//!
//! Unlike the Swift original (which re-derives each block's verbatim source
//! from 1-based start/end *lines*, because `cmark-gfm`'s C API only exposes
//! line/column positions), this parses with `pulldown-cmark`'s
//! `into_offset_iter`, which hands back an exact byte `Range` per event — so
//! a message body no one has edited is sliced out of the source by its own
//! byte range rather than reconstructed line-by-line. Same behavior, a
//! simpler mechanism.
//!
//! Message attributes live in **braces** — `💬 {author @ timestamp}:` —
//! where the `💬`, both fields, and the trailing colon are each optional and
//! a paragraph-leading `{` is the signal.

use pulldown_cmark::{Event, Options, Parser, Tag};
use std::ops::Range;

const COMMENT_EMOJI: &str = "💬";

/// A comment stored in a Markdown document as a GFM footnote whose label
/// matches `^comment-[\w-]+$` (`mudl_core::footnotes::is_comment_label`,
/// Phase 1.8).
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    /// The footnote label, e.g. `comment-a`.
    pub label: String,
    /// 1-based document-order position. Display only, assigned by the
    /// caller at render time — this module never sets it meaningfully.
    pub ordinal: usize,
    /// The root blockquote text, whitespace-collapsed; `None` for a general
    /// (unanchored) comment.
    pub quotation: Option<String>,
    /// One message per attributes block (or a single author-less message
    /// when the body carries no header).
    pub messages: Vec<CommentMessage>,
}

/// One message in a comment thread, introduced on disk by a
/// `💬 {author @ timestamp}:` attributes block.
#[derive(Debug, Clone, PartialEq)]
pub struct CommentMessage {
    /// The brace text before the timestamp's `@`; `None` if unattributed.
    pub author: Option<String>,
    /// Parsed from the brace's `@ <timestamp>`; `None` if the header carries
    /// no parseable timestamp.
    pub created: Option<Timestamp>,
    /// The commentary as Markdown.
    pub body: String,
}

/// A local wall-clock timestamp — mudl's hand-rolled stand-in for Swift's
/// `Date` (parsed/formatted as `en_US_POSIX`/`.current`, i.e. deliberately
/// zone-less). Field order gives correct chronological `Ord` for free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

// MARK: - Read

/// Structures a comment definition's de-indented body Markdown into the root
/// quotation and ordered messages.
pub fn parse(body_markdown: &str) -> (Option<String>, Vec<CommentMessage>) {
    let mut blocks = top_level_blocks(body_markdown);

    // (1) A leading blockquote -- and only a leading one, before any message
    // -- is the root quotation. A blockquote that follows a message header
    // belongs to that message's body.
    let mut quotation = None;
    if let Some(first) = blocks.first() {
        if matches!(first.tag, Tag::BlockQuote(_)) {
            let block = blocks.remove(0);
            quotation = Some(flatten(&plain_text(&block.inner)));
        }
    }

    // (2) Split the remaining blocks into messages at every paragraph that
    // *begins* with a message attributes block -- a `💬` or a `{`. Blocks
    // before the first such paragraph (or all of them, when there is none)
    // form one implicit author-less message.
    let mut groups: Vec<Vec<Block>> = Vec::new();
    let mut current: Vec<Block> = Vec::new();
    for block in blocks {
        if is_message_start(&block) && !current.is_empty() {
            groups.push(std::mem::take(&mut current));
        }
        current.push(block);
    }
    if !current.is_empty() {
        groups.push(current);
    }

    let messages = groups
        .into_iter()
        .map(|group| build_message(group, body_markdown))
        .collect();
    (quotation, messages)
}

/// True when `block` is a paragraph whose text begins (after any leading
/// whitespace) with a message attributes block -- the `💬` header emoji or a
/// `{` brace. A `💬` or `{` anywhere else in running prose never splits a
/// message.
fn is_message_start(block: &Block) -> bool {
    if !matches!(block.tag, Tag::Paragraph) {
        return false;
    }
    let text = plain_text(&block.inner);
    let trimmed = text.trim_start();
    trimmed.starts_with(COMMENT_EMOJI) || trimmed.starts_with('{')
}

/// Builds a `CommentMessage` from its block group. The first paragraph is
/// run through `parse_attribution`; when it carries a header, that
/// paragraph is the header and the rest is the body. Otherwise the whole
/// group is an unattributed body.
fn build_message(blocks: Vec<Block>, source: &str) -> CommentMessage {
    if let Some(first) = blocks.first() {
        if matches!(first.tag, Tag::Paragraph) {
            let (author, created, inline_body, is_header) =
                parse_attribution(&plain_text(&first.inner));
            if is_header {
                let mut parts = Vec::new();
                if !inline_body.is_empty() {
                    parts.push(inline_body);
                }
                if let Some(body) = slice_body(&blocks[1..], source) {
                    parts.push(body);
                }
                return CommentMessage {
                    author,
                    created,
                    body: parts.join("\n\n"),
                };
            }
        }
    }
    CommentMessage {
        author: None,
        created: None,
        body: slice_body(&blocks, source).unwrap_or_default(),
    }
}

// MARK: - Write

/// Renders a quotation + messages into the strict canonical body Markdown
/// (un-indented): the quotation as a leading blockquote, then one
/// `💬 {author @ timestamp}:` header per attributed message -- alone on its
/// line, commentary in the block below. The caller (`mudl_comments::editor`)
/// prefixes `[^label]:` and indents continuation lines by four spaces.
pub fn serialize(quotation: Option<&str>, messages: &[CommentMessage]) -> String {
    let mut blocks: Vec<String> = Vec::new();
    if let Some(q) = quotation {
        if !q.is_empty() {
            blocks.push(format!("> {q}"));
        }
    }
    for (index, message) in messages.iter().enumerate() {
        if let Some(header) = header_line(message) {
            blocks.push(header);
            if !message.body.is_empty() {
                blocks.push(message.body.clone());
            }
        } else if index > 0 {
            // A new message with no attribution still needs a bare `💬` to
            // mark it -- without one, re-parsing would merge it into the
            // previous message. The first message needs no marker.
            blocks.push(if message.body.is_empty() {
                COMMENT_EMOJI.to_string()
            } else {
                format!("{COMMENT_EMOJI} {}", message.body)
            });
        } else if !message.body.is_empty() {
            blocks.push(message.body.clone());
        }
    }
    blocks.join("\n\n")
}

/// The `💬 {author @ timestamp}:` header for an attributed message, or
/// `None` for a bare unattributed message (which serializes as body alone).
/// With only one field present the brace carries just that field:
/// `{author}` or `{@ timestamp}`.
fn header_line(message: &CommentMessage) -> Option<String> {
    let author = message.author.as_deref().filter(|a| !a.is_empty());
    if author.is_none() && message.created.is_none() {
        return None;
    }
    let mut interior = author.unwrap_or("").to_string();
    if let Some(created) = &message.created {
        let stamp = format_timestamp(created);
        interior = if interior.is_empty() {
            format!("@ {stamp}")
        } else {
            format!("{interior} @ {stamp}")
        };
    }
    Some(format!("{COMMENT_EMOJI} {{{interior}}}:"))
}

// MARK: - Attribution grammar

/// Peels a leading message attributes block -- `[💬 ]{author @ timestamp}[:]`
/// -- from a message's first paragraph. The `💬` is optional and the braces
/// are the signal: a paragraph that (after an optional `💬`) begins with `{`
/// carries attributes, even when they are empty (`{}`) or hold one field. A
/// `💬` with no following brace is itself a (no-attribute) header. The
/// returned `is_header` lets the caller peel such a bare marker even when it
/// yields no author or timestamp.
///
/// Inside the braces, the **last** `@` whose trailing text parses as a
/// timestamp splits `author` from `created`; with no such `@` the whole
/// interior is the author (so an author may contain `@`).
pub fn parse_attribution(
    paragraph_text: &str,
) -> (Option<String>, Option<Timestamp>, String, bool) {
    let mut s = paragraph_text.trim_start_matches([' ', '\t']);

    let mut saw_emoji = false;
    if let Some(rest) = s.strip_prefix(COMMENT_EMOJI) {
        saw_emoji = true;
        s = rest.trim_start_matches([' ', '\t']);
    }

    // Brace form (canonical). The `{...}` must open the (post-💬) text; a
    // `{` later in the paragraph is ordinary prose.
    if let Some(after_brace) = s.strip_prefix('{') {
        if let Some(close) = after_brace.find('}') {
            let interior = &after_brace[..close];
            let (author, created) = parse_brace_interior(interior);
            // The colon is optional but, when present, must *immediately*
            // follow `}` -- a space before it makes the colon message
            // content.
            let mut rest = &after_brace[close + 1..];
            if let Some(stripped) = rest.strip_prefix(':') {
                rest = stripped;
            }
            let rest = rest.trim_start_matches([' ', '\t']);
            return (author, created, rest.to_string(), true);
        }
    }

    // A bare `💬` with no brace is still a header carrying no attributes.
    if saw_emoji {
        return (None, None, s.to_string(), true);
    }

    (None, None, paragraph_text.to_string(), false)
}

/// Splits a brace interior into author and timestamp at the **last** `@`
/// whose trailing text parses as a timestamp. With no such `@`, the whole
/// (trimmed) interior is the author. An empty interior yields neither.
fn parse_brace_interior(interior: &str) -> (Option<String>, Option<Timestamp>) {
    let at_positions: Vec<usize> = interior
        .char_indices()
        .filter(|&(_, c)| c == '@')
        .map(|(i, _)| i)
        .collect();
    for &at in at_positions.iter().rev() {
        let suffix = interior[at + '@'.len_utf8()..].trim();
        if let Some(created) = parse_timestamp(suffix) {
            let author = interior[..at].trim();
            return (non_empty(author), Some(created));
        }
    }
    (non_empty(interior.trim()), None)
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

// MARK: - Timestamp grammar

/// Parses `YYYY-MM-DD`, `YYYY-MM-DD HH:MM`, or `YYYY-MM-DD HH:MM:SS` as a
/// local wall clock. Strict: a single space separates date and time, every
/// field is zero-padded to its fixed width, and anything else (extra
/// whitespace, wrong digit counts, trailing garbage, an out-of-range field)
/// is rejected.
pub fn parse_timestamp(s: &str) -> Option<Timestamp> {
    let (date_part, time_part) = match s.split_once(' ') {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let (year, month, day) = parse_date(date_part)?;
    let (hour, minute, second) = match time_part {
        None => (0, 0, 0),
        Some(t) => parse_time(t)?,
    };
    Some(Timestamp {
        year,
        month,
        day,
        hour,
        minute,
        second,
    })
}

fn parse_date(s: &str) -> Option<(i32, u8, u8)> {
    let parts: Vec<&str> = s.split('-').collect();
    let [y, m, d] = parts[..] else { return None };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let year = parse_exact_digits(y)? as i32;
    let month = parse_exact_digits(m)? as u8;
    let day = parse_exact_digits(d)? as u8;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

fn parse_time(s: &str) -> Option<(u8, u8, u8)> {
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts[..] {
        [h, m] => (h, m, "00"),
        [h, m, s] => (h, m, s),
        _ => return None,
    };
    if h.len() != 2 || m.len() != 2 || sec.len() != 2 {
        return None;
    }
    let hour = parse_exact_digits(h)? as u8;
    let minute = parse_exact_digits(m)? as u8;
    let second = parse_exact_digits(sec)? as u8;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some((hour, minute, second))
}

fn parse_exact_digits(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Formats a `Timestamp` as the canonical `YYYY-MM-DD HH:MM:SS` local wall
/// clock.
pub fn format_timestamp(t: &Timestamp) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// The same wall clock as `format_timestamp` in the form an HTML
/// `<time datetime="...">` takes: `YYYY-MM-DDTHH:MM:SS`. Deliberately
/// zone-less -- a comment's on-disk stamp is a bare local wall clock, so a
/// *floating* date-time is its honest HTML rendering.
pub fn iso_timestamp(t: &Timestamp) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

// MARK: - cmark walk

/// A top-level block of a `pulldown-cmark` parse: its tag, its verbatim
/// source byte range, and the flat run of events strictly inside it
/// (nested Start/End tags included, so a container's descendant text is
/// still reachable via `plain_text`).
struct Block<'a> {
    tag: Tag<'a>,
    range: Range<usize>,
    inner: Vec<(Event<'a>, Range<usize>)>,
}

fn body_parser_options() -> Options {
    Options::ENABLE_STRIKETHROUGH
}

fn top_level_blocks(markdown: &str) -> Vec<Block<'_>> {
    let events: Vec<_> = Parser::new_ext(markdown, body_parser_options())
        .into_offset_iter()
        .collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(tag) = &events[i].0 {
            let tag = tag.clone();
            let start = events[i].1.start;
            let mut depth = 1;
            let mut inner = Vec::new();
            let mut j = i + 1;
            let end;
            loop {
                match &events[j].0 {
                    Event::Start(_) => depth += 1,
                    Event::End(_) => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    end = events[j].1.end;
                    j += 1;
                    break;
                }
                inner.push(events[j].clone());
                j += 1;
            }
            blocks.push(Block {
                tag,
                range: start..end,
                inner,
            });
            i = j;
        } else {
            i += 1;
        }
    }
    blocks
}

/// The plain text of a run of events: `Text`/`Code` contribute their
/// literal, a soft or hard break contributes a space, everything else (a
/// container's own `Start`/`End`, an image, ...) contributes nothing --
/// matching the Swift original's recursive `plainText(of:)`, since a flat
/// `pulldown-cmark` event run already includes every descendant leaf.
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

/// Collapses every run of whitespace (including block boundaries flattened
/// by `plain_text`) to a single space and trims the ends.
fn flatten(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The verbatim source of a run of top-level blocks: sliced directly from
/// `source` by the first block's start byte through the last block's end
/// byte. Slicing the source -- rather than re-serializing each block
/// through a Markdown formatter -- is what lets an unedited message
/// round-trip byte-for-byte. `None` for an empty run.
///
/// `pulldown-cmark`'s block `Range`s include the newline terminating a
/// block's last source line when one follows in the source (there is
/// nothing after the trailing block to swallow it, unlike blank-line
/// separators between sibling blocks, which fall entirely outside every
/// block's range) -- a parser artifact, never meaningful content, so it is
/// trimmed here rather than reappearing as a spurious trailing byte on
/// every body that isn't the file's last block.
fn slice_body(blocks: &[Block], source: &str) -> Option<String> {
    let first = blocks.first()?;
    let last = blocks.last()?;
    let slice = &source[first.range.start..last.range.end];
    Some(slice.strip_suffix('\n').unwrap_or(slice).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Option<Timestamp> {
        parse_timestamp(s)
    }

    // MARK: - Spec examples (parse)

    #[test]
    fn comment_a_bare_comment() {
        let (quotation, messages) =
            parse("The simplest comment. No quotation, no author, no timestamp.");
        assert_eq!(quotation, None);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].author, None);
        assert_eq!(messages[0].created, None);
        assert_eq!(
            messages[0].body,
            "The simplest comment. No quotation, no author, no timestamp."
        );
    }

    #[test]
    fn comment_b_quoted_no_attributes() {
        let (quotation, messages) = parse("> fox\n\nA quoted comment, no attributes.");
        assert_eq!(quotation.as_deref(), Some("fox"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].author, None);
        assert_eq!(messages[0].created, None);
        assert_eq!(messages[0].body, "A quoted comment, no attributes.");
    }

    #[test]
    fn comment_c_attributed_inline_body() {
        let (quotation, messages) =
            parse("> brown fox\n\n{JP @ 2026-06-01 18:33}: A message with author and timestamp.");
        assert_eq!(quotation.as_deref(), Some("brown fox"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].author.as_deref(), Some("JP"));
        assert_eq!(messages[0].created, ts("2026-06-01 18:33"));
        assert_eq!(messages[0].body, "A message with author and timestamp.");
    }

    #[test]
    fn comment_d_thread_with_reply_blockquote() {
        let (quotation, messages) = parse(
            "> quick brown fox\n\n\
             💬 {JP @ 2026-06-01 18:33}:\n\n\
             First message in the thread.\n\n\
             💬 {Claude Opus 4.8 @ 2026-06-01 18:33:13}:\n\n\
             > First message in the thread.\n\n\
             Second message in the thread.",
        );
        assert_eq!(quotation.as_deref(), Some("quick brown fox"));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].author.as_deref(), Some("JP"));
        assert_eq!(messages[0].created, ts("2026-06-01 18:33"));
        assert_eq!(messages[0].body, "First message in the thread.");
        assert_eq!(messages[1].author.as_deref(), Some("Claude Opus 4.8"));
        assert_eq!(messages[1].created, ts("2026-06-01 18:33:13"));
        assert!(messages[1].body.contains("First message in the thread."));
        assert!(messages[1].body.contains("Second message in the thread."));
        assert!(messages[1].body.starts_with('>'));
    }

    #[test]
    fn comment_e_brace_header_splits_without_emoji() {
        let (quotation, messages) = parse(
            "> The quick brown fox\n\n\
             {JP @ 2026-06-01 18:33}:\n\n\
             First message in the thread.\n\n\
             {Claude Opus 4.8 @ 2026-06-01 18:33:13}:\n\n\
             Second message in the thread.",
        );
        assert_eq!(quotation.as_deref(), Some("The quick brown fox"));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].author.as_deref(), Some("JP"));
        assert_eq!(messages[0].created, ts("2026-06-01 18:33"));
        assert_eq!(messages[0].body, "First message in the thread.");
        assert_eq!(messages[1].author.as_deref(), Some("Claude Opus 4.8"));
        assert_eq!(messages[1].created, ts("2026-06-01 18:33:13"));
        assert_eq!(messages[1].body, "Second message in the thread.");
    }

    #[test]
    fn comment_f_emoji_in_prose_does_not_split() {
        let (quotation, messages) = parse(
            "> fox\n\n\
             💬 {JP @ 2026-06-01 18:33}:\n\n\
             A single message. The body mentions a 💬 mid-sentence, which must not split.",
        );
        assert_eq!(quotation.as_deref(), Some("fox"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].author.as_deref(), Some("JP"));
        assert!(messages[0].body.contains('💬'));
    }

    #[test]
    fn comment_g_header_without_colon() {
        let (quotation, messages) = parse(
            "> brown fox\n\n\
             💬 {JP @ 2026-06-01 18:33}\n\n\
             The colon after the closing brace is optional; this block omits it.",
        );
        assert_eq!(quotation.as_deref(), Some("brown fox"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].author.as_deref(), Some("JP"));
        assert_eq!(messages[0].created, ts("2026-06-01 18:33"));
        assert_eq!(
            messages[0].body,
            "The colon after the closing brace is optional; this block omits it."
        );
    }

    #[test]
    fn comment_h_general_and_threaded() {
        let (quotation, messages) = parse(
            "💬 {JP @ 2026-06-01 18:33}:\n\n\
             A general message with no quotation, but part of a thread.\n\n\
             💬 {Claude Opus 4.8 @ 2026-06-01 18:33:13}:\n\n\
             A reply, also with no document quotation.",
        );
        assert_eq!(quotation, None);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].author.as_deref(), Some("JP"));
        assert_eq!(
            messages[0].body,
            "A general message with no quotation, but part of a thread."
        );
        assert_eq!(messages[1].author.as_deref(), Some("Claude Opus 4.8"));
        assert_eq!(
            messages[1].body,
            "A reply, also with no document quotation."
        );
    }

    #[test]
    fn comment_i_author_only() {
        let (quotation, messages) =
            parse("> fox\n\n{JP}: A message with an author but no timestamp.");
        assert_eq!(quotation.as_deref(), Some("fox"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].author.as_deref(), Some("JP"));
        assert_eq!(messages[0].created, None);
        assert_eq!(
            messages[0].body,
            "A message with an author but no timestamp."
        );
    }

    #[test]
    fn comment_j_date_only_no_author() {
        let (quotation, messages) =
            parse("> fox\n\n{@ 2026-06-01}: A message with a timestamp but no author.");
        assert_eq!(quotation.as_deref(), Some("fox"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].author, None);
        assert_eq!(messages[0].created, ts("2026-06-01"));
        assert_eq!(
            messages[0].body,
            "A message with a timestamp but no author."
        );
    }

    #[test]
    fn comment_k_author_containing_at() {
        // The only `@` is followed by `jp`, which is not a timestamp, so
        // nothing splits and the whole interior is the author.
        let (quotation, messages) = parse("> fox\n\n{@jp}: An author that is an @-handle.");
        assert_eq!(quotation.as_deref(), Some("fox"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].author.as_deref(), Some("@jp"));
        assert_eq!(messages[0].created, None);
        assert_eq!(messages[0].body, "An author that is an @-handle.");
    }

    #[test]
    fn comment_l_empty_braces() {
        let (quotation, messages) = parse("> fox\n\n{}: Empty braces carry no attributes.");
        assert_eq!(quotation.as_deref(), Some("fox"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].author, None);
        assert_eq!(messages[0].created, None);
        assert_eq!(messages[0].body, "Empty braces carry no attributes.");
    }

    #[test]
    fn comment_m_bare_emoji_unattributed_thread() {
        let (quotation, messages) = parse(
            "> fox\n\n\
             💬\n\n\
             A threaded message with no author or timestamp.\n\n\
             💬\n\n\
             A reply, also unattributed.",
        );
        assert_eq!(quotation.as_deref(), Some("fox"));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].author, None);
        assert_eq!(messages[0].created, None);
        assert_eq!(
            messages[0].body,
            "A threaded message with no author or timestamp."
        );
        assert_eq!(messages[1].author, None);
        assert_eq!(messages[1].body, "A reply, also unattributed.");
    }

    #[test]
    fn empty_input_has_no_quotation_and_no_messages() {
        let (quotation, messages) = parse("");
        assert_eq!(quotation, None);
        assert!(messages.is_empty());
    }

    // MARK: - Attributes / timestamp grammar

    #[test]
    fn attribution_brace_author_and_timestamp() {
        let (author, created, body, is_header) =
            parse_attribution("{JP @ 2026-06-01 18:33}: the body");
        assert!(is_header);
        assert_eq!(author.as_deref(), Some("JP"));
        assert_eq!(created, ts("2026-06-01 18:33"));
        assert_eq!(body, "the body");
    }

    #[test]
    fn attribution_last_at_splits_author_may_contain_at() {
        let (author, created, body, is_header) =
            parse_attribution("{jp@example.com @ 2026-06-01 18:33}: hi");
        assert!(is_header);
        assert_eq!(author.as_deref(), Some("jp@example.com"));
        assert_eq!(created, ts("2026-06-01 18:33"));
        assert_eq!(body, "hi");
    }

    #[test]
    fn attribution_author_only() {
        let (author, created, _, is_header) = parse_attribution("{JP}:");
        assert!(is_header);
        assert_eq!(author.as_deref(), Some("JP"));
        assert_eq!(created, None);
    }

    #[test]
    fn attribution_date_only_no_author() {
        let (author, created, _, is_header) = parse_attribution("{@ 2026-06-01}");
        assert!(is_header);
        assert_eq!(author, None);
        assert_eq!(created, ts("2026-06-01"));
    }

    #[test]
    fn attribution_empty_braces_is_header_no_attributes() {
        let (author, created, body, is_header) = parse_attribution("{}: body");
        assert!(is_header);
        assert_eq!(author, None);
        assert_eq!(created, None);
        assert_eq!(body, "body");
    }

    #[test]
    fn attribution_space_before_colon_makes_it_content() {
        let (author, _, body, is_header) = parse_attribution("{JP} : the body");
        assert!(is_header);
        assert_eq!(author.as_deref(), Some("JP"));
        assert_eq!(body, ": the body");
    }

    #[test]
    fn attribution_bare_emoji_is_header() {
        let (author, created, body, is_header) = parse_attribution("💬 hello");
        assert!(is_header);
        assert_eq!(author, None);
        assert_eq!(created, None);
        assert_eq!(body, "hello");
    }

    #[test]
    fn attribution_no_header_is_all_body() {
        let (author, created, body, is_header) =
            parse_attribution("A quoted comment, no attributes.");
        assert!(!is_header);
        assert_eq!(author, None);
        assert_eq!(created, None);
        assert_eq!(body, "A quoted comment, no attributes.");
    }

    #[test]
    fn timestamp_forms_and_date_only() {
        assert!(ts("2026-06-01 18:33").is_some());
        assert!(ts("2026-06-01 18:33:13").is_some());
        assert!(ts("2026-06-01").is_some()); // date-only is accepted
        assert!(ts("not a timestamp").is_none());
    }

    #[test]
    fn timestamp_rejects_out_of_range_fields() {
        assert!(ts("2026-13-01").is_none());
        assert!(ts("2026-06-32").is_none());
        assert!(ts("2026-06-01 24:00").is_none());
        assert!(ts("2026-06-01 18:60").is_none());
        assert!(ts("2026-06-01 18:33:60").is_none());
    }

    #[test]
    fn timestamp_rejects_malformed_input() {
        assert!(ts("2026-6-1").is_none()); // not zero-padded
        assert!(ts("2026-06-01  18:33").is_none()); // double space
        assert!(ts("2026-06-01 18:33 trailing").is_none());
    }

    // MARK: - Round trip

    fn round_trip(quotation: Option<&str>, messages: &[CommentMessage]) {
        let serialized = serialize(quotation, messages);
        let (q, m) = parse(&serialized);
        assert_eq!(q.as_deref(), quotation);
        assert_eq!(m, messages);
    }

    #[test]
    fn round_trip_general_unattributed() {
        round_trip(
            None,
            &[CommentMessage {
                author: None,
                created: None,
                body: "Just an observation.".to_string(),
            }],
        );
    }

    #[test]
    fn round_trip_quoted_unattributed() {
        round_trip(
            Some("fox"),
            &[CommentMessage {
                author: None,
                created: None,
                body: "A note.".to_string(),
            }],
        );
    }

    #[test]
    fn round_trip_attributed() {
        round_trip(
            Some("brown fox"),
            &[CommentMessage {
                author: Some("JP".to_string()),
                created: ts("2026-06-01 18:33:00"),
                body: "A comment.".to_string(),
            }],
        );
    }

    // A truncated quotation is just plain blockquote text; the spaced
    // ellipsis must survive serialize/parse untouched. Matching the
    // truncation is a render-time concern, not the codec's.
    #[test]
    fn round_trip_truncated_quotation() {
        round_trip(
            Some("Anchoring by verbatim echo … computed in JS, never stored."),
            &[CommentMessage {
                author: Some("JP".to_string()),
                created: ts("2026-06-22 20:52:00"),
                body: "A truncated quotation.".to_string(),
            }],
        );
    }

    #[test]
    fn round_trip_author_only() {
        round_trip(
            Some("fox"),
            &[CommentMessage {
                author: Some("JP".to_string()),
                created: None,
                body: "A note.".to_string(),
            }],
        );
    }

    #[test]
    fn round_trip_timestamp_only() {
        round_trip(
            None,
            &[CommentMessage {
                author: None,
                created: ts("2026-06-01 18:33:00"),
                body: "A note.".to_string(),
            }],
        );
    }

    #[test]
    fn round_trip_thread() {
        round_trip(
            Some("quick brown fox"),
            &[
                CommentMessage {
                    author: Some("JP".to_string()),
                    created: ts("2026-06-01 18:33:00"),
                    body: "First.".to_string(),
                },
                CommentMessage {
                    author: Some("Claude Opus 4.8".to_string()),
                    created: ts("2026-06-01 18:33:13"),
                    body: "Second.".to_string(),
                },
            ],
        );
    }

    #[test]
    fn round_trip_reply_with_blockquote_body() {
        round_trip(
            Some("quick brown fox"),
            &[
                CommentMessage {
                    author: Some("JP".to_string()),
                    created: ts("2026-06-01 18:33:00"),
                    body: "First.".to_string(),
                },
                CommentMessage {
                    author: Some("Claude Opus 4.8".to_string()),
                    created: ts("2026-06-01 18:33:13"),
                    body: "> First.\n\nSecond.".to_string(),
                },
            ],
        );
    }

    // A thread of consecutive unattributed messages must keep its
    // boundaries: each message after the first serializes with a bare
    // `💬`, or the two would merge back into one on re-parse.
    #[test]
    fn round_trip_unattributed_thread() {
        round_trip(
            Some("fox"),
            &[
                CommentMessage {
                    author: None,
                    created: None,
                    body: "First, unattributed.".to_string(),
                },
                CommentMessage {
                    author: None,
                    created: None,
                    body: "Reply, also unattributed.".to_string(),
                },
            ],
        );
    }

    // A reply added to an unattributed first message keeps both distinct.
    #[test]
    fn round_trip_unattributed_then_attributed_reply() {
        round_trip(
            None,
            &[
                CommentMessage {
                    author: None,
                    created: None,
                    body: "An open observation.".to_string(),
                },
                CommentMessage {
                    author: Some("JP".to_string()),
                    created: ts("2026-06-01 18:33:00"),
                    body: "A reply.".to_string(),
                },
            ],
        );
    }

    // MARK: - Byte identity

    // `parse` slices each message body verbatim out of the source instead of
    // re-serializing it through a Markdown formatter. So a message no one
    // has touched must survive a reply byte-for-byte -- including
    // formatting a formatter would have normalized (`_emphasis_` over
    // `*emphasis*`, the exact list marker, the blank-line spacing).
    #[test]
    fn reply_leaves_the_earlier_message_bytes_unchanged() {
        let fixture = "💬 {JP @ 2026-06-01 18:33:00}:\n\n\
             A body with:\n\n\
             * a bullet\n\
             * another bullet\n\n\
             and some _emphasis_.";
        let expected_body = "A body with:\n\n\
             * a bullet\n\
             * another bullet\n\n\
             and some _emphasis_.";

        let (quotation, mut messages) = parse(fixture);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].body, expected_body);

        // Add a reply and serialize the whole thread, as the editor would.
        messages.push(CommentMessage {
            author: Some("Claude".to_string()),
            created: ts("2026-06-01 18:40:00"),
            body: "A reply.".to_string(),
        });
        let serialized = serialize(quotation.as_deref(), &messages);

        // Re-parsing the rewritten thread returns the first body
        // byte-for-byte.
        let (_, reparsed) = parse(&serialized);
        assert_eq!(reparsed.len(), 2);
        assert_eq!(reparsed[0].body, expected_body);
        assert_eq!(reparsed[1].body, "A reply.");
    }
}

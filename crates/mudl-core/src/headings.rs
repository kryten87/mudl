//! Heading extraction for the outline sidebar: a flat list of every heading
//! in a document (level, slug `id`, and a simplified inline-content model),
//! independent of `render_up`'s HTML output but sharing its exact heading-ID
//! assignment behavior.
use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Tag};

use crate::frontmatter;
use crate::parse::ParsedMarkdown;
use crate::slug::Tracker;

/// A styled span within a heading's inline content. This is a simplified
/// inline model (not full HTML) — just enough for a sidebar to render
/// heading text with inline code spans styled distinctly. Every other
/// inline element (emphasis, strong, links, ...) has its text content
/// flattened into `Plain` segments; only inline code gets its own `Code`
/// segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutlineTextSegment {
    Plain(String),
    Code(String),
}

/// A single heading extracted from a Markdown document.
///
/// `id` is guaranteed to match the `id="..."` attribute `render_up` assigns
/// the same heading in its rendered HTML — see the parity test
/// `heading_ids_match_render_up_ids` below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineHeading {
    pub level: u8,
    pub id: String,
    pub segments: Vec<OutlineTextSegment>,
    /// The heading's 1-indexed line number in the source document — lets a
    /// sidebar (Phase 10.3) locate the corresponding `.line[data-line="N"]`
    /// element in Down mode's raw-source view, where there's no `id=`
    /// attribute to jump to the way Up mode has.
    pub line: usize,
}

/// Walks `markdown`'s `pulldown-cmark` event stream (the same parse
/// `render_up` builds from — see `crate::parse::parser_options`) and
/// collects every heading, in document order.
///
/// Each heading gets an `id` from one fresh `slug::Tracker`, tracked in the
/// same order and against the same derived text `render_up` slugs with: the
/// heading's inline content flattened to plain text, code-span content
/// included verbatim (no backticks — `pulldown-cmark`'s `Event::Code`
/// payload never carries its delimiters) and soft/hard breaks collapsed to
/// a single space. Because both walks visit `Start(Tag::Heading)` events in
/// the same document order and derive the same string from each one, the
/// two `Tracker`s (this function's, and `render_up`'s) stay in lockstep.
pub fn extract_headings(markdown: &str) -> Vec<OutlineHeading> {
    // A leading YAML frontmatter block is stripped before parsing — the
    // same frontmatter `render_up` (crate::render) pulls off and renders
    // as a key-value table instead of Markdown — so a bare `key:` line or
    // `# comment` inside it can't be misread as a heading. Headings found
    // in the (frontmatter-stripped) body have their line numbers shifted
    // back by the frontmatter's line count so they still index into the
    // *original* document, matching Down mode's unstripped line numbering.
    let (content, line_offset) = match frontmatter::extract(markdown) {
        Some(fm) => (fm.body, fm.line_count),
        None => (markdown.to_string(), 0),
    };

    let parsed = ParsedMarkdown::new(&content);
    let events = &parsed.events;
    let mut headings = Vec::new();
    let mut tracker = Tracker::new();
    let mut pos = 0;

    while pos < events.len() {
        match &events[pos].0 {
            Event::Start(Tag::Heading { level, .. }) => {
                let level = *level;
                let start_offset = events[pos].1.start;
                pos += 1; // consume Start(Heading)
                let (segments, next_pos) = collect_heading_segments(events, pos);
                pos = next_pos;

                let plain_text = flatten_segments(&segments);
                let id = tracker.track(&plain_text);
                headings.push(OutlineHeading {
                    level: heading_level_to_u8(level),
                    id,
                    segments,
                    line: line_number_at(&content, start_offset) + line_offset,
                });
            }
            _ => pos += 1,
        }
    }

    headings
}

/// The 1-indexed line number containing byte offset `pos`, counting `\n`s
/// before it — the same simple `str::lines()`-equivalent numbering
/// `render_down` uses for its `data-line` attributes, so the two agree.
fn line_number_at(markdown: &str, pos: usize) -> usize {
    markdown[..pos].matches('\n').count() + 1
}

fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Consumes events from `start` through the heading's matching
/// `End(Heading)` (exclusive on the way in, the returned index is just past
/// that `End`), producing one segment per `Text`/`Code`/soft-or-hard-break
/// leaf encountered, in document order.
///
/// Every other event — nested `Start`/`End` for emphasis, strong, links,
/// etc. — is transparent: it emits no segment of its own, but any leaf
/// descendants it wraps still contribute theirs, which is what flattens a
/// heading's inline formatting into a plain run of text/code segments. The
/// depth-tracked `Start`/`End` bookkeeping mirrors `render_up`'s own
/// `collect_plain_text`/`peek_plain_text` walk, so the two agree on exactly
/// which leaves belong to this heading.
fn collect_heading_segments(
    events: &[(Event<'_>, Range<usize>)],
    start: usize,
) -> (Vec<OutlineTextSegment>, usize) {
    let mut segments = Vec::new();
    let mut pos = start;
    let mut depth: i32 = 0;

    loop {
        match events.get(pos).map(|(e, _)| e) {
            Some(Event::Start(_)) => {
                depth += 1;
                pos += 1;
            }
            Some(Event::End(_)) => {
                pos += 1;
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Some(Event::Text(s)) => {
                segments.push(OutlineTextSegment::Plain(s.to_string()));
                pos += 1;
            }
            Some(Event::Code(s)) => {
                segments.push(OutlineTextSegment::Code(s.to_string()));
                pos += 1;
            }
            Some(Event::SoftBreak) | Some(Event::HardBreak) => {
                segments.push(OutlineTextSegment::Plain(" ".to_string()));
                pos += 1;
            }
            Some(_) => pos += 1,
            None => break,
        }
    }

    (segments, pos)
}

/// The plain-text form used for slugging: every segment's text
/// concatenated in document order, code spans included with no delimiters —
/// exactly what `render_up`'s `peek_plain_text` slugs headings with.
fn flatten_segments(segments: &[OutlineTextSegment]) -> String {
    let mut text = String::new();
    for segment in segments {
        match segment {
            OutlineTextSegment::Plain(s) | OutlineTextSegment::Code(s) => text.push_str(s),
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::RenderOptions;
    use crate::render::render_up;

    #[test]
    fn single_heading() {
        let headings = extract_headings("# Hello\n");
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[0].id, "hello");
        assert_eq!(
            headings[0].segments,
            vec![OutlineTextSegment::Plain("Hello".to_string())]
        );
    }

    #[test]
    fn multiple_headings_at_different_levels() {
        let headings = extract_headings("# One\n## Two\n### Three\n");
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[1].level, 2);
        assert_eq!(headings[2].level, 3);
        assert_eq!(headings[0].id, "one");
        assert_eq!(headings[1].id, "two");
        assert_eq!(headings[2].id, "three");
    }

    #[test]
    fn line_number_is_one_indexed_and_tracks_each_headings_own_line() {
        let headings = extract_headings("intro text\n\n# One\n\nbody\n\n## Two\n");
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].line, 3);
        assert_eq!(headings[1].line, 7);
    }

    #[test]
    fn line_number_of_a_heading_with_no_preceding_content_is_one() {
        let headings = extract_headings("# Hello\n");
        assert_eq!(headings[0].line, 1);
    }

    #[test]
    fn heading_with_inline_code() {
        let headings = extract_headings("## The `foo` method\n");
        assert_eq!(headings.len(), 1);
        assert_eq!(
            headings[0].segments,
            vec![
                OutlineTextSegment::Plain("The ".to_string()),
                OutlineTextSegment::Code("foo".to_string()),
                OutlineTextSegment::Plain(" method".to_string()),
            ]
        );
        // Slugging strips the backticks (and everything else that isn't a
        // word character/space/hyphen) regardless.
        assert_eq!(headings[0].id, "the-foo-method");
    }

    #[test]
    fn heading_with_emphasis_flattens_to_plain_segments() {
        let headings = extract_headings("## An *important* note\n");
        assert_eq!(headings.len(), 1);
        assert_eq!(
            headings[0].segments,
            vec![
                OutlineTextSegment::Plain("An ".to_string()),
                OutlineTextSegment::Plain("important".to_string()),
                OutlineTextSegment::Plain(" note".to_string()),
            ]
        );
        assert_eq!(headings[0].id, "an-important-note");
    }

    #[test]
    fn heading_with_link_flattens_to_plain_segments() {
        let headings = extract_headings("## See [this](url)\n");
        assert_eq!(headings.len(), 1);
        assert_eq!(
            headings[0].segments,
            vec![
                OutlineTextSegment::Plain("See ".to_string()),
                OutlineTextSegment::Plain("this".to_string()),
            ]
        );
        assert_eq!(headings[0].id, "see-this");
    }

    #[test]
    fn mixed_inline_segments_in_one_heading() {
        let headings = extract_headings("### Use `foo` for *bar* now\n");
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].level, 3);
        assert_eq!(
            headings[0].segments,
            vec![
                OutlineTextSegment::Plain("Use ".to_string()),
                OutlineTextSegment::Code("foo".to_string()),
                OutlineTextSegment::Plain(" for ".to_string()),
                OutlineTextSegment::Plain("bar".to_string()),
                OutlineTextSegment::Plain(" now".to_string()),
            ]
        );
        assert_eq!(headings[0].id, "use-foo-for-bar-now");
    }

    #[test]
    fn empty_document_has_no_headings() {
        assert!(extract_headings("").is_empty());
        assert!(extract_headings("No headings here.\n").is_empty());
    }

    #[test]
    fn duplicate_heading_text_gets_deduplicated_slugs() {
        let headings = extract_headings("# Title\n## Title\n## Title\n");
        let ids: Vec<&str> = headings.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, vec!["title", "title-1", "title-2"]);
        let levels: Vec<u8> = headings.iter().map(|h| h.level).collect();
        assert_eq!(levels, vec![1, 2, 2]);
    }

    #[test]
    fn soft_line_break_in_heading_becomes_a_space_segment() {
        // A two-line setext heading carries a soft break between its lines.
        let headings = extract_headings("One\nTwo\n===\n");
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].level, 1);
        assert_eq!(
            headings[0].segments,
            vec![
                OutlineTextSegment::Plain("One".to_string()),
                OutlineTextSegment::Plain(" ".to_string()),
                OutlineTextSegment::Plain("Two".to_string()),
            ]
        );
        assert_eq!(headings[0].id, "one-two");
    }

    /// Ported from `mud`'s `HeadingExtractorTests` (the test enforcing the
    /// parity `HeadingExtractor`/`SlugGenerator` guarantee between the
    /// sidebar's heading IDs and `UpHTMLVisitor`'s `id=` attributes): the
    /// slugs `extract_headings` assigns must match `render_up`'s `id=`
    /// attributes exactly, in the same order — including through duplicate
    /// heading text (the `-1`/`-2` suffix path) and a heading containing
    /// inline code.
    #[test]
    fn heading_ids_match_render_up_ids() {
        let markdown = "\
# Getting Started

## Installation

## The `foo` API

### Details

## The `foo` API

## The `foo` API
";

        let headings = extract_headings(markdown);
        let html = render_up(markdown, &RenderOptions::default());

        let extracted_ids: Vec<String> = headings.iter().map(|h| h.id.clone()).collect();
        let rendered_ids = extract_id_attributes(&html);

        assert_eq!(extracted_ids, rendered_ids);
        // Exercise both the duplicate-suffix path and an inline-code heading.
        assert!(extracted_ids.contains(&"the-foo-api".to_string()));
        assert!(extracted_ids.contains(&"the-foo-api-1".to_string()));
        assert!(extracted_ids.contains(&"the-foo-api-2".to_string()));
    }

    #[test]
    fn frontmatter_comment_and_bare_key_are_not_treated_as_headings() {
        let markdown = "---\ntitle: Hello\n# a comment\nempty_value:\n---\n\n# Real Heading\n";
        let headings = extract_headings(markdown);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].id, "real-heading");
    }

    #[test]
    fn heading_line_number_accounts_for_stripped_frontmatter() {
        // line_count is 4 (the two `---` delimiters plus the two YAML
        // lines between them); the heading sits two lines after that.
        let markdown = "---\ntitle: Hello\nauthor: Jane\n---\n\n# Heading\n";
        let headings = extract_headings(markdown);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].line, 6);
    }

    #[test]
    fn frontmatter_heading_ids_and_lines_match_render_up() {
        let markdown = "---\ntitle: Hello\n# a comment\nempty_value:\n---\n\n# Heading\n";
        let headings = extract_headings(markdown);
        let html = render_up(markdown, &RenderOptions::default());
        let rendered_ids = extract_id_attributes(&html);
        assert_eq!(
            headings.iter().map(|h| h.id.clone()).collect::<Vec<_>>(),
            rendered_ids
        );
    }

    /// A simple string scan for every `id="..."` attribute value in
    /// `html`, in the order they appear — sufficient for `render_up`'s
    /// output, where only headings ever carry an `id=` attribute.
    fn extract_id_attributes(html: &str) -> Vec<String> {
        let mut ids = Vec::new();
        let mut rest = html;
        while let Some(idx) = rest.find("id=\"") {
            let after = &rest[idx + 4..];
            let Some(end) = after.find('"') else {
                break;
            };
            ids.push(after[..end].to_string());
            rest = &after[end + 1..];
        }
        ids
    }
}

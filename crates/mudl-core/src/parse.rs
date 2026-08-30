use pulldown_cmark::{Event, Options, Parser};
use std::ops::Range;

/// The `Options` bitset every `mudl-core` parse uses. Tables, strikethrough,
/// task lists, and footnotes are the GFM/CommonMark extensions the renderer
/// supports (footnotes enabled as of Phase 14 — see `render::Renderer`'s
/// `Tag::FootnoteDefinition`/`Event::FootnoteReference` handling, and
/// `mudl_comments` for the comment layer built on top of the same syntax).
/// `Options::ENABLE_GFM` is deliberately left off — it would turn on
/// pulldown-cmark's own `[!NOTE]`-style blockquote-kind detection, but
/// `render_up` does that detection itself (via `crate::alerts`) so it can
/// share logic with the DocC aside path and gate on
/// `RenderOptions::doc_c_alert_mode`.
pub fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
}

/// One parse of a Markdown document: the full `pulldown-cmark` event stream,
/// each event paired with its byte range in the source. Materialized as a
/// `Vec` (not left as a lazy iterator) so a renderer can look ahead —
/// e.g. to inspect a blockquote's first paragraph before deciding whether
/// it opens a GFM/DocC alert, without consuming the events it peeked at.
pub struct ParsedMarkdown<'a> {
    pub events: Vec<(Event<'a>, Range<usize>)>,
}

impl<'a> ParsedMarkdown<'a> {
    pub fn new(markdown: &'a str) -> Self {
        let parser = Parser::new_ext(markdown, parser_options());
        Self {
            events: parser.into_offset_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_produces_at_least_one_event() {
        let parsed = ParsedMarkdown::new("# Hello");
        assert!(!parsed.events.is_empty());
    }
}

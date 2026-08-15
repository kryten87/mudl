//! Up-mode HTML rendering: a visitor over the `pulldown-cmark` event stream.
//!
//! `pulldown-cmark` yields a *flat* stream of `Start(tag)` / `End(tag)` /
//! leaf events (start and end events are always balanced — see the crate's
//! own docs on `Event`). `Renderer` walks that stream with an explicit
//! index (`pos`) rather than recursing over a tree, using one invariant
//! throughout: whenever a method is entered right after a `Start` event has
//! been consumed, calling `children()` renders (and consumes) everything up
//! to and including that `Start`'s own matching `End` — because every
//! *nested* `Start` it encounters along the way is itself fully consumed,
//! matching `End` included, by the recursive call before `children()`'s
//! loop sees another event. That's what lets a handful of small methods
//! stand in for a full AST visitor without building one.
use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, LinkType, Tag, TagEnd};

use crate::alerts::{detect_docc_aside, detect_gfm_alert, parse_aside_tag, AlertCategory};
use crate::emoji::replace_shortcodes;
use crate::encoding::html_escape;
use crate::options::RenderOptions;
use crate::parse::ParsedMarkdown;
use crate::slug::Tracker;

/// Renders `markdown` to Up-mode HTML body content (no surrounding
/// `<html>`/`<head>` document — that's Phase 3's `HtmlDocument` template).
pub fn render_up(markdown: &str, options: &RenderOptions) -> String {
    let parsed = ParsedMarkdown::new(markdown);
    let mut renderer = Renderer {
        events: &parsed.events,
        pos: 0,
        out: String::new(),
        slugs: Tracker::new(),
        options,
        table_alignments: Vec::new(),
        in_table_head: false,
        cell_index: 0,
    };
    renderer.run();
    renderer.out
}

// ---------------------------------------------------------------------
// Down mode: raw-source view
// ---------------------------------------------------------------------

/// Renders `markdown` to Down-mode HTML: the raw-source view, one
/// `<div class="line" data-line="N">` per source line (1-indexed), each
/// line HTML-escaped.
///
/// Deliberately trivial — a `str::lines()` split, not a `pulldown-cmark`
/// walk. Down mode has no need for an event-stream visitor the way
/// `render_up` does: syntax highlighting is applied client-side (see the
/// implementation plan's §4), so there's no server-side span-tracking to
/// do here, and Rust string slicing never lets a highlighted span cross
/// a newline the way `mud`'s `HTMLLineSplitter` had to account for.
/// Visual line numbering is CSS-driven from the `data-line` attribute,
/// matching `mud`'s approach — this function only emits the attribute,
/// not rendered number text.
///
/// `options` is accepted for API symmetry with `render_up` (and in case
/// a later phase needs it) but is currently unused by Down mode.
pub fn render_down(markdown: &str, _options: &RenderOptions) -> String {
    let mut out = String::new();
    for (index, line) in markdown.lines().enumerate() {
        out.push_str("<div class=\"line\" data-line=\"");
        out.push_str(&(index + 1).to_string());
        out.push_str("\">");
        out.push_str(&html_escape(line));
        out.push_str("</div>");
    }
    out
}

struct Renderer<'a> {
    events: &'a [(Event<'a>, Range<usize>)],
    pos: usize,
    out: String,
    slugs: Tracker,
    options: &'a RenderOptions,
    table_alignments: Vec<Alignment>,
    in_table_head: bool,
    cell_index: usize,
}

impl<'a> Renderer<'a> {
    fn current(&self) -> Option<&Event<'a>> {
        self.events.get(self.pos).map(|(e, _)| e)
    }

    fn run(&mut self) {
        while self.pos < self.events.len() {
            self.step();
        }
    }

    /// Processes exactly one event at `self.pos`: a leaf event, or (for a
    /// `Start`) an entire balanced subtree, advancing `self.pos` past
    /// whatever it consumed.
    fn step(&mut self) {
        let event = self.events[self.pos].0.clone();
        match event {
            Event::Start(tag) => {
                self.pos += 1;
                self.render_container(tag);
            }
            // A bare `End` should never reach `step` in a well-formed walk
            // (every `Start` this walker consumes is paired with its `End`
            // by `children`/the container renderers below). Consume it
            // defensively rather than looping forever.
            Event::End(_) => {
                self.pos += 1;
            }
            leaf => {
                self.pos += 1;
                self.render_leaf(leaf);
            }
        }
    }

    /// Renders every event up to, and consuming, the next bare `End` at
    /// this nesting level — i.e. the matching close of whatever `Start`
    /// the caller already consumed. See the module doc comment for why
    /// this is always correct regardless of which tag it's closing.
    fn children(&mut self) {
        loop {
            match self.current() {
                Some(Event::End(_)) => {
                    self.pos += 1;
                    return;
                }
                Some(_) => self.step(),
                None => return,
            }
        }
    }

    /// Like `children`, but collects plain text (ignoring markup/wrapper
    /// tags) instead of rendering HTML — used for image alt text and
    /// heading slugs. Consumes through the matching `End`.
    fn collect_plain_text(&mut self) -> String {
        let mut text = String::new();
        let mut depth: i32 = 0;
        loop {
            match self.events.get(self.pos).map(|(e, _)| e) {
                Some(Event::Start(_)) => {
                    depth += 1;
                    self.pos += 1;
                }
                Some(Event::End(_)) => {
                    self.pos += 1;
                    if depth == 0 {
                        return text;
                    }
                    depth -= 1;
                }
                Some(Event::Text(s)) | Some(Event::Code(s)) => {
                    text.push_str(s);
                    self.pos += 1;
                }
                Some(Event::SoftBreak) | Some(Event::HardBreak) => {
                    text.push(' ');
                    self.pos += 1;
                }
                Some(_) => {
                    self.pos += 1;
                }
                None => return text,
            }
        }
    }

    /// Non-mutating look-ahead: the plain text `collect_plain_text` would
    /// return if called at `start`, without moving `self.pos`. Used so a
    /// heading can compute its slug before rendering its (still fully
    /// formatted) visible content from the same events.
    fn peek_plain_text(&self, start: usize) -> String {
        let mut idx = start;
        let mut text = String::new();
        let mut depth: i32 = 0;
        loop {
            match self.events.get(idx).map(|(e, _)| e) {
                Some(Event::Start(_)) => {
                    depth += 1;
                    idx += 1;
                }
                Some(Event::End(_)) => {
                    idx += 1;
                    if depth == 0 {
                        return text;
                    }
                    depth -= 1;
                }
                Some(Event::Text(s)) | Some(Event::Code(s)) => {
                    text.push_str(s);
                    idx += 1;
                }
                Some(Event::SoftBreak) | Some(Event::HardBreak) => {
                    text.push(' ');
                    idx += 1;
                }
                Some(_) => idx += 1,
                None => return text,
            }
        }
    }

    /// Concatenates consecutive `Text` events starting at `start`, without
    /// mutating `self.pos`, stopping at the first non-`Text` event. Returns
    /// the concatenated string and the index to resume consumption at.
    ///
    /// This is "the literal opening run of a paragraph's first line" —
    /// see the comment in `render_blockquote` for why a GFM/DocC alert tag
    /// can span more than one `Text` event.
    fn peek_leading_text_run(&self, start: usize) -> (String, usize) {
        let mut idx = start;
        let mut text = String::new();
        while let Some(Event::Text(s)) = self.events.get(idx).map(|(e, _)| e) {
            text.push_str(s);
            idx += 1;
        }
        (text, idx)
    }

    fn render_container(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => {
                self.out.push_str("<p>");
                self.children();
                self.out.push_str("</p>\n");
            }
            Tag::Heading { level, .. } => {
                let plain = self.peek_plain_text(self.pos);
                let slug = self.slugs.track(&plain);
                self.out
                    .push_str(&format!("<{level} id=\"{}\">", html_escape(&slug)));
                self.children();
                self.out.push_str(&format!("</{level}>\n"));
            }
            Tag::BlockQuote(_) => self.render_blockquote(),
            Tag::CodeBlock(kind) => self.render_code_block(kind),
            Tag::HtmlBlock => self.render_html_block(),
            Tag::List(start) => self.render_list(start),
            Tag::Item => {
                self.out.push_str("<li>");
                self.children();
                self.out.push_str("</li>\n");
            }
            Tag::Table(alignments) => self.render_table(alignments),
            Tag::TableHead => self.render_table_head(),
            Tag::TableRow => self.render_table_row(),
            Tag::TableCell => self.render_table_cell(),
            Tag::Emphasis => {
                self.out.push_str("<em>");
                self.children();
                self.out.push_str("</em>");
            }
            Tag::Strong => {
                self.out.push_str("<strong>");
                self.children();
                self.out.push_str("</strong>");
            }
            Tag::Strikethrough => {
                self.out.push_str("<s>");
                self.children();
                self.out.push_str("</s>");
            }
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => self.render_link(link_type, &dest_url, &title),
            Tag::Image {
                dest_url, title, ..
            } => self.render_image(&dest_url, &title),
            // None of these are ever produced by the `Options` bitset
            // `parser_options` turns on (footnotes are deferred to Phase
            // 14; definition lists, sub/superscript, heading attributes,
            // and metadata blocks are never enabled at all) — kept so the
            // match stays exhaustive if that ever changes, rendering inner
            // content with no wrapper rather than panicking.
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript
            | Tag::MetadataBlock(_) => {
                self.children();
            }
        }
    }

    fn render_leaf(&mut self, event: Event<'a>) {
        match event {
            Event::Text(s) => {
                self.out.push_str(&html_escape(&replace_shortcodes(&s)));
            }
            Event::Code(s) => {
                self.out.push_str("<code>");
                self.out.push_str(&html_escape(&s));
                self.out.push_str("</code>");
            }
            Event::InlineHtml(s) => {
                self.out.push_str(&s);
            }
            Event::SoftBreak => self.out.push('\n'),
            Event::HardBreak => self.out.push_str("<br />\n"),
            Event::Rule => self.out.push_str("<hr />\n"),
            Event::TaskListMarker(checked) => {
                if checked {
                    self.out
                        .push_str("<input type=\"checkbox\" checked=\"\" disabled=\"\" /> ");
                } else {
                    self.out
                        .push_str("<input type=\"checkbox\" disabled=\"\" /> ");
                }
            }
            // Math and footnotes are not enabled by `parser_options`; these
            // variants cannot be produced by the parser today. No-op
            // rather than making the match non-exhaustive.
            Event::InlineMath(_) | Event::DisplayMath(_) | Event::FootnoteReference(_) => {}
            // Only ever produced as a child of `Tag::HtmlBlock`, which
            // `render_html_block` consumes directly rather than going
            // through `step`/`render_leaf` — unreachable here in practice.
            Event::Html(_) => {}
            Event::Start(_) | Event::End(_) => {
                unreachable!("Start/End are handled by step(), never render_leaf")
            }
        }
    }

    // MARK: - Blockquotes and alerts

    fn render_blockquote(&mut self) {
        // Peek: does this blockquote open with a paragraph whose first
        // line is (at least in part) plain text? That's the only shape GFM
        // alerts (`[!NOTE]`) and DocC asides (`Note:`) can open with —
        // mirrors `mud`'s `AlertDetector`, which requires
        // `blockQuote.firstChild.kind == .paragraph` and (for DocC)
        // `paragraph.firstChild.kind == .text`.
        //
        // A GFM tag's brackets force pulldown-cmark's link-scanning
        // machinery to revert `[`/`!TAG`/`]` to three *separate* adjacent
        // `Text` events rather than one merged node (it tried reading them
        // as a link, failed to resolve one, and fell back to literal
        // text token-by-token) — so detection and stripping both work on
        // the whole leading run of `Text` events, not just the first one.
        let first_line = match self.current() {
            Some(Event::Start(Tag::Paragraph)) => {
                let (text, _) = self.peek_leading_text_run(self.pos + 1);
                Some(text)
            }
            _ => None,
        };

        if let Some(first_line) = &first_line {
            if let Some((category, ())) = detect_gfm_alert(first_line) {
                self.render_gfm_alert(category);
                return;
            }
            if let Some((tag, byte_len)) = parse_aside_tag(first_line) {
                if let Some(category) = detect_docc_aside(&tag, self.options.doc_c_alert_mode) {
                    self.render_docc_alert(category, &tag, byte_len);
                    return;
                }
            }
        }

        self.out.push_str("<blockquote>\n");
        self.children();
        self.out.push_str("</blockquote>\n");
    }

    fn render_gfm_alert(&mut self, category: AlertCategory) {
        let strip_len = gfm_tag_literal(category).len();
        self.render_alert_body(alert_class(category), alert_title(category), strip_len);
    }

    fn render_docc_alert(&mut self, category: AlertCategory, tag: &str, byte_len: usize) {
        let title = docc_display_name(tag);
        self.render_alert_body(alert_class(category), &title, byte_len);
    }

    /// Shared alert body renderer. `self.pos` must be at the blockquote's
    /// `Start(Paragraph)` with a leading `Text` child — verified by
    /// `render_blockquote`'s peek. Strips `strip_len` bytes (the detected
    /// tag, colon, and trailing whitespace) off that first text node, then
    /// renders the rest of the paragraph and the remaining blockquote
    /// content normally.
    fn render_alert_body(&mut self, class: &str, title: &str, strip_len: usize) {
        self.out
            .push_str(&format!("<blockquote class=\"alert {class}\">\n"));
        self.out.push_str(&format!(
            "<p class=\"alert-title\">{}</p>\n",
            html_escape(title)
        ));

        self.pos += 1; // consume Start(Paragraph)
        let (first_text, after_leading_text) = self.peek_leading_text_run(self.pos);
        self.pos = after_leading_text; // consume the tag's leading Text run

        let remainder = first_text
            .get(strip_len.min(first_text.len())..)
            .unwrap_or("")
            .trim_start_matches(' ');

        let mut opened = false;
        if !remainder.is_empty() {
            opened = true;
            self.out.push_str("<p>");
            self.out
                .push_str(&html_escape(&replace_shortcodes(remainder)));
        }
        // Skip the soft break separating the tag line from the rest of the
        // paragraph's content, if there is one.
        if matches!(self.current(), Some(Event::SoftBreak)) {
            self.pos += 1;
        }
        let at_paragraph_end = matches!(self.current(), Some(Event::End(TagEnd::Paragraph)));
        if at_paragraph_end {
            self.pos += 1; // consume End(Paragraph); nothing more on this line
        } else {
            if !opened {
                self.out.push_str("<p>");
                opened = true;
            }
            self.children(); // rest of the paragraph; consumes its End
        }
        if opened {
            self.out.push_str("</p>\n");
        }

        self.children(); // remaining blockquote blocks; consumes End(BlockQuote)
        self.out.push_str("</blockquote>\n");
    }

    // MARK: - Code blocks and HTML blocks

    fn render_code_block(&mut self, kind: CodeBlockKind<'a>) {
        let lang = match &kind {
            CodeBlockKind::Fenced(info) => {
                let first_word = info.split(' ').next().unwrap_or("");
                if first_word.is_empty() {
                    None
                } else {
                    Some(first_word.to_string())
                }
            }
            CodeBlockKind::Indented => None,
        };

        let mut code = String::new();
        loop {
            match self.current() {
                Some(Event::Text(s)) => {
                    code.push_str(s);
                    self.pos += 1;
                }
                Some(Event::End(TagEnd::CodeBlock)) => {
                    self.pos += 1;
                    break;
                }
                Some(_) => self.pos += 1,
                None => break,
            }
        }

        match lang {
            Some(lang) => {
                self.out.push_str("<pre><code class=\"language-");
                self.out.push_str(&html_escape(&lang));
                self.out.push_str("\">");
            }
            None => self.out.push_str("<pre><code>"),
        }
        self.out.push_str(&html_escape(&code));
        self.out.push_str("</code></pre>\n");
    }

    fn render_html_block(&mut self) {
        loop {
            match self.current() {
                Some(Event::Html(s)) => {
                    let s = s.to_string();
                    self.out.push_str(&s);
                    self.pos += 1;
                }
                Some(Event::End(TagEnd::HtmlBlock)) => {
                    self.pos += 1;
                    break;
                }
                Some(_) => self.pos += 1,
                None => break,
            }
        }
    }

    // MARK: - Lists

    fn render_list(&mut self, start: Option<u64>) {
        match start {
            None => self.out.push_str("<ul>\n"),
            Some(1) => self.out.push_str("<ol>\n"),
            Some(n) => self.out.push_str(&format!("<ol start=\"{n}\">\n")),
        }
        self.children();
        match start {
            None => self.out.push_str("</ul>\n"),
            Some(_) => self.out.push_str("</ol>\n"),
        }
    }

    // MARK: - Tables

    fn render_table(&mut self, alignments: Vec<Alignment>) {
        self.table_alignments = alignments;
        self.out.push_str("<table>\n");

        // A GFM table always has exactly one header row.
        if matches!(self.current(), Some(Event::Start(Tag::TableHead))) {
            self.step();
        }

        if matches!(self.current(), Some(Event::Start(Tag::TableRow))) {
            self.out.push_str("<tbody>\n");
            while matches!(self.current(), Some(Event::Start(Tag::TableRow))) {
                self.step();
            }
            self.out.push_str("</tbody>\n");
        }

        if matches!(self.current(), Some(Event::End(TagEnd::Table))) {
            self.pos += 1;
        }
        self.out.push_str("</table>\n");
        self.table_alignments.clear();
    }

    fn render_table_head(&mut self) {
        self.in_table_head = true;
        self.cell_index = 0;
        self.out.push_str("<thead>\n<tr>\n");
        self.children();
        self.out.push_str("</tr>\n</thead>\n");
        self.in_table_head = false;
    }

    fn render_table_row(&mut self) {
        self.cell_index = 0;
        self.out.push_str("<tr>\n");
        self.children();
        self.out.push_str("</tr>\n");
    }

    fn render_table_cell(&mut self) {
        let tag = if self.in_table_head { "th" } else { "td" };
        let align = self
            .table_alignments
            .get(self.cell_index)
            .copied()
            .unwrap_or(Alignment::None);
        let class = match align {
            Alignment::None => "",
            Alignment::Left => " class=\"align-left\"",
            Alignment::Center => " class=\"align-center\"",
            Alignment::Right => " class=\"align-right\"",
        };
        self.out.push_str(&format!("<{tag}{class}>"));
        self.children();
        self.out.push_str(&format!("</{tag}>\n"));
        self.cell_index += 1;
    }

    // MARK: - Links and images

    fn render_link(&mut self, link_type: LinkType, dest_url: &str, title: &str) {
        let href = if link_type == LinkType::Email {
            format!("mailto:{dest_url}")
        } else {
            dest_url.to_string()
        };
        self.out.push_str("<a href=\"");
        self.out.push_str(&html_escape(&href));
        self.out.push('"');
        if !title.is_empty() {
            self.out.push_str(" title=\"");
            self.out.push_str(&html_escape(title));
            self.out.push('"');
        }
        self.out.push('>');
        self.children();
        self.out.push_str("</a>");
    }

    fn render_image(&mut self, dest_url: &str, title: &str) {
        // Alt text is the image's inline children flattened to plain text
        // (no emoji-shortcode replacement, matching `mud`'s `image.plainText`
        // used verbatim for the `alt` attribute).
        let alt = self.collect_plain_text();
        self.out.push_str("<img src=\"");
        self.out.push_str(&html_escape(dest_url));
        self.out.push_str("\" alt=\"");
        self.out.push_str(&html_escape(&alt));
        self.out.push('"');
        if !title.is_empty() {
            self.out.push_str(" title=\"");
            self.out.push_str(&html_escape(title));
            self.out.push('"');
        }
        self.out.push_str(" />");
    }
}

// MARK: - Alert presentation tables
//
// These mirror `mud`'s `AlertDetector`/`UpHTMLVisitor`, reconstructed at the
// render site rather than threaded through `crate::alerts` — exactly how
// the Swift reference does it too (`UpHTMLVisitor.emitGFMAlertContent`
// builds `"[!\(category.rawValue.uppercased())]"` locally rather than
// asking the detector for it).

fn alert_class(category: AlertCategory) -> &'static str {
    match category {
        AlertCategory::Note => "alert-note",
        AlertCategory::Tip => "alert-tip",
        AlertCategory::Important => "alert-important",
        AlertCategory::Warning => "alert-warning",
        AlertCategory::Caution => "alert-caution",
        AlertCategory::Status => "alert-status",
    }
}

fn alert_title(category: AlertCategory) -> &'static str {
    match category {
        AlertCategory::Note => "Note",
        AlertCategory::Tip => "Tip",
        AlertCategory::Important => "Important",
        AlertCategory::Warning => "Warning",
        AlertCategory::Caution => "Caution",
        AlertCategory::Status => "Status",
    }
}

fn gfm_tag_literal(category: AlertCategory) -> &'static str {
    match category {
        AlertCategory::Note => "[!NOTE]",
        AlertCategory::Tip => "[!TIP]",
        AlertCategory::Important => "[!IMPORTANT]",
        AlertCategory::Warning => "[!WARNING]",
        AlertCategory::Caution => "[!CAUTION]",
        AlertCategory::Status => "[!STATUS]",
    }
}

/// Mirrors `AlertDetector.docCDisplayName(for:)`: most DocC tags display
/// as-is, a handful get a friendlier spelling.
fn docc_display_name(tag: &str) -> String {
    match tag {
        "SeeAlso" => "See Also".to_string(),
        "NonMutatingVariant" => "Non-Mutating Variant".to_string(),
        "MutatingVariant" => "Mutating Variant".to_string(),
        "ToDo" => "To Do".to_string(),
        _ => tag.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(markdown: &str) -> String {
        render_up(markdown, &RenderOptions::default())
    }

    // MARK: Paragraphs

    mod paragraph_tests {
        use super::render;

        #[test]
        fn single_paragraph() {
            assert_eq!(render("Hello world"), "<p>Hello world</p>\n");
        }

        #[test]
        fn two_paragraphs() {
            assert_eq!(
                render("First.\n\nSecond."),
                "<p>First.</p>\n<p>Second.</p>\n"
            );
        }

        #[test]
        fn escapes_special_characters() {
            // `<` followed by a space (rather than a letter/`/`/`!`/`?`)
            // doesn't open CommonMark raw HTML, so this stays plain text.
            assert_eq!(
                render("Tom & Jerry: 1 < 2 and 2 > 1"),
                "<p>Tom &amp; Jerry: 1 &lt; 2 and 2 &gt; 1</p>\n"
            );
        }
    }

    // MARK: Emphasis / strong

    mod emphasis_tests {
        use super::render;

        #[test]
        fn emphasis() {
            assert_eq!(render("*em*"), "<p><em>em</em></p>\n");
        }

        #[test]
        fn strong() {
            assert_eq!(render("**strong**"), "<p><strong>strong</strong></p>\n");
        }

        #[test]
        fn nested_strong_in_emphasis() {
            assert_eq!(
                render("*em **strong** em*"),
                "<p><em>em <strong>strong</strong> em</em></p>\n"
            );
        }
    }

    // MARK: Inline code

    mod inline_code_tests {
        use super::render;

        #[test]
        fn inline_code() {
            assert_eq!(render("`code`"), "<p><code>code</code></p>\n");
        }

        #[test]
        fn inline_code_escapes_content() {
            assert_eq!(render("`<a>&b`"), "<p><code>&lt;a&gt;&amp;b</code></p>\n");
        }
    }

    // MARK: Links / images

    mod link_and_image_tests {
        use super::render;

        #[test]
        fn inline_link() {
            assert_eq!(
                render("[text](https://example.com)"),
                "<p><a href=\"https://example.com\">text</a></p>\n"
            );
        }

        #[test]
        fn link_with_title() {
            assert_eq!(
                render("[text](https://example.com \"Title\")"),
                "<p><a href=\"https://example.com\" title=\"Title\">text</a></p>\n"
            );
        }

        #[test]
        fn image() {
            assert_eq!(
                render("![alt text](img.png)"),
                "<p><img src=\"img.png\" alt=\"alt text\" /></p>\n"
            );
        }

        #[test]
        fn image_with_title() {
            assert_eq!(
                render("![alt](img.png \"caption\")"),
                "<p><img src=\"img.png\" alt=\"alt\" title=\"caption\" /></p>\n"
            );
        }
    }

    // MARK: Headings

    mod heading_tests {
        use super::render;

        #[test]
        fn h1_gets_slug_id() {
            assert_eq!(
                render("# Hello World"),
                "<h1 id=\"hello-world\">Hello World</h1>\n"
            );
        }

        #[test]
        fn h2_through_h6() {
            assert_eq!(render("###### Deep"), "<h6 id=\"deep\">Deep</h6>\n");
        }

        #[test]
        fn duplicate_headings_get_suffixed_slugs() {
            assert_eq!(
                render("# Intro\n\n# Intro"),
                "<h1 id=\"intro\">Intro</h1>\n<h1 id=\"intro-1\">Intro</h1>\n"
            );
        }

        #[test]
        fn heading_with_inline_formatting_keeps_formatting_but_slugs_plain_text() {
            assert_eq!(
                render("# Hello *World*"),
                "<h1 id=\"hello-world\">Hello <em>World</em></h1>\n"
            );
        }
    }

    // MARK: Lists (tight vs loose)

    mod list_tests {
        use super::render;

        #[test]
        fn tight_unordered_list() {
            assert_eq!(
                render("- one\n- two\n"),
                "<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n"
            );
        }

        #[test]
        fn loose_unordered_list_wraps_items_in_paragraphs() {
            assert_eq!(
                render("- one\n\n- two\n"),
                "<ul>\n<li><p>one</p>\n</li>\n<li><p>two</p>\n</li>\n</ul>\n"
            );
        }

        #[test]
        fn ordered_list_default_start() {
            assert_eq!(
                render("1. one\n2. two\n"),
                "<ol>\n<li>one</li>\n<li>two</li>\n</ol>\n"
            );
        }

        #[test]
        fn ordered_list_custom_start() {
            assert_eq!(
                render("5. five\n6. six\n"),
                "<ol start=\"5\">\n<li>five</li>\n<li>six</li>\n</ol>\n"
            );
        }

        #[test]
        fn nested_list() {
            assert_eq!(
                render("- one\n  - nested\n"),
                "<ul>\n<li>one<ul>\n<li>nested</li>\n</ul>\n</li>\n</ul>\n"
            );
        }
    }

    // MARK: Blockquotes (plain, then alerts)

    mod blockquote_tests {
        use super::render;

        #[test]
        fn plain_blockquote() {
            assert_eq!(
                render("> Just a quote."),
                "<blockquote>\n<p>Just a quote.</p>\n</blockquote>\n"
            );
        }

        #[test]
        fn nested_blockquote() {
            assert_eq!(
                render("> Outer\n>\n> > Inner"),
                "<blockquote>\n<p>Outer</p>\n<blockquote>\n<p>Inner</p>\n</blockquote>\n</blockquote>\n"
            );
        }
    }

    mod gfm_alert_tests {
        use super::render;

        #[test]
        fn note_alert_with_body_on_next_line() {
            assert_eq!(
                render("> [!NOTE]\n> Body text."),
                "<blockquote class=\"alert alert-note\">\n\
                 <p class=\"alert-title\">Note</p>\n\
                 <p>Body text.</p>\n\
                 </blockquote>\n"
            );
        }

        #[test]
        fn warning_alert_with_trailing_blocks() {
            assert_eq!(
                render("> [!WARNING]\n> Careful.\n>\n> More detail."),
                "<blockquote class=\"alert alert-warning\">\n\
                 <p class=\"alert-title\">Warning</p>\n\
                 <p>Careful.</p>\n\
                 <p>More detail.</p>\n\
                 </blockquote>\n"
            );
        }

        #[test]
        fn tag_only_alert_has_no_content_paragraph() {
            assert_eq!(
                render("> [!NOTE]"),
                "<blockquote class=\"alert alert-note\">\n\
                 <p class=\"alert-title\">Note</p>\n\
                 </blockquote>\n"
            );
        }

        #[test]
        fn unrecognized_tag_renders_as_plain_blockquote() {
            assert_eq!(
                render("> [!BOGUS]\n> Body."),
                "<blockquote>\n<p>[!BOGUS]\nBody.</p>\n</blockquote>\n"
            );
        }
    }

    mod docc_alert_tests {
        use super::render;
        use crate::alerts::DocCAlertMode;
        use crate::options::RenderOptions;
        use crate::render::render_up;

        #[test]
        fn note_aside_default_mode_is_extended() {
            assert_eq!(
                render("> Note: Body text."),
                "<blockquote class=\"alert alert-note\">\n\
                 <p class=\"alert-title\">Note</p>\n\
                 <p>Body text.</p>\n\
                 </blockquote>\n"
            );
        }

        #[test]
        fn extended_alias_gets_friendly_title() {
            assert_eq!(
                render("> SeeAlso: Related things."),
                "<blockquote class=\"alert alert-note\">\n\
                 <p class=\"alert-title\">See Also</p>\n\
                 <p>Related things.</p>\n\
                 </blockquote>\n"
            );
        }

        #[test]
        fn off_mode_renders_plain_blockquote() {
            let options = RenderOptions {
                doc_c_alert_mode: DocCAlertMode::Off,
            };
            assert_eq!(
                render_up("> Note: Body text.", &options),
                "<blockquote>\n<p>Note: Body text.</p>\n</blockquote>\n"
            );
        }

        #[test]
        fn common_mode_excludes_extended_aliases() {
            let options = RenderOptions {
                doc_c_alert_mode: DocCAlertMode::Common,
            };
            assert_eq!(
                render_up("> Bug: Something broke.", &options),
                "<blockquote>\n<p>Bug: Something broke.</p>\n</blockquote>\n"
            );
        }
    }

    // MARK: Code fences

    mod code_block_tests {
        use super::render;

        #[test]
        fn fenced_code_with_language() {
            assert_eq!(
                render("```rust\nfn main() {}\n```"),
                "<pre><code class=\"language-rust\">fn main() {}\n</code></pre>\n"
            );
        }

        #[test]
        fn fenced_code_without_language() {
            assert_eq!(
                render("```\nplain\n```"),
                "<pre><code>plain\n</code></pre>\n"
            );
        }

        #[test]
        fn code_content_is_escaped_not_highlighted() {
            assert_eq!(
                render("```html\n<div>&amp;</div>\n```"),
                "<pre><code class=\"language-html\">&lt;div&gt;&amp;amp;&lt;/div&gt;\n</code></pre>\n"
            );
        }

        #[test]
        fn math_fence_renders_as_plain_language_tagged_code() {
            // Math rendering is entirely client-side JS in a later phase;
            // Phase 2 just tags the block with its fence info like any
            // other code fence.
            assert_eq!(
                render("```math\nx^2\n```"),
                "<pre><code class=\"language-math\">x^2\n</code></pre>\n"
            );
        }

        #[test]
        fn indented_code_block() {
            assert_eq!(render("    indented"), "<pre><code>indented</code></pre>\n");
        }
    }

    // MARK: Thematic breaks

    mod thematic_break_tests {
        use super::render;

        #[test]
        fn thematic_break() {
            assert_eq!(render("---"), "<hr />\n");
        }

        #[test]
        fn thematic_break_between_paragraphs() {
            assert_eq!(
                render("First\n\n---\n\nSecond"),
                "<p>First</p>\n<hr />\n<p>Second</p>\n"
            );
        }
    }

    // MARK: Hard line breaks

    mod hard_break_tests {
        use super::render;

        #[test]
        fn trailing_two_spaces_is_a_hard_break() {
            assert_eq!(
                render("line one  \nline two"),
                "<p>line one<br />\nline two</p>\n"
            );
        }

        #[test]
        fn backslash_is_a_hard_break() {
            assert_eq!(
                render("line one\\\nline two"),
                "<p>line one<br />\nline two</p>\n"
            );
        }

        #[test]
        fn single_newline_is_a_soft_break() {
            assert_eq!(render("line one\nline two"), "<p>line one\nline two</p>\n");
        }
    }

    // MARK: HTML blocks (passthrough)

    mod html_block_tests {
        use super::render;

        #[test]
        fn html_block_passes_through_unescaped() {
            assert_eq!(render("<div>\n  hi\n</div>\n"), "<div>\n  hi\n</div>\n");
        }

        #[test]
        fn inline_html_passes_through_unescaped() {
            assert_eq!(
                render("before <span>inline</span> after"),
                "<p>before <span>inline</span> after</p>\n"
            );
        }
    }

    // MARK: GFM extensions — tables

    mod table_tests {
        use super::render;

        #[test]
        fn simple_table_with_alignments() {
            assert_eq!(
                render("| L | C | R |\n|:--|:-:|--:|\n| a | b | c |\n"),
                "<table>\n\
                 <thead>\n<tr>\n\
                 <th class=\"align-left\">L</th>\n\
                 <th class=\"align-center\">C</th>\n\
                 <th class=\"align-right\">R</th>\n\
                 </tr>\n</thead>\n\
                 <tbody>\n<tr>\n\
                 <td class=\"align-left\">a</td>\n\
                 <td class=\"align-center\">b</td>\n\
                 <td class=\"align-right\">c</td>\n\
                 </tr>\n</tbody>\n\
                 </table>\n"
            );
        }

        #[test]
        fn header_only_table_has_no_tbody() {
            assert_eq!(
                render("| A | B |\n|---|---|\n"),
                "<table>\n\
                 <thead>\n<tr>\n<th>A</th>\n<th>B</th>\n</tr>\n</thead>\n\
                 </table>\n"
            );
        }

        #[test]
        fn row_with_fewer_cells_than_header_pads_with_empty_cells() {
            // GFM table rule: a short data row is padded with empty cells
            // rather than being a parse error.
            assert_eq!(
                render("| A | B |\n|---|---|\n| only |\n"),
                "<table>\n\
                 <thead>\n<tr>\n<th>A</th>\n<th>B</th>\n</tr>\n</thead>\n\
                 <tbody>\n<tr>\n<td>only</td>\n<td></td>\n</tr>\n</tbody>\n\
                 </table>\n"
            );
        }

        #[test]
        fn row_with_more_cells_than_header_drops_extras() {
            assert_eq!(
                render("| A | B |\n|---|---|\n| a | b | extra |\n"),
                "<table>\n\
                 <thead>\n<tr>\n<th>A</th>\n<th>B</th>\n</tr>\n</thead>\n\
                 <tbody>\n<tr>\n<td>a</td>\n<td>b</td>\n</tr>\n</tbody>\n\
                 </table>\n"
            );
        }
    }

    // MARK: GFM extensions — strikethrough

    mod strikethrough_tests {
        use super::render;

        #[test]
        fn strikethrough() {
            assert_eq!(render("~~struck~~"), "<p><s>struck</s></p>\n");
        }

        #[test]
        fn nested_strikethrough_inside_emphasis() {
            assert_eq!(
                render("*em ~~struck~~ text*"),
                "<p><em>em <s>struck</s> text</em></p>\n"
            );
        }
    }

    // MARK: GFM extensions — task lists

    mod task_list_tests {
        use super::render;

        #[test]
        fn unchecked_task_item() {
            assert_eq!(
                render("- [ ] todo"),
                "<ul>\n<li><input type=\"checkbox\" disabled=\"\" /> todo</li>\n</ul>\n"
            );
        }

        #[test]
        fn checked_task_item() {
            assert_eq!(
                render("- [x] done"),
                "<ul>\n<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> done</li>\n</ul>\n"
            );
        }

        #[test]
        fn mixed_checked_and_unchecked_items() {
            assert_eq!(
                render("- [x] done\n- [ ] not done\n"),
                "<ul>\n\
                 <li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> done</li>\n\
                 <li><input type=\"checkbox\" disabled=\"\" /> not done</li>\n\
                 </ul>\n"
            );
        }
    }

    // MARK: GFM extensions — autolinks

    mod autolink_tests {
        use super::render;

        #[test]
        fn uri_autolink() {
            assert_eq!(
                render("<https://example.com>"),
                "<p><a href=\"https://example.com\">https://example.com</a></p>\n"
            );
        }

        #[test]
        fn email_autolink_gets_mailto_prefix() {
            assert_eq!(
                render("<jane@example.com>"),
                "<p><a href=\"mailto:jane@example.com\">jane@example.com</a></p>\n"
            );
        }
    }
}

// ---------------------------------------------------------------------
// Down mode tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod down_tests {
    use super::*;

    fn render(markdown: &str) -> String {
        render_down(markdown, &RenderOptions::default())
    }

    #[test]
    fn empty_document_yields_no_line_divs() {
        assert_eq!(render(""), "");
    }

    #[test]
    fn single_line() {
        assert_eq!(
            render("hello world"),
            "<div class=\"line\" data-line=\"1\">hello world</div>"
        );
    }

    #[test]
    fn no_trailing_newline_does_not_add_a_phantom_empty_line() {
        assert_eq!(
            render("first\nsecond"),
            "<div class=\"line\" data-line=\"1\">first</div>\
             <div class=\"line\" data-line=\"2\">second</div>"
        );
    }

    #[test]
    fn trailing_newline_does_not_add_a_phantom_empty_line_either() {
        // A single trailing `\n` is just line termination, not a third
        // (empty) line — `str::lines()` already gives us this for free,
        // but it's worth pinning down explicitly since it's exactly the
        // "phantom trailing line" bug this step's plan calls out.
        assert_eq!(render("first\nsecond"), render("first\nsecond\n"));
    }

    #[test]
    fn multiple_blank_lines_are_each_preserved_and_numbered() {
        assert_eq!(
            render("a\n\n\nb"),
            "<div class=\"line\" data-line=\"1\">a</div>\
             <div class=\"line\" data-line=\"2\"></div>\
             <div class=\"line\" data-line=\"3\"></div>\
             <div class=\"line\" data-line=\"4\">b</div>"
        );
    }

    #[test]
    fn special_characters_are_escaped_exactly_once() {
        assert_eq!(
            render("<b>Tom & Jerry</b>"),
            "<div class=\"line\" data-line=\"1\">&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;</div>"
        );
    }

    #[test]
    fn a_line_that_is_already_html_escaped_text_is_escaped_again() {
        // Matches `html_escape`'s own documented behavior (no smart
        // double-escape detection) — `render_down` doesn't add any
        // special-casing on top of it.
        assert_eq!(
            render("&amp;"),
            "<div class=\"line\" data-line=\"1\">&amp;amp;</div>"
        );
    }
}

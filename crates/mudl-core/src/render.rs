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
use std::collections::HashMap;
use std::ops::Range;

use mudl_diff::block::{LeafBlock, LeafTag};
use mudl_diff::line::Annotation as LineAnnotation;
use mudl_diff::word::WordSpan;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, LinkType, Tag, TagEnd};

use crate::alerts::{detect_docc_aside, detect_gfm_alert, parse_aside_tag, AlertCategory};
use crate::changes::collect::{is_display_math, is_inline};
use crate::changes::placer::Context as PlaceContext;
use crate::changes::up::{ChangeAttrs, Role, UpChanges, WordSpanEmitter};
use crate::comments_html::{comments_section, footnotes_section};
use crate::emoji::replace_shortcodes;
use crate::encoding::html_escape;
use crate::footnotes::is_comment_label;
use crate::frontmatter::{self, parse_top_level_keys};
use crate::frontmatter_html::render_table as render_frontmatter_table;
use crate::html_sanitize::{is_safe_url, sanitize_html};
use crate::options::RenderOptions;
use crate::parse::ParsedMarkdown;
use crate::slug::Tracker;

/// Renders `markdown` to Up-mode HTML body content (no surrounding
/// `<html>`/`<head>` document — that's Phase 3's `HtmlDocument` template).
///
/// A leading YAML frontmatter block (`crate::frontmatter::extract`) is
/// pulled off first and rendered separately as a collapsible key-value
/// table (`crate::frontmatter_html::render_table`), prepended to the
/// document body's own rendered HTML — otherwise the raw YAML would be fed
/// straight into the CommonMark parser, which happily misreads `key:`
/// lines as paragraphs and any bare `# comment` as a heading.
///
/// With `options.waypoint` set (and differing from `markdown`), changed
/// blocks carry change-tracking markup — see [`crate::changes::up`] for
/// the contract. Without one, or when nothing changed, the output is
/// byte-identical to a plain render.
pub fn render_up(markdown: &str, options: &RenderOptions) -> String {
    let (body, frontmatter_html) = match frontmatter::extract(markdown) {
        Some(fm) => {
            let keys = parse_top_level_keys(&fm.yaml);
            (fm.body, render_frontmatter_table(&keys))
        }
        None => (markdown.to_string(), String::new()),
    };

    let parsed = ParsedMarkdown::new(&body);
    let footnote_numbers = number_footnotes(&parsed.events);
    let mut renderer = Renderer::new(&parsed.events, options, &footnote_numbers);
    renderer.out = frontmatter_html;
    // Change tracking (`_working/DIFF-IMPLEMENTATION.md` §9.2): only when
    // a waypoint is set *and* it differs, so an unchanged document renders
    // byte-identically to a plain one.
    renderer.changes = options
        .waypoint
        .as_ref()
        .and_then(|waypoint| UpChanges::build(waypoint, markdown, options));
    renderer.run();
    if let Some(changes) = renderer.changes.as_mut() {
        let trailing = changes.placer.trailing_html();
        renderer.out.push_str(&trailing);
    }
    // The bottom sections render definition bodies through `render_up`
    // again; those are never change-tracked (the Up policy skips
    // definitions), so they must not see the waypoint.
    let section_options;
    let section_options = if options.waypoint.is_some() {
        section_options = RenderOptions {
            waypoint: None,
            ..options.clone()
        };
        &section_options
    } else {
        options
    };
    renderer.out.push_str(&footnotes_section(
        &body,
        &footnote_numbers,
        section_options,
    ));
    renderer
        .out
        .push_str(&comments_section(&body, section_options));
    renderer.out
}

/// Assigns each *authorial* (non-comment) footnote label a 1-based display
/// number in first-reference order, over the same event stream `render_up`
/// walks -- comments occupy no number and leave no gap, matching `mud`'s
/// `FootnoteProcessor.process`. A reference nested inside another
/// footnote/comment's own definition is (unlike `mud`) not excluded from
/// this count -- a narrow, deliberate scope cut, since a footnote that
/// self-references is vanishingly rare and excluding it would need this
/// walk to also track definition boundaries, which it otherwise has no use
/// for (the definition-body skip in `render_container` handles the
/// exclusion structurally, by discarding the nested marker's *output*
/// rather than by never assigning it a number).
pub(crate) fn number_footnotes(events: &[(Event, Range<usize>)]) -> HashMap<String, u32> {
    let mut numbers = HashMap::new();
    let mut next_number = 1u32;
    for (event, _) in events {
        if let Event::FootnoteReference(label) = event {
            if !is_comment_label(label) && !numbers.contains_key(label.as_ref()) {
                numbers.insert(label.to_string(), next_number);
                next_number += 1;
            }
        }
    }
    numbers
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
/// With `options.waypoint` set (and differing from `markdown`), changed
/// rows are marked and deleted rows put back — see
/// [`crate::changes::down`]. Otherwise the output is unchanged.
pub fn render_down(markdown: &str, options: &RenderOptions) -> String {
    if let Some(waypoint) = &options.waypoint {
        if let Some(html) = crate::changes::down::render(waypoint, markdown) {
            return html;
        }
    }
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
    // Set by the `` $`…`$ `` inline-math check in `render_leaf`'s `Code`
    // arm when it consumes the opening `$` off the end of `self.out`: the
    // very next event is the `Text` holding the closing `$`, so its leading
    // byte must be dropped too. Always consumed by the next `Text` seen.
    skip_leading_dollar: bool,
    /// Authorial footnote label -> 1-based display number (Phase 14.6),
    /// computed once by `number_footnotes` before the walk begins so a
    /// `FootnoteReference` can render its marker without re-scanning.
    footnote_numbers: &'a HashMap<String, u32>,

    // MARK: Change tracking (all inert for a plain render)
    /// The Up-mode change overlay; `None` renders exactly as without
    /// change tracking.
    changes: Option<UpChanges>,
    /// The active word-span cursor of the block being rendered.
    emitter: Option<WordSpanEmitter>,
    /// A simple loose list item's word spans, handed to its paragraph.
    item_spans: Option<Vec<WordSpan>>,
    /// Set while rendering a deleted block from the old document: comment
    /// markers are suppressed so the surviving block's marker stays the
    /// only anchor for the comment in the DOM.
    is_deletion_render: bool,
}

impl<'a> Renderer<'a> {
    fn new(
        events: &'a [(Event<'a>, Range<usize>)],
        options: &'a RenderOptions,
        footnote_numbers: &'a HashMap<String, u32>,
    ) -> Self {
        Renderer {
            events,
            pos: 0,
            out: String::new(),
            slugs: Tracker::new(),
            options,
            table_alignments: Vec::new(),
            in_table_head: false,
            cell_index: 0,
            skip_leading_dollar: false,
            footnote_numbers,
            changes: None,
            emitter: None,
            item_spans: None,
            is_deletion_render: false,
        }
    }

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

    /// GitHub's other display-math form: a paragraph whose entire text is
    /// fenced by a literal `$$` at each end (the doc's own example:
    /// `$$ x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a} $$` as its own paragraph).
    /// If `self.pos` (already past the `Paragraph` `Start`) opens such a
    /// paragraph, consumes it through its matching `End` and returns the
    /// inner TeX; otherwise leaves `self.pos` untouched.
    fn take_display_math_paragraph(&mut self) -> Option<String> {
        let trimmed = self.peek_plain_text(self.pos).trim().to_string();
        if trimmed.len() > 4 && trimmed.starts_with("$$") && trimmed.ends_with("$$") {
            self.collect_plain_text();
            Some(trimmed[2..trimmed.len() - 2].trim().to_string())
        } else {
            None
        }
    }

    /// Emits the same `<pre><code class="language-math">` markup a fenced
    /// ` ```math ` block gets from `render_code_block`, so a `$$ ... $$`
    /// paragraph is picked up by the identical client-side Temml pass in
    /// `math-init.js` — no separate JS path needed for this form.
    fn push_math_fence(&mut self, tex: &str) {
        self.out.push_str("<pre><code class=\"language-math\">");
        self.out.push_str(&html_escape(tex));
        self.out.push_str("\n</code></pre>\n");
    }

    /// The inline counterpart to `push_math_fence`: `math-init.js` selects
    /// on the `language-math-inline` class (a superstring of
    /// `language-math`, so `select_assets`'s substring check still catches
    /// it) and renders with `displayMode: false` instead of swapping in a
    /// `<pre>`-based block wrapper, keeping the math flowing inline.
    fn push_inline_math(&mut self, tex: &str) {
        self.out.push_str("<code class=\"language-math-inline\">");
        self.out.push_str(&html_escape(tex));
        self.out.push_str("</code>");
    }

    /// The byte range of the `Start` event `render_container` was just
    /// handed (always `self.pos - 1` on entry): the key change tracking
    /// joins blocks on.
    fn start_range(&self) -> Range<usize> {
        self.events[self.pos - 1].1.clone()
    }

    /// Writes a block element's opening tag: `<name{attrs} class="…">`.
    /// `class` may be empty (no `class` attribute is written). Every
    /// block arm opens its element through here so that change tracking
    /// has a single place to add its attributes, keyed by the block's
    /// source `range` (inline tags are unaffected): any deletions keyed to
    /// the block are emitted first, then the tag with `mud-change-ins`
    /// merged into its class and the `data-change-*` attributes appended.
    fn open_tag(&mut self, name: &str, class: &str, attrs: &str, range: &Range<usize>) {
        let context = if name == "li" {
            PlaceContext::ListItem
        } else {
            PlaceContext::Block
        };
        let change = self.change_attrs(range, context);
        self.write_open_tag(name, class, attrs, change.as_ref());
    }

    fn write_open_tag(
        &mut self,
        name: &str,
        class: &str,
        attrs: &str,
        change: Option<&ChangeAttrs>,
    ) {
        self.out.push('<');
        self.out.push_str(name);
        self.out.push_str(attrs);
        let change_class = change.map_or("", |c| c.class);
        if !class.is_empty() || !change_class.is_empty() {
            self.out.push_str(" class=\"");
            self.out.push_str(class);
            if !class.is_empty() && !change_class.is_empty() {
                self.out.push(' ');
            }
            self.out.push_str(change_class);
            self.out.push('"');
        }
        if let Some(change) = change {
            self.out.push_str(&change.data);
        }
        self.out.push('>');
    }

    // MARK: - Change tracking hooks

    /// Emits the deletions keyed to the block at `range` and returns its
    /// insertion attributes, if it changed. A no-op without a waypoint.
    fn change_attrs(&mut self, range: &Range<usize>, context: PlaceContext) -> Option<ChangeAttrs> {
        let changes = self.changes.as_mut()?;
        let preceding = changes.placer.preceding_html(range, context);
        self.out.push_str(&preceding);
        changes.attrs(range)
    }

    /// The word spans of the changed block at `range` whose inline events
    /// are `lo..hi`, unless the block renders math (its MathML
    /// substitution would desync the cursor; it keeps whole-block
    /// treatment instead, as in `mud`).
    fn block_spans(&self, range: &Range<usize>, lo: usize, hi: usize) -> Option<Vec<WordSpan>> {
        let spans = self.changes.as_ref()?.word_spans(range)?;
        (!contains_inline_math(&self.events[lo..hi.min(self.events.len())])).then(|| spans.to_vec())
    }

    /// [`Renderer::block_spans`] for the container whose `Start` is at
    /// `start` (its extent is only computed for a changed block).
    fn container_spans(&self, range: &Range<usize>, start: usize) -> Option<Vec<WordSpan>> {
        self.changes.as_ref()?.word_spans(range)?;
        self.block_spans(range, start + 1, self.matching_end(start))
    }

    /// Starts an insertion-role cursor over `spans`; false when there are
    /// none or one is already active (the caller then must not finish it).
    fn activate(&mut self, spans: Option<Vec<WordSpan>>) -> bool {
        match spans {
            Some(spans) if self.emitter.is_none() => {
                let show = self
                    .changes
                    .as_ref()
                    .is_some_and(|c| c.show_inline_deletions);
                self.emitter = Some(WordSpanEmitter::new(spans, Role::Insertion, show));
                true
            }
            _ => false,
        }
    }

    /// Flushes and clears the active cursor.
    fn finish_spans(&mut self) {
        if let Some(mut emitter) = self.emitter.take() {
            self.out.push_str(&emitter.finish());
        }
    }

    /// Feeds `chars` characters to the active cursor (`emit` false for
    /// breaks and alt text) and closes its marker. Returns false without
    /// a cursor.
    fn advance_spans(&mut self, chars: usize, emit: bool) -> bool {
        let Some(emitter) = self.emitter.as_mut() else {
            return false;
        };
        let mut html = emitter.advance(chars, emit);
        html.push_str(&emitter.close_open_tag());
        self.out.push_str(&html);
        true
    }

    /// Emits a run of literal text: through the cursor when one is
    /// active, otherwise shortcode-replaced and escaped.
    fn emit_text(&mut self, text: &str) {
        if !self.advance_spans(text.chars().count(), true) {
            self.out.push_str(&html_escape(&replace_shortcodes(text)));
        }
    }

    /// Index of the `End` matching the `Start` at `start`.
    fn matching_end(&self, start: usize) -> usize {
        let mut depth = 0usize;
        for (i, (event, _)) in self.events.iter().enumerate().skip(start) {
            match event {
                Event::Start(_) => depth += 1,
                Event::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                _ => {}
            }
        }
        self.events.len()
    }

    /// The end (exclusive) of the run of inline events starting at
    /// `start`, as the leaf collector groups them.
    fn inline_run_end(&self, start: usize) -> usize {
        let mut i = start;
        while let Some((event, _)) = self.events.get(i) {
            if !is_inline(event) {
                break;
            }
            i = if matches!(event, Event::Start(_)) {
                self.matching_end(i) + 1
            } else {
                i + 1
            };
        }
        i
    }

    /// The change-tracking key of the block starting at event `i`: an
    /// inline run's span, or the event's own range.
    fn block_key(&self, i: usize) -> Option<Range<usize>> {
        let (event, range) = self.events.get(i)?;
        if matches!(event, Event::End(_)) {
            return None;
        }
        if is_inline(event) {
            let end = self.inline_run_end(i);
            return Some(range.start..self.events[end - 1].1.end);
        }
        Some(range.clone())
    }

    fn render_paragraph(&mut self) {
        let range = self.start_range();
        let start = self.pos;
        if let Some(tex) = self.take_display_math_paragraph() {
            match self.change_attrs(&range, PlaceContext::Block) {
                Some(change) => {
                    self.write_open_tag("div", "", "", Some(&change));
                    self.push_math_fence(&tex);
                    self.out.push_str("</div>\n");
                }
                None => self.push_math_fence(&tex),
            }
            return;
        }
        self.open_tag("p", "", "", &range);
        let spans = self
            .container_spans(&range, start - 1)
            .or_else(|| self.item_spans.take());
        let active = self.activate(spans);
        self.children();
        if active {
            self.finish_spans();
        }
        self.out.push_str("</p>\n");
    }

    /// A list item. Without change tracking this is `<li>` + children.
    /// With it, `li` deletions keyed to the first child are emitted before
    /// the `<li>`, a simple item's word spans are activated for its
    /// content, and each inline run (a tight item's text has no
    /// `Paragraph` of its own) is looked up as a leaf: a changed run in a
    /// complex item is wrapped in `<span class="mud-change-ins" …>`.
    fn render_item(&mut self) {
        let range = self.start_range();
        if self.changes.is_none() {
            self.open_tag("li", "", "", &range);
            self.children();
            self.out.push_str("</li>\n");
            return;
        }
        if let Some(first) = self.block_key(self.pos) {
            let html = self
                .changes
                .as_mut()
                .map(|c| c.placer.list_item_html(&first))
                .unwrap_or_default();
            self.out.push_str(&html);
        }
        self.open_tag("li", "", "", &range);
        let mut item_spans = self.container_spans(&range, self.pos - 1);
        loop {
            match self.current() {
                Some(Event::End(_)) => {
                    self.pos += 1;
                    break;
                }
                None => break,
                Some(event) if is_inline(event) => {
                    let spans = item_spans.take();
                    self.render_inline_run(spans);
                }
                Some(Event::Start(Tag::Paragraph)) => {
                    self.item_spans = item_spans.take();
                    self.step();
                    self.item_spans = None;
                }
                Some(_) => self.step(),
            }
        }
        self.out.push_str("</li>\n");
    }

    /// An inline run directly under a list item (change tracking only).
    fn render_inline_run(&mut self, item_spans: Option<Vec<WordSpan>>) {
        let start = self.pos;
        let end = self.inline_run_end(start);
        let range = self.events[start].1.start..self.events[end - 1].1.end;
        let change = self.change_attrs(&range, PlaceContext::Block);
        if let Some(change) = &change {
            self.write_open_tag("span", "", "", Some(change));
        }
        let spans = item_spans.or_else(|| self.block_spans(&range, start, end));
        let active = self.activate(spans);
        while self.pos < end {
            self.step();
        }
        if active {
            self.finish_spans();
        }
        if change.is_some() {
            self.out.push_str("</span>");
        }
    }

    fn render_container(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => self.render_paragraph(),
            Tag::Heading { level, .. } => {
                let range = self.start_range();
                let start = self.pos;
                let plain = self.peek_plain_text(self.pos);
                let slug = self.slugs.track(&plain);
                let id = format!(" id=\"{}\"", html_escape(&slug));
                self.open_tag(&level.to_string(), "", &id, &range);
                let spans = self.container_spans(&range, start - 1);
                let active = self.activate(spans);
                self.children();
                if active {
                    self.finish_spans();
                }
                self.out.push_str(&format!("</{level}>\n"));
            }
            Tag::BlockQuote(_) => self.render_blockquote(),
            Tag::CodeBlock(kind) => self.render_code_block(kind),
            Tag::HtmlBlock => self.render_html_block(),
            Tag::List(start) => self.render_list(start),
            Tag::Item => self.render_item(),
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
            // A footnote/comment definition's body renders separately, in
            // the bottom Footnotes/Comments section (`footnotes_section`/
            // `comments_section`) — never inline where it's *defined*, only
            // where it's *referenced* (a superscript number or a `💬`
            // marker, from `Event::FootnoteReference` below). So its
            // subtree is still walked (`children()` advances `self.pos`
            // past the whole balanced definition, exactly like every other
            // container), but the HTML it would have produced is
            // discarded: swap in a scratch buffer for the duration, matching
            // the visitor's "consume the whole balanced subtree" shape
            // rather than adding a second traversal mode.
            Tag::FootnoteDefinition(_) => {
                let discarded = std::mem::take(&mut self.out);
                self.children();
                self.out = discarded;
            }
            // None of these are ever produced by the `Options` bitset
            // `parser_options` turns on (definition lists, sub/superscript,
            // heading attributes, and metadata blocks are never enabled at
            // all) — kept so the match stays exhaustive if that ever
            // changes, rendering inner content with no wrapper rather than
            // panicking.
            Tag::DefinitionList
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
                let s: &str = if self.skip_leading_dollar {
                    self.skip_leading_dollar = false;
                    &s[1..]
                } else {
                    &s
                };
                self.emit_text(s);
            }
            Event::Code(s) if self.emitter.is_some() => {
                // Word spans are never active in a math-bearing block, so
                // no inline-math check is needed here.
                self.out.push_str("<code>");
                self.advance_spans(s.chars().count(), true);
                self.out.push_str("</code>");
            }
            Event::Code(s) => {
                // GitHub's inline-math form: a code span immediately
                // flanked by a bare `$` on each side — `` $`…`$ `` — with
                // no space between the `$` and the backtick, unlike a bare
                // `$x + y$` (which the parser never turns into a `Code`
                // event at all, so it can't reach this branch and stays
                // literal text, matching the doc's "what is not math").
                let closes_with_dollar =
                    matches!(self.current(), Some(Event::Text(t)) if t.starts_with('$'));
                if self.out.ends_with('$') && closes_with_dollar {
                    self.out.pop();
                    self.push_inline_math(&s);
                    self.skip_leading_dollar = true;
                } else {
                    self.out.push_str("<code>");
                    self.out.push_str(&html_escape(&s));
                    self.out.push_str("</code>");
                }
            }
            Event::InlineHtml(s) => {
                self.out.push_str(&sanitize_html(&s));
            }
            Event::SoftBreak => {
                self.advance_spans(1, false);
                self.out.push('\n');
            }
            Event::HardBreak => {
                self.advance_spans(1, false);
                self.out.push_str("<br />\n");
            }
            Event::Rule => {
                let range = self.events[self.pos - 1].1.clone();
                match self.change_attrs(&range, PlaceContext::Block) {
                    Some(change) => {
                        self.write_open_tag("div", "", "", Some(&change));
                        self.out.push_str("<hr /></div>\n");
                    }
                    None => self.out.push_str("<hr />\n"),
                }
            }
            Event::TaskListMarker(checked) => {
                if checked {
                    self.out
                        .push_str("<input type=\"checkbox\" checked=\"\" disabled=\"\" /> ");
                } else {
                    self.out
                        .push_str("<input type=\"checkbox\" disabled=\"\" /> ");
                }
            }
            // Math is not enabled by `parser_options`; these variants
            // cannot be produced by the parser today. No-op rather than
            // making the match non-exhaustive.
            Event::InlineMath(_) | Event::DisplayMath(_) => {}
            Event::FootnoteReference(label) => self.render_footnote_reference(&label),
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
        if let Some(alert) = self.detect_alert() {
            self.render_alert_body(&alert, true);
            return;
        }

        self.out.push_str("<blockquote>\n");
        self.children();
        self.out.push_str("</blockquote>\n");
    }

    /// Detects a GFM alert or DocC aside opening at `self.pos` (a
    /// blockquote's first child).
    fn detect_alert(&self) -> Option<Alert> {
        let first_line = match self.current() {
            Some(Event::Start(Tag::Paragraph)) => self.peek_leading_text_run(self.pos + 1).0,
            _ => return None,
        };
        if let Some((category, ())) = detect_gfm_alert(&first_line) {
            return Some(Alert {
                class: alert_class(category),
                title: alert_title(category).to_string(),
                strip_len: gfm_tag_literal(category).len(),
                docc: false,
            });
        }
        let (tag, byte_len) = parse_aside_tag(&first_line)?;
        let category = detect_docc_aside(&tag, self.options.doc_c_alert_mode)?;
        Some(Alert {
            class: alert_class(category),
            title: docc_display_name(&tag),
            strip_len: byte_len,
            docc: true,
        })
    }

    /// Shared alert body renderer. `self.pos` must be at the blockquote's
    /// `Start(Paragraph)` with a leading `Text` child — verified by
    /// `detect_alert`. Strips `strip_len` bytes (the detected tag, colon,
    /// and trailing whitespace) off that first text node, then renders the
    /// rest of the paragraph and the remaining blockquote content
    /// normally. `wrapper` false omits the `<blockquote>` tags (a deleted
    /// alert, which the placer wraps itself).
    ///
    /// Change tracking keys an alert by its first paragraph, so its
    /// attributes land on the `<blockquote>`. Only a DocC aside takes word
    /// spans (as in `mud`), with the cursor skipping the `Kind:` prefix.
    fn render_alert_body(&mut self, alert: &Alert, wrapper: bool) {
        let para_start = self.pos;
        let range = self.events[para_start].1.clone(); // the inner paragraph
        if wrapper {
            self.open_tag("blockquote", &format!("alert {}", alert.class), "", &range);
            self.out.push('\n');
        }
        self.out.push_str(&format!(
            "<p class=\"alert-title\">{}</p>\n",
            html_escape(&alert.title)
        ));

        let active = alert.docc && {
            let spans = self.container_spans(&range, para_start);
            self.activate(spans)
        };

        self.pos += 1; // consume Start(Paragraph)
        let (first_text, after_leading_text) = self.peek_leading_text_run(self.pos);
        self.pos = after_leading_text; // consume the tag's leading Text run

        let remainder = first_text
            .get(alert.strip_len.min(first_text.len())..)
            .unwrap_or("")
            .trim_start_matches(' ');
        if let Some(emitter) = self.emitter.as_mut() {
            let prefix = first_text.len() - remainder.len();
            emitter.skip_prefix(first_text[..prefix].chars().count());
        }

        let mut opened = false;
        if !remainder.is_empty() {
            opened = true;
            self.out.push_str("<p>");
            self.emit_text(remainder);
        }
        // Skip the soft break separating the tag line from the rest of the
        // paragraph's content, if there is one.
        if matches!(self.current(), Some(Event::SoftBreak)) {
            self.pos += 1;
            self.advance_spans(1, false);
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
        if active {
            self.finish_spans();
        }
        if opened {
            self.out.push_str("</p>\n");
        }

        self.children(); // remaining blockquote blocks; consumes End(BlockQuote)
        if wrapper {
            self.out.push_str("</blockquote>\n");
        }
    }

    // MARK: - Code blocks and HTML blocks

    /// Consumes a code block's content through its `End`, returning the
    /// language (the info string's first word) and the code.
    fn take_code_block(&mut self, kind: &CodeBlockKind<'a>) -> (Option<String>, String) {
        let lang = match kind {
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
        (lang, code)
    }

    /// `<code[ class="language-…"]>` — a code block's opening inner tag.
    fn push_code_open(&mut self, lang: Option<&str>) {
        match lang {
            Some(lang) => {
                self.out.push_str("<code class=\"language-");
                self.out.push_str(&html_escape(lang));
                self.out.push_str("\">");
            }
            None => self.out.push_str("<code>"),
        }
    }

    /// A code block's inner HTML: `<code …>escaped</code>`.
    fn push_code_inner(&mut self, lang: Option<&str>, code: &str) {
        self.push_code_open(lang);
        self.out.push_str(&html_escape(code));
        self.out.push_str("</code>");
    }

    fn render_code_block(&mut self, kind: CodeBlockKind<'a>) {
        let range = self.start_range();
        let (lang, code) = self.take_code_block(&kind);
        if self.changes.is_none() {
            self.open_tag("pre", "", "", &range);
            self.push_code_inner(lang.as_deref(), &code);
            self.out.push_str("</pre>\n");
            return;
        }

        let change = self.change_attrs(&range, PlaceContext::Block);
        if let Some(lines) = self
            .changes
            .as_ref()
            .and_then(|c| c.code_diff(&range))
            .map(<[_]>::to_vec)
        {
            self.push_code_diff(lang.as_deref(), &lines);
            return;
        }
        match change {
            // Mermaid and math blocks are swapped out wholesale by their
            // client-side renderers, which would drop attributes on the
            // `<pre>`: carry them on a wrapper instead.
            Some(change) if lang.as_deref().is_some_and(is_diagram_language) => {
                self.write_open_tag("div", "", "", Some(&change));
                self.out.push_str("<pre>");
                self.push_code_inner(lang.as_deref(), &code);
                self.out.push_str("</pre>\n</div>\n");
            }
            change => {
                self.write_open_tag("pre", "", "", change.as_ref());
                self.push_code_inner(lang.as_deref(), &code);
                self.out.push_str("</pre>\n");
            }
        }
    }

    /// A paired code block's line diff: one `<span class="cl …">` per line
    /// (unchanged, `cl-del`, or `cl-ins`), each holding the escaped line
    /// with word-level `<del>`/`<ins>` and its trailing newline. The
    /// `<pre>` itself carries no change ID; the line spans own them.
    fn push_code_diff(&mut self, lang: Option<&str>, lines: &[mudl_diff::code::CodeLine]) {
        self.out.push_str("<pre class=\"mud-code-diff\">");
        self.push_code_open(lang);
        for line in lines {
            let (class, mark) = match line.annotation {
                LineAnnotation::Unchanged => ("cl", ""),
                LineAnnotation::Inserted => ("cl cl-ins", "ins"),
                LineAnnotation::Deleted => ("cl cl-del", "del"),
            };
            let data = self
                .changes
                .as_ref()
                .map(|c| c.code_line_data(line))
                .unwrap_or_default();
            self.out
                .push_str(&format!("<span class=\"{class}\"{data}>"));
            let mut at = 0;
            for range in &line.word_ranges {
                let (start, end) = (
                    range.start.min(line.text.len()),
                    range.end.min(line.text.len()),
                );
                if start < at
                    || !line.text.is_char_boundary(start)
                    || !line.text.is_char_boundary(end)
                {
                    continue;
                }
                self.out.push_str(&html_escape(&line.text[at..start]));
                self.out.push_str(&format!(
                    "<{mark}>{}</{mark}>",
                    html_escape(&line.text[start..end])
                ));
                at = end;
            }
            self.out.push_str(&html_escape(&line.text[at..]));
            self.out.push_str("\n</span>");
        }
        self.out.push_str("</code></pre>\n");
    }

    fn take_html_block(&mut self) -> String {
        let mut block = String::new();
        loop {
            match self.current() {
                Some(Event::Html(s)) => {
                    block.push_str(s);
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
        block
    }

    fn render_html_block(&mut self) {
        let range = self.start_range();
        let block = self.take_html_block();
        match self.change_attrs(&range, PlaceContext::Block) {
            Some(change) => {
                self.write_open_tag("div", "mud-html-block", "", Some(&change));
                self.out.push_str(&sanitize_html(&block));
                self.out.push_str("</div>\n");
            }
            None => self.out.push_str(&sanitize_html(&block)),
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

    /// Runs `f` on the placer, appending what it returns (change tracking
    /// only).
    fn place(&mut self, f: impl FnOnce(&mut crate::changes::placer::Placer) -> String) {
        if let Some(changes) = self.changes.as_mut() {
            let html = f(&mut changes.placer);
            self.out.push_str(&html);
        }
    }

    fn render_table(&mut self, alignments: Vec<Alignment>) {
        self.table_alignments = alignments;

        // Deletions keyed to the head are hoisted above the table, so
        // they don't become invalid children of `<table>`.
        if let Some(Event::Start(Tag::TableHead)) = self.current() {
            let head = self.events[self.pos].1.clone();
            self.place(|p| p.hoisted_html(&head));
        }

        self.out.push_str("<table>\n");

        // A GFM table always has exactly one header row.
        if matches!(self.current(), Some(Event::Start(Tag::TableHead))) {
            self.step();
        }

        if matches!(self.current(), Some(Event::Start(Tag::TableRow))) {
            self.out.push_str("<tbody>\n");
            let mut last_row = None;
            while matches!(self.current(), Some(Event::Start(Tag::TableRow))) {
                let row = self.events[self.pos].1.clone();
                self.place(|p| p.row_html(&row));
                self.step();
                last_row = Some(row);
            }
            if let Some(row) = last_row {
                self.place(|p| p.reclaimed_row_html(&row));
            }
            self.out.push_str("</tbody>\n");
        }

        if matches!(self.current(), Some(Event::End(TagEnd::Table))) {
            self.pos += 1;
        }
        self.out.push_str("</table>\n");
        self.table_alignments.clear();
        self.place(|p| p.deferred_html());
    }

    fn render_table_head(&mut self) {
        let range = self.start_range();
        self.in_table_head = true;
        self.cell_index = 0;
        self.out.push_str("<thead>\n");
        self.open_tag("tr", "", "", &range);
        self.out.push('\n');
        self.children();
        self.out.push_str("</tr>\n</thead>\n");
        self.in_table_head = false;
    }

    fn render_table_row(&mut self) {
        let range = self.start_range();
        self.cell_index = 0;
        self.open_tag("tr", "", "", &range);
        self.out.push('\n');
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
        // `docs/SECURITY.md` Finding 3: a link destination is HTML-escaped
        // below, but escaping doesn't touch its scheme — a `javascript:`
        // URL survives untouched otherwise and runs on click.
        let href = if is_safe_url(&href) {
            href
        } else {
            "#".to_string()
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
        // The leaf collector counts alt text into the inline stream.
        self.advance_spans(alt.chars().count(), false);
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

    // MARK: - Footnotes and comments

    /// Renders a `[^label]` marker inline: a superscript numbered backlink
    /// for an authorial footnote, a `💬` marker for a comment (`mud`'s own
    /// glyph for the same purpose). `self.footnote_numbers` is populated
    /// for every authorial label the parser resolved a reference to (see
    /// `number_footnotes`), so the number lookup always succeeds for a
    /// non-comment label reaching here; a comment label is never in that
    /// map at all, hence the `is_comment_label` check first.
    fn render_footnote_reference(&mut self, label: &str) {
        let escaped = html_escape(label);
        if is_comment_label(label) && self.is_deletion_render {
            // A deleted copy of a block emits no comment marker: the
            // surviving block's marker must stay the only anchor.
        } else if is_comment_label(label) {
            self.out.push_str(&format!(
                "<a class=\"mud-comment-marker\" id=\"cmtref-{escaped}\" href=\"#cmt-{escaped}\">\u{1F4AC}</a>"
            ));
        } else if let Some(&number) = self.footnote_numbers.get(label) {
            // `footnote-ref` matches `mud`'s own class (and the selector
            // already bundled in `mud-up.css` since Phase 3) so the marker
            // gets its chip styling instead of rendering as a bare number.
            self.out.push_str(&format!(
                "<sup class=\"footnote-ref\" id=\"fnref-{escaped}\"><a href=\"#fn-{escaped}\">{number}</a></sup>"
            ));
        }
    }
}

/// A detected GFM alert or DocC aside, ready to render.
struct Alert {
    class: &'static str,
    title: String,
    /// Bytes of the tag prefix to strip off the first text run.
    strip_len: usize,
    /// A DocC aside (only those take word spans).
    docc: bool,
}

/// Mermaid and math fences, which client-side renderers replace wholesale.
fn is_diagram_language(lang: &str) -> bool {
    lang.eq_ignore_ascii_case("mermaid") || lang.eq_ignore_ascii_case("math")
}

/// True when `events` hold a `` $`…`$ `` inline-math span: a code span
/// between a text ending in `$` and a text starting with `$` (the same
/// shape `render_leaf`'s `Code` arm turns into math).
fn contains_inline_math(events: &[(Event, Range<usize>)]) -> bool {
    events.windows(3).any(|w| {
        matches!(&w[0].0, Event::Text(t) if t.ends_with('$'))
            && matches!(w[1].0, Event::Code(_))
            && matches!(&w[2].0, Event::Text(t) if t.starts_with('$'))
    })
}

// MARK: - Deleted blocks

/// A deleted block rendered from the old document: its native element,
/// inner HTML, and any classes that precede `mud-change-del`.
pub(crate) struct DeletionBody {
    pub tag: String,
    pub html: String,
    pub extra_class: Option<String>,
}

/// Renders the deleted leaf `leaf` of the old document `old`
/// (`DeletionRenderer.swift`): the renderer runs over the old document's
/// own events for that block, seeded with the old document's footnote
/// numbering, with comment markers suppressed. `spans` (a paired
/// deletion's significant word spans) mark deleted words with `<del>`.
///
/// - A deleted Mermaid block shows `<code><em>[revised diagram]</em></code>`.
/// - Display math (```` ```math ````, `$$…$$`) renders as math inside a
///   `<div>`, never with word spans.
/// - An alert's first paragraph renders the whole alert (title included)
///   as a `<blockquote class="alert …">`.
pub(crate) fn render_deletion(
    old: &ParsedMarkdown,
    numbers: &HashMap<String, u32>,
    options: &RenderOptions,
    leaf: &LeafBlock,
    spans: Option<&[WordSpan]>,
) -> DeletionBody {
    let mut r = Renderer::new(&old.events, options, numbers);
    r.is_deletion_render = true;
    let body = |tag: &str, html: String| DeletionBody {
        tag: tag.to_string(),
        html,
        extra_class: None,
    };

    let Some(i) = locate_leaf(&old.events, leaf) else {
        // A synthesised inline-run paragraph (no `Start` of its own), or
        // a leaf the parse no longer yields: render the run, else the
        // escaped source.
        let run = old
            .events
            .iter()
            .position(|(event, range)| range.start == leaf.byte_range.start && is_inline(event));
        let Some(start) = run else {
            return body("p", html_escape(&leaf.source_text));
        };
        let end = r.inline_run_end(start);
        r.pos = start;
        r.start_deletion_spans(spans, start, end);
        while r.pos < end {
            r.step();
        }
        r.finish_spans();
        return body("p", r.out);
    };

    let end = r.matching_end(i);
    let event = old.events[i].0.clone();
    match (leaf.tag, event) {
        (_, Event::Rule) => body("hr", String::new()),
        (_, Event::Start(Tag::HtmlBlock)) => {
            r.pos = i + 1;
            let block = r.take_html_block();
            DeletionBody {
                tag: "div".to_string(),
                html: sanitize_html(&block),
                extra_class: Some("mud-html-block".to_string()),
            }
        }
        (_, Event::Start(Tag::CodeBlock(kind))) => {
            r.pos = i + 1;
            let (lang, code) = r.take_code_block(&kind);
            match lang.as_deref() {
                Some(l) if l.eq_ignore_ascii_case("mermaid") => {
                    body("pre", "<code><em>[revised diagram]</em></code>".to_string())
                }
                Some(l) if is_diagram_language(l) => {
                    r.out.push_str("<pre>");
                    r.push_code_inner(lang.as_deref(), &code);
                    r.out.push_str("</pre>\n");
                    body("div", r.out)
                }
                _ => {
                    r.push_code_inner(lang.as_deref(), &code);
                    body("pre", r.out)
                }
            }
        }
        (_, Event::Start(Tag::Paragraph)) if is_display_math(&old.events[i + 1..end]) => {
            r.pos = i + 1;
            if let Some(tex) = r.take_display_math_paragraph() {
                r.push_math_fence(&tex);
            }
            body("div", r.out)
        }
        (_, Event::Start(Tag::Paragraph))
            if i > 0 && matches!(old.events[i - 1].0, Event::Start(Tag::BlockQuote(_))) =>
        {
            r.pos = i;
            match r.detect_alert() {
                Some(alert) => {
                    if alert.docc {
                        r.start_deletion_spans(spans, i + 1, end);
                    }
                    r.out.push('\n'); // as after an inserted alert's open tag
                    r.render_alert_body(&alert, false);
                    r.finish_spans();
                    DeletionBody {
                        tag: "blockquote".to_string(),
                        html: r.out,
                        extra_class: Some(format!("alert {}", alert.class)),
                    }
                }
                None => r.render_deletion_children(i, end, spans, "p"),
            }
        }
        (LeafTag::Heading(level), _) => {
            r.render_deletion_children(i, end, spans, &format!("h{level}"))
        }
        (_, Event::Start(Tag::Item)) => r.render_deletion_children(i, end, spans, "li"),
        (_, Event::Start(Tag::TableHead)) => {
            r.in_table_head = true;
            r.out.push('\n');
            r.render_deletion_children(i, end, spans, "tr")
        }
        (_, Event::Start(Tag::TableRow)) => {
            r.out.push('\n');
            r.render_deletion_children(i, end, spans, "tr")
        }
        _ => r.render_deletion_children(i, end, spans, "p"),
    }
}

/// The index of `leaf`'s own `Start` (or `Rule`) event: the event whose
/// range is the leaf's and whose kind matches its tag (a one-item list
/// shares its item's range).
fn locate_leaf(events: &[(Event, Range<usize>)], leaf: &LeafBlock) -> Option<usize> {
    events.iter().position(|(event, range)| {
        *range == leaf.byte_range
            && matches!(
                (leaf.tag, event),
                (LeafTag::Paragraph, Event::Start(Tag::Paragraph))
                    | (LeafTag::Heading(_), Event::Start(Tag::Heading { .. }))
                    | (LeafTag::ListItem, Event::Start(Tag::Item))
                    | (LeafTag::TableRow, Event::Start(Tag::TableRow))
                    | (LeafTag::TableHead, Event::Start(Tag::TableHead))
                    | (LeafTag::CodeBlock, Event::Start(Tag::CodeBlock(_)))
                    | (LeafTag::HtmlBlock, Event::Start(Tag::HtmlBlock))
                    | (LeafTag::ThematicBreak, Event::Rule)
            )
    })
}

impl Renderer<'_> {
    /// Starts a deletion-role cursor for the block whose inline events
    /// are `lo..hi`, unless there are no spans or the block holds math.
    fn start_deletion_spans(&mut self, spans: Option<&[WordSpan]>, lo: usize, hi: usize) {
        let Some(spans) = spans.filter(|s| !s.is_empty()) else {
            return;
        };
        if contains_inline_math(&self.events[lo..hi.min(self.events.len())]) {
            return;
        }
        self.emitter = Some(WordSpanEmitter::new(spans.to_vec(), Role::Deletion, false));
    }

    /// Renders the children of the `Start` at `i` (through its `End` at
    /// `end`) as a deletion's inner HTML, with deletion-role word spans.
    fn render_deletion_children(
        mut self,
        i: usize,
        end: usize,
        spans: Option<&[WordSpan]>,
        tag: &str,
    ) -> DeletionBody {
        self.start_deletion_spans(spans, i + 1, end);
        self.pos = i + 1;
        self.children();
        self.finish_spans();
        DeletionBody {
            tag: tag.to_string(),
            html: self.out,
            extra_class: None,
        }
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
                ..Default::default()
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
                ..Default::default()
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

    // MARK: Display/inline math ($$...$$ and $`...`$)

    mod math_tests {
        use super::render;

        #[test]
        fn dollar_paragraph_renders_as_a_math_fence() {
            assert_eq!(
                render("$$ x = \\frac{-b}{2a} $$"),
                "<pre><code class=\"language-math\">x = \\frac{-b}{2a}\n</code></pre>\n"
            );
        }

        #[test]
        fn dollar_paragraph_preserves_underscored_subscripts() {
            assert_eq!(
                render("$$ a_1 x_1 = b $$"),
                "<pre><code class=\"language-math\">a_1 x_1 = b\n</code></pre>\n"
            );
        }

        #[test]
        fn bare_single_dollar_pair_is_not_math() {
            assert_eq!(render("$x + y$"), "<p>$x + y$</p>\n");
        }

        #[test]
        fn dollar_amounts_in_prose_are_not_math() {
            assert_eq!(
                render("a coffee is $3 and a refill is $2"),
                "<p>a coffee is $3 and a refill is $2</p>\n"
            );
        }

        #[test]
        fn backtick_dollar_span_renders_as_inline_math() {
            assert_eq!(
                render("The area is $`\\pi r^2`$ exactly."),
                "<p>The area is <code class=\"language-math-inline\">\\pi r^2</code> exactly.</p>\n"
            );
        }

        #[test]
        fn adjacent_inline_math_spans_both_render() {
            assert_eq!(
                render("radius $`r`$ is $`\\pi r^2`$ area"),
                "<p>radius <code class=\"language-math-inline\">r</code> is \
                 <code class=\"language-math-inline\">\\pi r^2</code> area</p>\n"
            );
        }

        #[test]
        fn inline_math_content_is_escaped() {
            assert_eq!(
                render("$`a < b & c`$"),
                "<p><code class=\"language-math-inline\">a &lt; b &amp; c</code></p>\n"
            );
        }

        #[test]
        fn spaced_dollar_backtick_is_not_inline_math() {
            assert_eq!(render("$ `code` $"), "<p>$ <code>code</code> $</p>\n");
        }

        #[test]
        fn plain_code_span_is_unaffected() {
            assert_eq!(render("`code`"), "<p><code>code</code></p>\n");
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

    // MARK: Frontmatter

    mod frontmatter_tests {
        use super::render;

        #[test]
        fn frontmatter_renders_as_table_before_body() {
            let html = render("---\ntitle: Hello\nauthor: Jane\n---\n\n# Heading\n");
            assert_eq!(
                html,
                "<details class=\"mud-frontmatter\"><summary>Frontmatter</summary>\
<table class=\"mud-frontmatter-table\">\
<tr><th>title</th><td>Hello</td></tr>\
<tr><th>author</th><td>Jane</td></tr>\
</table></details><h1 id=\"heading\">Heading</h1>\n"
            );
        }

        #[test]
        fn frontmatter_comment_and_bare_key_are_not_treated_as_markdown() {
            // A `#`-led comment and a key with no value, inside frontmatter,
            // must not surface as a heading or stray text in the body.
            let html = render("---\ntitle: Hello\n# a comment\nempty_value:\n---\n\nBody\n");
            assert!(!html.contains("<h1"));
            assert!(html.contains("<p>Body</p>"));
        }

        #[test]
        fn no_frontmatter_renders_body_only() {
            assert_eq!(render("# Heading"), "<h1 id=\"heading\">Heading</h1>\n");
        }

        #[test]
        fn empty_frontmatter_produces_no_details_wrapper() {
            let html = render("---\n---\n\nBody\n");
            assert!(!html.contains("<details"));
            assert!(html.contains("<p>Body</p>"));
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

#[cfg(test)]
mod change_placement_tests {
    use std::sync::Arc;

    use super::*;
    use crate::changes::Waypoint;

    /// Every deletion must be emitted by a keyed lookup during the walk;
    /// none may depend on `trailing_html`'s defensive sweep.
    fn unplaced(old: &str, new: &str) -> Vec<String> {
        let options = RenderOptions {
            waypoint: Some(Waypoint {
                old_markdown: Arc::from(old),
                show_inline_deletions: true,
                word_diff_threshold_pct: 25,
            }),
            ..RenderOptions::default()
        };
        let body = crate::changes::body(new);
        let parsed = ParsedMarkdown::new(&body);
        let numbers = number_footnotes(&parsed.events);
        let mut renderer = Renderer::new(&parsed.events, &options, &numbers);
        renderer.changes = UpChanges::build(options.waypoint.as_ref().unwrap(), new, &options);
        renderer.run();
        renderer
            .changes
            .map(|c| c.placer.unplaced().into_iter().map(String::from).collect())
            .unwrap_or_default()
    }

    #[test]
    fn every_deletion_is_placed_by_the_walk() {
        let cases = [
            ("Alpha.\n\nGone.\n", "Alpha.\n"),
            ("Gone.\n\nKeep.\n", "Keep.\n"),
            ("- A\n- B\n- C\n", "- A\n- C\n"),
            ("1. A\n2. B\n3. C\n   - x\n", "1. A\n3. C\n   - x\n"),
            ("- A\n- B\n  - x\n", "- A\n- B edited\n  - x\n"),
            ("- A\n- B\n\nAfter.\n", "- A\n\nAfter.\n"),
            (
                "| A |\n|---|\n| 1 |\n| 2 |\n| 3 |\n",
                "| A |\n|---|\n| 1 |\n| 3 |\n",
            ),
            ("| A |\n|---|\n| 1 |\n| 2 |\n", "| A |\n|---|\n| 1 |\n"),
            (
                "Intro.\n\n| A |\n|---|\n| 1 |\n\nTail.\n",
                "Intro.\n\nTail.\n",
            ),
            ("Gone.\n\n| A |\n|---|\n| 1 |\n", "| A |\n|---|\n| 1 |\n"),
            (
                "| A |\n|---|\n| 1 |\n\nGone.\n\nKeep.\n",
                "| A |\n|---|\n| 1 |\n| 2 |\n\nKeep.\n",
            ),
            (
                "> [!NOTE]\n> Old.\n\nKeep.\n",
                "> [!NOTE]\n> New text.\n\nKeep.\n",
            ),
            ("> Status: Planning\n", "> Status: Underway\n"),
            ("Gone.\n\n```\nkeep\nold\n```\n", "```\nkeep\nnew\n```\n"),
            ("Alpha.\n\n---\n\nBeta.\n", "Alpha.\n\nBeta.\n"),
            ("Keep.\n\n<div>\nraw\n</div>\n\nTail.\n", "Keep.\n\nTail.\n"),
            ("- [ ] a\n\n  para\n- b\n", "- b\n"),
            (
                "Intro.\n\n$$ x = 1 $$\n\nTail.\n",
                "Intro.\n\n$$ x = 2 $$\n\nTail.\n",
            ),
        ];
        for (old, new) in cases {
            assert_eq!(
                unplaced(old, new),
                Vec::<String>::new(),
                "{old:?} -> {new:?}"
            );
        }
    }

    /// The same over mechanically edited `docs/examples` documents.
    #[test]
    fn every_deletion_is_placed_in_real_documents() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples");
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != "md") {
                continue;
            }
            let md = std::fs::read_to_string(&path).unwrap();
            let edited: Vec<String> = md
                .lines()
                .enumerate()
                .filter(|(i, _)| i % 7 != 6)
                .map(|(i, line)| {
                    if i % 4 == 3 && !line.trim().is_empty() {
                        format!("{line} edited")
                    } else {
                        line.to_string()
                    }
                })
                .collect();
            let edited = edited.join("\n");
            assert_eq!(unplaced(&md, &edited), Vec::<String>::new(), "{path:?}");
            assert_eq!(unplaced(&edited, &md), Vec::<String>::new(), "{path:?}");
        }
    }
}

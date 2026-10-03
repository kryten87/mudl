//! Leaf-block collection: `pulldown-cmark` events → [`LeafBlock`]s
//! (`LeafBlockCollector` in `mud`'s `BlockMatcher.swift`;
//! `_working/DIFF-IMPLEMENTATION.md` §8.1).
//!
//! The diff engine (`mudl-diff`) matches *leaf blocks*: the smallest blocks
//! a reader sees change. This module flattens one parse into them, in
//! document order.
//!
//! # The cmark → pulldown-cmark mapping
//!
//! `mud` walks a cmark tree; mudl walks pulldown's flat event stream, whose
//! shape differs in a few places that matter here:
//!
//! | Construct | Leaf? | Note |
//! | --- | --- | --- |
//! | Paragraph, Heading, CodeBlock, Rule, HtmlBlock | leaf | |
//! | BlockQuote, Table, List | descend | an alert is a blockquote: its paragraph is the leaf |
//! | `TableHead` | leaf (`TableHead`) | pulldown has no row inside the head; cells sit directly under it |
//! | `TableRow` | leaf (`TableRow`) | |
//! | `Item` with only inline content | leaf (`ListItem`) | a *tight* item has no `Paragraph` events |
//! | `Item` with exactly one `Paragraph` | leaf (`ListItem`) | a *loose* item |
//! | any other `Item` | descend | each inline run directly under the item becomes a `Paragraph` leaf |
//! | `$$…$$` paragraph | leaf, `Verbatim` | no inline text, so no word spans |
//! | ```` ```math ```` | leaf, `CodeBlock` | the plan never pairs `math` code |
//! | `FootnoteDefinition` | per [`DefinitionPolicy`] | comment definitions are always skipped |
//!
//! An empty item (`-` alone) yields no leaf, as in `mud` (cmark gives it no
//! children, so it is not "simple" and descending finds nothing).
//!
//! # Coordinates
//!
//! The collector works on whatever text was parsed. Change tracking always
//! parses the **frontmatter-stripped body** ([`super::body`]), so:
//!
//! - [`LeafBlock::byte_range`] is the block's `Start` event range in the
//!   body, **untrimmed** — exactly the range a renderer walking the same
//!   parse sees on that `Start`, so it can be used as a lookup key
//!   directly. The exception is a list item's inline-run leaf (see below),
//!   which has no `Start` event of its own.
//! - [`LeafBlock::source_line`] is 1-based **within the body**. Down mode
//!   numbers lines of the whole file, so add [`super::body_line_offset`]
//!   to get a file line.
//! - [`LeafBlock::source_text`] is the whole source lines the block touches
//!   (first line from column 1, so a nested paragraph keeps its `- ` or
//!   `> ` prefix, as in `mud`'s `extractSourceText`), with trailing blank
//!   lines dropped and a trailing `\r` stripped from each line.
//!
//! # Inline-run leaves
//!
//! When a list item is not simple (it holds a nested list, a code block,
//! several paragraphs, …), cmark gives it a leading `paragraph` child, and
//! `mud` collects that paragraph. In a tight list pulldown emits the text
//! directly under `Item` with no `Paragraph` wrapper, so the collector
//! synthesises the leaf: tag [`LeafTag::Paragraph`], kind
//! [`BlockKind::Prose`], `byte_range` from the run's first event's start to
//! its last event's end. The Up renderer will find no `<p>` for it (a tight
//! list renders bare text) and has to attach its attributes to the `<li>`.
//!
//! As in `mud`, such a paragraph fingerprints as prose *including* the list
//! marker on its first line, so renumbering an ordered item that has a
//! nested list *is* a change (only simple ordered items ignore the marker).

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Tag};

use mudl_diff::block::{BlockKind, DefinitionPolicy, LeafBlock, LeafTag};
use mudl_diff::is_comment_label;

use crate::parse::ParsedMarkdown;

/// Flattens `parsed` into leaf blocks under `policy`, in document order.
///
/// Under [`DefinitionPolicy::DescendPlainFootnotes`] the result is
/// stable-sorted by `source_line`, as `mud` does to undo cmark's relocation
/// of definitions. pulldown-cmark leaves definitions in place, so the sort
/// is a no-op in practice; it is kept for parity.
pub fn leaf_blocks(parsed: &ParsedMarkdown, policy: DefinitionPolicy) -> Vec<LeafBlock> {
    let mut collector = Collector::new(parsed, policy);
    let mut i = 0;
    while i < collector.events.len() {
        i = collector.block(i, false);
    }
    let mut blocks = collector.blocks;
    if policy == DefinitionPolicy::DescendPlainFootnotes {
        blocks.sort_by_key(|b| b.source_line);
    }
    blocks
}

struct Collector<'p, 'a> {
    source: &'a str,
    events: &'p [(Event<'a>, Range<usize>)],
    /// `close[i]` = index of the `End` matching the `Start` at `i` (or `i`
    /// itself for a non-`Start` event).
    close: Vec<usize>,
    /// Byte offset of the start of each line.
    line_starts: Vec<usize>,
    policy: DefinitionPolicy,
    blocks: Vec<LeafBlock>,
}

impl<'p, 'a> Collector<'p, 'a> {
    fn new(parsed: &'p ParsedMarkdown<'a>, policy: DefinitionPolicy) -> Self {
        let events = parsed.events.as_slice();
        let mut close: Vec<usize> = (0..events.len()).collect();
        let mut stack = Vec::new();
        for (i, (event, _)) in events.iter().enumerate() {
            match event {
                Event::Start(_) => stack.push(i),
                Event::End(_) => {
                    if let Some(open) = stack.pop() {
                        close[open] = i;
                    }
                }
                _ => {}
            }
        }
        // Unbalanced (never produced by pulldown-cmark): close at the end.
        for open in stack {
            close[open] = events.len().saturating_sub(1);
        }
        let source = parsed.source;
        let line_starts = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Collector {
            source,
            events,
            close,
            line_starts,
            policy,
            blocks: Vec::new(),
        }
    }

    /// Processes the block-level event at `i` (a `Start` and its whole
    /// subtree, or a single leaf event) and returns the index just past
    /// it. `ordered` is true when `i` is an item of an ordered list.
    fn block(&mut self, i: usize, ordered: bool) -> usize {
        let events = self.events;
        let (event, range) = &events[i];
        let end = self.close[i];
        let next = end + 1;
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    self.paragraph(i, range.clone());
                    next
                }
                Tag::Heading { level, .. } => {
                    let inline = self.inline_text(i + 1, end);
                    self.push(
                        BlockKind::Prose,
                        LeafTag::Heading(*level as u8),
                        range.clone(),
                        |b| b.with_inline_text(inline),
                    );
                    next
                }
                Tag::CodeBlock(kind) => {
                    let language = match kind {
                        CodeBlockKind::Fenced(info) => Some(info.to_string()),
                        CodeBlockKind::Indented => None,
                    };
                    let code: String = self.events[i + 1..end]
                        .iter()
                        .filter_map(|(e, _)| match e {
                            Event::Text(t) => Some(t.as_ref()),
                            _ => None,
                        })
                        .collect();
                    self.push(
                        BlockKind::CodeBlock,
                        LeafTag::CodeBlock,
                        range.clone(),
                        |b| b.with_language(language).with_code(code),
                    );
                    next
                }
                Tag::HtmlBlock => {
                    self.push(
                        BlockKind::Verbatim,
                        LeafTag::HtmlBlock,
                        range.clone(),
                        |b| b,
                    );
                    next
                }
                Tag::TableHead | Tag::TableRow => {
                    let leaf_tag = if matches!(tag, Tag::TableHead) {
                        LeafTag::TableHead
                    } else {
                        LeafTag::TableRow
                    };
                    let inline = self.inline_text(i + 1, end);
                    self.push(BlockKind::TableRow, leaf_tag, range.clone(), |b| {
                        b.with_inline_text(inline)
                    });
                    next
                }
                Tag::Item => {
                    self.item(i, ordered);
                    next
                }
                Tag::List(start) => {
                    self.descend(i, start.is_some());
                    next
                }
                Tag::FootnoteDefinition(label) => {
                    // `visitFootnoteDefinition`: comments are always
                    // invisible; plain definitions only under Descend.
                    if self.policy == DefinitionPolicy::DescendPlainFootnotes
                        && !is_comment_label(label)
                    {
                        self.descend(i, false);
                    }
                    next
                }
                // BlockQuote (alerts included), Table, and the containers
                // `parser_options` never enables: descend.
                _ => {
                    self.descend(i, false);
                    next
                }
            },
            Event::Rule => {
                self.push(
                    BlockKind::Verbatim,
                    LeafTag::ThematicBreak,
                    range.clone(),
                    |b| b,
                );
                next
            }
            // Stray inline or `End` events at block level: nothing to collect.
            _ => next,
        }
    }

    /// Walks the children of the container `Start` at `i`.
    fn descend(&mut self, i: usize, ordered: bool) {
        let end = self.close[i];
        let mut j = i + 1;
        while j < end {
            j = self.block(j, ordered);
        }
    }

    /// A paragraph leaf: prose, or verbatim when it is display math (the
    /// renderer's `take_display_math_paragraph` test).
    fn paragraph(&mut self, i: usize, range: Range<usize>) {
        let end = self.close[i];
        if is_display_math(&self.events[i + 1..end]) {
            self.push(BlockKind::Verbatim, LeafTag::Paragraph, range, |b| b);
        } else {
            let inline = self.inline_text(i + 1, end);
            self.push(BlockKind::Prose, LeafTag::Paragraph, range, |b| {
                b.with_inline_text(inline)
            });
        }
    }

    /// `visitListItem`: a simple item (only inline content, or exactly one
    /// paragraph) is one leaf; anything else descends.
    fn item(&mut self, i: usize, ordered: bool) {
        let events = self.events;
        let end = self.close[i];
        let item_range = events[i].1.clone();

        // Partition the item's direct children into inline runs and blocks.
        let mut runs: Vec<Range<usize>> = Vec::new(); // event index ranges
        let mut blocks: Vec<usize> = Vec::new(); // event indices
        let mut j = i + 1;
        while j < end {
            let (event, _) = &events[j];
            let after = self.close[j] + 1;
            if is_inline(event) {
                match runs.last_mut() {
                    Some(run) if run.end == j => run.end = after,
                    _ => runs.push(j..after),
                }
            } else {
                blocks.push(j);
            }
            j = after;
        }

        let kind = if ordered {
            BlockKind::OrderedListItem
        } else {
            BlockKind::Prose
        };
        let single_paragraph = runs.is_empty()
            && blocks.len() == 1
            && matches!(self.events[blocks[0]].0, Event::Start(Tag::Paragraph));

        if blocks.is_empty() && runs.len() == 1 {
            // Tight simple item.
            let inline = self.inline_text(runs[0].start, runs[0].end);
            self.push(kind, LeafTag::ListItem, item_range, |b| {
                b.with_inline_text(inline)
            });
        } else if single_paragraph {
            // Loose simple item.
            let p = blocks[0];
            let inline = self.inline_text(p + 1, self.close[p]);
            self.push(kind, LeafTag::ListItem, item_range, |b| {
                b.with_inline_text(inline)
            });
        } else {
            // Complex (or empty) item: every child in order, each inline
            // run standing in for cmark's implicit paragraph.
            let mut runs = runs.into_iter().peekable();
            let mut blocks = blocks.into_iter().peekable();
            loop {
                let take_run = match (runs.peek(), blocks.peek()) {
                    (Some(r), Some(&b)) => r.start < b,
                    (Some(_), None) => true,
                    (None, Some(_)) => false,
                    (None, None) => break,
                };
                if take_run {
                    let run = runs.next().expect("peeked");
                    self.inline_run(run);
                } else {
                    let b = blocks.next().expect("peeked");
                    self.block(b, false);
                }
            }
        }
    }

    /// A paragraph leaf for a run of inline events sitting directly under
    /// an item (no `Paragraph` event). Its range spans the run's events.
    fn inline_run(&mut self, run: Range<usize>) {
        let start = self.events[run.start].1.start;
        let end = self.events[run.end - 1].1.end;
        let inline = self.inline_text(run.start, run.end);
        self.push(BlockKind::Prose, LeafTag::Paragraph, start..end, |b| {
            b.with_inline_text(inline)
        });
    }

    /// The inline character stream of events `lo..hi`, as the Up renderer
    /// consumes it (`WordDiff.inlineText`): `Text` and `Code` literals
    /// as-is, soft break → `" "`, hard break → `"\n"`. Containers
    /// (emphasis, links, images, table cells) are transparent, so an
    /// image contributes its alt text; footnote references, inline HTML
    /// and task-list markers contribute nothing.
    fn inline_text(&self, lo: usize, hi: usize) -> String {
        let mut out = String::new();
        for (event, _) in &self.events[lo..hi] {
            match event {
                Event::Text(t) | Event::Code(t) => out.push_str(t),
                Event::SoftBreak => out.push(' '),
                Event::HardBreak => out.push('\n'),
                _ => {}
            }
        }
        out
    }

    /// Appends a leaf for `range`, letting `finish` set the optional fields.
    fn push(
        &mut self,
        kind: BlockKind,
        tag: LeafTag,
        range: Range<usize>,
        finish: impl FnOnce(LeafBlock) -> LeafBlock,
    ) {
        let source_line = self.line_of(range.start);
        let source_text = self.source_text(&range);
        let leaf = LeafBlock::new(kind, tag, source_line, range, source_text);
        self.blocks.push(finish(leaf));
    }

    /// 1-based line containing byte `offset`.
    fn line_of(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&s| s <= offset).max(1)
    }

    /// Whole lines touched by `range`, trailing blank lines dropped
    /// (`extractSourceText`).
    fn source_text(&self, range: &Range<usize>) -> String {
        let first = self.line_of(range.start);
        let last_byte = if range.end > range.start {
            range.end - 1
        } else {
            range.start
        };
        let last = self.line_of(last_byte).max(first);
        let mut lines: Vec<&str> = (first..=last)
            .filter_map(|n| self.line(n))
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect();
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Line `n` (1-based) without its terminator.
    fn line(&self, n: usize) -> Option<&'a str> {
        let start = *self.line_starts.get(n - 1)?;
        if start > self.source.len() {
            return None;
        }
        let end = self
            .line_starts
            .get(n)
            .map_or(self.source.len(), |&next| next - 1);
        Some(&self.source[start..end])
    }
}

/// True for events that live inside a paragraph (inline content).
pub(crate) fn is_inline(event: &Event) -> bool {
    match event {
        Event::Start(tag) => matches!(
            tag,
            Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Superscript
                | Tag::Subscript
                | Tag::Link { .. }
                | Tag::Image { .. }
        ),
        Event::Text(_)
        | Event::Code(_)
        | Event::InlineMath(_)
        | Event::DisplayMath(_)
        | Event::InlineHtml(_)
        | Event::FootnoteReference(_)
        | Event::SoftBreak
        | Event::HardBreak
        | Event::TaskListMarker(_) => true,
        Event::End(_) | Event::Html(_) | Event::Rule => false,
    }
}

/// The renderer's display-math test (`render::take_display_math_paragraph`):
/// the paragraph's plain text (text and code literals, breaks as spaces),
/// trimmed, is longer than 4 bytes and fenced by `$$` at both ends.
pub(crate) fn is_display_math(paragraph_events: &[(Event, Range<usize>)]) -> bool {
    let mut text = String::new();
    for (event, _) in paragraph_events {
        match event {
            Event::Text(t) | Event::Code(t) => text.push_str(t),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            _ => {}
        }
    }
    let trimmed = text.trim();
    trimmed.len() > 4 && trimmed.starts_with("$$") && trimmed.ends_with("$$")
}

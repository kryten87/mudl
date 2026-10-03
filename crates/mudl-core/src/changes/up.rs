//! Up-mode change overlay: the bridge between a [`ChangePlan`] and the
//! Up renderer (`mud`'s `DiffContext.swift` and `WordSpanEmitter.swift`;
//! `_working/DIFF-IMPLEMENTATION.md` §9.2).
//!
//! [`UpChanges`] projects a plan into lookups keyed by a block's **byte
//! range** in the new body — the range of the block's `Start` event, which
//! the renderer sees as it walks the same parse (never node identity: a
//! cached plan may come from an earlier, textually identical parse).
//!
//! - Inserted blocks get native-element attributes ([`ChangeAttrs`]).
//! - Paired insertions carry word spans, emitted through a
//!   [`WordSpanEmitter`] in step with the renderer's character stream.
//! - Paired code blocks carry their per-line diff ([`CodeLine`]s).
//! - Deleted blocks are pre-rendered from the **old** document and handed
//!   to the [`Placer`], which emits each exactly once.
//!
//! # Markup contract (consumed by `mud-changes.js` / `mud-changes.css`)
//!
//! ```html
//! <p class="mud-change-ins" data-change-id="change-2" data-group-id="group-1"
//!    data-group-type="mix" data-group-index="1">…</p>
//! <p class="mud-change-del" data-change-id="change-1" …>…</p>
//! ```
//!
//! `data-group-index` appears only on a group's first (or sole) member.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use mudl_diff::block::{BlockKind, DefinitionPolicy};
use mudl_diff::code::CodeLine;
use mudl_diff::plan::{ChangePlan, InsertionSlot};
use mudl_diff::word::WordSpan;

use super::placer::{Placer, RenderedDeletion};
use super::{body, plan, Waypoint};
use crate::emoji::replace_shortcodes;
use crate::encoding::html_escape;
use crate::options::RenderOptions;
use crate::parse::ParsedMarkdown;
use crate::render::{number_footnotes, render_deletion};

/// The class every inserted block carries.
pub const INS_CLASS: &str = "mud-change-ins";
/// The class every deleted block carries.
pub const DEL_CLASS: &str = "mud-change-del";

/// The change attributes of one element: its class and the `data-*`
/// attribute string (leading space included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeAttrs {
    pub class: &'static str,
    pub data: String,
}

/// ` data-change-id="…"` plus the group attributes for `change_id`
/// (`data-group-id`, `data-group-type`, and `data-group-index` on a
/// group's first or sole member).
pub fn data_attrs(plan: &ChangePlan, change_id: &str) -> String {
    let mut data = format!(" data-change-id=\"{}\"", html_escape(change_id));
    if let Some(info) = plan.groups.get(change_id) {
        data.push_str(&format!(
            " data-group-id=\"{}\" data-group-type=\"{}\"",
            html_escape(&info.group_id),
            info.ty.as_str()
        ));
        if info.pos.shows_badge() {
            data.push_str(&format!(" data-group-index=\"{}\"", info.index));
        }
    }
    data
}

/// An inserted block's change ID and (for a significant paired edit) its
/// word spans.
#[derive(Debug, Clone)]
struct Annotation {
    change_id: String,
    word_spans: Option<Vec<WordSpan>>,
}

/// The Up-mode projection of one plan (`DiffContext`).
pub(crate) struct UpChanges {
    plan: Arc<ChangePlan>,
    annotations: HashMap<Range<usize>, Annotation>,
    code_diffs: HashMap<Range<usize>, Vec<CodeLine>>,
    pub(crate) placer: Placer,
    pub(crate) show_inline_deletions: bool,
}

impl UpChanges {
    /// Diffs `waypoint` against `new_markdown` (whole files) under the Up
    /// policy and pre-renders every deletion. `None` when nothing changed,
    /// so an unchanged document renders byte-identically to a plain one.
    pub(crate) fn build(
        waypoint: &Waypoint,
        new_markdown: &str,
        options: &RenderOptions,
    ) -> Option<UpChanges> {
        let plan = plan(
            &waypoint.old_markdown,
            new_markdown,
            DefinitionPolicy::SkipAll,
            waypoint.threshold(),
        );
        if plan.is_empty() {
            return None;
        }

        let old_body = body(&waypoint.old_markdown);
        let old = ParsedMarkdown::new(&old_body);
        let old_numbers = number_footnotes(&old.events);

        let mut annotations = HashMap::new();
        let mut code_diffs = HashMap::new();
        let mut placer = Placer::default();

        for gap in &plan.gaps {
            for slot in &gap.insertion_slots {
                match slot {
                    InsertionSlot::Block(change) => {
                        annotations.insert(
                            change.block.byte_range.clone(),
                            Annotation {
                                change_id: change.change_id.clone(),
                                word_spans: plan.word_spans.get(&change.change_id).cloned(),
                            },
                        );
                    }
                    InsertionSlot::CodeBlockPair(pair) => {
                        code_diffs
                            .insert(pair.insertion.block.byte_range.clone(), pair.lines.clone());
                    }
                }
            }

            if gap.deletions.is_empty() {
                continue;
            }
            let rendered: Vec<usize> = gap
                .deletions
                .iter()
                .map(|del| {
                    let spans = plan.word_spans.get(&del.change_id).map(Vec::as_slice);
                    let body = render_deletion(&old, &old_numbers, options, &del.block, spans);
                    placer.add(RenderedDeletion {
                        change_id: del.change_id.clone(),
                        tag: body.tag,
                        html: body.html,
                        extra_class: body.extra_class,
                        ordered: del.block.kind == BlockKind::OrderedListItem,
                        data: data_attrs(&plan, &del.change_id),
                    })
                })
                .collect();

            // Deletions flush to the first block after them — an insertion
            // in the same gap, else the following anchor — or trail after
            // the last surviving block.
            let flush_key = gap
                .insertion_slots
                .first()
                .map(|slot| slot.insertion_block().byte_range.clone())
                .or_else(|| gap.following_anchor.as_ref().map(|a| a.byte_range.clone()));
            match flush_key {
                Some(key) => placer.key_preceding(key, &rendered),
                None => placer.key_trailing(&rendered),
            }
            // Also keyed by the anchor before the gap, so a table can
            // reclaim row deletions that follow its last surviving row.
            if let Some(anchor) = &gap.preceding_anchor {
                placer.key_following(anchor.byte_range.clone(), &rendered);
            }
        }

        Some(UpChanges {
            plan,
            annotations,
            code_diffs,
            placer,
            show_inline_deletions: waypoint.show_inline_deletions,
        })
    }

    /// The insertion attributes of the block at `range`, if it changed.
    pub(crate) fn attrs(&self, range: &Range<usize>) -> Option<ChangeAttrs> {
        let annotation = self.annotations.get(range)?;
        Some(ChangeAttrs {
            class: INS_CLASS,
            data: data_attrs(&self.plan, &annotation.change_id),
        })
    }

    /// The word spans of the paired insertion at `range` (non-empty).
    pub(crate) fn word_spans(&self, range: &Range<usize>) -> Option<&[WordSpan]> {
        self.annotations
            .get(range)?
            .word_spans
            .as_deref()
            .filter(|s| !s.is_empty())
    }

    /// The line diff of the paired code block at `range`.
    pub(crate) fn code_diff(&self, range: &Range<usize>) -> Option<&[CodeLine]> {
        self.code_diffs.get(range).map(Vec::as_slice)
    }

    /// The `data-*` attributes of a code-line cluster.
    pub(crate) fn code_line_data(&self, line: &CodeLine) -> String {
        let mut data = String::new();
        if let Some(id) = &line.change_id {
            data.push_str(&format!(" data-change-id=\"{}\"", html_escape(id)));
        }
        if let Some(group) = &line.group_id {
            data.push_str(&format!(" data-group-id=\"{}\"", html_escape(group)));
            if let Some(info) = line
                .change_id
                .as_ref()
                .and_then(|id| self.plan.groups.get(id))
            {
                data.push_str(&format!(" data-group-type=\"{}\"", info.ty.as_str()));
            }
        }
        if let Some(index) = line.group_index {
            data.push_str(&format!(" data-group-index=\"{index}\""));
        }
        data
    }
}

// MARK: - Word spans

/// Which spans consume the renderer's characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A paired insertion (the new text): unchanged and inserted spans
    /// consume; deleted spans are emitted eagerly as `<del>` when inline
    /// deletions are shown, otherwise skipped.
    Insertion,
    /// A paired deletion (the old text): unchanged and deleted spans
    /// consume; inserted spans are always skipped.
    Deletion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Ins,
    Del,
}

impl Mark {
    fn open(self) -> &'static str {
        match self {
            Mark::Ins => "<ins>",
            Mark::Del => "<del>",
        }
    }

    fn close(self) -> &'static str {
        match self {
            Mark::Ins => "</ins>",
            Mark::Del => "</del>",
        }
    }
}

/// Emits `<ins>`/`<del>` word markers in step with the renderer's
/// character stream (`WordSpanEmitter.swift`).
///
/// The renderer reports each character run it renders (a `Text` or
/// `Code` literal, one character per break, an image's alt text); the
/// emitter owns the cursor through the block's spans, splits spans that
/// straddle a run boundary, and returns the HTML for exactly the
/// characters consumed. Counts are in `char`s on both sides: the stream
/// must equal the block's `inline_text` as the leaf collector built it.
#[derive(Debug, Clone)]
pub struct WordSpanEmitter {
    spans: Vec<WordSpan>,
    cursor: usize,
    role: Role,
    show_inline_deletions: bool,
    open: Option<Mark>,
}

impl WordSpanEmitter {
    pub fn new(spans: Vec<WordSpan>, role: Role, show_inline_deletions: bool) -> Self {
        WordSpanEmitter {
            spans,
            cursor: 0,
            role,
            show_inline_deletions,
            open: None,
        }
    }

    /// True when the span at the cursor never consumes in this role.
    fn non_consuming(&self, span: &WordSpan) -> bool {
        matches!(
            (span, self.role),
            (WordSpan::Deleted(_), Role::Insertion) | (WordSpan::Inserted(_), Role::Deletion)
        )
    }

    /// Handles a non-consuming span: emits it as `<del>` (insertion role,
    /// inline deletions shown) or drops it.
    fn flush_non_consuming(&mut self, out: &mut String) {
        let span = &self.spans[self.cursor];
        if let (WordSpan::Deleted(text), Role::Insertion) = (span, self.role) {
            if self.show_inline_deletions {
                let text = text.clone();
                self.set_mark(Some(Mark::Del), out);
                out.push_str(&escape(&text));
            }
        }
        self.cursor += 1;
    }

    /// Advances the cursor by `count` characters and returns the HTML to
    /// append. With `emit` false (breaks, image alt text) the characters
    /// are consumed silently; non-consuming spans are still handled.
    pub fn advance(&mut self, count: usize, emit: bool) -> String {
        let mut out = String::new();
        let mut remaining = count;
        while self.cursor < self.spans.len() {
            if self.non_consuming(&self.spans[self.cursor]) {
                self.flush_non_consuming(&mut out);
                continue;
            }
            if remaining == 0 {
                break;
            }
            let span = self.spans[self.cursor].clone();
            let text = span.text();
            let len = text.chars().count();
            if len <= remaining {
                if emit {
                    self.emit_span(&span, text, &mut out);
                }
                self.cursor += 1;
                remaining -= len;
            } else {
                let split = text
                    .char_indices()
                    .nth(remaining)
                    .map_or(text.len(), |(i, _)| i);
                let (head, rest) = text.split_at(split);
                if emit {
                    self.emit_span(&span, head, &mut out);
                }
                self.spans[self.cursor] = with_text(&span, rest);
                break;
            }
        }
        out
    }

    /// Skips `count` consuming characters without emitting anything
    /// (an aside's `Kind:` prefix); non-consuming spans inside the prefix
    /// are dropped.
    pub fn skip_prefix(&mut self, count: usize) {
        let mut remaining = count;
        while self.cursor < self.spans.len() && remaining > 0 {
            let span = self.spans[self.cursor].clone();
            if self.non_consuming(&span) {
                self.cursor += 1;
                continue;
            }
            let text = span.text();
            let len = text.chars().count();
            if len <= remaining {
                self.cursor += 1;
                remaining -= len;
            } else {
                let split = text
                    .char_indices()
                    .nth(remaining)
                    .map_or(text.len(), |(i, _)| i);
                self.spans[self.cursor] = with_text(&span, &text[split..]);
                remaining = 0;
            }
        }
    }

    /// Closes the open marker, if any. The renderer calls this after
    /// every run so a marker never spans its own inline tags.
    pub fn close_open_tag(&mut self) -> String {
        self.open
            .take()
            .map_or_else(String::new, |m| m.close().to_string())
    }

    /// Emits the non-consuming spans left after the last run and closes
    /// the open marker. Called once when the block ends.
    pub fn finish(&mut self) -> String {
        let mut out = String::new();
        while self.cursor < self.spans.len() && self.non_consuming(&self.spans[self.cursor]) {
            self.flush_non_consuming(&mut out);
        }
        out.push_str(&self.close_open_tag());
        out
    }

    fn set_mark(&mut self, mark: Option<Mark>, out: &mut String) {
        if mark == self.open {
            return;
        }
        if let Some(open) = self.open {
            out.push_str(open.close());
        }
        if let Some(mark) = mark {
            out.push_str(mark.open());
        }
        self.open = mark;
    }

    fn emit_span(&mut self, span: &WordSpan, text: &str, out: &mut String) {
        let mark = match span {
            WordSpan::Unchanged(_) => None,
            WordSpan::Inserted(_) => Some(Mark::Ins),
            WordSpan::Deleted(_) => Some(Mark::Del),
        };
        self.set_mark(mark, out);
        out.push_str(&escape(text));
    }
}

fn with_text(span: &WordSpan, text: &str) -> WordSpan {
    let text = text.to_string();
    match span {
        WordSpan::Unchanged(_) => WordSpan::Unchanged(text),
        WordSpan::Inserted(_) => WordSpan::Inserted(text),
        WordSpan::Deleted(_) => WordSpan::Deleted(text),
    }
}

/// Span text is raw source text: shortcodes are replaced, then escaped,
/// exactly like the renderer's plain-text path.
fn escape(text: &str) -> String {
    html_escape(&replace_shortcodes(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mudl_diff::word;
    use WordSpan::{Deleted as D, Inserted as I, Unchanged as U};

    fn s(t: &str) -> String {
        t.to_string()
    }

    fn substitution() -> Vec<WordSpan> {
        word::diff("The quick fox.", "The brown fox.")
    }

    /// Ported from `WordSpanEmitterTests.swift`.
    mod emitter_tests {
        use super::*;

        #[test]
        fn insertion_role_emits_grouped_markers() {
            let mut e = WordSpanEmitter::new(substitution(), Role::Insertion, true);
            let mut out = e.advance("The brown fox.".chars().count(), true);
            out += &e.finish();
            assert_eq!(out, "The <del>quick</del><ins>brown</ins> fox.");
        }

        #[test]
        fn insertion_role_skips_deletions_when_hidden() {
            let mut e = WordSpanEmitter::new(substitution(), Role::Insertion, false);
            let mut out = e.advance(14, true);
            out += &e.finish();
            assert_eq!(out, "The <ins>brown</ins> fox.");
        }

        #[test]
        fn deletion_role_emits_del_and_skips_insertions() {
            let mut e = WordSpanEmitter::new(substitution(), Role::Deletion, false);
            let mut out = e.advance(14, true);
            out += &e.finish();
            assert_eq!(out, "The <del>quick</del> fox.");
        }

        #[test]
        fn spans_split_across_inline_node_boundaries() {
            let spans = word::diff("notable", "remarkable");
            let mut e = WordSpanEmitter::new(spans, Role::Insertion, false);
            let pieces: Vec<String> = [2, 4, 4]
                .iter()
                .map(|&n| {
                    let mut out = e.advance(n, true);
                    out += &e.close_open_tag();
                    out
                })
                .collect();
            assert_eq!(
                pieces,
                ["<ins>re</ins>", "<ins>mark</ins>", "<ins>able</ins>"]
            );
        }

        #[test]
        fn breaks_consume_silently() {
            let spans = word::diff("alpha beta", "alpha gamma");
            let mut e = WordSpanEmitter::new(spans, Role::Insertion, false);
            let mut pieces = Vec::new();
            for (n, emit) in [(5, true), (1, false), (5, true)] {
                let mut out = e.advance(n, emit);
                out += &e.close_open_tag();
                pieces.push(out);
            }
            assert_eq!(pieces, ["alpha", "", "<ins>gamma</ins>"]);
        }

        #[test]
        fn finish_flushes_spans_after_the_last_text_node() {
            let mut shown = WordSpanEmitter::new(vec![D(s("Bye"))], Role::Insertion, true);
            assert_eq!(shown.finish(), "<del>Bye</del>");
            let mut hidden = WordSpanEmitter::new(vec![D(s("Bye"))], Role::Insertion, false);
            assert_eq!(hidden.finish(), "");
        }

        #[test]
        fn skip_prefix_consumes_silently_and_skips_non_consuming_spans() {
            let spans = vec![
                D(s("Old")),
                U(s("Note:")),
                U(s(" ")),
                I(s("new")),
                U(s(" ")),
                U(s("body")),
            ];
            let mut e = WordSpanEmitter::new(spans, Role::Insertion, true);
            e.skip_prefix("Note: ".len());
            let mut out = e.advance("new body".len(), true);
            out += &e.finish();
            assert_eq!(out, "<ins>new</ins> body");
        }

        #[test]
        fn skip_prefix_splits_a_span_midway() {
            let mut e = WordSpanEmitter::new(vec![U(s("Note: body"))], Role::Insertion, false);
            e.skip_prefix(6);
            let mut out = e.advance(4, true);
            out += &e.finish();
            assert_eq!(out, "body");
        }

        #[test]
        fn escapes_and_substitutes_shortcodes_at_emission() {
            let mut e = WordSpanEmitter::new(vec![I(s("a<b & :tada:"))], Role::Insertion, false);
            let mut out = e.advance("a<b & :tada:".chars().count(), true);
            out += &e.finish();
            assert_eq!(out, "<ins>a&lt;b &amp; \u{1F389}</ins>");
        }

        #[test]
        fn splits_on_char_boundaries_for_non_ascii() {
            let spans = vec![I(s("héllo wörld"))];
            let mut e = WordSpanEmitter::new(spans, Role::Insertion, false);
            let mut out = e.advance(2, true);
            out += &e.close_open_tag();
            out += &e.advance(9, true);
            out += &e.finish();
            assert_eq!(out, "<ins>hé</ins><ins>llo wörld</ins>");
        }

        #[test]
        fn consuming_spans_reproduce_inline_text_exactly() {
            use crate::changes::leaves;
            use mudl_diff::block::DefinitionPolicy;
            let old = "The quick brown fox jumped over the `lazy` dog\n\
                       across :tada: **two** long lines.";
            let new = "The slow brown fox leapt over the `lazy` cat\n\
                       across :tada: **three** long lines.";
            let (o, n) = (
                leaves(old, DefinitionPolicy::SkipAll),
                leaves(new, DefinitionPolicy::SkipAll),
            );
            let (ot, nt) = (o[0].word_text(), n[0].word_text());
            let spans = word::diff(ot, nt);
            let new_side: String = spans
                .iter()
                .filter(|s| !s.is_deleted())
                .map(WordSpan::text)
                .collect();
            let old_side: String = spans
                .iter()
                .filter(|s| !s.is_inserted())
                .map(WordSpan::text)
                .collect();
            assert_eq!(new_side, nt);
            assert_eq!(old_side, ot);
        }
    }
}

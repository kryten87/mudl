//! Down-mode change markup (`mud`'s `LineDiffMap.swift` and
//! `DownHTMLVisitor.buildLayoutWithChanges`; `_working/DIFF-IMPLEMENTATION.md`
//! §9.3).
//!
//! Down mode shows the raw source, one row per line. With a waypoint,
//! changed lines of the new document are marked and deleted lines of the
//! old document are put back inline:
//!
//! ```html
//! <div class="line dl-ins" data-line="7" data-change-id="change-3" data-word-ins="4-9,15-18">…</div>
//! <div class="line dl-del" data-change-id="change-2" data-word-del="0-3">old source…</div>
//! ```
//!
//! - Deleted rows carry **no** `data-line` (the gutter shows an en dash).
//! - Word markers are **not** emitted inline: `mud-down.js` replaces each
//!   row's content with highlighted HTML, which would wipe them. Instead
//!   `data-word-ins` / `data-word-del` list `start-end` ranges in **UTF-16
//!   code units** of the row's text (so they index JS strings directly),
//!   for `mud-changes.js` to wrap after highlighting. Adjacent ranges are
//!   coalesced.
//! - No group attributes: the overlays are Up-only.
//!
//! The diff runs on the frontmatter-stripped bodies under
//! [`DefinitionPolicy::DescendPlainFootnotes`]; rows number the whole file,
//! so a body line `n` is file line `n + body_line_offset`. Frontmatter rows
//! are never marked.

use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};

use mudl_diff::block::{DefinitionPolicy, LeafBlock};
use mudl_diff::line::{self, Annotation};
use mudl_diff::pairing;
use mudl_diff::plan::{ChangePlan, CodeBlockPair, InsertionSlot, Pair};
use mudl_diff::word::{self, WordSpan};

use super::{body, body_line_offset, plan, Waypoint};
use crate::encoding::html_escape;

/// Renders `new_markdown` as Down-mode rows with change markup against
/// `waypoint`. `None` when nothing changed (the caller renders plain rows).
pub fn render(waypoint: &Waypoint, new_markdown: &str) -> Option<String> {
    let threshold = waypoint.threshold();
    let plan = plan(
        &waypoint.old_markdown,
        new_markdown,
        DefinitionPolicy::DescendPlainFootnotes,
        threshold,
    );
    if plan.is_empty() {
        return None;
    }
    let map = LineDiffMap::build(&plan, threshold);
    let old_body = body(&waypoint.old_markdown);
    let old_lines: Vec<&str> = old_body.lines().collect();
    let offset = body_line_offset(new_markdown);

    let mut out = String::new();
    let mut next_group = 0;
    for (index, text) in new_markdown.lines().enumerate() {
        let file_line = index + 1;
        let body_line = file_line.checked_sub(offset).filter(|&n| n > 0);
        if let Some(body_line) = body_line {
            while let Some(group) = map.groups.get(next_group) {
                if group.before > body_line {
                    break;
                }
                map.push_deletion_group(group, &old_lines, &mut out);
                next_group += 1;
            }
        }
        match body_line.and_then(|n| map.annotations.get(&n).map(|id| (n, id))) {
            Some((body_line, change_id)) => {
                out.push_str(&format!(
                    "<div class=\"line dl-ins\" data-line=\"{file_line}\" data-change-id=\"{}\"",
                    html_escape(change_id)
                ));
                let ranges = map
                    .ins_words
                    .get(&(change_id.clone(), body_line))
                    .map(|data| data.markers(body_line))
                    .unwrap_or_default();
                push_ranges(&mut out, "data-word-ins", &ranges);
                out.push('>');
            }
            None => {
                out.push_str(&format!("<div class=\"line\" data-line=\"{file_line}\">"));
            }
        }
        out.push_str(&html_escape(text));
        out.push_str("</div>");
    }
    for group in &map.groups[next_group..] {
        map.push_deletion_group(group, &old_lines, &mut out);
    }
    Some(out)
}

fn push_ranges(out: &mut String, attr: &str, ranges: &[Range<usize>]) {
    if ranges.is_empty() {
        return;
    }
    let list: Vec<String> = ranges
        .iter()
        .map(|r| format!("{}-{}", r.start, r.end))
        .collect();
    out.push_str(&format!(" {attr}=\"{}\"", list.join(",")));
}

/// A run of old-document lines to put back, before a new-document line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeletionGroup {
    /// Emit before this new body line (1-based); `usize::MAX` = at the end.
    before: usize,
    /// Old body lines (1-based, inclusive).
    old_lines: RangeInclusive<usize>,
    change_id: String,
}

/// Word-diff data for one line of a paired block (`BlockWordData`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct WordData {
    spans: Vec<WordSpan>,
    /// The text the spans were computed over (one line, or a whole block).
    source_text: String,
    is_insertion: bool,
    /// The document line `source_text` starts on.
    start_line: usize,
}

impl WordData {
    /// The changed ranges of document line `line`, in UTF-16 code units
    /// of that line's text, adjacent ranges coalesced
    /// (`DownHTMLVisitor.wordMarkers`).
    fn markers(&self, line: usize) -> Vec<Range<usize>> {
        let lines: Vec<&str> = self.source_text.split('\n').collect();
        let Some(index) = line.checked_sub(self.start_line) else {
            return Vec::new();
        };
        let Some(this) = lines.get(index) else {
            return Vec::new();
        };
        let line_offset: usize = lines[..index].iter().map(|l| utf16_len(l) + 1).sum();
        let line_end = line_offset + utf16_len(this);

        let mut markers: Vec<Range<usize>> = Vec::new();
        let mut pos = 0;
        for span in &self.spans {
            let len = utf16_len(span.text());
            let marks = match span {
                WordSpan::Unchanged(_) => {
                    pos += len;
                    continue;
                }
                WordSpan::Inserted(_) => self.is_insertion,
                WordSpan::Deleted(_) => !self.is_insertion,
            };
            if !marks {
                continue;
            }
            let (start, end) = (pos.max(line_offset), (pos + len).min(line_end));
            if start < end {
                let range = start - line_offset..end - line_offset;
                match markers.last_mut() {
                    Some(last) if last.end == range.start => last.end = range.end,
                    _ => markers.push(range),
                }
            }
            pos += len;
        }
        markers
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The 1-based line range a leaf occupies.
fn line_range(block: &LeafBlock) -> RangeInclusive<usize> {
    let count = block.source_text.split('\n').count();
    block.source_line..=block.source_line + count - 1
}

/// The plan projected onto lines (`LineDiffMap`).
#[derive(Debug, Default)]
struct LineDiffMap {
    /// New body line → change ID of an inserted line.
    annotations: HashMap<usize, String>,
    groups: Vec<DeletionGroup>,
    del_words: HashMap<(String, usize), WordData>,
    ins_words: HashMap<(String, usize), WordData>,
}

impl LineDiffMap {
    fn build(plan: &ChangePlan, threshold: f64) -> Self {
        let mut map = LineDiffMap::default();
        for gap in &plan.gaps {
            let anchor = gap
                .following_anchor
                .as_ref()
                .map_or(usize::MAX, |a| a.source_line);

            // Paired blocks, in document order of their insertions.
            let mut pair_index = 0;
            for slot in &gap.insertion_slots {
                match slot {
                    InsertionSlot::CodeBlockPair(pair) => map.code_block_pair(pair, threshold),
                    InsertionSlot::Block(change) => {
                        if let Some(pair) = gap.pairs.get(pair_index) {
                            pair_index += 1;
                            map.line_level_pair(pair, anchor, threshold);
                        } else {
                            for line in line_range(&change.block) {
                                map.annotations.insert(line, change.change_id.clone());
                            }
                        }
                    }
                }
            }

            // Unpaired deletions: whole blocks, before the anchor.
            for del in gap.deletions.iter().skip(gap.pairs.len()) {
                map.groups.push(DeletionGroup {
                    before: anchor,
                    old_lines: line_range(&del.block),
                    change_id: del.change_id.clone(),
                });
            }
        }
        map
    }

    /// A positional pair: a line diff over the two blocks' source lines,
    /// falling back to whole-block treatment when no line matches
    /// (`processLineLevelPair`).
    fn line_level_pair(&mut self, pair: &Pair, anchor: usize, threshold: f64) {
        let (del, ins) = (&pair.deletion, &pair.insertion);
        let old_lines: Vec<&str> = del.block.source_text.split('\n').collect();
        let new_lines: Vec<&str> = ins.block.source_text.split('\n').collect();
        let Some(entries) = line::diff(&old_lines, &new_lines) else {
            self.block_level_pair(pair, threshold);
            return;
        };
        let (del_start, ins_start) = (del.block.source_line, ins.block.source_line);

        let mut idx = 0;
        while idx < entries.len() {
            if entries[idx].annotation == Annotation::Unchanged {
                idx += 1;
                continue;
            }
            let (mut gap_del, mut gap_ins) = (Vec::new(), Vec::new());
            while idx < entries.len() && entries[idx].annotation != Annotation::Unchanged {
                match entries[idx].annotation {
                    Annotation::Deleted => gap_del.push(del_start + entries[idx].source_index),
                    _ => gap_ins.push(ins_start + entries[idx].source_index),
                }
                idx += 1;
            }

            // Before the gap's first insertion, else the next unchanged
            // line, else the overall anchor.
            let before = gap_ins.first().copied().unwrap_or_else(|| {
                entries
                    .get(idx)
                    .map_or(anchor, |e| ins_start + e.source_index)
            });
            if let (Some(&first), Some(&last)) = (gap_del.first(), gap_del.last()) {
                self.groups.push(DeletionGroup {
                    before,
                    old_lines: first..=last,
                    change_id: del.change_id.clone(),
                });
            }
            for &line in &gap_ins {
                self.annotations.insert(line, ins.change_id.clone());
            }

            let del_texts: Vec<&str> = gap_del.iter().map(|&l| old_lines[l - del_start]).collect();
            let ins_texts: Vec<&str> = gap_ins.iter().map(|&l| new_lines[l - ins_start]).collect();
            for (d, i) in pairing::best_pairs(&del_texts, &ins_texts) {
                self.pair_words(
                    (&del.change_id, gap_del[d], del_texts[d]),
                    (&ins.change_id, gap_ins[i], ins_texts[i]),
                    threshold,
                );
            }
        }
    }

    /// Records word data for a best-matched (deleted, inserted) line pair
    /// when its word diff is significant.
    fn pair_words(
        &mut self,
        (del_id, del_line, del_text): (&str, usize, &str),
        (ins_id, ins_line, ins_text): (&str, usize, &str),
        threshold: f64,
    ) {
        let spans = word::diff(del_text, ins_text);
        if !word::has_significant_changes(&spans, threshold) {
            return;
        }
        self.del_words.insert(
            (del_id.to_string(), del_line),
            WordData {
                spans: spans.clone(),
                source_text: del_text.to_string(),
                is_insertion: false,
                start_line: del_line,
            },
        );
        self.ins_words.insert(
            (ins_id.to_string(), ins_line),
            WordData {
                spans,
                source_text: ins_text.to_string(),
                is_insertion: true,
                start_line: ins_line,
            },
        );
    }

    /// Whole-block fallback: every old line deleted, every new line
    /// inserted, with a block-wide word diff when significant
    /// (`emitBlockLevel`).
    fn block_level_pair(&mut self, pair: &Pair, threshold: f64) {
        let (del, ins) = (&pair.deletion, &pair.insertion);
        let ins_lines = line_range(&ins.block);
        let spans = word::diff(&del.block.source_text, &ins.block.source_text);
        if word::has_significant_changes(&spans, threshold) {
            for line in line_range(&del.block) {
                self.del_words.insert(
                    (del.change_id.clone(), line),
                    WordData {
                        spans: spans.clone(),
                        source_text: del.block.source_text.clone(),
                        is_insertion: false,
                        start_line: del.block.source_line,
                    },
                );
            }
            for line in ins_lines.clone() {
                self.ins_words.insert(
                    (ins.change_id.clone(), line),
                    WordData {
                        spans: spans.clone(),
                        source_text: ins.block.source_text.clone(),
                        is_insertion: true,
                        start_line: ins.block.source_line,
                    },
                );
            }
        }
        self.groups.push(DeletionGroup {
            before: *ins_lines.start(),
            old_lines: line_range(&del.block),
            change_id: del.change_id.clone(),
        });
        for line in ins_lines {
            self.annotations.insert(line, ins.change_id.clone());
        }
    }

    /// Projects a code-block pair's line clusters (IDs minted by the plan)
    /// onto document lines (`processCodeBlockPair`). Content starts one
    /// line below a fence.
    fn code_block_pair(&mut self, pair: &CodeBlockPair, threshold: f64) {
        let (del, ins) = (&pair.deletion, &pair.insertion);
        let del_fence = usize::from(is_fenced(&del.block));
        let ins_fence = usize::from(is_fenced(&ins.block));
        let (del_start, ins_start) = (
            del.block.source_line + del_fence,
            ins.block.source_line + ins_fence,
        );
        let del_src: Vec<&str> = del.block.source_text.split('\n').collect();
        let ins_src: Vec<&str> = ins.block.source_text.split('\n').collect();

        // (document line, code line index)
        let mut gap_del: Vec<(usize, usize)> = Vec::new();
        let mut gap_ins: Vec<(usize, usize)> = Vec::new();
        let mut gap_id: Option<String> = None;
        let (mut old_ci, mut new_ci) = (0, 0);

        let flush = |map: &mut Self,
                     gap_del: &mut Vec<(usize, usize)>,
                     gap_ins: &mut Vec<(usize, usize)>,
                     gap_id: &mut Option<String>,
                     new_ci: usize| {
            let Some(change_id) = gap_id.take() else {
                return;
            };
            if let (Some(first), Some(last)) = (gap_del.first(), gap_del.last()) {
                map.groups.push(DeletionGroup {
                    before: gap_ins.first().map_or(ins_start + new_ci, |e| e.0),
                    old_lines: first.0..=last.0,
                    change_id: change_id.clone(),
                });
            }
            for entry in gap_ins.iter() {
                map.annotations.insert(entry.0, change_id.clone());
            }
            fn source<'s>(src: &[&'s str], code: usize, fence: usize) -> Option<&'s str> {
                src.get(code + fence).copied()
            }
            let dels: Vec<(usize, &str)> = gap_del
                .iter()
                .filter_map(|&(doc, code)| source(&del_src, code, del_fence).map(|t| (doc, t)))
                .collect();
            let inss: Vec<(usize, &str)> = gap_ins
                .iter()
                .filter_map(|&(doc, code)| source(&ins_src, code, ins_fence).map(|t| (doc, t)))
                .collect();
            let del_texts: Vec<&str> = dels.iter().map(|e| e.1).collect();
            let ins_texts: Vec<&str> = inss.iter().map(|e| e.1).collect();
            for (d, i) in pairing::best_pairs(&del_texts, &ins_texts) {
                map.pair_words(
                    (&change_id, dels[d].0, dels[d].1),
                    (&change_id, inss[i].0, inss[i].1),
                    threshold,
                );
            }
            gap_del.clear();
            gap_ins.clear();
        };

        for line in &pair.lines {
            match line.annotation {
                Annotation::Unchanged => {
                    flush(self, &mut gap_del, &mut gap_ins, &mut gap_id, new_ci);
                    old_ci += 1;
                    new_ci += 1;
                }
                Annotation::Deleted => {
                    gap_del.push((del_start + old_ci, old_ci));
                    gap_id = line.change_id.clone();
                    old_ci += 1;
                }
                Annotation::Inserted => {
                    gap_ins.push((ins_start + new_ci, new_ci));
                    gap_id = line.change_id.clone();
                    new_ci += 1;
                }
            }
        }
        flush(self, &mut gap_del, &mut gap_ins, &mut gap_id, new_ci);
    }

    fn push_deletion_group(&self, group: &DeletionGroup, old_lines: &[&str], out: &mut String) {
        for line in group.old_lines.clone() {
            let text = line
                .checked_sub(1)
                .and_then(|i| old_lines.get(i))
                .copied()
                .unwrap_or("");
            out.push_str(&format!(
                "<div class=\"line dl-del\" data-change-id=\"{}\"",
                html_escape(&group.change_id)
            ));
            let ranges = self
                .del_words
                .get(&(group.change_id.clone(), line))
                .map(|data| data.markers(line))
                .unwrap_or_default();
            push_ranges(out, "data-word-del", &ranges);
            out.push('>');
            out.push_str(&html_escape(text));
            out.push_str("</div>");
        }
    }
}

/// True for a fenced code block: its first source line holds the fence.
/// (`mud` tests the first character, which misses a fence nested in a
/// list item or blockquote, whose source line starts with the marker.)
fn is_fenced(block: &LeafBlock) -> bool {
    let first = block.source_text.split('\n').next().unwrap_or("");
    first.contains("```") || first.contains("~~~")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(spans: Vec<WordSpan>, text: &str, ins: bool, start: usize) -> WordData {
        WordData {
            spans,
            source_text: text.to_string(),
            is_insertion: ins,
            start_line: start,
        }
    }

    #[test]
    fn markers_are_utf16_offsets() {
        // "é" is 1 UTF-16 unit, "😀" is 2, "漢" is 1.
        let spans = word::diff("é 😀 old 漢", "é 😀 new 漢");
        let ins = data(spans.clone(), "é 😀 new 漢", true, 1);
        assert_eq!(ins.markers(1), vec![5..8]);
        let del = data(spans, "é 😀 old 漢", false, 1);
        assert_eq!(del.markers(1), vec![5..8]);
    }

    #[test]
    fn markers_clip_to_the_requested_line() {
        let spans = word::diff("one two\nthree four", "one TWO\nthree FOUR");
        let ins = data(spans, "one TWO\nthree FOUR", true, 10);
        assert_eq!(ins.markers(10), vec![4..7]);
        assert_eq!(ins.markers(11), vec![6..10]);
        assert!(ins.markers(12).is_empty());
        assert!(ins.markers(9).is_empty());
    }

    #[test]
    fn adjacent_markers_coalesce() {
        let spans = vec![
            WordSpan::Unchanged("a ".into()),
            WordSpan::Inserted("b".into()),
            WordSpan::Inserted(" c".into()),
        ];
        assert_eq!(data(spans, "a b c", true, 1).markers(1), vec![2..5]);
    }
}

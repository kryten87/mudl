//! Placement of pre-rendered deleted blocks in the Up-mode HTML stream
//! (`mud`'s `DeletionPlacer.swift`; `_working/DIFF-IMPLEMENTATION.md` §9.2).
//!
//! [`super::up::UpChanges`] keys each deletion to a surviving block of the
//! new document (by byte range); the renderer asks for the HTML at the
//! matching point of its walk. The placer keeps every deletion emitted
//! **exactly once** as valid HTML:
//!
//! - **Preceding:** deletions are emitted just before the block they are
//!   keyed to (the gap's first insertion, else its following anchor).
//! - **Lists:** `li` deletions keyed to a list item's first child are
//!   emitted before the `<li>` opens, as siblings.
//! - **Tables:** deletions keyed to the head are hoisted above `<table>`
//!   (`tr` ones wrapped in their own `<table><tbody>`); `tr` deletions
//!   keyed to a body row are emitted inline, anything else is deferred to
//!   after `</table>`; `tr` deletions following the last surviving row are
//!   reclaimed into the table.
//! - **Trailing:** whatever is keyed to no block is emitted after the walk
//!   (before the footnote and comment sections).
//! - Outside their native parent, runs of `tr` deletions are wrapped in
//!   `<table><tbody>` (as `mud` does) and runs of `li` deletions in
//!   `<ul>`/`<ol>` (a mudl addition: `mud` emits a bare `<li>`, which is
//!   invalid HTML outside a list).
//!
//! Emitted deletions are recorded in a consumed set and filtered from
//! every later lookup.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use super::up::DEL_CLASS;

/// A deleted block, rendered from the old document, ready for injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedDeletion {
    pub change_id: String,
    /// The native element: `p`, `hN`, `li`, `tr`, `pre`, `hr`,
    /// `blockquote` (alerts), or `div` (HTML blocks, display math).
    pub tag: String,
    /// The element's inner HTML.
    pub html: String,
    /// Classes placed before `mud-change-del` (e.g. `alert alert-note`).
    pub extra_class: Option<String>,
    /// An ordered list item (wrapped in `<ol>` outside a list).
    pub ordered: bool,
    /// The `data-*` attributes (leading space included).
    pub data: String,
}

impl RenderedDeletion {
    /// The deletion as a native element with change attributes.
    pub fn to_html(&self) -> String {
        let class = match &self.extra_class {
            Some(extra) => format!("{extra} {DEL_CLASS}"),
            None => DEL_CLASS.to_string(),
        };
        let attrs = format!(" class=\"{class}\"{}", self.data);
        if self.tag == "hr" {
            return format!("<hr{attrs} />\n");
        }
        format!("<{tag}{attrs}>{}</{tag}>\n", self.html, tag = self.tag)
    }
}

/// Where a run of preceding deletions is being emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    /// Directly inside a `<ul>`/`<ol>`, before an `<li>`: `li`
    /// deletions are valid as they are.
    ListItem,
    /// Anywhere else.
    Block,
}

#[derive(Debug, Default)]
pub(crate) struct Placer {
    deletions: Vec<RenderedDeletion>,
    preceding: HashMap<Range<usize>, Vec<usize>>,
    following: HashMap<Range<usize>, Vec<usize>>,
    trailing: Vec<usize>,
    consumed: HashSet<usize>,
    deferred: Vec<usize>,
}

impl Placer {
    /// Registers a rendered deletion; returns its handle.
    pub fn add(&mut self, deletion: RenderedDeletion) -> usize {
        self.deletions.push(deletion);
        self.deletions.len() - 1
    }

    pub fn key_preceding(&mut self, key: Range<usize>, handles: &[usize]) {
        self.preceding.insert(key, handles.to_vec());
    }

    pub fn key_following(&mut self, key: Range<usize>, handles: &[usize]) {
        self.following.insert(key, handles.to_vec());
    }

    pub fn key_trailing(&mut self, handles: &[usize]) {
        self.trailing.extend_from_slice(handles);
    }

    fn unconsumed(&self, handles: Option<&Vec<usize>>) -> Vec<usize> {
        handles
            .map(|h| {
                h.iter()
                    .copied()
                    .filter(|i| !self.consumed.contains(i))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn take(&mut self, handle: usize) -> String {
        self.consumed.insert(handle);
        self.deletions[handle].to_html()
    }

    /// Deletions keyed to the block at `key`, not yet emitted.
    pub fn preceding_html(&mut self, key: &Range<usize>, context: Context) -> String {
        let handles = self.unconsumed(self.preceding.get(key));
        self.wrapped(&handles, context)
    }

    /// Deletions after the last surviving block, then (defensively) any
    /// deletion whose key the walk never reached, so none is ever lost.
    pub fn trailing_html(&mut self) -> String {
        let mut handles = self.unconsumed(Some(&self.trailing.clone()));
        let mut rest: Vec<usize> = (0..self.deletions.len())
            .filter(|i| !self.consumed.contains(i) && !handles.contains(i))
            .collect();
        handles.append(&mut rest);
        self.wrapped(&handles, Context::Block)
    }

    /// Handles never emitted by the keyed lookups (before
    /// [`Placer::trailing_html`] sweeps them up). Empty in a correct walk.
    #[cfg(test)]
    pub fn unplaced(&self) -> Vec<&str> {
        let trailing: HashSet<usize> = self.trailing.iter().copied().collect();
        (0..self.deletions.len())
            .filter(|i| !self.consumed.contains(i) && !trailing.contains(i))
            .map(|i| self.deletions[i].change_id.as_str())
            .collect()
    }

    /// `li` deletions keyed to a list item's first child, emitted before
    /// the `<li>` opens so they become siblings rather than nesting.
    pub fn list_item_html(&mut self, first_child: &Range<usize>) -> String {
        let mut out = String::new();
        for handle in self.unconsumed(self.preceding.get(first_child)) {
            if self.deletions[handle].tag == "li" {
                out.push_str(&self.take(handle));
            }
        }
        out
    }

    /// Deletions keyed to a table's head, hoisted before `<table>`:
    /// `tr` deletions (a replaced table) wrapped in their own table,
    /// others emitted directly.
    pub fn hoisted_html(&mut self, head: &Range<usize>) -> String {
        let handles = self.unconsumed(self.preceding.get(head));
        let (rows, others): (Vec<usize>, Vec<usize>) = handles
            .into_iter()
            .partition(|&h| self.deletions[h].tag == "tr");
        let mut out = self.wrapped(&others, Context::Block);
        if !rows.is_empty() {
            out.push_str("<table>\n<tbody>\n");
            for handle in rows {
                out.push_str(&self.take(handle));
            }
            out.push_str("</tbody>\n</table>\n");
        }
        out
    }

    /// Deletions keyed to a table-body row: `tr` deletions emitted as
    /// siblings, anything else deferred until after `</table>`.
    pub fn row_html(&mut self, row: &Range<usize>) -> String {
        let mut out = String::new();
        for handle in self.unconsumed(self.preceding.get(row)) {
            if self.deletions[handle].tag == "tr" {
                out.push_str(&self.take(handle));
            } else {
                self.consumed.insert(handle);
                self.deferred.push(handle);
            }
        }
        out
    }

    /// `tr` deletions following the last surviving row, reclaimed into
    /// the table.
    pub fn reclaimed_row_html(&mut self, last_row: &Range<usize>) -> String {
        let mut out = String::new();
        for handle in self.unconsumed(self.following.get(last_row)) {
            if self.deletions[handle].tag == "tr" {
                out.push_str(&self.take(handle));
            }
        }
        out
    }

    /// Emits and clears the deletions deferred from inside a table body.
    pub fn deferred_html(&mut self) -> String {
        let deferred = std::mem::take(&mut self.deferred);
        let mut out = String::new();
        for handle in deferred {
            out.push_str(&self.deletions[handle].to_html());
        }
        out
    }

    /// Emits `handles` in order, wrapping runs that would be invalid in
    /// `context`: `tr` runs in `<table><tbody>`, and `li` runs (outside a
    /// list) in `<ul>`/`<ol>`.
    fn wrapped(&mut self, handles: &[usize], context: Context) -> String {
        let mut out = String::new();
        let mut i = 0;
        while i < handles.len() {
            let del = &self.deletions[handles[i]];
            let wrapper = match (del.tag.as_str(), context) {
                ("tr", _) => Some(("<table>\n<tbody>\n", "</tbody>\n</table>\n")),
                ("li", Context::Block) if del.ordered => Some(("<ol>\n", "</ol>\n")),
                ("li", Context::Block) => Some(("<ul>\n", "</ul>\n")),
                _ => None,
            };
            let Some((open, close)) = wrapper else {
                out.push_str(&self.take(handles[i]));
                i += 1;
                continue;
            };
            let (tag, ordered) = (del.tag.clone(), del.ordered);
            out.push_str(open);
            while i < handles.len()
                && self.deletions[handles[i]].tag == tag
                && (tag == "tr" || self.deletions[handles[i]].ordered == ordered)
            {
                out.push_str(&self.take(handles[i]));
                i += 1;
            }
            out.push_str(close);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn del(id: &str, tag: &str) -> RenderedDeletion {
        RenderedDeletion {
            change_id: id.to_string(),
            tag: tag.to_string(),
            html: id.to_string(),
            extra_class: None,
            ordered: false,
            data: format!(" data-change-id=\"{id}\""),
        }
    }

    #[test]
    fn deletion_html_is_a_native_element() {
        assert_eq!(
            del("change-1", "p").to_html(),
            "<p class=\"mud-change-del\" data-change-id=\"change-1\">change-1</p>\n"
        );
        assert_eq!(
            del("change-1", "hr").to_html(),
            "<hr class=\"mud-change-del\" data-change-id=\"change-1\" />\n"
        );
        let mut alert = del("change-2", "blockquote");
        alert.extra_class = Some("alert alert-note".into());
        assert!(alert
            .to_html()
            .starts_with("<blockquote class=\"alert alert-note mud-change-del\""));
    }

    #[test]
    fn preceding_emits_exactly_once() {
        let mut p = Placer::default();
        let a = p.add(del("change-1", "p"));
        p.key_preceding(0..5, &[a]);
        assert!(p
            .preceding_html(&(0..5), Context::Block)
            .contains("change-1"));
        assert_eq!(p.preceding_html(&(0..5), Context::Block), "");
        assert_eq!(p.trailing_html(), "");
    }

    #[test]
    fn rows_outside_a_table_are_wrapped() {
        let mut p = Placer::default();
        let a = p.add(del("change-1", "tr"));
        let b = p.add(del("change-2", "tr"));
        let c = p.add(del("change-3", "p"));
        p.key_trailing(&[a, b, c]);
        let html = p.trailing_html();
        assert!(html.starts_with("<table>\n<tbody>\n<tr"));
        assert_eq!(html.matches("<table>").count(), 1);
        assert!(html.ends_with(
            "</table>\n<p class=\"mud-change-del\" data-change-id=\"change-3\">change-3</p>\n"
        ));
    }

    #[test]
    fn items_are_wrapped_outside_a_list_only() {
        let mut p = Placer::default();
        let a = p.add(del("change-1", "li"));
        p.key_preceding(0..5, &[a]);
        let html = p.preceding_html(&(0..5), Context::Block);
        assert!(html.starts_with("<ul>\n<li class=\"mud-change-del\""));

        let mut p = Placer::default();
        let mut ordered = del("change-1", "li");
        ordered.ordered = true;
        let a = p.add(ordered);
        p.key_preceding(0..5, &[a]);
        assert!(p
            .preceding_html(&(0..5), Context::Block)
            .starts_with("<ol>\n<li"));

        let mut p = Placer::default();
        let a = p.add(del("change-1", "li"));
        p.key_preceding(0..5, &[a]);
        assert!(p
            .preceding_html(&(0..5), Context::ListItem)
            .starts_with("<li class=\"mud-change-del\""));
    }

    #[test]
    fn row_deletions_defer_non_rows() {
        let mut p = Placer::default();
        let a = p.add(del("change-1", "tr"));
        let b = p.add(del("change-2", "p"));
        p.key_preceding(3..9, &[a, b]);
        let rows = p.row_html(&(3..9));
        assert!(rows.contains("change-1") && !rows.contains("change-2"));
        assert!(p.deferred_html().contains("change-2"));
        assert_eq!(p.trailing_html(), "");
    }

    #[test]
    fn reclaimed_rows_are_not_emitted_again() {
        let mut p = Placer::default();
        let a = p.add(del("change-1", "tr"));
        p.key_trailing(&[a]);
        p.key_following(0..4, &[a]);
        assert!(p.reclaimed_row_html(&(0..4)).contains("change-1"));
        assert_eq!(p.trailing_html(), "");
    }

    #[test]
    fn unkeyed_leftovers_are_swept_into_trailing() {
        let mut p = Placer::default();
        let a = p.add(del("change-1", "p"));
        p.key_preceding(10..20, &[a]);
        assert_eq!(p.unplaced(), vec!["change-1"]);
        assert!(p.trailing_html().contains("change-1"));
    }
}

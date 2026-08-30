//! Outline sidebar (Phase 10.3 of `docs/IMPLEMENTATION-PLAN.md`): a
//! `gtk::TreeView` fed from Phase 9.3's `Vec<OutlineNode>`, and the pure
//! logic deciding what to run in the WebView when a row is activated.
//!
//! Folder-mode (Phase 9.1's `Tree`, toggled by the `sidebar_pane`
//! preference) isn't wired here yet: `mudl-gui` doesn't have a
//! directory-open entry point or preferences loading wired up yet either,
//! so there's nothing for it to switch between today. This module only
//! builds the outline pane, which is reachable now.

use gtk::glib;
use gtk::prelude::*;
use mudl_core::changes::GroupSummary;
use mudl_core::headings::{OutlineHeading, OutlineTextSegment};
use mudl_core::outline::OutlineNode;
use mudl_core::template::js_string_literal;
use mudl_diff::plan::GroupType;
use mudl_server::routes::Mode;

const COLUMN_LABEL: u32 = 0;
const COLUMN_SLUG: u32 = 1;
const COLUMN_LINE: u32 = 2;

/// Builds a `gtk::TreeView` listing `nodes`' outline structure, one row per
/// heading, nested to match. Each row carries its heading's slug and
/// source line number as hidden columns (not shown to the user, but read
/// back on activation — see [`navigation_script`]).
pub fn build_outline_tree_view(nodes: &[OutlineNode]) -> gtk::TreeView {
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::U64]);
    insert_nodes(&store, None, nodes);

    let tree_view = gtk::TreeView::new();
    tree_view.set_model(Some(&store));
    tree_view.set_headers_visible(false);

    let column = gtk::TreeViewColumn::new();
    let renderer = gtk::CellRendererText::new();
    gtk::prelude::CellLayoutExt::pack_start(&column, &renderer, true);
    gtk::prelude::CellLayoutExt::add_attribute(&column, &renderer, "text", COLUMN_LABEL as i32);
    tree_view.append_column(&column);

    tree_view
}

fn insert_nodes(store: &gtk::TreeStore, parent: Option<&gtk::TreeIter>, nodes: &[OutlineNode]) {
    for node in nodes {
        let label = plain_label(&node.heading);
        let iter = store.insert_with_values(
            parent,
            None,
            &[
                (COLUMN_LABEL, &label),
                (COLUMN_SLUG, &node.heading.id),
                (COLUMN_LINE, &(node.heading.line as u64)),
            ],
        );
        insert_nodes(store, Some(&iter), &node.children);
    }
}

/// Flattens a heading's inline segments into the plain text a TreeView row
/// shows (code spans included, no backticks) — the same text
/// `mudl_core::headings::extract_headings` slugged the heading from.
fn plain_label(heading: &OutlineHeading) -> String {
    heading
        .segments
        .iter()
        .map(|segment| match segment {
            OutlineTextSegment::Plain(s) | OutlineTextSegment::Code(s) => s.as_str(),
        })
        .collect()
}

/// Reads a row's slug/line columns back out of `model` at `path`, for a
/// `row-activated` handler to pass to [`navigation_script`].
pub fn slug_and_line_at(model: &gtk::TreeStore, path: &gtk::TreePath) -> Option<(String, u64)> {
    let iter = model.iter(path)?;
    let slug = model
        .value(&iter, COLUMN_SLUG as i32)
        .get::<String>()
        .ok()?;
    let line = model.value(&iter, COLUMN_LINE as i32).get::<u64>().ok()?;
    Some((slug, line))
}

/// The JS to run in the WebView when a sidebar row is activated: jump to
/// the heading's `id="<slug>"` anchor in Up mode (the same slug
/// `render_up` assigns it — see `headings::heading_ids_match_render_up_ids`),
/// or to its `.line[data-line="<line>"]` element in Down mode, where
/// there's no `id=` anchor to jump to instead.
pub fn navigation_script(mode: Mode, slug: &str, line: u64) -> String {
    match mode {
        Mode::Up => format!(
            "(function() {{ var el = document.getElementById({slug}); \
             if (el) el.scrollIntoView({{behavior: \"smooth\", block: \"start\"}}); }})();",
            slug = js_string_literal(slug)
        ),
        Mode::Down => format!(
            "(function() {{ var el = document.querySelector('.line[data-line=\"{line}\"]'); \
             if (el) el.scrollIntoView({{behavior: \"smooth\", block: \"start\"}}); }})();"
        ),
    }
}

// MARK: - Changes pane (Phase 13.9)

const COLUMN_GROUP_ID: u32 = 0;

/// Builds the empty "Changes" pane list view — populated later by
/// [`populate_changes_list`] once a waypoint is picked from the toolbar's
/// "Changes since…" popover (there's nothing to list before that).
pub fn build_changes_list_view() -> gtk::TreeView {
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    let tree_view = gtk::TreeView::new();
    tree_view.set_model(Some(&store));
    tree_view.set_headers_visible(false);

    let column = gtk::TreeViewColumn::new();
    let renderer = gtk::CellRendererText::new();
    gtk::prelude::CellLayoutExt::pack_start(&column, &renderer, true);
    gtk::prelude::CellLayoutExt::add_attribute(&column, &renderer, "text", 1);
    tree_view.append_column(&column);

    tree_view
}

/// Replaces the changes pane's rows with `summaries`, one row per group.
pub fn populate_changes_list(tree_view: &gtk::TreeView, summaries: &[GroupSummary]) {
    let Some(model) = tree_view.model() else {
        return;
    };
    let Ok(store) = model.downcast::<gtk::ListStore>() else {
        return;
    };
    store.clear();
    for summary in summaries {
        store.insert_with_values(
            None,
            &[
                (COLUMN_GROUP_ID, &summary.group_id),
                (COLUMN_GROUP_ID + 1, &group_label(summary)),
            ],
        );
    }
}

fn group_label(summary: &GroupSummary) -> String {
    let kind = match summary.group_type {
        GroupType::Ins => "insertion",
        GroupType::Del => "deletion",
        GroupType::Mix => "change",
    };
    let plural = if summary.change_count == 1 { "" } else { "s" };
    format!(
        "{} ({} {kind}{plural})",
        summary.group_id, summary.change_count
    )
}

/// Reads a changes-pane row's group ID back out of `model` at `path`, for a
/// `row-activated` handler to pass to [`changes_navigation_script`].
pub fn group_id_at(model: &gtk::ListStore, path: &gtk::TreePath) -> Option<String> {
    let iter = model.iter(path)?;
    model
        .value(&iter, COLUMN_GROUP_ID as i32)
        .get::<String>()
        .ok()
}

/// The JS to run in the WebView when a changes-pane row is activated:
/// jump to the first element carrying that `data-group-id`, in either
/// mode (Up mode's block wrappers and Down mode's line `<div>`s both
/// carry it — see `mudl-core::changes`).
pub fn changes_navigation_script(group_id: &str) -> String {
    format!(
        "(function() {{ var el = document.querySelector('[data-group-id={id}]'); \
         if (el) el.scrollIntoView({{behavior: \"smooth\", block: \"start\"}}); }})();",
        id = js_string_literal(group_id)
    )
}

#[cfg(test)]
mod navigation_script_tests {
    use super::*;

    #[test]
    fn up_mode_jumps_to_heading_id() {
        let script = navigation_script(Mode::Up, "getting-started", 5);
        assert!(script.contains("getElementById(\"getting-started\")"));
        assert!(!script.contains("data-line"));
    }

    #[test]
    fn down_mode_jumps_to_line() {
        let script = navigation_script(Mode::Down, "getting-started", 5);
        assert!(script.contains("data-line=\"5\""));
        assert!(!script.contains("getElementById"));
    }

    #[test]
    fn up_mode_slug_is_js_escaped() {
        let script = navigation_script(Mode::Up, "a\"quote", 1);
        assert!(script.contains("getElementById(\"a\\\"quote\")"));
    }
}

#[cfg(test)]
mod changes_pane_tests {
    use super::*;

    #[test]
    fn changes_navigation_script_targets_the_group_id_attribute() {
        let script = changes_navigation_script("group-1");
        assert!(script.contains("[data-group-id=\"group-1\"]"));
    }

    #[test]
    fn group_label_pluralizes_change_count() {
        let one = GroupSummary {
            group_id: "group-1".to_string(),
            group_type: GroupType::Ins,
            change_count: 1,
        };
        let many = GroupSummary {
            group_id: "group-2".to_string(),
            group_type: GroupType::Del,
            change_count: 3,
        };
        assert_eq!(group_label(&one), "group-1 (1 insertion)");
        assert_eq!(group_label(&many), "group-2 (3 deletions)");
    }

    #[test]
    fn group_label_describes_mixed_groups_as_changes() {
        let mixed = GroupSummary {
            group_id: "group-3".to_string(),
            group_type: GroupType::Mix,
            change_count: 2,
        };
        assert_eq!(group_label(&mixed), "group-3 (2 changes)");
    }
}

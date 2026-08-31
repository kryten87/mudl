//! Toolbar (Phase 10.4 of `docs/IMPLEMENTATION-PLAN.md`): now just the
//! "Changes since…" popover. Zoom, Readable Column, Line Numbers, Word
//! Wrap, and the theme picker all moved to the Phase 15 menu bar's
//! View/Theme menus — the toolbar controls for them were redundant with
//! the menu items and have been removed; `set_zoom`/`step_zoom`/
//! `set_readable_column`/`set_line_numbers`/`set_word_wrap`/`set_theme`
//! below are what the menu calls directly.
//!
//! Every control keeps three things in lockstep: the live WebView (via
//! `WebView::set_zoom_level` for zoom, or a small injected script for the
//! body-class toggles — matching `mud`'s `ViewToggle` -> CSS-class
//! pattern), the in-memory `Preferences`, and the on-disk preferences
//! file, so a later reload/relaunch starts from the same state the user
//! left the toolbar in.

use std::cell::{Cell, RefCell};
use std::net::SocketAddr;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use webkit2gtk::WebViewExt;

use mudl_config::{Preferences, Theme};
use mudl_server::routes::Mode;
use mudl_server::server::DocumentSource;

use crate::changes;
use crate::config;

/// The step `step_zoom` moves by — reused by the Phase 15 menu's Zoom
/// In/Out items.
pub const ZOOM_STEP: f64 = 0.1;
const ZOOM_RANGE: RangeInclusive<f64> = 0.1..=10.0;

/// Everything a toolbar control needs to read current state and apply a
/// change consistently. Cheap to clone (every field is a reference-counted
/// handle or a small `Copy`/owned value), which is how each signal handler
/// gets its own copy to move into its closure.
#[derive(Clone)]
pub struct Context {
    pub webview: webkit2gtk::WebView,
    pub mode: Rc<Cell<Mode>>,
    pub prefs: Rc<RefCell<Preferences>>,
    pub prefs_path: PathBuf,
    pub document: Arc<DocumentSource>,
    pub addr: SocketAddr,
    /// The Phase 13.9 "Changes" sidebar pane's list view, when the tab was
    /// built with `sidebar_pane = changes` — `None` in outline mode, where
    /// there's no group list to refresh after a waypoint pick.
    pub changes_list: Option<gtk::TreeView>,
}

impl Context {
    /// Writes the in-memory preferences to disk and updates the running
    /// server's rendering config, so both a relaunch and the next
    /// mode-toggle/reload reflect the change. Save errors are swallowed —
    /// there's no good place to surface them from a toolbar click, and the
    /// in-memory/live-server state is already updated either way.
    fn save_and_apply(&self) {
        let prefs = self.prefs.borrow();
        let _ = mudl_config::save(&mudl_config::RealFileSystem, &self.prefs_path, &prefs);
        self.document.set_config(config::document_config(&prefs));
    }

    fn document_url(&self) -> String {
        changes::document_url(self.addr, self.mode.get(), None)
    }
}

/// Builds the toolbar widget and wires every control's signal handler.
pub fn build(ctx: &Context) -> gtk::Box {
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);

    let changes_button = gtk::MenuButton::new();
    changes_button.set_label("Changes since…");
    connect_changes_button(&changes_button, ctx);
    toolbar.pack_start(&changes_button, false, false, 4);

    toolbar
}

/// Wires the "Changes since…" button: on click, queries the file's git
/// history fresh (matching every other control here reading live state
/// rather than a cached snapshot) and pops up `changes::build_popover`
/// anchored to the button. Picking a row re-navigates the WebView with
/// `?waypoint=<index>`, so `mudl-server`'s `serve_document` overlays that
/// diff on the next render.
fn connect_changes_button(button: &gtk::MenuButton, ctx: &Context) {
    let ctx = ctx.clone();
    let button_for_handler = button.clone();
    button.connect_clicked(move |_| {
        let current_content = std::fs::read_to_string(&ctx.document.path).unwrap_or_default();
        let candidates = mudl_diff::git::query_waypoints(
            &mudl_diff::git::RealGitRunner,
            &ctx.document.path,
            &current_content,
        );

        let ctx_for_select = ctx.clone();
        let current_content_for_select = current_content.clone();
        let candidates_for_select = candidates.clone();
        let popover = changes::build_popover(&candidates, move |index| {
            let url =
                changes::document_url(ctx_for_select.addr, ctx_for_select.mode.get(), Some(index));
            ctx_for_select.webview.load_uri(&url);

            if let Some(list_view) = &ctx_for_select.changes_list {
                if let Some(candidate) = candidates_for_select.get(index) {
                    refresh_changes_list(
                        list_view,
                        &current_content_for_select,
                        &candidate.content,
                    );
                }
            }
        });
        popover.set_relative_to(Some(&button_for_handler));
        popover.popup();
    });
}

/// Recomputes the block-level `ChangePlan` for `old`/`new` (stripping
/// frontmatter the same way `render_up` does, so group IDs match what
/// actually ends up in the rendered DOM) and repopulates the "Changes"
/// sidebar pane with its groups.
fn refresh_changes_list(list_view: &gtk::TreeView, old: &str, new: &str) {
    let old_body = mudl_core::frontmatter::extract(old)
        .map(|fm| fm.body)
        .unwrap_or_else(|| old.to_string());
    let new_body = mudl_core::frontmatter::extract(new)
        .map(|fm| fm.body)
        .unwrap_or_else(|| new.to_string());
    let plan = mudl_core::changes::up_change_plan(&old_body, &new_body, 0.25);
    let summaries = mudl_core::changes::group_summaries(&plan);
    crate::sidebar::populate_changes_list(list_view, &summaries);
}

/// Sets the theme preference and applies it live. Used by the menu's Theme
/// submenu (Phase 15.5) — there's no toolbar picker for this anymore.
pub fn set_theme(ctx: &Context, theme: Theme) {
    ctx.prefs.borrow_mut().theme = theme;
    ctx.save_and_apply();
    // The theme is baked into the server-rendered page (its CSS is embedded
    // inline), so — unlike zoom or the toggle classes — applying it live
    // means re-navigating, not injecting a script.
    ctx.webview.load_uri(&ctx.document_url());
}

/// Sets the current mode's zoom level to `value` (clamped to
/// `ZOOM_RANGE`), applies it to the live `WebView`, and persists it. Used
/// by the menu's absolute-value "Actual Size" item and by `step_zoom`
/// below.
pub fn set_zoom(ctx: &Context, value: f64) {
    let mode = ctx.mode.get();
    let clamped = value.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end());
    {
        let mut prefs = ctx.prefs.borrow_mut();
        match mode {
            Mode::Up => prefs.up_mode_zoom_level = clamped,
            Mode::Down => prefs.down_mode_zoom_level = clamped,
        }
    }
    ctx.webview.set_zoom_level(clamped);
    ctx.save_and_apply();
}

/// Moves the current mode's zoom level by `delta` (positive to zoom in,
/// negative to zoom out) via `set_zoom`. Used by the menu's Zoom In/Out
/// items (Phase 15.5) — there's no toolbar button for this anymore.
pub fn step_zoom(ctx: &Context, delta: f64) {
    let current = {
        let prefs = ctx.prefs.borrow();
        match ctx.mode.get() {
            Mode::Up => prefs.up_mode_zoom_level,
            Mode::Down => prefs.down_mode_zoom_level,
        }
    };
    set_zoom(ctx, current + delta);
}

/// Sets whether Down mode's raw-source view shows gutter line numbers, and
/// applies it live. Used by the menu's "Line Numbers" item (Phase 15) —
/// there's no toolbar button for this anymore. Only visible in Down mode
/// (Space bar, or View > Mark Down); has no visible effect in Up mode.
pub fn set_line_numbers(ctx: &Context, active: bool) {
    ctx.prefs.borrow_mut().down_mode_show_line_numbers = active;
    toggle_root_class(&ctx.webview, "has-line-numbers", active);
    ctx.save_and_apply();
}

/// Sets whether Down mode's raw-source view wraps long lines, and applies
/// it live. Used by the menu's "Word Wrap" item (Phase 15) — there's no
/// toolbar button for this anymore. Only visible in Down mode; has no
/// visible effect in Up mode.
pub fn set_word_wrap(ctx: &Context, active: bool) {
    ctx.prefs.borrow_mut().down_mode_wrap_lines = active;
    toggle_root_class(&ctx.webview, "has-word-wrap", active);
    ctx.save_and_apply();
}

/// Sets the Readable Column preference and applies it live to `ctx`'s
/// `WebView`. Used by the menu's "Readable Column" item (Phase 15.5) —
/// there's no toolbar button for this anymore.
pub fn set_readable_column(ctx: &Context, active: bool) {
    ctx.prefs.borrow_mut().ui_show_readable_column = active;
    toggle_root_class(&ctx.webview, "is-readable-column", active);
    ctx.save_and_apply();
}

/// Adds or removes `class` on `<html>` in the live page — the `mud`
/// `ViewToggle` -> CSS-class pattern, applied instantly with no reload.
fn toggle_root_class(webview: &webkit2gtk::WebView, class: &str, add: bool) {
    let script = if add {
        format!("document.documentElement.classList.add(\"{class}\");")
    } else {
        format!("document.documentElement.classList.remove(\"{class}\");")
    };
    webview.evaluate_javascript(&script, None, None, None::<&gtk::gio::Cancellable>, |_| {});
}

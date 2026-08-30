//! Toolbar (Phase 10.4 of `docs/IMPLEMENTATION-PLAN.md`): theme picker,
//! zoom in/out, and word-wrap/line-numbers/readable-column toggle buttons.
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

const ZOOM_STEP: f64 = 0.1;
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

    let theme_combo = build_theme_combo(ctx);
    toolbar.pack_start(&theme_combo, false, false, 4);

    let zoom_out = gtk::Button::with_label("-");
    let zoom_in = gtk::Button::with_label("+");
    connect_zoom_button(&zoom_out, ctx, -ZOOM_STEP);
    connect_zoom_button(&zoom_in, ctx, ZOOM_STEP);
    toolbar.pack_start(&zoom_out, false, false, 0);
    toolbar.pack_start(&zoom_in, false, false, 4);

    let line_numbers = gtk::ToggleButton::with_label("Line #s");
    let word_wrap = gtk::ToggleButton::with_label("Wrap");
    let readable_column = gtk::ToggleButton::with_label("Readable");
    {
        let prefs = ctx.prefs.borrow();
        line_numbers.set_active(prefs.down_mode_show_line_numbers);
        word_wrap.set_active(prefs.down_mode_wrap_lines);
        readable_column.set_active(prefs.ui_show_readable_column);
    }
    connect_line_numbers_toggle(&line_numbers, ctx);
    connect_word_wrap_toggle(&word_wrap, ctx);
    connect_readable_column_toggle(&readable_column, ctx);
    toolbar.pack_start(&line_numbers, false, false, 4);
    toolbar.pack_start(&word_wrap, false, false, 0);
    toolbar.pack_start(&readable_column, false, false, 0);

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

fn build_theme_combo(ctx: &Context) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    for theme in Theme::all() {
        combo.append(Some(theme.as_str()), theme.as_str());
    }
    combo.set_active_id(Some(ctx.prefs.borrow().theme.as_str()));

    let ctx = ctx.clone();
    combo.connect_changed(move |combo| {
        let Some(id) = combo.active_id() else {
            return;
        };
        let Some(theme) = Theme::parse(&id) else {
            return;
        };
        ctx.prefs.borrow_mut().theme = theme;
        ctx.save_and_apply();
        // The theme is baked into the server-rendered page (its CSS is
        // embedded inline), so — unlike zoom or the toggle classes —
        // applying it live means re-navigating, not injecting a script.
        ctx.webview.load_uri(&ctx.document_url());
    });

    combo
}

fn connect_zoom_button(button: &gtk::Button, ctx: &Context, delta: f64) {
    let ctx = ctx.clone();
    button.connect_clicked(move |_| {
        let mode = ctx.mode.get();
        let current = {
            let prefs = ctx.prefs.borrow();
            match mode {
                Mode::Up => prefs.up_mode_zoom_level,
                Mode::Down => prefs.down_mode_zoom_level,
            }
        };
        let next = (current + delta).clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end());
        {
            let mut prefs = ctx.prefs.borrow_mut();
            match mode {
                Mode::Up => prefs.up_mode_zoom_level = next,
                Mode::Down => prefs.down_mode_zoom_level = next,
            }
        }
        ctx.webview.set_zoom_level(next);
        ctx.save_and_apply();
    });
}

fn connect_line_numbers_toggle(button: &gtk::ToggleButton, ctx: &Context) {
    let ctx = ctx.clone();
    button.connect_toggled(move |button| {
        let active = button.is_active();
        ctx.prefs.borrow_mut().down_mode_show_line_numbers = active;
        toggle_root_class(&ctx.webview, "has-line-numbers", active);
        ctx.save_and_apply();
    });
}

fn connect_word_wrap_toggle(button: &gtk::ToggleButton, ctx: &Context) {
    let ctx = ctx.clone();
    button.connect_toggled(move |button| {
        let active = button.is_active();
        ctx.prefs.borrow_mut().down_mode_wrap_lines = active;
        toggle_root_class(&ctx.webview, "has-word-wrap", active);
        ctx.save_and_apply();
    });
}

fn connect_readable_column_toggle(button: &gtk::ToggleButton, ctx: &Context) {
    let ctx = ctx.clone();
    button.connect_toggled(move |button| {
        let active = button.is_active();
        ctx.prefs.borrow_mut().ui_show_readable_column = active;
        toggle_root_class(&ctx.webview, "is-readable-column", active);
        ctx.save_and_apply();
    });
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

//! Toolbar (Phase 10.4 of `docs/IMPLEMENTATION-PLAN.md`). Zoom, Readable
//! Column, Line Numbers, Word Wrap, and the theme picker all moved to the
//! Phase 15 menu bar's View/Theme menus — the toolbar controls for them
//! were redundant with the menu items and have been removed;
//! `set_zoom`/`step_zoom`/`set_readable_column`/`set_line_numbers`/
//! `set_word_wrap`/`set_theme` below are what the menu calls directly.
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

use webkit2gtk::WebViewExt;

use mudl_config::{Preferences, Theme};
use mudl_server::routes::Mode;
use mudl_server::server::DocumentSource;

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
        document_url(self.addr, self.mode.get())
    }
}

/// The `http://<addr>/...` URL for `mode` — `?mode=down` for Down mode, no
/// query string at all for Up mode (plain `/` already means Up mode).
pub fn document_url(addr: SocketAddr, mode: Mode) -> String {
    match mode {
        Mode::Up => format!("http://{addr}/"),
        Mode::Down => format!("http://{addr}/?mode=down"),
    }
}

/// Builds the toolbar widget. Every control that used to live here (Zoom,
/// Readable Column, Line Numbers, Word Wrap, the theme picker) has moved to
/// the Phase 15 menu bar — see the module doc comment — leaving nothing to
/// pack into the returned box today.
pub fn build(_ctx: &Context) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, 4)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn addr() -> SocketAddr {
        "127.0.0.1:53211".parse().unwrap()
    }

    #[test]
    fn up_mode_is_plain_root() {
        assert_eq!(document_url(addr(), Mode::Up), "http://127.0.0.1:53211/");
    }

    #[test]
    fn down_mode_has_mode_query() {
        assert_eq!(
            document_url(addr(), Mode::Down),
            "http://127.0.0.1:53211/?mode=down"
        );
    }
}

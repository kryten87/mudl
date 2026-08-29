//! The GTK3 + WebKit2GTK window (Phases 10.1-10.8 of
//! `docs/IMPLEMENTATION-PLAN.md`): a `gtk::ApplicationWindow` containing a
//! `gtk::Notebook`, one tab per opened file, each tab holding its own
//! toolbar, outline sidebar, and `webkit2gtk::WebView` pointed at its own
//! `mudl-server` instance (with its own file watcher live-reloading it —
//! Phase 10.8), with a Space-bar Up/Down mode toggle scoped to whichever
//! tab currently has focus.
//!
//! Per-tab keyboard shortcuts (Space, Ctrl+F) are connected on each tab's
//! own root container rather than the shared window: GTK3 delivers
//! `key-press-event` to the focused widget and bubbles it up through that
//! widget's own ancestor chain, so a handler on tab A's container never
//! fires while focus is inside tab B's — no explicit "is this the active
//! tab" bookkeeping needed.
//!
//! This is GTK signal wiring and WebKit navigation, not algorithmic logic —
//! per the plan's own note on Phase 10, there's no pure decision to extract
//! here beyond the mode flip itself (`crate::toggle::next_mode`, unit
//! tested on its own) and the preferences/document-config mapping
//! (`crate::config`, `crate::sidebar::navigation_script`, also unit
//! tested), so this module itself has no unit tests; it's verified by the
//! manual smoke-test checklist the plan prescribes for this phase.

use std::cell::{Cell, RefCell};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::thread;

use gtk::prelude::*;
use javascriptcore::ValueExt;
use webkit2gtk::WebViewExt;

use mudl_config::Preferences;
use mudl_core::headings::extract_headings;
use mudl_core::outline;
use mudl_server::document::DocumentConfig;
use mudl_server::fs::RealFileSystem;
use mudl_server::routes::Mode;
use mudl_server::server::{self, DocumentSource};
use mudl_server::version::VersionCounter;

use crate::find;
use crate::geometry;
use crate::sidebar;
use crate::toggle::next_mode;
use crate::toolbar;

const APP_ID: &str = "com.monstergfx.mudl";

/// How often each open document's file is checked for changes (Phase
/// 10.8). Matches the plan's suggested default (§6.1: "300ms — tunable,
/// not hardcoded"); there's no preference wired up to tune it yet.
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(300);

/// Reports the current scroll position as a fraction of the scrollable
/// height: `0.0` at the top, `1.0` at the bottom, `0.0` when the page
/// doesn't scroll at all (avoids a divide-by-zero rather than reporting
/// `NaN`/`Infinity`).
const CAPTURE_SCROLL_FRACTION_JS: &str = "\
(function () {
  var max = document.body.scrollHeight - window.innerHeight;
  return max > 0 ? window.scrollY / max : 0;
})();";

/// One file's server instance, started before the window exists so a
/// failure to bind/canonicalize/read is reported before any GTK state is
/// touched.
struct TabSource {
    /// The canonicalized path — identity for outline extraction and (for
    /// the first tab specifically) the window geometry key (Phase 10.7).
    path: PathBuf,
    title: String,
    addr: SocketAddr,
    markdown: String,
    document: Arc<DocumentSource>,
    /// Kept alive only to keep the file watcher running (Phase 10.8) —
    /// dropped, and the watch stopped, when the window closes and `tabs`
    /// goes out of scope.
    _watch: mudl_watch::WatchHandle,
}

/// Starts a `mudl-server` instance per file in `paths` and opens one
/// window with one tab per file. Blocks until the window is closed —
/// `gtk::Application::run` drives the GTK main loop for the lifetime of
/// the process.
pub fn run(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("no file given (folder index launch mode isn't implemented yet)".to_string());
    }

    let prefs_path = preferences_path();
    let prefs = Rc::new(RefCell::new(mudl_config::load(
        &mudl_config::RealFileSystem,
        &prefs_path,
    )));

    let mut tabs = Vec::with_capacity(paths.len());
    for path in paths {
        tabs.push(start_tab_source(path, &prefs.borrow())?);
    }

    // `NON_UNIQUE`: without it, GApplication treats every process sharing
    // `APP_ID` as one single-instance app — a second `mudl` invocation
    // would just send an `activate` signal to the already-running first
    // process (over D-Bus) instead of running its own GTK app, so the
    // "new" window would actually be `build_window` re-running in the
    // original process with the original process's `tabs`, i.e. the first
    // file ever opened. Each invocation must run fully independently.
    let application = gtk::Application::new(
        Some(APP_ID),
        gtk::gio::ApplicationFlags::NON_UNIQUE,
    );
    application.connect_activate(move |app| {
        build_window(app, &tabs, Rc::clone(&prefs), &prefs_path);
    });

    // No CLI args of ours are meaningful to GTK/GApplication's own option
    // parsing, so run with an empty argv rather than `run()`'s default of
    // forwarding `std::env::args()` (mudl's own flags like `-u`/`-d` are
    // already consumed by mudl-cli before this ever runs, but there's no
    // reason to hand them to GApplication a second time).
    application.run_with_args::<&str>(&[]);
    Ok(())
}

/// `~/.config/mudl/preferences` — matches `mudl-config`'s documented
/// on-disk location (Phase 7.1). Falls back to a relative path if `$HOME`
/// isn't set (e.g. an unusual test/container environment) rather than
/// panicking; `mudl_config::load` already treats a missing/unreadable file
/// as "use defaults", so a wrong-but-harmless path degrades gracefully.
fn preferences_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/mudl/preferences")
}

/// `~/.config/mudl/window-geometry` — see `crate::geometry`'s doc comment
/// for why this is a separate file from `preferences_path()`'s.
fn geometry_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/mudl/window-geometry")
}

fn start_tab_source(path: &Path, prefs: &Preferences) -> Result<TabSource, String> {
    let absolute =
        std::fs::canonicalize(path).map_err(|err| format!("{}: {err}", path.display()))?;
    // Best-effort: the sidebar's initial outline just starts empty if the
    // file can't be read here (the main WebView will independently 404 for
    // the same reason once it navigates to the server).
    let markdown = std::fs::read_to_string(&absolute).unwrap_or_default();
    let title = absolute
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let initial_config = crate::config::document_config(prefs);
    let (addr, document, watch) = start_server_for(absolute.clone(), initial_config)?;

    Ok(TabSource {
        path: absolute,
        title,
        addr,
        markdown,
        document,
        _watch: watch,
    })
}

fn build_window(
    app: &gtk::Application,
    tabs: &[TabSource],
    prefs: Rc<RefCell<Preferences>>,
    prefs_path: &Path,
) {
    let window = gtk::ApplicationWindow::new(app);
    window.set_title("mudl");

    // The window's geometry is keyed by its first tab's path (Phase
    // 10.7) — `tabs` is never empty (`run` errors first if `paths` was).
    let geometry_key = tabs[0].path.clone();
    let geometry_path = geometry_path();
    match geometry::load(&mudl_config::RealFileSystem, &geometry_path, &geometry_key) {
        Some(saved) => {
            window.resize(saved.width, saved.height);
            window.move_(saved.x, saved.y);
        }
        None => {
            window.set_default_size(960, 720);
            window.set_position(gtk::WindowPosition::Center);
        }
    }
    connect_geometry_save(&window, geometry_path, geometry_key);

    let notebook = gtk::Notebook::new();
    for tab in tabs {
        let tab_widget = build_tab(tab, Rc::clone(&prefs), prefs_path);
        let label = gtk::Label::new(Some(&tab.title));
        notebook.append_page(&tab_widget, Some(&label));
    }

    window.add(&notebook);
    window.show_all();
}

/// On close, saves the window's current size/position keyed by
/// `geometry_key` (Phase 10.7). Save errors are swallowed — there's
/// nothing useful to do with them at the point the window is closing.
fn connect_geometry_save(
    window: &gtk::ApplicationWindow,
    geometry_path: PathBuf,
    geometry_key: PathBuf,
) {
    window.connect_delete_event(move |window, _event| {
        let (width, height) = window.size();
        let (x, y) = window.position();
        let saved = geometry::Geometry {
            width,
            height,
            x,
            y,
        };
        let _ = geometry::save(
            &mudl_config::RealFileSystem,
            &geometry_path,
            &geometry_key,
            saved,
        );
        gtk::glib::Propagation::Proceed
    });
}

/// Builds one tab's entire content: toolbar, outline sidebar, WebView, and
/// the find-bar overlay, wired together and pointed at `tab`'s server
/// instance. Returns the tab's root widget, ready to hand to
/// `gtk::Notebook::append_page`.
fn build_tab(tab: &TabSource, prefs: Rc<RefCell<Preferences>>, prefs_path: &Path) -> gtk::Box {
    let addr = tab.addr;
    let webview = webkit2gtk::WebView::new();
    webview.load_uri(&format!("http://{addr}/"));

    // The mode a Space-bar press toggles *from*, shared with the sidebar's
    // row-activation handler and the toolbar (both need to know whether
    // Up- or Down-mode preferences/navigation apply) — and the scroll
    // fraction captured just before a mode-toggle navigation, restored by
    // the load-changed handler once the new page loads. All of these live
    // in `Rc<Cell<_>>`/`Rc<RefCell<_>>`, shared across closures on GTK's
    // single-threaded main loop (`Rc`, not `Arc`, is the right tool here).
    let mode = Rc::new(Cell::new(Mode::Up));
    let pending_scroll_fraction = Rc::new(Cell::new(0.0_f64));

    connect_scroll_restore(&webview, Rc::clone(&pending_scroll_fraction));
    connect_zoom_restore(&webview, Rc::clone(&mode), Rc::clone(&prefs));

    let headings = extract_headings(&tab.markdown);
    let outline_tree = outline::build_tree(&headings);
    let sidebar_view = sidebar::build_outline_tree_view(&outline_tree);
    connect_sidebar_navigation(&sidebar_view, &webview, Rc::clone(&mode));

    let sidebar_scroller =
        gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sidebar_scroller.add(&sidebar_view);
    sidebar_scroller.set_size_request(220, -1);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.pack1(&sidebar_scroller, false, false);
    paned.pack2(&webview, true, true);

    let toolbar_ctx = toolbar::Context {
        webview: webview.clone(),
        mode: Rc::clone(&mode),
        prefs,
        prefs_path: prefs_path.to_path_buf(),
        document: Arc::clone(&tab.document),
        addr,
    };
    let toolbar_widget = toolbar::build(&toolbar_ctx);

    let (overlay, find_bar) = find::build(&paned, &webview);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.pack_start(&toolbar_widget, false, false, 0);
    vbox.pack_start(&overlay, true, true, 0);

    connect_find_shortcut(&vbox, find_bar);
    connect_mode_toggle(&vbox, &webview, addr, mode, pending_scroll_fraction);

    vbox
}

/// Ctrl+F shows and focuses the find bar; WebKit2GTK's own
/// `WebKitFindController` handles everything else (Phase 10.5). Connected
/// on the tab's own container (see the module doc comment on per-tab
/// shortcut scoping), not the shared window.
fn connect_find_shortcut(tab_container: &gtk::Box, find_bar: find::FindBar) {
    tab_container.connect_key_press_event(move |_widget, event| {
        let ctrl_f = event.state().contains(gtk::gdk::ModifierType::CONTROL_MASK)
            && event.keyval() == gtk::gdk::keys::constants::f;
        if ctrl_f {
            find_bar.show();
            return gtk::glib::Propagation::Stop;
        }
        gtk::glib::Propagation::Proceed
    });
}

/// After every finished page load, scrolls to whatever fraction was
/// captured before the navigation that produced it (`0.0` — the top — on
/// the very first load, since nothing has been captured yet).
fn connect_scroll_restore(webview: &webkit2gtk::WebView, pending_scroll_fraction: Rc<Cell<f64>>) {
    webview.connect_load_changed(move |webview, event| {
        if event != webkit2gtk::LoadEvent::Finished {
            return;
        }
        let fraction = pending_scroll_fraction.get();
        let script = format!(
            "window.scrollTo(0, {fraction} * (document.body.scrollHeight - window.innerHeight));"
        );
        webview.evaluate_javascript(&script, None, None, None::<&gtk::gio::Cancellable>, |_| {});
    });
}

/// After every finished page load, applies the current mode's persisted
/// zoom level via WebKit's own zoom API (Phase 10.4) — needed on every
/// load, not just once, since a mode toggle or theme change re-navigates
/// to a fresh page that otherwise starts back at WebKit's default zoom.
fn connect_zoom_restore(
    webview: &webkit2gtk::WebView,
    mode: Rc<Cell<Mode>>,
    prefs: Rc<RefCell<Preferences>>,
) {
    webview.connect_load_changed(move |webview, event| {
        if event != webkit2gtk::LoadEvent::Finished {
            return;
        }
        let zoom = {
            let prefs = prefs.borrow();
            match mode.get() {
                Mode::Up => prefs.up_mode_zoom_level,
                Mode::Down => prefs.down_mode_zoom_level,
            }
        };
        webview.set_zoom_level(zoom);
    });
}

/// Space bar: captures the current scroll fraction, flips `mode`, and
/// re-navigates the WebView to `/` with `?mode=down` when the new mode is
/// Down (its absence already means Up — see `mudl_server::routes::dispatch`).
/// Connected on the tab's own container (see the module doc comment on
/// per-tab shortcut scoping), not the shared window.
fn connect_mode_toggle(
    tab_container: &gtk::Box,
    webview: &webkit2gtk::WebView,
    addr: SocketAddr,
    mode: Rc<Cell<Mode>>,
    pending_scroll_fraction: Rc<Cell<f64>>,
) {
    let webview = webview.clone();
    tab_container.connect_key_press_event(move |_widget, event| {
        if event.keyval() != gtk::gdk::keys::constants::space {
            return gtk::glib::Propagation::Proceed;
        }

        let webview_for_nav = webview.clone();
        let mode = Rc::clone(&mode);
        let pending_scroll_fraction = Rc::clone(&pending_scroll_fraction);
        webview.evaluate_javascript(
            CAPTURE_SCROLL_FRACTION_JS,
            None,
            None,
            None::<&gtk::gio::Cancellable>,
            move |result| {
                let fraction = result
                    .ok()
                    .filter(|value| value.is_number())
                    .map(|value| value.to_double())
                    .unwrap_or(0.0);
                pending_scroll_fraction.set(fraction);

                let next = next_mode(mode.get());
                mode.set(next);
                let path = match next {
                    Mode::Up => "/",
                    Mode::Down => "/?mode=down",
                };
                webview_for_nav.load_uri(&format!("http://{addr}{path}"));
            },
        );

        gtk::glib::Propagation::Stop
    });
}

/// Row activation (double-click or Enter) in the outline sidebar: reads
/// the activated row's slug/line back out of the tree store and runs
/// `sidebar::navigation_script` in the WebView to jump there.
fn connect_sidebar_navigation(
    tree_view: &gtk::TreeView,
    webview: &webkit2gtk::WebView,
    mode: Rc<Cell<Mode>>,
) {
    let webview = webview.clone();
    tree_view.connect_row_activated(move |tree_view, path, _column| {
        let Some(model) = tree_view.model().and_downcast::<gtk::TreeStore>() else {
            return;
        };
        let Some((slug, line)) = sidebar::slug_and_line_at(&model, path) else {
            return;
        };
        let script = sidebar::navigation_script(mode.get(), &slug, line);
        webview.evaluate_javascript(&script, None, None, None::<&gtk::gio::Cancellable>, |_| {});
    });
}

/// Binds a `mudl-server` instance for `path`, seeds it with `config`, and
/// runs its accept loop on a background thread, and starts watching `path`
/// for changes on another (Phase 10.8), bumping the same `VersionCounter`
/// the live-reload script served with every page (§2, wired in Phase
/// 10.1's `document::render`) long-polls against. Returns the address the
/// WebView should navigate to, the shared `DocumentSource` handle (so the
/// toolbar can update its config live — Phase 10.4), and the watcher's
/// handle — dropping/stopping it stops the watch, so the caller must keep
/// it alive for as long as the window should live-reload.
fn start_server_for(
    path: PathBuf,
    config: DocumentConfig,
) -> Result<(SocketAddr, Arc<DocumentSource>, mudl_watch::WatchHandle), String> {
    let listener = server::bind().map_err(|err| format!("failed to start local server: {err}"))?;
    let addr = listener
        .local_addr()
        .map_err(|err| format!("failed to read local server address: {err}"))?;

    let version = VersionCounter::new();
    let filesystem = Arc::new(RealFileSystem);
    let document = Arc::new(DocumentSource::new(path.clone()));
    document.set_config(config);

    let document_for_thread = Arc::clone(&document);
    let version_for_server = version.clone();
    thread::spawn(move || {
        server::serve(
            listener,
            version_for_server,
            filesystem,
            document_for_thread,
        )
    });

    let watch_source = mudl_watch::PollingChangeSource::new(
        path,
        mudl_watch::RealFileSystem,
        mudl_watch::RealClock,
        WATCH_INTERVAL,
    );
    let watch_handle = watch_source.spawn(move |_event| version.bump());

    Ok((addr, document, watch_handle))
}

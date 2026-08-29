//! The GTK3 + WebKit2GTK window (Phases 10.1-10.4 of
//! `docs/IMPLEMENTATION-PLAN.md`): a `gtk::ApplicationWindow` containing a
//! toolbar, an outline sidebar, and a `webkit2gtk::WebView`, pointed at a
//! `mudl-server` instance started for the given file, with a Space-bar
//! Up/Down mode toggle.
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
use std::path::PathBuf;
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
use crate::sidebar;
use crate::toggle::next_mode;
use crate::toolbar;

const APP_ID: &str = "com.monstergfx.mudl";

/// Reports the current scroll position as a fraction of the scrollable
/// height: `0.0` at the top, `1.0` at the bottom, `0.0` when the page
/// doesn't scroll at all (avoids a divide-by-zero rather than reporting
/// `NaN`/`Infinity`).
const CAPTURE_SCROLL_FRACTION_JS: &str = "\
(function () {
  var max = document.body.scrollHeight - window.innerHeight;
  return max > 0 ? window.scrollY / max : 0;
})();";

/// Starts a `mudl-server` instance serving `path` and opens a window
/// pointed at it. Blocks until the window is closed — `gtk::Application::run`
/// drives the GTK main loop for the lifetime of the process.
pub fn run(path: PathBuf) -> Result<(), String> {
    let absolute =
        std::fs::canonicalize(&path).map_err(|err| format!("{}: {err}", path.display()))?;

    // Best-effort: the sidebar's initial outline just starts empty if the
    // file can't be read here (the main WebView will independently 404 for
    // the same reason once it navigates to the server).
    let markdown = std::fs::read_to_string(&absolute).unwrap_or_default();

    let prefs_path = preferences_path();
    let prefs = Rc::new(RefCell::new(mudl_config::load(
        &mudl_config::RealFileSystem,
        &prefs_path,
    )));
    let initial_config = crate::config::document_config(&prefs.borrow());

    let (addr, document) = start_server_for(absolute, initial_config)?;

    let application = gtk::Application::new(Some(APP_ID), gtk::gio::ApplicationFlags::empty());
    application.connect_activate(move |app| {
        build_window(
            app,
            addr,
            &markdown,
            Rc::clone(&prefs),
            &prefs_path,
            Arc::clone(&document),
        );
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

fn build_window(
    app: &gtk::Application,
    addr: SocketAddr,
    markdown: &str,
    prefs: Rc<RefCell<Preferences>>,
    prefs_path: &std::path::Path,
    document: Arc<DocumentSource>,
) {
    let window = gtk::ApplicationWindow::new(app);
    window.set_title("mudl");
    window.set_default_size(960, 720);

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
    connect_mode_toggle(
        &window,
        &webview,
        addr,
        Rc::clone(&mode),
        pending_scroll_fraction,
    );

    let headings = extract_headings(markdown);
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
        mode,
        prefs,
        prefs_path: prefs_path.to_path_buf(),
        document,
        addr,
    };
    let toolbar_widget = toolbar::build(&toolbar_ctx);

    let (overlay, find_bar) = find::build(&paned, &webview);
    connect_find_shortcut(&window, find_bar);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.pack_start(&toolbar_widget, false, false, 0);
    vbox.pack_start(&overlay, true, true, 0);

    window.add(&vbox);
    window.show_all();
}

/// Ctrl+F shows and focuses the find bar; WebKit2GTK's own
/// `WebKitFindController` handles everything else (Phase 10.5).
fn connect_find_shortcut(window: &gtk::ApplicationWindow, find_bar: find::FindBar) {
    window.connect_key_press_event(move |_window, event| {
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
fn connect_mode_toggle(
    window: &gtk::ApplicationWindow,
    webview: &webkit2gtk::WebView,
    addr: SocketAddr,
    mode: Rc<Cell<Mode>>,
    pending_scroll_fraction: Rc<Cell<f64>>,
) {
    let webview = webview.clone();
    window.connect_key_press_event(move |_window, event| {
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
/// runs its accept loop on a background thread. Returns the address it's
/// listening on (so the WebView can navigate to it) and the shared
/// `DocumentSource` handle (so the toolbar can update its config live —
/// Phase 10.4).
fn start_server_for(
    path: PathBuf,
    config: DocumentConfig,
) -> Result<(SocketAddr, Arc<DocumentSource>), String> {
    let listener = server::bind().map_err(|err| format!("failed to start local server: {err}"))?;
    let addr = listener
        .local_addr()
        .map_err(|err| format!("failed to read local server address: {err}"))?;

    let version = VersionCounter::new();
    let filesystem = Arc::new(RealFileSystem);
    let document = Arc::new(DocumentSource::new(path));
    document.set_config(config);

    let document_for_thread = Arc::clone(&document);
    thread::spawn(move || server::serve(listener, version, filesystem, document_for_thread));

    Ok((addr, document))
}

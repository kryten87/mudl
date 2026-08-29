//! The GTK3 + WebKit2GTK window (Phase 10.1-10.2 of
//! `docs/IMPLEMENTATION-PLAN.md`): one `gtk::ApplicationWindow` containing
//! one `webkit2gtk::WebView`, pointed at a `mudl-server` instance started
//! for the given file, with a Space-bar Up/Down mode toggle.
//!
//! This is GTK signal wiring and WebKit navigation, not algorithmic logic —
//! per the plan's own note on Phase 10, there's no pure decision to extract
//! here beyond the mode flip itself (`crate::toggle::next_mode`, unit
//! tested on its own), so this module has no unit tests; it's verified by
//! the manual smoke-test checklist the plan prescribes for this phase.

use std::cell::Cell;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;

use gtk::prelude::*;
use javascriptcore::ValueExt;
use webkit2gtk::WebViewExt;

use mudl_core::headings::extract_headings;
use mudl_core::outline;
use mudl_server::fs::RealFileSystem;
use mudl_server::routes::Mode;
use mudl_server::server::{self, DocumentSource};
use mudl_server::version::VersionCounter;

use crate::sidebar;
use crate::toggle::next_mode;

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

    let addr = start_server_for(absolute)?;

    let application = gtk::Application::new(Some(APP_ID), gtk::gio::ApplicationFlags::empty());
    application.connect_activate(move |app| build_window(app, addr, &markdown));

    // No CLI args of ours are meaningful to GTK/GApplication's own option
    // parsing, so run with an empty argv rather than `run()`'s default of
    // forwarding `std::env::args()` (mudl's own flags like `-u`/`-d` are
    // already consumed by mudl-cli before this ever runs, but there's no
    // reason to hand them to GApplication a second time).
    application.run_with_args::<&str>(&[]);
    Ok(())
}

fn build_window(app: &gtk::Application, addr: SocketAddr, markdown: &str) {
    let window = gtk::ApplicationWindow::new(app);
    window.set_title("mudl");
    window.set_default_size(960, 720);

    let webview = webkit2gtk::WebView::new();
    webview.load_uri(&format!("http://{addr}/"));

    // The mode a Space-bar press toggles *from*, shared with the sidebar's
    // row-activation handler so it knows whether to navigate by `#slug`
    // (Up) or by `data-line` (Down) — and the scroll fraction captured just
    // before a mode-toggle navigation, restored by the load-changed
    // handler once the new page loads. All three live in `Rc<Cell<_>>`,
    // shared across closures on GTK's single-threaded main loop (`Rc`, not
    // `Arc`, is the right tool here).
    let mode = Rc::new(Cell::new(Mode::Up));
    let pending_scroll_fraction = Rc::new(Cell::new(0.0_f64));

    connect_scroll_restore(&webview, Rc::clone(&pending_scroll_fraction));
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
    connect_sidebar_navigation(&sidebar_view, &webview, mode);

    let sidebar_scroller =
        gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sidebar_scroller.add(&sidebar_view);
    sidebar_scroller.set_size_request(220, -1);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.pack1(&sidebar_scroller, false, false);
    paned.pack2(&webview, true, true);

    window.add(&paned);
    window.show_all();
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

/// Binds a `mudl-server` instance for `path` and runs its accept loop on a
/// background thread. Returns the address it's listening on so the WebView
/// can navigate to it.
fn start_server_for(path: PathBuf) -> Result<SocketAddr, String> {
    let listener = server::bind().map_err(|err| format!("failed to start local server: {err}"))?;
    let addr = listener
        .local_addr()
        .map_err(|err| format!("failed to read local server address: {err}"))?;

    let version = VersionCounter::new();
    let filesystem = Arc::new(RealFileSystem);
    let document = Arc::new(DocumentSource::new(path));

    thread::spawn(move || server::serve(listener, version, filesystem, document));

    Ok(addr)
}

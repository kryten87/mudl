//! The minimal GTK3 + WebKit2GTK window (Phase 10.1 of
//! `docs/IMPLEMENTATION-PLAN.md`): one `gtk::ApplicationWindow` containing
//! one `webkit2gtk::WebView`, pointed at a `mudl-server` instance started
//! for the given file.
//!
//! This is GTK signal wiring and WebKit navigation, not algorithmic logic —
//! per the plan's own note on Phase 10, there's no pure decision to extract
//! here (unlike 10.2's `toggle::next_mode`), so this module has no unit
//! tests; it's verified by the manual smoke-test checklist the plan
//! prescribes for this phase.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use gtk::prelude::*;
use webkit2gtk::WebViewExt;

use mudl_server::fs::RealFileSystem;
use mudl_server::server::{self, DocumentSource};
use mudl_server::version::VersionCounter;

const APP_ID: &str = "com.monstergfx.mudl";

/// Starts a `mudl-server` instance serving `path` and opens a window
/// pointed at it. Blocks until the window is closed — `gtk::Application::run`
/// drives the GTK main loop for the lifetime of the process.
pub fn run(path: PathBuf) -> Result<(), String> {
    let absolute =
        std::fs::canonicalize(&path).map_err(|err| format!("{}: {err}", path.display()))?;

    let addr = start_server_for(absolute)?;

    let application = gtk::Application::new(Some(APP_ID), gtk::gio::ApplicationFlags::empty());
    application.connect_activate(move |app| {
        let window = gtk::ApplicationWindow::new(app);
        window.set_title("mudl");
        window.set_default_size(960, 720);

        let webview = webkit2gtk::WebView::new();
        webview.load_uri(&format!("http://{addr}/"));
        window.add(&webview);
        window.show_all();
    });

    // No CLI args of ours are meaningful to GTK/GApplication's own option
    // parsing, so run with an empty argv rather than `run()`'s default of
    // forwarding `std::env::args()` (mudl's own flags like `-u`/`-d` are
    // already consumed by mudl-cli before this ever runs, but there's no
    // reason to hand them to GApplication a second time).
    application.run_with_args::<&str>(&[]);
    Ok(())
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

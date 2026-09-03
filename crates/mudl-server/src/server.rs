//! The impure server loop (Phase 4, step 4.5 of
//! `docs/IMPLEMENTATION-PLAN.md`): bind a `TcpListener`, accept
//! connections, and for each one read a single HTTP request, dispatch it,
//! and write back a response.
//!
//! Thread-per-connection is deliberately simple — `mudl` serves a handful
//! of concurrent requests from a single local WebView, not production web
//! traffic (see the plan's rationale in §10, step 4.5).

use std::collections::HashSet;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::assets;
use crate::document::{self, DocumentConfig};
use crate::fs::FileSystem;
use crate::http::{self, Request};
use crate::routes::{self, Route};
use crate::version::VersionCounter;

/// Which file `Route::Document` renders, and how — the per-server-instance
/// identity a `mudl-gui` window/tab (Phase 10.6: one server per document)
/// supplies when it starts serving.
///
/// `config` is behind a `Mutex` rather than a plain field: Phase 10.4's
/// theme picker/zoom controls change how the *running* server renders
/// (e.g. re-navigating the WebView to the same URL after a theme change),
/// not just the next server instance — so a request in flight and a GUI
/// control updating the config from another thread both need safe
/// concurrent access to the same `DocumentSource`.
#[derive(Debug)]
pub struct DocumentSource {
    pub path: PathBuf,
    pub config: Mutex<DocumentConfig>,
    /// The absolute local paths the most recent render of `path` actually
    /// referenced (via `<img src>`) — the allowlist `Route::LocalFile`
    /// (`serve_local_file`) is confined to. Closes `docs/SECURITY.md`
    /// Finding 2: `/local/<path>` used to read and return any path a
    /// request named, with no relation to the document being viewed.
    allowed_local_paths: Mutex<HashSet<PathBuf>>,
    /// A random token minted once per `DocumentSource` (one per server
    /// instance — Phase 10.6's "one server per document/tab") and required
    /// as part of every `/local/<token>/<path>` request. This is
    /// `docs/SECURITY.md` Finding 2's first hardening step: without it, a
    /// party that merely finds the port — a local port scan, or a
    /// DNS-rebound page guessing it — could request `/local/` paths cold.
    /// With it, the only way to learn a valid token is to have already
    /// loaded `/` and read it out of that render's own `<img src>` values.
    local_token: String,
}

impl DocumentSource {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            config: Mutex::new(DocumentConfig::default()),
            allowed_local_paths: Mutex::new(HashSet::new()),
            local_token: random_token(),
        }
    }

    /// The per-instance token every `/local/` request must present
    /// (`docs/SECURITY.md` Finding 2). `document::render` embeds it in the
    /// `/local/<token>/<path>` URLs it generates.
    pub fn local_token(&self) -> &str {
        &self.local_token
    }

    /// Replaces the current rendering config; the next request (or the
    /// next `/wait`-triggered reload) picks it up.
    pub fn set_config(&self, config: DocumentConfig) {
        *self.config.lock().unwrap() = config;
    }

    fn config_snapshot(&self) -> DocumentConfig {
        self.config.lock().unwrap().clone()
    }

    /// Records the local paths the render that just produced this page
    /// referenced, replacing whatever the previous render allowed.
    fn set_allowed_local_paths(&self, paths: Vec<PathBuf>) {
        *self.allowed_local_paths.lock().unwrap() = paths.into_iter().collect();
    }

    /// Whether `path` was referenced by the most recent render — the only
    /// paths `Route::LocalFile` may serve.
    fn allows_local_path(&self, path: &Path) -> bool {
        self.allowed_local_paths.lock().unwrap().contains(path)
    }
}

/// How long a `/wait` long-poll request blocks before returning the
/// unchanged version. Short enough to keep tests fast, long enough to be a
/// plausible long-poll interval in practice.
const WAIT_TIMEOUT: Duration = Duration::from_millis(500);

/// Upper bound on the size of a single HTTP request line. No real request
/// from `mudl-gui`'s WebView or `mudl-cli` needs anywhere near this many
/// bytes; it exists only to stop a local client that never sends a newline
/// from growing `handle_connection`'s buffer without limit
/// (`docs/SECURITY.md` Finding 8, "Unbounded request line").
const MAX_REQUEST_LINE_LEN: u64 = 8 * 1024;

/// Upper bound on simultaneously in-flight connections. `serve`'s
/// thread-per-connection accept loop otherwise spawns without limit, so a
/// local client that opens connections and never closes them could exhaust
/// threads/memory (`docs/SECURITY.md` Finding 8, "Unbounded request line").
/// Comfortably above anything a single WebView tab plus long-polls would
/// ever hold open at once.
const MAX_CONCURRENT_CONNECTIONS: usize = 256;

/// Generates a per-instance random token for `DocumentSource::local_token`.
/// Built from two independently-created `RandomState` hashers rather than a
/// `rand` dependency: each `RandomState::new()` draws fresh keys from the
/// OS's own randomness source (see the standard library's
/// `std::collections::hash_map::RandomState`), so hashing a fixed input
/// under two of them yields 128 unpredictable bits without pulling in an
/// external crate for the one thing that needs it.
fn random_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let mut high = RandomState::new().build_hasher();
    high.write_u8(0);
    let mut low = RandomState::new().build_hasher();
    low.write_u8(0);
    format!("{:016x}{:016x}", high.finish(), low.finish())
}

/// Binds a `TcpListener` on `127.0.0.1`, letting the OS pick a free port.
/// Call `.local_addr()` on the result to find out which port was chosen.
pub fn bind() -> io::Result<TcpListener> {
    TcpListener::bind("127.0.0.1:0")
}

/// Runs the accept loop forever, spawning one thread per connection. Only
/// returns if `listener.incoming()` stops yielding items, which in
/// practice means the listener itself was dropped/closed.
///
/// `version` is shared (cheaply cloned per connection) so every connection
/// handler and Phase 6's file watcher coordinate through the same
/// `VersionCounter`. `filesystem` is shared the same way (an `Arc` clone per
/// connection) so `Route::LocalFile` and `Route::Document` can read files
/// through the injected `FileSystem` trait rather than calling `std::fs`
/// directly. `document` identifies which file `Route::Document` renders and
/// how (Phase 10.1) — one `DocumentSource` per server instance, matching
/// Phase 10.6's "one server per open document/tab" design.
pub fn serve(
    listener: TcpListener,
    version: VersionCounter,
    filesystem: Arc<dyn FileSystem>,
    document: Arc<DocumentSource>,
) {
    let in_flight = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT_CONNECTIONS {
                    // Over the cap: drop the connection without spawning a
                    // thread for it. Closing `stream` here sends the client
                    // a reset rather than leaving it hanging.
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let version = version.clone();
                let filesystem = Arc::clone(&filesystem);
                let document = Arc::clone(&document);
                let in_flight = Arc::clone(&in_flight);
                thread::spawn(move || {
                    handle_connection(stream, &version, &filesystem, &document);
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                });
            }
            // A single failed accept (e.g. a connection reset before we
            // could accept it) shouldn't take down the whole server.
            Err(_) => continue,
        }
    }
}

/// Reads one HTTP request line from `stream`, dispatches it, and writes
/// back the response. Errors reading or writing (a client disconnecting
/// mid-request, say) are swallowed here — there's no one left to report
/// them to once the connection is already broken.
///
/// The read is capped at `MAX_REQUEST_LINE_LEN` bytes via `Read::take`: a
/// client that sends that many bytes without a newline gets a 400 rather
/// than an unbounded buffer.
fn handle_connection(
    mut stream: TcpStream,
    version: &VersionCounter,
    filesystem: &Arc<dyn FileSystem>,
    document: &DocumentSource,
) {
    let mut reader = BufReader::new(&stream).take(MAX_REQUEST_LINE_LEN);
    let mut line = String::new();
    let response = match reader.read_line(&mut line) {
        Ok(_) if line.ends_with('\n') => match http::parse_request_line(&line) {
            Some(req) => respond_to(&req, version, filesystem.as_ref(), document),
            None => bad_request(),
        },
        // Either an I/O error, or the line never ended in a newline within
        // the cap — treat both as a malformed request rather than reading
        // further.
        _ => bad_request(),
    };

    let _ = stream.write_all(&response);
    let _ = stream.flush();
}

/// The pure decision logic for turning a parsed request into response
/// bytes: dispatch the route, then decide what to serve for it. Kept
/// separate from `handle_connection` so it's testable without any real
/// socket I/O.
fn respond_to(
    req: &Request,
    version: &VersionCounter,
    filesystem: &dyn FileSystem,
    document: &DocumentSource,
) -> Vec<u8> {
    match routes::dispatch(req) {
        Route::Asset(name) => serve_asset(&name),
        Route::Document(mode) => serve_document(mode, version, filesystem, document),
        Route::LocalFile { token, path } => serve_local_file(&token, &path, filesystem, document),
        Route::WaitForChange(since) => wait_for_change(since, version),
        Route::NotFound => not_found(),
    }
}

/// Reads the document's file fresh from disk on every request (never
/// trusting cached content, so a concurrent external edit is always
/// reflected — the same "re-read fresh" principle the plan calls for
/// elsewhere, e.g. Phase 14.5's comment writes) and renders it via
/// `crate::document::render`. A read error (file missing, permission
/// denied, ...) is reported as a plain 404, matching `serve_local_file`'s
/// same simplification.
fn serve_document(
    mode: routes::Mode,
    version: &VersionCounter,
    filesystem: &dyn FileSystem,
    document: &DocumentSource,
) -> Vec<u8> {
    let bytes = match filesystem.read(&document.path) {
        Ok(bytes) => bytes,
        Err(_) => return not_found(),
    };
    let markdown = String::from_utf8_lossy(&bytes);
    let base_dir = document.path.parent().unwrap_or_else(|| Path::new("."));
    let title = document
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let config = document.config_snapshot();

    let (html, allowed_local_paths) = document::render(
        &markdown,
        base_dir,
        &title,
        mode,
        version.current(),
        &config,
        document.local_token(),
    );
    document.set_allowed_local_paths(allowed_local_paths);
    http::format_response(200, &[("Content-Type", "text/html")], html.as_bytes())
}

/// Blocks (via `version`'s `Condvar`) until the version advances past
/// `since` or `WAIT_TIMEOUT` elapses, then reports whichever version comes
/// back — the new one, or `since` unchanged on timeout (see
/// `VersionCounter::wait_for_change` and plan §2).
fn wait_for_change(since: u64, version: &VersionCounter) -> Vec<u8> {
    let new_version = version.wait_for_change(since, WAIT_TIMEOUT);
    let body = format!("{{\"version\":{new_version}}}");
    http::format_response(
        200,
        &[("Content-Type", "application/json")],
        body.as_bytes(),
    )
}

fn serve_asset(name: &str) -> Vec<u8> {
    match assets::lookup(name) {
        Some(content) => {
            let content_type = crate::mime::lookup(assets::extension_of(name));
            http::format_response(200, &[("Content-Type", content_type)], content.as_bytes())
        }
        None => not_found(),
    }
}

/// Reads `path` via the injected `FileSystem` and serves it with a
/// `Content-Type` from the Phase 4.3 MIME lookup. A read error (not found,
/// permission denied, or anything else `std::fs`/the fake can report) is
/// reported as a plain 404 — matching the plan's Linux-simplification note
/// (§1) that a bare `ENOENT`/`EACCES` distinction from `std::fs` is enough,
/// with no sandboxed denied-vs-missing distinction to preserve.
///
/// `token` must match `document`'s own `local_token` (`docs/SECURITY.md`
/// Finding 2's first hardening step) and `path` must be in `document`'s
/// current allowlist — the set of local paths its last render actually
/// referenced. Either failing is reported as 404, indistinguishable from a
/// path that simply doesn't exist, so a request can't use the response to
/// probe which files are present versus merely disallowed, or to probe
/// whether a guessed token is correct.
fn serve_local_file(
    token: &str,
    path: &str,
    filesystem: &dyn FileSystem,
    document: &DocumentSource,
) -> Vec<u8> {
    if token != document.local_token() || !document.allows_local_path(Path::new(path)) {
        return not_found();
    }
    match filesystem.read(Path::new(path)) {
        Ok(content) => {
            let content_type = crate::mime::lookup(extension_of(path));
            http::format_response(200, &[("Content-Type", content_type)], &content)
        }
        Err(_) => not_found(),
    }
}

/// The file extension (without the leading dot) of a local file path, for
/// use with `mime::lookup` — mirrors `assets::extension_of`'s "last dot
/// segment, or empty string" convention.
fn extension_of(path: &str) -> &str {
    path.rsplit_once('.').map_or("", |(_, ext)| ext)
}

fn not_found() -> Vec<u8> {
    http::format_response(404, &[("Content-Type", "text/plain")], b"Not Found")
}

fn bad_request() -> Vec<u8> {
    http::format_response(400, &[("Content-Type", "text/plain")], b"Bad Request")
}

#[cfg(test)]
mod respond_to_tests {
    use super::*;
    use crate::fs::InMemoryFileSystem;
    use crate::http::Method;
    use std::collections::HashMap;
    use std::time::Instant;

    fn empty_fs() -> InMemoryFileSystem {
        InMemoryFileSystem::new()
    }

    fn document_source() -> DocumentSource {
        DocumentSource::new(PathBuf::from("/docs/notes.md"))
    }

    fn req(path: &str) -> Request {
        Request {
            method: Method::Get,
            path: path.to_string(),
            query: HashMap::new(),
        }
    }

    /// Builds a `/local/<token>/<encoded_path>` request against `document`'s
    /// actual `local_token`, matching what a real rendered page's `<img
    /// src>` would contain.
    fn local_req(document: &DocumentSource, encoded_path: &str) -> Request {
        req(&format!("/local/{}/{encoded_path}", document.local_token()))
    }

    fn wait_req(since: u64) -> Request {
        let mut query = HashMap::new();
        query.insert("since".to_string(), since.to_string());
        Request {
            method: Method::Get,
            path: "/wait".to_string(),
            query,
        }
    }

    fn status_line(response: &[u8]) -> String {
        let text = String::from_utf8_lossy(response);
        text.lines().next().unwrap_or_default().to_string()
    }

    fn body_of(response: &[u8]) -> String {
        let text = String::from_utf8_lossy(response).into_owned();
        text.split("\r\n\r\n")
            .nth(1)
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn known_asset_is_served_with_200_and_content_type() {
        let version = VersionCounter::new();
        let response = respond_to(
            &req("/assets/mud.css"),
            &version,
            &empty_fs(),
            &document_source(),
        );
        let text = String::from_utf8(response.clone()).unwrap();
        assert_eq!(status_line(&response), "HTTP/1.1 200 OK");
        assert!(text.contains("Content-Type: text/css"));
        assert!(text.contains(mudl_core::resources::MUD_CSS));
    }

    #[test]
    fn unknown_asset_is_404() {
        let version = VersionCounter::new();
        let response = respond_to(
            &req("/assets/does-not-exist.css"),
            &version,
            &empty_fs(),
            &document_source(),
        );
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn unknown_route_is_404() {
        let version = VersionCounter::new();
        let response = respond_to(
            &req("/nonexistent"),
            &version,
            &empty_fs(),
            &document_source(),
        );
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn document_route_renders_the_configured_file() {
        let version = VersionCounter::new();
        let filesystem = InMemoryFileSystem::new();
        filesystem.insert("/docs/notes.md", b"# Hello".to_vec());

        let response = respond_to(
            &req("/"),
            &version,
            &filesystem,
            &document_source(),
        );

        assert_eq!(status_line(&response), "HTTP/1.1 200 OK");
        let text = String::from_utf8_lossy(&response).into_owned();
        assert!(text.contains("Content-Type: text/html"));
        assert!(text.contains("<h1"));
        assert!(text.contains("up-mode-output"));
    }

    #[test]
    fn set_config_changes_what_the_next_request_renders() {
        let version = VersionCounter::new();
        let filesystem = InMemoryFileSystem::new();
        filesystem.insert("/docs/notes.md", b"# Hello".to_vec());
        let document = document_source();

        document.set_config(DocumentConfig {
            theme_css_name: "theme-riot.css",
            ..DocumentConfig::default()
        });

        let response = respond_to(&req("/"), &version, &filesystem, &document);
        let text = String::from_utf8_lossy(&response).into_owned();
        assert!(text.contains("Theme: Riot"));
    }

    #[test]
    fn document_route_with_mode_down_renders_down_mode() {
        let version = VersionCounter::new();
        let filesystem = InMemoryFileSystem::new();
        filesystem.insert("/docs/notes.md", b"line one".to_vec());

        let mut query = HashMap::new();
        query.insert("mode".to_string(), "down".to_string());
        let req = Request {
            method: Method::Get,
            path: "/".to_string(),
            query,
        };
        let response = respond_to(
            &req,
            &version,
            &filesystem,
            &document_source(),
        );

        assert_eq!(status_line(&response), "HTTP/1.1 200 OK");
        let text = String::from_utf8_lossy(&response).into_owned();
        assert!(text.contains("down-mode-output"));
    }

    #[test]
    fn document_route_missing_file_is_404() {
        let version = VersionCounter::new();
        let response = respond_to(
            &req("/"),
            &version,
            &empty_fs(),
            &document_source(),
        );
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    /// Performs a `/` request first so the document's render populates its
    /// allowlist, matching how a real WebView always loads the page before
    /// it can request any image the page embeds.
    fn render_document_first(
        version: &VersionCounter,
        filesystem: &dyn FileSystem,
        document: &DocumentSource,
    ) {
        respond_to(&req("/"), version, filesystem, document);
    }

    #[test]
    fn local_file_route_present_in_filesystem_is_200_with_content_type_and_body() {
        let version = VersionCounter::new();
        let filesystem = InMemoryFileSystem::new();
        filesystem.insert("/docs/notes.md", b"![alt](photo.png)".to_vec());
        filesystem.insert("/docs/photo.png", b"fake-png-bytes".to_vec());
        let document = document_source();
        render_document_first(&version, &filesystem, &document);

        let response = respond_to(
            &local_req(&document, "%2Fdocs%2Fphoto.png"),
            &version,
            &filesystem,
            &document,
        );

        assert_eq!(status_line(&response), "HTTP/1.1 200 OK");
        let text = String::from_utf8_lossy(&response).into_owned();
        assert!(text.contains("Content-Type: image/png"));
        assert_eq!(body_of(&response), "fake-png-bytes");
    }

    #[test]
    fn local_file_route_with_wrong_token_is_404_even_when_path_is_allowed() {
        // Second half of Finding 2's hardening: even a path the document
        // legitimately references must not be served if the request's
        // token doesn't match this `DocumentSource`'s own.
        let version = VersionCounter::new();
        let filesystem = InMemoryFileSystem::new();
        filesystem.insert("/docs/notes.md", b"![alt](photo.png)".to_vec());
        filesystem.insert("/docs/photo.png", b"fake-png-bytes".to_vec());
        let document = document_source();
        render_document_first(&version, &filesystem, &document);

        let response = respond_to(
            &req("/local/wrong-token/%2Fdocs%2Fphoto.png"),
            &version,
            &filesystem,
            &document,
        );
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn local_file_route_absent_from_filesystem_is_404() {
        let version = VersionCounter::new();
        let filesystem = InMemoryFileSystem::new();
        filesystem.insert("/docs/notes.md", b"![alt](missing.md)".to_vec());
        let document = document_source();
        render_document_first(&version, &filesystem, &document);

        let response = respond_to(
            &local_req(&document, "%2Fdocs%2Fmissing.md"),
            &version,
            &filesystem,
            &document,
        );
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn local_file_route_not_referenced_by_the_document_is_404_even_if_present_on_disk() {
        // The core of Finding 2: a path that exists and is readable, but
        // that the current document never referenced, must not be served —
        // otherwise `/local/<any-path>` is an arbitrary file read.
        let version = VersionCounter::new();
        let filesystem = InMemoryFileSystem::new();
        filesystem.insert("/docs/notes.md", b"no images here".to_vec());
        filesystem.insert("/etc/passwd", b"root:x:0:0::/root:/bin/bash".to_vec());
        let document = document_source();
        render_document_first(&version, &filesystem, &document);

        let response = respond_to(
            &local_req(&document, "%2Fetc%2Fpasswd"),
            &version,
            &filesystem,
            &document,
        );
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn local_file_route_requested_before_any_document_render_is_404() {
        let version = VersionCounter::new();
        let filesystem = InMemoryFileSystem::new();
        filesystem.insert("/docs/photo.png", b"fake-png-bytes".to_vec());
        let document = document_source();

        let response = respond_to(
            &local_req(&document, "%2Fdocs%2Fphoto.png"),
            &version,
            &filesystem,
            &document,
        );
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn wait_route_unblocks_promptly_when_version_bumped() {
        let version = VersionCounter::new();
        let bumper = version.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            bumper.bump();
        });

        let start = Instant::now();
        let response = respond_to(
            &wait_req(0),
            &version,
            &empty_fs(),
            &document_source(),
        );
        assert!(start.elapsed() < WAIT_TIMEOUT);
        assert_eq!(status_line(&response), "HTTP/1.1 200 OK");
        assert_eq!(body_of(&response), r#"{"version":1}"#);
    }

    #[test]
    fn wait_route_times_out_with_unchanged_version() {
        let version = VersionCounter::new();
        let response = respond_to(
            &wait_req(0),
            &version,
            &empty_fs(),
            &document_source(),
        );
        assert_eq!(status_line(&response), "HTTP/1.1 200 OK");
        assert_eq!(body_of(&response), r#"{"version":0}"#);
    }
}

#[cfg(test)]
mod handle_connection_tests {
    use super::*;
    use crate::fs::InMemoryFileSystem;

    /// Runs `handle_connection` against a real loopback socket: spawns an
    /// acceptor thread that reads one connection and hands it to
    /// `handle_connection`, connects a client, writes `request_bytes`, and
    /// returns whatever came back before the acceptor thread closes the
    /// socket.
    fn run_request(request_bytes: &[u8]) -> Vec<u8> {
        let listener = bind().unwrap();
        let addr = listener.local_addr().unwrap();
        let version = VersionCounter::new();
        let filesystem: Arc<dyn FileSystem> = Arc::new(InMemoryFileSystem::new());
        let document = Arc::new(DocumentSource::new(PathBuf::from("/docs/notes.md")));

        let acceptor = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(stream, &version, &filesystem, &document);
        });

        let mut client = TcpStream::connect(addr).unwrap();
        let mut writer = client.try_clone().unwrap();
        let request_bytes = request_bytes.to_vec();
        // Writes on their own thread: once the server has read enough to
        // decide the request is malformed (the oversized-request-line
        // case), it responds and closes its side without draining the
        // rest of `request_bytes`, which can otherwise turn the write into
        // a `ConnectionReset` on this end. The read below only needs
        // whatever the server sends back.
        let writer_thread = thread::spawn(move || {
            let _ = writer.write_all(&request_bytes);
            let _ = writer.shutdown(std::net::Shutdown::Write);
        });
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        acceptor.join().unwrap();
        let _ = writer_thread.join();
        response
    }

    #[test]
    fn ordinary_request_line_is_served_normally() {
        let response = run_request(b"GET /assets/mud.css HTTP/1.1\r\n\r\n");
        let text = String::from_utf8_lossy(&response);
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
    }

    #[test]
    fn request_line_over_the_cap_with_no_newline_is_a_400() {
        // Exactly `MAX_REQUEST_LINE_LEN` bytes, no trailing newline at all —
        // sending anything past the cap would leave unread bytes in the
        // socket when the server closes it, which triggers a spurious
        // `ConnectionReset` on this end rather than exercising the cap.
        let request = vec![b'a'; MAX_REQUEST_LINE_LEN as usize];

        let response = run_request(&request);
        let text = String::from_utf8_lossy(&response);
        assert!(text.starts_with("HTTP/1.1 400 Bad Request"), "{text}");
    }
}

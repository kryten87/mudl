//! Black-box integration test for the wired-up server loop (Phase 4, step
//! 4.5 of `docs/IMPLEMENTATION-PLAN.md`). Starts a real server on an
//! OS-assigned port and talks to it over an actual `TcpStream`, the same
//! way a browser/WebView would — no in-process shortcuts.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use mudl_server::fs::{FileSystem, InMemoryFileSystem};
use mudl_server::server::DocumentSource;
use mudl_server::version::VersionCounter;
use std::path::PathBuf;

/// The document path every test server in this file is configured with.
/// Most tests don't care about `Route::Document` at all; the ones that do
/// insert content at this path into their filesystem fake.
const DOCUMENT_PATH: &str = "/docs/notes.md";

/// Binds the server to `127.0.0.1:0`, spawns its accept loop on a
/// background thread, and returns the address it ended up listening on
/// along with the `VersionCounter` it was wired up with, so a test can bump
/// it programmatically.
fn start_server() -> (SocketAddr, VersionCounter) {
    let (addr, version, _document) = start_server_with_fs(Arc::new(InMemoryFileSystem::new()));
    (addr, version)
}

/// Same as `start_server`, but with a caller-supplied filesystem, for tests
/// that exercise `/local/<token>/<path>` (or `/`) against known fake
/// contents. Also returns the `DocumentSource` handle so a test can read its
/// `local_token()` — the same way a real rendered page's `<img src>` would
/// carry it — to build a valid `/local/` request (`docs/SECURITY.md`
/// Finding 2's first hardening step).
fn start_server_with_fs(
    filesystem: Arc<dyn FileSystem>,
) -> (SocketAddr, VersionCounter, Arc<DocumentSource>) {
    let listener = mudl_server::server::bind().expect("failed to bind test server");
    let addr = listener.local_addr().expect("failed to read local addr");
    let version = VersionCounter::new();
    let server_version = version.clone();
    let document = Arc::new(DocumentSource::new(PathBuf::from(DOCUMENT_PATH)));
    let document_for_server = Arc::clone(&document);
    thread::spawn(move || {
        mudl_server::server::serve(listener, server_version, filesystem, document_for_server)
    });
    (addr, version, document)
}

/// Sends a raw `GET <path> HTTP/1.1` request and returns the full response
/// bytes read until the peer closes the connection.
fn get(addr: SocketAddr, path: &str) -> Vec<u8> {
    // The accept loop's background thread may not have reached `accept()`
    // yet the instant it's spawned; retry the connect briefly rather than
    // introducing a flaky fixed sleep.
    let mut stream = connect_with_retry(addr);
    // `Host` has to name exactly the address the server bound to
    // (`docs/SECURITY.md` Finding 2's `Host`-check hardening step) — a real
    // WebView navigating to `http://127.0.0.1:<port>/` sends exactly this.
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("failed to write request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("failed to read response");
    response
}

fn connect_with_retry(addr: SocketAddr) -> TcpStream {
    let mut last_err = None;
    for _ in 0..50 {
        match TcpStream::connect(addr) {
            Ok(stream) => return stream,
            Err(err) => {
                last_err = Some(err);
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    panic!("could not connect to test server: {last_err:?}");
}

fn header_value<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    response
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}: ")))
}

fn split_head_and_body(response: &[u8]) -> (String, &[u8]) {
    let separator = b"\r\n\r\n";
    let split_at = response
        .windows(separator.len())
        .position(|window| window == separator)
        .expect("response missing header/body separator");
    let head = String::from_utf8_lossy(&response[..split_at]).into_owned();
    let body = &response[split_at + separator.len()..];
    (head, body)
}

#[test]
fn known_asset_is_served_with_its_embedded_content() {
    let (addr, _version) = start_server();
    let response = get(addr, "/assets/mud.css");
    let (head, body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(header_value(&head, "Content-Type"), Some("text/css"));
    assert_eq!(body, mudl_core::resources::MUD_CSS.as_bytes());
}

#[test]
fn unknown_route_is_404() {
    let (addr, _version) = start_server();
    let response = get(addr, "/this-does-not-exist");
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 404 Not Found"));
}

#[test]
fn unknown_asset_name_is_404() {
    let (addr, _version) = start_server();
    let response = get(addr, "/assets/does-not-exist.css");
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 404 Not Found"));
}

#[test]
fn document_route_missing_file_is_404() {
    let (addr, _version) = start_server();
    let response = get(addr, "/");
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 404 Not Found"));
}

#[test]
fn document_route_renders_the_configured_file() {
    let filesystem = InMemoryFileSystem::new();
    filesystem.insert(DOCUMENT_PATH, b"# Hello".to_vec());
    let (addr, _version, _document) = start_server_with_fs(Arc::new(filesystem));

    let response = get(addr, "/");
    let (head, body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(header_value(&head, "Content-Type"), Some("text/html"));
    let text = String::from_utf8_lossy(body);
    assert!(text.contains("<h1"));
    assert!(text.contains("up-mode-output"));
}

#[test]
fn local_file_route_serves_bytes_present_in_the_filesystem_and_referenced_by_the_document() {
    let filesystem = InMemoryFileSystem::new();
    filesystem.insert(DOCUMENT_PATH, b"![alt](photo.png)".to_vec());
    filesystem.insert("/docs/photo.png", b"fake-png-bytes".to_vec());
    let (addr, _version, document) = start_server_with_fs(Arc::new(filesystem));

    // The document has to be rendered at least once before its allowlist
    // (the set of local paths its own images resolved to) is populated —
    // exactly what a real WebView does by loading `/` before requesting
    // any `/local/` image it found there.
    get(addr, "/");
    let response = get(
        addr,
        &format!("/local/{}/%2Fdocs%2Fphoto.png", document.local_token()),
    );
    let (head, body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(header_value(&head, "Content-Type"), Some("image/png"));
    assert_eq!(body, b"fake-png-bytes");
}

#[test]
fn local_file_route_missing_from_filesystem_is_404() {
    let filesystem = InMemoryFileSystem::new();
    filesystem.insert(DOCUMENT_PATH, b"![alt](missing.md)".to_vec());
    let (addr, _version, document) = start_server_with_fs(Arc::new(filesystem));

    get(addr, "/");
    let response = get(
        addr,
        &format!("/local/{}/%2Fdocs%2Fmissing.md", document.local_token()),
    );
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 404 Not Found"));
}

#[test]
fn local_file_route_not_referenced_by_the_document_is_404_even_when_present_on_disk() {
    // Regression test for `docs/SECURITY.md` Finding 2: `/local/<path>`
    // used to serve any readable path a request named, letting any local
    // process (or a DNS-rebound web page) read arbitrary files such as
    // `~/.ssh/id_rsa`. It must now be confined to paths the document
    // itself referenced.
    let filesystem = InMemoryFileSystem::new();
    filesystem.insert(DOCUMENT_PATH, b"no images here".to_vec());
    filesystem.insert("/etc/passwd", b"root:x:0:0::/root:/bin/bash".to_vec());
    let (addr, _version, document) = start_server_with_fs(Arc::new(filesystem));

    get(addr, "/");
    let response = get(
        addr,
        &format!("/local/{}/%2Fetc%2Fpasswd", document.local_token()),
    );
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 404 Not Found"));
}

#[test]
fn local_file_route_with_wrong_token_is_404_even_when_path_is_allowed() {
    // Second half of `docs/SECURITY.md` Finding 2's first hardening step:
    // even a path the document legitimately references must not be served
    // if the request's token doesn't match this server instance's own.
    let filesystem = InMemoryFileSystem::new();
    filesystem.insert(DOCUMENT_PATH, b"![alt](photo.png)".to_vec());
    filesystem.insert("/docs/photo.png", b"fake-png-bytes".to_vec());
    let (addr, _version, _document) = start_server_with_fs(Arc::new(filesystem));

    get(addr, "/");
    let response = get(addr, "/local/wrong-token/%2Fdocs%2Fphoto.png");
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 404 Not Found"));
}

#[test]
fn request_with_host_header_naming_a_different_address_is_403() {
    // Regression test for `docs/SECURITY.md` Finding 2's DNS-rebinding
    // vector: a remote page can point a hostname it controls at
    // 127.0.0.1 and have the browser send that hostname as `Host` while
    // still connecting to this loopback port.
    let (addr, _version) = start_server();
    let mut stream = connect_with_retry(addr);
    stream
        .write_all(b"GET /assets/mud.css HTTP/1.1\r\nHost: evil.example\r\n\r\n")
        .expect("failed to write request");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("failed to read response");
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 403 Forbidden"));
}

#[test]
fn wait_route_unblocks_promptly_when_version_bumped() {
    let (addr, version) = start_server();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        version.bump();
    });

    let start = Instant::now();
    let response = get(addr, "/wait?since=0");
    let (head, body) = split_head_and_body(&response);

    assert!(
        start.elapsed() < Duration::from_millis(400),
        "long-poll took {:?}, expected it to unblock promptly on bump",
        start.elapsed()
    );
    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(
        header_value(&head, "Content-Type"),
        Some("application/json")
    );
    assert_eq!(body, br#"{"version":1}"#);
}

#[test]
fn wait_route_times_out_with_unchanged_version() {
    let (addr, _version) = start_server();

    let start = Instant::now();
    let response = get(addr, "/wait?since=0");
    let (head, body) = split_head_and_body(&response);

    assert!(
        start.elapsed() < Duration::from_secs(1),
        "long-poll timeout took {:?}, expected it to stay under a second",
        start.elapsed()
    );
    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(body, br#"{"version":0}"#);
}

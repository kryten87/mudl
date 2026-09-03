//! Black-box integration test for Phase 6, step 6.2's background-thread
//! wiring (`docs/IMPLEMENTATION-PLAN.md`): a `mudl_watch::PollingChangeSource`
//! spawned onto a background thread, watching a *real* temp file, bumping a
//! real `mudl-server` `VersionCounter` on change. This is the literal
//! integration test the plan calls for: "writes to a real temp file and
//! asserts the HTTP `/wait` endpoint unblocks within a bounded time" — no
//! in-memory fakes anywhere in this file, matching `mudl-watch`'s
//! `RealFileSystem`/`RealClock` and a real `TcpStream` against a real
//! server, the same way `tests/http_server.rs` talks to the server.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use mudl_server::fs::InMemoryFileSystem;
use mudl_server::server::DocumentSource;
use mudl_server::version::VersionCounter;
use mudl_watch::{PollingChangeSource, RealClock, RealFileSystem};

/// Binds the server to `127.0.0.1:0`, spawns its accept loop on a
/// background thread, and returns the address it ended up listening on
/// along with the `VersionCounter` it was wired up with. Mirrors
/// `tests/http_server.rs`'s `start_server` helper.
fn start_server() -> (SocketAddr, VersionCounter) {
    let listener = mudl_server::server::bind().expect("failed to bind test server");
    let addr = listener.local_addr().expect("failed to read local addr");
    let version = VersionCounter::new();
    let server_version = version.clone();
    let filesystem = Arc::new(InMemoryFileSystem::new());
    let document = Arc::new(DocumentSource::new(PathBuf::from("/docs/notes.md")));
    thread::spawn(move || {
        mudl_server::server::serve(listener, server_version, filesystem, document)
    });
    (addr, version)
}

/// Sends a raw `GET <path> HTTP/1.1` request and returns the full response
/// bytes read until the peer closes the connection. Mirrors
/// `tests/http_server.rs`'s `get` helper.
fn get(addr: SocketAddr, path: &str) -> Vec<u8> {
    let mut stream = connect_with_retry(addr);
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n");
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

/// A unique path under the system temp dir, so concurrent test runs (and
/// repeated runs against a leftover file from a previous crash) never
/// collide.
fn unique_temp_path(label: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "mudl-watch-integration-{}-{}-{}.md",
        std::process::id(),
        label,
        n
    ))
}

#[test]
fn writing_a_real_temp_file_unblocks_the_wait_endpoint() {
    let path = unique_temp_path("wait-unblocks");
    std::fs::write(&path, b"initial contents").expect("failed to create temp file");

    let (addr, version) = start_server();

    let interval = Duration::from_millis(30);
    let source = PollingChangeSource::new(path.clone(), RealFileSystem, RealClock, interval);
    let watch_version = version.clone();
    let handle = source.spawn(move |_event| watch_version.bump());

    // Let the watcher's baseline poll happen before we mutate the file, so
    // the write below is unambiguously "the" change it reports.
    thread::sleep(interval * 2);

    std::fs::write(&path, b"changed contents").expect("failed to write temp file");

    let start = Instant::now();
    let response = get(addr, "/wait?since=0");
    let (head, body) = split_head_and_body(&response);
    let elapsed = start.elapsed();

    handle.stop();
    std::fs::remove_file(&path).ok();

    assert!(
        elapsed < Duration::from_millis(500),
        "expected /wait to unblock promptly after the real file changed, took {elapsed:?}"
    );
    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(body, br#"{"version":1}"#);
}

//! The impure server loop (Phase 4, step 4.5 of
//! `docs/IMPLEMENTATION-PLAN.md`): bind a `TcpListener`, accept
//! connections, and for each one read a single HTTP request, dispatch it,
//! and write back a response.
//!
//! Thread-per-connection is deliberately simple — `mudl` serves a handful
//! of concurrent requests from a single local WebView, not production web
//! traffic (see the plan's rationale in §10, step 4.5).

use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use crate::assets;
use crate::http::{self, Request};
use crate::routes::{self, Route};
use crate::version::VersionCounter;

/// How long a `/wait` long-poll request blocks before returning the
/// unchanged version. Short enough to keep tests fast, long enough to be a
/// plausible long-poll interval in practice.
const WAIT_TIMEOUT: Duration = Duration::from_millis(500);

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
/// handler and, eventually, Phase 6's file watcher, coordinate through the
/// same `VersionCounter`.
pub fn serve(listener: TcpListener, version: VersionCounter) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let version = version.clone();
                thread::spawn(move || handle_connection(stream, &version));
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
fn handle_connection(mut stream: TcpStream, version: &VersionCounter) {
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }

    let response = match http::parse_request_line(&line) {
        Some(req) => respond_to(&req, version),
        None => bad_request(),
    };

    let _ = stream.write_all(&response);
    let _ = stream.flush();
}

/// The pure decision logic for turning a parsed request into response
/// bytes: dispatch the route, then decide what to serve for it. Kept
/// separate from `handle_connection` so it's testable without any real
/// socket I/O.
fn respond_to(req: &Request, version: &VersionCounter) -> Vec<u8> {
    match routes::dispatch(req) {
        Route::Asset(name) => serve_asset(&name),
        Route::Document | Route::LocalFile(_) => not_implemented(),
        Route::WaitForChange(since) => wait_for_change(since, version),
        Route::NotFound => not_found(),
    }
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

fn not_found() -> Vec<u8> {
    http::format_response(404, &[("Content-Type", "text/plain")], b"Not Found")
}

fn not_implemented() -> Vec<u8> {
    // TODO(Phase 5+): Route::Document needs render_up/render_down wired to
    // a specific file, and Route::LocalFile needs the injected FileSystem
    // trait from Phase 5.2 — both stubbed here until those phases land.
    http::format_response(501, &[("Content-Type", "text/plain")], b"Not Implemented")
}

fn bad_request() -> Vec<u8> {
    http::format_response(400, &[("Content-Type", "text/plain")], b"Bad Request")
}

#[cfg(test)]
mod respond_to_tests {
    use super::*;
    use crate::http::Method;
    use std::collections::HashMap;
    use std::time::Instant;

    fn req(path: &str) -> Request {
        Request {
            method: Method::Get,
            path: path.to_string(),
            query: HashMap::new(),
        }
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
        let response = respond_to(&req("/assets/mud.css"), &version);
        let text = String::from_utf8(response.clone()).unwrap();
        assert_eq!(status_line(&response), "HTTP/1.1 200 OK");
        assert!(text.contains("Content-Type: text/css"));
        assert!(text.contains(mudl_core::resources::MUD_CSS));
    }

    #[test]
    fn unknown_asset_is_404() {
        let version = VersionCounter::new();
        let response = respond_to(&req("/assets/does-not-exist.css"), &version);
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn unknown_route_is_404() {
        let version = VersionCounter::new();
        let response = respond_to(&req("/nonexistent"), &version);
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn document_route_is_501() {
        let version = VersionCounter::new();
        let response = respond_to(&req("/"), &version);
        assert_eq!(status_line(&response), "HTTP/1.1 501 Not Implemented");
    }

    #[test]
    fn local_file_route_is_501() {
        let version = VersionCounter::new();
        let response = respond_to(&req("/local/%2Ftmp%2Fnotes.md"), &version);
        assert_eq!(status_line(&response), "HTTP/1.1 501 Not Implemented");
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
        let response = respond_to(&wait_req(0), &version);
        assert!(start.elapsed() < WAIT_TIMEOUT);
        assert_eq!(status_line(&response), "HTTP/1.1 200 OK");
        assert_eq!(body_of(&response), r#"{"version":1}"#);
    }

    #[test]
    fn wait_route_times_out_with_unchanged_version() {
        let version = VersionCounter::new();
        let response = respond_to(&wait_req(0), &version);
        assert_eq!(status_line(&response), "HTTP/1.1 200 OK");
        assert_eq!(body_of(&response), r#"{"version":0}"#);
    }
}

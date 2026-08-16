//! The impure server loop (Phase 4, step 4.5 of
//! `docs/IMPLEMENTATION-PLAN.md`): bind a `TcpListener`, accept
//! connections, and for each one read a single HTTP request, dispatch it,
//! and write back a response.
//!
//! Thread-per-connection is deliberately simple — `mudl` serves a handful
//! of concurrent requests from a single local WebView, not production web
//! traffic (see the plan's rationale in §10, step 4.5).
//!
//! `Route::WaitForChange` (the `/wait` long-poll endpoint) isn't wired up
//! yet — that's Phase 4.6, which needs the `Condvar`-based version counter
//! from `mudl-watch`. For now it falls through to the same 404 response as
//! an unrecognized route, so the server never crashes or hangs on it.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use crate::assets;
use crate::http::{self, Request};
use crate::routes::{self, Route};

/// Binds a `TcpListener` on `127.0.0.1`, letting the OS pick a free port.
/// Call `.local_addr()` on the result to find out which port was chosen.
pub fn bind() -> io::Result<TcpListener> {
    TcpListener::bind("127.0.0.1:0")
}

/// Runs the accept loop forever, spawning one thread per connection. Only
/// returns if `listener.incoming()` stops yielding items, which in
/// practice means the listener itself was dropped/closed.
pub fn serve(listener: TcpListener) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(move || handle_connection(stream));
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
fn handle_connection(mut stream: TcpStream) {
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }

    let response = match http::parse_request_line(&line) {
        Some(req) => respond_to(&req),
        None => bad_request(),
    };

    let _ = stream.write_all(&response);
    let _ = stream.flush();
}

/// The pure decision logic for turning a parsed request into response
/// bytes: dispatch the route, then decide what to serve for it. Kept
/// separate from `handle_connection` so it's testable without any real
/// socket I/O.
fn respond_to(req: &Request) -> Vec<u8> {
    match routes::dispatch(req) {
        Route::Asset(name) => serve_asset(&name),
        Route::Document | Route::LocalFile(_) => not_implemented(),
        // Phase 4.6 fills this in; until then it's indistinguishable from
        // an unrecognized route.
        Route::WaitForChange(_) | Route::NotFound => not_found(),
    }
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

    fn req(path: &str) -> Request {
        Request {
            method: Method::Get,
            path: path.to_string(),
            query: HashMap::new(),
        }
    }

    fn status_line(response: &[u8]) -> String {
        let text = String::from_utf8_lossy(response);
        text.lines().next().unwrap_or_default().to_string()
    }

    #[test]
    fn known_asset_is_served_with_200_and_content_type() {
        let response = respond_to(&req("/assets/mud.css"));
        let text = String::from_utf8(response.clone()).unwrap();
        assert_eq!(status_line(&response), "HTTP/1.1 200 OK");
        assert!(text.contains("Content-Type: text/css"));
        assert!(text.contains(mudl_core::resources::MUD_CSS));
    }

    #[test]
    fn unknown_asset_is_404() {
        let response = respond_to(&req("/assets/does-not-exist.css"));
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn unknown_route_is_404() {
        let response = respond_to(&req("/nonexistent"));
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn document_route_is_501() {
        let response = respond_to(&req("/"));
        assert_eq!(status_line(&response), "HTTP/1.1 501 Not Implemented");
    }

    #[test]
    fn local_file_route_is_501() {
        let response = respond_to(&req("/local/%2Ftmp%2Fnotes.md"));
        assert_eq!(status_line(&response), "HTTP/1.1 501 Not Implemented");
    }

    #[test]
    fn wait_route_is_404_until_phase_4_6() {
        let response = respond_to(&req("/wait"));
        assert_eq!(status_line(&response), "HTTP/1.1 404 Not Found");
    }
}

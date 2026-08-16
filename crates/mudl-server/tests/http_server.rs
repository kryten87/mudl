//! Black-box integration test for the wired-up server loop (Phase 4, step
//! 4.5 of `docs/IMPLEMENTATION-PLAN.md`). Starts a real server on an
//! OS-assigned port and talks to it over an actual `TcpStream`, the same
//! way a browser/WebView would — no in-process shortcuts.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::time::Duration;

/// Binds the server to `127.0.0.1:0`, spawns its accept loop on a
/// background thread, and returns the address it ended up listening on.
fn start_server() -> SocketAddr {
    let listener = mudl_server::server::bind().expect("failed to bind test server");
    let addr = listener.local_addr().expect("failed to read local addr");
    thread::spawn(move || mudl_server::server::serve(listener));
    addr
}

/// Sends a raw `GET <path> HTTP/1.1` request and returns the full response
/// bytes read until the peer closes the connection.
fn get(addr: SocketAddr, path: &str) -> Vec<u8> {
    // The accept loop's background thread may not have reached `accept()`
    // yet the instant it's spawned; retry the connect briefly rather than
    // introducing a flaky fixed sleep.
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
    let addr = start_server();
    let response = get(addr, "/assets/mud.css");
    let (head, body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(header_value(&head, "Content-Type"), Some("text/css"));
    assert_eq!(body, mudl_core::resources::MUD_CSS.as_bytes());
}

#[test]
fn unknown_route_is_404() {
    let addr = start_server();
    let response = get(addr, "/this-does-not-exist");
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 404 Not Found"));
}

#[test]
fn unknown_asset_name_is_404() {
    let addr = start_server();
    let response = get(addr, "/assets/does-not-exist.css");
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 404 Not Found"));
}

#[test]
fn document_route_is_a_placeholder_response_not_a_hang() {
    let addr = start_server();
    let response = get(addr, "/");
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 501"));
}

#[test]
fn wait_route_does_not_hang_before_phase_4_6() {
    let addr = start_server();
    let response = get(addr, "/wait?since=0");
    let (head, _body) = split_head_and_body(&response);

    assert!(head.starts_with("HTTP/1.1 404 Not Found"));
}

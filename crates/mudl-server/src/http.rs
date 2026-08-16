//! Pure HTTP request-line parsing and response formatting (Phase 4, steps
//! 4.1-4.2 of `docs/IMPLEMENTATION-PLAN.md`).

use std::collections::HashMap;

/// HTTP request method. `mudl-server` only ever needs to distinguish `GET`
/// in practice, but the request line can carry any method token, so unknown
/// methods are preserved rather than rejected at this layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Other(String),
}

/// A parsed HTTP request line (`GET /path?query HTTP/1.1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub query: HashMap<String, String>,
}

/// Parses a single HTTP request line into a [`Request`]. Returns `None` for
/// anything malformed (wrong token count, missing method, missing path).
pub fn parse_request_line(line: &str) -> Option<Request> {
    todo!("Phase 4.1: {line}")
}

/// Formats a complete HTTP response (status line + headers + body) as raw
/// bytes ready to write to a socket. `Content-Length` is always computed
/// from `body.len()`, never trusted from `headers`.
pub fn format_response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    todo!("Phase 4.2: {status} {headers:?} {}", body.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_types_compile() {
        let req = Request {
            method: Method::Get,
            path: "/".to_string(),
            query: HashMap::new(),
        };
        assert_eq!(req.method, Method::Get);
    }
}

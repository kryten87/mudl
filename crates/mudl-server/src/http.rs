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

/// Decodes `%XX` percent-escapes in `s`. A `%` not followed by two valid hex
/// digits is passed through literally rather than rejecting the whole
/// input — a client sending a stray `%` shouldn't be able to turn a decode
/// error into a dropped request.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses `a=1&b=2`-style query strings. Pairs with no `=` are kept as a
/// key with an empty value; pairs with an empty key (e.g. from a stray `&&`
/// or a leading `=value`) are dropped as not meaningfully addressable.
fn parse_query(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if query.is_empty() {
        return map;
    }
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        let key = percent_decode(key);
        if key.is_empty() {
            continue;
        }
        map.insert(key, percent_decode(value));
    }
    map
}

fn parse_method(token: &str) -> Method {
    match token {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        other => Method::Other(other.to_string()),
    }
}

/// A valid HTTP version token is `HTTP/` followed by `<digits>.<digits>`.
fn is_valid_http_version(token: &str) -> bool {
    let Some(version) = token.strip_prefix("HTTP/") else {
        return false;
    };
    let Some((major, minor)) = version.split_once('.') else {
        return false;
    };
    !major.is_empty()
        && !minor.is_empty()
        && major.bytes().all(|b| b.is_ascii_digit())
        && minor.bytes().all(|b| b.is_ascii_digit())
}

/// Parses a single HTTP request line into a [`Request`]. Returns `None` for
/// anything malformed (wrong token count, missing method, missing path).
pub fn parse_request_line(line: &str) -> Option<Request> {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut parts = line.splitn(3, ' ');
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;

    if method.is_empty() || target.is_empty() || !is_valid_http_version(version) {
        return None;
    }

    let (raw_path, raw_query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };

    Some(Request {
        method: parse_method(method),
        path: percent_decode(raw_path),
        query: parse_query(raw_query),
    })
}

const fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

/// Formats a complete HTTP response (status line + headers + body) as raw
/// bytes ready to write to a socket. `Content-Length` is always computed
/// from `body.len()`, never trusted from `headers`.
pub fn format_response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status} {}\r\n", reason_phrase(status)).into_bytes();

    for (name, value) in headers {
        if name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    out
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

    mod parse_request_line_tests {
        use super::*;

        #[test]
        fn well_formed_request_line() {
            let req = parse_request_line("GET /foo?a=1&b=2 HTTP/1.1").unwrap();
            assert_eq!(req.method, Method::Get);
            assert_eq!(req.path, "/foo");
            assert_eq!(req.query.get("a").map(String::as_str), Some("1"));
            assert_eq!(req.query.get("b").map(String::as_str), Some("2"));
            assert_eq!(req.query.len(), 2);
        }

        #[test]
        fn missing_http_version() {
            assert_eq!(parse_request_line("GET /foo"), None);
        }

        #[test]
        fn missing_method() {
            assert_eq!(parse_request_line("/foo HTTP/1.1"), None);
        }

        #[test]
        fn empty_path() {
            assert_eq!(parse_request_line("GET  HTTP/1.1"), None);
        }

        #[test]
        fn percent_encoded_path_is_decoded() {
            let req = parse_request_line("GET /foo%20bar HTTP/1.1").unwrap();
            assert_eq!(req.path, "/foo bar");
        }

        #[test]
        fn percent_encoded_query_is_decoded() {
            let req = parse_request_line("GET /search?q=a%2Bb HTTP/1.1").unwrap();
            assert_eq!(req.query.get("q").map(String::as_str), Some("a+b"));
        }

        #[test]
        fn malformed_query_string() {
            let req = parse_request_line("GET /foo?a=1&&flag&=orphan HTTP/1.1").unwrap();
            assert_eq!(req.query.get("a").map(String::as_str), Some("1"));
            assert_eq!(req.query.get("flag").map(String::as_str), Some(""));
            assert_eq!(req.query.len(), 2);
        }

        #[test]
        fn empty_input_line() {
            assert_eq!(parse_request_line(""), None);
        }

        #[test]
        fn invalid_percent_escape_is_passed_through_without_panicking() {
            let req = parse_request_line("GET /foo%2 HTTP/1.1").unwrap();
            assert_eq!(req.path, "/foo%2");
        }

        #[test]
        fn no_query_string_yields_empty_map() {
            let req = parse_request_line("GET /foo HTTP/1.1").unwrap();
            assert_eq!(req.path, "/foo");
            assert!(req.query.is_empty());
        }

        #[test]
        fn unknown_method_is_preserved() {
            let req = parse_request_line("PATCH /foo HTTP/1.1").unwrap();
            assert_eq!(req.method, Method::Other("PATCH".to_string()));
        }
    }

    mod format_response_tests {
        use super::*;

        #[test]
        fn ok_with_body() {
            let out = format_response(200, &[("Content-Type", "text/plain")], b"hello");
            let text = String::from_utf8(out).unwrap();
            assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(text.contains("Content-Type: text/plain\r\n"));
            assert!(text.contains("Content-Length: 5\r\n"));
            assert!(text.ends_with("\r\n\r\nhello"));
        }

        #[test]
        fn not_found_with_no_body() {
            let out = format_response(404, &[], b"");
            let text = String::from_utf8(out).unwrap();
            assert_eq!(text, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        }

        #[test]
        fn multiple_headers_joined_with_crlf() {
            let out = format_response(
                200,
                &[("Content-Type", "text/html"), ("X-Custom", "value")],
                b"",
            );
            let text = String::from_utf8(out).unwrap();
            assert!(text.contains("Content-Type: text/html\r\nX-Custom: value\r\n"));
        }

        #[test]
        fn caller_supplied_content_length_is_ignored() {
            let out = format_response(200, &[("Content-Length", "9999")], b"hi");
            let text = String::from_utf8(out).unwrap();
            assert_eq!(text.matches("Content-Length").count(), 1);
            assert!(text.contains("Content-Length: 2\r\n"));
        }
    }
}

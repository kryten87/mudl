//! Pure route dispatch, no I/O (Phase 4, step 4.4 of
//! `docs/IMPLEMENTATION-PLAN.md`).

use crate::http::Request;

/// Which of `render_up`/`render_down` a `/` request wants (Phase 10.2's
/// mode toggle re-navigates with `?mode=down`; its absence means Up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Up,
    Down,
}

/// The route a request maps to, decided purely from its path/query — no
/// filesystem or socket access happens here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `/` — the rendered document, in the given mode.
    Document(Mode),
    /// `/assets/<name>` — a bundled, embedded static asset.
    Asset(String),
    /// `/local/<token>/<percent-encoded-path>` — a local file referenced by
    /// the document (e.g. a relative image), percent-decoded. `token` must
    /// match the serving `DocumentSource`'s own per-instance random token
    /// before `server.rs`'s `serve_local_file` will treat `path` as
    /// legitimate (`docs/SECURITY.md` Finding 2's first hardening step) —
    /// dispatch only extracts the two segments, it doesn't itself check the
    /// token, since routing here has no access to the `DocumentSource` that
    /// knows the expected value.
    LocalFile {
        token: String,
        path: String,
    },
    /// `/wait?since=N` — long-poll for the next change past version `N`.
    WaitForChange(u64),
    NotFound,
}

/// Maps a parsed [`Request`] to the [`Route`] it should be served by.
pub fn dispatch(req: &Request) -> Route {
    match req.path.as_str() {
        "/" => Route::Document(parse_mode(req.query.get("mode"))),
        "/wait" => match parse_since(req.query.get("since")) {
            Some(since) => Route::WaitForChange(since),
            None => Route::NotFound,
        },
        path => {
            if let Some(name) = path.strip_prefix("/assets/") {
                if name.is_empty() {
                    Route::NotFound
                } else {
                    Route::Asset(name.to_string())
                }
            } else if let Some(after_prefix) = path.strip_prefix("/local/") {
                match after_prefix.split_once('/') {
                    Some((token, encoded)) if !token.is_empty() && !encoded.is_empty() => {
                        match percent_decode(encoded) {
                            Some(path) => Route::LocalFile {
                                token: token.to_string(),
                                path,
                            },
                            None => Route::NotFound,
                        }
                    }
                    _ => Route::NotFound,
                }
            } else {
                Route::NotFound
            }
        }
    }
}

/// `?mode=down` selects Down mode; anything else (including the param's
/// absence) is Up — there's no malformed-value error case here the way
/// there is for `since`, since an unrecognized mode is just as reasonably
/// "not Down" as a missing one.
fn parse_mode(raw: Option<&String>) -> Mode {
    match raw.map(String::as_str) {
        Some("down") => Mode::Down,
        _ => Mode::Up,
    }
}

/// `None` (missing param) defaults to `Some(0)`; a present-but-unparseable
/// value is treated as malformed, not silently defaulted.
fn parse_since(raw: Option<&String>) -> Option<u64> {
    match raw {
        None => Some(0),
        Some(s) => s.parse::<u64>().ok(),
    }
}

/// Public so `mudl-gui`'s link-navigation handler (`/local-md/`,
/// `/local-file/`) can decode the same percent-encoding `mudl-core::template`
/// uses to build those hrefs, without duplicating this logic.
pub fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                let hex_str = std::str::from_utf8(hex).ok()?;
                out.push(u8::from_str_radix(hex_str, 16).ok()?);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Method;

    fn req(path: &str, query: &[(&str, &str)]) -> Request {
        Request {
            method: Method::Get,
            path: path.to_string(),
            query: query
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn root_with_no_mode_is_document_up() {
        assert_eq!(dispatch(&req("/", &[])), Route::Document(Mode::Up));
    }

    #[test]
    fn root_with_mode_down_is_document_down() {
        assert_eq!(
            dispatch(&req("/", &[("mode", "down")])),
            Route::Document(Mode::Down)
        );
    }

    #[test]
    fn root_with_unrecognized_mode_is_document_up() {
        assert_eq!(
            dispatch(&req("/", &[("mode", "sideways")])),
            Route::Document(Mode::Up)
        );
    }

    #[test]
    fn assets_path_is_asset_route() {
        assert_eq!(
            dispatch(&req("/assets/mud.css", &[])),
            Route::Asset("mud.css".to_string())
        );
    }

    #[test]
    fn assets_path_with_empty_name_is_not_found() {
        assert_eq!(dispatch(&req("/assets/", &[])), Route::NotFound);
    }

    #[test]
    fn local_path_token_and_path_are_split_and_path_is_percent_decoded() {
        assert_eq!(
            dispatch(&req("/local/tok123/%2Fhome%2Fuser%2Fnotes.md", &[])),
            Route::LocalFile {
                token: "tok123".to_string(),
                path: "/home/user/notes.md".to_string(),
            }
        );
    }

    #[test]
    fn local_path_missing_segment_is_not_found() {
        assert_eq!(dispatch(&req("/local/", &[])), Route::NotFound);
    }

    #[test]
    fn local_path_with_only_a_token_and_no_path_is_not_found() {
        assert_eq!(dispatch(&req("/local/tok123", &[])), Route::NotFound);
        assert_eq!(dispatch(&req("/local/tok123/", &[])), Route::NotFound);
    }

    #[test]
    fn local_path_with_empty_token_is_not_found() {
        assert_eq!(
            dispatch(&req("/local//%2Fetc%2Fpasswd", &[])),
            Route::NotFound
        );
    }

    #[test]
    fn local_path_invalid_percent_encoding_is_not_found() {
        assert_eq!(dispatch(&req("/local/tok123/%zz", &[])), Route::NotFound);
    }

    #[test]
    fn local_path_truncated_percent_encoding_is_not_found() {
        assert_eq!(dispatch(&req("/local/tok123/abc%2", &[])), Route::NotFound);
    }

    #[test]
    fn wait_with_since_is_wait_for_change() {
        assert_eq!(
            dispatch(&req("/wait", &[("since", "42")])),
            Route::WaitForChange(42)
        );
    }

    #[test]
    fn wait_without_since_defaults_to_zero() {
        assert_eq!(dispatch(&req("/wait", &[])), Route::WaitForChange(0));
    }

    #[test]
    fn wait_with_unparseable_since_is_not_found() {
        assert_eq!(
            dispatch(&req("/wait", &[("since", "not-a-number")])),
            Route::NotFound
        );
    }

    #[test]
    fn wait_with_negative_since_is_not_found() {
        assert_eq!(dispatch(&req("/wait", &[("since", "-1")])), Route::NotFound);
    }

    #[test]
    fn unknown_path_is_not_found() {
        assert_eq!(dispatch(&req("/nonexistent", &[])), Route::NotFound);
    }
}

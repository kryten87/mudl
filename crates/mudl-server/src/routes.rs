//! Pure route dispatch, no I/O (Phase 4, step 4.4 of
//! `docs/IMPLEMENTATION-PLAN.md`).

use crate::http::Request;

/// The route a request maps to, decided purely from its path/query — no
/// filesystem or socket access happens here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `/` — the rendered document.
    Document,
    /// `/assets/<name>` — a bundled, embedded static asset.
    Asset(String),
    /// `/local/<percent-encoded-path>` — a local file referenced by the
    /// document (e.g. a relative image), percent-decoded.
    LocalFile(String),
    /// `/wait?since=N` — long-poll for the next change past version `N`.
    WaitForChange(u64),
    NotFound,
}

/// Maps a parsed [`Request`] to the [`Route`] it should be served by.
pub fn dispatch(req: &Request) -> Route {
    todo!("Phase 4.4: {req:?}")
}

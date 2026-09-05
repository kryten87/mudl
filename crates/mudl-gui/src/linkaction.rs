//! Pure decision logic for what a WebView navigation to a given URI should
//! do: let WebKit apply its own default policy (in-page anchors, this tab's
//! own `mudl-server` pages), open a new mudl window (`/local-md/...`, from
//! `mudl_core::template::rewrite_local_link_hrefs`), open the OS's default
//! browser/mail client (`http(s)://`/`mailto:`), or hand off to `xdg-open`
//! (`/local-file/...`). Kept separate from `window.rs`'s WebKit signal
//! wiring so it's unit-testable without GTK.

use std::path::PathBuf;

use mudl_core::images::is_external_source;

const LOCAL_MD_PREFIX: &str = "/local-md/";
const LOCAL_FILE_PREFIX: &str = "/local-file/";

/// What a navigation to a given URI should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkAction {
    /// Not a link this handler should intercept — let WebKit apply its
    /// default navigation policy.
    Default,
    /// A local `.md`/`.markdown` file — open a new mudl window for it.
    OpenNewWindow(PathBuf),
    /// `http(s)://`/`mailto:` (or anything else the OS has a default
    /// handler for) — hand off to `gio::AppInfo::launch_default_for_uri`.
    OpenExternally(String),
    /// Any other local file — hand off to `xdg-open`.
    OpenWithSystemDefault(PathBuf),
}

/// Classifies `uri` (the navigation target WebKit reports) given
/// `server_addr` (this tab's own `mudl-server` instance's `host:port`, e.g.
/// `"127.0.0.1:53211"`) — needed to tell a same-origin navigation (this
/// tab's own `/`, `/?mode=down`, or an in-page `#anchor`) apart from the
/// `/local-md/`/`/local-file/` routes `rewrite_local_link_hrefs` produces,
/// which also live on that same origin.
pub fn classify(uri: &str, server_addr: &str) -> LinkAction {
    let own_origin = format!("http://{server_addr}");
    if let Some(path) = uri.strip_prefix(own_origin.as_str()) {
        if let Some(encoded) = path.strip_prefix(LOCAL_MD_PREFIX) {
            return decode_path(encoded)
                .map(LinkAction::OpenNewWindow)
                .unwrap_or(LinkAction::Default);
        }
        if let Some(encoded) = path.strip_prefix(LOCAL_FILE_PREFIX) {
            return decode_path(encoded)
                .map(LinkAction::OpenWithSystemDefault)
                .unwrap_or(LinkAction::Default);
        }
        return LinkAction::Default;
    }

    if is_external_source(uri) {
        return LinkAction::OpenExternally(uri.to_string());
    }

    LinkAction::Default
}

fn decode_path(encoded: &str) -> Option<PathBuf> {
    mudl_server::routes::percent_decode(encoded).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "127.0.0.1:53211";

    #[test]
    fn own_root_is_default() {
        assert_eq!(
            classify("http://127.0.0.1:53211/", ADDR),
            LinkAction::Default
        );
    }

    #[test]
    fn own_root_with_mode_query_is_default() {
        assert_eq!(
            classify("http://127.0.0.1:53211/?mode=down", ADDR),
            LinkAction::Default
        );
    }

    #[test]
    fn in_page_anchor_is_default() {
        assert_eq!(
            classify("http://127.0.0.1:53211/#section", ADDR),
            LinkAction::Default
        );
    }

    #[test]
    fn own_wait_route_is_default() {
        assert_eq!(
            classify("http://127.0.0.1:53211/wait?since=1", ADDR),
            LinkAction::Default
        );
    }

    #[test]
    fn local_md_route_opens_new_window() {
        assert_eq!(
            classify(
                "http://127.0.0.1:53211/local-md/%2Fhome%2Fuser%2Fstub.md",
                ADDR
            ),
            LinkAction::OpenNewWindow(PathBuf::from("/home/user/stub.md"))
        );
    }

    #[test]
    fn local_file_route_opens_with_system_default() {
        assert_eq!(
            classify(
                "http://127.0.0.1:53211/local-file/%2Fhome%2Fuser%2Fnotes.txt",
                ADDR
            ),
            LinkAction::OpenWithSystemDefault(PathBuf::from("/home/user/notes.txt"))
        );
    }

    #[test]
    fn local_md_route_with_invalid_percent_encoding_is_default() {
        assert_eq!(
            classify("http://127.0.0.1:53211/local-md/%zz", ADDR),
            LinkAction::Default
        );
    }

    #[test]
    fn https_link_opens_externally() {
        assert_eq!(
            classify("https://example.com", ADDR),
            LinkAction::OpenExternally("https://example.com".to_string())
        );
    }

    #[test]
    fn http_link_opens_externally() {
        assert_eq!(
            classify("http://example.com", ADDR),
            LinkAction::OpenExternally("http://example.com".to_string())
        );
    }

    #[test]
    fn mailto_link_opens_externally() {
        assert_eq!(
            classify("mailto:test@example.com", ADDR),
            LinkAction::OpenExternally("mailto:test@example.com".to_string())
        );
    }

    #[test]
    fn other_servers_http_link_is_not_confused_with_own_origin() {
        assert_eq!(
            classify("http://example.com/local-md/x", ADDR),
            LinkAction::OpenExternally("http://example.com/local-md/x".to_string())
        );
    }
}

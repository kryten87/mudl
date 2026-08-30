//! "Changes since…" popover (Phase 13.9 of `docs/IMPLEMENTATION-PLAN.md`):
//! lists `mudl-diff::git`'s waypoint candidates for the open file and
//! re-navigates the WebView with `?waypoint=N` when one is picked, so
//! `mudl-server`'s `serve_document` (Phase 13.9's server-side half)
//! overlays that diff on the next render.

use std::net::SocketAddr;

use gtk::prelude::*;
use mudl_diff::git::WaypointCandidate;
use mudl_server::routes::Mode;

/// The `?mode=…&waypoint=…` query string for a document URL. Empty when
/// neither is needed (Up mode, no diff overlay) — matching the existing
/// convention that plain `/` means Up mode with no query string at all.
fn build_query(mode: Mode, waypoint: Option<usize>) -> String {
    let mut parts = Vec::new();
    if mode == Mode::Down {
        parts.push("mode=down".to_string());
    }
    if let Some(index) = waypoint {
        parts.push(format!("waypoint={index}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// The full `http://<addr>/...` URL for `mode`, optionally overlaying the
/// waypoint at `waypoint` (a 0-based index into a `query_waypoints` call
/// for the same file).
pub fn document_url(addr: SocketAddr, mode: Mode, waypoint: Option<usize>) -> String {
    format!("http://{addr}/{}", build_query(mode, waypoint))
}

/// The popover row label for one candidate: its label, plus its commit
/// message (if any) as a short parenthetical.
fn row_label(candidate: &WaypointCandidate) -> String {
    match &candidate.detail {
        Some(detail) => format!("{} ({detail})", candidate.label),
        None => candidate.label.clone(),
    }
}

/// Builds a `gtk::Popover` listing `candidates`, one row each. Activating a
/// row calls `on_select` with that candidate's index and closes the
/// popover. Empty `candidates` shows a single disabled "No changes found"
/// placeholder row instead of an empty popup.
pub fn build_popover(
    candidates: &[WaypointCandidate],
    on_select: impl Fn(usize) + 'static,
) -> gtk::Popover {
    let popover = gtk::Popover::new(gtk::Widget::NONE);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);

    if candidates.is_empty() {
        let row = gtk::ListBoxRow::new();
        row.set_sensitive(false);
        row.add(&gtk::Label::new(Some("No changes found")));
        list.add(&row);
    } else {
        for candidate in candidates {
            let row = gtk::ListBoxRow::new();
            row.add(&gtk::Label::new(Some(&row_label(candidate))));
            list.add(&row);
        }
    }

    let popover_for_handler = popover.clone();
    list.connect_row_activated(move |_, row| {
        let index = row.index();
        if index >= 0 {
            on_select(index as usize);
        }
        popover_for_handler.popdown();
    });

    popover.add(&list);
    popover.show_all();
    popover
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr() -> SocketAddr {
        "127.0.0.1:53211".parse().unwrap()
    }

    #[test]
    fn up_mode_no_waypoint_is_plain_root() {
        assert_eq!(
            document_url(addr(), Mode::Up, None),
            "http://127.0.0.1:53211/"
        );
    }

    #[test]
    fn down_mode_no_waypoint_has_mode_query() {
        assert_eq!(
            document_url(addr(), Mode::Down, None),
            "http://127.0.0.1:53211/?mode=down"
        );
    }

    #[test]
    fn up_mode_with_waypoint_has_only_waypoint_query() {
        assert_eq!(
            document_url(addr(), Mode::Up, Some(2)),
            "http://127.0.0.1:53211/?waypoint=2"
        );
    }

    #[test]
    fn down_mode_with_waypoint_has_both_queries() {
        assert_eq!(
            document_url(addr(), Mode::Down, Some(0)),
            "http://127.0.0.1:53211/?mode=down&waypoint=0"
        );
    }

    #[test]
    fn row_label_includes_detail_when_present() {
        let candidate = WaypointCandidate {
            label: "since commit abc1234".to_string(),
            detail: Some("Fix typo".to_string()),
            content: String::new(),
            timestamp: std::time::SystemTime::UNIX_EPOCH,
        };
        assert_eq!(row_label(&candidate), "since commit abc1234 (Fix typo)");
    }

    #[test]
    fn row_label_omits_parens_when_no_detail() {
        let candidate = WaypointCandidate {
            label: "since last staged".to_string(),
            detail: None,
            content: String::new(),
            timestamp: std::time::SystemTime::UNIX_EPOCH,
        };
        assert_eq!(row_label(&candidate), "since last staged");
    }
}

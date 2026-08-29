//! Find-in-page (Phase 10.5 of `docs/IMPLEMENTATION-PLAN.md`): a floating
//! `gtk::SearchEntry` overlay driving WebKit2GTK's own `WebKitFindController`.
//!
//! `mud`'s Find is a DOM-based `<mark>`-wrapping script (`mud.js`'s
//! `highlightAll`) because AppKit + `WKWebView` has no first-class
//! find-in-page API to call instead. WebKit2GTK does — `FindController` —
//! so this doesn't need (or use) any bundled JS at all, confirming the
//! plan's guess at this step.

use gtk::prelude::*;
use webkit2gtk::{FindControllerExt, FindOptions, WebViewExt};

/// Generous enough that "how many matches" never silently caps out on a
/// real document, without asking WebKit to count unbounded matches on
/// pathological input.
const MAX_MATCH_COUNT: u32 = 1000;

fn find_options() -> u32 {
    (FindOptions::CASE_INSENSITIVE | FindOptions::WRAP_AROUND).bits()
}

/// The find bar's widgets, returned so the window's Ctrl+F handler can
/// show and focus it.
pub struct FindBar {
    pub container: gtk::Box,
    pub entry: gtk::SearchEntry,
}

impl FindBar {
    /// Shows the bar and focuses the entry (Ctrl+F).
    pub fn show(&self) {
        self.container.show();
        self.entry.grab_focus();
    }
}

/// Wraps `child` in a `gtk::Overlay` with a floating find bar pinned to
/// the top-right corner, hidden until [`FindBar::show`] is called. Returns
/// the overlay (to add to the window in `child`'s place) and the bar.
pub fn build(
    child: &impl IsA<gtk::Widget>,
    webview: &webkit2gtk::WebView,
) -> (gtk::Overlay, FindBar) {
    let overlay = gtk::Overlay::new();
    overlay.add(child);

    let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    container.set_halign(gtk::Align::End);
    container.set_valign(gtk::Align::Start);
    container.set_margin_top(8);
    container.set_margin_end(8);
    container.set_no_show_all(true);

    let entry = gtk::SearchEntry::new();
    let previous = gtk::Button::with_label("\u{25B2}");
    let next = gtk::Button::with_label("\u{25BC}");
    let count_label = gtk::Label::new(None);

    container.pack_start(&entry, false, false, 0);
    container.pack_start(&previous, false, false, 0);
    container.pack_start(&next, false, false, 0);
    container.pack_start(&count_label, false, false, 4);

    overlay.add_overlay(&container);

    let bar = FindBar { container, entry };

    let Some(find_controller) = webview.find_controller() else {
        // No FindController available (shouldn't happen in practice — see
        // the module doc comment — but the binding models it as
        // `Option`); the bar stays built but inert rather than panicking.
        return (overlay, bar);
    };

    {
        let find_controller = find_controller.clone();
        let count_label = count_label.clone();
        bar.entry.connect_search_changed(move |entry| {
            let text = entry.text();
            if text.is_empty() {
                find_controller.search_finish();
                count_label.set_text("");
            } else {
                find_controller.search(&text, find_options(), MAX_MATCH_COUNT);
                find_controller.count_matches(&text, find_options(), MAX_MATCH_COUNT);
            }
        });
    }
    {
        let find_controller = find_controller.clone();
        bar.entry
            .connect_activate(move |_| find_controller.search_next());
    }
    {
        let find_controller = find_controller.clone();
        previous.connect_clicked(move |_| find_controller.search_previous());
    }
    {
        let find_controller = find_controller.clone();
        next.connect_clicked(move |_| find_controller.search_next());
    }
    {
        let find_controller = find_controller.clone();
        let container = bar.container.clone();
        bar.entry.connect_stop_search(move |entry| {
            entry.set_text("");
            find_controller.search_finish();
            container.hide();
        });
    }
    find_controller.connect_counted_matches(move |_, count| {
        count_label.set_text(&count.to_string());
    });

    (overlay, bar)
}

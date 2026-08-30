//! Comments column (Phase 14.7 of `docs/IMPLEMENTATION-PLAN.md`): compose
//! box, reply/edit/delete affordances, wired to `mudl_comments::write`'s
//! impure write flow (Phase 14.5). Mirrors `mud`'s comments sidebar/column
//! within the same "GTK-native sidebar pane, not an HTML/JS column
//! projection" architecture Phase 13.9 already established for the Changes
//! pane — there's no `CommentSubmissionHandler`-style JS bridge here at
//! all, since every write goes straight from a GTK button to
//! `mudl_comments::write`, not through the WebView.
//!
//! Two deliberate narrowings from `mud`, both already flagged where the
//! underlying capability was cut:
//! - A comment created here is always **general** (unanchored):
//!   `mudl_comments::anchor::locate` only resolves a whole-quotation match,
//!   and this panel has no text-selection capture to hand it one — see
//!   `anchor`'s own doc comment. A comment already anchored by hand-editing
//!   the Markdown source still displays and locates correctly; only
//!   *creating* one from this panel is unanchored.
//! - Every message posted from here is unattributed (`author: None,
//!   created: None`): the project's dependency policy has no date/time
//!   crate, and converting `SystemTime` to a *local* wall-clock date needs
//!   either one or platform FFI — out of scope for this pass. A message's
//!   author/timestamp still displays correctly when present (e.g. written
//!   by hand, or by another tool).
//!
//! After every write, the already-running file watcher (Phase 10.8) picks
//! up the change exactly like any external edit and reloads the WebView —
//! no separate "trigger a reload" call is needed here, only a refresh of
//! this panel's own list (the WebView reload doesn't touch it).

use std::path::PathBuf;

use gtk::prelude::*;
use mudl_comments::serialization::CommentMessage;
use mudl_comments::write::{self, RealFileSystem};

use crate::sidebar;

/// Everything the comments column needs: the list view it repopulates
/// after every write, and the open file's path (`mudl_comments::write`'s
/// own re-read-from-disk design means no in-memory document handle is
/// needed here at all).
#[derive(Clone)]
pub struct Context {
    pub list_view: gtk::TreeView,
    pub path: PathBuf,
}

/// Builds the compose box and its Add/Reply/Edit Last/Delete action row,
/// and does the initial population of `ctx.list_view` from the file's
/// current comments.
pub fn build(ctx: &Context) -> gtk::Box {
    refresh_list(ctx);

    let container = gtk::Box::new(gtk::Orientation::Vertical, 4);

    let compose = gtk::TextView::new();
    compose.set_wrap_mode(gtk::WrapMode::Word);
    let compose_scroller =
        gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    compose_scroller.set_size_request(-1, 60);
    compose_scroller.add(&compose);
    container.pack_start(&compose_scroller, true, true, 0);

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let add_button = gtk::Button::with_label("Add");
    let reply_button = gtk::Button::with_label("Reply");
    let edit_button = gtk::Button::with_label("Edit Last");
    let delete_button = gtk::Button::with_label("Delete");
    button_row.pack_start(&add_button, true, true, 0);
    button_row.pack_start(&reply_button, true, true, 0);
    button_row.pack_start(&edit_button, true, true, 0);
    button_row.pack_start(&delete_button, true, true, 0);
    container.pack_start(&button_row, false, false, 0);

    connect_add(&add_button, ctx, &compose);
    connect_reply(&reply_button, ctx, &compose);
    connect_edit(&edit_button, ctx, &compose);
    connect_delete(&delete_button, ctx);

    container
}

/// Re-reads the file from disk and repopulates `ctx.list_view` from its
/// current comments. A read failure (file briefly locked, mid external
/// rewrite) just leaves the list showing its last-known state rather than
/// clearing it — the next successful refresh (the file watcher's own
/// reload will trigger one indirectly, since the user sees the WebView
/// update) catches up.
fn refresh_list(ctx: &Context) {
    let Ok(markdown) = std::fs::read_to_string(&ctx.path) else {
        return;
    };
    let comments = mudl_comments::document::parse_comments(&markdown);
    sidebar::populate_comments_list(&ctx.list_view, &comments);
}

fn connect_add(button: &gtk::Button, ctx: &Context, compose: &gtk::TextView) {
    let ctx = ctx.clone();
    let compose = compose.clone();
    button.connect_clicked(move |_| {
        let Some(body) = compose_text(&compose) else {
            return;
        };
        let message = CommentMessage {
            author: None,
            created: None,
            body,
        };
        let _ = write::add_comment(&RealFileSystem, &ctx.path, None, 0, message);
        clear_compose(&compose);
        refresh_list(&ctx);
    });
}

fn connect_reply(button: &gtk::Button, ctx: &Context, compose: &gtk::TextView) {
    let ctx = ctx.clone();
    let compose = compose.clone();
    button.connect_clicked(move |_| {
        let (Some(label), Some(body)) = (selected_label(&ctx.list_view), compose_text(&compose))
        else {
            return;
        };
        let message = CommentMessage {
            author: None,
            created: None,
            body,
        };
        let _ = write::reply(&RealFileSystem, &ctx.path, &label, message);
        clear_compose(&compose);
        refresh_list(&ctx);
    });
}

fn connect_edit(button: &gtk::Button, ctx: &Context, compose: &gtk::TextView) {
    let ctx = ctx.clone();
    let compose = compose.clone();
    button.connect_clicked(move |_| {
        let (Some(label), Some(body)) = (selected_label(&ctx.list_view), compose_text(&compose))
        else {
            return;
        };
        let _ = write::edit_last_message(&RealFileSystem, &ctx.path, &label, body);
        clear_compose(&compose);
        refresh_list(&ctx);
    });
}

/// Removes the selected comment entirely. `mudl_comments::write` also
/// offers `delete_last_message` (drop just the thread's last reply,
/// keeping the rest), but with only one "Delete" button in v1's action
/// row, whole-comment removal is the more obviously-expected action for
/// it to take; the finer-grained operation isn't reachable from this panel
/// yet.
fn connect_delete(button: &gtk::Button, ctx: &Context) {
    let ctx = ctx.clone();
    button.connect_clicked(move |_| {
        let Some(label) = selected_label(&ctx.list_view) else {
            return;
        };
        let _ = write::delete_comment(&RealFileSystem, &ctx.path, &label);
        refresh_list(&ctx);
    });
}

/// The label of `list_view`'s currently-selected row, if any.
fn selected_label(list_view: &gtk::TreeView) -> Option<String> {
    let (model, iter) = list_view.selection().selected()?;
    let path = model.path(&iter)?;
    let store = model.downcast::<gtk::ListStore>().ok()?;
    sidebar::comment_label_at(&store, &path)
}

/// The compose box's current text, trimmed; `None` when it's empty (post
/// buttons are no-ops rather than writing a blank message).
fn compose_text(view: &gtk::TextView) -> Option<String> {
    let buffer = view.buffer()?;
    let (start, end) = buffer.bounds();
    let text = buffer.text(&start, &end, false)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn clear_compose(view: &gtk::TextView) {
    if let Some(buffer) = view.buffer() {
        buffer.set_text("");
    }
}

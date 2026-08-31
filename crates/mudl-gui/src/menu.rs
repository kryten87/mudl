//! Menu bar (Phase 15 of `docs/IMPLEMENTATION-PLAN.md`), built from
//! `docs/MENUS.md`. One menu bar per window (not per tab, unlike the
//! toolbar): every item resolves "the current tab" through
//! `notebook.current_page()` indexing into `Context.tabs` at the moment
//! it's activated, rather than closing over one fixed tab.
//!
//! Wherever an item duplicates something the toolbar already does, the
//! handler drives the existing toolbar widget instead of reimplementing
//! the state change (Readable Column flips the toolbar's own toggle
//! button; the Theme radios drive the toolbar's combo; Zoom In/Out click
//! the toolbar's own buttons) — so there's exactly one code path per
//! behavior. Checkable items (Hide Sidebar, Readable Column, Mark Up/Mark
//! Down, Theme) are resynced from the current tab's true state whenever
//! their parent submenu's `show` signal fires, rather than via continuous
//! bidirectional signal wiring — a menu's checkmark only needs to be
//! correct at the moment it's opened, and GTK's `set_active` is a no-op
//! when the value isn't actually changing, so this can't loop.
//!
//! Show Comments, Add Comment, Hide Changes, Undo, and Redo are built
//! disabled (`set_sensitive(false)`) — see the Phase 15 intro in
//! `docs/IMPLEMENTATION-PLAN.md` for why each is deferred.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use webkit2gtk::{PrintOperationExt, WebViewExt};

use mudl_config::{Preferences, Theme};
use mudl_server::routes::Mode;

use crate::changes;
use crate::recent;
use crate::toolbar;
use crate::window::{self, Registry, TabHandle};

/// Everything the menu bar needs. One instance per window, built in
/// `window::build_window` after that window's tabs exist.
pub(crate) struct Context {
    pub(crate) app: gtk::Application,
    pub(crate) window: gtk::ApplicationWindow,
    pub(crate) notebook: gtk::Notebook,
    pub(crate) tabs: Rc<RefCell<Vec<TabHandle>>>,
    pub(crate) registry: Registry,
    pub(crate) prefs: Rc<RefCell<Preferences>>,
    pub(crate) prefs_path: PathBuf,
    pub(crate) recent_path: PathBuf,
}

/// Builds the menu bar and an `AccelGroup` the caller must attach to the
/// window (`window.add_accel_group(&accel_group)`) for the accelerators
/// below to fire.
pub(crate) fn build(ctx: &Context) -> (gtk::MenuBar, gtk::AccelGroup) {
    let accel_group = gtk::AccelGroup::new();
    let menu_bar = gtk::MenuBar::new();

    menu_bar.append(&build_file_menu(ctx, &accel_group));
    menu_bar.append(&build_edit_menu(ctx, &accel_group));
    menu_bar.append(&build_view_menu(ctx, &accel_group));
    menu_bar.append(&build_theme_menu(ctx));

    (menu_bar, accel_group)
}

fn add_accel(
    item: &impl IsA<gtk::Widget>,
    accel_group: &gtk::AccelGroup,
    key: gtk::gdk::keys::Key,
    mods: gtk::gdk::ModifierType,
) {
    item.add_accelerator(
        "activate",
        accel_group,
        *key,
        mods,
        gtk::AccelFlags::VISIBLE,
    );
}

/// Runs `f` with the tab `notebook.current_page()` currently selects, if
/// any — the one piece of bookkeeping every per-tab menu handler needs,
/// resolved fresh at activation time rather than captured once at build
/// time (tabs can close, and the active one can change, between builds).
fn with_current_tab<R>(
    tabs: &Rc<RefCell<Vec<TabHandle>>>,
    notebook: &gtk::Notebook,
    f: impl FnOnce(&TabHandle) -> R,
) -> Option<R> {
    let tabs = tabs.borrow();
    let index = notebook.current_page()? as usize;
    tabs.get(index).map(f)
}

// ---------------------------------------------------------------- File

fn build_file_menu(ctx: &Context, accel_group: &gtk::AccelGroup) -> gtk::MenuItem {
    use gtk::gdk::keys::constants as key;
    use gtk::gdk::ModifierType as Mod;

    let menu = gtk::Menu::new();

    let open = gtk::MenuItem::with_label("Open...");
    add_accel(&open, accel_group, key::o, Mod::CONTROL_MASK);
    connect_open(&open, ctx);
    menu.append(&open);

    let open_recent = gtk::MenuItem::with_label("Open Recent");
    let open_recent_submenu = gtk::Menu::new();
    connect_open_recent(&open_recent_submenu, ctx);
    open_recent.set_submenu(Some(&open_recent_submenu));
    menu.append(&open_recent);

    menu.append(&gtk::SeparatorMenuItem::new());

    let open_in_browser = gtk::MenuItem::with_label("Open In Browser");
    add_accel(
        &open_in_browser,
        accel_group,
        key::b,
        Mod::CONTROL_MASK | Mod::SHIFT_MASK,
    );
    connect_open_in_browser(&open_in_browser, ctx);
    menu.append(&open_in_browser);

    let print = gtk::MenuItem::with_label("Print...");
    add_accel(&print, accel_group, key::p, Mod::CONTROL_MASK);
    connect_print(&print, ctx);
    menu.append(&print);

    menu.append(&gtk::SeparatorMenuItem::new());

    let reload = gtk::MenuItem::with_label("Reload");
    add_accel(&reload, accel_group, key::r, Mod::CONTROL_MASK);
    connect_reload(&reload, ctx);
    menu.append(&reload);

    let close = gtk::MenuItem::with_label("Close");
    add_accel(&close, accel_group, key::w, Mod::CONTROL_MASK);
    connect_close(&close, ctx);
    menu.append(&close);

    let file = gtk::MenuItem::with_label("File");
    file.set_submenu(Some(&menu));
    file
}

/// `File > Open...`: a plain file-picker that reuses `window::open_files`'s
/// existing "focus if already open, else start a tab and open a window"
/// logic — the same path a fresh `GApplication::open` request already
/// uses — rather than re-implementing it.
fn connect_open(item: &gtk::MenuItem, ctx: &Context) {
    let app = ctx.app.clone();
    let window = ctx.window.clone();
    let registry = Rc::clone(&ctx.registry);
    let prefs = Rc::clone(&ctx.prefs);
    let prefs_path = ctx.prefs_path.clone();
    item.connect_activate(move |_| {
        let dialog = gtk::FileChooserDialog::builder()
            .title("Open")
            .transient_for(&window)
            .action(gtk::FileChooserAction::Open)
            .modal(true)
            .build();
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Open", gtk::ResponseType::Accept);

        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Markdown files"));
        filter.add_pattern("*.md");
        filter.add_pattern("*.markdown");
        filter.add_pattern("*.mkd");
        dialog.add_filter(filter);

        let response = dialog.run();
        let selected = dialog.filename();
        dialog.close();

        if response == gtk::ResponseType::Accept {
            if let Some(path) = selected {
                window::open_files(&app, &[path], &registry, &prefs, &prefs_path);
            }
        }
    });
}

/// `File > Open Recent`: rebuilt from `mudl_gui::recent`'s saved list every
/// time the submenu is shown, so a file opened from another window's menu
/// (or from the command line) since the last time it was opened is
/// reflected without needing a live-update channel between windows.
fn connect_open_recent(submenu: &gtk::Menu, ctx: &Context) {
    let app = ctx.app.clone();
    let registry = Rc::clone(&ctx.registry);
    let prefs = Rc::clone(&ctx.prefs);
    let prefs_path = ctx.prefs_path.clone();
    let recent_path = ctx.recent_path.clone();
    let submenu_for_show = submenu.clone();

    submenu.connect_show(move |_| {
        for child in submenu_for_show.children() {
            submenu_for_show.remove(&child);
        }

        let paths = recent::load(&mudl_config::RealFileSystem, &recent_path);
        if paths.is_empty() {
            let empty = gtk::MenuItem::with_label("(No Recent Files)");
            empty.set_sensitive(false);
            submenu_for_show.append(&empty);
        } else {
            for path in paths {
                let item = gtk::MenuItem::with_label(&path.to_string_lossy());
                let app = app.clone();
                let registry = Rc::clone(&registry);
                let prefs = Rc::clone(&prefs);
                let prefs_path = prefs_path.clone();
                item.connect_activate(move |_| {
                    window::open_files(
                        &app,
                        std::slice::from_ref(&path),
                        &registry,
                        &prefs,
                        &prefs_path,
                    );
                });
                submenu_for_show.append(&item);
            }
        }
        submenu_for_show.show_all();
    });
}

fn connect_open_in_browser(item: &gtk::MenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    item.connect_activate(move |_| {
        with_current_tab(&tabs, &notebook, |tab| {
            let url = changes::document_url(tab.toolbar_ctx.addr, tab.toolbar_ctx.mode.get(), None);
            let _ =
                gtk::gio::AppInfo::launch_default_for_uri(&url, gtk::gio::AppLaunchContext::NONE);
        });
    });
}

fn connect_print(item: &gtk::MenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    let window = ctx.window.clone();
    item.connect_activate(move |_| {
        with_current_tab(&tabs, &notebook, |tab| {
            let operation = webkit2gtk::PrintOperation::new(&tab.toolbar_ctx.webview);
            operation.run_dialog(Some(&window));
        });
    });
}

fn connect_reload(item: &gtk::MenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    item.connect_activate(move |_| {
        with_current_tab(&tabs, &notebook, |tab| {
            tab.toolbar_ctx.webview.reload_bypass_cache();
        });
    });
}

/// Removes the current tab; closes the window if that was the last one.
fn connect_close(item: &gtk::MenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    let window = ctx.window.clone();
    item.connect_activate(move |_| {
        let Some(index) = notebook.current_page() else {
            return;
        };
        notebook.remove_page(Some(index));
        let is_empty = {
            let mut tabs = tabs.borrow_mut();
            let index = index as usize;
            if index < tabs.len() {
                tabs.remove(index);
            }
            tabs.is_empty()
        };
        if is_empty {
            window.close();
        }
    });
}

// ---------------------------------------------------------------- Edit

#[derive(Clone, Copy)]
enum EditAction {
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
}

fn build_edit_menu(ctx: &Context, accel_group: &gtk::AccelGroup) -> gtk::MenuItem {
    use gtk::gdk::keys::constants as key;
    use gtk::gdk::ModifierType as Mod;

    let menu = gtk::Menu::new();

    // Deferred (Phase 15 intro): the only editable widget is the Comments
    // compose box, and GTK3's TextView has no built-in undo/redo.
    let undo = gtk::MenuItem::with_label("Undo");
    add_accel(&undo, accel_group, key::z, Mod::CONTROL_MASK);
    undo.set_sensitive(false);
    menu.append(&undo);

    let redo = gtk::MenuItem::with_label("Redo");
    add_accel(
        &redo,
        accel_group,
        key::z,
        Mod::CONTROL_MASK | Mod::SHIFT_MASK,
    );
    redo.set_sensitive(false);
    menu.append(&redo);

    menu.append(&gtk::SeparatorMenuItem::new());

    let cut = gtk::MenuItem::with_label("Cut");
    add_accel(&cut, accel_group, key::x, Mod::CONTROL_MASK);
    connect_edit_action(&cut, ctx, EditAction::Cut);
    menu.append(&cut);

    let copy = gtk::MenuItem::with_label("Copy");
    add_accel(&copy, accel_group, key::c, Mod::CONTROL_MASK);
    connect_edit_action(&copy, ctx, EditAction::Copy);
    menu.append(&copy);

    let paste = gtk::MenuItem::with_label("Paste");
    add_accel(&paste, accel_group, key::v, Mod::CONTROL_MASK);
    connect_edit_action(&paste, ctx, EditAction::Paste);
    menu.append(&paste);

    let delete = gtk::MenuItem::with_label("Delete");
    connect_edit_action(&delete, ctx, EditAction::Delete);
    menu.append(&delete);

    let select_all = gtk::MenuItem::with_label("Select All");
    add_accel(&select_all, accel_group, key::a, Mod::CONTROL_MASK);
    connect_edit_action(&select_all, ctx, EditAction::SelectAll);
    menu.append(&select_all);

    menu.append(&gtk::SeparatorMenuItem::new());

    // Deferred (Phase 15 intro): needs runtime sidebar-pane switching,
    // same as View > Show Comments below, which shares this accelerator.
    let add_comment = gtk::MenuItem::with_label("Add Comment");
    add_accel(
        &add_comment,
        accel_group,
        key::k,
        Mod::CONTROL_MASK | Mod::SHIFT_MASK,
    );
    add_comment.set_sensitive(false);
    menu.append(&add_comment);

    menu.append(&gtk::SeparatorMenuItem::new());

    let find = gtk::MenuItem::with_label("Find");
    add_accel(&find, accel_group, key::f, Mod::CONTROL_MASK);
    connect_find(&find, ctx);
    menu.append(&find);

    let find_next = gtk::MenuItem::with_label("Find Next");
    add_accel(&find_next, accel_group, key::g, Mod::CONTROL_MASK);
    connect_find_next(&find_next, ctx);
    menu.append(&find_next);

    let find_previous = gtk::MenuItem::with_label("Find Previous");
    add_accel(
        &find_previous,
        accel_group,
        key::g,
        Mod::CONTROL_MASK | Mod::SHIFT_MASK,
    );
    connect_find_previous(&find_previous, ctx);
    menu.append(&find_previous);

    let edit = gtk::MenuItem::with_label("Edit");
    edit.set_submenu(Some(&menu));
    edit
}

fn connect_edit_action(item: &gtk::MenuItem, ctx: &Context, action: EditAction) {
    let window = ctx.window.clone();
    item.connect_activate(move |_| apply_edit_action(&window, action));
}

/// Resolves the currently-focused widget and routes the action to
/// whichever of it applies to: the find bar's `Entry`, the comments
/// compose box's `TextView`, or (Copy/Select All only — page content isn't
/// editable) the `WebView`. GTK/WebKit glue with no useful pure core to
/// extract, per Phase 10's own framing for signal wiring like this.
fn apply_edit_action(window: &gtk::ApplicationWindow, action: EditAction) {
    let Some(focus) = window.focused_widget() else {
        return;
    };

    if let Some(entry) = focus.downcast_ref::<gtk::Entry>() {
        match action {
            EditAction::Cut => entry.cut_clipboard(),
            EditAction::Copy => entry.copy_clipboard(),
            EditAction::Paste => entry.paste_clipboard(),
            EditAction::Delete => entry.delete_selection(),
            EditAction::SelectAll => entry.select_region(0, -1),
        }
        return;
    }

    if let Some(text_view) = focus.downcast_ref::<gtk::TextView>() {
        let Some(buffer) = text_view.buffer() else {
            return;
        };
        let display = text_view.display();
        let Some(clipboard) = gtk::Clipboard::default(&display) else {
            return;
        };
        let editable = text_view.is_editable();
        match action {
            EditAction::Cut => buffer.cut_clipboard(&clipboard, editable),
            EditAction::Copy => buffer.copy_clipboard(&clipboard),
            EditAction::Paste => buffer.paste_clipboard(&clipboard, None, editable),
            EditAction::Delete => {
                buffer.delete_selection(true, editable);
            }
            EditAction::SelectAll => {
                let start = buffer.start_iter();
                let end = buffer.end_iter();
                buffer.select_range(&start, &end);
            }
        }
        return;
    }

    if let Some(webview) = focus.downcast_ref::<webkit2gtk::WebView>() {
        match action {
            EditAction::Copy => webview.execute_editing_command("Copy"),
            EditAction::SelectAll => webview.execute_editing_command("SelectAll"),
            EditAction::Cut | EditAction::Paste | EditAction::Delete => {}
        }
    }
}

fn connect_find(item: &gtk::MenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    item.connect_activate(move |_| {
        with_current_tab(&tabs, &notebook, |tab| tab.find_bar.show());
    });
}

fn connect_find_next(item: &gtk::MenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    item.connect_activate(move |_| {
        with_current_tab(&tabs, &notebook, |tab| tab.find_bar.search_next());
    });
}

fn connect_find_previous(item: &gtk::MenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    item.connect_activate(move |_| {
        with_current_tab(&tabs, &notebook, |tab| tab.find_bar.search_previous());
    });
}

// ---------------------------------------------------------------- View

fn build_view_menu(ctx: &Context, accel_group: &gtk::AccelGroup) -> gtk::MenuItem {
    use gtk::gdk::keys::constants as key;
    use gtk::gdk::ModifierType as Mod;

    let menu = gtk::Menu::new();

    let hide_sidebar = gtk::CheckMenuItem::with_label("Hide Sidebar");
    add_accel(
        &hide_sidebar,
        accel_group,
        key::s,
        Mod::CONTROL_MASK | Mod::SHIFT_MASK,
    );
    connect_hide_sidebar(&hide_sidebar, ctx);
    menu.append(&hide_sidebar);

    // Deferred (Phase 15 intro): needs a new CSS class to hide the
    // waypoint-diff overlay independently of clearing the waypoint.
    let hide_changes = gtk::CheckMenuItem::with_label("Hide Changes");
    add_accel(
        &hide_changes,
        accel_group,
        key::c,
        Mod::CONTROL_MASK | Mod::SHIFT_MASK,
    );
    hide_changes.set_sensitive(false);
    menu.append(&hide_changes);

    // Deferred (Phase 15 intro): needs runtime sidebar-pane switching.
    let show_comments = gtk::CheckMenuItem::with_label("Show Comments");
    add_accel(
        &show_comments,
        accel_group,
        key::k,
        Mod::CONTROL_MASK | Mod::SHIFT_MASK,
    );
    show_comments.set_sensitive(false);
    menu.append(&show_comments);

    menu.append(&gtk::SeparatorMenuItem::new());

    let mark_up = gtk::RadioMenuItem::with_label("Mark Up");
    let mark_down = gtk::RadioMenuItem::with_label_from_widget(&mark_up, Some("Mark Down"));
    connect_mark_mode(&mark_up, ctx, Mode::Up);
    connect_mark_mode(&mark_down, ctx, Mode::Down);
    menu.append(&mark_up);
    menu.append(&mark_down);

    menu.append(&gtk::SeparatorMenuItem::new());

    let readable_column = gtk::CheckMenuItem::with_label("Readable Column");
    add_accel(
        &readable_column,
        accel_group,
        key::r,
        Mod::CONTROL_MASK | Mod::SHIFT_MASK,
    );
    connect_readable_column(&readable_column, ctx);
    menu.append(&readable_column);

    menu.append(&gtk::SeparatorMenuItem::new());

    let actual_size = gtk::MenuItem::with_label("Actual Size");
    add_accel(&actual_size, accel_group, key::_0, Mod::CONTROL_MASK);
    connect_actual_size(&actual_size, ctx);
    menu.append(&actual_size);

    let zoom_in = gtk::MenuItem::with_label("Zoom In");
    add_accel(&zoom_in, accel_group, key::plus, Mod::CONTROL_MASK);
    connect_zoom_in(&zoom_in, ctx);
    menu.append(&zoom_in);

    let zoom_out = gtk::MenuItem::with_label("Zoom Out");
    add_accel(&zoom_out, accel_group, key::minus, Mod::CONTROL_MASK);
    connect_zoom_out(&zoom_out, ctx);
    menu.append(&zoom_out);

    connect_view_menu_show(
        &menu,
        ctx,
        hide_sidebar,
        mark_up,
        mark_down,
        readable_column,
    );

    let view = gtk::MenuItem::with_label("View");
    view.set_submenu(Some(&menu));
    view
}

/// Resyncs the checkable View items from the current tab's true state
/// every time the View menu is opened (see the module doc comment on why
/// this doesn't need continuous bidirectional signal wiring).
fn connect_view_menu_show(
    menu: &gtk::Menu,
    ctx: &Context,
    hide_sidebar: gtk::CheckMenuItem,
    mark_up: gtk::RadioMenuItem,
    mark_down: gtk::RadioMenuItem,
    readable_column: gtk::CheckMenuItem,
) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    menu.connect_show(move |_| {
        with_current_tab(&tabs, &notebook, |tab| {
            hide_sidebar.set_active(!tab.sidebar_scroller.is_visible());
            // Read the flag into a local first: `set_active` below can
            // synchronously fire `connect_readable_column`'s handler,
            // which does `ctx.prefs.borrow_mut()` — holding the `Ref`
            // from `.borrow()` alive across that call (Rust extends a
            // temporary's lifetime to the end of its statement) would
            // panic with a `BorrowMutError` on this same `RefCell`.
            let readable_column_active = tab.toolbar_ctx.prefs.borrow().ui_show_readable_column;
            readable_column.set_active(readable_column_active);
            match tab.toolbar_ctx.mode.get() {
                Mode::Up => mark_up.set_active(true),
                Mode::Down => mark_down.set_active(true),
            }
        });
    });
}

fn connect_hide_sidebar(check: &gtk::CheckMenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    let prefs = Rc::clone(&ctx.prefs);
    let prefs_path = ctx.prefs_path.clone();
    check.connect_toggled(move |check| {
        let hidden = check.is_active();
        with_current_tab(&tabs, &notebook, |tab| {
            tab.sidebar_scroller.set_visible(!hidden);
        });
        prefs.borrow_mut().sidebar_enabled = !hidden;
        let _ = mudl_config::save(&mudl_config::RealFileSystem, &prefs_path, &prefs.borrow());
    });
}

fn connect_readable_column(check: &gtk::CheckMenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    check.connect_toggled(move |check| {
        let active = check.is_active();
        with_current_tab(&tabs, &notebook, |tab| {
            toolbar::set_readable_column(&tab.toolbar_ctx, active);
        });
    });
}

fn connect_mark_mode(radio: &gtk::RadioMenuItem, ctx: &Context, target: Mode) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    radio.connect_toggled(move |radio| {
        if !radio.is_active() {
            return;
        }
        with_current_tab(&tabs, &notebook, |tab| {
            window::navigate_to_mode(
                &tab.toolbar_ctx.webview,
                tab.toolbar_ctx.addr,
                &tab.toolbar_ctx.mode,
                &tab.pending_scroll_fraction,
                target,
            );
        });
    });
}

fn connect_actual_size(item: &gtk::MenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    item.connect_activate(move |_| {
        with_current_tab(&tabs, &notebook, |tab| {
            toolbar::set_zoom(&tab.toolbar_ctx, 1.0);
        });
    });
}

fn connect_zoom_in(item: &gtk::MenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    item.connect_activate(move |_| {
        with_current_tab(&tabs, &notebook, |tab| {
            toolbar::step_zoom(&tab.toolbar_ctx, toolbar::ZOOM_STEP);
        });
    });
}

fn connect_zoom_out(item: &gtk::MenuItem, ctx: &Context) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    item.connect_activate(move |_| {
        with_current_tab(&tabs, &notebook, |tab| {
            toolbar::step_zoom(&tab.toolbar_ctx, -toolbar::ZOOM_STEP);
        });
    });
}

// ---------------------------------------------------------------- Theme

fn build_theme_menu(ctx: &Context) -> gtk::MenuItem {
    let menu = gtk::Menu::new();

    let mut group_head: Option<gtk::RadioMenuItem> = None;
    let mut items: Vec<(Theme, gtk::RadioMenuItem)> = Vec::new();
    for theme in Theme::all() {
        let item = match &group_head {
            None => gtk::RadioMenuItem::with_label(theme.as_str()),
            Some(head) => gtk::RadioMenuItem::with_label_from_widget(head, Some(theme.as_str())),
        };
        if group_head.is_none() {
            group_head = Some(item.clone());
        }
        connect_theme_item(&item, ctx, theme);
        menu.append(&item);
        items.push((theme, item));
    }

    // The current theme is a single shared preference (not per-tab), so
    // resyncing the checkmark just reads it directly rather than going
    // through `with_current_tab`.
    let prefs = Rc::clone(&ctx.prefs);
    menu.connect_show(move |_| {
        let current = prefs.borrow().theme;
        for (theme, item) in &items {
            if *theme == current {
                item.set_active(true);
            }
        }
    });

    let theme_menu_item = gtk::MenuItem::with_label("Theme");
    theme_menu_item.set_submenu(Some(&menu));
    theme_menu_item
}

fn connect_theme_item(item: &gtk::RadioMenuItem, ctx: &Context, theme: Theme) {
    let tabs = Rc::clone(&ctx.tabs);
    let notebook = ctx.notebook.clone();
    item.connect_toggled(move |item| {
        if !item.is_active() {
            return;
        }
        with_current_tab(&tabs, &notebook, |tab| {
            toolbar::set_theme(&tab.toolbar_ctx, theme);
        });
    });
}

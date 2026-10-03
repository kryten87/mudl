//! Change tracking UI (`_working/DIFF-IMPLEMENTATION.md` §11.3-11.5): the
//! floating Changes bar (count badge, "N changes since …" status button
//! with its "Changes since…" popover, Previous / Next / Accept), and the
//! sidebar's Changes pane.
//!
//! The tracker itself lives in the tab's `mudl_server::server::DocumentSource`
//! (the server feeds it on every request); everything here reads it after
//! each finished page load ([`Changes::refresh`]) and mutates it only
//! through Accept and the waypoint picker, each followed by a reload that
//! keeps the scroll position.
//!
//! The decisions — labels, badge colors, wrap-around navigation, row
//! condensing, local-time formatting — are pure functions at the top of
//! the module, unit tested; the GTK wiring below has no logic of its own.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use gtk::prelude::*;
use webkit2gtk::WebViewExt;

use mudl_config::FloatingControlsPosition;
use mudl_core::changes::{ChangeGroup, ChangeType, MenuItem};
use mudl_core::template::js_string_literal;

use crate::toolbar;
use crate::window;

// MARK: - Pure helpers

/// The color a count badge takes (`ins` green, `del` red, `mix` orange,
/// `none` gray), as the CSS class [`install_css`] styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    Ins,
    Del,
    Mix,
    None,
}

impl BadgeKind {
    pub fn css_class(self) -> &'static str {
        match self {
            BadgeKind::Ins => "ins",
            BadgeKind::Del => "del",
            BadgeKind::Mix => "mix",
            BadgeKind::None => "none",
        }
    }

    fn from_flags(count: usize, has_insertions: bool, has_deletions: bool) -> Self {
        match (count, has_insertions, has_deletions) {
            (0, _, _) => BadgeKind::None,
            (_, true, true) => BadgeKind::Mix,
            (_, false, true) => BadgeKind::Del,
            _ => BadgeKind::Ins,
        }
    }
}

/// The bar's badge: gray with no changes, orange when insertions and
/// deletions both occur anywhere, else red/green (`ChangesBar.badgeColor`).
pub fn badge_kind(groups: &[ChangeGroup]) -> BadgeKind {
    let has = |ty: ChangeType| {
        groups
            .iter()
            .flat_map(|g| g.members.iter())
            .any(|(t, _)| *t == ty)
    };
    BadgeKind::from_flags(
        groups.len(),
        has(ChangeType::Insertion),
        has(ChangeType::Deletion),
    )
}

/// One sidebar row's badge (`ChangeGroupRow.badgeColor`).
pub fn group_badge_kind(group: &ChangeGroup) -> BadgeKind {
    if group.is_mixed {
        return BadgeKind::Mix;
    }
    match group.ty {
        ChangeType::Insertion => BadgeKind::Ins,
        ChangeType::Deletion => BadgeKind::Del,
    }
}

/// One "Changes since…" row's badge (gray at zero).
pub fn item_badge_kind(item: &MenuItem) -> BadgeKind {
    BadgeKind::from_flags(item.change_count, item.has_insertions, item.has_deletions)
}

/// A wall-clock reading in the local time zone — the only part of a
/// timestamp the labels need. Built from a `SystemTime` by [`local_stamp`];
/// plain data so the formatting below is testable without a time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalStamp {
    pub year: i32,
    /// 1..=12
    pub month: u32,
    /// 1..=31
    pub day: u32,
    /// 0..=23
    pub hour: u32,
    /// 0..=59
    pub minute: u32,
}

/// Converts `t` to local wall-clock time via GLib (already a dependency
/// through GTK; it consults the system time zone database). `None` only
/// for times GLib can't represent.
pub fn local_stamp(t: SystemTime) -> Option<LocalStamp> {
    let secs = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok()?,
        Err(e) => -i64::try_from(e.duration().as_secs()).ok()?,
    };
    let dt = gtk::glib::DateTime::from_unix_local(secs).ok()?;
    Some(LocalStamp {
        year: dt.year(),
        month: u32::try_from(dt.month()).ok()?,
        day: u32::try_from(dt.day_of_month()).ok()?,
        hour: u32::try_from(dt.hour()).ok()?,
        minute: u32::try_from(dt.minute()).ok()?,
    })
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`), so "yesterday" works across month and year ends.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `mud`'s `Date.shortTimestamp`: "10:19am" today, "yesterday", otherwise
/// "Aug 2" — plus ", 2025" when the year differs from `now`'s.
pub fn short_timestamp(ts: LocalStamp, now: LocalStamp) -> String {
    let day = days_from_civil(ts.year, ts.month, ts.day);
    let today = days_from_civil(now.year, now.month, now.day);
    if day == today {
        let hour12 = match ts.hour % 12 {
            0 => 12,
            h => h,
        };
        let meridiem = if ts.hour < 12 { "am" } else { "pm" };
        return format!("{hour12}:{:02}{meridiem}", ts.minute);
    }
    if day == today - 1 {
        return "yesterday".to_string();
    }
    let month = MONTHS
        .get(ts.month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("?");
    if ts.year == now.year {
        format!("{month} {}", ts.day)
    } else {
        format!("{month} {}, {}", ts.day, ts.year)
    }
}

/// The status button's text: "1 change since 10:19am", "3 changes", ….
pub fn status_label(count: usize, ts: Option<LocalStamp>, now: LocalStamp) -> String {
    let noun = if count == 1 { "change" } else { "changes" };
    match ts {
        Some(ts) => format!("{count} {noun} since {}", short_timestamp(ts, now)),
        None => format!("{count} {noun}"),
    }
}

/// The group after `selected`, wrapping; the first group with no (or a
/// vanished) selection. `None` only with no groups.
pub fn next_group(groups: &[ChangeGroup], selected: Option<&str>) -> Option<usize> {
    if groups.is_empty() {
        return None;
    }
    Some(match position_of(groups, selected) {
        Some(i) => (i + 1) % groups.len(),
        None => 0,
    })
}

/// The group before `selected`, wrapping; the last group with no (or a
/// vanished) selection. `None` only with no groups.
pub fn prev_group(groups: &[ChangeGroup], selected: Option<&str>) -> Option<usize> {
    if groups.is_empty() {
        return None;
    }
    Some(match position_of(groups, selected) {
        Some(i) => (i + groups.len() - 1) % groups.len(),
        None => groups.len() - 1,
    })
}

fn position_of(groups: &[ChangeGroup], selected: Option<&str>) -> Option<usize> {
    let selected = selected?;
    groups.iter().position(|g| g.group_id == selected)
}

/// `Mud.scrollToChange([...])` for a group's change IDs, every ID a JS
/// string literal. Guarded so a page without `mud-changes.js` (no changes
/// rendered) ignores it.
pub fn scroll_to_change_script(ids: &[String]) -> String {
    let list = ids
        .iter()
        .map(|id| js_string_literal(id))
        .collect::<Vec<_>>()
        .join(", ");
    format!("if (window.Mud && Mud.scrollToChange) {{ Mud.scrollToChange([{list}]); }}")
}

/// One line of a sidebar row: a member's summary, or the "and N more"
/// note that stands in for the tail of a long same-type run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayLine {
    Member(ChangeType, String),
    Overflow(usize, ChangeType),
}

/// Condenses a group's members (`ChangeGroupRow.displayLines`): a run of
/// up to 3 same-type members shows in full; a longer run shows its first 2
/// and an overflow line for the rest.
pub fn display_lines(members: &[(ChangeType, String)]) -> Vec<DisplayLine> {
    let mut lines = Vec::new();
    let mut i = 0;
    while i < members.len() {
        let ty = members[i].0;
        let end = members[i..]
            .iter()
            .position(|(t, _)| *t != ty)
            .map_or(members.len(), |n| i + n);
        let run = &members[i..end];
        if run.len() <= 3 {
            lines.extend(run.iter().map(|(t, s)| DisplayLine::Member(*t, s.clone())));
        } else {
            lines.extend(
                run[..2]
                    .iter()
                    .map(|(t, s)| DisplayLine::Member(*t, s.clone())),
            );
            lines.push(DisplayLine::Overflow(run.len() - 2, ty));
        }
        i = end;
    }
    lines
}

/// A member line's text: "+ summary" / "− summary" ("—" for an empty
/// summary, e.g. a deleted blank-ish block).
pub fn member_text(ty: ChangeType, summary: &str) -> String {
    let sign = match ty {
        ChangeType::Insertion => '+',
        ChangeType::Deletion => '\u{2212}',
    };
    let summary = if summary.is_empty() {
        "\u{2014}"
    } else {
        summary
    };
    format!("{sign} {summary}")
}

/// "and 1 more insertion", "and 4 more deletions".
pub fn overflow_text(count: usize, ty: ChangeType) -> String {
    let noun = match (ty, count) {
        (ChangeType::Insertion, 1) => "insertion",
        (ChangeType::Insertion, _) => "insertions",
        (ChangeType::Deletion, 1) => "deletion",
        (ChangeType::Deletion, _) => "deletions",
    };
    format!("and {count} more {noun}")
}

/// Splits the popover's rows into "Milestones" (since last accepted /
/// since document opened) and "Recent" (the "since N … ago" rows).
pub fn split_menu_items(items: Vec<MenuItem>) -> (Vec<MenuItem>, Vec<MenuItem>) {
    items
        .into_iter()
        .partition(|item| !item.label.ends_with(" ago"))
}

/// Where the bar floats over the document (`ui_floating_controls_position`).
pub fn bar_alignment(position: FloatingControlsPosition) -> (gtk::Align, gtk::Align) {
    use gtk::Align::{Center, End, Start};
    use FloatingControlsPosition as P;
    match position {
        P::TopLeft => (Start, Start),
        P::TopCenter => (Center, Start),
        P::TopRight => (End, Start),
        P::BottomLeft => (Start, End),
        P::BottomCenter => (Center, End),
        P::BottomRight => (End, End),
    }
}

// MARK: - Styling

const CSS: &str = "
.mudl-changes-bar {
  background-color: @theme_bg_color;
  border: 1px solid alpha(@theme_fg_color, 0.2);
  border-radius: 999px;
  padding: 4px 8px;
}
.mudl-change-badge {
  border-radius: 11px;
  min-width: 22px;
  min-height: 22px;
  padding: 0 4px;
  color: #ffffff;
  font-weight: bold;
  font-size: smaller;
}
.mudl-change-badge.ins { background-color: #1a7f37; }
.mudl-change-badge.del { background-color: #cf222e; }
.mudl-change-badge.mix { background-color: #e89530; }
.mudl-change-badge.none { background-color: #8c959f; }
.mudl-change-line.ins { color: #1a7f37; }
.mudl-change-line.del { color: #cf222e; }
.mudl-change-overflow { font-style: italic; opacity: 0.6; }
.mudl-changes-heading { font-weight: bold; font-size: smaller; opacity: 0.7; }
.mudl-changes-detail { font-size: smaller; opacity: 0.7; }
.mudl-changes-empty-title { font-weight: bold; }
";

thread_local! {
    static CSS_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Installs the badge/bar stylesheet for the default screen, once.
fn install_css() {
    if CSS_INSTALLED.with(Cell::get) {
        return;
    }
    let Some(screen) = gtk::gdk::Screen::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    if provider.load_from_data(CSS.as_bytes()).is_err() {
        return;
    }
    gtk::StyleContext::add_provider_for_screen(
        &screen,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    CSS_INSTALLED.with(|c| c.set(true));
}

fn new_badge() -> gtk::Label {
    let badge = gtk::Label::new(None);
    badge.style_context().add_class("mudl-change-badge");
    badge.set_valign(gtk::Align::Center);
    badge
}

fn set_badge(badge: &gtk::Label, count: usize, kind: BadgeKind) {
    badge.set_text(&count.to_string());
    let style = badge.style_context();
    for other in [
        BadgeKind::Ins,
        BadgeKind::Del,
        BadgeKind::Mix,
        BadgeKind::None,
    ] {
        style.remove_class(other.css_class());
    }
    style.add_class(kind.css_class());
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(Some(icon), gtk::IconSize::Button);
    button.set_tooltip_text(Some(tooltip));
    button.set_relief(gtk::ReliefStyle::None);
    button
}

// MARK: - Widgets

/// One tab's change-tracking UI: the floating bar and the sidebar pane.
pub(crate) struct Changes {
    ctx: toolbar::Context,
    pending_scroll_fraction: Rc<Cell<f64>>,
    /// The floating capsule; add it as an overlay child over the document.
    pub(crate) bar: gtk::Box,
    badge: gtk::Label,
    status_button: gtk::Button,
    status_label: gtk::Label,
    clock_icon: gtk::Image,
    prev: gtk::Button,
    next: gtk::Button,
    accept: gtk::Button,
    /// The sidebar's Changes pane (list / "No Changes" / "Changes Hidden").
    pub(crate) pane: gtk::Stack,
    list_box: gtk::ListBox,
    /// The groups the bar and pane currently show (as of the last
    /// refresh), index-aligned with `list_box`'s rows.
    groups: RefCell<Vec<ChangeGroup>>,
    /// The group Previous/Next last moved to, by group ID.
    selected: RefCell<Option<String>>,
}

/// Builds the bar and the pane for the tab `ctx` describes.
/// `pending_scroll_fraction` is the tab's scroll-restore cell, so Accept
/// and the waypoint picker reload without jumping to the top.
pub(crate) fn build(ctx: &toolbar::Context, pending_scroll_fraction: Rc<Cell<f64>>) -> Rc<Changes> {
    install_css();

    // -- Bar
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    bar.style_context().add_class("mudl-changes-bar");
    let (halign, valign) = bar_alignment(ctx.prefs.borrow().ui_floating_controls_position);
    bar.set_halign(halign);
    bar.set_valign(valign);
    bar.set_margin_top(12);
    bar.set_margin_bottom(12);
    bar.set_margin_start(12);
    bar.set_margin_end(12);
    bar.set_no_show_all(true);

    let badge = new_badge();
    let status_label = gtk::Label::new(None);
    let clock_icon =
        gtk::Image::from_icon_name(Some("document-open-recent-symbolic"), gtk::IconSize::Button);
    let status_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    status_content.pack_start(&badge, false, false, 0);
    status_content.pack_start(&status_label, false, false, 0);
    status_content.pack_start(&clock_icon, false, false, 0);
    status_content.show_all();
    let status_button = gtk::Button::new();
    status_button.set_relief(gtk::ReliefStyle::None);
    status_button.set_tooltip_text(Some("Changes since\u{2026}"));
    status_button.add(&status_content);
    status_button.show();

    let prev = icon_button("go-previous-symbolic", "Previous Change");
    let next = icon_button("go-next-symbolic", "Next Change");
    let accept = icon_button("object-select-symbolic", "Accept Changes");

    bar.pack_start(&status_button, false, false, 0);
    bar.pack_start(&prev, false, false, 0);
    bar.pack_start(&next, false, false, 0);
    bar.pack_start(&accept, false, false, 0);

    // -- Pane
    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::Single);
    list_box.set_activate_on_single_click(true);
    let list_scroller =
        gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    list_scroller.add(&list_box);

    let empty = empty_state(
        "No Changes",
        "Changes will appear when\nthe file is modified.",
    );
    let (hidden, show_button) = hidden_state();

    let pane = gtk::Stack::new();
    pane.add_named(&list_scroller, "list");
    pane.add_named(&empty, "empty");
    pane.add_named(&hidden, "hidden");
    // A `gtk::Stack` only switches to visible children, and `refresh`
    // below picks one before the window's `show_all` runs.
    pane.show_all();

    let changes = Rc::new(Changes {
        ctx: ctx.clone(),
        pending_scroll_fraction,
        bar,
        badge,
        status_button,
        status_label,
        clock_icon,
        prev,
        next,
        accept,
        pane,
        list_box,
        groups: RefCell::new(Vec::new()),
        selected: RefCell::new(None),
    });

    connect(&changes, &changes.prev, |c| c.previous());
    connect(&changes, &changes.next, |c| c.next());
    connect(&changes, &changes.accept, |c| c.accept());
    connect(&changes, &changes.status_button, |c| c.show_popover());
    connect(&changes, &show_button, |c| c.set_enabled(true));
    {
        let weak = Rc::downgrade(&changes);
        changes.list_box.connect_row_activated(move |_, row| {
            if let (Some(changes), Ok(index)) = (weak.upgrade(), usize::try_from(row.index())) {
                changes.go_to(index);
            }
        });
    }

    changes.refresh();
    changes
}

/// Connects `button`'s click to `f(changes)` without a reference cycle.
fn connect(changes: &Rc<Changes>, button: &gtk::Button, f: impl Fn(&Rc<Changes>) + 'static) {
    let weak: Weak<Changes> = Rc::downgrade(changes);
    button.connect_clicked(move |_| {
        if let Some(changes) = weak.upgrade() {
            f(&changes);
        }
    });
}

fn empty_state(title: &str, detail: &str) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.set_margin_top(24);
    column.set_margin_start(12);
    column.set_margin_end(12);
    let title = gtk::Label::new(Some(title));
    title.style_context().add_class("mudl-changes-empty-title");
    let detail = gtk::Label::new(Some(detail));
    detail.set_justify(gtk::Justification::Center);
    detail.style_context().add_class("mudl-changes-detail");
    column.pack_start(&title, false, false, 0);
    column.pack_start(&detail, false, false, 0);
    column
}

fn hidden_state() -> (gtk::Box, gtk::Button) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.set_margin_top(24);
    column.set_margin_start(12);
    column.set_margin_end(12);
    let title = gtk::Label::new(Some("Changes Hidden"));
    title.style_context().add_class("mudl-changes-empty-title");
    let button = gtk::Button::with_label("Show changes");
    button.set_halign(gtk::Align::Center);
    column.pack_start(&title, false, false, 0);
    column.pack_start(&button, false, false, 0);
    (column, button)
}

impl Changes {
    /// Re-reads the tracker and updates the bar and pane. Called after
    /// every finished page load (the server has just fed the tracker) and
    /// when the tab becomes current.
    pub(crate) fn refresh(&self) {
        let enabled = self.ctx.prefs.borrow().changes_enabled;

        // Another tab may have flipped the shared preference since this
        // tab last rendered: bring its server config in line and reload
        // (this refresh runs again when that load finishes).
        let server_enabled = self
            .ctx
            .document
            .config
            .lock()
            .map(|config| config.changes_enabled)
            .unwrap_or(enabled);
        if server_enabled != enabled {
            self.ctx.apply_document_config();
            self.reload_preserving_scroll();
        }

        self.bar.set_visible(enabled);
        if !enabled {
            self.groups.borrow_mut().clear();
            self.selected.replace(None);
            self.populate_list();
            self.pane.set_visible_child_name("hidden");
            return;
        }

        let groups = self.ctx.document.change_groups();
        let now = SystemTime::now();
        let label = match local_stamp(now) {
            Some(now_local) => status_label(
                groups.len(),
                self.ctx
                    .document
                    .active_waypoint_time()
                    .and_then(local_stamp),
                now_local,
            ),
            None => status_label(groups.len(), None, LocalStamp::EPOCH),
        };
        set_badge(&self.badge, groups.len(), badge_kind(&groups));
        self.status_label.set_text(&label);
        self.clock_icon.set_visible(groups.is_empty());
        self.prev.set_visible(groups.len() >= 2);
        self.next.set_visible(!groups.is_empty());
        self.accept.set_visible(!groups.is_empty());

        {
            let mut selected = self.selected.borrow_mut();
            if position_of(&groups, selected.as_deref()).is_none() {
                *selected = None;
            }
        }
        *self.groups.borrow_mut() = groups;
        self.populate_list();
        let has_groups = !self.groups.borrow().is_empty();
        self.pane
            .set_visible_child_name(if has_groups { "list" } else { "empty" });
    }

    fn populate_list(&self) {
        for child in self.list_box.children() {
            self.list_box.remove(&child);
        }
        let selected = self.selected.borrow().clone();
        for group in self.groups.borrow().iter() {
            let row = group_row(group);
            self.list_box.add(&row);
            if selected.as_deref() == Some(group.group_id.as_str()) {
                self.list_box.select_row(Some(&row));
            }
        }
        self.list_box.show_all();
    }

    /// Next Change (wraps; the first group with nothing selected).
    pub(crate) fn next(&self) {
        let index = next_group(&self.groups.borrow(), self.selected.borrow().as_deref());
        if let Some(index) = index {
            self.go_to(index);
        }
    }

    /// Previous Change (wraps; the last group with nothing selected).
    pub(crate) fn previous(&self) {
        let index = prev_group(&self.groups.borrow(), self.selected.borrow().as_deref());
        if let Some(index) = index {
            self.go_to(index);
        }
    }

    /// Selects group `index` and scrolls the document to it. Re-running it
    /// on the same group scrolls again (a sidebar row clicked twice).
    fn go_to(&self, index: usize) {
        let Some(group) = self.groups.borrow().get(index).cloned() else {
            return;
        };
        self.selected.replace(Some(group.group_id.clone()));
        if let Ok(i) = i32::try_from(index) {
            if let Some(row) = self.list_box.row_at_index(i) {
                self.list_box.select_row(Some(&row));
            }
        }
        let script = scroll_to_change_script(&group.change_ids);
        self.ctx.webview.evaluate_javascript(
            &script,
            None,
            None,
            None::<&gtk::gio::Cancellable>,
            |_| {},
        );
    }

    /// Accept Changes: the current content becomes the baseline; reloads
    /// keeping the scroll position.
    pub(crate) fn accept(&self) {
        self.ctx.document.accept_changes();
        self.selected.replace(None);
        self.reload_preserving_scroll();
    }

    /// Show/Hide Changes: persists `changes_enabled`, applies it to this
    /// tab's server and reloads (other tabs catch up in [`Self::refresh`]
    /// when they next become current).
    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.ctx.prefs.borrow_mut().changes_enabled = enabled;
        self.ctx.save_prefs();
        self.ctx.apply_document_config();
        self.reload_preserving_scroll();
    }

    fn reload_preserving_scroll(&self) {
        window::navigate_to_mode(
            &self.ctx.webview,
            self.ctx.addr,
            &self.ctx.mode,
            &self.pending_scroll_fraction,
            self.ctx.mode.get(),
        );
    }

    /// The "Changes since…" popover (§11.5), rebuilt from the tracker's
    /// menu items every time it opens.
    fn show_popover(self: &Rc<Self>) {
        let items = self.ctx.document.change_menu_items();
        let now_local = local_stamp(SystemTime::now());
        let (milestones, recent) = split_menu_items(items);

        let popover = gtk::Popover::new(Some(&self.status_button));
        let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
        column.set_margin_top(8);
        column.set_margin_bottom(8);
        column.set_margin_start(8);
        column.set_margin_end(8);

        let sections = [("Milestones", milestones), ("Recent", recent)];
        let mut first = true;
        for (title, items) in sections {
            if items.is_empty() {
                continue;
            }
            if !first {
                column.pack_start(
                    &gtk::Separator::new(gtk::Orientation::Horizontal),
                    false,
                    false,
                    4,
                );
            }
            first = false;
            let heading = gtk::Label::new(Some(title));
            heading.set_halign(gtk::Align::Start);
            heading.style_context().add_class("mudl-changes-heading");
            column.pack_start(&heading, false, false, 2);
            for item in items {
                let row = self.popover_row(&item, now_local, &popover);
                column.pack_start(&row, false, false, 0);
            }
        }
        if first {
            let none = gtk::Label::new(Some("No waypoints yet"));
            column.pack_start(&none, false, false, 0);
        }

        popover.add(&column);
        column.show_all();
        popover.popup();
    }

    fn popover_row(
        self: &Rc<Self>,
        item: &MenuItem,
        now_local: Option<LocalStamp>,
        popover: &gtk::Popover,
    ) -> gtk::Button {
        let badge = new_badge();
        set_badge(&badge, item.change_count, item_badge_kind(item));

        let label = gtk::Label::new(Some(&item.label));
        label.set_halign(gtk::Align::Start);
        let stamp = match (local_stamp(item.at), now_local) {
            (Some(at), Some(now)) => short_timestamp(at, now),
            _ => String::new(),
        };
        let detail = gtk::Label::new(Some(&stamp));
        detail.set_halign(gtk::Align::Start);
        detail.style_context().add_class("mudl-changes-detail");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.pack_start(&label, false, false, 0);
        text.pack_start(&detail, false, false, 0);

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.pack_start(&badge, false, false, 0);
        content.pack_start(&text, true, true, 0);
        if item.is_active {
            let check =
                gtk::Image::from_icon_name(Some("object-select-symbolic"), gtk::IconSize::Button);
            content.pack_end(&check, false, false, 0);
        }

        let button = gtk::Button::new();
        button.set_relief(gtk::ReliefStyle::None);
        button.add(&content);

        let weak = Rc::downgrade(self);
        let popover = popover.clone();
        let id = item.id;
        let is_active = item.is_active;
        button.connect_clicked(move |_| {
            popover.popdown();
            if is_active {
                return;
            }
            if let Some(changes) = weak.upgrade() {
                changes.ctx.document.select_waypoint(Some(id));
                changes.selected.replace(None);
                changes.reload_preserving_scroll();
            }
        });
        button
    }
}

impl LocalStamp {
    /// A placeholder "now" for the (practically unreachable) case GLib
    /// can't convert the current time; only used with no timestamp.
    const EPOCH: LocalStamp = LocalStamp {
        year: 1970,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
    };
}

/// One Changes-pane row: the group's numbered badge beside its condensed
/// member lines.
fn group_row(group: &ChangeGroup) -> gtk::ListBoxRow {
    let badge = new_badge();
    set_badge(&badge, group.index, group_badge_kind(group));
    badge.set_valign(gtk::Align::Start);

    let lines = gtk::Box::new(gtk::Orientation::Vertical, 1);
    for line in display_lines(&group.members) {
        let label = match line {
            DisplayLine::Member(ty, summary) => {
                let label = gtk::Label::new(Some(&member_text(ty, &summary)));
                let style = label.style_context();
                style.add_class("mudl-change-line");
                style.add_class(match ty {
                    ChangeType::Insertion => "ins",
                    ChangeType::Deletion => "del",
                });
                label
            }
            DisplayLine::Overflow(count, ty) => {
                let label = gtk::Label::new(Some(&overflow_text(count, ty)));
                label.style_context().add_class("mudl-change-overflow");
                label.set_margin_start(14);
                label
            }
        };
        label.set_halign(gtk::Align::Start);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        lines.pack_start(&label, false, false, 0);
    }

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_margin_top(4);
    content.set_margin_bottom(4);
    content.set_margin_start(6);
    content.set_margin_end(6);
    content.pack_start(&badge, false, false, 0);
    content.pack_start(&lines, true, true, 0);

    let row = gtk::ListBoxRow::new();
    row.add(&content);
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> LocalStamp {
        LocalStamp {
            year,
            month,
            day,
            hour,
            minute,
        }
    }

    fn group(id: &str, ty: ChangeType, is_mixed: bool, members: &[ChangeType]) -> ChangeGroup {
        ChangeGroup {
            group_id: id.to_string(),
            index: 1,
            ty,
            is_mixed,
            change_ids: vec![format!("change-{id}")],
            members: members.iter().map(|t| (*t, "x".to_string())).collect(),
            source_line: 1,
        }
    }

    fn groups(n: usize) -> Vec<ChangeGroup> {
        (1..=n)
            .map(|i| {
                group(
                    &format!("group-{i}"),
                    ChangeType::Insertion,
                    false,
                    &[ChangeType::Insertion],
                )
            })
            .collect()
    }

    const NOW: LocalStamp = LocalStamp {
        year: 2026,
        month: 3,
        day: 1,
        hour: 15,
        minute: 4,
    };

    // -- short_timestamp

    #[test]
    fn today_is_lowercase_twelve_hour_time() {
        assert_eq!(short_timestamp(stamp(2026, 3, 1, 10, 19), NOW), "10:19am");
        assert_eq!(short_timestamp(stamp(2026, 3, 1, 15, 4), NOW), "3:04pm");
        assert_eq!(short_timestamp(stamp(2026, 3, 1, 0, 5), NOW), "12:05am");
        assert_eq!(short_timestamp(stamp(2026, 3, 1, 12, 0), NOW), "12:00pm");
    }

    #[test]
    fn previous_day_is_yesterday_even_across_a_month_end() {
        // 2026 is not a leap year: Feb 28 is the day before Mar 1.
        assert_eq!(
            short_timestamp(stamp(2026, 2, 28, 23, 59), NOW),
            "yesterday"
        );
        let new_year = stamp(2027, 1, 1, 9, 0);
        assert_eq!(
            short_timestamp(stamp(2026, 12, 31, 9, 0), new_year),
            "yesterday"
        );
    }

    #[test]
    fn older_dates_show_month_and_day() {
        assert_eq!(short_timestamp(stamp(2026, 2, 27, 8, 0), NOW), "Feb 27");
        assert_eq!(short_timestamp(stamp(2026, 1, 2, 8, 0), NOW), "Jan 2");
    }

    #[test]
    fn other_years_add_the_year() {
        assert_eq!(short_timestamp(stamp(2025, 8, 2, 8, 0), NOW), "Aug 2, 2025");
        let new_year = stamp(2027, 1, 1, 9, 0);
        assert_eq!(
            short_timestamp(stamp(2026, 12, 30, 9, 0), new_year),
            "Dec 30, 2026"
        );
    }

    #[test]
    fn days_from_civil_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn local_stamp_of_now_is_plausible() {
        let now = local_stamp(SystemTime::now()).expect("GLib converts now");
        assert!(now.year >= 2024);
        assert!((1..=12).contains(&now.month));
        assert!(now.hour < 24 && now.minute < 60);
    }

    // -- status_label

    #[test]
    fn status_label_pluralizes_and_names_the_baseline_time() {
        let at = Some(stamp(2026, 3, 1, 10, 19));
        assert_eq!(status_label(0, at, NOW), "0 changes since 10:19am");
        assert_eq!(status_label(1, at, NOW), "1 change since 10:19am");
        assert_eq!(status_label(3, at, NOW), "3 changes since 10:19am");
        assert_eq!(status_label(2, None, NOW), "2 changes");
    }

    // -- badges

    #[test]
    fn badge_kind_from_member_types() {
        use ChangeType::{Deletion as D, Insertion as I};
        assert_eq!(badge_kind(&[]), BadgeKind::None);
        assert_eq!(badge_kind(&[group("g1", I, false, &[I])]), BadgeKind::Ins);
        assert_eq!(badge_kind(&[group("g1", D, false, &[D])]), BadgeKind::Del);
        assert_eq!(
            badge_kind(&[group("g1", I, false, &[I]), group("g2", D, false, &[D])]),
            BadgeKind::Mix
        );
        assert_eq!(badge_kind(&[group("g1", I, true, &[D, I])]), BadgeKind::Mix);
    }

    #[test]
    fn group_badge_kind_prefers_mixed() {
        use ChangeType::{Deletion as D, Insertion as I};
        assert_eq!(
            group_badge_kind(&group("g", I, true, &[I, D])),
            BadgeKind::Mix
        );
        assert_eq!(
            group_badge_kind(&group("g", I, false, &[I])),
            BadgeKind::Ins
        );
        assert_eq!(
            group_badge_kind(&group("g", D, false, &[D])),
            BadgeKind::Del
        );
    }

    fn item(label: &str, count: usize, ins: bool, del: bool) -> MenuItem {
        MenuItem {
            id: 1,
            label: label.to_string(),
            at: UNIX_EPOCH,
            change_count: count,
            is_active: false,
            has_insertions: ins,
            has_deletions: del,
        }
    }

    #[test]
    fn item_badge_kind_is_gray_at_zero() {
        assert_eq!(item_badge_kind(&item("x", 0, true, true)), BadgeKind::None);
        assert_eq!(item_badge_kind(&item("x", 2, true, true)), BadgeKind::Mix);
        assert_eq!(item_badge_kind(&item("x", 1, false, true)), BadgeKind::Del);
        assert_eq!(item_badge_kind(&item("x", 1, true, false)), BadgeKind::Ins);
    }

    #[test]
    fn badge_css_classes() {
        assert_eq!(BadgeKind::Ins.css_class(), "ins");
        assert_eq!(BadgeKind::Del.css_class(), "del");
        assert_eq!(BadgeKind::Mix.css_class(), "mix");
        assert_eq!(BadgeKind::None.css_class(), "none");
    }

    // -- navigation

    #[test]
    fn next_group_starts_at_first_and_wraps() {
        let gs = groups(3);
        assert_eq!(next_group(&gs, None), Some(0));
        assert_eq!(next_group(&gs, Some("group-1")), Some(1));
        assert_eq!(next_group(&gs, Some("group-3")), Some(0));
        assert_eq!(next_group(&gs, Some("gone")), Some(0));
        assert_eq!(next_group(&[], None), None);
    }

    #[test]
    fn prev_group_starts_at_last_and_wraps() {
        let gs = groups(3);
        assert_eq!(prev_group(&gs, None), Some(2));
        assert_eq!(prev_group(&gs, Some("group-3")), Some(1));
        assert_eq!(prev_group(&gs, Some("group-1")), Some(2));
        assert_eq!(prev_group(&gs, Some("gone")), Some(2));
        assert_eq!(prev_group(&[], Some("group-1")), None);
    }

    #[test]
    fn single_group_navigation_stays_put() {
        let gs = groups(1);
        assert_eq!(next_group(&gs, Some("group-1")), Some(0));
        assert_eq!(prev_group(&gs, Some("group-1")), Some(0));
    }

    #[test]
    fn scroll_script_passes_escaped_ids() {
        let script = scroll_to_change_script(&["change-1".to_string(), "a\"b".to_string()]);
        assert!(script.contains("Mud.scrollToChange([\"change-1\", \"a\\\"b\"])"));
        assert!(script.starts_with("if (window.Mud && Mud.scrollToChange)"));
    }

    // -- pane rows

    fn members(types: &[ChangeType]) -> Vec<(ChangeType, String)> {
        types
            .iter()
            .enumerate()
            .map(|(i, t)| (*t, format!("m{i}")))
            .collect()
    }

    #[test]
    fn short_runs_show_every_member() {
        use ChangeType::{Deletion as D, Insertion as I};
        let lines = display_lines(&members(&[D, D, D, I]));
        assert_eq!(
            lines,
            vec![
                DisplayLine::Member(D, "m0".into()),
                DisplayLine::Member(D, "m1".into()),
                DisplayLine::Member(D, "m2".into()),
                DisplayLine::Member(I, "m3".into()),
            ]
        );
    }

    #[test]
    fn long_runs_collapse_to_two_lines_and_an_overflow() {
        use ChangeType::{Deletion as D, Insertion as I};
        let lines = display_lines(&members(&[D, I, I, I, I, I]));
        assert_eq!(
            lines,
            vec![
                DisplayLine::Member(D, "m0".into()),
                DisplayLine::Member(I, "m1".into()),
                DisplayLine::Member(I, "m2".into()),
                DisplayLine::Overflow(3, I),
            ]
        );
    }

    #[test]
    fn member_and_overflow_text() {
        assert_eq!(member_text(ChangeType::Insertion, "Hello"), "+ Hello");
        assert_eq!(member_text(ChangeType::Deletion, "Bye"), "\u{2212} Bye");
        assert_eq!(member_text(ChangeType::Deletion, ""), "\u{2212} \u{2014}");
        assert_eq!(
            overflow_text(1, ChangeType::Insertion),
            "and 1 more insertion"
        );
        assert_eq!(
            overflow_text(4, ChangeType::Deletion),
            "and 4 more deletions"
        );
    }

    // -- popover

    #[test]
    fn split_menu_items_separates_milestones_from_recent() {
        let (milestones, recent) = split_menu_items(vec![
            item("since last accepted", 1, true, false),
            item("since 3 minutes ago", 1, true, false),
            item("since 1 hour ago", 0, false, false),
            item("since document opened", 2, true, true),
        ]);
        let labels = |v: &[MenuItem]| v.iter().map(|i| i.label.clone()).collect::<Vec<_>>();
        assert_eq!(
            labels(&milestones),
            vec!["since last accepted", "since document opened"]
        );
        assert_eq!(
            labels(&recent),
            vec!["since 3 minutes ago", "since 1 hour ago"]
        );
    }

    #[test]
    fn bar_alignment_follows_the_preference() {
        assert_eq!(
            bar_alignment(FloatingControlsPosition::BottomCenter),
            (gtk::Align::Center, gtk::Align::End)
        );
        assert_eq!(
            bar_alignment(FloatingControlsPosition::TopLeft),
            (gtk::Align::Start, gtk::Align::Start)
        );
    }
}

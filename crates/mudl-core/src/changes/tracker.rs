//! The per-window waypoint history (`mud`'s `ChangeTracker.swift`, minus
//! `.external` waypoints; `_working/DIFF-IMPLEMENTATION.md` §8.2).
//!
//! A *waypoint* is a snapshot of the document's text. The tracker keeps:
//!
//! - one `Initial` waypoint, taken when the document opens ([`ChangeTracker::seed`]);
//! - up to [`MAX_RELOAD_COUNT`] `Reload` waypoints, at most one per
//!   absolute clock minute;
//! - at most one `Accept` waypoint, replaced in place on every accept.
//!
//! The diff is drawn against the *active* waypoint: the selected one, else
//! the last `Accept`, else `Initial`.
//!
//! Nothing here reads a clock: every time is passed in as a `SystemTime`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mudl_diff::block::DefinitionPolicy;
use mudl_diff::list::{ChangeGroup, ChangeType, DocumentChange};

use super::{compute_changes, TrackerMode, Waypoint, DEFAULT_WORD_DIFF_THRESHOLD};

/// Most `Reload` waypoints retained; the oldest go first
/// (`ChangeTracker.maxReloadCount`). `Initial` and `Accept` never count.
pub const MAX_RELOAD_COUNT: usize = 10;

/// How a waypoint came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaypointKind {
    /// The document as opened.
    Initial,
    /// A save seen while open.
    Reload,
    /// The content the user last accepted.
    Accept,
}

/// One snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedWaypoint {
    /// Unique within its tracker; never reused.
    pub id: u64,
    /// The whole file, frontmatter included.
    pub markdown: Arc<str>,
    pub at: SystemTime,
    pub kind: WaypointKind,
}

/// One row of the "Changes since…" picker (`ChangeMenuItem`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    /// The waypoint selecting this row makes active.
    pub id: u64,
    pub label: String,
    /// The milestone's own time, or for a time-gap row the *newer*
    /// waypoint's time (the one its label counts from).
    pub at: SystemTime,
    /// Change groups between this waypoint and the current content.
    pub change_count: usize,
    pub is_active: bool,
    pub has_insertions: bool,
    pub has_deletions: bool,
}

/// Per-waypoint diff summary for the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiffSummary {
    group_count: usize,
    has_insertions: bool,
    has_deletions: bool,
}

/// The policy dedup and menu counts use, whatever the mode
/// (`computeChanges(mode: nil)`): the broader one, so a definition-only
/// edit still counts as a real save.
const CONTENT_POLICY: DefinitionPolicy = DefinitionPolicy::DescendPlainFootnotes;

/// The waypoint history of one open document.
#[derive(Debug, Default)]
pub struct ChangeTracker {
    waypoints: Vec<TrackedWaypoint>,
    /// The change list against the active waypoint, under `mode`'s policy.
    changes: Vec<DocumentChange>,
    /// The explicitly selected waypoint, if any.
    selected: Option<u64>,
    mode: TrackerMode,
    /// The most recent content passed to `seed`/`update`.
    current: Option<Arc<str>>,
    /// Waypoint ID → summary; cleared whenever content or waypoints change.
    summaries: HashMap<u64, DiffSummary>,
    next_id: u64,
}

impl ChangeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    // MARK: - Accessors

    /// Every waypoint, in insertion order (an `Accept` keeps its slot when
    /// replaced).
    pub fn waypoints(&self) -> &[TrackedWaypoint] {
        &self.waypoints
    }

    /// True once an `Initial` waypoint exists.
    pub fn is_seeded(&self) -> bool {
        !self.waypoints.is_empty()
    }

    /// The most recent content seen.
    pub fn current(&self) -> Option<&Arc<str>> {
        self.current.as_ref()
    }

    /// The mode the change list follows.
    pub fn mode(&self) -> TrackerMode {
        self.mode
    }

    /// The explicitly selected waypoint (`activeWaypointID`); `None` means
    /// the default (last Accept, else Initial).
    pub fn selected_waypoint(&self) -> Option<u64> {
        self.selected
    }

    /// The waypoint the diff is drawn against: selected ?? last Accept ??
    /// Initial. `None` only before seeding.
    pub fn active(&self) -> Option<&TrackedWaypoint> {
        if let Some(wp) = self
            .selected
            .and_then(|id| self.waypoints.iter().find(|w| w.id == id))
        {
            return Some(wp);
        }
        self.last_accept().or_else(|| self.initial())
    }

    /// The change list against the active waypoint, under the current
    /// mode's policy. Empty right after an accept.
    pub fn changes(&self) -> &[DocumentChange] {
        &self.changes
    }

    /// [`Self::changes`] bucketed into groups (the bar and sidebar rows).
    pub fn groups(&self) -> Vec<ChangeGroup> {
        ChangeGroup::build(&self.changes)
    }

    // MARK: - Mutation

    /// Records the document as opened. A no-op once seeded.
    pub fn seed(&mut self, markdown: Arc<str>, at: SystemTime) {
        if self.is_seeded() {
            return;
        }
        self.push(markdown.clone(), at, WaypointKind::Initial);
        self.current = Some(markdown);
        self.changes.clear();
        self.summaries.clear();
    }

    /// Records a load of the document (`update(_:at:)`). The first call on
    /// an unseeded tracker seeds it. Otherwise a new `Reload` waypoint is
    /// stored unless the save is redundant, and the change list is
    /// recomputed either way. Returns false for a redundant save.
    pub fn update(&mut self, markdown: Arc<str>, at: SystemTime) -> bool {
        if !self.is_seeded() {
            self.seed(markdown, at);
            return true;
        }
        self.current = Some(markdown.clone());
        self.summaries.clear();

        let stored = !self.is_redundant_save(&markdown);
        if stored {
            // ChangeTracker.swift:182-188 — one reload per absolute minute.
            let minute = absolute_minute(at);
            self.waypoints
                .retain(|w| !(w.kind == WaypointKind::Reload && absolute_minute(w.at) == minute));
            self.push(markdown, at, WaypointKind::Reload);
            self.cap_reloads();
        }
        self.recompute_changes();
        stored
    }

    /// Sets the mode the change list follows, recomputing it only when the
    /// mode actually changes.
    pub fn set_mode(&mut self, mode: TrackerMode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        self.recompute_changes();
    }

    /// Makes `id` the diff reference (`None` = the default) and recomputes
    /// the change list.
    pub fn select_waypoint(&mut self, id: Option<u64>) {
        self.selected = id;
        self.recompute_changes();
    }

    /// Accepts the current content: it becomes the single `Accept`
    /// waypoint (replacing any earlier one in place), the selection
    /// resets, and the change list empties until the next edit.
    pub fn accept(&mut self, at: SystemTime) {
        let Some(current) = self.current.clone() else {
            return;
        };
        let id = self.mint_id();
        let waypoint = TrackedWaypoint {
            id,
            markdown: current,
            at,
            kind: WaypointKind::Accept,
        };
        match self
            .waypoints
            .iter_mut()
            .find(|w| w.kind == WaypointKind::Accept)
        {
            Some(slot) => *slot = waypoint,
            None => self.waypoints.push(waypoint),
        }
        self.changes.clear();
        self.selected = None;
        self.summaries.clear();
    }

    // MARK: - Rendering

    /// What the renderer should diff against: `None` when tracking is
    /// disabled or there is nothing to show (no change markup or assets at
    /// all, as in `mud`). Call [`Self::set_mode`] first so "nothing to
    /// show" is judged under the policy being rendered.
    pub fn render_waypoint(
        &self,
        enabled: bool,
        show_inline_deletions: bool,
        word_diff_threshold_pct: u8,
    ) -> Option<Waypoint> {
        if !enabled || self.changes.is_empty() {
            return None;
        }
        Some(Waypoint {
            old_markdown: self.active()?.markdown.clone(),
            show_inline_deletions,
            word_diff_threshold_pct,
        })
    }

    // MARK: - Menu

    /// The "Changes since…" rows (`computeMenuItems`):
    ///
    /// 1. "since last accepted" when an Accept exists, else "since document
    ///    opened".
    /// 2. One row per gap between consecutive waypoints (by time), newest
    ///    first, labelled from the newer end ("since 3 minutes ago") and
    ///    counted from the older end against the current content. Gaps
    ///    whose older end is a milestone, and gaps with no changes, are
    ///    left out.
    /// 3. "since document opened" again, at the bottom, when an Accept
    ///    exists.
    ///
    /// Counts are cached per waypoint until the next `update`/`accept`.
    pub fn menu_items(&mut self, now: SystemTime) -> Vec<MenuItem> {
        let Some(current) = self.current.clone() else {
            return Vec::new();
        };
        let mut items = Vec::new();
        let mut used: Vec<u64> = Vec::new();

        let accept = self.last_accept().cloned();
        let initial = self.initial().cloned();
        let primary = accept.clone().or_else(|| initial.clone());

        if let Some(wp) = &primary {
            let label = if accept.is_some() {
                "since last accepted"
            } else {
                "since document opened"
            };
            items.push(self.menu_item(wp, label.to_string(), wp.at, &current));
            used.push(wp.id);
        }
        // Reserve the initial so the gap walk never claims it.
        if let Some(wp) = &initial {
            used.push(wp.id);
        }

        let gap_start = items.len();
        let mut ordered = self.waypoints.clone();
        ordered.sort_by_key(|w| w.at);
        for pair in ordered.windows(2) {
            let (old, new) = (&pair[0], &pair[1]);
            if used.contains(&old.id) {
                continue;
            }
            let summary = self.summary(old, &current);
            if summary.group_count == 0 {
                continue;
            }
            let label = format!("since {} ago", time_ago_phrase(new.at, now));
            items.push(self.menu_item(old, label, new.at, &current));
            used.push(old.id);
        }
        items[gap_start..].reverse();

        if accept.is_some() {
            if let Some(wp) = &initial {
                if primary.as_ref().map(|p| p.id) != Some(wp.id) {
                    items.push(self.menu_item(
                        wp,
                        "since document opened".to_string(),
                        wp.at,
                        &current,
                    ));
                }
            }
        }
        items
    }

    fn menu_item(
        &mut self,
        wp: &TrackedWaypoint,
        label: String,
        at: SystemTime,
        current: &str,
    ) -> MenuItem {
        let summary = self.summary(wp, current);
        MenuItem {
            id: wp.id,
            label,
            at,
            change_count: summary.group_count,
            is_active: self.is_active(wp),
            has_insertions: summary.has_insertions,
            has_deletions: summary.has_deletions,
        }
    }

    // MARK: - Internals

    fn mint_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn push(&mut self, markdown: Arc<str>, at: SystemTime, kind: WaypointKind) {
        let id = self.mint_id();
        self.waypoints.push(TrackedWaypoint {
            id,
            markdown,
            at,
            kind,
        });
    }

    fn initial(&self) -> Option<&TrackedWaypoint> {
        self.waypoints
            .iter()
            .find(|w| w.kind == WaypointKind::Initial)
    }

    fn last_accept(&self) -> Option<&TrackedWaypoint> {
        self.waypoints
            .iter()
            .rev()
            .find(|w| w.kind == WaypointKind::Accept)
    }

    /// `isActiveWaypoint`.
    fn is_active(&self, wp: &TrackedWaypoint) -> bool {
        if let Some(id) = self.selected {
            return wp.id == id;
        }
        match self.last_accept() {
            Some(accept) => wp.id == accept.id,
            None => wp.kind == WaypointKind::Initial,
        }
    }

    /// `isRedundantSave`: the raw text equals any waypoint's, or the save
    /// diffs to nothing against the most recent waypoint (blank-line-only
    /// edits, re-wraps, comment-only edits).
    fn is_redundant_save(&self, markdown: &str) -> bool {
        if self.waypoints.iter().any(|w| *w.markdown == *markdown) {
            return true;
        }
        let Some(most_recent) = self.waypoints.last() else {
            return false;
        };
        compute_changes(
            &most_recent.markdown,
            markdown,
            CONTENT_POLICY,
            DEFAULT_WORD_DIFF_THRESHOLD,
        )
        .is_empty()
    }

    /// Drops the oldest reloads beyond the cap, resetting an orphaned
    /// selection (ChangeTracker.swift:191-205).
    fn cap_reloads(&mut self) {
        let mut reloads: Vec<(SystemTime, u64)> = self
            .waypoints
            .iter()
            .filter(|w| w.kind == WaypointKind::Reload)
            .map(|w| (w.at, w.id))
            .collect();
        if reloads.len() <= MAX_RELOAD_COUNT {
            return;
        }
        reloads.sort_by_key(|&(at, _)| at);
        let drop: Vec<u64> = reloads[..reloads.len() - MAX_RELOAD_COUNT]
            .iter()
            .map(|&(_, id)| id)
            .collect();
        self.waypoints.retain(|w| !drop.contains(&w.id));
        if let Some(id) = self.selected {
            if !self.waypoints.iter().any(|w| w.id == id) {
                self.selected = None;
            }
        }
    }

    /// Re-diffs the current content against the active waypoint under the
    /// current mode.
    fn recompute_changes(&mut self) {
        self.changes = match (&self.current, self.active()) {
            (Some(current), Some(active)) => compute_changes(
                &active.markdown,
                current,
                self.mode.policy(),
                DEFAULT_WORD_DIFF_THRESHOLD,
            ),
            _ => Vec::new(),
        };
    }

    /// `diffSummary`, cached per waypoint ID.
    fn summary(&mut self, wp: &TrackedWaypoint, current: &str) -> DiffSummary {
        if let Some(summary) = self.summaries.get(&wp.id) {
            return *summary;
        }
        let changes = compute_changes(
            &wp.markdown,
            current,
            CONTENT_POLICY,
            DEFAULT_WORD_DIFF_THRESHOLD,
        );
        let summary = DiffSummary {
            group_count: ChangeGroup::build(&changes).len(),
            has_insertions: changes.iter().any(|c| c.ty == ChangeType::Insertion),
            has_deletions: changes.iter().any(|c| c.ty == ChangeType::Deletion),
        };
        self.summaries.insert(wp.id, summary);
        summary
    }

    /// Number of cached summaries (tests).
    #[cfg(test)]
    pub(crate) fn cached_summary_count(&self) -> usize {
        self.summaries.len()
    }
}

/// Whole seconds since the Unix epoch, negative before it.
fn epoch_secs(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// The absolute clock minute `t` falls in (`absoluteMinute`). Swift counts
/// from 2001-01-01, which is a whole number of minutes after the Unix
/// epoch, so minute boundaries agree.
pub fn absolute_minute(t: SystemTime) -> i64 {
    epoch_secs(t).div_euclid(60)
}

/// The "N minutes ago" count for `anchor` (`minutesAgoLabel`): whole
/// minutes between the two absolute minutes, plus one when `now` is past
/// its minute's start, floored at 1.
pub fn minutes_ago(anchor: SystemTime, now: SystemTime) -> i64 {
    let secs_into_now_minute = epoch_secs(now).rem_euclid(60);
    let delta = absolute_minute(now) - absolute_minute(anchor);
    (delta + i64::from(secs_into_now_minute > 0)).max(1)
}

/// The elapsed-time phrase for a gap row (`timeAgoPhrase`): exact minutes
/// under an hour, then whole hours under a day, days under a week, then
/// weeks — all rounded down.
pub fn time_ago_phrase(anchor: SystemTime, now: SystemTime) -> String {
    fn phrase(count: i64, unit: &str) -> String {
        let plural = if count == 1 { "" } else { "s" };
        format!("{count} {unit}{plural}")
    }
    let minutes = minutes_ago(anchor, now);
    if minutes < 60 {
        return phrase(minutes, "minute");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return phrase(hours, "hour");
    }
    let days = hours / 24;
    if days < 7 {
        return phrase(days, "day");
    }
    phrase(days / 7, "week")
}

#[cfg(test)]
mod tests;

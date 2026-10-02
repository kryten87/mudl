//! Change tracking glue between Markdown and the `mudl-diff` engine
//! (`_working/DIFF-IMPLEMENTATION.md` §8).
//!
//! - [`collect`]: `pulldown-cmark` events → leaf blocks.
//! - [`plan`] / [`compute_changes`] / [`compute_groups`]: diff two documents,
//!   through one process-wide plan cache shared with the renderers.
//! - [`tracker`]: the per-window waypoint history (`ChangeTracker`).
//!
//! # Frontmatter and line numbers
//!
//! Frontmatter is never diffed: both sides are reduced to their
//! [`body`] (the same text `render_up` parses) before collection, so every
//! leaf's `byte_range` and `source_line` are relative to the **body**.
//! `render_down` numbers the lines of the **whole file**, frontmatter
//! included; add [`body_line_offset`] to a body line to get a file line.

pub mod collect;
pub mod tracker;

use std::borrow::Cow;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use mudl_diff::block::{DefinitionPolicy, LeafBlock};
use mudl_diff::list::{self, ChangeGroup, DocumentChange};
use mudl_diff::plan::{ChangePlan, PlanCache, PlanKey};

use crate::frontmatter;
use crate::parse::ParsedMarkdown;

pub use mudl_diff::plan::DEFAULT_WORD_DIFF_THRESHOLD;
pub use tracker::{ChangeTracker, MenuItem, TrackedWaypoint, WaypointKind};

/// The document view a diff is drawn in; it decides the
/// [`DefinitionPolicy`] (mudl-core's stand-in for the server's `Mode`,
/// which it cannot depend on).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TrackerMode {
    /// Rendered view: footnote definitions are invisible to the diff.
    #[default]
    Up,
    /// Raw-source view: plain footnote definitions diff like any line.
    Down,
}

impl TrackerMode {
    /// Up → [`DefinitionPolicy::SkipAll`]; Down →
    /// [`DefinitionPolicy::DescendPlainFootnotes`] (`MudCore.computeChanges`).
    pub fn policy(self) -> DefinitionPolicy {
        match self {
            TrackerMode::Up => DefinitionPolicy::SkipAll,
            TrackerMode::Down => DefinitionPolicy::DescendPlainFootnotes,
        }
    }
}

/// What a render diffs against (`RenderOptions.waypoint` in Phase 3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Waypoint {
    /// The whole old file (frontmatter included; renderers strip it).
    /// `Arc` so cloning per request never copies the text.
    pub old_markdown: Arc<str>,
    /// Show deleted words (`<del>`) inside inserted blocks.
    pub show_inline_deletions: bool,
    /// Word-diff similarity threshold as a percentage (25 = 0.25). An
    /// integer keeps `RenderOptions: Eq`.
    pub word_diff_threshold_pct: u8,
}

impl Waypoint {
    /// The threshold as the fraction `mudl-diff` takes, clamped to 0..=1.
    pub fn threshold(&self) -> f64 {
        f64::from(self.word_diff_threshold_pct.min(100)) / 100.0
    }
}

/// The part of `markdown` change tracking diffs: everything after a
/// leading frontmatter block, exactly as `render_up` computes it (so byte
/// ranges agree with its parse). Without frontmatter, `markdown` itself.
pub fn body(markdown: &str) -> Cow<'_, str> {
    match frontmatter::extract(markdown) {
        Some(fm) => Cow::Owned(fm.body),
        None => Cow::Borrowed(markdown),
    }
}

/// How many file lines precede [`body`]'s first line: the frontmatter's
/// line count (both delimiters included), or 0. File line =
/// `source_line + body_line_offset(markdown)`.
pub fn body_line_offset(markdown: &str) -> usize {
    frontmatter::extract(markdown).map_or(0, |fm| fm.line_count)
}

/// Collects the leaf blocks of `markdown`'s [`body`] under `policy`.
pub fn leaves(markdown: &str, policy: DefinitionPolicy) -> Vec<LeafBlock> {
    leaves_of_body(&body(markdown), policy)
}

fn leaves_of_body(body: &str, policy: DefinitionPolicy) -> Vec<LeafBlock> {
    collect::leaf_blocks(&ParsedMarkdown::new(body), policy)
}

/// The process-wide plan cache. One diff serves the Up render, the sidebar
/// list and the menu counts of a reload; `PlanCache` keeps 8 plans.
static PLAN_CACHE: LazyLock<Mutex<PlanCache>> = LazyLock::new(|| Mutex::new(PlanCache::new()));

/// The shared cache [`plan`] uses (for tests and callers wanting to
/// inspect or clear it).
pub fn plan_cache() -> &'static Mutex<PlanCache> {
    &PLAN_CACHE
}

/// Diffs `old_md` against `new_md` (whole files; frontmatter stripped
/// here) through the process-wide cache.
pub fn plan(
    old_md: &str,
    new_md: &str,
    policy: DefinitionPolicy,
    threshold: f64,
) -> Arc<ChangePlan> {
    plan_with_cache(&PLAN_CACHE, old_md, new_md, policy, threshold)
}

/// [`plan`] through a caller-owned cache. The lock is not held while the
/// documents are parsed and diffed; two racing misses both compute and the
/// later insert wins (the plans are identical).
pub fn plan_with_cache(
    cache: &Mutex<PlanCache>,
    old_md: &str,
    new_md: &str,
    policy: DefinitionPolicy,
    threshold: f64,
) -> Arc<ChangePlan> {
    let (old_body, new_body) = (body(old_md), body(new_md));
    let key = PlanKey::new(&old_body, &new_body, threshold, policy);
    if let Some(plan) = lock(cache).get(&key) {
        return plan;
    }
    let plan = ChangePlan::build(
        &leaves_of_body(&old_body, policy),
        &leaves_of_body(&new_body, policy),
        threshold,
    );
    lock(cache).insert(key, plan)
}

/// A poisoned cache is still a valid cache (every mutation is a single
/// `HashMap`/`VecDeque` call), so recover rather than propagate a panic.
fn lock(cache: &Mutex<PlanCache>) -> MutexGuard<'_, PlanCache> {
    cache.lock().unwrap_or_else(|e| e.into_inner())
}

/// The sidebar change list for `old_md` → `new_md` (`MudCore.computeChanges`).
pub fn compute_changes(
    old_md: &str,
    new_md: &str,
    policy: DefinitionPolicy,
    threshold: f64,
) -> Vec<DocumentChange> {
    list::compute_changes(&plan(old_md, new_md, policy, threshold))
}

/// [`compute_changes`], bucketed into change groups.
pub fn compute_groups(
    old_md: &str,
    new_md: &str,
    policy: DefinitionPolicy,
    threshold: f64,
) -> Vec<ChangeGroup> {
    ChangeGroup::build(&compute_changes(old_md, new_md, policy, threshold))
}

#[cfg(test)]
mod tests;

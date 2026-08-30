use crate::alerts::DocCAlertMode;

/// Rendering configuration shared by `render_up` and `render_down`.
///
/// Deliberately minimal for Phase 2: it carries only what the Phase 2
/// renderer needs. Later phases add fields — a `waypoint` for change
/// tracking (Phase 13), comment-related settings (Phase 14), theme/zoom
/// (Phase 3's template assembly) — without breaking existing callers, by
/// design (see §19/§20 of the implementation plan). Don't add fields here
/// speculatively; add them in the phase that needs them.
///
/// Not `Eq`: `Waypoint::word_diff_threshold` is an `f64`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderOptions {
    /// Controls whether/how DocC-style `Kind:` blockquote asides are
    /// detected and styled. GFM `[!NOTE]`-style alerts are always detected
    /// regardless of this setting — matches `mud`'s `AlertDetector`, where
    /// `docCAlertMode` gates only the DocC-aside path.
    pub doc_c_alert_mode: DocCAlertMode,

    /// `true` for a self-contained export (e.g. `--standalone`), `false` for
    /// the live, interactive app view. Gates interactive-only features such
    /// as the Find-feature CSS (see `template::select_assets`). Defaults to
    /// `false`, matching `mud`'s `RenderOptions.standalone` default.
    pub standalone: bool,

    /// When set (Phase 13), `render_up`/`render_down` overlay a change-
    /// tracking diff against `old_markdown` on top of the normal render:
    /// changed blocks (Up mode) or lines (Down mode) are wrapped in
    /// `<ins>`/`<del data-change-id=… data-group-id=…>`, and deleted
    /// content is spliced back in at the position it was removed from.
    /// `None` (the default) renders exactly as before Phase 13 existed.
    pub waypoint: Option<Waypoint>,
}

/// A point in a file's history to diff the current content against.
#[derive(Debug, Clone, PartialEq)]
pub struct Waypoint {
    /// The full Markdown source to diff against (already frontmatter-free
    /// or not — `render_up`/`render_down` strip frontmatter from both sides
    /// consistently).
    pub old_markdown: String,
    /// Passed through to `mudl_diff::plan::ChangePlan::build`: a paired
    /// block's word-level diff is shown only when its similarity is at or
    /// above this threshold (see `mudl_diff::word::has_significant_changes`).
    pub word_diff_threshold: f64,
}

impl Default for Waypoint {
    fn default() -> Self {
        Waypoint {
            old_markdown: String::new(),
            word_diff_threshold: 0.25,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_doc_c_alert_mode_is_extended() {
        assert_eq!(
            RenderOptions::default().doc_c_alert_mode,
            DocCAlertMode::Extended
        );
    }

    #[test]
    fn default_standalone_is_false() {
        assert!(!RenderOptions::default().standalone);
    }
}

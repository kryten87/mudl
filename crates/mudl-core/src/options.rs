use crate::alerts::DocCAlertMode;

/// Rendering configuration shared by `render_up` and `render_down`.
///
/// Deliberately minimal for Phase 2: it carries only what the Phase 2
/// renderer needs. Later phases add fields — comment-related settings
/// (Phase 14), theme/zoom (Phase 3's template assembly) — without breaking
/// existing callers, by design (see §19/§20 of the implementation plan).
/// Don't add fields here speculatively; add them in the phase that needs
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

//! Bridges `mudl-config`'s `Preferences` to `mudl-server`'s
//! `DocumentConfig` (Phase 10.4 of `docs/IMPLEMENTATION-PLAN.md`) — the
//! pure mapping the toolbar's theme/zoom/toggle controls read from and
//! write back through.

use mudl_config::{DocCAlertMode as ConfigDocCAlertMode, Preferences, Theme};
use mudl_core::alerts::DocCAlertMode as CoreDocCAlertMode;
use mudl_core::options::RenderOptions;
use mudl_server::document::DocumentConfig;

/// Builds the config `mudl_server::document::render` needs from the
/// current preferences. Zoom always stays at the baseline `1.0`: Phase
/// 10.4 applies zoom live via `webkit2gtk::WebView::set_zoom_level`
/// instead of `HtmlDocument`'s inline `style="zoom: ..."` (using both at
/// once would double the effect) — see `window.rs`'s load-changed handler.
pub fn document_config(prefs: &Preferences) -> DocumentConfig {
    DocumentConfig {
        render_options: RenderOptions {
            doc_c_alert_mode: map_doc_c_alert_mode(prefs.markdown_doc_c_alert_mode),
            standalone: false,
            waypoint: None,
        },
        theme_css_name: theme_css_asset_name(prefs.theme),
        up_zoom: 1.0,
        down_zoom: 1.0,
        show_line_numbers: prefs.down_mode_show_line_numbers,
        wrap_lines: prefs.down_mode_wrap_lines,
        readable_column: prefs.ui_show_readable_column,
        // Never sourced from `Preferences` (`docs/SECURITY.md` Finding 4) —
        // `toolbar::Context::current_document_config` overrides this from
        // the tab's own per-open in-memory opt-in.
        allow_remote_images: false,
        changes_enabled: prefs.changes_enabled,
        show_inline_deletions: prefs.changes_show_inline_deletions,
        auto_expand_changes: prefs.changes_auto_expand_groups,
        word_diff_threshold_pct: threshold_pct(prefs.changes_word_diff_threshold),
    }
}

/// `changes_word_diff_threshold` (a 0.0..=1.0 fraction) as the integer
/// percentage `mudl_core::changes::Waypoint` carries, clamped so an
/// out-of-range value can't wrap.
pub fn threshold_pct(fraction: f64) -> u8 {
    if fraction.is_nan() {
        return 25;
    }
    (fraction.clamp(0.0, 1.0) * 100.0).round() as u8
}

fn map_doc_c_alert_mode(mode: ConfigDocCAlertMode) -> CoreDocCAlertMode {
    match mode {
        ConfigDocCAlertMode::Off => CoreDocCAlertMode::Off,
        ConfigDocCAlertMode::Common => CoreDocCAlertMode::Common,
        ConfigDocCAlertMode::Extended => CoreDocCAlertMode::Extended,
    }
}

/// The bundled theme stylesheet's asset name for `theme` (see
/// `mudl_server::assets::lookup`).
pub fn theme_css_asset_name(theme: Theme) -> &'static str {
    match theme {
        Theme::Austere => "theme-austere.css",
        Theme::Blues => "theme-blues.css",
        Theme::Earthy => "theme-earthy.css",
        Theme::Riot => "theme-riot.css",
        Theme::System => "theme-system.css",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_theme_to_its_asset_name() {
        assert_eq!(theme_css_asset_name(Theme::Austere), "theme-austere.css");
        assert_eq!(theme_css_asset_name(Theme::Blues), "theme-blues.css");
        assert_eq!(theme_css_asset_name(Theme::Earthy), "theme-earthy.css");
        assert_eq!(theme_css_asset_name(Theme::Riot), "theme-riot.css");
        assert_eq!(theme_css_asset_name(Theme::System), "theme-system.css");
    }

    #[test]
    fn maps_toggle_flags_from_preferences() {
        let prefs = Preferences {
            down_mode_show_line_numbers: false,
            down_mode_wrap_lines: false,
            ui_show_readable_column: true,
            ..Preferences::default()
        };
        let config = document_config(&prefs);
        assert!(!config.show_line_numbers);
        assert!(!config.wrap_lines);
        assert!(config.readable_column);
    }

    #[test]
    fn zoom_stays_at_baseline_regardless_of_preferences() {
        let prefs = Preferences {
            up_mode_zoom_level: 2.0,
            down_mode_zoom_level: 0.5,
            ..Preferences::default()
        };
        let config = document_config(&prefs);
        assert_eq!(config.up_zoom, 1.0);
        assert_eq!(config.down_zoom, 1.0);
    }

    #[test]
    fn maps_doc_c_alert_mode() {
        let prefs = Preferences {
            markdown_doc_c_alert_mode: ConfigDocCAlertMode::Off,
            ..Preferences::default()
        };
        assert_eq!(
            document_config(&prefs).render_options.doc_c_alert_mode,
            CoreDocCAlertMode::Off
        );
    }

    #[test]
    fn maps_change_tracking_preferences() {
        let prefs = Preferences {
            changes_enabled: false,
            changes_show_inline_deletions: true,
            changes_auto_expand_groups: true,
            changes_word_diff_threshold: 0.4,
            ..Preferences::default()
        };
        let config = document_config(&prefs);
        assert!(!config.changes_enabled);
        assert!(config.show_inline_deletions);
        assert!(config.auto_expand_changes);
        assert_eq!(config.word_diff_threshold_pct, 40);
        assert_eq!(config.render_options.waypoint, None);
    }

    #[test]
    fn default_preferences_match_default_document_config_for_changes() {
        let config = document_config(&Preferences::default());
        let default = DocumentConfig::default();
        assert_eq!(config.changes_enabled, default.changes_enabled);
        assert_eq!(
            config.word_diff_threshold_pct,
            default.word_diff_threshold_pct
        );
    }

    #[test]
    fn threshold_pct_rounds_and_clamps() {
        assert_eq!(threshold_pct(0.25), 25);
        assert_eq!(threshold_pct(0.333), 33);
        assert_eq!(threshold_pct(1.0), 100);
        assert_eq!(threshold_pct(2.0), 100);
        assert_eq!(threshold_pct(-1.0), 0);
        assert_eq!(threshold_pct(f64::NAN), 25);
    }
}

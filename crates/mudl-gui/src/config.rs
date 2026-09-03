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
    }
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
}

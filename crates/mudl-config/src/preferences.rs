//! Typed preferences (Phase 7, step 7.3): a validated, defaulted view over
//! the raw `(key, value)` entries [`crate::format`] parses/serializes,
//! covering the subset of `mud`'s preference list that applies to a Linux
//! core viewer (`docs/IMPLEMENTATION-PLAN.md` Appendix B).
//!
//! Every conversion is total: an unknown key is ignored (forward
//! compatibility — a newer `mudl` version's keys don't break an older one
//! reading the same file, and vice versa), and any value that fails to
//! parse or falls outside its valid range is treated exactly like a missing
//! key, falling back to the field's documented default rather than erroring
//! or panicking.

use std::ops::RangeInclusive;

/// Valid range for both zoom-level preferences: keeps a corrupted or
/// hand-edited preferences file from producing a degenerate (zero,
/// negative, or absurdly large) `WebView::set_zoom_level` call.
const ZOOM_RANGE: RangeInclusive<f64> = 0.1..=10.0;

macro_rules! enum_pref {
    ($name:ident { $($variant:ident => $str:literal),+ $(,)? }, default = $default:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($str => Some(Self::$variant),)+
                    _ => None,
                }
            }

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $str),+
                }
            }

            /// Every variant, in declaration order — lets a consumer (e.g.
            /// a GUI dropdown) enumerate the valid set without duplicating
            /// it.
            pub fn all() -> Vec<Self> {
                vec![$(Self::$variant),+]
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::$default
            }
        }
    };
}

enum_pref!(Lighting { Auto => "auto", Bright => "bright", Dark => "dark" }, default = Auto);

enum_pref!(Theme {
    Austere => "austere",
    Blues => "blues",
    Earthy => "earthy",
    Riot => "riot",
    System => "system",
}, default = Earthy);

enum_pref!(FolderOpenBehavior { Index => "index", Tabs => "tabs" }, default = Index);

enum_pref!(SidebarPane { Outline => "outline", Changes => "changes" }, default = Outline);

enum_pref!(DocCAlertMode {
    Off => "off",
    Common => "common",
    Extended => "extended",
}, default = Extended);

enum_pref!(FloatingControlsPosition {
    TopLeft => "top-left",
    TopCenter => "top-center",
    TopRight => "top-right",
    BottomLeft => "bottom-left",
    BottomCenter => "bottom-center",
    BottomRight => "bottom-right",
}, default = BottomCenter);

/// A validated, defaulted preferences snapshot. Construct from raw entries
/// with [`Preferences::from_entries`]; the impure load/save wrapper around
/// this (step 7.4) lives in [`crate::io`].
#[derive(Debug, Clone, PartialEq)]
pub struct Preferences {
    pub lighting: Lighting,
    pub theme: Theme,
    pub folder_open_behavior: FolderOpenBehavior,
    pub up_mode_zoom_level: f64,
    pub down_mode_zoom_level: f64,
    pub up_mode_allow_remote_content: bool,
    pub down_mode_show_line_numbers: bool,
    pub down_mode_wrap_lines: bool,
    pub sidebar_enabled: bool,
    pub sidebar_pane: SidebarPane,
    pub markdown_doc_c_alert_mode: DocCAlertMode,
    pub ui_use_heading_as_title: bool,
    pub ui_show_readable_column: bool,
    pub ui_foldable_headings: bool,
    pub quit_on_close: bool,
    pub enabled_extensions: Vec<String>,
    pub ui_floating_controls_position: FloatingControlsPosition,
    /// Replaces macOS LaunchServices' `openInDefaultBundleID`/
    /// `openInDefaultFormat`: a plain command string (e.g. `"$EDITOR"`).
    /// `None` means "use `xdg-open`".
    pub open_in_default_command: Option<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            lighting: Lighting::default(),
            theme: Theme::default(),
            folder_open_behavior: FolderOpenBehavior::default(),
            up_mode_zoom_level: 1.0,
            down_mode_zoom_level: 1.0,
            up_mode_allow_remote_content: true,
            down_mode_show_line_numbers: true,
            down_mode_wrap_lines: true,
            sidebar_enabled: false,
            sidebar_pane: SidebarPane::default(),
            markdown_doc_c_alert_mode: DocCAlertMode::default(),
            ui_use_heading_as_title: true,
            ui_show_readable_column: false,
            ui_foldable_headings: true,
            quit_on_close: true,
            enabled_extensions: vec!["mermaid".to_string(), "copy-code".to_string()],
            ui_floating_controls_position: FloatingControlsPosition::default(),
            open_in_default_command: None,
        }
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn bool_str(b: bool) -> String {
    if b { "true" } else { "false" }.to_string()
}

fn parse_f64_in_range(s: &str, range: RangeInclusive<f64>) -> Option<f64> {
    s.parse::<f64>().ok().filter(|v| range.contains(v))
}

fn parse_extensions(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

impl Preferences {
    /// Builds a `Preferences` from raw `(key, value)` entries (as produced
    /// by [`crate::format::parse`]), starting from [`Preferences::default`]
    /// and overriding one field per recognized key. Unknown keys are
    /// ignored; a value that fails to parse or validate leaves the field at
    /// its default.
    pub fn from_entries(entries: &[(String, String)]) -> Self {
        let mut prefs = Self::default();
        for (key, value) in entries {
            match key.as_str() {
                "lighting" => {
                    if let Some(v) = Lighting::parse(value) {
                        prefs.lighting = v;
                    }
                }
                "theme" => {
                    if let Some(v) = Theme::parse(value) {
                        prefs.theme = v;
                    }
                }
                "folder_open_behavior" => {
                    if let Some(v) = FolderOpenBehavior::parse(value) {
                        prefs.folder_open_behavior = v;
                    }
                }
                "up_mode_zoom_level" => {
                    if let Some(v) = parse_f64_in_range(value, ZOOM_RANGE) {
                        prefs.up_mode_zoom_level = v;
                    }
                }
                "down_mode_zoom_level" => {
                    if let Some(v) = parse_f64_in_range(value, ZOOM_RANGE) {
                        prefs.down_mode_zoom_level = v;
                    }
                }
                "up_mode_allow_remote_content" => {
                    if let Some(v) = parse_bool(value) {
                        prefs.up_mode_allow_remote_content = v;
                    }
                }
                "down_mode_show_line_numbers" => {
                    if let Some(v) = parse_bool(value) {
                        prefs.down_mode_show_line_numbers = v;
                    }
                }
                "down_mode_wrap_lines" => {
                    if let Some(v) = parse_bool(value) {
                        prefs.down_mode_wrap_lines = v;
                    }
                }
                "sidebar_enabled" => {
                    if let Some(v) = parse_bool(value) {
                        prefs.sidebar_enabled = v;
                    }
                }
                "sidebar_pane" => {
                    if let Some(v) = SidebarPane::parse(value) {
                        prefs.sidebar_pane = v;
                    }
                }
                "markdown_doc_c_alert_mode" => {
                    if let Some(v) = DocCAlertMode::parse(value) {
                        prefs.markdown_doc_c_alert_mode = v;
                    }
                }
                "ui_use_heading_as_title" => {
                    if let Some(v) = parse_bool(value) {
                        prefs.ui_use_heading_as_title = v;
                    }
                }
                "ui_show_readable_column" => {
                    if let Some(v) = parse_bool(value) {
                        prefs.ui_show_readable_column = v;
                    }
                }
                "ui_foldable_headings" => {
                    if let Some(v) = parse_bool(value) {
                        prefs.ui_foldable_headings = v;
                    }
                }
                "quit_on_close" => {
                    if let Some(v) = parse_bool(value) {
                        prefs.quit_on_close = v;
                    }
                }
                "enabled_extensions" => {
                    prefs.enabled_extensions = parse_extensions(value);
                }
                "ui_floating_controls_position" => {
                    if let Some(v) = FloatingControlsPosition::parse(value) {
                        prefs.ui_floating_controls_position = v;
                    }
                }
                "open_in_default_command" => {
                    prefs.open_in_default_command = if value.is_empty() {
                        None
                    } else {
                        Some(value.clone())
                    };
                }
                _ => {} // unknown key: ignored, for forward compatibility
            }
        }
        prefs
    }

    /// The inverse of [`Preferences::from_entries`]: one entry per field, in
    /// a fixed order, suitable for [`crate::format::serialize`].
    pub fn to_entries(&self) -> Vec<(String, String)> {
        vec![
            ("lighting".to_string(), self.lighting.as_str().to_string()),
            ("theme".to_string(), self.theme.as_str().to_string()),
            (
                "folder_open_behavior".to_string(),
                self.folder_open_behavior.as_str().to_string(),
            ),
            (
                "up_mode_zoom_level".to_string(),
                self.up_mode_zoom_level.to_string(),
            ),
            (
                "down_mode_zoom_level".to_string(),
                self.down_mode_zoom_level.to_string(),
            ),
            (
                "up_mode_allow_remote_content".to_string(),
                bool_str(self.up_mode_allow_remote_content),
            ),
            (
                "down_mode_show_line_numbers".to_string(),
                bool_str(self.down_mode_show_line_numbers),
            ),
            (
                "down_mode_wrap_lines".to_string(),
                bool_str(self.down_mode_wrap_lines),
            ),
            (
                "sidebar_enabled".to_string(),
                bool_str(self.sidebar_enabled),
            ),
            (
                "sidebar_pane".to_string(),
                self.sidebar_pane.as_str().to_string(),
            ),
            (
                "markdown_doc_c_alert_mode".to_string(),
                self.markdown_doc_c_alert_mode.as_str().to_string(),
            ),
            (
                "ui_use_heading_as_title".to_string(),
                bool_str(self.ui_use_heading_as_title),
            ),
            (
                "ui_show_readable_column".to_string(),
                bool_str(self.ui_show_readable_column),
            ),
            (
                "ui_foldable_headings".to_string(),
                bool_str(self.ui_foldable_headings),
            ),
            ("quit_on_close".to_string(), bool_str(self.quit_on_close)),
            (
                "enabled_extensions".to_string(),
                self.enabled_extensions.join(","),
            ),
            (
                "ui_floating_controls_position".to_string(),
                self.ui_floating_controls_position.as_str().to_string(),
            ),
            (
                "open_in_default_command".to_string(),
                self.open_in_default_command.clone().unwrap_or_default(),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_entries_yield_defaults() {
        assert_eq!(Preferences::from_entries(&[]), Preferences::default());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let entries = vec![
            ("theme".to_string(), "riot".to_string()),
            ("some_future_key".to_string(), "whatever".to_string()),
        ];
        let prefs = Preferences::from_entries(&entries);
        assert_eq!(prefs.theme, Theme::Riot);
        assert_eq!(
            prefs,
            Preferences {
                theme: Theme::Riot,
                ..Preferences::default()
            }
        );
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let entries = vec![("theme".to_string(), "blues".to_string())];
        let prefs = Preferences::from_entries(&entries);
        assert_eq!(prefs.theme, Theme::Blues);
        assert_eq!(prefs.lighting, Lighting::Auto);
        assert_eq!(prefs.up_mode_zoom_level, 1.0);
        assert!(prefs.quit_on_close);
    }

    #[test]
    fn zoom_level_above_range_falls_back_to_default() {
        let entries = vec![("up_mode_zoom_level".to_string(), "20".to_string())];
        assert_eq!(
            Preferences::from_entries(&entries).up_mode_zoom_level,
            Preferences::default().up_mode_zoom_level
        );
    }

    #[test]
    fn zoom_level_below_range_falls_back_to_default() {
        let entries = vec![("down_mode_zoom_level".to_string(), "0.0".to_string())];
        assert_eq!(
            Preferences::from_entries(&entries).down_mode_zoom_level,
            Preferences::default().down_mode_zoom_level
        );
    }

    #[test]
    fn zoom_level_non_numeric_falls_back_to_default() {
        let entries = vec![("up_mode_zoom_level".to_string(), "not-a-number".to_string())];
        assert_eq!(
            Preferences::from_entries(&entries).up_mode_zoom_level,
            Preferences::default().up_mode_zoom_level
        );
    }

    #[test]
    fn zoom_level_in_range_is_applied() {
        let entries = vec![("up_mode_zoom_level".to_string(), "1.5".to_string())];
        assert_eq!(Preferences::from_entries(&entries).up_mode_zoom_level, 1.5);
    }

    #[test]
    fn unknown_lighting_variant_falls_back_to_default() {
        let entries = vec![("lighting".to_string(), "neon".to_string())];
        assert_eq!(
            Preferences::from_entries(&entries).lighting,
            Lighting::default()
        );
    }

    #[test]
    fn unknown_theme_variant_falls_back_to_default() {
        let entries = vec![("theme".to_string(), "not-a-theme".to_string())];
        assert_eq!(Preferences::from_entries(&entries).theme, Theme::default());
    }

    #[test]
    fn system_theme_variant_parses() {
        let entries = vec![("theme".to_string(), "system".to_string())];
        assert_eq!(Preferences::from_entries(&entries).theme, Theme::System);
    }

    #[test]
    fn theme_all_lists_every_variant_and_each_round_trips_through_as_str() {
        let all = Theme::all();
        assert_eq!(all.len(), 5);
        for theme in all {
            assert_eq!(Theme::parse(theme.as_str()), Some(theme));
        }
    }

    #[test]
    fn unknown_folder_open_behavior_variant_falls_back_to_default() {
        let entries = vec![("folder_open_behavior".to_string(), "windows".to_string())];
        assert_eq!(
            Preferences::from_entries(&entries).folder_open_behavior,
            FolderOpenBehavior::default()
        );
    }

    #[test]
    fn unknown_sidebar_pane_variant_falls_back_to_default() {
        let entries = vec![("sidebar_pane".to_string(), "todo".to_string())];
        assert_eq!(
            Preferences::from_entries(&entries).sidebar_pane,
            SidebarPane::default()
        );
    }

    #[test]
    fn unknown_doc_c_alert_mode_variant_falls_back_to_default() {
        let entries = vec![(
            "markdown_doc_c_alert_mode".to_string(),
            "maximal".to_string(),
        )];
        assert_eq!(
            Preferences::from_entries(&entries).markdown_doc_c_alert_mode,
            DocCAlertMode::default()
        );
    }

    #[test]
    fn unknown_floating_controls_position_variant_falls_back_to_default() {
        let entries = vec![(
            "ui_floating_controls_position".to_string(),
            "middle".to_string(),
        )];
        assert_eq!(
            Preferences::from_entries(&entries).ui_floating_controls_position,
            FloatingControlsPosition::default()
        );
    }

    #[test]
    fn invalid_bool_falls_back_to_default() {
        let entries = vec![("quit_on_close".to_string(), "nope".to_string())];
        assert_eq!(
            Preferences::from_entries(&entries).quit_on_close,
            Preferences::default().quit_on_close
        );
    }

    #[test]
    fn enabled_extensions_parses_comma_separated_list() {
        let entries = vec![(
            "enabled_extensions".to_string(),
            "mermaid, copy-code, foo".to_string(),
        )];
        assert_eq!(
            Preferences::from_entries(&entries).enabled_extensions,
            vec![
                "mermaid".to_string(),
                "copy-code".to_string(),
                "foo".to_string()
            ]
        );
    }

    #[test]
    fn enabled_extensions_empty_value_yields_empty_list() {
        let entries = vec![("enabled_extensions".to_string(), "".to_string())];
        assert_eq!(
            Preferences::from_entries(&entries).enabled_extensions,
            Vec::<String>::new()
        );
    }

    #[test]
    fn open_in_default_command_empty_is_none() {
        let entries = vec![("open_in_default_command".to_string(), "".to_string())];
        assert_eq!(
            Preferences::from_entries(&entries).open_in_default_command,
            None
        );
    }

    #[test]
    fn open_in_default_command_set_is_some() {
        let entries = vec![("open_in_default_command".to_string(), "$EDITOR".to_string())];
        assert_eq!(
            Preferences::from_entries(&entries).open_in_default_command,
            Some("$EDITOR".to_string())
        );
    }

    #[test]
    fn round_trip_default_preferences() {
        let prefs = Preferences::default();
        assert_eq!(Preferences::from_entries(&prefs.to_entries()), prefs);
    }

    #[test]
    fn round_trip_customized_preferences() {
        let prefs = Preferences {
            lighting: Lighting::Dark,
            theme: Theme::Riot,
            folder_open_behavior: FolderOpenBehavior::Tabs,
            up_mode_zoom_level: 1.25,
            down_mode_zoom_level: 0.75,
            up_mode_allow_remote_content: false,
            down_mode_show_line_numbers: false,
            down_mode_wrap_lines: false,
            sidebar_enabled: true,
            sidebar_pane: SidebarPane::Changes,
            markdown_doc_c_alert_mode: DocCAlertMode::Off,
            ui_use_heading_as_title: false,
            ui_show_readable_column: true,
            ui_foldable_headings: false,
            quit_on_close: false,
            enabled_extensions: vec!["mermaid".to_string()],
            ui_floating_controls_position: FloatingControlsPosition::TopRight,
            open_in_default_command: Some("$EDITOR".to_string()),
        };
        assert_eq!(Preferences::from_entries(&prefs.to_entries()), prefs);
    }
}

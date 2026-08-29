//! Maps a bundled asset's request filename (the `<name>` in
//! `/assets/<name>`, per `routes::Route::Asset`) to its embedded content
//! from `mudl_core::resources` (Phase 4, step 4.5 of
//! `docs/IMPLEMENTATION-PLAN.md`).
//!
//! The filenames used as keys here are the same convention
//! `mudl_core::template::select_assets` (Phase 3.3) already returns in its
//! `AssetSelection` — this table is the other half, turning that name back
//! into actual bytes to serve. Only assets that already have a
//! `mudl_core::resources` constant are wired up; anything else simply
//! isn't in the table, so a request for one falls through to `None` (404).

use mudl_core::resources;

/// Looks up the embedded content for a bundled asset by its request
/// filename (e.g. `"mud.css"`, `"highlight.min.js"`). `None` means "not an
/// embedded asset" — the caller should respond 404.
pub fn lookup(name: &str) -> Option<&'static str> {
    match name {
        "mud.css" => Some(resources::MUD_CSS),
        "mud-up.css" => Some(resources::MUD_UP_CSS),
        "mud-down.css" => Some(resources::MUD_DOWN_CSS),
        "mud-narrow.css" => Some(resources::MUD_NARROW_CSS),
        "mud-print.css" => Some(resources::MUD_PRINT_CSS),
        "mud-find.css" => Some(resources::MUD_FIND_CSS),
        "mud-math.css" => Some(resources::MUD_MATH_CSS),
        "theme-austere.css" => Some(resources::THEME_AUSTERE_CSS),
        "theme-blues.css" => Some(resources::THEME_BLUES_CSS),
        "theme-earthy.css" => Some(resources::THEME_EARTHY_CSS),
        "theme-riot.css" => Some(resources::THEME_RIOT_CSS),
        "theme-system.css" => Some(resources::THEME_SYSTEM_CSS),
        "highlight.min.js" => Some(resources::HIGHLIGHT_JS),
        "mermaid.min.js" => Some(resources::MERMAID_JS),
        "temml.min.js" => Some(resources::TEMML_JS),
        "mud.js" => Some(resources::MUD_JS),
        "mud-up.js" => Some(resources::MUD_UP_JS),
        "mud-down.js" => Some(resources::MUD_DOWN_JS),
        "highlight-init.js" => Some(resources::HIGHLIGHT_INIT_JS),
        "math-init.js" => Some(resources::MATH_INIT_JS),
        "mermaid-init.js" => Some(resources::MERMAID_INIT_JS),
        "live-reload.js" => Some(resources::LIVE_RELOAD_JS),
        _ => None,
    }
}

/// The file extension (without the leading dot) of an asset's request
/// filename, for use with `mime::lookup`. No extension (or a filename with
/// a trailing dot and nothing after it) yields an empty string, which
/// `mime::lookup` already maps to the `application/octet-stream` fallback.
pub fn extension_of(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, ext)| ext)
}

#[cfg(test)]
mod lookup_tests {
    use super::*;

    #[test]
    fn known_css_asset_matches_its_resources_constant() {
        assert_eq!(lookup("mud.css"), Some(resources::MUD_CSS));
        assert_eq!(
            lookup("theme-earthy.css"),
            Some(resources::THEME_EARTHY_CSS)
        );
    }

    #[test]
    fn known_js_asset_matches_its_resources_constant() {
        assert_eq!(lookup("highlight.min.js"), Some(resources::HIGHLIGHT_JS));
        assert_eq!(lookup("mermaid.min.js"), Some(resources::MERMAID_JS));
        assert_eq!(lookup("temml.min.js"), Some(resources::TEMML_JS));
        assert_eq!(lookup("mud.js"), Some(resources::MUD_JS));
        assert_eq!(lookup("mud-up.js"), Some(resources::MUD_UP_JS));
        assert_eq!(lookup("mud-down.js"), Some(resources::MUD_DOWN_JS));
        assert_eq!(
            lookup("highlight-init.js"),
            Some(resources::HIGHLIGHT_INIT_JS)
        );
        assert_eq!(lookup("math-init.js"), Some(resources::MATH_INIT_JS));
        assert_eq!(lookup("mermaid-init.js"), Some(resources::MERMAID_INIT_JS));
        assert_eq!(lookup("live-reload.js"), Some(resources::LIVE_RELOAD_JS));
    }

    #[test]
    fn unknown_name_is_none() {
        assert_eq!(lookup("does-not-exist.css"), None);
        assert_eq!(lookup("does-not-exist.js"), None);
        assert_eq!(lookup(""), None);
    }
}

#[cfg(test)]
mod extension_of_tests {
    use super::*;

    #[test]
    fn simple_extension() {
        assert_eq!(extension_of("mud.css"), "css");
    }

    #[test]
    fn multiple_dots_uses_last_segment() {
        assert_eq!(extension_of("highlight.min.js"), "js");
    }

    #[test]
    fn no_extension_is_empty() {
        assert_eq!(extension_of("Makefile"), "");
    }

    #[test]
    fn trailing_dot_is_empty() {
        assert_eq!(extension_of("name."), "");
    }
}

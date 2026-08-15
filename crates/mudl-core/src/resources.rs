//! Static CSS/JS assets vendored from `mud`, embedded at compile time.
//!
//! These are copied byte-for-byte from `mud`'s
//! `Core/Sources/Resources/` (theme/layout CSS) and its bundled
//! third-party libraries (syntax highlighting, diagrams, math rendering).
//! `include_str!` pulls each file into the binary at build time, so there
//! is no runtime filesystem dependency for serving them (Phase 3, steps
//! 3.4-3.5 of `docs/IMPLEMENTATION-PLAN.md`) — `mudl-server`'s asset route
//! (Phase 4) and `mudl-core::template` (document assembly) can reference
//! these constants directly instead of reading from disk.
//!
//! Third-party licenses (highlight.js: BSD-3-Clause, Mermaid: MIT, Temml:
//! MIT) are carried over unchanged inside each file's own header/body — see
//! `mudl/README.md`'s "Third-party assets" section.

/// `mud`'s base stylesheet, shared across Up/Down modes.
pub const MUD_CSS: &str = include_str!("../../../resources/css/mud.css");
/// Up-mode (rendered HTML) specific styles.
pub const MUD_UP_CSS: &str = include_str!("../../../resources/css/mud-up.css");
/// Down-mode (raw source view) specific styles.
pub const MUD_DOWN_CSS: &str = include_str!("../../../resources/css/mud-down.css");
/// Narrow-viewport layout adjustments.
pub const MUD_NARROW_CSS: &str = include_str!("../../../resources/css/mud-narrow.css");
/// Print-media styles.
pub const MUD_PRINT_CSS: &str = include_str!("../../../resources/css/mud-print.css");
/// Find-in-page (interactive, non-standalone mode) styles.
pub const MUD_FIND_CSS: &str = include_str!("../../../resources/css/mud-find.css");
/// Math-block (Temml output) styles.
pub const MUD_MATH_CSS: &str = include_str!("../../../resources/css/mud-math.css");

/// The "Austere" theme.
pub const THEME_AUSTERE_CSS: &str = include_str!("../../../resources/css/theme-austere.css");
/// The "Blues" theme.
pub const THEME_BLUES_CSS: &str = include_str!("../../../resources/css/theme-blues.css");
/// The "Earthy" theme (the default, per `mud`'s preferences).
pub const THEME_EARTHY_CSS: &str = include_str!("../../../resources/css/theme-earthy.css");
/// The "Riot" theme.
pub const THEME_RIOT_CSS: &str = include_str!("../../../resources/css/theme-riot.css");
/// The "System" theme (follows OS light/dark mode).
pub const THEME_SYSTEM_CSS: &str = include_str!("../../../resources/css/theme-system.css");

/// [highlight.js](https://highlightjs.org/) (BSD-3-Clause) — client-side
/// syntax highlighting for fenced code blocks.
pub const HIGHLIGHT_JS: &str = include_str!("../../../resources/js/highlight.min.js");
/// [Mermaid](https://mermaid.js.org/) (MIT) — client-side diagram rendering.
pub const MERMAID_JS: &str = include_str!("../../../resources/js/mermaid.min.js");
/// [Temml](https://temml.org/) (MIT) — client-side TeX-to-MathML math
/// rendering.
pub const TEMML_JS: &str = include_str!("../../../resources/js/temml.min.js");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_constants_are_non_empty() {
        assert!(!MUD_CSS.is_empty());
        assert!(!MUD_UP_CSS.is_empty());
        assert!(!MUD_DOWN_CSS.is_empty());
        assert!(!MUD_NARROW_CSS.is_empty());
        assert!(!MUD_PRINT_CSS.is_empty());
        assert!(!MUD_FIND_CSS.is_empty());
        assert!(!MUD_MATH_CSS.is_empty());
        assert!(!THEME_AUSTERE_CSS.is_empty());
        assert!(!THEME_BLUES_CSS.is_empty());
        assert!(!THEME_EARTHY_CSS.is_empty());
        assert!(!THEME_RIOT_CSS.is_empty());
        assert!(!THEME_SYSTEM_CSS.is_empty());
    }

    #[test]
    fn js_constants_are_non_empty() {
        assert!(!HIGHLIGHT_JS.is_empty());
        assert!(!MERMAID_JS.is_empty());
        assert!(!TEMML_JS.is_empty());
    }

    #[test]
    fn highlight_js_retains_its_license_banner() {
        assert!(HIGHLIGHT_JS.contains("Highlight.js"));
        assert!(HIGHLIGHT_JS.contains("License: BSD-3-Clause"));
    }

    #[test]
    fn temml_js_retains_its_license_banner() {
        assert!(TEMML_JS.contains("Temml"));
        assert!(TEMML_JS.contains("License: MIT"));
    }

    #[test]
    fn mermaid_js_retains_its_embedded_license_text() {
        assert!(MERMAID_JS.contains("MIT License"));
    }

    #[test]
    fn theme_css_files_match_their_named_theme() {
        // Spot-check each constant's content against its own header
        // comment, so a copy/paste mistake (e.g. two constants pointing at
        // the same file) would fail loudly.
        assert!(THEME_AUSTERE_CSS.contains("Theme: Austere"));
        assert!(THEME_BLUES_CSS.contains("Theme: Blues"));
        assert!(THEME_EARTHY_CSS.contains("Theme: Earthy"));
        assert!(THEME_RIOT_CSS.contains("Theme: Riot"));
        assert!(THEME_SYSTEM_CSS.contains("Theme: System"));
    }

    #[test]
    fn layout_css_files_match_their_named_purpose() {
        assert!(MUD_UP_CSS.contains("Up Mode"));
        assert!(MUD_DOWN_CSS.contains("Down Mode"));
        assert!(MUD_FIND_CSS.contains("Find highlights"));
    }
}

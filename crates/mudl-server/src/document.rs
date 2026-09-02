//! Assembles the full HTML page served at `/` (Phase 10.1 of
//! `docs/IMPLEMENTATION-PLAN.md`): renders the requested mode via
//! `mudl_core::render::{render_up, render_down}`, rewrites local image
//! `src`s through the `/local/` route and local link `href`s through the
//! `/local-md/`/`/local-file/` routes (`mudl-gui`'s WebView navigation
//! handler intercepts those before they ever reach this server — see
//! `mudl-gui/src/linkaction.rs`), wraps the body in the
//! `up-mode-output`/`down-mode-output` marker `mudl_core::template::select_assets`
//! keys off of, resolves the resulting asset selection to embedded CSS
//! content and `/assets/<name>` script tags, and appends the live-reload
//! bootstrap (plan §2) as the last script.
//!
//! Pure given its inputs — no I/O happens here. `server.rs` is what reads
//! the file fresh from disk (via the injected `FileSystem`) on every
//! request and passes its contents in.

use mudl_core::options::RenderOptions;
use mudl_core::render::{render_down, render_up};
use mudl_core::resources;
use mudl_core::template::{
    rewrite_local_image_srcs_with_paths, rewrite_local_link_hrefs, select_assets, HtmlDocument,
    Script,
};

use std::path::{Path, PathBuf};

use crate::assets;
use crate::routes::Mode;

/// Per-server-instance rendering configuration — the pieces of `mudl-config`
/// preferences that affect how the document looks, kept as plain fields
/// rather than a `mudl-config` dependency (`mudl-server` stays decoupled
/// from preferences file I/O; `mudl-gui` reads `Preferences` and maps it to
/// this).
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentConfig {
    pub render_options: RenderOptions,
    /// The bundled theme stylesheet's asset name, e.g. `"theme-earthy.css"`.
    pub theme_css_name: &'static str,
    pub up_zoom: f64,
    pub down_zoom: f64,
    /// Down mode's `has-line-numbers` root class (`mud-down.css`'s
    /// `html:not(.has-line-numbers) .line { ... }`).
    pub show_line_numbers: bool,
    /// Down mode's `has-word-wrap` root class (`mud-down.css`).
    pub wrap_lines: bool,
    /// Both modes' `is-readable-column` root class (`mud-up.css`/
    /// `mud-down.css`).
    pub readable_column: bool,
}

impl Default for DocumentConfig {
    fn default() -> Self {
        Self {
            render_options: RenderOptions::default(),
            theme_css_name: "theme-earthy.css",
            up_zoom: 1.0,
            down_zoom: 1.0,
            show_line_numbers: true,
            wrap_lines: true,
            readable_column: false,
        }
    }
}

/// Renders the complete HTML page for `markdown` (the file's current
/// contents), in `mode`, at long-poll baseline `version`.
///
/// Also returns the absolute local paths every rewritten `<img src>`
/// resolved to — `server.rs` records these on `DocumentSource` so
/// `Route::LocalFile` can confine itself to exactly the files this render
/// referenced, rather than serving any path a request names (`docs/SECURITY.md`
/// Finding 2).
pub fn render(
    markdown: &str,
    base_dir: &Path,
    title: &str,
    mode: Mode,
    version: u64,
    config: &DocumentConfig,
) -> (String, Vec<PathBuf>) {
    let body = match mode {
        Mode::Up => render_up(markdown, &config.render_options),
        Mode::Down => render_down(markdown, &config.render_options),
    };
    let (body, allowed_local_paths) = rewrite_local_image_srcs_with_paths(&body, base_dir);
    let body = rewrite_local_link_hrefs(&body, base_dir);
    // `data-mudl-version` carries the live-reload baseline to
    // `live-reload.js` without an inline `<script>` — see the
    // `csp_script_src` comment below.
    let wrapped = format!(
        "<div class=\"{}\" data-mudl-version=\"{version}\">{body}</div>",
        wrapper_class(mode)
    );

    let selection = select_assets(&wrapped, &config.render_options);

    let mut styles = vec![resources::MUD_CSS.to_string(), mode_css(mode).to_string()];
    if let Some(theme_css) = assets::lookup(config.theme_css_name) {
        styles.push(theme_css.to_string());
    }
    for name in &selection.stylesheets {
        if let Some(css) = assets::lookup(name) {
            styles.push(css.to_string());
        }
    }

    let mut scripts: Vec<Script> = selection
        .scripts
        .iter()
        .map(|name| Script::Src(format!("/assets/{name}")))
        .collect();
    scripts.push(Script::Src("/assets/live-reload.js".to_string()));

    let doc = HtmlDocument {
        title: title.to_string(),
        base_href: None,
        styles,
        csp_img_src: vec![
            "'self'".to_string(),
            "https:".to_string(),
            "http:".to_string(),
            "data:".to_string(),
        ],
        // No `'unsafe-inline'` (`docs/SECURITY.md` Finding 3): every script
        // this page runs is loaded from `/assets/`, and the live-reload
        // version it used to need an inline bootstrap for now travels as
        // the `data-mudl-version` attribute set above instead.
        csp_script_src: vec!["'self'".to_string()],
        html_classes: html_classes(config),
        zoom_level: match mode {
            Mode::Up => config.up_zoom,
            Mode::Down => config.down_zoom,
        },
        body_content: wrapped,
        body_scripts: scripts,
    };
    (doc.render(), allowed_local_paths)
}

/// The `<html>` root classes matching Phase 10.4's toggle-button state,
/// per the `mud`'s `ViewToggle` -> CSS-class convention (`mud-down.css`/
/// `mud-up.css`).
fn html_classes(config: &DocumentConfig) -> Vec<String> {
    let mut classes = Vec::new();
    if config.show_line_numbers {
        classes.push("has-line-numbers".to_string());
    }
    if config.wrap_lines {
        classes.push("has-word-wrap".to_string());
    }
    if config.readable_column {
        classes.push("is-readable-column".to_string());
    }
    classes
}

fn wrapper_class(mode: Mode) -> &'static str {
    match mode {
        Mode::Up => "up-mode-output",
        Mode::Down => "down-mode-output",
    }
}

fn mode_css(mode: Mode) -> &'static str {
    match mode {
        Mode::Up => resources::MUD_UP_CSS,
        Mode::Down => resources::MUD_DOWN_CSS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn base_dir() -> &'static Path {
        Path::new("/docs")
    }

    #[test]
    fn up_mode_wraps_body_and_includes_up_mode_script() {
        let (html, _) = render(
            "# Hello",
            base_dir(),
            "notes.md",
            Mode::Up,
            0,
            &DocumentConfig::default(),
        );
        assert!(html.contains("class=\"up-mode-output\""));
        assert!(html.contains("<h1"));
        assert!(html.contains("/assets/mud.js"));
        assert!(html.contains("/assets/mud-up.js"));
        assert!(!html.contains("/assets/mud-down.js"));
    }

    #[test]
    fn down_mode_wraps_body_and_includes_down_mode_script() {
        let (html, _) = render(
            "line one",
            base_dir(),
            "notes.md",
            Mode::Down,
            0,
            &DocumentConfig::default(),
        );
        assert!(html.contains("class=\"down-mode-output\""));
        assert!(html.contains("line one"));
        assert!(html.contains("/assets/mud-down.js"));
        assert!(!html.contains("/assets/mud-up.js"));
    }

    #[test]
    fn title_is_used_as_document_title() {
        let (html, _) = render(
            "hi",
            base_dir(),
            "my-notes.md",
            Mode::Up,
            0,
            &DocumentConfig::default(),
        );
        assert!(html.contains("<title>my-notes.md</title>"));
    }

    #[test]
    fn theme_css_content_is_embedded() {
        let (html, _) = render(
            "hi",
            base_dir(),
            "notes.md",
            Mode::Up,
            0,
            &DocumentConfig::default(),
        );
        assert!(html.contains("Theme: Earthy"));
    }

    #[test]
    fn version_is_embedded_for_live_reload() {
        let (html, _) = render(
            "hi",
            base_dir(),
            "notes.md",
            Mode::Up,
            42,
            &DocumentConfig::default(),
        );
        assert!(html.contains("data-mudl-version=\"42\""));
        assert!(html.contains("/assets/live-reload.js"));
    }

    #[test]
    fn local_image_src_is_rewritten_through_local_route() {
        let (html, _) = render(
            "![alt](photo.png)",
            base_dir(),
            "notes.md",
            Mode::Up,
            0,
            &DocumentConfig::default(),
        );
        assert!(html.contains("src=\"/local/"));
    }

    #[test]
    fn local_image_path_is_reported_in_allowed_local_paths() {
        let (_, allowed_local_paths) = render(
            "![alt](photo.png)",
            base_dir(),
            "notes.md",
            Mode::Up,
            0,
            &DocumentConfig::default(),
        );
        assert_eq!(allowed_local_paths, vec![Path::new("/docs/photo.png")]);
    }

    #[test]
    fn no_images_reports_no_allowed_local_paths() {
        let (_, allowed_local_paths) = render(
            "hi",
            base_dir(),
            "notes.md",
            Mode::Up,
            0,
            &DocumentConfig::default(),
        );
        assert!(allowed_local_paths.is_empty());
    }

    #[test]
    fn local_markdown_link_is_rewritten_through_local_md_route() {
        let (html, _) = render(
            "[stub](./stub.md)",
            base_dir(),
            "notes.md",
            Mode::Up,
            0,
            &DocumentConfig::default(),
        );
        assert!(html.contains("href=\"/local-md/"));
    }

    #[test]
    fn other_local_file_link_is_rewritten_through_local_file_route() {
        let (html, _) = render(
            "[text](./example.txt)",
            base_dir(),
            "notes.md",
            Mode::Up,
            0,
            &DocumentConfig::default(),
        );
        assert!(html.contains("href=\"/local-file/"));
    }

    #[test]
    fn anchor_link_is_left_unrewritten() {
        let (html, _) = render(
            "[jump](#section)",
            base_dir(),
            "notes.md",
            Mode::Up,
            0,
            &DocumentConfig::default(),
        );
        assert!(html.contains("href=\"#section\""));
    }

    #[test]
    fn code_fence_selects_highlight_assets() {
        let (html, _) = render(
            "```rust\nfn main() {}\n```",
            base_dir(),
            "notes.md",
            Mode::Up,
            0,
            &DocumentConfig::default(),
        );
        assert!(html.contains("/assets/highlight.min.js"));
        assert!(html.contains("/assets/highlight-init.js"));
    }

    #[test]
    fn zoom_level_applied_per_mode() {
        let config = DocumentConfig {
            up_zoom: 1.5,
            down_zoom: 0.8,
            ..DocumentConfig::default()
        };
        let (up_html, _) = render("hi", base_dir(), "notes.md", Mode::Up, 0, &config);
        let (down_html, _) = render("hi", base_dir(), "notes.md", Mode::Down, 0, &config);
        assert!(up_html.contains("style=\"zoom: 1.5\""));
        assert!(down_html.contains("style=\"zoom: 0.8\""));
    }

    /// Extracts the `<html ...>` opening tag's `class="..."` attribute
    /// value, so assertions check the root element's actual classes rather
    /// than merely finding the class name as a substring somewhere in the
    /// document (it also appears inside the embedded CSS's own selectors,
    /// e.g. `.is-readable-column .down-mode-output { ... }`).
    fn html_root_class_attr(html: &str) -> &str {
        let after = html.split_once("class=\"").expect("no class attr").1;
        after.split_once('"').expect("unterminated class attr").0
    }

    #[test]
    fn default_config_html_classes_match_default_preferences() {
        let (html, _) = render(
            "hi",
            base_dir(),
            "notes.md",
            Mode::Down,
            0,
            &DocumentConfig::default(),
        );
        let classes = html_root_class_attr(&html);
        assert_eq!(classes, "has-line-numbers has-word-wrap");
    }

    #[test]
    fn toggle_flags_control_html_classes() {
        let config = DocumentConfig {
            show_line_numbers: false,
            wrap_lines: false,
            readable_column: true,
            ..DocumentConfig::default()
        };
        let (html, _) = render("hi", base_dir(), "notes.md", Mode::Down, 0, &config);
        let classes = html_root_class_attr(&html);
        assert_eq!(classes, "is-readable-column");
    }
}

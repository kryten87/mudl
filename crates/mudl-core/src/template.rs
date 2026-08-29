//! HTML document template assembly and asset selection.
//!
//! Mirrors `mud`'s `HTMLDocument`/`HTMLTemplate`
//! (`Core/Sources/Rendering/HTMLDocument.swift`) as an independent, pure Rust
//! builder rather than a line-for-line translation — see `HtmlDocument`'s doc
//! comment and `Script` below for where this deliberately diverges.

use std::path::Path;

use crate::encoding::html_escape;
use crate::images::is_external_source;
use crate::options::RenderOptions;

/// A single `<script>` tag to place just before `</body>`.
///
/// The implementation plan's literal signature for `build_scripts` is
/// `&[&str]`, but a bare string can't distinguish an inline script body from
/// a `src=` URL — this enum is the deliberate, documented deviation from
/// that literal text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Script {
    Inline(String),
    Src(String),
}

/// A pure builder for a complete HTML document.
///
/// Unlike Swift's `HTMLDocument`, which derives its fields from
/// `RenderOptions` in `init(options:)`, this builder takes plain fields set
/// directly by the caller. Wiring it up to `RenderOptions` (theme, zoom,
/// title) is deferred to the phase that grows those fields on
/// `RenderOptions` — see that struct's "don't add fields speculatively" doc
/// comment.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HtmlDocument {
    pub title: String,
    /// The `<base href="...">` target, or `None` to omit the tag entirely.
    pub base_href: Option<String>,
    pub styles: Vec<String>,
    pub csp_img_src: Vec<String>,
    pub csp_script_src: Vec<String>,
    pub html_classes: Vec<String>,
    pub zoom_level: f64,
    pub body_content: String,
    pub body_scripts: Vec<Script>,
}

impl HtmlDocument {
    pub fn render(&self) -> String {
        let classes: Vec<&str> = self.html_classes.iter().map(String::as_str).collect();
        let html_attrs = build_html_attributes(&classes, self.zoom_level);

        let directives = self.csp_directives();
        let directive_refs: Vec<&str> = directives.iter().map(String::as_str).collect();
        let csp = build_csp(&directive_refs);

        let base_tag = self
            .base_href
            .as_deref()
            .map(|href| format!("<base href=\"{href}\">"))
            .unwrap_or_default();

        let style_block = self.styles.concat();
        let script_block = build_scripts(&self.body_scripts);
        let title = html_escape(&self.title);
        let body = &self.body_content;

        format!(
            "<!DOCTYPE html>\n\
             <html{html_attrs}>\n\
             <head>\n\
             \x20   <meta charset=\"utf-8\">\n\
             \x20   <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
             \x20   {csp}\n\
             \x20   {base_tag}\n\
             \x20   <title>{title}</title>\n\
             \x20   <style>{style_block}</style>\n\
             </head>\n\
             <body>\n\
             {body}{script_block}\n\
             </body>\n\
             </html>\n"
        )
    }

    /// Builds the ordered list of CSP directive strings, applying the
    /// img-src/script-src special-casing described in the implementation
    /// plan. `build_csp` itself stays ignorant of this policy — it only
    /// joins whatever directive strings it's handed.
    fn csp_directives(&self) -> Vec<String> {
        let mut directives = vec!["default-src 'none'".to_string()];
        if !self.csp_img_src.is_empty() {
            directives.push(format!("img-src {}", self.csp_img_src.join(" ")));
        }
        directives.push("style-src 'unsafe-inline'".to_string());
        if self.csp_script_src.is_empty() {
            directives.push("script-src 'none'".to_string());
        } else {
            directives.push(format!("script-src {}", self.csp_script_src.join(" ")));
        }
        directives
    }
}

/// Joins pre-built CSP directive strings with `"; "` and wraps them in the
/// `<meta>` tag. Knows nothing about which directives are meaningful or how
/// they're derived — that policy lives in `HtmlDocument::csp_directives`.
pub fn build_csp(directives: &[&str]) -> String {
    format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">",
        directives.join("; ")
    )
}

/// Builds the `<html ...>` opening tag's attribute string.
///
/// `zoom_level != 1.0` is a real special case carried over from Swift's
/// `htmlStyles = zoomLevel != 1.0 ? [...] : []`, not a formatting nicety: at
/// exactly 1.0 no `style` attribute is emitted at all, rather than emitting
/// `style="zoom: 1"`.
pub fn build_html_attributes(classes: &[&str], zoom_level: f64) -> String {
    let mut attrs: Vec<String> = Vec::new();
    if !classes.is_empty() {
        attrs.push(format!("class=\"{}\"", classes.join(" ")));
    }
    if zoom_level != 1.0 {
        attrs.push(format!("style=\"zoom: {zoom_level}\""));
    }
    if attrs.is_empty() {
        String::new()
    } else {
        format!(" {}", attrs.join(" "))
    }
}

/// Builds the block of `<script>` tags placed just before `</body>`.
pub fn build_scripts(scripts: &[Script]) -> String {
    if scripts.is_empty() {
        return String::new();
    }
    let tags: Vec<String> = scripts
        .iter()
        .map(|script| match script {
            Script::Inline(source) => format!("<script>{source}</script>"),
            Script::Src(url) => format!("<script src=\"{url}\"></script>"),
        })
        .collect();
    tags.join("\n")
}

/// Produces a bare JS string literal (quotes included), safe to splice into
/// an inline `<script>`.
///
/// `std` has no JSON/JS string encoder, so this hand-rolls the minimal
/// escaping needed: backslash, double quote, and control characters.
/// Non-control Unicode passes through unescaped, since inline `<script>`
/// content is UTF-8 JS source, not a quoted wire format.
pub fn js_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Which bundled CSS/JS assets (identified by their static filename) a
/// rendered document needs. Actual asset content/embedding is later phase
/// work; these are just the identifiers a later phase maps 1:1 to embedded
/// resources.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssetSelection {
    pub stylesheets: Vec<&'static str>,
    pub scripts: Vec<&'static str>,
}

/// Pure, deterministic decision logic for which bundled CSS/JS to include in
/// a rendered document, based only on markers present in `body_html` and on
/// `options.standalone`.
///
/// `body_html` is expected to already carry `mudl-server::document`'s
/// `up-mode-output`/`down-mode-output` wrapper class (Phase 10) — that's how
/// this function tells which of `mud-up.js`/`mud-down.js` applies, the same
/// marker-detection convention it already uses for math/Mermaid/code-fence
/// content. `mud.js` (shared find/scroll/zoom helpers) is unconditional:
/// every served page needs it regardless of mode.
pub fn select_assets(body_html: &str, options: &RenderOptions) -> AssetSelection {
    let mut stylesheets = Vec::new();
    let mut scripts = vec!["mud.js"];

    if body_html.contains("up-mode-output") {
        scripts.push("mud-up.js");
    }
    if body_html.contains("down-mode-output") {
        scripts.push("mud-down.js");
    }

    // A `<math>` element, a `mud-math-block` div (present even when the
    // renderer emits escaped-TeX fallback), or a `temml-error` span from
    // invalid TeX — any of these means the document needs math styles and
    // Temml's client-side initializer.
    if body_html.contains("<math")
        || body_html.contains("mud-math-block")
        || body_html.contains("temml-error")
    {
        stylesheets.push("mud-math.css");
        scripts.push("temml.min.js");
        scripts.push("math-init.js");
    }

    if body_html.contains("language-mermaid") {
        scripts.push("mermaid.min.js");
        scripts.push("mermaid-init.js");
    }

    // "Any code fence" per the plan's literal wording, not "any code fence
    // that isn't Mermaid" — a Mermaid-only document also loads highlight.js
    // under this rule, which is harmless (client-side JS is free to no-op on
    // a language it doesn't touch after Mermaid replaces the block).
    if body_html.contains("<pre><code") {
        scripts.push("highlight.min.js");
        scripts.push("highlight-init.js");
    }

    if !options.standalone {
        stylesheets.push("mud-find.css");
    }

    stylesheets.push("mud-narrow.css");
    stylesheets.push("mud-print.css");

    AssetSelection {
        stylesheets,
        scripts,
    }
}

/// Rewrites relative image `src` attributes in rendered HTML so the
/// served-document path (Phase 5, step 5.2 of the implementation plan) can
/// resolve them through `mudl-server`'s `/local/<percent-encoded-path>`
/// route rather than as bare filesystem paths a browser could never load.
///
/// This is a lightweight text scan for `<img ...src="...">`-style tags, not
/// a full HTML parser — matching the "hand-rolled but correct" level this
/// codebase already uses for string-level transforms (see `html_escape`).
/// Only the `src` attribute's value is touched; every other attribute on the
/// tag, and everything outside `<img>` tags, passes through untouched.
///
/// External sources (per [`crate::images::is_external_source`]) are left
/// alone. Everything else is resolved against `base_dir` the same way
/// [`crate::images::classify`] does (`base_dir.join(src)`, so an already-
/// absolute `src` simply replaces `base_dir` per `Path::join`'s standard
/// behavior), then percent-encoded and rewritten to `/local/<encoded>`.
pub fn rewrite_local_image_srcs(html: &str, base_dir: &Path) -> String {
    rewrite_img_srcs(html, &|src| rewrite_src(src, base_dir))
}

/// The shared `<img src="...">` text scanner behind [`rewrite_local_image_srcs`]
/// and (Phase 8.2) `mudl_core::images::rewrite_srcs_to_data_uris` — the two
/// differ only in how a single already-isolated `src` value is rewritten, so
/// that policy is factored out to `rewrite_one` rather than duplicating the
/// tag-scanning loop.
pub(crate) fn rewrite_img_srcs(html: &str, rewrite_one: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(tag_start) = rest.find("<img") {
        out.push_str(&rest[..tag_start]);
        let from_tag = &rest[tag_start..];

        match from_tag.find('>') {
            Some(close_rel) => {
                let tag_end = close_rel + 1;
                out.push_str(&rewrite_img_tag(&from_tag[..tag_end], rewrite_one));
                rest = &from_tag[tag_end..];
            }
            None => {
                // No closing `>` — malformed/truncated input. Nothing left
                // to safely rewrite; pass the remainder through untouched.
                out.push_str(from_tag);
                rest = "";
            }
        }
    }

    out.push_str(rest);
    out
}

/// Rewrites the `src="..."` attribute value (if any) within a single
/// already-isolated `<img ...>` tag; everything else in `tag` is copied
/// verbatim.
fn rewrite_img_tag(tag: &str, rewrite_one: &dyn Fn(&str) -> String) -> String {
    const NEEDLE: &str = "src=\"";

    let Some(attr_start) = tag.find(NEEDLE) else {
        return tag.to_string();
    };
    let value_start = attr_start + NEEDLE.len();
    let Some(value_len) = tag[value_start..].find('"') else {
        return tag.to_string();
    };

    let src = &tag[value_start..value_start + value_len];
    let rewritten = rewrite_one(src);

    format!(
        "{}{}{}",
        &tag[..value_start],
        rewritten,
        &tag[value_start + value_len..]
    )
}

/// Rewrites a single `src` value: external sources pass through unchanged;
/// everything else becomes `/local/<percent-encoded-absolute-path>`.
fn rewrite_src(src: &str, base_dir: &Path) -> String {
    if is_external_source(src) {
        return src.to_string();
    }

    let resolved = base_dir.join(src);
    format!("/local/{}", percent_encode(&resolved.to_string_lossy()))
}

/// Rewrites local-file `<a href="...">` targets in rendered HTML so
/// `mudl-gui`'s WebView navigation handler can tell, from the URL alone,
/// that a click should open a new mudl window (`.md`/`.markdown` files) or
/// hand off to `xdg-open` (every other local file) instead of navigating
/// the WebView itself.
///
/// Left untouched: in-page anchors (a bare `#section` href), and anything
/// [`crate::images::is_external_source`] already treats as external
/// (`http(s)://`, `mailto:`, `data:`) — those are handled directly by the
/// WebView/OS, not routed through `mudl-server`. Everything else is
/// resolved against `base_dir` the same way [`rewrite_local_image_srcs`]
/// resolves an image `src`, then percent-encoded and rewritten to
/// `/local-md/<encoded>` or `/local-file/<encoded>` depending on its
/// extension.
pub fn rewrite_local_link_hrefs(html: &str, base_dir: &Path) -> String {
    rewrite_a_hrefs(html, &|href| rewrite_href(href, base_dir))
}

/// The shared `<a href="...">` text scanner behind [`rewrite_local_link_hrefs`],
/// mirroring [`rewrite_img_srcs`] but keyed on the `<a>` tag and its `href`
/// attribute instead of `<img>`/`src`.
pub(crate) fn rewrite_a_hrefs(html: &str, rewrite_one: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(tag_start) = find_anchor_tag_start(rest) {
        out.push_str(&rest[..tag_start]);
        let from_tag = &rest[tag_start..];

        match from_tag.find('>') {
            Some(close_rel) => {
                let tag_end = close_rel + 1;
                out.push_str(&rewrite_a_tag(&from_tag[..tag_end], rewrite_one));
                rest = &from_tag[tag_end..];
            }
            None => {
                out.push_str(from_tag);
                rest = "";
            }
        }
    }

    out.push_str(rest);
    out
}

/// Finds the byte offset of the next `<a>` tag's start in `s`, requiring
/// the `a` be followed by whitespace, `>`, or `/` so `<article>`,
/// `<aside>`, and `<audio>` tags aren't mistaken for anchors.
fn find_anchor_tag_start(s: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(rel) = s[search_from..].find("<a") {
        let idx = search_from + rel;
        let after = idx + 2;
        match s[after..].chars().next() {
            Some(c) if c.is_whitespace() || c == '>' || c == '/' => return Some(idx),
            _ => search_from = after,
        }
    }
    None
}

/// Rewrites the `href="..."` attribute value (if any) within a single
/// already-isolated `<a ...>` tag; everything else in `tag` is copied
/// verbatim.
fn rewrite_a_tag(tag: &str, rewrite_one: &dyn Fn(&str) -> String) -> String {
    const NEEDLE: &str = "href=\"";

    let Some(attr_start) = tag.find(NEEDLE) else {
        return tag.to_string();
    };
    let value_start = attr_start + NEEDLE.len();
    let Some(value_len) = tag[value_start..].find('"') else {
        return tag.to_string();
    };

    let href = &tag[value_start..value_start + value_len];
    let rewritten = rewrite_one(href);

    format!(
        "{}{}{}",
        &tag[..value_start],
        rewritten,
        &tag[value_start + value_len..]
    )
}

/// Rewrites a single `href` value: an in-page anchor or external source
/// (per [`crate::images::is_external_source`]) passes through unchanged;
/// everything else becomes `/local-md/<percent-encoded-absolute-path>` (for
/// a `.md`/`.markdown` target) or `/local-file/<percent-encoded-absolute-path>`
/// (for anything else).
fn rewrite_href(href: &str, base_dir: &Path) -> String {
    if href.starts_with('#') || is_external_source(href) {
        return href.to_string();
    }

    let resolved = base_dir.join(href);
    let is_markdown = resolved
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
        .unwrap_or(false);
    let prefix = if is_markdown {
        "/local-md/"
    } else {
        "/local-file/"
    };

    format!("{prefix}{}", percent_encode(&resolved.to_string_lossy()))
}

/// A minimal RFC 3986 percent-encoder: bytes in the "unreserved" set
/// (`A-Za-z0-9-._~`) plus `/` (kept literal so the encoded path stays
/// legible and so `mudl-server`'s `routes::dispatch` — which only strips
/// the `/local/` prefix before percent-decoding the remainder — sees a
/// normal-looking path) pass through unchanged; every other byte is
/// escaped as `%XX` (uppercase hex).
pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod build_csp_tests {
    use super::build_csp;

    #[test]
    fn empty_list() {
        assert_eq!(
            build_csp(&[]),
            "<meta http-equiv=\"Content-Security-Policy\" content=\"\">"
        );
    }

    #[test]
    fn one_entry() {
        assert_eq!(
            build_csp(&["default-src 'none'"]),
            "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'\">"
        );
    }

    #[test]
    fn several_entries() {
        assert_eq!(
            build_csp(&["default-src 'none'", "img-src data:", "style-src 'unsafe-inline'"]),
            "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'\">"
        );
    }
}

#[cfg(test)]
mod build_html_attributes_tests {
    use super::build_html_attributes;

    #[test]
    fn zero_classes_default_zoom() {
        assert_eq!(build_html_attributes(&[], 1.0), "");
    }

    #[test]
    fn some_classes_default_zoom() {
        assert_eq!(build_html_attributes(&["a", "b"], 1.0), " class=\"a b\"");
    }

    #[test]
    fn zoom_exactly_one_emits_no_style_attribute() {
        assert_eq!(build_html_attributes(&[], 1.0), "");
    }

    #[test]
    fn non_default_zoom_emits_style_attribute() {
        assert_eq!(build_html_attributes(&[], 1.5), " style=\"zoom: 1.5\"");
    }

    #[test]
    fn classes_and_non_default_zoom_together() {
        assert_eq!(
            build_html_attributes(&["a", "b"], 1.5),
            " class=\"a b\" style=\"zoom: 1.5\""
        );
    }
}

#[cfg(test)]
mod build_scripts_tests {
    use super::{build_scripts, Script};

    #[test]
    fn empty_list() {
        assert_eq!(build_scripts(&[]), "");
    }

    #[test]
    fn one_inline_script() {
        assert_eq!(
            build_scripts(&[Script::Inline("console.log(1)".to_string())]),
            "<script>console.log(1)</script>"
        );
    }

    #[test]
    fn one_src_script() {
        assert_eq!(
            build_scripts(&[Script::Src("/assets/mud.js".to_string())]),
            "<script src=\"/assets/mud.js\"></script>"
        );
    }

    #[test]
    fn several_mixed_scripts() {
        assert_eq!(
            build_scripts(&[
                Script::Src("/assets/mud.js".to_string()),
                Script::Inline("init()".to_string()),
                Script::Src("/assets/mermaid.min.js".to_string()),
            ]),
            "<script src=\"/assets/mud.js\"></script>\n\
             <script>init()</script>\n\
             <script src=\"/assets/mermaid.min.js\"></script>"
        );
    }
}

#[cfg(test)]
mod render_tests {
    use super::{HtmlDocument, Script};

    #[test]
    fn doctype_present() {
        let doc = HtmlDocument::default();
        assert!(doc.render().contains("<!DOCTYPE html>"));
    }

    #[test]
    fn title_is_escaped() {
        let doc = HtmlDocument {
            title: "A & B <C>".to_string(),
            ..Default::default()
        };
        assert!(doc.render().contains("<title>A &amp; B &lt;C&gt;</title>"));
    }

    #[test]
    fn base_tag_present_when_set() {
        let doc = HtmlDocument {
            base_href: Some("/docs/".to_string()),
            ..Default::default()
        };
        assert!(doc.render().contains("<base href=\"/docs/\">"));
    }

    #[test]
    fn base_tag_absent_when_unset() {
        let doc = HtmlDocument::default();
        assert!(!doc.render().contains("<base"));
    }

    #[test]
    fn csp_meta_present() {
        let doc = HtmlDocument::default();
        assert!(doc.render().contains("Content-Security-Policy"));
    }

    #[test]
    fn body_content_and_scripts_present() {
        let doc = HtmlDocument {
            body_content: "<p>hi</p>".to_string(),
            body_scripts: vec![Script::Inline("go()".to_string())],
            ..Default::default()
        };
        let rendered = doc.render();
        assert!(rendered.contains("<p>hi</p>"));
        assert!(rendered.contains("<script>go()</script>"));
    }

    #[test]
    fn html_classes_and_zoom_baked_in() {
        let doc = HtmlDocument {
            html_classes: vec!["is-readable-column".to_string()],
            zoom_level: 1.5,
            ..Default::default()
        };
        assert!(doc
            .render()
            .contains("<html class=\"is-readable-column\" style=\"zoom: 1.5\">"));
    }
}

#[cfg(test)]
mod js_string_literal_tests {
    use super::js_string_literal;

    #[test]
    fn empty_string() {
        assert_eq!(js_string_literal(""), "\"\"");
    }

    #[test]
    fn contains_double_quotes() {
        assert_eq!(js_string_literal("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn contains_backslash() {
        assert_eq!(js_string_literal("a\\b"), "\"a\\\\b\"");
    }

    #[test]
    fn contains_unicode() {
        assert_eq!(js_string_literal("Ñoño 🎉"), "\"Ñoño 🎉\"");
    }

    #[test]
    fn contains_control_characters() {
        assert_eq!(js_string_literal("a\nb\tc"), "\"a\\nb\\tc\"");
    }
}

#[cfg(test)]
mod select_assets_tests {
    use super::{select_assets, AssetSelection};
    use crate::options::RenderOptions;

    fn options(standalone: bool) -> RenderOptions {
        RenderOptions {
            standalone,
            ..Default::default()
        }
    }

    #[test]
    fn no_markers_non_standalone() {
        let selection = select_assets("<p>hi</p>", &options(false));
        assert_eq!(
            selection,
            AssetSelection {
                stylesheets: vec!["mud-find.css", "mud-narrow.css", "mud-print.css"],
                scripts: vec!["mud.js"],
            }
        );
    }

    #[test]
    fn no_markers_standalone_omits_find_css() {
        let selection = select_assets("<p>hi</p>", &options(true));
        assert_eq!(
            selection,
            AssetSelection {
                stylesheets: vec!["mud-narrow.css", "mud-print.css"],
                scripts: vec!["mud.js"],
            }
        );
    }

    #[test]
    fn up_mode_wrapper_triggers_mud_up_script() {
        let selection = select_assets(r#"<div class="up-mode-output"></div>"#, &options(true));
        assert_eq!(selection.scripts, vec!["mud.js", "mud-up.js"]);
    }

    #[test]
    fn down_mode_wrapper_triggers_mud_down_script() {
        let selection = select_assets(r#"<div class="down-mode-output"></div>"#, &options(true));
        assert_eq!(selection.scripts, vec!["mud.js", "mud-down.js"]);
    }

    #[test]
    fn no_mode_wrapper_omits_both_mode_scripts() {
        let selection = select_assets("<p>hi</p>", &options(true));
        assert!(!selection.scripts.contains(&"mud-up.js"));
        assert!(!selection.scripts.contains(&"mud-down.js"));
    }

    #[test]
    fn math_marker_also_selects_temml_and_math_init_scripts() {
        let selection = select_assets("<math></math>", &options(true));
        assert!(selection.scripts.contains(&"temml.min.js"));
        assert!(selection.scripts.contains(&"math-init.js"));
    }

    #[test]
    fn mermaid_marker_also_selects_mermaid_init_script() {
        let selection = select_assets(
            "<pre><code class=\"language-mermaid\">graph TD</code></pre>",
            &options(true),
        );
        assert!(selection.scripts.contains(&"mermaid-init.js"));
    }

    #[test]
    fn code_fence_also_selects_highlight_init_script() {
        let selection = select_assets(
            "<pre><code class=\"language-rust\">fn main() {}</code></pre>",
            &options(true),
        );
        assert!(selection.scripts.contains(&"highlight-init.js"));
    }

    #[test]
    fn math_element_triggers_math_css() {
        let selection = select_assets("<math></math>", &options(true));
        assert!(selection.stylesheets.contains(&"mud-math.css"));
    }

    #[test]
    fn math_block_class_triggers_math_css() {
        let selection = select_assets("<div class=\"mud-math-block\"></div>", &options(true));
        assert!(selection.stylesheets.contains(&"mud-math.css"));
    }

    #[test]
    fn temml_error_triggers_math_css() {
        let selection = select_assets("<span class=\"temml-error\"></span>", &options(true));
        assert!(selection.stylesheets.contains(&"mud-math.css"));
    }

    #[test]
    fn no_math_markers_omits_math_css() {
        let selection = select_assets("<p>no math here</p>", &options(true));
        assert!(!selection.stylesheets.contains(&"mud-math.css"));
    }

    #[test]
    fn mermaid_marker_triggers_mermaid_script() {
        let selection = select_assets(
            "<pre><code class=\"language-mermaid\">graph TD</code></pre>",
            &options(true),
        );
        assert!(selection.scripts.contains(&"mermaid.min.js"));
    }

    #[test]
    fn no_mermaid_marker_omits_mermaid_script() {
        let selection = select_assets("<p>no diagrams</p>", &options(true));
        assert!(!selection.scripts.contains(&"mermaid.min.js"));
    }

    #[test]
    fn code_fence_triggers_highlight_script() {
        let selection = select_assets(
            "<pre><code class=\"language-rust\">fn main() {}</code></pre>",
            &options(true),
        );
        assert!(selection.scripts.contains(&"highlight.min.js"));
    }

    #[test]
    fn no_code_fence_omits_highlight_script() {
        let selection = select_assets("<p>no code here</p>", &options(true));
        assert!(!selection.scripts.contains(&"highlight.min.js"));
    }

    #[test]
    fn standalone_suppresses_find_css() {
        let selection = select_assets("<p>hi</p>", &options(true));
        assert!(!selection.stylesheets.contains(&"mud-find.css"));
    }

    #[test]
    fn non_standalone_includes_find_css() {
        let selection = select_assets("<p>hi</p>", &options(false));
        assert!(selection.stylesheets.contains(&"mud-find.css"));
    }

    #[test]
    fn deterministic_order_with_everything_present() {
        let body = "<div class=\"up-mode-output\">\
                    <math></math>\
                    <pre><code class=\"language-mermaid\">graph TD</code></pre>\
                    <pre><code class=\"language-rust\">fn main() {}</code></pre>\
                    </div>";
        let selection = select_assets(body, &options(false));
        assert_eq!(
            selection,
            AssetSelection {
                stylesheets: vec![
                    "mud-math.css",
                    "mud-find.css",
                    "mud-narrow.css",
                    "mud-print.css",
                ],
                scripts: vec![
                    "mud.js",
                    "mud-up.js",
                    "temml.min.js",
                    "math-init.js",
                    "mermaid.min.js",
                    "mermaid-init.js",
                    "highlight.min.js",
                    "highlight-init.js",
                ],
            }
        );
    }

    #[test]
    fn narrow_and_print_always_included_last_in_that_order() {
        let selection = select_assets("", &options(true));
        let last_two = &selection.stylesheets[selection.stylesheets.len() - 2..];
        assert_eq!(last_two, ["mud-narrow.css", "mud-print.css"]);
    }
}

#[cfg(test)]
mod rewrite_local_image_srcs_tests {
    use super::rewrite_local_image_srcs;
    use std::path::Path;

    #[test]
    fn relative_src_is_rewritten_to_local_route() {
        let base = Path::new("/base/dir");
        let html = r#"<img src="photo.png">"#;
        assert_eq!(
            rewrite_local_image_srcs(html, base),
            "<img src=\"/local//base/dir/photo.png\">"
        );
    }

    #[test]
    fn http_src_is_left_unchanged() {
        let base = Path::new("/base/dir");
        let html = r#"<img src="http://example.com/photo.png">"#;
        assert_eq!(rewrite_local_image_srcs(html, base), html);
    }

    #[test]
    fn https_src_is_left_unchanged() {
        let base = Path::new("/base/dir");
        let html = r#"<img src="https://example.com/photo.png">"#;
        assert_eq!(rewrite_local_image_srcs(html, base), html);
    }

    #[test]
    fn data_uri_src_is_left_unchanged() {
        let base = Path::new("/base/dir");
        let html = r#"<img src="data:image/png;base64,abc">"#;
        assert_eq!(rewrite_local_image_srcs(html, base), html);
    }

    #[test]
    fn mailto_src_is_left_unchanged() {
        let base = Path::new("/base/dir");
        let html = r#"<img src="mailto:test@example.com">"#;
        assert_eq!(rewrite_local_image_srcs(html, base), html);
    }

    #[test]
    fn multiple_img_tags_are_each_rewritten_independently() {
        let base = Path::new("/base/dir");
        let html = r#"<p><img src="a.png"> text <img src="b.png"></p>"#;
        assert_eq!(
            rewrite_local_image_srcs(html, base),
            "<p><img src=\"/local//base/dir/a.png\"> text <img src=\"/local//base/dir/b.png\"></p>"
        );
    }

    #[test]
    fn other_attributes_are_preserved_untouched() {
        let base = Path::new("/base/dir");
        let html = r#"<img alt="A photo" src="photo.png" width="100">"#;
        assert_eq!(
            rewrite_local_image_srcs(html, base),
            "<img alt=\"A photo\" src=\"/local//base/dir/photo.png\" width=\"100\">"
        );
    }

    #[test]
    fn src_with_space_is_percent_encoded() {
        let base = Path::new("/base/dir");
        let html = r#"<img src="my photo.png">"#;
        assert_eq!(
            rewrite_local_image_srcs(html, base),
            "<img src=\"/local//base/dir/my%20photo.png\">"
        );
    }

    #[test]
    fn html_with_no_img_tags_is_unchanged() {
        let base = Path::new("/base/dir");
        let html = "<p>No images here.</p>";
        assert_eq!(rewrite_local_image_srcs(html, base), html);
    }

    #[test]
    fn absolute_local_path_src_is_still_rewritten_through_local_route() {
        let base = Path::new("/base/dir");
        let html = r#"<img src="/absolute/photo.png">"#;
        assert_eq!(
            rewrite_local_image_srcs(html, base),
            "<img src=\"/local//absolute/photo.png\">"
        );
    }
}

#[cfg(test)]
mod rewrite_local_link_hrefs_tests {
    use super::rewrite_local_link_hrefs;
    use std::path::Path;

    #[test]
    fn relative_markdown_link_is_rewritten_to_local_md_route() {
        let base = Path::new("/base/dir");
        let html = r#"<a href="stub.md">stub</a>"#;
        assert_eq!(
            rewrite_local_link_hrefs(html, base),
            "<a href=\"/local-md//base/dir/stub.md\">stub</a>"
        );
    }

    #[test]
    fn relative_markdown_extension_link_is_rewritten_to_local_md_route() {
        let base = Path::new("/base/dir");
        let html = r#"<a href="notes.markdown">notes</a>"#;
        assert_eq!(
            rewrite_local_link_hrefs(html, base),
            "<a href=\"/local-md//base/dir/notes.markdown\">notes</a>"
        );
    }

    #[test]
    fn parent_directory_markdown_link_is_rewritten() {
        let base = Path::new("/base/dir");
        let html = r#"<a href="../Plans/plan.md">plan</a>"#;
        assert_eq!(
            rewrite_local_link_hrefs(html, base),
            "<a href=\"/local-md//base/dir/../Plans/plan.md\">plan</a>"
        );
    }

    #[test]
    fn other_local_file_link_is_rewritten_to_local_file_route() {
        let base = Path::new("/base/dir");
        let html = r#"<a href="example.txt">text</a>"#;
        assert_eq!(
            rewrite_local_link_hrefs(html, base),
            "<a href=\"/local-file//base/dir/example.txt\">text</a>"
        );
    }

    #[test]
    fn anchor_link_is_left_unchanged() {
        let base = Path::new("/base/dir");
        let html = "<a href=\"#section\">jump</a>";
        assert_eq!(rewrite_local_link_hrefs(html, base), html);
    }

    #[test]
    fn https_link_is_left_unchanged() {
        let base = Path::new("/base/dir");
        let html = r#"<a href="https://example.com">ex</a>"#;
        assert_eq!(rewrite_local_link_hrefs(html, base), html);
    }

    #[test]
    fn http_link_is_left_unchanged() {
        let base = Path::new("/base/dir");
        let html = r#"<a href="http://example.com">ex</a>"#;
        assert_eq!(rewrite_local_link_hrefs(html, base), html);
    }

    #[test]
    fn mailto_link_is_left_unchanged() {
        let base = Path::new("/base/dir");
        let html = r#"<a href="mailto:test@example.com">mail</a>"#;
        assert_eq!(rewrite_local_link_hrefs(html, base), html);
    }

    #[test]
    fn similarly_named_tags_are_not_mistaken_for_anchors() {
        let base = Path::new("/base/dir");
        let html = r#"<article><aside>x</aside></article>"#;
        assert_eq!(rewrite_local_link_hrefs(html, base), html);
    }

    #[test]
    fn multiple_links_are_each_rewritten_independently() {
        let base = Path::new("/base/dir");
        let html = r#"<a href="a.md">a</a> and <a href="b.txt">b</a>"#;
        assert_eq!(
            rewrite_local_link_hrefs(html, base),
            "<a href=\"/local-md//base/dir/a.md\">a</a> and \
             <a href=\"/local-file//base/dir/b.txt\">b</a>"
        );
    }

    #[test]
    fn other_attributes_are_preserved_untouched() {
        let base = Path::new("/base/dir");
        let html = r#"<a class="link" href="stub.md" title="Stub">stub</a>"#;
        assert_eq!(
            rewrite_local_link_hrefs(html, base),
            "<a class=\"link\" href=\"/local-md//base/dir/stub.md\" title=\"Stub\">stub</a>"
        );
    }

    #[test]
    fn html_with_no_a_tags_is_unchanged() {
        let base = Path::new("/base/dir");
        let html = "<p>No links here.</p>";
        assert_eq!(rewrite_local_link_hrefs(html, base), html);
    }

    #[test]
    fn extension_case_is_ignored() {
        let base = Path::new("/base/dir");
        let html = r#"<a href="STUB.MD">stub</a>"#;
        assert_eq!(
            rewrite_local_link_hrefs(html, base),
            "<a href=\"/local-md//base/dir/STUB.MD\">stub</a>"
        );
    }
}

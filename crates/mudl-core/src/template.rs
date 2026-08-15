//! HTML document template assembly and asset selection.
//!
//! Mirrors `mud`'s `HTMLDocument`/`HTMLTemplate`
//! (`Core/Sources/Rendering/HTMLDocument.swift`) as an independent, pure Rust
//! builder rather than a line-for-line translation — see `HtmlDocument`'s doc
//! comment and `Script` below for where this deliberately diverges.

use crate::encoding::html_escape;
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
pub fn select_assets(body_html: &str, options: &RenderOptions) -> AssetSelection {
    let mut stylesheets = Vec::new();
    let mut scripts = Vec::new();

    // A `<math>` element, a `mud-math-block` div (present even when the
    // renderer emits escaped-TeX fallback), or a `temml-error` span from
    // invalid TeX — any of these means the document needs math styles.
    if body_html.contains("<math")
        || body_html.contains("mud-math-block")
        || body_html.contains("temml-error")
    {
        stylesheets.push("mud-math.css");
    }

    if body_html.contains("language-mermaid") {
        scripts.push("mermaid.min.js");
    }

    // "Any code fence" per the plan's literal wording, not "any code fence
    // that isn't Mermaid" — a Mermaid-only document also loads highlight.js
    // under this rule, which is harmless (client-side JS is free to no-op on
    // a language it doesn't touch after Mermaid replaces the block).
    if body_html.contains("<pre><code") {
        scripts.push("highlight.min.js");
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
                scripts: vec![],
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
                scripts: vec![],
            }
        );
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
        let body = "<math></math>\
                    <pre><code class=\"language-mermaid\">graph TD</code></pre>\
                    <pre><code class=\"language-rust\">fn main() {}</code></pre>";
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
                scripts: vec!["mermaid.min.js", "highlight.min.js"],
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

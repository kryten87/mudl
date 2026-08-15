use crate::encoding::html_escape;
use crate::frontmatter::{FrontMatterValue, KeyValue};

/// Renders parsed frontmatter keys (from [`crate::frontmatter::parse_top_level_keys`])
/// as a collapsible `<details><table>` HTML fragment for Up-mode rendering.
///
/// Shape: `<details class="frontmatter"><summary>Frontmatter</summary>
/// <table><tr><th>{key}</th><td>{value}</td></tr>...</table></details>`,
/// one row per key, in input order. An empty key list produces an empty
/// string — no `<details>` wrapper at all, since there is nothing to show.
///
/// Value rendering per variant:
/// - `Scalar` renders as escaped text.
/// - `InlineArray` renders as its items joined with `", "`, escaped as a
///   whole (so a comma inside an element and the joining comma are
///   indistinguishable in the output — acceptable for a display fallback).
/// - `Block` is where the fallback rule lives. `parse_top_level_keys`
///   collapses both a "plain multi-line block" (e.g. a literal scalar) and
///   a "nested mapping" (e.g. `config:\n  nested:\n    key: value`) into the
///   same `Block(String)` shape — the type gives no way to tell those apart.
///   The one signal that *is* available here is line count, so: a `Block`
///   whose content is a single line (or empty) renders inline as escaped
///   text, same as a scalar; a `Block` spanning more than one line is
///   assumed too structurally complex to flatten sensibly into a table cell
///   and falls back to a `<pre><code>` block (still escaped, still inside
///   the cell — the fallback is per-cell, not a whole-table replacement,
///   since this function only sees already-parsed keys, not the raw YAML
///   source needed for a document-level fallback).
///
/// All key and value text is passed through [`html_escape`] exactly once.
pub fn render_table(keys: &[KeyValue]) -> String {
    if keys.is_empty() {
        return String::new();
    }

    let mut html =
        String::from("<details class=\"frontmatter\"><summary>Frontmatter</summary><table>");
    for kv in keys {
        html.push_str("<tr><th>");
        html.push_str(&html_escape(&kv.key));
        html.push_str("</th><td>");
        html.push_str(&render_value_cell(&kv.value));
        html.push_str("</td></tr>");
    }
    html.push_str("</table></details>");
    html
}

fn render_value_cell(value: &FrontMatterValue) -> String {
    match value {
        FrontMatterValue::Scalar(v) => html_escape(v),
        FrontMatterValue::InlineArray(items) => html_escape(&items.join(", ")),
        FrontMatterValue::Block(raw) => {
            if raw.lines().count() <= 1 {
                html_escape(raw)
            } else {
                format!("<pre><code>{}</code></pre>", html_escape(raw))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_key_list_produces_empty_output() {
        assert_eq!(render_table(&[]), "");
    }

    #[test]
    fn simple_scalar_keys_one_row_each() {
        let keys = vec![
            KeyValue {
                key: "title".into(),
                value: FrontMatterValue::Scalar("Hello".into()),
            },
            KeyValue {
                key: "author".into(),
                value: FrontMatterValue::Scalar("Jane Doe".into()),
            },
        ];
        assert_eq!(
            render_table(&keys),
            "<details class=\"frontmatter\"><summary>Frontmatter</summary><table>\
<tr><th>title</th><td>Hello</td></tr>\
<tr><th>author</th><td>Jane Doe</td></tr>\
</table></details>"
        );
    }

    #[test]
    fn inline_array_value_is_comma_joined() {
        let keys = vec![KeyValue {
            key: "tags".into(),
            value: FrontMatterValue::InlineArray(vec![
                "swift".into(),
                "markdown".into(),
                "preview".into(),
            ]),
        }];
        assert_eq!(
            render_table(&keys),
            "<details class=\"frontmatter\"><summary>Frontmatter</summary><table>\
<tr><th>tags</th><td>swift, markdown, preview</td></tr>\
</table></details>"
        );
    }

    #[test]
    fn single_line_block_value_stays_tabular() {
        // A `Block` whose joined content happens to be a single line (e.g. a
        // one-item indented continuation) is simple enough to render as flat
        // escaped text, same as a scalar — no `<pre><code>` fallback.
        let keys = vec![KeyValue {
            key: "tags".into(),
            value: FrontMatterValue::Block("  - swift".into()),
        }];
        assert_eq!(
            render_table(&keys),
            "<details class=\"frontmatter\"><summary>Frontmatter</summary><table>\
<tr><th>tags</th><td>  - swift</td></tr>\
</table></details>"
        );
    }

    #[test]
    fn multi_line_block_value_falls_back_to_pre_code() {
        let keys = vec![KeyValue {
            key: "config".into(),
            value: FrontMatterValue::Block("  nested:\n    key: value\n    other: thing".into()),
        }];
        assert_eq!(
            render_table(&keys),
            "<details class=\"frontmatter\"><summary>Frontmatter</summary><table>\
<tr><th>config</th><td><pre><code>  nested:\n    key: value\n    other: thing\
</code></pre></td></tr></table></details>"
        );
    }

    #[test]
    fn empty_block_value_stays_tabular() {
        let keys = vec![KeyValue {
            key: "notes".into(),
            value: FrontMatterValue::Block(String::new()),
        }];
        assert_eq!(
            render_table(&keys),
            "<details class=\"frontmatter\"><summary>Frontmatter</summary><table>\
<tr><th>notes</th><td></td></tr></table></details>"
        );
    }

    #[test]
    fn html_special_characters_escaped_exactly_once() {
        let keys = vec![
            KeyValue {
                key: "a<b>&\"c".into(),
                value: FrontMatterValue::Scalar("<script>&\"boo\"</script>".into()),
            },
            KeyValue {
                key: "list".into(),
                value: FrontMatterValue::InlineArray(vec!["<x>".into(), "y&z".into()]),
            },
            KeyValue {
                key: "block".into(),
                value: FrontMatterValue::Block("<tag>\n&\"amp\"".into()),
            },
        ];
        let rendered = render_table(&keys);
        assert!(rendered.contains("<th>a&lt;b&gt;&amp;&quot;c</th>"));
        assert!(rendered.contains("<td>&lt;script&gt;&amp;&quot;boo&quot;&lt;/script&gt;</td>"));
        assert!(rendered.contains("<td>&lt;x&gt;, y&amp;z</td>"));
        assert!(rendered.contains("<pre><code>&lt;tag&gt;\n&amp;&quot;amp&quot;</code></pre>"));
        // Guard against double-escaping: no literal "&amp;amp;" or "&amp;lt;" etc.
        assert!(!rendered.contains("&amp;amp;"));
        assert!(!rendered.contains("&amp;lt;"));
        assert!(!rendered.contains("&amp;gt;"));
        assert!(!rendered.contains("&amp;quot;"));
    }
}

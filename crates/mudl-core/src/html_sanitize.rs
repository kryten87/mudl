//! Sanitizes raw HTML passed through verbatim from Markdown source, and
//! validates link/image URL schemes.
//!
//! `docs/SECURITY.md` Finding 3: `pulldown-cmark` hands raw HTML (both
//! `Event::Html` block content and `Event::InlineHtml` fragments) through
//! as opaque text, so `render.rs`'s two pass-through sites are the only
//! place this filtering can happen. This module has no dependency on the
//! parser or the renderer — it operates purely on the raw markup text (or,
//! for [`is_safe_url`], a bare URL string) so both call sites can share it.
//!
//! The sanitizer is a hand-rolled tag scanner rather than a full HTML
//! parser: it recognizes tag/comment/declaration boundaries (respecting
//! quoted attribute values so a literal `>` inside one doesn't end the tag
//! early) and rewrites or drops what it finds. It does not build a DOM or
//! track cross-event nesting — each call sanitizes the text it's given,
//! which is exactly right for `render_html_block` (an entire buffered
//! block) and good enough for `Event::InlineHtml` (one tag at a time: an
//! unmatched blocked open tag has nothing to skip over, so it's simply
//! dropped on its own, and its later, separately-dropped closing tag
//! leaves only inert escaped text between them).

/// Tag names whose *entire element* — opening tag, content, and matching
/// closing tag — is dropped, per `docs/SECURITY.md` Finding 3's minimum
/// list. Content between an unmatched open tag (as `Event::InlineHtml`
/// hands it over one tag at a time) and its separately-processed closing
/// tag is left as inert text, not re-included here.
const BLOCKED_TAGS: &[&str] = &["script", "iframe", "object", "embed"];

/// Sanitizes a fragment of raw HTML: drops `BLOCKED_TAGS` elements
/// entirely, strips `on*` event-handler attributes from everything else,
/// and drops `href`/`src` attributes whose value fails [`is_safe_url`].
/// Comments, declarations, and processing instructions pass through
/// unchanged — none of them execute script.
pub fn sanitize_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input.as_bytes()[i] == b'<' {
            if let Some(end) = find_tag_end(input, i) {
                let tag = &input[i..end];
                if let Some(name) = blocked_start_tag_name(tag) {
                    i = find_matching_close(input, end, &name).unwrap_or(input.len());
                    continue;
                }
                if is_blocked_close_tag(tag) {
                    i = end;
                    continue;
                }
                if tag_name(tag).is_some() {
                    out.push_str(&sanitize_attributes(tag));
                } else {
                    out.push_str(tag);
                }
                i = end;
                continue;
            }
        }
        let ch = input[i..].chars().next().expect("i < input.len()");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// True if `url` may be used as an `href`/`src` value: no scheme at all
/// (a relative path or a `#fragment`), or one of `http`, `https`, `mailto`
/// — `docs/SECURITY.md` Finding 3's allowlist. Everything else, notably
/// `javascript:` and `data:`, is rejected. ASCII control characters (which
/// browsers strip before parsing a URL's scheme, so `java\tscript:` still
/// runs as `javascript:`) are ignored when looking for the scheme.
pub fn is_safe_url(url: &str) -> bool {
    let cleaned: String = url.chars().filter(|c| !c.is_ascii_control()).collect();
    let scheme_end = match cleaned.find([':', '/', '?', '#']) {
        Some(idx) if cleaned.as_bytes()[idx] == b':' => idx,
        _ => return true,
    };
    let scheme = &cleaned[..scheme_end];
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    {
        return true;
    }
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto"
    )
}

/// Finds the end (exclusive, just past the closing delimiter) of the tag,
/// comment, declaration, processing instruction, or CDATA section starting
/// at `start` (where `input.as_bytes()[start] == b'<'`). Returns `None` for
/// an unterminated construct — the caller then treats the rest of the
/// input as plain text.
fn find_tag_end(input: &str, start: usize) -> Option<usize> {
    let rest = &input[start..];
    if let Some(needle_end) = matched_end(rest, "<!--", "-->") {
        return Some(start + needle_end);
    }
    if let Some(needle_end) = matched_end(rest, "<![CDATA[", "]]>") {
        return Some(start + needle_end);
    }
    if let Some(needle_end) = matched_end(rest, "<?", "?>") {
        return Some(start + needle_end);
    }
    // A bare declaration, e.g. `<!DOCTYPE html>` — no internal `>` to worry
    // about in practice, but scan quote-aware anyway for uniformity.
    let bytes = input.as_bytes();
    let mut i = start + 1;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        match quote {
            Some(q) => {
                if bytes[i] == q {
                    quote = None;
                }
            }
            None => match bytes[i] {
                b'"' | b'\'' => quote = Some(bytes[i]),
                b'>' => return Some(i + 1),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

fn matched_end(rest: &str, open: &str, close: &str) -> Option<usize> {
    if !rest.starts_with(open) {
        return None;
    }
    rest[open.len()..]
        .find(close)
        .map(|rel| open.len() + rel + close.len())
}

/// Parses `tag` (a full `<...>` construct) as an element start/end tag,
/// returning `(is_closing, lowercased_name, byte_index_in_tag_just_past_the_name)`.
/// `None` for anything that isn't an element tag (comment, declaration, PI).
fn tag_name(tag: &str) -> Option<(bool, String, usize)> {
    let bytes = tag.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'<' {
        return None;
    }
    let mut i = 1;
    let is_close = bytes[i] == b'/';
    if is_close {
        i += 1;
    }
    let name_start = i;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    if i == name_start || !bytes[name_start].is_ascii_alphabetic() {
        return None;
    }
    Some((is_close, tag[name_start..i].to_ascii_lowercase(), i))
}

fn blocked_start_tag_name(tag: &str) -> Option<String> {
    tag_name(tag).and_then(|(is_close, name, _)| {
        (!is_close && BLOCKED_TAGS.contains(&name.as_str())).then_some(name)
    })
}

fn is_blocked_close_tag(tag: &str) -> bool {
    tag_name(tag)
        .is_some_and(|(is_close, name, _)| is_close && BLOCKED_TAGS.contains(&name.as_str()))
}

/// Case-insensitively finds `</name` (as a whole tag name, not merely a
/// prefix — `</scriptx>` doesn't count as closing `<script>`) at or after
/// `from`, and returns the index just past its closing `>`.
fn find_matching_close(input: &str, from: usize, name: &str) -> Option<usize> {
    let haystack = &input[from..];
    let lower = haystack.to_ascii_lowercase();
    let needle = format!("</{name}");
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find(&needle) {
        let after_needle = search_from + rel + needle.len();
        let tail = &haystack[after_needle..];
        let ws_len = tail
            .as_bytes()
            .iter()
            .take_while(|b| b.is_ascii_whitespace())
            .count();
        if tail.as_bytes().get(ws_len) == Some(&b'>') {
            return Some(from + after_needle + ws_len + 1);
        }
        search_from = search_from + rel + 1;
    }
    None
}

/// Rebuilds an element start tag with `on*` attributes dropped and any
/// `href`/`src` attribute removed if [`is_safe_url`] rejects its value.
/// Closing tags carry no attributes and are returned unchanged.
fn sanitize_attributes(tag: &str) -> String {
    let (is_close, name, name_end) = match tag_name(tag) {
        Some(v) => v,
        None => return tag.to_string(),
    };
    if is_close {
        return tag.to_string();
    }
    let inner = &tag[name_end..tag.len() - 1]; // strip leading "<name" and trailing '>'
    let trimmed = inner.trim_end();
    let self_closing = trimmed.ends_with('/');
    let attrs_str = if self_closing {
        &trimmed[..trimmed.len() - 1]
    } else {
        inner
    };

    let mut result = String::new();
    result.push('<');
    result.push_str(&name);
    for (attr_name, raw) in parse_attributes(attrs_str) {
        let lower_name = attr_name.to_ascii_lowercase();
        if lower_name.starts_with("on") {
            continue;
        }
        if (lower_name == "href" || lower_name == "src") && !is_safe_url(&attr_value(&raw)) {
            continue;
        }
        result.push(' ');
        result.push_str(&raw);
    }
    if self_closing {
        result.push_str(" /");
    }
    result.push('>');
    result
}

/// Splits an attribute list into `(name, raw_token)` pairs, where
/// `raw_token` is the attribute's original source text (`name`,
/// `name=value`, or `name="quoted value"`) — reused verbatim for anything
/// that survives filtering, so quoting style is preserved unchanged.
fn parse_attributes(s: &str) -> Vec<(String, String)> {
    let bytes = s.as_bytes();
    let mut attrs = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let name_end = i;
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'=' {
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                let quote = bytes[j];
                j += 1;
                while j < bytes.len() && bytes[j] != quote {
                    j += 1;
                }
                if j < bytes.len() {
                    j += 1; // include closing quote
                }
            } else {
                while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
            }
            attrs.push((s[start..name_end].to_string(), s[start..j].to_string()));
            i = j;
        } else {
            attrs.push((
                s[start..name_end].to_string(),
                s[start..name_end].to_string(),
            ));
            i = name_end;
        }
    }
    attrs
}

/// Extracts the value portion of a `name=value` or `name="value"` raw
/// attribute token (from [`parse_attributes`]), with surrounding quotes
/// stripped. A bare boolean attribute (no `=`) has no value to check.
fn attr_value(raw: &str) -> String {
    match raw.find('=') {
        None => String::new(),
        Some(eq) => {
            let v = &raw[eq + 1..];
            let bytes = v.as_bytes();
            if bytes.len() >= 2
                && (bytes[0] == b'"' || bytes[0] == b'\'')
                && bytes[bytes.len() - 1] == bytes[0]
            {
                v[1..v.len() - 1].to_string()
            } else {
                v.to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod sanitize_html_tests {
        use super::sanitize_html;

        #[test]
        fn script_block_is_dropped_entirely() {
            assert_eq!(sanitize_html("<script>alert(1)</script>"), "");
        }

        #[test]
        fn script_tag_is_case_insensitive() {
            assert_eq!(sanitize_html("<SCRIPT>alert(1)</SCRIPT>"), "");
        }

        #[test]
        fn unclosed_script_drops_rest_of_fragment() {
            assert_eq!(sanitize_html("before<script>alert(1)"), "before");
        }

        #[test]
        fn iframe_object_embed_are_dropped() {
            assert_eq!(sanitize_html("<iframe src=\"x\"></iframe>"), "");
            assert_eq!(sanitize_html("<object data=\"x\"></object>"), "");
            assert_eq!(sanitize_html("<embed src=\"x\">"), "");
        }

        #[test]
        fn self_closing_blocked_tag_is_dropped() {
            assert_eq!(sanitize_html("<script/>"), "");
        }

        #[test]
        fn lone_open_tag_with_no_close_in_fragment_is_dropped() {
            // Mirrors how `Event::InlineHtml` hands over one tag at a time.
            assert_eq!(sanitize_html("<script>"), "");
            assert_eq!(sanitize_html("</script>"), "");
        }

        #[test]
        fn on_attribute_is_stripped_from_img() {
            assert_eq!(
                sanitize_html("<img src=x onerror=\"fetch(1)\">"),
                "<img src=x>"
            );
        }

        #[test]
        fn on_attribute_stripped_case_insensitively() {
            assert_eq!(
                sanitize_html("<div OnClick='evil()'>hi</div>"),
                "<div>hi</div>"
            );
        }

        #[test]
        fn javascript_href_is_dropped() {
            assert_eq!(
                sanitize_html("<a href=\"javascript:alert(1)\">x</a>"),
                "<a>x</a>"
            );
        }

        #[test]
        fn safe_href_is_kept() {
            assert_eq!(
                sanitize_html("<a href=\"https://example.com\">x</a>"),
                "<a href=\"https://example.com\">x</a>"
            );
        }

        #[test]
        fn relative_src_is_kept() {
            assert_eq!(sanitize_html("<img src=photo.png>"), "<img src=photo.png>");
        }

        #[test]
        fn benign_tag_and_attributes_pass_through_unchanged() {
            assert_eq!(
                sanitize_html("<div class=\"note\" id=\"x\">hi</div>"),
                "<div class=\"note\" id=\"x\">hi</div>"
            );
        }

        #[test]
        fn comment_passes_through_including_embedded_angle_bracket() {
            assert_eq!(sanitize_html("<!-- if x > y -->"), "<!-- if x > y -->");
        }

        #[test]
        fn doctype_passes_through() {
            assert_eq!(sanitize_html("<!DOCTYPE html>"), "<!DOCTYPE html>");
        }

        #[test]
        fn quoted_gt_inside_tag_does_not_end_it_early() {
            assert_eq!(
                sanitize_html("<div title=\"a > b\">x</div>"),
                "<div title=\"a > b\">x</div>"
            );
        }

        #[test]
        fn text_with_no_tags_is_unaffected() {
            assert_eq!(sanitize_html("plain text"), "plain text");
        }
    }

    mod is_safe_url_tests {
        use super::is_safe_url;

        #[test]
        fn http_and_https_are_safe() {
            assert!(is_safe_url("http://example.com"));
            assert!(is_safe_url("https://example.com"));
        }

        #[test]
        fn mailto_is_safe() {
            assert!(is_safe_url("mailto:jane@example.com"));
        }

        #[test]
        fn relative_and_fragment_urls_are_safe() {
            assert!(is_safe_url("./notes.md"));
            assert!(is_safe_url("#section"));
            assert!(is_safe_url(""));
        }

        #[test]
        fn javascript_scheme_is_unsafe() {
            assert!(!is_safe_url("javascript:alert(1)"));
        }

        #[test]
        fn data_scheme_is_unsafe() {
            assert!(!is_safe_url("data:text/html,<script>alert(1)</script>"));
        }

        #[test]
        fn control_character_obfuscated_scheme_is_still_caught() {
            assert!(!is_safe_url("java\tscript:alert(1)"));
        }

        #[test]
        fn scheme_check_is_case_insensitive() {
            assert!(!is_safe_url("JavaScript:alert(1)"));
        }
    }
}

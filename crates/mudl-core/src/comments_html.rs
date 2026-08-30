//! Bottom-of-document HTML for footnotes and comments (Phase 14.6). Ported
//! from `mud`'s `Core/Sources/Rendering/CommentHTMLRenderer.swift`, plus the
//! authorial-footnotes counterpart mud's `HTMLTemplate` builds from
//! `FootnoteProcessor.process`'s `footnotes` list.
//!
//! Kept separate from `render.rs`'s inline visitor (`Renderer`) the same
//! way `mud` keeps `CommentHTMLRenderer` out of `UpHTMLVisitor`: these
//! functions render a *list already collected* by `mudl_comments::document`
//! into markup, with no `pulldown-cmark` event-stream walking of their own
//! beyond the fresh `render_up` call each entry's body gets.

use std::collections::HashMap;

use mudl_comments::document::{parse_comments, parse_footnotes, FootnoteEntry};
use mudl_comments::serialization::{format_timestamp, iso_timestamp, Comment, CommentMessage};

use crate::encoding::html_escape;
use crate::options::RenderOptions;
use crate::render::render_up;

/// The bottom Footnotes section: `<section class="footnotes">` with one
/// numbered `<li>` per *referenced* authorial footnote (an unreferenced
/// definition is dropped, matching `mud`), in number order. Empty string
/// when there are none.
pub fn footnotes_section(
    body: &str,
    footnote_numbers: &HashMap<String, u32>,
    options: &RenderOptions,
) -> String {
    let mut numbered: Vec<(u32, FootnoteEntry)> = parse_footnotes(body)
        .into_iter()
        .filter_map(|entry| footnote_numbers.get(&entry.label).map(|&n| (n, entry)))
        .collect();
    numbered.sort_by_key(|(number, _)| *number);
    if numbered.is_empty() {
        return String::new();
    }

    let mut inner_options = options.clone();
    inner_options.waypoint = None;

    let mut html = String::from("<section class=\"footnotes\">\n<ol>\n");
    for (number, entry) in numbered {
        let label = html_escape(&entry.label);
        html.push_str(&format!(
            "<li id=\"fn-{label}\" data-mud-footnote-number=\"{number}\">\n"
        ));
        html.push_str(&render_up(&entry.body_markdown, &inner_options));
        html.push_str(&format!(
            "<a class=\"footnote-backref\" href=\"#fnref-{label}\" aria-label=\"Back to content\">\u{21A9}</a>\n"
        ));
        html.push_str("</li>\n");
    }
    html.push_str("</ol>\n</section>");
    html
}

/// The bottom Comments section: a `<footer class="comments">` with one
/// `<li>` per comment -- its quotation (if any) and its thread of
/// messages, carrying the `data-mud-*` fields a future GUI layer can read
/// without re-parsing the rendered HTML. Empty string when there are none.
pub fn comments_section(body: &str, options: &RenderOptions) -> String {
    let comments = parse_comments(body);
    if comments.is_empty() {
        return String::new();
    }

    let mut inner_options = options.clone();
    inner_options.waypoint = None;

    let mut html =
        String::from("<footer class=\"comments\" data-comments>\n<h2>Comments</h2>\n<ol>\n");
    for comment in &comments {
        html.push_str(&comment_list_item(comment, &inner_options));
    }
    html.push_str("</ol>\n</footer>");
    html
}

/// One comment's `<li>`: its thread plus a marker back-reference, carrying
/// the label on the item and the author/time on each message.
fn comment_list_item(comment: &Comment, options: &RenderOptions) -> String {
    let label = html_escape(&comment.label);
    let mut html = format!("<li id=\"cmt-{label}\" data-mud-label=\"{label}\"");
    if let Some(quotation) = non_empty(&comment.quotation) {
        html.push_str(&format!(
            " data-mud-quotation=\"{}\"",
            html_escape(quotation)
        ));
    }
    html.push_str(">\n");

    if let Some(quotation) = non_empty(&comment.quotation) {
        html.push_str("<blockquote class=\"mud-comment-quote\">");
        html.push_str(&html_escape(quotation));
        html.push_str("</blockquote>\n");
    }

    for message in &comment.messages {
        html.push_str(&message_div(message, options));
    }

    html.push_str(&format!(
        "<a class=\"footnote-backref\" href=\"#cmtref-{label}\" aria-label=\"Back to content\">\u{21A9}</a>\n"
    ));
    html.push_str("</li>\n");
    html
}

fn message_div(message: &CommentMessage, options: &RenderOptions) -> String {
    let mut html = String::from("<div class=\"mud-comment-message\"");
    if let Some(author) = non_empty(&message.author) {
        html.push_str(&format!(" data-mud-author=\"{}\"", html_escape(author)));
    }
    html.push_str(">\n");

    let attribution = format_attribution(message);
    if !attribution.is_empty() {
        html.push_str("<div class=\"mud-comment-attribution\">");
        html.push_str(&attribution);
        html.push_str("</div>\n");
    }

    html.push_str("<div class=\"mud-comment-body\">");
    html.push_str(&render_up(&message.body, options));
    html.push_str("</div>\n");
    html.push_str("</div>\n");
    html
}

/// The `author · timestamp` attribution line for a message, HTML-escaped;
/// empty when the message carries neither.
fn format_attribution(message: &CommentMessage) -> String {
    let mut parts = Vec::new();
    if let Some(author) = non_empty(&message.author) {
        parts.push(html_escape(author));
    }
    if let Some(created) = &message.created {
        let iso = iso_timestamp(created);
        let stamp = html_escape(&format_timestamp(created));
        parts.push(format!(
            "<time class=\"mud-comment-time\" datetime=\"{iso}\">{stamp}</time>"
        ));
    }
    parts.join(" ")
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(markdown: &str) -> String {
        crate::render::render_up(markdown, &RenderOptions::default())
    }

    #[test]
    fn no_footnotes_yields_empty_section() {
        assert_eq!(
            footnotes_section("Plain text.\n", &HashMap::new(), &RenderOptions::default()),
            ""
        );
    }

    #[test]
    fn no_comments_yields_empty_section() {
        assert_eq!(
            comments_section("Plain text.\n", &RenderOptions::default()),
            ""
        );
    }

    #[test]
    fn referenced_footnote_appears_numbered() {
        let markdown = "Text.[^1]\n\n[^1]: A note.\n";
        let html = render(markdown);
        assert!(html
            .contains("<sup class=\"footnote-ref\" id=\"fnref-1\"><a href=\"#fn-1\">1</a></sup>"));
        assert!(html.contains("<section class=\"footnotes\">"));
        assert!(html.contains("id=\"fn-1\" data-mud-footnote-number=\"1\""));
        assert!(html.contains("A note."));
        assert!(html.contains("href=\"#fnref-1\""));
        // The definition never renders inline where it's written.
        assert!(!html.contains("[^1]"));
    }

    #[test]
    fn unreferenced_footnote_definition_is_dropped_from_the_section() {
        let markdown = "No references here.\n\n[^1]: Orphaned.\n";
        let html = render(markdown);
        assert!(!html.contains("footnotes"));
        assert!(!html.contains("Orphaned."));
    }

    #[test]
    fn comment_marker_and_section_render() {
        let markdown = "Text.[^comment-a]\n\n[^comment-a]: > Text.\n\n    {JP}: Nice.\n";
        let html = render(markdown);
        assert!(html.contains("<a class=\"mud-comment-marker\" id=\"cmtref-comment-a\" href=\"#cmt-comment-a\">\u{1F4AC}</a>"));
        assert!(html.contains("<footer class=\"comments\" data-comments>"));
        assert!(html.contains("data-mud-quotation=\"Text.\""));
        assert!(html.contains("data-mud-author=\"JP\""));
        assert!(html.contains("<p>Nice.</p>"));
    }

    #[test]
    fn comment_message_body_renders_as_markdown() {
        let markdown = "Text.[^comment-a]\n\n[^comment-a]: **Bold** note.\n";
        let html = render(markdown);
        assert!(html.contains("<strong>Bold</strong> note."));
    }
}

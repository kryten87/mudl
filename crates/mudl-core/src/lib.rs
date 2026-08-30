pub mod alerts;
pub mod changes;
pub mod comments_html;
pub mod emoji;
pub mod encoding;
pub mod folder;
pub mod footnotes;
pub mod frontmatter;
pub mod frontmatter_html;
pub mod headings;
pub mod images;
pub mod options;
pub mod outline;
pub mod parse;
pub mod render;
pub mod resources;
pub mod slug;
pub mod template;

#[cfg(test)]
mod tests {
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn crate_compiles() {
        assert!(true);
    }
}

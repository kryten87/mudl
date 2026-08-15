pub mod alerts;
pub mod emoji;
pub mod encoding;
pub mod footnotes;
pub mod frontmatter;
pub mod images;
pub mod slug;

#[cfg(test)]
mod tests {
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn crate_compiles() {
        assert!(true);
    }
}

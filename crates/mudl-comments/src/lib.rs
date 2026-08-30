//! Comments (`mudl-comments`, Phase 14 of `docs/IMPLEMENTATION-PLAN.md`).
//!
//! Ported from `mud`'s `Core/Sources/Comments/*.swift`: a comment is a GFM
//! footnote whose label matches `^comment-[\w-]+$` (`mudl_core::footnotes::
//! is_comment_label`, Phase 1.8), storing a quoted-anchor + threaded
//! discussion as the footnote's body. Not a dependency of `mudl-core` in the
//! other direction: `mudl-core` depends on this crate (the same shape as
//! `mudl-diff`), so this crate does its own `pulldown-cmark` parsing rather
//! than importing `mudl-core`'s renderer.

pub mod anchor;
pub mod labels;
pub mod serialization;

#[cfg(test)]
mod tests {
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn crate_compiles() {
        assert!(true);
    }
}

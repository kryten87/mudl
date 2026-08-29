//! Change tracking (`mudl-diff`, Phase 13 of `docs/IMPLEMENTATION-PLAN.md`).
//!
//! Ported from `mud`'s `Core/Sources/Diff/*.swift`: word-level and
//! line-level diffing, greedy line pairing, leaf-block matching, and the
//! `ChangePlan` that assembles those pieces into the overlay `mudl-core`'s
//! renderer draws. Nothing here depends on `mudl-core`'s rendering code —
//! only on its `footnotes::is_comment_label` predicate, reused rather than
//! re-implemented.

mod lcs;
pub mod word;

#[cfg(test)]
mod tests {
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn crate_compiles() {
        assert!(true);
    }
}

//! Preferences (`mudl-config`, Phase 7 of `docs/IMPLEMENTATION-PLAN.md`):
//! parsing/validating the on-disk `key = value` preferences format and
//! loading/saving it via the injected `FileSystem` trait.

pub mod format;

#[cfg(test)]
mod tests {
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn crate_compiles() {
        assert!(true);
    }
}

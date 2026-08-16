//! Pure MIME-type lookup by file extension (Phase 4, step 4.3 of
//! `docs/IMPLEMENTATION-PLAN.md`).

/// Looks up the MIME type for a file extension (without the leading dot),
/// matching case-insensitively. Falls back to
/// `application/octet-stream` for anything unrecognized.
pub fn lookup(extension: &str) -> &'static str {
    todo!("Phase 4.3: {extension}")
}

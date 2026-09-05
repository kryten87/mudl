//! Open Recent (Phase 15.1 of `docs/IMPLEMENTATION-PLAN.md`): a
//! most-recently-opened file list, modeled on `crate::geometry`'s own
//! on-disk file + `FileSystem`-DI load/save, re-read-fresh-before-write.
//!
//! Kept out of `mudl_config::Preferences` deliberately: that schema is a
//! fixed set of known keys round-tripped as a whole, which doesn't fit an
//! open-ended ordered list of paths. Simpler than `crate::geometry`'s
//! `key = value` format too, since this is just one ordered list, not a
//! per-path map — a plain one-path-per-line file is enough.

use std::path::{Path, PathBuf};

use mudl_config::FileSystem;

/// Parses one path per line, skipping blank lines (a trailing newline, or a
/// blank line from hand-editing, shouldn't produce a phantom empty entry).
pub fn parse(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

pub fn serialize(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pure most-recently-opened update: `opened` moves to the front, any
/// existing occurrence of it elsewhere in `existing` is dropped (no
/// duplicate), and the result is capped at `max` entries, dropping the
/// oldest first.
pub fn record(existing: &[PathBuf], opened: &Path, max: usize) -> Vec<PathBuf> {
    let mut updated = Vec::with_capacity(existing.len().saturating_add(1).min(max.max(1)));
    updated.push(opened.to_path_buf());
    updated.extend(
        existing
            .iter()
            .filter(|path| path.as_path() != opened)
            .cloned(),
    );
    updated.truncate(max);
    updated
}

/// Looks up the saved recent-files list. A missing or unreadable file is
/// treated as "no recent files" rather than an error.
pub fn load(fs: &dyn FileSystem, path: &Path) -> Vec<PathBuf> {
    match fs.read(path) {
        Ok(bytes) => parse(&String::from_utf8_lossy(&bytes)),
        Err(_) => Vec::new(),
    }
}

pub fn save(fs: &dyn FileSystem, path: &Path, paths: &[PathBuf]) -> std::io::Result<()> {
    fs.write_atomic(path, serialize(paths).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mudl_config::InMemoryFileSystem;

    #[test]
    fn parse_empty_text_yields_empty_list() {
        assert_eq!(parse(""), Vec::<PathBuf>::new());
    }

    #[test]
    fn parse_skips_blank_lines() {
        assert_eq!(
            parse("/a.md\n\n/b.md\n"),
            vec![PathBuf::from("/a.md"), PathBuf::from("/b.md")]
        );
    }

    #[test]
    fn serialize_joins_with_newlines() {
        let paths = vec![PathBuf::from("/a.md"), PathBuf::from("/b.md")];
        assert_eq!(serialize(&paths), "/a.md\n/b.md");
    }

    #[test]
    fn parse_and_serialize_round_trip() {
        let paths = vec![PathBuf::from("/a.md"), PathBuf::from("/b.md")];
        assert_eq!(parse(&serialize(&paths)), paths);
    }

    #[test]
    fn recording_into_an_empty_list_prepends_it() {
        assert_eq!(
            record(&[], Path::new("/a.md"), 10),
            vec![PathBuf::from("/a.md")]
        );
    }

    #[test]
    fn recording_a_new_path_prepends_it_keeping_the_rest() {
        let existing = vec![PathBuf::from("/a.md"), PathBuf::from("/b.md")];
        assert_eq!(
            record(&existing, Path::new("/c.md"), 10),
            vec![
                PathBuf::from("/c.md"),
                PathBuf::from("/a.md"),
                PathBuf::from("/b.md"),
            ]
        );
    }

    #[test]
    fn reopening_an_already_present_path_moves_it_to_front_without_duplicating() {
        let existing = vec![
            PathBuf::from("/a.md"),
            PathBuf::from("/b.md"),
            PathBuf::from("/c.md"),
        ];
        assert_eq!(
            record(&existing, Path::new("/b.md"), 10),
            vec![
                PathBuf::from("/b.md"),
                PathBuf::from("/a.md"),
                PathBuf::from("/c.md"),
            ]
        );
    }

    #[test]
    fn list_is_capped_at_max_dropping_the_oldest() {
        let existing = vec![PathBuf::from("/a.md"), PathBuf::from("/b.md")];
        assert_eq!(
            record(&existing, Path::new("/c.md"), 2),
            vec![PathBuf::from("/c.md"), PathBuf::from("/a.md")]
        );
    }

    #[test]
    fn missing_file_has_no_recent_entries() {
        let fs = InMemoryFileSystem::new();
        assert_eq!(load(&fs, Path::new("/recent")), Vec::<PathBuf>::new());
    }

    #[test]
    fn save_then_load_round_trips() {
        let fs = InMemoryFileSystem::new();
        let paths = vec![PathBuf::from("/a.md"), PathBuf::from("/b.md")];
        save(&fs, Path::new("/recent"), &paths).unwrap();
        assert_eq!(load(&fs, Path::new("/recent")), paths);
    }
}

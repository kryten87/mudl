//! Comment label allocation (Phase 14.2). Ported from the labelling half of
//! `mud`'s `Core/Sources/Comments/CommentEditor.swift` (`nextLabel`,
//! `schemeValidSuffixes`, `isSchemeValid`, `increment`).
//!
//! `mud` scans the raw source and allocates a label in one step
//! (`nextLabel(in: source)`); this port splits that into a scan
//! ([`existing_labels`], mechanical) and a pure allocation decision
//! ([`next_label`], over the resulting list) so the allocation logic is
//! testable without any Markdown fixture at all.

/// Scans `source` for `[^comment-<suffix>]` occurrences (references and
/// definitions alike) and returns each as a full label string
/// (`comment-<suffix>`), duplicates included, in source order. Every
/// candidate is returned regardless of whether its suffix fits the
/// allocation scheme -- [`next_label`] is what decides which ones count as
/// a basis for the next label.
pub fn existing_labels(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let prefix = b"[^comment-";
    let mut labels = Vec::new();
    let mut i = 0;
    while i + prefix.len() <= bytes.len() {
        if &bytes[i..i + prefix.len()] != prefix {
            i += 1;
            continue;
        }
        let suffix_start = i + prefix.len();
        let mut j = suffix_start;
        while j < bytes.len() && is_label_byte(bytes[j]) {
            j += 1;
        }
        if j > suffix_start && j < bytes.len() && bytes[j] == b']' {
            // Every byte in `suffix_start..j` matched `is_label_byte`, which
            // only ever accepts single-byte ASCII, so this is always valid
            // UTF-8.
            let suffix = std::str::from_utf8(&bytes[suffix_start..j]).unwrap();
            labels.push(format!("comment-{suffix}"));
        }
        i = j.max(i + 1);
    }
    labels
}

/// A label-suffix byte: `[A-Za-z0-9_-]`.
fn is_label_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// The next label: `comment-` plus the lexicographically greatest existing
/// *scheme-valid* suffix among `existing`, incremented (last letter `z` =>
/// append `a`, otherwise bump it). Anomalous entries (an `existing` value
/// with no `comment-` prefix, or a suffix outside the scheme, e.g. `az`,
/// `aa`, `comment-1`, `comment-foo`) are ignored as the basis, so they can
/// neither lengthen nor misdirect the next label; the result exceeds every
/// scheme-valid label and differs from every anomaly, so it can never
/// collide.
pub fn next_label(existing: &[String]) -> String {
    let greatest = existing
        .iter()
        .filter_map(|label| label.strip_prefix("comment-"))
        .filter(|suffix| is_scheme_valid(suffix))
        .max();
    match greatest {
        Some(suffix) => format!("comment-{}", increment(suffix)),
        None => "comment-a".to_string(),
    }
}

/// True when `s` is a suffix the scheme itself produces: `z*[a-y]` (zero or
/// more `z` then one `a`-`y`) or `z+`.
pub fn is_scheme_valid(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() || !chars.iter().all(|c| c.is_ascii_lowercase()) {
        return false;
    }
    if chars.iter().all(|&c| c == 'z') {
        return true; // z+
    }
    let (last, rest) = chars.split_last().unwrap();
    ('a'..='y').contains(last) && rest.iter().all(|&c| c == 'z')
}

/// Increments a scheme-valid suffix: a trailing `z` rolls over by appending
/// `a` (`z` -> `za`, `zz` -> `zza`); otherwise the last letter bumps up
/// (`a` -> `b`, `zy` -> `zz`).
pub fn increment(s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    if chars.last() == Some(&'z') {
        return format!("{s}a");
    }
    let last = chars.pop().unwrap();
    let bumped = char::from_u32(last as u32 + 1).unwrap();
    chars.push(bumped);
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // MARK: - existing_labels

    #[test]
    fn existing_labels_none_found() {
        assert!(existing_labels("no comments here").is_empty());
    }

    #[test]
    fn existing_labels_finds_references() {
        assert_eq!(
            existing_labels("x[^comment-a] y[^comment-b]"),
            vec!["comment-a".to_string(), "comment-b".to_string()]
        );
    }

    #[test]
    fn existing_labels_finds_anomalous_suffixes_too() {
        assert_eq!(
            existing_labels("[^comment-1][^comment-foo]"),
            vec!["comment-1".to_string(), "comment-foo".to_string()]
        );
    }

    #[test]
    fn existing_labels_ignores_unterminated_or_empty_suffix() {
        assert!(existing_labels("[^comment-]").is_empty());
        assert!(existing_labels("[^comment-abc no closing bracket").is_empty());
    }

    // MARK: - next_label

    #[test]
    fn next_label_empty_existing() {
        assert_eq!(next_label(&[]), "comment-a");
    }

    #[test]
    fn next_label_increments_greatest() {
        let existing = vec!["comment-a".to_string(), "comment-b".to_string()];
        assert_eq!(next_label(&existing), "comment-c");
    }

    #[test]
    fn next_label_rolls_over_at_z() {
        assert_eq!(next_label(&["comment-z".to_string()]), "comment-za");
        assert_eq!(next_label(&["comment-zz".to_string()]), "comment-zza");
    }

    #[test]
    fn next_label_ignores_anomalies() {
        // `ya` is not scheme-valid, so the basis is `b` -> `c`.
        let existing = vec![
            "comment-a".to_string(),
            "comment-b".to_string(),
            "comment-ya".to_string(),
        ];
        assert_eq!(next_label(&existing), "comment-c");

        // `aa` is not scheme-valid -> basis `a` -> `b`.
        let existing = vec!["comment-a".to_string(), "comment-aa".to_string()];
        assert_eq!(next_label(&existing), "comment-b");

        // Only anomalies present -> start over at `a`.
        let existing = vec!["comment-1".to_string(), "comment-foo".to_string()];
        assert_eq!(next_label(&existing), "comment-a");
    }

    #[test]
    fn next_label_ignores_entries_without_the_comment_prefix() {
        assert_eq!(next_label(&["footnote-a".to_string()]), "comment-a");
    }

    // End-to-end, mirroring `mud`'s `nextLabel(in: source)` (scan + allocate
    // in one step).
    #[test]
    fn next_label_end_to_end_over_raw_source() {
        assert_eq!(
            next_label(&existing_labels("x[^comment-a] y[^comment-b]")),
            "comment-c"
        );
        assert_eq!(next_label(&existing_labels("[^comment-z]")), "comment-za");
        assert_eq!(
            next_label(&existing_labels("[^comment-1][^comment-foo]")),
            "comment-a"
        );
    }

    // MARK: - increment

    #[test]
    fn increment_cases() {
        assert_eq!(increment("a"), "b");
        assert_eq!(increment("y"), "z");
        assert_eq!(increment("z"), "za");
        assert_eq!(increment("za"), "zb");
        assert_eq!(increment("zy"), "zz");
        assert_eq!(increment("zz"), "zza");
    }

    // MARK: - is_scheme_valid

    #[test]
    fn is_scheme_valid_cases() {
        for valid in ["a", "y", "z", "za", "zy", "zz", "zza"] {
            assert!(is_scheme_valid(valid), "{valid} should be valid");
        }
        for invalid in ["", "ya", "az", "aa", "1", "foo", "a-b", "Z"] {
            assert!(!is_scheme_valid(invalid), "{invalid} should be invalid");
        }
    }
}

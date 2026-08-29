//! The on-disk preferences format (Phase 7, step 7.1): a flat `key = value`
//! text file, one entry per line, `#`-prefixed comments and blank lines
//! ignored. Lives at `~/.config/mudl/preferences` (the impure load/save
//! wrapper around this pure parser is added in step 7.4).

/// Parses `text` into an ordered list of `(key, value)` pairs.
///
/// - Blank lines and lines whose first non-whitespace character is `#` are
///   ignored.
/// - Leading/trailing whitespace around both the key and the value is
///   trimmed.
/// - A line with no `=` is malformed and is skipped, not an error.
/// - Only the *first* `=` on a line splits key from value, so a value
///   containing `=` is preserved verbatim.
/// - If the same key appears more than once, the last occurrence wins (but
///   keeps its original position in the returned order, matching a plain
///   sequential overwrite).
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        let value = value.trim().to_string();

        if let Some(existing) = entries.iter_mut().find(|(k, _)| *k == key) {
            existing.1 = value;
        } else {
            entries.push((key, value));
        }
    }

    entries
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn empty_file() {
        assert_eq!(parse(""), Vec::<(String, String)>::new());
    }

    #[test]
    fn comment_only_file() {
        let text = "# a comment\n# another comment\n";
        assert_eq!(parse(text), Vec::<(String, String)>::new());
    }

    #[test]
    fn single_key() {
        assert_eq!(
            parse("theme = earthy"),
            vec![("theme".to_string(), "earthy".to_string())]
        );
    }

    #[test]
    fn multiple_keys() {
        let text = "theme = earthy\nlighting = dark\n";
        assert_eq!(
            parse(text),
            vec![
                ("theme".to_string(), "earthy".to_string()),
                ("lighting".to_string(), "dark".to_string()),
            ]
        );
    }

    #[test]
    fn extra_whitespace_around_equals() {
        assert_eq!(
            parse("theme    =   earthy  "),
            vec![("theme".to_string(), "earthy".to_string())]
        );
    }

    #[test]
    fn malformed_line_with_no_equals_is_skipped() {
        let text = "theme = earthy\nnot a valid line\nlighting = dark\n";
        assert_eq!(
            parse(text),
            vec![
                ("theme".to_string(), "earthy".to_string()),
                ("lighting".to_string(), "dark".to_string()),
            ]
        );
    }

    #[test]
    fn duplicate_key_last_occurrence_wins() {
        let text = "theme = earthy\ntheme = blues\n";
        assert_eq!(
            parse(text),
            vec![("theme".to_string(), "blues".to_string())]
        );
    }

    #[test]
    fn duplicate_key_keeps_original_position() {
        let text = "theme = earthy\nlighting = dark\ntheme = blues\n";
        assert_eq!(
            parse(text),
            vec![
                ("theme".to_string(), "blues".to_string()),
                ("lighting".to_string(), "dark".to_string()),
            ]
        );
    }

    #[test]
    fn value_containing_equals_only_splits_on_first() {
        assert_eq!(
            parse("custom_command = a=b=c"),
            vec![("custom_command".to_string(), "a=b=c".to_string())]
        );
    }

    #[test]
    fn trailing_and_leading_blank_lines() {
        let text = "\n\n\ntheme = earthy\n\n\n";
        assert_eq!(
            parse(text),
            vec![("theme".to_string(), "earthy".to_string())]
        );
    }

    #[test]
    fn indented_comment_is_ignored() {
        let text = "   # indented comment\ntheme = earthy\n";
        assert_eq!(
            parse(text),
            vec![("theme".to_string(), "earthy".to_string())]
        );
    }

    #[test]
    fn line_with_only_equals_yields_empty_key_and_value() {
        assert_eq!(parse("="), vec![(String::new(), String::new())]);
    }
}

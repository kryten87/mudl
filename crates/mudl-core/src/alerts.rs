/// Visual category for GFM alerts and DocC asides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertCategory {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
    Status,
}

/// Controls how DocC asides are processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocCAlertMode {
    /// No DocC asides are processed; all blockquotes render as plain.
    Off,
    /// Only the 6 canonical DocC kinds are processed.
    Common,
    /// All DocC kinds, including extended aliases, are processed.
    Extended,
}

impl Default for DocCAlertMode {
    /// Matches `mud`'s documented default (`markdownDocCAlertMode` = `extended`,
    /// see Appendix B of the implementation plan).
    fn default() -> Self {
        DocCAlertMode::Extended
    }
}

const GFM_ALERT_TAGS: &[(&str, AlertCategory)] = &[
    ("[!NOTE]", AlertCategory::Note),
    ("[!TIP]", AlertCategory::Tip),
    ("[!IMPORTANT]", AlertCategory::Important),
    ("[!STATUS]", AlertCategory::Status),
    ("[!WARNING]", AlertCategory::Warning),
    ("[!CAUTION]", AlertCategory::Caution),
];

/// Returns the alert category for a GFM blockquote's first line, or `None`
/// if it does not begin with a recognised `[!TAG]` (matching is case
/// sensitive, mirroring Swift's `hasPrefix`).
pub fn detect_gfm_alert(first_line: &str) -> Option<(AlertCategory, ())> {
    if !first_line.starts_with("[!") {
        return None;
    }
    GFM_ALERT_TAGS
        .iter()
        .find(|(tag, _)| first_line.starts_with(tag))
        .map(|(_, category)| (*category, ()))
}

/// Parses a leading `Kind:` tag off `first_line`. Returns the raw tag and
/// the UTF-8 byte length of the tag plus its colon and any trailing
/// spaces/tabs, or `None` if there's no colon.
///
/// Measures only substrings of `first_line` itself — never a byte range
/// computed against some other, possibly-rewritten source string — so there
/// is no arithmetic here that can invert and panic, unlike swift-markdown's
/// `Aside.parseAsideTag`, which could trap on smart-typography input whose
/// decoded string outgrew its pre-expansion source span.
pub fn parse_aside_tag(first_line: &str) -> Option<(String, usize)> {
    let colon_index = first_line.find(':')?;
    let tag = &first_line[..colon_index];
    let after_colon = &first_line[colon_index + 1..];
    let trailing_whitespace_len = after_colon
        .as_bytes()
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let byte_length = tag.len() + 1 + trailing_whitespace_len;
    Some((tag.to_string(), byte_length))
}

fn core_category(tag: &str) -> Option<AlertCategory> {
    match tag {
        "Note" => Some(AlertCategory::Note),
        "Tip" => Some(AlertCategory::Tip),
        "Important" => Some(AlertCategory::Important),
        "Warning" => Some(AlertCategory::Warning),
        "Caution" => Some(AlertCategory::Caution),
        "Status" => Some(AlertCategory::Status),
        _ => None,
    }
}

fn extended_category(tag: &str) -> Option<AlertCategory> {
    match tag {
        "Remark" | "Complexity" | "Author" | "Authors" | "Copyright" | "Date" | "Since"
        | "Version" | "SeeAlso" | "MutatingVariant" | "NonMutatingVariant" => {
            Some(AlertCategory::Note)
        }
        "ToDo" => Some(AlertCategory::Status),
        "Experiment" => Some(AlertCategory::Tip),
        "Attention" => Some(AlertCategory::Important),
        "Precondition" | "Postcondition" | "Requires" | "Invariant" => Some(AlertCategory::Warning),
        "Bug" | "Throws" | "Error" => Some(AlertCategory::Caution),
        _ => None,
    }
}

/// Returns the alert category for a DocC aside `tag`, or `None` if
/// `mode` excludes it (`Off` excludes everything; `Common` excludes the
/// extended aliases) or if `tag` is not a recognised DocC kind at all.
pub fn detect_docc_aside(tag: &str, mode: DocCAlertMode) -> Option<AlertCategory> {
    if mode == DocCAlertMode::Off {
        return None;
    }
    if let Some(category) = core_category(tag) {
        return Some(category);
    }
    if mode == DocCAlertMode::Extended {
        return extended_category(tag);
    }
    None
}

#[cfg(test)]
mod gfm_tests {
    use super::{detect_gfm_alert, AlertCategory};

    #[test]
    fn note_recognized() {
        assert_eq!(
            detect_gfm_alert("[!NOTE]\nBody."),
            Some((AlertCategory::Note, ()))
        );
    }

    #[test]
    fn tip_recognized() {
        assert_eq!(
            detect_gfm_alert("[!TIP]\nBody."),
            Some((AlertCategory::Tip, ()))
        );
    }

    #[test]
    fn important_recognized() {
        assert_eq!(
            detect_gfm_alert("[!IMPORTANT]\nBody."),
            Some((AlertCategory::Important, ()))
        );
    }

    #[test]
    fn warning_recognized() {
        assert_eq!(
            detect_gfm_alert("[!WARNING]\nBody."),
            Some((AlertCategory::Warning, ()))
        );
    }

    #[test]
    fn caution_recognized() {
        assert_eq!(
            detect_gfm_alert("[!CAUTION]\nBody."),
            Some((AlertCategory::Caution, ()))
        );
    }

    #[test]
    fn status_recognized() {
        assert_eq!(
            detect_gfm_alert("[!STATUS]\nBody."),
            Some((AlertCategory::Status, ()))
        );
    }

    #[test]
    fn plain_quote_is_none() {
        assert_eq!(detect_gfm_alert("Not an alert."), None);
    }

    #[test]
    fn missing_closing_bracket_is_none() {
        assert_eq!(detect_gfm_alert("[!NOTE\nBody."), None);
    }

    #[test]
    fn missing_bang_is_none() {
        assert_eq!(detect_gfm_alert("[NOTE]\nBody."), None);
    }

    #[test]
    fn wrong_case_is_none() {
        assert_eq!(detect_gfm_alert("[!note]\nBody."), None);
    }

    #[test]
    fn unrecognized_tag_with_valid_shape_is_none() {
        assert_eq!(detect_gfm_alert("[!BOGUS]\nBody."), None);
    }

    #[test]
    fn empty_string_is_none() {
        assert_eq!(detect_gfm_alert(""), None);
    }
}

#[cfg(test)]
mod parse_aside_tag_tests {
    use super::parse_aside_tag;

    #[test]
    fn tagless_quote_is_none() {
        assert_eq!(parse_aside_tag("Plain quote, no tag."), None);
    }

    #[test]
    fn empty_string_is_none() {
        assert_eq!(parse_aside_tag(""), None);
    }

    #[test]
    fn skips_tag_colon_and_single_space() {
        let (tag, len) = parse_aside_tag("Note: Body text.").unwrap();
        assert_eq!(tag, "Note");
        assert_eq!(len, "Note: ".len());
    }

    #[test]
    fn skips_multiple_trailing_spaces() {
        let (tag, len) = parse_aside_tag("Note:   Body.").unwrap();
        assert_eq!(tag, "Note");
        assert_eq!(len, "Note:   ".len());
    }

    #[test]
    fn skips_trailing_tab() {
        let (tag, len) = parse_aside_tag("Note:\tBody.").unwrap();
        assert_eq!(tag, "Note");
        assert_eq!(len, "Note:\t".len());
    }

    #[test]
    fn no_trailing_whitespace() {
        let (tag, len) = parse_aside_tag("Note:Body.").unwrap();
        assert_eq!(tag, "Note");
        assert_eq!(len, "Note:".len());
    }

    #[test]
    fn colon_at_end_of_string() {
        let (tag, len) = parse_aside_tag("Note:").unwrap();
        assert_eq!(tag, "Note");
        assert_eq!(len, "Note:".len());
    }

    /// Regression test for the tag-shift crash swift-markdown's `Aside` was
    /// vulnerable to: smart typography expands the apostrophe in "Don't"
    /// from one byte to three, which could invert a range built against the
    /// pre-expansion source span and trap. `parse_aside_tag` measures the
    /// literal directly (no source-range arithmetic), so "Don't: x" just
    /// parses to an unrecognized tag rather than panicking.
    #[test]
    fn tag_shift_crash_input_does_not_panic() {
        let (tag, len) = parse_aside_tag("Don't: x").unwrap();
        assert_eq!(tag, "Don't");
        assert_eq!(len, "Don't: ".len());
    }

    /// A pathological short input: the colon is the very last byte, so any
    /// "remaining content" arithmetic that assumed bytes existed past it
    /// must not panic when slicing.
    #[test]
    fn colon_only_string_does_not_panic() {
        let (tag, len) = parse_aside_tag(":").unwrap();
        assert_eq!(tag, "");
        assert_eq!(len, 1);
    }

    /// No colon at all in a single-byte string: must return `None` rather
    /// than panic while searching for one.
    #[test]
    fn single_char_no_colon_does_not_panic() {
        assert_eq!(parse_aside_tag("N"), None);
    }

    #[test]
    fn multibyte_utf8_tag_byte_length_is_correct() {
        let (tag, len) = parse_aside_tag("Ñoño: x").unwrap();
        assert_eq!(tag, "Ñoño");
        assert_eq!(len, "Ñoño: ".len());
        assert!(len > tag.chars().count());
    }
}

#[cfg(test)]
mod docc_alert_tests {
    use super::{detect_docc_aside, AlertCategory, DocCAlertMode};

    #[test]
    fn unrecognized_tag_is_none() {
        assert_eq!(
            detect_docc_aside("Unrecognized", DocCAlertMode::Extended),
            None
        );
    }

    #[test]
    fn off_mode_rejects_core_kind() {
        assert_eq!(detect_docc_aside("Note", DocCAlertMode::Off), None);
    }

    #[test]
    fn off_mode_rejects_extended_alias() {
        assert_eq!(detect_docc_aside("Bug", DocCAlertMode::Off), None);
    }

    #[test]
    fn common_mode_matches_core_kinds() {
        assert_eq!(
            detect_docc_aside("Note", DocCAlertMode::Common),
            Some(AlertCategory::Note)
        );
        assert_eq!(
            detect_docc_aside("Tip", DocCAlertMode::Common),
            Some(AlertCategory::Tip)
        );
        assert_eq!(
            detect_docc_aside("Important", DocCAlertMode::Common),
            Some(AlertCategory::Important)
        );
        assert_eq!(
            detect_docc_aside("Warning", DocCAlertMode::Common),
            Some(AlertCategory::Warning)
        );
        assert_eq!(
            detect_docc_aside("Caution", DocCAlertMode::Common),
            Some(AlertCategory::Caution)
        );
        assert_eq!(
            detect_docc_aside("Status", DocCAlertMode::Common),
            Some(AlertCategory::Status)
        );
    }

    #[test]
    fn common_mode_excludes_extended_aliases() {
        assert_eq!(detect_docc_aside("Bug", DocCAlertMode::Common), None);
    }

    #[test]
    fn extended_mode_matches_core_kinds() {
        assert_eq!(
            detect_docc_aside("Note", DocCAlertMode::Extended),
            Some(AlertCategory::Note)
        );
    }

    #[test]
    fn extended_mode_matches_every_extended_alias() {
        let cases = [
            ("Remark", AlertCategory::Note),
            ("Complexity", AlertCategory::Note),
            ("Author", AlertCategory::Note),
            ("Authors", AlertCategory::Note),
            ("Copyright", AlertCategory::Note),
            ("Date", AlertCategory::Note),
            ("Since", AlertCategory::Note),
            ("Version", AlertCategory::Note),
            ("SeeAlso", AlertCategory::Note),
            ("MutatingVariant", AlertCategory::Note),
            ("NonMutatingVariant", AlertCategory::Note),
            ("ToDo", AlertCategory::Status),
            ("Experiment", AlertCategory::Tip),
            ("Attention", AlertCategory::Important),
            ("Precondition", AlertCategory::Warning),
            ("Postcondition", AlertCategory::Warning),
            ("Requires", AlertCategory::Warning),
            ("Invariant", AlertCategory::Warning),
            ("Bug", AlertCategory::Caution),
            ("Throws", AlertCategory::Caution),
            ("Error", AlertCategory::Caution),
        ];
        for (tag, expected) in cases {
            assert_eq!(
                detect_docc_aside(tag, DocCAlertMode::Extended),
                Some(expected),
                "tag {tag} should map to {expected:?}"
            );
        }
    }

    #[test]
    fn empty_tag_is_none() {
        assert_eq!(detect_docc_aside("", DocCAlertMode::Extended), None);
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::{detect_docc_aside, parse_aside_tag, AlertCategory, DocCAlertMode};

    #[test]
    fn parse_then_detect_round_trip() {
        let (tag, len) = parse_aside_tag("Bug: Body text.").unwrap();
        assert_eq!(len, "Bug: ".len());
        assert_eq!(
            detect_docc_aside(&tag, DocCAlertMode::Extended),
            Some(AlertCategory::Caution)
        );
    }

    #[test]
    fn tagless_quote_never_reaches_detection() {
        assert_eq!(parse_aside_tag("Plain quote, no tag."), None);
    }
}

#[cfg(test)]
mod doc_c_alert_mode_default_tests {
    use super::DocCAlertMode;

    #[test]
    fn default_is_extended() {
        assert_eq!(DocCAlertMode::default(), DocCAlertMode::Extended);
    }
}

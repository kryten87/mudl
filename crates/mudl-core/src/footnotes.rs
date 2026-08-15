const COMMENT_LABEL_PREFIX: &str = "comment-";

/// True when `label` is a comment label (`comment-` followed by one or more
/// `[\w-]` characters) rather than an authorial footnote label. Mirrors
/// `FootnoteProcessor.isCommentLabel` in the Swift reference, including its
/// Unicode-aware notion of "word character" (`Character.isLetter` /
/// `Character.isNumber`, not ASCII-only), so labels written with non-ASCII
/// letters stay classified the same way on both ports.
pub fn is_comment_label(label: &str) -> bool {
    match label.strip_prefix(COMMENT_LABEL_PREFIX) {
        Some(suffix) if !suffix.is_empty() => suffix
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_label_is_recognized() {
        assert!(is_comment_label("comment-abc123"));
    }

    #[test]
    fn empty_suffix_is_rejected() {
        assert!(!is_comment_label("comment-"));
    }

    #[test]
    fn suffix_with_space_is_rejected() {
        assert!(!is_comment_label("comment-a b"));
    }

    #[test]
    fn suffix_with_period_is_rejected() {
        assert!(!is_comment_label("comment-a.b"));
    }

    #[test]
    fn missing_prefix_is_rejected() {
        assert!(!is_comment_label("footnote-abc123"));
    }

    #[test]
    fn empty_label_is_rejected() {
        assert!(!is_comment_label(""));
    }

    #[test]
    fn prefix_only_as_substring_is_rejected() {
        assert!(!is_comment_label("xcomment-abc"));
    }

    #[test]
    fn label_with_only_hyphen_suffix_is_accepted() {
        assert!(is_comment_label("comment---"));
    }

    #[test]
    fn label_with_underscore_suffix_is_accepted() {
        assert!(is_comment_label("comment-_"));
    }

    #[test]
    fn case_sensitive_prefix_is_rejected() {
        assert!(!is_comment_label("Comment-abc"));
    }
}

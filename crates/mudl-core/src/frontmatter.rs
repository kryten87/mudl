pub struct FrontMatter {
    pub yaml: String,
    pub body: String,
    pub line_count: usize,
}

pub fn extract(markdown: &str) -> Option<FrontMatter> {
    // \r\n is normalized to \n before scanning so CRLF input splits the
    // same way as LF input.
    let normalized = markdown.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();

    if lines.is_empty() || !is_delimiter(lines[0], true) {
        return None;
    }

    for i in 1..lines.len() {
        if is_delimiter(lines[i], false) {
            let yaml = lines[1..i].join("\n");
            let body = lines[(i + 1)..].join("\n");
            return Some(FrontMatter {
                yaml,
                body,
                line_count: i + 1,
            });
        }
    }

    None
}

fn is_delimiter(line: &str, opening: bool) -> bool {
    let content = trim_spaces_tabs_end(line);
    if content == "---" {
        return true;
    }
    if !opening && content == "..." {
        return true;
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontMatterValue {
    Scalar(String),
    InlineArray(Vec<String>),
    Block(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: FrontMatterValue,
}

pub fn parse_top_level_keys(yaml: &str) -> Vec<KeyValue> {
    if yaml.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut current_key: Option<String> = None;
    let mut current_value = String::new();
    let mut continuation_lines: Vec<String> = Vec::new();

    for line in yaml.split('\n') {
        let indented = line.starts_with(' ') || line.starts_with('\t');

        if !indented && line.starts_with('#') {
            continue;
        }

        if indented {
            if current_key.is_some() {
                continuation_lines.push(line.to_string());
            }
            continue;
        }

        if let Some(key) = current_key.take() {
            result.push(KeyValue {
                key,
                value: build_value(&current_value, &continuation_lines),
            });
        }

        match line.find(':').filter(|&i| i > 0) {
            Some(colon_index) => {
                current_key = Some(trim_spaces_tabs(&line[..colon_index]).to_string());
                current_value = trim_spaces_tabs(&line[colon_index + 1..]).to_string();
                continuation_lines = Vec::new();
            }
            None => {
                current_key = None;
                current_value = String::new();
                continuation_lines = Vec::new();
            }
        }
    }

    if let Some(key) = current_key {
        result.push(KeyValue {
            key,
            value: build_value(&current_value, &continuation_lines),
        });
    }

    result
}

fn build_value(first_line: &str, continuation: &[String]) -> FrontMatterValue {
    if continuation.is_empty() {
        return parse_scalar_or_inline_array(first_line);
    }
    if first_line.is_empty() {
        return FrontMatterValue::Block(continuation.join("\n"));
    }
    let mut lines = Vec::with_capacity(continuation.len() + 1);
    lines.push(first_line.to_string());
    lines.extend_from_slice(continuation);
    FrontMatterValue::Block(lines.join("\n"))
}

fn parse_scalar_or_inline_array(value: &str) -> FrontMatterValue {
    let trimmed = trim_spaces_tabs(value);
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let inner = &trimmed[1..trimmed.len() - 1];
        // Adjacent/leading/trailing commas contribute no element (mirrors
        // Swift's `split(separator:)` default of omitting empty
        // subsequences); a whitespace-only segment between commas still
        // counts and trims down to an empty scalar.
        let elements: Vec<String> = inner
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| trim_spaces_tabs(s).to_string())
            .collect();
        return FrontMatterValue::InlineArray(elements);
    }
    FrontMatterValue::Scalar(trimmed.to_string())
}

fn trim_spaces_tabs(s: &str) -> &str {
    s.trim_matches([' ', '\t'])
}

fn trim_spaces_tabs_end(s: &str) -> &str {
    s.trim_end_matches([' ', '\t'])
}

#[cfg(test)]
mod extract_tests {
    use super::*;

    #[test]
    fn standard_frontmatter() {
        let input = "---\ntitle: Hello\nauthor: Jane\n---\n\n# Heading\n";
        let result = extract(input).unwrap();
        assert_eq!(result.yaml, "title: Hello\nauthor: Jane");
        assert_eq!(result.body, "\n# Heading\n");
        assert_eq!(result.line_count, 4);
    }

    #[test]
    fn empty_frontmatter_body() {
        let input = "---\n---\n\nBody text\n";
        let result = extract(input).unwrap();
        assert_eq!(result.yaml, "");
        assert_eq!(result.body, "\nBody text\n");
        assert_eq!(result.line_count, 2);
    }

    #[test]
    fn closing_with_dots() {
        let input = "---\ntitle: Hello\n...\n\nBody\n";
        let result = extract(input).unwrap();
        assert_eq!(result.yaml, "title: Hello");
    }

    #[test]
    fn no_closing_delimiter() {
        let input = "---\ntitle: Hello\nauthor: Jane\n";
        assert!(extract(input).is_none());
    }

    #[test]
    fn not_on_line_one() {
        let input = "\n---\ntitle: Hello\n---\n";
        assert!(extract(input).is_none());
    }

    #[test]
    fn text_before_delimiter() {
        let input = "Some text\n---\ntitle: Hello\n---\n";
        assert!(extract(input).is_none());
    }

    #[test]
    fn trailing_whitespace_on_delimiters() {
        let input = "---  \ntitle: Hello\n---  \n\nBody\n";
        let result = extract(input).unwrap();
        assert_eq!(result.yaml, "title: Hello");
    }

    #[test]
    fn windows_line_endings() {
        let input = "---\r\ntitle: Hello\r\n---\r\n\r\nBody\r\n";
        let result = extract(input).unwrap();
        assert_eq!(result.yaml, "title: Hello");
    }

    #[test]
    fn no_frontmatter() {
        let input = "# Just a heading\n\nSome body text.\n";
        assert!(extract(input).is_none());
    }

    #[test]
    fn frontmatter_only() {
        let input = "---\ntitle: Hello\n---\n";
        let result = extract(input).unwrap();
        assert_eq!(result.yaml, "title: Hello");
    }

    #[test]
    fn frontmatter_only_no_trailing_newline() {
        let input = "---\ntitle: Hello\n---";
        let result = extract(input).unwrap();
        assert_eq!(result.yaml, "title: Hello");
    }

    #[test]
    fn thematic_break_later_in_document() {
        let input = "# Heading\n\n---\n\nMore text\n";
        assert!(extract(input).is_none());
    }

    #[test]
    fn empty_input() {
        assert!(extract("").is_none());
    }
}

#[cfg(test)]
mod parse_top_level_keys_tests {
    use super::*;

    #[test]
    fn simple_key_value_pairs() {
        let yaml = "title: My Document\nauthor: Jane Doe\ndate: 2026-04-08";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0].key, "title");
        assert_eq!(
            keys[0].value,
            FrontMatterValue::Scalar("My Document".into())
        );
        assert_eq!(keys[1].key, "author");
        assert_eq!(keys[1].value, FrontMatterValue::Scalar("Jane Doe".into()));
        assert_eq!(keys[2].key, "date");
        assert_eq!(keys[2].value, FrontMatterValue::Scalar("2026-04-08".into()));
    }

    #[test]
    fn inline_array() {
        let yaml = "tags: [swift, markdown, preview]";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, "tags");
        assert_eq!(
            keys[0].value,
            FrontMatterValue::InlineArray(vec![
                "swift".into(),
                "markdown".into(),
                "preview".into()
            ])
        );
    }

    #[test]
    fn block_array() {
        let yaml = "tags:\n  - swift\n  - markdown\n  - preview";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, "tags");
        assert_eq!(
            keys[0].value,
            FrontMatterValue::Block("  - swift\n  - markdown\n  - preview".into())
        );
    }

    #[test]
    fn nested_mapping() {
        let yaml = "config:\n  nested:\n    key: value\n    other: thing";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, "config");
        assert_eq!(
            keys[0].value,
            FrontMatterValue::Block("  nested:\n    key: value\n    other: thing".into())
        );
    }

    #[test]
    fn multi_line_scalar_literal() {
        let yaml = "description: |\n  First line\n  Second line";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, "description");
        assert_eq!(
            keys[0].value,
            FrontMatterValue::Block("|\n  First line\n  Second line".into())
        );
    }

    #[test]
    fn multi_line_scalar_folded() {
        let yaml = "description: >\n  First line\n  Second line";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, "description");
        assert_eq!(
            keys[0].value,
            FrontMatterValue::Block(">\n  First line\n  Second line".into())
        );
    }

    #[test]
    fn quoted_values_preserved() {
        let yaml = "title: \"My Document\"\nsubtitle: 'Another Title'";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 2);
        assert_eq!(
            keys[0].value,
            FrontMatterValue::Scalar("\"My Document\"".into())
        );
        assert_eq!(
            keys[1].value,
            FrontMatterValue::Scalar("'Another Title'".into())
        );
    }

    #[test]
    fn comment_lines_between_keys() {
        let yaml = "title: Hello\n# This is a comment\nauthor: Jane";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].key, "title");
        assert_eq!(keys[1].key, "author");
    }

    #[test]
    fn all_comments_returns_empty() {
        let yaml = "# Just a comment\n# Another comment";
        assert!(parse_top_level_keys(yaml).is_empty());
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(parse_top_level_keys("").is_empty());
    }

    #[test]
    fn mixed_simple_and_complex() {
        let yaml = "title: Hello\ntags: [a, b]\nconfig:\n  key: value\ndraft: true";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 4);
        assert_eq!(keys[0].value, FrontMatterValue::Scalar("Hello".into()));
        assert_eq!(
            keys[1].value,
            FrontMatterValue::InlineArray(vec!["a".into(), "b".into()])
        );
        assert_eq!(
            keys[2].value,
            FrontMatterValue::Block("  key: value".into())
        );
        assert_eq!(keys[3].value, FrontMatterValue::Scalar("true".into()));
    }

    #[test]
    fn indented_comment_line_is_kept_as_continuation() {
        let yaml = "description: |\n  # not a comment here\n  actual text";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 1);
        assert_eq!(
            keys[0].value,
            FrontMatterValue::Block("|\n  # not a comment here\n  actual text".into())
        );
    }

    #[test]
    fn inline_array_with_adjacent_commas_omits_empty_elements() {
        let yaml = "tags: [a,,b,]";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(
            keys[0].value,
            FrontMatterValue::InlineArray(vec!["a".into(), "b".into()])
        );
    }

    #[test]
    fn line_starting_with_colon_is_not_a_key() {
        let yaml = "title: Hello\n: not a key\nauthor: Jane";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].key, "title");
        assert_eq!(keys[1].key, "author");
    }

    #[test]
    fn trailing_continuation_lines_are_flushed_at_end_of_input() {
        let yaml = "tags:\n  - a\n  - b";
        let keys = parse_top_level_keys(yaml);
        assert_eq!(keys.len(), 1);
        assert_eq!(
            keys[0].value,
            FrontMatterValue::Block("  - a\n  - b".into())
        );
    }
}

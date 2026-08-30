//! Word-level diffing (ported from `mud`'s `Core/Sources/Diff/WordDiff.swift`).
//!
//! Phase 13.1 (`tokenize`/`extract_words`) and 13.2 (`diff`/`similarity`/
//! `has_significant_changes`) share this module, matching the Swift
//! source's single `WordDiff` enum.

/// A span in a word-level diff result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordSpan {
    Unchanged(String),
    Inserted(String),
    Deleted(String),
}

impl WordSpan {
    pub fn text(&self) -> &str {
        match self {
            WordSpan::Unchanged(t) | WordSpan::Inserted(t) | WordSpan::Deleted(t) => t,
        }
    }

    pub fn is_unchanged(&self) -> bool {
        matches!(self, WordSpan::Unchanged(_))
    }

    pub fn is_inserted(&self) -> bool {
        matches!(self, WordSpan::Inserted(_))
    }

    pub fn is_deleted(&self) -> bool {
        matches!(self, WordSpan::Deleted(_))
    }
}

/// A word with its trailing whitespace separator (13.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordPart {
    pub word: String,
    pub separator: String,
}

/// Splits text into alternating word and whitespace tokens. Each token is
/// either all non-whitespace (a word) or all whitespace (a separator).
/// Concatenating all tokens reproduces the original text.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    let mut current = String::new();
    let mut current_is_whitespace = match chars.peek() {
        Some(c) => c.is_whitespace(),
        None => return tokens,
    };

    for c in text.chars() {
        if c.is_whitespace() == current_is_whitespace {
            current.push(c);
        } else {
            tokens.push(std::mem::take(&mut current));
            current.push(c);
            current_is_whitespace = c.is_whitespace();
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Extracts word+separator pairs from a token list, skipping any leading
/// whitespace token (rare in paragraph text — callers that must preserve a
/// leading indent, e.g. Down-mode marker injection, split it out before
/// calling this).
pub fn extract_words(tokens: &[String]) -> Vec<WordPart> {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i].chars().all(char::is_whitespace) {
            i += 1;
            continue;
        }
        let word = tokens[i].clone();
        i += 1;
        let separator = if i < tokens.len() && tokens[i].chars().all(char::is_whitespace) {
            let sep = tokens[i].clone();
            i += 1;
            sep
        } else {
            String::new()
        };
        parts.push(WordPart { word, separator });
    }
    parts
}

/// Fraction of the longer side (old or new) that is unchanged text. Returns
/// `1.0` when both sides are empty.
pub fn similarity(spans: &[WordSpan]) -> f64 {
    let mut unchanged_len = 0usize;
    let mut deleted_len = 0usize;
    let mut inserted_len = 0usize;
    for span in spans {
        let len = span.text().chars().count();
        match span {
            WordSpan::Unchanged(_) => unchanged_len += len,
            WordSpan::Deleted(_) => deleted_len += len,
            WordSpan::Inserted(_) => inserted_len += len,
        }
    }
    let total = (unchanged_len + deleted_len).max(unchanged_len + inserted_len);
    if total == 0 {
        return 1.0;
    }
    unchanged_len as f64 / total as f64
}

/// True when `spans` contain word-level changes worth highlighting: at
/// least one non-unchanged span, and similarity at or above `threshold`.
pub fn has_significant_changes(spans: &[WordSpan], threshold: f64) -> bool {
    let has_changes = spans.iter().any(|s| !s.is_unchanged());
    has_changes && similarity(spans) >= threshold
}

/// Returns the run of leading spaces/tabs from `text` and the remainder.
/// Newlines are not considered indent.
fn split_leading_indent(text: &str) -> (&str, &str) {
    let end = text
        .char_indices()
        .find(|&(_, c)| c != ' ' && c != '\t')
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    text.split_at(end)
}

enum SpanKind {
    Unchanged,
    Inserted,
    Deleted,
}

impl SpanKind {
    fn span(&self, text: String) -> WordSpan {
        match self {
            SpanKind::Unchanged => WordSpan::Unchanged(text),
            SpanKind::Inserted => WordSpan::Inserted(text),
            SpanKind::Deleted => WordSpan::Deleted(text),
        }
    }
}

fn emit_word(part: &WordPart, kind: &SpanKind) -> Vec<WordSpan> {
    let mut spans = vec![kind.span(part.word.clone())];
    if !part.separator.is_empty() {
        spans.push(kind.span(part.separator.clone()));
    }
    spans
}

/// Computes a word-level diff between two plain-text strings.
///
/// The diff operates on words only — whitespace is tracked as separators
/// between words and emitted with the same type as its surrounding word.
/// Within each gap between unchanged anchors, all deletions are emitted
/// before all insertions (grouped style).
///
/// Concatenating all non-deleted spans reproduces `new`; concatenating all
/// non-inserted spans reproduces `old`.
pub fn diff(old: &str, new: &str) -> Vec<WordSpan> {
    let (old_lead, old_rest) = split_leading_indent(old);
    let (new_lead, new_rest) = split_leading_indent(new);

    let mut lead_spans = Vec::new();
    if old_lead == new_lead {
        if !old_lead.is_empty() {
            lead_spans.push(WordSpan::Unchanged(old_lead.to_string()));
        }
    } else {
        if !old_lead.is_empty() {
            lead_spans.push(WordSpan::Deleted(old_lead.to_string()));
        }
        if !new_lead.is_empty() {
            lead_spans.push(WordSpan::Inserted(new_lead.to_string()));
        }
    }

    let old_parts = extract_words(&tokenize(old_rest));
    let new_parts = extract_words(&tokenize(new_rest));

    if old_parts.is_empty() && new_parts.is_empty() {
        return lead_spans;
    }
    if old_parts.is_empty() {
        let mut result = lead_spans;
        for p in &new_parts {
            result.extend(emit_word(p, &SpanKind::Inserted));
        }
        return result;
    }
    if new_parts.is_empty() {
        let mut result = lead_spans;
        for p in &old_parts {
            result.extend(emit_word(p, &SpanKind::Deleted));
        }
        return result;
    }

    // Diff on word content only (whitespace is not diffed).
    let old_words: Vec<&str> = old_parts.iter().map(|p| p.word.as_str()).collect();
    let new_words: Vec<&str> = new_parts.iter().map(|p| p.word.as_str()).collect();
    let (removed_old, inserted_new) = crate::lcs::diff_indices(&old_words, &new_words);

    // Find unchanged pairs (anchors).
    let mut anchors: Vec<(usize, usize)> = Vec::new();
    let mut oi = 0usize;
    let mut ni = 0usize;
    while oi < old_parts.len() && ni < new_parts.len() {
        if removed_old.contains(&oi) {
            oi += 1;
            continue;
        }
        if inserted_new.contains(&ni) {
            ni += 1;
            continue;
        }
        anchors.push((oi, ni));
        oi += 1;
        ni += 1;
    }

    let mut result = lead_spans;

    let mut boundaries: Vec<(isize, isize)> = vec![(-1, -1)];
    boundaries.extend(anchors.iter().map(|&(o, n)| (o as isize, n as isize)));
    boundaries.push((old_parts.len() as isize, new_parts.len() as isize));

    for i in 0..(boundaries.len() - 1) {
        let (prev_old, prev_new) = boundaries[i];
        let (next_old, next_new) = boundaries[i + 1];

        let del_indices: Vec<usize> = ((prev_old + 1)..next_old)
            .map(|x| x as usize)
            .filter(|x| removed_old.contains(x))
            .collect();
        let ins_indices: Vec<usize> = ((prev_new + 1)..next_new)
            .map(|x| x as usize)
            .filter(|x| inserted_new.contains(x))
            .collect();
        let has_anchor = i + 1 < boundaries.len() - 1;
        let is_substitution = !del_indices.is_empty() && !ins_indices.is_empty();

        for (j, &oi) in del_indices.iter().enumerate() {
            let is_last = j == del_indices.len() - 1;
            if is_last && has_anchor && is_substitution {
                result.push(WordSpan::Deleted(old_parts[oi].word.clone()));
            } else {
                result.extend(emit_word(&old_parts[oi], &SpanKind::Deleted));
            }
        }

        for (j, &ni) in ins_indices.iter().enumerate() {
            let is_last = j == ins_indices.len() - 1;
            if is_last && has_anchor && is_substitution {
                result.push(WordSpan::Inserted(new_parts[ni].word.clone()));
            } else {
                result.extend(emit_word(&new_parts[ni], &SpanKind::Inserted));
            }
        }

        if has_anchor {
            let (_, next_new_usize) = (next_old as usize, next_new as usize);
            if is_substitution {
                if let Some(&ni) = ins_indices.last() {
                    let sep = &new_parts[ni].separator;
                    if !sep.is_empty() {
                        result.push(WordSpan::Unchanged(sep.clone()));
                    }
                }
            }

            result.extend(emit_word(&new_parts[next_new_usize], &SpanKind::Unchanged));

            let next_old_usize = next_old as usize;
            let old_sep = &old_parts[next_old_usize].separator;
            let new_sep = &new_parts[next_new_usize].separator;
            let old_sep_len = old_sep.chars().count();
            let new_sep_len = new_sep.chars().count();
            if old_sep_len > new_sep_len {
                let excess: String = old_sep.chars().skip(new_sep_len).collect();
                result.push(WordSpan::Deleted(excess));
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reconstruct_new(spans: &[WordSpan]) -> String {
        spans
            .iter()
            .filter(|s| !s.is_deleted())
            .map(|s| s.text())
            .collect()
    }

    fn reconstruct_old(spans: &[WordSpan]) -> String {
        spans
            .iter()
            .filter(|s| !s.is_inserted())
            .map(|s| s.text())
            .collect()
    }

    // --- tokenize ---

    #[test]
    fn tokenize_empty_string() {
        assert_eq!(tokenize(""), Vec::<String>::new());
    }

    #[test]
    fn tokenize_whitespace_only() {
        assert_eq!(tokenize("   "), vec!["   "]);
    }

    #[test]
    fn tokenize_single_word() {
        assert_eq!(tokenize("hello"), vec!["hello"]);
    }

    #[test]
    fn tokenize_leading_and_trailing_whitespace_preserved() {
        assert_eq!(tokenize(" hi "), vec![" ", "hi", " "]);
    }

    #[test]
    fn tokenize_alternates_word_and_whitespace() {
        assert_eq!(
            tokenize("the quick fox"),
            vec!["the", " ", "quick", " ", "fox"]
        );
    }

    #[test]
    fn tokenize_punctuation_adjacent_words() {
        assert_eq!(tokenize("hello, world!"), vec!["hello,", " ", "world!"]);
    }

    // --- extract_words ---

    #[test]
    fn extract_words_empty() {
        assert_eq!(extract_words(&[]), Vec::<WordPart>::new());
    }

    #[test]
    fn extract_words_single_word_no_separator() {
        assert_eq!(
            extract_words(&tokenize("hello")),
            vec![WordPart {
                word: "hello".into(),
                separator: "".into()
            }]
        );
    }

    #[test]
    fn extract_words_word_with_separator() {
        assert_eq!(
            extract_words(&tokenize("the quick")),
            vec![
                WordPart {
                    word: "the".into(),
                    separator: " ".into()
                },
                WordPart {
                    word: "quick".into(),
                    separator: "".into()
                },
            ]
        );
    }

    #[test]
    fn extract_words_skips_leading_whitespace_token() {
        assert_eq!(
            extract_words(&tokenize("  hi")),
            vec![WordPart {
                word: "hi".into(),
                separator: "".into()
            }]
        );
    }

    // --- diff ---

    #[test]
    fn identical_strings() {
        let spans = diff("hello world", "hello world");
        assert!(spans.iter().all(|s| s.is_unchanged()));
        assert_eq!(reconstruct_new(&spans), "hello world");
    }

    #[test]
    fn single_word_changed() {
        let spans = diff("the quick fox", "the slow fox");
        assert_eq!(
            spans,
            vec![
                WordSpan::Unchanged("the".into()),
                WordSpan::Unchanged(" ".into()),
                WordSpan::Deleted("quick".into()),
                WordSpan::Inserted("slow".into()),
                WordSpan::Unchanged(" ".into()),
                WordSpan::Unchanged("fox".into()),
            ]
        );
    }

    #[test]
    fn word_added_in_middle() {
        let spans = diff("the fox", "the brown fox");
        assert_eq!(
            spans,
            vec![
                WordSpan::Unchanged("the".into()),
                WordSpan::Unchanged(" ".into()),
                WordSpan::Inserted("brown".into()),
                WordSpan::Inserted(" ".into()),
                WordSpan::Unchanged("fox".into()),
            ]
        );
    }

    #[test]
    fn word_removed_from_middle() {
        let spans = diff("the brown fox", "the fox");
        assert_eq!(
            spans,
            vec![
                WordSpan::Unchanged("the".into()),
                WordSpan::Unchanged(" ".into()),
                WordSpan::Deleted("brown".into()),
                WordSpan::Deleted(" ".into()),
                WordSpan::Unchanged("fox".into()),
            ]
        );
    }

    #[test]
    fn multiple_changes() {
        let spans = diff("the quick brown fox jumps", "the slow brown dog leaps");
        assert_eq!(reconstruct_old(&spans), "the quick brown fox jumps");
        assert_eq!(reconstruct_new(&spans), "the slow brown dog leaps");
        let deleted_words = spans
            .iter()
            .filter(|s| s.is_deleted() && !s.text().chars().all(char::is_whitespace))
            .count();
        let inserted_words = spans
            .iter()
            .filter(|s| s.is_inserted() && !s.text().chars().all(char::is_whitespace))
            .count();
        assert_eq!(deleted_words, 3);
        assert_eq!(inserted_words, 3);
    }

    #[test]
    fn completely_different() {
        let spans = diff("alpha beta", "gamma delta");
        assert_eq!(reconstruct_old(&spans), "alpha beta");
        assert_eq!(reconstruct_new(&spans), "gamma delta");
    }

    #[test]
    fn empty_old() {
        let spans = diff("", "hello world");
        assert!(spans.iter().all(|s| s.is_inserted()));
        assert_eq!(reconstruct_new(&spans), "hello world");
    }

    #[test]
    fn empty_new() {
        let spans = diff("hello world", "");
        assert!(spans.iter().all(|s| s.is_deleted()));
        assert_eq!(reconstruct_old(&spans), "hello world");
    }

    #[test]
    fn both_empty() {
        assert!(diff("", "").is_empty());
    }

    #[test]
    fn whitespace_preservation() {
        let spans = diff("one two three", "one changed three");
        assert_eq!(reconstruct_old(&spans), "one two three");
        assert_eq!(reconstruct_new(&spans), "one changed three");
    }

    #[test]
    fn multiple_consecutive_spaces() {
        let spans = diff("hello  world", "hello  earth");
        assert_eq!(reconstruct_old(&spans), "hello  world");
        assert_eq!(reconstruct_new(&spans), "hello  earth");
        assert!(spans.iter().any(|s| s.is_deleted()));
        assert!(spans.iter().any(|s| s.is_inserted()));
    }

    #[test]
    fn words_removed_from_end() {
        let spans = diff("hello world end", "hello world");
        assert_eq!(reconstruct_new(&spans), "hello world");
        assert_eq!(reconstruct_old(&spans), "hello world end");
        let deleted_words: Vec<&WordSpan> = spans
            .iter()
            .filter(|s| s.is_deleted() && !s.text().chars().all(char::is_whitespace))
            .collect();
        assert_eq!(deleted_words.len(), 1);
        assert_eq!(deleted_words[0].text(), "end");
    }

    #[test]
    fn leading_indent_kept_as_its_own_span_when_unchanged() {
        let spans = diff("  the fox", "  the dog");
        assert_eq!(spans[0], WordSpan::Unchanged("  ".into()));
    }

    #[test]
    fn leading_indent_changed_emits_deleted_then_inserted() {
        let spans = diff("  the fox", "    the fox");
        assert_eq!(spans[0], WordSpan::Deleted("  ".into()));
        assert_eq!(spans[1], WordSpan::Inserted("    ".into()));
    }

    // --- similarity ---

    #[test]
    fn similarity_all_unchanged() {
        let spans = diff("hello world", "hello world");
        assert_eq!(similarity(&spans), 1.0);
    }

    #[test]
    fn similarity_completely_different() {
        let spans = vec![
            WordSpan::Deleted("alpha".into()),
            WordSpan::Deleted(" ".into()),
            WordSpan::Inserted("gamma".into()),
            WordSpan::Inserted(" ".into()),
            WordSpan::Deleted("beta".into()),
            WordSpan::Inserted("delta".into()),
        ];
        assert_eq!(similarity(&spans), 0.0);
    }

    #[test]
    fn similarity_empty_spans() {
        assert_eq!(similarity(&[]), 1.0);
    }

    #[test]
    fn similarity_mixed_case() {
        let spans = diff("the quick fox", "the slow fox");
        let sim = similarity(&spans);
        assert!(sim > 0.6);
        assert!(sim < 0.7);
    }

    #[test]
    fn similarity_all_deleted() {
        let spans = diff("hello world", "");
        assert_eq!(similarity(&spans), 0.0);
    }

    #[test]
    fn similarity_all_inserted() {
        let spans = diff("", "hello world");
        assert_eq!(similarity(&spans), 0.0);
    }

    // --- has_significant_changes ---

    #[test]
    fn significant_changes_all_unchanged() {
        let spans = diff("hello world", "hello world");
        assert!(!has_significant_changes(&spans, 0.25));
    }

    #[test]
    fn significant_changes_below_threshold() {
        let spans = diff("the quick brown fox jumps", "the slow red dog leaps");
        assert!(!has_significant_changes(&spans, 0.25));
    }

    #[test]
    fn significant_changes_above_threshold() {
        let spans = diff("the quick fox", "the slow fox");
        assert!(has_significant_changes(&spans, 0.25));
    }

    #[test]
    fn significant_changes_respects_custom_threshold() {
        let spans = diff("the quick fox", "the slow fox");
        assert!(has_significant_changes(&spans, 0.5));
        assert!(!has_significant_changes(&spans, 0.7));
    }

    #[test]
    fn similarity_threshold_boundary_is_inclusive() {
        // Two spans, one word of two unchanged -> similarity exactly 0.5.
        let spans = vec![
            WordSpan::Unchanged("aa".into()),
            WordSpan::Deleted("bb".into()),
        ];
        assert_eq!(similarity(&spans), 0.5);
        assert!(has_significant_changes(&spans, 0.5));
        assert!(!has_significant_changes(&spans, 0.51));
    }
}

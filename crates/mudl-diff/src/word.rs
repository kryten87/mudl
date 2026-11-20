//! Word-level diffing (`WordDiff.swift`; plan §7.2).
//!
//! Text is split into words and the whitespace after each word; only the
//! words are diffed, and whitespace rides along with its word. The result is
//! a list of [`WordSpan`]s that reproduces both sides exactly: concatenating
//! the non-deleted spans gives `new`, the non-inserted spans give `old`.
//!
//! Lengths are counted in `char`s where Swift counts grapheme clusters. The
//! two agree except inside multi-scalar clusters (combining marks, ZWJ
//! emoji), where a similarity ratio may differ in the last decimal places —
//! not enough to matter for a 0.25 threshold, and not worth a Unicode
//! segmentation dependency.

use std::ops::Range;

use crate::myers;

/// A span in a word-level diff result.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WordSpan {
    Unchanged(String),
    Inserted(String),
    Deleted(String),
}

impl WordSpan {
    /// The span's text, whatever its kind.
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

/// A word with its trailing whitespace separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordPart {
    pub word: String,
    pub separator: String,
}

/// Splits text into alternating word and whitespace tokens, using
/// `char::is_whitespace`. Each token is all-whitespace or all-non-whitespace;
/// concatenating the tokens reproduces `text`.
pub fn tokenize(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut current_is_ws = match text.chars().next() {
        Some(c) => c.is_whitespace(),
        None => return tokens,
    };
    for (i, c) in text.char_indices() {
        if c.is_whitespace() != current_is_ws {
            tokens.push(&text[start..i]);
            start = i;
            current_is_ws = !current_is_ws;
        }
    }
    tokens.push(&text[start..]);
    tokens
}

/// Pairs each word token with the whitespace token after it (or `""`).
/// A leading whitespace token is skipped; [`diff`] splits the leading
/// indent off beforehand so nothing is lost.
pub fn extract_words(tokens: &[&str]) -> Vec<WordPart> {
    let is_ws = |t: &str| t.chars().all(char::is_whitespace);
    let mut parts = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if is_ws(tokens[i]) {
            i += 1;
            continue;
        }
        let word = tokens[i].to_string();
        i += 1;
        let separator = if i < tokens.len() && is_ws(tokens[i]) {
            i += 1;
            tokens[i - 1].to_string()
        } else {
            String::new()
        };
        parts.push(WordPart { word, separator });
    }
    parts
}

/// Fraction of the longer side (old or new) that is unchanged text:
/// `unchanged / max(unchanged + deleted, unchanged + inserted)`. Two empty
/// sides give 1.0.
pub fn similarity(spans: &[WordSpan]) -> f64 {
    let (mut unchanged, mut deleted, mut inserted) = (0usize, 0usize, 0usize);
    for span in spans {
        let len = span.text().chars().count();
        match span {
            WordSpan::Unchanged(_) => unchanged += len,
            WordSpan::Deleted(_) => deleted += len,
            WordSpan::Inserted(_) => inserted += len,
        }
    }
    let total = (unchanged + deleted).max(unchanged + inserted);
    if total == 0 {
        return 1.0;
    }
    unchanged as f64 / total as f64
}

/// True when `spans` hold changes worth highlighting: at least one changed
/// span, and a similarity at or above `threshold` (inclusive, as in `mud`).
/// Below the threshold a word diff is mostly noise, so callers show the
/// blocks whole instead.
pub fn has_significant_changes(spans: &[WordSpan], threshold: f64) -> bool {
    spans.iter().any(|s| !s.is_unchanged()) && similarity(spans) >= threshold
}

/// Splits off the leading whitespace run.
///
/// Swift splits off spaces and tabs only, and its word extraction then
/// silently drops any other leading whitespace (a newline, a non-breaking
/// space), breaking reconstruction. Taking the whole run is identical on
/// every input Swift handles correctly.
fn split_leading_indent(text: &str) -> (&str, &str) {
    let end = text.len() - text.trim_start_matches(char::is_whitespace).len();
    text.split_at(end)
}

#[derive(Clone, Copy)]
enum Kind {
    Unchanged,
    Inserted,
    Deleted,
}

impl Kind {
    fn span(self, text: &str) -> WordSpan {
        let text = text.to_string();
        match self {
            Kind::Unchanged => WordSpan::Unchanged(text),
            Kind::Inserted => WordSpan::Inserted(text),
            Kind::Deleted => WordSpan::Deleted(text),
        }
    }
}

fn emit_word(out: &mut Vec<WordSpan>, part: &WordPart, kind: Kind) {
    out.push(kind.span(&part.word));
    if !part.separator.is_empty() {
        out.push(kind.span(&part.separator));
    }
}

/// Computes a word-level diff between two plain-text strings.
///
/// Within each gap between unchanged words, deletions precede insertions.
/// Separator rules (all from `WordDiff.swift`):
///
/// - A leading indent is its own span (unchanged, or deleted + inserted), so
///   callers tracking positions through the result stay aligned.
/// - In a pure deletion or insertion gap, words keep their separators.
/// - In a substitution gap followed by an anchor, the last word on each side
///   gives up its separator; the new side's separator is emitted once as
///   unchanged before the anchor.
/// - If the old anchor's separator is longer than the new one's, the excess
///   is emitted as deleted after the anchor.
///
/// **Deviation from `mud`:** Swift only guarantees the `new` reconstruction.
/// Where separators differ between the sides (a word appended after the old
/// last word, a substitution next to a changed whitespace run) its spans
/// rebuild `old` with the wrong whitespace, which would desync any cursor
/// walking the deleted block's text. Here both reconstructions always hold:
/// the shared separator is only handed to the anchor when both sides agree,
/// and an anchor separator that grew is split into unchanged + inserted. On
/// every input where Swift's output already reconstructs `old`, the output
/// is identical to Swift's.
pub fn diff(old: &str, new: &str) -> Vec<WordSpan> {
    let (old_lead, old_rest) = split_leading_indent(old);
    let (new_lead, new_rest) = split_leading_indent(new);

    let mut result = Vec::new();
    if old_lead == new_lead {
        if !old_lead.is_empty() {
            result.push(Kind::Unchanged.span(old_lead));
        }
    } else {
        if !old_lead.is_empty() {
            result.push(Kind::Deleted.span(old_lead));
        }
        if !new_lead.is_empty() {
            result.push(Kind::Inserted.span(new_lead));
        }
    }

    let old_parts = extract_words(&tokenize(old_rest));
    let new_parts = extract_words(&tokenize(new_rest));

    if old_parts.is_empty() {
        for p in &new_parts {
            emit_word(&mut result, p, Kind::Inserted);
        }
        return result;
    }
    if new_parts.is_empty() {
        for p in &old_parts {
            emit_word(&mut result, p, Kind::Deleted);
        }
        return result;
    }

    let old_words: Vec<&str> = old_parts.iter().map(|p| p.word.as_str()).collect();
    let new_words: Vec<&str> = new_parts.iter().map(|p| p.word.as_str()).collect();

    for seg in myers::segments(&old_words, &new_words) {
        let has_anchor = seg.anchor.is_some();
        let is_substitution = !seg.deleted.is_empty() && !seg.inserted.is_empty();
        let last_del = seg.deleted.end.saturating_sub(1);
        let last_ins = seg.inserted.end.saturating_sub(1);
        // `mud` hands the substitution's trailing separator to the anchor,
        // which is only faithful to `old` when both sides' separators match.
        // Otherwise each side keeps its own (see the module docs).
        let share_separator = has_anchor
            && is_substitution
            && old_parts[last_del].separator == new_parts[last_ins].separator;

        for oi in seg.deleted.clone() {
            if oi == last_del && share_separator {
                result.push(Kind::Deleted.span(&old_parts[oi].word));
            } else {
                emit_word(&mut result, &old_parts[oi], Kind::Deleted);
            }
        }
        for ni in seg.inserted.clone() {
            if ni == last_ins && share_separator {
                result.push(Kind::Inserted.span(&new_parts[ni].word));
            } else {
                emit_word(&mut result, &new_parts[ni], Kind::Inserted);
            }
        }

        if let Some((next_old, next_new)) = seg.anchor {
            if share_separator {
                let sep = &new_parts[last_ins].separator;
                if !sep.is_empty() {
                    result.push(Kind::Unchanged.span(sep));
                }
            }
            result.push(Kind::Unchanged.span(&new_parts[next_new].word));
            emit_anchor_separator(
                &mut result,
                &old_parts[next_old].separator,
                &new_parts[next_new].separator,
            );
        }
    }
    result
}

/// Emits the separator after an anchor word so both sides reconstruct.
///
/// When the new separator is a prefix of the old one (the usual case: equal,
/// or a word moved to the end), this is exactly `mud`'s output — the new
/// separator unchanged, the old excess deleted. Otherwise the shared prefix
/// stays unchanged and the remainders become deleted/inserted.
fn emit_anchor_separator(out: &mut Vec<WordSpan>, old_sep: &str, new_sep: &str) {
    if let Some(excess) = old_sep.strip_prefix(new_sep) {
        if !new_sep.is_empty() {
            out.push(Kind::Unchanged.span(new_sep));
        }
        if !excess.is_empty() {
            out.push(Kind::Deleted.span(excess));
        }
    } else if let Some(extra) = new_sep.strip_prefix(old_sep) {
        if !old_sep.is_empty() {
            out.push(Kind::Unchanged.span(old_sep));
        }
        out.push(Kind::Inserted.span(extra));
    } else {
        out.push(Kind::Deleted.span(old_sep));
        out.push(Kind::Inserted.span(new_sep));
    }
}

/// Byte ranges of the changed spans on each side of a diff: deleted spans
/// as ranges into `old`, inserted spans as ranges into `new`.
///
/// One range per span, never merged — the same markers `CodeBlockDiff`
/// and `DownHTMLVisitor` inject. Byte offsets are what Rust renderers slice
/// with; Down mode converts them to UTF-16 for the browser.
pub fn change_ranges(spans: &[WordSpan]) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (mut old_pos, mut new_pos) = (0usize, 0usize);
    let (mut deleted, mut inserted) = (Vec::new(), Vec::new());
    for span in spans {
        let len = span.text().len();
        match span {
            WordSpan::Unchanged(_) => {
                old_pos += len;
                new_pos += len;
            }
            WordSpan::Deleted(_) => {
                deleted.push(old_pos..old_pos + len);
                old_pos += len;
            }
            WordSpan::Inserted(_) => {
                inserted.push(new_pos..new_pos + len);
                new_pos += len;
            }
        }
    }
    (deleted, inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use WordSpan::{Deleted as D, Inserted as I, Unchanged as U};

    fn s(t: &str) -> String {
        t.to_string()
    }

    fn reconstruct_new(spans: &[WordSpan]) -> String {
        spans
            .iter()
            .filter(|s| !s.is_deleted())
            .map(WordSpan::text)
            .collect()
    }

    fn reconstruct_old(spans: &[WordSpan]) -> String {
        spans
            .iter()
            .filter(|s| !s.is_inserted())
            .map(WordSpan::text)
            .collect()
    }

    fn is_word(span: &&WordSpan) -> bool {
        !span.text().chars().all(char::is_whitespace)
    }

    mod tokenize_tests {
        use super::*;

        #[test]
        fn empty_string() {
            assert!(tokenize("").is_empty());
        }

        #[test]
        fn whitespace_only() {
            assert_eq!(tokenize("   "), vec!["   "]);
        }

        #[test]
        fn alternates_word_and_whitespace() {
            assert_eq!(
                tokenize("the quick fox"),
                vec!["the", " ", "quick", " ", "fox"]
            );
        }

        #[test]
        fn leading_and_trailing_whitespace_preserved() {
            assert_eq!(tokenize(" hi "), vec![" ", "hi", " "]);
        }

        #[test]
        fn punctuation_stays_with_words() {
            assert_eq!(tokenize("hello, world!"), vec!["hello,", " ", "world!"]);
        }

        #[test]
        fn unicode_whitespace_separates() {
            assert_eq!(tokenize("a\u{a0}b"), vec!["a", "\u{a0}", "b"]);
        }

        #[test]
        fn extract_words_pairs_separators_and_skips_leading_whitespace() {
            assert_eq!(
                extract_words(&tokenize("  the quick")),
                vec![
                    WordPart {
                        word: s("the"),
                        separator: s(" ")
                    },
                    WordPart {
                        word: s("quick"),
                        separator: s("")
                    },
                ]
            );
        }
    }

    /// Ported from `WordDiffTests.swift` ("Diff algorithm").
    mod diff_tests {
        use super::*;

        #[test]
        fn identical_strings() {
            let spans = diff("hello world", "hello world");
            assert!(spans.iter().all(WordSpan::is_unchanged));
            assert_eq!(reconstruct_new(&spans), "hello world");
        }

        #[test]
        fn single_word_changed() {
            assert_eq!(
                diff("the quick fox", "the slow fox"),
                vec![
                    U(s("the")),
                    U(s(" ")),
                    D(s("quick")),
                    I(s("slow")),
                    U(s(" ")),
                    U(s("fox")),
                ]
            );
        }

        #[test]
        fn word_added_in_middle() {
            assert_eq!(
                diff("the fox", "the brown fox"),
                vec![
                    U(s("the")),
                    U(s(" ")),
                    I(s("brown")),
                    I(s(" ")),
                    U(s("fox"))
                ]
            );
        }

        #[test]
        fn word_removed_from_middle() {
            assert_eq!(
                diff("the brown fox", "the fox"),
                vec![
                    U(s("the")),
                    U(s(" ")),
                    D(s("brown")),
                    D(s(" ")),
                    U(s("fox"))
                ]
            );
        }

        #[test]
        fn multiple_changes() {
            let spans = diff("the quick brown fox jumps", "the slow brown dog leaps");
            assert_eq!(reconstruct_old(&spans), "the quick brown fox jumps");
            assert_eq!(reconstruct_new(&spans), "the slow brown dog leaps");
            let deleted = spans.iter().filter(|s| s.is_deleted()).filter(is_word);
            let inserted = spans.iter().filter(|s| s.is_inserted()).filter(is_word);
            assert_eq!(deleted.count(), 3);
            assert_eq!(inserted.count(), 3);
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
            assert!(spans.iter().all(WordSpan::is_inserted));
            assert_eq!(reconstruct_new(&spans), "hello world");
        }

        #[test]
        fn empty_new() {
            let spans = diff("hello world", "");
            assert!(spans.iter().all(WordSpan::is_deleted));
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
            assert!(spans.iter().any(WordSpan::is_deleted));
            assert!(spans.iter().any(WordSpan::is_inserted));
        }

        #[test]
        fn words_removed_from_end() {
            let spans = diff("hello world end", "hello world");
            assert_eq!(reconstruct_new(&spans), "hello world");
            assert_eq!(reconstruct_old(&spans), "hello world end");
            let deleted: Vec<&WordSpan> = spans
                .iter()
                .filter(|s| s.is_deleted())
                .filter(is_word)
                .collect();
            assert_eq!(deleted.len(), 1);
            assert_eq!(deleted[0].text(), "end");
        }

        #[test]
        fn excess_old_separator_is_emitted_as_deleted() {
            // "b" moves from the middle (separator " ") to the end (none).
            assert_eq!(
                diff("a b c", "a b"),
                vec![U(s("a")), U(s(" ")), U(s("b")), D(s(" ")), D(s("c")),]
            );
        }

        #[test]
        fn word_appended_marks_the_new_separator_inserted() {
            // Deviation from mud (which emits the " " as unchanged and so
            // rebuilds old as "hello world ").
            assert_eq!(
                diff("hello world", "hello world end"),
                vec![
                    U(s("hello")),
                    U(s(" ")),
                    U(s("world")),
                    I(s(" ")),
                    I(s("end")),
                ]
            );
        }

        #[test]
        fn substitution_with_differing_separators_keeps_each_side_whole() {
            assert_eq!(
                diff("a x  c", "a y c"),
                vec![
                    U(s("a")),
                    U(s(" ")),
                    D(s("x")),
                    D(s("  ")),
                    I(s("y")),
                    I(s(" ")),
                    U(s("c")),
                ]
            );
        }

        #[test]
        fn leading_indent_kept_as_its_own_span_when_unchanged() {
            assert_eq!(diff("  the fox", "  the dog")[0], U(s("  ")));
        }

        #[test]
        fn leading_indent_changed_emits_deleted_then_inserted() {
            let spans = diff("  the fox", "    the fox");
            assert_eq!(spans[0], D(s("  ")));
            assert_eq!(spans[1], I(s("    ")));
        }
    }

    /// Ported from `WordDiffTests.swift` ("Similarity" and
    /// "hasSignificantChanges").
    mod similarity_tests {
        use super::*;

        #[test]
        fn all_unchanged() {
            assert_eq!(similarity(&diff("hello world", "hello world")), 1.0);
        }

        #[test]
        fn completely_different() {
            let spans = vec![
                D(s("alpha")),
                D(s(" ")),
                I(s("gamma")),
                I(s(" ")),
                D(s("beta")),
                I(s("delta")),
            ];
            assert_eq!(similarity(&spans), 0.0);
        }

        #[test]
        fn empty_spans() {
            assert_eq!(similarity(&[]), 1.0);
        }

        #[test]
        fn mixed_case() {
            // unchanged 8, old total 13 → ≈ 0.615.
            let sim = similarity(&diff("the quick fox", "the slow fox"));
            assert!(sim > 0.6 && sim < 0.7);
        }

        #[test]
        fn all_deleted() {
            assert_eq!(similarity(&diff("hello world", "")), 0.0);
        }

        #[test]
        fn all_inserted() {
            assert_eq!(similarity(&diff("", "hello world")), 0.0);
        }

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
        fn threshold_boundary_is_inclusive() {
            let spans = vec![U(s("aa")), D(s("bb"))];
            assert_eq!(similarity(&spans), 0.5);
            assert!(has_significant_changes(&spans, 0.5));
            assert!(!has_significant_changes(&spans, 0.51));
        }
    }

    mod change_ranges_tests {
        use super::*;

        #[test]
        fn ranges_index_each_side() {
            let (del, ins) = change_ranges(&diff("let y = 2", "let z = 3"));
            assert_eq!(del, vec![4..5, 8..9]);
            assert_eq!(ins, vec![4..5, 8..9]);
        }

        #[test]
        fn ranges_are_byte_offsets() {
            let (del, ins) = change_ranges(&diff("é old", "é new"));
            assert_eq!(&"é old"[del[0].clone()], "old");
            assert_eq!(&"é new"[ins[0].clone()], "new");
        }
    }

    /// The reconstruction invariant over generated inputs:
    /// unchanged + inserted == new and unchanged + deleted == old.
    mod property_tests {
        use super::*;

        #[test]
        fn spans_reconstruct_both_sides() {
            let pieces = ["a", "b", "cc", " ", "  ", "\t", "\n", "é", "  x"];
            let mut state = 12345u64;
            let mut next = || {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 33) as usize
            };
            for _ in 0..3000 {
                let mut sides = [String::new(), String::new()];
                for side in &mut sides {
                    for _ in 0..next() % 9 {
                        side.push_str(pieces[next() % pieces.len()]);
                    }
                }
                let [old, new] = sides;
                let spans = diff(&old, &new);
                assert_eq!(reconstruct_old(&spans), old, "old {old:?} new {new:?}");
                assert_eq!(reconstruct_new(&spans), new, "old {old:?} new {new:?}");
            }
        }
    }
}

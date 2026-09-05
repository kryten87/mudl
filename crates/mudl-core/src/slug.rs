use std::collections::HashMap;

pub fn slugify(text: &str) -> String {
    let lowered: String = text.chars().flat_map(|c| c.to_lowercase()).collect();

    let filtered: String = lowered
        .chars()
        .filter(|&c| is_word_char(c) || c.is_whitespace() || c == '-')
        .collect();

    let mut result = String::with_capacity(filtered.len());
    let mut in_space_run = false;
    for c in filtered.trim().chars() {
        if c.is_whitespace() {
            if !in_space_run {
                result.push('-');
                in_space_run = true;
            }
        } else {
            result.push(c);
            in_space_run = false;
        }
    }
    result
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub struct Tracker {
    counts: HashMap<String, usize>,
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            counts: HashMap::new(),
        }
    }

    pub fn track(&mut self, text: &str) -> String {
        let base = slugify(text);
        let n = *self.counts.get(&base).unwrap_or(&0);
        self.counts.insert(base.clone(), n + 1);
        if n == 0 {
            base
        } else {
            format!("{}-{}", base, n)
        }
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod slugify_tests {
    use super::slugify;

    #[test]
    fn plain_text() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn punctuation_stripped() {
        assert_eq!(slugify("What's new?"), "whats-new");
    }

    #[test]
    fn leading_trailing_spaces_trimmed() {
        assert_eq!(slugify("  hello  "), "hello");
    }

    #[test]
    fn multiple_spaces_collapsed() {
        assert_eq!(slugify("a  b"), "a-b");
    }

    #[test]
    fn unicode_preserved() {
        assert_eq!(slugify("Ñoño"), "ñoño");
    }

    #[test]
    fn empty_string() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn already_slugged() {
        assert_eq!(slugify("hello-world"), "hello-world");
    }

    #[test]
    fn hyphens_preserved() {
        // "A - B" lowercases to "a - b"; the two literal spaces each
        // collapse to a hyphen, and the literal hyphen between them
        // is preserved as-is, so three consecutive hyphens result.
        // This matches the original Swift implementation exactly.
        assert_eq!(slugify("A - B"), "a---b");
    }

    #[test]
    fn numbers_preserved() {
        assert_eq!(slugify("Section 42"), "section-42");
    }
}

#[cfg(test)]
mod tracker_tests {
    use super::Tracker;

    #[test]
    fn first_occurrence_bare() {
        let mut tracker = Tracker::new();
        assert_eq!(tracker.track("Features"), "features");
    }

    #[test]
    fn duplicates_get_suffix() {
        let mut tracker = Tracker::new();
        assert_eq!(tracker.track("Features"), "features");
        assert_eq!(tracker.track("Features"), "features-1");
        assert_eq!(tracker.track("Features"), "features-2");
    }

    #[test]
    fn distinct_headings_unaffected() {
        let mut tracker = Tracker::new();
        assert_eq!(tracker.track("Alpha"), "alpha");
        assert_eq!(tracker.track("Beta"), "beta");
        assert_eq!(tracker.track("Alpha"), "alpha-1");
        assert_eq!(tracker.track("Gamma"), "gamma");
    }
}

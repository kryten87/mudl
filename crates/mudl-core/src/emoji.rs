#[path = "emoji_data.rs"]
mod emoji_data;

use emoji_data::EMOJI_SHORTCODES;

fn is_shortcode_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'+' || b == b'-'
}

fn shortcode_end(bytes: &[u8], colon_pos: usize) -> Option<usize> {
    let mut j = colon_pos + 1;
    let mut has_content = false;
    while j < bytes.len() {
        match bytes[j] {
            b':' if has_content => return Some(j + 1),
            b':' => return None,
            b if is_shortcode_char(b) => {
                has_content = true;
                j += 1;
            }
            _ => return None,
        }
    }
    None
}

fn lookup(alias: &str) -> Option<&'static str> {
    EMOJI_SHORTCODES
        .iter()
        .find(|(shortcode, _)| *shortcode == alias)
        .map(|(_, emoji)| *emoji)
}

pub fn replace_shortcodes(text: &str) -> String {
    if !text.contains(':') {
        return text.to_string();
    }

    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            if let Some(match_end) = shortcode_end(bytes, i) {
                let alias = &text[i + 1..match_end - 1];
                result.push_str(&text[last_end..i]);
                match lookup(alias) {
                    Some(emoji) => result.push_str(emoji),
                    None => result.push_str(&text[i..match_end]),
                }
                last_end = match_end;
                i = match_end;
                continue;
            }
        }
        i += 1;
    }
    result.push_str(&text[last_end..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_shortcode() {
        assert_eq!(replace_shortcodes(":rocket:"), "🚀");
    }

    #[test]
    fn shortcode_with_special_chars() {
        assert_eq!(replace_shortcodes(":+1:"), "👍");
        assert_eq!(replace_shortcodes(":t-rex:"), "🦖");
    }

    #[test]
    fn unknown_shortcode_left_unchanged() {
        let input = ":not_a_real_shortcode:";
        assert_eq!(replace_shortcodes(input), input);
    }

    #[test]
    fn no_colons_takes_fast_path() {
        assert_eq!(replace_shortcodes("hello world"), "hello world");
    }

    #[test]
    fn mixed_known_unknown_and_plain_text() {
        let result = replace_shortcodes("I gave this a :+1: because it was :fire: but not :nope:");
        assert_eq!(result, "I gave this a 👍 because it was 🔥 but not :nope:");
    }

    #[test]
    fn consecutive_shortcodes() {
        assert_eq!(replace_shortcodes(":smile::+1:"), "😄👍");
    }

    #[test]
    fn empty_between_colons_is_not_a_match() {
        assert_eq!(replace_shortcodes("::"), "::");
    }

    #[test]
    fn colon_heavy_non_shortcode_text_is_untouched() {
        assert_eq!(replace_shortcodes("10:30:00"), "10:30:00");
        assert_eq!(replace_shortcodes("12:30:00"), "12:30:00");
    }
}

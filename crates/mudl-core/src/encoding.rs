pub fn html_escape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            _ => result.push(c),
        }
    }
    result
}

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut chunks = bytes.chunks_exact(3);
    for chunk in &mut chunks {
        let n = (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8 | chunk[2] as u32;
        result.push(BASE64_TABLE[(n >> 18 & 0x3F) as usize] as char);
        result.push(BASE64_TABLE[(n >> 12 & 0x3F) as usize] as char);
        result.push(BASE64_TABLE[(n >> 6 & 0x3F) as usize] as char);
        result.push(BASE64_TABLE[(n & 0x3F) as usize] as char);
    }
    let remainder = chunks.remainder();
    match remainder.len() {
        1 => {
            let n = (remainder[0] as u32) << 16;
            result.push(BASE64_TABLE[(n >> 18 & 0x3F) as usize] as char);
            result.push(BASE64_TABLE[(n >> 12 & 0x3F) as usize] as char);
            result.push('=');
            result.push('=');
        }
        2 => {
            let n = (remainder[0] as u32) << 16 | (remainder[1] as u32) << 8;
            result.push(BASE64_TABLE[(n >> 18 & 0x3F) as usize] as char);
            result.push(BASE64_TABLE[(n >> 12 & 0x3F) as usize] as char);
            result.push(BASE64_TABLE[(n >> 6 & 0x3F) as usize] as char);
            result.push('=');
        }
        _ => {}
    }
    result
}

#[cfg(test)]
mod html_escape_tests {
    use super::html_escape;

    #[test]
    fn empty_string() {
        assert_eq!(html_escape(""), "");
    }

    #[test]
    fn no_special_chars() {
        assert_eq!(html_escape("hello world 123"), "hello world 123");
    }

    #[test]
    fn all_four_specials() {
        assert_eq!(html_escape("&"), "&amp;");
        assert_eq!(html_escape("<"), "&lt;");
        assert_eq!(html_escape(">"), "&gt;");
        assert_eq!(html_escape("\""), "&quot;");
    }

    #[test]
    fn single_quote_not_escaped() {
        assert_eq!(html_escape("'"), "'");
    }

    #[test]
    fn mixed_content() {
        assert_eq!(
            html_escape("<a href=\"x\">Tom & Jerry</a>"),
            "&lt;a href=&quot;x&quot;&gt;Tom &amp; Jerry&lt;/a&gt;"
        );
    }

    #[test]
    fn already_escaped_ampersand_is_double_escaped() {
        assert_eq!(html_escape("&amp;"), "&amp;amp;");
    }
}

#[cfg(test)]
mod base64_encode_tests {
    use super::base64_encode;

    #[test]
    fn empty_input() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn one_byte_input() {
        assert_eq!(base64_encode(b"f"), "Zg==");
    }

    #[test]
    fn two_byte_input() {
        assert_eq!(base64_encode(b"fo"), "Zm8=");
    }

    #[test]
    fn three_byte_input() {
        assert_eq!(base64_encode(b"foo"), "Zm9v");
    }

    #[test]
    fn known_vectors() {
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
    }

    #[test]
    fn longer_multi_block_input() {
        assert_eq!(
            base64_encode(b"The quick brown fox jumps over the lazy dog"),
            "VGhlIHF1aWNrIGJyb3duIGZveCBqdW1wcyBvdmVyIHRoZSBsYXp5IGRvZw=="
        );
    }

    #[test]
    fn binary_bytes() {
        assert_eq!(base64_encode(&[0x00, 0x00, 0x00]), "AAAA");
        assert_eq!(base64_encode(&[0xFF, 0xFF, 0xFF]), "////");
        assert_eq!(base64_encode(&[0xFF]), "/w==");
    }
}

//! Pure MIME-type lookup by file extension (Phase 4, step 4.3 of
//! `docs/IMPLEMENTATION-PLAN.md`).

/// Looks up the MIME type for a file extension (without the leading dot),
/// matching case-insensitively. Falls back to
/// `application/octet-stream` for anything unrecognized.
pub fn lookup(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "html" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_html() {
        assert_eq!(lookup("html"), "text/html");
    }

    #[test]
    fn looks_up_css() {
        assert_eq!(lookup("css"), "text/css");
    }

    #[test]
    fn looks_up_js() {
        assert_eq!(lookup("js"), "application/javascript");
    }

    #[test]
    fn looks_up_png() {
        assert_eq!(lookup("png"), "image/png");
    }

    #[test]
    fn looks_up_jpg_and_jpeg() {
        assert_eq!(lookup("jpg"), "image/jpeg");
        assert_eq!(lookup("jpeg"), "image/jpeg");
    }

    #[test]
    fn looks_up_gif() {
        assert_eq!(lookup("gif"), "image/gif");
    }

    #[test]
    fn looks_up_svg() {
        assert_eq!(lookup("svg"), "image/svg+xml");
    }

    #[test]
    fn looks_up_webp() {
        assert_eq!(lookup("webp"), "image/webp");
    }

    #[test]
    fn matches_case_insensitively() {
        assert_eq!(lookup("HTML"), "text/html");
        assert_eq!(lookup("Png"), "image/png");
        assert_eq!(lookup("JPEG"), "image/jpeg");
    }

    #[test]
    fn falls_back_for_unknown_extension() {
        assert_eq!(lookup("xyz"), "application/octet-stream");
    }

    #[test]
    fn falls_back_for_empty_extension() {
        assert_eq!(lookup(""), "application/octet-stream");
    }
}

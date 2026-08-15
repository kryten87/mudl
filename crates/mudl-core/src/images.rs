use std::path::{Path, PathBuf};

pub fn is_external_source(source: &str) -> bool {
    let lower = source.to_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || lower.starts_with("mailto:")
}

fn mime_type_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" => Some("image/jpeg"),
        "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

pub fn classify(source: &str, base_dir: &Path) -> Option<(PathBuf, &'static str)> {
    if is_external_source(source) {
        return None;
    }

    let ext = Path::new(source)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())?;
    let mime = mime_type_for_extension(&ext)?;

    let resolved = base_dir.join(source);

    Some((resolved, mime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_is_external() {
        assert!(is_external_source("http://example.com/img.png"));
    }

    #[test]
    fn https_is_external() {
        assert!(is_external_source("https://example.com/img.png"));
    }

    #[test]
    fn data_uri_is_external() {
        assert!(is_external_source("data:image/png;base64,abc"));
    }

    #[test]
    fn mailto_is_external() {
        assert!(is_external_source("mailto:test@example.com"));
    }

    #[test]
    fn case_insensitive_http() {
        assert!(is_external_source("HTTP://EXAMPLE.COM"));
    }

    #[test]
    fn case_insensitive_https() {
        assert!(is_external_source("Https://Example.com"));
    }

    #[test]
    fn case_insensitive_data() {
        assert!(is_external_source("Data:image/png;base64,abc"));
    }

    #[test]
    fn case_insensitive_mailto() {
        assert!(is_external_source("MAILTO:test@example.com"));
    }

    #[test]
    fn relative_path_not_external() {
        assert!(!is_external_source("images/photo.png"));
    }

    #[test]
    fn absolute_path_not_external() {
        assert!(!is_external_source("/usr/local/img.png"));
    }

    #[test]
    fn bare_filename_not_external() {
        assert!(!is_external_source("photo.png"));
    }

    #[test]
    fn classify_png() {
        let base = Path::new("/base/dir");
        let (path, mime) = classify("photo.png", base).unwrap();
        assert_eq!(path, base.join("photo.png"));
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn classify_jpg() {
        let base = Path::new("/base/dir");
        let (_, mime) = classify("photo.jpg", base).unwrap();
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn classify_jpeg() {
        let base = Path::new("/base/dir");
        let (_, mime) = classify("photo.jpeg", base).unwrap();
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn classify_gif() {
        let base = Path::new("/base/dir");
        let (_, mime) = classify("photo.gif", base).unwrap();
        assert_eq!(mime, "image/gif");
    }

    #[test]
    fn classify_svg() {
        let base = Path::new("/base/dir");
        let (_, mime) = classify("photo.svg", base).unwrap();
        assert_eq!(mime, "image/svg+xml");
    }

    #[test]
    fn classify_webp() {
        let base = Path::new("/base/dir");
        let (_, mime) = classify("photo.webp", base).unwrap();
        assert_eq!(mime, "image/webp");
    }

    #[test]
    fn classify_unknown_extension_is_none() {
        let base = Path::new("/base/dir");
        assert!(classify("document.pdf", base).is_none());
    }

    #[test]
    fn classify_no_extension_is_none() {
        let base = Path::new("/base/dir");
        assert!(classify("photo", base).is_none());
    }

    #[test]
    fn classify_case_insensitive_extension() {
        let base = Path::new("/base/dir");
        let (_, mime) = classify("photo.PNG", base).unwrap();
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn classify_relative_path_resolved_against_base_dir() {
        let base = Path::new("/base/dir");
        let (path, _) = classify("images/photo.png", base).unwrap();
        assert_eq!(path, base.join("images/photo.png"));
    }

    #[test]
    fn classify_absolute_source_replaces_base_dir() {
        let base = Path::new("/base/dir");
        let (path, _) = classify("/absolute/photo.png", base).unwrap();
        assert_eq!(path, PathBuf::from("/absolute/photo.png"));
    }

    #[test]
    fn classify_external_source_is_none() {
        let base = Path::new("/base/dir");
        assert!(classify("https://example.com/photo.png", base).is_none());
    }

    #[test]
    fn classify_data_uri_is_none() {
        let base = Path::new("/base/dir");
        assert!(classify("data:image/png;base64,abc", base).is_none());
    }
}

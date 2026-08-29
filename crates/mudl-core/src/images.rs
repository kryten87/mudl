use std::io;
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

pub fn encode_data_uri(
    source: &str,
    base_dir: &Path,
    read: &dyn Fn(&Path) -> io::Result<Vec<u8>>,
) -> Option<String> {
    let (path, mime) = classify(source, base_dir)?;
    let bytes = read(&path).ok()?;
    Some(format!(
        "data:{};base64,{}",
        mime,
        crate::encoding::base64_encode(&bytes)
    ))
}

/// Rewrites every local `<img src="...">` in `html` to an inlined data URI
/// (used for `--standalone` export, Phase 8.2), reusing the same tag scanner
/// as [`crate::template::rewrite_local_image_srcs`]. A source that can't be
/// inlined (external, unknown extension, or an unreadable file) is left as
/// the original `src` value rather than dropped, so a broken/missing local
/// image degrades to the same "broken image" rendering a browser would show
/// for the un-rewritten path, instead of vanishing silently.
pub fn rewrite_srcs_to_data_uris(
    html: &str,
    base_dir: &Path,
    read: &dyn Fn(&Path) -> io::Result<Vec<u8>>,
) -> String {
    crate::template::rewrite_img_srcs(html, &|src| {
        encode_data_uri(src, base_dir, read).unwrap_or_else(|| src.to_string())
    })
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

    #[test]
    fn encode_data_uri_known_extension_with_fake_bytes() {
        let base = Path::new("/base/dir");
        let read = |path: &Path| -> io::Result<Vec<u8>> {
            assert_eq!(path, base.join("photo.png"));
            Ok(b"fo".to_vec())
        };
        let result = encode_data_uri("photo.png", base, &read).unwrap();
        assert_eq!(result, "data:image/png;base64,Zm8=");
    }

    #[test]
    fn encode_data_uri_external_source_is_none() {
        let base = Path::new("/base/dir");
        let read = |_: &Path| -> io::Result<Vec<u8>> { panic!("read should not be called") };
        assert!(encode_data_uri("https://example.com/photo.png", base, &read).is_none());
    }

    #[test]
    fn encode_data_uri_unknown_extension_short_circuits_before_read() {
        let base = Path::new("/base/dir");
        let read = |_: &Path| -> io::Result<Vec<u8>> { panic!("read should not be called") };
        assert!(encode_data_uri("document.pdf", base, &read).is_none());
    }

    #[test]
    fn encode_data_uri_read_error_is_none() {
        let base = Path::new("/base/dir");
        let read = |_: &Path| -> io::Result<Vec<u8>> {
            Err(io::Error::new(io::ErrorKind::NotFound, "missing"))
        };
        assert!(encode_data_uri("photo.png", base, &read).is_none());
    }

    #[test]
    fn encode_data_uri_empty_bytes_is_valid_empty_data_uri() {
        let base = Path::new("/base/dir");
        let read = |_: &Path| -> io::Result<Vec<u8>> { Ok(Vec::new()) };
        let result = encode_data_uri("photo.png", base, &read).unwrap();
        assert_eq!(result, "data:image/png;base64,");
    }

    #[test]
    fn rewrite_srcs_to_data_uris_local_image_inlined() {
        let base = Path::new("/base/dir");
        let read = |_: &Path| -> io::Result<Vec<u8>> { Ok(b"fo".to_vec()) };
        let html = r#"<p><img src="photo.png" alt="x"></p>"#;
        assert_eq!(
            rewrite_srcs_to_data_uris(html, base, &read),
            r#"<p><img src="data:image/png;base64,Zm8=" alt="x"></p>"#
        );
    }

    #[test]
    fn rewrite_srcs_to_data_uris_external_source_unchanged() {
        let base = Path::new("/base/dir");
        let read = |_: &Path| -> io::Result<Vec<u8>> { panic!("read should not be called") };
        let html = r#"<img src="https://example.com/photo.png">"#;
        assert_eq!(rewrite_srcs_to_data_uris(html, base, &read), html);
    }

    #[test]
    fn rewrite_srcs_to_data_uris_missing_file_falls_back_to_original_src() {
        let base = Path::new("/base/dir");
        let read = |_: &Path| -> io::Result<Vec<u8>> {
            Err(io::Error::new(io::ErrorKind::NotFound, "missing"))
        };
        let html = r#"<img src="photo.png">"#;
        assert_eq!(rewrite_srcs_to_data_uris(html, base, &read), html);
    }

    #[test]
    fn rewrite_srcs_to_data_uris_no_img_tags_unchanged() {
        let base = Path::new("/base/dir");
        let read = |_: &Path| -> io::Result<Vec<u8>> { panic!("read should not be called") };
        let html = "<p>no images here</p>";
        assert_eq!(rewrite_srcs_to_data_uris(html, base, &read), html);
    }

    #[test]
    fn rewrite_srcs_to_data_uris_multiple_images() {
        let base = Path::new("/base/dir");
        let read = |_: &Path| -> io::Result<Vec<u8>> { Ok(b"fo".to_vec()) };
        let html = r#"<img src="a.png"><img src="b.png">"#;
        assert_eq!(
            rewrite_srcs_to_data_uris(html, base, &read),
            r#"<img src="data:image/png;base64,Zm8="><img src="data:image/png;base64,Zm8=">"#
        );
    }
}

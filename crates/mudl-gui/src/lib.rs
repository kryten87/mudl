//! GTK3 + WebKit2GTK application shell (Phase 10 of
//! `docs/IMPLEMENTATION-PLAN.md`).

mod window;

use std::path::PathBuf;

/// Opens the GUI on the first file in `files` (multiple files opening as
/// tabs in one window is Phase 10.6; a folder-index/no-file launch mode is
/// a later addition too — see Appendix B's `folderOpenBehavior`). Blocks
/// until the window closes.
pub fn launch(files: &[String]) -> Result<(), String> {
    let Some(first) = files.first() else {
        return Err(
            "no file given (folder index / multi-file launch isn't implemented yet)".to_string(),
        );
    };
    window::run(PathBuf::from(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_with_no_files_is_an_error() {
        assert!(launch(&[]).is_err());
    }
}

//! GTK3 + WebKit2GTK application shell (Phase 10 of
//! `docs/IMPLEMENTATION-PLAN.md`).

mod changes;
mod config;
mod find;
mod geometry;
mod linkaction;
mod sidebar;
mod toggle;
mod toolbar;
mod window;

use std::path::PathBuf;

/// Opens one window with one tab per file in `files` (Phase 10.6). A
/// folder-index/no-file launch mode is a later addition — see Appendix
/// B's `folderOpenBehavior`. Blocks until the window closes.
pub fn launch(files: &[String]) -> Result<(), String> {
    let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    window::run(&paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_with_no_files_is_an_error() {
        assert!(launch(&[]).is_err());
    }
}

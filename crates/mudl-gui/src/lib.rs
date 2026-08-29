//! GTK3 + WebKit2GTK application shell (Phase 10 of
//! `docs/IMPLEMENTATION-PLAN.md`) — not yet built.
//!
//! [`launch`] exists now only so `mudl-cli`'s "no render flag given" path
//! (Phase 8.2) has a real function to call ahead of Phase 10 implementing
//! the window itself.

/// Placeholder for the eventual GTK window launch. Always fails until
/// Phase 10 lands.
pub fn launch(files: &[String]) -> Result<(), String> {
    let _ = files;
    Err("GUI not yet implemented (see Phase 10 of docs/IMPLEMENTATION-PLAN.md)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_is_not_yet_implemented() {
        assert!(launch(&[]).is_err());
    }
}

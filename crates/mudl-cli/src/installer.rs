//! CLI installer (Phase 11, step 11.1 of `docs/IMPLEMENTATION-PLAN.md`).
//!
//! Per the scope decision (plan §1), there's no `osascript`-style privilege
//! elevation here — installing `mudl` as a CLI tool is just placing a
//! symlink at `~/.local/bin/mudl` pointing at the running binary, which is
//! already writable by the invoking user. The target-path construction is
//! pure and tested directly; the actual symlink/directory/current-exe calls
//! are behind the [`FileSystem`] DI boundary (§5.2) so [`install`] itself is
//! testable without touching the real filesystem.

use std::io;
use std::path::{Path, PathBuf};

/// The DI boundary for the handful of OS operations `install` needs. Scoped
/// narrowly to this module's use, mirroring the per-crate `FileSystem` trait
/// convention already used by `mudl-watch`/`mudl-config`/`mudl-server`.
pub trait FileSystem: Send + Sync {
    /// The path of the currently-running binary (`std::env::current_exe`).
    fn current_exe(&self) -> io::Result<PathBuf>;
    fn exists(&self, path: &Path) -> bool;
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    fn symlink(&self, original: &Path, link: &Path) -> io::Result<()>;
}

/// A thin wrapper over `std::env`/`std::fs`/`std::os::unix::fs`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn current_exe(&self) -> io::Result<PathBuf> {
        std::env::current_exe()
    }

    fn exists(&self, path: &Path) -> bool {
        path.symlink_metadata().is_ok()
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn symlink(&self, original: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }
}

/// Pure decision logic: where the CLI symlink goes, given a home directory.
/// Separated from [`install`] so it's testable with no fake at all.
pub fn target_path(home: &Path) -> PathBuf {
    home.join(".local").join("bin").join("mudl")
}

/// Installs `mudl` as a CLI tool: symlinks [`target_path`] to the running
/// binary. Idempotent — re-running replaces a stale symlink (or a leftover
/// plain file) at the target rather than failing on "already exists".
pub fn install(fs: &dyn FileSystem, home: &Path) -> io::Result<PathBuf> {
    let target = target_path(home);
    let binary = fs.current_exe()?;

    if let Some(parent) = target.parent() {
        fs.create_dir_all(parent)?;
    }
    if fs.exists(&target) {
        fs.remove_file(&target)?;
    }
    fs.symlink(&binary, &target)?;

    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct FakeState {
        current_exe: Option<PathBuf>,
        existing: HashSet<PathBuf>,
        dirs_created: HashSet<PathBuf>,
        symlinks: HashMap<PathBuf, PathBuf>,
        symlink_should_fail: bool,
    }

    #[derive(Debug, Clone, Default)]
    struct FakeFileSystem {
        state: Arc<Mutex<FakeState>>,
    }

    impl FakeFileSystem {
        fn new() -> Self {
            Self::default()
        }

        fn set_current_exe(&self, path: impl Into<PathBuf>) {
            self.state.lock().unwrap().current_exe = Some(path.into());
        }

        fn mark_existing(&self, path: impl Into<PathBuf>) {
            self.state.lock().unwrap().existing.insert(path.into());
        }

        fn force_symlink_error(&self) {
            self.state.lock().unwrap().symlink_should_fail = true;
        }

        fn symlink_target(&self, link: &Path) -> Option<PathBuf> {
            self.state.lock().unwrap().symlinks.get(link).cloned()
        }

        fn dir_created(&self, path: &Path) -> bool {
            self.state.lock().unwrap().dirs_created.contains(path)
        }
    }

    impl FileSystem for FakeFileSystem {
        fn current_exe(&self) -> io::Result<PathBuf> {
            self.state
                .lock()
                .unwrap()
                .current_exe
                .clone()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "current_exe not set"))
        }

        fn exists(&self, path: &Path) -> bool {
            let state = self.state.lock().unwrap();
            state.existing.contains(path) || state.symlinks.contains_key(path)
        }

        fn create_dir_all(&self, path: &Path) -> io::Result<()> {
            self.state
                .lock()
                .unwrap()
                .dirs_created
                .insert(path.to_path_buf());
            Ok(())
        }

        fn remove_file(&self, path: &Path) -> io::Result<()> {
            let mut state = self.state.lock().unwrap();
            state.existing.remove(path);
            state.symlinks.remove(path);
            Ok(())
        }

        fn symlink(&self, original: &Path, link: &Path) -> io::Result<()> {
            let mut state = self.state.lock().unwrap();
            if state.symlink_should_fail {
                return Err(io::Error::other("symlink failed"));
            }
            state
                .symlinks
                .insert(link.to_path_buf(), original.to_path_buf());
            Ok(())
        }
    }

    #[test]
    fn target_path_joins_local_bin_mudl() {
        assert_eq!(
            target_path(Path::new("/home/dave")),
            PathBuf::from("/home/dave/.local/bin/mudl")
        );
    }

    #[test]
    fn install_creates_parent_dir_and_symlink_to_current_exe() {
        let fake = FakeFileSystem::new();
        fake.set_current_exe("/opt/mudl/bin/mudl");
        let home = Path::new("/home/dave");

        let target = install(&fake, home).unwrap();

        assert_eq!(target, PathBuf::from("/home/dave/.local/bin/mudl"));
        assert!(fake.dir_created(Path::new("/home/dave/.local/bin")));
        assert_eq!(
            fake.symlink_target(&target),
            Some(PathBuf::from("/opt/mudl/bin/mudl"))
        );
    }

    #[test]
    fn install_replaces_a_stale_existing_target() {
        let fake = FakeFileSystem::new();
        fake.set_current_exe("/opt/mudl/bin/mudl");
        fake.mark_existing("/home/dave/.local/bin/mudl");
        let home = Path::new("/home/dave");

        let target = install(&fake, home).unwrap();

        assert_eq!(
            fake.symlink_target(&target),
            Some(PathBuf::from("/opt/mudl/bin/mudl"))
        );
    }

    #[test]
    fn install_propagates_current_exe_error() {
        let fake = FakeFileSystem::new();
        let home = Path::new("/home/dave");

        assert!(install(&fake, home).is_err());
    }

    #[test]
    fn install_propagates_symlink_error() {
        let fake = FakeFileSystem::new();
        fake.set_current_exe("/opt/mudl/bin/mudl");
        fake.force_symlink_error();
        let home = Path::new("/home/dave");

        assert!(install(&fake, home).is_err());
    }
}

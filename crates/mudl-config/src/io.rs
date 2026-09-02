//! Impure load/save (Phase 7, step 7.4): the `FileSystem` dependency-injection
//! boundary (plan §5.2) around the pure [`crate::format`]/[`crate::preferences`]
//! logic, plus the two functions that tie them together. Mirrors the
//! narrowly-scoped `FileSystem` trait convention already used by
//! `mudl-watch` and `mudl-server` — this one needs both a read and an
//! atomic write, so it's shaped differently from either.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::format;
use crate::preferences::Preferences;

/// The impure half of preferences persistence: reading the raw file bytes
/// and atomically replacing them. Production code uses [`RealFileSystem`];
/// tests use [`InMemoryFileSystem`].
pub trait FileSystem: Send + Sync {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Writes `contents` to `path` atomically: a partial write (crash,
    /// full disk) must never leave `path` truncated or corrupt, only either
    /// unchanged or fully replaced.
    fn write_atomic(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
}

/// A thin wrapper over `std::fs`: writes to a sibling temp file, then
/// `rename`s it over the real target (a rename within the same directory is
/// atomic on Linux). Creates the parent directory first, since
/// `~/.config/mudl/` may not exist yet on a first run.
///
/// - The target is resolved through symlinks first ([`resolve_symlink`]), so
///   replacing a symlinked preferences file replaces what it points to,
///   rather than clobbering the link itself.
/// - The temp file is created with `O_EXCL` (`create_new`), so a symlink an
///   attacker pre-placed at the temp path is refused, not followed.
/// - The temp name carries a random suffix ([`sibling_tmp_path`]) on top of
///   that, so the name can't be guessed and raced in the first place.
/// - The original file's permissions (if it exists) are restored on the
///   replacement, rather than left at the default mode.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write_atomic(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let real_path = resolve_symlink(path);
        let existing_permissions = std::fs::metadata(&real_path).ok().map(|m| m.permissions());

        let tmp_path = sibling_tmp_path(&real_path);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);

        if let Some(permissions) = existing_permissions {
            std::fs::set_permissions(&tmp_path, permissions)?;
        }

        std::fs::rename(&tmp_path, &real_path)
    }
}

/// Follows `path` through symlinks to the file that would actually receive
/// the write. Falls back to `path` unchanged when it doesn't exist yet or
/// isn't a symlink -- `canonicalize` requires every component to exist.
fn resolve_symlink(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A sibling of `path` with an unpredictable suffix, for use as a `create_new`
/// temp file target.
fn sibling_tmp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(
        ".tmp-{:x}-{:x}-{:x}",
        std::process::id(),
        nanos,
        counter
    ));
    path.with_file_name(tmp_name)
}

/// A `HashMap`-backed in-memory fake, for tests: never touches the real
/// filesystem, and a path absent from the map reports a `NotFound` error
/// the same way a real missing file would.
#[derive(Debug, Clone, Default)]
pub struct InMemoryFileSystem {
    files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
}

impl InMemoryFileSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, path: impl Into<PathBuf>, contents: impl Into<Vec<u8>>) {
        self.files
            .lock()
            .unwrap()
            .insert(path.into(), contents.into());
    }
}

impl FileSystem for InMemoryFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found"))
    }

    fn write_atomic(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), contents.to_vec());
        Ok(())
    }
}

/// Loads preferences from `path` via `fs`. A missing file (or any other
/// read error) is not a failure — it just means "no preferences saved yet",
/// so this returns [`Preferences::default`] rather than a `Result`.
pub fn load(fs: &dyn FileSystem, path: &Path) -> Preferences {
    match fs.read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            Preferences::from_entries(&format::parse(&text))
        }
        Err(_) => Preferences::default(),
    }
}

/// Serializes `prefs` and writes it to `path` via `fs`, atomically.
pub fn save(fs: &dyn FileSystem, path: &Path, prefs: &Preferences) -> io::Result<()> {
    let text = format::serialize(&prefs.to_entries());
    fs.write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preferences::Theme;

    #[test]
    fn missing_file_loads_as_defaults() {
        let fake = InMemoryFileSystem::new();
        assert_eq!(
            load(&fake, Path::new("/home/user/.config/mudl/preferences")),
            Preferences::default()
        );
    }

    #[test]
    fn file_with_some_keys_overrides_those_and_defaults_the_rest() {
        let fake = InMemoryFileSystem::new();
        let path = Path::new("/home/user/.config/mudl/preferences");
        fake.insert(path, *b"theme = riot\n");

        let prefs = load(&fake, path);

        assert_eq!(prefs.theme, Theme::Riot);
        assert_eq!(prefs.lighting, Preferences::default().lighting);
        assert_eq!(prefs.quit_on_close, Preferences::default().quit_on_close);
    }

    #[test]
    fn save_then_load_round_trips() {
        let fake = InMemoryFileSystem::new();
        let path = Path::new("/home/user/.config/mudl/preferences");
        let prefs = Preferences {
            theme: Theme::Blues,
            up_mode_zoom_level: 1.5,
            sidebar_enabled: true,
            ..Preferences::default()
        };

        save(&fake, path, &prefs).unwrap();
        let loaded = load(&fake, path);

        assert_eq!(loaded, prefs);
    }

    #[test]
    fn real_fs_save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("mudl-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preferences");

        let real = RealFileSystem;
        let prefs = Preferences {
            theme: Theme::Austere,
            ..Preferences::default()
        };

        save(&real, &path, &prefs).unwrap();
        let loaded = load(&real, &path);

        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(loaded, prefs);
    }

    #[test]
    fn real_fs_creates_missing_parent_directory() {
        let dir =
            std::env::temp_dir().join(format!("mudl-config-test-nested-{}", std::process::id()));
        let path = dir.join("nested").join("preferences");

        let real = RealFileSystem;
        let result = save(&real, &path, &Preferences::default());

        std::fs::remove_dir_all(&dir).ok();
        assert!(result.is_ok());
    }

    #[test]
    fn real_fs_write_atomic_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir =
            std::env::temp_dir().join(format!("mudl-config-perms-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preferences");
        std::fs::write(&path, "theme = riot\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let real = RealFileSystem;
        save(&real, &path, &Preferences::default()).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn real_fs_write_atomic_follows_symlink() {
        use std::os::unix::fs::symlink;

        let dir =
            std::env::temp_dir().join(format!("mudl-config-symlink-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real_path = dir.join("real-preferences");
        let link_path = dir.join("preferences");
        std::fs::write(&real_path, "theme = riot\n").unwrap();
        symlink(&real_path, &link_path).unwrap();

        let real = RealFileSystem;
        save(&real, &link_path, &Preferences::default()).unwrap();

        let link_is_still_a_symlink = std::fs::symlink_metadata(&link_path)
            .unwrap()
            .file_type()
            .is_symlink();
        let target = load(&real, &real_path);
        std::fs::remove_dir_all(&dir).ok();

        assert!(link_is_still_a_symlink);
        assert_eq!(target, Preferences::default());
    }
}

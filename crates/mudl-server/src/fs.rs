//! The `FileSystem` dependency-injection boundary (plan §5.2) for
//! `Route::LocalFile` (Phase 5, step 5.2 of `docs/IMPLEMENTATION-PLAN.md`):
//! reading a file referenced by a served document (e.g. a relative image)
//! is impure, so it goes through a trait rather than a bare `std::fs::read`
//! call, letting tests exercise the route handler with an in-memory fake
//! and never touch the real filesystem.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The impure half of local file serving: reading arbitrary bytes from a
/// path. Production code uses [`RealFileSystem`]; tests use
/// [`InMemoryFileSystem`].
pub trait FileSystem: Send + Sync {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
}

/// A thin wrapper over `std::fs::read`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }
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

    /// Builds a fake pre-populated with `files`, for tests that want to
    /// construct it inline rather than calling `insert` repeatedly.
    pub fn with_files<I, P>(files: I) -> Self
    where
        I: IntoIterator<Item = (P, Vec<u8>)>,
        P: Into<PathBuf>,
    {
        let fake = Self::new();
        for (path, contents) in files {
            fake.insert(path, contents);
        }
        fake
    }

    pub fn insert(&self, path: impl Into<PathBuf>, contents: Vec<u8>) {
        self.files.lock().unwrap().insert(path.into(), contents);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_fs_returns_inserted_bytes() {
        let fake = InMemoryFileSystem::new();
        fake.insert("/tmp/notes.md", b"hello".to_vec());
        assert_eq!(fake.read(Path::new("/tmp/notes.md")).unwrap(), b"hello");
    }

    #[test]
    fn in_memory_fs_missing_path_is_not_found() {
        let fake = InMemoryFileSystem::new();
        let err = fake.read(Path::new("/tmp/missing.md")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn in_memory_fs_with_files_prepopulates() {
        let fake = InMemoryFileSystem::with_files([(PathBuf::from("/a.png"), b"bytes".to_vec())]);
        assert_eq!(fake.read(Path::new("/a.png")).unwrap(), b"bytes");
    }

    #[test]
    fn real_fs_reads_a_real_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mudl-fs-test-{}", std::process::id()));
        std::fs::write(&path, b"real bytes").unwrap();

        let real = RealFileSystem;
        let result = real.read(&path).unwrap();

        std::fs::remove_file(&path).ok();
        assert_eq!(result, b"real bytes");
    }
}

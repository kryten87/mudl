//! The impure write flow for comments (Phase 14.5). Ported from `mud`'s
//! `App/CommentController.swift`, minus the macOS-specific bits (security-
//! scoped resource access, the `isUserImmutable`/lock-file check -- Linux
//! has no equivalent of Finder's Locked checkbox, and a plain permission
//! error from `write_atomic` already reports the same "couldn't write"
//! outcome without a separate up-front gate).
//!
//! Every mutation re-reads the file fresh from disk rather than trusting
//! any in-memory state, so a concurrent external edit outside the touched
//! span is never clobbered; the caller's file watcher (Phase 6) picks up
//! the write like any other external change.

use std::io;
use std::path::Path;

use crate::anchor;
use crate::document;
use crate::editor;
use crate::serialization::CommentMessage;

/// The impure half of comment persistence: reading the raw file bytes and
/// atomically replacing them. Production code uses [`RealFileSystem`];
/// tests use [`InMemoryFileSystem`].
pub trait FileSystem: Send + Sync {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Writes `contents` to `path` atomically: a partial write (crash, full
    /// disk) must never leave `path` truncated or corrupt, only either
    /// unchanged or fully replaced.
    fn write_atomic(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
}

/// A thin wrapper over `std::fs`: writes to a sibling temp file, then
/// `rename`s it into place (a rename within the same directory is atomic on
/// Linux). The temp name *appends* `.tmp` to the full file name (rather
/// than replacing the extension) so two differently-extensioned files
/// sharing a base name -- `notes.md` and `notes.txt` -- can never collide
/// on the same temp path.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write_atomic(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp_path = path.with_file_name(tmp_name);
        std::fs::write(&tmp_path, contents)?;
        std::fs::rename(&tmp_path, path)
    }
}

/// Why a comment mutation failed. The two causes look identical to the user
/// but have different fixes, so conflating them (as a single `bool`/`None`
/// would) hides a wrong diagnosis: `AnchorFailed` means the comment no
/// longer matches the source -- an add's quoted text no longer maps to a
/// source byte, or the label's definition is gone (changed or removed on
/// disk while the edit was pending); `WriteFailed` means the file itself
/// couldn't be read or written (permission, missing parent, IO).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    AnchorFailed,
    WriteFailed,
}

/// Inserts a new comment. When `quotation` is `Some`, the marker is
/// anchored at the end of the `occurrence`-th block whose text matches it
/// (`anchor::locate`, Phase 14.3); a miss there is `AnchorFailed` -- v1 has
/// no general-comment fallback for an anchor miss. When `quotation` is
/// `None` (a general, unanchored comment), the marker is placed right after
/// the document's last non-whitespace byte -- attached to the last visible
/// content rather than dangling alone after the file's trailing newline.
/// Returns the new label on success.
pub fn add_comment(
    fs: &dyn FileSystem,
    path: &Path,
    quotation: Option<&str>,
    occurrence: usize,
    message: CommentMessage,
) -> Result<String, WriteError> {
    let source = read_source(fs, path)?;

    let marker_byte_offset = match quotation {
        Some(q) => anchor::locate(&source, q, occurrence).ok_or(WriteError::AnchorFailed)?,
        None => source.trim_end().len(),
    };

    let (rewritten, comment) = editor::insert(&source, marker_byte_offset, quotation, message);
    write(fs, path, &rewritten)?;
    Ok(comment.label)
}

/// Appends a reply message to the `label` comment's thread, preserving its
/// quotation and existing messages. Re-reads the current thread from disk
/// so a concurrent external edit isn't lost.
pub fn reply(
    fs: &dyn FileSystem,
    path: &Path,
    label: &str,
    message: CommentMessage,
) -> Result<(), WriteError> {
    let source = read_source(fs, path)?;
    let comment = find_comment(&source, label)?;
    let mut messages = comment.messages;
    messages.push(message);
    rewrite_and_write(
        fs,
        path,
        &source,
        label,
        comment.quotation.as_deref(),
        &messages,
    )
}

/// Replaces the body of the `label` comment's most recent message, keeping
/// that message's author and timestamp (it is the same message, edited).
pub fn edit_last_message(
    fs: &dyn FileSystem,
    path: &Path,
    label: &str,
    body: String,
) -> Result<(), WriteError> {
    let source = read_source(fs, path)?;
    let comment = find_comment(&source, label)?;
    let mut messages = comment.messages;
    let last = messages.last_mut().ok_or(WriteError::AnchorFailed)?;
    last.body = body;
    rewrite_and_write(
        fs,
        path,
        &source,
        label,
        comment.quotation.as_deref(),
        &messages,
    )
}

/// Removes the `label` comment entirely -- definition and every marker.
pub fn delete_comment(fs: &dyn FileSystem, path: &Path, label: &str) -> Result<(), WriteError> {
    let source = read_source(fs, path)?;
    let deleted = editor::delete(&source, label).ok_or(WriteError::AnchorFailed)?;
    write(fs, path, &deleted)
}

/// Removes the most recent message from the `label` comment's thread. When
/// it was the only message, the whole comment goes (a comment can't be
/// empty).
pub fn delete_last_message(
    fs: &dyn FileSystem,
    path: &Path,
    label: &str,
) -> Result<(), WriteError> {
    let source = read_source(fs, path)?;
    let comment = find_comment(&source, label)?;
    if comment.messages.len() <= 1 {
        let deleted = editor::delete(&source, label).ok_or(WriteError::AnchorFailed)?;
        return write(fs, path, &deleted);
    }
    let mut messages = comment.messages;
    messages.pop();
    rewrite_and_write(
        fs,
        path,
        &source,
        label,
        comment.quotation.as_deref(),
        &messages,
    )
}

fn find_comment(source: &str, label: &str) -> Result<crate::serialization::Comment, WriteError> {
    document::parse_comments(source)
        .into_iter()
        .find(|c| c.label == label)
        .ok_or(WriteError::AnchorFailed)
}

/// Rewrites the `label` definition and writes the result, telling a vanished
/// label apart from a failed disk write.
fn rewrite_and_write(
    fs: &dyn FileSystem,
    path: &Path,
    source: &str,
    label: &str,
    quotation: Option<&str>,
    messages: &[CommentMessage],
) -> Result<(), WriteError> {
    let rewritten =
        editor::rewrite(source, label, quotation, messages).ok_or(WriteError::AnchorFailed)?;
    write(fs, path, &rewritten)
}

fn read_source(fs: &dyn FileSystem, path: &Path) -> Result<String, WriteError> {
    let bytes = fs.read(path).map_err(|_| WriteError::WriteFailed)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn write(fs: &dyn FileSystem, path: &Path, contents: &str) -> Result<(), WriteError> {
    fs.write_atomic(path, contents.as_bytes())
        .map_err(|_| WriteError::WriteFailed)
}

/// A `HashMap`-backed in-memory fake, for tests: never touches the real
/// filesystem, and a path absent from the map reports a `NotFound` error
/// the same way a real missing file would.
#[cfg(test)]
mod test_support {
    use super::FileSystem;
    use std::collections::HashMap;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct InMemoryFileSystem {
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
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

        pub fn get(&self, path: &Path) -> String {
            String::from_utf8(self.files.lock().unwrap().get(path).unwrap().clone()).unwrap()
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
}

#[cfg(test)]
mod tests {
    use super::test_support::InMemoryFileSystem;
    use super::*;
    use std::path::Path;

    fn msg(body: &str) -> CommentMessage {
        CommentMessage {
            author: None,
            created: None,
            body: body.to_string(),
        }
    }

    #[test]
    fn add_comment_with_quotation_anchors_and_writes() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/doc.md");
        fs.insert(path, "The quick brown fox.\n");

        let label = add_comment(&fs, path, Some("The quick brown fox."), 0, msg("Note.")).unwrap();

        assert_eq!(label, "comment-a");
        let contents = fs.get(path);
        // The whole sentence (period included) is the quotation, so the
        // marker anchors right after it.
        assert!(contents.starts_with("The quick brown fox.[^comment-a]\n"));
        assert!(contents.contains("[^comment-a]:\n    > The quick brown fox.\n\n    Note."));
    }

    #[test]
    fn add_comment_anchor_miss_is_anchor_failed() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/doc.md");
        fs.insert(path, "The quick brown fox.\n");

        let result = add_comment(&fs, path, Some("Not in the document."), 0, msg("Note."));

        assert_eq!(result, Err(WriteError::AnchorFailed));
    }

    #[test]
    fn add_general_comment_appends_at_end_of_document() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/doc.md");
        fs.insert(path, "Some text.\n");

        let label = add_comment(&fs, path, None, 0, msg("A general note.")).unwrap();

        assert_eq!(label, "comment-a");
        let contents = fs.get(path);
        assert!(contents.starts_with("Some text.[^comment-a]\n"));
        assert!(contents.contains("[^comment-a]:\n    A general note."));
    }

    #[test]
    fn add_comment_read_failure_is_write_failed() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/missing.md");

        let result = add_comment(&fs, path, None, 0, msg("Note."));

        assert_eq!(result, Err(WriteError::WriteFailed));
    }

    #[test]
    fn reply_appends_message_preserving_quotation() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/doc.md");
        fs.insert(
            path,
            "Fox.[^comment-a]\n\n[^comment-a]: > Fox.\n\n    First.\n",
        );

        reply(&fs, path, "comment-a", msg("Second.")).unwrap();

        let contents = fs.get(path);
        assert!(contents.contains("First."));
        assert!(contents.contains("💬 Second."));
        assert!(contents.contains("> Fox."));
    }

    #[test]
    fn reply_to_missing_label_is_anchor_failed() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/doc.md");
        fs.insert(path, "Plain text.\n");

        let result = reply(&fs, path, "comment-a", msg("Second."));

        assert_eq!(result, Err(WriteError::AnchorFailed));
    }

    #[test]
    fn edit_last_message_replaces_body() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/doc.md");
        fs.insert(path, "Fox.[^comment-a]\n\n[^comment-a]: Original.\n");

        edit_last_message(&fs, path, "comment-a", "Edited.".to_string()).unwrap();

        let contents = fs.get(path);
        assert!(contents.contains("Edited."));
        assert!(!contents.contains("Original."));
    }

    #[test]
    fn delete_comment_removes_marker_and_definition() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/doc.md");
        fs.insert(path, "Fox.[^comment-a]\n\n[^comment-a]: Note.\n");

        delete_comment(&fs, path, "comment-a").unwrap();

        let contents = fs.get(path);
        assert!(!contents.contains("comment-a"));
        assert!(contents.contains("Fox."));
    }

    #[test]
    fn delete_missing_comment_is_anchor_failed() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/doc.md");
        fs.insert(path, "Plain text.\n");

        assert_eq!(
            delete_comment(&fs, path, "comment-a"),
            Err(WriteError::AnchorFailed)
        );
    }

    #[test]
    fn delete_last_message_removes_whole_comment_when_it_was_the_only_one() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/doc.md");
        fs.insert(path, "Fox.[^comment-a]\n\n[^comment-a]: Only message.\n");

        delete_last_message(&fs, path, "comment-a").unwrap();

        let contents = fs.get(path);
        assert!(!contents.contains("comment-a"));
    }

    #[test]
    fn delete_last_message_keeps_earlier_messages_in_a_thread() {
        let fs = InMemoryFileSystem::new();
        let path = Path::new("/doc.md");
        fs.insert(
            path,
            "Fox.[^comment-a]\n\n[^comment-a]: 💬 First.\n\n    💬 Second.\n",
        );

        delete_last_message(&fs, path, "comment-a").unwrap();

        let contents = fs.get(path);
        assert!(contents.contains("comment-a")); // comment survives
        assert!(contents.contains("First."));
        assert!(!contents.contains("Second."));
    }

    #[test]
    fn real_file_system_add_then_delete_round_trips() {
        let dir =
            std::env::temp_dir().join(format!("mudl-comments-write-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        let original = "The quick brown fox.\n";
        std::fs::write(&path, original).unwrap();

        let real = RealFileSystem;
        let label =
            add_comment(&real, &path, Some("The quick brown fox."), 0, msg("Note.")).unwrap();
        delete_comment(&real, &path, &label).unwrap();
        let restored = std::fs::read_to_string(&path).unwrap();

        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(restored, original);
    }
}

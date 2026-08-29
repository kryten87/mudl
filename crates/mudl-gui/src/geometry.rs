//! Window geometry persistence (Phase 10.7 of
//! `docs/IMPLEMENTATION-PLAN.md`): save width/height/position keyed by
//! file path on close, restore on open.
//!
//! Stored in its own on-disk file rather than folded into
//! `mudl_config::Preferences`: `Preferences`' schema is a fixed set of
//! known keys round-tripped as a whole (`to_entries`/`from_entries`),
//! which can't represent an open-ended number of per-path entries without
//! either the whole model changing shape or every `mudl_config::save`
//! silently dropping any geometry keys `Preferences` doesn't recognize.
//! Reusing the same flat `key = value` format (`mudl_config::format`) and
//! `mudl_config::FileSystem` trait avoids inventing new infrastructure for
//! this rather than a new file format.
//!
//! Phase 10.6 put multiple files in one shared window (a `gtk::Notebook`),
//! not one window per file — so "keyed by file path" now means keyed by
//! the *first* open tab's path, the window's natural anchor identity.

use std::path::Path;

use mudl_config::FileSystem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
}

impl Geometry {
    fn to_value(self) -> String {
        format!("{},{},{},{}", self.width, self.height, self.x, self.y)
    }

    fn from_value(value: &str) -> Option<Self> {
        let mut parts = value.split(',');
        let width = parts.next()?.parse().ok()?;
        let height = parts.next()?.parse().ok()?;
        let x = parts.next()?.parse().ok()?;
        let y = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            width,
            height,
            x,
            y,
        })
    }
}

/// The key a document path is stored under. Real-world paths essentially
/// never contain `=`, the one character `mudl_config::format` treats
/// specially in a key (it splits key from value on the *first* `=`) — a
/// path that did would still parse, just as a truncated/wrong key: an
/// accepted, known limitation rather than a percent-encoding scheme's
/// worth of complexity for a cosmetic edge case.
fn key_for(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Looks up `path`'s saved geometry, if any. A missing file, an absent
/// key, or a key present but not parseable as four comma-separated
/// integers are all treated the same: "nothing saved" — the caller falls
/// back to a default size/position either way.
pub fn load(fs: &dyn FileSystem, geometry_path: &Path, path: &Path) -> Option<Geometry> {
    let bytes = fs.read(geometry_path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let entries = mudl_config::format::parse(&text);
    let key = key_for(path);
    entries
        .into_iter()
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| Geometry::from_value(&v))
}

/// Saves `geometry` for `path`, preserving every other path's existing
/// entry in the same file — re-read fresh first (never trust in-memory
/// state, the same principle the plan uses for `mudl-config` itself and,
/// later, comment writes) rather than assuming this process is the only
/// writer of the file.
pub fn save(
    fs: &dyn FileSystem,
    geometry_path: &Path,
    path: &Path,
    geometry: Geometry,
) -> std::io::Result<()> {
    let mut entries = match fs.read(geometry_path) {
        Ok(bytes) => mudl_config::format::parse(&String::from_utf8_lossy(&bytes)),
        Err(_) => Vec::new(),
    };
    let key = key_for(path);
    entries.retain(|(k, _)| *k != key);
    entries.push((key, geometry.to_value()));
    let text = mudl_config::format::serialize(&entries);
    fs.write_atomic(geometry_path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mudl_config::InMemoryFileSystem;

    fn sample() -> Geometry {
        Geometry {
            width: 960,
            height: 720,
            x: 100,
            y: 50,
        }
    }

    #[test]
    fn missing_file_has_no_saved_geometry() {
        let fs = InMemoryFileSystem::new();
        assert_eq!(load(&fs, Path::new("/geo"), Path::new("/docs/a.md")), None);
    }

    #[test]
    fn save_then_load_round_trips() {
        let fs = InMemoryFileSystem::new();
        save(&fs, Path::new("/geo"), Path::new("/docs/a.md"), sample()).unwrap();
        assert_eq!(
            load(&fs, Path::new("/geo"), Path::new("/docs/a.md")),
            Some(sample())
        );
    }

    #[test]
    fn saving_one_path_preserves_another_paths_entry() {
        let fs = InMemoryFileSystem::new();
        save(&fs, Path::new("/geo"), Path::new("/docs/a.md"), sample()).unwrap();
        let other = Geometry {
            width: 800,
            height: 600,
            x: 0,
            y: 0,
        };
        save(&fs, Path::new("/geo"), Path::new("/docs/b.md"), other).unwrap();

        assert_eq!(
            load(&fs, Path::new("/geo"), Path::new("/docs/a.md")),
            Some(sample())
        );
        assert_eq!(
            load(&fs, Path::new("/geo"), Path::new("/docs/b.md")),
            Some(other)
        );
    }

    #[test]
    fn resaving_the_same_path_overwrites_its_entry() {
        let fs = InMemoryFileSystem::new();
        save(&fs, Path::new("/geo"), Path::new("/docs/a.md"), sample()).unwrap();
        let updated = Geometry {
            width: 1000,
            height: 800,
            x: 10,
            y: 10,
        };
        save(&fs, Path::new("/geo"), Path::new("/docs/a.md"), updated).unwrap();
        assert_eq!(
            load(&fs, Path::new("/geo"), Path::new("/docs/a.md")),
            Some(updated)
        );
    }

    #[test]
    fn malformed_value_is_treated_as_no_saved_geometry() {
        let fs = InMemoryFileSystem::new();
        fs.insert("/geo", b"/docs/a.md=not-a-geometry".to_vec());
        assert_eq!(load(&fs, Path::new("/geo"), Path::new("/docs/a.md")), None);
    }
}

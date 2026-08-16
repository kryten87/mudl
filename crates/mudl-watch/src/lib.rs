//! File-change detection (`mudl-watch`, Phase 6 of
//! `docs/IMPLEMENTATION-PLAN.md`).
//!
//! This crate defines the `ChangeSource` trait (§5.2's DI boundary) and its
//! polling implementation, `PollingChangeSource` (step 6.1), plus the
//! `FileSystem`/`Clock` traits it's built from. Background-thread wiring
//! (`spawn`/`WatchHandle`, step 6.2) is deliberately left for a follow-up
//! step — only the pure, unit-testable polling logic lives here.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// A single detected change to a watched file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeEvent {
    Changed(SystemTime),
    Removed,
}

/// The DI boundary (§5.2) for reading a watched file's modification time.
/// Scoped to just what `mudl-watch` needs — mirrors `mudl-server`'s own
/// narrowly-scoped `FileSystem` trait rather than a single shared
/// mega-trait covering every filesystem operation any crate might need.
pub trait FileSystem: Send + Sync {
    fn metadata_modified(&self, path: &Path) -> io::Result<SystemTime>;
}

/// A thin wrapper over `std::fs::metadata(path)?.modified()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn metadata_modified(&self, path: &Path) -> io::Result<SystemTime> {
        std::fs::metadata(path)?.modified()
    }
}

/// A `HashMap`-backed in-memory fake, for tests: never touches the real
/// filesystem. A path absent from the map (or explicitly [`remove`d]
/// (InMemoryFileSystem::remove)) reports a `NotFound` error, mirroring the
/// real behavior of a deleted file.
#[derive(Debug, Clone, Default)]
pub struct InMemoryFileSystem {
    mtimes: Arc<Mutex<HashMap<PathBuf, SystemTime>>>,
}

impl InMemoryFileSystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `path` as present with modification time `mtime`.
    pub fn set(&self, path: impl Into<PathBuf>, mtime: SystemTime) {
        self.mtimes.lock().unwrap().insert(path.into(), mtime);
    }

    /// Simulates deletion: subsequent `metadata_modified` calls for `path`
    /// report `NotFound` until it's `set` again.
    pub fn remove(&self, path: impl AsRef<Path>) {
        self.mtimes.lock().unwrap().remove(path.as_ref());
    }
}

impl FileSystem for InMemoryFileSystem {
    fn metadata_modified(&self, path: &Path) -> io::Result<SystemTime> {
        self.mtimes
            .lock()
            .unwrap()
            .get(path)
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found"))
    }
}

/// The DI boundary (§5.2) for reading the current time, so
/// `PollingChangeSource`'s interval-gating logic is testable without real
/// sleeping.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// A thin wrapper over `SystemTime::now()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A settable/advanceable fake clock, for tests: no real sleeping, ever.
#[derive(Debug, Clone)]
pub struct FakeClock {
    now: Arc<Mutex<SystemTime>>,
}

impl FakeClock {
    pub fn new(now: SystemTime) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    pub fn set(&self, now: SystemTime) {
        *self.now.lock().unwrap() = now;
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.now.lock().unwrap();
        *now += by;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        *self.now.lock().unwrap()
    }
}

/// A source of file-change notifications, polled by a caller (eventually
/// on a background thread — see the follow-up `spawn`/`WatchHandle` step)
/// rather than pushing events itself. Keeping it pull-based is what makes
/// [`PollingChangeSource`] trivially unit-testable: no threads, no
/// channels, just call `poll()` and check the return value.
///
/// # Why polling, not `inotify` (deferred; see §3's dependency table)
///
/// The only implementation of this trait today is [`PollingChangeSource`],
/// which checks [`FileSystem::metadata_modified`] on a timer rather than
/// subscribing to kernel-level filesystem-change notifications. That's a
/// deliberate simplicity-over-latency tradeoff for v1, not an oversight:
///
/// - Polling needs no additional dependency at all — not even `libc`, let
///   alone an `inotify`-wrapping crate — matching this project's default
///   preference for `std` unless the third-party alternative avoids
///   re-deriving a large, correctness-critical spec by hand (§3), which
///   `inotify`'s event semantics are not.
/// - `ChangeSource` being a trait that consumers (`mudl-server`) depend on,
///   rather than `PollingChangeSource` being used directly, means an
///   `inotify`-backed implementation can be swapped in later without
///   touching any consumer — the trait boundary is exactly what makes that
///   swap free when/if it's worth doing.
/// - A markdown viewer reloading some tunable interval after a save (not
///   instantly) is imperceptible in practice; today, the latency polling
///   gives up isn't worth the added dependency and platform-specific code.
///   This is flagged as an explicit future optimization, not a v1
///   requirement.
pub trait ChangeSource: Send {
    fn poll(&mut self) -> Option<ChangeEvent>;
}

/// Last-known state of the watched path, as observed by
/// [`PollingChangeSource`]'s filesystem checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Never checked yet — the very next successful check is the baseline
    /// and reports no change.
    Baseline,
    Present(SystemTime),
    Absent,
}

/// Checks a watched path's modification time on an interval (§5.2's
/// `Clock`/`FileSystem` traits injected, per the plan's step 6.1), reporting
/// a [`ChangeEvent`] when it changes, disappears, or reappears.
pub struct PollingChangeSource<F: FileSystem, C: Clock> {
    path: PathBuf,
    filesystem: F,
    clock: C,
    interval: Duration,
    state: State,
    /// `None` until the first `poll()`, so the interval gate is treated as
    /// already open on construction — the very first call always checks.
    last_checked: Option<SystemTime>,
}

impl<F: FileSystem, C: Clock> PollingChangeSource<F, C> {
    pub fn new(path: PathBuf, filesystem: F, clock: C, interval: Duration) -> Self {
        Self {
            path,
            filesystem,
            clock,
            interval,
            state: State::Baseline,
            last_checked: None,
        }
    }
}

impl<F: FileSystem, C: Clock> ChangeSource for PollingChangeSource<F, C> {
    fn poll(&mut self) -> Option<ChangeEvent> {
        let now = self.clock.now();
        if let Some(last_checked) = self.last_checked {
            // `duration_since` errors if the clock went backwards; treating
            // that as "no time has passed" keeps the gate closed rather
            // than panicking or under-gating.
            let elapsed = now.duration_since(last_checked).unwrap_or(Duration::ZERO);
            if elapsed < self.interval {
                return None;
            }
        }
        self.last_checked = Some(now);

        match self.filesystem.metadata_modified(&self.path) {
            Ok(mtime) => match self.state {
                State::Baseline => {
                    self.state = State::Present(mtime);
                    None
                }
                State::Present(prev) if prev == mtime => None,
                State::Present(_) | State::Absent => {
                    self.state = State::Present(mtime);
                    Some(ChangeEvent::Changed(mtime))
                }
            },
            Err(_) => match self.state {
                State::Absent => None,
                State::Baseline | State::Present(_) => {
                    self.state = State::Absent;
                    Some(ChangeEvent::Removed)
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch_plus(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn source(
        path: &str,
        filesystem: &InMemoryFileSystem,
        clock: &FakeClock,
        interval: Duration,
    ) -> PollingChangeSource<InMemoryFileSystem, FakeClock> {
        PollingChangeSource::new(
            PathBuf::from(path),
            filesystem.clone(),
            clock.clone(),
            interval,
        )
    }

    #[test]
    fn first_poll_after_construction_reports_no_change() {
        let fs = InMemoryFileSystem::new();
        fs.set("/doc.md", epoch_plus(1));
        let clock = FakeClock::new(epoch_plus(100));
        let mut src = source("/doc.md", &fs, &clock, Duration::from_millis(300));

        assert_eq!(src.poll(), None);
    }

    #[test]
    fn mtime_advance_past_interval_reports_changed() {
        let fs = InMemoryFileSystem::new();
        fs.set("/doc.md", epoch_plus(1));
        let clock = FakeClock::new(epoch_plus(100));
        let interval = Duration::from_millis(300);
        let mut src = source("/doc.md", &fs, &clock, interval);
        assert_eq!(src.poll(), None); // baseline

        clock.advance(interval);
        fs.set("/doc.md", epoch_plus(2));

        assert_eq!(src.poll(), Some(ChangeEvent::Changed(epoch_plus(2))));
    }

    #[test]
    fn unchanged_mtime_across_multiple_polls_reports_none() {
        let fs = InMemoryFileSystem::new();
        fs.set("/doc.md", epoch_plus(1));
        let clock = FakeClock::new(epoch_plus(100));
        let interval = Duration::from_millis(300);
        let mut src = source("/doc.md", &fs, &clock, interval);
        assert_eq!(src.poll(), None); // baseline

        for _ in 0..3 {
            clock.advance(interval);
            assert_eq!(src.poll(), None);
        }
    }

    #[test]
    fn poll_before_interval_elapsed_does_not_check_filesystem() {
        let fs = InMemoryFileSystem::new();
        fs.set("/doc.md", epoch_plus(1));
        let clock = FakeClock::new(epoch_plus(100));
        let interval = Duration::from_millis(300);
        let mut src = source("/doc.md", &fs, &clock, interval);
        assert_eq!(src.poll(), None); // baseline

        // The clock is NOT advanced, but the file changes underneath — if
        // `poll` actually checked the filesystem here it would report
        // `Changed`. It must not: the interval gate keeps it from even
        // looking.
        fs.set("/doc.md", epoch_plus(999));

        assert_eq!(src.poll(), None);
    }

    #[test]
    fn disappearing_file_is_reported_as_removed() {
        let fs = InMemoryFileSystem::new();
        fs.set("/doc.md", epoch_plus(1));
        let clock = FakeClock::new(epoch_plus(100));
        let interval = Duration::from_millis(300);
        let mut src = source("/doc.md", &fs, &clock, interval);
        assert_eq!(src.poll(), None); // baseline

        clock.advance(interval);
        fs.remove("/doc.md");

        assert_eq!(src.poll(), Some(ChangeEvent::Removed));
    }

    #[test]
    fn poll_after_removal_while_still_absent_reports_none() {
        let fs = InMemoryFileSystem::new();
        fs.set("/doc.md", epoch_plus(1));
        let clock = FakeClock::new(epoch_plus(100));
        let interval = Duration::from_millis(300);
        let mut src = source("/doc.md", &fs, &clock, interval);
        assert_eq!(src.poll(), None); // baseline
        clock.advance(interval);
        fs.remove("/doc.md");
        assert_eq!(src.poll(), Some(ChangeEvent::Removed));

        clock.advance(interval);
        assert_eq!(src.poll(), None);
    }

    #[test]
    fn reappearing_file_after_removal_reports_changed() {
        let fs = InMemoryFileSystem::new();
        fs.set("/doc.md", epoch_plus(1));
        let clock = FakeClock::new(epoch_plus(100));
        let interval = Duration::from_millis(300);
        let mut src = source("/doc.md", &fs, &clock, interval);
        assert_eq!(src.poll(), None); // baseline
        clock.advance(interval);
        fs.remove("/doc.md");
        assert_eq!(src.poll(), Some(ChangeEvent::Removed));

        clock.advance(interval);
        fs.set("/doc.md", epoch_plus(50));

        assert_eq!(src.poll(), Some(ChangeEvent::Changed(epoch_plus(50))));
    }

    #[test]
    fn in_memory_fs_missing_path_is_not_found() {
        let fs = InMemoryFileSystem::new();
        let err = fs.metadata_modified(Path::new("/missing.md")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn real_fs_reads_a_real_files_mtime() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mudl-watch-test-{}", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();

        let real = RealFileSystem;
        let result = real.metadata_modified(&path);

        std::fs::remove_file(&path).ok();
        assert!(result.is_ok());
    }

    #[test]
    fn real_clock_now_is_close_to_system_time_now() {
        let before = SystemTime::now();
        let observed = RealClock.now();
        let after = SystemTime::now();
        assert!(observed >= before && observed <= after);
    }

    #[test]
    fn fake_clock_set_and_advance() {
        let clock = FakeClock::new(epoch_plus(10));
        assert_eq!(clock.now(), epoch_plus(10));

        clock.advance(Duration::from_secs(5));
        assert_eq!(clock.now(), epoch_plus(15));

        clock.set(epoch_plus(0));
        assert_eq!(clock.now(), epoch_plus(0));
    }
}

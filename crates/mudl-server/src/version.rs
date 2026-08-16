//! The `Condvar`-based version counter for the `/wait` long-poll route
//! (Phase 4, step 4.6 of `docs/IMPLEMENTATION-PLAN.md`; design rationale in
//! §2, "Live reload without extra dependencies").
//!
//! This is deliberately a plain struct wrapping real shared state, not a
//! trait: §5.2's DI boundary is for swappable *implementations* (a fake
//! clock, an in-memory filesystem); `VersionCounter` has exactly one
//! implementation, and tests exercise it directly by constructing one and
//! bumping it from another thread.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A shared, thread-safe counter that starts at `0` and only ever
/// increases. `mudl-server` hands out clones of this (cheap: an `Arc`
/// clone) to connection-handling threads so a long-polling `/wait` request
/// and whatever bumps the counter (eventually Phase 6's file watcher) can
/// coordinate through the same `Condvar`.
#[derive(Clone, Debug)]
pub struct VersionCounter {
    inner: Arc<(Mutex<u64>, Condvar)>,
}

impl VersionCounter {
    pub fn new() -> Self {
        Self {
            inner: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }

    /// Advances the version by one and wakes every thread blocked in
    /// `wait_for_change`.
    pub fn bump(&self) {
        let (lock, condvar) = &*self.inner;
        let mut version = lock.lock().unwrap();
        *version += 1;
        condvar.notify_all();
    }

    pub fn current(&self) -> u64 {
        let (lock, _) = &*self.inner;
        *lock.lock().unwrap()
    }

    /// Blocks until the version advances past `since`, or `timeout`
    /// elapses first. Returns the new version in the former case; returns
    /// `since` unchanged in the latter, so a caller can tell "nothing
    /// happened" from "something happened" without a separate flag.
    pub fn wait_for_change(&self, since: u64, timeout: Duration) -> u64 {
        let (lock, condvar) = &*self.inner;
        let mut version = lock.lock().unwrap();
        let deadline = Instant::now() + timeout;
        while *version <= since {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return since;
            }
            version = condvar.wait_timeout(version, remaining).unwrap().0;
        }
        *version
    }
}

impl Default for VersionCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn new_counter_starts_at_zero() {
        assert_eq!(VersionCounter::new().current(), 0);
    }

    #[test]
    fn bump_increments_current_version() {
        let counter = VersionCounter::new();
        counter.bump();
        assert_eq!(counter.current(), 1);
        counter.bump();
        assert_eq!(counter.current(), 2);
    }

    #[test]
    fn wait_for_change_times_out_with_unchanged_version_when_nothing_happens() {
        let counter = VersionCounter::new();
        let since = counter.current();
        let start = Instant::now();
        let result = counter.wait_for_change(since, Duration::from_millis(100));
        assert_eq!(result, since);
        assert!(start.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn wait_for_change_returns_immediately_when_since_is_already_behind() {
        let counter = VersionCounter::new();
        counter.bump();
        let current = counter.current();
        let start = Instant::now();
        let result = counter.wait_for_change(0, Duration::from_secs(5));
        assert_eq!(result, current);
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn wait_for_change_unblocks_promptly_when_bumped_from_another_thread() {
        let counter = VersionCounter::new();
        let since = counter.current();
        let bumper = counter.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            bumper.bump();
        });

        let start = Instant::now();
        let result = counter.wait_for_change(since, Duration::from_secs(5));
        assert_eq!(result, since + 1);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}

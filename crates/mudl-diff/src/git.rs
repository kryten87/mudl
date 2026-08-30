//! Git-sourced waypoints (ported from `mud`'s `App/GitProvider.swift`,
//! Phase 13.7).
//!
//! [`GitRunner`] is the DI boundary (§5.2) for a single git invocation,
//! mirroring `mud`'s `GitProvider.Runner` closure: production code gets
//! [`RealGitRunner`] (a thin `std::process::Command` wrapper); tests get
//! [`ScriptedGitRunner`], keyed on the exact argument list like the Swift
//! suite's `responses` dictionary keyed on the joined arguments. Parsing
//! git's output into [`CommitInfo`]/[`WaypointCandidate`] is pure and
//! tested with no runner at all.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// Runs one git invocation and returns its exit code and (UTF-8) stdout.
/// A non-zero exit is not an error at this layer — callers decide what a
/// given command's failure means (e.g. "not a git repository" vs. "no
/// unstaged changes").
pub trait GitRunner {
    fn run(&self, args: &[&str], cwd: &Path) -> io::Result<(i32, String)>;
}

/// Spawns the system `git` binary.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealGitRunner;

impl GitRunner for RealGitRunner {
    fn run(&self, args: &[&str], cwd: &Path) -> io::Result<(i32, String)> {
        let output = Command::new("git").args(args).current_dir(cwd).output()?;
        let stdout = String::from_utf8_lossy(&output.stdout)
            .trim_matches(|c| c == '\n' || c == '\r')
            .to_string();
        Ok((output.status.code().unwrap_or(-1), stdout))
    }
}

/// A scripted fake keyed on the exact argument list, for tests. An
/// unscripted invocation fails like a real git error (exit 128, no
/// output), matching the Swift suite's `responses[key] ?? (128, nil)`.
#[derive(Debug, Clone, Default)]
pub struct ScriptedGitRunner {
    responses: HashMap<Vec<String>, (i32, String)>,
}

impl ScriptedGitRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scripts a response for exactly this argument list.
    pub fn script(mut self, args: &[&str], status: i32, output: &str) -> Self {
        let key = args.iter().map(|s| s.to_string()).collect();
        self.responses.insert(key, (status, output.to_string()));
        self
    }
}

impl GitRunner for ScriptedGitRunner {
    fn run(&self, args: &[&str], _cwd: &Path) -> io::Result<(i32, String)> {
        let key: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        Ok(self
            .responses
            .get(&key)
            .cloned()
            .unwrap_or((128, String::new())))
    }
}

/// One commit touching the watched file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub hash: String,
    pub timestamp: SystemTime,
    pub message: String,
}

/// A candidate entry for the "Changes since…" menu: a piece of git history
/// with the file's content at that point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaypointCandidate {
    pub label: String,
    pub detail: Option<String>,
    pub content: String,
    pub timestamp: SystemTime,
}

/// Parses `git log --format=%H%x00%aI%x00%s` output (one commit per line,
/// fields NUL-separated) into [`CommitInfo`]s. A line that fails to parse
/// (wrong field count) is skipped; a date that fails to parse falls back
/// to "now", matching the Swift source's `?? Date()`.
pub fn parse_commit_log(output: &str) -> Vec<CommitInfo> {
    if output.is_empty() {
        return Vec::new();
    }
    output
        .split('\n')
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(3, '\u{0}').collect();
            if parts.len() != 3 {
                return None;
            }
            Some(CommitInfo {
                hash: parts[0].to_string(),
                timestamp: parse_iso8601(parts[1]).unwrap_or_else(SystemTime::now),
                message: parts[2].to_string(),
            })
        })
        .collect()
}

/// Scrapes the undocumented `mtime: <seconds>:<nanoseconds>` line from
/// `git ls-files --debug` output. Only the seconds half is kept, matching
/// the Swift source.
pub fn parse_index_mtime(ls_files_debug_output: &str) -> Option<SystemTime> {
    for line in ls_files_debug_output.split('\n') {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("mtime:") {
            let value = rest.trim();
            let secs: u64 = value.split(':').next()?.parse().ok()?;
            return Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
        }
    }
    None
}

/// Parses an ISO 8601 timestamp (`%aI` / `ISO8601DateFormatter` output):
/// `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)`. Returns `None` on any
/// malformed input rather than panicking.
fn parse_iso8601(s: &str) -> Option<SystemTime> {
    if s.len() < 20 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    if s.get(4..5)? != "-" {
        return None;
    }
    let month: u32 = s.get(5..7)?.parse().ok()?;
    if s.get(7..8)? != "-" {
        return None;
    }
    let day: u32 = s.get(8..10)?.parse().ok()?;
    if s.get(10..11)? != "T" {
        return None;
    }
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    if s.get(13..14)? != ":" {
        return None;
    }
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    if s.get(16..17)? != ":" {
        return None;
    }
    let second: i64 = s.get(17..19)?.parse().ok()?;

    let rest = &s[19..];
    let (frac_digits, rest) = match rest.strip_prefix('.') {
        Some(after_dot) => {
            let digit_len = after_dot
                .char_indices()
                .find(|&(_, c)| !c.is_ascii_digit())
                .map(|(i, _)| i)
                .unwrap_or(after_dot.len());
            (&after_dot[..digit_len], &after_dot[digit_len..])
        }
        None => ("", rest),
    };
    let nanos: u32 = if frac_digits.is_empty() {
        0
    } else {
        let mut digits = frac_digits.to_string();
        digits.truncate(9);
        while digits.len() < 9 {
            digits.push('0');
        }
        digits.parse().ok()?
    };

    let offset_seconds: i64 = if rest == "Z" {
        0
    } else if rest.len() >= 3 {
        let sign = match rest.as_bytes()[0] {
            b'+' => 1i64,
            b'-' => -1i64,
            _ => return None,
        };
        let digits: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() < 4 {
            return None;
        }
        let oh: i64 = digits[0..2].parse().ok()?;
        let om: i64 = digits[2..4].parse().ok()?;
        sign * (oh * 3600 + om * 60)
    } else {
        return None;
    };

    let days = days_from_civil(year, month, day);
    let total_seconds = days
        .checked_mul(86400)?
        .checked_add(hour * 3600 + minute * 60 + second - offset_seconds)?;

    if total_seconds >= 0 {
        Some(SystemTime::UNIX_EPOCH + Duration::new(total_seconds as u64, nanos))
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(Duration::new((-total_seconds) as u64, 0))
    }
}

/// Days since the Unix epoch (1970-01-01) for a proleptic-Gregorian civil
/// date. Howard Hinnant's `days_from_civil` algorithm — a standard,
/// well-known calendar routine, hand-rolled here rather than pulling in a
/// date/time crate for one calculation.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Builds the deduplicated waypoint list from already-fetched content:
/// pure business logic, no git invocation. `staged` is `None` unless the
/// caller has already established there are unstaged changes and fetched
/// the staged content; `commits` is in the reverse-chronological order
/// `git log` returns, each paired with its content at that commit.
pub fn build_waypoint_candidates(
    current_content: &str,
    staged: Option<(String, SystemTime)>,
    commits: &[(CommitInfo, String)],
) -> Vec<WaypointCandidate> {
    let mut result = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    seen.insert(current_content);

    let head_content = commits.first().map(|(_, content)| content.as_str());

    if let Some((staged_content, staged_mtime)) = &staged {
        if !seen.contains(staged_content.as_str()) {
            let differs_from_head = head_content.map(|h| h != staged_content).unwrap_or(true);
            if differs_from_head {
                seen.insert(staged_content.as_str());
                result.push(WaypointCandidate {
                    label: "since last staged".to_string(),
                    detail: None,
                    content: staged_content.clone(),
                    timestamp: *staged_mtime,
                });
            }
        }
    }

    for (commit, content) in commits {
        if seen.contains(content.as_str()) {
            continue;
        }
        seen.insert(content.as_str());
        let short_hash: String = commit.hash.chars().take(7).collect();
        result.push(WaypointCandidate {
            label: format!("since commit {short_hash}"),
            detail: Some(commit.message.clone()),
            content: content.clone(),
            timestamp: commit.timestamp,
        });
    }

    result
}

/// The path of `file_path` relative to `repo_root`, falling back to just
/// the file name when `file_path` isn't under `repo_root`.
fn relative_path(file_path: &Path, repo_root: &Path) -> String {
    match file_path.strip_prefix(repo_root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

fn run_ok(runner: &dyn GitRunner, args: &[&str], cwd: &Path) -> Option<String> {
    match runner.run(args, cwd) {
        Ok((0, out)) => Some(out),
        _ => None,
    }
}

/// Queries `runner` for `file_path`'s git history and returns waypoint
/// candidates for the "Changes since…" menu. Returns an empty list when
/// `file_path` isn't inside a git repository. Every git call can fail
/// independently, matching the Swift source's `try?`-per-call approach.
pub fn query_waypoints(
    runner: &dyn GitRunner,
    file_path: &Path,
    current_content: &str,
) -> Vec<WaypointCandidate> {
    let parent = file_path.parent().unwrap_or_else(|| Path::new("."));
    let Some(root_out) = run_ok(runner, &["rev-parse", "--show-toplevel"], parent) else {
        return Vec::new();
    };
    let repo_root = PathBuf::from(root_out.trim());
    let rel_path = relative_path(file_path, &repo_root);

    let staged_content = run_ok(runner, &["show", &format!(":{rel_path}")], &repo_root);
    let staged_mtime = run_ok(
        runner,
        &["ls-files", "--debug", "--", &rel_path],
        &repo_root,
    )
    .and_then(|out| parse_index_mtime(&out));

    let log_out = run_ok(
        runner,
        &[
            "log",
            "--format=%H%x00%aI%x00%s",
            "-n",
            "5",
            "--",
            &rel_path,
        ],
        &repo_root,
    )
    .unwrap_or_default();

    let mut commits: Vec<(CommitInfo, String)> = Vec::new();
    for info in parse_commit_log(&log_out) {
        let show_arg = format!("{}:{}", info.hash, rel_path);
        if let Some(content) = run_ok(runner, &["show", &show_arg], &repo_root) {
            commits.push((info, content));
        }
    }

    let has_unstaged_changes = match runner.run(&["diff", "--quiet", "--", &rel_path], &repo_root) {
        Ok((status, _)) => status != 0,
        Err(_) => true,
    };

    let staged = if has_unstaged_changes {
        staged_content.map(|content| (content, staged_mtime.unwrap_or_else(SystemTime::now)))
    } else {
        None
    };

    build_waypoint_candidates(current_content, staged, &commits)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn base_runner() -> ScriptedGitRunner {
        ScriptedGitRunner::new()
            .script(&["rev-parse", "--show-toplevel"], 0, "/repo")
            .script(&["show", ":notes.md"], 0, "staged content")
            .script(&["diff", "--quiet", "--", "notes.md"], 1, "")
            .script(
                &["ls-files", "--debug", "--", "notes.md"],
                0,
                "notes.md\n  ctime: 1699000000:0\n  mtime: 1700000000:123456789\n  dev: 16777231\tino: 8631556\n  uid: 501\tgid: 20\n  size: 42\tflags: 0",
            )
            .script(
                &["log", "--format=%H%x00%aI%x00%s", "-n", "5", "--", "notes.md"],
                0,
                &format!(
                    "{HASH_A}\u{0}2026-07-01T12:00:00+00:00\u{0}Newest commit\n{HASH_B}\u{0}2026-06-30T08:15:30.123+00:00\u{0}Older commit"
                ),
            )
            .script(&["show", &format!("{HASH_A}:notes.md")], 0, "head content")
            .script(&["show", &format!("{HASH_B}:notes.md")], 0, "older content")
    }

    fn labels(candidates: &[WaypointCandidate]) -> Vec<&str> {
        candidates.iter().map(|w| w.label.as_str()).collect()
    }

    // --- parse_iso8601 (via parse_commit_log's timestamps) ---

    #[test]
    fn commit_dates_parse_with_and_without_fractional_seconds() {
        let log = format!(
            "{HASH_A}\u{0}2026-07-01T12:00:00+00:00\u{0}Newest commit\n{HASH_B}\u{0}2026-06-30T08:15:30.123+00:00\u{0}Older commit"
        );
        let commits = parse_commit_log(&log);
        assert_eq!(
            commits[0].timestamp,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_782_907_200)
        );
        assert_eq!(
            commits[1].timestamp,
            SystemTime::UNIX_EPOCH + Duration::new(1_782_807_330, 123_000_000)
        );
        assert_eq!(commits[0].message, "Newest commit");
        assert_eq!(commits[1].message, "Older commit");
    }

    #[test]
    fn malformed_log_line_is_skipped() {
        assert!(parse_commit_log("not-enough-fields").is_empty());
    }

    #[test]
    fn empty_log_output_is_empty() {
        assert!(parse_commit_log("").is_empty());
    }

    // --- parse_index_mtime ---

    #[test]
    fn staged_timestamp_comes_from_the_index_mtime() {
        let out = "notes.md\n  ctime: 1699000000:0\n  mtime: 1700000000:123456789\n";
        assert_eq!(
            parse_index_mtime(out),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
    }

    #[test]
    fn missing_mtime_line_is_none() {
        assert_eq!(parse_index_mtime("notes.md\n  ctime: 1699000000:0\n"), None);
    }

    // --- build_waypoint_candidates ---

    #[test]
    fn builds_staged_then_commit_waypoints() {
        let commit_a = CommitInfo {
            hash: HASH_A.to_string(),
            timestamp: SystemTime::UNIX_EPOCH,
            message: "Newest commit".to_string(),
        };
        let commit_b = CommitInfo {
            hash: HASH_B.to_string(),
            timestamp: SystemTime::UNIX_EPOCH,
            message: "Older commit".to_string(),
        };
        let commits = vec![
            (commit_a, "head content".to_string()),
            (commit_b, "older content".to_string()),
        ];
        let candidates = build_waypoint_candidates(
            "current",
            Some(("staged content".to_string(), SystemTime::UNIX_EPOCH)),
            &commits,
        );
        assert_eq!(
            labels(&candidates),
            vec![
                "since last staged",
                "since commit aaaaaaa",
                "since commit bbbbbbb",
            ]
        );
        assert_eq!(
            candidates
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec!["staged content", "head content", "older content"]
        );
    }

    #[test]
    fn staged_is_skipped_when_it_matches_head() {
        let commits = vec![(
            CommitInfo {
                hash: HASH_A.to_string(),
                timestamp: SystemTime::UNIX_EPOCH,
                message: "m".to_string(),
            },
            "head content".to_string(),
        )];
        let candidates = build_waypoint_candidates(
            "current",
            Some(("head content".to_string(), SystemTime::UNIX_EPOCH)),
            &commits,
        );
        assert!(labels(&candidates)
            .iter()
            .all(|l| l.starts_with("since commit")));
    }

    #[test]
    fn waypoints_deduplicate_by_content() {
        let commits = vec![
            (
                CommitInfo {
                    hash: HASH_A.to_string(),
                    timestamp: SystemTime::UNIX_EPOCH,
                    message: "m".to_string(),
                },
                "head content".to_string(),
            ),
            (
                CommitInfo {
                    hash: HASH_B.to_string(),
                    timestamp: SystemTime::UNIX_EPOCH,
                    message: "m2".to_string(),
                },
                "head content".to_string(),
            ),
        ];
        let candidates = build_waypoint_candidates(
            "current",
            Some(("staged content".to_string(), SystemTime::UNIX_EPOCH)),
            &commits,
        );
        assert_eq!(
            labels(&candidates),
            vec!["since last staged", "since commit aaaaaaa"]
        );
    }

    #[test]
    fn content_matching_the_current_text_is_excluded() {
        let commits = vec![(
            CommitInfo {
                hash: HASH_A.to_string(),
                timestamp: SystemTime::UNIX_EPOCH,
                message: "m".to_string(),
            },
            "head content".to_string(),
        )];
        let candidates = build_waypoint_candidates(
            "head content",
            Some(("staged content".to_string(), SystemTime::UNIX_EPOCH)),
            &commits,
        );
        assert_eq!(labels(&candidates), vec!["since last staged"]);
    }

    #[test]
    fn no_staged_and_no_commits_is_empty() {
        assert!(build_waypoint_candidates("current", None, &[]).is_empty());
    }

    // --- query_waypoints (full orchestration over a scripted runner) ---

    #[test]
    fn full_flow_builds_staged_then_commit_waypoints() {
        let runner = base_runner();
        let candidates = query_waypoints(&runner, Path::new("/repo/notes.md"), "current");
        assert_eq!(
            labels(&candidates),
            vec![
                "since last staged",
                "since commit aaaaaaa",
                "since commit bbbbbbb",
            ]
        );
        assert_eq!(
            candidates[0].timestamp,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
        );
    }

    #[test]
    fn staged_is_skipped_without_unstaged_changes() {
        let runner = base_runner().script(&["diff", "--quiet", "--", "notes.md"], 0, "");
        let candidates = query_waypoints(&runner, Path::new("/repo/notes.md"), "current");
        assert!(labels(&candidates)
            .iter()
            .all(|l| l.starts_with("since commit")));
    }

    #[test]
    fn outside_a_repository_there_are_no_waypoints() {
        let runner = ScriptedGitRunner::new().script(&["rev-parse", "--show-toplevel"], 128, "");
        let candidates = query_waypoints(&runner, Path::new("/repo/notes.md"), "current");
        assert!(candidates.is_empty());
    }

    #[test]
    fn a_failed_log_yields_only_the_staged_waypoint() {
        let runner = base_runner().script(
            &[
                "log",
                "--format=%H%x00%aI%x00%s",
                "-n",
                "5",
                "--",
                "notes.md",
            ],
            129,
            "",
        );
        let candidates = query_waypoints(&runner, Path::new("/repo/notes.md"), "current");
        assert_eq!(labels(&candidates), vec!["since last staged"]);
    }
}

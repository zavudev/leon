//! What shows a worker's work is right: its project's check, and the CI of
//! its pull request.
//!
//! A worker says it is done; that is not evidence. Before its branch goes
//! out as a pull request, the service runs the project's own check in the
//! worktree (`scripts/check.sh`, `cargo test`, `npm test`…, or what
//! `king.toml` names under `[checks]`) and keeps what came of it as a
//! [`Check`]: the command, the commit it ran on, whether it passed and the
//! end of what it printed. `pr_open` refuses a branch whose last commit
//! passed no check (unless King says why it cannot be checked), and puts
//! the evidence in the pull request's description.
//!
//! Once the pull request is open, the service watches its CI ([`read_ci`]
//! reads what `gh pr view --json state,statusCheckRollup` prints) and King
//! hears when the checks settle: passed, so the person can review it, or
//! failed, so a worker can mend it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How much of what a check printed is kept, in characters: its end.
pub const TAIL_CHARS: usize = 4_000;

/// Where a check is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    /// It runs.
    Running,
    /// It ended well.
    Passed,
    /// It ended badly, or did not end in time.
    Failed,
}

/// A run of a project's check in a worker's worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// The command, as `sh -c` ran it.
    pub command: String,
    /// The commit it ran on.
    pub commit: String,
    /// Whether the worktree had changes not committed: then it was not
    /// the commit alone that was checked.
    pub dirty: bool,
    /// Where it is.
    pub state: CheckState,
    /// When it started, in milliseconds since the epoch.
    pub started_ms: u64,
    /// How long it took, once it ended.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// The end of what it printed, once it ended.
    #[serde(default)]
    pub tail: String,
}

impl Check {
    /// Whether it passed on `commit`, with nothing else in the worktree.
    pub fn proves(&self, commit: &str) -> bool {
        self.state == CheckState::Passed && !self.dirty && same_commit(&self.commit, commit)
    }

    /// The lines a pull request's description ends with.
    pub fn evidence(&self) -> String {
        format!(
            "Verified by Leon: `{}` passed on {} in {}.",
            self.command,
            short(&self.commit),
            duration(self.duration_ms.unwrap_or_default())
        )
    }
}

/// Where a pull request's CI is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ci {
    /// Its checks have not all ended, or have not been read yet.
    #[default]
    Pending,
    /// Every check passed (or was skipped).
    Passed,
    /// A check failed.
    Failed,
    /// It has no checks: the project runs no CI on it.
    None,
}

/// A pull request King opened for a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    /// Its address.
    pub url: String,
    /// Its number, when the address has it.
    #[serde(default)]
    pub number: Option<u64>,
    /// The branch.
    pub branch: String,
    /// When it was opened or last pushed to.
    pub pushed_ms: u64,
    /// Its CI.
    #[serde(default)]
    pub ci: Ci,
    /// The checks that failed, by name.
    #[serde(default)]
    pub failing: Vec<String>,
    /// It was merged or closed: it is not watched any more.
    #[serde(default)]
    pub closed: Option<String>,
}

impl PullRequest {
    /// A pull request just opened (or pushed to) at `url`.
    pub fn new(url: &str, branch: &str, now_ms: u64) -> Self {
        Self {
            url: url.trim().to_owned(),
            number: number_of(url),
            branch: branch.to_owned(),
            pushed_ms: now_ms,
            ci: Ci::Pending,
            failing: Vec::new(),
            closed: None,
        }
    }

    /// Whether its CI is still to be read.
    pub fn watched(&self) -> bool {
        self.ci == Ci::Pending && self.closed.is_none()
    }

    /// `#62`, or its address when it has no number.
    pub fn name(&self) -> String {
        self.number
            .map_or_else(|| self.url.clone(), |number| format!("#{number}"))
    }
}

/// What a look at a pull request found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// Its CI.
    pub ci: Ci,
    /// The checks that failed, by name.
    pub failing: Vec<String>,
    /// `merged` or `closed`, when it is not open any more.
    pub closed: Option<String>,
}

/// How long a pull request may show no checks at all before it is taken as
/// one its project runs no CI on.
pub const NO_CI_AFTER_MS: u64 = 10 * 60 * 1000;

/// Reads what `gh pr view --json state,statusCheckRollup` printed. A check
/// is a GitHub Actions run (`status`, `conclusion`) or a commit status
/// (`state`).
pub fn read_ci(json: &str) -> Result<Seen, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|error| format!("gh printed no JSON: {error}"))?;
    let closed = match value["state"].as_str() {
        Some("MERGED") => Some("merged".to_owned()),
        Some("CLOSED") => Some("closed".to_owned()),
        _ => None,
    };
    let checks = value["statusCheckRollup"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut pending = false;
    let mut failing = Vec::new();
    for check in &checks {
        let name = check["name"]
            .as_str()
            .or_else(|| check["context"].as_str())
            .unwrap_or("a check")
            .to_owned();
        let outcome = match check["state"].as_str() {
            // A commit status.
            Some(state) => match state {
                "SUCCESS" => Outcome::Passed,
                "PENDING" | "EXPECTED" => Outcome::Pending,
                _ => Outcome::Failed,
            },
            // A check run.
            None if check["status"].as_str() != Some("COMPLETED") => Outcome::Pending,
            None => match check["conclusion"].as_str().unwrap_or_default() {
                "SUCCESS" | "NEUTRAL" | "SKIPPED" => Outcome::Passed,
                _ => Outcome::Failed,
            },
        };
        match outcome {
            Outcome::Pending => pending = true,
            Outcome::Failed => failing.push(name),
            Outcome::Passed => {}
        }
    }
    let ci = if checks.is_empty() {
        Ci::Pending
    } else if !failing.is_empty() {
        Ci::Failed
    } else if pending {
        Ci::Pending
    } else {
        Ci::Passed
    };
    Ok(Seen {
        ci,
        failing,
        closed,
    })
}

enum Outcome {
    Pending,
    Passed,
    Failed,
}

/// The check of a project with these files at its root, when there is one
/// Leon knows: `has` says whether a file is there, `read` what it holds.
pub fn detect_check(
    has: impl Fn(&str) -> bool,
    read: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if has("scripts/check.sh") {
        return Some("scripts/check.sh".to_owned());
    }
    if read("Makefile").is_some_and(|text| text.lines().any(|line| line.starts_with("check:"))) {
        return Some("make check".to_owned());
    }
    if has("justfile")
        && read("justfile").is_some_and(|text| text.lines().any(|line| line.starts_with("check")))
    {
        return Some("just check".to_owned());
    }
    if has("Cargo.toml") {
        return Some("cargo test".to_owned());
    }
    if let Some(package) = read("package.json") {
        let scripts = serde_json::from_str::<Value>(&package).ok();
        let script = |name: &str| {
            scripts
                .as_ref()
                .is_some_and(|value| value["scripts"][name].is_string())
        };
        let runner = if has("pnpm-lock.yaml") {
            "pnpm"
        } else if has("yarn.lock") {
            "yarn"
        } else if has("bun.lockb") || has("bun.lock") {
            "bun"
        } else {
            "npm"
        };
        if script("check") {
            return Some(format!("{runner} run check"));
        }
        if script("test") {
            return Some(format!("{runner} test"));
        }
    }
    if has("go.mod") {
        return Some("go test ./...".to_owned());
    }
    if has("pyproject.toml") || has("pytest.ini") || has("setup.cfg") {
        return Some("pytest".to_owned());
    }
    None
}

/// The end of `text`, at most `most` characters, from a line's start.
pub fn tail(text: &str, most: usize) -> String {
    let count = text.chars().count();
    if count <= most {
        return text.trim_end().to_owned();
    }
    let kept: String = text.chars().skip(count - most).collect();
    let from_line = kept.find('\n').map_or(kept.as_str(), |at| &kept[at + 1..]);
    format!("…\n{}", from_line.trim_end())
}

/// Whether two commit ids name the same commit, either one short.
pub fn same_commit(one: &str, other: &str) -> bool {
    let (one, other) = (one.trim(), other.trim());
    !one.is_empty() && !other.is_empty() && (one.starts_with(other) || other.starts_with(one))
}

/// A commit id as people read it.
pub fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// `4m12s`, `38s`, `1h3m`.
pub fn duration(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m{:02}s", secs / 60, secs % 60),
        _ => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// The number at the end of a pull request's address.
fn number_of(url: &str) -> Option<u64> {
    url.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_requests_ci_is_read_from_its_check_runs_and_statuses() {
        let pending = r#"{"state":"OPEN","statusCheckRollup":[
            {"__typename":"CheckRun","name":"check","status":"IN_PROGRESS","conclusion":""},
            {"__typename":"StatusContext","context":"deploy","state":"SUCCESS"}]}"#;
        assert_eq!(read_ci(pending).unwrap().ci, Ci::Pending);
        let failed = r#"{"state":"OPEN","statusCheckRollup":[
            {"__typename":"CheckRun","name":"check","status":"IN_PROGRESS","conclusion":""},
            {"__typename":"CheckRun","name":"clippy","status":"COMPLETED","conclusion":"FAILURE"},
            {"__typename":"StatusContext","context":"deploy","state":"ERROR"}]}"#;
        let seen = read_ci(failed).unwrap();
        assert_eq!(seen.ci, Ci::Failed);
        assert_eq!(seen.failing, ["clippy", "deploy"]);
        let passed = r#"{"state":"MERGED","statusCheckRollup":[
            {"__typename":"CheckRun","name":"check","status":"COMPLETED","conclusion":"SUCCESS"},
            {"__typename":"CheckRun","name":"docs","status":"COMPLETED","conclusion":"SKIPPED"}]}"#;
        let seen = read_ci(passed).unwrap();
        assert_eq!(
            (seen.ci, seen.closed.as_deref()),
            (Ci::Passed, Some("merged"))
        );
        // No checks yet: still to be read; the service gives up later.
        let none = r#"{"state":"OPEN","statusCheckRollup":[]}"#;
        assert_eq!(read_ci(none).unwrap().ci, Ci::Pending);
        assert!(read_ci("no pull requests found").is_err());
    }

    #[test]
    fn a_projects_own_check_is_found_from_its_files() {
        let with = |files: &'static [(&'static str, &'static str)]| {
            detect_check(
                |name| files.iter().any(|(file, _)| *file == name),
                |name| {
                    files
                        .iter()
                        .find(|(file, _)| *file == name)
                        .map(|(_, text)| (*text).to_owned())
                },
            )
        };
        assert_eq!(
            with(&[("scripts/check.sh", ""), ("Cargo.toml", "")]).as_deref(),
            Some("scripts/check.sh")
        );
        assert_eq!(
            with(&[("Makefile", "build:\n\tcc\ncheck: build\n\t./t\n")]).as_deref(),
            Some("make check")
        );
        assert_eq!(with(&[("Cargo.toml", "")]).as_deref(), Some("cargo test"));
        assert_eq!(
            with(&[
                ("package.json", r#"{"scripts":{"test":"vitest"}}"#),
                ("pnpm-lock.yaml", "")
            ])
            .as_deref(),
            Some("pnpm test")
        );
        assert_eq!(
            with(&[("package.json", r#"{"scripts":{"check":"x","test":"y"}}"#)]).as_deref(),
            Some("npm run check")
        );
        assert_eq!(with(&[("package.json", "{}")]), None);
        assert_eq!(with(&[("go.mod", "")]).as_deref(), Some("go test ./..."));
        assert_eq!(with(&[("README.md", "")]), None);
    }

    #[test]
    fn a_check_proves_only_the_clean_commit_it_passed_on() {
        let mut check = Check {
            command: "cargo test".into(),
            commit: "0123456789abcdef".into(),
            dirty: false,
            state: CheckState::Passed,
            started_ms: 0,
            duration_ms: Some(252_000),
            tail: String::new(),
        };
        assert!(check.proves("0123456"));
        assert!(!check.proves("fedcba9"));
        assert_eq!(
            check.evidence(),
            "Verified by Leon: `cargo test` passed on 0123456 in 4m12s."
        );
        check.dirty = true;
        assert!(!check.proves("0123456789abcdef"));
        check.dirty = false;
        check.state = CheckState::Failed;
        assert!(!check.proves("0123456789abcdef"));
        assert!(!same_commit("", "abc"));
    }

    #[test]
    fn the_end_of_a_long_output_is_kept_from_a_lines_start() {
        assert_eq!(tail("ok\n", 10), "ok");
        assert_eq!(tail("first line\nsecond\nthird\n", 12), "…\nthird");
        assert_eq!(duration(38_000), "38s");
        assert_eq!(duration(3_780_000), "1h03m");
        let pr = PullRequest::new("https://github.com/zavudev/leon/pull/62\n", "b", 0);
        assert_eq!((pr.number, pr.name()), (Some(62), "#62".to_owned()));
        assert!(pr.watched());
    }
}

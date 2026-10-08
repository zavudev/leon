//! What the GitHub CLI says about the repository of a project.
//!
//! Only the questions the sidebar asks are here: which branches have a merged
//! pull request, and which have an open one with how its reviews and checks
//! stand. Everything runs through the [`Runner`] on the project's
//! machine, so a repository on an SSH server is asked over SSH and a missing
//! `gh` there is the same quiet "cannot ask" as a missing one here.
//!
//! `gh pr list --state merged --json headRefName` answers in one round trip for
//! the whole repository, which is what keeps a sidebar full of worktrees from
//! asking once per row.

use std::collections::HashMap;

use leon_core::{Checks, Machine, PullRequest, Review};
use serde_json::Value;
use thiserror::Error;

use crate::command::{run_on, CommandSpec, SshOptions};
use crate::runner::{RunError, Runner};

/// What is wrong with a GitHub question.
#[derive(Debug, Error)]
pub enum GithubError {
    /// The command could not be run at all (`gh` is not installed).
    #[error(transparent)]
    Run(#[from] RunError),
    /// The CLI ran and reported a failure (not authenticated, no repository).
    #[error("gh failed (status {status:?}): {stderr}")]
    Failed {
        /// Exit status, absent when the process was ended by a signal.
        status: Option<i32>,
        /// What the CLI printed to standard error, trimmed.
        stderr: String,
    },
}

/// The default number of merged pull requests asked for: enough for a busy
/// project, few enough to stay one short answer.
pub const DEFAULT_LIMIT: usize = 100;

/// GitHub questions bound to one machine.
#[derive(Debug)]
pub struct Github<'a, R: Runner> {
    pub(crate) runner: &'a R,
    pub(crate) machine: &'a Machine,
    pub(crate) ssh: &'a SshOptions,
}

impl<'a, R: Runner> Github<'a, R> {
    /// The GitHub CLI for `machine`.
    pub fn new(runner: &'a R, machine: &'a Machine, ssh: &'a SshOptions) -> Self {
        Self {
            runner,
            machine,
            ssh,
        }
    }

    /// The head branches of the repository at `cwd` that have a merged pull
    /// request, at most `limit` of them. One question for the whole
    /// repository: the CLI reads the repository from `cwd`.
    ///
    /// `Ok(None)` is "cannot ask here": no `gh`, no repository, or a failure
    /// that says nothing about the branches. The caller reads that as "not
    /// known", never as "no pull request was merged".
    pub async fn merged_pull_request_branches(
        &self,
        cwd: &str,
        limit: usize,
    ) -> Result<Option<Vec<String>>, GithubError> {
        let limit = limit.clamp(1, 1000).to_string();
        let asked = self
            .ask(
                cwd,
                [
                    "pr",
                    "list",
                    "--state",
                    "merged",
                    "--json",
                    "headRefName",
                    "--limit",
                    &limit,
                ],
            )
            .await?;
        Ok(asked.map(|stdout| parse_head_branches(&stdout)))
    }

    /// The open pull request of every branch of the repository at `cwd` that
    /// has one, at most `limit` pull requests asked for. One question for the
    /// whole repository, like the merged one; the answer carries the reviews
    /// and the checks, so a worktree row needs no question of its own.
    ///
    /// `Ok(None)` is "cannot ask here", as for the merged question.
    pub async fn open_pull_requests(
        &self,
        cwd: &str,
        limit: usize,
    ) -> Result<Option<HashMap<String, PullRequest>>, GithubError> {
        let limit = limit.clamp(1, 1000).to_string();
        let asked = self
            .ask(
                cwd,
                [
                    "pr",
                    "list",
                    "--state",
                    "open",
                    "--json",
                    OPEN_FIELDS,
                    "--limit",
                    &limit,
                ],
            )
            .await?;
        Ok(asked.map(|stdout| parse_open_pull_requests(&stdout)))
    }

    /// Runs `gh` with `args` in `cwd` and returns what it printed; `None` when
    /// there is no `gh` on the machine.
    async fn ask<const N: usize>(
        &self,
        cwd: &str,
        args: [&str; N],
    ) -> Result<Option<String>, GithubError> {
        let command = CommandSpec::new("gh")
            .args(args)
            .env("GH_PAGER", "cat")
            .env("NO_COLOR", "1")
            .cwd(cwd);
        let placed = run_on(self.machine, &command, self.ssh);
        match self.runner.run(&placed).await {
            Ok(output) if output.success() => Ok(Some(output.stdout)),
            Ok(output) => Err(GithubError::Failed {
                status: output.status,
                stderr: output.stderr.trim().to_owned(),
            }),
            // `gh` is simply not there: nothing to report, and nothing broken.
            Err(RunError::Spawn { .. }) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

/// The fields of an open pull request the worktree views show.
const OPEN_FIELDS: &str = "number,url,headRefName,isDraft,reviewDecision,statusCheckRollup";

/// The `headRefName` of every entry `gh` printed, in its order. Output that is
/// not the expected array yields nothing rather than an error: a new shape
/// from the CLI must not take the sidebar down.
pub fn parse_head_branches(stdout: &str) -> Vec<String> {
    serde_json::from_str::<Vec<serde_json::Value>>(stdout)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            entry
                .get("headRefName")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|branch| !branch.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

/// The open pull requests `gh pr list --json` printed, by head branch. When a
/// branch has several (the newest is first) the first one stands. Output that
/// is not the expected array yields nothing, like [`parse_head_branches`].
pub fn parse_open_pull_requests(stdout: &str) -> HashMap<String, PullRequest> {
    let mut found = HashMap::new();
    let entries = serde_json::from_str::<Vec<Value>>(stdout).unwrap_or_default();
    for entry in entries {
        let branch = entry
            .get("headRefName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|branch| !branch.is_empty());
        let number = entry.get("number").and_then(Value::as_u64);
        let (Some(branch), Some(number)) = (branch, number) else {
            continue;
        };
        let text = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or_default();
        found
            .entry(branch.to_owned())
            .or_insert_with(|| PullRequest {
                number,
                url: text("url").trim().to_owned(),
                draft: entry
                    .get("isDraft")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                review: match text("reviewDecision") {
                    "APPROVED" => Review::Approved,
                    "CHANGES_REQUESTED" => Review::ChangesRequested,
                    "REVIEW_REQUIRED" => Review::Required,
                    _ => Review::None,
                },
                checks: entry
                    .get("statusCheckRollup")
                    .and_then(Value::as_array)
                    .map_or(Checks::None, |rollup| summarise_checks(rollup)),
            });
    }
    found
}

/// One word for a pull request's checks: failing when any failed, otherwise
/// running when any has not finished, otherwise passing; none without checks.
/// Both kinds of entry in the rollup are read: a check run (`status` and
/// `conclusion`) and a commit status (`state`).
fn summarise_checks(rollup: &[Value]) -> Checks {
    let mut summary = Checks::None;
    for entry in rollup {
        let word = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or_default();
        let this = if entry.get("state").is_some() {
            match word("state") {
                "SUCCESS" => Checks::Passing,
                "PENDING" | "EXPECTED" => Checks::Running,
                _ => Checks::Failing,
            }
        } else if word("status") != "COMPLETED" {
            Checks::Running
        } else {
            match word("conclusion") {
                "SUCCESS" | "NEUTRAL" | "SKIPPED" => Checks::Passing,
                "STALE" => Checks::Running,
                _ => Checks::Failing,
            }
        };
        summary = match (summary, this) {
            (Checks::Failing, _) | (_, Checks::Failing) => Checks::Failing,
            (Checks::Running, _) | (_, Checks::Running) => Checks::Running,
            _ => Checks::Passing,
        };
    }
    summary
}

/// Whether `url` is a GitHub remote, so a project that has one is worth asking
/// about pull requests at all.
pub fn is_github_url(url: &str) -> bool {
    let url = url.trim().to_lowercase();
    url.contains("github.com") || url.contains("github.com:")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{Output, ScriptedRunner};
    use leon_core::{Machine, MachineId, MachineKind};

    fn local() -> Machine {
        Machine {
            id: MachineId::local(),
            name: "this computer".into(),
            kind: MachineKind::Local,
        }
    }

    fn ssh() -> SshOptions {
        SshOptions::default()
    }

    #[test]
    fn the_head_branches_of_a_merged_listing_are_read() {
        let branches =
            parse_head_branches(r#"[{"headRefName":"feature/login"},{"headRefName":"main"}]"#);
        assert_eq!(branches, ["feature/login", "main"]);
    }

    #[test]
    fn output_that_is_not_the_expected_array_reads_as_nothing() {
        assert!(parse_head_branches("").is_empty());
        assert!(parse_head_branches("[]").is_empty());
        assert!(parse_head_branches("not json").is_empty());
        assert!(parse_head_branches(r#"[{"other":"x"}]"#).is_empty());
        assert!(parse_head_branches(r#"[{"headRefName":"  "}]"#).is_empty());
    }

    #[tokio::test]
    async fn one_question_asks_for_the_whole_repository() {
        let runner = ScriptedRunner::new().reply(Output::ok(
            r#"[{"headRefName":"feature/login"}]"#.to_owned(),
        ));
        let machine = local();
        let ssh = ssh();
        let github = Github::new(&runner, &machine, &ssh);
        let branches = github
            .merged_pull_request_branches("/srv/api", DEFAULT_LIMIT)
            .await
            .unwrap();
        assert_eq!(branches, Some(vec!["feature/login".to_owned()]));
        let call = &runner.calls()[0];
        assert_eq!(call.program, "gh");
        assert_eq!(
            call.args,
            [
                "pr",
                "list",
                "--state",
                "merged",
                "--json",
                "headRefName",
                "--limit",
                "100"
            ]
        );
        assert_eq!(call.cwd.as_deref(), Some("/srv/api"));
    }

    #[tokio::test]
    async fn a_missing_cli_is_not_known_rather_than_wrong() {
        let runner = ScriptedRunner::new().fail(RunError::Spawn {
            program: "gh".into(),
            source: std::io::Error::other("no gh here"),
        });
        let machine = local();
        let ssh = ssh();
        let github = Github::new(&runner, &machine, &ssh);
        assert!(
            github
                .merged_pull_request_branches("/srv/api", DEFAULT_LIMIT)
                .await
                .unwrap()
                .is_none(),
            "no gh means not known"
        );
    }

    #[tokio::test]
    async fn a_failure_that_is_not_a_missing_cli_is_reported() {
        let runner = ScriptedRunner::new().reply(Output::failed(1, "not authenticated"));
        let machine = local();
        let ssh = ssh();
        let github = Github::new(&runner, &machine, &ssh);
        let error = github
            .merged_pull_request_branches("/srv/api", DEFAULT_LIMIT)
            .await
            .expect_err("a failure the user may want to hear about");
        assert!(matches!(error, GithubError::Failed { .. }), "{error:?}");
    }

    /// What `gh pr list --state open --json ...` printed for a repository with
    /// a draft, an approved pull request with a failing check, one still
    /// running, a second (older) one for the same branch and an entry without
    /// a number.
    const OPEN: &str = r#"[
      {"number":31,"url":"https://github.com/zavudev/leon/pull/31","headRefName":"feature/login",
       "isDraft":true,"reviewDecision":"","statusCheckRollup":[]},
      {"number":30,"url":"https://github.com/zavudev/leon/pull/30","headRefName":"fix/tabs",
       "isDraft":false,"reviewDecision":"APPROVED","statusCheckRollup":[
         {"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"SUCCESS"},
         {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"FAILURE"},
         {"__typename":"CheckRun","name":"lint","status":"IN_PROGRESS","conclusion":""}]},
      {"number":29,"url":"https://github.com/zavudev/leon/pull/29","headRefName":"chore/deps",
       "isDraft":false,"reviewDecision":"REVIEW_REQUIRED","statusCheckRollup":[
         {"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"SUCCESS"},
         {"__typename":"StatusContext","context":"ci/old","state":"PENDING"}]},
      {"number":12,"url":"https://github.com/zavudev/leon/pull/12","headRefName":"fix/tabs",
       "isDraft":false,"reviewDecision":"CHANGES_REQUESTED","statusCheckRollup":[]},
      {"url":"https://github.com/zavudev/leon/pull/x","headRefName":"no-number"}
    ]"#;

    #[test]
    fn open_pull_requests_are_read_by_branch_with_their_reviews_and_checks() {
        let found = parse_open_pull_requests(OPEN);
        assert_eq!(found.len(), 3, "the entry without a number is dropped");
        let draft = &found["feature/login"];
        assert_eq!(draft.number, 31);
        assert!(draft.draft);
        assert_eq!(draft.review, Review::None);
        assert_eq!(draft.checks, Checks::None);
        assert_eq!(draft.url, "https://github.com/zavudev/leon/pull/31");
        let tabs = &found["fix/tabs"];
        assert_eq!(
            tabs.number, 30,
            "the newest pull request of a branch stands"
        );
        assert_eq!(tabs.review, Review::Approved);
        assert_eq!(
            tabs.checks,
            Checks::Failing,
            "one failure outweighs a running check"
        );
        let deps = &found["chore/deps"];
        assert_eq!(deps.review, Review::Required);
        assert_eq!(
            deps.checks,
            Checks::Running,
            "a pending status is not finished"
        );
    }

    #[test]
    fn checks_are_summarised_as_failing_then_running_then_passing() {
        let rollup = |entries: &str| {
            let json =
                format!(r#"[{{"number":1,"headRefName":"b","statusCheckRollup":[{entries}]}}]"#);
            parse_open_pull_requests(&json)["b"].checks
        };
        let run = |status: &str, conclusion: &str| {
            format!(r#"{{"status":"{status}","conclusion":"{conclusion}"}}"#)
        };
        assert_eq!(rollup(""), Checks::None);
        assert_eq!(
            rollup(
                &[
                    run("COMPLETED", "SUCCESS"),
                    run("COMPLETED", "SKIPPED"),
                    run("COMPLETED", "NEUTRAL")
                ]
                .join(",")
            ),
            Checks::Passing
        );
        assert_eq!(rollup(&run("QUEUED", "")), Checks::Running);
        assert_eq!(rollup(&run("COMPLETED", "CANCELLED")), Checks::Failing);
        assert_eq!(rollup(&run("COMPLETED", "TIMED_OUT")), Checks::Failing);
        assert_eq!(rollup(r#"{"state":"SUCCESS"}"#), Checks::Passing);
        assert_eq!(rollup(r#"{"state":"ERROR"}"#), Checks::Failing);
        assert_eq!(rollup(r#"{"state":"FAILURE"}"#), Checks::Failing);
    }

    #[test]
    fn open_pull_request_output_that_is_not_the_expected_array_reads_as_nothing() {
        assert!(parse_open_pull_requests("").is_empty());
        assert!(parse_open_pull_requests("[]").is_empty());
        assert!(parse_open_pull_requests("not json").is_empty());
        assert!(parse_open_pull_requests(r#"{"number":1}"#).is_empty());
        assert!(parse_open_pull_requests(r#"[{"number":1}]"#).is_empty());
    }

    #[tokio::test]
    async fn one_question_asks_for_the_open_pull_requests_of_the_whole_repository() {
        let runner = ScriptedRunner::new().reply(Output::ok(OPEN.to_owned()));
        let machine = local();
        let ssh = ssh();
        let github = Github::new(&runner, &machine, &ssh);
        let found = github
            .open_pull_requests("/srv/api", 50)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found["fix/tabs"].number, 30);
        let call = &runner.calls()[0];
        assert_eq!(call.program, "gh");
        assert_eq!(
            call.args,
            [
                "pr",
                "list",
                "--state",
                "open",
                "--json",
                "number,url,headRefName,isDraft,reviewDecision,statusCheckRollup",
                "--limit",
                "50"
            ]
        );
        assert_eq!(call.cwd.as_deref(), Some("/srv/api"));
    }

    #[tokio::test]
    async fn the_open_question_is_not_known_without_gh_and_a_failure_is_reported() {
        let machine = local();
        let ssh = ssh();
        let missing = ScriptedRunner::new().fail(RunError::Spawn {
            program: "gh".into(),
            source: std::io::Error::other("no gh here"),
        });
        assert!(Github::new(&missing, &machine, &ssh)
            .open_pull_requests("/srv/api", 50)
            .await
            .unwrap()
            .is_none());
        let signed_out = ScriptedRunner::new().reply(Output::failed(4, "run gh auth login"));
        let error = Github::new(&signed_out, &machine, &ssh)
            .open_pull_requests("/srv/api", 50)
            .await
            .expect_err("a signed-out gh is a failure, not an empty list");
        assert!(matches!(error, GithubError::Failed { .. }), "{error:?}");
    }

    #[test]
    fn only_a_github_remote_is_worth_asking_about() {
        assert!(is_github_url("git@github.com:zavudev/leon.git"));
        assert!(is_github_url("https://github.com/zavudev/leon.git"));
        assert!(!is_github_url("git@gitlab.com:zavudev/leon.git"));
        assert!(!is_github_url("/srv/code/leon"));
    }
}

//! What the GitHub CLI says about the repository of a project.
//!
//! Only the questions the sidebar asks are here: the pull request of every
//! branch (open, merged or closed) and, for the open ones, how their reviews
//! and checks stand. Everything runs through the [`Runner`] on the project's
//! machine, so a repository on an SSH server is asked over SSH and a missing
//! `gh` there is the same quiet "cannot ask" as a missing one here.
//!
//! `gh pr list --json ...` answers in one round trip for the whole repository,
//! which is what keeps a sidebar full of worktrees from asking once per row.
//! The answer is made of two such calls that run at the same time (see
//! [`Github::branch_pull_requests`]): a light one for every state, and the
//! heavy one (checks and reviews) for the open pull requests only, so a long
//! history of merged ones never pays for the check rollups.

use std::collections::HashMap;

use leon_core::{Checks, Machine, PullRequest, PullState, Review};
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

/// The default number of pull requests asked for in every state: enough for a
/// busy project, few enough to stay one short answer.
pub const DEFAULT_LIMIT: usize = 200;

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

    /// The pull request of every branch of the repository at `cwd`, open,
    /// merged or closed, as far as the newest `states_limit` pull requests and
    /// the `open_limit` open ones go. Two questions that run at the same time:
    /// the open pull requests with their reviews and checks, and every state
    /// without them. A branch with an open pull request has that one; else
    /// its newest.
    ///
    /// `Ok(None)` is "cannot ask here": no `gh`, no repository, or a failure
    /// that says nothing about the branches. The caller reads that as "not
    /// known", never as "no pull request".
    pub async fn branch_pull_requests(
        &self,
        cwd: &str,
        open_limit: usize,
        states_limit: usize,
    ) -> Result<Option<HashMap<String, PullRequest>>, GithubError> {
        // The open question goes first: it is the one a failure is worth
        // hearing about, and the one whose answer carries the most.
        let (open, states) = tokio::join!(
            self.open_pull_requests(cwd, open_limit),
            self.pull_request_states(cwd, states_limit),
        );
        let (Some(mut found), Some(mut states)) = (open?, states?) else {
            return Ok(None);
        };
        // The open ones are in both: the detailed answer wins.
        for (branch, pull_request) in states.drain() {
            found.entry(branch).or_insert(pull_request);
        }
        Ok(Some(found))
    }

    /// The pull request of every branch of the repository at `cwd` in any
    /// state, without reviews or checks, at most `limit` pull requests asked
    /// for (the newest). `Ok(None)` is "cannot ask here", as for
    /// [`Self::branch_pull_requests`].
    pub async fn pull_request_states(
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
                    "all",
                    "--json",
                    STATE_FIELDS,
                    "--limit",
                    &limit,
                ],
            )
            .await?;
        Ok(asked.map(|stdout| parse_pull_request_states(&stdout)))
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

    /// The repository the folder `cwd` belongs to on GitHub, with its
    /// address: what a person is asked to confirm before issues are synced.
    /// `Ok(None)` when `gh` is not there.
    pub async fn repository(&self, cwd: &str) -> Result<Option<RepoInfo>, GithubError> {
        let asked = self
            .ask(cwd, ["repo", "view", "--json", "nameWithOwner,url"])
            .await?;
        Ok(asked.and_then(|stdout| parse_repository(&stdout)))
    }

    /// The issues of `repo`, open and closed, the most recently updated
    /// first, at most `limit`. `repo` is always named (`-R`): the folder's
    /// remotes are not asked, so a fork never answers for its upstream.
    pub async fn issues(
        &self,
        cwd: &str,
        repo: &str,
        limit: usize,
    ) -> Result<Option<Vec<Issue>>, GithubError> {
        let limit = limit.clamp(1, 1000).to_string();
        let asked = self
            .ask_with(
                cwd,
                vec![
                    "issue".into(),
                    "list".into(),
                    "-R".into(),
                    repo.into(),
                    "--state".into(),
                    "all".into(),
                    "--limit".into(),
                    limit,
                    "--json".into(),
                    ISSUE_FIELDS.into(),
                ],
            )
            .await?;
        Ok(asked.map(|stdout| parse_issues(&stdout)))
    }

    /// Makes the label `name` of `repo` exist (and sets its colour and
    /// description when it did). `false` when `gh` is not there.
    pub async fn ensure_label(
        &self,
        cwd: &str,
        repo: &str,
        name: &str,
        color: &str,
        description: &str,
    ) -> Result<bool, GithubError> {
        let asked = self
            .ask_with(
                cwd,
                vec![
                    "label".into(),
                    "create".into(),
                    name.into(),
                    "-R".into(),
                    repo.into(),
                    "--force".into(),
                    format!("--color={color}"),
                    format!("--description={description}"),
                ],
            )
            .await?;
        Ok(asked.is_some())
    }

    /// Opens an issue in `repo` and answers its address. `Ok(None)` when
    /// `gh` is not there. Labels the repository does not have are the
    /// caller's to make first ([`Github::ensure_label`]).
    pub async fn create_issue(
        &self,
        cwd: &str,
        repo: &str,
        title: &str,
        body: &str,
        labels: &[String],
    ) -> Result<Option<String>, GithubError> {
        let mut args: Vec<String> = vec![
            "issue".into(),
            "create".into(),
            "-R".into(),
            repo.into(),
            format!("--title={title}"),
            format!("--body={body}"),
        ];
        args.extend(
            labels
                .iter()
                .filter(|label| !label.contains(','))
                .map(|label| format!("--label={label}")),
        );
        let asked = self.ask_with(cwd, args).await?;
        Ok(asked.and_then(|stdout| {
            stdout
                .lines()
                .map(str::trim)
                .rev()
                .find(|line| line.starts_with("http"))
                .map(str::to_owned)
        }))
    }

    /// Changes an issue: only what `edit` holds. `false` when `gh` is not
    /// there.
    pub async fn edit_issue(
        &self,
        cwd: &str,
        repo: &str,
        number: i64,
        edit: &IssueEdit,
    ) -> Result<bool, GithubError> {
        let mut args: Vec<String> = vec![
            "issue".into(),
            "edit".into(),
            number.to_string(),
            "-R".into(),
            repo.into(),
        ];
        if let Some(title) = &edit.title {
            args.push(format!("--title={title}"));
        }
        if let Some(body) = &edit.body {
            args.push(format!("--body={body}"));
        }
        let plain = |label: &&String| !label.contains(',');
        args.extend(
            edit.add_labels
                .iter()
                .filter(plain)
                .map(|label| format!("--add-label={label}")),
        );
        args.extend(
            edit.remove_labels
                .iter()
                .filter(plain)
                .map(|label| format!("--remove-label={label}")),
        );
        Ok(self.ask_with(cwd, args).await?.is_some())
    }

    /// Closes an issue (as not planned when `not_planned`), or opens it
    /// again. `false` when `gh` is not there.
    pub async fn set_issue_closed(
        &self,
        cwd: &str,
        repo: &str,
        number: i64,
        closed: bool,
        not_planned: bool,
    ) -> Result<bool, GithubError> {
        let mut args: Vec<String> = vec![
            "issue".into(),
            if closed { "close" } else { "reopen" }.into(),
            number.to_string(),
            "-R".into(),
            repo.into(),
        ];
        if closed && not_planned {
            args.push("--reason=not planned".into());
        }
        Ok(self.ask_with(cwd, args).await?.is_some())
    }

    async fn ask_with(&self, cwd: &str, args: Vec<String>) -> Result<Option<String>, GithubError> {
        let command = CommandSpec::new("gh")
            .args(args)
            .env("GH_PAGER", "cat")
            .env("NO_COLOR", "1")
            .env("GH_PROMPT_DISABLED", "1")
            .cwd(cwd);
        self.run_asked(command).await
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
        self.run_asked(command).await
    }

    async fn run_asked(&self, command: CommandSpec) -> Result<Option<String>, GithubError> {
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

/// The repository a folder belongs to on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// `owner/name`.
    pub name: String,
    /// Its address.
    pub url: String,
}

/// An issue as far as tasks use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Its number in the repository.
    pub number: i64,
    /// Its address.
    pub url: String,
    /// Its title.
    pub title: String,
    /// Its body, as GitHub wrote it (`\r\n` and all); empty when it has none.
    pub body: String,
    /// Whether it is open.
    pub open: bool,
    /// It was closed as not planned.
    pub not_planned: bool,
    /// Its labels, as named.
    pub labels: Vec<String>,
    /// The first person it is assigned to.
    pub assignee: Option<String>,
    /// Its `updatedAt`, as GitHub wrote it.
    pub updated_at: String,
}

/// What to change of an issue; what is `None` or empty stays.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueEdit {
    /// A new title.
    pub title: Option<String>,
    /// A new body.
    pub body: Option<String>,
    /// Labels to add.
    pub add_labels: Vec<String>,
    /// Labels to take away.
    pub remove_labels: Vec<String>,
}

impl IssueEdit {
    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The fields of an issue tasks use.
const ISSUE_FIELDS: &str = "number,url,title,body,state,stateReason,labels,assignees,updatedAt";

/// The issues `gh issue list --json` printed. An entry without a number is
/// dropped, and output that is not the expected array yields nothing, as the
/// other parsers here: a new shape must not take a sync down.
pub fn parse_issues(stdout: &str) -> Vec<Issue> {
    let entries = serde_json::from_str::<Vec<Value>>(stdout).unwrap_or_default();
    entries
        .into_iter()
        .filter_map(|entry| {
            let number = entry.get("number").and_then(Value::as_i64)?;
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let names = |key: &str, field: &str| -> Vec<String> {
                entry
                    .get(key)
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.get(field).and_then(Value::as_str))
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            Some(Issue {
                number,
                url: text("url").trim().to_owned(),
                title: text("title"),
                body: text("body"),
                open: !text("state").eq_ignore_ascii_case("closed"),
                not_planned: text("stateReason").eq_ignore_ascii_case("not_planned"),
                labels: names("labels", "name"),
                assignee: names("assignees", "login").into_iter().next(),
                updated_at: text("updatedAt"),
            })
        })
        .collect()
}

/// The repository `gh repo view --json nameWithOwner,url` printed.
pub fn parse_repository(stdout: &str) -> Option<RepoInfo> {
    let value: Value = serde_json::from_str(stdout).ok()?;
    let name = value.get("nameWithOwner")?.as_str()?.trim();
    // `owner/name`, and nothing a shell or a path could take another way.
    let plain = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let (owner, repo) = name.split_once('/')?;
    if !plain(owner) || !plain(repo) {
        return None;
    }
    Some(RepoInfo {
        name: name.to_owned(),
        url: value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned(),
    })
}

/// The number at the end of an issue's address
/// (`https://github.com/o/r/issues/12`).
pub fn issue_number_of_url(url: &str) -> Option<i64> {
    let (_, number) = url.trim().rsplit_once("/issues/")?;
    number.split(['/', '?', '#']).next()?.parse().ok()
}

/// The fields of an open pull request the worktree views show.
const OPEN_FIELDS: &str = "number,url,headRefName,isDraft,reviewDecision,statusCheckRollup";

/// The fields of a pull request of any state the worktree views show.
const STATE_FIELDS: &str = "number,url,headRefName,state,isDraft";

/// The pull request of each head branch `gh pr list --state all --json` printed
/// (newest first): the open one when the branch has one, else the newest. A
/// merged or closed one carries no reviews or checks, they stopped mattering.
/// Output that is not the expected array yields nothing rather than an error:
/// a new shape from the CLI must not take the sidebar down.
pub fn parse_pull_request_states(stdout: &str) -> HashMap<String, PullRequest> {
    let mut found: HashMap<String, PullRequest> = HashMap::new();
    let entries = serde_json::from_str::<Vec<Value>>(stdout).unwrap_or_default();
    for entry in entries {
        let text = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or_default();
        let branch = text("headRefName").trim();
        let number = entry.get("number").and_then(Value::as_u64);
        let state = match text("state") {
            "OPEN" => PullState::Open,
            "MERGED" => PullState::Merged,
            "CLOSED" => PullState::Closed,
            _ => continue,
        };
        let (false, Some(number)) = (branch.is_empty(), number) else {
            continue;
        };
        let pull_request = PullRequest {
            number,
            state,
            url: text("url").trim().to_owned(),
            draft: state == PullState::Open
                && entry
                    .get("isDraft")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            review: Review::None,
            checks: Checks::None,
        };
        match found.get(branch) {
            // The newest stands, unless an open one comes after it.
            Some(kept) if kept.state == PullState::Open || state != PullState::Open => {}
            _ => {
                found.insert(branch.to_owned(), pull_request);
            }
        }
    }
    found
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
                state: PullState::Open,
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

    /// What `gh pr list --state all --json ...` printed, newest first: a merged
    /// pull request, a closed one, an open draft with an older merged one on
    /// the same branch, a branch reused after its pull request was closed
    /// (the newest, merged, wins), and entries that say nothing usable.
    const STATES: &str = r#"[
        {"number":50,"url":"https://github.com/zavudev/leon/pull/50","headRefName":"feature/login","state":"MERGED","isDraft":false},
        {"number":49,"url":"https://github.com/zavudev/leon/pull/49","headRefName":"fix/typo","state":"CLOSED","isDraft":false},
        {"number":48,"url":"https://github.com/zavudev/leon/pull/48","headRefName":"wip/spike","state":"OPEN","isDraft":true},
        {"number":40,"url":"https://github.com/zavudev/leon/pull/40","headRefName":"wip/spike","state":"MERGED","isDraft":false},
        {"number":45,"url":"https://github.com/zavudev/leon/pull/45","headRefName":"reused","state":"MERGED","isDraft":false},
        {"number":30,"url":"https://github.com/zavudev/leon/pull/30","headRefName":"reused","state":"CLOSED","isDraft":false},
        {"number":20,"url":"https://github.com/zavudev/leon/pull/20","headRefName":"strange","state":"DRAFT","isDraft":false},
        {"number":10,"url":"u","headRefName":"  ","state":"MERGED"},
        {"url":"u","headRefName":"no-number","state":"MERGED"}
    ]"#;

    #[test]
    fn every_state_of_a_pull_request_is_read_by_branch_and_the_open_one_wins() {
        let found = parse_pull_request_states(STATES);
        assert_eq!(found["feature/login"].state, PullState::Merged);
        assert_eq!(found["feature/login"].number, 50);
        assert_eq!(found["fix/typo"].state, PullState::Closed);
        // The open draft is newer, and an open one wins over a merged one
        // wherever it comes in the listing.
        assert_eq!(found["wip/spike"].number, 48);
        assert_eq!(found["wip/spike"].state, PullState::Open);
        assert!(found["wip/spike"].draft);
        // Otherwise the newest.
        assert_eq!(found["reused"].number, 45);
        assert_eq!(found["reused"].state, PullState::Merged);
        assert!(!found["feature/login"].draft);
        assert_eq!(found["feature/login"].checks, Checks::None);
        assert_eq!(found["feature/login"].review, Review::None);
        assert_eq!(
            found["feature/login"].url,
            "https://github.com/zavudev/leon/pull/50"
        );
        assert!(
            !found.contains_key("strange"),
            "an unknown state says nothing"
        );
        assert!(!found.contains_key("no-number"));
        assert!(!found.contains_key(""));
        assert_eq!(found.len(), 4);
    }

    #[test]
    fn an_open_pull_request_listed_after_a_merged_one_of_its_branch_still_wins() {
        let found = parse_pull_request_states(
            r#"[{"number":9,"url":"u","headRefName":"b","state":"MERGED"},
                {"number":3,"url":"v","headRefName":"b","state":"OPEN","isDraft":false}]"#,
        );
        assert_eq!(found["b"].number, 3);
        assert_eq!(found["b"].state, PullState::Open);
    }

    #[test]
    fn output_that_is_not_the_expected_array_reads_as_nothing() {
        assert!(parse_pull_request_states("").is_empty());
        assert!(parse_pull_request_states("[]").is_empty());
        assert!(parse_pull_request_states("not json").is_empty());
        assert!(parse_pull_request_states(r#"[{"other":"x"}]"#).is_empty());
        assert!(parse_pull_request_states(r#"{"number":1}"#).is_empty());
    }

    #[tokio::test]
    async fn the_states_are_asked_for_the_whole_repository_in_one_light_question() {
        let runner = ScriptedRunner::new().reply(Output::ok(STATES.to_owned()));
        let machine = local();
        let ssh = ssh();
        let github = Github::new(&runner, &machine, &ssh);
        let found = github
            .pull_request_states("/srv/api", DEFAULT_LIMIT)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found["feature/login"].state, PullState::Merged);
        let call = &runner.calls()[0];
        assert_eq!(call.program, "gh");
        assert_eq!(
            call.args,
            [
                "pr",
                "list",
                "--state",
                "all",
                "--json",
                "number,url,headRefName,state,isDraft",
                "--limit",
                "200"
            ]
        );
        assert_eq!(call.cwd.as_deref(), Some("/srv/api"));
    }

    #[tokio::test]
    async fn the_open_details_and_every_state_are_asked_together_and_the_open_details_win() {
        let open = r#"[{"number":48,"url":"https://github.com/zavudev/leon/pull/48","headRefName":"wip/spike","isDraft":true,"reviewDecision":"APPROVED","statusCheckRollup":[{"status":"COMPLETED","conclusion":"FAILURE"}]}]"#;
        let runner = ScriptedRunner::new()
            .reply(Output::ok(open.to_owned()))
            .reply(Output::ok(STATES.to_owned()));
        let machine = local();
        let ssh = ssh();
        let github = Github::new(&runner, &machine, &ssh);
        let found = github
            .branch_pull_requests("/srv/api", 100, DEFAULT_LIMIT)
            .await
            .unwrap()
            .unwrap();
        let calls = runner.calls();
        assert_eq!(calls.len(), 2, "two questions, no more");
        assert_eq!(calls[0].args[3], "open");
        assert_eq!(calls[1].args[3], "all");
        assert_eq!(found["wip/spike"].review, Review::Approved);
        assert_eq!(found["wip/spike"].checks, Checks::Failing);
        assert_eq!(found["feature/login"].state, PullState::Merged);
        assert_eq!(found["fix/typo"].state, PullState::Closed);
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
                .branch_pull_requests("/srv/api", 100, DEFAULT_LIMIT)
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
            .branch_pull_requests("/srv/api", 100, DEFAULT_LIMIT)
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

    /// What `gh issue list --state all --json ...` printed: an open issue
    /// with labels and an assignee, a closed one that was not planned, a body
    /// that is `null`, one written on Windows, and an entry without a number.
    const ISSUES: &str = r#"[
      {"number":12,"url":"https://github.com/zavudev/api/issues/12","title":"Fix the login",
       "body":"Users land back.\r\nUse a cleared jar.","state":"OPEN","stateReason":"",
       "labels":[{"name":"Bug"},{"name":"status:doing"}],
       "assignees":[{"login":"ana"},{"login":"luis"}],"updatedAt":"2026-10-10T10:00:00Z"},
      {"number":9,"url":"https://github.com/zavudev/api/issues/9","title":"Old idea",
       "body":null,"state":"CLOSED","stateReason":"NOT_PLANNED","labels":[],
       "assignees":[],"updatedAt":"2026-09-01T08:00:00Z"},
      {"url":"https://github.com/zavudev/api/issues/x","title":"No number"}
    ]"#;

    #[test]
    fn issues_are_read_with_their_labels_state_and_first_assignee() {
        let issues = parse_issues(ISSUES);
        assert_eq!(issues.len(), 2, "the entry without a number is dropped");
        let open = &issues[0];
        assert_eq!(open.number, 12);
        assert!(open.open && !open.not_planned);
        assert_eq!(open.labels, ["Bug", "status:doing"]);
        assert_eq!(open.assignee.as_deref(), Some("ana"));
        assert_eq!(open.body, "Users land back.\r\nUse a cleared jar.");
        assert_eq!(open.updated_at, "2026-10-10T10:00:00Z");
        let closed = &issues[1];
        assert!(!closed.open && closed.not_planned);
        assert_eq!(closed.body, "", "a null body is empty");
        assert_eq!(closed.assignee, None);
        assert!(parse_issues("not json").is_empty());
    }

    #[test]
    fn a_repository_is_read_and_a_name_that_is_not_owner_slash_name_is_refused() {
        let repo = parse_repository(
            r#"{"nameWithOwner":"zavudev/leon","url":"https://github.com/zavudev/leon"}"#,
        )
        .unwrap();
        assert_eq!(repo.name, "zavudev/leon");
        assert_eq!(repo.url, "https://github.com/zavudev/leon");
        for bad in [
            r#"{"nameWithOwner":"zavudev"}"#,
            r#"{"nameWithOwner":"a/b/c"}"#,
            r#"{"nameWithOwner":"a/b c"}"#,
            r#"{"nameWithOwner":"a/;rm"}"#,
            "oops",
        ] {
            assert_eq!(parse_repository(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_number_of_an_issue_is_at_the_end_of_its_address() {
        assert_eq!(
            issue_number_of_url("https://github.com/zavudev/api/issues/42\n"),
            Some(42)
        );
        assert_eq!(
            issue_number_of_url("https://github.com/zavudev/api/issues/42#issuecomment-1"),
            Some(42)
        );
        assert_eq!(
            issue_number_of_url("https://github.com/zavudev/api/pull/42"),
            None
        );
    }

    fn calls_of(runner: &ScriptedRunner) -> Vec<Vec<String>> {
        runner.calls().into_iter().map(|call| call.args).collect()
    }

    #[tokio::test]
    async fn every_call_about_issues_names_the_repository() {
        let runner = ScriptedRunner::new()
            .reply(Output::ok(ISSUES.to_owned()))
            .reply(Output::ok(String::new()))
            .reply(Output::ok(
                "Creating issue in zavudev/api\n\nhttps://github.com/zavudev/api/issues/77\n"
                    .to_owned(),
            ))
            .reply(Output::ok(String::new()))
            .reply(Output::ok(String::new()))
            .reply(Output::ok(String::new()));
        let (machine, ssh) = (local(), ssh());
        let github = Github::new(&runner, &machine, &ssh);
        let repo = "zavudev/api";
        let issues = github.issues("/srv/api", repo, 50).await.unwrap().unwrap();
        assert_eq!(issues.len(), 2);
        assert!(github
            .ensure_label("/srv/api", repo, "status:doing", "fbca04", "Leon: doing")
            .await
            .unwrap());
        let url = github
            .create_issue(
                "/srv/api",
                repo,
                "-Rotate the key",
                "- first\n- second",
                &["status:todo".to_owned(), "a,b".to_owned(), "bug".to_owned()],
            )
            .await
            .unwrap();
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/zavudev/api/issues/77")
        );
        assert!(github
            .edit_issue(
                "/srv/api",
                repo,
                12,
                &IssueEdit {
                    title: Some("New title".into()),
                    body: Some("".into()),
                    add_labels: vec!["status:review".into()],
                    remove_labels: vec!["status:doing".into()],
                },
            )
            .await
            .unwrap());
        assert!(github
            .set_issue_closed("/srv/api", repo, 12, true, true)
            .await
            .unwrap());
        assert!(github
            .set_issue_closed("/srv/api", repo, 12, false, false)
            .await
            .unwrap());

        let calls = calls_of(&runner);
        let args = |call: usize| calls[call].iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(
            args(0),
            [
                "issue",
                "list",
                "-R",
                "zavudev/api",
                "--state",
                "all",
                "--limit",
                "50",
                "--json",
                ISSUE_FIELDS
            ]
        );
        assert_eq!(
            args(1),
            [
                "label",
                "create",
                "status:doing",
                "-R",
                "zavudev/api",
                "--force",
                "--color=fbca04",
                "--description=Leon: doing"
            ]
        );
        // A title or a body that starts with a dash is a value, not a flag,
        // and a label with a comma (which gh would split) is left out.
        assert_eq!(
            args(2),
            [
                "issue",
                "create",
                "-R",
                "zavudev/api",
                "--title=-Rotate the key",
                "--body=- first\n- second",
                "--label=status:todo",
                "--label=bug"
            ]
        );
        assert_eq!(
            args(3),
            [
                "issue",
                "edit",
                "12",
                "-R",
                "zavudev/api",
                "--title=New title",
                "--body=",
                "--add-label=status:review",
                "--remove-label=status:doing"
            ]
        );
        assert_eq!(
            args(4),
            [
                "issue",
                "close",
                "12",
                "-R",
                "zavudev/api",
                "--reason=not planned"
            ]
        );
        assert_eq!(args(5), ["issue", "reopen", "12", "-R", "zavudev/api"]);
        // Every one of them is asked in the project's folder, never prompting.
        assert!(runner
            .calls()
            .iter()
            .all(|call| call.cwd.as_deref() == Some("/srv/api")
                && call
                    .env
                    .contains(&("GH_PROMPT_DISABLED".into(), "1".into()))));
    }

    #[tokio::test]
    async fn the_repository_is_asked_of_the_folder() {
        let runner = ScriptedRunner::new().reply(Output::ok(
            r#"{"nameWithOwner":"zavudev/api","url":"https://github.com/zavudev/api"}"#.to_owned(),
        ));
        let (machine, ssh) = (local(), ssh());
        let github = Github::new(&runner, &machine, &ssh);
        let repo = github.repository("/srv/api").await.unwrap().unwrap();
        assert_eq!(repo.name, "zavudev/api");
        assert_eq!(
            calls_of(&runner)[0],
            ["repo", "view", "--json", "nameWithOwner,url"]
        );
    }
}

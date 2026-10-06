//! What the GitHub CLI says about the repository of a project.
//!
//! Only the questions the sidebar asks are here: which branches have a merged
//! pull request. Everything runs through the [`Runner`] on the project's
//! machine, so a repository on an SSH server is asked over SSH and a missing
//! `gh` there is the same quiet "cannot ask" as a missing one here.
//!
//! `gh pr list --state merged --json headRefName` answers in one round trip for
//! the whole repository, which is what keeps a sidebar full of worktrees from
//! asking once per row.

use leon_core::Machine;
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
    runner: &'a R,
    machine: &'a Machine,
    ssh: &'a SshOptions,
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
        let command = CommandSpec::new("gh")
            .args([
                "pr",
                "list",
                "--state",
                "merged",
                "--json",
                "headRefName",
                "--limit",
                &limit,
            ])
            .env("GH_PAGER", "cat")
            .env("NO_COLOR", "1")
            .cwd(cwd);
        let placed = run_on(self.machine, &command, self.ssh);
        match self.runner.run(&placed).await {
            Ok(output) if output.success() => Ok(Some(parse_head_branches(&output.stdout))),
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

    #[test]
    fn only_a_github_remote_is_worth_asking_about() {
        assert!(is_github_url("git@github.com:zavudev/leon.git"));
        assert!(is_github_url("https://github.com/zavudev/leon.git"));
        assert!(!is_github_url("git@gitlab.com:zavudev/leon.git"));
        assert!(!is_github_url("/srv/code/leon"));
    }
}

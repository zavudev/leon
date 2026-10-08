//! The engine's half of the changes view: reading what changed in a worktree,
//! committing, pushing, opening the pull request and asking an agent to word
//! them.
//!
//! The view asks and awaits; nothing here is kept in the store except what the
//! store already holds for a row, the worktree's status, which is read again
//! when a step ends so the tree shows the new count of changed files, the new
//! distance from the upstream and the new pull request at once.
//!
//! Everything runs through the engine's runner on the machine of the worktree,
//! so it is the same on this computer, over SSH and over the relay. Commit and
//! push may take minutes (hooks run), so they use the slow runner. Nothing
//! here is destructive: see [`leon_remote::ship`].

use std::sync::{Arc, PoisonError};
use std::time::Duration;

use leon_core::{AgentId, MachineId, ProjectId};
use leon_remote::{
    ChangedFile, Changes, FileDiff, Git, Github, PullRequestDraft, PullRequestStart, ShipError,
    Step, Suggest,
};
use tokio::task::JoinHandle;

use super::{Engine, EngineError, SharedRunner, StatusKind};

/// How long an agent is given to word a message.
const AGENT_LIMIT: Duration = Duration::from_secs(120);

/// A commit to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRequest {
    /// The message.
    pub message: String,
    /// The files to commit; every changed file when absent.
    pub files: Option<Vec<ChangedFile>>,
}

/// Steps to take on a worktree, in the order commit, push, pull request. A
/// step that is `None` or `false` is not taken; one command does all three and
/// each is also available alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipRequest {
    /// The machine of the worktree.
    pub machine: MachineId,
    /// The project of the worktree.
    pub project: ProjectId,
    /// The worktree's folder.
    pub path: String,
    /// The commit to make.
    pub commit: Option<CommitRequest>,
    /// Whether to push the branch.
    pub push: bool,
    /// The pull request to open.
    pub pull_request: Option<PullRequestDraft>,
}

/// What a [`ShipRequest`] came to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShipReport {
    /// The steps that were done, with what the command printed.
    pub done: Vec<(Step, String)>,
    /// The address of the pull request, when it was opened.
    pub url: Option<String>,
    /// The step that failed, with the text of the failure, whole. The steps
    /// after it were not taken.
    pub failed: Option<(Step, String)>,
}

impl ShipReport {
    /// The line the status shows for it.
    fn status(&self) -> (StatusKind, String) {
        if let Some((step, text)) = &self.failed {
            let first = text.lines().next().unwrap_or_default();
            return (
                StatusKind::Error,
                format!("{} failed: {first}", step.words()),
            );
        }
        let mut said: Vec<String> = Vec::new();
        for (step, _) in &self.done {
            said.push(
                match step {
                    Step::Commit => "committed",
                    Step::Push => "pushed",
                    Step::PullRequest => "opened a pull request",
                    Step::Suggestion => continue,
                }
                .to_owned(),
            );
        }
        let mut line = match said.split_last() {
            None => "Nothing to do.".to_owned(),
            Some((only, [])) => format!("{}.", capitalised(only)),
            Some((last, rest)) => format!("{} and {last}.", capitalised(&rest.join(", "))),
        };
        if let Some(url) = &self.url {
            line.push(' ');
            line.push_str(url);
        }
        (StatusKind::Info, line)
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

impl Engine {
    /// The runner of commands that may take minutes.
    fn slow(&self) -> SharedRunner {
        let slow = self
            .inner
            .slow_runner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        SharedRunner(slow.unwrap_or_else(|| Arc::clone(&self.inner.runner)))
    }

    /// The changed files of the worktree at `path`, and its branch. A failure
    /// goes to the view that asked, not to the status line.
    pub fn read_changes(
        &self,
        machine: MachineId,
        path: String,
    ) -> JoinHandle<Result<Changes, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let machine = engine.inner.store.machine(&machine)?;
            let runner = SharedRunner(engine.inner.runner.clone());
            let ssh = engine.ssh();
            Ok(Git::new(&runner, &machine, &ssh).changes(&path).await?)
        })
    }

    /// The diff of one changed file of the worktree at `path`.
    pub fn read_diff(
        &self,
        machine: MachineId,
        path: String,
        file: ChangedFile,
    ) -> JoinHandle<Result<FileDiff, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let machine = engine.inner.store.machine(&machine)?;
            let runner = SharedRunner(engine.inner.runner.clone());
            let ssh = engine.ssh();
            Ok(Git::new(&runner, &machine, &ssh)
                .file_diff(&path, &file)
                .await?)
        })
    }

    /// What a pull request of the worktree at `path` starts with: its branch,
    /// the branch it goes into by default, and a title and body from the
    /// commits between them.
    pub fn pull_request_start(
        &self,
        machine: MachineId,
        path: String,
    ) -> JoinHandle<Result<PullRequestStart, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let machine = engine.inner.store.machine(&machine)?;
            let runner = SharedRunner(engine.inner.runner.clone());
            let ssh = engine.ssh();
            Git::new(&runner, &machine, &ssh)
                .pull_request_start(&path)
                .await
                .map_err(ship_error)
        })
    }

    /// Takes the steps of `request`, stops at the first that fails and says
    /// what came of it on the status line. The worktree's status in the store
    /// is read again afterwards, and the open pull request asked for at once
    /// when one was opened, not at the next minute.
    pub fn ship(&self, request: ShipRequest) -> JoinHandle<ShipReport> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let report = engine.ship_now(&request).await;
            let (kind, line) = report.status();
            engine.set_status(kind, line);
            if !report.done.is_empty() {
                engine.refresh_after_ship(&request, &report).await;
            }
            report
        })
    }

    async fn ship_now(&self, request: &ShipRequest) -> ShipReport {
        let mut report = ShipReport::default();
        let machine = match self.inner.store.machine(&request.machine) {
            Ok(machine) => machine,
            Err(error) => {
                report.failed = Some((Step::Commit, error.to_string()));
                return report;
            }
        };
        let runner = self.slow();
        let ssh = self.ssh();
        let git = Git::new(&runner, &machine, &ssh);
        let path = request.path.as_str();
        macro_rules! take {
            ($step:expr, $outcome:expr) => {
                match $outcome {
                    Ok(text) => text,
                    Err(error) => {
                        report.failed = Some(($step, error.to_string()));
                        return report;
                    }
                }
            };
        }
        if let Some(commit) = &request.commit {
            let text = take!(
                Step::Commit,
                git.commit(path, &commit.message, commit.files.as_deref())
                    .await
            );
            report.done.push((Step::Commit, text));
        }
        if request.push {
            let text = take!(Step::Push, git.push(path).await);
            report.done.push((Step::Push, text));
        }
        if let Some(draft) = &request.pull_request {
            let url = take!(
                Step::PullRequest,
                Github::new(&runner, &machine, &ssh)
                    .create_pull_request(path, draft)
                    .await
            );
            report.done.push((Step::PullRequest, url.clone()));
            report.url = Some(url);
        }
        report
    }

    /// Reads the worktree's state again after a step: git for the changed
    /// files and the distance, and, when a pull request was opened, GitHub for
    /// it without waiting for the once-a-minute allowance of the timer.
    async fn refresh_after_ship(&self, request: &ShipRequest, report: &ShipReport) {
        let Ok(project) = self.inner.store.project(&request.project) else {
            return;
        };
        self.refresh_checkouts_of(&project.id).await;
        if report.url.is_some() {
            if let Err(error) = self.probe_open_pull_requests(&project, None).await {
                tracing::debug!(%error, "could not read the new pull request");
            }
        }
    }

    /// Asks `agent`, headless, to word a commit message from the diff of
    /// `files` (every changed file when absent), or a pull request's title and
    /// body from the worktree's commits. The answer is the cleaned text; a
    /// failure is a plain message and never stops anything else.
    pub fn suggest(
        &self,
        machine: MachineId,
        path: String,
        what: Suggest,
        files: Option<Vec<ChangedFile>>,
        agent: AgentId,
    ) -> JoinHandle<Result<String, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let asked = engine.suggest_now(&machine, &path, what, files, agent);
            match tokio::time::timeout(AGENT_LIMIT, asked).await {
                Ok(answer) => answer,
                Err(_) => Err(EngineError::Invalid(format!(
                    "{} did not answer in {} seconds.",
                    agent.name(),
                    AGENT_LIMIT.as_secs()
                ))),
            }
        })
    }

    async fn suggest_now(
        &self,
        machine_id: &MachineId,
        path: &str,
        what: Suggest,
        files: Option<Vec<ChangedFile>>,
        agent: AgentId,
    ) -> Result<String, EngineError> {
        let machine = self.inner.store.machine(machine_id)?;
        let spec = agent
            .spec()
            .filter(|spec| spec.headless.is_some())
            .ok_or_else(|| {
                EngineError::Invalid(format!(
                    "{} cannot be asked to word a message: its non-interactive form is not known.",
                    agent.name()
                ))
            })?;
        let headless = spec.headless.clone().unwrap_or_default();
        // On another machine the program is where the probe found it: a
        // command run over SSH does not read the login shell's PATH.
        let program = match self.machine_state(machine_id) {
            super::MachineState::Online(Some(report)) if !machine_id.is_local() => {
                crate::launch::probed(&report, spec)
                    .unwrap_or(&spec.command)
                    .to_owned()
            }
            _ => spec.command.clone(),
        };
        let runner = self.slow();
        let ssh = self.ssh();
        let git = Git::new(&runner, &machine, &ssh);
        let input = match what {
            Suggest::Commit => git
                .diff_for_message(path, files.as_deref())
                .await
                .map_err(ship_error)?,
            Suggest::PullRequest => {
                let start = git.pull_request_start(path).await.map_err(ship_error)?;
                if start.commits.is_empty() {
                    return Err(EngineError::Invalid(
                        "There are no commits on this branch to word a pull request from."
                            .to_owned(),
                    ));
                }
                start
                    .commits
                    .iter()
                    .map(|commit| format!("{}\n\n{}\n", commit.subject, commit.body))
                    .collect::<Vec<_>>()
                    .join("\n---\n")
            }
        };
        if input.trim().is_empty() {
            return Err(EngineError::Invalid(
                "There is nothing to word yet: no changes were found.".to_owned(),
            ));
        }
        git.suggest(path, &program, &headless, what, &input)
            .await
            .map_err(ship_error)
    }
}

fn ship_error(error: ShipError) -> EngineError {
    EngineError::Invalid(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::Store;
    use leon_history::HistoryRoots;
    use leon_remote::{Output, ScriptedRunner, SshOptions};
    use tokio::runtime::Handle;

    struct Rig {
        engine: Engine,
        runner: Arc<ScriptedRunner>,
        store: Arc<Store>,
        project: ProjectId,
    }

    fn rig(runner: ScriptedRunner) -> Rig {
        let store = Store::open_in_memory().unwrap();
        let runner = Arc::new(runner);
        let engine = Engine::new(
            store.clone(),
            runner.clone(),
            SshOptions::without_multiplexing(),
            HistoryRoots::default(),
            Handle::current(),
        );
        let project = store
            .add_project(&MachineId::local(), "api", "/srv/api")
            .unwrap();
        Rig {
            engine,
            runner,
            store,
            project: project.id,
        }
    }

    fn request(rig: &Rig) -> ShipRequest {
        ShipRequest {
            machine: MachineId::local(),
            project: rig.project.clone(),
            path: "/srv/api".to_owned(),
            commit: Some(CommitRequest {
                message: "Fix the tabs".to_owned(),
                files: None,
            }),
            push: true,
            pull_request: Some(PullRequestDraft {
                title: "Fix the tabs".to_owned(),
                body: "They leaked.".to_owned(),
                base: "main".to_owned(),
                draft: false,
            }),
        }
    }

    fn programs(rig: &Rig) -> Vec<String> {
        rig.runner
            .calls()
            .iter()
            .map(|call| {
                let first = call
                    .args
                    .iter()
                    .find(|arg| !arg.starts_with('-'))
                    .cloned()
                    .unwrap_or_default();
                format!("{} {first}", call.program)
            })
            .collect()
    }

    #[tokio::test]
    async fn one_command_commits_pushes_and_opens_the_pull_request_in_that_order() {
        let rig = rig(ScriptedRunner::new()
            // git add, git commit
            .reply(Output::ok(""))
            .reply(Output::ok("[topic 1a2b3c4] Fix the tabs\n"))
            // the branch, then git push
            .reply(Output::ok("## topic\0"))
            .reply(Output::ok(""))
            // gh pr create
            .reply(Output::ok("https://github.com/zavudev/leon/pull/41\n")));
        let report = rig.engine.ship(request(&rig)).await.unwrap();
        assert_eq!(report.failed, None);
        assert_eq!(
            report.url.as_deref(),
            Some("https://github.com/zavudev/leon/pull/41")
        );
        let steps: Vec<Step> = report.done.iter().map(|(step, _)| *step).collect();
        assert_eq!(steps, [Step::Commit, Step::Push, Step::PullRequest]);
        let ran = programs(&rig);
        assert_eq!(
            &ran[..5],
            ["git add", "git commit", "git status", "git push", "gh pr"]
        );
        let status = rig.engine.status().expect("a status line");
        assert_eq!(status.kind, StatusKind::Info);
        assert_eq!(
            status.text,
            "Committed, pushed and opened a pull request. https://github.com/zavudev/leon/pull/41"
        );
    }

    #[tokio::test]
    async fn the_chain_stops_at_the_first_failure_and_shows_it_whole() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok("")).reply(Output {
            status: Some(1),
            stdout: "running the checks\n".into(),
            stderr: "check failed: 2 errors\n".into(),
        }));
        let report = rig.engine.ship(request(&rig)).await.unwrap();
        assert!(report.done.is_empty());
        assert_eq!(
            report.failed,
            Some((
                Step::Commit,
                "running the checks\ncheck failed: 2 errors".to_owned()
            ))
        );
        assert_eq!(rig.runner.calls().len(), 2, "nothing was pushed or opened");
        let status = rig.engine.status().expect("a status line");
        assert_eq!(status.kind, StatusKind::Error);
        assert_eq!(status.text, "The commit failed: running the checks");
    }

    #[tokio::test]
    async fn a_failed_push_keeps_the_commit_that_was_made_and_opens_nothing() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok("[topic 1a2b3c4] Fix\n"))
            .reply(Output::ok("## topic...origin/topic [behind 1]\0"))
            .reply(Output::failed(
                1,
                "! [rejected] topic -> topic (non-fast-forward)\n",
            )));
        let report = rig.engine.ship(request(&rig)).await.unwrap();
        let steps: Vec<Step> = report.done.iter().map(|(step, _)| *step).collect();
        assert_eq!(steps, [Step::Commit]);
        let (step, text) = report.failed.expect("the push failed");
        assert_eq!(step, Step::Push);
        assert!(text.contains("non-fast-forward"));
        assert!(!programs(&rig).iter().any(|ran| ran == "gh pr"));
    }

    #[tokio::test]
    async fn each_step_is_available_alone() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("## topic...origin/topic [ahead 1]\0"))
            .reply(Output::ok("")));
        let only_push = ShipRequest {
            commit: None,
            pull_request: None,
            ..request(&rig)
        };
        let report = rig.engine.ship(only_push).await.unwrap();
        let steps: Vec<Step> = report.done.iter().map(|(step, _)| *step).collect();
        assert_eq!(steps, [Step::Push]);
        assert_eq!(
            rig.runner.calls()[1].args,
            ["push"],
            "the upstream is set already"
        );
        assert_eq!(rig.engine.status().unwrap().text, "Pushed.");
    }

    #[tokio::test]
    async fn the_pull_request_just_opened_is_stored_at_once_whatever_the_once_a_minute_allowance() {
        use leon_core::{Checks, PullRequest, Review};
        let rig = rig(ScriptedRunner::new());
        // The project is known to have asked GitHub a moment ago.
        rig.engine
            .inner
            .github_asked
            .lock()
            .unwrap()
            .insert(rig.project.clone(), std::time::Instant::now());
        rig.store
            .replace_worktrees(
                &rig.project,
                vec![leon_core::NewWorktree {
                    path: "/srv/api".into(),
                    branch: Some("topic".into()),
                    head: None,
                    is_main: true,
                }],
            )
            .unwrap();
        let open = r#"[{"number":41,"url":"https://github.com/zavudev/leon/pull/41","headRefName":"topic","isDraft":false,"reviewDecision":"","statusCheckRollup":[]}]"#;
        rig.runner
            .queue(Output::ok("https://github.com/zavudev/leon/pull/41\n"));
        // refresh_checkouts: git status of the worktree
        rig.runner.queue(Output::ok("## topic...origin/topic\n"));
        // probe_open_pull_requests: origin, then gh pr list
        rig.runner
            .queue(Output::ok("git@github.com:zavudev/leon.git\n"));
        rig.runner.queue(Output::ok(open));
        let only_pr = ShipRequest {
            commit: None,
            push: false,
            ..request(&rig)
        };
        let report = rig.engine.ship(only_pr).await.unwrap();
        assert_eq!(report.failed, None);
        let worktree = rig.store.worktrees(&rig.project).unwrap().remove(0);
        let status = rig
            .store
            .worktree_statuses()
            .unwrap()
            .remove(&worktree.id)
            .unwrap_or_default();
        assert_eq!(
            status.pull_request,
            Some(PullRequest {
                number: 41,
                url: "https://github.com/zavudev/leon/pull/41".into(),
                draft: false,
                review: Review::None,
                checks: Checks::None,
            })
        );
    }

    #[tokio::test]
    async fn the_changed_files_and_a_diff_are_read_through_the_runner() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("## topic\0 M a.txt\0"))
            .reply(Output::ok(
                "diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n",
            )));
        let changes = rig
            .engine
            .read_changes(MachineId::local(), "/srv/api".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(changes.files.len(), 1);
        let diff = rig
            .engine
            .read_diff(
                MachineId::local(),
                "/srv/api".into(),
                changes.files[0].clone(),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(diff, FileDiff::Lines(lines) if lines.len() == 3));
    }

    #[tokio::test]
    async fn an_agent_without_a_verified_form_is_never_asked() {
        let rig = rig(ScriptedRunner::new());
        let error = rig
            .engine
            .suggest(
                MachineId::local(),
                "/srv/api".into(),
                Suggest::Commit,
                None,
                AgentId::OPENCODE,
            )
            .await
            .unwrap()
            .expect_err("opencode has no verified headless form");
        assert!(error
            .to_string()
            .contains("non-interactive form is not known"));
        assert!(rig.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn an_agent_with_one_is_run_headless_in_the_worktree_with_the_diff_on_stdin() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("diff --git a/a b/a\n+x\n"))
            .reply(Output::ok("Fix the tabs\n")));
        let said = rig
            .engine
            .suggest(
                MachineId::local(),
                "/srv/api".into(),
                Suggest::Commit,
                None,
                AgentId::CLAUDE,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(said, "Fix the tabs");
        let call = &rig.runner.calls()[1];
        assert_eq!(call.program, "claude");
        assert_eq!(
            &call.args[..6],
            [
                "--tools",
                "",
                "-p",
                "--permission-mode",
                "dontAsk",
                "--no-session-persistence"
            ],
            "Claude Code is given no tools to act on the diff with"
        );
        assert_eq!(call.cwd.as_deref(), Some("/srv/api"));
        assert!(call
            .stdin
            .as_deref()
            .is_some_and(|stdin| stdin.starts_with(b"diff --git")));
    }

    #[tokio::test]
    async fn an_agent_that_fails_is_a_plain_message_and_a_clean_tree_has_nothing_to_word() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("diff --git a/a b/a\n+x\n"))
            .reply(Output::failed(1, "Please run codex login")));
        let error = rig
            .engine
            .suggest(
                MachineId::local(),
                "/srv/api".into(),
                Suggest::Commit,
                None,
                AgentId::CODEX,
            )
            .await
            .unwrap()
            .expect_err("not signed in");
        assert_eq!(error.to_string(), "Please run codex login");
        let rig = rig_clean();
        let error = rig
            .engine
            .suggest(
                MachineId::local(),
                "/srv/api".into(),
                Suggest::Commit,
                None,
                AgentId::CLAUDE,
            )
            .await
            .unwrap()
            .expect_err("nothing changed");
        assert!(error.to_string().contains("nothing to word"), "{error}");
    }

    fn rig_clean() -> Rig {
        rig(ScriptedRunner::new().reply(Output::ok("")))
    }

    #[test]
    fn the_status_line_tells_the_steps_in_words() {
        let done = |steps: &[Step]| ShipReport {
            done: steps.iter().map(|step| (*step, String::new())).collect(),
            ..ShipReport::default()
        };
        assert_eq!(done(&[Step::Commit]).status().1, "Committed.");
        assert_eq!(
            done(&[Step::Commit, Step::Push]).status().1,
            "Committed and pushed."
        );
        assert_eq!(done(&[]).status().1, "Nothing to do.");
    }
}

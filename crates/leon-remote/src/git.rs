//! Git worktree operations on any machine.
//!
//! Every operation is a `git` command described once and run through a
//! [`Runner`] on the project's machine, so it behaves the same for a local
//! checkout and for a repository on an SSH server. The output parser is a
//! pure function and carries most of the tests.
//!
//! `git worktree list --porcelain` prints one block per worktree, separated
//! by blank lines:
//!
//! ```text
//! worktree /srv/api
//! HEAD 2f1c...
//! branch refs/heads/main
//!
//! worktree /srv/wt/feature
//! HEAD 9a0b...
//! detached
//! locked being rebased
//! ```
//!
//! The first block is always the main worktree. A block may also carry
//! `bare`, `locked [reason]` and `prunable [reason]` lines.

use leon_core::{Machine, NewWorktree, Project};
use thiserror::Error;

use crate::command::{run_on, CommandSpec, SshOptions};
use crate::runner::{RunError, Runner};

/// One worktree as git reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitWorktree {
    /// Absolute path of the worktree on its machine.
    pub path: String,
    /// Commit the worktree points at; absent for a bare repository.
    pub head: Option<String>,
    /// Checked-out branch without the `refs/heads/` prefix; absent when the
    /// head is detached or the entry is bare.
    pub branch: Option<String>,
    /// Whether this is the repository's main worktree.
    pub is_main: bool,
    /// Whether the entry is a bare repository rather than a checkout.
    pub is_bare: bool,
    /// Whether the head is detached.
    pub is_detached: bool,
    /// Present when the worktree is locked; holds the reason, which may be
    /// empty.
    pub locked: Option<String>,
    /// Present when git considers the worktree prunable; holds the reason,
    /// which may be empty.
    pub prunable: Option<String>,
}

impl GitWorktree {
    /// The worktree in the shape the store accepts.
    pub fn to_new_worktree(&self) -> NewWorktree {
        NewWorktree {
            path: self.path.clone(),
            branch: self.branch.clone(),
            head: self.head.clone(),
            is_main: self.is_main,
        }
    }
}

/// Parses the output of `git worktree list --porcelain`.
///
/// Unknown lines are ignored so that attributes added by future git versions
/// do not break the listing.
pub fn parse_worktree_list(porcelain: &str) -> Vec<GitWorktree> {
    let mut worktrees: Vec<GitWorktree> = Vec::new();
    for line in porcelain.lines() {
        let line = line.trim_end_matches('\r');
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            worktrees.push(GitWorktree {
                path: value.to_owned(),
                is_main: worktrees.is_empty(),
                ..GitWorktree::default()
            });
            continue;
        }
        let Some(current) = worktrees.last_mut() else {
            continue;
        };
        match key {
            "HEAD" => current.head = Some(value.to_owned()),
            "branch" => {
                current.branch = Some(
                    value
                        .strip_prefix("refs/heads/")
                        .unwrap_or(value)
                        .to_owned(),
                );
            }
            "bare" => current.is_bare = true,
            "detached" => current.is_detached = true,
            "locked" => current.locked = Some(value.to_owned()),
            "prunable" => current.prunable = Some(value.to_owned()),
            _ => {}
        }
    }
    worktrees
}

/// Why a git operation failed.
#[derive(Debug, Error)]
pub enum GitError {
    /// The command could not be run at all.
    #[error(transparent)]
    Run(#[from] RunError),
    /// Git ran and reported a failure.
    #[error("git {operation} failed (status {status:?}): {stderr}")]
    Failed {
        /// The git subcommand that failed.
        operation: &'static str,
        /// Exit status, absent when git was ended by a signal.
        status: Option<i32>,
        /// What git printed to standard error, trimmed.
        stderr: String,
    },
    /// An argument was refused before running anything.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

/// Git operations bound to one machine.
#[derive(Debug)]
pub struct Git<'a, R> {
    runner: &'a R,
    machine: &'a Machine,
    ssh: &'a SshOptions,
}

impl<'a, R: Runner> Git<'a, R> {
    /// Git on `machine`, executed through `runner`.
    pub fn new(runner: &'a R, machine: &'a Machine, ssh: &'a SshOptions) -> Self {
        Self {
            runner,
            machine,
            ssh,
        }
    }

    /// Lists the worktrees of a project, main worktree first.
    pub async fn list_worktrees(&self, project: &Project) -> Result<Vec<GitWorktree>, GitError> {
        self.list_worktrees_at(&project.root).await
    }

    /// Lists the worktrees of the repository that contains `cwd`, main
    /// worktree first. Works from any folder inside any worktree, so the first
    /// entry says which repository a folder belongs to.
    pub async fn list_worktrees_at(&self, cwd: &str) -> Result<Vec<GitWorktree>, GitError> {
        let command = git(cwd, ["worktree", "list", "--porcelain"]);
        let stdout = self.run("worktree list", command).await?;
        Ok(parse_worktree_list(&stdout))
    }

    /// Adds a worktree at `path`.
    ///
    /// With a `base`, a new branch named `branch` is created from it. Without
    /// one, the existing branch `branch` is checked out.
    pub async fn add_worktree(
        &self,
        project: &Project,
        branch: &str,
        path: &str,
        base: Option<&str>,
    ) -> Result<(), GitError> {
        reject_option_like("branch", branch)?;
        reject_option_like("path", path)?;
        let command = match base {
            Some(base) => {
                reject_option_like("base", base)?;
                git(&project.root, ["worktree", "add", "-b", branch, path, base])
            }
            None => git(&project.root, ["worktree", "add", path, branch]),
        };
        self.run("worktree add", command).await.map(drop)
    }

    /// Removes the worktree at `path`. With `force`, a worktree with
    /// uncommitted changes is removed too.
    pub async fn remove_worktree(
        &self,
        project: &Project,
        path: &str,
        force: bool,
    ) -> Result<(), GitError> {
        reject_option_like("path", path)?;
        let mut command = git(&project.root, ["worktree", "remove"]);
        if force {
            command = command.arg("--force");
        }
        self.run("worktree remove", command.arg(path))
            .await
            .map(drop)
    }

    /// The branch checked out in `cwd`, or `None` when the head is detached.
    pub async fn current_branch(&self, cwd: &str) -> Result<Option<String>, GitError> {
        let stdout = self
            .run("branch", git(cwd, ["branch", "--show-current"]))
            .await?;
        let branch = stdout.trim();
        Ok((!branch.is_empty()).then(|| branch.to_owned()))
    }

    async fn run(&self, operation: &'static str, command: CommandSpec) -> Result<String, GitError> {
        let placed = run_on(self.machine, &command, self.ssh);
        let output = self.runner.run(&placed).await?;
        if output.success() {
            Ok(output.stdout)
        } else {
            Err(GitError::Failed {
                operation,
                status: output.status,
                stderr: output.stderr.trim().to_owned(),
            })
        }
    }
}

/// A git command run in `cwd`. Terminal prompts are disabled so that a
/// command needing credentials fails instead of waiting for input nobody can
/// give.
fn git<const N: usize>(cwd: &str, args: [&str; N]) -> CommandSpec {
    CommandSpec::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .cwd(cwd)
}

/// Refuses a value git would read as an option. Branch names and paths come
/// from user input and must never be able to change what the command does.
fn reject_option_like(what: &str, value: &str) -> Result<(), GitError> {
    if value.is_empty() || value.starts_with('-') {
        return Err(GitError::InvalidArgument(format!(
            "{what} must not be empty or start with a dash: {value:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{Output, ProcessRunner, ScriptedRunner};
    use leon_core::{MachineId, MachineKind, ProjectId};

    const LISTING: &str = "\
worktree /srv/api
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /srv/wt/feature login
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feature/login

worktree /srv/wt/detached
HEAD 3333333333333333333333333333333333333333
detached

worktree /srv/wt/locked
HEAD 4444444444444444444444444444444444444444
branch refs/heads/hotfix
locked on a removable drive

worktree /srv/wt/gone
HEAD 5555555555555555555555555555555555555555
detached
locked
prunable gitdir file points to non-existent location
";

    fn local() -> Machine {
        Machine {
            id: MachineId::local(),
            name: "This machine".into(),
            kind: MachineKind::Local,
        }
    }

    fn remote() -> Machine {
        Machine {
            id: MachineId::from_string("m1"),
            name: "build box".into(),
            kind: MachineKind::Ssh {
                host: "build.example".into(),
                user: None,
                port: None,
                identity_file: None,
            },
        }
    }

    fn project(machine: &Machine, root: &str) -> Project {
        Project {
            id: ProjectId::from_string("p1"),
            machine_id: machine.id.clone(),
            name: "api".into(),
            root: root.into(),
        }
    }

    #[test]
    fn the_first_listed_worktree_is_the_main_one() {
        let worktrees = parse_worktree_list(LISTING);
        assert_eq!(worktrees.len(), 5);
        assert!(worktrees[0].is_main);
        assert!(worktrees[1..].iter().all(|worktree| !worktree.is_main));
        assert_eq!(worktrees[0].path, "/srv/api");
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(
            worktrees[0].head.as_deref(),
            Some("1111111111111111111111111111111111111111")
        );
    }

    #[test]
    fn paths_with_spaces_and_branches_with_slashes_are_kept_whole() {
        let worktrees = parse_worktree_list(LISTING);
        assert_eq!(worktrees[1].path, "/srv/wt/feature login");
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature/login"));
    }

    #[test]
    fn a_detached_head_has_no_branch() {
        let worktrees = parse_worktree_list(LISTING);
        assert!(worktrees[2].is_detached);
        assert_eq!(worktrees[2].branch, None);
        assert!(worktrees[2].head.is_some());
    }

    #[test]
    fn locked_and_prunable_lines_are_read_with_or_without_a_reason() {
        let worktrees = parse_worktree_list(LISTING);
        assert_eq!(worktrees[3].locked.as_deref(), Some("on a removable drive"));
        assert_eq!(worktrees[3].prunable, None);
        assert_eq!(worktrees[4].locked.as_deref(), Some(""));
        assert_eq!(
            worktrees[4].prunable.as_deref(),
            Some("gitdir file points to non-existent location")
        );
        assert_eq!(worktrees[0].locked, None);
    }

    #[test]
    fn a_bare_repository_is_listed_without_head_or_branch() {
        let worktrees = parse_worktree_list(
            "worktree /srv/repo.git\nbare\n\nworktree /srv/wt/a\nHEAD abc\nbranch refs/heads/a\n",
        );
        assert!(worktrees[0].is_bare);
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[0].head, None);
        assert_eq!(worktrees[0].branch, None);
        assert!(!worktrees[1].is_bare);
    }

    #[test]
    fn windows_line_endings_and_unknown_lines_are_tolerated() {
        let worktrees = parse_worktree_list(
            "stray line before anything\r\nworktree C:/code/api\r\nHEAD abc\r\nbranch refs/heads/main\r\nfuture-attribute yes\r\n\r\n",
        );
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path, "C:/code/api");
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn empty_output_lists_nothing() {
        assert!(parse_worktree_list("").is_empty());
    }

    #[test]
    fn a_listed_worktree_converts_to_the_shape_the_store_accepts() {
        let worktrees = parse_worktree_list(LISTING);
        assert_eq!(
            worktrees[0].to_new_worktree(),
            NewWorktree {
                path: "/srv/api".into(),
                branch: Some("main".into()),
                head: Some("1111111111111111111111111111111111111111".into()),
                is_main: true,
            }
        );
    }

    #[tokio::test]
    async fn listing_runs_git_in_the_project_root() {
        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let runner = ScriptedRunner::new().reply(Output::ok(LISTING));
        let worktrees = Git::new(&runner, &machine, &ssh)
            .list_worktrees(&project(&machine, "/srv/api"))
            .await
            .unwrap();
        assert_eq!(worktrees.len(), 5);

        let calls = runner.calls();
        assert_eq!(calls[0].program, "git");
        assert_eq!(calls[0].args, ["worktree", "list", "--porcelain"]);
        assert_eq!(calls[0].cwd.as_deref(), Some("/srv/api"));
        assert!(calls[0]
            .env
            .contains(&("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned())));
    }

    #[tokio::test]
    async fn listing_from_any_folder_of_a_repository_finds_its_main_worktree_first() {
        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let runner = ScriptedRunner::new().reply(Output::ok(LISTING));
        let worktrees = Git::new(&runner, &machine, &ssh)
            .list_worktrees_at("/srv/wt/feature login/src")
            .await
            .unwrap();
        assert_eq!(worktrees[0].path, "/srv/api");
        assert!(worktrees[0].is_main);
        let calls = runner.calls();
        assert_eq!(calls[0].args, ["worktree", "list", "--porcelain"]);
        assert_eq!(calls[0].cwd.as_deref(), Some("/srv/wt/feature login/src"));
    }

    #[tokio::test]
    async fn listing_from_a_folder_that_is_not_a_repository_is_a_git_failure() {
        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let runner =
            ScriptedRunner::new().reply(Output::failed(128, "fatal: not a git repository"));
        let outcome = Git::new(&runner, &machine, &ssh)
            .list_worktrees_at("/tmp")
            .await;
        assert!(matches!(outcome, Err(GitError::Failed { .. })));
    }

    #[tokio::test]
    async fn the_same_operation_on_a_remote_machine_goes_through_ssh() {
        let machine = remote();
        let ssh = SshOptions::without_multiplexing();
        let runner = ScriptedRunner::new().reply(Output::ok(LISTING));
        Git::new(&runner, &machine, &ssh)
            .list_worktrees(&project(&machine, "/srv/my api"))
            .await
            .unwrap();

        let calls = runner.calls();
        assert_eq!(calls[0].program, "ssh");
        assert_eq!(
            calls[0].args.last().map(String::as_str),
            Some("cd '/srv/my api' && exec env 'GIT_TERMINAL_PROMPT=0' git worktree list --porcelain")
        );
    }

    #[tokio::test]
    async fn adding_a_worktree_with_a_base_creates_a_new_branch() {
        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let runner = ScriptedRunner::new().reply(Output::ok(""));
        Git::new(&runner, &machine, &ssh)
            .add_worktree(
                &project(&machine, "/srv/api"),
                "feature/x",
                "/srv/wt/x",
                Some("origin/main"),
            )
            .await
            .unwrap();
        assert_eq!(
            runner.calls()[0].args,
            [
                "worktree",
                "add",
                "-b",
                "feature/x",
                "/srv/wt/x",
                "origin/main"
            ]
        );
    }

    #[tokio::test]
    async fn adding_a_worktree_without_a_base_checks_out_the_existing_branch() {
        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let runner = ScriptedRunner::new().reply(Output::ok(""));
        Git::new(&runner, &machine, &ssh)
            .add_worktree(
                &project(&machine, "/srv/api"),
                "hotfix",
                "/srv/wt/hotfix",
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            runner.calls()[0].args,
            ["worktree", "add", "/srv/wt/hotfix", "hotfix"]
        );
    }

    #[tokio::test]
    async fn removing_a_worktree_passes_force_only_when_asked() {
        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""));
        let git = Git::new(&runner, &machine, &ssh);
        let project = project(&machine, "/srv/api");
        git.remove_worktree(&project, "/srv/wt/x", false)
            .await
            .unwrap();
        git.remove_worktree(&project, "/srv/wt/x", true)
            .await
            .unwrap();

        let calls = runner.calls();
        assert_eq!(calls[0].args, ["worktree", "remove", "/srv/wt/x"]);
        assert_eq!(
            calls[1].args,
            ["worktree", "remove", "--force", "/srv/wt/x"]
        );
    }

    #[tokio::test]
    async fn arguments_that_look_like_options_are_refused_before_running_git() {
        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let runner = ScriptedRunner::new();
        let git = Git::new(&runner, &machine, &ssh);
        let project = project(&machine, "/srv/api");

        for (branch, path, base) in [
            ("--force", "/srv/wt/x", None),
            ("ok", "-rf", None),
            ("ok", "/srv/wt/x", Some("--orphan")),
            ("", "/srv/wt/x", None),
        ] {
            assert!(matches!(
                git.add_worktree(&project, branch, path, base).await,
                Err(GitError::InvalidArgument(_))
            ));
        }
        assert!(matches!(
            git.remove_worktree(&project, "--force", false).await,
            Err(GitError::InvalidArgument(_))
        ));
        assert!(runner.calls().is_empty());
    }

    #[tokio::test]
    async fn a_failing_git_command_reports_status_and_message() {
        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let runner =
            ScriptedRunner::new().reply(Output::failed(128, "fatal: not a git repository\n"));
        let outcome = Git::new(&runner, &machine, &ssh)
            .list_worktrees(&project(&machine, "/srv/api"))
            .await;
        match outcome {
            Err(GitError::Failed {
                operation,
                status,
                stderr,
            }) => {
                assert_eq!(operation, "worktree list");
                assert_eq!(status, Some(128));
                assert_eq!(stderr, "fatal: not a git repository");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_command_that_cannot_run_is_reported_as_a_run_error() {
        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let runner = ScriptedRunner::new();
        let outcome = Git::new(&runner, &machine, &ssh)
            .current_branch("/srv/api")
            .await;
        assert!(matches!(outcome, Err(GitError::Run(_))));
    }

    #[tokio::test]
    async fn the_current_branch_is_trimmed_and_absent_when_detached() {
        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let runner = ScriptedRunner::new()
            .reply(Output::ok("feature/login\n"))
            .reply(Output::ok("\n"));
        let git = Git::new(&runner, &machine, &ssh);
        assert_eq!(
            git.current_branch("/srv/api").await.unwrap().as_deref(),
            Some("feature/login")
        );
        assert_eq!(git.current_branch("/srv/api").await.unwrap(), None);
        assert_eq!(runner.calls()[0].args, ["branch", "--show-current"]);
    }

    /// Runs a setup command in a scratch repository, isolated from the
    /// user's own git configuration.
    async fn setup(runner: &ProcessRunner, cwd: &str, args: &[&str]) {
        let spec = CommandSpec::new("git")
            .args([
                "-c",
                "user.name=Leon Test",
                "-c",
                "user.email=leon@example.invalid",
            ])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args.iter().copied())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .cwd(cwd);
        let output = runner.run(&spec).await.unwrap();
        assert!(output.success(), "git {args:?} failed: {}", output.stderr);
    }

    #[tokio::test]
    async fn worktrees_are_added_listed_and_removed_in_a_real_repository() {
        let runner = ProcessRunner::new();
        if runner
            .run(&CommandSpec::new("git").arg("--version"))
            .await
            .map_or(true, |output| !output.success())
        {
            eprintln!("skipped: git is not available");
            return;
        }

        let scratch = tempfile::tempdir().unwrap();
        // Resolve symlinks so the paths match what git prints.
        let base = scratch.path().canonicalize().unwrap();
        let root = base.join("repo");
        std::fs::create_dir(&root).unwrap();
        let root = root.to_string_lossy().into_owned();
        setup(&runner, &root, &["init", "--quiet"]).await;
        setup(&runner, &root, &["checkout", "--quiet", "-B", "main"]).await;
        setup(
            &runner,
            &root,
            &["commit", "--quiet", "--allow-empty", "-m", "initial"],
        )
        .await;

        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let git = Git::new(&runner, &machine, &ssh);
        let project = project(&machine, &root);
        let feature = base.join("wt feature").to_string_lossy().into_owned();

        assert_eq!(
            git.current_branch(&root).await.unwrap().as_deref(),
            Some("main")
        );
        git.add_worktree(&project, "feature/one", &feature, Some("main"))
            .await
            .unwrap();

        let worktrees = git.list_worktrees(&project).await.unwrap();
        assert_eq!(worktrees.len(), 2);
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature/one"));
        assert!(!worktrees[1].is_main);
        assert_eq!(worktrees[0].head, worktrees[1].head);
        assert!(std::path::Path::new(&worktrees[1].path).ends_with("wt feature"));
        assert_eq!(
            git.current_branch(&feature).await.unwrap().as_deref(),
            Some("feature/one")
        );

        git.remove_worktree(&project, &feature, false)
            .await
            .unwrap();
        assert_eq!(git.list_worktrees(&project).await.unwrap().len(), 1);

        let missing = git.remove_worktree(&project, &feature, false).await;
        assert!(matches!(missing, Err(GitError::Failed { .. })));
    }
}

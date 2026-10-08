//! Running things on machines: command construction and execution.
//!
//! Leon treats the local machine and SSH servers uniformly. Code that wants
//! something done describes it once as a [`CommandSpec`] in terms of the
//! target machine (program, arguments, working directory there), and this
//! crate turns that description into a process that can be started here:
//! unchanged for the local machine, wrapped in an `ssh` invocation for a
//! remote one.
//!
//! The crate is split so that almost everything is pure and unit-tested:
//!
//! * [`quote`]: POSIX shell quoting.
//! * [`command`]: [`CommandSpec`] and the placement of a command onto a
//!   machine.
//! * [`spawn`]: the command a local child is started from, so Windows does not
//!   open a console window for it.
//! * [`runner`]: the [`Runner`] trait, the real process-based implementation
//!   and a scripted fake for tests.
//! * [`git`]: git worktree operations expressed through a runner, so they
//!   behave identically on local and remote machines.
//! * [`files`]: reading, writing and listing files, and git marks, as short
//!   POSIX scripts and the parsers of their answers.
//! * [`changes`]: the changed files of a checkout and the diff of one.
//! * [`ship`]: commit, push and pull request, and an agent's help to word them.
//! * [`search`]: text search in the files of a project, as a script that uses
//!   ripgrep, `git grep` or `grep`, and the parsers of their answers.
//! * [`agent`]: how each coding agent is started and resumed.
//! * [`relay`]: machines reached through a relay: a hub of durable
//!   connections and a runner that sends commands to them.
//! * [`mod@probe`]: a one-round-trip inventory of a machine.
//!
//! Remote machines are assumed to provide a POSIX shell. Remote Windows hosts
//! are not supported yet.

#![warn(missing_docs)]

pub mod agent;
pub mod changes;
pub mod command;
pub mod connect;
pub mod diagnosis;
pub mod files;
pub mod git;
pub mod github;
pub mod icon;
pub mod probe;
pub mod processes;
pub mod quote;
pub mod relay;
pub mod runner;
pub mod search;
pub mod ship;
pub mod spawn;

pub use agent::{agent_launch, session_command};
pub use changes::{
    parse_changes, parse_diff, ChangedFile, Changes, DiffLine, FileDiff, FileStatus, LineKind,
};
pub use command::{interactive_on, remote_shell_command, run_on, CommandSpec, SshOptions};
pub use diagnosis::{classify, Diagnosis, DiagnosisKind, Stage};
pub use git::{parse_status, parse_worktree_list, CheckoutState, Git, GitError, GitWorktree};
pub use github::{is_github_url, parse_open_pull_requests, Github, GithubError};
pub use probe::{
    catalogue_tools, parse_probe, probe, probe_command, probe_command_for, probe_script,
    ProbeError, ProbeReport, RemoteOs, MAX_TOOLS,
};
pub use quote::{sh_join, sh_quote, sh_quote_typed};
pub use relay::{RelayHub, RoutingRunner};
pub use runner::{Output, ProcessRunner, RunError, Runner, ScriptedRunner};
pub use ship::{
    clean_suggestion, create_pull_request_args, parse_pull_request_url, prefill_pull_request,
    push_args, split_title_and_body, Commit, PullRequestDraft, PullRequestStart, ShipError, Step,
    Suggest,
};
pub use spawn::{child, std_child};

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
//! * [`runner`]: the [`Runner`] trait, the real process-based implementation
//!   and a scripted fake for tests.
//! * [`files`]: reading, writing and listing text files, one command each.
//! * [`git`]: git worktree operations expressed through a runner, so they
//!   behave identically on local and remote machines.
//! * [`agent`]: how each coding agent is started and resumed.
//! * [`relay`]: machines reached through a relay: a hub of durable
//!   connections and a runner that sends commands to them.
//! * [`mod@probe`]: a one-round-trip inventory of a machine.
//!
//! Remote machines are assumed to provide a POSIX shell. Remote Windows hosts
//! are not supported yet.

#![warn(missing_docs)]

pub mod agent;
pub mod command;
pub mod connect;
pub mod diagnosis;
pub mod files;
pub mod git;
pub mod icon;
pub mod probe;
pub mod processes;
pub mod quote;
pub mod relay;
pub mod runner;

pub use agent::{agent_launch, session_command};
pub use command::{interactive_on, remote_shell_command, run_on, CommandSpec, SshOptions};
pub use diagnosis::{classify, Diagnosis, DiagnosisKind, Stage};
pub use git::{parse_worktree_list, Git, GitError, GitWorktree};
pub use probe::{parse_probe, probe, probe_command, ProbeError, ProbeReport, RemoteOs};
pub use quote::{sh_join, sh_quote};
pub use relay::{RelayHub, RoutingRunner};
pub use runner::{Output, ProcessRunner, RunError, Runner, ScriptedRunner};

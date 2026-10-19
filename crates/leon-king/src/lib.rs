//! King Leon: the coordinator, without a window and without a process.
//!
//! King is one conversation with a coding agent the person chooses (Claude
//! Code, Codex, opencode…), kept by a service that runs with the window
//! closed. The person talks to King about every project Leon knows; King
//! answers, and gives work to other agents, its *workers*, each in a
//! worktree of its own and on the agent and model that cost least for the
//! job. The workers' turns, their questions and their ends come back to
//! King on their own: it never has to ask whether they are done.
//!
//! This crate holds everything that can be decided about it without
//! touching a disk or starting a process:
//!
//! * [`config`]: `king.toml`, the agent King runs on, the tiers of workers
//!   and the limits it works within;
//! * [`coordinator`]: the state of King and its workers, changed by what the
//!   person says and what the agents print, and answering with the
//!   [`coordinator::Effect`]s the service carries out;
//! * [`transcript`]: what a conversation said, kept short enough to send;
//! * [`history`]: King's past conversations, to read and to resume;
//! * [`policy`]: the questions of the workers Leon answers itself;
//! * [`route`]: which agent and model take a job, from the tier asked for,
//!   what each agent has left of its limits and what was spent today;
//! * [`protocol`]: the messages of the service's control socket, which the
//!   window, the phone's host, the command line and King's own tools speak;
//! * [`models`]: what King knows of each model's talents and benchmarks;
//! * [`report`]: the line under every answer of King's: its tokens, how
//!   fast they came, and what is left of the budget and the usage windows;
//! * [`run`]: a task of the board carried out as its steps, in the order
//!   their needs say;
//! * [`verify`]: the evidence a worker's work is right: its project's check,
//!   and the CI of its pull request;
//! * [`prompt`]: the instructions King is given, and the message that wakes
//!   it with news of its workers.
//!
//! The service (`leon king serve`), the socket and King's tools
//! (`leon king mcp`) are the application's (`crates/app/src/king`). See
//! `docs/KING.md`.

#![warn(missing_docs)]

pub mod config;
pub mod coordinator;
pub mod history;
pub mod models;
pub mod policy;
pub mod prompt;
pub mod protocol;
pub mod report;
pub mod route;
pub mod run;
pub mod transcript;
pub mod verify;

/// The environment variable that holds the path of the service's control
/// socket, given to King's tools.
pub const ENV_SOCKET: &str = "LEON_KING";

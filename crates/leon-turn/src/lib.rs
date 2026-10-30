//! Leon's unified agent protocol: one conversation with a coding agent, the
//! same whatever the agent is.
//!
//! Coding agents each have a way of being driven by a program instead of a
//! person at a terminal: Claude Code's control protocol, Codex's application
//! server, and the Agent Client Protocol, which opencode, Grok and Cursor's
//! agent serve. They differ in everything (framing, handshake, how a
//! permission is asked, what a tool's use looks like). This crate is the one
//! place that knows them, and what comes out of it is the same for all:
//!
//! * a **turn** is one prompt sent to an agent and everything it does about
//!   it ([`TurnSpec`]);
//! * what happens is a short list of [`TurnEvent`]s: the conversation
//!   started, words, thoughts, a tool's use and its result, a question, the
//!   end;
//! * what the agent asks is a [`QuestionKind`] (a permission, a choice, a
//!   plan) and is answered with an [`Answer`];
//! * what a person can do meanwhile is an [`Action`]: say more, answer,
//!   interrupt, change the mode or the model;
//! * what an agent offers (modes, models, efforts, whether it asks) is its
//!   [`AgentOptions`].
//!
//! A client shows these and nothing else, so every agent looks and behaves
//! alike in it. Adding an agent is one [`Adapter`] in [`agents`].
//!
//! The crate is pure: lines in, events and lines out. It starts no process
//! and knows no transport. The desktop application runs the agent on a pipe,
//! here or through `ssh` (`crates/app/src/chat`); a paired computer offers
//! terminals and no pipes, so there the phone's library (`leon-mobile`) and
//! the desktop both run it in a terminal behind the [`bridge`].
//!
//! * [`agents`]: the catalogue, and the [`AgentSession`] an adapter makes;
//! * [`change`]: what a tool changed in a file, as lines;
//! * [`claude`], [`codex`]: each agent's own protocol;
//! * [`acp`]: the Agent Client Protocol, one session for every agent that
//!   serves it, and [`opencode`], [`grok`], [`cursor`]: what each of them
//!   does its own way;
//! * [`bridge`]: an agent behind a terminal instead of a pipe: the script
//!   between them, the frames written to it and the marks it answers with;
//! * [`rewind`]: taking a conversation back to before a turn, as each agent
//!   allows;
//! * [`events`]: the events, the questions and the answers, and [`Lines`],
//!   which cuts a stream of bytes into clean lines;
//! * [`shell`]: the login shell an agent's command is started through;
//! * [`tag`]: the names messages are acknowledged by.

pub mod acp;
pub mod agents;
pub mod bridge;
pub mod change;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod error;
pub mod events;
pub mod grok;
pub mod opencode;
pub mod rewind;
pub mod shell;
pub mod tag;

pub use agents::{
    adapter, agents, Action, Adapter, AgentInfo, AgentSession, McpServer, Outgoing, Probe, Step,
    TurnSpec, ADAPTERS,
};
pub use change::{ChangeKind, DiffLine, FileChange, Hunk, LineKind};
pub use claude::{AgentCommand, AgentMode, AgentModel, AgentOptions};
pub use error::TurnError;
pub use events::{
    object_on, Answer, ChoiceAnswer, ChoiceOption, ChoiceQuestion, Lines, PermissionChoice,
    PermissionEffect, QuestionKind, Todo, TodoStatus, Tokens, TurnEvent,
};
pub use rewind::{fork_transcript, CodexRevert, RevertStep, Rewind};

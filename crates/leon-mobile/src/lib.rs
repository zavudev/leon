//! Leon for phones: everything a mobile app needs from a paired computer,
//! behind an API small enough to cross a language boundary.
//!
//! The phone is a client like the desktop one. It pairs with a short code,
//! holds a durable encrypted session through the relay, and reads what the
//! other Leon shares. All of that is `leon-link` and `leon-wire`, used as
//! they are: nothing of the protocol or its cryptography is written again
//! here (see `docs/REMOTE.md`).
//!
//! What this crate adds is the **turn**: one prompt sent to an agent, its
//! answer streamed back, and whatever the agent asks on the way answered
//! from the phone. A host offers commands and terminals and nothing else,
//! and only a terminal streams and outlives a dropped connection, so a turn
//! is the agent run inside a host terminal:
//!
//! * `leon-turn`, Leon's unified agent protocol, is the catalogue of agents
//!   and each one's own way of being spoken to (Claude Code's control
//!   protocol, Codex's application server, opencode over the Agent Client
//!   Protocol), reduced to the few events a chat shows. The desktop
//!   application drives agents with the same crate;
//! * [`agents`] starts an agent's command through the owner's login shell;
//! * `bridge` makes a terminal a two-way line to an agent's standard
//!   streams, with acknowledgements that survive in the output the host
//!   keeps;
//! * `turn` is the engine of one turn: what to tell the chat and what to
//!   write, with nothing written early, lost while offline or sent twice
//!   after a replay;
//! * [`events`] and [`options`] are that protocol's types as they cross to
//!   the application;
//! * [`session`] is the connection: shared state, transcripts, turns that
//!   can be found again and re-attached after the app was closed;
//! * [`king`] speaks to King Leon, the other computer's coordinator,
//!   through its host;
//! * [`logo`] finds a project's logo on the other computer with the
//!   desktop's own detection;
//! * [`mobile`] is the entry point: identity, pairing, connecting.
//!
//! The types are exported with UniFFI. Callbacks arrive on threads of this
//! crate's own runtime; the generated bindings take them to the caller's.

#![warn(missing_docs)]

pub mod agents;
mod bridge;
pub mod error;
pub mod events;
pub mod king;
pub mod logo;
pub mod mobile;
pub mod options;
mod runtime;
pub mod session;
mod turn;

pub use agents::{agents, AgentInfo};
pub use error::MobileError;
pub use events::{
    Answer, ChoiceAnswer, ChoiceOption, ChoiceQuestion, PermissionChoice, PermissionEffect,
    QuestionKind, TurnEvent,
};
pub use king::{KingListener, KingWatch};
pub use logo::{LogoFormat, LogoImage, ProjectLogo};
pub use mobile::{check_code, CodeCheck, LeonMobile, PairedHost};
pub use options::{AgentMode, AgentModel, AgentOptions};
pub use session::{
    ConnectionListener, ConnectionState, HistorySession, HostSession, OfflineReason, Project,
    RunningTurn, SharedState, SpawnOverrides, TranscriptEntry, Turn, TurnListener, TurnRequest,
    Worktree,
};

uniffi::setup_scaffolding!();

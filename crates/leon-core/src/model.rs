//! The domain model: machines, projects, worktrees, sessions and messages.
//!
//! These are plain data types. They carry no behaviour beyond conversion to
//! and from the short text tags used in the database, so they can be cloned
//! freely into UI state and serialised for diagnostics.
//!
//! Paths that live on a machine (`Project::root`, `Worktree::path`,
//! `Session::cwd`) are stored as strings rather than `PathBuf`, because the
//! machine may run a different operating system than the one Leon runs on and
//! its paths must be passed through untouched.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use crate::agent::AgentId;

use crate::ids::{MachineId, ProjectId, SessionId, WorktreeId};

/// A computer that agents can run on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    /// Identifier of the machine.
    pub id: MachineId,
    /// Name shown to the user.
    pub name: String,
    /// How Leon reaches the machine.
    pub kind: MachineKind,
}

/// How a [`Machine`] is reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MachineKind {
    /// The machine Leon itself runs on.
    Local,
    /// A machine reached over SSH.
    Ssh {
        /// Host name, address or an alias from the user's SSH configuration.
        host: String,
        /// Login name; the SSH configuration decides when absent.
        user: Option<String>,
        /// TCP port; the SSH configuration decides when absent.
        port: Option<u16>,
        /// Private key file on the machine Leon runs on.
        identity_file: Option<String>,
    },
    /// A machine reached through a relay: both computers dial out to it, so
    /// nothing needs to be opened or set up on the network. Everything between
    /// the two Leon installations is end-to-end encrypted.
    Relay {
        /// The host's id (base32), which the relay routes on.
        host_id: String,
        /// The host's pinned static public key, hex. Connections are accepted
        /// only from the host that holds the matching private key.
        host_key: String,
        /// The relay used when the machine was paired (`wss://…`).
        relay_url: String,
        /// The name the host gave itself.
        name: String,
    },
}

/// A code base on a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Identifier of the project.
    pub id: ProjectId,
    /// The machine the project lives on.
    pub machine_id: MachineId,
    /// Name shown to the user.
    pub name: String,
    /// Absolute path of the project root on that machine.
    pub root: String,
}

/// A git worktree belonging to a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    /// Identifier of the worktree. It is preserved across
    /// [`Store::replace_worktrees`](crate::Store::replace_worktrees) calls for
    /// as long as the path stays the same.
    pub id: WorktreeId,
    /// The project the worktree belongs to.
    pub project_id: ProjectId,
    /// Absolute path of the worktree on the project's machine.
    pub path: String,
    /// Checked-out branch, absent when the head is detached.
    pub branch: Option<String>,
    /// Commit the worktree points at.
    pub head: Option<String>,
    /// Whether this is the repository's main worktree.
    pub is_main: bool,
}

/// A worktree as reported by git, before the store has assigned it an id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewWorktree {
    /// Absolute path of the worktree on the project's machine.
    pub path: String,
    /// Checked-out branch, absent when the head is detached.
    pub branch: Option<String>,
    /// Commit the worktree points at.
    pub head: Option<String>,
    /// Whether this is the repository's main worktree.
    pub is_main: bool,
}

/// Who produced a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The person driving the agent.
    User,
    /// The agent's own reply.
    Assistant,
    /// A tool invocation, reduced to one readable line.
    Tool,
    /// Instructions or notices injected by the agent harness.
    System,
}

impl Role {
    /// The short tag used in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::System => "system",
        }
    }

    /// Parses the tag produced by [`Role::as_str`].
    pub fn parse(tag: &str) -> Option<Self> {
        match tag {
            "user" => Some(Role::User),
            "assistant" => Some(Role::Assistant),
            "tool" => Some(Role::Tool),
            "system" => Some(Role::System),
            _ => None,
        }
    }
}

/// One agent conversation, as recorded in the unified history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Leon's identifier for the session.
    pub id: SessionId,
    /// The agent that ran the session.
    pub agent: AgentId,
    /// The agent's own identifier, the value its `resume` option accepts.
    pub external_id: String,
    /// The machine the session ran on.
    pub machine_id: MachineId,
    /// Working directory of the session on that machine.
    pub cwd: String,
    /// The project whose root or worktree contains `cwd`, when one is known.
    pub project_id: Option<ProjectId>,
    /// Short human-readable title.
    pub title: String,
    /// Model name reported by the agent, when known.
    pub model: Option<String>,
    /// Time of the first message.
    pub started_at: DateTime<Utc>,
    /// Time of the latest message.
    pub updated_at: DateTime<Utc>,
    /// Number of messages stored for the session.
    pub message_count: u32,
}

/// One entry of a session transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// The session the message belongs to.
    pub session_id: SessionId,
    /// Zero-based position within the session.
    pub seq: u32,
    /// Who produced the message.
    pub role: Role,
    /// Plain text of the message.
    pub text: String,
    /// When the message was produced.
    pub at: DateTime<Utc>,
}

/// A session as produced by an importer, before the store has assigned an id.
///
/// The triple `(agent, machine_id, external_id)` identifies the session across
/// imports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSession {
    /// The agent that ran the session.
    pub agent: AgentId,
    /// The agent's own identifier for the session.
    pub external_id: String,
    /// The machine the session ran on.
    pub machine_id: MachineId,
    /// Working directory of the session on that machine.
    pub cwd: String,
    /// Short human-readable title.
    pub title: String,
    /// Model name reported by the agent, when known.
    pub model: Option<String>,
    /// Time of the first message.
    pub started_at: DateTime<Utc>,
    /// Time of the latest message.
    pub updated_at: DateTime<Utc>,
}

/// A transcript entry as produced by an importer. Its position in the slice
/// handed to the store becomes its `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewMessage {
    /// Who produced the message.
    pub role: Role,
    /// Plain text of the message.
    pub text: String,
    /// When the message was produced.
    pub at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_tags_round_trip() {
        for spec in crate::agent::builtin() {
            assert_eq!(AgentId::parse(spec.id.as_str()), Some(spec.id));
        }
        assert_eq!(AgentId::parse("Not Valid"), None);
    }

    #[test]
    fn role_tags_round_trip() {
        for role in [Role::User, Role::Assistant, Role::Tool, Role::System] {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        assert_eq!(Role::parse(""), None);
    }

    #[test]
    fn a_machine_survives_a_json_round_trip() {
        let machine = Machine {
            id: MachineId::from_string("m1"),
            name: "build box".into(),
            kind: MachineKind::Ssh {
                host: "build.example".into(),
                user: Some("dev".into()),
                port: Some(2222),
                identity_file: None,
            },
        };
        let json = serde_json::to_string(&machine).unwrap();
        assert_eq!(serde_json::from_str::<Machine>(&json).unwrap(), machine);
    }
}

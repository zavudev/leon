//! The agents that can be driven, and how each one is.
//!
//! An agent is a row of [`ADAPTERS`]. Each is a process spoken to both ways
//! for as long as the turn lasts. Its [`AgentSession`] writes what the agent
//! must read (its handshake, the prompt, the answers to what it asks) and
//! reads what it prints. This is how a person gets the agent's own questions
//! and modes whatever the agent is.
//!
//! Nothing here starts a process: an adapter says the argument vector of a
//! turn and a session says the lines. Who runs it (a pipe on this computer,
//! a terminal of another one behind the phone's bridge) is the client's own
//! business. The prompt never becomes shell text: it is bytes written to the
//! agent's input.

use serde_json::Value;

use crate::claude::{AgentCommand, AgentOptions};
use crate::error::TurnError;
use crate::events::{Answer, TurnEvent};

/// An agent this library can drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    /// The catalogue tag, the same the desktop application uses (`claude`).
    pub id: String,
    /// The name to show.
    pub name: String,
    /// The program that must be installed on the other computer.
    pub program: String,
}

/// One turn, as an adapter needs it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnSpec<'a> {
    /// The program to run (the adapter's, unless a test replaced it).
    pub program: &'a str,
    /// What the person wrote.
    pub prompt: &'a str,
    /// The conversation to continue, by the id the agent gave it.
    pub resume: Option<&'a str>,
    /// The agent's permission mode, by the id [`AgentOptions`] lists.
    pub mode: Option<&'a str>,
    /// The model, by the id [`AgentOptions`] lists.
    pub model: Option<&'a str>,
    /// How hard it should think, by the id [`AgentOptions`] lists.
    pub effort: Option<&'a str>,
    /// The folder it works in, as a whole path, when it is known.
    pub cwd: Option<&'a str>,
    /// More folders it may work in.
    pub add_dirs: &'a [String],
    /// Servers of tools the agent is given for this turn, beside its own.
    pub mcp_servers: &'a [McpServer],
    /// Instructions added to the agent's own, for this turn. An agent of the
    /// Agent Client Protocol has no way to be told them: it reads the
    /// instruction files of its folder (`AGENTS.md`) instead.
    pub instructions: Option<&'a str>,
}

/// A server of tools (the Model Context Protocol) an agent starts on its own
/// computer and speaks to on its standard streams.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpServer {
    /// The name the agent knows its tools by.
    pub name: String,
    /// The program.
    pub command: String,
    /// What follows the program.
    pub args: Vec<String>,
    /// Variables added to its environment.
    pub env: Vec<(String, String)>,
}

impl McpServer {
    /// The server as an agent's configuration names one (`.mcp.json`,
    /// Codex's `mcp_servers`): its command, arguments and environment.
    pub fn config(&self) -> Value {
        let env: serde_json::Map<String, Value> = self
            .env
            .iter()
            .map(|(name, value)| (name.clone(), Value::from(value.as_str())))
            .collect();
        serde_json::json!({ "command": self.command, "args": self.args, "env": env })
    }

    /// The servers by name, each as [`Self::config`] says it.
    pub fn configs(servers: &[Self]) -> Value {
        Value::Object(
            servers
                .iter()
                .map(|server| (server.name.clone(), server.config()))
                .collect(),
        )
    }

    /// The server as the Agent Client Protocol names one in `session/new`.
    pub fn acp(&self) -> Value {
        let env: Vec<Value> = self
            .env
            .iter()
            .map(|(name, value)| serde_json::json!({ "name": name, "value": value }))
            .collect();
        serde_json::json!({
            "name": self.name,
            "command": self.command,
            "args": self.args,
            "env": env,
        })
    }
}

/// One line for the agent's input, and the tag its delivery is known by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// Names this message when its delivery is acknowledged. Deterministic
    /// for anything sent in reaction to what the agent printed, so that a
    /// replayed turn recognises what it already sent.
    pub tag: String,
    /// One JSON value, without a line end.
    pub line: String,
}

/// What a person can do to a turn that is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Say more.
    Prompt(String),
    /// Answer the question with this id.
    Answer(String, Answer),
    /// Stop working, keep the conversation.
    Interrupt,
    /// Work in another permission mode from now on.
    SetMode(String),
    /// Answer with another model from now on.
    SetModel(String),
}

/// What reading one line led to.
#[derive(Debug, Default)]
pub struct Step {
    /// What to tell the chat.
    pub events: Vec<TurnEvent>,
    /// What to write to the agent without anybody asking: the next message
    /// of a handshake, the refusal of a request this client does not serve.
    pub send: Vec<Outgoing>,
    /// What a prompt can ask the agent for by name, when it just listed
    /// them: an agent that only says so inside a conversation.
    pub commands: Option<Vec<AgentCommand>>,
}

/// One agent process spoken to both ways, for one turn.
///
/// Whoever runs the process owns its streams; a session only knows the
/// agent's protocol. It must be able to start in the middle:
/// a turn found again after the application was closed is read from the
/// output the host kept, by a session made with [`Adapter::session`] that
/// was never told the prompt and sees only what the agent printed and which
/// of the client's messages were acknowledged ([`Self::delivered`]).
///
/// For a protocol with numbered requests (JSON-RPC, as Codex's app server
/// and opencode's ACP speak): [`Self::first`] returns the opening request;
/// the next steps of the handshake are returned from [`Self::line`] as
/// [`Step::send`] when the answer they wait for is read, each with a tag
/// derived from its request id (`h.2`), never from a counter of this
/// process; a request *from* the agent that needs an answer becomes a
/// [`TurnEvent::Question`] whose id is the request's, answered through
/// [`Self::encode`] with the tag `a.<id>` ([`crate::tag::id_tag`]); a
/// request this client cannot serve is refused at once through
/// [`Step::send`], so the agent never waits for nobody.
pub trait AgentSession: Send {
    /// What the agent must read first, in order: its handshake, then the
    /// prompt if the protocol lets it follow at once. Delivered by the
    /// client before anything else.
    fn first(&mut self, turn: &TurnSpec<'_>) -> Vec<Outgoing>;

    /// One JSON value the agent printed.
    fn line(&mut self, value: &Value, step: &mut Step);

    /// A line that was not JSON: kept to explain a failure.
    fn noise(&mut self, line: &str);

    /// The message with this tag was delivered: the agent has it.
    fn delivered(&mut self, tag: &str, step: &mut Step);

    /// What to write so that `action` happens.
    fn encode(&mut self, action: Action) -> Result<Vec<Outgoing>, TurnError>;

    /// The agent has nothing more to do and waits for input that will not
    /// come: its input is to be closed, which ends it.
    fn idle(&self) -> bool;

    /// The process ended with `code`.
    fn exited(&mut self, code: u32) -> Vec<TurnEvent>;
}

/// How an agent is asked what it offers, in one command on the other
/// computer: `sh -c <script> leon <program> <arguments…>`.
pub struct Probe {
    /// The script; `$1` is the agent's program.
    pub script: &'static str,
    /// What follows the program.
    pub arguments: fn() -> Vec<String>,
    /// Reads what the script printed.
    pub parse: fn(&str) -> AgentOptions,
}

/// How one agent is driven.
pub struct Adapter {
    /// The catalogue tag.
    pub id: &'static str,
    /// The name to show.
    pub name: &'static str,
    /// The program.
    pub program: &'static str,
    /// The argument vector of the agent for a turn, program first.
    pub argv: fn(&TurnSpec<'_>) -> Vec<String>,
    /// A session for one turn; `unique` seeds the ids it makes up, so that
    /// two processes following the same turn never reuse one.
    pub session: fn(unique: u64) -> Box<dyn AgentSession>,
    /// How the agent on the other computer is asked what it offers.
    pub options: Probe,
    /// Its protocol must be told the folder to work in as a whole path:
    /// [`TurnSpec::cwd`] is found out when the turn names none.
    pub needs_folder: bool,
}

/// Every agent that can be driven. Nothing else is offered: a session of
/// another agent in the history is shown, and cannot be continued.
pub const ADAPTERS: &[Adapter] = &[
    Adapter {
        id: "claude",
        name: "Claude Code",
        program: "claude",
        argv: crate::claude::argv,
        session: crate::claude::session,
        options: Probe {
            script: crate::claude::OPTIONS_SCRIPT,
            arguments: crate::claude::options_arguments,
            parse: crate::claude::parse_options,
        },
        needs_folder: false,
    },
    Adapter {
        id: "codex",
        name: "Codex",
        program: "codex",
        argv: crate::codex::argv,
        session: crate::codex::session,
        options: Probe {
            script: crate::codex::OPTIONS_SCRIPT,
            arguments: crate::codex::options_arguments,
            parse: crate::codex::parse_options,
        },
        needs_folder: false,
    },
    Adapter {
        id: "opencode",
        name: "opencode",
        program: "opencode",
        argv: crate::opencode::argv,
        session: crate::opencode::session,
        options: Probe {
            script: crate::opencode::OPTIONS_SCRIPT,
            arguments: crate::opencode::options_arguments,
            parse: crate::opencode::parse_options,
        },
        needs_folder: true,
    },
    Adapter {
        id: "grok",
        name: "Grok",
        program: "grok",
        argv: crate::grok::argv,
        session: crate::grok::session,
        options: Probe {
            script: crate::grok::OPTIONS_SCRIPT,
            arguments: crate::grok::options_arguments,
            parse: crate::grok::parse_options,
        },
        needs_folder: true,
    },
    Adapter {
        id: "cursor",
        name: "Cursor",
        program: "cursor-agent",
        argv: crate::cursor::argv,
        session: crate::cursor::session,
        options: Probe {
            script: crate::cursor::OPTIONS_SCRIPT,
            arguments: crate::cursor::options_arguments,
            parse: crate::cursor::parse_options,
        },
        needs_folder: true,
    },
];

/// The adapter of the agent with this tag.
pub fn adapter(id: &str) -> Option<&'static Adapter> {
    ADAPTERS.iter().find(|adapter| adapter.id == id)
}

/// The agents this library can drive.
pub fn agents() -> Vec<AgentInfo> {
    ADAPTERS
        .iter()
        .map(|adapter| AgentInfo {
            id: adapter.id.to_owned(),
            name: adapter.name.to_owned(),
            program: adapter.program.to_owned(),
        })
        .collect()
}

/// The items as owned strings.
pub fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_names_its_agents_once_each() {
        let listed: Vec<String> = agents().into_iter().map(|agent| agent.id).collect();
        assert_eq!(listed, ["claude", "codex", "opencode", "grok", "cursor"]);
        assert!(adapter("claude").is_some());
        assert!(adapter("vim").is_none());
        // Every one of them starts its server and is told the rest there.
        let turn = TurnSpec {
            program: "agent",
            prompt: "--help; rm -rf ~",
            ..TurnSpec::default()
        };
        for adapter in ADAPTERS {
            assert!(!(adapter.argv)(&turn).contains(&turn.prompt.to_owned()));
        }
    }
}

//! opencode, spoken to over the Agent Client Protocol.
//!
//! opencode serves the protocol on its standard streams (checked against
//! opencode 2.0.24):
//!
//! ```text
//! opencode acp
//! ```
//!
//! The session is [`crate::acp`]'s; here is what is opencode's own. It
//! continues a conversation with `session/resume`, which does not print it
//! again. Its modes are its agents, an option of the session, and the
//! models of every provider are only known a moment after the session
//! answered. A refusal ends the turn: that is opencode's way.
//!
//! opencode has no mode in which it asks nothing. [`MODE_AUTO`] works as its
//! default agent with every permission allowed at once, as `opencode --auto`
//! does.

use serde_json::Value;

use crate::acp::{self, Dialect};
use crate::agents::{strings, AgentSession, TurnSpec};
use crate::claude::{AgentCommand, AgentMode, AgentModel, AgentOptions};
use crate::events::blocks;

pub use crate::acp::MODE_AUTO;

/// The agent [`MODE_AUTO`] works as.
const AUTO_AGENT: &str = "build";

const DIALECT: Dialect = Dialect {
    name: "opencode",
    resume: "session/resume",
    login: None,
    effort: "effort",
    auto_agent: Some(AUTO_AGENT),
    models_come_late: true,
    refusal_ends_turn: true,
};

/// The command line of a turn. What the turn is about travels in requests.
pub fn argv(turn: &TurnSpec<'_>) -> Vec<String> {
    strings(&[turn.program, "acp"])
}

/// A session for one turn.
pub fn session(unique: u64) -> Box<dyn AgentSession> {
    acp::session(unique, &DIALECT)
}

/// The command that asks opencode what it offers, without starting a
/// conversation: its version, the models it lists and its agents.
pub const OPTIONS_SCRIPT: &str = r#"program=$1
printf 'leon-version:%s\n' "$("$program" --version 2>/dev/null | tail -n 1)"
models=$("$program" models 2>/dev/null)
if [ -z "$models" ]; then
    # Asked in a folder it has not worked in yet, it may answer before it
    # knows its providers.
    sleep 1
    models=$("$program" models 2>/dev/null)
fi
printf 'leon-models:\n%s\n\nleon-agents:\n' "$models"
"$program" debug agents 2>/dev/null
exit 0"#;

/// The commands every opencode has. Its own list, with the ones a person
/// wrote, is only said inside a conversation ([`Step::commands`]).
fn own_commands() -> Vec<AgentCommand> {
    vec![
        AgentCommand::slash("init", Some("Guided AGENTS.md setup."), None),
        AgentCommand::slash(
            "review",
            Some("Review changes; what is not committed unless you say."),
            Some("[commit|branch|pr]"),
        ),
        AgentCommand::slash("compact", Some("Compact the session."), None),
    ]
}

/// What [`OPTIONS_SCRIPT`] is given after the program: nothing.
pub fn options_arguments() -> Vec<String> {
    Vec::new()
}

/// The efforts offered for every model. Which of them a model has is only
/// known inside a conversation; one it lacks is left as that computer has it.
const EFFORTS: &[&str] = &["low", "medium", "high", "max"];

/// Reads the output of [`OPTIONS_SCRIPT`].
pub fn parse_options(text: &str) -> AgentOptions {
    let mut options = AgentOptions {
        questions: true,
        follow_ups: false,
        live_changes: true,
        // Until it listed its agents and its models, nothing is known.
        assumed: true,
        ..AgentOptions::default()
    };
    let (mut part, mut agents) = ("", String::new());
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(version) = line.strip_prefix("leon-version:") {
            // `opencode v2.0.24`: the number alone.
            options.version = version
                .split_whitespace()
                .last()
                .map(|word| word.trim_start_matches('v').to_owned())
                .filter(|text| !text.is_empty());
            continue;
        }
        match line {
            "leon-models:" => part = "models",
            "leon-agents:" => part = "agents",
            _ if part == "agents" => {
                agents.push_str(line);
                agents.push('\n');
            }
            _ if part == "models" => {
                // `provider/model`, nothing else on the line.
                let id = line.trim();
                let Some((provider, name)) = id.split_once('/') else {
                    continue;
                };
                if provider.is_empty() || name.is_empty() || id.contains(char::is_whitespace) {
                    continue;
                }
                options.models.push(AgentModel {
                    id: id.to_owned(),
                    label: format!("{name} · {provider}"),
                    description: None,
                    efforts: EFFORTS.iter().map(|effort| (*effort).to_owned()).collect(),
                    resolves_to: None,
                });
            }
            _ => {}
        }
    }
    let listed = agents
        .find('[')
        .and_then(|at| serde_json::from_str::<Value>(agents[at..].trim()).ok());
    for agent in listed.as_ref().map(blocks).unwrap_or_default() {
        // The agents a person works with; the others are their helpers.
        if agent["mode"] != "primary" || agent["hidden"].as_bool() == Some(true) {
            continue;
        }
        let Some(id) = agent["id"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        options.modes.push(AgentMode {
            id: id.to_owned(),
            label: agent["name"].as_str().unwrap_or(id).to_owned(),
            description: agent["description"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            dangerous: false,
        });
    }
    if options.modes.iter().any(|mode| mode.id == AUTO_AGENT) {
        options.default_mode = Some(AUTO_AGENT.to_owned());
        options.modes.push(AgentMode {
            id: MODE_AUTO.to_owned(),
            label: "Build, never asking".to_owned(),
            description: Some(
                "Every permission it asks for is allowed at once, as `opencode --auto` does."
                    .to_owned(),
            ),
            dangerous: true,
        });
    }
    options.assumed = options.modes.is_empty() && options.models.is_empty();
    options.commands = own_commands();
    options
}

#[cfg(test)]
mod tests;

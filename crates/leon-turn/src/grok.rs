//! Grok, spoken to over the Agent Client Protocol.
//!
//! Grok Build serves the protocol on its standard streams (checked against
//! grok 1.0.52 in real runs):
//!
//! ```text
//! grok agent stdio
//! ```
//!
//! The session is [`crate::acp`]'s; here is what is Grok's own. It uses the
//! sign-in its terminal program keeps and asks for none. It continues a
//! conversation with `session/resume`. The model and the effort are options
//! of the session (`model`, `reasoning_effort`); it has no modes there, so
//! the two offered are this library's: asking, and [`acp::MODE_AUTO`]. A
//! refusal ends the turn. Beside the protocol's notifications it prints many
//! of its own (`_x.ai/…`), which are passed over.

use crate::acp::{self, Dialect};
use crate::agents::{strings, AgentSession, TurnSpec};
use crate::claude::{AgentCommand, AgentMode, AgentModel, AgentOptions};

/// The mode in which Grok asks before it changes anything.
pub const MODE_ASK: &str = "default";

const DIALECT: Dialect = Dialect {
    name: "Grok",
    resume: "session/resume",
    login: None,
    effort: "reasoning_effort",
    auto_agent: None,
    models_come_late: false,
    refusal_ends_turn: true,
};

/// The command line of a turn. What the turn is about travels in requests.
pub fn argv(turn: &TurnSpec<'_>) -> Vec<String> {
    strings(&[turn.program, "agent", "stdio"])
}

/// A session for one turn.
pub fn session(unique: u64) -> Box<dyn AgentSession> {
    acp::session(unique, &DIALECT)
}

/// The command that asks Grok what it offers, without starting a
/// conversation: its version and the models it lists.
pub const OPTIONS_SCRIPT: &str = r#"program=$1
printf 'leon-version:%s\n' "$("$program" --version 2>/dev/null | head -n 1)"
printf 'leon-models:\n'
"$program" models 2>/dev/null
exit 0"#;

/// What [`OPTIONS_SCRIPT`] is given after the program: nothing.
pub fn options_arguments() -> Vec<String> {
    Vec::new()
}

/// The efforts offered for every model. Which of them a model has is only
/// known inside a conversation; one it lacks is left as that computer has it.
const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh"];

/// The two modes of an agent that has none to set: this library's.
pub(crate) fn two_modes(name: &str) -> Vec<AgentMode> {
    vec![
        AgentMode {
            id: MODE_ASK.to_owned(),
            label: "Ask before changes".to_owned(),
            description: Some(format!(
                "{name} asks before it writes a file or runs a command."
            )),
            dangerous: false,
        },
        AgentMode {
            id: acp::MODE_AUTO.to_owned(),
            label: "Never asking".to_owned(),
            description: Some("Every permission it asks for is allowed at once.".to_owned()),
            dangerous: true,
        },
    ]
}

/// Reads the output of [`OPTIONS_SCRIPT`].
pub fn parse_options(text: &str) -> AgentOptions {
    let mut options = AgentOptions {
        questions: true,
        follow_ups: false,
        live_changes: true,
        modes: two_modes("Grok"),
        default_mode: Some(MODE_ASK.to_owned()),
        ..AgentOptions::default()
    };
    let mut listing = false;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(version) = line.strip_prefix("leon-version:") {
            // `grok 1.0.52 (0920c8169e27)`: the number alone.
            options.version = version
                .split_whitespace()
                .find(|word| word.starts_with(|first: char| first.is_ascii_digit()))
                .map(str::to_owned);
            continue;
        }
        if line == "leon-models:" {
            listing = true;
            continue;
        }
        if !listing {
            continue;
        }
        // `  * grok-4.7 (default)` and `  - grok-4.6`, among other lines.
        let Some(rest) = line
            .trim_start()
            .strip_prefix("* ")
            .or_else(|| line.trim_start().strip_prefix("- "))
        else {
            continue;
        };
        let Some(id) = rest.split_whitespace().next() else {
            continue;
        };
        // `(default)` marks the one it answers with when none is asked for.
        if rest.contains("(default)") {
            options.default_model = Some(id.to_owned());
        }
        options.models.push(AgentModel {
            id: id.to_owned(),
            label: id.to_owned(),
            description: None,
            efforts: EFFORTS.iter().map(|effort| (*effort).to_owned()).collect(),
            resolves_to: None,
        });
    }
    // Without its models nothing was heard from it: what is offered is a guess.
    options.assumed = options.models.is_empty();
    options.commands = vec![AgentCommand::slash(
        "compact",
        Some("Compress the conversation to save room."),
        Some("[what to keep]"),
    )];
    options
}

#[cfg(test)]
mod tests;

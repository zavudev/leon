//! Conversations with agents in Leon's own interface, instead of in the
//! agent's terminal program.
//!
//! Every agent is driven through Leon's unified agent protocol (`leon-turn`),
//! so a conversation is the same data whatever the agent is, and the window
//! draws one interface for all of them. This module is the part without a
//! window:
//!
//! * [`model`]: the conversation as a list of things to draw, built from the
//!   protocol's events;
//! * [`changed`]: what a turn changed, as the card at its end says it;
//! * [`checkpoint`]: the worktree's files as they were when a turn began, kept
//!   in git without touching the person's index, and the way back to them;
//! * [`rewind`]: taking a conversation back to before one of its turns, in the
//!   way each agent allows;
//! * [`stamp`]: when a message was said, as a person reads it;
//! * [`phrase`]: what a stretch of tools says of itself, in words;
//! * [`paths`]: the files a message names, to open them from it;
//! * [`find`]: finding words in what was said and answered;
//! * [`complete`]: what the field offers while a command, a file, a mode or
//!   a model is being named;
//! * [`handoff`]: what a new agent is told of the conversation it takes over
//!   when the person switches agents mid-thread;
//! * [`turn`]: one turn on a pipe, the agent's process and its protocol;
//! * [`reach`]: that process on the machine it belongs to, this computer or
//!   another one.
//!
//! The window's side is `ui/chat.rs` (what the shell does) and
//! `ui/chat_view.rs` (what it draws). The setting `agent_interface` chooses
//! between this and the terminal; it is experimental (its own section of the settings) and off by default.

pub mod changed;
pub mod checkpoint;
pub mod complete;
pub mod find;
pub mod handoff;
pub mod model;
pub mod paths;
pub mod phrase;
pub mod reach;
pub mod rewind;
pub mod stamp;
pub mod turn;

use leon_core::AgentId;
use leon_turn::Adapter;

/// The way `agent` is driven in a conversation, when the unified protocol
/// knows one. A custom agent has none and always runs in a terminal.
pub fn adapter_of(agent: AgentId) -> Option<&'static Adapter> {
    leon_turn::adapter(agent.as_str())
}

/// The value of the setting `agent_interface` that asks for conversations.
pub const UNIFIED: &str = "chat";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agents_of_the_protocol_can_be_driven_and_the_others_cannot() {
        for agent in [
            AgentId::CLAUDE,
            AgentId::CODEX,
            AgentId::OPENCODE,
            AgentId::GROK,
            AgentId::CURSOR,
        ] {
            assert!(adapter_of(agent).is_some(), "{agent:?}");
        }
        let custom = AgentId::parse("my-agent").expect("a valid id");
        assert!(adapter_of(custom).is_none());
        // In the catalogue of agents, and not in the protocol's.
        assert!(adapter_of(AgentId::KIMI).is_none());
    }
}

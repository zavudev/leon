//! The tag that wraps what a new agent is told of a conversation it takes
//! over, and how it is read back.
//!
//! The text of the block is written by the application (`handoff::text`
//! there); the agent records it, with the rest of the message, in its own
//! history, and a conversation read back from there shows the switch instead
//! of a page of someone else's words ([`crate::model::Conversation::resumed`]).
//!
//! Pure: text in, agents out.

use leon_core::AgentId;

/// The tag that wraps the block, as the harness's list knows it.
pub const TAG: &str = "leon-handoff";

/// The opening tag of the block of a switch from `from` to `to`.
pub fn marker(from: AgentId, to: AgentId) -> String {
    format!("<{TAG} from=\"{}\" to=\"{}\">", from.as_str(), to.as_str())
}

/// The agents of the switch a message tells of, when it begins with the
/// block.
pub fn parse_marker(text: &str) -> Option<(AgentId, AgentId)> {
    let rest = text.trim_start().strip_prefix(&format!("<{TAG} from=\""))?;
    let (from, rest) = rest.split_once("\" to=\"")?;
    let (to, _) = rest.split_once("\">")?;
    Some((AgentId::parse(from)?, AgentId::parse(to)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marker_reads_back_as_the_switch_it_names() {
        let open = marker(AgentId::CLAUDE, AgentId::CODEX);
        assert_eq!(open, "<leon-handoff from=\"claude\" to=\"codex\">");
        assert_eq!(
            parse_marker(&format!("  {open}\nbody\n</leon-handoff>")),
            Some((AgentId::CLAUDE, AgentId::CODEX))
        );
    }

    #[test]
    fn what_is_not_a_marker_is_not_a_switch() {
        assert_eq!(parse_marker("just words"), None);
        assert_eq!(parse_marker("<leon-handoff from=\"?\" to=\"codex\">"), None);
    }
}

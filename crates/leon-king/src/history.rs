//! King's past conversations.
//!
//! King talks with the person in one conversation at a time. The person may
//! start a new one; the one it replaces is *archived*: its summary ([`Past`])
//! stays in King's state, so a client lists them without reading anything
//! else, and its entries go to a file of their own beside the state, written
//! by the service. An archived conversation can be read, and resumed: its
//! agent's session is continued, and the conversation it replaces is
//! archived in turn.
//!
//! Pure: entries in, summaries out.

use serde::{Deserialize, Serialize};

use crate::transcript::{cut, Entry, Kind};

/// How many past conversations are kept; the oldest go first.
pub const KEPT: usize = 200;

/// How many characters of the first prompt make a conversation's title.
const TITLE_CHARS: usize = 90;

/// A past conversation, in short.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Past {
    /// Its number, unique among King's conversations.
    pub id: u64,
    /// What it was about: the first thing the person asked.
    pub title: String,
    /// When it started, in milliseconds since the epoch.
    pub started_ms: u64,
    /// When it was put away.
    pub ended_ms: u64,
    /// The agent it ran on, by tag.
    pub agent: String,
    /// The model it ran on, when one was chosen.
    pub model: Option<String>,
    /// The agent's id of it, to continue it.
    pub session_id: Option<String>,
    /// How many entries it kept.
    pub entries: usize,
}

/// A conversation just put away: its summary and what it said, for the
/// service to write.
#[derive(Debug, Clone, PartialEq)]
pub struct Archived {
    /// Its summary, now in the list.
    pub past: Past,
    /// What it said.
    pub entries: Vec<Entry>,
    /// Conversations that fell off the end of the list: their files go.
    pub dropped: Vec<u64>,
}

/// What a conversation was about: the first thing the person asked, on one
/// line; "(no question)" when it holds none.
pub fn title_of(entries: &[Entry]) -> String {
    entries
        .iter()
        .find(|entry| entry.kind == Kind::Prompt)
        .map(|entry| {
            cut(
                &entry.text.split_whitespace().collect::<Vec<_>>().join(" "),
                TITLE_CHARS,
            )
        })
        .unwrap_or_else(|| "(no question)".to_owned())
}

/// How many characters of King's last answer are carried into the next
/// conversation.
const CARRIED_CHARS: usize = 2000;

/// What King is told when a conversation put away for its length goes on
/// in a new one: what it was about, what the person asked last, and what
/// King answered last.
pub fn carried(past: &Past, entries: &[Entry]) -> String {
    let mut text = format!(
        "[Leon] This conversation goes on from a longer one that Leon put away to save \
         tokens (past conversation {}, \"{}\"; `leon orchestrator history {}` reads it). Where it \
         left off:",
        past.id, past.title, past.id
    );
    let asked: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.kind == Kind::Prompt)
        .collect();
    for entry in asked.iter().rev().take(3).rev() {
        text.push_str(&format!(
            "\n- The person asked: {}",
            cut(entry.text.trim(), 400)
        ));
    }
    if let Some(answer) = entries.iter().rev().find(|entry| entry.kind == Kind::Agent) {
        text.push_str(&format!(
            "\n- You answered last:\n{}",
            cut(answer.text.trim(), CARRIED_CHARS)
        ));
    }
    text.push_str("\nYour workers, tasks and memory are as they were: read them with your tools.");
    text
}

/// Adds `past` to the list, newest last, and says which fell off the end.
pub fn keep(list: &mut Vec<Past>, past: Past) -> Vec<u64> {
    list.push(past);
    let extra = list.len().saturating_sub(KEPT);
    list.drain(..extra).map(|gone| gone.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Kind, text: &str) -> Entry {
        Entry {
            seq: 0,
            at_ms: 0,
            kind,
            text: text.to_owned(),
            report: None,
        }
    }

    #[test]
    fn a_conversation_is_called_by_the_first_thing_asked() {
        let entries = [
            entry(Kind::News, "[Leon] News of your workers"),
            entry(Kind::Prompt, "What is the state\n  of   the driver app?"),
            entry(Kind::Prompt, "And the tests?"),
        ];
        assert_eq!(title_of(&entries), "What is the state of the driver app?");
        assert_eq!(title_of(&[]), "(no question)");
        let long = "word ".repeat(40);
        assert!(title_of(&[entry(Kind::Prompt, &long)]).chars().count() <= TITLE_CHARS + 1);
    }

    #[test]
    fn the_oldest_go_once_there_are_too_many() {
        let past = |id| Past {
            id,
            title: String::new(),
            started_ms: 0,
            ended_ms: 0,
            agent: "claude".into(),
            model: None,
            session_id: None,
            entries: 0,
        };
        let mut list: Vec<Past> = (0..KEPT as u64).map(past).collect();
        assert!(keep(&mut list, past(KEPT as u64)).len() == 1);
        assert_eq!(list.len(), KEPT);
        assert_eq!(list[0].id, 1);
    }
}

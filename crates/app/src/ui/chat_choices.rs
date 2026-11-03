//! What a conversation was set to run with, remembered for when it is opened
//! again.
//!
//! A conversation's permission mode, model and effort are the person's
//! choices under the field (`Chat::mode`, `model`, `effort`), and the agent
//! may change the mode itself (leaving plan mode). They lived only as long as
//! the conversation was open: closed and opened again from the sidebar, or
//! after Leon started again, it was back to the agent's own setup. So they
//! are kept here, by the agent and the agent's own id of the session (what a
//! conversation resumes), and a conversation that continues that session
//! starts with them ([`Shell::chat_restore_choices`]). One the agent no
//! longer offers is dropped once it says what it offers.
//!
//! The file is `chat_choices.json`, next to the settings; a missing or
//! unreadable file means none, and a write that fails is logged. It keeps the
//! [`KEPT`] sessions changed last.

use std::collections::BTreeMap;
use std::path::Path;

use leon_core::AgentId;
use serde::{Deserialize, Serialize};

use super::live::LiveId;
use super::shell::Shell;

/// The file's name, next to the settings file.
pub const FILE_NAME: &str = "chat_choices.json";

/// The most sessions whose choices are kept: the ones changed last.
const KEPT: usize = 500;

/// What one session runs with: `None` is the agent's own setup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    /// The permission mode.
    pub mode: Option<String>,
    /// The model.
    pub model: Option<String>,
    /// How hard the model thinks.
    pub effort: Option<String>,
    /// When it was last changed, in seconds since the epoch.
    pub at: i64,
}

impl Choice {
    fn same_as(&self, other: &Choice) -> bool {
        self.mode == other.mode && self.model == other.model && self.effort == other.effort
    }

    fn is_empty(&self) -> bool {
        self.mode.is_none() && self.model.is_none() && self.effort.is_none()
    }
}

/// The choices of every session remembered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatChoices {
    sessions: BTreeMap<String, Choice>,
}

/// The key of a session: the agent and its own id of it.
pub fn key(agent: AgentId, session: &str) -> String {
    format!("{}:{session}", agent.as_str())
}

impl ChatChoices {
    /// Reads the file, or none when it is missing or unreadable.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "the conversations' choices file is unreadable; using none");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes the file whole, through a temporary one.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let draft = path.with_extension("json.tmp");
        std::fs::write(&draft, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&draft, path)
    }

    /// What the session `key` was set to run with, when anything.
    pub fn get(&self, key: &str) -> Option<&Choice> {
        self.sessions.get(key)
    }

    /// The session `key` runs with `choice` now (`at` is ignored and set to
    /// `now`). `true` when that changed what is kept, so the file is to be
    /// written.
    pub fn remember(&mut self, key: String, choice: Choice, now: i64) -> bool {
        match self.sessions.get(&key) {
            Some(kept) if kept.same_as(&choice) => return false,
            None if choice.is_empty() => return false,
            _ => {}
        }
        self.sessions.insert(key, Choice { at: now, ..choice });
        if self.sessions.len() > KEPT {
            let mut by_age: Vec<(i64, String)> = self
                .sessions
                .iter()
                .map(|(key, choice)| (choice.at, key.clone()))
                .collect();
            by_age.sort();
            for (_, old) in by_age.into_iter().take(self.sessions.len() - KEPT) {
                self.sessions.remove(&old);
            }
        }
        true
    }
}

impl Shell {
    /// A conversation that continues the agent's session `session` starts
    /// with what that session was last set to run with.
    pub(super) fn chat_restore_choices(&mut self, id: LiveId, session: &str) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let Some(choice) = self.chat_choices.get(&key(chat.agent, session)) else {
            return;
        };
        chat.mode = choice.mode.clone();
        chat.model = choice.model.clone();
        chat.effort = choice.effort.clone();
    }

    /// Keeps what a conversation runs with now, under its session, once the
    /// agent has given the session an id; the file is written when that
    /// changed anything.
    pub(super) fn chat_remember_choices(&mut self, id: LiveId) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let Some(session) = chat.conversation.session_id.as_deref() else {
            return;
        };
        let choice = Choice {
            mode: chat.mode.clone(),
            model: chat.model.clone(),
            effort: chat.effort.clone(),
            at: 0,
        };
        let key = key(chat.agent, session);
        let now = self.now().timestamp();
        if !self.chat_choices.remember(key, choice, now) {
            return;
        }
        if let Some(file) = &self.chat_choices_file {
            if let Err(error) = self.chat_choices.save(file) {
                tracing::warn!(%error, "the conversations' choices could not be saved");
            }
        }
    }

    /// The agent said what it offers: a mode or a model kept from before
    /// that it no longer has is dropped, so a turn is not started with it.
    pub(super) fn chat_drop_stale_choices(&mut self, id: LiveId) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let Some(options) = chat.options.as_ref() else {
            return;
        };
        if !options.modes.is_empty()
            && chat
                .mode
                .as_ref()
                .is_some_and(|mode| !options.modes.iter().any(|offered| &offered.id == mode))
        {
            chat.mode = None;
        }
        if !options.models.is_empty()
            && chat
                .model
                .as_ref()
                .is_some_and(|model| !options.models.iter().any(|offered| &offered.id == model))
        {
            chat.model = None;
        }
        if chat
            .effort
            .as_ref()
            .is_some_and(|effort| !chat.efforts().contains(effort))
        {
            chat.effort = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto() -> Choice {
        Choice {
            mode: Some("auto".into()),
            ..Choice::default()
        }
    }

    #[test]
    fn a_choice_is_kept_by_agent_and_session_and_written_only_when_it_changes() {
        let mut choices = ChatChoices::default();
        let claude = key(AgentId::CLAUDE, "s1");
        assert!(
            !choices.remember(claude.clone(), Choice::default(), 1),
            "the agent's own setup is nothing to keep"
        );
        assert!(choices.remember(claude.clone(), auto(), 2));
        assert!(
            !choices.remember(claude.clone(), auto(), 3),
            "the same again"
        );
        assert_eq!(choices.get(&claude).unwrap().mode.as_deref(), Some("auto"));
        assert_eq!(choices.get(&claude).unwrap().at, 2);
        assert!(choices.get(&key(AgentId::CODEX, "s1")).is_none());
        // Back to the agent's own is a change, kept as such.
        assert!(choices.remember(claude.clone(), Choice::default(), 4));
        assert_eq!(choices.get(&claude).unwrap().mode, None);
    }

    #[test]
    fn only_the_sessions_changed_last_are_kept() {
        let mut choices = ChatChoices::default();
        for n in 0..(KEPT + 3) {
            choices.remember(key(AgentId::CLAUDE, &n.to_string()), auto(), n as i64);
        }
        assert_eq!(choices.sessions.len(), KEPT);
        assert!(choices.get(&key(AgentId::CLAUDE, "0")).is_none());
        assert!(choices
            .get(&key(AgentId::CLAUDE, &(KEPT + 2).to_string()))
            .is_some());
    }

    #[test]
    fn the_file_is_read_back_and_a_broken_one_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut choices = ChatChoices::default();
        choices.remember(key(AgentId::CLAUDE, "s1"), auto(), 1);
        choices.save(&path).unwrap();
        assert_eq!(ChatChoices::load(&path), choices);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(ChatChoices::load(&path), ChatChoices::default());
        assert_eq!(
            ChatChoices::load(&dir.path().join("missing.json")),
            ChatChoices::default()
        );
    }
}

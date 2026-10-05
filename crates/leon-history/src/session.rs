//! The normalised result of parsing one agent session, and the builder the
//! parsers share to produce it.
//!
//! Every parser walks its input once and reports what it finds to a
//! [`SessionBuilder`]: messages, the working directory, the model, title
//! candidates. The builder owns the rules that must be the same for every
//! agent, such as how a title is chosen, how a missing timestamp is filled in
//! and how oversized messages are clipped, so the three parsers cannot drift
//! apart.

use chrono::{DateTime, Utc};
use leon_core::{AgentKind, MachineId, NewMessage, NewSession, Role};

use crate::normalize::{clip, title_from};

/// The longest message text kept, in bytes. Longer texts are cut at a
/// character boundary. Pasted logs and generated files would otherwise
/// dominate the search index without making anything easier to find.
pub(crate) const MAX_MESSAGE_BYTES: usize = 32 * 1024;

/// Title given to a session in which no usable title was found.
pub(crate) const UNTITLED: &str = "Untitled session";

/// One agent session in Leon's vocabulary, not yet tied to a machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSession {
    /// The agent that ran the session.
    pub agent: AgentKind,
    /// The agent's own identifier, the value its resume option accepts.
    pub external_id: String,
    /// Working directory of the session; empty when the transcript does not
    /// record one.
    pub cwd: String,
    /// Short human-readable title.
    pub title: String,
    /// Model name, when the transcript records one.
    pub model: Option<String>,
    /// Time of the first message, in UTC.
    pub started_at: DateTime<Utc>,
    /// Time of the latest message, in UTC.
    pub updated_at: DateTime<Utc>,
    /// The transcript, in order.
    pub messages: Vec<NewMessage>,
    /// How many lines or rows could not be understood and were skipped.
    pub malformed: usize,
}

impl ParsedSession {
    /// The session as the store expects it, attributed to `machine_id`.
    pub fn new_session(&self, machine_id: &MachineId) -> NewSession {
        NewSession {
            agent: self.agent,
            external_id: self.external_id.clone(),
            machine_id: machine_id.clone(),
            cwd: self.cwd.clone(),
            title: self.title.clone(),
            model: self.model.clone(),
            started_at: self.started_at,
            updated_at: self.updated_at,
        }
    }
}

/// Accumulates what a parser finds and turns it into a [`ParsedSession`].
#[derive(Debug)]
pub(crate) struct SessionBuilder {
    agent: AgentKind,
    external_id: String,
    cwd: Option<String>,
    model: Option<String>,
    title: Option<String>,
    first_prompt: Option<String>,
    first_user_text: Option<String>,
    messages: Vec<NewMessage>,
    latest: Option<DateTime<Utc>>,
    malformed: usize,
}

impl SessionBuilder {
    pub(crate) fn new(agent: AgentKind, external_id: &str) -> Self {
        Self {
            agent,
            external_id: external_id.to_owned(),
            cwd: None,
            model: None,
            title: None,
            first_prompt: None,
            first_user_text: None,
            messages: Vec::new(),
            latest: None,
            malformed: 0,
        }
    }

    /// Replaces the external id, for transcripts that state their own.
    pub(crate) fn set_external_id(&mut self, external_id: &str) {
        if !external_id.is_empty() {
            self.external_id = external_id.to_owned();
        }
    }

    /// Records the working directory; the first one seen wins, because that
    /// is where the session was started.
    pub(crate) fn see_cwd(&mut self, cwd: Option<&str>) {
        if self.cwd.is_none() {
            if let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) {
                self.cwd = Some(cwd.to_owned());
            }
        }
    }

    /// Records the model; the latest one seen wins.
    pub(crate) fn see_model(&mut self, model: Option<&str>) {
        if let Some(model) = model.filter(|model| !model.is_empty()) {
            self.model = Some(model.to_owned());
        }
    }

    /// Records a title stated by the transcript itself; the latest wins.
    pub(crate) fn see_title(&mut self, title: Option<&str>) {
        if let Some(title) = title.map(title_from).filter(|title| !title.is_empty()) {
            self.title = Some(title);
        }
    }

    /// Counts a line or row that could not be understood.
    pub(crate) fn skip_malformed(&mut self) {
        self.malformed += 1;
    }

    /// Appends a message. Blank text is dropped. A message without a
    /// timestamp inherits the time of the one before it.
    pub(crate) fn push(&mut self, role: Role, text: &str, at: Option<DateTime<Utc>>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if role == Role::User {
            // A prompt typed by a person makes a better title than text the
            // harness wrapped in a tag, so the two are tracked separately.
            if self.first_user_text.is_none() {
                self.first_user_text = Some(title_from(text));
            }
            if self.first_prompt.is_none() && !text.starts_with('<') {
                self.first_prompt = Some(title_from(text));
            }
        }
        let at = at.or(self.latest).unwrap_or(DateTime::UNIX_EPOCH);
        self.latest = Some(self.latest.map_or(at, |latest| latest.max(at)));
        self.messages.push(NewMessage {
            role,
            text: clip(text, MAX_MESSAGE_BYTES).to_owned(),
            at,
        });
    }

    /// Finishes the session. Returns `None` when no message was found, since
    /// a session without content has nothing to show or search.
    pub(crate) fn finish(self) -> Option<ParsedSession> {
        let started_at = self.messages.iter().map(|message| message.at).min()?;
        let updated_at = self.latest.unwrap_or(started_at);
        let title = self
            .title
            .or(self.first_prompt)
            .or(self.first_user_text)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| UNTITLED.to_owned());
        Some(ParsedSession {
            agent: self.agent,
            external_id: self.external_id,
            cwd: self.cwd.unwrap_or_default(),
            title,
            model: self.model,
            started_at,
            updated_at,
            messages: self.messages,
            malformed: self.malformed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    fn builder() -> SessionBuilder {
        SessionBuilder::new(AgentKind::Claude, "s1")
    }

    #[test]
    fn a_session_without_messages_is_not_a_session() {
        let mut empty = builder();
        empty.see_cwd(Some("/srv/api"));
        empty.push(Role::User, "   ", Some(at(1)));
        assert!(empty.finish().is_none());
    }

    #[test]
    fn the_time_span_covers_the_first_and_latest_message() {
        let mut session = builder();
        session.push(Role::User, "one", Some(at(50)));
        session.push(Role::Assistant, "two", Some(at(90)));
        session.push(Role::User, "three", Some(at(70)));
        let parsed = session.finish().unwrap();
        assert_eq!(parsed.started_at, at(50));
        assert_eq!(parsed.updated_at, at(90));
    }

    #[test]
    fn a_message_without_a_timestamp_inherits_the_previous_one() {
        let mut session = builder();
        session.push(Role::User, "one", Some(at(50)));
        session.push(Role::Assistant, "two", None);
        assert_eq!(session.finish().unwrap().messages[1].at, at(50));
    }

    #[test]
    fn a_stated_title_wins_over_the_first_prompt() {
        let mut session = builder();
        session.push(Role::User, "fix the login flow", Some(at(1)));
        session.see_title(Some("Login redirect bug"));
        assert_eq!(session.finish().unwrap().title, "Login redirect bug");
    }

    #[test]
    fn the_first_typed_prompt_is_preferred_over_wrapped_text() {
        let mut session = builder();
        session.push(Role::User, "<command-name>init</command-name>", Some(at(1)));
        session.push(Role::User, "fix the login flow", Some(at(2)));
        assert_eq!(session.finish().unwrap().title, "fix the login flow");
    }

    #[test]
    fn wrapped_text_is_used_when_nothing_else_is_available() {
        let mut session = builder();
        session.push(Role::User, "<note>only this</note>", Some(at(1)));
        assert_eq!(session.finish().unwrap().title, "<note>only this</note>");
    }

    #[test]
    fn a_session_with_no_user_text_is_untitled() {
        let mut session = builder();
        session.push(Role::Assistant, "hello", Some(at(1)));
        assert_eq!(session.finish().unwrap().title, UNTITLED);
    }

    #[test]
    fn the_first_working_directory_and_the_latest_model_win() {
        let mut session = builder();
        session.see_cwd(Some(""));
        session.see_cwd(Some("/srv/api"));
        session.see_cwd(Some("/srv/other"));
        session.see_model(Some("model-a"));
        session.see_model(None);
        session.see_model(Some("model-b"));
        session.push(Role::User, "hi", Some(at(1)));
        let parsed = session.finish().unwrap();
        assert_eq!(parsed.cwd, "/srv/api");
        assert_eq!(parsed.model.as_deref(), Some("model-b"));
    }

    #[test]
    fn oversized_messages_are_clipped() {
        let mut session = builder();
        session.push(Role::User, &"é".repeat(MAX_MESSAGE_BYTES), Some(at(1)));
        let parsed = session.finish().unwrap();
        assert!(parsed.messages[0].text.len() <= MAX_MESSAGE_BYTES);
        assert!(parsed.messages[0].text.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_parsed_session_is_attributed_to_a_machine() {
        let mut session = builder();
        session.push(Role::User, "hi", Some(at(1)));
        let parsed = session.finish().unwrap();
        let new = parsed.new_session(&MachineId::local());
        assert_eq!(new.machine_id, MachineId::local());
        assert_eq!(new.external_id, "s1");
        assert_eq!(new.agent, AgentKind::Claude);
    }
}

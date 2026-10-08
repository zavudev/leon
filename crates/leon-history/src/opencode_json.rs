//! Reader for opencode's older file layout.
//!
//! Before opencode moved to SQLite it kept one JSON file per object under its
//! data folder (`<data>/opencode/storage`):
//!
//! * `session/<project id>/<session id>.json`: `id`, `directory`, `title`,
//!   `parentID` and `time` (`created`, `updated`, milliseconds);
//! * `message/<session id>/<message id>.json`: `id`, `role`, `modelID`;
//! * `part/<message id>/<part id>.json`: `id`, `type`, `text`, `tool`, `state`.
//!
//! These are the same objects the SQLite layout keeps in the `data` columns,
//! so the transcript is built by the same code ([`apply_part`]). Versions that
//! moved to SQLite leave this folder behind, and some installed versions
//! still write it, so Leon reads both.
//!
//! The parser here is pure: it takes the bytes of the files, never the file
//! system.

use std::collections::HashMap;

use chrono::DateTime;
use leon_core::{AgentId, Role};
use serde::Deserialize;

use crate::opencode::{apply_part, flush, MessageData, PartData, Pending};
use crate::session::{ParsedSession, SessionBuilder};
use crate::tokens::opencode_counts;

#[derive(Debug, Deserialize)]
struct Time {
    created: Option<i64>,
    updated: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Info {
    id: Option<String>,
    directory: Option<String>,
    title: Option<String>,
    #[serde(rename = "parentID")]
    parent_id: Option<String>,
    time: Option<Time>,
}

#[derive(Debug, Deserialize)]
struct MessageHeader {
    id: Option<String>,
    time: Option<Time>,
    #[serde(flatten)]
    data: MessageData,
}

#[derive(Debug, Deserialize)]
struct PartHeader {
    id: Option<String>,
    #[serde(flatten)]
    data: PartData,
}

/// One message file with the part files that belong to it.
#[derive(Debug, Clone, Default)]
pub struct JsonMessage {
    /// The bytes of the message file.
    pub info: Vec<u8>,
    /// The bytes of each of its part files.
    pub parts: Vec<Vec<u8>>,
}

/// What a session file turned out to be.
#[derive(Debug, PartialEq, Eq)]
pub enum JsonOutcome {
    /// The file is not a session object.
    Unreadable,
    /// The session was spawned by another one; it is not listed.
    Child,
    /// The session has no messages yet.
    Empty,
    /// A session with a transcript.
    Session(Box<ParsedSession>),
}

/// The id and parent of a session file, for listing without reading the
/// transcript. `None` when the file is not a session object.
pub fn peek(info: &[u8]) -> Option<(String, bool)> {
    let info: Info = serde_json::from_slice(info).ok()?;
    Some((info.id?, info.parent_id.is_some()))
}

/// Builds a session from its file and its message files.
pub fn parse_session(info: &[u8], messages: &[JsonMessage]) -> JsonOutcome {
    let Ok(info) = serde_json::from_slice::<Info>(info) else {
        return JsonOutcome::Unreadable;
    };
    let Some(id) = info.id.filter(|id| !id.is_empty()) else {
        return JsonOutcome::Unreadable;
    };
    if info.parent_id.is_some() {
        return JsonOutcome::Child;
    }
    let mut session = SessionBuilder::new(AgentId::OPENCODE, &id);
    session.see_cwd(info.directory.as_deref());
    session.see_title(info.title.as_deref());

    // Turns in time order, then parts in id order, as the database path does.
    let mut turns: Vec<(i64, String, MessageHeader, &JsonMessage)> = Vec::new();
    for message in messages {
        match serde_json::from_slice::<MessageHeader>(&message.info) {
            Ok(header) => {
                let at = header.time.as_ref().and_then(|t| t.created).unwrap_or(0);
                let id = header.id.clone().unwrap_or_default();
                turns.push((at, id, header, message));
            }
            Err(_) => session.skip_malformed(),
        }
    }
    turns.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));

    let mut roles: HashMap<String, Option<Role>> = HashMap::new();
    let mut model = None;
    for (_, id, header, _) in &turns {
        let role = match header.data.role.as_deref() {
            Some("user") => Some(Role::User),
            Some("assistant") => Some(Role::Assistant),
            _ => None,
        };
        if let (Some(Role::Assistant), Some(tokens)) = (role, &header.data.tokens) {
            let at = header
                .time
                .as_ref()
                .and_then(|time| time.created)
                .and_then(DateTime::from_timestamp_millis);
            session.see_tokens(header.data.model_id.as_deref(), at, opencode_counts(tokens));
        }
        if header.data.model_id.is_some() {
            model = header.data.model_id.clone();
        }
        roles.insert(id.clone(), role);
    }
    session.see_model(model.as_deref());

    let mut pending: Option<Pending> = None;
    for (at, id, _, message) in &turns {
        let mut parts: Vec<(String, PartHeader)> = Vec::new();
        for bytes in &message.parts {
            match serde_json::from_slice::<PartHeader>(bytes) {
                Ok(part) => parts.push((part.id.clone().unwrap_or_default(), part)),
                Err(_) => session.skip_malformed(),
            }
        }
        parts.sort_by(|a, b| a.0.cmp(&b.0));
        let at = DateTime::from_timestamp_millis(*at);
        for (_, part) in parts {
            apply_part(
                &mut session,
                &mut pending,
                &roles,
                id.clone(),
                at,
                part.data,
            );
        }
    }
    flush(&mut session, pending.take());

    let created = info
        .time
        .as_ref()
        .and_then(|t| t.created)
        .and_then(DateTime::from_timestamp_millis);
    let updated = info
        .time
        .as_ref()
        .and_then(|t| t.updated)
        .and_then(DateTime::from_timestamp_millis);
    match session.finish() {
        None => JsonOutcome::Empty,
        Some(mut parsed) => {
            if let Some(created) = created {
                parsed.started_at = parsed.started_at.min(created);
            }
            if let Some(updated) = updated {
                parsed.updated_at = parsed.updated_at.max(updated);
            }
            JsonOutcome::Session(Box::new(parsed))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bytes(value: serde_json::Value) -> Vec<u8> {
        value.to_string().into_bytes()
    }

    fn session_file(parent: Option<&str>) -> Vec<u8> {
        bytes(
            json!({"id": "ses_j", "projectID": "p", "directory": "/srv/api",
            "title": "Old layout", "parentID": parent,
            "time": {"created": 1000, "updated": 9000}}),
        )
    }

    fn user() -> JsonMessage {
        JsonMessage {
            info: bytes(json!({"id": "msg_1", "role": "user", "time": {"created": 1000}})),
            parts: vec![bytes(
                json!({"id": "prt_1", "type": "text", "text": "hello"}),
            )],
        }
    }

    fn assistant() -> JsonMessage {
        JsonMessage {
            info: bytes(
                json!({"id": "msg_2", "role": "assistant", "modelID": "model-j",
                "time": {"created": 2000}}),
            ),
            parts: vec![
                bytes(json!({"id": "prt_b", "type": "text", "text": "second"})),
                bytes(json!({"id": "prt_a", "type": "tool", "tool": "read",
                    "state": {"title": "src/lib.rs"}})),
            ],
        }
    }

    #[test]
    fn a_session_is_built_from_its_files_in_time_and_id_order() {
        let JsonOutcome::Session(session) =
            parse_session(&session_file(None), &[assistant(), user()])
        else {
            panic!("expected a session");
        };
        assert_eq!(session.external_id, "ses_j");
        assert_eq!(session.cwd, "/srv/api");
        assert_eq!(session.title, "Old layout");
        assert_eq!(session.model.as_deref(), Some("model-j"));
        let lines: Vec<_> = session
            .messages
            .iter()
            .map(|m| (m.role, m.text.as_str()))
            .collect();
        assert_eq!(
            lines,
            [
                (Role::User, "hello"),
                (Role::Tool, "read: src/lib.rs"),
                (Role::Assistant, "second"),
            ]
        );
        assert_eq!(session.started_at.timestamp_millis(), 1000);
        assert_eq!(session.updated_at.timestamp_millis(), 9000);
    }

    #[test]
    fn an_assistant_files_tokens_are_counted_under_its_model_and_day() {
        let mut turn = assistant();
        turn.info = bytes(
            json!({"id": "msg_2", "role": "assistant", "modelID": "model-j",
            "time": {"created": 86_400_000 + 5},
            "tokens": {"input": 798, "output": 146, "reasoning": 52,
                       "cache": {"read": 12416, "write": 3}}}),
        );
        let JsonOutcome::Session(session) = parse_session(&session_file(None), &[user(), turn])
        else {
            panic!("expected a session");
        };
        assert_eq!(session.tokens.len(), 1);
        let counted = &session.tokens[0];
        assert_eq!(
            (counted.model.as_str(), counted.day.as_str()),
            ("model-j", "1970-01-02")
        );
        assert_eq!((counted.counts.input, counted.counts.output), (798, 198));
        assert_eq!(
            (counted.counts.cache_read, counted.counts.cache_write),
            (12416, 3)
        );
    }

    #[test]
    fn a_child_a_missing_id_and_an_empty_session_are_told_apart() {
        assert_eq!(
            parse_session(&session_file(Some("ses_p")), &[user()]),
            JsonOutcome::Child
        );
        assert_eq!(parse_session(b"not json", &[]), JsonOutcome::Unreadable);
        assert_eq!(
            parse_session(b"{\"title\":\"x\"}", &[]),
            JsonOutcome::Unreadable
        );
        assert_eq!(parse_session(&session_file(None), &[]), JsonOutcome::Empty);
        assert_eq!(peek(&session_file(Some("p"))), Some(("ses_j".into(), true)));
        assert_eq!(peek(b"[]"), None);
    }

    #[test]
    fn unreadable_message_and_part_files_are_counted() {
        let broken = JsonMessage {
            info: b"{oops".to_vec(),
            parts: vec![],
        };
        let mut with_bad_part = user();
        with_bad_part.parts.push(b"[broken".to_vec());
        let JsonOutcome::Session(session) =
            parse_session(&session_file(None), &[broken, with_bad_part])
        else {
            panic!("expected a session");
        };
        assert_eq!(session.malformed, 2);
        assert_eq!(session.messages.len(), 1);
    }
}

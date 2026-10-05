//! Parser for Claude Code transcripts.
//!
//! Claude Code writes one JSON-lines file per session, named after the
//! session id. Each line is an independent JSON object with a `type`. Only a
//! few types matter here:
//!
//! * `user` and `assistant` lines carry a `message` whose `content` is either
//!   plain text or a list of blocks (`text`, `tool_use`, `tool_result`,
//!   `thinking`, `image`, ...), plus `cwd` and `timestamp`.
//! * `summary` and `ai-title` lines carry a title for the session.
//!
//! Everything else (attachments, file-history snapshots, mode changes, hook
//! records and whatever future versions add) is ignored without complaint.
//!
//! Normalisation rules:
//!
//! * Sidechain lines (sub-agent conversations) and meta lines (text injected
//!   by the harness rather than typed) are skipped.
//! * Text blocks become messages with the line's role. A `tool_use` block
//!   becomes one [`Role::Tool`] line. Tool results and thinking blocks are
//!   dropped: they are bulky, and what was asked and what was answered is
//!   what people search for.
//! * A line that is not valid JSON, including a final line cut short while
//!   the agent was still writing, is counted and skipped.

use leon_core::{AgentKind, Role};
use serde::Deserialize;

use crate::normalize::{parse_timestamp, tool_line, Block, Content};
use crate::session::{ParsedSession, SessionBuilder};

/// The placeholder Claude Code records as the model of messages it generated
/// itself rather than received from a model.
const SYNTHETIC_MODEL: &str = "<synthetic>";

#[derive(Debug, Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,
    #[serde(rename = "isMeta")]
    is_meta: Option<bool>,
    #[serde(rename = "isCompactSummary")]
    is_compact_summary: Option<bool>,
    cwd: Option<String>,
    timestamp: Option<String>,
    message: Option<LineMessage>,
    summary: Option<String>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LineMessage {
    model: Option<String>,
    content: Option<Content>,
}

/// Parses the bytes of one Claude Code transcript file.
///
/// `external_id` is the session id, which is the file name without its
/// extension; it is the value `claude --resume` accepts. Returns `None` when
/// the file holds no messages.
pub fn parse_session(external_id: &str, bytes: &[u8]) -> Option<ParsedSession> {
    let mut session = SessionBuilder::new(AgentKind::Claude, external_id);
    let mut summary: Option<String> = None;
    let mut ai_title: Option<String> = None;

    for raw in bytes.split(|byte| *byte == b'\n') {
        if raw.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(line) = serde_json::from_slice::<Line>(raw) else {
            session.skip_malformed();
            continue;
        };
        match line.kind.as_deref() {
            Some("summary") => {
                if summary.is_none() {
                    summary = line.summary;
                }
            }
            Some("ai-title") => {
                if line.ai_title.is_some() {
                    ai_title = line.ai_title;
                }
            }
            Some(kind @ ("user" | "assistant")) => {
                if line.is_sidechain == Some(true) || line.is_meta == Some(true) {
                    continue;
                }
                let role = match (kind, line.is_compact_summary) {
                    (_, Some(true)) => Role::System,
                    ("user", _) => Role::User,
                    _ => Role::Assistant,
                };
                read_message(&mut session, role, line);
            }
            _ => {}
        }
    }

    session.see_title(ai_title.as_deref().or(summary.as_deref()));
    session.finish()
}

fn read_message(session: &mut SessionBuilder, role: Role, line: Line) {
    session.see_cwd(line.cwd.as_deref());
    let at = line.timestamp.as_deref().and_then(parse_timestamp);
    let Some(message) = line.message else {
        return;
    };
    if role == Role::Assistant {
        session.see_model(message.model.as_deref().filter(|m| *m != SYNTHETIC_MODEL));
    }
    match message.content {
        Some(Content::Text(text)) => session.push(role, &text, at),
        Some(Content::Blocks(blocks)) => {
            for block in blocks {
                read_block(session, role, block, at);
            }
        }
        None => {}
    }
}

fn read_block(
    session: &mut SessionBuilder,
    role: Role,
    block: Block,
    at: Option<chrono::DateTime<chrono::Utc>>,
) {
    match block.kind.as_deref() {
        Some("text") => {
            if let Some(text) = block.text {
                session.push(role, &text, at);
            }
        }
        Some("tool_use") => {
            let line = tool_line(block.name.as_deref().unwrap_or(""), block.input.as_ref());
            session.push(Role::Tool, &line, at);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines(values: &[serde_json::Value]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(value.to_string().as_bytes());
            bytes.push(b'\n');
        }
        bytes
    }

    fn user(text: &str, timestamp: &str) -> serde_json::Value {
        json!({
            "type": "user", "isSidechain": false, "sessionId": "s1", "cwd": "/srv/api",
            "gitBranch": "main", "timestamp": timestamp, "uuid": "u",
            "message": {"role": "user", "content": text}
        })
    }

    fn assistant(blocks: serde_json::Value, timestamp: &str) -> serde_json::Value {
        json!({
            "type": "assistant", "isSidechain": false, "sessionId": "s1", "cwd": "/srv/api",
            "timestamp": timestamp, "uuid": "a",
            "message": {"role": "assistant", "model": "model-a", "content": blocks}
        })
    }

    fn roles_and_texts(session: &ParsedSession) -> Vec<(Role, &str)> {
        session
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect()
    }

    #[test]
    fn a_conversation_is_read_in_order_with_its_metadata() {
        let bytes = lines(&[
            user("fix the login flow", "2026-03-01T10:00:00.000Z"),
            assistant(
                json!([{"type": "text", "text": "Looking into it."}]),
                "2026-03-01T10:00:05.000Z",
            ),
        ]);
        let session = parse_session("s1", &bytes).unwrap();

        assert_eq!(session.agent, AgentKind::Claude);
        assert_eq!(session.external_id, "s1");
        assert_eq!(session.cwd, "/srv/api");
        assert_eq!(session.model.as_deref(), Some("model-a"));
        assert_eq!(session.title, "fix the login flow");
        assert_eq!(session.started_at.to_rfc3339(), "2026-03-01T10:00:00+00:00");
        assert_eq!(session.updated_at.to_rfc3339(), "2026-03-01T10:00:05+00:00");
        assert_eq!(
            roles_and_texts(&session),
            [
                (Role::User, "fix the login flow"),
                (Role::Assistant, "Looking into it.")
            ]
        );
        assert_eq!(session.malformed, 0);
    }

    #[test]
    fn a_tool_call_becomes_one_tool_line_and_its_result_is_dropped() {
        let bytes = lines(&[
            user("run the tests", "2026-03-01T10:00:00Z"),
            assistant(
                json!([
                    {"type": "thinking", "thinking": "private reasoning", "signature": "x"},
                    {"type": "tool_use", "id": "t1", "name": "Bash",
                     "input": {"command": "cargo test", "description": "run tests"}}
                ]),
                "2026-03-01T10:00:01Z",
            ),
            json!({
                "type": "user", "isSidechain": false, "cwd": "/srv/api",
                "timestamp": "2026-03-01T10:00:02Z",
                "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "test output"}
                ]}
            }),
        ]);
        let session = parse_session("s1", &bytes).unwrap();
        assert_eq!(
            roles_and_texts(&session),
            [
                (Role::User, "run the tests"),
                (Role::Tool, "Bash: cargo test")
            ]
        );
    }

    #[test]
    fn user_content_given_as_blocks_is_read_and_images_are_ignored() {
        let bytes = lines(&[json!({
            "type": "user", "cwd": "/srv/api", "timestamp": "2026-03-01T10:00:00Z",
            "message": {"role": "user", "content": [
                {"type": "text", "text": "what is in this picture"},
                {"type": "image", "source": {"type": "base64", "data": "AAAA"}}
            ]}
        })]);
        let session = parse_session("s1", &bytes).unwrap();
        assert_eq!(
            roles_and_texts(&session),
            [(Role::User, "what is in this picture")]
        );
    }

    #[test]
    fn unknown_line_types_are_ignored_without_failing() {
        let bytes = lines(&[
            json!({"type": "file-history-snapshot", "messageId": "m", "snapshot": {}}),
            json!({"type": "attachment", "attachment": {"kind": "x"}, "isSidechain": false}),
            json!({"type": "permission-mode", "permissionMode": "default", "sessionId": "s1"}),
            json!({"type": "something-from-the-future", "payload": [1, 2, 3]}),
            json!({"no_type_at_all": true}),
            user("hello", "2026-03-01T10:00:00Z"),
        ]);
        let session = parse_session("s1", &bytes).unwrap();
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.malformed, 0);
    }

    #[test]
    fn sidechain_and_meta_lines_are_skipped() {
        let mut sidechain = user("sub-agent prompt", "2026-03-01T10:00:00Z");
        sidechain["isSidechain"] = json!(true);
        let mut meta = user("injected caveat", "2026-03-01T10:00:01Z");
        meta["isMeta"] = json!(true);
        let bytes = lines(&[
            sidechain,
            meta,
            user("the real prompt", "2026-03-01T10:00:02Z"),
        ]);
        let session = parse_session("s1", &bytes).unwrap();
        assert_eq!(roles_and_texts(&session), [(Role::User, "the real prompt")]);
    }

    #[test]
    fn a_generated_title_wins_over_a_summary_and_the_first_prompt() {
        let with_summary = lines(&[
            json!({"type": "summary", "summary": "Login redirect bug", "leafUuid": "l"}),
            user("fix the login flow", "2026-03-01T10:00:00Z"),
        ]);
        assert_eq!(
            parse_session("s1", &with_summary).unwrap().title,
            "Login redirect bug"
        );

        let with_both = lines(&[
            json!({"type": "summary", "summary": "Login redirect bug", "leafUuid": "l"}),
            user("fix the login flow", "2026-03-01T10:00:00Z"),
            json!({"type": "ai-title", "aiTitle": "First title", "sessionId": "s1"}),
            json!({"type": "ai-title", "aiTitle": "Latest title", "sessionId": "s1"}),
        ]);
        assert_eq!(
            parse_session("s1", &with_both).unwrap().title,
            "Latest title"
        );
    }

    #[test]
    fn a_long_first_prompt_is_truncated_into_the_title() {
        let prompt = "please ".repeat(60);
        let bytes = lines(&[user(&prompt, "2026-03-01T10:00:00Z")]);
        let session = parse_session("s1", &bytes).unwrap();
        assert!(session.title.chars().count() <= 80);
        assert!(session.title.ends_with('…'));
        assert_eq!(session.messages[0].text, prompt.trim());
    }

    #[test]
    fn malformed_and_truncated_lines_are_counted_and_skipped() {
        let mut bytes = lines(&[user("first", "2026-03-01T10:00:00Z")]);
        bytes.extend_from_slice(b"this is not json\n");
        bytes.extend_from_slice(b"{\"type\":\"user\",\"message\":42}\n");
        bytes.extend_from_slice(&[0xff, 0xfe, b'\n']);
        bytes.extend_from_slice(b"\n   \n");
        bytes.extend_from_slice(&lines(&[user("second", "2026-03-01T10:00:01Z")]));
        bytes
            .extend_from_slice(b"{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"te");

        let session = parse_session("s1", &bytes).unwrap();
        assert_eq!(
            roles_and_texts(&session),
            [(Role::User, "first"), (Role::User, "second")]
        );
        assert_eq!(session.malformed, 4);
    }

    #[test]
    fn a_file_without_messages_yields_nothing() {
        assert!(parse_session("s1", b"").is_none());
        let bytes = lines(&[json!({"type": "mode", "mode": "normal", "sessionId": "s1"})]);
        assert!(parse_session("s1", &bytes).is_none());
    }

    #[test]
    fn timestamps_with_an_offset_are_converted_to_utc() {
        let bytes = lines(&[user("hi", "2026-03-01T12:00:00+02:00")]);
        let session = parse_session("s1", &bytes).unwrap();
        assert_eq!(session.started_at.to_rfc3339(), "2026-03-01T10:00:00+00:00");
    }

    #[test]
    fn the_synthetic_model_placeholder_is_not_reported_as_a_model() {
        let mut reply = assistant(
            json!([{"type": "text", "text": "ok"}]),
            "2026-03-01T10:00:00Z",
        );
        reply["message"]["model"] = json!("<synthetic>");
        let session = parse_session("s1", &lines(&[reply])).unwrap();
        assert_eq!(session.model, None);
    }

    #[test]
    fn a_compaction_summary_is_recorded_as_a_system_message() {
        let mut compacted = user("Summary of the conversation so far", "2026-03-01T10:00:00Z");
        compacted["isCompactSummary"] = json!(true);
        let bytes = lines(&[compacted, user("continue", "2026-03-01T10:00:01Z")]);
        let session = parse_session("s1", &bytes).unwrap();
        assert_eq!(session.messages[0].role, Role::System);
        assert_eq!(session.title, "continue");
    }
}

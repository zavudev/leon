//! Parser for Codex CLI rollout files.
//!
//! Codex writes one JSON-lines "rollout" file per session. Current versions
//! wrap every record in an envelope, `{ timestamp, type, payload }`:
//!
//! * `session_meta`: the payload carries the session `id`, `cwd` and the
//!   CLI version.
//! * `turn_context`: the payload carries the `model` and `cwd` of a turn.
//! * `response_item`: the payload is a conversation item whose own `type` is
//!   `message` (with `role` and `content` blocks), `function_call`,
//!   `custom_tool_call`, `web_search_call`, `reasoning`, and so on.
//!
//! Every other envelope type (`event_msg`, `world_state`, `realtime_item`,
//! token accounting, ...) is ignored.
//!
//! Early versions wrote the same records without the envelope: a first line
//! holding the session `id` directly and bare conversation items after it.
//! Both shapes are accepted by treating a record without a `payload` as its
//! own payload.
//!
//! Normalisation rules:
//!
//! * `user` and `assistant` messages become messages. `developer` and
//!   `system` messages are instructions injected by the harness and are
//!   dropped.
//! * Tool calls become one [`Role::Tool`] line; their outputs and reasoning
//!   items are dropped.
//! * Unparseable lines, including a final line cut short, are counted and
//!   skipped.

use leon_core::{AgentId, Role};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::Value;

use crate::normalize::{parse_timestamp, tool_line, Content};
use crate::session::{ParsedSession, SessionBuilder};

/// The envelope around a record. The payload is kept as raw JSON so that the
/// many record types Leon does not read are skipped without being decoded.
#[derive(Debug, Deserialize)]
struct Envelope<'a> {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(borrow)]
    payload: Option<&'a RawValue>,
}

/// The fields Leon reads from a payload, whichever record type it belongs
/// to. All are optional so one shape serves every type.
#[derive(Debug, Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    id: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    role: Option<String>,
    content: Option<Content>,
    name: Option<String>,
    arguments: Option<Value>,
    input: Option<Value>,
}

/// Parses the bytes of one Codex rollout file.
///
/// `fallback_id` is used as the session id when the file does not state one;
/// callers pass the id embedded in the file name. The id is the value
/// `codex resume` accepts. Returns `None` when the file holds no messages.
pub fn parse_session(fallback_id: &str, bytes: &[u8]) -> Option<ParsedSession> {
    let mut session = SessionBuilder::new(AgentId::CODEX, fallback_id);
    let mut named = false;

    for raw in bytes.split(|byte| *byte == b'\n') {
        if raw.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(envelope) = serde_json::from_slice::<Envelope<'_>>(raw) else {
            session.skip_malformed();
            continue;
        };
        let at = envelope.timestamp.as_deref().and_then(parse_timestamp);
        let kind = envelope.kind.as_deref();
        let record = match (kind, envelope.payload) {
            (Some("session_meta" | "turn_context" | "response_item"), Some(payload)) => {
                serde_json::from_str::<Record>(payload.get())
            }
            (_, Some(_)) => continue,
            // No envelope: the line is the record itself.
            (_, None) => serde_json::from_slice::<Record>(raw),
        };
        let Ok(record) = record else {
            session.skip_malformed();
            continue;
        };
        match (kind, envelope.payload.is_some()) {
            (Some("session_meta"), true) | (None, false) => {
                // A rollout forked from another session repeats the parent's
                // header after its own, so only the first header that names
                // a session is read.
                if !named {
                    read_meta(&mut session, &record);
                    named = record.id.is_some() || record.session_id.is_some();
                }
            }
            (Some("turn_context"), true) => {
                session.see_model(record.model.as_deref());
                session.see_cwd(record.cwd.as_deref());
            }
            _ => read_item(&mut session, record, at),
        }
    }

    session.finish()
}

fn read_meta(session: &mut SessionBuilder, meta: &Record) {
    if let Some(id) = meta.id.as_deref().or(meta.session_id.as_deref()) {
        session.set_external_id(id);
    }
    session.see_cwd(meta.cwd.as_deref());
    session.see_model(meta.model.as_deref());
}

fn read_item(
    session: &mut SessionBuilder,
    item: Record,
    at: Option<chrono::DateTime<chrono::Utc>>,
) {
    let at = at.or_else(|| item.timestamp.as_deref().and_then(parse_timestamp));
    match item.kind.as_deref() {
        Some("message") => {
            let role = match item.role.as_deref() {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => return,
            };
            match item.content {
                Some(Content::Text(text)) => session.push(role, &text, at),
                Some(Content::Blocks(blocks)) => {
                    let text = blocks
                        .iter()
                        .filter_map(|block| block.text.as_deref())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    session.push(role, &text, at);
                }
                None => {}
            }
        }
        Some("function_call") => {
            // Arguments arrive as JSON encoded in a string.
            let arguments = match item.arguments {
                Some(Value::String(encoded)) => {
                    Some(serde_json::from_str(&encoded).unwrap_or(Value::String(encoded)))
                }
                other => other,
            };
            let line = tool_line(item.name.as_deref().unwrap_or(""), arguments.as_ref());
            session.push(Role::Tool, &line, at);
        }
        Some("custom_tool_call") => {
            let line = tool_line(item.name.as_deref().unwrap_or(""), item.input.as_ref());
            session.push(Role::Tool, &line, at);
        }
        Some(kind @ ("local_shell_call" | "web_search_call" | "tool_search_call")) => {
            let name = kind.trim_end_matches("_call");
            session.push(Role::Tool, &tool_line(name, None), at);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines(values: &[Value]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(value.to_string().as_bytes());
            bytes.push(b'\n');
        }
        bytes
    }

    fn envelope(kind: &str, timestamp: &str, payload: Value, ordinal: u64) -> Value {
        json!({"timestamp": timestamp, "type": kind, "payload": payload, "ordinal": ordinal})
    }

    fn meta() -> Value {
        envelope(
            "session_meta",
            "2026-03-01T10:00:00.000Z",
            json!({
                "id": "codex-session-1", "session_id": "codex-session-1",
                "timestamp": "2026-03-01T10:00:00.000Z", "cwd": "/srv/api",
                "originator": "codex_cli", "cli_version": "0.1.0",
                "model_provider": "provider", "base_instructions": {"text": "be helpful"}
            }),
            0,
        )
    }

    fn message(role: &str, block_type: &str, text: &str, timestamp: &str) -> Value {
        envelope(
            "response_item",
            timestamp,
            json!({"type": "message", "role": role,
                   "content": [{"type": block_type, "text": text}]}),
            1,
        )
    }

    fn roles_and_texts(session: &ParsedSession) -> Vec<(Role, &str)> {
        session
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect()
    }

    #[test]
    fn a_rollout_is_read_with_its_metadata() {
        let bytes = lines(&[
            meta(),
            envelope(
                "turn_context",
                "2026-03-01T10:00:01Z",
                json!({"model": "model-c", "cwd": "/srv/api", "effort": "high"}),
                1,
            ),
            message(
                "user",
                "input_text",
                "add a health check",
                "2026-03-01T10:00:02Z",
            ),
            message("assistant", "output_text", "Added.", "2026-03-01T10:00:09Z"),
        ]);
        let session = parse_session("from-file-name", &bytes).unwrap();

        assert_eq!(session.agent, AgentId::CODEX);
        assert_eq!(session.external_id, "codex-session-1");
        assert_eq!(session.cwd, "/srv/api");
        assert_eq!(session.model.as_deref(), Some("model-c"));
        assert_eq!(session.title, "add a health check");
        assert_eq!(session.started_at.to_rfc3339(), "2026-03-01T10:00:02+00:00");
        assert_eq!(session.updated_at.to_rfc3339(), "2026-03-01T10:00:09+00:00");
        assert_eq!(
            roles_and_texts(&session),
            [
                (Role::User, "add a health check"),
                (Role::Assistant, "Added.")
            ]
        );
    }

    #[test]
    fn a_forked_rollout_keeps_its_own_id_and_ignores_the_repeated_parent_header() {
        let parent = envelope(
            "session_meta",
            "2026-03-01T09:00:00Z",
            json!({"id": "parent-session", "session_id": "parent-session", "cwd": "/srv/parent"}),
            1,
        );
        let mut own = meta();
        own["payload"]["forked_from_id"] = json!("parent-session");
        let bytes = lines(&[
            own,
            parent,
            message(
                "user",
                "input_text",
                "continue from the fork",
                "2026-03-01T10:00:02Z",
            ),
        ]);
        let session = parse_session("x", &bytes).unwrap();
        assert_eq!(session.external_id, "codex-session-1");
        assert_eq!(session.cwd, "/srv/api");
    }

    #[test]
    fn injected_instructions_are_dropped() {
        let bytes = lines(&[
            meta(),
            message(
                "developer",
                "input_text",
                "follow the rules",
                "2026-03-01T10:00:01Z",
            ),
            message("system", "input_text", "more rules", "2026-03-01T10:00:01Z"),
            message("user", "input_text", "hello", "2026-03-01T10:00:02Z"),
        ]);
        let session = parse_session("x", &bytes).unwrap();
        assert_eq!(roles_and_texts(&session), [(Role::User, "hello")]);
    }

    #[test]
    fn several_text_blocks_of_one_message_are_joined() {
        let bytes = lines(&[envelope(
            "response_item",
            "2026-03-01T10:00:02Z",
            json!({"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "first part"},
                {"type": "input_image", "image_url": "data:..."},
                {"type": "input_text", "text": "second part"}
            ]}),
            1,
        )]);
        let session = parse_session("x", &bytes).unwrap();
        assert_eq!(session.messages[0].text, "first part\n\nsecond part");
    }

    #[test]
    fn tool_calls_become_tool_lines_and_their_outputs_are_dropped() {
        let bytes = lines(&[
            meta(),
            envelope(
                "response_item",
                "2026-03-01T10:00:03Z",
                json!({"type": "function_call", "name": "shell", "call_id": "c1",
                       "arguments": "{\"command\":[\"cargo\",\"check\"]}"}),
                2,
            ),
            envelope(
                "response_item",
                "2026-03-01T10:00:04Z",
                json!({"type": "function_call_output", "call_id": "c1", "output": "ok"}),
                3,
            ),
            envelope(
                "response_item",
                "2026-03-01T10:00:05Z",
                json!({"type": "custom_tool_call", "name": "apply_patch", "call_id": "c2",
                       "status": "completed", "input": "*** Begin Patch\n*** Update File: a.rs"}),
                4,
            ),
            envelope(
                "response_item",
                "2026-03-01T10:00:06Z",
                json!({"type": "reasoning", "summary": [], "encrypted_content": "zzz"}),
                5,
            ),
            envelope(
                "response_item",
                "2026-03-01T10:00:07Z",
                json!({"type": "web_search_call", "status": "completed"}),
                6,
            ),
        ]);
        let session = parse_session("x", &bytes).unwrap();
        assert_eq!(
            roles_and_texts(&session),
            [
                (Role::Tool, "shell: cargo check"),
                (Role::Tool, "apply_patch: *** Begin Patch"),
                (Role::Tool, "web_search")
            ]
        );
    }

    #[test]
    fn a_tool_call_whose_arguments_are_not_json_still_produces_a_line() {
        let bytes = lines(&[envelope(
            "response_item",
            "2026-03-01T10:00:03Z",
            json!({"type": "function_call", "name": "shell", "arguments": "not json at all"}),
            1,
        )]);
        let session = parse_session("x", &bytes).unwrap();
        assert_eq!(session.messages[0].text, "shell: not json at all");
    }

    #[test]
    fn other_record_types_are_ignored() {
        let bytes = lines(&[
            meta(),
            envelope(
                "event_msg",
                "2026-03-01T10:00:01Z",
                json!({"type": "token_count"}),
                1,
            ),
            envelope(
                "world_state",
                "2026-03-01T10:00:01Z",
                json!({"anything": 1}),
                2,
            ),
            envelope(
                "realtime_item",
                "2026-03-01T10:00:01Z",
                json!({"type": "transcript_segment", "role": "user", "content": "spoken"}),
                3,
            ),
            envelope(
                "compacted",
                "2026-03-01T10:00:01Z",
                json!({"message": "x"}),
                4,
            ),
            json!({"timestamp": "2026-03-01T10:00:01Z", "type": "token_usage_record", "payload": 7}),
            envelope(
                "event_msg",
                "2026-03-01T10:00:01Z",
                json!({"id": 5, "content": 9}),
                5,
            ),
            message("user", "input_text", "hello", "2026-03-01T10:00:02Z"),
        ]);
        let session = parse_session("x", &bytes).unwrap();
        assert_eq!(roles_and_texts(&session), [(Role::User, "hello")]);
        assert_eq!(session.malformed, 0, "ignored records are never decoded");
    }

    #[test]
    fn a_record_leon_reads_with_an_unusable_payload_is_counted_as_malformed() {
        let bytes = lines(&[
            json!({"timestamp": "2026-03-01T10:00:01Z", "type": "response_item", "payload": 7}),
            message("user", "input_text", "hello", "2026-03-01T10:00:02Z"),
        ]);
        let session = parse_session("x", &bytes).unwrap();
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.malformed, 1);
    }

    #[test]
    fn an_older_rollout_without_envelopes_is_read() {
        let bytes = lines(&[
            json!({"id": "old-session", "timestamp": "2025-05-01T08:00:00Z",
                   "instructions": "be helpful", "git": {"branch": "main"}}),
            json!({"record_type": "state"}),
            json!({"type": "message", "role": "user",
                   "content": [{"type": "input_text", "text": "rename the module"}]}),
            json!({"type": "function_call", "name": "shell",
                   "arguments": "{\"command\":[\"ls\"]}"}),
            json!({"type": "message", "role": "assistant",
                   "content": [{"type": "output_text", "text": "Renamed."}]}),
        ]);
        let session = parse_session("from-file-name", &bytes).unwrap();
        assert_eq!(session.external_id, "old-session");
        assert_eq!(
            roles_and_texts(&session),
            [
                (Role::User, "rename the module"),
                (Role::Tool, "shell: ls"),
                (Role::Assistant, "Renamed.")
            ]
        );
    }

    #[test]
    fn the_file_name_id_is_used_when_the_rollout_states_none() {
        let bytes = lines(&[message(
            "user",
            "input_text",
            "hello",
            "2026-03-01T10:00:02Z",
        )]);
        let session = parse_session("from-file-name", &bytes).unwrap();
        assert_eq!(session.external_id, "from-file-name");
        assert_eq!(session.cwd, "");
    }

    #[test]
    fn malformed_and_truncated_lines_are_counted_and_skipped() {
        let mut bytes = lines(&[
            meta(),
            message("user", "input_text", "one", "2026-03-01T10:00:02Z"),
        ]);
        bytes.extend_from_slice(b"garbage\n");
        bytes.extend_from_slice(&lines(&[message(
            "assistant",
            "output_text",
            "two",
            "2026-03-01T10:00:03Z",
        )]));
        bytes.extend_from_slice(b"{\"timestamp\":\"2026-03-01T10:00:04Z\",\"type\":\"response_it");

        let session = parse_session("x", &bytes).unwrap();
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.malformed, 2);
    }

    #[test]
    fn a_rollout_without_messages_yields_nothing() {
        assert!(parse_session("x", &lines(&[meta()])).is_none());
        assert!(parse_session("x", b"").is_none());
    }
}

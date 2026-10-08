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
//!
//! Token counts come from the `message.usage` of assistant lines and are read
//! apart from the messages, so a sidechain line (a sub-agent's call, which
//! was paid for all the same) counts although its text is not kept. Current
//! Claude Code writes a sub-agent's calls to a file of their own,
//! `<session-id>/subagents/agent-*.jsonl` beside the session file;
//! [`parse_session_with`] counts those files into the session that started
//! them, reading only their usage. The lines of one reply share `message.id`
//! and each repeats the reply's usage, so a reply counts once, with the
//! largest figure any of its lines gave, whichever of the files it is in. A
//! reply repeated in another session file (a resumed or forked session
//! copies the earlier ones) is counted in each file; nothing here can tell.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use leon_core::{AgentId, Role, TokenCounts};
use serde::Deserialize;
use serde_json::Value;

use crate::normalize::{parse_timestamp, tool_line, Block, Content};
use crate::session::{ParsedSession, SessionBuilder};
use crate::tokens::claude_counts;

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
    id: Option<String>,
    model: Option<String>,
    content: Option<Content>,
    /// Kept as written: a usage of a shape nobody knows counts as nothing
    /// instead of making the whole line unreadable.
    usage: Option<Value>,
}

/// One model reply as its lines report it.
struct Reply {
    model: Option<String>,
    at: Option<DateTime<Utc>>,
    counts: TokenCounts,
}

/// The replies of a file, in order, one per `message.id`.
#[derive(Default)]
struct Replies {
    list: Vec<Reply>,
    by_id: HashMap<String, usize>,
}

impl Replies {
    /// Notes the usage one assistant line reports.
    fn see(&mut self, message: &LineMessage, at: Option<DateTime<Utc>>) {
        let Some(usage) = &message.usage else {
            return;
        };
        if message.model.as_deref() == Some(SYNTHETIC_MODEL) {
            return;
        }
        let counts = claude_counts(usage);
        let known = message
            .id
            .as_ref()
            .and_then(|id| self.by_id.get(id).copied());
        match known {
            Some(at_index) => {
                let seen = &mut self.list[at_index].counts;
                seen.input = seen.input.max(counts.input);
                seen.output = seen.output.max(counts.output);
                seen.cache_read = seen.cache_read.max(counts.cache_read);
                seen.cache_write = seen.cache_write.max(counts.cache_write);
                seen.cache_write_1h = seen.cache_write_1h.max(counts.cache_write_1h);
            }
            None => {
                if let Some(id) = &message.id {
                    self.by_id.insert(id.clone(), self.list.len());
                }
                self.list.push(Reply {
                    model: message.model.clone(),
                    at,
                    counts,
                });
            }
        }
    }
}

/// The part of a line a sub-agent file is read for: its usage and nothing
/// else, so a large file is not parsed down to its text.
#[derive(Debug, Deserialize)]
struct UsageLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    message: Option<UsageMessage>,
}

#[derive(Debug, Deserialize)]
struct UsageMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Value>,
}

impl Replies {
    /// Notes the usage of every assistant line of one sub-agent file. Lines
    /// that cannot be read are left out: the file is not a session, so there
    /// is no malformed count to put them in.
    fn see_file(&mut self, bytes: &[u8]) {
        for raw in bytes.split(|byte| *byte == b'\n') {
            if raw.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(line) = serde_json::from_slice::<UsageLine>(raw) else {
                continue;
            };
            if let (Some("assistant"), Some(message)) = (line.kind.as_deref(), line.message) {
                let at = line.timestamp.as_deref().and_then(parse_timestamp);
                self.see(
                    &LineMessage {
                        id: message.id,
                        model: message.model,
                        content: None,
                        usage: message.usage,
                    },
                    at,
                );
            }
        }
    }
}

/// Parses the bytes of one Claude Code transcript file.
///
/// `external_id` is the session id, which is the file name without its
/// extension; it is the value `claude --resume` accepts. Returns `None` when
/// the file holds no messages.
pub fn parse_session(external_id: &str, bytes: &[u8]) -> Option<ParsedSession> {
    parse_session_with(external_id, bytes, std::iter::empty::<Vec<u8>>())
}

/// Like [`parse_session`], and counts the usage of the session's sub-agent
/// files too. They are pulled one at a time, so only one is in memory at once;
/// their text is not kept.
pub fn parse_session_with<B: AsRef<[u8]>>(
    external_id: &str,
    bytes: &[u8],
    subagents: impl IntoIterator<Item = B>,
) -> Option<ParsedSession> {
    let mut session = SessionBuilder::new(AgentId::CLAUDE, external_id);
    let mut summary: Option<String> = None;
    let mut ai_title: Option<String> = None;
    let mut replies = Replies::default();

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
                if let (true, Some(message)) = (kind == "assistant", &line.message) {
                    let at = line.timestamp.as_deref().and_then(parse_timestamp);
                    replies.see(message, at);
                }
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

    for file in subagents {
        replies.see_file(file.as_ref());
    }

    for reply in replies.list {
        session.see_tokens(reply.model.as_deref(), reply.at, reply.counts);
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

        assert_eq!(session.agent, AgentId::CLAUDE);
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

    fn counted(session: &ParsedSession) -> Vec<(&str, &str, u64, u64, u64, u64, u64)> {
        session
            .tokens
            .iter()
            .map(|t| {
                let c = &t.counts;
                (
                    t.model.as_str(),
                    t.day.as_str(),
                    c.input,
                    c.output,
                    c.cache_read,
                    c.cache_write,
                    c.cache_write_1h,
                )
            })
            .collect()
    }

    /// An assistant line as Claude Code writes it, with the usage it recorded.
    fn reply(id: &str, timestamp: &str, output: u64) -> serde_json::Value {
        json!({
            "type": "assistant", "isSidechain": false, "cwd": "/srv/api",
            "timestamp": timestamp, "requestId": "req_1",
            "message": {"id": id, "role": "assistant", "model": "model-a",
                "content": [{"type": "text", "text": "ok"}],
                "usage": {
                    "input_tokens": 2, "cache_creation_input_tokens": 14227,
                    "cache_read_input_tokens": 30516, "output_tokens": output,
                    "cache_creation": {"ephemeral_1h_input_tokens": 14000,
                                       "ephemeral_5m_input_tokens": 227},
                    "service_tier": "standard"
                }}
        })
    }

    #[test]
    fn the_lines_of_one_reply_count_once_with_the_largest_figures() {
        let bytes = lines(&[
            user("go", "2026-03-01T10:00:00Z"),
            reply("msg_1", "2026-03-01T10:00:01Z", 8),
            reply("msg_1", "2026-03-01T10:00:02Z", 54),
            reply("msg_1", "2026-03-01T10:00:03Z", 54),
            reply("msg_2", "2026-03-01T10:00:09Z", 10),
        ]);
        let session = parse_session("s1", &bytes).unwrap();
        assert_eq!(
            counted(&session),
            [("model-a", "2026-03-01", 4, 64, 61032, 454, 28000)]
        );
    }

    #[test]
    fn a_sub_agents_calls_are_counted_although_its_text_is_not_kept() {
        let mut side = reply("msg_side", "2026-03-01T10:00:05Z", 20);
        side["isSidechain"] = json!(true);
        let bytes = lines(&[
            user("go", "2026-03-01T10:00:00Z"),
            reply("msg_1", "2026-03-01T10:00:01Z", 54),
            side,
        ]);
        let session = parse_session("s1", &bytes).unwrap();
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.tokens[0].counts.output, 74);
    }

    #[test]
    fn replies_are_counted_on_the_utc_day_of_their_line_per_model() {
        let mut late = reply("msg_2", "2026-03-02T00:00:05+02:00", 1);
        late["message"]["model"] = json!("model-b");
        let bytes = lines(&[
            user("go", "2026-03-01T10:00:00Z"),
            reply("msg_1", "2026-03-01T23:59:59Z", 5),
            reply("msg_3", "2026-03-02T00:00:00Z", 7),
            late,
        ]);
        let session = parse_session("s1", &bytes).unwrap();
        let got: Vec<_> = session
            .tokens
            .iter()
            .map(|t| (t.model.as_str(), t.day.as_str(), t.counts.output))
            .collect();
        assert_eq!(
            got,
            [
                ("model-a", "2026-03-01", 5),
                ("model-a", "2026-03-02", 7),
                // 00:00:05 at +02:00 is still the 1st in UTC.
                ("model-b", "2026-03-01", 1),
            ]
        );
    }

    #[test]
    fn synthetic_replies_and_unreadable_usage_count_as_nothing() {
        let mut synthetic = reply("msg_s", "2026-03-01T10:00:01Z", 99);
        synthetic["message"]["model"] = json!("<synthetic>");
        let mut odd = reply("msg_o", "2026-03-01T10:00:02Z", 99);
        odd["message"]["usage"] = json!("none");
        let bytes = lines(&[user("go", "2026-03-01T10:00:00Z"), synthetic, odd]);
        let session = parse_session("s1", &bytes).unwrap();
        assert!(session.tokens.is_empty());
        assert_eq!(session.malformed, 0);
    }

    #[test]
    fn a_reply_without_an_id_counts_each_line() {
        let mut a = reply("x", "2026-03-01T10:00:01Z", 3);
        let mut b = reply("x", "2026-03-01T10:00:02Z", 4);
        a["message"].as_object_mut().unwrap().remove("id");
        b["message"].as_object_mut().unwrap().remove("id");
        let bytes = lines(&[user("go", "2026-03-01T10:00:00Z"), a, b]);
        let session = parse_session("s1", &bytes).unwrap();
        assert_eq!(session.tokens[0].counts.output, 7);
    }

    #[test]
    fn a_sub_agent_file_is_counted_into_its_session_without_its_text() {
        let mut inside = reply("msg_side", "2026-03-01T10:00:05Z", 20);
        inside["isSidechain"] = json!(true);
        let mut again = reply("msg_side", "2026-03-01T10:00:06Z", 25);
        again["isSidechain"] = json!(true);
        let mut other_model = reply("msg_other", "2026-03-02T01:00:00Z", 3);
        other_model["isSidechain"] = json!(true);
        other_model["message"]["model"] = json!("model-b");
        let sub_agent = lines(&[
            user("sub-agent prompt", "2026-03-01T10:00:04Z"),
            inside,
            again,
            other_model,
            json!({"type": "assistant", "message": {"id": "x", "usage": "none"}}),
        ]);
        let mut cut = sub_agent.clone();
        cut.extend_from_slice(b"{\"type\":\"assistant\",\"message\":{\"id\":");
        let main = lines(&[
            user("go", "2026-03-01T10:00:00Z"),
            reply("msg_1", "2026-03-01T10:00:01Z", 54),
        ]);

        let session = parse_session_with("s1", &main, [cut]).unwrap();

        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.malformed, 0);
        let got: Vec<_> = session
            .tokens
            .iter()
            .map(|t| (t.model.as_str(), t.day.as_str(), t.counts.output))
            .collect();
        assert_eq!(
            got,
            [
                ("model-a", "2026-03-01", 54 + 25),
                ("model-b", "2026-03-02", 3)
            ]
        );
    }

    #[test]
    fn a_reply_in_the_session_and_in_a_sub_agent_file_counts_once() {
        let main = lines(&[
            user("go", "2026-03-01T10:00:00Z"),
            reply("msg_1", "2026-03-01T10:00:01Z", 54),
        ]);
        let sub_agent = lines(&[reply("msg_1", "2026-03-01T10:00:02Z", 54)]);
        let session = parse_session_with("s1", &main, [sub_agent.clone(), sub_agent]).unwrap();
        assert_eq!(session.tokens[0].counts.output, 54);
    }
}

//! Reads one line of a Codex rollout into beats.
//!
//! The envelope is the one [`crate::codex`] describes: `{ timestamp, type,
//! payload }`, with early versions writing the payload bare. A live view
//! reads more of it than the history does:
//!
//! * `event_msg` payloads of type `task_started`, `task_complete` and
//!   `turn_aborted` open, close and interrupt a turn; `token_count` carries
//!   the token numbers of the last model call and the context window.
//! * `response_item` payloads are the conversation: an assistant `message`,
//!   a `reasoning` item, a tool call (`function_call`, `custom_tool_call`,
//!   `local_shell_call`, each with a `call_id`) and its output
//!   (`function_call_output`, `custom_tool_call_output`).
//! * `compacted` records a compaction.
//!
//! Current versions route most tools through one `exec` call whose input is
//! a short script such as `const r = await tools.exec_command({"cmd": ...})`
//! or a patch in a string. Such a call is reported as the tool the script
//! uses, so a command, a patch and a page view are told apart.
//!
//! Codex records no sub-agent conversation in a rollout, and nothing about a
//! pending approval.

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::Value;

use super::beat::{
    base_name, detail, heard, patched_file, seconds, speech, Beat, ToolKind, MAX_DETAIL_CHARS,
};
use crate::normalize::{clip, truncate_chars};

/// How the text the harness writes for the folder's instructions opens, in
/// a message of the user's role.
const INJECTED_HEADING: &str = "# AGENTS.md instructions";

/// The heading over what the user typed, in a message that also carries
/// what they attached (`# Files mentioned by the user:`, `# Diff comments:`).
const OWN_REQUEST: &str = "## My request:";

/// How much of a script is searched for the tool it uses.
const SCRIPT_HEAD_BYTES: usize = 4096;

#[derive(Debug, Deserialize)]
struct Envelope<'a> {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(borrow)]
    payload: Option<&'a RawValue>,
    timestamp: Option<String>,
}

/// The fields read from a payload, whichever record it belongs to.
#[derive(Debug, Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: Option<String>,
    role: Option<String>,
    name: Option<String>,
    call_id: Option<Value>,
    id: Option<Value>,
    arguments: Option<Value>,
    input: Option<Value>,
    action: Option<Value>,
    output: Option<Value>,
    content: Option<Value>,
    info: Option<TokenInfo>,
}

#[derive(Debug, Deserialize)]
struct TokenInfo {
    last_token_usage: Option<TokenUsage>,
    model_context_window: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

/// Reads one complete line. `Err` means the line is not a JSON object, or is
/// a record Leon reads whose payload has an unusable shape.
pub(crate) fn read_line(raw: &[u8], out: &mut Vec<Beat>) -> Result<(), ()> {
    let envelope: Envelope<'_> = serde_json::from_slice(raw).map_err(|_| ())?;
    match (envelope.kind.as_deref(), envelope.payload) {
        (Some("event_msg"), Some(payload)) => read_event(parse(payload.get())?, out),
        (Some("response_item"), Some(payload)) => read_item(
            parse(payload.get())?,
            seconds(envelope.timestamp.as_deref()),
            out,
        ),
        (Some("compacted"), Some(_)) => out.push(Beat::Compacted {
            before: None,
            after: None,
        }),
        (_, Some(_)) => {}
        // No envelope: an early rollout, where the line is the item itself.
        (_, None) => read_item(serde_json::from_slice(raw).map_err(|_| ())?, None, out),
    }
    Ok(())
}

fn parse(payload: &str) -> Result<Record, ()> {
    serde_json::from_str(payload).map_err(|_| ())
}

fn read_event(event: Record, out: &mut Vec<Beat>) {
    match event.kind.as_deref() {
        Some("task_started") => out.push(Beat::Prompt),
        Some("task_complete") => out.push(Beat::TurnEnded),
        Some("turn_aborted") => out.push(Beat::Interrupted),
        Some("token_count") => {
            let Some(info) = event.info else {
                return;
            };
            if let Some(usage) = info.last_token_usage {
                out.push(Beat::Usage {
                    context: usage.input_tokens,
                    output: usage.output_tokens,
                    window: info.model_context_window,
                });
            }
        }
        _ => {}
    }
}

/// The text of a message item: its `output_text` blocks one after the
/// other (a plain string in the earliest rollouts).
fn message_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn read_item(item: Record, at: Option<i64>, out: &mut Vec<Beat>) {
    // The call id links a call to its output; a record without one falls
    // back to its own id.
    let id = || {
        [&item.call_id, &item.id]
            .into_iter()
            .find_map(|id| id.as_ref().and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned()
    };
    match item.kind.as_deref() {
        Some("message") if item.role.as_deref() == Some("assistant") => out.push(Beat::Said {
            text: speech(&message_text(item.content.as_ref())),
            at,
        }),
        // What the user wrote. The harness writes its own text under the
        // same role (the instructions of the folder, the environment): a
        // block that opens with a tag or with that heading is not theirs.
        Some("message") if item.role.as_deref() == Some("user") => {
            let own = |text: &&str| {
                let text = text.trim_start();
                !text.is_empty() && !text.starts_with('<') && !text.starts_with(INJECTED_HEADING)
            };
            let words = match item.content.as_ref() {
                Some(Value::String(text)) => Some(text.as_str()).filter(own).map(str::to_owned),
                Some(Value::Array(blocks)) => {
                    let kept: Vec<&str> = blocks
                        .iter()
                        .filter_map(|block| block.get("text").and_then(Value::as_str))
                        .filter(own)
                        .collect();
                    (!kept.is_empty()).then(|| kept.join("\n"))
                }
                _ => None,
            };
            // An interface that attaches files or comments writes them
            // first and the user's own words under this heading.
            let words = words.map(|words| match words.rfind(OWN_REQUEST) {
                Some(at) => heard(&words[at + OWN_REQUEST.len()..]),
                None => heard(&words),
            });
            if let Some(words) = words.filter(|words| !words.is_empty()) {
                out.push(Beat::Heard { text: words, at });
            }
        }
        Some("reasoning") => out.push(Beat::Thinking),
        Some("function_call") => {
            // Arguments arrive as JSON encoded in a string.
            let arguments = match item.arguments.clone() {
                Some(Value::String(encoded)) => {
                    Some(serde_json::from_str(&encoded).unwrap_or(Value::String(encoded)))
                }
                other => other,
            };
            started(
                id(),
                item.name.as_deref().unwrap_or_default(),
                arguments.as_ref(),
                out,
            );
        }
        Some("custom_tool_call") => {
            let name = item.name.as_deref().unwrap_or_default();
            match item.input.as_ref().and_then(Value::as_str) {
                Some(script) if name == "exec" => read_script(id(), script, out),
                _ => started(id(), name, item.input.as_ref(), out),
            }
        }
        Some("local_shell_call") => started(id(), "local_shell", item.action.as_ref(), out),
        Some("web_search_call") => {
            // The search runs on the provider's side: the record is written
            // once, when it is over, and has no separate output.
            let id = id();
            started(id.clone(), "web_search", item.action.as_ref(), out);
            out.push(Beat::ToolFinished {
                id,
                failed: false,
                refused: false,
            });
        }
        Some("function_call_output" | "custom_tool_call_output") => {
            out.push(Beat::ToolFinished {
                id: id(),
                failed: item.output.as_ref().is_some_and(output_failed),
                refused: false,
            });
        }
        _ => {}
    }
}

/// A tool call, and what more is known of it than its caption holds.
fn started(id: String, name: &str, input: Option<&Value>, out: &mut Vec<Beat>) {
    let kind = ToolKind::of(name);
    let detail = detail(kind, input);
    let brief = Beat::brief_of(&id, kind, name, input, &detail);
    out.push(Beat::ToolStarted {
        id,
        name: if name.is_empty() { "tool" } else { name }.to_owned(),
        kind,
        detail,
    });
    out.extend(brief);
}

/// Reports an `exec` script as the tool it uses.
fn read_script(id: String, script: &str, out: &mut Vec<Beat>) {
    let head = clip(script, SCRIPT_HEAD_BYTES);
    if head.contains("*** Begin Patch") {
        let file = patched_file(head).map(base_name).unwrap_or_default();
        out.push(Beat::ToolStarted {
            id,
            name: "apply_patch".to_owned(),
            kind: ToolKind::Edit,
            detail: truncate_chars(file, MAX_DETAIL_CHARS),
        });
        return;
    }
    match script_call(head) {
        Some((name, arguments)) => started(id, name, arguments.as_ref(), out),
        None => started(id, "exec", None, out),
    }
}

/// The first `tools.<name>(` of a script and, when the call is given a JSON
/// object, that object.
fn script_call(script: &str) -> Option<(&str, Option<Value>)> {
    let start = script.find("tools.")? + "tools.".len();
    let rest = &script[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let name = &rest[..end];
    if name.is_empty() {
        return None;
    }
    let arguments = rest[end..].strip_prefix('(').and_then(|arguments| {
        serde_json::Deserializer::from_str(arguments)
            .into_iter::<Value>()
            .next()?
            .ok()
    });
    Some((name, arguments))
}

/// Whether a tool's output says the tool failed: an `exec` script that
/// failed, or an early shell output with a non-zero exit code.
fn output_failed(output: &Value) -> bool {
    let text = match output {
        Value::String(text) => text.as_str(),
        Value::Array(blocks) => blocks
            .first()
            .and_then(|block| block.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        _ => return false,
    };
    let text = text.trim_start();
    if text.starts_with("Script failed") {
        return true;
    }
    text.starts_with('{')
        && serde_json::from_str::<Value>(text).is_ok_and(|json| {
            json.pointer("/metadata/exit_code")
                .and_then(Value::as_i64)
                .is_some_and(|code| code != 0)
        })
}

#[cfg(test)]
pub(crate) mod fixture {
    //! Synthetic records with the shape of Codex's.

    use serde_json::{json, Value};

    pub(crate) fn envelope(kind: &str, payload: Value) -> Value {
        json!({"timestamp": "2026-03-01T10:00:00.000Z", "type": kind, "payload": payload})
    }

    pub(crate) fn event(kind: &str) -> Value {
        envelope(
            "event_msg",
            json!({"type": kind, "turn_id": "turn-1", "started_at": 1_700_000_000}),
        )
    }

    pub(crate) fn item(payload: Value) -> Value {
        envelope("response_item", payload)
    }

    pub(crate) fn exec(call: &str, script: &str) -> Value {
        item(
            json!({"type": "custom_tool_call", "id": "ctc_1", "call_id": call,
            "name": "exec", "status": "completed", "input": script}),
        )
    }

    pub(crate) fn exec_output(call: &str, text: &str) -> Value {
        item(
            json!({"type": "custom_tool_call_output", "id": "ctco_1", "call_id": call,
            "output": [{"type": "input_text", "text": text}]}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use serde_json::json;

    fn beats(line: &Value) -> Vec<Beat> {
        let mut out = Vec::new();
        read_line(line.to_string().as_bytes(), &mut out).unwrap();
        out
    }

    fn tool(id: &str, name: &str, kind: ToolKind, detail: &str) -> Beat {
        Beat::ToolStarted {
            id: id.into(),
            name: name.into(),
            kind,
            detail: detail.into(),
        }
    }

    fn finished(id: &str, failed: bool) -> Beat {
        Beat::ToolFinished {
            id: id.into(),
            failed,
            refused: false,
        }
    }

    #[test]
    fn turn_events_open_close_and_interrupt_a_turn() {
        assert_eq!(beats(&event("task_started")), [Beat::Prompt]);
        assert_eq!(beats(&event("task_complete")), [Beat::TurnEnded]);
        assert_eq!(beats(&event("turn_aborted")), [Beat::Interrupted]);
        assert_eq!(beats(&event("thread_settings_applied")), []);
    }

    #[test]
    fn the_conversation_items_are_beats() {
        let said = item(
            json!({"type": "message", "role": "assistant", "phase": "final",
            "content": [{"type": "output_text", "text": "Added."}]}),
        );
        assert_eq!(
            beats(&said),
            [Beat::Said {
                text: "Added.".into(),
                at: Some(1_772_359_200),
            }]
        );
        // Several blocks are one message; an early rollout has no envelope
        // and so no time.
        let bare = json!({"type": "message", "role": "assistant",
            "content": [{"type": "output_text", "text": "One.\u{1b}[0m"},
                        {"type": "output_text", "text": "Two."}]});
        assert_eq!(
            beats(&bare),
            [Beat::Said {
                text: "One.[0m\nTwo.".into(),
                at: None
            }]
        );
        let reasoning = item(json!({"type": "reasoning", "summary": [], "encrypted_content": "z"}));
        assert_eq!(beats(&reasoning), [Beat::Thinking]);
        for role in ["user", "developer"] {
            let injected = item(json!({"type": "message", "role": role,
                "content": [{"type": "input_text", "text": "<environment_context>"}]}));
            assert_eq!(beats(&injected), [], "{role}");
        }
    }

    #[test]
    fn what_the_user_wrote_is_heard_and_what_the_harness_wrote_for_them_is_not() {
        let typed = item(json!({"type": "message", "role": "user",
            "content": [{"type": "input_text", "text": "rename the module\nand its tests"}]}));
        assert_eq!(
            beats(&typed),
            [Beat::Heard {
                text: "rename the module\nand its tests".into(),
                at: Some(1_772_359_200)
            }]
        );
        // The instructions of the folder and the environment come under the
        // same role, in one message: none of it is the user's.
        let harness = item(json!({"type": "message", "role": "user", "content": [
            {"type": "input_text", "text": "# AGENTS.md instructions\n\n<INSTRUCTIONS>be brief"},
            {"type": "input_text", "text": "<environment_context>\n  <cwd>/srv/api</cwd>"}]}));
        assert_eq!(beats(&harness), []);
        // A message that holds both keeps what the user typed.
        let mixed = item(json!({"type": "message", "role": "user", "content": [
            {"type": "input_text", "text": "<environment_context>"},
            {"type": "input_text", "text": "run the tests"}]}));
        assert!(
            matches!(&beats(&mixed)[..], [Beat::Heard { text, .. }] if text == "run the tests")
        );
        // The desktop interface writes what was attached first, and the
        // user's own words under a heading of its own.
        let attached = item(json!({"type": "message", "role": "user", "content": [
            {"type": "input_text", "text": "# Files mentioned by the user:\n\n## plan.png: /tmp/plan.png\n\n## My request:\n[Image #1] follow this plan\n"},
            {"type": "input_image", "image_url": "data:image/png;base64,AAAA"}]}));
        assert!(
            matches!(&beats(&attached)[..], [Beat::Heard { text, .. }] if text == "follow this plan")
        );
        let comments = item(json!({"type": "message", "role": "user", "content": [
            {"type": "input_text", "text": "# Diff comments:\n\n## User Comment 1\nFile: a.rs\nrename it\n\n## My request:\napply the comments"}]}));
        assert!(
            matches!(&beats(&comments)[..], [Beat::Heard { text, .. }] if text == "apply the comments")
        );
        // An early rollout wrote a plain string.
        let bare = json!({"type": "message", "role": "user", "content": "hello"});
        assert!(matches!(&beats(&bare)[..], [Beat::Heard { text, at: None }] if text == "hello"));
        // The developer's role is never the user.
        let developer = item(json!({"type": "message", "role": "developer",
            "content": [{"type": "input_text", "text": "be careful"}]}));
        assert_eq!(beats(&developer), []);
    }

    #[test]
    fn a_long_command_and_a_question_come_with_their_brief() {
        let long = "cargo test --workspace --locked --no-fail-fast -- --test-threads 1 --nocapture";
        let call = item(
            json!({"type": "function_call", "name": "shell", "call_id": "c1",
            "arguments": json!({"command": ["bash", "-lc", long]}).to_string()}),
        );
        let got = beats(&call);
        assert_eq!(got.len(), 2);
        assert!(
            matches!(&got[1], Beat::Brief { id, text, options }
                if id == "c1" && text.ends_with("--nocapture") && options.is_empty()),
            "{got:?}"
        );
        // A short command says nothing more than its caption.
        let short = item(
            json!({"type": "function_call", "name": "shell", "call_id": "c2",
            "arguments": json!({"command": "ls"}).to_string()}),
        );
        assert_eq!(beats(&short).len(), 1);
        let asked = item(
            json!({"type": "function_call", "name": "request_user_input",
            "call_id": "c3", "arguments": json!({"questions": [{"id": "q1", "header": "Scope",
                "question": "Which crate first?",
                "options": [{"label": "api", "description": "the server"},
                            {"label": "web", "description": "the client"}]}]}).to_string()}),
        );
        assert_eq!(
            beats(&asked)[1],
            Beat::Brief {
                id: "c3".into(),
                text: "Which crate first?".into(),
                options: vec!["api".into(), "web".into()]
            }
        );
    }

    #[test]
    fn a_function_call_and_its_output_start_and_finish_a_tool() {
        let call = item(
            json!({"type": "function_call", "name": "shell", "call_id": "c1",
            "arguments": "{\"command\":[\"cargo\",\"check\"]}"}),
        );
        assert_eq!(
            beats(&call),
            [tool("c1", "shell", ToolKind::Run, "cargo check")]
        );
        let ok = item(json!({"type": "function_call_output", "call_id": "c1",
            "output": "{\"output\":\"fine\",\"metadata\":{\"exit_code\":0}}"}));
        assert_eq!(beats(&ok), [finished("c1", false)]);
        let bad = item(json!({"type": "function_call_output", "call_id": "c1",
            "output": "{\"output\":\"boom\",\"metadata\":{\"exit_code\":101}}"}));
        assert_eq!(beats(&bad), [finished("c1", true)]);
    }

    #[test]
    fn an_exec_script_is_reported_as_the_tool_it_uses() {
        let command = exec(
            "c1",
            "const r = await tools.exec_command({\"cmd\":\"cargo test -p api\",\"workdir\":\"/srv/api\"}); text(r.output);",
        );
        assert_eq!(
            beats(&command),
            [tool(
                "c1",
                "exec_command",
                ToolKind::Run,
                "cargo test -p api"
            )]
        );
        let patch = exec(
            "c2",
            "const patch = \"*** Begin Patch\\n*** Update File: src/login/form.rs\\n@@\\n-a\\n+b\";\nawait tools.apply_patch(patch);",
        );
        assert_eq!(
            beats(&patch),
            [tool("c2", "apply_patch", ToolKind::Edit, "form.rs")]
        );
        let image = exec(
            "c3",
            "const r = await tools.view_image({\"path\":\"/srv/api/shot.png\"});",
        );
        assert_eq!(
            beats(&image),
            [
                tool("c3", "view_image", ToolKind::Read, "shot.png"),
                Beat::Brief {
                    id: "c3".into(),
                    text: "/srv/api/shot.png".into(),
                    options: Vec::new()
                }
            ]
        );
        let opaque = exec("c4", "text(1 + 1)");
        assert_eq!(beats(&opaque), [tool("c4", "exec", ToolKind::Run, "")]);
        let loose = exec("c5", "await tools.exec_command({cmd: 'ls'})");
        assert_eq!(
            beats(&loose),
            [tool("c5", "exec_command", ToolKind::Run, "")]
        );
    }

    #[test]
    fn a_script_that_failed_is_a_failed_tool() {
        assert_eq!(
            beats(&exec_output(
                "c1",
                "Script completed\nWall time 0.2 seconds\nOutput:\nok"
            )),
            [finished("c1", false)]
        );
        assert_eq!(
            beats(&exec_output(
                "c1",
                "Script failed\nWall time 0.2 seconds\nError: x"
            )),
            [finished("c1", true)]
        );
    }

    #[test]
    fn a_web_search_starts_and_finishes_at_once() {
        let search = item(
            json!({"type": "web_search_call", "id": "ws_1", "status": "completed",
            "action": {"type": "search", "query": "rust incremental parsing"}}),
        );
        assert_eq!(
            beats(&search),
            [
                tool(
                    "ws_1",
                    "web_search",
                    ToolKind::Web,
                    "rust incremental parsing"
                ),
                finished("ws_1", false)
            ]
        );
    }

    #[test]
    fn token_counts_report_the_last_call_and_the_window() {
        let mut line = event("token_count");
        line["payload"]["info"] = json!({
            "total_token_usage": {"input_tokens": 90000, "output_tokens": 900},
            "last_token_usage": {"input_tokens": 23441, "cached_input_tokens": 11904,
                "output_tokens": 196, "total_tokens": 23637},
            "model_context_window": 258400
        });
        assert_eq!(
            beats(&line),
            [Beat::Usage {
                context: 23441,
                output: 196,
                window: Some(258_400)
            }]
        );
        // The first count of a session has no numbers yet.
        let mut empty = event("token_count");
        empty["payload"]["info"] = json!(null);
        assert_eq!(beats(&empty), []);
    }

    #[test]
    fn a_compaction_is_a_beat_and_other_records_are_ignored() {
        assert_eq!(
            beats(&envelope("compacted", json!({"message": "x"}))),
            [Beat::Compacted {
                before: None,
                after: None
            }]
        );
        for line in [
            envelope("session_meta", json!({"id": "s", "cwd": "/srv/api"})),
            envelope("turn_context", json!({"model": "model-c"})),
            envelope("world_state", json!({"full": true})),
            json!({"timestamp": "2026-03-01T10:00:00Z", "type": "token_usage_record", "payload": 7}),
        ] {
            assert_eq!(beats(&line), [], "{line}");
        }
    }

    #[test]
    fn an_early_rollout_without_envelopes_is_read() {
        let call = json!({"type": "function_call", "name": "shell", "call_id": "c1",
            "arguments": "{\"command\":[\"ls\"]}"});
        assert_eq!(beats(&call), [tool("c1", "shell", ToolKind::Run, "ls")]);
        let header = json!({"id": "old-session", "timestamp": "2025-05-01T08:00:00Z"});
        assert_eq!(beats(&header), []);
    }

    #[test]
    fn unreadable_lines_are_errors_and_never_panics() {
        let mut out = Vec::new();
        assert!(read_line(b"garbage", &mut out).is_err());
        let odd = json!({"type": "response_item", "payload": 7}).to_string();
        assert!(read_line(odd.as_bytes(), &mut out).is_err());
        assert!(out.is_empty());
    }
}

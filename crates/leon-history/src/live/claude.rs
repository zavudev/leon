//! Reads one line of a Claude Code transcript into beats.
//!
//! Claude Code appends a JSON object per line while it works. The lines that
//! matter for a live view:
//!
//! * `assistant` lines carry **one content block each** (`thinking`, `text`
//!   or `tool_use`); the blocks of one model reply share `message.id`. The
//!   reply's `stop_reason` is on its last line, and on every line when the
//!   reply was written at once. `message.usage` holds the token numbers.
//! * `user` lines carry either what somebody sent (a string, or `text` and
//!   `image` blocks) or the `tool_result` blocks answering earlier calls,
//!   with `is_error` when the tool failed. The harness also writes its own
//!   notices as user lines: `origin.kind` says who is speaking
//!   (`human`, `task-notification`, `peer`), `isMeta` marks injected text.
//! * A sub-agent call (`Agent`, formerly `Task`) usually returns at once:
//!   the line with its result has `toolUseResult.status` `async_launched`
//!   and the sub-agent's `agentId`. The sub-agent's end arrives later as a
//!   `<task-notification>` user line naming that id in `<task-id>`.
//! * `system` lines with subtype `turn_duration` close a turn,
//!   `compact_boundary` records a compaction, `agents_killed` the stopping
//!   of every background sub-agent.
//!
//! Nothing is written between a `tool_use` line and its result, so a
//! permission question that is waiting for the user leaves no trace here.
//!
//! Each field is read leniently: a field of an unexpected shape is treated
//! as absent instead of making the line unreadable.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use serde_json::Value;

use super::beat::{detail, seconds, speech, Beat, TaskOutcome, ToolKind};

/// The stop reasons after which the model waits for the user.
const TURN_ENDING: [&str; 3] = ["end_turn", "stop_sequence", "refusal"];

/// How a tool result begins when the call was refused rather than run.
const REFUSALS: [&str; 3] = [
    "The user doesn't want to proceed",
    "Permission for this",
    "Permission to use",
];

/// How a user line begins when the user interrupted the turn.
const INTERRUPTED: &str = "[Request interrupted by user";

/// How user lines begin that record a local command, not a message.
const LOCAL_PREFIXES: [&str; 6] = [
    "<command-name>",
    "<command-message>",
    "<local-command-",
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
];

/// Reads a field of any shape and keeps it only when it has the shape `T`.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = Option::<&RawValue>::deserialize(deserializer)?;
    Ok(raw.and_then(|raw| serde_json::from_str(raw.get()).ok()))
}

#[derive(Debug, Deserialize)]
struct Line {
    #[serde(rename = "type", default, deserialize_with = "lenient")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    subtype: Option<String>,
    #[serde(rename = "isSidechain", default, deserialize_with = "lenient")]
    is_sidechain: Option<bool>,
    #[serde(rename = "isMeta", default, deserialize_with = "lenient")]
    is_meta: Option<bool>,
    #[serde(rename = "isCompactSummary", default, deserialize_with = "lenient")]
    is_compact_summary: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    origin: Option<Origin>,
    #[serde(default, deserialize_with = "lenient")]
    message: Option<Message>,
    #[serde(rename = "toolUseResult", default, deserialize_with = "lenient")]
    tool_use_result: Option<ToolUseResult>,
    #[serde(rename = "compactMetadata", default, deserialize_with = "lenient")]
    compact: Option<CompactMetadata>,
    #[serde(default, deserialize_with = "lenient")]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Origin {
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default, deserialize_with = "lenient")]
    content: Option<Content>,
    #[serde(default, deserialize_with = "lenient")]
    stop_reason: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    usage: Option<Usage>,
}

/// The `content` of a message: plain text or a list of blocks. A block of
/// an unexpected shape is left out without losing its neighbours.
#[derive(Debug)]
enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

impl<'de> Deserialize<'de> for Content {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Read by hand rather than as an untagged enum, which cannot hold
        // the raw JSON a block keeps.
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        if let Ok(text) = serde_json::from_str::<String>(raw.get()) {
            return Ok(Self::Text(text));
        }
        let blocks =
            serde_json::from_str::<Vec<&RawValue>>(raw.get()).map_err(serde::de::Error::custom)?;
        Ok(Self::Blocks(
            blocks
                .into_iter()
                .filter_map(|block| serde_json::from_str(block.get()).ok())
                .collect(),
        ))
    }
}

/// One content block. `content` (a tool result's output) is kept as raw
/// JSON: only its first bytes are ever looked at.
#[derive(Debug, Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<Value>,
    tool_use_id: Option<String>,
    is_error: Option<bool>,
    content: Option<Box<RawValue>>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

/// The harness's own record of a tool result. Only a launched sub-agent is
/// read from it.
#[derive(Debug, Deserialize)]
struct ToolUseResult {
    #[serde(rename = "agentId")]
    agent_id: Option<String>,
    status: Option<String>,
    #[serde(rename = "isAsync")]
    is_async: Option<bool>,
}

impl ToolUseResult {
    /// The id of the sub-agent this result launched into the background.
    fn launched(&self) -> Option<&str> {
        let background = self.is_async == Some(true)
            || self
                .status
                .as_deref()
                .is_some_and(|status| status.starts_with("async"));
        self.agent_id
            .as_deref()
            .filter(|id| background && !id.is_empty())
    }
}

#[derive(Debug, Deserialize)]
struct CompactMetadata {
    #[serde(rename = "preTokens")]
    before: Option<u64>,
    #[serde(rename = "postTokens")]
    after: Option<u64>,
}

/// Reads one complete line. `sidechain` says which lines belong to the
/// conversation being followed: `false` for a session's own transcript,
/// where sub-agent lines of older versions are left out, and `true` for a
/// sub-agent's own file. `Err` means the line is not valid JSON.
pub(crate) fn read_line(raw: &[u8], sidechain: bool, out: &mut Vec<Beat>) -> Result<(), ()> {
    let line: Line = serde_json::from_slice(raw).map_err(|_| ())?;
    if !sidechain && line.is_sidechain == Some(true) {
        return Ok(());
    }
    match line.kind.as_deref() {
        Some("assistant") => read_assistant(line, out),
        Some("user") => read_user(line, out),
        Some("system") => match line.subtype.as_deref() {
            Some("turn_duration") => out.push(Beat::TurnEnded),
            Some("compact_boundary") => out.push(Beat::Compacted {
                before: line.compact.as_ref().and_then(|compact| compact.before),
                after: line.compact.as_ref().and_then(|compact| compact.after),
            }),
            Some("agents_killed") => out.push(Beat::TasksKilled),
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

fn read_assistant(line: Line, out: &mut Vec<Beat>) {
    let Some(message) = line.message else {
        return;
    };
    // Whether the line holds something the turn can end on: a reply that is
    // only reasoning is followed by more lines of the same reply.
    let mut conclusive = false;
    let at = seconds(line.timestamp.as_deref());
    match message.content {
        Some(Content::Text(text)) => {
            if !text.trim().is_empty() {
                out.push(Beat::Said {
                    text: speech(&text),
                    at,
                });
                conclusive = true;
            }
        }
        Some(Content::Blocks(blocks)) => {
            for block in blocks {
                match block.kind.as_deref() {
                    Some("thinking" | "redacted_thinking") => out.push(Beat::Thinking),
                    Some("text") => {
                        conclusive = true;
                        if let Some(text) = block.text.filter(|text| !text.trim().is_empty()) {
                            out.push(Beat::Said {
                                text: speech(&text),
                                at,
                            });
                        }
                    }
                    Some("tool_use") => {
                        conclusive = true;
                        let name = block.name.unwrap_or_default();
                        let kind = ToolKind::of(&name);
                        out.push(Beat::ToolStarted {
                            id: block.id.unwrap_or_default(),
                            detail: detail(kind, block.input.as_ref()),
                            name,
                            kind,
                        });
                    }
                    _ => {}
                }
            }
        }
        None => {}
    }
    if let Some(usage) = message.usage {
        let context =
            usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
        if context > 0 {
            out.push(Beat::Usage {
                context,
                output: usage.output_tokens,
                window: None,
            });
        }
    }
    let ending = message
        .stop_reason
        .as_deref()
        .is_some_and(|reason| TURN_ENDING.contains(&reason));
    if ending && conclusive {
        out.push(Beat::TurnEnded);
    }
}

fn read_user(line: Line, out: &mut Vec<Beat>) {
    if line.is_compact_summary == Some(true) {
        // The compaction itself is told by its `compact_boundary` line.
        return;
    }
    let origin = line
        .origin
        .as_ref()
        .and_then(|origin| origin.kind.as_deref());
    let launched = line
        .tool_use_result
        .as_ref()
        .and_then(ToolUseResult::launched);
    let mut text: Option<String> = None;
    let mut results = false;
    match line.message.and_then(|message| message.content) {
        Some(Content::Text(body)) => text = Some(body),
        Some(Content::Blocks(blocks)) => {
            for block in blocks {
                match block.kind.as_deref() {
                    Some("tool_result") => {
                        results = true;
                        out.push(read_result(block, launched));
                    }
                    Some("text") if text.is_none() => text = block.text,
                    // An image alone is still something the user sent.
                    Some("image" | "document") if text.is_none() => text = Some(String::new()),
                    _ => {}
                }
            }
        }
        None => {}
    }
    if results {
        return;
    }
    let Some(text) = text else {
        return;
    };
    let text = text.trim_start();
    if origin == Some("task-notification") || text.starts_with("<task-notification>") {
        read_notifications(text, out);
        out.push(Beat::Woken);
    } else if origin.is_some_and(|kind| kind != "human") {
        out.push(Beat::Woken);
    } else if line.is_meta == Some(true) {
        // Text the harness injected: neither the user nor a new turn.
    } else if text.starts_with(INTERRUPTED) {
        out.push(Beat::Interrupted);
    } else if !LOCAL_PREFIXES.iter().any(|prefix| text.starts_with(prefix)) {
        out.push(Beat::Prompt);
    }
}

fn read_result(block: Block, launched: Option<&str>) -> Beat {
    let id = block.tool_use_id.unwrap_or_default();
    if let Some(task) = launched {
        return Beat::Detached {
            id,
            task: task.to_owned(),
        };
    }
    let failed = block.is_error == Some(true);
    let refused = failed
        && block.content.as_deref().is_some_and(|content| {
            // The wording sits at the very start of the output, either as a
            // string or as the text of its first block.
            let head = crate::normalize::clip(content.get(), 160);
            REFUSALS.iter().any(|wording| head.contains(wording))
        });
    Beat::ToolFinished {
        id,
        failed,
        refused,
    }
}

/// Reads every `<task-notification>` of a notice that tells a task's end.
/// Notices without a status are progress events and tell no end.
fn read_notifications(text: &str, out: &mut Vec<Beat>) {
    for notice in text.split("<task-notification>").skip(1) {
        let notice = notice
            .split("</task-notification>")
            .next()
            .unwrap_or(notice);
        let (Some(task), Some(status)) = (tag(notice, "task-id"), tag(notice, "status")) else {
            continue;
        };
        out.push(Beat::TaskEnded {
            task: task.to_owned(),
            tool: tag(notice, "tool-use-id").map(str::to_owned),
            outcome: TaskOutcome::of(status),
        });
    }
}

/// The trimmed text of the first `<name>...</name>` in `text`, if not empty.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    let value = text[start..end].trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
pub(crate) mod fixture {
    //! Synthetic lines with the shape of Claude Code's, for the tests of
    //! this module and of its neighbours.

    use serde_json::{json, Value};

    /// What every conversation line carries besides its message.
    fn framed(kind: &str, sidechain: bool, message: Value) -> Value {
        json!({
            "parentUuid": "p", "isSidechain": sidechain, "type": kind, "uuid": "u",
            "timestamp": "2026-03-01T10:00:00.000Z", "sessionId": "s1", "cwd": "/srv/api",
            "userType": "external", "entrypoint": "cli", "version": "2.1.0",
            "gitBranch": "main", "message": message
        })
    }

    pub(crate) fn prompt(text: &str) -> Value {
        let mut line = framed("user", false, json!({"role": "user", "content": text}));
        line["origin"] = json!({"kind": "human"});
        line["promptSource"] = json!("typed");
        line
    }

    /// One line of a model reply: one block, as Claude Code writes them.
    pub(crate) fn reply(id: &str, block: Value, stop_reason: Option<&str>) -> Value {
        framed(
            "assistant",
            false,
            json!({
                "id": id, "type": "message", "role": "assistant", "model": "model-a",
                "content": [block], "stop_reason": stop_reason, "stop_sequence": null,
                "usage": {
                    "input_tokens": 10, "cache_read_input_tokens": 900,
                    "cache_creation_input_tokens": 90, "output_tokens": 42,
                    "service_tier": "standard"
                }
            }),
        )
    }

    pub(crate) fn thinking() -> Value {
        json!({"type": "thinking", "thinking": "private reasoning", "signature": "x"})
    }

    pub(crate) fn text(text: &str) -> Value {
        json!({"type": "text", "text": text})
    }

    pub(crate) fn tool_use(id: &str, name: &str, input: Value) -> Value {
        json!({"type": "tool_use", "id": id, "name": name, "input": input,
               "caller": {"type": "direct"}})
    }

    /// The user line answering one tool call.
    pub(crate) fn result(id: &str, content: Value, is_error: bool, record: Value) -> Value {
        let mut block = json!({"type": "tool_result", "tool_use_id": id, "content": content});
        if is_error {
            block["is_error"] = json!(true);
        }
        let mut line = framed("user", false, json!({"role": "user", "content": [block]}));
        line["toolUseResult"] = record;
        line["sourceToolAssistantUUID"] = json!("a");
        line
    }

    /// The result of an `Agent` call that launched a background sub-agent.
    pub(crate) fn launched(id: &str, agent: &str) -> Value {
        result(
            id,
            json!([{"type": "text", "text": "Async agent launched successfully."}]),
            false,
            json!({
                "isAsync": true, "status": "async_launched", "agentId": agent,
                "description": "Map the parser", "prompt": "private",
                "outputFile": format!("/tmp/tasks/{agent}.output"), "canReadOutputFile": true
            }),
        )
    }

    /// The notice that a background task ended.
    pub(crate) fn notification(task: &str, tool: &str, status: &str) -> Value {
        let body = format!(
            "<task-notification>\n<task-id>{task}</task-id>\n<tool-use-id>{tool}</tool-use-id>\n\
             <output-file>/tmp/tasks/{task}.output</output-file>\n<status>{status}</status>\n\
             <summary>Agent \"Map the parser\" finished</summary>\n</task-notification>"
        );
        let mut line = framed("user", false, json!({"role": "user", "content": body}));
        line["origin"] = json!({"kind": "task-notification", "producer": "session-task"});
        line["promptSource"] = json!("system");
        line
    }

    pub(crate) fn system(subtype: &str) -> Value {
        json!({
            "parentUuid": "p", "isSidechain": false, "type": "system", "subtype": subtype,
            "timestamp": "2026-03-01T10:00:00.000Z", "uuid": "u", "sessionId": "s1",
            "isMeta": false, "durationMs": 1200, "messageCount": 8
        })
    }

    /// Marks a line as written by a sub-agent.
    pub(crate) fn sidechain(mut line: Value, agent: &str) -> Value {
        line["isSidechain"] = json!(true);
        line["agentId"] = json!(agent);
        line
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use serde_json::json;

    fn beats(line: &Value) -> Vec<Beat> {
        let mut out = Vec::new();
        read_line(line.to_string().as_bytes(), false, &mut out).unwrap();
        out
    }

    fn usage() -> Beat {
        Beat::Usage {
            context: 1000,
            output: 42,
            window: None,
        }
    }

    #[test]
    fn a_typed_message_is_a_prompt() {
        assert_eq!(beats(&prompt("fix the login flow")), [Beat::Prompt]);
        // Older versions wrote no origin.
        let mut old = prompt("fix the login flow");
        old.as_object_mut().unwrap().remove("origin");
        assert_eq!(beats(&old), [Beat::Prompt]);
    }

    #[test]
    fn a_message_with_a_picture_is_a_prompt() {
        let mut line = prompt("");
        line["message"]["content"] = json!([
            {"type": "image", "source": {"type": "base64", "data": "AAAA"}},
            {"type": "text", "text": "what is this"}
        ]);
        assert_eq!(beats(&line), [Beat::Prompt]);
    }

    #[test]
    fn injected_text_and_local_commands_are_not_prompts() {
        let mut meta = prompt("Caveat: the messages below were generated locally");
        meta.as_object_mut().unwrap().remove("origin");
        meta["isMeta"] = json!(true);
        assert_eq!(beats(&meta), []);

        let mut command = prompt("<command-name>/model</command-name>");
        command.as_object_mut().unwrap().remove("origin");
        assert_eq!(beats(&command), []);
        let mut output = prompt("<local-command-stdout>Set model</local-command-stdout>");
        output.as_object_mut().unwrap().remove("origin");
        assert_eq!(beats(&output), []);

        let mut summary = prompt("Summary of the conversation so far");
        summary["isCompactSummary"] = json!(true);
        assert_eq!(beats(&summary), []);
    }

    #[test]
    fn an_interruption_is_told_apart_from_a_prompt() {
        let mut line = prompt("");
        line.as_object_mut().unwrap().remove("origin");
        line["message"]["content"] =
            json!([{"type": "text", "text": "[Request interrupted by user for tool use]"}]);
        assert_eq!(beats(&line), [Beat::Interrupted]);
    }

    #[test]
    fn each_block_of_a_reply_is_its_own_beat() {
        assert_eq!(
            beats(&reply("m1", thinking(), None)),
            [Beat::Thinking, usage()]
        );
        assert_eq!(
            beats(&reply("m1", text("Looking into it."), None)),
            [
                Beat::Said {
                    text: "Looking into it.".into(),
                    at: Some(1_772_359_200),
                },
                usage()
            ]
        );
        // The text is the model's own, whole: its line breaks, its markdown.
        let long = "# Plan\n\n1. Read `main.rs`.\n2. Fix it.\r\n";
        let said = beats(&reply("m1", text(long), None));
        assert_eq!(
            said[0],
            Beat::Said {
                text: "# Plan\n\n1. Read `main.rs`.\n2. Fix it.".into(),
                at: Some(1_772_359_200),
            }
        );
        // An empty text block is no speech.
        assert_eq!(beats(&reply("m1", text("  \n"), None)), [usage()]);
        let call = tool_use(
            "t1",
            "Edit",
            json!({"file_path": "/srv/api/src/main.rs", "old_string": "a", "new_string": "b"}),
        );
        assert_eq!(
            beats(&reply("m1", call, Some("tool_use"))),
            [
                Beat::ToolStarted {
                    id: "t1".into(),
                    name: "Edit".into(),
                    kind: ToolKind::Edit,
                    detail: "main.rs".into()
                },
                usage()
            ]
        );
    }

    #[test]
    fn a_reply_that_stops_for_the_user_ends_the_turn() {
        assert_eq!(
            beats(&reply("m1", text("Done."), Some("end_turn"))),
            [
                Beat::Said {
                    text: "Done.".into(),
                    at: Some(1_772_359_200),
                },
                usage(),
                Beat::TurnEnded
            ]
        );
        // A reply written at once repeats its stop reason on every line; the
        // reasoning line must not end the turn before the text arrives.
        assert_eq!(
            beats(&reply("m1", thinking(), Some("end_turn"))),
            [Beat::Thinking, usage()]
        );
        assert_eq!(
            beats(&reply(
                "m1",
                tool_use("t1", "Bash", json!({})),
                Some("tool_use")
            ))
            .last(),
            Some(&usage())
        );
    }

    #[test]
    fn a_turn_duration_line_ends_the_turn() {
        assert_eq!(beats(&system("turn_duration")), [Beat::TurnEnded]);
        assert_eq!(beats(&system("stop_hook_summary")), []);
        assert_eq!(beats(&system("agents_killed")), [Beat::TasksKilled]);
    }

    #[test]
    fn a_tool_result_finishes_its_call() {
        let ok = result(
            "t1",
            json!("output"),
            false,
            json!({"stdout": "output", "stderr": "", "interrupted": false}),
        );
        assert_eq!(
            beats(&ok),
            [Beat::ToolFinished {
                id: "t1".into(),
                failed: false,
                refused: false
            }]
        );
        let failed = result("t1", json!("Exit code 1\nboom"), true, json!("Error: boom"));
        assert_eq!(
            beats(&failed),
            [Beat::ToolFinished {
                id: "t1".into(),
                failed: true,
                refused: false
            }]
        );
    }

    #[test]
    fn a_refused_call_is_marked() {
        let by_user = result(
            "t1",
            json!(
                "The user doesn't want to proceed with this tool use. The tool use was rejected."
            ),
            true,
            json!("User rejected tool use"),
        );
        let by_rule = result(
            "t2",
            json!([{"type": "text", "text": "Permission for this action was denied by a rule."}]),
            true,
            json!("Error"),
        );
        for line in [by_user, by_rule] {
            assert!(matches!(
                beats(&line)[..],
                [Beat::ToolFinished {
                    failed: true,
                    refused: true,
                    ..
                }]
            ));
        }
    }

    #[test]
    fn a_launched_sub_agent_detaches_and_its_notice_ends_it() {
        assert_eq!(
            beats(&launched("t9", "a0b1c2d3e4f5a6b7c")),
            [Beat::Detached {
                id: "t9".into(),
                task: "a0b1c2d3e4f5a6b7c".into()
            }]
        );
        assert_eq!(
            beats(&notification("a0b1c2d3e4f5a6b7c", "t9", "completed")),
            [
                Beat::TaskEnded {
                    task: "a0b1c2d3e4f5a6b7c".into(),
                    tool: Some("t9".into()),
                    outcome: TaskOutcome::Completed
                },
                Beat::Woken
            ]
        );
    }

    #[test]
    fn a_sub_agent_that_ran_inside_its_call_finishes_with_it() {
        let line = result(
            "t9",
            json!([{"type": "text", "text": "report"}]),
            false,
            json!({"status": "completed", "agentId": "a0b1c2d3e4f5a6b7c", "totalToolUseCount": 4}),
        );
        assert_eq!(
            beats(&line),
            [Beat::ToolFinished {
                id: "t9".into(),
                failed: false,
                refused: false
            }]
        );
    }

    #[test]
    fn one_notice_may_tell_several_ends_and_events_tell_none() {
        let mut line = notification("b12345678", "t1", "killed");
        let body = format!(
            "{}\n<task-notification>\n<task-id>a0b1c2d3e4f5a6b7c</task-id>\n\
             <status>failed</status>\n</task-notification>\n\
             <task-notification>\n<task-id>w1</task-id>\n<summary>tick</summary>\n\
             <event>line</event>\n</task-notification>",
            line["message"]["content"].as_str().unwrap()
        );
        line["message"]["content"] = json!(body);
        assert_eq!(
            beats(&line),
            [
                Beat::TaskEnded {
                    task: "b12345678".into(),
                    tool: Some("t1".into()),
                    outcome: TaskOutcome::Stopped
                },
                Beat::TaskEnded {
                    task: "a0b1c2d3e4f5a6b7c".into(),
                    tool: None,
                    outcome: TaskOutcome::Failed
                },
                Beat::Woken
            ]
        );
    }

    #[test]
    fn a_message_from_another_agent_wakes_the_session() {
        let mut line = prompt("status?");
        line["origin"] = json!({"kind": "peer"});
        line["isMeta"] = json!(true);
        assert_eq!(beats(&line), [Beat::Woken]);
    }

    #[test]
    fn a_compaction_reports_its_token_numbers() {
        let mut line = system("compact_boundary");
        line["compactMetadata"] =
            json!({"trigger": "auto", "preTokens": 180000, "postTokens": 21000});
        assert_eq!(
            beats(&line),
            [Beat::Compacted {
                before: Some(180_000),
                after: Some(21_000)
            }]
        );
        assert_eq!(
            beats(&system("compact_boundary")),
            [Beat::Compacted {
                before: None,
                after: None
            }]
        );
    }

    #[test]
    fn sidechain_lines_belong_to_the_sub_agent_only() {
        let line = sidechain(reply("m1", text("sub-agent speaking"), None), "a1");
        let raw = line.to_string();
        let mut own = Vec::new();
        read_line(raw.as_bytes(), false, &mut own).unwrap();
        assert_eq!(own, []);
        let mut sub = Vec::new();
        read_line(raw.as_bytes(), true, &mut sub).unwrap();
        assert_eq!(
            sub,
            [
                Beat::Said {
                    text: "sub-agent speaking".into(),
                    at: Some(1_772_359_200),
                },
                usage()
            ]
        );
    }

    #[test]
    fn other_lines_and_odd_shapes_yield_nothing_and_never_fail() {
        for line in [
            json!({"type": "file-history-snapshot", "messageId": "m", "snapshot": {}}),
            json!({"type": "attachment", "attachment": {"type": "hook_success"}}),
            json!({"type": "permission-mode", "permissionMode": "default"}),
            json!({"type": "something-from-the-future", "payload": [1, 2, 3]}),
            json!({"no_type_at_all": true}),
            json!({"type": "user", "message": 42}),
            json!({"type": "assistant", "message": {"content": {"odd": true}, "usage": "none"}}),
            json!({"type": 7, "isSidechain": "yes", "toolUseResult": [1, 2]}),
            json!({"type": "user", "toolUseResult": "plain", "message": {"content": []}}),
        ] {
            assert_eq!(beats(&line), [], "{line}");
        }
        let mut out = Vec::new();
        assert!(read_line(b"not json", false, &mut out).is_err());
        assert!(read_line(b"{\"type\":\"user\",\"mess", false, &mut out).is_err());
        // Valid JSON of another shape is read as a line that says nothing.
        assert!(read_line(b"[1, 2]", false, &mut out).is_ok());
        assert!(out.is_empty());
    }
}

//! Codex, spoken to both ways.
//!
//! Codex has an application server on its standard streams, the one its own
//! applications use (checked against Codex 0.160.1, where it is marked
//! experimental; `codex app-server generate-ts` prints the protocol):
//!
//! ```text
//! codex app-server
//! ```
//!
//! Each line either way is one JSON value in the shape of JSON-RPC, without
//! the `jsonrpc` member: a request has `id` and `method`, an answer `id` and
//! `result` or `error`, a notification `method` only. The client writes:
//!
//! * `initialize`, the `initialized` notification, then `thread/start` or
//!   `thread/resume` (an id any Codex gave a conversation, the interactive
//!   one's included) with the mode and the model. The three are written at
//!   once: the server answers them in order;
//! * `turn/start` with the prompt, once the thread's id is known;
//! * `turn/steer` to say more while it works, `turn/interrupt` to stop it;
//! * the answer to what the server asks.
//!
//! The server prints notifications (`item/started`, `item/completed`,
//! `item/agentMessage/delta`, `turn/completed`…) and asks with requests of
//! its own, each of which waits for an answer:
//!
//! * `item/commandExecution/requestApproval`, answered `{"decision":…}` with
//!   one of the decisions it lists as available (`accept`,
//!   `acceptForSession`, an amendment of its rules, `decline`, `cancel`);
//! * `item/fileChange/requestApproval`, answered the same way;
//! * `item/permissions/requestApproval`: more of the file system or the
//!   network, granted for the turn or the session, or not at all;
//! * `item/tool/requestUserInput`: questions with options;
//! * `mcpServer/elicitation/request`: a tool server asks something.
//!
//! `serverRequest/resolved` says a request no longer waits. The server never
//! ends by itself: after `turn/completed` its input is closed.
//!
//! A mode is a pair the server takes apart again: what it asks before doing
//! (`approvalPolicy`) and what the sandbox lets it do (`sandbox`). Neither
//! can be changed while a turn runs.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde_json::{json, Map, Value};

use crate::agents::{strings, Action, AgentSession, Outgoing, Step, TurnSpec};
use crate::claude::{AgentCommand, AgentMode, AgentModel, AgentOptions};
use crate::error::TurnError;
use crate::events::{
    blocks, cap_detail, clip, content_text, diff_lines, first_line, input_detail, input_summary,
    last_line, text_detail, text_of, written_detail, Answer, ChoiceOption, ChoiceQuestion,
    PermissionChoice, PermissionEffect, QuestionKind, TurnEvent,
};
use crate::tag::{id_tag, tag_id};

/// The ids of this client's own requests. They are fixed, so that a turn
/// read again knows which answer is which.
const ID_HELLO: &str = "leon-init";
const ID_THREAD: &str = "leon-thread";
const ID_TURN: &str = "leon-turn";
/// A request a process that was never told the prompt writes in place of
/// `turn/start`: its answer means nobody had started the turn.
const ID_LOST: &str = "leon-lost";

const TAG_HELLO: &str = "h.init";
const TAG_READY: &str = "h.ready";
const TAG_THREAD: &str = "h.thread";
const TAG_TURN: &str = "h.turn";

/// How many characters of a change are shown to whoever must approve it.
const DETAIL_CHARS: usize = 20_000;
/// How many lines that were not the server's are kept to explain a failure.
const NOISE_LINES: usize = 6;

/// What is said when the turn was never started.
const NEVER_STARTED: &str =
    "The prompt did not reach Codex before the connection was lost. Send it again.";

/// Notifications nobody here reads: left out, they do not use up the output
/// the host keeps of a turn.
const UNREAD: &[&str] = &[
    "hook/started",
    "hook/completed",
    "mcpServer/startupStatus/updated",
    "account/updated",
    "account/rateLimits/updated",
    "remoteControl/status/changed",
    "thread/status/changed",
    "turn/diff/updated",
    "item/commandExecution/outputDelta",
    "item/fileChange/outputDelta",
    "item/fileChange/patchUpdated",
    "item/reasoning/summaryTextDelta",
    "item/reasoning/summaryPartAdded",
    "item/reasoning/textDelta",
    "item/mcpToolCall/progress",
    "thread/tokenUsage/updated",
    "skills/changed",
    "app/list/updated",
];

/// The command line of a turn. What the turn is about travels in requests.
pub fn argv(turn: &TurnSpec<'_>) -> Vec<String> {
    strings(&[turn.program, "app-server"])
}

/// A session for one turn.
pub fn session(unique: u64) -> Box<dyn AgentSession> {
    Box::new(CodexSession {
        unique,
        prompt: None,
        effort: None,
        thread: None,
        turn: None,
        started: false,
        finished: false,
        idle: false,
        stop_wanted: false,
        say_later: Vec::new(),
        asked: BTreeMap::new(),
        announced: HashSet::new(),
        changes: HashMap::new(),
        mode: None,
        last_error: None,
        noise: VecDeque::new(),
    })
}

fn request(id: &str, method: &str, params: Value) -> String {
    json!({ "id": id, "method": method, "params": params }).to_string()
}

/// The opening request, as a line.
pub fn hello() -> String {
    request(
        ID_HELLO,
        "initialize",
        json!({
            "clientInfo": {
                "name": "leon-mobile",
                "title": "Leon",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {
                "experimentalApi": true,
                "requestAttestation": false,
                "optOutNotificationMethods": UNREAD,
            },
        }),
    )
}

fn text_input(text: &str) -> Value {
    json!([{ "type": "text", "text": text, "text_elements": [] }])
}

/// What a mode asks for: when it asks, and what the sandbox allows.
fn mode_parts(id: &str) -> Option<(&'static str, &'static str)> {
    MODES
        .iter()
        .find(|mode| mode.0 == id)
        .map(|mode| (mode.1, mode.2))
}

/// The mode a thread is in, when it is one of [`MODES`].
fn mode_named(approval: &Value, sandbox: &Value) -> Option<&'static str> {
    let approval = approval.as_str()?;
    // A thread's answer names the sandbox as a policy (`readOnly`), a
    // request as a mode (`read-only`).
    let sandbox = match sandbox["type"].as_str().or(sandbox.as_str())? {
        "readOnly" | "read-only" => "read-only",
        "workspaceWrite" | "workspace-write" => "workspace-write",
        "dangerFullAccess" | "danger-full-access" => "danger-full-access",
        _ => return None,
    };
    MODES
        .iter()
        .find(|mode| mode.1 == approval && mode.2 == sandbox)
        .map(|mode| mode.0)
}

/// The modes offered: id, `approvalPolicy`, `sandbox`, name, meaning.
const MODES: &[(&str, &str, &str, &str, &str)] = &[
    (
        "read-only",
        "on-request",
        "read-only",
        "Read only",
        "It reads. It asks before it changes a file or runs a command that needs more.",
    ),
    (
        "auto",
        "on-request",
        "workspace-write",
        "Auto",
        "It edits files and runs commands in the folder it works in, and asks to go beyond it.",
    ),
    (
        "untrusted",
        "untrusted",
        "workspace-write",
        "Ask before commands",
        "It asks before every command it does not know to be harmless.",
    ),
    (
        "full-access",
        "never",
        "danger-full-access",
        "Full access",
        "It does anything you could do on that computer, without a sandbox and without asking.",
    ),
];

/// What the server asked and has no answer to yet.
#[derive(Debug)]
struct Asked {
    /// The request's id as the server wrote it (a number, or text).
    id: Value,
    sort: Sort,
}

#[derive(Debug)]
enum Sort {
    /// A command or a change of files: one of these decisions goes back.
    Decision(Vec<Value>),
    /// More permissions: what was asked for, to grant as asked.
    Grant(Value),
    /// Questions with options.
    Input,
    /// A tool server's question.
    Elicitation,
}

/// What the server does itself when asked by name, where any other prompt
/// is a turn of the model's.
#[derive(Debug, PartialEq, Eq)]
enum Native<'a> {
    /// `/compact`: the conversation so far becomes a summary of itself.
    Compact,
    /// `/review`, then what to look at: nothing is what is not committed.
    Review(&'a str),
}

/// The server's own doing a prompt asks for, when it is one.
fn native(prompt: &str) -> Option<Native<'_>> {
    let prompt = prompt.trim();
    let (name, rest) = prompt
        .split_once(char::is_whitespace)
        .unwrap_or((prompt, ""));
    match name {
        "/compact" if rest.trim().is_empty() => Some(Native::Compact),
        "/review" => Some(Native::Review(rest.trim())),
        _ => None,
    }
}

/// The server's own doings, as [`AgentOptions::commands`] lists them.
fn natives() -> Vec<AgentCommand> {
    vec![
        AgentCommand::slash(
            "compact",
            Some("Make the conversation so far a summary of itself."),
            None,
        ),
        AgentCommand::slash(
            "review",
            Some("Review what is not committed, or what you say after it."),
            Some("[what to look at]"),
        ),
    ]
}

struct CodexSession {
    /// Makes the ids of this process's own requests unlike any other's.
    unique: u64,
    /// What the person wrote; nothing in a process that found the turn
    /// running.
    prompt: Option<String>,
    effort: Option<String>,
    thread: Option<String>,
    /// The turn in flight, once the server named it.
    turn: Option<String>,
    started: bool,
    /// The turn ended (or never began): nothing more can be said in it.
    finished: bool,
    idle: bool,
    /// An interrupt was asked for before the turn had an id.
    stop_wanted: bool,
    /// What was said before the turn had an id.
    say_later: Vec<String>,
    /// Open questions, by the text of the request's id.
    asked: BTreeMap<String, Asked>,
    /// Items shown as a tool in use.
    announced: HashSet<String>,
    /// The changes of files seen, by item: one line, and the whole.
    changes: HashMap<String, (String, String)>,
    mode: Option<&'static str>,
    last_error: Option<String>,
    noise: VecDeque<String>,
}

impl CodexSession {
    fn next_id(&mut self, kind: &str) -> String {
        self.unique += 1;
        format!("leon-{kind}-{}", self.unique)
    }

    fn resolve(&mut self, id: &str, step: &mut Step) {
        if self.asked.remove(id).is_some() {
            step.events
                .push(TurnEvent::QuestionResolved { id: id.to_owned() });
        }
    }

    fn resolve_all(&mut self, step: &mut Step) {
        let open: Vec<String> = self.asked.keys().cloned().collect();
        for id in open {
            self.resolve(&id, step);
        }
    }

    fn start(&mut self, thread: &Value, model: Option<&str>, step: &mut Step) {
        let Some(id) = thread["id"].as_str() else {
            return;
        };
        self.thread = Some(id.to_owned());
        if !self.started {
            self.started = true;
            step.events.push(TurnEvent::Started {
                session_id: id.to_owned(),
                model: model.or(thread["model"].as_str()).map(str::to_owned),
            });
        }
    }

    fn mode_is(&mut self, approval: &Value, sandbox: &Value, step: &mut Step) {
        let Some(mode) = mode_named(approval, sandbox) else {
            return;
        };
        if self.mode != Some(mode) {
            self.mode = Some(mode);
            step.events.push(TurnEvent::ModeChanged {
                mode: mode.to_owned(),
            });
        }
    }

    /// The turn ended without the server saying how.
    fn fail(&mut self, error: String, step: &mut Step) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.idle = true;
        self.resolve_all(step);
        step.events.push(TurnEvent::Finished {
            ok: false,
            error: Some(error),
            cost_usd: None,
            duration_ms: None,
            denied_tools: Vec::new(),
            interrupted: false,
        });
    }

    /// The turn has an id: what waited for it is written.
    fn turn_is(&mut self, id: &str, step: &mut Step) {
        if id.is_empty() || self.turn.as_deref() == Some(id) {
            return;
        }
        self.turn = Some(id.to_owned());
        if self.stop_wanted {
            self.stop_wanted = false;
            if let Some(outgoing) = self.interrupt() {
                step.send.push(outgoing);
            }
        }
        for text in std::mem::take(&mut self.say_later) {
            if let Some(outgoing) = self.steer(&text) {
                step.send.push(outgoing);
            }
        }
    }

    fn interrupt(&mut self) -> Option<Outgoing> {
        let (thread, turn) = (self.thread.clone()?, self.turn.clone()?);
        let id = self.next_id("stop");
        Some(Outgoing {
            tag: format!("i.{id}"),
            line: request(
                &id,
                "turn/interrupt",
                json!({ "threadId": thread, "turnId": turn }),
            ),
        })
    }

    fn steer(&mut self, text: &str) -> Option<Outgoing> {
        let (thread, turn) = (self.thread.clone()?, self.turn.clone()?);
        let id = self.next_id("say");
        Some(Outgoing {
            tag: format!("u.{id}"),
            line: request(
                &id,
                "turn/steer",
                json!({ "threadId": thread, "expectedTurnId": turn, "input": text_input(text) }),
            ),
        })
    }

    fn answered(&mut self, value: &Value, step: &mut Step) {
        let id = value["id"].as_str().unwrap_or_default();
        let failed = value["error"]["message"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| (!value["error"].is_null()).then(|| value["error"].to_string()));
        match id {
            ID_HELLO => {
                if let Some(error) = failed {
                    self.fail(error, step);
                }
            }
            ID_THREAD => {
                if let Some(error) = failed {
                    self.fail(error, step);
                    return;
                }
                let result = &value["result"];
                self.start(&result["thread"], result["model"].as_str(), step);
                self.mode_is(&result["approvalPolicy"], &result["sandbox"], step);
                let Some(thread) = self.thread.clone() else {
                    self.fail("Codex did not name the conversation".to_owned(), step);
                    return;
                };
                let line = match &self.prompt {
                    Some(prompt) => match native(prompt) {
                        Some(Native::Compact) => request(
                            ID_TURN,
                            "thread/compact/start",
                            json!({ "threadId": thread }),
                        ),
                        Some(Native::Review(about)) => {
                            let target = if about.is_empty() {
                                json!({ "type": "uncommittedChanges" })
                            } else {
                                json!({ "type": "custom", "instructions": about })
                            };
                            request(
                                ID_TURN,
                                "review/start",
                                json!({ "threadId": thread, "target": target }),
                            )
                        }
                        None => {
                            let mut params =
                                json!({ "threadId": thread, "input": text_input(prompt) });
                            if let Some(effort) = &self.effort {
                                params["effort"] = json!(effort);
                            }
                            request(ID_TURN, "turn/start", params)
                        }
                    },
                    // This process found the turn running and was never told
                    // the prompt. Whoever started it wrote `turn/start`
                    // under this tag: if that arrived, this line is never
                    // written; if it did not, the answer to this one says so.
                    None => request(ID_LOST, "thread/loaded/list", json!({})),
                };
                step.send.push(Outgoing {
                    tag: TAG_TURN.to_owned(),
                    line,
                });
            }
            ID_TURN => match failed {
                Some(error) => self.fail(error, step),
                None => {
                    let id = text_of(&value["result"]["turn"]["id"]);
                    self.turn_is(&id, step);
                }
            },
            ID_LOST if self.turn.is_none() => self.fail(NEVER_STARTED.to_owned(), step),
            // The answers to steering and interrupting: nothing to do. One
            // refused because the turn had just ended is no failure.
            _ => {}
        }
    }

    fn ask(&mut self, value: &Value, step: &mut Step) {
        let id = id_text(&value["id"]);
        let params = &value["params"];
        let (sort, kind) = match value["method"].as_str().unwrap_or_default() {
            "item/commandExecution/requestApproval" => {
                let decisions = decisions(params, COMMAND_DECISIONS);
                let stdin = params["kind"] == "writeStdin";
                let kind = QuestionKind::Permission {
                    tool: if stdin { "terminal input" } else { "shell" }.to_owned(),
                    summary: command_line(params),
                    detail: detail_of(params),
                    reason: params["reason"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| {
                            params["networkApprovalContext"]["host"]
                                .as_str()
                                .map(|host| format!("It needs the network: {host}"))
                        })
                        .filter(|text| !text.is_empty()),
                    choices: decision_choices(&decisions),
                };
                (Sort::Decision(decisions), kind)
            }
            "item/fileChange/requestApproval" => {
                let decisions = decisions(params, FILE_DECISIONS);
                let (summary, detail) = self
                    .changes
                    .get(params["itemId"].as_str().unwrap_or_default())
                    .cloned()
                    .unwrap_or_default();
                let kind = QuestionKind::Permission {
                    tool: "edit".to_owned(),
                    summary,
                    detail,
                    reason: params["reason"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| {
                            params["grantRoot"].as_str().map(|root| {
                                format!("It asks to write under {root} for the rest of the session")
                            })
                        })
                        .filter(|text| !text.is_empty()),
                    choices: decision_choices(&decisions),
                };
                (Sort::Decision(decisions), kind)
            }
            "item/permissions/requestApproval" => {
                let wanted = &params["permissions"];
                let kind = QuestionKind::Permission {
                    tool: "permissions".to_owned(),
                    summary: permissions_line(wanted),
                    detail: serde_json::to_string_pretty(wanted).unwrap_or_default(),
                    reason: params["reason"]
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned),
                    choices: vec![
                        choice(
                            GRANT_TURN,
                            PermissionEffect::AllowOnce,
                            "Allow for this turn",
                            None,
                        ),
                        choice(
                            GRANT_SESSION,
                            PermissionEffect::AllowAlways,
                            "Allow for this session",
                            Some("this session"),
                        ),
                        choice(CHOICE_DENY, PermissionEffect::Deny, "Deny", None),
                    ],
                };
                (Sort::Grant(wanted.clone()), kind)
            }
            "item/tool/requestUserInput" => {
                let questions = blocks(&params["questions"])
                    .iter()
                    .map(input_question)
                    .collect();
                (Sort::Input, QuestionKind::Choices { questions })
            }
            "mcpServer/elicitation/request" => {
                let server = text_of(&params["serverName"]);
                let url = params["url"].as_str();
                // A form with fields cannot be filled in from here; one
                // without fields, or a page to visit, can be agreed to.
                let fields = params["requestedSchema"]["properties"]
                    .as_object()
                    .is_some_and(|fields| !fields.is_empty());
                let mut choices = Vec::new();
                if !fields {
                    let label = if url.is_some() { "Done" } else { "Accept" };
                    choices.push(choice(
                        ELICIT_ACCEPT,
                        PermissionEffect::AllowOnce,
                        label,
                        None,
                    ));
                }
                choices.push(choice(CHOICE_DENY, PermissionEffect::Deny, "Decline", None));
                let kind = QuestionKind::Permission {
                    tool: format!("mcp:{server}"),
                    summary: first_line(&text_of(&params["message"])),
                    detail: match url {
                        Some(url) => url.to_owned(),
                        None => serde_json::to_string_pretty(&params["requestedSchema"])
                            .unwrap_or_default(),
                    },
                    reason: fields.then(|| {
                        "It asks for details that can only be filled in on that computer."
                            .to_owned()
                    }),
                    choices,
                };
                (Sort::Elicitation, kind)
            }
            // Tools and tokens of a client this is not: said at once, so
            // the server does not wait for nobody.
            _ => {
                step.send.push(Outgoing {
                    tag: id_tag('r', &id),
                    line: json!({
                        "id": value["id"],
                        "error": { "code": -32601, "message": "not supported by this client" },
                    })
                    .to_string(),
                });
                return;
            }
        };
        // Asked twice (a replay after a gap): the later asking stands.
        self.asked.insert(
            id.clone(),
            Asked {
                id: value["id"].clone(),
                sort,
            },
        );
        step.events.push(TurnEvent::Question { id, kind });
    }

    fn answer(&self, id: &str, answer: Answer) -> Result<Value, TurnError> {
        let asked = self.asked.get(id).ok_or(TurnError::UnknownQuestion)?;
        let result = match (&asked.sort, answer) {
            (Sort::Decision(decisions), Answer::Permission { choice_id, .. }) => {
                let decision = match choice_id.strip_prefix(CHOICE_DECISION) {
                    Some(at) => at
                        .parse::<usize>()
                        .ok()
                        .and_then(|at| decisions.get(at))
                        .ok_or(TurnError::BadAnswer)?
                        .clone(),
                    None if choice_id == CHOICE_DENY => refusal(decisions),
                    None => return Err(TurnError::BadAnswer),
                };
                json!({ "decision": decision })
            }
            (Sort::Grant(wanted), Answer::Permission { choice_id, .. }) => {
                let granted = |scope: &str| {
                    let permissions: Map<String, Value> = wanted
                        .as_object()
                        .into_iter()
                        .flatten()
                        .filter(|(_, value)| !value.is_null())
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect();
                    json!({ "permissions": permissions, "scope": scope })
                };
                match choice_id.as_str() {
                    GRANT_TURN => granted("turn"),
                    GRANT_SESSION => granted("session"),
                    CHOICE_DENY => json!({ "permissions": {}, "scope": "turn" }),
                    _ => return Err(TurnError::BadAnswer),
                }
            }
            (Sort::Input, Answer::Choices { answers }) => {
                let mut said = Map::new();
                for answer in answers {
                    let mut parts = answer.labels;
                    parts.extend(
                        answer
                            .other
                            .map(|text| text.trim().to_owned())
                            .filter(|text| !text.is_empty()),
                    );
                    said.insert(answer.question_id, json!({ "answers": parts }));
                }
                json!({ "answers": said })
            }
            // Refusing questions is answering none of them.
            (Sort::Input, Answer::Permission { choice_id, .. }) if choice_id == CHOICE_DENY => {
                json!({ "answers": {} })
            }
            (Sort::Elicitation, Answer::Permission { choice_id, .. }) => match choice_id.as_str() {
                ELICIT_ACCEPT => json!({ "action": "accept", "content": {}, "_meta": null }),
                CHOICE_DENY => json!({ "action": "decline", "content": null, "_meta": null }),
                _ => return Err(TurnError::BadAnswer),
            },
            _ => return Err(TurnError::BadAnswer),
        };
        Ok(json!({ "id": asked.id, "result": result }))
    }

    fn item(&mut self, item: &Value, completed: bool, step: &mut Step) {
        let id = text_of(&item["id"]);
        let failed = matches!(item["status"].as_str(), Some("failed" | "declined"));
        let declined = item["status"] == "declined";
        let whole_use;
        let whole_result;
        let (name, summary, ok, result) = match item["type"].as_str() {
            Some("agentMessage" | "plan") => {
                if let (true, Some(text)) = (completed, item["text"].as_str()) {
                    if !text.is_empty() {
                        step.events.push(TurnEvent::Text {
                            text: text.to_owned(),
                        });
                    }
                }
                return;
            }
            Some("commandExecution") => {
                whole_use = typed_command(item).to_owned();
                whole_result = text_of(&item["aggregatedOutput"]);
                (
                    "shell".to_owned(),
                    command_line(item),
                    !failed && item["exitCode"].as_i64().unwrap_or(0) == 0,
                    last_line(&whole_result),
                )
            }
            Some("fileChange") => {
                let (line, whole) = changes_of(&item["changes"]);
                self.changes.insert(id.clone(), (line.clone(), whole));
                whole_use = changes_detail(&item["changes"]);
                whole_result = String::new();
                ("edit".to_owned(), line, !failed, String::new())
            }
            Some("mcpToolCall") => {
                let summary = input_summary(&item["arguments"]);
                whole_use = input_detail(&item["arguments"], &summary);
                whole_result = match item["error"]["message"].as_str() {
                    Some(error) => error.to_owned(),
                    None => content_text(&item["result"]["content"]),
                };
                (
                    format!("{}.{}", text_of(&item["server"]), text_of(&item["tool"])),
                    summary,
                    !failed,
                    first_line(&text_of(&item["error"]["message"])),
                )
            }
            Some("dynamicToolCall") => {
                let summary = input_summary(&item["arguments"]);
                whole_use = input_detail(&item["arguments"], &summary);
                whole_result = content_text(&item["contentItems"]);
                (
                    text_of(&item["tool"]),
                    summary,
                    !failed && item["success"].as_bool() != Some(false),
                    String::new(),
                )
            }
            Some("webSearch") => {
                whole_use = text_of(&item["query"]);
                whole_result = String::new();
                (
                    "web_search".to_owned(),
                    first_line(&whole_use),
                    true,
                    String::new(),
                )
            }
            // Its own message echoed, its reasoning (which arrives empty
            // unless that computer asks for summaries), and the rest.
            _ => return,
        };
        if self.announced.insert(id.clone()) {
            step.events.push(TurnEvent::ToolUse {
                id: id.clone(),
                name,
                detail: text_detail(&whole_use, &summary),
                summary,
            });
        }
        if completed {
            self.announced.remove(&id);
            step.events.push(TurnEvent::ToolResult {
                id,
                ok,
                detail: text_detail(&whole_result, &result),
                summary: if declined && result.is_empty() {
                    "declined".to_owned()
                } else {
                    result
                },
            });
        }
    }

    fn told(&mut self, value: &Value, step: &mut Step) {
        let params = &value["params"];
        match value["method"].as_str().unwrap_or_default() {
            "thread/started" => self.start(&params["thread"], None, step),
            "turn/started" => {
                let id = text_of(&params["turn"]["id"]);
                self.turn_is(&id, step);
            }
            "item/started" => self.item(&params["item"], false, step),
            "item/completed" => self.item(&params["item"], true, step),
            "item/agentMessage/delta" | "item/plan/delta" => {
                if let Some(text) = params["delta"].as_str().filter(|text| !text.is_empty()) {
                    step.events.push(TurnEvent::TextDelta {
                        text: text.to_owned(),
                    });
                }
            }
            "serverRequest/resolved" => {
                let id = id_text(&params["requestId"]);
                self.resolve(&id, step);
            }
            "thread/settings/updated" => {
                let settings = &params["threadSettings"];
                self.mode_is(
                    &settings["approvalPolicy"],
                    &settings["sandboxPolicy"],
                    step,
                );
            }
            "error" => {
                if params["willRetry"].as_bool() != Some(true) {
                    self.last_error = params["error"]["message"].as_str().map(str::to_owned);
                }
            }
            "turn/completed" => {
                if self.finished {
                    return;
                }
                let turn = &params["turn"];
                self.resolve_all(step);
                // What an interrupted turn was doing is never said to end.
                let mut running: Vec<String> = self.announced.drain().collect();
                running.sort();
                for id in running {
                    step.events.push(TurnEvent::ToolResult {
                        id,
                        ok: false,
                        summary: "stopped".to_owned(),
                        detail: String::new(),
                    });
                }
                self.finished = true;
                self.idle = true;
                let status = turn["status"].as_str().unwrap_or("completed");
                let ok = status == "completed";
                let interrupted = status == "interrupted";
                let error = (!ok && !interrupted).then(|| {
                    turn["error"]["message"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| self.last_error.take())
                        .unwrap_or_else(|| "the agent reported an error".to_owned())
                });
                step.events.push(TurnEvent::Finished {
                    ok,
                    error,
                    cost_usd: None,
                    duration_ms: turn["durationMs"].as_u64(),
                    denied_tools: Vec::new(),
                    interrupted,
                });
            }
            _ => {}
        }
    }
}

const CHOICE_DECISION: &str = "d.";
const CHOICE_DENY: &str = "deny";
const GRANT_TURN: &str = "turn";
const GRANT_SESSION: &str = "session";
const ELICIT_ACCEPT: &str = "accept";

/// What a command may be answered with when the server does not list it.
const COMMAND_DECISIONS: &[&str] = &["accept", "acceptForSession", "decline", "cancel"];
/// What a change of files may be answered with; the server never lists it.
const FILE_DECISIONS: &[&str] = &["accept", "acceptForSession", "decline", "cancel"];

/// The id of a request as text: the same for the number and for its digits.
fn id_text(id: &Value) -> String {
    match id {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn choice(
    id: &str,
    effect: PermissionEffect,
    label: &str,
    scope: Option<&str>,
) -> PermissionChoice {
    PermissionChoice {
        id: id.to_owned(),
        effect,
        label: label.to_owned(),
        scope: scope.map(str::to_owned),
        takes_reason: false,
    }
}

/// The decisions a request may be answered with: the ones it lists, or the
/// protocol's own for a request that lists none.
fn decisions(params: &Value, all: &[&str]) -> Vec<Value> {
    match params["availableDecisions"].as_array() {
        Some(listed) if !listed.is_empty() => listed.clone(),
        _ => all.iter().map(|decision| json!(decision)).collect(),
    }
}

/// The decision that refuses without ending the turn, when there is one.
fn refusal(decisions: &[Value]) -> Value {
    for wanted in ["decline", "cancel"] {
        if decisions.iter().any(|decision| decision == wanted) {
            return json!(wanted);
        }
    }
    json!("decline")
}

fn decision_choices(decisions: &[Value]) -> Vec<PermissionChoice> {
    decisions
        .iter()
        .enumerate()
        .filter_map(|(at, decision)| {
            let id = format!("{CHOICE_DECISION}{at}");
            if let Some(name) = decision.as_str() {
                return Some(match name {
                    "accept" => choice(&id, PermissionEffect::AllowOnce, "Allow once", None),
                    "acceptForSession" => choice(
                        &id,
                        PermissionEffect::AllowAlways,
                        "Allow for this session",
                        Some("this session"),
                    ),
                    "decline" => choice(&id, PermissionEffect::Deny, "Deny", None),
                    "cancel" => {
                        choice(&id, PermissionEffect::Deny, "Deny, and stop the turn", None)
                    }
                    _ => return None,
                });
            }
            let prefix = &decision["acceptWithExecpolicyAmendment"]["execpolicy_amendment"];
            if let Some(words) = prefix.as_array() {
                let words: Vec<String> = words.iter().map(text_of).collect();
                return Some(choice(
                    &id,
                    PermissionEffect::AllowAlways,
                    &format!("Always allow commands starting with {}", words.join(" ")),
                    Some("every such command, on that computer"),
                ));
            }
            let rule = &decision["applyNetworkPolicyAmendment"]["network_policy_amendment"];
            let host = rule["host"].as_str()?;
            Some(if rule["action"] == "deny" {
                choice(
                    &id,
                    PermissionEffect::Deny,
                    &format!("Always block {host}"),
                    None,
                )
            } else {
                choice(
                    &id,
                    PermissionEffect::AllowAlways,
                    &format!("Always allow the network for {host}"),
                    Some("that host, on that computer"),
                )
            })
        })
        .collect()
}

/// The command as a person would type it: the server wraps what the agent
/// runs in a shell of its own.
fn command_line(item: &Value) -> String {
    first_line(typed_command(item))
}

/// The whole of that command.
fn typed_command(item: &Value) -> &str {
    let actions = blocks(&item["commandActions"]);
    let typed = match actions {
        [only] => only["command"].as_str(),
        _ => None,
    };
    typed.or(item["command"].as_str()).unwrap_or_default()
}

/// What is asked, without the ids that tell a person nothing.
fn detail_of(params: &Value) -> String {
    let shown: Map<String, Value> = params
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, value)| {
            !value.is_null()
                && !matches!(
                    key.as_str(),
                    "threadId"
                        | "turnId"
                        | "itemId"
                        | "approvalId"
                        | "startedAtMs"
                        | "environmentId"
                        | "availableDecisions"
                        | "commandActions"
                        | "kind"
                )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    serde_json::to_string_pretty(&shown).unwrap_or_default()
}

/// The files a change touches in one line, and the change itself.
fn changes_of(changes: &Value) -> (String, String) {
    let changes = blocks(changes);
    let paths: Vec<&str> = changes
        .iter()
        .filter_map(|change| change["path"].as_str())
        .collect();
    let whole: Vec<String> = changes
        .iter()
        .map(|change| {
            format!(
                "{} {}\n{}",
                change["kind"]["type"].as_str().unwrap_or("change"),
                change["path"].as_str().unwrap_or_default(),
                change["diff"].as_str().unwrap_or_default()
            )
        })
        .collect();
    (
        clip(&paths.join(", "), 200),
        clip(&whole.join("\n"), DETAIL_CHARS),
    )
}

/// A change as a chat shows it: under each file's path, the lines that went
/// after `- ` and the lines that came after `+ `. A file added or deleted is
/// its first lines.
fn changes_detail(changes: &Value) -> String {
    let files: Vec<String> = blocks(changes)
        .iter()
        .map(|change| {
            let path = change["path"].as_str().unwrap_or_default();
            let diff = change["diff"].as_str().unwrap_or_default();
            match change["kind"]["type"].as_str() {
                Some("add") => written_detail(path, diff, '+'),
                Some("delete") => written_detail(path, diff, '-'),
                _ => {
                    let mut lines = vec![path.to_owned()];
                    lines.extend(diff_lines(diff));
                    lines.join("\n")
                }
            }
        })
        .collect();
    cap_detail(&files.join("\n\n"))
}

fn permissions_line(wanted: &Value) -> String {
    let mut parts = Vec::new();
    if wanted["network"]["enabled"].as_bool() == Some(true) {
        parts.push("the network".to_owned());
    }
    for (key, words) in [("write", "write to"), ("read", "read")] {
        let paths: Vec<String> = blocks(&wanted["fileSystem"][key])
            .iter()
            .map(text_of)
            .collect();
        if !paths.is_empty() {
            parts.push(format!("{words} {}", paths.join(", ")));
        }
    }
    if parts.is_empty() {
        return "more than its sandbox allows".to_owned();
    }
    clip(&parts.join("; "), 200)
}

fn input_question(question: &Value) -> ChoiceQuestion {
    let options: Vec<ChoiceOption> = blocks(&question["options"])
        .iter()
        .map(|option| ChoiceOption {
            label: text_of(&option["label"]),
            description: option["description"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
        })
        .collect();
    ChoiceQuestion {
        id: text_of(&question["id"]),
        header: question["header"]
            .as_str()
            .filter(|header| !header.is_empty())
            .map(str::to_owned),
        question: text_of(&question["question"]),
        multi_select: false,
        allow_other: question["isOther"].as_bool() == Some(true) || options.is_empty(),
        options,
    }
}

impl AgentSession for CodexSession {
    fn first(&mut self, turn: &TurnSpec<'_>) -> Vec<Outgoing> {
        self.prompt = Some(turn.prompt.to_owned());
        self.effort = turn
            .effort
            .filter(|effort| !effort.is_empty())
            .map(str::to_owned);
        let mut params = Map::new();
        if let Some((approval, sandbox)) = turn.mode.and_then(mode_parts) {
            params.insert("approvalPolicy".to_owned(), json!(approval));
            params.insert("sandbox".to_owned(), json!(sandbox));
        }
        if let Some(model) = turn.model.filter(|model| !model.is_empty()) {
            params.insert("model".to_owned(), json!(model));
        }
        let thread = match turn.resume.filter(|id| !id.is_empty()) {
            Some(id) => {
                params.insert("threadId".to_owned(), json!(id));
                // Its history is not wanted back: a long one would fill the
                // output the host keeps of this turn.
                params.insert("excludeTurns".to_owned(), json!(true));
                request(ID_THREAD, "thread/resume", Value::Object(params))
            }
            None => request(ID_THREAD, "thread/start", Value::Object(params)),
        };
        vec![
            Outgoing {
                tag: TAG_HELLO.to_owned(),
                line: hello(),
            },
            Outgoing {
                tag: TAG_READY.to_owned(),
                line: json!({ "method": "initialized" }).to_string(),
            },
            Outgoing {
                tag: TAG_THREAD.to_owned(),
                line: thread,
            },
        ]
    }

    fn line(&mut self, value: &Value, step: &mut Step) {
        match (value["method"].is_string(), value["id"].is_null()) {
            (true, false) => self.ask(value, step),
            (true, true) => self.told(value, step),
            (false, false) => self.answered(value, step),
            (false, true) => {}
        }
    }

    fn noise(&mut self, line: &str) {
        let text = line.trim();
        if !text.is_empty() {
            if self.noise.len() == NOISE_LINES {
                self.noise.pop_front();
            }
            self.noise.push_back(clip(text, 200));
        }
    }

    fn delivered(&mut self, tag: &str, step: &mut Step) {
        if let Some(id) = tag_id('a', tag) {
            self.resolve(&id, step);
        }
    }

    fn encode(&mut self, action: Action) -> Result<Vec<Outgoing>, TurnError> {
        match action {
            Action::Prompt(text) => {
                if self.finished {
                    return Err(TurnError::TurnEnded);
                }
                match self.steer(&text) {
                    Some(outgoing) => Ok(vec![outgoing]),
                    None => {
                        self.say_later.push(text);
                        Ok(Vec::new())
                    }
                }
            }
            Action::Answer(id, answer) => Ok(vec![Outgoing {
                tag: id_tag('a', &id),
                line: self.answer(&id, answer)?.to_string(),
            }]),
            Action::Interrupt => {
                if self.finished {
                    return Err(TurnError::TurnEnded);
                }
                match self.interrupt() {
                    Some(outgoing) => Ok(vec![outgoing]),
                    None => {
                        self.stop_wanted = true;
                        Ok(Vec::new())
                    }
                }
            }
            // A thread's mode and model are those it was started with.
            Action::SetMode(_) | Action::SetModel(_) => Err(TurnError::NotSupported),
        }
    }

    fn idle(&self) -> bool {
        self.idle && self.asked.is_empty()
    }

    fn exited(&mut self, code: u32) -> Vec<TurnEvent> {
        let mut events: Vec<TurnEvent> = std::mem::take(&mut self.asked)
            .into_keys()
            .map(|id| TurnEvent::QuestionResolved { id })
            .collect();
        if !self.finished {
            self.finished = true;
            let ok = code == 0 && self.turn.is_some();
            let error = (!ok).then(|| {
                self.last_error.take().unwrap_or_else(|| {
                    if self.noise.is_empty() {
                        format!("the agent stopped with exit code {code}")
                    } else {
                        self.noise.iter().cloned().collect::<Vec<_>>().join("\n")
                    }
                })
            });
            events.push(TurnEvent::Finished {
                ok,
                error,
                cost_usd: None,
                duration_ms: None,
                denied_tools: Vec::new(),
                interrupted: false,
            });
        }
        events.push(TurnEvent::Exited { code });
        events
    }
}

/// The command that asks Codex what it offers: its version, then the answers
/// of its server to the requests given as `$2`…, in whichever order they
/// come. Its models and its configuration (`model/list`, `config/read`) are
/// waited for; its skills (`skills/list`) a moment longer, because a server
/// may be slow to list them and what it offers is still worth knowing
/// without. The server ends when its input does, so its input is a pipe
/// held open by a process of its own, which is ended when the answers are
/// in, and ends by itself if they never are.
pub const OPTIONS_SCRIPT: &str = r#"program=$1; shift
printf 'leon-version:%s\n' "$("$program" --version 2>/dev/null | tail -n 1)"
pipe="${TMPDIR:-/tmp}/leon-options.$$"
rm -f "$pipe"
mkfifo "$pipe" || exit 0
trap 'rm -f "$pipe"' EXIT
( printf '%s\n' "$@"; exec sleep 20 ) >"$pipe" 2>/dev/null &
writer=$!
"$program" app-server <"$pipe" 2>/dev/null | {
    models=; config=; skills=; grace=
    while IFS= read -r line; do
        printf '%s\n' "$line"
        case "$line" in
        *'"id":"leon-models"'*) models=1 ;;
        *'"id":"leon-config"'*) config=1 ;;
        *'"id":"leon-skills"'*) skills=1 ;;
        esac
        [ "$models$config$skills" = 111 ] && break
        if [ "$models$config" = 11 ] && [ -z "$grace" ]; then
            grace=1
            ( sleep 2; kill "$writer" ) >/dev/null 2>&1 &
        fi
    done
    kill "$writer" 2>/dev/null
}
exit 0"#;

/// What [`OPTIONS_SCRIPT`] is given after the program: the requests.
pub fn options_arguments() -> Vec<String> {
    vec![
        hello(),
        json!({ "method": "initialized" }).to_string(),
        request("leon-models", "model/list", json!({})),
        request("leon-config", "config/read", json!({})),
        request("leon-skills", "skills/list", json!({})),
    ]
}

/// Reads the output of [`OPTIONS_SCRIPT`].
pub fn parse_options(text: &str) -> AgentOptions {
    let mut options = AgentOptions {
        modes: MODES
            .iter()
            .map(|mode| AgentMode {
                id: mode.0.to_owned(),
                label: mode.3.to_owned(),
                description: Some(mode.4.to_owned()),
                dangerous: mode.2 == "danger-full-access",
            })
            .collect(),
        commands: natives(),
        questions: true,
        follow_ups: true,
        live_changes: false,
        // Until its server answered, the models are not known.
        assumed: true,
        ..AgentOptions::default()
    };
    let (mut set_model, mut set_effort) = (None::<String>, None::<String>);
    let mut listed_model = None::<String>;
    let mut efforts_unasked: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(version) = line.strip_prefix("leon-version:") {
            options.version = Some(version.trim().to_owned()).filter(|text| !text.is_empty());
            continue;
        }
        let Some(value) = crate::events::object_on(line) else {
            continue;
        };
        let result = &value["result"];
        match value["id"].as_str() {
            Some("leon-models") if result["data"].is_array() => {
                options.assumed = false;
                // Its own choice, and how hard each one thinks unasked.
                let name = |model: &Value| {
                    model["model"]
                        .as_str()
                        .or(model["id"].as_str())
                        .map(str::to_owned)
                };
                listed_model = blocks(&result["data"])
                    .iter()
                    .find(|model| model["isDefault"].as_bool() == Some(true))
                    .and_then(name);
                efforts_unasked = blocks(&result["data"])
                    .iter()
                    .filter_map(|model| {
                        Some((
                            name(model)?,
                            model["defaultReasoningEffort"].as_str()?.to_owned(),
                        ))
                    })
                    .collect();
                options.models = blocks(&result["data"])
                    .iter()
                    .filter(|model| model["hidden"].as_bool() != Some(true))
                    .filter_map(|model| {
                        let id = model["model"].as_str().or(model["id"].as_str())?;
                        Some(AgentModel {
                            id: id.to_owned(),
                            label: model["displayName"].as_str().unwrap_or(id).to_owned(),
                            description: model["description"]
                                .as_str()
                                .filter(|text| !text.is_empty())
                                .map(str::to_owned),
                            efforts: blocks(&model["supportedReasoningEfforts"])
                                .iter()
                                .map(|effort| text_of(&effort["reasoningEffort"]))
                                .filter(|effort| !effort.is_empty())
                                .collect(),
                            resolves_to: None,
                        })
                    })
                    .collect();
            }
            Some("leon-skills") => {
                // A skill is asked for by naming it after a dollar sign.
                for skill in blocks(&result["data"])
                    .iter()
                    .flat_map(|folder| blocks(&folder["skills"]))
                {
                    let Some(name) = skill["name"].as_str().filter(|name| !name.is_empty()) else {
                        continue;
                    };
                    if skill["enabled"].as_bool() == Some(false)
                        || options.commands.iter().any(|have| have.name == name)
                    {
                        continue;
                    }
                    let says = skill["shortDescription"]
                        .as_str()
                        .or(skill["description"].as_str());
                    options.commands.push(AgentCommand {
                        insert: format!("${name}"),
                        ..AgentCommand::slash(name, says, None)
                    });
                }
            }
            Some("leon-config") => {
                let config = &result["config"];
                options.default_mode =
                    mode_named(&config["approval_policy"], &config["sandbox_mode"])
                        .map(str::to_owned);
                let set = |value: &Value| {
                    value
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned)
                };
                set_model = set(&config["model"]);
                set_effort = set(&config["model_reasoning_effort"]);
            }
            _ => {}
        }
    }
    // What its configuration names, else its own choice.
    options.default_model = set_model.or(listed_model);
    options.default_effort = set_effort.or_else(|| {
        let model = options.default_model.as_ref()?;
        efforts_unasked
            .iter()
            .find(|(id, _)| id == model)
            .map(|(_, effort)| effort.clone())
    });
    options
}

#[cfg(test)]
mod tests;

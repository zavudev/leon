//! The Agent Client Protocol, spoken to both ways: one session for every
//! agent that serves it on its standard streams.
//!
//! Each line either way is one JSON-RPC 2.0 value; the `jsonrpc` member is
//! required. The client writes:
//!
//! * `initialize`, then `session/new` (with the folder to work in, which
//!   must be a whole path) or what continues a conversation the agent named
//!   before: `session/resume`, which says nothing of what was said, or
//!   `session/load`, which prints it all again (nothing of that is shown: a
//!   client that continues a conversation has it already). An agent that
//!   wants to be signed in to is told `authenticate` first, and the session
//!   is asked for once that was answered;
//! * `session/set_config_option` for the model, the effort and the mode that
//!   were asked for, one after the other, each once the one before was
//!   answered. Requests are served as they come, not in order, so nothing
//!   that needs the session is written before the session's answer;
//! * `session/prompt` with the prompt. Its answer is the end of the turn;
//! * `session/cancel` to stop it;
//! * the answer to what the agent asks.
//!
//! The agent prints `session/update` notifications (`agent_message_chunk`,
//! `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`,
//! `usage_update`, `config_option_update`…) and asks with
//! `session/request_permission`, which lists what it may be answered with
//! (`allow_once`, `allow_always`, `reject_once`, `reject_always`). It cannot
//! be told more while it works (a second prompt is the next turn), and it
//! reads no file and runs no terminal through the client: this one offers
//! neither. Notifications of an agent's own making (`_x.ai/…`) are passed
//! over.
//!
//! Where the agents differ is a [`Dialect`]: what the option of the effort
//! is called, whether the agent has modes, how a conversation is continued.
//! `opencode`, `grok` and `cursor` each hold theirs.
//!
//! No agent of the three has a mode in which it asks nothing that can be
//! set over the protocol. [`MODE_AUTO`] is this library's: every permission
//! the agent asks for is allowed at once.
//!
//! The server ends when its input does: after the prompt's answer, its input
//! is closed.

use std::collections::{BTreeMap, HashSet, VecDeque};

use serde_json::{json, Value};

use crate::agents::{Action, AgentSession, McpServer, Outgoing, Step, TurnSpec};
use crate::claude::AgentCommand;
use crate::error::TurnError;
use crate::events::{
    blocks, cap_detail, clip, edit_detail, first_line, input_detail, input_summary, text_detail,
    text_of, written_detail, Answer, PermissionChoice, PermissionEffect, QuestionKind, Tokens,
    TurnEvent,
};
use crate::tag::{id_tag, tag_id};

/// The ids of this client's own requests. They are fixed, so that a turn
/// read again knows which answer is which.
const ID_HELLO: &str = "leon-init";
const ID_LOGIN: &str = "leon-login";
const ID_SESSION: &str = "leon-session";
const ID_MODEL: &str = "leon-model";
const ID_EFFORT: &str = "leon-effort";
const ID_MODE: &str = "leon-mode";
const ID_TURN: &str = "leon-turn";
/// A request a process that was never told the prompt writes in place of
/// `session/prompt`: its answer means nobody had started the turn.
const ID_LOST: &str = "leon-lost";

const TAG_HELLO: &str = "h.init";
const TAG_LOGIN: &str = "h.login";
const TAG_SESSION: &str = "h.session";
const TAG_TURN: &str = "h.turn";

/// The mode of this library's making: the agent as it is, never asking.
pub const MODE_AUTO: &str = "leon-auto";

/// How one agent speaks the protocol where the agents differ.
#[derive(Debug)]
pub struct Dialect {
    /// The agent's name, for what is said when something fails.
    pub name: &'static str,
    /// The request that continues a conversation: `session/resume`, or
    /// `session/load` where the agent has only that.
    pub resume: &'static str,
    /// The way of signing in the agent must be told before a session, where
    /// it wants to be told.
    pub login: Option<&'static str>,
    /// The id of the session's option that is the effort.
    pub effort: &'static str,
    /// The mode [`MODE_AUTO`] works as, for an agent whose modes are an
    /// option of the session. `None`: it has no modes to set, and the only
    /// ones are this library's (asking, and never asking).
    pub auto_agent: Option<&'static str>,
    /// The models of every provider are only known a moment after the
    /// session answered: the first `config_option_update` says so, and a
    /// model or an effort waits for it (set before, the session would be
    /// tied to a model it may not be able to use).
    pub models_come_late: bool,
    /// A refusal ends the turn, which the choice then says.
    pub refusal_ends_turn: bool,
}

/// How many characters of a change are shown to whoever must approve it.
const DETAIL_CHARS: usize = 20_000;
/// How many lines that were not the server's are kept to explain a failure.
const NOISE_LINES: usize = 6;

/// What is said when the turn was never started.
pub(crate) fn never_started(name: &str) -> String {
    format!("The prompt did not reach {name} before the connection was lost. Send it again.")
}

/// A session for one turn of the agent that speaks `dialect`.
pub fn session(unique: u64, dialect: &'static Dialect) -> Box<dyn AgentSession> {
    Box::new(AcpSession {
        dialect,
        held: None,
        replaying: false,
        unique,
        prompt: None,
        resumed: false,
        auto: false,
        todo: VecDeque::new(),
        waits_for_models: false,
        models_known: false,
        setting: false,
        session: None,
        started: false,
        ready: false,
        prompt_queued: false,
        prompt_delivered: false,
        finished: false,
        idle: false,
        stop_wanted: false,
        asked: BTreeMap::new(),
        tools: BTreeMap::new(),
        announced: HashSet::new(),
        message: None,
        draft: String::new(),
        mode: None,
        model: None,
        cost: None,
        context_window: None,
        noise: VecDeque::new(),
    })
}

fn request(id: &str, method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// The opening request, as a line. No capability is offered: the agent
/// reads files and runs commands itself, on its own computer.
fn hello() -> String {
    request(
        ID_HELLO,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {
                "name": "leon-mobile",
                "title": "Leon",
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
    )
}

/// One option of the session still to be set: the request's id, the
/// option, its value.
#[derive(Debug)]
struct Setting {
    id: &'static str,
    option: &'static str,
    value: String,
}

/// What the agent asked and has no answer to yet.
#[derive(Debug)]
struct Asked {
    /// The request's id as the agent wrote it (a number, or text).
    id: Value,
    /// What it may be answered with: the option's id and its kind.
    options: Vec<(String, String)>,
}

/// A tool the agent uses, as far as it said.
#[derive(Debug, Default)]
struct Tool {
    /// The tool's own name (`shell`, `write`), from its first mention.
    name: String,
    /// Whether what it does was shown in full when it started.
    shown: bool,
}

struct AcpSession {
    dialect: &'static Dialect,
    /// The request for the session, held back until the agent was signed
    /// in to.
    held: Option<Outgoing>,
    /// The agent prints a conversation it was asked to load: none of it is
    /// news.
    replaying: bool,
    /// Makes the ids of this process's own requests unlike any other's.
    unique: u64,
    /// What the person wrote; nothing in a process that found the turn
    /// running.
    prompt: Option<String>,
    /// The conversation is an earlier one: what it has cost so far is not
    /// known, and so not what this turn costs.
    resumed: bool,
    /// Every permission asked for is allowed at once ([`MODE_AUTO`]).
    auto: bool,
    /// What is still to be set before the prompt, in order.
    todo: VecDeque<Setting>,
    /// A model or an effort is to be set, which needs the models known.
    waits_for_models: bool,
    models_known: bool,
    /// An option was written and its answer is awaited.
    setting: bool,
    session: Option<String>,
    started: bool,
    /// The session answered: what needs it may be written.
    ready: bool,
    /// The prompt (or what stands for it) was handed over to be written.
    prompt_queued: bool,
    /// The prompt reached the agent (this process's, or an earlier one's).
    prompt_delivered: bool,
    /// The turn ended (or never began): nothing more can be said in it.
    finished: bool,
    idle: bool,
    /// An interrupt was asked for before the session had an id.
    stop_wanted: bool,
    /// Open questions, by the text of the request's id.
    asked: BTreeMap<String, Asked>,
    tools: BTreeMap<String, Tool>,
    /// Tools shown as in use and not as done.
    announced: HashSet<String>,
    /// The message being written: its id, and what it says so far.
    message: Option<String>,
    draft: String,
    mode: Option<String>,
    model: Option<String>,
    /// What the conversation has cost, as last said.
    cost: Option<f64>,
    /// How many tokens the model can hold, as last said.
    context_window: Option<u64>,
    noise: VecDeque<String>,
}

/// The tokens of a turn, from the `usage` of the prompt's answer: what was
/// read is the fresh input and both caches, what was written the answer and
/// the thoughts.
fn acp_tokens(usage: &Value, context_window: Option<u64>) -> Option<Tokens> {
    if !usage.is_object() {
        return None;
    }
    let count = |keys: &[&str]| -> u64 {
        keys.iter()
            .filter_map(|key| usage[*key].as_u64())
            .next()
            .unwrap_or(0)
    };
    let cached = count(&["cachedReadTokens"]);
    let reasoning = count(&["thoughtTokens", "reasoningTokens"]);
    Some(Tokens {
        input: count(&["inputTokens"])
            + cached
            + count(&["cachedWriteTokens", "cacheCreationTokens"]),
        cached_input: cached,
        output: count(&["outputTokens"]) + reasoning,
        reasoning,
        context_window,
    })
}

impl AcpSession {
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

    /// The conversation has this id: said once.
    fn session_is(&mut self, id: &str, step: &mut Step) {
        if id.is_empty() {
            return;
        }
        if self.session.is_none() {
            self.session = Some(id.to_owned());
        }
        if !self.started {
            self.started = true;
            step.events.push(TurnEvent::Started {
                session_id: id.to_owned(),
                model: self.model.clone(),
            });
        }
        if self.stop_wanted {
            self.stop_wanted = false;
            step.send.extend(self.cancel());
        }
    }

    /// The session's options as the agent lists them: what it is set to.
    fn options_are(&mut self, options: &Value, step: &mut Step) {
        for option in blocks(options) {
            let Some(current) = option["currentValue"].as_str() else {
                continue;
            };
            match option["id"].as_str() {
                Some("model") => self.model = Some(current.to_owned()),
                Some("mode") => self.mode_is(current, step),
                _ => {}
            }
        }
    }

    fn mode_is(&mut self, mode: &str, step: &mut Step) {
        // Its agent under this library's mode is still that mode.
        let mode = if self.auto && Some(mode) == self.dialect.auto_agent {
            MODE_AUTO
        } else {
            mode
        };
        if !mode.is_empty() && self.mode.as_deref() != Some(mode) {
            self.mode = Some(mode.to_owned());
            step.events.push(TurnEvent::ModeChanged {
                mode: mode.to_owned(),
            });
        }
    }

    fn set(&self, id: &str, option: &str, value: &str) -> Option<String> {
        let session = self.session.as_deref()?;
        Some(request(
            id,
            "session/set_config_option",
            json!({ "sessionId": session, "configId": option, "value": value }),
        ))
    }

    /// The next thing to write before the turn runs: the next option, or
    /// the prompt.
    fn advance(&mut self, step: &mut Step) {
        if !self.ready
            || self.finished
            || self.prompt_queued
            || self.setting
            || (self.waits_for_models && !self.models_known)
        {
            return;
        }
        if let Some(setting) = self.todo.pop_front() {
            if let Some(line) = self.set(setting.id, setting.option, &setting.value) {
                self.setting = true;
                step.send.push(Outgoing {
                    tag: format!("h.{}", setting.option),
                    line,
                });
            }
            return;
        }
        let line = match (&self.prompt, &self.session) {
            (Some(prompt), Some(session)) => request(
                ID_TURN,
                "session/prompt",
                json!({ "sessionId": session, "prompt": [{ "type": "text", "text": prompt }] }),
            ),
            (Some(_), None) => return,
            // This process found the turn running and was never told the
            // prompt. Whoever started it wrote the prompt under this tag: if
            // that arrived, this line is never written; if it did not, the
            // answer to this one (no such method) says so.
            (None, _) => request(ID_LOST, "leon/lost", json!({})),
        };
        self.prompt_queued = true;
        step.send.push(Outgoing {
            tag: TAG_TURN.to_owned(),
            line,
        });
    }

    /// The turn ended without a result.
    fn fail(&mut self, error: String, step: &mut Step) {
        if self.finished {
            return;
        }
        self.end(step);
        step.events.push(TurnEvent::Finished {
            ok: false,
            error: Some(error),
            cost_usd: None,
            duration_ms: None,
            denied_tools: Vec::new(),
            interrupted: false,
        });
    }

    /// What every end of a turn does first.
    fn end(&mut self, step: &mut Step) {
        self.settle(step);
        self.finished = true;
        self.idle = true;
        self.resolve_all(step);
        // What a stopped turn was doing is never said to end.
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
    }

    /// What stops the turn: the notification, and "cancelled" to whatever
    /// the agent still asks, as the protocol wants.
    fn cancel(&mut self) -> Vec<Outgoing> {
        let Some(session) = self.session.clone() else {
            return Vec::new();
        };
        let id = self.next_id("stop");
        let mut out = vec![Outgoing {
            tag: format!("i.{id}"),
            line: json!({
                "jsonrpc": "2.0",
                "method": "session/cancel",
                "params": { "sessionId": session },
            })
            .to_string(),
        }];
        for (id, asked) in &self.asked {
            out.push(Outgoing {
                tag: id_tag('a', id),
                line: outcome(&asked.id, json!({ "outcome": "cancelled" })),
            });
        }
        out
    }

    fn answered(&mut self, value: &Value, step: &mut Step) {
        let failed = value["error"]["message"]
            .as_str()
            .map(|text| {
                text.strip_prefix("Internal error: ")
                    .unwrap_or(text)
                    .to_owned()
            })
            .or_else(|| (!value["error"].is_null()).then(|| value["error"].to_string()));
        match value["id"].as_str().unwrap_or_default() {
            ID_HELLO => {
                if let Some(error) = failed {
                    self.fail(error, step);
                }
            }
            ID_LOGIN => {
                if let Some(error) = failed {
                    self.fail(error, step);
                    return;
                }
                step.send.extend(self.held.take());
            }
            ID_SESSION => {
                if let Some(error) = failed {
                    self.fail(error, step);
                    return;
                }
                self.replaying = false;
                let result = &value["result"];
                self.model = blocks(&result["configOptions"])
                    .iter()
                    .find(|option| option["id"] == "model")
                    .and_then(|option| option["currentValue"].as_str())
                    .map(str::to_owned);
                // A resumed session is not named again: its id is the one
                // that was asked for.
                let id = result["sessionId"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| self.session.clone());
                match id {
                    Some(id) => self.session_is(&id, step),
                    None if self.prompt.is_some() => {
                        let name = self.dialect.name;
                        self.fail(format!("{name} did not name the conversation"), step);
                        return;
                    }
                    // Found running: an update will name it.
                    None => {}
                }
                self.options_are(&result["configOptions"], step);
                self.ready = true;
                self.advance(step);
            }
            // An option it does not have (a model that is gone, an effort
            // this model lacks) is no reason not to run: the turn goes on
            // with what that computer has.
            ID_MODEL | ID_EFFORT | ID_MODE => {
                self.setting = false;
                if failed.is_none() {
                    self.options_are(&value["result"]["configOptions"], step);
                }
                self.advance(step);
            }
            ID_TURN => {
                if self.finished {
                    return;
                }
                if let Some(error) = failed {
                    self.fail(error, step);
                    return;
                }
                self.end(step);
                let reason = value["result"]["stopReason"].as_str().unwrap_or("end_turn");
                let ok = reason == "end_turn";
                let interrupted = reason == "cancelled";
                let error = (!ok && !interrupted).then(|| match reason {
                    "max_tokens" => "It ran out of room for its answer.".to_owned(),
                    "max_turn_requests" => "It reached its limit of steps for one turn.".to_owned(),
                    "refusal" => "The model declined to go on.".to_owned(),
                    other => format!("The turn stopped: {other}"),
                });
                if let Some(tokens) = acp_tokens(&value["result"]["usage"], self.context_window) {
                    step.events.push(TurnEvent::Usage { tokens });
                }
                step.events.push(TurnEvent::Finished {
                    ok,
                    error,
                    // The cost said is the conversation's: this turn's only
                    // when the conversation began with it.
                    cost_usd: self.cost.filter(|_| !self.resumed && self.prompt.is_some()),
                    duration_ms: None,
                    denied_tools: Vec::new(),
                    interrupted,
                });
            }
            ID_LOST if !self.prompt_delivered => {
                self.fail(never_started(self.dialect.name), step);
            }
            _ => {}
        }
    }

    fn ask(&mut self, value: &Value, step: &mut Step) {
        let id = id_text(&value["id"]);
        let params = &value["params"];
        if value["method"] != "session/request_permission" {
            // Files and terminals of a client this is not: said at once, so
            // the agent does not wait for nobody.
            step.send.push(Outgoing {
                tag: id_tag('r', &id),
                line: json!({
                    "jsonrpc": "2.0",
                    "id": value["id"],
                    "error": { "code": -32601, "message": "not supported by this client" },
                })
                .to_string(),
            });
            return;
        }
        if let Some(session) = params["sessionId"].as_str() {
            self.session_is(session, step);
        }
        let options: Vec<(String, String)> = blocks(&params["options"])
            .iter()
            .map(|option| (text_of(&option["optionId"]), text_of(&option["kind"])))
            .collect();
        if self.auto {
            let allow = options
                .iter()
                .find(|option| option.1 == "allow_once")
                .or_else(|| options.iter().find(|option| option.1 == "allow_always"));
            if let Some((option, _)) = allow {
                step.send.push(Outgoing {
                    tag: id_tag('a', &id),
                    line: outcome(
                        &value["id"],
                        json!({ "outcome": "selected", "optionId": option }),
                    ),
                });
                return;
            }
        }
        let call = &params["toolCall"];
        let tool = match call["kind"].as_str() {
            Some("execute") => "shell".to_owned(),
            Some(kind) if !kind.is_empty() && kind != "other" => kind.to_owned(),
            _ => self
                .tools
                .get(call["toolCallId"].as_str().unwrap_or_default())
                .map(|tool| tool.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "tool".to_owned()),
        };
        let choices = blocks(&params["options"])
            .iter()
            .filter_map(|option| {
                let name = text_of(&option["name"]);
                let (effect, label) = match option["kind"].as_str()? {
                    "allow_once" => (PermissionEffect::AllowOnce, name),
                    "allow_always" => (PermissionEffect::AllowAlways, name),
                    // Where a refusal ends the turn, the choice says so.
                    "reject_once" if self.dialect.refusal_ends_turn => {
                        (PermissionEffect::Deny, format!("{name}, and stop the turn"))
                    }
                    "reject_once" => (PermissionEffect::Deny, name),
                    "reject_always" => (PermissionEffect::Deny, name),
                    _ => return None,
                };
                Some(PermissionChoice {
                    id: text_of(&option["optionId"]),
                    effect,
                    label,
                    scope: None,
                    takes_reason: false,
                })
            })
            .collect();
        // Asked twice (a replay after a gap): the later asking stands.
        self.asked.insert(
            id.clone(),
            Asked {
                id: value["id"].clone(),
                options,
            },
        );
        step.events.push(TurnEvent::Question {
            id,
            kind: QuestionKind::Permission {
                tool,
                summary: first_line(&text_of(&call["title"])),
                detail: detail_of(call),
                reason: None,
                choices,
            },
        });
    }

    fn answer(&self, id: &str, answer: Answer) -> Result<String, TurnError> {
        let asked = self.asked.get(id).ok_or(TurnError::UnknownQuestion)?;
        let Answer::Permission { choice_id, .. } = answer else {
            return Err(TurnError::BadAnswer);
        };
        let known = |wanted: &str| asked.options.iter().any(|option| option.0 == wanted);
        let result = if known(&choice_id) {
            json!({ "outcome": "selected", "optionId": choice_id })
        } else if choice_id == "deny" {
            // A refusal that names no option: the agent's own, if it has one.
            match asked
                .options
                .iter()
                .find(|option| option.1.starts_with("reject"))
            {
                Some((option, _)) => json!({ "outcome": "selected", "optionId": option }),
                None => json!({ "outcome": "cancelled" }),
            }
        } else {
            return Err(TurnError::BadAnswer);
        };
        Ok(outcome(&asked.id, result))
    }

    /// The message being written is whole: said as one, so that the next
    /// one starts on its own.
    fn settle(&mut self, step: &mut Step) {
        self.message = None;
        let text = std::mem::take(&mut self.draft);
        if !text.is_empty() {
            step.events.push(TurnEvent::Text { text });
        }
    }

    fn tool(&mut self, update: &Value, step: &mut Step) {
        let id = text_of(&update["toolCallId"]);
        if id.is_empty() {
            return;
        }
        let title = text_of(&update["title"]);
        let tool = self.tools.entry(id.clone()).or_default();
        // The first mention names the tool; later ones say what it does.
        if tool.name.is_empty() {
            tool.name = match update["kind"].as_str() {
                Some("execute") => "shell".to_owned(),
                _ if !title.is_empty() => first_line(&title),
                Some(kind) => kind.to_owned(),
                None => "tool".to_owned(),
            };
        }
        let status = update["status"].as_str().unwrap_or("pending");
        if status == "pending" && update["sessionUpdate"] == "tool_call" {
            // Nothing is known of it yet but that it will come.
            return;
        }
        let name = tool.name.clone();
        let done = matches!(status, "completed" | "failed");
        let changes = changes_detail(update);
        if self.announced.insert(id.clone()) {
            // What it was given, in a line; where that is not something this
            // library reads (nothing, or the whole input as it stands), the
            // agent's own words for it.
            let summary = match input_summary(&update["rawInput"]) {
                summary if (summary.is_empty() || summary.starts_with('{')) && title != name => {
                    first_line(&title)
                }
                summary => summary,
            };
            let detail = if changes.is_empty() {
                input_detail(&update["rawInput"], &summary)
            } else {
                changes.clone()
            };
            tool.shown = !detail.is_empty();
            self.settle(step);
            step.events.push(TurnEvent::ToolUse {
                id: id.clone(),
                name,
                summary,
                detail,
            });
        }
        if done {
            self.announced.remove(&id);
            let shown = self.tools.remove(&id).is_some_and(|tool| tool.shown);
            let texts: Vec<&str> = blocks(&update["content"])
                .iter()
                .filter_map(|block| block["content"]["text"].as_str())
                .collect();
            let said = texts
                .first()
                .copied()
                .or(update["rawOutput"]["error"].as_str())
                .unwrap_or_default();
            let summary = first_line(said);
            let whole = match texts.is_empty() {
                true => update["rawOutput"]["output"]
                    .as_str()
                    .unwrap_or(said)
                    .to_owned(),
                false => texts.join("\n"),
            };
            let mut detail = text_detail(&whole, &summary);
            // A change only said once it was made is not lost.
            if detail.is_empty() && !shown {
                detail = changes;
            }
            step.events.push(TurnEvent::ToolResult {
                id,
                ok: status == "completed",
                summary,
                detail,
            });
        }
    }

    fn told(&mut self, value: &Value, step: &mut Step) {
        if value["method"] != "session/update" {
            return;
        }
        let params = &value["params"];
        // Some agents speak of the session before they answered for it: the
        // answer names it then, with the model it runs on. Only a process
        // that found the turn running learns its name here.
        if let Some(session) = params["sessionId"]
            .as_str()
            .filter(|_| self.ready || self.prompt.is_none())
        {
            self.session_is(session, step);
        }
        let update = &params["update"];
        let kind = update["sessionUpdate"].as_str().unwrap_or_default();
        // A conversation printed again: only what it is set to is news.
        if self.replaying
            && !matches!(
                kind,
                "config_option_update" | "available_commands_update" | "current_mode_update"
            )
        {
            return;
        }
        match kind {
            "agent_message_chunk" => {
                let Some(text) = update["content"]["text"].as_str().filter(|t| !t.is_empty())
                else {
                    return;
                };
                let id = update["messageId"].as_str();
                if self.message.is_some() && self.message.as_deref() != id {
                    self.settle(step);
                }
                self.message = id.map(str::to_owned);
                self.draft.push_str(text);
                step.events.push(TurnEvent::TextDelta {
                    text: text.to_owned(),
                });
            }
            "agent_thought_chunk" => {
                if let Some(text) = update["content"]["text"].as_str().filter(|t| !t.is_empty()) {
                    step.events.push(TurnEvent::Thought {
                        text: text.to_owned(),
                    });
                }
            }
            "tool_call" | "tool_call_update" => self.tool(update, step),
            "plan" => {
                let lines: Vec<String> = blocks(&update["entries"])
                    .iter()
                    .map(|entry| {
                        let mark = match entry["status"].as_str() {
                            Some("completed") => "x",
                            _ => " ",
                        };
                        format!("- [{mark}] {}", text_of(&entry["content"]))
                    })
                    .collect();
                if !lines.is_empty() {
                    self.settle(step);
                    step.events.push(TurnEvent::Text {
                        text: lines.join("\n"),
                    });
                }
            }
            "usage_update" => {
                self.context_window = update["size"].as_u64().or(self.context_window);
                if update["cost"]["currency"] == "USD" {
                    self.cost = update["cost"]["amount"].as_f64().or(self.cost);
                }
            }
            "config_option_update" => {
                self.options_are(&update["configOptions"], step);
                // The first one after the session says every provider's
                // models are known now.
                if !self.models_known {
                    self.models_known = true;
                    self.advance(step);
                }
            }
            "available_commands_update" => {
                step.commands = Some(commands(&update["availableCommands"]));
            }
            "current_mode_update" => {
                let mode = text_of(&update["currentModeId"]);
                self.mode_is(&mode, step);
            }
            _ => {}
        }
    }
}

/// The answer to a permission request.
fn outcome(id: &Value, outcome: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": outcome } }).to_string()
}

/// The id of a request as text: the same for the number and for its digits.
fn id_text(id: &Value) -> String {
    match id {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The changes of files a tool call carries, as a chat shows them: under
/// each path, the lines that went after `- ` and the lines that came after
/// `+ `; a new file is its first lines. Empty when it carries none.
fn changes_detail(call: &Value) -> String {
    let files: Vec<String> = blocks(&call["content"])
        .iter()
        .filter(|block| block["type"] == "diff")
        .map(|block| {
            let path = text_of(&block["path"]);
            let new = text_of(&block["newText"]);
            match block["oldText"].as_str() {
                Some(old) if !old.is_empty() => edit_detail(&path, old, &new),
                _ => written_detail(&path, &new, '+'),
            }
        })
        .collect();
    cap_detail(&files.join("\n\n"))
}

/// What is asked, in full: the change of a file as old and new, or what
/// the tool was given.
fn detail_of(call: &Value) -> String {
    let changes: Vec<String> = blocks(&call["content"])
        .iter()
        .filter(|block| block["type"] == "diff")
        .map(|block| {
            let path = text_of(&block["path"]);
            let new = text_of(&block["newText"]);
            match block["oldText"].as_str() {
                Some(old) if !old.is_empty() => {
                    format!("{path}\n--- before\n{old}\n+++ after\n{new}")
                }
                _ => format!("{path}\n+++ new file\n{new}"),
            }
        })
        .collect();
    if !changes.is_empty() {
        return clip(&changes.join("\n\n"), DETAIL_CHARS);
    }
    // A file to be written, said only as what the tool was given.
    let given = &call["rawInput"];
    let path = given["file_path"].as_str().or(given["path"].as_str());
    if let (Some(path), Some(content)) = (path, given["content"].as_str()) {
        return clip(&format!("{path}\n+++ new file\n{content}"), DETAIL_CHARS);
    }
    clip(
        &serde_json::to_string_pretty(&call["rawInput"]).unwrap_or_default(),
        DETAIL_CHARS,
    )
}

impl AgentSession for AcpSession {
    fn first(&mut self, turn: &TurnSpec<'_>) -> Vec<Outgoing> {
        self.prompt = Some(turn.prompt.to_owned());
        let wanted = |text: Option<&str>| text.filter(|text| !text.is_empty()).map(str::to_owned);
        let late = self.dialect.models_come_late;
        if let Some(value) = wanted(turn.model) {
            self.waits_for_models = late;
            self.todo.push_back(Setting {
                id: ID_MODEL,
                option: "model",
                value,
            });
        }
        if let Some(value) = wanted(turn.effort) {
            self.waits_for_models = late;
            self.todo.push_back(Setting {
                id: ID_EFFORT,
                option: self.dialect.effort,
                value,
            });
        }
        if let Some(mode) = wanted(turn.mode) {
            self.auto = mode == MODE_AUTO;
            // An agent without modes has only this library's two, and
            // neither is its to be told.
            if let Some(agent) = self.dialect.auto_agent {
                self.todo.push_back(Setting {
                    id: ID_MODE,
                    option: "mode",
                    value: if self.auto { agent.to_owned() } else { mode },
                });
            }
        }
        let folder = turn.cwd.filter(|cwd| !cwd.is_empty()).unwrap_or("/");
        let servers: Vec<Value> = turn.mcp_servers.iter().map(McpServer::acp).collect();
        let session = match wanted(turn.resume) {
            Some(id) => {
                self.resumed = true;
                self.session = Some(id.clone());
                self.replaying = self.dialect.resume == "session/load";
                request(
                    ID_SESSION,
                    self.dialect.resume,
                    json!({ "sessionId": id, "cwd": folder, "mcpServers": servers }),
                )
            }
            None => request(
                ID_SESSION,
                "session/new",
                json!({ "cwd": folder, "mcpServers": servers }),
            ),
        };
        let hello = Outgoing {
            tag: TAG_HELLO.to_owned(),
            line: hello(),
        };
        let session = Outgoing {
            tag: TAG_SESSION.to_owned(),
            line: session,
        };
        match self.dialect.login {
            // The session is refused to whoever did not sign in first.
            Some(method) => {
                self.held = Some(session);
                vec![
                    hello,
                    Outgoing {
                        tag: TAG_LOGIN.to_owned(),
                        line: request(ID_LOGIN, "authenticate", json!({ "methodId": method })),
                    },
                ]
            }
            None => vec![hello, session],
        }
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
        if tag == TAG_TURN {
            self.prompt_delivered = true;
        }
        if let Some(id) = tag_id('a', tag) {
            self.resolve(&id, step);
        }
    }

    fn encode(&mut self, action: Action) -> Result<Vec<Outgoing>, TurnError> {
        match action {
            // A second prompt while one runs is the next turn.
            Action::Prompt(_) => Err(TurnError::NotSupported),
            Action::Answer(id, answer) => Ok(vec![Outgoing {
                tag: id_tag('a', &id),
                line: self.answer(&id, answer)?,
            }]),
            Action::Interrupt => {
                if self.finished {
                    return Err(TurnError::TurnEnded);
                }
                if self.session.is_none() {
                    self.stop_wanted = true;
                    return Ok(Vec::new());
                }
                Ok(self.cancel())
            }
            Action::SetMode(mode) => {
                if self.finished {
                    return Err(TurnError::TurnEnded);
                }
                self.auto = mode == MODE_AUTO;
                let Some(agent) = self.dialect.auto_agent else {
                    // Its two modes are this library's: nothing to tell it.
                    return Ok(Vec::new());
                };
                let value = if self.auto { agent } else { mode.as_str() };
                let id = self.next_id("mode");
                let line = self
                    .set(&id, "mode", value)
                    .ok_or(TurnError::NotSupported)?;
                Ok(vec![Outgoing {
                    tag: format!("m.{id}"),
                    line,
                }])
            }
            Action::SetModel(model) => {
                if self.finished {
                    return Err(TurnError::TurnEnded);
                }
                let id = self.next_id("model");
                let line = self
                    .set(&id, "model", &model)
                    .ok_or(TurnError::NotSupported)?;
                Ok(vec![Outgoing {
                    tag: format!("m.{id}"),
                    line,
                }])
            }
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
            let error = if self.noise.is_empty() {
                format!("the agent stopped with exit code {code}")
            } else {
                self.noise.iter().cloned().collect::<Vec<_>>().join("\n")
            };
            events.push(TurnEvent::Finished {
                ok: false,
                error: Some(error),
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

/// The commands a session listed.
pub(crate) fn commands(listed: &Value) -> Vec<AgentCommand> {
    blocks(listed)
        .iter()
        .filter_map(|command| {
            let name = command["name"].as_str().filter(|name| !name.is_empty())?;
            Some(AgentCommand::slash(
                name,
                command["description"].as_str(),
                command["input"]["hint"].as_str(),
            ))
        })
        .collect()
}

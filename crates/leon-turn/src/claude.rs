//! Claude Code, spoken to both ways.
//!
//! Claude Code has a control protocol on its standard streams, the one its
//! SDK uses (checked against Claude Code 2.1.293):
//!
//! ```text
//! claude -p --input-format stream-json --output-format stream-json --verbose
//!        --include-partial-messages --permission-prompt-tool stdio
//!        [--resume <id>] [--permission-mode <mode>] [--model <model>]
//!        [--effort <level>] [--add-dir <dir>…]
//! ```
//!
//! Each line either way is one JSON value. The client writes:
//!
//! * `{"type":"control_request","request_id":…,"request":{"subtype":"initialize"}}`
//!   first; the answer lists the models;
//! * `{"type":"user","message":{"role":"user","content":[{"type":"text",…}]}}`
//!   for the prompt, and again to say more while it works (it is queued);
//! * `control_request`s `interrupt`, `set_permission_mode`, `set_model`;
//! * `{"type":"control_response","response":{"subtype":"success","request_id":…,
//!   "response":{…}}}` to answer what the agent asks.
//!
//! The agent prints what `--output-format stream-json` always printed
//! (`crate::events::Reader` reads it), and:
//!
//! * `control_request` `can_use_tool`: it wants to use a tool and somebody
//!   must say. The answer is `{"behavior":"allow","updatedInput":…}`, with
//!   `updatedPermissions` to make it a rule, or `{"behavior":"deny",
//!   "message":…}`. Two tools are questions in disguise: `AskUserQuestion`
//!   (allowed with the picks added to its input as `answers`) and
//!   `ExitPlanMode` (allowed to start working, denied with what to change);
//! * `control_cancel_request`: it no longer waits for that answer;
//! * `result` at the end of each prompt, with `queued_turn_count` saying how
//!   many more it was told meanwhile.
//!
//! It then waits for the next prompt until its input is closed.

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Map, Value};

use crate::agents::{strings, Action, AgentSession, McpServer, Outgoing, Step, TurnSpec};
use crate::error::TurnError;
use crate::events::{
    blocks, input_summary, text_of, Answer, ChoiceOption, ChoiceQuestion, PermissionChoice,
    PermissionEffect, QuestionKind, Reader, TurnEvent,
};
use crate::tag::{id_tag, tag_id};

/// The tag of the opening request.
const TAG_HELLO: &str = "h.init";
/// The tag of the turn's own prompt.
const TAG_PROMPT: &str = "p";
/// What a refusal says when the person gave no reason.
const DENIED: &str = "The user denied this tool use.";
/// What keeping on planning says when the person gave no direction.
const KEEP_PLANNING: &str = "The user wants to keep planning before any change is made.";

/// The command line of a turn.
pub fn argv(turn: &TurnSpec<'_>) -> Vec<String> {
    let mut argv = strings(&[
        turn.program,
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
    ]);
    for (flag, value) in [
        ("--resume", turn.resume),
        ("--permission-mode", turn.mode),
        ("--model", turn.model),
        ("--effort", turn.effort),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            argv.extend(strings(&[flag, value]));
        }
    }
    for dir in turn.add_dirs.iter().filter(|dir| !dir.is_empty()) {
        argv.extend(strings(&["--add-dir", dir]));
    }
    if !turn.mcp_servers.is_empty() {
        // One JSON value in the shape of a `.mcp.json` file.
        let config = json!({ "mcpServers": McpServer::configs(turn.mcp_servers) });
        argv.extend(["--mcp-config".to_owned(), config.to_string()]);
    }
    if let Some(text) = turn.instructions.filter(|text| !text.is_empty()) {
        argv.extend(strings(&["--append-system-prompt", text]));
    }
    argv
}

/// A session for one turn.
pub fn session(unique: u64) -> Box<dyn AgentSession> {
    Box::new(ClaudeSession {
        reader: Reader::new(),
        asked: BTreeMap::new(),
        quiet_tools: HashSet::new(),
        unique,
        mode: None,
        idle: false,
        interrupting: false,
    })
}

/// The opening request, as a line.
pub fn hello(request_id: &str) -> String {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "initialize", "hooks": null },
    })
    .to_string()
}

fn user_message(text: &str) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
        "parent_tool_use_id": null,
        "session_id": "",
    })
    .to_string()
}

fn control_request(request_id: &str, request: Value) -> String {
    json!({ "type": "control_request", "request_id": request_id, "request": request }).to_string()
}

fn control_answer(request_id: &str, response: Value) -> String {
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": response },
    })
    .to_string()
}

fn control_refusal(request_id: &str, error: &str) -> String {
    json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request_id, "error": error },
    })
    .to_string()
}

/// What the agent asked and has no answer to yet.
#[derive(Debug)]
struct Asked {
    /// The tool use the question is about: its result means the turn went on.
    tool_use_id: String,
    /// What the tool would be given, to hand back with the answer.
    input: Value,
    /// The rule changes offered with "always", as the agent wrote them.
    suggestions: Vec<Value>,
    /// What sort of question it was shown as.
    sort: Sort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Permission,
    Choices,
    Plan,
}

struct ClaudeSession {
    /// Reads the plain events.
    reader: Reader,
    /// Open questions, by request id.
    asked: BTreeMap<String, Asked>,
    /// Tool uses that are questions: shown as such, not as tools.
    quiet_tools: HashSet<String>,
    /// Makes the ids of this process's own requests unlike any other's.
    unique: u64,
    /// The permission mode last heard of.
    mode: Option<String>,
    /// The last prompt was answered and none is queued.
    idle: bool,
    /// An interrupt was asked for: the next failed result is its doing.
    interrupting: bool,
}

impl ClaudeSession {
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

    fn mode_is(&mut self, mode: Option<&str>, step: &mut Step) {
        let Some(mode) = mode.filter(|mode| !mode.is_empty()) else {
            return;
        };
        if self.mode.as_deref() != Some(mode) {
            self.mode = Some(mode.to_owned());
            step.events.push(TurnEvent::ModeChanged {
                mode: mode.to_owned(),
            });
        }
    }

    fn ask(&mut self, id: &str, request: &Value, step: &mut Step) {
        let tool = text_of(&request["tool_name"]);
        let input = request["input"].clone();
        let tool_use_id = text_of(&request["tool_use_id"]);
        let suggestions: Vec<Value> = blocks(&request["permission_suggestions"])
            .iter()
            .filter(|suggestion| suggestion["behavior"].as_str().unwrap_or("allow") == "allow")
            .cloned()
            .collect();
        let (sort, kind) = match tool.as_str() {
            "AskUserQuestion" => (
                Sort::Choices,
                QuestionKind::Choices {
                    questions: blocks(&input["questions"])
                        .iter()
                        .map(choice_question)
                        .collect(),
                },
            ),
            "ExitPlanMode" => (
                Sort::Plan,
                QuestionKind::Plan {
                    plan: text_of(&input["plan"]),
                },
            ),
            _ => (
                Sort::Permission,
                QuestionKind::Permission {
                    summary: input_summary(&input),
                    detail: serde_json::to_string_pretty(&input).unwrap_or_default(),
                    reason: request["decision_reason"]
                        .as_str()
                        .or(request["description"].as_str())
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned),
                    choices: permission_choices(&suggestions),
                    tool,
                },
            ),
        };
        if sort != Sort::Permission {
            self.quiet_tools.insert(tool_use_id.clone());
        }
        // Asked twice (a replay after a gap): the later asking stands.
        self.asked.insert(
            id.to_owned(),
            Asked {
                tool_use_id,
                input,
                suggestions,
                sort,
            },
        );
        step.events.push(TurnEvent::Question {
            id: id.to_owned(),
            kind,
        });
    }

    fn answer(&mut self, id: &str, answer: Answer) -> Result<Value, TurnError> {
        let asked = self.asked.get(id).ok_or(TurnError::UnknownQuestion)?;
        let allow = |input: Value| json!({ "behavior": "allow", "updatedInput": input });
        let deny = |message: Option<String>, fallback: &str| {
            let message = message
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| fallback.to_owned());
            json!({ "behavior": "deny", "message": message })
        };
        match (asked.sort, answer) {
            (Sort::Permission, Answer::Permission { choice_id, reason }) => {
                if choice_id == CHOICE_ONCE {
                    return Ok(allow(asked.input.clone()));
                }
                if choice_id == CHOICE_DENY {
                    return Ok(deny(reason, DENIED));
                }
                let suggestion = choice_id
                    .strip_prefix(CHOICE_ALWAYS)
                    .and_then(|at| at.parse::<usize>().ok())
                    .and_then(|at| asked.suggestions.get(at))
                    .ok_or(TurnError::BadAnswer)?;
                let mut response = allow(asked.input.clone());
                response["updatedPermissions"] = json!([suggestion]);
                Ok(response)
            }
            (Sort::Choices, Answer::Choices { answers }) => {
                let mut picked = Map::new();
                for answer in answers {
                    let mut parts = answer.labels;
                    parts.extend(
                        answer
                            .other
                            .map(|text| text.trim().to_owned())
                            .filter(|text| !text.is_empty()),
                    );
                    picked.insert(answer.question_id, Value::String(parts.join(", ")));
                }
                let mut input = asked.input.clone();
                if !input.is_object() {
                    input = json!({});
                }
                input["answers"] = Value::Object(picked);
                Ok(allow(input))
            }
            (Sort::Plan, Answer::Plan { approve, feedback }) => {
                if approve {
                    Ok(allow(asked.input.clone()))
                } else {
                    Ok(deny(feedback, KEEP_PLANNING))
                }
            }
            // A refusal is an answer to anything.
            (_, Answer::Permission { choice_id, reason }) if choice_id == CHOICE_DENY => {
                Ok(deny(reason, DENIED))
            }
            _ => Err(TurnError::BadAnswer),
        }
    }
}

const CHOICE_ONCE: &str = "once";
const CHOICE_DENY: &str = "deny";
const CHOICE_ALWAYS: &str = "always.";

fn choice_question(question: &Value) -> ChoiceQuestion {
    let text = text_of(&question["question"]);
    ChoiceQuestion {
        // The agent reads the answers back by the question's own words.
        id: text.clone(),
        header: question["header"]
            .as_str()
            .filter(|header| !header.is_empty())
            .map(str::to_owned),
        question: text,
        options: blocks(&question["options"])
            .iter()
            .map(|option| ChoiceOption {
                label: text_of(&option["label"]),
                description: option["description"]
                    .as_str()
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned),
            })
            .collect(),
        multi_select: question["multiSelect"].as_bool() == Some(true),
        allow_other: true,
    }
}

fn permission_choices(suggestions: &[Value]) -> Vec<PermissionChoice> {
    let mut choices = vec![PermissionChoice {
        id: CHOICE_ONCE.to_owned(),
        effect: PermissionEffect::AllowOnce,
        label: "Allow once".to_owned(),
        scope: None,
        takes_reason: false,
    }];
    for (at, suggestion) in suggestions.iter().enumerate() {
        choices.push(PermissionChoice {
            id: format!("{CHOICE_ALWAYS}{at}"),
            effect: PermissionEffect::AllowAlways,
            label: suggestion_label(suggestion),
            scope: scope(suggestion["destination"].as_str()),
            takes_reason: false,
        });
    }
    choices.push(PermissionChoice {
        id: CHOICE_DENY.to_owned(),
        effect: PermissionEffect::Deny,
        label: "Deny".to_owned(),
        scope: None,
        takes_reason: true,
    });
    choices
}

fn suggestion_label(suggestion: &Value) -> String {
    match suggestion["type"].as_str() {
        Some("addRules") => {
            let rules: Vec<String> = blocks(&suggestion["rules"])
                .iter()
                .map(|rule| {
                    let tool = text_of(&rule["toolName"]);
                    match rule["ruleContent"].as_str().filter(|text| !text.is_empty()) {
                        Some(content) => format!("{tool}({content})"),
                        None => tool,
                    }
                })
                .collect();
            format!("Always allow {}", rules.join(", "))
        }
        Some("setMode") => format!("Allow, and switch to {}", text_of(&suggestion["mode"])),
        Some("addDirectories") => {
            let dirs: Vec<String> = blocks(&suggestion["directories"])
                .iter()
                .map(text_of)
                .collect();
            format!("Allow, and let it work in {}", dirs.join(", "))
        }
        _ => "Allow, with the rule it suggests".to_owned(),
    }
}

fn scope(destination: Option<&str>) -> Option<String> {
    let words = match destination? {
        "session" => "this session",
        "localSettings" => "this project, on that computer",
        "projectSettings" => "this project, for everybody",
        "userSettings" => "every project",
        other => other,
    };
    Some(words.to_owned())
}

impl AgentSession for ClaudeSession {
    fn first(&mut self, turn: &TurnSpec<'_>) -> Vec<Outgoing> {
        vec![
            Outgoing {
                tag: TAG_HELLO.to_owned(),
                line: hello("leon-init"),
            },
            Outgoing {
                tag: TAG_PROMPT.to_owned(),
                line: user_message(turn.prompt),
            },
        ]
    }

    fn line(&mut self, value: &Value, step: &mut Step) {
        match value["type"].as_str() {
            Some("control_request") => {
                let id = text_of(&value["request_id"]);
                let request = &value["request"];
                if request["subtype"] == "can_use_tool" {
                    self.ask(&id, request, step);
                } else {
                    // Hooks, MCP messages: this client serves none. Saying
                    // so at once keeps the agent from waiting for nobody.
                    step.send.push(Outgoing {
                        tag: id_tag('r', &id),
                        line: control_refusal(&id, "not supported by this client"),
                    });
                }
                return;
            }
            Some("control_cancel_request") => {
                let id = text_of(&value["request_id"]);
                self.resolve(&id, step);
                return;
            }
            Some("control_response") => return,
            _ => {}
        }
        if !value["parent_tool_use_id"].is_null() {
            return;
        }
        match value["type"].as_str() {
            Some("system") => {
                if matches!(value["subtype"].as_str(), Some("init" | "status")) {
                    let mode = value["permissionMode"].as_str().map(str::to_owned);
                    self.mode_is(mode.as_deref(), step);
                }
            }
            Some("assistant") => {
                for block in blocks(&value["message"]["content"]) {
                    let name = block["name"].as_str().unwrap_or_default();
                    if block["type"] == "tool_use"
                        && matches!(name, "AskUserQuestion" | "ExitPlanMode")
                    {
                        self.quiet_tools.insert(text_of(&block["id"]));
                    }
                }
            }
            Some("user") => {
                // A tool's result: whatever was asked about it is settled.
                for block in blocks(&value["message"]["content"]) {
                    if block["type"] != "tool_result" {
                        continue;
                    }
                    let tool_use_id = text_of(&block["tool_use_id"]);
                    let settled: Vec<String> = self
                        .asked
                        .iter()
                        .filter(|(_, asked)| asked.tool_use_id == tool_use_id)
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in settled {
                        self.resolve(&id, step);
                    }
                }
            }
            Some("result") => {
                let open: Vec<String> = self.asked.keys().cloned().collect();
                for id in open {
                    self.resolve(&id, step);
                }
                self.idle = value["queued_turn_count"].as_u64().unwrap_or(0) == 0;
            }
            _ => {}
        }
        let mut events = Vec::new();
        self.reader.value(value, &mut events);
        for mut event in events {
            match &mut event {
                TurnEvent::ToolUse { id, .. } | TurnEvent::ToolResult { id, .. }
                    if self.quiet_tools.contains(id) =>
                {
                    continue;
                }
                TurnEvent::Finished {
                    ok,
                    error,
                    interrupted,
                    ..
                } if !*ok => {
                    let aborted = value["terminal_reason"]
                        .as_str()
                        .is_some_and(|reason| reason.starts_with("aborted"));
                    if aborted || self.interrupting {
                        *interrupted = true;
                        *error = None;
                    }
                    self.interrupting = false;
                }
                TurnEvent::Finished { .. } => self.interrupting = false,
                _ => {}
            }
            step.events.push(event);
        }
    }

    fn noise(&mut self, line: &str) {
        self.reader.noise(line);
    }

    fn delivered(&mut self, tag: &str, step: &mut Step) {
        if let Some(id) = tag_id('a', tag) {
            self.resolve(&id, step);
        } else if tag == TAG_PROMPT || tag.starts_with("u.") {
            // It has more to answer: its input stays open.
            self.idle = false;
        } else if tag.starts_with("i.") {
            self.interrupting = true;
        }
    }

    fn encode(&mut self, action: Action) -> Result<Vec<Outgoing>, TurnError> {
        let outgoing = match action {
            Action::Prompt(text) => {
                self.idle = false;
                Outgoing {
                    tag: format!("u.{}", self.next_id("say")),
                    line: user_message(&text),
                }
            }
            Action::Answer(id, answer) => Outgoing {
                tag: id_tag('a', &id),
                line: control_answer(&id, self.answer(&id, answer)?),
            },
            Action::Interrupt => {
                let id = self.next_id("stop");
                Outgoing {
                    tag: format!("i.{id}"),
                    line: control_request(&id, json!({ "subtype": "interrupt" })),
                }
            }
            Action::SetMode(mode) => {
                let id = self.next_id("mode");
                Outgoing {
                    tag: format!("m.{id}"),
                    line: control_request(
                        &id,
                        json!({ "subtype": "set_permission_mode", "mode": mode }),
                    ),
                }
            }
            Action::SetModel(model) => {
                let id = self.next_id("model");
                Outgoing {
                    tag: format!("m.{id}"),
                    line: control_request(&id, json!({ "subtype": "set_model", "model": model })),
                }
            }
        };
        Ok(vec![outgoing])
    }

    fn idle(&self) -> bool {
        self.idle && self.asked.is_empty()
    }

    fn exited(&mut self, code: u32) -> Vec<TurnEvent> {
        let mut events: Vec<TurnEvent> = std::mem::take(&mut self.asked)
            .into_keys()
            .map(|id| TurnEvent::QuestionResolved { id })
            .collect();
        events.extend(self.reader.exited(code));
        events
    }
}

/// A permission mode of the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMode {
    /// What a turn is started with, and what changing it takes.
    pub id: String,
    /// Words for it.
    pub label: String,
    /// What it means, in a sentence.
    pub description: Option<String>,
    /// It lets the agent do anything without asking: confirm before using.
    pub dangerous: bool,
}

/// A model the agent can answer with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentModel {
    /// What a turn is started with, and what changing it takes.
    pub id: String,
    /// Its name.
    pub label: String,
    /// What it is good for, when the agent says.
    pub description: Option<String>,
    /// The efforts it can be asked for, the lightest first; none when it has
    /// no such setting.
    pub efforts: Vec<String>,
    /// The model this entry stands for, where the id is a name of the
    /// agent's for it (`opus` for `claude-opus-5-5`): what a turn says it
    /// answers with.
    pub resolves_to: Option<String>,
}

/// Something of the agent's own that a prompt can ask for by name: a
/// command of its, a skill, a prompt somebody saved for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    /// Its name, without what a person types in front of it.
    pub name: String,
    /// What it does, when the agent says.
    pub description: Option<String>,
    /// What may follow it, when the agent says.
    pub hint: Option<String>,
    /// What a prompt starts with to ask for it: `/compact`, `$skill`.
    pub insert: String,
}

impl AgentCommand {
    /// A command asked for the way most agents take one: a slash, its name.
    pub fn slash(name: &str, description: Option<&str>, hint: Option<&str>) -> Self {
        let filled = |text: Option<&str>| {
            text.map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        Self {
            name: name.to_owned(),
            description: filled(description),
            hint: filled(hint),
            insert: format!("/{name}"),
        }
    }
}

/// What an agent offers on the other computer, as installed there.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentOptions {
    /// Its version, when it says.
    pub version: Option<String>,
    /// Its permission modes. A turn started with none uses the one that
    /// computer is configured for.
    pub modes: Vec<AgentMode>,
    /// The mode it starts in when none is asked for, when known.
    pub default_mode: Option<String>,
    /// Its models. A turn started with none uses that computer's default.
    pub models: Vec<AgentModel>,
    /// The model a turn answers with when none is asked for, when known: an
    /// id of [`Self::models`] where it is one of them.
    pub default_model: Option<String>,
    /// The effort a turn thinks with when none is asked for, when known.
    pub default_effort: Option<String>,
    /// What a prompt can ask it for by name, as it lists them.
    pub commands: Vec<AgentCommand>,
    /// It asks before acting and waits for an answer.
    pub questions: bool,
    /// It can be told more while it works and
    /// interrupted.
    pub follow_ups: bool,
    /// Its mode and model can be changed while a turn runs.
    pub live_changes: bool,
    /// The lists are this library's own, because the agent on that computer
    /// could not be asked.
    pub assumed: bool,
}

/// The command that asks Claude Code what it offers: its version, the modes
/// its help lists, and the answers to the requests it is given. `$1` is the
/// program and what follows are the requests, one a line.
pub const OPTIONS_SCRIPT: &str = r#"program=$1; shift
printf 'leon-version:%s\n' "$("$program" --version 2>/dev/null | head -n 1)"
"$program" --help 2>/dev/null | tr -s ' \n' ' ' | sed -n 's/.*--permission-mode <mode>[^(]*(choices: \([^)]*\)).*/leon-modes:\1/p'
echo
printf '%s\n' "$@" | "$program" -p --input-format stream-json --output-format stream-json --verbose --no-session-persistence 2>/dev/null
exit 0"#;

/// What [`OPTIONS_SCRIPT`] is given after the program: the opening request,
/// whose answer lists what it offers, and the request for its settings,
/// whose answer says what a turn would run with (`applied`: the model and
/// the effort, after every file and default was weighed).
pub fn options_arguments() -> Vec<String> {
    vec![
        hello("leon-options"),
        control_request("leon-settings", json!({ "subtype": "get_settings" })),
    ]
}

/// Reads the output of [`OPTIONS_SCRIPT`].
pub fn parse_options(text: &str) -> AgentOptions {
    let mut options = AgentOptions {
        questions: true,
        follow_ups: true,
        live_changes: true,
        ..AgentOptions::default()
    };
    // The model a turn would run with, and the one the settings name.
    let (mut applied, mut chosen) = (None::<String>, None::<String>);
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(version) = line.strip_prefix("leon-version:") {
            options.version = Some(version.trim().to_owned()).filter(|text| !text.is_empty());
        } else if let Some(modes) = line.strip_prefix("leon-modes:") {
            options.modes = modes
                .split(',')
                .map(|mode| mode.trim().trim_matches('"'))
                .filter(|mode| !mode.is_empty())
                .map(mode)
                .collect();
        } else if let Some(value) = crate::events::object_on(line) {
            if value["type"] != "control_response" {
                continue;
            }
            let answer = &value["response"]["response"];
            // The answer about its settings: what a turn would run with.
            if answer["applied"].is_object() {
                applied = answer["applied"]["model"].as_str().map(str::to_owned);
                chosen = answer["effective"]["model"].as_str().map(str::to_owned);
                options.default_effort = answer["applied"]["effort"]
                    .as_str()
                    .filter(|effort| !effort.is_empty())
                    .map(str::to_owned);
                continue;
            }
            if !answer["models"].is_array() {
                continue;
            }
            options.default_mode = answer["current_permission_mode"]
                .as_str()
                .map(str::to_owned);
            options.models = blocks(&answer["models"])
                .iter()
                .filter_map(|model| {
                    let id = model["value"].as_str()?;
                    Some(AgentModel {
                        id: id.to_owned(),
                        label: model["displayName"].as_str().unwrap_or(id).to_owned(),
                        description: model["description"]
                            .as_str()
                            .filter(|text| !text.is_empty())
                            .map(str::to_owned),
                        efforts: blocks(&model["supportedEffortLevels"])
                            .iter()
                            .map(text_of)
                            .collect(),
                        resolves_to: model["resolvedModel"]
                            .as_str()
                            .filter(|name| !name.is_empty())
                            .map(str::to_owned),
                    })
                })
                .collect();
            options.commands = blocks(&answer["commands"])
                .iter()
                .filter_map(|command| {
                    let name = command["name"].as_str().filter(|name| !name.is_empty())?;
                    Some(AgentCommand::slash(
                        name,
                        command["description"].as_str(),
                        command["argumentHint"].as_str(),
                    ))
                })
                .collect();
        }
    }
    options.default_model = applied.map(|applied| {
        // The entry a person would know it by: the one their settings name,
        // else the first that is that model and not "whatever is default".
        let stands_for = |model: &&AgentModel| {
            model.resolves_to.as_deref() == Some(applied.as_str()) || model.id == applied
        };
        options
            .models
            .iter()
            .filter(stands_for)
            .find(|model| Some(&model.id) == chosen.as_ref())
            .or_else(|| {
                options
                    .models
                    .iter()
                    .filter(stands_for)
                    .find(|model| model.id != "default")
            })
            .or_else(|| options.models.iter().find(stands_for))
            .map_or(applied.clone(), |model| model.id.clone())
    });
    if options.modes.is_empty() {
        options.assumed = true;
        options.modes = ["default", "acceptEdits", "plan", "bypassPermissions"]
            .into_iter()
            .map(mode)
            .collect();
    }
    options
}

fn mode(id: &str) -> AgentMode {
    let (label, description) = match id {
        "default" | "manual" => ("Ask first", "It asks before it edits or runs anything."),
        "acceptEdits" => (
            "Accept edits",
            "It edits files without asking, and asks before running commands.",
        ),
        "plan" => ("Plan", "It reads and plans, and asks before it starts."),
        "auto" => (
            "Auto",
            "It decides by itself what is safe to do without asking.",
        ),
        "dontAsk" => (
            "Don't ask",
            "It never asks: what its rules do not already allow is refused.",
        ),
        "bypassPermissions" => (
            "Bypass permissions",
            "It does anything you could do on that computer, without asking.",
        ),
        _ => (id, ""),
    };
    AgentMode {
        id: id.to_owned(),
        label: label.to_owned(),
        description: Some(description.to_owned()).filter(|text| !text.is_empty()),
        dangerous: id == "bypassPermissions",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ChoiceAnswer;

    /// What Claude Code 2.1.293 printed and read in real runs, cut down to
    /// the fields that matter.
    const ASK_PERMISSION: &str = r#"{"type":"control_request","request_id":"6429c3de-45c4-4b10-9a4f-a835d774e70c","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"touch probe.txt","description":"Create empty file probe.txt"},"description":"Create empty file probe.txt","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"touch probe.txt"}],"behavior":"allow","destination":"localSettings"},{"type":"addDirectories","directories":["/work"],"destination":"session"},{"type":"setMode","mode":"acceptEdits","destination":"session"}],"blocked_path":"/work/probe.txt","tool_use_id":"toolu_01HJ"}}"#;
    const ASK_CHOICE: &str = r#"{"type":"control_request","request_id":"a44b447b","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","display_name":"AskUserQuestion","input":{"questions":[{"question":"Which color do you prefer?","header":"Color","options":[{"label":"Red","description":"Choose red"},{"label":"Blue","description":"Choose blue"}],"multiSelect":false}]},"tool_use_id":"toolu_01Es","requires_user_interaction":true}}"#;
    const ASK_PLAN: &str = r#"{"type":"control_request","request_id":"9ad332e1","request":{"subtype":"can_use_tool","tool_name":"ExitPlanMode","display_name":"ExitPlanMode","input":{"plan":"1. Create hello.txt\n2. Verify it\n","planFilePath":"/home/ana/.claude/plans/p.md"},"tool_use_id":"toolu_015L","requires_user_interaction":true}}"#;
    const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"/work","session_id":"16da0ca3","model":"claude-haiku-5-5","permissionMode":"default","claude_code_version":"2.1.293"}"#;
    const RESULT: &str = r#"{"duration_api_ms":1896,"stop_reason":"end_turn","session_id":"16da0ca3","total_cost_usd":0.00989445,"permission_denials":[],"terminal_reason":"completed","is_error":false,"num_turns":2,"subtype":"success","result":"done","type":"result","duration_ms":2602,"queued_turn_count":0}"#;
    const INTERRUPTED: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"16da0ca3","total_cost_usd":0.005,"terminal_reason":"aborted_tools","queued_turn_count":0,"permission_denials":[]}"#;

    fn fresh() -> Box<dyn AgentSession> {
        session(100)
    }

    fn read(session: &mut dyn AgentSession, line: &str) -> Step {
        let mut step = Step::default();
        session.line(&serde_json::from_str(line).unwrap(), &mut step);
        step
    }

    fn sent(outgoing: &[Outgoing]) -> Value {
        assert_eq!(outgoing.len(), 1);
        serde_json::from_str(&outgoing[0].line).unwrap()
    }

    #[test]
    fn a_turn_speaks_the_control_protocol_with_the_options_asked_for() {
        let dirs = ["/srv/shared".to_owned()];
        let line = argv(&TurnSpec {
            program: "claude",
            prompt: "never an argument",
            resume: Some("abc"),
            mode: Some("plan"),
            model: Some("opus"),
            effort: Some("high"),
            add_dirs: &dirs,
            cwd: None,
            mcp_servers: &[],
            instructions: None,
        });
        assert_eq!(
            line,
            [
                "claude",
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--permission-prompt-tool",
                "stdio",
                "--resume",
                "abc",
                "--permission-mode",
                "plan",
                "--model",
                "opus",
                "--effort",
                "high",
                "--add-dir",
                "/srv/shared"
            ]
        );
        let plain = argv(&TurnSpec {
            program: "claude",
            ..TurnSpec::default()
        });
        assert_eq!(plain.len(), 10);
    }

    #[test]
    fn servers_of_tools_and_instructions_are_given_on_the_command_line() {
        let servers = [crate::agents::McpServer {
            name: "leon-king".into(),
            command: "/bin/leon".into(),
            args: vec!["king".into(), "mcp".into()],
            env: vec![("LEON_DATA_DIR".into(), "/data".into())],
        }];
        let line = argv(&TurnSpec {
            program: "claude",
            mcp_servers: &servers,
            instructions: Some("You coordinate."),
            ..TurnSpec::default()
        });
        let at = line.iter().position(|arg| arg == "--mcp-config").unwrap();
        let config: Value = serde_json::from_str(&line[at + 1]).unwrap();
        assert_eq!(
            config,
            json!({ "mcpServers": { "leon-king": {
                "command": "/bin/leon",
                "args": ["king", "mcp"],
                "env": { "LEON_DATA_DIR": "/data" },
            } } })
        );
        assert_eq!(
            line[line.len() - 2..],
            ["--append-system-prompt", "You coordinate."]
        );
    }

    #[test]
    fn it_is_greeted_then_given_the_prompt_as_a_message() {
        let mut session = fresh();
        let nasty = "--help; rm -rf ~ \"$(id)\"\n'second line'";
        let first = session.first(&TurnSpec {
            prompt: nasty,
            ..TurnSpec::default()
        });
        assert_eq!(first.len(), 2);
        let hello: Value = serde_json::from_str(&first[0].line).unwrap();
        assert_eq!(hello["request"]["subtype"], "initialize");
        let prompt: Value = serde_json::from_str(&first[1].line).unwrap();
        assert_eq!(prompt["type"], "user");
        assert_eq!(prompt["message"]["content"][0]["text"], nasty);
        assert!(!first[1].line.contains('\n'));
    }

    #[test]
    fn a_tool_it_may_not_use_by_itself_is_a_question_with_the_agents_own_choices() {
        let mut session = fresh();
        let step = read(session.as_mut(), ASK_PERMISSION);
        let [TurnEvent::Question { id, kind }] = &step.events[..] else {
            panic!("{:?}", step.events);
        };
        assert_eq!(id, "6429c3de-45c4-4b10-9a4f-a835d774e70c");
        let QuestionKind::Permission {
            tool,
            summary,
            detail,
            reason,
            choices,
        } = kind
        else {
            panic!("{kind:?}");
        };
        assert_eq!(tool, "Bash");
        assert_eq!(summary, "touch probe.txt");
        assert!(detail.contains("\"command\": \"touch probe.txt\""));
        assert_eq!(reason.as_deref(), Some("Create empty file probe.txt"));
        let said: Vec<(&str, PermissionEffect, &str, Option<&str>)> = choices
            .iter()
            .map(|c| {
                (
                    c.id.as_str(),
                    c.effect,
                    c.label.as_str(),
                    c.scope.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            said,
            [
                ("once", PermissionEffect::AllowOnce, "Allow once", None),
                (
                    "always.0",
                    PermissionEffect::AllowAlways,
                    "Always allow Bash(touch probe.txt)",
                    Some("this project, on that computer")
                ),
                (
                    "always.1",
                    PermissionEffect::AllowAlways,
                    "Allow, and let it work in /work",
                    Some("this session")
                ),
                (
                    "always.2",
                    PermissionEffect::AllowAlways,
                    "Allow, and switch to acceptEdits",
                    Some("this session")
                ),
                ("deny", PermissionEffect::Deny, "Deny", None),
            ]
        );
        assert!(!session.idle());
    }

    #[test]
    fn allowing_hands_the_input_back_and_always_adds_the_rule() {
        let mut session = fresh();
        read(session.as_mut(), ASK_PERMISSION);
        let id = "6429c3de-45c4-4b10-9a4f-a835d774e70c";
        let once = session
            .encode(Action::Answer(
                id.into(),
                Answer::Permission {
                    choice_id: "once".into(),
                    reason: None,
                },
            ))
            .unwrap();
        assert_eq!(once[0].tag, format!("a.{id}"));
        let value = sent(&once);
        assert_eq!(value["type"], "control_response");
        assert_eq!(value["response"]["subtype"], "success");
        assert_eq!(value["response"]["request_id"], id);
        assert_eq!(value["response"]["response"]["behavior"], "allow");
        assert_eq!(
            value["response"]["response"]["updatedInput"]["command"],
            "touch probe.txt"
        );
        assert!(value["response"]["response"]["updatedPermissions"].is_null());

        let always = session
            .encode(Action::Answer(
                id.into(),
                Answer::Permission {
                    choice_id: "always.2".into(),
                    reason: None,
                },
            ))
            .unwrap();
        let value = sent(&always);
        assert_eq!(
            value["response"]["response"]["updatedPermissions"],
            json!([{ "type": "setMode", "mode": "acceptEdits", "destination": "session" }])
        );

        let deny = session
            .encode(Action::Answer(
                id.into(),
                Answer::Permission {
                    choice_id: "deny".into(),
                    reason: Some("  not in this folder ".into()),
                },
            ))
            .unwrap();
        let value = sent(&deny);
        assert_eq!(
            value["response"]["response"],
            json!({ "behavior": "deny", "message": "not in this folder" })
        );
    }

    #[test]
    fn a_question_stays_open_until_the_agent_has_its_answer() {
        let mut session = fresh();
        read(session.as_mut(), ASK_PERMISSION);
        let id = "6429c3de-45c4-4b10-9a4f-a835d774e70c";
        let answer = session
            .encode(Action::Answer(
                id.into(),
                Answer::Permission {
                    choice_id: "once".into(),
                    reason: None,
                },
            ))
            .unwrap();
        // Written is not delivered: it can still be answered again.
        assert!(session
            .encode(Action::Answer(
                id.into(),
                Answer::Permission {
                    choice_id: "deny".into(),
                    reason: None
                }
            ))
            .is_ok());
        let mut step = Step::default();
        session.delivered(&answer[0].tag, &mut step);
        assert_eq!(step.events, [TurnEvent::QuestionResolved { id: id.into() }]);
        assert_eq!(
            session.encode(Action::Answer(
                id.into(),
                Answer::Permission {
                    choice_id: "once".into(),
                    reason: None
                }
            )),
            Err(TurnError::UnknownQuestion)
        );
    }

    #[test]
    fn a_question_is_also_closed_by_the_agent_and_by_the_turn_going_on() {
        let mut session = fresh();
        read(session.as_mut(), ASK_PERMISSION);
        let step = read(
            session.as_mut(),
            r#"{"type":"control_cancel_request","request_id":"6429c3de-45c4-4b10-9a4f-a835d774e70c"}"#,
        );
        assert_eq!(
            step.events,
            [TurnEvent::QuestionResolved {
                id: "6429c3de-45c4-4b10-9a4f-a835d774e70c".into()
            }]
        );

        read(session.as_mut(), ASK_PERMISSION);
        let step = read(
            session.as_mut(),
            r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_01HJ","type":"tool_result","content":"(Bash completed with no output)","is_error":false}]},"parent_tool_use_id":null}"#,
        );
        assert_eq!(
            step.events,
            [
                TurnEvent::QuestionResolved {
                    id: "6429c3de-45c4-4b10-9a4f-a835d774e70c".into()
                },
                TurnEvent::ToolResult {
                    id: "toolu_01HJ".into(),
                    ok: true,
                    summary: "(Bash completed with no output)".into(),
                    detail: String::new(),
                },
            ]
        );

        read(session.as_mut(), ASK_CHOICE);
        let step = read(session.as_mut(), RESULT);
        assert!(matches!(
            step.events[..],
            [
                TurnEvent::QuestionResolved { .. },
                TurnEvent::Started { .. },
                TurnEvent::Finished { ok: true, .. }
            ]
        ));
        assert!(session.idle());
    }

    #[test]
    fn its_own_question_is_answered_with_the_picks_added_to_what_it_asked() {
        let mut session = fresh();
        let step = read(session.as_mut(), ASK_CHOICE);
        let [TurnEvent::Question {
            id,
            kind: QuestionKind::Choices { questions },
        }] = &step.events[..]
        else {
            panic!("{:?}", step.events);
        };
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].id, "Which color do you prefer?");
        assert_eq!(questions[0].header.as_deref(), Some("Color"));
        assert!(!questions[0].multi_select);
        assert_eq!(questions[0].options[1].label, "Blue");
        assert_eq!(
            questions[0].options[1].description.as_deref(),
            Some("Choose blue")
        );

        let answer = session
            .encode(Action::Answer(
                id.clone(),
                Answer::Choices {
                    answers: vec![ChoiceAnswer {
                        question_id: "Which color do you prefer?".into(),
                        labels: vec!["Red".into(), "Blue".into()],
                        other: Some(" teal ".into()),
                    }],
                },
            ))
            .unwrap();
        let value = sent(&answer);
        let response = &value["response"]["response"];
        assert_eq!(response["behavior"], "allow");
        assert_eq!(
            response["updatedInput"]["answers"],
            json!({ "Which color do you prefer?": "Red, Blue, teal" })
        );
        assert_eq!(
            response["updatedInput"]["questions"][0]["header"], "Color",
            "what it asked goes back with the answers"
        );
        // A permission answer does not fit; a refusal always does.
        assert_eq!(
            session.encode(Action::Answer(
                id.clone(),
                Answer::Plan {
                    approve: true,
                    feedback: None
                }
            )),
            Err(TurnError::BadAnswer)
        );
        assert!(session
            .encode(Action::Answer(
                id.clone(),
                Answer::Permission {
                    choice_id: "deny".into(),
                    reason: None
                }
            ))
            .is_ok());
    }

    #[test]
    fn a_plan_is_approved_or_sent_back_with_what_to_change() {
        let mut session = fresh();
        let step = read(session.as_mut(), ASK_PLAN);
        let [TurnEvent::Question {
            id,
            kind: QuestionKind::Plan { plan },
        }] = &step.events[..]
        else {
            panic!("{:?}", step.events);
        };
        assert_eq!(plan, "1. Create hello.txt\n2. Verify it\n");
        let approve = session
            .encode(Action::Answer(
                id.clone(),
                Answer::Plan {
                    approve: true,
                    feedback: None,
                },
            ))
            .unwrap();
        let value = sent(&approve);
        assert_eq!(value["response"]["response"]["behavior"], "allow");
        assert_eq!(
            value["response"]["response"]["updatedInput"]["planFilePath"],
            "/home/ana/.claude/plans/p.md"
        );
        let back = session
            .encode(Action::Answer(
                id.clone(),
                Answer::Plan {
                    approve: false,
                    feedback: Some("also add a newline".into()),
                },
            ))
            .unwrap();
        assert_eq!(
            sent(&back)["response"]["response"],
            json!({ "behavior": "deny", "message": "also add a newline" })
        );
        let silent = session
            .encode(Action::Answer(
                id.clone(),
                Answer::Plan {
                    approve: false,
                    feedback: None,
                },
            ))
            .unwrap();
        assert_eq!(
            sent(&silent)["response"]["response"]["message"],
            KEEP_PLANNING
        );
    }

    #[test]
    fn a_question_in_disguise_is_not_also_shown_as_a_tool() {
        let mut session = fresh();
        let step = read(
            session.as_mut(),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_01Es","name":"AskUserQuestion","input":{"questions":[]}},{"type":"tool_use","id":"toolu_02","name":"Read","input":{"file_path":"/a"}}]},"parent_tool_use_id":null}"#,
        );
        assert_eq!(
            step.events,
            [TurnEvent::ToolUse {
                id: "toolu_02".into(),
                name: "Read".into(),
                summary: "/a".into(),
                detail: String::new(),
            }]
        );
        read(session.as_mut(), ASK_CHOICE);
        let step = read(
            session.as_mut(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"Your questions have been answered","tool_use_id":"toolu_01Es"}]},"parent_tool_use_id":null}"#,
        );
        assert_eq!(
            step.events,
            [TurnEvent::QuestionResolved {
                id: "a44b447b".into()
            }]
        );
    }

    #[test]
    fn a_request_nobody_here_serves_is_refused_at_once() {
        let mut session = fresh();
        let step = read(
            session.as_mut(),
            r#"{"type":"control_request","request_id":"hook 7","request":{"subtype":"hook_callback","callback_id":"x"}}"#,
        );
        assert!(step.events.is_empty());
        assert_eq!(step.send.len(), 1);
        assert_eq!(step.send[0].tag, id_tag('r', "hook 7"));
        let value: Value = serde_json::from_str(&step.send[0].line).unwrap();
        assert_eq!(value["response"]["subtype"], "error");
        assert_eq!(value["response"]["request_id"], "hook 7");
    }

    #[test]
    fn the_mode_is_told_when_it_is_first_heard_and_when_it_changes() {
        let mut session = fresh();
        let step = read(session.as_mut(), INIT);
        assert_eq!(
            step.events,
            [
                TurnEvent::ModeChanged {
                    mode: "default".into()
                },
                TurnEvent::Started {
                    session_id: "16da0ca3".into(),
                    model: Some("claude-haiku-5-5".into()),
                },
            ]
        );
        let status = r#"{"type":"system","subtype":"status","status":null,"permissionMode":"acceptEdits","session_id":"16da0ca3"}"#;
        assert_eq!(
            read(session.as_mut(), status).events,
            [TurnEvent::ModeChanged {
                mode: "acceptEdits".into()
            }]
        );
        assert!(read(session.as_mut(), status).events.is_empty());
        let change = session.encode(Action::SetMode("plan".into())).unwrap();
        let value = sent(&change);
        assert_eq!(value["request"]["subtype"], "set_permission_mode");
        assert_eq!(value["request"]["mode"], "plan");
        let model = session.encode(Action::SetModel("opus".into())).unwrap();
        assert_eq!(sent(&model)["request"]["model"], "opus");
        assert_ne!(change[0].tag, model[0].tag);
    }

    #[test]
    fn stopping_it_is_not_a_failure() {
        let mut session = fresh();
        let stop = session.encode(Action::Interrupt).unwrap();
        assert_eq!(sent(&stop)["request"]["subtype"], "interrupt");
        let step = read(session.as_mut(), INTERRUPTED);
        let Some(TurnEvent::Finished {
            ok,
            error,
            interrupted,
            ..
        }) = step.events.last()
        else {
            panic!("{:?}", step.events);
        };
        assert!(!ok);
        assert!(interrupted);
        assert_eq!(error, &None);
        assert!(session.idle());
    }

    #[test]
    fn it_is_idle_once_everything_it_was_told_is_answered() {
        let mut session = fresh();
        assert!(!session.idle());
        read(session.as_mut(), RESULT);
        assert!(session.idle());
        // Told more: it has work again, from the moment it is written.
        let more = session.encode(Action::Prompt("and then?".into())).unwrap();
        assert_eq!(sent(&more)["message"]["content"][0]["text"], "and then?");
        assert!(!session.idle());
        // A result that says more is queued does not end it either.
        read(
            session.as_mut(),
            r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s","queued_turn_count":1}"#,
        );
        assert!(!session.idle());
        read(session.as_mut(), RESULT);
        assert!(session.idle());
        // A replayed turn learns of the follow-up from its acknowledgement.
        let mut step = Step::default();
        session.delivered(&more[0].tag, &mut step);
        assert!(!session.idle());
    }

    #[test]
    fn its_end_closes_what_was_still_asked() {
        let mut session = fresh();
        read(session.as_mut(), ASK_PLAN);
        let events = session.exited(1);
        assert_eq!(
            events[0],
            TurnEvent::QuestionResolved {
                id: "9ad332e1".into()
            }
        );
        assert!(matches!(events[1], TurnEvent::Finished { ok: false, .. }));
        assert_eq!(events[2], TurnEvent::Exited { code: 1 });
    }

    #[test]
    fn the_commands_of_its_greeting_are_asked_for_with_a_slash() {
        let text = "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"o\",\"response\":{\"commands\":[{\"name\":\"compact\",\"description\":\"Clear the history but keep a summary\",\"argumentHint\":\"<optional instructions>\"},{\"name\":\"stripe:test-cards\",\"description\":\"\",\"argumentHint\":\"\"},{\"name\":\"\"}],\"models\":[]}}}\n";
        let options = parse_options(text);
        assert_eq!(
            options.commands,
            [
                AgentCommand {
                    name: "compact".into(),
                    description: Some("Clear the history but keep a summary".into()),
                    hint: Some("<optional instructions>".into()),
                    insert: "/compact".into(),
                },
                AgentCommand {
                    name: "stripe:test-cards".into(),
                    description: None,
                    hint: None,
                    insert: "/stripe:test-cards".into(),
                },
            ]
        );
        // An agent that could not be asked lists none.
        assert!(parse_options("command not found: claude\n")
            .commands
            .is_empty());
    }

    #[test]
    fn what_it_offers_is_read_from_its_help_and_its_greeting() {
        let text = "Welcome back\r\nleon-version:2.1.293 (Claude Code)\n\
leon-modes:\"acceptEdits\", \"auto\", \"bypassPermissions\", \"manual\", \"dontAsk\", \"plan\"\n\n\
{\"type\":\"system\",\"subtype\":\"hook_started\"}\n\
{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"o\",\"response\":{\"models\":[{\"value\":\"default\",\"resolvedModel\":\"claude-opus-5-5\",\"displayName\":\"Default (recommended)\",\"description\":\"Opus 5.5 · Best for everyday, complex tasks\",\"supportsEffort\":true,\"supportedEffortLevels\":[\"low\",\"medium\",\"high\",\"xhigh\",\"max\"]},{\"value\":\"claude-haiku-4-5-20251001\",\"displayName\":\"Haiku 4.5\",\"description\":\"Fastest for quick answers\"}],\"current_permission_mode\":\"default\"}}}\n";
        let options = parse_options(text);
        assert_eq!(options.version.as_deref(), Some("2.1.293 (Claude Code)"));
        let modes: Vec<&str> = options.modes.iter().map(|mode| mode.id.as_str()).collect();
        assert_eq!(
            modes,
            [
                "acceptEdits",
                "auto",
                "bypassPermissions",
                "manual",
                "dontAsk",
                "plan"
            ]
        );
        assert!(options.modes[2].dangerous);
        assert_eq!(options.modes[3].label, "Ask first");
        assert_eq!(options.default_mode.as_deref(), Some("default"));
        assert_eq!(options.models.len(), 2);
        assert_eq!(options.models[0].id, "default");
        assert_eq!(options.models[0].label, "Default (recommended)");
        assert_eq!(
            options.models[0].efforts,
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert!(options.models[1].efforts.is_empty());
        assert!(options.questions && options.follow_ups && options.live_changes);
        assert!(!options.assumed);
    }

    /// What Claude Code 2.1.296 answered, cut to what is read: its models,
    /// then its settings as it weighed them.
    const OFFERED: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"leon-options","response":{"models":[{"value":"default","resolvedModel":"claude-opus-5-5","displayName":"Default (recommended)","supportedEffortLevels":["low","medium","high"]},{"value":"opus","resolvedModel":"claude-opus-5-5","displayName":"Opus 5.5","supportedEffortLevels":["low","medium","high"]},{"value":"fable","resolvedModel":"claude-fable-5-1","displayName":"Fable 5.1","supportedEffortLevels":["low","medium","high"]}],"current_permission_mode":"auto"}}}"#;

    fn settings(effective: &str, applied: &str) -> String {
        format!(
            r#"{{"type":"control_response","response":{{"subtype":"success","request_id":"leon-settings","response":{{"effective":{effective},"sources":[],"applied":{applied}}}}}}}"#
        )
    }

    #[test]
    fn what_a_turn_would_run_with_is_asked_and_named_as_a_person_knows_it() {
        // Both requests are written, each on its line.
        let asked = options_arguments();
        assert_eq!(asked.len(), 2);
        assert!(asked[0].contains(r#""subtype":"initialize""#));
        assert!(asked[1].contains(r#""subtype":"get_settings""#));
        assert!(asked.iter().all(|line| !line.contains('\n')));

        // The settings name a model: that entry, not "whatever is default".
        let text = format!(
            "{OFFERED}\n{}\n",
            settings(
                r#"{"model":"opus"}"#,
                r#"{"model":"claude-opus-5-5","effort":"medium"}"#
            )
        );
        let options = parse_options(&text);
        assert_eq!(
            options.models.len(),
            3,
            "the settings are not a list of models"
        );
        assert_eq!(options.default_mode.as_deref(), Some("auto"));
        assert_eq!(options.default_model.as_deref(), Some("opus"));
        assert_eq!(options.default_effort.as_deref(), Some("medium"));
        assert_eq!(
            options.models[0].resolves_to.as_deref(),
            Some("claude-opus-5-5")
        );

        // They name none: the entry that is that model.
        let unnamed = format!(
            "{OFFERED}\n{}\n",
            settings("{}", r#"{"model":"claude-fable-5-1","effort":"high"}"#)
        );
        let options = parse_options(&unnamed);
        assert_eq!(options.default_model.as_deref(), Some("fable"));
        assert_eq!(options.default_effort.as_deref(), Some("high"));

        // A model it does not list is still what it runs with.
        let own = format!(
            "{OFFERED}\n{}\n",
            settings(
                r#"{"model":"my-model"}"#,
                r#"{"model":"my-model","effort":null}"#
            )
        );
        let options = parse_options(&own);
        assert_eq!(options.default_model.as_deref(), Some("my-model"));
        assert_eq!(options.default_effort, None);

        // An older Claude Code that does not answer for its settings.
        let options = parse_options(&format!("{OFFERED}\n"));
        assert_eq!(options.default_model, None);
        assert_eq!(options.default_effort, None);
    }

    /// The script itself, with a stand-in that says what it was written.
    #[cfg(unix)]
    #[test]
    fn the_script_writes_every_request_to_the_agent() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("claude");
        std::fs::write(
            &program,
            "#!/bin/sh\ncase \"$1\" in --version) echo '9.9 (Stand-in)';; --help) echo;; *) sed 's/^/got:/';; esac\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", OPTIONS_SCRIPT, "leon"])
            .arg(&program)
            .args(options_arguments())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("leon-version:9.9 (Stand-in)"), "{text}");
        let got: Vec<&str> = text
            .lines()
            .filter_map(|line| line.strip_prefix("got:"))
            .collect();
        assert_eq!(got, options_arguments());
    }

    #[test]
    fn an_agent_that_does_not_answer_gets_the_modes_every_version_had() {
        let options = parse_options("command not found: claude\n");
        assert!(options.assumed);
        assert!(options.models.is_empty());
        assert_eq!(options.modes.len(), 4);
        assert_eq!(options.version, None);
    }
}

//! From the bytes of a turn's terminal to the events a chat shows.
//!
//! The agent prints one JSON value per line. Around those lines a terminal
//! adds what terminals add: carriage returns, whatever a shell's start-up
//! files print, escape sequences. [`Lines`] cuts the byte stream into clean
//! lines, and a [`Reader`] turns each line that is a JSON object into zero or
//! more [`TurnEvent`]s. Anything else is
//! kept only as the tail a failed turn is explained with.
//!
//! Everything here is pure: bytes in, events out.

use std::collections::VecDeque;

use serde_json::Value;

/// A line longer than this is not an agent's event; it is dropped rather
/// than kept in memory. (A frame of the wire is 10 MiB at most.)
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// How long a one-line summary may be.
const SUMMARY_CHARS: usize = 200;
/// How long the detail of a tool's use or of its result may be.
const TOOL_DETAIL_CHARS: usize = 6000;
/// How many lines of a file being written are shown.
const WRITTEN_LINES: usize = 40;
/// How many lines that were not events are remembered, to explain a failure.
const NOISE_LINES: usize = 6;

/// What happens during a turn, as a chat shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    /// The agent's conversation exists. `session_id` is what a later turn
    /// passes as `resume_id` to continue it.
    Started {
        /// The id the agent gave the conversation.
        session_id: String,
        /// The model answering, when the agent says.
        model: Option<String>,
    },
    /// More of the message being written. Deltas accumulate into a draft
    /// that the next [`TurnEvent::Text`] replaces.
    TextDelta {
        /// The next characters.
        text: String,
    },
    /// A complete message of the agent. It replaces the draft built from the
    /// deltas before it, if any.
    Text {
        /// The whole message.
        text: String,
    },
    /// More of what the agent thinks before it answers, when it says. Not
    /// part of the answer: a chat keeps it apart, and out of sight unless
    /// asked for.
    Thought {
        /// The next characters.
        text: String,
    },
    /// The agent started using a tool.
    ToolUse {
        /// Pairs it with its [`TurnEvent::ToolResult`].
        id: String,
        /// The tool (`Bash`, `Read`, `shell`…).
        name: String,
        /// One line saying what with: the command, the file, the query.
        summary: String,
        /// What the line leaves out: the whole command, the change of a file
        /// as removed (`- `) and added (`+ `) lines under its path, or the
        /// arguments one per line. Empty when the summary says it all.
        detail: String,
    },
    /// A tool finished.
    ToolResult {
        /// The id of its [`TurnEvent::ToolUse`].
        id: String,
        /// Whether it worked.
        ok: bool,
        /// One line of what came back.
        summary: String,
        /// Everything that came back, as text: the output, the listing, the
        /// error. Empty when the summary says it all.
        detail: String,
    },
    /// The agent finished the turn. Exactly one per turn, before
    /// [`TurnEvent::Exited`].
    Finished {
        /// Whether the agent says it succeeded.
        ok: bool,
        /// What went wrong, when it did not.
        error: Option<String>,
        /// What the turn cost, when the agent says.
        cost_usd: Option<f64>,
        /// How long it took, when the agent says.
        duration_ms: Option<u64>,
        /// Tools the agent wanted and the permission mode refused.
        denied_tools: Vec<String>,
        /// The person stopped it (an interrupt); `ok` is then
        /// false without anything having gone wrong.
        interrupted: bool,
    },
    /// The tokens the turn used so far, as the agent counts them. Each one
    /// replaces the one before it in the same turn.
    Usage {
        /// The counts.
        tokens: Tokens,
    },
    /// The agent asks something and waits for an [`Answer`].
    /// Several may be open at once.
    Question {
        /// Names the question in the answer and in
        /// [`TurnEvent::QuestionResolved`].
        id: String,
        /// What is asked.
        kind: QuestionKind,
    },
    /// The question is no longer open: its answer reached the agent, or the
    /// agent withdrew it, or the turn went on without it.
    QuestionResolved {
        /// The id of its [`TurnEvent::Question`].
        id: String,
    },
    /// The agent works in another permission mode now: the person changed
    /// it, or an answer did (approving a plan, "always allow").
    ModeChanged {
        /// The mode, as the agent names it.
        mode: String,
    },
    /// The connection dropped; the turn keeps running on the other computer.
    Disconnected,
    /// The connection is back and the turn was attached again.
    Reconnected,
    /// Part of the output was missed and can no longer be had (the host
    /// keeps the last 2 MiB). What follows is the turn from somewhere in the
    /// middle.
    Gap,
    /// The agent's process ended. The last event of a turn.
    Exited {
        /// Its exit code.
        code: u32,
    },
    /// The turn no longer exists on the other computer. The last event.
    Gone,
}

/// The tokens of a turn, as the agent counts them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Read by the model, cached reads included.
    pub input: u64,
    /// Of those, read from the provider's cache (cheaper, and quicker).
    pub cached_input: u64,
    /// Written by the model, its reasoning included.
    pub output: u64,
    /// Of those, its reasoning, when the agent says.
    pub reasoning: u64,
    /// How many the model can hold at once, when the agent says.
    pub context_window: Option<u64>,
}

impl Tokens {
    /// Read and written.
    pub fn total(&self) -> u64 {
        self.input + self.output
    }
}

/// What an agent asks the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionKind {
    /// May it use this tool, this way?
    Permission {
        /// The tool (`Bash`, `Edit`…).
        tool: String,
        /// One line saying what with: the command, the file.
        summary: String,
        /// Everything it would pass to the tool, as text, for whoever wants
        /// to read it before answering.
        detail: String,
        /// Why it has to ask, when the agent says.
        reason: Option<String>,
        /// What may be answered; one of them goes back in
        /// [`Answer::Permission`].
        choices: Vec<PermissionChoice>,
    },
    /// One or more questions with options to pick from.
    Choices {
        /// The questions, to be answered together.
        questions: Vec<ChoiceQuestion>,
    },
    /// It planned and asks to start working.
    Plan {
        /// The plan, as Markdown.
        plan: String,
    },
}

/// What a way of answering a permission question does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionEffect {
    /// Allows this one use.
    AllowOnce,
    /// Allows it and changes a rule, so the like is not asked again.
    AllowAlways,
    /// Refuses it. A reason may be given with the answer.
    Deny,
}

/// One way of answering a permission question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionChoice {
    /// What goes back in [`Answer::Permission`].
    pub id: String,
    /// What it does.
    pub effect: PermissionEffect,
    /// Words for it.
    pub label: String,
    /// How far an "always" reaches (this session, this project, everywhere),
    /// when the agent says.
    pub scope: Option<String>,
    /// Whether the agent reads a reason given with it ([`Answer::Permission`]).
    /// Without it, what to do instead is said as the next message.
    pub takes_reason: bool,
}

/// One question with options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceQuestion {
    /// Names the question in [`ChoiceAnswer`].
    pub id: String,
    /// A word or two saying what it is about, when the agent gives one.
    pub header: Option<String>,
    /// The question.
    pub question: String,
    /// What can be picked.
    pub options: Vec<ChoiceOption>,
    /// Whether several may be picked.
    pub multi_select: bool,
    /// Whether an answer in the person's own words is accepted.
    pub allow_other: bool,
}

/// One thing that can be picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    /// What is picked; it goes back in [`ChoiceAnswer`].
    pub label: String,
    /// What picking it means, when the agent says.
    pub description: Option<String>,
}

/// The answer to one [`ChoiceQuestion`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceAnswer {
    /// The question's id.
    pub question_id: String,
    /// The labels picked.
    pub labels: Vec<String>,
    /// An answer in the person's own words, with or instead of labels.
    pub other: Option<String>,
}

/// What the person answers to a [`TurnEvent::Question`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// To [`QuestionKind::Permission`].
    Permission {
        /// The id of the [`PermissionChoice`] taken.
        choice_id: String,
        /// Why not, with a refusal; the agent reads it.
        reason: Option<String>,
    },
    /// To [`QuestionKind::Choices`]: one answer per question.
    Choices {
        /// The answers.
        answers: Vec<ChoiceAnswer>,
    },
    /// To [`QuestionKind::Plan`].
    Plan {
        /// Start working, or keep planning.
        approve: bool,
        /// What to change, when it should keep planning.
        feedback: Option<String>,
    },
}

/// Cuts a terminal's bytes into lines without escape sequences.
#[derive(Debug, Default)]
pub struct Lines {
    pending: Vec<u8>,
    /// The current line outgrew [`MAX_LINE_BYTES`]: skip to its end.
    skipping: bool,
}

impl Lines {
    /// Adds bytes; returns the lines they completed.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                if !self.skipping {
                    lines.push(clean(&self.pending));
                }
                self.pending.clear();
                self.skipping = false;
            } else if !self.skipping {
                self.pending.push(byte);
                if self.pending.len() > MAX_LINE_BYTES {
                    self.pending.clear();
                    self.skipping = true;
                }
            }
        }
        lines
    }

    /// Forgets a line cut in two: after a gap its beginning is lost.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.skipping = true;
    }
}

/// A line as text: no escape sequences, no control characters.
fn clean(line: &[u8]) -> String {
    let mut out = Vec::with_capacity(line.len());
    let mut at = 0;
    while at < line.len() {
        let byte = line[at];
        if byte == 0x1B {
            at = skip_escape(line, at);
        } else {
            if byte >= 0x20 || byte == b'\t' {
                out.push(byte);
            }
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The index after the escape sequence starting at `at`.
fn skip_escape(line: &[u8], at: usize) -> usize {
    match line.get(at + 1) {
        // CSI: parameters, then one final byte.
        Some(b'[') => {
            let mut end = at + 2;
            while end < line.len() && !(0x40..=0x7E).contains(&line[end]) {
                end += 1;
            }
            (end + 1).min(line.len())
        }
        // OSC, DCS, APC, PM: up to BEL or ESC \.
        Some(b']' | b'P' | b'_' | b'^') => {
            let mut end = at + 2;
            while end < line.len() {
                if line[end] == 0x07 {
                    return end + 1;
                }
                if line[end] == 0x1B && line.get(end + 1) == Some(&b'\\') {
                    return end + 2;
                }
                end += 1;
            }
            line.len()
        }
        Some(_) => at + 2,
        None => at + 1,
    }
}

/// The JSON object on a line, allowing for text a shell left before it.
pub fn object_on(line: &str) -> Option<Value> {
    let start = line.find('{')?;
    match serde_json::from_str::<Value>(&line[start..]) {
        Ok(value) if value.is_object() => Some(value),
        _ => None,
    }
}

/// Reads one turn's output and says what happened.
#[derive(Debug)]
pub(crate) struct Reader {
    /// For reading a terminal's bytes directly, which only the tests do.
    #[cfg(test)]
    lines: Lines,
    started: bool,
    finished: bool,
    /// An error the agent reported that did not end the turn by itself.
    last_error: Option<String>,
    /// The last lines that were not events.
    noise: VecDeque<String>,
}

impl Reader {
    /// A reader for one turn.
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(test)]
            lines: Lines::default(),
            started: false,
            finished: false,
            last_error: None,
            noise: VecDeque::new(),
        }
    }

    /// Terminal output, in order.
    #[cfg(test)]
    pub(crate) fn data(&mut self, bytes: &[u8]) -> Vec<TurnEvent> {
        let mut events = Vec::new();
        for line in self.lines.push(bytes) {
            self.line(&line, &mut events);
        }
        events
    }

    /// Output was lost before what comes next.
    #[cfg(test)]
    pub(crate) fn gap(&mut self) -> Vec<TurnEvent> {
        self.lines.reset();
        vec![TurnEvent::Gap]
    }

    /// The process ended with `code`. A turn whose agent never said how it
    /// went (it was not found, it crashed, its end was lost in a gap) is
    /// finished here, from the exit code and the last things printed.
    pub(crate) fn exited(&mut self, code: u32) -> Vec<TurnEvent> {
        let mut events = Vec::new();
        if !self.finished {
            self.finished = true;
            let ok = code == 0;
            let error = if ok {
                None
            } else {
                Some(
                    self.last_error
                        .take()
                        .unwrap_or_else(|| self.explanation(code)),
                )
            };
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

    fn explanation(&self, code: u32) -> String {
        if self.noise.is_empty() {
            return format!("the agent stopped with exit code {code}");
        }
        self.noise.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    /// A line that is no event: kept as what may explain a failure.
    pub(crate) fn noise(&mut self, line: &str) {
        let text = line.trim();
        if !text.is_empty() {
            if self.noise.len() == NOISE_LINES {
                self.noise.pop_front();
            }
            self.noise.push_back(clip(text, SUMMARY_CHARS));
        }
    }

    #[cfg(test)]
    fn line(&mut self, line: &str, events: &mut Vec<TurnEvent>) {
        let Some(value) = object_on(line) else {
            self.noise(line);
            return;
        };
        self.value(&value, events);
    }

    /// One value the agent printed.
    pub(crate) fn value(&mut self, value: &Value, events: &mut Vec<TurnEvent>) {
        self.claude(value, events);
    }

    fn start(&mut self, id: Option<&str>, model: Option<&str>, events: &mut Vec<TurnEvent>) {
        let Some(id) = id else { return };
        if self.started {
            return;
        }
        self.started = true;
        events.push(TurnEvent::Started {
            session_id: id.to_owned(),
            model: model.map(str::to_owned),
        });
    }

    fn claude(&mut self, value: &Value, events: &mut Vec<TurnEvent>) {
        // What a sub-agent says and does stays inside the tool that runs it.
        if !value["parent_tool_use_id"].is_null() {
            return;
        }
        match value["type"].as_str() {
            Some("system") if value["subtype"] == "init" => {
                self.start(
                    value["session_id"].as_str(),
                    value["model"].as_str(),
                    events,
                );
            }
            Some("stream_event") => {
                let event = &value["event"];
                if event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta"
                {
                    if let Some(text) = event["delta"]["text"].as_str() {
                        if !text.is_empty() {
                            events.push(TurnEvent::TextDelta {
                                text: text.to_owned(),
                            });
                        }
                    }
                }
            }
            Some("assistant") => {
                for block in blocks(&value["message"]["content"]) {
                    match block["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = block["text"].as_str() {
                                events.push(TurnEvent::Text {
                                    text: text.to_owned(),
                                });
                            }
                        }
                        Some("tool_use") => {
                            let summary = input_summary(&block["input"]);
                            events.push(TurnEvent::ToolUse {
                                id: text_of(&block["id"]),
                                name: text_of(&block["name"]),
                                detail: input_detail(&block["input"], &summary),
                                summary,
                            });
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for block in blocks(&value["message"]["content"]) {
                    if block["type"] == "tool_result" {
                        let said = content_text(&block["content"]);
                        let summary = first_line(&said);
                        events.push(TurnEvent::ToolResult {
                            id: text_of(&block["tool_use_id"]),
                            ok: block["is_error"].as_bool() != Some(true),
                            detail: text_detail(&said, &summary),
                            summary,
                        });
                    }
                }
            }
            Some("result") => {
                self.start(value["session_id"].as_str(), None, events);
                let ok = value["is_error"].as_bool() != Some(true);
                let error = if ok {
                    None
                } else {
                    let said = value["result"].as_str().filter(|text| !text.is_empty());
                    Some(
                        said.or(value["subtype"].as_str())
                            .unwrap_or("the agent reported an error")
                            .to_owned(),
                    )
                };
                let denied_tools = blocks(&value["permission_denials"])
                    .iter()
                    .filter_map(|denial| denial["tool_name"].as_str())
                    .map(str::to_owned)
                    .collect();
                self.finished = true;
                if let Some(tokens) = claude_tokens(value) {
                    events.push(TurnEvent::Usage { tokens });
                }
                events.push(TurnEvent::Finished {
                    ok,
                    error,
                    cost_usd: value["total_cost_usd"].as_f64(),
                    duration_ms: value["duration_ms"].as_u64(),
                    denied_tools,
                    interrupted: false,
                });
            }
            _ => {}
        }
    }
}

/// The tokens of a Claude Code turn, from its result's `usage` and
/// `modelUsage`.
fn claude_tokens(result: &Value) -> Option<Tokens> {
    let usage = result.get("usage").filter(|usage| usage.is_object())?;
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    let cached = count("cache_read_input_tokens");
    let context_window = result["modelUsage"].as_object().and_then(|models| {
        models
            .values()
            .filter_map(|m| m["contextWindow"].as_u64())
            .max()
    });
    Some(Tokens {
        input: count("input_tokens") + count("cache_creation_input_tokens") + cached,
        cached_input: cached,
        output: count("output_tokens"),
        reasoning: usage["output_tokens_details"]["thinking_tokens"]
            .as_u64()
            .unwrap_or(0),
        context_window,
    })
}

/// The elements of a JSON array, or none.
pub(crate) fn blocks(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

pub(crate) fn text_of(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

/// The text of a tool result: a string, or blocks of text.
pub(crate) fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// What a tool was called with, in one line: the argument a person would
/// name (the command, the file, the pattern), or the arguments as JSON.
pub(crate) fn input_summary(input: &Value) -> String {
    const TELLING: &[&str] = &[
        "command",
        "file_path",
        "filePath",
        "path",
        "notebook_path",
        "pattern",
        "url",
        "query",
        "description",
        "prompt",
    ];
    for key in TELLING {
        if let Some(text) = input[*key].as_str() {
            return first_line(text);
        }
    }
    match input {
        Value::Null => String::new(),
        Value::Object(map) if map.is_empty() => String::new(),
        other => clip(&other.to_string(), SUMMARY_CHARS),
    }
}

pub(crate) fn first_line(text: &str) -> String {
    clip(
        text.lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or(""),
        SUMMARY_CHARS,
    )
}

pub(crate) fn last_line(text: &str) -> String {
    clip(
        text.lines()
            .rfind(|line| !line.trim().is_empty())
            .unwrap_or(""),
        SUMMARY_CHARS,
    )
}

/// At most `limit` characters of `text`, with an ellipsis when cut.
pub(crate) fn clip(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// What a tool was called with beyond its `summary`: the whole command, the
/// change of a file as removed and added lines, or the arguments one per
/// line. Empty when the summary says it all.
pub(crate) fn input_detail(input: &Value, summary: &str) -> String {
    if let Some(command) = input["command"].as_str() {
        return text_detail(command, summary);
    }
    if let Some(patch) = ["patchText", "patch"]
        .iter()
        .find_map(|key| input[*key].as_str())
        .filter(|patch| patch.trim_start().starts_with("*** Begin Patch"))
    {
        return cap_detail(&diff_lines(patch).join("\n"));
    }
    let text = |keys: &[&str]| keys.iter().find_map(|key| input[*key].as_str());
    if let Some(path) = text(&["file_path", "filePath", "notebook_path", "path"]) {
        let pair = |edit: &Value| {
            let old = ["old_string", "oldString"]
                .iter()
                .find_map(|key| edit[*key].as_str())?;
            let new = ["new_string", "newString"]
                .iter()
                .find_map(|key| edit[*key].as_str())?;
            Some(changed_lines(old, new).join("\n"))
        };
        let edits: Vec<String> = match input["edits"].as_array() {
            Some(edits) => edits.iter().filter_map(pair).collect(),
            None => pair(input).into_iter().collect(),
        };
        if !edits.is_empty() {
            return cap_detail(&format!("{path}\n{}", edits.join("\n\n")));
        }
        if let Some(content) = text(&["content", "new_source"]) {
            return written_detail(path, content, '+');
        }
    }
    arguments_detail(input, summary)
}

/// `text` whole, when it says more than its `summary`.
pub(crate) fn text_detail(text: &str, summary: &str) -> String {
    let text = text.trim_matches(['\r', '\n']).trim_end();
    if text.trim_start() == summary {
        return String::new();
    }
    cap_detail(text)
}

/// A file changed from `old` to `new`: its path, then the lines that went
/// and the lines that came. The lines both begin or end with are left out.
pub(crate) fn edit_detail(path: &str, old: &str, new: &str) -> String {
    let mut lines = vec![path.to_owned()];
    lines.extend(changed_lines(old, new));
    cap_detail(&lines.join("\n"))
}

/// A file written or removed whole: its path, then its first lines, each
/// after `sign` (`+` or `-`).
pub(crate) fn written_detail(path: &str, content: &str, sign: char) -> String {
    let mut lines = vec![path.to_owned()];
    lines.extend(
        content
            .lines()
            .take(WRITTEN_LINES)
            .map(|line| format!("{sign} {line}")),
    );
    let more = content.lines().count().saturating_sub(WRITTEN_LINES);
    if more > 0 {
        lines.push(not_shown(more));
    }
    cap_detail(&lines.join("\n"))
}

/// The lines of a patch (a unified diff, or what `apply_patch` takes) with
/// the removed ones after `- `, the added ones after `+ ` and the unchanged
/// ones after two spaces. The two lines naming the file before the first
/// hunk are left out.
pub(crate) fn diff_lines(patch: &str) -> Vec<String> {
    let mut inside = false;
    let mut lines = Vec::new();
    for line in patch.lines() {
        if line.starts_with("@@") {
            inside = true;
        } else if !inside && (line.starts_with("--- ") || line.starts_with("+++ ")) {
            continue;
        }
        lines.push(match line.as_bytes().first() {
            Some(b'-' | b'+') => format!("{} {}", &line[..1], &line[1..]),
            // The lines around a change stay in line with it.
            Some(b' ') => format!(" {line}"),
            _ => line.to_owned(),
        });
    }
    lines
}

/// The lines only `old` has, after `- `, then the lines only `new` has,
/// after `+ `: what is left without the lines both begin and end with.
fn changed_lines(old: &str, new: &str) -> Vec<String> {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let shorter = old.len().min(new.len());
    let head = (0..shorter).take_while(|&at| old[at] == new[at]).count();
    let tail = (0..shorter - head)
        .take_while(|&at| old[old.len() - 1 - at] == new[new.len() - 1 - at])
        .count();
    let gone = old[head..old.len() - tail].iter();
    let come = new[head..new.len() - tail].iter();
    gone.map(|line| format!("- {line}"))
        .chain(come.map(|line| format!("+ {line}")))
        .collect()
}

/// The arguments of a tool, one `key: value` per line, when they hold more
/// than `summary` shows. Text of several lines and long values go under
/// their key, indented.
fn arguments_detail(input: &Value, summary: &str) -> String {
    let Value::Object(map) = input else {
        return match input {
            Value::Null => String::new(),
            Value::String(text) => text_detail(text, summary),
            other => text_detail(
                &serde_json::to_string_pretty(other).unwrap_or_default(),
                summary,
            ),
        };
    };
    let said = |value: &Value| value.as_str().is_some_and(|text| text.trim() == summary);
    let more = map.values().any(|value| !value.is_null() && !said(value));
    let whole = input.to_string();
    if !more || whole == summary {
        return String::new();
    }
    let mut lines = Vec::new();
    for (key, value) in map {
        let shown = match value {
            Value::Null => continue,
            Value::String(text) => text.trim_end().to_owned(),
            Value::Array(_) | Value::Object(_) => {
                let short = value.to_string();
                if short.chars().count() <= 80 {
                    short
                } else {
                    serde_json::to_string_pretty(value).unwrap_or(short)
                }
            }
            other => other.to_string(),
        };
        if shown.contains('\n') {
            lines.push(format!("{key}:"));
            lines.extend(shown.lines().map(|line| format!("  {line}")));
        } else {
            lines.push(format!("{key}: {shown}"));
        }
    }
    cap_detail(&lines.join("\n"))
}

/// The line that stands for `lines` lines left out.
fn not_shown(lines: usize) -> String {
    format!("… {lines} lines not shown …")
}

/// At most [`TOOL_DETAIL_CHARS`] characters of `text`: its head and its
/// tail, with a line between them saying how many lines are not shown.
pub(crate) fn cap_detail(text: &str) -> String {
    cap_to(text, TOOL_DETAIL_CHARS)
}

fn cap_to(text: &str, limit: usize) -> String {
    let text = text.trim_matches(['\r', '\n']).trim_end();
    let length = text.chars().count();
    if length <= limit {
        return text.to_owned();
    }
    // Room for the line between the two halves.
    let half = limit.saturating_sub(40) / 2;
    let byte_at = |count: usize| {
        text.char_indices()
            .nth(count)
            .map_or(text.len(), |(at, _)| at)
    };
    let mut head = byte_at(half);
    let mut tail = byte_at(length - half);
    // Whole lines, unless that would give up most of a half.
    let enough = |part: &str| part.chars().count() * 2 >= half;
    if let Some(end) = text[..head].rfind('\n') {
        if enough(&text[..end]) {
            head = end;
        }
    }
    if let Some(start) = text[tail..].find('\n') {
        if enough(&text[tail + start + 1..]) {
            tail += start + 1;
        }
    }
    let hidden = text[head..tail].trim_matches(['\r', '\n']);
    format!(
        "{}\n{}\n{}",
        text[..head].trim_end_matches('\r'),
        not_shown(hidden.lines().count().max(1)),
        &text[tail..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A turn of Claude Code 2.1 (`-p --output-format stream-json --verbose
    /// --include-partial-messages`), captured and shortened: hooks first,
    /// then a tool, its result, the answer in deltas and whole, the result.
    const CLAUDE: &str = r#"{"type":"system","subtype":"hook_started","hook_id":"25d5","hook_name":"SessionStart:startup","session_id":"c1156c91"}
{"type":"system","subtype":"init","cwd":"/work","session_id":"c1156c91","model":"claude-opus-5-5","permissionMode":"acceptEdits","tools":["Read"]}
{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}},"session_id":"c1156c91","parent_tool_use_id":null}
{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_01","name":"Read","input":{}}},"session_id":"c1156c91","parent_tool_use_id":null}
{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\": \"/work"}},"session_id":"c1156c91","parent_tool_use_id":null}
{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_01","name":"Read","input":{"file_path":"/work/note.txt"},"caller":{"type":"direct"}}],"stop_reason":null},"parent_tool_use_id":null,"session_id":"c1156c91"}
{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"},"session_id":"c1156c91"}
{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_01","type":"tool_result","content":"1\thello\n2\t"}]},"parent_tool_use_id":null,"session_id":"c1156c91","tool_use_result":{"type":"text"}}
{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},"session_id":"c1156c91","parent_tool_use_id":null}
{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"It"}},"session_id":"c1156c91","parent_tool_use_id":null}
{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" says \"hello\"."}},"session_id":"c1156c91","parent_tool_use_id":null}
{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_2","type":"message","role":"assistant","content":[{"type":"text","text":"It says \"hello\"."}],"stop_reason":null},"parent_tool_use_id":null,"session_id":"c1156c91"}
{"type":"result","subtype":"success","is_error":false,"duration_ms":4120,"duration_api_ms":2735,"num_turns":2,"result":"It says \"hello\".","stop_reason":"end_turn","session_id":"c1156c91","total_cost_usd":0.25,"permission_denials":[]}
"#;

    fn finished(ok: bool, error: Option<&str>) -> TurnEvent {
        TurnEvent::Finished {
            ok,
            error: error.map(str::to_owned),
            cost_usd: None,
            duration_ms: None,
            denied_tools: Vec::new(),
            interrupted: false,
        }
    }

    /// The fixture as a terminal delivers it: `\r\n`, in chunks of any size.
    fn through_a_terminal(text: &str, chunk: usize) -> Vec<TurnEvent> {
        let bytes = text.replace('\n', "\r\n").into_bytes();
        let mut reader = Reader::new();
        let mut events = Vec::new();
        for piece in bytes.chunks(chunk) {
            events.extend(reader.data(piece));
        }
        events.extend(reader.exited(0));
        events
    }

    #[test]
    fn a_claude_result_says_the_turns_tokens_before_it_ends() {
        // What Claude Code printed in a real run, cut down.
        let result = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":4613,"session_id":"db48","total_cost_usd":0.0092,"usage":{"input_tokens":4,"cache_creation_input_tokens":43587,"cache_read_input_tokens":35055,"output_tokens":416,"output_tokens_details":{"thinking_tokens":290}},"modelUsage":{"claude-haiku-5-5":{"inputTokens":4,"outputTokens":416,"contextWindow":1000000}},"permission_denials":[]}"#;
        let events = Reader::new().data(format!("{result}\n").as_bytes());
        assert_eq!(
            events[1],
            TurnEvent::Usage {
                tokens: Tokens {
                    input: 4 + 43587 + 35055,
                    cached_input: 35055,
                    output: 416,
                    reasoning: 290,
                    context_window: Some(1_000_000),
                }
            }
        );
        assert!(matches!(events[2], TurnEvent::Finished { ok: true, .. }));
    }

    #[test]
    fn a_claude_turn_reads_as_a_tool_then_an_answer() {
        let events = through_a_terminal(CLAUDE, 4096);
        assert_eq!(
            events,
            vec![
                TurnEvent::Started {
                    session_id: "c1156c91".into(),
                    model: Some("claude-opus-5-5".into()),
                },
                TurnEvent::ToolUse {
                    id: "toolu_01".into(),
                    name: "Read".into(),
                    summary: "/work/note.txt".into(),
                    detail: String::new(),
                },
                TurnEvent::ToolResult {
                    id: "toolu_01".into(),
                    ok: true,
                    summary: "1\thello".into(),
                    detail: "1\thello\n2".into(),
                },
                TurnEvent::TextDelta { text: "It".into() },
                TurnEvent::TextDelta {
                    text: " says \"hello\".".into()
                },
                TurnEvent::Text {
                    text: "It says \"hello\".".into()
                },
                TurnEvent::Finished {
                    ok: true,
                    error: None,
                    cost_usd: Some(0.25),
                    duration_ms: Some(4120),
                    denied_tools: Vec::new(),
                    interrupted: false,
                },
                TurnEvent::Exited { code: 0 },
            ]
        );
    }

    #[test]
    fn the_events_do_not_depend_on_how_the_bytes_were_cut() {
        let whole = through_a_terminal(CLAUDE, usize::MAX);
        for chunk in [1, 7, 64, 1000] {
            assert_eq!(through_a_terminal(CLAUDE, chunk), whole);
        }
        // Cutting inside a multi-byte character is no different.
        let accented = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"I’ll do it — sí\"}]},\"parent_tool_use_id\":null,\"session_id\":\"s\"}\n";
        let whole = through_a_terminal(accented, usize::MAX);
        assert!(format!("{whole:?}").contains("I’ll do it — sí"));
        assert_eq!(through_a_terminal(accented, 1), whole);
    }

    #[test]
    fn what_a_shell_prints_around_the_events_is_not_an_event() {
        let mut reader = Reader::new();
        let noisy = concat!(
            "Welcome to fish\r\n",
            "\x1b]133;A\x07\x1b[32m\x1b[0m{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"t1\"}\r\n",
            "{\"type\": not json at all\r\n",
            "[1, 2, 3]\r\n",
            "\x1b]0;title\x1b\\{\"type\":\"result\",\"is_error\":false}\r\n",
        );
        let events = reader.data(noisy.as_bytes());
        assert_eq!(
            events,
            vec![
                TurnEvent::Started {
                    session_id: "t1".into(),
                    model: None
                },
                finished(true, None),
            ]
        );
    }

    #[test]
    fn an_agent_that_is_not_installed_is_a_failed_turn_with_the_shell_s_words() {
        let mut reader = Reader::new();
        assert!(reader
            .data(b"zsh:1: command not found: claude\r\n")
            .is_empty());
        assert_eq!(
            reader.exited(127),
            vec![
                finished(false, Some("zsh:1: command not found: claude")),
                TurnEvent::Exited { code: 127 },
            ]
        );
    }

    #[test]
    fn a_silent_failure_names_its_exit_code() {
        let mut reader = Reader::new();
        assert_eq!(
            reader.exited(3)[0],
            finished(false, Some("the agent stopped with exit code 3"))
        );
    }

    #[test]
    fn claude_says_what_went_wrong_and_what_it_was_refused() {
        let mut reader = Reader::new();
        let line = concat!(
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"","#,
            r#""session_id":"s9","duration_ms":10,"permission_denials":"#,
            r#"[{"tool_name":"Bash","tool_use_id":"t","tool_input":{}}]}"#,
            "\n"
        );
        let events = reader.data(line.as_bytes());
        assert_eq!(
            events,
            vec![
                TurnEvent::Started {
                    session_id: "s9".into(),
                    model: None
                },
                TurnEvent::Finished {
                    ok: false,
                    error: Some("error_max_turns".into()),
                    cost_usd: None,
                    duration_ms: Some(10),
                    denied_tools: vec!["Bash".into()],
                    interrupted: false,
                },
            ]
        );
        // The process then exits; the turn is not finished twice.
        assert_eq!(reader.exited(1), vec![TurnEvent::Exited { code: 1 }]);
    }

    #[test]
    fn a_sub_agent_s_chatter_is_not_the_answer() {
        let mut reader = Reader::new();
        let line = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"inner"}]},"#,
            r#""parent_tool_use_id":"toolu_task"}"#,
            "\n"
        );
        assert!(reader.data(line.as_bytes()).is_empty());
    }

    #[test]
    fn after_a_gap_the_cut_line_is_dropped_and_reading_goes_on() {
        let mut reader = Reader::new();
        reader.data(b"{\"type\":\"system\",\"subtype\":\"in");
        assert_eq!(reader.gap(), vec![TurnEvent::Gap]);
        let events = reader.data(
            b"\"text\":\"tail of a lost line\"}}\n{\"type\":\"result\",\"is_error\":false}\n",
        );
        assert_eq!(events, vec![finished(true, None)]);
    }

    #[test]
    fn a_tool_s_arguments_are_summed_up_in_one_line() {
        let bash = serde_json::json!({"command": "cargo test\n  --all", "description": "tests"});
        assert_eq!(input_summary(&bash), "cargo test");
        let odd = serde_json::json!({"todos": [1, 2]});
        assert_eq!(input_summary(&odd), r#"{"todos":[1,2]}"#);
        assert_eq!(input_summary(&serde_json::json!({})), "");
        let long = serde_json::json!({"command": "x".repeat(500)});
        let summary = input_summary(&long);
        assert_eq!(summary.chars().count(), SUMMARY_CHARS);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn a_long_or_multi_line_command_is_shown_whole() {
        let short = serde_json::json!({"command": "ls", "description": "List files"});
        assert_eq!(input_detail(&short, &input_summary(&short)), "");
        let script = serde_json::json!({"command": "cargo test\n  --all\n"});
        assert_eq!(
            input_detail(&script, &input_summary(&script)),
            "cargo test\n  --all"
        );
        let long = serde_json::json!({"command": "x".repeat(500)});
        assert_eq!(input_detail(&long, &input_summary(&long)), "x".repeat(500));
    }

    #[test]
    fn an_edit_is_the_lines_that_went_and_the_lines_that_came_under_its_path() {
        let edit = serde_json::json!({
            "file_path": "/work/a.rs",
            "old_string": "fn a() {\n    one();\n}\n",
            "new_string": "fn a() {\n    two();\n    three();\n}\n",
        });
        assert_eq!(
            input_detail(&edit, "/work/a.rs"),
            "/work/a.rs\n-     one();\n+     two();\n+     three();"
        );
        // As opencode names them.
        let camel = serde_json::json!({"filePath": "a.txt", "oldString": "a", "newString": "b"});
        assert_eq!(input_detail(&camel, "a.txt"), "a.txt\n- a\n+ b");
        let several = serde_json::json!({
            "file_path": "a.txt",
            "edits": [
                {"old_string": "a", "new_string": "b"},
                {"old_string": "c\nd", "new_string": "c"},
            ],
        });
        assert_eq!(input_detail(&several, "a.txt"), "a.txt\n- a\n+ b\n\n- d");
        assert_eq!(edit_detail("a.txt", "", "new\n"), "a.txt\n+ new");
    }

    #[test]
    fn a_file_written_is_its_first_lines() {
        let write = serde_json::json!({"file_path": "/work/n.txt", "content": "a\n\nb\n"});
        assert_eq!(
            input_detail(&write, "/work/n.txt"),
            "/work/n.txt\n+ a\n+ \n+ b"
        );
        let cell = serde_json::json!({"notebook_path": "n.ipynb", "new_source": "x = 1"});
        assert_eq!(input_detail(&cell, "n.ipynb"), "n.ipynb\n+ x = 1");
        let many = (0..WRITTEN_LINES + 5)
            .map(|at| at.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let detail = written_detail("big.txt", &many, '+');
        let lines: Vec<&str> = detail.lines().collect();
        assert_eq!(lines.len(), WRITTEN_LINES + 2);
        assert_eq!(lines[WRITTEN_LINES], format!("+ {}", WRITTEN_LINES - 1));
        assert_eq!(lines[WRITTEN_LINES + 1], "… 5 lines not shown …");
    }

    #[test]
    fn a_patch_keeps_its_lines_with_the_signs_set_apart() {
        let unified = "--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n same\n-old\n+new\n--- gone";
        assert_eq!(
            diff_lines(unified),
            ["@@ -1,3 +1,3 @@", "  same", "- old", "+ new", "- -- gone"]
        );
        let patch = serde_json::json!({
            "patchText": "*** Begin Patch\n*** Update File: x\n@@\n-old\n+new\n*** End Patch",
        });
        assert_eq!(
            input_detail(&patch, ""),
            "*** Begin Patch\n*** Update File: x\n@@\n- old\n+ new\n*** End Patch"
        );
    }

    #[test]
    fn other_arguments_are_one_per_line_when_they_say_more_than_the_summary() {
        let read = serde_json::json!({"file_path": "/work/note.txt"});
        assert_eq!(input_detail(&read, "/work/note.txt"), "");
        assert_eq!(input_detail(&serde_json::json!({}), ""), "");
        assert_eq!(input_detail(&Value::Null, ""), "");
        let odd = serde_json::json!({"todos": [1, 2]});
        assert_eq!(input_detail(&odd, &input_summary(&odd)), "");
        let grep =
            serde_json::json!({"pattern": "fn main", "glob": "*.rs", "-n": true, "skip": null});
        assert_eq!(
            input_detail(&grep, "fn main"),
            "pattern: fn main\nglob: *.rs\n-n: true"
        );
        let task = serde_json::json!({
            "description": "Look",
            "prompt": "Find it.\nSay where.",
            "wide": ["x".repeat(90)],
        });
        assert_eq!(
            input_detail(&task, "Look"),
            format!(
                "description: Look\nprompt:\n  Find it.\n  Say where.\nwide:\n  [\n    \"{}\"\n  ]",
                "x".repeat(90)
            )
        );
    }

    #[test]
    fn what_a_tool_returned_is_kept_whole_unless_the_summary_says_it_all() {
        assert_eq!(text_detail("done\n", "done"), "");
        assert_eq!(text_detail("", ""), "");
        assert_eq!(text_detail("\na\nb\n\n", "a"), "a\nb");
        // The summary was cut: the whole line is the detail.
        let long = "y".repeat(SUMMARY_CHARS + 1);
        assert_eq!(text_detail(&long, &first_line(&long)), long);
    }

    #[test]
    fn a_long_detail_keeps_its_head_and_its_tail() {
        let short = "a\nb";
        assert_eq!(cap_detail(short), short);
        let exact = "é".repeat(TOOL_DETAIL_CHARS);
        assert_eq!(cap_detail(&exact), exact);

        let text = (0..2000)
            .map(|at| format!("line {at}"))
            .collect::<Vec<_>>()
            .join("\n");
        let capped = cap_detail(&text);
        assert!(capped.chars().count() <= TOOL_DETAIL_CHARS);
        let lines: Vec<&str> = capped.lines().collect();
        assert_eq!(lines[0], "line 0");
        assert_eq!(lines[lines.len() - 1], "line 1999");
        let marks: Vec<&&str> = lines.iter().filter(|line| line.starts_with('…')).collect();
        assert_eq!(marks.len(), 1);
        // Every line is either shown whole or counted.
        let hidden = 2000 - (lines.len() - 1);
        assert_eq!(*marks[0], format!("… {hidden} lines not shown …"));
        assert!(lines
            .iter()
            .all(|line| line.starts_with("line ") || line.starts_with('…')));
    }

    #[test]
    fn a_detail_of_one_endless_line_is_cut_by_characters() {
        let line = "ñ".repeat(50_000);
        let capped = cap_detail(&line);
        assert!(capped.chars().count() <= TOOL_DETAIL_CHARS);
        let lines: Vec<&str> = capped.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1], "… 1 lines not shown …");
        assert_eq!(lines[0].chars().count(), lines[2].chars().count());
        // A short line before an endless one does not cost the head.
        let mixed = format!("$ make\n{}\nexit 2", "x".repeat(50_000));
        let capped = cap_detail(&mixed);
        assert!(capped.chars().count() <= TOOL_DETAIL_CHARS);
        assert!(capped.starts_with("$ make\nxxx"));
        assert!(capped.ends_with("xxx\nexit 2"));
    }

    #[test]
    fn claude_s_tools_say_what_they_did_and_what_came_back() {
        let mut reader = Reader::new();
        let lines = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/work/a.txt","old_string":"one","new_string":"two","replace_all":false}}]},"parent_tool_use_id":null}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"tool_use_id":"t1","type":"tool_result","is_error":true,"content":[{"type":"text","text":"String to replace not found in file.\nString: one"}]}]},"parent_tool_use_id":null}"#,
            "\n",
        );
        assert_eq!(
            reader.data(lines.as_bytes()),
            vec![
                TurnEvent::ToolUse {
                    id: "t1".into(),
                    name: "Edit".into(),
                    summary: "/work/a.txt".into(),
                    detail: "/work/a.txt\n- one\n+ two".into(),
                },
                TurnEvent::ToolResult {
                    id: "t1".into(),
                    ok: false,
                    summary: "String to replace not found in file.".into(),
                    detail: "String to replace not found in file.\nString: one".into(),
                },
            ]
        );
    }

    #[test]
    fn an_endless_line_is_dropped_without_being_kept() {
        let mut lines = Lines::default();
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..9 {
            assert!(lines.push(&chunk).is_empty());
        }
        assert!(lines.pending.len() <= MAX_LINE_BYTES);
        assert_eq!(lines.push(b"\nnext\n"), vec!["next".to_owned()]);
    }

    #[test]
    fn decoding_never_panics_on_arbitrary_bytes() {
        let mut reader = Reader::new();
        let mut seed: u32 = 0x9E37_79B9;
        let mut bytes = Vec::new();
        for _ in 0..20_000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            bytes.push((seed >> 24) as u8);
        }
        reader.data(&bytes);
        reader.data(b"\x1b");
        reader.data(b"\x1b[\n\x1b]\n{\"type\":\"result\"}\n");
    }
}

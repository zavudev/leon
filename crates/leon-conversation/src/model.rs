//! A conversation as the window shows it: the events of the unified protocol
//! reduced to a list of things to draw.
//!
//! [`Conversation::apply`] takes one [`TurnEvent`] and changes the list; it
//! knows nothing of which agent sent it, which is the point: the same list is
//! drawn the same way for all of them. [`blocks`] then says how the list is
//! laid out: what the person said, what the agent answered, and between the
//! two the work it did (thoughts and tools), folded into one line once the
//! conversation has moved on.
//!
//! Pure: events in, data out.

use std::collections::VecDeque;

use leon_core::AgentId;
use leon_turn::{FileChange, QuestionKind, Todo, TodoStatus, TurnEvent};

/// One thing in a conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// What the person wrote.
    User {
        /// Their words.
        text: String,
        /// When they said it, in seconds since the epoch, when it is known.
        at: Option<i64>,
    },
    /// What the agent answered, as Markdown.
    Agent {
        /// Its words so far.
        text: String,
        /// More is coming: the text is a draft built from deltas.
        streaming: bool,
        /// When it began to answer, in seconds since the epoch, when it is
        /// known.
        at: Option<i64>,
    },
    /// What the agent thought before answering.
    Thought {
        /// Its thoughts so far.
        text: String,
    },
    /// A tool the agent used.
    Tool(Tool),
    /// The agent's list of things to do, as it stands: one card per turn,
    /// that each newer list replaces.
    Todos(Vec<Todo>),
    /// The conversation went from one agent to another here: what comes
    /// after it is the second's, which was told what was said before
    /// (`crate::handoff`).
    Switched {
        /// The agent that answered until here.
        from: AgentId,
        /// The agent that answers from here on.
        to: AgentId,
    },
    /// Something the conversation itself says: it failed, it was stopped.
    Notice {
        /// How it reads.
        tone: Tone,
        /// The words.
        text: String,
    },
}

/// How a notice reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Neither good nor bad: the person stopped the turn.
    Plain,
    /// Something went wrong.
    Bad,
}

/// One use of a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    /// Pairs the use with its result.
    pub id: String,
    /// The tool (`Bash`, `Read`, `shell`…).
    pub name: String,
    /// One line saying what with.
    pub summary: String,
    /// What the line leaves out.
    pub detail: String,
    /// The files it changes, as lines: what it was given to do, then what
    /// it did when the agent says. Empty for a tool that changes none.
    pub changes: Vec<FileChange>,
    /// How it ended; nothing while it runs.
    pub outcome: Option<Outcome>,
}

/// How a tool ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Whether it worked.
    pub ok: bool,
    /// One line of what came back.
    pub summary: String,
    /// Everything that came back.
    pub detail: String,
}

/// What the agent is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing: it waits for the person.
    Idle,
    /// A turn was started and the agent has said nothing yet.
    Starting,
    /// It is working.
    Working,
    /// It asked something and waits for the answer.
    Asking,
}

/// A question the agent waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// Names it in the answer.
    pub id: String,
    /// What is asked.
    pub kind: QuestionKind,
}

/// One conversation with an agent.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    /// Everything said and done, oldest first.
    pub items: Vec<Item>,
    /// What the agent asks and has no answer to yet.
    pub questions: Vec<Question>,
    /// The id the agent gave the conversation: what the next turn resumes.
    pub session_id: Option<String>,
    /// The model answering, when the agent says.
    pub model: Option<String>,
    /// The permission mode it works in, when it said so.
    pub mode: Option<String>,
    /// What the turns cost so far, when the agent says.
    pub cost_usd: Option<f64>,
    /// How long the last turn took, when the agent says.
    pub last_duration_ms: Option<u64>,
    /// The tokens of the last turn, when the agent says: those of the turn
    /// in flight as they grow.
    pub last_tokens: Option<leon_turn::Tokens>,
    /// How full the model's context is, as the agent last said: it outlives
    /// the turn it was said in.
    pub context: Option<Meter>,
    /// What the person wrote while a turn ran and the agent could not be
    /// told: each becomes a turn of its own, in order.
    pub queued: VecDeque<String>,
    /// Where the turn in flight began in `items`: its list of todos is the
    /// last one from here on.
    turn_from: usize,
    /// A turn is in flight.
    running: bool,
    /// The agent of the turn in flight has spoken.
    heard: bool,
    /// The last turn failed.
    pub failed: bool,
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

impl Conversation {
    /// An empty conversation.
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            questions: Vec::new(),
            session_id: None,
            model: None,
            mode: None,
            cost_usd: None,
            last_duration_ms: None,
            last_tokens: None,
            context: None,
            queued: VecDeque::new(),
            turn_from: 0,
            running: false,
            heard: false,
            failed: false,
        }
    }

    /// A conversation that continues one the agent has: `session_id` is what
    /// the first turn resumes, `earlier` what was said before, as the history
    /// kept it (`true` for the person's words) and when, when it says.
    pub fn resumed(
        session_id: String,
        earlier: impl IntoIterator<Item = (bool, String, Option<i64>)>,
    ) -> Self {
        let mut conversation = Self::new();
        conversation.session_id = Some(session_id);
        // What a program wrote in the person's name (a background task that
        // finished, a reminder) reads as a note, not as their words.
        conversation.items = earlier
            .into_iter()
            .filter(|(_, text, _)| !text.trim().is_empty())
            .flat_map(|(person, text, at)| {
                if !person {
                    return vec![Item::Agent {
                        text,
                        streaming: false,
                        at,
                    }];
                }
                let split = crate::harness::split(&text);
                // What Leon handed a new agent is the switch, not a note.
                let switched = crate::handoff::parse_marker(&text)
                    .map(|(from, to)| Item::Switched { from, to });
                let note = match switched {
                    Some(switched) => Some(switched),
                    None => split.note.map(|text| Item::Notice {
                        tone: Tone::Plain,
                        text,
                    }),
                };
                let said = (!split.said.is_empty()).then_some(Item::User {
                    text: split.said,
                    at,
                });
                note.into_iter().chain(said).collect()
            })
            .collect();
        conversation.turn_from = conversation.items.len();
        conversation
    }

    /// What the agent is doing.
    pub fn phase(&self) -> Phase {
        if !self.running {
            Phase::Idle
        } else if !self.questions.is_empty() {
            Phase::Asking
        } else if self.heard {
            Phase::Working
        } else {
            Phase::Starting
        }
    }

    /// Whether a turn is in flight.
    pub fn running(&self) -> bool {
        self.running
    }

    /// The person said something and a turn was started with it.
    #[cfg(test)]
    pub fn sent(&mut self, text: &str) {
        self.sent_at(text, None);
    }

    /// [`Conversation::sent`], at a time (seconds since the epoch).
    pub fn sent_at(&mut self, text: &str, at: Option<i64>) {
        self.settle();
        self.turn_from = self.items.len();
        self.said_at(text, at);
        self.last_tokens = None;
        self.running = true;
        self.heard = false;
        self.failed = false;
    }

    /// The person said something to the turn in flight.
    #[cfg(test)]
    pub fn said(&mut self, text: &str) {
        self.said_at(text, None);
    }

    /// [`Conversation::said`], at a time (seconds since the epoch).
    pub fn said_at(&mut self, text: &str, at: Option<i64>) {
        self.settle();
        self.items.push(Item::User {
            text: text.to_owned(),
            at,
        });
    }

    /// Takes the conversation back to before the item at `at`, which must be
    /// the person's words that began a turn: everything from there on is
    /// gone, as are the questions, the queue and the failure. The conversation
    /// continues as the session `session`, which is a new one for `None`.
    pub fn rewind(&mut self, at: usize, session: Option<String>) {
        self.items.truncate(at);
        self.questions.clear();
        self.queued.clear();
        self.session_id = session;
        self.last_tokens = None;
        self.context = None;
        self.turn_from = at.min(self.items.len());
        self.running = false;
        self.heard = false;
        self.failed = false;
    }

    /// Something the conversation says of itself, in the middle of whatever
    /// it is doing: it changes neither the turn in flight nor the questions.
    pub fn noted(&mut self, tone: Tone, text: &str) {
        self.settle();
        self.items.push(Item::Notice {
            tone,
            text: text.to_owned(),
        });
    }

    /// Something other than the person woke the agent, and a turn began
    /// with it: what it was told is a note, not the person's words.
    pub fn woke(&mut self, text: &str) {
        self.settle();
        self.turn_from = self.items.len();
        self.noted(Tone::Plain, text);
        self.last_tokens = None;
        self.running = true;
        self.heard = false;
        self.failed = false;
    }

    /// A turn could not be started at all.
    pub fn refused(&mut self, why: &str) {
        self.settle();
        self.running = false;
        self.failed = true;
        self.questions.clear();
        self.items.push(Item::Notice {
            tone: Tone::Bad,
            text: why.to_owned(),
        });
    }

    /// One event of the turn in flight.
    #[cfg(test)]
    pub fn apply(&mut self, event: TurnEvent) {
        self.apply_at(event, None);
    }

    /// [`Conversation::apply`], at a time (seconds since the epoch): what the
    /// event begins is stamped with it.
    pub fn apply_at(&mut self, event: TurnEvent, now: Option<i64>) {
        match event {
            TurnEvent::Started { session_id, model } => {
                self.heard = true;
                if !session_id.is_empty() {
                    self.session_id = Some(session_id);
                }
                if model.is_some() {
                    self.model = model;
                }
            }
            TurnEvent::TextDelta { text } => {
                self.heard = true;
                match self.items.last_mut() {
                    Some(Item::Agent {
                        text: draft,
                        streaming: true,
                        ..
                    }) => draft.push_str(&text),
                    _ => self.items.push(Item::Agent {
                        text,
                        streaming: true,
                        at: now,
                    }),
                }
            }
            TurnEvent::Text { text } => {
                self.heard = true;
                match self.items.last_mut() {
                    Some(Item::Agent {
                        text: draft,
                        streaming,
                        ..
                    }) if *streaming => {
                        *draft = text;
                        *streaming = false;
                    }
                    _ if text.trim().is_empty() => {}
                    _ => self.items.push(Item::Agent {
                        text,
                        streaming: false,
                        at: now,
                    }),
                }
            }
            TurnEvent::Thought { text } => {
                self.heard = true;
                match self.items.last_mut() {
                    Some(Item::Thought { text: thoughts }) => thoughts.push_str(&text),
                    _ => {
                        self.settle();
                        self.items.push(Item::Thought { text });
                    }
                }
            }
            TurnEvent::ToolUse {
                id,
                name,
                summary,
                detail,
            } => {
                self.heard = true;
                self.settle();
                // An agent may say a tool's use again once it knows more of it.
                match self.tool_mut(&id) {
                    Some(tool) => {
                        tool.name = name;
                        tool.summary = summary;
                        tool.detail = detail;
                    }
                    None => self.items.push(Item::Tool(Tool {
                        id,
                        name,
                        summary,
                        detail,
                        changes: Vec::new(),
                        outcome: None,
                    })),
                }
            }
            TurnEvent::ToolResult {
                id,
                ok,
                summary,
                detail,
            } => {
                if let Some(tool) = self.tool_mut(&id) {
                    tool.outcome = Some(Outcome {
                        ok,
                        summary,
                        detail,
                    });
                }
            }
            // The same tool again says the change better: it replaces.
            TurnEvent::ToolChange { id, changes } => {
                if let Some(tool) = self.tool_mut(&id) {
                    tool.changes = changes;
                }
            }
            TurnEvent::Todos { todos } => {
                self.heard = true;
                self.settle();
                let from = self.turn_from.min(self.items.len());
                let card = self.items[from..]
                    .iter()
                    .rposition(|item| matches!(item, Item::Todos(_)))
                    .map(|at| from + at);
                match (card, todos.is_empty()) {
                    (Some(at), true) => {
                        self.items.remove(at);
                    }
                    (Some(at), false) => self.items[at] = Item::Todos(todos),
                    (None, true) => {}
                    (None, false) => self.items.push(Item::Todos(todos)),
                }
            }
            TurnEvent::Question { id, kind } => {
                self.heard = true;
                self.settle();
                self.questions.retain(|question| question.id != id);
                self.questions.push(Question { id, kind });
            }
            TurnEvent::QuestionResolved { id } => {
                self.questions.retain(|question| question.id != id);
            }
            TurnEvent::ModeChanged { mode } => self.mode = Some(mode),
            TurnEvent::ModeRefused { mode, reason } => self.items.push(Item::Notice {
                tone: Tone::Bad,
                text: format!(
                    "It kept its mode instead of {mode}: {}. The next message starts in {mode}.",
                    reason.trim_end_matches('.')
                ),
            }),
            TurnEvent::Finished {
                ok,
                error,
                cost_usd,
                duration_ms,
                denied_tools: _,
                interrupted,
            } => {
                self.settle();
                self.questions.clear();
                self.last_duration_ms = duration_ms;
                if let Some(cost) = cost_usd {
                    self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost);
                }
                self.failed = !ok && !interrupted;
                if interrupted {
                    self.items.push(Item::Notice {
                        tone: Tone::Plain,
                        text: "Stopped.".to_owned(),
                    });
                } else if !ok {
                    self.items.push(Item::Notice {
                        tone: Tone::Bad,
                        text: error
                            .filter(|error| !error.trim().is_empty())
                            .unwrap_or_else(|| "The agent stopped with an error.".to_owned()),
                    });
                }
                // A tool still running when the turn ends never ended.
                for item in &mut self.items {
                    if let Item::Tool(tool) = item {
                        if tool.outcome.is_none() {
                            tool.outcome = Some(Outcome {
                                ok: false,
                                summary: "Not finished".to_owned(),
                                detail: String::new(),
                            });
                        }
                    }
                }
            }
            TurnEvent::Exited { .. } | TurnEvent::Gone => {
                self.settle();
                self.questions.clear();
                self.running = false;
            }
            TurnEvent::Usage { tokens } => {
                self.last_tokens = Some(tokens);
                if let Some(meter) = Meter::of(&tokens) {
                    self.context = Some(meter);
                }
            }
            // A pipe on this computer neither drops nor loses output.
            TurnEvent::Disconnected | TurnEvent::Reconnected | TurnEvent::Gap => {}
        }
    }

    /// A draft nothing more is coming for is a message.
    fn settle(&mut self) {
        if let Some(Item::Agent { streaming, .. }) = self.items.last_mut() {
            *streaming = false;
        }
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut Tool> {
        self.items.iter_mut().rev().find_map(|item| match item {
            Item::Tool(tool) if tool.id == id => Some(tool),
            _ => None,
        })
    }
}

/// How full a model's context is: what the conversation fills of what the
/// model can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meter {
    /// Tokens the conversation fills.
    pub used: u64,
    /// Tokens the model can hold.
    pub window: u64,
}

impl Meter {
    /// The meter an agent's count makes, when it says both what fills the
    /// context and how large it is.
    pub fn of(tokens: &leon_turn::Tokens) -> Option<Self> {
        let (used, window) = (tokens.context_used?, tokens.context_window?);
        (window > 0).then_some(Self { used, window })
    }

    /// How full, from 0 to 1.
    pub fn fraction(&self) -> f32 {
        (self.used as f64 / self.window as f64).clamp(0.0, 1.0) as f32
    }

    /// How full, as a whole percent from 0 to 100.
    pub fn percent(&self) -> u8 {
        (self.fraction() * 100.0).floor() as u8
    }

    /// How worrying it is: nothing to say, nearly full, or full.
    pub fn level(&self) -> Level {
        match self.percent() {
            0..=79 => Level::Roomy,
            80..=94 => Level::Tight,
            _ => Level::Full,
        }
    }

    /// The counts as a person reads them: "48.2k / 200k".
    pub fn label(&self) -> String {
        let short = |tokens| crate::format::count(tokens).replace(".0", "");
        format!("{} / {}", short(self.used), short(self.window))
    }
}

/// How full a context is, as it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Plenty of room.
    Roomy,
    /// Past four fifths: the agent will soon have to forget.
    Tight,
    /// Past nineteen twentieths.
    Full,
}

/// How many of the entries of a list are done, and how many there are.
pub fn todo_progress(todos: &[Todo]) -> (usize, usize) {
    let done = todos
        .iter()
        .filter(|todo| todo.status == TodoStatus::Completed)
        .count();
    (done, todos.len())
}

/// The title of a card of todos: "Todos 3/7".
pub fn todo_title(todos: &[Todo]) -> String {
    let (done, all) = todo_progress(todos);
    format!("Todos {done}/{all}")
}

/// The agent that said the item at `at`: the one that answered until the
/// first switch after it, or `current` when there is none.
pub fn agent_at(items: &[Item], at: usize, current: AgentId) -> AgentId {
    items
        .iter()
        .skip(at + 1)
        .find_map(|item| match item {
            Item::Switched { from, .. } => Some(*from),
            _ => None,
        })
        .unwrap_or(current)
}

/// How a stretch of the conversation is laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// One item by itself: the index of a message or a notice.
    Single(usize),
    /// The work between two messages: the indexes of its thoughts and tools,
    /// first to last.
    Work {
        /// The first item of the work.
        from: usize,
        /// One past its last item.
        to: usize,
        /// It is the last thing in the conversation and a turn is in flight:
        /// it is shown open, as it happens.
        live: bool,
    },
}

/// The layout of `items`. `running` is whether a turn is in flight.
pub fn blocks(items: &[Item], running: bool) -> Vec<Block> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < items.len() {
        if is_work(&items[at]) {
            let from = at;
            while at < items.len() && is_work(&items[at]) {
                at += 1;
            }
            out.push(Block::Work {
                from,
                to: at,
                live: running && at == items.len(),
            });
        } else {
            out.push(Block::Single(at));
            at += 1;
        }
    }
    out
}

fn is_work(item: &Item) -> bool {
    matches!(item, Item::Thought { .. } | Item::Tool(_))
}

/// How a stretch of work is cut for drawing: what changes files stands by
/// itself, and what does not folds together with its neighbours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Thoughts and tools that change no file: the indexes of the first and
    /// one past the last, in the stretch.
    Run {
        /// The first item of the run.
        from: usize,
        /// One past its last item.
        to: usize,
    },
    /// A tool that changes files: its index in the stretch.
    Edit(usize),
}

/// The segments of a stretch of work (`items` are thoughts and tools), in
/// order. A tool is an edit when it says the files it changes.
pub fn segments(items: &[Item]) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut run: Option<usize> = None;
    for (at, item) in items.iter().enumerate() {
        if matches!(item, Item::Tool(tool) if !tool.changes.is_empty()) {
            if let Some(from) = run.take() {
                out.push(Segment::Run { from, to: at });
            }
            out.push(Segment::Edit(at));
        } else if run.is_none() {
            run = Some(at);
        }
    }
    if let Some(from) = run {
        out.push(Segment::Run {
            from,
            to: items.len(),
        });
    }
    out
}

/// One row of a drawn diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row<'a> {
    /// Where lines between two hunks are left out.
    Gap,
    /// A line of the change.
    Line(&'a leon_turn::DiffLine),
}

/// The rows of a change, hunk after hunk, at most `cap` lines, and how many
/// lines the cap left out.
pub fn diff_rows(change: &FileChange, cap: usize) -> (Vec<Row<'_>>, usize) {
    let mut rows = Vec::new();
    let mut shown = 0;
    let mut hidden = 0;
    for (n, hunk) in change.hunks.iter().enumerate() {
        if n > 0 && shown < cap {
            rows.push(Row::Gap);
        }
        for line in &hunk.lines {
            if shown < cap {
                rows.push(Row::Line(line));
                shown += 1;
            } else {
                hidden += 1;
            }
        }
    }
    (rows, hidden)
}

/// What a folded stretch of work says of itself: how many tools, and whether
/// one failed.
pub fn work_summary(items: &[Item]) -> (usize, usize, bool) {
    let mut tools = 0;
    let mut thoughts = 0;
    let mut failed = false;
    for item in items {
        match item {
            Item::Tool(tool) => {
                tools += 1;
                failed |= tool.outcome.as_ref().is_some_and(|outcome| !outcome.ok);
            }
            Item::Thought { .. } => thoughts += 1,
            _ => {}
        }
    }
    (tools, thoughts, failed)
}

/// The words of a folded stretch of work, as a person would say what
/// happened: "Ran 2 shell commands, read 3 files", "Thought", "Changed 1 file,
/// with a failure".
pub fn work_label(items: &[Item]) -> String {
    let (_, _, failed) = work_summary(items);
    let tools: Vec<&Tool> = items
        .iter()
        .filter_map(|item| match item {
            Item::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();
    let mut label = if tools.is_empty() {
        "Thought".to_owned()
    } else {
        crate::phrase::summary(&tools)
    };
    if failed {
        label.push_str(", with a failure");
    }
    label
}

/// A duration for the foot of a turn: "0.8 s", "42 s", "3 min 05 s".
pub fn duration_text(ms: u64) -> String {
    let seconds = ms / 1000;
    if ms < 10_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else if seconds < 60 {
        format!("{seconds} s")
    } else {
        format!("{} min {:02} s", seconds / 60, seconds % 60)
    }
}

/// The tokens of the last turn for the foot of a conversation, with how
/// fast they were written when the turn's length is known: "48.2k tokens,
/// 62 tok/s".
pub fn tokens_text(tokens: &leon_turn::Tokens, duration_ms: Option<u64>) -> Option<String> {
    if tokens.total() == 0 {
        return None;
    }
    let mut text = format!("{} tokens", crate::format::count(tokens.total()));
    if let Some(speed) = crate::format::per_second(tokens.output, duration_ms.unwrap_or(0)) {
        text.push_str(&format!(", {speed:.0} tok/s"));
    }
    Some(text)
}

/// A cost for the foot of a conversation: "$0.04", "$1.20".
pub fn cost_text(usd: f64) -> String {
    if usd < 0.01 {
        "<$0.01".to_owned()
    } else {
        format!("${usd:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn finished(ok: bool, interrupted: bool) -> TurnEvent {
        TurnEvent::Finished {
            ok,
            error: (!ok && !interrupted).then(|| "it broke".to_owned()),
            cost_usd: Some(0.25),
            duration_ms: Some(1200),
            denied_tools: Vec::new(),
            interrupted,
        }
    }

    fn tool(id: &str) -> TurnEvent {
        TurnEvent::ToolUse {
            id: id.into(),
            name: "Bash".into(),
            summary: "ls".into(),
            detail: String::new(),
        }
    }

    fn tool_again(id: &str) -> TurnEvent {
        TurnEvent::ToolUse {
            id: id.into(),
            name: "Edit".into(),
            summary: "a.rs".into(),
            detail: String::new(),
        }
    }

    #[test]
    fn a_note_changes_nothing_of_the_turn_and_news_begins_one() {
        let mut conversation = Conversation::new();
        conversation.woke("Worker 1 is done.");
        assert!(conversation.running());
        assert_eq!(conversation.phase(), Phase::Starting);
        conversation.noted(Tone::Plain, "Leon allowed it.");
        assert!(conversation.running(), "a note does not end the turn");
        conversation.apply(TurnEvent::Exited { code: 0 });
        assert!(!conversation.running());
        assert_eq!(
            conversation.items,
            [
                Item::Notice {
                    tone: Tone::Plain,
                    text: "Worker 1 is done.".into()
                },
                Item::Notice {
                    tone: Tone::Plain,
                    text: "Leon allowed it.".into()
                },
            ]
        );
    }

    #[test]
    fn deltas_build_a_draft_that_the_whole_message_replaces() {
        let mut conversation = Conversation::new();
        conversation.sent("hello");
        assert_eq!(conversation.phase(), Phase::Starting);
        conversation.apply(TurnEvent::TextDelta { text: "Hi".into() });
        conversation.apply(TurnEvent::TextDelta {
            text: " the".into(),
        });
        assert_eq!(conversation.phase(), Phase::Working);
        assert_eq!(
            conversation.items[1],
            Item::Agent {
                text: "Hi the".into(),
                streaming: true,
                at: None
            }
        );
        conversation.apply(TurnEvent::Text {
            text: "Hi there.".into(),
        });
        assert_eq!(
            conversation.items[1],
            Item::Agent {
                text: "Hi there.".into(),
                streaming: false,
                at: None
            }
        );
        assert_eq!(conversation.items.len(), 2);
    }

    #[test]
    fn a_message_without_deltas_is_a_message_of_its_own() {
        let mut conversation = Conversation::new();
        conversation.sent("go");
        conversation.apply(TurnEvent::Text { text: "One".into() });
        conversation.apply(TurnEvent::Text { text: "Two".into() });
        conversation.apply(TurnEvent::Text { text: "  ".into() });
        assert_eq!(conversation.items.len(), 3);
    }

    #[test]
    fn a_tool_is_paired_with_its_result_by_its_id() {
        let mut conversation = Conversation::new();
        conversation.sent("go");
        conversation.apply(tool("a"));
        conversation.apply(tool("b"));
        conversation.apply(TurnEvent::ToolResult {
            id: "a".into(),
            ok: true,
            summary: "2 files".into(),
            detail: "x\ny".into(),
        });
        let Item::Tool(first) = &conversation.items[1] else {
            panic!()
        };
        let Item::Tool(second) = &conversation.items[2] else {
            panic!()
        };
        assert_eq!(first.outcome.as_ref().unwrap().summary, "2 files");
        assert!(second.outcome.is_none());
        // Said again with more known of it, it is still one tool.
        conversation.apply(TurnEvent::ToolUse {
            id: "b".into(),
            name: "Edit".into(),
            summary: "main.rs".into(),
            detail: "- a\n+ b".into(),
        });
        assert_eq!(conversation.items.len(), 3);
    }

    #[test]
    fn the_change_of_a_tool_is_said_twice_and_the_second_replaces_the_first() {
        let mut conversation = Conversation::new();
        conversation.sent("go");
        conversation.apply(tool("a"));
        conversation.apply(TurnEvent::ToolChange {
            id: "a".into(),
            changes: vec![FileChange::edited("a.rs", &[("x", "y")])],
        });
        conversation.apply(TurnEvent::ToolChange {
            id: "nobody".into(),
            changes: vec![FileChange::written("b.rs", "z")],
        });
        let patch = json!([{"oldStart": 4, "newStart": 4, "lines": ["-x", "+y"]}]);
        conversation.apply(TurnEvent::ToolChange {
            id: "a".into(),
            changes: vec![FileChange::from_structured_patch("a.rs", &patch).unwrap()],
        });
        assert_eq!(conversation.items.len(), 2, "a change is not an item");
        let Item::Tool(tool) = &conversation.items[1] else {
            panic!()
        };
        assert_eq!(tool.changes.len(), 1);
        assert_eq!(tool.changes[0].hunks[0].lines[1].new, Some(4));
        // The tool said again keeps what it changes.
        conversation.apply(tool_again("a"));
        let Item::Tool(tool) = &conversation.items[1] else {
            panic!()
        };
        assert_eq!(tool.changes.len(), 1);
    }

    #[test]
    fn edits_stand_by_themselves_and_the_rest_folds_between_them() {
        let mut conversation = Conversation::new();
        conversation.sent("go");
        conversation.apply(tool("a"));
        conversation.apply(TurnEvent::Thought { text: "hm".into() });
        conversation.apply(tool("b"));
        conversation.apply(tool("c"));
        conversation.apply(TurnEvent::ToolChange {
            id: "c".into(),
            changes: vec![FileChange::written("c.rs", "x")],
        });
        conversation.apply(tool("d"));
        let items = &conversation.items[1..];
        assert_eq!(
            segments(items),
            vec![
                Segment::Run { from: 0, to: 3 },
                Segment::Edit(3),
                Segment::Run { from: 4, to: 5 },
            ]
        );
        assert_eq!(segments(&items[3..4]), vec![Segment::Edit(0)]);
        assert!(segments(&[]).is_empty());
    }

    #[test]
    fn a_long_diff_shows_its_cap_and_counts_what_it_left_out() {
        let patch = json!([
            {"oldStart": 1, "newStart": 1, "lines": ["-a", "+b", " c"]},
            {"oldStart": 20, "newStart": 20, "lines": ["-d", "+e"]}
        ]);
        let change = FileChange::from_structured_patch("a.rs", &patch).unwrap();
        let (rows, hidden) = diff_rows(&change, 100);
        assert_eq!((rows.len(), hidden), (6, 0), "{rows:?}");
        assert_eq!(rows[3], Row::Gap);
        let (rows, hidden) = diff_rows(&change, 4);
        assert_eq!((rows.len(), hidden), (5, 1));
        let (rows, hidden) = diff_rows(&change, 3);
        assert_eq!((rows.len(), hidden), (3, 2), "no gap before nothing");
    }

    #[test]
    fn a_question_is_open_until_it_is_resolved_and_the_agent_then_asks() {
        let mut conversation = Conversation::new();
        conversation.sent("go");
        conversation.apply(TurnEvent::Question {
            id: "q".into(),
            kind: QuestionKind::Plan { plan: "1.".into() },
        });
        assert_eq!(conversation.phase(), Phase::Asking);
        conversation.apply(TurnEvent::QuestionResolved { id: "q".into() });
        assert_eq!(conversation.phase(), Phase::Working);
    }

    #[test]
    fn the_end_of_a_turn_adds_its_cost_and_frees_the_conversation() {
        let mut conversation = Conversation::new();
        conversation.sent("one");
        conversation.apply(tool("a"));
        conversation.apply(finished(true, false));
        assert!(conversation.running(), "the process has not ended yet");
        conversation.apply(TurnEvent::Exited { code: 0 });
        assert_eq!(conversation.phase(), Phase::Idle);
        conversation.sent("two");
        conversation.apply(finished(true, false));
        conversation.apply(TurnEvent::Exited { code: 0 });
        assert_eq!(conversation.cost_usd, Some(0.5));
        assert!(!conversation.failed);
        // The tool nobody reported the end of did not finish.
        let Item::Tool(left) = &conversation.items[1] else {
            panic!()
        };
        assert!(!left.outcome.as_ref().unwrap().ok);
    }

    #[test]
    fn a_failure_says_why_and_a_stop_is_not_a_failure() {
        let mut conversation = Conversation::new();
        conversation.sent("one");
        conversation.apply(finished(false, false));
        assert!(conversation.failed);
        assert_eq!(
            conversation.items.last(),
            Some(&Item::Notice {
                tone: Tone::Bad,
                text: "it broke".into()
            })
        );
        conversation.sent("two");
        conversation.apply(finished(false, true));
        assert!(!conversation.failed);
        assert_eq!(
            conversation.items.last(),
            Some(&Item::Notice {
                tone: Tone::Plain,
                text: "Stopped.".into()
            })
        );
    }

    #[test]
    fn a_mode_the_agent_refuses_is_said_and_keeps_its_own() {
        let mut conversation = Conversation::new();
        conversation.sent("go");
        conversation.apply(TurnEvent::ModeChanged {
            mode: "default".into(),
        });
        conversation.apply(TurnEvent::ModeRefused {
            mode: "bypassPermissions".into(),
            reason: "Not launched for it".into(),
        });
        assert_eq!(conversation.mode.as_deref(), Some("default"));
        assert_eq!(
            conversation.items.last(),
            Some(&Item::Notice {
                tone: Tone::Bad,
                text: "It kept its mode instead of bypassPermissions: Not launched for it. \
                       The next message starts in bypassPermissions."
                    .into()
            })
        );
    }

    #[test]
    fn the_work_between_two_messages_is_one_block() {
        let mut conversation = Conversation::new();
        conversation.sent("go");
        conversation.apply(TurnEvent::Thought { text: "hm".into() });
        conversation.apply(TurnEvent::Thought { text: "m".into() });
        conversation.apply(tool("a"));
        conversation.apply(tool("b"));
        assert_eq!(
            blocks(&conversation.items, true),
            [
                Block::Single(0),
                Block::Work {
                    from: 1,
                    to: 4,
                    live: true
                }
            ]
        );
        conversation.apply(TurnEvent::Text {
            text: "Done".into(),
        });
        assert_eq!(
            blocks(&conversation.items, true),
            [
                Block::Single(0),
                Block::Work {
                    from: 1,
                    to: 4,
                    live: false
                },
                Block::Single(4)
            ]
        );
        assert_eq!(
            work_label(&conversation.items[1..4]),
            "Ran 2 shell commands",
            "tools are counted by what they do"
        );
        assert_eq!(work_label(&conversation.items[1..2]), "Thought");
    }

    #[test]
    fn going_back_to_a_turn_leaves_what_came_before_it_and_the_session_given() {
        let mut conversation = Conversation::new();
        conversation.sent("one");
        conversation.apply(TurnEvent::Started {
            session_id: "s1".into(),
            model: None,
        });
        conversation.apply(TurnEvent::Text {
            text: "first".into(),
        });
        conversation.apply(finished(true, false));
        conversation.sent("two");
        conversation.apply(tool("t1"));
        conversation.apply(TurnEvent::Question {
            id: "q".into(),
            kind: QuestionKind::Plan { plan: "p".into() },
        });
        conversation.queued.push_back("three".into());
        assert_eq!(conversation.phase(), Phase::Asking);
        conversation.rewind(2, Some("s2".into()));
        assert_eq!(conversation.items.len(), 2);
        assert!(matches!(&conversation.items[0], Item::User { text, .. } if text == "one"));
        assert!(matches!(&conversation.items[1], Item::Agent { text, .. } if text == "first"));
        assert_eq!(conversation.session_id.as_deref(), Some("s2"));
        assert!(conversation.questions.is_empty() && conversation.queued.is_empty());
        assert_eq!(conversation.phase(), Phase::Idle);
        // A new conversation has no session to resume.
        conversation.rewind(0, None);
        assert!(conversation.items.is_empty());
        assert_eq!(conversation.session_id, None);
    }

    #[test]
    fn a_resumed_conversation_shows_what_was_said_and_resumes_by_its_id() {
        let conversation = Conversation::resumed(
            "abc".into(),
            [
                (true, "fix it".to_owned(), None),
                (false, String::new(), None),
                (false, "Fixed.".to_owned(), None),
            ],
        );
        assert_eq!(conversation.session_id.as_deref(), Some("abc"));
        assert_eq!(conversation.items.len(), 2);
        assert_eq!(conversation.phase(), Phase::Idle);
    }

    #[test]
    fn what_a_program_wrote_in_the_persons_name_is_read_back_as_a_note() {
        let conversation = Conversation::resumed(
            "abc".into(),
            [
                (
                    true,
                    "<task-notification><task-id>b1</task-id><summary>Tests passed</summary></task-notification>".to_owned(),
                    None,
                ),
                (
                    true,
                    "<system-reminder>x</system-reminder>\nship it".to_owned(),
                    None,
                ),
            ],
        );
        assert_eq!(
            conversation.items,
            [
                Item::Notice {
                    tone: Tone::Plain,
                    text: "task notification: Tests passed".into()
                },
                Item::Notice {
                    tone: Tone::Plain,
                    text: "system reminder".into()
                },
                Item::User {
                    text: "ship it".into(),
                    at: None
                },
            ]
        );
    }

    #[test]
    fn a_handoff_in_a_history_is_read_back_as_the_switch_it_was() {
        let handed = format!(
            "{}\nUser:\nold words\n</leon-handoff>\n\nand now this",
            crate::handoff::marker(AgentId::CLAUDE, AgentId::CODEX)
        );
        let conversation = Conversation::resumed(
            "abc".into(),
            [(true, handed, Some(5)), (false, "Done.".to_owned(), None)],
        );
        assert_eq!(
            conversation.items,
            [
                Item::Switched {
                    from: AgentId::CLAUDE,
                    to: AgentId::CODEX
                },
                Item::User {
                    text: "and now this".into(),
                    at: Some(5)
                },
                Item::Agent {
                    text: "Done.".into(),
                    streaming: false,
                    at: None
                },
            ]
        );
    }

    #[test]
    fn what_was_answered_before_a_switch_was_the_first_agents() {
        let said = |text: &str| Item::Agent {
            text: text.into(),
            streaming: false,
            at: None,
        };
        let items = [
            said("a"),
            Item::Switched {
                from: AgentId::CLAUDE,
                to: AgentId::CODEX,
            },
            said("b"),
            Item::Switched {
                from: AgentId::CODEX,
                to: AgentId::GROK,
            },
            said("c"),
        ];
        assert_eq!(agent_at(&items, 0, AgentId::GROK), AgentId::CLAUDE);
        assert_eq!(agent_at(&items, 2, AgentId::GROK), AgentId::CODEX);
        assert_eq!(agent_at(&items, 4, AgentId::GROK), AgentId::GROK);
        assert_eq!(agent_at(&[], 0, AgentId::GROK), AgentId::GROK);
    }

    fn todo(text: &str, status: TodoStatus) -> Todo {
        Todo {
            text: text.into(),
            status,
        }
    }

    fn todos(list: &[(&str, TodoStatus)]) -> TurnEvent {
        TurnEvent::Todos {
            todos: list
                .iter()
                .map(|(text, status)| todo(text, *status))
                .collect(),
        }
    }

    #[test]
    fn a_list_of_todos_is_one_card_that_later_lists_replace_within_the_turn() {
        let mut conversation = Conversation::new();
        conversation.sent("go");
        conversation.apply(todos(&[("read", TodoStatus::InProgress)]));
        conversation.apply(TurnEvent::Text { text: "ok".into() });
        conversation.apply(todos(&[
            ("read", TodoStatus::Completed),
            ("write", TodoStatus::Pending),
        ]));
        let lists: Vec<&Item> = conversation
            .items
            .iter()
            .filter(|item| matches!(item, Item::Todos(_)))
            .collect();
        assert_eq!(
            lists,
            [&Item::Todos(vec![
                todo("read", TodoStatus::Completed),
                todo("write", TodoStatus::Pending)
            ])]
        );
        // Said again in the middle of the turn, it is still the same turn.
        conversation.said("and also this");
        conversation.apply(todos(&[("write", TodoStatus::InProgress)]));
        let count = |c: &Conversation| {
            c.items
                .iter()
                .filter(|item| matches!(item, Item::Todos(_)))
                .count()
        };
        assert_eq!(count(&conversation), 1);
        // The next turn keeps its own.
        conversation.apply(finished(true, false));
        conversation.apply(TurnEvent::Exited { code: 0 });
        conversation.sent("more");
        conversation.apply(todos(&[("ship", TodoStatus::Pending)]));
        assert_eq!(count(&conversation), 2);
    }

    #[test]
    fn a_list_with_nothing_in_it_takes_the_card_away_and_never_makes_one() {
        let mut conversation = Conversation::new();
        conversation.sent("go");
        conversation.apply(todos(&[]));
        assert_eq!(conversation.items.len(), 1);
        conversation.apply(todos(&[("read", TodoStatus::Pending)]));
        assert_eq!(conversation.items.len(), 2);
        conversation.apply(todos(&[]));
        assert_eq!(conversation.items.len(), 1);
    }

    #[test]
    fn the_card_of_todos_stands_between_two_stretches_of_work() {
        let items = [
            Item::Tool(Tool {
                id: "a".into(),
                name: "Bash".into(),
                summary: String::new(),
                detail: String::new(),
                changes: Vec::new(),
                outcome: None,
            }),
            Item::Todos(vec![todo("x", TodoStatus::Pending)]),
            Item::Thought { text: "hm".into() },
        ];
        assert_eq!(
            blocks(&items, false),
            [
                Block::Work {
                    from: 0,
                    to: 1,
                    live: false
                },
                Block::Single(1),
                Block::Work {
                    from: 2,
                    to: 3,
                    live: false
                },
            ]
        );
    }

    #[test]
    fn a_list_says_how_far_it_is() {
        let list = [
            todo("a", TodoStatus::Completed),
            todo("b", TodoStatus::Completed),
            todo("c", TodoStatus::InProgress),
            todo("d", TodoStatus::Pending),
        ];
        assert_eq!(todo_progress(&list), (2, 4));
        assert_eq!(todo_title(&list), "Todos 2/4");
        assert_eq!(todo_progress(&[]), (0, 0));
    }

    #[test]
    fn what_is_said_is_stamped_with_the_time_it_is_said() {
        let mut conversation = Conversation::new();
        conversation.sent_at("go", Some(1_000));
        conversation.apply_at(TurnEvent::TextDelta { text: "Hi".into() }, Some(1_005));
        conversation.apply_at(TurnEvent::TextDelta { text: "!".into() }, Some(1_009));
        conversation.apply_at(TurnEvent::Text { text: "Hi!".into() }, Some(1_010));
        conversation.said_at("more", Some(1_020));
        assert_eq!(
            conversation.items,
            [
                Item::User {
                    text: "go".into(),
                    at: Some(1_000)
                },
                Item::Agent {
                    text: "Hi!".into(),
                    streaming: false,
                    at: Some(1_005)
                },
                Item::User {
                    text: "more".into(),
                    at: Some(1_020)
                },
            ]
        );
    }

    #[test]
    fn a_resumed_conversation_keeps_the_time_the_history_says() {
        let conversation = Conversation::resumed(
            "abc".into(),
            [
                (true, "fix it".to_owned(), Some(10)),
                (false, "Fixed.".to_owned(), None),
            ],
        );
        assert!(matches!(
            conversation.items[..],
            [
                Item::User { at: Some(10), .. },
                Item::Agent { at: None, .. }
            ]
        ));
    }

    #[test]
    fn the_context_is_what_the_agent_says_fills_it_and_outlives_the_turn() {
        let usage = |used, window| TurnEvent::Usage {
            tokens: leon_turn::Tokens {
                input: 10,
                context_window: window,
                context_used: used,
                ..leon_turn::Tokens::default()
            },
        };
        let mut conversation = Conversation::new();
        assert_eq!(conversation.context, None);
        conversation.sent("go");
        // Without its window, or without what fills it, there is no meter.
        conversation.apply(usage(Some(5_000), None));
        conversation.apply(usage(None, Some(200_000)));
        assert_eq!(conversation.context, None);
        conversation.apply(usage(Some(50_000), Some(200_000)));
        assert_eq!(
            conversation.context,
            Some(Meter {
                used: 50_000,
                window: 200_000
            })
        );
        // The next turn starts with the last count, until it has a newer one.
        conversation.apply(TurnEvent::Exited { code: 0 });
        conversation.sent("more");
        assert!(conversation.context.is_some());
        // A conversation taken back to a point no longer knows it.
        conversation.rewind(0, None);
        assert_eq!(conversation.context, None);
    }

    #[test]
    fn a_meter_reads_as_a_fraction_a_percent_and_a_pair_of_counts() {
        let meter = Meter {
            used: 48_200,
            window: 200_000,
        };
        assert_eq!(meter.percent(), 24);
        assert!((meter.fraction() - 0.241).abs() < 1e-6);
        assert_eq!(meter.label(), "48.2k / 200k");
        assert_eq!(meter.level(), Level::Roomy);
        let tight = Meter {
            used: 160_000,
            window: 200_000,
        };
        assert_eq!(tight.level(), Level::Tight);
        let full = Meter {
            used: 190_000,
            window: 200_000,
        };
        assert_eq!(full.level(), Level::Full);
        // Over what it can hold, it is full and no more.
        let over = Meter {
            used: 300,
            window: 200,
        };
        assert_eq!((over.fraction(), over.percent()), (1.0, 100));
        // A million is written in millions.
        assert_eq!(
            Meter {
                used: 900_000,
                window: 1_000_000
            }
            .label(),
            "900k / 1M"
        );
    }

    #[test]
    fn durations_and_costs_read_short() {
        assert_eq!(duration_text(800), "0.8 s");
        assert_eq!(duration_text(42_000), "42 s");
        assert_eq!(duration_text(185_000), "3 min 05 s");
        let tokens = leon_turn::Tokens {
            input: 45_100,
            cached_input: 31_000,
            output: 3_100,
            ..leon_turn::Tokens::default()
        };
        assert_eq!(
            tokens_text(&tokens, Some(50_000)).as_deref(),
            Some("48.2k tokens, 62 tok/s")
        );
        assert_eq!(tokens_text(&tokens, None).as_deref(), Some("48.2k tokens"));
        assert_eq!(tokens_text(&leon_turn::Tokens::default(), Some(1)), None);
        assert_eq!(cost_text(0.004), "<$0.01");
        assert_eq!(cost_text(1.2), "$1.20");
    }
}

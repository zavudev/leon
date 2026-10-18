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

use leon_turn::{QuestionKind, TurnEvent};

/// One thing in a conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// What the person wrote.
    User {
        /// Their words.
        text: String,
    },
    /// What the agent answered, as Markdown.
    Agent {
        /// Its words so far.
        text: String,
        /// More is coming: the text is a draft built from deltas.
        streaming: bool,
    },
    /// What the agent thought before answering.
    Thought {
        /// Its thoughts so far.
        text: String,
    },
    /// A tool the agent used.
    Tool(Tool),
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
    /// What the person wrote while a turn ran and the agent could not be
    /// told: each becomes a turn of its own, in order.
    pub queued: VecDeque<String>,
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
            queued: VecDeque::new(),
            running: false,
            heard: false,
            failed: false,
        }
    }

    /// A conversation that continues one the agent has: `session_id` is what
    /// the first turn resumes, `earlier` what was said before, as the history
    /// kept it (`true` for the person's words).
    pub fn resumed(session_id: String, earlier: impl IntoIterator<Item = (bool, String)>) -> Self {
        let mut conversation = Self::new();
        conversation.session_id = Some(session_id);
        conversation.items = earlier
            .into_iter()
            .filter(|(_, text)| !text.trim().is_empty())
            .map(|(person, text)| {
                if person {
                    Item::User { text }
                } else {
                    Item::Agent {
                        text,
                        streaming: false,
                    }
                }
            })
            .collect();
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
    pub fn sent(&mut self, text: &str) {
        self.said(text);
        self.running = true;
        self.heard = false;
        self.failed = false;
    }

    /// The person said something to the turn in flight.
    pub fn said(&mut self, text: &str) {
        self.settle();
        self.items.push(Item::User {
            text: text.to_owned(),
        });
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
    pub fn apply(&mut self, event: TurnEvent) {
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
                    }) => draft.push_str(&text),
                    _ => self.items.push(Item::Agent {
                        text,
                        streaming: true,
                    }),
                }
            }
            TurnEvent::Text { text } => {
                self.heard = true;
                match self.items.last_mut() {
                    Some(Item::Agent {
                        text: draft,
                        streaming,
                    }) if *streaming => {
                        *draft = text;
                        *streaming = false;
                    }
                    _ if text.trim().is_empty() => {}
                    _ => self.items.push(Item::Agent {
                        text,
                        streaming: false,
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
            // A pipe on this computer neither drops nor loses output.
            TurnEvent::Disconnected
            | TurnEvent::Reconnected
            | TurnEvent::Gap
            | TurnEvent::Usage { .. } => {}
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

/// The words of a folded stretch of work: "3 steps", "Thought", "1 step, 1
/// failed".
pub fn work_label(items: &[Item]) -> String {
    let (tools, thoughts, failed) = work_summary(items);
    let mut label = match (tools, thoughts) {
        (0, _) => "Thought".to_owned(),
        (1, _) => "1 step".to_owned(),
        (count, _) => format!("{count} steps"),
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
                streaming: true
            }
        );
        conversation.apply(TurnEvent::Text {
            text: "Hi there.".into(),
        });
        assert_eq!(
            conversation.items[1],
            Item::Agent {
                text: "Hi there.".into(),
                streaming: false
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
        assert_eq!(work_label(&conversation.items[1..4]), "2 steps");
        assert_eq!(work_label(&conversation.items[1..2]), "Thought");
    }

    #[test]
    fn a_resumed_conversation_shows_what_was_said_and_resumes_by_its_id() {
        let conversation = Conversation::resumed(
            "abc".into(),
            [
                (true, "fix it".to_owned()),
                (false, String::new()),
                (false, "Fixed.".to_owned()),
            ],
        );
        assert_eq!(conversation.session_id.as_deref(), Some("abc"));
        assert_eq!(conversation.items.len(), 2);
        assert_eq!(conversation.phase(), Phase::Idle);
    }

    #[test]
    fn durations_and_costs_read_short() {
        assert_eq!(duration_text(800), "0.8 s");
        assert_eq!(duration_text(42_000), "42 s");
        assert_eq!(duration_text(185_000), "3 min 05 s");
        assert_eq!(cost_text(0.004), "<$0.01");
        assert_eq!(cost_text(1.2), "$1.20");
    }
}

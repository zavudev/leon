//! A conversation as the window draws it, built from what the service kept.
//!
//! A client reduces the records of the log ([`crate::log`]) into a
//! [`leon_conversation::model::Conversation`], the same one a chat of the
//! window holds, so that King's conversation and a worker's are drawn as
//! any other. [`Followed`] is the client's side of
//! [`crate::protocol::Request::Events`]: it applies what a reply holds, and
//! says when what it has can no longer be added to (the log was cut past
//! what it has, or is another one) so that the client asks again from the
//! start.
//!
//! What has no log (a conversation kept before there was one, a past
//! conversation's file, a service too old to keep any) is read from its
//! [`Entry`]s by [`legacy`]: less than the log says, the same shape.
//!
//! Pure: records in, a conversation out.

use leon_conversation::model::{Conversation, Item, Outcome, Tone, Tool};
use leon_turn::TurnEvent;

use crate::log::{Events, Logged, Record};
use crate::report::Report;
use crate::transcript::{Entry, Kind};

/// What applying a reply did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// What it said is added, or there was nothing to add.
    Kept,
    /// What it said does not follow what is held: the log was cut past it,
    /// or it is another log. Nothing is held now; ask again from the start.
    Reset,
}

/// Where a turn began, and the snapshot of the worktree before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Began {
    /// The place of the words that began it in `conversation.items`.
    pub item: usize,
    /// The commit of the snapshot, when one was taken.
    pub checkpoint: Option<String>,
}

/// A conversation reduced from the records of a log, as far as they have
/// come.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Followed {
    /// What the records say.
    pub conversation: Conversation,
    /// The facts of each of Leon's lines under an answer, by the place of
    /// its note in `conversation.items`: a window draws them its own way.
    pub reports: Vec<(usize, Report)>,
    /// Where each turn the person or King began begins, and the snapshot of
    /// the worktree taken before it, oldest first.
    pub turns: Vec<Began>,
    last: Option<u64>,
    log: bool,
}

impl Followed {
    /// The number of the newest record applied: what to ask the next ones
    /// after.
    pub fn last(&self) -> Option<u64> {
        self.last
    }

    /// Whether the service has a log of this conversation: it said so, or
    /// gave records of it. Without one the conversation is read from the
    /// transcript ([`legacy`]).
    pub fn has_log(&self) -> bool {
        self.log
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Adds what a reply holds.
    pub fn apply(&mut self, events: &Events) -> Applied {
        match (events.last_seq, self.last) {
            // A log that is not there any more, or is shorter than what is
            // held, is another one.
            (None, Some(_)) => return self.reset(),
            (Some(newest), Some(last)) if newest < last => return self.reset(),
            _ => {}
        }
        if events.last_seq.is_some() {
            self.log = true;
        }
        let fresh: Vec<&Logged> = events
            .events
            .iter()
            .filter(|logged| self.last.is_none_or(|last| logged.seq > last))
            .collect();
        // The records the log dropped are missed: what follows them can
        // not be drawn from here.
        if let (Some(first), Some(last)) = (fresh.first(), self.last) {
            if first.seq > last + 1 {
                return self.reset();
            }
        }
        for logged in fresh {
            if let Record::Report { report } = &logged.record {
                self.reports
                    .push((self.conversation.items.len(), report.clone()));
            }
            reduce(&mut self.conversation, logged);
            match &logged.record {
                Record::Prompt { turn: true, .. } => self.turns.push(Began {
                    item: self.conversation.items.len().saturating_sub(1),
                    checkpoint: None,
                }),
                Record::Checkpoint { commit } => {
                    if let Some(began) = self.turns.last_mut() {
                        began.checkpoint = Some(commit.clone());
                    }
                }
                _ => {}
            }
            self.last = Some(logged.seq);
        }
        Applied::Kept
    }

    fn reset(&mut self) -> Applied {
        self.clear();
        Applied::Reset
    }
}

/// Adds one record to a conversation.
pub fn reduce(conversation: &mut Conversation, logged: &Logged) {
    let at = Some((logged.at_ms / 1000) as i64);
    match &logged.record {
        Record::Event { event } => conversation.apply_at(event.clone(), at),
        Record::Prompt { text, turn, .. } => {
            if *turn {
                conversation.sent_at(text, at);
            } else {
                conversation.said_at(text, at);
            }
        }
        Record::News { text } => conversation.woke(text),
        Record::Notice { text, ends_turn } => {
            if *ends_turn {
                conversation.refused(text);
            } else {
                conversation.noted(Tone::Plain, text);
            }
        }
        // Leon's line under an answer: a note, until the window draws it
        // by its facts.
        Record::Report { report } => {
            conversation.noted(Tone::Plain, &crate::report::line(&report.facts()));
        }
        // Kept beside the items, not among them ([`Followed::turns`]).
        Record::Checkpoint { .. } => {}
        Record::Legacy { entry } => entry_of(conversation, entry),
    }
}

/// A conversation read from entries of a transcript, for what has no log:
/// what was asked is the person's words, what the agent answered is its
/// message, a tool is its one line (the whole of it is not kept), and
/// questions, notices, news and reports are notes.
pub fn legacy(entries: &[Entry]) -> Conversation {
    let mut conversation = Conversation::new();
    for entry in entries {
        entry_of(&mut conversation, entry);
    }
    conversation
}

fn entry_of(conversation: &mut Conversation, entry: &Entry) {
    let at = Some((entry.at_ms / 1000) as i64);
    match entry.kind {
        Kind::Prompt => conversation.said_at(&entry.text, at),
        Kind::Agent => conversation.apply_at(
            TurnEvent::Text {
                text: entry.text.clone(),
            },
            at,
        ),
        Kind::Tool => {
            let (name, summary, ok) = split_tool(&entry.text);
            conversation.items.push(Item::Tool(Tool {
                id: format!("entry-{}", entry.seq),
                name,
                summary,
                detail: String::new(),
                changes: Vec::new(),
                outcome: Some(Outcome {
                    ok,
                    summary: String::new(),
                    detail: String::new(),
                }),
            }));
        }
        Kind::Notice => {
            let bad = entry.text.starts_with("The turn failed")
                || entry.text.starts_with("The agent could not be started")
                || entry.text.starts_with("The turn was interrupted");
            conversation.noted(if bad { Tone::Bad } else { Tone::Plain }, &entry.text);
        }
        Kind::News | Kind::Question | Kind::Report => {
            conversation.noted(Tone::Plain, &entry.text);
        }
    }
}

/// A tool's line (`Bash: cargo test`, `failed: exit 1`) as its name, what
/// it was given and whether it worked.
fn split_tool(line: &str) -> (String, String, bool) {
    if let Some(why) = line.strip_prefix("failed: ") {
        return ("tool".to_owned(), why.to_owned(), false);
    }
    match line.split_once(": ") {
        Some((name, summary))
            if !name.is_empty() && name.len() <= 40 && !name.contains(char::is_whitespace) =>
        {
            (name.to_owned(), summary.to_owned(), true)
        }
        _ => (String::new(), line.to_owned(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Events;

    fn logged(seq: u64, record: Record) -> Logged {
        Logged {
            seq,
            at_ms: 1_000 + seq * 1_000,
            record,
        }
    }

    fn event(seq: u64, event: TurnEvent) -> Logged {
        logged(seq, Record::Event { event })
    }

    fn prompt(seq: u64, text: &str) -> Logged {
        logged(
            seq,
            Record::Prompt {
                text: text.into(),
                attachments: Vec::new(),
                turn: true,
            },
        )
    }

    fn page(events: Vec<Logged>) -> Events {
        Events {
            conversation: 1,
            last_seq: events.last().map(|logged| logged.seq),
            events,
            more: false,
        }
    }

    fn finished() -> TurnEvent {
        TurnEvent::Finished {
            ok: true,
            error: None,
            cost_usd: Some(0.5),
            duration_ms: Some(900),
            denied_tools: Vec::new(),
            interrupted: false,
        }
    }

    /// What `apply` is for: the records are the events, so the
    /// conversation is the one the chat of the window would hold.
    #[test]
    fn the_records_of_a_turn_reduce_to_what_the_chat_holds() {
        let events = [
            TurnEvent::Started {
                session_id: "s".into(),
                model: Some("opus".into()),
            },
            TurnEvent::ToolUse {
                id: "t".into(),
                name: "Bash".into(),
                summary: "ls".into(),
                detail: String::new(),
            },
            TurnEvent::ToolResult {
                id: "t".into(),
                ok: true,
                summary: "3 files".into(),
                detail: "a\nb\nc".into(),
            },
            TurnEvent::Text {
                text: "Done.".into(),
            },
            finished(),
            TurnEvent::Exited { code: 0 },
        ];
        let mut records = vec![prompt(0, "list")];
        records.extend(
            events
                .iter()
                .enumerate()
                .map(|(at, e)| event(at as u64 + 1, e.clone())),
        );
        let mut followed = Followed::default();
        assert_eq!(followed.apply(&page(records)), Applied::Kept);

        let mut chat = Conversation::new();
        chat.sent_at("list", Some(1));
        for (at, e) in events.iter().enumerate() {
            chat.apply_at(e.clone(), Some(at as i64 + 2));
        }
        assert_eq!(followed.conversation, chat);
        assert_eq!(followed.last(), Some(6));
        assert!(followed.has_log());
        assert!(!followed.conversation.running());
    }

    #[test]
    fn what_comes_after_is_added_and_what_came_twice_is_not() {
        let mut followed = Followed::default();
        followed.apply(&page(vec![prompt(0, "a"), event(1, finished())]));
        let again = page(vec![
            event(1, finished()),
            event(2, TurnEvent::Exited { code: 0 }),
        ]);
        assert_eq!(followed.apply(&again), Applied::Kept);
        assert_eq!(followed.last(), Some(2));
        assert_eq!(followed.conversation.items.len(), 1, "the prompt, once");
        assert!(!followed.conversation.running());
    }

    #[test]
    fn a_log_cut_past_what_is_held_must_be_asked_again_from_the_start() {
        let mut followed = Followed::default();
        followed.apply(&page(vec![prompt(0, "a"), prompt(1, "b")]));
        // Records 2 to 9 are gone: the next one is 10.
        assert_eq!(followed.apply(&page(vec![prompt(10, "c")])), Applied::Reset);
        assert_eq!(followed, Followed::default());
        // Asked from the start, it holds what the log has.
        assert_eq!(
            followed.apply(&page(vec![prompt(8, "x"), prompt(10, "c")])),
            Applied::Kept
        );
        assert_eq!(followed.last(), Some(10));
    }

    #[test]
    fn a_log_that_is_shorter_or_gone_is_another_conversation() {
        let mut followed = Followed::default();
        followed.apply(&page(vec![prompt(0, "a"), prompt(1, "b")]));
        let shorter = Events {
            last_seq: Some(0),
            ..Events::default()
        };
        assert_eq!(followed.apply(&shorter), Applied::Reset);
        let mut followed = Followed::default();
        followed.apply(&page(vec![prompt(0, "a")]));
        assert_eq!(followed.apply(&Events::default()), Applied::Reset);
    }

    #[test]
    fn nothing_new_changes_nothing_and_an_empty_log_is_no_log() {
        let mut followed = Followed::default();
        assert_eq!(followed.apply(&Events::default()), Applied::Kept);
        assert!(!followed.has_log(), "the service has none for it");
        followed.apply(&page(vec![prompt(0, "a")]));
        let before = followed.clone();
        let nothing = Events {
            last_seq: Some(0),
            ..Events::default()
        };
        assert_eq!(followed.apply(&nothing), Applied::Kept);
        assert_eq!(followed, before);
    }

    #[test]
    fn news_notices_and_reports_are_notes_and_an_ending_notice_ends_the_turn() {
        let mut followed = Followed::default();
        followed.apply(&page(vec![
            logged(
                0,
                Record::News {
                    text: "#1 done".into(),
                },
            ),
            logged(
                1,
                Record::Notice {
                    text: "Leon allowed it.".into(),
                    ends_turn: false,
                },
            ),
        ]));
        assert!(followed.conversation.running(), "the news began a turn");
        followed.apply(&page(vec![
            logged(
                2,
                Record::Notice {
                    text: "The turn was interrupted.".into(),
                    ends_turn: true,
                },
            ),
            logged(
                3,
                Record::Report {
                    report: Default::default(),
                },
            ),
        ]));
        assert!(!followed.conversation.running());
        assert!(followed.conversation.failed);
        let notes: Vec<&str> = followed
            .conversation
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Notice { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notes[..3],
            ["#1 done", "Leon allowed it.", "The turn was interrupted."]
        );
        assert!(notes[3].starts_with("This answer:") || notes[3].starts_with("Today:"));
    }

    #[test]
    fn the_facts_of_a_report_are_kept_by_the_place_of_its_note() {
        let mut followed = Followed::default();
        let report = Report {
            spent_today_usd: 1.5,
            ..Report::default()
        };
        followed.apply(&page(vec![
            prompt(0, "a"),
            logged(
                1,
                Record::Report {
                    report: report.clone(),
                },
            ),
        ]));
        assert_eq!(followed.reports, [(1, report)]);
        assert!(matches!(
            followed.conversation.items[1],
            Item::Notice { .. }
        ));
        followed.clear();
        assert!(followed.reports.is_empty());
    }

    #[test]
    fn a_checkpoint_belongs_to_the_turn_the_last_prompt_began() {
        let mut followed = Followed::default();
        followed.apply(&page(vec![
            prompt(0, "first"),
            logged(
                1,
                Record::Checkpoint {
                    commit: "abc".into(),
                },
            ),
            prompt(2, "second"),
        ]));
        followed.apply(&page(vec![
            logged(
                3,
                Record::Checkpoint {
                    commit: "def".into(),
                },
            ),
            prompt(4, "third"),
        ]));
        assert_eq!(
            followed.turns,
            [
                Began {
                    item: 0,
                    checkpoint: Some("abc".into())
                },
                Began {
                    item: 1,
                    checkpoint: Some("def".into())
                },
                Began {
                    item: 2,
                    checkpoint: None
                },
            ]
        );
        // A checkpoint is no item.
        assert_eq!(followed.conversation.items.len(), 3);
        followed.clear();
        assert!(followed.turns.is_empty());
    }

    #[test]
    fn what_was_said_to_an_agent_in_a_terminal_begins_no_turn() {
        let mut followed = Followed::default();
        followed.apply(&page(vec![logged(
            0,
            Record::Prompt {
                text: "do it".into(),
                attachments: Vec::new(),
                turn: false,
            },
        )]));
        assert!(!followed.conversation.running());
        assert_eq!(followed.conversation.items.len(), 1);
    }

    fn entry(seq: u64, kind: Kind, text: &str) -> Entry {
        Entry {
            seq,
            at_ms: 5_000,
            kind,
            text: text.into(),
            report: None,
        }
    }

    #[test]
    fn entries_without_a_log_become_the_same_items() {
        let conversation = legacy(&[
            entry(0, Kind::Prompt, "fix it"),
            entry(1, Kind::Tool, "Bash: cargo test"),
            entry(2, Kind::Tool, "failed: exit 1"),
            entry(3, Kind::Tool, "Leon allowed it (routine): May it use x"),
            entry(4, Kind::Question, "[q1] Which one? Options: a | b."),
            entry(5, Kind::Agent, "Fixed."),
            entry(6, Kind::Notice, "The turn failed: rate limited"),
            entry(7, Kind::Notice, "The turn was stopped."),
            entry(8, Kind::News, "#2 finished"),
            entry(9, Kind::Report, "This answer: 1.5k tokens"),
        ]);
        let tool = |at: usize| match &conversation.items[at] {
            Item::Tool(tool) => (
                tool.name.clone(),
                tool.summary.clone(),
                tool.outcome.clone(),
            ),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            conversation.items[0],
            Item::User {
                text: "fix it".into(),
                at: Some(5)
            }
        );
        let ok = Some(Outcome {
            ok: true,
            summary: String::new(),
            detail: String::new(),
        });
        let bad = Some(Outcome {
            ok: false,
            summary: String::new(),
            detail: String::new(),
        });
        assert_eq!(tool(1), ("Bash".into(), "cargo test".into(), ok.clone()));
        assert_eq!(tool(2), ("tool".into(), "exit 1".into(), bad));
        assert_eq!(
            tool(3),
            (
                String::new(),
                "Leon allowed it (routine): May it use x".into(),
                ok
            )
        );
        assert_eq!(
            conversation.items[5],
            Item::Agent {
                text: "Fixed.".into(),
                streaming: false,
                at: Some(5)
            }
        );
        assert_eq!(
            conversation.items[6],
            Item::Notice {
                tone: Tone::Bad,
                text: "The turn failed: rate limited".into()
            }
        );
        assert!(matches!(
            conversation.items[7],
            Item::Notice {
                tone: Tone::Plain,
                ..
            }
        ));
        assert_eq!(conversation.items.len(), 10);
        assert!(!conversation.running(), "nothing says a turn is in flight");
    }

    #[test]
    fn a_log_seeded_with_entries_reads_as_those_entries() {
        let entries = [entry(0, Kind::Prompt, "hello"), entry(1, Kind::Agent, "hi")];
        let seeded: Vec<Logged> = entries
            .iter()
            .enumerate()
            .map(|(at, entry)| {
                logged(
                    at as u64,
                    Record::Legacy {
                        entry: entry.clone(),
                    },
                )
            })
            .collect();
        let mut followed = Followed::default();
        followed.apply(&page(seeded));
        assert_eq!(followed.conversation, legacy(&entries));
    }
}

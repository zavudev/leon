//! What a conversation said, kept short enough to send.
//!
//! The window draws a conversation from every event of the protocol
//! (`crates/app/src/chat/model.rs`). King's service keeps less: what was
//! asked, what the agent answered, one line per tool it used, its questions
//! and how each turn ended. That is what King reads of a worker
//! (`worker_transcript`) and what a client shows of a conversation it was
//! not watching while it happened. Entries are numbered from the first one
//! ever kept, so that a client asks for what came after the last one it has,
//! and the oldest go once there are more than [`KEPT`].
//!
//! Everything it is told also goes to its [`Journal`], for the event log
//! (`crate::log`): the entries are what a phone reads, the log what the
//! window draws.
//!
//! Pure: events in, entries out.

use std::collections::VecDeque;

use leon_turn::{QuestionKind, TurnEvent};
use serde::{Deserialize, Serialize};

use crate::log::{Journal, Out, Record};

/// How many entries a transcript keeps.
pub const KEPT: usize = 400;

/// How many characters of a tool's line are kept.
const TOOL_CHARS: usize = 200;

/// What kind of thing an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// What was asked: by the person, or by King of a worker.
    Prompt,
    /// What Leon told King of its workers, waking it.
    News,
    /// What the agent answered.
    Agent,
    /// A tool the agent used, and how it went.
    Tool,
    /// A question the agent asks and waits on.
    Question,
    /// How a turn ended, or what went wrong.
    Notice,
    /// Leon's line under an answer: its tokens, and what is left.
    Report,
}

/// One thing a conversation said.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Its number, from the first entry ever kept.
    pub seq: u64,
    /// When, in milliseconds since the epoch.
    pub at_ms: u64,
    /// What it is.
    pub kind: Kind,
    /// The words.
    pub text: String,
    /// For a report, its facts, for a client to draw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<crate::report::Report>,
}

/// The entries of one conversation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    entries: VecDeque<Entry>,
    next: u64,
    /// The answer being written, from deltas, until its whole text comes.
    #[serde(skip)]
    draft: String,
    /// What the event log is to be given of it.
    #[serde(skip)]
    journal: Journal,
}

impl Transcript {
    /// A transcript that goes on from `entries`, as kept before.
    pub fn restored(entries: Vec<Entry>) -> Self {
        let next = entries.last().map_or(0, |entry| entry.seq + 1);
        let mut transcript = Self {
            entries: entries.into(),
            next,
            draft: String::new(),
            journal: Journal::default(),
        };
        transcript.seed_log();
        transcript
    }

    /// Says that what it holds was kept before the event log: the log is
    /// given those entries first, if it has none of its own yet.
    pub fn seed_log(&mut self) {
        self.journal.seed(self.entries());
    }

    /// What the event log is to be given: [`Out`]s, oldest first. A thought
    /// that more may join stays until something else is said.
    pub fn take_log(&mut self) -> Vec<Out> {
        self.journal.take()
    }

    /// All of it, for a conversation that is over.
    pub fn finish_log(&mut self) -> Vec<Out> {
        self.journal.finish()
    }

    /// Every entry it keeps, oldest first.
    pub fn entries(&self) -> Vec<Entry> {
        self.entries.iter().cloned().collect()
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Keeps an entry.
    pub fn push(&mut self, kind: Kind, text: impl Into<String>, at_ms: u64) {
        let text = text.into();
        let record = match kind {
            Kind::Prompt => Record::Prompt {
                text: text.clone(),
                attachments: Vec::new(),
                turn: true,
            },
            Kind::News => Record::News { text: text.clone() },
            Kind::Agent => Record::Event {
                event: TurnEvent::Text { text: text.clone() },
            },
            Kind::Tool | Kind::Question | Kind::Notice | Kind::Report => Record::Notice {
                text: text.clone(),
                ends_turn: false,
            },
        };
        if self.keep(kind, text, None, at_ms) {
            self.journal.record(record, at_ms);
        }
    }

    /// Keeps what was asked of an agent that Leon runs no turns of (one in a
    /// terminal).
    pub fn push_said(&mut self, text: impl Into<String>, at_ms: u64) {
        let text = text.into();
        let record = Record::Prompt {
            text: text.clone(),
            attachments: Vec::new(),
            turn: false,
        };
        if self.keep(Kind::Prompt, text, None, at_ms) {
            self.journal.record(record, at_ms);
        }
    }

    /// Keeps a notice that the turn in flight ended with: it was cut short,
    /// or it never began.
    pub fn push_end(&mut self, text: impl Into<String>, at_ms: u64) {
        let text = text.into();
        let record = Record::Notice {
            text: text.clone(),
            ends_turn: true,
        };
        if self.keep(Kind::Notice, text, None, at_ms) {
            self.journal.record(record, at_ms);
        }
    }

    /// Keeps that the worktree was snapshotted before the turn just begun:
    /// not something said, so it is in the event log only.
    pub fn push_checkpoint(&mut self, commit: impl Into<String>, at_ms: u64) {
        self.journal.record(
            Record::Checkpoint {
                commit: commit.into(),
            },
            at_ms,
        );
    }

    /// Keeps Leon's report under an answer: its line, and its facts.
    pub fn push_report(&mut self, report: crate::report::Report, at_ms: u64) {
        let text = crate::report::line(&report.facts());
        if self.keep(Kind::Report, text, Some(report.clone()), at_ms) {
            self.journal.record(Record::Report { report }, at_ms);
        }
    }

    fn keep(
        &mut self,
        kind: Kind,
        text: String,
        report: Option<crate::report::Report>,
        at_ms: u64,
    ) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        self.entries.push_back(Entry {
            seq: self.next,
            at_ms,
            kind,
            text,
            report,
        });
        self.next += 1;
        while self.entries.len() > KEPT {
            self.entries.pop_front();
        }
        true
    }

    /// Keeps what an event says, if it says anything worth keeping.
    pub fn apply(&mut self, event: &TurnEvent, at_ms: u64) {
        self.journal.event(event, at_ms, &self.draft);
        match event {
            TurnEvent::TextDelta { text } => self.draft.push_str(text),
            TurnEvent::Text { text } => {
                self.draft.clear();
                self.keep(Kind::Agent, text.clone(), None, at_ms);
            }
            TurnEvent::ToolUse { name, summary, .. } => {
                let line = format!("{name}: {summary}");
                self.keep(Kind::Tool, cut(&line, TOOL_CHARS), None, at_ms);
            }
            TurnEvent::ToolResult {
                ok: false, summary, ..
            } => {
                let line = format!("failed: {summary}");
                self.keep(Kind::Tool, cut(&line, TOOL_CHARS), None, at_ms);
            }
            TurnEvent::Question { id, kind } => {
                self.keep(
                    Kind::Question,
                    format!("[{id}] {}", question_text(kind)),
                    None,
                    at_ms,
                );
            }
            TurnEvent::Finished {
                ok,
                error,
                interrupted,
                ..
            } => {
                if !self.draft.is_empty() {
                    let draft = std::mem::take(&mut self.draft);
                    self.keep(Kind::Agent, draft, None, at_ms);
                }
                if *interrupted {
                    self.keep(
                        Kind::Notice,
                        "The turn was stopped.".to_owned(),
                        None,
                        at_ms,
                    );
                } else if !ok {
                    let why = error.as_deref().unwrap_or("the agent gave no reason");
                    self.keep(Kind::Notice, format!("The turn failed: {why}"), None, at_ms);
                }
            }
            TurnEvent::Gone => {
                self.keep(
                    Kind::Notice,
                    "The turn no longer exists.".to_owned(),
                    None,
                    at_ms,
                );
            }
            _ => {}
        }
    }

    /// The entries after `seq`, all of them when it is `None`, at most
    /// `limit` of the newest.
    pub fn since(&self, seq: Option<u64>, limit: usize) -> Vec<Entry> {
        let after: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| seq.is_none_or(|seq| entry.seq > seq))
            .collect();
        let skip = after.len().saturating_sub(limit);
        after.into_iter().skip(skip).cloned().collect()
    }

    /// The last thing the agent answered.
    pub fn last_answer(&self) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.kind == Kind::Agent)
            .map(|entry| entry.text.as_str())
    }

    /// The number of the newest entry.
    pub fn last_seq(&self) -> Option<u64> {
        self.entries.back().map(|entry| entry.seq)
    }
}

/// A question as one paragraph of text: what is asked and what it may be
/// answered with.
pub fn question_text(kind: &QuestionKind) -> String {
    match kind {
        QuestionKind::Permission {
            tool,
            summary,
            reason,
            choices,
            ..
        } => {
            // Each choice with what it does: an agent's ids (Codex's `d.2`)
            // say nothing, and a guess may deny what was meant to be allowed.
            let named: Vec<String> = choices
                .iter()
                .map(|choice| {
                    let effect = match choice.effect {
                        leon_turn::PermissionEffect::AllowOnce => "allow",
                        leon_turn::PermissionEffect::AllowAlways => "always",
                        leon_turn::PermissionEffect::Deny => "deny",
                    };
                    format!("{} = {effect}", choice.id)
                })
                .collect();
            let why = reason
                .as_deref()
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default();
            format!(
                "May it use {tool}: {}{why}? Choices: {}. Answer with allow, always or deny.",
                cut(summary, TOOL_CHARS),
                named.join(", ")
            )
        }
        QuestionKind::Choices { questions } => questions
            .iter()
            .map(|question| {
                let labels: Vec<&str> = question
                    .options
                    .iter()
                    .map(|option| option.label.as_str())
                    .collect();
                format!("{} Options: {}.", question.question, labels.join(" | "))
            })
            .collect::<Vec<_>>()
            .join(" "),
        QuestionKind::Plan { plan } => format!("{PLAN_LEAD}{plan}"),
    }
}

/// What the text of a plan's question says before the plan itself.
const PLAN_LEAD: &str = "It planned and asks to start:\n";

/// The plan a question's text carries, when it is the question of a plan.
pub fn plan_of(text: &str) -> Option<&str> {
    text.strip_prefix(PLAN_LEAD)
}

/// At most `most` characters of `text`, with an ellipsis when cut.
pub fn cut(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_owned();
    }
    let mut kept: String = text.chars().take(most.saturating_sub(1)).collect();
    kept.push('…');
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(ok: bool) -> TurnEvent {
        TurnEvent::Finished {
            ok,
            error: (!ok).then(|| "rate limited".to_owned()),
            cost_usd: Some(0.5),
            duration_ms: None,
            denied_tools: Vec::new(),
            interrupted: false,
        }
    }

    #[test]
    fn a_plans_question_gives_the_plan_back() {
        let kind = QuestionKind::Plan {
            plan: "1. Do it".to_owned(),
        };
        assert_eq!(plan_of(&question_text(&kind)), Some("1. Do it"));
        assert_eq!(plan_of("May it use Bash?"), None);
    }

    #[test]
    fn answers_tools_and_ends_are_kept_and_drafts_and_thoughts_are_not() {
        let mut transcript = Transcript::default();
        transcript.push(Kind::Prompt, "fix the bug", 1);
        transcript.apply(&TurnEvent::Thought { text: "hm".into() }, 2);
        transcript.apply(
            &TurnEvent::ToolUse {
                id: "t".into(),
                name: "Bash".into(),
                summary: "cargo test".into(),
                detail: String::new(),
            },
            3,
        );
        transcript.apply(&TurnEvent::TextDelta { text: "Do".into() }, 4);
        transcript.apply(
            &TurnEvent::Text {
                text: "Done.".into(),
            },
            5,
        );
        transcript.apply(&finished(true), 6);
        let kinds: Vec<Kind> = transcript.since(None, 50).iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [Kind::Prompt, Kind::Tool, Kind::Agent]);
        assert_eq!(transcript.last_answer(), Some("Done."));
    }

    #[test]
    fn a_draft_left_when_the_turn_ends_is_the_answer_and_a_failure_is_said() {
        let mut transcript = Transcript::default();
        transcript.apply(
            &TurnEvent::TextDelta {
                text: "Half".into(),
            },
            1,
        );
        transcript.apply(&finished(false), 2);
        let entries = transcript.since(None, 50);
        assert_eq!(entries[0].text, "Half");
        assert_eq!(entries[1].text, "The turn failed: rate limited");
    }

    #[test]
    fn a_client_asks_for_what_came_after_the_last_entry_it_has() {
        let mut transcript = Transcript::default();
        for at in 0..(KEPT as u64 + 10) {
            transcript.push(Kind::Agent, format!("line {at}"), at);
        }
        // The oldest went, the numbers did not move.
        let all = transcript.since(None, usize::MAX);
        assert_eq!(all.len(), KEPT);
        assert_eq!(all[0].seq, 10);
        let after = transcript.since(Some(KEPT as u64 + 7), 50);
        let seqs: Vec<u64> = after.iter().map(|entry| entry.seq).collect();
        assert_eq!(seqs, [KEPT as u64 + 8, KEPT as u64 + 9]);
        // The newest, when there are more than asked for.
        assert_eq!(
            transcript.since(None, 1)[0].text,
            format!("line {}", KEPT + 9)
        );
    }

    #[test]
    fn a_long_line_is_cut_on_a_character() {
        assert_eq!(cut("ñandú", 3), "ña…");
        assert_eq!(cut("abc", 3), "abc");
    }

    #[test]
    fn a_permission_says_what_each_choice_does() {
        let kind = QuestionKind::Permission {
            tool: "shell".into(),
            summary: "open chrome".into(),
            detail: String::new(),
            reason: None,
            choices: ["d.0", "d.1", "d.2"]
                .iter()
                .zip([
                    leon_turn::PermissionEffect::AllowOnce,
                    leon_turn::PermissionEffect::AllowAlways,
                    leon_turn::PermissionEffect::Deny,
                ])
                .map(|(id, effect)| leon_turn::PermissionChoice {
                    id: (*id).into(),
                    effect,
                    label: String::new(),
                    scope: None,
                    takes_reason: false,
                })
                .collect(),
        };
        let text = question_text(&kind);
        assert!(
            text.contains("d.0 = allow, d.1 = always, d.2 = deny"),
            "{text}"
        );
    }
}

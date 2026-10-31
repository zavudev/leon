//! What a conversation did, event by event.
//!
//! [`crate::transcript`] keeps a line per thing a conversation said, short
//! enough for a phone. The window draws a conversation from more than that:
//! every tool with its output, the files it changed, its todos, its
//! questions (`leon_conversation`). So the service also keeps the stream
//! the agents produced, as one JSON line per [`Logged`] record, appended to
//! a file per conversation and target (`king/log/<conversation>/<target>.jsonl`,
//! written by the application). A client asks for what came after the last
//! record it has ([`crate::protocol::Request::Events`]) and reduces them with
//! [`crate::follow`].
//!
//! **What is kept.** Every [`TurnEvent`] but its noise:
//!
//! * `TextDelta` is not kept. The service never told a client when one came
//!   (only the whole message makes it say the state changed), so a client
//!   had nothing to stream; the message arrives whole as its `Text`. A draft
//!   that was still there when the turn finished (an agent that never sent
//!   the whole message) is kept as that `Text`, as the transcript does.
//! * Consecutive `Thought`s are one record, and consecutive `Usage`s are
//!   the last of them: a model that thinks aloud sends hundreds.
//!
//! Beside the events, what only King has: what the person asked
//! ([`Record::Prompt`]), the news that woke King ([`Record::News`]), what
//! Leon says of a conversation ([`Record::Notice`]), the line under an
//! answer ([`Record::Report`]) and the snapshot taken before a worker's turn
//! ([`Record::Checkpoint`]). [`Record::Legacy`] is an entry of a
//! transcript kept before there was a log: when a conversation that has
//! entries but no log goes on, they are written first, so the log is the
//! whole conversation.
//!
//! **How much.** A log keeps [`MAX_RECORDS`] records or [`MAX_BYTES`], the
//! newest: when it holds more, the oldest go, from a turn's beginning so
//! that no turn starts in the middle. Numbers go on from the first record
//! ever kept, as the transcript's. One record is at most [`RECORD_BYTES`]
//! once serialised, and a reply to a client at most [`PAGE_BYTES`] (but for
//! one record), well under [`crate::protocol::MAX_LINE`].
//!
//! Pure: records in, lines and pages out. The files are the application's.

use leon_turn::TurnEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::report::Report;
use crate::transcript::Entry;

/// The most records a log keeps.
pub const MAX_RECORDS: usize = 4_000;

/// The most bytes a log keeps.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

/// The most bytes of one record, serialised.
pub const RECORD_BYTES: usize = 256 * 1024;

/// The most bytes of the records in one reply (a record bigger than that is
/// alone in its reply).
pub const PAGE_BYTES: usize = 512 * 1024;

/// How many records a reply holds when the client does not say.
pub const PAGE_RECORDS: usize = 500;

/// What a log keeps of an event, or of what only King has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum Record {
    /// What an agent printed.
    Event {
        /// The event.
        event: TurnEvent,
    },
    /// What was asked, and a turn began with it: by the person, or by King
    /// of a worker.
    Prompt {
        /// The words.
        text: String,
        /// The files that went with it, by path.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
        /// A turn began with it. Not when it was said to an agent in a
        /// terminal, of which Leon runs no turns.
        #[serde(default = "yes", skip_serializing_if = "is_yes")]
        turn: bool,
    },
    /// What Leon told King of its workers, waking it: a turn began with it.
    News {
        /// The words.
        text: String,
    },
    /// What Leon says of the conversation.
    Notice {
        /// The words.
        text: String,
        /// The turn in flight ended with it: it was cut short, or never began.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        ends_turn: bool,
    },
    /// Leon's line under an answer.
    Report {
        /// Its facts.
        report: Report,
    },
    /// The files of the worker's worktree were snapshotted before the turn
    /// that the last prompt began: `commit` keeps them (see
    /// `leon_remote::checkpoint`). Only a worker on this computer, in a git
    /// worktree, has them.
    Checkpoint {
        /// The commit of the snapshot.
        commit: String,
    },
    /// An entry of a transcript that was kept before there was a log.
    Legacy {
        /// The entry.
        entry: Entry,
    },
}

fn yes() -> bool {
    true
}

fn is_yes(turn: &bool) -> bool {
    *turn
}

impl Record {
    /// A turn begins here: where a log may be cut.
    fn begins(&self) -> bool {
        matches!(
            self,
            Self::Prompt { .. } | Self::News { .. } | Self::Legacy { .. }
        )
    }
}

/// A record, numbered and dated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Logged {
    /// Its number, from the first record ever kept. First on its line, so
    /// that [`peek`] finds it without reading the rest.
    pub seq: u64,
    /// When, in milliseconds since the epoch.
    pub at_ms: u64,
    /// What it is.
    #[serde(flatten)]
    pub record: Record,
}

/// What a conversation hands the service to write: not yet numbered.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// The entries a conversation had before it had a log: written first,
    /// when it has none yet, and dropped when it has.
    Seed(Vec<Entry>),
    /// A record.
    Record {
        /// When.
        at_ms: u64,
        /// What.
        record: Record,
    },
}

/// What [`crate::protocol::Request::Events`] answers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Events {
    /// The King conversation the log is of (a worker's log is of the one
    /// that started it).
    #[serde(default)]
    pub conversation: u64,
    /// The records after the one asked, oldest first.
    pub events: Vec<Logged>,
    /// The number of the newest record of the log, whether it is in this
    /// reply or not; none when there is no log.
    #[serde(default)]
    pub last_seq: Option<u64>,
    /// There are more records after the last of these.
    #[serde(default)]
    pub more: bool,
}

/// The conversation's pending records and what is waiting to be written.
///
/// Part of a [`crate::transcript::Transcript`], but not of what it is: two
/// transcripts that said the same are equal whatever they have written.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    out: Vec<Out>,
    /// A thought or a usage not written yet: the next one of its kind joins
    /// it.
    pending: Option<(u64, TurnEvent)>,
}

impl PartialEq for Journal {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Journal {
    /// The conversation had `entries` before it had a log.
    pub fn seed(&mut self, entries: Vec<Entry>) {
        if !entries.is_empty() {
            self.out.push(Out::Seed(entries));
        }
    }

    /// Keeps a record.
    pub fn record(&mut self, record: Record, at_ms: u64) {
        self.flush();
        self.out.push(Out::Record { at_ms, record });
    }

    /// Keeps an event, but its noise. `draft` is what the deltas before it
    /// added up to.
    pub fn event(&mut self, event: &TurnEvent, at_ms: u64, draft: &str) {
        match event {
            TurnEvent::TextDelta { .. } => {}
            TurnEvent::Thought { text } => match &mut self.pending {
                Some((_, TurnEvent::Thought { text: thoughts })) => thoughts.push_str(text),
                _ => self.hold(event, at_ms),
            },
            TurnEvent::Usage { .. } => match &mut self.pending {
                Some((_, held @ TurnEvent::Usage { .. })) => *held = event.clone(),
                _ => self.hold(event, at_ms),
            },
            TurnEvent::Finished { .. } => {
                self.flush();
                if !draft.is_empty() {
                    self.push_event(
                        TurnEvent::Text {
                            text: draft.to_owned(),
                        },
                        at_ms,
                    );
                }
                self.push_event(event.clone(), at_ms);
            }
            _ => {
                self.flush();
                self.push_event(event.clone(), at_ms);
            }
        }
    }

    /// What is waiting to be written, taken. A thought or a usage that more
    /// may join stays.
    pub fn take(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    /// Everything, taken: the conversation is over.
    pub fn finish(&mut self) -> Vec<Out> {
        self.flush();
        self.take()
    }

    fn hold(&mut self, event: &TurnEvent, at_ms: u64) {
        self.flush();
        self.pending = Some((at_ms, event.clone()));
    }

    fn flush(&mut self) {
        if let Some((at_ms, event)) = self.pending.take() {
            self.push_event(event, at_ms);
        }
    }

    fn push_event(&mut self, event: TurnEvent, at_ms: u64) {
        self.out.push(Out::Record {
            at_ms,
            record: Record::Event { event },
        });
    }
}

/// The line of a record: JSON, one line, at most [`RECORD_BYTES`] (what is
/// longer is cut where it is longest, and said to be).
pub fn line(logged: &Logged) -> String {
    let text = serde_json::to_string(logged).unwrap_or_default();
    if text.len() <= RECORD_BYTES {
        return text;
    }
    let Ok(mut value) = serde_json::to_value(logged) else {
        return text;
    };
    for _ in 0..64 {
        let text = value.to_string();
        if text.len() <= RECORD_BYTES {
            return text;
        }
        if !cut_longest(&mut value) {
            break;
        }
    }
    let notice = Logged {
        seq: logged.seq,
        at_ms: logged.at_ms,
        record: Record::Notice {
            text: "A record was too large to keep.".to_owned(),
            ends_turn: false,
        },
    };
    serde_json::to_string(&notice).unwrap_or_default()
}

const CUT_MARK: &str = "\n[cut: too long to keep]";

/// Halves the longest string of `value`. False when there is none worth it.
fn cut_longest(value: &mut Value) -> bool {
    fn longest<'a>(value: &'a mut Value, best: &mut Option<&'a mut String>) {
        match value {
            Value::String(text) => {
                if best.as_ref().is_none_or(|best| text.len() > best.len()) {
                    *best = Some(text);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|item| longest(item, best)),
            Value::Object(fields) => fields.values_mut().for_each(|item| longest(item, best)),
            _ => {}
        }
    }
    let mut best = None;
    longest(value, &mut best);
    let Some(text) = best else {
        return false;
    };
    if text.len() <= CUT_MARK.len() * 2 {
        return false;
    }
    let mut at = text.len() / 2;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    text.truncate(at);
    text.push_str(CUT_MARK);
    true
}

/// The number a line of a log starts with, read without reading the rest.
pub fn peek(line: &str) -> Option<u64> {
    let digits = line.strip_prefix(r#"{"seq":"#)?;
    let end = digits.find(|c: char| !c.is_ascii_digit())?;
    digits[..end].parse().ok()
}

/// A page of a log.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    /// The records after `since`, oldest first.
    pub events: Vec<Logged>,
    /// The newest number the lines hold.
    pub last_seq: Option<u64>,
    /// There are more after these.
    pub more: bool,
}

/// What comes after `since` in `lines`, at most `limit` records and about
/// `budget` bytes of them (one record at least, whatever its size). Lines
/// that are not a record are skipped.
pub fn page<'a>(
    lines: impl IntoIterator<Item = &'a str>,
    since: Option<u64>,
    limit: usize,
    budget: usize,
) -> Page {
    let mut page = Page::default();
    let mut bytes = 0;
    for line in lines {
        let Some(seq) = peek(line) else {
            continue;
        };
        page.last_seq = Some(page.last_seq.map_or(seq, |last| last.max(seq)));
        if since.is_some_and(|since| seq <= since) {
            continue;
        }
        if page.more {
            continue;
        }
        let full = page.events.len() >= limit.max(1)
            || (!page.events.is_empty() && bytes + line.len() + 1 > budget);
        if full {
            page.more = true;
            continue;
        }
        let Ok(logged) = serde_json::from_str::<Logged>(line) else {
            continue;
        };
        bytes += line.len() + 1;
        page.events.push(logged);
    }
    page
}

/// Where to cut a log of `lines` so that it holds its limits: the index of
/// the first line to keep, or none when it holds them already. It keeps the
/// newest three quarters of what is allowed, from the beginning of a turn.
pub fn cut_at(lines: &[&str]) -> Option<usize> {
    let bytes: usize = lines.iter().map(|line| line.len() + 1).sum();
    if lines.len() <= MAX_RECORDS && bytes <= MAX_BYTES {
        return None;
    }
    let (most_records, most_bytes) = (MAX_RECORDS / 4 * 3, MAX_BYTES / 4 * 3);
    let (mut kept, mut size) = (0, 0);
    for line in lines.iter().rev() {
        if kept + 1 > most_records || size + line.len() + 1 > most_bytes {
            break;
        }
        kept += 1;
        size += line.len() + 1;
    }
    let from = lines.len() - kept;
    let begins = |line: &&str| {
        serde_json::from_str::<Logged>(line).is_ok_and(|logged| logged.record.begins())
    };
    let turn = lines[from..].iter().position(begins).map(|at| from + at);
    Some(turn.unwrap_or(from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> TurnEvent {
        TurnEvent::Text { text: text.into() }
    }

    fn logged(seq: u64, record: Record) -> Logged {
        Logged {
            seq,
            at_ms: seq * 10,
            record,
        }
    }

    fn event(seq: u64, event: TurnEvent) -> Logged {
        logged(seq, Record::Event { event })
    }

    fn kinds(out: &[Out]) -> Vec<String> {
        out.iter()
            .map(|out| match out {
                Out::Seed(entries) => format!("seed {}", entries.len()),
                Out::Record { record, .. } => match record {
                    Record::Event { event } => serde_json::to_value(event).unwrap()["type"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    Record::Prompt { .. } => "prompt".into(),
                    Record::News { .. } => "news".into(),
                    Record::Notice { .. } => "notice".into(),
                    Record::Report { .. } => "report".into(),
                    Record::Checkpoint { .. } => "checkpoint".into(),
                    Record::Legacy { .. } => "legacy".into(),
                },
            })
            .collect()
    }

    #[test]
    fn a_record_is_one_line_that_starts_with_its_number() {
        let record = event(7, text("hola"));
        let line = line(&record);
        assert_eq!(
            line,
            r#"{"seq":7,"at_ms":70,"record":"event","event":{"type":"text","text":"hola"}}"#
        );
        assert_eq!(peek(&line), Some(7));
        assert_eq!(serde_json::from_str::<Logged>(&line).unwrap(), record);
        let prompt = logged(
            8,
            Record::Prompt {
                text: "a\nb".into(),
                attachments: vec!["/tmp/x.png".into()],
                turn: true,
            },
        );
        let line = line_of(&prompt);
        assert!(!line.contains('\n'), "{line}");
        assert_eq!(serde_json::from_str::<Logged>(&line).unwrap(), prompt);
        // Without files it is as short as it was before they existed.
        let plain = logged(
            9,
            Record::Prompt {
                text: "x".into(),
                attachments: Vec::new(),
                turn: true,
            },
        );
        assert_eq!(
            line_of(&plain),
            r#"{"seq":9,"at_ms":90,"record":"prompt","text":"x"}"#
        );
        let said = logged(
            10,
            Record::Prompt {
                text: "x".into(),
                attachments: Vec::new(),
                turn: false,
            },
        );
        assert!(line_of(&said).ends_with(r#""text":"x","turn":false}"#));
        let old: Logged =
            serde_json::from_str(r#"{"seq":1,"at_ms":2,"record":"prompt","text":"x"}"#).unwrap();
        assert!(matches!(old.record, Record::Prompt { turn: true, .. }));
        assert_eq!(peek("not a record"), None);
        assert_eq!(peek(r#"{"seq":x}"#), None);
    }

    fn line_of(logged: &Logged) -> String {
        line(logged)
    }

    #[test]
    fn deltas_are_not_kept_and_thoughts_and_usages_join() {
        let mut journal = Journal::default();
        let usage = |input| TurnEvent::Usage {
            tokens: leon_turn::Tokens {
                input,
                ..Default::default()
            },
        };
        journal.event(&TurnEvent::Thought { text: "a".into() }, 1, "");
        journal.event(&TurnEvent::Thought { text: "b".into() }, 2, "");
        journal.event(&TurnEvent::TextDelta { text: "Hi".into() }, 3, "Hi");
        journal.event(&usage(10), 4, "Hi");
        journal.event(&usage(20), 5, "Hi");
        journal.event(&text("Hi."), 6, "Hi");
        let out = journal.take();
        assert_eq!(kinds(&out), ["thought", "usage", "text"]);
        let Out::Record { at_ms, record } = &out[0] else {
            panic!();
        };
        assert_eq!(*at_ms, 1);
        assert_eq!(
            *record,
            Record::Event {
                event: TurnEvent::Thought { text: "ab".into() }
            }
        );
        let Out::Record { record, .. } = &out[1] else {
            panic!();
        };
        assert_eq!(
            *record,
            Record::Event { event: usage(20) },
            "the last usage is the one"
        );
    }

    #[test]
    fn what_waits_to_join_is_not_taken_until_something_else_comes() {
        let mut journal = Journal::default();
        journal.event(&TurnEvent::Thought { text: "a".into() }, 1, "");
        assert!(journal.take().is_empty());
        journal.event(&TurnEvent::Thought { text: "b".into() }, 2, "");
        assert!(journal.take().is_empty());
        journal.event(&TurnEvent::Exited { code: 0 }, 3, "");
        assert_eq!(kinds(&journal.take()), ["thought", "exited"]);
        journal.event(&TurnEvent::Thought { text: "c".into() }, 4, "");
        assert_eq!(kinds(&journal.finish()), ["thought"]);
    }

    #[test]
    fn a_draft_left_when_the_turn_finishes_is_kept_as_the_message() {
        let mut journal = Journal::default();
        journal.event(
            &TurnEvent::TextDelta {
                text: "Half".into(),
            },
            1,
            "Half",
        );
        let finished = TurnEvent::Finished {
            ok: true,
            error: None,
            cost_usd: None,
            duration_ms: None,
            denied_tools: Vec::new(),
            interrupted: false,
        };
        journal.event(&finished, 2, "Half");
        let out = journal.take();
        assert_eq!(kinds(&out), ["text", "finished"]);
        let Out::Record { record, .. } = &out[0] else {
            panic!();
        };
        assert_eq!(
            *record,
            Record::Event {
                event: text("Half")
            }
        );
    }

    #[test]
    fn a_seed_of_nothing_is_nothing() {
        let mut journal = Journal::default();
        journal.seed(Vec::new());
        assert!(journal.take().is_empty());
        assert_eq!(journal, Journal::default());
    }

    #[test]
    fn a_record_that_is_too_big_is_cut_where_it_is_longest_and_says_so() {
        let big = "ñ".repeat(RECORD_BYTES);
        let record = event(
            3,
            TurnEvent::ToolResult {
                id: "t".into(),
                ok: true,
                summary: "ok".into(),
                detail: big,
            },
        );
        let line = line(&record);
        assert!(line.len() <= RECORD_BYTES, "{}", line.len());
        let read: Logged = serde_json::from_str(&line).unwrap();
        let Record::Event {
            event:
                TurnEvent::ToolResult {
                    id,
                    summary,
                    detail,
                    ..
                },
        } = read.record
        else {
            panic!();
        };
        assert_eq!((id.as_str(), summary.as_str()), ("t", "ok"));
        assert!(detail.ends_with("[cut: too long to keep]"));
    }

    fn lines(logged: &[Logged]) -> Vec<String> {
        logged.iter().map(line).collect()
    }

    #[test]
    fn a_page_is_what_came_after_the_last_record_a_client_has() {
        let all: Vec<Logged> = (0..10).map(|n| event(n, text(&format!("m{n}")))).collect();
        let lines = lines(&all);
        let read = |since, limit| page(lines.iter().map(String::as_str), since, limit, usize::MAX);
        let first = read(None, 4);
        let seqs: Vec<u64> = first.events.iter().map(|e| e.seq).collect();
        assert_eq!(
            (seqs, first.more, first.last_seq),
            (vec![0, 1, 2, 3], true, Some(9))
        );
        let next = read(Some(3), 4);
        let seqs: Vec<u64> = next.events.iter().map(|e| e.seq).collect();
        assert_eq!((seqs, next.more), (vec![4, 5, 6, 7], true));
        let rest = read(Some(7), 4);
        let seqs: Vec<u64> = rest.events.iter().map(|e| e.seq).collect();
        assert_eq!((seqs, rest.more), (vec![8, 9], false));
        let none = read(Some(9), 4);
        assert_eq!(
            (none.events.len(), none.more, none.last_seq),
            (0, false, Some(9))
        );
        assert_eq!(read(None, 0).events.len(), 1, "at least one");
        assert_eq!(page([], None, 5, 5), Page::default());
    }

    #[test]
    fn a_page_stays_within_its_bytes_but_always_holds_one_record() {
        // Numbers of one digit and times of two: every line as long.
        let all: Vec<Logged> = (1..6).map(|n| event(n, text(&"x".repeat(1_000)))).collect();
        let lines = lines(&all);
        let size = lines[0].len() + 1;
        let page_of = |budget| page(lines.iter().map(String::as_str), None, 100, budget);
        assert_eq!(page_of(size * 2).events.len(), 2);
        assert_eq!(page_of(size * 2 - 1).events.len(), 1);
        assert_eq!(page_of(1).events.len(), 1);
        assert!(page_of(1).more);
    }

    #[test]
    fn a_reply_of_the_biggest_records_is_well_under_the_longest_line() {
        let big = "z".repeat(RECORD_BYTES * 2);
        let all: Vec<Logged> = (0..20)
            .map(|n| {
                event(
                    n,
                    TurnEvent::ToolResult {
                        id: "t".into(),
                        ok: true,
                        summary: String::new(),
                        detail: big.clone(),
                    },
                )
            })
            .collect();
        let lines = lines(&all);
        assert!(lines.iter().all(|line| line.len() <= RECORD_BYTES));
        let page = page(
            lines.iter().map(String::as_str),
            None,
            PAGE_RECORDS,
            PAGE_BYTES,
        );
        assert!(page.more);
        let reply = serde_json::to_string(&crate::protocol::Reply::done(
            u64::MAX,
            serde_json::to_value(Events {
                conversation: u64::MAX,
                events: page.events,
                last_seq: page.last_seq,
                more: page.more,
            })
            .unwrap(),
        ))
        .unwrap();
        assert!(
            reply.len() < crate::protocol::MAX_LINE / 2,
            "{} bytes",
            reply.len()
        );
    }

    #[test]
    fn lines_that_are_not_records_are_skipped() {
        let good = line(&event(2, text("x")));
        let lines = ["", "garbage", r#"{"seq":1,"at_ms":0}"#, good.as_str()];
        let page = page(lines, None, 10, usize::MAX);
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.last_seq, Some(2).max(Some(1)));
    }

    #[test]
    fn a_log_within_its_limits_is_not_cut_and_one_over_is_cut_at_a_turn() {
        let small: Vec<String> = (0..10).map(|n| line(&event(n, text("x")))).collect();
        let refs: Vec<&str> = small.iter().map(String::as_str).collect();
        assert_eq!(cut_at(&refs), None);

        // Turns of a prompt and two more records, over the limit of records.
        let mut over = Vec::new();
        let mut seq = 0;
        while over.len() <= MAX_RECORDS {
            over.push(line(&logged(
                seq,
                Record::Prompt {
                    text: "q".into(),
                    attachments: Vec::new(),
                    turn: true,
                },
            )));
            over.push(line(&event(seq + 1, text("a"))));
            over.push(line(&event(seq + 2, TurnEvent::Exited { code: 0 })));
            seq += 3;
        }
        let refs: Vec<&str> = over.iter().map(String::as_str).collect();
        let at = cut_at(&refs).unwrap();
        let kept = &refs[at..];
        assert!(kept.len() <= MAX_RECORDS / 4 * 3, "{}", kept.len());
        assert!(kept.len() > MAX_RECORDS / 2, "{}", kept.len());
        assert!(
            kept[0].contains(r#""record":"prompt""#),
            "it starts at a turn: {}",
            kept[0]
        );
    }

    #[test]
    fn a_log_over_its_bytes_is_cut_too() {
        let big = "y".repeat(100_000);
        let lines: Vec<String> = (0..60).map(|n| line(&event(n, text(&big)))).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let at = cut_at(&refs).unwrap();
        let kept: usize = refs[at..].iter().map(|line| line.len() + 1).sum();
        assert!(kept <= MAX_BYTES / 4 * 3);
        assert!(at > 0);
    }
}

//! The engine of one turn: what to tell the chat and what to write to the
//! terminal, given what the terminal printed and what the person did.
//!
//! It is pure: bytes, the clock and actions go in, [`Effects`] come out.
//! [`crate::session::Turn`] feeds it from the host's terminal and carries
//! its effects out. A turn speaks to the agent through the bridge
//! ([`crate::bridge`]) with an [`AgentSession`] that knows its protocol.
//!
//! What it guarantees:
//!
//! * **Nothing is written early.** Messages wait in the outbox until the
//!   bridge said it is ready.
//! * **Nothing is lost while offline.** A message written while the
//!   connection is down stays in the outbox and is written when it is back.
//!   A message is only forgotten when the bridge acknowledged it; one that is
//!   not acknowledged within [`RESEND_AFTER`] is written again, and the
//!   bridge passes a number on once however often it arrives.
//! * **A replayed turn is not answered twice.** A turn found again after the
//!   application was closed is read from its beginning. While it catches up,
//!   nothing is written; the bridge's acknowledgements in the replay say
//!   what the agent was already told, and whatever the replay would send
//!   again under the same tag is dropped. Questions without an acknowledged
//!   answer come out of the replay open, and can be answered.
//! * **After a gap** (the host keeps the last 2 MiB of a terminal) the
//!   beginning is lost: questions read after the gap are exact; one asked
//!   before it is not known and cannot be shown, and the turn then looks
//!   busy until it is interrupted. A waiting agent prints nothing, so a
//!   question still open is among the last things printed and the case needs
//!   2 MiB of output after it. Answering a question the agent already has an
//!   answer for (its acknowledgement fell in the gap) is harmless: the agent
//!   ignores an answer to nothing.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::bridge::{self, Mark, Marks, END_TAG};
use crate::error::MobileError;
use leon_turn::{object_on, Lines, TurnEvent};
use leon_turn::{Action, AgentSession, Outgoing, Step};

/// How long a written message may stay unacknowledged before it is written
/// again.
const RESEND_AFTER: Duration = Duration::from_secs(5);
/// How long a replayed turn may be silent before it counts as caught up,
/// when the number of bytes to expect could not be trusted.
const CAUGHT_UP_AFTER: Duration = Duration::from_millis(700);

/// What the engine wants done.
#[derive(Debug, Default)]
pub(crate) struct Effects {
    /// What to tell the listener, in order.
    pub events: Vec<TurnEvent>,
    /// What to write to the terminal, in order.
    pub write: Vec<Vec<u8>>,
}

/// One message on its way to the agent.
#[derive(Debug)]
struct Queued {
    seq: u64,
    tag: String,
    bytes: Vec<u8>,
    /// When it was last written; nothing when it has to be (again).
    written: Option<Instant>,
}

/// A turn read from its beginning that has not reached the present yet.
#[derive(Debug)]
struct Replay {
    /// How many bytes the host had when the turn was attached.
    expected: u64,
    received: u64,
    last_data: Instant,
}

struct Bridged {
    session: Box<dyn AgentSession>,
    marks: Marks,
    lines: Lines,
    /// The bridge said it is ready (or must be: output was lost before us).
    ready: bool,
    online: bool,
    replay: Option<Replay>,
    outbox: VecDeque<Queued>,
    /// Tags the bridge acknowledged: never to be sent again.
    acknowledged: HashSet<String>,
    /// The number of the last frame made.
    seq: u64,
    ended: bool,
}

/// See the module.
pub(crate) struct Engine {
    bridged: Box<Bridged>,
    exited: bool,
}

impl Engine {
    /// A turn spoken to both ways, just started. `clock_ms` (milliseconds
    /// since the Unix epoch) is where its frame numbers start: later than
    /// any number an earlier process gave a frame of the same turn.
    /// `waiting` are the first messages that did not fit the bridge's
    /// arguments.
    pub(crate) fn session(
        session: Box<dyn AgentSession>,
        clock_ms: u64,
        waiting: Vec<Outgoing>,
    ) -> Self {
        let mut bridged = Bridged::new(session, clock_ms, None);
        for outgoing in waiting {
            bridged.queue(outgoing);
        }
        Self {
            bridged: Box::new(bridged),
            exited: false,
        }
    }

    /// A turn spoken to both ways, found again: its output is about to be
    /// replayed from the start, `expected` bytes of it.
    pub(crate) fn attached(
        session: Box<dyn AgentSession>,
        clock_ms: u64,
        expected: u64,
        now: Instant,
    ) -> Self {
        let replay = Replay {
            expected,
            received: 0,
            last_data: now,
        };
        Self {
            bridged: Box::new(Bridged::new(session, clock_ms, Some(replay))),
            exited: false,
        }
    }

    /// Terminal output, in order.
    pub(crate) fn data(&mut self, bytes: &[u8], now: Instant) -> Effects {
        self.bridged.data(bytes, now)
    }

    /// Output was lost before what comes next.
    pub(crate) fn gap(&mut self, now: Instant) -> Effects {
        let bridged = &mut self.bridged;
        bridged.marks.reset();
        bridged.lines.reset();
        // The bridge said it was ready in what was lost.
        bridged.ready = true;
        let mut effects = Effects {
            events: vec![TurnEvent::Gap],
            write: Vec::new(),
        };
        bridged.flush(now, &mut effects);
        effects
    }

    /// The process ended.
    pub(crate) fn exited(&mut self, code: u32) -> Effects {
        self.exited = true;
        self.bridged.outbox.clear();
        let events = self.bridged.session.exited(code);
        Effects {
            events,
            write: Vec::new(),
        }
    }

    /// The connection dropped: nothing written now would arrive.
    pub(crate) fn disconnected(&mut self) -> Effects {
        self.bridged.online = false;
        Effects {
            events: vec![TurnEvent::Disconnected],
            write: Vec::new(),
        }
    }

    /// The connection is back: what waited, and what may have been lost on
    /// the way, is written.
    pub(crate) fn reconnected(&mut self, now: Instant) -> Effects {
        let mut effects = Effects {
            events: vec![TurnEvent::Reconnected],
            write: Vec::new(),
        };
        let bridged = &mut self.bridged;
        bridged.online = true;
        for queued in &mut bridged.outbox {
            queued.written = None;
        }
        bridged.flush(now, &mut effects);
        effects
    }

    /// Time passed.
    pub(crate) fn tick(&mut self, now: Instant) -> Effects {
        let mut effects = Effects::default();
        let bridged = &mut self.bridged;
        if bridged
            .replay
            .as_ref()
            .is_some_and(|replay| now.duration_since(replay.last_data) >= CAUGHT_UP_AFTER)
        {
            bridged.replay = None;
        }
        for queued in &mut bridged.outbox {
            if queued
                .written
                .is_some_and(|at| now.duration_since(at) >= RESEND_AFTER)
            {
                queued.written = None;
            }
        }
        if !self.exited {
            bridged.flush(now, &mut effects);
        }
        effects
    }

    /// The person did something.
    pub(crate) fn act(&mut self, action: Action, now: Instant) -> Result<Effects, MobileError> {
        if self.exited {
            return Err(MobileError::TurnEnded);
        }
        let bridged = &mut self.bridged;
        let mut effects = Effects::default();
        for outgoing in bridged.session.encode(action)? {
            bridged.queue(outgoing);
        }
        bridged.flush(now, &mut effects);
        Ok(effects)
    }

    /// Messages the agent has not acknowledged, for whoever wants to say
    /// "waiting to be sent".
    pub(crate) fn unsent(&self) -> usize {
        self.bridged.outbox.len()
    }
}

impl Bridged {
    fn new(session: Box<dyn AgentSession>, clock_ms: u64, replay: Option<Replay>) -> Self {
        Self {
            session,
            marks: Marks::default(),
            lines: Lines::default(),
            ready: false,
            online: true,
            replay,
            outbox: VecDeque::new(),
            acknowledged: HashSet::new(),
            seq: clock_ms,
            ended: false,
        }
    }

    fn queue(&mut self, outgoing: Outgoing) {
        let tag = bridge::safe_tag(&outgoing.tag);
        if self.acknowledged.contains(&tag) {
            return;
        }
        self.seq += 1;
        let bytes = bridge::frame(self.seq, &tag, &outgoing.line);
        // The same message again (an answer changed before it left): the
        // later one stands.
        self.outbox.retain(|queued| queued.tag != tag);
        self.outbox.push_back(Queued {
            seq: self.seq,
            tag,
            bytes,
            written: None,
        });
    }

    fn data(&mut self, bytes: &[u8], now: Instant) -> Effects {
        let mut effects = Effects::default();
        if let Some(replay) = &mut self.replay {
            replay.received += bytes.len() as u64;
            replay.last_data = now;
            if replay.received >= replay.expected {
                self.replay = None;
            }
        }
        // Marks and output are handled in the order they were printed: an
        // acknowledgement must not overtake the lines before it.
        let mut step = Step::default();
        let mut start = 0;
        for at in 0..=bytes.len() {
            if at < bytes.len() && bytes[at] != leon_turn::bridge::CLOSE {
                continue;
            }
            let end = (at + 1).min(bytes.len());
            let (mut output, mut marks) = (Vec::new(), Vec::new());
            self.marks.push(&bytes[start..end], &mut output, &mut marks);
            start = end;
            self.output(&output, &mut step);
            for mark in marks {
                self.mark(mark, &mut step);
            }
        }
        effects.events.append(&mut step.events);
        for outgoing in step.send {
            self.queue(outgoing);
        }
        if self.session.idle() && !self.ended {
            self.end();
        }
        self.flush(now, &mut effects);
        effects
    }

    fn output(&mut self, bytes: &[u8], step: &mut Step) {
        for line in self.lines.push(bytes) {
            // What a login shell printed before the bridge started is never
            // the agent's.
            match object_on(&line).filter(|_| self.ready) {
                Some(value) => self.session.line(&value, step),
                None => self.session.noise(&line),
            }
        }
    }

    fn mark(&mut self, mark: Mark, step: &mut Step) {
        match mark {
            Mark::Ready => self.ready = true,
            Mark::Ack { seq, tag } => {
                self.outbox
                    .retain(|queued| queued.seq != seq && queued.tag != tag);
                if tag == END_TAG {
                    self.ended = true;
                }
                self.session.delivered(&tag, step);
                self.acknowledged.insert(tag);
            }
            Mark::Nak { seq } => {
                for queued in &mut self.outbox {
                    if queued.seq == seq {
                        queued.written = None;
                    }
                }
            }
        }
    }

    /// Closes the agent's input: it has nothing more to do.
    fn end(&mut self) {
        self.ended = true;
        if self.acknowledged.contains(END_TAG) {
            return;
        }
        self.seq += 1;
        self.outbox.push_back(Queued {
            seq: self.seq,
            tag: END_TAG.to_owned(),
            bytes: bridge::end(self.seq),
            written: None,
        });
    }

    fn flush(&mut self, now: Instant, effects: &mut Effects) {
        if !self.ready || !self.online || self.replay.is_some() {
            return;
        }
        let acknowledged = &self.acknowledged;
        self.outbox
            .retain(|queued| !acknowledged.contains(&queued.tag));
        for queued in &mut self.outbox {
            if queued.written.is_none() {
                queued.written = Some(now);
                effects.write.push(queued.bytes.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_turn::Answer;
    use leon_turn::TurnSpec;

    const ASK: &str = r#"{"type":"control_request","request_id":"q1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"tool_use_id":"t1"}}"#;
    const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s","queued_turn_count":0}"#;

    fn claude() -> Box<dyn AgentSession> {
        leon_turn::claude::session(0)
    }

    fn allow() -> Action {
        Action::Answer(
            "q1".into(),
            Answer::Permission {
                choice_id: "once".into(),
                reason: None,
            },
        )
    }

    fn text(write: &[Vec<u8>]) -> String {
        write
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect()
    }

    /// The numbers of the frames in `write`, in order.
    fn numbers(write: &[Vec<u8>]) -> Vec<u64> {
        text(write)
            .lines()
            .filter_map(|line| line.strip_prefix("B ").or(line.strip_prefix("X ")))
            .map(|seq| seq.parse().unwrap())
            .collect()
    }

    fn ready_line(line: &str) -> Vec<u8> {
        format!("\x1b]5379;leon-ready\x07{line}\r\n").into_bytes()
    }

    #[test]
    fn nothing_is_written_before_the_bridge_is_ready() {
        let now = Instant::now();
        let mut session = claude();
        let first = session.first(&TurnSpec {
            prompt: "hello",
            ..TurnSpec::default()
        });
        let mut engine = Engine::session(session, 1000, first);
        assert!(engine.tick(now).write.is_empty());
        assert!(engine
            .data(b"Welcome to zsh\r\n{\"type\":\"result\"}\r\n", now)
            .write
            .is_empty());
        let effects = engine.data(b"\x1b]5379;leon-ready\x07", now);
        assert_eq!(numbers(&effects.write), [1001, 1002]);
        let written = text(&effects.write);
        assert!(written.contains("\"subtype\":\"initialize\""));
        assert!(written.contains("E 1002 1 p\n"));
        // What a shell printed before the bridge was not read as the agent's.
        assert!(effects.events.is_empty());
    }

    #[test]
    fn an_answer_is_written_once_acknowledged_once_and_closes_the_question() {
        let now = Instant::now();
        let mut engine = Engine::session(claude(), 50, Vec::new());
        let asked = engine.data(&ready_line(ASK), now);
        assert!(matches!(asked.events[..], [TurnEvent::Question { .. }]));
        let answered = engine.act(allow(), now).unwrap();
        assert_eq!(numbers(&answered.write), [51]);
        assert_eq!(engine.unsent(), 1);
        // Nothing more is written while its acknowledgement is awaited.
        assert!(engine.tick(now + Duration::from_secs(1)).write.is_empty());
        let acked = engine.data(b"\x1b]5379;leon-ack:51:a.q1\x07", now);
        assert_eq!(
            acked.events,
            [TurnEvent::QuestionResolved { id: "q1".into() }]
        );
        assert_eq!(engine.unsent(), 0);
        assert_eq!(
            engine.act(allow(), now).map(|_| ()),
            Err(MobileError::UnknownQuestion)
        );
    }

    #[test]
    fn an_answer_given_while_offline_waits_and_is_written_when_the_connection_is_back() {
        let now = Instant::now();
        let mut engine = Engine::session(claude(), 50, Vec::new());
        engine.data(&ready_line(ASK), now);
        assert_eq!(engine.disconnected().events, [TurnEvent::Disconnected]);
        let answered = engine.act(allow(), now).unwrap();
        assert!(answered.write.is_empty());
        assert!(engine.tick(now + Duration::from_secs(60)).write.is_empty());
        assert_eq!(engine.unsent(), 1);
        let back = engine.reconnected(now + Duration::from_secs(61));
        assert_eq!(back.events, [TurnEvent::Reconnected]);
        assert_eq!(numbers(&back.write), [51]);
    }

    #[test]
    fn a_message_lost_on_the_way_is_written_again_under_the_same_number() {
        let now = Instant::now();
        let mut engine = Engine::session(claude(), 50, Vec::new());
        engine.data(&ready_line(ASK), now);
        engine.act(allow(), now).unwrap();
        assert!(engine.tick(now + Duration::from_secs(4)).write.is_empty());
        let again = engine.tick(now + RESEND_AFTER);
        assert_eq!(numbers(&again.write), [51]);
        // The bridge says it arrived damaged: at once.
        let damaged = engine.data(b"\x1b]5379;leon-nak:51\x07", now + RESEND_AFTER);
        assert_eq!(numbers(&damaged.write), [51]);
        // Written while the connection was dropping: again when it is back.
        engine.disconnected();
        let back = engine.reconnected(now + RESEND_AFTER);
        assert_eq!(numbers(&back.write), [51]);
    }

    #[test]
    fn when_the_agent_is_done_its_input_is_closed_once() {
        let now = Instant::now();
        let mut engine = Engine::session(claude(), 50, Vec::new());
        let done = engine.data(&ready_line(RESULT), now);
        assert!(matches!(
            done.events.last(),
            Some(TurnEvent::Finished { ok: true, .. })
        ));
        assert_eq!(text(&done.write), "X 51\n");
        assert!(engine
            .data(b"\x1b]5379;leon-ack:51:end\x07", now)
            .write
            .is_empty());
        assert!(engine.tick(now + Duration::from_secs(30)).write.is_empty());
        let exited = engine.exited(0);
        assert_eq!(exited.events, [TurnEvent::Exited { code: 0 }]);
        assert_eq!(
            engine.act(Action::Interrupt, now).map(|_| ()),
            Err(MobileError::TurnEnded)
        );
    }

    #[test]
    fn a_replayed_turn_comes_back_with_its_open_question_and_writes_nothing_twice() {
        let now = Instant::now();
        // First the agent refused nobody: a request this client does not
        // serve, refused by the first process, acknowledged; then a question
        // answered and acknowledged; then one still open.
        let replay = format!(
            "noise\r\n\x1b]5379;leon-ready\x07{}\r\n\x1b]5379;leon-ack:7:r.h1\x07{ASK}\r\n\x1b]5379;leon-ack:8:a.q1\x07{}\r\n",
            r#"{"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback"}}"#,
            ASK.replace("q1", "q2").replace("t1", "t2"),
        );
        let mut engine = Engine::attached(claude(), 9000, replay.len() as u64, now);
        let (head, tail) = replay.as_bytes().split_at(replay.len() / 2);
        let first = engine.data(head, now);
        assert!(first.write.is_empty());
        let second = engine.data(tail, now);
        assert!(second.write.is_empty(), "{}", text(&second.write));
        let mut events = first.events;
        events.extend(second.events);
        let open: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                TurnEvent::Question { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(open, ["q1", "q2"]);
        assert!(events.contains(&TurnEvent::QuestionResolved { id: "q1".into() }));
        assert!(!events.contains(&TurnEvent::QuestionResolved { id: "q2".into() }));
        // Caught up: the open one can be answered, numbered after anything
        // an earlier process wrote.
        let answer = Action::Answer(
            "q2".into(),
            Answer::Permission {
                choice_id: "deny".into(),
                reason: None,
            },
        );
        let answered = engine.act(answer, now).unwrap();
        assert_eq!(numbers(&answered.write), [9001]);
    }

    #[test]
    fn a_replay_that_is_shorter_than_announced_is_caught_up_once_it_falls_silent() {
        let now = Instant::now();
        let mut engine = Engine::attached(claude(), 9000, 1_000_000, now);
        engine.data(&ready_line(ASK), now);
        let waiting = engine.act(allow(), now).unwrap();
        assert!(waiting.write.is_empty());
        assert!(engine
            .tick(now + Duration::from_millis(300))
            .write
            .is_empty());
        let caught_up = engine.tick(now + CAUGHT_UP_AFTER);
        assert_eq!(numbers(&caught_up.write), [9001]);
    }

    #[test]
    fn after_a_gap_the_turn_reads_on_and_can_be_written_to() {
        let now = Instant::now();
        let mut engine = Engine::attached(claude(), 9000, 10, now);
        // The beginning, the bridge's "ready" with it, was lost.
        let gap = engine.gap(now);
        assert_eq!(gap.events, [TurnEvent::Gap]);
        let asked = engine.data(format!("cut line\"}}\r\n{ASK}\r\n").as_bytes(), now);
        assert!(matches!(asked.events[..], [TurnEvent::Question { .. }]));
        let answered = engine.act(allow(), now).unwrap();
        assert_eq!(numbers(&answered.write), [9001]);
        let stopped = engine.act(Action::Interrupt, now).unwrap();
        assert_eq!(numbers(&stopped.write), [9002]);
    }
}

#[cfg(test)]
mod replay_tests;

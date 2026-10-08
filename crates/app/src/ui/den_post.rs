//! Messages to a lion: when one may be typed into its session, and the ones
//! that wait until then.
//!
//! A message from the Den is typed into the session's terminal as a prompt:
//! pasted, then Enter. That is only safe while the agent sits at its own
//! prompt with nothing on screen to answer, so this module decides, purely,
//! from the same facts the lion is drawn from ([`ready`]):
//!
//! | What is known | The message |
//! | --- | --- |
//! | the terminal ended | refused |
//! | restored and paused | refused |
//! | no agent in front of the shell | refused |
//! | the transcript is not followed (a remote session, an agent Leon cannot read, an id not learned yet) | refused: nothing tells a prompt from a question |
//! | a tool call without a result (it runs, or a permission prompt or a question is on screen) | waits |
//! | the turn is not over | waits |
//! | the turn is over, the terminal still prints | waits |
//! | the turn is over and the terminal is quiet | sent |
//!
//! **Never into a question.** A permission prompt and a question of the
//! agent's are both a tool call without a result: text typed there would
//! answer them. So a call in flight always waits, whatever the terminal
//! does.
//!
//! The ones that wait are kept in an [`Outbox`], in memory, one line of
//! messages a session, oldest first. After one is typed the next waits until
//! the session is known to have taken it (it was seen busy, or its
//! transcript holds one more prompt) and is ready again: a transcript is read
//! a moment after the terminal took the text, and until then it still says
//! the turn is over. A message that shows neither sign may still sit in the
//! agent's input with its Enter lost: nothing is typed after it, and the
//! user is told. A session that ends loses its line.

use std::collections::{HashMap, VecDeque};

use leon_history::live::Pulse;

use super::activity::Activity;
use super::den::Facts;

/// Why a session can never be sent a message as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Never {
    /// Its terminal ended.
    Ended,
    /// It was restored and its agent is not resumed yet.
    Paused,
    /// No agent is in front of its shell.
    NoAgent,
    /// Its transcript is not followed.
    Unread,
}

impl Never {
    /// The reason in a few words, for a list of several sessions.
    pub fn short(self) -> &'static str {
        match self {
            Never::Ended => "it has ended",
            Never::Paused => "it is paused",
            Never::NoAgent => "no agent is in front of its shell",
            Never::Unread => "its transcript is not followed",
        }
    }

    /// The reason, for the status line: `name` is the session's.
    pub fn why(self, name: &str) -> String {
        match self {
            Never::Ended => format!("{name} has ended: there is nobody to message."),
            Never::Paused => {
                format!("{name} is paused: open it to resume its agent, then message it.")
            }
            Never::NoAgent => format!("{name} has no agent in front of its shell."),
            Never::Unread => format!(
                "Leon does not follow the transcript of {name}, so it cannot tell its prompt from a question: type in its terminal."
            ),
        }
    }
}

/// Whether a message may be typed into a session now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ready {
    /// The agent is at its prompt.
    Now,
    /// It is busy, or something on screen waits for an answer.
    Later,
    /// It cannot be messaged.
    Never(Never),
}

/// Whether a message may be typed into the session these facts are of: the
/// table of this module's documentation.
pub fn ready(facts: &Facts, pulse: Option<&Pulse>) -> Ready {
    if facts.exit.is_some() {
        return Ready::Never(Never::Ended);
    }
    if facts.paused {
        return Ready::Never(Never::Paused);
    }
    if !facts.agent {
        return Ready::Never(Never::NoAgent);
    }
    let Some(pulse) = pulse else {
        return Ready::Never(Never::Unread);
    };
    if !pulse.tools().is_empty() || !pulse.turn_over() {
        return Ready::Later;
    }
    match facts.activity {
        Activity::Waiting => Ready::Now,
        _ => Ready::Later,
    }
}

/// What became of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Posted {
    /// Type it now.
    Send(String),
    /// It waits; so many wait for this session now.
    Queued(usize),
    /// It was refused.
    Refused(Never),
}

/// A message that was typed and is not known to have been taken yet.
#[derive(Debug, Clone, Copy)]
struct Typed {
    /// How many prompts the transcript held when it was typed.
    prompts: u32,
    /// How many times the session was looked at since and found at its
    /// prompt, with no new prompt in its transcript.
    looks: u32,
    /// The user was told that it does not seem to have been taken.
    stalled: bool,
}

#[derive(Debug, Default)]
struct Line {
    waiting: VecDeque<String>,
    /// The message last typed, until the session is seen to have taken it:
    /// seen busy, or with one more prompt in its transcript.
    typed: Option<Typed>,
}

impl Line {
    /// Forgets the message last typed once the session has taken it.
    fn settle(&mut self, ready: Ready, prompts: u32) {
        if let Some(typed) = self.typed {
            if ready != Ready::Now || prompts > typed.prompts {
                self.typed = None;
            }
        }
    }
}

/// How many looks a session that was just sent a message may be found at
/// its prompt, with no new prompt in its transcript, before the message is
/// taken not to have been sent: its Enter was lost, and the text may still
/// sit in the agent's input.
pub const SETTLE_LOOKS: u32 = 20;

/// What a look at a session gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Due {
    /// Nothing to do.
    Nothing,
    /// Type this message now.
    Type(String),
    /// The message last typed does not seem to have been taken, and so many
    /// wait behind it. Said once; nothing more is typed until the session
    /// starts a turn.
    Stalled(usize),
}

/// The messages that wait, by session.
///
/// **Never two in one prompt.** A message is typed only when the one before
/// it is known to have been taken: the session was seen busy since, or its
/// transcript holds one more prompt. A message whose Enter was lost still
/// sits in the agent's input, and another typed there would join it. So a
/// session that shows neither sign after [`SETTLE_LOOKS`] looks is stalled:
/// the user is told ([`Due::Stalled`]) and its line holds until the session
/// starts a turn, by whatever the user does in its terminal.
#[derive(Debug, Default)]
pub struct Outbox {
    lines: HashMap<u64, Line>,
}

impl Outbox {
    /// A message for a session that is `ready` or not, whose transcript
    /// holds `prompts` prompts.
    pub fn post(&mut self, id: u64, text: &str, ready: Ready, prompts: u32) -> Posted {
        if let Ready::Never(why) = ready {
            return Posted::Refused(why);
        }
        let line = self.lines.entry(id).or_default();
        line.settle(ready, prompts);
        // The ones that wait go first, and never over one not taken yet.
        if ready == Ready::Now && line.waiting.is_empty() && line.typed.is_none() {
            line.typed = Some(Typed {
                prompts,
                looks: 0,
                stalled: false,
            });
            return Posted::Send(text.to_owned());
        }
        line.waiting.push_back(text.to_owned());
        Posted::Queued(line.waiting.len())
    }

    /// The session was looked at: the message to type now, if one is due.
    pub fn due(&mut self, id: u64, ready: Ready, prompts: u32) -> Due {
        let Some(line) = self.lines.get_mut(&id) else {
            return Due::Nothing;
        };
        line.settle(ready, prompts);
        if ready != Ready::Now {
            return Due::Nothing;
        }
        if let Some(typed) = &mut line.typed {
            typed.looks += 1;
            if typed.looks >= SETTLE_LOOKS && !typed.stalled {
                typed.stalled = true;
                return Due::Stalled(line.waiting.len());
            }
            return Due::Nothing;
        }
        match line.waiting.pop_front() {
            Some(next) => {
                line.typed = Some(Typed {
                    prompts,
                    looks: 0,
                    stalled: false,
                });
                Due::Type(next)
            }
            None => Due::Nothing,
        }
    }

    /// Whether the message last typed into a session does not seem to have
    /// been taken: nothing more is typed into it for now.
    pub fn stalled(&self, id: u64) -> bool {
        self.lines
            .get(&id)
            .and_then(|line| line.typed)
            .is_some_and(|typed| typed.stalled)
    }

    /// Whether anything is left to do: a message waits, or one was just
    /// typed and its session was not seen to take it yet. A stalled session
    /// with nothing behind it needs no more looks.
    pub fn busy(&self) -> bool {
        self.lines
            .values()
            .any(|line| !line.waiting.is_empty() || line.typed.is_some_and(|typed| !typed.stalled))
    }

    /// Every session with a line, in order.
    pub fn all(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.lines.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// How many messages wait for a session.
    pub fn waiting(&self, id: u64) -> usize {
        self.lines.get(&id).map_or(0, |line| line.waiting.len())
    }

    /// The messages that wait for a session, oldest first.
    pub fn list(&self, id: u64) -> Vec<String> {
        self.lines
            .get(&id)
            .map(|line| line.waiting.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Takes back the message at a place of a session's line, the oldest
    /// being at 0: it will not be typed. It answers the message, or `None`
    /// when there is none there. One that was already typed is not there.
    pub fn cancel(&mut self, id: u64, place: usize) -> Option<String> {
        self.lines.get_mut(&id)?.waiting.remove(place)
    }

    /// Takes back every message that waits for a session. It answers how
    /// many. What was already typed stays typed.
    pub fn clear(&mut self, id: u64) -> usize {
        self.lines
            .get_mut(&id)
            .map_or(0, |line| std::mem::take(&mut line.waiting).len())
    }

    /// Forgets a session: it ended, or can no longer be messaged. It
    /// answers how many messages were dropped.
    pub fn forget(&mut self, id: u64) -> usize {
        self.lines.remove(&id).map_or(0, |line| line.waiting.len())
    }
}

/// What is typed for a message: its text with plain line breaks and without
/// the blanks at its ends. `None` when nothing is left.
pub fn prompt(text: &str) -> Option<String> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// "1 message" or "3 messages".
pub fn count(messages: usize) -> String {
    if messages == 1 {
        "1 message".to_owned()
    } else {
        format!("{messages} messages")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::rgb;
    use leon_history::live::{Beat, ToolKind};
    use std::time::Duration;

    fn facts(activity: Activity) -> Facts {
        Facts {
            id: 7,
            name: "moss".to_owned(),
            tint: rgb(0xd97757).into(),
            agent: true,
            activity,
            paused: false,
            exit: None,
            quiet_for: Some(Duration::from_secs(9)),
        }
    }

    fn pulse(beats: &[Beat]) -> Pulse {
        let mut pulse = Pulse::new();
        for beat in beats {
            pulse.apply(beat);
        }
        pulse
    }

    fn tool(id: &str, name: &str, kind: ToolKind) -> Beat {
        Beat::ToolStarted {
            id: id.to_owned(),
            name: name.to_owned(),
            kind,
            detail: String::new(),
        }
    }

    #[test]
    fn a_message_is_typed_only_at_the_prompt_of_an_agent_whose_turn_is_over() {
        let waiting = facts(Activity::Waiting);
        let over = pulse(&[Beat::Prompt, Beat::TurnEnded]);
        assert_eq!(ready(&waiting, Some(&over)), Ready::Now);
        // The turn is over but the terminal still prints: not yet.
        assert_eq!(ready(&facts(Activity::Working), Some(&over)), Ready::Later);
        assert_eq!(ready(&facts(Activity::Idle), Some(&over)), Ready::Later);
        // The model is at work, quiet terminal or not.
        let thinking = pulse(&[Beat::Prompt, Beat::Thinking]);
        assert_eq!(ready(&waiting, Some(&thinking)), Ready::Later);
    }

    #[test]
    fn a_permission_prompt_and_a_question_are_never_answered_by_a_message() {
        let waiting = facts(Activity::Waiting);
        // A call without a result and a quiet terminal: the permission
        // prompt the Den infers. The message waits.
        let asked = pulse(&[Beat::Prompt, tool("t1", "Bash", ToolKind::Run)]);
        assert_eq!(
            super::super::den::state(&waiting, Some(&asked)).0,
            leon_den::CubState::NeedsPermission
        );
        assert_eq!(ready(&waiting, Some(&asked)), Ready::Later);
        // A question of the agent's own.
        let question = pulse(&[Beat::Prompt, tool("t1", "AskUserQuestion", ToolKind::Ask)]);
        assert_eq!(ready(&waiting, Some(&question)), Ready::Later);
        // Once it is answered and the turn ends, the message goes.
        let answered = pulse(&[
            Beat::Prompt,
            tool("t1", "Bash", ToolKind::Run),
            Beat::ToolFinished {
                id: "t1".to_owned(),
                failed: false,
                refused: false,
            },
            Beat::TurnEnded,
        ]);
        assert_eq!(ready(&waiting, Some(&answered)), Ready::Now);
    }

    #[test]
    fn what_cannot_be_messaged_is_refused_with_its_reason() {
        let over = pulse(&[Beat::TurnEnded]);
        let mut ended = facts(Activity::Waiting);
        ended.exit = Some(0);
        assert_eq!(ready(&ended, Some(&over)), Ready::Never(Never::Ended));
        let mut paused = facts(Activity::Waiting);
        paused.paused = true;
        assert_eq!(ready(&paused, Some(&over)), Ready::Never(Never::Paused));
        let mut shell = facts(Activity::Waiting);
        shell.agent = false;
        assert_eq!(ready(&shell, Some(&over)), Ready::Never(Never::NoAgent));
        // Without a transcript a quiet terminal may be a question.
        assert_eq!(
            ready(&facts(Activity::Waiting), None),
            Ready::Never(Never::Unread)
        );
        for never in [Never::Ended, Never::Paused, Never::NoAgent, Never::Unread] {
            assert!(never.why("MOSS").contains("MOSS"));
        }
        let mut outbox = Outbox::default();
        assert_eq!(
            outbox.post(7, "hello", Ready::Never(Never::Paused), 0),
            Posted::Refused(Never::Paused)
        );
        assert!(!outbox.busy());
    }

    #[test]
    fn a_message_for_a_busy_session_waits_and_they_go_one_a_turn_in_order() {
        let mut outbox = Outbox::default();
        assert_eq!(outbox.post(7, "one", Ready::Later, 0), Posted::Queued(1));
        assert_eq!(outbox.post(7, "two", Ready::Later, 0), Posted::Queued(2));
        assert_eq!(outbox.waiting(7), 2);
        assert_eq!(outbox.all(), vec![7]);
        assert_eq!(outbox.due(7, Ready::Later, 0), Due::Nothing);
        // It waits: the oldest goes.
        assert_eq!(outbox.due(7, Ready::Now, 0), Due::Type("one".to_owned()));
        // The transcript still says the turn is over: the next one holds
        // until the session was seen busy.
        assert_eq!(outbox.due(7, Ready::Now, 0), Due::Nothing);
        assert_eq!(outbox.due(7, Ready::Later, 0), Due::Nothing);
        assert_eq!(outbox.due(7, Ready::Now, 0), Due::Type("two".to_owned()));
        assert_eq!(outbox.waiting(7), 0);
        // Another session's line is its own.
        assert_eq!(outbox.due(8, Ready::Now, 0), Due::Nothing);
    }

    #[test]
    fn a_message_for_a_session_at_its_prompt_goes_at_once_but_never_past_the_line() {
        let mut outbox = Outbox::default();
        assert_eq!(
            outbox.post(7, "now", Ready::Now, 0),
            Posted::Send("now".to_owned())
        );
        // A second one right behind it waits its turn.
        assert_eq!(outbox.post(7, "next", Ready::Now, 0), Posted::Queued(1));
        assert_eq!(outbox.due(7, Ready::Now, 0), Due::Nothing);
        assert_eq!(outbox.due(7, Ready::Later, 0), Due::Nothing);
        assert_eq!(outbox.due(7, Ready::Now, 0), Due::Type("next".to_owned()));
        // With one waiting, a new one never jumps ahead.
        outbox.due(7, Ready::Later, 0);
        assert_eq!(outbox.post(7, "a", Ready::Later, 0), Posted::Queued(1));
        assert_eq!(outbox.post(7, "b", Ready::Now, 0), Posted::Queued(2));
        assert_eq!(outbox.due(7, Ready::Now, 0), Due::Type("a".to_owned()));
    }

    #[test]
    fn a_message_that_was_not_seen_taken_holds_the_line_and_is_said_once() {
        let mut outbox = Outbox::default();
        assert_eq!(
            outbox.post(7, "one", Ready::Now, 3),
            Posted::Send("one".to_owned())
        );
        assert_eq!(outbox.post(7, "two", Ready::Now, 3), Posted::Queued(1));
        assert!(outbox.busy());
        // The session is never seen busy and its transcript holds no new
        // prompt: "one" may still sit in its input, so "two" is not typed
        // after it. That is said once.
        for _ in 1..SETTLE_LOOKS {
            assert_eq!(outbox.due(7, Ready::Now, 3), Due::Nothing);
        }
        assert!(!outbox.stalled(7));
        assert_eq!(outbox.due(7, Ready::Now, 3), Due::Stalled(1));
        assert!(outbox.stalled(7));
        for _ in 0..3 * SETTLE_LOOKS {
            assert_eq!(outbox.due(7, Ready::Now, 3), Due::Nothing);
        }
        // Nor does a new message go over it: it joins the line.
        assert_eq!(outbox.post(7, "three", Ready::Now, 3), Posted::Queued(2));
        assert_eq!(outbox.list(7), ["two", "three"]);
        // The user sends what was in the input: the transcript holds one
        // more prompt, the turn runs, and the line moves again, one a turn.
        assert_eq!(outbox.due(7, Ready::Later, 4), Due::Nothing);
        assert!(!outbox.stalled(7));
        assert_eq!(outbox.due(7, Ready::Now, 4), Due::Type("two".to_owned()));
        assert_eq!(outbox.due(7, Ready::Now, 4), Due::Nothing);
    }

    #[test]
    fn a_turn_too_short_to_be_seen_busy_is_known_by_its_prompt() {
        let mut outbox = Outbox::default();
        assert_eq!(
            outbox.post(7, "one", Ready::Now, 3),
            Posted::Send("one".to_owned())
        );
        assert_eq!(outbox.post(7, "two", Ready::Now, 3), Posted::Queued(1));
        assert_eq!(outbox.due(7, Ready::Now, 3), Due::Nothing);
        // The session was at its prompt at every look, but its transcript
        // holds the prompt now: "one" was taken, and answered already.
        assert_eq!(outbox.due(7, Ready::Now, 4), Due::Type("two".to_owned()));
    }

    #[test]
    fn a_stalled_session_with_nothing_behind_it_needs_no_more_looks() {
        let mut outbox = Outbox::default();
        outbox.post(7, "one", Ready::Now, 0);
        for _ in 1..SETTLE_LOOKS {
            assert!(outbox.busy());
            outbox.due(7, Ready::Now, 0);
        }
        assert_eq!(outbox.due(7, Ready::Now, 0), Due::Stalled(0));
        assert!(!outbox.busy());
        // The next message does not go over it either, until it was taken.
        assert_eq!(outbox.post(7, "two", Ready::Now, 0), Posted::Queued(1));
        assert!(outbox.busy());
        assert_eq!(
            outbox.post(8, "x", Ready::Now, 0),
            Posted::Send("x".to_owned())
        );
        assert_eq!(outbox.due(7, Ready::Now, 1), Due::Type("two".to_owned()));
    }

    #[test]
    fn a_session_that_ends_loses_its_line() {
        let mut outbox = Outbox::default();
        outbox.post(7, "one", Ready::Later, 0);
        outbox.post(7, "two", Ready::Later, 0);
        assert_eq!(outbox.forget(7), 2);
        assert_eq!(outbox.forget(7), 0);
        assert_eq!(outbox.due(7, Ready::Now, 0), Due::Nothing);
    }

    #[test]
    fn the_messages_that_wait_can_be_read_and_taken_back() {
        let mut outbox = Outbox::default();
        assert!(outbox.list(7).is_empty());
        for text in ["one", "two", "three"] {
            outbox.post(7, text, Ready::Later, 0);
        }
        assert_eq!(outbox.list(7), ["one", "two", "three"]);
        // One is taken back: the others keep their order.
        assert_eq!(outbox.cancel(7, 1).as_deref(), Some("two"));
        assert_eq!(outbox.cancel(7, 5), None);
        assert_eq!(outbox.cancel(9, 0), None);
        assert_eq!(outbox.list(7), ["one", "three"]);
        assert_eq!(outbox.due(7, Ready::Now, 0), Due::Type("one".to_owned()));
        // The rest at once; what was typed stays typed, and the line is
        // still watched until the session was seen busy.
        assert_eq!(outbox.clear(7), 1);
        assert_eq!(outbox.clear(7), 0);
        assert_eq!(outbox.waiting(7), 0);
        assert!(outbox.busy());
        outbox.due(7, Ready::Later, 0);
        assert!(!outbox.busy());
        assert_eq!(outbox.due(7, Ready::Now, 0), Due::Nothing);
    }

    #[test]
    fn a_message_is_typed_trimmed_and_an_empty_one_is_none() {
        assert_eq!(
            prompt("  run the tests \n").as_deref(),
            Some("run the tests")
        );
        assert_eq!(prompt("a\r\nb\rc").as_deref(), Some("a\nb\nc"));
        assert_eq!(prompt(" \n "), None);
        assert_eq!(count(1), "1 message");
        assert_eq!(count(3), "3 messages");
    }
}

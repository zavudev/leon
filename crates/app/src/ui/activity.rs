//! How the sessions of a worktree are doing, as one dot.
//!
//! [`terminal_activity`] is a pure function of what a terminal already says
//! about itself: whether it ended and how, how long it has been quiet, whether
//! the shell or a program is in front of it, whether the bell rang. The
//! states, most urgent first:
//!
//! | State | Meaning | Dot |
//! | --- | --- | --- |
//! | [`Activity::Failed`] | exited with a non-zero status, not yet looked at | error colour |
//! | [`Activity::NeedsYou`] | the agent's transcript has a tool call without a result and the terminal is quiet (a permission prompt, inferred), or the call is a question | warning colour, ringed |
//! | [`Activity::Waiting`] | a program is in front but quiet beyond the threshold, or the bell rang, and nothing better is known | warning colour |
//! | [`Activity::TurnOver`] | the agent's transcript says its turn ended: your move | info colour |
//! | [`Activity::Working`] | a program is in front and printed recently, or the transcript says a turn is going on | success colour, ringed |
//! | [`Activity::Idle`] | at the shell prompt, or a plain shell | success colour |
//! | [`Activity::Off`] | nothing live | hollow, grey |
//!
//! A worktree shows the most urgent state of its terminals ([`Activity::most_urgent`]);
//! a project the most urgent of its worktrees.
//!
//! # Two sources, one decision
//!
//! [`terminal_activity`] is the *heuristic*: it only knows what a terminal says
//! about itself. [`session_activity`] refines its answer with
//! [`Transcript`], what the agent's own transcript says (see
//! `leon_history::live`), when that is known. The precedence, top wins:
//!
//! 1. The terminal ended, or the shell is in front, or it is not an agent, or
//!    the terminal cannot tell the foreground (Windows, SSH): the heuristic
//!    (the transcript of an agent that has quit says nothing about the
//!    terminal). See [`transcript_applies`].
//! 2. No transcript, or nothing read from it yet: the heuristic.
//! 3. The turn is over: [`Activity::TurnOver`], even while the terminal
//!    prints (that is the user typing) and even if the bell rang.
//! 4. A question or a plan to approve is among the calls in flight, last or
//!    not: [`Activity::NeedsYou`], a fact.
//! 5. Any other tool call without a result: the transcript cannot tell a
//!    tool that runs from one that waits for a permission. A quiet terminal
//!    (or the bell) reads [`Activity::NeedsYou`], **inferred**; a terminal
//!    that prints reads [`Activity::Working`].
//! 6. A turn is going on and no tool is open (the model thinks or writes):
//!    [`Activity::Working`], however quiet the terminal is and whatever the
//!    bell did.
//!
//! The transcript is only followed for Claude Code and Codex sessions of
//! this computer on a system where the foreground program can be told (not
//! Windows), whose session id Leon knows for sure; every other session
//! (another agent, SSH, Windows) has no [`Transcript`] and reads the
//! heuristic alone.
//!
//! Over SSH the terminal's process is `ssh` whatever runs on the other side,
//! so whether a program is in front cannot be known: only output activity is
//! used. A session started as an agent then reads *working* while it prints
//! and *waiting* once quiet (including after the agent has returned to the
//! prompt, which cannot be told); a plain shell reads *working* while it
//! prints and *idle* otherwise.

use std::time::Duration;

use leon_history::live::Pulse;

/// How a terminal, a worktree or a project is doing. The order is the
/// urgency: the later, the more urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Activity {
    /// No live terminal.
    Off,
    /// At the prompt, or a plain shell.
    Idle,
    /// A program is running and printing, or its transcript says a turn is
    /// going on.
    Working,
    /// The agent's transcript says it finished its turn: the user's move.
    TurnOver,
    /// A program is running and quiet, or the bell rang: it wants the user,
    /// and nothing tells whether it finished or asks.
    Waiting,
    /// The agent asks something: a question, or (inferred) a permission.
    NeedsYou,
    /// A terminal ended with a failure the user has not looked at.
    Failed,
}

impl Activity {
    /// The more urgent of two states.
    pub fn most_urgent(self, other: Self) -> Self {
        self.max(other)
    }

    /// Whether the session waits for the user, for whatever reason: it
    /// finished, it asks, or nothing tells which. A failure is not counted.
    pub fn wants_you(self) -> bool {
        matches!(self, Self::TurnOver | Self::Waiting | Self::NeedsYou)
    }

    /// Whether coming to read as `self` after `before` is news the user is
    /// told: a session came to want them. Moving from one state that wants
    /// them to another is not (it was already said), except an agent that
    /// asked and then, the question answered, finished its turn.
    pub fn is_news_after(self, before: Self) -> bool {
        self != before
            && self.wants_you()
            && (!before.wants_you() || (before == Self::NeedsYou && self == Self::TurnOver))
    }

    /// What the dot's tooltip says.
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Off => "No live terminal",
            Self::Idle => "Live, at the prompt",
            Self::Working => "Working: a program is running, or the agent is in a turn",
            Self::TurnOver => "Finished its turn: your move",
            Self::Waiting => "Waiting for you: a program is quiet, or the bell rang",
            Self::NeedsYou => {
                "Needs you: a question, or a tool call without a result in a quiet terminal (a permission prompt, inferred)"
            }
            Self::Failed => "A terminal exited with an error",
        }
    }
}

/// Where the thresholds are; tests replace them so nothing waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// How long a program in front of a terminal may be quiet before it reads
    /// as waiting for the user.
    pub waiting_after: Duration,
    /// How often live terminals are looked at while any is live, so that a
    /// program going quiet is noticed without any output to wake the window.
    pub tick: Duration,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            waiting_after: Duration::from_secs(6),
            tick: Duration::from_secs(2),
        }
    }
}

/// What a terminal says about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signals {
    /// The exit code, once the program ended.
    pub exited: Option<u32>,
    /// Whether the user has looked at the terminal since it failed.
    pub failure_seen: bool,
    /// Whether the shell has the terminal in front (`Some(false)`: a program
    /// does), or `None` where that cannot be told (over SSH, on Windows).
    pub shell_in_front: Option<bool>,
    /// Whether it was started as an agent (and not as a plain shell).
    pub agent: bool,
    /// How long since it last printed; `None` before the first output.
    pub quiet_for: Option<Duration>,
    /// Whether the bell rang since the user last looked.
    pub bell: bool,
}

/// How one terminal is doing, from the terminal alone: the heuristic.
pub fn terminal_activity(signals: &Signals, thresholds: &Thresholds) -> Activity {
    if let Some(code) = signals.exited {
        return if code != 0 && !signals.failure_seen {
            Activity::Failed
        } else {
            Activity::Off
        };
    }
    if signals.bell {
        return Activity::Waiting;
    }
    let printing = signals
        .quiet_for
        .is_none_or(|quiet| quiet < thresholds.waiting_after);
    match signals.shell_in_front {
        Some(true) => Activity::Idle,
        Some(false) if printing => Activity::Working,
        Some(false) => Activity::Waiting,
        None if printing && signals.quiet_for.is_some() => Activity::Working,
        None if signals.agent && !printing => Activity::Waiting,
        None => Activity::Idle,
    }
}

/// What an agent's own transcript says it is doing, as far as it was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transcript {
    /// A turn is going on and no tool call is open: the model reads, thinks
    /// or writes.
    Turn,
    /// A tool call has no result yet. `asks`: by its nature the call is a
    /// question for the user or a plan to approve. A transcript cannot tell
    /// a tool that runs from one that waits for a permission.
    Call {
        /// The call is a question or a plan.
        asks: bool,
    },
    /// The turn is over.
    TurnOver,
}

impl Transcript {
    /// What a pulse says. `None` before anything was read from the
    /// transcript: a pulse that saw no beat reports its turn over, which says
    /// nothing about the session. A pulse that did not see where the turn
    /// began says as little (see `transcript_watch::Reading`).
    pub fn of(pulse: &Pulse) -> Option<Self> {
        pulse.last()?;
        Some(if pulse.tools().is_empty() {
            if pulse.turn_over() {
                Self::TurnOver
            } else {
                Self::Turn
            }
        } else {
            // Any call asking is a question to answer, not only the newest.
            Self::Call {
                asks: pulse.tools().iter().any(super::den::asks),
            }
        })
    }
}

/// Whether the transcript of the agent in a terminal can say anything about
/// it: the terminal is live, an agent runs in it and the terminal can tell
/// that the agent, and not the shell, is in front. Where the foreground
/// cannot be told (Windows, SSH) it is false, and so the transcript is not
/// even read, for nothing it said would be used.
pub fn transcript_applies(signals: &Signals) -> bool {
    signals.exited.is_none() && signals.agent && signals.shell_in_front == Some(false)
}

/// How one session is doing: the terminal's `heuristic` refined by what the
/// agent's `transcript` says, when it is known. See the precedence in the
/// module's documentation.
pub fn session_activity(
    heuristic: Activity,
    signals: &Signals,
    transcript: Option<Transcript>,
) -> Activity {
    let Some(transcript) = transcript else {
        return heuristic;
    };
    // Only an agent in front of the terminal has something to say: after it
    // quit, or in a plain shell, the transcript is history.
    if !transcript_applies(signals) {
        return heuristic;
    }
    match transcript {
        Transcript::TurnOver => Activity::TurnOver,
        Transcript::Call { asks: true } => Activity::NeedsYou,
        // Quiet or the bell: the heuristic reads waiting.
        Transcript::Call { asks: false } if heuristic == Activity::Waiting => Activity::NeedsYou,
        Transcript::Call { asks: false } => heuristic,
        Transcript::Turn => Activity::Working,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Thresholds = Thresholds {
        waiting_after: Duration::from_secs(6),
        tick: Duration::from_secs(2),
    };

    fn local(front: bool, quiet: Option<u64>) -> Signals {
        Signals {
            exited: None,
            failure_seen: false,
            shell_in_front: Some(front),
            agent: true,
            quiet_for: quiet.map(Duration::from_secs),
            bell: false,
        }
    }

    /// The session as the window decides it: the heuristic, then the transcript.
    fn decide(signals: &Signals, transcript: Option<Transcript>) -> Activity {
        session_activity(terminal_activity(signals, &T), signals, transcript)
    }

    fn ssh(agent: bool, quiet: Option<u64>) -> Signals {
        Signals {
            shell_in_front: None,
            agent,
            ..local(false, quiet)
        }
    }

    #[test]
    fn a_program_that_printed_recently_is_working_and_a_quiet_one_is_waiting() {
        assert_eq!(
            terminal_activity(&local(false, Some(1)), &T),
            Activity::Working
        );
        assert_eq!(
            terminal_activity(&local(false, Some(5)), &T),
            Activity::Working
        );
        assert_eq!(
            terminal_activity(&local(false, Some(6)), &T),
            Activity::Waiting
        );
        assert_eq!(
            terminal_activity(&local(false, Some(600)), &T),
            Activity::Waiting
        );
    }

    #[test]
    fn a_program_that_just_started_and_has_printed_nothing_yet_is_working() {
        assert_eq!(
            terminal_activity(&local(false, None), &T),
            Activity::Working
        );
    }

    #[test]
    fn the_shell_in_front_is_idle_however_long_it_has_been_quiet() {
        assert_eq!(terminal_activity(&local(true, Some(1)), &T), Activity::Idle);
        assert_eq!(
            terminal_activity(&local(true, Some(9_999)), &T),
            Activity::Idle
        );
    }

    #[test]
    fn the_bell_means_waiting_until_the_user_looks() {
        let mut rang = local(false, Some(1));
        rang.bell = true;
        assert_eq!(terminal_activity(&rang, &T), Activity::Waiting);
        rang.shell_in_front = Some(true);
        assert_eq!(terminal_activity(&rang, &T), Activity::Waiting);
    }

    #[test]
    fn a_failure_is_an_error_until_seen_and_a_clean_exit_is_not_live() {
        let mut failed = local(true, Some(1));
        failed.exited = Some(2);
        assert_eq!(terminal_activity(&failed, &T), Activity::Failed);
        failed.failure_seen = true;
        assert_eq!(terminal_activity(&failed, &T), Activity::Off);
        failed.exited = Some(0);
        failed.failure_seen = false;
        assert_eq!(terminal_activity(&failed, &T), Activity::Off);
    }

    #[test]
    fn an_exit_outranks_the_bell() {
        let mut ended = local(false, Some(1));
        ended.bell = true;
        ended.exited = Some(1);
        assert_eq!(terminal_activity(&ended, &T), Activity::Failed);
    }

    #[test]
    fn over_ssh_only_output_counts() {
        assert_eq!(
            terminal_activity(&ssh(true, Some(1)), &T),
            Activity::Working
        );
        assert_eq!(
            terminal_activity(&ssh(true, Some(60)), &T),
            Activity::Waiting
        );
        assert_eq!(
            terminal_activity(&ssh(false, Some(1)), &T),
            Activity::Working
        );
        assert_eq!(terminal_activity(&ssh(false, Some(60)), &T), Activity::Idle);
        assert_eq!(
            terminal_activity(&ssh(true, None), &T),
            Activity::Idle,
            "nothing printed yet"
        );
    }

    #[test]
    fn the_most_urgent_state_wins_in_the_documented_order() {
        use Activity::*;
        let order = [Off, Idle, Working, TurnOver, Waiting, NeedsYou, Failed];
        for (at, urgent) in order.iter().enumerate() {
            for lesser in &order[..at] {
                assert_eq!(urgent.most_urgent(*lesser), *urgent);
                assert_eq!(lesser.most_urgent(*urgent), *urgent);
            }
        }
        assert_eq!(
            [Idle, Working, Waiting]
                .into_iter()
                .fold(Off, Activity::most_urgent),
            Waiting
        );
        assert_eq!(
            [TurnOver, Working, Idle]
                .into_iter()
                .fold(Off, Activity::most_urgent),
            TurnOver
        );
        assert_eq!(NeedsYou.most_urgent(TurnOver), NeedsYou);
        assert_eq!(
            [Idle, Working].into_iter().fold(Off, Activity::most_urgent),
            Working
        );
    }

    #[test]
    fn every_state_has_a_tooltip() {
        for state in [
            Activity::Off,
            Activity::Idle,
            Activity::Working,
            Activity::TurnOver,
            Activity::Waiting,
            Activity::NeedsYou,
            Activity::Failed,
        ] {
            assert!(!state.tooltip().is_empty());
        }
        assert!(
            Activity::NeedsYou.tooltip().contains("inferred"),
            "a permission prompt is a guess and the tooltip says so"
        );
    }

    #[test]
    fn the_states_that_want_the_user_are_the_three_waits_and_not_a_failure() {
        use Activity::*;
        for state in [TurnOver, Waiting, NeedsYou] {
            assert!(state.wants_you(), "{state:?}");
        }
        for state in [Off, Idle, Working, Failed] {
            assert!(!state.wants_you(), "{state:?}");
        }
    }

    #[test]
    fn coming_to_want_the_user_is_news_once() {
        use Activity::*;
        // From anything that does not want them.
        for before in [Off, Idle, Working, Failed] {
            for now in [TurnOver, Waiting, NeedsYou] {
                assert!(now.is_news_after(before), "{before:?} -> {now:?}");
            }
        }
        // Not a state that does not want them, and not the same state.
        for now in [Off, Idle, Working, Failed] {
            assert!(!now.is_news_after(Working), "{now:?}");
        }
        for state in [TurnOver, Waiting, NeedsYou] {
            assert!(!state.is_news_after(state));
        }
        // Between two that do: said already, but for the end of an answered
        // question.
        assert!(!TurnOver.is_news_after(Waiting));
        assert!(!NeedsYou.is_news_after(Waiting));
        assert!(!Waiting.is_news_after(TurnOver));
        assert!(!Waiting.is_news_after(NeedsYou));
        assert!(!NeedsYou.is_news_after(TurnOver));
        assert!(TurnOver.is_news_after(NeedsYou));
    }

    use Transcript::{Call, Turn, TurnOver as Over};

    /// Every row: the terminal (a program in front, quiet for this many
    /// seconds, the bell), what the transcript says, and what the dot reads.
    #[test]
    fn the_transcript_refines_the_heuristic_in_the_documented_order() {
        use Activity::*;
        let rows: &[(Option<u64>, bool, Option<Transcript>, Activity)] = &[
            // Nothing read: the heuristic, unchanged.
            (Some(1), false, None, Working),
            (Some(60), false, None, Waiting),
            (Some(1), true, None, Waiting),
            // The turn is over: your move, printing or not, bell or not.
            (Some(1), false, Some(Over), Activity::TurnOver),
            (Some(60), false, Some(Over), Activity::TurnOver),
            (Some(60), true, Some(Over), Activity::TurnOver),
            // A turn with no tool open is work, however quiet, bell or not.
            (Some(1), false, Some(Turn), Working),
            (Some(600), false, Some(Turn), Working),
            (Some(600), true, Some(Turn), Working),
            (None, false, Some(Turn), Working),
            // A question is a fact: it needs you at once.
            (Some(1), false, Some(Call { asks: true }), NeedsYou),
            (Some(60), false, Some(Call { asks: true }), NeedsYou),
            // Another call: running while the terminal prints, a permission
            // prompt (inferred) once it is quiet or rang.
            (Some(1), false, Some(Call { asks: false }), Working),
            (None, false, Some(Call { asks: false }), Working),
            (Some(5), false, Some(Call { asks: false }), Working),
            (Some(6), false, Some(Call { asks: false }), NeedsYou),
            (Some(60), false, Some(Call { asks: false }), NeedsYou),
            (Some(1), true, Some(Call { asks: false }), NeedsYou),
        ];
        for (quiet, bell, transcript, expected) in rows {
            let mut signals = local(false, *quiet);
            signals.bell = *bell;
            assert_eq!(
                decide(&signals, *transcript),
                *expected,
                "quiet {quiet:?}, bell {bell}, {transcript:?}"
            );
        }
    }

    #[test]
    fn the_transcript_of_an_agent_that_is_not_in_front_says_nothing() {
        let said = [Over, Turn, Call { asks: true }, Call { asks: false }];
        for transcript in said {
            // The shell has the terminal back: the agent quit.
            assert_eq!(
                decide(&local(true, Some(60)), Some(transcript)),
                Activity::Idle,
                "{transcript:?}"
            );
            // A plain shell, not an agent.
            let mut shell = local(false, Some(1));
            shell.agent = false;
            assert_eq!(
                decide(&shell, Some(transcript)),
                terminal_activity(&shell, &T),
                "{transcript:?}"
            );
            // Where the foreground cannot be told (over SSH, on Windows).
            assert_eq!(
                decide(&ssh(true, Some(60)), Some(transcript)),
                terminal_activity(&ssh(true, Some(60)), &T),
                "{transcript:?}"
            );
            // An exit outranks everything.
            let mut ended = local(false, Some(1));
            ended.exited = Some(2);
            assert_eq!(decide(&ended, Some(transcript)), Activity::Failed);
            ended.failure_seen = true;
            assert_eq!(decide(&ended, Some(transcript)), Activity::Off);
        }
    }

    #[test]
    fn a_transcript_is_only_read_where_an_agent_in_front_can_be_told() {
        assert!(transcript_applies(&local(false, Some(1))));
        // The shell has the terminal back.
        assert!(!transcript_applies(&local(true, Some(1))));
        // The foreground cannot be told: Windows, or SSH. The transcript
        // would be thrown away, so it is not read.
        assert!(!transcript_applies(&ssh(true, Some(1))));
        // Not an agent, or ended.
        let mut shell = local(false, Some(1));
        shell.agent = false;
        assert!(!transcript_applies(&shell));
        let mut ended = local(false, Some(1));
        ended.exited = Some(0);
        assert!(!transcript_applies(&ended));
    }

    #[test]
    fn a_question_is_a_question_even_when_it_is_not_the_newest_call() {
        use leon_history::live::{Beat, ToolKind};
        let started = |id: &str, name: &str| Beat::ToolStarted {
            id: id.into(),
            name: name.into(),
            kind: ToolKind::of(name),
            detail: String::new(),
        };
        let mut pulse = Pulse::new();
        for beat in [
            Beat::Prompt,
            started("q", "AskUserQuestion"),
            started("b", "Bash"),
        ] {
            pulse.apply(&beat);
        }
        assert_eq!(Transcript::of(&pulse), Some(Call { asks: true }));
        // Answered, what is left is a call that runs.
        pulse.apply(&Beat::ToolFinished {
            id: "q".into(),
            failed: false,
            refused: false,
        });
        assert_eq!(Transcript::of(&pulse), Some(Call { asks: false }));
    }

    #[test]
    fn a_pulse_says_nothing_until_a_beat_was_read_and_then_what_its_last_beats_say() {
        use leon_history::live::{Beat, ToolKind};
        let started = |id: &str, name: &str| Beat::ToolStarted {
            id: id.into(),
            name: name.into(),
            kind: ToolKind::of(name),
            detail: String::new(),
        };
        let apply = |beats: &[Beat]| {
            let mut pulse = Pulse::new();
            for beat in beats {
                pulse.apply(beat);
            }
            Transcript::of(&pulse)
        };
        assert_eq!(apply(&[]), None, "a new pulse reports its turn over");
        assert_eq!(apply(&[Beat::Prompt, Beat::Thinking]), Some(Turn));
        assert_eq!(
            apply(&[Beat::Prompt, started("t1", "Bash")]),
            Some(Call { asks: false })
        );
        assert_eq!(
            apply(&[Beat::Prompt, started("t1", "AskUserQuestion")]),
            Some(Call { asks: true })
        );
        assert_eq!(
            apply(&[Beat::Prompt, started("t1", "ExitPlanMode")]),
            Some(Call { asks: true })
        );
        let finished = Beat::ToolFinished {
            id: "t1".into(),
            failed: false,
            refused: false,
        };
        assert_eq!(
            apply(&[Beat::Prompt, started("t1", "Bash"), finished]),
            Some(Turn),
            "the result is in and the model decides"
        );
        assert_eq!(
            apply(&[Beat::Prompt, started("t1", "Bash"), Beat::TurnEnded]),
            Some(Over),
            "a turn that ended leaves no call open"
        );
        assert_eq!(apply(&[Beat::Prompt, Beat::Interrupted]), Some(Over));
    }
}

//! When a project may ask GitHub about the pull requests of its branches.
//!
//! The answer is what puts a merged mark on a worktree, so the person who just
//! merged wants it quickly, and the person who left the window for the night
//! does not want a `gh` call per project every few seconds. The decision is a
//! pure function of what the project asked last, the clock and why it is
//! asking, so each rule is tested without a runner.
//!
//! * a **look** (the timer, a sync) asks every [`FOCUSED_EVERY`] while the
//!   window is in front, and every [`AWAY_EVERY`] while it is not;
//! * an **event** that probably changed the answer (the window came back to
//!   the front, a head moved, a turn ended, a pull request was opened) asks as
//!   soon as [`EVENT_GAP`] has passed since the last question, whatever the
//!   cadence says: the same event twice in a row is one question;
//! * a project whose last question failed (no `gh`, signed out, offline) backs
//!   off, doubling from [`FAILURE_BASE`] up to [`FAILURE_CAP`], and an event
//!   waits for [`FAILING_EVENT_GAP`] instead, so a missing `gh` costs one
//!   failed spawn a minute at most and never a line of noise.

use std::time::{Duration, Instant};

/// How often a project asks while the window is in front.
pub const FOCUSED_EVERY: Duration = Duration::from_secs(30);

/// How often a project asks while the window is behind others or minimised.
pub const AWAY_EVERY: Duration = Duration::from_secs(300);

/// The least time between two questions when something that changes the
/// answer happened.
pub const EVENT_GAP: Duration = Duration::from_secs(5);

/// The least time between two event questions of a project whose last
/// question failed.
pub const FAILING_EVENT_GAP: Duration = Duration::from_secs(60);

/// The wait after a first failure; every failure after it doubles it.
pub const FAILURE_BASE: Duration = Duration::from_secs(30);

/// The longest wait a failing project backs off to.
pub const FAILURE_CAP: Duration = Duration::from_secs(900);

/// Why a project is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The timer, or somebody opening the project: ask when it is time, the
    /// cadence depending on whether the window is in front.
    Look {
        /// Whether the window is the one in front.
        focused: bool,
    },
    /// Something that probably changed the answer happened.
    Event,
}

/// What a project knows about its last question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asked {
    /// When it was asked.
    pub at: Instant,
    /// How many questions in a row failed, until one answered.
    pub failures: u32,
}

/// How long to wait after the last question before a look asks again.
pub fn look_interval(focused: bool, failures: u32) -> Duration {
    let cadence = if focused { FOCUSED_EVERY } else { AWAY_EVERY };
    if failures == 0 {
        return cadence;
    }
    let exponent = failures.saturating_sub(1).min(16);
    let backoff = FAILURE_BASE
        .saturating_mul(2u32.saturating_pow(exponent))
        .min(FAILURE_CAP);
    cadence.max(backoff)
}

/// Whether a project whose last question was `last` may ask at `now`, for
/// `why`. A project that never asked always may.
pub fn due(last: Option<Asked>, now: Instant, why: Why) -> bool {
    let Some(last) = last else {
        return true;
    };
    let since = now.saturating_duration_since(last.at);
    let wait = match why {
        Why::Look { focused } => look_interval(focused, last.failures),
        Why::Event if last.failures == 0 => EVENT_GAP,
        Why::Event => FAILING_EVENT_GAP,
    };
    since >= wait
}

/// What the project knows after a question: its time, and how many in a row
/// failed (`failed` adds one, an answer starts again from none).
pub fn after_question(before: Option<Asked>, at: Instant, failed: bool) -> Asked {
    Asked {
        at,
        failures: if failed {
            before.map_or(0, |asked| asked.failures).saturating_add(1)
        } else {
            0
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> Instant {
        // One fixed origin for the whole test, far enough from the start of
        // the clock that nothing here subtracts below it.
        thread_local! {
            static ORIGIN: Instant = Instant::now();
        }
        ORIGIN.with(|origin| *origin + Duration::from_secs(seconds))
    }

    fn asked(seconds: u64, failures: u32) -> Option<Asked> {
        Some(Asked {
            at: at(seconds),
            failures,
        })
    }

    const FRONT: Why = Why::Look { focused: true };
    const BEHIND: Why = Why::Look { focused: false };

    #[test]
    fn a_project_that_never_asked_asks_whatever_the_reason() {
        for why in [FRONT, BEHIND, Why::Event] {
            assert!(due(None, at(0), why), "{why:?}");
        }
    }

    #[test]
    fn a_look_asks_every_half_minute_in_front_and_every_five_minutes_behind() {
        assert!(!due(asked(0, 0), at(29), FRONT));
        assert!(due(asked(0, 0), at(30), FRONT));
        assert!(!due(asked(0, 0), at(299), BEHIND));
        assert!(due(asked(0, 0), at(300), BEHIND));
        // A look right after a question is never due.
        assert!(!due(asked(10, 0), at(10), FRONT));
    }

    #[test]
    fn an_event_asks_at_once_unless_the_last_question_was_a_moment_ago() {
        assert!(!due(asked(100, 0), at(100), Why::Event));
        assert!(!due(asked(100, 0), at(104), Why::Event));
        assert!(due(asked(100, 0), at(105), Why::Event));
        // Far sooner than the cadence of a look in front, and of one behind.
        assert!(due(asked(100, 0), at(110), Why::Event));
        assert!(!due(asked(100, 0), at(110), FRONT));
        assert!(!due(asked(100, 0), at(110), BEHIND));
    }

    #[test]
    fn a_clock_that_went_back_asks_nobody() {
        assert!(!due(asked(50, 0), at(10), FRONT));
        assert!(!due(asked(50, 0), at(10), Why::Event));
    }

    #[test]
    fn a_failing_project_backs_off_by_doubling_up_to_the_cap() {
        let wait = |failures| look_interval(true, failures);
        assert_eq!(wait(0), FOCUSED_EVERY);
        // The first failures are inside the cadence of a look in front.
        assert_eq!(wait(1), Duration::from_secs(30));
        assert_eq!(wait(2), Duration::from_secs(60));
        assert_eq!(wait(3), Duration::from_secs(120));
        assert_eq!(wait(4), Duration::from_secs(240));
        assert_eq!(wait(5), Duration::from_secs(480));
        assert_eq!(wait(6), FAILURE_CAP);
        assert_eq!(wait(60), FAILURE_CAP);
        assert_eq!(wait(u32::MAX), FAILURE_CAP);
        // Behind the window the longer of the two stands.
        assert_eq!(look_interval(false, 1), AWAY_EVERY);
        assert_eq!(look_interval(false, 6), FAILURE_CAP);
    }

    #[test]
    fn a_failing_project_is_asked_again_only_when_its_wait_is_over() {
        assert!(!due(asked(0, 3), at(119), FRONT));
        assert!(due(asked(0, 3), at(120), FRONT));
    }

    #[test]
    fn an_event_does_not_wake_a_failing_project_more_than_once_a_minute() {
        assert!(!due(asked(0, 4), at(59), Why::Event));
        assert!(due(asked(0, 4), at(60), Why::Event));
        // However long the backoff has grown.
        assert!(due(asked(0, 40), at(60), Why::Event));
    }

    #[test]
    fn failures_count_in_a_row_and_an_answer_starts_again() {
        let first = after_question(None, at(0), true);
        assert_eq!(first.failures, 1);
        let second = after_question(Some(first), at(30), true);
        assert_eq!(second.failures, 2);
        assert_eq!(second.at, at(30));
        let healed = after_question(Some(second), at(90), false);
        assert_eq!(healed.failures, 0);
        let first_ever_answer = after_question(None, at(0), false);
        assert_eq!(first_ever_answer.failures, 0);
        let capped = after_question(
            Some(Asked {
                at: at(0),
                failures: u32::MAX,
            }),
            at(1),
            true,
        );
        assert_eq!(capped.failures, u32::MAX, "no overflow");
    }
}

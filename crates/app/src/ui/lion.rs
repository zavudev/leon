//! How the lion of the header feels: the mood of the whole application.
//!
//! The mark is Leon's one face, so it carries the state of the work. Most
//! urgent first:
//!
//! | Mood | When |
//! | --- | --- |
//! | `Asleep` | the window does not have the focus: nobody is looking |
//! | `Error` | a session just exited with an error, for a few seconds |
//! | `Working` | any session is working: a program is running and printing |
//! | `Waiting` | else any session wants the user: quiet, or its bell rang |
//! | `Idle` | otherwise |
//!
//! A pure function of what the shell already knows, so each rule is tested
//! without a window.

use leon_mark::Mood;

use super::activity::Activity;

/// The mood of the lion: `window_active` is whether the window has the
/// focus, `error_flash` whether a session ended with an error a moment ago,
/// and `activities` how each live session is doing.
pub fn mood(
    window_active: bool,
    error_flash: bool,
    activities: impl IntoIterator<Item = Activity>,
) -> Mood {
    if !window_active {
        return Mood::Asleep;
    }
    if error_flash {
        return Mood::Error;
    }
    let (mut working, mut waiting) = (false, false);
    for activity in activities {
        working |= activity == Activity::Working;
        waiting |= activity == Activity::Waiting;
    }
    if working {
        Mood::Working
    } else if waiting {
        Mood::Waiting
    } else {
        Mood::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Activity::{Failed, Idle, Off, Waiting, Working};

    #[test]
    fn with_nothing_live_the_lion_is_idle() {
        assert_eq!(mood(true, false, []), Mood::Idle);
        assert_eq!(mood(true, false, [Off, Idle]), Mood::Idle);
    }

    #[test]
    fn any_working_session_makes_the_lion_work_even_beside_a_waiting_one() {
        assert_eq!(mood(true, false, [Idle, Working]), Mood::Working);
        assert_eq!(mood(true, false, [Waiting, Working, Off]), Mood::Working);
    }

    #[test]
    fn without_work_a_session_that_wants_the_user_makes_the_lion_wait() {
        assert_eq!(mood(true, false, [Idle, Waiting]), Mood::Waiting);
    }

    #[test]
    fn a_failure_the_user_has_not_looked_at_is_the_dot_s_business_and_not_the_lions_for_long() {
        assert_eq!(mood(true, false, [Failed]), Mood::Idle);
    }

    #[test]
    fn a_fresh_error_beats_the_work_going_on() {
        assert_eq!(mood(true, true, [Working]), Mood::Error);
        assert_eq!(mood(true, true, []), Mood::Error);
    }

    #[test]
    fn a_window_without_the_focus_puts_the_lion_to_sleep_whatever_else_is_going_on() {
        assert_eq!(mood(false, false, [Working]), Mood::Asleep);
        assert_eq!(mood(false, true, [Working, Waiting]), Mood::Asleep);
        assert_eq!(mood(false, false, []), Mood::Asleep);
    }
}

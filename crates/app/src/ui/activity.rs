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
//! | [`Activity::Waiting`] | a program is in front but quiet beyond the threshold, or the bell rang | warning colour |
//! | [`Activity::Working`] | a program is in front and printed recently | success colour, ringed |
//! | [`Activity::Idle`] | at the shell prompt, or a plain shell | success colour |
//! | [`Activity::Off`] | nothing live | hollow, grey |
//!
//! A worktree shows the most urgent state of its terminals ([`Activity::most_urgent`]);
//! a project the most urgent of its worktrees.
//!
//! Over SSH the terminal's process is `ssh` whatever runs on the other side,
//! so whether a program is in front cannot be known: only output activity is
//! used. A session started as an agent then reads *working* while it prints
//! and *waiting* once quiet (including after the agent has returned to the
//! prompt, which cannot be told); a plain shell reads *working* while it
//! prints and *idle* otherwise.

use std::time::Duration;

/// How a terminal, a worktree or a project is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Activity {
    /// No live terminal.
    Off,
    /// At the prompt, or a plain shell.
    Idle,
    /// A program is running and printing.
    Working,
    /// A program is running and quiet, or the bell rang: it wants the user.
    Waiting,
    /// A terminal ended with a failure the user has not looked at.
    Failed,
}

impl Activity {
    /// The more urgent of two states.
    pub fn most_urgent(self, other: Self) -> Self {
        self.max(other)
    }

    /// What the dot's tooltip says.
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Off => "No live terminal",
            Self::Idle => "Live, at the prompt",
            Self::Working => "Working: a program is running and printing",
            Self::Waiting => "Waiting for you: a program is quiet, or the bell rang",
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

/// How one terminal is doing.
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
        let order = [Off, Idle, Working, Waiting, Failed];
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
            Activity::Waiting,
            Activity::Failed,
        ] {
            assert!(!state.tooltip().is_empty());
        }
    }
}

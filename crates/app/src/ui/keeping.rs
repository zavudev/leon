//! Keeping sessions running: what is decided without a window.
//!
//! With the setting `durable_sessions` on, the terminals of local sessions are
//! held by the keeper (`durable.rs`) and outlive the window. Three decisions
//! belong to that, and none needs a window:
//!
//! * [`use_keeper`]: whether a new terminal is held by the keeper or lives in
//!   the window as it always did;
//! * [`fate`] and [`summary`]: what quitting does to each session, and what the
//!   question before it says (sessions that keep running, sessions that are
//!   closed);
//! * [`reattach_plan`]: at the next start, which saved terminal is attached to
//!   which terminal the keeper holds, which fall back to the paused resume
//!   line of every restore, and which running terminals the layout does not
//!   know.
//!
//! The glue that acts on them is in `terminals.rs`, `quit_gently.rs` and
//! `restore_view.rs`.

use crate::durable::Held;
use leon_core::{SavedState, SavedTerminal};
use std::collections::HashSet;

/// Whether a terminal that is about to start is held by the keeper: the
/// setting is on, this system has POSIX terminals, the session runs on this
/// computer (not through ssh or a relay) and there is a keeper to ask.
pub fn use_keeper(setting: bool, posix: bool, local_machine: bool, has_service: bool) -> bool {
    setting && posix && local_machine && has_service
}

/// What quitting asks of the terminals the keeper holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaving {
    /// The ordinary quit, and the window being closed: the held terminals
    /// keep running and nothing is signalled to them.
    Keep,
    /// "Quit and end every session": the old behaviour for every terminal.
    EndAll,
}

/// What a session is when the application quits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    /// The keeper holds its terminal.
    pub held: bool,
    /// Its program runs on another computer (ssh, relay).
    pub remote: bool,
    /// An agent is in front of its shell, running.
    pub agent_running: bool,
}

/// What becomes of a session's terminal when the application quits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    /// Let go of: the program keeps running where it runs.
    Detach,
    /// The agent is given the chance to save (its exit line, then a
    /// termination signal) before the terminal is hung up.
    Gently,
    /// Hung up at once.
    Hang,
}

/// The fate of one session.
pub fn fate(leaving: Leaving, seen: Seen) -> Fate {
    if seen.remote || (seen.held && leaving == Leaving::Keep) {
        Fate::Detach
    } else if seen.agent_running {
        Fate::Gently
    } else {
        Fate::Hang
    }
}

/// What the question before quitting says of the sessions with a program
/// running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Sessions that keep running after the quit.
    pub kept: usize,
    /// Sessions that are closed by it.
    pub closed: usize,
}

/// Counts the sessions with a program running (`busy`) by what the quit does
/// to them: the held ones are kept unless the quit ends everything.
pub fn summary(leaving: Leaving, busy: impl IntoIterator<Item = bool>) -> Summary {
    let mut total = Summary::default();
    for held in busy {
        if held && leaving == Leaving::Keep {
            total.kept += 1;
        } else {
            total.closed += 1;
        }
    }
    total
}

/// What a start does with the terminals the layout remembered and the keeper
/// holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reattach {
    /// The saved terminals (by their saved id) and the held ones they attach
    /// to, in the saved order.
    pub attach: Vec<(u64, Held)>,
    /// Saved terminals that were held by a keeper whose terminal is gone: the
    /// computer restarted, the keeper was stopped, or the program ended. They
    /// are restored as every saved terminal is (a paused resume line).
    pub gone: Vec<u64>,
    /// Running terminals the layout does not know (a crash lost the last
    /// change of it): they appear in a session of their own, not orphaned.
    pub orphans: Vec<Held>,
    /// Saved terminals whose terminal another Leon window of this data
    /// directory is attached to. They are neither attached (two windows would
    /// type into one program) nor resumed (a second agent on the same
    /// session): they are left to that window.
    pub taken: Vec<u64>,
    /// How many running terminals are in use by another window, matched to
    /// the layout or not.
    pub in_use: usize,
}

/// Matches the saved terminals that were held by the keeper to what the keeper
/// lists now. A saved terminal attaches only to the terminal with its number
/// **and** its token, so a number a new keeper reused for another terminal
/// never attaches to the wrong one; a terminal whose program ended is not
/// attached (there is nothing to look at but its last output) and falls back.
pub fn reattach_plan(saved: &[SavedTerminal], held: &[Held]) -> Reattach {
    let mut plan = Reattach::default();
    let mut used: HashSet<u64> = HashSet::new();
    for terminal in saved {
        let Some(reference) = &terminal.keeper else {
            continue;
        };
        let found = held.iter().find(|h| {
            h.pty == reference.pty && h.token == reference.token && !used.contains(&h.pty)
        });
        match found {
            Some(h) if h.exit.is_none() && h.attached > 0 => {
                used.insert(h.pty);
                plan.taken.push(terminal.id);
            }
            Some(h) if h.exit.is_none() => {
                used.insert(h.pty);
                plan.attach.push((terminal.id, h.clone()));
            }
            _ => plan.gone.push(terminal.id),
        }
    }
    for h in held.iter().filter(|h| h.exit.is_none()) {
        if h.attached > 0 {
            plan.in_use += 1;
        } else if !used.contains(&h.pty) {
            plan.orphans.push(h.clone());
        }
    }
    plan
}

/// The saved terminals that go back in the same pass as those that attach:
/// the others of the same saved workspace. A tab can hold a terminal that is
/// still running and one that is gone; restoring the first now and the second
/// later would split the workspace in two, so the second (a paused resume line,
/// or a reason it cannot be) comes back with the first, in its pane.
pub fn companions(state: &SavedState, attach: &HashSet<u64>) -> HashSet<u64> {
    let known: HashSet<u64> = state.terminals.iter().map(|t| t.id).collect();
    let mut with = HashSet::new();
    for workspace in &state.workspaces {
        let leaves: Vec<u64> = workspace
            .tabs
            .iter()
            .flat_map(|tab| tab.layout.leaves())
            .collect();
        if leaves.iter().any(|leaf| attach.contains(leaf)) {
            with.extend(
                leaves
                    .into_iter()
                    .filter(|leaf| known.contains(leaf) && !attach.contains(leaf)),
            );
        }
    }
    with
}

/// The sentence for sessions another window of this data directory is
/// attached to.
pub fn in_use_note(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "1 kept session is open in another Leon window of this data directory and was left to it."
            .to_owned(),
        n => format!(
            "{n} kept sessions are open in another Leon window of this data directory and were left to it."
        ),
    }
}

/// The sentence for terminals the keeper no longer held, for the status line.
pub fn gone_note(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "The keeper no longer held 1 of the sessions that were running (the computer restarted, \
              or it was stopped); it is restored as usual."
            .to_owned(),
        n => format!(
            "The keeper no longer held {n} of the sessions that were running (the computer \
             restarted, or it was stopped); they are restored as usual."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::KeeperRef;
    use leon_wire::{Exit, Grid};

    fn grid() -> Grid {
        Grid {
            cols: 120,
            rows: 40,
            cell_width: 8,
            cell_height: 16,
        }
    }

    fn held(pty: u64, token: &str) -> Held {
        Held {
            pty,
            token: token.to_owned(),
            cwd: Some("/srv/api".into()),
            size: grid(),
            started_unix: 1,
            exit: None,
            attached: 0,
        }
    }

    fn saved(id: u64, keeper: Option<(u64, &str)>) -> SavedTerminal {
        SavedTerminal {
            id,
            machine: "local".into(),
            cwd: "/srv/api".into(),
            agent: None,
            session: None,
            confidence: None,
            history: None,
            name: None,
            title: None,
            started_at: 0,
            account: None,
            keeper: keeper.map(|(pty, token)| KeeperRef {
                pty,
                token: token.to_owned(),
            }),
        }
    }

    #[test]
    fn a_terminal_is_held_by_the_keeper_only_when_everything_allows_it() {
        assert!(use_keeper(true, true, true, true));
        assert!(!use_keeper(false, true, true, true), "the setting is off");
        assert!(!use_keeper(true, false, true, true), "no POSIX terminals");
        assert!(!use_keeper(true, true, false, true), "ssh or a relay");
        assert!(!use_keeper(true, true, true, false), "no keeper to ask");
    }

    #[test]
    fn quitting_lets_go_of_held_terminals_and_signals_nothing_to_them() {
        let held_agent = Seen {
            held: true,
            remote: false,
            agent_running: true,
        };
        assert_eq!(fate(Leaving::Keep, held_agent), Fate::Detach);
        let held_shell = Seen {
            agent_running: false,
            ..held_agent
        };
        assert_eq!(fate(Leaving::Keep, held_shell), Fate::Detach);
    }

    #[test]
    fn quitting_and_ending_everything_is_the_old_behaviour_for_held_terminals_too() {
        let held_agent = Seen {
            held: true,
            remote: false,
            agent_running: true,
        };
        assert_eq!(fate(Leaving::EndAll, held_agent), Fate::Gently);
        let held_shell = Seen {
            agent_running: false,
            ..held_agent
        };
        assert_eq!(fate(Leaving::EndAll, held_shell), Fate::Hang);
    }

    #[test]
    fn terminals_that_are_not_held_are_treated_as_before_whatever_the_quit() {
        let in_window = Seen {
            held: false,
            remote: false,
            agent_running: true,
        };
        for leaving in [Leaving::Keep, Leaving::EndAll] {
            assert_eq!(fate(leaving, in_window), Fate::Gently);
            assert_eq!(
                fate(
                    leaving,
                    Seen {
                        agent_running: false,
                        ..in_window
                    }
                ),
                Fate::Hang
            );
            // A program on another computer is let go of, as ever.
            assert_eq!(
                fate(
                    leaving,
                    Seen {
                        remote: true,
                        ..in_window
                    }
                ),
                Fate::Detach
            );
        }
    }

    #[test]
    fn the_question_counts_what_keeps_running_and_what_is_closed() {
        let busy = [true, true, false, true];
        assert_eq!(summary(Leaving::Keep, busy), Summary { kept: 3, closed: 1 });
        assert_eq!(
            summary(Leaving::EndAll, busy),
            Summary { kept: 0, closed: 4 }
        );
        assert_eq!(summary(Leaving::Keep, []), Summary::default());
    }

    #[test]
    fn a_saved_terminal_attaches_to_the_terminal_with_its_number_and_its_token() {
        let saved = [saved(1, Some((70, "a"))), saved(2, Some((71, "b")))];
        let held = [held(71, "b"), held(70, "a")];
        let plan = reattach_plan(&saved, &held);
        assert_eq!(
            plan.attach
                .iter()
                .map(|(id, h)| (*id, h.pty))
                .collect::<Vec<_>>(),
            [(1, 70), (2, 71)],
            "in the saved order"
        );
        assert!(plan.gone.is_empty() && plan.orphans.is_empty());
    }

    #[test]
    fn a_number_a_new_keeper_reused_does_not_attach_to_the_wrong_terminal() {
        // The old keeper held pty 5 under token "old"; a new keeper numbered
        // another terminal 5 under token "new".
        let saved = [saved(1, Some((5, "old")))];
        let held = [held(5, "new")];
        let plan = reattach_plan(&saved, &held);
        assert!(plan.attach.is_empty());
        assert_eq!(plan.gone, [1]);
        assert_eq!(plan.orphans.len(), 1, "the stranger appears, not hidden");
    }

    #[test]
    fn a_terminal_whose_keeper_is_gone_or_whose_program_ended_falls_back() {
        let saved = [
            saved(1, Some((70, "a"))),
            saved(2, Some((71, "b"))),
            saved(3, None),
        ];
        let mut ended = held(71, "b");
        ended.exit = Some(Exit {
            code: 0,
            signal: None,
        });
        // No keeper at all: nothing is held.
        let none = reattach_plan(&saved, &[]);
        assert_eq!(none.gone, [1, 2]);
        assert!(none.attach.is_empty() && none.orphans.is_empty());
        // 70 is gone; 71 ended while nobody looked.
        let plan = reattach_plan(&saved, &[ended]);
        assert_eq!(plan.gone, [1, 2]);
        assert!(plan.orphans.is_empty(), "an ended terminal is not adopted");
        // The saved terminal without a keeper reference is not in the plan.
        assert!(plan.attach.iter().all(|(id, _)| *id != 3));
    }

    #[test]
    fn a_running_terminal_the_layout_does_not_know_appears() {
        let saved = [saved(1, Some((70, "a")))];
        let held = [held(70, "a"), held(72, "late")];
        let plan = reattach_plan(&saved, &held);
        assert_eq!(plan.attach.len(), 1);
        assert_eq!(plan.orphans, [held[1].clone()]);
        // With nothing saved at all (a crash before the first write) every
        // running terminal appears.
        let all = reattach_plan(&[], &held);
        assert_eq!(all.orphans.len(), 2);
        assert!(all.attach.is_empty());
    }

    #[test]
    fn a_terminal_another_window_is_attached_to_is_left_to_it() {
        let saved = [saved(1, Some((70, "a"))), saved(2, Some((71, "b")))];
        let mut busy = held(70, "a");
        busy.attached = 1;
        let mut stranger = held(80, "elsewhere");
        stranger.attached = 2;
        let plan = reattach_plan(&saved, &[busy, held(71, "b"), stranger]);
        assert_eq!(plan.attach.len(), 1);
        assert_eq!(plan.attach[0].1.pty, 71);
        assert_eq!(plan.taken, [1], "neither attached nor resumed beside it");
        assert!(plan.gone.is_empty());
        assert!(plan.orphans.is_empty(), "not adopted from under the other");
        assert_eq!(plan.in_use, 2);
        assert!(in_use_note(2).contains("2 kept sessions"));
    }

    #[test]
    fn the_rest_of_a_workspace_with_a_survivor_comes_back_in_the_same_pass() {
        use leon_core::{SavedLayout, SavedTab, SavedWorkspace};
        let tab = |ids: &[u64]| SavedTab {
            layout: match ids {
                [one] => SavedLayout::Leaf(*one),
                [a, b] => SavedLayout::Split {
                    axis: "row".into(),
                    ratio: 0.5,
                    a: Box::new(SavedLayout::Leaf(*a)),
                    b: Box::new(SavedLayout::Leaf(*b)),
                },
                _ => unreachable!(),
            },
            focus: ids[0],
            zoomed: false,
        };
        let state = SavedState {
            workspaces: vec![
                SavedWorkspace {
                    key: "one".into(),
                    active: 0,
                    tabs: vec![tab(&[1, 2]), tab(&[3])],
                },
                SavedWorkspace {
                    key: "two".into(),
                    active: 0,
                    tabs: vec![tab(&[4])],
                },
            ],
            terminals: (1..=4).map(|id| saved(id, None)).collect(),
            ..Default::default()
        };
        let attach: HashSet<u64> = [1].into();
        let with = companions(&state, &attach);
        assert_eq!(with, [2, 3].into(), "its tab-mate and its other tab");
        assert!(!with.contains(&4), "another workspace follows the setting");
        assert!(companions(&state, &HashSet::new()).is_empty());
    }

    #[test]
    fn one_terminal_is_attached_once_even_if_two_saved_ones_name_it() {
        let saved = [saved(1, Some((70, "a"))), saved(2, Some((70, "a")))];
        let plan = reattach_plan(&saved, &[held(70, "a")]);
        assert_eq!(plan.attach.len(), 1);
        assert_eq!(plan.gone, [2]);
    }

    #[test]
    fn the_note_about_a_gone_keeper_counts() {
        assert_eq!(gone_note(0), "");
        assert!(gone_note(1).contains("1 of the sessions"));
        assert!(gone_note(4).contains("4 of the sessions"));
    }
}

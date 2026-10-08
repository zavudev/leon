//! Settle, snooze and undo in the sidebar: the window's side of the shelves.
//!
//! The rules are in `shelf.rs` (what is where, when a snooze ends, what undo
//! restores) and the placement in `tree.rs`; this file is what the window does
//! with them. A change to the shelves is asked of the engine
//! ([`Op::SetShelves`]), which writes the store; the window reads the store
//! again and the tree follows. The banner that offers Undo lives here for five
//! seconds ([`shelf::UNDO_WINDOW`], on the toolkit's clock like the other
//! banners), and the removal of a closed session's history waits for it (or for
//! the application to end, which finishes it at once).
//!
//! What is not covered, said plainly: a sleeping row that has no history
//! session (a plain shell) has nothing in the store to put on a shelf, so it
//! cannot be settled or snoozed; and a session running in another terminal
//! cannot end its own snooze, because Leon does not see it.

use super::live::LiveId;
use super::shelf::{self, Pending, Restore, Undo};
use super::shell::Shell;
use crate::engine::{Op, StatusKind};
use crate::keys::Command;
use chrono::{DateTime, Utc};
use gpui_kit::{Context, Task, Window};
use leon_core::{MachineId, Session, SessionId, SessionScope, Shelf};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// The longest the timer that brings a snooze back sleeps before it looks at
/// the clock again, so a computer that slept does not bring it back late.
const LOOK_AT_THE_CLOCK: Duration = Duration::from_secs(60);

/// What the window knows of the shelves.
#[derive(Default)]
pub struct ShelfUi {
    /// Where each session stands, as the store last said.
    pub(super) shelf: HashMap<SessionId, Shelf>,
    /// The step that can be undone, while its banner is up.
    pub(super) undo: Option<Pending<Instant>>,
    /// Closed sessions whose removal from the history waits for the banner to
    /// go: the tree leaves them out meanwhile.
    pub(super) hidden: HashSet<SessionId>,
    /// Sessions the automatic settling has asked for and the store has not
    /// answered yet, so one is not asked for twice.
    settling: HashSet<SessionId>,
    /// Whether `sidebar_settle_merged` is on.
    pub(super) settle_merged: bool,
    /// Brings the next snooze back.
    timer: Option<Task<()>>,
}

impl ShelfUi {
    /// Reads where the sessions stand from the store's answer.
    pub(super) fn read(&mut self, shelf: HashMap<SessionId, Shelf>) {
        self.settling.retain(|id| !shelf.contains_key(id));
        self.shelf = shelf;
    }
}

impl Shell {
    /// The history sessions that have a live terminal: the ones a terminal was
    /// started from and the ones it only learned it is (see
    /// [`Shell::history_of_live`]).
    pub(super) fn running_histories(&self) -> HashSet<SessionId> {
        self.live
            .ids()
            .into_iter()
            .filter_map(|id| self.history_of_live(id))
            .collect()
    }

    /// The session a command of the shelves is about: the one the keyboard is
    /// on. Tells the person when there is none.
    fn shelf_target(&self) -> Option<Session> {
        let found = self.here_session();
        if found.is_none() {
            self.engine.report(
                StatusKind::Info,
                "Select a session first: only a session of the history has a shelf.",
            );
        }
        found
    }

    /// Puts the session under the keyboard on the Settled shelf.
    pub(super) fn settle_here(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.shelf_target() else {
            return;
        };
        let running = self.running_histories().contains(&session.id);
        if let Some(why) = shelf::cannot_settle(running) {
            self.engine.report(StatusKind::Info, why);
            return;
        }
        self.put_on_shelf(&session, Some(Shelf::Settled), cx);
    }

    /// Hides the session until `until` (asked for by the palette's questions).
    pub(super) fn snooze_session(
        &mut self,
        session: &SessionId,
        until: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) {
        let found = self
            .snapshot
            .sessions
            .iter()
            .find(|candidate| &candidate.id == session)
            .cloned();
        if let Some(found) = found {
            self.put_on_shelf(&found, Some(Shelf::Snoozed(until)), cx);
        }
    }

    /// Takes the session under the keyboard off its shelf: it goes back to
    /// where it was, and stays there.
    pub(super) fn bring_back_here(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.shelf_target() else {
            return;
        };
        if !self.placement.shelved.contains(&session.id) {
            self.engine
                .report(StatusKind::Info, "That session is not on a shelf.");
            return;
        }
        self.put_on_shelf(&session, Some(Shelf::Returned), cx);
    }

    /// Asks for `session` to be where `to` says, and offers Undo.
    fn put_on_shelf(&mut self, session: &Session, to: Option<Shelf>, cx: &mut Context<Self>) {
        let was = self.shelves.shelf.get(&session.id).copied();
        if was == to {
            return;
        }
        self.engine.submit(Op::SetShelves {
            changes: vec![(session.id.clone(), to)],
            done: None,
        });
        let undo = Undo::Shelved {
            session: session.id.clone(),
            was,
            to,
        };
        self.offer_undo(undo, &session.title, Vec::new(), cx);
    }

    /// Records the unpinning of a session, to be undone: `order` is the Pinned
    /// section of its machine before.
    pub(super) fn offer_unpin_undo(
        &mut self,
        machine: MachineId,
        order: Vec<SessionId>,
        title: &str,
        cx: &mut Context<Self>,
    ) {
        self.offer_undo(Undo::Unpinned { machine, order }, title, Vec::new(), cx);
    }

    /// Shows the banner that offers Undo for the step just taken. `forget` is
    /// the history sessions whose removal waits for the banner to go. A step
    /// still on offer is final from now: the new one replaces it.
    pub(super) fn offer_undo(
        &mut self,
        undo: Undo,
        title: &str,
        forget: Vec<SessionId>,
        cx: &mut Context<Self>,
    ) {
        self.settle_undo();
        let text = undo.banner(title, shelf::local_offset());
        self.shelves.undo = Some(Pending {
            undo,
            text,
            until: cx.background_executor().now() + shelf::UNDO_WINDOW,
            forget,
        });
        self.keep_banners(cx);
        cx.notify();
    }

    /// Makes the step on offer final: what waited for the banner is done now.
    pub(super) fn settle_undo(&mut self) {
        if let Some(pending) = self.shelves.undo.take() {
            self.forget_for_good(pending.forget);
        }
    }

    /// [`Shell::settle_undo`] for the way out of the application: the removal
    /// is made before this returns, because an operation that is only
    /// submitted may not run once the process ends.
    pub(super) fn settle_undo_before_quit(&mut self) {
        let Some(pending) = self.shelves.undo.take() else {
            return;
        };
        if let Err(error) = self.engine.forget_sessions(&pending.forget) {
            tracing::warn!(%error, "closed sessions could not be removed from the history");
        }
    }

    /// Ends the window to undo when it is over at `now`. `true` when it was.
    pub(super) fn expire_undo(&mut self, now: Instant) -> bool {
        if !self
            .shelves
            .undo
            .as_ref()
            .is_some_and(|pending| pending.over(&now))
        {
            return false;
        }
        self.settle_undo();
        true
    }

    /// Removes closed sessions from the history, now that closing them can no
    /// longer be undone.
    fn forget_for_good(&mut self, sessions: Vec<SessionId>) {
        if !sessions.is_empty() {
            self.engine.submit(Op::ForgetSessions(sessions));
        }
    }

    /// Puts back what the banner offers to.
    pub(super) fn undo_last(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.shelves.undo.take() else {
            self.engine
                .report(StatusKind::Info, "There is nothing to undo.");
            return;
        };
        let machine = match &pending.undo {
            Undo::Unpinned { machine, .. } => Some(machine.clone()),
            _ => None,
        };
        let pinned = machine
            .as_ref()
            .map(|machine| self.pinned_ids(machine))
            .unwrap_or_default();
        let known = |id: &SessionId| {
            self.snapshot
                .sessions
                .iter()
                .any(|session| &session.id == id)
                || self.shelves.hidden.contains(id)
        };
        let restore = pending.undo.restore(&pinned, known);
        // What waited for the banner is not removed after all.
        for id in &pending.forget {
            self.shelves.hidden.remove(id);
        }
        match restore {
            Restore::Shelves(changes) => self.engine.submit(Op::SetShelves {
                changes,
                done: Some("Undone.".to_owned()),
            }),
            Restore::Pins { machine, order } => self.engine.submit(Op::PinSessions {
                parent: SessionScope::Machine(machine),
                pinned: order,
                done: "Undone.",
            }),
            Restore::Sleeper(record) => {
                self.dormant.restore(record);
                self.save_dormant();
                self.refresh_live();
                self.engine.report(StatusKind::Info, "Undone.");
            }
            Restore::Asleep(gone) => {
                // It had no sleeping row to go back to: it is a new one, after
                // the others.
                self.dormant.add(
                    &gone.machine,
                    &gone.cwd,
                    gone.agent,
                    gone.label,
                    gone.account,
                );
                self.save_dormant();
                self.refresh_live();
                self.engine.report(StatusKind::Info, "Undone.");
            }
            Restore::Sleeping(history) => {
                self.slept.insert(history);
                self.reload(cx);
                self.engine.report(StatusKind::Info, "Undone.");
            }
            Restore::Wake { sleeper, history } => self.wake_again(sleeper, history, window, cx),
            Restore::Nothing => self
                .engine
                .report(StatusKind::Info, "That session is gone: nothing to undo."),
        }
        cx.notify();
    }

    /// Wakes the session a sleep was undone for.
    fn wake_again(
        &mut self,
        sleeper: Option<LiveId>,
        history: Option<SessionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = sleeper.filter(|id| self.dormant.get(*id).is_some()) {
            self.wake_dormant(id, window, cx);
            return;
        }
        let found = history.and_then(|history| {
            self.snapshot
                .sessions
                .iter()
                .find(|session| session.id == history)
                .cloned()
        });
        match found {
            Some(session) => self.resume_session(session, window, cx),
            None => self
                .engine
                .report(StatusKind::Info, "That session is gone: nothing to wake."),
        }
    }

    /// A live session needs the person, failed or finished: when its history
    /// session is snoozed, the snooze ends now (see [`shelf::woken`]).
    pub(super) fn wake_snoozed(&mut self, id: LiveId) {
        let Some(history) = self.history_of_live(id) else {
            return;
        };
        let Some(state) = shelf::woken(self.shelves.shelf.get(&history), self.now()) else {
            return;
        };
        let title = self
            .snapshot
            .sessions
            .iter()
            .find(|session| session.id == history)
            .map_or_else(|| "A snoozed session".to_owned(), |s| s.title.clone());
        self.engine.submit(Op::SetShelves {
            changes: vec![(history, Some(state))],
            done: Some(format!("{title} is back from its snooze.")),
        });
    }

    /// Settles the sessions whose pull request is merged, when the setting asks
    /// for it. Each is asked for once; the store ignores what it already says.
    pub(super) fn settle_merged_now(&mut self) {
        if !self.shelves.settle_merged {
            return;
        }
        let running = self.running_histories();
        let ids: Vec<SessionId> = shelf::settle_merged(
            &self.snapshot,
            &self.placement,
            &self.shelves.shelf,
            &running,
        )
        .into_iter()
        .filter(|id| !self.shelves.settling.contains(id))
        // An agent working in another terminal is live too, as far as the
        // person is concerned. Leon knows that only from a scan of the
        // machine: where there is none (scanning is off, or the machine has
        // not been looked at yet, or the last look failed) nothing is claimed,
        // so nothing is settled behind the person's back.
        .filter(|id| {
            self.snapshot
                .sessions
                .iter()
                .find(|session| &session.id == id)
                .is_some_and(|session| {
                    self.engine.elsewhere(&session.machine_id).is_some()
                        && self.elsewhere_of(session).is_none()
                })
        })
        .collect();
        if ids.is_empty() {
            return;
        }
        self.shelves.settling.extend(ids.iter().cloned());
        self.engine.submit(Op::SetShelves {
            changes: ids
                .into_iter()
                .map(|id| (id, Some(Shelf::Settled)))
                .collect(),
            done: None,
        });
    }

    /// Brings the next snooze back when its time comes: a timer that looks at
    /// the clock at least once a minute, and places the sessions again.
    pub(super) fn arm_shelf_timer(&mut self, cx: &mut Context<Self>) {
        let now = self.now();
        let Some(next) = shelf::next_wake(self.shelves.shelf.values(), now) else {
            self.shelves.timer = None;
            return;
        };
        let wait = (next - now)
            .to_std()
            .unwrap_or_default()
            .min(LOOK_AT_THE_CLOCK);
        self.shelves.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                this.refresh_live();
                this.arm_shelf_timer(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// The label of the undo command, for the banner's button.
    pub(super) fn undo_keys() -> Option<String> {
        crate::keys::keys_label(Command::Undo)
    }
}

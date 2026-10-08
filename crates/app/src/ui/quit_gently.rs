//! Quitting without cutting an agent off mid-save.
//!
//! An agent that flushes its session when it ends (a hard hang-up may lose
//! the last turn, or leave a SQLite WAL not checkpointed) is given the chance:
//!
//! 1. the state of what is open is written first, so the next start restores
//!    the agents that were running (they are gone by the time the window is);
//! 2. each local terminal with an agent in front of its shell is sent the
//!    agent's own exit line from the catalogue (`/exit` for Claude Code and
//!    opencode, the only ones verified; the others are skipped here);
//! 3. after [`Options::quit_gesture_wait`](super::shell::Options) those still
//!    in front get SIGTERM on the terminal's foreground process group;
//! 4. after [`Options::quit_grace`](super::shell::Options) in all, whatever is
//!    left is hung up as before (SIGHUP, then SIGKILL of the group).
//!
//! The terminals are handled in parallel, the wait ends as soon as every agent
//! is gone, and nothing here can wait longer than the grace: quitting never
//! hangs. Terminals on other computers are let go of, not closed, as ever.
//!
//! With durable sessions on, the terminals the keeper holds are let go of too,
//! and none of the above happens to them: no exit line is typed, nothing is
//! signalled, and the programs keep running. "Quit and end every session"
//! asks for the whole of it, for those as well (see `keeping::fate`).

use super::keeping::{self, Fate, Leaving, Seen};
use super::shell::Shell;
use crate::engine::StatusKind;
use gpui_kit::{Context, Task};
use std::time::Duration;

/// How often the terminals are looked at while waiting.
pub const POLL: Duration = Duration::from_millis(50);

/// What the window keeps while it is closing sessions.
#[derive(Default)]
pub struct Closing {
    /// Whether a quit has begun: the state is not written again.
    pub quitting: bool,
    /// Whether this quit ends the terminals the keeper holds too ("Quit and
    /// end every session"); a plain quit lets go of them.
    pub end_held: bool,
    task: Option<Task<()>>,
    terminated: bool,
    /// When the quit began, by the executor's clock (the one tests advance).
    began: Option<std::time::Instant>,
}

impl Shell {
    /// What this quit asks of the terminals the keeper holds.
    pub(super) fn leaving(&self) -> Leaving {
        if self.closing.end_held {
            Leaving::EndAll
        } else {
            Leaving::Keep
        }
    }

    /// What becomes of a session's terminal when the application quits.
    pub(super) fn fate_of(&self, session: &super::live::LiveSession, cx: &gpui_kit::App) -> Fate {
        let terminal = session.view.read(cx).terminal();
        keeping::fate(
            self.leaving(),
            Seen {
                held: terminal.is_held(),
                remote: terminal.is_remote(),
                agent_running: terminal.exit_info().is_none()
                    && session.shown_agent().is_some()
                    && !session.is_paused()
                    && terminal.shell_is_foreground() == Some(false),
            },
        )
    }

    /// The sessions with an agent in front of the shell, that is, running,
    /// and that this quit gives the chance to save: not those let go of.
    fn agents_running(&self, cx: &gpui_kit::App) -> Vec<super::live::LiveId> {
        self.live
            .all()
            .iter()
            .filter(|session| self.fate_of(session, cx) == Fate::Gently)
            .map(|session| session.id)
            .collect()
    }

    /// "Quit and end every session": the quit that hangs up the terminals the
    /// keeper holds as well. `files` says what to do with files that have
    /// changes: `Some(true)` saves them first, `Some(false)` discards them, and
    /// `None` has none to decide about.
    pub(super) fn end_sessions_and_quit(
        &mut self,
        files: Option<bool>,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        self.closing.end_held = true;
        match files {
            None => self.quit_now(cx),
            Some(true) => self.save_all_and_quit(window, cx),
            Some(false) => self.discard_and_quit(cx),
        }
        // Saving the files can take a moment, and the quit that follows it
        // is still this one: the choice is kept until the quit goes ahead and
        // dropped when it is given up (`continue_quit`).
        if !self.closing.quitting && !self.quit_pending_saves() {
            self.closing.end_held = false;
        }
    }

    /// Begins quitting. `true` when there was nothing to wait for and
    /// everything is flushed: the caller ends the application. `false` when
    /// agents are being given their chance: the application ends by itself,
    /// within the grace.
    pub(super) fn begin_quit(&mut self, cx: &mut Context<Self>) -> bool {
        if self.closing.quitting {
            return self.closing.task.is_none();
        }
        self.closing.quitting = true;
        // Before any agent ends: what was running is what is restored.
        self.remember_clean_shutdown(cx);
        let running = self.agents_running(cx);
        if running.is_empty() || self.options.quit_grace.is_zero() {
            self.flush(cx);
            return true;
        }
        self.engine.report(
            StatusKind::Busy,
            format!(
                "Closing {} session{}…",
                running.len(),
                if running.len() == 1 { "" } else { "s" }
            ),
        );
        for id in &running {
            let Some(session) = self.live.get(*id) else {
                continue;
            };
            let exit = session
                .agent
                .and_then(|agent| agent.spec())
                .and_then(|spec| spec.exit.clone());
            if let Some(line) = exit {
                session
                    .view
                    .read(cx)
                    .terminal()
                    .write(format!("{line}\r").into_bytes());
            }
        }
        self.closing.began = Some(cx.background_executor().now());
        // The watcher: looks every POLL, and again at the end of the grace,
        // whatever else happened. Time is the executor's clock, not a count
        // of laps, so a late wake-up shortens the wait and never stretches
        // it. Terminals ending are looked at too (see `closing_look`).
        self.closing.task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            let done = this
                .update(cx, |this, cx| this.closing_look(cx))
                .unwrap_or(true);
            if done {
                return;
            }
        }));
        false
    }

    /// Looks at the agents now: ends the application when none is left or
    /// the grace is over. `true` once it did. Called by the timer and by
    /// every terminal's end, so the last agent leaving is never waited for.
    pub(super) fn closing_look(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(began) = self.closing.began else {
            return false;
        };
        if self.closing.task.is_none() {
            return true;
        }
        let elapsed = cx
            .background_executor()
            .now()
            .saturating_duration_since(began);
        let (wait, grace) = (self.options.quit_gesture_wait, self.options.quit_grace);
        if !self.closing_step(elapsed, wait, grace, cx) {
            return false;
        }
        self.closing.task = None;
        self.flush(cx);
        (self.options.quit)(cx);
        true
    }

    /// One look while closing: `true` when it is time to hang up what is left.
    fn closing_step(
        &mut self,
        elapsed: Duration,
        gesture_wait: Duration,
        grace: Duration,
        cx: &mut Context<Self>,
    ) -> bool {
        let running = self.agents_running(cx);
        if running.is_empty() || elapsed >= grace {
            return true;
        }
        if elapsed >= gesture_wait && !self.closing.terminated {
            self.closing.terminated = true;
            for id in running {
                if let Some(session) = self.live.get(id) {
                    session.view.read(cx).terminal().terminate_foreground();
                }
            }
        }
        false
    }
}

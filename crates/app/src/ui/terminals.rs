//! Live sessions in the shell: terminals in split panes and tabs, starting an
//! agent or a shell, resuming a history session, closing them, and routing
//! the keyboard to the terminal that has it.
//!
//! Every terminal is a base shell (see `launch.rs`); an agent is typed into
//! it. Opening a history session resumes it that way: a terminal on the
//! session's machine in its folder, with `claude --resume <id>` (or the other
//! agents' flags) typed into the shell. A session that already has a live
//! terminal focuses it instead of starting another. Where the session cannot
//! be resumed (the folder is gone, the machine is off, the agent is not
//! installed) its stored transcript is shown with the reason. The terminals are [`leon_term::TerminalView`] entities kept by
//! [`Sessions`](super::live::Sessions); how they are laid out is
//! [`Workspaces`](super::workspace::Workspaces), pure data. The shell decides
//! which terminal is on screen and whether it has the keyboard (`Pane::Main`
//! with a live session open, no overlay). The chords Leon keeps while that is
//! so are listed in `keys.rs`.

use super::activity::Activity;
use super::live::{AgentPhase, LiveId, LiveSession, LiveState};
use super::panes::{Axis, Dir, Layout, MinSize, Path, Rect, RESIZE_STEP};
use super::shell::{ElsewhereNote, Main, Notice, Overlay, Pane, Shell};
use super::steps::SessionIntent;
use super::tree::{self, NodeId};
use super::workspace::{self, Closed};
use crate::engine::{MachineState, Op, StatusKind, Target};
use crate::icons::agent_icon;
use crate::launch::{self, Launch, LaunchError};
use crate::theme::{self, hairline, metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, relative, AnyElement, App, Context, Div, DragMoveEvent, FontWeight, IntoElement, Render,
    Window,
};
use leon_core::{MachineId, MachineKind, Session, SessionId};
use leon_term::{GridSize, TerminalView, ViewEvent};
use std::time::{Duration, Instant};

/// The grid a terminal starts with, before the first paint measures the pane
/// and resizes it.
const START_COLS: u16 = 120;
const START_ROWS: u16 = 32;

/// When a freshly started shell is ready for the agent's command line to be
/// typed into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Readiness {
    /// How long the shell must have been quiet after printing (its prompt).
    pub quiet: Duration,
    /// How long to wait for that at most; the line is typed anyway then.
    pub timeout: Duration,
    /// How often to look.
    pub poll: Duration,
}

impl Default for Readiness {
    fn default() -> Self {
        Self {
            quiet: Duration::from_millis(250),
            timeout: Duration::from_secs(5),
            poll: Duration::from_millis(40),
        }
    }
}

/// The fewest columns and rows a pane keeps when a divider is moved.
pub const MIN_PANE: MinSize = MinSize {
    cols: 20.0,
    rows: 5.0,
};

/// Where a new terminal goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// A new tab of the workspace of its folder.
    Tab,
    /// A new pane beside or below this terminal's.
    Split(LiveId, Axis),
}

/// What a divider being dragged carries.
#[derive(Clone)]
pub struct DividerDrag {
    /// The split it belongs to.
    pub path: Path,
    /// The terminal whose tab it is in.
    pub of: LiveId,
}

/// The ghost of a drag: nothing is drawn, the divider itself moves.
pub(super) struct NoGhost;

impl Render for NoGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Shell {
    /// How the lion of the header feels: asleep without the focus, startled
    /// by a fresh error, else as busy as the sessions are.
    pub(super) fn mark_mood(&self) -> leon_mark::Mood {
        super::lion::mood(
            self.window_active,
            self.mark_error,
            self.live.all().iter().map(|session| session.activity),
        )
    }

    /// Shows the error of a session that just failed on the lion for a few
    /// seconds, then lets it go.
    fn flash_error(&mut self, cx: &mut Context<Self>) {
        self.mark_error = true;
        let flash = self.options.error_flash;
        self.mark_error_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(flash).await;
            this.update(cx, |this, cx| {
                this.mark_error = false;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Whether a terminal has the keyboard: one is on screen, the main pane
    /// is where the keyboard is and nothing is open over it.
    pub(super) fn terminal_focused(&self) -> bool {
        self.overlay == Overlay::None
            && self.pane == Pane::Main
            && matches!(self.main, Main::Live(id) if self.live.get(id).is_some())
    }

    /// Gives the keyboard to the terminal on screen when it should have it,
    /// and takes it back to the window when it should not.
    pub(super) fn sync_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_focused() && !self.find_focused(window, cx) {
            if let Main::Live(id) = &self.main {
                if let Some(session) = self.live.get(*id) {
                    let handle = session.view.read(cx).focus().clone();
                    if !handle.is_focused(window) {
                        window.focus(&handle, cx);
                    }
                    return;
                }
            }
        }
        let held_by_terminal = self
            .live
            .all()
            .iter()
            .any(|session| session.view.read(cx).focus().is_focused(window));
        if held_by_terminal && self.overlay == Overlay::None {
            self.focus.focus(window, cx);
        }
    }

    /// Starts `launch` in `cwd` on `machine`: a base terminal in a tab or a
    /// pane, with the agent typed into its shell when one is asked for.
    /// `history` is the history session it resumes, when it does.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_live(
        &mut self,
        launch: Launch,
        machine: &MachineId,
        cwd: &str,
        place: Place,
        history: Option<SessionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(found) = self.snapshot.machine(machine).cloned() else {
            self.engine
                .report(StatusKind::Error, "That machine is not known.");
            return;
        };
        let state = self.engine.machine_state(machine);
        let report = match &state {
            MachineState::Online(Some(report)) => Some(report.clone()),
            _ => None,
        };
        // A machine nobody probed yet is tried anyway; the probe that runs now
        // tells the next start what is installed there.
        let remote = matches!(found.kind, MachineKind::Ssh { .. });
        if remote && report.is_none() {
            self.engine.submit(Op::Probe(machine.clone()));
        }
        let prefs = Self::launch_prefs(cx);
        let plan = match launch::plan_with(
            &found,
            report.as_ref(),
            cwd,
            &launch,
            &self.engine.ssh(),
            &*self.options.system,
            &prefs,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                self.engine.report(StatusKind::Error, error.to_string());
                cx.notify();
                return;
            }
        };
        let colours = theme::palette(cx);
        let view = match TerminalView::spawn_with(
            &*self.options.backend,
            &plan.spawn,
            GridSize::new(START_COLS, START_ROWS),
            colours.terminal,
            Self::terminal_font(cx),
            cx,
        ) {
            Ok(view) => view,
            Err(error) => {
                self.engine.report(StatusKind::Error, error.to_string());
                cx.notify();
                return;
            }
        };
        view.update(cx, |view, _| view.set_padding(metrics::TERMINAL_PADDING()));
        super::prefs::apply_terminal_prefs(&view, cx);

        let id = self.live.next_id();
        let (agent, resumed) = match &launch {
            Launch::Agent { kind, resume } => (Some(*kind), resume.clone()),
            Launch::Shell => (None, None),
        };
        let subscriptions = vec![
            cx.subscribe_in(
                &view,
                window,
                move |this, _, event: &ViewEvent, window, cx| {
                    this.live_event(id, event, window, cx)
                },
            ),
            // The sidebar shows each session's state; it changes when the
            // first output arrives, and a program ending prints something, so
            // both are noticed here.
            cx.observe(&view, move |this, _, cx| this.live_changed(id, cx)),
        ];
        // The line is typed by a thread of its own, from the terminal's point
        // of view and not the window's: it waits for the shell to have printed
        // its prompt and gone quiet, and holds the terminal only weakly, so
        // closing the pane is never held up by it.
        if let Some(line) = plan.send.clone() {
            let terminal = std::sync::Arc::downgrade(view.read(cx).terminal());
            let ready = self.options.ready;
            std::thread::spawn(move || {
                let begun = Instant::now();
                loop {
                    std::thread::sleep(ready.poll);
                    let Some(terminal) = terminal.upgrade() else {
                        return;
                    };
                    if terminal.exit_info().is_some() {
                        return;
                    }
                    let quiet = terminal
                        .quiet_for()
                        .is_some_and(|quiet| quiet >= ready.quiet);
                    if quiet || begun.elapsed() >= ready.timeout {
                        terminal.write(line.into_bytes());
                        return;
                    }
                }
            });
        }
        self.live.push(LiveSession {
            id,
            machine: machine.clone(),
            machine_name: found.name.clone(),
            cwd: cwd.to_owned(),
            agent,
            resumed,
            history,
            title: None,
            name: None,
            phase: if agent.is_some() {
                AgentPhase::Launched
            } else {
                AgentPhase::Shell
            },
            can_detect: plan.can_detect_foreground,
            view,
            shown: LiveState::Starting,
            bell: false,
            failure_seen: false,
            activity: Activity::Off,
            _subscriptions: subscriptions,
        });
        self.refresh_activity(cx);
        self.keep_watching(cx);
        match place {
            Place::Split(of, axis) if self.workspaces.locate(of).is_some() => {
                self.workspaces.split(of, axis, id);
            }
            _ => {
                let root = tree::workspace_root(&self.snapshot, machine, cwd);
                self.workspaces
                    .add_tab(&workspace::key_of(machine.as_str(), &root), id);
            }
        }
        self.refresh_live();
        self.open_live(id, window, cx);
        self.warn_before_session(agent, machine, cx);
    }

    /// A line in the status bar when the agent that was just started is near
    /// the end of a limit, with when it resets. It never stops anything: the
    /// session is already starting.
    fn warn_before_session(
        &mut self,
        agent: Option<leon_core::AgentId>,
        machine: &MachineId,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = agent else {
            return;
        };
        if !crate::settings::flag(cx, "usage_warn_before_session") {
            return;
        }
        let notice = crate::agent_usage::start_notice(
            &self.usage.board,
            machine,
            agent,
            (self.options.now)().timestamp(),
            crate::settings::usage_thresholds(cx),
        );
        if let Some(notice) = notice {
            self.engine.report(StatusKind::Info, notice);
        }
    }

    /// Starts the session of a flow's answer.
    pub(super) fn start_intent(
        &mut self,
        intent: SessionIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let launch = Launch::Agent {
            kind: intent.agent,
            resume: None,
        };
        self.start_live(
            launch,
            &intent.machine,
            &intent.cwd,
            Place::Tab,
            None,
            window,
            cx,
        );
    }

    /// What changed in a terminal's state or output: the agent phase, the
    /// state the sidebar shows.
    fn live_changed(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(session) = self.live.get_mut(id) else {
            return;
        };
        let state = session.state(cx);
        let phase = if session.can_detect {
            let in_front = session.view.read(cx).terminal().shell_is_foreground();
            session.phase.observe(in_front)
        } else {
            session.phase
        };
        let moved = state != session.shown || phase != session.phase;
        session.shown = state;
        session.phase = phase;
        // Only a change of what the dot says repaints the sidebar.
        let activity = self.options.activity;
        let changed = self.set_activity(id, &activity, cx);
        // A find bar over this pane searches what was just printed.
        self.refresh_find(id, cx);
        if moved || changed {
            cx.notify();
        }
    }

    /// Reads a session's activity again. `true` when it changed.
    fn set_activity(
        &mut self,
        id: LiveId,
        thresholds: &super::activity::Thresholds,
        cx: &App,
    ) -> bool {
        let Some(session) = self.live.get_mut(id) else {
            return false;
        };
        let now = session.read_activity(cx, thresholds);
        std::mem::replace(&mut session.activity, now) != now
    }

    /// Reads every session's activity again, for the coarse timer and for
    /// tests that move a threshold. `true` when any changed.
    pub(super) fn refresh_activity(&mut self, cx: &App) -> bool {
        let thresholds = self.options.activity;
        let mut any = false;
        for id in self.live.ids() {
            any |= self.set_activity(id, &thresholds, cx);
        }
        any
    }

    /// Starts the coarse timer when a terminal is live and none runs. It
    /// looks at every terminal each `tick`, because a program going quiet
    /// wakes nobody, and ends itself once no terminal is live: nothing ticks
    /// while nothing runs.
    pub(super) fn keep_watching(&mut self, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| loop {
            let Ok(tick) = this.read_with(cx, |this, _| this.options.activity.tick) else {
                return;
            };
            cx.background_executor().timer(tick).await;
            let alive = this.update(cx, |this, cx| {
                if this.refresh_activity(cx) {
                    cx.notify();
                }
                let alive = this
                    .live
                    .all()
                    .iter()
                    .any(|session| session.view.read(cx).terminal().exit_info().is_none());
                if !alive {
                    // Dropping the handle ends the task after this turn.
                    this.ticker = None;
                }
                alive
            });
            if !matches!(alive, Ok(true)) {
                return;
            }
        }));
    }

    /// What a live terminal reports.
    fn live_event(
        &mut self,
        id: LiveId,
        event: &ViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ViewEvent::Title(title) => {
                if let Some(session) = self.live.get_mut(id) {
                    session.title = title.clone();
                }
            }
            ViewEvent::Bell => {
                // A bell in the terminal on screen has been heard.
                let on_screen = matches!(self.main, Main::Live(open) if open == id);
                let marks = crate::settings::flag(cx, "terminal_bell_mark");
                if let Some(session) = self.live.get_mut(id) {
                    session.bell = !on_screen && marks;
                }
                let thresholds = self.options.activity;
                self.set_activity(id, &thresholds, cx);
            }
            ViewEvent::Exited(info) => {
                let on_screen = matches!(self.main, Main::Live(open) if open == id);
                if let Some(session) = self.live.get_mut(id) {
                    session.failure_seen = on_screen;
                    session.shown = LiveState::Exited(info.code);
                    let what = format!(
                        "{} on {} exited with code {}.",
                        session.label(),
                        session.machine_name,
                        info.code
                    );
                    let kind = if info.success() {
                        StatusKind::Info
                    } else {
                        StatusKind::Error
                    };
                    self.engine.report(kind, what);
                    if !info.success() {
                        self.flash_error(cx);
                    }
                }
            }
            ViewEvent::ContextMenu(at) => self.open_terminal_menu(id, *at, window, cx),
            ViewEvent::Clicked => {
                // A click on a pane makes it the focused one of its tab.
                if self.overlay == Overlay::None && self.live.get(id).is_some() {
                    self.pane = Pane::Main;
                    if self.here_live() != Some(id)
                        || !matches!(self.main, Main::Live(open) if open == id)
                    {
                        self.open_live(id, window, cx);
                    }
                }
            }
        }
        let thresholds = self.options.activity;
        self.set_activity(id, &thresholds, cx);
        cx.notify();
    }

    /// Puts the live sessions in the tree and keeps the cursor on its node.
    pub(super) fn refresh_live(&mut self) {
        self.placement = self.place(&self.snapshot);
        self.rebuild_rows();
    }

    /// Shows a live session in the main pane (its tab, with it focused) and
    /// gives it the keyboard.
    pub(super) fn open_live(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        if self.live.get(id).is_none() {
            return;
        }
        // Looking at a session answers its bell and its failure.
        let ended = self
            .live
            .get(id)
            .is_some_and(|session| session.view.read(cx).terminal().exit_info().is_some());
        if let Some(session) = self.live.get_mut(id) {
            session.bell = false;
            session.failure_seen |= ended;
        }
        let thresholds = self.options.activity;
        self.set_activity(id, &thresholds, cx);
        self.workspaces.focus(id);
        self.main = Main::Live(id);
        self.pane = Pane::Main;
        // A terminal that resumed a history session is that session's row.
        let node = match self
            .live
            .get(id)
            .and_then(|session| session.history.clone())
        {
            Some(history) if self.placement.merged.contains(&history) => NodeId::Session(history),
            _ => NodeId::Live(id),
        };
        self.show(&node);
        self.sync_focus(window, cx);
        cx.notify();
    }

    /// Ends a live session: its program is hung up and the terminal dropped,
    /// and its pane closes. The pane's sibling takes the room; the last pane
    /// of the workspace returns the main pane to what the folder shows
    /// without terminals.
    pub(super) fn close_live(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(place) = self
            .live
            .get(id)
            .map(|session| (session.machine.clone(), session.cwd.clone()))
        else {
            return;
        };
        let was_open = matches!(self.main, Main::Live(open) if open == id);
        let closed = self.workspaces.close(id);
        if let Some(session) = self.live.get(id) {
            let terminal = session.view.read(cx).terminal();
            if terminal.is_remote() {
                // Closing is an explicit end: hang the program up over there.
                terminal.kill();
            }
        }
        self.live.remove(id);
        self.find.remove(&id);
        self.refresh_live();
        match closed {
            Closed::Focus(next) if was_open => self.open_live(next, window, cx),
            Closed::Empty if was_open => {
                let root = tree::workspace_root(&self.snapshot, &place.0, &place.1);
                self.main = match tree::detail_of_root(&self.snapshot, &place.0, &root) {
                    Some((project, Some(worktree))) => Main::Worktree(project, worktree),
                    Some((project, None)) => Main::Project(project),
                    None => Main::Empty,
                };
                self.pane = Pane::Sidebar;
            }
            _ => {}
        }
        self.sync_focus(window, cx);
        cx.notify();
    }

    /// Moves the keyboard into the terminal on screen, or to the most recent
    /// live session.
    pub(super) fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.main {
            Main::Live(id) if self.live.get(id).is_some() => Some(id),
            _ => self.live.ids().last().copied(),
        };
        match target {
            Some(id) => self.open_live(id, window, cx),
            None => self
                .engine
                .report(StatusKind::Info, "There are no live sessions."),
        }
    }

    /// Opens a history session: a terminal that resumes it in its agent, or
    /// the one that already runs it. Where it cannot be resumed the
    /// transcript is shown instead, with the reason.
    pub(super) fn resume_session(
        &mut self,
        session: Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = session.cwd.clone();
        self.resume_in(session, cwd, window, cx);
    }

    /// [`Self::resume_session`] in another folder of the session's machine
    /// ("Resume in…").
    pub(super) fn resume_in(
        &mut self,
        session: Session,
        cwd: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // One terminal per history session: look at the one that has it.
        if let Some(running) = self.live.of_history(&session.id).map(|live| live.id) {
            self.open_live(running, window, cx);
            return;
        }
        // Another terminal may hold the session: look at the processes now,
        // off the UI thread, before a second agent is started on it. A scan
        // that cannot be made says nothing, and the session opens as ever.
        if self.detecting_elsewhere(cx) {
            let check = self.engine.scan_elsewhere(session.machine_id.clone());
            self.checking = Some(cx.spawn_in(window, async move |this, cx| {
                let _ = check.await;
                this.update_in(cx, |this, window, cx| {
                    this.resume_after_check(session, cwd, window, cx)
                })
                .ok();
            }));
            return;
        }
        self.resume_unchecked(session, cwd, window, cx);
    }

    /// What the fresh look at the processes said: a session another terminal
    /// holds shows its transcript and says so; any other opens as ever.
    fn resume_after_check(
        &mut self,
        session: Session,
        cwd: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Asked twice while it was being checked: it is running here now.
        if let Some(running) = self.live.of_history(&session.id).map(|live| live.id) {
            self.open_live(running, window, cx);
            return;
        }
        match self.elsewhere_of(&session) {
            Some(found) => self.show_elsewhere(session, &found, cx),
            None => self.resume_unchecked(session, cwd, window, cx),
        }
    }

    /// Shows the stored transcript of a session that another terminal runs,
    /// with who holds it and the ways on.
    fn show_elsewhere(
        &mut self,
        session: Session,
        found: &crate::elsewhere::Found,
        cx: &mut Context<Self>,
    ) {
        let likely = !found.is_certain();
        let text = if likely {
            format!(
                "This session is probably running in another terminal ({}). Enter resumes it here.",
                found.describe()
            )
        } else {
            format!(
                "This session is running in another terminal ({}).",
                found.describe()
            )
        };
        self.engine.report(
            StatusKind::Info,
            format!(
                "\"{}\" {} in another terminal. Showing the transcript.",
                session.title,
                if likely { "probably runs" } else { "runs" }
            ),
        );
        let can_reveal = session.machine_id.is_local() && found.app.is_some();
        self.open_transcript(
            session,
            None,
            Some(Notice {
                text,
                other_folder: false,
                elsewhere: Some(ElsewhereNote {
                    pid: found.pid,
                    likely,
                    can_reveal,
                }),
            }),
            cx,
        );
    }

    /// "Resume here anyway": starts the session in Leon although a process in
    /// another terminal holds it. The person has been asked when that was
    /// certain.
    pub(super) fn resume_anyway(
        &mut self,
        session: Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(running) = self.live.of_history(&session.id).map(|live| live.id) {
            self.open_live(running, window, cx);
            return;
        }
        if let Some(found) = self.elsewhere_of(&session) {
            self.engine.report(
                StatusKind::Info,
                format!(
                    "Resuming \"{}\" here although {} also holds it (pid {}).",
                    session.title,
                    if found.is_certain() {
                        "another terminal"
                    } else {
                        "another terminal probably"
                    },
                    found.pid
                ),
            );
        }
        let cwd = session.cwd.clone();
        self.resume_unchecked(session, cwd, window, cx);
    }

    /// Brings forward the terminal application the session of the keyboard
    /// runs in, when the process tree names it; says why not otherwise.
    pub(super) fn reveal_terminal_here(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.here_session() else {
            self.engine
                .report(StatusKind::Info, "Select a history session first.");
            return;
        };
        let app = self
            .elsewhere_of(&session)
            .filter(|_| session.machine_id.is_local())
            .and_then(|found| found.app);
        match app {
            Some(app) => {
                let reveal = self.options.reveal_app.clone();
                reveal(cx, &app.path);
                self.engine.report(
                    StatusKind::Info,
                    format!(
                        "Brought {} forward: find the session in its windows.",
                        app.name
                    ),
                );
            }
            None => self.engine.report(
                StatusKind::Info,
                "Leon cannot tell which application that terminal belongs to.",
            ),
        }
    }

    /// [`Self::resume_in`] once nothing holds the session: what can be known
    /// at once, then a terminal that resumes it.
    fn resume_unchecked(
        &mut self,
        session: Session,
        cwd: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(machine) = self.snapshot.machine(&session.machine_id).cloned() else {
            self.show_transcript_instead(
                session,
                "That machine is not known.".to_owned(),
                false,
                cx,
            );
            return;
        };
        let report = match self.engine.machine_state(&machine.id) {
            MachineState::Online(Some(report)) => Some(report),
            _ => None,
        };
        let launch = Launch::Agent {
            kind: session.agent,
            resume: Some(session.external_id.clone()),
        };
        // What this computer can tell at once: whether the agent is installed
        // here (or, over SSH, in the last probe) and whether the folder is.
        let planned = launch::plan(
            &machine,
            report.as_ref(),
            &cwd,
            &launch,
            &self.engine.ssh(),
            &*self.options.system,
        );
        match planned {
            Err(LaunchError::NoSuchFolder(_)) => {
                let why = format!("The folder {cwd} no longer exists on {}.", machine.name);
                self.show_transcript_instead(session, why, true, cx);
                return;
            }
            Err(
                error @ (LaunchError::NotInstalled { .. }
                | LaunchError::UnknownAgent(_)
                | LaunchError::CannotResume { .. }),
            ) => {
                self.show_transcript_instead(session, error.to_string(), false, cx);
                return;
            }
            Ok(_) => {}
        }
        if machine.kind == MachineKind::Local {
            self.start_resumed(&session, &cwd, window, cx);
            return;
        }
        // Another machine answers in its own time: ask it off the UI thread.
        self.engine.report(
            StatusKind::Busy,
            format!("Checking {cwd} on {}…", machine.name),
        );
        let check = self
            .engine
            .check_target(machine.id.clone(), cwd.clone(), session.agent);
        self.resuming = Some(cx.spawn_in(window, async move |this, cx| {
            let found = check
                .await
                .unwrap_or_else(|_| Target::Offline("The check did not finish.".to_owned()));
            this.update_in(cx, |this, window, cx| {
                this.resume_checked(session, cwd, found, window, cx)
            })
            .ok();
        }));
    }

    /// What the machine said about the folder and the agent.
    fn resume_checked(
        &mut self,
        session: Session,
        cwd: String,
        found: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let machine = self
            .snapshot
            .machine(&session.machine_id)
            .map_or_else(String::new, |machine| machine.name.clone());
        match found {
            Target::Ready => {
                // Asked twice while it was being checked: it is running now.
                if let Some(running) = self.live.of_history(&session.id).map(|live| live.id) {
                    self.open_live(running, window, cx);
                    return;
                }
                self.engine.report(
                    StatusKind::Info,
                    format!("Resumed \"{}\" on {machine}.", session.title),
                );
                self.start_resumed(&session, &cwd, window, cx);
            }
            Target::FolderMissing => {
                let why = format!("The folder {cwd} no longer exists on {machine}.");
                self.show_transcript_instead(session, why, true, cx);
            }
            Target::AgentMissing => {
                let why = LaunchError::NotInstalled {
                    agent: session.agent,
                    machine,
                }
                .to_string();
                self.show_transcript_instead(session, why, false, cx);
            }
            Target::Offline(why) => self.show_transcript_instead(session, why, false, cx),
        }
    }

    /// Starts the terminal that resumes `session`, in a new tab of its
    /// folder's workspace (or as its first pane), with the keyboard.
    fn start_resumed(
        &mut self,
        session: &Session,
        cwd: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let launch = Launch::Agent {
            kind: session.agent,
            resume: Some(session.external_id.clone()),
        };
        self.start_live(
            launch,
            &session.machine_id,
            cwd,
            Place::Tab,
            Some(session.id.clone()),
            window,
            cx,
        );
    }

    /// Shows the stored transcript where a terminal was asked for, and says
    /// why in the status line and at the top of the transcript.
    fn show_transcript_instead(
        &mut self,
        session: Session,
        why: String,
        other_folder: bool,
        cx: &mut Context<Self>,
    ) {
        self.engine.report(
            StatusKind::Error,
            format!("{why} Showing the transcript instead."),
        );
        self.open_transcript(session, None, Some(Notice::plain(why, other_folder)), cx);
    }

    /// Opens a base terminal in a new tab, in the folder the keyboard is on.
    pub(super) fn open_shell_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.here().map(|place| (place.machine, place.cwd)) {
            Some((machine, cwd)) => {
                self.start_live(Launch::Shell, &machine, &cwd, Place::Tab, None, window, cx)
            }
            None => self
                .engine
                .report(StatusKind::Info, "Select a project or a worktree first."),
        }
    }

    // ----- panes ---------------------------------------------------------------------------

    /// The terminal the pane commands act on: the one on screen, or the one
    /// under the cursor.
    fn pane_target(&self) -> Option<LiveId> {
        self.here_live()
    }

    /// The area the panes share, in cells, and the least a pane keeps. Taken
    /// from the window and the size of a cell, so that it is the same whether
    /// or not the terminals have been painted yet.
    pub(super) fn pane_area(&self, cx: &App) -> (Rect, MinSize) {
        let (cell_w, cell_h) = self
            .live
            .all()
            .iter()
            .map(|session| session.cell_size(cx))
            .find(|(w, h)| *w > 0.0 && *h > 0.0)
            .unwrap_or((8.0, 18.0));
        self.pane_area_with(cell_w, cell_h)
    }

    fn pane_area_with(&self, cell_w: f32, cell_h: f32) -> (Rect, MinSize) {
        let width = self.viewport.width.as_f32() - metrics::SIDEBAR_WIDTH().as_f32();
        let height = self.viewport.height.as_f32()
            - metrics::HEADER_HEIGHT().as_f32()
            - metrics::FOOTER_HEIGHT().as_f32()
            - metrics::TAB_BAR_HEIGHT().as_f32();
        (
            Rect::new(
                0.0,
                0.0,
                (width / cell_w).max(1.0),
                (height / cell_h).max(1.0),
            ),
            MIN_PANE,
        )
    }

    /// [`Self::pane_area`] for tests, which have no `App` at hand.
    #[cfg(test)]
    #[cfg_attr(not(unix), allow(dead_code))] // used by the Unix-only tests
    pub(super) fn pane_area_for_test(&self) -> (Rect, MinSize) {
        self.pane_area_with(8.0, 18.0)
    }

    /// Splits the focused pane; the new one is a base terminal on the same
    /// machine and in the same folder.
    pub(super) fn split_pane(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.pane_target() else {
            self.engine
                .report(StatusKind::Info, "There is no terminal to split.");
            return;
        };
        let Some((machine, cwd)) = self
            .live
            .get(id)
            .map(|session| (session.machine.clone(), session.cwd.clone()))
        else {
            return;
        };
        self.start_live(
            Launch::Shell,
            &machine,
            &cwd,
            Place::Split(id, axis),
            None,
            window,
            cx,
        );
    }

    /// Moves the focus to the neighbouring pane in a direction.
    pub(super) fn focus_pane_dir(&mut self, dir: Dir, window: &mut Window, cx: &mut Context<Self>) {
        let (area, _) = self.pane_area(cx);
        let Some(id) = self.pane_target() else { return };
        let moved = self.workspaces.tab_of_mut(id).is_some_and(|tab| {
            tab.focus = id;
            tab.focus_dir(dir, area)
        });
        if moved {
            if let Some(next) = self.workspaces.tab_of(id).map(|tab| tab.focus) {
                self.open_live(next, window, cx);
            }
        }
    }

    /// Moves the focus to the next or the previous pane of the tab.
    pub(super) fn focus_pane_step(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.pane_target() else { return };
        let moved = self.workspaces.tab_of_mut(id).is_some_and(|tab| {
            tab.focus = id;
            tab.focus_step(delta)
        });
        if moved {
            if let Some(next) = self.workspaces.tab_of(id).map(|tab| tab.focus) {
                self.open_live(next, window, cx);
            }
        }
    }

    /// Moves the focused pane's divider a step.
    pub(super) fn resize_pane(&mut self, dir: Dir, cx: &mut Context<Self>) {
        let (area, min) = self.pane_area(cx);
        let Some(id) = self.pane_target() else { return };
        if let Some(tab) = self.workspaces.tab_of_mut(id) {
            if tab.layout.resize(id, dir, RESIZE_STEP, area, min) {
                cx.notify();
            }
        }
    }

    /// Makes every pane of the tab the same size.
    pub(super) fn equalize_panes(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.pane_target() else { return };
        if let Some(tab) = self.workspaces.tab_of_mut(id) {
            tab.layout.equalize();
            cx.notify();
        }
    }

    /// Maximises the focused pane, or restores the layout.
    pub(super) fn toggle_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.pane_target() else { return };
        if let Some(tab) = self.workspaces.tab_of_mut(id) {
            tab.focus = id;
            tab.toggle_zoom();
        }
        self.open_live(id, window, cx);
    }

    /// Shows the next or the previous tab of the workspace.
    pub(super) fn step_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.pane_target() else { return };
        if let Some(next) = self.workspaces.step_tab(id, delta) {
            self.open_live(next, window, cx);
        }
    }

    /// Shows tab `number` (from 1) of the workspace.
    pub(super) fn goto_tab(&mut self, number: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.pane_target() else { return };
        match self.workspaces.goto_tab(id, number) {
            Some(next) => self.open_live(next, window, cx),
            None => self
                .engine
                .report(StatusKind::Info, format!("There is no tab {number}.")),
        }
    }

    /// A divider was dragged: the split's ratio follows the pointer.
    fn drag_divider(
        &mut self,
        drag: &DividerDrag,
        event: &DragMoveEvent<DividerDrag>,
        axis: Axis,
        cx: &mut Context<Self>,
    ) {
        let (area, min) = self.pane_area(cx);
        let (at, length) = match axis {
            Axis::Row => (
                (event.event.position.x - event.bounds.origin.x).as_f32(),
                event.bounds.size.width.as_f32(),
            ),
            Axis::Column => (
                (event.event.position.y - event.bounds.origin.y).as_f32(),
                event.bounds.size.height.as_f32(),
            ),
        };
        let wanted = Layout::ratio_from_pointer(at, length);
        if let Some(tab) = self.workspaces.tab_of_mut(drag.of) {
            if tab.layout.set_ratio(&drag.path, wanted, area, min) {
                cx.notify();
            }
        }
    }

    /// Offers a keystroke to the terminal that has the keyboard. `true` when
    /// it was a terminal key.
    pub(super) fn terminal_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let Main::Live(id) = self.main else {
            return false;
        };
        let Some(view) = self.live.get(id).map(|session| session.view.clone()) else {
            return false;
        };
        view.update(cx, |view, cx| view.handle_keystroke(stroke, cx))
    }

    /// Copy, paste and the scrollback keys, for the terminal on screen.
    pub(super) fn terminal_action(
        &mut self,
        command: crate::keys::Command,
        cx: &mut Context<Self>,
    ) {
        use crate::keys::Command as C;
        let Main::Live(id) = self.main else { return };
        let Some(view) = self.live.get(id).map(|session| session.view.clone()) else {
            return;
        };
        view.update(cx, |view, cx| match command {
            C::Copy => {
                view.copy(cx);
            }
            C::Paste => {
                view.paste(cx);
            }
            C::ScrollPageUp => view.scroll_page_up(cx),
            C::ScrollPageDown => view.scroll_page_down(cx),
            _ => {}
        });
    }

    // ----- drawing -------------------------------------------------------------------------

    /// The workspace of a live session: its tab bar when it has several tabs,
    /// and its active tab's panes.
    pub(super) fn render_live(
        &self,
        id: LiveId,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let nothing = || {
            div()
                .debug_selector(|| "live-none".into())
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(super::lines::empty_frame(
                    "live-none",
                    div()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_faint)
                        .child("No terminal here."),
                    Some("NO TERMINAL".to_owned()),
                    colours,
                ))
                .into_any_element()
        };
        let Some(workspace) = self.workspaces.workspace_of(id) else {
            return nothing();
        };
        let Some(tab) = workspace.tabs.get(workspace.active) else {
            return nothing();
        };
        let many_tabs = workspace.tabs.len() > 1;
        let panes = tab.layout.leaves().len();
        let body = if tab.zoomed {
            self.render_pane(tab.focus, false, colours, cx)
        } else {
            self.render_layout(
                &tab.layout,
                Vec::new(),
                tab.focus,
                panes > 1,
                id,
                colours,
                cx,
            )
        };
        div()
            .debug_selector(|| "live-terminal".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(colours.background)
            .when(many_tabs, |this| {
                this.child(self.render_tab_bar(workspace, colours, cx))
            })
            .child(div().flex_1().min_h_0().min_w_0().child(body))
            .into_any_element()
    }

    fn render_tab_bar(
        &self,
        workspace: &workspace::Workspace,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut bar = div()
            .debug_selector(|| "terminal-tabs".into())
            .flex_none()
            .h(metrics::TAB_BAR_HEIGHT())
            .border_b_1()
            .border_color(colours.border)
            .flex()
            .items_stretch();
        for (index, tab) in workspace.tabs.iter().enumerate() {
            let focus = tab.focus;
            let active = index == workspace.active;
            let (label, agent) = self.live.get(focus).map_or_else(
                || (String::new(), None),
                |session| (session.label(), session.shown_agent()),
            );
            let panes = tab.layout.leaves().len();
            bar = bar.child(
                div()
                    .id(("tab", index))
                    .debug_selector(move || format!("terminal-tab-{index}"))
                    .flex_none()
                    .max_w(px(220.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .border_b_2()
                    .border_color(if active {
                        colours.signal
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .text_color(if active {
                        colours.text
                    } else {
                        colours.text_muted
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_live(focus, window, cx);
                    }))
                    .child(
                        super::widgets::mono(format!("{}", index + 1))
                            .text_color(colours.text_faint),
                    )
                    .children(agent.map(|agent| agent_icon(agent, px(12.), colours)))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(metrics::TEXT_SMALL())
                            .font_weight(if active {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(label),
                    )
                    .when(panes > 1, |this| {
                        this.child(
                            super::widgets::mono(format!("{panes}")).text_color(colours.text_faint),
                        )
                    }),
            );
        }
        bar
    }

    /// One pane: the terminal, marked with the accent when it is the focused
    /// one of several.
    fn render_pane(
        &self,
        id: LiveId,
        marked: bool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(session) = self.live.get(id) else {
            return div().into_any_element();
        };
        div()
            .debug_selector(move || format!("pane-{}", id.0))
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .rounded(metrics::RADIUS_CELL())
            .border_1()
            .border_color(if marked {
                colours.signal
            } else {
                gpui_kit::transparent_black()
            })
            .child(session.view.clone())
            // Files dropped from the file manager paste their quoted paths.
            .on_drop(
                cx.listener(move |this, dropped: &gpui_kit::ExternalPaths, _, cx| {
                    this.paste_dropped(id, dropped.paths(), cx);
                }),
            )
            // The pane that has the keyboard keeps its accent outline and, in a
            // theme with corner ticks, wears them at its corners in the accent.
            .when(marked, |this| {
                this.children(super::lines::focus_ticks(
                    &format!("pane-{}", id.0),
                    colours,
                ))
            })
            .children(self.render_find_bar(id, colours, cx))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_layout(
        &self,
        layout: &Layout,
        path: Path,
        focus: LiveId,
        several: bool,
        of: LiveId,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            Layout::Leaf(id) => self.render_pane(*id, several && *id == focus, colours, cx),
            Layout::Split { axis, ratio, a, b } => {
                let axis = *axis;
                let mut first_path = path.clone();
                first_path.push(false);
                let mut second_path = path.clone();
                second_path.push(true);
                let first = self.render_layout(a, first_path, focus, several, of, colours, cx);
                let second = self.render_layout(b, second_path, focus, several, of, colours, cx);
                let drag_path = path.clone();
                let move_path = path.clone();
                let selector = path
                    .iter()
                    .map(|second| if *second { '1' } else { '0' })
                    .collect::<String>();
                let divider_id = format!("divider-{selector}");
                let handle_selector = divider_id.clone();
                // The divider is a hairline; the handle that takes the mouse is
                // wider than it is, centred on it.
                let hit = px(7.);
                let handle = match axis {
                    Axis::Row => div()
                        .occlude()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(-(hit - hairline()) / 2.0)
                        .w(hit)
                        .cursor_col_resize(),
                    Axis::Column => div()
                        .occlude()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(-(hit - hairline()) / 2.0)
                        .h(hit)
                        .cursor_row_resize(),
                };
                let divider = div()
                    .id(gpui_kit::SharedString::from(divider_id.clone()))
                    .debug_selector(move || handle_selector.clone())
                    .relative()
                    .flex_none()
                    .bg(colours.border)
                    .map(|this| match axis {
                        Axis::Row => this.w(hairline()).h_full(),
                        Axis::Column => this.h(hairline()).w_full(),
                    })
                    .child(
                        handle
                            .id(gpui_kit::SharedString::from(format!("{divider_id}-handle")))
                            .on_drag(
                                DividerDrag {
                                    path: drag_path,
                                    of,
                                },
                                |_, _, _, cx| cx.new(|_| NoGhost),
                            ),
                    );
                let sized = |child: AnyElement, share: Option<f32>| {
                    let wrapper = div().min_w_0().min_h_0().overflow_hidden();
                    match (axis, share) {
                        (Axis::Row, Some(share)) => wrapper.flex_none().h_full().w(relative(share)),
                        (Axis::Column, Some(share)) => {
                            wrapper.flex_none().w_full().h(relative(share))
                        }
                        (Axis::Row, None) => wrapper.flex_1().h_full(),
                        (Axis::Column, None) => wrapper.flex_1().w_full(),
                    }
                    .child(child)
                };
                div()
                    .size_full()
                    .flex()
                    .when(axis == Axis::Column, |this| this.flex_col())
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<DividerDrag>, _, cx| {
                            let drag = event.drag(cx).clone();
                            if drag.path == move_path {
                                this.drag_divider(&drag, event, axis, cx);
                            }
                        },
                    ))
                    .child(sized(first, Some(*ratio)))
                    .child(divider)
                    .child(sized(second, None))
                    .into_any_element()
            }
        }
    }
}

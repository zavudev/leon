//! Remembering and restoring what was open, in the window.
//!
//! Every change to the terminals (a new one, a closed one, a split, a divider
//! moved, the focus, a rename) is noticed in `render` by comparing the state
//! with what was last written; a change is written after a short pause
//! ([`Options::save_debounce`](super::shell::Options)), so a power cut loses
//! at most that pause. A normal quit writes the state once more and marks the
//! run as ended normally.
//!
//! At start the previous run's state is read and the setting
//! `restore_sessions` decides: `always` reopens it, `ask` shows the question
//! "Restore N sessions from last time?", `never` leaves it (the palette's
//! "Restore last sessions" still brings it back). Reopening builds the tabs
//! and panes at once, with the same layout and focus; every terminal is the
//! base shell in its folder. An agent terminal is *paused*: its resume line
//! is held back until its tab is first shown, or until Enter is pressed in
//! it, so ten restored agents spend nothing until looked at (the setting
//! `restore_resume` = `all` resumes them in the background, a few at a time).
//! A terminal that cannot be reopened (folder gone, agent missing, session id
//! unknown, already running in another terminal) is listed with the reason
//! instead of becoming a broken shell.

use super::live::LiveId;
use super::restore::{self, PAUSED_NOTE};
use super::shell::{Overlay, Shell};
use super::updates_view::pill;
use super::widgets::section_label;
use crate::engine::StatusKind;
use crate::launch::{self, Launch, LaunchError};
use crate::settings;
use crate::theme::{metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, Stateful, Task, Window};
use leon_core::{AgentId, MachineId, SavedState, SavedTab, SavedTerminal, SavedWorkspace, Slot};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// How many paused agents the background resume wakes at once.
pub const RESUME_BATCH: usize = 3;

/// What the window keeps about remembering and restoring.
#[derive(Default)]
pub struct RestoreUi {
    /// What the question is about.
    pub offered: Option<SavedState>,
    /// Whether the run it comes from did not end normally.
    pub unclean: bool,
    /// Whether the person is picking rows.
    pub choosing: bool,
    /// The rows picked.
    pub chosen: HashSet<u64>,
    /// Whether the overlay shows what could not be restored.
    pub report_open: bool,
    /// What could not be restored and why: what it was, and the reason.
    pub failures: Vec<(String, String)>,
    /// The state last written.
    pub last: Option<SavedState>,
    /// The state waiting for its pause to end.
    pub waiting: Option<SavedState>,
    saving: Option<Task<()>>,
    resuming: Option<Task<()>>,
    checking: Option<Task<()>>,
}

/// What becomes of one saved terminal.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Start `launch`; paused when it is an agent that resumes.
    Start {
        launch: Launch,
        title: Option<String>,
    },
    /// Nothing is started; this is why.
    Cannot(String),
}

impl Shell {
    // ----- remembering ----------------------------------------------------------------------

    /// What is open now, in its stored form. A terminal whose program ended
    /// is left out, and its pane with it.
    pub(super) fn snapshot_state(&self, cx: &gpui_kit::App) -> SavedState {
        let alive: HashSet<LiveId> = self
            .live
            .all()
            .iter()
            .filter(|session| session.view.read(cx).terminal().exit_info().is_none())
            .map(|session| session.id)
            .collect();
        let mut workspaces = Vec::new();
        for workspace in self.workspaces.all() {
            let mut tabs = Vec::new();
            let mut active = 0;
            for (position, tab) in workspace.tabs.iter().enumerate() {
                let mut layout = Some(tab.layout.clone());
                for leaf in tab.layout.leaves() {
                    if !alive.contains(&leaf) {
                        layout = layout.and_then(|layout| layout.without(leaf));
                    }
                }
                let Some(layout) = layout else { continue };
                if position == workspace.active {
                    active = tabs.len();
                }
                let focus = if layout.contains(tab.focus) {
                    tab.focus
                } else {
                    layout.leaves()[0]
                };
                tabs.push(SavedTab {
                    layout: restore::to_saved(&layout),
                    focus: focus.0,
                    zoomed: tab.zoomed && layout.leaves().len() > 1,
                });
            }
            if !tabs.is_empty() {
                workspaces.push(SavedWorkspace {
                    key: workspace.key.clone(),
                    active,
                    tabs,
                });
            }
        }
        let shown: HashSet<u64> = workspaces
            .iter()
            .flat_map(|w| w.tabs.iter().flat_map(|t| t.layout.leaves()))
            .collect();
        let terminals = self
            .live
            .all()
            .iter()
            .filter(|session| shown.contains(&session.id.0))
            .map(|session| SavedTerminal {
                id: session.id.0,
                machine: session.machine.as_str().to_owned(),
                cwd: session.cwd.clone(),
                // A shell that got the terminal back from its agent is a shell.
                agent: session.shown_agent().map(|a| a.as_str().to_owned()),
                session: session.learned.as_ref().map(|(id, _)| id.clone()),
                confidence: session.learned.as_ref().map(|(_, how)| how.clone()),
                history: session.history.as_ref().map(|h| h.as_str().to_owned()),
                name: session.name.clone().filter(|name| !name.trim().is_empty()),
                title: session
                    .title
                    .clone()
                    .filter(|title| !title.ends_with(PAUSED_NOTE)),
                started_at: session.started_ms,
                account: session.account.clone(),
            })
            .collect();
        SavedState {
            saved_at: (self.options.now)().timestamp_millis(),
            clean_shutdown: false,
            selection: None,
            main: match self.main {
                super::shell::Main::Live(id) if shown.contains(&id.0) => Some(id.0),
                _ => None,
            },
            workspaces,
            terminals,
        }
    }

    /// Notices a change of what is open and writes it after a pause. Cheap:
    /// called from `render`.
    pub(super) fn watch_workspace(&mut self, cx: &mut Context<Self>) {
        // Once the end has been written, what is left is not a change.
        if self.closing.quitting {
            return;
        }
        // Until the previous run has been dealt with the new state must not
        // replace it: an empty window would erase what could be restored.
        if self.overlay == Overlay::Restore
            && !self.restore.report_open
            && self.restore.offered.is_some()
        {
            return;
        }
        let state = self.snapshot_state(cx);
        let same = |a: &SavedState, b: &SavedState| {
            SavedState {
                saved_at: 0,
                ..a.clone()
            } == SavedState {
                saved_at: 0,
                ..b.clone()
            }
        };
        let known = self.restore.waiting.as_ref().or(self.restore.last.as_ref());
        if known.is_some_and(|known| same(known, &state)) {
            return;
        }
        if known.is_none() && state.is_empty() {
            self.restore.last = Some(state);
            return;
        }
        self.restore.waiting = Some(state);
        if self.restore.saving.is_some() {
            return;
        }
        let pause = self.options.save_debounce;
        self.restore.saving = Some(cx.spawn(async move |this, cx| {
            if !pause.is_zero() {
                cx.background_executor().timer(pause).await;
            }
            this.update(cx, |this, _| {
                this.write_waiting();
                this.restore.saving = None;
            })
            .ok();
        }));
    }

    /// Writes the state that waits, now.
    pub(super) fn write_waiting(&mut self) {
        let Some(state) = self.restore.waiting.take() else {
            return;
        };
        match self.engine.store().save_workspace(Slot::Current, &state) {
            Ok(()) => self.restore.last = Some(state),
            Err(error) => tracing::warn!(%error, "the open terminals could not be remembered"),
        }
    }

    /// The last write of a normal end: what is open now, marked as ended
    /// normally.
    pub(super) fn remember_clean_shutdown(&mut self, cx: &gpui_kit::App) {
        let mut state = self.snapshot_state(cx);
        state.clean_shutdown = true;
        if let Err(error) = self.engine.store().save_workspace(Slot::Current, &state) {
            tracing::warn!(%error, "the open terminals could not be remembered");
        }
    }

    // ----- restoring ------------------------------------------------------------------------

    /// At start: reads what the previous run left and acts on the setting.
    pub(super) fn begin_session_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.engine.store().clone();
        let previous = match store.archive_workspace() {
            Ok(previous) => previous,
            Err(error) => {
                tracing::warn!(%error, "the previous terminals could not be read");
                None
            }
        };
        // This run is not over until it says so.
        let _ = store.set_clean_shutdown(false);
        let Some(state) = previous.filter(|state| !state.is_empty()) else {
            return;
        };
        let unclean = !state.clean_shutdown;
        match settings::text(cx, "restore_sessions").as_str() {
            "always" => {
                self.restore.unclean = unclean;
                self.restore_state(state, None, window, cx);
            }
            "never" => {}
            _ => self.offer_restore(state, unclean, window, cx),
        }
    }

    /// The palette's "Restore last sessions": what the previous run left.
    pub(super) fn restore_last_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kept = self
            .restore
            .offered
            .clone()
            .or_else(|| {
                self.engine
                    .store()
                    .load_workspace(Slot::Previous)
                    .ok()
                    .flatten()
            })
            .filter(|state| !state.is_empty());
        match kept {
            Some(state) => {
                let unclean = !state.clean_shutdown;
                self.offer_restore(state, unclean, window, cx);
            }
            None => self
                .engine
                .report(StatusKind::Info, "There are no sessions from last time."),
        }
    }

    /// Shows the question.
    pub(super) fn offer_restore(
        &mut self,
        state: SavedState,
        unclean: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restore.chosen = state.terminals.iter().map(|t| t.id).collect();
        self.restore.offered = Some(state);
        self.restore.unclean = unclean;
        self.restore.choosing = false;
        self.restore.report_open = false;
        self.overlay = Overlay::Restore;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// "Restore all" / "Restore chosen": reopens and closes the question.
    pub(super) fn restore_offered(
        &mut self,
        only_chosen: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.restore.offered.take() else {
            return;
        };
        let only = only_chosen.then(|| self.restore.chosen.clone());
        self.overlay = Overlay::None;
        self.restore_state(state, only, window, cx);
    }

    /// "Not now": the question closes; what was offered stays available to
    /// the palette's command.
    pub(super) fn dismiss_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore.report_open = false;
        self.overlay = Overlay::None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// What would be done with a saved terminal.
    fn decide(&self, saved: &SavedTerminal, cx: &gpui_kit::App) -> Decision {
        let machine_id = MachineId::from_string(saved.machine.as_str());
        let Some(machine) = self.snapshot.machine(&machine_id).cloned() else {
            return Decision::Cannot("its machine is no longer known".to_owned());
        };
        let report = match self.engine.machine_state(&machine_id) {
            crate::engine::MachineState::Online(Some(report)) => Some(report),
            _ => None,
        };
        let prefs = Self::launch_prefs(cx);
        let plan = |launch: &Launch| {
            launch::plan_with(
                &machine,
                report.as_ref(),
                &saved.cwd,
                launch,
                &self.engine.ssh(),
                &*self.options.system,
                &prefs,
            )
        };
        let why = |error: LaunchError| match error {
            LaunchError::NoSuchFolder(_) => format!("the folder {} no longer exists", saved.cwd),
            other => other.to_string(),
        };
        let Some(agent_id) = saved.agent.as_deref() else {
            return match plan(&Launch::Shell) {
                Ok(_) => Decision::Start {
                    launch: Launch::Shell,
                    title: None,
                },
                Err(error) => Decision::Cannot(why(error)),
            };
        };
        let Some(agent) = AgentId::parse(agent_id) else {
            return Decision::Cannot(format!("the agent {agent_id} is no longer known"));
        };
        let Some(spec) = agent.spec() else {
            return Decision::Cannot(format!("the agent {agent_id} is no longer known"));
        };
        let name = spec.name.clone();
        if !spec.can_resume() {
            // Launch-only agents cannot be picked up again: a shell in the
            // folder, with a note, never a fresh agent started silently.
            return match plan(&Launch::Shell) {
                Ok(_) => Decision::Start {
                    launch: Launch::Shell,
                    title: Some(format!("{name} was running here · now a shell")),
                },
                Err(error) => Decision::Cannot(why(error)),
            };
        }
        let Some(session) = saved.session.clone() else {
            return Decision::Cannot(format!(
                "Leon never learned which {name} session this was, so it cannot be resumed"
            ));
        };
        let launch = Launch::Agent {
            kind: agent,
            resume: Some(session.clone()),
            account: saved.account.clone(),
        };
        if let Err(error) = plan(&launch) {
            return Decision::Cannot(why(error));
        }
        // The same session running in another terminal is not resumed twice.
        if let Ok(Some(row)) = self
            .engine
            .store()
            .session_by_external(&machine_id, agent, &session)
        {
            if let Some(found) = self.elsewhere_of(&row) {
                return Decision::Cannot(format!(
                    "that session is already running in another terminal (process {})",
                    found.pid
                ));
            }
        }
        let label = saved
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| saved.title.clone())
            .unwrap_or(name);
        Decision::Start {
            launch,
            title: Some(format!("{label} · {PAUSED_NOTE}")),
        }
    }

    /// Reopens `state` (only the terminals in `only`, when given): the tabs
    /// and panes at once, the agents paused.
    pub(super) fn restore_state(
        &mut self,
        state: SavedState,
        only: Option<HashSet<u64>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A fresh look at the other terminals first, so a session that is
        // running elsewhere is not started a second time.
        if self.detecting_elsewhere(cx) && state.terminals.iter().any(|t| t.session.is_some()) {
            let check = self.engine.scan_elsewhere(MachineId::local());
            self.restore.checking = Some(cx.spawn_in(window, async move |this, cx| {
                let _ = check.await;
                this.update_in(cx, |this, window, cx| {
                    this.restore_now(state, only, window, cx)
                })
                .ok();
            }));
            return;
        }
        self.restore_now(state, only, window, cx);
    }

    fn restore_now(
        &mut self,
        state: SavedState,
        only: Option<HashSet<u64>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let wanted = |id: u64| only.as_ref().is_none_or(|only| only.contains(&id));
        let mut started: HashMap<u64, LiveId> = HashMap::new();
        let mut failures: Vec<(String, String)> = Vec::new();
        let describe = |t: &SavedTerminal| {
            let what =
                restore::agent_of(t).map_or("Shell", |agent| crate::format::agent_name(agent));
            let folder = std::path::Path::new(&t.cwd)
                .file_name()
                .map_or_else(|| t.cwd.clone(), |n| n.to_string_lossy().into_owned());
            format!("{what} · {folder}")
        };
        for saved in &state.terminals {
            if !wanted(saved.id) {
                continue;
            }
            match self.decide(saved, cx) {
                Decision::Cannot(why) => failures.push((describe(saved), why)),
                Decision::Start { launch, title } => {
                    let machine = MachineId::from_string(saved.machine.as_str());
                    let deferred = matches!(launch, Launch::Agent { .. });
                    let history = saved
                        .history
                        .as_deref()
                        .map(leon_core::SessionId::from_string);
                    match self.spawn_live(
                        launch, &machine, &saved.cwd, history, deferred, title, window, cx,
                    ) {
                        Some(id) => {
                            if let Some(session) = self.live.get_mut(id) {
                                session.name = saved.name.clone();
                                session.started_ms = saved.started_at;
                            }
                            started.insert(saved.id, id);
                        }
                        None => failures.push((describe(saved), "it could not be started".into())),
                    }
                }
            }
        }
        // The layout of each tab, with the panes that could not be started
        // given back to their siblings.
        for workspace in &state.workspaces {
            let mut tabs = Vec::new();
            let mut active = 0;
            for (position, tab) in workspace.tabs.iter().enumerate() {
                let Some(layout) = restore::from_saved(&tab.layout, &started) else {
                    continue;
                };
                if position == workspace.active {
                    active = tabs.len();
                }
                let focus = started
                    .get(&tab.focus)
                    .copied()
                    .filter(|id| layout.contains(*id))
                    .unwrap_or_else(|| layout.leaves()[0]);
                let zoomed = tab.zoomed && layout.leaves().len() > 1;
                tabs.push(super::panes::Tab {
                    layout,
                    focus,
                    zoomed,
                });
            }
            self.workspaces.restore(&workspace.key, tabs, active);
        }
        self.refresh_live();
        // The tab that was on screen is shown, and with it resumed.
        let shown = state
            .main
            .and_then(|id| started.get(&id).copied())
            .or_else(|| self.live.ids().into_iter().last());
        if let Some(id) = shown.filter(|id| started.values().any(|s| s == id)) {
            self.open_live(id, window, cx);
        }
        if settings::text(cx, "restore_resume") == "all" {
            self.resume_in_background(cx);
        }
        self.restore.failures = failures;
        let count = started.len();
        let failed = self.restore.failures.len();
        if failed > 0 {
            self.restore.report_open = true;
            self.overlay = Overlay::Restore;
            self.restore.offered = None;
        }
        self.engine.report(
            StatusKind::Info,
            match failed {
                0 => format!("Restored {count} sessions."),
                _ => format!("Restored {count} sessions; {failed} could not be restored."),
            },
        );
        cx.notify();
    }

    /// Wakes every paused agent, [`RESUME_BATCH`] at a time, a second apart.
    fn resume_in_background(&mut self, cx: &mut Context<Self>) {
        self.restore.resuming = Some(cx.spawn(async move |this, cx| loop {
            let more = this
                .update(cx, |this, cx| {
                    let paused: Vec<LiveId> = this
                        .live
                        .all()
                        .iter()
                        .filter(|session| session.is_paused())
                        .map(|session| session.id)
                        .take(RESUME_BATCH)
                        .collect();
                    for id in &paused {
                        this.resume_paused(*id, cx);
                    }
                    this.live.all().iter().any(|session| session.is_paused())
                })
                .unwrap_or(false);
            if !more {
                return;
            }
            cx.background_executor().timer(Duration::from_secs(1)).await;
        }));
    }

    /// Resumes every paused session of the tab holding `id`: the tab is
    /// being shown.
    pub(super) fn resume_tab_of(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let panes = self
            .workspaces
            .tab_of(id)
            .map_or_else(|| vec![id], |tab| tab.layout.leaves());
        for pane in panes {
            self.resume_paused(pane, cx);
        }
    }

    /// Enter in a paused terminal resumes it instead of reaching the shell.
    pub(super) fn paused_enter(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &stroke.modifiers;
        if stroke.key != "enter" || m.platform || m.control || m.alt || m.shift {
            return false;
        }
        match self.main {
            super::shell::Main::Live(id) => self.resume_paused(id, cx),
            _ => false,
        }
    }

    /// Says an event of a session the way the settings ask, as a wake-up of
    /// its terminal does.
    #[cfg(all(test, leon_posix_tests))]
    pub(super) fn raise_for_test(
        &mut self,
        event: super::notify::Event,
        id: LiveId,
        cx: &mut Context<Self>,
    ) {
        self.raise(event, id, cx);
    }

    /// Shows the tab of a terminal, as clicking its row does.
    #[cfg(all(test, leon_posix_tests))]
    pub(super) fn pending_open_for_test(&mut self, id: LiveId, cx: &mut Context<Self>) {
        self.resume_tab_of(id, cx);
    }

    // ----- the overlay ----------------------------------------------------------------------

    pub(super) fn restore_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &stroke.modifiers;
        if m.platform || m.control || m.alt || m.shift {
            return false;
        }
        match stroke.key.as_str() {
            "enter" if self.restore.report_open => self.dismiss_restore(window, cx),
            "enter" => self.restore_offered(self.restore.choosing, window, cx),
            "c" if !self.restore.report_open => {
                self.restore.choosing = !self.restore.choosing;
                cx.notify();
            }
            _ => return false,
        }
        true
    }

    pub(super) fn render_restore(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let now = self.now();
        let (title, rows): (String, Vec<(Option<u64>, String, String)>) =
            if self.restore.report_open {
                (
                    "Some sessions could not be restored".to_owned(),
                    self.restore
                        .failures
                        .iter()
                        .map(|(what, why)| (None, what.clone(), why.clone()))
                        .collect(),
                )
            } else {
                let state = self.restore.offered.clone().unwrap_or_default();
                (
                    restore::question(state.terminals.len(), self.restore.unclean),
                    restore::rows(&state, now)
                        .into_iter()
                        .map(|row| {
                            let detail = format!(
                                "{} · {} ago",
                                if row.title.is_empty() {
                                    row.folder.clone()
                                } else {
                                    format!("{} · {}", row.title, row.folder)
                                },
                                row.ago
                            );
                            (Some(row.id), row.what, detail)
                        })
                        .collect(),
                )
            };
        let choosing = self.restore.choosing && !self.restore.report_open;
        let mut body = div()
            .id("restore-rows")
            .debug_selector(|| "restore-rows".into())
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap(px(4.));
        for (id, what, detail) in rows {
            let mark = match (choosing, id) {
                (true, Some(id)) if self.restore.chosen.contains(&id) => "[x] ",
                (true, Some(_)) => "[ ] ",
                _ => "",
            };
            let mut row = div()
                .id(("restore-row", id.unwrap_or(0) as usize))
                .debug_selector(move || format!("restore-row-{}", id.unwrap_or(0)))
                .flex()
                .gap_3()
                .font_family(crate::theme::fonts::mono())
                .text_size(metrics::TEXT_SMALL())
                .child(div().flex_none().w(px(150.)).child(format!("{mark}{what}")))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(colours.text_muted)
                        .child(detail),
                );
            if let (true, Some(id)) = (choosing, id) {
                row = row
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.restore.chosen.remove(&id) {
                            this.restore.chosen.insert(id);
                        }
                        cx.notify();
                    }));
            }
            body = body.child(row);
        }
        let report = self.restore.report_open;
        self.card("restore", colours)
            .w(px(620.))
            .max_w(self.viewport.width - px(32.))
            .max_h(self.viewport.height - px(112.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py(px(10.))
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section_label(
                        if report { "Restore" } else { "Last time" },
                        colours,
                    ))
                    .child(div().debug_selector(|| "restore-title".into()).child(title))
                    .when(!report, |this| {
                        this.child(
                            div()
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(colours.text_muted)
                                .child("Agents stay paused until you open them. Scrollback is not restored."),
                        )
                    }),
            )
            .child(body)
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py(px(8.))
                    .border_t_1()
                    .border_color(colours.border)
                    .flex()
                    .gap(px(8.))
                    .when(report, |this| {
                        this.child(pill("restore-ok", "OK", colours).on_click(cx.listener(
                            |this, _, window, cx| this.dismiss_restore(window, cx),
                        )))
                    })
                    .when(!report && !choosing, |this| {
                        this.child(pill("restore-all", "RESTORE ALL", colours).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.restore_offered(false, window, cx)
                            }),
                        ))
                        .child(pill("restore-choose", "CHOOSE", colours).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.restore.choosing = true;
                                cx.notify();
                            }),
                        ))
                    })
                    .when(!report && choosing, |this| {
                        this.child(pill("restore-chosen", "RESTORE CHOSEN", colours).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.restore_offered(true, window, cx)
                            }),
                        ))
                        .child(pill("restore-back", "BACK", colours).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.restore.choosing = false;
                                cx.notify();
                            },
                        )))
                    })
                    .when(!report, |this| {
                        this.child(pill("restore-not-now", "NOT NOW", colours).on_click(
                            cx.listener(|this, _, window, cx| this.dismiss_restore(window, cx)),
                        ))
                    }),
            )
    }
}

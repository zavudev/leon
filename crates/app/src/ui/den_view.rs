//! The Den in the window: the live sessions as lions at work.
//!
//! The Den takes the place of the main pane ([`Main::Den`]); the sidebar
//! stays. The command that opens it closes it again, as Escape does, and
//! both put back what the main pane showed (Escape first lets go of the
//! feed's entry and of the selection). Arrows and Tab move the selection,
//! which narrows the feed to that lion; Page Up, Page Down, Home and End move
//! the feed, Shift with Up or Down walks its entries and Enter then opens a
//! long message. Enter and a double click open the selected session's terminal
//! (a sub-agent opens its parent's). `E` opens the editor of the room, which
//! has keys of its own ([`super::den_edit`]).
//!
//! # Where the facts come from
//!
//! Every live session with an agent in front of its shell is a lion; a
//! plain shell is none ([`super::den::cubs`]). What it does is decided by [`super::den`]
//! from two things: what its terminal says (the same [`Activity`] the
//! sidebar's dot shows) and, for a session of this computer whose agent
//! Leon can follow and whose own session id was learned, its transcript,
//! read by [`super::den_follow`] off the window's thread.
//!
//! # Cost
//!
//! Nothing is followed while the Den is closed: opening it reads each
//! transcript once from its start (tens of milliseconds for tens of
//! megabytes, on the background executor), then only what is appended, once
//! every [`Options::den_tick`]. The loop ends when the Den is closed or no
//! session is live, and the followers of sessions that ended are dropped.
//!
//! [`Activity`]: super::activity::Activity
//! [`Options::den_tick`]: super::shell::Options

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, Context, Entity, Keystroke, Subscription, Task, Window};
use leon_den::{Cub, DenEvent, DenPalette, DenStyle, DenView, Tokens};
use leon_history::live::{Beat, Format};

use super::den::{self, Facts, Reading};
use super::den_follow::{Followers, Report, Wanted};
use super::live::LiveId;
use super::shell::{Main, Pane, Shell};
use crate::settings;
use crate::theme::{fonts, metrics, Palette};

/// What the window keeps for the Den.
#[derive(Default)]
pub struct DenUi {
    /// The view, made the first time the Den is opened.
    pub(super) view: Option<Entity<DenView>>,
    events: Option<Subscription>,
    /// What the main pane showed before the Den, and which pane had the
    /// keyboard: what closing it puts back.
    back: Option<(Main, Pane)>,
    /// The transcripts as far as they were read, by live session, with the
    /// agent's session id each belongs to.
    readings: HashMap<u64, (String, Reading)>,
    followers: Arc<Mutex<Followers>>,
    /// The loop that polls the transcripts while the Den is open.
    watch: Option<Task<()>>,
    /// The lions last shown: what the next ones are compared with.
    pub(super) cubs: Vec<Cub>,
    /// When each followed transcript was last written, in seconds since the
    /// Unix epoch.
    written: HashMap<u64, i64>,
    /// The Den was just opened: what the next poll reads is what happened
    /// while it was closed, to be summed up and not told line by line.
    catching_up: bool,
    /// The den in use, once it was read.
    pub(super) active: Option<super::den_edit::ActiveDen>,
    /// What the strip under the room shows while it is edited.
    pub(super) tab: super::den_edit::Tab,
    /// The pictures of the strip, made once each: the image and its size in
    /// device pixels.
    pub(super) thumbs: std::cell::RefCell<HashMap<String, super::den_edit::Thumb>>,
}

impl DenUi {
    /// Whether no loop reads transcripts now.
    #[cfg(test)]
    #[cfg_attr(not(leon_posix_tests), allow(dead_code))] // used by the Unix-only tests
    pub(super) fn watch_is_idle(&self) -> bool {
        self.watch.is_none()
    }
}

/// A session of this computer that runs in another window of Leon or in a
/// plain terminal, and that the Den can follow.
#[derive(Debug, Clone)]
pub(super) struct AwaySession {
    /// The lion's id.
    pub id: u64,
    /// What the sidebar calls the session, or its agent and folder when the
    /// history does not know it yet.
    pub name: String,
    /// Its agent.
    pub agent: leon_core::AgentId,
    /// The agent's own id of the session.
    pub external: String,
    /// The folder it runs in.
    pub cwd: String,
    /// The process that holds it.
    pub found: crate::elsewhere::Found,
    /// The session in the history, when it is there.
    pub stored: Option<leon_core::Session>,
}

/// What the feed is told of a transcript.
enum Told {
    /// Something the narrator says.
    Happened(leon_den::Happening),
    /// What the agent wrote to the user: the lion, its name, the text and
    /// its time.
    Said(u64, String, String, Option<i64>),
    /// What a session did before: the lion, its name and its past.
    Past(u64, String, Vec<leon_den::feed::Past>),
}

/// The colours of the Den's chrome in a theme.
pub fn den_palette(colours: &Palette) -> DenPalette {
    DenPalette::from_tokens(&Tokens {
        background: colours.background,
        surface: colours.surface,
        surface_2: colours.surface_2,
        border: colours.border,
        guide: colours.guide,
        grid_mark: colours.grid_mark,
        text: colours.text,
        text_muted: colours.text_muted,
        text_faint: colours.text_faint,
        signal: colours.signal,
        accent_fill: colours.accent_fill,
        on_accent_fill: colours.on_accent_fill,
        success: colours.success,
        warning: colours.warning,
        error: colours.error,
        info: colours.info,
    })
}

impl Shell {
    /// Whether the Den is what the main pane shows.
    pub(super) fn den_open(&self) -> bool {
        matches!(self.main, Main::Den)
    }

    fn den_style(&self, cx: &gpui_kit::App) -> DenStyle {
        DenStyle {
            palette: den_palette(&crate::theme::palette(cx)),
            font_family: fonts::mono().into(),
            font_size: metrics::TEXT_SMALL(),
        }
    }

    /// Opens the Den in the main pane, or closes it when it is open.
    pub(super) fn toggle_den(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.den_open() {
            self.close_den(window, cx);
            return;
        }
        self.close_overlay(window, cx);
        if self.den.view.is_none() {
            let style = self.den_style(cx);
            let view = cx.new(|cx| DenView::new(style, cx));
            self.den.events = Some(cx.subscribe_in(
                &view,
                window,
                |this, _, event: &DenEvent, window, cx| match event {
                    DenEvent::Opened(id) => this.den_open_lion(*id, window, cx),
                    DenEvent::LayoutChanged => this.den_keep(cx),
                    DenEvent::Clicked(_) | DenEvent::Selected(_) => {}
                },
            ));
            self.den.view = Some(view);
        }
        // The den of the setting, as its file is now.
        self.den_load(cx);
        // Nobody is selected in a den just opened: the feed is everybody's.
        if let Some(view) = self.den.view.clone() {
            view.update(cx, |den, cx| den.select(None, cx));
        }
        // Who runs elsewhere, as of now rather than as of the last look.
        self.scan_elsewhere_now(false, cx);
        // The cursor of the sidebar is on its row, as on a project that is
        // opened.
        self.home.nav = Some(super::home::Nav::Den);
        let before = std::mem::replace(&mut self.main, Main::Den);
        self.den.back = Some((before, self.pane));
        self.pane = Pane::Main;
        self.focus.focus(window, cx);
        // What changed while it was closed is not news: no line is said of
        // it, the lions are simply where they are. What the transcripts
        // grew by meanwhile is the past: the next poll sums it up.
        self.den.catching_up = true;
        self.den_refresh(false, cx);
        self.den_watch(cx);
        cx.notify();
    }

    /// Closes the Den and puts back what the main pane showed.
    pub(super) fn close_den(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.den_open() {
            return;
        }
        self.den.watch = None;
        if let Some(view) = self.den.view.clone() {
            view.update(cx, |den, cx| den.stop_editing(cx));
        }
        let (main, pane) = self.den.back.take().unwrap_or((Main::Empty, Pane::Sidebar));
        match main {
            // A terminal takes the keyboard back as when it is opened.
            Main::Live(id) if self.live.get(id).is_some() => {
                self.main = Main::Empty;
                self.open_live(id, window, cx);
                self.pane = pane;
            }
            Main::Live(_) | Main::Den => {
                self.main = Main::Empty;
                self.pane = Pane::Sidebar;
            }
            other => {
                self.main = other;
                self.pane = pane;
            }
        }
        // Back at the home, its row has the cursor; back anywhere else, the
        // tree has it again.
        self.home.nav = matches!(self.main, Main::Empty).then_some(super::home::Nav::Home);
        self.sync_focus(window, cx);
        cx.notify();
    }

    /// Leaves the Den for something else that takes the main pane: nothing
    /// is put back, the editor closes and the transcripts stop being read.
    pub(super) fn den_leave(&mut self, cx: &mut Context<Self>) {
        if !self.den_open() {
            return;
        }
        self.den.watch = None;
        self.den.back = None;
        self.home.nav = None;
        if let Some(view) = self.den.view.clone() {
            view.update(cx, |den, cx| den.stop_editing(cx));
        }
    }

    /// Opens the terminal of a lion: the session's own, or its parent's for
    /// a little one.
    fn den_open_lion(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let session = self
            .den
            .cubs
            .iter()
            .find(|cub| cub.id == id)
            .map(|cub| cub.parent.unwrap_or(cub.id));
        // A session that runs elsewhere has no terminal here: what Leon
        // shows of it is its transcript, with who holds it and the ways on.
        // Nothing is done to the other process.
        if let Some(away) = session.filter(|id| den::is_away(*id)) {
            let Some(away) = self.den_away(cx).into_iter().find(|one| one.id == away) else {
                return;
            };
            match away.stored {
                Some(stored) => {
                    self.den.watch = None;
                    self.den.back = None;
                    self.home.nav = None;
                    self.show_elsewhere(stored, &away.found, cx);
                }
                None => self.engine.report(
                    crate::engine::StatusKind::Info,
                    format!(
                        "{} runs elsewhere ({}) and is not in the history yet: there is no transcript to open.",
                        away.name,
                        away.found.describe()
                    ),
                ),
            }
            return;
        }
        let Some(live) = session
            .map(LiveId)
            .filter(|id| self.live.get(*id).is_some())
        else {
            return;
        };
        self.den.watch = None;
        self.den.back = None;
        self.home.nav = None;
        self.open_live(live, window, cx);
    }

    /// What a key does while the Den has the keyboard. `true` when taken.
    pub(super) fn den_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // The editor has keys of its own, and the lions are not walked
        // through while the furniture is.
        if self.den_editing(cx) {
            return self.den_edit_key(stroke, cx);
        }
        let m = &stroke.modifiers;
        if m.platform || m.control || m.alt || m.function {
            return false;
        }
        let Some(view) = self.den.view.clone() else {
            return false;
        };
        match (stroke.key.as_str(), m.shift) {
            // Escape lets go a step at a time: the entry of the feed the
            // keyboard is on, then the selected lion (the feed is
            // everybody's again), then the Den.
            ("escape", false) => {
                let held = view.update(cx, |den, cx| {
                    if den.feed_release(cx) {
                        return true;
                    }
                    let selected = den.selected().is_some();
                    den.select(None, cx);
                    selected
                });
                if !held {
                    self.close_den(window, cx);
                }
            }
            ("pageup", false) => view.update(cx, |den, cx| den.scroll_feed(-1, cx)),
            ("pagedown", false) => view.update(cx, |den, cx| den.scroll_feed(1, cx)),
            ("home", false) => view.update(cx, |den, cx| den.feed_to(false, cx)),
            ("end", false) => view.update(cx, |den, cx| den.feed_to(true, cx)),
            ("up", true) => view.update(cx, |den, cx| den.feed_step(true, cx)),
            ("down", true) => view.update(cx, |den, cx| den.feed_step(false, cx)),
            ("e", false) => self.den_tool(super::den_edit::Tool::Edit, cx),
            ("tab", true) | ("up", false) | ("left", false) => {
                view.update(cx, |den, cx| den.select_previous(cx));
            }
            ("tab", false) | ("down", false) | ("right", false) => {
                view.update(cx, |den, cx| den.select_next(cx));
            }
            // On an entry of the feed, Enter opens its message; otherwise it
            // opens the selected lion's session.
            ("enter", false) => {
                view.update(cx, |den, cx| {
                    if !den.feed_toggle(cx) {
                        den.open_selection(cx);
                    }
                });
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// What the terminals say of each live session, in the order they were
    /// started.
    fn den_facts(&self, cx: &gpui_kit::App) -> Vec<Facts> {
        let colours = crate::theme::palette(cx);
        self.live
            .all()
            .iter()
            .map(|session| {
                let signals = session.signals(cx);
                Facts {
                    id: session.id.0,
                    name: session.label(),
                    tint: session
                        .agent
                        .map_or(colours.text_muted, |agent| colours.agent(agent)),
                    agent: session.shown_agent().is_some(),
                    activity: session.activity,
                    paused: session.is_paused(),
                    exit: signals.exited,
                    quiet_for: signals.quiet_for,
                }
            })
            .collect()
    }

    /// The sessions of this computer that run elsewhere and that the Den can
    /// follow: a process the last scan found outside this window, that
    /// names its session (a certain match), of an agent whose transcript
    /// Leon reads, in a known folder. Never one this window runs itself, and
    /// each session once.
    pub(super) fn den_away(&self, cx: &gpui_kit::App) -> Vec<AwaySession> {
        if !settings::flag(cx, "den_elsewhere") {
            return Vec::new();
        }
        let local = leon_core::MachineId::local();
        let Some(report) = self.engine.elsewhere(&local) else {
            return Vec::new();
        };
        let own: Vec<&str> = self
            .live
            .all()
            .iter()
            .filter_map(|session| {
                session
                    .learned
                    .as_ref()
                    .map(|(id, _)| id.as_str())
                    .or(session.resumed.as_deref())
            })
            .collect();
        let mut out: Vec<AwaySession> = Vec::new();
        for found in crate::elsewhere::foreign(&report.found, true, &self.own_terminals(&local)) {
            let Some(external) = found.external_id.clone().filter(|_| found.is_certain()) else {
                continue;
            };
            if Format::of(found.agent).is_none() || own.contains(&external.as_str()) {
                continue;
            }
            let id = den::away_id(found.agent.as_str(), &external);
            if out.iter().any(|one| one.id == id) {
                continue;
            }
            let stored = found
                .session
                .as_ref()
                .and_then(|id| self.snapshot.sessions.iter().find(|s| &s.id == id))
                .cloned();
            let Some(cwd) = found
                .cwd
                .clone()
                .or_else(|| stored.as_ref().map(|stored| stored.cwd.clone()))
                .filter(|cwd| !cwd.is_empty())
            else {
                continue;
            };
            let name = stored
                .as_ref()
                .map(|stored| stored.title.trim().to_owned())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| den::away_name(found.agent.name(), Some(&cwd)));
            out.push(AwaySession {
                id,
                name,
                agent: found.agent,
                external,
                cwd,
                found,
                stored,
            });
        }
        out
    }

    /// The sub-agents of a followed session that are alive, as far as its
    /// transcript was read.
    fn den_subagents(&self, id: u64, session: &str) -> Vec<(String, Option<String>)> {
        self.den
            .readings
            .get(&id)
            .filter(|(read, _)| read == session)
            .map(|(_, reading)| {
                reading
                    .pulse
                    .subagents()
                    .iter()
                    .map(|sub| (sub.tool.clone(), sub.agent.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The transcripts to follow: those of this window's own sessions and
    /// those of the sessions that run elsewhere.
    fn den_wanted_all(&self, cx: &gpui_kit::App) -> Vec<Wanted> {
        let mut wanted = self.den_wanted();
        wanted.extend(self.den_away(cx).into_iter().map(|away| Wanted {
            live: away.id,
            agent: away.agent,
            subagents: self.den_subagents(away.id, &away.external),
            cwd: away.cwd,
            session: away.external,
        }));
        wanted
    }

    /// The sessions whose transcript can be followed: on this computer, an
    /// agent Leon can read, in front of its shell, its own session id known.
    fn den_wanted(&self) -> Vec<Wanted> {
        self.live
            .all()
            .iter()
            .filter(|session| session.machine.is_local() && !session.is_paused())
            .filter_map(|session| {
                let agent = session.shown_agent()?;
                Format::of(agent)?;
                let id = session
                    .learned
                    .as_ref()
                    .map(|(id, _)| id.clone())
                    .or_else(|| session.resumed.clone())?;
                let subagents = self
                    .den
                    .readings
                    .get(&session.id.0)
                    .filter(|(read, _)| *read == id)
                    .map(|(_, reading)| {
                        reading
                            .pulse
                            .subagents()
                            .iter()
                            .map(|sub| (sub.tool.clone(), sub.agent.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(Wanted {
                    live: session.id.0,
                    agent,
                    cwd: session.cwd.clone(),
                    session: id,
                    subagents,
                })
            })
            .collect()
    }

    /// Starts the loop that reads the transcripts, if the Den is open, a
    /// session is live and it does not run already.
    pub(super) fn den_watch(&mut self, cx: &mut Context<Self>) {
        if !self.den_open()
            || self.den.watch.is_some()
            || (self.live.all().is_empty() && self.den_away(cx).is_empty())
        {
            return;
        }
        self.den
            .followers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set_roots(self.engine.prefs().roots);
        let followers = self.den.followers.clone();
        self.den.watch = Some(cx.spawn(async move |this, cx| loop {
            let Ok(Some(wanted)) = this.read_with(cx, |this, cx| {
                let wanted = this.den_wanted_all(cx);
                (this.den_open() && !(this.live.all().is_empty() && wanted.is_empty()))
                    .then_some(wanted)
            }) else {
                // Closed, or nobody left: the loop ends and with it the task.
                // Whoever was last in the den is seen to leave.
                this.update(cx, |this, cx| {
                    this.den.watch = None;
                    if this.den_open() {
                        this.den_refresh(true, cx);
                    }
                })
                .ok();
                return;
            };
            let reading = followers.clone();
            let reports = cx
                .background_spawn(async move {
                    reading
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .poll(&wanted)
                })
                .await;
            let Ok(tick) = this.update(cx, |this, cx| {
                // The pictures first, then the lions, then what is told of
                // them: a little one is in the den before its words are.
                let told = this.den_apply(reports, cx);
                this.den_refresh(true, cx);
                this.den_tell(told, cx);
                this.options.den_tick
            }) else {
                return;
            };
            cx.background_executor().timer(tick).await;
        }));
    }

    /// Takes what the transcripts grew by: the pictures move on, and it
    /// answers what the feed is to be told of it, in order. What an agent
    /// did is a line of the narrator and what it wrote to the user is its
    /// own words; what is read for the first time, or was written while the
    /// Den was closed, is the past, summed up.
    fn den_apply(&mut self, reports: Vec<Report>, cx: &gpui_kit::App) -> Vec<Told> {
        // A session that is gone, or that moved to another transcript.
        let away = self.den_away(cx);
        let mut live: Vec<u64> = self.live.all().iter().map(|session| session.id.0).collect();
        live.extend(away.iter().map(|away| away.id));
        self.den.readings.retain(|id, _| live.contains(id));
        self.den.written.retain(|id, _| live.contains(id));
        let gap = std::mem::take(&mut self.den.catching_up);
        let mut told = Vec::new();
        for report in reports {
            let name = match away.iter().find(|away| away.id == report.live) {
                Some(away) => away.name.clone(),
                None => self
                    .live
                    .get(LiveId(report.live))
                    .map(|session| session.label())
                    .unwrap_or_default(),
            };
            if let Some(written) = report.written {
                self.den.written.insert(report.live, written);
            }
            let entry = self
                .den
                .readings
                .entry(report.live)
                .or_insert_with(|| (report.session.clone(), Reading::default()));
            let moved = entry.0 != report.session;
            if moved || report.restarted {
                *entry = (report.session.clone(), Reading::default());
            }
            let reading = &mut entry.1;
            if gap || moved || report.restarted {
                den::tell(report.live, &name, &mut reading.pulse, &report.beats);
                told.push(Told::Past(
                    report.live,
                    name.clone(),
                    den::past(&report.beats),
                ));
            } else {
                for beat in &report.beats {
                    let one = std::slice::from_ref(beat);
                    for happening in den::tell(report.live, &name, &mut reading.pulse, one) {
                        told.push(Told::Happened(happening));
                    }
                    if let Beat::Said { text, at } = beat {
                        told.push(Told::Said(report.live, name.clone(), text.clone(), *at));
                    }
                }
            }
            for sub in report.subs {
                reading.pulse.name_subagent(&sub.tool, &sub.agent);
                let own = reading.subs.entry(sub.tool.clone()).or_default();
                if sub.restarted {
                    *own = leon_history::live::Pulse::new();
                }
                for beat in &sub.beats {
                    own.apply(beat);
                    reading.pulse.apply_to_subagent(&sub.tool, beat);
                }
                // A little one speaks under the name of its kind.
                let little = den::little_id(report.live, &sub.tool);
                let called = reading
                    .pulse
                    .subagents()
                    .iter()
                    .find(|known| known.tool == sub.tool)
                    .map(|known| den::little_name(&known.label))
                    .unwrap_or_else(|| den::little_name(""));
                if gap || moved || report.restarted || sub.restarted {
                    told.push(Told::Past(little, called, den::past(&sub.beats)));
                } else {
                    for beat in &sub.beats {
                        if let Beat::Said { text, at } = beat {
                            told.push(Told::Said(little, called.clone(), text.clone(), *at));
                        }
                    }
                }
            }
            let alive: Vec<&str> = reading
                .pulse
                .subagents()
                .iter()
                .map(|sub| sub.tool.as_str())
                .collect();
            reading
                .subs
                .retain(|tool, _| alive.contains(&tool.as_str()));
        }
        told
    }

    /// Puts in the feed what [`Self::den_apply`] answered.
    fn den_tell(&mut self, told: Vec<Told>, cx: &mut Context<Self>) {
        let Some(view) = self.den.view.clone() else {
            return;
        };
        view.update(cx, |den, cx| {
            for told in &told {
                match told {
                    Told::Happened(happening) => den.happen(happening, cx),
                    Told::Said(cub, name, text, at) => den.speak(*cub, name, text, *at, cx),
                    Told::Past(cub, name, past) => den.remember(*cub, name, past, cx),
                }
            }
        });
    }

    /// Tells the view who is in the den now. With `narrate`, what changed
    /// since the last time is told to the narrator.
    pub(super) fn den_refresh(&mut self, narrate: bool, cx: &mut Context<Self>) {
        let Some(view) = self.den.view.clone() else {
            return;
        };
        let facts = self.den_facts(cx);
        let wanted: Vec<(u64, String)> = self
            .den_wanted()
            .into_iter()
            .map(|wanted| (wanted.live, wanted.session))
            .collect();
        let mut cubs = Vec::new();
        for facts in &facts {
            // A reading of another session than the one the terminal holds
            // now says nothing of it.
            let reading = self
                .den
                .readings
                .get(&facts.id)
                .filter(|(session, _)| wanted.contains(&(facts.id, session.clone())))
                .map(|(_, reading)| reading);
            cubs.extend(den::cubs(facts, reading));
        }
        // The sessions that run elsewhere, once their transcript was found.
        let now = self.now().timestamp();
        let colours = crate::theme::palette(cx);
        for away in self.den_away(cx) {
            let Some((_, reading)) = self
                .den
                .readings
                .get(&away.id)
                .filter(|(session, _)| *session == away.external)
            else {
                continue;
            };
            let quiet_for = self
                .den
                .written
                .get(&away.id)
                .map(|written| std::time::Duration::from_secs((now - written).max(0) as u64));
            cubs.extend(den::away_cubs(
                &den::Away {
                    id: away.id,
                    name: away.name.clone(),
                    tint: colours.agent(away.agent),
                    place: match away.found.holder {
                        crate::elsewhere::Holder::OtherLeon(_) => den::Place::OtherLeon,
                        crate::elsewhere::Holder::Terminal => den::Place::Terminal(
                            away.found.app.as_ref().map(|app| app.name.clone()),
                        ),
                    },
                    pid: away.found.pid,
                    quiet_for,
                },
                reading,
            ));
        }
        let told = if narrate {
            den::changes(
                &self.den.cubs,
                &cubs,
                |id| facts.iter().find(|facts| facts.id == id)?.exit,
                |id| self.den.readings.contains_key(&id),
            )
        } else {
            Vec::new()
        };
        let style = self.den_style(cx);
        let reduced = settings::reduce_motion(cx);
        let narrator = settings::flag(cx, "den_narrator");
        // The time of what is told from now on, and what "5m" counts from.
        let wall = self.now().timestamp();
        view.update(cx, |den, cx| {
            den.set_wall_time(wall, cx);
            den.set_style(style, cx);
            den.set_reduced_motion(Some(reduced), cx);
            den.set_narrator(narrator, cx);
            den.set_cubs(&cubs, cx);
            for happening in &told {
                den.happen(happening, cx);
            }
        });
        self.den.cubs = cubs;
    }

    /// The heading of the main pane while the Den is open.
    pub(super) fn den_heading(&self) -> (&'static str, String, String) {
        let lions = self.den.cubs.iter().filter(|cub| cub.parent.is_none());
        let working = lions.clone().filter(|cub| cub.state.is_working()).count();
        (
            "Den",
            "The Den".to_owned(),
            format!("{} LIVE \u{b7} {working} WORKING", lions.count()),
        )
    }

    /// The Den, in the place of the main pane's body.
    pub(super) fn render_den(&self, colours: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.den_editing(cx);
        div()
            .id("den")
            .debug_selector(|| "den".into())
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_den_bar(colours, cx))
            .child(div().flex_1().min_h_0().children(self.den.view.clone()))
            .when(editing, |den| den.child(self.render_den_strip(colours, cx)))
            .into_any_element()
    }
}

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
//! # What is done to a lion
//!
//! While the Den has the keyboard, the selected lion's session is what the
//! commands of a session act on ([`Shell::den_lion`]): Rename, Sleep, Close
//! and Pin do to it what they do from its row of the sidebar, to the whole
//! session and never to one pane of it. A little one stands for its
//! parent's session. The right button on a lion, `M`, `Shift+F10` or the
//! menu key open its menu: Open, Message, Rename, Pin, Send home, Close.
//!
//! **Send home** puts the session to sleep, always after asking: its agent
//! is stopped and it stays in the sidebar to be woken. **Message** (`I`)
//! types a prompt into the session's terminal: at once when its agent waits
//! at its prompt, else it is kept and typed when it next does
//! ([`super::den_post`] says when that is, and why never into a question).
//! A session that runs elsewhere has no terminal here: it is only opened.
//!
//! The truth card says what a lion asks, in full, and ends with a summary
//! of the session in two groups, made of facts alone
//! ([`super::den::notes`]). Leon types no answer to a question or a
//! permission prompt: Enter opens the terminal, where it is answered.
//!
//! # For somebody with many agents
//!
//! * **Who needs me.** `N` selects the next lion that asks for a
//!   permission, waits, or fainted, the most pressing first; the roster
//!   lists those first and counts them.
//! * **Interrupt** (`X`) sends the agent its interrupt key, where
//!   [`leon_core::agent::AgentSpec::interrupt`] knows it, and only in the
//!   middle of a turn.
//! * **Messages.** `Q` lists the ones that wait for the selected lion, to
//!   take any back. `P` sends one message to several lions, after saying
//!   who gets it now, for whom it waits and who is left out.
//! * **Hatch and wake.** `A` starts a new agent session and stays in the
//!   Den, where it joins as an egg. The sessions that were sent home are
//!   listed under the roster; a click, or `W`, wakes one.
//! * **Go to a lion** (`/`) asks which, by name, state or folder.
//! * `?` shows every key of the Den ([`DEN_KEYS`], the table the keys are
//!   read from).
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
use leon_den::{Cub, CubState, DenEvent, DenPalette, DenStyle, DenView, HomeEntry, Note, Tokens};
use leon_history::live::{Beat, Format};

use super::den::{self, Facts, Reading};
use super::den_follow::{Followers, Report, Wanted};
use super::den_post::{self, Due, Outbox, Posted, Ready};
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
    /// Which picture the view last said it shows: logged when it changes,
    /// so that a fall back to the pixel art is never silent.
    drawn: Option<leon_den::Drawn>,
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
    /// The lion selected in the view, as the view last said.
    pub(super) selected: Option<u64>,
    /// The messages that wait for a session to be at its prompt.
    pub(super) post: Outbox,
    /// The sessions that were sent home, as the roster last listed them:
    /// the row's id, and what wakes the session.
    home: Vec<(u64, Home)>,
    /// What the strip under the room shows while it is edited.
    pub(super) tab: super::den_edit::Tab,
    /// The pictures of the strip, made once each: the image and its size in
    /// device pixels.
    pub(super) thumbs: std::cell::RefCell<HashMap<String, super::den_edit::Thumb>>,
    /// Until when the strip may go on making pictures in 2.5D in the
    /// render under way, and whether one was put off to the next.
    pub(super) thumbs_until: std::cell::Cell<Option<std::time::Instant>>,
    pub(super) thumbs_owed: std::cell::Cell<bool>,
    /// The made-up lions of a development run (`LEON_DEN_CAST`): how many
    /// times they have moved on.
    pub(super) cast_round: usize,
    pub(super) cast_loop: Option<gpui_kit::Task<()>>,
}

impl DenUi {
    /// Whether no loop reads transcripts now.
    #[cfg(test)]
    #[cfg_attr(not(leon_posix_tests), allow(dead_code))] // used by the Unix-only tests
    pub(super) fn watch_is_idle(&self) -> bool {
        self.watch.is_none()
    }
}

/// A session that was sent home and can be woken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Home {
    /// It sleeps as a session of the history: resuming it wakes it.
    Slept(leon_core::SessionId),
    /// It sleeps without a history row of its own.
    Dormant(LiveId),
}

impl Home {
    /// What the questions of the palette know it by.
    fn key(&self) -> String {
        match self {
            Home::Slept(id) => format!("s:{id}"),
            Home::Dormant(id) => format!("d:{}", id.0),
        }
    }
}

/// What a key of the Den does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum DenKey {
    /// Lets go a step at a time: the keys, the feed's entry, the selected
    /// lion, then the Den.
    Back,
    /// The next lion of the roster.
    Next,
    /// The previous one.
    Previous,
    /// Opens the selected lion's terminal, or the feed's entry.
    Open,
    /// The next lion that needs the user.
    Needy,
    /// A message to the selected lion.
    Message,
    /// A message to several lions.
    Pride,
    /// The messages that wait for the selected lion.
    Queued,
    /// The selected lion's agent is interrupted.
    Interrupt,
    /// Renames the selected lion's session.
    Rename,
    /// Sends the selected lion home.
    SendHome,
    /// Wakes a session that is at home.
    Wake,
    /// Starts a new agent session.
    Hatch,
    /// Asks which lion and selects it.
    GoTo,
    /// The selected lion's menu.
    Menu,
    /// The feed, a page up or down.
    FeedPage(bool),
    /// The feed, to its start or its end.
    FeedEnd(bool),
    /// The feed's entries, one back or on.
    FeedStep(bool),
    /// The editor of the room.
    Edit,
    /// Every key, over the room.
    Keys,
}

/// A row of the Den's keys: the strokes (a key and whether Shift is held),
/// how the row is written, what it does, and the action.
pub(super) struct DenKeyRow {
    strokes: &'static [(&'static str, bool)],
    /// The keys as the overlay writes them.
    pub label: &'static str,
    /// What they do, in plain words.
    pub what: &'static str,
    pub key: DenKey,
}

/// The keys of the Den. [`Shell::den_key`] reads a keystroke from this
/// table and the overlay (`?`) lists it, so the two cannot differ.
/// What is said when 2.5D is on and the Den shows the pixel art.
pub(super) fn den_fallback_line(why: &str) -> String {
    format!("2.5D is on but cannot be drawn here: {why}. The Den shows the pixel art.")
}

/// Why a view that shows this picture is not in 2.5D although it was asked
/// to be, in a line for the user. Pixel art that was asked for, and a
/// renderer that has not been needed yet, are nothing to say.
pub(super) fn den_fallback_of(drawn: &leon_den::Drawn) -> Option<String> {
    match drawn {
        leon_den::Drawn::Failed(why) => Some(den_fallback_line(why)),
        leon_den::Drawn::Pixels | leon_den::Drawn::Waiting | leon_den::Drawn::Iso(_) => None,
    }
}

pub(super) const DEN_KEYS: &[DenKeyRow] = &[
    DenKeyRow {
        strokes: &[("tab", false), ("down", false), ("right", false)],
        label: "Tab, Down, Right",
        what: "the next lion",
        key: DenKey::Next,
    },
    DenKeyRow {
        strokes: &[("tab", true), ("up", false), ("left", false)],
        label: "Shift+Tab, Up, Left",
        what: "the previous lion",
        key: DenKey::Previous,
    },
    DenKeyRow {
        strokes: &[("n", false)],
        label: "N",
        what: "the next lion that needs you",
        key: DenKey::Needy,
    },
    DenKeyRow {
        strokes: &[("/", false), ("g", false)],
        label: "/ or G",
        what: "go to a lion by name, state or folder",
        key: DenKey::GoTo,
    },
    DenKeyRow {
        strokes: &[("enter", false)],
        label: "Enter",
        what: "open its terminal: answer it there",
        key: DenKey::Open,
    },
    DenKeyRow {
        strokes: &[("i", false)],
        label: "I",
        what: "message it: typed at its prompt, or queued",
        key: DenKey::Message,
    },
    DenKeyRow {
        strokes: &[("q", false)],
        label: "Q",
        what: "its queued messages, to take one back",
        key: DenKey::Queued,
    },
    DenKeyRow {
        strokes: &[("p", false)],
        label: "P",
        what: "message several lions of the pride",
        key: DenKey::Pride,
    },
    DenKeyRow {
        strokes: &[("x", false)],
        label: "X",
        what: "interrupt its agent in the middle of a turn",
        key: DenKey::Interrupt,
    },
    DenKeyRow {
        strokes: &[("r", false)],
        label: "R",
        what: "rename its session",
        key: DenKey::Rename,
    },
    DenKeyRow {
        strokes: &[("h", false)],
        label: "H",
        what: "send it home: asleep, to be woken later",
        key: DenKey::SendHome,
    },
    DenKeyRow {
        strokes: &[("w", false)],
        label: "W",
        what: "wake a session that is at home",
        key: DenKey::Wake,
    },
    DenKeyRow {
        strokes: &[("a", false)],
        label: "A",
        what: "hatch a lion: a new agent session",
        key: DenKey::Hatch,
    },
    DenKeyRow {
        strokes: &[("m", false)],
        label: "M",
        what: "its menu",
        key: DenKey::Menu,
    },
    DenKeyRow {
        strokes: &[("pageup", false)],
        label: "Page Up",
        what: "the feed, a page back",
        key: DenKey::FeedPage(true),
    },
    DenKeyRow {
        strokes: &[("pagedown", false)],
        label: "Page Down",
        what: "the feed, a page on",
        key: DenKey::FeedPage(false),
    },
    DenKeyRow {
        strokes: &[("home", false)],
        label: "Home",
        what: "the start of the feed",
        key: DenKey::FeedEnd(true),
    },
    DenKeyRow {
        strokes: &[("end", false)],
        label: "End",
        what: "the end of the feed",
        key: DenKey::FeedEnd(false),
    },
    DenKeyRow {
        strokes: &[("up", true)],
        label: "Shift+Up",
        what: "the feed's entry before",
        key: DenKey::FeedStep(true),
    },
    DenKeyRow {
        strokes: &[("down", true)],
        label: "Shift+Down",
        what: "the feed's entry after",
        key: DenKey::FeedStep(false),
    },
    DenKeyRow {
        strokes: &[("e", false)],
        label: "E",
        what: "edit the room",
        key: DenKey::Edit,
    },
    DenKeyRow {
        strokes: &[("?", false), ("?", true), ("/", true)],
        label: "?",
        what: "these keys",
        key: DenKey::Keys,
    },
    DenKeyRow {
        strokes: &[("escape", false)],
        label: "Escape",
        what: "let go of the entry, the lion, then the Den",
        key: DenKey::Back,
    },
];

/// What a key does in the Den, with or without Shift.
pub(super) fn den_key_of(key: &str, shift: bool) -> Option<DenKey> {
    DEN_KEYS
        .iter()
        .find(|row| row.strokes.contains(&(key, shift)))
        .map(|row| row.key)
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
    DenPalette::from_tokens(&den_tokens(colours))
}

/// The tokens of a theme, as the Den takes them: its chrome and its room in
/// 2.5D are both made of these.
pub fn den_tokens(colours: &Palette) -> Tokens {
    Tokens {
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
    }
}

impl Shell {
    /// Whether the Den is what the main pane shows.
    pub(super) fn den_open(&self) -> bool {
        matches!(self.main, Main::Den)
    }

    fn den_style(&self, cx: &gpui_kit::App) -> DenStyle {
        let colours = crate::theme::palette(cx);
        DenStyle {
            palette: den_palette(&colours),
            room: leon_den::iso::Theme::from_tokens(&den_tokens(&colours)),
            font_family: fonts::mono().into(),
            font_size: metrics::TEXT_SMALL(),
        }
    }

    /// Opens the Den, if it is not open: what `--den` asks at the start.
    /// With made-up lions asked for (`LEON_DEN_CAST`, in a development
    /// build), they move on every few seconds.
    pub fn show_den(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.den_open() {
            self.toggle_den(window, cx);
        }
        if self.options.den_cast > 0 && self.den.cast_loop.is_none() {
            self.den.cast_loop = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(6))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    this.den.cast_round += 1;
                    if this.den_open() {
                        this.den_refresh(true, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }));
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
                    DenEvent::Selected(id) => {
                        this.den.selected = *id;
                        cx.notify();
                    }
                    DenEvent::Menu { id, at } => this.den_menu(*id, *at, window, cx),
                    DenEvent::Wake(id) => {
                        let home = this
                            .den
                            .home
                            .iter()
                            .find(|(row, _)| row == id)
                            .map(|(_, home)| home.clone());
                        if let Some(home) = home {
                            this.den_wake(&home.key(), window, cx);
                        }
                    }
                    DenEvent::Clicked(_) => {}
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
        self.den.selected = None;
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
        self.den_stop_watching();
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
        self.den_stop_watching();
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
                    self.den_stop_watching();
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
        self.den_stop_watching();
        self.den.back = None;
        self.home.nav = None;
        self.open_live(live, window, cx);
    }

    /// The transcripts stop being read as the Den is left, unless a message
    /// still waits for a session: then they are read until it was typed.
    fn den_stop_watching(&mut self) {
        if !self.den.post.busy() {
            self.den.watch = None;
        }
    }

    // ----- what is done to a lion --------------------------------------------

    /// The lion the Den's keyboard is on: the selected one, or its parent
    /// for a little one. Only while the Den has the keyboard.
    pub(super) fn den_lion(&self) -> Option<u64> {
        if !self.den_open() || self.pane != Pane::Main {
            return None;
        }
        let selected = self.den.selected?;
        let cub = self.den.cubs.iter().find(|cub| cub.id == selected)?;
        Some(cub.parent.unwrap_or(cub.id))
    }

    /// The terminal of the lion the Den's keyboard is on. `None` for a
    /// session that runs elsewhere: it has none here.
    pub(super) fn den_lion_live(&self) -> Option<LiveId> {
        let id = self.den_lion()?;
        (!den::is_away(id))
            .then_some(LiveId(id))
            .filter(|live| self.live.get(*live).is_some())
    }

    /// The transcript of a live session as far as it was read, when it is
    /// the one its terminal holds now.
    fn den_reading(&self, id: u64) -> Option<&Reading> {
        let (session, reading) = self.den.readings.get(&id)?;
        self.den_wanted()
            .iter()
            .any(|wanted| wanted.live == id && &wanted.session == session)
            .then_some(reading)
    }

    /// Whether a message may be typed into a live session now.
    /// How many prompts the transcript of a session held when last read.
    fn den_prompts(&self, id: u64) -> u32 {
        self.den_reading(id).map_or(0, |reading| reading.prompts)
    }

    fn den_ready(&self, id: u64, cx: &gpui_kit::App) -> Ready {
        let facts = self.den_facts(cx);
        match facts.iter().find(|facts| facts.id == id) {
            Some(facts) => {
                den_post::ready(facts, self.den_reading(id).map(|reading| &reading.pulse))
            }
            None => Ready::Never(den_post::Never::Ended),
        }
    }

    /// The selected lion, as the questions of the palette need to know it.
    pub(super) fn den_lion_info(&self, cx: &gpui_kit::App) -> Option<super::steps::LionInfo> {
        let id = self.den_lion()?;
        if den::is_away(id) {
            let away = self.den_away(cx).into_iter().find(|away| away.id == id)?;
            let name = leon_den::narrator::shout(&away.name);
            let why = format!(
                "{name} runs elsewhere ({}): Leon has no terminal of it here, so it can only be opened.",
                away.found.describe()
            );
            return Some(super::steps::LionInfo {
                name,
                live: None,
                elsewhere: Some(why),
                no_message: None,
                queued: Vec::new(),
                cwd: None,
            });
        }
        let live = self.den_lion_live()?;
        let name = leon_den::narrator::shout(&self.live.get(live)?.label());
        let no_message = match self.den_ready(id, cx) {
            Ready::Never(why) => Some(why.why(&name)),
            Ready::Now | Ready::Later => None,
        };
        Some(super::steps::LionInfo {
            name,
            live: Some(live),
            elsewhere: None,
            no_message,
            queued: self.den.post.list(id),
            cwd: self.live.get(live).map(|session| session.cwd.clone()),
        })
    }

    /// The Den and its lions, as the questions of the palette need to know
    /// them: in the order of the roster, the little ones left out.
    pub(super) fn den_pride_info(&self, cx: &gpui_kit::App) -> super::steps::PrideInfo {
        use super::steps::{Post, PrideLion};
        let Some(view) = self.den.view.as_ref().filter(|_| self.den_open()) else {
            return super::steps::PrideInfo {
                home: self
                    .den_home()
                    .into_iter()
                    .map(|(home, name, _)| (home.key(), name))
                    .collect(),
                ..Default::default()
            };
        };
        let roster = view.read(cx).read(|den| den.roster());
        let away = self.den_away(cx);
        let folder = |cwd: &str| {
            cwd.trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(cwd)
                .to_owned()
        };
        let lions = roster
            .iter()
            .filter(|entry| !entry.little)
            .map(|entry| {
                let name = leon_den::narrator::shout(&entry.name);
                let (place, post) = match self.live.get(LiveId(entry.id)) {
                    Some(session) if !den::is_away(entry.id) => {
                        let post = match self.den_ready(entry.id, cx) {
                            Ready::Now => Post::Now,
                            Ready::Later => Post::Later,
                            Ready::Never(why) => Post::Never(why.short().to_owned()),
                        };
                        (self.den_place(&session.machine, &session.cwd).0, Some(post))
                    }
                    _ => (
                        away.iter()
                            .find(|one| one.id == entry.id)
                            .map_or_else(String::new, |one| folder(&one.cwd)),
                        None,
                    ),
                };
                PrideLion {
                    id: entry.id,
                    name,
                    state: entry.state.label().to_owned(),
                    place,
                    needs: entry.needs,
                    post,
                }
            })
            .collect();
        super::steps::PrideInfo {
            open: true,
            lions,
            home: self
                .den_home()
                .into_iter()
                .map(|(home, name, _)| (home.key(), name))
                .collect(),
        }
    }

    /// The project a folder of a machine belongs to and the branch checked
    /// out there, when it is in a worktree Leon knows; else the folder's
    /// own name and no branch.
    fn den_place(&self, machine: &leon_core::MachineId, cwd: &str) -> (String, Option<String>) {
        let within = |root: &str| {
            let root = root.trim_end_matches('/');
            !root.is_empty() && (cwd == root || cwd.starts_with(&format!("{root}/")))
        };
        let found = self
            .snapshot
            .projects
            .iter()
            .filter(|entry| entry.project.machine_id == *machine)
            .flat_map(|entry| {
                entry
                    .worktrees
                    .iter()
                    .map(move |worktree| (entry, worktree))
            })
            .filter(|(_, worktree)| within(&worktree.path))
            .max_by_key(|(_, worktree)| worktree.path.len());
        match found {
            Some((entry, worktree)) => (entry.project.name.clone(), worktree.branch.clone()),
            None => (
                cwd.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(cwd)
                    .to_owned(),
                None,
            ),
        }
    }

    /// The agent sessions that were sent home and can be woken: what wakes
    /// each, its name and its agent. A plain shell that sleeps is none.
    pub(super) fn den_home(&self) -> Vec<(Home, String, Option<leon_core::AgentId>)> {
        let mut home: Vec<(Home, String, Option<leon_core::AgentId>)> = self
            .snapshot
            .sessions
            .iter()
            .filter(|stored| self.slept.contains(&stored.id))
            // One that runs again is in the room, not at home.
            .filter(|stored| self.live.of_history(&stored.id).is_none())
            .map(|stored| {
                (
                    Home::Slept(stored.id.clone()),
                    stored.title.trim().to_owned(),
                    Some(stored.agent),
                )
            })
            .collect();
        home.extend(
            self.dormant
                .all()
                .iter()
                .filter_map(|sleeping| Some((sleeping, sleeping.agent()?)))
                .map(|(sleeping, agent)| {
                    let name = sleeping
                        .label
                        .clone()
                        .filter(|label| !label.trim().is_empty())
                        .unwrap_or_else(|| agent.name().to_owned());
                    (Home::Dormant(sleeping.id()), name, Some(agent))
                }),
        );
        home
    }

    /// Wakes a session that was sent home, by the key the palette knows it
    /// by, and comes back to the Den when its terminal opened at once.
    pub(super) fn den_wake(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((home, name, _)) = self
            .den_home()
            .into_iter()
            .find(|(home, ..)| home.key() == key)
        else {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "That session is no longer at home.",
            );
            return;
        };
        let was_open = self.den_open();
        match home {
            Home::Slept(id) => {
                let Some(stored) = self
                    .snapshot
                    .sessions
                    .iter()
                    .find(|stored| stored.id == id)
                    .cloned()
                else {
                    return;
                };
                self.resume_session(stored, window, cx);
            }
            Home::Dormant(id) => self.wake_dormant(id, window, cx),
        }
        self.engine.report(
            crate::engine::StatusKind::Info,
            format!(
                "Woke {}: its agent starts again in its terminal.",
                leon_den::narrator::shout(&name)
            ),
        );
        // Its terminal took the main pane: back to the Den, where it was
        // woken from. (When Leon first looks for another process that holds
        // the session, the terminal opens a moment later and stays in front.)
        if was_open && !self.den_open() && matches!(self.main, Main::Live(_)) {
            self.toggle_den(window, cx);
        }
        cx.notify();
    }

    /// Selects a lion of the Den, opening the Den when it is closed.
    pub(super) fn den_select(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if !self.den_open() {
            self.toggle_den(window, cx);
        }
        if let Some(view) = self.den.view.clone() {
            view.update(cx, |den, cx| den.select(Some(id), cx));
        }
        cx.notify();
    }

    /// Selects the next lion that needs the user, opening the Den when it
    /// is closed.
    pub(super) fn den_next_needy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.den_open() {
            self.toggle_den(window, cx);
        }
        let found = self
            .den
            .view
            .clone()
            .is_some_and(|view| view.update(cx, |den, cx| den.select_needy(cx)));
        if !found {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "No lion needs you: none waits, asks for a permission or fainted.",
            );
        }
        cx.notify();
    }

    /// Why the Den shows the pixel art although 2.5D is asked for, in a
    /// line for the user; `None` when it shows what was asked.
    pub(super) fn den_fallback(&self, cx: &gpui_kit::App) -> Option<String> {
        den_fallback_of(&self.den.view.as_ref()?.read(cx).drawn())
    }

    /// Shows the keys of the Den over the room, or takes them away.
    pub(super) fn den_toggle_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.den_open() {
            self.toggle_den(window, cx);
        }
        let fallback = self.den_fallback(cx);
        if let Some(view) = self.den.view.clone() {
            view.update(cx, |den, cx| {
                let keys = (!den.keys_shown()).then(|| {
                    DEN_KEYS
                        .iter()
                        .map(|row| (row.label.to_owned(), row.what.to_owned()))
                        // In the Den `?` and `/` are the Den's: the sheet of
                        // every shortcut and the sidebar's filter are still
                        // a command away.
                        .chain([(
                            "The palette".to_owned(),
                            "Keyboard shortcuts: all of Leon's keys".to_owned(),
                        )])
                        // And why the room is the pixel art, when 2.5D was
                        // asked for and cannot be drawn.
                        .chain(fallback.map(|why| ("Pixel art".to_owned(), why)))
                        .collect()
                });
                den.show_keys(keys, cx);
            });
        }
        cx.notify();
    }

    /// Whether the agent of a live session can be sent its interrupt key
    /// now: the key, or why not.
    fn den_can_interrupt(&self, id: LiveId, cx: &gpui_kit::App) -> Result<String, String> {
        let Some(session) = self.live.get(id) else {
            return Err("That session has ended: there is nothing to interrupt.".to_owned());
        };
        let name = leon_den::narrator::shout(&session.label());
        if session.signals(cx).exited.is_some() {
            return Err(format!("{name} has ended: there is nothing to interrupt."));
        }
        if session.is_paused() {
            return Err(format!("{name} is paused: its agent is not running."));
        }
        let Some(agent) = session.shown_agent() else {
            return Err(format!("{name} has no agent in front of its shell."));
        };
        let Some(key) = agent.spec().and_then(|spec| spec.interrupt.clone()) else {
            return Err(format!(
                "Leon does not know the key that interrupts {}: open {name} and stop it there.",
                agent.name()
            ));
        };
        // Only in the middle of a turn: at its prompt the key would do
        // something else.
        let state = self
            .den
            .cubs
            .iter()
            .find(|cub| cub.id == id.0)
            .map(|cub| cub.state);
        let mid_turn = match state {
            Some(state) if state.is_working() => true,
            Some(CubState::NeedsPermission) => true,
            Some(CubState::WaitingForUser) => self
                .den_reading(id.0)
                .is_some_and(|reading| !reading.pulse.tools().is_empty()),
            _ => false,
        };
        if !mid_turn {
            return Err(format!(
                "{name} is not in the middle of a turn: there is nothing to interrupt."
            ));
        }
        Ok(key)
    }

    /// Sends the agent of the selected lion its interrupt key.
    pub(super) fn den_interrupt(&mut self, cx: &mut Context<Self>) {
        let Some(info) = self.den_lion_info(cx) else {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "Select a lion in the Den first.",
            );
            return;
        };
        let Some(live) = info.live else {
            self.engine.report(
                crate::engine::StatusKind::Info,
                info.elsewhere
                    .unwrap_or_else(|| format!("{} has no terminal here.", info.name)),
            );
            return;
        };
        match self.den_can_interrupt(live, cx) {
            Ok(key) => {
                if let Some(session) = self.live.get(live) {
                    let agent = session
                        .shown_agent()
                        .map_or("its agent", |agent| agent.name());
                    session
                        .view
                        .read(cx)
                        .terminal()
                        .write(key.as_bytes().to_vec());
                    let named = if key == "\u{1b}" { "Escape" } else { "its key" };
                    self.engine.report(
                        crate::engine::StatusKind::Info,
                        format!(
                            "Sent {named} to {}: that interrupts {agent} in the middle of a turn.",
                            info.name
                        ),
                    );
                }
            }
            Err(why) => self.engine.report(crate::engine::StatusKind::Info, why),
        }
        cx.notify();
    }

    /// Takes back a message that waits for a live session, or all of them.
    pub(super) fn den_cancel_queued(
        &mut self,
        id: LiveId,
        place: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let name = self.live.get(id).map_or_else(
            || "it".to_owned(),
            |s| leon_den::narrator::shout(&s.label()),
        );
        let said = match place {
            Some(place) => match self.den.post.cancel(id.0, place) {
                Some(_) => format!(
                    "Took back a message queued for {name}: it will not be typed ({} still queued).",
                    den_post::count(self.den.post.waiting(id.0))
                ),
                None => format!("That message is no longer queued for {name}."),
            },
            None => match self.den.post.clear(id.0) {
                0 => format!("No message is queued for {name}."),
                taken => format!(
                    "Took back {} queued for {name}: none of them will be typed.",
                    den_post::count(taken)
                ),
            },
        };
        self.engine.report(crate::engine::StatusKind::Info, said);
        self.den_refresh(false, cx);
        cx.notify();
    }

    /// One message for several live sessions: each gets it now or when its
    /// agent next waits, and the status line says who, and who did not.
    pub(super) fn den_message_pride(&mut self, ids: &[LiveId], text: &str, cx: &mut Context<Self>) {
        let Some(text) = den_post::prompt(text) else {
            return;
        };
        let (mut sent, mut queued, mut refused) = (Vec::new(), Vec::new(), Vec::new());
        for id in ids {
            let Some(session) = self.live.get(*id) else {
                continue;
            };
            let name = leon_den::narrator::shout(&session.label());
            let ready = self.den_ready(id.0, cx);
            match self
                .den
                .post
                .post(id.0, &text, ready, self.den_prompts(id.0))
            {
                Posted::Send(text) => {
                    self.den_type(*id, &text, cx);
                    sent.push(name);
                }
                Posted::Queued(_) => queued.push(name),
                Posted::Refused(why) => refused.push(format!("{name} ({})", why.short())),
            }
        }
        let mut said = Vec::new();
        if !sent.is_empty() {
            said.push(format!("Sent to {}.", sent.join(", ")));
        }
        if !queued.is_empty() {
            said.push(format!(
                "Queued for {} until each waits at its prompt.",
                queued.join(", ")
            ));
        }
        if !refused.is_empty() {
            said.push(format!("Not sent to {}.", refused.join("; ")));
        }
        if said.is_empty() {
            said.push("Nobody was there to message.".to_owned());
        }
        self.engine.report(
            if sent.is_empty() && queued.is_empty() {
                crate::engine::StatusKind::Error
            } else {
                crate::engine::StatusKind::Info
            },
            said.join(" "),
        );
        self.den_watch(cx);
        self.den_refresh(false, cx);
        cx.notify();
    }

    /// Starts an agent session from the Den and stays in the Den, the new
    /// lion selected: it joins as an egg.
    pub(super) fn den_hatch(
        &mut self,
        intent: super::steps::SessionIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.live.ids();
        self.start_intent(intent, window, cx);
        let new = self.live.ids().into_iter().find(|id| !before.contains(id));
        if !self.den_open() {
            self.toggle_den(window, cx);
        }
        if let (Some(new), Some(view)) = (new, self.den.view.clone()) {
            view.update(cx, |den, cx| den.select(Some(new.0), cx));
        }
        cx.notify();
    }

    /// Asks the view for the menu of the selected lion (the keyboard's way
    /// in): it answers with [`DenEvent::Menu`].
    pub(super) fn den_menu_here(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = self.den.view.clone() {
            view.update(cx, |den, cx| den.menu_selection(cx));
        }
    }

    /// Opens the menu of a lion at a point of the window.
    fn den_menu(
        &mut self,
        id: u64,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.den_open() || self.den_editing(cx) {
            return;
        }
        // The view selected it before it asked.
        self.den.selected = Some(id);
        self.pane = Pane::Main;
        let Some(session) = self.den_lion() else {
            return;
        };
        let live = self.den_lion_live();
        let pinned = live.and_then(|live| {
            let history = self.history_of_live(live)?;
            let machine = self.live.get(live)?.machine.clone();
            Some(self.pinned_ids(&machine).contains(&history))
        });
        let items = super::menu::items_for_lion(super::menu::LionMenu {
            pinned,
            elsewhere: live.is_none(),
            interrupt: live.is_some_and(|live| {
                self.live
                    .get(live)
                    .and_then(|session| session.shown_agent()?.spec()?.interrupt.clone())
                    .is_some()
            }),
            queued: live.map_or(0, |live| self.den.post.waiting(live.0)),
        });
        if live.is_none() {
            if let Some(why) = self.den_lion_info(cx).and_then(|lion| lion.elsewhere) {
                self.engine.report(crate::engine::StatusKind::Info, why);
            }
        }
        if self.overlay != super::shell::Overlay::None {
            self.close_overlay(window, cx);
        }
        let mut menu =
            super::menu::Menu::new(super::tree::NodeId::Live(LiveId(session)), items, at);
        menu.lion = Some(session);
        self.overlay = super::shell::Overlay::Menu;
        self.menu = Some(menu);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Does what an item of a lion's menu says, for that lion.
    pub(super) fn run_lion_item(
        &mut self,
        lion: u64,
        command: crate::keys::Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The menu took nothing away: the lion is still the selected one,
        // unless it left meanwhile.
        if self.den_lion() != Some(lion) {
            return;
        }
        match command {
            crate::keys::Command::Open => self.den_open_lion(lion, window, cx),
            command => {
                self.run_command(command, window, cx);
            }
        }
    }

    /// Pins or unpins the session of the selected lion.
    pub(super) fn den_pin(&mut self, pinned: bool, cx: &mut Context<Self>) {
        let Some(live) = self.den_lion_live() else {
            if let Some(why) = self.den_lion_info(cx).and_then(|lion| lion.elsewhere) {
                self.engine.report(crate::engine::StatusKind::Info, why);
            }
            return;
        };
        let Some(session) = self.live.get(live) else {
            return;
        };
        let (machine, name) = (session.machine.clone(), session.label());
        match self.history_of_live(live) {
            Some(history) => self.pin_session(history, machine, pinned),
            None => self.engine.report(
                crate::engine::StatusKind::Info,
                format!("{name} is not in the history yet: there is nothing to pin."),
            ),
        }
    }

    /// Sends a lion home: its whole session is put to sleep, as from its
    /// row of the sidebar. The messages that waited for it are dropped.
    pub(super) fn den_send_home(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.live.get(id).map(|session| session.label()) else {
            return;
        };
        let dropped = self.den.post.forget(id.0);
        // The lions as they are now, a new name included: who leaves is
        // told by the name it has.
        self.den_refresh(true, cx);
        self.end_live(id, true, true, window, cx);
        let name = leon_den::narrator::shout(&name);
        let mut said = format!("Sent {name} home: its session is asleep in the sidebar.");
        if dropped > 0 {
            said.push_str(&format!(
                " {} queued for it {} dropped.",
                den_post::count(dropped),
                if dropped == 1 { "was" } else { "were" }
            ));
        }
        self.engine.report(crate::engine::StatusKind::Info, said);
        // Whoever left is seen to leave now, not at the next poll.
        self.den_refresh(true, cx);
        cx.notify();
    }

    /// A message for a live session: typed now when its agent waits at its
    /// prompt, kept for later when it is busy, refused when it can never be.
    pub(super) fn den_message(&mut self, id: LiveId, text: &str, cx: &mut Context<Self>) {
        let (Some(text), Some(session)) = (den_post::prompt(text), self.live.get(id)) else {
            return;
        };
        let name = leon_den::narrator::shout(&session.label());
        let ready = self.den_ready(id.0, cx);
        match self.den.post.post(id.0, &text, ready, self.den_prompts(id.0)) {
            Posted::Send(text) => {
                self.den_type(id, &text, cx);
                self.engine.report(
                    crate::engine::StatusKind::Info,
                    format!("Sent to {name}."),
                );
            }
            Posted::Queued(waiting) if self.den.post.stalled(id.0) => self.engine.report(
                crate::engine::StatusKind::Info,
                format!(
                    "Not sent yet: the last message to {name} may still be in its input. Open {name} and send or clear it ({} queued).",
                    den_post::count(waiting)
                ),
            ),
            Posted::Queued(waiting) => self.engine.report(
                crate::engine::StatusKind::Info,
                format!(
                    "{name} is busy: not sent yet. It is typed when {name} next waits at its prompt ({} queued).",
                    den_post::count(waiting)
                ),
            ),
            Posted::Refused(why) => self
                .engine
                .report(crate::engine::StatusKind::Error, why.why(&name)),
        }
        // Until it was typed the transcripts are read, the Den open or not.
        self.den_watch(cx);
        self.den_refresh(false, cx);
        cx.notify();
    }

    /// Looks at every session a message waits for, or was just typed into:
    /// the next one is typed into a session that is at its prompt, and the
    /// ones of a session that ended, or that can no longer be messaged, are
    /// dropped, which the status line says.
    fn den_post_tick(&mut self, cx: &mut Context<Self>) {
        for id in self.den.post.all() {
            let name = self
                .live
                .get(LiveId(id))
                .map(|session| leon_den::narrator::shout(&session.label()));
            let ready = match &name {
                Some(_) => self.den_ready(id, cx),
                None => Ready::Never(den_post::Never::Ended),
            };
            if let Ready::Never(why) = ready {
                let dropped = self.den.post.forget(id);
                if dropped > 0 {
                    let name = name.unwrap_or_else(|| "A session".to_owned());
                    self.engine.report(
                        crate::engine::StatusKind::Info,
                        format!(
                            "{} {} queued for it {} dropped.",
                            why.why(&name),
                            den_post::count(dropped),
                            if dropped == 1 { "was" } else { "were" }
                        ),
                    );
                }
                continue;
            }
            let Some(name) = name else {
                continue;
            };
            match self.den.post.due(id, ready, self.den_prompts(id)) {
                Due::Nothing => {}
                Due::Type(text) => {
                    self.den_type(LiveId(id), &text, cx);
                    let left = self.den.post.waiting(id);
                    self.engine.report(
                        crate::engine::StatusKind::Info,
                        match left {
                            0 => format!(
                                "{name} waits at its prompt: the queued message was typed."
                            ),
                            left => format!(
                                "{name} waits at its prompt: a queued message was typed ({} still queued).",
                                den_post::count(left)
                            ),
                        },
                    );
                }
                // Its Enter may have been lost: nothing is typed after it.
                Due::Stalled(left) => self.engine.report(
                    crate::engine::StatusKind::Error,
                    match left {
                        0 => format!(
                            "{name} did not start on the message it was sent: it may still be in its input. Open {name} and send or clear it."
                        ),
                        left => format!(
                            "{name} did not start on the message it was sent: it may still be in its input. Open {name} and send or clear it; {} queued {} held until it starts a turn.",
                            den_post::count(left),
                            if left == 1 { "is" } else { "are" }
                        ),
                    },
                ),
            }
        }
    }

    /// Types a message into a session's terminal as a prompt: pasted, so
    /// that its line breaks are not the Enter key, then Enter a moment
    /// later, as a person would. Where the program did not ask for pastes to
    /// be bracketed, the line breaks are typed as spaces.
    fn den_type(&mut self, id: LiveId, text: &str, cx: &mut Context<Self>) {
        let Some(session) = self.live.get(id) else {
            return;
        };
        let terminal = session.view.read(cx).terminal().clone();
        terminal.scroll_to_bottom();
        if terminal.bracketed_paste() {
            terminal.paste(text);
        } else {
            terminal.paste(&text.replace('\n', " "));
        }
        let after = self.options.den_submit_after;
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(after).await;
            terminal.write(b"\r".to_vec());
        })
        .detach();
    }

    /// What the truth card of each lion says under its state: what it asks,
    /// and the summary of its session.
    fn den_notes(&self, cx: &gpui_kit::App) -> HashMap<u64, Vec<Note>> {
        let now = self.now().timestamp_millis();
        let facts = self.den_facts(cx);
        let mut notes = HashMap::new();
        for session in self.live.all() {
            let id = session.id.0;
            let stored = self.history_of_live(session.id).and_then(|history| {
                self.snapshot
                    .sessions
                    .iter()
                    .find(|stored| stored.id == history)
            });
            let (folder, branch) = self.den_place(&session.machine, &session.cwd);
            let about = den::About {
                title: stored.map(|stored| stored.title.clone()),
                running_for: Some(std::time::Duration::from_millis(
                    (now - session.started_ms).max(0) as u64,
                )),
                messages: stored.map(|stored| stored.message_count),
                queued: self.den.post.waiting(id),
                agent: session.shown_agent().map(|agent| agent.name().to_owned()),
                model: stored.and_then(|stored| stored.model.clone()),
                folder: Some(folder),
                branch,
            };
            let reading = self.den_reading(id);
            let state = facts
                .iter()
                .find(|facts| facts.id == id)
                .map(|facts| den::state(facts, reading.map(|reading| &reading.pulse)).0);
            let lines = den::notes(&session.label(), &about, reading, state);
            if !lines.is_empty() {
                notes.insert(id, lines);
            }
        }
        for away in self.den_away(cx) {
            let reading = self
                .den
                .readings
                .get(&away.id)
                .filter(|(session, _)| *session == away.external)
                .map(|(_, reading)| reading);
            let (folder, branch) = self.den_place(&leon_core::MachineId::local(), &away.cwd);
            let about = den::About {
                title: away.stored.as_ref().map(|stored| stored.title.clone()),
                running_for: None,
                messages: away.stored.as_ref().map(|stored| stored.message_count),
                queued: 0,
                agent: Some(away.agent.name().to_owned()),
                model: away.stored.as_ref().and_then(|stored| stored.model.clone()),
                folder: Some(folder),
                branch,
            };
            // What it asks is said as for any lion; no permission prompt is
            // claimed for it, so only its own questions show.
            let state = self
                .den
                .cubs
                .iter()
                .find(|cub| cub.id == away.id)
                .map(|cub| cub.state);
            let lines = den::notes(&away.name, &about, reading, state);
            if !lines.is_empty() {
                notes.insert(away.id, lines);
            }
        }
        notes
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
        // The keys, while they are shown, are over everything: any key takes
        // them away and does nothing else.
        if view.read(cx).keys_shown() {
            view.update(cx, |den, cx| den.show_keys(None, cx));
            cx.notify();
            return true;
        }
        let Some(key) = den_key_of(stroke.key.as_str(), m.shift) else {
            return false;
        };
        use crate::keys::Command;
        // What is done to a lion needs one: said here, so that the key never
        // falls through to the row the sidebar's cursor was left on.
        let needs_lion = matches!(
            key,
            DenKey::Message
                | DenKey::Queued
                | DenKey::Interrupt
                | DenKey::Rename
                | DenKey::SendHome
                | DenKey::Menu
        );
        if needs_lion && self.den_lion().is_none() {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "Select a lion in the Den first.",
            );
            cx.notify();
            return true;
        }
        match key {
            // Escape lets go a step at a time: the entry of the feed the
            // keyboard is on, then the selected lion (the feed is
            // everybody's again), then the Den.
            DenKey::Back => {
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
            DenKey::FeedPage(up) => {
                view.update(cx, |den, cx| den.scroll_feed(if up { -1 } else { 1 }, cx));
            }
            DenKey::FeedEnd(top) => view.update(cx, |den, cx| den.feed_to(!top, cx)),
            DenKey::FeedStep(back) => view.update(cx, |den, cx| den.feed_step(back, cx)),
            DenKey::Previous => view.update(cx, |den, cx| den.select_previous(cx)),
            DenKey::Next => view.update(cx, |den, cx| den.select_next(cx)),
            // On an entry of the feed, Enter opens its message; otherwise it
            // opens the selected lion's session, where a question or a
            // permission prompt is answered.
            DenKey::Open => {
                view.update(cx, |den, cx| {
                    if !den.feed_toggle(cx) {
                        den.open_selection(cx);
                    }
                });
            }
            DenKey::Needy => self.den_next_needy(window, cx),
            DenKey::Message => {
                self.run_command(Command::MessageLion, window, cx);
            }
            DenKey::Pride => {
                self.run_command(Command::MessagePride, window, cx);
            }
            DenKey::Queued => {
                self.run_command(Command::QueuedMessages, window, cx);
            }
            DenKey::Interrupt => self.den_interrupt(cx),
            DenKey::Rename => {
                self.run_command(Command::Rename, window, cx);
            }
            DenKey::SendHome => {
                self.run_command(Command::SendLionHome, window, cx);
            }
            DenKey::Wake => {
                self.run_command(Command::WakeLion, window, cx);
            }
            DenKey::Hatch => {
                self.run_command(Command::HatchLion, window, cx);
            }
            DenKey::GoTo => {
                self.run_command(Command::GoToLion, window, cx);
            }
            DenKey::Menu => self.den_menu_here(cx),
            DenKey::Edit => self.den_tool(super::den_edit::Tool::Edit, cx),
            DenKey::Keys => self.den_toggle_keys(window, cx),
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
        if !(self.den_open() || self.den.post.busy())
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
                ((this.den_open() || this.den.post.busy())
                    && !(this.live.all().is_empty() && wanted.is_empty()))
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
                // What waits for a session to be at its prompt is typed
                // from what was just read of it.
                this.den_post_tick(cx);
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
            reading.note(&report.beats);
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
        let drawn = view.read(cx).drawn();
        if self.den.drawn.as_ref() != Some(&drawn) {
            match &drawn {
                leon_den::Drawn::Failed(why) => {
                    tracing::warn!(%why, "the Den shows the pixel art: 2.5D cannot be drawn");
                    // Said once where it is seen, too: a den that looks as
                    // it always did must not be the only sign.
                    self.engine
                        .report(crate::engine::StatusKind::Info, den_fallback_line(why));
                }
                other => tracing::info!(picture = ?other, "the Den's picture"),
            }
            self.den.drawn = Some(drawn);
        }
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
        // The made-up lions of a development run.
        if self.options.den_cast > 0 {
            cubs.extend(den::made_up(
                self.options.den_cast,
                self.den.cast_round,
                [
                    colours.agent(leon_core::AgentId::CLAUDE),
                    colours.agent(leon_core::AgentId::CODEX),
                    colours.agent(leon_core::AgentId::OPENCODE),
                ],
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
        // The room in 2.5D, where the setting asks for it. Never in a test:
        // a test has no graphics card to count on, and the pixel art is the
        // same picture every time.
        let three_d = !cfg!(test) && settings::flag(cx, "den_3d");
        let notes = self.den_notes(cx);
        // Who was sent home: listed under the roster, to be woken there.
        let home = self.den_home();
        self.den.home = home
            .iter()
            .map(|(home, ..)| (den::home_id(&home.key()), home.clone()))
            .collect();
        let home: Vec<HomeEntry> = home
            .into_iter()
            .map(|(home, name, agent)| HomeEntry {
                id: den::home_id(&home.key()),
                name,
                tint: agent.map_or(colours.text_muted, |agent| colours.agent(agent)),
            })
            .collect();
        // The time of what is told from now on, and what "5m" counts from.
        let wall = self.now().timestamp();
        view.update(cx, |den, cx| {
            den.set_wall_time(wall, cx);
            den.set_style(style, cx);
            den.set_three_d(three_d, cx);
            den.set_reduced_motion(Some(reduced), cx);
            den.set_narrator(narrator, cx);
            den.set_cubs(&cubs, cx);
            den.set_notes(notes, cx);
            den.set_home(home, cx);
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
            // A click anywhere in the Den gives it the keyboard back, as a
            // click in the tree gives it to the tree: its keys are dead
            // while the sidebar has it.
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                if this.pane != Pane::Main && this.overlay == super::shell::Overlay::None {
                    this.pane = Pane::Main;
                    if this.filter_focused(window, cx) {
                        this.focus.focus(window, cx);
                    }
                    cx.notify();
                }
            }))
            .child(self.render_den_bar(colours, cx))
            .child(div().flex_1().min_h_0().children(self.den.view.clone()))
            .when(editing, |den| den.child(self.render_den_strip(colours, cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{self, Command};
    use std::collections::HashSet;

    #[test]
    fn pixel_art_shown_for_want_of_a_gpu_is_said_with_its_reason_and_nothing_else_is() {
        use leon_den::Drawn;
        let said = den_fallback_of(&Drawn::Failed("no adapter: none was found".to_owned()));
        assert_eq!(
            said.as_deref(),
            Some(
                "2.5D is on but cannot be drawn here: no adapter: none was found. The Den shows the pixel art."
            )
        );
        // What was asked for is shown, or will be: nothing to say.
        for drawn in [
            Drawn::Pixels,
            Drawn::Waiting,
            Drawn::Iso("an adapter".to_owned()),
        ] {
            assert_eq!(den_fallback_of(&drawn), None, "{drawn:?}");
        }
    }

    /// The command of the palette that does what a key of the Den does,
    /// where there is one.
    fn command_of(key: DenKey) -> Option<Command> {
        Some(match key {
            DenKey::Needy => Command::NextNeedyLion,
            DenKey::Message => Command::MessageLion,
            DenKey::Pride => Command::MessagePride,
            DenKey::Queued => Command::QueuedMessages,
            DenKey::Interrupt => Command::InterruptLion,
            DenKey::SendHome => Command::SendLionHome,
            DenKey::Wake => Command::WakeLion,
            DenKey::Hatch => Command::HatchLion,
            DenKey::GoTo => Command::GoToLion,
            DenKey::Edit => Command::EditDen,
            DenKey::Keys => Command::DenKeys,
            _ => return None,
        })
    }

    #[test]
    fn every_key_of_the_den_is_in_its_table_once_and_is_written_as_it_is_pressed() {
        let mut strokes = HashSet::new();
        let mut actions = HashSet::new();
        for row in DEN_KEYS {
            assert!(!row.strokes.is_empty(), "{}", row.label);
            assert!(!row.label.is_empty() && !row.what.is_empty());
            assert!(actions.insert(row.key), "{:?} has two rows", row.key);
            for (key, shift) in row.strokes {
                assert!(strokes.insert((*key, *shift)), "{key} is bound twice");
                // The table is what the keyboard is read from.
                assert_eq!(den_key_of(key, *shift), Some(row.key));
            }
            // A row names its first key, as it is pressed.
            let (first, shift) = row.strokes[0];
            let written = row.label.to_lowercase().replace(' ', "");
            assert!(written.contains(first), "{} lacks {first}", row.label);
            assert_eq!(shift, written.starts_with("shift+"), "{}", row.label);
        }
        // What a key does plainly, it does not do with Shift.
        assert_eq!(den_key_of("n", true), None);
        assert_eq!(den_key_of("f5", false), None);
        // The keys the Den had before are still its keys.
        for (key, shift, action) in [
            ("escape", false, DenKey::Back),
            ("enter", false, DenKey::Open),
            ("i", false, DenKey::Message),
            ("e", false, DenKey::Edit),
            ("m", false, DenKey::Menu),
            ("tab", true, DenKey::Previous),
            ("pageup", false, DenKey::FeedPage(true)),
        ] {
            assert_eq!(den_key_of(key, shift), Some(action), "{key}");
        }
    }

    #[test]
    fn the_readme_says_the_key_of_every_command_the_den_has_a_key_for() {
        let readme = include_str!("../../../../README.md");
        let mut seen = 0;
        for row in DEN_KEYS {
            let Some(command) = command_of(row.key) else {
                continue;
            };
            seen += 1;
            let label = keys::label(command);
            let line = readme
                .lines()
                .find(|line| line.starts_with(&format!("| {label} |")))
                .unwrap_or_else(|| panic!("the README has no row for {label:?}"));
            let key = row.strokes[0].0.to_uppercase();
            assert!(
                line.contains(&format!("`{key}` in the Den")),
                "the README does not say {key} for {label:?}: {line}"
            );
        }
        assert_eq!(seen, 11);
    }
}

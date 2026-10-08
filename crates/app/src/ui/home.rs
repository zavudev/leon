//! The home: where the window starts, and a place to act from.
//!
//! The home is what the main pane shows when nothing is open
//! ([`Main::Empty`]). It is reached from anywhere: the pinned "Home" row over
//! the tree of the sidebar, the command "Go home", its chord. Going home
//! closes nothing: the terminals go on running and stay in the sidebar.
//!
//! It holds, top to bottom:
//!
//! * the **actions** that start things, each a button with the chord of its
//!   command ([`ACTIONS`], the Den first);
//! * the **sessions at a glance**: how many run here and elsewhere, how many
//!   work and how many wait, and the ones that wait, each a row that opens
//!   it; then the ones that run elsewhere, which open as they do from the
//!   sidebar;
//! * the **recent sessions** of the history, which open as from the sidebar.
//!
//! The arrows walk all of it in that order and Enter does what a click does.
//! The words are the interface's plain ones: the comic voice is the Den's.
//!
//! What is shown is decided here without a window ([`pride`], [`headline`],
//! [`items`], [`step`]); the shell gathers the facts and draws.

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, Context, Div, FontWeight, SharedString, Window};
use leon_core::SessionId;

use super::activity::Activity;
use super::lines::empty_frame;
use super::live::LiveId;
use super::shell::{Main, Pane, Shell};
use super::tree::NodeId;
use super::widgets::{activity_dot, elsewhere_mark, key_cap, mark, mono, section_label, Lion};
use crate::format;
use crate::icons::{icon, IconName};
use crate::keys::{self, Command};
use crate::theme::{metrics, px, Palette};

/// The actions of the home, in the order they are shown: the Den first.
pub const ACTIONS: [Command; 10] = [
    Command::ShowDen,
    Command::NewSession,
    Command::OpenProject,
    Command::GoTo,
    Command::SearchHistory,
    Command::Commands,
    Command::AddMachine,
    Command::Shortcuts,
    Command::ShowUsage,
    Command::Settings,
];

/// How many sessions that wait are listed.
pub const WAITING_ROWS: usize = 4;
/// How many sessions that run elsewhere are listed.
pub const ELSEWHERE_ROWS: usize = 3;
/// How many recent sessions are listed.
pub const RECENT_ROWS: usize = 5;

/// Where the keyboard is on the home, and whether the sidebar's cursor is
/// on the Home row over the tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HomeUi {
    /// The item of the home the keyboard is on ([`items`]).
    pub at: usize,
    /// The row of the sidebar's navigation the cursor is on, when it is
    /// there rather than on a row of the tree.
    pub nav: Option<Nav>,
}

/// A row of the sidebar's navigation: the places that are always one step
/// away, over the filter and the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nav {
    /// The home.
    Home,
    /// The Den.
    Den,
}

impl Nav {
    /// The rows, top to bottom.
    pub const ALL: [Nav; 2] = [Nav::Home, Nav::Den];

    /// Its name.
    pub fn label(self) -> &'static str {
        match self {
            Nav::Home => "Home",
            Nav::Den => "The Den",
        }
    }

    /// The command that goes there, whose chord its tooltip says.
    pub fn command(self) -> Command {
        match self {
            Nav::Home => Command::GoHome,
            Nav::Den => Command::ShowDen,
        }
    }

    /// The row the cursor goes to from this one: the one above (`up`) or
    /// below, `None` where the navigation ends (below it is the tree).
    pub fn step(self, up: bool) -> Option<Nav> {
        match (self, up) {
            (Nav::Home, true) => Some(Nav::Home),
            (Nav::Home, false) => Some(Nav::Den),
            (Nav::Den, true) => Some(Nav::Home),
            (Nav::Den, false) => None,
        }
    }
}

/// A terminal of this window, as the home counts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Here {
    /// The live session.
    pub id: LiveId,
    /// What the sidebar calls it.
    pub name: String,
    /// What its dot says.
    pub activity: Activity,
}

/// A session that runs in another terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elsewhere {
    /// The session in the history, when it is known: what a click opens.
    pub session: Option<SessionId>,
    /// What it is called.
    pub name: String,
    /// Where it runs, in a few words.
    pub place: String,
    /// The process names the session; otherwise it is the best fit.
    pub certain: bool,
}

/// The sessions at a glance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pride {
    /// Terminals of this window.
    pub here: usize,
    /// Of those, the ones whose program prints.
    pub working: usize,
    /// Of those, the ones that wait for the user without saying why (quiet,
    /// or the bell), or failed.
    pub waiting: usize,
    /// Of those, the agents whose transcript says they need an answer: a
    /// question, or (inferred) a permission.
    pub asking: usize,
    /// Of those, the agents whose transcript says they finished their turn.
    pub finished: usize,
    /// Every session in the list of those that want the user, however many
    /// rows it shows: [`Pride::waiting`], [`Pride::asking`] and
    /// [`Pride::finished`] together.
    pub wants: usize,
    /// Sessions that run in another terminal.
    pub elsewhere: usize,
    /// The first of those that wait.
    pub waiting_rows: Vec<Here>,
    /// The first of those that run elsewhere.
    pub elsewhere_rows: Vec<Elsewhere>,
}

/// Counts the sessions and picks the rows: at most [`WAITING_ROWS`] of
/// those that want the user, the most urgent first (a failure, then an
/// agent that needs an answer, then one that waits, then one that finished),
/// and at most [`ELSEWHERE_ROWS`] of those that run elsewhere, the ones the
/// history knows first.
pub fn pride(here: &[Here], elsewhere: &[Elsewhere]) -> Pride {
    let wants = |one: &&Here| one.activity.wants_you() || one.activity == Activity::Failed;
    let mut waiting: Vec<Here> = here.iter().filter(wants).cloned().collect();
    // Stable: the sessions of one state keep the order they were started in.
    waiting.sort_by_key(|one| std::cmp::Reverse(one.activity));
    let count = |activity: Activity| {
        waiting
            .iter()
            .filter(|one| one.activity == activity)
            .count()
    };
    let mut away: Vec<Elsewhere> = elsewhere.to_vec();
    away.sort_by_key(|one| (one.session.is_none(), !one.certain));
    Pride {
        here: here.len(),
        working: here
            .iter()
            .filter(|one| one.activity == Activity::Working)
            .count(),
        waiting: count(Activity::Waiting) + count(Activity::Failed),
        asking: count(Activity::NeedsYou),
        finished: count(Activity::TurnOver),
        wants: waiting.len(),
        elsewhere: elsewhere.len(),
        waiting_rows: waiting.into_iter().take(WAITING_ROWS).collect(),
        elsewhere_rows: away.into_iter().take(ELSEWHERE_ROWS).collect(),
    }
}

/// What the row of a session that wants the user says beside its name.
fn row_note(activity: Activity) -> &'static str {
    match activity {
        Activity::Failed => "failed",
        Activity::NeedsYou => "needs an answer",
        Activity::TurnOver => "finished its turn",
        _ => "waits for you",
    }
}

/// The sessions in one line, in plain words.
pub fn headline(pride: &Pride) -> String {
    if pride.here == 0 && pride.elsewhere == 0 {
        return "No session is running".to_owned();
    }
    let mut parts = Vec::new();
    if pride.here > 0 {
        parts.push(match pride.here {
            1 => "1 session here".to_owned(),
            n => format!("{n} sessions here"),
        });
        parts.push(format!("{} working", pride.working));
        parts.push(format!("{} waiting for you", pride.waiting));
        // Only said when there is one: the transcript tells these apart.
        if pride.asking > 0 {
            parts.push(format!("{} need an answer", pride.asking));
        }
        if pride.finished > 0 {
            parts.push(format!("{} finished", pride.finished));
        }
    }
    if pride.elsewhere > 0 {
        parts.push(format!("{} running elsewhere", pride.elsewhere));
    }
    parts.join(" \u{b7} ")
}

/// Something of the home the keyboard can be on, and a click can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// An action: its command.
    Action(Command),
    /// A session that waits: its place in [`Pride::waiting_rows`].
    Waiting(usize),
    /// A session that runs elsewhere: its place in
    /// [`Pride::elsewhere_rows`]. One the history does not know is listed
    /// but is no stop: there is nothing to open.
    Elsewhere(usize),
    /// A recent session: its place in the list.
    Recent(usize),
}

/// Everything the keyboard walks, in the order it is shown.
pub fn items(pride: &Pride, recents: usize) -> Vec<Item> {
    let mut all: Vec<Item> = ACTIONS.into_iter().map(Item::Action).collect();
    all.extend((0..pride.waiting_rows.len()).map(Item::Waiting));
    all.extend(
        pride
            .elsewhere_rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.session.is_some())
            .map(|(index, _)| Item::Elsewhere(index)),
    );
    all.extend((0..recents.min(RECENT_ROWS)).map(Item::Recent));
    all
}

/// The item `by` steps from `at` among `count`, stopping at the ends. An
/// item that is gone (the list got shorter) is the last one.
pub fn step(at: usize, count: usize, by: i64) -> usize {
    if count == 0 {
        return 0;
    }
    let last = count as i64 - 1;
    (at.min(count - 1) as i64).saturating_add(by).clamp(0, last) as usize
}

/// A recent session, as the home lists it.
struct Recent {
    id: SessionId,
    title: String,
    note: String,
}

pub(super) struct Model {
    pub(super) pride: Pride,
    recents: Vec<Recent>,
}

impl Shell {
    /// Shows the home in the main pane and gives it the keyboard. Nothing is
    /// closed: the terminals go on running and stay in the sidebar.
    pub(super) fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(window, cx);
        self.den_leave(cx);
        self.main = Main::Empty;
        self.pane = Pane::Main;
        self.home.at = 0;
        self.home.nav = Some(Nav::Home);
        self.sync_focus(window, cx);
        cx.notify();
    }

    /// The facts of the home: the sessions of this window by their dot, the
    /// sessions the last look at the processes found elsewhere, and the
    /// newest sessions of the history as the window already holds them.
    pub(super) fn home_model(&self) -> Model {
        let here: Vec<Here> = self
            .live
            .all()
            .iter()
            .map(|session| Here {
                id: session.id,
                name: session.label(),
                activity: session.activity,
            })
            .collect();
        let mut elsewhere = Vec::new();
        for machine in &self.snapshot.machines {
            let Some(report) = self.engine.elsewhere(&machine.id) else {
                continue;
            };
            let away = crate::elsewhere::foreign(
                &report.found,
                machine.id.is_local(),
                &self.own_terminals(&machine.id),
            );
            for found in away {
                let stored = found
                    .session
                    .as_ref()
                    .and_then(|id| self.snapshot.sessions.iter().find(|s| &s.id == id));
                let name = stored
                    .map(|stored| stored.title.trim().to_owned())
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| {
                        super::den::away_name(format::agent_name(found.agent), found.cwd.as_deref())
                    });
                let place = match (found.holder, machine.id.is_local()) {
                    (crate::elsewhere::Holder::OtherLeon(_), true) => "in another Leon".to_owned(),
                    (crate::elsewhere::Holder::OtherLeon(_), false) => {
                        format!("in another Leon on {}", machine.name)
                    }
                    (crate::elsewhere::Holder::Terminal, true) => "in another terminal".to_owned(),
                    (crate::elsewhere::Holder::Terminal, false) => {
                        format!("in a terminal on {}", machine.name)
                    }
                };
                elsewhere.push(Elsewhere {
                    session: stored.map(|stored| stored.id.clone()),
                    name,
                    place,
                    certain: found.is_certain(),
                });
            }
        }
        let now = self.now();
        let recents = self
            .snapshot
            .sessions
            .iter()
            .take(RECENT_ROWS)
            .map(|session| Recent {
                id: session.id.clone(),
                title: session.title.clone(),
                note: format!(
                    "{} \u{b7} {}",
                    format::agent_tag(session.agent),
                    format::age(now, session.updated_at)
                ),
            })
            .collect();
        Model {
            pride: pride(&here, &elsewhere),
            recents,
        }
    }

    /// The sessions in one line, for the heading of the main pane.
    pub(super) fn home_headline(&self) -> String {
        headline(&self.home_model().pride)
    }

    /// Moves the keyboard over the items of the home.
    pub(super) fn home_step(&mut self, by: i64, cx: &mut Context<Self>) {
        let model = self.home_model();
        let count = items(&model.pride, model.recents.len()).len();
        self.home.at = step(self.home.at, count, by);
        cx.notify();
    }

    /// Enter on the item of the home the keyboard is on.
    pub(super) fn home_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.home_model();
        let all = items(&model.pride, model.recents.len());
        if let Some(item) = all.get(step(self.home.at, all.len(), 0)).copied() {
            self.home_activate(item, window, cx);
        }
    }

    /// What a click on an item of the home does: an action runs its
    /// command, a session that waits is shown, and a session of the history
    /// (recent, or running elsewhere) opens as its row in the sidebar would.
    fn home_activate(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.home_model();
        if let Some(at) = items(&model.pride, model.recents.len())
            .iter()
            .position(|one| *one == item)
        {
            self.home.at = at;
        }
        let session = match item {
            Item::Action(command) => {
                self.run_command(command, window, cx);
                return;
            }
            Item::Waiting(index) => {
                if let Some(row) = model.pride.waiting_rows.get(index) {
                    if self.live.get(row.id).is_some() {
                        self.open_live(row.id, window, cx);
                    }
                }
                return;
            }
            Item::Elsewhere(index) => model
                .pride
                .elsewhere_rows
                .get(index)
                .and_then(|row| row.session.clone()),
            Item::Recent(index) => model.recents.get(index).map(|recent| recent.id.clone()),
        };
        let Some(session) = session else {
            return;
        };
        // As from the sidebar: the cursor goes to its row, and Enter there.
        let node = NodeId::Session(session);
        self.show(&node);
        let on_it = self
            .cursor
            .filter(|index| self.rows.get(*index).is_some_and(|row| row.id == node));
        if let Some(index) = on_it {
            self.home.nav = None;
            self.activate(index, window, cx);
        }
        cx.notify();
    }

    /// Opens what a row of the navigation stands for.
    pub(super) fn open_nav(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) {
        match nav {
            Nav::Home => self.go_home(window, cx),
            Nav::Den => {
                if !self.den_open() {
                    self.toggle_den(window, cx);
                }
                self.home.nav = Some(Nav::Den);
                cx.notify();
            }
        }
    }

    /// How many lions the Den has to show, and whether one of them waits
    /// for the user: this window's terminals and the sessions elsewhere
    /// that the Den follows.
    pub(super) fn den_glance(&self, cx: &gpui_kit::App) -> (usize, bool) {
        let live = self.live.all();
        let waits = live
            .iter()
            .any(|session| session.activity.wants_you() || session.activity == Activity::Failed)
            || self.den.cubs.iter().any(|cub| {
                self.den_open()
                    && matches!(
                        cub.state,
                        leon_den::CubState::WaitingForUser | leon_den::CubState::NeedsPermission
                    )
            });
        (live.len() + self.den_away(cx).len(), waits)
    }

    /// The navigation of the sidebar: Home and The Den, between the header
    /// and the filter and outside the tree's list, so that no scroll and no
    /// filter takes them away. Each is a row like a project's: the same
    /// height, the same columns for the icon and the name, the same hover
    /// and the same cursor.
    pub(super) fn render_nav(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let focused = self.pane == Pane::Sidebar;
        let hover = colours.surface;
        let (lions, waits) = self.den_glance(cx);
        let mut nav = div()
            .debug_selector(|| "sidebar-nav".into())
            .flex_none()
            // The rows end where the tree's do, before its scrollbar's column.
            .pr(metrics::SCROLLBAR_GUTTER())
            .border_b_1()
            .border_color(colours.border)
            .flex()
            .flex_col();
        for row in Nav::ALL {
            let on = self.home.nav == Some(row);
            let (selector, glyph) = match row {
                Nav::Home => ("sidebar-home", IconName::House),
                Nav::Den => ("sidebar-den", IconName::PawPrint),
            };
            let tip: SharedString = super::sidebar::tooltip_text(row.command()).into();
            nav = nav.child(
                div()
                    .id(selector)
                    .debug_selector(move || selector.into())
                    .relative()
                    .h(metrics::ROW_HEIGHT())
                    .w_full()
                    .pl(px(12.))
                    .pr(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(on, |this| this.bg(colours.surface_2))
                    .when(!on, |this| this.hover(move |style| style.bg(hover)))
                    // The row the keyboard is on carries the accent as a bar,
                    // as in the tree.
                    .when(on && focused, |this| {
                        this.child(
                            div()
                                .debug_selector(|| "sidebar-nav-cursor".into())
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(2.))
                                .bg(colours.signal),
                        )
                    })
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    // The columns of a project's row: the chevron's slot,
                    // the icon, the name, and what closes the row.
                    .child(div().flex_none().size(px(16.)))
                    .child(
                        div()
                            .flex_none()
                            .size(metrics::PROJECT_ICON())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(glyph, px(14.), colours.text_muted)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pr(px(6.))
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(row.label()),
                    )
                    .when(row == Nav::Den && waits, |this| {
                        this.child(
                            div()
                                .debug_selector(|| "sidebar-den-waits".into())
                                .child(activity_dot(Activity::Waiting, colours)),
                        )
                    })
                    .when(row == Nav::Den && lions > 0, |this| {
                        this.child(
                            mono(lions.to_string())
                                .debug_selector(|| "sidebar-den-count".into())
                                .text_color(colours.text_faint),
                        )
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.open_nav(row, window, cx);
                    })),
            );
        }
        nav
    }

    /// The home.
    pub(super) fn render_home(&self, colours: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let model = self.home_model();
        let all = items(&model.pride, model.recents.len());
        let at = step(self.home.at, all.len(), 0);
        let keyboard = self.pane == Pane::Main && matches!(self.main, Main::Empty);
        let on = |item: Item| keyboard && all.get(at) == Some(&item);
        let hover = colours.surface;

        // The actions: buttons that say their chord.
        let action = |command: Command, cx: &mut Context<Self>| {
            let item = Item::Action(command);
            let first = command == Command::ShowDen;
            let lit = on(item);
            div()
                .id(SharedString::from(format!("home-action-{command:?}")))
                .debug_selector(move || format!("hint-{command:?}"))
                // Two to a row where there is room, one where there is not;
                // the Den has a row of its own.
                .when(first, |this| this.w_full())
                .when(!first, |this| {
                    this.flex_grow(1.)
                        .flex_shrink(1.)
                        .flex_basis(px(240.))
                        .min_w_0()
                })
                .h(if first {
                    metrics::CONTROL() + px(10.)
                } else {
                    metrics::CONTROL()
                })
                .px(px(12.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if lit || first {
                    colours.signal
                } else {
                    colours.elevated_border
                })
                .when(lit, |this| this.bg(colours.surface_2))
                .when(!lit, |this| this.hover(move |style| style.bg(hover)))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .cursor_pointer()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(if first || lit {
                            colours.text
                        } else {
                            colours.text_muted
                        })
                        .child(keys::label(command)),
                )
                .children(keys::keys_label(command).map(|text| key_cap(text, colours)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.home_activate(item, window, cx);
                }))
        };
        let mut actions = div()
            .debug_selector(|| "home-actions".into())
            .flex()
            .flex_wrap()
            .justify_center()
            .gap(px(12.))
            .w_full();
        for command in ACTIONS {
            actions = actions.child(action(command, cx));
        }

        // A row of a list: what leads it, a name, a note.
        let row = |selector: String,
                   item: Option<Item>,
                   lead: Div,
                   name: String,
                   note: String,
                   cx: &mut Context<Self>| {
            let lit = item.is_some_and(on);
            let id = SharedString::from(selector.clone());
            div()
                .id(id)
                .debug_selector(move || selector.clone())
                .h(metrics::ROW_HEIGHT())
                .px(px(8.))
                .rounded(metrics::RADIUS())
                .flex()
                .items_center()
                .gap(px(8.))
                .when(lit, |this| this.bg(colours.surface_2))
                .when(item.is_some() && !lit, |this| {
                    this.cursor_pointer().hover(move |style| style.bg(hover))
                })
                .when(lit, |this| this.cursor_pointer())
                .child(
                    div()
                        .flex_none()
                        .w(px(12.))
                        .flex()
                        .justify_center()
                        .child(lead),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if item.is_some() {
                            colours.text
                        } else {
                            colours.text_muted
                        })
                        .child(name),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(note),
                )
                .when_some(item, |this, item| {
                    this.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.home_activate(item, window, cx);
                    }))
                })
        };
        let block = |name: &'static str, title: &str| {
            div()
                .debug_selector(move || name.into())
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_grow(1.)
                .flex_shrink(1.)
                .flex_basis(px(260.))
                .min_w_0()
                .child(div().pb(px(4.)).child(section_label(title, colours)))
        };

        let pride = &model.pride;
        let mut sessions = block("home-sessions", "Sessions").child(
            div()
                .debug_selector(|| "home-headline".into())
                .px(px(8.))
                .pb(px(4.))
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text_muted)
                .child(headline(pride)),
        );
        for (index, one) in pride.waiting_rows.iter().enumerate() {
            let note = row_note(one.activity);
            sessions = sessions.child(row(
                format!("home-waiting-{index}"),
                Some(Item::Waiting(index)),
                activity_dot(one.activity, colours),
                one.name.clone(),
                note.to_owned(),
                cx,
            ));
        }
        if pride.wants > pride.waiting_rows.len() {
            sessions = sessions.child(
                div()
                    .px(px(8.))
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint)
                    .child(format!(
                        "and {} more waiting",
                        pride.wants - pride.waiting_rows.len()
                    )),
            );
        }
        for (index, one) in pride.elsewhere_rows.iter().enumerate() {
            let probably = if one.certain { "" } else { "probably " };
            sessions = sessions.child(row(
                format!("home-elsewhere-{index}"),
                one.session.as_ref().map(|_| Item::Elsewhere(index)),
                elsewhere_mark(one.certain, colours),
                one.name.clone(),
                format!("{probably}{}", one.place),
                cx,
            ));
        }
        if pride.elsewhere > pride.elsewhere_rows.len() {
            sessions = sessions.child(
                div()
                    .px(px(8.))
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint)
                    .child(format!(
                        "and {} more running elsewhere",
                        pride.elsewhere - pride.elsewhere_rows.len()
                    )),
            );
        }

        let mut recent = block("home-recent", "Recent sessions");
        for (index, one) in model.recents.iter().enumerate() {
            recent = recent.child(row(
                format!("home-recent-{index}"),
                Some(Item::Recent(index)),
                div(),
                one.title.clone(),
                one.note.clone(),
                cx,
            ));
        }
        if model.recents.is_empty() {
            recent = recent.child(
                div()
                    .px(px(8.))
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint)
                    .child("No session in the history yet"),
            );
        }

        // What there is to open, as the dimension of the drawing.
        let caption = format!(
            "{} PROJECTS \u{b7} {} SESSIONS",
            self.snapshot.projects.len(),
            self.snapshot.sessions.len()
        );
        div()
            .id("home")
            .debug_selector(|| "main-empty".into())
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .min_h_full()
                    .px(px(16.))
                    .py(px(24.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(16.))
                    .child(mark(
                        px(36.),
                        colours,
                        cx,
                        Lion::new("home-mark").mood(self.mark_mood()),
                    ))
                    .child(section_label("Home", colours))
                    // A column as wide as the pane allows, and no wider than
                    // reads well: what is in it wraps inside it.
                    .child(
                        div()
                            .w_full()
                            .max_w(px(660.))
                            .flex()
                            .flex_col()
                            .child(empty_frame("main-empty", actions, Some(caption), colours)),
                    )
                    .child(
                        div()
                            .debug_selector(|| "home-lists".into())
                            .w_full()
                            .max_w(px(660.))
                            .px(px(20.))
                            .flex()
                            .flex_wrap()
                            .gap(px(12.))
                            .child(sessions)
                            .child(recent),
                    )
                    .child(
                        div()
                            .debug_selector(|| "main-empty-remote".into())
                            .w_full()
                            .max_w(px(660.))
                            .px(px(20.))
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(format!(
                                "Projects on another computer work too: connect a machine ({}) with a code, no network setup needed, or over SSH.",
                                keys::keys_label(Command::AddMachine).unwrap_or_default()
                            )),
                    )
                    .child(
                        // The product's name is a name, not a label: as it is
                        // written, in the interface font.
                        div()
                            .debug_selector(|| "maker".into())
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_faint)
                            .child(format!(
                                "{} {} \u{b7} by {}",
                                crate::product::PRODUCT_NAME,
                                crate::product::VERSION,
                                crate::product::MAKER
                            )),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here(id: u64, name: &str, activity: Activity) -> Here {
        Here {
            id: LiveId(id),
            name: name.to_owned(),
            activity,
        }
    }

    fn away(name: &str, known: bool, certain: bool) -> Elsewhere {
        Elsewhere {
            session: known.then(|| SessionId::from_string(format!("s-{name}"))),
            name: name.to_owned(),
            place: "in another terminal".to_owned(),
            certain,
        }
    }

    #[test]
    fn the_sessions_are_counted_by_what_their_dot_says() {
        let pride = pride(
            &[
                here(1, "a", Activity::Working),
                here(2, "b", Activity::Waiting),
                here(3, "c", Activity::Idle),
                here(4, "d", Activity::Failed),
                here(5, "e", Activity::Working),
                here(6, "f", Activity::Off),
            ],
            &[away("x", true, true)],
        );
        assert_eq!(
            (pride.here, pride.working, pride.waiting, pride.elsewhere),
            (6, 2, 2, 1)
        );
        // A failure is what wants the user most.
        let names: Vec<&str> = pride
            .waiting_rows
            .iter()
            .map(|row| row.name.as_str())
            .collect();
        assert_eq!(names, ["d", "b"]);
        assert_eq!(
            headline(&pride),
            "6 sessions here \u{b7} 2 working \u{b7} 2 waiting for you \u{b7} 1 running elsewhere"
        );
    }

    #[test]
    fn the_transcript_tells_a_finished_agent_from_one_that_asks() {
        let pride = pride(
            &[
                here(1, "idle", Activity::Idle),
                here(2, "done", Activity::TurnOver),
                here(3, "quiet", Activity::Waiting),
                here(4, "asks", Activity::NeedsYou),
                here(5, "also done", Activity::TurnOver),
                here(6, "broke", Activity::Failed),
                here(7, "busy", Activity::Working),
            ],
            &[],
        );
        assert_eq!(
            (
                pride.waiting,
                pride.asking,
                pride.finished,
                pride.wants,
                pride.working
            ),
            (2, 1, 2, 5, 1)
        );
        // Most urgent first; the ones of one state in the order they started.
        let names: Vec<&str> = pride
            .waiting_rows
            .iter()
            .map(|row| row.name.as_str())
            .collect();
        assert_eq!(names, ["broke", "asks", "quiet", "done"]);
        assert_eq!(
            headline(&pride),
            "7 sessions here \u{b7} 1 working \u{b7} 2 waiting for you \u{b7} 1 need an answer \u{b7} 2 finished"
        );
        assert_eq!(row_note(Activity::NeedsYou), "needs an answer");
        assert_eq!(row_note(Activity::TurnOver), "finished its turn");
        assert_eq!(row_note(Activity::Waiting), "waits for you");
        assert_eq!(row_note(Activity::Failed), "failed");
    }

    #[test]
    fn the_headline_says_only_what_there_is() {
        assert_eq!(headline(&Pride::default()), "No session is running");
        assert_eq!(
            headline(&pride(&[here(1, "a", Activity::Idle)], &[])),
            "1 session here \u{b7} 0 working \u{b7} 0 waiting for you"
        );
        assert_eq!(
            headline(&pride(
                &[],
                &[away("x", false, false), away("y", true, true)]
            )),
            "2 running elsewhere"
        );
    }

    #[test]
    fn only_a_few_rows_are_listed_and_the_rest_is_counted() {
        let many: Vec<Here> = (0..9)
            .map(|n| here(n, &format!("s{n}"), Activity::Waiting))
            .collect();
        let away: Vec<Elsewhere> = vec![
            away("unknown", false, true),
            away("guess", true, false),
            away("sure", true, true),
            away("other", true, true),
            away("last", false, false),
        ];
        let pride = pride(&many, &away);
        assert_eq!((pride.wants, pride.waiting_rows.len()), (9, WAITING_ROWS));
        assert_eq!(
            (pride.elsewhere, pride.elsewhere_rows.len()),
            (5, ELSEWHERE_ROWS)
        );
        // The ones the history knows first, the certain before the guessed.
        let names: Vec<&str> = pride
            .elsewhere_rows
            .iter()
            .map(|row| row.name.as_str())
            .collect();
        assert_eq!(names, ["sure", "other", "guess"]);
    }

    #[test]
    fn the_keyboard_walks_the_actions_then_the_sessions_then_the_recent_ones() {
        let pride = pride(
            &[
                here(1, "a", Activity::Waiting),
                here(2, "b", Activity::Working),
            ],
            &[away("known", true, true), away("unknown", false, true)],
        );
        let all = items(&pride, 9);
        assert_eq!(all[0], Item::Action(Command::ShowDen), "the Den is first");
        assert_eq!(all.len(), ACTIONS.len() + 1 + 1 + RECENT_ROWS);
        assert_eq!(all[ACTIONS.len()], Item::Waiting(0));
        // A session elsewhere that the history does not know is no stop.
        assert_eq!(all[ACTIONS.len() + 1], Item::Elsewhere(0));
        assert_eq!(all[ACTIONS.len() + 2], Item::Recent(0));
        assert_eq!(*all.last().unwrap(), Item::Recent(RECENT_ROWS - 1));
        // With nothing running and no history: the actions alone.
        assert_eq!(items(&Pride::default(), 0).len(), ACTIONS.len());
        // Every action is a command the registry has, once.
        let mut seen = std::collections::HashSet::new();
        for command in ACTIONS {
            assert!(seen.insert(command));
            assert!(!keys::label(command).is_empty());
        }
    }

    #[test]
    fn stepping_stops_at_the_ends_and_survives_a_list_that_got_shorter() {
        assert_eq!(step(0, 5, 1), 1);
        assert_eq!(step(4, 5, 1), 4);
        assert_eq!(step(0, 5, -1), 0);
        assert_eq!(step(2, 5, -8), 0);
        assert_eq!(step(2, 5, 8), 4);
        assert_eq!(step(2, 5, i64::MAX), 4);
        assert_eq!(step(2, 5, i64::MIN), 0);
        // The keyboard was on an item that is gone.
        assert_eq!(step(40, 5, 0), 4);
        assert_eq!(step(40, 5, -1), 3);
        assert_eq!(step(usize::MAX, 1, 1), 0);
        assert_eq!(step(3, 0, 1), 0);
    }
}

//! The palette: one field over everything the application can go to or do.
//!
//! Cmd or Ctrl with P (or K) opens it to go somewhere: machines, projects,
//! worktrees and sessions. Cmd or Ctrl with Shift and P opens it on the
//! commands, and after them every theme and every appearance as a command of
//! its own, previewed under the selection. A prefix says what is looked through, and can be typed or
//! deleted at any time: `>` commands, `@` machines, `#` projects and
//! worktrees, `/` the full text of the whole session history, `~` the files
//! of the project in view (quick open, with `path:line`), `%` the text inside
//! them, `?` the list of these. The last two are `editor/quick.rs`'s.
//!
//! Every command is an entry of the registry (`crate::keys`), so nothing that
//! is on a key is missing here. A command that needs to be told something
//! asks in place: the question replaces the list and its answer is the next
//! row (`steps.rs` holds the questions).
//!
//! Names are matched as they are typed. What is read from the store (sessions
//! by title, the full-text search) is read off the UI thread, a moment after
//! the typing stops; a newer keystroke drops the read that was under way.

use super::editor::{file_items, hit_items, QuickFiles, TextSearch};
use super::shell::Pane;
use super::shell::{Overlay, Shell};
use super::steps::{self, Action, Choice, Custom, Outcome, Step, StepKind, Validate, World};
use super::tree::NodeId;
use super::widgets::{key_cap, mono, section_label};
use crate::address;
use crate::fuzzy::{rank, score};
use crate::icons::{agent_icon, icon, IconName};
use crate::keys::{self, Command, BINDINGS};
use crate::schema::{self, Def};
use crate::settings::AppearanceChoice;
use crate::theme::{metrics, px, Palette as Colours, ThemeId};
use crate::usage::{Target, Usage};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, Context, Div, Entity, FontWeight, HighlightStyle, Keystroke, SharedString, Stateful,
    StyledText, Task, Window,
};
use leon_core::{
    snippet_segments, Machine, Project, SearchHit, SearchQuery, Session, SessionFilter, Store,
    Worktree,
};
use std::sync::Arc;
use std::time::Duration;

/// How many of each kind the palette lists.
const PER_KIND: usize = 8;
/// How many files the plain palette lists beside the places.
const FILES_IN_ALL: usize = 20;
/// How many commands it lists.
const COMMANDS: usize = 120;
/// How many hits the full-text search asks for.
const HITS: usize = 40;
/// How many recent sessions are looked through by title.
const TITLE_POOL: usize = 400;
/// How many rows a page key moves.
const PAGE: usize = 6;
/// The wait after the last keystroke before the store is read.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

/// What the palette is looking through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Where to go: machines, projects, worktrees and sessions.
    All,
    /// `>`: commands.
    Commands,
    /// `@`: machines.
    Machines,
    /// `#`: projects and worktrees.
    Projects,
    /// `/`: the full text of the session history.
    History,
    /// `~`: the files of the project in view, to open one.
    Files,
    /// `%`: the text inside those files.
    Text,
    /// `?`: what the prefixes are.
    Help,
}

/// The prefixes, as `?` lists them.
pub const MODES: [(&str, &str); 8] = [
    ("", "Go to a project, session or machine, or open a file"),
    (">", "Run a command"),
    ("@", "Find a machine"),
    ("#", "Find a project or a worktree"),
    ("/", "Search the text of every session"),
    ("~", "Open a file of the project by name"),
    ("%", "Search the text of the project's files"),
    ("?", "Show these prefixes"),
];

/// Splits what was typed into what to look through and what to look for.
pub fn parse(typed: &str) -> (Scope, &str) {
    let typed = typed.trim_start();
    match typed.chars().next() {
        Some('>') => (Scope::Commands, typed[1..].trim()),
        Some('@') => (Scope::Machines, typed[1..].trim()),
        Some('#') => (Scope::Projects, typed[1..].trim()),
        Some('/') => (Scope::History, typed[1..].trim()),
        Some('~') => (Scope::Files, typed[1..].trim()),
        Some('%') => (Scope::Text, typed[1..].trim()),
        Some('?') => (Scope::Help, typed[1..].trim()),
        _ => (Scope::All, typed.trim()),
    }
}

/// A theme or an appearance offered as a command of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// Wear this theme.
    Theme(ThemeId),
    /// Wear this appearance.
    Appearance(AppearanceChoice),
}

impl Look {
    /// Every look: each theme of the registry, then each appearance.
    pub fn all() -> Vec<Look> {
        crate::theme::registry::all()
            .into_iter()
            .map(Look::Theme)
            .chain(AppearanceChoice::ALL.into_iter().map(Look::Appearance))
            .collect()
    }

    /// The name of the row.
    pub fn label(self) -> String {
        match self {
            Look::Theme(id) => format!("Theme: {}", id.name()),
            Look::Appearance(choice) => format!("Appearance: {}", choice.label()),
        }
    }

    /// Words that find it besides its name.
    fn keywords(self) -> String {
        match self {
            Look::Theme(id) => format!("{} theme colors colours", id.slug()),
            Look::Appearance(choice) => format!(
                "{} appearance colors colours",
                choice.label().to_lowercase()
            ),
        }
    }

    /// How well `query` finds it: by name first, then by its keywords.
    fn score(self, query: &str) -> Option<u32> {
        score(query, &self.label()).or_else(|| score(query, &self.keywords()))
    }

    /// Whether it is the one in use (kept, not previewed).
    pub fn is_current(self, world: &World) -> bool {
        match self {
            Look::Theme(id) => id == world.theme_id,
            Look::Appearance(choice) => choice == world.theme,
        }
    }
}

/// One row of the palette.
#[derive(Clone, Debug)]
pub enum Item {
    /// A heading.
    Section(&'static str),
    /// A machine to show.
    Machine(Machine),
    /// A project to open.
    Project(Project),
    /// A worktree to open, with its project.
    Worktree(Worktree, Project),
    /// A session to open.
    Session(Session),
    /// A match inside a session's text.
    Hit(Box<SearchHit>),
    /// A file of the project, by its path from the root, to open at a line.
    File(String, Option<u32>),
    /// The file the lines that follow are in.
    FileHeading(String),
    /// A line of a file that matches the text searched for.
    Match(Box<crate::search::SearchHit>),
    /// A command to run.
    Command(Command),
    /// A theme or an appearance to wear.
    Look(Look),
    /// A setting to change.
    Setting(&'static Def),
    /// A choice of the question being asked: its place among the choices.
    Choice(usize),
    /// Typed text taken as the answer.
    Custom(String),
    /// A prefix, listed by `?`: its place in [`MODES`].
    Mode(usize),
    /// A line to read, not a row to pick.
    Line(SharedString, SharedString),
}

impl Item {
    fn is_row(&self) -> bool {
        !matches!(
            self,
            Item::Section(_) | Item::Line(..) | Item::FileHeading(_)
        )
    }
}

/// The question being asked, with what has been answered so far.
#[derive(Clone, Debug)]
pub struct Flow {
    /// The command being asked for.
    pub command: Command,
    /// The answers so far.
    pub answers: Vec<String>,
    /// The question on screen.
    pub step: Step,
}

/// What was read from the store for the query on screen.
#[derive(Default)]
struct Found {
    sessions: Vec<Session>,
    hits: Vec<SearchHit>,
}

/// The palette's state.
pub struct PaletteState {
    pub(super) input: Entity<InputState>,
    /// What it lists.
    pub items: Vec<Item>,
    /// The row the keyboard is on, among `items`.
    pub cursor: usize,
    /// The question being asked, if a command is.
    pub flow: Option<Flow>,
    /// The setting whose question is being asked, when it is a setting's.
    pub setting: Option<&'static Def>,
    /// What is wrong with the text typed as an answer.
    pub error: Option<String>,
    /// The machines and projects it can go to, read when it opened.
    pub world: Option<World>,
    found: Found,
    generation: u64,
    search: Option<Task<()>>,
    /// The files quick open lists.
    pub files: QuickFiles,
    /// The text search of the project.
    pub text: TextSearch,
    usage: Usage,
    usage_file: Option<std::path::PathBuf>,
}

impl PaletteState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        let usage_file = crate::settings::sibling(crate::usage::FILE_NAME, cx);
        let usage = usage_file.as_deref().map(Usage::load).unwrap_or_default();
        Self {
            input: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Go to a project, or type > for commands")
            }),
            items: Vec::new(),
            cursor: 0,
            flow: None,
            setting: None,
            error: None,
            world: None,
            found: Found::default(),
            generation: 0,
            search: None,
            files: QuickFiles::default(),
            text: TextSearch::default(),
            usage,
            usage_file,
        }
    }

    /// The rows the keyboard can stop on, by place in `items`.
    fn rows(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_row())
            .map(|(index, _)| index)
            .collect()
    }

    fn save_usage(&self) {
        if let Some(file) = &self.usage_file {
            if let Err(error) = self.usage.save(file) {
                tracing::warn!(%error, "could not remember what the palette was used for");
            }
        }
    }
}

/// The name a command is remembered by.
fn command_name(command: Command) -> String {
    format!("{command:?}")
}

impl Shell {
    // ----- opening and closing -------------------------------------------------

    /// Opens the palette with `prefix` typed: empty to go somewhere, `>` for
    /// the commands, `/` to search the history.
    pub(super) fn open_palette(
        &mut self,
        prefix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.palette.flow = None;
        self.palette.setting = None;
        self.palette.error = None;
        self.palette.found = Found::default();
        self.palette.search = None;
        self.palette.world = Some(self.world(cx));
        self.palette.files.reopened();
        self.palette.text.reset();
        self.overlay = Overlay::Palette;
        let prefix = prefix.to_owned();
        self.palette.input.update(cx, |field, cx| {
            field.set_placeholder("Go to a project, or type > for commands", window, cx);
            field.set_value(prefix, window, cx);
            field.focus(window, cx);
        });
        self.palette_scope_changed(cx);
        self.fill_palette(cx);
    }

    /// Closes the palette and gives the keyboard back to the panes.
    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette.flow = None;
        self.palette.setting = None;
        self.palette.error = None;
        self.palette.search = None;
        self.palette.found = Found::default();
        self.cancel_text_search();
        // A theme or appearance that was only being previewed goes back to
        // the one that is kept. A choice that was made is already kept.
        crate::settings::clear_preview(cx);
        self.overlay = Overlay::None;
        self.focus.focus(window, cx);
        // A question asked from the Settings screen ends back in it.
        if std::mem::take(&mut self.settings_ui.return_from_palette) {
            self.overlay = Overlay::Settings;
            self.settings_focus(window, cx);
        }
        cx.notify();
    }

    // ----- flows -----------------------------------------------------------------

    /// Starts asking for `command`, opening the palette if it is closed.
    pub(super) fn begin_flow(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_flow_with(command, Vec::new(), window, cx);
    }

    /// Starts a flow with its first questions already answered: what a
    /// context menu does, since it knows the row it was opened on.
    pub(super) fn begin_flow_with(
        &mut self,
        command: Command,
        answers: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_palette("", window, cx);
        self.palette.world = Some(self.world(cx));
        self.advance_flow(command, answers, window, cx);
    }

    fn advance_flow(
        &mut self,
        command: Command,
        answers: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let world = self.palette.world.clone().unwrap_or_else(|| self.world(cx));
        let outcome = match self.palette.setting {
            Some(def) => {
                steps::advance_setting(def, &answers, &crate::settings::value(cx, def.key))
            }
            None => steps::advance(command, &answers, &world),
        };
        match outcome {
            Outcome::Ask(step) => {
                let placeholder = match &step.kind {
                    StepKind::Text { placeholder, .. } => placeholder.clone(),
                    StepKind::Choices { .. } => "Type to narrow, Enter to choose".to_owned(),
                };
                self.palette.flow = Some(Flow {
                    command,
                    answers,
                    step,
                });
                self.palette.error = None;
                self.palette.found = Found::default();
                self.palette.search = None;
                self.palette.input.update(cx, |field, cx| {
                    field.set_value("", window, cx);
                    field.set_placeholder(placeholder, window, cx);
                    field.focus(window, cx);
                });
                self.fill_palette(cx);
            }
            Outcome::Run(action) => {
                self.close_palette(window, cx);
                self.apply(action, window, cx);
            }
            Outcome::Refuse(why) => {
                self.close_palette(window, cx);
                self.engine.report(crate::engine::StatusKind::Error, why);
                cx.notify();
            }
        }
    }

    /// Starts asking for a new value of a setting, opening the palette.
    pub(super) fn begin_setting_flow(
        &mut self,
        def: &'static Def,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_palette("", window, cx);
        self.palette.setting = Some(def);
        self.advance_flow(Command::Settings, Vec::new(), window, cx);
    }

    /// Takes `value` as the answer to the question on screen.
    fn answer(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = self.palette.flow.clone() else {
            return;
        };
        let mut answers = flow.answers;
        answers.push(value);
        self.advance_flow(flow.command, answers, window, cx);
    }

    /// Escape, or Backspace in an empty field: back one question, and from the
    /// first question back to the commands. `false` when no command was being
    /// asked for.
    pub(super) fn palette_back(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(flow) = self.palette.flow.clone() else {
            return false;
        };
        if flow.answers.is_empty() {
            self.palette.flow = None;
            self.palette.setting = None;
            self.palette.error = None;
            self.palette.input.update(cx, |field, cx| {
                field.set_placeholder("Go to a project, or type > for commands", window, cx);
                field.set_value(">", window, cx);
            });
            self.fill_palette(cx);
        } else {
            let mut answers = flow.answers;
            answers.pop();
            self.advance_flow(flow.command, answers, window, cx);
        }
        true
    }

    // ----- what is listed ------------------------------------------------------------

    /// Rebuilds the rows for what is typed and what the store answered.
    pub(super) fn fill_palette(&mut self, cx: &mut Context<Self>) {
        let typed = self.palette.input.read(cx).value().to_string();
        let mut items: Vec<Item> = Vec::new();
        let section = |items: &mut Vec<Item>, title: &'static str, rows: Vec<Item>| {
            if !rows.is_empty() {
                items.push(Item::Section(title));
                items.extend(rows);
            }
        };

        if let Some(flow) = &self.palette.flow {
            let query = typed.trim();
            match &flow.step.kind {
                StepKind::Choices { choices, custom } => {
                    let places: Vec<usize> = (0..choices.len()).collect();
                    let ranked = rank(query, places, |place| choices[*place].label.clone());
                    let none_ranked = ranked.is_empty();
                    items.extend(ranked.into_iter().map(Item::Choice));
                    let exact = choices.iter().any(|choice| choice.label == query);
                    let accepted = match custom {
                        Custom::No => false,
                        Custom::Any => true,
                        Custom::AbsolutePath => address::is_absolute_path(query),
                    };
                    if accepted && !query.is_empty() && !exact {
                        let typed = Item::Custom(query.to_owned());
                        // What was typed in full outranks a guess at it.
                        if *custom == Custom::AbsolutePath {
                            items.insert(0, typed);
                        } else {
                            items.push(typed);
                        }
                    }
                    if none_ranked && items.is_empty() {
                        let line = match custom {
                            Custom::AbsolutePath => "Type an absolute path, such as /srv/api.",
                            _ => "Nothing matches.",
                        };
                        items.push(Item::Line(line.into(), "".into()));
                    }
                }
                StepKind::Text { validate, .. } => {
                    let hint = match validate {
                        Validate::Optional => "Enter to continue, empty is fine",
                        _ => "Enter to continue",
                    };
                    if !query.is_empty() {
                        items.push(Item::Line(query.to_owned().into(), hint.into()));
                    } else {
                        items.push(Item::Line("Type the answer.".into(), hint.into()));
                    }
                    if let Some(error) = &self.palette.error {
                        items.push(Item::Line(error.clone().into(), "".into()));
                    }
                }
            }
            self.palette.cursor = items.iter().position(Item::is_row).unwrap_or(0);
            // A question about the look opens on the value in use, so that
            // opening it previews nothing new.
            if query.is_empty() {
                if let StepKind::Choices { choices, .. } = &flow.step.kind {
                    if let Some(at) = items.iter().position(|item| {
                        matches!(item, Item::Choice(place)
                            if choices.get(*place).is_some_and(|choice| choice.current))
                    }) {
                        self.palette.cursor = at;
                    }
                }
            }
            self.palette.items = items;
            self.sync_preview(cx);
            return cx.notify();
        }

        let (scope, query) = parse(&typed);
        let world = self.palette.world.clone();
        let world = world.as_ref();
        match scope {
            Scope::Help => {
                items.push(Item::Section("What to type"));
                items.extend((0..MODES.len()).map(Item::Mode));
            }
            Scope::Commands => {
                let (commands, looks, settings) = self.ranked_commands(query);
                section(&mut items, "Commands", commands);
                section(&mut items, "Settings", settings);
                section(&mut items, "Themes", looks);
            }
            Scope::Machines => {
                section(&mut items, "Machines", machine_items(world, query, 100));
            }
            Scope::Projects => {
                section(&mut items, "Projects", project_items(world, query, 100));
                section(&mut items, "Worktrees", worktree_items(world, query, 100));
            }
            Scope::History => {
                if query.is_empty() {
                    items.push(Item::Line(
                        "Type to search the text of every session.".into(),
                        "".into(),
                    ));
                }
                section(
                    &mut items,
                    "History",
                    self.palette
                        .found
                        .hits
                        .iter()
                        .cloned()
                        .map(|hit| Item::Hit(Box::new(hit)))
                        .collect(),
                );
            }
            Scope::Files => match (self.palette.files.list(), &self.palette.files.failed) {
                (Some(files), _) => {
                    let rows = file_items(query, files);
                    if rows.is_empty() {
                        items.push(Item::Line("No file matches.".into(), "".into()));
                    }
                    section(&mut items, "Files", rows);
                }
                (None, Some(why)) => items.push(Item::Line(why.clone().into(), "".into())),
                (None, None) if self.palette.files.target.is_none() => {
                    items.push(Item::Line("Open a project first.".into(), "".into()))
                }
                (None, None) => {
                    items.push(Item::Line("Listing the files\u{2026}".into(), "".into()))
                }
            },
            Scope::Text => {
                let search = &self.palette.text;
                if query.is_empty() {
                    items.push(Item::Line(
                        "Type to search the text of the project's files.".into(),
                        "".into(),
                    ));
                } else if let Some(why) = &search.error {
                    items.push(Item::Line(why.clone().into(), "".into()));
                } else if search.running {
                    items.push(Item::Line("Searching\u{2026}".into(), "".into()));
                } else if search.hits.is_empty() {
                    items.push(Item::Line("No matches.".into(), "".into()));
                }
                let (rows, more) = hit_items(&search.hits);
                items.extend(rows);
                if more || search.truncated {
                    items.push(Item::Line(
                        format!(
                            "Showing the first {} lines: narrow the search to see the rest.",
                            search.hits.len().min(super::editor::HIT_ROWS)
                        )
                        .into(),
                        "".into(),
                    ));
                }
            }
            Scope::All if query.is_empty() => {
                section(&mut items, "Recent", self.recent_items(world));
                section(&mut items, "Machines", machine_items(world, "", PER_KIND));
                section(&mut items, "Projects", project_items(world, "", PER_KIND));
            }
            Scope::All => {
                section(
                    &mut items,
                    "Machines",
                    machine_items(world, query, PER_KIND),
                );
                section(
                    &mut items,
                    "Projects",
                    project_items(world, query, PER_KIND),
                );
                section(
                    &mut items,
                    "Worktrees",
                    worktree_items(world, query, PER_KIND),
                );
                section(
                    &mut items,
                    "Sessions",
                    self.palette
                        .found
                        .sessions
                        .iter()
                        .cloned()
                        .map(Item::Session)
                        .collect(),
                );
                // The files of the project in view come last, as in an editor.
                if let Some(files) = self.palette.files.list() {
                    let rows: Vec<Item> = file_items(query, files)
                        .into_iter()
                        .take(FILES_IN_ALL)
                        .collect();
                    section(&mut items, "Files", rows);
                }
            }
        }
        self.palette.cursor = items.iter().position(Item::is_row).unwrap_or(0);
        self.palette.items = items;
        self.sync_preview(cx);
        cx.notify();
    }

    /// Shows the theme or appearance under the selection of a question about
    /// the look, on the whole window, without keeping it; shows what is kept
    /// when no such question is open or nothing is selected.
    pub(super) fn sync_preview(&mut self, cx: &mut Context<Self>) {
        let mut preview = crate::settings::Preview::default();
        if self.palette.flow.is_none() {
            match self.palette.items.get(self.palette.cursor) {
                Some(Item::Look(Look::Theme(id))) => preview.theme = id.is_usable().then_some(*id),
                Some(Item::Look(Look::Appearance(choice))) => preview.appearance = Some(*choice),
                _ => {}
            }
        }
        if let Some(Flow {
            command,
            step:
                Step {
                    kind: StepKind::Choices { choices, .. },
                    ..
                },
            ..
        }) = &self.palette.flow
        {
            let value = match self.palette.items.get(self.palette.cursor) {
                Some(Item::Choice(place)) => {
                    choices.get(*place).map(|choice| choice.value.as_str())
                }
                _ => None,
            };
            if let Some(value) = value {
                let key = self.palette.setting.map(|def| def.key);
                match command {
                    _ if key == Some("theme_id") => {
                        preview.theme =
                            crate::theme::ThemeId::parse(value).filter(|id| id.is_usable())
                    }
                    _ if key == Some("theme") => {
                        preview.appearance = crate::settings::AppearanceChoice::parse(value)
                    }
                    Command::ChooseTheme => {
                        preview.theme =
                            crate::theme::ThemeId::parse(value).filter(|id| id.is_usable())
                    }
                    Command::SetAppearance => {
                        preview.appearance = crate::settings::AppearanceChoice::parse(value)
                    }
                    _ => {}
                }
            }
        }
        crate::settings::preview(cx, preview);
    }

    /// The commands that answer `query`, best first; the ones used most float
    /// up without ever passing a better kind of match. The themes and
    /// appearances rank among them when something is typed; with nothing
    /// typed they come apart, as the second list, under their own heading.
    fn ranked_commands(&self, query: &str) -> (Vec<Item>, Vec<Item>, Vec<Item>) {
        let mut scored: Vec<(u32, Item)> = BINDINGS
            .iter()
            .filter(|binding| binding.palette)
            .filter(|binding| {
                !crate::keys::needs_file(binding.command) || self.focused_file().is_some()
            })
            .filter_map(|binding| {
                let base = score(query, binding.label).or_else(|| {
                    let words = crate::keys::keywords(binding.command);
                    (!words.is_empty()).then(|| score(query, words)).flatten()
                })?;
                let boost = self.palette.usage.boost(&command_name(binding.command));
                Some((base + boost, Item::Command(binding.command)))
            })
            .collect();
        let mut found: Vec<(u32, Item)> = if query.is_empty() {
            Vec::new()
        } else {
            schema::settings()
                .iter()
                .filter(|def| def.platform.here())
                .filter_map(|def| {
                    let base = score(query, def.label)
                        .or_else(|| score(query, def.keywords))
                        .or_else(|| score(query, def.section.title()))?;
                    Some((base, Item::Setting(def)))
                })
                .collect()
        };
        found.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        let settings: Vec<Item> = found.into_iter().map(|(_, item)| item).collect();
        let looks = Look::all();
        let mut apart = Vec::new();
        if query.is_empty() {
            apart.extend(looks.into_iter().map(Item::Look));
        } else {
            scored.extend(
                looks
                    .into_iter()
                    .filter_map(|look| Some((look.score(query)?, Item::Look(look)))),
            );
        }
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        let commands = scored
            .into_iter()
            .take(COMMANDS)
            .map(|(_, item)| item)
            .collect();
        (commands, apart, settings)
    }

    /// The places gone to last that still exist.
    fn recent_items(&self, world: Option<&World>) -> Vec<Item> {
        let Some(world) = world else {
            return Vec::new();
        };
        self.palette
            .usage
            .recent_targets()
            .iter()
            .filter_map(|target| match target.kind.as_str() {
                "machine" => world
                    .machines
                    .iter()
                    .find(|machine| machine.id.as_str() == target.id)
                    .cloned()
                    .map(Item::Machine),
                "project" => world
                    .projects
                    .iter()
                    .find(|info| info.project.id.as_str() == target.id)
                    .map(|info| Item::Project(info.project.clone())),
                "worktree" => world.projects.iter().find_map(|info| {
                    info.worktrees
                        .iter()
                        .find(|worktree| worktree.id.as_str() == target.id)
                        .map(|worktree| Item::Worktree(worktree.clone(), info.project.clone()))
                }),
                "session" => self
                    .snapshot
                    .sessions
                    .iter()
                    .find(|session| session.id.as_str() == target.id)
                    .cloned()
                    .map(Item::Session),
                _ => None,
            })
            .collect()
    }

    // ----- typing --------------------------------------------------------------------

    /// What is typed changed: the names are matched now, the store is read a
    /// moment after the typing stops.
    pub(super) fn palette_changed(&mut self, cx: &mut Context<Self>) {
        self.palette.error = None;
        self.palette.search = None;
        let typed = self.palette.input.read(cx).value().to_string();
        if self.palette.flow.is_some() {
            return self.fill_palette(cx);
        }
        let (scope, query) = parse(&typed);
        let query = query.to_owned();
        self.palette_scope_changed(cx);
        if matches!(scope, Scope::Files | Scope::Text) {
            self.palette.found = Found::default();
            return self.fill_palette(cx);
        }
        if query.is_empty() || !matches!(scope, Scope::All | Scope::History) {
            self.palette.found = Found::default();
            return self.fill_palette(cx);
        }
        // What was found for the earlier text is dropped at once rather than
        // shown beside names that no longer match.
        self.palette.found = Found::default();
        self.fill_palette(cx);

        self.palette.generation += 1;
        let generation = self.palette.generation;
        let store = self.engine.store().clone();
        let debounce = self.options.search_debounce;
        self.palette.search = Some(cx.spawn(async move |this, cx| {
            if !debounce.is_zero() {
                cx.background_executor().timer(debounce).await;
            }
            let found = cx
                .background_spawn(async move { read_found(&store, scope, &query) })
                .await;
            this.update(cx, |this, cx| {
                if this.palette.generation == generation && this.overlay == Overlay::Palette {
                    this.palette.found = found;
                    this.fill_palette(cx);
                }
            })
            .ok();
        }));
    }

    // ----- keys ------------------------------------------------------------------------

    /// The keys of the palette: the arrows walk the rows, skipping headings;
    /// Enter runs the row (Cmd or Ctrl with Enter keeps the palette open);
    /// Tab takes the row's name into the field; Backspace in an empty field
    /// goes back one question. `true` when the key was taken.
    pub(super) fn palette_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = stroke.key.as_str();
        // Alt with C or R flips the switches of the text search.
        if self.palette.flow.is_none()
            && stroke.modifiers.alt
            && !stroke.modifiers.control
            && !stroke.modifiers.platform
            && matches!(key, "c" | "r")
            && parse(&self.palette.input.read(cx).value()).0 == Scope::Text
        {
            if key == "c" {
                self.toggle_search_case(cx);
            } else {
                self.toggle_search_regex(cx);
            }
            return true;
        }
        let modified = stroke.modifiers.alt || stroke.modifiers.function;
        if modified {
            return false;
        }
        let secondary = if crate::platform::is_mac() {
            stroke.modifiers.platform
        } else {
            stroke.modifiers.control
        };
        match key {
            "enter" => {
                self.run_palette_item(self.palette.cursor, secondary, window, cx);
                true
            }
            "down" | "up" | "pagedown" | "pageup" if !secondary => {
                let rows = self.palette.rows();
                if rows.is_empty() {
                    return true;
                }
                let at = rows
                    .iter()
                    .position(|index| *index == self.palette.cursor)
                    .unwrap_or(0);
                self.palette.cursor = match key {
                    "down" => rows[(at + 1) % rows.len()],
                    "up" => rows[(at + rows.len() - 1) % rows.len()],
                    "pagedown" => rows[(at + PAGE).min(rows.len() - 1)],
                    _ => rows[at.saturating_sub(PAGE)],
                };
                self.sync_preview(cx);
                cx.notify();
                true
            }
            "tab" if !secondary && !stroke.modifiers.shift => {
                self.complete_palette(window, cx);
                true
            }
            "backspace"
                if self.palette.flow.is_some()
                    && self.palette.input.read(cx).value().is_empty() =>
            {
                self.palette_back(window, cx)
            }
            _ => false,
        }
    }

    /// Tab: the name of the row the keyboard is on goes into the field.
    fn complete_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name: Option<String> = match self.palette.items.get(self.palette.cursor) {
            Some(Item::Choice(place)) => {
                self.palette
                    .flow
                    .as_ref()
                    .and_then(|flow| match &flow.step.kind {
                        StepKind::Choices { choices, .. } => {
                            choices.get(*place).map(|choice| choice.label.clone())
                        }
                        StepKind::Text { .. } => None,
                    })
            }
            Some(Item::Command(command)) => Some(format!(">{}", keys::label(*command))),
            Some(Item::Look(look)) => Some(format!(">{}", look.label())),
            Some(Item::Setting(def)) => Some(format!(">{}", def.label)),
            Some(Item::Machine(machine)) => Some(format!("@{}", machine.name)),
            Some(Item::Project(project)) => Some(format!("#{}", project.name)),
            Some(Item::Mode(place)) => MODES.get(*place).map(|(prefix, _)| (*prefix).to_owned()),
            Some(Item::File(path, _)) => Some(format!("~{path}")),
            _ => None,
        };
        let Some(name) = name else {
            return;
        };
        self.palette
            .input
            .update(cx, |field, cx| field.set_value(name, window, cx));
        self.palette_changed(cx);
    }

    // ----- running a row -------------------------------------------------------------------

    /// Runs the row at `index`. With `stay`, a command leaves the palette open.
    pub(super) fn run_palette_item(
        &mut self,
        index: usize,
        stay: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A text question takes what was typed, whatever row the cursor is on.
        if let Some(Flow {
            step:
                Step {
                    kind: StepKind::Text { validate, .. },
                    ..
                },
            ..
        }) = &self.palette.flow
        {
            let typed = self.palette.input.read(cx).value().to_string();
            match steps::validate(*validate, &typed) {
                Ok(value) => self.answer(value, window, cx),
                Err(why) => {
                    self.palette.error = Some(why);
                    self.fill_palette(cx);
                }
            }
            return;
        }
        let Some(item) = self.palette.items.get(index).cloned() else {
            return;
        };
        match item {
            Item::Section(_) | Item::Line(..) | Item::FileHeading(_) => {}
            Item::File(path, line) => {
                let target = self.palette.files.target.clone();
                self.close_palette(window, cx);
                self.open_found(target, &path, line, window, cx);
            }
            Item::Match(hit) => {
                let target = self.palette.text.target.clone();
                self.close_palette(window, cx);
                self.open_found(target, &hit.path, Some(hit.line), window, cx);
            }
            Item::Choice(place) => {
                let value = self
                    .palette
                    .flow
                    .as_ref()
                    .and_then(|flow| match &flow.step.kind {
                        StepKind::Choices { choices, .. } => choices
                            .get(place)
                            .map(|choice: &Choice| choice.value.clone()),
                        StepKind::Text { .. } => None,
                    });
                if let Some(value) = value {
                    self.answer(value, window, cx);
                }
            }
            Item::Custom(text) => self.answer(text, window, cx),
            Item::Mode(place) => {
                if let Some((prefix, _)) = MODES.get(place) {
                    let prefix = (*prefix).to_owned();
                    self.palette
                        .input
                        .update(cx, |field, cx| field.set_value(prefix, window, cx));
                    self.palette_changed(cx);
                }
            }
            Item::Command(command) => {
                self.palette.usage.note_command(&command_name(command));
                self.palette.save_usage();
                if steps::is_flow(command) {
                    self.begin_flow(command, window, cx);
                } else if stay {
                    self.run_command(command, window, cx);
                    self.fill_palette(cx);
                } else {
                    self.close_palette(window, cx);
                    self.run_command(command, window, cx);
                }
            }
            Item::Setting(def) => {
                self.begin_setting_flow(def, window, cx);
            }
            Item::Look(look) => {
                // Keeping it ends the preview and saves the choice.
                self.close_palette(window, cx);
                match look {
                    Look::Theme(id) => self.apply(Action::SetThemeId(id), window, cx),
                    Look::Appearance(choice) => {
                        self.apply(Action::SetAppearance(choice), window, cx)
                    }
                }
            }
            Item::Machine(machine) => {
                self.remember(Target::new("machine", machine.id.as_str()));
                self.close_palette(window, cx);
                self.show(&NodeId::Machine(machine.id.clone()));
                self.pane = Pane::Sidebar;
            }
            Item::Project(project) => {
                self.remember(Target::new("project", project.id.as_str()));
                self.close_palette(window, cx);
                self.open_project(&project.id, cx);
            }
            Item::Worktree(worktree, project) => {
                self.remember(Target::new("worktree", worktree.id.as_str()));
                self.close_palette(window, cx);
                self.open_worktree(&project.id, &worktree.id, cx);
            }
            // A session found by its title is opened as the tree opens it: it
            // is resumed in a terminal.
            Item::Session(session) => {
                self.remember(Target::new("session", session.id.as_str()));
                self.close_palette(window, cx);
                self.open_history(session, window, cx);
            }
            // A hit inside a message shows that message, in the stored
            // transcript; a hit on the title is the session itself.
            Item::Hit(hit) => {
                let hit = *hit;
                self.remember(Target::new("session", hit.session.id.as_str()));
                self.close_palette(window, cx);
                match hit.seq {
                    Some(_) => self.open_session(hit.session, hit.seq, cx),
                    None => self.open_history(hit.session, window, cx),
                }
            }
        }
    }

    fn remember(&mut self, target: Target) {
        self.palette.usage.note_target(target);
        self.palette.save_usage();
    }

    /// What a finished flow does.
    pub(super) fn apply(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::PickCloneParent { url, name } => self.pick_clone_parent(url, name, window, cx),
            Action::PickProjectParent { name } => self.pick_project_parent(name, window, cx),
            Action::Engine(crate::engine::Op::AddWorktree {
                project,
                branch,
                base,
            }) => {
                // A new worktree always gets a session: the default agent
                // when the settings name one that is installed there, else
                // a shell.
                let machine = self
                    .snapshot
                    .project(&project)
                    .map(|entry| entry.project.machine_id.clone())
                    .unwrap_or_else(leon_core::MachineId::local);
                let launch = self.new_worktree_launch(&machine, cx);
                self.engine.submit(crate::engine::Op::AddWorktree {
                    project: project.clone(),
                    branch: branch.clone(),
                    base,
                });
                self.start_when_worktree_appears(project, machine, branch, launch, window, cx);
            }
            Action::Engine(op) => self.engine.submit(op),
            Action::StartSession(intent) => self.start_intent(intent, window, cx),
            Action::CloseLive(id) => self.forget_live(id, window, cx),
            Action::OpenFile(path) => self.open_typed_file(&path, window, cx),
            Action::SaveFile(id) => self.save_file(id, false, window, cx),
            Action::SaveAndCloseFile(id) => self.save_file(id, true, window, cx),
            Action::CloseFile(id) => self.close_file(id, window, cx),
            Action::OverwriteFile(id) => self.overwrite_file(id, window, cx),
            Action::ReloadFile(id) => self.reload_file(id, window, cx),
            Action::SleepLive(id) => self.sleep_live(id, window, cx),
            Action::RemoveWorktree {
                project,
                worktree,
                force,
            } => self.remove_worktree(project, worktree, force, window, cx),
            Action::ResumeSession(session) => {
                let found = self
                    .snapshot
                    .sessions
                    .iter()
                    .find(|candidate| candidate.id == session)
                    .cloned();
                if let Some(found) = found {
                    self.open_history(found, window, cx);
                }
            }
            Action::OpenTranscript => self.open_transcript_noting_holder(cx),
            Action::ResumeIn(session, cwd) => {
                let found = self
                    .snapshot
                    .sessions
                    .iter()
                    .find(|candidate| candidate.id == session)
                    .cloned();
                if let Some(found) = found {
                    self.resume_in(found, cwd, window, cx);
                }
            }
            Action::ResumeAnyway(session) => {
                let found = self
                    .snapshot
                    .sessions
                    .iter()
                    .find(|candidate| candidate.id == session)
                    .cloned();
                if let Some(found) = found {
                    self.resume_anyway(found, window, cx);
                }
            }
            Action::TakeOver(session) => {
                let found = self
                    .snapshot
                    .sessions
                    .iter()
                    .find(|candidate| candidate.id == session)
                    .cloned();
                if let Some(found) = found {
                    self.take_over(found, window, cx);
                }
            }
            Action::RenameLive(id, name) => {
                self.rename_live(id, &name, cx);
            }
            Action::RenameSession(session, live, name) => {
                self.engine.submit(crate::engine::Op::RenameSession {
                    session,
                    name: name.clone(),
                });
                if let Some(id) = live {
                    self.rename_live(id, &name, cx);
                }
            }
            Action::SetAppearance(choice) => crate::settings::set_appearance(cx, choice),
            Action::SetThemeId(id) => match id.problem() {
                Some(reason) => self.engine.report(
                    crate::engine::StatusKind::Error,
                    format!("Theme {} is invalid: {reason}", id.name()),
                ),
                None => crate::settings::set_theme_id(cx, id),
            },
            Action::Quit => self.quit_now(cx),
            Action::SaveAllAndQuit => self.save_all_and_quit(window, cx),
            Action::DiscardAndQuit => self.discard_and_quit(cx),
            Action::RestartToUpdate => self.restart_now(cx),
            Action::NewTheme(name) => self.create_theme(&name, cx),
            Action::SetDen(id) => self.den_choose(&id, cx),
            Action::SaveDenAs(name) => self.den_save_as(&name, cx),
            Action::RenameDen(name) => self.den_rename(&name, cx),
            Action::DeleteDen => self.den_delete(cx),
            Action::AddAgent {
                name,
                command,
                args,
                resume_args,
            } => {
                match crate::settings::add_custom_agent(cx, &name, &command, &args, &resume_args) {
                    Ok(_) => self.engine.report(
                        crate::engine::StatusKind::Info,
                        format!("{name} is now an agent: New agent session offers it."),
                    ),
                    Err(error) => self
                        .engine
                        .report(crate::engine::StatusKind::Error, error.to_string()),
                }
            }
            Action::RemoveAgent(agent) => {
                crate::settings::remove_custom_agent(cx, agent);
                self.engine.report(
                    crate::engine::StatusKind::Info,
                    format!("{} was removed from your agents.", agent.name()),
                );
            }
            Action::SetScale(step) => {
                crate::settings::update(cx, |settings| settings.interface_scale = step)
            }
            Action::SetSetting(key, value) => {
                if let Some(def) = schema::find(key) {
                    self.choose_setting(def, value, cx);
                }
            }
            Action::Button(key) => self.run_setting_action(key, window, cx),
            Action::Nothing => {}
        }
        self.focus.focus(window, cx);
        self.sync_focus(window, cx);
        cx.notify();
    }

    // ----- drawing ---------------------------------------------------------------------------

    /// The palette card.
    pub(super) fn render_palette(
        &self,
        colours: &Colours,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let flow = self.palette.flow.as_ref();
        let world = self.palette.world.as_ref();
        let mut list = div()
            .id("palette-list")
            .debug_selector(|| "palette-list".into())
            .max_h(
                (self.viewport.height - self.palette_top() - px(150.))
                    .min(px(440.))
                    .max(px(120.)),
            )
            .overflow_y_scroll()
            .p_1()
            .flex()
            .flex_col();
        for (index, item) in self.palette.items.iter().enumerate() {
            let (glyph, title, detail, keys_text): (
                IconName,
                Option<SharedString>,
                SharedString,
                Option<String>,
            ) = match item {
                Item::Section(title) => {
                    list = list.child(
                        div()
                            .debug_selector(move || format!("palette-section-{index}"))
                            .px_2()
                            .pt(px(8.))
                            .pb(px(3.))
                            .child(section_label(title, colours)),
                    );
                    continue;
                }
                Item::FileHeading(path) => {
                    let name = path.rsplit('/').next().unwrap_or(path);
                    let dir = path.strip_suffix(name).unwrap_or("").trim_end_matches('/');
                    list = list.child(
                        div()
                            .debug_selector(move || format!("palette-file-{index}"))
                            .h(px(28.))
                            .pl(px(12.))
                            .pr_2()
                            .pt(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .text_size(metrics::TEXT_BODY())
                            .text_color(colours.text)
                            .child(self.file_icon(name, false, false, false, colours, cx))
                            .child(div().flex_none().child(name.to_owned()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(colours.text_faint)
                                    .child(dir.to_owned()),
                            ),
                    );
                    continue;
                }
                Item::Line(name, value) => {
                    let is_error = self.palette.error.as_deref() == Some(name.as_ref());
                    list = list.child(
                        div()
                            .debug_selector(move || format!("palette-line-{index}"))
                            .h(px(30.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .text_size(metrics::TEXT_BODY())
                            .text_color(if is_error {
                                colours.error
                            } else {
                                colours.text_muted
                            })
                            .child(div().min_w_0().truncate().child(name.clone()))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(colours.text_faint)
                                    .child(value.clone()),
                            ),
                    );
                    continue;
                }
                Item::Machine(machine) => (
                    if machine.id.is_local() {
                        IconName::Laptop
                    } else {
                        IconName::Server
                    },
                    Some(machine.name.clone().into()),
                    "machine".into(),
                    None,
                ),
                Item::Project(project) => (
                    IconName::Folder,
                    Some(self.project_label(project).into()),
                    project.root.clone().into(),
                    None,
                ),
                Item::Worktree(worktree, project) => (
                    IconName::GitBranch,
                    Some(
                        format!(
                            "{} / {}",
                            project.name,
                            worktree.branch.as_deref().unwrap_or("detached")
                        )
                        .into(),
                    ),
                    worktree.path.clone().into(),
                    None,
                ),
                Item::Session(session) => (
                    IconName::Terminal,
                    Some(session.title.clone().into()),
                    crate::format::age(chrono::Utc::now(), session.updated_at).into(),
                    None,
                ),
                Item::Hit(hit) => (
                    IconName::Search,
                    None,
                    hit.session.title.clone().into(),
                    None,
                ),
                Item::File(path, line) => {
                    let name = path.rsplit('/').next().unwrap_or(path);
                    let dir = path.strip_suffix(name).unwrap_or("").trim_end_matches('/');
                    (
                        IconName::File,
                        Some(match line {
                            Some(line) => format!("{name}:{line}").into(),
                            None => name.to_owned().into(),
                        }),
                        dir.to_owned().into(),
                        None,
                    )
                }
                Item::Match(hit) => (
                    IconName::Search,
                    None,
                    format!(":{}", hit.line).into(),
                    None,
                ),
                Item::Command(command) => (
                    IconName::ChevronRight,
                    Some(keys::label(*command).into()),
                    keys::binding(*command)
                        .map(|binding| binding.section.title())
                        .unwrap_or_default()
                        .into(),
                    keys::keys_label(*command),
                ),
                Item::Setting(def) => (
                    IconName::Settings,
                    Some(def.label.into()),
                    format!(
                        "{} · {}",
                        def.section.title(),
                        match crate::settings::value_opt(cx, def.key) {
                            Some(value) => def.show(&value),
                            None => "button".to_owned(),
                        }
                    )
                    .into(),
                    None,
                ),
                Item::Look(look) => (
                    if world.is_some_and(|world| look.is_current(world)) {
                        IconName::Check
                    } else {
                        IconName::Circle
                    },
                    Some(look.label().into()),
                    match look {
                        Look::Theme(id) => id.detail().into(),
                        Look::Appearance(_) => "".into(),
                    },
                    None,
                ),
                Item::Choice(place) => match flow.map(|flow| &flow.step.kind) {
                    Some(StepKind::Choices { choices, .. }) => match choices.get(*place) {
                        Some(choice) => (
                            if choice.current {
                                IconName::Check
                            } else {
                                IconName::Circle
                            },
                            Some(choice.label.clone().into()),
                            choice.detail.clone().into(),
                            None,
                        ),
                        None => continue,
                    },
                    _ => continue,
                },
                Item::Custom(text) => (
                    IconName::Plus,
                    Some(format!("Use \"{text}\"").into()),
                    "typed text".into(),
                    None,
                ),
                Item::Mode(place) => match MODES.get(*place) {
                    Some((prefix, what)) => (
                        IconName::Hash,
                        Some((*what).into()),
                        if prefix.is_empty() {
                            "nothing".into()
                        } else {
                            (*prefix).into()
                        },
                        None,
                    ),
                    None => continue,
                },
            };
            // Sessions and the agent choices lead with the agent's logo.
            let lead_agent = match item {
                Item::Session(session) => Some(session.agent),
                Item::Hit(hit) => Some(hit.session.agent),
                Item::Choice(place) => match flow.map(|flow| &flow.step.kind) {
                    Some(StepKind::Choices { choices, .. }) => {
                        choices.get(*place).and_then(|choice| choice.agent)
                    }
                    _ => None,
                },
                _ => None,
            };
            // A theme's choice shows the theme's own page, card, text and
            // accent, in the appearance on screen.
            let swatch = match item {
                Item::Look(Look::Theme(id)) => Some(id.palette(colours.appearance).swatch()),
                Item::Choice(place) => match flow.map(|flow| &flow.step.kind) {
                    Some(StepKind::Choices { choices, .. }) => choices
                        .get(*place)
                        .and_then(|choice| choice.swatch)
                        .map(|id| id.palette(colours.appearance).swatch()),
                    _ => None,
                },
                _ => None,
            };
            let on = index == self.palette.cursor;
            let hover = colours.surface_2;
            let label: gpui_kit::AnyElement = match (title, item) {
                (Some(title), _) => title.into_any_element(),
                (None, Item::Hit(hit)) => snippet_text(&hit.snippet, colours).into_any_element(),
                (None, Item::Match(hit)) => {
                    match_text(&hit.text, self.palette.text.matcher.as_ref(), colours)
                        .into_any_element()
                }
                (None, _) => "".into_any_element(),
            };
            list = list.child(
                div()
                    .id(("palette-item", index))
                    .debug_selector(move || format!("palette-item-{index}"))
                    .relative()
                    .h(px(34.))
                    .pl(px(12.))
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .text_size(metrics::TEXT_BODY())
                    // The row the keyboard is on: raised, with a bar of the
                    // accent on its left.
                    .when(on, |this| {
                        this.bg(colours.surface_2).child(
                            div()
                                .debug_selector(|| "palette-cursor".into())
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(2.))
                                .bg(colours.signal),
                        )
                    })
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.run_palette_item(index, false, window, cx);
                    }))
                    .child(
                        div().flex_none().w(px(18.)).flex().justify_center().child(
                            match lead_agent {
                                Some(agent) => div()
                                    .debug_selector(move || format!("palette-agent-{index}"))
                                    .child(agent_icon(agent, px(15.), colours))
                                    .into_any_element(),
                                None => match item {
                                    Item::File(path, _) => self.file_icon(
                                        path.rsplit('/').next().unwrap_or(path),
                                        false,
                                        false,
                                        false,
                                        colours,
                                        cx,
                                    ),
                                    // A match leads with nothing: its file is the heading.
                                    Item::Match(_) => div().into_any_element(),
                                    // A project leads with its logo.
                                    Item::Project(project) => self.logo(
                                        &project.id,
                                        metrics::PROJECT_ICON(),
                                        &format!("palette-{index}"),
                                        colours,
                                    ),
                                    _ => {
                                        icon(glyph, px(15.), colours.text_muted).into_any_element()
                                    }
                                },
                            },
                        ),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .children(swatch.map(|colours_of_theme| {
                        div()
                            .debug_selector(move || format!("palette-swatch-{index}"))
                            .flex_none()
                            .flex()
                            .border_1()
                            .border_color(colours.elevated_border)
                            .children(
                                colours_of_theme
                                    .into_iter()
                                    .map(|colour| div().w(px(14.)).h(px(14.)).bg(colour)),
                            )
                    }))
                    .when(!detail.is_empty(), |this| {
                        this.child(
                            div()
                                .debug_selector(move || format!("palette-detail-{index}"))
                                .flex_none()
                                .max_w(px(240.))
                                .truncate()
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(colours.text_muted)
                                .child(detail),
                        )
                    })
                    .children(keys_text.map(|text| {
                        key_cap(text, colours)
                            .debug_selector(move || format!("palette-keys-{index}"))
                    })),
            );
        }
        let empty = !self
            .palette
            .items
            .iter()
            .any(|item| item.is_row() || matches!(item, Item::Line(..)));
        let typed = self.palette.input.read(cx).value().to_string();
        let scope = parse(&typed).0;
        let hints = match (flow.map(|flow| &flow.step.kind), scope) {
            (Some(StepKind::Text { .. }), _) => "ENTER CONTINUE   ESC BACK",
            (Some(_), _) => "ENTER CHOOSE   TAB COMPLETE   ESC BACK",
            (None, Scope::Commands) => "ENTER RUN   CTRL+ENTER RUN AND STAY   ESC CLOSE",
            (None, Scope::Help) => "ENTER CHOOSE   ESC CLOSE",
            (None, Scope::Files) => "ENTER OPEN   PATH:LINE GOES TO A LINE   ESC CLOSE",
            (None, Scope::Text) => "ENTER OPEN   ALT+C CASE   ALT+R REGEX   ESC CLOSE",
            (None, _) => {
                "ENTER OPEN   > COMMANDS   @ MACHINES   # PROJECTS   / HISTORY   ~ FILES   % TEXT   ? HELP"
            }
        };
        let hints = if crate::platform::is_mac() {
            hints
                .replace("CTRL+ENTER", "CMD+ENTER")
                .replace("ALT+", "OPT+")
        } else {
            hints.to_owned()
        };
        self.card("palette", colours)
            .w(metrics::PALETTE_WIDTH())
            .max_w(self.viewport.width - px(32.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(48.))
                    .px_3()
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(IconName::Search, px(16.), colours.text_muted))
                    // The question being asked, before its answer.
                    .children(flow.map(|flow| {
                        div()
                            .debug_selector(|| "palette-step".into())
                            .flex_none()
                            .max_w(px(280.))
                            .truncate()
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(metrics::RADIUS())
                            .bg(colours.surface_2)
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text)
                            .child(format!(
                                "{} · {}",
                                keys::label(flow.command),
                                flow.step.prompt
                            ))
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            // Small: its 24 px box holds the 20 px line with 2 px
                            // above and below. The default Medium box (32 px
                            // with 8 px above and below) leaves 16 px for the
                            // same line and cuts the letters.
                            .child(
                                Input::new(&self.palette.input)
                                    .with_size(Size::Small)
                                    .appearance(false),
                            ),
                    )
                    .children(
                        (flow.is_none() && scope == Scope::Text)
                            .then(|| self.search_switches(colours, cx)),
                    )
                    .child(key_cap(keys::key_label("escape"), colours)),
            )
            .child(list)
            .when(empty, |this| {
                this.child(
                    div().px_4().pb_4().child(super::lines::empty_frame(
                        "palette-empty",
                        div()
                            .debug_selector(|| "palette-empty".into())
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child("Nothing found."),
                        None,
                        colours,
                    )),
                )
            })
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(colours.border)
                    .child(
                        mono(hints)
                            .debug_selector(|| "palette-hints".into())
                            .text_color(colours.text_muted),
                    ),
            )
    }

    /// The two switches of the text search, beside the field: they flip with a
    /// click or with Alt+C and Alt+R.
    fn search_switches(&self, colours: &Colours, cx: &mut Context<Self>) -> Div {
        let switch = |id: &'static str, label: &'static str, on: bool, regex: bool| {
            let hover = colours.surface_2;
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .flex_none()
                .px(px(6.))
                .py(px(2.))
                .rounded(metrics::RADIUS())
                .cursor_pointer()
                .text_size(metrics::TEXT_SMALL())
                .when(on, |this| {
                    this.bg(colours.surface_2).text_color(colours.text)
                })
                .when(!on, |this| this.text_color(colours.text_muted))
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if regex {
                        this.toggle_search_regex(cx);
                    } else {
                        this.toggle_search_case(cx);
                    }
                }))
                .child(label)
        };
        div()
            .flex_none()
            .flex()
            .gap(px(2.))
            .child(switch(
                "search-case",
                "Aa",
                self.palette.text.case_sensitive,
                false,
            ))
            .child(switch("search-regex", ".*", self.palette.text.regex, true))
    }

    /// How far down the window the palette's top is: a fixed share of the
    /// window's height, so it stays put while its list grows and shrinks.
    pub(super) fn palette_top(&self) -> gpui_kit::Pixels {
        gpui_kit::px((self.viewport.height.as_f32() * 0.16).round())
    }
}

/// Reads from the store what `query` needs, off the UI thread.
fn read_found(store: &Arc<Store>, scope: Scope, query: &str) -> Found {
    match scope {
        Scope::History => Found {
            hits: store
                .search(&SearchQuery {
                    limit: HITS,
                    ..SearchQuery::new(query)
                })
                .unwrap_or_default(),
            sessions: Vec::new(),
        },
        _ => {
            let sessions = store
                .recent_sessions(&SessionFilter::default(), TITLE_POOL)
                .unwrap_or_default();
            Found {
                sessions: rank(query, sessions, |session| session.title.clone())
                    .into_iter()
                    .take(PER_KIND)
                    .collect(),
                hits: Vec::new(),
            }
        }
    }
}

fn machine_items(world: Option<&World>, query: &str, limit: usize) -> Vec<Item> {
    let Some(world) = world else {
        return Vec::new();
    };
    rank(query, world.machines.clone(), |machine| {
        machine.name.clone()
    })
    .into_iter()
    .take(limit)
    .map(Item::Machine)
    .collect()
}

fn project_items(world: Option<&World>, query: &str, limit: usize) -> Vec<Item> {
    let Some(world) = world else {
        return Vec::new();
    };
    let projects: Vec<Project> = world
        .projects
        .iter()
        .map(|info| info.project.clone())
        .collect();
    rank(query, projects, |project| project.name.clone())
        .into_iter()
        .take(limit)
        .map(Item::Project)
        .collect()
}

fn worktree_items(world: Option<&World>, query: &str, limit: usize) -> Vec<Item> {
    let Some(world) = world else {
        return Vec::new();
    };
    let all: Vec<(Worktree, Project)> = world
        .projects
        .iter()
        .flat_map(|info| {
            info.worktrees
                .iter()
                .map(|worktree| (worktree.clone(), info.project.clone()))
        })
        .collect();
    rank(query, all, |(worktree, project)| {
        format!(
            "{} {}",
            project.name,
            worktree.branch.as_deref().unwrap_or("detached")
        )
    })
    .into_iter()
    .take(limit)
    .map(|(worktree, project)| Item::Worktree(worktree, project))
    .collect()
}

/// A matching line with what matched drawn bold and bright, its indent
/// dropped.
fn match_text(
    line: &str,
    matcher: Option<&crate::search::Matcher>,
    colours: &Colours,
) -> StyledText {
    let text = line.trim_start();
    let highlights = matcher
        .map(|matcher| matcher.ranges(text))
        .unwrap_or_default()
        .into_iter()
        .map(|range| {
            (
                range,
                HighlightStyle {
                    color: Some(colours.text),
                    font_weight: Some(FontWeight::BOLD),
                    background_color: Some(colours.border),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect::<Vec<_>>();
    StyledText::new(text.to_owned()).with_highlights(highlights)
}

/// A snippet with the words that matched drawn bold and bright.
fn snippet_text(snippet: &str, colours: &Colours) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();
    for (matched, part) in snippet_segments(snippet) {
        let start = text.len();
        text.push_str(part);
        if matched {
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: Some(colours.text),
                    font_weight: Some(FontWeight::BOLD),
                    background_color: Some(colours.border),
                    ..HighlightStyle::default()
                },
            ));
        }
    }
    // A newline is one byte and so is the space that replaces it: the ranges
    // stay valid.
    StyledText::new(text.replace('\n', " ")).with_highlights(highlights)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_says_what_to_look_through() {
        assert_eq!(parse("api"), (Scope::All, "api"));
        assert_eq!(parse("  > new wo"), (Scope::Commands, "new wo"));
        assert_eq!(parse("@box"), (Scope::Machines, "box"));
        assert_eq!(parse("#api"), (Scope::Projects, "api"));
        assert_eq!(parse("/ migrate users"), (Scope::History, "migrate users"));
        assert_eq!(parse("~ src/main.rs:12"), (Scope::Files, "src/main.rs:12"));
        assert_eq!(parse("%needle"), (Scope::Text, "needle"));
        assert_eq!(parse("?"), (Scope::Help, ""));
        assert_eq!(parse(""), (Scope::All, ""));
    }

    #[test]
    fn every_prefix_is_listed_by_the_help() {
        let prefixes: Vec<&str> = MODES.iter().map(|(prefix, _)| *prefix).collect();
        assert_eq!(prefixes, ["", ">", "@", "#", "/", "~", "%", "?"]);
    }

    #[test]
    fn rows_are_the_items_the_keyboard_can_stop_on() {
        assert!(!Item::Section("x").is_row());
        assert!(!Item::Line("a".into(), "b".into()).is_row());
        assert!(Item::Choice(0).is_row());
        assert!(Item::Custom("x".into()).is_row());
        assert!(Item::Command(Command::Refresh).is_row());
        assert!(Item::Look(Look::Theme(ThemeId::Leon)).is_row());
    }
}

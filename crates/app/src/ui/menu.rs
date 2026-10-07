//! The context menu of the tree.
//!
//! Right-click on a row, or `Shift+F10`, the menu key or `m` while the tree
//! has the keyboard, opens a menu anchored at the row. What is in it depends
//! on what the row is ([`items_for`], a pure function); every item is a
//! [`Command`] of the registry, so its shortcut chip is read from there and
//! the palette and the shortcuts sheet list the same commands. The menu acts
//! on the row it was opened on: opening it moves the tree's cursor there, and
//! the command then does what it does for the cursor, whether it was picked
//! from the menu, the palette or with its own keys.
//!
//! Keys: `j` and `k` or the arrows move, `Enter` or the right arrow chooses
//! (the right arrow only opens a submenu or chooses, like Enter), the left
//! arrow closes a submenu, `Escape` closes the menu, and letters jump to the
//! first item that starts with what was typed (a space is part of the word).
//! Once a word has been started, `j` and `k` are letters of it until a pause.

use super::shell::{Overlay, Pane, Shell};
use super::tree::{Kind, NodeId, Row};
use super::widgets::{key_cap, mono};
use crate::format;
use crate::icons::agent_icon;
use crate::keys::{self, Command};
use crate::theme::{metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{div, point, Context, Div, MouseButton, Pixels, Point, Window};
use leon_core::AgentId;
use std::time::{Duration, Instant};

/// How long a pause ends what was typed to select an item.
const TYPE_PAUSE: Duration = Duration::from_millis(900);

/// The width of the menu.
const WIDTH: f32 = 264.0;
/// The height of one item.
const ITEM: f32 = 30.0;

/// One line of the menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What it says.
    pub label: String,
    /// The command it runs; its chord is the one shown.
    pub command: Command,
    /// For "new agent session": the agent to start.
    pub agent: Option<AgentId>,
    /// A submenu, when it has one.
    pub children: Vec<Item>,
}

impl Item {
    fn new(label: &str, command: Command) -> Self {
        Self {
            label: label.to_owned(),
            command,
            agent: None,
            children: Vec::new(),
        }
    }

    /// "New agent session", with one entry per agent.
    fn new_session() -> Self {
        Self {
            label: "New agent session".to_owned(),
            command: Command::NewSession,
            agent: None,
            // The agents whose history Leon reads, and the user's own; the
            // whole catalogue (with what is installed first) is the item
            // itself, which asks in the palette.
            children: leon_core::agent::all()
                .iter()
                .filter(|spec| spec.history.is_some() || spec.custom)
                .map(|spec| Self {
                    label: format::agent_name(spec.id).to_owned(),
                    command: Command::NewSession,
                    agent: Some(spec.id),
                    children: Vec::new(),
                })
                .collect(),
        }
    }
}

/// The items for a row. `local` is whether the row is on this computer (the
/// file manager and the machine's removal depend on it).
#[cfg(test)]
pub fn items_for(kind: &Kind, local: bool) -> Vec<Item> {
    items_for_held(kind, local, None)
}

/// [`items_for`], for a session a process in another terminal may hold:
/// `held` is `None` when none does, else whether the terminal's application
/// can be brought forward. Such a session offers to resume here anyway
/// (and to reveal the terminal), the same choices the palette has.
pub fn items_for_held(kind: &Kind, local: bool, held: Option<bool>) -> Vec<Item> {
    use Command as C;
    let reveal = || local.then(|| Item::new("Reveal in file manager", C::Reveal));
    match kind {
        Kind::Machine(_) => [
            Some(Item::new("Open project…", C::OpenProject)),
            Some(Item::new("New shell", C::OpenShell)),
            Some(Item::new("Probe", C::ProbeMachine)),
            (!local).then(|| Item::new("Why is it offline?", C::WhyOffline)),
            (!local).then(|| Item::new("Edit machine…", C::EditMachine)),
            Some(Item::new("Rename", C::Rename)),
            Some(Item::new("Connect a machine…", C::AddMachine)),
            (!local).then(|| Item::new("Remove machine", C::RemoveMachine)),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Kind::Project { .. } => [
            Some(Item::new("New worktree…", C::NewWorktree)),
            Some(Item::new_session()),
            Some(Item::new("Open shell here", C::OpenShell)),
            Some(Item::new("Copy path", C::CopyPath)),
            Some(Item::new("Rename", C::Rename)),
            Some(Item::new("Move up", C::MoveRowUp)),
            Some(Item::new("Move down", C::MoveRowDown)),
            reveal(),
            Some(Item::new("Refresh icon", C::RefreshIcon)),
            Some(Item::new("Choose icon…", C::ChooseIcon)),
            Some(Item::new("Reset icon", C::ResetIcon)),
            Some(Item::new("Remove project", C::RemoveProject)),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Kind::Worktree { worktree, .. } => [
            Some(Item::new_session()),
            Some(Item::new("Open shell here", C::OpenShell)),
            Some(Item::new("Copy path", C::CopyPath)),
            Some(Item::new("Copy branch name", C::CopyBranch)),
            Some(Item::new("Move up", C::MoveRowUp)),
            Some(Item::new("Move down", C::MoveRowDown)),
            reveal(),
            (!worktree.is_main).then(|| Item::new("Close", C::RemoveWorktree)),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Kind::Session(session) => [
            Some(Item::new("Resume", C::ResumeSession)),
            Some(Item::new("Open transcript", C::OpenTranscript)),
            held.filter(|_| local && cfg!(unix))
                .map(|_| Item::new("Take over…", C::TakeOver)),
            held.map(|_| Item::new("Resume here anyway…", C::ResumeAnyway)),
            held.filter(|can_reveal| *can_reveal)
                .map(|_| Item::new("Reveal the terminal", C::RevealTerminal)),
            Some(Item::new(
                if session.sort_order.is_some() {
                    "Unpin"
                } else {
                    "Pin"
                },
                if session.sort_order.is_some() {
                    C::UnpinSession
                } else {
                    C::PinSession
                },
            )),
            Some(Item::new("Move up", C::MoveRowUp)),
            Some(Item::new("Move down", C::MoveRowDown)),
            Some(Item::new("Rename", C::Rename)),
            Some(Item::new("Copy session id", C::CopySessionId)),
            Some(Item::new("Remove from history", C::RemoveFromHistory)),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Kind::Live(_) => vec![
            Item::new("Focus", C::FocusTerminal),
            Item::new("Split right", C::SplitRight),
            Item::new("Split down", C::SplitDown),
            Item::new("Rename", C::Rename),
            Item::new("Sleep", C::SleepSession),
            Item::new("Close", C::CloseSession),
        ],
        Kind::Folder { .. } => [
            Some(Item::new("Open shell here", C::OpenShell)),
            Some(Item::new("Copy path", C::CopyPath)),
            reveal(),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Kind::Open => vec![Item::new("Open project…", C::OpenProject)],
        Kind::Unsorted { .. } | Kind::More { .. } | Kind::NoMatch | Kind::NoActive => Vec::new(),
    }
}

/// What choosing the selected item does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chosen {
    /// Nothing is selected.
    Nothing,
    /// A submenu opened.
    Submenu,
    /// Run this item.
    Run(Item),
}

/// An open menu.
#[derive(Clone, Debug)]
pub struct Menu {
    /// The row it was opened on.
    pub node: NodeId,
    /// Its items.
    pub items: Vec<Item>,
    /// The selected item.
    pub cursor: usize,
    /// The submenu that is open: the index of its parent item and the
    /// selected entry in it.
    pub sub: Option<(usize, usize)>,
    /// What has been typed to select an item, and when.
    typed: String,
    typed_at: Option<Instant>,
    /// Where it is anchored, in window coordinates: the left end of the
    /// row's bottom edge, or the pointer.
    pub at: Point<Pixels>,
    /// Whether it is the menu of a terminal pane (`node` is then that
    /// terminal) and not of a row of the tree.
    pub terminal: bool,
}

impl Menu {
    /// A menu with its first item selected.
    pub fn new(node: NodeId, items: Vec<Item>, at: Point<Pixels>) -> Self {
        Self {
            node,
            items,
            cursor: 0,
            sub: None,
            typed: String::new(),
            typed_at: None,
            at,
            terminal: false,
        }
    }

    fn list_len(&self) -> usize {
        match self.sub {
            Some((parent, _)) => self.items[parent].children.len(),
            None => self.items.len(),
        }
    }

    fn cursor_mut(&mut self) -> &mut usize {
        match &mut self.sub {
            Some((_, cursor)) => cursor,
            None => &mut self.cursor,
        }
    }

    /// The items of the list that has the cursor.
    pub fn current_list(&self) -> &[Item] {
        match self.sub {
            Some((parent, _)) => &self.items[parent].children,
            None => &self.items,
        }
    }

    /// The place of the cursor in the list that has it.
    pub fn current_cursor(&self) -> usize {
        match self.sub {
            Some((_, cursor)) => cursor,
            None => self.cursor,
        }
    }

    /// Moves the cursor, wrapping round.
    pub fn step(&mut self, delta: isize) {
        let len = self.list_len() as isize;
        if len == 0 {
            return;
        }
        let cursor = self.cursor_mut();
        *cursor = (*cursor as isize + delta).rem_euclid(len) as usize;
    }

    /// Moves the cursor to the first or the last item.
    pub fn edge(&mut self, last: bool) {
        let len = self.list_len();
        *self.cursor_mut() = if last { len.saturating_sub(1) } else { 0 };
    }

    /// Selects the first item whose label starts with what has been typed:
    /// letters typed in quick succession make a word, a pause starts again.
    /// `false` when nothing matches (what was typed is then forgotten).
    pub fn type_char(&mut self, character: char, now: Instant) -> bool {
        let expired = self
            .typed_at
            .is_none_or(|at| now.duration_since(at) > TYPE_PAUSE);
        // A space only means something inside a word.
        if character == ' ' && (self.typed.is_empty() || expired) {
            return false;
        }
        if self
            .typed_at
            .is_none_or(|at| now.duration_since(at) > TYPE_PAUSE)
        {
            self.typed.clear();
        }
        self.typed.extend(character.to_lowercase());
        self.typed_at = Some(now);
        let wanted = self.typed.clone();
        let found = self
            .current_list()
            .iter()
            .position(|item| item.label.to_lowercase().starts_with(&wanted));
        match found {
            Some(at) => {
                *self.cursor_mut() = at;
                true
            }
            None => {
                self.typed.clear();
                false
            }
        }
    }

    /// Whether a word is being typed: letters typed a moment ago and not yet
    /// forgotten. While one is, `j` and `k` are letters, not movement.
    pub fn typing(&self, now: Instant) -> bool {
        !self.typed.is_empty()
            && self
                .typed_at
                .is_some_and(|at| now.duration_since(at) <= TYPE_PAUSE)
    }

    /// Chooses the selected item: opens its submenu or hands it back to run.
    pub fn choose(&mut self) -> Chosen {
        let Some(item) = self.current_list().get(self.current_cursor()).cloned() else {
            return Chosen::Nothing;
        };
        if self.sub.is_none() && !item.children.is_empty() {
            self.sub = Some((self.cursor, 0));
            self.typed.clear();
            return Chosen::Submenu;
        }
        Chosen::Run(item)
    }

    /// Closes the submenu. `false` when none was open.
    pub fn close_sub(&mut self) -> bool {
        self.typed.clear();
        self.sub.take().is_some()
    }
}

impl Shell {
    /// The row to put a menu on and the menu's items.
    fn menu_for(&self, index: usize, at: Point<Pixels>) -> Option<Menu> {
        let row: &Row = self.rows.get(index)?;
        let local = row.machine.is_local();
        let held = match &row.kind {
            Kind::Session(session) => self
                .elsewhere_of(session)
                .map(|found| local && found.app.is_some()),
            _ => None,
        };
        let mut items = items_for_held(&row.kind, local, held);
        // A session another terminal or Leon holds is not resumed from here
        // by default: the transcript comes first, and resuming is "Resume
        // here anyway".
        if let (Some(_), Kind::Session(session)) = (held, &row.kind) {
            if self.live.of_history(&session.id).is_none() {
                items.retain(|item| item.command != Command::ResumeSession);
            }
        }
        // A history session is in one of three states, and its menu starts
        // with what that state does: running, asleep, or history only (which
        // `items_for_held` already is).
        if let Kind::Session(session) = &row.kind {
            if self.live.of_history(&session.id).is_some() {
                // The terminal's row: it is closed from here as a live row
                // is, and removing it from the history would only bring the
                // terminal back as a live row, so that is not offered.
                items.retain(|item| {
                    !matches!(
                        item.command,
                        Command::RemoveFromHistory | Command::ResumeSession | Command::Rename
                    )
                });
                items.splice(
                    0..0,
                    [
                        Item::new("Focus", Command::Open),
                        Item::new("Rename", Command::Rename),
                        Item::new("Sleep", Command::SleepSession),
                        Item::new("Close", Command::CloseSession),
                    ],
                );
            } else if self.slept.contains(&session.id) {
                for item in &mut items {
                    match item.command {
                        Command::ResumeSession => item.label = "Wake".into(),
                        Command::RemoveFromHistory => item.label = "Close".into(),
                        _ => {}
                    }
                }
            }
        }
        (!items.is_empty()).then(|| Menu::new(row.id.clone(), items, at))
    }

    /// The labels of the menu of the row at `index`, for tests.
    #[cfg(test)]
    pub(super) fn menu_for_test(&self, index: usize) -> Vec<String> {
        self.menu_for(index, point(px(0.), px(0.)))
            .map(|menu| menu.items.into_iter().map(|item| item.label).collect())
            .unwrap_or_default()
    }

    /// Opens the menu of the row at `index`, moving the cursor there.
    /// `at` is where it is anchored; the row's bottom edge when absent.
    pub(super) fn open_menu_at(
        &mut self,
        index: usize,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let anchor = at.unwrap_or_else(|| self.row_anchor(index));
        let Some(menu) = self.menu_for(index, anchor) else {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "There is nothing to do with this row.",
            );
            return;
        };
        if self.overlay != Overlay::None {
            self.close_overlay(window, cx);
        }
        self.cursor = Some(index);
        self.pane = Pane::Sidebar;
        self.overlay = Overlay::Menu;
        self.menu = Some(menu);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Opens the menu of the row the cursor is on (the keyboard's way in).
    pub(super) fn open_menu_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay == Overlay::Menu {
            self.close_menu(window, cx);
            return;
        }
        match self.cursor {
            Some(index) => self.open_menu_at(index, None, window, cx),
            None => self
                .engine
                .report(crate::engine::StatusKind::Info, "Select a row first."),
        }
    }

    /// Where a menu opened by the keyboard goes: under the row's label.
    fn row_anchor(&self, index: usize) -> Point<Pixels> {
        let row = &self.rows[index];
        let x = px(12.) + metrics::INDENT() * f32::from(row.depth) + px(24.);
        let bounds = self
            .tree_scroll
            .0
            .borrow()
            .base_handle
            .bounds_for_item(index);
        let y = bounds.map_or(
            metrics::HEADER_HEIGHT() + metrics::ROW_HEIGHT() * (index as f32 + 1.0),
            |bounds| bounds.bottom(),
        );
        point(x, y)
    }

    /// Closes the menu.
    pub(super) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if self.overlay == Overlay::Menu {
            self.overlay = Overlay::None;
        }
        self.focus.focus(window, cx);
        self.sync_focus(window, cx);
        cx.notify();
    }

    /// A keystroke while the menu is open. Bare keys are the menu's whether it
    /// uses them or not, so nothing reaches the tree beneath; chords with
    /// Cmd, Ctrl or Alt go on to the registry. `true` when taken.
    pub(super) fn menu_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let held = &stroke.modifiers;
        if held.platform || held.control || held.alt {
            return false;
        }
        let Some(menu) = self.menu.as_mut() else {
            return false;
        };
        match stroke.key.as_str() {
            "escape" | "f10" | "menu" => {
                if !menu.close_sub() {
                    self.close_menu(window, cx);
                }
            }
            "j" if !menu.typing(Instant::now()) => menu.step(1),
            "k" if !menu.typing(Instant::now()) => menu.step(-1),
            "down" => menu.step(1),
            "up" => menu.step(-1),
            "home" => menu.edge(false),
            "end" => menu.edge(true),
            "left" => {
                menu.close_sub();
            }
            "enter" | "right" => {
                let chosen = menu.choose();
                if let Chosen::Run(item) = chosen {
                    self.run_menu_item(item, window, cx);
                }
            }
            _ => {
                let typed = stroke
                    .key_char
                    .as_deref()
                    .filter(|text| !held.platform && text.chars().count() == 1)
                    .and_then(|text| text.chars().next())
                    .filter(|character| character.is_alphanumeric() || *character == ' ');
                if let Some(character) = typed {
                    menu.type_char(character, Instant::now());
                }
            }
        }
        cx.notify();
        true
    }

    /// Does what a chosen item says, on the row the menu was opened on.
    pub(super) fn run_menu_item(
        &mut self,
        item: Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let node = self.menu.as_ref().map(|menu| menu.node.clone());
        let of_terminal = self.menu.as_ref().is_some_and(|menu| menu.terminal);
        self.close_menu(window, cx);
        if of_terminal {
            // The terminal it was opened on has the keyboard: the command does
            // what it does for it.
            if let Some(NodeId::Live(id)) = node {
                self.open_live(id, window, cx);
            }
            self.run_command(item.command, window, cx);
            return;
        }
        // The cursor is on the row the menu was opened on.
        if let Some(node) = node {
            if let Some(index) = self.rows.iter().position(|row| row.id == node) {
                self.cursor = Some(index);
                self.pane = Pane::Sidebar;
            }
        }
        let kind = self
            .cursor
            .and_then(|index| self.rows.get(index))
            .map(|row| row.kind.clone());
        // The menu knows the row, so the questions that only ask which one
        // are answered: what is left is the confirmation.
        match (item.agent, item.command, kind) {
            (Some(agent), _, _) => self.new_session_with(agent, window, cx),
            (None, Command::RemoveProject, Some(Kind::Project { project, .. })) => self
                .begin_flow_with(
                    Command::RemoveProject,
                    vec![project.id.as_str().to_owned()],
                    window,
                    cx,
                ),
            (None, Command::RemoveWorktree, Some(Kind::Worktree { worktree, .. })) => self
                .begin_flow_with(
                    Command::RemoveWorktree,
                    vec![format!("{}|{}", worktree.project_id, worktree.id)],
                    window,
                    cx,
                ),
            (None, Command::NewWorktree, Some(Kind::Project { project, .. })) => {
                self.open_new_worktree_for(&project.id, window, cx)
            }
            (None, command, _) => {
                self.run_command(command, window, cx);
            }
        }
    }

    /// Starts a session of `agent` where the cursor is, without asking which.
    pub(super) fn new_session_with(
        &mut self,
        agent: AgentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(place) = self.here() else {
            self.engine.report(
                crate::engine::StatusKind::Error,
                "Select a project or a worktree first.",
            );
            return;
        };
        let launch = crate::launch::Launch::Agent {
            kind: agent,
            resume: None,
        };
        self.start_live(
            launch,
            &place.machine,
            &place.cwd,
            super::terminals::Place::Tab,
            None,
            window,
            cx,
        );
    }

    /// The menu, drawn over the window at its anchor.
    pub(super) fn render_menu(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::Stateful<Div>> {
        let menu = self.menu.as_ref()?;
        let list = |items: &[Item],
                    cursor: Option<usize>,
                    left: Pixels,
                    top: Pixels,
                    level: usize,
                    cx: &mut Context<Self>| {
            let height = px(ITEM) * items.len() as f32 + px(8.);
            let (left, top) = self.clamp_menu(left, top, px(WIDTH), height);
            let mut card = self
                .card(
                    if level == 0 {
                        "context-menu"
                    } else {
                        "context-submenu"
                    },
                    colours,
                )
                .absolute()
                .left(left)
                .top(top)
                .w(px(WIDTH))
                .p_1()
                .shadow_md()
                .flex()
                .flex_col();
            for (index, item) in items.iter().enumerate() {
                let on = cursor == Some(index);
                let hover = colours.surface_2;
                let chip = keys::keys_label(item.command)
                    .filter(|_| item.agent.is_none())
                    .map(|text| {
                        div()
                            .debug_selector(move || format!("menu-chip-{level}-{index}"))
                            .child(key_cap(text, colours))
                    });
                let lead = match item.agent {
                    Some(agent) => agent_icon(agent, px(15.), colours).into_any_element(),
                    None => div().w(px(15.)).into_any_element(),
                };
                card = card.child(
                    div()
                        .id(("menu-item", level * 100 + index))
                        .debug_selector(move || format!("menu-item-{level}-{index}"))
                        .relative()
                        .h(px(ITEM))
                        .pl(px(10.))
                        .pr(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .cursor_pointer()
                        .text_size(metrics::TEXT_BODY())
                        .when(on, |this| {
                            this.bg(colours.surface_2).child(
                                div()
                                    .debug_selector(|| "menu-cursor".into())
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(px(2.))
                                    .bg(colours.signal),
                            )
                        })
                        .hover(move |style| style.bg(hover))
                        .on_mouse_move(cx.listener(move |this, _, _, cx| {
                            if let Some(menu) = this.menu.as_mut() {
                                let moved = match (level, menu.sub) {
                                    (0, None) => menu.cursor != index,
                                    (0, Some(_)) => false,
                                    (_, Some((_, current))) => current != index,
                                    _ => false,
                                };
                                if moved {
                                    *menu.cursor_mut() = index;
                                    cx.notify();
                                }
                            }
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            let Some(menu) = this.menu.as_mut() else {
                                return;
                            };
                            if level == 0 {
                                menu.sub = None;
                                menu.cursor = index;
                            } else if let Some((_, current)) = &mut menu.sub {
                                *current = index;
                            }
                            if let Chosen::Run(item) = menu.choose() {
                                this.run_menu_item(item, window, cx);
                            }
                            cx.notify();
                        }))
                        .child(lead)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(item.label.clone()),
                        )
                        .children(chip)
                        .when(!item.children.is_empty(), |this| {
                            this.child(mono("▸").text_color(colours.text_muted))
                        }),
                );
            }
            card
        };
        let mut layer = div()
            .id("menu-layer")
            .debug_selector(|| "menu-layer".into())
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .on_click(cx.listener(|this, _, window, cx| this.close_menu(window, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| this.close_menu(window, cx)),
            );
        let (parent_cursor, sub) = (menu.cursor, menu.sub);
        layer = layer.child(list(
            &menu.items,
            sub.is_none().then_some(parent_cursor),
            menu.at.x,
            menu.at.y,
            0,
            cx,
        ));
        if let Some((parent, cursor)) = sub {
            layer = layer.child(list(
                &menu.items[parent].children,
                Some(cursor),
                menu.at.x + px(WIDTH) - px(4.),
                menu.at.y + px(ITEM) * parent as f32,
                1,
                cx,
            ));
        }
        Some(layer)
    }

    /// Keeps a menu of this size inside the window.
    fn clamp_menu(
        &self,
        left: Pixels,
        top: Pixels,
        width: Pixels,
        height: Pixels,
    ) -> (Pixels, Pixels) {
        let max_left = (self.viewport.width - width - px(8.)).max(px(8.));
        let max_top = (self.viewport.height - height - px(8.)).max(px(8.));
        (left.min(max_left).max(px(8.)), top.min(max_top).max(px(8.)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::live::LiveId;
    use crate::ui::tree::LiveEntry;
    use leon_core::{
        Machine, MachineId, MachineKind, Project, ProjectId, Session, Worktree, WorktreeId,
    };

    fn labels(items: &[Item]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }

    fn machine(local: bool) -> Kind {
        Kind::Machine(Machine {
            id: if local {
                MachineId::local()
            } else {
                MachineId::from_string("m")
            },
            name: "box".into(),
            kind: if local {
                MachineKind::Local
            } else {
                MachineKind::Ssh {
                    host: "h".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                }
            },
        })
    }

    fn worktree(main: bool) -> Kind {
        Kind::Worktree {
            worktree: Worktree {
                id: WorktreeId::from_string("w"),
                project_id: ProjectId::from_string("p"),
                path: "/srv/api".into(),
                branch: Some("main".into()),
                head: None,
                is_main: main,
                merged_pull_request: None,
            },
            sessions: 0,
        }
    }

    fn project() -> Kind {
        Kind::Project {
            project: Project {
                id: ProjectId::from_string("p"),
                machine_id: MachineId::local(),
                name: "api".into(),
                root: "/srv/api".into(),
            },
            sessions: 0,
        }
    }

    #[test]
    fn a_machine_offers_projects_shells_probing_and_renaming_and_removal_unless_it_is_local() {
        assert_eq!(
            labels(&items_for(&machine(false), false)),
            [
                "Open project…",
                "New shell",
                "Probe",
                "Why is it offline?",
                "Edit machine…",
                "Rename",
                "Connect a machine…",
                "Remove machine"
            ]
        );
        assert_eq!(
            labels(&items_for(&machine(true), true)),
            [
                "Open project…",
                "New shell",
                "Probe",
                "Rename",
                "Connect a machine…"
            ],
            "the local machine cannot be removed"
        );
    }

    #[test]
    fn a_project_offers_worktrees_agents_shell_copy_reveal_and_removal() {
        let items = items_for(&project(), true);
        assert_eq!(
            labels(&items),
            [
                "New worktree…",
                "New agent session",
                "Open shell here",
                "Copy path",
                "Rename",
                "Move up",
                "Move down",
                "Reveal in file manager",
                "Refresh icon",
                "Choose icon…",
                "Reset icon",
                "Remove project"
            ]
        );
        assert_eq!(
            labels(&items_for(&project(), false)),
            [
                "New worktree…",
                "New agent session",
                "Open shell here",
                "Copy path",
                "Rename",
                "Move up",
                "Move down",
                "Refresh icon",
                "Choose icon…",
                "Reset icon",
                "Remove project"
            ],
            "revealing is for this computer only"
        );
    }

    #[test]
    fn the_agent_submenu_has_one_entry_per_agent_with_the_agent_to_start() {
        let items = items_for(&project(), true);
        let sub = &items[1].children;
        assert_eq!(labels(sub), ["Claude Code", "Codex", "opencode"]);
        let agents: Vec<_> = sub.iter().map(|item| item.agent).collect();
        assert_eq!(
            agents,
            [
                Some(AgentId::CLAUDE),
                Some(AgentId::CODEX),
                Some(AgentId::OPENCODE)
            ]
        );
        assert!(sub.iter().all(|item| item.command == Command::NewSession));
    }

    #[test]
    fn a_worktree_offers_branch_copy_and_removal_unless_it_is_the_main_one() {
        let linked = items_for(&worktree(false), true);
        assert_eq!(
            labels(&linked),
            [
                "New agent session",
                "Open shell here",
                "Copy path",
                "Copy branch name",
                "Move up",
                "Move down",
                "Reveal in file manager",
                "Close"
            ]
        );
        assert!(!labels(&items_for(&worktree(true), true)).contains(&"Close"));
        assert!(!labels(&items_for(&worktree(false), false)).contains(&"Reveal in file manager"));
    }

    #[test]
    fn a_history_session_offers_open_transcript_id_and_removal() {
        let session = Kind::Session(Session {
            id: leon_core::SessionId::from_string("s"),
            agent: AgentId::CLAUDE,
            external_id: "x".into(),
            machine_id: MachineId::local(),
            project_id: None,
            cwd: "/".into(),
            title: "t".into(),
            model: None,
            started_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            message_count: 0,
            sort_order: None,
        });
        assert_eq!(
            labels(&items_for(&session, true)),
            [
                "Resume",
                "Open transcript",
                "Pin",
                "Move up",
                "Move down",
                "Rename",
                "Copy session id",
                "Remove from history"
            ]
        );
        // A pinned session offers to unpin instead.
        let Kind::Session(mut session) = session else {
            unreachable!()
        };
        session.sort_order = Some(0);
        assert_eq!(
            labels(&items_for(&Kind::Session(session), true)),
            [
                "Resume",
                "Open transcript",
                "Unpin",
                "Move up",
                "Move down",
                "Rename",
                "Copy session id",
                "Remove from history"
            ]
        );
    }

    #[test]
    fn a_live_terminal_offers_focus_splits_rename_and_close() {
        let live = Kind::Live(LiveEntry {
            id: LiveId(1),
            machine: MachineId::local(),
            cwd: "/".into(),
            agent: None,
            history: None,
        });
        assert_eq!(
            labels(&items_for(&live, true)),
            [
                "Focus",
                "Split right",
                "Split down",
                "Rename",
                "Sleep",
                "Close"
            ]
        );
    }

    #[test]
    fn rows_with_nothing_to_do_have_no_menu() {
        assert!(items_for(&Kind::Unsorted { sessions: 1 }, true).is_empty());
        assert!(items_for(&Kind::More { hidden: 3 }, true).is_empty());
    }

    fn menu() -> Menu {
        Menu::new(
            NodeId::Open(MachineId::local()),
            items_for(&machine(false), false),
            point(px(0.), px(0.)),
        )
    }

    #[test]
    fn the_cursor_wraps_and_jumps_to_the_ends() {
        let mut menu = menu();
        menu.step(-1);
        assert_eq!(menu.cursor, 7);
        menu.step(1);
        assert_eq!(menu.cursor, 0);
        menu.edge(true);
        assert_eq!(menu.cursor, 7);
        menu.edge(false);
        assert_eq!(menu.cursor, 0);
    }

    #[test]
    fn typing_selects_the_first_item_that_starts_with_it_and_a_pause_starts_again() {
        let mut menu = menu();
        let start = Instant::now();
        assert!(menu.type_char('r', start));
        assert_eq!(menu.cursor, 5, "Rename comes before Remove machine");
        assert!(menu.type_char('e', start));
        assert!(menu.type_char('m', start));
        assert_eq!(menu.cursor, 7, "rem: Remove machine");
        assert!(!menu.type_char('z', start), "nothing starts with remz");
        // After a pause, a new word: "p" is Probe.
        assert!(menu.type_char('p', start + TYPE_PAUSE * 2));
        assert_eq!(menu.cursor, 2);
    }

    #[test]
    fn choosing_an_item_with_children_opens_its_submenu_and_choosing_inside_runs_the_entry() {
        let items = items_for(&project(), true);
        let mut menu = Menu::new(
            NodeId::Open(MachineId::local()),
            items,
            point(px(0.), px(0.)),
        );
        menu.step(1);
        assert_eq!(menu.choose(), Chosen::Submenu);
        assert_eq!(menu.sub, Some((1, 0)));
        menu.step(1);
        let Chosen::Run(item) = menu.choose() else {
            panic!("expected an entry")
        };
        assert_eq!(item.agent, Some(AgentId::CODEX));
        assert!(menu.close_sub());
        assert!(!menu.close_sub());
        assert_eq!(menu.cursor, 1, "the parent item is still selected");
    }

    #[test]
    fn choosing_a_plain_item_runs_it() {
        let mut menu = menu();
        let Chosen::Run(item) = menu.choose() else {
            panic!()
        };
        assert_eq!(item.command, Command::OpenProject);
    }

    #[test]
    fn every_item_is_a_command_of_the_registry() {
        let kinds = [machine(false), project(), worktree(false)];
        for kind in &kinds {
            for item in items_for(kind, true) {
                assert!(keys::binding(item.command).is_some(), "{}", item.label);
                for child in &item.children {
                    assert!(keys::binding(child.command).is_some());
                }
            }
        }
    }

    #[test]
    fn j_and_k_are_letters_once_a_word_has_been_started() {
        let mut menu = menu();
        let start = Instant::now();
        assert!(!menu.typing(start));
        menu.type_char('r', start);
        assert!(menu.typing(start));
        assert!(
            !menu.typing(start + TYPE_PAUSE * 2),
            "a pause ends the word"
        );
    }

    #[test]
    fn a_space_means_something_only_inside_a_word() {
        let mut menu = Menu::new(
            NodeId::Open(MachineId::local()),
            items_for(&worktree(false), true),
            point(px(0.), px(0.)),
        );
        let start = Instant::now();
        assert!(!menu.type_char(' ', start), "a leading space is ignored");
        for character in "copy b".chars() {
            menu.type_char(character, start);
        }
        assert_eq!(menu.current_list()[menu.cursor].label, "Copy branch name");
    }
}

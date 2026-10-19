//! The board of tasks: the window's part.
//!
//! The board takes the place of the main pane ([`Main::Tasks`]), as the Den
//! does, and the sidebar stays: a worktree made from a card shows up in the
//! tree beside the board. The command that opens it closes it again, as
//! Escape does, and both put back what the main pane showed.
//!
//! Everything the board decides is in `task_board` (the keys, the cursor, what
//! a key or a drop asks for, which board is shown); this file reads the store,
//! hands the plans to the engine and draws.
//!
//! **Reading.** The board is read from the store when it opens, whenever the
//! engine says something changed, and every [`REFRESH`] while it is the main
//! pane: an agent's `leon task` or MCP call runs in another process, and the
//! window's store is never told about what another process wrote.
//!
//! **Writing.** Never here: a moved card, a new task and a new priority are
//! engine operations (`Op::MoveTask`, `Op::AddTask`, `Op::UpdateTask`), as
//! every write of the window is. The title of a new task is asked in the
//! palette, as every question is, and the board has the keyboard back when it
//! is answered.
//!
//! **Dragging.** A card is dragged with the pointer from any column to any
//! place of any column. While a drag goes on the window's sheet takes the
//! pointer (see `Shell::row_drop_listeners`), so no element is hovered: every
//! card and every empty tail of a column reports each move of the drag and
//! answers only when the pointer is inside its own bounds, a line marks where
//! the card would land, and the window-wide release completes the drop from
//! what the moves saw last ([`Shell::finish_card_drop`]). A release outside
//! the columns, or Escape, drops nothing.

use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{
    div, AnyElement, Context, Div, DragMoveEvent, FontWeight, Hsla, Render, SharedString, Stateful,
    Task, Window,
};
use leon_core::{
    store::memory::normalized, MemoryScope, ProjectId, Task as Card, TaskPatch, TaskPriority,
    TaskStatus,
};

use super::home::Nav;
use super::shell::{Main, Pane, Shell};
use super::task_detail::{work_phrase, Work};
use super::widgets::{activity_dot, key_cap, mark, mono, Lion};
use crate::engine::Op;
use crate::icons::{agent_icon, icon, IconName};
use crate::task_board::{self, Action, Board, Which, COLUMNS, PER_COLUMN};
use crate::theme::{metrics, px, Palette};

/// How often the board, while it is the main pane, reads the store again.
pub const REFRESH: Duration = Duration::from_millis(1500);

/// Where a released drag would put the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropAt {
    /// The card dragged.
    pub id: String,
    /// The column it would land in.
    pub column: usize,
    /// Before the card at this place of the column as it is drawn; the
    /// number of cards for the end.
    pub index: usize,
}

/// What the window keeps of the board while it is the main pane.
pub struct TasksUi {
    /// The cards and the cursor.
    pub board: Board,
    /// The board that is shown.
    pub which: Which,
    /// The project of `which`, when it is one the window lists.
    pub project: Option<ProjectId>,
    /// Why it is not the one that may be expected, when it is not.
    pub note: Option<&'static str>,
    /// The person switched to the global board.
    pub want_global: bool,
    /// A question asked from the board ends back in it.
    pub return_from_palette: bool,
    /// The title of a task that was just asked for: the cursor goes to it
    /// when it shows up.
    pub follow_title: Option<String>,
    /// The store could not be read.
    pub error: Option<String>,
    /// What the main pane showed before, to put back.
    pub back: Option<(Main, Pane)>,
    /// Where a released drag would land.
    pub drop: Option<DropAt>,
    /// The card being dragged.
    pub dragging: Option<String>,
    /// The details of a card, over the right edge of the board.
    pub detail: Option<super::task_detail::DetailUi>,
    /// The panel for a new task, in the same place.
    pub create: Option<super::task_create::CreateUi>,
    /// How the project is synced with GitHub, and which tasks are linked.
    pub sync: super::task_github::SyncState,
    /// The panel that asks before sync is turned on, or says how it stands.
    pub sync_panel: Option<super::task_github::SyncPanel>,
    /// Turns of the board's refresh since the last sync it asked for.
    pub sync_ticks: u32,
    /// How many turns pass between the syncs the open board asks for.
    pub sync_every: u32,
    /// The loop that reads again while the board is open.
    refresher: Option<Task<()>>,
}

impl TasksUi {
    /// Stops the loop that reads again: the board is not the main pane any
    /// more.
    pub(super) fn refresher_stop(&mut self) {
        self.refresher = None;
    }
}

impl Default for TasksUi {
    fn default() -> Self {
        Self {
            board: Board::new(),
            which: Which::Global,
            project: None,
            note: None,
            want_global: false,
            return_from_palette: false,
            follow_title: None,
            error: None,
            back: None,
            drop: None,
            dragging: None,
            detail: None,
            create: None,
            sync: super::task_github::SyncState::default(),
            sync_panel: None,
            sync_ticks: 0,
            sync_every: super::task_github::SYNC_EVERY_TICKS,
            refresher: None,
        }
    }
}

/// What is dragged: a card, by its id.
#[derive(Clone)]
struct CardDrag {
    id: String,
}

/// The card as it follows the pointer: a lifted copy of the real one.
struct CardGhost {
    title: String,
    meta: String,
    stripe: Option<Hsla>,
    palette: Palette,
}

impl Render for CardGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let c = self.palette;
        div()
            .w(px(240.))
            .flex()
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(c.signal)
            .bg(c.surface_2)
            .shadow_md()
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .w(px(3.))
                    .bg(self.stripe.unwrap_or(c.elevated_border)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(metrics::TEXT_BODY())
                            .text_color(c.text)
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(c.text_muted)
                            .child(self.meta.clone()),
                    ),
            )
    }
}

/// The colour a column is told by: its pip, its count and the wash of its
/// lane.
fn status_colour(status: TaskStatus, c: &Palette) -> Hsla {
    match status {
        TaskStatus::Backlog => c.text_muted,
        TaskStatus::Todo => c.info,
        TaskStatus::Doing => c.success,
        TaskStatus::Review => c.warning,
        TaskStatus::Done | TaskStatus::Canceled => c.text_faint,
    }
}

/// The stripe of a card of this priority, when it has one.
fn priority_colour(priority: TaskPriority, c: &Palette) -> Option<Hsla> {
    match priority {
        TaskPriority::Urgent => Some(c.error),
        TaskPriority::High => Some(c.warning),
        TaskPriority::Medium => Some(c.info),
        TaskPriority::Low => Some(c.text_faint),
        TaskPriority::None => None,
    }
}

/// The line under a card's title: its id, its priority and who it is for.
fn meta_of(task: &Card) -> String {
    let mut parts: Vec<String> = vec![task.short_id().to_owned()];
    if task.priority != TaskPriority::None {
        parts.push(task.priority.as_str().to_owned());
    }
    if let Some(assignee) = &task.assignee {
        parts.push(format!("@{assignee}"));
    }
    parts.join("  ")
}

impl Shell {
    /// Whether the board is what the main pane shows.
    pub(super) fn tasks_open(&self) -> bool {
        matches!(self.main, Main::Tasks)
    }

    /// Opens the board, if it is not open: what `--tasks` asks at the start.
    pub fn show_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tasks_open() {
            self.toggle_tasks(window, cx);
        }
    }

    /// Opens the board, or closes it when it is open.
    pub(super) fn toggle_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tasks_open() {
            self.close_tasks(window, cx);
            return;
        }
        self.close_overlay(window, cx);
        let (which, project, note) = self.tasks_pick();
        // What goes back when it closes: the main pane as it was, or under
        // the Den when that is what it was showing.
        let back = if self.den_open() {
            let back = self.den_take_back();
            self.den_leave(cx);
            back.unwrap_or((Main::Empty, Pane::Sidebar))
        } else {
            (std::mem::replace(&mut self.main, Main::Empty), self.pane)
        };
        self.main = Main::Tasks;
        self.tasks_ui.back = Some(back);
        self.tasks_ui.which = which;
        self.tasks_ui.project = project;
        self.tasks_ui.note = note;
        self.tasks_ui.error = None;
        self.tasks_ui.drop = None;
        self.tasks_ui.dragging = None;
        self.tasks_ui.detail = None;
        self.tasks_ui.create = None;
        self.home.nav = Some(Nav::Tasks);
        self.pane = Pane::Main;
        self.tasks_reload();
        self.tasks_ui.sync_panel = None;
        self.tasks_ui.sync_ticks = 0;
        self.sync_now(cx);
        self.tasks_keep_fresh(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Closes the board and puts back what the main pane showed.
    pub(super) fn close_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tasks_open() {
            return;
        }
        // What was being typed is saved, not lost.
        self.save_detail(cx);
        self.tasks_ui.detail = None;
        self.tasks_ui.create = None;
        self.tasks_ui.sync_panel = None;
        self.tasks_ui.refresher = None;
        self.tasks_ui.follow_title = None;
        self.tasks_ui.drop = None;
        self.tasks_ui.dragging = None;
        let (main, pane) = self
            .tasks_ui
            .back
            .take()
            .unwrap_or((Main::Empty, Pane::Sidebar));
        match main {
            // A terminal takes the keyboard back as when it is opened.
            Main::Live(id) if self.live.get(id).is_some() => {
                self.main = Main::Empty;
                self.open_live(id, window, cx);
                self.pane = pane;
            }
            Main::Live(_) | Main::Den | Main::Tasks | Main::King => {
                self.main = Main::Empty;
                self.pane = Pane::Sidebar;
            }
            other => {
                self.main = other;
                self.pane = pane;
            }
        }
        self.home.nav = matches!(self.main, Main::Empty).then_some(Nav::Home);
        self.sync_focus(window, cx);
        cx.notify();
    }

    /// The board to show on opening, with the project it belongs to: the one
    /// the window is on, else the one that was shown last, else the first
    /// project of this computer, else the global one.
    fn tasks_pick(&self) -> (Which, Option<ProjectId>, Option<&'static str>) {
        let here = self.here();
        let local = here.as_ref().is_none_or(|here| here.machine.is_local());
        let mut root = here.as_ref().and_then(|here| match &here.project {
            Some(id) => self
                .snapshot
                .project(id)
                .map(|entry| entry.project.root.clone()),
            None => self
                .engine
                .store()
                .memory_root(&here.machine, &here.cwd)
                .ok()
                .flatten(),
        });
        if local && root.is_none() {
            root = match &self.tasks_ui.which {
                Which::Project(last) => Some(last.clone()),
                Which::Global => None,
            }
            .filter(|last| self.local_project(last).is_some())
            .or_else(|| {
                self.board_projects()
                    .first()
                    .map(|(_, _, root)| root.clone())
            });
        }
        let (which, note) = task_board::choose(local, root, self.tasks_ui.want_global);
        let project = match &which {
            Which::Project(root) => self.local_project(root).map(|(id, _)| id),
            Which::Global => None,
        };
        (which, project, note)
    }

    /// The projects of this computer the board can show: id, name and root.
    pub(super) fn board_projects(&self) -> Vec<(ProjectId, String, String)> {
        self.snapshot
            .projects
            .iter()
            .filter(|entry| entry.project.machine_id.is_local())
            .map(|entry| {
                (
                    entry.project.id.clone(),
                    self.project_label(&entry.project),
                    entry.project.root.clone(),
                )
            })
            .collect()
    }

    /// The project of this computer with this root.
    fn local_project(&self, root: &str) -> Option<(ProjectId, String)> {
        let key = leon_core::path::key(root);
        self.snapshot
            .projects
            .iter()
            .find(|entry| {
                entry.project.machine_id.is_local()
                    && leon_core::path::key(&entry.project.root) == key
            })
            .map(|entry| (entry.project.id.clone(), entry.project.root.clone()))
    }

    /// Shows another board: a project's, or the global one.
    pub(super) fn tasks_show(&mut self, which: Which) {
        self.tasks_ui.want_global = which == Which::Global;
        self.tasks_ui.project = match &which {
            Which::Project(root) => self.local_project(root).map(|(id, _)| id),
            Which::Global => None,
        };
        self.tasks_ui.which = which;
        self.tasks_ui.note = None;
        self.tasks_ui.drop = None;
        self.tasks_reload();
    }

    /// Reads the board again while it is the main pane.
    fn tasks_keep_fresh(&mut self, cx: &mut Context<Self>) {
        self.tasks_ui.refresher = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(REFRESH).await;
            let open = this
                .update(cx, |this, cx| {
                    if !this.tasks_open() {
                        return false;
                    }
                    if this.tasks_reload() {
                        cx.notify();
                    }
                    // The board syncs by itself now and then.
                    this.tasks_ui.sync_ticks += 1;
                    if this.tasks_ui.sync_ticks >= this.tasks_ui.sync_every {
                        this.tasks_ui.sync_ticks = 0;
                        this.sync_now(cx);
                    }
                    true
                })
                .unwrap_or(false);
            if !open {
                return;
            }
        }));
    }

    fn tasks_scope(&self) -> MemoryScope {
        match &self.tasks_ui.which {
            Which::Project(root) => MemoryScope::project(root.clone()),
            Which::Global => MemoryScope::Global,
        }
    }

    /// Reads the board again. `true` when what is shown changed.
    pub(super) fn tasks_reload(&mut self) -> bool {
        let scope = self.tasks_scope();
        match crate::tasks::load_board(self.engine.store(), &scope, PER_COLUMN) {
            Ok(tasks) => {
                if let Some(title) = &self.tasks_ui.follow_title {
                    let wanted = normalized(title);
                    if let Some(task) = tasks.iter().find(|task| normalized(&task.title) == wanted)
                    {
                        self.tasks_ui.board.follow(task.id.clone());
                        self.tasks_ui.follow_title = None;
                    }
                }
                let before = self.tasks_ui.board.clone();
                self.tasks_ui.board.set_tasks(tasks);
                let had_error = self.tasks_ui.error.take().is_some();
                self.tasks_load_sync(&scope);
                self.detail_reloaded();
                before != self.tasks_ui.board || had_error
            }
            Err(error) => {
                let said = crate::tasks::sentence(&error);
                let changed = self.tasks_ui.error.as_deref() != Some(said.as_str());
                self.tasks_ui.error = Some(said);
                changed
            }
        }
    }

    /// The questions of the palette come back to the board.
    pub(super) fn return_to_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tasks_open() {
            self.pane = Pane::Main;
            self.tasks_reload();
            self.sync_focus(window, cx);
        }
    }

    /// Creates a task with the answer of the palette: in the column the
    /// cursor is in while the board is open, else in the one of tasks to do.
    pub(super) fn new_task(&mut self, title: &str, cx: &mut Context<Self>) {
        let open = self.tasks_open();
        let (which, status) = if open {
            (self.tasks_ui.which.clone(), self.tasks_ui.board.status())
        } else {
            (self.tasks_pick().0, TaskStatus::Todo)
        };
        let root = match which {
            Which::Project(root) => Some(root),
            Which::Global => None,
        };
        if open {
            self.tasks_ui.follow_title = Some(title.to_owned());
        }
        self.engine.submit(Op::AddTask {
            root,
            title: title.to_owned(),
            status,
            body: String::new(),
            priority: None,
            assignee: None,
            labels: Vec::new(),
        });
        cx.notify();
    }

    /// A key while the board is the main pane and has the keyboard. `true`
    /// when it was the board's.
    pub(super) fn tasks_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &stroke.modifiers;
        let other = m.platform || m.control || m.alt;
        if stroke.key == "escape" && !other && !m.shift {
            // The first Escape lets go of a drag, the next closes the
            // details, the last leaves the board.
            if self.tasks_ui.drop.is_some() || self.tasks_ui.dragging.is_some() {
                self.tasks_ui.drop = None;
                self.tasks_ui.dragging = None;
                cx.notify();
            } else if self.tasks_ui.sync_panel.is_some() {
                self.close_sync_panel(cx);
            } else if self.create_open() {
                self.close_create(window, cx);
            } else if self.detail_open() {
                self.close_detail(window, cx);
            } else {
                self.close_tasks(window, cx);
            }
            return true;
        }
        if self.tasks_ui.sync_panel.is_some() {
            // The question about GitHub is answered with Enter or with the
            // buttons; the board's other keys wait.
            if stroke.key == "enter" && !other && !m.shift {
                if self.github_on() {
                    self.sync_now(cx);
                    self.close_sync_panel(cx);
                } else {
                    self.turn_on_sync(cx);
                }
                return true;
            }
            return false;
        }
        if self.create_open() {
            // The panel for a new task is typing: its own keys, and nothing
            // of the board's.
            return self.create_key(stroke, window, cx);
        }
        if self.detail_open() && self.detail_key(stroke, window, cx) {
            return true;
        }
        // A field of the details has the keyboard: what is typed is text.
        if self.tasks_typing(window, cx) {
            return false;
        }
        let Some(action) = task_board::action_of(stroke.key.as_str(), m.shift, other) else {
            return false;
        };
        match action {
            Action::Left | Action::Right | Action::Up | Action::Down => {
                self.tasks_ui.board.step(action);
                self.follow_detail(window, cx);
            }
            Action::Details => self.toggle_detail(window, cx),
            Action::Sync => {
                if self.github_on() {
                    self.sync_now(cx);
                } else {
                    self.open_sync_panel(cx);
                }
            }
            Action::CardLeft
            | Action::CardRight
            | Action::CardUp
            | Action::CardDown
            | Action::Done => {
                if let Some(plan) = self.tasks_ui.board.plan(action) {
                    self.tasks_move(plan);
                }
            }
            Action::Priority => self.tasks_raise_priority(),
            Action::New => self.open_create(None, window, cx),
            Action::Start => self.tasks_start(window, cx),
            Action::Scope => {
                let which = if self.tasks_ui.want_global {
                    self.tasks_pick_project()
                } else {
                    Which::Global
                };
                self.tasks_show(which);
            }
        }
        cx.notify();
        true
    }

    /// The project's board to go back to from the global one.
    fn tasks_pick_project(&self) -> Which {
        self.board_projects()
            .first()
            .map(|(_, _, root)| Which::Project(root.clone()))
            .unwrap_or(Which::Global)
    }

    fn tasks_move(&mut self, plan: task_board::Plan) {
        self.tasks_ui.board.follow(plan.id.clone());
        self.engine.submit(Op::MoveTask {
            id: plan.id,
            status: plan.status,
            before: plan.before,
        });
    }

    fn tasks_raise_priority(&mut self) {
        let Some(card) = self.tasks_ui.board.selected() else {
            return;
        };
        let patch = TaskPatch {
            priority: Some(task_board::next_priority(card.priority)),
            ..TaskPatch::default()
        };
        let id = card.id.clone();
        self.engine.submit(Op::UpdateTask { id, patch });
    }

    /// Notes a drag passing over a place of the board: what releasing the
    /// pointer now would do. `true` when anything changed.
    fn note_card_drag_over(&mut self, id: &str, column: usize, index: usize) -> bool {
        let at = DropAt {
            id: id.to_owned(),
            column,
            index,
        };
        let mut changed = self.tasks_ui.dragging.as_deref() != Some(id);
        self.tasks_ui.dragging = Some(id.to_owned());
        if self.tasks_ui.drop.as_ref() != Some(&at) {
            self.tasks_ui.drop = Some(at);
            changed = true;
        }
        changed
    }

    /// Forgets where a drag would land: the pointer left the columns.
    pub(super) fn forget_card_drop(&mut self) -> bool {
        let had = self.tasks_ui.drop.is_some() || self.tasks_ui.dragging.is_some();
        self.tasks_ui.drop = None;
        self.tasks_ui.dragging = None;
        had
    }

    /// The release of a drag: carries the card where the moves saw it last.
    pub(super) fn finish_card_drop(&mut self) {
        let drop = self.tasks_ui.drop.take();
        self.tasks_ui.dragging = None;
        let Some(drop) = drop else {
            return;
        };
        if let Some(plan) = self
            .tasks_ui
            .board
            .drop_plan(&drop.id, drop.column, drop.index)
        {
            self.tasks_move(plan);
        }
    }

    /// The header line of the main pane while the board is shown.
    pub(super) fn tasks_heading(&self) -> (&'static str, String, String) {
        let name = match &self.tasks_ui.which {
            Which::Project(root) => self
                .local_project(root)
                .and_then(|(id, _)| self.snapshot.project(&id))
                .map(|entry| self.project_label(&entry.project))
                .unwrap_or_else(|| root.clone()),
            Which::Global => "Global board".to_owned(),
        };
        let board = &self.tasks_ui.board;
        (
            "Tasks",
            name,
            format!("{} open · {} cards", board.open(), board.len()),
        )
    }

    /// The board, as the main pane's body.
    pub(super) fn render_tasks(&self, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.tasks_ui;
        let board = &ui.board;
        let mut root = div()
            .debug_selector(|| "tasks".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.tasks_header(c, cx));
        // The columns (or the lion, on an empty board) and, over their right
        // edge, the details of a card or the panel of a new task: the
        // columns keep their width, nothing is pushed.
        let main = if board.is_empty() && ui.error.is_none() {
            self.tasks_empty(c, cx).into_any_element()
        } else {
            self.tasks_lanes(c, cx).into_any_element()
        };
        root = root.child(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .relative()
                .overflow_hidden()
                .flex()
                .child(main)
                .children(self.render_detail(c, cx))
                .children(self.render_create(c, cx))
                .children(self.render_sync_panel(c, cx)),
        );
        root.child(self.tasks_footer(c)).into_any_element()
    }

    fn tasks_header(&self, c: &Palette, cx: &mut Context<Self>) -> Div {
        let ui = &self.tasks_ui;
        let board = &ui.board;
        let total = board.len();
        let done = total - board.open();
        let share = if total == 0 {
            0.0
        } else {
            done as f32 / total as f32
        };
        let mut chips = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(6.))
            .overflow_hidden();
        let chip = |id: SharedString, label: String, on: bool| {
            mono(label)
                .id(id.clone())
                .debug_selector(move || id.to_string())
                .flex_none()
                .px(px(8.))
                .py(px(3.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if on { c.signal } else { c.elevated_border })
                .bg(if on { c.surface_2 } else { c.surface })
                .text_color(if on { c.text } else { c.text_muted })
                .cursor_pointer()
        };
        for (index, (_, name, root)) in self.board_projects().into_iter().enumerate() {
            let on = ui.which == Which::Project(root.clone());
            let target = Which::Project(root);
            chips = chips.child(
                chip(format!("tasks-project-{index}").into(), name, on).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.tasks_show(target.clone());
                        cx.notify();
                    },
                )),
            );
        }
        if let Some(github) = self.github_chip(c, cx) {
            chips = chips.child(github);
        }
        chips = chips.child(
            chip(
                "tasks-global".into(),
                "Global".to_owned(),
                ui.which == Which::Global,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.tasks_show(Which::Global);
                cx.notify();
            })),
        );
        let (_, title, _) = self.tasks_heading();
        div()
            .flex_none()
            .px_4()
            .py(px(10.))
            .border_b_1()
            .border_color(c.border)
            .flex()
            .items_center()
            .gap_3()
            .child(mark(px(30.), c, cx, Lion::new("tasks-lion").hover()))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .debug_selector(|| "tasks-title".into())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(metrics::TEXT_BODY())
                            .text_color(c.text)
                            .child(title),
                    )
                    .child(
                        mono(format!(
                            "{} OPEN · {} DONE · {} TOTAL",
                            board.open(),
                            done,
                            total
                        ))
                        .debug_selector(|| "tasks-counts".into())
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(c.text_faint),
                    ),
            )
            .child(
                // How much of the board is done.
                div()
                    .debug_selector(|| "tasks-progress".into())
                    .flex_none()
                    .w(px(96.))
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(c.border)
                    .child(
                        div()
                            .h_full()
                            .w(px(96. * share))
                            .rounded(px(2.))
                            .bg(c.success),
                    ),
            )
            .child(chips)
            .child(
                super::updates_view::pill("tasks-new", "New task  n", c).on_click(
                    cx.listener(|this, _, window, cx| this.open_create(None, window, cx)),
                ),
            )
    }

    fn tasks_lanes(&self, c: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let ui = &self.tasks_ui;
        let (cursor_column, cursor_row) = ui.board.cursor();
        let drop = ui.drop.clone();
        let mut lanes = div()
            .id("tasks-columns")
            .debug_selector(|| "tasks-columns".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .gap(px(10.))
            .p_3()
            // The pointer left every column: a release drops nothing.
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<CardDrag>, _, cx| {
                if !event.bounds.contains(&event.event.position) && this.forget_card_drop() {
                    cx.notify();
                }
            }));
        for (index, status) in COLUMNS.into_iter().enumerate() {
            let cards = &ui.board.columns()[index];
            let accent = status_colour(status, c);
            let here = drop.as_ref().is_some_and(|d| d.column == index);
            let focused = index == cursor_column;
            let mut list = div()
                .id(("tasks-cards", index))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(8.))
                .pb(px(8.))
                .flex()
                .flex_col()
                .gap(px(8.));
            for (row, task) in cards.iter().enumerate() {
                let selected = focused && row == cursor_row;
                let line = drop
                    .as_ref()
                    .is_some_and(|d| d.column == index && d.index == row);
                let ghosted = ui.dragging.as_deref() == Some(task.id.as_str());
                list = list.child(self.task_card(task, index, row, selected, line, ghosted, c, cx));
            }
            // The room under the last card: a drop here is the end of the
            // column, and in an empty column the whole of it.
            let at_end = drop
                .as_ref()
                .is_some_and(|d| d.column == index && d.index >= cards.len());
            let empty = cards.is_empty();
            let id_for_tail = index;
            list = list.child(
                div()
                    .id(("tasks-tail", index))
                    .debug_selector(move || format!("tasks-tail-{id_for_tail}"))
                    .relative()
                    .flex_1()
                    .min_h(px(if empty { 96. } else { 36. }))
                    .when(empty, |this| {
                        this.rounded(metrics::RADIUS())
                            .border_1()
                            .border_dashed()
                            .border_color(if here { c.signal } else { c.border })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .debug_selector(move || format!("tasks-empty-{id_for_tail}"))
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(c.text_faint)
                                    .child(if here { "Drop here" } else { "Nothing here" }),
                            )
                    })
                    .when(at_end && !empty, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(-5.))
                                .left_0()
                                .right_0()
                                .h(px(2.))
                                .rounded(px(1.))
                                .bg(c.signal),
                        )
                    })
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<CardDrag>, _, cx| {
                            if !event.bounds.contains(&event.event.position) {
                                return;
                            }
                            let id = event.drag(cx).id.clone();
                            let end = this.tasks_ui.board.columns()[index].len();
                            if this.note_card_drag_over(&id, index, end) {
                                cx.notify();
                            }
                        },
                    )),
            );
            let count = cards.len();
            lanes = lanes.child(
                div()
                    .id(("tasks-column", index))
                    .debug_selector(move || format!("tasks-column-{index}"))
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .rounded(metrics::RADIUS())
                    .border_1()
                    .border_color(if here {
                        c.signal
                    } else if focused {
                        c.elevated_border
                    } else {
                        c.border
                    })
                    .bg(c
                        .surface
                        .blend(accent.opacity(if here { 0.14 } else { 0.05 })))
                    .child(
                        div()
                            .flex_none()
                            .px(px(10.))
                            .py(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_none().size(px(8.)).bg(accent))
                            .child(mono(status.as_str().to_uppercase()).text_color(if focused {
                                c.text
                            } else {
                                c.text_muted
                            }))
                            .child(
                                mono(count.to_string())
                                    .debug_selector(move || format!("tasks-count-{index}"))
                                    .text_color(c.text_faint),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id(("tasks-add", index))
                                    .debug_selector(move || format!("tasks-add-{index}"))
                                    .flex_none()
                                    .size(px(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(metrics::RADIUS())
                                    .cursor_pointer()
                                    .hover(move |style| style.bg(c.surface_2))
                                    .child(icon(IconName::Plus, px(12.), c.text_muted))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        // A new task of this column.
                                        this.tasks_ui.board.select(index, usize::MAX);
                                        this.open_create(Some(COLUMNS[index]), window, cx);
                                    })),
                            ),
                    )
                    .child(list),
            );
        }
        lanes
    }

    #[allow(clippy::too_many_arguments)]
    fn task_card(
        &self,
        task: &Card,
        column: usize,
        row: usize,
        selected: bool,
        line: bool,
        ghosted: bool,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let c = *c;
        let stripe = priority_colour(task.priority, &c);
        let id = task.id.clone();
        let title = task.title.clone();
        let mut meta = meta_of(task);
        if let Some((number, _)) = self.tasks_ui.sync.links.get(&task.id) {
            meta.push_str(&format!("  GH #{number}"));
        }
        let ghost = (title.clone(), meta.clone());
        let agent = task
            .assignee
            .as_deref()
            .or(task.agent.as_deref())
            .and_then(leon_core::AgentId::parse);
        let mut footer = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(metrics::TEXT_SMALL())
            .text_color(c.text_muted);
        if let Some(agent) = agent {
            footer = footer.child(agent_icon(agent, px(12.), &c));
        }
        footer = footer.child(mono(meta));
        // Where the work on it is, and whether it is active: the branch and
        // what the session in its worktree is doing.
        let work_row = match self.work_of(task) {
            Work::None => None,
            Work::Here {
                branch, activity, ..
            } => {
                let (phrase, tone) = work_phrase(activity, &c);
                Some(
                    div()
                        .debug_selector(move || format!("tasks-branch-{column}-{row}"))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .pt(px(6.))
                        .border_t_1()
                        .border_color(c.border)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(activity_dot(activity, &c))
                                .child(icon(IconName::GitBranch, px(11.), c.success))
                                .child(mono(branch).min_w_0().truncate().text_color(c.text)),
                        )
                        .child(
                            mono(phrase.to_owned())
                                .debug_selector(move || format!("tasks-work-{column}-{row}"))
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(tone),
                        ),
                )
            }
            Work::Gone(branch) => Some(
                div()
                    .debug_selector(move || format!("tasks-branch-{column}-{row}"))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pt(px(6.))
                    .border_t_1()
                    .border_color(c.border)
                    .text_color(c.text_faint)
                    .child(icon(IconName::GitBranch, px(11.), c.text_faint))
                    .child(mono(format!("{branch} · worktree removed")).truncate()),
            ),
        };
        let mut labels = div().flex().flex_wrap().gap(px(4.));
        for label in &task.labels {
            labels = labels.child(
                mono(label.clone())
                    .px(px(5.))
                    .py(px(1.))
                    .rounded(metrics::RADIUS())
                    .bg(c.surface_2)
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_muted),
            );
        }
        let hover = c.surface_2;
        div()
            .id(("tasks-card", column * 10_000 + row))
            .debug_selector(move || format!("tasks-card-{column}-{row}"))
            .relative()
            .flex_none()
            .flex()
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(if selected {
                c.signal
            } else {
                c.elevated_border
            })
            .bg(if selected { c.surface_2 } else { c.surface })
            .overflow_hidden()
            .cursor_pointer()
            .when(ghosted, |this| this.opacity(0.35))
            .hover(move |style| style.bg(hover))
            .child(div().flex_none().w(px(3.)).bg(stripe.unwrap_or(c.border)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .text_size(metrics::TEXT_BODY())
                            .text_color(c.text)
                            .child(title),
                    )
                    .when(!task.labels.is_empty(), |this| this.child(labels))
                    .child(footer)
                    .children(work_row),
            )
            .when(line, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(-5.))
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .rounded(px(1.))
                        .bg(c.signal),
                )
            })
            .on_click(
                cx.listener(move |this, _: &gpui_kit::ClickEvent, window, cx| {
                    // A click opens the card: its details are where it is
                    // read and edited. A drag is another gesture and never
                    // gets here.
                    this.tasks_ui.board.select(column, row);
                    if this.detail_open() {
                        this.follow_detail(window, cx);
                    } else {
                        this.open_detail(window, cx);
                    }
                    cx.notify();
                }),
            )
            .on_drag(CardDrag { id: id.clone() }, move |_, _, _, cx| {
                cx.new(|_| CardGhost {
                    title: ghost.0.clone(),
                    meta: ghost.1.clone(),
                    stripe,
                    palette: c,
                })
            })
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<CardDrag>, _, cx| {
                    // Every card hears every move of the drag: only the one
                    // under the pointer answers, above or below its middle.
                    if !event.bounds.contains(&event.event.position) {
                        return;
                    }
                    let dragged = event.drag(cx).id.clone();
                    let after = event.event.position.y > event.bounds.center().y;
                    let index = row + usize::from(after);
                    if this.note_card_drag_over(&dragged, column, index) {
                        cx.notify();
                    }
                }),
            )
    }

    /// The empty board: the lion and what to do.
    fn tasks_empty(&self, c: &Palette, cx: &mut Context<Self>) -> Div {
        div()
            .debug_selector(|| "tasks-hint".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .child(mark(px(96.), c, cx, Lion::new("tasks-empty-lion").hover()))
            .child(
                div()
                    .text_size(metrics::TEXT_BODY())
                    .text_color(c.text)
                    .child("Nothing on this board yet"),
            )
            .child(
                div()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_muted)
                    .child(
                        "Press n to add a task. Agents add theirs from their terminal or \
                         through the task tools.",
                    ),
            )
    }

    fn tasks_footer(&self, c: &Palette) -> Div {
        let ui = &self.tasks_ui;
        let mut footer = div()
            .debug_selector(|| "tasks-footer".into())
            .flex_none()
            .px_4()
            .py(px(7.))
            .border_t_1()
            .border_color(c.border)
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(12.))
            .gap_y(px(4.))
            .text_size(metrics::TEXT_SMALL())
            .text_color(c.text_muted);
        if let Some(error) = &ui.error {
            footer = footer.child(
                div()
                    .debug_selector(|| "tasks-error".into())
                    .text_color(c.text)
                    .child(format!("Could not read the board: {error}")),
            );
        }
        if let Some(note) = ui.note {
            footer = footer.child(div().debug_selector(|| "tasks-note".into()).child(note));
        }
        let hint = |keys: &str, what: &str| {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(key_cap(keys.to_owned(), c))
                .child(what.to_owned())
        };
        footer
            .child(hint("click", "open the card to edit it"))
            .child(hint("← → ↑ ↓", "move"))
            .child(hint("⇧", "carry the card"))
            .child(hint("drag", "move it anywhere"))
            .child(hint("e", "details"))
            .child(hint("s", "sync with GitHub"))
            .child(hint("w", "create worktree"))
            .child(hint("n", "new"))
            .child(hint("d", "done"))
            .child(hint("p", "priority"))
            .child(hint("g", "global board"))
            .child(hint("esc", "back"))
    }
}

//! The details of a task: a drawer over the right edge of the board.
//!
//! It reads and edits what a card cannot show: the title, the description
//! (several lines), who it is for, its labels, its status and priority, and
//! where the work on it is. What can be done to it is in view at the bottom:
//! create its worktree or open it, finish, cancel, delete. It floats over
//! the columns and pushes nothing: they keep their width and the cards that
//! are not under it can still be dragged while it is open.
//!
//! **Keys.** While a field of the drawer has the keyboard nothing but its own
//! keys is the board's: `Esc` saves and closes, `Cmd+Enter` or `Ctrl+Enter`
//! saves, `Enter` in the title or the assignee saves and in the labels adds
//! one; everything else, `n` and `d` and `w` included, is typed
//! ([`Shell::tasks_typing`] tells the window so, and bare commands wait).
//!
//! **Saving.** The text fields are saved when asked, when the drawer closes
//! and when another card is chosen, as one patch of only what changed
//! ([`task_board::changes`]); a title that would be empty is refused where it
//! is typed. Status, priority and labels apply at once. The drawer is of a
//! task, by id, and not of the cursor: a task that is canceled, moved or
//! changed by an agent while the drawer is open is still the one shown, and a
//! task that is deleted elsewhere says so. The reading of the board never
//! touches the fields, so what is being typed is not lost.

use gpui_kit::component::input::{InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, ClickEvent, Context, Div, Entity, Focusable, FontWeight, Hsla, Stateful, Window,
};
use leon_core::{Task as Card, TaskPatch, TaskPriority, TaskStatus};
use leon_king::run::{RunState, StepState};

use super::activity::Activity;
use super::shell::{Pane, Shell};
use super::updates_view::pill;
use super::widgets::{activity_dot, mono, text_input};
use crate::engine::Op;
use crate::icons::{agent_icon, icon, IconName};
use crate::task_board::{self, Fields};
use crate::theme::{metrics, px, Palette};

/// The widest the drawer is, in pixels.
const WIDEST: f32 = 420.;

/// The narrowest the drawer is, in pixels.
const NARROWEST: f32 = 320.;

/// What the open drawer holds.
pub struct DetailUi {
    /// The task it is of.
    pub id: String,
    /// The task as it was last read, for when it is not on the board
    /// (canceled) any more.
    pub task: Card,
    /// The task is not in the store any more.
    pub gone: bool,
    /// The title field.
    pub title: Entity<InputState>,
    /// The description field.
    pub body: Entity<TextareaState>,
    /// The assignee field.
    pub assignee: Entity<InputState>,
    /// The field a label is typed in.
    pub label: Entity<InputState>,
    /// What the fields held when they were filled or last saved.
    pub base: Fields,
    /// The delete button was pressed once.
    pub confirm_delete: bool,
    /// Why the last save was refused.
    pub error: Option<String>,
}

/// Where the work on a task is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Work {
    /// No worktree was made for it.
    None,
    /// It has a branch and the project has no worktree on it any more.
    Gone(String),
    /// Its worktree is in the tree.
    Here {
        /// The branch.
        branch: String,
        /// The folder of the worktree.
        path: String,
        /// How its terminals are doing.
        activity: Activity,
        /// How many sessions of it are open in this window.
        sessions: usize,
    },
}

/// What the activity of a worktree says of the work on its task, and the
/// colour it is said in. A worktree with a session open is *active*.
pub(super) fn work_phrase(activity: Activity, c: &Palette) -> (&'static str, Hsla) {
    match activity {
        Activity::Working => ("ACTIVE · WORKING", c.success),
        Activity::TurnOver => ("ACTIVE · READY FOR YOU", c.info),
        Activity::Waiting => ("ACTIVE · WAITING FOR YOU", c.warning),
        Activity::NeedsYou => ("ACTIVE · ASKS YOU", c.warning),
        Activity::Failed => ("ACTIVE · FAILED", c.error),
        Activity::Idle => ("ACTIVE · AT THE PROMPT", c.text_muted),
        Activity::Off => ("WORKTREE · NO SESSION", c.text_faint),
    }
}

fn status_colour(status: TaskStatus, c: &Palette) -> Hsla {
    match status {
        TaskStatus::Backlog => c.text_muted,
        TaskStatus::Todo => c.info,
        TaskStatus::Doing => c.success,
        TaskStatus::Review => c.warning,
        TaskStatus::Done | TaskStatus::Canceled => c.text_faint,
    }
}

fn priority_colour(priority: TaskPriority, c: &Palette) -> Hsla {
    match priority {
        TaskPriority::Urgent => c.error,
        TaskPriority::High => c.warning,
        TaskPriority::Medium => c.info,
        TaskPriority::Low | TaskPriority::None => c.text_faint,
    }
}

/// The caption over a group of the drawer.
fn caption(text: &str, c: &Palette) -> Div {
    mono(text.to_owned())
        .text_size(metrics::TEXT_SMALL())
        .text_color(c.text_faint)
}

impl Shell {
    /// Where the work on a task is: the worktree its branch names, and how
    /// the terminals in it are doing.
    pub(super) fn work_of(&self, task: &Card) -> Work {
        let Some(branch) = task.branch.clone() else {
            return Work::None;
        };
        let found = self
            .tasks_ui
            .project
            .as_ref()
            .and_then(|project| self.snapshot.project(project))
            .and_then(|entry| {
                entry
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.branch.as_deref() == Some(branch.as_str()))
            });
        match found {
            Some(worktree) => Work::Here {
                path: worktree.path.clone(),
                activity: self.worktree_activity(&worktree.id),
                sessions: self.placement.live_of_worktree(&worktree.id).len(),
                branch,
            },
            None => Work::Gone(branch),
        }
    }

    /// Whether the drawer is open.
    pub(super) fn detail_open(&self) -> bool {
        self.tasks_ui.detail.is_some()
    }

    /// Whether one of the drawer's fields has the keyboard: what is typed is
    /// text, not commands.
    pub(super) fn tasks_typing(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.tasks_open()
            && (self.create_typing(window, cx)
                || self.tasks_ui.detail.as_ref().is_some_and(|detail| {
                    detail.title.read(cx).focus_handle(cx).is_focused(window)
                        || detail.body.read(cx).focus_handle(cx).is_focused(window)
                        || detail.assignee.read(cx).focus_handle(cx).is_focused(window)
                        || detail.label.read(cx).focus_handle(cx).is_focused(window)
                }))
    }

    /// Opens the drawer on the card under the cursor, or closes it.
    pub(super) fn toggle_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.detail_open() {
            self.close_detail(window, cx);
        } else {
            self.open_detail(window, cx);
        }
    }

    /// Opens the drawer on the card under the cursor.
    pub(super) fn open_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.tasks_ui.board.selected().cloned() else {
            return;
        };
        self.tasks_ui.create = None;
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("What is to be done"));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("What it is about: why, where, how to know it is done")
        });
        let assignee = cx.new(|cx| InputState::new(window, cx).placeholder("A person or an agent"));
        let label = cx.new(|cx| InputState::new(window, cx).placeholder("Add a label, Enter"));
        let base = Fields {
            title: task.title.clone(),
            body: task.body.clone(),
            assignee: task.assignee.clone().unwrap_or_default(),
        };
        title.update(cx, |field, cx| {
            field.set_value(base.title.clone(), window, cx)
        });
        body.update(cx, |field, cx| {
            field.set_value(base.body.clone(), window, cx)
        });
        assignee.update(cx, |field, cx| {
            field.set_value(base.assignee.clone(), window, cx)
        });
        self.tasks_ui.detail = Some(DetailUi {
            id: task.id.clone(),
            task,
            gone: false,
            title,
            body,
            assignee,
            label,
            base,
            confirm_delete: false,
            error: None,
        });
        cx.notify();
    }

    /// The fields as they are typed now.
    fn detail_fields(&self, cx: &gpui_kit::App) -> Option<Fields> {
        let detail = self.tasks_ui.detail.as_ref()?;
        Some(Fields {
            title: detail.title.read(cx).value().to_string(),
            body: detail.body.read(cx).value().to_string(),
            assignee: detail.assignee.read(cx).value().to_string(),
        })
    }

    /// Saves what was typed, as one patch of what changed. `false` when it
    /// was refused (the reason is in the drawer).
    pub(super) fn save_detail(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(now) = self.detail_fields(cx) else {
            return true;
        };
        let Some(detail) = self.tasks_ui.detail.as_mut() else {
            return true;
        };
        if detail.gone {
            return true;
        }
        match task_board::changes(&detail.base, &now) {
            Err(why) => {
                detail.error = Some(why.to_owned());
                false
            }
            Ok(None) => {
                detail.error = None;
                true
            }
            Ok(Some(patch)) => {
                detail.error = None;
                // What the store will hold, so that it is not a change again.
                detail.base = Fields {
                    title: task_board::clean_line(&now.title),
                    body: task_board::clean_body(&now.body),
                    assignee: task_board::clean_line(&now.assignee),
                };
                self.engine.submit(Op::UpdateTask {
                    id: detail.id.clone(),
                    patch,
                });
                true
            }
        }
    }

    /// Saves and closes the drawer. When the save is refused the drawer stays
    /// open with the reason.
    pub(super) fn close_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.save_detail(cx) {
            cx.notify();
            return;
        }
        self.tasks_ui.detail = None;
        self.pane = Pane::Main;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// The cursor moved: the drawer goes to the card it is on, after saving
    /// the one it was on. A refused save keeps the drawer where it was.
    pub(super) fn follow_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.tasks_ui.detail.as_ref().map(|d| d.id.clone()) else {
            return;
        };
        let selected = self.tasks_ui.board.selected().map(|task| task.id.clone());
        if selected.as_deref() == Some(id.as_str()) || selected.is_none() {
            return;
        }
        if !self.save_detail(cx) {
            // Back on the card whose title is wrong.
            self.tasks_ui.board.follow(id);
            if let Some((column, row)) = self.tasks_ui.board.find(&self.detail_id()) {
                self.tasks_ui.board.select(column, row);
            }
            return;
        }
        self.tasks_ui.detail = None;
        self.open_detail(window, cx);
    }

    fn detail_id(&self) -> String {
        self.tasks_ui
            .detail
            .as_ref()
            .map(|detail| detail.id.clone())
            .unwrap_or_default()
    }

    /// The task of the drawer: the card on the board, else the last copy.
    fn detail_card(&self) -> Option<Card> {
        let detail = self.tasks_ui.detail.as_ref()?;
        match self.tasks_ui.board.find(&detail.id) {
            Some((column, row)) => Some(self.tasks_ui.board.columns()[column][row].clone()),
            None => Some(detail.task.clone()),
        }
    }

    /// Keeps the drawer's copy of its task as the board last read it, and
    /// notices a task that is gone. Never touches the fields.
    pub(super) fn detail_reloaded(&mut self) {
        let Some(id) = self.tasks_ui.detail.as_ref().map(|d| d.id.clone()) else {
            return;
        };
        let on_board = self
            .tasks_ui
            .board
            .find(&id)
            .map(|(column, row)| self.tasks_ui.board.columns()[column][row].clone());
        let read = match on_board {
            Some(card) => Ok(card),
            None => self.engine.store().task(&id),
        };
        if let Some(detail) = self.tasks_ui.detail.as_mut() {
            match read {
                Ok(card) => {
                    detail.task = card;
                    detail.gone = false;
                }
                Err(leon_core::StoreError::NotFound(_)) => detail.gone = true,
                Err(_) => {}
            }
        }
    }

    /// A key while the drawer is open. `true` when it was the drawer's.
    pub(super) fn detail_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &stroke.modifiers;
        let typing = self.tasks_typing(window, cx);
        let save = stroke.key == "enter" && (m.platform || m.control) && !m.alt;
        if save {
            if self.save_detail(cx) {
                self.engine.report(
                    crate::engine::StatusKind::Info,
                    "Saved the details of the task.",
                );
            }
            cx.notify();
            return true;
        }
        if !typing || m.platform || m.control || m.alt || m.shift || m.function {
            return false;
        }
        let in_field = |field: &Entity<InputState>, cx: &gpui_kit::App| {
            field.read(cx).focus_handle(cx).is_focused(window)
        };
        if stroke.key == "enter" {
            let (title, assignee, label) = match self.tasks_ui.detail.as_ref() {
                Some(d) => (
                    in_field(&d.title, cx),
                    in_field(&d.assignee, cx),
                    in_field(&d.label, cx),
                ),
                None => return false,
            };
            if label {
                self.add_label_from_field(window, cx);
                return true;
            }
            if title || assignee {
                self.save_detail(cx);
                cx.notify();
                return true;
            }
        }
        false
    }

    /// Adds the label typed in its field to the task.
    fn add_label_from_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.tasks_ui.detail.as_ref() else {
            return;
        };
        let typed = detail.label.read(cx).value().to_string();
        let label = detail.label.clone();
        let Some(card) = self.detail_card() else {
            return;
        };
        if let Some(labels) = task_board::with_label(&card.labels, &typed) {
            self.engine.submit(Op::UpdateTask {
                id: card.id,
                patch: TaskPatch {
                    labels: Some(labels),
                    ..TaskPatch::default()
                },
            });
        }
        label.update(cx, |field, cx| field.set_value("", window, cx));
    }

    fn detail_remove_label(&mut self, name: &str) {
        let Some(card) = self.detail_card() else {
            return;
        };
        let labels: Vec<String> = card.labels.into_iter().filter(|l| l != name).collect();
        self.engine.submit(Op::UpdateTask {
            id: card.id,
            patch: TaskPatch {
                labels: Some(labels),
                ..TaskPatch::default()
            },
        });
    }

    fn detail_set_status(&mut self, status: TaskStatus) {
        let Some(card) = self.detail_card() else {
            return;
        };
        if card.status != status {
            self.engine.submit(Op::MoveTask {
                id: card.id,
                status,
                before: None,
            });
        }
    }

    fn detail_set_priority(&mut self, priority: TaskPriority) {
        let Some(card) = self.detail_card() else {
            return;
        };
        if card.priority != priority {
            self.engine.submit(Op::UpdateTask {
                id: card.id,
                patch: TaskPatch {
                    priority: Some(priority),
                    ..TaskPatch::default()
                },
            });
        }
    }

    fn detail_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.tasks_ui.detail.as_mut() else {
            return;
        };
        if !detail.confirm_delete {
            detail.confirm_delete = true;
            cx.notify();
            return;
        }
        let id = detail.id.clone();
        self.tasks_ui.detail = None;
        self.engine.submit(Op::DeleteTask { id });
        self.pane = Pane::Main;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// The tasks of the board that are steps of `card`.
    fn detail_steps<'a>(&'a self, card: &'a Card) -> impl Iterator<Item = &'a Card> + 'a {
        self.tasks_ui
            .board
            .columns()
            .iter()
            .flatten()
            .filter(move |task| task.parent.as_deref() == Some(card.id.as_str()))
    }

    /// What the drawer says of a task's steps and of its run, or of the
    /// task it is a step of: a group's title and its content.
    fn steps_of(&self, card: &Card, c: &Palette) -> Option<(&'static str, Div)> {
        let title_of = |id: &str| {
            self.tasks_ui
                .board
                .columns()
                .iter()
                .flatten()
                .find(|task| task.id == id)
                .map_or_else(
                    || leon_core::store::memory::short_id(id).to_owned(),
                    |task| task.title.clone(),
                )
        };
        let line = |text: String, tone: Hsla| {
            div()
                .text_size(metrics::TEXT_SMALL())
                .text_color(tone)
                .child(text)
        };
        let steps: Vec<&Card> = self.detail_steps(card).collect();
        if steps.is_empty() {
            let parent = card.parent.as_deref()?;
            let mut content = div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(line(title_of(parent), c.text));
            if !card.needs.is_empty() {
                let needs: Vec<String> = card.needs.iter().map(|need| title_of(need)).collect();
                content = content.child(line(format!("After: {}", needs.join(", ")), c.text_muted));
            }
            return Some(("STEP OF", content));
        }
        let run = self
            .king_ui
            .status
            .as_ref()
            .and_then(|status| status.runs.iter().rev().find(|run| run.task == card.id));
        let mut content = div().flex().flex_col().gap(px(4.));
        if let Some(run) = run {
            let (state, tone) = match run.state {
                RunState::Running => ("RUNNING", c.signal),
                RunState::Blocked => ("BLOCKED", c.error),
                RunState::Done => ("DONE", c.success),
                RunState::Canceled => ("CALLED OFF", c.text_faint),
            };
            content = content.child(
                mono(format!(
                    "RUN #{} · {state} · {} of {} done",
                    run.id,
                    run.done(),
                    run.steps.len()
                ))
                .debug_selector(|| "task-run-state".into())
                .text_size(metrics::TEXT_SMALL())
                .text_color(tone),
            );
            if let Some(note) = &run.note {
                content = content.child(line(note.clone(), c.text_muted));
            }
        }
        for step in steps {
            let ran = run.and_then(|run| run.step(&step.id)).map(|(_, ran)| ran);
            let (mark, tone) = match ran.map(|ran| ran.state) {
                Some(StepState::Done) => ("✓", c.success),
                Some(StepState::Working) => ("●", c.signal),
                Some(StepState::Checking) => ("◷", c.signal),
                Some(StepState::Failed) => ("✗", c.error),
                Some(StepState::Canceled) => ("–", c.text_faint),
                Some(StepState::Waiting) | None if step.status.is_closed() => ("✓", c.text_faint),
                Some(StepState::Waiting) | None => ("○", c.text_muted),
            };
            let mut text = format!("{mark} {}", step.title);
            if let Some(worker) = ran.and_then(|ran| ran.worker) {
                text.push_str(&format!("  · #{worker}"));
            }
            if !step.needs.is_empty() {
                let needs: Vec<String> = step.needs.iter().map(|need| title_of(need)).collect();
                text.push_str(&format!("  · after {}", needs.join(", ")));
            }
            content = content.child(line(text, tone));
        }
        Some(("STEPS", content))
    }

    /// The drawer, beside the columns.
    pub(super) fn render_detail(
        &self,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let detail = self.tasks_ui.detail.as_ref()?;
        let card = self.detail_card()?;
        let c = *c;
        let width = px(WIDEST)
            .min(self.viewport.width * 0.36)
            .max(px(NARROWEST));
        let dirty = self
            .detail_fields(cx)
            .is_some_and(|now| !matches!(task_board::changes(&detail.base, &now), Ok(None)));
        let field = |content: gpui_kit::AnyElement| {
            div()
                .px(px(8.))
                .py(px(4.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(c.elevated_border)
                .bg(c.background)
                .child(content)
        };
        let accent = status_colour(card.status, &c);
        // A low window gets a shorter description, so that more of the
        // drawer is in view without scrolling.
        let body_height = if self.viewport.height < px(900.) {
            90.
        } else {
            150.
        };
        let group = |title: &str, content: Div| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(caption(title, &c))
                .child(content)
        };
        let chip = |id: SharedStringLike, label: String, on: bool, tone: Hsla| {
            mono(label)
                .id(id.0.clone())
                .debug_selector(move || id.0.to_string())
                .flex_none()
                .px(px(8.))
                .py(px(3.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if on { tone } else { c.elevated_border })
                .bg(if on { c.surface_2 } else { c.surface })
                .text_color(if on { c.text } else { c.text_muted })
                .cursor_pointer()
        };

        // Status: where the card is.
        let mut status = div().flex().flex_wrap().gap(px(6.));
        for option in TaskStatus::ALL {
            let on = card.status == option;
            status = status.child(
                chip(
                    SharedStringLike::new(format!("task-status-{}", option.as_str())),
                    option.as_str().to_uppercase(),
                    on,
                    status_colour(option, &c),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.detail_set_status(option);
                    cx.notify();
                })),
            );
        }
        // Priority.
        let mut priority = div().flex().flex_wrap().gap(px(6.));
        for option in TaskPriority::ALL {
            let on = card.priority == option;
            priority = priority.child(
                chip(
                    SharedStringLike::new(format!("task-priority-{}", option.as_str())),
                    option.as_str().to_uppercase(),
                    on,
                    priority_colour(option, &c),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.detail_set_priority(option);
                    cx.notify();
                })),
            );
        }
        // Labels: each one with its way out.
        let mut labels = div().flex().flex_wrap().gap(px(6.));
        for (index, name) in card.labels.iter().enumerate() {
            let remove = name.clone();
            labels = labels.child(
                div()
                    .id(("task-label", index))
                    .debug_selector(move || format!("task-label-{index}"))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .px(px(7.))
                    .py(px(2.))
                    .rounded(metrics::RADIUS())
                    .bg(c.surface_2)
                    .child(
                        mono(name.clone())
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(c.text),
                    )
                    .child(
                        div()
                            .id(("task-label-x", index))
                            .debug_selector(move || format!("task-label-remove-{index}"))
                            .cursor_pointer()
                            .child(icon(IconName::X, px(10.), c.text_muted))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.detail_remove_label(&remove);
                                cx.notify();
                            })),
                    ),
            );
        }
        // Who: the agent's mark when it is one.
        let mut who = div().flex().items_center().gap(px(8.));
        if let Some(agent) = card.assignee.as_deref().and_then(leon_core::AgentId::parse) {
            who = who.child(agent_icon(agent, px(16.), &c));
        }
        who = who.child(
            div()
                .flex_1()
                .min_w_0()
                .child(field(text_input(&detail.assignee).into_any_element())),
        );
        // Where the work is.
        let linked = self.work_of(&card);
        let work = match &linked {
            Work::None => div()
                .debug_selector(|| "task-work-none".into())
                .text_size(metrics::TEXT_SMALL())
                .text_color(c.text_muted)
                .child(
                    "No worktree yet. Create one to code this task: a branch, a folder \
                     and a session of the agent you pick.",
                ),
            Work::Gone(branch) => div()
                .debug_selector(|| "task-work-gone".into())
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_color(c.text_muted)
                        .child(icon(IconName::GitBranch, px(12.), c.text_muted))
                        .child(mono(branch.clone()).truncate()),
                )
                .child(
                    div()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(c.warning)
                        .child("Its worktree is not in the tree any more. Create it again to keep working."),
                ),
            Work::Here {
                branch,
                path,
                activity,
                sessions,
            } => {
                let (phrase, tone) = work_phrase(*activity, &c);
                div()
                    .debug_selector(|| "task-work-here".into())
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(activity_dot(*activity, &c))
                            .child(
                                mono(phrase.to_owned())
                                    .debug_selector(|| "task-work-state".into())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(tone),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_color(c.text)
                            .child(icon(IconName::GitBranch, px(12.), c.success))
                            .child(mono(branch.clone()).truncate()),
                    )
                    .child(
                        mono(path.clone())
                            .truncate()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(c.text_faint),
                    )
                    .child(
                        div()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(c.text_muted)
                            .child(match sessions {
                                0 => "No session is open in it.".to_owned(),
                                1 => "1 session open in it.".to_owned(),
                                n => format!("{n} sessions open in it."),
                            }),
                    )
            }
        };
        let ago = |at: i64| {
            let seconds = (self.now().timestamp_millis() - at).max(0) / 1000;
            leon_usage::ago(seconds)
        };
        let mut meta = format!(
            "Created {}{} · changed {}",
            ago(card.created_at),
            card.agent
                .as_deref()
                .map(|agent| format!(" by {agent}"))
                .unwrap_or_default(),
            ago(card.updated_at)
        );
        if let Some(closed) = card.closed_at {
            meta.push_str(&format!(" · closed {}", ago(closed)));
        }

        let body = div()
            .id("tasks-drawer-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(group(
                "TITLE",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(field(text_input(&detail.title).into_any_element()))
                    .children(detail.error.as_ref().map(|why| {
                        div()
                            .debug_selector(|| "task-detail-error".into())
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(c.error)
                            .child(why.clone())
                    })),
            ))
            .child(group(
                "DESCRIPTION",
                field(
                    Textarea::new(&detail.body)
                        .appearance(false)
                        .bordered(false)
                        .h(px(body_height))
                        .into_any_element(),
                ),
            ))
            .child(group("STATUS", status))
            .child(group("PRIORITY", priority))
            .child(group("ASSIGNED TO", who))
            .child(group(
                "LABELS",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(labels)
                    .child(field(text_input(&detail.label).into_any_element())),
            ))
            .children(
                self.steps_of(&card, &c)
                    .map(|(title, steps)| group(title, steps)),
            )
            .child(group("WORK", work))
            .child(group("GITHUB", self.github_group(&card, &c, cx)))
            .child(
                div()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_faint)
                    .child(meta),
            );

        // What can be done to it, always in view.
        let id = card.id.clone();
        let started = matches!(linked, Work::Here { .. });
        let sessions = match &linked {
            Work::Here { sessions, .. } => *sessions,
            _ => 0,
        };
        let finished = card.status.is_closed();
        let mut actions = div().flex().flex_wrap().gap(px(6.));
        actions = actions.child(
            pill(
                "task-action-start",
                if started {
                    "Open worktree  ⏎"
                } else {
                    "Create worktree  w"
                },
                &c,
            )
            .border_color(c.signal)
            .on_click(cx.listener(|this, _, window, cx| {
                this.tasks_start(window, cx);
                cx.notify();
            })),
        );
        if !finished {
            actions = actions.child(
                pill("task-action-king", "Give it to the King  a", &c).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.tasks_send_to_king(window, cx);
                        cx.notify();
                    },
                )),
            );
        }
        if !finished && self.detail_steps(&card).next().is_some() {
            actions = actions.child(
                pill("task-action-run", "Run its steps  r", &c)
                    .border_color(c.signal)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.tasks_run(window, cx);
                        cx.notify();
                    })),
            );
        }
        if sessions > 0 {
            actions = actions.child(pill("task-action-session", "Open session", &c).on_click(
                cx.listener(|this, _, window, cx| {
                    this.tasks_open_session(window, cx);
                    cx.notify();
                }),
            ));
        }
        actions = actions.child(
            pill(
                "task-action-done",
                if finished { "Reopen" } else { "Mark done  d" },
                &c,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.detail_set_status(if finished {
                    TaskStatus::Todo
                } else {
                    TaskStatus::Done
                });
                cx.notify();
            })),
        );
        if card.status != TaskStatus::Canceled {
            actions = actions.child(pill("task-action-cancel", "Cancel task", &c).on_click(
                cx.listener(|this, _, _, cx| {
                    this.detail_set_status(TaskStatus::Canceled);
                    cx.notify();
                }),
            ));
        }
        actions = actions.child(
            pill(
                "task-action-delete",
                if detail.confirm_delete {
                    "Really delete?"
                } else {
                    "Delete"
                },
                &c,
            )
            .text_color(c.error)
            .on_click(cx.listener(|this, _, window, cx| this.detail_delete(window, cx))),
        );
        let _ = id;

        let footer = div()
            .flex_none()
            .px_4()
            .py(px(10.))
            .border_t_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(actions)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        pill(
                            "task-action-save",
                            if dirty {
                                "Save changes  ⌘⏎"
                            } else {
                                "Saved"
                            },
                            &c,
                        )
                        .when(dirty, |this| this.border_color(c.signal))
                        .when(!dirty, |this| this.text_color(c.text_faint))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.save_detail(cx);
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(c.text_faint)
                            .child("Saved when you close, or move to another card"),
                    ),
            );

        Some(
            div()
                .id("tasks-drawer")
                .debug_selector(|| "tasks-drawer".into())
                // Over the board, against its right edge, as tall as it.
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(width)
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(c.elevated_border)
                .bg(c.surface)
                .shadow_md()
                // What is under it does not hear the pointer.
                .occlude()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex_none()
                        .px_4()
                        .py(px(10.))
                        .border_b_1()
                        .border_color(c.border)
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_none().size(px(8.)).bg(accent))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c.text)
                                .child("Details"),
                        )
                        .child(
                            mono(card.short_id().to_owned())
                                .debug_selector(|| "task-detail-id".into())
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(c.text_faint),
                        )
                        .children(detail.gone.then(|| {
                            div()
                                .debug_selector(|| "task-detail-gone".into())
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(c.warning)
                                .child("removed elsewhere")
                        }))
                        .child(div().flex_1())
                        .child(
                            pill("task-detail-close", "Close  esc", &c).on_click(cx.listener(
                                |this, _: &ClickEvent, window, cx| this.close_detail(window, cx),
                            )),
                        ),
                )
                .child(body)
                .child(footer),
        )
    }
}

/// An element id that is both an id and a debug selector.
#[derive(Clone)]
struct SharedStringLike(gpui_kit::SharedString);

impl SharedStringLike {
    fn new(text: String) -> Self {
        Self(text.into())
    }
}

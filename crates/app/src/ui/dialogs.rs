//! The two dialogs of the sidebar: "Add a project" and "Create worktree".
//!
//! Both are overlays in Leon's own look (the card, the hairline rows, the
//! mono labels and the accent bar of the row the keyboard is on), and both
//! end by asking the engine for the work: the same operations the palette
//! flows use, so there is one implementation of what a clone or a worktree
//! is behind each door.

use super::shell::{Overlay, Shell};
use super::terminals::Place;
use super::widgets::{key_cap, mono, section_label};
use crate::engine::Op;
use crate::format;
use crate::icons::{agent_icon, icon, IconName};
use crate::keys::Command;
use crate::launch::Launch;
use crate::theme::{metrics, px, Palette};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, Context, Div, Entity, FontWeight, SharedString, Stateful, Window};
use leon_core::{AgentId, MachineId, ProjectId};

/// What the "Add a project" dialog is showing. The rows are: the host, then
/// browse, clone and create.
#[derive(Clone, Debug, Default)]
pub struct AddProjectUi {
    /// The machine the new project goes on, as a place in the snapshot's
    /// machine list.
    pub host: usize,
    /// The row the keyboard is on: 0 host, 1 browse, 2 clone, 3 create.
    pub cursor: usize,
}

/// The rows of the "Add a project" dialog, in order.
pub const ADD_ROWS: usize = 4;

/// What the "Create worktree" dialog is showing. The rows are: the name, the
/// base, the agent, "create more" and the create button.
pub struct NewWorktreeUi {
    /// The project the worktree belongs to.
    pub project: ProjectId,
    /// The name (and branch) of the worktree.
    pub name: Entity<InputState>,
    /// What the branch starts from; `HEAD` when empty.
    pub base: Entity<InputState>,
    /// The branches git reported for this project, in their order; empty
    /// while they are loading or when git could not tell.
    pub base_refs: Vec<String>,
    /// Whether the branch list under "Create from" is open.
    pub base_list: bool,
    /// Whether the field was typed in since the list opened. The first
    /// keystroke clears the value that was picked for the user, so typing
    /// searches instead of appending to it.
    pub base_typed: bool,
    /// Whether the agent list is open.
    pub agent_list: bool,
    /// The entry of that list the keyboard is on.
    pub agent_cursor: usize,
    /// The entry of that list the keyboard is on.
    pub base_cursor: usize,
    /// The row the keyboard is on: 0 name, 1 base, 2 agent, 3 more, 4 create.
    pub cursor: usize,
    /// The agent to start in the new worktree, when one is asked for.
    pub agent: Option<AgentId>,
    /// Whether the dialog stays open after creating, for another worktree.
    pub create_more: bool,
}

/// The rows of the "Create worktree" dialog, in order.
pub const WORKTREE_ROWS: usize = 5;

impl Shell {
    // ----- add a project -----------------------------------------------------

    /// Opens the "Add a project" dialog on the machine the keyboard is on.
    pub(super) fn open_add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay != Overlay::None {
            self.close_overlay(window, cx);
        }
        let current = self.current_machine();
        let host = self
            .snapshot
            .machines
            .iter()
            .position(|machine| machine.id == current)
            .unwrap_or(0);
        self.add_project_ui = super::dialogs::AddProjectUi { host, cursor: 1 };
        self.overlay = Overlay::AddProject;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// The machine the add dialog's host row is on.
    fn add_host(&self) -> Option<MachineId> {
        self.snapshot
            .machines
            .get(self.add_project_ui.host)
            .map(|machine| machine.id.clone())
    }

    /// The keys of the "Add a project" dialog. `true` when taken.
    pub(super) fn add_project_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let held = &stroke.modifiers;
        if held.control || held.alt {
            return false;
        }
        let machines = self.snapshot.machines.len().max(1);
        match stroke.key.as_str() {
            "escape" => {
                let agent_open = self
                    .new_worktree_ui
                    .as_ref()
                    .is_some_and(|ui| ui.agent_list);
                if self.base_list_open() || agent_open {
                    if let Some(ui) = self.new_worktree_ui.as_mut() {
                        ui.base_list = false;
                        ui.agent_list = false;
                    }
                    cx.notify();
                } else {
                    self.close_overlay(window, cx);
                    cx.notify();
                }
            }
            "j" | "down" | "tab" => {
                self.add_project_ui.cursor = (self.add_project_ui.cursor + 1) % ADD_ROWS;
                cx.notify();
            }
            "k" | "up" | "shift-tab" => {
                self.add_project_ui.cursor = (self.add_project_ui.cursor + ADD_ROWS - 1) % ADD_ROWS;
                cx.notify();
            }
            "left" | "h" => {
                if self.add_project_ui.cursor == 0 {
                    self.add_project_ui.host = (self.add_project_ui.host + machines - 1) % machines;
                    cx.notify();
                }
            }
            "right" | "l" => {
                if self.add_project_ui.cursor == 0 {
                    self.add_project_ui.host = (self.add_project_ui.host + 1) % machines;
                    cx.notify();
                }
            }
            "enter" => match self.add_project_ui.cursor {
                0 => {
                    self.add_project_ui.host = (self.add_project_ui.host + 1) % machines;
                    cx.notify();
                }
                1 => self.add_project_browse(window, cx),
                2 => self.add_project_clone(window, cx),
                _ => self.add_project_create(window, cx),
            },
            _ => return false,
        }
        true
    }

    /// "Browse folder": the folder dialog on this computer, the palette's
    /// path question on any other machine.
    fn add_project_browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.add_host() else {
            return;
        };
        self.open_project_on(&host, window, cx);
    }

    /// "Clone from URL": the palette asks for the URL, the name and the
    /// folder, with the chosen host already answered.
    fn add_project_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.add_host() else {
            return;
        };
        self.begin_flow_with(
            Command::CloneProject,
            vec![host.as_str().to_owned()],
            window,
            cx,
        );
    }

    /// "Create new project": the palette asks for the name and the folder,
    /// with the chosen host already answered.
    fn add_project_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.add_host() else {
            return;
        };
        self.begin_flow_with(
            Command::NewProject,
            vec![host.as_str().to_owned()],
            window,
            cx,
        );
    }

    /// The "Add a project" card.
    pub(super) fn render_add_project(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let row = |cursor: usize, index: usize, colours: &Palette| {
            let on = cursor == index;
            div()
                .id(("add-row", index))
                .debug_selector(move || format!("add-row-{index}"))
                .relative()
                .min_h(metrics::CONTROL())
                .px(px(12.))
                .py(px(9.))
                .flex()
                .items_center()
                .gap(px(12.))
                .rounded(metrics::RADIUS())
                .cursor_pointer()
                .when(on, |this| {
                    this.bg(colours.surface_2).child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(2.))
                            .bg(colours.signal),
                    )
                })
                .hover({
                    let hover = colours.surface_2;
                    move |style| style.bg(hover)
                })
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if this.add_project_ui.cursor != index {
                        this.add_project_ui.cursor = index;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.add_project_ui.cursor = index;
                    match index {
                        1 => this.add_project_browse(window, cx),
                        2 => this.add_project_clone(window, cx),
                        3 => this.add_project_create(window, cx),
                        _ => cx.notify(),
                    }
                }))
        };
        let host = self
            .snapshot
            .machines
            .get(self.add_project_ui.host)
            .map_or_else(String::new, |machine| machine.name.clone());
        let host_detail = if self.add_host().is_some_and(|id| id.is_local()) {
            "this computer"
        } else {
            "over SSH"
        };
        self.card("add-project", colours)
            .debug_selector(|| "add-project".into())
            .w(px(440.))
            .p_4()
            .flex()
            .flex_col()
            .gap(px(8.))
            .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(
                div()
                    .px_2()
                    .pb(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(section_label("Add a project", colours))
                    .child(div().flex_1())
                    .child(key_cap("Esc".to_owned(), colours)),
            )
            .child(
                row(self.add_project_ui.cursor, 0, colours)
                    .child(icon(IconName::Server, px(16.), colours.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(format!("Host  {host}")),
                    )
                    .child(mono(host_detail).text_color(colours.text_faint))
                    .child(mono("\u{2190} \u{2192}").text_color(colours.text_faint)),
            )
            .child(
                row(self.add_project_ui.cursor, 1, colours)
                    .child(icon(IconName::Folder, px(16.), colours.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(div().font_weight(FontWeight::MEDIUM).child("Browse folder"))
                            .child(
                                div()
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(colours.text_faint)
                                    .child("A local project, a git repository, or a folder"),
                            ),
                    ),
            )
            .child(
                row(self.add_project_ui.cursor, 2, colours)
                    .child(icon(IconName::GitBranch, px(16.), colours.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Clone from URL"),
                            )
                            .child(
                                div()
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(colours.text_faint)
                                    .child("Clone a remote git repository"),
                            ),
                    ),
            )
            .child(
                row(self.add_project_ui.cursor, 3, colours)
                    .child(icon(IconName::Plus, px(16.), colours.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Create new project"),
                            )
                            .child(
                                div()
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(colours.text_faint)
                                    .child("Start from an empty folder"),
                            ),
                    ),
            )
    }

    // ----- create worktree ---------------------------------------------------

    /// Opens the "Create worktree" dialog for `project`.
    pub(super) fn open_new_worktree_for(
        &mut self,
        project: &ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.project(project).is_none() {
            return;
        }
        if self.overlay != Overlay::None {
            self.close_overlay(window, cx);
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("feature/login"));
        let base = cx.new(|cx| InputState::new(window, cx).placeholder("HEAD"));
        // The choice on offer: the agent the settings name, else the last one
        // started. "Ask" is not an agent, so it falls through.
        let prefs = super::shell::Shell::step_prefs(cx);
        let agent = prefs
            .default_agent
            .filter(|agent| prefs.offered().contains(agent))
            .or(self.last_agent);
        self.new_worktree_ui = Some(super::dialogs::NewWorktreeUi {
            project: project.clone(),
            name,
            base,
            base_refs: Vec::new(),
            base_list: false,
            base_typed: false,
            base_cursor: 0,
            agent_list: false,
            agent_cursor: 0,
            cursor: 0,
            agent,
            create_more: false,
        });
        self.overlay = Overlay::NewWorktree;
        self.focus_worktree_row(window, cx);
        // Typing in "Create from" filters the branch list as it goes.
        let base = self.new_worktree_ui.as_ref().map(|ui| ui.base.clone());
        if let Some(base) = base {
            self._subscriptions.push(cx.subscribe_in(
                &base,
                window,
                |this, _, event: &gpui_kit::component::input::InputEvent, _, cx| {
                    if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                        if let Some(ui) = this.new_worktree_ui.as_mut() {
                            ui.base_cursor = 0;
                        }
                        cx.notify();
                    }
                },
            ));
        }
        // The branches git knows for this project arrive a moment later, so
        // "Create from" can offer them; the field is free text meanwhile.
        let loading = self.engine.base_refs(project.clone());
        self.worktree_task = Some(cx.spawn_in(window, async move |this, cx| {
            let refs = loading.await.ok().flatten().unwrap_or_default();
            if refs.is_empty() {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                let Some(ui) = this.new_worktree_ui.as_mut() else {
                    return;
                };
                ui.base_refs = refs;
                // A fresh dialog answers itself with the usual base, like
                // `git clone` would leave checked out.
                let field = ui.base.clone();
                if field.read(cx).value().trim().is_empty() {
                    if let Some(default) = ui
                        .base_refs
                        .iter()
                        .find(|name| {
                            name.as_str() == "origin/main" || name.as_str() == "origin/master"
                        })
                        .cloned()
                    {
                        field.update(cx, |field, cx| field.set_value(&default, window, cx));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Opens the dialog for the project the keyboard is on; without one, the
    /// palette asks for it first.
    pub(super) fn open_new_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.project_here() {
            Some(project) => self.open_new_worktree_for(&project, window, cx),
            None => self.begin_flow(Command::NewWorktree, window, cx),
        }
    }

    /// Gives the keyboard to the highlighted row: its field for the text
    /// rows, the window for the rest.
    fn focus_worktree_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = &self.new_worktree_ui else {
            return;
        };
        match ui.cursor {
            0 => ui
                .name
                .clone()
                .update(cx, |field, cx| field.focus(window, cx)),
            1 => {
                ui.base
                    .clone()
                    .update(cx, |field, cx| field.focus(window, cx));
                if let Some(ui) = self.new_worktree_ui.as_mut() {
                    ui.base_list = !ui.base_refs.is_empty();
                    ui.base_cursor = 0;
                    ui.base_typed = false;
                    ui.agent_list = false;
                }
            }
            2 => {
                self.focus.focus(window, cx);
                if let Some(ui) = self.new_worktree_ui.as_mut() {
                    ui.base_list = false;
                    ui.agent_list = true;
                    ui.agent_cursor = agent_choices(cx)
                        .iter()
                        .position(|agent| *agent == ui.agent)
                        .unwrap_or(0);
                }
            }
            _ => self.focus.focus(window, cx),
        }
    }

    /// The keys of the "Create worktree" dialog. `true` when taken.
    pub(super) fn new_worktree_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let held = &stroke.modifiers;
        let submit = stroke.key == "enter" && (held.platform || held.control);
        if held.platform || held.control || held.alt {
            if submit {
                self.submit_new_worktree(window, cx);
                return true;
            }
            return false;
        }
        let Some(cursor) = self.new_worktree_ui.as_ref().map(|ui| ui.cursor) else {
            return false;
        };
        match stroke.key.as_str() {
            "escape" => {
                let agent_open = self
                    .new_worktree_ui
                    .as_ref()
                    .is_some_and(|ui| ui.agent_list);
                if self.base_list_open() || agent_open {
                    if let Some(ui) = self.new_worktree_ui.as_mut() {
                        ui.base_list = false;
                        ui.agent_list = false;
                    }
                    cx.notify();
                } else {
                    self.close_overlay(window, cx);
                    cx.notify();
                }
            }
            "tab" => {
                if let Some(ui) = self.new_worktree_ui.as_mut() {
                    ui.cursor = (ui.cursor + 1) % WORKTREE_ROWS;
                    ui.base_list = false;
                    ui.agent_list = false;
                }
                self.focus_worktree_row(window, cx);
                cx.notify();
            }
            "shift-tab" => {
                if let Some(ui) = self.new_worktree_ui.as_mut() {
                    ui.cursor = (ui.cursor + WORKTREE_ROWS - 1) % WORKTREE_ROWS;
                    ui.base_list = false;
                    ui.agent_list = false;
                }
                self.focus_worktree_row(window, cx);
                cx.notify();
            }
            "down" | "up" => {
                let forward = stroke.key == "down";
                let has_refs = self
                    .new_worktree_ui
                    .as_ref()
                    .is_some_and(|ui| !ui.base_refs.is_empty());
                let agent_open = self
                    .new_worktree_ui
                    .as_ref()
                    .is_some_and(|ui| ui.cursor == 2 && ui.agent_list);
                if cursor == 2 && forward && !agent_open {
                    if let Some(ui) = self.new_worktree_ui.as_mut() {
                        ui.agent_list = true;
                        ui.agent_cursor = agent_choices(cx)
                            .iter()
                            .position(|agent| *agent == ui.agent)
                            .unwrap_or(0);
                    }
                    cx.notify();
                } else if cursor == 2 && agent_open {
                    let count = agent_choices(cx).len();
                    if forward {
                        if let Some(ui) = self.new_worktree_ui.as_mut() {
                            ui.agent_cursor = (ui.agent_cursor + 1).min(count - 1);
                        }
                    } else if let Some(ui) = self.new_worktree_ui.as_mut() {
                        ui.agent_cursor = ui.agent_cursor.saturating_sub(1);
                    }
                    cx.notify();
                } else if cursor == 1 && forward && has_refs && !self.base_list_open() {
                    // Down opens the branch list under the field.
                    if let Some(ui) = self.new_worktree_ui.as_mut() {
                        ui.base_list = true;
                        ui.base_cursor = 0;
                        ui.base_typed = false;
                    }
                    cx.notify();
                } else if cursor == 1 && self.base_list_open() {
                    // Walk the filtered branches.
                    let count = self.filtered_bases(cx).len();
                    if count > 0 {
                        if let Some(ui) = self.new_worktree_ui.as_mut() {
                            ui.base_cursor = if forward {
                                (ui.base_cursor + 1).min(count - 1)
                            } else {
                                ui.base_cursor.saturating_sub(1)
                            };
                        }
                    }
                    cx.notify();
                } else {
                    if let Some(ui) = self.new_worktree_ui.as_mut() {
                        ui.cursor = if forward {
                            (ui.cursor + 1) % WORKTREE_ROWS
                        } else {
                            (ui.cursor + WORKTREE_ROWS - 1) % WORKTREE_ROWS
                        };
                        ui.base_list = false;
                    }
                    self.focus_worktree_row(window, cx);
                    cx.notify();
                }
            }
            "enter" => {
                if cursor == 1 && self.base_list_open() {
                    let picked = self
                        .filtered_bases(cx)
                        .get(self.new_worktree_ui.as_ref().map_or(0, |ui| ui.base_cursor))
                        .cloned();
                    if let Some(picked) = picked {
                        if let Some(ui) = &self.new_worktree_ui {
                            ui.base
                                .clone()
                                .update(cx, |field, cx| field.set_value(picked, window, cx));
                        }
                        if let Some(ui) = self.new_worktree_ui.as_mut() {
                            ui.base_list = false;
                            ui.base_typed = true;
                        }
                        cx.notify();
                    }
                } else if cursor == 2
                    && self
                        .new_worktree_ui
                        .as_ref()
                        .is_some_and(|ui| ui.agent_list)
                {
                    let picked = agent_choices(cx)
                        .get(
                            self.new_worktree_ui
                                .as_ref()
                                .map_or(0, |ui| ui.agent_cursor),
                        )
                        .copied()
                        .flatten();
                    if let Some(ui) = self.new_worktree_ui.as_mut() {
                        ui.agent = picked;
                        ui.agent_list = false;
                    }
                    cx.notify();
                } else if cursor == 4 {
                    self.submit_new_worktree(window, cx);
                } else if cursor == 3 {
                    if let Some(ui) = self.new_worktree_ui.as_mut() {
                        ui.create_more = !ui.create_more;
                    }
                    cx.notify();
                } else {
                    // From a field, Enter goes to the button.
                    if let Some(ui) = self.new_worktree_ui.as_mut() {
                        ui.cursor = 4;
                    }
                    self.focus_worktree_row(window, cx);
                    cx.notify();
                }
            }
            "left" | "right" => {
                let forward = stroke.key == "right";
                if cursor == 1 {
                    // Cycle the bases git reported, `HEAD` first.
                    let (refs, current) = match &self.new_worktree_ui {
                        Some(ui) => (
                            std::iter::once("HEAD".to_owned())
                                .chain(ui.base_refs.iter().cloned())
                                .collect::<Vec<_>>(),
                            ui.base.read(cx).value().trim().to_owned(),
                        ),
                        None => return false,
                    };
                    if refs.len() <= 1 {
                        return true;
                    }
                    let at = refs.iter().position(|name| name == &current).unwrap_or(0);
                    let next = if forward {
                        (at + 1) % refs.len()
                    } else {
                        (at + refs.len() - 1) % refs.len()
                    };
                    if let Some(ui) = &self.new_worktree_ui {
                        ui.base.clone().update(cx, |field, cx| {
                            field.set_value(refs[next].clone(), window, cx)
                        });
                    }
                    cx.notify();
                } else if cursor == 2 {
                    if let Some(ui) = self.new_worktree_ui.as_mut() {
                        ui.agent = next_agent(&agent_choices(cx), ui.agent, forward);
                    }
                    cx.notify();
                } else {
                    return false;
                }
            }
            "space" if cursor == 3 => {
                if let Some(ui) = self.new_worktree_ui.as_mut() {
                    ui.create_more = !ui.create_more;
                }
                cx.notify();
            }
            _ => {
                // The fields type; the other rows swallow the key.
                if cursor <= 1 {
                    if cursor == 1 {
                        // The first keystroke searches instead of appending
                        // to the base that was picked for the user.
                        let printable = stroke.key_char.as_deref().is_some_and(|text| {
                            !text.is_empty() && !text.chars().any(char::is_control)
                        });
                        if printable {
                            if let Some(ui) = self.new_worktree_ui.as_mut() {
                                if !ui.base_typed {
                                    ui.base_typed = true;
                                    ui.base
                                        .clone()
                                        .update(cx, |field, cx| field.set_value("", window, cx));
                                }
                            }
                        }
                    }
                    return false;
                }
                return true;
            }
        }
        true
    }

    /// Runs "New worktree" with what the dialog holds: git makes the
    /// worktree, and an agent asked for starts in it once it appears.
    fn submit_new_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = &self.new_worktree_ui else {
            return;
        };
        let branch = ui.name.read(cx).value().trim().to_owned();
        if let Err(why) = crate::address::validate_branch(&branch) {
            self.engine
                .report(crate::engine::StatusKind::Error, why.to_owned());
            return;
        }
        let base = ui.base.read(cx).value().trim().to_owned();
        let base = (!base.is_empty() && base != "HEAD").then_some(base);
        let project = ui.project.clone();
        let agent = ui.agent;
        let machine = self
            .snapshot
            .project(&project)
            .map(|entry| entry.project.machine_id.clone())
            .unwrap_or_else(MachineId::local);
        self.engine.submit(Op::AddWorktree {
            project: project.clone(),
            branch: branch.clone(),
            base,
        });
        if let Some(kind) = agent {
            self.start_agent_when_worktree_appears(project, machine, branch, kind, window, cx);
        }
        let keep_open = self
            .new_worktree_ui
            .as_ref()
            .is_some_and(|ui| ui.create_more);
        if keep_open {
            if let Some(ui) = &self.new_worktree_ui {
                ui.name
                    .clone()
                    .update(cx, |field, cx| field.set_value("", window, cx));
                ui.base
                    .clone()
                    .update(cx, |field, cx| field.set_value("", window, cx));
            }
            if let Some(ui) = self.new_worktree_ui.as_mut() {
                ui.cursor = 0;
            }
            self.focus_worktree_row(window, cx);
            cx.notify();
        } else {
            self.close_overlay(window, cx);
            cx.notify();
        }
    }

    /// Waits for the worktree to be synced and starts `agent` in it, in a
    /// new terminal.
    fn start_agent_when_worktree_appears(
        &mut self,
        project: ProjectId,
        machine: MachineId,
        branch: String,
        agent: AgentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.worktree_task = Some(cx.spawn_in(window, async move |this, cx| {
            for _ in 0..75 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                let found = this.read_with(cx, |this, _| {
                    this.snapshot.project(&project).and_then(|entry| {
                        entry
                            .worktrees
                            .iter()
                            .find(|worktree| worktree.branch.as_deref() == Some(branch.as_str()))
                            .map(|worktree| worktree.path.clone())
                    })
                });
                match found {
                    Ok(Some(path)) => {
                        this.update_in(cx, |this, window, cx| {
                            this.start_live(
                                Launch::Agent {
                                    kind: agent,
                                    resume: None,
                                },
                                &machine,
                                &path,
                                Place::Tab,
                                None,
                                window,
                                cx,
                            );
                        })
                        .ok();
                        return;
                    }
                    Ok(None) => continue,
                    Err(_) => return,
                }
            }
        }));
    }

    /// Whether the "Create from" list is open (and the row has the
    /// keyboard).
    fn base_list_open(&self) -> bool {
        self.new_worktree_ui
            .as_ref()
            .is_some_and(|ui| ui.cursor == 1 && ui.base_list)
    }

    /// The bases on offer, filtered by what is typed: `HEAD` first, then the
    /// branches git reported, in their order. Case is ignored; an empty
    /// query offers them all.
    fn filtered_bases(&self, cx: &gpui_kit::App) -> Vec<String> {
        let Some(ui) = &self.new_worktree_ui else {
            return Vec::new();
        };
        let query = ui.base.read(cx).value().trim().to_lowercase();
        std::iter::once("HEAD".to_owned())
            .chain(ui.base_refs.iter().cloned())
            .filter(|name| query.is_empty() || name.to_lowercase().contains(&query))
            .collect()
    }

    /// The "Create worktree" card, laid out like Orca's: a title with a
    /// close button, a label over every field, a bordered box for each
    /// value, the toggle, and the primary button on the right.
    pub(super) fn render_new_worktree(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(ui) = &self.new_worktree_ui else {
            return div().id("new-worktree-empty");
        };
        let entry = self.snapshot.project(&ui.project);
        let project_name = entry.map_or_else(String::new, |entry| entry.project.name.clone());
        let machine_name = entry
            .and_then(|entry| self.snapshot.machine(&entry.project.machine_id))
            .map_or_else(String::new, |machine| machine.name.clone());
        let root = entry.map_or_else(String::new, |entry| entry.project.root.clone());
        let on = |row: usize| ui.cursor == row;
        // The box a value lives in: a hairline, a filled background, and the
        // accent border while its row has the keyboard.
        let field = |row: usize, colours: &Palette, content: AnyElement| {
            div()
                .w_full()
                .h(metrics::CONTROL())
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if on(row) {
                    colours.signal
                } else {
                    colours.border
                })
                .bg(colours.background)
                .child(content)
                .into_any_element()
        };
        let label = |text: &str, colours: &Palette| {
            div()
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text_muted)
                .child(text.to_owned())
        };
        let row = |row: usize, _colours: &Palette| {
            div()
                .id(("worktree-row", row))
                .debug_selector(move || format!("worktree-row-{row}"))
                .relative()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(5.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(ui) = this.new_worktree_ui.as_mut() {
                        ui.cursor = row;
                    }
                    this.focus_worktree_row(window, cx);
                    cx.notify();
                }))
        };
        let agent_name = ui.agent.map_or_else(
            || "None".to_owned(),
            |agent| format::agent_name(agent).to_owned(),
        );
        let agent_lead: AnyElement = match ui.agent {
            Some(agent) => agent_icon(agent, px(14.), colours).into_any_element(),
            None => div().into_any_element(),
        };
        let tip: SharedString = "Starts the agent in the new worktree".into();
        let mut card = self
            .card("new-worktree", colours)
            .debug_selector(|| "new-worktree".into())
            .w(px(560.))
            .p_4()
            .flex()
            .flex_col()
            .gap(px(14.))
            .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(section_label("Create worktree", colours))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("new-worktree-close")
                            .debug_selector(|| "new-worktree-close".into())
                            .size(metrics::CONTROL())
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(metrics::RADIUS())
                            .cursor_pointer()
                            .hover({
                                let hover = colours.surface_2;
                                move |style| style.bg(hover)
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_overlay(window, cx);
                                cx.notify();
                            }))
                            .child(mono("\u{2715}").text_color(colours.text_muted)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(label("Project", colours))
                            .child(field(
                                9,
                                colours,
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(project_name.clone())
                                    .into_any_element(),
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(label("Run on", colours))
                            .child(field(
                                9,
                                colours,
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(mono(&machine_name).text_color(colours.text_muted))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(metrics::TEXT_SMALL())
                                            .text_color(colours.text_faint)
                                            .child(root.clone()),
                                    )
                                    .into_any_element(),
                            )),
                    ),
            )
            .child(
                row(0, colours).child(label("Name", colours)).child(field(
                    0,
                    colours,
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&ui.name).appearance(false))
                        .into_any_element(),
                )),
            )
            .child(
                row(1, colours)
                    .child(label("Create from [Optional]", colours))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .child(field(
                                1,
                                colours,
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&ui.base).appearance(false))
                                    .into_any_element(),
                            ))
                            .when(!ui.base_refs.is_empty(), |this| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .text_size(metrics::TEXT_SMALL())
                                        .text_color(colours.text_faint)
                                        .child(format!("{} branches \u{25be}", ui.base_refs.len())),
                                )
                            })
                            .when(self.base_list_open(), |this| {
                                let bases = self.filtered_bases(cx);
                                let current = ui.base.read(cx).value().trim().to_owned();
                                let mut list = div()
                                    .id("base-list")
                                    .debug_selector(|| "base-list".into())
                                    .absolute()
                                    .top(metrics::CONTROL() + px(5.))
                                    .left_0()
                                    .right_0()
                                    .max_h(px(180.))
                                    .overflow_y_scroll()
                                    .occlude()
                                    .rounded(metrics::RADIUS())
                                    .border_1()
                                    .border_color(colours.elevated_border)
                                    .bg(colours.surface)
                                    .shadow_md()
                                    .flex()
                                    .flex_col()
                                    .p_1();
                                if bases.is_empty() {
                                    list = list.child(
                                        div()
                                            .px(px(10.))
                                            .py(px(6.))
                                            .text_size(metrics::TEXT_SMALL())
                                            .text_color(colours.text_faint)
                                            .child("No branch matches"),
                                    );
                                }
                                for (at, name) in bases.iter().enumerate() {
                                    let chosen =
                                        at == ui.base_cursor.min(bases.len().saturating_sub(1));
                                    let shown = name.clone();
                                    let value = name.clone();
                                    let is_current = current == *name;
                                    let mut option = div()
                                        .id(("base-option", at))
                                        .debug_selector(move || format!("base-option-{at}"))
                                        .relative()
                                        .h(px(26.))
                                        .px(px(10.))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .rounded(px(3.))
                                        .cursor_pointer()
                                        .when(chosen, |this| {
                                            this.bg(colours.surface_2).child(
                                                div()
                                                    .absolute()
                                                    .left_0()
                                                    .top_0()
                                                    .bottom_0()
                                                    .w(px(2.))
                                                    .bg(colours.signal),
                                            )
                                        })
                                        .hover({
                                            let hover = colours.surface_2;
                                            move |style| style.bg(hover)
                                        })
                                        .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                            if let Some(ui) = this.new_worktree_ui.as_mut() {
                                                if ui.base_cursor != at {
                                                    ui.base_cursor = at;
                                                    cx.notify();
                                                }
                                            }
                                        }))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            // Not the row beneath: it would reopen the list.
                                            cx.stop_propagation();
                                            if let Some(ui) = &this.new_worktree_ui {
                                                ui.base.clone().update(cx, |field, cx| {
                                                    field.set_value(value.clone(), window, cx)
                                                });
                                            }
                                            if let Some(ui) = this.new_worktree_ui.as_mut() {
                                                ui.base_list = false;
                                                ui.base_typed = true;
                                            }
                                            cx.notify();
                                        }))
                                        .child(mono(shown).text_color(colours.text_muted));
                                    if is_current {
                                        option = option
                                            .child(mono("current").text_color(colours.text_faint));
                                    }
                                    list = list.child(option);
                                }
                                this.child(gpui_kit::deferred(list))
                            }),
                    ),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        row(2, colours)
                            .child(label("Agent", colours))
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .child(field(
                                        2,
                                        colours,
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.))
                                            .w_full()
                                            .child(agent_lead)
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .child(agent_name),
                                            )
                                            .child(mono("\u{25be}").text_color(colours.text_faint))
                                            .into_any_element(),
                                    ))
                                    .when(ui.cursor == 2 && ui.agent_list, |this| {
                                        let mut list = div()
                                            .id("agent-list")
                                            .debug_selector(|| "agent-list".into())
                                            .absolute()
                                            .top(metrics::CONTROL() + px(5.))
                                            .left_0()
                                            .right_0()
                                            .occlude()
                                            .rounded(metrics::RADIUS())
                                            .border_1()
                                            .border_color(colours.elevated_border)
                                            .bg(colours.surface)
                                            .shadow_md()
                                            .flex()
                                            .flex_col()
                                            .p_1();
                                        for (at, agent) in agent_choices(cx).iter().enumerate() {
                                            let chosen = at
                                                == ui.agent_cursor.min(agent_choices(cx).len() - 1);
                                            let picked = *agent;
                                            let label = agent.map_or_else(
                                                || "None".to_owned(),
                                                |agent| format::agent_name(agent).to_owned(),
                                            );
                                            let detail = if picked.is_some() {
                                                "starts in the new worktree"
                                            } else {
                                                "just the worktree"
                                            };
                                            list = list.child(
                                                div()
                                                    .id(("agent-option", at))
                                                    .debug_selector(move || {
                                                        format!("agent-option-{at}")
                                                    })
                                                    .relative()
                                                    .h(px(28.))
                                                    .px(px(10.))
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(8.))
                                                    .rounded(px(3.))
                                                    .cursor_pointer()
                                                    .when(chosen, |this| {
                                                        this.bg(colours.surface_2).child(
                                                            div()
                                                                .absolute()
                                                                .left_0()
                                                                .top_0()
                                                                .bottom_0()
                                                                .w(px(2.))
                                                                .bg(colours.signal),
                                                        )
                                                    })
                                                    .hover({
                                                        let hover = colours.surface_2;
                                                        move |style| style.bg(hover)
                                                    })
                                                    .on_mouse_move(cx.listener(
                                                        move |this, _, _, cx| {
                                                            if let Some(ui) =
                                                                this.new_worktree_ui.as_mut()
                                                            {
                                                                if ui.agent_cursor != at {
                                                                    ui.agent_cursor = at;
                                                                    cx.notify();
                                                                }
                                                            }
                                                        },
                                                    ))
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            // Not the row beneath, and not through
                                                            // focus_worktree_row either: opening
                                                            // the list is what just ended.
                                                            cx.stop_propagation();
                                                            if let Some(ui) =
                                                                this.new_worktree_ui.as_mut()
                                                            {
                                                                ui.agent = picked;
                                                                ui.agent_list = false;
                                                            }
                                                            this.focus.focus(window, cx);
                                                            cx.notify();
                                                        },
                                                    ))
                                                    .children(picked.map(|agent| {
                                                        agent_icon(agent, px(14.), colours)
                                                    }))
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .truncate()
                                                            .child(label),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(metrics::TEXT_SMALL())
                                                            .text_color(colours.text_faint)
                                                            .child(detail),
                                                    ),
                                            );
                                        }
                                        this.child(gpui_kit::deferred(list))
                                    }),
                            )
                            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)),
                    ),
            );
        card = card.child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .id("worktree-more")
                        .debug_selector(|| "worktree-more".into())
                        .h(metrics::CONTROL())
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .rounded(metrics::RADIUS())
                        .cursor_pointer()
                        .when(on(3), |this| {
                            this.bg(colours.surface_2).child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(px(2.))
                                    .bg(colours.signal),
                            )
                        })
                        .hover({
                            let hover = colours.surface_2;
                            move |style| style.bg(hover)
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(ui) = this.new_worktree_ui.as_mut() {
                                ui.create_more = !ui.create_more;
                                ui.cursor = 3;
                            }
                            this.focus_worktree_row(window, cx);
                            cx.notify();
                        }))
                        // A small track with its knob, like a switch.
                        .child(
                            div()
                                .relative()
                                .w(px(28.))
                                .h(px(16.))
                                .rounded(px(8.))
                                .bg(if ui.create_more {
                                    colours.signal
                                } else {
                                    colours.border
                                })
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(2.))
                                        .left(if ui.create_more { px(14.) } else { px(2.) })
                                        .size(px(12.))
                                        .rounded(px(6.))
                                        .bg(colours.background),
                                ),
                        )
                        .child(
                            div()
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(colours.text_muted)
                                .child("Create more"),
                        ),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("worktree-create")
                        .debug_selector(|| "worktree-create".into())
                        .h(metrics::CONTROL())
                        .px(px(16.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .rounded(metrics::RADIUS())
                        .cursor_pointer()
                        .when(on(4), |this| this.bg(colours.signal))
                        .when(!on(4), |this| this.bg(colours.surface_2))
                        .text_color(if on(4) {
                            colours.background
                        } else {
                            colours.text
                        })
                        // Pointing at it shows what pressing it does: the
                        // accent of the row the keyboard is on.
                        .hover({
                            let fill = colours.signal;
                            let ink = colours.background;
                            move |style| style.bg(fill).text_color(ink)
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(ui) = this.new_worktree_ui.as_mut() {
                                ui.cursor = 4;
                            }
                            this.submit_new_worktree(window, cx);
                        }))
                        .child("Create worktree")
                        .child(key_cap("\u{21a9}".to_owned(), colours)),
                ),
        );
        card
    }
}

/// What the agent list offers, in order: no agent, then every agent a new
/// session may start.
fn agent_choices(cx: &gpui_kit::App) -> Vec<Option<AgentId>> {
    let mut list = vec![None];
    list.extend(
        super::shell::Shell::step_prefs(cx)
            .offered()
            .into_iter()
            .map(Some),
    );
    list
}

/// The next agent in the cycle, `None` included.
fn next_agent(
    list: &[Option<AgentId>],
    current: Option<AgentId>,
    forward: bool,
) -> Option<AgentId> {
    let at = list.iter().position(|agent| *agent == current).unwrap_or(0);
    let next = if forward {
        (at + 1) % list.len()
    } else {
        (at + list.len() - 1) % list.len()
    };
    list[next]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_cycle_visits_none_and_every_agent() {
        let list: Vec<Option<AgentId>> = vec![
            None,
            Some(AgentId::CLAUDE),
            Some(AgentId::CODEX),
            Some(AgentId::OPENCODE),
        ];
        assert_eq!(next_agent(&list, None, true), Some(AgentId::CLAUDE));
        assert_eq!(
            next_agent(&list, Some(AgentId::CLAUDE), true),
            Some(AgentId::CODEX)
        );
        assert_eq!(next_agent(&list, Some(AgentId::OPENCODE), true), None);
        assert_eq!(next_agent(&list, None, false), Some(AgentId::OPENCODE));
    }
}

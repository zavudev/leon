//! What the shell does about projects as a whole: the sidebar's filter field
//! and the project's logo commands.
//!
//! The filter is a text field above the tree. Its text is looked up in the
//! snapshot already in memory (`filter.rs`), never in the store, and the tree
//! is rebuilt through it; what is open is not changed, the filtered view is
//! open on its own. `/` or `Cmd+F` (`Ctrl+F`) put the keyboard in the field;
//! while it is there Down moves into the tree, Enter opens the best match and
//! Escape clears the text, then returns to the tree.

use super::filter::{self, project_labels};
use super::shell::{Overlay, Pane, PickFolder, Picked, Shell};
use super::tree::Kind;
use crate::engine::{Op, StatusKind};
use gpui_kit::{Context, Focusable as _, Keystroke, PathPromptOptions, Task, Window};
use leon_core::ProjectId;

/// The system's own file dialog, for an image.
pub fn system_image_picker(cx: &mut gpui_kit::App) -> Task<Picked> {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Choose icon".into()),
    });
    cx.spawn(async move |_| match paths.await {
        Ok(Ok(Some(mut paths))) => paths.pop().map_or(Picked::Cancelled, Picked::Folder),
        Ok(Ok(None)) => Picked::Cancelled,
        _ => Picked::Unavailable,
    })
}

/// The default of [`Options::pick_image`](super::shell::Options).
pub fn default_image_picker() -> PickFolder {
    std::rc::Rc::new(system_image_picker)
}

impl Shell {
    // ----- the filter ------------------------------------------------------------------

    /// Whether the keyboard is in the filter field.
    pub(super) fn filter_focused(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.filter_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }

    /// Looks the projects up again: the labels and what the query leaves.
    pub(super) fn refilter(&mut self) {
        self.labels = project_labels(&self.snapshot);
        self.filter = filter::filter(&self.snapshot, &self.labels, &self.filter_query);
    }

    /// The text of the field changed: the tree is filtered again and the
    /// cursor goes to the best match.
    pub(super) fn filter_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.filter_input.read(cx).value().to_string();
        if text == self.filter_query {
            return;
        }
        self.filter_query = text;
        self.refilter();
        self.rebuild_rows();
        let best = self
            .filter
            .as_ref()
            .and_then(|filter| filter.best.clone())
            .and_then(|node| self.rows.iter().position(|row| row.id == node));
        if let Some(index) = best.or_else(|| self.first_match_row()) {
            self.move_cursor_to(index);
        }
        cx.notify();
    }

    /// The first row that is a project or a worktree.
    fn first_match_row(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row.kind, Kind::Project { .. } | Kind::Worktree { .. }))
    }

    /// Turns "show inactive sessions" on or off: the setting is kept,
    /// and the tree is read again at once (the settings screen and the file
    /// do the same through the settings' own sync).
    pub(super) fn toggle_show_inactive(&mut self, cx: &mut Context<Self>) {
        let Some(def) = crate::schema::find("sidebar_show_inactive") else {
            return;
        };
        let on = !crate::settings::flag(cx, "sidebar_show_inactive");
        crate::settings::set_value(cx, def, crate::schema::Value::Bool(on));
        self.sync_settings(cx);
    }

    /// Puts the keyboard in the field.
    pub(super) fn focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(window, cx);
        self.pane = Pane::Sidebar;
        self.filter_input
            .update(cx, |field, cx| field.focus(window, cx));
    }

    /// Takes the keyboard out of the field, back to the tree.
    pub(super) fn leave_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pane = Pane::Sidebar;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Empties the field. The tree is whole again.
    pub(super) fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_input
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.filter_changed(cx);
    }

    /// The keys of the field: Down goes into the tree, Enter opens the best
    /// match, Escape clears the text and then leaves. `true` when taken; any
    /// other key is typed.
    pub(super) fn filter_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if stroke.modifiers.modified() {
            return false;
        }
        match stroke.key.as_str() {
            "escape" => {
                if self.filter_query.is_empty() {
                    self.leave_filter(window, cx);
                } else {
                    self.clear_filter(window, cx);
                }
                true
            }
            "down" => {
                self.leave_filter(window, cx);
                true
            }
            "enter" => {
                let best = self
                    .filter
                    .as_ref()
                    .and_then(|filter| filter.best.clone())
                    .and_then(|node| self.rows.iter().position(|row| row.id == node));
                self.leave_filter(window, cx);
                if let Some(index) = best {
                    self.move_cursor_to(index);
                    self.activate(index, window, cx);
                }
                true
            }
            _ => false,
        }
    }

    // ----- logos -------------------------------------------------------------------------

    /// The project the keyboard is on: its row, or a worktree's project, or
    /// the one open in the main pane.
    pub(super) fn project_here(&self) -> Option<ProjectId> {
        use super::shell::Main;
        let from_row = || match &self.rows.get(self.cursor?)?.kind {
            Kind::Project { project, .. } => Some(project.id.clone()),
            Kind::Worktree { worktree, .. } => Some(worktree.project_id.clone()),
            _ => None,
        };
        let from_main = || match &self.main {
            Main::Project(id) | Main::Worktree(id, _) => Some(id.clone()),
            _ => None,
        };
        if self.pane == Pane::Sidebar {
            from_row().or_else(from_main)
        } else {
            from_main().or_else(from_row)
        }
    }

    /// "Refresh project icon": looks for the logo again.
    pub(super) fn refresh_icon_here(&mut self) {
        match self.project_here() {
            Some(project) => self.engine.submit(Op::DetectIcon(project)),
            None => self
                .engine
                .report(StatusKind::Info, "Select a project first."),
        }
    }

    /// "Reset icon": the detected logo again.
    pub(super) fn reset_icon_here(&mut self) {
        match self.project_here() {
            Some(project) => self.engine.submit(Op::ResetIcon(project)),
            None => self
                .engine
                .report(StatusKind::Info, "Select a project first."),
        }
    }

    /// "Choose icon…": the system's file dialog, then the file becomes the
    /// project's logo.
    pub(super) fn choose_icon_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project_here() else {
            self.engine
                .report(StatusKind::Info, "Select a project first.");
            return;
        };
        if self.overlay != Overlay::None {
            self.close_overlay(window, cx);
        }
        let picking = (self.options.pick_image)(cx);
        self.choosing = Some(cx.spawn_in(window, async move |this, cx| {
            let picked = picking.await;
            this.update(cx, |this, _| match picked {
                Picked::Folder(path) => this.engine.submit(Op::SetIcon { project, path }),
                Picked::Cancelled => {}
                Picked::Unavailable => this.engine.report(
                    StatusKind::Error,
                    "This system has no file dialog to choose an icon with.",
                ),
            })
            .ok();
        }));
    }
}

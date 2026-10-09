//! What is done with a file: open it, save it, read it again, close it.
//!
//! Reading and saving go through the engine, which does them where the file
//! is (this computer or an SSH machine); the window only waits for the answer.
//! A save carries the revision the file had when it was read, so a file that
//! somebody else changed is not overwritten by surprise: the answer is a
//! conflict, and the palette asks what to do (`steps::save_file`).

use super::super::live::LiveId;
use super::super::shell::{Main, Overlay, Pane, Shell};
use super::super::steps::FileInfo;
use super::super::workspace;
use super::document::{image_format, parent_of, resolve, EditorDoc};
use crate::engine::StatusKind;
use crate::files::{FileContent, ImageContent, WriteOutcome};
use crate::keys::Command;
use gpui_kit::component::input::Position;
use gpui_kit::{App, Context, FocusHandle, Focusable as _, Window};
use leon_core::MachineId;

impl Shell {
    /// The file on screen, which has the keyboard when the main pane does.
    pub(in crate::ui) fn focused_file(&self) -> Option<LiveId> {
        match self.main {
            Main::Live(id) if self.files.contains_key(&id) => Some(id),
            _ => None,
        }
    }

    /// Whether an editor has the keyboard: a file is on screen, the main pane
    /// is where the keyboard is and nothing is open over it.
    pub(in crate::ui) fn file_has_keyboard(&self) -> bool {
        self.overlay == Overlay::None && self.pane == Pane::Main && self.focused_file().is_some()
    }

    /// Whether the search bar of the editor of the file on screen has the
    /// keyboard: one of its fields, not the text.
    pub(in crate::ui) fn editor_search_has_keyboard(&self, window: &Window, cx: &App) -> bool {
        if !self.file_has_keyboard() {
            return false;
        }
        let Some(state) = self
            .focused_file()
            .and_then(|id| self.files.get(&id))
            .and_then(EditorDoc::state)
        else {
            return false;
        };
        let handle = state.focus_handle(cx);
        // The bar's fields are not inside the editor's own focus handle:
        // the keyboard is on something that is neither the text nor the
        // shell.
        state.read(cx).search_session().open
            && !handle.is_focused(window)
            && !self.focus.is_focused(window)
            && window.focused(cx).is_some()
    }

    /// The focus handle of the editor of the file on screen.
    pub(in crate::ui) fn focused_file_handle(&self, cx: &App) -> Option<FocusHandle> {
        let state = self.files.get(&self.focused_file()?)?.state()?;
        Some(state.focus_handle(cx))
    }

    /// Whether any editor holds the keyboard.
    pub(in crate::ui) fn any_editor_focused(&self, window: &Window, cx: &App) -> bool {
        self.files
            .values()
            .filter_map(EditorDoc::state)
            .any(|state| state.focus_handle(cx).is_focused(window))
    }

    /// The focused file as the flows know it.
    pub(in crate::ui) fn file_info(&self) -> Option<FileInfo> {
        let id = self.focused_file()?;
        let doc = self.files.get(&id)?;
        Some(FileInfo {
            id,
            name: doc.name().to_owned(),
            dirty: doc.dirty,
            conflict: doc.conflict,
        })
    }

    /// The names of the files with changes that were not saved.
    pub(in crate::ui) fn unsaved_files(&self) -> Vec<String> {
        let mut names: Vec<(LiveId, String)> = self
            .files
            .iter()
            .filter(|(_, doc)| doc.dirty)
            .map(|(id, doc)| (*id, doc.name().to_owned()))
            .collect();
        names.sort_by_key(|(id, _)| id.0);
        names.into_iter().map(|(_, name)| name).collect()
    }

    /// The workspace folder of a file: the project folder it is in, else its
    /// own folder.
    pub(super) fn file_folder(&self, machine: &MachineId, path: &str) -> String {
        super::super::tree::workspace_root(&self.snapshot, machine, parent_of(path))
    }

    /// The leaf that already shows `path` in the workspace of `folder`.
    pub(super) fn file_leaf(
        &self,
        machine: &MachineId,
        path: &str,
        folder: &str,
    ) -> Option<LiveId> {
        let key = workspace::key_of(machine.as_str(), folder);
        self.files
            .iter()
            .filter(|(_, doc)| &doc.machine == machine && doc.path == path)
            .map(|(id, _)| *id)
            .find(|id| {
                self.workspaces
                    .workspace_of(*id)
                    .is_some_and(|workspace| workspace.in_folder(&key))
            })
    }

    /// Opens a file in a tab of its project's workspace, or shows the tab
    /// that already has it. `line` (from 1) is where the cursor goes.
    pub(in crate::ui) fn open_file(
        &mut self,
        machine: MachineId,
        path: String,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = self.file_folder(&machine, &path);
        if let Some(id) = self.file_leaf(&machine, &path, &folder) {
            self.open_live(id, window, cx);
            self.go_to_line(id, line, window, cx);
            return;
        }
        // A picture on this computer is drawn, not read as text.
        if machine.is_local() && image_format(&path).is_some() {
            self.open_image(machine, path, folder, window, cx);
            return;
        }
        let reading = self.engine.read_file(machine.clone(), path.clone());
        cx.spawn_in(window, async move |this, cx| {
            // A failure is already on the status line.
            let Ok(Ok(content)) = reading.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.finish_open(machine, path, folder, content, line, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Reads a picture of this computer and opens it in a tab.
    fn open_image(
        &mut self,
        machine: MachineId,
        path: String,
        folder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reading = read_picture(path.clone(), cx);
        cx.spawn_in(window, async move |this, cx| {
            let answer = reading.await;
            this.update_in(cx, |this, window, cx| match answer {
                Ok(content) => this.finish_open_image(machine, path, folder, content, window, cx),
                Err(error) => this.engine.report(StatusKind::Error, error.to_string()),
            })
            .ok();
        })
        .detach();
    }

    fn finish_open_image(
        &mut self,
        machine: MachineId,
        path: String,
        folder: String,
        content: ImageContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Asked twice before the first answer came: one tab is enough.
        if let Some(id) = self.file_leaf(&machine, &path, &folder) {
            self.open_live(id, window, cx);
            return;
        }
        let id = self.live.next_id();
        let doc = EditorDoc::open_image(machine, path, folder, content);
        let (machine, folder) = (doc.machine.clone(), doc.folder.clone());
        self.files.insert(id, doc);
        self.workspaces
            .add_folder_tab(machine.as_str(), &folder, id);
        self.open_live(id, window, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_open(
        &mut self,
        machine: MachineId,
        path: String,
        folder: String,
        content: FileContent,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Asked twice before the first answer came: one tab is enough.
        if let Some(id) = self.file_leaf(&machine, &path, &folder) {
            self.open_live(id, window, cx);
            self.go_to_line(id, line, window, cx);
            return;
        }
        let id = self.live.next_id();
        let mut doc = EditorDoc::open(machine, path, folder, content, id, window, cx);
        self.restore_draft_of(&mut doc, window, cx);
        let (machine, folder) = (doc.machine.clone(), doc.folder.clone());
        self.files.insert(id, doc);
        self.workspaces
            .add_folder_tab(machine.as_str(), &folder, id);
        self.open_live(id, window, cx);
        self.go_to_line(id, line, window, cx);
    }

    /// Opens the search bar of the file on screen, for replacing too when
    /// `replace`, with the keyboard in it.
    pub(in crate::ui) fn find_in_file(
        &mut self,
        replace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self
            .focused_file()
            .and_then(|id| self.files.get(&id))
            .and_then(EditorDoc::state)
            .cloned();
        let Some(state) = state else {
            self.engine
                .report(StatusKind::Info, "There is no text file to search in.");
            return;
        };
        self.pane = Pane::Main;
        state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.open_search(replace, cx);
        });
        cx.notify();
    }

    /// Puts the cursor of a file at the start of `line` (from 1) and brings
    /// the line into view. An editor that was just made has not been laid out,
    /// and one that has not been laid out cannot scroll, so the cursor is put
    /// there again two frames later, when it has.
    pub(super) fn go_to_line(
        &mut self,
        id: LiveId,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(line) = line else { return };
        let Some(state) = self.files.get(&id).and_then(|doc| doc.state()).cloned() else {
            return;
        };
        let at = Position::new(line.saturating_sub(1), 0);
        state.update(cx, |state, cx| state.set_cursor_position(at, window, cx));
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |window, cx| {
                state.update(cx, |state, cx| state.set_cursor_position(at, window, cx));
            });
        });
    }

    /// The path a person typed, opened on the machine and in the folder the
    /// keyboard is in.
    pub(in crate::ui) fn open_typed_file(
        &mut self,
        typed: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self.here().map(|place| place.cwd).unwrap_or_default();
        let machine = self.current_machine();
        let path = resolve(&cwd, typed);
        if path.is_empty() {
            return;
        }
        self.open_file(machine, path, None, window, cx);
    }

    /// The text of a file changed: whether it still differs from what was
    /// saved is looked at after a pause, so typing does not hash it every
    /// key.
    pub(in crate::ui) fn editor_changed(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let pause = self.options.editor_debounce;
        let Some(doc) = self.files.get_mut(&id) else {
            return;
        };
        doc.check = Some(cx.spawn(async move |this, cx| {
            if !pause.is_zero() {
                cx.background_executor().timer(pause).await;
            }
            this.update(cx, |this, cx| {
                if let Some(doc) = this.files.get_mut(&id) {
                    doc.refresh_dirty(cx);
                }
                cx.notify();
            })
            .ok();
        }));
        // The page, if it shows, follows the text.
        self.refresh_preview(id, false, cx);
        self.schedule_draft(id, cx);
    }

    // ----- saving --------------------------------------------------------------------

    /// Saves the file on screen (the chord and the menu).
    pub(in crate::ui) fn save_file_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.focused_file() {
            Some(_) => self.begin_flow(Command::SaveFile, window, cx),
            None => self
                .engine
                .report(StatusKind::Info, "There is no file to save."),
        }
    }

    /// Saves a file; with `then_close` it is closed once it is saved.
    pub(in crate::ui) fn save_file(
        &mut self,
        id: LiveId,
        then_close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.files.get_mut(&id) else {
            return;
        };
        let (Some(contents), Some(body)) = (doc.contents(cx), doc.text(cx)) else {
            self.engine
                .report(StatusKind::Info, "This file is not text: nothing to save.");
            return;
        };
        if doc.saving {
            return;
        }
        // Nothing changed: nothing is written, and the file may close.
        if !doc.dirty && !doc.conflict && doc.revision.is_some() {
            doc.refresh_dirty(cx);
            if !doc.dirty {
                if then_close {
                    self.close_file(id, window, cx);
                }
                return;
            }
        }
        doc.saving = true;
        doc.close_after_save = then_close;
        let writing = self.engine.write_file(
            doc.machine.clone(),
            doc.path.clone(),
            contents,
            doc.revision.clone(),
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = writing.await;
            this.update_in(cx, |this, window, cx| {
                this.finish_save(id, &body, answer, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_save(
        &mut self,
        id: LiveId,
        body: &str,
        answer: Result<Result<WriteOutcome, crate::engine::EngineError>, tokio::task::JoinError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.files.get_mut(&id) else {
            self.continue_quit(cx);
            return;
        };
        doc.saving = false;
        match answer {
            Ok(Ok(WriteOutcome::Saved(revision))) => {
                doc.saved_as(body, revision, cx);
                self.remove_draft(id, cx);
                // A saved file changes what git says about it.
                self.files_refresh(cx);
                let Some(doc) = self.files.get_mut(&id) else {
                    return;
                };
                if std::mem::take(&mut doc.close_after_save) && !doc.dirty {
                    self.close_file(id, window, cx);
                }
            }
            Ok(Ok(WriteOutcome::Conflict)) => {
                doc.conflict = true;
                self.open_live(id, window, cx);
                self.begin_flow(Command::SaveFile, window, cx);
            }
            // The engine said what went wrong on the status line.
            _ => doc.close_after_save = false,
        }
        self.continue_quit(cx);
        cx.notify();
    }

    /// Saves over what somebody else wrote: the file is read for the
    /// revision it has now, which the save then expects.
    pub(in crate::ui) fn overwrite_file(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.files.get(&id) else {
            return;
        };
        let reading = self.engine.read_file(doc.machine.clone(), doc.path.clone());
        let then_close = doc.close_after_save;
        cx.spawn_in(window, async move |this, cx| {
            let revision = match reading.await {
                Ok(Ok(FileContent::Text { revision, .. })) => Some(revision),
                // Not text any more: there is nothing safe to write over.
                Ok(Ok(_)) => {
                    this.update(cx, |this, _| {
                        this.engine.report(
                            StatusKind::Error,
                            "The file on disk is no longer text: it was not overwritten.",
                        )
                    })
                    .ok();
                    return;
                }
                // The file is gone (or cannot be read): saving creates it
                // again, and refuses if something appears there meanwhile.
                _ => None,
            };
            this.update_in(cx, |this, window, cx| {
                if let Some(doc) = this.files.get_mut(&id) {
                    doc.revision = revision;
                    doc.conflict = false;
                    doc.external = None;
                }
                this.save_file(id, then_close, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Reads a file again; what is in the editor is replaced.
    pub(in crate::ui) fn reload_file(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.files.get_mut(&id) else {
            return;
        };
        doc.close_after_save = false;
        let reading = self.engine.read_file(doc.machine.clone(), doc.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(content)) = reading.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                if let Some(doc) = this.files.get_mut(&id) {
                    doc.reloaded(content, window, cx);
                }
                this.remove_draft(id, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `Open anyway` on a file that is not text or is too big: its first
    /// bytes, read on this computer, as lossy text in an editor that cannot
    /// change them.
    pub(in crate::ui) fn open_anyway(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.files.get(&id) else {
            return;
        };
        if !doc.machine.is_local() {
            self.engine.report(
                StatusKind::Info,
                "Open anyway works for files on this computer only.",
            );
            return;
        }
        let path = std::path::PathBuf::from(&doc.path);
        let reading = cx.background_executor().spawn(async move {
            use std::io::Read as _;
            let file = std::fs::File::open(&path)?;
            let size = file.metadata()?.len();
            let mut bytes = Vec::new();
            file.take(super::document::LOSSY_LIMIT as u64)
                .read_to_end(&mut bytes)?;
            Ok::<_, std::io::Error>((bytes, size))
        });
        cx.spawn_in(window, async move |this, cx| {
            let answer = reading.await;
            this.update_in(cx, |this, window, cx| match answer {
                Ok((bytes, size)) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    if let Some(doc) = this.files.get_mut(&id) {
                        doc.show_lossy(&text, bytes.len() as u64, size, window, cx);
                    }
                    cx.notify();
                }
                Err(error) => this
                    .engine
                    .report(StatusKind::Error, format!("Cannot read the file: {error}.")),
            })
            .ok();
        })
        .detach();
    }

    // ----- closing -------------------------------------------------------------------

    /// Closes the file on screen, asking first when it has unsaved changes.
    pub(in crate::ui) fn close_file_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.focused_file() {
            Some(_) => self.begin_flow(Command::CloseFile, window, cx),
            None => self
                .engine
                .report(StatusKind::Info, "There is no file to close."),
        }
    }

    /// Closes a file's pane, its changes (if any) thrown away. The pane's
    /// sibling takes the room; the last pane of the workspace returns the
    /// main pane to what the folder shows.
    pub(in crate::ui) fn close_file(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remove_draft(id, cx);
        let Some(doc) = self.files.remove(&id) else {
            return;
        };
        let was_open = matches!(self.main, Main::Live(open) if open == id);
        let closed = self.workspaces.close(id);
        self.after_close(closed, was_open, &doc.machine, &doc.folder, window, cx);
    }
}

/// The bytes of a picture of this computer, read off the UI thread.
pub(super) fn read_picture(
    path: String,
    cx: &gpui_kit::App,
) -> gpui_kit::Task<Result<ImageContent, crate::files::FileError>> {
    cx.background_executor()
        .spawn(async move { crate::files::read_image(std::path::Path::new(&path)) })
}

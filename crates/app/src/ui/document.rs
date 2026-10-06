//! Markdown documents: opening one, reading it rendered or editing it, and
//! saving it back where it lives.
//!
//! The pane shows one [`Document`] at a time. Reading and writing go through
//! the engine's file operations (local files directly, files on other
//! machines with one command each); the UI never touches another machine
//! itself. A document that was edited and left without saving keeps its text
//! as a draft, so browsing away never loses it: reopening the file brings the
//! draft back, marked unsaved.
//!
//! Rendering is the component library's rich text view (`TextView`):
//! headings, lists, tables, quotes, task lists, code, links and images, with
//! links opening in the system's browser. Editing is its code editor
//! (`EditorState`): one text, undo, selection, clipboard and the interface
//! theme's colours. `Cmd`/`Ctrl+S` writes the file; `e` switches between the
//! rendered view and the editor, and `Esc` leaves the editor.

use super::shell::{DocMode, Document, Main, Pane, Shell};
use crate::engine::{EngineError, ReadFile};
use crate::format;
use crate::keys::Command;
use crate::theme::{metrics, Palette};
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, Context, Window};
use leon_core::MachineId;

/// Whether a path names a document Leon opens. The extension decides,
/// whatever its case.
pub fn is_markdown(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && matches!(
            extension.to_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd"
        )
}

/// What a document is called where it is: the last part of its path.
pub fn name_of(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_owned()
}

/// The folder a document lives in, as a path on its machine.
pub fn folder_of(path: &str) -> Option<String> {
    let cut = path.trim_end_matches(['/', '\\']).rfind(['/', '\\'])?;
    Some(if cut == 0 {
        "/".to_owned()
    } else {
        path[..cut].to_owned()
    })
}

/// The key a draft is kept under: one file of one machine.
fn draft_key(machine: &MachineId, path: &str) -> String {
    format!("{}\u{1f}{path}", machine.as_str())
}

impl Shell {
    /// Opens (or re-opens) the file `path` on `machine` in the main pane.
    /// The text on disk is read for its revision; unsaved changes left over
    /// from an earlier visit are shown instead of it.
    pub(super) fn open_document(
        &mut self,
        machine: MachineId,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let same = matches!(
            &self.main,
            Main::Document(document) if document.machine == machine && document.path == path
        );
        if !same {
            self.leave_document(cx);
        }
        let machine_name = self.snapshot.machine(&machine).map_or_else(
            || machine.as_str().to_owned(),
            |machine| machine.name.clone(),
        );
        let draft = self.drafts.get(&draft_key(&machine, &path)).cloned();
        let rendered = cx.new(|cx| TextViewState::markdown("", cx));
        if let Some(draft) = &draft {
            let draft = draft.clone();
            rendered.update(cx, |state, cx| state.set_text(&draft, cx));
        }
        self.main = Main::Document(Box::new(Document {
            machine: machine.clone(),
            machine_name,
            path: path.clone(),
            contents: String::new(),
            draft: draft.clone(),
            revision: None,
            loaded: false,
            mode: DocMode::Rendered,
            rendered,
            editor: None,
            changes: None,
            dirty: draft.is_some(),
            saving: false,
            failed: None,
        }));
        self.pane = Pane::Main;
        self.document_seq += 1;
        let seq = self.document_seq;
        let read = self.engine.read_file(machine, path);
        self.document_task = Some(cx.spawn(async move |this, cx| {
            let outcome = match read.await {
                Ok(read) => read,
                // The job was cancelled: another document was asked for.
                Err(error) => Err(EngineError::File(error.to_string())),
            };
            this.update(cx, |shell, cx| shell.document_read(seq, outcome, cx))
                .ok();
        }));
        cx.notify();
    }

    /// The read of a document came back: its text is shown, or the reason it
    /// cannot be is.
    fn document_read(
        &mut self,
        seq: u64,
        outcome: Result<ReadFile, EngineError>,
        cx: &mut Context<Self>,
    ) {
        if seq != self.document_seq {
            return;
        }
        let key = match &self.main {
            Main::Document(document) => Some(draft_key(&document.machine, &document.path)),
            _ => None,
        };
        let Some(key) = key else {
            return;
        };
        match outcome {
            Ok(read) => {
                self.drafts.remove(&key);
                let Main::Document(document) = &mut self.main else {
                    return;
                };
                document.contents = read.contents;
                document.revision = read.revision;
                document.loaded = true;
                // A draft left from an earlier visit is what is on screen;
                // the file on disk only supplied the revision a save checks.
                if let Some(draft) = document.draft.clone() {
                    document.dirty = true;
                    document
                        .rendered
                        .update(cx, |state, cx| state.set_text(&draft, cx));
                }
            }
            Err(error) => {
                if let Main::Document(document) = &mut self.main {
                    document.failed = Some(error.to_string());
                }
            }
        }
        cx.notify();
    }

    /// The text the document would save right now.
    pub(super) fn document_text(&self, document: &Document, cx: &Context<Self>) -> String {
        match (&document.mode, &document.editor) {
            (DocMode::Editing, Some(editor)) => editor.read(cx).value().to_string(),
            _ => document
                .draft
                .clone()
                .unwrap_or_else(|| document.contents.clone()),
        }
    }

    /// Saves the open document where it lives, refusing when somebody else
    /// changed the file since it was read.
    pub(super) fn save_document(&mut self, cx: &mut Context<Self>) {
        let (text, machine, path, revision) = match &self.main {
            Main::Document(document) if document.loaded && document.failed.is_none() => (
                self.document_text(document, cx),
                document.machine.clone(),
                document.path.clone(),
                document.revision.clone(),
            ),
            Main::Document(_) => {
                self.engine.report(
                    crate::engine::StatusKind::Info,
                    "Nothing was read from this file: there is nothing to save.",
                );
                return;
            }
            _ => {
                self.engine.report(
                    crate::engine::StatusKind::Info,
                    "Open a document first: there is nothing to save.",
                );
                return;
            }
        };
        self.document_seq += 1;
        let seq = self.document_seq;
        let saved = text.clone();
        let write = self.engine.write_file(machine, path, text, revision);
        if let Main::Document(document) = &mut self.main {
            document.saving = true;
        }
        self.document_task = Some(cx.spawn(async move |this, cx| {
            let outcome = match write.await {
                Ok(written) => written,
                Err(error) => Err(EngineError::File(error.to_string())),
            };
            this.update(cx, |shell, cx| {
                shell.document_saved(seq, saved, outcome, cx)
            })
            .ok();
        }));
        cx.notify();
    }

    fn document_saved(
        &mut self,
        seq: u64,
        text: String,
        outcome: Result<Option<String>, EngineError>,
        cx: &mut Context<Self>,
    ) {
        if seq != self.document_seq {
            return;
        }
        let Main::Document(document) = &mut self.main else {
            return;
        };
        document.saving = false;
        match outcome {
            Ok(revision) => {
                document.contents = text;
                document.draft = None;
                document.dirty = false;
                document.revision = revision;
                let name = name_of(&document.path);
                self.engine
                    .report(crate::engine::StatusKind::Info, format!("Saved {name}."));
            }
            Err(error) => {
                let text = error.to_string();
                self.engine.report(crate::engine::StatusKind::Error, text);
                // Keep the edited text: the file did not change, so the
                // change is not lost, and the next save can try again.
                document.dirty = true;
            }
        }
        cx.notify();
    }

    /// Switches between the rendered view and the editor, carrying the text
    /// across and focusing whichever is shown.
    pub(super) fn toggle_document_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editing = matches!(
            &self.main,
            Main::Document(document) if matches!(document.mode, DocMode::Editing)
        );
        if !matches!(&self.main, Main::Document(_)) {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "Open a Markdown file first: there is nothing to edit.",
            );
            return;
        }
        if matches!(&self.main, Main::Document(document) if !document.loaded) {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "The document is still being read.",
            );
            return;
        }
        if editing {
            self.leave_editor(window, cx);
        } else {
            self.enter_editor(window, cx);
        }
        cx.notify();
    }

    /// Rendered to editor: makes the editor, or brings back the one this
    /// document already has, and puts the keyboard in it.
    fn enter_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (contents, draft, existing) = match &mut self.main {
            Main::Document(document) => (
                document.contents.clone(),
                document.draft.clone(),
                document.editor.clone(),
            ),
            _ => return,
        };
        let text = draft.unwrap_or(contents);
        let editor = match existing {
            Some(editor) => editor,
            None => {
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .default_value(text.clone())
                        .placeholder("Write the document here.")
                });
                // Any edit makes the document unsaved. The subscription lives
                // in the document: leaving it drops the subscription with it.
                let changes =
                    cx.subscribe(&editor, |shell: &mut Shell, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            if let Main::Document(document) = &mut shell.main {
                                document.dirty = true;
                            }
                            cx.notify();
                        }
                    });
                if let Main::Document(document) = &mut self.main {
                    document.changes = Some(changes);
                    document.editor = Some(editor.clone());
                }
                editor
            }
        };
        if let Main::Document(document) = &mut self.main {
            document.mode = DocMode::Editing;
        }
        editor.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Editor to rendered: the text crosses over, and unsaved changes become
    /// the draft the rendered view shows.
    fn leave_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = match &self.main {
            Main::Document(document) => match &document.editor {
                Some(editor) => editor.read(cx).value().to_string(),
                None => document.contents.clone(),
            },
            _ => return,
        };
        if let Main::Document(document) = &mut self.main {
            document.mode = DocMode::Rendered;
            document.dirty = text != document.contents;
            document.draft = document.dirty.then(|| text.clone());
            document
                .rendered
                .update(cx, |state, cx| state.set_text(&text, cx));
        }
        self.focus.focus(window, cx);
    }

    /// Leaves the open document, if any: unsaved changes become a draft that
    /// reopening brings back.
    pub(super) fn leave_document(&mut self, cx: &mut Context<Self>) {
        let Some(document) = (match &self.main {
            Main::Document(document) if document.loaded && document.dirty => {
                Some(document.as_ref())
            }
            _ => None,
        }) else {
            return;
        };
        let key = draft_key(&document.machine, &document.path);
        let name = name_of(&document.path);
        let text = self.document_text(document, cx);
        self.drafts.insert(key, text);
        self.engine.report(
            crate::engine::StatusKind::Info,
            format!("{name} has unsaved changes; they will be back when you open it again."),
        );
    }

    /// How many documents with unsaved changes the application holds: one
    /// open, plus the drafts left from earlier visits.
    pub(super) fn unsaved_documents(&self) -> usize {
        let open = matches!(&self.main, Main::Document(document) if document.dirty);
        let open_key = match &self.main {
            Main::Document(document) if document.dirty => {
                Some(draft_key(&document.machine, &document.path))
            }
            _ => None,
        };
        self.drafts
            .keys()
            .filter(|draft| Some(*draft) != open_key.as_ref())
            .count()
            + usize::from(open)
    }

    /// The main pane's document body: the rendered long text, the editor, or
    /// the reason either is missing.
    pub(super) fn render_document(&self, document: &Document, colours: &Palette) -> AnyElement {
        if let Some(why) = &document.failed {
            return div()
                .debug_selector(|| "document-failed".into())
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_color(colours.error).child(why.clone()))
                .child(
                    div()
                        .text_color(colours.text_muted)
                        .child("Check that the file is still there and is text."),
                )
                .into_any_element();
        }
        if !document.loaded {
            return div()
                .debug_selector(|| "document-loading".into())
                .p_4()
                .child(super::widgets::mono("LOADING\u{2026}").text_color(colours.text_muted))
                .into_any_element();
        }
        match document.mode {
            DocMode::Rendered => div()
                .debug_selector(|| "document".into())
                .size_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .debug_selector(|| "document-rendered".into())
                        .w(metrics::DOCUMENT_WIDTH())
                        .max_w_full()
                        .h_full()
                        .child(
                            TextView::new(&document.rendered)
                                .scrollable(true)
                                .selectable(true),
                        ),
                )
                .into_any_element(),
            DocMode::Editing => match &document.editor {
                Some(editor) => div()
                    .debug_selector(|| "document".into())
                    .size_full()
                    .child(
                        div()
                            .debug_selector(|| "document-editor".into())
                            .size_full()
                            .child(Editor::new(editor).appearance(false).bordered(false)),
                    )
                    .into_any_element(),
                None => div().into_any_element(),
            },
        }
    }

    /// The document's header line: what it is, its size, and where it lives.
    pub(super) fn document_meta(&self, document: &Document) -> String {
        let size = if document.loaded {
            format::size(document.contents.len() as u64)
        } else {
            "\u{2026}".to_owned()
        };
        let mut meta = format!("MD \u{b7} {size} \u{b7} {}", document.machine_name);
        if document.dirty {
            meta.push_str(" \u{b7} UNSAVED");
        }
        if document.saving {
            meta.push_str(" \u{b7} SAVING\u{2026}");
        }
        meta.push_str(&format!(" \u{b7} {}", document.path));
        meta
    }

    /// "Open file…": the system's file dialog, then the file, rendered.
    pub(super) fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay != super::shell::Overlay::None {
            self.close_overlay(window, cx);
        }
        let picking = (self.options.pick_file)(cx);
        self.picking = Some(cx.spawn_in(window, async move |this, cx| {
            let picked = picking.await;
            this.update(cx, |shell, cx| shell.file_picked(picked, cx))
                .ok();
        }));
    }

    /// What the file dialog answered.
    fn file_picked(&mut self, picked: super::shell::Picked, cx: &mut Context<Self>) {
        use super::shell::Picked;
        match picked {
            Picked::File(path) | Picked::Folder(path) => {
                let path = path.to_string_lossy().into_owned();
                if !is_markdown(&path) {
                    self.engine.report(
                        crate::engine::StatusKind::Info,
                        format!(
                            "{} is not a Markdown file; Leon opens .md files.",
                            name_of(&path)
                        ),
                    );
                    return;
                }
                self.open_document(MachineId::local(), path, cx);
            }
            Picked::Cancelled => {}
            Picked::Unavailable => self.engine.report(
                crate::engine::StatusKind::Error,
                "This system has no file dialog to open a document with.",
            ),
        }
    }

    /// Files dropped on the window: a Markdown file opens. A drop on a
    /// terminal is the terminal's (it pastes the paths) and never gets here.
    pub(super) fn open_dropped(
        &mut self,
        dropped: &gpui_kit::ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = dropped.paths().first() else {
            return;
        };
        let path = path.to_string_lossy().into_owned();
        if !is_markdown(&path) {
            self.engine.report(
                crate::engine::StatusKind::Info,
                format!(
                    "{} is not a Markdown file; Leon opens .md files.",
                    name_of(&path)
                ),
            );
            return;
        }
        self.pane = Pane::Main;
        self.open_document(MachineId::local(), path, cx);
        let _ = window;
    }
}

/// Moves through the rendered document with the pane's navigation keys. The
/// editor answers those keys itself; `false` leaves the command alone.
pub(super) fn scroll_document(
    document: &Document,
    command: Command,
    page: gpui_kit::Pixels,
    cx: &Context<Shell>,
) -> bool {
    if !matches!(document.mode, DocMode::Rendered) {
        return false;
    }
    use Command as C;
    let list = document.rendered.read(cx).list_state();
    match command {
        C::Down => list.scroll_by(gpui_kit::px(48.)),
        C::Up => list.scroll_by(gpui_kit::px(-48.)),
        C::PageDown => list.scroll_by(page),
        C::PageUp => list.scroll_by(-page),
        C::Top => list.scroll_to(gpui_kit::ListOffset {
            item_ix: 0,
            offset_in_item: gpui_kit::px(0.),
        }),
        C::Bottom => {
            let last = list.item_count().saturating_sub(1);
            list.scroll_to(gpui_kit::ListOffset {
                item_ix: last,
                offset_in_item: gpui_kit::px(0.),
            });
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_extensions_are_recognised_in_any_case() {
        for path in [
            "README.md",
            "docs/NOTES.MD",
            "a/b/notes.Markdown",
            "plan.mdown",
            "x.mkd",
        ] {
            assert!(is_markdown(path), "{path}");
        }
        for path in ["README", "README.txt", "notes.md.bak", ".md", "dir.md/file"] {
            assert!(!is_markdown(path), "{path}");
        }
    }

    #[test]
    fn a_document_is_named_by_the_last_part_of_its_path() {
        assert_eq!(name_of("/home/dev/api/README.md"), "README.md");
        assert_eq!(name_of("docs\\NOTES.md"), "NOTES.md");
        assert_eq!(name_of("README.md"), "README.md");
        assert_eq!(name_of("/a/b/"), "b");
    }

    #[test]
    fn a_documents_folder_is_its_parent() {
        assert_eq!(
            folder_of("/home/dev/api/README.md").as_deref(),
            Some("/home/dev/api")
        );
        assert_eq!(folder_of("/README.md").as_deref(), Some("/"));
        assert_eq!(folder_of("README.md"), None);
    }
}

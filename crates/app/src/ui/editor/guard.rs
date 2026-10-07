//! What keeps a person's text from being lost: drafts, the file changing
//! under the editor, and quitting with unsaved files.
//!
//! * **Drafts** ([`super::drafts`]): a pause after the last change, and again
//!   as the application ends, the text of a file with unsaved changes is
//!   written beside the settings. Saving, discarding and reloading remove it;
//!   opening the file finds it and puts the text back.
//! * **Changes on disk.** When the window comes to the front every open text
//!   file is read again (quietly) and its revision compared with the one it
//!   was read with. A file without unsaved changes is replaced by the new
//!   text at once, its cursor where it was; one with changes gets a banner
//!   above the editor that asks `Reload` or `Keep mine`, and nothing is
//!   touched until the person answers. A save always compares the revision
//!   again where the file is, so a change that landed after the look is still
//!   caught (`WriteOutcome::Conflict`).
//! * **Quitting** with unsaved files is asked by the `Quit` flow
//!   (`steps::quit`): save all, quit without saving, or cancel.

use super::super::live::LiveId;
use super::super::shell::Shell;
use super::document::EditorDoc;
use super::drafts::{self, Draft};
use crate::engine::StatusKind;
use crate::files::FileContent;
use crate::settings;
use gpui_kit::{App, Context, Window};
use std::path::PathBuf;

impl Shell {
    /// Where drafts are kept; `None` when the settings live in memory only.
    fn drafts_dir(cx: &App) -> Option<PathBuf> {
        settings::sibling("drafts", cx)
    }

    /// The draft of a document as it is now, when it has unsaved changes.
    fn draft_of(doc: &EditorDoc, cx: &App) -> Option<Draft> {
        if !doc.dirty {
            return None;
        }
        Some(Draft {
            machine: doc.machine.as_str().to_owned(),
            path: doc.path.clone(),
            text: doc.text(cx)?,
            revision: doc.revision.as_ref().map(|r| r.as_str().to_owned()),
        })
    }

    /// The text of a file changed: its draft is written after a pause.
    pub(in crate::ui) fn schedule_draft(&mut self, id: LiveId, cx: &mut Context<Self>) {
        if Self::drafts_dir(cx).is_none() {
            return;
        }
        let pause = self.options.draft_debounce;
        let Some(doc) = self.files.get_mut(&id) else {
            return;
        };
        doc.draft_task = Some(cx.spawn(async move |this, cx| {
            if !pause.is_zero() {
                cx.background_executor().timer(pause).await;
            }
            this.update(cx, |this, cx| this.write_draft(id, cx)).ok();
        }));
    }

    /// Writes the draft of a file, or removes it when the text is what was
    /// saved again (undone back to clean).
    fn write_draft(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(dir) = Self::drafts_dir(cx) else {
            return;
        };
        let Some(doc) = self.files.get_mut(&id) else {
            return;
        };
        doc.refresh_dirty(cx);
        match Self::draft_of(doc, cx) {
            Some(draft) => {
                cx.background_executor()
                    .spawn(async move {
                        if let Err(error) = drafts::write(&dir, &draft) {
                            tracing::warn!(%error, "a draft could not be written");
                        }
                    })
                    .detach();
            }
            None => drafts::remove(&dir, doc.machine.as_str(), &doc.path),
        }
    }

    /// Removes the draft of a file: it was saved, thrown away or read again.
    pub(in crate::ui) fn remove_draft(&mut self, id: LiveId, cx: &App) {
        let (Some(dir), Some(doc)) = (Self::drafts_dir(cx), self.files.get(&id)) else {
            return;
        };
        drafts::remove(&dir, doc.machine.as_str(), &doc.path);
    }

    /// Writes the drafts of every file with unsaved changes, now: the
    /// application is ending.
    pub(in crate::ui) fn flush_drafts(&self, cx: &App) {
        let Some(dir) = Self::drafts_dir(cx) else {
            return;
        };
        for doc in self.files.values() {
            // The flag is a pause behind the text; the text is what counts.
            let Some(text) = doc.text(cx) else { continue };
            if super::document::fingerprint(&text) == doc.saved {
                continue;
            }
            let draft = Draft {
                machine: doc.machine.as_str().to_owned(),
                path: doc.path.clone(),
                text,
                revision: doc.revision.as_ref().map(|r| r.as_str().to_owned()),
            };
            if let Err(error) = drafts::write(&dir, &draft) {
                tracing::warn!(%error, "a draft could not be written");
            }
        }
    }

    /// A file was just read: if it has a draft, the draft's text is put back
    /// as unsaved changes.
    pub(in crate::ui) fn restore_draft_of(
        &mut self,
        doc: &mut EditorDoc,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = Self::drafts_dir(cx) else {
            return;
        };
        let Some(draft) = drafts::read(&dir, doc.machine.as_str(), &doc.path) else {
            return;
        };
        if doc.text(cx).as_deref() == Some(draft.text.as_str()) {
            // The file already holds it: the draft has nothing to add.
            drafts::remove(&dir, doc.machine.as_str(), &doc.path);
            return;
        }
        if doc.state().is_none() {
            return;
        }
        doc.restore_draft(&draft, window, cx);
        let name = doc.name().to_owned();
        if doc.conflict {
            // The banner says it; a save asks what to do with the two.
            doc.external = doc.revision.clone();
            self.engine.report(
                StatusKind::Info,
                format!(
                    "Restored the unsaved changes to {name}, but the file changed on disk since."
                ),
            );
        } else {
            self.engine.report(
                StatusKind::Info,
                format!("Restored the unsaved changes to {name}."),
            );
        }
    }

    /// Gives the open files the editor settings when any setting changed.
    pub(in crate::ui) fn sync_editor_prefs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let generation = settings::generation(cx);
        if generation == self.applied_editor_settings {
            return;
        }
        self.applied_editor_settings = generation;
        for doc in self.files.values() {
            doc.apply_prefs(window, cx);
        }
    }

    // ----- changes on disk -----------------------------------------------------------------

    /// Reads every open text file again and compares revisions (see the
    /// module documentation).
    pub(in crate::ui) fn check_external_changes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<LiveId> = self
            .files
            .iter()
            .filter(|(_, doc)| doc.state().is_some() && !doc.saving)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let Some(doc) = self.files.get(&id) else {
                continue;
            };
            let reading = self
                .engine
                .check_file(doc.machine.clone(), doc.path.clone());
            cx.spawn_in(window, async move |this, cx| {
                // A file that is gone or unreachable is not a change to show.
                let Ok(Ok(content)) = reading.await else {
                    return;
                };
                this.update_in(cx, |this, window, cx| {
                    this.apply_external(id, content, window, cx)
                })
                .ok();
            })
            .detach();
        }
    }

    fn apply_external(
        &mut self,
        id: LiveId,
        content: FileContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let FileContent::Text { revision, .. } = &content else {
            return;
        };
        let Some(doc) = self.files.get_mut(&id) else {
            return;
        };
        if doc.saving {
            return;
        }
        if doc.revision.as_ref() == Some(revision) {
            // Back to what was read: the banner has nothing to say.
            if !doc.conflict {
                doc.external = None;
            }
            cx.notify();
            return;
        }
        doc.refresh_dirty(cx);
        if doc.dirty {
            doc.external = Some(revision.clone());
        } else {
            let name = doc.name().to_owned();
            doc.reloaded(content, window, cx);
            self.remove_draft(id, cx);
            self.engine.report(
                StatusKind::Info,
                format!("{name} changed on disk and was read again."),
            );
        }
        cx.notify();
    }

    /// `Keep mine` on the banner: the text in the editor is what the file
    /// will hold, whatever is on disk now.
    pub(in crate::ui) fn keep_mine(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(doc) = self.files.get_mut(&id) else {
            return;
        };
        if let Some(revision) = doc.external.take() {
            doc.revision = Some(revision);
        }
        doc.conflict = false;
        cx.notify();
    }

    // ----- quitting ------------------------------------------------------------------------

    /// "Save all and quit": every file with changes is saved, and the
    /// application ends when the last save is done; one that cannot be saved
    /// (it changed on disk, the disk refused) keeps it open.
    pub(in crate::ui) fn save_all_and_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut dirty: Vec<LiveId> = Vec::new();
        for (id, doc) in &mut self.files {
            doc.refresh_dirty(cx);
            if doc.dirty {
                dirty.push(*id);
            }
        }
        dirty.sort_by_key(|id| id.0);
        self.quit_after_saves = true;
        for id in dirty {
            self.save_file(id, false, window, cx);
        }
        self.continue_quit(cx);
    }

    /// After a save: quits when it was the last one asked for by
    /// [`Shell::save_all_and_quit`] and all went well.
    pub(in crate::ui) fn continue_quit(&mut self, cx: &mut Context<Self>) {
        if !self.quit_after_saves || self.files.values().any(|doc| doc.saving) {
            return;
        }
        self.quit_after_saves = false;
        if self.files.values().all(|doc| !doc.dirty && !doc.conflict) {
            self.quit_now(cx);
        } else {
            self.engine.report(
                StatusKind::Error,
                "Not quitting: some files could not be saved.",
            );
        }
    }

    /// "Quit without saving": the drafts go with the changes.
    pub(in crate::ui) fn discard_and_quit(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<LiveId> = self.files.keys().copied().collect();
        for id in ids {
            self.remove_draft(id, cx);
        }
        // The files stay as they are on screen, but nothing is flushed.
        for doc in self.files.values_mut() {
            doc.dirty = false;
            doc.saved = doc
                .text(cx)
                .map_or(doc.saved, |text| super::document::fingerprint(&text));
        }
        self.quit_now(cx);
    }
}

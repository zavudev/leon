//! The files that were open and how the tree was left, kept across restarts.
//!
//! `open_files.json`, beside the settings, holds a [`Saved`]: the open text
//! files in the order of the tabs (machine, path, cursor line, Markdown view,
//! and whether the file was the one on screen in its workspace) and, for each
//! folder the tree showed, the folders that were open and the selection.
//! Every field is optional with a default and the file carries a version, so
//! an older or newer build reads what it understands and the rest is
//! ignored. A missing or unreadable file means nothing is restored.
//!
//! The window writes it when the set of files, a view mode, the active tab or
//! the tree changes, and again as it ends (the cursor lines only then, so
//! typing a new line does not write a file). A file that no longer exists, or
//! is no longer text, is skipped without a word when they are opened again.

use super::super::live::LiveId;
use super::super::shell::Shell;
use super::document::{image_format, EditorDoc};
use super::preview::{has_page, ViewMode};
use crate::files::{FileContent, ImageContent};
use crate::settings;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The file's name, next to the settings file.
pub const FILE_NAME: &str = "open_files.json";

/// The version this build writes.
pub const VERSION: u32 = 1;

/// One open file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedFile {
    /// The machine it is on.
    pub machine: String,
    /// Its path there.
    pub path: String,
    /// The line the cursor was on, from 0.
    pub line: u32,
    /// The Markdown view: `edit`, `split` or `preview`.
    pub mode: String,
    /// Whether it was the one shown in its workspace.
    pub active: bool,
}

/// What the tree showed of one folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedTree {
    /// The open folders, relative to the root.
    pub open: Vec<String>,
    /// The selected path, relative to the root.
    pub selected: Option<String>,
}

/// Everything kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// The version that wrote it.
    pub version: u32,
    /// The open files, in the order of the tabs.
    pub files: Vec<SavedFile>,
    /// The trees, by the key of their folder.
    pub trees: BTreeMap<String, SavedTree>,
}

impl Saved {
    /// Reads the file; defaults when it is missing or unreadable.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "the open files could not be read; starting without them");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes the file whole, through a temporary one.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temp, path)
    }

    /// The same without the cursor lines: what decides whether to write
    /// while the application runs.
    pub fn shape(&self) -> Self {
        let mut shape = self.clone();
        shape.version = 0;
        for file in &mut shape.files {
            file.line = 0;
        }
        shape
    }
}

impl Shell {
    /// What is open now, as it would be written.
    fn open_files_now(&self, cx: &gpui_kit::App) -> Saved {
        let mut files = Vec::new();
        for workspace in self.workspaces.all() {
            let shown = workspace.tabs.get(workspace.active).map(|tab| tab.focus);
            for tab in &workspace.tabs {
                for id in tab.layout.leaves() {
                    let Some(doc) = self.files.get(&id) else {
                        continue;
                    };
                    files.push(SavedFile {
                        machine: doc.machine.as_str().to_owned(),
                        path: doc.path.clone(),
                        line: cursor_line(doc, cx),
                        mode: doc.mode.name().to_owned(),
                        active: shown == Some(id),
                    });
                }
            }
        }
        // Trees of folders not shown this run keep what they had.
        let mut trees = self.saved_open.trees.clone();
        for (key, tree) in self.file_tree.trees() {
            if tree.open.is_empty() && tree.selected.is_none() {
                trees.remove(key);
                continue;
            }
            trees.insert(
                key.clone(),
                SavedTree {
                    open: tree
                        .open
                        .iter()
                        .filter(|path| !path.is_empty())
                        .cloned()
                        .collect(),
                    selected: tree.selected.clone(),
                },
            );
        }
        Saved {
            version: VERSION,
            files,
            trees,
        }
    }

    /// Writes the open files when their shape changed. Runs when the window
    /// is drawn.
    pub(in crate::ui) fn watch_open_files(&mut self, cx: &mut gpui_kit::Context<Self>) {
        if self.restoring_files || self.closing.quitting {
            return;
        }
        let Some(path) = self.open_files_file.clone() else {
            return;
        };
        let now = self.open_files_now(cx);
        if now.shape() == self.saved_open.shape() {
            return;
        }
        if let Err(error) = now.save(&path) {
            tracing::warn!(%error, "the open files could not be saved");
        }
        self.saved_open = now;
    }

    /// Writes the open files as they are, cursor lines included: the
    /// application is ending.
    pub(in crate::ui) fn flush_open_files(&mut self, cx: &gpui_kit::App) {
        if self.restoring_files {
            return;
        }
        let Some(path) = self.open_files_file.clone() else {
            return;
        };
        let now = self.open_files_now(cx);
        if let Err(error) = now.save(&path) {
            tracing::warn!(%error, "the open files could not be saved");
        }
        self.saved_open = now;
    }

    /// Opens again the files of the last run, one after the other, without
    /// taking the keyboard from what is on screen.
    pub(in crate::ui) fn restore_open_files(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        let Some(path) = self.open_files_file.clone() else {
            return;
        };
        let saved = Saved::load(&path);
        self.saved_open = saved.clone();
        if saved.files.is_empty() || !settings::flag(cx, "editor_restore_files") {
            return;
        }
        self.restoring_files = true;
        cx.spawn_in(window, async move |this, cx| {
            let mut shown: Vec<LiveId> = Vec::new();
            for entry in saved.files {
                let machine = leon_core::MachineId::from_string(entry.machine.clone());
                // A picture of this computer is read as bytes, not as text.
                if machine.is_local() && image_format(&entry.path).is_some() {
                    let Ok(reading) = this.update(cx, |_, cx| {
                        super::actions::read_picture(entry.path.clone(), cx)
                    }) else {
                        return;
                    };
                    // Gone or unreadable: skipped quietly.
                    let Ok(content) = reading.await else {
                        continue;
                    };
                    let opened = this.update(cx, |this, cx| {
                        this.finish_restore_image(machine, entry, content, cx)
                    });
                    if let Ok(Some(id)) = opened {
                        shown.push(id);
                    }
                    continue;
                }
                let Ok(reading) = this.update(cx, |this, _| {
                    this.engine.check_file(machine.clone(), entry.path.clone())
                }) else {
                    return;
                };
                // Gone, unreachable or not text any more: skipped quietly.
                let Ok(Ok(content @ FileContent::Text { .. })) = reading.await else {
                    continue;
                };
                let opened = this.update_in(cx, |this, window, cx| {
                    this.finish_restore(machine, entry, content, window, cx)
                });
                if let Ok(Some(id)) = opened {
                    shown.push(id);
                }
            }
            this.update_in(cx, |this, window, cx| {
                // The tab each workspace had on screen is shown again.
                for id in shown {
                    this.workspaces.focus(id);
                }
                this.restoring_files = false;
                this.sync_focus(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// One file of the last run is read: its tab is made. Returns its leaf
    /// when it was the one on screen in its workspace.
    fn finish_restore(
        &mut self,
        machine: leon_core::MachineId,
        entry: SavedFile,
        content: FileContent,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Option<LiveId> {
        let folder = self.file_folder(&machine, &entry.path);
        if self.file_leaf(&machine, &entry.path, &folder).is_some() {
            return None;
        }
        let id = self.live.next_id();
        let mut doc = EditorDoc::open(machine, entry.path.clone(), folder, content, id, window, cx);
        self.restore_draft_of(&mut doc, window, cx);
        let (machine, folder) = (doc.machine.clone(), doc.folder.clone());
        self.files.insert(id, doc);
        self.workspaces
            .add_folder_tab(machine.as_str(), &folder, id);
        self.go_to_line(id, Some(entry.line + 1), window, cx);
        if has_page(&entry.path) {
            match ViewMode::parse(&entry.mode) {
                ViewMode::Edit => {}
                mode => self.set_view_mode(id, mode, window, cx),
            }
        }
        cx.notify();
        entry.active.then_some(id)
    }

    /// [`Self::finish_restore`] for a picture.
    fn finish_restore_image(
        &mut self,
        machine: leon_core::MachineId,
        entry: SavedFile,
        content: ImageContent,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Option<LiveId> {
        let folder = self.file_folder(&machine, &entry.path);
        if self.file_leaf(&machine, &entry.path, &folder).is_some() {
            return None;
        }
        let id = self.live.next_id();
        let doc = EditorDoc::open_image(machine, entry.path, folder, content);
        let (machine, folder) = (doc.machine.clone(), doc.folder.clone());
        self.files.insert(id, doc);
        self.workspaces
            .add_folder_tab(machine.as_str(), &folder, id);
        cx.notify();
        entry.active.then_some(id)
    }

    /// Seeds a tree that is made for the first time with what the last run
    /// left in it.
    pub(in crate::ui) fn saved_tree(&self, key: &str) -> Option<SavedTree> {
        self.saved_open.trees.get(key).cloned()
    }
}

fn cursor_line(doc: &EditorDoc, cx: &gpui_kit::App) -> u32 {
    doc.state()
        .map_or(0, |state| state.read(cx).cursor_position().line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Saved {
        Saved {
            version: VERSION,
            files: vec![SavedFile {
                machine: "local".into(),
                path: "/p/a.md".into(),
                line: 12,
                mode: "split".into(),
                active: true,
            }],
            trees: BTreeMap::from([(
                "local\u{1f}/p".to_owned(),
                SavedTree {
                    open: vec!["src".into(), "src/ui".into()],
                    selected: Some("src/ui/a.rs".into()),
                },
            )]),
        }
    }

    #[test]
    fn what_is_saved_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        sample().save(&file).unwrap();
        assert_eq!(Saved::load(&file), sample());
    }

    #[test]
    fn a_missing_broken_or_older_file_is_nothing_or_what_it_has() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        assert_eq!(Saved::load(&file), Saved::default());
        std::fs::write(&file, b"{ nope").unwrap();
        assert_eq!(Saved::load(&file), Saved::default());
        // A first build's file without trees, and a later one's with a field
        // this build does not know.
        std::fs::write(
            &file,
            br#"{"version":1,"files":[{"machine":"local","path":"/p/a.rs"}],"from_the_future":[1]}"#,
        )
        .unwrap();
        let loaded = Saved::load(&file);
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[0].mode, "");
        assert_eq!(ViewMode::parse(&loaded.files[0].mode), ViewMode::Edit);
        assert!(loaded.trees.is_empty());
    }

    #[test]
    fn the_cursor_line_alone_does_not_change_the_shape() {
        let mut moved = sample();
        moved.files[0].line = 99;
        assert_eq!(sample().shape(), moved.shape());
        moved.files[0].mode = "edit".into();
        assert_ne!(sample().shape(), moved.shape());
    }
}

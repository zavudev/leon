//! The files panel: the files of the project or worktree the keyboard is on,
//! on the right of the main pane, hidden until asked for.
//!
//! The listing is git-aware when the folder is a repository (tracked and
//! untracked-not-ignored files, so `target/` and `node_modules/` stay out) and
//! a bounded walk otherwise; both happen where the files are, so a project on
//! another machine lists just the same (see the engine's `list_files`). The
//! tree itself — folders, order, what an open folder reveals — is a pure
//! function ([`rows`]) with its own tests.
//!
//! Clicking a folder folds or unfolds it; clicking a Markdown file opens it in
//! the main pane. Other files are listed for context and are not opened:
//! Leon reads Markdown.

use super::shell::{Overlay, Shell};
use crate::engine::EngineError;
use crate::theme::{metrics, px, Palette};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    div, uniform_list, AnyElement, Context, Div, Stateful, Task, UniformListScrollHandle, Window,
};
use leon_core::MachineId;
use std::collections::{HashMap, HashSet};

/// One row of the panel: a folder or a file, with how deep it sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRow {
    /// The path relative to the panel's root.
    pub path: String,
    /// What the row shows: the last part of the path.
    pub name: String,
    /// How many folders deep it is.
    pub depth: usize,
    /// Whether it is a folder.
    pub folder: bool,
    /// Whether a folder is open.
    pub open: bool,
}

/// The rows of `paths` a panel with `expanded` open folders shows, folders
/// before files at every level, names case-insensitive.
pub fn rows(paths: &[String], expanded: &HashSet<String>) -> Vec<FileRow> {
    // Every folder that any path implies.
    let mut folders: HashSet<String> = HashSet::new();
    for path in paths {
        let mut parent = String::new();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            let full = if parent.is_empty() {
                part.to_owned()
            } else {
                format!("{parent}/{part}")
            };
            if full != *path {
                folders.insert(full.clone());
            }
            parent = full;
        }
    }
    // The children of every folder, the root under the empty key: every
    // ancestor and every file is a child of the level above it.
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for path in paths {
        let mut parent = String::new();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            let full = if parent.is_empty() {
                part.to_owned()
            } else {
                format!("{parent}/{part}")
            };
            children
                .entry(parent.clone())
                .or_default()
                .push(full.clone());
            parent = full;
        }
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| {
            let folder_a = folders.contains(a);
            let folder_b = folders.contains(b);
            folder_b
                .cmp(&folder_a)
                .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
                .then_with(|| a.cmp(b))
        });
        list.dedup();
    }
    fn visit(
        children: &HashMap<String, Vec<String>>,
        folders: &HashSet<String>,
        expanded: &HashSet<String>,
        parent: &str,
        depth: usize,
        out: &mut Vec<FileRow>,
    ) {
        for path in children.get(parent).into_iter().flatten() {
            let folder = folders.contains(path);
            let open = folder && expanded.contains(path);
            out.push(FileRow {
                name: path.rsplit('/').next().unwrap_or(path).to_owned(),
                path: path.clone(),
                depth,
                folder,
                open,
            });
            if open {
                visit(children, folders, expanded, path, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(&children, &folders, expanded, "", 0, &mut out);
    out
}

/// What the panel is doing with its listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FilesState {
    /// Nothing asked for yet.
    #[default]
    Idle,
    /// A listing is on its way.
    Loading,
    /// The listing is in.
    Ready,
    /// Why it could not be listed.
    Failed(String),
}

/// The folder the panel is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilesRoot {
    /// Where it lives.
    pub machine: MachineId,
    /// That machine's name.
    pub machine_name: String,
    /// The folder's path on that machine.
    pub path: String,
}

/// The panel's state.
#[derive(Default)]
pub struct FilesUi {
    /// Whether it is on screen; hidden until asked for.
    pub visible: bool,
    /// What it is showing.
    pub root: Option<FilesRoot>,
    /// The paths of the listing, relative to the root.
    pub entries: Vec<String>,
    /// The folders the person opened.
    pub expanded: HashSet<String>,
    /// The flattened tree, kept from the last time it changed.
    pub flattened: Vec<FileRow>,
    /// Whether the listing is on its way in, or why it is not.
    pub state: FilesState,
    /// Which listing was asked for last: one that lands late only fills its
    /// own.
    pub seq: u64,
    /// The listing in flight.
    pub task: Option<Task<()>>,
    /// The scroll of the row list.
    pub scroll: UniformListScrollHandle,
}

impl FilesUi {
    /// Rebuilds the flattened rows from the listing and the open folders.
    pub fn refresh_rows(&mut self) {
        self.flattened = rows(&self.entries, &self.expanded);
    }

    /// Folds or unfolds one folder.
    pub fn toggle_folder(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_owned());
        }
        self.refresh_rows();
    }
}

impl Shell {
    /// "Show or hide the files": the panel comes and goes; the first time it
    /// comes it lists the folder the keyboard is on.
    pub(super) fn toggle_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay != Overlay::None {
            self.close_overlay(window, cx);
        }
        self.files.visible = !self.files.visible;
        if self.files.visible {
            self.files_reload(cx);
        }
        cx.notify();
    }

    /// Lists the files of the folder the keyboard is on: the worktree or
    /// project open, the cursor's folder, or, with nothing selected, the
    /// first project of the machine in view.
    pub(super) fn files_reload(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.files_root() else {
            self.files.state =
                FilesState::Failed("Select a project, worktree or folder first.".to_owned());
            cx.notify();
            return;
        };
        self.files.root = Some(root.clone());
        self.files.expanded.clear();
        self.files.flattened.clear();
        self.files.entries.clear();
        self.files.state = FilesState::Loading;
        self.files.seq += 1;
        let seq = self.files.seq;
        let listing = self
            .engine
            .list_files(root.machine.clone(), root.path.clone());
        self.files.task = Some(cx.spawn(async move |this, cx| {
            let outcome = match listing.await {
                Ok(listed) => listed,
                Err(error) => Err(EngineError::File(error.to_string())),
            };
            this.update(cx, |shell, cx| shell.files_listed(seq, outcome, cx))
                .ok();
        }));
        cx.notify();
    }

    /// What the panel should be showing: where the keyboard is, or the first
    /// project of the machine in view.
    fn files_root(&self) -> Option<FilesRoot> {
        if let Some(here) = self.here() {
            return Some(FilesRoot {
                machine: here.machine,
                machine_name: here.machine_name,
                path: here.cwd,
            });
        }
        let machine = self.current_machine();
        let entry = self
            .snapshot
            .projects
            .iter()
            .find(|entry| entry.project.machine_id == machine)
            .or_else(|| self.snapshot.projects.first())?;
        Some(FilesRoot {
            machine: entry.project.machine_id.clone(),
            machine_name: self
                .snapshot
                .machine(&entry.project.machine_id)
                .map_or_else(String::new, |machine| machine.name.clone()),
            path: entry.project.root.clone(),
        })
    }

    fn files_listed(
        &mut self,
        seq: u64,
        outcome: Result<Vec<String>, EngineError>,
        cx: &mut Context<Self>,
    ) {
        if seq != self.files.seq {
            return;
        }
        match outcome {
            Ok(entries) => {
                self.files.entries = entries;
                self.files.state = FilesState::Ready;
                self.files.refresh_rows();
            }
            Err(error) => self.files.state = FilesState::Failed(error.to_string()),
        }
        cx.notify();
    }

    /// A row of the panel was clicked: a folder folds or unfolds, a Markdown
    /// file opens in the main pane, anything else says why it does not.
    pub(super) fn files_activate(&mut self, row: &FileRow, cx: &mut Context<Self>) {
        if row.folder {
            self.files.toggle_folder(&row.path);
            cx.notify();
            return;
        }
        let Some(root) = self.files.root.clone() else {
            return;
        };
        if !super::document::is_markdown(&row.path) {
            self.engine.report(
                crate::engine::StatusKind::Info,
                format!("{} is not a Markdown file; Leon opens .md files.", row.name),
            );
            return;
        }
        let path = format!("{}/{}", root.path.trim_end_matches('/'), row.path);
        self.open_document(root.machine, path, cx);
    }

    /// The panel itself: its header, the tree, and where it stands.
    pub(super) fn render_files(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let header = div()
            .debug_selector(|| "files-header".into())
            .relative()
            .flex_none()
            .h(metrics::HEADER_HEIGHT())
            .px_4()
            .border_b_1()
            .border_color(colours.border)
            .flex()
            .items_center()
            .gap_3()
            .child(super::widgets::section_label("Files", colours))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(colours.text_muted)
                    .children(
                        self.files
                            .root
                            .as_ref()
                            .map(|root| root.machine_name.clone()),
                    ),
            )
            .child(self.files_button(
                "files-refresh",
                "REFRESH",
                colours,
                cx,
                |shell, _window, cx| shell.files_reload(cx),
            ))
            .child(
                self.files_button("files-close", "CLOSE", colours, cx, |shell, window, cx| {
                    shell.toggle_files(window, cx)
                }),
            );
        let body: AnyElement = match &self.files.state {
            FilesState::Idle => div().into_any_element(),
            FilesState::Loading => div()
                .debug_selector(|| "files-loading".into())
                .p_4()
                .child(super::widgets::mono("LOADING\u{2026}").text_color(colours.text_muted))
                .into_any_element(),
            FilesState::Failed(why) => div()
                .debug_selector(|| "files-failed".into())
                .p_4()
                .text_color(colours.error)
                .child(why.clone())
                .into_any_element(),
            FilesState::Ready if self.files.flattened.is_empty() => div()
                .debug_selector(|| "files-empty".into())
                .p_4()
                .text_color(colours.text_faint)
                .child("No files here.")
                .into_any_element(),
            FilesState::Ready => {
                let colours = *colours;
                div()
                    .debug_selector(|| "files".into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "files-list",
                            self.files.flattened.len(),
                            cx.processor(
                                move |this: &mut Shell,
                                      range: std::ops::Range<usize>,
                                      _window,
                                      cx| {
                                    range
                                        .filter_map(|index| {
                                            this.files.flattened.get(index).map(|row| {
                                                render_file_row(index, row, &colours, cx)
                                            })
                                        })
                                        .collect::<Vec<_>>()
                                },
                            ),
                        )
                        .track_scroll(&self.files.scroll)
                        .size_full(),
                    )
                    .child(gpui_kit::component::scroll::Scrollbar::vertical(
                        &self.files.scroll,
                    ))
                    .into_any_element()
            }
        };
        div()
            .debug_selector(|| "files-panel".into())
            .flex_none()
            .w(metrics::FILES_WIDTH())
            .h_full()
            .border_l_1()
            .border_color(colours.border)
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
    }

    /// A small text button of the panel's header.
    fn files_button(
        &self,
        id: &'static str,
        label: &'static str,
        colours: &Palette,
        cx: &mut Context<Self>,
        action: fn(&mut Shell, &mut Window, &mut Context<Shell>),
    ) -> Stateful<Div> {
        let colour = colours.text_muted;
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .flex_none()
            .cursor_pointer()
            .child(super::widgets::mono(label).text_color(colour))
            .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
    }

    /// The main header's button that shows or hides the panel, beside the
    /// sidebar's own toggle.
    pub(super) fn files_toggle_button(
        &self,
        id: &'static str,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let hover = colours.surface;
        let tip: gpui_kit::SharedString =
            super::sidebar::tooltip_text(crate::keys::Command::ToggleFiles).into();
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex_none()
            .size(metrics::HEADER_TITLE_LINE())
            .flex()
            .items_center()
            .justify_center()
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.run_command(crate::keys::Command::ToggleFiles, window, cx);
            }))
            .child(crate::icons::icon(
                if self.files.visible {
                    crate::icons::IconName::FolderOpen
                } else {
                    crate::icons::IconName::Folder
                },
                px(14.),
                colours.text_muted,
            ))
    }
}

/// One row: the indent, the folder arrow, the name. A Markdown file wears the
/// accent so the files Leon opens stand out.
fn render_file_row(
    index: usize,
    row: &FileRow,
    colours: &Palette,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let folder = row.folder;
    let open = row.open;
    let markdown = !folder && super::document::is_markdown(&row.path);
    let colour = if folder {
        colours.text
    } else if markdown {
        colours.signal
    } else {
        colours.text_muted
    };
    let name = row.name.clone();
    let row = row.clone();
    div()
        .id(("file-row", index))
        .debug_selector(move || format!("file-row-{index}"))
        .w_full()
        .h(metrics::ROW_HEIGHT())
        .px_4()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_none()
                .w(px(12.))
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text_faint)
                .children(folder.then_some(if open { "\u{25be}" } else { "\u{25b8}" })),
        )
        .child(div().flex_none().w(px(row.depth as f32 * 12.)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(colour)
                .child(name),
        )
        .on_click(cx.listener(move |shell, _, _window, cx| {
            shell.files_activate(&row, cx);
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|path| (*path).to_owned()).collect()
    }

    fn names(rows: &[FileRow]) -> Vec<String> {
        rows.iter().map(|row| row.path.clone()).collect()
    }

    #[test]
    fn folders_come_first_at_every_level_and_only_open_ones_reveal_children() {
        let entries = paths(&["README.md", "src/main.rs", "src/ui/sidebar.rs", "docs/a.md"]);
        let closed = rows(&entries, &HashSet::new());
        assert_eq!(
            names(&closed),
            ["docs", "src", "README.md"],
            "folders first, names sorted, children hidden"
        );
        assert!(closed[0].folder && !closed[0].open);
        assert_eq!(closed[0].depth, 0);

        let expanded: HashSet<String> = ["src".to_owned()].into_iter().collect();
        let open = rows(&entries, &expanded);
        assert_eq!(
            names(&open),
            ["docs", "src", "src/ui", "src/main.rs", "README.md"]
        );
        assert_eq!(open[2].depth, 1, "the folder under src is one deep");
        assert_eq!(open[3].depth, 1);
        assert!(open[2].folder && !open[2].open);
    }

    #[test]
    fn a_folder_implied_by_a_deep_path_shows_every_level() {
        let entries = paths(&["a/b/c/d.md"]);
        let all: HashSet<String> = ["a", "a/b", "a/b/c"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            names(&rows(&entries, &all)),
            ["a", "a/b", "a/b/c", "a/b/c/d.md"]
        );
    }

    #[test]
    fn names_sort_case_insensitively_and_duplicates_do_not_repeat() {
        let entries = paths(&["Zeta.md", "alpha.md", "Alpha.md"]);
        assert_eq!(
            names(&rows(&entries, &HashSet::new())),
            ["Alpha.md", "alpha.md", "Zeta.md"]
        );
    }

    #[test]
    fn toggling_a_folder_opens_and_closes_it() {
        let mut ui = FilesUi {
            entries: paths(&["src/a.rs"]),
            ..FilesUi::default()
        };
        ui.refresh_rows();
        assert_eq!(names(&ui.flattened), ["src"]);
        ui.toggle_folder("src");
        assert_eq!(names(&ui.flattened), ["src", "src/a.rs"]);
        ui.toggle_folder("src");
        assert_eq!(names(&ui.flattened), ["src"]);
    }

    #[test]
    fn an_empty_listing_has_no_rows() {
        assert!(rows(&[], &HashSet::new()).is_empty());
        assert!(rows(&paths(&[""]), &HashSet::new()).is_empty());
    }
}

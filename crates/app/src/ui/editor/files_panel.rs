//! The file tree panel: a column between the sidebar and the main pane that
//! lists the files of the project or worktree in view.
//!
//! * **Which folder.** The folder of the workspace in view: the worktree or
//!   project the sidebar's cursor or the main pane is in (`Shell::here`, the
//!   same answer a new session gets), else the first project of the machine
//!   in view. A [`Tree`] is kept for each folder shown, under
//!   `workspace::key_of(machine, root)`, so the open folders and the
//!   selection are as they were left when the folder comes back.
//! * **Lazy.** A folder is listed (through the engine, where the folder is)
//!   the first time it is opened. Every ~5 seconds while the panel is
//!   visible, on the window's focus and after a save, the root and the open
//!   folders are listed again and git is asked for its marks; an answer that
//!   fails keeps what was shown.
//! * **Keyboard.** The panel is a pane of the keyboard like the sidebar: the
//!   movement and open commands act on it while it has the keyboard, `Esc`
//!   gives the keyboard to the main pane.
//! * **Icons.** Glyphs of the bundled symbols font ([`super::icons_map`]); with
//!   no such font the Lucide file and folder icons are drawn.

use std::collections::HashMap;

use super::super::shell::{Overlay, Pane, Shell};
use super::super::widgets::{focus_rule, mono, section_label};
use super::super::workspace;
use super::filetree::{self, Left, Right, Row, RowKind, Tree};
use super::icons_map::{self, glyph};
use crate::files::GitMark;
use crate::icons::{icon, IconName};
use crate::keys::Command;
use crate::settings;
use crate::theme::{metrics, px, Palette};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    div, uniform_list, AnyElement, App, Context, Div, Hsla, ScrollStrategy, SharedString, Stateful,
    Task, UniformListScrollHandle, Window,
};
use leon_core::MachineId;

/// The panel's state in the shell.
pub struct FileTreeUi {
    /// The tree of each folder shown, by `workspace::key_of`.
    trees: HashMap<String, Tree>,
    /// The key of the tree on screen.
    current: Option<String>,
    /// The list's scroll.
    scroll: UniformListScrollHandle,
    /// The timer of the refresh, while the panel is visible.
    ticker: Option<Task<()>>,
    /// Whether the symbols font can be drawn (looked at once).
    font: std::cell::Cell<Option<bool>>,
    /// The selection is to be scrolled into view once its row exists.
    reveal: bool,
}

impl Default for FileTreeUi {
    fn default() -> Self {
        Self {
            trees: HashMap::new(),
            current: None,
            scroll: UniformListScrollHandle::new(),
            ticker: None,
            font: std::cell::Cell::new(None),
            reveal: false,
        }
    }
}

impl FileTreeUi {
    /// The tree on screen.
    pub(in crate::ui) fn current(&self) -> Option<&Tree> {
        self.trees.get(self.current.as_ref()?)
    }

    /// The trees, by the key of their folder.
    pub(in crate::ui) fn trees(&self) -> impl Iterator<Item = (&String, &Tree)> {
        self.trees.iter()
    }

    fn current_mut(&mut self) -> Option<&mut Tree> {
        let key = self.current.as_ref()?;
        self.trees.get_mut(key)
    }
}

/// The colour a git mark is drawn in: tokens of the palette.
pub fn mark_colour(mark: GitMark, palette: &Palette) -> Hsla {
    match mark {
        GitMark::Modified => palette.warning,
        GitMark::Added => palette.success,
        GitMark::Deleted | GitMark::Conflicted => palette.error,
        GitMark::Renamed => palette.info,
        GitMark::Untracked => palette.signal,
        GitMark::Ignored => palette.text_faint,
    }
}

/// The panes the keyboard visits with Tab, in order.
pub fn pane_order(files: bool) -> &'static [Pane] {
    if files {
        &[Pane::Sidebar, Pane::Files, Pane::Main]
    } else {
        &[Pane::Sidebar, Pane::Main]
    }
}

/// The pane after (or before) `from` in [`pane_order`].
pub fn next_pane(from: Pane, files: bool, forward: bool) -> Pane {
    let order = pane_order(files);
    let at = order.iter().position(|pane| *pane == from).unwrap_or(0);
    let to = if forward {
        (at + 1) % order.len()
    } else {
        (at + order.len() - 1) % order.len()
    };
    order[to]
}

impl Shell {
    /// Whether the panel is showing.
    pub(in crate::ui) fn files_shown(&self, cx: &App) -> bool {
        settings::get(cx).files_visible
    }

    /// The tree on screen.
    pub(in crate::ui) fn file_tree_now(&self) -> Option<&Tree> {
        self.file_tree.current()
    }

    // ----- showing and hiding ----------------------------------------------------------------

    /// Shows the panel and gives it the keyboard, or hides it (the keyboard
    /// goes to the main pane if it was in the panel).
    pub(in crate::ui) fn toggle_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let show = !self.files_shown(cx);
        settings::update(cx, |settings| settings.files_visible = show);
        self.close_overlay(window, cx);
        if show {
            self.pane = Pane::Files;
            self.files_sync(cx);
        } else if self.pane == Pane::Files {
            self.pane = Pane::Main;
        }
        cx.notify();
    }

    /// Keeps the panel on the folder in view and its timer going while it
    /// shows. Called when the window is drawn.
    pub(in crate::ui) fn files_sync(&mut self, cx: &mut Context<Self>) {
        if !self.files_shown(cx) {
            self.file_tree.ticker = None;
            if self.pane == Pane::Files {
                self.pane = Pane::Main;
            }
            return;
        }
        self.watch_files(cx);
        let Some((machine, root)) = self.files_target() else {
            self.file_tree.current = None;
            return;
        };
        let key = workspace::key_of(machine.as_str(), &root);
        if self.file_tree.current.as_ref() == Some(&key) {
            // A revealed file is scrolled to when its folders have been listed.
            if self.file_tree.reveal {
                if let Some(index) = self.file_tree.current().and_then(Tree::selected_index) {
                    self.file_tree.reveal = false;
                    self.file_tree
                        .scroll
                        .scroll_to_item(index, ScrollStrategy::Center);
                }
            }
            return;
        }
        self.file_tree.current = Some(key.clone());
        let saved = self.saved_tree(&key);
        self.file_tree.trees.entry(key).or_insert_with(|| {
            let mut tree = Tree::new(machine, root);
            // As the last run left it: the open folders are listed by the
            // refresh that follows.
            if let Some(saved) = saved {
                tree.open = saved
                    .open
                    .into_iter()
                    .filter(|path| !path.is_empty())
                    .collect();
                tree.selected = saved.selected;
                tree.rebuild();
            }
            tree
        });
        self.files_refresh(cx);
    }

    /// `RevealInTree`: shows the panel, opens the folders above the file on
    /// screen and selects it.
    pub(in crate::ui) fn reveal_in_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((machine, path)) = self
            .focused_file()
            .and_then(|id| self.files.get(&id))
            .map(|doc| (doc.machine.clone(), doc.path.clone()))
        else {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "There is no file to reveal.",
            );
            return;
        };
        if !self.files_shown(cx) {
            settings::update(cx, |settings| settings.files_visible = true);
        }
        self.files_sync(cx);
        let Some(key) = self.file_tree.current.clone() else {
            return;
        };
        let Some(tree) = self.file_tree.trees.get_mut(&key) else {
            return;
        };
        let Some(relative) = (tree.machine == machine)
            .then(|| filetree::relative(&tree.root, &path))
            .flatten()
        else {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "The file is outside the folder the tree shows.",
            );
            return;
        };
        let (root, machine) = (tree.root.clone(), tree.machine.clone());
        let mut folders = Vec::new();
        let mut folder = String::new();
        let names: Vec<&str> = relative.split('/').collect();
        for name in &names[..names.len() - 1] {
            folder = filetree::join(&folder, name);
            folders.push(folder.clone());
        }
        let mut to_list = Vec::new();
        for folder in folders {
            if tree.set_open(&folder, true) && tree.listing.insert(folder.clone()) {
                to_list.push(folder);
            }
        }
        tree.selected = Some(relative);
        for folder in to_list {
            self.list_folder(&key, machine.clone(), &root, folder, cx);
        }
        self.file_tree.reveal = true;
        let _ = window;
        cx.notify();
    }

    /// The folder the panel shows: the workspace folder of what the keyboard
    /// is on, else the first project of the machine in view.
    pub(in crate::ui) fn files_target(&self) -> Option<(MachineId, String)> {
        if let Some(place) = self.here() {
            let root =
                super::super::tree::workspace_root(&self.snapshot, &place.machine, &place.cwd);
            return Some((place.machine, root));
        }
        let machine = self.current_machine();
        let entry = self
            .snapshot
            .projects
            .iter()
            .find(|entry| entry.project.machine_id == machine)?;
        Some((machine, entry.project.root.clone()))
    }

    /// The timer that lists the folders again while the panel shows. Never
    /// when `Options::files_interval` is zero.
    fn watch_files(&mut self, cx: &mut Context<Self>) {
        let every = self.options.files_interval;
        if self.file_tree.ticker.is_some() || every.is_zero() {
            return;
        }
        self.file_tree.ticker = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(every).await;
            if this.update(cx, |this, cx| this.files_refresh(cx)).is_err() {
                return;
            }
        }));
    }

    // ----- listing ---------------------------------------------------------------------------

    /// Lists the root and the open folders of the tree on screen again, and
    /// asks git for its marks. Does nothing while the panel is hidden, and
    /// asks for nothing that is already being asked.
    pub(in crate::ui) fn files_refresh(&mut self, cx: &mut Context<Self>) {
        if !self.files_shown(cx) {
            return;
        }
        let Some(key) = self.file_tree.current.clone() else {
            return;
        };
        let Some(tree) = self.file_tree.trees.get_mut(&key) else {
            return;
        };
        let machine = tree.machine.clone();
        let root = tree.root.clone();
        let folders: Vec<String> = filetree::to_list(&tree.open)
            .into_iter()
            .filter(|folder| tree.listing.insert(folder.clone()))
            .collect();
        let marks = (!tree.marking).then(|| {
            tree.marking = true;
        });
        for folder in folders {
            self.list_folder(&key, machine.clone(), &root, folder, cx);
        }
        if marks.is_some() {
            let reading = self.engine.git_marks(machine, root);
            cx.spawn(async move |this, cx| {
                let marks = reading.await.ok().and_then(Result::ok);
                this.update(cx, |this, cx| {
                    if let Some(tree) = this.file_tree.trees.get_mut(&key) {
                        tree.marking = false;
                        if let Some(marks) = marks {
                            tree.marks = marks;
                            tree.rebuild();
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// Asks for the entries of one folder of the tree `key`.
    fn list_folder(
        &mut self,
        key: &str,
        machine: MachineId,
        root: &str,
        folder: String,
        cx: &mut Context<Self>,
    ) {
        let key = key.to_owned();
        let listing = self
            .engine
            .list_dir(machine, filetree::absolute(root, &folder));
        cx.spawn(async move |this, cx| {
            let answer = match listing.await {
                Ok(Ok(entries)) => Ok(entries),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            this.update(cx, |this, cx| {
                if let Some(tree) = this.file_tree.trees.get_mut(&key) {
                    tree.listed(&folder, answer);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ----- the keyboard ----------------------------------------------------------------------

    /// Moves the selection (the movement commands, while the panel has the
    /// keyboard).
    pub(in crate::ui) fn files_move(&mut self, command: Command) {
        let Some(tree) = self.file_tree.current_mut() else {
            return;
        };
        let from = tree.selected_index();
        let (from, step) = match command {
            Command::Down => (from, 1),
            Command::Up => (from, -1),
            Command::PageDown => (from, 8),
            Command::PageUp => (from, -8),
            // From nowhere, forwards is the first row and backwards the last.
            Command::Top => (None, 1),
            Command::Bottom => (None, -1),
            _ => return,
        };
        if let Some(to) = filetree::step(&tree.rows, from, step) {
            tree.select(to);
            self.file_tree
                .scroll
                .scroll_to_item(to, ScrollStrategy::Nearest);
        }
    }

    /// Right: opens the selected folder, or goes into it.
    pub(in crate::ui) fn files_expand(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = self.file_tree.current() else {
            return;
        };
        let Some(index) = tree.selected_index() else {
            return;
        };
        match filetree::right(&tree.rows, index) {
            Right::Open => self.files_set_open(index, true, cx),
            Right::Child(child) => {
                if let Some(tree) = self.file_tree.current_mut() {
                    tree.select(child);
                }
                self.file_tree
                    .scroll
                    .scroll_to_item(child, ScrollStrategy::Nearest);
            }
            Right::Nothing => {}
        }
        cx.notify();
    }

    /// Left: closes the selected folder, or goes to the folder that holds it.
    pub(in crate::ui) fn files_collapse(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = self.file_tree.current() else {
            return;
        };
        let Some(index) = tree.selected_index() else {
            return;
        };
        match filetree::left(&tree.rows, index) {
            Left::Close => self.files_set_open(index, false, cx),
            Left::Parent(parent) => {
                if let Some(tree) = self.file_tree.current_mut() {
                    tree.select(parent);
                }
                self.file_tree
                    .scroll
                    .scroll_to_item(parent, ScrollStrategy::Nearest);
            }
            Left::Nothing => {}
        }
        cx.notify();
    }

    /// Opens or closes the folder at `index`, listing it when it is new.
    fn files_set_open(&mut self, index: usize, open: bool, cx: &mut Context<Self>) {
        let Some(key) = self.file_tree.current.clone() else {
            return;
        };
        let Some(tree) = self.file_tree.trees.get_mut(&key) else {
            return;
        };
        let Some(row) = tree.rows.get(index).filter(|row| row.kind == RowKind::Dir) else {
            return;
        };
        let path = row.path.clone();
        let (machine, root) = (tree.machine.clone(), tree.root.clone());
        if tree.set_open(&path, open) && tree.listing.insert(path.clone()) {
            self.list_folder(&key, machine, &root, path, cx);
        }
    }

    /// Enter: a file opens in a tab, a folder opens or closes.
    pub(in crate::ui) fn files_open_selected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tree) = self.file_tree.current() else {
            return;
        };
        let Some(index) = tree.selected_index() else {
            return;
        };
        self.files_activate(index, window, cx);
    }

    /// Does to the row at `index` what Enter does: a folder opens or closes
    /// and a file opens in a tab.
    fn files_activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tree) = self.file_tree.current() else {
            return;
        };
        let Some(row) = tree.rows.get(index) else {
            return;
        };
        match row.kind {
            RowKind::Dir => {
                let open = !row.open;
                self.files_set_open(index, open, cx);
            }
            RowKind::File => {
                let machine = tree.machine.clone();
                let path = filetree::absolute(&tree.root, &row.path);
                self.open_file(machine, path, None, window, cx);
            }
            RowKind::Note => {}
        }
        cx.notify();
    }

    /// A click on a row: it is selected and has the keyboard; a folder
    /// toggles, a file opens on a double click.
    fn files_click(
        &mut self,
        index: usize,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tree) = self.file_tree.current_mut() {
            tree.select(index);
        }
        self.pane = Pane::Files;
        self.focus.focus(window, cx);
        let is_dir = self
            .file_tree
            .current()
            .and_then(|tree| tree.rows.get(index))
            .is_some_and(|row| row.kind == RowKind::Dir);
        if is_dir || count >= 2 {
            self.files_activate(index, window, cx);
        }
        self.sync_focus(window, cx);
        cx.notify();
    }

    // ----- drawing ---------------------------------------------------------------------------

    /// Whether the symbols font is available; if not, Lucide icons are drawn.
    fn symbols_font(&self, cx: &App) -> bool {
        if let Some(known) = self.file_tree.font.get() {
            return known;
        }
        let found = cx
            .text_system()
            .all_font_names()
            .iter()
            .any(|name| name == icons_map::FAMILY);
        self.file_tree.font.set(Some(found));
        found
    }

    /// The column.
    pub(in crate::ui) fn render_files_panel(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let focused = self.pane == Pane::Files && self.overlay == Overlay::None;
        let palette = *colours;
        let tree = self.file_tree_now();
        let name = tree.map(|tree| super::super::tree::folder_name(&tree.root).to_owned());
        div()
            .debug_selector(|| "files-panel".into())
            .relative()
            .flex_none()
            .w(metrics::FILES_WIDTH())
            .h_full()
            .border_r_1()
            .border_color(colours.border)
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| "files-header".into())
                    .relative()
                    .flex_none()
                    .h(metrics::HEADER_HEIGHT())
                    .px_4()
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(metrics::HEADER_GAP())
                    .child(section_label("Files", colours))
                    .child(
                        div()
                            .h(metrics::HEADER_TITLE_LINE())
                            .flex()
                            .items_center()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(name.unwrap_or_else(|| "No folder".to_owned())),
                    )
                    .when(focused, |this| this.child(focus_rule(colours))),
            )
            .child(match tree {
                Some(tree) => {
                    let count = tree.rows.len();
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(
                            uniform_list(
                                "file-tree",
                                count,
                                cx.processor(
                                    move |this, range: std::ops::Range<usize>, _window, cx| {
                                        range
                                            .filter_map(|index| {
                                                this.render_file_row(index, &palette, cx)
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                ),
                            )
                            .track_scroll(&self.file_tree.scroll)
                            .size_full(),
                        )
                        .child(Scrollbar::vertical(&self.file_tree.scroll))
                }
                None => div()
                    .flex_1()
                    .p_4()
                    .text_color(colours.text_muted)
                    .child("Select a project or a worktree to see its files."),
            })
    }

    /// One row of the tree.
    fn render_file_row(
        &self,
        index: usize,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tree = self.file_tree_now()?;
        let row = tree.rows.get(index)?;
        let on = tree.selected_index() == Some(index);
        let focused = self.pane == Pane::Files;
        let hover = colours.surface;
        let ignored = row.mark == Some(GitMark::Ignored);
        let text = if ignored {
            colours.text_faint
        } else {
            colours.text
        };
        let base = div()
            .id(("file-row", index))
            .debug_selector(move || format!("file-row-{index}"))
            .relative()
            .flex_none()
            .h(px(26.))
            .w_full()
            .pl(px(8.) + metrics::INDENT() * f32::from(row.depth))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .overflow_hidden()
            .when(row.kind != RowKind::Note, |this| this.cursor_pointer())
            .when(on, |this| this.bg(colours.surface_2))
            .when(!on, |this| this.hover(move |style| style.bg(hover)))
            .when(on && focused, |this| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(colours.signal),
                )
            });
        if row.kind == RowKind::Note {
            return Some(
                base.text_color(colours.text_faint)
                    .child(div().min_w_0().truncate().child(row.name.clone()))
                    .into_any_element(),
            );
        }
        let is_dir = row.kind == RowKind::Dir;
        let chevron = if is_dir {
            icon(
                if row.open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                px(12.),
                colours.text_faint,
            )
            .into_any_element()
        } else {
            div().flex_none().size(px(12.)).into_any_element()
        };
        let tip: SharedString = row.name.clone().into();
        Some(
            base.on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                    this.files_click(index, event.click_count(), window, cx);
                }),
            )
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(chevron)
            .child(self.render_file_icon(row, colours, ignored, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(text)
                    .child(row.name.clone()),
            )
            .children(
                row.mark
                    .filter(|mark| *mark != GitMark::Ignored)
                    .map(|mark| {
                        // A file shows its letter; a folder only that it holds changes.
                        mono(if is_dir {
                            "\u{2022}".to_owned()
                        } else {
                            mark.letter().to_string()
                        })
                        .debug_selector(move || format!("file-mark-{index}"))
                        .text_color(mark_colour(mark, colours))
                    }),
            )
            .into_any_element(),
        )
    }

    /// The icon of a row: a glyph of the symbols font in its colour token, or
    /// Lucide's file or folder when there is no such font.
    fn render_file_icon(
        &self,
        row: &Row,
        colours: &Palette,
        ignored: bool,
        cx: &App,
    ) -> AnyElement {
        let is_dir = row.kind == RowKind::Dir;
        self.file_icon(&row.name, is_dir, row.open, ignored, colours, cx)
    }

    /// The icon of a file or folder called `name`, for the panel and the
    /// palette's file rows alike.
    pub(in crate::ui) fn file_icon(
        &self,
        name: &str,
        is_dir: bool,
        open: bool,
        ignored: bool,
        colours: &Palette,
        cx: &App,
    ) -> AnyElement {
        let (glyph, token) = glyph(name, is_dir, open);
        let mut colour = token.resolve(colours);
        if ignored {
            colour = colours.text_faint;
        }
        if self.symbols_font(cx) {
            div()
                .flex_none()
                .w(px(16.))
                .flex()
                .justify_center()
                .font_family(icons_map::FAMILY)
                .text_size(px(14.))
                .text_color(colour)
                .child(glyph.to_string())
                .into_any_element()
        } else {
            let name = match (is_dir, open) {
                (true, true) => IconName::FolderOpen,
                (true, false) => IconName::Folder,
                (false, _) => IconName::File,
            };
            div()
                .flex_none()
                .w(px(16.))
                .flex()
                .justify_center()
                .child(icon(name, px(14.), colour))
                .into_any_element()
        }
    }

    /// The button of the main header that shows or hides the panel.
    pub(in crate::ui) fn files_toggle_button(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let hover = colours.surface;
        let tip: SharedString = crate::ui::sidebar::tooltip_text(Command::ToggleFiles).into();
        let shown = self.files_shown(cx);
        div()
            .id("files-toggle")
            .debug_selector(|| "files-toggle".into())
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
                this.run_command(Command::ToggleFiles, window, cx);
            }))
            .child(icon(
                if shown {
                    IconName::FolderOpen
                } else {
                    IconName::Folder
                },
                px(14.),
                if shown {
                    colours.signal
                } else {
                    colours.text_muted
                },
            ))
    }
}

//! What a conversation's turns changed: the card at the end of a turn and
//! the panel beside the conversation that shows the diffs.
//!
//! The card says "3 files changed (+12 -4)" and lists the files. Its numbers
//! are git's own between the snapshots of the worktree that surround the turn
//! (`crate::chat::checkpoint`) when there are such, and else what the turn's
//! tools said they changed (`crate::chat::changed`).
//!
//! A click on the card, or on one of its files, opens the panel on the right
//! of the conversation. It shows the diffs for one of three scopes: the
//! files of one turn, a single file, or every uncommitted change of the
//! worktree. The scopes of a turn are read from the snapshots (or, without
//! them, from what the tools said); the whole worktree is read by the
//! engine, the way the changes tab reads it, and the diff itself is drawn by
//! the same code as that tab's (`changes_view::diff_body`).
//!
//! Escape closes the panel, and so does the command that opens it.

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, Context, SharedString, UniformListScrollHandle};
use leon_remote::{parse_diff, ChangedFile, DiffLine, FileDiff, FileStatus};

use super::changes::DiffState;
use super::changes_view::{split_name, DiffNames};
use super::chat::Chat;
use super::live::LiveId;
use super::shell::Shell;
use super::widgets::{led, mono};
use crate::chat::changed::{self, Stat};
use crate::chat::checkpoint;
use crate::chat::model::Item;
use crate::icons::{icon, IconName};
use crate::theme::{fonts, metrics, px, Palette};

/// How many files a card lists before it counts the rest.
const CARD_FILES: usize = 8;

/// Where the panel's files come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// What one turn changed, by the number of its checkpoint.
    Turn(u32),
    /// Every uncommitted change of the worktree.
    All,
}

/// How the panel is scoped, as its three chips say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The files of one turn.
    Turn,
    /// The one file that is selected.
    File,
    /// Every uncommitted change of the worktree.
    All,
}

/// The panel beside a conversation.
pub struct DiffPanel {
    /// Where its files come from.
    pub source: Source,
    /// The turn the person looked at last, which the TURN chip goes back to.
    pub turn: Option<u32>,
    /// Only the selected file is shown, without the list of the others.
    pub single: bool,
    /// The path of the selected file, from the root of the worktree.
    pub selected: Option<String>,
    /// The files of the whole worktree, once read.
    pub all: Option<Vec<ChangedFile>>,
    /// The diff of the selected file.
    pub diff: DiffState,
    /// The scroll of the diff.
    pub scroll: UniformListScrollHandle,
}

impl DiffPanel {
    /// The scope it shows.
    pub fn scope(&self) -> Scope {
        match (self.source, self.single) {
            (_, true) => Scope::File,
            (Source::Turn(_), false) => Scope::Turn,
            (Source::All, false) => Scope::All,
        }
    }
}

/// One row of the panel's list of files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The file, from the root of the worktree.
    pub path: String,
    /// What it added and removed, when that is known.
    pub counts: Option<(usize, usize)>,
    /// What happened to it, when git said.
    pub status: Option<FileStatus>,
}

/// The rows of a turn's files.
pub fn rows_of_stats(stats: &[Stat]) -> Vec<Row> {
    stats
        .iter()
        .map(|stat| Row {
            path: stat.path.clone(),
            counts: Some((stat.added, stat.removed)),
            status: None,
        })
        .collect()
}

/// The rows of the worktree's changed files.
pub fn rows_of_files(files: &[ChangedFile]) -> Vec<Row> {
    files
        .iter()
        .map(|file| Row {
            path: file.path.clone(),
            counts: None,
            status: Some(file.status),
        })
        .collect()
}

/// The path of the selection after the rows changed: the same file when it
/// is still there, else the first.
pub fn keep(selected: Option<&str>, rows: &[Row]) -> Option<String> {
    selected
        .filter(|path| rows.iter().any(|row| row.path == *path))
        .or_else(|| rows.first().map(|row| row.path.as_str()))
        .map(str::to_owned)
}

/// The selectors of the panel's diff body.
const NAMES: DiffNames = DiffNames {
    list: "chat-diff-lines",
    line: "chat-diff-line",
    none: "chat-diff-none",
    loading: "chat-diff-loading",
    failed: "chat-diff-failed",
    binary: "chat-diff-binary",
    too_large: "chat-diff-too-large",
    empty: "chat-diff-empty",
};

/// The lines of the diff the panel of conversation `id` holds.
fn panel_lines(shell: &Shell, id: LiveId) -> Option<&[DiffLine]> {
    match shell.chats.get(&id)?.diff.as_ref().map(|panel| &panel.diff) {
        Some(DiffState::Ready(FileDiff::Lines(lines))) => Some(lines.as_slice()),
        _ => None,
    }
}

impl Chat {
    /// The position among its marks of the turn numbered `turn`.
    fn turn_at(&self, turn: u32) -> Option<usize> {
        self.marks.iter().position(|mark| mark.turn == turn)
    }

    /// The items of the turn numbered `turn`: what was said and done in it.
    pub fn turn_items(&self, turn: u32) -> &[Item] {
        let items = &self.conversation.items;
        match self.turn_at(turn) {
            Some(at) => &items[changed::turn_range(&self.marks, at, items.len())],
            None => &[],
        }
    }

    /// What the turn numbered `turn` changed: git's count when it was made,
    /// else what its tools said, and only when a tool that can change files
    /// ran in it (see [`changed::of_turn`]).
    pub fn turn_stats(&self, turn: u32) -> Vec<Stat> {
        changed::of_turn(
            self.turn_items(turn),
            &self.cwd,
            self.measured.get(&turn).map(Vec::as_slice),
        )
    }

    /// The cards of the conversation: for each turn that is over and changed
    /// files, where its items end, its number and what it changed.
    pub fn cards(&self) -> Vec<(usize, u32, Vec<Stat>)> {
        let items = self.conversation.items.len();
        let going = self.conversation.running() || self.launching;
        self.marks
            .iter()
            .enumerate()
            .filter_map(|(at, mark)| {
                if going && at + 1 == self.marks.len() {
                    return None;
                }
                let stats = self.turn_stats(mark.turn);
                let end = changed::turn_range(&self.marks, at, items).end;
                (!stats.is_empty()).then_some((end, mark.turn, stats))
            })
            .collect()
    }

    /// The newest turn that changed files.
    pub fn latest_changed_turn(&self) -> Option<u32> {
        self.marks
            .iter()
            .rev()
            .map(|mark| mark.turn)
            .find(|turn| !self.turn_stats(*turn).is_empty())
    }

    /// The snapshots that surround a turn: the one before it and the one
    /// after, which is the worktree as it is for the newest turn (`None`).
    /// Absent where either was not taken.
    fn turn_span(&self, turn: u32) -> Option<(String, Option<String>)> {
        let at = self.turn_at(turn)?;
        let from = self.marks[at].checkpoint.clone()?;
        match self.marks.get(at + 1) {
            Some(next) => Some((from, Some(next.checkpoint.clone()?))),
            None => Some((from, None)),
        }
    }

    /// What the tools of a turn said of one file, as the lines of a diff.
    fn said_diff(&self, turn: u32, path: &str) -> FileDiff {
        let changes = changed::changes_in(self.turn_items(turn));
        match changed::merged(changes, path, &self.cwd) {
            Some(change) => FileDiff::Lines(changed::diff_lines(&change)),
            None => FileDiff::Empty,
        }
    }
}

impl Shell {
    /// The panel of a conversation, opened or closed by its command.
    pub(super) fn chat_diff_toggle(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        if chat.diff.is_some() {
            self.chat_diff_close(id, cx);
            return;
        }
        let source = chat.latest_changed_turn().map_or(Source::All, Source::Turn);
        self.chat_diff_open(id, source, None, false, cx);
    }

    /// Opens the panel on `source`, with `file` selected (the first one
    /// without it), alone when `single`.
    pub(super) fn chat_diff_open(
        &mut self,
        id: LiveId,
        source: Source,
        file: Option<String>,
        single: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let turn = match source {
            Source::Turn(turn) => Some(turn),
            Source::All => chat.latest_changed_turn(),
        };
        let rows = match source {
            Source::Turn(turn) => rows_of_stats(&chat.turn_stats(turn)),
            Source::All => Vec::new(),
        };
        let selected = keep(file.as_deref(), &rows).or(file);
        let scroll = chat
            .diff
            .as_ref()
            .map(|panel| panel.scroll.clone())
            .unwrap_or_default();
        chat.diff = Some(DiffPanel {
            source,
            turn,
            single: single && selected.is_some(),
            selected,
            all: None,
            diff: DiffState::None,
            scroll,
        });
        self.chat_diff_load(id, cx);
    }

    /// Closes the panel.
    pub(super) fn chat_diff_close(&mut self, id: LiveId, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.get_mut(&id) {
            if chat.diff.take().is_some() {
                chat.diff_reads += 1;
                cx.notify();
            }
        }
    }

    /// The rows of the panel's list, for the scope it has.
    fn chat_diff_rows(chat: &Chat, panel: &DiffPanel) -> Vec<Row> {
        match panel.source {
            Source::Turn(turn) => rows_of_stats(&chat.turn_stats(turn)),
            Source::All => panel.all.as_deref().map(rows_of_files).unwrap_or_default(),
        }
    }

    /// Changes the scope of the panel.
    pub(super) fn chat_diff_scope(&mut self, id: LiveId, scope: Scope, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let latest = chat.latest_changed_turn();
        let Some(panel) = chat.diff.as_mut() else {
            return;
        };
        match scope {
            Scope::File => {
                if panel.selected.is_none() {
                    return;
                }
                panel.single = true;
            }
            Scope::All => {
                panel.source = Source::All;
                panel.single = false;
            }
            Scope::Turn => {
                let Some(turn) = panel.turn.or(latest) else {
                    return;
                };
                panel.turn = Some(turn);
                panel.source = Source::Turn(turn);
                panel.single = false;
            }
        }
        if let Some(chat) = self.chats.get_mut(&id) {
            if let Some(panel) = chat.diff.as_ref() {
                let rows = Self::chat_diff_rows(chat, panel);
                let selected =
                    keep(panel.selected.as_deref(), &rows).or_else(|| panel.selected.clone());
                if let Some(panel) = chat.diff.as_mut() {
                    panel.selected = selected;
                }
            }
        }
        self.chat_diff_load(id, cx);
    }

    /// Selects a file of the panel's list.
    pub(super) fn chat_diff_select(&mut self, id: LiveId, path: String, cx: &mut Context<Self>) {
        let Some(panel) = self.chats.get_mut(&id).and_then(|chat| chat.diff.as_mut()) else {
            return;
        };
        if panel.selected.as_deref() == Some(path.as_str()) {
            return;
        }
        panel.selected = Some(path);
        self.chat_diff_load(id, cx);
    }

    /// Reads the diff of the selected file, and for the whole worktree its
    /// list of files first.
    pub(super) fn chat_diff_load(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        chat.diff_reads += 1;
        let read = chat.diff_reads;
        let (machine, cwd) = (chat.machine.clone(), chat.cwd.clone());
        let local = chat.machine.is_local();
        let Some((source, selected)) = chat
            .diff
            .as_ref()
            .map(|panel| (panel.source, panel.selected.clone()))
        else {
            return;
        };
        match source {
            Source::All => {
                if let Some(panel) = chat.diff.as_mut() {
                    panel.diff = DiffState::Loading;
                }
                let reading = self.engine.read_changes(machine, cwd);
                cx.spawn(async move |this, cx| {
                    let answer = reading.await;
                    this.update(cx, |this, cx| this.chat_diff_files(id, read, answer, cx))
                        .ok();
                })
                .detach();
            }
            Source::Turn(turn) => {
                let Some(path) = selected else {
                    if let Some(panel) = chat.diff.as_mut() {
                        panel.diff = DiffState::None;
                    }
                    cx.notify();
                    return;
                };
                let said = chat.said_diff(turn, &path);
                let span = chat.turn_span(turn).filter(|_| local);
                if let Some(panel) = chat.diff.as_mut() {
                    panel.diff = DiffState::Loading;
                }
                let Some((from, to)) = span else {
                    self.chat_diff_loaded(id, read, Ok(said), cx);
                    return;
                };
                cx.spawn(async move |this, cx| {
                    let patch = cx
                        .background_spawn(async move {
                            let repo = checkpoint::Repo::of(std::path::Path::new(&cwd))?;
                            repo.patch(&from, to.as_deref(), &path).ok()
                        })
                        .await;
                    let diff = patch.map_or(said, |patch| parse_diff(&patch));
                    this.update(cx, |this, cx| this.chat_diff_loaded(id, read, Ok(diff), cx))
                        .ok();
                })
                .detach();
            }
        }
    }

    /// The files of the whole worktree were read: the selection is kept
    /// where it can be, and its diff is read.
    fn chat_diff_files(
        &mut self,
        id: LiveId,
        read: u64,
        answer: Result<
            Result<leon_remote::Changes, crate::engine::EngineError>,
            tokio::task::JoinError,
        >,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if chat.diff_reads != read {
            return;
        }
        let (machine, cwd) = (chat.machine.clone(), chat.cwd.clone());
        let Some(panel) = chat.diff.as_mut() else {
            return;
        };
        let files = match answer {
            Ok(Ok(changes)) => changes.files,
            Ok(Err(error)) => {
                panel.diff = DiffState::Failed(error.to_string());
                cx.notify();
                return;
            }
            Err(error) => {
                panel.diff = DiffState::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        let rows = rows_of_files(&files);
        panel.selected = keep(panel.selected.as_deref(), &rows);
        let file = panel
            .selected
            .as_deref()
            .and_then(|path| files.iter().find(|file| file.path == path))
            .cloned();
        panel.all = Some(files);
        let Some(file) = file else {
            panel.diff = DiffState::None;
            cx.notify();
            return;
        };
        let reading = self.engine.read_diff(machine, cwd, file);
        cx.spawn(async move |this, cx| {
            let answer = reading.await;
            let diff = match answer {
                Ok(Ok(diff)) => Ok(diff),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            this.update(cx, |this, cx| this.chat_diff_loaded(id, read, diff, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// A diff was read for the selected file; one that is no longer the
    /// latest asked for is dropped.
    fn chat_diff_loaded(
        &mut self,
        id: LiveId,
        read: u64,
        diff: Result<FileDiff, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if chat.diff_reads != read {
            return;
        }
        let Some(panel) = chat.diff.as_mut() else {
            return;
        };
        panel.diff = match diff {
            Ok(diff) => DiffState::Ready(diff),
            Err(why) => DiffState::Failed(why),
        };
        panel
            .scroll
            .scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
        cx.notify();
    }

    /// Counts what the turn that just ended changed in the worktree, against
    /// the snapshot it began from, off the interface thread. The card says
    /// the answer once it is there.
    pub(super) fn chat_measure(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        if !chat.machine.is_local() {
            return;
        }
        let Some((turn, from)) = chat
            .marks
            .last()
            .and_then(|mark| Some((mark.turn, mark.checkpoint.clone()?)))
        else {
            return;
        };
        let cwd = chat.cwd.clone();
        cx.spawn(async move |this, cx| {
            let stats = cx
                .background_spawn(async move {
                    checkpoint::Repo::of(std::path::Path::new(&cwd))?
                        .numstat(&from, None)
                        .ok()
                })
                .await;
            let Some(stats) = stats else {
                return;
            };
            this.update(cx, |this, cx| {
                let Some(chat) = this.chats.get_mut(&id) else {
                    return;
                };
                if chat.turn_at(turn).is_none() {
                    return;
                }
                chat.measured.insert(turn, stats);
                let showing = chat
                    .diff
                    .as_ref()
                    .is_some_and(|panel| panel.source == Source::Turn(turn));
                if showing {
                    this.chat_diff_reselect(id, turn, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The count of a turn that is on show changed: its selection is kept
    /// where it can be, and the diff is read again.
    fn chat_diff_reselect(&mut self, id: LiveId, turn: u32, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let rows = rows_of_stats(&chat.turn_stats(turn));
        if let Some(panel) = chat.diff.as_mut() {
            panel.selected = keep(panel.selected.as_deref(), &rows);
        }
        self.chat_diff_load(id, cx);
    }

    /// The card at the end of a turn that changed files.
    pub(super) fn chat_changed_card(
        &self,
        id: LiveId,
        chat: &Chat,
        turn: u32,
        stats: &[Stat],
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (added, removed) = changed::totals(stats);
        let open = chat
            .diff
            .as_ref()
            .is_some_and(|panel| panel.source == Source::Turn(turn));
        let hover = colours.surface_2;
        let head = SharedString::from(format!("chat-changed-open-{}-{turn}", id.0));
        let mut card = div()
            .debug_selector(move || format!("chat-changed-{}-{turn}", id.0))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(if open { colours.signal } else { colours.border })
            .child(
                div()
                    .id(head.clone())
                    .debug_selector(move || head.to_string())
                    .h(px(28.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.chat_diff_open(id, Source::Turn(turn), None, false, cx);
                    }))
                    .child(led(colours.success))
                    .child(mono(changed::title(stats)).text_color(colours.text))
                    .child(div().flex_1())
                    .child(mono("VIEW DIFF").text_color(colours.text_faint)),
            );
        let _ = (added, removed);
        for (at, stat) in stats.iter().take(CARD_FILES).enumerate() {
            let path = stat.path.clone();
            let tip = SharedString::from(stat.path.clone());
            let name = format!("chat-changed-file-{}-{turn}-{at}", id.0);
            let path_name = format!("chat-changed-path-{}-{turn}-{at}", id.0);
            let file_name = format!("chat-changed-name-{}-{turn}-{at}", id.0);
            let (dir, file) = split_name(&stat.path);
            // One line: the folders give way (cut with an ellipsis) so the
            // file name always stays whole. The row grows with its text, the
            // card with its rows: nothing has a fixed height.
            card = card.child(
                div()
                    .id(SharedString::from(name.clone()))
                    .debug_selector(move || name.clone())
                    .min_h(px(22.))
                    .px(px(10.))
                    .py(px(2.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .border_t_1()
                    .border_color(colours.border)
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.chat_diff_open(id, Source::Turn(turn), Some(path.clone()), true, cx);
                    }))
                    .child(
                        div()
                            .debug_selector(move || path_name.clone())
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .whitespace_nowrap()
                            .font_family(fonts::mono())
                            .text_size(metrics::TEXT_SMALL())
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(colours.text_faint)
                                    .child(dir.to_owned()),
                            )
                            .child(
                                div()
                                    .debug_selector(move || file_name.clone())
                                    .flex_shrink_0()
                                    .max_w_full()
                                    .text_color(colours.text)
                                    .child(file.to_owned()),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .child(counts_of(stat.added, stat.removed, colours)),
                    ),
            );
        }
        if stats.len() > CARD_FILES {
            card = card.child(
                div()
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(colours.border)
                    .child(
                        mono(format!("+{} MORE", stats.len() - CARD_FILES))
                            .text_color(colours.text_faint),
                    ),
            );
        }
        card.into_any_element()
    }

    /// The panel on the right of a conversation, when it is open.
    pub(super) fn chat_diff_panel(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = chat.diff.as_ref()?;
        let rows = Self::chat_diff_rows(chat, panel);
        let chip = |name: &'static str,
                    label: &'static str,
                    scope: Scope,
                    enabled: bool,
                    cx: &mut Context<Self>| {
            let selector = format!("chat-diff-scope-{name}-{}", id.0);
            let active = panel.scope() == scope;
            let hover = colours.surface_2;
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone())
                .flex_none()
                .h(px(22.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if active {
                    colours.signal
                } else {
                    colours.border
                })
                .child(mono(label).text_color(if active {
                    colours.text
                } else if enabled {
                    colours.text_muted
                } else {
                    colours.text_faint
                }))
                .when(enabled, |this| {
                    this.cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.chat_diff_scope(id, scope, cx);
                        }))
                })
        };
        let can_turn = panel.turn.is_some() || chat.latest_changed_turn().is_some();
        let hover = colours.surface_2;
        let close = SharedString::from(format!("chat-diff-close-{}", id.0));
        let header = div()
            .flex_none()
            .h(px(36.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .border_b_1()
            .border_color(colours.border)
            .child(mono("CHANGES").text_color(colours.text_faint))
            .child(div().flex_1())
            .child(chip("turn", "THIS TURN", Scope::Turn, can_turn, cx))
            .child(chip(
                "file",
                "FILE",
                Scope::File,
                panel.selected.is_some(),
                cx,
            ))
            .child(chip("all", "ALL UNCOMMITTED", Scope::All, true, cx))
            .child(
                div()
                    .id(close.clone())
                    .debug_selector(move || close.to_string())
                    .flex_none()
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(metrics::RADIUS())
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.chat_diff_close(id, cx);
                    }))
                    .child(icon(IconName::X, px(12.), colours.text_muted)),
            );
        let list = (!panel.single).then(|| {
            let mut list = div()
                .id(SharedString::from(format!("chat-diff-files-{}", id.0)))
                .flex_none()
                .max_h(px(168.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .border_b_1()
                .border_color(colours.border);
            if rows.is_empty() {
                list = list.child(
                    div()
                        .h(px(28.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_faint)
                        .child(if panel.source == Source::All && panel.all.is_none() {
                            "Reading the changes\u{2026}"
                        } else {
                            "No changes."
                        }),
                );
            }
            for (at, row) in rows.iter().enumerate() {
                let name = format!("chat-diff-file-{}-{at}", id.0);
                let selected = panel.selected.as_deref() == Some(row.path.as_str());
                let path = row.path.clone();
                let (dir, file) = split_name(&row.path);
                list = list.child(
                    div()
                        .id(SharedString::from(name.clone()))
                        .debug_selector(move || name.clone())
                        .flex_none()
                        .h(px(24.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .when(selected, |this| this.bg(hover))
                        .hover(move |style| style.bg(hover))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.chat_diff_select(id, path.clone(), cx);
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(fonts::mono())
                                .text_size(metrics::TEXT_SMALL())
                                .child(div().text_color(colours.text_faint).child(dir.to_owned()))
                                .child(div().text_color(colours.text).child(file.to_owned())),
                        )
                        .children(
                            row.counts
                                .map(|(added, removed)| counts_of(added, removed, colours)),
                        )
                        .children(row.status.map(|status| {
                            mono(status.words().to_uppercase()).text_color(colours.text_faint)
                        })),
                );
            }
            list
        });
        let title = panel
            .selected
            .clone()
            .unwrap_or_else(|| "No file selected".to_owned());
        let body = self.diff_body(
            id,
            &panel.diff,
            &panel.scroll,
            &NAMES,
            panel_lines,
            colours,
            cx,
        );
        Some(
            div()
                .debug_selector(move || format!("chat-diff-panel-{}", id.0))
                .flex_none()
                .w(gpui_kit::relative(0.45))
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(colours.border)
                .bg(colours.background)
                .child(header)
                .children(list)
                .child(
                    div()
                        .flex_none()
                        .h(px(28.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .min_w_0()
                        .truncate()
                        .border_b_1()
                        .border_color(colours.border)
                        .font_family(fonts::mono())
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(title),
                )
                .child(body)
                .into_any_element(),
        )
    }
}

/// `+12 -4`, the added in the colour of an addition and the removed in the
/// colour of a removal.
fn counts_of(added: usize, removed: usize, colours: &Palette) -> gpui_kit::Div {
    div()
        .flex_none()
        .flex()
        .gap(px(6.))
        .font_family(fonts::mono())
        .text_size(metrics::TEXT_SMALL())
        .child(div().text_color(colours.success).child(format!("+{added}")))
        .child(div().text_color(colours.error).child(format!("-{removed}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(path: &str) -> Stat {
        Stat {
            path: path.into(),
            added: 1,
            removed: 0,
        }
    }

    #[test]
    fn the_selection_stays_on_its_file_while_it_is_there_and_goes_to_the_first_when_not() {
        let rows = rows_of_stats(&[stat("a.rs"), stat("b.rs")]);
        assert_eq!(keep(Some("b.rs"), &rows).as_deref(), Some("b.rs"));
        assert_eq!(keep(Some("gone.rs"), &rows).as_deref(), Some("a.rs"));
        assert_eq!(keep(None, &rows).as_deref(), Some("a.rs"));
        assert_eq!(keep(Some("a.rs"), &[]), None);
    }

    #[test]
    fn a_panel_is_scoped_to_a_turn_a_file_or_the_whole_worktree() {
        let panel = |source, single| DiffPanel {
            source,
            turn: None,
            single,
            selected: None,
            all: None,
            diff: DiffState::None,
            scroll: UniformListScrollHandle::new(),
        };
        assert_eq!(panel(Source::Turn(3), false).scope(), Scope::Turn);
        assert_eq!(panel(Source::Turn(3), true).scope(), Scope::File);
        assert_eq!(panel(Source::All, false).scope(), Scope::All);
        assert_eq!(panel(Source::All, true).scope(), Scope::File);
    }
}

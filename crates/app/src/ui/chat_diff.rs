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
use gpui_kit::{
    div, AnyElement, ClickEvent, Context, SharedString, UniformListScrollHandle, Window,
};
use leon_remote::{parse_diff, ChangedFile, DiffLine, FileDiff, FileStatus};

use super::changes::DiffState;
use super::changes_view::{split_name, DiffNames};
use super::edits::{Edits, Subject};
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
    /// What the pane says of the selected file when it is not text: its size,
    /// or that it is gone.
    pub note: Option<String>,
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
    /// It is not text: there are no lines to count.
    pub binary: bool,
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
            binary: stat.binary,
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
            binary: false,
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

/// The lines of the diff the panel of `subject` holds.
fn panel_lines(shell: &Shell, subject: Subject) -> Option<&[DiffLine]> {
    match shell.edits(subject)?.diff.as_ref().map(|panel| &panel.diff) {
        Some(DiffState::Ready(FileDiff::Lines(lines))) => Some(lines.as_slice()),
        _ => None,
    }
}

impl Edits {
    /// The position among its marks of the turn numbered `turn`.
    fn turn_at(&self, turn: u32) -> Option<usize> {
        self.marks.iter().position(|mark| mark.turn == turn)
    }

    /// The items of the turn numbered `turn`: what was said and done in it.
    pub fn turn_items<'a>(&self, items: &'a [Item], turn: u32) -> &'a [Item] {
        match self.turn_at(turn) {
            Some(at) => &items[changed::turn_range(&self.marks, at, items.len())],
            None => &[],
        }
    }

    /// What the turn numbered `turn` changed: git's count when it was made,
    /// else what its tools said, and only when a tool that can change files
    /// ran in it (see [`changed::of_turn`]).
    pub fn turn_stats(&self, items: &[Item], cwd: &str, turn: u32) -> Vec<Stat> {
        changed::of_turn(
            self.turn_items(items, turn),
            cwd,
            self.measured.get(&turn).map(Vec::as_slice),
        )
    }

    /// The cards of the conversation: for each turn that is over and changed
    /// files, where its items end, its number and what it changed. `going`:
    /// a turn is in flight, which has no card yet.
    pub fn cards(&self, items: &[Item], cwd: &str, going: bool) -> Vec<(usize, u32, Vec<Stat>)> {
        self.marks
            .iter()
            .enumerate()
            .filter_map(|(at, mark)| {
                if going && at + 1 == self.marks.len() {
                    return None;
                }
                let stats = self.turn_stats(items, cwd, mark.turn);
                let end = changed::turn_range(&self.marks, at, items.len()).end;
                (!stats.is_empty()).then_some((end, mark.turn, stats))
            })
            .collect()
    }

    /// The newest turn that changed files.
    pub fn latest_changed_turn(&self, items: &[Item], cwd: &str) -> Option<u32> {
        self.marks
            .iter()
            .rev()
            .map(|mark| mark.turn)
            .find(|turn| !self.turn_stats(items, cwd, *turn).is_empty())
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
    fn said_diff(&self, items: &[Item], cwd: &str, turn: u32, path: &str) -> FileDiff {
        let changes = changed::changes_in(self.turn_items(items, turn));
        match changed::merged(changes, path, cwd) {
            Some(change) => FileDiff::Lines(changed::diff_lines(&change)),
            None => FileDiff::Empty,
        }
    }
}

#[cfg(test)]
impl super::chat::Chat {
    /// What the turn numbered `turn` changed.
    #[cfg_attr(not(leon_posix_tests), allow(dead_code))]
    pub fn turn_stats(&self, turn: u32) -> Vec<Stat> {
        self.edits
            .turn_stats(&self.conversation.items, &self.cwd, turn)
    }
}

impl Shell {
    /// Lists all the files of the card that ends `turn`, or only the first
    /// few again.
    pub(super) fn chat_card_toggle(&mut self, subject: Subject, turn: u32, cx: &mut Context<Self>) {
        if let Some(edits) = self.edits_mut(subject) {
            if !edits.expanded.remove(&turn) {
                edits.expanded.insert(turn);
            }
            cx.notify();
        }
    }

    /// Opens a file a conversation changed, named from the root of its
    /// worktree: in Leon when it is text, with the program the system has for
    /// it when it is not (a folder in the file manager), and nothing when it
    /// is gone. On another machine only Leon's editor can.
    pub(super) fn chat_open_changed(
        &mut self,
        subject: Subject,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(scene) = self.scene(subject) else {
            return;
        };
        let (machine, cwd) = (scene.machine.clone(), scene.cwd.to_owned());
        if machine.is_local() {
            let file = locate(std::path::Path::new(&cwd), path);
            self.open_path(&file, None, window, cx);
        } else {
            let file = std::path::Path::new(&cwd).join(path);
            self.open_file(
                machine,
                file.to_string_lossy().into_owned(),
                None,
                window,
                cx,
            );
        }
    }

    /// The panel of a conversation, opened or closed by its command.
    pub(super) fn chat_diff_toggle(&mut self, subject: Subject, cx: &mut Context<Self>) {
        let Some(scene) = self.scene(subject) else {
            return;
        };
        let Some(edits) = self.edits(subject) else {
            return;
        };
        if edits.diff.is_some() {
            self.chat_diff_close(subject, cx);
            return;
        }
        let source = edits
            .latest_changed_turn(scene.items, scene.cwd)
            .map_or(Source::All, Source::Turn);
        self.chat_diff_open(subject, source, None, false, cx);
    }

    /// Opens the panel on `source`, with `file` selected (the first one
    /// without it), alone when `single`.
    pub(super) fn chat_diff_open(
        &mut self,
        subject: Subject,
        source: Source,
        file: Option<String>,
        single: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(scene) = self.scene(subject) else {
            return;
        };
        let Some(edits) = self.edits(subject) else {
            return;
        };
        let turn = match source {
            Source::Turn(turn) => Some(turn),
            Source::All => edits.latest_changed_turn(scene.items, scene.cwd),
        };
        let rows = match source {
            Source::Turn(turn) => rows_of_stats(&edits.turn_stats(scene.items, scene.cwd, turn)),
            Source::All => Vec::new(),
        };
        let selected = keep(file.as_deref(), &rows).or(file);
        let scroll = edits
            .diff
            .as_ref()
            .map(|panel| panel.scroll.clone())
            .unwrap_or_default();
        if let Some(edits) = self.edits_mut(subject) {
            edits.diff = Some(DiffPanel {
                source,
                turn,
                single: single && selected.is_some(),
                selected,
                all: None,
                diff: DiffState::None,
                scroll,
                note: None,
            });
        }
        self.chat_diff_load(subject, cx);
    }

    /// Closes the panel.
    pub(super) fn chat_diff_close(&mut self, subject: Subject, cx: &mut Context<Self>) {
        if let Some(edits) = self.edits_mut(subject) {
            if edits.diff.take().is_some() {
                edits.diff_reads += 1;
                cx.notify();
            }
        }
    }

    /// The rows of the panel's list, for the scope it has.
    fn chat_diff_rows(&self, subject: Subject, panel: &DiffPanel) -> Vec<Row> {
        match panel.source {
            Source::Turn(turn) => match (self.scene(subject), self.edits(subject)) {
                (Some(scene), Some(edits)) => {
                    rows_of_stats(&edits.turn_stats(scene.items, scene.cwd, turn))
                }
                _ => Vec::new(),
            },
            Source::All => panel.all.as_deref().map(rows_of_files).unwrap_or_default(),
        }
    }

    /// Changes the scope of the panel.
    pub(super) fn chat_diff_scope(
        &mut self,
        subject: Subject,
        scope: Scope,
        cx: &mut Context<Self>,
    ) {
        let Some(scene) = self.scene(subject) else {
            return;
        };
        let Some(edits) = self.edits(subject) else {
            return;
        };
        let latest = edits.latest_changed_turn(scene.items, scene.cwd);
        let Some(panel) = edits.diff.as_ref() else {
            return;
        };
        let (mut source, mut turn) = (panel.source, panel.turn);
        let single;
        match scope {
            Scope::File => {
                if panel.selected.is_none() {
                    return;
                }
                single = true;
            }
            Scope::All => {
                source = Source::All;
                single = false;
            }
            Scope::Turn => {
                let Some(wanted) = turn.or(latest) else {
                    return;
                };
                turn = Some(wanted);
                source = Source::Turn(wanted);
                single = false;
            }
        }
        let panel = DiffPanel {
            source,
            single,
            turn,
            selected: panel.selected.clone(),
            all: panel.all.clone(),
            diff: DiffState::None,
            scroll: panel.scroll.clone(),
            note: None,
        };
        let rows = self.chat_diff_rows(subject, &panel);
        let selected = keep(panel.selected.as_deref(), &rows).or_else(|| panel.selected.clone());
        if let Some(open) = self
            .edits_mut(subject)
            .and_then(|edits| edits.diff.as_mut())
        {
            open.source = source;
            open.single = single;
            open.turn = turn;
            open.selected = selected;
        }
        self.chat_diff_load(subject, cx);
    }

    /// Selects a file of the panel's list.
    pub(super) fn chat_diff_select(
        &mut self,
        subject: Subject,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self
            .edits_mut(subject)
            .and_then(|edits| edits.diff.as_mut())
        else {
            return;
        };
        if panel.selected.as_deref() == Some(path.as_str()) {
            return;
        }
        panel.selected = Some(path);
        self.chat_diff_load(subject, cx);
    }

    /// Reads the diff of the selected file, and for the whole worktree its
    /// list of files first.
    pub(super) fn chat_diff_load(&mut self, subject: Subject, cx: &mut Context<Self>) {
        let Some(scene) = self.scene(subject) else {
            return;
        };
        let (machine, cwd) = (scene.machine.clone(), scene.cwd.to_owned());
        let local = machine.is_local();
        let Some(edits) = self.edits(subject) else {
            return;
        };
        let Some((source, selected)) = edits
            .diff
            .as_ref()
            .map(|panel| (panel.source, panel.selected.clone()))
        else {
            return;
        };
        let said = match (source, &selected) {
            (Source::Turn(turn), Some(path)) => {
                Some(edits.said_diff(scene.items, scene.cwd, turn, path))
            }
            _ => None,
        };
        let span = match source {
            Source::Turn(turn) => edits.turn_span(turn).filter(|_| local),
            Source::All => None,
        };
        let Some(edits) = self.edits_mut(subject) else {
            return;
        };
        edits.diff_reads += 1;
        let read = edits.diff_reads;
        match source {
            Source::All => {
                if let Some(panel) = edits.diff.as_mut() {
                    panel.diff = DiffState::Loading;
                }
                let reading = self.engine.read_changes(machine, cwd);
                cx.spawn(async move |this, cx| {
                    let answer = reading.await;
                    this.update(cx, |this, cx| {
                        this.chat_diff_files(subject, read, answer, cx)
                    })
                    .ok();
                })
                .detach();
            }
            Source::Turn(_) => {
                let (Some(path), Some(said)) = (selected, said) else {
                    if let Some(panel) = edits.diff.as_mut() {
                        panel.diff = DiffState::None;
                    }
                    cx.notify();
                    return;
                };
                if let Some(panel) = edits.diff.as_mut() {
                    panel.diff = DiffState::Loading;
                }
                let Some((from, to)) = span else {
                    self.chat_diff_loaded(subject, read, Ok(said), cx);
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
                    this.update(cx, |this, cx| {
                        this.chat_diff_loaded(subject, read, Ok(diff), cx)
                    })
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
        subject: Subject,
        read: u64,
        answer: Result<
            Result<leon_remote::Changes, crate::engine::EngineError>,
            tokio::task::JoinError,
        >,
        cx: &mut Context<Self>,
    ) {
        let Some(scene) = self.scene(subject) else {
            return;
        };
        let (machine, cwd) = (scene.machine.clone(), scene.cwd.to_owned());
        let Some(edits) = self.edits_mut(subject) else {
            return;
        };
        if edits.diff_reads != read {
            return;
        }
        let Some(panel) = edits.diff.as_mut() else {
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
            this.update(cx, |this, cx| {
                this.chat_diff_loaded(subject, read, diff, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// A diff was read for the selected file; one that is no longer the
    /// latest asked for is dropped.
    fn chat_diff_loaded(
        &mut self,
        subject: Subject,
        read: u64,
        diff: Result<FileDiff, String>,
        cx: &mut Context<Self>,
    ) {
        // Where the files are, when they are on this computer: a file that
        // is not text is told by its size.
        let here = self
            .scene(subject)
            .filter(|scene| scene.machine.is_local())
            .map(|scene| scene.cwd.to_owned());
        let Some(edits) = self.edits_mut(subject) else {
            return;
        };
        if edits.diff_reads != read {
            return;
        }
        let Some(panel) = edits.diff.as_mut() else {
            return;
        };
        panel.note = match (&diff, &panel.selected, here) {
            (Ok(FileDiff::Binary), Some(path), Some(cwd)) => {
                let file = locate(std::path::Path::new(&cwd), path);
                Some(match std::fs::metadata(&file) {
                    Ok(meta) => size_words(meta.len()),
                    Err(_) => "deleted".to_owned(),
                })
            }
            _ => None,
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
    pub(super) fn chat_measure(&mut self, subject: Subject, cx: &mut Context<Self>) {
        let Some(scene) = self.scene(subject) else {
            return;
        };
        if !scene.machine.is_local() {
            return;
        }
        let Some((turn, from)) = self
            .edits(subject)
            .and_then(|edits| edits.marks.last())
            .and_then(|mark| Some((mark.turn, mark.checkpoint.clone()?)))
        else {
            return;
        };
        self.measure_between(subject, vec![(turn, from, None)], cx);
    }

    /// Counts what turns changed in the worktree, off the interface thread:
    /// for each, its number, the snapshot it began from and the one it
    /// ended at (`None`: the worktree as it is).
    pub(super) fn measure_between(
        &mut self,
        subject: Subject,
        jobs: Vec<(u32, String, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        let Some(scene) = self.scene(subject) else {
            return;
        };
        let cwd = scene.cwd.to_owned();
        let turns: Vec<u32> = jobs.iter().map(|(turn, ..)| *turn).collect();
        if let Some(edits) = self.edits_mut(subject) {
            edits.counting.extend(turns.iter().copied());
        }
        cx.spawn(async move |this, cx| {
            let counted = cx
                .background_spawn(async move {
                    let repo = checkpoint::Repo::of(std::path::Path::new(&cwd))?;
                    let counted: Vec<(u32, Vec<Stat>)> = jobs
                        .into_iter()
                        .filter_map(|(turn, from, to)| {
                            Some((turn, repo.numstat(&from, to.as_deref()).ok()?))
                        })
                        .collect();
                    Some(counted)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(edits) = this.edits_mut(subject) {
                    for turn in &turns {
                        edits.counting.remove(turn);
                    }
                }
                for (turn, stats) in counted.unwrap_or_default() {
                    let Some(edits) = this.edits_mut(subject) else {
                        return;
                    };
                    if edits.turn_at(turn).is_none() {
                        continue;
                    }
                    edits.measured.insert(turn, stats);
                    let showing = edits
                        .diff
                        .as_ref()
                        .is_some_and(|panel| panel.source == Source::Turn(turn));
                    if showing {
                        this.chat_diff_reselect(subject, turn, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The count of a turn that is on show changed: its selection is kept
    /// where it can be, and the diff is read again.
    fn chat_diff_reselect(&mut self, subject: Subject, turn: u32, cx: &mut Context<Self>) {
        let (Some(scene), Some(edits)) = (self.scene(subject), self.edits(subject)) else {
            return;
        };
        let rows = rows_of_stats(&edits.turn_stats(scene.items, scene.cwd, turn));
        if let Some(panel) = self
            .edits_mut(subject)
            .and_then(|edits| edits.diff.as_mut())
        {
            panel.selected = keep(panel.selected.as_deref(), &rows);
        }
        self.chat_diff_load(subject, cx);
    }

    /// The card at the end of a turn that changed files.
    pub(super) fn chat_changed_card(
        &self,
        subject: Subject,
        turn: u32,
        stats: &[Stat],
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (added, removed) = changed::totals(stats);
        let tag = subject.tag();
        let open = self
            .edits(subject)
            .and_then(|edits| edits.diff.as_ref())
            .is_some_and(|panel| panel.source == Source::Turn(turn));
        let hover = colours.surface_2;
        let head = SharedString::from(format!("chat-changed-open-{tag}-{turn}"));
        let mut card = div()
            .debug_selector({
                let tag = tag.clone();
                move || format!("chat-changed-{tag}-{turn}")
            })
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
                        this.chat_diff_open(subject, Source::Turn(turn), None, false, cx);
                    }))
                    .child(led(colours.success))
                    .child(mono(changed::title(stats)).text_color(colours.text))
                    .child(div().flex_1())
                    .child(mono("VIEW DIFF").text_color(colours.text_faint)),
            );
        let _ = (added, removed);
        let all = self
            .edits(subject)
            .is_some_and(|edits| edits.expanded.contains(&turn));
        let shown = if all { stats.len() } else { CARD_FILES };
        for (at, stat) in stats.iter().take(shown).enumerate() {
            let path = stat.path.clone();
            let for_open = stat.path.clone();
            let group = SharedString::from(format!("chat-changed-group-{tag}-{turn}-{at}"));
            let open_name = format!("chat-changed-open-file-{tag}-{turn}-{at}");
            let counts_name = format!("chat-changed-counts-{tag}-{turn}-{at}");
            let tip = SharedString::from(stat.path.clone());
            let name = format!("chat-changed-file-{tag}-{turn}-{at}");
            let path_name = format!("chat-changed-path-{tag}-{turn}-{at}");
            let file_name = format!("chat-changed-name-{tag}-{turn}-{at}");
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
                    .group(group.clone())
                    .hover(move |style| style.bg(hover))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        // Ctrl or Cmd, or a second click, opens the file
                        // itself; a click shows what changed in it.
                        if opens_file(event) {
                            this.chat_open_changed(subject, &path, window, cx);
                        } else {
                            this.chat_diff_open(
                                subject,
                                Source::Turn(turn),
                                Some(path.clone()),
                                true,
                                cx,
                            );
                        }
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
                    .child(open_button(
                        open_name,
                        group,
                        colours,
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            this.chat_open_changed(subject, &for_open, window, cx);
                        }),
                    ))
                    .child(div().flex_none().child(counts_text(
                        counts_name,
                        stat.added,
                        stat.removed,
                        stat.binary,
                        colours,
                    ))),
            );
        }
        if stats.len() > CARD_FILES {
            let name = format!("chat-changed-more-{tag}-{turn}");
            let label = if all {
                "SHOW LESS".to_owned()
            } else {
                format!("+{} MORE", stats.len() - CARD_FILES)
            };
            card = card.child(
                div()
                    .id(SharedString::from(name.clone()))
                    .debug_selector(move || name.clone())
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(colours.border)
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.chat_card_toggle(subject, turn, cx);
                    }))
                    .child(mono(label).text_color(colours.text_muted)),
            );
        }
        card.into_any_element()
    }

    /// The panel on the right of a conversation, when it is open.
    pub(super) fn chat_diff_panel(
        &self,
        subject: Subject,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let scene = self.scene(subject)?;
        let edits = self.edits(subject)?;
        let panel = edits.diff.as_ref()?;
        let tag = subject.tag();
        let rows = self.chat_diff_rows(subject, panel);
        let chip = |name: &'static str,
                    label: &'static str,
                    scope: Scope,
                    enabled: bool,
                    cx: &mut Context<Self>| {
            let selector = format!("chat-diff-scope-{name}-{tag}");
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
                            this.chat_diff_scope(subject, scope, cx);
                        }))
                })
        };
        let can_turn =
            panel.turn.is_some() || edits.latest_changed_turn(scene.items, scene.cwd).is_some();
        let hover = colours.surface_2;
        let close = SharedString::from(format!("chat-diff-close-{tag}"));
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
                        this.chat_diff_close(subject, cx);
                    }))
                    .child(icon(IconName::X, px(12.), colours.text_muted)),
            );
        let list = (!panel.single).then(|| {
            let mut list = div()
                .id(SharedString::from(format!("chat-diff-files-{tag}")))
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
                let name = format!("chat-diff-file-{tag}-{at}");
                let selected = panel.selected.as_deref() == Some(row.path.as_str());
                let path = row.path.clone();
                let (dir, file) = split_name(&row.path);
                let folders = folder_budget(self.viewport.width.as_f32(), file.chars().count());
                let group = SharedString::from(format!("chat-diff-group-{tag}-{at}"));
                let open_name = format!("chat-diff-open-file-{tag}-{at}");
                let for_open = row.path.clone();
                let tip = SharedString::from(row.path.clone());
                list = list.child(
                    div()
                        .id(SharedString::from(name.clone()))
                        .debug_selector(move || name.clone())
                        .group(group.clone())
                        .flex_none()
                        .h(px(ROW_HEIGHT))
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .when(selected, |this| this.bg(hover))
                        .hover(move |style| style.bg(hover))
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            if opens_file(event) {
                                this.chat_open_changed(subject, &path, window, cx);
                            } else {
                                this.chat_diff_select(subject, path.clone(), cx);
                            }
                        }))
                        // One line: the file name whole and in the text
                        // colour, then its folders, dim, cut with an
                        // ellipsis where they do not fit.
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .whitespace_nowrap()
                                .font_family(fonts::mono())
                                .text_size(metrics::TEXT_SMALL())
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w_full()
                                        .text_color(colours.text)
                                        .child(file.to_owned()),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(colours.text_faint)
                                        .child(
                                            fit_folders(dir, folders)
                                                .trim_end_matches('/')
                                                .to_owned(),
                                        ),
                                ),
                        )
                        .child(open_button(
                            open_name,
                            group,
                            colours,
                            cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.chat_open_changed(subject, &for_open, window, cx);
                            }),
                        ))
                        .children(
                            row.counts
                                .map(|(added, removed)| {
                                    counts_text(
                                        format!("chat-diff-counts-{tag}-{at}"),
                                        added,
                                        removed,
                                        row.binary,
                                        colours,
                                    )
                                })
                                .map(|counts| div().flex_none().child(counts)),
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
        // A file that is not text has no diff: the pane says what it is, and
        // opens it.
        let body = match (&panel.diff, panel.selected.clone()) {
            (DiffState::Ready(FileDiff::Binary), Some(path)) => {
                let tag = tag.clone();
                let open = open_file_button(
                    format!("chat-diff-binary-open-{tag}"),
                    colours,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.chat_open_changed(subject, &path, window, cx);
                    }),
                );
                div()
                    .debug_selector(|| NAMES.binary.to_owned())
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint)
                    .child(binary_words(panel.note.as_deref()))
                    .child(open)
                    .into_any_element()
            }
            _ => self.diff_body(
                subject,
                &panel.diff,
                &panel.scroll,
                &NAMES,
                panel_lines,
                colours,
                cx,
            ),
        };
        let open_selected = panel.selected.clone().map(|path| {
            open_file_button(
                format!("chat-diff-open-selected-{tag}"),
                colours,
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.chat_open_changed(subject, &path, window, cx);
                }),
            )
        });
        Some(
            div()
                .debug_selector(move || format!("chat-diff-panel-{tag}"))
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
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .children(open_selected),
                )
                .child(body)
                .into_any_element(),
        )
    }
}

/// How tall a row of the panel's list is: one line, always.
const ROW_HEIGHT: f32 = 24.;

/// How many characters of folders fit in a row of the panel's list next to
/// the file name, for a window `viewport` pixels wide: the panel is a bit
/// less than half of it, and the row keeps room for the counts and the OPEN
/// button. An estimate, as wide as the letters of the mono font are.
pub fn folder_budget(viewport: f32, file_chars: usize) -> usize {
    const LETTER: f32 = 7.5;
    const KEPT: f32 = 20. + 70. + 44. + 16.;
    let panel = viewport * 0.4;
    let room = (panel - KEPT) / LETTER;
    (room as isize - file_chars as isize).max(0) as usize
}

/// The folders of a path (`marketing/video/public/audio/`) in at most
/// `budget` characters, keeping the end, the part nearest the file:
/// `\u{2026}/public/audio/`. Whole when it fits; whole folders when any fit,
/// else the last letters.
pub fn fit_folders(folders: &str, budget: usize) -> String {
    let total = folders.chars().count();
    if total <= budget {
        return folders.to_owned();
    }
    const CUT: &str = "\u{2026}/";
    let mut kept = String::new();
    for part in folders.split('/').filter(|part| !part.is_empty()).rev() {
        let next = format!("{part}/{kept}");
        if CUT.chars().count() + next.chars().count() > budget {
            break;
        }
        kept = next;
    }
    if !kept.is_empty() {
        return format!("{CUT}{kept}");
    }
    // Not even the last folder fits: its last letters.
    match budget {
        0 => String::new(),
        n => {
            let tail: String = folders.chars().skip(total.saturating_sub(n - 1)).collect();
            format!("\u{2026}{tail}")
        }
    }
}

/// What the pane of a file that is not text says: that, and what is known of
/// its size.
pub fn binary_words(note: Option<&str>) -> String {
    match note {
        Some(note) => format!("Binary file \u{2014} not shown \u{b7} {note}"),
        None => "Binary file \u{2014} not shown".to_owned(),
    }
}

/// How a file's size is said in the pane of a binary file: `84.2 KB`.
pub fn size_words(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Whether a click on a row opens the file itself: with Ctrl (Cmd on a Mac)
/// held, or a second click. A plain click shows the changes.
fn opens_file(event: &ClickEvent) -> bool {
    event.click_count() >= 2 || event.modifiers().secondary()
}

/// The counts of a file in a list: `+12 -4`, or `BINARY` for one that is not
/// text and has no lines to count.
fn counts_text(
    selector: String,
    added: usize,
    removed: usize,
    binary: bool,
    colours: &Palette,
) -> gpui_kit::Div {
    if binary {
        div()
            .debug_selector(move || format!("{selector}-binary"))
            .flex_none()
            .child(mono("BINARY").text_color(colours.text_faint))
    } else {
        counts_of(added, removed, colours).debug_selector(move || selector.clone())
    }
}

/// The button of a row that opens its file, shown while the row is hovered
/// (`group` is the row's).
fn open_button(
    selector: String,
    group: SharedString,
    colours: &Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    let hover = colours.surface_2;
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .flex_none()
        .h(px(18.))
        .px(px(5.))
        .flex()
        .items_center()
        .rounded(metrics::RADIUS())
        .cursor_pointer()
        .invisible()
        .group_hover(group, |style| style.visible())
        .hover(move |style| style.bg(hover))
        .on_click(on_click)
        .child(mono("OPEN").text_color(colours.text_muted))
}

/// A button that opens the selected file, always shown.
fn open_file_button(
    selector: String,
    colours: &Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    let hover = colours.surface_2;
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .flex_none()
        .h(px(20.))
        .px(px(6.))
        .flex()
        .items_center()
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(colours.border)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(on_click)
        .child(mono("OPEN FILE").text_color(colours.text_muted))
}

/// Where `path`, named from the root of a worktree, is, for a conversation
/// that works in `cwd` (the root or a folder below it): the first folder from
/// `cwd` up that has it.
pub fn locate(cwd: &std::path::Path, path: &str) -> std::path::PathBuf {
    cwd.ancestors()
        .map(|folder| folder.join(path))
        .find(|file| file.exists())
        .unwrap_or_else(|| cwd.join(path))
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

    #[test]
    fn folders_keep_the_end_nearest_the_file_when_they_do_not_fit() {
        let dir = "marketing/nely-social-video/public/audio/";
        assert_eq!(fit_folders(dir, 80), dir, "whole when it fits");
        assert_eq!(fit_folders(dir, dir.chars().count()), dir);
        assert_eq!(fit_folders(dir, 30), "\u{2026}/public/audio/");
        assert_eq!(
            fit_folders(dir, 40),
            "\u{2026}/nely-social-video/public/audio/"
        );
        assert_eq!(fit_folders(dir, 17), "\u{2026}/public/audio/");
        // No whole folder fits: the last letters, with the cut shown.
        assert_eq!(fit_folders(dir, 5), "\u{2026}dio/");
        assert_eq!(fit_folders(dir, 1), "\u{2026}");
        assert_eq!(fit_folders(dir, 0), "");
        assert_eq!(fit_folders("", 0), "");
        // Letters are counted, not bytes.
        assert_eq!(
            fit_folders("m\u{fc}sica/\u{e9}pica/", 9),
            "\u{2026}/\u{e9}pica/"
        );
        for budget in 0..60 {
            assert!(
                fit_folders(dir, budget).chars().count() <= budget,
                "{budget}"
            );
        }
    }

    #[test]
    fn the_room_for_folders_grows_with_the_window_and_shrinks_with_the_file_name() {
        assert_eq!(folder_budget(0., 5), 0);
        assert_eq!(
            folder_budget(400., 5),
            0,
            "a narrow window has no room for them"
        );
        assert!(folder_budget(1600., 5) > folder_budget(1200., 5));
        assert!(folder_budget(1600., 30) < folder_budget(1600., 5));
        assert_eq!(folder_budget(1600., 10_000), 0);
    }

    #[test]
    fn a_binary_file_is_told_by_its_size_and_not_by_lines() {
        assert_eq!(size_words(0), "0 B");
        assert_eq!(size_words(900), "900 B");
        assert_eq!(size_words(86_220), "84.2 KB");
        assert_eq!(size_words(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(
            binary_words(Some("84.2 KB")),
            "Binary file \u{2014} not shown \u{b7} 84.2 KB"
        );
        assert_eq!(binary_words(None), "Binary file \u{2014} not shown");
        let binary = Stat {
            binary: true,
            ..stat("public/audio/approve.mp3")
        };
        assert!(rows_of_stats(&[binary, stat("a.rs")])[0].binary);
        assert!(!rows_of_stats(&[stat("a.rs")])[0].binary);
    }

    #[test]
    fn a_file_is_looked_for_from_the_folder_up_to_the_root_of_the_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("crates/app")).unwrap();
        std::fs::write(root.join("README.md"), "x").unwrap();
        // Named from the root, in a conversation that works below it.
        assert_eq!(
            locate(&root.join("crates/app"), "README.md"),
            root.join("README.md")
        );
        // Not there: where it would be, from the folder.
        assert_eq!(
            locate(&root.join("crates/app"), "gone.md"),
            root.join("crates/app/gone.md")
        );
    }

    fn stat(path: &str) -> Stat {
        Stat {
            path: path.into(),
            added: 1,
            removed: 0,
            binary: false,
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
            note: None,
        };
        assert_eq!(panel(Source::Turn(3), false).scope(), Scope::Turn);
        assert_eq!(panel(Source::Turn(3), true).scope(), Scope::File);
        assert_eq!(panel(Source::All, false).scope(), Scope::All);
        assert_eq!(panel(Source::All, true).scope(), Scope::File);
    }
}

//! The changes of a worktree in a tab: the changed files, the diff of the
//! selected one, and the commit, push and pull request that take them away.
//!
//! The tab is a leaf like a file's or a terminal's (see [`super::editor`]):
//! the layout, the workspaces and the focus know nothing of the difference,
//! and what tells them apart is the side table `Shell::changes`, from the
//! leaf's [`LiveId`] to its [`ChangesView`].
//!
//! The window reads nothing itself. The engine reads the changed files and
//! the diffs and takes the steps, through the runner of the worktree's machine
//! (this computer, SSH or the relay), and the view keeps what came back. A
//! failure of git, `gh` or a hook is kept whole and drawn as it was printed.
//!
//! What can be decided without a window is a pure function with tests at the
//! end of this file: which files a commit takes, what the buttons allow, how
//! the pull request starts from a commit message, the paths the editor opens.

use std::collections::HashSet;

use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::{AppContext as _, Context, Entity, Subscription, UniformListScrollHandle, Window};
use leon_core::{AgentId, MachineId, ProjectId, WorktreeId};
use leon_remote::{ChangedFile, FileDiff, FileStatus, PullRequestDraft, Step, Suggest};

use super::live::LiveId;
use super::shell::{Main, Shell};
use super::tree;
use crate::engine::{CommitRequest, ShipReport, ShipRequest, StatusKind};

/// What a worktree's changes tab is for: where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The machine of the worktree.
    pub machine: MachineId,
    /// Its project.
    pub project: ProjectId,
    /// The worktree, when the store has one for the folder.
    pub worktree: Option<WorktreeId>,
    /// The worktree's folder.
    pub path: String,
}

/// The diff of the selected file, as far as it is known.
pub enum DiffState {
    /// No file is selected.
    None,
    /// Git is being asked.
    Loading,
    /// The diff.
    Ready(FileDiff),
    /// Git could not make it; the text is what it said.
    Failed(String),
}

/// What the last step came to, drawn under the buttons.
pub struct Outcome {
    /// Whether it went well.
    pub ok: bool,
    /// The first line: what was done or what failed.
    pub title: String,
    /// What the commands printed, whole.
    pub text: String,
    /// The pull request that was opened, to open in the browser.
    pub url: Option<String>,
}

/// The steps a button asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Steps {
    /// `git add` and `git commit`.
    Commit,
    /// `git push`.
    Push,
    /// `gh pr create`.
    PullRequest,
    /// All of them, each only when there is something for it to do.
    All,
}

/// One changes tab.
pub struct ChangesView {
    /// Where the worktree is.
    pub target: Target,
    /// The branch checked out, when the head is not detached.
    pub branch: Option<String>,
    /// Whether the branch has an upstream.
    pub has_upstream: bool,
    /// The changed files; `None` until git answered.
    pub files: Option<Vec<ChangedFile>>,
    /// Why the files could not be read, as git said it.
    pub failed: Option<String>,
    /// The paths the person took out of the commit; every file is in it
    /// until then, so a file that appears later is in it too.
    pub unchosen: HashSet<String>,
    /// The path of the selected file.
    pub selected: Option<String>,
    /// Its diff.
    pub diff: DiffState,
    /// The scroll of the diff.
    pub diff_scroll: UniformListScrollHandle,
    /// The scroll of the list of files.
    pub list_scroll: UniformListScrollHandle,
    /// The commit message.
    pub message: Entity<TextareaState>,
    /// The title of the pull request.
    pub title: Entity<InputState>,
    /// Its body.
    pub body: Entity<TextareaState>,
    /// The branch it goes into.
    pub base: Entity<InputState>,
    /// Whether it is opened as a draft.
    pub draft: bool,
    /// Whether the pull request's fields are showing.
    pub form: bool,
    /// What is being done, in a word, while it is.
    pub working: Option<&'static str>,
    /// Whether an agent is being asked.
    pub suggesting: bool,
    /// The agent asked to word things; the first that can, until chosen.
    pub agent: Option<AgentId>,
    /// What the last step came to.
    pub outcome: Option<Outcome>,
    /// The pull request's fields are to be filled from the commits once the
    /// reading in progress has ended.
    prefill: bool,
    /// How many readings of the pull request's start are under way.
    prefilling: u32,
    /// The whole chain was asked for before the tab knew its files and the
    /// start of the pull request: it runs when both have been read.
    ship_when_ready: bool,
    /// Counts reads, so an answer that is no longer the latest is dropped.
    reads: u64,
    /// Counts diff reads, for the same.
    diffs: u64,
    /// The subscriptions to the fields.
    _fields: Vec<Subscription>,
}

impl ChangesView {
    /// The files in the commit: every changed file less those taken out.
    pub fn chosen(&self) -> Vec<ChangedFile> {
        chosen_files(self.files.as_deref().unwrap_or_default(), &self.unchosen)
    }

    /// The selected file.
    pub fn selected_file(&self) -> Option<&ChangedFile> {
        let path = self.selected.as_deref()?;
        self.files.as_ref()?.iter().find(|file| file.path == path)
    }

    /// Whether the tab is still reading what the chain would start from: the
    /// files, and the title and base the pull request starts with.
    pub fn settling(&self) -> bool {
        self.files.is_none() || (self.prefill && self.failed.is_none()) || self.prefilling > 0
    }

    /// Whether any of the fields has the keyboard.
    pub fn typing(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        use gpui_kit::Focusable as _;
        self.message.focus_handle(cx).is_focused(window)
            || self.body.focus_handle(cx).is_focused(window)
            || self.title.focus_handle(cx).is_focused(window)
            || self.base.focus_handle(cx).is_focused(window)
    }
}

// ----- the pure parts -----------------------------------------------------------------

/// The files a commit takes: those not taken out.
pub fn chosen_files(files: &[ChangedFile], unchosen: &HashSet<String>) -> Vec<ChangedFile> {
    files
        .iter()
        .filter(|file| !unchosen.contains(&file.path))
        .cloned()
        .collect()
}

/// The files to hand to the commit: `None` for every changed file (so the
/// commit is `git add -A` and takes what appears meanwhile as well), the
/// chosen ones when some were taken out.
pub fn commit_files(files: &[ChangedFile], unchosen: &HashSet<String>) -> Option<Vec<ChangedFile>> {
    let chosen = chosen_files(files, unchosen);
    (chosen.len() != files.len()).then_some(chosen)
}

/// `3 of 5 files in the commit`, the line above the list.
pub fn chosen_words(total: usize, chosen: usize) -> String {
    let noun = if total == 1 { "file" } else { "files" };
    match total {
        0 => "No changes".to_owned(),
        _ if chosen == total => format!("{total} {noun}"),
        _ => format!("{chosen} of {total} {noun} in the commit"),
    }
}

/// The path of the selected file after the list was read again: the same file
/// if it is still there, else the first.
pub fn keep_selection(selected: Option<&str>, files: &[ChangedFile]) -> Option<String> {
    selected
        .filter(|path| files.iter().any(|file| file.path == *path))
        .or_else(|| files.first().map(|file| file.path.as_str()))
        .map(str::to_owned)
}

/// The path after moving the selection by `delta` rows, stopping at the ends.
pub fn move_selection(selected: Option<&str>, files: &[ChangedFile], delta: i64) -> Option<String> {
    if files.is_empty() {
        return None;
    }
    let at = selected
        .and_then(|path| files.iter().position(|file| file.path == path))
        .map_or(0, |at| at as i64);
    let to = (at + delta).clamp(0, files.len() as i64 - 1);
    Some(files[to as usize].path.clone())
}

/// Where a file of the worktree is: the worktree's folder, written the way
/// it is, and the path from its root.
pub fn file_path(root: &str, relative: &str) -> String {
    let separator = if root.contains('\\') && !root.contains('/') {
        '\\'
    } else {
        '/'
    };
    let relative = relative.replace('/', &separator.to_string());
    format!(
        "{}{separator}{relative}",
        root.trim_end_matches(['/', '\\'])
    )
}

/// The title and body a pull request starts with when the person did not
/// write them: the first line of the commit message and the rest.
pub fn pull_request_from_message(message: &str) -> (String, String) {
    let message = message.trim();
    let (title, body) = message.split_once('\n').unwrap_or((message, ""));
    (title.trim().to_owned(), body.trim().to_owned())
}

/// What the buttons allow, given what the view holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allowed {
    /// There is a message and a file to commit.
    pub commit: bool,
    /// There is a branch to push.
    pub push: bool,
    /// There is a branch, a title and a base.
    pub pull_request: bool,
    /// The whole chain can start: it can do at least its first step.
    pub all: bool,
}

/// The state of the fields that decides [`Allowed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fields<'a> {
    /// The commit message.
    pub message: &'a str,
    /// The pull request's title.
    pub title: &'a str,
    /// The branch it goes into.
    pub base: &'a str,
    /// How many files are in the commit.
    pub chosen: usize,
    /// How many files changed.
    pub changed: usize,
    /// Whether the head is on a branch.
    pub branch: bool,
    /// Whether something is being done already.
    pub busy: bool,
}

/// What the buttons allow. The chain commits only when files changed, so a
/// branch that is already committed can still be pushed and opened.
pub fn allowed(fields: Fields<'_>) -> Allowed {
    let message = !fields.message.trim().is_empty();
    let commit = !fields.busy && message && fields.chosen > 0;
    let push = !fields.busy && fields.branch;
    let pull_request = push && !fields.base.trim().is_empty() && {
        // The chain borrows the title from the message when there is none.
        !fields.title.trim().is_empty() || message
    };
    // A chain with changes needs them to be committable; one without needs
    // only a branch.
    let all = if fields.changed > 0 {
        commit && pull_request
    } else {
        pull_request
    };
    Allowed {
        commit,
        push,
        pull_request: push && !fields.base.trim().is_empty() && !fields.title.trim().is_empty(),
        all,
    }
}

/// Whether the whole chain can start now: the tab has its files and the start
/// of the pull request (its title and base) is read. A chain that starts
/// earlier would not know whether there is anything to commit, nor which
/// branch the pull request goes into.
pub fn chain_ready(files_read: bool, settling: bool) -> bool {
    files_read && !settling
}

/// The agents that can word a message on a machine, in the order offered.
pub fn headless_agents(installed: &[AgentId]) -> Vec<AgentId> {
    installed
        .iter()
        .copied()
        .filter(|agent| agent.spec().is_some_and(|spec| spec.headless.is_some()))
        .collect()
}

/// The next agent after `current` among `candidates`, wrapping.
pub fn next_agent(candidates: &[AgentId], current: Option<AgentId>) -> Option<AgentId> {
    let at = current.and_then(|agent| candidates.iter().position(|c| *c == agent));
    match at {
        Some(at) => candidates.get((at + 1) % candidates.len()).copied(),
        None => candidates.first().copied(),
    }
}

/// How a step is worded in the title of an outcome.
fn done_words(steps: &[(Step, String)]) -> String {
    let said: Vec<&str> = steps
        .iter()
        .map(|(step, _)| match step {
            Step::Commit => "committed",
            Step::Push => "pushed",
            Step::PullRequest => "opened a pull request",
            Step::Suggestion => "worded",
        })
        .collect();
    match said.split_last() {
        None => "Nothing was done".to_owned(),
        Some((only, [])) => capital(only),
        Some((last, rest)) => format!("{} and {last}", capital(&rest.join(", "))),
    }
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The outcome a report is drawn as.
pub fn outcome_of(report: &ShipReport) -> Outcome {
    let text = report
        .done
        .iter()
        .map(|(_, text)| text.as_str())
        .chain(report.failed.iter().map(|(_, text)| text.as_str()))
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    match &report.failed {
        Some((step, _)) => Outcome {
            ok: false,
            title: format!("{} failed", step.words()),
            text,
            url: None,
        },
        None => Outcome {
            ok: true,
            title: done_words(&report.done),
            text,
            url: report.url.clone(),
        },
    }
}

// ----- the shell ------------------------------------------------------------------------

impl Shell {
    /// The changes tab on screen, which has the keyboard when the main pane
    /// does.
    pub(in crate::ui) fn focused_changes(&self) -> Option<LiveId> {
        match self.main {
            Main::Live(id) if self.changes.contains_key(&id) => Some(id),
            _ => None,
        }
    }

    /// Whether a field of the changes tab on screen has the keyboard: what is
    /// typed there is text, not a shortcut.
    pub(in crate::ui) fn changes_typing(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.focused_changes()
            .and_then(|id| self.changes.get(&id))
            .is_some_and(|view| view.typing(window, cx))
    }

    /// The worktree the changes command is about: the changes tab on screen,
    /// else where the keyboard is (the worktree, project or terminal's
    /// folder).
    pub(in crate::ui) fn changes_target(&self) -> Option<Target> {
        if let Some(view) = self.focused_changes().and_then(|id| self.changes.get(&id)) {
            return Some(view.target.clone());
        }
        let place = self.here()?;
        let root = tree::workspace_root(&self.snapshot, &place.machine, &place.cwd);
        let (project, worktree) = tree::detail_of_root(&self.snapshot, &place.machine, &root)?;
        Some(Target {
            machine: place.machine,
            project,
            worktree,
            path: root,
        })
    }

    /// The changes command: the tab of the worktree in view, or the question
    /// of which worktree when none is.
    pub(in crate::ui) fn open_changes_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.changes_target() {
            Some(target) => {
                self.open_changes(target, window, cx);
            }
            None => self.begin_flow(crate::keys::Command::OpenChanges, window, cx),
        }
    }

    /// Opens the changes tab of a worktree, or shows the one it has.
    pub(in crate::ui) fn open_changes(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> LiveId {
        let folder = tree::workspace_root(&self.snapshot, &target.machine, &target.path);
        let found = self.changes.iter().find_map(|(id, view)| {
            (view.target.machine == target.machine
                && leon_core::path::key(&view.target.path) == leon_core::path::key(&target.path))
            .then_some(*id)
        });
        if let Some(id) = found {
            self.open_live(id, window, cx);
            self.changes_refresh(id, window, cx);
            return id;
        }
        let id = self.live.next_id();
        let view = self.new_changes_view(target, window, cx);
        let machine = view.target.machine.clone();
        self.changes.insert(id, view);
        self.workspaces
            .add_folder_tab(machine.as_str(), &folder, id);
        self.open_live(id, window, cx);
        if let Some(view) = self.changes.get_mut(&id) {
            view.prefill = true;
        }
        self.changes_refresh(id, window, cx);
        id
    }

    fn new_changes_view(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ChangesView {
        let message = cx.new(|cx| TextareaState::new(window, cx).placeholder("Commit message"));
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Pull request title"));
        let body =
            cx.new(|cx| TextareaState::new(window, cx).placeholder("What it changes and why"));
        let base = cx.new(|cx| InputState::new(window, cx).placeholder("main"));
        let candidates = headless_agents(&self.agents_for_buttons(&target.machine));
        let prefs = Shell::step_prefs(cx);
        let agent = prefs
            .default_agent
            .filter(|agent| candidates.contains(agent))
            .or_else(|| candidates.first().copied());
        ChangesView {
            target,
            branch: None,
            has_upstream: false,
            files: None,
            failed: None,
            unchosen: HashSet::new(),
            selected: None,
            diff: DiffState::None,
            diff_scroll: UniformListScrollHandle::new(),
            list_scroll: UniformListScrollHandle::new(),
            message,
            title,
            body,
            base,
            draft: false,
            form: false,
            working: None,
            suggesting: false,
            agent,
            outcome: None,
            prefill: false,
            prefilling: 0,
            ship_when_ready: false,
            reads: 0,
            diffs: 0,
            _fields: Vec::new(),
        }
    }

    /// Reads the changed files again, keeping the selection and the files
    /// taken out of the commit when they are still there.
    pub(in crate::ui) fn changes_refresh(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        view.reads += 1;
        let read = view.reads;
        let reading = self
            .engine
            .read_changes(view.target.machine.clone(), view.target.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let answer = reading.await;
            this.update_in(cx, |this, window, cx| {
                this.changes_read(id, read, answer, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn changes_read(
        &mut self,
        id: LiveId,
        read: u64,
        answer: Result<
            Result<leon_remote::Changes, crate::engine::EngineError>,
            tokio::task::JoinError,
        >,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        if read != view.reads {
            return;
        }
        match answer {
            Ok(Ok(changes)) => {
                view.failed = None;
                view.branch = changes.branch;
                view.has_upstream = changes.has_upstream;
                let now: HashSet<&str> = changes.files.iter().map(|f| f.path.as_str()).collect();
                view.unchosen.retain(|path| now.contains(path.as_str()));
                let selected = keep_selection(view.selected.as_deref(), &changes.files);
                view.selected = selected;
                view.files = Some(changes.files);
                // The same file may have changed since its diff was read.
                self.changes_read_diff(id, window, cx);
            }
            Ok(Err(error)) => {
                view.failed = Some(error.to_string());
                view.files = Some(Vec::new());
                view.selected = None;
                view.diff = DiffState::None;
                Self::chain_unread(view);
            }
            Err(error) => {
                view.failed = Some(error.to_string());
                Self::chain_unread(view);
            }
        }
        cx.notify();
    }

    /// The chain that was waiting for the files will not run: they could not
    /// be read, and the tab says why.
    fn chain_unread(view: &mut ChangesView) {
        if std::mem::take(&mut view.ship_when_ready) {
            view.outcome = Some(Outcome {
                ok: false,
                title: "The changes could not be read, so nothing was run.".to_owned(),
                text: String::new(),
                url: None,
            });
        }
    }

    /// Reads the diff of the selected file.
    fn changes_read_diff(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        let Some(file) = view.selected_file().cloned() else {
            view.diff = DiffState::None;
            // Nothing to read: the pull request can start from its commits.
            if std::mem::take(&mut view.prefill) {
                self.changes_prefill(id, window, cx);
            }
            return;
        };
        view.diffs += 1;
        let read = view.diffs;
        view.diff = DiffState::Loading;
        let reading = self.engine.read_diff(
            view.target.machine.clone(),
            view.target.path.clone(),
            file.clone(),
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = reading.await;
            this.update_in(cx, |this, window, cx| {
                let Some(view) = this.changes.get_mut(&id) else {
                    return;
                };
                if read != view.diffs {
                    return;
                }
                view.diff = match answer {
                    Ok(Ok(diff)) => DiffState::Ready(diff),
                    Ok(Err(error)) => DiffState::Failed(error.to_string()),
                    Err(error) => DiffState::Failed(error.to_string()),
                };
                view.diff_scroll
                    .scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
                // The first reading after opening goes on to the pull
                // request's start: one command at a time, in this order.
                if std::mem::take(&mut view.prefill) {
                    this.changes_prefill(id, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Selects a file and reads its diff.
    pub(in crate::ui) fn changes_select(
        &mut self,
        id: LiveId,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        if view.selected.as_deref() == Some(path.as_str()) {
            return;
        }
        view.selected = Some(path);
        self.changes_read_diff(id, window, cx);
        cx.notify();
    }

    /// Moves the selection of the changes tab on screen by `delta` files.
    pub(in crate::ui) fn changes_move(
        &mut self,
        delta: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.focused_changes() else {
            return;
        };
        let Some(view) = self.changes.get(&id) else {
            return;
        };
        let next = move_selection(
            view.selected.as_deref(),
            view.files.as_deref().unwrap_or_default(),
            delta,
        );
        if let Some(path) = next {
            if let Some(at) = view
                .files
                .as_ref()
                .and_then(|files| files.iter().position(|file| file.path == path))
            {
                view.list_scroll
                    .scroll_to_item(at, gpui_kit::ScrollStrategy::Nearest);
            }
            self.changes_select(id, path, window, cx);
        }
    }

    /// Takes a file out of the commit, or puts it back.
    pub(in crate::ui) fn changes_toggle(&mut self, id: LiveId, path: &str, cx: &mut Context<Self>) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        if !view.unchosen.remove(path) {
            view.unchosen.insert(path.to_owned());
        }
        cx.notify();
    }

    /// Puts every file in the commit, or, when all are in, takes them all out.
    pub(in crate::ui) fn changes_toggle_all(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        if view.unchosen.is_empty() {
            view.unchosen = view
                .files
                .iter()
                .flatten()
                .map(|file| file.path.clone())
                .collect();
        } else {
            view.unchosen.clear();
        }
        cx.notify();
    }

    /// Enter on a file: opens it in the editor, in a tab of its worktree.
    pub(in crate::ui) fn changes_open_selected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.focused_changes() else {
            return;
        };
        self.changes_open(id, None, window, cx);
    }

    /// Opens a file of the changes in the editor: `path` or the selected one.
    pub(in crate::ui) fn changes_open(
        &mut self,
        id: LiveId,
        path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.changes.get(&id) else {
            return;
        };
        let Some(relative) = path.or_else(|| view.selected.clone()) else {
            return;
        };
        let deleted = view
            .files
            .iter()
            .flatten()
            .any(|file| file.path == relative && file.status == FileStatus::Deleted);
        if deleted {
            self.engine.report(
                StatusKind::Info,
                format!("{relative} was deleted: there is no file to open."),
            );
            return;
        }
        let machine = view.target.machine.clone();
        let absolute = file_path(&view.target.path, &relative);
        self.open_file(machine, absolute, None, window, cx);
    }

    /// Fills the base branch (and the title and body, while they are empty)
    /// from what the worktree's commits say.
    pub(in crate::ui) fn changes_prefill(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        view.prefilling += 1;
        let starting = self
            .engine
            .pull_request_start(view.target.machine.clone(), view.target.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let start = starting.await;
            this.update_in(cx, |this, window, cx| {
                let Some(view) = this.changes.get_mut(&id) else {
                    return;
                };
                view.prefilling = view.prefilling.saturating_sub(1);
                if let Ok(Ok(start)) = start {
                    let empty = |text: gpui_kit::SharedString| text.trim().is_empty();
                    if empty(view.base.read(cx).value()) {
                        view.base.update(cx, |field, cx| {
                            field.set_value(start.base.clone(), window, cx)
                        });
                    }
                    if empty(view.title.read(cx).value()) && !start.title.is_empty() {
                        view.title.update(cx, |field, cx| {
                            field.set_value(start.title.clone(), window, cx)
                        });
                    }
                    if empty(view.body.read(cx).value()) && !start.body.is_empty() {
                        view.body.update(cx, |field, cx| {
                            field.set_value(start.body.clone(), window, cx)
                        });
                    }
                }
                // The chain that was waiting for this reading starts now, with
                // the pull request's title and base filled when git gave them.
                if view.ship_when_ready {
                    this.changes_ship_when_ready(id, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows or hides the pull request's fields.
    pub(in crate::ui) fn changes_toggle_form(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::Focusable as _;
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        view.form = !view.form;
        if view.form {
            let title = view.title.clone();
            window.focus(&title.focus_handle(cx), cx);
            self.changes_prefill(id, window, cx);
        }
        cx.notify();
    }

    /// Whether the pull request is a draft, flipped.
    pub(in crate::ui) fn changes_toggle_draft(&mut self, id: LiveId, cx: &mut Context<Self>) {
        if let Some(view) = self.changes.get_mut(&id) {
            view.draft = !view.draft;
            cx.notify();
        }
    }

    /// Asks the next agent that can word things to do it from now on.
    pub(in crate::ui) fn changes_next_agent(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(view) = self.changes.get(&id) else {
            return;
        };
        let candidates = headless_agents(&self.agents_for_buttons(&view.target.machine));
        let next = next_agent(&candidates, view.agent);
        if let Some(view) = self.changes.get_mut(&id) {
            view.agent = next;
            cx.notify();
        }
    }

    /// The fields of the view as the buttons judge them.
    pub(in crate::ui) fn changes_allowed(&self, id: LiveId, cx: &gpui_kit::App) -> Allowed {
        let Some(view) = self.changes.get(&id) else {
            return allowed(Fields {
                message: "",
                title: "",
                base: "",
                chosen: 0,
                changed: 0,
                branch: false,
                busy: true,
            });
        };
        let (message, title, base) = (
            view.message.read(cx).value(),
            view.title.read(cx).value(),
            view.base.read(cx).value(),
        );
        allowed(Fields {
            message: &message,
            title: &title,
            base: &base,
            chosen: view.chosen().len(),
            changed: view.files.as_ref().map_or(0, Vec::len),
            branch: view.branch.is_some(),
            busy: view.working.is_some(),
        })
    }

    /// Takes the steps a button asks for.
    pub(in crate::ui) fn changes_ship(
        &mut self,
        id: LiveId,
        steps: Steps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.changes.get(&id) else {
            return;
        };
        if view.working.is_some() {
            return;
        }
        let message = view.message.read(cx).value().trim().to_owned();
        // A commit chosen before the changes were read would not know what it
        // commits; a push or a pull request does not depend on them.
        if view.files.is_none() && matches!(steps, Steps::Commit | Steps::All) {
            if let Some(view) = self.changes.get_mut(&id) {
                view.outcome = Some(Outcome {
                    ok: false,
                    title: "The changes are still being read: try again in a moment.".to_owned(),
                    text: String::new(),
                    url: None,
                });
            }
            cx.notify();
            return;
        }
        let files = view.files.clone().unwrap_or_default();
        let wants_commit = match steps {
            Steps::Commit => true,
            Steps::All => !files.is_empty(),
            Steps::Push | Steps::PullRequest => false,
        };
        let refuse = |view: &mut ChangesView, why: &str| {
            view.outcome = Some(Outcome {
                ok: false,
                title: why.to_owned(),
                text: String::new(),
                url: None,
            });
        };
        if wants_commit && message.is_empty() {
            let message = view.message.clone();
            use gpui_kit::Focusable as _;
            window.focus(&message.focus_handle(cx), cx);
            if let Some(view) = self.changes.get_mut(&id) {
                refuse(view, "Write a commit message first.");
            }
            cx.notify();
            return;
        }
        let commit = wants_commit.then(|| CommitRequest {
            message: message.clone(),
            files: commit_files(&files, &view.unchosen),
        });
        if commit
            .as_ref()
            .is_some_and(|commit| commit.files.as_ref().is_some_and(Vec::is_empty))
        {
            if let Some(view) = self.changes.get_mut(&id) {
                refuse(view, "Choose at least one file to commit.");
            }
            cx.notify();
            return;
        }
        let pull_request = matches!(steps, Steps::PullRequest | Steps::All).then(|| {
            let typed = view.title.read(cx).value().trim().to_owned();
            let (title, body) = if typed.is_empty() {
                pull_request_from_message(&message)
            } else {
                (typed, view.body.read(cx).value().trim().to_owned())
            };
            PullRequestDraft {
                title,
                body,
                base: view.base.read(cx).value().trim().to_owned(),
                draft: view.draft,
            }
        });
        if pull_request
            .as_ref()
            .is_some_and(|draft| draft.title.is_empty() || draft.base.is_empty())
        {
            let form_shown = view.form;
            if let Some(view) = self.changes.get_mut(&id) {
                view.form = true;
                refuse(
                    view,
                    "Give the pull request a title and the branch it goes into.",
                );
            }
            let _ = form_shown;
            cx.notify();
            return;
        }
        let request = ShipRequest {
            machine: view.target.machine.clone(),
            project: view.target.project.clone(),
            path: view.target.path.clone(),
            commit,
            push: matches!(steps, Steps::Push | Steps::All),
            pull_request,
        };
        let working = match steps {
            Steps::Commit => "Committing\u{2026}",
            Steps::Push => "Pushing\u{2026}",
            Steps::PullRequest => "Opening the pull request\u{2026}",
            Steps::All => "Committing, pushing and opening the pull request\u{2026}",
        };
        if let Some(view) = self.changes.get_mut(&id) {
            view.working = Some(working);
            view.outcome = None;
        }
        let running = self.engine.ship(request);
        cx.spawn_in(window, async move |this, cx| {
            let report = running.await.unwrap_or_else(|error| ShipReport {
                failed: Some((Step::Commit, error.to_string())),
                ..ShipReport::default()
            });
            this.update_in(cx, |this, window, cx| {
                this.changes_shipped(id, report, window, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn changes_shipped(
        &mut self,
        id: LiveId,
        report: ShipReport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        view.working = None;
        view.outcome = Some(outcome_of(&report));
        if report.done.iter().any(|(step, _)| *step == Step::Commit) {
            // The message was used, and the next pull request starts from the
            // commits there are now.
            view.message
                .update(cx, |field, cx| field.set_value("", window, cx));
            view.title
                .update(cx, |field, cx| field.set_value("", window, cx));
            view.body
                .update(cx, |field, cx| field.set_value("", window, cx));
            view.prefill = true;
        }
        self.changes_refresh(id, window, cx);
        cx.notify();
    }

    /// Asks the agent to word the commit message (`Suggest::Commit`) or the
    /// pull request (`Suggest::PullRequest`). A failure is a plain message in
    /// the outcome; committing never waits for it.
    pub(in crate::ui) fn changes_suggest(
        &mut self,
        id: LiveId,
        what: Suggest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        if view.suggesting {
            return;
        }
        let Some(agent) = view.agent else {
            view.outcome = Some(Outcome {
                ok: false,
                title: "No agent on this machine can word a message from here.".to_owned(),
                text: "Claude Code and Codex can; the others have no non-interactive form Leon knows for certain.".to_owned(),
                url: None,
            });
            cx.notify();
            return;
        };
        view.suggesting = true;
        let asked_with = match what {
            Suggest::Commit => view.message.read(cx).value().to_string(),
            Suggest::PullRequest => view.title.read(cx).value().to_string(),
        };
        let files = match what {
            Suggest::Commit => {
                commit_files(view.files.as_deref().unwrap_or_default(), &view.unchosen)
                    .or_else(|| view.files.clone())
            }
            Suggest::PullRequest => None,
        };
        let asking = self.engine.suggest(
            view.target.machine.clone(),
            view.target.path.clone(),
            what,
            files,
            agent,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = asking.await;
            this.update_in(cx, |this, window, cx| {
                this.changes_suggested(id, what, asked_with, agent, answer, window, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn changes_suggested(
        &mut self,
        id: LiveId,
        what: Suggest,
        asked_with: String,
        agent: AgentId,
        answer: Result<Result<String, crate::engine::EngineError>, tokio::task::JoinError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        view.suggesting = false;
        let text = match answer {
            Ok(Ok(text)) => text,
            Ok(Err(error)) => {
                view.outcome = Some(Outcome {
                    ok: false,
                    title: format!("{} could not word it", agent.name()),
                    text: error.to_string(),
                    url: None,
                });
                cx.notify();
                return;
            }
            Err(error) => {
                view.outcome = Some(Outcome {
                    ok: false,
                    title: format!("{} could not word it", agent.name()),
                    text: error.to_string(),
                    url: None,
                });
                cx.notify();
                return;
            }
        };
        let (field_now, message) = match what {
            Suggest::Commit => (
                view.message.read(cx).value().to_string(),
                view.message.clone(),
            ),
            Suggest::PullRequest => (
                view.title.read(cx).value().to_string(),
                view.message.clone(),
            ),
        };
        // Words the person typed meanwhile are not thrown away.
        if field_now != asked_with {
            view.outcome = Some(Outcome {
                ok: false,
                title: "The suggestion was not used: you started writing.".to_owned(),
                text,
                url: None,
            });
            cx.notify();
            return;
        }
        match what {
            Suggest::Commit => {
                message.update(cx, |field, cx| field.set_value(text, window, cx));
            }
            Suggest::PullRequest => {
                let (title, body) = leon_remote::split_title_and_body(&text);
                view.title
                    .update(cx, |field, cx| field.set_value(title, window, cx));
                view.body
                    .update(cx, |field, cx| field.set_value(body, window, cx));
            }
        }
        view.outcome = None;
        cx.notify();
    }

    /// A key of the changes tab that is not a command: space puts the
    /// selected file in the commit or takes it out. `true` when it was taken.
    pub(in crate::ui) fn changes_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let held = &stroke.modifiers;
        if held.platform || held.control || held.alt || held.shift || held.function {
            return false;
        }
        let Some(id) = self.focused_changes() else {
            return false;
        };
        if stroke.key != "space" {
            return false;
        }
        match self.changes.get(&id).and_then(|view| view.selected.clone()) {
            Some(path) => {
                self.changes_toggle(id, &path, cx);
                true
            }
            None => false,
        }
    }

    /// Opens the pull request that was opened, in the browser.
    pub(in crate::ui) fn changes_open_url(&mut self, url: &str, cx: &mut Context<Self>) {
        (self.options.open_url)(cx, url);
    }

    /// Closes a changes tab. The pane's sibling takes the room; the last pane
    /// of the workspace returns the main pane to what the folder shows.
    pub(in crate::ui) fn close_changes(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.changes.remove(&id) else {
            return;
        };
        let was_open = matches!(self.main, Main::Live(open) if open == id);
        let closed = self.workspaces.close(id);
        let folder = tree::workspace_root(&self.snapshot, &view.target.machine, &view.target.path);
        self.after_close(closed, was_open, &view.target.machine, &folder, window, cx);
    }

    /// Reads the files of every changes tab again: the window came back, and
    /// whatever ran meanwhile may have changed them.
    pub(in crate::ui) fn changes_refresh_all(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<LiveId> = self.changes.keys().copied().collect();
        for id in ids {
            self.changes_refresh(id, window, cx);
        }
    }

    /// The worktree whose changes the file tree shows (the folder of the
    /// panel), when it is one.
    pub(in crate::ui) fn open_changes_of_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((machine, root)) = self.files_target() else {
            return;
        };
        match tree::detail_of_root(&self.snapshot, &machine, &root) {
            Some((project, worktree)) => {
                let target = Target {
                    machine,
                    project,
                    worktree,
                    path: root,
                };
                self.open_changes(target, window, cx);
            }
            None => self.engine.report(
                StatusKind::Info,
                "The folder of the file tree is not a worktree: there are no changes to show.",
            ),
        };
    }

    /// Pushes the branch of the worktree in view, without opening anything.
    pub(in crate::ui) fn push_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.changes_target() else {
            self.engine.report(
                StatusKind::Info,
                "Open a worktree first: there is nothing to push.",
            );
            return;
        };
        let id = self.open_changes(target, window, cx);
        self.changes_ship(id, Steps::Push, window, cx);
    }

    /// The whole chain for the worktree in view: its tab opens, and the chain
    /// starts once the changes and the start of the pull request have been
    /// read. It commits only when files changed (and then asks for the message
    /// if there is none), so a branch already committed is pushed and opened.
    pub(in crate::ui) fn ship_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.changes_target() else {
            self.engine.report(
                StatusKind::Info,
                "Open a worktree first: there is nothing to commit.",
            );
            return;
        };
        let id = self.open_changes(target, window, cx);
        self.changes_ship_when_ready(id, window, cx);
    }

    /// Starts the whole chain of a tab if it is ready, else leaves it to start
    /// when the reading in progress ends (see [`ChangesView::settling`]).
    fn changes_ship_when_ready(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.changes.get_mut(&id) else {
            return;
        };
        if !chain_ready(view.files.is_some(), view.settling()) {
            view.ship_when_ready = true;
            view.outcome = Some(Outcome {
                ok: true,
                title: "Reading the changes first: the steps start when that is done.".to_owned(),
                text: String::new(),
                url: None,
            });
            cx.notify();
            return;
        }
        view.ship_when_ready = false;
        self.changes_ship(id, Steps::All, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, status: FileStatus) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            from: None,
            status,
        }
    }

    fn files() -> Vec<ChangedFile> {
        vec![
            file("a.rs", FileStatus::Modified),
            file("b.rs", FileStatus::Added),
            file("c.rs", FileStatus::Untracked),
        ]
    }

    #[test]
    fn every_file_is_in_the_commit_until_one_is_taken_out() {
        let none = HashSet::new();
        assert_eq!(chosen_files(&files(), &none).len(), 3);
        assert_eq!(commit_files(&files(), &none), None, "all: `git add -A`");
        let out: HashSet<String> = ["b.rs".to_owned()].into();
        let chosen = commit_files(&files(), &out).expect("a subset");
        assert_eq!(
            chosen.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["a.rs", "c.rs"]
        );
        let all_out: HashSet<String> = files().into_iter().map(|f| f.path).collect();
        assert_eq!(commit_files(&files(), &all_out), Some(Vec::new()));
    }

    #[test]
    fn the_line_above_the_list_counts_the_files() {
        assert_eq!(chosen_words(0, 0), "No changes");
        assert_eq!(chosen_words(1, 1), "1 file");
        assert_eq!(chosen_words(5, 5), "5 files");
        assert_eq!(chosen_words(5, 3), "3 of 5 files in the commit");
    }

    #[test]
    fn the_selection_survives_a_new_reading_when_its_file_is_still_there() {
        let list = files();
        assert_eq!(keep_selection(Some("b.rs"), &list).as_deref(), Some("b.rs"));
        assert_eq!(
            keep_selection(Some("gone.rs"), &list).as_deref(),
            Some("a.rs")
        );
        assert_eq!(keep_selection(None, &list).as_deref(), Some("a.rs"));
        assert_eq!(keep_selection(Some("a.rs"), &[]), None);
    }

    #[test]
    fn the_selection_moves_by_rows_and_stops_at_the_ends() {
        let list = files();
        assert_eq!(
            move_selection(Some("a.rs"), &list, 1).as_deref(),
            Some("b.rs")
        );
        assert_eq!(
            move_selection(Some("a.rs"), &list, -1).as_deref(),
            Some("a.rs")
        );
        assert_eq!(
            move_selection(Some("c.rs"), &list, 1).as_deref(),
            Some("c.rs")
        );
        assert_eq!(
            move_selection(Some("a.rs"), &list, 99).as_deref(),
            Some("c.rs")
        );
        assert_eq!(move_selection(None, &list, 1).as_deref(), Some("b.rs"));
        assert_eq!(move_selection(None, &[], 1), None);
    }

    #[test]
    fn a_file_is_opened_where_the_worktree_spells_its_paths() {
        assert_eq!(file_path("/srv/api", "src/lib.rs"), "/srv/api/src/lib.rs");
        assert_eq!(file_path("/srv/api/", "a.rs"), "/srv/api/a.rs");
        assert_eq!(
            file_path(r"C:\work\api", "src/lib.rs"),
            r"C:\work\api\src\lib.rs"
        );
    }

    #[test]
    fn a_pull_request_starts_from_the_commit_message_when_nothing_was_written() {
        assert_eq!(
            pull_request_from_message("Fix the tabs\n\nThey leaked.\n"),
            ("Fix the tabs".to_owned(), "They leaked.".to_owned())
        );
        assert_eq!(
            pull_request_from_message("  Only a subject "),
            ("Only a subject".to_owned(), String::new())
        );
    }

    fn fields<'a>() -> Fields<'a> {
        Fields {
            message: "Fix",
            title: "Fix",
            base: "main",
            chosen: 2,
            changed: 2,
            branch: true,
            busy: false,
        }
    }

    #[test]
    fn the_buttons_allow_what_has_what_it_needs() {
        let all = allowed(fields());
        assert!(all.commit && all.push && all.pull_request && all.all);
        let no_message = allowed(Fields {
            message: " ",
            ..fields()
        });
        assert!(!no_message.commit);
        assert!(no_message.push && no_message.pull_request);
        assert!(!no_message.all, "changes to commit need a message");
        let none_chosen = allowed(Fields {
            chosen: 0,
            ..fields()
        });
        assert!(!none_chosen.commit && !none_chosen.all);
        let detached = allowed(Fields {
            branch: false,
            ..fields()
        });
        assert!(!detached.push && !detached.pull_request && !detached.all);
        let busy = allowed(Fields {
            busy: true,
            ..fields()
        });
        assert!(!busy.commit && !busy.push && !busy.pull_request && !busy.all);
        let no_base = allowed(Fields {
            base: "",
            ..fields()
        });
        assert!(!no_base.pull_request && !no_base.all);
    }

    #[test]
    fn a_branch_with_nothing_left_to_commit_can_still_be_pushed_and_opened() {
        let clean = allowed(Fields {
            message: "",
            changed: 0,
            chosen: 0,
            title: "From the commits",
            ..fields()
        });
        assert!(!clean.commit);
        assert!(clean.push && clean.pull_request && clean.all);
    }

    #[test]
    fn the_chain_borrows_the_title_from_the_message() {
        let borrowed = allowed(Fields {
            title: "",
            ..fields()
        });
        assert!(borrowed.all, "the title comes from the message");
        assert!(!borrowed.pull_request, "the form alone wants its own title");
    }

    #[test]
    fn only_the_agents_with_a_verified_headless_form_word_things() {
        let installed = [AgentId::OPENCODE, AgentId::CLAUDE, AgentId::CODEX];
        assert_eq!(
            headless_agents(&installed),
            [AgentId::CLAUDE, AgentId::CODEX]
        );
        assert!(headless_agents(&[AgentId::OPENCODE]).is_empty());
        let candidates = [AgentId::CLAUDE, AgentId::CODEX];
        assert_eq!(next_agent(&candidates, None), Some(AgentId::CLAUDE));
        assert_eq!(
            next_agent(&candidates, Some(AgentId::CLAUDE)),
            Some(AgentId::CODEX)
        );
        assert_eq!(
            next_agent(&candidates, Some(AgentId::CODEX)),
            Some(AgentId::CLAUDE)
        );
        assert_eq!(next_agent(&[], None), None);
    }

    #[test]
    fn a_report_is_drawn_with_what_was_printed_and_the_address_that_was_opened() {
        let report = ShipReport {
            done: vec![
                (Step::Commit, "[topic 1a2b3c4] Fix".to_owned()),
                (Step::Push, String::new()),
                (
                    Step::PullRequest,
                    "https://github.com/zavudev/leon/pull/41".to_owned(),
                ),
            ],
            url: Some("https://github.com/zavudev/leon/pull/41".to_owned()),
            failed: None,
        };
        let outcome = outcome_of(&report);
        assert!(outcome.ok);
        assert_eq!(outcome.title, "Committed, pushed and opened a pull request");
        assert!(outcome.text.contains("[topic 1a2b3c4] Fix"));
        assert_eq!(
            outcome.url.as_deref(),
            Some("https://github.com/zavudev/leon/pull/41")
        );
        let failed = outcome_of(&ShipReport {
            done: vec![(Step::Commit, "[topic 1a2b3c4] Fix".to_owned())],
            url: None,
            failed: Some((Step::Push, "! [rejected] topic -> topic".to_owned())),
        });
        assert!(!failed.ok);
        assert_eq!(failed.title, "The push failed");
        assert!(failed.text.ends_with("! [rejected] topic -> topic"));
        assert_eq!(failed.url, None);
    }
}

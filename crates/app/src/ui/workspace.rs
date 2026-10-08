//! Workspaces: the terminals of a session, grouped in tabs of split panes.
//!
//! A [`Workspace`] is one session: a unit of work in one folder of one
//! machine (the worktree it was started in, or the folder itself when no
//! project contains it). Several sessions may share a folder. Its tabs are
//! [`Tab`]s, each a layout of panes: the first is the session's own terminal
//! (the agent, or a shell), the others are shells beside it. [`Workspaces`]
//! is the whole set, found by the id of any terminal in it. It is pure data:
//! the shell draws it and turns keys into the methods here.

use super::live::LiveId;
use super::panes::{Axis, Tab};

/// The terminals of one session.
#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    /// The machine, the folder and the number of the session.
    pub key: String,
    /// Its tabs, in the order they were opened.
    pub tabs: Vec<Tab>,
    /// The tab on screen.
    pub active: usize,
}

impl Workspace {
    /// Whether the session is in `folder`, a [`key_of`] a folder of a machine
    /// (a key from before sessions were told apart is the folder's).
    pub fn in_folder(&self, folder: &str) -> bool {
        self.key == folder
            || number_of(&self.key).is_some()
                && self
                    .key
                    .rsplit_once('\u{1f}')
                    .is_some_and(|(of, _)| of == folder)
    }

    /// The terminal the session is known by: the first pane of its first tab.
    pub fn lead(&self) -> Option<LiveId> {
        self.tabs
            .first()
            .and_then(|tab| tab.layout.leaves().first().copied())
    }

    /// The terminal with the keyboard in the tab on screen.
    pub fn front(&self) -> Option<LiveId> {
        self.tabs.get(self.active).map(|tab| tab.focus)
    }

    /// Every terminal of the session.
    pub fn terminals(&self) -> Vec<LiveId> {
        self.tabs
            .iter()
            .flat_map(|tab| tab.layout.leaves())
            .collect()
    }
}

/// What closing a pane leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    /// The pane was not in any workspace.
    Unknown,
    /// Other panes remain in the workspace: this one has the focus now.
    Focus(LiveId),
    /// That was the last pane of the workspace, which is gone.
    Empty,
}

/// Every workspace.
#[derive(Default)]
pub struct Workspaces {
    items: Vec<Workspace>,
    /// The number the next session gets in its key.
    sequence: u64,
}

/// The key of the folder of a machine, which a session's key starts with
/// (and which was the whole key before sessions were told apart).
pub fn key_of(machine: &str, root: &str) -> String {
    format!("{machine}\u{1f}{root}")
}

/// The key of session `number` in a folder of a machine.
pub fn session_key(machine: &str, root: &str, number: u64) -> String {
    format!("{}\u{1f}{number}", key_of(machine, root))
}

/// The number a key names its session by; `None` for the keys of before
/// sessions were told apart, which name a folder only.
fn number_of(key: &str) -> Option<u64> {
    let (folder, number) = key.rsplit_once('\u{1f}')?;
    if !folder.contains('\u{1f}') {
        return None;
    }
    number.parse().ok()
}

impl Workspaces {
    /// Opens a new session, one tab with one pane, in a folder of a machine,
    /// and returns its key.
    pub fn add_session(&mut self, machine: &str, root: &str, id: LiveId) -> String {
        let key = self.next_key(machine, root);
        self.items.push(Workspace {
            key: key.clone(),
            tabs: vec![Tab::new(id)],
            active: 0,
        });
        key
    }

    /// Opens a new tab with one pane in the session holding `of`, and shows
    /// it. `false` when `of` is in no session.
    pub fn add_tab(&mut self, of: LiveId, id: LiveId) -> bool {
        let Some((w, _)) = self.locate(of) else {
            return false;
        };
        let workspace = &mut self.items[w];
        workspace.tabs.push(Tab::new(id));
        workspace.active = workspace.tabs.len() - 1;
        true
    }

    /// Opens a tab with one pane (a file) in the last session of a folder of
    /// a machine, and shows it; the folder gets a session of its own when it
    /// has none.
    pub fn add_folder_tab(&mut self, machine: &str, root: &str, id: LiveId) {
        let folder = key_of(machine, root);
        let found = self.items.iter().rposition(|w| w.in_folder(&folder));
        match found {
            Some(at) => {
                let workspace = &mut self.items[at];
                workspace.tabs.push(Tab::new(id));
                workspace.active = workspace.tabs.len() - 1;
            }
            None => {
                self.add_session(machine, root, id);
            }
        }
    }

    /// The next free key of a session in a folder.
    fn next_key(&mut self, machine: &str, root: &str) -> String {
        loop {
            let key = session_key(machine, root, self.sequence);
            self.sequence += 1;
            if !self.items.iter().any(|w| w.key == key) {
                return key;
            }
        }
    }

    /// The workspaces, in the order they were opened.
    pub fn all(&self) -> &[Workspace] {
        &self.items
    }

    /// Adds a restored session with `tabs`, showing tab `active` of them.
    /// A key from before sessions were told apart held a whole folder: each
    /// of its tabs (once a terminal with its own row) becomes a session.
    pub fn restore(&mut self, key: &str, tabs: Vec<Tab>, active: usize) {
        if tabs.is_empty() {
            return;
        }
        match number_of(key) {
            Some(number) => {
                self.sequence = self.sequence.max(number + 1);
                let key = if self.items.iter().any(|w| w.key == key) {
                    let folder = key.rsplit_once('\u{1f}').map_or(key, |(folder, _)| folder);
                    let (machine, root) = folder.split_once('\u{1f}').unwrap_or((folder, ""));
                    self.next_key(machine, root)
                } else {
                    key.to_owned()
                };
                let active = active.min(tabs.len() - 1);
                self.items.push(Workspace { key, tabs, active });
            }
            None => {
                let (machine, root) = key.split_once('\u{1f}').unwrap_or((key, ""));
                for tab in tabs {
                    let key = self.next_key(machine, root);
                    self.items.push(Workspace {
                        key,
                        tabs: vec![tab],
                        active: 0,
                    });
                }
            }
        }
    }

    /// The workspace and tab holding a terminal.
    pub fn locate(&self, id: LiveId) -> Option<(usize, usize)> {
        self.items.iter().enumerate().find_map(|(w, workspace)| {
            workspace
                .tabs
                .iter()
                .position(|tab| tab.layout.contains(id))
                .map(|t| (w, t))
        })
    }

    /// The workspace holding a terminal.
    pub fn workspace_of(&self, id: LiveId) -> Option<&Workspace> {
        self.locate(id).map(|(w, _)| &self.items[w])
    }

    /// The terminal a terminal's session is known by.
    pub fn lead_of(&self, id: LiveId) -> Option<LiveId> {
        self.workspace_of(id).and_then(Workspace::lead)
    }

    /// The terminal with the keyboard in the tab on screen of the session
    /// holding `id`.
    pub fn front_of(&self, id: LiveId) -> Option<LiveId> {
        self.workspace_of(id).and_then(Workspace::front)
    }

    /// Every terminal of the session holding `id`.
    pub fn terminals_of(&self, id: LiveId) -> Vec<LiveId> {
        self.workspace_of(id)
            .map_or_else(Vec::new, Workspace::terminals)
    }

    /// The tab holding a terminal.
    pub fn tab_of(&self, id: LiveId) -> Option<&Tab> {
        self.locate(id).map(|(w, t)| &self.items[w].tabs[t])
    }

    /// The tab holding a terminal, to change.
    pub fn tab_of_mut(&mut self, id: LiveId) -> Option<&mut Tab> {
        self.locate(id).map(|(w, t)| &mut self.items[w].tabs[t])
    }

    /// Splits the pane of `target`; the new pane gets the focus and its tab
    /// is shown.
    pub fn split(&mut self, target: LiveId, axis: Axis, new: LiveId) -> bool {
        let Some((w, t)) = self.locate(target) else {
            return false;
        };
        let tab = &mut self.items[w].tabs[t];
        tab.focus = target;
        tab.split(axis, new);
        self.items[w].active = t;
        true
    }

    /// Gives a terminal the focus in its tab and shows that tab.
    pub fn focus(&mut self, id: LiveId) -> bool {
        let Some((w, t)) = self.locate(id) else {
            return false;
        };
        let tab = &mut self.items[w].tabs[t];
        if tab.focus != id {
            tab.focus = id;
        }
        self.items[w].active = t;
        true
    }

    /// Closes a pane. An empty tab goes too, then the tab before it (or the
    /// one after, for the first) is shown; the last tab takes the workspace
    /// with it.
    pub fn close(&mut self, id: LiveId) -> Closed {
        let Some((w, t)) = self.locate(id) else {
            return Closed::Unknown;
        };
        let workspace = &mut self.items[w];
        if workspace.tabs[t].close(id) {
            workspace.active = t;
            return Closed::Focus(workspace.tabs[t].focus);
        }
        workspace.tabs.remove(t);
        if workspace.tabs.is_empty() {
            self.items.remove(w);
            return Closed::Empty;
        }
        workspace.active = t.saturating_sub(1).min(workspace.tabs.len() - 1);
        Closed::Focus(workspace.tabs[workspace.active].focus)
    }

    /// Shows the tab `delta` places from the one holding `id`, wrapping
    /// round; the pane that has its focus.
    pub fn step_tab(&mut self, id: LiveId, delta: isize) -> Option<LiveId> {
        let (w, t) = self.locate(id)?;
        let workspace = &mut self.items[w];
        let len = workspace.tabs.len() as isize;
        let next = (t as isize + delta).rem_euclid(len) as usize;
        workspace.active = next;
        Some(workspace.tabs[next].focus)
    }

    /// Shows tab `number` (from 1) of the workspace holding `id`; the pane
    /// that has its focus. `None` when there is no such tab.
    pub fn goto_tab(&mut self, id: LiveId, number: usize) -> Option<LiveId> {
        let (w, _) = self.locate(id)?;
        let workspace = &mut self.items[w];
        let at = number.checked_sub(1)?;
        let tab = workspace.tabs.get(at)?;
        let focus = tab.focus;
        workspace.active = at;
        Some(focus)
    }

    /// The position of the tab holding `id` and how many tabs there are.
    #[cfg(test)]
    pub fn tab_position(&self, id: LiveId) -> Option<(usize, usize)> {
        let (w, t) = self.locate(id)?;
        Some((t, self.items[w].tabs.len()))
    }

    /// How many workspaces there are.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: LiveId = LiveId(1);
    const B: LiveId = LiveId(2);
    const C: LiveId = LiveId(3);
    const D: LiveId = LiveId(4);

    /// A session of terminals `ids`: the first is its own, the others tabs.
    fn session(all: &mut Workspaces, ids: &[LiveId]) {
        all.add_session("m", "/api", ids[0]);
        for id in &ids[1..] {
            all.add_tab(ids[0], *id);
        }
    }

    #[test]
    fn sessions_of_one_folder_are_told_apart_and_tabs_stay_in_their_session() {
        let mut all = Workspaces::default();
        let first = all.add_session("m", "/api", A);
        let second = all.add_session("m", "/api", C);
        assert_ne!(first, second, "two sessions of one folder");
        assert!(all.add_tab(A, B));
        assert!(!all.add_tab(D, LiveId(9)), "no session holds D");
        assert_eq!(all.len(), 2, "a tab is not a session");
        assert_eq!(all.tab_position(A), Some((0, 2)));
        assert_eq!(all.tab_position(B), Some((1, 2)));
        assert_eq!(all.tab_position(C), Some((0, 1)));
        assert_eq!(
            all.workspace_of(B).unwrap().active,
            1,
            "the new tab is shown"
        );
        assert_eq!(all.workspace_of(C).unwrap().key, second);
    }

    #[test]
    fn a_session_is_known_by_its_first_terminal() {
        let mut all = Workspaces::default();
        session(&mut all, &[A, B]);
        all.split(B, Axis::Row, C);
        assert_eq!(all.lead_of(C), Some(A));
        assert_eq!(all.lead_of(A), Some(A));
        assert_eq!(all.front_of(A), Some(C), "the split pane has the keyboard");
        assert_eq!(all.terminals_of(B), [A, B, C]);
        assert_eq!(all.lead_of(D), None);
    }

    #[test]
    fn the_key_names_machine_folder_and_session() {
        assert_ne!(key_of("m1", "/a"), key_of("m2", "/a"));
        assert_ne!(key_of("m", "/a"), key_of("m", "/b"));
        assert_ne!(session_key("m", "/a", 0), session_key("m", "/a", 1));
        assert_eq!(number_of(&session_key("m", "/a", 7)), Some(7));
        assert_eq!(
            number_of(&key_of("m", "/a")),
            None,
            "a key of a folder only"
        );
    }

    #[test]
    fn restoring_keeps_sessions_and_gives_each_tab_of_an_old_folder_one() {
        let mut all = Workspaces::default();
        let key = session_key("m", "/api", 4);
        all.restore(&key, vec![Tab::new(A), Tab::new(B)], 1);
        assert_eq!(all.len(), 1);
        assert_eq!(all.workspace_of(A).unwrap().active, 1);
        // The same key again is another session, not a merge.
        all.restore(&key, vec![Tab::new(C)], 0);
        assert_eq!(all.len(), 2);
        assert_ne!(all.workspace_of(C).unwrap().key, key);
        // A key of before sessions were told apart: a session per tab.
        all.restore(
            &key_of("m", "/web"),
            vec![Tab::new(D), Tab::new(LiveId(5))],
            1,
        );
        assert_eq!(all.len(), 4);
        assert_eq!(all.tab_position(D), Some((0, 1)));
        assert_eq!(all.tab_position(LiveId(5)), Some((0, 1)));
        let keys: std::collections::HashSet<_> = all.all().iter().map(|w| &w.key).collect();
        assert_eq!(keys.len(), 4, "every key is its own");
        all.restore("k", Vec::new(), 0);
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn splitting_adds_a_pane_to_the_tab_of_its_parent_and_focuses_it() {
        let mut all = Workspaces::default();
        session(&mut all, &[A, B]);
        assert!(all.split(A, Axis::Row, C));
        assert_eq!(all.locate(C), Some((0, 0)), "in A's tab");
        assert_eq!(all.tab_of(C).unwrap().focus, C);
        assert_eq!(
            all.workspace_of(C).unwrap().active,
            0,
            "and that tab is shown"
        );
        assert!(!all.split(D, Axis::Row, LiveId(9)));
    }

    #[test]
    fn closing_a_pane_collapses_the_layout_and_keeps_the_tab() {
        let mut all = Workspaces::default();
        session(&mut all, &[A]);
        all.split(A, Axis::Row, B);
        assert_eq!(all.close(B), Closed::Focus(A));
        assert_eq!(all.tab_of(A).unwrap().layout.leaves(), [A]);
    }

    #[test]
    fn closing_the_last_pane_of_a_tab_shows_the_tab_before_and_the_last_tab_ends_the_workspace() {
        let mut all = Workspaces::default();
        session(&mut all, &[A, B, C]);
        assert_eq!(all.close(C), Closed::Focus(B));
        assert_eq!(all.workspace_of(B).unwrap().active, 1);
        assert_eq!(
            all.close(A),
            Closed::Focus(B),
            "the first tab goes: the next is shown"
        );
        assert_eq!(all.close(B), Closed::Empty);
        assert_eq!(all.len(), 0);
        assert_eq!(all.close(B), Closed::Unknown);
    }

    #[test]
    fn tabs_step_and_jump_by_number() {
        let mut all = Workspaces::default();
        session(&mut all, &[A, B, C]);
        assert_eq!(all.step_tab(C, 1), Some(A), "wraps");
        assert_eq!(all.step_tab(A, -1), Some(C));
        assert_eq!(all.goto_tab(A, 2), Some(B));
        assert_eq!(all.workspace_of(B).unwrap().active, 1);
        assert_eq!(all.goto_tab(A, 4), None);
        assert_eq!(all.goto_tab(A, 0), None);
    }

    #[test]
    fn focusing_a_terminal_shows_its_tab() {
        let mut all = Workspaces::default();
        session(&mut all, &[A, B]);
        all.split(A, Axis::Column, C);
        all.focus(B);
        assert_eq!(all.workspace_of(B).unwrap().active, 1);
        all.focus(A);
        assert_eq!(all.workspace_of(A).unwrap().active, 0);
        assert_eq!(all.tab_of(A).unwrap().focus, A);
        assert!(!all.focus(D));
    }
}

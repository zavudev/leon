//! Workspaces: the terminals of a worktree, grouped in tabs of split panes.
//!
//! A [`Workspace`] belongs to one folder of one machine (the worktree the
//! terminals were started in, or the folder itself when no project contains
//! it). Its tabs are [`Tab`]s, each a layout of panes. [`Workspaces`] is the
//! whole set, found by the id of any terminal in it. It is pure data: the
//! shell draws it and turns keys into the methods here.

use super::live::LiveId;
use super::panes::{Axis, Tab};

/// The terminals of one folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    /// The machine and folder it belongs to.
    pub key: String,
    /// Its tabs, in the order they were opened.
    pub tabs: Vec<Tab>,
    /// The tab on screen.
    pub active: usize,
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
}

/// The key of the workspace of a folder on a machine.
pub fn key_of(machine: &str, root: &str) -> String {
    format!("{machine}\u{1f}{root}")
}

impl Workspaces {
    /// Opens a new tab with one pane in the workspace `key`, which is made
    /// when it is the first, and shows it.
    pub fn add_tab(&mut self, key: &str, id: LiveId) {
        let workspace = match self.items.iter_mut().find(|w| w.key == key) {
            Some(workspace) => workspace,
            None => {
                self.items.push(Workspace {
                    key: key.to_owned(),
                    tabs: Vec::new(),
                    active: 0,
                });
                self.items.last_mut().expect("just pushed")
            }
        };
        workspace.tabs.push(Tab::new(id));
        workspace.active = workspace.tabs.len() - 1;
    }

    /// The workspaces, in the order they were opened.
    pub fn all(&self) -> &[Workspace] {
        &self.items
    }

    /// Adds restored tabs to the workspace `key` (made when it is the
    /// first) and shows tab `active` of them.
    pub fn restore(&mut self, key: &str, tabs: Vec<Tab>, active: usize) {
        if tabs.is_empty() {
            return;
        }
        let workspace = match self.items.iter_mut().position(|w| w.key == key) {
            Some(at) => &mut self.items[at],
            None => {
                self.items.push(Workspace {
                    key: key.to_owned(),
                    tabs: Vec::new(),
                    active: 0,
                });
                self.items.last_mut().expect("just pushed")
            }
        };
        let before = workspace.tabs.len();
        workspace.tabs.extend(tabs);
        workspace.active = (before + active).min(workspace.tabs.len() - 1);
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

    #[test]
    fn terminals_of_one_folder_share_a_workspace_as_tabs() {
        let mut all = Workspaces::default();
        all.add_tab("m\u{1f}/api", A);
        all.add_tab("m\u{1f}/api", B);
        all.add_tab("m\u{1f}/web", C);
        assert_eq!(all.len(), 2);
        assert_eq!(all.tab_position(A), Some((0, 2)));
        assert_eq!(all.tab_position(B), Some((1, 2)));
        assert_eq!(all.tab_position(C), Some((0, 1)));
        assert_eq!(
            all.workspace_of(B).unwrap().active,
            1,
            "the new tab is shown"
        );
    }

    #[test]
    fn the_key_names_machine_and_folder() {
        assert_ne!(key_of("m1", "/a"), key_of("m2", "/a"));
        assert_ne!(key_of("m", "/a"), key_of("m", "/b"));
    }

    #[test]
    fn splitting_adds_a_pane_to_the_tab_of_its_parent_and_focuses_it() {
        let mut all = Workspaces::default();
        all.add_tab("k", A);
        all.add_tab("k", B);
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
        all.add_tab("k", A);
        all.split(A, Axis::Row, B);
        assert_eq!(all.close(B), Closed::Focus(A));
        assert_eq!(all.tab_of(A).unwrap().layout.leaves(), [A]);
    }

    #[test]
    fn closing_the_last_pane_of_a_tab_shows_the_tab_before_and_the_last_tab_ends_the_workspace() {
        let mut all = Workspaces::default();
        all.add_tab("k", A);
        all.add_tab("k", B);
        all.add_tab("k", C);
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
        all.add_tab("k", A);
        all.add_tab("k", B);
        all.add_tab("k", C);
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
        all.add_tab("k", A);
        all.add_tab("k", B);
        all.split(A, Axis::Column, C);
        all.focus(B);
        assert_eq!(all.workspace_of(B).unwrap().active, 1);
        all.focus(A);
        assert_eq!(all.workspace_of(A).unwrap().active, 0);
        assert_eq!(all.tab_of(A).unwrap().focus, A);
        assert!(!all.focus(D));
    }
}

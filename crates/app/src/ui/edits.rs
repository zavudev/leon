//! What a conversation did to its worktree, and what the window keeps to show
//! it: the same for a chat of a pane and for a worker of King.
//!
//! The card at the end of a turn that changed files, the panel of diffs and
//! the git cell (`chat_diff.rs`, `chat_git.rs`) need the conversation's
//! items, the folder it works in and the state in [`Edits`]. A [`Subject`]
//! says whose; [`Shell::scene`] and [`Shell::edits`] find them, so the same
//! code serves both.

use std::collections::{HashMap, HashSet};

use leon_conversation::model::Item;
use leon_core::MachineId;
use leon_king::follow::Followed;

use super::chat_diff::DiffPanel;
use super::chat_git::GitCell;
use super::live::LiveId;
use super::shell::Shell;
use crate::chat::changed::Stat;
use crate::chat::rewind::{Mark, Memory};

/// Whose changes: a chat of a pane, or a worker of King.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Subject {
    /// A chat of a pane.
    Chat(LiveId),
    /// A worker of King, by its number.
    Worker(u32),
}

impl Subject {
    /// What its selectors end with: `3` for the chat 3, `w3` for the worker
    /// 3.
    pub fn tag(self) -> String {
        match self {
            Self::Chat(id) => id.0.to_string(),
            Self::Worker(id) => format!("w{id}"),
        }
    }
}

/// What the window keeps of what a conversation did to its worktree.
pub struct Edits {
    /// Where each of its turns began, and the snapshot taken before it.
    pub marks: Vec<Mark>,
    /// What each turn changed, as git counted it between the snapshots of
    /// the worktree, by the turn's number. A turn without an entry is
    /// counted from what its tools said.
    pub measured: HashMap<u32, Vec<Stat>>,
    /// The turns whose count is being read.
    pub counting: HashSet<u32>,
    /// The cards that list all their files instead of the first few, by the
    /// turn they end.
    pub expanded: HashSet<u32>,
    /// The panel of diffs beside it, when open.
    pub diff: Option<DiffPanel>,
    /// Counts the readings of the panel, so an answer that is no longer the
    /// latest is dropped.
    pub diff_reads: u64,
    /// Its branch and the steps that take its changes away.
    pub git: GitCell,
}

impl Edits {
    /// Nothing done yet, with `git` closed.
    pub fn new(git: GitCell) -> Self {
        Self {
            marks: Vec::new(),
            measured: HashMap::new(),
            counting: HashSet::new(),
            expanded: HashSet::new(),
            diff: None,
            diff_reads: 0,
            git,
        }
    }

    /// The marks of a conversation King kept the log of: its turns, as the
    /// log numbers them. What was counted of turns the log no longer has is
    /// forgotten.
    pub fn follow(&mut self, followed: &Followed, reset: bool) {
        self.marks = followed
            .turns
            .iter()
            .enumerate()
            .map(|(turn, began)| Mark {
                item: began.item,
                turn: turn as u32,
                checkpoint: began.checkpoint.clone(),
                memory: Memory::Unknown,
            })
            .collect();
        if reset {
            self.measured.clear();
        }
        let turns = self.marks.len() as u32;
        self.measured.retain(|turn, _| *turn < turns);
    }
}

/// Where a conversation works, and what it said.
pub(super) struct Scene<'a> {
    /// What was said and done.
    pub items: &'a [Item],
    /// The folder it works in.
    pub cwd: &'a str,
    /// The machine the folder is on.
    pub machine: MachineId,
    /// A turn is going on, or being made ready.
    pub going: bool,
}

impl Shell {
    /// The conversation of `subject`: its items, folder and machine. A
    /// worker on another machine has none: its worktree is not looked at
    /// from here.
    pub(super) fn scene(&self, subject: Subject) -> Option<Scene<'_>> {
        match subject {
            Subject::Chat(id) => {
                let chat = self.chats.get(&id)?;
                Some(Scene {
                    items: &chat.conversation.items,
                    cwd: &chat.cwd,
                    machine: chat.machine.clone(),
                    going: chat.conversation.running() || chat.launching,
                })
            }
            Subject::Worker(id) => {
                let status = self.king_ui.status.as_ref()?;
                let worker = status.workers.iter().find(|worker| worker.id == id)?;
                if worker.machine.is_some() {
                    return None;
                }
                let feed = self.king_ui.workers.get(&id)?;
                Some(Scene {
                    items: &feed.conversation().items,
                    cwd: &worker.cwd,
                    machine: MachineId::local(),
                    going: worker.conversation.busy,
                })
            }
        }
    }

    /// What was kept of what the conversation of `subject` did.
    pub(super) fn edits(&self, subject: Subject) -> Option<&Edits> {
        match subject {
            Subject::Chat(id) => self.chats.get(&id).map(|chat| &chat.edits),
            Subject::Worker(id) => self.king_ui.edits.get(&id),
        }
    }

    /// The same, to change.
    pub(super) fn edits_mut(&mut self, subject: Subject) -> Option<&mut Edits> {
        match subject {
            Subject::Chat(id) => self.chats.get_mut(&id).map(|chat| &mut chat.edits),
            Subject::Worker(id) => self.king_ui.edits.get_mut(&id),
        }
    }
}

impl Edits {
    /// The turns still to count against the worktree: for each that has a
    /// snapshot and no count yet, its number, the snapshot it began from and
    /// the one it ended at: the next turn's, or for the newest the worktree
    /// as it is (`None`), once it is over (`going` is false).
    pub fn to_count(&self, going: bool) -> Vec<(u32, String, Option<String>)> {
        let mut jobs = Vec::new();
        for (at, mark) in self.marks.iter().enumerate() {
            if self.measured.contains_key(&mark.turn) || self.counting.contains(&mark.turn) {
                continue;
            }
            let Some(from) = mark.checkpoint.clone() else {
                continue;
            };
            match self.marks.get(at + 1) {
                Some(next) => {
                    if let Some(to) = next.checkpoint.clone() {
                        jobs.push((mark.turn, from, Some(to)));
                    }
                }
                None if !going => jobs.push((mark.turn, from, None)),
                None => {}
            }
        }
        jobs
    }
}

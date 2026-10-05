//! Which nodes of the sidebar tree are open, remembered across restarts.
//!
//! Only what the person chose is stored: a node nobody ever opened or folded
//! has no entry and takes the default the tree gives it (see
//! [`tree`](super::tree)). The file is `tree.json`, next to the settings. A
//! missing or unreadable file means the defaults, and a write that fails is
//! logged: the tree keeps working for the session.
//!
//! "Show more" is not remembered: it only widens a list for as long as the
//! window is open.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The file's name, next to the settings file.
pub const FILE_NAME: &str = "tree.json";

/// The open and folded nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Expansion {
    /// What was chosen, by node key.
    open: HashMap<String, bool>,
    #[serde(skip)]
    wider: HashSet<String>,
    #[serde(skip)]
    everything: bool,
}

impl Expansion {
    /// Every node open and every list in full: for finding where a node is.
    pub fn everything() -> Self {
        Self {
            everything: true,
            ..Self::default()
        }
    }

    /// Reads the file, or the defaults when it is missing or unreadable.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "the tree file is unreadable; using the defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes the file whole, through a temporary one.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let draft = path.with_extension("json.tmp");
        std::fs::write(&draft, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&draft, path)
    }

    /// Whether the node is open: what was chosen, else `default`.
    pub fn is_open(&self, key: &str, default: bool) -> bool {
        self.everything || self.open.get(key).copied().unwrap_or(default)
    }

    /// Records the choice to open or fold a node. `true` when it changed what
    /// is stored.
    pub fn set_open(&mut self, key: &str, open: bool) -> bool {
        self.open.insert(key.to_owned(), open) != Some(open)
    }

    /// Lets the list under `parent` grow past its cap.
    pub fn show_all_under(&mut self, parent: &str) {
        self.wider.insert(parent.to_owned());
    }

    /// Whether the list under `parent` shows every session.
    pub fn shows_all_under(&self, parent: &str) -> bool {
        self.everything || self.wider.contains(parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_nobody_touched_takes_the_default() {
        let expansion = Expansion::default();
        assert!(expansion.is_open("project:a", true));
        assert!(!expansion.is_open("unsorted:local", false));
    }

    #[test]
    fn what_was_chosen_beats_the_default() {
        let mut expansion = Expansion::default();
        assert!(expansion.set_open("project:a", false));
        assert!(expansion.set_open("unsorted:local", true));
        assert!(!expansion.set_open("unsorted:local", true), "no change");
        assert!(!expansion.is_open("project:a", true));
        assert!(expansion.is_open("unsorted:local", false));
    }

    #[test]
    fn choices_survive_a_round_trip_through_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("deep").join(FILE_NAME);
        let mut expansion = Expansion::default();
        expansion.set_open("project:a", false);
        expansion.set_open("worktree:w", true);
        expansion.show_all_under("worktree:w");
        expansion.save(&file).unwrap();

        let loaded = Expansion::load(&file);
        assert!(!loaded.is_open("project:a", true));
        assert!(loaded.is_open("worktree:w", false));
        assert!(
            !loaded.shows_all_under("worktree:w"),
            "show more is for one session of the window"
        );
    }

    #[test]
    fn a_missing_or_unreadable_file_gives_the_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        assert_eq!(Expansion::load(&file), Expansion::default());
        std::fs::write(&file, b"{ nope").unwrap();
        assert_eq!(Expansion::load(&file), Expansion::default());
    }

    #[test]
    fn everything_is_open_and_in_full() {
        let mut expansion = Expansion::everything();
        expansion.set_open("project:a", false);
        assert!(expansion.is_open("project:a", false));
        assert!(expansion.shows_all_under("anything"));
    }
}

//! What the palette remembers between runs.
//!
//! Two small things, kept in `palette.json` next to the settings: how often
//! and how lately each command was run (so the commands used most float up),
//! and the places last gone to (shown when the palette opens with nothing
//! typed). A missing or unreadable file is an empty memory; a write that
//! fails is logged and forgotten.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The file's name, next to `settings.json`.
pub const FILE_NAME: &str = "palette.json";

/// How many places are remembered.
const RECENT: usize = 8;

/// A place the palette can go to, remembered by kind and id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// `machine`, `project`, `worktree` or `session`.
    pub kind: String,
    /// The id of the machine, project, worktree or session.
    pub id: String,
}

impl Target {
    /// A target of the given kind.
    pub fn new(kind: &str, id: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            id: id.into(),
        }
    }
}

/// The palette's memory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// By command name: how many runs, and the count of all runs when it was
    /// run last (a clock only the palette moves).
    used: HashMap<String, (u32, u64)>,
    runs: u64,
    recent: Vec<Target>,
}

impl Usage {
    /// Reads the memory from `path`; empty when there is none.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes the memory to `path`.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        std::fs::write(path, serde_json::to_vec(self)?)
    }

    /// A command was run.
    pub fn note_command(&mut self, name: &str) {
        self.runs += 1;
        let entry = self.used.entry(name.to_owned()).or_default();
        *entry = (entry.0.saturating_add(1), self.runs);
    }

    /// What having been used adds to a command's score: more for one used
    /// often, more for one used lately. Never enough to pass a better kind of
    /// match.
    pub fn boost(&self, name: &str) -> u32 {
        let Some((count, last)) = self.used.get(name) else {
            return 0;
        };
        let often = (*count).min(10) * 4;
        let lately = 40u32.saturating_sub(self.runs.saturating_sub(*last).min(40) as u32);
        often + lately
    }

    /// A place was gone to: it is the most recent one.
    pub fn note_target(&mut self, target: Target) {
        self.recent.retain(|known| known != &target);
        self.recent.insert(0, target);
        self.recent.truncate(RECENT);
    }

    /// The places gone to last, newest first.
    pub fn recent_targets(&self) -> &[Target] {
        &self.recent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_used_often_and_lately_outranks_one_never_used() {
        let mut usage = Usage::default();
        usage.note_command("AddMachine");
        usage.note_command("AddMachine");
        assert!(usage.boost("AddMachine") > usage.boost("RemoveWorktree"));
        assert_eq!(usage.boost("RemoveWorktree"), 0);
    }

    #[test]
    fn the_boost_fades_as_other_commands_are_run() {
        let mut usage = Usage::default();
        usage.note_command("A");
        let fresh = usage.boost("A");
        for _ in 0..60 {
            usage.note_command("B");
        }
        assert!(usage.boost("A") < fresh);
    }

    #[test]
    fn the_boost_stays_below_the_gap_between_kinds_of_match() {
        let mut usage = Usage::default();
        for _ in 0..50 {
            usage.note_command("A");
        }
        assert!(usage.boost("A") < 200, "{}", usage.boost("A"));
    }

    #[test]
    fn recent_places_are_newest_first_without_repeats_and_bounded() {
        let mut usage = Usage::default();
        for n in 0..12 {
            usage.note_target(Target::new("project", n.to_string()));
        }
        usage.note_target(Target::new("project", "5"));
        let recent = usage.recent_targets();
        assert_eq!(recent.len(), RECENT);
        assert_eq!(recent[0], Target::new("project", "5"));
        assert_eq!(recent.iter().filter(|t| t.id == "5").count(), 1);
    }

    #[test]
    fn the_memory_survives_a_round_trip_through_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("nested").join(FILE_NAME);
        let mut usage = Usage::default();
        usage.note_command("A");
        usage.note_target(Target::new("session", "s1"));
        usage.save(&file).unwrap();
        assert_eq!(Usage::load(&file), usage);
    }

    #[test]
    fn a_missing_or_broken_file_is_an_empty_memory() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            Usage::load(&directory.path().join("none.json")),
            Usage::default()
        );
        let broken = directory.path().join("broken.json");
        std::fs::write(&broken, b"{ not json").unwrap();
        assert_eq!(Usage::load(&broken), Usage::default());
    }
}

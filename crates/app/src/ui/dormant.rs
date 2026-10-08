//! Sleeping sessions that have no history row to stand for them.
//!
//! A session put to sleep (or whose last tab was closed) keeps its row in
//! the sidebar. An agent session that resumed history is that history row,
//! marked asleep. A plain shell, or an agent not (yet) in the history, has
//! none, so Leon keeps a [`Dormant`] record of it instead: the machine, the
//! folder, the agent and the name it had. The file is `dormant.json`, next to
//! the settings; a missing or unreadable file means none, and a write that
//! fails is logged. Waking one starts a new session where it was.

use super::live::LiveId;
use leon_core::{AgentId, MachineId};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The file's name, next to the settings file.
pub const FILE_NAME: &str = "dormant.json";

/// One sleeping session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dormant {
    /// Its number among the sleeping ones.
    pub number: u64,
    /// The machine it ran on.
    pub machine: String,
    /// The folder it ran in.
    pub cwd: String,
    /// The agent's tag, or `None` for a plain shell.
    pub agent: Option<String>,
    /// What its row says: the name or title it had.
    pub label: Option<String>,
    /// The id of the account it ran with; `None` is the agent's own setup.
    #[serde(default)]
    pub account: Option<String>,
}

impl Dormant {
    /// The id its row goes by: far above the ids of live terminals, which
    /// count up from 1.
    pub fn id(&self) -> LiveId {
        LiveId(u64::MAX - self.number)
    }

    /// The machine it ran on.
    pub fn machine(&self) -> MachineId {
        MachineId::from_string(&self.machine)
    }

    /// The agent it ran, when it ran one.
    pub fn agent(&self) -> Option<AgentId> {
        self.agent.as_deref().and_then(AgentId::parse)
    }
}

/// Every sleeping session without a history row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dormants {
    items: Vec<Dormant>,
}

impl Dormants {
    /// Reads the file, or none when it is missing or unreadable.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "the sleeping sessions file is unreadable; using none");
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

    /// The sleeping sessions, oldest first.
    pub fn all(&self) -> &[Dormant] {
        &self.items
    }

    /// Puts a session to sleep.
    pub fn add(
        &mut self,
        machine: &MachineId,
        cwd: &str,
        agent: Option<AgentId>,
        label: Option<String>,
        account: Option<String>,
    ) {
        let number = self.next_number();
        self.items.push(Dormant {
            number,
            machine: machine.as_str().to_owned(),
            cwd: cwd.to_owned(),
            agent: agent.map(|agent| agent.as_str().to_owned()),
            label,
            account,
        });
    }

    /// The number a session put to sleep now gets: above every one in use.
    fn next_number(&self) -> u64 {
        self.items.iter().map(|d| d.number + 1).max().unwrap_or(0)
    }

    /// Puts a record back (closing one was undone) with the number it had, so
    /// its row keeps its id and its place among the others; a number another
    /// record took meanwhile is not given twice. The id its row goes by.
    pub fn restore(&mut self, mut record: Dormant) -> LiveId {
        if self.items.iter().any(|d| d.number == record.number) {
            record.number = self.next_number();
        }
        let id = record.id();
        let at = self.items.partition_point(|d| d.number < record.number);
        self.items.insert(at, record);
        id
    }

    /// The sleeping session whose row goes by `id`.
    pub fn get(&self, id: LiveId) -> Option<&Dormant> {
        self.items.iter().find(|d| d.id() == id)
    }

    /// Takes a session out (woken or closed).
    pub fn remove(&mut self, id: LiveId) -> Option<Dormant> {
        let at = self.items.iter().position(|d| d.id() == id)?;
        Some(self.items.remove(at))
    }

    /// Takes out every session whose folder is `root` or lies inside it.
    /// `true` when any was.
    pub fn remove_within(&mut self, machine: &MachineId, root: &str) -> bool {
        let before = self.items.len();
        self.items
            .retain(|d| d.machine != machine.as_str() || !leon_core::path::is_within(&d.cwd, root));
        before != self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sleeping_session_keeps_where_it_ran_and_leaves_when_woken_or_closed() {
        let mut all = Dormants::default();
        let local = MachineId::local();
        all.add(&local, "/srv/api", None, Some("build".into()), None);
        all.add(
            &local,
            "/srv/web",
            Some(AgentId::CLAUDE),
            None,
            Some("claude-work".into()),
        );
        let first = all.all()[0].id();
        let second = all.all()[1].id();
        assert_ne!(first, second);
        assert_eq!(all.get(second).unwrap().agent(), Some(AgentId::CLAUDE));
        assert_eq!(all.get(first).unwrap().machine(), local);
        assert!(all.remove(first).is_some());
        assert!(all.remove(first).is_none());
        all.add(&local, "/srv/x", None, None, None);
        assert_ne!(all.all()[1].id(), second, "a number is not reused");
        assert!(all.remove_within(&local, "/srv/web"));
        assert_eq!(all.all().len(), 1);
    }

    #[test]
    fn a_record_put_back_keeps_its_number_and_its_place_unless_the_number_is_taken() {
        let mut all = Dormants::default();
        let local = MachineId::local();
        for cwd in ["/a", "/b", "/c"] {
            all.add(&local, cwd, None, None, None);
        }
        let middle = all.all()[1].clone();
        assert!(all.remove(middle.id()).is_some());
        assert_eq!(all.restore(middle.clone()), middle.id());
        let order: Vec<&str> = all.all().iter().map(|d| d.cwd.as_str()).collect();
        assert_eq!(order, ["/a", "/b", "/c"]);
        // Put back twice, the second one gets a number of its own.
        let again = all.restore(middle.clone());
        assert_ne!(again, middle.id());
        assert_eq!(all.all().len(), 4);
    }

    #[test]
    fn the_file_round_trips_and_a_bad_one_means_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut all = Dormants::default();
        all.add(&MachineId::local(), "/a", None, Some("x".into()), None);
        all.save(&path).unwrap();
        assert_eq!(Dormants::load(&path), all);
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(Dormants::load(&path), Dormants::default());
    }
}

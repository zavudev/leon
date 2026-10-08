//! What the sidebar shows, read from the store.
//!
//! [`Snapshot`] is one consistent read of everything the tree needs: every
//! machine, every project with its worktrees and the recent sessions of every
//! machine. It takes a handful of queries whatever the size of the history; grouping
//! the sessions under their worktrees is then done in memory, by
//! [`tree::Placement`](super::tree::Placement).

use leon_core::{
    Machine, MachineId, Project, ProjectIcon, ProjectId, Result, Session, SessionFilter, Store,
    Worktree, WorktreeId, WorktreeStatus,
};
use std::collections::HashMap;

/// How many recent sessions the tree holds, all machines together.
pub const SESSION_LIMIT: usize = 2_000;

/// A project and its worktrees, the main one first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
    /// The project.
    pub project: Project,
    /// Its worktrees.
    pub worktrees: Vec<Worktree>,
}

/// One consistent read of the store.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Every machine, the local one first.
    pub machines: Vec<Machine>,
    /// Every project of every machine, by name.
    pub projects: Vec<ProjectEntry>,
    /// The most recent sessions of every machine, newest first.
    pub sessions: Vec<Session>,
    /// The logo in effect of every project that has one (no image bytes).
    pub icons: HashMap<ProjectId, ProjectIcon>,
    /// The folders somebody removed (projects and worktrees), by machine.
    pub dismissed: Vec<(MachineId, String)>,
    /// What was last read about the checkout of each worktree; a worktree
    /// nothing was read about has no entry.
    pub statuses: HashMap<WorktreeId, WorktreeStatus>,
    /// What was decided about each project's agent memory, by the identity
    /// of its root (`leon_core::path::key`).
    pub memory: HashMap<String, leon_core::MemoryChoice>,
}

impl Snapshot {
    /// Reads the machines, the projects with their worktrees and the recent
    /// sessions and the removed folders.
    pub fn load(store: &Store) -> Result<Self> {
        let mut machines = store.machines()?;
        machines.sort_by_key(|machine| !machine.id.is_local());

        let mut worktrees: HashMap<ProjectId, Vec<Worktree>> = HashMap::new();
        for worktree in store.all_worktrees()? {
            worktrees
                .entry(worktree.project_id.clone())
                .or_default()
                .push(worktree);
        }
        let projects = store
            .projects(None)?
            .into_iter()
            .map(|project| ProjectEntry {
                worktrees: worktrees.remove(&project.id).unwrap_or_default(),
                project,
            })
            .collect();

        let sessions = store.recent_sessions(&SessionFilter::default(), SESSION_LIMIT)?;
        let icons = store.project_icons()?;
        let mut dismissed = Vec::new();
        for machine in &machines {
            for root in store.dismissed_roots(&machine.id)? {
                dismissed.push((machine.id.clone(), root));
            }
        }
        let statuses = store.worktree_statuses()?;
        let memory = store
            .memory_choices()?
            .into_iter()
            .map(|(root, choice)| (leon_core::path::key(&root), choice))
            .collect();
        Ok(Self {
            machines,
            projects,
            sessions,
            icons,
            dismissed,
            statuses,
            memory,
        })
    }

    /// The machine with this id.
    pub fn machine(&self, id: &MachineId) -> Option<&Machine> {
        self.machines.iter().find(|machine| &machine.id == id)
    }

    /// What is known about the checkout of a worktree.
    pub fn status(&self, worktree: &WorktreeId) -> Option<&WorktreeStatus> {
        self.statuses.get(worktree)
    }

    /// The project with this id.
    pub fn project(&self, id: &ProjectId) -> Option<&ProjectEntry> {
        self.projects.iter().find(|entry| &entry.project.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use leon_core::{AgentId, MachineKind, NewMessage, NewSession, NewWorktree, Role};

    fn session(store: &Store, machine: &MachineId, cwd: &str, title: &str, minute: u32) {
        let at = Utc.with_ymd_and_hms(2026, 10, 4, 10, minute, 0).unwrap();
        store
            .upsert_session(
                &NewSession {
                    agent: AgentId::CLAUDE,
                    external_id: format!("{title}-{minute}"),
                    machine_id: machine.clone(),
                    cwd: cwd.to_owned(),
                    title: title.to_owned(),
                    model: None,
                    started_at: at,
                    updated_at: at,
                },
                &[NewMessage {
                    role: Role::User,
                    text: title.to_owned(),
                    at,
                }],
            )
            .unwrap();
    }

    fn worktree(path: &str, branch: &str, is_main: bool) -> NewWorktree {
        NewWorktree {
            path: path.to_owned(),
            branch: Some(branch.to_owned()),
            head: Some("abcdef0123".to_owned()),
            is_main,
        }
    }

    fn ssh() -> MachineKind {
        MachineKind::Ssh {
            host: "box".into(),
            user: None,
            port: None,
            identity_file: None,
        }
    }

    #[test]
    fn the_local_machine_is_first() {
        let store = Store::open_in_memory().unwrap();
        store.add_machine("box", ssh()).unwrap();
        let snapshot = Snapshot::load(&store).unwrap();
        assert!(snapshot.machines[0].id.is_local());
        assert_eq!(snapshot.machines.len(), 2);
    }

    #[test]
    fn a_snapshot_holds_every_machines_projects_worktrees_and_sessions() {
        let store = Store::open_in_memory().unwrap();
        let other = store.add_machine("box", ssh()).unwrap();
        let api = store
            .add_project(&MachineId::local(), "api", "/srv/api")
            .unwrap();
        let web = store.add_project(&other.id, "web", "/srv/web").unwrap();
        store
            .replace_worktrees(
                &api.id,
                vec![
                    worktree("/srv/api", "main", true),
                    worktree("/srv/wt/x", "x", false),
                ],
            )
            .unwrap();
        store
            .replace_worktrees(&web.id, vec![worktree("/srv/web", "main", true)])
            .unwrap();
        session(&store, &MachineId::local(), "/srv/api", "local one", 1);
        session(&store, &other.id, "/srv/web", "remote one", 2);

        let snapshot = Snapshot::load(&store).unwrap();
        assert_eq!(snapshot.projects.len(), 2);
        assert_eq!(snapshot.project(&api.id).unwrap().worktrees.len(), 2);
        assert_eq!(snapshot.project(&web.id).unwrap().worktrees.len(), 1);
        assert_eq!(snapshot.sessions.len(), 2);
        assert_eq!(snapshot.sessions[0].title, "remote one", "newest first");
        assert_eq!(snapshot.machine(&other.id).unwrap().name, "box");
    }

    #[test]
    fn a_project_without_worktrees_is_still_listed() {
        let store = Store::open_in_memory().unwrap();
        let api = store
            .add_project(&MachineId::local(), "api", "/srv/api")
            .unwrap();
        let snapshot = Snapshot::load(&store).unwrap();
        assert!(snapshot.project(&api.id).unwrap().worktrees.is_empty());
    }
}

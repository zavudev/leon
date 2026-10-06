//! Project and worktree persistence, and the link between sessions and
//! projects.
//!
//! A session belongs to the project whose root, or one of whose worktrees,
//! contains the session's working directory. That link is derived data: it is
//! recomputed here whenever the set of projects or worktrees of a machine
//! changes, and by the history code whenever a session is written, so it never
//! has to be maintained by callers.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use super::Store;
use crate::change::StoreChange;
use crate::error::{Result, StoreError};
use crate::ids::{MachineId, ProjectId, SessionId, WorktreeId};
use crate::model::{NewWorktree, Project, Session, SessionScope, Worktree};

impl Store {
    /// Projects in the order of the sidebar, optionally restricted to one
    /// machine: the dragged order first, the name to tell apart what was never
    /// dragged.
    pub fn projects(&self, machine_id: Option<&MachineId>) -> Result<Vec<Project>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT id, machine_id, name, root FROM project
                 WHERE ?1 IS NULL OR machine_id = ?1
                 ORDER BY sort_order, name COLLATE NOCASE, id",
            )?;
            let projects = statement
                .query_map([machine_id.map(MachineId::as_str)], project_from_row)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(projects)
        })
    }

    /// The project with the given id.
    pub fn project(&self, id: &ProjectId) -> Result<Project> {
        self.read(|connection| find_project(connection, id))
    }

    /// Registers a project rooted at `root` on a machine. Trailing path
    /// separators are dropped, and a machine cannot hold two projects with the
    /// same root. Sessions already imported from inside the root are linked to
    /// the new project. The project goes last in the sidebar order of its
    /// machine.
    pub fn add_project(&self, machine_id: &MachineId, name: &str, root: &str) -> Result<Project> {
        let root = trim_trailing_separators(root);
        if root.is_empty() {
            return Err(StoreError::Invalid("a project needs a root path".into()));
        }
        let project = Project {
            id: ProjectId::generate(),
            machine_id: machine_id.clone(),
            name: name.to_owned(),
            root: root.to_owned(),
        };
        let relinked = self.transact(|tx| {
            let machine_exists = tx
                .prepare_cached("SELECT 1 FROM machine WHERE id = ?1")?
                .exists([machine_id.as_str()])?;
            if !machine_exists {
                return Err(StoreError::NotFound("machine"));
            }
            // The same folder under another spelling is the same project.
            let wanted = crate::path::key(&project.root);
            let existing: Vec<String> = tx
                .prepare_cached("SELECT root FROM project WHERE machine_id = ?1")?
                .query_map([machine_id.as_str()], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            if existing
                .iter()
                .any(|known| crate::path::key(known) == wanted)
            {
                return Err(StoreError::Invalid(format!(
                    "a project rooted at {root} already exists on this machine"
                )));
            }
            let next: i64 = tx
                .prepare_cached("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM project WHERE machine_id = ?1")?
                .query_row([machine_id.as_str()], |row| row.get(0))?;
            let inserted = tx
                .prepare_cached(
                    "INSERT INTO project (id, machine_id, name, root, sort_order) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (machine_id, root) DO NOTHING",
                )?
                .execute(params![
                    project.id.as_str(),
                    project.machine_id.as_str(),
                    project.name,
                    project.root,
                    next
                ])?;
            if inserted == 0 {
                return Err(StoreError::Invalid(format!(
                    "a project rooted at {root} already exists on this machine"
                )));
            }
            forget_dismissed(tx, machine_id, &project.root)?;
            relink_sessions(tx, machine_id)
        })?;
        self.notify(StoreChange::Projects);
        if relinked > 0 {
            self.notify(StoreChange::Sessions);
        }
        Ok(project)
    }

    /// Changes the display name of a project. An empty name is refused.
    pub fn rename_project(&self, id: &ProjectId, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(StoreError::Invalid("a project needs a name".into()));
        }
        self.write(StoreChange::Projects, |tx| {
            let updated = tx
                .prepare_cached("UPDATE project SET name = ?2 WHERE id = ?1")?
                .execute(params![id.as_str(), name])?;
            if updated == 0 {
                return Err(StoreError::NotFound("project"));
            }
            Ok(())
        })
    }

    /// Writes the sidebar order of one machine's projects: `ordered` holds
    /// every project of the machine, first row first. Anything else is
    /// refused, so a filtered view cannot drop what it does not show.
    pub fn reorder_projects(&self, machine_id: &MachineId, ordered: &[ProjectId]) -> Result<()> {
        self.write(StoreChange::Projects, |tx| {
            let current: Vec<String> = tx
                .prepare_cached(
                    "SELECT id FROM project WHERE machine_id = ?1 ORDER BY sort_order, name COLLATE NOCASE, id",
                )?
                .query_map([machine_id.as_str()], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut wanted: Vec<String> =
                ordered.iter().map(ProjectId::as_str).map(str::to_owned).collect();
            let mut current_sorted = current.clone();
            current_sorted.sort();
            wanted.sort();
            if current_sorted != wanted {
                return Err(StoreError::Invalid(
                    "the order must hold every project of the machine exactly once".into(),
                ));
            }
            for (position, id) in ordered.iter().enumerate() {
                tx.prepare_cached("UPDATE project SET sort_order = ?2 WHERE id = ?1")?
                    .execute(params![id.as_str(), position as i64])?;
            }
            Ok(())
        })
    }

    /// Moves one project one step in the sidebar order of its machine:
    /// `delta` is -1 (up) or 1 (down). At the ends nothing changes.
    /// Returns the new position, or `None` when the project is unknown.
    pub fn move_project(&self, id: &ProjectId, delta: isize) -> Result<Option<usize>> {
        let project = self.project(id)?;
        let all = self.projects(Some(&project.machine_id))?;
        let Some(at) = all.iter().position(|candidate| &candidate.id == id) else {
            return Ok(None);
        };
        let last = all.len().saturating_sub(1);
        let next = at.saturating_add_signed(delta).min(last);
        if next == at {
            return Ok(Some(at));
        }
        let mut ordered: Vec<ProjectId> = all.into_iter().map(|project| project.id).collect();
        let moved = ordered.remove(at);
        ordered.insert(next, moved);
        self.reorder_projects(&project.machine_id, &ordered)?;
        Ok(Some(next))
    }

    /// Removes a project and its worktrees. Its sessions are kept and become
    /// unlinked, or move to another project that also contains them. The root
    /// is remembered as dismissed (see [`Store::dismissed_roots`]).
    pub fn remove_project(&self, id: &ProjectId) -> Result<()> {
        self.transact(|tx| {
            let project = find_project(tx, id)?;
            tx.prepare_cached("DELETE FROM project WHERE id = ?1")?
                .execute([id.as_str()])?;
            tx.prepare_cached(
                "INSERT OR IGNORE INTO dismissed_root (machine_id, root) VALUES (?1, ?2)",
            )?
            .execute(params![project.machine_id.as_str(), project.root])?;
            relink_sessions(tx, &project.machine_id)?;
            Ok(())
        })?;
        self.notify(StoreChange::Projects);
        self.notify(StoreChange::Worktrees);
        self.notify(StoreChange::Sessions);
        Ok(())
    }

    /// The worktrees of a project, in the order of the sidebar: the dragged
    /// order first, the main worktree and then the path to tell apart what
    /// was never dragged.
    pub fn worktrees(&self, project_id: &ProjectId) -> Result<Vec<Worktree>> {
        self.read(|connection| load_worktrees(connection, project_id))
    }

    /// Every worktree of every project, in one read: grouped by project, in
    /// the order of the sidebar. Lets a view build a whole tree without
    /// asking once per project.
    pub fn all_worktrees(&self) -> Result<Vec<Worktree>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT id, project_id, path, branch, head, is_main,
                        merged_pull_request
                 FROM worktree
                 ORDER BY project_id, sort_order, is_main DESC, path",
            )?;
            let worktrees = statement
                .query_map([], worktree_from_row)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(worktrees)
        })
    }

    /// The roots of the projects somebody removed from a machine. Automatic
    /// discovery must not bring them back; adding one by hand does.
    pub fn dismissed_roots(&self, machine_id: &MachineId) -> Result<HashSet<String>> {
        self.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT root FROM dismissed_root WHERE machine_id = ?1")?;
            let roots = statement
                .query_map([machine_id.as_str()], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(roots)
        })
    }

    /// Forgets that a project root was removed, so that discovery may adopt
    /// it again. `true` when it was dismissed.
    pub fn restore_dismissed_root(&self, machine_id: &MachineId, root: &str) -> Result<bool> {
        let removed = self.transact(|tx| forget_dismissed(tx, machine_id, root))?;
        if removed > 0 {
            self.notify(StoreChange::Projects);
        }
        Ok(removed > 0)
    }

    /// Makes the stored worktrees of a project match what git reports.
    ///
    /// Worktrees are matched by path: one that is still present keeps its id,
    /// its place in the sidebar order and has its branch and head refreshed,
    /// a new one goes last and one that is no longer reported is removed.
    /// When a path is listed more than once the last entry wins. Returns the
    /// resulting worktrees.
    pub fn replace_worktrees(
        &self,
        project_id: &ProjectId,
        reported: Vec<NewWorktree>,
    ) -> Result<Vec<Worktree>> {
        // A path listed twice counts once, as its last entry.
        let mut seen = HashSet::new();
        let reported: Vec<&NewWorktree> = reported
            .iter()
            .rev()
            .filter(|worktree| seen.insert(trim_trailing_separators(&worktree.path).to_owned()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let (worktrees, relinked) = self.transact(|tx| {
            let project = find_project(tx, project_id)?;
            // Matched by path identity: a worktree git now spells with `/` is
            // the one stored with `\`, and keeps its id.
            let mut stale: HashMap<String, String> = HashMap::new();
            let mut by_key: HashMap<String, String> = HashMap::new();
            for (path, id) in tx
                .prepare_cached("SELECT path, id FROM worktree WHERE project_id = ?1")?
                .query_map([project_id.as_str()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            {
                by_key.insert(crate::path::key(&path), id.clone());
                stale.insert(id.clone(), id);
            }
            let next: i64 = tx
                .prepare_cached(
                    "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM worktree WHERE project_id = ?1",
                )?
                .query_row([project_id.as_str()], |row| row.get(0))?;

            // The paths not stored yet go last, main worktrees before linked
            // ones, then by path: the order the sidebar shows until somebody
            // drags one.
            let mut added: Vec<&&NewWorktree> = reported
                .iter()
                .filter(|worktree| {
                    !by_key.contains_key(&crate::path::key(trim_trailing_separators(
                        &worktree.path
                    )))
                })
                .collect();
            added.sort_by(|a, b| {
                b.is_main
                    .cmp(&a.is_main)
                    .then_with(|| a.path.cmp(&b.path))
            });
            let mut added_order: HashMap<String, i64> = HashMap::new();
            for (offset, worktree) in added.into_iter().enumerate() {
                added_order.insert(
                    crate::path::key(trim_trailing_separators(&worktree.path)),
                    next + offset as i64,
                );
            }

            for worktree in &reported {
                let path = trim_trailing_separators(&worktree.path);
                let key = crate::path::key(path);
                match by_key.get(&key) {
                    Some(id) => {
                        stale.remove(id);
                        // A spelling that is already another row's is left to
                        // that row (the unique index); the path is refreshed.
                        let _ = tx
                            .prepare_cached(
                                "UPDATE OR IGNORE worktree
                                 SET path = ?2, branch = ?3, head = ?4, is_main = ?5
                                 WHERE id = ?1",
                            )?
                            .execute(params![
                                id,
                                path,
                                worktree.branch,
                                worktree.head,
                                worktree.is_main
                            ])?;
                    }
                    None => {
                        let slot = added_order
                            .remove(&key)
                            .expect("a new path has its slot");
                        let id = WorktreeId::generate();
                        tx.prepare_cached(
                            "INSERT INTO worktree (id, project_id, path, branch, head, is_main, sort_order)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                             ON CONFLICT (project_id, path) DO UPDATE
                             SET branch = excluded.branch, head = excluded.head,
                                 is_main = excluded.is_main",
                        )?
                        .execute(params![
                            id.as_str(),
                            project_id.as_str(),
                            path,
                            worktree.branch,
                            worktree.head,
                            worktree.is_main,
                            slot
                        ])?;
                        by_key.insert(key, id.as_str().to_owned());
                    }
                }
            }
            for id in stale.values() {
                tx.prepare_cached("DELETE FROM worktree WHERE id = ?1")?
                    .execute([id])?;
            }

            let relinked = relink_sessions(tx, &project.machine_id)?;
            Ok((load_worktrees(tx, project_id)?, relinked))
        })?;
        self.notify(StoreChange::Worktrees);
        if relinked > 0 {
            self.notify(StoreChange::Sessions);
        }
        Ok(worktrees)
    }

    /// Writes the sidebar order of one project's worktrees: `ordered` holds
    /// every worktree of the project, first row first. Anything else is
    /// refused.
    pub fn reorder_worktrees(&self, project_id: &ProjectId, ordered: &[WorktreeId]) -> Result<()> {
        self.write(StoreChange::Worktrees, |tx| {
            let current: Vec<String> = tx
                .prepare_cached(
                    "SELECT id FROM worktree WHERE project_id = ?1
                     ORDER BY sort_order, is_main DESC, path, id",
                )?
                .query_map([project_id.as_str()], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut wanted: Vec<String> = ordered
                .iter()
                .map(WorktreeId::as_str)
                .map(str::to_owned)
                .collect();
            let mut current_sorted = current.clone();
            current_sorted.sort();
            wanted.sort();
            if current_sorted != wanted {
                return Err(StoreError::Invalid(
                    "the order must hold every worktree of the project exactly once".into(),
                ));
            }
            for (position, id) in ordered.iter().enumerate() {
                tx.prepare_cached("UPDATE worktree SET sort_order = ?2 WHERE id = ?1")?
                    .execute(params![id.as_str(), position as i64])?;
            }
            Ok(())
        })
    }

    /// What GitHub says about a worktree's pull request being merged. Writes
    /// nothing when the row already says this, so a probe that runs on a timer
    /// does not rewrite the database (nor wake every listener) on every pass.
    ///
    /// Returns whether the row changed.
    pub fn set_merged(&self, id: &WorktreeId, merged: Option<bool>) -> Result<bool> {
        self.write(StoreChange::Worktrees, |tx| {
            let current = find_worktree(tx, id)?;
            if current.merged_pull_request == merged {
                return Ok(false);
            }
            tx.prepare_cached("UPDATE worktree SET merged_pull_request = ?2 WHERE id = ?1")?
                .execute(params![id.as_str(), merged])?;
            Ok(true)
        })
    }

    /// Pins `pinned` sessions, in this order, on top of their parent's list;
    /// every other session of that parent goes back to automatic (by
    /// recency). Every pinned session must belong to `parent`.
    pub fn pin_sessions(&self, parent: &SessionScope, pinned: &[SessionId]) -> Result<()> {
        self.write(StoreChange::Sessions, |tx| {
            for (position, id) in pinned.iter().enumerate() {
                let session = find_session(tx, id)?;
                if !scope_holds(tx, parent, &session)? {
                    return Err(StoreError::Invalid(
                        "a pinned session must belong to its parent".into(),
                    ));
                }
                tx.prepare_cached("UPDATE session SET sort_order = ?2 WHERE id = ?1")?
                    .execute(params![id.as_str(), position as i64])?;
            }
            // Unpin every other session of the parent.
            let all: Vec<Session> = tx
                .prepare_cached(&format!(
                    "SELECT {} FROM session s",
                    super::history::SESSION_COLUMNS
                ))?
                .query_map([], |row| super::history::session_from_row(row, 0))?
                .collect::<rusqlite::Result<_>>()?;
            let pinned_set: HashSet<&str> = pinned.iter().map(SessionId::as_str).collect();
            for session in &all {
                if pinned_set.contains(session.id.as_str()) {
                    continue;
                }
                if scope_holds(tx, parent, session)? {
                    tx.prepare_cached("UPDATE session SET sort_order = NULL WHERE id = ?1")?
                        .execute([session.id.as_str()])?;
                }
            }
            Ok(())
        })
    }
}

/// Whether `session` belongs to `parent`: the same grouping the sidebar
/// tree uses (see `Placement`), so pinning a row pins what it shows.
fn scope_holds(connection: &Connection, parent: &SessionScope, session: &Session) -> Result<bool> {
    match parent {
        SessionScope::Worktree(id) => {
            let worktree = find_worktree(connection, id)?;
            Ok(session.project_id.as_ref() == Some(&worktree.project_id)
                && crate::path::is_within(&session.cwd, &worktree.path))
        }
        SessionScope::Project(id) => {
            let project = find_project(connection, id)?;
            if session.project_id.as_ref() != Some(&project.id)
                || session.machine_id != project.machine_id
            {
                return Ok(false);
            }
            // Directly under the project: inside its root but inside none of
            // its worktrees.
            if !crate::path::is_within(&session.cwd, &project.root) {
                return Ok(false);
            }
            let worktrees = load_worktrees(connection, &project.id)?;
            Ok(!worktrees
                .iter()
                .any(|worktree| crate::path::is_within(&session.cwd, &worktree.path)))
        }
        SessionScope::Folder(machine, cwd) => {
            Ok(session.machine_id == *machine && session.cwd == *cwd)
        }
    }
}

fn find_project(connection: &Connection, id: &ProjectId) -> Result<Project> {
    connection
        .prepare_cached("SELECT id, machine_id, name, root FROM project WHERE id = ?1")?
        .query_row([id.as_str()], project_from_row)
        .optional()?
        .ok_or(StoreError::NotFound("project"))
}

fn find_worktree(connection: &Connection, id: &WorktreeId) -> Result<Worktree> {
    connection
        .prepare_cached(
            "SELECT id, project_id, path, branch, head, is_main,
                    merged_pull_request
             FROM worktree WHERE id = ?1",
        )?
        .query_row([id.as_str()], worktree_from_row)
        .optional()?
        .ok_or(StoreError::NotFound("worktree"))
}

fn find_session(connection: &Connection, id: &SessionId) -> Result<Session> {
    connection
        .prepare_cached(&format!(
            "SELECT {} FROM session s WHERE s.id = ?1",
            super::history::SESSION_COLUMNS
        ))?
        .query_row([id.as_str()], |row| {
            super::history::session_from_row(row, 0)
        })
        .optional()?
        .ok_or(StoreError::NotFound("session"))
}

fn load_worktrees(connection: &Connection, project_id: &ProjectId) -> Result<Vec<Worktree>> {
    let mut statement = connection.prepare_cached(
        "SELECT id, project_id, path, branch, head, is_main,
                merged_pull_request
         FROM worktree
         WHERE project_id = ?1
         ORDER BY sort_order, is_main DESC, path",
    )?;
    let worktrees = statement
        .query_map([project_id.as_str()], worktree_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(worktrees)
}

fn worktree_from_row(row: &Row<'_>) -> rusqlite::Result<Worktree> {
    Ok(Worktree {
        id: WorktreeId::from_string(row.get::<_, String>(0)?),
        project_id: ProjectId::from_string(row.get::<_, String>(1)?),
        path: row.get(2)?,
        branch: row.get(3)?,
        head: row.get(4)?,
        is_main: row.get(5)?,
        merged_pull_request: row.get(6)?,
    })
}

fn project_from_row(row: &Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: ProjectId::from_string(row.get::<_, String>(0)?),
        machine_id: MachineId::from_string(row.get::<_, String>(1)?),
        name: row.get(2)?,
        root: row.get(3)?,
    })
}

/// Directories on one machine that belong to a project: every project root
/// and every worktree path, each with the project that owns it.
pub(crate) type ProjectRoots = Vec<(String, ProjectId)>;

/// Loads the [`ProjectRoots`] of a machine.
pub(crate) fn project_roots(
    connection: &Connection,
    machine_id: &MachineId,
) -> Result<ProjectRoots> {
    let mut statement = connection.prepare_cached(
        "SELECT root, id FROM project WHERE machine_id = ?1
         UNION ALL
         SELECT w.path, w.project_id FROM worktree w
         JOIN project p ON p.id = w.project_id
         WHERE p.machine_id = ?1",
    )?;
    let roots = statement
        .query_map([machine_id.as_str()], |row| {
            Ok((
                row.get(0)?,
                ProjectId::from_string(row.get::<_, String>(1)?),
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(roots)
}

/// The project owning the deepest directory that contains `cwd`, if any.
pub(crate) fn owning_project(roots: &ProjectRoots, cwd: &str) -> Option<ProjectId> {
    // Compared by identity (`path::key`): `C:/code/api` and `c:\code\api\src`
    // are the same folder and one inside the other.
    let cwd = crate::path::key(cwd);
    roots
        .iter()
        .map(|(root, id)| (crate::path::key(root), id))
        .filter(|(root, _)| crate::path::within_keys(&cwd, root))
        .max_by_key(|(root, _)| root.len())
        .map(|(_, project_id)| (*project_id).clone())
}

/// Recomputes the project of every session on a machine. Returns how many
/// sessions changed.
pub(crate) fn relink_sessions(tx: &Transaction<'_>, machine_id: &MachineId) -> Result<usize> {
    let roots = project_roots(tx, machine_id)?;
    let directories: Vec<String> = tx
        .prepare_cached("SELECT DISTINCT cwd FROM session WHERE machine_id = ?1")?
        .query_map([machine_id.as_str()], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut changed = 0;
    let mut update = tx.prepare_cached(
        "UPDATE session SET project_id = ?1
         WHERE machine_id = ?2 AND cwd = ?3 AND project_id IS NOT ?1",
    )?;
    for cwd in directories {
        let owner = owning_project(&roots, &cwd);
        changed += update.execute(params![
            owner.as_ref().map(ProjectId::as_str),
            machine_id.as_str(),
            cwd
        ])?;
    }
    Ok(changed)
}

#[cfg(test)]
use crate::path::is_within;

/// Forgets a dismissed root under any spelling. Returns how many rows went.
fn forget_dismissed(tx: &Transaction<'_>, machine_id: &MachineId, root: &str) -> Result<usize> {
    let wanted = crate::path::key(root);
    let rows: Vec<String> = tx
        .prepare_cached("SELECT root FROM dismissed_root WHERE machine_id = ?1")?
        .query_map([machine_id.as_str()], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut removed = 0;
    for row in rows
        .into_iter()
        .filter(|row| crate::path::key(row) == wanted)
    {
        removed += tx
            .prepare_cached("DELETE FROM dismissed_root WHERE machine_id = ?1 AND root = ?2")?
            .execute(params![machine_id.as_str(), row])?;
    }
    Ok(removed)
}

/// The migration of version 6: projects and worktrees that differ only by how
/// their path is spelled are merged (the oldest by id order keeps its id; the
/// worktrees of the others move to it), the sessions are linked again, and
/// the dismissed roots that name one folder are one. Idempotent: on a database
/// with nothing to merge it changes nothing.
pub(crate) fn heal_path_spellings(tx: &Transaction<'_>) -> Result<()> {
    let projects: Vec<(String, String, String)> = tx
        .prepare_cached("SELECT id, machine_id, root FROM project ORDER BY machine_id, id")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut keepers: HashMap<(String, String), String> = HashMap::new();
    for (id, machine, root) in &projects {
        let key = (machine.clone(), crate::path::key(root));
        match keepers.get(&key) {
            None => {
                keepers.insert(key, id.clone());
            }
            Some(keeper) => {
                // Move what the duplicate holds, then drop it.
                let rows: Vec<(String, String)> = tx
                    .prepare_cached("SELECT id, path FROM worktree WHERE project_id = ?1")?
                    .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                let held: Vec<String> = tx
                    .prepare_cached("SELECT path FROM worktree WHERE project_id = ?1")?
                    .query_map([keeper], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let held: std::collections::HashSet<String> =
                    held.iter().map(|path| crate::path::key(path)).collect();
                for (worktree, path) in rows {
                    if !held.contains(&crate::path::key(&path)) {
                        tx.prepare_cached(
                            "UPDATE OR IGNORE worktree SET project_id = ?1 WHERE id = ?2",
                        )?
                        .execute(params![keeper, worktree])?;
                    }
                }
                tx.prepare_cached("DELETE FROM project WHERE id = ?1")?
                    .execute([id])?;
            }
        }
    }
    // Worktrees of one project that are one folder.
    let worktrees: Vec<(String, String, String)> = tx
        .prepare_cached(
            "SELECT id, project_id, path FROM worktree ORDER BY project_id, is_main DESC, id",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut seen = std::collections::HashSet::new();
    for (id, project, path) in worktrees {
        if !seen.insert((project, crate::path::key(&path))) {
            tx.prepare_cached("DELETE FROM worktree WHERE id = ?1")?
                .execute([id])?;
        }
    }
    // Dismissed roots that are one folder.
    let dismissed: Vec<(String, String)> = tx
        .prepare_cached("SELECT machine_id, root FROM dismissed_root ORDER BY machine_id, root")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut seen = std::collections::HashSet::new();
    for (machine, root) in dismissed {
        if !seen.insert((machine.clone(), crate::path::key(&root))) {
            tx.prepare_cached("DELETE FROM dismissed_root WHERE machine_id = ?1 AND root = ?2")?
                .execute(params![machine, root])?;
        }
    }
    // Every session, linked again.
    let machines: Vec<String> = tx
        .prepare_cached("SELECT id FROM machine")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for machine in machines {
        relink_sessions(tx, &MachineId::from_string(machine))?;
    }
    Ok(())
}

/// Drops trailing separators so that `/srv/api/` and `/srv/api` are the same
/// path, while keeping a bare root such as `/` or `C:\` intact.
fn trim_trailing_separators(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.ends_with(':') {
        // "/" or "C:\": keep the first separator after the trimmed part.
        let keep = trimmed.len()
            + path[trimmed.len()..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
        &path[..keep]
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentId, NewSession};
    use chrono::DateTime;

    fn local() -> MachineId {
        MachineId::local()
    }

    fn worktree(path: &str, branch: Option<&str>, is_main: bool) -> NewWorktree {
        NewWorktree {
            path: path.into(),
            branch: branch.map(str::to_owned),
            head: Some("0123abc".into()),
            is_main,
        }
    }

    #[test]
    fn what_a_worktree_says_about_being_merged_is_kept_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leon.db");
        let wanted = {
            let store = Store::open(&path).unwrap();
            let project = store.add_project(&local(), "api", "/srv/api").unwrap();
            store
                .replace_worktrees(
                    &project.id,
                    vec![
                        worktree("/srv/api", Some("main"), true),
                        worktree("/srv/api-worktrees/feature", Some("feature/login"), false),
                    ],
                )
                .unwrap();
            let worktree = store
                .worktrees(&project.id)
                .unwrap()
                .into_iter()
                .find(|worktree| !worktree.is_main)
                .unwrap();
            // A new worktree is "not known" until something asks.
            assert_eq!(worktree.merged_pull_request, None);
            assert!(store.set_merged(&worktree.id, Some(true)).unwrap());
            worktree.id
        };
        // A second run of the application reads what the first one wrote.
        let store = Store::open(&path).unwrap();
        let stored = store
            .all_worktrees()
            .unwrap()
            .into_iter()
            .find(|worktree| worktree.id == wanted)
            .unwrap();
        assert_eq!(stored.merged_pull_request, Some(true));
    }

    #[test]
    fn a_worktree_that_cannot_change_again_is_not_written_again() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        store
            .replace_worktrees(&project.id, vec![worktree("/srv/api", Some("main"), true)])
            .unwrap();
        let worktree = store.worktrees(&project.id).unwrap().remove(0);
        let merged = Some(false);
        assert!(store.set_merged(&worktree.id, merged).unwrap());
        assert!(
            !store.set_merged(&worktree.id, merged).unwrap(),
            "the same answer writes nothing"
        );
        assert_eq!(
            store
                .worktrees(&project.id)
                .unwrap()
                .remove(0)
                .merged_pull_request,
            merged
        );
    }

    #[test]
    fn a_worktree_that_cannot_be_found_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let refused = store.set_merged(&WorktreeId::from_string("nope"), Some(true));
        assert!(
            matches!(refused, Err(StoreError::NotFound(_))),
            "{refused:?}"
        );
    }

    fn session_in(cwd: &str, external_id: &str) -> NewSession {
        NewSession {
            agent: AgentId::CLAUDE,
            external_id: external_id.into(),
            machine_id: local(),
            cwd: cwd.into(),
            title: "work".into(),
            model: None,
            started_at: DateTime::UNIX_EPOCH,
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn an_added_project_is_read_back() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        assert_eq!(store.project(&project.id).unwrap(), project);
        assert_eq!(store.projects(Some(&local())).unwrap(), vec![project]);
    }

    #[test]
    fn a_trailing_separator_in_the_root_is_dropped() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api/").unwrap();
        assert_eq!(project.root, "/srv/api");
    }

    #[test]
    fn bare_roots_keep_their_separator() {
        assert_eq!(trim_trailing_separators("/"), "/");
        assert_eq!(trim_trailing_separators("C:\\"), "C:\\");
        assert_eq!(trim_trailing_separators("C:\\code\\"), "C:\\code");
        assert_eq!(trim_trailing_separators(""), "");
    }

    #[test]
    fn two_projects_cannot_share_a_root_on_one_machine() {
        let store = Store::open_in_memory().unwrap();
        store.add_project(&local(), "api", "/srv/api").unwrap();
        assert!(matches!(
            store.add_project(&local(), "again", "/srv/api/"),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn a_project_on_an_unknown_machine_is_refused() {
        let store = Store::open_in_memory().unwrap();
        assert!(matches!(
            store.add_project(&MachineId::generate(), "api", "/srv/api"),
            Err(StoreError::NotFound("machine"))
        ));
    }

    #[test]
    fn projects_can_be_filtered_by_machine() {
        let store = Store::open_in_memory().unwrap();
        let remote = store
            .add_machine(
                "box",
                crate::MachineKind::Ssh {
                    host: "box.example".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        store.add_project(&local(), "here", "/srv/here").unwrap();
        let there = store
            .add_project(&remote.id, "there", "/srv/there")
            .unwrap();
        assert_eq!(store.projects(None).unwrap().len(), 2);
        assert_eq!(store.projects(Some(&remote.id)).unwrap(), vec![there]);
    }

    #[test]
    fn renaming_a_project_changes_only_its_name() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        store.rename_project(&project.id, "backend").unwrap();
        let renamed = store.project(&project.id).unwrap();
        assert_eq!(renamed.name, "backend");
        assert_eq!(renamed.root, "/srv/api");
    }

    #[test]
    fn renaming_a_project_refuses_an_empty_name() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        assert!(matches!(
            store.rename_project(&project.id, "  "),
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(store.project(&project.id).unwrap().name, "api");
    }

    #[test]
    fn reordering_worktrees_changes_only_their_order() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        store
            .replace_worktrees(
                &project.id,
                vec![
                    worktree("/srv/api", Some("main"), true),
                    worktree("/srv/wt/a", Some("a"), false),
                    worktree("/srv/wt/b", Some("b"), false),
                ],
            )
            .unwrap();
        let ids: Vec<WorktreeId> = store
            .worktrees(&project.id)
            .unwrap()
            .into_iter()
            .map(|worktree| worktree.id)
            .collect();
        let mut reversed = ids;
        reversed.reverse();
        store.reorder_worktrees(&project.id, &reversed).unwrap();
        let paths: Vec<String> = store
            .worktrees(&project.id)
            .unwrap()
            .into_iter()
            .map(|worktree| worktree.path)
            .collect();
        assert_eq!(paths, ["/srv/wt/b", "/srv/wt/a", "/srv/api"]);
        assert!(
            matches!(
                store.reorder_worktrees(&project.id, &reversed[..2]),
                Err(StoreError::Invalid(_))
            ),
            "a partial order is refused"
        );
    }

    fn session_at(cwd: &str, external_id: &str, seconds: i64) -> NewSession {
        let at = DateTime::from_timestamp(seconds, 0).unwrap();
        let mut session = session_in(cwd, external_id);
        session.started_at = at;
        session.updated_at = at;
        session
    }

    #[test]
    fn pinning_sessions_puts_them_first_and_unpin_restores_recency() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        store
            .replace_worktrees(&project.id, vec![worktree("/srv/api", Some("main"), true)])
            .unwrap();
        let worktree = store.worktrees(&project.id).unwrap().remove(0);
        let scope = SessionScope::Worktree(worktree.id.clone());
        let s1 = store
            .upsert_session(&session_at("/srv/api", "s1", 100), &[])
            .unwrap();
        let s2 = store
            .upsert_session(&session_at("/srv/api", "s2", 200), &[])
            .unwrap();
        let s3 = store
            .upsert_session(&session_at("/srv/api", "s3", 300), &[])
            .unwrap();

        store
            .pin_sessions(&scope, std::slice::from_ref(&s1))
            .unwrap();
        assert_eq!(store.session(&s1).unwrap().sort_order, Some(0));
        assert_eq!(store.session(&s2).unwrap().sort_order, None);

        // A new pin order replaces the old one; the rest go automatic.
        store
            .pin_sessions(&scope, &[s3.clone(), s1.clone()])
            .unwrap();
        assert_eq!(store.session(&s3).unwrap().sort_order, Some(0));
        assert_eq!(store.session(&s1).unwrap().sort_order, Some(1));
        assert_eq!(store.session(&s2).unwrap().sort_order, None);

        // An empty pin list unpins everything of the parent.
        store.pin_sessions(&scope, &[]).unwrap();
        assert_eq!(store.session(&s3).unwrap().sort_order, None);

        // A session of another project cannot be pinned here.
        let web = store.add_project(&local(), "web", "/srv/web").unwrap();
        let far = store
            .upsert_session(&session_at("/srv/web", "far", 400), &[])
            .unwrap();
        assert!(matches!(
            store.pin_sessions(&scope, std::slice::from_ref(&far)),
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(store.session(&far).unwrap().sort_order, None);
        let _ = web;
    }

    #[test]
    fn removing_a_project_removes_its_worktrees_and_keeps_its_sessions() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        store
            .replace_worktrees(&project.id, vec![worktree("/srv/api", Some("main"), true)])
            .unwrap();
        let session_id = store
            .upsert_session(&session_in("/srv/api", "s1"), &[])
            .unwrap();

        store.remove_project(&project.id).unwrap();

        assert!(matches!(
            store.project(&project.id),
            Err(StoreError::NotFound("project"))
        ));
        assert!(store.worktrees(&project.id).unwrap().is_empty());
        assert_eq!(store.session(&session_id).unwrap().project_id, None);
    }

    #[test]
    fn replacing_worktrees_adds_updates_and_removes_to_match_git() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        let first = store
            .replace_worktrees(
                &project.id,
                vec![
                    worktree("/srv/api", Some("main"), true),
                    worktree("/srv/wt/feature", Some("feature"), false),
                    worktree("/srv/wt/old", Some("old"), false),
                ],
            )
            .unwrap();
        assert_eq!(first.len(), 3);
        assert!(first[0].is_main);

        let second = store
            .replace_worktrees(
                &project.id,
                vec![
                    worktree("/srv/api", Some("main"), true),
                    worktree("/srv/wt/feature", None, false),
                    worktree("/srv/wt/new", Some("new"), false),
                ],
            )
            .unwrap();

        let paths: Vec<_> = second.iter().map(|w| w.path.as_str()).collect();
        assert_eq!(paths, ["/srv/api", "/srv/wt/feature", "/srv/wt/new"]);
        assert_eq!(second[1].branch, None, "the branch was refreshed");
        assert_eq!(second[0].id, first[0].id, "a kept worktree keeps its id");
        assert_eq!(second[1].id, first[1].id, "a kept worktree keeps its id");
        assert_eq!(store.worktrees(&project.id).unwrap(), second);
    }

    #[test]
    fn replacing_the_worktrees_of_an_unknown_project_reports_not_found() {
        let store = Store::open_in_memory().unwrap();
        assert!(matches!(
            store.replace_worktrees(&ProjectId::generate(), vec![]),
            Err(StoreError::NotFound("project"))
        ));
    }

    #[test]
    fn a_session_inside_a_project_root_is_linked_to_the_project() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        let inside = store
            .upsert_session(&session_in("/srv/api/src", "inside"), &[])
            .unwrap();
        let sibling = store
            .upsert_session(&session_in("/srv/api-old", "sibling"), &[])
            .unwrap();
        assert_eq!(store.session(&inside).unwrap().project_id, Some(project.id));
        assert_eq!(store.session(&sibling).unwrap().project_id, None);
    }

    #[test]
    fn sessions_imported_before_their_project_existed_are_linked_when_it_is_added() {
        let store = Store::open_in_memory().unwrap();
        let session_id = store
            .upsert_session(&session_in("/srv/api", "early"), &[])
            .unwrap();
        let mut listener = store.subscribe();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        assert_eq!(
            store.session(&session_id).unwrap().project_id,
            Some(project.id)
        );
        assert_eq!(listener.try_next(), Some(StoreChange::Projects));
        assert_eq!(listener.try_next(), Some(StoreChange::Sessions));
    }

    #[test]
    fn a_session_inside_a_worktree_is_linked_to_the_worktrees_project() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        let session_id = store
            .upsert_session(&session_in("/srv/wt/feature/src", "wt"), &[])
            .unwrap();
        assert_eq!(store.session(&session_id).unwrap().project_id, None);

        store
            .replace_worktrees(
                &project.id,
                vec![
                    worktree("/srv/api", Some("main"), true),
                    worktree("/srv/wt/feature", Some("feature"), false),
                ],
            )
            .unwrap();
        assert_eq!(
            store.session(&session_id).unwrap().project_id,
            Some(project.id)
        );
    }

    #[test]
    fn the_deepest_containing_project_wins() {
        let store = Store::open_in_memory().unwrap();
        store.add_project(&local(), "mono", "/srv/mono").unwrap();
        let nested = store
            .add_project(&local(), "nested", "/srv/mono/apps/web")
            .unwrap();
        let session_id = store
            .upsert_session(&session_in("/srv/mono/apps/web/src", "deep"), &[])
            .unwrap();
        assert_eq!(
            store.session(&session_id).unwrap().project_id,
            Some(nested.id)
        );
    }

    #[test]
    fn every_worktree_is_listed_in_one_read_with_each_projects_main_one_first() {
        let store = Store::open_in_memory().unwrap();
        let api = store.add_project(&local(), "api", "/srv/api").unwrap();
        let web = store.add_project(&local(), "web", "/srv/web").unwrap();
        store
            .replace_worktrees(
                &api.id,
                vec![
                    worktree("/srv/wt/x", Some("x"), false),
                    worktree("/srv/api", Some("main"), true),
                ],
            )
            .unwrap();
        store
            .replace_worktrees(&web.id, vec![worktree("/srv/web", Some("main"), true)])
            .unwrap();
        let all = store.all_worktrees().unwrap();
        assert_eq!(all.len(), 3);
        let of = |project: &ProjectId| -> Vec<&str> {
            all.iter()
                .filter(|w| &w.project_id == project)
                .map(|w| w.path.as_str())
                .collect()
        };
        assert_eq!(of(&api.id), ["/srv/api", "/srv/wt/x"]);
        assert_eq!(of(&web.id), ["/srv/web"]);
    }

    #[test]
    fn a_removed_project_is_remembered_until_it_is_added_again() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        assert!(store.dismissed_roots(&local()).unwrap().is_empty());

        store.remove_project(&project.id).unwrap();
        assert!(store
            .dismissed_roots(&local())
            .unwrap()
            .contains("/srv/api"));

        store.add_project(&local(), "api", "/srv/api").unwrap();
        assert!(
            store.dismissed_roots(&local()).unwrap().is_empty(),
            "adding it by hand forgives it"
        );
    }

    #[test]
    fn a_dismissed_root_can_be_forgiven_without_adding_the_project() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", "/srv/api").unwrap();
        store.remove_project(&project.id).unwrap();
        assert!(!store
            .restore_dismissed_root(&local(), "/srv/other")
            .unwrap());
        assert!(store.restore_dismissed_root(&local(), "/srv/api").unwrap());
        assert!(store.dismissed_roots(&local()).unwrap().is_empty());
        assert!(
            store.projects(None).unwrap().is_empty(),
            "discovery decides"
        );
        assert!(!store.restore_dismissed_root(&local(), "/srv/api").unwrap());
    }

    #[test]
    fn dismissed_roots_belong_to_their_machine() {
        let store = Store::open_in_memory().unwrap();
        let remote = store
            .add_machine(
                "box",
                crate::MachineKind::Ssh {
                    host: "box.example".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        let project = store.add_project(&remote.id, "api", "/srv/api").unwrap();
        store.remove_project(&project.id).unwrap();
        assert!(store
            .dismissed_roots(&remote.id)
            .unwrap()
            .contains("/srv/api"));
        assert!(store.dismissed_roots(&local()).unwrap().is_empty());
    }

    #[test]
    fn containment_understands_both_separator_styles() {
        assert!(is_within("/srv/api", "/srv/api"));
        assert!(is_within("/srv/api/src", "/srv/api"));
        assert!(!is_within("/srv/api-old", "/srv/api"));
        assert!(is_within("C:\\code\\api\\src", "C:\\code\\api"));
        assert!(is_within("/srv", "/"));
        assert!(!is_within("/srv", "/srv/api"));
    }

    // ----- path spellings ---------------------------------------------------

    #[test]
    fn a_session_with_a_backslash_cwd_links_to_a_worktree_git_reported_with_slashes() {
        let store = Store::open_in_memory().unwrap();
        let project = store
            .add_project(&local(), "api", "C:/Users/me/code/api")
            .unwrap();
        let worktrees = store
            .replace_worktrees(
                &project.id,
                vec![worktree("C:/Users/me/code/api", Some("main"), true)],
            )
            .unwrap();
        for (n, cwd) in [
            r"C:\Users\me\code\api",
            r"c:\users\me\code\api\src",
            r"\\?\C:\Users\me\code\api\",
            "C:/Users/me/code/api/src/",
        ]
        .into_iter()
        .enumerate()
        {
            let id = store
                .upsert_session(&session_in(cwd, &format!("s{n}")), &[])
                .unwrap();
            assert_eq!(
                store.session(&id).unwrap().project_id,
                Some(project.id.clone()),
                "{cwd}"
            );
        }
        // Not a session of this project: another folder with the same prefix.
        let other = store
            .upsert_session(&session_in(r"C:\Users\me\code\api-old", "x"), &[])
            .unwrap();
        assert_eq!(store.session(&other).unwrap().project_id, None);
        assert_eq!(worktrees.len(), 1);
    }

    #[test]
    fn a_folder_that_differs_only_by_spelling_is_the_same_project_and_worktree() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", r"C:\code\api").unwrap();
        assert!(store
            .add_project(&local(), "again", "c:/code/api/")
            .is_err());
        let first = store
            .replace_worktrees(
                &project.id,
                vec![worktree(r"C:\code\api", Some("main"), true)],
            )
            .unwrap();
        let second = store
            .replace_worktrees(
                &project.id,
                vec![worktree("C:/code/api", Some("dev"), true)],
            )
            .unwrap();
        assert_eq!(second.len(), 1, "no duplicate worktree");
        assert_eq!(second[0].id, first[0].id, "the id is kept");
        assert_eq!(second[0].branch.as_deref(), Some("dev"));
        // POSIX paths that differ by case are different folders.
        store.add_project(&local(), "a", "/srv/Api").unwrap();
        store.add_project(&local(), "b", "/srv/api").unwrap();
    }

    #[test]
    fn a_dismissed_root_is_found_under_any_spelling() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&local(), "api", r"C:\code\api").unwrap();
        store.remove_project(&project.id).unwrap();
        assert!(store
            .restore_dismissed_root(&local(), "c:/code/api/")
            .unwrap());
        assert!(store.dismissed_roots(&local()).unwrap().is_empty());
    }

    #[test]
    fn the_migration_merges_both_spellings_and_relinks_sessions_and_is_idempotent() {
        let store = Store::open_in_memory().unwrap();
        let (kept, dup) = store
            .transact(|tx| {
                tx.execute_batch(
                    "INSERT INTO project (id, machine_id, name, root) VALUES
                       ('p1', 'local', 'api', 'C:/code/api'),
                       ('p2', 'local', 'api', 'c:\\code\\api\\');
                     INSERT INTO worktree (id, project_id, path, branch, head, is_main) VALUES
                       ('w1', 'p1', 'C:/code/api', 'main', NULL, 1),
                       ('w2', 'p2', 'c:\\code\\api', 'main', NULL, 1),
                       ('w3', 'p2', 'c:\\code\\api-x', 'x', NULL, 0);
                     INSERT INTO dismissed_root (machine_id, root) VALUES
                       ('local', 'D:/a'), ('local', 'd:\\a');",
                )?;
                Ok(("p1", "p2"))
            })
            .unwrap();
        let id = store
            .upsert_session(&session_in(r"C:\code\api\src", "s"), &[])
            .unwrap();
        store
            .transact(|tx| {
                tx.execute("UPDATE session SET project_id = NULL", [])?;
                heal_path_spellings(tx)?;
                heal_path_spellings(tx)?;
                Ok(())
            })
            .unwrap();
        let projects = store.projects(Some(&local())).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id.as_str(), kept);
        let paths: Vec<String> = store
            .worktrees(&projects[0].id)
            .unwrap()
            .into_iter()
            .map(|w| w.path)
            .collect();
        assert_eq!(
            paths.len(),
            2,
            "the duplicate worktree is gone, the other moved: {paths:?}"
        );
        assert_eq!(
            store
                .session(&id)
                .unwrap()
                .project_id
                .as_ref()
                .map(ProjectId::as_str),
            Some(kept)
        );
        assert_eq!(store.dismissed_roots(&local()).unwrap().len(), 1);
        assert_ne!(dup, kept);
    }
}

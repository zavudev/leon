//! Session history persistence: sessions, their messages and import cursors.
//!
//! Importers hand over a session together with its complete transcript. The
//! store compares that transcript with what it already holds and writes only
//! the difference, so importing an unchanged session costs one read, and a
//! session that merely grew costs one insert per new message. This matters
//! because every message written is also indexed for full-text search.
//!
//! Import cursors remember a fingerprint per source (a transcript file, or a
//! session inside another tool's database) so an importer can skip sources
//! that have not changed without parsing them.
//!
//! The `*_in` functions take an open transaction. They exist so an importer
//! can store a session and advance its cursor atomically through
//! [`Store::write`].

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use super::projects::{owning_project, project_roots};
use super::{bad_tag, from_millis, to_millis, Store};
use crate::change::StoreChange;
use crate::error::{Result, StoreError};
use crate::ids::{MachineId, ProjectId, SessionId};
use crate::model::{AgentId, Message, NewMessage, NewSession, Role, Session};

/// The session columns, in the order [`session_from_row`] expects them, for a
/// `session` table aliased as `s`.
pub(crate) const SESSION_COLUMNS: &str = "s.id, s.agent, s.external_id, s.machine_id, s.cwd, \
     s.project_id, COALESCE(s.custom_title, s.title), s.model, s.started_at, s.updated_at, s.message_count, s.sort_order, s.account";

/// How many columns [`SESSION_COLUMNS`] selects.
pub(crate) const SESSION_COLUMN_COUNT: usize = 13;

/// Restricts a session listing. An absent field does not restrict.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Only sessions of this agent.
    pub agent: Option<AgentId>,
    /// Only sessions that ran on this machine.
    pub machine_id: Option<MachineId>,
    /// Only sessions linked to this project.
    pub project_id: Option<ProjectId>,
}

impl Store {
    /// Stores a session and its complete transcript, announcing
    /// [`StoreChange::Sessions`]. See [`upsert_session_in`].
    pub fn upsert_session(
        &self,
        session: &NewSession,
        messages: &[NewMessage],
    ) -> Result<SessionId> {
        self.write(StoreChange::Sessions, |tx| {
            upsert_session_in(tx, session, messages)
        })
    }

    /// The session with the given id.
    pub fn session(&self, id: &SessionId) -> Result<Session> {
        self.read(|connection| {
            connection
                .prepare_cached(&format!(
                    "SELECT {SESSION_COLUMNS} FROM session s WHERE s.id = ?1"
                ))?
                .query_row([id.as_str()], |row| session_from_row(row, 0))
                .optional()?
                .ok_or(StoreError::NotFound("session"))
        })
    }

    /// The most recently updated sessions that pass `filter`, newest first.
    pub fn recent_sessions(&self, filter: &SessionFilter, limit: usize) -> Result<Vec<Session>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(&format!(
                "SELECT {SESSION_COLUMNS} FROM session s
                 WHERE (?1 IS NULL OR s.agent = ?1)
                   AND (?2 IS NULL OR s.machine_id = ?2)
                   AND (?3 IS NULL OR s.project_id = ?3)
                 ORDER BY s.updated_at DESC, s.pk DESC
                 LIMIT ?4"
            ))?;
            let sessions = statement
                .query_map(
                    params![
                        filter.agent.map(AgentId::as_str),
                        filter.machine_id.as_ref().map(MachineId::as_str),
                        filter.project_id.as_ref().map(ProjectId::as_str),
                        sql_limit(limit),
                    ],
                    |row| session_from_row(row, 0),
                )?
                .collect::<rusqlite::Result<_>>()?;
            Ok(sessions)
        })
    }

    /// The history session an agent's own id names on a machine, if it was
    /// imported.
    pub fn session_by_external(
        &self,
        machine_id: &MachineId,
        agent: AgentId,
        external_id: &str,
    ) -> Result<Option<Session>> {
        self.read(|connection| {
            Ok(connection
                .prepare_cached(&format!(
                    "SELECT {SESSION_COLUMNS} FROM session s
                     WHERE s.machine_id = ?1 AND s.agent = ?2 AND s.external_id = ?3"
                ))?
                .query_row(
                    params![machine_id.as_str(), agent.as_str(), external_id],
                    |row| session_from_row(row, 0),
                )
                .optional()?)
        })
    }

    /// The distinct working directories of a machine's sessions, sorted. With
    /// `unlinked_only`, only those no project or worktree contains yet: the
    /// ones that could still turn out to belong to a project nobody added.
    pub fn session_cwds(&self, machine_id: &MachineId, unlinked_only: bool) -> Result<Vec<String>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT DISTINCT cwd FROM session
                 WHERE machine_id = ?1 AND (?2 = 0 OR project_id IS NULL)
                 ORDER BY cwd",
            )?;
            let cwds = statement
                .query_map(params![machine_id.as_str(), unlinked_only], |row| {
                    row.get(0)
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(cwds)
        })
    }

    /// The transcript of a session, in order. Empty for an unknown session.
    pub fn session_messages(&self, id: &SessionId) -> Result<Vec<Message>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT m.seq, m.role, m.text, m.at FROM message m
                 JOIN session s ON s.pk = m.session_pk
                 WHERE s.id = ?1
                 ORDER BY m.seq",
            )?;
            let messages = statement
                .query_map([id.as_str()], |row| {
                    let role: String = row.get(1)?;
                    Ok(Message {
                        session_id: id.clone(),
                        seq: row.get(0)?,
                        role: Role::parse(&role).ok_or_else(|| bad_tag(1, &role))?,
                        text: row.get(2)?,
                        at: from_millis(row.get(3)?),
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(messages)
        })
    }

    /// Gives a session the name a person chose (`None` or blank takes it back
    /// to the agent's own title). Imports never overwrite it.
    pub fn rename_session(&self, id: &SessionId, name: Option<&str>) -> Result<()> {
        let name = name.map(str::trim).filter(|name| !name.is_empty());
        self.write(StoreChange::Sessions, |tx| {
            let changed = tx
                .prepare_cached("UPDATE session SET custom_title = ?2 WHERE id = ?1")?
                .execute(params![id.as_str(), name])?;
            if changed == 0 {
                return Err(StoreError::NotFound("session"));
            }
            Ok(())
        })
    }

    /// Remembers which account the session ran with (`None`: the agent's own
    /// setup), so that resuming it starts the same one. Announces nothing when
    /// the session already has it.
    pub fn set_session_account(&self, id: &SessionId, account: Option<&str>) -> Result<()> {
        let current: Option<Option<String>> = self.read(|connection| {
            Ok(connection
                .prepare_cached("SELECT account FROM session WHERE id = ?1")?
                .query_row([id.as_str()], |row| row.get(0))
                .optional()?)
        })?;
        match current {
            None => Err(StoreError::NotFound("session")),
            Some(known) if known.as_deref() == account => Ok(()),
            Some(_) => self.write(StoreChange::Sessions, |tx| {
                tx.prepare_cached("UPDATE session SET account = ?2 WHERE id = ?1")?
                    .execute(params![id.as_str(), account])?;
                Ok(())
            }),
        }
    }

    /// Removes a session and its messages from the history.
    pub fn remove_session(&self, id: &SessionId) -> Result<()> {
        self.write(StoreChange::Sessions, |tx| {
            let removed = tx
                .prepare_cached("DELETE FROM session WHERE id = ?1")?
                .execute([id.as_str()])?;
            if removed == 0 {
                return Err(StoreError::NotFound("session"));
            }
            Ok(())
        })
    }

    /// The fingerprint last recorded for a source, if any.
    pub fn import_cursor(
        &self,
        machine_id: &MachineId,
        source_key: &str,
    ) -> Result<Option<String>> {
        self.read(|connection| {
            Ok(connection
                .prepare_cached(
                    "SELECT fingerprint FROM import_cursor
                     WHERE machine_id = ?1 AND source_key = ?2",
                )?
                .query_row(params![machine_id.as_str(), source_key], |row| row.get(0))
                .optional()?)
        })
    }

    /// Every fingerprint recorded for a machine, keyed by source. Lets an
    /// importer decide what to skip with a single query.
    pub fn import_cursors(&self, machine_id: &MachineId) -> Result<HashMap<String, String>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT source_key, fingerprint FROM import_cursor WHERE machine_id = ?1",
            )?;
            let cursors = statement
                .query_map([machine_id.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(cursors)
        })
    }

    /// Records the fingerprint of a source. Announces nothing: cursors are
    /// bookkeeping that no view displays.
    pub fn set_import_cursor(
        &self,
        machine_id: &MachineId,
        source_key: &str,
        fingerprint: &str,
    ) -> Result<()> {
        self.transact(|tx| set_import_cursor_in(tx, machine_id, source_key, fingerprint))
    }
}

/// What the last import of one agent on one machine did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRun {
    /// The agent whose source was imported.
    pub agent: String,
    /// When the run finished.
    pub at: chrono::DateTime<chrono::Utc>,
    /// How long it took, in milliseconds.
    pub duration_ms: u64,
    /// Items found.
    pub scanned: u64,
    /// Items stored.
    pub imported: u64,
    /// Items skipped because they had not changed.
    pub unchanged: u64,
    /// Items without messages.
    pub empty: u64,
    /// Items or sources that could not be read or stored.
    pub failed: u64,
    /// Rows inside items that could not be understood.
    pub malformed: u64,
    /// Sources whose layout is not known.
    pub unsupported: u64,
}

/// What the store holds of one agent's history, for the diagnosis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOverview {
    /// The agent's catalogue id.
    pub agent: String,
    /// Sessions stored.
    pub sessions: u64,
    /// The latest update time of any of them.
    pub newest: Option<chrono::DateTime<chrono::Utc>>,
    /// Sessions linked to no project.
    pub unplaced: u64,
    /// The last import of this agent, when one was recorded.
    pub last_run: Option<ImportRun>,
}

fn import_run_from_row(row: &Row<'_>) -> rusqlite::Result<ImportRun> {
    Ok(ImportRun {
        agent: row.get(0)?,
        at: from_millis(row.get(1)?),
        duration_ms: row.get::<_, i64>(2)? as u64,
        scanned: row.get::<_, i64>(3)? as u64,
        imported: row.get::<_, i64>(4)? as u64,
        unchanged: row.get::<_, i64>(5)? as u64,
        empty: row.get::<_, i64>(6)? as u64,
        failed: row.get::<_, i64>(7)? as u64,
        malformed: row.get::<_, i64>(8)? as u64,
        unsupported: row.get::<_, i64>(9)? as u64,
    })
}

const IMPORT_RUN_COLUMNS: &str =
    "agent, at, duration_ms, scanned, imported, unchanged, empty, failed, malformed, unsupported";

impl Store {
    /// Records what an import of one agent did, replacing the previous run.
    /// Announces nothing: nothing displays it but the diagnosis.
    pub fn record_import_run(&self, machine_id: &MachineId, run: &ImportRun) -> Result<()> {
        self.transact(|tx| {
            tx.prepare_cached(&format!(
                "INSERT OR REPLACE INTO import_run (machine_id, {IMPORT_RUN_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            ))?
            .execute(params![
                machine_id.as_str(),
                run.agent,
                to_millis(run.at),
                run.duration_ms as i64,
                run.scanned as i64,
                run.imported as i64,
                run.unchanged as i64,
                run.empty as i64,
                run.failed as i64,
                run.malformed as i64,
                run.unsupported as i64,
            ])?;
            Ok(())
        })
    }

    /// The history of every agent on a machine, with its last import.
    pub fn history_overview(&self, machine_id: &MachineId) -> Result<Vec<AgentOverview>> {
        self.read(|connection| overview(connection, machine_id.as_str()))
    }
}

/// Counts what the store holds per agent from an open connection. Tolerates
/// a database from before the import log existed.
fn overview(connection: &Connection, machine: &str) -> Result<Vec<AgentOverview>> {
    let mut runs: HashMap<String, ImportRun> = HashMap::new();
    if let Ok(mut statement) = connection.prepare(&format!(
        "SELECT {IMPORT_RUN_COLUMNS} FROM import_run WHERE machine_id = ?1"
    )) {
        for run in statement
            .query_map([machine], import_run_from_row)?
            .flatten()
        {
            runs.insert(run.agent.clone(), run);
        }
    }
    let mut statement = connection.prepare(
        "SELECT agent, COUNT(*), MAX(updated_at), SUM(project_id IS NULL)
         FROM session WHERE machine_id = ?1 GROUP BY agent ORDER BY agent",
    )?;
    let mut rows: Vec<AgentOverview> = statement
        .query_map([machine], |row| {
            let agent: String = row.get(0)?;
            Ok(AgentOverview {
                sessions: row.get::<_, i64>(1)? as u64,
                newest: row.get::<_, Option<i64>>(2)?.map(from_millis),
                unplaced: row.get::<_, Option<i64>>(3)?.unwrap_or(0) as u64,
                last_run: None,
                agent,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (agent, run) in runs {
        match rows.iter_mut().find(|row| row.agent == agent) {
            Some(row) => row.last_run = Some(run),
            None => rows.push(AgentOverview {
                agent,
                sessions: 0,
                newest: None,
                unplaced: 0,
                last_run: Some(run),
            }),
        }
    }
    rows.sort_by(|a, b| a.agent.cmp(&b.agent));
    Ok(rows)
}

/// Reads the history overview of the database file at `path` without
/// opening it for writing, migrating it or touching its settings: safe
/// against the database of a Leon that is running, of any version.
pub fn history_overview_at(
    path: &std::path::Path,
    machine_id: &MachineId,
) -> Result<Vec<AgentOverview>> {
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    overview(&connection, machine_id.as_str())
}

/// The import cursors of the database file at `path`, read without opening
/// it for writing. Empty for a database from before cursors existed.
pub fn import_cursors_at(
    path: &std::path::Path,
    machine_id: &MachineId,
) -> Result<HashMap<String, String>> {
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    let Ok(mut statement) = connection
        .prepare("SELECT source_key, fingerprint FROM import_cursor WHERE machine_id = ?1")
    else {
        return Ok(HashMap::new());
    };
    let cursors = statement
        .query_map([machine_id.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(cursors)
}

/// Stores a session and its complete transcript inside an open transaction.
///
/// The session is identified by `(agent, machine_id, external_id)`: the first
/// call creates it, later calls update it and keep its id. `messages` is the
/// whole transcript; the stored one is brought in line with it by keeping the
/// leading messages that are identical and rewriting everything from the
/// first difference on. Calling this twice with the same arguments therefore
/// changes nothing the second time.
///
/// The session is linked to the project that contains its working directory,
/// if there is one.
pub fn upsert_session_in(
    tx: &Transaction<'_>,
    session: &NewSession,
    messages: &[NewMessage],
) -> Result<SessionId> {
    let roots = project_roots(tx, &session.machine_id)?;
    let project_id = owning_project(&roots, &session.cwd);

    let (pk, id): (i64, String) = tx
        .prepare_cached(
            "INSERT INTO session (id, agent, external_id, machine_id, cwd, project_id, title,
                                  model, started_at, updated_at, message_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (agent, machine_id, external_id) DO UPDATE
             SET cwd = excluded.cwd, project_id = excluded.project_id, title = excluded.title,
                 model = excluded.model, started_at = excluded.started_at,
                 updated_at = excluded.updated_at, message_count = excluded.message_count
             RETURNING pk, id",
        )?
        .query_row(
            params![
                SessionId::generate().as_str(),
                session.agent.as_str(),
                session.external_id,
                session.machine_id.as_str(),
                session.cwd,
                project_id.as_ref().map(ProjectId::as_str),
                session.title,
                session.model,
                to_millis(session.started_at),
                to_millis(session.updated_at),
                messages.len() as i64,
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

    let (unchanged, stored) = compare_with_stored(tx, pk, messages)?;
    if stored > unchanged {
        tx.prepare_cached("DELETE FROM message WHERE session_pk = ?1 AND seq >= ?2")?
            .execute(params![pk, unchanged as i64])?;
    }
    let mut insert = tx.prepare_cached(
        "INSERT INTO message (session_pk, seq, role, text, at) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (seq, message) in messages.iter().enumerate().skip(unchanged) {
        insert.execute(params![
            pk,
            seq as i64,
            message.role.as_str(),
            message.text,
            to_millis(message.at)
        ])?;
    }

    Ok(SessionId::from_string(id))
}

/// Records, inside an open transaction, that the session `id` ran with the
/// account `account` (an id from [`crate::Account`]). The importer calls it for
/// the sessions it reads from an account's own folder; a session found in the
/// agent's own folders keeps whatever it has.
pub fn set_session_account_in(tx: &Transaction<'_>, id: &SessionId, account: &str) -> Result<()> {
    tx.prepare_cached("UPDATE session SET account = ?2 WHERE id = ?1 AND account IS NOT ?2")?
        .execute(params![id.as_str(), account])?;
    Ok(())
}

/// Compares the stored transcript of a session with `messages`. Returns how
/// many leading messages are identical and how many messages are stored.
fn compare_with_stored(
    connection: &Connection,
    session_pk: i64,
    messages: &[NewMessage],
) -> Result<(usize, usize)> {
    let mut statement = connection
        .prepare_cached("SELECT role, text, at FROM message WHERE session_pk = ?1 ORDER BY seq")?;
    let mut rows = statement.query([session_pk])?;
    let mut unchanged = 0;
    let mut stored = 0;
    let mut diverged = false;
    while let Some(row) = rows.next()? {
        if !diverged {
            let same = messages.get(stored).is_some_and(|message| {
                row.get_ref(0).ok().and_then(|v| v.as_str().ok()) == Some(message.role.as_str())
                    && row.get_ref(1).ok().and_then(|v| v.as_str().ok())
                        == Some(message.text.as_str())
                    && row.get_ref(2).ok().and_then(|v| v.as_i64().ok())
                        == Some(to_millis(message.at))
            });
            if same {
                unchanged += 1;
            } else {
                diverged = true;
            }
        }
        stored += 1;
    }
    Ok((unchanged, stored))
}

/// Records the fingerprint of a source inside an open transaction.
pub fn set_import_cursor_in(
    tx: &Transaction<'_>,
    machine_id: &MachineId,
    source_key: &str,
    fingerprint: &str,
) -> Result<()> {
    tx.prepare_cached(
        "INSERT INTO import_cursor (machine_id, source_key, fingerprint) VALUES (?1, ?2, ?3)
         ON CONFLICT (machine_id, source_key) DO UPDATE SET fingerprint = excluded.fingerprint",
    )?
    .execute(params![machine_id.as_str(), source_key, fingerprint])?;
    Ok(())
}

/// Reads a session from the [`SESSION_COLUMNS`] starting at column `offset`.
pub(crate) fn session_from_row(row: &Row<'_>, offset: usize) -> rusqlite::Result<Session> {
    let agent: String = row.get(offset + 1)?;
    Ok(Session {
        id: SessionId::from_string(row.get::<_, String>(offset)?),
        agent: AgentId::parse(&agent).ok_or_else(|| bad_tag(offset + 1, &agent))?,
        external_id: row.get(offset + 2)?,
        machine_id: MachineId::from_string(row.get::<_, String>(offset + 3)?),
        cwd: row.get(offset + 4)?,
        project_id: row
            .get::<_, Option<String>>(offset + 5)?
            .map(ProjectId::from_string),
        title: row.get(offset + 6)?,
        model: row.get(offset + 7)?,
        started_at: from_millis(row.get(offset + 8)?),
        updated_at: from_millis(row.get(offset + 9)?),
        message_count: row.get(offset + 10)?,
        sort_order: row.get(offset + 11)?,
        account: row.get(offset + 12)?,
    })
}

/// Converts a caller-supplied limit to the integer SQLite expects.
pub(crate) fn sql_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    fn session(external_id: &str) -> NewSession {
        NewSession {
            agent: AgentId::CLAUDE,
            external_id: external_id.into(),
            machine_id: MachineId::local(),
            cwd: "/srv/api".into(),
            title: "Fix the login flow".into(),
            model: Some("model-a".into()),
            started_at: at(100),
            updated_at: at(200),
        }
    }

    fn message(role: Role, text: &str, seconds: i64) -> NewMessage {
        NewMessage {
            role,
            text: text.into(),
            at: at(seconds),
        }
    }

    fn transcript() -> Vec<NewMessage> {
        vec![
            message(Role::User, "please fix the login flow", 100),
            message(Role::Tool, "Read: src/login.rs", 110),
            message(Role::Assistant, "the redirect was wrong", 200),
        ]
    }

    fn message_row_ids(store: &Store) -> Vec<i64> {
        store
            .read(|c| {
                let ids = c
                    .prepare("SELECT id FROM message ORDER BY id")?
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(ids)
            })
            .unwrap()
    }

    #[test]
    fn a_name_a_person_gave_survives_the_next_import_and_can_be_taken_back() {
        let store = Store::open_in_memory().unwrap();
        let id = store.upsert_session(&session("s1"), &transcript()).unwrap();
        store.rename_session(&id, Some("  Login  ")).unwrap();
        assert_eq!(store.session(&id).unwrap().title, "Login");
        // The agent's file changes its title and the import runs again.
        let mut again = session("s1");
        again.title = "Fresh heading".into();
        store.upsert_session(&again, &transcript()).unwrap();
        assert_eq!(store.session(&id).unwrap().title, "Login");
        // A blank name gives the agent's own title back.
        store.rename_session(&id, Some("  ")).unwrap();
        assert_eq!(store.session(&id).unwrap().title, "Fresh heading");
        assert!(store
            .rename_session(&SessionId::from_string("nope"), Some("x"))
            .is_err());
    }

    #[test]
    fn a_stored_session_is_read_back_with_its_transcript() {
        let store = Store::open_in_memory().unwrap();
        let id = store.upsert_session(&session("s1"), &transcript()).unwrap();

        let stored = store.session(&id).unwrap();
        assert_eq!(stored.agent, AgentId::CLAUDE);
        assert_eq!(stored.external_id, "s1");
        assert_eq!(stored.title, "Fix the login flow");
        assert_eq!(stored.model.as_deref(), Some("model-a"));
        assert_eq!(stored.started_at, at(100));
        assert_eq!(stored.updated_at, at(200));
        assert_eq!(stored.message_count, 3);

        let messages = store.session_messages(&id).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].seq, 1);
        assert_eq!(messages[1].role, Role::Tool);
        assert_eq!(messages[1].text, "Read: src/login.rs");
        assert_eq!(messages[2].at, at(200));
        assert_eq!(messages[0].session_id, id);
    }

    #[test]
    fn storing_the_same_session_twice_changes_nothing() {
        let store = Store::open_in_memory().unwrap();
        let first = store.upsert_session(&session("s1"), &transcript()).unwrap();
        let rows_before = message_row_ids(&store);
        let second = store.upsert_session(&session("s1"), &transcript()).unwrap();

        assert_eq!(first, second);
        assert_eq!(message_row_ids(&store), rows_before);
        assert_eq!(
            store
                .recent_sessions(&SessionFilter::default(), 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_grown_transcript_only_appends_the_new_messages() {
        let store = Store::open_in_memory().unwrap();
        let id = store.upsert_session(&session("s1"), &transcript()).unwrap();
        let rows_before = message_row_ids(&store);

        let mut longer = transcript();
        longer.push(message(Role::User, "thanks", 300));
        let mut updated = session("s1");
        updated.updated_at = at(300);
        store.upsert_session(&updated, &longer).unwrap();

        let rows_after = message_row_ids(&store);
        assert_eq!(rows_after[..3], rows_before[..]);
        assert_eq!(rows_after.len(), 4);
        let stored = store.session(&id).unwrap();
        assert_eq!(stored.message_count, 4);
        assert_eq!(stored.updated_at, at(300));
        assert_eq!(store.session_messages(&id).unwrap()[3].text, "thanks");
    }

    #[test]
    fn a_rewritten_transcript_replaces_everything_after_the_first_difference() {
        let store = Store::open_in_memory().unwrap();
        let id = store.upsert_session(&session("s1"), &transcript()).unwrap();

        let rewritten = vec![
            message(Role::User, "please fix the login flow", 100),
            message(Role::Assistant, "a different answer", 150),
        ];
        store.upsert_session(&session("s1"), &rewritten).unwrap();

        let texts: Vec<_> = store
            .session_messages(&id)
            .unwrap()
            .into_iter()
            .map(|m| (m.seq, m.text))
            .collect();
        assert_eq!(
            texts,
            [
                (0, "please fix the login flow".to_owned()),
                (1, "a different answer".to_owned())
            ]
        );
        assert_eq!(store.session(&id).unwrap().message_count, 2);
    }

    #[test]
    fn the_same_external_id_on_another_agent_is_a_different_session() {
        let store = Store::open_in_memory().unwrap();
        let claude = store.upsert_session(&session("shared"), &[]).unwrap();
        let mut other = session("shared");
        other.agent = AgentId::CODEX;
        let codex = store.upsert_session(&other, &[]).unwrap();
        assert_ne!(claude, codex);
    }

    #[test]
    fn a_session_for_an_unknown_machine_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let mut orphan = session("s1");
        orphan.machine_id = MachineId::generate();
        assert!(store.upsert_session(&orphan, &transcript()).is_err());
        assert!(store
            .recent_sessions(&SessionFilter::default(), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn recent_sessions_are_listed_newest_first_and_limited() {
        let store = Store::open_in_memory().unwrap();
        for (name, updated) in [("old", 10), ("new", 30), ("middle", 20)] {
            let mut entry = session(name);
            entry.updated_at = at(updated);
            store.upsert_session(&entry, &[]).unwrap();
        }
        let names: Vec<_> = store
            .recent_sessions(&SessionFilter::default(), 2)
            .unwrap()
            .into_iter()
            .map(|s| s.external_id)
            .collect();
        assert_eq!(names, ["new", "middle"]);
    }

    #[test]
    fn recent_sessions_can_be_filtered_by_agent_machine_and_project() {
        let store = Store::open_in_memory().unwrap();
        let project = store
            .add_project(&MachineId::local(), "api", "/srv/api")
            .unwrap();
        store.upsert_session(&session("in-project"), &[]).unwrap();
        let mut elsewhere = session("elsewhere");
        elsewhere.cwd = "/tmp/scratch".into();
        elsewhere.agent = AgentId::OPENCODE;
        store.upsert_session(&elsewhere, &[]).unwrap();

        let by = |filter: SessionFilter| -> Vec<String> {
            store
                .recent_sessions(&filter, 10)
                .unwrap()
                .into_iter()
                .map(|s| s.external_id)
                .collect()
        };
        assert_eq!(
            by(SessionFilter {
                project_id: Some(project.id),
                ..Default::default()
            }),
            ["in-project"]
        );
        assert_eq!(
            by(SessionFilter {
                agent: Some(AgentId::OPENCODE),
                ..Default::default()
            }),
            ["elsewhere"]
        );
        assert_eq!(
            by(SessionFilter {
                machine_id: Some(MachineId::local()),
                ..Default::default()
            })
            .len(),
            2
        );
        assert!(by(SessionFilter {
            machine_id: Some(MachineId::generate()),
            ..Default::default()
        })
        .is_empty());
    }

    #[test]
    fn session_cwds_are_distinct_per_machine_and_can_be_limited_to_unlinked_ones() {
        let store = Store::open_in_memory().unwrap();
        store
            .add_project(&MachineId::local(), "api", "/srv/api")
            .unwrap();
        for (id, cwd) in [
            ("a", "/srv/api"),
            ("b", "/srv/api"),
            ("c", "/tmp/scratch"),
            ("d", "/tmp/scratch"),
            ("e", "/home/me"),
        ] {
            let mut entry = session(id);
            entry.cwd = cwd.into();
            store.upsert_session(&entry, &[]).unwrap();
        }
        let other = store
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
        let mut remote = session("r");
        remote.machine_id = other.id.clone();
        remote.cwd = "/opt/remote".into();
        store.upsert_session(&remote, &[]).unwrap();

        let local = MachineId::local();
        assert_eq!(
            store.session_cwds(&local, false).unwrap(),
            ["/home/me", "/srv/api", "/tmp/scratch"]
        );
        assert_eq!(
            store.session_cwds(&local, true).unwrap(),
            ["/home/me", "/tmp/scratch"],
            "the project's own directory is already linked"
        );
        assert_eq!(
            store.session_cwds(&other.id, true).unwrap(),
            ["/opt/remote"]
        );
    }

    #[test]
    fn removing_a_session_removes_its_messages() {
        let store = Store::open_in_memory().unwrap();
        let id = store.upsert_session(&session("s1"), &transcript()).unwrap();
        store.remove_session(&id).unwrap();
        assert!(matches!(
            store.session(&id),
            Err(StoreError::NotFound("session"))
        ));
        assert!(message_row_ids(&store).is_empty());
        assert!(matches!(
            store.remove_session(&id),
            Err(StoreError::NotFound("session"))
        ));
    }

    #[test]
    fn an_import_cursor_remembers_the_latest_fingerprint() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        assert_eq!(store.import_cursor(&local, "file-a").unwrap(), None);

        store.set_import_cursor(&local, "file-a", "10:1").unwrap();
        store.set_import_cursor(&local, "file-a", "20:2").unwrap();
        store.set_import_cursor(&local, "file-b", "5:5").unwrap();

        assert_eq!(
            store.import_cursor(&local, "file-a").unwrap().as_deref(),
            Some("20:2")
        );
        let all = store.import_cursors(&local).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all["file-b"], "5:5");
    }

    #[test]
    fn a_session_and_its_cursor_can_be_written_atomically() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        let outcome: Result<()> = store.write(StoreChange::Sessions, |tx| {
            upsert_session_in(tx, &session("s1"), &transcript())?;
            set_import_cursor_in(tx, &local, "file-a", "10:1")?;
            Err(StoreError::Invalid("simulated failure".into()))
        });
        assert!(outcome.is_err());
        assert_eq!(store.import_cursor(&local, "file-a").unwrap(), None);
        assert!(store
            .recent_sessions(&SessionFilter::default(), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_overview_counts_sessions_per_agent_and_keeps_the_last_import_run() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_session(&session("one"), &[]).unwrap();
        let run = ImportRun {
            agent: "claude".into(),
            at: from_millis(5_000),
            duration_ms: 42,
            scanned: 3,
            imported: 2,
            unchanged: 1,
            empty: 0,
            failed: 0,
            malformed: 4,
            unsupported: 0,
        };
        store.record_import_run(&MachineId::local(), &run).unwrap();
        let later = ImportRun {
            duration_ms: 7,
            ..run.clone()
        };
        store
            .record_import_run(&MachineId::local(), &later)
            .unwrap();

        let overview = store.history_overview(&MachineId::local()).unwrap();
        assert_eq!(overview.len(), 1);
        assert_eq!(overview[0].agent, "claude");
        assert_eq!(overview[0].sessions, 1);
        // The session sits in no project, and the last run replaced the first.
        assert_eq!(overview[0].unplaced, 1);
        assert_eq!(overview[0].last_run, Some(later));
    }

    #[test]
    fn the_overview_of_a_file_is_read_without_migrating_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leon.db");
        {
            let store = Store::open(&path).unwrap();
            store.upsert_session(&session("one"), &[]).unwrap();
        }
        let before = std::fs::metadata(&path).unwrap().len();
        let overview = history_overview_at(&path, &MachineId::local()).unwrap();
        assert_eq!(overview[0].sessions, 1);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
    }
}

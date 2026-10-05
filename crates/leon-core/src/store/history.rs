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
use crate::model::{AgentKind, Message, NewMessage, NewSession, Role, Session};

/// The session columns, in the order [`session_from_row`] expects them, for a
/// `session` table aliased as `s`.
pub(crate) const SESSION_COLUMNS: &str = "s.id, s.agent, s.external_id, s.machine_id, s.cwd, \
     s.project_id, s.title, s.model, s.started_at, s.updated_at, s.message_count";

/// How many columns [`SESSION_COLUMNS`] selects.
pub(crate) const SESSION_COLUMN_COUNT: usize = 11;

/// Restricts a session listing. An absent field does not restrict.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Only sessions of this agent.
    pub agent: Option<AgentKind>,
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
                        filter.agent.map(AgentKind::as_str),
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
        agent: AgentKind::parse(&agent).ok_or_else(|| bad_tag(offset + 1, &agent))?,
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
            agent: AgentKind::Claude,
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
    fn a_stored_session_is_read_back_with_its_transcript() {
        let store = Store::open_in_memory().unwrap();
        let id = store.upsert_session(&session("s1"), &transcript()).unwrap();

        let stored = store.session(&id).unwrap();
        assert_eq!(stored.agent, AgentKind::Claude);
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
        other.agent = AgentKind::Codex;
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
        elsewhere.agent = AgentKind::Opencode;
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
                agent: Some(AgentKind::Opencode),
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
}

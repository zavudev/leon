//! Reader for the opencode session database.
//!
//! opencode keeps every session in one SQLite database. Three tables matter:
//!
//! * `session`: one row per session with `directory`, `title`, creation and
//!   update times in milliseconds, and (in newer versions) a `model` column
//!   holding a small JSON object.
//! * `message`: one row per turn; its `data` column is JSON with a `role`
//!   (`user` or `assistant`) and, for assistant turns, a `modelID`.
//! * `part`: the pieces of a turn; `data` is JSON with a `type`. `text` parts
//!   carry the text, `tool` parts describe a tool call (`tool`, and a `state`
//!   with `title` and `input`); the remaining types (`reasoning`,
//!   `step-start`, `step-finish`, `patch`, `file`, ...) are bookkeeping.
//!
//! The database belongs to opencode and may be open and in WAL mode while
//! Leon reads it. It is therefore opened read-only and additionally switched
//! to `query_only`; nothing here ever writes to it.
//!
//! Normalisation rules:
//!
//! * Sessions spawned by another session (sub-agents, marked by `parent_id`)
//!   are not listed, matching how sidechains are skipped for Claude Code.
//! * Consecutive text parts of one turn are joined into one message with the
//!   turn's role. A tool part becomes one [`Role::Tool`] line.
//! * A row whose JSON cannot be read is counted and skipped.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use leon_core::{AgentId, Role};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use serde_json::Value;

use crate::normalize::tool_line;
use crate::session::{ParsedSession, SessionBuilder};

/// How long to wait when opencode holds a lock on its database.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// A session as listed by [`list_sessions`]: its id and a fingerprint that
/// changes whenever the session's content does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedSession {
    /// The opencode session id, the value `opencode --session` accepts.
    pub id: String,
    /// Opaque change marker built from the session's update time, the latest
    /// update time of its messages and its number of parts.
    pub fingerprint: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageData {
    pub(crate) role: Option<String>,
    #[serde(rename = "modelID")]
    pub(crate) model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PartData {
    #[serde(rename = "type")]
    pub(crate) kind: Option<String>,
    pub(crate) text: Option<String>,
    /// Tool name as a V1 part writes it.
    pub(crate) tool: Option<String>,
    /// Tool name as a V2 content part writes it.
    pub(crate) name: Option<String>,
    pub(crate) state: Option<ToolState>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolState {
    pub(crate) title: Option<String>,
    pub(crate) input: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct SessionModel {
    id: Option<String>,
    #[serde(rename = "modelID")]
    model_id: Option<String>,
}

/// One row of `session_message`: a turn of a V2 session.
#[derive(Debug, Deserialize)]
struct TurnData {
    time: Option<TurnTime>,
    text: Option<String>,
    model: Option<SessionModel>,
    content: Option<Vec<PartData>>,
}

#[derive(Debug, Deserialize)]
struct TurnTime {
    created: Option<i64>,
}

/// The two generations of tables opencode keeps its sessions in. A database
/// may hold either or both (an upgraded one keeps the old tables, usually
/// empty, and a downgraded or mixed one may have sessions in each); Leon
/// reads every generation present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generation {
    /// `session`, `message` and `part` (opencode 1.x).
    V1,
    /// `session_v2` and `session_message` (opencode 2.x).
    V2,
}

impl Generation {
    /// The generation as the report names it.
    pub fn label(self) -> &'static str {
        match self {
            Self::V1 => "v1 (session, message, part)",
            Self::V2 => "v2 (session_v2, session_message)",
        }
    }

    fn session_table(self) -> &'static str {
        match self {
            Self::V1 => "session",
            Self::V2 => "session_v2",
        }
    }
}

const SESSION_COLUMNS_NEEDED: [&str; 5] =
    ["id", "directory", "title", "time_created", "time_updated"];

/// Whether the database has a table named `table`.
fn has_table(connection: &Connection, table: &str) -> rusqlite::Result<bool> {
    connection
        .prepare_cached("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?
        .exists([table])
}

/// The generations of tables present and complete enough to read, V2 first
/// (it is the newer: a session in both is read from it).
pub fn generations(connection: &Connection) -> rusqlite::Result<Vec<Generation>> {
    let mut found = Vec::new();
    let complete = |generation: Generation, others: &[&str]| -> rusqlite::Result<bool> {
        for table in others {
            if !has_table(connection, table)? {
                return Ok(false);
            }
        }
        for column in SESSION_COLUMNS_NEEDED {
            if !has_column(connection, generation.session_table(), column)? {
                return Ok(false);
            }
        }
        Ok(true)
    };
    if complete(Generation::V2, &["session_v2", "session_message"])? {
        found.push(Generation::V2);
    }
    if complete(Generation::V1, &["session", "message", "part"])? {
        found.push(Generation::V1);
    }
    Ok(found)
}

/// Opens an opencode database for reading only.
pub fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(connection)
}

/// Lists the top-level sessions of every generation in the database with
/// their fingerprints. A session id held by both generations is listed once,
/// from the newer.
pub fn list_sessions(connection: &Connection) -> rusqlite::Result<Vec<ListedSession>> {
    let mut listed: Vec<ListedSession> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for generation in generations(connection)? {
        for session in list_generation(connection, generation)? {
            if seen.insert(session.id.clone()) {
                listed.push(session);
            }
        }
    }
    Ok(listed)
}

/// The top-level sessions of one generation.
pub fn list_generation(
    connection: &Connection,
    generation: Generation,
) -> rusqlite::Result<Vec<ListedSession>> {
    let (sessions, messages, items) = match generation {
        Generation::V1 => ("session", "message", "part"),
        Generation::V2 => ("session_v2", "session_message", "session_message"),
    };
    let top_level = if has_column(connection, sessions, "parent_id")? {
        "WHERE s.parent_id IS NULL"
    } else {
        ""
    };
    let mut statement = connection.prepare_cached(&format!(
        "SELECT s.id, s.time_updated,
                (SELECT COALESCE(MAX(m.time_updated), 0) FROM {messages} m WHERE m.session_id = s.id),
                (SELECT COUNT(*) FROM {items} p WHERE p.session_id = s.id)
         FROM {sessions} s {top_level}
         ORDER BY s.time_updated"
    ))?;
    let listed = statement
        .query_map([], |row| {
            let updated: i64 = row.get(1)?;
            let latest_message: i64 = row.get(2)?;
            let count: i64 = row.get(3)?;
            Ok(ListedSession {
                id: row.get(0)?,
                fingerprint: format!("{updated}:{latest_message}:{count}"),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(listed)
}

/// Reads one session and its transcript. Returns `None` when the session
/// does not exist or holds no messages.
pub fn read_session(
    connection: &Connection,
    session_id: &str,
) -> rusqlite::Result<Option<ParsedSession>> {
    for generation in generations(connection)? {
        let found = match generation {
            Generation::V2 => read_session_v2(connection, session_id)?,
            Generation::V1 => read_session_v1(connection, session_id)?,
        };
        if let Some(parsed) = found {
            return Ok(Some(parsed));
        }
    }
    Ok(None)
}

/// Reads one V1 session from `session`, `message` and `part`.
fn read_session_v1(
    connection: &Connection,
    session_id: &str,
) -> rusqlite::Result<Option<ParsedSession>> {
    let model_column = if has_column(connection, "session", "model")? {
        "model"
    } else {
        "NULL"
    };
    let header = connection
        .prepare_cached(&format!(
            "SELECT directory, title, time_created, time_updated, {model_column}
             FROM session WHERE id = ?1"
        ))?
        .query_row([session_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .optional()?;
    let Some((directory, title, created, updated, model)) = header else {
        return Ok(None);
    };

    let mut session = SessionBuilder::new(AgentId::OPENCODE, session_id);
    session.see_cwd(directory.as_deref());
    session.see_title(title.as_deref());

    // Turn id to role, plus the model each assistant turn reports.
    let mut roles: HashMap<String, Option<Role>> = HashMap::new();
    let mut turn_model: Option<String> = None;
    {
        let mut statement = connection.prepare_cached(
            "SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created, id",
        )?;
        let mut rows = statement.query([session_id])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let data = row.get_ref(1)?.as_bytes().unwrap_or_default();
            match serde_json::from_slice::<MessageData>(data) {
                Ok(data) => {
                    let role = match data.role.as_deref() {
                        Some("user") => Some(Role::User),
                        Some("assistant") => Some(Role::Assistant),
                        _ => None,
                    };
                    if data.model_id.is_some() {
                        turn_model = data.model_id;
                    }
                    roles.insert(id, role);
                }
                Err(_) => session.skip_malformed(),
            }
        }
    }

    let session_model = model
        .as_deref()
        .and_then(|json| serde_json::from_str::<SessionModel>(json).ok())
        .and_then(|model| model.id.or(model.model_id));
    session.see_model(session_model.as_deref().or(turn_model.as_deref()));

    {
        let mut statement = connection.prepare_cached(
            "SELECT p.message_id, p.time_created, p.data
             FROM part p JOIN message m ON m.id = p.message_id
             WHERE p.session_id = ?1
             ORDER BY m.time_created, m.id, p.id",
        )?;
        let mut rows = statement.query([session_id])?;
        let mut pending: Option<Pending> = None;
        while let Some(row) = rows.next()? {
            let message_id: String = row.get(0)?;
            let at = row
                .get::<_, Option<i64>>(1)?
                .and_then(DateTime::from_timestamp_millis);
            let data = row.get_ref(2)?.as_bytes().unwrap_or_default();
            match serde_json::from_slice::<PartData>(data) {
                Ok(part) => apply_part(&mut session, &mut pending, &roles, message_id, at, part),
                Err(_) => session.skip_malformed(),
            }
        }
        flush(&mut session, pending.take());
    }

    Ok(session.finish().map(|mut parsed| {
        // The session row is the authority on the time span: it also covers
        // activity that left no message behind.
        if let Some(created) = created.and_then(DateTime::from_timestamp_millis) {
            parsed.started_at = parsed.started_at.min(created);
        }
        if let Some(updated) = updated.and_then(DateTime::from_timestamp_millis) {
            parsed.updated_at = parsed.updated_at.max(updated);
        }
        parsed
    }))
}

/// Reads one V2 session from `session_v2` and `session_message`.
fn read_session_v2(
    connection: &Connection,
    session_id: &str,
) -> rusqlite::Result<Option<ParsedSession>> {
    let model_column = if has_column(connection, "session_v2", "model")? {
        "model"
    } else {
        "NULL"
    };
    let header = connection
        .prepare_cached(&format!(
            "SELECT directory, title, time_created, time_updated, {model_column}
             FROM session_v2 WHERE id = ?1"
        ))?
        .query_row([session_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .optional()?;
    let Some((directory, title, created, updated, model)) = header else {
        return Ok(None);
    };

    let mut session = SessionBuilder::new(AgentId::OPENCODE, session_id);
    session.see_cwd(directory.as_deref());
    session.see_title(title.as_deref());
    let session_model = model
        .as_deref()
        .and_then(|json| serde_json::from_str::<SessionModel>(json).ok())
        .and_then(|model| model.id.or(model.model_id));

    // The model each assistant turn reports, used when the session names none.
    let mut turn_model: Option<String> = None;
    {
        let mut statement = connection.prepare_cached(
            "SELECT type, data FROM session_message WHERE session_id = ?1 ORDER BY seq",
        )?;
        let mut rows = statement.query([session_id])?;
        // Text parts of one turn accumulate here until the turn ends or a
        // part that is not text interrupts them.
        let mut pending: Option<(Role, String, Option<DateTime<Utc>>)> = None;
        while let Some(row) = rows.next()? {
            let kind: String = row.get(0)?;
            let data = row.get_ref(1)?.as_bytes().unwrap_or_default();
            let turn = match serde_json::from_slice::<TurnData>(data) {
                Ok(turn) => turn,
                Err(_) => {
                    session.skip_malformed();
                    continue;
                }
            };
            let at = turn
                .time
                .as_ref()
                .and_then(|time| time.created)
                .and_then(DateTime::from_timestamp_millis);
            // Every row is a whole turn: text parts are joined inside one.
            flush_text(&mut session, pending.take());
            match kind.as_str() {
                "user" => {
                    if let Some(text) = turn.text.as_deref() {
                        session.push(Role::User, text, at);
                    }
                }
                "assistant" => {
                    if let Some(model) = turn.model.as_ref().and_then(|model| model.id.as_deref()) {
                        turn_model = Some(model.to_owned());
                    }
                    for part in turn.content.unwrap_or_default() {
                        if part.kind.as_deref() != Some("text") {
                            flush_text(&mut session, pending.take());
                        }
                        match part.kind.as_deref() {
                            Some("text") => {
                                let text = part.text.unwrap_or_default();
                                match pending.as_mut() {
                                    Some((_, joined, _)) => {
                                        joined.push_str("\n\n");
                                        joined.push_str(&text);
                                    }
                                    None => pending = Some((Role::Assistant, text, at)),
                                }
                            }
                            Some("tool") => {
                                let name =
                                    part.tool.as_deref().or(part.name.as_deref()).unwrap_or("");
                                let state = part.state.as_ref();
                                let line = match state.and_then(|state| state.title.as_deref()) {
                                    Some(title) if !title.trim().is_empty() => {
                                        tool_line(name, Some(&Value::String(title.to_owned())))
                                    }
                                    _ => tool_line(
                                        name,
                                        state.and_then(|state| state.input.as_ref()),
                                    ),
                                };
                                session.push(Role::Tool, &line, at);
                            }
                            _ => {}
                        }
                    }
                }
                // The harness's own rows: instructions, shell output it
                // synthesised, idle markers and model switches.
                _ => {}
            }
        }
        flush_text(&mut session, pending.take());
    }
    session.see_model(session_model.as_deref().or(turn_model.as_deref()));

    Ok(session.finish().map(|mut parsed| {
        // The session row is the authority on the time span: it also covers
        // activity that left no message behind.
        if let Some(created) = created.and_then(DateTime::from_timestamp_millis) {
            parsed.started_at = parsed.started_at.min(created);
        }
        if let Some(updated) = updated.and_then(DateTime::from_timestamp_millis) {
            parsed.updated_at = parsed.updated_at.max(updated);
        }
        parsed
    }))
}

/// Appends the text parts collected for one V2 turn as one message.
fn flush_text(
    session: &mut SessionBuilder,
    pending: Option<(Role, String, Option<DateTime<Utc>>)>,
) {
    if let Some((role, text, at)) = pending {
        session.push(role, &text, at);
    }
}

/// The text parts of one turn, collected until the turn ends or a tool call
/// interrupts them: the turn's id, its role, the joined text and its time.
pub(crate) type Pending = (String, Role, String, Option<DateTime<Utc>>);

/// Feeds one part of a turn to the session being built. Shared by the SQLite
/// reader and the older JSON-file reader, so both layouts produce the same
/// transcript. `roles` maps a turn id to its role; a turn that is neither
/// the user's nor the assistant's contributes nothing.
pub(crate) fn apply_part(
    session: &mut SessionBuilder,
    pending: &mut Option<Pending>,
    roles: &HashMap<String, Option<Role>>,
    message_id: String,
    at: Option<DateTime<Utc>>,
    part: PartData,
) {
    let Some(Some(role)) = roles.get(&message_id).copied() else {
        return;
    };
    let continues = pending
        .as_ref()
        .is_some_and(|(turn, _, _, _)| *turn == message_id);
    let is_text = part.kind.as_deref() == Some("text");
    if !(continues && is_text) {
        flush(session, pending.take());
    }
    match part.kind.as_deref() {
        Some("text") => {
            let text = part.text.unwrap_or_default();
            match pending.as_mut() {
                Some((_, _, joined, _)) => {
                    joined.push_str("\n\n");
                    joined.push_str(&text);
                }
                None => *pending = Some((message_id, role, text, at)),
            }
        }
        Some("tool") => {
            let name = part.tool.as_deref().or(part.name.as_deref()).unwrap_or("");
            let state = part.state.as_ref();
            let line = match state.and_then(|s| s.title.as_deref()) {
                Some(title) if !title.trim().is_empty() => {
                    tool_line(name, Some(&Value::String(title.to_owned())))
                }
                _ => tool_line(name, state.and_then(|s| s.input.as_ref())),
            };
            session.push(Role::Tool, &line, at);
        }
        _ => {}
    }
}

pub(crate) fn flush(session: &mut SessionBuilder, pending: Option<Pending>) {
    if let Some((_, role, text, at)) = pending {
        session.push(role, &text, at);
    }
}

/// What an opencode database looks like, for deciding whether Leon can read
/// it and for the history diagnosis. Names and numbers only.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SchemaInfo {
    /// The tables present, sorted.
    pub tables: Vec<String>,
    /// The columns of the `session` table (V1), in table order.
    pub session_columns: Vec<String>,
    /// The columns of the `session_v2` table (V2), in table order.
    pub v2_session_columns: Vec<String>,
    /// `PRAGMA user_version`.
    pub user_version: i64,
    /// `PRAGMA journal_mode`, for example `wal`.
    pub journal_mode: String,
}

impl SchemaInfo {
    fn has(&self, table: &str) -> bool {
        self.tables.iter().any(|name| name == table)
    }

    fn generation_problem(&self, generation: Generation) -> Option<String> {
        let (tables, columns, name): (&[&str], &[String], &str) = match generation {
            Generation::V1 => (
                &["session", "message", "part"],
                &self.session_columns,
                "session",
            ),
            Generation::V2 => (
                &["session_v2", "session_message"],
                &self.v2_session_columns,
                "session_v2",
            ),
        };
        for table in tables {
            if !self.has(table) {
                return Some(format!("no `{table}` table"));
            }
        }
        for column in SESSION_COLUMNS_NEEDED {
            if !columns.iter().any(|found| found == column) {
                return Some(format!("the `{name}` table has no `{column}` column"));
            }
        }
        None
    }

    /// The generations Leon can read in this database.
    pub fn readable(&self) -> Vec<Generation> {
        [Generation::V2, Generation::V1]
            .into_iter()
            .filter(|generation| self.generation_problem(*generation).is_none())
            .collect()
    }

    /// Why Leon cannot read this database, or `None` when it can read at
    /// least one generation. A database that shows signs of V2 is explained
    /// by what its V2 tables lack; any other by what its V1 tables lack.
    pub fn unsupported(&self) -> Option<String> {
        if !self.readable().is_empty() {
            return None;
        }
        let v2 = self.has("session_v2") || self.has("session_message");
        self.generation_problem(if v2 { Generation::V2 } else { Generation::V1 })
    }
}

/// Describes the database's tables and settings.
pub fn inspect(connection: &Connection) -> rusqlite::Result<SchemaInfo> {
    let tables = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let columns = |table: &str| -> rusqlite::Result<Vec<String>> {
        connection
            .prepare("SELECT name FROM pragma_table_info(?1)")?
            .query_map([table], |row| row.get::<_, String>(0))?
            .collect()
    };
    let user_version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let journal_mode = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    Ok(SchemaInfo {
        session_columns: columns("session")?,
        v2_session_columns: columns("session_v2")?,
        tables,
        user_version,
        journal_mode,
    })
}

/// How many sessions a generation holds in all, and how many of them were
/// spawned by another session (sub-agents), which are not listed.
pub fn count_generation(
    connection: &Connection,
    generation: Generation,
) -> rusqlite::Result<(usize, usize)> {
    let table = generation.session_table();
    let total: i64 = connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })?;
    let children: i64 = if has_column(connection, table, "parent_id")? {
        connection.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE parent_id IS NOT NULL"),
            [],
            |row| row.get(0),
        )?
    } else {
        0
    };
    Ok((total as usize, children as usize))
}

/// The ids of every session in the database, children included, of every
/// generation. The older JSON layout is left alone for the sessions the
/// database already holds.
pub fn all_ids(connection: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut ids = Vec::new();
    for generation in generations(connection)? {
        let table = generation.session_table();
        ids.extend(
            connection
                .prepare_cached(&format!("SELECT id FROM {table}"))?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(ids)
}

/// Whether `table` has a column named `column`. opencode adds columns over
/// time, so optional ones are checked before being selected.
fn has_column(connection: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    connection
        .prepare_cached("SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2")?
        .exists([table, column])
}

#[cfg(test)]
pub(crate) mod fixture {
    //! Builds synthetic opencode databases for tests.

    use rusqlite::{params, Connection};
    use serde_json::Value;

    /// Creates the subset of the opencode schema that Leon reads.
    pub(crate) fn create(connection: &Connection) {
        connection
            .execute_batch(
                "CREATE TABLE session (
                     id TEXT PRIMARY KEY, project_id TEXT NOT NULL, parent_id TEXT,
                     slug TEXT NOT NULL, directory TEXT NOT NULL, title TEXT NOT NULL,
                     version TEXT NOT NULL, time_created INTEGER NOT NULL,
                     time_updated INTEGER NOT NULL, agent TEXT, model TEXT);
                 CREATE TABLE message (
                     id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                     time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                     data TEXT NOT NULL);
                 CREATE TABLE part (
                     id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL,
                     time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                     data TEXT NOT NULL);",
            )
            .unwrap();
    }

    pub(crate) fn session(
        connection: &Connection,
        id: &str,
        parent: Option<&str>,
        title: &str,
        created: i64,
        updated: i64,
    ) {
        connection
            .execute(
                "INSERT INTO session (id, project_id, parent_id, slug, directory, title, version,
                                      time_created, time_updated, model)
                 VALUES (?1, 'p', ?2, 'slug', '/srv/api', ?3, '1.0.0', ?4, ?5,
                         '{\"id\":\"model-o\",\"providerID\":\"provider\"}')",
                params![id, parent, title, created, updated],
            )
            .unwrap();
    }

    pub(crate) fn message(connection: &Connection, id: &str, session: &str, at: i64, data: &str) {
        connection
            .execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data)
                 VALUES (?1, ?2, ?3, ?3, ?4)",
                params![id, session, at, data],
            )
            .unwrap();
    }

    pub(crate) fn part(
        connection: &Connection,
        id: &str,
        message: &str,
        session: &str,
        at: i64,
        data: Value,
    ) {
        connection
            .execute(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                params![id, message, session, at, data.to_string()],
            )
            .unwrap();
    }

    /// One session with a user turn, an assistant turn with a tool call, and
    /// a sub-agent session that must not be listed.
    pub(crate) fn populate(connection: &Connection) {
        create(connection);
        session(
            connection,
            "ses_1",
            None,
            "Tidy the config loader",
            1_000,
            9_000,
        );
        message(
            connection,
            "msg_1",
            "ses_1",
            1_000,
            r#"{"role":"user","time":{"created":1000}}"#,
        );
        part(
            connection,
            "prt_1",
            "msg_1",
            "ses_1",
            1_000,
            serde_json::json!({"type": "text", "text": "tidy the config loader"}),
        );
        message(
            connection,
            "msg_2",
            "ses_1",
            2_000,
            r#"{"role":"assistant","modelID":"model-turn","providerID":"provider"}"#,
        );
        let parts = [
            serde_json::json!({"type": "step-start", "snapshot": "abc"}),
            serde_json::json!({"type": "reasoning", "text": "private reasoning"}),
            serde_json::json!({"type": "text", "text": "Reading the loader."}),
            serde_json::json!({"type": "tool", "callID": "c1", "tool": "read",
                "state": {"status": "completed", "title": "src/config.rs",
                          "input": {"filePath": "/srv/api/src/config.rs"}}}),
            serde_json::json!({"type": "text", "text": "Done."}),
            serde_json::json!({"type": "text", "text": "Anything else?"}),
            serde_json::json!({"type": "step-finish", "reason": "stop"}),
        ];
        for (index, data) in parts.into_iter().enumerate() {
            part(
                connection,
                &format!("prt_2{index}"),
                "msg_2",
                "ses_1",
                2_000 + index as i64,
                data,
            );
        }

        session(
            connection,
            "ses_child",
            Some("ses_1"),
            "Sub-agent",
            3_000,
            4_000,
        );
        message(
            connection,
            "msg_c",
            "ses_child",
            3_000,
            r#"{"role":"user"}"#,
        );
        part(
            connection,
            "prt_c",
            "msg_c",
            "ses_child",
            3_000,
            serde_json::json!({"type": "text", "text": "delegated work"}),
        );
    }
    /// Creates the V2 tables, alongside the empty V1 ones an upgraded
    /// database keeps, and returns the connection ready for sessions.
    pub(crate) fn create_v2(connection: &Connection) {
        create_v2_only(connection);
        create(connection);
    }

    /// Only the V2 tables, as a fresh opencode 2.x database has them.
    pub(crate) fn create_v2_only(connection: &Connection) {
        connection
            .execute_batch(
                "CREATE TABLE session_v2 (
                     id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL,
                     title TEXT NOT NULL, time_created INTEGER NOT NULL,
                     time_updated INTEGER NOT NULL, model TEXT);
                 CREATE TABLE session_message (
                     id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
                     seq INTEGER NOT NULL, time_created INTEGER NOT NULL,
                     time_updated INTEGER NOT NULL, data TEXT NOT NULL);",
            )
            .unwrap();
    }

    pub(crate) fn session_v2(
        connection: &Connection,
        id: &str,
        parent: Option<&str>,
        title: &str,
        created: i64,
        updated: i64,
    ) {
        connection
            .execute(
                "INSERT INTO session_v2 (id, parent_id, directory, title, time_created,
                                         time_updated, model)
                 VALUES (?1, ?2, '/srv/api', ?3, ?4, ?5,
                         '{\"id\":\"model-o\",\"providerID\":\"provider\"}')",
                params![id, parent, title, created, updated],
            )
            .unwrap();
    }

    pub(crate) fn turn(
        connection: &Connection,
        id: &str,
        session: &str,
        kind: &str,
        seq: i64,
        at: i64,
        data: Value,
    ) {
        connection
            .execute(
                "INSERT INTO session_message (id, session_id, type, seq, time_created,
                                              time_updated, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                params![id, session, kind, seq, at, data.to_string()],
            )
            .unwrap();
    }

    /// The same session as [`populate`], in the V2 layout.
    pub(crate) fn populate_v2(connection: &Connection) {
        create_v2(connection);
        session_v2(
            connection,
            "ses_1",
            None,
            "Tidy the config loader",
            1_000,
            9_000,
        );
        turn(
            connection,
            "msg_1",
            "ses_1",
            "user",
            1,
            1_000,
            serde_json::json!({"time": {"created": 1_000}, "text": "tidy the config loader"}),
        );
        turn(
            connection,
            "msg_sys",
            "ses_1",
            "system",
            2,
            1_500,
            serde_json::json!({"time": {"created": 1_500},
                               "text": "instructions the harness injected"}),
        );
        turn(
            connection,
            "msg_2",
            "ses_1",
            "assistant",
            3,
            2_000,
            serde_json::json!({
                "time": {"created": 2_000, "completed": 2_500},
                "agent": "build",
                "model": {"id": "model-turn", "providerID": "provider"},
                "content": [
                    {"type": "reasoning", "text": "private reasoning"},
                    {"type": "step-start", "snapshot": "abc"},
                    {"type": "text", "text": "Reading the loader."},
                    {"type": "tool", "name": "read", "state": {
                        "status": "completed", "title": "src/config.rs",
                        "input": {"filePath": "/srv/api/src/config.rs"}}},
                    {"type": "text", "text": "Done."},
                    {"type": "text", "text": "Anything else?"},
                    {"type": "step-finish", "reason": "stop"}
                ]
            }),
        );
        turn(
            connection,
            "msg_idle",
            "ses_1",
            "idle",
            4,
            9_000,
            serde_json::json!({"time": {"created": 9_000}, "outcome": "succeeded"}),
        );
        session_v2(
            connection,
            "ses_child",
            Some("ses_1"),
            "Sub-agent",
            3_000,
            4_000,
        );
        turn(
            connection,
            "msg_c",
            "ses_child",
            "user",
            1,
            3_000,
            serde_json::json!({"time": {"created": 3_000}, "text": "delegated work"}),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        fixture::populate(&connection);
        connection
    }

    fn database_v2() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        fixture::populate_v2(&connection);
        connection
    }

    fn roles_and_texts(session: &ParsedSession) -> Vec<(Role, &str)> {
        session
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect()
    }

    #[test]
    fn only_top_level_sessions_are_listed() {
        let listed = list_sessions(&database()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "ses_1");
    }

    #[test]
    fn a_session_is_read_with_its_metadata_and_transcript() {
        let session = read_session(&database(), "ses_1").unwrap().unwrap();

        assert_eq!(session.agent, AgentId::OPENCODE);
        assert_eq!(session.external_id, "ses_1");
        assert_eq!(session.cwd, "/srv/api");
        assert_eq!(session.title, "Tidy the config loader");
        assert_eq!(session.model.as_deref(), Some("model-o"));
        assert_eq!(
            session.started_at,
            DateTime::from_timestamp_millis(1_000).unwrap()
        );
        assert_eq!(
            session.updated_at,
            DateTime::from_timestamp_millis(9_000).unwrap()
        );
        assert_eq!(
            roles_and_texts(&session),
            [
                (Role::User, "tidy the config loader"),
                (Role::Assistant, "Reading the loader."),
                (Role::Tool, "read: src/config.rs"),
                (Role::Assistant, "Done.\n\nAnything else?"),
            ]
        );
        assert_eq!(session.malformed, 0);
    }

    #[test]
    fn the_model_of_the_latest_turn_is_used_when_the_session_names_none() {
        let connection = database();
        connection
            .execute("UPDATE session SET model = NULL", [])
            .unwrap();
        let session = read_session(&connection, "ses_1").unwrap().unwrap();
        assert_eq!(session.model.as_deref(), Some("model-turn"));
    }

    #[test]
    fn a_tool_call_without_a_title_is_described_by_its_input() {
        let connection = database();
        connection
            .execute(
                "UPDATE part SET data = ?1 WHERE id = 'prt_23'",
                [json!({"type": "tool", "tool": "bash",
                        "state": {"status": "running", "input": {"command": "cargo check"}}})
                .to_string()],
            )
            .unwrap();
        let session = read_session(&connection, "ses_1").unwrap().unwrap();
        assert_eq!(session.messages[2].text, "bash: cargo check");
    }

    #[test]
    fn rows_with_unreadable_json_are_counted_and_skipped() {
        let connection = database();
        fixture::message(&connection, "msg_bad", "ses_1", 5_000, "{not json");
        fixture::part(
            &connection,
            "prt_x",
            "msg_bad",
            "ses_1",
            5_000,
            json!({"type": "text", "text": "lost"}),
        );
        connection
            .execute("UPDATE part SET data = '[broken' WHERE id = 'prt_24'", [])
            .unwrap();

        let session = read_session(&connection, "ses_1").unwrap().unwrap();
        assert_eq!(session.malformed, 2);
        assert_eq!(
            roles_and_texts(&session).last(),
            Some(&(Role::Assistant, "Anything else?"))
        );
    }

    #[test]
    fn an_unknown_or_empty_session_yields_nothing() {
        let connection = database();
        assert!(read_session(&connection, "missing").unwrap().is_none());
        fixture::session(&connection, "ses_empty", None, "Empty", 1, 2);
        assert!(read_session(&connection, "ses_empty").unwrap().is_none());
    }

    #[test]
    fn the_fingerprint_changes_when_the_session_grows() {
        let connection = database();
        let before = list_sessions(&connection).unwrap();
        fixture::part(
            &connection,
            "prt_new",
            "msg_2",
            "ses_1",
            8_000,
            json!({"type": "text", "text": "more"}),
        );
        let after = list_sessions(&connection).unwrap();
        assert_ne!(before[0].fingerprint, after[0].fingerprint);
        assert_eq!(list_sessions(&connection).unwrap(), after);
    }

    #[test]
    fn a_v2_database_lists_only_its_top_level_sessions() {
        let listed = list_sessions(&database_v2()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "ses_1");
    }

    #[test]
    fn a_v2_session_is_read_with_its_metadata_and_transcript() {
        let session = read_session(&database_v2(), "ses_1").unwrap().unwrap();

        assert_eq!(session.agent, AgentId::OPENCODE);
        assert_eq!(session.external_id, "ses_1");
        assert_eq!(session.cwd, "/srv/api");
        assert_eq!(session.title, "Tidy the config loader");
        assert_eq!(session.model.as_deref(), Some("model-o"));
        assert_eq!(
            session.started_at,
            DateTime::from_timestamp_millis(1_000).unwrap()
        );
        assert_eq!(
            session.updated_at,
            DateTime::from_timestamp_millis(9_000).unwrap()
        );
        assert_eq!(
            roles_and_texts(&session),
            [
                (Role::User, "tidy the config loader"),
                (Role::Assistant, "Reading the loader."),
                (Role::Tool, "read: src/config.rs"),
                (Role::Assistant, "Done.\n\nAnything else?"),
            ],
            "the system and idle rows are bookkeeping and are skipped"
        );
        assert_eq!(session.malformed, 0);
    }

    #[test]
    fn the_model_of_the_latest_v2_turn_is_used_when_the_session_names_none() {
        let connection = database_v2();
        connection
            .execute("UPDATE session_v2 SET model = NULL", [])
            .unwrap();
        let session = read_session(&connection, "ses_1").unwrap().unwrap();
        assert_eq!(session.model.as_deref(), Some("model-turn"));
    }

    #[test]
    fn a_v2_tool_call_without_a_title_is_described_by_its_input() {
        let connection = database_v2();
        connection
            .execute(
                "UPDATE session_message SET data = ?1 WHERE id = 'msg_2'",
                [json!({"time": {"created": 2_000},
                        "content": [{"type": "tool", "name": "bash",
                                     "state": {"status": "running",
                                               "input": {"command": "cargo check"}}}]})
                .to_string()],
            )
            .unwrap();
        let session = read_session(&connection, "ses_1").unwrap().unwrap();
        assert_eq!(session.messages[1].text, "bash: cargo check");
    }

    #[test]
    fn v2_rows_with_unreadable_json_are_counted_and_skipped() {
        let connection = database_v2();
        connection
            .execute(
                "INSERT INTO session_message (id, session_id, type, seq, time_created,
                                              time_updated, data)
                 VALUES ('msg_bad', 'ses_1', 'user', 5, 5_000, 5_000, '{not json')",
                [],
            )
            .unwrap();

        let session = read_session(&connection, "ses_1").unwrap().unwrap();
        assert_eq!(session.malformed, 1);
        assert_eq!(
            roles_and_texts(&session).last(),
            Some(&(Role::Assistant, "Done.\n\nAnything else?"))
        );
    }

    #[test]
    fn the_v2_fingerprint_changes_when_a_turn_arrives() {
        let connection = database_v2();
        let before = list_sessions(&connection).unwrap();
        fixture::turn(
            &connection,
            "msg_new",
            "ses_1",
            "user",
            5,
            8_000,
            json!({"time": {"created": 8_000}, "text": "more"}),
        );
        let after = list_sessions(&connection).unwrap();
        assert_ne!(before[0].fingerprint, after[0].fingerprint);
        assert_eq!(list_sessions(&connection).unwrap(), after);
    }

    #[test]
    fn a_database_with_both_generations_lists_the_sessions_of_each_once() {
        let connection = Connection::open_in_memory().unwrap();
        fixture::create_v2(&connection);
        // One session only in the old tables, one only in the new ones, and
        // one id held by both (the newer generation wins).
        fixture::session(&connection, "ses_old", None, "Old session", 1, 2);
        fixture::message(&connection, "m_old", "ses_old", 1, r#"{"role":"user"}"#);
        fixture::part(
            &connection,
            "p_old",
            "m_old",
            "ses_old",
            1,
            json!({"type": "text", "text": "from the old tables"}),
        );
        fixture::session_v2(&connection, "ses_new", None, "New session", 3, 4);
        fixture::turn(
            &connection,
            "t_new",
            "ses_new",
            "user",
            1,
            3,
            json!({"time": {"created": 3}, "text": "from the new tables"}),
        );
        fixture::session(&connection, "ses_both", None, "Stale copy", 5, 6);
        fixture::session_v2(&connection, "ses_both", None, "Fresh copy", 7, 8);
        fixture::turn(
            &connection,
            "t_both",
            "ses_both",
            "user",
            1,
            7,
            json!({"time": {"created": 7}, "text": "the newer one"}),
        );

        let mut ids: Vec<String> = list_sessions(&connection)
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            ["ses_both", "ses_new", "ses_old"],
            "none dropped, none twice"
        );
        let old = read_session(&connection, "ses_old").unwrap().unwrap();
        assert_eq!(roles_and_texts(&old), [(Role::User, "from the old tables")]);
        let new = read_session(&connection, "ses_new").unwrap().unwrap();
        assert_eq!(roles_and_texts(&new), [(Role::User, "from the new tables")]);
        let both = read_session(&connection, "ses_both").unwrap().unwrap();
        assert_eq!(roles_and_texts(&both), [(Role::User, "the newer one")]);
        assert_eq!(both.title, "Fresh copy");
        let info = inspect(&connection).unwrap();
        assert_eq!(info.readable(), [Generation::V2, Generation::V1]);
        assert_eq!(all_ids(&connection).unwrap().len(), 4);
    }

    #[test]
    fn a_two_x_only_database_has_no_old_tables_and_is_read() {
        let connection = Connection::open_in_memory().unwrap();
        fixture::create_v2_only(&connection);
        fixture::session_v2(&connection, "ses_new", None, "New session", 3, 4);
        fixture::turn(
            &connection,
            "t",
            "ses_new",
            "user",
            1,
            3,
            json!({"time": {"created": 3}, "text": "hello 2.x"}),
        );
        let info = inspect(&connection).unwrap();
        assert_eq!(info.unsupported(), None);
        assert_eq!(info.readable(), [Generation::V2]);
        assert_eq!(list_sessions(&connection).unwrap().len(), 1);
        let session = read_session(&connection, "ses_new").unwrap().unwrap();
        assert_eq!(roles_and_texts(&session), [(Role::User, "hello 2.x")]);
        assert_eq!(
            count_generation(&connection, Generation::V2).unwrap(),
            (1, 0)
        );
    }

    #[test]
    fn a_half_made_v2_database_is_unsupported_and_says_what_is_missing() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE session_v2 (id TEXT, directory TEXT);")
            .unwrap();
        let info = inspect(&connection).unwrap();
        assert_eq!(
            info.unsupported().as_deref(),
            Some("no `session_message` table")
        );
        assert!(list_sessions(&connection).unwrap().is_empty());
    }

    #[test]
    fn a_database_from_an_older_schema_without_optional_columns_is_read() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT NOT NULL,
                     title TEXT NOT NULL, time_created INTEGER NOT NULL,
                     time_updated INTEGER NOT NULL);
                 CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                     time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                     data TEXT NOT NULL);
                 CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL,
                     session_id TEXT NOT NULL, time_created INTEGER NOT NULL,
                     time_updated INTEGER NOT NULL, data TEXT NOT NULL);
                 INSERT INTO session VALUES ('ses_old', '/srv/old', 'Old session', 10, 20);",
            )
            .unwrap();
        fixture::message(&connection, "m", "ses_old", 10, r#"{"role":"user"}"#);
        fixture::part(
            &connection,
            "p",
            "m",
            "ses_old",
            10,
            json!({"type": "text", "text": "hello"}),
        );

        assert_eq!(list_sessions(&connection).unwrap().len(), 1);
        let session = read_session(&connection, "ses_old").unwrap().unwrap();
        assert_eq!(session.model, None);
        assert_eq!(roles_and_texts(&session), [(Role::User, "hello")]);
    }

    #[test]
    fn a_database_opened_for_import_cannot_be_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .unwrap();
            fixture::populate(&connection);
        }
        let connection = open_read_only(&path).unwrap();
        assert_eq!(list_sessions(&connection).unwrap().len(), 1);
        assert!(connection.execute("DELETE FROM session", []).is_err());
    }
}

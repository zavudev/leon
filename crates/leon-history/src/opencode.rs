//! Reader for the opencode session database.
//!
//! opencode keeps every session in one SQLite database, in one of two
//! layouts:
//!
//! * **V2** (opencode 2.x): `session_v2` holds one row per session, with
//!   `directory`, `title`, creation and update times in milliseconds and a
//!   `model` column holding a small JSON object; `session_message` holds one
//!   row per turn, with a `type` (`user`, `assistant`, `system`, `synthetic`,
//!   `idle`, `model-switched`) and a `data` column with the turn's payload.
//!   An assistant turn carries its content as a list of parts (`text`,
//!   `reasoning`, `tool`, `step-start`, `step-finish`, ...).
//! * **V1** (older versions): `session`, `message` and `part`. A message's
//!   `data` is JSON with a `role` (`user` or `assistant`); a part's `data` is
//!   JSON with a `type`. `text` parts carry the text, `tool` parts describe a
//!   tool call (`tool`, and a `state` with `title` and `input`); the
//!   remaining types are bookkeeping.
//!
//! An upgraded database keeps the V1 tables behind, empty; the presence of
//! `session_v2` and `session_message` decides which layout is read.
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
//!   turn's role. A tool part becomes one [`Role::Tool`] line. Reasoning,
//!   step markers and the harness's own rows (`system`, `synthetic`, `idle`,
//!   `model-switched`) are bookkeeping and are skipped.
//! * A row whose JSON cannot be read is counted and skipped.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use leon_core::{AgentKind, Role};
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
struct MessageData {
    role: Option<String>,
    #[serde(rename = "modelID")]
    model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PartData {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    /// Tool name as a V1 part writes it.
    tool: Option<String>,
    /// Tool name as a V2 content part writes it.
    name: Option<String>,
    state: Option<ToolState>,
}

#[derive(Debug, Deserialize)]
struct ToolState {
    title: Option<String>,
    input: Option<Value>,
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

/// Which tables hold the sessions. opencode V2 moved them into `session_v2`
/// and `session_message`; an upgraded database keeps the old tables behind,
/// empty, so the new ones decide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Schema {
    /// `session_v2` and `session_message` (opencode 2.x).
    V2,
    /// `session`, `message` and `part` (older versions).
    Legacy,
}

/// The layout of `connection`.
fn schema(connection: &Connection) -> rusqlite::Result<Schema> {
    if has_table(connection, "session_v2")? && has_table(connection, "session_message")? {
        Ok(Schema::V2)
    } else {
        Ok(Schema::Legacy)
    }
}

/// Whether the database has a table named `table`. A database written by an
/// older opencode has no `session_v2` at all.
fn has_table(connection: &Connection, table: &str) -> rusqlite::Result<bool> {
    connection
        .prepare_cached("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?
        .exists([table])
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

/// Lists the top-level sessions in the database with their fingerprints.
pub fn list_sessions(connection: &Connection) -> rusqlite::Result<Vec<ListedSession>> {
    match schema(connection)? {
        Schema::V2 => {
            let top_level = if has_column(connection, "session_v2", "parent_id")? {
                "WHERE s.parent_id IS NULL"
            } else {
                ""
            };
            list_sessions_in(
                connection,
                "session_v2",
                "session_message",
                "session_message",
                top_level,
            )
        }
        Schema::Legacy => {
            let top_level = if has_column(connection, "session", "parent_id")? {
                "WHERE s.parent_id IS NULL"
            } else {
                ""
            };
            list_sessions_in(connection, "session", "message", "part", top_level)
        }
    }
}

/// Lists the sessions of a layout: `sessions` is the session table,
/// `messages` the table whose latest update marks a change and `items` the
/// pieces counted (parts in V1, turns in V2).
fn list_sessions_in(
    connection: &Connection,
    sessions: &str,
    messages: &str,
    items: &str,
    top_level: &str,
) -> rusqlite::Result<Vec<ListedSession>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT s.id, s.time_updated,
                (SELECT COALESCE(MAX(m.time_updated), 0) FROM {messages} m WHERE m.session_id = s.id),
                (SELECT COUNT(*) FROM {items} p WHERE p.session_id = s.id)
         FROM {sessions} s {top_level}
         ORDER BY s.time_updated"
    ))?;
    let sessions = statement
        .query_map([], |row| {
            let updated: i64 = row.get(1)?;
            let latest_message: i64 = row.get(2)?;
            let parts: i64 = row.get(3)?;
            Ok(ListedSession {
                id: row.get(0)?,
                fingerprint: format!("{updated}:{latest_message}:{parts}"),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(sessions)
}

/// Reads one session and its transcript. Returns `None` when the session
/// does not exist or holds no messages.
pub fn read_session(
    connection: &Connection,
    session_id: &str,
) -> rusqlite::Result<Option<ParsedSession>> {
    match schema(connection)? {
        Schema::V2 => read_session_v2(connection, session_id),
        Schema::Legacy => read_session_legacy(connection, session_id),
    }
}

/// Reads one V1 session from `session`, `message` and `part`.
fn read_session_legacy(
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

    let mut session = SessionBuilder::new(AgentKind::Opencode, session_id);
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
        // Text parts of one turn accumulate here until the turn ends or a
        // tool call interrupts them.
        let mut pending: Option<(String, Role, String, Option<DateTime<Utc>>)> = None;
        while let Some(row) = rows.next()? {
            let message_id: String = row.get(0)?;
            let at = row
                .get::<_, Option<i64>>(1)?
                .and_then(DateTime::from_timestamp_millis);
            let data = row.get_ref(2)?.as_bytes().unwrap_or_default();
            let Ok(part) = serde_json::from_slice::<PartData>(data) else {
                session.skip_malformed();
                continue;
            };
            let Some(Some(role)) = roles.get(&message_id).copied() else {
                continue;
            };

            let continues = pending
                .as_ref()
                .is_some_and(|(turn, _, _, _)| *turn == message_id);
            let is_text = part.kind.as_deref() == Some("text");
            if !(continues && is_text) {
                flush(&mut session, pending.take());
            }
            match part.kind.as_deref() {
                Some("text") => {
                    let text = part.text.unwrap_or_default();
                    match pending.as_mut() {
                        Some((_, _, joined, _)) => {
                            joined.push_str("\n\n");
                            joined.push_str(&text);
                        }
                        None => pending = Some((message_id, role, text, at)),
                    }
                }
                Some("tool") => {
                    let name = part.tool.as_deref().unwrap_or("");
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

    let mut session = SessionBuilder::new(AgentKind::Opencode, session_id);
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

fn flush(
    session: &mut SessionBuilder,
    pending: Option<(String, Role, String, Option<DateTime<Utc>>)>,
) {
    if let Some((_, role, text, at)) = pending {
        session.push(role, &text, at);
    }
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
        create(connection);
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

        assert_eq!(session.agent, AgentKind::Opencode);
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

        assert_eq!(session.agent, AgentKind::Opencode);
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
    fn the_v2_schema_is_read_when_the_old_tables_are_still_behind() {
        let connection = Connection::open_in_memory().unwrap();
        fixture::create_v2(&connection);
        // An upgraded database keeps the V1 tables behind: a row still
        // lingering there must not be listed once the V2 tables exist.
        fixture::session(&connection, "ses_old", None, "Old session", 1, 2);
        fixture::session_v2(&connection, "ses_new", None, "New session", 3, 4);
        fixture::turn(
            &connection,
            "msg_new",
            "ses_new",
            "user",
            1,
            3,
            json!({"time": {"created": 3}, "text": "from the new schema"}),
        );

        let listed = list_sessions(&connection).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            ["ses_new"]
        );
        let session = read_session(&connection, "ses_new").unwrap().unwrap();
        assert_eq!(
            roles_and_texts(&session),
            [(Role::User, "from the new schema")]
        );
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

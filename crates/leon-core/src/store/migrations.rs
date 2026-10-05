//! Schema migrations.
//!
//! The schema only ever moves forward. `PRAGMA user_version` records how many
//! entries of [`MIGRATIONS`] have been applied; opening a database applies the
//! remaining ones, each in its own transaction. Entries are never edited once
//! released: a schema change is always a new entry appended to the list.
//!
//! Layout notes for version 1:
//!
//! * Timestamps are integer milliseconds since the Unix epoch, in UTC.
//! * `session.pk` is a compact integer key used for joins and as the row id of
//!   the title index; `session.id` is the public identifier.
//! * `message_fts` and `session_fts` are external-content FTS5 indexes kept in
//!   step by triggers, so the text is stored once and the index cannot drift
//!   from the tables, whichever code path writes them.

use rusqlite::Connection;

use crate::error::{Result, StoreError};

/// Every migration, oldest first. The index of an entry plus one is the
/// schema version it produces.
const MIGRATIONS: &[&str] = &[V1, V2, V3];

const V1: &str = r#"
CREATE TABLE machine (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL,
    host          TEXT,
    user          TEXT,
    port          INTEGER,
    identity_file TEXT
) WITHOUT ROWID;

INSERT INTO machine (id, name, kind) VALUES ('local', 'This machine', 'local');

CREATE TABLE project (
    id         TEXT PRIMARY KEY,
    machine_id TEXT NOT NULL REFERENCES machine(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    root       TEXT NOT NULL,
    UNIQUE (machine_id, root)
) WITHOUT ROWID;

CREATE TABLE worktree (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    branch     TEXT,
    head       TEXT,
    is_main    INTEGER NOT NULL,
    UNIQUE (project_id, path)
) WITHOUT ROWID;

CREATE TABLE session (
    pk            INTEGER PRIMARY KEY,
    id            TEXT NOT NULL UNIQUE,
    agent         TEXT NOT NULL,
    external_id   TEXT NOT NULL,
    machine_id    TEXT NOT NULL REFERENCES machine(id) ON DELETE CASCADE,
    cwd           TEXT NOT NULL,
    project_id    TEXT REFERENCES project(id) ON DELETE SET NULL,
    title         TEXT NOT NULL,
    model         TEXT,
    started_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    message_count INTEGER NOT NULL,
    UNIQUE (agent, machine_id, external_id)
);

CREATE INDEX session_by_updated ON session (updated_at DESC);
CREATE INDEX session_by_machine ON session (machine_id, updated_at DESC);
CREATE INDEX session_by_project ON session (project_id, updated_at DESC);
CREATE INDEX session_by_cwd ON session (machine_id, cwd);

CREATE TABLE message (
    id         INTEGER PRIMARY KEY,
    session_pk INTEGER NOT NULL REFERENCES session(pk) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    role       TEXT NOT NULL,
    text       TEXT NOT NULL,
    at         INTEGER NOT NULL,
    UNIQUE (session_pk, seq)
);

CREATE TABLE import_cursor (
    machine_id  TEXT NOT NULL REFERENCES machine(id) ON DELETE CASCADE,
    source_key  TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    PRIMARY KEY (machine_id, source_key)
) WITHOUT ROWID;

CREATE VIRTUAL TABLE message_fts USING fts5(
    text,
    content = 'message',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER message_fts_insert AFTER INSERT ON message BEGIN
    INSERT INTO message_fts (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER message_fts_delete AFTER DELETE ON message BEGIN
    INSERT INTO message_fts (message_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER message_fts_update AFTER UPDATE OF text ON message BEGIN
    INSERT INTO message_fts (message_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO message_fts (rowid, text) VALUES (new.id, new.text);
END;

CREATE VIRTUAL TABLE session_fts USING fts5(
    title,
    content = 'session',
    content_rowid = 'pk',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER session_fts_insert AFTER INSERT ON session BEGIN
    INSERT INTO session_fts (rowid, title) VALUES (new.pk, new.title);
END;
CREATE TRIGGER session_fts_delete AFTER DELETE ON session BEGIN
    INSERT INTO session_fts (session_fts, rowid, title) VALUES ('delete', old.pk, old.title);
END;
CREATE TRIGGER session_fts_update AFTER UPDATE OF title ON session
WHEN old.title IS NOT new.title BEGIN
    INSERT INTO session_fts (session_fts, rowid, title) VALUES ('delete', old.pk, old.title);
    INSERT INTO session_fts (rowid, title) VALUES (new.pk, new.title);
END;
"#;

/// Version 2: the roots of projects somebody removed, so that discovering
/// projects from session history does not bring them back.
const V2: &str = r#"
CREATE TABLE dismissed_root (
    machine_id TEXT NOT NULL REFERENCES machine(id) ON DELETE CASCADE,
    root       TEXT NOT NULL,
    PRIMARY KEY (machine_id, root)
) WITHOUT ROWID;
"#;

/// Version 3: a logo per project. `origin` is `detected` (written by the
/// engine) or `custom` (the user's own file); the image bytes are kept next to
/// their content hash.
const V3: &str = r#"
CREATE TABLE project_icon (
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    origin     TEXT NOT NULL,
    kind       TEXT NOT NULL,
    source     TEXT NOT NULL,
    format     TEXT,
    hash       TEXT,
    image      BLOB,
    remote     TEXT,
    set_at     INTEGER NOT NULL,
    PRIMARY KEY (project_id, origin)
) WITHOUT ROWID;

CREATE INDEX project_icon_by_hash ON project_icon (hash);
"#;

/// Brings the database up to the latest schema version.
pub(crate) fn migrate(connection: &mut Connection) -> Result<()> {
    let supported = MIGRATIONS.len() as u32;
    let found: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if found > supported {
        return Err(StoreError::SchemaTooNew { found, supported });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let version = index as u32 + 1;
        let transaction = connection.transaction()?;
        transaction.execute_batch(sql)?;
        transaction.pragma_update(None, "user_version", version)?;
        transaction.commit()?;
        tracing::debug!(version, "applied store migration");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(connection: &Connection) -> u32 {
        connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn migrating_an_empty_database_reaches_the_latest_version() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(version(&connection), MIGRATIONS.len() as u32);
    }

    #[test]
    fn a_version_1_database_gains_the_dismissed_roots_table_and_keeps_its_data() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(V1).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        connection
            .execute(
                "INSERT INTO project (id, machine_id, name, root) VALUES ('p', 'local', 'api', '/srv/api')",
                [],
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        assert_eq!(version(&connection), MIGRATIONS.len() as u32);
        let projects: i64 = connection
            .query_row("SELECT count(*) FROM project", [], |row| row.get(0))
            .unwrap();
        let dismissed: i64 = connection
            .query_row("SELECT count(*) FROM dismissed_root", [], |row| row.get(0))
            .unwrap();
        assert_eq!((projects, dismissed), (1, 0));
    }

    #[test]
    fn a_version_2_database_gains_the_icon_table_and_keeps_its_projects() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(V1).unwrap();
        connection.execute_batch(V2).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        connection
            .execute(
                "INSERT INTO project (id, machine_id, name, root) VALUES ('p', 'local', 'api', '/srv/api')",
                [],
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        assert_eq!(version(&connection), 3);
        connection
            .execute(
                "INSERT INTO project_icon (project_id, origin, kind, source, set_at)
                 VALUES ('p', 'detected', 'folder', '', 0)",
                [],
            )
            .unwrap();
        let projects: i64 = connection
            .query_row("SELECT count(*) FROM project", [], |row| row.get(0))
            .unwrap();
        assert_eq!(projects, 1);
    }

    #[test]
    fn migrating_twice_changes_nothing() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        migrate(&mut connection).unwrap();
        let machines: i64 = connection
            .query_row("SELECT count(*) FROM machine", [], |row| row.get(0))
            .unwrap();
        assert_eq!(machines, 1);
    }

    #[test]
    fn a_database_from_a_newer_version_is_refused() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "user_version", 9_999)
            .unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(StoreError::SchemaTooNew { found: 9_999, .. })
        ));
    }
}

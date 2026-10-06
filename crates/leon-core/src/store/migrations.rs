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
///
/// Versions 4 (usage limits) and 5 (relay machines) were developed on separate
/// branches that both wanted "version 4". Neither was ever released: the only
/// public schema is version 3, so version 4 never existed in the wild and this
/// order (usage, then relay) is the one every database goes through.
const MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11, V12];

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

/// Version 4: usage limits. `usage_reading` keeps the latest reading of each
/// agent on each machine (one JSON document, so the model can grow without a
/// migration); `usage_history` keeps bounded observations, only percentages
/// and times, under a local hash of the account and never an identifier.
const V4: &str = r#"
CREATE TABLE usage_reading (
    machine_id   TEXT NOT NULL REFERENCES machine(id) ON DELETE CASCADE,
    agent        TEXT NOT NULL,
    payload      TEXT NOT NULL,
    collected_at INTEGER NOT NULL,
    PRIMARY KEY (machine_id, agent)
) WITHOUT ROWID;

CREATE TABLE usage_history (
    machine_id   TEXT NOT NULL REFERENCES machine(id) ON DELETE CASCADE,
    agent        TEXT NOT NULL,
    account      TEXT NOT NULL,
    window       TEXT NOT NULL,
    observed_at  INTEGER NOT NULL,
    used_percent REAL NOT NULL,
    PRIMARY KEY (machine_id, agent, account, window, observed_at)
) WITHOUT ROWID;
"#;

/// Version 5: machines reached through a relay: the pinned host key, the relay's address
/// and the name the host gave itself. Existing rows are untouched (the columns
/// are NULL for them); the host's id lives in the existing `host` column.
const V5: &str = r#"
ALTER TABLE machine ADD COLUMN relay_host_key TEXT;
ALTER TABLE machine ADD COLUMN relay_url TEXT;
ALTER TABLE machine ADD COLUMN relay_name TEXT;
"#;

/// Version 6: no schema change. Its step (in Rust, below) merges the projects
/// and worktrees that differ only by how their path is spelled (`C:/code/api`
/// from git, `c:\code\api` from an agent) and links the sessions again.
const V6: &str = "SELECT 1;";

/// Version 7: what the last history import of each agent did, for the history
/// diagnosis (when it ran, how long it took, how it ended). One row per agent
/// and machine, replaced by every run; counts and times only.
const V7: &str = r#"
CREATE TABLE import_run (
    machine_id  TEXT NOT NULL REFERENCES machine(id) ON DELETE CASCADE,
    agent       TEXT NOT NULL,
    at          INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    scanned     INTEGER NOT NULL,
    imported    INTEGER NOT NULL,
    unchanged   INTEGER NOT NULL,
    empty       INTEGER NOT NULL,
    failed      INTEGER NOT NULL,
    malformed   INTEGER NOT NULL,
    unsupported INTEGER NOT NULL,
    PRIMARY KEY (machine_id, agent)
) WITHOUT ROWID;
"#;

/// Version 8: the open terminals, remembered for the next start (see
/// `workspace.rs`). Two slots (current, previous); one small row per
/// terminal, tab and workspace; no scrollback.
const V8: &str = r#"
CREATE TABLE saved_meta (
    slot     INTEGER PRIMARY KEY,
    saved_at INTEGER NOT NULL,
    clean    INTEGER NOT NULL,
    selection TEXT,
    main     INTEGER
);
CREATE TABLE saved_workspace (
    slot     INTEGER NOT NULL,
    position INTEGER NOT NULL,
    key      TEXT NOT NULL,
    active   INTEGER NOT NULL,
    PRIMARY KEY (slot, position)
) WITHOUT ROWID;
CREATE TABLE saved_tab (
    slot      INTEGER NOT NULL,
    workspace INTEGER NOT NULL,
    position  INTEGER NOT NULL,
    layout    TEXT NOT NULL,
    focus     INTEGER NOT NULL,
    zoomed    INTEGER NOT NULL,
    PRIMARY KEY (slot, workspace, position)
) WITHOUT ROWID;
CREATE TABLE saved_terminal (
    slot       INTEGER NOT NULL,
    position   INTEGER NOT NULL,
    id         INTEGER NOT NULL,
    machine    TEXT NOT NULL,
    cwd        TEXT NOT NULL,
    agent      TEXT,
    session    TEXT,
    confidence TEXT,
    history    TEXT,
    name       TEXT,
    title      TEXT,
    started_at INTEGER NOT NULL,
    PRIMARY KEY (slot, position)
) WITHOUT ROWID;
"#;

/// Version 9: the order of the projects in the sidebar. `sort_order` is the
/// position of the project among the projects of its machine, lowest first.
/// Existing projects all start at zero, so they keep their alphabetical order
/// until somebody drags one; new projects go after the greatest position of
/// their machine.
const V9: &str = r#"
ALTER TABLE project ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
UPDATE project SET sort_order = (
    SELECT COUNT(*) FROM project AS other
    WHERE other.machine_id = project.machine_id
      AND (other.name < project.name
           OR (other.name = project.name AND other.id < project.id))
);
"#;

/// Version 10: the order of the worktrees in the sidebar, like the projects'.
/// The main worktree keeps its first place until somebody drags one; new
/// worktrees go last.
const V10: &str = r#"
ALTER TABLE worktree ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
UPDATE worktree SET sort_order = (
    SELECT COUNT(*) FROM worktree AS other
    WHERE other.project_id = worktree.project_id
      AND (other.is_main > worktree.is_main
           OR (other.is_main = worktree.is_main
               AND (other.path < worktree.path
                    OR (other.path = worktree.path AND other.id < worktree.id))))
);
"#;

/// Version 11: pinned sessions. `sort_order` holds the pinned position inside
/// the parent's list, lowest first; `NULL` means automatic (by recency), as
/// before. Every existing session starts unpinned, so nothing moves.
const V11: &str = r#"
ALTER TABLE session ADD COLUMN sort_order INTEGER;
"#;

/// Version 12: what is known about each worktree being merged. Both columns
/// hold NULL until a probe says otherwise, so a worktree is "not known" rather
/// than "not merged" until then, and no existing worktree moves.
const V12: &str = r#"
ALTER TABLE worktree ADD COLUMN merged_branch INTEGER;
ALTER TABLE worktree ADD COLUMN merged_pull_request INTEGER;
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
        if version == 6 {
            super::projects::heal_path_spellings(&transaction)?;
        }
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
    fn a_version_3_database_gains_the_relay_columns_and_keeps_its_machines() {
        let mut connection = Connection::open_in_memory().unwrap();
        for (index, sql) in [V1, V2, V3].iter().enumerate() {
            connection.execute_batch(sql).unwrap();
            connection
                .pragma_update(None, "user_version", index as u32 + 1)
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO machine (id, name, kind, host) VALUES ('m1', 'box', 'ssh', 'box.example')",
                [],
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(version(&connection), MIGRATIONS.len() as u32);
        let (host, key): (String, Option<String>) = connection
            .query_row(
                "SELECT host, relay_host_key FROM machine WHERE id = 'm1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((host.as_str(), key), ("box.example", None));
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

        assert_eq!(version(&connection), MIGRATIONS.len() as u32);
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
    fn a_version_3_database_gains_the_usage_tables() {
        let mut connection = Connection::open_in_memory().unwrap();
        for sql in [V1, V2, V3] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 3).unwrap();

        migrate(&mut connection).unwrap();

        assert_eq!(version(&connection), MIGRATIONS.len() as u32);
        connection
            .execute(
                "INSERT INTO usage_reading (machine_id, agent, payload, collected_at)
                 VALUES ('local', 'codex', '{}', 0)",
                [],
            )
            .unwrap();
    }

    #[test]
    fn a_fresh_database_has_both_the_usage_tables_and_the_relay_columns() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(version(&connection), MIGRATIONS.len() as u32);
        connection
            .execute(
                "INSERT INTO usage_history (machine_id, agent, account, window, observed_at, used_percent)
                 VALUES ('local', 'codex', 'a', 'w', 0, 1.0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO machine (id, name, kind, host, relay_host_key, relay_url, relay_name)
                 VALUES ('r', 'box', 'relay', 'h', 'k', 'wss://x', 'n')",
                [],
            )
            .unwrap();
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

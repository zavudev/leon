//! The local SQLite store.
//!
//! One [`Store`] is created at start-up and shared as `Arc<Store>` by every
//! part of the application. Its API is synchronous: each call is a short,
//! indexed query, so callers on an async runtime run it on a blocking thread
//! (or directly, for the cheap lookups) instead of the store owning a runtime.
//!
//! A file-backed store keeps two connections to the same database in WAL
//! mode: a writer and a read-only reader. Reads therefore never wait for a
//! long-running write such as a history import. An in-memory store has a
//! single connection that serves both roles, because a second connection to
//! `:memory:` would see a different, empty database.
//!
//! Every successful write announces the area it touched on a broadcast
//! channel; see [`StoreChange`].

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use tokio::sync::broadcast;

use crate::change::{ChangeListener, StoreChange, CHANGE_CAPACITY};
use crate::error::Result;

mod history;
mod icons;
mod machines;
mod migrations;
mod projects;
pub mod search;
mod usage;
mod workspace;

pub use history::{
    history_overview_at, import_cursors_at, set_import_cursor_in, upsert_session_in, AgentOverview,
    ImportRun, SessionFilter,
};
pub use usage::{UsagePoint, UsageRow};
pub use workspace::{SavedLayout, SavedState, SavedTab, SavedTerminal, SavedWorkspace, Slot};

/// How long a connection waits for a lock held by another connection before
/// giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// How many compiled statements each connection keeps ready for reuse.
const STATEMENT_CACHE_CAPACITY: usize = 64;

/// Handle to Leon's local database. See the [module documentation](self).
#[derive(Debug)]
pub struct Store {
    writer: Mutex<Connection>,
    reader: Option<Mutex<Connection>>,
    changes: broadcast::Sender<StoreChange>,
}

impl Store {
    /// Opens the database at `path`, creating and migrating it as needed.
    pub fn open(path: impl AsRef<Path>) -> Result<Arc<Self>> {
        let path = path.as_ref();
        let mut writer = Connection::open(path)?;
        configure(&writer)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        writer.pragma_update(None, "synchronous", "NORMAL")?;
        migrations::migrate(&mut writer)?;

        let reader = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        configure(&reader)?;
        reader.pragma_update(None, "query_only", true)?;

        Ok(Arc::new(Self::assemble(writer, Some(reader))))
    }

    /// Opens a private database that lives only as long as the store. Meant
    /// for tests and previews.
    pub fn open_in_memory() -> Result<Arc<Self>> {
        let mut writer = Connection::open_in_memory()?;
        configure(&writer)?;
        migrations::migrate(&mut writer)?;
        Ok(Arc::new(Self::assemble(writer, None)))
    }

    fn assemble(writer: Connection, reader: Option<Connection>) -> Self {
        let (changes, _) = broadcast::channel(CHANGE_CAPACITY);
        Self {
            writer: Mutex::new(writer),
            reader: reader.map(Mutex::new),
            changes,
        }
    }

    /// Runs a read-only closure against the database.
    ///
    /// The closure must not block on anything but the database, and must not
    /// call back into the store: in an in-memory store reads and writes share
    /// one connection and a nested call would deadlock.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let connection = lock(self.reader.as_ref().unwrap_or(&self.writer));
        f(&connection)
    }

    /// Runs a closure inside a write transaction and, when it succeeds,
    /// commits and announces `change` to every listener. When the closure
    /// fails, nothing is written and nothing is announced.
    ///
    /// The same re-entrancy rule as [`Store::read`] applies.
    pub fn write<T>(
        &self,
        change: StoreChange,
        f: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let value = self.transact(f)?;
        self.notify(change);
        Ok(value)
    }

    /// Runs a write transaction without announcing anything. For bookkeeping
    /// no listener can observe, and for writes that decide afterwards which
    /// areas they touched.
    pub(crate) fn transact<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut connection = lock(&self.writer);
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }

    /// Subscribes to change notifications for writes made from now on.
    pub fn subscribe(&self) -> ChangeListener {
        ChangeListener::new(self.changes.subscribe())
    }

    pub(crate) fn notify(&self, change: StoreChange) {
        // Sending fails only when nobody is listening, which is fine.
        let _ = self.changes.send(change);
    }
}

/// Applies the settings every connection needs.
fn configure(connection: &Connection) -> Result<()> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.set_prepared_statement_cache_capacity(STATEMENT_CACHE_CAPACITY);
    Ok(())
}

/// Locks a connection, recovering it if a previous holder panicked. A
/// connection is left in a consistent state by a panic because the open
/// transaction is rolled back when its guard is dropped.
fn lock(connection: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    connection.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Converts a timestamp to the integer the database stores: milliseconds
/// since the Unix epoch, in UTC.
pub(crate) fn to_millis(at: DateTime<Utc>) -> i64 {
    at.timestamp_millis()
}

/// Converts a stored integer back to a timestamp.
pub(crate) fn from_millis(millis: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(millis).unwrap_or(DateTime::UNIX_EPOCH)
}

/// Builds the error for a row whose text tag is not one the model knows.
pub(crate) fn bad_tag(column: usize, tag: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        format!("unknown tag {tag:?}").into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StoreError;
    use crate::ids::MachineId;
    use crate::model::MachineKind;

    fn ssh(host: &str) -> MachineKind {
        MachineKind::Ssh {
            host: host.into(),
            user: None,
            port: None,
            identity_file: None,
        }
    }

    #[test]
    fn a_new_store_contains_the_local_machine() {
        let store = Store::open_in_memory().unwrap();
        let machines = store.machines().unwrap();
        assert_eq!(machines.len(), 1);
        assert_eq!(machines[0].id, MachineId::local());
        assert_eq!(machines[0].kind, MachineKind::Local);
    }

    #[test]
    fn data_written_to_a_file_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leon.db");
        let added = {
            let store = Store::open(&path).unwrap();
            store
                .add_machine("build box", ssh("build.example"))
                .unwrap()
        };
        let store = Store::open(&path).unwrap();
        assert_eq!(store.machine(&added.id).unwrap(), added);
        assert_eq!(store.machines().unwrap().len(), 2);
    }

    #[test]
    fn a_file_store_uses_write_ahead_logging() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("leon.db")).unwrap();
        let mode: String = store
            .read(|c| Ok(c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn the_reader_connection_refuses_to_write() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("leon.db")).unwrap();
        let outcome = store.read(|c| Ok(c.execute("DELETE FROM machine", [])?));
        assert!(matches!(outcome, Err(StoreError::Sqlite(_))));
        assert_eq!(store.machines().unwrap().len(), 1);
    }

    #[test]
    fn a_failed_write_changes_nothing_and_announces_nothing() {
        let store = Store::open_in_memory().unwrap();
        let mut listener = store.subscribe();
        let outcome: Result<()> = store.write(StoreChange::Machines, |tx| {
            tx.execute("DELETE FROM machine", [])?;
            Err(StoreError::Invalid("stop".into()))
        });
        assert!(outcome.is_err());
        assert_eq!(store.machines().unwrap().len(), 1);
        assert_eq!(listener.try_next(), None);
    }

    #[test]
    fn a_successful_write_announces_its_area() {
        let store = Store::open_in_memory().unwrap();
        let mut listener = store.subscribe();
        store
            .add_machine("build box", ssh("build.example"))
            .unwrap();
        assert_eq!(listener.try_next(), Some(StoreChange::Machines));
        assert_eq!(listener.try_next(), None);
    }

    #[tokio::test]
    async fn a_listener_is_woken_by_a_write_from_another_thread() {
        let store = Store::open_in_memory().unwrap();
        let mut listener = store.subscribe();
        let writer = Arc::clone(&store);
        std::thread::spawn(move || writer.add_machine("box", ssh("box.example")).unwrap())
            .join()
            .unwrap();
        assert_eq!(listener.next().await, Some(StoreChange::Machines));
    }

    #[test]
    fn the_bundled_sqlite_supports_full_text_search() {
        let store = Store::open_in_memory().unwrap();
        let hits: i64 = store
            .write(StoreChange::Everything, |tx| {
                tx.execute_batch(
                    "CREATE VIRTUAL TABLE probe USING fts5(body);
                     INSERT INTO probe(body) VALUES ('quick brown fox');",
                )?;
                Ok(tx.query_row(
                    "SELECT count(*) FROM probe WHERE probe MATCH 'brown'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(hits, 1);
    }

    #[test]
    fn timestamps_round_trip_through_milliseconds() {
        let at = DateTime::from_timestamp_millis(1_700_000_000_123).unwrap();
        assert_eq!(from_millis(to_millis(at)), at);
    }
}

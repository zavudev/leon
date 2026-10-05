//! The error type returned by every store operation.
//!
//! Callers mostly need to tell three situations apart: the database itself
//! failed, the thing they asked for does not exist, or the request was not
//! acceptable. Each has its own variant so the application can react without
//! parsing messages.

use thiserror::Error;

/// Result alias used throughout the store.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// Everything that can go wrong while talking to the local store.
#[derive(Debug, Error)]
pub enum StoreError {
    /// SQLite reported an error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// The requested entity does not exist. The payload names the kind of
    /// entity, for example `"project"`.
    #[error("{0} not found")]
    NotFound(&'static str),

    /// The request was rejected before touching the database.
    #[error("invalid request: {0}")]
    Invalid(String),

    /// The database was written by a newer version of Leon than this one.
    #[error("database schema version {found} is newer than the supported version {supported}")]
    SchemaTooNew {
        /// Version recorded in the database file.
        found: u32,
        /// Highest version this build knows how to read.
        supported: u32,
    },

    /// A row held a value the model cannot represent.
    #[error("corrupt row: {0}")]
    Corrupt(String),
}

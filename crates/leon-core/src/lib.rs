//! Leon's core: the domain model and the local store every other crate builds on.
//!
//! Leon orchestrates coding agents across machines. This crate owns the
//! vocabulary shared by the whole product (machines, projects, worktrees,
//! agent sessions and their messages) and the SQLite-backed [`Store`] that
//! persists it, including the full-text index behind the unified session
//! history.
//!
//! The crate is deliberately free of any UI or networking dependency so that
//! the desktop application, the history importers and the remote execution
//! layer can all depend on it without pulling each other in.

#![warn(missing_docs)]

mod change;
mod error;
pub mod icon;
mod ids;
mod model;
pub mod store;

pub use change::{ChangeListener, StoreChange};
pub use error::{Result, StoreError};
pub use icon::{IconFormat, IconImage, IconKind, NewIcon, ProjectIcon};
pub use ids::{MachineId, ProjectId, SessionId, WorktreeId};
pub use model::{
    AgentKind, Machine, MachineKind, Message, NewMessage, NewSession, NewWorktree, Project, Role,
    Session, Worktree,
};
pub use store::search::{
    fts_query, snippet_segments, SearchHit, SearchQuery, SNIPPET_ELLIPSIS, SNIPPET_END,
    SNIPPET_START,
};
pub use store::{set_import_cursor_in, upsert_session_in, SessionFilter, Store};

/// The SQLite binding the store is built on, re-exported so that code using
/// [`Store::read`] and [`Store::write`] names the same version of its types.
pub use rusqlite;

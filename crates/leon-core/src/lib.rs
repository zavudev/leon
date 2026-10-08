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

pub mod account;
pub mod agent;
mod change;
pub mod cleanup;
mod error;
pub mod icon;
mod ids;
mod model;
pub mod path;
mod status;
pub mod store;

pub use account::Account;
pub use agent::{AgentSpec, CustomAgent};
pub use change::{ChangeListener, StoreChange};
pub use error::{Result, StoreError};
pub use icon::{IconFormat, IconImage, IconKind, NewIcon, ProjectIcon};
pub use ids::{MachineId, ProjectId, SessionId, WorktreeId};
pub use model::{
    AgentId, Machine, MachineKind, Message, NewMessage, NewSession, NewWorktree, Project, Role,
    Session, SessionScope, Worktree,
};
pub use status::{Checks, Divergence, PullRequest, Review, WorktreeStatus};
pub use store::search::{
    fts_query, snippet_segments, SearchHit, SearchQuery, SNIPPET_ELLIPSIS, SNIPPET_END,
    SNIPPET_START,
};
pub use store::{
    history_overview_at, import_cursors_at, set_import_cursor_in, set_session_account_in,
    set_session_tokens_in, upsert_session_in, AgentOverview, ImportRun, KeeperRef, SavedLayout,
    SavedState, SavedTab, SavedTerminal, SavedWorkspace, SessionFilter, SessionTokens, Shelf, Slot,
    Store, TokenCounts, TokenRow, UsagePoint, UsageRow,
};

/// The SQLite binding the store is built on, re-exported so that code using
/// [`Store::read`] and [`Store::write`] names the same version of its types.
pub use rusqlite;

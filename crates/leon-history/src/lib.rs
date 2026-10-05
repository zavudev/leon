//! Importers that feed agent transcripts into Leon's unified history.
//!
//! Each supported agent keeps its sessions in its own format: Claude Code and
//! Codex write one JSON-lines file per session, opencode keeps everything in a
//! SQLite database. This crate reads those formats and normalises them into
//! the session and message shapes of [`leon_core`], so the rest of Leon sees a
//! single searchable history.
//!
//! The crate is layered so the same code can later serve remote machines:
//!
//! * **Parsers** ([`claude`], [`codex`], [`opencode`]) are pure: bytes or
//!   database rows in, a [`ParsedSession`] out. They never touch the store and
//!   never fail on unexpected input; anything they do not understand is
//!   skipped and counted.
//! * **Sources** ([`HistorySource`]) enumerate what can be imported and load
//!   one item at a time. The implementations here read the local file system;
//!   a source backed by files fetched over SSH can reuse the parsers
//!   unchanged.
//! * The **[`Importer`]** drives sources into a [`leon_core::Store`],
//!   skipping whatever has not changed since the previous run.

#![warn(missing_docs)]

pub mod claude;
pub mod codex;
mod importer;
mod normalize;
pub mod opencode;
mod roots;
mod session;
mod source;

pub use importer::{ImportReport, Importer};
pub use roots::{default_roots, default_roots_in, HistoryRoots};
pub use session::ParsedSession;
pub use source::{ClaudeFiles, CodexFiles, HistoryError, HistorySource, OpencodeDb, SourceItem};

//! The memory agents share in Leon, without a window and without a process.
//!
//! Leon keeps what agents decide, agree on and find out in its store
//! (`leon_core::store::memory`), by project or for every project. This crate
//! holds everything that can be decided about it without touching a disk:
//!
//! * [`render`]: the memory file, one compact markdown document an agent reads
//!   before it starts, within a hard size budget; and how an entry and a search
//!   hit read as plain text.
//! * [`block`]: the managed block of a project's instruction files, the few
//!   lines that tell an agent the memory exists. Inserted, replaced and removed
//!   with everything else left byte for byte as it was.
//! * [`files`]: which instruction files there are (`AGENTS.md`, `CLAUDE.md`,
//!   `GEMINI.md`) and which of them the block goes to.
//! * [`mcp`]: the MCP server, a JSON-RPC request in and a response out over a
//!   small trait for the store.
//! * [`mcp_json`]: the `leon-memory` entry of a project's `.mcp.json`.
//! * [`scope`]: which project a folder belongs to when the store does not know
//!   it (git's own files say), and the name of a project's memory file.
//!
//! The application does the reading and the writing (`crates/app/src/memory`).
//! See `docs/MEMORY.md`.

#![warn(missing_docs)]

pub mod block;
pub mod files;
pub mod mcp;
pub mod mcp_json;
pub mod render;
pub mod scope;

/// The environment variable that holds the path of the memory file of a
/// terminal's folder.
pub const ENV_MEMORY: &str = "LEON_MEMORY";
/// The environment variable that holds the path of the running Leon.
pub const ENV_BIN: &str = "LEON_BIN";
/// The environment variable that holds Leon's data folder, set when it is not
/// the platform's.
pub const ENV_DATA_DIR: &str = "LEON_DATA_DIR";

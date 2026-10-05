//! The seam between "where transcripts come from" and "how they are parsed".
//!
//! A [`HistorySource`] enumerates importable items, each with a stable key
//! and a cheap fingerprint, and loads one item into a [`ParsedSession`]. The
//! importer needs nothing else, so it does not care whether an item is a file
//! on this machine, a session inside another tool's database, or (in a later
//! unit) a file fetched from a remote machine: such a source lists remote
//! files with their size and modification time and hands the fetched bytes to
//! the same parsers used here.
//!
//! Sources are synchronous and may block on I/O; callers on an async runtime
//! run an import on a blocking thread.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use leon_core::AgentKind;
use rusqlite::Connection;
use thiserror::Error;

use crate::session::ParsedSession;
use crate::{claude, codex, opencode};

/// Why a source could not be listed or an item could not be loaded.
#[derive(Debug, Error)]
pub enum HistoryError {
    /// Reading a file or directory failed.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The path that could not be read.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// Reading another tool's database failed.
    #[error("cannot read database {path}: {source}")]
    Database {
        /// The database file.
        path: String,
        /// The underlying error.
        source: rusqlite::Error,
    },
}

/// One importable unit: a transcript file or a session inside a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceItem {
    /// Identifies the item across runs. It is stored as the import cursor's
    /// source key, so it must be stable and unique per machine. By convention
    /// it starts with the agent tag, for example `claude:/home/u/.../x.jsonl`.
    pub key: String,
    /// Changes whenever the item's content may have changed. An item whose
    /// fingerprint equals the stored one is skipped without being loaded.
    pub fingerprint: String,
    /// What the source needs to load the item: a path or a session id.
    pub locator: String,
}

/// A place sessions of one agent can be imported from.
pub trait HistorySource: Send + Sync {
    /// The agent whose sessions this source provides.
    fn agent(&self) -> AgentKind;

    /// Enumerates the items currently available. A source whose root does
    /// not exist lists nothing; that is not an error.
    fn list(&self) -> Result<Vec<SourceItem>, HistoryError>;

    /// Loads and parses one item. `Ok(None)` means the item was readable but
    /// holds no messages.
    fn load(&self, item: &SourceItem) -> Result<Option<ParsedSession>, HistoryError>;
}

/// Claude Code transcripts on the local file system.
///
/// The root is Claude Code's `projects` directory. Session files sit exactly
/// one directory below it, `<root>/<project>/<session-id>.jsonl`; deeper
/// files belong to sub-agents and are left alone.
#[derive(Debug, Clone)]
pub struct ClaudeFiles {
    root: PathBuf,
}

impl ClaudeFiles {
    /// A source reading the `projects` directory at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl HistorySource for ClaudeFiles {
    fn agent(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn list(&self) -> Result<Vec<SourceItem>, HistoryError> {
        let mut items = Vec::new();
        for project in read_dir(&self.root)? {
            if project.is_dir() {
                for file in read_dir(&project)? {
                    if has_extension(&file, "jsonl") {
                        items.extend(file_item(AgentKind::Claude, &file));
                    }
                }
            }
        }
        items.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(items)
    }

    fn load(&self, item: &SourceItem) -> Result<Option<ParsedSession>, HistoryError> {
        let path = Path::new(&item.locator);
        let bytes = read_file(path)?;
        Ok(claude::parse_session(&file_stem(path), &bytes))
    }
}

/// Codex rollout files on the local file system.
///
/// The root is Codex's `sessions` directory. Rollouts are found at any depth
/// below it (Codex nests them by year, month and day) and are recognised by
/// their `rollout-*.jsonl` name.
#[derive(Debug, Clone)]
pub struct CodexFiles {
    root: PathBuf,
}

impl CodexFiles {
    /// A source reading the `sessions` directory at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl HistorySource for CodexFiles {
    fn agent(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn list(&self) -> Result<Vec<SourceItem>, HistoryError> {
        let mut items = Vec::new();
        let mut pending = vec![(self.root.clone(), 0usize)];
        while let Some((directory, depth)) = pending.pop() {
            for entry in read_dir(&directory)? {
                if entry.is_dir() {
                    if depth < MAX_ROLLOUT_DEPTH {
                        pending.push((entry, depth + 1));
                    }
                } else if has_extension(&entry, "jsonl")
                    && file_stem(&entry).starts_with("rollout-")
                {
                    items.extend(file_item(AgentKind::Codex, &entry));
                }
            }
        }
        items.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(items)
    }

    fn load(&self, item: &SourceItem) -> Result<Option<ParsedSession>, HistoryError> {
        let path = Path::new(&item.locator);
        let bytes = read_file(path)?;
        Ok(codex::parse_session(&rollout_id(&file_stem(path)), &bytes))
    }
}

/// How deep below the sessions root rollouts are looked for. The layout needs
/// three levels; the margin guards against odd layouts without following a
/// directory cycle forever.
const MAX_ROLLOUT_DEPTH: usize = 6;

/// The opencode database on the local file system. See [`opencode`] for the
/// format and the read-only guarantees.
#[derive(Debug)]
pub struct OpencodeDb {
    path: PathBuf,
    connection: Mutex<Option<Connection>>,
}

impl OpencodeDb {
    /// A source reading the opencode database file at `path`. The file is
    /// opened on first use.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            connection: Mutex::new(None),
        }
    }

    fn with_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, HistoryError> {
        let wrap = |source| HistoryError::Database {
            path: self.path.display().to_string(),
            source,
        };
        let mut guard = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            *guard = Some(opencode::open_read_only(&self.path).map_err(wrap)?);
        }
        let connection = guard.as_ref().expect("connection was just opened");
        f(connection).map_err(wrap)
    }
}

impl HistorySource for OpencodeDb {
    fn agent(&self) -> AgentKind {
        AgentKind::Opencode
    }

    fn list(&self) -> Result<Vec<SourceItem>, HistoryError> {
        if !self.path.is_file() {
            return Ok(Vec::new());
        }
        let sessions = self.with_connection(opencode::list_sessions)?;
        let database = self.path.display();
        Ok(sessions
            .into_iter()
            .map(|session| SourceItem {
                key: format!("opencode:{database}#{}", session.id),
                fingerprint: session.fingerprint,
                locator: session.id,
            })
            .collect())
    }

    fn load(&self, item: &SourceItem) -> Result<Option<ParsedSession>, HistoryError> {
        self.with_connection(|connection| opencode::read_session(connection, &item.locator))
    }
}

/// The entries of a directory. A directory that does not exist has none.
fn read_dir(directory: &Path) -> Result<Vec<PathBuf>, HistoryError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io_error(directory, source)),
    };
    // An entry that vanishes or cannot be inspected while listing is skipped:
    // agents create and rotate these files while Leon is reading.
    Ok(entries.flatten().map(|entry| entry.path()).collect())
}

fn read_file(path: &Path) -> Result<Vec<u8>, HistoryError> {
    fs::read(path).map_err(|source| io_error(path, source))
}

fn io_error(path: &Path, source: std::io::Error) -> HistoryError {
    HistoryError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|found| found == extension)
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Describes a transcript file as a source item. The fingerprint is the
/// file's size and modification time, which is enough to notice appends and
/// rewrites without reading the file. A file that cannot be inspected is left
/// out.
fn file_item(agent: AgentKind, path: &Path) -> Option<SourceItem> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis());
    let locator = path.to_string_lossy().into_owned();
    Some(SourceItem {
        key: format!("{}:{locator}", agent.as_str()),
        fingerprint: format!("{}:{modified}", metadata.len()),
        locator,
    })
}

/// Extracts the session id from a rollout file stem of the form
/// `rollout-<timestamp>-<uuid>`. A stem that does not end in a UUID is
/// returned whole.
fn rollout_id(stem: &str) -> String {
    const UUID_LENGTH: usize = 36;
    let candidate = stem
        .len()
        .checked_sub(UUID_LENGTH)
        .and_then(|start| stem.get(start..));
    match candidate {
        Some(tail)
            if tail.split('-').map(str::len).eq([8, 4, 4, 4, 12])
                && tail.chars().all(|c| c.is_ascii_hexdigit() || c == '-') =>
        {
            tail.to_owned()
        }
        _ => stem.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER_LINE: &str = r#"{"type":"user","cwd":"/srv/api","timestamp":"2026-03-01T10:00:00Z","message":{"role":"user","content":"hello"}}"#;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn claude_sessions_are_found_one_level_below_the_root_only() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("project-a/session-1.jsonl"), USER_LINE);
        write(&root.path().join("project-b/session-2.jsonl"), USER_LINE);
        write(&root.path().join("project-a/notes.txt"), "not a transcript");
        write(&root.path().join("stray.jsonl"), USER_LINE);
        write(
            &root
                .path()
                .join("project-a/session-1/subagents/agent-x.jsonl"),
            USER_LINE,
        );

        let source = ClaudeFiles::new(root.path());
        let items = source.list().unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.key.starts_with("claude:")));

        let session = source.load(&items[0]).unwrap().unwrap();
        assert_eq!(session.external_id, "session-1");
        assert_eq!(session.messages.len(), 1);
    }

    #[test]
    fn a_missing_root_lists_nothing() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("does-not-exist");
        assert!(ClaudeFiles::new(&missing).list().unwrap().is_empty());
        assert!(CodexFiles::new(&missing).list().unwrap().is_empty());
        assert!(OpencodeDb::new(missing.join("opencode.db"))
            .list()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_file_fingerprint_changes_when_the_file_grows() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("project/session.jsonl");
        write(&path, USER_LINE);
        let source = ClaudeFiles::new(root.path());
        let before = source.list().unwrap();
        assert_eq!(source.list().unwrap(), before);

        write(&path, &format!("{USER_LINE}\n{USER_LINE}\n"));
        let after = source.list().unwrap();
        assert_eq!(after[0].key, before[0].key);
        assert_ne!(after[0].fingerprint, before[0].fingerprint);
    }

    #[test]
    fn codex_rollouts_are_found_at_any_depth_and_identified_by_file_name() {
        let root = tempfile::tempdir().unwrap();
        let id = "0198c0de-aaaa-7bbb-8ccc-0123456789ab";
        let rollout = r#"{"timestamp":"2026-03-01T10:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#;
        write(
            &root
                .path()
                .join(format!("2026/03/01/rollout-2026-03-01T10-00-00-{id}.jsonl")),
            rollout,
        );
        write(&root.path().join("2026/03/01/other.jsonl"), rollout);
        write(&root.path().join("rollout-loose.jsonl"), rollout);

        let source = CodexFiles::new(root.path());
        let items = source.list().unwrap();
        assert_eq!(items.len(), 2);

        let ids: Vec<_> = items
            .iter()
            .map(|item| source.load(item).unwrap().unwrap().external_id)
            .collect();
        assert!(ids.contains(&id.to_owned()));
        assert!(ids.contains(&"rollout-loose".to_owned()));
    }

    #[test]
    fn a_rollout_id_is_the_trailing_uuid_of_the_file_name() {
        assert_eq!(
            rollout_id("rollout-2026-03-01T10-00-00-0198c0de-aaaa-7bbb-8ccc-0123456789ab"),
            "0198c0de-aaaa-7bbb-8ccc-0123456789ab"
        );
        assert_eq!(rollout_id("rollout-short"), "rollout-short");
        assert_eq!(
            rollout_id("rollout-2026-03-01T10-00-00-not-a-uuid-at-all-really-nope"),
            "rollout-2026-03-01T10-00-00-not-a-uuid-at-all-really-nope"
        );
        assert_eq!(
            rollout_id("rollout-ééééééééééééééééééééééé"),
            "rollout-ééééééééééééééééééééééé"
        );
    }

    #[test]
    fn opencode_sessions_are_listed_and_loaded_from_the_database_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        opencode::fixture::populate(&Connection::open(&path).unwrap());

        let source = OpencodeDb::new(&path);
        let items = source.list().unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].key.starts_with("opencode:"));
        assert!(items[0].key.ends_with("#ses_1"));
        let session = source.load(&items[0]).unwrap().unwrap();
        assert_eq!(session.external_id, "ses_1");
    }

    #[test]
    fn a_file_that_is_not_a_database_is_reported_as_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        fs::write(&path, "this is not a database, just some text").unwrap();
        assert!(matches!(
            OpencodeDb::new(&path).list(),
            Err(HistoryError::Database { .. })
        ));
    }
}

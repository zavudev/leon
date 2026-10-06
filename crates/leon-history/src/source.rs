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

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use leon_core::AgentId;
use rusqlite::Connection;
use thiserror::Error;

use crate::opencode_json::{self, JsonMessage, JsonOutcome};
use crate::session::ParsedSession;
use crate::survey::{Place, SkipReason, Survey};
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
    /// The database opened but its layout is not one Leon knows. This is
    /// reported, never treated as "no sessions".
    #[error("{path} has a layout Leon does not know: {detail}")]
    Unsupported {
        /// The database file.
        path: String,
        /// What is missing.
        detail: String,
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
    fn agent(&self) -> AgentId;

    /// Enumerates the items currently available. A source whose root does
    /// not exist lists nothing; that is not an error.
    fn list(&self) -> Result<Vec<SourceItem>, HistoryError>;

    /// Loads and parses one item. `Ok(None)` means the item was readable but
    /// holds no messages.
    fn load(&self, item: &SourceItem) -> Result<Option<ParsedSession>, HistoryError>;

    /// A cheap marker that changes whenever something in the source may have
    /// changed (file sizes and times, including a database's `-wal` file),
    /// without listing or parsing anything. `None` when the source cannot say.
    /// Callers use it to skip a whole import when nothing moved.
    fn stamp(&self) -> Option<String> {
        None
    }

    /// What the last [`list`](Self::list) could not read although it listed
    /// other things: a database of a layout Leon does not know next to one
    /// it does. Words only, no content.
    fn problems(&self) -> Vec<String> {
        Vec::new()
    }

    /// Looks at everything the source holds and reports what it found and
    /// why parts of it are not imported. Reads transcripts but stores
    /// nothing, and the report carries no content.
    fn survey(&self) -> Survey {
        Survey::new(self.agent())
    }
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
    fn agent(&self) -> AgentId {
        AgentId::CLAUDE
    }

    fn list(&self) -> Result<Vec<SourceItem>, HistoryError> {
        let mut items = Vec::new();
        for project in read_dir(&self.root)? {
            if project.is_dir() {
                for file in read_dir(&project)? {
                    if has_extension(&file, "jsonl") {
                        items.extend(file_item(AgentId::CLAUDE, &file));
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

    fn stamp(&self) -> Option<String> {
        Some(tree_stamp(&self.root, 2))
    }

    fn survey(&self) -> Survey {
        let mut survey = Survey::new(AgentId::CLAUDE);
        let mut place = Place::new("configured", &self.root, self.root.is_dir());
        let projects: Vec<_> = read_dir(&self.root)
            .unwrap_or_default()
            .into_iter()
            .filter(|path| path.is_dir())
            .collect();
        place
            .details
            .push("layout: <projects>/<project>/<session>.jsonl".to_owned());
        place
            .details
            .push(format!("{} project folders", projects.len()));
        survey.places.push(place);
        for project in projects {
            for entry in read_dir(&project).unwrap_or_default() {
                if entry.is_dir() {
                    // Sub-agent transcripts live deeper than the session files.
                    for _ in jsonl_below(&entry, 4) {
                        survey.skip(SkipReason::ChildSession);
                    }
                } else if has_extension(&entry, "jsonl") {
                    survey.found += 1;
                    survey_transcript(&mut survey, &entry, |bytes| {
                        claude::parse_session(&file_stem(&entry), bytes)
                    });
                }
            }
        }
        survey
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
    fn agent(&self) -> AgentId {
        AgentId::CODEX
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
                    items.extend(file_item(AgentId::CODEX, &entry));
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

    fn stamp(&self) -> Option<String> {
        Some(tree_stamp(&self.root, MAX_ROLLOUT_DEPTH))
    }

    fn survey(&self) -> Survey {
        let mut survey = Survey::new(AgentId::CODEX);
        let mut place = Place::new("configured", &self.root, self.root.is_dir());
        place
            .details
            .push("layout: <sessions>/<year>/<month>/<day>/rollout-*.jsonl".to_owned());
        survey.places.push(place);
        for entry in jsonl_below(&self.root, MAX_ROLLOUT_DEPTH) {
            if file_stem(&entry).starts_with("rollout-") {
                survey.found += 1;
                survey_transcript(&mut survey, &entry, |bytes| {
                    codex::parse_session(&rollout_id(&file_stem(&entry)), bytes)
                });
            } else {
                survey.skip(SkipReason::NotATranscript);
            }
        }
        survey
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

impl OpencodeDb {
    /// The top-level sessions and the ids of every session, children
    /// included. A database whose layout Leon does not know is an
    /// [`HistoryError::Unsupported`], not an empty list.
    fn list_with_ids(&self) -> Result<(Vec<SourceItem>, Vec<String>), HistoryError> {
        if !self.path.is_file() {
            return Ok((Vec::new(), Vec::new()));
        }
        let schema = self.with_connection(opencode::inspect)?;
        if let Some(detail) = schema.unsupported() {
            return Err(HistoryError::Unsupported {
                path: self.path.display().to_string(),
                detail,
            });
        }
        let sessions = self.with_connection(opencode::list_sessions)?;
        let ids = self.with_connection(opencode::all_ids)?;
        let database = self.path.display();
        let items = sessions
            .into_iter()
            .map(|session| SourceItem {
                key: format!("opencode:{database}#{}", session.id),
                fingerprint: session.fingerprint,
                locator: session.id,
            })
            .collect();
        Ok((items, ids))
    }
}

impl HistorySource for OpencodeDb {
    fn agent(&self) -> AgentId {
        AgentId::OPENCODE
    }

    fn list(&self) -> Result<Vec<SourceItem>, HistoryError> {
        self.list_with_ids().map(|(items, _)| items)
    }

    fn load(&self, item: &SourceItem) -> Result<Option<ParsedSession>, HistoryError> {
        self.with_connection(|connection| opencode::read_session(connection, &item.locator))
    }
}

/// Every `.jsonl` file below `directory`, at most `depth` levels down.
fn jsonl_below(directory: &Path, depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(directory.to_path_buf(), 0usize)];
    while let Some((directory, level)) = pending.pop() {
        for entry in read_dir(&directory).unwrap_or_default() {
            if entry.is_dir() {
                if level < depth {
                    pending.push((entry, level + 1));
                }
            } else if has_extension(&entry, "jsonl") {
                found.push(entry);
            }
        }
    }
    found.sort();
    found
}

/// Parses one transcript for a survey and files what it finds.
fn survey_transcript(
    survey: &mut Survey,
    path: &Path,
    parse: impl FnOnce(&[u8]) -> Option<ParsedSession>,
) {
    let Ok(bytes) = fs::read(path) else {
        survey.skip(SkipReason::Unreadable);
        return;
    };
    match parse(&bytes) {
        None => survey.skip(SkipReason::Empty),
        Some(parsed) => see_parsed(survey, &parsed),
    }
}

fn see_parsed(survey: &mut Survey, parsed: &ParsedSession) {
    survey.malformed += parsed.malformed;
    survey.see_time(parsed.updated_at);
    if !Survey::is_usable_folder(&parsed.cwd) {
        survey.without_folder += 1;
    }
}

/// A marker for a directory tree: the size and modification time of every
/// directory and `.jsonl` file down to `depth`. Cheap (metadata only) and it
/// changes when a transcript is added, grows or is rewritten.
fn tree_stamp(root: &Path, depth: usize) -> String {
    let mut files = 0usize;
    let mut newest = 0u128;
    let mut bytes = 0u64;
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    while let Some((directory, level)) = pending.pop() {
        for entry in read_dir(&directory).unwrap_or_default() {
            let Ok(metadata) = fs::metadata(&entry) else {
                continue;
            };
            if metadata.is_dir() {
                if level < depth {
                    pending.push((entry, level + 1));
                }
            } else if has_extension(&entry, "jsonl") {
                files += 1;
                bytes += metadata.len();
                newest = newest.max(modified_millis(&metadata));
            }
        }
    }
    format!("{files}:{bytes}:{newest}")
}

fn modified_millis(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis())
}

/// Size and modification time of a file, or `-` when it is not there.
fn file_stamp(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(metadata) => format!("{}@{}", metadata.len(), modified_millis(&metadata)),
        Err(_) => "-".to_owned(),
    }
}

/// opencode's own data: every database in its folder and the older JSON
/// layout beside them. See [`opencode`] and [`crate::opencode_json`].
///
/// opencode names its database `opencode.db`, but builds from other channels
/// use `opencode-<channel>.db` and `OPENCODE_DB` can point anywhere, so every
/// `opencode*.db` next to the configured one is read too. A session held by
/// both a database and the JSON folder (opencode leaves the folder behind
/// when it moves to SQLite) is imported once, from the database.
#[derive(Debug)]
pub struct OpencodeData {
    primary: PathBuf,
    problems: Mutex<Vec<String>>,
}

impl OpencodeData {
    /// A source for the database at `primary`; the folder it sits in is
    /// searched for the others and for `storage/`.
    pub fn new(primary: impl Into<PathBuf>) -> Self {
        Self {
            primary: primary.into(),
            problems: Mutex::new(Vec::new()),
        }
    }

    fn folder(&self) -> PathBuf {
        self.primary
            .parent()
            .map_or_else(PathBuf::new, Path::to_path_buf)
    }

    fn storage(&self) -> PathBuf {
        self.folder().join("storage")
    }

    /// The database files: the configured one, then the others beside it.
    fn databases(&self) -> Vec<PathBuf> {
        let mut found = vec![self.primary.clone()];
        let mut beside: Vec<PathBuf> = read_dir(&self.folder())
            .unwrap_or_default()
            .into_iter()
            .filter(|path| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                path.is_file()
                    && name.ends_with(".db")
                    && (name == "opencode.db" || name.starts_with("opencode-"))
                    && *path != self.primary
            })
            .collect();
        beside.sort();
        found.extend(beside);
        found.retain(|path| path.is_file());
        found
    }

    fn session_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for project in read_dir(&self.storage().join("session")).unwrap_or_default() {
            if project.is_dir() {
                files.extend(
                    read_dir(&project)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|file| has_extension(file, "json")),
                );
            }
        }
        files.sort();
        files
    }

    fn json_fingerprint(&self, session_file: &Path, id: &str) -> String {
        let messages = self.storage().join("message").join(id);
        let count = read_dir(&messages).map_or(0, |entries| entries.len());
        format!(
            "{}:{}:{count}",
            file_stamp(session_file),
            file_stamp(&messages)
        )
    }

    fn read_json(&self, session_file: &Path) -> Result<JsonOutcome, HistoryError> {
        let info = read_file(session_file)?;
        let Some((id, _)) = opencode_json::peek(&info) else {
            return Ok(JsonOutcome::Unreadable);
        };
        let mut messages = Vec::new();
        for file in read_dir(&self.storage().join("message").join(&id))? {
            if !has_extension(&file, "json") {
                continue;
            }
            let Ok(message) = fs::read(&file) else {
                continue;
            };
            let mut parts = Vec::new();
            let message_id = file_stem(&file);
            for part in read_dir(&self.storage().join("part").join(message_id))? {
                if has_extension(&part, "json") {
                    if let Ok(bytes) = fs::read(&part) {
                        parts.push(bytes);
                    }
                }
            }
            messages.push(JsonMessage {
                info: message,
                parts,
            });
        }
        Ok(opencode_json::parse_session(&info, &messages))
    }
}

impl HistorySource for OpencodeData {
    fn agent(&self) -> AgentId {
        AgentId::OPENCODE
    }

    fn list(&self) -> Result<Vec<SourceItem>, HistoryError> {
        let mut items = Vec::new();
        let mut ids: HashSet<String> = HashSet::new();
        let mut first_error = None;
        let mut problems = Vec::new();
        for path in self.databases() {
            let db = OpencodeDb::new(&path);
            match db.list_with_ids() {
                Ok((listed, all)) => {
                    ids.extend(all);
                    items.extend(listed.into_iter().map(|item| SourceItem {
                        locator: format!("sqlite\t{}\t{}", path.display(), item.locator),
                        ..item
                    }));
                }
                Err(error) => {
                    tracing::warn!(%error, "cannot list an opencode database");
                    problems.push(error.to_string());
                    first_error.get_or_insert(error);
                }
            }
        }
        for file in self.session_files() {
            let Ok(info) = read_file(&file) else { continue };
            let Some((id, child)) = opencode_json::peek(&info) else {
                continue;
            };
            if child || ids.contains(&id) {
                continue;
            }
            items.push(SourceItem {
                key: format!("opencode:{}", file.display()),
                fingerprint: self.json_fingerprint(&file, &id),
                locator: format!("json\t{}", file.display()),
            });
        }
        *self.problems.lock().unwrap_or_else(PoisonError::into_inner) = problems;
        match first_error {
            // One unreadable database must not hide the rest, but when nothing
            // at all could be listed the failure is the answer.
            Some(error) if items.is_empty() => Err(error),
            _ => Ok(items),
        }
    }

    fn load(&self, item: &SourceItem) -> Result<Option<ParsedSession>, HistoryError> {
        let mut parts = item.locator.splitn(3, '\t');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("sqlite"), Some(path), Some(id)) => OpencodeDb::new(path).load(&SourceItem {
                locator: id.to_owned(),
                ..item.clone()
            }),
            (Some("json"), Some(path), None) => Ok(match self.read_json(Path::new(path))? {
                JsonOutcome::Session(parsed) => Some(*parsed),
                _ => None,
            }),
            _ => Ok(None),
        }
    }

    fn problems(&self) -> Vec<String> {
        self.problems
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn stamp(&self) -> Option<String> {
        let mut stamp = String::new();
        for path in self.databases() {
            let mut wal = path.clone().into_os_string();
            wal.push("-wal");
            // An empty log is the same as none: opening a database in WAL mode
            // may create it without anything having changed.
            let log = match fs::metadata(&wal) {
                Ok(metadata) if metadata.len() > 0 => file_stamp(Path::new(&wal)),
                _ => "-".to_owned(),
            };
            stamp.push_str(&format!("{}|{log};", file_stamp(&path)));
        }
        stamp.push_str(&file_stamp(&self.storage().join("session")));
        stamp.push_str(&file_stamp(&self.storage().join("message")));
        Some(stamp)
    }

    fn survey(&self) -> Survey {
        let mut survey = Survey::new(AgentId::OPENCODE);
        let mut ids: HashSet<String> = HashSet::new();
        let databases = self.databases();
        if databases.is_empty() {
            survey
                .places
                .push(Place::new("configured", &self.primary, false));
        }
        for path in &databases {
            let origin = if *path == self.primary {
                "configured"
            } else {
                "found beside"
            };
            survey_database(&mut survey, origin, path, &mut ids);
        }
        let storage = self.storage();
        let mut place = Place::new("json layout", &storage, storage.is_dir());
        let files = self.session_files();
        if !files.is_empty() {
            place
                .details
                .push("layout: storage/session, message, part (older opencode)".to_owned());
            place.details.push(format!("{} session files", files.len()));
        }
        survey.places.push(place);
        for file in files {
            match self.read_json(&file) {
                Err(_) | Ok(JsonOutcome::Unreadable) => survey.skip(SkipReason::Unreadable),
                Ok(JsonOutcome::Child) => survey.skip(SkipReason::ChildSession),
                Ok(JsonOutcome::Empty) => {
                    survey.found += 1;
                    survey.skip(SkipReason::Empty);
                }
                Ok(JsonOutcome::Session(parsed)) => {
                    if ids.contains(&parsed.external_id) {
                        survey.skip(SkipReason::DuplicateId);
                    } else {
                        survey.found += 1;
                        see_parsed(&mut survey, &parsed);
                    }
                }
            }
        }
        survey
    }
}

/// Reads one opencode database into a survey: layout, journal, sizes and
/// the sessions with the reasons some of them are not imported.
fn survey_database(survey: &mut Survey, origin: &str, path: &Path, ids: &mut HashSet<String>) {
    let mut place = Place::new(origin, path, true);
    let mut wal = path.to_path_buf().into_os_string();
    wal.push("-wal");
    let wal = PathBuf::from(wal);
    place.details.push(format!(
        "size: {} bytes",
        fs::metadata(path).map_or(0, |m| m.len())
    ));
    match fs::metadata(&wal) {
        Ok(metadata) => place
            .details
            .push(format!("-wal file present, {} bytes", metadata.len())),
        Err(_) => place.details.push("no -wal file".to_owned()),
    }
    let connection = match opencode::open_read_only(path) {
        Ok(connection) => connection,
        Err(error) => {
            survey
                .problems
                .push(format!("cannot open {}: {error}", path.display()));
            survey.skip(SkipReason::Unreadable);
            survey.places.push(place);
            return;
        }
    };
    match opencode::inspect(&connection) {
        Err(error) => {
            survey
                .problems
                .push(format!("cannot inspect {}: {error}", path.display()));
            survey.skip(SkipReason::Unreadable);
        }
        Ok(schema) => {
            place
                .details
                .push("layout: SQLite (session, message, part)".to_owned());
            place
                .details
                .push(format!("tables: {}", schema.tables.join(", ")));
            place.details.push(format!(
                "session columns: {}",
                schema.session_columns.join(", ")
            ));
            place
                .details
                .push(format!("user_version: {}", schema.user_version));
            place
                .details
                .push(format!("journal_mode: {}", schema.journal_mode));
            if let Some(detail) = schema.unsupported() {
                survey
                    .problems
                    .push(format!("{}: unsupported layout: {detail}", path.display()));
                survey.skip(SkipReason::UnsupportedSchema);
            } else {
                ids.extend(opencode::all_ids(&connection).unwrap_or_default());
                if let Ok((_, children)) = opencode::count_sessions(&connection) {
                    for _ in 0..children {
                        survey.skip(SkipReason::ChildSession);
                    }
                }
                for session in opencode::list_sessions(&connection).unwrap_or_default() {
                    survey.found += 1;
                    match opencode::read_session(&connection, &session.id) {
                        Ok(Some(parsed)) => see_parsed(survey, &parsed),
                        Ok(None) => survey.skip(SkipReason::Empty),
                        Err(_) => survey.skip(SkipReason::Unreadable),
                    }
                }
            }
        }
    }
    survey.places.push(place);
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
fn file_item(agent: AgentId, path: &Path) -> Option<SourceItem> {
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

    fn json_session(root: &Path, id: &str, parent: Option<&str>, text: &str) {
        let storage = root.join("storage");
        let info = serde_json::json!({"id": id, "projectID": "p", "directory": "/srv/api",
            "title": "t", "parentID": parent, "time": {"created": 1000, "updated": 2000}});
        write(
            &storage.join(format!("session/p/{id}.json")),
            &info.to_string(),
        );
        write(
            &storage.join(format!("message/{id}/msg_{id}.json")),
            &serde_json::json!({"id": format!("msg_{id}"), "role": "user",
                "time": {"created": 1000}})
            .to_string(),
        );
        write(
            &storage.join(format!("part/msg_{id}/prt_{id}.json")),
            &serde_json::json!({"id": format!("prt_{id}"), "type": "text", "text": text})
                .to_string(),
        );
    }

    #[test]
    fn the_older_json_layout_is_listed_and_loaded_and_children_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        json_session(dir.path(), "ses_old", None, "from the json layout");
        json_session(dir.path(), "ses_kid", Some("ses_old"), "delegated");

        let source = OpencodeData::new(dir.path().join("opencode.db"));
        let items = source.list().unwrap();
        assert_eq!(items.len(), 1);
        let session = source.load(&items[0]).unwrap().unwrap();
        assert_eq!(session.external_id, "ses_old");
        assert_eq!(session.messages[0].text, "from the json layout");

        let survey = source.survey();
        assert_eq!(survey.found, 1);
        assert_eq!(survey.skipped[&SkipReason::ChildSession], 1);
    }

    #[test]
    fn a_json_session_that_grows_changes_its_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        json_session(dir.path(), "ses_old", None, "one");
        let source = OpencodeData::new(dir.path().join("opencode.db"));
        let before = source.list().unwrap();
        write(
            &dir.path().join("storage/message/ses_old/msg_two.json"),
            r#"{"id":"msg_two","role":"assistant","time":{"created":3000}}"#,
        );
        assert_ne!(source.list().unwrap()[0].fingerprint, before[0].fingerprint);
    }

    #[test]
    fn a_session_held_by_the_database_and_the_json_folder_is_listed_once() {
        let dir = tempfile::tempdir().unwrap();
        opencode::fixture::populate(&Connection::open(dir.path().join("opencode.db")).unwrap());
        json_session(dir.path(), "ses_1", None, "stale copy");
        json_session(dir.path(), "ses_only_json", None, "old");

        let source = OpencodeData::new(dir.path().join("opencode.db"));
        let items = source.list().unwrap();
        let ids: Vec<_> = items
            .iter()
            .map(|item| source.load(item).unwrap().unwrap().external_id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"ses_1".to_owned()) && ids.contains(&"ses_only_json".to_owned()));
        let survey = source.survey();
        assert_eq!(survey.skipped[&SkipReason::DuplicateId], 1);
    }

    #[test]
    fn a_channel_database_beside_the_configured_one_is_read_too() {
        let dir = tempfile::tempdir().unwrap();
        opencode::fixture::populate(&Connection::open(dir.path().join("opencode.db")).unwrap());
        let other = Connection::open(dir.path().join("opencode-dev.db")).unwrap();
        opencode::fixture::create(&other);
        opencode::fixture::session(&other, "ses_dev", None, "dev build", 5, 6);
        opencode::fixture::message(&other, "m", "ses_dev", 5, r#"{"role":"user"}"#);
        opencode::fixture::part(
            &other,
            "p",
            "m",
            "ses_dev",
            5,
            serde_json::json!({"type": "text", "text": "hi"}),
        );
        drop(other);

        let source = OpencodeData::new(dir.path().join("opencode.db"));
        assert_eq!(source.list().unwrap().len(), 2);
        let places = source.survey().places;
        assert!(places
            .iter()
            .any(|p| p.origin == "found beside" && p.exists));
    }

    #[test]
    fn an_unknown_database_layout_is_an_error_and_a_reported_problem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE sessions_v2 (id TEXT, cwd TEXT);")
            .unwrap();
        let source = OpencodeData::new(&path);
        assert!(matches!(
            source.list(),
            Err(HistoryError::Unsupported { .. })
        ));
        let survey = source.survey();
        assert_eq!(survey.skipped[&SkipReason::UnsupportedSchema], 1);
        assert!(survey.problems[0].contains("no `session` table"));
    }

    #[test]
    fn a_session_row_without_messages_is_imported_once_it_gains_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let connection = Connection::open(&path).unwrap();
        opencode::fixture::create(&connection);
        opencode::fixture::session(&connection, "ses_new", None, "New", 10, 10);
        let source = OpencodeData::new(&path);
        let before = source.list().unwrap();
        assert_eq!(before.len(), 1);
        assert!(source.load(&before[0]).unwrap().is_none());

        opencode::fixture::message(&connection, "m", "ses_new", 20, r#"{"role":"user"}"#);
        opencode::fixture::part(
            &connection,
            "p",
            "m",
            "ses_new",
            20,
            serde_json::json!({"type": "text", "text": "first words"}),
        );
        let after = source.list().unwrap();
        assert_ne!(after[0].fingerprint, before[0].fingerprint);
        assert!(source.load(&after[0]).unwrap().is_some());
    }

    #[test]
    fn a_change_that_only_reached_the_wal_file_moves_the_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let writer = Connection::open(&path).unwrap();
        writer.pragma_update(None, "journal_mode", "WAL").unwrap();
        // Keep the log from being folded back into the main file.
        writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        opencode::fixture::populate(&writer);
        let source = OpencodeData::new(&path);
        let before = source.stamp().unwrap();
        let main_before = fs::metadata(&path).unwrap().len();

        opencode::fixture::message(&writer, "m9", "ses_1", 9_500, r#"{"role":"user"}"#);
        assert_eq!(fs::metadata(&path).unwrap().len(), main_before);
        assert_ne!(source.stamp().unwrap(), before);
    }
}

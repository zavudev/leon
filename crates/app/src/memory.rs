//! The shared memory on this computer: the reading and the writing.
//!
//! `leon-memory` decides (what a memory file says, where the block goes, what
//! a `.mcp.json` becomes) and the store keeps the entries; this module is the
//! thin part between them and the disk, used by the command line
//! (`memory_cli.rs`), by the engine (the palette's "Turn on agent memory for
//! this project") and by the window when it starts a terminal:
//!
//! * [`resolve_root`]: the project a folder shares its memory with. The store
//!   is asked first, then git's own files (so a folder Leon was never shown
//!   still shares with its repository, a linked worktree with its main
//!   checkout), and a folder outside any repository is its own project.
//! * [`Service`]: the entries of one folder's project and of the global
//!   scope, and the memory files that mirror them: `<data dir>/memory/
//!   global.md` and one file per project. A project's file holds the global
//!   entries too, so an agent reads one file. Every write of the service
//!   writes the files again, each to a temporary file that is then renamed
//!   over the old one.
//! * [`set_block`] and [`set_mcp_json`]: the opt-in. They write inside the
//!   project's root and nowhere else: the three instruction files and
//!   `.mcp.json`, never through a link that leads elsewhere.
//!
//! Nothing here runs a process, and nothing is written into a project unless
//! somebody asked (`leon memory enable`, or the palette's command).

use leon_core::store::memory::purge_before;
use leon_core::{
    MachineId, Memory, MemoryCounts, MemoryHit, MemoryKind, MemoryPatch, MemoryScope, NewMemory,
    SavedMemory, Store, StoreError,
};
use leon_memory::files::{self, Found, Step};
use leon_memory::mcp::{Backend, Draft, Reach};
use leon_memory::mcp_json::{self, Edit};
use leon_memory::render::{self, Contents};
use leon_memory::scope::{self, Folders};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The most bytes read of one of git's own files, or of an instruction file.
const MAX_READ: u64 = 1 << 20;

/// The key of the setting that holds the memory file's size.
pub const BUDGET_KEY: &str = "memory_budget";
/// The smallest and the largest size a memory file may be given, in bytes:
/// room for the frame and a few lines, and far more than any agent should be
/// made to read at the start of a session.
pub const BUDGET_RANGE: (i64, i64) = (2_000, 200_000);
/// The size of a memory file unless the setting says otherwise.
pub const DEFAULT_BUDGET: i64 = render::DEFAULT_BUDGET as i64;

/// The size a stored value of the setting means: the value within its range,
/// the default when there is none.
pub fn budget_of(stored: Option<i64>) -> usize {
    let bytes = stored
        .unwrap_or(DEFAULT_BUDGET)
        .clamp(BUDGET_RANGE.0, BUDGET_RANGE.1);
    usize::try_from(bytes).unwrap_or(render::DEFAULT_BUDGET)
}

/// The size of the memory files of a data folder: the setting in its
/// `settings.json`, read the way the application reads it (a value that
/// cannot be used is the default, one out of range the nearest bound), or the
/// default when there is no file. The command line and the MCP server are
/// processes of their own: this is how they write the same file the
/// application does.
pub fn budget_in(data_dir: &Path) -> usize {
    let stored = crate::settings::stored(&data_dir.join(crate::settings::FILE_NAME));
    budget_of(stored.value(BUDGET_KEY).as_int())
}

/// How many days a forgotten entry is kept before it is purged, unless
/// `purge --older-than` says otherwise.
pub const KEPT_DAYS: u32 = 30;

/// How many entries of a scope a memory file is made from: more than its
/// budget can ever show.
const RENDERED: usize = 500;

/// The disk, as the walk to a repository's root asks for it.
pub struct Disk;

impl Folders for Disk {
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read(&self, path: &Path) -> Option<String> {
        read_text(path).ok()
    }
}

/// The text of a file that is at most [`MAX_READ`] bytes of UTF-8.
fn read_text(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    let read = std::fs::File::open(path)?
        .take(MAX_READ + 1)
        .read_to_string(&mut text)?;
    if read as u64 > MAX_READ {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the file is too large",
        ));
    }
    Ok(text)
}

/// The root of the project `folder` shares its memory with: the project of
/// the store that owns it, else the main checkout of the git repository it is
/// in, else the folder itself.
pub fn resolve_root(store: &Store, folders: &dyn Folders, folder: &Path) -> String {
    let text = folder.to_string_lossy();
    match store.memory_root(&MachineId::local(), &text) {
        Ok(Some(root)) => return root,
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "could not ask the store whose folder this is"),
    }
    scope::git_root(folders, folder)
        .map(|root| root.to_string_lossy().into_owned())
        .unwrap_or_else(|| text.into_owned())
}

/// Where the instruction files and `.mcp.json` of the project are for somebody
/// working in `folder`: the project's root, or, from a linked git worktree
/// that lives outside it, the top of that worktree. A worktree shares the
/// project's memory but has its own checkout of the project's files, and that
/// is the copy an agent started there reads.
pub fn files_root(folders: &dyn Folders, root: &str, folder: &Path) -> PathBuf {
    if leon_core::path::is_within(&folder.to_string_lossy(), root) {
        return PathBuf::from(root);
    }
    scope::checkout_root(folders, folder).unwrap_or_else(|| PathBuf::from(root))
}

/// The folder of the memory files below a data folder.
pub fn files_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(scope::FOLDER)
}

/// The memory file of the project rooted at `root`.
pub fn file_of(data_dir: &Path, root: &str) -> PathBuf {
    files_dir(data_dir).join(scope::file_name(root))
}

/// The memory file of the global scope.
pub fn global_file(data_dir: &Path) -> PathBuf {
    files_dir(data_dir).join(scope::GLOBAL_FILE)
}

/// Writes a file whole or not at all: to a temporary file beside it, which is
/// then renamed over it. An existing file keeps its permissions.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the file has no folder"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("the file has no name"))?
        .to_string_lossy();
    let temporary = dir.join(format!(".{name}.leon-{}.tmp", std::process::id()));
    let written = (|| {
        // A new file, never one that is there: a link somebody left under
        // this name is not written through.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&temporary, meta.permissions())?;
        }
        std::fs::rename(&temporary, path)
    })();
    match &written {
        // What was there under the temporary name is not this run's.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => {
            let _ = std::fs::remove_file(&temporary);
        }
        Ok(()) => {}
    }
    written
}

/// The memory file's text for a project (`Some(root)`) or for the global
/// scope alone, in at most `budget` bytes.
pub fn document(store: &Store, root: Option<&str>, budget: usize) -> Result<String, StoreError> {
    let global = store.memories(&MemoryScope::Global, RENDERED)?;
    let global_total = store.memory_count(&MemoryScope::Global)?;
    let (project, project_total) = match root {
        Some(root) => {
            let scope = MemoryScope::project(root);
            (
                store.memories(&scope, RENDERED)?,
                store.memory_count(&scope)?,
            )
        }
        None => (Vec::new(), 0),
    };
    Ok(render::render(
        &Contents {
            root,
            global: &global,
            global_total,
            project: &project,
            project_total,
        },
        budget,
    ))
}

/// Writes the memory file of one project again and returns where it is.
pub fn write_file(
    store: &Store,
    data_dir: &Path,
    root: &str,
    budget: usize,
) -> Result<PathBuf, String> {
    let path = file_of(data_dir, root);
    let text = document(store, Some(root), budget).map_err(|error| error.to_string())?;
    write_to(&path, &text)?;
    Ok(path)
}

fn write_to(path: &Path, text: &str) -> Result<(), String> {
    let failed = |error: io::Error| format!("cannot write {}: {error}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(failed)?;
    }
    write_atomic(path, text).map_err(failed)
}

/// Writes the global memory file again, and every project's file that is
/// there: each holds the global entries too. A project's file says whose it
/// is in its first line; one that does not, or whose name is not the one its
/// root gets, is somebody else's and is left alone.
pub fn write_global_files(store: &Store, data_dir: &Path, budget: usize) -> Result<(), String> {
    let text = document(store, None, budget).map_err(|error| error.to_string())?;
    write_to(&global_file(data_dir), &text)?;
    let Ok(entries) = std::fs::read_dir(files_dir(data_dir)) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == scope::GLOBAL_FILE || !name.ends_with(".md") || name.starts_with('.') {
            continue;
        }
        let Ok(old) = read_text(&entry.path()) else {
            continue;
        };
        let Some(root) = render::root_of(&old).filter(|root| scope::file_name(root) == name) else {
            continue;
        };
        write_file(store, data_dir, root, budget)?;
    }
    Ok(())
}

/// The entries of one folder's project and of the global scope, with the
/// memory files that mirror them.
pub struct Service {
    store: Arc<Store>,
    data_dir: PathBuf,
    root: String,
    budget: usize,
    now: fn() -> i64,
}

/// The time, in milliseconds since the Unix epoch.
pub fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Service {
    /// The service of the project rooted at `root`, with its files below
    /// `data_dir`, each of the size that folder's settings give.
    pub fn new(store: Arc<Store>, data_dir: PathBuf, root: String) -> Self {
        Self {
            budget: budget_in(&data_dir),
            store,
            data_dir,
            root,
            now: now_millis,
        }
    }

    /// The same with another clock.
    #[cfg(test)]
    pub fn at(mut self, now: fn() -> i64) -> Self {
        self.now = now;
        self
    }

    /// The project's root.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// The scope a command means.
    pub fn scope(&self, global: bool) -> MemoryScope {
        if global {
            MemoryScope::Global
        } else {
            MemoryScope::project(self.root.clone())
        }
    }

    /// The project's memory file.
    pub fn file(&self) -> PathBuf {
        file_of(&self.data_dir, &self.root)
    }

    /// The global memory file.
    pub fn global_file(&self) -> PathBuf {
        global_file(&self.data_dir)
    }

    /// How many entries the project and the global scope hold.
    pub fn counts(&self) -> Result<(MemoryCounts, MemoryCounts), StoreError> {
        Ok((
            self.store.memory_counts(&self.scope(false))?,
            self.store.memory_counts(&MemoryScope::Global)?,
        ))
    }

    /// Remembers that the project's agent memory was turned on or off from
    /// here, so that the window does not ask about it.
    pub fn record_choice(&self, choice: leon_core::MemoryChoice) {
        let recorded = self
            .store
            .set_memory_choice(&self.root, choice, (self.now)());
        if let Err(error) = recorded {
            tracing::warn!(%error, "the answer about agent memory was not kept");
        }
    }

    /// What was decided about the project's agent memory, and when.
    pub fn choice(&self) -> Result<Option<(leon_core::MemoryChoice, i64)>, StoreError> {
        self.store.memory_choice(&self.root)
    }

    /// The memory file's text, from the store.
    pub fn document(&self) -> Result<String, StoreError> {
        document(&self.store, Some(&self.root), self.budget)
    }

    /// One entry of this project or of the global scope, whole, live or
    /// forgotten.
    pub fn get(&self, id: &str) -> Result<Memory, StoreError> {
        self.own(id)
    }

    /// Writes the files a change of `scope` shows in. A file that cannot be
    /// written is a warning: the entry is in the store either way.
    pub fn refresh(&self, scope: &MemoryScope) {
        let written = match scope {
            MemoryScope::Global => write_global_files(&self.store, &self.data_dir, self.budget),
            MemoryScope::Project(_) => Ok(()),
        }
        .and_then(|()| {
            write_file(&self.store, &self.data_dir, &self.root, self.budget).map(|_| ())
        });
        if let Err(error) = written {
            tracing::warn!(%error, "the memory file was not written");
        }
    }

    /// The entry `id` names, when it is this folder's to change: one of this
    /// project or of the global scope. Another project's is not.
    fn own(&self, id: &str) -> Result<Memory, StoreError> {
        let memory = self.store.memory(id)?;
        if let MemoryScope::Project(root) = &memory.scope {
            if leon_core::path::key(root) != leon_core::path::key(&self.root) {
                return Err(StoreError::NotFound("memory"));
            }
        }
        Ok(memory)
    }

    /// Does `change` to the entry `id` names and writes the files.
    fn change(
        &self,
        id: &str,
        change: impl FnOnce(&Store, &str) -> Result<Memory, StoreError>,
    ) -> Result<Memory, StoreError> {
        let memory = change(&self.store, &self.own(id)?.id)?;
        self.refresh(&memory.scope);
        Ok(memory)
    }

    /// Saves an entry (a new one, a revision of its topic's, or nothing new)
    /// and writes the files.
    pub fn add(&self, new: &NewMemory) -> Result<SavedMemory, StoreError> {
        let saved = self.store.add_memory(new, (self.now)())?;
        if !matches!(saved, SavedMemory::Known(_)) {
            self.refresh(&saved.memory().scope);
        }
        Ok(saved)
    }

    /// Edits an entry and writes the files.
    pub fn update(&self, id: &str, patch: &MemoryPatch) -> Result<Memory, StoreError> {
        let now = (self.now)();
        self.change(id, |store, id| store.update_memory(id, patch, now))
    }

    /// Pins an entry, or unpins it, and writes the files.
    pub fn pin(&self, id: &str, pinned: bool) -> Result<Memory, StoreError> {
        self.change(id, |store, id| store.pin_memory(id, pinned))
    }

    /// Forgets an entry and writes the files: it can be restored until it is
    /// purged. What was forgotten more than [`KEPT_DAYS`] ago is purged on the
    /// way, so that forgotten entries do not pile up when nobody purges.
    pub fn forget(&self, id: &str) -> Result<Memory, StoreError> {
        let now = (self.now)();
        let memory = self.change(id, |store, id| store.forget_memory(id, now))?;
        if let Err(error) = self.purge(KEPT_DAYS) {
            tracing::warn!(%error, "the forgotten entries were not purged");
        }
        Ok(memory)
    }

    /// Removes an entry for good and writes the files.
    pub fn delete(&self, id: &str) -> Result<Memory, StoreError> {
        self.change(id, |store, id| store.delete_memory(id))
    }

    /// Brings a forgotten entry back and writes the files.
    pub fn restore(&self, id: &str) -> Result<Memory, StoreError> {
        self.change(id, |store, id| store.restore_memory(id))
    }

    /// Removes for good what was forgotten more than `days` ago, in every
    /// scope. Returns how many entries went. No file changes: a forgotten
    /// entry was in none.
    pub fn purge(&self, days: u32) -> Result<usize, StoreError> {
        self.store.purge_memories(purge_before((self.now)(), days))
    }

    /// The live entries of the project, or of the global scope.
    pub fn list(&self, global: bool, limit: usize) -> Result<Vec<Memory>, StoreError> {
        self.store.memories(&self.scope(global), limit)
    }

    /// The forgotten entries of the project, or of the global scope.
    pub fn forgotten(&self, global: bool, limit: usize) -> Result<Vec<Memory>, StoreError> {
        self.store.forgotten_memories(&self.scope(global), limit)
    }

    /// The best matches in the project and the global scope, or in the
    /// global scope alone.
    pub fn search(
        &self,
        global: bool,
        words: &str,
        limit: usize,
    ) -> Result<Vec<MemoryHit>, StoreError> {
        self.store
            .search_memories(&self.scope(global), words, limit)
    }
}

/// A store error as a sentence for whoever asked.
pub fn sentence(error: &StoreError) -> String {
    match error {
        StoreError::NotFound(_) => "no entry of this memory has that id".to_owned(),
        StoreError::Invalid(why) => why.clone(),
        other => other.to_string(),
    }
}

impl Backend for Service {
    fn search(
        &mut self,
        words: &str,
        reach: Reach,
        limit: usize,
    ) -> Result<Vec<MemoryHit>, String> {
        Service::search(self, reach == Reach::Global, words, limit).map_err(|e| sentence(&e))
    }

    fn add(&mut self, draft: Draft) -> Result<SavedMemory, String> {
        let new = NewMemory {
            scope: self.scope(draft.reach == Reach::Global),
            kind: draft.kind,
            title: draft.title,
            text: draft.text,
            agent: draft.agent,
            topic: draft.topic,
        };
        Service::add(self, &new).map_err(|e| sentence(&e))
    }

    fn get(&mut self, id: &str) -> Result<Memory, String> {
        Service::get(self, id).map_err(|e| sentence(&e))
    }

    fn update(&mut self, id: &str, patch: MemoryPatch) -> Result<Memory, String> {
        Service::update(self, id, &patch).map_err(|e| sentence(&e))
    }

    fn pin(&mut self, id: &str, pinned: bool) -> Result<Memory, String> {
        Service::pin(self, id, pinned).map_err(|e| sentence(&e))
    }

    fn list(&mut self, reach: Reach, limit: usize) -> Result<Vec<Memory>, String> {
        Service::list(self, reach == Reach::Global, limit).map_err(|e| sentence(&e))
    }

    fn forget(&mut self, id: &str) -> Result<Memory, String> {
        Service::forget(self, id).map_err(|e| sentence(&e))
    }

    fn context(&mut self) -> Result<String, String> {
        self.document().map_err(|e| sentence(&e))
    }
}

/// One entry as the JSON the command line prints.
pub fn json_of(memory: &Memory, snippet: Option<&str>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "id": memory.id,
        "scope": render::scope_word(&memory.scope),
        "root": memory.scope.root(),
        "kind": memory.kind.as_str(),
        "title": memory.title,
        "text": memory.text,
        "agent": memory.agent,
        "topic": memory.topic,
        "revision": memory.revision,
        "duplicates": memory.duplicates,
        "pinned": memory.pinned,
        "created_at": memory.created_at,
        "updated_at": memory.updated_at,
        "last_seen_at": memory.last_seen_at,
        "forgotten_at": memory.forgotten_at,
    });
    if let Some(snippet) = snippet {
        value["snippet"] = render::plain_snippet(snippet).into();
    }
    value
}

/// The kinds, as a list for a message.
pub fn kinds() -> String {
    MemoryKind::ALL.map(MemoryKind::as_str).join(", ")
}

// ----- the opt-in: what is written into a project ------------------------

/// What is at each instruction file's place in `root`.
pub fn look(root: &Path) -> Vec<(&'static str, Found)> {
    let canonical = std::fs::canonicalize(root).ok();
    files::NAMES
        .iter()
        .map(|name| (*name, found_at(root, canonical.as_deref(), name)))
        .collect()
}

fn found_at(root: &Path, canonical: Option<&Path>, name: &'static str) -> Found {
    let path = root.join(name);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Found::Missing,
        Err(error) => return Found::Unusable(format!("cannot be read ({error})")),
    };
    if meta.file_type().is_symlink() {
        // A link is followed only to tell whether it is another name for one
        // of the other instruction files of this very folder. It is never
        // written through.
        let Ok(target) = std::fs::canonicalize(&path) else {
            return Found::Unusable("is a link that leads nowhere".to_owned());
        };
        let other = canonical.and_then(|root| {
            files::NAMES.iter().find(|other| {
                **other != name
                    && target == root.join(other)
                    && std::fs::symlink_metadata(root.join(other))
                        .is_ok_and(|meta| meta.file_type().is_file())
            })
        });
        return match other {
            Some(other) => Found::Alias(other),
            None => Found::Unusable(format!(
                "is a link to {}, which Leon does not follow",
                target.display()
            )),
        };
    }
    if !meta.file_type().is_file() {
        return Found::Unusable("is not a file".to_owned());
    }
    match read_text(&path) {
        Ok(text) => Found::File(text),
        Err(error) => Found::Unusable(format!("cannot be read as text ({error})")),
    }
}

/// One line of what was done, or would be: `wrote /x/AGENTS.md`.
fn step_line(root: &Path, step: &Step, dry_run: bool) -> String {
    let path = root.join(step.name());
    let verb = |done: &str, would: &str| {
        if dry_run {
            format!("would {would} {}", path.display())
        } else {
            format!("{done} {}", path.display())
        }
    };
    match step {
        Step::Write { .. } => verb("wrote", "write"),
        Step::Create { .. } => verb("created", "create"),
        Step::Delete { .. } => verb("removed", "remove"),
        Step::Leave { why, .. } => format!("left {} alone: it {why}", path.display()),
    }
}

/// Does the steps (unless `dry_run`) and returns a line for each.
fn carry_out(root: &Path, steps: &[Step], dry_run: bool) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for step in steps {
        let path = root.join(step.name());
        if !dry_run {
            let done = match step {
                Step::Write { text, .. } | Step::Create { text, .. } => write_atomic(&path, text),
                Step::Delete { .. } => std::fs::remove_file(&path),
                Step::Leave { .. } => Ok(()),
            };
            done.map_err(|error| format!("cannot change {}: {error}", path.display()))?;
        }
        lines.push(step_line(root, step, dry_run));
    }
    Ok(lines)
}

/// Puts the managed block into the instruction files of the project at `root`
/// (`on`), or takes it out. Returns a line for every file looked at that is
/// there; nothing is written with `dry_run`.
pub fn set_block(root: &Path, on: bool, dry_run: bool) -> Result<Vec<String>, String> {
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    let found = look(root);
    let steps = if on {
        files::enable(&found)
    } else {
        files::disable(&found)
    };
    carry_out(root, &steps, dry_run)
}

/// The instruction files of `root` that hold the block.
#[cfg(test)]
pub fn block_files(root: &Path) -> Vec<&'static str> {
    files::holding(&look(root))
}

/// The text of the project's `.mcp.json`: `Ok(None)` when there is none, an
/// error when what is there is not a plain file inside the project.
fn mcp_json_text(root: &Path) -> Result<Option<String>, String> {
    let path = root.join(mcp_json::FILE);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{} cannot be read ({error})", path.display())),
        Ok(meta) if !meta.file_type().is_file() => Err(format!(
            "{} is a link or not a file, and Leon does not write through it",
            path.display()
        )),
        Ok(_) => read_text(&path)
            .map(Some)
            .map_err(|error| format!("{} cannot be read ({error})", path.display())),
    }
}

/// Whether the project's `.mcp.json` registers the memory server.
pub fn mcp_registered(root: &Path) -> Result<bool, String> {
    let text = mcp_json_text(root)?;
    mcp_json::has(text.as_deref())
        .map_err(|error| format!("{}: {error}", root.join(mcp_json::FILE).display()))
}

/// Registers the memory server in the project's `.mcp.json` (`on`), or takes
/// it out. Returns a line saying what was done, `None` when nothing had to
/// be; a file that is not what is expected is refused, never replaced.
pub fn set_mcp_json(root: &Path, on: bool, dry_run: bool) -> Result<Option<String>, String> {
    let path = root.join(mcp_json::FILE);
    let text = mcp_json_text(root)?;
    let edit = if on {
        mcp_json::add(text.as_deref())
    } else {
        mcp_json::remove(text.as_deref())
    }
    .map_err(|error| format!("{} was left alone: {error}", path.display()))?;
    let verb = |done: &str, would: &str| {
        Some(if dry_run {
            format!("would {would} {}", path.display())
        } else {
            format!("{done} {}", path.display())
        })
    };
    let failed = |error: io::Error| format!("cannot change {}: {error}", path.display());
    match edit {
        Edit::Unchanged => Ok(None),
        Edit::Write(new) => {
            if !dry_run {
                write_atomic(&path, &new).map_err(failed)?;
            }
            Ok(match text {
                Some(_) => verb("wrote", "write"),
                None => verb("created", "create"),
            })
        }
        Edit::Delete => {
            if !dry_run {
                std::fs::remove_file(&path).map_err(failed)?;
            }
            Ok(verb("removed", "remove"))
        }
    }
}

// ----- what a terminal of Leon is told ------------------------------------

/// What the window needs to tell a terminal where the memory is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminals {
    /// The running Leon.
    pub bin: PathBuf,
    /// Leon's data folder.
    pub data_dir: PathBuf,
    /// Whether the data folder is the platform's: a terminal is told the
    /// folder only when it is not.
    pub default_data_dir: bool,
}

/// The variables of a terminal whose folder shares the memory of `root`:
/// where Leon is, where the memory file is, and the data folder when it is
/// not the one a bare `leon` would use.
pub fn terminal_env(terminals: &Terminals, root: &str) -> Vec<(String, String)> {
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let mut env = vec![
        (leon_memory::ENV_BIN.to_owned(), text(&terminals.bin)),
        (
            leon_memory::ENV_MEMORY.to_owned(),
            text(&file_of(&terminals.data_dir, root)),
        ),
    ];
    if !terminals.default_data_dir {
        env.push((
            leon_memory::ENV_DATA_DIR.to_owned(),
            text(&terminals.data_dir),
        ));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_memory::block;

    fn fixed() -> i64 {
        1_791_331_200_000
    }

    struct Rig {
        dir: tempfile::TempDir,
        store: Arc<Store>,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                store: Store::open_in_memory().unwrap(),
            }
        }

        fn data(&self) -> PathBuf {
            self.dir.path().join("data")
        }

        fn project(&self, name: &str) -> PathBuf {
            let path = self.dir.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        }

        fn service(&self, root: &str) -> Service {
            Service::new(self.store.clone(), self.data(), root.to_owned()).at(fixed)
        }
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    /// No folder and no git file anywhere.
    struct Nowhere;

    impl Folders for Nowhere {
        fn is_dir(&self, _: &Path) -> bool {
            false
        }
        fn read(&self, _: &Path) -> Option<String> {
            None
        }
    }

    /// One repository, at `/code/api`.
    struct OneRepository;

    impl Folders for OneRepository {
        fn is_dir(&self, path: &Path) -> bool {
            path == Path::new("/code/api/.git")
        }
        fn read(&self, _: &Path) -> Option<String> {
            None
        }
    }

    #[test]
    fn a_folder_shares_with_its_project_then_its_repository_then_itself() {
        let rig = Rig::new();
        rig.store
            .add_project(&MachineId::local(), "api", "/srv/api")
            .unwrap();
        // The store knows it: git is not asked.
        assert_eq!(
            resolve_root(&rig.store, &OneRepository, Path::new("/srv/api/src")),
            "/srv/api"
        );
        // It does not: the repository's root.
        assert_eq!(
            Path::new(&resolve_root(
                &rig.store,
                &OneRepository,
                Path::new("/code/api/src")
            )),
            Path::new("/code/api")
        );
        // No repository: the folder.
        assert_eq!(
            resolve_root(&rig.store, &Nowhere, Path::new("/tmp/scratch")),
            "/tmp/scratch"
        );
    }

    #[test]
    fn the_projects_files_are_at_its_root_or_at_the_top_of_a_worktree_elsewhere() {
        // Inside the project: its root, whatever git says.
        assert_eq!(
            files_root(&OneRepository, "/srv/api", Path::new("/srv/api/src")),
            Path::new("/srv/api")
        );
        // A checkout elsewhere that shares the project's memory: its own top.
        assert_eq!(
            files_root(&OneRepository, "/srv/api", Path::new("/code/api/src")),
            Path::new("/code/api")
        );
        // Nothing says where: the root.
        assert_eq!(
            files_root(&Nowhere, "/srv/api", Path::new("/tmp/x")),
            Path::new("/srv/api")
        );
    }

    #[test]
    fn a_write_puts_the_entry_in_the_projects_file() {
        let rig = Rig::new();
        let service = rig.service("/srv/api");
        let added = service
            .add(&NewMemory::note(service.scope(false), "Use pnpm."))
            .unwrap()
            .into_memory();
        let file = service.file();
        assert!(file.starts_with(rig.data().join("memory")));
        let text = read(&file);
        assert!(text.starts_with("<!-- leon-memory root: /srv/api -->\n"));
        assert!(text.contains("- Use pnpm. ["));
        assert_eq!(text, service.document().unwrap());
        assert_eq!(
            text,
            document(&rig.store, Some("/srv/api"), 12_000).unwrap()
        );
        // No temporary file is left beside it.
        let names: Vec<String> = std::fs::read_dir(rig.data().join("memory"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [scope::file_name("/srv/api")]);

        service.forget(added.short_id()).unwrap();
        assert!(!read(&file).contains("Use pnpm."));
    }

    #[test]
    fn a_global_write_reaches_every_projects_file_that_is_there() {
        let rig = Rig::new();
        let api = rig.service("/srv/api");
        let web = rig.service("/srv/web");
        api.add(&NewMemory::note(api.scope(false), "api fact"))
            .unwrap();
        web.add(&NewMemory::note(web.scope(false), "web fact"))
            .unwrap();
        // A file of somebody else in the folder is not touched.
        let stray = rig.data().join("memory").join("notes.md");
        std::fs::write(&stray, "<!-- leon-memory root: /srv/other -->\nmine\n").unwrap();

        api.add(&NewMemory::note(api.scope(true), "everywhere fact"))
            .unwrap();
        for (file, own, other) in [
            (api.file(), "api fact", "web fact"),
            (web.file(), "web fact", "api fact"),
        ] {
            let text = read(&file);
            assert!(text.contains("everywhere fact"), "{text}");
            assert!(text.contains(own) && !text.contains(other), "{text}");
        }
        let global = read(&api.global_file());
        assert!(global.contains("everywhere fact"));
        assert!(!global.contains("api fact"));
        assert_eq!(
            read(&stray),
            "<!-- leon-memory root: /srv/other -->\nmine\n"
        );
    }

    #[test]
    fn the_file_follows_pins_revisions_forgetting_and_restoring() {
        let rig = Rig::new();
        let service = rig.service("/srv/api");
        let note = |text: &str| NewMemory::note(service.scope(false), text);
        let old = service
            .add(&note("The oldest rule."))
            .unwrap()
            .into_memory();
        service.add(&note("A newer note.")).unwrap();
        let file = service.file();
        let order = |text: &str| (text.find("oldest").unwrap(), text.find("newer").unwrap());

        service.pin(&old.id, true).unwrap();
        let text = read(&file);
        assert!(
            text.contains("### Pinned\n\n- The oldest rule. ["),
            "{text}"
        );
        let (oldest, newer) = order(&text);
        assert!(oldest < newer);
        service.pin(&old.id, false).unwrap();
        assert!(!read(&file).contains("### Pinned"));

        // The same words again change no file and add no entry.
        let before = read(&file);
        assert!(matches!(
            service.add(&note("the OLDEST rule")).unwrap(),
            SavedMemory::Known(_)
        ));
        assert_eq!(read(&file), before);

        let mut topical = note("Ports: 8080.");
        topical.topic = Some("ports".into());
        let first = service.add(&topical).unwrap().into_memory();
        topical.text = "Ports: 9090.".into();
        assert!(matches!(
            service.add(&topical).unwrap(),
            SavedMemory::Revised(_)
        ));
        let text = read(&file);
        assert!(
            text.contains("- Ports: 9090. [") && !text.contains("8080"),
            "{text}"
        );
        assert!(text.contains("topic ports, revision 2"), "{text}");

        service.forget(&first.id).unwrap();
        assert!(!read(&file).contains("9090"));
        assert_eq!(service.forgotten(false, 10).unwrap().len(), 1);
        let (project, _) = service.counts().unwrap();
        assert_eq!((project.live, project.forgotten), (2, 1));
        service.restore(&first.id).unwrap();
        assert!(read(&file).contains("9090"));
        service.delete(&first.id).unwrap();
        assert!(!read(&file).contains("9090"));
        assert!(service.restore(&first.id).is_err());
    }

    #[test]
    fn the_size_of_the_file_is_the_setting_within_its_range_or_the_default() {
        assert_eq!(budget_of(None), 12_000);
        assert_eq!(budget_of(Some(30_000)), 30_000);
        assert_eq!(budget_of(Some(2_000)), 2_000);
        assert_eq!(budget_of(Some(200_000)), 200_000);
        // Out of range: the nearest bound, as every number of the settings.
        assert_eq!(budget_of(Some(10)), 2_000);
        assert_eq!(budget_of(Some(-5)), 2_000);
        assert_eq!(budget_of(Some(i64::MAX)), 200_000);

        let rig = Rig::new();
        let data = rig.data();
        std::fs::create_dir_all(&data).unwrap();
        let file = data.join(crate::settings::FILE_NAME);
        assert_eq!(budget_in(&data), 12_000, "no settings file");
        for (text, expected) in [
            (r#"{"memory_budget": 40000}"#, 40_000),
            (r#"{"memory_budget": 5}"#, 2_000),
            (r#"{"memory_budget": 99999999}"#, 200_000),
            (r#"{"memory_budget": "large"}"#, 12_000),
            (r#"{"memory_budget": null}"#, 12_000),
            (r#"{"theme": "dark"}"#, 12_000),
            ("{ not json", 12_000),
            ("", 12_000),
        ] {
            std::fs::write(&file, text).unwrap();
            assert_eq!(budget_in(&data), expected, "{text}");
        }
        // A file that cannot be used never fails a write.
        std::fs::write(&file, "{ not json").unwrap();
        let service = rig.service("/srv/api");
        assert!(service
            .add(&NewMemory::note(service.scope(false), "Use pnpm."))
            .is_ok());
        assert!(read(&service.file()).contains("Use pnpm."));
    }

    #[test]
    fn the_file_is_as_large_as_the_setting_lets_it_be() {
        let rig = Rig::new();
        std::fs::create_dir_all(rig.data()).unwrap();
        let settings = rig.data().join(crate::settings::FILE_NAME);
        let fill = rig.service("/srv/api");
        for n in 0..300 {
            let text = format!("fact {n} {}", "word ".repeat(50));
            fill.add(&NewMemory::note(fill.scope(false), text)).unwrap();
        }
        let size = |budget: Option<i64>| {
            match budget {
                Some(budget) => {
                    std::fs::write(&settings, format!(r#"{{"memory_budget": {budget}}}"#)).unwrap()
                }
                None => std::fs::remove_file(&settings).unwrap(),
            }
            // A service of its own, as each run of the command line is.
            let service = rig.service("/srv/api");
            service.refresh(&service.scope(false));
            read(&service.file()).len()
        };
        let small = size(Some(3_000));
        let large = size(Some(60_000));
        let default = size(None);
        assert!(small <= 3_000 && small > 2_000, "{small}");
        assert!(default <= 12_000 && default > 11_000, "{default}");
        assert!(large <= 60_000 && large > 50_000, "{large}");
    }

    #[test]
    fn one_entry_is_read_whole_when_it_is_this_folders() {
        let rig = Rig::new();
        let api = rig.service("/srv/api");
        let web = rig.service("/srv/web");
        let long = format!("A long fact.\n{}", "more of it. ".repeat(100));
        let mine = api
            .add(&NewMemory::note(api.scope(false), &long))
            .unwrap()
            .into_memory();
        let global = web
            .add(&NewMemory::note(web.scope(true), "shared"))
            .unwrap()
            .into_memory();
        let theirs = web
            .add(&NewMemory::note(web.scope(false), "web fact"))
            .unwrap()
            .into_memory();
        // Live, by its id or the end of it: all of the text.
        assert_eq!(api.get(mine.short_id()).unwrap().text, long.trim());
        assert_eq!(api.get(&global.id).unwrap().text, "shared");
        // Forgotten: still read, and it says so.
        api.forget(&mine.id).unwrap();
        let forgotten = api.get(&mine.id).unwrap();
        assert!(forgotten.is_forgotten());
        assert!(render::detail(&forgotten).contains("forgotten: "));
        // Unknown, and another project's: not found, in the same words.
        for id in ["ffffffff", theirs.id.as_str()] {
            let refused = api.get(id).unwrap_err();
            assert_eq!(sentence(&refused), "no entry of this memory has that id");
        }
    }

    #[test]
    fn forgetting_purges_what_was_forgotten_long_ago() {
        let rig = Rig::new();
        let service = rig.service("/srv/api");
        let old = service
            .add(&NewMemory::note(service.scope(false), "old"))
            .unwrap()
            .into_memory();
        let new = service
            .add(&NewMemory::note(service.scope(false), "new"))
            .unwrap()
            .into_memory();
        // Forgotten 31 days before the service's clock.
        rig.store
            .forget_memory(&old.id, fixed() - 31 * 86_400_000)
            .unwrap();
        assert_eq!(service.purge(60).unwrap(), 0);
        service.forget(&new.id).unwrap();
        assert!(rig.store.memory(&old.id).is_err(), "purged on the way");
        assert!(rig.store.memory(&new.id).is_ok(), "just forgotten: kept");
        assert_eq!(
            service.purge(0).unwrap(),
            0,
            "forgotten at this very moment"
        );
    }

    #[test]
    fn another_projects_entry_is_not_this_folders_to_forget() {
        let rig = Rig::new();
        let api = rig.service("/srv/api");
        let web = rig.service("/srv/web");
        let theirs = web
            .add(&NewMemory::note(web.scope(false), "web fact"))
            .unwrap()
            .into_memory();
        let global = web
            .add(&NewMemory::note(web.scope(true), "shared"))
            .unwrap()
            .into_memory();
        // Not to pin, change or remove for good either.
        assert!(api.pin(&theirs.id, true).is_err());
        assert!(api.delete(&theirs.id).is_err());
        let patch = MemoryPatch {
            kind: Some(MemoryKind::Decision),
            ..MemoryPatch::default()
        };
        assert!(api.update(&theirs.id, &patch).is_err());
        assert!(matches!(
            api.forget(&theirs.id),
            Err(StoreError::NotFound("memory"))
        ));
        assert!(api.forget(&global.id).is_ok());
        assert!(web.forget(&theirs.id).is_ok());
    }

    #[test]
    fn the_block_goes_into_the_files_that_exist_and_comes_out_byte_for_byte() {
        let rig = Rig::new();
        let root = rig.project("api");
        let agents = "# Agents\r\n\r\nUse pnpm.\r\n";
        let claude = "# Claude\n\nno final newline";
        std::fs::write(root.join("AGENTS.md"), agents).unwrap();
        std::fs::write(root.join("CLAUDE.md"), claude).unwrap();

        let dry = set_block(&root, true, true).unwrap();
        assert_eq!(dry.len(), 2);
        assert!(dry[0].starts_with("would write ") && dry[0].ends_with("AGENTS.md"));
        assert_eq!(read(&root.join("AGENTS.md")), agents);
        assert!(block_files(&root).is_empty());

        let done = set_block(&root, true, false).unwrap();
        assert!(done[0].starts_with("wrote ") && done[1].ends_with("CLAUDE.md"));
        assert_eq!(block_files(&root), ["AGENTS.md", "CLAUDE.md"]);
        assert!(read(&root.join("AGENTS.md")).starts_with(agents));
        assert!(!root.join("GEMINI.md").exists());
        // Again: nothing to do.
        let again = set_block(&root, true, false).unwrap();
        assert!(again
            .iter()
            .all(|line| line.contains("already has the block")));

        set_block(&root, false, false).unwrap();
        assert_eq!(read(&root.join("AGENTS.md")), agents);
        assert_eq!(read(&root.join("CLAUDE.md")), claude);
        assert!(set_block(&root, false, false).unwrap().is_empty());
    }

    #[test]
    fn a_project_without_instruction_files_gets_agents_md_and_loses_it_again() {
        let rig = Rig::new();
        let root = rig.project("fresh");
        let done = set_block(&root, true, false).unwrap();
        assert_eq!(done.len(), 1);
        assert!(done[0].starts_with("created "));
        assert!(block::current(&read(&root.join("AGENTS.md"))));
        let undone = set_block(&root, false, false).unwrap();
        assert!(undone[0].starts_with("removed "));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);

        // A file that was there and empty is not Leon's to remove: it is
        // empty again.
        std::fs::write(root.join("AGENTS.md"), "").unwrap();
        assert!(set_block(&root, true, false).unwrap()[0].starts_with("wrote "));
        assert!(set_block(&root, false, false).unwrap()[0].starts_with("wrote "));
        assert_eq!(read(&root.join("AGENTS.md")), "");
        assert!(set_block(&rig.dir.path().join("absent"), true, false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_written_once_through_its_file_and_never_out_of_the_project() {
        use std::os::unix::fs::symlink;
        let rig = Rig::new();
        let root = rig.project("linked");
        let outside = rig.dir.path().join("outside.md");
        std::fs::write(&outside, "not the project's\n").unwrap();
        std::fs::write(root.join("AGENTS.md"), "# Agents\n").unwrap();
        symlink("AGENTS.md", root.join("CLAUDE.md")).unwrap();
        symlink(&outside, root.join("GEMINI.md")).unwrap();

        let done = set_block(&root, true, false).unwrap();
        assert!(done[0].starts_with("wrote "));
        assert!(done[1].contains("CLAUDE.md alone: it is a link to AGENTS.md"));
        assert!(done[2].contains("GEMINI.md alone") && done[2].contains("does not follow"));
        // The link is still a link, and reads the block through its file.
        assert!(std::fs::symlink_metadata(root.join("CLAUDE.md"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(read(&root.join("AGENTS.md")).matches(block::END).count(), 1);
        assert_eq!(read(&outside), "not the project's\n");

        set_block(&root, false, false).unwrap();
        assert_eq!(read(&root.join("AGENTS.md")), "# Agents\n");
        assert_eq!(read(&outside), "not the project's\n");

        // A link that is all there is: nothing is created beside it, and
        // nothing is written through it.
        let only = rig.project("only-a-link");
        symlink(&outside, only.join("AGENTS.md")).unwrap();
        let done = set_block(&only, true, false).unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(read(&outside), "not the project's\n");
        assert!(!only.join("CLAUDE.md").exists());
    }

    #[test]
    fn the_server_is_merged_into_mcp_json_and_taken_out_byte_for_byte() {
        let rig = Rig::new();
        let root = rig.project("api");
        let path = root.join(".mcp.json");
        assert_eq!(mcp_registered(&root), Ok(false));
        assert_eq!(set_mcp_json(&root, false, false), Ok(None));

        // No file: one is created, and removed again.
        let dry = set_mcp_json(&root, true, true).unwrap().unwrap();
        assert!(dry.starts_with("would create "));
        assert!(!path.exists());
        assert!(set_mcp_json(&root, true, false)
            .unwrap()
            .unwrap()
            .starts_with("created "));
        assert_eq!(mcp_registered(&root), Ok(true));
        assert_eq!(set_mcp_json(&root, true, false), Ok(None));
        assert!(set_mcp_json(&root, false, false)
            .unwrap()
            .unwrap()
            .starts_with("removed "));
        assert!(!path.exists());

        // A file with other servers keeps them, and comes back as it was.
        let before = "{\n    \"mcpServers\": {\n        \"zeta\": {\n            \"command\": \"z\"\n        }\n    },\n    \"extra\": 1\n}\n";
        std::fs::write(&path, before).unwrap();
        assert!(set_mcp_json(&root, true, false)
            .unwrap()
            .unwrap()
            .starts_with("wrote "));
        assert!(read(&path).contains("\"zeta\"") && read(&path).contains("\"leon-memory\""));
        set_mcp_json(&root, false, false).unwrap();
        assert_eq!(read(&path), before);
    }

    #[test]
    fn an_mcp_json_that_does_not_parse_is_refused_and_left_as_it_is() {
        let rig = Rig::new();
        let root = rig.project("api");
        let path = root.join(".mcp.json");
        std::fs::write(&path, "{ \"mcpServers\": { broken").unwrap();
        for on in [true, false] {
            let refused = set_mcp_json(&root, on, false).unwrap_err();
            assert!(refused.contains("was left alone"), "{refused}");
            assert!(refused.contains("not valid JSON"), "{refused}");
        }
        assert!(mcp_registered(&root).is_err());
        assert_eq!(read(&path), "{ \"mcpServers\": { broken");
    }

    #[test]
    fn a_terminal_is_told_where_leon_and_the_memory_file_are() {
        let mut terminals = Terminals {
            bin: PathBuf::from("/opt/leon/leon"),
            data_dir: PathBuf::from("/data/leon"),
            default_data_dir: true,
        };
        let env = terminal_env(&terminals, "/srv/api");
        assert_eq!(env.len(), 2);
        assert_eq!(env[0], ("LEON_BIN".to_owned(), "/opt/leon/leon".to_owned()));
        assert_eq!(env[1].0, "LEON_MEMORY");
        assert_eq!(
            Path::new(&env[1].1),
            Path::new("/data/leon/memory").join(scope::file_name("/srv/api"))
        );
        // Another data folder is said too, so `"$LEON_BIN" memory` finds it.
        terminals.default_data_dir = false;
        let env = terminal_env(&terminals, "/srv/api");
        assert_eq!(
            env[2],
            ("LEON_DATA_DIR".to_owned(), "/data/leon".to_owned())
        );
    }

    #[test]
    fn an_entry_prints_as_json_with_its_scope_and_its_plain_snippet() {
        let rig = Rig::new();
        let service = rig.service("/srv/api");
        let added = service
            .add(&NewMemory::note(service.scope(false), "Use pnpm."))
            .unwrap()
            .into_memory();
        let value = json_of(&added, Some("Use \u{2}pnpm\u{3}."));
        assert_eq!(value["scope"], "project");
        assert_eq!(value["root"], "/srv/api");
        assert_eq!(value["kind"], "note");
        assert_eq!(value["snippet"], "Use pnpm.");
        assert_eq!(value["revision"], 1);
        assert_eq!(value["pinned"], false);
        assert_eq!(value["duplicates"], 0);
        assert!(value["topic"].is_null() && value["forgotten_at"].is_null());
        assert_eq!(value["last_seen_at"], fixed());
        assert_eq!(value["updated_at"], fixed());
        assert_eq!(json_of(&added, None).get("snippet"), None);
    }
}

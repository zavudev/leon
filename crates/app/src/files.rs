//! Files on this computer: the local twin of [`leon_remote::files`].
//!
//! The same operations answer in the same shapes ([`FileContent`],
//! [`WriteOutcome`], [`FileEntry`], [`GitMarks`]), done with `std::fs` instead
//! of a script, so the code above neither knows nor cares which machine a file
//! is on. Nothing here needs a shell, so it works on Windows too.
//!
//! A revision is `L <modified> <size> <hash>`: when the file was last modified
//! in nanoseconds, its size and the SHA-256 of its bytes. A save is refused
//! when the size or the hash differ from what was read; a file that was only
//! touched is the same file.
//!
//! A save writes a temporary file in the same folder, gives it the mode of the
//! original and renames it over the file, so a crash never leaves half a
//! file. A symbolic link is followed to its target, which is what gets
//! replaced, and the link stays a link. The check and the rename are two
//! steps: a change in between is not seen.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use leon_remote::CommandSpec;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub use leon_remote::files::{
    sort_entries, EntryKind, FileContent, FileEntry, FileRevision, GitMark, GitMarks, WriteOutcome,
    MAX_FILE_BYTES, MAX_LIST_ENTRIES,
};

/// How deep a walk of a folder that is not a repository goes.
const WALK_DEPTH: usize = 32;

/// The folders a walk leaves alone: version control and the heavy build or
/// dependency folders of the common toolchains. The same ones the remote
/// `find` prunes. Hidden files stay in: a `.github/` holds files worth
/// opening.
const WALK_SKIPPED: [&str; 10] = [
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".cache",
];

/// Why a file operation could not be done.
#[derive(Debug, Error)]
pub enum FileError {
    /// There is nothing at the path.
    #[error("{0} does not exist.")]
    Missing(String),
    /// The path is not a regular file.
    #[error("{0} is not a file.")]
    NotAFile(String),
    /// The path is not a folder.
    #[error("{0} is not a folder.")]
    NotADir(String),
    /// The contents are above [`MAX_FILE_BYTES`].
    #[error("{0} is too big to save: the most Leon writes is {MAX_FILE_BYTES} bytes.")]
    TooBig(String),
    /// The operating system refused.
    #[error("Cannot {action} {path}: {source}.")]
    Io {
        /// What was being done, as a verb.
        action: &'static str,
        /// The path it was done to.
        path: String,
        /// What the system said.
        source: std::io::Error,
    },
}

fn io_error(action: &'static str, path: &Path, source: std::io::Error) -> FileError {
    let text = path.display().to_string();
    if source.kind() == std::io::ErrorKind::NotFound && action == "read" {
        return FileError::Missing(text);
    }
    FileError::Io {
        action,
        path: text,
        source,
    }
}

/// Reads a file: its text, or why it is not shown as text.
pub fn read(path: &Path) -> Result<FileContent, FileError> {
    let shown = path.display().to_string();
    let file = std::fs::File::open(path).map_err(|e| io_error("read", path, e))?;
    let meta = file.metadata().map_err(|e| io_error("read", path, e))?;
    if !meta.is_file() {
        return Err(FileError::NotAFile(shown));
    }
    if meta.len() > MAX_FILE_BYTES as u64 {
        return Ok(FileContent::TooBig { size: meta.len() });
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, MAX_FILE_BYTES as u64 + 1),
        &mut bytes,
    )
    .map_err(|e| io_error("read", path, e))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Ok(FileContent::TooBig {
            size: bytes.len() as u64,
        });
    }
    let size = bytes.len() as u64;
    if bytes.contains(&0) {
        return Ok(FileContent::Binary { size });
    }
    let revision = revision_of(&meta, &bytes);
    // A byte order mark and carriage returns stay in the text.
    Ok(match String::from_utf8(bytes) {
        Ok(text) => FileContent::Text { text, revision },
        Err(_) => FileContent::Binary { size },
    })
}

/// Saves `contents` to `path`. `expected` is the revision the file was read
/// with; `None` creates a file that must not exist yet. A file that is not
/// what was read is a [`WriteOutcome::Conflict`], not an error.
pub fn write(
    path: &Path,
    expected: Option<&FileRevision>,
    contents: &[u8],
) -> Result<WriteOutcome, FileError> {
    let shown = path.display().to_string();
    if contents.len() > MAX_FILE_BYTES {
        return Err(FileError::TooBig(shown));
    }
    // A link is followed, so the target is what gets replaced.
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    let target = if is_link {
        std::fs::canonicalize(path).map_err(|e| io_error("save", path, e))?
    } else {
        path.to_path_buf()
    };
    let Some(want) = expected else {
        return create(&target, contents);
    };
    let meta = match std::fs::metadata(&target) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(WriteOutcome::Conflict),
        Err(e) => return Err(io_error("save", path, e)),
    };
    if !meta.is_file() {
        return Err(FileError::NotAFile(shown));
    }
    if meta.len() > MAX_FILE_BYTES as u64 {
        return Ok(WriteOutcome::Conflict);
    }
    let current = std::fs::read(&target).map_err(|e| io_error("save", path, e))?;
    if !same_content(want, current.len() as u64, &hash(&current)) {
        return Ok(WriteOutcome::Conflict);
    }
    let folder = target.parent().unwrap_or_else(|| Path::new("."));
    let temp = temp_name(folder);
    let saved = write_temp(&temp, &meta, contents).and_then(|()| std::fs::rename(&temp, &target));
    if let Err(source) = saved {
        let _ = std::fs::remove_file(&temp);
        return Err(io_error("save", path, source));
    }
    after_write(&target, path, contents)
}

/// Creates a file that must not exist.
fn create(target: &Path, contents: &[u8]) -> Result<WriteOutcome, FileError> {
    let opened = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target);
    let mut file = match opened {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Ok(WriteOutcome::Conflict)
        }
        Err(e) => return Err(io_error("save", target, e)),
    };
    if let Err(source) = file.write_all(contents).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(target);
        return Err(io_error("save", target, source));
    }
    after_write(target, target, contents)
}

fn after_write(target: &Path, path: &Path, contents: &[u8]) -> Result<WriteOutcome, FileError> {
    let meta = std::fs::metadata(target).map_err(|e| io_error("save", path, e))?;
    Ok(WriteOutcome::Saved(revision_of(&meta, contents)))
}

/// Writes the temporary file next to the original, with the original's mode.
fn write_temp(temp: &Path, original: &std::fs::Metadata, contents: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)?;
    file.write_all(contents)?;
    file.set_permissions(original.permissions())?;
    file.sync_all()
}

fn temp_name(folder: &Path) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    folder.join(format!(
        ".leon-save.{}.{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn revision_of(meta: &std::fs::Metadata, bytes: &[u8]) -> FileRevision {
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    FileRevision::new(format!("L {modified} {} {}", bytes.len(), hash(bytes)))
}

/// Whether `want` (a revision made here) names a file of this size and hash.
/// The modification time does not count: touching a file changes nothing.
fn same_content(want: &FileRevision, size: u64, hash: &str) -> bool {
    let mut parts = want.as_str().split(' ');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()),
        (Some("L"), Some(_), Some(have_size), Some(have_hash), None)
            if have_size.parse() == Ok(size) && have_hash == hash
    )
}

/// The entries of one folder, folders first and then by name. A link shows
/// as what it points at, and is marked as a link.
pub fn list_dir(path: &Path) -> Result<Vec<FileEntry>, FileError> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(FileError::NotADir(path.display().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(FileError::Missing(path.display().to_string()))
        }
        Err(e) => return Err(io_error("list", path, e)),
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| io_error("list", path, e))? {
        // An entry that vanished or cannot be read is left out.
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let symlink = file_type.is_symlink();
        let meta = if symlink {
            std::fs::metadata(entry.path()).ok()
        } else {
            entry.metadata().ok()
        };
        let (kind, size) = match &meta {
            Some(meta) if meta.is_dir() => (EntryKind::Dir, None),
            Some(meta) if meta.is_file() => (EntryKind::File, Some(meta.len())),
            _ => (EntryKind::Other, None),
        };
        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            kind,
            symlink,
            size,
        });
        if entries.len() >= MAX_LIST_ENTRIES {
            break;
        }
    }
    sort_entries(&mut entries);
    Ok(entries)
}

/// The command that asks git for the files of a project: tracked and
/// untracked-not-ignored, NUL separated. No shell is involved.
pub fn project_files_command(root: &str) -> CommandSpec {
    CommandSpec::new("git")
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .cwd(root)
}

/// The command that asks git what changed under `root`, a work tree's root or
/// a folder inside one. Its paths are from the work tree's root, which
/// [`git_prefix_command`] says how to undo.
pub fn git_marks_command(root: &str) -> CommandSpec {
    CommandSpec::new("git")
        .args(["status", "--porcelain=v1", "-z", "--ignored"])
        .cwd(root)
}

/// The command that asks git where `root` is in its work tree: empty at the
/// root, else `sub/dir/`.
pub fn git_prefix_command(root: &str) -> CommandSpec {
    CommandSpec::new("git")
        .args(["rev-parse", "--show-prefix"])
        .cwd(root)
}

/// Every file under `root` as relative paths with `/` separators, for a
/// folder git does not know. Links are not followed, the folders of
/// [`WALK_SKIPPED`] are left out, and the result is sorted.
pub fn walk(root: &Path) -> Result<Vec<String>, FileError> {
    if !root.is_dir() {
        return Err(FileError::NotADir(root.display().to_string()));
    }
    let mut paths = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth >= WALK_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !WALK_SKIPPED.contains(&entry.file_name().to_string_lossy().as_ref()) {
                    stack.push((entry.path(), depth + 1));
                }
            } else if kind.is_file() {
                if let Ok(relative) = entry.path().strip_prefix(root) {
                    paths.push(relative.to_string_lossy().replace('\\', "/"));
                    if paths.len() >= MAX_LIST_ENTRIES {
                        paths.sort();
                        return Ok(paths);
                    }
                }
            }
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(content: FileContent) -> (String, FileRevision) {
        match content {
            FileContent::Text { text, revision } => (text, revision),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn a_file_is_read_with_its_bom_and_line_endings_and_saved_back_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a b's.txt");
        let original = "\u{feff}uno\r\ndos\r\n";
        std::fs::write(&path, original).unwrap();
        let (text, revision) = text_of(read(&path).unwrap());
        assert_eq!(text, original);

        let newer = "\u{feff}tres\r\n";
        let WriteOutcome::Saved(after) = write(&path, Some(&revision), newer.as_bytes()).unwrap()
        else {
            panic!("saved");
        };
        assert_eq!(std::fs::read(&path).unwrap(), newer.as_bytes());
        assert_eq!(text_of(read(&path).unwrap()).1, after);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temp");
    }

    #[test]
    fn a_save_is_refused_when_the_content_moved_but_not_when_only_touched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("n.md");
        std::fs::write(&path, "one\n").unwrap();
        let (_, revision) = text_of(read(&path).unwrap());

        // Same bytes written again: a different time, the same file.
        std::fs::write(&path, "one\n").unwrap();
        let WriteOutcome::Saved(second) = write(&path, Some(&revision), b"two\n").unwrap() else {
            panic!("a touched file is still the file that was read");
        };
        assert_ne!(second, revision);

        // The first revision no longer describes it.
        assert_eq!(
            write(&path, Some(&revision), b"mine\n").unwrap(),
            WriteOutcome::Conflict
        );
        // Same size, other bytes: only the hash can tell.
        std::fs::write(&path, "twp\n").unwrap();
        assert_eq!(
            write(&path, Some(&second), b"mine\n").unwrap(),
            WriteOutcome::Conflict
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "twp\n");

        // Gone, or never read as a revision of ours.
        assert_eq!(
            write(&path, Some(&FileRevision::new("123 4")), b"x").unwrap(),
            WriteOutcome::Conflict
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            write(&path, Some(&second), b"x").unwrap(),
            WriteOutcome::Conflict
        );
        assert!(!path.exists());
    }

    #[test]
    fn creating_a_file_refuses_one_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        assert!(matches!(
            write(&path, None, b"hi\n").unwrap(),
            WriteOutcome::Saved(_)
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi\n");
        assert_eq!(
            write(&path, None, b"again").unwrap(),
            WriteOutcome::Conflict
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi\n");
        let nowhere = dir.path().join("no/such/folder.txt");
        assert!(matches!(
            write(&nowhere, None, b"x"),
            Err(FileError::Io { .. })
        ));
    }

    #[test]
    fn reading_tells_missing_big_binary_and_folders_apart() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            read(&dir.path().join("nope")),
            Err(FileError::Missing(_))
        ));
        assert!(matches!(
            read(dir.path()),
            Err(FileError::NotAFile(_)) | Err(FileError::Io { .. })
        ));

        let big = dir.path().join("big.txt");
        std::fs::write(&big, vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        assert_eq!(
            read(&big).unwrap(),
            FileContent::TooBig {
                size: MAX_FILE_BYTES as u64 + 1
            }
        );
        assert!(matches!(
            write(&big, None, &vec![b'x'; MAX_FILE_BYTES + 1]),
            Err(FileError::TooBig(_))
        ));

        let nul = dir.path().join("nul.bin");
        std::fs::write(&nul, [b'a', 0, b'b']).unwrap();
        assert_eq!(read(&nul).unwrap(), FileContent::Binary { size: 3 });
        let latin = dir.path().join("latin.txt");
        std::fs::write(&latin, [0xe9, b'a']).unwrap();
        assert_eq!(read(&latin).unwrap(), FileContent::Binary { size: 2 });
        let empty = dir.path().join("empty");
        std::fs::write(&empty, "").unwrap();
        assert_eq!(text_of(read(&empty).unwrap()).0, "");
    }

    #[cfg(unix)]
    #[test]
    fn a_save_keeps_the_mode_and_goes_through_a_link_to_its_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("run.sh");
        std::fs::write(&real, "old\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o750)).unwrap();
        let link = dir.path().join("link.sh");
        symlink("run.sh", &link).unwrap();
        let (_, revision) = text_of(read(&link).unwrap());
        assert!(matches!(
            write(&link, Some(&revision), b"new\n").unwrap(),
            WriteOutcome::Saved(_)
        ));
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new\n");
        assert_eq!(
            std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o750
        );
        // A link to nowhere cannot be saved through.
        let dangling = dir.path().join("dangling");
        symlink("nowhere", &dangling).unwrap();
        assert!(write(&dangling, None, b"x").is_err());
    }

    #[test]
    fn a_folder_lists_one_level_with_kinds_sizes_and_links() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Sub dir")).unwrap();
        std::fs::write(dir.path().join("Sub dir/inner.txt"), "x").unwrap();
        std::fs::write(dir.path().join("b.txt"), "12345").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("b.txt", dir.path().join("alias")).unwrap();
            std::os::unix::fs::symlink("nowhere", dir.path().join("dangling")).unwrap();
        }
        let entries = list_dir(dir.path()).unwrap();
        assert_eq!(entries[0].name, "Sub dir");
        assert_eq!(entries[0].kind, EntryKind::Dir);
        let find = |name: &str| entries.iter().find(|e| e.name == name).unwrap();
        assert_eq!(find("b.txt").size, Some(5));
        assert_eq!(find(".hidden").size, Some(0));
        #[cfg(unix)]
        {
            assert!(find("alias").symlink);
            assert_eq!(find("alias").size, Some(5));
            assert_eq!(find("dangling").kind, EntryKind::Other);
            assert_eq!(entries.len(), 5);
        }
        assert!(matches!(
            list_dir(&dir.path().join("gone")),
            Err(FileError::Missing(_))
        ));
        assert!(matches!(
            list_dir(&dir.path().join("b.txt")),
            Err(FileError::NotADir(_))
        ));
    }

    #[test]
    fn walking_skips_the_heavy_folders_and_returns_sorted_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            "a.md",
            "sub/b c.md",
            ".github/ci.md",
            "target/x.md",
            "sub/node_modules/y.md",
        ] {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        assert_eq!(
            walk(dir.path()).unwrap(),
            [".github/ci.md", "a.md", "sub/b c.md"]
        );
        assert!(matches!(
            walk(&dir.path().join("a.md")),
            Err(FileError::NotADir(_))
        ));
    }

    #[test]
    fn the_git_commands_run_in_the_project_root_without_a_shell() {
        let files = project_files_command("/srv/my api");
        assert_eq!(files.program, "git");
        assert_eq!(files.cwd.as_deref(), Some("/srv/my api"));
        assert_eq!(files.args, ["ls-files", "-co", "--exclude-standard", "-z"]);
        let marks = git_marks_command("/srv/api");
        assert_eq!(marks.args, ["status", "--porcelain=v1", "-z", "--ignored"]);
        let prefix = git_prefix_command("/srv/api/app");
        assert_eq!(prefix.args, ["rev-parse", "--show-prefix"]);
        assert_eq!(prefix.cwd.as_deref(), Some("/srv/api/app"));
    }
}

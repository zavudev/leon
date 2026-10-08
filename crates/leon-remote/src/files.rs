//! Reading, writing and listing files on a machine.
//!
//! Every operation is a short POSIX `sh` script described once as a
//! [`CommandSpec`], the way [`crate::git`] describes git: it runs over SSH and
//! through a relay alike, because both execute a command as written on the
//! machine. The path is an argument of the script (`$1`), never part of its
//! text, so it travels shell-quoted and a name with spaces or quotes is just a
//! name. The answers are read by pure parsers, which carry most of the tests.
//!
//! Each script prints [`MARKER`] before its answer. A login banner that an SSH
//! server or a shell profile prints first is skipped by looking for that line.
//!
//! * **Read** ([`read_command`], [`parse_read`]): the size is checked against
//!   [`MAX_FILE_BYTES`] before anything is read, a binary file is reported
//!   instead of sent, and a text file travels base64-encoded together with its
//!   revision, the `cksum` of its bytes.
//! * **Write** ([`write_command`], [`parse_write`]): the new bytes are the
//!   command's standard input. The script checks that the file still has the
//!   revision that was read, writes a temporary file in the same folder, gives
//!   it the mode of the original and renames it over the file. A revision that
//!   moved prints `CONFLICT` and nothing is written.
//! * **List** ([`list_command`], [`parse_dir`]): the entries of one folder,
//!   asked for lazily, folder by folder.
//! * **Git marks** ([`git_marks_command`], [`parse_marks`]) and the **project
//!   file list** ([`project_files_command`], [`parse_project_files`]).
//!
//! The types are shared with the application's local twin, which does the same
//! with `std::fs` and answers in the same shapes.

use std::collections::BTreeMap;

use base64::Engine as _;

use crate::command::CommandSpec;

/// The most bytes of a file Leon reads or writes. Twice this in base64 plus the
/// command's own output stays below `leon_wire::MAX_EXEC_OUTPUT`, and the
/// bytes of a write stay below `leon_wire::MAX_EXEC_INPUT`.
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// The most entries one folder listing, and the most paths one project file
/// list, returns.
pub const MAX_LIST_ENTRIES: usize = 50_000;

/// The most bytes of a project file list the script prints, so the answer
/// stays below the output cap of a command; a path cut by it is dropped.
pub(crate) const MAX_LIST_BYTES: usize = 3_500_000;

/// The line every script prints before its answer.
const MARKER: &str = "LEON-FILE 1";

// ----- shared types ---------------------------------------------------------

/// What a file looked like when it was read: the thing a save insists on.
///
/// The text is opaque to everyone but whoever made it: the `cksum` of the
/// bytes on a remote machine, modification time, size and hash on this one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileRevision(String);

impl FileRevision {
    /// A revision from its text.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text of the revision.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What reading a file gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileContent {
    /// UTF-8 text, exactly as stored: a byte order mark and carriage returns
    /// are part of it.
    Text {
        /// The text.
        text: String,
        /// The revision the bytes had when they were read.
        revision: FileRevision,
    },
    /// The file holds bytes that are not text.
    Binary {
        /// Its size in bytes.
        size: u64,
    },
    /// The file is larger than [`MAX_FILE_BYTES`].
    TooBig {
        /// Its size in bytes.
        size: u64,
    },
}

/// What a save came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// The file holds the new bytes, with this revision.
    Saved(FileRevision),
    /// The file is not what was read (somebody changed it, it is gone, or it
    /// exists although the save meant to create it); nothing was written.
    Conflict,
}

/// What a directory entry is, following a symbolic link to what it points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    /// A folder.
    Dir,
    /// A regular file.
    File,
    /// Anything else, including a link that points nowhere.
    Other,
}

/// One entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// The file name, without the folder.
    pub name: String,
    /// What it is.
    pub kind: EntryKind,
    /// Whether the entry is a symbolic link.
    pub symlink: bool,
    /// The size of a regular file; absent for the rest and when unknown.
    pub size: Option<u64>,
}

/// What git says about a path. Later variants win when a folder rolls up the
/// marks of what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GitMark {
    /// Ignored by git.
    Ignored,
    /// Not tracked.
    Untracked,
    /// Deleted.
    Deleted,
    /// Renamed or copied.
    Renamed,
    /// Added to the index.
    Added,
    /// Modified (or its type changed).
    Modified,
    /// In conflict after a merge.
    Conflicted,
}

impl GitMark {
    /// The letter a tree shows for the mark: `M A D R ? U` and `!` for
    /// ignored.
    pub fn letter(self) -> char {
        match self {
            GitMark::Modified => 'M',
            GitMark::Added => 'A',
            GitMark::Deleted => 'D',
            GitMark::Renamed => 'R',
            GitMark::Untracked => '?',
            GitMark::Conflicted => 'U',
            GitMark::Ignored => '!',
        }
    }
}

/// The git marks of a folder's files, with every folder carrying the strongest
/// mark of what it holds (ignored files excepted: they do not color their
/// folder).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitMarks {
    marks: BTreeMap<String, GitMark>,
    /// Folders git reports whole (`?? dir/`, `!! dir/`): what is inside has
    /// the same mark though git lists none of it.
    whole: BTreeMap<String, GitMark>,
}

/// `path` (from the work tree's root) made relative to the folder at `prefix`
/// (`sub/dir/`, or empty for the root): `None` for what lies outside it, and
/// empty for the folder itself.
fn relative_to(path: &str, prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return Some(path.to_owned());
    }
    let folder = prefix.trim_end_matches('/');
    if path == folder {
        return Some(String::new());
    }
    path.strip_prefix(folder)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(str::to_owned)
}

impl GitMarks {
    /// Marks from the output of `git status --porcelain=v1 -z` run at the
    /// root of a work tree.
    pub fn from_status(status: &str) -> Self {
        Self::from_status_in(status, "")
    }

    /// Marks from the same output run in a folder inside a work tree.
    /// `prefix` is where the folder is in the work tree (`git rev-parse
    /// --show-prefix`: empty at the root, else `sub/dir/`). Git names every
    /// path from the work tree's root, so the paths are made relative to the
    /// folder and what lies outside it is left out.
    pub fn from_status_in(status: &str, prefix: &str) -> Self {
        let prefix = prefix.trim_matches(['\n', '\r']);
        let mut marks: BTreeMap<String, GitMark> = BTreeMap::new();
        let mut whole = BTreeMap::new();
        for (path, mark) in parse_status(status) {
            let (path, is_dir) = match path.strip_suffix('/') {
                Some(dir) => (dir.to_owned(), true),
                None => (path, false),
            };
            if path.is_empty() {
                continue;
            }
            let path = match relative_to(&path, prefix) {
                Some(path) => path,
                None => {
                    // A folder git reports whole that holds the folder shown.
                    if is_dir
                        && matches!(mark, GitMark::Untracked | GitMark::Ignored)
                        && prefix.starts_with(&format!("{path}/"))
                    {
                        whole.insert(String::new(), mark);
                    }
                    continue;
                }
            };
            if path.is_empty() {
                // The folder shown is itself reported whole.
                if is_dir && matches!(mark, GitMark::Untracked | GitMark::Ignored) {
                    whole.insert(String::new(), mark);
                }
                continue;
            }
            if is_dir && matches!(mark, GitMark::Untracked | GitMark::Ignored) {
                whole.insert(path.clone(), mark);
            }
            if mark != GitMark::Ignored {
                let mut end = 0;
                while let Some(slash) = path[end..].find('/') {
                    end += slash;
                    if end == 0 {
                        end += 1;
                        continue;
                    }
                    let slot = marks.entry(path[..end].to_owned()).or_insert(mark);
                    *slot = (*slot).max(mark);
                    end += 1;
                }
            }
            let slot = marks.entry(path).or_insert(mark);
            *slot = (*slot).max(mark);
        }
        Self { marks, whole }
    }

    /// The mark of `path` (relative to the root, `/` separated), if any.
    pub fn get(&self, path: &str) -> Option<GitMark> {
        if let Some(mark) = self.marks.get(path) {
            return Some(*mark);
        }
        if let Some(mark) = self.whole.get("") {
            return Some(*mark);
        }
        let mut end = 0;
        while let Some(slash) = path[end..].find('/') {
            end += slash;
            if let Some(mark) = self.whole.get(&path[..end]) {
                return Some(*mark);
            }
            end += 1;
        }
        None
    }

    /// How many paths carry a mark of their own.
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    /// Whether git marks nothing.
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }
}

// ----- read -------------------------------------------------------------------

/// What the read command found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    /// The file exists; see [`FileContent`].
    Found(FileContent),
    /// There is nothing at that path.
    Missing,
    /// The path is not a regular file (a folder, a device).
    NotAFile,
}

/// The command that reads one file: size-checked, binary-checked and
/// base64-encoded.
///
/// The answer is read with [`parse_read`].
pub fn read_command(path: &str) -> CommandSpec {
    let script = READ_SCRIPT.replace("@CAP@", &MAX_FILE_BYTES.to_string());
    CommandSpec::new("sh").args(["-c", script.as_str(), "sh", path])
}

// Everything is computed before the first line is printed, so a failing
// `base64` ends the script without an answer instead of an empty file.
const READ_SCRIPT: &str = r#"f=$1
if [ ! -e "$f" ]; then printf 'LEON-FILE 1\nMISSING\n'; exit 0; fi
if [ ! -f "$f" ]; then printf 'LEON-FILE 1\nNOT-FILE\n'; exit 0; fi
s=$(wc -c < "$f" | tr -d ' ') || exit 6
if [ "$s" -gt @CAP@ ]; then printf 'LEON-FILE 1\nTOO-BIG %s\n' "$s"; exit 0; fi
t=$(tr -d '\000' < "$f" | wc -c | tr -d ' ') || exit 6
if [ "$t" != "$s" ]; then printf 'LEON-FILE 1\nBINARY %s\n' "$s"; exit 0; fi
c=$(cksum < "$f") || exit 6
b=$(base64 < "$f" | tr -d '\n\r') || exit 6
printf 'LEON-FILE 1\nTEXT %s\n%s\n' "$c" "$b"
"#;

/// Reads what the read command printed. `None` when the output is not the
/// script's answer at all (an error of `ssh`, a missing tool).
pub fn parse_read(output: &str) -> Option<ReadOutcome> {
    let mut lines = after_marker(output)?.split('\n');
    let head = lines.next()?.trim_end_matches('\r');
    let (word, rest) = head.split_once(' ').unwrap_or((head, ""));
    match word {
        "MISSING" => Some(ReadOutcome::Missing),
        "NOT-FILE" => Some(ReadOutcome::NotAFile),
        "TOO-BIG" => Some(ReadOutcome::Found(FileContent::TooBig {
            size: rest.parse().ok()?,
        })),
        "BINARY" => Some(ReadOutcome::Found(FileContent::Binary {
            size: rest.parse().ok()?,
        })),
        "TEXT" => {
            let payload = lines.next().unwrap_or_default().trim();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(payload)
                .ok()?;
            let size = bytes.len() as u64;
            // Bytes that are not UTF-8 are bytes the editor cannot show.
            Some(ReadOutcome::Found(match String::from_utf8(bytes) {
                Ok(text) => FileContent::Text {
                    text,
                    revision: FileRevision::new(rest),
                },
                Err(_) => FileContent::Binary { size },
            }))
        }
        _ => None,
    }
}

// ----- write ------------------------------------------------------------------

/// The command that saves one file. `expected` is the revision the file was
/// read with; `None` creates a file that must not exist yet. The new bytes are
/// the command's standard input, as they are.
///
/// The answer is read with [`parse_write`].
pub fn write_command(path: &str, expected: Option<&FileRevision>, contents: &[u8]) -> CommandSpec {
    let want = expected.map_or("", FileRevision::as_str);
    CommandSpec::new("sh")
        .args(["-c", WRITE_SCRIPT, "sh", path, want])
        .stdin(contents)
}

// A link is followed to the file it names (a bounded number of times), so the
// save lands in the target and the link stays a link. `cp -p` gives the
// temporary file the mode, owner and times of the original, and `cat` fills
// it without touching them.
const WRITE_SCRIPT: &str = r#"f=$1
want=$2
n=0
while [ -L "$f" ] && [ "$n" -lt 40 ]; do
  t=$(readlink "$f") || exit 7
  case $t in
    /*) f=$t ;;
    *) case $f in */*) f=${f%/*}/$t ;; *) f=$t ;; esac ;;
  esac
  n=$((n + 1))
done
if [ -L "$f" ]; then echo "too many levels of symbolic links" >&2; exit 7; fi
if [ -e "$f" ]; then
  if [ ! -f "$f" ]; then echo "not a regular file" >&2; exit 8; fi
  have=$(cksum < "$f") || exit 6
  if [ -z "$want" ] || [ "$have" != "$want" ]; then printf 'LEON-FILE 1\nCONFLICT\n'; exit 0; fi
elif [ -n "$want" ]; then
  printf 'LEON-FILE 1\nCONFLICT\n'; exit 0
fi
case $f in */*) d=${f%/*} ;; *) d=. ;; esac
[ -n "$d" ] || d=/
tmp="$d/.leon-save.$$"
if [ -e "$f" ]; then cp -p "$f" "$tmp" || exit 5; else : > "$tmp" || exit 5; fi
if ! cat > "$tmp"; then rm -f "$tmp"; exit 5; fi
if ! mv -f "$tmp" "$f"; then rm -f "$tmp"; exit 5; fi
printf 'LEON-FILE 1\nOK %s\n' "$(cksum < "$f")"
"#;

/// Reads what the write command printed. `None` when the output is not the
/// script's answer at all.
pub fn parse_write(output: &str) -> Option<WriteOutcome> {
    let head = after_marker(output)?
        .split('\n')
        .next()?
        .trim_end_matches('\r');
    if head == "CONFLICT" {
        return Some(WriteOutcome::Conflict);
    }
    let revision = head.strip_prefix("OK ")?;
    Some(WriteOutcome::Saved(FileRevision::new(revision)))
}

// ----- list -------------------------------------------------------------------

/// What the listing command found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirListing {
    /// The entries, folders first and then by name.
    Entries(Vec<FileEntry>),
    /// There is nothing at that path.
    Missing,
    /// The path is not a folder.
    NotADir,
}

/// The command that lists the entries of one folder: no recursion, folders
/// and files with their kind, whether they are links, and the size of regular
/// files.
///
/// The answer is read with [`parse_dir`].
pub fn list_command(dir: &str) -> CommandSpec {
    let script = LIST_SCRIPT.replace("@CAP@", &MAX_LIST_ENTRIES.to_string());
    CommandSpec::new("sh").args(["-c", script.as_str(), "sh", dir])
}

// One line per entry, `<kind><link> <name>`, kind `d` `f` or `o` and link `l`
// or `-`. Sizes come from a single `wc` over all the files, in the same order.
// A name with a newline in it cannot be told apart in the answer and is left
// out. Tests are builtins, so a folder of thousands costs two processes.
const LIST_SCRIPT: &str = r#"d=$1
if [ ! -e "$d" ]; then printf 'LEON-FILE 1\nMISSING\n'; exit 0; fi
if [ ! -d "$d" ]; then printf 'LEON-FILE 1\nNOT-DIR\n'; exit 0; fi
nl=$(printf '\n_'); nl=${nl%_}
printf 'LEON-FILE 1\nDIR\n'
n=0
set --
for e in "$d"/.[!.]* "$d"/..?* "$d"/*; do
  if [ -L "$e" ]; then l=l; elif [ -e "$e" ]; then l=-; else continue; fi
  name=${e##*/}
  case $name in *"$nl"*) continue ;; esac
  n=$((n + 1))
  if [ "$n" -gt @CAP@ ]; then printf 'TRUNCATED\n'; break; fi
  if [ -d "$e" ]; then k=d
  elif [ -f "$e" ]; then k=f; set -- "$@" "$e"
  else k=o; fi
  printf '%s%s %s\n' "$k" "$l" "$name"
done
printf 'SIZES\n'
if [ "$#" -gt 0 ]; then wc -c -- "$@" 2>/dev/null; fi
"#;

/// Reads what the listing command printed. `None` when the output is not the
/// script's answer at all.
pub fn parse_dir(output: &str) -> Option<DirListing> {
    let body = after_marker(output)?;
    let mut lines = body.split('\n');
    match lines.next()?.trim_end_matches('\r') {
        "MISSING" => return Some(DirListing::Missing),
        "NOT-DIR" => return Some(DirListing::NotADir),
        "DIR" => {}
        _ => return None,
    }
    let mut entries = Vec::new();
    let mut wants_size = Vec::new();
    for line in lines.by_ref() {
        if line == "SIZES" {
            break;
        }
        let Some((flags, name)) = line.split_once(' ') else {
            continue;
        };
        let kind = match flags.as_bytes() {
            [b'd', b'-' | b'l'] => EntryKind::Dir,
            [b'f', b'-' | b'l'] => EntryKind::File,
            [b'o', b'-' | b'l'] => EntryKind::Other,
            _ => continue,
        };
        if kind == EntryKind::File {
            wants_size.push(entries.len());
        }
        entries.push(FileEntry {
            name: name.to_owned(),
            kind,
            symlink: flags.ends_with('l'),
            size: None,
        });
    }
    // `wc` prints one line per file, and a `total` line after several. Any
    // other count means a file could not be read: no sizes rather than wrong
    // ones.
    let sizes: Vec<Option<u64>> = lines
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.split_whitespace().next().and_then(|n| n.parse().ok()))
        .collect();
    let expected = wants_size.len();
    if sizes.len() == expected || (expected > 1 && sizes.len() == expected + 1) {
        for (index, size) in wants_size.into_iter().zip(sizes) {
            entries[index].size = size;
        }
    }
    sort_entries(&mut entries);
    Some(DirListing::Entries(entries))
}

/// Folders first, then by name without regard to case, then by exact name.
pub fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        (a.kind != EntryKind::Dir)
            .cmp(&(b.kind != EntryKind::Dir))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

// ----- git marks --------------------------------------------------------------

/// The command that asks git what changed under `root`, a work tree's root or
/// a folder inside one: the paths of the answer are relative to `root`.
/// Ignored paths are included.
///
/// The answer is read with [`parse_marks`].
pub fn git_marks_command(root: &str) -> CommandSpec {
    let script = MARKS_SCRIPT.replace("@BYTES@", &MAX_LIST_BYTES.to_string());
    CommandSpec::new("sh").args(["-c", script.as_str(), "sh", root])
}

const MARKS_SCRIPT: &str = r#"cd "$1" 2>/dev/null || { printf 'LEON-FILE 1\nMISSING\n'; exit 0; }
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  printf 'LEON-FILE 1\nGIT\n%s\n' "$(git rev-parse --show-prefix 2>/dev/null)"
  git status --porcelain=v1 -z --ignored 2>/dev/null | head -c @BYTES@
else
  printf 'LEON-FILE 1\nNO-GIT\n'
fi
"#;

/// Reads what the git marks command printed: the marks (none for a folder
/// that is not a repository). `None` when the folder is missing or the output
/// is not the script's answer at all.
pub fn parse_marks(output: &str) -> Option<GitMarks> {
    let body = after_marker(output)?;
    let (head, rest) = body.split_once('\n').unwrap_or((body, ""));
    match head.trim_end_matches('\r') {
        "GIT" => {
            // The folder's place in the work tree, then git's status.
            let (prefix, status) = rest.split_once('\n').unwrap_or((rest, ""));
            Some(GitMarks::from_status_in(status, prefix))
        }
        "NO-GIT" => Some(GitMarks::default()),
        _ => None,
    }
}

/// The paths and marks of `git status --porcelain=v1 -z`: entries are
/// `XY path` ending in NUL, and a rename or copy is followed by its origin as
/// an entry of its own, which is skipped.
pub fn parse_status(status: &str) -> Vec<(String, GitMark)> {
    let mut found = Vec::new();
    let mut entries = status.split('\0');
    while let Some(entry) = entries.next() {
        let Some((xy, path)) = entry
            .get(..2)
            .and_then(|xy| entry.get(3..).map(|path| (xy, path)))
        else {
            continue;
        };
        let (x, y) = (xy.as_bytes()[0], xy.as_bytes()[1]);
        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            entries.next();
        }
        let mark = match (x, y) {
            (b'?', b'?') => GitMark::Untracked,
            (b'!', b'!') => GitMark::Ignored,
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => GitMark::Conflicted,
            _ => {
                // The worktree side says more than the index side.
                let side = if y == b' ' { x } else { y };
                match side {
                    b'M' | b'T' => GitMark::Modified,
                    b'A' | b'C' => GitMark::Added,
                    b'D' => GitMark::Deleted,
                    b'R' => GitMark::Renamed,
                    _ => continue,
                }
            }
        };
        if !path.is_empty() {
            found.push((path.to_owned(), mark));
        }
    }
    found
}

// ----- project files ----------------------------------------------------------

/// The command that lists every file under `root`, recursively: a git
/// repository's tracked and untracked-not-ignored files, or a bounded `find`
/// that leaves out `.git` and the usual dependency and build folders.
///
/// The answer is read with [`parse_project_files`].
pub fn project_files_command(root: &str) -> CommandSpec {
    let script = FILES_SCRIPT
        .replace("@CAP@", &MAX_LIST_ENTRIES.to_string())
        .replace("@BYTES@", &MAX_LIST_BYTES.to_string());
    CommandSpec::new("sh").args(["-c", script.as_str(), "sh", root])
}

const FILES_SCRIPT: &str = r#"cd "$1" 2>/dev/null || { printf 'LEON-FILE 1\nMISSING\n'; exit 0; }
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  printf 'LEON-FILE 1\nGIT\n'
  git ls-files -co --exclude-standard -z 2>/dev/null | head -c @BYTES@
else
  printf 'LEON-FILE 1\nFIND\n'
  find . \( -name .git -o -name node_modules -o -name target -o -name dist -o -name build -o -name .venv -o -name venv -o -name __pycache__ -o -name .next -o -name .cache \) -prune -o -type f -print 2>/dev/null | head -n @CAP@ | head -c @BYTES@
fi
"#;

/// Reads what the project files command printed: relative paths with `/`
/// separators. `None` when the folder is missing or the output is not the
/// script's answer at all.
pub fn parse_project_files(output: &str) -> Option<Vec<String>> {
    let body = after_marker(output)?;
    let (head, rest) = body.split_once('\n').unwrap_or((body, ""));
    match head.trim_end_matches('\r') {
        "GIT" => Some(parse_paths(rest, '\0')),
        "FIND" => Some(parse_paths(rest, '\n')),
        _ => None,
    }
}

/// The paths of a list separated by `separator`. A last path with no
/// separator after it was cut short and is dropped, and so is anything
/// absolute, empty or leading out of the root. At most [`MAX_LIST_ENTRIES`]
/// survive.
pub fn parse_paths(list: &str, separator: char) -> Vec<String> {
    let mut parts: Vec<&str> = list.split(separator).collect();
    // Whatever follows the last separator is empty for a complete list.
    parts.pop();
    parts
        .into_iter()
        .map(|path| {
            let path = if separator == '\n' {
                path.trim_end_matches('\r')
            } else {
                path
            };
            path.strip_prefix("./").unwrap_or(path)
        })
        .filter(|path| inside_root(path))
        .take(MAX_LIST_ENTRIES)
        .map(str::to_owned)
        .collect()
}

/// Whether `path` is relative and has no way up.
pub(crate) fn inside_root(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// What follows the marker line, or `None` when no line is the marker.
pub(crate) fn after_marker(output: &str) -> Option<&str> {
    let mut offset = 0;
    for line in output.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end_matches(['\n', '\r']) == MARKER {
            return Some(&output[offset..]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::remote_shell_command;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn a_text_file_is_read_after_a_login_banner() {
        let output = format!(
            "Welcome to build-box\nLast login: Mon\nLEON-FILE 1\nTEXT 123 12\n{}\n",
            b64(b"fn main() {}\n")
        );
        assert_eq!(
            parse_read(&output),
            Some(ReadOutcome::Found(FileContent::Text {
                text: "fn main() {}\n".into(),
                revision: FileRevision::new("123 12"),
            }))
        );
    }

    #[test]
    fn a_byte_order_mark_and_carriage_returns_are_kept() {
        let bytes = "\u{feff}one\r\ntwo\r\n";
        let output = format!("LEON-FILE 1\nTEXT 1 2\n{}\n", b64(bytes.as_bytes()));
        let Some(ReadOutcome::Found(FileContent::Text { text, .. })) = parse_read(&output) else {
            panic!("text");
        };
        assert_eq!(text, bytes);
        assert!(text.starts_with('\u{feff}'));
    }

    #[test]
    fn every_other_answer_of_the_read_script_is_understood() {
        assert_eq!(
            parse_read("LEON-FILE 1\nMISSING\n"),
            Some(ReadOutcome::Missing)
        );
        assert_eq!(
            parse_read("LEON-FILE 1\r\nNOT-FILE\r\n"),
            Some(ReadOutcome::NotAFile)
        );
        assert_eq!(
            parse_read("LEON-FILE 1\nTOO-BIG 2097153\n"),
            Some(ReadOutcome::Found(FileContent::TooBig { size: 2_097_153 }))
        );
        assert_eq!(
            parse_read("LEON-FILE 1\nBINARY 9\n"),
            Some(ReadOutcome::Found(FileContent::Binary { size: 9 }))
        );
        assert_eq!(
            parse_read("ssh: connect to host box port 22: refused"),
            None
        );
        assert_eq!(parse_read("LEON-FILE 1\nWHAT\n"), None);
        assert_eq!(parse_read("LEON-FILE 1\nTEXT 1 2\n!!!\n"), None);
    }

    #[test]
    fn bytes_that_are_not_utf8_come_back_as_binary() {
        let output = format!("LEON-FILE 1\nTEXT 1 3\n{}\n", b64(&[0xff, 0xfe, 0x41]));
        assert_eq!(
            parse_read(&output),
            Some(ReadOutcome::Found(FileContent::Binary { size: 3 }))
        );
    }

    #[test]
    fn the_write_answers_are_understood_after_a_banner() {
        assert_eq!(
            parse_write("hello\nLEON-FILE 1\nOK 77 5\n"),
            Some(WriteOutcome::Saved(FileRevision::new("77 5")))
        );
        assert_eq!(
            parse_write("LEON-FILE 1\nCONFLICT\n"),
            Some(WriteOutcome::Conflict)
        );
        assert_eq!(parse_write("LEON-FILE 1\nOK\n"), None);
        assert_eq!(parse_write(""), None);
    }

    #[test]
    fn a_path_with_spaces_and_quotes_is_quoted_and_the_bytes_are_the_stdin() {
        let path = "/srv/my project/it's \"here\".rs";
        let command = write_command(path, Some(&FileRevision::new("1 2")), b"\xef\xbb\xbfa\r\n");
        assert_eq!(command.args[3], path);
        assert_eq!(command.args[4], "1 2");
        assert_eq!(command.stdin.as_deref(), Some(&b"\xef\xbb\xbfa\r\n"[..]));
        let line = remote_shell_command(&command);
        assert!(
            line.contains(r#"'/srv/my project/it'\''s "here".rs'"#),
            "{line}"
        );
        let create = write_command(path, None, b"");
        assert_eq!(create.args[4], "");
        let read = read_command(path);
        assert_eq!(read.args[3], path);
        assert!(read.args[1].contains("2097152"), "the cap is in the script");
    }

    #[test]
    fn a_folder_listing_is_read_with_kinds_links_and_sizes() {
        let output = "motd\nLEON-FILE 1\nDIR\nfl link with space.txt\nd- src\nf- b's.rs\ndl docs\nf- .env\no- socket\nSIZES\n   10 /w/link with space.txt\n   20 /w/b's.rs\n   5 /w/.env\n  35 total\n";
        let Some(DirListing::Entries(entries)) = parse_dir(output) else {
            panic!("entries");
        };
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "docs",
                "src",
                ".env",
                "b's.rs",
                "link with space.txt",
                "socket"
            ]
        );
        assert_eq!(entries[0].kind, EntryKind::Dir);
        assert!(entries[0].symlink);
        assert_eq!(entries[2].size, Some(5));
        assert_eq!(entries[3].size, Some(20));
        assert_eq!(entries[4].size, Some(10));
        assert!(entries[4].symlink);
        assert_eq!(entries[5].kind, EntryKind::Other);
        assert_eq!(entries[5].size, None);
    }

    #[test]
    fn sizes_that_do_not_add_up_are_left_out() {
        let output = "LEON-FILE 1\nDIR\nf- a\nf- b\nSIZES\n 1 /w/a\n";
        let Some(DirListing::Entries(entries)) = parse_dir(output) else {
            panic!("entries");
        };
        assert!(entries.iter().all(|entry| entry.size.is_none()));
    }

    #[test]
    fn a_missing_folder_and_a_file_are_not_listings() {
        assert_eq!(
            parse_dir("LEON-FILE 1\nMISSING\n"),
            Some(DirListing::Missing)
        );
        assert_eq!(
            parse_dir("LEON-FILE 1\nNOT-DIR\n"),
            Some(DirListing::NotADir)
        );
        assert_eq!(parse_dir("nothing"), None);
        assert_eq!(
            parse_dir("LEON-FILE 1\nDIR\nSIZES\n"),
            Some(DirListing::Entries(Vec::new()))
        );
    }

    #[test]
    fn git_status_becomes_marks_and_folders_roll_them_up() {
        let status = "A  new.rs\0 M src/lib.rs\0?? notes/\0?? scratch file.txt\0!! target/\0UU src/merge.rs\0R  moved.rs\0old.rs\0 D gone.rs\0";
        let marks = GitMarks::from_status(status);
        assert_eq!(marks.get("new.rs"), Some(GitMark::Added));
        assert_eq!(marks.get("src/lib.rs"), Some(GitMark::Modified));
        assert_eq!(marks.get("src/merge.rs"), Some(GitMark::Conflicted));
        assert_eq!(marks.get("src"), Some(GitMark::Conflicted));
        assert_eq!(marks.get("moved.rs"), Some(GitMark::Renamed));
        assert_eq!(
            marks.get("old.rs"),
            None,
            "the origin of a rename is not a path"
        );
        assert_eq!(marks.get("gone.rs"), Some(GitMark::Deleted));
        assert_eq!(marks.get("scratch file.txt"), Some(GitMark::Untracked));
        // Whole folders mark what is inside, rolled-up folders do not.
        assert_eq!(marks.get("notes"), Some(GitMark::Untracked));
        assert_eq!(marks.get("notes/deep/a.md"), Some(GitMark::Untracked));
        assert_eq!(marks.get("target/debug/x"), Some(GitMark::Ignored));
        assert_eq!(marks.get("src/other.rs"), None);
        // Ignored files do not color their folder.
        let ignored = GitMarks::from_status("!! a/b.log\0");
        assert_eq!(ignored.get("a/b.log"), Some(GitMark::Ignored));
        assert_eq!(ignored.get("a"), None);
        assert_eq!(GitMark::Modified.letter(), 'M');
        assert_eq!(GitMark::Conflicted.letter(), 'U');
    }

    #[test]
    fn a_staged_and_edited_file_is_modified_and_the_index_side_counts_alone() {
        let marks = GitMarks::from_status("AM both.rs\0M  staged.rs\0 T typed.rs\0");
        assert_eq!(marks.get("both.rs"), Some(GitMark::Modified));
        assert_eq!(marks.get("staged.rs"), Some(GitMark::Modified));
        assert_eq!(marks.get("typed.rs"), Some(GitMark::Modified));
    }

    #[test]
    fn a_folder_inside_a_work_tree_gets_paths_relative_to_itself() {
        let status =
            " M app/src/a.rs\0?? app/new/\0 M other/b.rs\0!! app/target/\0?? appendix.md\0";
        let marks = GitMarks::from_status_in(status, "app/");
        assert_eq!(marks.get("src/a.rs"), Some(GitMark::Modified));
        assert_eq!(marks.get("src"), Some(GitMark::Modified));
        assert_eq!(marks.get("new/x.rs"), Some(GitMark::Untracked));
        assert_eq!(marks.get("target/x"), Some(GitMark::Ignored));
        assert_eq!(marks.get("other"), None, "outside the folder");
        assert_eq!(marks.get("b.rs"), None);
        assert_eq!(
            marks.get("appendix.md"),
            None,
            "a sibling with the same start"
        );
        assert_eq!(marks.len(), 4);

        // The folder itself, or a folder above it, reported whole.
        let marks = GitMarks::from_status_in("?? app/\0", "app/");
        assert_eq!(marks.get("any/thing.rs"), Some(GitMark::Untracked));
        let marks = GitMarks::from_status_in("!! app/\0", "app/deep/");
        assert_eq!(marks.get("x.rs"), Some(GitMark::Ignored));
        let marks = GitMarks::from_status_in(" M app/deep/x.rs\0", "app/deep/");
        assert_eq!(marks.get("x.rs"), Some(GitMark::Modified));
        assert_eq!(marks.get("y.rs"), None);

        let marks = parse_marks("LEON-FILE 1\nGIT\napp/\n M app/a.rs\0 M b.rs\0").unwrap();
        assert_eq!(marks.get("a.rs"), Some(GitMark::Modified));
        assert_eq!(marks.get("b.rs"), None);
    }

    #[test]
    fn marks_are_read_after_a_banner_and_a_plain_folder_has_none() {
        let marks = parse_marks("banner\nLEON-FILE 1\nGIT\n\n M a b.rs\0").unwrap();
        assert_eq!(marks.get("a b.rs"), Some(GitMark::Modified));
        assert_eq!(
            parse_marks("LEON-FILE 1\nGIT\n\n").unwrap(),
            GitMarks::default()
        );
        assert!(parse_marks("LEON-FILE 1\nNO-GIT\n").unwrap().is_empty());
        assert_eq!(parse_marks("LEON-FILE 1\nMISSING\n"), None);
        assert_eq!(parse_marks("fatal: not a repo"), None);
    }

    #[test]
    fn project_files_come_from_git_or_find() {
        assert_eq!(
            parse_project_files("Hi!\nLEON-FILE 1\nGIT\nsrc/a b.rs\0it's.md\0").unwrap(),
            ["src/a b.rs", "it's.md"]
        );
        assert_eq!(
            parse_project_files("LEON-FILE 1\nFIND\n./a.md\n./sub dir/b.md\n\n").unwrap(),
            ["a.md", "sub dir/b.md"]
        );
        assert_eq!(parse_project_files("LEON-FILE 1\nMISSING\n"), None);
    }

    #[test]
    fn a_path_cut_by_the_output_limit_or_leading_out_of_the_root_is_dropped() {
        assert_eq!(parse_paths("a.rs\0b.r", '\0'), ["a.rs"]);
        assert_eq!(
            parse_paths("/etc/passwd\0../up\0a/../b\0fine.md\0", '\0'),
            ["fine.md"]
        );
        let many = (0..MAX_LIST_ENTRIES + 10)
            .map(|n| format!("f{n}\0"))
            .collect::<String>();
        assert_eq!(parse_paths(&many, '\0').len(), MAX_LIST_ENTRIES);
    }
}

/// The scripts against a real shell, standing in for the remote side of an SSH
/// connection. There is no POSIX shell to stand in on Windows.
#[cfg(all(test, unix))]
mod shell_tests {
    use super::*;
    use crate::command::remote_shell_command;
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    fn run(command: &CommandSpec) -> std::process::Output {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(remote_shell_command(command))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh starts");
        let mut stdin = child.stdin.take().expect("piped");
        stdin
            .write_all(command.stdin.as_deref().unwrap_or_default())
            .expect("stdin accepts the bytes");
        drop(stdin);
        child.wait_with_output().expect("the command finishes")
    }

    fn stdout(command: &CommandSpec) -> String {
        let output = run(command);
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn read(path: &std::path::Path) -> ReadOutcome {
        parse_read(&stdout(&read_command(&path.to_string_lossy()))).expect("an answer")
    }

    fn revision_of(path: &std::path::Path) -> FileRevision {
        match read(path) {
            ReadOutcome::Found(FileContent::Text { revision, .. }) => revision,
            other => panic!("expected text, got {other:?}"),
        }
    }

    fn save(
        path: &std::path::Path,
        expected: Option<&FileRevision>,
        bytes: &[u8],
    ) -> Option<WriteOutcome> {
        parse_write(&stdout(&write_command(
            &path.to_string_lossy(),
            expected,
            bytes,
        )))
    }

    #[test]
    fn a_file_round_trips_with_its_bom_and_line_endings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("it's a \"file\".txt");
        let text = "\u{feff}Título\r\nsecond 'line' $HOME\r\n";
        std::fs::write(&path, text).unwrap();
        let ReadOutcome::Found(FileContent::Text {
            text: read_text,
            revision,
        }) = read(&path)
        else {
            panic!("text");
        };
        assert_eq!(read_text, text);

        let newer = "\u{feff}Otro\r\n";
        let Some(WriteOutcome::Saved(after)) = save(&path, Some(&revision), newer.as_bytes())
        else {
            panic!("saved");
        };
        assert_eq!(std::fs::read(&path).unwrap(), newer.as_bytes());
        assert_eq!(revision_of(&path), after);
    }

    #[test]
    fn a_save_is_refused_when_the_revision_moved_or_the_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "first\n").unwrap();
        let revision = revision_of(&path);
        std::fs::write(&path, "somebody else\n").unwrap();
        assert_eq!(
            save(&path, Some(&revision), b"mine\n"),
            Some(WriteOutcome::Conflict)
        );
        assert_eq!(save(&path, None, b"mine\n"), Some(WriteOutcome::Conflict));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "somebody else\n");

        // A file that was read and is now gone is a conflict as well.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            save(&path, Some(&revision), b"mine\n"),
            Some(WriteOutcome::Conflict)
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_new_file_is_created_and_no_temporary_file_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        assert!(matches!(
            save(&path, None, b"hello\n"),
            Some(WriteOutcome::Saved(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_save_keeps_the_mode_and_writes_through_a_link() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("run.sh");
        std::fs::write(&real, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o750)).unwrap();
        let link = dir.path().join("link.sh");
        symlink("run.sh", &link).unwrap();
        let revision = revision_of(&link);
        assert!(matches!(
            save(&link, Some(&revision), b"#!/bin/sh\necho hi\n"),
            Some(WriteOutcome::Saved(_))
        ));
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            "#!/bin/sh\necho hi\n"
        );
        let mode = std::fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o750);
    }

    #[test]
    fn reading_reports_missing_big_binary_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(&dir.path().join("nope")), ReadOutcome::Missing);
        assert_eq!(read(dir.path()), ReadOutcome::NotAFile);

        let big = dir.path().join("big.txt");
        std::fs::write(&big, vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        assert_eq!(
            read(&big),
            ReadOutcome::Found(FileContent::TooBig {
                size: MAX_FILE_BYTES as u64 + 1
            })
        );

        let binary = dir.path().join("a.bin");
        std::fs::write(&binary, [1, 0, 2, 0xff]).unwrap();
        assert_eq!(
            read(&binary),
            ReadOutcome::Found(FileContent::Binary { size: 4 })
        );

        let empty = dir.path().join("empty");
        std::fs::write(&empty, "").unwrap();
        assert!(matches!(
            read(&empty),
            ReadOutcome::Found(FileContent::Text { .. })
        ));
    }

    #[test]
    fn a_folder_is_listed_one_level_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub dir")).unwrap();
        std::fs::write(dir.path().join("sub dir/inner.txt"), "x").unwrap();
        std::fs::write(dir.path().join("b's file.txt"), "12345").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        std::os::unix::fs::symlink("b's file.txt", dir.path().join("alias")).unwrap();
        std::os::unix::fs::symlink("nowhere", dir.path().join("dangling")).unwrap();
        let listing = parse_dir(&stdout(&list_command(&dir.path().to_string_lossy()))).unwrap();
        let DirListing::Entries(entries) = listing else {
            panic!("entries");
        };
        let by_name = |name: &str| entries.iter().find(|e| e.name == name).unwrap();
        assert_eq!(entries[0].name, "sub dir");
        assert_eq!(by_name("sub dir").kind, EntryKind::Dir);
        assert_eq!(by_name("b's file.txt").size, Some(5));
        assert_eq!(by_name(".hidden").size, Some(0));
        assert!(by_name("alias").symlink);
        assert_eq!(by_name("alias").size, Some(5));
        assert_eq!(by_name("dangling").kind, EntryKind::Other);
        assert!(by_name("dangling").symlink);
        assert_eq!(entries.len(), 5, "nothing inside the folder is listed");

        let gone = dir.path().join("gone");
        assert_eq!(
            parse_dir(&stdout(&list_command(&gone.to_string_lossy()))),
            Some(DirListing::Missing)
        );
        let file = dir.path().join(".hidden");
        assert_eq!(
            parse_dir(&stdout(&list_command(&file.to_string_lossy()))),
            Some(DirListing::NotADir)
        );
    }

    fn git_in(dir: &std::path::Path, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .is_ok_and(|output| output.status.success())
    }

    #[test]
    fn project_files_and_marks_follow_git_and_fall_back_to_find() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        std::fs::write(dir.path().join("a.md"), "a").unwrap();
        std::fs::create_dir_all(dir.path().join("sub/node_modules")).unwrap();
        std::fs::write(dir.path().join("sub/b c.md"), "b").unwrap();
        std::fs::write(dir.path().join("sub/node_modules/x.js"), "x").unwrap();
        let mut found = parse_project_files(&stdout(&project_files_command(&root))).unwrap();
        found.sort();
        assert_eq!(found, ["a.md", "sub/b c.md"]);
        assert_eq!(
            parse_marks(&stdout(&git_marks_command(&root))).unwrap(),
            GitMarks::default()
        );

        if !git_in(dir.path(), &["init", "-q"]) {
            eprintln!("skipped: git is not installed");
            return;
        }
        std::fs::write(dir.path().join(".gitignore"), "ignored.md\n").unwrap();
        std::fs::write(dir.path().join("ignored.md"), "i").unwrap();
        assert!(git_in(dir.path(), &["add", "a.md", ".gitignore"]));
        let mut found = parse_project_files(&stdout(&project_files_command(&root))).unwrap();
        found.sort();
        assert_eq!(
            found,
            [".gitignore", "a.md", "sub/b c.md", "sub/node_modules/x.js"]
        );
        let marks = parse_marks(&stdout(&git_marks_command(&root))).unwrap();
        assert_eq!(marks.get("a.md"), Some(GitMark::Added));
        assert_eq!(marks.get("ignored.md"), Some(GitMark::Ignored));
        assert_eq!(marks.get("sub/b c.md"), Some(GitMark::Untracked));

        let gone = dir.path().join("gone").to_string_lossy().into_owned();
        assert_eq!(
            parse_project_files(&stdout(&project_files_command(&gone))),
            None
        );

        // A folder inside the repository is marked relative to itself.
        std::fs::write(dir.path().join("sub/inner.md"), "i").unwrap();
        let sub = dir.path().join("sub").to_string_lossy().into_owned();
        let marks = parse_marks(&stdout(&git_marks_command(&sub))).unwrap();
        assert_eq!(marks.get("inner.md"), Some(GitMark::Untracked));
        assert_eq!(marks.get("b c.md"), Some(GitMark::Untracked));
    }
}

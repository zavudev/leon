//! A snapshot of a worktree at the start of every turn, and the way back to
//! it.
//!
//! Before an agent starts a turn in a git worktree, the files as they are
//! (tracked, modified, untracked and not ignored) are written to a commit
//! kept under `refs/leon/checkpoints/<chat>/<turn>`. Nothing the person
//! sees is touched while that happens: the commit is built with plumbing in a
//! scratch index (`GIT_INDEX_FILE`), so the real index, `HEAD`, the branch
//! and the stash stay as they were. "Edit from here" can then put the files
//! back as they were when that turn began.
//!
//! [`restore`] writes the files of a checkpoint into the worktree and removes
//! the ones the snapshot did not have, which are the ones the agent made
//! since. It compares the checkpoint with a fresh snapshot of the worktree and
//! touches only what differs, so files git ignores are never read or removed.
//! Like the snapshot it leaves `HEAD`, the index and the branch alone.
//!
//! A folder git does not know has no checkpoints: [`Repo::of`] is `None` and
//! the feature is not offered.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::changed::{parse_numstat, Stat};

/// Where the checkpoints of every chat are, as refs.
const NAMESPACE: &str = "refs/leon/checkpoints";

/// What a checkpoint's commit says of itself.
const MESSAGE: &str = "Leon checkpoint";

/// A mode git gives to a submodule or an embedded repository: a commit, not a
/// file this module can write.
const GITLINK: &str = "160000";

/// Why a snapshot or a restore failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

type Result<T> = std::result::Result<T, Error>;

/// A git worktree, as the checkpoints see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The root of the worktree.
    top: PathBuf,
    /// Where the folder asked about lies below that root, with a trailing
    /// slash, or empty when it is the root.
    prefix: String,
}

/// What a restore did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Restored {
    /// Files written: put back, or changed back.
    pub written: usize,
    /// Files removed: the ones that were not there at the checkpoint.
    pub removed: usize,
}

/// Whether `folder` looks like part of a git worktree: a `.git` above it.
/// Only looks at the names, so it is cheap enough to ask before every turn;
/// [`Repo::of`] is what knows.
pub fn in_worktree(folder: &Path) -> bool {
    folder.ancestors().any(|dir| dir.join(".git").exists())
}

/// The ref a checkpoint of `chat` for `turn` is kept under.
pub fn ref_name(chat: &str, turn: u32) -> String {
    format!("{NAMESPACE}/{chat}/{turn}")
}

impl Repo {
    /// The worktree `folder` is in, or `None` when it is not in one (or git
    /// is not there to say).
    pub fn of(folder: &Path) -> Option<Self> {
        let out = run(
            folder,
            &["rev-parse", "--show-toplevel", "--show-prefix"],
            &[],
            None,
        )
        .ok()?;
        let text = String::from_utf8(out).ok()?;
        let mut lines = text.lines();
        let top = PathBuf::from(lines.next()?);
        let prefix = lines.next().unwrap_or_default().to_owned();
        Some(Self { top, prefix })
    }

    /// Takes the snapshot of the worktree as it is and keeps it as the
    /// checkpoint of `turn` of `chat`. Returns its commit.
    pub fn capture(&self, chat: &str, turn: u32) -> Result<String> {
        let tree = self.snapshot_tree()?;
        let head = self.head();
        let mut args = vec!["commit-tree".to_owned(), tree];
        if let Some(head) = &head {
            args.extend(["-p".to_owned(), head.clone()]);
        }
        args.extend(["-m".to_owned(), MESSAGE.to_owned()]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let commit = self.git_with(&args, &identity(), None)?;
        let commit = text(commit);
        let name = ref_name(chat, turn);
        self.git(&["update-ref", &name, &commit])?;
        Ok(commit)
    }

    /// Puts the files below the folder asked about back as they were at
    /// `commit`: the files that differ are written from it, the ones it does
    /// not have are removed.
    pub fn restore(&self, commit: &str) -> Result<Restored> {
        let target = text(self.git(&["rev-parse", "--verify", &format!("{commit}^{{tree}}")])?);
        let now = self.snapshot_tree()?;
        let raw = self.git(&[
            "diff-tree",
            "-r",
            "-z",
            "--raw",
            "--no-renames",
            "--no-abbrev",
            &now,
            &target,
        ])?;
        let mut gone = Vec::new();
        let mut put = Vec::new();
        for change in parse_raw(&raw) {
            if !change.path.starts_with(&self.prefix) {
                continue;
            }
            if change.old_mode == GITLINK || change.new_mode == GITLINK {
                continue;
            }
            match change.status {
                'A' => put.push(change.path),
                'D' => gone.push(change.path),
                // A file that became a link, or the other way: out of the
                // way first, then written.
                'T' => {
                    gone.push(change.path.clone());
                    put.push(change.path);
                }
                _ => put.push(change.path),
            }
        }
        // Deleted first: a file may stand where a folder is to be written.
        for path in &gone {
            self.remove(path);
        }
        if !put.is_empty() {
            let index = ScratchIndex::new(self)?;
            self.git_with(&["read-tree", &target], &index.env(), None)?;
            let mut paths = Vec::new();
            for path in &put {
                paths.extend_from_slice(path.as_bytes());
                paths.push(0);
            }
            self.git_with(
                &["checkout-index", "-f", "-z", "--stdin"],
                &index.env(),
                Some(&paths),
            )?;
        }
        Ok(Restored {
            written: put.len(),
            removed: gone.iter().filter(|path| !put.contains(path)).count(),
        })
    }

    /// Whether the checkpoint's commit is still kept.
    pub fn has(&self, commit: &str) -> bool {
        self.git(&["cat-file", "-e", &format!("{commit}^{{commit}}")])
            .is_ok()
    }

    /// The files that differ between the checkpoint `from` and `to`, another
    /// checkpoint or, for `None`, the worktree as it is, and how many lines
    /// went and came in each. Only the files below the folder asked about.
    /// What git ignores is in neither.
    pub fn numstat(&self, from: &str, to: Option<&str>) -> Result<Vec<Stat>> {
        let to = self.tree_to(to)?;
        let raw = self.git(&[
            "diff",
            "--numstat",
            "-z",
            "--no-renames",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            from,
            &to,
        ])?;
        Ok(parse_numstat(&String::from_utf8_lossy(&raw))
            .into_iter()
            .filter(|stat| stat.path.starts_with(&self.prefix))
            .collect())
    }

    /// The patch of one file (from the root of the worktree) between the
    /// checkpoint `from` and `to`, as [`Self::numstat`] reads `to`.
    pub fn patch(&self, from: &str, to: Option<&str>, path: &str) -> Result<String> {
        if path.starts_with('-') {
            return Err(Error(format!(
                "a path must not start with a dash: {path:?}"
            )));
        }
        let to = self.tree_to(to)?;
        let raw = self.git(&[
            "--literal-pathspecs",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            from,
            &to,
            "--",
            path,
        ])?;
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }

    /// What to compare against: the commit of a checkpoint, or the tree of
    /// the worktree as it is.
    fn tree_to(&self, to: Option<&str>) -> Result<String> {
        match to {
            Some(commit) => Ok(commit.to_owned()),
            None => self.snapshot_tree(),
        }
    }

    /// Forgets the checkpoints of `chat` from `turn` on.
    pub fn forget_from(&self, chat: &str, turn: u32) {
        let doomed: Vec<String> = self
            .refs_of(chat)
            .into_iter()
            .filter(|(at, _)| *at >= turn)
            .map(|(_, name)| name)
            .collect();
        self.delete_refs(&doomed);
    }

    /// Forgets all the checkpoints of `chat`.
    pub fn forget(&self, chat: &str) {
        self.forget_from(chat, 0);
    }

    /// Keeps only the newest `most` checkpoints of `chat`.
    pub fn keep_newest(&self, chat: &str, most: usize) {
        let mut refs = self.refs_of(chat);
        refs.sort();
        let excess = refs.len().saturating_sub(most);
        let doomed: Vec<String> = refs
            .into_iter()
            .take(excess)
            .map(|(_, name)| name)
            .collect();
        self.delete_refs(&doomed);
    }

    /// Forgets the checkpoints of every chat but `except` that were taken
    /// before `before`, in seconds since the epoch: what a chat that was
    /// never closed properly (a crash) left behind.
    pub fn forget_older_than(&self, before: i64, except: &str) {
        let Ok(listed) = self.git(&[
            "for-each-ref",
            "--format=%(committerdate:unix) %(refname)",
            NAMESPACE,
        ]) else {
            return;
        };
        let own = format!("{NAMESPACE}/{except}/");
        let doomed: Vec<String> = text(listed)
            .lines()
            .filter_map(|line| {
                let (when, name) = line.split_once(' ')?;
                let old = when.parse::<i64>().ok()? < before;
                (old && !name.starts_with(&own)).then(|| name.to_owned())
            })
            .collect();
        self.delete_refs(&doomed);
    }

    /// The checkpoints of `chat`: the turn and the ref of each.
    pub fn refs_of(&self, chat: &str) -> Vec<(u32, String)> {
        let prefix = format!("{NAMESPACE}/{chat}/");
        let Ok(listed) = self.git(&["for-each-ref", "--format=%(refname)", &prefix]) else {
            return Vec::new();
        };
        text(listed)
            .lines()
            .filter_map(|name| {
                let turn = name.strip_prefix(&prefix)?.parse().ok()?;
                Some((turn, name.to_owned()))
            })
            .collect()
    }

    fn delete_refs(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let script: String = names
            .iter()
            .map(|name| format!("delete {name}\n"))
            .collect();
        let _ = self.git_with(&["update-ref", "--stdin"], &[], Some(script.as_bytes()));
    }

    /// The commit `HEAD` is at; `None` before the first commit.
    fn head(&self) -> Option<String> {
        self.git(&["rev-parse", "--verify", "-q", "HEAD^{commit}"])
            .ok()
            .map(text)
    }

    /// The tree of the worktree as it is, written to the object database
    /// through a scratch index.
    fn snapshot_tree(&self) -> Result<String> {
        let index = ScratchIndex::new(self)?;
        let env = index.env();
        let added = self.git_with(&["add", "-A"], &env, None);
        if added.is_err() {
            // The copy of the person's index could not be read (a layout
            // this does not know): start from what is committed.
            index.reset();
            if self.head().is_some() {
                self.git_with(&["read-tree", "HEAD"], &env, None)?;
            }
            self.git_with(&["add", "-A"], &env, None)?;
        }
        Ok(text(self.git_with(&["write-tree"], &env, None)?))
    }

    /// Removes a file the snapshot did not have, and the folders that leaves
    /// empty.
    fn remove(&self, path: &str) {
        let file = self.top.join(path);
        if std::fs::remove_file(&file).is_err() {
            return;
        }
        let mut folder = file.parent();
        while let Some(dir) = folder {
            if dir == self.top || std::fs::remove_dir(dir).is_err() {
                break;
            }
            folder = dir.parent();
        }
    }

    fn git(&self, args: &[&str]) -> Result<Vec<u8>> {
        self.git_with(args, &[], None)
    }

    fn git_with(
        &self,
        args: &[&str],
        env: &[(&'static str, OsString)],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        run(&self.top, args, env, input)
    }
}

/// A copy of the person's index that is written to instead of it.
struct ScratchIndex {
    path: PathBuf,
}

impl ScratchIndex {
    fn new(repo: &Repo) -> Result<Self> {
        let git_dir = text(repo.git(&["rev-parse", "--absolute-git-dir"])?);
        let real =
            text(repo.git(&["rev-parse", "--path-format=absolute", "--git-path", "index"])?);
        let unique = format!(
            "leon-checkpoint-index-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        )
        .replace(['(', ')'], "");
        let path = Path::new(&git_dir).join(unique);
        let index = Self { path };
        // The copy keeps git from reading every file again; without one the
        // index starts empty and `add -A` fills it.
        let _ = std::fs::copy(real, &index.path);
        Ok(index)
    }

    /// Empties it.
    fn reset(&self) {
        let _ = std::fs::remove_file(&self.path);
    }

    fn env(&self) -> Vec<(&'static str, OsString)> {
        vec![
            ("GIT_INDEX_FILE", self.path.clone().into_os_string()),
            // A split index leans on files of the real one.
            ("GIT_CONFIG_COUNT", "2".into()),
            ("GIT_CONFIG_KEY_0", "core.splitIndex".into()),
            ("GIT_CONFIG_VALUE_0", "false".into()),
            ("GIT_CONFIG_KEY_1", "core.fsmonitor".into()),
            ("GIT_CONFIG_VALUE_1", "false".into()),
        ]
    }
}

impl Drop for ScratchIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let mut lock = self.path.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
    }
}

/// Who a checkpoint's commit is by: git refuses to commit without one, and
/// the person may have set none.
fn identity() -> Vec<(&'static str, OsString)> {
    vec![
        ("GIT_AUTHOR_NAME", "Leon".into()),
        ("GIT_AUTHOR_EMAIL", "noreply@zavu.dev".into()),
        ("GIT_COMMITTER_NAME", "Leon".into()),
        ("GIT_COMMITTER_EMAIL", "noreply@zavu.dev".into()),
    ]
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

/// One entry of `git diff-tree --raw -z`.
struct Change {
    old_mode: String,
    new_mode: String,
    status: char,
    path: String,
}

/// Reads `git diff-tree -r -z --raw --no-renames`: `:old new oldsha newsha S`
/// then the path, each ended by a NUL.
fn parse_raw(raw: &[u8]) -> Vec<Change> {
    let mut parts = raw
        .split(|byte| *byte == 0)
        .map(|part| String::from_utf8_lossy(part).into_owned());
    let mut changes = Vec::new();
    while let Some(head) = parts.next() {
        let Some(head) = head.strip_prefix(':') else {
            continue;
        };
        let Some(path) = parts.next() else {
            break;
        };
        let mut fields = head.split(' ');
        let (Some(old_mode), Some(new_mode)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Some(status) = fields.nth(2).and_then(|status| status.chars().next()) else {
            continue;
        };
        changes.push(Change {
            old_mode: old_mode.to_owned(),
            new_mode: new_mode.to_owned(),
            status,
            path,
        });
    }
    changes
}

/// Runs git in `folder` and returns what it printed.
fn run(
    folder: &Path,
    args: &[&str],
    env: &[(&'static str, OsString)],
    input: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut command = leon_remote::spawn::std_child("git");
    command
        .current_dir(folder)
        // Snapshots keep the bytes on disk as they are, and a restore writes
        // them back the same: no line-ending conversion either way (Windows
        // installs default to `core.autocrlf=true`).
        .args(["-c", "core.autocrlf=false", "-c", "core.safecrlf=false"])
        .args(args)
        // Never wait for a person, and never take the lock a status takes.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command
        .spawn()
        .map_err(|error| Error(format!("git could not be started: {error}")))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(input)
            .map_err(|error| Error(format!("git did not take its input: {error}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| Error(format!("git did not finish: {error}")))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let said = String::from_utf8_lossy(&output.stderr);
        Err(Error(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            said.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn write(dir: &Path, name: &str, text: &str) {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(dir: &Path, name: &str) -> Option<String> {
        fs::read_to_string(dir.join(name)).ok()
    }

    /// A repository with `a.txt`, `b.txt` and `sub/c.txt` committed and
    /// `*.log` ignored.
    fn repo() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "trunk"]);
        write(&root, ".gitignore", "*.log\n");
        write(&root, "a.txt", "a0\n");
        write(&root, "b.txt", "b0\n");
        write(&root, "sub/c.txt", "c0\n");
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "first"]);
        let repo = Repo::of(&root).expect("a repository");
        (dir, repo)
    }

    fn files_of(repo: &Repo, commit: &str) -> Vec<String> {
        let listed = git(&repo.top, &["ls-tree", "-r", "--name-only", commit]);
        listed.lines().map(str::to_owned).collect()
    }

    #[test]
    fn a_folder_git_does_not_know_has_no_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Repo::of(dir.path()), None);
        assert_eq!(Repo::of(&dir.path().join("missing")), None);
        assert!(!in_worktree(dir.path()));
        let (inside, repo) = repo();
        assert!(in_worktree(&repo.top));
        assert!(in_worktree(&repo.top.join("sub")));
        drop(inside);
    }

    #[test]
    fn a_snapshot_has_what_is_modified_and_untracked_and_not_what_is_ignored() {
        let (_dir, repo) = repo();
        let root = repo.top.clone();
        write(&root, "a.txt", "a1\n");
        write(&root, "new.txt", "new\n");
        write(&root, "debug.log", "noise\n");
        fs::remove_file(root.join("b.txt")).unwrap();
        let commit = repo.capture("chat", 1).unwrap();
        let files = files_of(&repo, &commit);
        assert!(files.contains(&"a.txt".to_owned()), "{files:?}");
        assert!(files.contains(&"new.txt".to_owned()), "{files:?}");
        assert!(files.contains(&"sub/c.txt".to_owned()), "{files:?}");
        assert!(!files.contains(&"b.txt".to_owned()), "deleted: {files:?}");
        assert!(
            !files.contains(&"debug.log".to_owned()),
            "ignored: {files:?}"
        );
        assert_eq!(git(&root, &["show", &format!("{commit}:a.txt")]), "a1");
        // It is kept under the chat's namespace, on top of HEAD.
        assert_eq!(git(&root, &["rev-parse", &ref_name("chat", 1)]), commit);
        assert_eq!(
            git(&root, &["rev-parse", &format!("{commit}^")]),
            git(&root, &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn a_snapshot_leaves_the_index_head_branch_and_stash_alone() {
        let (_dir, repo) = repo();
        let root = repo.top.clone();
        write(&root, "a.txt", "staged\n");
        git(&root, &["add", "a.txt"]);
        write(&root, "a.txt", "staged and more\n");
        write(&root, "new.txt", "new\n");
        let state = |root: &Path| {
            (
                git(root, &["rev-parse", "HEAD"]),
                git(root, &["symbolic-ref", "HEAD"]),
                git(root, &["status", "--porcelain=v1"]),
                git(root, &["diff", "--cached"]),
                git(root, &["stash", "list"]),
                git(root, &["for-each-ref", "refs/heads", "refs/stash"]),
            )
        };
        let before = state(&root);
        repo.capture("chat", 1).unwrap();
        assert_eq!(state(&root), before);
        assert_eq!(read(&root, "a.txt").as_deref(), Some("staged and more\n"));
        // No scratch file is left in the git directory.
        let left: Vec<String> = fs::read_dir(root.join(".git"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("leon-checkpoint"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn what_changed_between_two_snapshots_is_counted_and_read_as_a_patch() {
        let (_dir, repo) = repo();
        let root = repo.top.clone();
        let before = repo.capture("chat", 1).unwrap();
        write(&root, "a.txt", "a0\nmore\n");
        write(&root, "new.txt", "x\ny\n");
        fs::remove_file(root.join("b.txt")).unwrap();
        write(&root, "debug.log", "ignored\n");
        let after = repo.capture("chat", 2).unwrap();
        write(&root, "sub/c.txt", "c1\n");

        let mut between = repo.numstat(&before, Some(&after)).unwrap();
        between.sort_by(|a, b| a.path.cmp(&b.path));
        let said: Vec<(&str, usize, usize)> = between
            .iter()
            .map(|stat| (stat.path.as_str(), stat.added, stat.removed))
            .collect();
        assert_eq!(said, [("a.txt", 1, 0), ("b.txt", 0, 1), ("new.txt", 2, 0)]);

        // Against the worktree as it is: the file touched since shows too.
        let now = repo.numstat(&after, None).unwrap();
        assert_eq!(now.len(), 1, "{now:?}");
        assert_eq!(now[0].path, "sub/c.txt");

        let patch = repo.patch(&before, Some(&after), "a.txt").unwrap();
        assert!(patch.contains("+more"), "{patch}");
        assert!(!patch.contains("new.txt"), "only the file asked: {patch}");
        let live = repo.patch(&after, None, "sub/c.txt").unwrap();
        assert!(live.contains("+c1"), "{live}");
        // Neither touched the person's files, index or branch.
        assert_eq!(git(&root, &["diff", "--cached"]), "");
    }

    #[test]
    fn a_file_in_a_folder_below_the_one_asked_about_is_the_only_one_counted() {
        let (_dir, repo) = repo();
        let root = repo.top.clone();
        let before = repo.capture("chat", 1).unwrap();
        write(&root, "a.txt", "a1\n");
        write(&root, "sub/c.txt", "c1\n");
        let below = Repo::of(&root.join("sub")).unwrap();
        let stats = below.numstat(&before, None).unwrap();
        assert_eq!(
            stats.iter().map(|s| s.path.as_str()).collect::<Vec<_>>(),
            ["sub/c.txt"]
        );
    }

    #[test]
    fn a_repository_before_its_first_commit_is_snapshotted_too() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "trunk"]);
        write(&root, "a.txt", "a\n");
        let repo = Repo::of(&root).unwrap();
        let commit = repo.capture("chat", 1).unwrap();
        assert_eq!(files_of(&repo, &commit), ["a.txt"]);
        write(&root, "a.txt", "changed\n");
        write(&root, "later.txt", "x\n");
        repo.restore(&commit).unwrap();
        assert_eq!(read(&root, "a.txt").as_deref(), Some("a\n"));
        assert_eq!(read(&root, "later.txt"), None);
    }

    #[test]
    fn restoring_brings_back_modified_and_deleted_files_and_removes_added_ones() {
        let (_dir, repo) = repo();
        let root = repo.top.clone();
        // The state at the start of the turn: a change of the person's, and
        // an untracked file.
        write(&root, "a.txt", "a1\n");
        write(&root, "mine.txt", "mine\n");
        let checkpoint = repo.capture("chat", 1).unwrap();
        let head = git(&root, &["rev-parse", "HEAD"]);
        // What the agent did.
        write(&root, "a.txt", "a2\n");
        fs::remove_file(root.join("b.txt")).unwrap();
        fs::remove_file(root.join("mine.txt")).unwrap();
        write(&root, "made.txt", "made\n");
        write(&root, "deep/er/made.txt", "made\n");
        write(&root, "sub/c.txt", "c1\n");
        write(&root, "debug.log", "an ignored file stays\n");
        git(&root, &["add", "made.txt"]);
        let restored = repo.restore(&checkpoint).unwrap();
        assert_eq!(read(&root, "a.txt").as_deref(), Some("a1\n"));
        assert_eq!(read(&root, "b.txt").as_deref(), Some("b0\n"));
        assert_eq!(read(&root, "mine.txt").as_deref(), Some("mine\n"));
        assert_eq!(read(&root, "sub/c.txt").as_deref(), Some("c0\n"));
        assert_eq!(read(&root, "made.txt"), None);
        assert_eq!(read(&root, "deep/er/made.txt"), None);
        assert!(!root.join("deep").exists(), "the empty folders go too");
        assert_eq!(
            read(&root, "debug.log").as_deref(),
            Some("an ignored file stays\n")
        );
        assert_eq!(
            restored,
            Restored {
                written: 4,
                removed: 2
            }
        );
        // HEAD and the branch did not move, and the index was not reset: the
        // file the agent staged is still staged, now missing from disk.
        assert_eq!(git(&root, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&root, &["symbolic-ref", "HEAD"]), "refs/heads/trunk");
        assert_eq!(git(&root, &["diff", "--cached", "--name-only"]), "made.txt");
    }

    #[test]
    fn restoring_only_touches_what_differs_and_what_is_below_the_folder() {
        let (_dir, repo) = repo();
        let root = repo.top.clone();
        let checkpoint = repo.capture("chat", 1).unwrap();
        write(&root, "a.txt", "a2\n");
        write(&root, "sub/c.txt", "c2\n");
        write(&root, "sub/new.txt", "n\n");
        // The conversation works in `sub`: the rest of the repository is
        // somebody else's.
        let sub = Repo::of(&root.join("sub")).unwrap();
        assert_eq!(sub.top, root);
        let restored = sub.restore(&checkpoint).unwrap();
        assert_eq!(read(&root, "a.txt").as_deref(), Some("a2\n"));
        assert_eq!(read(&root, "sub/c.txt").as_deref(), Some("c0\n"));
        assert_eq!(read(&root, "sub/new.txt"), None);
        assert_eq!(
            restored,
            Restored {
                written: 1,
                removed: 1
            }
        );
    }

    #[test]
    fn a_file_that_became_a_folder_is_put_back_and_the_other_way() {
        let (_dir, repo) = repo();
        let root = repo.top.clone();
        let checkpoint = repo.capture("chat", 1).unwrap();
        fs::remove_file(root.join("b.txt")).unwrap();
        write(&root, "b.txt/inside.txt", "x\n");
        fs::remove_dir_all(root.join("sub")).unwrap();
        write(&root, "sub", "now a file\n");
        repo.restore(&checkpoint).unwrap();
        assert_eq!(read(&root, "b.txt").as_deref(), Some("b0\n"));
        assert_eq!(read(&root, "sub/c.txt").as_deref(), Some("c0\n"));
    }

    #[cfg(unix)]
    #[test]
    fn the_mode_of_a_file_comes_back_with_it() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_dir, repo) = repo();
        let root = repo.top.clone();
        write(&root, "run.sh", "#!/bin/sh\n");
        fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        let checkpoint = repo.capture("chat", 1).unwrap();
        fs::remove_file(root.join("run.sh")).unwrap();
        repo.restore(&checkpoint).unwrap();
        let mode = fs::metadata(root.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
    }

    #[test]
    fn checkpoints_are_forgotten_by_chat_by_turn_and_by_count() {
        let (_dir, repo) = repo();
        for turn in 1..=4 {
            repo.capture("one", turn).unwrap();
        }
        repo.capture("two", 1).unwrap();
        assert_eq!(repo.refs_of("one").len(), 4);
        repo.forget_from("one", 3);
        let kept: Vec<u32> = repo
            .refs_of("one")
            .into_iter()
            .map(|(turn, _)| turn)
            .collect();
        assert_eq!(kept.iter().copied().max(), Some(2));
        repo.keep_newest("one", 1);
        let kept: Vec<u32> = repo
            .refs_of("one")
            .into_iter()
            .map(|(turn, _)| turn)
            .collect();
        assert_eq!(kept, [2]);
        repo.forget("one");
        assert!(repo.refs_of("one").is_empty());
        assert_eq!(
            repo.refs_of("two").len(),
            1,
            "another chat's are not touched"
        );
    }

    #[test]
    fn what_a_chat_that_never_closed_left_behind_is_forgotten_by_age() {
        let (_dir, repo) = repo();
        repo.capture("old", 1).unwrap();
        repo.capture("mine", 1).unwrap();
        // Everything is older than a moment from now: all but mine goes.
        repo.forget_older_than(i64::MAX, "mine");
        assert!(repo.refs_of("old").is_empty());
        assert_eq!(repo.refs_of("mine").len(), 1);
        // And nothing is older than the epoch.
        repo.forget_older_than(0, "x");
        assert_eq!(repo.refs_of("mine").len(), 1);
    }

    #[test]
    fn a_checkpoint_is_known_while_its_commit_is_kept() {
        let (_dir, repo) = repo();
        let commit = repo.capture("chat", 1).unwrap();
        assert!(repo.has(&commit));
        assert!(!repo.has("0000000000000000000000000000000000000000"));
    }
}

//! What changed in a checkout, and how: the list of changed files and the
//! unified diff of one of them.
//!
//! Both are asked of git through the [`Runner`], so a worktree on an SSH
//! server or behind the relay answers the same as one on this computer. The
//! parsing is pure and tested on outputs recorded from a real repository:
//!
//! * `git status --porcelain=v1 -z --branch` lists the files. `-z` is what
//!   keeps a name with spaces, quotes or accents as it is (without it git
//!   quotes such names), and with it a rename is two fields, the new name
//!   first.
//! * `git diff` of one file against `HEAD` is the diff; an untracked file has
//!   nothing to be compared with, so it is diffed against `/dev/null`, which
//!   git answers with status 1 when the files differ.
//!
//! A diff is read into lines of a kind (added, removed, context, hunk header)
//! with the line numbers of both sides. A binary file and one whose diff is
//! beyond [`MAX_DIFF_BYTES`] are answers of their own, so a view says what
//! they are instead of drawing them.

use crate::command::{run_on, CommandSpec};
use crate::git::{reject_option_like, Git, GitError};
use crate::runner::Runner;

/// The longest diff, in bytes, that is read into lines; a longer one is
/// answered as [`FileDiff::TooLarge`]. A lock file of a big project is the
/// usual case.
pub const MAX_DIFF_BYTES: usize = 1_000_000;

/// What happened to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// Its content changed (or its mode, or its type).
    Modified,
    /// It is new to git and was added to the index.
    Added,
    /// It was removed.
    Deleted,
    /// It was renamed, and maybe changed.
    Renamed,
    /// It was copied from another file.
    Copied,
    /// It is new and git does not track it yet.
    Untracked,
    /// A merge left it unresolved.
    Conflicted,
}

impl FileStatus {
    /// The status of the two-letter code of `git status --porcelain=v1`.
    pub fn of(index: char, worktree: char) -> Self {
        match (index, worktree) {
            ('?', '?') => Self::Untracked,
            ('A', 'A') | ('D', 'D') => Self::Conflicted,
            (x, y) if x == 'U' || y == 'U' => Self::Conflicted,
            (x, y) if x == 'R' || y == 'R' => Self::Renamed,
            (x, y) if x == 'C' || y == 'C' => Self::Copied,
            (x, y) if x == 'A' || y == 'A' => Self::Added,
            (x, y) if x == 'D' || y == 'D' => Self::Deleted,
            _ => Self::Modified,
        }
    }

    /// The one letter a list shows for it.
    pub fn letter(self) -> char {
        match self {
            Self::Modified => 'M',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
            Self::Untracked => '?',
            Self::Conflicted => 'U',
        }
    }

    /// What it is called in words.
    pub fn words(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Added => "added",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::Untracked => "new, not tracked",
            Self::Conflicted => "conflicted",
        }
    }
}

/// One changed file of a checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// Its path from the root of the checkout, as git spells it.
    pub path: String,
    /// The path it had before, for a renamed or copied file.
    pub from: Option<String>,
    /// What happened to it.
    pub status: FileStatus,
}

impl ChangedFile {
    /// The paths a commit of this file has to name: its own and, for a
    /// rename, the one it left (or the removal would stay behind).
    pub fn paths(&self) -> Vec<&str> {
        let mut paths = vec![self.path.as_str()];
        if self.status == FileStatus::Renamed {
            paths.extend(self.from.as_deref());
        }
        paths
    }
}

/// The changed files of a checkout and the branch they are on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Changes {
    /// The checked-out branch; `None` for a detached head.
    pub branch: Option<String>,
    /// Whether the branch has an upstream to push to.
    pub has_upstream: bool,
    /// The changed files, in git's order (by path).
    pub files: Vec<ChangedFile>,
}

/// Reads the output of `git status --porcelain=v1 -z --branch`.
pub fn parse_changes(porcelain: &str) -> Changes {
    let mut changes = Changes::default();
    let mut fields = porcelain.split('\0').filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        if let Some(header) = field.strip_prefix("## ") {
            (changes.branch, changes.has_upstream) = parse_branch(header);
            continue;
        }
        let mut chars = field.chars();
        let (Some(index), Some(worktree), Some(' ')) = (chars.next(), chars.next(), chars.next())
        else {
            continue;
        };
        let path = chars.as_str().to_owned();
        let status = FileStatus::of(index, worktree);
        // A rename or copy carries the old name as a field of its own.
        let from = matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C');
        let from = from.then(|| fields.next().map(str::to_owned)).flatten();
        changes.files.push(ChangedFile { path, from, status });
    }
    changes
}

/// The branch and whether it has an upstream, from a header such as
/// `main...origin/main [ahead 1]`, `feature`, `HEAD (no branch)` or
/// `No commits yet on main`.
fn parse_branch(header: &str) -> (Option<String>, bool) {
    if header.starts_with("HEAD (no branch)") {
        return (None, false);
    }
    for unborn in ["No commits yet on ", "Initial commit on "] {
        if let Some(branch) = header.strip_prefix(unborn) {
            return (Some(branch.trim().to_owned()), false);
        }
    }
    match header.split_once("...") {
        // `[gone]`: the upstream was deleted, there is nothing to push to.
        Some((branch, rest)) => (Some(branch.to_owned()), !rest.contains("[gone]")),
        None => (Some(header.trim().to_owned()), false),
    }
}

/// The kind of a line of a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// A fact git states about the file: `new file mode`, `rename from`...
    Meta,
    /// The `@@ -1,3 +1,4 @@` line that starts a hunk.
    Hunk,
    /// A line that was added.
    Added,
    /// A line that was removed.
    Removed,
    /// A line around the changes, in both sides.
    Context,
    /// `\ No newline at end of file`.
    Note,
}

/// One line of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// What kind of line it is.
    pub kind: LineKind,
    /// The text, without the `+`, `-` or space that marks the kind.
    pub text: String,
    /// Its number in the old file, for a removed or context line.
    pub old: Option<u32>,
    /// Its number in the new file, for an added or context line.
    pub new: Option<u32>,
}

/// The diff of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDiff {
    /// The lines of the diff.
    Lines(Vec<DiffLine>),
    /// The file is not text.
    Binary,
    /// The diff is longer than [`MAX_DIFF_BYTES`].
    TooLarge {
        /// How long it is, in bytes.
        bytes: usize,
    },
    /// There is nothing to show (an empty new file, a change of mode).
    Empty,
}

/// Reads the output of `git diff` for one file.
pub fn parse_diff(output: &str) -> FileDiff {
    if output.len() > MAX_DIFF_BYTES {
        return FileDiff::TooLarge {
            bytes: output.len(),
        };
    }
    let mut lines = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for raw in output.lines() {
        if raw.starts_with("Binary files ") && raw.ends_with(" differ") {
            return FileDiff::Binary;
        }
        if raw.starts_with("GIT binary patch") {
            return FileDiff::Binary;
        }
        if in_hunk {
            let (marker, text) = raw.split_at(raw.chars().next().map_or(0, char::len_utf8));
            let kind = match marker {
                "+" => Some(LineKind::Added),
                "-" => Some(LineKind::Removed),
                " " => Some(LineKind::Context),
                "\\" => Some(LineKind::Note),
                _ => None,
            };
            if let Some(kind) = kind {
                let line = match kind {
                    LineKind::Added => {
                        new += 1;
                        (None, Some(new))
                    }
                    LineKind::Removed => {
                        old += 1;
                        (Some(old), None)
                    }
                    LineKind::Context => {
                        old += 1;
                        new += 1;
                        (Some(old), Some(new))
                    }
                    _ => (None, None),
                };
                lines.push(DiffLine {
                    kind,
                    text: if kind == LineKind::Note {
                        raw.to_owned()
                    } else {
                        text.to_owned()
                    },
                    old: line.0,
                    new: line.1,
                });
                continue;
            }
            // Not a line of the hunk: the next file's header, or an empty
            // line a hunk's end leaves.
            in_hunk = false;
        }
        if let Some((o, n)) = parse_hunk_header(raw) {
            // The counters hold the last line seen: the first one is `o`. A
            // side that is empty starts at 0 and never counts a line.
            (old, new) = (o.saturating_sub(1), n.saturating_sub(1));
            in_hunk = true;
            lines.push(DiffLine {
                kind: LineKind::Hunk,
                text: raw.to_owned(),
                old: None,
                new: None,
            });
            continue;
        }
        // What precedes the first hunk: the file's header. The names are the
        // tab's, so the `diff --git`, `index`, `---` and `+++` lines are left
        // out; the facts about the file (mode, rename, similarity) stay.
        let boring = raw.starts_with("diff --git ")
            || raw.starts_with("diff --cc ")
            || raw.starts_with("index ")
            || raw.starts_with("--- ")
            || raw.starts_with("+++ ")
            || raw.trim().is_empty();
        if !boring {
            lines.push(DiffLine {
                kind: LineKind::Meta,
                text: raw.to_owned(),
                old: None,
                new: None,
            });
        }
    }
    if lines.is_empty() {
        return FileDiff::Empty;
    }
    FileDiff::Lines(lines)
}

/// The starting lines of `@@ -a,b +c,d @@ ...`, or `None` for another line.
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(' ')?;
    let new = rest.strip_prefix('+')?.split(' ').next()?;
    let start = |range: &str| range.split(',').next()?.parse::<u32>().ok();
    Some((start(old)?, start(new)?))
}

/// A git command in `cwd`, with the prompts that nobody could answer off.
pub(crate) fn git_command<I, S>(cwd: &str, args: I) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    CommandSpec::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .cwd(cwd)
}

impl<R: Runner> Git<'_, R> {
    /// The changed files of the checkout at `cwd`, and its branch. Optional
    /// locks are off, so looking never makes a git command somebody types
    /// wait.
    pub async fn changes(&self, cwd: &str) -> Result<Changes, GitError> {
        reject_option_like("path", cwd)?;
        let command = git_command(
            cwd,
            [
                "--no-optional-locks",
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--untracked-files=all",
            ],
        );
        Ok(parse_changes(&self.run("status", command).await?))
    }

    /// The diff of `file` in the checkout at `cwd`: against `HEAD`, or against
    /// nothing for a file git does not track. A checkout without a commit yet
    /// shows what is in its index.
    pub async fn file_diff(&self, cwd: &str, file: &ChangedFile) -> Result<FileDiff, GitError> {
        reject_option_like("path", cwd)?;
        if file.path.starts_with('-') {
            return Err(GitError::InvalidArgument(format!(
                "path must not start with a dash: {:?}",
                file.path
            )));
        }
        let base = [
            "--literal-pathspecs",
            "--no-pager",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
        ];
        let untracked = file.status == FileStatus::Untracked;
        let mut args: Vec<String> = base.iter().map(|arg| (*arg).to_owned()).collect();
        if untracked {
            args.extend(["--no-index".into(), "--".into(), "/dev/null".into()]);
            args.push(file.path.clone());
        } else {
            args.extend(["-M".into(), "HEAD".into(), "--".into()]);
            args.extend(file.paths().into_iter().map(str::to_owned));
        }
        let first = self.output(git_command(cwd, args)).await?;
        // `--no-index` ends with 1 when the files differ: that is the answer.
        let found = match (untracked, first.status) {
            (_, Some(0)) | (true, Some(1)) => first.stdout,
            _ => {
                // No `HEAD` yet: what was staged is all there is to compare.
                let mut args: Vec<String> = base.iter().map(|arg| (*arg).to_owned()).collect();
                args.extend(["--cached".into(), "--".into()]);
                args.extend(file.paths().into_iter().map(str::to_owned));
                let second = self.output(git_command(cwd, args)).await?;
                if !second.success() {
                    return Err(GitError::Failed {
                        operation: "diff",
                        status: first.status,
                        stderr: first.stderr.trim().to_owned(),
                    });
                }
                second.stdout
            }
        };
        Ok(parse_diff(&found))
    }

    /// Runs `command` where the machine is and returns what it printed, its
    /// status included, whatever it came to.
    pub(crate) async fn output(
        &self,
        command: CommandSpec,
    ) -> Result<crate::runner::Output, GitError> {
        let placed = run_on(self.machine, &command, self.ssh);
        Ok(self.runner.run(&placed).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{Output, ScriptedRunner};
    use leon_core::{Machine, MachineId, MachineKind};

    /// `git status --porcelain=v1 -z --branch --untracked-files=all` of a
    /// repository with a modified file, an added binary, a rename, a removed
    /// file whose name has a space and two files git does not track, one with
    /// accents. `|` stands for the NUL that ends each field.
    fn recorded() -> String {
        "## main...origin/main [ahead 1]| M a.txt|A  bin.dat|R  new.txt|old.txt|D  with space.txt|?? d/f.txt|?? ñandú.txt|"
            .replace('|', "\0")
    }

    #[test]
    fn the_changed_files_are_read_with_the_names_git_wrote() {
        let changes = parse_changes(&recorded());
        assert_eq!(changes.branch.as_deref(), Some("main"));
        assert!(changes.has_upstream);
        let seen: Vec<(&str, FileStatus)> = changes
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.status))
            .collect();
        assert_eq!(
            seen,
            [
                ("a.txt", FileStatus::Modified),
                ("bin.dat", FileStatus::Added),
                ("new.txt", FileStatus::Renamed),
                ("with space.txt", FileStatus::Deleted),
                ("d/f.txt", FileStatus::Untracked),
                ("ñandú.txt", FileStatus::Untracked),
            ]
        );
    }

    #[test]
    fn a_rename_carries_the_name_it_left_and_a_commit_names_both() {
        let changes = parse_changes(&recorded());
        let renamed = &changes.files[2];
        assert_eq!(renamed.from.as_deref(), Some("old.txt"));
        assert_eq!(renamed.paths(), ["new.txt", "old.txt"]);
        assert_eq!(changes.files[0].paths(), ["a.txt"]);
        assert_eq!(changes.files[0].from, None);
    }

    #[test]
    fn the_branch_header_says_the_branch_and_whether_it_can_be_pushed_as_it_is() {
        let of = |header: &str| parse_changes(&format!("## {header}\0"));
        let upstream = |header: &str| (of(header).branch, of(header).has_upstream);
        assert_eq!(upstream("main...origin/main"), (Some("main".into()), true));
        assert_eq!(
            upstream("feature/x...origin/feature/x [ahead 2, behind 1]"),
            (Some("feature/x".into()), true)
        );
        assert_eq!(
            upstream("main...origin/main [gone]"),
            (Some("main".into()), false)
        );
        assert_eq!(upstream("topic"), (Some("topic".into()), false));
        assert_eq!(upstream("HEAD (no branch)"), (None, false));
        assert_eq!(
            upstream("No commits yet on main"),
            (Some("main".into()), false)
        );
        assert_eq!(parse_changes(""), Changes::default());
    }

    #[test]
    fn a_status_is_told_from_the_two_letters_of_git() {
        let of = FileStatus::of;
        assert_eq!(of(' ', 'M'), FileStatus::Modified);
        assert_eq!(of('M', 'M'), FileStatus::Modified);
        assert_eq!(of('T', ' '), FileStatus::Modified);
        assert_eq!(of('A', ' '), FileStatus::Added);
        assert_eq!(of('A', 'M'), FileStatus::Added);
        assert_eq!(of(' ', 'D'), FileStatus::Deleted);
        assert_eq!(of('R', ' '), FileStatus::Renamed);
        assert_eq!(of('C', ' '), FileStatus::Copied);
        assert_eq!(of('?', '?'), FileStatus::Untracked);
        assert_eq!(of('U', 'U'), FileStatus::Conflicted);
        assert_eq!(of('A', 'A'), FileStatus::Conflicted);
        assert_eq!(of('D', 'D'), FileStatus::Conflicted);
        assert_eq!(of('D', 'U'), FileStatus::Conflicted);
        assert_eq!(FileStatus::Untracked.letter(), '?');
        assert_eq!(FileStatus::Renamed.words(), "renamed");
    }

    /// What `git diff HEAD -- a.txt` printed, with the mnemonic prefixes a
    /// configuration may give the names.
    const MODIFIED: &str = "diff --git c/a.txt w/a.txt\nindex 5626abf..f687d0f 100644\n--- c/a.txt\n+++ w/a.txt\n@@ -1 +1,2 @@\n one\n+changed\n";

    #[test]
    fn a_diff_is_lines_with_the_numbers_of_both_sides() {
        let FileDiff::Lines(lines) = parse_diff(MODIFIED) else {
            panic!("a diff of text is lines");
        };
        let seen: Vec<(LineKind, &str, Option<u32>, Option<u32>)> = lines
            .iter()
            .map(|line| (line.kind, line.text.as_str(), line.old, line.new))
            .collect();
        assert_eq!(
            seen,
            [
                (LineKind::Hunk, "@@ -1 +1,2 @@", None, None),
                (LineKind::Context, "one", Some(1), Some(1)),
                (LineKind::Added, "changed", None, Some(2)),
            ]
        );
    }

    #[test]
    fn a_removed_line_that_looks_like_a_file_header_is_still_a_line() {
        let text = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n--- not a header\n+++ nor this\n keep\n";
        let FileDiff::Lines(lines) = parse_diff(text) else {
            panic!("lines");
        };
        assert_eq!(lines[1].kind, LineKind::Removed);
        assert_eq!(lines[1].text, "-- not a header");
        assert_eq!(lines[2].kind, LineKind::Added);
        assert_eq!(lines[2].text, "++ nor this");
        assert_eq!(lines[3].kind, LineKind::Context);
    }

    #[test]
    fn hunks_restart_the_numbers_and_the_end_of_file_note_has_none() {
        let text = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -3,2 +3,2 @@ fn x()\n a\n-b\n+c\n@@ -20 +20 @@\n-z\n\\ No newline at end of file\n+y\n";
        let FileDiff::Lines(lines) = parse_diff(text) else {
            panic!("lines");
        };
        let numbers: Vec<(Option<u32>, Option<u32>)> =
            lines.iter().map(|line| (line.old, line.new)).collect();
        assert_eq!(
            numbers,
            [
                (None, None),
                (Some(3), Some(3)),
                (Some(4), None),
                (None, Some(4)),
                (None, None),
                (Some(20), None),
                (None, None),
                (None, Some(20)),
            ]
        );
        assert_eq!(lines[6].kind, LineKind::Note);
        assert_eq!(lines[6].text, "\\ No newline at end of file");
        assert_eq!(lines[0].text, "@@ -3,2 +3,2 @@ fn x()");
    }

    #[test]
    fn a_new_file_is_numbered_from_one_and_keeps_the_facts_git_states() {
        let text = "diff --git 1/d/f.txt 2/d/f.txt\nnew file mode 100644\nindex 0000000..bca70f3\n--- /dev/null\n+++ 2/d/f.txt\n@@ -0,0 +1,2 @@\n+q\n+r\n";
        let FileDiff::Lines(lines) = parse_diff(text) else {
            panic!("lines");
        };
        assert_eq!(lines[0].kind, LineKind::Meta);
        assert_eq!(lines[0].text, "new file mode 100644");
        assert_eq!(lines[2].new, Some(1));
        assert_eq!(lines[3].new, Some(2));
    }

    #[test]
    fn a_rename_without_changes_says_so_and_has_no_hunks() {
        let text = "diff --git c/old.txt w/new.txt\nsimilarity index 100%\nrename from old.txt\nrename to new.txt\n";
        let FileDiff::Lines(lines) = parse_diff(text) else {
            panic!("lines");
        };
        let said: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(
            said,
            [
                "similarity index 100%",
                "rename from old.txt",
                "rename to new.txt"
            ]
        );
    }

    #[test]
    fn binary_and_huge_and_empty_diffs_are_answers_not_lines() {
        let binary = "diff --git c/bin.dat w/bin.dat\nnew file mode 100644\nindex 0000000..8352675\nBinary files /dev/null and w/bin.dat differ\n";
        assert_eq!(parse_diff(binary), FileDiff::Binary);
        assert_eq!(
            parse_diff("GIT binary patch\nliteral 3\n"),
            FileDiff::Binary
        );
        let huge = format!("@@ -1 +1 @@\n+{}\n", "x".repeat(MAX_DIFF_BYTES));
        assert!(
            matches!(parse_diff(&huge), FileDiff::TooLarge { bytes } if bytes > MAX_DIFF_BYTES)
        );
        assert_eq!(parse_diff(""), FileDiff::Empty);
        assert_eq!(parse_diff("\n"), FileDiff::Empty);
        assert_eq!(
            parse_diff("diff --git a/x b/x\nindex 1..2 100644\n"),
            FileDiff::Empty,
            "a header alone says nothing about the file"
        );
    }

    fn local() -> Machine {
        Machine {
            id: MachineId::local(),
            name: "This computer".into(),
            kind: MachineKind::Local,
        }
    }

    fn file(path: &str, status: FileStatus) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            from: None,
            status,
        }
    }

    #[tokio::test]
    async fn the_changes_are_one_status_with_zero_terminated_names() {
        let runner = ScriptedRunner::new().reply(Output::ok(recorded()));
        let machine = local();
        let ssh = crate::SshOptions::default();
        let changes = Git::new(&runner, &machine, &ssh)
            .changes("/srv/wt")
            .await
            .unwrap();
        assert_eq!(changes.files.len(), 6);
        let call = &runner.calls()[0];
        assert_eq!(call.program, "git");
        assert_eq!(
            call.args,
            [
                "--no-optional-locks",
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--untracked-files=all"
            ]
        );
        assert_eq!(call.cwd.as_deref(), Some("/srv/wt"));
        assert!(call
            .env
            .contains(&("GIT_TERMINAL_PROMPT".into(), "0".into())));
    }

    #[tokio::test]
    async fn a_tracked_file_is_diffed_against_head_with_the_name_it_left() {
        let runner = ScriptedRunner::new().reply(Output::ok(MODIFIED));
        let machine = local();
        let ssh = crate::SshOptions::default();
        let renamed = ChangedFile {
            path: "new.txt".into(),
            from: Some("old.txt".into()),
            status: FileStatus::Renamed,
        };
        let diff = Git::new(&runner, &machine, &ssh)
            .file_diff("/srv/wt", &renamed)
            .await
            .unwrap();
        assert!(matches!(diff, FileDiff::Lines(_)));
        assert_eq!(
            runner.calls()[0].args,
            [
                "--literal-pathspecs",
                "--no-pager",
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "-M",
                "HEAD",
                "--",
                "new.txt",
                "old.txt"
            ]
        );
    }

    #[tokio::test]
    async fn an_untracked_file_is_diffed_against_nothing_and_status_one_is_the_answer() {
        let new_file = "diff --git 1/d/f.txt 2/d/f.txt\nnew file mode 100644\n--- /dev/null\n+++ 2/d/f.txt\n@@ -0,0 +1 @@\n+q\n";
        let runner = ScriptedRunner::new().reply(Output {
            status: Some(1),
            stdout: new_file.into(),
            stderr: String::new(),
        });
        let machine = local();
        let ssh = crate::SshOptions::default();
        let diff = Git::new(&runner, &machine, &ssh)
            .file_diff("/srv/wt", &file("d/f.txt", FileStatus::Untracked))
            .await
            .unwrap();
        assert!(matches!(diff, FileDiff::Lines(_)));
        assert_eq!(runner.calls().len(), 1, "no second try");
        let args = &runner.calls()[0].args;
        assert!(args.ends_with(&[
            "--no-index".into(),
            "--".into(),
            "/dev/null".into(),
            "d/f.txt".into()
        ]));
    }

    #[tokio::test]
    async fn a_checkout_without_a_commit_falls_back_to_what_is_staged() {
        let runner = ScriptedRunner::new()
            .reply(Output::failed(128, "fatal: bad revision 'HEAD'"))
            .reply(Output::ok(MODIFIED));
        let machine = local();
        let ssh = crate::SshOptions::default();
        let diff = Git::new(&runner, &machine, &ssh)
            .file_diff("/srv/wt", &file("a.txt", FileStatus::Added))
            .await
            .unwrap();
        assert!(matches!(diff, FileDiff::Lines(_)));
        assert!(runner.calls()[1].args.contains(&"--cached".to_owned()));
    }

    #[tokio::test]
    async fn a_diff_git_cannot_make_is_the_error_of_the_first_try_and_a_dash_name_is_refused() {
        let runner = ScriptedRunner::new()
            .reply(Output::failed(128, "fatal: not a git repository"))
            .reply(Output::failed(128, "fatal: also not"));
        let machine = local();
        let ssh = crate::SshOptions::default();
        let git = Git::new(&runner, &machine, &ssh);
        let error = git
            .file_diff("/srv/wt", &file("a.txt", FileStatus::Modified))
            .await
            .expect_err("git said no");
        assert!(
            error.to_string().contains("not a git repository"),
            "{error}"
        );
        let refused = git
            .file_diff("/srv/wt", &file("-p", FileStatus::Modified))
            .await;
        assert!(matches!(refused, Err(GitError::InvalidArgument(_))));
    }
}

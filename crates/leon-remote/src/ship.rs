//! From a changed checkout to a pull request: commit, push, open the pull
//! request, and the help of an agent to word them.
//!
//! Every step is a command run through the [`Runner`] where the checkout is,
//! so it is the same on this computer, over SSH and over the relay. What can
//! be decided without running anything is a pure function with tests: the
//! arguments of each command, the title and body a pull request starts with
//! from its commits, the address `gh` prints, and the clean-up of what an
//! agent answers.
//!
//! Nothing here is destructive. There is no force push, no reset, no amend and
//! no discarding of changes: a commit adds a commit, a push adds commits to the
//! remote branch (and sets its upstream when it has none), and a pull request
//! is opened for a branch that exists.
//!
//! What git, `gh` and the hooks they run print is kept whole. A failure is
//! [`ShipError::Failed`] and its text is what the command printed, standard
//! output and standard error together, because a hook may write to either and
//! `git commit` itself says "nothing to commit" on the first.

use thiserror::Error;

use crate::changes::{git_command, ChangedFile};
use crate::command::{run_on, CommandSpec};
use crate::git::{reject_option_like, Git};
use crate::github::Github;
use crate::runner::{Output, RunError, Runner};

/// The longest piece of a diff given to an agent, in bytes. A longer one is
/// cut at a line, and the agent is told it was.
pub const MAX_PROMPT_BYTES: usize = 60_000;

/// How many commits are read for a pull request's title and body.
const MAX_COMMITS: usize = 50;

/// The steps of getting a change to a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `git add` and `git commit`.
    Commit,
    /// `git push`.
    Push,
    /// `gh pr create`.
    PullRequest,
    /// An agent asked for a message.
    Suggestion,
}

impl Step {
    /// What the step is called in a sentence.
    pub fn words(self) -> &'static str {
        match self {
            Self::Commit => "The commit",
            Self::Push => "The push",
            Self::PullRequest => "The pull request",
            Self::Suggestion => "The suggestion",
        }
    }
}

/// Why a step did not happen.
#[derive(Debug, Error)]
pub enum ShipError {
    /// The command could not be started (`gh` is not installed).
    #[error("{0}")]
    Run(#[from] RunError),
    /// The request was refused before anything ran.
    #[error("{0}")]
    Invalid(String),
    /// The command ran and failed. The text is what it printed, whole.
    #[error("{text}")]
    Failed {
        /// The step that failed.
        step: Step,
        /// The exit status; absent when a signal ended it.
        status: Option<i32>,
        /// Standard output and standard error, as printed.
        text: String,
    },
}

impl From<crate::git::GitError> for ShipError {
    fn from(error: crate::git::GitError) -> Self {
        match error {
            crate::git::GitError::Run(error) => Self::Run(error),
            other => Self::Invalid(other.to_string()),
        }
    }
}

/// Standard output and standard error of a command, the way they are shown.
pub fn printed(output: &Output) -> String {
    let (out, err) = (output.stdout.trim_end(), output.stderr.trim_end());
    match (out.is_empty(), err.is_empty()) {
        (true, true) => String::new(),
        (false, true) => out.to_owned(),
        (true, false) => err.to_owned(),
        (false, false) => format!("{out}\n{err}"),
    }
}

/// A finished command as a result: its text when it succeeded, the failure of
/// `step` with its text when it did not.
fn judged(step: Step, what: &str, output: Output) -> Result<String, ShipError> {
    let text = printed(&output);
    if output.success() {
        return Ok(text);
    }
    let text = if text.is_empty() {
        match output.status {
            Some(status) => format!("{what} failed (status {status}) and printed nothing."),
            None => format!("{what} was ended before it finished."),
        }
    } else {
        text
    };
    Err(ShipError::Failed {
        step,
        status: output.status,
        text,
    })
}

// ----- commit ----------------------------------------------------------------

/// The commands of a commit: `git add` of what is chosen, then `git commit`
/// with the message on standard input. `files` is `None` for every changed
/// file (`git add -A`) and the chosen ones otherwise; the commit then names the
/// same paths, so anything else that happens to be staged is left out of it.
/// Paths are taken literally, whatever characters they have.
pub fn commit_commands(
    cwd: &str,
    message: &str,
    files: Option<&[ChangedFile]>,
) -> Result<Vec<CommandSpec>, ShipError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(ShipError::Invalid(
            "Write a commit message first.".to_owned(),
        ));
    }
    if files.is_some_and(<[ChangedFile]>::is_empty) {
        return Err(ShipError::Invalid(
            "Choose at least one file to commit.".to_owned(),
        ));
    }
    let mut add = vec!["--literal-pathspecs", "add"];
    let mut commit = vec!["--literal-pathspecs", "commit", "-F", "-"];
    match files {
        None => add.push("-A"),
        Some(files) => {
            // A rename is already in the index under its new name, and the old
            // one is no longer a path `git add` can find; the commit still
            // has to name both, or the removal of the old one stays behind.
            add.push("--");
            add.extend(files.iter().map(|file| file.path.as_str()));
            commit.push("--");
            commit.extend(files.iter().flat_map(ChangedFile::paths));
        }
    }
    Ok(vec![
        git_command(cwd, add),
        git_command(cwd, commit).stdin(format!("{message}\n")),
    ])
}

impl<R: Runner> Git<'_, R> {
    /// Commits the chosen files (every changed file with `None`) with
    /// `message`. Answers what git printed, which hooks may have added to.
    pub async fn commit(
        &self,
        cwd: &str,
        message: &str,
        files: Option<&[ChangedFile]>,
    ) -> Result<String, ShipError> {
        reject_option_like("path", cwd).map_err(|error| ShipError::Invalid(error.to_string()))?;
        let commands = commit_commands(cwd, message, files)?;
        let mut shown = String::new();
        for (command, what) in commands.into_iter().zip(["git add", "git commit"]) {
            let output = self.output(command).await?;
            shown = judged(Step::Commit, what, output)?;
        }
        Ok(shown)
    }

    // ----- push ----------------------------------------------------------------

    /// Pushes the branch of the checkout at `cwd`, setting its upstream to
    /// `origin` when it has none. Never forced.
    pub async fn push(&self, cwd: &str) -> Result<String, ShipError> {
        let changes = self
            .changes(cwd)
            .await
            .map_err(|error| ShipError::Invalid(error.to_string()))?;
        let args = push_args(changes.branch.as_deref(), changes.has_upstream)?;
        let output = self.output(git_command(cwd, args)).await?;
        judged(Step::Push, "git push", output)
    }

    // ----- what a pull request starts with ---------------------------------------

    /// The branch a pull request of this checkout is for, the branch it goes
    /// into by default and the commits between them, newest last.
    pub async fn pull_request_start(&self, cwd: &str) -> Result<PullRequestStart, ShipError> {
        let changes = self
            .changes(cwd)
            .await
            .map_err(|error| ShipError::Invalid(error.to_string()))?;
        let base = self.default_base(cwd).await;
        let mut commits = Vec::new();
        for range in [format!("origin/{base}..HEAD"), format!("{base}..HEAD")] {
            let limit = format!("--max-count={MAX_COMMITS}");
            let command = git_command(
                cwd,
                [
                    "log",
                    "--reverse",
                    limit.as_str(),
                    "--format=%s%x1f%b%x1e",
                    range.as_str(),
                ],
            );
            let output = self.output(command).await?;
            if output.success() {
                commits = parse_log(&output.stdout);
                break;
            }
        }
        let (title, body) = prefill_pull_request(changes.branch.as_deref(), &commits);
        Ok(PullRequestStart {
            branch: changes.branch,
            base,
            title,
            body,
            commits,
        })
    }

    /// The branch pull requests of this repository go into: the one
    /// `origin/HEAD` names, else `main`.
    async fn default_base(&self, cwd: &str) -> String {
        let command = git_command(cwd, ["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
        match self.output(command).await {
            Ok(output) if output.success() => output
                .stdout
                .trim()
                .strip_prefix("origin/")
                .filter(|base| !base.is_empty())
                .map_or_else(|| "main".to_owned(), str::to_owned),
            _ => "main".to_owned(),
        }
    }

    /// What an agent is shown to word a commit: the diff of the chosen files
    /// (every changed one with `None`) against `HEAD`, and the names of the
    /// files git does not track yet, cut at [`MAX_PROMPT_BYTES`].
    ///
    /// When files are chosen and none of them is tracked, git is not asked for
    /// a diff at all: a `git diff` without paths would show every tracked
    /// change, files that are not in the commit. The agent is then given the
    /// names of the new files alone, and a choice with nothing in it is
    /// refused before any agent is asked.
    pub async fn diff_for_message(
        &self,
        cwd: &str,
        files: Option<&[ChangedFile]>,
    ) -> Result<String, ShipError> {
        let tracked: Vec<&ChangedFile> = files
            .map(|files| {
                files
                    .iter()
                    .filter(|file| file.status != crate::changes::FileStatus::Untracked)
                    .collect()
            })
            .unwrap_or_default();
        let new: Vec<&str> = files
            .map(|files| {
                files
                    .iter()
                    .filter(|file| file.status == crate::changes::FileStatus::Untracked)
                    .map(|file| file.path.as_str())
                    .collect()
            })
            .unwrap_or_default();
        if files.is_some() && tracked.is_empty() && new.is_empty() {
            return Err(ShipError::Invalid(
                "There are no files chosen to word a commit from.".to_owned(),
            ));
        }
        let mut text = String::new();
        if files.is_none() || !tracked.is_empty() {
            let mut args: Vec<String> = ["--literal-pathspecs", "--no-pager", "diff", "--no-color"]
                .into_iter()
                .map(str::to_owned)
                .collect();
            args.extend([
                "--no-ext-diff".into(),
                "--no-textconv".into(),
                "HEAD".into(),
            ]);
            if files.is_some() {
                args.push("--".into());
                args.extend(
                    tracked
                        .iter()
                        .flat_map(|file| file.paths())
                        .map(str::to_owned),
                );
            }
            let output = self.output(git_command(cwd, args)).await?;
            // A checkout without a commit has no `HEAD` to compare with.
            if output.success() {
                text = output.stdout;
            }
        }
        if !new.is_empty() {
            text.push_str("\nNew files, not shown: ");
            text.push_str(&new.join(", "));
            text.push('\n');
        }
        Ok(cut_for_prompt(&text))
    }
}

/// What a pull request starts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestStart {
    /// The branch of the checkout; `None` for a detached head.
    pub branch: Option<String>,
    /// The branch it goes into by default.
    pub base: String,
    /// The title it starts with.
    pub title: String,
    /// The body it starts with.
    pub body: String,
    /// The commits it carries, oldest first.
    pub commits: Vec<Commit>,
}

/// The arguments of `git push` for a branch: plain when the branch has an
/// upstream, setting it on `origin` when it has none. A detached head has no
/// branch to push.
pub fn push_args(branch: Option<&str>, has_upstream: bool) -> Result<Vec<String>, ShipError> {
    let Some(branch) = branch else {
        return Err(ShipError::Invalid(
            "This worktree has a detached head: check out a branch before pushing.".to_owned(),
        ));
    };
    reject_option_like("branch", branch).map_err(|error| ShipError::Invalid(error.to_string()))?;
    Ok(if has_upstream {
        vec!["push".to_owned()]
    } else {
        ["push", "--set-upstream", "origin", branch]
            .into_iter()
            .map(str::to_owned)
            .collect()
    })
}

// ----- commits, as a pull request starts from them ------------------------------

/// One commit: its subject line and the rest of its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The first line.
    pub subject: String,
    /// What follows it, trimmed; empty for a one-line message.
    pub body: String,
}

/// Reads `git log --format=%s%x1f%b%x1e`: subject and body of each commit,
/// separated by the unit separator and ended by the record separator.
pub fn parse_log(output: &str) -> Vec<Commit> {
    output
        .split('\u{1e}')
        .filter_map(|record| {
            let (subject, body) = record.split_once('\u{1f}').unwrap_or((record, ""));
            let subject = subject.trim();
            (!subject.is_empty()).then(|| Commit {
                subject: subject.to_owned(),
                body: body.trim().to_owned(),
            })
        })
        .collect()
}

/// The title and body a pull request starts with. One commit is its own
/// subject and body. Several are the branch's name worded as a title (the
/// first subject without one) and the subjects as a list, oldest first. No
/// commits leave the body empty and the branch's name as the title.
pub fn prefill_pull_request(branch: Option<&str>, commits: &[Commit]) -> (String, String) {
    match commits {
        [] => (branch.map(humanise).unwrap_or_default(), String::new()),
        [only] => (only.subject.clone(), only.body.clone()),
        [first, ..] => {
            let title = branch
                .map(humanise)
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| first.subject.clone());
            let list: Vec<String> = commits
                .iter()
                .map(|commit| format!("- {}", commit.subject))
                .collect();
            (title, list.join("\n"))
        }
    }
}

/// A branch name as a title: `fix-the_tabs` is `Fix the tabs`.
fn humanise(branch: &str) -> String {
    let spaced = branch.replace(['-', '_'], " ");
    let mut chars = spaced.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ----- the pull request ------------------------------------------------------------

/// What `gh pr create` is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestDraft {
    /// The title.
    pub title: String,
    /// The body, Markdown.
    pub body: String,
    /// The branch it goes into.
    pub base: String,
    /// Whether it is opened as a draft.
    pub draft: bool,
}

/// The arguments of `gh pr create` for a draft; the body goes on standard
/// input. A title and a base are required.
pub fn create_pull_request_args(draft: &PullRequestDraft) -> Result<Vec<String>, ShipError> {
    let title = draft.title.trim();
    let base = draft.base.trim();
    if title.is_empty() {
        return Err(ShipError::Invalid(
            "Give the pull request a title first.".to_owned(),
        ));
    }
    if base.is_empty() || base.starts_with('-') {
        return Err(ShipError::Invalid(
            "Name the branch the pull request goes into.".to_owned(),
        ));
    }
    let mut args: Vec<String> = [
        "pr",
        "create",
        "--title",
        title,
        "--body-file",
        "-",
        "--base",
        base,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if draft.draft {
        args.push("--draft".to_owned());
    }
    Ok(args)
}

/// The address `gh pr create` printed: the last line that is one. Its other
/// lines (a warning, the destination) go to standard error or come before it.
pub fn parse_pull_request_url(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .rev()
        .find(|line| line.starts_with("https://") && !line.contains(char::is_whitespace))
        .map(str::to_owned)
}

impl<R: Runner> Github<'_, R> {
    /// Opens a pull request for the branch of the checkout at `cwd` and
    /// answers its address. The branch must be pushed already.
    pub async fn create_pull_request(
        &self,
        cwd: &str,
        draft: &PullRequestDraft,
    ) -> Result<String, ShipError> {
        let args = create_pull_request_args(draft)?;
        let command = CommandSpec::new("gh")
            .args(args)
            .env("GH_PAGER", "cat")
            .env("NO_COLOR", "1")
            .env("GH_PROMPT_DISABLED", "1")
            .cwd(cwd)
            .stdin(format!("{}\n", draft.body.trim()));
        let placed = run_on(self.machine, &command, self.ssh);
        let output = self.runner.run(&placed).await.map_err(|error| match error {
            RunError::Spawn { .. } => ShipError::Invalid(format!(
                "{error}. Install the GitHub CLI (cli.github.com) on that machine and sign in with `gh auth login`."
            )),
            other => ShipError::Run(other),
        })?;
        let text = judged(Step::PullRequest, "gh pr create", output.clone())?;
        parse_pull_request_url(&output.stdout).ok_or(ShipError::Failed {
            step: Step::PullRequest,
            status: output.status,
            text: if text.is_empty() {
                "gh did not print the address of the pull request.".to_owned()
            } else {
                format!("gh did not print the address of the pull request. It printed:\n{text}")
            },
        })
    }
}

// ----- asking an agent --------------------------------------------------------------

/// What an agent is asked to word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suggest {
    /// A commit message, from a diff.
    Commit,
    /// A pull request's title and body, from its commits.
    PullRequest,
}

/// The instruction given to the agent; what it reads (the diff, the commits)
/// is its standard input.
pub fn suggestion_prompt(what: Suggest) -> &'static str {
    match what {
        Suggest::Commit => {
            "Write a git commit message for the changes in the diff on standard input. \
             The first line is a summary in the imperative mood of at most 72 characters. \
             Add a blank line and a short explanation of why only when it helps. \
             Reply with the message alone: no quotes, no code fence, no preface. \
             Do not run commands and do not change any file."
        }
        Suggest::PullRequest => {
            "Write a pull request title and description for the commits on standard input. \
             The first line is the title, at most 72 characters. Then a blank line and a \
             short description in Markdown of what changed and why. \
             Reply with that alone: no quotes, no code fence, no preface. \
             Do not run commands and do not change any file."
        }
    }
}

/// Cuts text for an agent at [`MAX_PROMPT_BYTES`], at the end of a line, and
/// says so when it did.
pub fn cut_for_prompt(text: &str) -> String {
    if text.len() <= MAX_PROMPT_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_PROMPT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let kept = &text[..end];
    let kept = kept.rfind('\n').map_or(kept, |at| &kept[..at]);
    format!("{kept}\n\n[The rest of the diff was left out: it is too long.]\n")
}

/// The answer of an agent as the text it meant: without a code fence around
/// it, quotes, a label, the attribution lines some agents append and the
/// notices a tool manager prints before the program runs.
pub fn clean_suggestion(raw: &str) -> String {
    let mut lines: Vec<&str> = raw
        .lines()
        .filter(|line| {
            let lower = line.trim().to_lowercase();
            !(lower.starts_with("co-authored-by:")
                || lower.starts_with("generated with")
                || lower.starts_with("mise "))
        })
        .collect();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if lines
        .first()
        .is_some_and(|line| line.trim_start().starts_with("```"))
    {
        lines.remove(0);
        if lines.last().is_some_and(|line| line.trim() == "```") {
            lines.pop();
        }
    }
    let mut text = lines.join("\n").trim().to_owned();
    for label in ["commit message:", "title:"] {
        if text.to_lowercase().starts_with(label) {
            text = text[label.len()..].trim_start().to_owned();
        }
    }
    let quoted = text.len() > 1
        && ((text.starts_with('"') && text.ends_with('"'))
            || (text.starts_with('`') && text.ends_with('`')));
    if quoted && !text[1..text.len() - 1].contains(['"', '`', '\n']) {
        text = text[1..text.len() - 1].to_owned();
    }
    // Trailing blanks left where the attribution lines were.
    text.trim_end().to_owned()
}

/// A pull request's title and body from what an agent answered: the first
/// line (without a Markdown heading mark) and the rest.
pub fn split_title_and_body(answer: &str) -> (String, String) {
    let cleaned = clean_suggestion(answer);
    let (first, rest) = cleaned.split_once('\n').unwrap_or((&cleaned, ""));
    let title = first.trim().trim_start_matches('#').trim();
    (title.to_owned(), rest.trim().to_owned())
}

/// The command that asks an agent: `program`, its non-interactive arguments,
/// then the instruction, with `input` on standard input.
pub fn suggestion_command(
    cwd: &str,
    program: &str,
    headless: &[String],
    what: Suggest,
    input: &str,
) -> CommandSpec {
    CommandSpec::new(program)
        .args(headless.iter().cloned())
        .arg(suggestion_prompt(what))
        .env("NO_COLOR", "1")
        .cwd(cwd)
        .stdin(input.to_owned())
}

impl<R: Runner> Git<'_, R> {
    /// Asks an agent, headless, to word something from `input`, and answers
    /// its text cleaned. A failure is the text the agent printed.
    pub async fn suggest(
        &self,
        cwd: &str,
        program: &str,
        headless: &[String],
        what: Suggest,
        input: &str,
    ) -> Result<String, ShipError> {
        let command = suggestion_command(cwd, program, headless, what, input);
        let output = self.output(command).await?;
        let text = judged(Step::Suggestion, program, output)?;
        let cleaned = clean_suggestion(&text);
        if cleaned.is_empty() {
            return Err(ShipError::Invalid(format!("{program} answered nothing.")));
        }
        Ok(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::FileStatus;
    use crate::runner::ScriptedRunner;
    use crate::SshOptions;
    use leon_core::{Machine, MachineId, MachineKind};

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

    #[test]
    fn committing_everything_adds_everything_and_sends_the_message_on_stdin() {
        let commands =
            commit_commands("/srv/wt", "  Fix the tabs\n\nThey leaked.  \n", None).unwrap();
        assert_eq!(commands[0].args, ["--literal-pathspecs", "add", "-A"]);
        assert_eq!(
            commands[1].args,
            ["--literal-pathspecs", "commit", "-F", "-"]
        );
        assert_eq!(
            commands[1].stdin.as_deref(),
            Some(b"Fix the tabs\n\nThey leaked.\n".as_slice())
        );
        assert_eq!(commands[1].cwd.as_deref(), Some("/srv/wt"));
        assert!(commands[0].stdin.is_none());
    }

    #[test]
    fn committing_some_files_names_them_in_the_add_and_in_the_commit() {
        let renamed = ChangedFile {
            path: "new.txt".into(),
            from: Some("old.txt".into()),
            status: FileStatus::Renamed,
        };
        let files = [file("-odd name.txt", FileStatus::Modified), renamed];
        let commands = commit_commands("/srv/wt", "Rename", Some(&files)).unwrap();
        assert_eq!(
            commands[0].args,
            [
                "--literal-pathspecs",
                "add",
                "--",
                "-odd name.txt",
                "new.txt"
            ],
            "the old name of a rename is gone from the tree: add cannot find it"
        );
        assert_eq!(
            commands[1].args,
            [
                "--literal-pathspecs",
                "commit",
                "-F",
                "-",
                "--",
                "-odd name.txt",
                "new.txt",
                "old.txt"
            ],
            "what else is staged stays out of this commit"
        );
    }

    #[test]
    fn a_commit_without_a_message_or_files_is_refused_before_anything_runs() {
        assert!(matches!(
            commit_commands("/w", "  \n", None),
            Err(ShipError::Invalid(text)) if text.contains("message")
        ));
        assert!(matches!(
            commit_commands("/w", "x", Some(&[])),
            Err(ShipError::Invalid(text)) if text.contains("file")
        ));
    }

    #[tokio::test]
    async fn a_commit_runs_add_then_commit_and_says_what_git_printed() {
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok("[main 1a2b3c4] Fix\n 1 file changed\n"));
        let machine = local();
        let ssh = SshOptions::default();
        let said = Git::new(&runner, &machine, &ssh)
            .commit("/srv/wt", "Fix", None)
            .await
            .unwrap();
        assert!(said.starts_with("[main 1a2b3c4] Fix"));
        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].args[1], "add");
        assert_eq!(calls[1].args[1], "commit");
    }

    #[tokio::test]
    async fn a_hook_that_refuses_the_commit_is_shown_whole_from_both_streams() {
        let runner = ScriptedRunner::new().reply(Output::ok("")).reply(Output {
            status: Some(1),
            stdout: "running lint...\n".into(),
            stderr: "lint failed: 3 problems\n".into(),
        });
        let machine = local();
        let ssh = SshOptions::default();
        let error = Git::new(&runner, &machine, &ssh)
            .commit("/srv/wt", "Fix", None)
            .await
            .expect_err("the hook said no");
        assert_eq!(
            error.to_string(),
            "running lint...\nlint failed: 3 problems"
        );
        assert!(matches!(
            error,
            ShipError::Failed {
                step: Step::Commit,
                status: Some(1),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn nothing_to_commit_is_git_s_own_words_from_standard_output() {
        let runner = ScriptedRunner::new().reply(Output::ok("")).reply(Output {
            status: Some(1),
            stdout: "On branch main\nnothing to commit, working tree clean\n".into(),
            stderr: String::new(),
        });
        let machine = local();
        let ssh = SshOptions::default();
        let error = Git::new(&runner, &machine, &ssh)
            .commit("/srv/wt", "Fix", None)
            .await
            .expect_err("nothing was committed");
        assert!(error.to_string().contains("nothing to commit"), "{error}");
    }

    #[tokio::test]
    async fn a_failure_that_prints_nothing_still_says_which_command_failed() {
        let runner = ScriptedRunner::new().reply(Output::failed(128, ""));
        let machine = local();
        let ssh = SshOptions::default();
        let error = Git::new(&runner, &machine, &ssh)
            .commit("/srv/wt", "Fix", None)
            .await
            .expect_err("git add failed");
        assert_eq!(
            error.to_string(),
            "git add failed (status 128) and printed nothing."
        );
        assert_eq!(runner.calls().len(), 1, "the commit is not tried");
    }

    #[test]
    fn a_branch_with_an_upstream_is_pushed_plain_and_one_without_sets_it() {
        assert_eq!(push_args(Some("feature/x"), true).unwrap(), ["push"]);
        assert_eq!(
            push_args(Some("feature/x"), false).unwrap(),
            ["push", "--set-upstream", "origin", "feature/x"]
        );
    }

    #[test]
    fn nothing_that_forces_a_push_is_ever_built_and_a_detached_head_has_nothing_to_push() {
        for upstream in [true, false] {
            let args = push_args(Some("main"), upstream).unwrap();
            assert!(!args.iter().any(|arg| arg.contains("force") || arg == "-f"));
        }
        assert!(matches!(
            push_args(None, false),
            Err(ShipError::Invalid(text)) if text.contains("detached")
        ));
        assert!(push_args(Some("--force"), false).is_err());
    }

    #[tokio::test]
    async fn pushing_reads_the_branch_and_its_upstream_from_git_first() {
        let runner = ScriptedRunner::new()
            .reply(Output::ok("## topic\0 M a.txt\0"))
            .reply(Output {
                status: Some(0),
                stdout: String::new(),
                stderr: "To github.com:zavudev/leon.git\n * [new branch] topic -> topic\n".into(),
            });
        let machine = local();
        let ssh = SshOptions::default();
        let said = Git::new(&runner, &machine, &ssh)
            .push("/srv/wt")
            .await
            .unwrap();
        assert!(said.contains("[new branch]"));
        assert_eq!(
            runner.calls()[1].args,
            ["push", "--set-upstream", "origin", "topic"]
        );
    }

    #[tokio::test]
    async fn a_rejected_push_is_shown_as_git_said_it() {
        let runner = ScriptedRunner::new()
            .reply(Output::ok("## topic...origin/topic [behind 2]\0"))
            .reply(Output::failed(
                1,
                "! [rejected] topic -> topic (non-fast-forward)\nhint: pull first\n",
            ));
        let machine = local();
        let ssh = SshOptions::default();
        let error = Git::new(&runner, &machine, &ssh)
            .push("/srv/wt")
            .await
            .expect_err("rejected");
        assert!(error.to_string().contains("non-fast-forward"));
        assert!(error.to_string().contains("hint: pull first"));
        assert_eq!(runner.calls()[1].args, ["push"], "never forced");
    }

    #[test]
    fn the_log_is_read_into_subjects_and_bodies() {
        let log =
            "Add the view\u{1f}It lists files.\n\nAnd diffs.\n\u{1e}\nFix a crash\u{1f}\n\u{1e}\n";
        assert_eq!(
            parse_log(log),
            [
                Commit {
                    subject: "Add the view".into(),
                    body: "It lists files.\n\nAnd diffs.".into()
                },
                Commit {
                    subject: "Fix a crash".into(),
                    body: String::new()
                }
            ]
        );
        assert!(parse_log("").is_empty());
    }

    fn commit(subject: &str, body: &str) -> Commit {
        Commit {
            subject: subject.into(),
            body: body.into(),
        }
    }

    #[test]
    fn one_commit_is_the_pull_request_and_several_are_listed_under_the_branch() {
        assert_eq!(
            prefill_pull_request(Some("fix-tabs"), &[commit("Fix the tabs", "Details.")]),
            ("Fix the tabs".to_owned(), "Details.".to_owned())
        );
        assert_eq!(
            prefill_pull_request(
                Some("feature/login_page"),
                &[commit("Add form", ""), commit("Wire it", "x")]
            ),
            (
                "Feature/login page".to_owned(),
                "- Add form\n- Wire it".to_owned()
            )
        );
        assert_eq!(
            prefill_pull_request(None, &[commit("A", ""), commit("B", "")]),
            ("A".to_owned(), "- A\n- B".to_owned())
        );
        assert_eq!(
            prefill_pull_request(Some("new-idea"), &[]),
            ("New idea".to_owned(), String::new())
        );
        assert_eq!(
            prefill_pull_request(None, &[]),
            (String::new(), String::new())
        );
    }

    fn draft() -> PullRequestDraft {
        PullRequestDraft {
            title: " Fix the tabs ".into(),
            body: "They leaked.".into(),
            base: "main".into(),
            draft: false,
        }
    }

    #[test]
    fn a_pull_request_is_created_with_the_body_on_stdin_and_a_draft_on_request() {
        assert_eq!(
            create_pull_request_args(&draft()).unwrap(),
            [
                "pr",
                "create",
                "--title",
                "Fix the tabs",
                "--body-file",
                "-",
                "--base",
                "main"
            ]
        );
        let as_draft = PullRequestDraft {
            draft: true,
            ..draft()
        };
        assert_eq!(
            create_pull_request_args(&as_draft)
                .unwrap()
                .last()
                .map(String::as_str),
            Some("--draft")
        );
        let untitled = PullRequestDraft {
            title: " ".into(),
            ..draft()
        };
        assert!(create_pull_request_args(&untitled).is_err());
        let unbased = PullRequestDraft {
            base: "-x".into(),
            ..draft()
        };
        assert!(create_pull_request_args(&unbased).is_err());
    }

    #[test]
    fn the_address_is_the_last_line_that_is_one() {
        assert_eq!(
            parse_pull_request_url(
                "Creating pull request for topic into main in zavudev/leon\n\nhttps://github.com/zavudev/leon/pull/41\n"
            )
            .as_deref(),
            Some("https://github.com/zavudev/leon/pull/41")
        );
        assert_eq!(parse_pull_request_url("no address here\n"), None);
        assert_eq!(parse_pull_request_url("see https://x.test/a b\n"), None);
    }

    #[tokio::test]
    async fn gh_is_asked_in_the_checkout_without_prompts_and_its_address_comes_back() {
        let runner = ScriptedRunner::new().reply(Output {
            status: Some(0),
            stdout: "https://github.com/zavudev/leon/pull/41\n".into(),
            stderr: "Warning: 1 uncommitted change\n".into(),
        });
        let machine = local();
        let ssh = SshOptions::default();
        let url = Github::new(&runner, &machine, &ssh)
            .create_pull_request("/srv/wt", &draft())
            .await
            .unwrap();
        assert_eq!(url, "https://github.com/zavudev/leon/pull/41");
        let call = &runner.calls()[0];
        assert_eq!(call.program, "gh");
        assert_eq!(call.cwd.as_deref(), Some("/srv/wt"));
        assert_eq!(call.stdin.as_deref(), Some(b"They leaked.\n".as_slice()));
        assert!(call
            .env
            .contains(&("GH_PROMPT_DISABLED".into(), "1".into())));
    }

    #[tokio::test]
    async fn gh_failing_or_missing_says_so_in_its_own_words() {
        let machine = local();
        let ssh = SshOptions::default();
        let refused = ScriptedRunner::new().reply(Output::failed(
            1,
            "a pull request for branch \"topic\" into branch \"main\" already exists:\nhttps://github.com/zavudev/leon/pull/40\n",
        ));
        let error = Github::new(&refused, &machine, &ssh)
            .create_pull_request("/srv/wt", &draft())
            .await
            .expect_err("it exists");
        assert!(error.to_string().contains("already exists"), "{error}");
        let missing = ScriptedRunner::new().fail(RunError::Spawn {
            program: "gh".into(),
            source: std::io::Error::other("no gh"),
        });
        let error = Github::new(&missing, &machine, &ssh)
            .create_pull_request("/srv/wt", &draft())
            .await
            .expect_err("no gh");
        assert!(error.to_string().contains("GitHub CLI"), "{error}");
        let silent = ScriptedRunner::new().reply(Output::ok("created\n"));
        let error = Github::new(&silent, &machine, &ssh)
            .create_pull_request("/srv/wt", &draft())
            .await
            .expect_err("no address");
        assert!(error.to_string().contains("address"), "{error}");
    }

    #[tokio::test]
    async fn the_start_of_a_pull_request_is_the_commits_between_origin_s_base_and_head() {
        let runner = ScriptedRunner::new()
            .reply(Output::ok("## topic...origin/topic\0"))
            .reply(Output::ok("origin/develop\n"))
            .reply(Output::ok("Add it\u{1f}Because.\u{1e}\n"));
        let machine = local();
        let ssh = SshOptions::default();
        let start = Git::new(&runner, &machine, &ssh)
            .pull_request_start("/srv/wt")
            .await
            .unwrap();
        assert_eq!(start.base, "develop");
        assert_eq!(start.branch.as_deref(), Some("topic"));
        assert_eq!(
            (start.title.as_str(), start.body.as_str()),
            ("Add it", "Because.")
        );
        assert_eq!(
            runner.calls()[2].args.last().map(String::as_str),
            Some("origin/develop..HEAD")
        );
    }

    #[tokio::test]
    async fn without_a_remote_base_the_local_one_is_tried_and_without_any_there_are_no_commits() {
        let runner = ScriptedRunner::new()
            .reply(Output::ok("## topic\0"))
            .reply(Output::failed(128, "fatal: not a symbolic ref"))
            .reply(Output::failed(128, "fatal: bad revision"))
            .reply(Output::ok("One\u{1f}\u{1e}"));
        let machine = local();
        let ssh = SshOptions::default();
        let start = Git::new(&runner, &machine, &ssh)
            .pull_request_start("/srv/wt")
            .await
            .unwrap();
        assert_eq!(start.base, "main");
        assert_eq!(start.title, "One");
        assert_eq!(
            runner.calls()[3].args.last().map(String::as_str),
            Some("main..HEAD")
        );
    }

    #[test]
    fn what_an_agent_answers_is_cleaned_into_the_message_it_meant() {
        assert_eq!(clean_suggestion("Add the view\n"), "Add the view");
        assert_eq!(
            clean_suggestion("```\nAdd the view\n\nBecause.\n```\n"),
            "Add the view\n\nBecause."
        );
        assert_eq!(clean_suggestion("\"Add the view\""), "Add the view");
        assert_eq!(
            clean_suggestion("Commit message: Add the view"),
            "Add the view"
        );
        assert_eq!(
            clean_suggestion("Add the view\n\nCo-Authored-By: Some Agent <a@b.c>\n"),
            "Add the view",
            "an attribution line is not the person's"
        );
        assert_eq!(
            clean_suggestion("mise ~/.config/mise/config.toml tools: codex@0.1\nAdd the view\n"),
            "Add the view"
        );
        assert_eq!(clean_suggestion("  \n"), "");
        assert_eq!(
            clean_suggestion("\"quoted\" in the middle"),
            "\"quoted\" in the middle"
        );
    }

    #[test]
    fn a_pull_request_answer_is_a_title_and_the_rest() {
        assert_eq!(
            split_title_and_body("# Fix the tabs\n\nThey leaked.\n- one\n"),
            ("Fix the tabs".to_owned(), "They leaked.\n- one".to_owned())
        );
        assert_eq!(
            split_title_and_body("Title: Only a title"),
            ("Only a title".to_owned(), String::new())
        );
    }

    #[test]
    fn a_long_diff_is_cut_at_a_line_and_says_so() {
        let line = format!("+{}\n", "x".repeat(99));
        let text = line.repeat(MAX_PROMPT_BYTES / 100 + 50);
        let cut = cut_for_prompt(&text);
        assert!(cut.len() < text.len());
        assert!(cut.contains("left out"));
        assert!(cut.starts_with(&line));
        assert_eq!(cut_for_prompt("short\n"), "short\n");
    }

    #[test]
    fn an_agent_is_asked_in_the_checkout_with_the_instruction_last_and_the_diff_on_stdin() {
        let command = suggestion_command(
            "/srv/wt",
            "claude",
            &[
                "-p".to_owned(),
                "--permission-mode".to_owned(),
                "dontAsk".to_owned(),
            ],
            Suggest::Commit,
            "diff --git a b\n",
        );
        assert_eq!(command.program, "claude");
        assert_eq!(&command.args[..3], ["-p", "--permission-mode", "dontAsk"]);
        assert_eq!(command.args[3], suggestion_prompt(Suggest::Commit));
        assert_eq!(command.args.len(), 4);
        assert_eq!(
            command.stdin.as_deref(),
            Some(b"diff --git a b\n".as_slice())
        );
        assert_eq!(command.cwd.as_deref(), Some("/srv/wt"));
    }

    async fn ask(
        runner: &ScriptedRunner,
        machine: &Machine,
        ssh: &SshOptions,
    ) -> Result<String, ShipError> {
        let headless = vec!["exec".to_owned()];
        Git::new(runner, machine, ssh)
            .suggest("/srv/wt", "codex", &headless, Suggest::Commit, "diff")
            .await
    }

    #[tokio::test]
    async fn an_agent_that_answers_is_cleaned_and_one_that_fails_or_is_silent_is_a_plain_message() {
        let machine = local();
        let ssh = SshOptions::default();
        let answered = ScriptedRunner::new().reply(Output::ok("```\nFix the tabs\n```\n"));
        assert_eq!(
            ask(&answered, &machine, &ssh).await.unwrap(),
            "Fix the tabs"
        );
        let failed = ScriptedRunner::new().reply(Output::failed(1, "Please log in"));
        assert_eq!(
            ask(&failed, &machine, &ssh).await.unwrap_err().to_string(),
            "Please log in"
        );
        let silent = ScriptedRunner::new().reply(Output::ok("\n"));
        assert!(ask(&silent, &machine, &ssh)
            .await
            .unwrap_err()
            .to_string()
            .contains("answered nothing"));
    }

    #[tokio::test]
    async fn the_diff_an_agent_reads_is_the_chosen_files_against_head_and_names_the_new_ones() {
        let runner = ScriptedRunner::new().reply(Output::ok("diff --git a/a b/a\n+x\n"));
        let machine = local();
        let ssh = SshOptions::default();
        let files = [
            file("a.txt", FileStatus::Modified),
            file("notes.md", FileStatus::Untracked),
        ];
        let text = Git::new(&runner, &machine, &ssh)
            .diff_for_message("/srv/wt", Some(&files))
            .await
            .unwrap();
        assert!(text.starts_with("diff --git"));
        assert!(text.contains("New files, not shown: notes.md"));
        let args = &runner.calls()[0].args;
        assert_eq!(args.last().map(String::as_str), Some("a.txt"));
        assert!(args.contains(&"HEAD".to_owned()));
    }

    #[test]
    fn claude_is_asked_with_no_tools_and_its_empty_list_survives_the_remote_shell() {
        let claude = leon_core::agent::builtin()
            .iter()
            .find(|spec| spec.id.as_str() == "claude")
            .and_then(|spec| spec.headless.clone())
            .expect("claude has a headless form");
        let command = suggestion_command("/srv/wt", "claude", &claude, Suggest::Commit, "diff");
        let at = command
            .args
            .iter()
            .position(|arg| arg == "--tools")
            .unwrap();
        assert_eq!(command.args[at + 1], "", "no tool is available");
        assert_eq!(command.args[at + 2], "-p", "the list ends at the next flag");
        assert_eq!(
            command.args.last().map(String::as_str),
            Some(suggestion_prompt(Suggest::Commit)),
            "the instruction is still the last word"
        );
        let remote = crate::command::remote_shell_command(&command);
        assert!(remote.contains(" --tools '' -p "), "{remote}");
    }

    #[tokio::test]
    async fn files_git_does_not_track_are_named_and_never_stand_for_the_tracked_diff() {
        // Only new files chosen: a `git diff` without paths would show every
        // tracked change, files that are not in the commit. Git is not asked.
        let runner = ScriptedRunner::new();
        let machine = local();
        let ssh = SshOptions::default();
        let only_new = [file("notes.md", FileStatus::Untracked)];
        let text = Git::new(&runner, &machine, &ssh)
            .diff_for_message("/srv/wt", Some(&only_new))
            .await
            .unwrap();
        assert_eq!(text, "\nNew files, not shown: notes.md\n");
        assert!(runner.calls().is_empty(), "no diff was asked for");
        // Nothing chosen at all: refused before an agent could be asked.
        let refused = Git::new(&runner, &machine, &ssh)
            .diff_for_message("/srv/wt", Some(&[]))
            .await;
        assert!(matches!(refused, Err(ShipError::Invalid(_))));
        assert!(runner.calls().is_empty());
        // Every changed file (`None`) is the whole diff, with no path list.
        let runner = ScriptedRunner::new().reply(Output::ok("diff --git a/a b/a\n+x\n"));
        Git::new(&runner, &machine, &ssh)
            .diff_for_message("/srv/wt", None)
            .await
            .unwrap();
        assert!(!runner.calls()[0].args.contains(&"--".to_owned()));
    }

    /// Every step that touches a checkout, run once against `machine`, and
    /// the processes the runner was asked to start for them.
    async fn every_step_on(machine: &Machine) -> Vec<CommandSpec> {
        let status = "## topic...origin/topic\0";
        let runner = ScriptedRunner::new()
            // commit: add, commit
            .reply(Output::ok(""))
            .reply(Output::ok("[topic 1a2b3c4] Fix"))
            // push: the status, then the push
            .reply(Output::ok(status))
            .reply(Output::ok(""))
            // the start of a pull request: status, base, log
            .reply(Output::ok(status))
            .reply(Output::ok("origin/main\n"))
            .reply(Output::ok("Fix\x1f\x1e"))
            // the diff for a message, then the agent
            .reply(Output::ok("diff --git a/a b/a\n+x\n"))
            .reply(Output::ok("Fix the tabs\n"))
            // gh pr create
            .reply(Output::ok("https://github.com/zavudev/leon/pull/41\n"));
        let ssh = SshOptions::default();
        let git = Git::new(&runner, machine, &ssh);
        let cwd = "/srv/my wt";
        git.commit(cwd, "Fix the tabs", None).await.unwrap();
        git.push(cwd).await.unwrap();
        git.pull_request_start(cwd).await.unwrap();
        let input = git
            .diff_for_message(cwd, Some(&[file("a", FileStatus::Modified)]))
            .await
            .unwrap();
        git.suggest(cwd, "claude", &["-p".to_owned()], Suggest::Commit, &input)
            .await
            .unwrap();
        Github::new(&runner, machine, &ssh)
            .create_pull_request(cwd, &draft())
            .await
            .unwrap();
        runner.calls()
    }

    #[tokio::test]
    async fn over_ssh_every_step_is_one_remote_command_in_the_checkout_with_its_input() {
        let machine = Machine {
            id: MachineId::from_string("m1"),
            name: "build box".into(),
            kind: MachineKind::Ssh {
                host: "build.example".into(),
                user: None,
                port: None,
                identity_file: None,
            },
        };
        let calls = every_step_on(&machine).await;
        assert_eq!(calls.len(), 10);
        let remote = format!("cd {} && exec", crate::quote::sh_quote("/srv/my wt"));
        for call in &calls {
            assert_eq!(call.program, "ssh", "{call:?}");
            assert_eq!(call.cwd, None, "the directory travels in the remote line");
            let line = call.args.last().unwrap();
            assert!(line.starts_with(&remote), "{line}");
            assert!(call.route.is_none());
        }
        // What is piped in still reaches the remote program through `ssh`.
        assert_eq!(
            calls[1].stdin.as_deref(),
            Some(b"Fix the tabs\n".as_slice())
        );
        assert!(calls[8].args.last().unwrap().contains(" claude "));
        assert!(calls[8]
            .stdin
            .as_deref()
            .is_some_and(|input| input.starts_with(b"diff --git")));
        assert_eq!(
            calls[9].stdin.as_deref(),
            Some(b"They leaked.\n".as_slice())
        );
        assert!(calls[9].args.last().unwrap().contains(" gh pr create "));
    }

    #[tokio::test]
    async fn over_the_relay_every_step_carries_the_route_of_the_host() {
        let machine = Machine {
            id: MachineId::from_string("m2"),
            name: "laptop".into(),
            kind: MachineKind::Relay {
                host_id: "host1".into(),
                host_key: "ab12".into(),
                relay_url: "wss://relay.example".into(),
                name: "laptop".into(),
            },
        };
        let calls = every_step_on(&machine).await;
        assert_eq!(calls.len(), 10);
        for call in &calls {
            assert_eq!(
                call.route.as_deref(),
                Some("host1 ab12 wss://relay.example"),
                "{call:?}"
            );
            assert_eq!(call.cwd.as_deref(), Some("/srv/my wt"));
        }
        assert_eq!(calls[0].program, "git");
        assert_eq!(calls[8].program, "claude");
        assert_eq!(calls[9].program, "gh");
    }

    /// Runs a command in a scratch repository, apart from the user's own
    /// configuration where it matters.
    async fn in_repo(runner: &crate::runner::ProcessRunner, cwd: &str, args: &[&str]) -> Output {
        let spec = CommandSpec::new("git")
            .args(args.iter().copied())
            .env("GIT_TERMINAL_PROMPT", "0")
            .cwd(cwd);
        runner.run(&spec).await.unwrap()
    }

    #[tokio::test]
    async fn a_real_repository_lists_diffs_and_commits_only_the_chosen_files() {
        use crate::changes::FileDiff;
        let runner = crate::runner::ProcessRunner::new();
        if runner
            .run(&CommandSpec::new("git").arg("--version"))
            .await
            .map_or(true, |output| !output.success())
        {
            eprintln!("skipped: git is not available");
            return;
        }
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let root = root.to_string_lossy().into_owned();
        for args in [
            &["init", "--quiet"][..],
            &["checkout", "--quiet", "-B", "main"],
            &["config", "user.name", "Leon Test"],
            &["config", "user.email", "leon@example.invalid"],
            &["config", "commit.gpgsign", "false"],
            &["config", "core.hooksPath", "no-hooks"],
        ] {
            assert!(in_repo(&runner, &root, args).await.success(), "{args:?}");
        }
        let at = |name: &str| std::path::Path::new(&root).join(name);
        std::fs::write(at("a.txt"), "one\n").unwrap();
        std::fs::write(at("old.txt"), "same\n").unwrap();
        std::fs::write(at("other.txt"), "x\n").unwrap();
        assert!(in_repo(&runner, &root, &["add", "-A"]).await.success());
        assert!(
            in_repo(&runner, &root, &["commit", "--quiet", "-m", "initial"])
                .await
                .success()
        );

        std::fs::write(at("a.txt"), "one\ntwo\n").unwrap();
        assert!(in_repo(&runner, &root, &["mv", "old.txt", "new name.txt"])
            .await
            .success());
        std::fs::write(at("other.txt"), "y\n").unwrap();
        assert!(in_repo(&runner, &root, &["add", "other.txt"])
            .await
            .success());
        std::fs::write(at("fresh.txt"), "hello\n").unwrap();

        let machine = local();
        let ssh = SshOptions::without_multiplexing();
        let git = Git::new(&runner, &machine, &ssh);
        let changes = git.changes(&root).await.unwrap();
        assert_eq!(changes.branch.as_deref(), Some("main"));
        assert!(!changes.has_upstream);
        let seen: Vec<(&str, FileStatus)> = changes
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.status))
            .collect();
        assert_eq!(
            seen,
            [
                ("a.txt", FileStatus::Modified),
                ("new name.txt", FileStatus::Renamed),
                ("other.txt", FileStatus::Modified),
                ("fresh.txt", FileStatus::Untracked),
            ],
            "git lists what it tracks first, then what it does not"
        );
        let named = |name: &str| {
            changes
                .files
                .iter()
                .find(|file| file.path == name)
                .expect("listed")
                .clone()
        };

        let FileDiff::Lines(lines) = git.file_diff(&root, &named("a.txt")).await.unwrap() else {
            panic!("a modified text file has a diff");
        };
        assert!(lines
            .iter()
            .any(|l| l.kind == crate::LineKind::Added && l.text == "two"));
        let FileDiff::Lines(lines) = git.file_diff(&root, &named("fresh.txt")).await.unwrap()
        else {
            panic!("a new file has a diff");
        };
        assert!(lines
            .iter()
            .any(|l| l.kind == crate::LineKind::Added && l.text == "hello"));
        let FileDiff::Lines(lines) = git.file_diff(&root, &named("new name.txt")).await.unwrap()
        else {
            panic!("a rename has facts");
        };
        assert!(lines.iter().any(|l| l.text == "rename from old.txt"));

        let chosen = [named("a.txt"), named("new name.txt")];
        let said = git
            .commit(&root, "Edit and rename", Some(&chosen))
            .await
            .unwrap();
        assert!(said.contains("Edit and rename"), "{said}");
        let committed = in_repo(
            &runner,
            &root,
            &["show", "--name-status", "--format=%s", "HEAD"],
        )
        .await;
        assert!(
            committed.stdout.starts_with("Edit and rename"),
            "{}",
            committed.stdout
        );
        assert!(committed.stdout.contains("a.txt"));
        assert!(committed.stdout.contains("new name.txt"));
        assert!(
            !committed.stdout.contains("other.txt"),
            "a file staged outside the chosen ones stays out: {}",
            committed.stdout
        );
        let left = git.changes(&root).await.unwrap();
        let left: Vec<&str> = left.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(left, ["other.txt", "fresh.txt"]);

        // Nothing is chosen from what is left but an untracked file: it is
        // added and committed by itself.
        let rest = git.changes(&root).await.unwrap();
        git.commit(&root, "Everything else", None).await.unwrap();
        assert!(git.changes(&root).await.unwrap().files.is_empty());
        assert_eq!(rest.files.len(), 2);

        // Nothing changed now: git's own words come back.
        let error = git
            .commit(&root, "Again", None)
            .await
            .expect_err("nothing to commit");
        assert!(error.to_string().contains("nothing to commit"), "{error}");
        // No upstream: the push sets one on `origin`, which is not there.
        let error = git.push(&root).await.expect_err("there is no origin");
        assert!(
            matches!(
                error,
                ShipError::Failed {
                    step: Step::Push,
                    ..
                }
            ),
            "{error:?}"
        );
    }
}

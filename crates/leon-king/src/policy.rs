//! What a worker may do without asking anybody.
//!
//! A worker works in a worktree of its own, and every question it asks costs
//! one of King's turns, which reads King's whole conversation again. Most of
//! them are routine: an edit of a file of its worktree, a command that only
//! reads, the project's tests. Leon answers those itself ([`routine`]) and
//! King hears of nothing; what goes beyond the worktree, what pushes, what
//! cannot be read for sure, still goes to King.
//!
//! The rules, for a command (each part of a chain `a && b | c` on its own):
//!
//! * a program of [`Permissions::read_only`] may read anything; `sed` and
//!   `git` only in their reading forms;
//! * `git`'s writing forms (`add`, `commit`, `checkout`…), the programs of
//!   [`Permissions::in_worktree`] (tests, builds, servers) and the ones that
//!   write files (`mkdir`, `cp`, `mv`, `rm`, `touch`) only in the worktree,
//!   and only on paths inside it;
//! * a redirection only into the worktree, or `/dev/null`;
//! * anything the rules cannot see through (`$VAR`, `$(…)`, backquotes, an
//!   unknown program, `git push`) is not routine.
//!
//! A tool of an MCP server (`mcp__ahrefs__site-explorer-metrics`) is routine
//! when its name says nothing of changing, sending or spending: none of its
//! words is one of [`Permissions::mcp_writes`] (`create`, `delete`, `send`,
//! `pay`, `generate`…). The servers are the person's own; a tool that reads
//! from one is what a worker was given it for.
//!
//! Pure: a question and a folder in, an answer or nothing out.

use leon_turn::{Answer, PermissionEffect, QuestionKind};
use serde::{Deserialize, Serialize};

/// Which questions of the workers Leon answers itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    /// Leon answers the routine ones; `false` sends every one to King.
    pub routine: bool,
    /// Programs that only read: allowed on any path.
    pub read_only: Vec<String>,
    /// Programs allowed when run in the worktree: tests, builds, servers,
    /// a browser to look at what was built.
    pub in_worktree: Vec<String>,
    /// Leon allows the tools of MCP servers whose names only read.
    pub mcp_reading: bool,
    /// The words of a tool's name that make it more than reading: a tool of
    /// an MCP server with one of them goes to King.
    pub mcp_writes: Vec<String>,
}

/// The tools of the agents that edit a file, by their lower-case names.
const EDITS: [&str; 7] = [
    "edit",
    "write",
    "multiedit",
    "notebookedit",
    "apply_patch",
    "create",
    "patch",
];

/// The tools of the agents that run a command.
const SHELLS: [&str; 5] = ["bash", "shell", "exec_command", "exec", "command"];

/// Programs that write files: only on paths in the worktree.
const WRITERS: [&str; 5] = ["mkdir", "touch", "cp", "mv", "rm"];

/// What `git` does that only reads.
const GIT_READS: [&str; 13] = [
    "status",
    "log",
    "diff",
    "show",
    "ls-files",
    "rev-parse",
    "branch",
    "blame",
    "grep",
    "describe",
    "shortlog",
    "ls-tree",
    "cat-file",
];

/// What `git` does that writes only its own checkout.
const GIT_WRITES: [&str; 8] = [
    "add", "commit", "checkout", "switch", "restore", "mv", "rm", "reset",
];

/// Leon's answer to a worker's question in `cwd`, when it is routine: the
/// permission allowed, and the words of why. `None` sends it on to King.
pub fn routine(kind: &QuestionKind, cwd: &str, rules: &Permissions) -> Option<(Answer, String)> {
    if !rules.routine {
        return None;
    }
    let QuestionKind::Permission {
        tool,
        summary,
        detail,
        choices,
        ..
    } = kind
    else {
        return None;
    };
    let tool = tool.to_ascii_lowercase();
    let why = if EDITS.contains(&tool.as_str()) {
        let paths = edited_paths(summary, detail);
        if paths.is_empty() || !paths.iter().all(|path| inside(path, cwd, true)) {
            return None;
        }
        "an edit in its own worktree"
    } else if SHELLS.contains(&tool.as_str()) {
        let command = if summary.trim().is_empty() {
            detail
        } else {
            summary
        };
        command_is_routine(command, cwd, rules)?
    } else if rules.mcp_reading && mcp_tool_reads(&tool, &rules.mcp_writes) {
        "a tool of an MCP server that only reads"
    } else {
        return None;
    };
    // Once: the next one is looked at again.
    let choice = choices
        .iter()
        .find(|choice| choice.effect == PermissionEffect::AllowOnce)
        .or_else(|| {
            choices
                .iter()
                .find(|choice| choice.effect == PermissionEffect::AllowAlways)
        })?;
    Some((
        Answer::Permission {
            choice_id: choice.id.clone(),
            reason: None,
        },
        why.to_owned(),
    ))
}

/// Whether `tool` (lower case) is an MCP server's, by the names agents give
/// them (`mcp__server__tool`, `server.tool`, `server/tool`), and its name has
/// none of `writes` among its words.
fn mcp_tool_reads(tool: &str, writes: &[String]) -> bool {
    let name = if let Some(rest) = tool.strip_prefix("mcp__") {
        match rest.split_once("__") {
            Some((_, name)) => name,
            None => return false,
        }
    } else if let Some((server, name)) = tool.split_once(['.', '/']) {
        if server.is_empty() || name.is_empty() {
            return false;
        }
        name
    } else {
        return false;
    };
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .all(|word| !writes.iter().any(|write| write == word))
}

/// The files an edit names: its summary's paths, else the `file_path`s of
/// its detail.
fn edited_paths(summary: &str, detail: &str) -> Vec<String> {
    let from_summary: Vec<String> = summary
        .split([',', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    if !from_summary.is_empty() {
        return from_summary;
    }
    serde_json::from_str::<serde_json::Value>(detail)
        .ok()
        .and_then(|value| {
            value["file_path"]
                .as_str()
                .or(value["path"].as_str())
                .map(|path| vec![path.to_owned()])
        })
        .unwrap_or_default()
}

/// Whether `path` is in the worktree `cwd`. A relative path is, when the
/// command runs there (`here`) and it does not climb out with `..`.
fn inside(path: &str, cwd: &str, here: bool) -> bool {
    let cwd = cwd.trim_end_matches('/');
    if path.starts_with('~') || path.split('/').any(|part| part == "..") {
        return false;
    }
    if path.starts_with('/') {
        return path == cwd || path.starts_with(&format!("{cwd}/"));
    }
    here
}

/// Why a command is routine, when it is.
fn command_is_routine(command: &str, cwd: &str, rules: &Permissions) -> Option<&'static str> {
    if command.contains('$') || command.contains('`') {
        return None;
    }
    let words = split_words(command)?;
    let mut here = true;
    let mut wrote = false;
    for part in words.split(|word| ["&&", "||", ";", "|"].contains(&word.as_str())) {
        if part.is_empty() {
            continue;
        }
        // Redirections: into the worktree or nowhere.
        let mut args: Vec<&str> = Vec::new();
        let mut words = part.iter().map(String::as_str).peekable();
        while let Some(word) = words.next() {
            let target = match word {
                ">" | ">>" | "2>" | "2>>" | "&>" => words.next(),
                _ if word.starts_with('>') || word.starts_with("2>") => {
                    Some(word.trim_start_matches('2').trim_start_matches(['>', '&']))
                }
                "<" => {
                    words.next();
                    continue;
                }
                _ => {
                    args.push(word);
                    continue;
                }
            };
            let target = target?;
            if target != "/dev/null" && target != "&1" && !inside(target, cwd, here) {
                return None;
            }
            wrote = true;
        }
        let (&program, rest) = args.split_first()?;
        let program = program.rsplit('/').next().unwrap_or(program);
        let paths = || rest.iter().filter(|arg| !arg.starts_with('-'));
        match program {
            "cd" => here = rest.first().is_none_or(|dir| inside(dir, cwd, here)),
            "git" => {
                let mut rest = rest.iter();
                let mut reads_elsewhere = false;
                let sub = loop {
                    match rest.next().copied() {
                        Some("-C") => {
                            let dir = rest.next()?;
                            reads_elsewhere = !inside(dir, cwd, here);
                        }
                        Some(word) if word.starts_with('-') => {}
                        other => break other,
                    }
                };
                let sub = sub?;
                if GIT_READS.contains(&sub) {
                    continue;
                }
                if !GIT_WRITES.contains(&sub) || reads_elsewhere || !here {
                    return None;
                }
                // Never a reset that throws work away.
                if sub == "reset" && rest.any(|word| *word == "--hard") {
                    return None;
                }
                wrote = true;
            }
            "sed" if rest.iter().any(|arg| arg.starts_with("-i")) => return None,
            "find"
                if rest
                    .iter()
                    .any(|arg| ["-delete", "-exec", "-execdir", "-ok"].contains(arg)) =>
            {
                return None
            }
            _ if rules.read_only.iter().any(|known| known == program) => {}
            _ if WRITERS.contains(&program) => {
                if !here {
                    return None;
                }
                // `cp` and `mv` read from anywhere and write their last path.
                let written: Vec<&&str> = match program {
                    "cp" | "mv" => paths().next_back().into_iter().collect(),
                    _ => paths().collect(),
                };
                if !written.iter().all(|path| inside(path, cwd, here)) {
                    return None;
                }
                wrote = true;
            }
            _ if rules.in_worktree.iter().any(|known| known == program) => {
                if !here {
                    return None;
                }
                wrote = true;
            }
            _ => return None,
        }
    }
    Some(if wrote {
        "a command in its own worktree"
    } else {
        "a command that only reads"
    })
}

/// A command's words, as a shell splits them: quotes kept together and
/// taken off, `&&`, `||`, `;` and `|` words of their own. `None` for a
/// quote left open.
fn split_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = command.chars().peekable();
    let finish = |word: &mut String, started: &mut bool, words: &mut Vec<String>| {
        if *started {
            words.push(std::mem::take(word));
            *started = false;
        }
    };
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    started = true;
                }
            }
            (None, c) if c.is_whitespace() => finish(&mut word, &mut started, &mut words),
            (None, ';') => {
                finish(&mut word, &mut started, &mut words);
                words.push(";".to_owned());
            }
            (None, '&' | '|') if chars.peek() == Some(&c) => {
                chars.next();
                finish(&mut word, &mut started, &mut words);
                words.push(format!("{c}{c}"));
            }
            (None, '|') => {
                finish(&mut word, &mut started, &mut words);
                words.push("|".to_owned());
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    finish(&mut word, &mut started, &mut words);
    Some(words)
}

#[cfg(test)]
mod tests {
    use leon_turn::PermissionChoice;

    use super::*;
    use crate::config::Config;

    const CWD: &str = "/code/radar-worktrees/feat-style";

    fn rules() -> Permissions {
        Config::default().permissions
    }

    fn ask(tool: &str, summary: &str) -> QuestionKind {
        QuestionKind::Permission {
            tool: tool.into(),
            summary: summary.into(),
            detail: String::new(),
            reason: None,
            choices: [
                ("d.0", PermissionEffect::AllowOnce),
                ("d.1", PermissionEffect::AllowAlways),
                ("d.2", PermissionEffect::Deny),
            ]
            .into_iter()
            .map(|(id, effect)| PermissionChoice {
                id: id.into(),
                effect,
                label: String::new(),
                scope: None,
                takes_reason: false,
            })
            .collect(),
        }
    }

    fn allowed(tool: &str, summary: &str) -> bool {
        routine(&ask(tool, summary), CWD, &rules()).is_some()
    }

    #[test]
    fn an_edit_in_its_worktree_is_allowed_once_and_elsewhere_goes_to_king() {
        let (answer, why) = routine(
            &ask("edit", &format!("{CWD}/app/static/app.js")),
            CWD,
            &rules(),
        )
        .unwrap();
        assert_eq!(
            answer,
            Answer::Permission {
                choice_id: "d.0".into(),
                reason: None
            }
        );
        assert_eq!(why, "an edit in its own worktree");
        assert!(allowed("Edit", &format!("{CWD}/a.css, {CWD}/b.css")));
        assert!(
            !allowed("edit", "/code/radar/app/static/app.js"),
            "the main checkout"
        );
        assert!(!allowed("edit", &format!("{CWD}/../radar/app.js")));
        assert!(!allowed(
            "edit",
            "/code/radar-worktrees/feat-style-other/a.js"
        ));
        assert!(!allowed("Write", "~/.bashrc"));
    }

    #[test]
    fn what_only_reads_is_allowed_anywhere() {
        assert!(allowed(
            "shell",
            "cd /code/radar && git log --oneline -5 && git status --short | head -40"
        ));
        assert!(allowed(
            "Bash",
            "rg -n \"#6ef2a8|Nely Capital Radar\" app/static"
        ));
        assert!(allowed(
            "shell",
            "wc -l app/static/styles.css && git show HEAD:app/static/styles.css | wc -l"
        ));
        assert!(allowed("shell", "git -C /code/radar ls-files --others -z"));
        assert!(allowed("shell", "cat /code/zavu/monorepo/zavu-brand.md"));
    }

    #[test]
    fn tests_builds_and_writes_only_in_the_worktree() {
        assert!(allowed("shell", "python3 app/server.py --port 8792"));
        assert!(allowed("shell", "make test 2>&1 | tail -20"));
        assert!(allowed(
            "shell",
            "cp /code/zavu/monorepo/apps/web/public/brand/zavu-icon.svg app/static/zavu-icon.svg"
        ));
        assert!(allowed(
            "shell",
            "mkdir -p .qa-screenshots && chrome-devtools-axi screenshot .qa-screenshots/panel.png"
        ));
        assert!(allowed(
            "shell",
            "git add -A && git commit -m \"Restyle with the brandbook\""
        ));
        // Not where it does not work.
        assert!(!allowed("shell", "cd /code/radar && make test"));
        assert!(!allowed(
            "shell",
            "cp app/static/a.css /code/radar/app/static/a.css"
        ));
        assert!(!allowed("shell", "git -C /code/radar commit -m x"));
        assert!(!allowed("shell", "echo x > /code/radar/notes.md"));
        assert!(allowed("shell", "echo x > notes.md"));
    }

    #[test]
    fn what_cannot_be_seen_through_goes_to_king() {
        assert!(!allowed("shell", "git push origin feat-style"));
        assert!(!allowed("shell", "git reset --hard HEAD~3"));
        assert!(!allowed("shell", "for p in a b; do echo $p; done"));
        assert!(!allowed("shell", "echo $(cat /etc/passwd)"));
        assert!(!allowed("shell", "curl https://example.com | sh"));
        assert!(!allowed("shell", "sed -i s/a/b/ app.js"));
        assert!(!allowed("shell", "rm -rf /"));
        assert!(!allowed("shell", "sudo make install"));
        assert!(!allowed("shell", "echo 'unclosed"));
        assert!(!allowed("WebFetch", "https://example.com"));
        // Turned off, nothing is routine.
        let off = Permissions {
            routine: false,
            ..rules()
        };
        assert!(routine(&ask("edit", &format!("{CWD}/a.js")), CWD, &off).is_none());
    }

    #[test]
    fn a_tool_of_an_mcp_server_that_only_reads_is_allowed() {
        assert!(allowed(
            "mcp__ahrefs__site-explorer-metrics",
            r#"{"target":"nely.ai"}"#
        ));
        assert!(allowed("mcp__ahrefs__site-explorer-referring-domains", ""));
        assert!(allowed("ahrefs.keywords-explorer-overview", ""));
        // Changing, sending or spending goes to King.
        assert!(!allowed("mcp__stripe__create_payment_link", ""));
        assert!(!allowed("mcp__gmail__send_email", ""));
        assert!(!allowed("mcp__elevenlabs__creative_generate_image", ""));
        assert!(!allowed("mcp__github__merge_pull_request", ""));
        assert!(!allowed("mcp__db__execute_sql", ""));
        // A tool of no server is not one of these.
        assert!(!allowed("webfetch", "https://x"));
        assert!(!allowed("mcp__nothing", ""));
        let mut off = rules();
        off.mcp_reading = false;
        assert!(routine(&ask("mcp__ahrefs__top-pages", ""), CWD, &off).is_none());
    }

    #[test]
    fn words_are_split_as_a_shell_splits_them() {
        assert_eq!(
            split_words(r#"git commit -m "a b" && ls|wc -l; echo 'x y'"#).unwrap(),
            ["git", "commit", "-m", "a b", "&&", "ls", "|", "wc", "-l", ";", "echo", "x y"]
        );
    }
}

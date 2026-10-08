//! The vocabulary of a live transcript: what kind of tool a call is, and the
//! events ("beats") a transcript yields while its agent works.
//!
//! Both agents Leon tails name their tools freely, so a tool is first reduced
//! to a [`ToolKind`] (what the agent is doing in the world) and to a short
//! `detail` (on what). The detail is meant for a caption under a small
//! figure: a file's name rather than its path, the first words of a command,
//! the host of an address. It is never the tool's output and never a whole
//! prompt.

use serde_json::Value;

use crate::normalize::{tool_detail, truncate_chars};

/// The longest detail kept, in characters.
pub const MAX_DETAIL_CHARS: usize = 48;

/// What a tool call does, whichever agent names the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
    /// Changes a file: `Edit`, `Write`, `MultiEdit`, `NotebookEdit`,
    /// `apply_patch`.
    Edit,
    /// Reads one file: `Read`, `view`, `view_image`.
    Read,
    /// Looks for something among files or tools: `Grep`, `Glob`, `LS`,
    /// `ToolSearch`.
    Search,
    /// Runs a command or talks to one already running: `Bash`, `shell`,
    /// `exec_command`, `BashOutput`, `write_stdin`.
    Run,
    /// Goes to the network: `WebFetch`, `WebSearch`.
    Web,
    /// Starts a sub-agent: `Task`, `Agent`.
    Delegate,
    /// Keeps the plan or the task list: `TodoWrite`, `ExitPlanMode`,
    /// `update_plan`.
    Plan,
    /// Asks the user a question and waits for the answer: `AskUserQuestion`.
    Ask,
    /// Anything else, including every `mcp__*` tool.
    Other,
}

impl ToolKind {
    /// The kind of the tool called `name`. Names are matched without regard
    /// to case; an unknown name is [`ToolKind::Other`].
    pub fn of(name: &str) -> Self {
        let name = name.trim().to_ascii_lowercase();
        if name.starts_with("mcp__") {
            return Self::Other;
        }
        match name.as_str() {
            "edit"
            | "write"
            | "multiedit"
            | "notebookedit"
            | "apply_patch"
            | "patch"
            | "str_replace_editor"
            | "str_replace_based_edit_tool"
            | "create_file"
            | "write_file"
            | "edit_file"
            | "replace" => Self::Edit,
            "read" | "view" | "notebookread" | "read_file" | "view_image" | "cat" => Self::Read,
            "grep" | "glob" | "ls" | "toolsearch" | "tool_search" | "find" | "search" | "list"
            | "list_dir" | "file_search" | "codebase_search" => Self::Search,
            "bash" | "shell" | "bashoutput" | "killshell" | "killbash" | "taskoutput" | "exec"
            | "exec_command" | "shell_command" | "local_shell" | "container.exec"
            | "write_stdin" | "monitor" => Self::Run,
            "webfetch" | "websearch" | "web_fetch" | "web_search" | "web__run" | "fetch" => {
                Self::Web
            }
            "task" | "agent" | "spawn_agent" => Self::Delegate,
            "todowrite" | "todoread" | "exitplanmode" | "enterplanmode" | "update_plan"
            | "taskcreate" | "taskupdate" | "tasklist" | "taskget" => Self::Plan,
            "askuserquestion" | "request_user_input" | "request_user_input_async" => Self::Ask,
            _ => Self::Other,
        }
    }
}

/// How a background task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskOutcome {
    /// It finished its work.
    Completed,
    /// It ended with an error.
    Failed,
    /// It was stopped or killed before it finished.
    Stopped,
}

impl TaskOutcome {
    /// Reads the status word of a task notification. An unknown word counts
    /// as a failure: the task is over and it did not say it completed.
    pub(crate) fn of(status: &str) -> Self {
        match status.trim() {
            "completed" => Self::Completed,
            "stopped" | "killed" | "cancelled" | "canceled" => Self::Stopped,
            _ => Self::Failed,
        }
    }
}

/// One event of a live transcript.
///
/// Beats are facts read from lines the agent wrote; they carry no clock. See
/// the module documentation of [`crate::live`] for the line each one comes
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Beat {
    /// The user sent a message.
    Prompt,
    /// Something other than the user started the agent again: a background
    /// task reported back or another agent sent a message.
    Woken,
    /// The model wrote a block of reasoning.
    Thinking,
    /// The model wrote text for the user.
    Said {
        /// What it wrote, as it wrote it: only control characters other
        /// than the line break and the tab are taken out, and a text longer
        /// than [`MAX_SPEECH_CHARS`] is cut there (see [`speech`]).
        text: String,
        /// When, in seconds since the Unix epoch, if the transcript says.
        at: Option<i64>,
    },
    /// The model called a tool. The call is written before the tool runs,
    /// and before any permission question about it is answered.
    ToolStarted {
        /// The call's id, unique within the transcript.
        id: String,
        /// The tool's name as the agent wrote it.
        name: String,
        /// What the tool does.
        kind: ToolKind,
        /// The short subject of the call (see [`detail`]); may be empty.
        detail: String,
    },
    /// The result of a tool call arrived.
    ToolFinished {
        /// The id of the call.
        id: String,
        /// The result is an error.
        failed: bool,
        /// The call never ran because the user or a rule refused it. Read
        /// from the wording of the result, so best effort; implies `failed`.
        refused: bool,
    },
    /// A tool call returned at once but its work goes on in the background:
    /// a sub-agent was launched. It ends with a [`Beat::TaskEnded`] naming
    /// the same task.
    Detached {
        /// The id of the call that launched it.
        id: String,
        /// The id of the background task; for a Claude Code sub-agent, the
        /// agent id its transcript file is named after.
        task: String,
    },
    /// A background task ended.
    TaskEnded {
        /// The id of the task, as in [`Beat::Detached`].
        task: String,
        /// The id of the tool call that launched it, when the notice says.
        tool: Option<String>,
        /// How it ended.
        outcome: TaskOutcome,
    },
    /// Every background sub-agent was stopped at once.
    TasksKilled,
    /// The agent finished its turn and waits for the user.
    TurnEnded,
    /// The user interrupted the turn.
    Interrupted,
    /// The conversation was compacted to free context.
    Compacted {
        /// Tokens of context before, when the transcript says.
        before: Option<u64>,
        /// Tokens of context after, when the transcript says.
        after: Option<u64>,
    },
    /// Token numbers of the latest model call.
    Usage {
        /// Tokens the model read for that call: the size of the context.
        context: u64,
        /// Tokens the model wrote in that call.
        output: u64,
        /// The size of the model's context window, when the transcript says.
        window: Option<u64>,
    },
}

/// The most characters of a message kept in a [`Beat::Said`]: enough for
/// any answer a person reads in one go, and a bound on what a follower of
/// many sessions holds.
pub const MAX_SPEECH_CHARS: usize = 4000;

/// A message of the model as a [`Beat::Said`] carries it: the text as it is,
/// without the control characters a terminal would act on (the line break
/// and the tab stay; a carriage return goes), trimmed, and cut at
/// [`MAX_SPEECH_CHARS`] with an ellipsis.
pub fn speech(text: &str) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let clean = clean.trim();
    if clean.chars().count() <= MAX_SPEECH_CHARS {
        return clean.to_owned();
    }
    let mut cut: String = clean.chars().take(MAX_SPEECH_CHARS - 1).collect();
    cut.push('\u{2026}');
    cut
}

/// A time of a transcript (RFC 3339) in seconds since the Unix epoch.
pub(crate) fn seconds(timestamp: Option<&str>) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp?)
        .ok()
        .map(|time| time.timestamp())
}

impl Beat {
    /// A [`Beat::Said`] with this text and no time.
    pub fn said(text: &str) -> Self {
        Self::Said {
            text: speech(text),
            at: None,
        }
    }

    /// Whether this beat means the agent is at work again after a turn
    /// ended. Results, notices and numbers do not: they can trail a turn.
    pub fn opens_turn(&self) -> bool {
        matches!(
            self,
            Self::Prompt
                | Self::Woken
                | Self::Thinking
                | Self::Said { .. }
                | Self::ToolStarted { .. }
        )
    }
}

/// The short subject of a tool call: what a caption shows next to the tool.
///
/// * [`ToolKind::Edit`] and [`ToolKind::Read`]: the file's name, without its
///   folders. For a patch given as text, the first file it touches.
/// * [`ToolKind::Search`]: the pattern or query, else the folder searched.
/// * [`ToolKind::Run`]: the first words of the command.
/// * [`ToolKind::Web`]: the host of the address, else the query.
/// * [`ToolKind::Delegate`]: the sub-agent's type and the task's short
///   description.
/// * Everything else: the most telling field of the input, as in the
///   history's tool lines.
///
/// The result is one line of at most [`MAX_DETAIL_CHARS`] characters and is
/// empty when the input offers nothing.
pub fn detail(kind: ToolKind, input: Option<&Value>) -> String {
    let field = |keys: &[&str]| -> Option<String> {
        let fields = input?.as_object()?;
        keys.iter()
            .find_map(|key| fields.get(*key).and_then(one_line))
    };
    let found = match kind {
        ToolKind::Edit | ToolKind::Read => {
            field(&["file_path", "filePath", "notebook_path", "path", "file"])
                .map(|path| base_name(&path).to_owned())
                .or_else(|| {
                    input
                        .and_then(Value::as_str)
                        .and_then(patched_file)
                        .map(|path| base_name(path).to_owned())
                })
        }
        ToolKind::Search => field(&["pattern", "query", "glob"])
            .or_else(|| field(&["path"]).map(|path| base_name(&path).to_owned())),
        ToolKind::Run => field(&["command", "cmd", "chars", "bash_id", "shell_id"]),
        ToolKind::Web => field(&["url"])
            .map(|url| host_of(&url).to_owned())
            .or_else(|| field(&["query", "q"])),
        ToolKind::Delegate => {
            let agent = field(&["subagent_type", "agent_type"]);
            let task = field(&["description", "name"]);
            match (agent, task) {
                (Some(agent), Some(task)) => Some(format!("{agent}: {task}")),
                (agent, task) => agent.or(task),
            }
        }
        ToolKind::Plan | ToolKind::Ask | ToolKind::Other => field(&["skill", "header"]),
    };
    let found = found.or_else(|| tool_detail(input)).unwrap_or_default();
    truncate_chars(&found, MAX_DETAIL_CHARS)
}

/// A JSON value as one line with single spaces: a string's first non-blank
/// line, or the strings of a list joined.
fn one_line(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(line.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The last component of a path written with either separator.
pub(crate) fn base_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

/// The host of a web address: no scheme, user, port or path, and no `www.`.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host.split(':').next().unwrap_or(host);
    host.strip_prefix("www.").unwrap_or(host)
}

/// The first file a patch in the `*** Begin Patch` format touches. The text
/// may be raw or still escaped inside a script, so a file name ends at a
/// real line break, an escaped one or a quote.
pub(crate) fn patched_file(patch: &str) -> Option<&str> {
    const MARKERS: [&str; 3] = ["*** Update File: ", "*** Add File: ", "*** Delete File: "];
    let start = MARKERS
        .iter()
        .filter_map(|marker| patch.find(marker).map(|at| at + marker.len()))
        .min()?;
    let rest = &patch[start..];
    let end = rest
        .find(['\n', '\r', '"', '\'', '`'])
        .into_iter()
        .chain(rest.find("\\n"))
        .min()
        .unwrap_or(rest.len());
    let file = rest[..end].trim();
    (!file.is_empty()).then_some(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_names_of_both_agents_fall_into_their_kinds() {
        for name in ["Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch"] {
            assert_eq!(ToolKind::of(name), ToolKind::Edit, "{name}");
        }
        for name in ["Read", "view", "view_image"] {
            assert_eq!(ToolKind::of(name), ToolKind::Read, "{name}");
        }
        for name in ["Grep", "Glob", "LS", "ToolSearch", "find"] {
            assert_eq!(ToolKind::of(name), ToolKind::Search, "{name}");
        }
        for name in ["Bash", "bash", "shell", "BashOutput", "exec_command"] {
            assert_eq!(ToolKind::of(name), ToolKind::Run, "{name}");
        }
        for name in ["WebFetch", "WebSearch", "web_search"] {
            assert_eq!(ToolKind::of(name), ToolKind::Web, "{name}");
        }
        for name in ["Task", "Agent"] {
            assert_eq!(ToolKind::of(name), ToolKind::Delegate, "{name}");
        }
        for name in ["TodoWrite", "ExitPlanMode", "update_plan"] {
            assert_eq!(ToolKind::of(name), ToolKind::Plan, "{name}");
        }
        assert_eq!(ToolKind::of("AskUserQuestion"), ToolKind::Ask);
    }

    #[test]
    fn unknown_and_mcp_tools_are_other() {
        assert_eq!(ToolKind::of("mcp__files__read"), ToolKind::Other);
        assert_eq!(ToolKind::of("mcp__shell__bash"), ToolKind::Other);
        assert_eq!(ToolKind::of("SomethingNew"), ToolKind::Other);
        assert_eq!(ToolKind::of(""), ToolKind::Other);
    }

    #[test]
    fn a_file_tool_shows_the_file_name_without_its_folders() {
        let input = json!({"file_path": "/srv/api/src/main.rs", "old_string": "a"});
        assert_eq!(detail(ToolKind::Edit, Some(&input)), "main.rs");
        let windows = json!({"file_path": "C:\\work\\api\\lib.rs"});
        assert_eq!(detail(ToolKind::Read, Some(&windows)), "lib.rs");
        let notebook = json!({"notebook_path": "/srv/notes/plot.ipynb"});
        assert_eq!(detail(ToolKind::Edit, Some(&notebook)), "plot.ipynb");
    }

    #[test]
    fn a_patch_given_as_text_shows_the_first_file_it_touches() {
        let raw = json!("*** Begin Patch\n*** Update File: src/login/form.rs\n@@\n-a\n+b");
        assert_eq!(detail(ToolKind::Edit, Some(&raw)), "form.rs");
        assert_eq!(
            patched_file("const p = \"*** Begin Patch\\n*** Add File: docs/new.md\\n+hi\";"),
            Some("docs/new.md")
        );
        assert_eq!(patched_file("no patch here"), None);
    }

    #[test]
    fn a_search_shows_its_pattern_and_falls_back_to_the_folder() {
        let grep = json!({"pattern": "fn main", "path": "/srv/api"});
        assert_eq!(detail(ToolKind::Search, Some(&grep)), "fn main");
        let list = json!({"path": "/srv/api/src/"});
        assert_eq!(detail(ToolKind::Search, Some(&list)), "src");
    }

    #[test]
    fn a_command_shows_its_first_words_on_one_line() {
        let input = json!({"command": "cargo   test -p api\necho done", "description": "x"});
        assert_eq!(detail(ToolKind::Run, Some(&input)), "cargo test -p api");
        let list = json!({"command": ["git", "status"]});
        assert_eq!(detail(ToolKind::Run, Some(&list)), "git status");

        let long = json!({"command": format!("cargo test {}", "--flag ".repeat(40))});
        let cut = detail(ToolKind::Run, Some(&long));
        assert_eq!(cut.chars().count(), MAX_DETAIL_CHARS);
        assert!(cut.starts_with("cargo test --flag") && cut.ends_with('…'));
    }

    #[test]
    fn a_web_call_shows_the_host_or_the_query() {
        let fetch = json!({"url": "https://user@www.example.org:8443/a/b?c=d", "prompt": "x"});
        assert_eq!(detail(ToolKind::Web, Some(&fetch)), "example.org");
        let bare = json!({"url": "example.org/path"});
        assert_eq!(detail(ToolKind::Web, Some(&bare)), "example.org");
        let search = json!({"query": "rust incremental parsing"});
        assert_eq!(
            detail(ToolKind::Web, Some(&search)),
            "rust incremental parsing"
        );
    }

    #[test]
    fn a_delegation_shows_the_sub_agent_type_and_its_task() {
        let both = json!({"subagent_type": "Explore", "description": "Map the parser",
            "prompt": "a very long private prompt"});
        assert_eq!(
            detail(ToolKind::Delegate, Some(&both)),
            "Explore: Map the parser"
        );
        let only_task = json!({"description": "Map the parser", "prompt": "long"});
        assert_eq!(
            detail(ToolKind::Delegate, Some(&only_task)),
            "Map the parser"
        );
    }

    #[test]
    fn other_tools_fall_back_to_their_most_telling_field_or_nothing() {
        let skill = json!({"skill": "deploy", "args": "now"});
        assert_eq!(detail(ToolKind::Other, Some(&skill)), "deploy");
        let generic = json!({"query": "open issues"});
        assert_eq!(detail(ToolKind::Other, Some(&generic)), "open issues");
        assert_eq!(detail(ToolKind::Plan, Some(&json!({"todos": []}))), "");
        assert_eq!(detail(ToolKind::Run, None), "");
        assert_eq!(detail(ToolKind::Edit, Some(&json!(42))), "");
    }

    #[test]
    fn task_statuses_map_to_outcomes() {
        assert_eq!(TaskOutcome::of("completed"), TaskOutcome::Completed);
        assert_eq!(TaskOutcome::of("failed"), TaskOutcome::Failed);
        assert_eq!(TaskOutcome::of("killed"), TaskOutcome::Stopped);
        assert_eq!(TaskOutcome::of("stopped"), TaskOutcome::Stopped);
        assert_eq!(TaskOutcome::of("something-new"), TaskOutcome::Failed);
    }

    #[test]
    fn only_work_reopens_a_turn() {
        assert!(Beat::Prompt.opens_turn());
        assert!(Beat::Thinking.opens_turn());
        assert!(!Beat::TurnEnded.opens_turn());
        assert!(!Beat::Usage {
            context: 1,
            output: 1,
            window: None
        }
        .opens_turn());
        assert!(!Beat::ToolFinished {
            id: "t".into(),
            failed: false,
            refused: false
        }
        .opens_turn());
    }

    #[test]
    fn a_message_is_kept_as_it_is_but_for_control_noise_and_a_cap() {
        assert_eq!(
            speech("  Done.\n\nTwo files changed.  \n"),
            "Done.\n\nTwo files changed."
        );
        // Markdown, code and other scripts are the text: nothing is rewritten.
        let text = "## Résumé\n\n```rust\n\tlet x = 1;\n```\n* 完了";
        assert_eq!(speech(text), text);
        // What a terminal would act on is taken out.
        assert_eq!(speech("a\u{1b}[31mb\r\nc\u{7}\u{0}"), "a[31mb\nc");
        assert_eq!(speech(" \n\t "), "");
        // A long text is cut on a character, with an ellipsis.
        let long = "é".repeat(MAX_SPEECH_CHARS + 500);
        let cut = speech(&long);
        assert_eq!(cut.chars().count(), MAX_SPEECH_CHARS);
        assert!(cut.ends_with('\u{2026}'));
        let exact = "x".repeat(MAX_SPEECH_CHARS);
        assert_eq!(speech(&exact), exact);
        assert_eq!(
            Beat::said(" hi "),
            Beat::Said {
                text: "hi".to_owned(),
                at: None
            }
        );
    }

    #[test]
    fn a_time_of_a_transcript_is_read_in_seconds() {
        assert_eq!(
            seconds(Some("2026-03-01T10:00:00.000Z")),
            Some(1_772_359_200)
        );
        assert_eq!(
            seconds(Some("2026-03-01T11:00:00+01:00")),
            Some(1_772_359_200)
        );
        assert_eq!(seconds(Some("yesterday")), None);
        assert_eq!(seconds(None), None);
    }
}

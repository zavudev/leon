//! What a stretch of tools says of itself, as a person would: "Ran 2 shell
//! commands, read 3 files" instead of five rows of tool names.
//!
//! Every agent names its tools its own way (`Bash`, `shell`, `exec_command`);
//! [`kind`] is the one place that knows which of them are the same thing, and
//! [`summary`] counts them by kind in the order they first happened.
//!
//! Pure: tools in, words out.

use std::collections::HashSet;

use crate::model::Tool;

/// What a tool does, as far as a person needs to tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Runs a command.
    Shell,
    /// Reads a file.
    Read,
    /// Looks for something in the code.
    Search,
    /// Goes to the web.
    Web,
    /// Hands a task to another agent.
    Agent,
    /// Changes files.
    Edit,
    /// Anything else (a tool of a server, a plan…).
    Other,
}

/// The kind of a tool: a tool that says what it changed is an edit whatever
/// it is called.
pub fn kind(tool: &Tool) -> Kind {
    if !tool.changes.is_empty() {
        return Kind::Edit;
    }
    match tool.name.to_ascii_lowercase().as_str() {
        "bash" | "bashoutput" | "shell" | "exec" | "exec_command" | "execute" | "run_command"
        | "terminal" => Kind::Shell,
        "read" | "read_file" | "view" | "cat" | "open" => Kind::Read,
        "grep" | "glob" | "search" | "find" | "ls" | "list" | "list_directory"
        | "codebase_search" | "toolsearch" => Kind::Search,
        "websearch" | "web_search" | "webfetch" | "web_fetch" | "fetch" => Kind::Web,
        "task" | "agent" => Kind::Agent,
        "edit" | "write" | "multiedit" | "apply_patch" | "patch" | "notebookedit" => Kind::Edit,
        _ => Kind::Other,
    }
}

/// The words for a stretch of tools: each kind once, with how many, in the
/// order they first came ("Ran 2 shell commands, read 3 files"). Empty for
/// no tools.
pub fn summary(tools: &[&Tool]) -> String {
    let mut kinds: Vec<Kind> = Vec::new();
    for tool in tools {
        let kind = kind(tool);
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    let mixed = kinds.len() > 1;
    let parts: Vec<String> = kinds
        .iter()
        .map(|&of| {
            let of_kind: Vec<&&Tool> = tools.iter().filter(|tool| kind(tool) == of).collect();
            part(of, &of_kind, mixed)
        })
        .collect();
    let mut words = parts.join(", ");
    if let Some(first) = words.get(..1) {
        words = first.to_uppercase() + &words[1..];
    }
    words
}

/// One kind's share of the words.
fn part(kind: Kind, tools: &[&&Tool], mixed: bool) -> String {
    let count = tools.len();
    let plural = |one: &str, many: &str| match count {
        1 => one.to_owned(),
        _ => format!("{count} {many}"),
    };
    match kind {
        Kind::Shell => format!("ran {}", plural("1 shell command", "shell commands")),
        Kind::Read => format!("read {}", plural("1 file", "files")),
        Kind::Search => match count {
            1 => "searched the code".to_owned(),
            _ => format!("searched the code {count} times"),
        },
        Kind::Web => format!("looked at {}", plural("1 web page", "web pages")),
        Kind::Agent => format!("delegated {}", plural("1 task", "tasks")),
        Kind::Edit => {
            // The files, not the tools: one file edited five times is a file.
            let files: HashSet<&str> = tools
                .iter()
                .flat_map(|tool| tool.changes.iter().map(|change| change.path.as_str()))
                .collect();
            let files = if files.is_empty() { count } else { files.len() };
            format!(
                "changed {}",
                if files == 1 {
                    "1 file".to_owned()
                } else {
                    format!("{files} files")
                }
            )
        }
        Kind::Other if mixed => format!("used {}", plural("1 other tool", "other tools")),
        Kind::Other => format!("used {}", plural("1 tool", "tools")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_turn::FileChange;

    fn tool(name: &str) -> Tool {
        Tool {
            id: name.into(),
            name: name.into(),
            summary: String::new(),
            detail: String::new(),
            changes: Vec::new(),
            outcome: None,
        }
    }

    fn words(names: &[&str]) -> String {
        let tools: Vec<Tool> = names.iter().map(|name| tool(name)).collect();
        summary(&tools.iter().collect::<Vec<_>>())
    }

    #[test]
    fn every_agents_names_for_a_tool_are_the_same_kind() {
        assert_eq!(kind(&tool("Bash")), Kind::Shell);
        assert_eq!(kind(&tool("shell")), Kind::Shell);
        assert_eq!(kind(&tool("exec_command")), Kind::Shell);
        assert_eq!(kind(&tool("Read")), Kind::Read);
        assert_eq!(kind(&tool("Grep")), Kind::Search);
        assert_eq!(kind(&tool("WebFetch")), Kind::Web);
        assert_eq!(kind(&tool("Task")), Kind::Agent);
        assert_eq!(kind(&tool("MultiEdit")), Kind::Edit);
        assert_eq!(kind(&tool("docs.search")), Kind::Other);
    }

    #[test]
    fn a_tool_that_changed_files_is_an_edit_whatever_it_is_called() {
        let mut odd = tool("patch-it");
        odd.changes = vec![FileChange::written("a.rs", "x")];
        assert_eq!(kind(&odd), Kind::Edit);
    }

    #[test]
    fn a_stretch_counts_each_kind_in_the_order_it_first_came() {
        assert_eq!(
            words(&["Bash", "Read", "Read", "Bash", "Read"]),
            "Ran 2 shell commands, read 3 files"
        );
        assert_eq!(words(&["Read", "Bash"]), "Read 1 file, ran 1 shell command");
        assert_eq!(words(&["Grep"]), "Searched the code");
        assert_eq!(words(&["Grep", "Glob"]), "Searched the code 2 times");
        assert_eq!(
            words(&["WebSearch", "Task", "Task"]),
            "Looked at 1 web page, delegated 2 tasks"
        );
        assert_eq!(words(&["mcp.a", "mcp.b"]), "Used 2 tools");
        assert_eq!(
            words(&["Bash", "mcp.a"]),
            "Ran 1 shell command, used 1 other tool"
        );
        assert_eq!(words(&[]), "");
    }

    #[test]
    fn edits_count_the_files_and_not_the_tools() {
        let mut one = tool("Edit");
        one.changes = vec![FileChange::written("a.rs", "x")];
        let mut again = tool("Edit");
        again.changes = vec![FileChange::written("a.rs", "y")];
        let mut other = tool("Write");
        other.changes = vec![FileChange::written("b.rs", "z")];
        assert_eq!(summary(&[&one, &again]), "Changed 1 file");
        assert_eq!(summary(&[&one, &again, &other]), "Changed 2 files");
    }
}

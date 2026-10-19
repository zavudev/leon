//! The `leon task` command line: how agents and people reach the board from a
//! shell.
//!
//! ```text
//! leon task add [--global] [--status S] [--priority P] [--label L]... [--assignee A]
//!               [--agent A] [--body B | --body -] [--json] <title...>
//! leon task list [--global] [--status S] [--all] [--assignee A] [--label L] [--limit N] [--json]
//! leon task board [--global]
//! leon task show <id> [--json]
//! leon task edit <id> [--title T] [--body B | --body -] [--status S] [--priority P]
//!                [--label L]... | --no-labels] [--assignee A | --no-assignee]
//!                [--branch B | --no-branch] [--parent P | --no-parent]
//!                [--need N]... | --no-needs]
//! leon task move <id> <status> [--before <id>]
//! leon task done <id>
//! leon task rm <id>
//! leon task search [--global] [--limit N] [--json] <words...>
//! leon task mcp            serve the board to an MCP client over stdio
//! ```
//!
//! Parsed by hand like `leon memory` ([`parse`] is pure) and intercepted in
//! `main` before the window's own options ([`intercept`]). The data folder
//! and the project are resolved exactly as the memory's are. Nothing is
//! written into a project: `leon task mcp` is registered by the person, in
//! their agent's own configuration, when they want it.

use crate::memory_cli::{agent_from_env, data_dir_of, mcp_folder};
use crate::task_mcp;
use crate::tasks::{self, Service};
use crate::{memory, product};
use leon_core::{NewTask, Store, StoreError, TaskFilter, TaskPatch, TaskPriority, TaskStatus};
use std::io::{BufRead as _, IsTerminal as _, Read as _, Write as _};
use std::path::{Path, PathBuf};

/// How many tasks `list` shows when it is not told.
const DEFAULT_LISTED: usize = 100;
/// How many hits `search` shows when it is not told.
const DEFAULT_HITS: usize = 10;
/// The most text read from standard input.
const MAX_STDIN: u64 = 64 * 1024;

/// Where a body comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// As given.
    Given(String),
    /// Standard input.
    Stdin,
}

/// What was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Create a task.
    Add {
        /// On the global board.
        global: bool,
        /// Its title.
        title: String,
        /// Its body.
        body: Option<Body>,
        /// Its column.
        status: Option<TaskStatus>,
        /// Its priority.
        priority: Option<TaskPriority>,
        /// Its labels.
        labels: Vec<String>,
        /// Its assignee.
        assignee: Option<String>,
        /// Who creates it.
        agent: Option<String>,
        /// As JSON.
        json: bool,
    },
    /// List tasks.
    List {
        /// Of the global board.
        global: bool,
        /// What is kept.
        filter: TaskFilter,
        /// How many at most.
        limit: usize,
        /// As JSON.
        json: bool,
    },
    /// Print the board by columns.
    Board {
        /// The global board.
        global: bool,
    },
    /// Print one task in full.
    Show {
        /// Its id, or the end of it.
        id: String,
        /// As JSON.
        json: bool,
    },
    /// Change a task.
    Edit {
        /// Its id, or the end of it.
        id: String,
        /// The new title.
        title: Option<String>,
        /// The new body.
        body: Option<Body>,
        /// The new column.
        status: Option<TaskStatus>,
        /// The new priority.
        priority: Option<TaskPriority>,
        /// The new labels.
        labels: Option<Vec<String>>,
        /// The new assignee, or none (`Some(None)`).
        assignee: Option<Option<String>>,
        /// The branch of the task's worktree, or none (`Some(None)`).
        branch: Option<Option<String>>,
        /// The task it is a step of, or none (`Some(None)`).
        parent: Option<Option<String>>,
        /// The tasks it waits for, replacing the old ones.
        needs: Option<Vec<String>>,
    },
    /// Move a task to a column.
    Move {
        /// Its id, or the end of it.
        id: String,
        /// The column.
        status: TaskStatus,
        /// The task to place it above.
        before: Option<String>,
    },
    /// Remove a task for good.
    Remove {
        /// Its id, or the end of it.
        id: String,
    },
    /// Find tasks.
    Search {
        /// In the global board alone.
        global: bool,
        /// How many at most.
        limit: usize,
        /// As JSON.
        json: bool,
        /// What to look for.
        words: String,
    },
    /// Serve the board over MCP.
    Mcp {
        /// Every board, not only the folder's project's: for King Leon,
        /// which runs in no project and names the project of each call.
        anywhere: bool,
    },
    /// Print the usage.
    Help,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The data folder asked for.
    pub data_dir: Option<PathBuf>,
    /// What to do.
    pub action: Action,
}

/// The text printed for `--help` and after a mistake.
pub fn usage() -> String {
    let slug = product::SLUG;
    format!(
        "{slug} task: the board of tasks that people and agents share\n\n\
         Usage:\n  \
         {slug} task add [--global] [--status S] [--priority P] [--label L]... [--assignee A] [--body B|-] [--json] <title...>\n  \
         {slug} task list [--global] [--status S] [--all] [--assignee A] [--label L] [--limit N] [--json]\n  \
         {slug} task board [--global]\n  \
         {slug} task show <id> [--json]\n  \
         {slug} task edit <id> [--title T] [--body B|-] [--status S] [--priority P] [--label L]... [--no-labels] [--assignee A | --no-assignee] [--branch B | --no-branch] [--parent P | --no-parent] [--need N]... [--no-needs]\n  \
         {slug} task move <id> <status> [--before <id>]\n  \
         {slug} task done <id>\n  \
         {slug} task rm <id>\n  \
         {slug} task search [--global] [--limit N] [--json] <words...>\n  \
         {slug} task mcp\n\n\
         Statuses: backlog, todo, doing, review, done, canceled.\n\
         Priorities: none, low, medium, high, urgent.\n\
         The board is the one of the project the current folder belongs to; --global is the\n\
         board that belongs to no project. `list` leaves done and canceled tasks out (--all\n\
         brings them). An id is the one printed in the first column, or the end of it.\n\
         `mcp` serves the board to an agent over standard input and output; register it in\n\
         the agent's own configuration, for example: claude mcp add leon-tasks -- {slug} task mcp\n"
    )
}

/// When the arguments of the program are a `leon task` command line: the data
/// folder given before the word `task`, and what follows it.
pub fn intercept(arguments: &[String]) -> Option<(Option<PathBuf>, Vec<String>)> {
    let mut data_dir = None;
    let mut rest = arguments;
    loop {
        match rest {
            [first, tail @ ..] if first == "task" => return Some((data_dir, tail.to_vec())),
            [first, value, tail @ ..] if first == "--data-dir" => {
                data_dir = Some(PathBuf::from(value));
                rest = tail;
            }
            [first, tail @ ..] if first.starts_with("--data-dir=") => {
                data_dir = Some(PathBuf::from(&first["--data-dir=".len()..]));
                rest = tail;
            }
            _ => return None,
        }
    }
}

fn limit_of(text: &str) -> Result<usize, String> {
    text.parse::<usize>()
        .ok()
        .filter(|limit| *limit >= 1)
        .ok_or_else(|| format!("--limit needs a number, 1 or more, not {text:?}."))
}

fn body_of(value: String) -> Body {
    if value == "-" {
        Body::Stdin
    } else {
        Body::Given(value)
    }
}

/// Parses what follows `leon task`. `data_dir` is the one given before it.
pub fn parse(
    data_dir: Option<PathBuf>,
    args: impl IntoIterator<Item = String>,
) -> Result<Invocation, String> {
    let mut data_dir = data_dir;
    let mut command: Option<String> = None;
    let mut words: Vec<String> = Vec::new();
    let (mut global, mut json, mut all) = (false, false, false);
    let mut anywhere = false;
    let (mut no_labels, mut no_assignee, mut no_branch) = (false, false, false);
    let mut branch: Option<String> = None;
    let (mut no_parent, mut no_needs) = (false, false);
    let mut parent: Option<String> = None;
    let mut needs: Vec<String> = Vec::new();
    let mut title: Option<String> = None;
    let mut body: Option<Body> = None;
    let mut status: Option<TaskStatus> = None;
    let mut priority: Option<TaskPriority> = None;
    let mut labels: Vec<String> = Vec::new();
    let (mut assignee, mut agent, mut before) = (None, None, None);
    let mut limit: Option<usize> = None;
    let mut given: Vec<&'static str> = Vec::new();

    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        if !argument.starts_with('-') || argument == "-" {
            match command {
                None => command = Some(argument),
                Some(_) => words.push(argument),
            }
            continue;
        }
        if argument == "--" {
            words.extend(args.by_ref());
            break;
        }
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) if name.starts_with("--") => {
                (name.to_owned(), Some(value.to_owned()))
            }
            _ => (argument.clone(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{name} needs {what}."))
        };
        match name.as_str() {
            "-h" | "--help" => {
                return Ok(Invocation {
                    data_dir,
                    action: Action::Help,
                })
            }
            "--data-dir" => {
                let path = value("a path")?;
                if path.is_empty() {
                    return Err("--data-dir needs a path.".to_owned());
                }
                data_dir = Some(PathBuf::from(path));
            }
            "--global" => {
                global = true;
                given.push("--global");
            }
            "--anywhere" => {
                anywhere = true;
                given.push("--anywhere");
            }
            "--json" => {
                json = true;
                given.push("--json");
            }
            "--all" => {
                all = true;
                given.push("--all");
            }
            "--no-labels" => {
                no_labels = true;
                given.push("--no-labels");
            }
            "--no-assignee" => {
                no_assignee = true;
                given.push("--no-assignee");
            }
            "--no-branch" => {
                no_branch = true;
                given.push("--no-branch");
            }
            "--branch" => {
                branch = Some(value("a branch name")?);
                given.push("--branch");
            }
            "--parent" => {
                parent = Some(value("a task id")?);
                given.push("--parent");
            }
            "--no-parent" => {
                no_parent = true;
                given.push("--no-parent");
            }
            "--need" => {
                needs.push(value("a task id")?);
                given.push("--need");
            }
            "--no-needs" => {
                no_needs = true;
                given.push("--no-needs");
            }
            "--title" => {
                title = Some(value("a title")?);
                given.push("--title");
            }
            "--body" => {
                body = Some(body_of(value("a text, or - for standard input")?));
                given.push("--body");
            }
            "--status" => {
                status = Some(
                    TaskStatus::parse(&value("a status")?).map_err(|error| error.to_string())?,
                );
                given.push("--status");
            }
            "--priority" => {
                priority = Some(
                    TaskPriority::parse(&value("a priority")?)
                        .map_err(|error| error.to_string())?,
                );
                given.push("--priority");
            }
            "--label" => {
                labels.push(value("a label")?);
                given.push("--label");
            }
            "--assignee" => {
                assignee = Some(value("a name")?);
                given.push("--assignee");
            }
            "--agent" => {
                agent = Some(value("a name")?);
                given.push("--agent");
            }
            "--before" => {
                before = Some(value("a task id")?);
                given.push("--before");
            }
            "--limit" => {
                limit = Some(limit_of(&value("a number")?)?);
                given.push("--limit");
            }
            other => return Err(format!("unknown option {other}.")),
        }
    }

    let Some(command) = command else {
        return Err("a command is needed.".to_owned());
    };
    // Only what a command takes: a flag it ignores is a mistake, said.
    let allowed: &[&str] = match command.as_str() {
        "add" => &[
            "--global",
            "--status",
            "--priority",
            "--label",
            "--assignee",
            "--agent",
            "--body",
            "--json",
        ],
        "list" | "ls" => &[
            "--global",
            "--status",
            "--all",
            "--assignee",
            "--label",
            "--limit",
            "--json",
        ],
        "board" => &["--global"],
        "show" => &["--json"],
        "edit" => &[
            "--title",
            "--body",
            "--status",
            "--priority",
            "--label",
            "--no-labels",
            "--assignee",
            "--no-assignee",
            "--branch",
            "--no-branch",
            "--parent",
            "--no-parent",
            "--need",
            "--no-needs",
        ],
        "move" => &["--before"],
        "search" => &["--global", "--limit", "--json"],
        "mcp" => &["--anywhere"],
        "done" | "rm" | "help" => &[],
        other => {
            return Err(format!(
                "unknown command {other:?}: use add, list, board, show, edit, move, done, \
                 rm, search or mcp."
            ))
        }
    };
    if let Some(flag) = given.iter().find(|flag| !allowed.contains(flag)) {
        return Err(format!("{flag} is not an option of `task {command}`."));
    }
    let one_id = |words: &[String]| -> Result<String, String> {
        match words {
            [id] => Ok(id.clone()),
            [] => Err(format!("`task {command}` needs a task id.")),
            _ => Err(format!("`task {command}` takes one task id.")),
        }
    };
    let action = match command.as_str() {
        "help" => Action::Help,
        "add" => {
            let title = words.join(" ");
            if title.trim().is_empty() {
                return Err("`task add` needs a title.".to_owned());
            }
            Action::Add {
                global,
                title,
                body,
                status,
                priority,
                labels,
                assignee,
                agent,
                json,
            }
        }
        "list" | "ls" => {
            if !words.is_empty() {
                return Err(
                    "`task list` takes no words: use --status, --assignee, --label.".into(),
                );
            }
            Action::List {
                global,
                filter: TaskFilter {
                    status,
                    open_only: !all && status.is_none(),
                    assignee,
                    label: labels.into_iter().next(),
                    parent: None,
                },
                limit: limit.unwrap_or(DEFAULT_LISTED),
                json,
            }
        }
        "board" => Action::Board { global },
        "show" => Action::Show {
            id: one_id(&words)?,
            json,
        },
        "edit" => {
            if assignee.is_some() && no_assignee {
                return Err("--assignee and --no-assignee exclude each other.".to_owned());
            }
            if !labels.is_empty() && no_labels {
                return Err("--label and --no-labels exclude each other.".to_owned());
            }
            if branch.is_some() && no_branch {
                return Err("--branch and --no-branch exclude each other.".to_owned());
            }
            if parent.is_some() && no_parent {
                return Err("--parent and --no-parent exclude each other.".to_owned());
            }
            if !needs.is_empty() && no_needs {
                return Err("--need and --no-needs exclude each other.".to_owned());
            }
            let edit = Action::Edit {
                id: one_id(&words)?,
                title,
                body,
                status,
                priority,
                labels: if no_labels {
                    Some(Vec::new())
                } else if labels.is_empty() {
                    None
                } else {
                    Some(labels)
                },
                assignee: if no_assignee {
                    Some(None)
                } else {
                    assignee.map(Some)
                },
                branch: if no_branch {
                    Some(None)
                } else {
                    branch.map(Some)
                },
                parent: if no_parent {
                    Some(None)
                } else {
                    parent.map(Some)
                },
                needs: if no_needs {
                    Some(Vec::new())
                } else if needs.is_empty() {
                    None
                } else {
                    Some(needs)
                },
            };
            if given.is_empty() {
                return Err("`task edit` needs something to change.".to_owned());
            }
            edit
        }
        "move" => match words.as_slice() {
            [id, column] => Action::Move {
                id: id.clone(),
                status: TaskStatus::parse(column).map_err(|error| error.to_string())?,
                before,
            },
            _ => return Err("`task move` needs a task id and a status.".to_owned()),
        },
        "done" => Action::Move {
            id: one_id(&words)?,
            status: TaskStatus::Done,
            before: None,
        },
        "rm" => Action::Remove {
            id: one_id(&words)?,
        },
        "search" => {
            let words = words.join(" ");
            if words.trim().is_empty() {
                return Err("`task search` needs words to look for.".to_owned());
            }
            Action::Search {
                global,
                limit: limit.unwrap_or(DEFAULT_HITS),
                json,
                words,
            }
        }
        _ => {
            if !words.is_empty() {
                return Err("`task mcp` takes no arguments.".to_owned());
            }
            Action::Mcp { anywhere }
        }
    };
    Ok(Invocation { data_dir, action })
}

fn open_failure(path: &Path, error: &StoreError) -> String {
    match error {
        StoreError::SchemaTooNew { found, supported } => format!(
            "{} was written by a newer {name} (schema {found}; this {name} reads up to {supported}).\n\
             Use the {name} that is running: \"$LEON_BIN\" task ...",
            path.display(),
            name = product::PRODUCT_NAME,
        ),
        other => format!("cannot open {}: {other}", path.display()),
    }
}

fn read_stdin(command: &str) -> Result<String, String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(format!(
            "{command} needs standard input to be piped when the body is -."
        ));
    }
    let mut text = String::new();
    stdin
        .lock()
        .take(MAX_STDIN)
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read standard input: {error}"))?;
    Ok(text)
}

fn body_text(body: Option<Body>, command: &str) -> Result<Option<String>, String> {
    match body {
        None => Ok(None),
        Some(Body::Given(text)) => Ok(Some(text)),
        Some(Body::Stdin) => read_stdin(command).map(Some),
    }
}

/// Runs `leon task`: `arguments` is what follows the word, `data_dir` the
/// folder given before it. Returns the exit code.
pub fn run(data_dir: Option<PathBuf>, arguments: Vec<String>) -> i32 {
    let invocation = match parse(data_dir, arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("{message}\n\n{}", usage());
            return 2;
        }
    };
    if invocation.action == Action::Help {
        println!("{}", usage());
        return 0;
    }
    match carry_out(invocation) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("{}: {message}", product::SLUG);
            1
        }
    }
}

fn pretty(values: &[serde_json::Value]) -> String {
    serde_json::to_string_pretty(values).unwrap_or_default()
}

fn carry_out(invocation: Invocation) -> Result<(), String> {
    let variable = |name: &str| std::env::var(name).ok();
    let data_dir = data_dir_of(invocation.data_dir, &variable, product::data_dir);
    let folder = std::env::current_dir()
        .map_err(|error| format!("cannot tell the current folder: {error}"))?;
    let mcp = match invocation.action {
        Action::Mcp { anywhere } => Some(anywhere),
        _ => None,
    };
    let folder = if mcp.is_some() {
        mcp_folder(&variable, folder)
    } else {
        folder
    };
    std::fs::create_dir_all(&data_dir)
        .map_err(|error| format!("cannot create {}: {error}", data_dir.display()))?;
    let database = data_dir.join("leon.db");
    let store = Store::open(&database).map_err(|error| open_failure(&database, &error))?;
    let root = memory::resolve_root(&store, &memory::Disk, &folder);
    let service = Service::new(store, root).anywhere(mcp == Some(true));
    if mcp.is_some() {
        tracing::info!(root = service.root(), "serving the board over MCP");
        let mut server = task_mcp::Server::new(service, product::VERSION);
        return converse(
            &mut server,
            &mut std::io::stdin().lock(),
            &mut std::io::stdout().lock(),
        )
        .map_err(|error| format!("the MCP conversation ended: {error}"));
    }
    let said = |error: StoreError| tasks::sentence(&error);
    let mut out = std::io::stdout().lock();
    // A closed pipe (`| head`) is not an error worth a message.
    let mut print = |text: &str| {
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    };

    match invocation.action {
        Action::Help | Action::Mcp { .. } => {}
        Action::Add {
            global,
            title,
            body,
            status,
            priority,
            labels,
            assignee,
            agent,
            json,
        } => {
            let new = NewTask {
                scope: service.scope(global),
                title,
                body: body_text(body, "add")?.unwrap_or_default(),
                status,
                priority,
                labels,
                assignee,
                agent: agent.or_else(|| agent_from_env(&variable)),
            };
            let saved = service.add(&new).map_err(said)?;
            let created = matches!(saved, leon_core::SavedTask::New(_));
            if json {
                let mut value = tasks::json_of(saved.task(), None);
                if let Some(object) = value.as_object_mut() {
                    object.insert("created".into(), created.into());
                }
                print(&format!("{}\n", pretty(&[value])));
            } else if created {
                print(&format!("Added {}\n", tasks::line(saved.task())));
            } else {
                print(&format!(
                    "Already on the board as {}\n",
                    tasks::line(saved.task())
                ));
            }
        }
        Action::List {
            global,
            filter,
            limit,
            json,
        } => {
            let found = service.list(global, &filter, limit).map_err(said)?;
            if json {
                let values: Vec<_> = found
                    .iter()
                    .map(|task| tasks::json_of(task, None))
                    .collect();
                print(&format!("{}\n", pretty(&values)));
            } else if found.is_empty() {
                print("No tasks.\n");
            } else {
                for task in &found {
                    print(&format!("{}\n", tasks::line(task)));
                }
            }
        }
        Action::Board { global } => {
            let found = service.board(global, 500).map_err(said)?;
            print(&tasks::board(&found));
        }
        Action::Show { id, json } => {
            let task = service.get(&id).map_err(said)?;
            if json {
                print(&format!("{}\n", pretty(&[tasks::json_of(&task, None)])));
            } else {
                print(&tasks::detail(&task));
            }
        }
        Action::Edit {
            id,
            title,
            body,
            status,
            priority,
            labels,
            assignee,
            branch,
            parent,
            needs,
        } => {
            service.get(&id).map_err(said)?;
            let patch = TaskPatch {
                title,
                body: body_text(body, "edit")?,
                status,
                priority,
                labels,
                assignee,
                branch,
                parent,
                needs,
            };
            let task = service.update(&id, &patch).map_err(said)?;
            print(&format!("Changed {}\n", tasks::line(&task)));
        }
        Action::Move { id, status, before } => {
            service.get(&id).map_err(said)?;
            let task = service
                .move_to(&id, status, before.as_deref())
                .map_err(said)?;
            print(&format!("Moved {}\n", tasks::line(&task)));
        }
        Action::Remove { id } => {
            service.get(&id).map_err(said)?;
            let task = service.delete(&id).map_err(said)?;
            print(&format!("Removed {}\n", tasks::line(&task)));
        }
        Action::Search {
            global,
            limit,
            json,
            words,
        } => {
            let hits = service.search(global, &words, limit).map_err(said)?;
            if json {
                let values: Vec<_> = hits
                    .iter()
                    .map(|hit| tasks::json_of(&hit.task, Some(&hit.snippet)))
                    .collect();
                print(&format!("{}\n", pretty(&values)));
            } else if hits.is_empty() {
                print("No task matches.\n");
            } else {
                for hit in &hits {
                    print(&format!("{}\n", tasks::line(&hit.task)));
                }
            }
        }
    }
    Ok(())
}

/// The MCP conversation over the two pipes given: a line in, at most a line
/// out, until the input ends. Lines longer than [`task_mcp::MAX_LINE`] are
/// read to their end, thrown away and answered with an error.
pub fn converse<B: task_mcp::Backend>(
    server: &mut task_mcp::Server<B>,
    input: &mut impl std::io::BufRead,
    output: &mut impl std::io::Write,
) -> std::io::Result<()> {
    loop {
        let mut bytes = Vec::new();
        let read = input
            .by_ref()
            .take(task_mcp::MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut bytes)?;
        if read == 0 {
            return Ok(());
        }
        let answer = if bytes.last() != Some(&b'\n') && bytes.len() > task_mcp::MAX_LINE {
            loop {
                let buffer = input.fill_buf()?;
                if buffer.is_empty() {
                    break;
                }
                match buffer.iter().position(|byte| *byte == b'\n') {
                    Some(at) => {
                        input.consume(at + 1);
                        break;
                    }
                    None => {
                        let length = buffer.len();
                        input.consume(length);
                    }
                }
            }
            Some(task_mcp::too_long())
        } else {
            server.handle(&String::from_utf8_lossy(&bytes))
        };
        if let Some(answer) = answer {
            output.write_all(answer.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn parsed(args: &[&str]) -> Result<Action, String> {
        parse(None, strings(args)).map(|invocation| invocation.action)
    }

    #[test]
    fn the_word_task_is_taken_with_a_data_folder_before_it() {
        assert_eq!(
            intercept(&strings(&["task", "list"])),
            Some((None, strings(&["list"])))
        );
        assert_eq!(
            intercept(&strings(&["--data-dir=/d", "task", "add", "x"])),
            Some((Some(PathBuf::from("/d")), strings(&["add", "x"])))
        );
        for other in [&[][..], &["memory", "task"], &["--theme", "dark", "task"]] {
            assert_eq!(intercept(&strings(other)), None, "{other:?}");
        }
    }

    #[test]
    fn add_takes_the_words_as_the_title_and_its_options_anywhere() {
        assert_eq!(
            parsed(&[
                "add",
                "--status",
                "backlog",
                "Fix",
                "the",
                "login",
                "--label",
                "bug",
                "--label",
                "ui",
                "--priority=high",
                "--json",
            ])
            .unwrap(),
            Action::Add {
                global: false,
                title: "Fix the login".into(),
                body: None,
                status: Some(TaskStatus::Backlog),
                priority: Some(TaskPriority::High),
                labels: vec!["bug".into(), "ui".into()],
                assignee: None,
                agent: None,
                json: true,
            }
        );
        assert!(parsed(&["add"]).unwrap_err().contains("needs a title"));
        assert!(parsed(&["add", "--status", "later", "x"])
            .unwrap_err()
            .contains("not a status"));
        // A dash alone is a word, and `--body -` is standard input.
        assert!(matches!(
            parsed(&["add", "--body", "-", "t"]).unwrap(),
            Action::Add {
                body: Some(Body::Stdin),
                ..
            }
        ));
    }

    #[test]
    fn list_keeps_the_open_tasks_unless_told_otherwise() {
        let open = |args: &[&str]| match parsed(args).unwrap() {
            Action::List { filter, .. } => filter.open_only,
            other => panic!("{other:?}"),
        };
        assert!(open(&["list"]));
        assert!(!open(&["list", "--all"]));
        // Asking for a column asks for it whatever it is.
        assert!(!open(&["list", "--status", "done"]));
        assert!(parsed(&["list", "extra"]).is_err());
    }

    #[test]
    fn move_done_and_edit_are_read_with_their_arguments() {
        assert_eq!(
            parsed(&["move", "abc123", "in-progress", "--before", "def456"]).unwrap(),
            Action::Move {
                id: "abc123".into(),
                status: TaskStatus::Doing,
                before: Some("def456".into()),
            }
        );
        assert_eq!(
            parsed(&["done", "abc123"]).unwrap(),
            Action::Move {
                id: "abc123".into(),
                status: TaskStatus::Done,
                before: None,
            }
        );
        assert!(matches!(
            parsed(&["edit", "abc123", "--no-assignee", "--no-labels"]).unwrap(),
            Action::Edit {
                assignee: Some(None),
                labels: Some(labels),
                ..
            } if labels.is_empty()
        ));
        assert!(parsed(&["edit", "abc123"])
            .unwrap_err()
            .contains("something"));
        assert!(parsed(&["edit", "abc123", "--assignee", "a", "--no-assignee"]).is_err());
    }

    #[test]
    fn a_flag_a_command_does_not_take_is_a_mistake() {
        assert!(parsed(&["show", "abc123", "--status", "todo"])
            .unwrap_err()
            .contains("not an option of `task show`"));
        assert!(parsed(&["frobnicate"])
            .unwrap_err()
            .contains("unknown command"));
        assert!(parsed(&[]).is_err());
        assert_eq!(parsed(&["--help"]).unwrap(), Action::Help);
    }

    #[test]
    fn the_conversation_answers_a_line_per_request_and_survives_a_long_one() {
        use crate::tasks::Service as Tasks;
        let service = Tasks::new(Store::open_in_memory().unwrap(), "/srv/api".to_owned());
        let mut server = task_mcp::Server::new(service, "test");
        let long = "x".repeat(task_mcp::MAX_LINE + 10);
        let input = format!(
            "{}\n{long}\n{}\n",
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        );
        let mut output = Vec::new();
        converse(&mut server, &mut input.as_bytes(), &mut output).unwrap();
        let lines: Vec<&str> = std::str::from_utf8(&output).unwrap().lines().collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains(r#""id":1"#));
        assert!(lines[1].contains("too long"));
        assert!(lines[2].contains(r#""id":2"#));
    }
}

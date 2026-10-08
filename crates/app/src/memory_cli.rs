//! The `leon memory` command line: how agents and people reach the shared
//! memory from a shell.
//!
//! ```text
//! leon memory add [--global] [--kind K] [--title T] [--topic KEY] [--agent A] <text...>
//! leon memory edit <id> [--kind K] [--title T] [--topic KEY | --no-topic] [<text...> | -]
//! leon memory show <id> [--json]
//! leon memory search [--global] [--limit N] [--json] <words...>
//! leon memory list [--global] [--forgotten] [--limit N] [--json]
//! leon memory pin <id> | unpin <id>
//! leon memory forget [--hard] <id>
//! leon memory restore <id>
//! leon memory purge [--older-than <days>]
//! leon memory context        print the memory file of this folder's project
//! leon memory path           print where that file is
//! leon memory mcp            serve the memory to an MCP client over stdio
//! leon memory enable [--mcp] [--dry-run]
//! leon memory disable [--dry-run]
//! leon memory status
//! ```
//!
//! Parsed by hand like the rest of the command line ([`parse`] is pure), and
//! intercepted in `main` before the window's own options ([`intercept`]), the
//! way `leon host` is. The data folder is `--data-dir`, else the
//! `LEON_DATA_DIR` a terminal of Leon was given, else the platform's.
//!
//! The project is the one the current folder belongs to
//! (`memory::resolve_root`). `add` reads the text from standard input when it
//! is `-`, or absent while standard input is not a terminal. `mcp` writes
//! nothing but protocol messages to standard output; the log goes to standard
//! error, as everywhere in Leon.
//!
//! `enable` is the one command that writes into a project: the managed block
//! in its instruction files and, with `--mcp`, the `leon-memory` entry of its
//! `.mcp.json`. It names every file it touched; `disable` undoes both.

use crate::memory::{self, Service};
use crate::product;
use leon_core::{MemoryKind, MemoryPatch, NewMemory, Store, StoreError};
use leon_memory::{mcp, mcp_json, render};
use std::io::{BufRead as _, IsTerminal as _, Read as _, Write as _};
use std::path::{Path, PathBuf};

/// How many entries `list` shows when it is not told.
const DEFAULT_LISTED: usize = 50;
/// How many hits `search` shows when it is not told.
const DEFAULT_HITS: usize = 10;
/// The most text read from standard input: far more than an entry may hold,
/// so that what is too long is refused by its length and not cut silently.
const MAX_STDIN: u64 = 64 * 1024;

/// The commands, as a list for a message.
const COMMANDS: &str =
    "add, edit, show, search, list, pin, unpin, forget, restore, purge, context, \
                        path, mcp, enable, disable or status";

/// What was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Save an entry.
    Add {
        /// To the global scope instead of the project's.
        global: bool,
        /// What it is about, when said.
        kind: Option<MemoryKind>,
        /// Its title, when given.
        title: Option<String>,
        /// Who writes it, when said.
        agent: Option<String>,
        /// The topic it is the word on, when said.
        topic: Option<String>,
        /// The text, or `None` to read standard input.
        text: Option<String>,
    },
    /// Change an entry.
    Edit {
        /// Its id, or the end of it.
        id: String,
        /// A new kind.
        kind: Option<MemoryKind>,
        /// A new title.
        title: Option<String>,
        /// A new topic, or none any more (`Some(None)`).
        topic: Option<Option<String>>,
        /// A new text.
        text: NewText,
    },
    /// Print one entry in full.
    Show {
        /// Its id, or the end of it.
        id: String,
        /// As JSON.
        json: bool,
    },
    /// Find entries.
    Search {
        /// In the global scope alone.
        global: bool,
        /// How many at most.
        limit: usize,
        /// As JSON.
        json: bool,
        /// What to look for.
        words: String,
    },
    /// List entries.
    List {
        /// Of the global scope.
        global: bool,
        /// The forgotten ones instead of the live ones.
        forgotten: bool,
        /// How many at most.
        limit: usize,
        /// As JSON.
        json: bool,
    },
    /// Pin one entry, or unpin it.
    Pin {
        /// Its id, or the end of it.
        id: String,
        /// Pin, or unpin.
        pinned: bool,
    },
    /// Forget one entry.
    Forget {
        /// Its id, or the end of it.
        id: String,
        /// For good, instead of until it is restored or purged.
        hard: bool,
    },
    /// Bring a forgotten entry back.
    Restore {
        /// Its id, or the end of it.
        id: String,
    },
    /// Remove for good what was forgotten long enough ago.
    Purge {
        /// How many days ago at least.
        days: u32,
    },
    /// Print the memory file's text.
    Context,
    /// Print the memory file's path.
    Path,
    /// Serve MCP over standard input and output.
    Mcp,
    /// Write the block (and the MCP entry) into the project.
    Enable {
        /// Also register the server in `.mcp.json`.
        mcp: bool,
        /// Only say what would change.
        dry_run: bool,
    },
    /// Take both out again.
    Disable {
        /// Only say what would change.
        dry_run: bool,
    },
    /// Say what is on and where things are.
    Status,
    /// Print the usage.
    Help,
}

/// The text an edit gives an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewText {
    /// None: the text stays.
    Keep,
    /// What standard input holds.
    Stdin,
    /// These words.
    Given(String),
}

/// A parsed `leon memory` command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The data folder asked for with `--data-dir`.
    pub data_dir: Option<PathBuf>,
    /// What to do.
    pub action: Action,
}

/// The text printed by `leon memory --help`.
pub fn usage() -> String {
    format!(
        "The memory agents share: decisions, conventions and discoveries kept across\n\
         sessions and agents, for one project (with all its worktrees) or for all.\n\n\
         Usage: {slug} memory <command> [options]\n\n\
         Commands:\n  \
         add [--global] [--kind K] [--title T] [--topic KEY] [--agent A] <text...>\n                             \
         Save one fact (kinds: {kinds}).\n                             \
         The text is read from standard input when it is - or absent.\n                             \
         With --topic, a fact saved before under that key is revised\n                             \
         instead of a second one added. A text that is known already\n                             \
         is not added again.\n  \
         edit <id> [--kind K] [--title T] [--topic KEY | --no-topic] [<text...> | -]\n                             \
         Change an entry; what is not given stays\n  \
         show <id> [--json]         Print one entry in full: the memory file shows only the\n                             \
         start of a long one\n  \
         search [--global] [--limit N] [--json] <words...>\n                             \
         Search this project's memory and the global one\n  \
         list [--global] [--forgotten] [--limit N] [--json]\n                             \
         List this project's entries, or the global ones\n  \
         pin <id> | unpin <id>      Keep an entry in full and first in the memory file\n  \
         forget [--hard] <id>       Forget an entry (the 8 characters shown are enough);\n                             \
         --hard removes it for good at once\n  \
         restore <id>               Bring a forgotten entry back\n  \
         purge [--older-than DAYS]  Remove for good what was forgotten more than DAYS days\n                             \
         ago (default {kept}), in every project\n  \
         context                    Print the memory file: what an agent reads\n  \
         path                       Print where the memory file is\n  \
         mcp                        Serve the memory to an MCP client over stdio\n  \
         enable [--mcp] [--dry-run] Tell this project's agents: write the managed block\n                             \
         into AGENTS.md / CLAUDE.md / GEMINI.md; with --mcp also\n                             \
         register the server in .mcp.json\n  \
         disable [--dry-run]        Remove what enable wrote\n  \
         status                     What is on, where the file is, how much is kept\n\n\
         Options:\n  \
         --data-dir <path>          Leon's data folder (default: $LEON_DATA_DIR, else the platform's)\n  \
         --global                   The memory of every project instead of this one's\n  \
         -h, --help                 Print this help\n\n\
         The project is the one the current folder belongs to. See docs/MEMORY.md.",
        slug = product::SLUG,
        kinds = memory::kinds(),
        kept = memory::KEPT_DAYS,
    )
}

/// When the arguments of the program are a `leon memory` command line: the
/// data folder given before the word `memory`, and what follows it.
pub fn intercept(arguments: &[String]) -> Option<(Option<PathBuf>, Vec<String>)> {
    let mut data_dir = None;
    let mut rest = arguments;
    loop {
        match rest {
            [first, tail @ ..] if first == "memory" => return Some((data_dir, tail.to_vec())),
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

/// Parses what follows `leon memory`. `data_dir` is the one given before it.
pub fn parse(
    data_dir: Option<PathBuf>,
    args: impl IntoIterator<Item = String>,
) -> Result<Invocation, String> {
    let mut data_dir = data_dir;
    let mut command: Option<String> = None;
    let mut words: Vec<String> = Vec::new();
    let (mut global, mut json, mut with_mcp, mut dry_run) = (false, false, false, false);
    let (mut forgotten, mut hard, mut no_topic) = (false, false, false);
    let mut topic: Option<String> = None;
    let mut days: Option<u32> = None;
    let mut kind: Option<MemoryKind> = None;
    let (mut title, mut agent) = (None, None);
    let mut limit: Option<usize> = None;
    let mut given: Vec<&'static str> = Vec::new();

    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        // `-` alone is "standard input", a word.
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
            "--json" => {
                json = true;
                given.push("--json");
            }
            "--mcp" => {
                with_mcp = true;
                given.push("--mcp");
            }
            "--dry-run" => {
                dry_run = true;
                given.push("--dry-run");
            }
            "--forgotten" => {
                forgotten = true;
                given.push("--forgotten");
            }
            "--hard" => {
                hard = true;
                given.push("--hard");
            }
            "--no-topic" => {
                no_topic = true;
                given.push("--no-topic");
            }
            "--topic" => {
                let text = value("a key")?;
                topic = Some(
                    leon_core::store::memory::topic_key(&text)
                        .map_err(|error| format!("{error}."))?,
                );
                given.push("--topic");
            }
            "--older-than" => {
                let text = value("a number of days")?;
                days =
                    Some(text.parse().map_err(|_| {
                        format!("--older-than needs a number of days, not {text:?}.")
                    })?);
                given.push("--older-than");
            }
            "--kind" => {
                let text = value("a kind")?;
                kind = Some(MemoryKind::parse(&text).map_err(|error| format!("{error}."))?);
                given.push("--kind");
            }
            "--title" => {
                title = Some(value("a title")?);
                given.push("--title");
            }
            "--agent" => {
                let text = value("an agent's name")?;
                agent = Some(
                    leon_core::store::memory::agent_tag(&text)
                        .map_err(|error| format!("{error}."))?,
                );
                given.push("--agent");
            }
            "--limit" => {
                limit = Some(limit_of(&value("a number")?)?);
                given.push("--limit");
            }
            other => return Err(format!("Unknown option {other:?}.")),
        }
    }

    let Some(command) = command else {
        return if given.is_empty() {
            Ok(Invocation {
                data_dir,
                action: Action::Help,
            })
        } else {
            Err(format!("Say what to do: {COMMANDS}."))
        };
    };
    // Each command takes its own options and no other.
    let only = |allowed: &[&str]| -> Result<(), String> {
        match given.iter().find(|option| !allowed.contains(option)) {
            Some(option) => Err(format!("{option} does not go with {command}.")),
            None => Ok(()),
        }
    };
    let no_words = || -> Result<(), String> {
        match words.first() {
            Some(word) => Err(format!("{command} takes no argument: {word:?}.")),
            None => Ok(()),
        }
    };
    let one_id = || -> Result<String, String> {
        match words.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(format!("{command} needs the id of an entry.")),
            _ => Err(format!("{command} takes one id.")),
        }
    };
    let action = match command.as_str() {
        "add" => {
            only(&["--global", "--kind", "--title", "--agent", "--topic"])?;
            let text = match words.as_slice() {
                [] => None,
                [dash] if dash == "-" => None,
                words => Some(words.join(" ")),
            };
            Action::Add {
                global,
                kind,
                title,
                agent,
                topic,
                text,
            }
        }
        "edit" => {
            only(&["--kind", "--title", "--topic", "--no-topic"])?;
            if topic.is_some() && no_topic {
                return Err("--topic and --no-topic do not go together.".to_owned());
            }
            let Some((id, rest)) = words.split_first() else {
                return Err("edit needs the id of an entry.".to_owned());
            };
            let text = match rest {
                [] => NewText::Keep,
                [dash] if dash == "-" => NewText::Stdin,
                rest => NewText::Given(rest.join(" ")),
            };
            let topic = match topic {
                Some(topic) => Some(Some(topic)),
                None if no_topic => Some(None),
                None => None,
            };
            if kind.is_none() && title.is_none() && topic.is_none() && text == NewText::Keep {
                return Err(
                    "edit needs something to change: a text, --kind, --title, --topic or --no-topic."
                        .to_owned(),
                );
            }
            Action::Edit {
                id: id.clone(),
                kind,
                title,
                topic,
                text,
            }
        }
        "search" => {
            only(&["--global", "--limit", "--json"])?;
            if words.is_empty() {
                return Err("search needs words to look for.".to_owned());
            }
            Action::Search {
                global,
                limit: limit.unwrap_or(DEFAULT_HITS),
                json,
                words: words.join(" "),
            }
        }
        "list" => {
            only(&["--global", "--limit", "--json", "--forgotten"])?;
            no_words()?;
            Action::List {
                global,
                forgotten,
                limit: limit.unwrap_or(DEFAULT_LISTED),
                json,
            }
        }
        "forget" => {
            only(&["--hard"])?;
            Action::Forget {
                id: one_id()?,
                hard,
            }
        }
        "show" => {
            only(&["--json"])?;
            Action::Show {
                id: one_id()?,
                json,
            }
        }
        "pin" | "unpin" => {
            only(&[])?;
            Action::Pin {
                id: one_id()?,
                pinned: command == "pin",
            }
        }
        "restore" => {
            only(&[])?;
            Action::Restore { id: one_id()? }
        }
        "purge" => {
            only(&["--older-than"])?;
            no_words()?;
            Action::Purge {
                days: days.unwrap_or(memory::KEPT_DAYS),
            }
        }
        "enable" => {
            only(&["--mcp", "--dry-run"])?;
            no_words()?;
            Action::Enable {
                mcp: with_mcp,
                dry_run,
            }
        }
        "disable" => {
            only(&["--dry-run"])?;
            no_words()?;
            Action::Disable { dry_run }
        }
        simple @ ("context" | "path" | "mcp" | "status") => {
            only(&[])?;
            no_words()?;
            match simple {
                "context" => Action::Context,
                "path" => Action::Path,
                "mcp" => Action::Mcp,
                _ => Action::Status,
            }
        }
        other => return Err(format!("Unknown memory command {other:?}: use {COMMANDS}.")),
    };
    Ok(Invocation { data_dir, action })
}

/// The data folder of a run: the one asked for, else the one a terminal of
/// Leon was told, else the platform's.
pub fn data_dir_of(
    asked: Option<PathBuf>,
    variable: &dyn Fn(&str) -> Option<String>,
    platform: impl FnOnce() -> PathBuf,
) -> PathBuf {
    asked
        .or_else(|| {
            variable(leon_memory::ENV_DATA_DIR)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(platform)
}

/// The folder the MCP server works for: the project folder its client names
/// (Claude Code sets `CLAUDE_PROJECT_DIR` for the servers it starts and does
/// not promise where it starts them), else the folder it was started in.
pub fn mcp_folder(variable: &dyn Fn(&str) -> Option<String>, current: PathBuf) -> PathBuf {
    variable("CLAUDE_PROJECT_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or(current)
}

/// The agent a shell command was run by, from the variables the agents set
/// for what they start. `None` when nothing says.
pub fn agent_from_env(variable: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    const SIGNS: [(&str, &str); 3] = [
        ("CLAUDECODE", "claude"),
        ("OPENCODE", "opencode"),
        ("GEMINI_CLI", "gemini"),
    ];
    SIGNS
        .iter()
        .find(|(name, _)| variable(name).is_some_and(|value| !value.is_empty() && value != "0"))
        .map(|(_, agent)| (*agent).to_owned())
}

/// What a store that would not open says, with what to do about it.
fn open_failure(path: &Path, error: &StoreError) -> String {
    match error {
        StoreError::SchemaTooNew { found, supported } => format!(
            "{} was written by a newer {name} (schema {found}; this {name} reads up to {supported}).\n\
             Use the {name} that is running: \"$LEON_BIN\" memory ...",
            path.display(),
            name = product::PRODUCT_NAME,
        ),
        other => format!("cannot open {}: {other}", path.display()),
    }
}

/// The text of an entry from standard input.
fn read_stdin(command: &str) -> Result<String, String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(format!(
            "{command} needs a text (or - to read it from standard input)."
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

/// Runs `leon memory`: `arguments` is what follows the word, `data_dir` the
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

fn carry_out(invocation: Invocation) -> Result<(), String> {
    let variable = |name: &str| std::env::var(name).ok();
    let data_dir = data_dir_of(invocation.data_dir, &variable, product::data_dir);
    let folder = std::env::current_dir()
        .map_err(|error| format!("cannot tell the current folder: {error}"))?;
    let folder = if invocation.action == Action::Mcp {
        mcp_folder(&variable, folder)
    } else {
        folder
    };
    std::fs::create_dir_all(&data_dir)
        .map_err(|error| format!("cannot create {}: {error}", data_dir.display()))?;
    let database = data_dir.join("leon.db");
    let store = Store::open(&database).map_err(|error| open_failure(&database, &error))?;
    let root = memory::resolve_root(&store, &memory::Disk, &folder);
    // Where the project's own files are for this folder: a linked worktree
    // has its own copy of them.
    let files_root = memory::files_root(&memory::Disk, &root, &folder);
    let service = Service::new(store, data_dir.clone(), root);
    if invocation.action == Action::Mcp {
        return serve(service);
    }
    let said = |error: StoreError| memory::sentence(&error);
    let mut out = std::io::stdout().lock();
    // A closed pipe (`| head`) is not an error worth a message.
    let mut print = |text: &str| {
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    };

    match invocation.action {
        Action::Help | Action::Mcp => {}
        Action::Add {
            global,
            kind,
            title,
            agent,
            topic,
            text,
        } => {
            let text = match text {
                Some(text) => text,
                None => read_stdin("add")?,
            };
            let new = NewMemory {
                scope: service.scope(global),
                kind,
                title,
                text,
                agent: agent.or_else(|| agent_from_env(&variable)),
                topic,
            };
            let saved = service.add(&new).map_err(said)?;
            print(&format!("{}\n", render::saved_line(&saved)));
        }
        Action::Edit {
            id,
            kind,
            title,
            topic,
            text,
        } => {
            let text = match text {
                NewText::Keep => None,
                NewText::Stdin => Some(read_stdin("edit")?),
                NewText::Given(text) => Some(text),
            };
            let patch = MemoryPatch {
                kind,
                title,
                text,
                topic,
            };
            let memory = service.update(&id, &patch).map_err(said)?;
            print(&format!(
                "Changed {} (revision {}): {}\n",
                memory.short_id(),
                memory.revision,
                memory.title
            ));
        }
        Action::Show { id, json } => {
            let memory = service.get(&id).map_err(said)?;
            if json {
                print(&format!("{}\n", pretty(&[memory::json_of(&memory, None)])));
            } else {
                print(&render::detail(&memory));
            }
        }
        Action::Pin { id, pinned } => {
            let memory = service.pin(&id, pinned).map_err(said)?;
            let done = if pinned { "Pinned" } else { "Unpinned" };
            print(&format!("{done} {}: {}\n", memory.short_id(), memory.title));
        }
        Action::Restore { id } => {
            let memory = service.restore(&id).map_err(said)?;
            print(&format!(
                "Restored {}: {}\n",
                memory.short_id(),
                memory.title
            ));
        }
        Action::Purge { days } => {
            let gone = service.purge(days).map_err(said)?;
            let entries = if gone == 1 { "entry" } else { "entries" };
            print(&format!(
                "Removed {gone} forgotten {entries} for good (forgotten more than {days} days ago).\n"
            ));
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
                    .map(|hit| memory::json_of(&hit.memory, Some(&hit.snippet)))
                    .collect();
                print(&format!("{}\n", pretty(&values)));
            } else if hits.is_empty() {
                print(&format!("Nothing in the memory matches {words:?}.\n"));
            } else {
                print(&render::hits(&hits));
            }
        }
        Action::List {
            global,
            forgotten,
            limit,
            json,
        } => {
            let entries = if forgotten {
                service.forgotten(global, limit)
            } else {
                service.list(global, limit)
            }
            .map_err(said)?;
            if json {
                let values: Vec<_> = entries
                    .iter()
                    .map(|memory| memory::json_of(memory, None))
                    .collect();
                print(&format!("{}\n", pretty(&values)));
            } else if entries.is_empty() && forgotten {
                print("Nothing is forgotten there.\n");
            } else if entries.is_empty() {
                print("Nothing is saved there yet.\n");
            } else {
                print(&render::listing(&entries));
            }
        }
        Action::Forget { id, hard: true } => {
            let memory = service.delete(&id).map_err(said)?;
            print(&format!(
                "Removed {} for good: {}\n",
                memory.short_id(),
                memory.title
            ));
        }
        Action::Forget { id, hard: false } => {
            let memory = service.forget(&id).map_err(said)?;
            print(&format!(
                "Forgot {}: {} (`{} memory restore {}` brings it back)\n",
                memory.short_id(),
                memory.title,
                product::SLUG,
                memory.short_id()
            ));
        }
        Action::Context => print(&service.document().map_err(said)?),
        Action::Path => {
            // The file an agent is pointed at is there to be read.
            service.refresh(&service.scope(false));
            print(&format!("{}\n", service.file().display()));
        }
        Action::Enable { mcp, dry_run } => {
            let root = files_root;
            if mcp {
                // A `.mcp.json` that will be refused is refused before
                // anything is written: it is all of it or nothing.
                memory::set_mcp_json(&root, true, true)?;
            }
            let mut lines = memory::set_block(&root, true, dry_run)?;
            if mcp {
                lines.extend(memory::set_mcp_json(&root, true, dry_run)?);
            }
            if !dry_run {
                service.refresh(&service.scope(false));
                // Turned on by hand: the window has nothing to ask.
                service.record_choice(leon_core::MemoryChoice::On);
            }
            print(&enabled_report(&lines, mcp, dry_run, &service));
        }
        Action::Disable { dry_run } => {
            let root = files_root;
            memory::set_mcp_json(&root, false, true)?;
            let mut lines = memory::set_block(&root, false, dry_run)?;
            lines.extend(memory::set_mcp_json(&root, false, dry_run)?);
            if !dry_run && !lines.is_empty() {
                service.record_choice(leon_core::MemoryChoice::Off);
            }
            if lines.is_empty() {
                lines.push(format!(
                    "nothing to remove: the memory is not turned on in {}",
                    root.display()
                ));
            }
            print(&format!("{}\n", lines.join("\n")));
        }
        Action::Status => print(&status(&service, &files_root, &variable).map_err(said)?),
    }
    Ok(())
}

fn pretty(values: &[serde_json::Value]) -> String {
    serde_json::to_string_pretty(values).unwrap_or_else(|_| "[]".to_owned())
}

/// What `enable` prints: the files, then how the other agents are told.
fn enabled_report(lines: &[String], mcp: bool, dry_run: bool, service: &Service) -> String {
    let mut out = format!("{}\n", lines.join("\n"));
    if mcp {
        let bin = std::env::current_exe()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| product::SLUG.to_owned());
        out.push_str(&format!(
            "\nClaude Code reads .mcp.json and asks once whether to trust the server.\n\
             The other agents keep their servers in their own settings, which Leon does not edit:\n\n\
             Codex:\n{}\nopencode, {}",
            indented(&mcp_json::codex_lines(&bin)),
            mcp_json::opencode_lines(&bin),
        ));
    }
    if !dry_run {
        out.push_str(&format!(
            "\nThe memory of {} is in {}\n",
            service.root(),
            service.file().display()
        ));
    }
    out
}

fn indented(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                "\n".to_owned()
            } else {
                format!("  {line}\n")
            }
        })
        .collect()
}

/// What `status` prints.
fn status(
    service: &Service,
    root: &Path,
    variable: &dyn Fn(&str) -> Option<String>,
) -> Result<String, StoreError> {
    let (project, global) = service.counts()?;
    let mut out = format!(
        "Project:       {}\n\
         Memory file:   {}\n\
         Global file:   {}\n\
         Entries:       {} of this project ({} pinned, {} forgotten), \
         {} global ({} pinned, {} forgotten)\n\
         Block, in {}:\n",
        service.root(),
        service.file().display(),
        service.global_file().display(),
        project.live,
        project.pinned,
        project.forgotten,
        global.live,
        global.pinned,
        global.forgotten,
        root.display(),
    );
    for (name, found) in memory::look(root) {
        use leon_memory::files::Found;
        let state = match found {
            Found::Missing => "not there".to_owned(),
            Found::File(text) if leon_memory::block::current(&text) => "has the block".to_owned(),
            Found::File(text) if leon_memory::block::present(&text) => {
                "has an older block (enable brings it up to date)".to_owned()
            }
            Found::File(_) => "no block".to_owned(),
            Found::Alias(other) => format!("a link to {other}"),
            Found::Unusable(why) => why,
        };
        out.push_str(&format!("  {name:<11}  {state}\n"));
    }
    let registered = match memory::mcp_registered(root) {
        Ok(true) => "registered in .mcp.json".to_owned(),
        Ok(false) => "not registered in .mcp.json".to_owned(),
        Err(why) => why,
    };
    out.push_str(&format!("MCP:           {registered}\n"));
    out.push_str(&format!(
        "Answer:        {}\n",
        answer_line(service.choice()?)
    ));
    let terminal = match variable(leon_memory::ENV_MEMORY).filter(|file| !file.is_empty()) {
        Some(file) => format!("yes (LEON_MEMORY={file})"),
        None => "no (LEON_MEMORY is not set)".to_owned(),
    };
    out.push_str(&format!("Leon terminal: {terminal}\n"));
    Ok(out)
}

/// What `status` says of the recorded answer about the project's agent
/// memory.
pub fn answer_line(choice: Option<(leon_core::MemoryChoice, i64)>) -> String {
    use leon_core::MemoryChoice;
    match choice {
        None => "none recorded (Leon asks when the project is added in the window)".to_owned(),
        Some((choice, at)) => {
            let what = match choice {
                MemoryChoice::On => "turned on",
                MemoryChoice::Off => "turned off",
                MemoryChoice::Never => "never for this project",
            };
            format!("{what}, {} (Leon does not ask again)", render::day(at))
        }
    }
}

/// One line of standard input, at most [`mcp::MAX_LINE`] bytes of it: what is
/// longer is read to its end and thrown away, and `Err` says so. `None` at
/// the end of the input.
fn read_line(input: &mut impl std::io::BufRead) -> std::io::Result<Option<Result<String, ()>>> {
    let mut bytes = Vec::new();
    let read = input
        .by_ref()
        .take(mcp::MAX_LINE as u64 + 1)
        .read_until(b'\n', &mut bytes)?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.last() != Some(&b'\n') && bytes.len() > mcp::MAX_LINE {
        // Too long: the rest of the line goes unread into nothing.
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
        return Ok(Some(Err(())));
    }
    Ok(Some(Ok(String::from_utf8_lossy(&bytes).into_owned())))
}

/// The MCP conversation over the two pipes given: a line in, at most a line
/// out, until the input ends.
pub fn converse<B: mcp::Backend>(
    server: &mut mcp::Server<B>,
    input: &mut impl std::io::BufRead,
    output: &mut impl std::io::Write,
) -> std::io::Result<()> {
    while let Some(line) = read_line(input)? {
        let answer = match line {
            Ok(line) => server.handle(&line),
            Err(()) => Some(mcp::too_long()),
        };
        if let Some(answer) = answer {
            output.write_all(answer.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

fn serve(service: Service) -> Result<(), String> {
    tracing::info!(root = service.root(), "serving the memory over MCP");
    let mut server = mcp::Server::new(service, product::VERSION);
    converse(
        &mut server,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
    )
    .map_err(|error| format!("the MCP conversation ended: {error}"))
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
    fn the_word_memory_is_taken_with_a_data_folder_before_it() {
        assert_eq!(
            intercept(&strings(&["memory", "list"])),
            Some((None, strings(&["list"])))
        );
        assert_eq!(
            intercept(&strings(&["--data-dir", "/d", "memory"])),
            Some((Some(PathBuf::from("/d")), vec![]))
        );
        assert_eq!(
            intercept(&strings(&["--data-dir=/d", "memory", "add", "x"])),
            Some((Some(PathBuf::from("/d")), strings(&["add", "x"])))
        );
        // Anything else is the window's command line.
        for other in [
            &[][..],
            &["--theme", "dark"],
            &["--theme", "dark", "memory"],
            &["host", "memory"],
            &["--data-dir"],
            &["--data-dir", "memory"],
        ] {
            assert_eq!(intercept(&strings(other)), None, "{other:?}");
        }
    }

    #[test]
    fn add_takes_its_text_as_words_and_its_options_anywhere() {
        assert_eq!(
            parsed(&["add", "Use", "pnpm,", "not npm."]),
            Ok(Action::Add {
                global: false,
                kind: None,
                title: None,
                agent: None,
                topic: None,
                text: Some("Use pnpm, not npm.".into()),
            })
        );
        assert_eq!(
            parsed(&[
                "add",
                "--kind=decision",
                "the text",
                "--global",
                "--title",
                "Money",
                "--agent",
                "Claude",
                "--topic=Auth/Token-Format"
            ]),
            Ok(Action::Add {
                global: true,
                kind: Some(MemoryKind::Decision),
                title: Some("Money".into()),
                agent: Some("claude".into()),
                topic: Some("auth/token-format".into()),
                text: Some("the text".into()),
            })
        );
        // After `--` everything is text.
        assert!(matches!(
            parsed(&["add", "--", "--kind", "is", "an option"]),
            Ok(Action::Add { text: Some(text), kind: None, .. }) if text == "--kind is an option"
        ));
        // No text, or a dash: standard input.
        assert!(matches!(
            parsed(&["add"]),
            Ok(Action::Add { text: None, .. })
        ));
        assert!(matches!(
            parsed(&["add", "-", "--kind", "note"]),
            Ok(Action::Add { text: None, .. })
        ));
    }

    #[test]
    fn search_and_list_take_a_scope_a_limit_and_json() {
        assert_eq!(
            parsed(&["search", "integer", "cents"]),
            Ok(Action::Search {
                global: false,
                limit: DEFAULT_HITS,
                json: false,
                words: "integer cents".into(),
            })
        );
        assert_eq!(
            parsed(&["search", "--json", "--limit", "3", "--global", "x"]),
            Ok(Action::Search {
                global: true,
                limit: 3,
                json: true,
                words: "x".into(),
            })
        );
        assert_eq!(
            parsed(&["list"]),
            Ok(Action::List {
                global: false,
                forgotten: false,
                limit: DEFAULT_LISTED,
                json: false,
            })
        );
        assert_eq!(
            parsed(&["list", "--limit=5", "--json", "--global", "--forgotten"]),
            Ok(Action::List {
                global: true,
                forgotten: true,
                limit: 5,
                json: true,
            })
        );
    }

    #[test]
    fn the_other_commands_parse() {
        assert_eq!(
            parsed(&["forget", "1a2b3c4d"]),
            Ok(Action::Forget {
                id: "1a2b3c4d".into(),
                hard: false
            })
        );
        assert_eq!(
            parsed(&["forget", "--hard", "1a2b3c4d"]),
            Ok(Action::Forget {
                id: "1a2b3c4d".into(),
                hard: true
            })
        );
        assert_eq!(
            parsed(&["show", "1a2b3c4d"]),
            Ok(Action::Show {
                id: "1a2b3c4d".into(),
                json: false
            })
        );
        assert_eq!(
            parsed(&["show", "--json", "1a2b3c4d"]),
            Ok(Action::Show {
                id: "1a2b3c4d".into(),
                json: true
            })
        );
        assert!(parsed(&["show"]).unwrap_err().contains("show needs the id"));
        assert!(parsed(&["show", "--global", "x"])
            .unwrap_err()
            .contains("--global does not go with show"));
        for (word, pinned) in [("pin", true), ("unpin", false)] {
            assert_eq!(
                parsed(&[word, "1a2b3c4d"]),
                Ok(Action::Pin {
                    id: "1a2b3c4d".into(),
                    pinned
                })
            );
        }
        assert_eq!(
            parsed(&["restore", "1a2b3c4d"]),
            Ok(Action::Restore {
                id: "1a2b3c4d".into()
            })
        );
        assert_eq!(
            parsed(&["purge"]),
            Ok(Action::Purge {
                days: memory::KEPT_DAYS
            })
        );
        assert_eq!(
            parsed(&["purge", "--older-than", "7"]),
            Ok(Action::Purge { days: 7 })
        );
        assert_eq!(parsed(&["context"]), Ok(Action::Context));
        assert_eq!(parsed(&["path"]), Ok(Action::Path));
        assert_eq!(parsed(&["mcp"]), Ok(Action::Mcp));
        assert_eq!(parsed(&["status"]), Ok(Action::Status));
        assert_eq!(
            parsed(&["enable"]),
            Ok(Action::Enable {
                mcp: false,
                dry_run: false
            })
        );
        assert_eq!(
            parsed(&["enable", "--mcp", "--dry-run"]),
            Ok(Action::Enable {
                mcp: true,
                dry_run: true
            })
        );
        assert_eq!(
            parsed(&["disable", "--dry-run"]),
            Ok(Action::Disable { dry_run: true })
        );
        for help in [
            &[][..],
            &["--help"],
            &["add", "-h"],
            &["nonsense", "--help"],
        ] {
            assert_eq!(parsed(help), Ok(Action::Help), "{help:?}");
        }
    }

    #[test]
    fn edit_takes_an_id_then_what_to_change_and_keeps_the_rest() {
        assert_eq!(
            parsed(&["edit", "1a2b3c4d", "--kind", "decision"]),
            Ok(Action::Edit {
                id: "1a2b3c4d".into(),
                kind: Some(MemoryKind::Decision),
                title: None,
                topic: None,
                text: NewText::Keep,
            })
        );
        assert_eq!(
            parsed(&["edit", "1a2b3c4d", "--title", "T", "--topic", "A/b", "new", "words"]),
            Ok(Action::Edit {
                id: "1a2b3c4d".into(),
                kind: None,
                title: Some("T".into()),
                topic: Some(Some("a/b".into())),
                text: NewText::Given("new words".into()),
            })
        );
        assert!(matches!(
            parsed(&["edit", "1a2b3c4d", "--no-topic", "-"]),
            Ok(Action::Edit {
                topic: Some(None),
                text: NewText::Stdin,
                ..
            })
        ));
        for (args, expected) in [
            (&["edit"][..], "edit needs the id"),
            (&["edit", "1a2b3c4d"], "edit needs something to change"),
            (
                &["edit", "1a2b3c4d", "--topic", "a", "--no-topic"],
                "do not go together",
            ),
            (
                &["edit", "1a2b3c4d", "--global", "x"],
                "--global does not go with edit",
            ),
            (
                &["edit", "1a2b3c4d", "--topic", "two words"],
                "is not a topic",
            ),
            (&["pin"], "pin needs the id"),
            (&["unpin", "a", "b"], "unpin takes one id"),
            (
                &["restore", "--hard", "a"],
                "--hard does not go with restore",
            ),
            (
                &["purge", "--older-than", "soon"],
                "--older-than needs a number of days",
            ),
            (
                &["purge", "--older-than", "-1"],
                "--older-than needs a number of days",
            ),
            (&["purge", "x"], "purge takes no argument"),
            (&["add", "--topic", "two words", "x"], "is not a topic"),
            (
                &["add", "--no-topic", "x"],
                "--no-topic does not go with add",
            ),
            (
                &["search", "--forgotten", "x"],
                "--forgotten does not go with search",
            ),
        ] {
            let refused = parsed(args).unwrap_err();
            assert!(refused.contains(expected), "{args:?}: {refused}");
        }
    }

    #[test]
    fn what_does_not_fit_is_refused_with_a_sentence() {
        for (args, expected) in [
            (
                &["remember", "x"][..],
                "Unknown memory command \"remember\"",
            ),
            (&["add", "--colour", "x"], "Unknown option \"--colour\""),
            (&["add", "--kind", "rule", "x"], "is not a kind"),
            (&["add", "--kind"], "--kind needs a kind"),
            (&["add", "--agent", "two words", "x"], "is not an agent tag"),
            (&["add", "--json", "x"], "--json does not go with add"),
            (&["search"], "search needs words"),
            (&["search", "--limit", "0", "x"], "--limit needs a number"),
            (
                &["search", "--limit", "many", "x"],
                "--limit needs a number",
            ),
            (&["search", "--mcp", "x"], "--mcp does not go with search"),
            (&["list", "extra"], "list takes no argument"),
            (&["list", "--kind", "note"], "--kind does not go with list"),
            (&["forget"], "forget needs the id"),
            (&["forget", "a", "b"], "forget takes one id"),
            (
                &["forget", "--global", "a"],
                "--global does not go with forget",
            ),
            (&["enable", "--global"], "--global does not go with enable"),
            (&["disable", "--mcp"], "--mcp does not go with disable"),
            (&["status", "x"], "status takes no argument"),
            (&["mcp", "--json"], "--json does not go with mcp"),
            (&["--global"], "Say what to do"),
            (&["--data-dir", ""], "--data-dir needs a path"),
        ] {
            let refused = parsed(args).unwrap_err();
            assert!(refused.contains(expected), "{args:?}: {refused}");
        }
    }

    #[test]
    fn the_data_folder_is_the_one_asked_then_the_terminals_then_the_platforms() {
        let platform = || PathBuf::from("/platform");
        let told = |name: &str| (name == "LEON_DATA_DIR").then(|| "/told".to_owned());
        let untold = |_: &str| None;
        let empty = |_: &str| Some(String::new());
        assert_eq!(
            data_dir_of(Some("/asked".into()), &told, platform),
            PathBuf::from("/asked")
        );
        assert_eq!(data_dir_of(None, &told, platform), PathBuf::from("/told"));
        assert_eq!(
            data_dir_of(None, &untold, platform),
            PathBuf::from("/platform")
        );
        assert_eq!(
            data_dir_of(None, &empty, platform),
            PathBuf::from("/platform")
        );
        // `--data-dir` after the word wins over the one before it.
        let invocation = parse(
            Some("/before".into()),
            strings(&["list", "--data-dir", "/after"]),
        );
        assert_eq!(invocation.unwrap().data_dir, Some(PathBuf::from("/after")));
    }

    #[test]
    fn the_agent_is_read_from_what_agents_set_for_their_commands() {
        let claude = |name: &str| (name == "CLAUDECODE").then(|| "1".to_owned());
        let off = |name: &str| (name == "CLAUDECODE").then(|| "0".to_owned());
        let opencode = |name: &str| (name == "OPENCODE").then(|| "1".to_owned());
        assert_eq!(agent_from_env(&claude).as_deref(), Some("claude"));
        assert_eq!(agent_from_env(&opencode).as_deref(), Some("opencode"));
        assert_eq!(agent_from_env(&off), None);
        assert_eq!(agent_from_env(&|_| None), None);
    }

    #[test]
    fn the_mcp_server_works_for_the_folder_its_client_names() {
        let here = || PathBuf::from("started-here");
        let root = if cfg!(windows) {
            "C:\\code\\api"
        } else {
            "/code/api"
        };
        let named = |name: &str| (name == "CLAUDE_PROJECT_DIR").then(|| root.to_owned());
        let relative = |name: &str| (name == "CLAUDE_PROJECT_DIR").then(|| "api".to_owned());
        assert_eq!(mcp_folder(&named, here()), PathBuf::from(root));
        assert_eq!(mcp_folder(&relative, here()), here());
        assert_eq!(mcp_folder(&|_| None, here()), here());
    }

    #[test]
    fn the_status_says_what_was_answered_about_the_project() {
        use leon_core::MemoryChoice;
        const DAY: i64 = 1_791_331_200_000;
        assert!(answer_line(None).starts_with("none recorded"));
        assert_eq!(
            answer_line(Some((MemoryChoice::On, DAY))),
            "turned on, 2026-10-07 (Leon does not ask again)"
        );
        assert!(answer_line(Some((MemoryChoice::Off, DAY))).starts_with("turned off, "));
        assert!(
            answer_line(Some((MemoryChoice::Never, DAY))).starts_with("never for this project, ")
        );
    }

    #[test]
    fn a_database_from_a_newer_leon_is_explained() {
        let said = open_failure(
            Path::new("/d/leon.db"),
            &StoreError::SchemaTooNew {
                found: 20,
                supported: 14,
            },
        );
        assert!(said.contains("written by a newer Leon"), "{said}");
        assert!(
            said.contains("schema 20") && said.contains("up to 14"),
            "{said}"
        );
        assert!(said.contains("\"$LEON_BIN\" memory"), "{said}");
    }

    #[test]
    fn the_help_names_every_command() {
        let usage = usage();
        for command in [
            "add",
            "edit <id>",
            "show <id> [--json]",
            "search",
            "list",
            "pin <id> | unpin <id>",
            "forget [--hard]",
            "restore <id>",
            "purge [--older-than DAYS]",
            "--topic KEY",
            "--no-topic",
            "--forgotten",
            "context",
            "path",
            "mcp",
            "enable",
            "disable",
            "status",
            "--data-dir",
            "--global",
            "--dry-run",
        ] {
            assert!(usage.contains(command), "{command}");
        }
        assert!(usage.contains("decision, convention, discovery, preference, note"));
    }

    /// A backend that only counts what is asked of it.
    #[derive(Default)]
    struct Quiet {
        searched: usize,
    }

    impl mcp::Backend for Quiet {
        fn search(
            &mut self,
            _: &str,
            _: mcp::Reach,
            _: usize,
        ) -> Result<Vec<leon_core::MemoryHit>, String> {
            self.searched += 1;
            Ok(Vec::new())
        }
        fn add(&mut self, _: mcp::Draft) -> Result<leon_core::SavedMemory, String> {
            Err("read-only".to_owned())
        }
        fn get(&mut self, _: &str) -> Result<leon_core::Memory, String> {
            Err("read-only".to_owned())
        }
        fn update(&mut self, _: &str, _: MemoryPatch) -> Result<leon_core::Memory, String> {
            Err("read-only".to_owned())
        }
        fn pin(&mut self, _: &str, _: bool) -> Result<leon_core::Memory, String> {
            Err("read-only".to_owned())
        }
        fn list(&mut self, _: mcp::Reach, _: usize) -> Result<Vec<leon_core::Memory>, String> {
            Ok(Vec::new())
        }
        fn forget(&mut self, _: &str) -> Result<leon_core::Memory, String> {
            Err("read-only".to_owned())
        }
        fn context(&mut self) -> Result<String, String> {
            Ok("# Leon memory\n".to_owned())
        }
    }

    #[test]
    fn the_conversation_answers_requests_line_by_line_and_nothing_else() {
        let mut server = mcp::Server::new(Quiet::default(), "1.0.0");
        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"x"}}}"#,
            "not json",
        ]
        .join("\n");
        let mut output = Vec::new();
        converse(&mut server, &mut input.as_bytes(), &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        let lines: Vec<serde_json::Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "{output}");
        assert!(output.ends_with('\n'));
        assert_eq!(lines[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(lines[1]["id"], 2);
        assert_eq!(lines[2]["error"]["code"], -32700);
        assert_eq!(server.backend().searched, 1);
    }

    #[test]
    fn a_message_that_is_too_long_is_refused_and_the_next_one_is_still_read() {
        let mut server = mcp::Server::new(Quiet::default(), "1.0.0");
        let input = format!(
            "{{\"id\":1,\"method\":\"ping\",\"pad\":\"{}\"}}\n{}\n",
            "x".repeat(mcp::MAX_LINE * 2),
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#
        );
        let mut output = Vec::new();
        // A small buffer, so the long line is thrown away in pieces.
        let mut reader = std::io::BufReader::with_capacity(4096, input.as_bytes());
        converse(&mut server, &mut reader, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        let lines: Vec<serde_json::Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2, "{output}");
        assert_eq!(lines[0]["error"]["code"], -32600);
        assert_eq!(
            lines[1],
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "result": {} })
        );
    }
}

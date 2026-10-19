//! King Leon, the coordinator, on this computer: its service, its command
//! line and its tools.
//!
//! What can be decided without a process is `leon-king`'s. This module
//! carries it out:
//!
//! * [`serve`]: `leon king serve`, the service that keeps King's
//!   conversation and its workers, runs their turns and listens on the
//!   control socket. It runs without a window; the window, the phone's host
//!   and the command line are its clients;
//! * [`mcp`]: `leon king mcp`, King's tools, an MCP server that the agent
//!   King runs on starts, and that speaks to the service;
//! * [`client`]: one connection to the service, and starting the service
//!   when nothing answers;
//! * [`link`]: the window's line to the service;
//! * this file: where everything lives, and `leon king …` itself.
//!
//! See `docs/KING.md`.

#[cfg(unix)]
pub mod client;
pub mod link;
#[cfg(unix)]
pub mod mcp;
#[cfg(unix)]
pub mod serve;

use std::path::{Path, PathBuf};

use leon_king::coordinator::Target;
use leon_king::protocol::{Request, WorkerRequest};

use crate::memory_cli::data_dir_of;
use crate::product;

/// Where King's things are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Leon's data folder.
    pub data_dir: PathBuf,
    /// King's own folder: its conversation runs here, and its instruction
    /// files are written here.
    pub folder: PathBuf,
    /// The saved state.
    pub state: PathBuf,
    /// `king.toml`.
    pub config: PathBuf,
    /// The control socket.
    pub socket: PathBuf,
    /// The lock that makes one process the service.
    pub lock: PathBuf,
}

impl Paths {
    /// Where King's things are for the data folder `data_dir`, with the
    /// socket under `runtime` (the keeper's rule: `XDG_RUNTIME_DIR` when the
    /// path fits, else a folder of the user's under `/tmp`).
    pub fn new(data_dir: &Path, runtime: &Path) -> Self {
        let key = leon_host::keeper::data_dir_key(data_dir);
        let folder = data_dir.join("king");
        Self {
            data_dir: data_dir.to_path_buf(),
            state: folder.join("state.json"),
            config: data_dir.join(leon_king::config::FILE_NAME),
            socket: runtime.join(format!("{key}.sock")),
            lock: runtime.join(format!("{key}.lock")),
            folder,
        }
    }

    /// [`Self::new`] for this process.
    #[cfg(unix)]
    pub fn here(data_dir: &Path) -> Self {
        let keeper = leon_host::keeper::paths_here(data_dir);
        let base = keeper
            .dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let runtime = base.join(format!("leon-king-{}", leon_host::keeper::our_uid()));
        let data_dir = std::fs::canonicalize(data_dir)
            .or_else(|_| std::path::absolute(data_dir))
            .unwrap_or_else(|_| data_dir.to_path_buf());
        Self::new(&data_dir, &runtime)
    }
}

/// The configuration: `king.toml` over Leon's defaults, or the defaults
/// alone when there is no file.
pub fn read_config(paths: &Paths) -> Result<leon_king::config::Config, String> {
    match std::fs::read_to_string(&paths.config) {
        Ok(text) => leon_king::config::Config::parse(&text)
            .map_err(|error| format!("{}: {error}", paths.config.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(error) => Err(format!("{}: {error}", paths.config.display())),
    }
}

/// `leon [--data-dir <path>] king …`: what follows the word, and the data
/// folder given before it.
pub fn intercept(arguments: &[String]) -> Option<(Option<PathBuf>, Vec<String>)> {
    let mut data_dir = None;
    let mut rest = arguments;
    loop {
        match rest {
            [first, tail @ ..] if first == "king" => return Some((data_dir, tail.to_vec())),
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

/// What `leon king` says to `--help`.
pub fn usage() -> String {
    "King Leon: one agent that answers for every project and runs other agents.\n\n\
     Usage:\n  \
       leon king say <words…>        say something to King and print its answer\n  \
       leon king status              King and its workers\n  \
       leon king log [worker]        what King, or a worker, said lately\n  \
       leon king new                 put King's conversation away and start a new one\n  \
       leon king history [n]         King's past conversations, or what one said\n  \
       leon king resume <n>          take a past conversation up again\n  \
       leon king start <title> --prompt <text> [--project <name>] [--tier <tier>]\n  \
       \x20                            [--agent <agent> --model <model>] [--research | --plan]\n  \
       leon king tell <worker> <words…>\n  \
       leon king stop <worker>\n  \
       leon king release <worker>    take a worker over: King lets it go\n  \
       leon king answer [--worker <n>] <question> <choice> [reason…]\n  \
       leon king brain <agent> [model] [effort]\n  \
       leon king config [--defaults] the configuration in force (king.toml over the defaults)\n  \
       leon king serve               run the service in the foreground\n  \
       leon king mcp                 King's tools, for the agent King runs on\n\n\
     The service starts by itself when a command needs it. Its settings are\n\
     king.toml in Leon's data folder; see docs/KING.md.\n"
        .to_owned()
}

/// What a command of `leon king` asks.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    Serve,
    Mcp,
    Help,
    /// A request, and whether to wait for King's answer after it.
    Ask(Request, bool),
    Log(Target),
    /// Print the configuration; Leon's defaults alone with `true`.
    Config(bool),
}

fn parse(arguments: &[String]) -> Result<Command, String> {
    let words = |from: &[String]| from.join(" ");
    let number = |text: Option<&String>| -> Result<u32, String> {
        text.and_then(|text| text.trim_start_matches('#').parse().ok())
            .ok_or_else(|| "name the worker by its number".to_owned())
    };
    let Some((first, rest)) = arguments.split_first() else {
        return Ok(Command::Help);
    };
    Ok(match first.as_str() {
        "serve" => Command::Serve,
        "config" => match rest {
            [] => Command::Config(false),
            [flag] if flag == "--defaults" => Command::Config(true),
            _ => return Err("config takes only --defaults".to_owned()),
        },
        "mcp" => Command::Mcp,
        "help" | "--help" | "-h" => Command::Help,
        "status" => Command::Ask(Request::Status, false),
        "new" => Command::Ask(Request::NewConversation, false),
        "history" => match rest.first() {
            None => Command::Ask(Request::Conversations, false),
            Some(id) => Command::Ask(
                Request::PastConversation {
                    id: id
                        .parse()
                        .map_err(|_| "name the conversation by its number")?,
                },
                false,
            ),
        },
        "resume" => Command::Ask(
            Request::ResumeConversation {
                id: rest
                    .first()
                    .and_then(|id| id.parse().ok())
                    .ok_or("name the conversation by its number")?,
            },
            false,
        ),
        "say" if !rest.is_empty() => Command::Ask(Request::Say { text: words(rest) }, true),
        "log" => match rest.first() {
            None => Command::Log(Target::Brain),
            some => Command::Log(Target::Worker(number(some)?)),
        },
        "tell" if rest.len() >= 2 => Command::Ask(
            Request::TellWorker {
                worker: number(rest.first())?,
                text: words(&rest[1..]),
            },
            false,
        ),
        "stop" => Command::Ask(
            Request::StopWorker {
                worker: number(rest.first())?,
            },
            false,
        ),
        "release" => Command::Ask(
            Request::Release {
                worker: number(rest.first())?,
            },
            false,
        ),
        "brain" if !rest.is_empty() => Command::Ask(
            Request::SetBrain {
                agent: rest[0].clone(),
                model: rest.get(1).cloned(),
                effort: rest.get(2).cloned(),
                mode: None,
            },
            false,
        ),
        "start" => Command::Ask(Request::StartWorker(Box::new(start_request(rest)?)), false),
        "answer" => {
            let (target, rest) = match rest {
                [flag, worker, tail @ ..] if flag == "--worker" => {
                    (Target::Worker(number(Some(worker))?), tail)
                }
                _ => (Target::Brain, rest),
            };
            let [question, choice, reason @ ..] = rest else {
                return Err("answer takes the question's id and a choice".to_owned());
            };
            Command::Ask(
                Request::Answer {
                    target,
                    question: question.clone(),
                    reply: leon_king::coordinator::Reply {
                        choice: Some(choice.clone()),
                        approve: match choice.as_str() {
                            "yes" | "approve" => Some(true),
                            "no" | "reject" => Some(false),
                            _ => None,
                        },
                        text: (!reason.is_empty()).then(|| words(reason)),
                    },
                },
                false,
            )
        }
        other => return Err(format!("unknown command {other:?}\n\n{}", usage())),
    })
}

fn start_request(arguments: &[String]) -> Result<WorkerRequest, String> {
    let mut request = WorkerRequest::default();
    let mut title = Vec::new();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let mut value = |name: &str| {
            rest.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match argument.as_str() {
            "--prompt" => request.prompt = value("--prompt")?,
            "--project" => request.project = Some(value("--project")?),
            "--branch" => request.branch = Some(value("--branch")?),
            "--cwd" => request.cwd = Some(value("--cwd")?),
            "--tier" => request.tier = Some(value("--tier")?),
            "--agent" => request.agent = Some(value("--agent")?),
            "--model" => request.model = Some(value("--model")?),
            "--effort" => request.effort = Some(value("--effort")?),
            "--mode" => request.mode = Some(value("--mode")?),
            "--research" => request.job = leon_king::coordinator::Job::Research,
            "--plan" => request.job = leon_king::coordinator::Job::Plan,
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            word => title.push(word.to_owned()),
        }
    }
    request.title = title.join(" ");
    if request.title.is_empty() || request.prompt.is_empty() {
        return Err("a worker needs a title and --prompt".to_owned());
    }
    Ok(request)
}

/// Runs `leon king …`.
pub fn run(data_dir: Option<PathBuf>, arguments: Vec<String>) -> i32 {
    let command = match parse(&arguments) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let variable = |name: &str| std::env::var(name).ok();
    let data_dir = data_dir_of(data_dir, &variable, product::data_dir);
    #[cfg(not(unix))]
    {
        let _ = (command, data_dir);
        eprintln!(
            "{} king: King's service runs on macOS and Linux only, for now.",
            product::SLUG
        );
        1
    }
    #[cfg(unix)]
    {
        let paths = Paths::here(&data_dir);
        let outcome = match command {
            Command::Help => {
                print!("{}", usage());
                Ok(())
            }
            Command::Serve => serve::run(paths),
            Command::Mcp => mcp::run(&paths),
            Command::Ask(request, wait) => client::ask_and_print(&paths, request, wait),
            Command::Log(target) => client::print_log(&paths, target),
            Command::Config(true) => {
                print!("{}", leon_king::config::DEFAULTS);
                Ok(())
            }
            Command::Config(false) => read_config(&paths).map(|config| {
                println!("# {} over Leon's defaults", paths.config.display());
                print!("{}", config.to_toml());
            }),
        };
        match outcome {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{} king: {error}", product::SLUG);
                1
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn the_word_king_is_taken_with_a_data_folder_before_it() {
        let (dir, rest) = intercept(&strings(&["--data-dir", "/d", "king", "status"])).unwrap();
        assert_eq!(dir, Some(PathBuf::from("/d")));
        assert_eq!(rest, ["status"]);
        assert!(intercept(&strings(&["memory", "list"])).is_none());
    }

    #[test]
    fn commands_read_as_requests() {
        assert_eq!(
            parse(&strings(&["say", "what", "is", "up?"])).unwrap(),
            Command::Ask(
                Request::Say {
                    text: "what is up?".into()
                },
                true
            )
        );
        assert_eq!(
            parse(&strings(&["log", "#3"])).unwrap(),
            Command::Log(Target::Worker(3))
        );
        assert_eq!(
            parse(&strings(&["tell", "2", "add", "a", "test"])).unwrap(),
            Command::Ask(
                Request::TellWorker {
                    worker: 2,
                    text: "add a test".into()
                },
                false
            )
        );
        let Command::Ask(Request::StartWorker(start), _) = parse(&strings(&[
            "start",
            "fix",
            "login",
            "--prompt",
            "fix it",
            "--project",
            "leon",
            "--tier",
            "cheap",
            "--research",
        ]))
        .unwrap() else {
            panic!();
        };
        assert_eq!(start.title, "fix login");
        assert_eq!(start.project.as_deref(), Some("leon"));
        assert_eq!(start.job, leon_king::coordinator::Job::Research);
        let Command::Ask(Request::Answer { target, reply, .. }, _) = parse(&strings(&[
            "answer", "--worker", "2", "q1", "deny", "not", "now",
        ]))
        .unwrap() else {
            panic!();
        };
        assert_eq!(target, Target::Worker(2));
        assert_eq!(reply.text.as_deref(), Some("not now"));
        assert!(parse(&strings(&["start", "x"])).is_err());
        assert!(parse(&strings(&["stop"])).is_err());
        assert!(parse(&strings(&["dance"])).is_err());
        assert_eq!(parse(&[]).unwrap(), Command::Help);
        assert_eq!(
            parse(&strings(&["config", "--defaults"])).unwrap(),
            Command::Config(true)
        );
    }

    #[test]
    fn the_socket_is_named_after_the_data_folder() {
        let paths = Paths::new(
            Path::new("/data"),
            Path::new("/run/user/1000/leon-king-1000"),
        );
        assert_eq!(paths.folder, PathBuf::from("/data/king"));
        assert_eq!(paths.config, PathBuf::from("/data/king.toml"));
        assert!(paths.socket.starts_with("/run/user/1000/leon-king-1000"));
        let other = Paths::new(Path::new("/other"), Path::new("/run"));
        assert_ne!(paths.socket.file_name(), other.socket.file_name());
    }
}

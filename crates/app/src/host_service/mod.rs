//! `leon host service`: the host as a background service of the user's own
//! session.
//!
//! ```text
//! leon host service install [--linger] [--relay <url>] [--name <name>] [--data-dir <path>]
//! leon host service uninstall
//! leon host service status
//! leon host service logs [--lines <n>]
//! ```
//!
//! On Linux the service is a systemd **user** unit written under the user's
//! systemd directory and driven with `systemctl --user`; on macOS it is a
//! launchd agent in `~/Library/LaunchAgents`. Windows is not supported yet and
//! says so. Nothing here needs or accepts root: the units belong to the person
//! who runs the command, and every device that pairs with the host gets that
//! person's rights, not more.
//!
//! The split follows the rest of the code. [`unit`] writes the text of the
//! definitions, [`plan`] decides the files to write and the commands to run,
//! [`status`] reads what the system manager says and puts it in words; all of
//! them are pure and tested. This file carries a plan out: the commands go
//! through `leon_remote::Runner` (the one place a process is started), so the
//! tests script the answers of `systemctl` and `launchctl` and never touch the
//! real ones, and the files go to whatever directories the [`Environment`]
//! names.
//!
//! The host itself is `leon_host::cli`; this command only installs and watches
//! it. Uninstalling leaves the pairings, the identity and the settings alone.

mod plan;
mod status;
mod unit;

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use leon_host::cli::{running_status, Paths};
use leon_link::DeviceRegistry;
use leon_remote::{ProcessRunner, RunError, Runner};

use crate::platform;
use crate::product;
use plan::{Action, Layout, Os, Plan};
use status::{Activity, Devices, Heartbeat, Manager, Report};
use unit::Service;

/// What was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verb {
    /// Write the definition and start the service.
    Install {
        /// Also keep the user's manager running after the last logout.
        linger: bool,
    },
    /// Stop the service and remove its definition.
    Uninstall,
    /// Say what it is doing.
    Status,
    /// Print the recent log.
    Logs {
        /// How many lines.
        lines: usize,
    },
    /// Print the usage.
    Help,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    verb: Verb,
    data_dir: Option<PathBuf>,
    relay: Option<String>,
    name: Option<String>,
}

const DEFAULT_LOG_LINES: usize = 100;
const MOST_LOG_LINES: usize = 5000;
/// The most of a log file read to show its last lines.
const LOG_TAIL_BYTES: u64 = 256 * 1024;
/// How long a service manager's command may take.
const COMMAND_TIME_LIMIT: Duration = Duration::from_secs(30);
/// How long `install` waits after starting the service before it asks the
/// manager whether the host is still up: a host that cannot start (a port, a
/// folder, a second copy) ends within this.
const SETTLE_TIME: Duration = Duration::from_secs(2);

const UNSUPPORTED: &str = "\
Running the host as a background service is not supported on this system yet.
It works on Linux (a systemd user unit) and macOS (a launchd agent).
Run `leon host` in a terminal instead.";

const NOT_ROOT: &str = "\
The host service is installed for your own user and never as root: every device
you pair would otherwise get a root shell. Run this command as yourself.";

const SAFETY: &str = "\
The service runs as you and gives every device you pair a full terminal as you
on this computer. Pair only your own devices, and revoke any you lose
(`leon host revoke`).";

fn usage() -> &'static str {
    "Usage: leon host service install [--linger] [--relay <url>] [--name <name>] [--data-dir <path>]\n       \
     leon host service uninstall | status\n       \
     leon host service logs [--lines <n>]\n\n\
     Runs the host in the background of your own session: a systemd user unit on\n\
     Linux, a launchd agent on macOS. It never needs root.\n\n\
     Options:\n  \
     --linger          (install, Linux) keep the host running when you are logged out:\n                    \
     runs `loginctl enable-linger` for your own user\n  \
     --relay <url>     (install) the relay the host uses (default wss://relay.getleon.dev,\n                    \
     or LEON_RELAY_URL when you install)\n  \
     --name <name>     (install) the name your devices see\n  \
     --data-dir <path> Where the host keeps its files (default: Leon's data folder)\n  \
     --lines <n>       (logs) how many lines to show (default 100)\n  \
     -h, --help        Print this help"
}

/// Parses the arguments after `host service`.
fn parse(args: impl IntoIterator<Item = String>) -> Result<Request, String> {
    let mut request = Request {
        verb: Verb::Help,
        data_dir: None,
        relay: None,
        name: None,
    };
    let mut positional: Vec<String> = Vec::new();
    let mut linger = false;
    let mut lines = None;
    let mut help = false;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
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
                .ok_or_else(|| format!("{name} needs {what}"))
        };
        match name.as_str() {
            "-h" | "--help" => help = true,
            "--linger" => linger = true,
            "--relay" => request.relay = Some(value("a relay URL")?),
            "--name" => request.name = Some(value("a name")?),
            "--data-dir" => request.data_dir = Some(PathBuf::from(value("a path")?)),
            "--lines" | "-n" => {
                let text = value("a number")?;
                lines = Some(
                    text.parse::<usize>()
                        .ok()
                        .filter(|n| (1..=MOST_LOG_LINES).contains(n))
                        .ok_or_else(|| {
                            format!("{name} takes a number from 1 to {MOST_LOG_LINES}.")
                        })?,
                );
            }
            "--allow-root" => {
                return Err("The service never runs as root; there is no --allow-root.".into())
            }
            other if other.starts_with('-') => return Err(format!("Unknown option {other:?}.")),
            _ => positional.push(argument),
        }
    }
    if help {
        return Ok(request);
    }
    request.verb = match positional.as_slice() {
        [word] if word == "install" => Verb::Install { linger },
        [word] if word == "uninstall" => Verb::Uninstall,
        [word] if word == "status" => Verb::Status,
        [word] if word == "logs" => Verb::Logs {
            lines: lines.unwrap_or(DEFAULT_LOG_LINES),
        },
        [] => return Err("Say what to do: install, uninstall, status or logs.".into()),
        [other, ..] => return Err(format!("Unknown command {other:?}.")),
    };
    let installing = matches!(request.verb, Verb::Install { .. });
    if !installing && (request.relay.is_some() || request.name.is_some()) {
        return Err("--relay and --name are written into the service when it is installed; give them to `install`.".into());
    }
    if linger && !installing {
        return Err("--linger belongs to `install`.".into());
    }
    if lines.is_some() && !matches!(request.verb, Verb::Logs { .. }) {
        return Err("--lines belongs to `logs`.".into());
    }
    Ok(request)
}

/// The system, from the platform's answers: a service exists for the two with
/// a user-level manager Leon knows.
fn os_of(windows: bool, mac: bool, unix: bool) -> Option<Os> {
    if windows {
        None
    } else if mac {
        Some(Os::MacOs)
    } else if unix {
        Some(Os::Linux)
    } else {
        None
    }
}

/// What the command needs to know about the computer and the session.
struct Environment {
    layout: Layout,
    /// The numeric id of the user (launchd's domain, `loginctl`).
    uid: u32,
    /// The program that is running.
    exe: PathBuf,
    /// Leon's data folder.
    data_dir: PathBuf,
    /// `PATH` of this session.
    path_env: Option<String>,
    /// `LEON_RELAY_URL` of this session.
    relay_env: Option<String>,
    /// Now, in seconds since the epoch.
    now: u64,
    /// How long `install` lets the service run before it checks it is up.
    settle: Duration,
}

/// What a command printed and how it ended.
#[derive(Debug, Default, PartialEq, Eq)]
struct Outcome {
    code: i32,
    out: String,
    err: String,
}

impl Outcome {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            out: String::new(),
            err: format!("{}\n", message.into()),
        }
    }
}

/// A hint for the failures people meet most.
fn hint(stderr: &str) -> &'static str {
    if stderr.contains("Failed to connect to bus") || stderr.contains("No medium found") {
        "\nsystemd's user manager is not reachable from here. Run this in a login session (on the computer, or an ssh login) so that XDG_RUNTIME_DIR is set."
    } else {
        ""
    }
}

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it gave no reason")
}

fn command_text(command: &leon_remote::CommandSpec) -> String {
    std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What went wrong with a command, in a sentence, or `None` when it worked.
fn problem_of(
    command: &leon_remote::CommandSpec,
    result: &Result<leon_remote::Output, RunError>,
) -> Option<String> {
    match result {
        Ok(output) if output.success() => None,
        Ok(output) => Some(format!(
            "`{}` failed ({}): {}{}",
            command_text(command),
            output
                .status
                .map_or("ended by a signal".to_owned(), |s| format!("status {s}")),
            first_line(&output.stderr),
            hint(&output.stderr),
        )),
        Err(RunError::Spawn { program, source }) => Some(format!(
            "cannot run {program}: {source}. Is it installed on this computer?"
        )),
        Err(error) => Some(error.to_string()),
    }
}

/// Carries a plan out. Returns what it printed, or the message of the first
/// required step that failed. A step that is not required and fails is a note.
async fn execute(plan: &Plan, runner: &impl Runner) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    for action in &plan.actions {
        match action {
            Action::MakeDir(dir) => std::fs::create_dir_all(dir)
                .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?,
            Action::Write { path, text } => std::fs::write(path, text)
                .map_err(|e| format!("Cannot write {}: {e}", path.display()))?,
            Action::Remove(path) => match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Cannot remove {}: {e}", path.display())),
            },
            Action::Run {
                why,
                command,
                required,
            } => {
                let Some(problem) = problem_of(command, &runner.run(command).await) else {
                    continue;
                };
                if *required {
                    return Err(format!("Could not {why}: {problem}"));
                }
                notes.push(format!("Could not {why}: {problem}"));
            }
        }
    }
    Ok(notes)
}

/// What the system manager says the service is doing now.
async fn ask_manager(environment: &Environment, runner: &impl Runner) -> Manager {
    let layout = &environment.layout;
    let asked = runner.run(&plan::probe(layout, environment.uid)).await;
    match layout.os {
        Os::Linux => status::parse_systemd(&asked),
        Os::MacOs => status::parse_launchd(&asked),
    }
}

async fn install(
    request: &Request,
    linger: bool,
    environment: &Environment,
    runner: &impl Runner,
) -> Outcome {
    let layout = &environment.layout;
    let exe = unit::normalise_exe(&environment.exe);
    if let Some(why) = unit::unstable_install(&exe) {
        return Outcome::failed(format!(
            "Not installing {}: {why}. Install Leon where it stays, then run this again.",
            exe.display()
        ));
    }
    let data_dir = request
        .data_dir
        .clone()
        .unwrap_or_else(|| environment.data_dir.clone());
    let service = Service {
        exe: exe.clone(),
        data_dir: data_dir.clone(),
        relay: request
            .relay
            .clone()
            .or_else(|| environment.relay_env.clone()),
        name: request.name.clone(),
        path_env: environment.path_env.clone(),
    };
    if let Err(why) = service.validate() {
        return Outcome::failed(format!("Not installing: {why}"));
    }
    if !data_dir.is_absolute() {
        return Outcome::failed(
            "Not installing: --data-dir must be an absolute path, because the service starts elsewhere.",
        );
    }
    let plan = plan::install(&service, layout, environment.uid, linger);
    let mut notes = match execute(&plan, runner).await {
        Ok(notes) => notes,
        Err(message) => {
            return Outcome::failed(format!(
                "{message}\nThe definition is at {}; fix the cause and run install again, or run `leon host service uninstall`.",
                layout.definition.display()
            ))
        }
    };
    // Starting only hands the host to the manager; whether it stays up is a
    // different question, so ask it.
    if !environment.settle.is_zero() {
        tokio::time::sleep(environment.settle).await;
    }
    match ask_manager(environment, runner).await.activity {
        Activity::Running => {}
        Activity::Unavailable(why) => {
            notes.push(format!("Could not confirm that the host is running: {why}"));
        }
        down => {
            let notes: String = notes.iter().map(|note| format!("{note}\n")).collect();
            return Outcome::failed(format!(
                "{notes}Installed, but the service is not running: it is {}.\nSee why with `leon host service logs`. The definition is at {}; fix the cause and run install again.",
                down.describe(),
                layout.definition.display()
            ));
        }
    }
    let mut out = format!(
        "Installed. {} runs `leon host` as you, starts at every login and restarts after a failure.\nDefinition: {}\n",
        exe.display(),
        layout.definition.display()
    );
    if unit::is_development_build(&exe) {
        out.push_str("Note: this is a development build; the service runs this file, so rebuilding replaces it.\n");
    }
    if let Some(why) = unit::versioned_install(&exe) {
        out.push_str(&format!(
            "Note: {why}. The service keeps running this version until you run `leon host service install` again after an update.\n"
        ));
    }
    out.push_str(&format!(
        "Files of the host: {}\n\n{SAFETY}\n",
        data_dir.display()
    ));
    if layout.os == Os::Linux {
        let lingering_failed = notes.iter().any(|n| n.contains("logged out"));
        if linger && !lingering_failed {
            out.push_str("\nLingering is on: the host starts at boot and keeps running when you log out. Uninstalling leaves it on; `loginctl disable-linger` turns it off.\n");
        } else if !linger {
            out.push_str("\nWithout lingering, systemd stops your services when you log out of your last session and starts the host again at your next login. To keep it running while you are logged out, run `leon host service install --linger` (it runs `loginctl enable-linger` for your own user; no root).\n");
        }
    }
    out.push_str("\nNext: `leon host pair` prints a code for a device; `leon host service status` shows how it is doing.\n");
    let err: String = notes.iter().map(|note| format!("{note}\n")).collect();
    Outcome { code: 0, out, err }
}

async fn uninstall(environment: &Environment, runner: &impl Runner) -> Outcome {
    let layout = &environment.layout;
    let was_installed = layout.definition.exists();
    let mut notes = Vec::new();
    // The definition goes only when nothing is left running without it: a host
    // that keeps running with no unit file is out of reach of `status` and of
    // a second `uninstall`.
    let stop = plan::stop(layout, environment.uid);
    let stopped = match problem_of(&stop, &runner.run(&stop).await) {
        None => true,
        Some(problem) => {
            let activity = ask_manager(environment, runner).await.activity;
            if activity.is_down() {
                // It was not loaded or not running: there was nothing to stop.
                true
            } else if was_installed || !matches!(activity, Activity::Unavailable(_)) {
                let kept = if was_installed {
                    format!(
                        "The definition at {} is kept, so that the service is not left running without one. Fix the cause and run `leon host service uninstall` again.",
                        layout.definition.display()
                    )
                } else {
                    "There is no definition to keep; fix the cause and run `leon host service uninstall` again.".to_owned()
                };
                return Outcome::failed(format!(
                    "Could not stop the service: {problem}\nThe system manager says it is {}. {kept}",
                    activity.describe()
                ));
            } else {
                notes.push(format!(
                    "Could not tell whether a host is still running: {problem}"
                ));
                false
            }
        }
    };
    match execute(&plan::remove(layout), runner).await {
        Ok(more) => notes.extend(more),
        Err(message) => return Outcome::failed(message),
    }
    let mut out = if was_installed {
        format!(
            "Uninstalled: the service is stopped and {} is removed.\n",
            layout.definition.display()
        )
    } else if stopped {
        "The service was not installed; nothing to remove.\n".to_owned()
    } else {
        "The service was not installed; nothing to remove, and a host may still be running.\n"
            .to_owned()
    };
    out.push_str(&format!(
        "Your paired devices, the identity and the settings are untouched (in {}).\n",
        environment.data_dir.display()
    ));
    if layout.os == Os::Linux {
        out.push_str(
            "If you turned lingering on, it stays on; `loginctl disable-linger` turns it off.\n",
        );
    } else if let Some(log) = &layout.log {
        out.push_str(&format!("The log stays at {}.\n", log.display()));
    }
    let err = notes.iter().map(|note| format!("{note}\n")).collect();
    Outcome { code: 0, out, err }
}

fn paired_devices(paths: &Paths) -> Result<Devices, String> {
    let registry = DeviceRegistry::open(&paths.devices()).map_err(|e| e.to_string())?;
    let revoked = registry.devices().iter().filter(|d| d.revoked).count();
    Ok(Devices {
        active: registry.devices().len() - revoked,
        revoked,
    })
}

async fn status(request: &Request, environment: &Environment, runner: &impl Runner) -> Outcome {
    let layout = &environment.layout;
    let mut manager = ask_manager(environment, runner).await;
    if let Some(probe) = plan::enabled_probe(layout, environment.uid) {
        manager.enabled = status::parse_launchd_disabled(&runner.run(&probe).await, &layout.label);
    }
    let linger = match plan::linger_probe(layout, environment.uid) {
        Some(probe) => status::parse_linger(&runner.run(&probe).await),
        None => None,
    };
    let paths = Paths::new(request.data_dir.as_ref().unwrap_or(&environment.data_dir));
    let report = Report {
        os: layout.os,
        definition: layout.definition.clone(),
        installed: layout.definition.exists(),
        manager,
        linger,
        heartbeat: running_status(&paths).map(|map| Heartbeat::from_map(&map)),
        devices: paired_devices(&paths),
        now: environment.now,
    };
    Outcome {
        code: if report.running() { 0 } else { 1 },
        out: status::render(&report),
        err: String::new(),
    }
}

/// The last `count` lines of `text`.
fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let from = lines.len().saturating_sub(count);
    lines[from..]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

/// The end of a file, at most [`LOG_TAIL_BYTES`] of it, without a cut line at
/// its start.
fn read_tail(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(LOG_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(LOG_TAIL_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(match (start, text.split_once('\n')) {
        (0, _) | (_, None) => text,
        (_, Some((_, rest))) => rest.to_owned(),
    })
}

async fn logs(lines: usize, environment: &Environment, runner: &impl Runner) -> Outcome {
    let layout = &environment.layout;
    if let Some(command) = plan::logs(layout, lines) {
        return match runner.run(&command).await {
            Ok(output) if output.success() => Outcome {
                code: 0,
                out: output.stdout,
                err: "Live: journalctl --user --follow --unit leon-host.service\n".into(),
            },
            Ok(output) => Outcome::failed(format!(
                "journalctl failed: {}{}",
                first_line(&output.stderr),
                hint(&output.stderr)
            )),
            Err(error) => Outcome::failed(format!("cannot read the journal: {error}")),
        };
    }
    let Some(log) = &layout.log else {
        return Outcome::failed("This system keeps no log of the service.");
    };
    match read_tail(log) {
        Ok(text) => Outcome {
            code: 0,
            out: last_lines(&text, lines),
            err: format!("Live: tail -f {}\n", log.display()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Outcome::failed(format!(
            "No log yet: the service writes {} once it has run.",
            log.display()
        )),
        Err(e) => Outcome::failed(format!("Cannot read {}: {e}", log.display())),
    }
}

/// Does what was asked.
async fn perform(request: &Request, environment: &Environment, runner: &impl Runner) -> Outcome {
    match request.verb {
        Verb::Install { linger } => install(request, linger, environment, runner).await,
        Verb::Uninstall => uninstall(environment, runner).await,
        Verb::Status => status(request, environment, runner).await,
        Verb::Logs { lines } => logs(lines, environment, runner).await,
        Verb::Help => Outcome {
            code: 0,
            out: format!("{}\n", usage()),
            err: String::new(),
        },
    }
}

fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Runs `leon host service ...`. Returns the process exit code.
pub fn run(args: Vec<String>, default_data_dir: PathBuf) -> i32 {
    let request = match parse(args) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("{message}\n\n{}", usage());
            return 2;
        }
    };
    if request.verb == Verb::Help {
        println!("{}", usage());
        return 0;
    }
    let Some(os) = os_of(platform::is_windows(), platform::is_mac(), cfg!(unix)) else {
        eprintln!("{UNSUPPORTED}");
        return 1;
    };
    if leon_host::running_as_root() {
        eprintln!("{NOT_ROOT}");
        return 1;
    }
    let (Some(home), Some(config_dir)) = (dirs::home_dir(), dirs::config_dir()) else {
        eprintln!("Cannot find your home directory.");
        return 1;
    };
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("Cannot tell which program is running.");
        return 1;
    };
    let environment = Environment {
        layout: Layout::new(os, &home, &config_dir, product::APP_ID),
        uid: current_uid(),
        exe,
        data_dir: default_data_dir,
        path_env: std::env::var("PATH").ok().filter(|path| !path.is_empty()),
        relay_env: std::env::var("LEON_RELAY_URL")
            .ok()
            .filter(|url| !url.is_empty()),
        now: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        settle: SETTLE_TIME,
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cannot start the runtime: {error}");
            return 1;
        }
    };
    let runner = ProcessRunner::with_time_limit(COMMAND_TIME_LIMIT);
    let outcome = runtime.block_on(perform(&request, &environment, &runner));
    print!("{}", outcome.out);
    eprint!("{}", outcome.err);
    outcome.code
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_remote::{Output, ScriptedRunner};

    fn p(args: &[&str]) -> Result<Request, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn the_four_commands_are_recognised() {
        assert_eq!(
            p(&["install"]).unwrap().verb,
            Verb::Install { linger: false }
        );
        assert_eq!(
            p(&["install", "--linger"]).unwrap().verb,
            Verb::Install { linger: true }
        );
        assert_eq!(p(&["uninstall"]).unwrap().verb, Verb::Uninstall);
        assert_eq!(p(&["status"]).unwrap().verb, Verb::Status);
        assert_eq!(
            p(&["logs"]).unwrap().verb,
            Verb::Logs {
                lines: DEFAULT_LOG_LINES
            }
        );
        assert_eq!(
            p(&["logs", "--lines=20"]).unwrap().verb,
            Verb::Logs { lines: 20 }
        );
        assert_eq!(p(&["--help"]).unwrap().verb, Verb::Help);
        assert_eq!(p(&["install", "-h"]).unwrap().verb, Verb::Help);
    }

    #[test]
    fn options_that_do_not_belong_to_a_command_are_refused() {
        assert!(p(&[]).is_err());
        assert!(p(&["frobnicate"]).is_err());
        assert!(p(&["status", "--linger"]).is_err());
        assert!(p(&["status", "--relay", "wss://x"]).is_err());
        assert!(p(&["uninstall", "--name", "x"]).is_err());
        assert!(p(&["install", "--lines", "5"]).is_err());
        assert!(p(&["logs", "--lines", "0"]).is_err());
        assert!(p(&["logs", "--lines", "100000"]).is_err());
        assert!(p(&["logs", "--lines", "many"]).is_err());
        assert!(p(&["install", "--nope"]).is_err());
        assert!(p(&["install", "--relay"]).is_err());
    }

    #[test]
    fn the_service_never_accepts_root() {
        let error = p(&["install", "--allow-root"]).unwrap_err();
        assert!(error.contains("never runs as root"));
    }

    #[test]
    fn install_takes_the_relay_the_name_and_the_data_directory() {
        let request = p(&[
            "install",
            "--relay=wss://r",
            "--name",
            "box",
            "--data-dir",
            "/srv/leon",
        ])
        .unwrap();
        assert_eq!(request.relay.as_deref(), Some("wss://r"));
        assert_eq!(request.name.as_deref(), Some("box"));
        assert_eq!(request.data_dir, Some(PathBuf::from("/srv/leon")));
    }

    #[test]
    fn windows_has_no_service_yet_and_says_so() {
        assert_eq!(os_of(true, false, false), None);
        assert_eq!(os_of(false, true, true), Some(Os::MacOs));
        assert_eq!(os_of(false, false, true), Some(Os::Linux));
        assert_eq!(os_of(false, false, false), None);
        assert!(UNSUPPORTED.contains("not supported"));
        assert!(UNSUPPORTED.contains("`leon host`"));
    }

    fn environment(root: &Path, os: Os) -> Environment {
        let home = root.join("home");
        Environment {
            layout: Layout::new(os, &home, &home.join(".config"), "dev.zavu.leon"),
            uid: 1000,
            exe: PathBuf::from("/usr/bin/leon"),
            data_dir: root.join("data"),
            path_env: Some("/usr/bin".into()),
            relay_env: None,
            now: 10_000,
            settle: Duration::ZERO,
        }
    }

    /// What `systemctl show` prints for a unit that is up.
    const UP: &str = "LoadState=loaded\nActiveState=active\nSubState=running\nUnitFileState=enabled\nMainPID=99\n";
    /// And for one that started, ended and waits to be restarted.
    const RESTARTING: &str = "LoadState=loaded\nActiveState=activating\nSubState=auto-restart\nUnitFileState=enabled\nMainPID=0\n";
    /// And for one the manager has never heard of.
    const UNKNOWN: &str = "LoadState=not-found\nActiveState=inactive\nSubState=dead\n";

    fn request(args: &[&str]) -> Request {
        p(args).unwrap()
    }

    fn calls(runner: &ScriptedRunner) -> Vec<String> {
        runner.calls().iter().map(command_text).collect()
    }

    #[tokio::test]
    async fn install_writes_the_unit_runs_the_commands_and_explains_lingering() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(UP));
        let outcome = perform(&request(&["install"]), &env, &runner).await;
        assert_eq!(outcome.code, 0, "{}", outcome.err);
        assert!(outcome.err.is_empty(), "{}", outcome.err);
        assert_eq!(
            calls(&runner),
            [
                "systemctl --user daemon-reload",
                "systemctl --user enable leon-host.service",
                "systemctl --user restart leon-host.service",
                "systemctl --user show leon-host.service --property=LoadState,ActiveState,SubState,UnitFileState,MainPID,ActiveEnterTimestamp",
            ]
        );
        let unit = std::fs::read_to_string(&env.layout.definition).unwrap();
        assert!(unit.contains(&format!(
            "ExecStart=\"/usr/bin/leon\" \"host\" \"--data-dir\" \"{}\"",
            root.path().join("data").display()
        )));
        assert!(outcome.out.contains("Without lingering"));
        assert!(outcome.out.contains("install --linger"));
        assert!(outcome.out.contains("full terminal"));
    }

    #[tokio::test]
    async fn install_with_linger_enables_it_and_a_refusal_is_a_note_not_a_failure() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(UP));
        let outcome = perform(&request(&["install", "--linger"]), &env, &runner).await;
        assert_eq!(calls(&runner)[3], "loginctl enable-linger");
        assert!(outcome.out.contains("Lingering is on"));

        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::failed(1, "Interactive authentication required."))
            .reply(Output::ok(UP));
        let outcome = perform(&request(&["install", "--linger"]), &env, &runner).await;
        assert_eq!(outcome.code, 0);
        assert!(!outcome.out.contains("Lingering is on"));
        assert!(outcome.err.contains("Interactive authentication required."));
    }

    #[tokio::test]
    async fn install_bakes_in_the_relay_of_the_session_unless_one_is_given() {
        let root = tempfile::tempdir().unwrap();
        let mut env = environment(root.path(), Os::Linux);
        env.relay_env = Some("wss://from-env".into());
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""));
        perform(&request(&["install"]), &env, &runner).await;
        let unit = std::fs::read_to_string(&env.layout.definition).unwrap();
        assert!(unit.contains("\"--relay\" \"wss://from-env\""));
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""));
        perform(
            &request(&["install", "--relay", "wss://given"]),
            &env,
            &runner,
        )
        .await;
        let unit = std::fs::read_to_string(&env.layout.definition).unwrap();
        assert!(unit.contains("\"--relay\" \"wss://given\""));
        assert!(!unit.contains("from-env"));
    }

    #[tokio::test]
    async fn a_required_command_that_fails_stops_the_install_and_says_why() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let runner = ScriptedRunner::new().reply(Output::failed(
            1,
            "Failed to connect to bus: No medium found",
        ));
        let outcome = perform(&request(&["install"]), &env, &runner).await;
        assert_eq!(outcome.code, 1);
        assert!(outcome.err.contains("Could not reload systemd"));
        assert!(outcome.err.contains("Failed to connect to bus"));
        assert!(outcome.err.contains("login session"));
        assert_eq!(calls(&runner).len(), 1, "nothing runs after the failure");
        assert!(outcome.out.is_empty());
    }

    #[tokio::test]
    async fn a_missing_systemctl_is_reported_as_such() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let runner = ScriptedRunner::new().fail(RunError::Spawn {
            program: "systemctl".into(),
            source: std::io::Error::other("No such file"),
        });
        let outcome = perform(&request(&["install"]), &env, &runner).await;
        assert_eq!(outcome.code, 1);
        assert!(outcome.err.contains("cannot run systemctl"));
    }

    #[tokio::test]
    async fn a_program_that_will_not_stay_is_not_installed_and_nothing_is_run() {
        let root = tempfile::tempdir().unwrap();
        let mut env = environment(root.path(), Os::Linux);
        env.exe = PathBuf::from("/tmp/.mount_LeonXyz/usr/bin/leon");
        let runner = ScriptedRunner::new();
        let outcome = perform(&request(&["install"]), &env, &runner).await;
        assert_eq!(outcome.code, 1);
        assert!(outcome.err.contains("AppImage"));
        assert!(runner.calls().is_empty());
        assert!(!env.layout.definition.exists());
    }

    #[tokio::test]
    async fn the_unit_points_at_the_file_an_update_leaves_in_place() {
        let root = tempfile::tempdir().unwrap();
        let mut env = environment(root.path(), Os::Linux);
        env.exe = PathBuf::from("/home/ana/.local/bin/leon (deleted)");
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""));
        perform(&request(&["install"]), &env, &runner).await;
        let unit = std::fs::read_to_string(&env.layout.definition).unwrap();
        assert!(unit.contains("ExecStart=\"/home/ana/.local/bin/leon\" "));
        assert!(!unit.contains("deleted"));
    }

    #[tokio::test]
    async fn a_relative_data_directory_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let runner = ScriptedRunner::new();
        let outcome = perform(
            &request(&["install", "--data-dir", "relative/dir"]),
            &env,
            &runner,
        )
        .await;
        assert_eq!(outcome.code, 1);
        assert!(outcome.err.contains("absolute"));
        assert!(runner.calls().is_empty());
    }

    #[tokio::test]
    async fn install_on_macos_writes_the_plist_and_loads_it() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::MacOs);
        let runner = ScriptedRunner::new()
            .reply(Output::failed(113, "Could not find service"))
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(
                "dev.zavu.leon.host = {\n\tstate = running\n\tpid = 7\n}\n",
            ));
        let outcome = perform(&request(&["install"]), &env, &runner).await;
        assert_eq!(outcome.code, 0, "{}", outcome.err);
        let plist = std::fs::read_to_string(&env.layout.definition).unwrap();
        assert!(plist.contains("<string>dev.zavu.leon.host</string>"));
        assert!(env.layout.log.as_ref().unwrap().parent().unwrap().is_dir());
        assert_eq!(calls(&runner).len(), 4);
        assert!(calls(&runner)[2].starts_with("launchctl bootstrap gui/1000 "));
        assert!(calls(&runner)[3].starts_with("launchctl print gui/1000/"));
        assert!(
            !outcome.out.contains("lingering"),
            "lingering is a Linux idea"
        );
    }

    /// Installs on a scripted Linux and leaves the paired data around.
    async fn installed(env: &Environment) {
        let paths = Paths::new(&env.data_dir);
        std::fs::create_dir_all(&paths.dir).unwrap();
        std::fs::write(paths.devices(), "{\"devices\":[]}").unwrap();
        std::fs::write(env.data_dir.join("identity.key"), "key").unwrap();
        std::fs::write(env.data_dir.join("settings.toml"), "theme = 'x'").unwrap();
        let install = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(UP));
        perform(&request(&["install"]), env, &install).await;
        assert!(env.layout.definition.exists());
    }

    #[tokio::test]
    async fn uninstall_removes_the_unit_and_leaves_the_pairings_and_settings() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        installed(&env).await;
        let paths = Paths::new(&env.data_dir);

        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""));
        let outcome = perform(&request(&["uninstall"]), &env, &runner).await;
        assert_eq!(outcome.code, 0);
        assert!(outcome.out.contains("the service is stopped and"));
        assert!(!env.layout.definition.exists());
        assert!(paths.devices().exists());
        assert!(env.data_dir.join("identity.key").exists());
        assert!(env.data_dir.join("settings.toml").exists());
        assert!(outcome.out.contains("untouched"));
        assert_eq!(
            calls(&runner),
            [
                "systemctl --user disable --now leon-host.service",
                "systemctl --user daemon-reload",
                "systemctl --user reset-failed leon-host.service",
            ]
        );
    }

    #[tokio::test]
    async fn a_service_that_cannot_be_stopped_keeps_its_definition_and_says_so() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        installed(&env).await;
        // No user bus: the stop fails, and the manager cannot be asked either.
        let no_bus = || Output::failed(1, "Failed to connect to bus: No medium found");
        let runner = ScriptedRunner::new().reply(no_bus()).reply(no_bus());
        let outcome = perform(&request(&["uninstall"]), &env, &runner).await;
        assert_eq!(outcome.code, 1);
        assert!(outcome.err.contains("Could not stop the service"));
        assert!(outcome.err.contains("Failed to connect to bus"));
        assert!(outcome.err.contains("is kept"));
        assert!(!outcome.out.contains("is stopped"), "{}", outcome.out);
        assert!(env.layout.definition.exists(), "the definition stays");
        assert_eq!(calls(&runner).len(), 2, "nothing else is run");

        // The manager answers and says the host still runs.
        let runner = ScriptedRunner::new()
            .reply(Output::failed(1, "Failed to disable unit"))
            .reply(Output::ok(UP));
        let outcome = perform(&request(&["uninstall"]), &env, &runner).await;
        assert_eq!(outcome.code, 1);
        assert!(outcome.err.contains("it is running"));
        assert!(env.layout.definition.exists());
    }

    #[tokio::test]
    async fn a_failed_stop_of_a_service_that_is_not_running_still_uninstalls() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        installed(&env).await;
        let runner = ScriptedRunner::new()
            .reply(Output::failed(1, "Failed to disable unit: not loaded"))
            .reply(Output::ok(UNKNOWN))
            .reply(Output::ok(""))
            .reply(Output::ok(""));
        let outcome = perform(&request(&["uninstall"]), &env, &runner).await;
        assert_eq!(outcome.code, 0, "{}", outcome.err);
        assert!(!env.layout.definition.exists());
        assert!(outcome.out.contains("the service is stopped and"));
    }

    #[tokio::test]
    async fn a_host_waiting_to_restart_is_not_a_stopped_one() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        installed(&env).await;
        let runner = ScriptedRunner::new()
            .reply(Output::failed(1, "Interactive authentication required."))
            .reply(Output::ok(RESTARTING));
        let outcome = perform(&request(&["uninstall"]), &env, &runner).await;
        assert_eq!(outcome.code, 1);
        assert!(outcome.err.contains("activating (auto-restart)"));
        assert!(env.layout.definition.exists());
    }

    #[tokio::test]
    async fn uninstalling_what_is_not_installed_is_not_an_error() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let runner = ScriptedRunner::new()
            .reply(Output::failed(1, "Unit leon-host.service does not exist"))
            .reply(Output::ok(UNKNOWN))
            .reply(Output::ok(""))
            .reply(Output::ok(""));
        let outcome = perform(&request(&["uninstall"]), &env, &runner).await;
        assert_eq!(outcome.code, 0);
        assert!(outcome.out.contains("was not installed"));
        assert!(!outcome.out.contains("may still be running"));
        assert!(outcome.err.is_empty());
    }

    #[tokio::test]
    async fn uninstalling_with_no_manager_to_ask_does_not_hide_that() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let no_bus = || Output::failed(1, "Failed to connect to bus: No medium found");
        let runner = ScriptedRunner::new()
            .reply(no_bus())
            .reply(no_bus())
            .reply(Output::ok(""))
            .reply(Output::ok(""));
        let outcome = perform(&request(&["uninstall"]), &env, &runner).await;
        assert_eq!(outcome.code, 0);
        assert!(outcome.out.contains("a host may still be running"));
        assert!(outcome
            .err
            .contains("Could not tell whether a host is still running"));
    }

    #[tokio::test]
    async fn install_says_so_when_the_service_starts_and_does_not_stay_up() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(RESTARTING));
        let outcome = perform(&request(&["install"]), &env, &runner).await;
        assert_eq!(outcome.code, 1);
        assert!(outcome
            .err
            .contains("Installed, but the service is not running"));
        assert!(outcome.err.contains("activating (auto-restart)"));
        assert!(outcome.err.contains("leon host service logs"));
        assert!(!outcome.out.contains("Installed."), "{}", outcome.out);
        assert!(env.layout.definition.exists());

        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(
                "LoadState=loaded\nActiveState=failed\nSubState=failed\n",
            ));
        let outcome = perform(&request(&["install"]), &env, &runner).await;
        assert_eq!(outcome.code, 1);
        assert!(outcome.err.contains("it is failed"));
    }

    #[tokio::test]
    async fn install_that_cannot_ask_the_manager_does_not_claim_it_checked() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::failed(1, "Failed to connect to bus"));
        let outcome = perform(&request(&["install"]), &env, &runner).await;
        assert_eq!(outcome.code, 0);
        assert!(outcome
            .err
            .contains("Could not confirm that the host is running"));
    }

    #[tokio::test]
    async fn install_from_a_per_version_folder_warns_that_an_update_will_not_reach_it() {
        let root = tempfile::tempdir().unwrap();
        let mut env = environment(root.path(), Os::Linux);
        env.exe = PathBuf::from("/opt/homebrew/Cellar/leon/0.6.0/bin/leon");
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(UP));
        let outcome = perform(&request(&["install"]), &env, &runner).await;
        assert_eq!(outcome.code, 0);
        assert!(outcome.out.contains("holds one version only"));
        assert!(outcome.out.contains("install` again after an update"));
    }

    #[tokio::test]
    async fn macos_status_asks_launchd_whether_the_agent_is_switched_off() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::MacOs);
        std::fs::create_dir_all(env.layout.definition.parent().unwrap()).unwrap();
        std::fs::write(&env.layout.definition, "plist").unwrap();
        let runner = ScriptedRunner::new()
            .reply(Output::ok("x = {\n\tstate = running\n\tpid = 7\n}\n"))
            .reply(Output::ok(
                "disabled services = {\n\t\"dev.zavu.leon.host\" => disabled\n}\n",
            ));
        let outcome = perform(&request(&["status"]), &env, &runner).await;
        assert_eq!(outcome.code, 0);
        assert!(outcome.out.contains("never by itself"));
        assert_eq!(calls(&runner)[1], "launchctl print-disabled gui/1000");
    }

    #[tokio::test]
    async fn status_reports_a_running_service_its_devices_and_its_uptime() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        std::fs::create_dir_all(env.layout.definition.parent().unwrap()).unwrap();
        std::fs::write(&env.layout.definition, "unit").unwrap();
        let paths = Paths::new(&env.data_dir);
        let mut registry = DeviceRegistry::open(&paths.devices()).unwrap();
        registry.authorise([1; 32], "phone", 5).unwrap();
        registry.authorise([2; 32], "tablet", 5).unwrap();
        registry.revoke("tablet").unwrap();
        let runner = ScriptedRunner::new()
            .reply(Output::ok(
                "LoadState=loaded\nActiveState=active\nSubState=running\nUnitFileState=enabled\nMainPID=99\nActiveEnterTimestamp=Wed 2026-10-07 09:00:00 UTC\n",
            ))
            .reply(Output::ok("Linger=yes\n"));
        let outcome = perform(&request(&["status"]), &env, &runner).await;
        assert_eq!(outcome.code, 0);
        assert!(outcome
            .out
            .contains("State:     running (pid 99), since Wed 2026-10-07 09:00:00 UTC"));
        assert!(outcome.out.contains("Starts:    at boot (lingering is on)"));
        assert!(outcome.out.contains("Devices:   1 paired, 1 revoked"));
    }

    #[tokio::test]
    async fn status_of_a_service_that_is_not_installed_exits_non_zero() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let runner = ScriptedRunner::new()
            .reply(Output::ok(
                "LoadState=not-found\nActiveState=inactive\nSubState=dead\n",
            ))
            .reply(Output::ok("Linger=no\n"));
        let outcome = perform(&request(&["status"]), &env, &runner).await;
        assert_eq!(outcome.code, 1);
        assert!(outcome.out.contains("not installed"));
        assert!(outcome.out.contains("none paired"));
    }

    #[tokio::test]
    async fn logs_come_from_the_journal_on_linux() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::Linux);
        let runner = ScriptedRunner::new().reply(Output::ok("line one\nline two\n"));
        let outcome = perform(&request(&["logs", "--lines", "2"]), &env, &runner).await;
        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.out, "line one\nline two\n");
        assert_eq!(
            calls(&runner),
            ["journalctl --user --unit leon-host.service --lines 2 --no-pager"]
        );
    }

    #[tokio::test]
    async fn logs_come_from_the_agents_file_on_macos() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path(), Os::MacOs);
        let runner = ScriptedRunner::new();
        let missing = perform(&request(&["logs"]), &env, &runner).await;
        assert_eq!(missing.code, 1);
        assert!(missing.err.contains("No log yet"));

        let log = env.layout.log.clone().unwrap();
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "a\nb\nc\nd\n").unwrap();
        let outcome = perform(&request(&["logs", "-n", "2"]), &env, &runner).await;
        assert_eq!(outcome.out, "c\nd\n");
        assert!(runner.calls().is_empty(), "a file needs no command");
    }

    #[test]
    fn a_long_log_is_read_from_its_end_without_a_cut_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.log");
        let line = format!("{}\n", "x".repeat(99));
        let count = (LOG_TAIL_BYTES as usize / line.len()) * 3;
        std::fs::write(&path, line.repeat(count)).unwrap();
        let tail = read_tail(&path).unwrap();
        assert!(tail.len() as u64 <= LOG_TAIL_BYTES);
        assert!(tail.lines().all(|l| l.len() == 99), "no partial first line");
        assert_eq!(last_lines("1\n2\n3\n", 5), "1\n2\n3\n");
        assert_eq!(last_lines("1\n2\n3\n", 1), "3\n");
    }
}

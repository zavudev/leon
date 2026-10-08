//! The `leon host` command line.
//!
//! ```text
//! leon host [--pair]    run the service in the foreground (Ctrl-C stops it)
//! leon host pair        print a fresh pairing code from the running service
//! leon host devices     list the paired devices
//! leon host revoke <d>  revoke a device by id (or its name)
//! leon host status      say whether the service is running and what it is doing
//! ```
//!
//! `leon host service install | uninstall | status | logs` (the background
//! service of the user's own session) is the application's: it needs the
//! command runner, which this crate does not use. The usage text names it.
//!
//! Options: `--relay <url>` (or `LEON_RELAY_URL`), `--name <name>`,
//! `--data-dir <path>`, `--allow-root`.
//!
//! The service keeps its files in `<data dir>/host`: `devices.json`
//! (owner-only), a `status` heartbeat and a `control`
//! directory through which `leon host pair` talks to the running process (it
//! says since when the process runs, as `started`, which the service's
//! `status` reads). The
//! installation's `identity.key` is in `<data dir>` itself (owner-only). Both
//! are plain text and carry no key material; the pairing code in the reply is
//! deleted as soon as it has been printed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use leon_link::identity::KEY_FILE;
use leon_link::{DeviceRegistry, Identity};

use crate::host::{running_as_root, Host, HostConfig, RelayState};

/// The relay Leon uses unless told otherwise.
pub const DEFAULT_RELAY_URL: &str = "wss://relay.getleon.dev";

const SAFETY: &str = "\
This service runs as you and gives every device you pair a full terminal as
you on this computer: it can read, change and delete everything you can.
Pair only your own devices, and revoke any you lose (leon host revoke).";

/// What was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Serve; optionally print a pairing code at start.
    Run {
        /// Print a code once the service is up.
        pair: bool,
    },
    /// Ask the running service for a code.
    Pair,
    /// List devices.
    Devices,
    /// Revoke one.
    Revoke(String),
    /// Report.
    Status,
    /// Print the usage.
    Help,
}

/// Parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// What to do.
    pub action: Action,
    /// Where the data lives (the platform's directory when absent).
    pub data_dir: Option<PathBuf>,
    /// The relay to use.
    pub relay: Option<String>,
    /// The name devices see.
    pub name: Option<String>,
    /// Permit running as the superuser.
    pub allow_root: bool,
}

/// The usage text.
pub fn usage() -> &'static str {
    "Usage: leon host [--pair] [options]\n       leon host pair | devices | status\n       leon host revoke <device id or name>\n\n\
     Options:\n  --relay <url>        The relay (default wss://relay.getleon.dev, or LEON_RELAY_URL)\n  \
     --name <name>        The name your devices see (default: this computer's name)\n  \
     --data-dir <path>    Keep the service's files here\n  \
     --allow-root         Allow running as the superuser (not recommended)\n  \
     -h, --help           Print this help\n\n\
     In the background: leon host service install | uninstall | status | logs"
}

/// Parses the arguments after `host`.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        action: Action::Run { pair: false },
        data_dir: None,
        relay: None,
        name: None,
        allow_root: false,
    };
    let mut args = args.into_iter();
    let mut positional: Vec<String> = Vec::new();
    let mut pair_flag = false;
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
            "-h" | "--help" => options.action = Action::Help,
            "--pair" => pair_flag = true,
            "--allow-root" => options.allow_root = true,
            "--relay" => options.relay = Some(value("a relay URL")?),
            "--name" => options.name = Some(value("a name")?),
            "--data-dir" => options.data_dir = Some(PathBuf::from(value("a path")?)),
            other if other.starts_with('-') => return Err(format!("Unknown option {other:?}.")),
            _ => positional.push(argument),
        }
    }
    if options.action != Action::Help {
        options.action = match positional.as_slice() {
            [] => Action::Run { pair: pair_flag },
            [word] if word == "pair" => Action::Pair,
            [word] if word == "devices" => Action::Devices,
            [word] if word == "status" => Action::Status,
            [word, who] if word == "revoke" => Action::Revoke(who.clone()),
            [word] if word == "revoke" => return Err("revoke needs a device id or name.".into()),
            [other, ..] => return Err(format!("Unknown command {other:?}.")),
        };
    }
    Ok(options)
}

/// The files of the service.
#[derive(Debug, Clone)]
pub struct Paths {
    /// The data directory: it holds `identity.key`, shared by the sharing
    /// service and by this installation's connections to other computers.
    pub root: PathBuf,
    /// `<data dir>/host`: the service's own files.
    pub dir: PathBuf,
}

impl Paths {
    /// Paths under a data directory.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.to_path_buf(),
            dir: data_dir.join("host"),
        }
    }
    /// The paired devices.
    pub fn devices(&self) -> PathBuf {
        self.dir.join("devices.json")
    }
    fn status(&self) -> PathBuf {
        self.dir.join("status")
    }
    fn request(&self) -> PathBuf {
        self.dir.join("control").join("pair-request")
    }
    fn response(&self) -> PathBuf {
        self.dir.join("control").join("pair-response")
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// This computer's name, for the default device name.
pub fn computer_name() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is valid for its length; the call NUL-terminates
        // within it on success.
        let ok = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len() - 1) } == 0;
        if ok {
            let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
            if let Ok(name) = std::str::from_utf8(&buffer[..end]) {
                if !name.is_empty() {
                    return name.trim_end_matches(".local").to_owned();
                }
            }
        }
    }
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this computer".into())
}

fn write_text(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(temp, path)
}

fn relay_text(state: &RelayState) -> String {
    match state {
        RelayState::Connecting => "connecting".into(),
        RelayState::Online => "online".into(),
        RelayState::Offline { reason, .. } => format!("offline ({reason})"),
        RelayState::Stopped => "stopped".into(),
    }
}

/// Runs the service until `stop` completes.
pub async fn serve(
    paths: &Paths,
    relay: String,
    name: String,
    print_code: bool,
    stop: impl std::future::Future<Output = ()>,
) -> Result<(), String> {
    let identity = Arc::new(
        Identity::load_or_create(&paths.root)
            .map_err(|e| format!("cannot load the identity: {e}"))?,
    );
    let mut config = HostConfig::new(relay.clone(), name.clone());
    config.registry_poll = Duration::from_millis(500);
    let host = Host::start(config, identity.clone(), Some(paths.devices()))
        .map_err(|e| format!("cannot open the device list: {e}"))?;
    println!("{SAFETY}\n");
    println!("Host id:  {}", identity.host_id());
    println!("Name:     {name}");
    println!("Relay:    {relay}");
    println!(
        "Devices:  {} paired",
        host.devices().iter().filter(|d| !d.revoked).count()
    );
    println!("\nRunning. Press Ctrl-C to stop; run `leon host pair` in another terminal for a pairing code.");
    let mut relay_state = host.watch_relay();
    let mut shown = relay_text(&relay_state.borrow());
    println!("Relay is {shown}.");
    if print_code {
        print_pairing(&host).await;
    }
    let _ = std::fs::remove_file(paths.request());
    tokio::pin!(stop);
    let mut tick = tokio::time::interval(Duration::from_millis(400));
    let mut last_status = 0u64;
    let started = now_unix();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = relay_state.changed() => {
                let now = relay_text(&relay_state.borrow_and_update());
                if now != shown {
                    println!("Relay is {now}.");
                    shown = now;
                }
            }
            _ = tick.tick() => {
                if paths.request().exists() {
                    let _ = std::fs::remove_file(paths.request());
                    let info = host.new_pairing_code().await;
                    let text = format!("code={}\nexpires_in={}\n", info.code, info.remaining.as_secs());
                    let _ = write_text(&paths.response(), &text);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(paths.response(), std::fs::Permissions::from_mode(0o600));
                    }
                }
                let now = now_unix();
                if now != last_status {
                    last_status = now;
                    let status = host.status();
                    let text = format!(
                        "pid={}\nhost_id={}\nrelay={}\nsessions={}\nterminals={}\nstarted={}\nupdated={}\n",
                        std::process::id(), status.host_id, relay_text(&status.relay),
                        status.sessions, status.terminals, started, now
                    );
                    let _ = write_text(&paths.status(), &text);
                }
            }
        }
    }
    host.stop();
    let _ = std::fs::remove_file(paths.status());
    println!("Stopped.");
    Ok(())
}

async fn print_pairing(host: &Host) {
    let info = host.new_pairing_code().await;
    println!(
        "\nPairing code:  {}\nOn the other computer: Leon > Connect a machine > With a code.\nThe code works once and expires in {} minutes.",
        info.code,
        info.remaining.as_secs() / 60
    );
}

fn read_kv(path: &Path) -> Option<std::collections::HashMap<String, String>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(
        text.lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
    )
}

/// How old a heartbeat may be and still count as a running host, in seconds.
const HEARTBEAT_LIFETIME: u64 = 10;

/// Whether the process `pid` exists. A pid that is not a plain process id
/// (zero and negative ones address whole groups) never does. Where this cannot
/// be asked (not Unix) it says yes, so the heartbeat's age decides alone.
fn process_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Some(pid) = i32::try_from(pid).ok().filter(|pid| *pid > 0) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process can be signalled.
        let found = unsafe { libc::kill(pid, 0) } == 0;
        // EPERM: it exists and belongs to someone else.
        found || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Whether a heartbeat is that of a host that is running: written in the last
/// [`HEARTBEAT_LIFETIME`] seconds *and* by a process that still exists. The
/// second half matters after a crash: the file a killed host leaves behind is
/// seconds old when a service manager starts the next one, and that start must
/// not be refused because of its dead predecessor. (A heartbeat without a pid
/// is judged by its age alone.)
fn heartbeat_is_live(
    map: &std::collections::HashMap<String, String>,
    now: u64,
    exists: impl Fn(u32) -> bool,
) -> bool {
    let Some(updated) = map.get("updated").and_then(|v| v.parse::<u64>().ok()) else {
        return false;
    };
    if now.saturating_sub(updated) > HEARTBEAT_LIFETIME {
        return false;
    }
    match map.get("pid").and_then(|v| v.parse::<u32>().ok()) {
        Some(pid) => exists(pid),
        None => true,
    }
}

/// The running service's heartbeat, when it is recent and its process lives.
pub fn running_status(paths: &Paths) -> Option<std::collections::HashMap<String, String>> {
    let map = read_kv(&paths.status())?;
    heartbeat_is_live(&map, now_unix(), process_exists).then_some(map)
}

fn ago(unix: u64) -> String {
    let secs = now_unix().saturating_sub(unix);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86400),
    }
}

/// Runs the command line. Returns the process exit code.
pub fn run(args: Vec<String>, default_data_dir: PathBuf) -> i32 {
    let options = match parse(args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n\n{}", usage());
            return 2;
        }
    };
    let paths = Paths::new(&options.data_dir.clone().unwrap_or(default_data_dir));
    match options.action {
        Action::Help => {
            println!("{}", usage());
            0
        }
        Action::Devices => match DeviceRegistry::open(&paths.devices()) {
            Ok(registry) if registry.devices().is_empty() => {
                println!("No devices are paired.");
                0
            }
            Ok(registry) => {
                for d in registry.devices() {
                    println!(
                        "{}  {}  {}  last seen {}",
                        d.device_id(),
                        if d.revoked { "revoked" } else { "active " },
                        d.name,
                        ago(d.last_seen_unix)
                    );
                }
                0
            }
            Err(error) => {
                eprintln!("{error}");
                1
            }
        },
        Action::Revoke(who) => {
            let mut registry = match DeviceRegistry::open(&paths.devices()) {
                Ok(registry) => registry,
                Err(error) => {
                    eprintln!("{error}");
                    return 1;
                }
            };
            match registry.revoke(&who) {
                Ok(record) => {
                    println!(
                        "Revoked {} ({}). A running service cuts it off within a second.",
                        record.name,
                        record.device_id()
                    );
                    0
                }
                Err(error) => {
                    eprintln!("{error}");
                    1
                }
            }
        }
        Action::Status => {
            let id = Identity::load_or_create(&paths.root).map(|i| i.host_id().to_string());
            match running_status(&paths) {
                Some(map) => {
                    println!(
                        "Running (pid {}).",
                        map.get("pid").map_or("?", String::as_str)
                    );
                    println!(
                        "Host id:   {}",
                        map.get("host_id").map_or("?", String::as_str)
                    );
                    println!(
                        "Relay:     {}",
                        map.get("relay").map_or("?", String::as_str)
                    );
                    println!(
                        "Sessions:  {}",
                        map.get("sessions").map_or("0", String::as_str)
                    );
                    println!(
                        "Terminals: {}",
                        map.get("terminals").map_or("0", String::as_str)
                    );
                    0
                }
                None => {
                    println!(
                        "Not running.{}",
                        id.map(|id| format!(" Host id: {id}")).unwrap_or_default()
                    );
                    1
                }
            }
        }
        Action::Pair => {
            if running_status(&paths).is_none() {
                eprintln!(
                    "No running service found. Start it with `leon host` in another terminal."
                );
                return 1;
            }
            let _ = std::fs::remove_file(paths.response());
            if write_text(&paths.request(), "pair\n").is_err() {
                eprintln!("Cannot reach the running service.");
                return 1;
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                if let Some(map) = read_kv(&paths.response()) {
                    let _ = std::fs::remove_file(paths.response());
                    let code = map.get("code").cloned().unwrap_or_default();
                    println!("Pairing code:  {code}\nOn the other computer: Leon > Connect a machine > With a code.\nThe code works once and expires in 10 minutes.");
                    return 0;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            eprintln!("The service did not answer.");
            1
        }
        Action::Run { pair } => {
            if running_as_root() && !options.allow_root {
                eprintln!("Refusing to run as root: every paired device would get a root shell. Run it as your own user, or pass --allow-root if you really mean it.");
                return 1;
            }
            if running_status(&paths).is_some() {
                eprintln!("A host service is already running for this data directory.");
                return 1;
            }
            let relay = options
                .relay
                .or_else(|| std::env::var("LEON_RELAY_URL").ok())
                .unwrap_or_else(|| DEFAULT_RELAY_URL.to_owned());
            let name = options.name.unwrap_or_else(computer_name);
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("cannot start the runtime: {error}");
                    return 1;
                }
            };
            let result = runtime.block_on(serve(&paths, relay, name, pair, shutdown_signal()));
            match result {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("{error}");
                    1
                }
            }
        }
    }
}

/// Completes when the person (Ctrl-C) or a service manager (`SIGTERM`, what
/// `systemctl stop` and `launchctl bootout` send) asks the service to stop, so
/// that it removes its heartbeat and closes its sessions instead of being cut.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// Where the identity of a host data directory lives, for the UI.
pub fn identity_file(paths: &Paths) -> PathBuf {
    paths.root.join(KEY_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Options, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_arguments_runs_the_service() {
        assert_eq!(p(&[]).unwrap().action, Action::Run { pair: false });
        assert_eq!(p(&["--pair"]).unwrap().action, Action::Run { pair: true });
    }

    #[test]
    fn the_subcommands_are_recognised() {
        assert_eq!(p(&["pair"]).unwrap().action, Action::Pair);
        assert_eq!(p(&["devices"]).unwrap().action, Action::Devices);
        assert_eq!(p(&["status"]).unwrap().action, Action::Status);
        assert_eq!(
            p(&["revoke", "ABC"]).unwrap().action,
            Action::Revoke("ABC".into())
        );
        assert!(p(&["revoke"]).is_err());
        assert!(p(&["frobnicate"]).is_err());
    }

    #[test]
    fn options_take_values_inline_or_separately() {
        let o = p(&[
            "--relay=ws://x",
            "--name",
            "box",
            "--data-dir",
            "/tmp/d",
            "--allow-root",
        ])
        .unwrap();
        assert_eq!(o.relay.as_deref(), Some("ws://x"));
        assert_eq!(o.name.as_deref(), Some("box"));
        assert_eq!(o.data_dir, Some(PathBuf::from("/tmp/d")));
        assert!(o.allow_root);
        assert!(p(&["--relay"]).is_err());
        assert!(p(&["--nope"]).is_err());
    }

    #[test]
    fn help_wins() {
        assert_eq!(p(&["pair", "--help"]).unwrap().action, Action::Help);
    }

    #[test]
    fn the_usage_names_the_background_service() {
        assert!(usage().contains("leon host service install"));
    }

    fn heartbeat(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_heartbeat_left_by_a_crashed_host_does_not_count_as_running() {
        // Written one second ago by a process that is gone: what a SIGKILL or
        // an out-of-memory kill leaves for the restart that follows it.
        let left_behind = heartbeat(&[("pid", "4242"), ("updated", "999")]);
        assert!(!heartbeat_is_live(&left_behind, 1_000, |_| false));
        assert!(heartbeat_is_live(&left_behind, 1_000, |pid| pid == 4242));
    }

    #[test]
    fn a_heartbeat_counts_only_while_it_is_recent() {
        let beat = heartbeat(&[("pid", "4242"), ("updated", "1000")]);
        assert!(heartbeat_is_live(&beat, 1_010, |_| true));
        assert!(!heartbeat_is_live(&beat, 1_011, |_| true));
        assert!(!heartbeat_is_live(
            &heartbeat(&[("pid", "1")]),
            1_000,
            |_| true
        ));
        assert!(!heartbeat_is_live(
            &heartbeat(&[("updated", "soon")]),
            1_000,
            |_| true
        ));
    }

    #[test]
    fn a_heartbeat_without_a_pid_is_judged_by_its_age() {
        let beat = heartbeat(&[("updated", "1000")]);
        assert!(heartbeat_is_live(&beat, 1_005, |_| false));
        assert!(!heartbeat_is_live(&beat, 1_020, |_| true));
    }

    #[cfg(unix)]
    #[test]
    fn this_process_exists_and_numbers_that_are_not_a_process_do_not() {
        assert!(process_exists(std::process::id()));
        // Zero and numbers above `i32::MAX` would address groups or nothing.
        assert!(!process_exists(0));
        assert!(!process_exists(u32::MAX));
        assert!(!process_exists(i32::MAX as u32));
    }

    #[test]
    fn a_stale_status_file_does_not_stop_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(&paths.dir).unwrap();
        // A process id no system hands out, written a moment ago.
        let text = format!("pid=0\nupdated={}\n", now_unix());
        std::fs::write(paths.status(), text).unwrap();
        #[cfg(unix)]
        assert!(running_status(&paths).is_none());
        let text = format!("pid={}\nupdated={}\n", std::process::id(), now_unix());
        std::fs::write(paths.status(), text).unwrap();
        assert!(running_status(&paths).is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sigterm_ends_the_wait_for_a_shutdown() {
        let mut stop = Box::pin(shutdown_signal());
        // Poll once so the handler is installed before the signal is sent.
        tokio::select! {
            biased;
            _ = &mut stop => panic!("stopped before any signal"),
            () = std::future::ready(()) => {}
        }
        // SAFETY: sends a signal to this process, which the handler now owns.
        assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGTERM) }, 0);
        tokio::time::timeout(Duration::from_secs(20), stop)
            .await
            .expect("SIGTERM stops the host");
    }

    #[test]
    fn the_safety_notice_says_what_a_paired_device_can_do() {
        assert!(SAFETY.contains("full terminal"));
    }

    #[test]
    fn devices_and_revoke_work_on_the_file_without_a_running_service() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let mut registry = DeviceRegistry::open(&paths.devices()).unwrap();
        registry.authorise([4; 32], "phone", now_unix()).unwrap();
        let id = registry.devices()[0].device_id().short();
        assert_eq!(
            run(
                vec![
                    "revoke".into(),
                    id,
                    "--data-dir".into(),
                    dir.path().display().to_string()
                ],
                PathBuf::new()
            ),
            0
        );
        let after = DeviceRegistry::open(&paths.devices()).unwrap();
        assert!(!after.is_authorised(&[4; 32]));
        assert_eq!(
            run(
                vec![
                    "devices".into(),
                    "--data-dir".into(),
                    dir.path().display().to_string()
                ],
                PathBuf::new()
            ),
            0
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pair_talks_to_the_running_service_through_the_control_files() {
        use leon_link::test_support::TestRelay;
        let relay = TestRelay::start().await;
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let service = {
            let paths = paths.clone();
            let url = relay.url();
            tokio::spawn(async move {
                serve(&paths, url, "box".into(), false, async {
                    let _ = stop_rx.await;
                })
                .await
            })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let heartbeat = loop {
            if let Some(map) = running_status(&paths) {
                break map;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the service never reported"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert!(
            heartbeat
                .get("started")
                .is_some_and(|s| s.parse::<u64>().is_ok()),
            "the heartbeat says since when the service runs"
        );
        let args = vec![
            "pair".to_owned(),
            "--data-dir".to_owned(),
            dir.path().display().to_string(),
        ];
        let code = tokio::task::spawn_blocking(move || run(args, PathBuf::new()))
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(
            !paths.response().exists(),
            "the code file is removed once printed"
        );
        let _ = stop_tx.send(());
        service.await.unwrap().unwrap();
        assert!(!paths.status().exists());
    }
}

//! Reading what the system manager and the host itself say about the service,
//! and putting it in words.
//!
//! Two witnesses are asked and neither is trusted for what the other knows.
//! The manager (systemd, launchd) says whether the service is loaded, running
//! and enabled; the host's own heartbeat (`<data dir>/host/status`) says since
//! when it runs, its relay and its host id; the device list says who is paired.
//! The parsers take the text of the commands' output and the renderer takes
//! plain values, so all of it is tested without a manager.

use std::collections::HashMap;
use std::path::PathBuf;

use leon_remote::{Output, RunError};

use super::plan::Os;

/// What the manager says the service is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    /// Running.
    Running,
    /// Ended with a failure and not (yet) started again.
    Failed,
    /// Loaded and not running.
    Stopped,
    /// The manager does not know the service.
    NotLoaded,
    /// Another state, in the manager's words (a restart waiting out its delay).
    Other(String),
    /// The manager could not be asked, and why.
    Unavailable(String),
}

/// The manager's view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manager {
    /// What it is doing.
    pub activity: Activity,
    /// The process, when there is one.
    pub pid: Option<u32>,
    /// Whether it starts by itself at login; unknown when the manager does not
    /// say.
    pub enabled: Option<bool>,
    /// Since when it has been running, as the manager writes it.
    pub since: Option<String>,
}

impl Activity {
    /// Whether nothing of the service is running or about to run: not
    /// loaded, stopped, or ended with a failure the manager does not retry.
    pub fn is_down(&self) -> bool {
        matches!(self, Self::NotLoaded | Self::Stopped | Self::Failed)
    }

    /// What it is doing, for a sentence ("the service is ...").
    pub fn describe(&self) -> String {
        match self {
            Self::Running => "running".into(),
            Self::Failed => "failed".into(),
            Self::Stopped => "stopped".into(),
            Self::NotLoaded => "not loaded by the system manager".into(),
            Self::Other(state) => state.clone(),
            Self::Unavailable(why) => format!("unknown ({why})"),
        }
    }
}

impl Manager {
    fn of(activity: Activity) -> Self {
        Self {
            activity,
            pid: None,
            enabled: None,
            since: None,
        }
    }
}

fn properties(text: &str) -> HashMap<&str, &str> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect()
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it gave no reason")
        .to_owned()
}

/// What `systemctl --user show` said about the unit.
pub fn parse_systemd(result: &Result<Output, RunError>) -> Manager {
    let output = match result {
        Err(error) => {
            return Manager::of(Activity::Unavailable(format!(
                "cannot run systemctl: {error}"
            )))
        }
        Ok(output) if !output.success() => {
            return Manager::of(Activity::Unavailable(first_line(&output.stderr)))
        }
        Ok(output) => output,
    };
    let values = properties(&output.stdout);
    let get = |key: &str| values.get(key).copied().unwrap_or("");
    let activity = match (get("LoadState"), get("ActiveState"), get("SubState")) {
        ("not-found", _, _) => Activity::NotLoaded,
        (_, "active", "running") => Activity::Running,
        (_, "failed", _) => Activity::Failed,
        (_, "inactive", _) => Activity::Stopped,
        (_, active, sub) if !active.is_empty() => Activity::Other(format!("{active} ({sub})")),
        _ => Activity::NotLoaded,
    };
    let running = activity == Activity::Running;
    Manager {
        pid: get("MainPID").parse().ok().filter(|pid| *pid != 0),
        enabled: match get("UnitFileState") {
            "" => None,
            state => Some(state == "enabled"),
        },
        since: (running && !get("ActiveEnterTimestamp").is_empty())
            .then(|| get("ActiveEnterTimestamp").to_owned()),
        activity,
    }
}

/// What `launchctl print gui/<uid>/<label>` said. It fails when the agent is
/// not loaded.
pub fn parse_launchd(result: &Result<Output, RunError>) -> Manager {
    let output = match result {
        Err(error) => {
            return Manager::of(Activity::Unavailable(format!(
                "cannot run launchctl: {error}"
            )))
        }
        Ok(output) => output,
    };
    if !output.success() {
        return Manager::of(Activity::NotLoaded);
    }
    let value = |key: &str| {
        output.stdout.lines().find_map(|line| {
            line.trim()
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(" = "))
        })
    };
    let activity = match value("state") {
        Some("running") => Activity::Running,
        Some(other) => Activity::Other(other.to_owned()),
        None => Activity::Stopped,
    };
    Manager {
        activity,
        pid: value("pid").and_then(|pid| pid.parse().ok()),
        // `print` does not say whether the agent is switched off for the next
        // login: `parse_launchd_disabled` reads that from another command.
        enabled: None,
        since: None,
    }
}

/// Whether launchd has the agent `label` enabled, from `launchctl
/// print-disabled gui/<uid>`: a line `"<label>" => enabled|disabled` (older
/// systems print `false|true`). An agent that is not listed has never been
/// switched off, so it is enabled; a command that failed says nothing.
pub fn parse_launchd_disabled(result: &Result<Output, RunError>, label: &str) -> Option<bool> {
    let output = result.as_ref().ok().filter(|output| output.success())?;
    let quoted = format!("\"{label}\"");
    let state = output.stdout.lines().find_map(|line| {
        let (name, state) = line.trim().split_once("=>")?;
        (name.trim() == quoted).then(|| state.trim())
    });
    match state {
        None | Some("enabled" | "false") => Some(true),
        Some("disabled" | "true") => Some(false),
        Some(_) => None,
    }
}

/// Whether lingering is on, from `loginctl show-user --property=Linger`.
pub fn parse_linger(result: &Result<Output, RunError>) -> Option<bool> {
    let output = result.as_ref().ok().filter(|output| output.success())?;
    match properties(&output.stdout).get("Linger").copied()? {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// What the running host wrote in its heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Heartbeat {
    /// Its process.
    pub pid: Option<u32>,
    /// Its host id.
    pub host_id: Option<String>,
    /// Its relay's state.
    pub relay: Option<String>,
    /// When it started, in seconds since the epoch.
    pub started: Option<u64>,
}

impl Heartbeat {
    /// Reads the fields of the heartbeat file.
    pub fn from_map(map: &HashMap<String, String>) -> Self {
        Self {
            pid: map.get("pid").and_then(|v| v.parse().ok()),
            host_id: map.get("host_id").cloned(),
            relay: map.get("relay").cloned(),
            started: map.get("started").and_then(|v| v.parse().ok()),
        }
    }
}

/// How many devices are paired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Devices {
    /// Paired and allowed.
    pub active: usize,
    /// Paired and revoked.
    pub revoked: usize,
}

/// Everything `status` prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The system.
    pub os: Os,
    /// The unit file or property list.
    pub definition: PathBuf,
    /// Whether that file exists.
    pub installed: bool,
    /// The manager's view.
    pub manager: Manager,
    /// Whether the user manager outlives the last logout (Linux).
    pub linger: Option<bool>,
    /// The running host's heartbeat, when there is a recent one.
    pub heartbeat: Option<Heartbeat>,
    /// The paired devices, or why they cannot be read.
    pub devices: Result<Devices, String>,
    /// Now, in seconds since the epoch.
    pub now: u64,
}

impl Report {
    /// Whether the service is running: the exit status of `status`.
    pub fn running(&self) -> bool {
        self.manager.activity == Activity::Running
    }
}

/// A span of time for a person: `3 h 12 min`.
pub fn duration_text(seconds: u64) -> String {
    let minutes = seconds / 60;
    match seconds {
        0..=59 => "less than a minute".into(),
        60..=3599 => format!("{minutes} min"),
        3600..=86399 => match minutes % 60 {
            0 => format!("{} h", minutes / 60),
            rest => format!("{} h {rest} min", minutes / 60),
        },
        _ => match (minutes / 60) % 24 {
            0 => format!("{} d", seconds / 86400),
            hours => format!("{} d {hours} h", seconds / 86400),
        },
    }
}

fn state_line(report: &Report) -> String {
    let manager = &report.manager;
    match &manager.activity {
        Activity::Running => {
            let pid = manager.pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default();
            let since = match (
                report.heartbeat.as_ref().and_then(|beat| beat.started),
                &manager.since,
            ) {
                (Some(started), _) => format!(", up {}", duration_text(report.now.saturating_sub(started))),
                (None, Some(since)) => format!(", since {since}"),
                (None, None) => String::new(),
            };
            format!("running{pid}{since}")
        }
        Activity::Failed => "failed. See `leon host service logs`.".into(),
        Activity::Stopped => "installed, not running".into(),
        Activity::NotLoaded if report.installed => {
            "installed, but the system manager has not loaded it. Run `leon host service install` again.".into()
        }
        Activity::NotLoaded => "not running".into(),
        Activity::Other(state) => format!("{state}. See `leon host service logs`."),
        Activity::Unavailable(why) => format!("unknown. {why}"),
    }
}

fn starts_line(report: &Report) -> Option<String> {
    if !report.installed {
        return None;
    }
    Some(match (report.os, report.manager.enabled, report.linger) {
        (_, Some(false), _) => "never by itself: the service is not enabled".into(),
        (Os::Linux, _, Some(true)) => "at boot (lingering is on)".into(),
        (Os::Linux, _, Some(false)) => {
            "at login. It stops when you log out of your last session; `leon host service install --linger` keeps it running".into()
        }
        (Os::MacOs, None, _) => {
            "at login, unless launchd has switched it off (that could not be checked)".into()
        }
        _ => "at login".into(),
    })
}

fn devices_line(devices: &Result<Devices, String>) -> String {
    match devices {
        Err(error) => format!("cannot be read ({error})"),
        Ok(Devices {
            active: 0,
            revoked: 0,
        }) => "none paired. `leon host pair` prints a code".into(),
        Ok(Devices { active, revoked: 0 }) => format!("{active} paired"),
        Ok(Devices { active, revoked }) => format!("{active} paired, {revoked} revoked"),
    }
}

/// The text of `leon host service status`.
pub fn render(report: &Report) -> String {
    let mut lines = Vec::new();
    if report.installed {
        lines.push(format!(
            "Service:   installed ({})",
            report.definition.display()
        ));
    } else {
        lines.push("Service:   not installed. `leon host service install` installs it".into());
    }
    lines.push(format!("State:     {}", state_line(report)));
    if let Some(starts) = starts_line(report) {
        lines.push(format!("Starts:    {starts}"));
    }
    if let Some(beat) = &report.heartbeat {
        if report.running() {
            lines.push(format!(
                "Relay:     {}",
                beat.relay.as_deref().unwrap_or("?")
            ));
            lines.push(format!(
                "Host id:   {}",
                beat.host_id.as_deref().unwrap_or("?")
            ));
        } else {
            let pid = beat
                .pid
                .map(|pid| format!(" (pid {pid})"))
                .unwrap_or_default();
            lines.push(format!(
                "Note:      a `leon host` started by hand is running{pid} for this data directory; it is not the service"
            ));
        }
    }
    lines.push(format!("Devices:   {}", devices_line(&report.devices)));
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> Result<Output, RunError> {
        Ok(Output::ok(text))
    }

    const RUNNING: &str = "LoadState=loaded\nActiveState=active\nSubState=running\nUnitFileState=enabled\nMainPID=4242\nActiveEnterTimestamp=Wed 2026-10-07 09:00:00 UTC\n";

    #[test]
    fn systemd_says_running_with_its_pid_and_since_when() {
        let manager = parse_systemd(&ok(RUNNING));
        assert_eq!(manager.activity, Activity::Running);
        assert_eq!(manager.pid, Some(4242));
        assert_eq!(manager.enabled, Some(true));
        assert_eq!(
            manager.since.as_deref(),
            Some("Wed 2026-10-07 09:00:00 UTC")
        );
    }

    #[test]
    fn systemd_states_other_than_running_are_told_apart() {
        let state = |text: &str| parse_systemd(&ok(text)).activity;
        assert_eq!(
            state("LoadState=not-found\nActiveState=inactive\nSubState=dead\n"),
            Activity::NotLoaded
        );
        assert_eq!(
            state("LoadState=loaded\nActiveState=failed\nSubState=failed\n"),
            Activity::Failed
        );
        assert_eq!(
            state("LoadState=loaded\nActiveState=inactive\nSubState=dead\n"),
            Activity::Stopped
        );
        assert_eq!(
            state("LoadState=loaded\nActiveState=activating\nSubState=auto-restart\n"),
            Activity::Other("activating (auto-restart)".into())
        );
        let stopped = parse_systemd(&ok("LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\nUnitFileState=disabled\n"));
        assert_eq!((stopped.pid, stopped.enabled), (None, Some(false)));
    }

    #[test]
    fn a_user_manager_that_cannot_be_reached_is_unknown_and_says_why() {
        let manager = parse_systemd(&Ok(Output::failed(
            1,
            "\nFailed to connect to bus: No medium found\n",
        )));
        assert_eq!(
            manager.activity,
            Activity::Unavailable("Failed to connect to bus: No medium found".into())
        );
        let missing = parse_systemd(&Err(RunError::Spawn {
            program: "systemctl".into(),
            source: std::io::Error::other("not found"),
        }));
        assert!(
            matches!(missing.activity, Activity::Unavailable(why) if why.contains("systemctl"))
        );
    }

    #[test]
    fn launchd_says_running_with_its_pid_and_not_loaded_when_print_fails() {
        let text =
            "dev.zavu.leon.host = {\n\tactive count = 1\n\tstate = running\n\n\tpid = 777\n}\n";
        let manager = parse_launchd(&ok(text));
        assert_eq!(manager.activity, Activity::Running);
        assert_eq!(manager.pid, Some(777));
        assert_eq!(manager.enabled, None, "print does not say");
        assert_eq!(
            parse_launchd(&ok("x = {\n\tstate = waiting\n}\n")).activity,
            Activity::Other("waiting".into())
        );
        assert_eq!(
            parse_launchd(&Ok(Output::failed(113, "Could not find service"))).activity,
            Activity::NotLoaded
        );
    }

    #[test]
    fn launchd_says_whether_the_agent_is_switched_off() {
        let listing = |state: &str| {
            ok(&format!(
                "disabled services = {{\n\t\"com.example.other\" => disabled\n\t\"dev.zavu.leon.host\" => {state}\n}}\n"
            ))
        };
        let label = "dev.zavu.leon.host";
        assert_eq!(
            parse_launchd_disabled(&listing("enabled"), label),
            Some(true)
        );
        assert_eq!(
            parse_launchd_disabled(&listing("disabled"), label),
            Some(false)
        );
        assert_eq!(parse_launchd_disabled(&listing("true"), label), Some(false));
        assert_eq!(parse_launchd_disabled(&listing("false"), label), Some(true));
        assert_eq!(parse_launchd_disabled(&listing("odd"), label), None);
        // Not listed: never switched off.
        assert_eq!(
            parse_launchd_disabled(&ok("disabled services = {\n}\n"), label),
            Some(true)
        );
        // Another agent's switch is not this one's.
        assert_eq!(
            parse_launchd_disabled(&ok("\"dev.zavu.leon.host.other\" => disabled\n"), label),
            Some(true)
        );
        assert_eq!(
            parse_launchd_disabled(&Ok(Output::failed(1, "nope")), label),
            None
        );
    }

    #[test]
    fn only_a_stopped_or_unknown_service_counts_as_down() {
        assert!(Activity::NotLoaded.is_down());
        assert!(Activity::Stopped.is_down());
        assert!(Activity::Failed.is_down());
        assert!(!Activity::Running.is_down());
        assert!(!Activity::Other("activating (auto-restart)".into()).is_down());
        assert!(!Activity::Unavailable("no bus".into()).is_down());
        assert_eq!(
            Activity::Other("activating (auto-restart)".into()).describe(),
            "activating (auto-restart)"
        );
    }

    #[test]
    fn lingering_is_read_from_loginctl() {
        assert_eq!(parse_linger(&ok("Linger=yes\n")), Some(true));
        assert_eq!(parse_linger(&ok("Linger=no\n")), Some(false));
        assert_eq!(parse_linger(&Ok(Output::failed(1, "no"))), None);
    }

    #[test]
    fn the_heartbeat_is_read_field_by_field() {
        let map: HashMap<String, String> = [
            ("pid", "12"),
            ("host_id", "H"),
            ("relay", "online"),
            ("started", "1000"),
            ("updated", "2000"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        assert_eq!(
            Heartbeat::from_map(&map),
            Heartbeat {
                pid: Some(12),
                host_id: Some("H".into()),
                relay: Some("online".into()),
                started: Some(1000)
            }
        );
        assert_eq!(Heartbeat::from_map(&HashMap::new()), Heartbeat::default());
    }

    #[test]
    fn spans_of_time_read_like_a_person_would_say_them() {
        assert_eq!(duration_text(5), "less than a minute");
        assert_eq!(duration_text(600), "10 min");
        assert_eq!(duration_text(3600), "1 h");
        assert_eq!(duration_text(3600 * 3 + 60 * 12), "3 h 12 min");
        assert_eq!(duration_text(86400 * 2), "2 d");
        assert_eq!(duration_text(86400 * 2 + 3600 * 5), "2 d 5 h");
    }

    fn report() -> Report {
        Report {
            os: Os::Linux,
            definition: PathBuf::from("/home/ana/.config/systemd/user/leon-host.service"),
            installed: true,
            manager: parse_systemd(&ok(RUNNING)),
            linger: Some(false),
            heartbeat: Some(Heartbeat {
                pid: Some(4242),
                host_id: Some("HOST-ID".into()),
                relay: Some("online".into()),
                started: Some(1_000),
            }),
            devices: Ok(Devices {
                active: 2,
                revoked: 1,
            }),
            now: 1_000 + 3 * 3600 + 12 * 60,
        }
    }

    #[test]
    fn a_running_service_says_since_when_which_relay_and_which_devices() {
        let expected = "\
Service:   installed (/home/ana/.config/systemd/user/leon-host.service)
State:     running (pid 4242), up 3 h 12 min
Starts:    at login. It stops when you log out of your last session; `leon host service install --linger` keeps it running
Relay:     online
Host id:   HOST-ID
Devices:   2 paired, 1 revoked
";
        assert_eq!(render(&report()), expected);
        assert!(report().running());
    }

    #[test]
    fn lingering_changes_when_it_starts() {
        let text = render(&Report {
            linger: Some(true),
            ..report()
        });
        assert!(text.contains("Starts:    at boot (lingering is on)\n"));
    }

    #[test]
    fn without_a_heartbeat_the_managers_own_time_is_given() {
        let text = render(&Report {
            heartbeat: None,
            ..report()
        });
        assert!(text.contains("State:     running (pid 4242), since Wed 2026-10-07 09:00:00 UTC\n"));
        assert!(!text.contains("Relay:"));
    }

    #[test]
    fn a_service_that_is_not_installed_says_how_to_install_it() {
        let text = render(&Report {
            installed: false,
            manager: Manager::of(Activity::NotLoaded),
            heartbeat: None,
            devices: Ok(Devices {
                active: 0,
                revoked: 0,
            }),
            ..report()
        });
        let expected = "\
Service:   not installed. `leon host service install` installs it
State:     not running
Devices:   none paired. `leon host pair` prints a code
";
        assert_eq!(text, expected);
    }

    #[test]
    fn a_host_started_by_hand_is_not_mistaken_for_the_service() {
        let text = render(&Report {
            manager: Manager::of(Activity::Stopped),
            ..report()
        });
        assert!(text.contains("State:     installed, not running\n"));
        assert!(text.contains("a `leon host` started by hand is running (pid 4242)"));
        assert!(!text.contains("Relay:"));
    }

    #[test]
    fn a_failed_service_points_to_the_log_and_an_unreadable_list_is_said() {
        let text = render(&Report {
            manager: Manager::of(Activity::Failed),
            heartbeat: None,
            devices: Err("corrupt".into()),
            ..report()
        });
        assert!(text.contains("State:     failed. See `leon host service logs`.\n"));
        assert!(text.contains("Devices:   cannot be read (corrupt)\n"));
    }

    #[test]
    fn on_macos_a_start_at_login_is_only_claimed_when_launchd_said_so() {
        let mac = |enabled| {
            let mut report = report();
            report.os = Os::MacOs;
            report.linger = None;
            report.manager.enabled = enabled;
            render(&report)
        };
        assert!(mac(Some(true)).contains("Starts:    at login\n"));
        assert!(mac(Some(false)).contains("never by itself"));
        assert!(mac(None).contains("could not be checked"));
    }

    #[test]
    fn a_service_that_is_not_enabled_says_it_will_not_start_by_itself() {
        let mut stopped = report();
        stopped.manager.enabled = Some(false);
        assert!(
            render(&stopped).contains("Starts:    never by itself: the service is not enabled\n")
        );
    }
}

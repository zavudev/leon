//! What installing, removing and asking about the service does, as data.
//!
//! A [`Plan`] is a list of [`Action`]s in order: write a file, remove a file,
//! make a folder, run a command. The functions here only decide the list; the
//! module above carries it out, running the commands through
//! `leon_remote::Runner` so that a test can script the answers and never touch
//! the real `systemctl` or `launchctl`.
//!
//! Nothing here asks for more rights than the user has: systemd's `--user`
//! manager, launchd's `gui/<uid>` domain of the person's own session, files in
//! the person's own directories. Lingering (the user manager started at boot
//! and kept after the last logout) is a separate, explicit step.

use std::path::{Path, PathBuf};

use leon_remote::CommandSpec;

use super::unit::{launchd_label, launchd_plist, systemd_unit, Service, UNIT_NAME};

/// A system a service can be installed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// A systemd user unit.
    Linux,
    /// A launchd agent.
    MacOs,
}

/// Where the service's files are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The system.
    pub os: Os,
    /// The unit file (Linux) or the property list (macOS).
    pub definition: PathBuf,
    /// The file the agent writes its output to (macOS; Linux has the journal).
    pub log: Option<PathBuf>,
    /// The launchd label (macOS).
    pub label: String,
}

impl Layout {
    /// The files of the service for a person's directories: `config_dir` is
    /// the user configuration directory (`~/.config` or `$XDG_CONFIG_HOME`),
    /// `home` the home directory.
    pub fn new(os: Os, home: &Path, config_dir: &Path, app_id: &str) -> Self {
        let label = launchd_label(app_id);
        match os {
            Os::Linux => Self {
                os,
                definition: config_dir.join("systemd").join("user").join(UNIT_NAME),
                log: None,
                label,
            },
            Os::MacOs => Self {
                os,
                definition: home
                    .join("Library")
                    .join("LaunchAgents")
                    .join(format!("{label}.plist")),
                log: Some(
                    home.join("Library")
                        .join("Logs")
                        .join("Leon")
                        .join("host.log"),
                ),
                label,
            },
        }
    }

    /// The agent in launchd's domain of this user: `gui/<uid>/<label>`.
    fn target(&self, uid: u32) -> String {
        format!("gui/{uid}/{}", self.label)
    }
}

/// One thing to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Make a folder (and its parents).
    MakeDir(PathBuf),
    /// Write a file whole.
    Write {
        /// Where.
        path: PathBuf,
        /// What.
        text: String,
    },
    /// Remove a file; one that is not there is fine.
    Remove(PathBuf),
    /// Run a command. A failure of a `required` one ends the plan; any other
    /// is reported and the plan goes on.
    Run {
        /// What it is for, in the words of a progress line.
        why: &'static str,
        /// The command.
        command: CommandSpec,
        /// Whether the plan depends on it.
        required: bool,
    },
}

/// The ordered actions of one command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// In order.
    pub actions: Vec<Action>,
}

fn systemctl(arguments: &[&str]) -> CommandSpec {
    CommandSpec::new("systemctl")
        .arg("--user")
        .args(arguments.iter().copied())
}

fn launchctl(arguments: &[&str]) -> CommandSpec {
    CommandSpec::new("launchctl").args(arguments.iter().copied())
}

fn run(why: &'static str, command: CommandSpec, required: bool) -> Action {
    Action::Run {
        why,
        command,
        required,
    }
}

/// Installs the service and starts it; installing again replaces the
/// definition and restarts the host, so a changed relay or name takes effect.
/// `linger` also asks for the user manager to outlive the last logout.
pub fn install(service: &Service, layout: &Layout, uid: u32, linger: bool) -> Plan {
    let mut actions = Vec::new();
    if let Some(dir) = layout.definition.parent() {
        actions.push(Action::MakeDir(dir.to_path_buf()));
    }
    match layout.os {
        Os::Linux => {
            actions.push(Action::Write {
                path: layout.definition.clone(),
                text: systemd_unit(service),
            });
            actions.push(run("reload systemd", systemctl(&["daemon-reload"]), true));
            actions.push(run(
                "start it at every login",
                systemctl(&["enable", UNIT_NAME]),
                true,
            ));
            actions.push(run(
                "start it now",
                systemctl(&["restart", UNIT_NAME]),
                true,
            ));
            if linger {
                actions.push(run(
                    "keep it running when you are logged out",
                    CommandSpec::new("loginctl").arg("enable-linger"),
                    false,
                ));
            }
        }
        Os::MacOs => {
            if let Some(dir) = layout.log.as_deref().and_then(Path::parent) {
                actions.push(Action::MakeDir(dir.to_path_buf()));
            }
            let log = layout.log.clone().unwrap_or_default();
            actions.push(Action::Write {
                path: layout.definition.clone(),
                text: launchd_plist(service, &layout.label, &log),
            });
            let target = layout.target(uid);
            let domain = format!("gui/{uid}");
            let definition = layout.definition.to_string_lossy().into_owned();
            actions.push(run(
                "stop an earlier copy",
                launchctl(&["bootout", &target]),
                false,
            ));
            actions.push(run(
                "allow it to load",
                launchctl(&["enable", &target]),
                false,
            ));
            actions.push(run(
                "load it and start it",
                launchctl(&["bootstrap", &domain, &definition]),
                true,
            ));
        }
    }
    Plan { actions }
}

/// The command that stops the service and stops it starting by itself. The
/// definition is removed only after this is known to have worked: see
/// [`remove`].
pub fn stop(layout: &Layout, uid: u32) -> CommandSpec {
    match layout.os {
        Os::Linux => systemctl(&["disable", "--now", UNIT_NAME]),
        Os::MacOs => launchctl(&["bootout", &layout.target(uid)]),
    }
}

/// Removes the definition, once the service is stopped. It touches nothing
/// else: the pairings, the identity, the settings and the log stay, and so
/// does the lingering a person may have asked for.
pub fn remove(layout: &Layout) -> Plan {
    let mut actions = vec![Action::Remove(layout.definition.clone())];
    if layout.os == Os::Linux {
        actions.push(run("reload systemd", systemctl(&["daemon-reload"]), false));
        actions.push(run(
            "forget a failure it left",
            systemctl(&["reset-failed", UNIT_NAME]),
            false,
        ));
    }
    Plan { actions }
}

/// The command that tells what the manager knows of the service.
pub fn probe(layout: &Layout, uid: u32) -> CommandSpec {
    match layout.os {
        Os::Linux => systemctl(&[
            "show",
            UNIT_NAME,
            "--property=LoadState,ActiveState,SubState,UnitFileState,MainPID,ActiveEnterTimestamp",
        ]),
        Os::MacOs => launchctl(&["print", &layout.target(uid)]),
    }
}

/// The command that lists which agents of this user launchd has switched off
/// (macOS only; systemd's `show` already says whether the unit is enabled).
pub fn enabled_probe(layout: &Layout, uid: u32) -> Option<CommandSpec> {
    (layout.os == Os::MacOs).then(|| launchctl(&["print-disabled", &format!("gui/{uid}")]))
}

/// The command that tells whether this user's manager outlives the last
/// logout (Linux only).
pub fn linger_probe(layout: &Layout, uid: u32) -> Option<CommandSpec> {
    (layout.os == Os::Linux).then(|| {
        CommandSpec::new("loginctl").args(["show-user", &uid.to_string(), "--property=Linger"])
    })
}

/// The command that prints the last `lines` lines of the service's log, where
/// the system keeps one (Linux: the journal). macOS writes a file instead.
pub fn logs(layout: &Layout, lines: usize) -> Option<CommandSpec> {
    (layout.os == Os::Linux).then(|| {
        CommandSpec::new("journalctl").args([
            "--user",
            "--unit",
            UNIT_NAME,
            "--lines",
            &lines.to_string(),
            "--no-pager",
        ])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_ID: &str = "dev.zavu.leon";

    fn service() -> Service {
        Service {
            exe: PathBuf::from("/usr/bin/leon"),
            data_dir: PathBuf::from("/home/ana/.local/share/leon"),
            relay: None,
            name: None,
            path_env: None,
        }
    }

    fn linux() -> Layout {
        Layout::new(
            Os::Linux,
            Path::new("/home/ana"),
            Path::new("/home/ana/.config"),
            APP_ID,
        )
    }

    fn mac() -> Layout {
        Layout::new(
            Os::MacOs,
            Path::new("/Users/ana"),
            Path::new("/Users/ana/Library/Application Support"),
            APP_ID,
        )
    }

    fn command_line(command: &CommandSpec) -> String {
        format!("{} {}", command.program, command.args.join(" "))
    }

    /// The commands of a plan as one line each.
    fn commands(plan: &Plan) -> Vec<String> {
        plan.actions
            .iter()
            .filter_map(|action| match action {
                Action::Run { command, .. } => Some(command_line(command)),
                _ => None,
            })
            .collect()
    }

    fn required(plan: &Plan) -> Vec<bool> {
        plan.actions
            .iter()
            .filter_map(|action| match action {
                Action::Run { required, .. } => Some(*required),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_files_are_in_the_users_own_directories() {
        assert_eq!(
            linux().definition,
            Path::new("/home/ana/.config/systemd/user/leon-host.service")
        );
        assert_eq!(
            mac().definition,
            Path::new("/Users/ana/Library/LaunchAgents/dev.zavu.leon.host.plist")
        );
        assert_eq!(
            mac().log.as_deref(),
            Some(Path::new("/Users/ana/Library/Logs/Leon/host.log"))
        );
        assert_eq!(linux().log, None);
    }

    #[test]
    fn installing_on_linux_writes_the_unit_then_enables_and_starts_it() {
        let plan = install(&service(), &linux(), 1000, false);
        assert_eq!(
            plan.actions[..2],
            [
                Action::MakeDir(PathBuf::from("/home/ana/.config/systemd/user")),
                Action::Write {
                    path: PathBuf::from("/home/ana/.config/systemd/user/leon-host.service"),
                    text: systemd_unit(&service()),
                },
            ]
        );
        assert_eq!(
            commands(&plan),
            [
                "systemctl --user daemon-reload",
                "systemctl --user enable leon-host.service",
                "systemctl --user restart leon-host.service",
            ]
        );
        assert_eq!(required(&plan), [true, true, true]);
    }

    #[test]
    fn lingering_is_asked_for_only_when_it_was_requested_and_never_blocks() {
        let without = commands(&install(&service(), &linux(), 1000, false));
        assert!(!without.iter().any(|c| c.contains("linger")));
        let with = install(&service(), &linux(), 1000, true);
        assert_eq!(
            commands(&with).last().map(String::as_str),
            Some("loginctl enable-linger")
        );
        assert_eq!(
            required(&with).last(),
            Some(&false),
            "a refused lingering leaves the service installed"
        );
    }

    #[test]
    fn every_systemd_command_talks_to_the_user_manager() {
        for plan in [install(&service(), &linux(), 1000, true), remove(&linux())] {
            for line in commands(&plan) {
                assert!(
                    line.starts_with("systemctl --user ") || line == "loginctl enable-linger",
                    "{line}"
                );
            }
        }
    }

    // Unix paths: absolute there, relative on Windows.
    #[cfg(leon_posix_tests)]
    #[test]
    fn installing_on_macos_loads_the_agent_into_the_users_domain() {
        let plan = install(&service(), &mac(), 501, false);
        assert_eq!(
            commands(&plan),
            [
                "launchctl bootout gui/501/dev.zavu.leon.host",
                "launchctl enable gui/501/dev.zavu.leon.host",
                "launchctl bootstrap gui/501 /Users/ana/Library/LaunchAgents/dev.zavu.leon.host.plist",
            ]
        );
        assert!(plan.actions.contains(&Action::MakeDir(PathBuf::from(
            "/Users/ana/Library/Logs/Leon"
        ))));
        // The old copy may not be loaded: that failure is fine. The load is not.
        assert_eq!(required(&plan), [false, false, true]);
    }

    #[test]
    fn uninstalling_stops_first_and_then_removes_only_the_definition() {
        assert_eq!(
            command_line(&stop(&linux(), 1000)),
            "systemctl --user disable --now leon-host.service"
        );
        let plan = remove(&linux());
        assert_eq!(
            commands(&plan),
            [
                "systemctl --user daemon-reload",
                "systemctl --user reset-failed leon-host.service",
            ]
        );
        let removed: Vec<_> = plan
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::Remove(path) => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(removed, [linux().definition]);
        assert_eq!(required(&plan), [false, false]);

        assert_eq!(
            command_line(&stop(&mac(), 501)),
            "launchctl bootout gui/501/dev.zavu.leon.host"
        );
        let plan = remove(&mac());
        assert!(commands(&plan).is_empty());
        assert_eq!(plan.actions, [Action::Remove(mac().definition.clone())]);
    }

    #[test]
    fn the_questions_for_status_and_logs_are_the_managers_own() {
        let show = probe(&linux(), 1000);
        assert_eq!(show.program, "systemctl");
        assert_eq!(&show.args[..3], ["--user", "show", "leon-host.service"]);
        assert_eq!(
            probe(&mac(), 501).args,
            ["print", "gui/501/dev.zavu.leon.host"]
        );
        assert_eq!(
            linger_probe(&linux(), 1000).unwrap().args,
            ["show-user", "1000", "--property=Linger"]
        );
        assert!(linger_probe(&mac(), 501).is_none());
        assert_eq!(
            enabled_probe(&mac(), 501).unwrap().args,
            ["print-disabled", "gui/501"]
        );
        assert!(enabled_probe(&linux(), 1000).is_none());
        assert_eq!(
            logs(&linux(), 50).unwrap().args,
            [
                "--user",
                "--unit",
                "leon-host.service",
                "--lines",
                "50",
                "--no-pager"
            ]
        );
        assert!(logs(&mac(), 50).is_none());
    }
}

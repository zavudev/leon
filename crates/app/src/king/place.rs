//! The machines King's workers work on: this computer, one reached over
//! SSH, or one reached through a relay.
//!
//! A worker of a project on another machine works there: its worktree is
//! made there, its agent runs there (through [`Reach`], as the window's
//! conversations do), and `git`, `gh` and the project's check run there on
//! its folder. [`Machines`] is what the service holds for that: the options
//! `ssh` runs with, the connections to paired computers and the runtime they
//! are carried on. A [`Place`] is one machine with what reaching it takes:
//! a command on it runs in a folder, and is waited for.
//!
//! On this computer and over SSH a command is a process started here (`ssh`
//! running it on the other machine); through a relay it is sent to the
//! other computer's host, which runs it and answers with what it printed.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use leon_core::{Machine, MachineId, MachineKind};
use leon_remote::{
    run_on, CommandSpec, ProcessRunner, RelayHub, RoutingRunner, Runner, SshOptions,
};

use crate::chat::reach::Reach;
use crate::chat::turn::Spawner;

/// How long a command on a machine may take, but a check.
const COMMAND_TIME_LIMIT: Duration = Duration::from_secs(120);

/// What reaching the other machines takes.
pub struct Machines {
    ssh: SshOptions,
    hub: Arc<RelayHub>,
    runtime: tokio::runtime::Runtime,
}

impl Machines {
    /// With this installation's keys (its `identity.key` in `data_dir`), as
    /// the window reaches them.
    pub fn new(data_dir: &Path) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("leon-king-machines")
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let identity = leon_link::Identity::load_or_create(data_dir)
            .map_err(|error| format!("the identity could not be read: {error}"))?;
        let hub = RelayHub::new(
            Arc::new(identity),
            leon_host::cli::computer_name(),
            runtime.handle().clone(),
        );
        let runtime_dir = crate::product::runtime_dir();
        let ssh = match crate::product::ensure_private_dir(&runtime_dir) {
            Ok(()) => SshOptions::for_this_platform(runtime_dir),
            Err(_) => SshOptions::without_multiplexing(),
        };
        Ok(Self { ssh, hub, runtime })
    }

    /// What starts a turn on the machine it is for.
    pub fn spawner(&self) -> Arc<dyn Spawner> {
        Arc::new(Reach::new(
            self.ssh.clone(),
            self.hub.clone(),
            self.runtime.handle().clone(),
        ))
    }

    /// `machine`, with what reaching it takes.
    pub fn place(&self, machine: Machine) -> Place {
        Place {
            machine,
            ssh: self.ssh.clone(),
            relay: Some((self.hub.clone(), self.runtime.handle().clone())),
        }
    }
}

/// One machine, and what reaching it takes.
#[derive(Clone)]
pub struct Place {
    machine: Machine,
    ssh: SshOptions,
    relay: Option<(Arc<RelayHub>, tokio::runtime::Handle)>,
}

impl Place {
    /// This computer.
    pub fn here() -> Self {
        Self {
            machine: this_computer(),
            ssh: SshOptions::without_multiplexing(),
            relay: None,
        }
    }

    /// The machine.
    pub fn machine(&self) -> &Machine {
        &self.machine
    }

    /// Whether it is this computer.
    pub fn is_here(&self) -> bool {
        self.machine.kind == MachineKind::Local
    }

    /// Leon's id of the machine, when it is not this computer.
    pub fn id(&self) -> Option<String> {
        (!self.is_here()).then(|| self.machine.id.as_str().to_owned())
    }

    /// What starts `program` with `args` in `cwd` on the machine, from here.
    pub fn placed(&self, program: &str, args: &[&str], cwd: &str) -> CommandSpec {
        let command = CommandSpec::new(program)
            .args(args.iter().copied())
            .cwd(cwd);
        run_on(&self.machine, &command, &self.ssh)
    }

    /// Runs `program` with `args` in `cwd` and gives what it printed, or
    /// what it said on failing.
    pub fn run(&self, program: &str, args: &[&str], cwd: &str) -> Result<String, String> {
        let output = self.output(&self.placed(program, args, cwd), COMMAND_TIME_LIMIT)?;
        if output.success() {
            Ok(output.stdout)
        } else if output.stderr.trim().is_empty() {
            Err(output.stdout.trim().to_owned())
        } else {
            Err(output.stderr.trim().to_owned())
        }
    }

    /// Whether `name` is in the folder `cwd`.
    pub fn exists(&self, cwd: &str, name: &str) -> bool {
        if self.is_here() {
            return Path::new(cwd).join(name).exists();
        }
        self.run("test", &["-e", name], cwd).is_ok()
    }

    /// The file `name` of the folder `cwd`, as text.
    pub fn read(&self, cwd: &str, name: &str) -> Option<String> {
        if self.is_here() {
            return std::fs::read_to_string(Path::new(cwd).join(name)).ok();
        }
        self.run("cat", &[name], cwd).ok()
    }

    /// Whether `path` is a folder on the machine.
    pub fn is_folder(&self, path: &str) -> bool {
        if self.is_here() {
            return Path::new(path).is_dir();
        }
        self.run("test", &["-d", path], "/").is_ok()
    }

    /// Runs `placed` (what [`Self::placed`] gives), for at most `limit`.
    pub fn output(
        &self,
        placed: &CommandSpec,
        limit: Duration,
    ) -> Result<leon_remote::Output, String> {
        if placed.route.is_some() {
            let Some((hub, handle)) = &self.relay else {
                return Err(format!("{} cannot be reached from here", self.machine.name));
            };
            let runner =
                RoutingRunner::new(ProcessRunner::new(), hub.clone()).with_time_limit(limit);
            return handle
                .block_on(runner.run(placed))
                .map_err(|error| format!("{}: {error}", self.machine.name));
        }
        let output = process(placed)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("{} could not run: {error}", placed.program))?;
        Ok(leon_remote::Output {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// The process to start here for `placed`, its input closed.
pub fn process(placed: &CommandSpec) -> std::process::Command {
    let mut command = leon_remote::spawn::std_child(&placed.program);
    command
        .args(&placed.args)
        .envs(placed.env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null());
    if let Some(cwd) = &placed.cwd {
        command.current_dir(cwd);
    }
    command
}

/// This computer, as the store names it.
pub fn this_computer() -> Machine {
    Machine {
        id: MachineId::local(),
        name: "This computer".to_owned(),
        kind: MachineKind::Local,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn over_ssh() -> Place {
        Place {
            machine: Machine {
                id: MachineId::from_string("m-build"),
                name: "build".to_owned(),
                kind: MachineKind::Ssh {
                    host: "build.lan".to_owned(),
                    user: Some("dev".to_owned()),
                    port: None,
                    identity_file: None,
                },
            },
            ssh: SshOptions::without_multiplexing(),
            relay: None,
        }
    }

    #[test]
    fn a_command_runs_here_in_its_folder_and_over_ssh_on_its_machine() {
        let folder = tempfile::tempdir().unwrap();
        let cwd = folder.path().to_str().unwrap();
        std::fs::write(folder.path().join("Cargo.toml"), "[package]").unwrap();
        let here = Place::here();
        assert!(here.is_here() && here.id().is_none());
        assert_eq!(here.run("pwd", &[], cwd).unwrap().trim(), cwd);
        assert!(here.exists(cwd, "Cargo.toml") && !here.exists(cwd, "package.json"));
        assert_eq!(here.read(cwd, "Cargo.toml").as_deref(), Some("[package]"));
        assert!(
            here.run("sh", &["-c", "echo no >&2; exit 1"], cwd)
                .unwrap_err()
                == "no"
        );

        let ssh = over_ssh();
        assert_eq!(ssh.id().as_deref(), Some("m-build"));
        let placed = ssh.placed("git", &["status", "--short"], "/srv/api");
        assert_eq!(placed.program, "ssh");
        assert!(placed.cwd.is_none(), "the folder is the other machine's");
        let remote = placed.args.last().unwrap();
        assert!(
            remote.contains("/srv/api") && remote.contains("git"),
            "{remote}"
        );
        assert!(placed.args.iter().any(|arg| arg == "build.lan"));
    }

    #[test]
    fn a_relayed_command_needs_the_connections_of_the_service() {
        let relayed = Place {
            machine: Machine {
                id: MachineId::from_string("m-mac"),
                name: "mac".to_owned(),
                kind: MachineKind::Relay {
                    host_id: "h".to_owned(),
                    host_key: "00".to_owned(),
                    relay_url: "wss://relay".to_owned(),
                    name: "mac".to_owned(),
                },
            },
            ssh: SshOptions::without_multiplexing(),
            relay: None,
        };
        let placed = relayed.placed("git", &["status"], "/Users/me/api");
        assert!(placed.route.is_some());
        assert!(relayed.run("git", &["status"], "/Users/me/api").is_err());
    }
}

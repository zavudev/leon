//! Machines reached through a relay.
//!
//! [`run_on`](crate::run_on) marks a command for a relay machine with the
//! machine's host id (`CommandSpec::route`) and leaves it otherwise as written.
//! [`RoutingRunner`] is the one place that then decides where a command
//! starts: on this computer for an unmarked command, through the machine's
//! durable connection for a marked one. Git, probing, icons, the process scan
//! and discovery all keep working unchanged on top of it.
//!
//! [`RelayHub`] owns the connections: one [`Client`] per paired host, started
//! when the machine is first used, reconnecting by itself, its state readable
//! for the machine LED and the "Why is it offline?" diagnosis.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use leon_link::client::{Client, ClientConfig, ClientError, ConnState, RelayDialer};
use leon_link::Identity;
use leon_wire::{ErrorCode, ExecSpec, HostId};

use crate::command::CommandSpec;
use crate::runner::{Output, ProcessRunner, RunError, Runner};

/// The route of a command for a relay machine: the host's id, its pinned key
/// and the relay, so the runner can reach it without knowing the machine.
pub fn route(host_id: &str, host_key_hex: &str, relay_url: &str) -> String {
    format!("{host_id} {host_key_hex} {relay_url}")
}

/// The pieces of a [`route`].
pub fn split_route(route: &str) -> Option<(&str, &str, &str)> {
    let mut parts = route.splitn(3, ' ');
    Some((parts.next()?, parts.next()?, parts.next()?))
}

/// The connections to every relay machine.
pub struct RelayHub {
    identity: Arc<Identity>,
    device_name: String,
    handle: tokio::runtime::Handle,
    clients: Mutex<HashMap<String, (String, Client)>>,
}

impl std::fmt::Debug for RelayHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayHub")
            .field("device", &self.identity.device_id())
            .finish_non_exhaustive()
    }
}

/// Why a machine's connection could not be set up.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HubError {
    /// The stored host id or key is not valid.
    #[error("the machine's stored identity is damaged")]
    BadIdentity,
}

fn parse_key(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *slot = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

/// Hex of a key, as stored in a machine.
pub fn key_hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

impl RelayHub {
    /// A hub for this installation's `identity`; connections run on `handle`.
    pub fn new(
        identity: Arc<Identity>,
        device_name: impl Into<String>,
        handle: tokio::runtime::Handle,
    ) -> Arc<Self> {
        Arc::new(Self {
            identity,
            device_name: device_name.into(),
            handle,
            clients: Mutex::new(HashMap::new()),
        })
    }

    /// This installation's identity.
    pub fn identity(&self) -> &Arc<Identity> {
        &self.identity
    }

    /// The connection to a machine, started now if there is none (or if the
    /// relay it should use changed).
    pub fn ensure(
        &self,
        host_id: &str,
        host_key_hex: &str,
        relay_url: &str,
    ) -> Result<Client, HubError> {
        let id: HostId = host_id.parse().map_err(|_| HubError::BadIdentity)?;
        let key = parse_key(host_key_hex).ok_or(HubError::BadIdentity)?;
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((url, client)) = clients.get(host_id) {
            if url == relay_url {
                return Ok(client.clone());
            }
            client.close();
        }
        let _enter = self.handle.enter();
        let client = Client::start(ClientConfig::new(
            self.identity.clone(),
            key,
            Arc::new(RelayDialer {
                url: relay_url.to_owned(),
                host_id: id,
                token: None,
            }),
            self.device_name.clone(),
        ));
        clients.insert(host_id.to_owned(), (relay_url.to_owned(), client.clone()));
        Ok(client)
    }

    /// The connection a [`route`] names, started now if there is none.
    pub fn ensure_route(&self, route: &str) -> Result<Client, HubError> {
        let (host_id, key, url) = split_route(route).ok_or(HubError::BadIdentity)?;
        self.ensure(host_id, key, url)
    }

    /// The connection to a machine, if one was started.
    pub fn client(&self, host_id: &str) -> Option<Client> {
        self.clients
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(host_id)
            .map(|(_, client)| client.clone())
    }

    /// The state of a machine's connection.
    pub fn state(&self, host_id: &str) -> Option<ConnState> {
        self.client(host_id).map(|client| client.state())
    }

    /// Closes and forgets a machine's connection (the machine was removed).
    pub fn forget(&self, host_id: &str) {
        if let Some((_, client)) = self
            .clients
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(host_id)
        {
            client.close();
        }
    }
}

/// Starts commands: here for the unmarked, through a relay for the marked.
#[derive(Debug, Clone)]
pub struct RoutingRunner {
    local: ProcessRunner,
    hub: Arc<RelayHub>,
    limit: Option<Duration>,
}

impl RoutingRunner {
    /// A runner over `local` and `hub`.
    pub fn new(local: ProcessRunner, hub: Arc<RelayHub>) -> Self {
        Self {
            local,
            hub,
            limit: None,
        }
    }

    /// Stops a relayed command after `limit`.
    pub fn with_time_limit(mut self, limit: Duration) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// What a person should be told about a connection that is not online, in
/// terms of what they can act on; `None` when it is.
pub fn describe(state: &ConnState, relay_url: &str) -> Option<String> {
    use leon_link::client::Failure;
    match state {
        ConnState::Offline {
            failure, reason, ..
        } => Some(match failure {
            Failure::RelayUnreachable => format!(
                "Cannot reach the relay at {relay_url} ({reason}). Check your internet connection; if the address is right, the relay service may not be running."
            ),
            Failure::HostOffline => {
                "The other computer is not connected to the relay: it is off, asleep, or Leon is not sharing it.".to_owned()
            }
            Failure::Revoked | Failure::VersionMismatch | Failure::Other => reason.clone(),
        }),
        ConnState::Connecting => Some("Still connecting to the other computer.".to_owned()),
        ConnState::Closed => Some("The connection was closed.".to_owned()),
        ConnState::Online => None,
    }
}

fn unavailable(program: &str, why: impl Into<String>) -> RunError {
    RunError::Spawn {
        program: program.to_owned(),
        source: io::Error::new(io::ErrorKind::NotConnected, why.into()),
    }
}

impl Runner for RoutingRunner {
    async fn run(&self, spec: &CommandSpec) -> Result<Output, RunError> {
        let Some(route) = &spec.route else {
            return self.local.run(spec).await;
        };
        let client = self
            .hub
            .ensure_route(route)
            .map_err(|error| unavailable(&spec.program, error.to_string()))?;
        let wire = ExecSpec {
            program: spec.program.clone(),
            args: spec.args.clone(),
            env: spec.env.clone(),
            cwd: spec.cwd.clone(),
        };
        let timeout_ms = self
            .limit
            .map(|limit| limit.as_millis().min(u128::from(u32::MAX)) as u32);
        match client.exec(wire, timeout_ms).await {
            Ok(out) => Ok(Output {
                status: out.status,
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
            Err(ClientError::Remote(error)) if error.code == ErrorCode::Timeout => {
                Err(RunError::TimedOut {
                    program: spec.program.clone(),
                    limit: self.limit.unwrap_or_default(),
                })
            }
            Err(ClientError::Remote(error)) => Err(unavailable(&spec.program, error.message)),
            Err(other) => {
                let url = split_route(route).map_or("", |(_, _, url)| url);
                Err(unavailable(
                    &spec.program,
                    describe(&client.state(), url).unwrap_or_else(|| other.to_string()),
                ))
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use leon_core::{Machine, MachineId, MachineKind};
    use leon_host::{Host, HostConfig, RelayState};
    use leon_link::pairing::PairedHost;
    use leon_link::relay_client::{dial, Target};
    use leon_link::test_support::TestRelay;
    use leon_link::{pair_as_client, PairingCode};

    async fn world() -> (TestRelay, Host, Arc<RelayHub>, PairedHost, String) {
        let relay = TestRelay::start().await;
        let url = relay.url();
        let host_identity = Arc::new(Identity::generate());
        let host = Host::start(HostConfig::new(&url, "box"), host_identity, None).unwrap();
        let mut state = host.watch_relay();
        while *state.borrow_and_update() != RelayState::Online {
            state.changed().await.unwrap();
        }
        let me = Arc::new(Identity::generate());
        let info = host.new_pairing_code().await;
        let pipe = dial(&url, &Target::Room(info.room), None).await.unwrap();
        let paired = pair_as_client(pipe, &me, &PairingCode::parse(&info.code).unwrap(), "me")
            .await
            .unwrap();
        let hub = RelayHub::new(me, "me", tokio::runtime::Handle::current());
        (relay, host, hub, paired, url)
    }

    fn machine(paired: &PairedHost, url: &str) -> Machine {
        Machine {
            id: MachineId::from_string("m"),
            name: "box".into(),
            kind: MachineKind::Relay {
                host_id: paired.host_id.to_string(),
                host_key: key_hex(&paired.host_key),
                relay_url: url.into(),
                name: "box".into(),
            },
        }
    }

    #[tokio::test]
    async fn a_command_for_a_relay_machine_is_marked_with_its_host() {
        let (_relay, _host, _hub, paired, url) = world().await;
        let machine = machine(&paired, &url);
        let placed = crate::run_on(
            &machine,
            &CommandSpec::new("git").arg("status"),
            &crate::SshOptions::without_multiplexing(),
        );
        assert_eq!(placed.program, "git");
        assert_eq!(placed.args, ["status"]);
        let route = placed.route.unwrap();
        let (id, _key, relay) = split_route(&route).unwrap();
        assert_eq!(id, paired.host_id.to_string());
        assert_eq!(relay, url);
    }

    #[tokio::test]
    async fn the_runner_sends_marked_commands_to_the_host_and_runs_the_rest_here() {
        let (_relay, _host, hub, paired, url) = world().await;
        let machine = machine(&paired, &url);
        let MachineKind::Relay {
            host_id,
            host_key,
            relay_url,
            ..
        } = &machine.kind
        else {
            unreachable!()
        };
        let client = hub.ensure(host_id, host_key, relay_url).unwrap();
        client.wait_online(Duration::from_secs(20)).await.unwrap();
        let runner = RoutingRunner::new(ProcessRunner::new(), hub.clone());

        let remote = crate::run_on(
            &machine,
            &CommandSpec::new("/bin/sh").args(["-c", "echo from-the-host; exit 3"]),
            &crate::SshOptions::without_multiplexing(),
        );
        let out = runner.run(&remote).await.unwrap();
        assert_eq!(
            (out.status, out.stdout.as_str()),
            (Some(3), "from-the-host\n")
        );

        let local = runner
            .run(&CommandSpec::new("/bin/echo").arg("here"))
            .await
            .unwrap();
        assert_eq!(local.stdout, "here\n");
    }

    #[tokio::test]
    async fn a_marked_command_for_an_unknown_machine_fails_cleanly() {
        let (_relay, _host, hub, ..) = world().await;
        let runner = RoutingRunner::new(ProcessRunner::new(), hub);
        let mut spec = CommandSpec::new("true");
        spec.route = Some("NOSUCHHOST".into());
        assert!(matches!(
            runner.run(&spec).await,
            Err(RunError::Spawn { .. })
        ));
    }

    #[tokio::test]
    async fn a_damaged_stored_identity_is_refused() {
        let (_relay, _host, hub, ..) = world().await;
        assert_eq!(
            hub.ensure("nope", "zz", "ws://x").err(),
            Some(HubError::BadIdentity)
        );
    }
}

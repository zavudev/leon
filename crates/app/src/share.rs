//! Sharing this computer from inside the application.
//!
//! "Share this machine" runs the host service of `leon-host` in this process,
//! on the engine's runtime, for as long as Leon is open and the switch is on:
//! closing Leon stops sharing and hangs up the terminals it was serving. A host
//! that outlives the window is the separate background service of
//! `leon host service install` (`host_service`), which runs `leon host` under
//! systemd or launchd.
//!
//! What this installation's own Leon shares with its paired devices is read
//! straight from this process's store ([`StoreShare`]): the projects and
//! history sessions of this machine, so a paired computer's Leon can show
//! them in its own sidebar. Sharing adds no rights — the device already holds
//! a full terminal as this user — and the store is read fresh at each ask, so
//! what is shared is never a stale snapshot.
//!
//! The screen reads this service on a timer: the relay's state, the code on
//! offer with its countdown, the paired devices, and any device waiting for the
//! person's approval.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use leon_core::{AgentId, MachineId, SessionFilter, Store};
use leon_host::{
    Approval, ApprovalRequest, Host, HostConfig, HostStatus, PairingInfo, ShareSource,
};
use leon_link::{DeviceRecord, Identity};
use leon_wire::{SharedEntry, SharedProject, SharedSession, SharedTranscript};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

/// What the person is warned of, in the screen and in the docs.
pub const RISK: &str = "Anyone you pair gets a terminal as you on this computer: they can read, change and delete everything you can. Pair only your own devices, and revoke any you lose.";

/// How many history sessions one [`StoreShare`] offers; the host cuts harder
/// if it must ([`leon_wire::SHARE_SESSIONS_LIMIT`]) and a client asks for
/// others' transcripts by their `(agent, external_id)` when it needs to.
pub const OWN_SESSIONS_OFFERED: usize = 500;

/// What this Leon knows of this machine, as paired devices may read it.
///
/// Every read is fresh and every failure shares nothing: the paired device
/// holds a terminal as this user, so an empty answer changes nothing for it,
/// and no error it may cause is worth a protocol message.
struct StoreShare {
    store: Arc<Store>,
}

impl ShareSource for StoreShare {
    fn projects(&self) -> Vec<SharedProject> {
        self.store
            .projects(Some(&MachineId::local()))
            .unwrap_or_default()
            .into_iter()
            .map(|project| SharedProject {
                name: project.name,
                root: project.root,
            })
            .collect()
    }

    fn sessions(&self) -> Vec<SharedSession> {
        let filter = SessionFilter {
            machine_id: Some(MachineId::local()),
            ..Default::default()
        };
        self.store
            .recent_sessions(&filter, OWN_SESSIONS_OFFERED)
            .unwrap_or_default()
            .into_iter()
            .map(|session| SharedSession {
                agent: session.agent.as_str().to_owned(),
                external_id: session.external_id,
                cwd: session.cwd,
                title: session.title,
                model: session.model,
                started_ms: session.started_at.timestamp_millis(),
                updated_ms: session.updated_at.timestamp_millis(),
                messages: session.message_count,
            })
            .collect()
    }

    fn transcript(&self, agent: &str, external_id: &str) -> Option<SharedTranscript> {
        let agent = AgentId::parse(agent)?;
        let session = self
            .store
            .session_by_external(&MachineId::local(), agent, external_id)
            .ok()
            .flatten()?;
        let messages = self.store.session_messages(&session.id).ok()?;
        Some(SharedTranscript {
            agent: agent.as_str().to_owned(),
            external_id: external_id.to_owned(),
            messages: messages
                .into_iter()
                .map(|message| SharedEntry {
                    role: message.role.as_str().to_owned(),
                    text: message.text,
                    at_ms: message.at.timestamp_millis(),
                })
                .collect(),
        })
    }
}

struct Running {
    host: Arc<Host>,
    approvals: mpsc::Receiver<ApprovalRequest>,
}

/// The sharing service of this application.
pub struct ShareService {
    identity: Arc<Identity>,
    handle: Handle,
    devices_file: PathBuf,
    /// What the paired devices may read of this Leon, while it is running.
    share: Arc<dyn ShareSource>,
    running: Mutex<Option<Running>>,
}

impl std::fmt::Debug for ShareService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareService")
            .field("running", &self.is_running())
            .finish_non_exhaustive()
    }
}

impl ShareService {
    /// A service that is off. Device records live in `<data dir>/host`.
    pub fn new(
        identity: Arc<Identity>,
        handle: Handle,
        data_dir: &std::path::Path,
        store: Arc<Store>,
    ) -> Arc<Self> {
        Arc::new(Self {
            identity,
            handle,
            devices_file: data_dir.join("host").join("devices.json"),
            share: Arc::new(StoreShare { store }),
            running: Mutex::new(None),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Running>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether sharing is on.
    pub fn is_running(&self) -> bool {
        self.lock().is_some()
    }

    /// Switches sharing on. Does nothing when it already is.
    pub fn start(
        &self,
        relay_url: &str,
        device_name: &str,
        require_approval: bool,
    ) -> Result<(), String> {
        let mut running = self.lock();
        if running.is_some() {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel(8);
        let mut config = HostConfig::new(relay_url, device_name);
        if require_approval {
            config.approval = Approval::Ask(tx);
        }
        config.share = Some(leon_host::Shared(self.share.clone()));
        let _enter = self.handle.enter();
        let host = Host::start(
            config,
            self.identity.clone(),
            Some(self.devices_file.clone()),
        )
        .map_err(|error| format!("cannot open the list of paired devices: {error}"))?;
        *running = Some(Running {
            host: Arc::new(host),
            approvals: rx,
        });
        Ok(())
    }

    /// Switches sharing off: sessions end and terminals are hung up.
    pub fn stop(&self) {
        if let Some(running) = self.lock().take() {
            running.host.stop();
        }
    }

    /// A snapshot, while sharing is on.
    pub fn status(&self) -> Option<HostStatus> {
        self.lock().as_ref().map(|r| r.host.status())
    }

    /// The code on offer, while sharing is on and it is usable.
    pub fn pairing(&self) -> Option<PairingInfo> {
        self.lock().as_ref().and_then(|r| r.host.pairing())
    }

    /// Replaces the code with a fresh one (in the background).
    pub fn new_code(&self) {
        if let Some(running) = self.lock().as_ref() {
            let host = running.host.clone();
            self.handle.spawn(async move {
                host.new_pairing_code().await;
            });
        }
    }

    /// The paired devices (also when sharing is off).
    pub fn devices(&self) -> Vec<DeviceRecord> {
        match self.lock().as_ref() {
            Some(running) => running.host.devices(),
            None => leon_link::DeviceRegistry::open(&self.devices_file)
                .map(|registry| registry.devices().to_vec())
                .unwrap_or_default(),
        }
    }

    /// Revokes a device by id or name. Works when sharing is off too.
    pub fn revoke(&self, selector: &str) -> Result<DeviceRecord, String> {
        if let Some(running) = self.lock().as_ref() {
            return running.host.revoke(selector).map_err(|e| e.to_string());
        }
        let mut registry = leon_link::DeviceRegistry::open(&self.devices_file)
            .map_err(|error| error.to_string())?;
        registry.revoke(selector).map_err(|error| error.to_string())
    }

    /// A device waiting for the person's answer, if one is.
    pub fn poll_approval(&self) -> Option<ApprovalRequest> {
        self.lock()
            .as_mut()
            .and_then(|r| r.approvals.try_recv().ok())
    }
}

impl Drop for ShareService {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(all(test, leon_posix_tests))]
mod tests {
    use super::*;
    use leon_host::RelayState;
    use leon_link::relay_client::{dial, Target};
    use leon_link::test_support::TestRelay;
    use leon_link::{pair_as_client, PairingCode};
    use std::time::{Duration, Instant};

    async fn until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn service(dir: &std::path::Path) -> Arc<ShareService> {
        let store = leon_core::Store::open_in_memory().unwrap();
        ShareService::new(
            Arc::new(Identity::generate()),
            Handle::current(),
            dir,
            store,
        )
    }

    #[tokio::test]
    async fn sharing_starts_offers_a_code_and_stops() {
        let relay = TestRelay::start().await;
        let dir = tempfile::tempdir().unwrap();
        let share = service(dir.path());
        assert!(!share.is_running());
        share.start(&relay.url(), "box", false).unwrap();
        assert!(share.is_running());
        until("online", || {
            share
                .status()
                .is_some_and(|s| s.relay == RelayState::Online)
        })
        .await;
        share.new_code();
        until("a code", || share.pairing().is_some()).await;
        let info = share.pairing().unwrap();
        assert_eq!(info.code.len(), 12);
        share.stop();
        assert!(!share.is_running());
        assert!(share.pairing().is_none());
    }

    #[tokio::test]
    async fn a_device_waits_for_the_persons_answer_and_can_be_revoked_afterwards() {
        let relay = TestRelay::start().await;
        let dir = tempfile::tempdir().unwrap();
        let share = service(dir.path());
        share.start(&relay.url(), "box", true).unwrap();
        until("online", || {
            share
                .status()
                .is_some_and(|s| s.relay == RelayState::Online)
        })
        .await;
        share.new_code();
        until("a code", || share.pairing().is_some()).await;
        let info = share.pairing().unwrap();

        let me = Identity::generate();
        let url = relay.url();
        let pairing = tokio::spawn(async move {
            let pipe = dial(&url, &Target::Room(info.room), None).await.unwrap();
            pair_as_client(
                pipe,
                &me,
                &PairingCode::parse(&info.code).unwrap(),
                "Ana's laptop",
            )
            .await
        });
        let mut waiting = None;
        until("the request", || {
            waiting = waiting.take().or_else(|| share.poll_approval());
            waiting.is_some()
        })
        .await;
        let ask = waiting.unwrap();
        assert_eq!(ask.request.device_name, "Ana's laptop");
        ask.reply.send(true).unwrap();
        pairing.await.unwrap().unwrap();
        assert_eq!(share.devices().len(), 1);
        share.revoke("Ana's laptop").unwrap();
        assert!(share.devices()[0].revoked);
    }

    #[tokio::test]
    async fn the_device_list_is_readable_and_revocable_while_sharing_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let share = service(dir.path());
        assert!(share.devices().is_empty());
        let mut registry =
            leon_link::DeviceRegistry::open(&dir.path().join("host").join("devices.json")).unwrap();
        registry.authorise([9; 32], "phone", 1).unwrap();
        assert_eq!(share.devices().len(), 1);
        share.revoke("phone").unwrap();
        assert!(share.devices()[0].revoked);
    }

    fn shared(dir: &std::path::Path) -> Arc<StoreShare> {
        Arc::new(StoreShare {
            store: Store::open(dir.join("leon.db")).unwrap(),
        })
    }

    #[tokio::test]
    async fn what_this_leon_shares_is_what_its_store_says_of_this_machine() {
        use chrono::{DateTime, Utc};
        use leon_core::{AgentId, MachineKind, NewMessage, NewSession};
        let dir = tempfile::tempdir().unwrap();
        let shared = shared(dir.path());
        let started = DateTime::<Utc>::from_timestamp_millis(1_700_000_000_000).unwrap();
        let updated = DateTime::<Utc>::from_timestamp_millis(1_700_000_060_000).unwrap();
        let machine = shared
            .store
            .machine(&leon_core::MachineId::local())
            .unwrap();

        // A project of this machine: offered as the host shows it.
        shared
            .store
            .add_project(&machine.id, "api", "/srv/api")
            .unwrap();
        assert_eq!(shared.projects().len(), 1);
        assert_eq!(
            (
                shared.projects()[0].name.as_str(),
                shared.projects()[0].root.as_str()
            ),
            ("api", "/srv/api")
        );

        // A session of this machine, with a transcript.
        let session = NewSession {
            agent: AgentId::CLAUDE,
            external_id: "one".into(),
            machine_id: machine.id.clone(),
            cwd: "/srv/api".into(),
            title: "fix the login bug".into(),
            model: None,
            started_at: started,
            updated_at: updated,
        };
        let messages = [NewMessage {
            role: leon_core::Role::User,
            text: "please fix the login".into(),
            at: started,
        }];
        shared.store.upsert_session(&session, &messages).unwrap();

        let listed = shared.sessions();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].external_id == "one");
        let transcript = shared.transcript("claude", "one").unwrap();
        assert_eq!(transcript.messages.len(), 1);
        assert_eq!(transcript.messages[0].text.as_str(), "please fix the login");
        assert!(shared.transcript("claude", "none").is_none());

        // Another machine's history is nobody else's business: only what
        // this machine holds is offered.
        let other = shared
            .store
            .add_machine(
                "elsewhere",
                MachineKind::Ssh {
                    host: "box".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        let theirs = NewSession {
            agent: AgentId::CLAUDE,
            external_id: "theirs".into(),
            machine_id: other.id.clone(),
            cwd: "/opt/other".into(),
            title: "theirs".into(),
            model: None,
            started_at: started,
            updated_at: updated,
        };
        shared.store.upsert_session(&theirs, &messages).unwrap();
        let listed = shared.sessions();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].external_id == "one");
        assert!(shared.transcript("claude", "theirs").is_none());
    }
}

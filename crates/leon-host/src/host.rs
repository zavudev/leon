//! The host: registers with a relay, serves pairing and sessions, reconnects.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use leon_link::relay_client::{register, DialError, HostLink, Incoming, LinkCommands};
use leon_link::{
    accept, pair_as_host, DeviceRecord, DeviceRegistry, Identity, PairRequest, PairingCode,
    PairingOffer, Pipe, RegistryError,
};
use leon_wire::{HostId, REPLAY_BUFFER_BYTES};
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::ptys::PtyTable;
use crate::session;
use crate::share::ShareSource;

/// Whether the process runs as the superuser (always `false` off Unix).
pub fn running_as_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// A device asking to pair, waiting for the person at this computer.
#[derive(Debug)]
pub struct ApprovalRequest {
    /// Who is asking.
    pub request: PairRequest,
    /// Answer `true` to allow it.
    pub reply: oneshot::Sender<bool>,
}

/// How pairing is approved.
#[derive(Debug, Clone)]
pub enum Approval {
    /// Whoever knows the code is accepted (headless hosts: the code is the
    /// approval).
    Automatic,
    /// Each device is shown to the person at this computer first.
    Ask(mpsc::Sender<ApprovalRequest>),
}

/// Reconnection delays: doubling from `initial` up to `max`.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    /// The first delay.
    pub initial: Duration,
    /// The longest delay.
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(30),
        }
    }
}

/// How a host is set up.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The relay (`wss://…`).
    pub relay_url: String,
    /// The name shown to paired devices.
    pub device_name: String,
    /// How pairing is approved.
    pub approval: Approval,
    /// Output kept per terminal.
    pub ring_bytes: usize,
    /// How long an exited terminal stays listed.
    pub retention: Duration,
    /// Reconnection delays.
    pub backoff: Backoff,
    /// How often the device list file is checked for changes.
    pub registry_poll: Duration,
    /// Most simultaneous sessions.
    pub max_sessions: usize,
    /// Most terminals.
    pub max_ptys: usize,
    /// An opaque token for a relay that requires one.
    pub token: Option<Vec<u8>>,
    /// What this installation's own Leon tells paired devices, when it wants
    /// to: its projects and history sessions, so theirs can show them. `None`
    /// means nothing is shared and the share requests answer with nothing.
    pub share: Option<Shared>,
}

/// What a host shares, kept out of [`HostConfig`]'s `Debug` so the machine's
/// projects and session titles never land in a log.
#[derive(Clone)]
pub struct Shared(pub Arc<dyn ShareSource>);

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShareSource")
    }
}

impl HostConfig {
    /// Defaults for `relay_url` and `device_name`.
    pub fn new(relay_url: impl Into<String>, device_name: impl Into<String>) -> Self {
        Self {
            relay_url: relay_url.into(),
            device_name: device_name.into(),
            approval: Approval::Automatic,
            ring_bytes: REPLAY_BUFFER_BYTES,
            retention: Duration::from_secs(30 * 60),
            backoff: Backoff::default(),
            registry_poll: Duration::from_secs(2),
            max_sessions: 16,
            max_ptys: 64,
            token: None,
            share: None,
        }
    }
}

/// Where the relay connection stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayState {
    /// Connecting for the first time.
    Connecting,
    /// Registered; devices can reach this host.
    Online,
    /// Not connected; trying again.
    Offline {
        /// Why, in a sentence without secrets.
        reason: String,
        /// How many attempts have failed in a row.
        attempt: u32,
    },
    /// Stopped.
    Stopped,
}

/// The pairing code currently on offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingInfo {
    /// The code as shown.
    pub code: String,
    /// Its public room.
    pub room: String,
    /// Time until it expires.
    pub remaining: Duration,
    /// Failed attempts still allowed.
    pub attempts_left: u32,
}

/// A snapshot of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostStatus {
    /// The relay connection.
    pub relay: RelayState,
    /// This host's id.
    pub host_id: HostId,
    /// Sessions open now.
    pub sessions: usize,
    /// Terminals listed.
    pub terminals: usize,
    /// The code on offer, if any and still usable.
    pub pairing: Option<PairingInfo>,
}

pub(crate) struct Inner {
    pub(crate) cfg: HostConfig,
    pub(crate) identity: Arc<Identity>,
    pub(crate) registry: Mutex<DeviceRegistry>,
    registry_path: Option<PathBuf>,
    pub(crate) table: Arc<PtyTable>,
    relay: watch::Sender<RelayState>,
    pairing: Mutex<Option<Arc<PairingOffer>>>,
    commands: Mutex<Option<LinkCommands>>,
    pub(crate) recheck: broadcast::Sender<()>,
    pub(crate) sessions: Arc<AtomicUsize>,
    pub(crate) next_session: AtomicU64,
    stop: watch::Sender<bool>,
}

impl Inner {
    pub(crate) fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn set_relay(&self, state: RelayState) {
        self.relay.send_replace(state);
    }

    fn current_offer(&self) -> Option<Arc<PairingOffer>> {
        self.pairing.lock().clone()
    }

    /// Announces the room of the current offer, choosing a fresh code when
    /// the room is taken by another host.
    async fn announce(&self, commands: &LinkCommands) {
        for _ in 0..6 {
            let Some(offer) = self.current_offer() else {
                return;
            };
            if !matches!(
                offer.status(Instant::now()),
                leon_link::pairing::OfferStatus::Open { .. }
            ) {
                return;
            }
            match commands.open_pairing(&offer.code().room()).await {
                Ok(()) => return,
                Err(DialError::Refused(_)) => {
                    *self.pairing.lock() =
                        Some(Arc::new(PairingOffer::new(PairingCode::generate())));
                }
                Err(_) => return,
            }
        }
    }

    async fn serve_pairing(self: Arc<Self>, pipe: Pipe) {
        let Some(offer) = self.current_offer() else {
            return;
        };
        let approval = self.cfg.approval.clone();
        let approve = move |request: PairRequest| async move {
            match approval {
                Approval::Automatic => true,
                Approval::Ask(ask) => {
                    let (reply, answer) = oneshot::channel();
                    if ask.send(ApprovalRequest { request, reply }).await.is_err() {
                        return false;
                    }
                    matches!(
                        tokio::time::timeout(Duration::from_secs(120), answer).await,
                        Ok(Ok(true))
                    )
                }
            }
        };
        match pair_as_host(pipe, &self.identity, &offer, &self.cfg.device_name, approve).await {
            Ok(device) => {
                let saved = self.registry.lock().authorise(
                    device.device_key,
                    &device.device_name,
                    Self::now_unix(),
                );
                if let Err(error) = saved {
                    tracing::error!(%error, "could not save the paired device");
                }
                tracing::info!(device = %leon_link::DeviceId::of(&device.device_key), "device paired");
                let commands = self.commands.lock().clone();
                if let Some(commands) = commands {
                    commands.close_pairing(&offer.code().room()).await;
                }
            }
            Err(error) => tracing::info!(%error, "pairing attempt failed"),
        }
    }

    async fn serve_incoming(self: Arc<Self>, incoming: Incoming) {
        if incoming.pairing {
            self.serve_pairing(incoming.pipe).await;
            return;
        }
        if self.sessions.load(Ordering::Relaxed) >= self.cfg.max_sessions {
            return;
        }
        let this = self.clone();
        let channel = accept(incoming.pipe, &self.identity, |key| {
            this.registry.lock().is_authorised(key)
        })
        .await;
        match channel {
            Ok(channel) => session::serve(self, channel).await,
            Err(error) => tracing::info!(%error, "connection refused"),
        }
    }

    async fn run(self: Arc<Self>) {
        let mut stop = self.stop.subscribe();
        let mut failures = 0u32;
        let mut delay = self.cfg.backoff.initial;
        loop {
            if *stop.borrow() {
                break;
            }
            let attempt = register(&self.cfg.relay_url, &self.identity, self.cfg.token.clone());
            let registered = tokio::select! {
                result = attempt => result,
                _ = stop.changed() => break,
            };
            match registered {
                Ok(link) => {
                    failures = 0;
                    delay = self.cfg.backoff.initial;
                    self.set_relay(RelayState::Online);
                    tracing::info!(host = %link.host_id().short(), "registered with the relay");
                    if self.serve_link(link, &mut stop).await {
                        break;
                    }
                    self.commands.lock().take();
                    self.set_relay(RelayState::Offline {
                        reason: "the relay connection ended".into(),
                        attempt: 0,
                    });
                }
                Err(error) => {
                    failures += 1;
                    self.set_relay(RelayState::Offline {
                        reason: error.to_string(),
                        attempt: failures,
                    });
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = stop.changed() => break,
            }
            delay = (delay * 2).min(self.cfg.backoff.max);
        }
        self.commands.lock().take();
        self.table.close_all();
        self.set_relay(RelayState::Stopped);
    }

    /// Serves a registered link; `true` when the host was told to stop.
    async fn serve_link(
        self: &Arc<Self>,
        mut link: HostLink,
        stop: &mut watch::Receiver<bool>,
    ) -> bool {
        let commands = link.commands();
        *self.commands.lock() = Some(commands.clone());
        {
            let this = self.clone();
            tokio::spawn(async move { this.announce(&commands).await });
        }
        loop {
            tokio::select! {
                incoming = link.accept() => match incoming {
                    Some(incoming) => {
                        tokio::spawn(self.clone().serve_incoming(incoming));
                    }
                    None => return false,
                },
                _ = stop.changed() => return true,
            }
        }
    }

    async fn maintain(self: Arc<Self>) {
        let mut stop = self.stop.subscribe();
        let mut last = self.registry_stamp();
        let mut reap = tokio::time::interval(Duration::from_secs(30));
        let mut poll = tokio::time::interval(self.cfg.registry_poll);
        loop {
            tokio::select! {
                _ = poll.tick() => {
                    let stamp = self.registry_stamp();
                    if stamp != last {
                        last = stamp;
                        if let Some(path) = &self.registry_path {
                            if let Ok(fresh) = DeviceRegistry::open(path) {
                                *self.registry.lock() = fresh;
                                let _ = self.recheck.send(());
                            }
                        }
                    }
                }
                _ = reap.tick() => { self.table.reap(Instant::now()); }
                _ = stop.changed() => return,
            }
        }
    }

    fn registry_stamp(&self) -> Option<(SystemTime, u64)> {
        let meta = std::fs::metadata(self.registry_path.as_ref()?).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }
}

/// A running host service.
pub struct Host {
    inner: Arc<Inner>,
}

impl Host {
    /// Starts serving: connects to the relay in the background and keeps
    /// reconnecting. The device list lives at `registry_path` (in memory
    /// when `None`); it is re-read when the file changes, so `leon host
    /// revoke` in another process takes effect.
    pub fn start(
        cfg: HostConfig,
        identity: Arc<Identity>,
        registry_path: Option<PathBuf>,
    ) -> Result<Self, RegistryError> {
        let registry = match &registry_path {
            Some(path) => DeviceRegistry::open(path)?,
            None => DeviceRegistry::in_memory(),
        };
        let (relay, _) = watch::channel(RelayState::Connecting);
        let (recheck, _) = broadcast::channel(16);
        let (stop, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            table: PtyTable::new(cfg.ring_bytes, cfg.retention, cfg.max_ptys),
            cfg,
            identity,
            registry: Mutex::new(registry),
            registry_path,
            relay,
            pairing: Mutex::new(None),
            commands: Mutex::new(None),
            recheck,
            sessions: Arc::new(AtomicUsize::new(0)),
            next_session: AtomicU64::new(1),
            stop,
        });
        tokio::spawn(inner.clone().run());
        tokio::spawn(inner.clone().maintain());
        Ok(Self { inner })
    }

    /// This host's id.
    pub fn host_id(&self) -> HostId {
        self.inner.identity.host_id()
    }

    /// The relay connection state, as a stream of changes.
    pub fn watch_relay(&self) -> watch::Receiver<RelayState> {
        self.inner.relay.subscribe()
    }

    /// A snapshot of everything.
    pub fn status(&self) -> HostStatus {
        HostStatus {
            relay: self.inner.relay.borrow().clone(),
            host_id: self.host_id(),
            sessions: self.inner.sessions.load(Ordering::Relaxed),
            terminals: self.inner.table.len(),
            pairing: self.pairing(),
        }
    }

    /// The code on offer, while it is usable.
    pub fn pairing(&self) -> Option<PairingInfo> {
        let offer = self.inner.current_offer()?;
        let now = Instant::now();
        match offer.status(now) {
            leon_link::pairing::OfferStatus::Open { attempts_left } => Some(PairingInfo {
                code: offer.code().display(),
                room: offer.code().room(),
                remaining: offer.remaining(now),
                attempts_left,
            }),
            _ => None,
        }
    }

    /// Why the last offer is no longer usable, if it is not: `Expired`,
    /// `Burned` or `Used`.
    pub fn pairing_state(&self) -> Option<leon_link::pairing::OfferStatus> {
        self.inner
            .current_offer()
            .map(|offer| offer.status(Instant::now()))
    }

    /// Replaces the code on offer with a fresh one and announces it.
    pub async fn new_pairing_code(&self) -> PairingInfo {
        let old = self.inner.current_offer();
        let commands = self.inner.commands.lock().clone();
        if let (Some(old), Some(commands)) = (&old, &commands) {
            commands.close_pairing(&old.code().room()).await;
        }
        *self.inner.pairing.lock() = Some(Arc::new(PairingOffer::new(PairingCode::generate())));
        if let Some(commands) = commands {
            self.inner.announce(&commands).await;
        }
        self.pairing().expect("a fresh code is usable")
    }

    /// Paired devices.
    pub fn devices(&self) -> Vec<DeviceRecord> {
        self.inner.registry.lock().devices().to_vec()
    }

    /// Revokes a device (by id prefix or name); its open sessions end.
    pub fn revoke(&self, selector: &str) -> Result<DeviceRecord, RegistryError> {
        let record = self.inner.registry.lock().revoke(selector)?;
        let _ = self.inner.recheck.send(());
        Ok(record)
    }

    /// Stops the service: sessions end and terminals are hung up.
    pub fn stop(&self) {
        self.inner.stop.send_replace(true);
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
    }
}

//! The host's terminals.
//!
//! A terminal belongs to the host, not to a client: it keeps running when
//! every client is gone. Output goes into a [`Ring`] and to every attached
//! session. Attaching replays from the ring and registers the session for live
//! output under one lock, so a client that resumes from the last offset it saw
//! gets each byte exactly once, in order.
//!
//! A session that cannot keep up (its queue is full) is detached and told to
//! reconnect; it resumes from where it was. An exited terminal stays listed for
//! a retention window, then is reaped.
//!
//! What a terminal runs in is a [`Spawner`]: the real one starts a program in a
//! pseudo-terminal ([`RealSpawner`]); a test hands in a scripted one, so what a
//! terminal prints, who is in front of it and when it ends are decided by the
//! test and not by a shell.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use leon_pty::{Events, Foreground, GridSize, PtyEvent, PtyProcess};
use leon_wire::{ErrorCode, Exit, Grid, Message, PtyInfo, SpawnSpec, WireError, MAX_PTY_CHUNK};
use parking_lot::Mutex;
use tokio::sync::{mpsc, Notify};

use crate::ring::Ring;

/// A running program in a terminal, as the table drives it.
pub trait Process: Send + Sync {
    /// Sends keystrokes.
    fn write(&self, bytes: Vec<u8>);
    /// Resizes the terminal.
    fn resize(&self, size: GridSize);
    /// Hangs the program up.
    fn kill(&self);
    /// The pid of the terminal's own process, where the system has one.
    fn pid(&self) -> Option<u32>;
    /// Who is in front of the terminal; `None` where that cannot be told.
    fn foreground(&self) -> Option<Foreground>;
    /// SIGTERM to the program in front of the shell; `false` when nothing was
    /// sent.
    fn terminate_foreground(&self) -> bool;
}

impl Process for PtyProcess {
    fn write(&self, bytes: Vec<u8>) {
        PtyProcess::write(self, bytes);
    }
    fn resize(&self, size: GridSize) {
        PtyProcess::resize(self, size);
    }
    fn kill(&self) {
        PtyProcess::kill(self);
    }
    fn pid(&self) -> Option<u32> {
        self.process_id()
    }
    fn foreground(&self) -> Option<Foreground> {
        PtyProcess::foreground(self)
    }
    fn terminate_foreground(&self) -> bool {
        PtyProcess::terminate_foreground(self)
    }
}

/// Starts programs in terminals.
pub trait Spawner: Send + Sync {
    /// Starts `spec` in a terminal of `size`. The events end with the
    /// program's exit.
    fn spawn(
        &self,
        spec: &leon_pty::SpawnSpec,
        size: GridSize,
    ) -> Result<(Arc<dyn Process>, Events), String>;
}

/// The real thing: a pseudo-terminal of this computer.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealSpawner {
    /// Whether the request's environment is the child's whole environment
    /// (the keeper), or goes on top of this process's (`leon host`).
    pub own_environment: bool,
}

impl Spawner for RealSpawner {
    fn spawn(
        &self,
        spec: &leon_pty::SpawnSpec,
        size: GridSize,
    ) -> Result<(Arc<dyn Process>, Events), String> {
        let (process, events) = PtyProcess::spawn_with(spec, size, self.own_environment)?;
        Ok((Arc::new(process), events))
    }
}

/// Where a session receives what the terminals say.
#[derive(Clone)]
pub struct Outbox {
    /// Identifies the session, so it can be detached as a whole.
    pub session: u64,
    /// The session's queue of outgoing messages.
    pub tx: mpsc::Sender<Message>,
    /// Woken when this session fell behind and was detached.
    pub lagged: Arc<Notify>,
}

struct Subscriber {
    outbox: Outbox,
}

struct Hosted {
    label: String,
    program: String,
    cwd: Option<String>,
    size: Grid,
    started_unix: u64,
    ring: Ring,
    exit: Option<Exit>,
    exited_at: Option<Instant>,
    process: Option<Arc<dyn Process>>,
    subscribers: Vec<Subscriber>,
}

/// What an attach found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attached {
    /// Where the replay starts.
    pub replay_from: u64,
    /// The client asked for bytes that are gone.
    pub gap: bool,
    /// The offset one past the newest byte now.
    pub end_offset: u64,
    /// The terminal's size.
    pub size: Grid,
    /// How it ended, if it has.
    pub exit: Option<Exit>,
}

fn not_found() -> WireError {
    WireError {
        code: ErrorCode::NotFound,
        message: "no such terminal".into(),
    }
}

fn to_size(grid: Grid) -> GridSize {
    GridSize {
        cols: grid.cols.max(2),
        rows: grid.rows.max(1),
        cell_width: grid.cell_width,
        cell_height: grid.cell_height,
    }
}

fn chunks(pty: u64, from: u64, bytes: &[u8]) -> Vec<Message> {
    let mut offset = from;
    bytes
        .chunks(MAX_PTY_CHUNK)
        .map(|part| {
            let message = Message::PtyData {
                pty,
                offset,
                bytes: part.to_vec(),
            };
            offset += part.len() as u64;
            message
        })
        .collect()
}

/// All the terminals of a host.
pub struct PtyTable {
    inner: Mutex<HashMap<u64, Hosted>>,
    next: AtomicU64,
    ring_bytes: usize,
    retention: Duration,
    max_ptys: usize,
    spawner: Arc<dyn Spawner>,
}

impl PtyTable {
    /// A table whose terminals keep `ring_bytes` of output and stay listed for
    /// `retention` after they exit.
    pub fn new(ring_bytes: usize, retention: Duration, max_ptys: usize) -> Arc<Self> {
        Self::with_spawner(
            ring_bytes,
            retention,
            max_ptys,
            1,
            Arc::new(RealSpawner::default()),
        )
    }

    /// A table that starts its terminals with `spawner` and numbers them from
    /// `first_id`. A keeper starts from a random number, so a terminal id kept
    /// by an app never names a terminal of another keeper that took the place
    /// of one that died.
    pub fn with_spawner(
        ring_bytes: usize,
        retention: Duration,
        max_ptys: usize,
        first_id: u64,
        spawner: Arc<dyn Spawner>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(HashMap::new()),
            next: AtomicU64::new(first_id.max(1)),
            ring_bytes,
            retention,
            max_ptys,
            spawner,
        })
    }

    /// How many terminals have a program running (a listed one that exited
    /// does not count).
    pub fn running(&self) -> usize {
        self.inner
            .lock()
            .values()
            .filter(|h| h.exit.is_none())
            .count()
    }

    /// Who is in front of a terminal: `(pid, foreground)`. Both are `None`
    /// for a terminal that ended or where the system cannot say.
    pub fn probe(&self, pty: u64) -> Result<(Option<u32>, Option<Foreground>), WireError> {
        Ok(match self.process(pty)? {
            Some(process) => (process.pid(), process.foreground()),
            None => (None, None),
        })
    }

    /// Asks the program in front of a terminal's shell to end.
    pub fn terminate_foreground(&self, pty: u64) -> Result<bool, WireError> {
        Ok(self
            .process(pty)?
            .is_some_and(|process| process.terminate_foreground()))
    }

    /// How many terminals are listed.
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Starts a terminal.
    pub fn open(
        self: &Arc<Self>,
        spec: &SpawnSpec,
        size: Grid,
        label: &str,
    ) -> Result<u64, WireError> {
        if self.inner.lock().len() >= self.max_ptys {
            return Err(WireError {
                code: ErrorCode::Busy,
                message: "too many terminals on this host".into(),
            });
        }
        let pty_spec = leon_pty::SpawnSpec {
            program: spec.program.clone(),
            args: spec.args.clone(),
            env: spec.env.clone(),
            cwd: spec.cwd.clone(),
            route: None,
        };
        let (process, events) =
            self.spawner
                .spawn(&pty_spec, to_size(size))
                .map_err(|message| WireError {
                    code: ErrorCode::SpawnFailed,
                    message,
                })?;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let started_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.inner.lock().insert(
            id,
            Hosted {
                label: label.chars().take(256).collect(),
                program: spec.program.clone(),
                cwd: spec.cwd.clone(),
                size,
                started_unix,
                ring: Ring::new(self.ring_bytes),
                exit: None,
                exited_at: None,
                process: Some(process),
                subscribers: Vec::new(),
            },
        );
        let table = self.clone();
        tokio::spawn(async move {
            while let Ok(event) = events.recv_async().await {
                let done = matches!(event, PtyEvent::Exited(_));
                table.on_event(id, event);
                if done {
                    break;
                }
            }
        });
        Ok(id)
    }

    fn on_event(&self, id: u64, event: PtyEvent) {
        let mut guard = self.inner.lock();
        let Some(hosted) = guard.get_mut(&id) else {
            return;
        };
        match event {
            PtyEvent::Output(bytes) => {
                let offset = hosted.ring.append(&bytes);
                let messages = chunks(id, offset, &bytes);
                deliver(hosted, &messages);
            }
            PtyEvent::Exited(exit) => {
                let exit = Exit {
                    code: exit.code,
                    signal: exit.signal,
                };
                hosted.exit = Some(exit.clone());
                hosted.exited_at = Some(Instant::now());
                hosted.process = None;
                deliver(hosted, &[Message::PtyExited { pty: id, exit }]);
            }
        }
    }

    /// Attaches `outbox` to a terminal: queues the replay (and, with
    /// `announce`, the `PtyAttached` answer first) and registers it for live
    /// output, atomically.
    pub fn attach(
        &self,
        pty: u64,
        from_offset: u64,
        outbox: &Outbox,
        announce: Option<u64>,
    ) -> Result<Attached, WireError> {
        let mut guard = self.inner.lock();
        let hosted = guard.get_mut(&pty).ok_or_else(not_found)?;
        let replay = hosted.ring.read_from(from_offset);
        let attached = Attached {
            replay_from: replay.from,
            gap: replay.gap,
            end_offset: hosted.ring.end_offset(),
            size: hosted.size,
            exit: hosted.exit.clone(),
        };
        let mut queue = Vec::new();
        if let Some(id) = announce {
            queue.push(Message::PtyAttached {
                id,
                pty,
                replay_from: attached.replay_from,
                gap: attached.gap,
                end_offset: attached.end_offset,
                size: attached.size,
                exit: attached.exit.clone(),
            });
        }
        queue.extend(chunks(pty, replay.from, &replay.bytes));
        if let Some(exit) = &hosted.exit {
            queue.push(Message::PtyExited {
                pty,
                exit: exit.clone(),
            });
        }
        for message in queue {
            if outbox.tx.try_send(message).is_err() {
                return Err(WireError {
                    code: ErrorCode::Busy,
                    message: "the connection cannot keep up".into(),
                });
            }
        }
        if hosted.exit.is_none() {
            hosted
                .subscribers
                .retain(|s| s.outbox.session != outbox.session);
            hosted.subscribers.push(Subscriber {
                outbox: outbox.clone(),
            });
        }
        Ok(attached)
    }

    /// Removes a session from every terminal; the terminals keep running.
    pub fn detach_session(&self, session: u64) {
        for hosted in self.inner.lock().values_mut() {
            hosted.subscribers.retain(|s| s.outbox.session != session);
        }
    }

    fn process(&self, pty: u64) -> Result<Option<Arc<dyn Process>>, WireError> {
        let guard = self.inner.lock();
        let hosted = guard.get(&pty).ok_or_else(not_found)?;
        Ok(hosted.process.clone())
    }

    /// Sends keystrokes to a terminal. Input to an ended terminal is dropped.
    pub fn write(&self, pty: u64, bytes: Vec<u8>) -> Result<(), WireError> {
        if let Some(process) = self.process(pty)? {
            process.write(bytes);
        }
        Ok(())
    }

    /// Resizes a terminal.
    pub fn resize(&self, pty: u64, size: Grid) -> Result<(), WireError> {
        let process = {
            let mut guard = self.inner.lock();
            let hosted = guard.get_mut(&pty).ok_or_else(not_found)?;
            hosted.size = size;
            hosted.process.clone()
        };
        if let Some(process) = process {
            process.resize(to_size(size));
        }
        Ok(())
    }

    /// Hangs a terminal's program up.
    pub fn close(&self, pty: u64) -> Result<(), WireError> {
        if let Some(process) = self.process(pty)? {
            process.kill();
        }
        Ok(())
    }

    /// The terminals, oldest first.
    pub fn list(&self) -> Vec<PtyInfo> {
        let guard = self.inner.lock();
        let mut infos: Vec<PtyInfo> = guard
            .iter()
            .map(|(id, h)| PtyInfo {
                pty: *id,
                label: h.label.clone(),
                program: h.program.clone(),
                cwd: h.cwd.clone(),
                size: h.size,
                started_unix: h.started_unix,
                exit: h.exit.clone(),
                first_offset: h.ring.first_offset(),
                end_offset: h.ring.end_offset(),
                attached: h.subscribers.len() as u32,
            })
            .collect();
        infos.sort_by_key(|info| info.pty);
        infos
    }

    /// Drops terminals that exited more than the retention window ago.
    pub fn reap(&self, now: Instant) -> usize {
        let mut guard = self.inner.lock();
        let before = guard.len();
        guard.retain(|_, h| {
            h.exited_at
                .is_none_or(|at| now.saturating_duration_since(at) < self.retention)
        });
        before - guard.len()
    }

    /// Hangs up every terminal (the host is shutting down).
    pub fn close_all(&self) {
        for hosted in self.inner.lock().values() {
            if let Some(process) = &hosted.process {
                process.kill();
            }
        }
    }
}

/// Queues `messages` for every subscriber; one that cannot keep up is dropped
/// and woken so its session ends and reconnects.
fn deliver(hosted: &mut Hosted, messages: &[Message]) {
    hosted.subscribers.retain(|sub| {
        for message in messages {
            if sub.outbox.tx.try_send(message.clone()).is_err() {
                sub.outbox.lagged.notify_one();
                return false;
            }
        }
        true
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn grid() -> Grid {
        Grid {
            cols: 80,
            rows: 24,
            cell_width: 0,
            cell_height: 0,
        }
    }

    fn sh(script: &str) -> SpawnSpec {
        SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            ..SpawnSpec::default()
        }
    }

    fn outbox(session: u64, capacity: usize) -> (Outbox, mpsc::Receiver<Message>) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            Outbox {
                session,
                tx,
                lagged: Arc::new(Notify::new()),
            },
            rx,
        )
    }

    async fn until(table: &PtyTable, pty: u64, done: impl Fn(&PtyInfo) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if table.list().iter().any(|i| i.pty == pty && done(i)) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn text(messages: &[Message]) -> String {
        let mut out = Vec::new();
        for m in messages {
            if let Message::PtyData { bytes, .. } = m {
                out.extend(bytes);
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn a_terminal_runs_with_no_client_and_a_late_attach_replays_what_it_missed() {
        let table = PtyTable::new(1 << 20, Duration::from_secs(60), 8);
        let pty = table
            .open(
                &sh("echo first; sleep 0.3; echo second; sleep 30"),
                grid(),
                "t",
            )
            .unwrap();
        until(&table, pty, |i| i.end_offset > 5).await;
        let (out, mut rx) = outbox(1, 64);
        let attached = table.attach(pty, 0, &out, Some(7)).unwrap();
        assert!(!attached.gap);
        let first = rx.recv().await.unwrap();
        assert!(matches!(first, Message::PtyAttached { id: 7, .. }));
        until(&table, pty, |i| i.end_offset >= 15).await;
        let mut got = Vec::new();
        while let Ok(m) = rx.try_recv() {
            got.push(m);
        }
        assert!(text(&got).contains("first"), "{:?}", text(&got));
        table.close(pty).unwrap();
    }

    #[tokio::test]
    async fn re_attaching_from_the_last_offset_gives_each_byte_exactly_once() {
        let table = PtyTable::new(1 << 20, Duration::from_secs(60), 8);
        let pty = table
            .open(
                &sh("echo one; sleep 0.4; echo two; sleep 0.4; echo three; sleep 30"),
                grid(),
                "t",
            )
            .unwrap();
        let (a, mut rx_a) = outbox(1, 64);
        table.attach(pty, 0, &a, None).unwrap();
        // Read until "one" has arrived, remember the offset, then drop the client.
        let mut seen = Vec::new();
        let mut cursor = 0u64;
        while !String::from_utf8_lossy(&seen).contains("one") {
            if let Some(Message::PtyData { offset, bytes, .. }) = rx_a.recv().await {
                assert_eq!(offset, cursor);
                cursor += bytes.len() as u64;
                seen.extend(bytes);
            }
        }
        table.detach_session(1);
        drop(rx_a);
        until(&table, pty, |i| i.end_offset > cursor + 8).await;
        let (b, mut rx_b) = outbox(2, 64);
        let attached = table.attach(pty, cursor, &b, None).unwrap();
        assert!(!attached.gap);
        assert_eq!(attached.replay_from, cursor);
        while !String::from_utf8_lossy(&seen).contains("three") {
            if let Some(Message::PtyData { offset, bytes, .. }) = rx_b.recv().await {
                assert_eq!(offset, cursor, "no gap and no duplicate");
                cursor += bytes.len() as u64;
                seen.extend(bytes);
            }
        }
        let all = String::from_utf8_lossy(&seen).replace("\r\n", "\n");
        assert_eq!(all, "one\ntwo\nthree\n");
        table.close(pty).unwrap();
    }

    #[tokio::test]
    async fn the_exit_status_is_delivered_and_the_terminal_is_reaped_later() {
        let table = PtyTable::new(1 << 20, Duration::from_millis(50), 8);
        let pty = table.open(&sh("exit 5"), grid(), "t").unwrap();
        until(&table, pty, |i| i.exit.is_some()).await;
        assert_eq!(table.list()[0].exit.as_ref().unwrap().code, 5);
        let (out, mut rx) = outbox(1, 16);
        let attached = table.attach(pty, 0, &out, None).unwrap();
        assert_eq!(attached.exit.unwrap().code, 5);
        let mut saw_exit = false;
        while let Ok(m) = rx.try_recv() {
            saw_exit |= matches!(m, Message::PtyExited { .. });
        }
        assert!(saw_exit);
        assert_eq!(table.reap(Instant::now()), 0, "within the window it stays");
        assert_eq!(table.reap(Instant::now() + Duration::from_secs(1)), 1);
        assert!(table.is_empty());
    }

    #[tokio::test]
    async fn a_session_that_cannot_keep_up_is_detached_and_woken() {
        let table = PtyTable::new(1 << 20, Duration::from_secs(60), 8);
        let pty = table
            .open(
                &sh("for i in 1 2 3 4 5 6 7 8; do echo line$i; sleep 0.05; done; sleep 30"),
                grid(),
                "t",
            )
            .unwrap();
        let (out, _rx) = outbox(1, 1);
        let lagged = out.lagged.clone();
        table.attach(pty, 0, &out, None).unwrap();
        tokio::time::timeout(Duration::from_secs(20), lagged.notified())
            .await
            .expect("woken");
        until(&table, pty, |i| i.attached == 0).await;
        table.close(pty).unwrap();
    }

    #[tokio::test]
    async fn too_many_terminals_are_refused() {
        let table = PtyTable::new(1 << 10, Duration::from_secs(60), 1);
        let first = table.open(&sh("sleep 30"), grid(), "a").unwrap();
        assert_eq!(
            table.open(&sh("sleep 30"), grid(), "b").unwrap_err().code,
            ErrorCode::Busy
        );
        table.close(first).unwrap();
    }

    #[tokio::test]
    async fn an_unknown_terminal_is_not_found() {
        let table = PtyTable::new(1 << 10, Duration::from_secs(60), 1);
        assert_eq!(table.close(99).unwrap_err().code, ErrorCode::NotFound);
        let (out, _rx) = outbox(1, 4);
        assert_eq!(
            table.attach(99, 0, &out, None).unwrap_err().code,
            ErrorCode::NotFound
        );
    }
}

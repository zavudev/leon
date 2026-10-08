//! The keeper of durable local sessions, over a real Unix socket in a
//! temporary directory, with scripted terminals.
//!
//! No shell runs and nothing touches the user's real runtime or data
//! directory: a terminal here is a [`Script`] the test drives (what it prints,
//! who is in front of it), and the socket lives in a directory of its own.
//! Everything between is the real code: the keeper, its peer check, the table
//! and its replay ring, the wire frames and the durable client.

#![cfg(unix)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use leon_host::keeper::{self, Keeper, KeeperConfig, KeeperError};
use leon_host::ptys::{Process, Spawner};
use leon_link::client::{Client, ClientConfig, ConnState, Failure, PtyEvent, PtyStream};
use leon_link::local::UnixDialer;
use leon_pty::{
    event_channel, EventSender, Events, Foreground, GridSize, PtyEvent as Raw, PtyExit,
};
use leon_wire::{ExecSpec, Grid};
use parking_lot::Mutex;

const WAIT: Duration = Duration::from_secs(30);

async fn until<F: FnMut() -> bool>(what: &str, mut condition: F) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn grid() -> Grid {
    Grid {
        cols: 80,
        rows: 24,
        cell_width: 0,
        cell_height: 0,
    }
}

// ----- a scripted terminal ----------------------------------------------------------------------

/// What the test sees and drives of one scripted terminal.
#[derive(Clone)]
struct Script {
    events: EventSender,
    written: Arc<Mutex<Vec<u8>>>,
    shell_in_front: Arc<AtomicBool>,
    command: Arc<Mutex<Option<String>>>,
    terminated: Arc<AtomicUsize>,
    spec: leon_pty::SpawnSpec,
    size: GridSize,
}

impl Script {
    /// Prints, as the program would.
    fn print(&self, text: &str) {
        let _ = self.events.send(Raw::Output(text.as_bytes().to_vec()));
    }
}

struct Scripted {
    script: Script,
    ended: AtomicBool,
}

impl Process for Scripted {
    fn write(&self, bytes: Vec<u8>) {
        self.script.written.lock().extend_from_slice(&bytes);
        // The terminal echoes what it is typed.
        let mut echo = b"echo:".to_vec();
        echo.extend(bytes);
        let _ = self.script.events.send(Raw::Output(echo));
    }
    fn resize(&self, _: GridSize) {}
    fn kill(&self) {
        if !self.ended.swap(true, Ordering::SeqCst) {
            let _ = self.script.events.send(Raw::Exited(PtyExit {
                code: 1,
                signal: Some("Hangup".into()),
            }));
        }
    }
    fn pid(&self) -> Option<u32> {
        Some(4242)
    }
    fn foreground(&self) -> Option<Foreground> {
        let front = self.script.shell_in_front.load(Ordering::SeqCst);
        Some(Foreground {
            pid: 4242,
            shell_in_front: front,
            command: if front {
                None
            } else {
                self.script.command.lock().clone()
            },
        })
    }
    fn terminate_foreground(&self) -> bool {
        self.script.terminated.fetch_add(1, Ordering::SeqCst);
        !self.script.shell_in_front.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct Studio {
    made: Mutex<Vec<Script>>,
}

impl Spawner for Studio {
    fn spawn(
        &self,
        spec: &leon_pty::SpawnSpec,
        size: GridSize,
    ) -> Result<(Arc<dyn Process>, Events), String> {
        let (events, receiver) = event_channel();
        let script = Script {
            events,
            written: Arc::default(),
            shell_in_front: Arc::new(AtomicBool::new(true)),
            command: Arc::default(),
            terminated: Arc::default(),
            spec: spec.clone(),
            size,
        };
        script.print("ready\r\n");
        self.made.lock().push(script.clone());
        Ok((
            Arc::new(Scripted {
                script,
                ended: AtomicBool::new(false),
            }),
            receiver,
        ))
    }
}

// ----- a keeper to talk to -----------------------------------------------------------------------

struct Rig {
    // Keeps the directory for as long as the test runs.
    dir: tempfile::TempDir,
    studio: Arc<Studio>,
    served: tokio::task::JoinHandle<()>,
    stop: tokio::sync::watch::Sender<bool>,
    socket: std::path::PathBuf,
}

fn config_in(dir: &Path, studio: &Arc<Studio>) -> KeeperConfig {
    let paths = keeper::paths(
        Some(dir.to_path_buf()),
        keeper::our_uid(),
        dir,
        &dir.join("data"),
    );
    let mut cfg = KeeperConfig::new(paths);
    cfg.spawner = studio.clone();
    cfg.ring_bytes = 64;
    cfg.idle_after = Duration::from_millis(400);
    cfg.first_id = 7_000_001;
    cfg
}

fn start_with(dir: tempfile::TempDir, edit: impl FnOnce(&mut KeeperConfig)) -> Rig {
    let studio = Arc::new(Studio::default());
    let mut cfg = config_in(dir.path(), &studio);
    edit(&mut cfg);
    let socket = cfg.paths.socket.clone();
    let keeper = Keeper::bind(cfg).expect("the keeper binds");
    let stop = keeper.stopper();
    let served = tokio::spawn(keeper.serve());
    Rig {
        dir,
        studio,
        served,
        stop,
        socket,
    }
}

fn start() -> Rig {
    start_with(tempfile::tempdir().unwrap(), |_| {})
}

fn client_of(rig: &Rig) -> Client {
    Client::start(ClientConfig::local(
        Arc::new(UnixDialer {
            path: rig.socket.clone(),
        }),
        "test",
    ))
}

fn spec(token: &str) -> (ExecSpec, String) {
    (
        ExecSpec {
            program: "/bin/zsh".into(),
            args: vec!["-l".into()],
            env: vec![
                ("ACCOUNT".into(), "work".into()),
                ("PATH".into(), "/x".into()),
            ],
            cwd: Some("/srv/api".into()),
            stdin: None,
        },
        format!("keep:{token}"),
    )
}

async fn text_until(stream: &mut PtyStream, wanted: &str) -> String {
    let mut seen = String::new();
    let deadline = Instant::now() + WAIT;
    while !seen.contains(wanted) {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, stream.events.recv()).await {
            Ok(Some(PtyEvent::Data(bytes))) => seen.push_str(&String::from_utf8_lossy(&bytes)),
            Ok(Some(_)) => {}
            other => panic!("waiting for {wanted:?}, saw {seen:?} then {other:?}"),
        }
    }
    seen
}

async fn foreground_of(stream: &mut PtyStream) -> leon_link::client::Foreground {
    stream.probe();
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, stream.events.recv()).await {
            Ok(Some(PtyEvent::Foreground(f))) => return f,
            Ok(Some(_)) => {}
            other => panic!("no answer to the probe: {other:?}"),
        }
    }
}

// ----- the tests ----------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_terminal_opened_through_the_socket_runs_with_the_environment_it_was_sent() {
    let rig = start();
    let client = client_of(&rig);
    let (exec, label) = spec("one");
    let mut stream = client.pty_open(exec.clone(), grid(), &label).await.unwrap();
    text_until(&mut stream, "ready").await;
    // The terminal was started with exactly the request's program, directory
    // and environment: the keeper adds nothing of its own to them.
    let made = rig.studio.made.lock()[0].clone();
    assert_eq!(made.spec.program, "/bin/zsh");
    assert_eq!(made.spec.args, ["-l"]);
    assert_eq!(made.spec.cwd.as_deref(), Some("/srv/api"));
    assert_eq!(made.spec.env, exec.env);
    assert_eq!((made.size.cols, made.size.rows), (80, 24));
    stream.write(b"hi".to_vec());
    text_until(&mut stream, "echo:hi").await;
    assert_eq!(made.written.lock().as_slice(), b"hi");
    // Its id comes from the keeper's own, random, base.
    assert!(stream.pty >= 7_000_001);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_keeper_says_who_is_in_front_of_a_terminal_and_can_ask_it_to_end() {
    let rig = start();
    let client = client_of(&rig);
    let (exec, label) = spec("fg");
    let mut stream = client.pty_open(exec, grid(), &label).await.unwrap();
    text_until(&mut stream, "ready").await;
    let shell = foreground_of(&mut stream).await;
    assert_eq!(shell.pid, Some(4242));
    assert_eq!(shell.shell_in_front, Some(true));
    assert_eq!(shell.command, None);

    let made = rig.studio.made.lock()[0].clone();
    made.shell_in_front.store(false, Ordering::SeqCst);
    *made.command.lock() = Some("claude".into());
    let agent = foreground_of(&mut stream).await;
    assert_eq!(agent.shell_in_front, Some(false));
    assert_eq!(agent.command.as_deref(), Some("claude"));

    stream.handle().terminate_foreground();
    until("the keeper to signal the program in front", || {
        made.terminated.load(Ordering::SeqCst) == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_terminal_outlives_its_client_and_a_new_one_lists_and_re_attaches_with_what_it_missed() {
    let rig = start();
    let first = client_of(&rig);
    let (exec, label) = spec("away");
    let mut stream = first.pty_open(exec, grid(), &label).await.unwrap();
    let pty = stream.pty;
    text_until(&mut stream, "ready").await;
    // "ready\r\n" is 7 bytes: that is where the window closed.
    let seen = 7;
    // The window goes away: the client and its connection with it.
    first.close();
    drop(stream);
    drop(first);
    let made = rig.studio.made.lock()[0].clone();
    // The program keeps going while nobody looks (less than the ring holds).
    made.print("while-");
    made.print("away\r\n");

    // The next start finds the terminal by what was listed.
    let second = client_of(&rig);
    let listed = second.pty_list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].pty, pty);
    assert_eq!(listed[0].label, label);
    assert_eq!(listed[0].cwd.as_deref(), Some("/srv/api"));
    assert!(listed[0].exit.is_none());
    assert_eq!(listed[0].size, grid());

    let mut again = second.pty_attach(pty, seen).await.unwrap();
    let attach = again.attach.clone().expect("what attaching found");
    assert!(!attach.gap, "the ring still reached back that far");
    assert_eq!(attach.replay_from, seen);
    assert_eq!(attach.end_offset, seen + "while-away\r\n".len() as u64);
    // Exactly what was missed, once.
    let replay = text_until(&mut again, "away").await;
    assert_eq!(replay, "while-away\r\n");
    // And it is live again.
    again.write(b"x".to_vec());
    text_until(&mut again, "echo:x").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_missed_more_than_the_ring_holds_is_told_there_is_a_gap() {
    let rig = start();
    let first = client_of(&rig);
    let (exec, label) = spec("gap");
    let mut stream = first.pty_open(exec, grid(), &label).await.unwrap();
    let pty = stream.pty;
    text_until(&mut stream, "ready").await;
    first.close();
    drop(stream);
    let made = rig.studio.made.lock()[0].clone();
    // 200 bytes into a ring of 64.
    for _ in 0..10 {
        made.print("0123456789abcdef\r\n");
    }
    made.print("the-end\r\n");

    let second = client_of(&rig);
    // Wait for the keeper to have taken all of it before looking.
    let mut listed = second.pty_list().await.unwrap();
    let want = 7 + 10 * 18 + 9;
    let deadline = Instant::now() + WAIT;
    while listed[0].end_offset < want as u64 {
        assert!(Instant::now() < deadline, "the keeper did not take it all");
        tokio::time::sleep(Duration::from_millis(10)).await;
        listed = second.pty_list().await.unwrap();
    }
    assert!(listed[0].first_offset > 7, "the oldest bytes were dropped");

    // Asking from the start of what it saw: the ring no longer reaches back.
    let mut again = second.pty_attach(pty, 7).await.unwrap();
    let attach = again.attach.clone().unwrap();
    assert!(attach.gap);
    assert_eq!(attach.replay_from, listed[0].first_offset);
    assert!(attach.replay_from > 7);
    // The stream says so before the bytes, so a screen can start over.
    let mut saw_gap = false;
    let mut text = String::new();
    while !text.contains("the-end") {
        match tokio::time::timeout(WAIT, again.events.recv())
            .await
            .unwrap()
        {
            Some(PtyEvent::Gap) => saw_gap = true,
            Some(PtyEvent::Data(bytes)) => text.push_str(&String::from_utf8_lossy(&bytes)),
            other => panic!("{other:?}"),
        }
    }
    assert!(saw_gap);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_terminal_that_ended_while_nobody_was_there_is_listed_with_how_it_ended() {
    let rig = start();
    let first = client_of(&rig);
    let (exec, label) = spec("ended");
    let mut stream = first.pty_open(exec, grid(), &label).await.unwrap();
    let pty = stream.pty;
    text_until(&mut stream, "ready").await;
    first.close();
    drop(stream);
    let made = rig.studio.made.lock()[0].clone();
    made.print("bye\r\n");
    let _ = made.events.send(Raw::Exited(PtyExit {
        code: 3,
        signal: None,
    }));

    let second = client_of(&rig);
    let deadline = Instant::now() + WAIT;
    loop {
        let listed = second.pty_list().await.unwrap();
        if let Some(exit) = &listed[0].exit {
            assert_eq!(exit.code, 3);
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut again = second.pty_attach(pty, 0).await.unwrap();
    assert_eq!(
        again.attach.as_ref().unwrap().exit.as_ref().unwrap().code,
        3
    );
    text_until(&mut again, "bye").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_terminal_ends_its_program_and_other_terminals_are_untouched() {
    let rig = start();
    let client = client_of(&rig);
    let (a, la) = spec("a");
    let (b, lb) = spec("b");
    let mut one = client.pty_open(a, grid(), &la).await.unwrap();
    let mut two = client.pty_open(b, grid(), &lb).await.unwrap();
    text_until(&mut one, "ready").await;
    text_until(&mut two, "ready").await;
    one.close();
    loop {
        match tokio::time::timeout(WAIT, one.events.recv()).await.unwrap() {
            Some(PtyEvent::Exited(exit)) => {
                assert_eq!(exit.signal.as_deref(), Some("Hangup"));
                break;
            }
            Some(_) => {}
            None => panic!("the stream ended without an exit"),
        }
    }
    let listed = client.pty_list().await.unwrap();
    let running: Vec<_> = listed.iter().filter(|i| i.exit.is_none()).collect();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].label, lb);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_from_another_user_is_refused_before_it_says_anything() {
    // The keeper believes its user is someone else, as it would of a peer that
    // is: every connection of ours is then a stranger's.
    let rig = start_with(tempfile::tempdir().unwrap(), |cfg| {
        cfg.uid = keeper::our_uid().wrapping_add(1);
    });
    let client = client_of(&rig);
    let outcome = client.wait_online(Duration::from_secs(5)).await;
    assert!(outcome.is_err(), "a stranger gets no session");
    match client.state() {
        ConnState::Offline {
            failure: Failure::Revoked | Failure::Other,
            ..
        }
        | ConnState::Connecting => {}
        other => panic!("{other:?}"),
    }
    assert!(client.pty_list().await.is_err());
    assert_eq!(rig.studio.made.lock().len(), 0);
    // The decision itself, for the ids that cannot be read at all.
    assert!(!keeper::peer_allowed(None, keeper::our_uid()));
    assert!(keeper::peer_allowed(
        Some(keeper::our_uid()),
        keeper::our_uid()
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_socket_is_closed_to_everyone_but_its_user() {
    use std::os::unix::fs::PermissionsExt;
    let rig = start();
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&rig.socket), 0o600);
    assert_eq!(mode(rig.socket.parent().unwrap()), 0o700);
    assert!(rig.dir.path().exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_keeper_for_the_same_directory_does_not_start_and_a_stale_socket_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let studio = Arc::new(Studio::default());
    let cfg = config_in(dir.path(), &studio);
    // A socket a keeper that died left behind.
    std::fs::create_dir_all(&cfg.paths.dir).unwrap();
    {
        let _dead = std::os::unix::net::UnixListener::bind(&cfg.paths.socket).unwrap();
    }
    assert!(cfg.paths.socket.exists(), "the file outlives its listener");
    let alive = Keeper::bind(cfg.clone()).expect("a stale socket is replaced");
    let socket = cfg.paths.socket.clone();
    // The lock names the keeper.
    assert_eq!(keeper::read_pid(&cfg.paths.lock), Some(std::process::id()));
    match Keeper::bind(cfg.clone()) {
        Err(KeeperError::AlreadyRunning) => {}
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("a second keeper started"),
    }
    // The first still works: the failed second did not take its socket away.
    let served = tokio::spawn(alive.serve());
    let client = Client::start(ClientConfig::local(
        Arc::new(UnixDialer { path: socket }),
        "t",
    ));
    client.wait_online(WAIT).await.unwrap();
    client.close();
    served.abort();
    drop(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_keeper_ends_by_itself_after_its_last_terminal_and_client_are_gone() {
    let rig = start();
    let client = client_of(&rig);
    let (exec, label) = spec("idle");
    let mut stream = client.pty_open(exec, grid(), &label).await.unwrap();
    text_until(&mut stream, "ready").await;
    // A terminal is running and a client is connected: it stays.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!rig.served.is_finished());
    // The client goes, the terminal keeps running: it still stays.
    client.close();
    drop(stream);
    drop(client);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!rig.served.is_finished(), "a running program keeps it");
    // The program ends (nobody to hear it): now it goes.
    let made = rig.studio.made.lock()[0].clone();
    let _ = made.events.send(Raw::Exited(PtyExit {
        code: 0,
        signal: None,
    }));
    until("the keeper to end", || rig.served.is_finished()).await;
    assert!(!rig.socket.exists(), "its socket is removed");
    let _ = rig.stop.send(true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ninth_window_is_told_the_keeper_is_full_and_the_others_are_not_dropped() {
    let rig = start();
    let mut clients = Vec::new();
    for _ in 0..8 {
        let client = client_of(&rig);
        client.wait_online(WAIT).await.unwrap();
        clients.push(client);
    }
    let ninth = client_of(&rig);
    assert!(
        ninth.wait_online(Duration::from_secs(2)).await.is_err(),
        "turned away"
    );
    // Answered, not silently dropped: the reason is what the client reports.
    let reason = match ninth.state() {
        ConnState::Offline { reason, .. } => reason,
        other => panic!("{other:?}"),
    };
    assert!(!reason.is_empty());
    // The eight that were served are still served.
    for client in &clients {
        assert!(client.pty_list().await.is_ok());
    }
    // A place frees up and the ninth gets it.
    clients.pop().unwrap().close();
    ninth.wait_online(WAIT).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keystrokes_and_a_hang_up_are_not_dropped_when_the_clients_queue_is_full() {
    let rig = start();
    let client = client_of(&rig);
    let (exec, label) = spec("busy");
    let mut stream = client.pty_open(exec, grid(), &label).await.unwrap();
    text_until(&mut stream, "ready").await;
    let handle = stream.handle();
    // Far more than the queue holds, sent without waiting for the connection.
    for _ in 0..2_000 {
        assert!(handle.write_wait(b"k".to_vec()).await);
    }
    assert!(handle.close_wait().await);
    let made = rig.studio.made.lock()[0].clone();
    until("every key to arrive", || made.written.lock().len() == 2_000).await;
    loop {
        match tokio::time::timeout(WAIT, stream.events.recv())
            .await
            .unwrap()
        {
            Some(PtyEvent::Exited(_)) => break,
            Some(_) => {}
            None => panic!("no exit"),
        }
    }
}

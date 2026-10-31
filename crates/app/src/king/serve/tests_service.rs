//! King's service as a whole, on a socket of its own with agents that are
//! scripts: what it does about the snapshot of a worker's worktree before
//! each turn.
//!
//! The service runs on a thread with a data folder of the test's. A worker
//! that was saved in its state is told something, which starts its turn, and
//! the test talks to the service as a client does, line by line.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use leon_king::config::Config;
use leon_king::coordinator::{Coordinator, Job, Target, WorkerSpec};
use leon_king::log::{Events, Record};
use leon_king::protocol::{Envelope, Reply, Request};
use leon_king::route::Choice;
use serde_json::Value;

use super::{serve, Snapshotter, Wiring};
use crate::chat::turn::testing::Scripted;
use crate::chat::turn::Output;
use crate::king::Paths;

/// A snapshotter that waits for the test to open its gate.
fn gated(gate: flume::Receiver<()>, answer: Result<String, String>) -> Snapshotter {
    Arc::new(move |_, _, _, _| {
        gate.recv().ok();
        answer.clone()
    })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let done = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(done.status.success(), "{args:?}: {done:?}");
    String::from_utf8_lossy(&done.stdout).trim().to_owned()
}

/// A repository with `a.txt` committed.
fn repository(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "first"]);
    repo
}

/// The service, running, and a client of it.
struct Rig {
    _root: tempfile::TempDir,
    repo: PathBuf,
    agents: Scripted,
    client: BufReader<UnixStream>,
    next: u64,
    service: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl Rig {
    /// A service with one worker in a repository, and `snapshotter` taking
    /// its checkpoints (the real one when absent).
    fn start(snapshotter: Option<Snapshotter>, timeout: Duration) -> Self {
        let root = tempfile::tempdir().unwrap();
        let top = std::fs::canonicalize(root.path()).unwrap();
        let repo = repository(&top);
        let data_dir = top.join("data");
        let folder = data_dir.join("king");
        std::fs::create_dir_all(&folder).unwrap();
        let paths = Paths {
            data_dir: data_dir.clone(),
            folder: folder.clone(),
            state: folder.join("state.json"),
            config: folder.join("king.toml"),
            socket: top.join("k.sock"),
            lock: top.join("k.lock"),
        };
        // A worker saved in the state, to be told something.
        let mut saved = Coordinator::new(Config::default());
        saved
            .start_worker(
                WorkerSpec {
                    title: "fix".into(),
                    prompt: "go".into(),
                    cwd: repo.to_string_lossy().into_owned(),
                    machine: None,
                    terminal: false,
                    project: None,
                    job: Job::Code,
                    choice: Choice {
                        agent: "claude".into(),
                        model: None,
                        effort: None,
                        tier: None,
                    },
                    mode: None,
                },
                1,
            )
            .unwrap();
        // Idle, with nothing to tell King: nothing is going on but what the
        // test starts.
        let mut state = saved.state().clone();
        state.news.clear();
        state.news_since_ms = None;
        for worker in state.workers.values_mut() {
            worker.conversation.busy = false;
        }
        std::fs::write(&paths.state, serde_json::to_string(&state).unwrap()).unwrap();
        let agents = Scripted::default();
        let mut wiring = Wiring::real();
        wiring.spawner = Some(Arc::new(agents.clone()));
        wiring.probe_models = false;
        wiring.snapshot_timeout = timeout;
        if let Some(snapshotter) = snapshotter {
            wiring.snapshotter = snapshotter;
        }
        let socket = paths.socket.clone();
        let service = std::thread::spawn(move || serve(paths, wiring));
        let started = Instant::now();
        let stream = loop {
            match UnixStream::connect(&socket) {
                Ok(stream) => break stream,
                Err(error) => {
                    assert!(started.elapsed() < Duration::from_secs(10), "{error}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        Self {
            _root: root,
            repo,
            agents,
            client: BufReader::new(stream),
            next: 1,
            service: Some(service),
        }
    }

    /// Asks the service and waits for its answer.
    fn ask(&mut self, request: Request) -> Reply {
        let id = self.next;
        self.next += 1;
        let line = serde_json::to_string(&Envelope { id, request }).unwrap();
        let writer = self.client.get_mut();
        writer.write_all(line.as_bytes()).unwrap();
        writer.write_all(b"\n").unwrap();
        loop {
            let mut reply = String::new();
            self.client.read_line(&mut reply).unwrap();
            if let Ok(reply) = serde_json::from_str::<Reply>(&reply) {
                if reply.id == id {
                    return reply;
                }
            }
        }
    }

    /// Tells the worker something: its turn begins.
    fn tell(&mut self, text: &str) {
        let reply = self.ask(Request::TellWorker {
            worker: 1,
            text: text.into(),
        });
        assert!(reply.ok, "{reply:?}");
    }

    /// The records of the worker's log, as the service keeps them.
    fn records(&mut self) -> Vec<Record> {
        let reply = self.ask(Request::Events {
            target: Target::Worker(1),
            since: None,
            limit: None,
            conversation: None,
        });
        assert!(reply.ok, "{reply:?}");
        serde_json::from_value::<Events>(reply.result)
            .unwrap()
            .events
            .into_iter()
            .map(|logged| logged.record)
            .collect()
    }

    fn checkpoints(&mut self) -> Vec<String> {
        self.records()
            .into_iter()
            .filter_map(|record| match record {
                Record::Checkpoint { commit } => Some(commit),
                _ => None,
            })
            .collect()
    }

    fn notices(&mut self) -> Vec<String> {
        self.records()
            .into_iter()
            .filter_map(|record| match record {
                Record::Notice { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    }

    /// How many agents were started.
    fn started(&self) -> usize {
        self.agents.0.lock().unwrap().started.len()
    }

    /// Waits until `condition` holds.
    fn until(&mut self, what: &str, condition: impl Fn(&mut Self) -> bool) {
        let started = Instant::now();
        while !condition(self) {
            assert!(started.elapsed() < Duration::from_secs(10), "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn busy(&mut self) -> bool {
        let reply = self.ask(Request::Status);
        reply.result["workers"][0]["conversation"]["busy"] == Value::Bool(true)
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(service) = self.service.take() {
            if !service.is_finished() {
                let _ = self.ask_quietly();
                let started = Instant::now();
                while !service.is_finished() && started.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}

impl Rig {
    /// Asks the service to stop, and does not wait for what it answers.
    fn ask_quietly(&mut self) -> std::io::Result<()> {
        let line = serde_json::to_string(&Envelope {
            id: 9999,
            request: Request::Shutdown { when_idle: false },
        })
        .unwrap();
        let writer = self.client.get_mut();
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")
    }
}

#[test]
fn a_workers_turn_is_snapshotted_before_the_agent_hears_it() {
    let mut rig = Rig::start(None, Duration::from_secs(30));
    // What the worktree holds when the turn begins: a change not committed,
    // and a file git does not know.
    std::fs::write(rig.repo.join("a.txt"), "two\n").unwrap();
    std::fs::write(rig.repo.join("b.txt"), "new\n").unwrap();
    rig.tell("fix the sum");
    rig.until("the agent was started", |rig| rig.started() == 1);
    // The agent changes the file as soon as it runs.
    std::fs::write(rig.repo.join("a.txt"), "three\n").unwrap();
    let records = rig.records();
    let checkpoint = records
        .iter()
        .position(|record| matches!(record, Record::Checkpoint { .. }))
        .expect("the log says a snapshot was taken");
    let prompt = records
        .iter()
        .position(|record| matches!(record, Record::Prompt { text, .. } if text == "fix the sum"))
        .expect("the prompt is in the log");
    assert!(
        prompt < checkpoint,
        "the checkpoint follows the prompt of its turn"
    );
    let commit = rig.checkpoints().remove(0);
    // The snapshot is of the files before the turn, not after it.
    assert_eq!(git(&rig.repo, &["show", &format!("{commit}:a.txt")]), "two");
    assert_eq!(git(&rig.repo, &["show", &format!("{commit}:b.txt")]), "new");
    assert_eq!(
        std::fs::read_to_string(rig.repo.join("a.txt")).unwrap(),
        "three\n"
    );
    // The person's own index, branch and history are as they were.
    assert_eq!(git(&rig.repo, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(git(&rig.repo, &["diff", "--cached", "--name-only"]), "");
}

#[test]
fn a_slow_snapshot_holds_back_its_turn_and_nothing_else() {
    let (open, gate) = flume::unbounded();
    let mut rig = Rig::start(
        Some(gated(gate, Ok("abc123".into()))),
        Duration::from_secs(30),
    );
    rig.tell("go");
    // The service answers while the snapshot is under way.
    let asked = Instant::now();
    assert!(rig.busy(), "the turn is begun, as far as everyone can see");
    assert!(
        asked.elapsed() < Duration::from_secs(2),
        "a status request waited for a snapshot"
    );
    let reply = rig.ask(Request::Models);
    assert!(reply.ok, "{reply:?}");
    assert_eq!(rig.started(), 0, "the agent has not heard the prompt yet");
    // The snapshot is there: the turn goes to the agent.
    open.send(()).unwrap();
    rig.until("the agent was started", |rig| rig.started() == 1);
    assert_eq!(rig.checkpoints(), ["abc123"]);
}

#[test]
fn a_snapshot_that_takes_too_long_is_left_and_the_turn_goes_without_it() {
    let (open, gate) = flume::unbounded();
    let mut rig = Rig::start(
        Some(gated(gate, Ok("late".into()))),
        Duration::from_millis(200),
    );
    rig.tell("go");
    rig.until("the turn went without it", |rig| rig.started() == 1);
    assert!(rig.checkpoints().is_empty());
    let notices = rig.notices();
    assert!(
        notices.iter().any(|text| text.contains("took longer than")),
        "{notices:?}"
    );
    // What it says when it is done is not for a turn that began long ago.
    open.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.started(), 1);
    assert!(rig.checkpoints().is_empty());
}

#[test]
fn a_snapshot_that_fails_is_no_checkpoint_and_says_so() {
    let mut rig = Rig::start(
        Some(Arc::new(|_, _, _, _| Err("the disk is full".to_owned()))),
        Duration::from_secs(30),
    );
    rig.tell("go");
    rig.until("the turn began", |rig| rig.started() == 1);
    assert!(rig.checkpoints().is_empty());
    let notices = rig.notices();
    assert!(
        notices.iter().any(|text| text.contains("the disk is full")),
        "{notices:?}"
    );
}

#[test]
fn a_turn_stopped_while_its_snapshot_is_taken_never_starts() {
    let (open, gate) = flume::unbounded();
    let mut rig = Rig::start(
        Some(gated(gate, Ok("abc123".into()))),
        Duration::from_secs(30),
    );
    rig.tell("go");
    assert!(rig.busy());
    let reply = rig.ask(Request::Interrupt {
        target: Target::Worker(1),
    });
    assert!(reply.ok, "{reply:?}");
    rig.until("the turn ended", |rig| !rig.busy());
    open.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rig.started(), 0, "the agent never heard it");
    assert!(rig.checkpoints().is_empty());
}

#[test]
fn a_service_asked_to_stop_when_idle_waits_for_a_turn_that_waits_for_its_snapshot() {
    let (open, gate) = flume::unbounded();
    let mut rig = Rig::start(
        Some(gated(gate, Ok("abc123".into()))),
        Duration::from_secs(30),
    );
    rig.tell("go");
    assert!(rig.busy());
    let reply = rig.ask(Request::Shutdown { when_idle: true });
    assert!(reply.ok, "{reply:?}");
    std::thread::sleep(Duration::from_millis(1400));
    assert!(
        !rig.service.as_ref().unwrap().is_finished(),
        "it stopped with a turn on its way"
    );
    // The snapshot comes, the agent runs and ends: now it is idle.
    open.send(()).unwrap();
    rig.until("the agent was started", |rig| rig.started() == 1);
    // The worker is stopped (nothing to tell King of it) and its process ends.
    let reply = rig.ask(Request::StopWorker { worker: 1 });
    assert!(reply.ok, "{reply:?}");
    rig.agents.0.lock().unwrap().outputs[0]
        .send(Output::Exited(0))
        .unwrap();
    let started = Instant::now();
    let service = rig.service.take().unwrap();
    while !service.is_finished() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it did not stop"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(service.join().unwrap(), Ok(()));
}

#[test]
fn a_service_asked_to_stop_at_once_stops_and_one_with_nothing_to_do_stops_when_idle() {
    let mut rig = Rig::start(None, Duration::from_secs(30));
    let reply = rig.ask(Request::Shutdown { when_idle: true });
    assert!(reply.ok, "{reply:?}");
    let started = Instant::now();
    let service = rig.service.take().unwrap();
    while !service.is_finished() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it did not stop"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(service.join().unwrap(), Ok(()));

    let mut rig = Rig::start(None, Duration::from_secs(30));
    let reply = rig.ask(Request::Shutdown { when_idle: false });
    assert!(reply.ok, "{reply:?}");
    let service = rig.service.take().unwrap();
    let started = Instant::now();
    while !service.is_finished() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it did not stop"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(service.join().unwrap(), Ok(()));
}

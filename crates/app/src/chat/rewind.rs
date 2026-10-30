//! Taking a conversation back to before one of its turns ("Edit from here"):
//! what goes on outside the window.
//!
//! Three things come back, each in its own way:
//!
//! * the agent's own memory of the conversation, in the way its protocol
//!   allows (`leon_turn::Rewind`): Claude Code's transcript is copied up to
//!   where the turn began, under a new id; Codex's server drops the turns;
//!   the agents of the Agent Client Protocol cannot, and the next turn starts
//!   a new conversation, which is said;
//! * the files of the worktree, from the checkpoint taken when the turn
//!   began ([`super::checkpoint`]), when the person asks for it;
//! * the checkpoints of the turns that are gone, which are forgotten.
//!
//! [`Mark`] is what is kept of every turn for that, and [`run`] does it all
//! off the interface thread.

use std::path::{Path, PathBuf};
use std::time::Duration;

use leon_history::live as locate;
use leon_turn::{object_on, CodexRevert, RevertStep, Rewind};

use super::checkpoint::{Repo, Restored};
use super::model::Item;
use super::turn::{Output, Spawner, Start};

/// How long Codex's server may take to answer before the attempt is given
/// up.
const PATIENCE: Duration = Duration::from_secs(60);

/// What the agent remembered of the conversation when a turn began.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Memory {
    /// Nothing: the turn started the conversation.
    Fresh,
    /// Claude Code's transcript of the session, as long as it was: the turn's
    /// prompt is what comes after that many bytes.
    Transcript {
        /// The session the turn resumed.
        session: String,
        /// The size of its file before the turn.
        len: u64,
    },
    /// There was a conversation, and what it was cannot be found again.
    Unknown,
}

impl Memory {
    /// What is remembered of the conversation of `session` in `cwd`, now,
    /// before a turn is sent.
    pub fn of(
        how: Rewind,
        session: Option<&str>,
        projects: Option<&Path>,
        cwd: &str,
        local: bool,
    ) -> Self {
        let Some(session) = session else {
            return Self::Fresh;
        };
        match (how, projects) {
            (Rewind::Fork, Some(projects)) if local => {
                match locate::find_claude_transcript(projects, cwd, session)
                    .and_then(|file| std::fs::metadata(file).ok())
                {
                    Some(meta) => Self::Transcript {
                        session: session.to_owned(),
                        len: meta.len(),
                    },
                    None => Self::Unknown,
                }
            }
            _ => Self::Unknown,
        }
    }
}

/// Where one turn of a conversation began, kept to go back there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    /// Where the person's words are among the conversation's items.
    pub item: usize,
    /// The turn's number in its chat: what names its checkpoint.
    pub turn: u32,
    /// The commit of the checkpoint, once it was taken. `None` when the
    /// folder is not in a git worktree, or the snapshot failed.
    pub checkpoint: Option<String>,
    /// What the agent remembered before it.
    pub memory: Memory,
}

/// How many of the turns from the one at `from` on got as far as the agent:
/// the ones that made it do something. A prompt that never was heard is not a
/// turn of the agent's memory.
pub fn turns_reached(items: &[Item], marks: &[Mark], from: usize) -> usize {
    let later: Vec<&Mark> = marks.iter().filter(|mark| mark.item >= from).collect();
    later
        .iter()
        .enumerate()
        .filter(|(at, mark)| {
            let end = later.get(at + 1).map_or(items.len(), |next| next.item);
            items
                .get(mark.item + 1..end)
                .unwrap_or_default()
                .iter()
                .any(|item| {
                    matches!(
                        item,
                        Item::Agent { .. } | Item::Thought { .. } | Item::Tool(_)
                    )
                })
        })
        .count()
}

/// Everything that is done to go back to before one turn.
pub struct Job {
    /// How the agent's conversation goes back.
    pub how: Rewind,
    /// The agent's name, for what is said.
    pub agent: String,
    /// What it remembered before the turn.
    pub memory: Memory,
    /// The session the conversation has now.
    pub session: Option<String>,
    /// How many turns Codex is to drop.
    pub turns: usize,
    /// Where Claude Code keeps its sessions.
    pub projects: Option<PathBuf>,
    /// The folder the agent works in.
    pub cwd: String,
    /// How to start the agent's server (Codex).
    pub start: Start,
    /// The checkpoint to put the files back to, when the person asked.
    pub restore: Option<String>,
    /// The chat the checkpoints are kept for, and the first turn whose
    /// checkpoint is forgotten.
    pub chat: String,
    /// The number of the turn.
    pub turn: u32,
}

/// What was done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// The session the conversation continues as: `None` is a new one.
    pub session: Option<String>,
    /// What to tell the person about the agent's memory, if it needs saying.
    pub notice: Option<String>,
    /// What came of the files, when they were asked for.
    pub files: Option<Result<Restored, String>>,
}

/// Does it. An error is a conversation that could not be taken back, and
/// then nothing was touched: the files are put back only after the agent's
/// memory is.
pub fn run(job: &Job, spawner: &dyn Spawner) -> Result<Done, String> {
    let (session, notice) = match job.how {
        Rewind::Fork => match &job.memory {
            Memory::Fresh => (None, None),
            Memory::Transcript { session, len } => {
                let projects = job
                    .projects
                    .as_deref()
                    .ok_or("Claude Code's sessions could not be found.")?;
                let new = uuid::Uuid::new_v4().to_string();
                fork_claude(projects, &job.cwd, session, *len, &new)?;
                (Some(new), None)
            }
            Memory::Unknown => (None, Some(reset(&job.agent))),
        },
        Rewind::Revert => match &job.session {
            Some(thread) => {
                revert_codex(spawner, job.start.clone(), thread, job.turns)?;
                (Some(thread.clone()), None)
            }
            None => (None, None),
        },
        Rewind::Reset => match job.memory {
            Memory::Fresh => (None, None),
            _ => (None, Some(reset(&job.agent))),
        },
    };
    let repo = Repo::of(Path::new(&job.cwd));
    let files = job.restore.as_ref().map(|commit| {
        let repo = repo
            .as_ref()
            .ok_or_else(|| "This folder is not in a git worktree.".to_owned())?;
        if !repo.has(commit) {
            return Err("The checkpoint of that turn is gone.".to_owned());
        }
        repo.restore(commit).map_err(|error| error.0)
    });
    if let Some(repo) = &repo {
        repo.forget_from(&job.chat, job.turn);
    }
    Ok(Done {
        session,
        notice,
        files,
    })
}

fn reset(agent: &str) -> String {
    format!(
        "Context was reset: {agent} cannot take its conversation back, so what follows starts a new session that does not remember what came before."
    )
}

/// Copies the transcript of the Claude Code session `session` of `cwd` up to
/// its first `len` bytes, as the session `new`, beside it.
pub fn fork_claude(
    projects: &Path,
    cwd: &str,
    session: &str,
    len: u64,
    new: &str,
) -> Result<(), String> {
    let source = locate::find_claude_transcript(projects, cwd, session)
        .ok_or("The transcript of this session could not be found.")?;
    let text = std::fs::read_to_string(&source)
        .map_err(|error| format!("The transcript could not be read: {error}."))?;
    let forked = leon_turn::fork_transcript(&text, len as usize, new)
        .ok_or("The transcript is not as it was when the message was sent.")?;
    let target = source.with_file_name(format!("{new}.jsonl"));
    std::fs::write(&target, forked)
        .map_err(|error| format!("The copy of the transcript could not be written: {error}."))
}

/// Has Codex's server drop the last `turns` turns of `thread`.
pub fn revert_codex(
    spawner: &dyn Spawner,
    start: Start,
    thread: &str,
    turns: usize,
) -> Result<(), String> {
    let mut revert = CodexRevert::new(thread, turns);
    let (mut transport, output) = spawner
        .spawn(start)
        .map_err(|error| format!("Codex could not be started: {error}."))?;
    let mut write = |lines: Vec<String>| {
        for line in lines {
            transport
                .write(&line)
                .map_err(|error| format!("Codex did not take the request: {error}."))?;
        }
        Ok::<(), String>(())
    };
    write(revert.first())?;
    let result = loop {
        match output.recv_timeout(PATIENCE) {
            Ok(Output::Line(line)) => {
                let Some(value) = object_on(&line) else {
                    continue;
                };
                match revert.line(&value) {
                    RevertStep::Wait => {}
                    RevertStep::Send(lines) => {
                        if let Err(error) = write(lines) {
                            break Err(error);
                        }
                    }
                    RevertStep::Done => break Ok(()),
                    RevertStep::Failed(why) => break Err(why),
                }
            }
            Ok(Output::Noise(_)) => {}
            Ok(Output::Exited(_)) => break Err("Codex stopped before it was done.".to_owned()),
            Err(_) => break Err("Codex did not answer in time.".to_owned()),
        }
    };
    transport.close_input();
    transport.kill();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::model::{Tone, Tool};
    use leon_core::{Machine, MachineId, MachineKind};
    use serde_json::{json, Value};
    use std::io;
    use std::sync::{Arc, Mutex};

    fn here() -> Machine {
        Machine {
            id: MachineId::local(),
            name: "This computer".into(),
            kind: MachineKind::Local,
        }
    }

    fn start() -> Start {
        Start {
            machine: here(),
            argv: vec!["codex".into(), "app-server".into()],
            cwd: "/work".into(),
            env: Vec::new(),
        }
    }

    /// A Codex whose thread has the turns `t1`..`t3`, which answers each
    /// request as the server does and remembers what it was asked.
    #[derive(Clone, Default)]
    struct Server {
        asked: Arc<Mutex<Vec<Value>>>,
        fail_revert: bool,
    }

    struct ServerTransport {
        server: Server,
        say: flume::Sender<Output>,
    }

    impl crate::chat::turn::Transport for ServerTransport {
        fn write(&mut self, line: &str) -> io::Result<()> {
            let request: Value = serde_json::from_str(line).unwrap();
            self.server.asked.lock().unwrap().push(request.clone());
            let Some(id) = request["id"].as_str() else {
                return Ok(());
            };
            let reply = match request["method"].as_str().unwrap() {
                "thread/turns/list" => {
                    let limit = request["params"]["limit"].as_u64().unwrap() as usize;
                    let data: Vec<Value> = ["t3", "t2", "t1"]
                        .iter()
                        .take(limit)
                        .map(|turn| json!({ "id": turn }))
                        .collect();
                    json!({ "id": id, "result": { "data": data, "nextCursor": null } })
                }
                "thread/revert" if self.server.fail_revert => {
                    json!({ "id": id, "error": { "message": "locked" } })
                }
                _ => json!({ "id": id, "result": {} }),
            };
            self.say.send(Output::Line(reply.to_string())).unwrap();
            Ok(())
        }
        fn close_input(&mut self) {}
        fn kill(&mut self) {}
    }

    impl Spawner for Server {
        fn spawn(
            &self,
            _: Start,
        ) -> io::Result<(
            Box<dyn crate::chat::turn::Transport>,
            flume::Receiver<Output>,
        )> {
            let (say, hear) = flume::unbounded();
            Ok((
                Box::new(ServerTransport {
                    server: self.clone(),
                    say,
                }),
                hear,
            ))
        }
    }

    fn job(how: Rewind, memory: Memory, session: Option<&str>) -> Job {
        Job {
            how,
            agent: "Agent".into(),
            memory,
            session: session.map(str::to_owned),
            turns: 2,
            projects: None,
            cwd: "/nowhere".into(),
            start: start(),
            restore: None,
            chat: "chat".into(),
            turn: 2,
        }
    }

    #[test]
    fn codex_is_asked_for_its_turns_and_then_to_drop_from_the_oldest_of_them() {
        let server = Server::default();
        revert_codex(&server, start(), "th1", 2).unwrap();
        let asked = server.asked.lock().unwrap();
        let methods: Vec<&str> = asked
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "initialized",
                "thread/resume",
                "thread/turns/list",
                "thread/revert"
            ]
        );
        assert_eq!(
            asked.last().unwrap()["params"],
            json!({ "threadId": "th1", "beforeTurnId": "t2" })
        );
    }

    #[test]
    fn a_codex_that_refuses_is_a_conversation_that_stays_as_it_is() {
        let server = Server {
            fail_revert: true,
            ..Server::default()
        };
        let error = revert_codex(&server, start(), "th1", 1).unwrap_err();
        assert!(error.contains("locked"), "{error}");
        let done = run(&job(Rewind::Revert, Memory::Unknown, Some("th1")), &server);
        assert!(done.is_err());
    }

    #[test]
    fn claudes_session_is_copied_up_to_where_the_turn_began() {
        let projects = tempfile::tempdir().unwrap();
        let cwd = "/work/app";
        let folder = projects.path().join(locate::claude_project_dir_name(cwd));
        std::fs::create_dir_all(&folder).unwrap();
        let one = "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"s1\"}\n";
        let two = "{\"type\":\"user\",\"uuid\":\"u2\",\"sessionId\":\"s1\"}\n";
        std::fs::write(folder.join("s1.jsonl"), format!("{one}{two}")).unwrap();
        let memory = Memory::of(Rewind::Fork, Some("s1"), Some(projects.path()), cwd, true);
        assert_eq!(
            memory,
            Memory::Transcript {
                session: "s1".into(),
                len: one.len() as u64 + two.len() as u64
            }
        );
        // The turn began after the first entry.
        let mut cut = job(
            Rewind::Fork,
            Memory::Transcript {
                session: "s1".into(),
                len: one.len() as u64,
            },
            Some("s1"),
        );
        cut.projects = Some(projects.path().to_owned());
        cut.cwd = cwd.into();
        let done = run(&cut, &Server::default()).unwrap();
        let new = done.session.expect("a new session");
        assert_ne!(new, "s1");
        assert_eq!(done.notice, None);
        let forked = std::fs::read_to_string(folder.join(format!("{new}.jsonl"))).unwrap();
        assert!(forked.contains("u1") && !forked.contains("u2"), "{forked}");
        assert!(forked.contains(&new) && !forked.contains("s1"), "{forked}");
        // The session it came from is as it was.
        assert_eq!(
            std::fs::read_to_string(folder.join("s1.jsonl")).unwrap(),
            format!("{one}{two}")
        );
    }

    #[test]
    fn what_cannot_be_taken_back_starts_over_and_says_so_unless_nothing_came_before() {
        let server = Server::default();
        for how in [Rewind::Fork, Rewind::Reset] {
            let lost = run(&job(how, Memory::Unknown, Some("s1")), &server).unwrap();
            assert_eq!(lost.session, None, "{how:?}");
            let notice = lost.notice.expect("it is said");
            assert!(notice.starts_with("Context was reset"), "{notice}");
            // The turn began the conversation: there is nothing to forget.
            let first = run(&job(how, Memory::Fresh, None), &server).unwrap();
            assert_eq!((first.session, first.notice), (None, None), "{how:?}");
        }
    }

    #[test]
    fn the_transcript_of_a_session_elsewhere_is_unknown() {
        let projects = tempfile::tempdir().unwrap();
        let of = |local, session| {
            Memory::of(Rewind::Fork, session, Some(projects.path()), "/work", local)
        };
        assert_eq!(of(true, None), Memory::Fresh);
        assert_eq!(of(true, Some("gone")), Memory::Unknown);
        assert_eq!(of(false, Some("s1")), Memory::Unknown);
        assert_eq!(
            Memory::of(Rewind::Fork, Some("s1"), None, "/work", true),
            Memory::Unknown
        );
    }

    fn user(text: &str) -> Item {
        Item::User {
            text: text.into(),
            at: None,
        }
    }

    fn agent(text: &str) -> Item {
        Item::Agent {
            text: text.into(),
            streaming: false,
            at: None,
        }
    }

    fn mark(item: usize, turn: u32) -> Mark {
        Mark {
            item,
            turn,
            checkpoint: None,
            memory: Memory::Fresh,
        }
    }

    #[test]
    fn a_turn_counts_for_the_agents_memory_only_when_the_agent_did_something() {
        let items = vec![
            user("one"),
            agent("a"),
            user("two"),
            Item::Notice {
                tone: Tone::Bad,
                text: "could not start".into(),
            },
            user("three"),
            Item::Tool(Tool {
                id: "t".into(),
                name: "Bash".into(),
                summary: String::new(),
                detail: String::new(),
                changes: Vec::new(),
                outcome: None,
            }),
        ];
        let marks = [mark(0, 1), mark(2, 2), mark(4, 3)];
        assert_eq!(turns_reached(&items, &marks, 0), 2);
        assert_eq!(turns_reached(&items, &marks, 2), 1);
        assert_eq!(turns_reached(&items, &marks, 4), 1);
        assert_eq!(turns_reached(&items, &marks, 5), 0);
    }
}

//! The messages of the service's control socket.
//!
//! The service listens on a Unix socket that only its own user may open. A
//! client (the window, the phone's host, `leon king …`, King's tools)
//! writes one JSON [`Envelope`] per line and reads one [`Reply`] per line,
//! in order. A client that sent [`Request::Subscribe`] then also reads
//! [`Notice`]s, each on a line of its own, whenever the state changes.
//!
//! Requests are serialised with their name in `op`:
//!
//! ```json
//! {"id":1,"op":"say","text":"What is the state of leon?"}
//! {"id":1,"ok":true,"result":null}
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::coordinator::{Job, Open, Reply as Answer, Target, WorkerState};
use crate::transcript::Entry;
use crate::verify::{Check, PullRequest};

/// The longest line read from a client, in bytes.
pub const MAX_LINE: usize = 1 << 20;

/// A request and the number its reply carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Chosen by the client; given back with the reply.
    pub id: u64,
    /// What is asked.
    #[serde(flatten)]
    pub request: Request,
}

/// What a client asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// King and its workers, in short ([`Status`]).
    Status,
    /// Say something to King.
    Say {
        /// The words.
        text: String,
    },
    /// What a conversation said after `since` ([`Entry`]s).
    Transcript {
        /// Whose.
        target: Target,
        /// The last entry the client has.
        #[serde(default)]
        since: Option<u64>,
        /// At most this many, the newest.
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Change the agent, model or effort King runs on.
    SetBrain {
        /// The agent's tag.
        agent: String,
        /// The model.
        #[serde(default)]
        model: Option<String>,
        /// How hard it thinks.
        #[serde(default)]
        effort: Option<String>,
        /// The permission mode.
        #[serde(default)]
        mode: Option<String>,
    },
    /// The projects Leon knows, with their worktrees.
    Projects,
    /// Where each agent stands: installed, its usage, what it costs.
    Usage,
    /// The models of this computer, with what is known of each.
    Models,
    /// Keep what was found out about a model, in `king-models.toml`.
    ModelNote {
        /// The card: it replaces the one with its pattern.
        card: crate::models::Card,
    },
    /// Start a worker.
    StartWorker(Box<WorkerRequest>),
    /// Tell a worker more.
    TellWorker {
        /// The worker.
        worker: u32,
        /// The words.
        text: String,
    },
    /// Answer a question King or a worker waits on.
    Answer {
        /// Whose question.
        target: Target,
        /// Its id.
        question: String,
        /// The answer.
        #[serde(flatten)]
        reply: Answer,
    },
    /// Stop a turn, keep the conversation.
    Interrupt {
        /// Whose.
        target: Target,
    },
    /// Stop a worker.
    StopWorker {
        /// The worker.
        worker: u32,
    },
    /// The person takes a worker over.
    Release {
        /// The worker.
        worker: u32,
    },
    /// Tell the person something: on their desktop and on the clients
    /// that listen.
    Notify {
        /// One line.
        title: String,
        /// The rest.
        #[serde(default)]
        body: String,
    },
    /// Send [`Notice`]s on this connection from now on.
    Subscribe,
    /// Put King's conversation away and start a new one.
    NewConversation,
    /// King's past conversations ([`crate::history::Past`]), newest first.
    Conversations,
    /// What a past conversation said ([`Entry`]s).
    PastConversation {
        /// Its number.
        conversation: u64,
    },
    /// Take a past conversation up again.
    ResumeConversation {
        /// Its number.
        conversation: u64,
    },
    /// End a process a turn left running ([`RunningProcess`]).
    StopProcess {
        /// Its id.
        pid: u32,
    },
    /// Run the project's check in a worker's worktree. It answers once the
    /// check started; King hears how it ended as news.
    Check {
        /// The worker.
        worker: u32,
    },
    /// Carry out a task of the board as its steps ([`crate::run`]).
    RunStart {
        /// The task, by id or the end of it.
        task: String,
        /// How many steps work at once, when not `[limits] run_parallel`.
        #[serde(default)]
        max_parallel: Option<usize>,
        /// The most its workers may cost, in dollars.
        #[serde(default)]
        budget_usd: Option<f64>,
    },
    /// Go on with a blocked run.
    RunContinue {
        /// The run.
        run: u32,
        /// What the workers of its failed steps are told.
        #[serde(default)]
        text: Option<String>,
        /// A new budget, in dollars.
        #[serde(default)]
        budget_usd: Option<f64>,
    },
    /// Call a run off.
    RunCancel {
        /// The run.
        run: u32,
    },
    /// A pull request was opened for a worker's branch, or its branch was
    /// pushed to again: watch its CI.
    PullRequestOpened {
        /// The worker.
        worker: u32,
        /// The pull request's address.
        url: String,
        /// The branch.
        branch: String,
    },
    /// Open a terminal in Leon's window that runs a command where the person
    /// sees it: a server, a watcher, anything whose output matters. The
    /// service keeps it until a window takes it ([`Request::TerminalTaken`]).
    OpenTerminal {
        /// The folder it runs in.
        cwd: String,
        /// The command, typed into the terminal's shell.
        command: String,
        /// What the terminal is called.
        #[serde(default)]
        title: Option<String>,
        /// The machine the folder is on, by name; this computer when absent.
        #[serde(default)]
        machine: Option<String>,
    },
    /// A window takes a terminal King asked for, to open it; only the first
    /// that asks gets it.
    TerminalTaken {
        /// The terminal's number ([`TerminalAsk::id`]). Not `id`, which is
        /// the envelope's.
        terminal: u64,
    },
    /// Run `git` or `gh` in a folder of another machine (King's tools run
    /// them here themselves): what it printed, or what it said on failing.
    Run {
        /// The machine, by Leon's id of it.
        machine: String,
        /// The folder.
        cwd: String,
        /// `git` or `gh`.
        program: String,
        /// Its arguments.
        #[serde(default)]
        args: Vec<String>,
    },
    /// Whether the person is looking at King on this connection's client:
    /// while anybody is, what King's conversation shows already is not also
    /// sent to the desktop.
    Watch {
        /// Looking, or not any more.
        on: bool,
    },
}

/// How a worker is asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRequest {
    /// What it is for, in a few words.
    pub title: String,
    /// What it is told first.
    pub prompt: String,
    /// The project, by name; the worker works in a new worktree of it
    /// unless `cwd` names a folder.
    #[serde(default)]
    pub project: Option<String>,
    /// The branch of the new worktree; one is made up from the title when
    /// absent.
    #[serde(default)]
    pub branch: Option<String>,
    /// A folder to work in instead: an existing worktree.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The machine `cwd` is on, by name; this computer when absent.
    #[serde(default)]
    pub machine: Option<String>,
    /// The tier.
    #[serde(default)]
    pub tier: Option<String>,
    /// The agent, instead of a tier.
    #[serde(default)]
    pub agent: Option<String>,
    /// The model, with an agent.
    #[serde(default)]
    pub model: Option<String>,
    /// How hard it thinks, with an agent.
    #[serde(default)]
    pub effort: Option<String>,
    /// The permission mode.
    #[serde(default)]
    pub mode: Option<String>,
    /// What kind of job.
    #[serde(default)]
    pub job: Job,
}

/// The answer to a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// The request's id.
    pub id: u64,
    /// Whether it was done.
    pub ok: bool,
    /// What it gave, when it was.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub result: Value,
    /// Why not, when it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    /// A request that was done.
    pub fn done(id: u64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result,
            error: None,
        }
    }

    /// A request that was refused.
    pub fn refused(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: Value::Null,
            error: Some(error.into()),
        }
    }
}

/// What a subscribed client is told without asking.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "notice", rename_all = "snake_case")]
pub enum Notice {
    /// The state changed; this is the new one.
    Changed {
        /// King and its workers.
        status: Box<Status>,
    },
    /// Something for the person.
    Notify {
        /// One line.
        title: String,
        /// The rest.
        body: String,
        /// What it says is in King's conversation already: a client that
        /// shows the conversation need not say it again.
        #[serde(default)]
        echo: bool,
    },
}

/// King and its workers, in short.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// The number of King's conversation now: another one after the person
    /// started a new one or took up a past one.
    #[serde(default)]
    pub conversation: u64,
    /// King's own conversation.
    pub brain: ConversationStatus,
    /// The workers that are still King's, newest last.
    pub workers: Vec<WorkerStatus>,
    /// What King has not been told yet.
    pub news: usize,
    /// What was spent today, in US dollars.
    pub spent_today_usd: f64,
    /// The day's budget, when there is one.
    pub daily_budget_usd: Option<f64>,
    /// What a turn of King's or of a worker left running when it ended: a
    /// server, a watcher. Filled by the service.
    #[serde(default)]
    pub running: Vec<RunningProcess>,
    /// The runs of tasks' steps, oldest first.
    #[serde(default)]
    pub runs: Vec<crate::run::Run>,
    /// The terminals King asked for that no window has opened yet, oldest
    /// first. Filled by the service.
    #[serde(default)]
    pub terminals: Vec<TerminalAsk>,
}

/// A terminal King asked a window to open ([`Request::OpenTerminal`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAsk {
    /// Its number, for [`Request::TerminalTaken`].
    pub id: u64,
    /// The folder it runs in.
    pub cwd: String,
    /// The machine the folder is on, by Leon's id of it; this computer when
    /// absent.
    #[serde(default)]
    pub machine: Option<String>,
    /// The command; the agent's first prompt when `agent` is set.
    pub command: String,
    /// The agent the terminal starts, by its tag, for a worker of an agent
    /// that only runs in a terminal; a shell running `command` when absent.
    #[serde(default)]
    pub agent: Option<String>,
    /// What it is called.
    pub title: String,
    /// When King asked for it, in milliseconds since the epoch.
    pub since_ms: u64,
}

/// A process a turn left running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningProcess {
    /// Its id on the service's computer.
    pub pid: u32,
    /// Its command line.
    pub command: String,
    /// Whose turn started it.
    pub of: Target,
    /// Since when it is known to run, in milliseconds since the epoch.
    pub since_ms: u64,
}

/// A conversation, in short.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationStatus {
    /// The agent's tag.
    pub agent: String,
    /// The model.
    pub model: Option<String>,
    /// How hard it thinks.
    pub effort: Option<String>,
    /// Its permission mode; the agent's own when absent.
    #[serde(default)]
    pub mode: Option<String>,
    /// The agent's id of the conversation.
    pub session_id: Option<String>,
    /// In a turn.
    pub busy: bool,
    /// When the turn in flight started, in milliseconds since the epoch.
    #[serde(default)]
    pub busy_since_ms: Option<u64>,
    /// What waits for the turn to end.
    pub queued: usize,
    /// The questions it waits on, with how each may be answered.
    pub questions: Vec<Open>,
    /// What it cost.
    pub cost_usd: f64,
    /// The number of its newest entry.
    pub last_seq: Option<u64>,
}

/// A worker, in short.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerStatus {
    /// Its number.
    pub id: u32,
    /// What it is for.
    pub title: String,
    /// Its project.
    pub project: Option<String>,
    /// Its folder, on its machine.
    pub cwd: String,
    /// Its machine, by Leon's id of it; this computer when absent.
    #[serde(default)]
    pub machine: Option<String>,
    /// It works in a terminal of the window ([`crate::coordinator::Worker::terminal`]).
    #[serde(default)]
    pub terminal: bool,
    /// Its kind of job.
    pub job: Job,
    /// The tier it came from.
    pub tier: Option<String>,
    /// Where it is.
    pub state: WorkerState,
    /// Its conversation.
    pub conversation: ConversationStatus,
    /// The last thing it answered, cut short.
    pub last_answer: Option<String>,
    /// When it last changed.
    pub updated_ms: u64,
    /// The conversation of King's that started it.
    #[serde(default)]
    pub started_in: Option<u64>,
    /// The last run of its project's check.
    #[serde(default)]
    pub check: Option<Check>,
    /// The pull request opened for its branch.
    #[serde(default)]
    pub pull_request: Option<PullRequest>,
}

/// The entries of a transcript, as [`Request::Transcript`] answers.
pub type Entries = Vec<Entry>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_its_name_and_its_fields_on_one_line() {
        let line = r#"{"id":7,"op":"say","text":"hola"}"#;
        let envelope: Envelope = serde_json::from_str(line).unwrap();
        assert_eq!(envelope.id, 7);
        assert_eq!(
            envelope.request,
            Request::Say {
                text: "hola".into()
            }
        );
        let answer: Envelope = serde_json::from_str(
            r#"{"id":1,"op":"answer","target":{"worker":3},"question":"q1","choice":"allow"}"#,
        )
        .unwrap();
        let Request::Answer {
            target,
            question,
            reply,
        } = answer.request
        else {
            panic!();
        };
        assert_eq!(target, Target::Worker(3));
        assert_eq!(question, "q1");
        assert_eq!(reply.choice.as_deref(), Some("allow"));
        let brain: Envelope =
            serde_json::from_str(r#"{"id":2,"op":"interrupt","target":"brain"}"#).unwrap();
        assert_eq!(
            brain.request,
            Request::Interrupt {
                target: Target::Brain
            }
        );
        let start: Envelope = serde_json::from_str(
            r#"{"id":3,"op":"start_worker","title":"t","prompt":"p","tier":"cheap","job":"research"}"#,
        )
        .unwrap();
        let Request::StartWorker(start) = start.request else {
            panic!();
        };
        assert_eq!(start.job, Job::Research);
        assert_eq!(start.tier.as_deref(), Some("cheap"));
    }

    #[test]
    fn no_request_has_a_field_named_like_the_envelopes_own() {
        // The request is flattened into the envelope: a field `id` of its
        // own would be read as the envelope's number.
        for request in [
            Request::PastConversation { conversation: 4 },
            Request::ResumeConversation { conversation: 4 },
            Request::RunStart {
                task: "1a2b3c4d".to_owned(),
                max_parallel: Some(2),
                budget_usd: Some(3.5),
            },
            Request::RunContinue {
                run: 4,
                text: None,
                budget_usd: None,
            },
            Request::RunCancel { run: 4 },
            Request::OpenTerminal {
                cwd: "/code/api".to_owned(),
                command: "bun run dev".to_owned(),
                title: None,
                machine: Some("build".to_owned()),
            },
            Request::TerminalTaken { terminal: 4 },
        ] {
            let line = serde_json::to_string(&Envelope {
                id: 91,
                request: request.clone(),
            })
            .unwrap();
            let read: Envelope = serde_json::from_str(&line).unwrap();
            assert_eq!((read.id, read.request), (91, request));
        }
    }

    #[test]
    fn a_reply_carries_a_result_or_an_error() {
        let done = serde_json::to_string(&Reply::done(1, serde_json::json!({"a": 1}))).unwrap();
        assert_eq!(done, r#"{"id":1,"ok":true,"result":{"a":1}}"#);
        let refused = serde_json::to_string(&Reply::refused(2, "no")).unwrap();
        assert_eq!(refused, r#"{"id":2,"ok":false,"error":"no"}"#);
    }
}

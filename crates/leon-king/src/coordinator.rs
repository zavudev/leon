//! The state of King and its workers, and what changes it.
//!
//! A [`Coordinator`] holds King's own conversation (the *brain*) and every
//! worker King started. It is changed by three things only: what the person
//! or King's tools ask ([`Coordinator::say`], [`Coordinator::start_worker`],
//! …), what an agent prints ([`Coordinator::event`]) and the clock
//! ([`Coordinator::tick`]). Each answers with the [`Effect`]s the service
//! carries out: start a turn, act on one, stop one, save and tell the
//! clients, tell the person. It starts nothing itself.
//!
//! **Turns.** A conversation is a sequence of turns, each one process of the
//! agent that continues the conversation by the id the agent gave it. What
//! is said to a conversation that is in the middle of a turn waits, and is
//! said, all of it at once, when the turn ends.
//!
//! **Waking.** King is never asked to poll. What a worker does that King
//! must know (it finished, it failed, it asks something) becomes *news*.
//! News is gathered for [`crate::config::Limits::wake_delay_secs`] after the
//! first of it, then King is given it as a message of its own
//! ([`crate::prompt::wake`]), or with the person's next message if they
//! speak first. King's tools see the same through the control socket.
//!
//! **Restarts.** The state is saved after every change ([`Effect::Changed`])
//! and read back by [`Coordinator::restored`]. A turn does not survive the
//! service: one that was running is said to have been interrupted, and a
//! worker that was working becomes news, so King can tell it to go on.

use std::collections::{BTreeMap, VecDeque};

use leon_turn::{Action, Answer, ChoiceAnswer, PermissionEffect, QuestionKind, TurnEvent};
use serde::{Deserialize, Serialize};

use crate::config::{Brain, Config};
use crate::history::{self, Archived, Past};
use crate::prompt;
use crate::protocol::{ConversationStatus, Status, WorkerStatus};
use crate::report::{Facts, Report, Totals, TurnStats};
use crate::route::Choice;
use crate::transcript::{self, Entry, Kind, Transcript};
use crate::verify::{self, Check, CheckState, Ci, PullRequest, Seen};

/// A day, in milliseconds.
const DAY_MS: u64 = 86_400_000;

/// The servers of tools Leon gives King. King's agent is never made to ask
/// the person before it uses one of them: they are what King is for, and
/// what they refuse they refuse themselves.
pub const OWN_SERVERS: [&str; 3] = ["leon-king", "leon-memory", "leon-tasks"];

/// Whose turn it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// King's own conversation.
    Brain,
    /// A worker, by number.
    Worker(u32),
}

/// What the service is to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Start a turn of `target` with `prompt`; the rest of the turn is
    /// [`Coordinator::plan`].
    Start {
        /// Whose.
        target: Target,
        /// What it is told.
        prompt: String,
    },
    /// Act on the turn `target` is in.
    Act {
        /// Whose.
        target: Target,
        /// What.
        action: Action,
    },
    /// End the turn `target` is in, now.
    Kill {
        /// Whose.
        target: Target,
    },
    /// The state changed: save it and tell the clients.
    Changed,
    /// Tell the person.
    Notify {
        /// One line.
        title: String,
        /// The rest.
        body: String,
        /// What it says is in King's conversation already.
        echo: bool,
    },
}

/// What a worker is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Job {
    /// It changes code in its worktree and commits on its branch.
    #[default]
    Code,
    /// It reads and reports; it changes nothing.
    Research,
    /// It plans, and changes nothing. By default it only runs on the
    /// `planning` tier (`[jobs] plan`, strict).
    Plan,
}

impl Job {
    /// Its name in `king.toml`'s `[jobs]`.
    pub fn as_str(self) -> &'static str {
        match self {
            Job::Code => "code",
            Job::Research => "research",
            Job::Plan => "plan",
        }
    }
}

/// Where a worker is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    /// In a turn.
    Working,
    /// In a turn, waiting for an answer to a question.
    Asking,
    /// Its last turn ended well; it waits to be told more.
    Done,
    /// Its last turn failed, or could not start.
    Failed,
    /// Its last turn was stopped before its end: by a refusal (Codex ends
    /// its turn when a permission is denied) or by an interrupt. It waits to
    /// be told how to go on.
    Paused,
    /// It was stopped.
    Stopped,
    /// The person took it over: it is no longer King's.
    Released,
}

/// A question an agent asked and nobody answered yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Open {
    /// The question's id, as the agent gave it.
    pub id: String,
    /// What is asked, as text.
    pub text: String,
    /// How it may be answered.
    pub form: Form,
}

/// How a question may be answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Form {
    /// One of these choices, by id, each with what it does
    /// (`allow`, `always`, `deny`).
    Permission {
        /// The choices.
        choices: Vec<(String, String)>,
    },
    /// One option per question, by label, or words of one's own.
    Choices {
        /// The questions' ids.
        questions: Vec<String>,
    },
    /// Approve the plan or not.
    Plan,
}

/// An answer to an [`Open`] question, as a tool or a client gives it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// For a permission, the choice's id or `allow`, `always`, `deny`; for
    /// options, the label picked.
    #[serde(default)]
    pub choice: Option<String>,
    /// For a plan, whether to start.
    #[serde(default)]
    pub approve: Option<bool>,
    /// Words: the reason for a refusal, an answer of one's own, what to
    /// change in a plan.
    #[serde(default)]
    pub text: Option<String>,
}

/// One conversation with an agent: King's or a worker's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    /// The agent's tag.
    pub agent: String,
    /// The model; the agent's own when absent.
    pub model: Option<String>,
    /// How hard it thinks; the agent's own when absent.
    pub effort: Option<String>,
    /// Its permission mode; the agent's own when absent.
    pub mode: Option<String>,
    /// The id the agent gave the conversation, once it did.
    pub session_id: Option<String>,
    /// A turn is running.
    pub busy: bool,
    /// What waits for the turn to end.
    pub queue: VecDeque<String>,
    /// The questions it waits on.
    pub questions: Vec<Open>,
    /// What it said.
    pub transcript: Transcript,
    /// What its turns cost, as the agent reported it.
    pub cost_usd: f64,
    /// How many turns it had.
    pub turns: u32,
    /// How its last turn ended: well, or not.
    #[serde(default)]
    pub last_ok: Option<bool>,
    /// Its last turn was stopped before its end, rather than failing.
    #[serde(default)]
    pub last_stopped: bool,
    /// What its turns read and wrote, all of them.
    #[serde(default)]
    pub tokens: Totals,
    /// The turn in flight: its tokens so far, and when it began.
    #[serde(default)]
    pub turn: TurnStats,
    /// When the turn in flight began.
    #[serde(default)]
    pub turn_started_ms: u64,
    /// The last turn that ended, measured.
    #[serde(default)]
    pub last_turn: Option<TurnStats>,
}

impl Conversation {
    fn new(choice: &Choice, mode: Option<String>) -> Self {
        Self {
            agent: choice.agent.clone(),
            model: choice.model.clone(),
            effort: choice.effort.clone(),
            mode,
            session_id: None,
            busy: false,
            queue: VecDeque::new(),
            questions: Vec::new(),
            transcript: Transcript::default(),
            cost_usd: 0.0,
            turns: 0,
            last_ok: None,
            last_stopped: false,
            tokens: Totals::default(),
            turn: TurnStats::default(),
            turn_started_ms: 0,
            last_turn: None,
        }
    }
}

/// A worker: a conversation King started, in a folder of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Worker {
    /// Its number, from 1.
    pub id: u32,
    /// What it is for, in a few words.
    pub title: String,
    /// The folder it works in, on this computer.
    pub cwd: String,
    /// The project's name, when it belongs to one Leon knows.
    pub project: Option<String>,
    /// What kind of job it has.
    pub job: Job,
    /// The tier it was chosen from, when it was.
    pub tier: Option<String>,
    /// Where it is.
    pub state: WorkerState,
    /// Its conversation.
    pub conversation: Conversation,
    /// When it started, in milliseconds since the epoch.
    pub started_ms: u64,
    /// When it last changed.
    pub updated_ms: u64,
    /// The conversation of King's that started it, by number; a
    /// conversation that went on in a new one ([`Coordinator::compact_if_long`])
    /// takes its workers along.
    #[serde(default)]
    pub started_in: Option<u64>,
    /// The last run of its project's check in its worktree.
    #[serde(default)]
    pub check: Option<Check>,
    /// The pull request opened for its branch.
    #[serde(default)]
    pub pull_request: Option<PullRequest>,
}

/// What King is to be told of a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct News {
    /// The worker.
    pub worker: u32,
    /// What happened, in a sentence or a paragraph.
    pub text: String,
}

/// What a worker is started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSpec {
    /// What it is for, in a few words.
    pub title: String,
    /// What it is told first.
    pub prompt: String,
    /// The folder it works in.
    pub cwd: String,
    /// The project's name, when known.
    pub project: Option<String>,
    /// What kind of job.
    pub job: Job,
    /// The agent and model, as [`crate::route::route`] chose them.
    pub choice: Choice,
    /// The permission mode, when King named one.
    pub mode: Option<String>,
}

/// Everything that is saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// King's own conversation.
    pub brain: Conversation,
    /// The workers, by number.
    pub workers: BTreeMap<u32, Worker>,
    /// The number the next worker gets.
    pub next_worker: u32,
    /// What King has not been told yet.
    pub news: Vec<News>,
    /// When the oldest of it came.
    pub news_since_ms: Option<u64>,
    /// The day the spending below is of, in days since the epoch.
    pub spent_day: u64,
    /// What the workers and King cost that day.
    pub spent_usd: f64,
    /// What `king.toml` said of King's agent when it was last read: a file
    /// that says something else now was edited, and wins over what was
    /// chosen since ([`Coordinator::set_brain`]).
    #[serde(default)]
    pub configured: Option<Brain>,
    /// What King read and wrote that day.
    #[serde(default)]
    pub king_today: Totals,
    /// What the workers read and wrote that day.
    #[serde(default)]
    pub workers_today: Totals,
    /// The number of King's conversation now.
    #[serde(default)]
    pub conversation: u64,
    /// King's past conversations, newest last ([`crate::history`]).
    #[serde(default)]
    pub past: Vec<Past>,
    /// Where a conversation put away for its length left off: told to King
    /// with the next thing it hears.
    #[serde(default)]
    pub carry: Option<String>,
}

/// The turn a target is to have, as the service needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The agent's tag.
    pub agent: String,
    /// The model.
    pub model: Option<String>,
    /// How hard it thinks.
    pub effort: Option<String>,
    /// The permission mode.
    pub mode: Option<String>,
    /// The conversation to continue.
    pub resume: Option<String>,
    /// The folder; `None` for King, whose folder is the service's.
    pub cwd: Option<String>,
    /// The instructions it is given beside its own, for a worker.
    pub instructions: Option<String>,
}

/// King and its workers.
#[derive(Debug, Clone)]
pub struct Coordinator {
    config: Config,
    state: State,
}

impl Coordinator {
    /// A coordinator that never ran.
    pub fn new(config: Config) -> Self {
        let brain = Conversation::new(
            &Choice {
                agent: config.brain.agent.clone(),
                model: config.brain.model.clone(),
                effort: config.brain.effort.clone(),
                tier: None,
            },
            config.brain.mode.clone(),
        );
        let configured = Some(config.brain.clone());
        Self {
            config,
            state: State {
                brain,
                workers: BTreeMap::new(),
                next_worker: 1,
                news: Vec::new(),
                news_since_ms: None,
                spent_day: 0,
                spent_usd: 0.0,
                configured,
                king_today: Totals::default(),
                workers_today: Totals::default(),
                conversation: 0,
                past: Vec::new(),
                carry: None,
            },
        }
    }

    /// A coordinator from what was saved. No turn survives the service: a
    /// conversation that was in one is told so, and King hears of every
    /// worker that was working.
    pub fn restored(config: Config, mut state: State, now_ms: u64) -> (Self, Vec<Effect>) {
        // King's agent is the one last chosen, unless the file was edited
        // since: then it is the file's. A new agent is a new conversation.
        let mut config = config;
        if state.configured.as_ref() == Some(&config.brain) {
            config.brain.agent = state.brain.agent.clone();
            config.brain.model = state.brain.model.clone();
            config.brain.effort = state.brain.effort.clone();
            config.brain.mode = state.brain.mode.clone();
        } else {
            if state.brain.agent != config.brain.agent {
                state.brain.session_id = None;
                state.brain.agent = config.brain.agent.clone();
            }
            state.brain.model = config.brain.model.clone();
            state.brain.effort = config.brain.effort.clone();
            state.brain.mode = config.brain.mode.clone();
            state.configured = Some(config.brain.clone());
        }
        // Workers saved before they knew their conversation: the one that
        // ran when they started.
        for worker in state.workers.values_mut() {
            if worker.started_in.is_none() {
                worker.started_in = Some(
                    state
                        .past
                        .iter()
                        .find(|past| (past.started_ms..=past.ended_ms).contains(&worker.started_ms))
                        .map_or(state.conversation, |past| past.id),
                );
            }
        }
        let mut coordinator = Self { config, state };
        let interrupted = "The turn was interrupted: Leon's service stopped.";
        if coordinator.state.brain.busy {
            coordinator.state.brain.busy = false;
            coordinator.state.brain.questions.clear();
            coordinator
                .state
                .brain
                .transcript
                .push(Kind::Notice, interrupted, now_ms);
        }
        let mut news = Vec::new();
        for worker in coordinator.state.workers.values_mut() {
            if worker.conversation.busy {
                worker.conversation.busy = false;
                worker.conversation.questions.clear();
                worker
                    .conversation
                    .transcript
                    .push(Kind::Notice, interrupted, now_ms);
                worker.state = WorkerState::Failed;
                worker.updated_ms = now_ms;
                news.push(News {
                    worker: worker.id,
                    text: "was interrupted when Leon's service stopped; tell it to go on \
                           with worker_send, or stop it"
                        .to_owned(),
                });
            }
        }
        for worker in coordinator.state.workers.values_mut() {
            if let Some(check) = worker
                .check
                .as_mut()
                .filter(|check| check.state == CheckState::Running)
            {
                check.state = CheckState::Failed;
                check.tail = "Leon's service stopped while it ran.".to_owned();
                news.push(News {
                    worker: worker.id,
                    text: format!(
                        "had its check (`{}`) cut short when Leon's service stopped; run \
                         worktree_check again",
                        check.command
                    ),
                });
            }
        }
        for item in news {
            coordinator.add_news(item, now_ms);
        }
        let mut effects = vec![Effect::Changed];
        effects.extend(coordinator.tick(now_ms));
        (coordinator, effects)
    }

    /// The configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Everything that is saved.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// One worker.
    pub fn worker(&self, id: u32) -> Option<&Worker> {
        self.state.workers.get(&id)
    }

    /// A conversation, King's or a worker's.
    pub fn conversation(&self, target: Target) -> Option<&Conversation> {
        match target {
            Target::Brain => Some(&self.state.brain),
            Target::Worker(id) => self.state.workers.get(&id).map(|w| &w.conversation),
        }
    }

    fn conversation_mut(&mut self, target: Target) -> Option<&mut Conversation> {
        match target {
            Target::Brain => Some(&mut self.state.brain),
            Target::Worker(id) => self
                .state
                .workers
                .get_mut(&id)
                .map(|worker| &mut worker.conversation),
        }
    }

    /// The workers that are in a turn.
    pub fn working(&self) -> usize {
        self.state
            .workers
            .values()
            .filter(|worker| worker.conversation.busy)
            .count()
    }

    /// What was spent today.
    pub fn spent_today(&self, now_ms: u64) -> f64 {
        if self.state.spent_day == now_ms / DAY_MS {
            self.state.spent_usd
        } else {
            0.0
        }
    }

    /// Puts King's conversation away and starts a new one. `None` when there
    /// was nothing to put away. Refused while King is in a turn.
    pub fn new_conversation(
        &mut self,
        now_ms: u64,
    ) -> Result<(Option<Archived>, Vec<Effect>), String> {
        if self.state.brain.busy {
            return Err("King is in a turn; stop it or wait for it to end first".to_owned());
        }
        let archived = self.archive(now_ms);
        self.state.conversation = self.next_conversation();
        Ok((archived, vec![Effect::Changed]))
    }

    /// Takes a past conversation up again: its agent's session goes on, its
    /// entries (`entries`, as the service kept them) are on screen again,
    /// and the conversation now is put away. Refused while King is in a turn.
    pub fn resume_conversation(
        &mut self,
        id: u64,
        entries: Vec<Entry>,
        now_ms: u64,
    ) -> Result<(Option<Archived>, Vec<Effect>), String> {
        if self.state.brain.busy {
            return Err("King is in a turn; stop it or wait for it to end first".to_owned());
        }
        let at = self
            .state
            .past
            .iter()
            .position(|past| past.id == id)
            .ok_or_else(|| format!("there is no past conversation {id}"))?;
        let past = self.state.past.remove(at);
        let archived = self.archive(now_ms);
        let brain = &mut self.state.brain;
        // The session is its agent's: King goes back to that agent.
        if brain.agent != past.agent {
            brain.agent = past.agent.clone();
            brain.model = past.model.clone();
            brain.effort = None;
            brain.mode = None;
            self.config.brain.agent = past.agent.clone();
            self.config.brain.model = past.model.clone();
            self.config.brain.effort = None;
            self.config.brain.mode = None;
        }
        brain.session_id = past.session_id;
        brain.transcript = Transcript::restored(entries);
        brain.questions.clear();
        self.state.conversation = past.id;
        Ok((archived, vec![Effect::Changed]))
    }

    /// After King's turn, with its report written: a turn that read
    /// [`crate::config::Limits::compact_at_tokens`] or more puts the
    /// conversation away, and the next one starts told where it left off.
    /// The archive is the service's to write.
    pub fn compact_if_long(&mut self, now_ms: u64) -> Option<Archived> {
        let most = self.config.limits.compact_at_tokens;
        let brain = &self.state.brain;
        let read = brain.last_turn.map_or(0, |turn| turn.input);
        if most == 0 || read < most || brain.busy || !brain.queue.is_empty() {
            return None;
        }
        let archived = self.archive(now_ms)?;
        self.state.carry = Some(history::carried(&archived.past, &archived.entries));
        let before = self.state.conversation;
        self.state.conversation = self.next_conversation();
        // King goes on with the same work: its workers come along.
        for worker in self.state.workers.values_mut() {
            if worker.started_in == Some(before) {
                worker.started_in = Some(self.state.conversation);
            }
        }
        self.state.brain.transcript.push(
            Kind::Notice,
            format!(
                "Leon put the conversation away: its last turn read {} tokens, and every turn \
                 reads all of it again. It is in HISTORY (\"{}\"); King goes on here, told \
                 where it left off.",
                crate::report::count(read),
                archived.past.title
            ),
            now_ms,
        );
        Some(archived)
    }

    /// The past conversations, newest last.
    pub fn past(&self) -> &[Past] {
        &self.state.past
    }

    /// Puts the conversation now away, when it said anything.
    fn archive(&mut self, now_ms: u64) -> Option<Archived> {
        let brain = &mut self.state.brain;
        let session_id = brain.session_id.take();
        let transcript = std::mem::take(&mut brain.transcript);
        brain.questions.clear();
        if transcript.is_empty() {
            return None;
        }
        let entries = transcript.entries();
        let past = Past {
            id: self.state.conversation,
            title: history::title_of(&entries),
            started_ms: entries.first().map_or(now_ms, |entry| entry.at_ms),
            ended_ms: now_ms,
            agent: brain.agent.clone(),
            model: brain.model.clone(),
            session_id,
            entries: entries.len(),
        };
        let dropped = history::keep(&mut self.state.past, past.clone());
        Some(Archived {
            past,
            entries,
            dropped,
        })
    }

    /// A number no conversation has.
    fn next_conversation(&self) -> u64 {
        self.state
            .past
            .iter()
            .map(|past| past.id)
            .chain(std::iter::once(self.state.conversation))
            .max()
            .map_or(0, |most| most + 1)
    }

    /// King and its workers, in short, as clients are told.
    pub fn status(&self, now_ms: u64) -> Status {
        let summary = |conversation: &Conversation| ConversationStatus {
            agent: conversation.agent.clone(),
            model: conversation.model.clone(),
            effort: conversation.effort.clone(),
            mode: conversation.mode.clone(),
            session_id: conversation.session_id.clone(),
            busy: conversation.busy,
            busy_since_ms: conversation.busy.then_some(conversation.turn_started_ms),
            queued: conversation.queue.len(),
            questions: conversation.questions.clone(),
            cost_usd: conversation.cost_usd,
            last_seq: conversation.transcript.last_seq(),
        };
        Status {
            conversation: self.state.conversation,
            brain: summary(&self.state.brain),
            workers: self
                .state
                .workers
                .values()
                .filter(|worker| worker.state != WorkerState::Released)
                .map(|worker| WorkerStatus {
                    id: worker.id,
                    title: worker.title.clone(),
                    project: worker.project.clone(),
                    cwd: worker.cwd.clone(),
                    job: worker.job,
                    tier: worker.tier.clone(),
                    state: worker.state,
                    conversation: summary(&worker.conversation),
                    last_answer: worker
                        .conversation
                        .transcript
                        .last_answer()
                        .map(|text| transcript::cut(text, 300)),
                    updated_ms: worker.updated_ms,
                    started_in: worker.started_in,
                    check: worker.check.clone(),
                    pull_request: worker.pull_request.clone(),
                })
                .collect(),
            news: self.state.news.len(),
            spent_today_usd: self.spent_today(now_ms),
            daily_budget_usd: self.config.limits.daily_budget_usd,
            running: Vec::new(),
        }
    }

    /// The turn `target` is to have next.
    pub fn plan(&self, target: Target) -> Option<Plan> {
        let conversation = self.conversation(target)?;
        let (cwd, instructions) = match target {
            Target::Brain => (None, None),
            Target::Worker(id) => {
                let worker = self.state.workers.get(&id)?;
                (Some(worker.cwd.clone()), Some(prompt::worker(worker)))
            }
        };
        Some(Plan {
            agent: conversation.agent.clone(),
            model: conversation.model.clone(),
            effort: conversation.effort.clone(),
            mode: conversation.mode.clone(),
            resume: conversation.session_id.clone(),
            cwd,
            instructions,
        })
    }

    /// The person says something to King.
    pub fn say(&mut self, text: &str, now_ms: u64) -> Vec<Effect> {
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        self.state.brain.queue.push_back(text.to_owned());
        let mut effects = vec![Effect::Changed];
        effects.extend(self.next_brain_turn(now_ms));
        effects
    }

    /// King changes the agent, the model or the effort it runs on. A new
    /// agent is a new conversation; a new model or effort is the same one.
    pub fn set_brain(&mut self, choice: &Choice, mode: Option<String>) -> Vec<Effect> {
        let brain = &mut self.state.brain;
        if brain.agent != choice.agent {
            brain.session_id = None;
            brain.agent = choice.agent.clone();
        }
        brain.model = choice.model.clone();
        brain.effort = choice.effort.clone();
        brain.mode = mode;
        self.config.brain.agent = choice.agent.clone();
        self.config.brain.model = choice.model.clone();
        self.config.brain.effort = choice.effort.clone();
        self.config.brain.mode = brain.mode.clone();
        vec![Effect::Changed]
    }

    /// Starts a worker. Refused when as many as allowed are working.
    pub fn start_worker(
        &mut self,
        spec: WorkerSpec,
        now_ms: u64,
    ) -> Result<(u32, Vec<Effect>), String> {
        let most = self.config.limits.max_workers;
        if self.working() >= most {
            return Err(format!(
                "{most} workers are working already, the most allowed; wait for one to finish \
                 or stop one"
            ));
        }
        if spec.prompt.trim().is_empty() {
            return Err("a worker needs a prompt".to_owned());
        }
        let id = self.state.next_worker;
        self.state.next_worker += 1;
        let mode = spec
            .mode
            .clone()
            .or_else(|| self.config.mode_of(&spec.choice.agent).map(str::to_owned));
        let mut conversation = Conversation::new(&spec.choice, mode);
        conversation.queue.push_back(spec.prompt);
        let worker = Worker {
            id,
            title: transcript::cut(spec.title.trim(), 80),
            cwd: spec.cwd,
            project: spec.project,
            job: spec.job,
            tier: spec.choice.tier.clone(),
            state: WorkerState::Working,
            conversation,
            started_ms: now_ms,
            updated_ms: now_ms,
            started_in: Some(self.state.conversation),
            check: None,
            pull_request: None,
        };
        self.state.workers.insert(id, worker);
        let mut effects = vec![Effect::Changed];
        effects.extend(self.next_turn(Target::Worker(id), now_ms));
        Ok((id, effects))
    }

    /// Something more for a worker: said now if it waits, after its turn if
    /// it works.
    pub fn tell_worker(&mut self, id: u32, text: &str, now_ms: u64) -> Result<Vec<Effect>, String> {
        let worker = self.owned(id)?;
        if text.trim().is_empty() {
            return Err("there is nothing to say".to_owned());
        }
        worker.conversation.queue.push_back(text.trim().to_owned());
        if worker.state == WorkerState::Stopped {
            worker.state = WorkerState::Done;
        }
        worker.updated_ms = now_ms;
        let mut effects = vec![Effect::Changed];
        effects.extend(self.next_turn(Target::Worker(id), now_ms));
        Ok(effects)
    }

    /// Answers a question `target` waits on.
    pub fn answer(
        &mut self,
        target: Target,
        question: &str,
        reply: &Reply,
    ) -> Result<Vec<Effect>, String> {
        if let Target::Worker(id) = target {
            self.owned(id)?;
        }
        let conversation = self
            .conversation(target)
            .ok_or_else(|| "there is no such conversation".to_owned())?;
        let open = conversation
            .questions
            .iter()
            .find(|open| open.id == question)
            .ok_or_else(|| format!("nothing waits on a question {question:?}"))?;
        let answer = answer_of(open, reply)?;
        Ok(vec![Effect::Act {
            target,
            action: Action::Answer(question.to_owned(), answer),
        }])
    }

    /// Stops the turn `target` is in and keeps the conversation.
    pub fn interrupt(&mut self, target: Target) -> Result<Vec<Effect>, String> {
        if let Target::Worker(id) = target {
            self.owned(id)?;
        }
        let busy = self.conversation(target).is_some_and(|c| c.busy);
        if !busy {
            return Err("it is not in a turn".to_owned());
        }
        Ok(vec![Effect::Act {
            target,
            action: Action::Interrupt,
        }])
    }

    /// Stops a worker: its turn ends now and what waited for it is dropped.
    pub fn stop_worker(&mut self, id: u32, now_ms: u64) -> Result<Vec<Effect>, String> {
        let worker = self.owned(id)?;
        worker.conversation.queue.clear();
        worker.state = WorkerState::Stopped;
        worker.updated_ms = now_ms;
        let mut effects = vec![Effect::Changed];
        if worker.conversation.busy {
            effects.push(Effect::Kill {
                target: Target::Worker(id),
            });
        }
        self.state.news.retain(|news| news.worker != id);
        Ok(effects)
    }

    /// The person takes a worker over: it is no longer King's, and its
    /// conversation can be continued anywhere by its session id. Only
    /// between turns.
    pub fn release(&mut self, id: u32, now_ms: u64) -> Result<(Worker, Vec<Effect>), String> {
        let worker = self.owned(id)?;
        if worker.conversation.busy {
            return Err("it is in a turn; wait for it to end or stop it first".to_owned());
        }
        worker.state = WorkerState::Released;
        worker.updated_ms = now_ms;
        worker.conversation.queue.clear();
        let released = worker.clone();
        self.state.news.retain(|news| news.worker != id);
        self.add_news(
            News {
                worker: id,
                text: "was taken over by the person; it is no longer yours".to_owned(),
            },
            now_ms,
        );
        Ok((released, vec![Effect::Changed]))
    }

    /// A worker that is King's to command.
    fn owned(&mut self, id: u32) -> Result<&mut Worker, String> {
        match self.state.workers.get_mut(&id) {
            None => Err(format!("there is no worker #{id}")),
            Some(worker) if worker.state == WorkerState::Released => {
                Err(format!("worker #{id} was taken over by the person"))
            }
            Some(worker) => Ok(worker),
        }
    }

    /// A turn could not be started.
    pub fn failed_to_start(&mut self, target: Target, error: &str, now_ms: u64) -> Vec<Effect> {
        let Some(conversation) = self.conversation_mut(target) else {
            return Vec::new();
        };
        conversation.busy = false;
        conversation.last_ok = Some(false);
        let text = format!("The agent could not be started: {error}");
        conversation
            .transcript
            .push(Kind::Notice, text.clone(), now_ms);
        let mut effects = vec![Effect::Changed];
        match target {
            Target::Brain => effects.push(Effect::Notify {
                title: "King could not start".to_owned(),
                body: text,
                echo: true,
            }),
            Target::Worker(id) => {
                if let Some(worker) = self.state.workers.get_mut(&id) {
                    worker.state = WorkerState::Failed;
                    worker.updated_ms = now_ms;
                }
                self.add_news(
                    News {
                        worker: id,
                        text: format!("could not start: {error}"),
                    },
                    now_ms,
                );
            }
        }
        effects
    }

    /// What an agent printed, as an event of the protocol.
    pub fn event(&mut self, target: Target, event: &TurnEvent, now_ms: u64) -> Vec<Effect> {
        let routine = match event {
            TurnEvent::Question { kind, .. } => self.routine_answer(target, kind),
            _ => None,
        };
        let Some(conversation) = self.conversation_mut(target) else {
            return Vec::new();
        };
        conversation.transcript.apply(event, now_ms);
        let mut effects = Vec::new();
        match event {
            TurnEvent::Started { session_id, model } => {
                conversation.session_id = Some(session_id.clone());
                if conversation.model.is_none() {
                    conversation.model = model.clone();
                }
                effects.push(Effect::Changed);
            }
            TurnEvent::Text { .. } | TurnEvent::ToolUse { .. } => effects.push(Effect::Changed),
            TurnEvent::Question { id, kind } if target == Target::Brain && own_tool(kind) => {
                if let Some(answer) = allow(kind) {
                    effects.push(Effect::Act {
                        target,
                        action: Action::Answer(id.clone(), answer),
                    });
                }
            }
            // A worker's routine question: Leon answers, King hears nothing.
            TurnEvent::Question { id, kind } if routine.is_some() => {
                if let Some((answer, why)) = routine {
                    let line = format!(
                        "Leon allowed it ({why}): {}",
                        transcript::question_text(kind)
                    );
                    conversation
                        .transcript
                        .push(Kind::Tool, transcript::cut(&line, 200), now_ms);
                    effects.push(Effect::Act {
                        target,
                        action: Action::Answer(id.clone(), answer),
                    });
                    effects.push(Effect::Changed);
                }
            }
            TurnEvent::Question { id, kind } => {
                let open = Open {
                    id: id.clone(),
                    text: transcript::question_text(kind),
                    form: form_of(kind),
                };
                conversation.questions.push(open.clone());
                effects.push(Effect::Changed);
                match target {
                    Target::Brain => effects.push(Effect::Notify {
                        title: "King asks".to_owned(),
                        body: open.text,
                        echo: true,
                    }),
                    Target::Worker(worker) => {
                        self.set_state(worker, WorkerState::Asking, now_ms);
                        self.add_news(
                            News {
                                worker,
                                text: format!(
                                    "asks, and waits (answer with worker_answer, question \
                                     {:?}): {}",
                                    open.id, open.text
                                ),
                            },
                            now_ms,
                        );
                    }
                }
            }
            TurnEvent::QuestionResolved { id } => {
                conversation.questions.retain(|open| open.id != *id);
                let waits = !conversation.questions.is_empty();
                effects.push(Effect::Changed);
                if let Target::Worker(worker) = target {
                    if !waits {
                        self.set_state(worker, WorkerState::Working, now_ms);
                        self.state
                            .news
                            .retain(|news| news.worker != worker || !news.text.starts_with("asks"));
                    }
                }
            }
            TurnEvent::Usage { tokens } => {
                let (duration, cost) = (conversation.turn.duration_ms, conversation.turn.cost_usd);
                conversation.turn = TurnStats::of(tokens, duration, cost);
            }
            TurnEvent::Finished {
                ok,
                cost_usd,
                duration_ms,
                interrupted,
                ..
            } => {
                conversation.last_ok = Some(*ok);
                conversation.last_stopped = *interrupted;
                conversation.turns += 1;
                conversation.turn.duration_ms = duration_ms
                    .unwrap_or_else(|| now_ms.saturating_sub(conversation.turn_started_ms));
                conversation.turn.cost_usd = *cost_usd;
                if let Some(cost) = cost_usd {
                    conversation.cost_usd += cost;
                    self.spend(*cost, now_ms);
                }
                effects.push(Effect::Changed);
            }
            TurnEvent::Exited { .. } | TurnEvent::Gone => {
                effects.extend(self.turn_ended(target, now_ms));
            }
            _ => {}
        }
        effects
    }

    /// Leon's own answer to a worker's question, when the question is
    /// routine for where the worker works ([`crate::policy`]).
    fn routine_answer(&self, target: Target, kind: &QuestionKind) -> Option<(Answer, String)> {
        let Target::Worker(id) = target else {
            return None;
        };
        let worker = self.state.workers.get(&id)?;
        crate::policy::routine(kind, &worker.cwd, &self.config.permissions)
    }

    fn set_state(&mut self, id: u32, state: WorkerState, now_ms: u64) {
        if let Some(worker) = self.state.workers.get_mut(&id) {
            if worker.state != WorkerState::Released && worker.state != WorkerState::Stopped {
                worker.state = state;
            }
            worker.updated_ms = now_ms;
        }
    }

    /// Starts a new day of spending when the day changed.
    fn roll_day(&mut self, now_ms: u64) {
        let today = now_ms / DAY_MS;
        if self.state.spent_day != today {
            self.state.spent_day = today;
            self.state.spent_usd = 0.0;
            self.state.king_today = Totals::default();
            self.state.workers_today = Totals::default();
        }
    }

    fn spend(&mut self, usd: f64, now_ms: u64) {
        self.roll_day(now_ms);
        self.state.spent_usd += usd;
    }

    /// Adds the turn that just ended to the counts.
    fn count_turn(&mut self, target: Target, now_ms: u64) {
        let Some(conversation) = self.conversation_mut(target) else {
            return;
        };
        if conversation.turn.duration_ms == 0 {
            conversation.turn.duration_ms = now_ms.saturating_sub(conversation.turn_started_ms);
        }
        let turn = std::mem::take(&mut conversation.turn);
        conversation.tokens.add(&turn);
        conversation.last_turn = Some(turn);
        self.roll_day(now_ms);
        match target {
            Target::Brain => self.state.king_today.add(&turn),
            Target::Worker(_) => self.state.workers_today.add(&turn),
        }
    }

    /// What the line under King's last answer is made of, but the usage
    /// windows, which the service reads.
    pub fn facts(&self, now_ms: u64) -> Facts<'static> {
        let today = self.state.spent_day == now_ms / DAY_MS;
        Facts {
            turn: self.state.brain.last_turn,
            workers_today: if today {
                self.state.workers_today
            } else {
                Totals::default()
            },
            king_today: if today {
                self.state.king_today
            } else {
                Totals::default()
            },
            spent_today_usd: self.spent_today(now_ms),
            budget_usd: self.config.limits.daily_budget_usd,
            usage: &[],
        }
    }

    /// Keeps a line of Leon's in a conversation's transcript: the report
    /// under an answer.
    pub fn annotate(&mut self, target: Target, report: Report, now_ms: u64) {
        if let Some(conversation) = self.conversation_mut(target) {
            conversation.transcript.push_report(report, now_ms);
        }
    }

    fn turn_ended(&mut self, target: Target, now_ms: u64) -> Vec<Effect> {
        self.count_turn(target, now_ms);
        let Some(conversation) = self.conversation_mut(target) else {
            return Vec::new();
        };
        conversation.busy = false;
        conversation.questions.clear();
        // A process that ended without saying how the turn went failed.
        let ok = conversation.last_ok.take().unwrap_or(false);
        conversation.last_ok = Some(ok);
        let answer = conversation
            .transcript
            .last_answer()
            .map(|text| transcript::cut(text, 1200));
        let waiting = !conversation.queue.is_empty();
        let mut effects = vec![Effect::Changed];
        match target {
            Target::Brain => {
                if !waiting {
                    effects.push(Effect::Notify {
                        title: "King answered".to_owned(),
                        body: answer.unwrap_or_default(),
                        echo: true,
                    });
                }
                effects.extend(self.next_brain_turn(now_ms));
            }
            Target::Worker(id) => {
                let Some(worker) = self.state.workers.get_mut(&id) else {
                    return effects;
                };
                worker.updated_ms = now_ms;
                if matches!(worker.state, WorkerState::Stopped | WorkerState::Released) {
                    return effects;
                }
                if waiting {
                    effects.extend(self.next_turn(target, now_ms));
                    return effects;
                }
                let stopped = !ok && worker.conversation.last_stopped;
                worker.state = match (ok, stopped) {
                    (true, _) => WorkerState::Done,
                    (false, true) => WorkerState::Paused,
                    (false, false) => WorkerState::Failed,
                };
                let text = match (ok, stopped, answer) {
                    (true, _, Some(answer)) => {
                        format!("finished its turn and answered:\n{answer}")
                    }
                    (true, _, None) => "finished its turn without a word".to_owned(),
                    (false, true, _) => "had its turn stopped before the end (a permission was \
                         denied, which ends a Codex turn, or the turn was interrupted). Its work so \
                         far is in its worktree; tell it how to go on with worker_send"
                        .to_owned(),
                    (false, false, Some(answer)) => format!("failed; its last words:\n{answer}"),
                    (false, false, None) => {
                        "failed without a word; read its transcript".to_owned()
                    }
                };
                self.state.news.retain(|news| news.worker != id);
                self.add_news(News { worker: id, text }, now_ms);
                effects.extend(self.tick(now_ms));
            }
        }
        effects
    }

    /// The project's check started in a worker's worktree.
    pub fn check_started(
        &mut self,
        id: u32,
        command: &str,
        commit: &str,
        dirty: bool,
        now_ms: u64,
    ) -> Result<Vec<Effect>, String> {
        let worker = self
            .state
            .workers
            .get_mut(&id)
            .ok_or_else(|| format!("there is no worker #{id}"))?;
        if worker
            .check
            .as_ref()
            .is_some_and(|check| check.state == CheckState::Running)
        {
            return Err(format!("worker #{id}'s check runs already"));
        }
        worker.check = Some(Check {
            command: command.to_owned(),
            commit: commit.to_owned(),
            dirty,
            state: CheckState::Running,
            started_ms: now_ms,
            duration_ms: None,
            tail: String::new(),
        });
        worker.updated_ms = now_ms;
        Ok(vec![Effect::Changed])
    }

    /// A worker's check ended: King hears how.
    pub fn check_finished(
        &mut self,
        id: u32,
        passed: bool,
        output: &str,
        now_ms: u64,
    ) -> Vec<Effect> {
        let Some(check) = self
            .state
            .workers
            .get_mut(&id)
            .and_then(|worker| worker.check.as_mut())
            .filter(|check| check.state == CheckState::Running)
        else {
            return Vec::new();
        };
        let took = now_ms.saturating_sub(check.started_ms);
        check.state = if passed {
            CheckState::Passed
        } else {
            CheckState::Failed
        };
        check.duration_ms = Some(took);
        check.tail = verify::tail(output, verify::TAIL_CHARS);
        let on = format!(
            "`{}` on {}{}",
            check.command,
            verify::short(&check.commit),
            if check.dirty {
                " (with changes not committed)"
            } else {
                ""
            }
        );
        let text = if passed {
            format!(
                "passed its check: {on}, in {}. Its branch can go out as a pull request \
                 (pr_open)",
                verify::duration(took)
            )
        } else {
            format!(
                "failed its check: {on}, after {}. The end of what it printed:\n{}\n\
                 Tell the worker what to mend (worker_send), then check again",
                verify::duration(took),
                transcript::cut(&check.tail, 1_500)
            )
        };
        self.add_news(News { worker: id, text }, now_ms);
        let mut effects = vec![Effect::Changed];
        effects.extend(self.tick(now_ms));
        effects
    }

    /// A pull request was opened for a worker's branch, or its branch was
    /// pushed to again: its CI is watched from now.
    pub fn pr_opened(
        &mut self,
        id: u32,
        url: &str,
        branch: &str,
        now_ms: u64,
    ) -> Result<Vec<Effect>, String> {
        let worker = self
            .state
            .workers
            .get_mut(&id)
            .ok_or_else(|| format!("there is no worker #{id}"))?;
        worker.pull_request = Some(PullRequest::new(url, branch, now_ms));
        worker.updated_ms = now_ms;
        Ok(vec![Effect::Changed])
    }

    /// The workers whose pull request's CI is still to be read, with its
    /// address and the folder `gh` runs in.
    pub fn watched_prs(&self) -> Vec<(u32, String, String)> {
        self.state
            .workers
            .values()
            .filter_map(|worker| {
                let pr = worker.pull_request.as_ref().filter(|pr| pr.watched())?;
                Some((worker.id, pr.url.clone(), worker.cwd.clone()))
            })
            .collect()
    }

    /// What a look at a worker's pull request found. When its CI settles,
    /// King hears it, and the person too when it passed: it is theirs to
    /// review and merge.
    pub fn ci_seen(&mut self, id: u32, seen: Seen, now_ms: u64) -> Vec<Effect> {
        let Some(worker) = self.state.workers.get_mut(&id) else {
            return Vec::new();
        };
        let title = worker.title.clone();
        let Some(pr) = worker.pull_request.as_mut().filter(|pr| pr.watched()) else {
            return Vec::new();
        };
        let mut ci = seen.ci;
        if ci == Ci::Pending
            && seen.failing.is_empty()
            && now_ms.saturating_sub(pr.pushed_ms) > verify::NO_CI_AFTER_MS
            && seen.closed.is_none()
        {
            // Nothing reported at all for long: the project runs no CI.
            ci = Ci::None;
        }
        let changed = pr.ci != ci || pr.closed != seen.closed || pr.failing != seen.failing;
        pr.ci = ci;
        pr.failing = seen.failing;
        pr.closed = seen.closed.clone();
        if !changed {
            return Vec::new();
        }
        let name = pr.name();
        let url = pr.url.clone();
        let mut effects = vec![Effect::Changed];
        let text = match (seen.closed.as_deref(), ci) {
            (Some(how), _) => Some(format!("has its pull request {name} {how} by the person")),
            (None, Ci::Passed) => {
                effects.push(Effect::Notify {
                    title: format!("{name} is ready for your review"),
                    body: format!("{title}: its CI passed. {url}"),
                    echo: false,
                });
                Some(format!(
                    "has its pull request {name} with every CI check passed: it is the \
                     person's to review and merge ({url})"
                ))
            }
            (None, Ci::Failed) => Some(format!(
                "has CI failing on its pull request {name}: {}. Read why (`gh pr checks {url}`, \
                 `gh run view --log-failed`) or have the worker do it, mend it on the same \
                 branch, check it, and pr_open again to push",
                pr.failing.join(", ")
            )),
            (None, Ci::None) => Some(format!(
                "has its pull request {name} open, and its project runs no CI on it"
            )),
            (None, Ci::Pending) => None,
        };
        worker.updated_ms = now_ms;
        if let Some(text) = text {
            self.add_news(News { worker: id, text }, now_ms);
            effects.extend(self.tick(now_ms));
        }
        effects
    }

    fn add_news(&mut self, news: News, now_ms: u64) {
        if self.state.news.is_empty() {
            self.state.news_since_ms = Some(now_ms);
        }
        self.state.news.push(news);
    }

    /// The clock: King is woken with news that waited long enough.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Effect> {
        let Some(since) = self.state.news_since_ms else {
            return Vec::new();
        };
        if self.state.news.is_empty() {
            self.state.news_since_ms = None;
            return Vec::new();
        }
        let delay = self.config.limits.wake_delay_secs.saturating_mul(1000);
        if self.state.brain.busy || now_ms < since.saturating_add(delay) {
            return Vec::new();
        }
        self.next_brain_turn(now_ms)
    }

    /// When the next tick matters: the moment news is due, if any waits.
    pub fn due_ms(&self) -> Option<u64> {
        let since = self.state.news_since_ms?;
        (!self.state.news.is_empty() && !self.state.brain.busy)
            .then(|| since.saturating_add(self.config.limits.wake_delay_secs.saturating_mul(1000)))
    }

    /// Starts King's next turn when it is free and has something to hear:
    /// what the person said, with the news that came meanwhile.
    fn next_brain_turn(&mut self, now_ms: u64) -> Vec<Effect> {
        if self.state.brain.busy {
            return Vec::new();
        }
        let said: Vec<String> = self.state.brain.queue.drain(..).collect();
        let delay = self.config.limits.wake_delay_secs.saturating_mul(1000);
        let news_due = self
            .state
            .news_since_ms
            .is_some_and(|since| now_ms >= since.saturating_add(delay));
        if said.is_empty() && !(news_due && !self.state.news.is_empty()) {
            return Vec::new();
        }
        let news: Vec<News> = std::mem::take(&mut self.state.news);
        self.state.news_since_ms = None;
        let mut parts = Vec::new();
        if let Some(carry) = self.state.carry.take() {
            parts.push((Kind::News, carry));
        }
        if !news.is_empty() {
            parts.push((Kind::News, prompt::wake(&news, &self.state.workers)));
        }
        parts.extend(said.into_iter().map(|text| (Kind::Prompt, text)));
        self.begin_with(Target::Brain, parts, now_ms)
    }

    /// Starts the next turn of a worker, with all that waits for it.
    fn next_turn(&mut self, target: Target, now_ms: u64) -> Vec<Effect> {
        let Some(conversation) = self.conversation_mut(target) else {
            return Vec::new();
        };
        if conversation.busy || conversation.queue.is_empty() {
            return Vec::new();
        }
        let said: Vec<String> = conversation.queue.drain(..).collect();
        if let Target::Worker(id) = target {
            self.set_state(id, WorkerState::Working, now_ms);
        }
        self.begin(target, said.join("\n\n"), now_ms)
    }

    fn begin(&mut self, target: Target, prompt: String, now_ms: u64) -> Vec<Effect> {
        self.begin_with(target, vec![(Kind::Prompt, prompt)], now_ms)
    }

    /// Starts a turn told all of `parts` as one prompt, each kept in the
    /// transcript as what it is: the person's words, or Leon's news.
    fn begin_with(
        &mut self,
        target: Target,
        parts: Vec<(Kind, String)>,
        now_ms: u64,
    ) -> Vec<Effect> {
        let Some(conversation) = self.conversation_mut(target) else {
            return Vec::new();
        };
        conversation.busy = true;
        conversation.last_ok = None;
        conversation.turn = TurnStats::default();
        conversation.turn_started_ms = now_ms;
        let prompt = parts
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        for (kind, text) in parts {
            conversation.transcript.push(kind, text, now_ms);
        }
        vec![Effect::Changed, Effect::Start { target, prompt }]
    }
}

/// Whether a question asks to use one of Leon's own tools ([`OWN_SERVERS`]),
/// by the names agents give them: `mcp__leon-king__worker_start` (Claude
/// Code), `leon-king.worker_start` or `leon-king/worker_start`.
fn own_tool(kind: &QuestionKind) -> bool {
    let QuestionKind::Permission { tool, .. } = kind else {
        return false;
    };
    let name = tool.strip_prefix("mcp__").unwrap_or(tool);
    OWN_SERVERS.iter().any(|server| {
        name.strip_prefix(server).is_some_and(|rest| {
            rest.starts_with("__") || rest.starts_with('.') || rest.starts_with('/')
        })
    })
}

/// The answer that allows a permission, for the session when the agent
/// offers that.
fn allow(kind: &QuestionKind) -> Option<Answer> {
    let QuestionKind::Permission { choices, .. } = kind else {
        return None;
    };
    let choice = choices
        .iter()
        .find(|choice| choice.effect == PermissionEffect::AllowAlways)
        .or_else(|| {
            choices
                .iter()
                .find(|choice| choice.effect == PermissionEffect::AllowOnce)
        })?;
    Some(Answer::Permission {
        choice_id: choice.id.clone(),
        reason: None,
    })
}

/// How a question may be answered, from what it is.
fn form_of(kind: &QuestionKind) -> Form {
    match kind {
        QuestionKind::Permission { choices, .. } => Form::Permission {
            choices: choices
                .iter()
                .map(|choice| {
                    let effect = match choice.effect {
                        PermissionEffect::AllowOnce => "allow",
                        PermissionEffect::AllowAlways => "always",
                        PermissionEffect::Deny => "deny",
                    };
                    (choice.id.clone(), effect.to_owned())
                })
                .collect(),
        },
        QuestionKind::Choices { questions } => Form::Choices {
            questions: questions.iter().map(|q| q.id.clone()).collect(),
        },
        QuestionKind::Plan { .. } => Form::Plan,
    }
}

/// The answer of the protocol a reply means, for this question.
fn answer_of(open: &Open, reply: &Reply) -> Result<Answer, String> {
    let text = reply.text.clone().filter(|text| !text.trim().is_empty());
    match &open.form {
        Form::Permission { choices } => {
            let wanted = reply
                .choice
                .as_deref()
                .ok_or_else(|| "a permission is answered with a choice".to_owned())?;
            let found = choices
                .iter()
                .find(|(id, _)| id == wanted)
                .or_else(|| choices.iter().find(|(_, effect)| effect == wanted))
                .ok_or_else(|| {
                    let ids: Vec<&str> = choices.iter().map(|(id, _)| id.as_str()).collect();
                    format!("the choices are {}, or allow, always, deny", ids.join(", "))
                })?;
            Ok(Answer::Permission {
                choice_id: found.0.clone(),
                reason: text,
            })
        }
        Form::Choices { questions } => {
            if reply.choice.is_none() && text.is_none() {
                return Err("options are answered with a choice or with text".to_owned());
            }
            Ok(Answer::Choices {
                answers: questions
                    .iter()
                    .map(|id| ChoiceAnswer {
                        question_id: id.clone(),
                        labels: reply.choice.iter().cloned().collect(),
                        other: text.clone(),
                    })
                    .collect(),
            })
        }
        Form::Plan => Ok(Answer::Plan {
            approve: reply
                .approve
                .ok_or_else(|| "a plan is answered with approve true or false".to_owned())?,
            feedback: text,
        }),
    }
}

#[cfg(test)]
mod tests;

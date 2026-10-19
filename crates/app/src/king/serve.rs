//! `leon king serve`: the service that keeps King and its workers.
//!
//! One thread owns the [`Coordinator`] and everything that changes it; the
//! rest only send it [`Input`]s: a thread per client connection (one JSON
//! request per line, see `leon_king::protocol`) and a thread per running
//! turn (what the agent's process prints). The owner carries out the
//! coordinator's effects (it starts turns through the same
//! [`Turn`]/[`Piped`] the window's conversations use), saves the state after
//! every change and tells the subscribed clients.
//!
//! **King's turn** runs in King's folder (`<data dir>/king`), whose
//! instruction files the service rewrites before every turn with the
//! projects Leon knows. The agent is given three servers of tools: King's
//! own (`leon king mcp`), the shared memory (`leon memory mcp`) and the
//! board (`leon task mcp`).
//!
//! **A worker's turn** runs in its folder, a worktree the service made for
//! it unless King named one, with what a worker is told
//! (`leon_king::prompt::worker`) beside its prompt.
//!
//! Guards that do not depend on what King was told live here: a worktree is
//! never made on a project's default branch, and a worker never works in a
//! project's main checkout.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use leon_core::{Machine, MachineId, MachineKind, Store};
use leon_king::coordinator::{Coordinator, Effect, State, Target, WorkerSpec};
use leon_king::models::{self, Available, AvailableMode, Catalogue};
use leon_king::prompt::{self, ProjectLine};
use leon_king::protocol::{Envelope, Notice, Reply, Request, WorkerRequest, MAX_LINE};
use leon_king::report::{self, AgentWindows, Window};
use leon_king::route::{self, Ask, Choice, Standing};
use leon_turn::{McpServer, TurnSpec};
use serde_json::{json, Value};

use super::Paths;
use crate::chat::turn::{Output, Piped, Spawner, Start, Turn};

/// How often the owner looks at the clock when nothing happens.
const IDLE_TICK: Duration = Duration::from_secs(1);

/// What the owner is told.
enum Input {
    /// A client's request, and where its reply goes.
    Request {
        envelope: Envelope,
        reply: flume::Sender<String>,
    },
    /// A turn's process said something.
    Output {
        target: Target,
        generation: u64,
        output: Output,
    },
    /// The agents said which models and permission modes they offer.
    Models(Vec<Available>, Vec<AvailableMode>),
    /// A while after a turn said its result: if its process still runs, the
    /// turn is over anyway ([`super::linger`]).
    Linger { target: Target, generation: u64 },
}

/// How long a process may go on after its turn said its result before the
/// turn is taken as over and the process as one that runs something.
const LINGER_AFTER: Duration = Duration::from_secs(4);

/// A turn whose process outlived it: what it started still runs.
struct Lingering {
    target: Target,
    generation: u64,
    turn: Turn,
    since_ms: u64,
}

/// How often the agents are asked again which models they offer.
const MODELS_EVERY: Duration = Duration::from_secs(30 * 60);

/// A turn in flight.
struct Running {
    generation: u64,
    turn: Turn,
    /// The agent said its result: the turn is over once the process ends,
    /// or [`LINGER_AFTER`] from now.
    finished: bool,
}

/// The owner: the coordinator and what carries out its effects.
struct Service {
    paths: Paths,
    exe: PathBuf,
    coordinator: Coordinator,
    turns: HashMap<Target, Running>,
    generation: u64,
    inputs: flume::Sender<Input>,
    subscribers: Vec<flume::Sender<String>>,
    /// The clients the person is looking at King on.
    watchers: Watchers,
    store: Option<Arc<Store>>,
    spawner: Arc<dyn Spawner>,
    /// Which agents are installed, and when that was found out.
    installed: Option<(u64, Vec<bool>)>,
    /// The models the agents offer, as they last said.
    models: Vec<Available>,
    /// The permission modes the agents offer, as they last said.
    modes: Vec<AvailableMode>,
    /// Turns whose process outlived them.
    lingering: Vec<Lingering>,
    /// What those left running, as last looked at.
    running: Vec<leon_king::protocol::RunningProcess>,
}

/// The connections whose client shows King to the person right now, by the
/// channel their replies go out on. A connection that closed stops counting.
#[derive(Default)]
struct Watchers {
    list: Vec<flume::Sender<String>>,
}

impl Watchers {
    /// The client on `connection` shows King, or stopped.
    fn set(&mut self, connection: &flume::Sender<String>, on: bool) {
        self.list
            .retain(|kept| !kept.same_channel(connection) && !kept.is_disconnected());
        if on {
            self.list.push(connection.clone());
        }
    }

    /// Whether any client shows King now.
    fn any(&mut self) -> bool {
        self.list.retain(|kept| !kept.is_disconnected());
        !self.list.is_empty()
    }
}

/// How long it is believed which agents are installed.
const INSTALLED_FOR_MS: u64 = 5 * 60 * 1000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Runs the service until it is killed.
pub fn run(paths: Paths) -> Result<(), String> {
    leon_host::keeper::ensure_private_dir(paths.socket.parent().ok_or("the socket has no folder")?)
        .map_err(|error| error.to_string())?;
    let _lock = lock(&paths.lock)?;
    std::fs::create_dir_all(&paths.folder).map_err(|error| error.to_string())?;
    let config = super::read_config(&paths)?;
    let saved: Option<State> = std::fs::read_to_string(&paths.state)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let (coordinator, effects) = match saved {
        Some(state) => Coordinator::restored(config, state, now_ms()),
        None => (Coordinator::new(config), vec![Effect::Changed]),
    };
    let store = Store::open(paths.data_dir.join("leon.db")).ok();
    let (sender, receiver) = flume::unbounded();
    let _ = std::fs::remove_file(&paths.socket);
    let listener = UnixListener::bind(&paths.socket).map_err(|error| error.to_string())?;
    {
        let sender = sender.clone();
        std::thread::Builder::new()
            .name("leon-king-listener".to_owned())
            .spawn(move || listen(listener, sender))
            .map_err(|error| error.to_string())?;
    }
    let mut service = Service {
        exe: std::env::current_exe().map_err(|error| error.to_string())?,
        paths,
        coordinator,
        turns: HashMap::new(),
        generation: 0,
        inputs: sender,
        subscribers: Vec::new(),
        watchers: Watchers::default(),
        store,
        spawner: Arc::new(Piped),
        installed: None,
        models: Vec::new(),
        modes: Vec::new(),
        lingering: Vec::new(),
        running: Vec::new(),
    };
    {
        let inputs = service.inputs.clone();
        let spawner = service.spawner.clone();
        std::thread::Builder::new()
            .name("leon-king-models".to_owned())
            .spawn(move || loop {
                let (models, modes) = probe_models(&*spawner);
                if inputs.send(Input::Models(models, modes)).is_err() {
                    return;
                }
                std::thread::sleep(MODELS_EVERY);
            })
            .map_err(|error| error.to_string())?;
    }
    tracing::info!(socket = %service.paths.socket.display(), "King's service is listening");
    service.carry_out(effects);
    loop {
        let wait = service
            .coordinator
            .due_ms()
            .map(|due| Duration::from_millis(due.saturating_sub(now_ms())))
            .unwrap_or(IDLE_TICK)
            .min(IDLE_TICK);
        let effects = match receiver.recv_timeout(wait) {
            Ok(input) => service.input(input),
            Err(flume::RecvTimeoutError::Timeout) => service.coordinator.tick(now_ms()),
            Err(flume::RecvTimeoutError::Disconnected) => return Ok(()),
        };
        service.carry_out(effects);
    }
}

/// Holds the lock that makes this process the service, for as long as the
/// file it returns is open.
fn lock(path: &Path) -> Result<std::fs::File, String> {
    use std::os::fd::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    // SAFETY: `flock` takes a descriptor this function owns, open for its
    // whole call.
    let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if !taken {
        return Err("King's service is running already".to_owned());
    }
    Ok(file)
}

/// Accepts connections, each served by a thread of its own.
fn listen(listener: UnixListener, inputs: flume::Sender<Input>) {
    let ours = leon_link::local::our_uid();
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            continue;
        };
        if !leon_link::local::peer_allowed(leon_link::local::peer_uid(&stream), ours) {
            continue;
        }
        let inputs = inputs.clone();
        let _ = std::thread::Builder::new()
            .name("leon-king-client".to_owned())
            .spawn(move || serve_client(stream, inputs));
    }
}

/// One client: requests in, replies and notices out, until it goes.
fn serve_client(stream: UnixStream, inputs: flume::Sender<Input>) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let (outgoing, lines) = flume::unbounded::<String>();
    std::thread::spawn(move || {
        for line in lines.iter() {
            if writer.write_all(line.as_bytes()).is_err() || writer.write_all(b"\n").is_err() {
                break;
            }
        }
    });
    let mut reader = BufReader::new(stream);
    loop {
        let mut bytes = Vec::new();
        match reader
            .by_ref()
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut bytes)
        {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if bytes.len() > MAX_LINE {
            let _ =
                outgoing.send(json!({"id": 0, "ok": false, "error": "line too long"}).to_string());
            return;
        }
        let text = String::from_utf8_lossy(&bytes);
        if text.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Envelope>(&text) {
            Ok(envelope) => {
                let input = Input::Request {
                    envelope,
                    reply: outgoing.clone(),
                };
                if inputs.send(input).is_err() {
                    return;
                }
            }
            Err(error) => {
                let refused = Reply::refused(0, format!("not a request: {error}"));
                let _ = outgoing.send(serde_json::to_string(&refused).unwrap_or_default());
            }
        }
    }
}

impl Service {
    fn input(&mut self, input: Input) -> Vec<Effect> {
        match input {
            Input::Request { envelope, reply } => {
                let id = envelope.id;
                let subscribe = matches!(envelope.request, Request::Subscribe);
                if let Request::Watch { on } = envelope.request {
                    self.watchers.set(&reply, on);
                }
                let (answer, effects) = match self.request(envelope.request) {
                    Ok((result, effects)) => (Reply::done(id, result), effects),
                    Err(error) => (Reply::refused(id, error), Vec::new()),
                };
                let _ = reply.send(serde_json::to_string(&answer).unwrap_or_default());
                if subscribe {
                    self.subscribers.push(reply);
                }
                effects
            }
            Input::Output {
                target,
                generation,
                output,
            } => {
                let ended = matches!(output, Output::Exited(_));
                // The process of a turn that is over already: when it ends,
                // what it left running ends with it.
                if let Some(at) = self
                    .lingering
                    .iter()
                    .position(|old| old.target == target && old.generation == generation)
                {
                    if ended {
                        self.lingering.remove(at);
                        self.refresh_running();
                        return vec![Effect::Changed];
                    }
                    return Vec::new();
                }
                let Some(running) = self.turns.get_mut(&target) else {
                    return Vec::new();
                };
                if running.generation != generation {
                    return Vec::new();
                }
                let events = running.turn.output(output);
                if ended {
                    self.turns.remove(&target);
                } else if !running.finished
                    && events
                        .iter()
                        .any(|event| matches!(event, leon_turn::TurnEvent::Finished { .. }))
                {
                    running.finished = true;
                    let inputs = self.inputs.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(LINGER_AFTER);
                        let _ = inputs.send(Input::Linger { target, generation });
                    });
                }
                let now = now_ms();
                let effects: Vec<Effect> = events
                    .iter()
                    .flat_map(|event| self.coordinator.event(target, event, now))
                    .collect();
                // Under every answer of King's: what it took, and what is left.
                let mut effects = effects;
                if ended && target == Target::Brain {
                    let report = self.report(now);
                    self.coordinator.annotate(Target::Brain, report, now);
                    effects.extend(self.compact(now));
                }
                effects
            }
            Input::Linger { target, generation } => {
                let lingers = self
                    .turns
                    .get(&target)
                    .is_some_and(|running| running.generation == generation && running.finished);
                if !lingers {
                    return Vec::new();
                }
                let Some(running) = self.turns.remove(&target) else {
                    return Vec::new();
                };
                let now = now_ms();
                self.lingering.push(Lingering {
                    target,
                    generation,
                    turn: running.turn,
                    since_ms: now,
                });
                self.refresh_running();
                // The turn is over: what waits for it goes on.
                let mut effects =
                    self.coordinator
                        .event(target, &leon_turn::TurnEvent::Exited { code: 0 }, now);
                if target == Target::Brain {
                    let report = self.report(now);
                    self.coordinator.annotate(Target::Brain, report, now);
                    effects.extend(self.compact(now));
                }
                effects.push(Effect::Changed);
                effects
            }
            Input::Models(models, modes) => {
                if !models.is_empty() {
                    self.models = models;
                }
                if !modes.is_empty() {
                    self.modes = modes;
                }
                Vec::new()
            }
        }
    }

    /// The line under King's last answer.
    fn report(&self, now: u64) -> report::Report {
        let facts = self.coordinator.facts(now);
        report::Report {
            turn: facts.turn,
            workers_today: facts.workers_today,
            king_today: facts.king_today,
            spent_today_usd: facts.spent_today_usd,
            budget_usd: facts.budget_usd,
            usage: usage_windows(self.store.as_deref(), now),
        }
    }

    /// The status, with what turns left running.
    fn status(&self, now: u64) -> leon_king::protocol::Status {
        let mut status = self.coordinator.status(now);
        status.running = self.running.clone();
        status
    }

    /// Looks again at what the turns that outlived themselves left running.
    /// One that runs nothing any more (only its tools) is ended.
    fn refresh_running(&mut self) {
        if self.lingering.is_empty() {
            self.running.clear();
            return;
        }
        let table = super::linger::process_table().unwrap_or_default();
        let mut running = Vec::new();
        let mut idle = Vec::new();
        for (at, old) in self.lingering.iter().enumerate() {
            let Some(pid) = old.turn.pid() else {
                continue;
            };
            let left = super::linger::left_running(&table, pid);
            if left.is_empty() && !table.is_empty() {
                idle.push(at);
            }
            running.extend(left.into_iter().map(|(pid, command)| {
                leon_king::protocol::RunningProcess {
                    pid,
                    command,
                    of: old.target,
                    since_ms: old.since_ms,
                }
            }));
        }
        for at in idle.into_iter().rev() {
            let mut old = self.lingering.remove(at);
            old.turn.kill();
        }
        self.running = running;
    }

    /// Puts King's conversation away when its last turn read too much,
    /// keeping it in the history.
    fn compact(&mut self, now: u64) -> Vec<Effect> {
        match self.coordinator.compact_if_long(now) {
            Some(archived) => {
                self.keep_archived(Some(archived));
                vec![Effect::Changed]
            }
            None => Vec::new(),
        }
    }

    /// Where a past conversation's entries are kept.
    fn archived_path(&self, id: u64) -> PathBuf {
        self.paths.folder.join("history").join(format!("{id}.json"))
    }

    /// Writes a conversation just put away, and removes the ones that fell
    /// off the list.
    fn keep_archived(&self, archived: Option<leon_king::history::Archived>) {
        let Some(archived) = archived else {
            return;
        };
        let path = self.archived_path(archived.past.id);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::write(
                    &path,
                    serde_json::to_string(&archived.entries).unwrap_or_default(),
                )
            });
        if let Err(error) = written {
            tracing::warn!(%error, "a past conversation of King's could not be written");
        }
        for id in archived.dropped {
            let _ = std::fs::remove_file(self.archived_path(id));
        }
    }

    /// What a past conversation said.
    fn read_archived(&self, id: u64) -> Result<Vec<leon_king::transcript::Entry>, String> {
        if !self.coordinator.past().iter().any(|past| past.id == id) {
            return Err(format!("there is no past conversation {id}"));
        }
        let text = std::fs::read_to_string(self.archived_path(id))
            .map_err(|error| format!("past conversation {id} could not be read: {error}"))?;
        serde_json::from_str(&text).map_err(|error| error.to_string())
    }

    /// What is known of models' talents: the catalogue built in, and the
    /// one in Leon's data folder over it.
    fn catalogue(&self) -> Catalogue {
        let own = std::fs::read_to_string(self.paths.data_dir.join(models::FILE_NAME)).ok();
        Catalogue::with(own.as_deref()).unwrap_or_else(|error| {
            tracing::warn!(%error, "king-models.toml could not be read");
            Catalogue::with(None).unwrap_or_default()
        })
    }

    fn request(&mut self, request: Request) -> Result<(Value, Vec<Effect>), String> {
        let now = now_ms();
        let done = |effects: Vec<Effect>| Ok((Value::Null, effects));
        match request {
            // Kept per connection, in `input`.
            Request::Watch { .. } => done(Vec::new()),
            Request::Status | Request::Subscribe => {
                self.refresh_running();
                Ok((json!(self.status(now)), Vec::new()))
            }
            Request::Say { text } => done(self.coordinator.say(&text, now)),
            Request::Transcript {
                target,
                since,
                limit,
            } => {
                let conversation = self
                    .coordinator
                    .conversation(target)
                    .ok_or("there is no such conversation")?;
                let entries = conversation.transcript.since(since, limit.unwrap_or(50));
                Ok((json!(entries), Vec::new()))
            }
            Request::SetBrain {
                agent,
                model,
                effort,
                mode,
            } => {
                if leon_turn::adapter(&agent).is_none() {
                    return Err(format!("King cannot run on {agent:?}"));
                }
                let choice = Choice {
                    agent,
                    model,
                    effort,
                    tier: None,
                };
                done(self.coordinator.set_brain(&choice, mode))
            }
            Request::Projects => Ok((json!(self.projects()), Vec::new())),
            Request::Usage => {
                let standings = self.standings(now);
                Ok((
                    json!({
                        "agents": standings,
                        "windows": usage_windows(self.store.as_deref(), now),
                        "tokens_today": {
                            "king": self.coordinator.facts(now).king_today,
                            "workers": self.coordinator.facts(now).workers_today,
                        },
                        "report": self.report(now),
                        "spent_today_usd": self.coordinator.spent_today(now),
                        "daily_budget_usd": self.coordinator.config().limits.daily_budget_usd,
                        "limit_threshold": self.coordinator.config().limits.limit_threshold,
                        "tiers": self.coordinator.config().tiers,
                    }),
                    Vec::new(),
                ))
            }
            Request::Models => {
                let catalogue = self.catalogue();
                let models: Vec<Value> = self
                    .models
                    .iter()
                    .map(|model| {
                        json!({
                            "agent": model.agent,
                            "id": model.id,
                            "label": model.label,
                            "description": model.description,
                            "efforts": model.efforts,
                            "card": catalogue.card_for(&model.id, &model.label),
                        })
                    })
                    .collect();
                Ok((
                    json!({
                        "models": models,
                        "modes": self.modes,
                        "jobs": self.coordinator.config().jobs,
                        "tiers": self.coordinator.config().tiers,
                    }),
                    Vec::new(),
                ))
            }
            Request::ModelNote { card } => {
                let path = self.paths.data_dir.join(models::FILE_NAME);
                let mut own = match std::fs::read_to_string(&path) {
                    Ok(text) => Catalogue::parse(&text)
                        .map_err(|error| format!("{}: {error}", path.display()))?,
                    Err(_) => Catalogue::default(),
                };
                if card.pattern.trim().is_empty() {
                    return Err("a card needs `match`: a part of the model's id".to_owned());
                }
                own.put(card);
                let text = toml::to_string_pretty(&own).map_err(|error| error.to_string())?;
                Catalogue::parse(&text)?;
                std::fs::write(&path, text).map_err(|error| error.to_string())?;
                done(Vec::new())
            }
            Request::StartWorker(asked) => {
                let spec = self.worker_spec(*asked, now)?;
                let (id, effects) = self.coordinator.start_worker(spec, now)?;
                let worker = self.coordinator.worker(id).ok_or("the worker went")?;
                let mut answer = json!({
                    "worker": id,
                    "agent": worker.conversation.agent,
                    "model": worker.conversation.model,
                    "effort": worker.conversation.effort,
                    "tier": worker.tier,
                    "cwd": worker.cwd,
                });
                // An agent says its own default model only once its turn
                // starts: King is told where to read it then.
                if worker.conversation.model.is_none() {
                    answer["model_note"] = json!(
                        "No model was named, so the worker runs on its agent's default. Its \
                         first turn says which: king_status shows it within seconds, and the \
                         news of the worker names it."
                    );
                }
                Ok((answer, effects))
            }
            Request::TellWorker { worker, text } => {
                done(self.coordinator.tell_worker(worker, &text, now)?)
            }
            Request::Answer {
                target,
                question,
                reply,
            } => done(self.coordinator.answer(target, &question, &reply)?),
            Request::Interrupt { target } => done(self.coordinator.interrupt(target)?),
            Request::StopWorker { worker } => done(self.coordinator.stop_worker(worker, now)?),
            Request::Release { worker } => {
                let (released, effects) = self.coordinator.release(worker, now)?;
                Ok((
                    json!({
                        "worker": released.id,
                        "agent": released.conversation.agent,
                        "session_id": released.conversation.session_id,
                        "cwd": released.cwd,
                    }),
                    effects,
                ))
            }
            Request::NewConversation => {
                let (archived, effects) = self.coordinator.new_conversation(now)?;
                let id = archived.as_ref().map(|archived| archived.past.id);
                self.keep_archived(archived);
                Ok((json!({ "archived": id }), effects))
            }
            Request::Conversations => {
                let mut past = self.coordinator.past().to_vec();
                past.reverse();
                Ok((json!(past), Vec::new()))
            }
            Request::PastConversation { id } => Ok((json!(self.read_archived(id)?), Vec::new())),
            Request::ResumeConversation { id } => {
                let entries = self.read_archived(id)?;
                let (archived, effects) = self.coordinator.resume_conversation(id, entries, now)?;
                self.keep_archived(archived);
                let _ = std::fs::remove_file(self.archived_path(id));
                Ok((Value::Null, effects))
            }
            Request::StopProcess { pid } => {
                self.refresh_running();
                if !self.running.iter().any(|process| process.pid == pid) {
                    return Err(format!(
                        "process {pid} is not one a turn of King's or of a worker left running"
                    ));
                }
                super::linger::terminate(pid);
                std::thread::sleep(Duration::from_millis(300));
                self.refresh_running();
                done(vec![Effect::Changed])
            }
            Request::Notify { title, body } => done(vec![Effect::Notify {
                title,
                body,
                echo: false,
            }]),
        }
    }

    /// Carries out effects, and the effects they lead to, then saves and
    /// tells the clients once if anything changed.
    fn carry_out(&mut self, effects: Vec<Effect>) {
        let mut waiting: std::collections::VecDeque<Effect> = effects.into();
        let mut changed = false;
        while let Some(effect) = waiting.pop_front() {
            match effect {
                Effect::Changed => changed = true,
                Effect::Start { target, prompt } => waiting.extend(self.start(target, &prompt)),
                Effect::Act { target, action } => {
                    let Some(running) = self.turns.get_mut(&target) else {
                        continue;
                    };
                    match running.turn.act(action) {
                        Ok(events) => {
                            let now = now_ms();
                            for event in &events {
                                waiting.extend(self.coordinator.event(target, event, now));
                            }
                        }
                        Err(error) => tracing::warn!(?target, %error, "an action was refused"),
                    }
                }
                Effect::Kill { target } => {
                    if let Some(running) = self.turns.get_mut(&target) {
                        running.turn.kill();
                    }
                }
                Effect::Notify { title, body, echo } => self.notify(title, body, echo),
            }
        }
        if changed {
            self.save();
            let notice = Notice::Changed {
                status: Box::new(self.status(now_ms())),
            };
            self.broadcast(&notice);
        }
    }

    fn broadcast(&mut self, notice: &Notice) {
        let line = serde_json::to_string(notice).unwrap_or_default();
        self.subscribers
            .retain(|subscriber| subscriber.send(line.clone()).is_ok());
    }

    fn notify(&mut self, title: String, body: String, echo: bool) {
        // What the person is looking at already does not also pop up.
        if !(echo && self.watchers.any()) {
            crate::king::serve::desktop_notification(&title, &body);
        }
        self.broadcast(&Notice::Notify { title, body, echo });
    }

    fn save(&self) {
        let Ok(text) = serde_json::to_string(self.coordinator.state()) else {
            return;
        };
        let temporary = self.paths.state.with_extension("json.new");
        let written = std::fs::write(&temporary, text)
            .and_then(|()| std::fs::rename(&temporary, &self.paths.state));
        if let Err(error) = written {
            tracing::warn!(%error, "King's state could not be saved");
        }
    }

    /// Starts a turn of `target`; what it leads to, if it cannot start.
    fn start(&mut self, target: Target, prompt: &str) -> Vec<Effect> {
        let now = now_ms();
        let Some(plan) = self.coordinator.plan(target) else {
            return Vec::new();
        };
        let Some(adapter) = leon_turn::adapter(&plan.agent) else {
            let why = format!("{} is not an agent Leon can drive", plan.agent);
            return self.coordinator.failed_to_start(target, &why, now);
        };
        let mut env = vec![
            (
                leon_memory::ENV_BIN.to_owned(),
                self.exe.to_string_lossy().into_owned(),
            ),
            (
                leon_memory::ENV_DATA_DIR.to_owned(),
                self.paths.data_dir.to_string_lossy().into_owned(),
            ),
        ];
        let (cwd, servers, add_dirs) = match target {
            Target::Brain => {
                self.write_instructions();
                env.push((
                    leon_king::ENV_SOCKET.to_owned(),
                    self.paths.socket.to_string_lossy().into_owned(),
                ));
                let roots: Vec<String> = self
                    .projects()
                    .into_iter()
                    .filter(|project| project.local)
                    .map(|project| project.root)
                    .collect();
                (
                    self.paths.folder.to_string_lossy().into_owned(),
                    self.tool_servers(),
                    roots,
                )
            }
            Target::Worker(_) => (plan.cwd.clone().unwrap_or_default(), Vec::new(), Vec::new()),
        };
        let program = adapter.program.to_owned();
        let spec = TurnSpec {
            program: &program,
            prompt,
            resume: plan.resume.as_deref(),
            mode: plan.mode.as_deref(),
            model: plan.model.as_deref(),
            effort: plan.effort.as_deref(),
            cwd: Some(&cwd),
            add_dirs: &add_dirs,
            mcp_servers: &servers,
            instructions: plan.instructions.as_deref(),
        };
        let start = Start {
            machine: Machine {
                id: MachineId::local(),
                name: "This computer".to_owned(),
                kind: MachineKind::Local,
            },
            argv: (adapter.argv)(&spec),
            cwd: cwd.clone(),
            env,
        };
        self.generation += 1;
        let generation = self.generation;
        match Turn::start(adapter, &spec, start, now, &*self.spawner) {
            Ok((turn, output, events)) => {
                let inputs = self.inputs.clone();
                std::thread::spawn(move || {
                    for output in output.iter() {
                        let last = matches!(output, Output::Exited(_));
                        let input = Input::Output {
                            target,
                            generation,
                            output,
                        };
                        if inputs.send(input).is_err() || last {
                            break;
                        }
                    }
                });
                self.turns.insert(
                    target,
                    Running {
                        generation,
                        turn,
                        finished: false,
                    },
                );
                events
                    .iter()
                    .flat_map(|event| self.coordinator.event(target, event, now))
                    .collect()
            }
            Err(error) => self
                .coordinator
                .failed_to_start(target, &error.to_string(), now),
        }
    }

    /// The servers of tools King's agent is given.
    fn tool_servers(&self) -> Vec<McpServer> {
        let exe = self.exe.to_string_lossy().into_owned();
        let data_dir = self.paths.data_dir.to_string_lossy().into_owned();
        let env = vec![(leon_memory::ENV_DATA_DIR.to_owned(), data_dir.clone())];
        let server = |name: &str, args: &[&str]| McpServer {
            name: name.to_owned(),
            command: exe.clone(),
            args: ["--data-dir", data_dir.as_str()]
                .iter()
                .chain(args)
                .map(|arg| (*arg).to_owned())
                .collect(),
            env: env.clone(),
        };
        vec![
            server("leon-king", &["king", "mcp"]),
            server("leon-memory", &["memory", "mcp"]),
            server("leon-tasks", &["task", "mcp", "--anywhere"]),
        ]
    }

    /// Rewrites King's instruction files with what Leon knows now.
    fn write_instructions(&self) {
        let lines: Vec<ProjectLine> = self
            .projects()
            .into_iter()
            .map(|project| ProjectLine {
                name: project.name,
                machine: project.machine,
                root: project.root,
                branches: project.branches,
            })
            .collect();
        let models = models::render(&self.models, &self.catalogue());
        let usage = report::line(&self.report(now_ms()).facts());
        let text = prompt::king(
            self.coordinator.config(),
            &prompt::Now {
                projects: &lines,
                models: &models,
                usage: &usage,
            },
        );
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let path = self.paths.folder.join(name);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                if let Err(error) = std::fs::write(&path, &text) {
                    tracing::warn!(%error, "King's instructions could not be written");
                }
            }
        }
    }

    /// The projects Leon knows.
    fn projects(&self) -> Vec<ProjectInfo> {
        let Some(store) = self.store.as_deref() else {
            return Vec::new();
        };
        let machines = store.machines().unwrap_or_default();
        let projects = store.projects(None).unwrap_or_default();
        projects
            .into_iter()
            .map(|project| {
                let machine = machines.iter().find(|m| m.id == project.machine_id);
                let mut worktrees = store.worktrees(&project.id).unwrap_or_default();
                worktrees.sort_by_key(|worktree| !worktree.is_main);
                ProjectInfo {
                    name: project.name.clone(),
                    machine: machine
                        .map(|m| m.name.clone())
                        .unwrap_or_else(|| "?".to_owned()),
                    local: machine.is_some_and(|m| m.kind == MachineKind::Local),
                    root: project.root.clone(),
                    branches: worktrees
                        .iter()
                        .filter_map(|worktree| worktree.branch.clone())
                        .collect(),
                    worktrees: worktrees
                        .iter()
                        .map(|worktree| {
                            json!({
                                "path": worktree.path,
                                "branch": worktree.branch,
                                "main": worktree.is_main,
                            })
                        })
                        .collect(),
                }
            })
            .collect()
    }

    /// What a worker is started with: its folder, made when needed, and
    /// the agent the router chose.
    fn worker_spec(&mut self, asked: WorkerRequest, now: u64) -> Result<WorkerSpec, String> {
        let ask = Ask {
            tier: asked.tier.clone(),
            agent: asked.agent.clone(),
            model: asked.model.clone(),
            effort: asked.effort.clone(),
            job: asked.job.as_str().to_owned(),
        };
        let standings = self.standings(now);
        let choice = route::route(
            self.coordinator.config(),
            &ask,
            &standings,
            self.coordinator.spent_today(now),
        )?;
        let projects = self.projects();
        let (cwd, project) = match (&asked.cwd, &asked.project) {
            (Some(cwd), _) => {
                let project = projects
                    .iter()
                    .find(|project| Path::new(cwd).starts_with(&project.root));
                if projects.iter().any(|project| project.root == *cwd) {
                    return Err(
                        "that is a project's main checkout; a worker works in a worktree"
                            .to_owned(),
                    );
                }
                (cwd.clone(), project.map(|project| project.name.clone()))
            }
            (None, Some(name)) => {
                let project = projects
                    .iter()
                    .find(|project| project.name == *name)
                    .ok_or_else(|| format!("Leon knows no project {name:?}"))?;
                if !project.local {
                    return Err(format!(
                        "{name} is on {}; workers run on this computer for now",
                        project.machine
                    ));
                }
                let branch = asked
                    .branch
                    .clone()
                    .unwrap_or_else(|| branch_for(&asked.title, now));
                let path = add_worktree(
                    &project.root,
                    &branch,
                    &self.coordinator.config().guards.protected_branches,
                )?;
                (path, Some(project.name.clone()))
            }
            (None, None) => return Err("name the project, or the folder to work in".to_owned()),
        };
        Ok(WorkerSpec {
            title: asked.title,
            prompt: asked.prompt,
            cwd,
            project,
            job: asked.job,
            choice,
            mode: asked.mode,
        })
    }
}

impl Service {
    /// Where each agent stands, with what is installed found out at most
    /// every few minutes (each look is a login shell per agent).
    fn standings(&mut self, now: u64) -> Vec<Standing> {
        let fresh = self
            .installed
            .as_ref()
            .is_some_and(|(at, _)| now.saturating_sub(*at) < INSTALLED_FOR_MS);
        if !fresh {
            let found = leon_turn::ADAPTERS
                .iter()
                .map(|adapter| installed(adapter.program))
                .collect();
            self.installed = Some((now, found));
        }
        let found = self
            .installed
            .as_ref()
            .map(|(_, found)| found.clone())
            .unwrap_or_default();
        standings(self.store.as_deref(), now, &found)
    }
}

/// A project as the service sees it.
#[derive(Debug, Clone, serde::Serialize)]
struct ProjectInfo {
    name: String,
    machine: String,
    local: bool,
    root: String,
    branches: Vec<String>,
    worktrees: Vec<Value>,
}

/// Where each agent stands: installed on this computer (`installed`, in
/// the order of the catalogue), and the largest share of its usage windows
/// Leon read last.
fn standings(store: Option<&Store>, now_ms: u64, installed: &[bool]) -> Vec<Standing> {
    let now = (now_ms / 1000) as i64;
    let readings: Vec<leon_usage::AgentUsage> = store
        .and_then(|store| store.usage_readings().ok())
        .unwrap_or_default()
        .iter()
        .filter(|row| row.machine == MachineId::local())
        .filter_map(|row| serde_json::from_str(&row.payload).ok())
        .collect();
    leon_turn::ADAPTERS
        .iter()
        .zip(installed.iter().copied().chain(std::iter::repeat(false)))
        .map(|(adapter, installed)| {
            let used = readings
                .iter()
                .filter(|usage| usage.agent.as_str() == adapter.id)
                .filter_map(|usage| match usage.effective(now) {
                    leon_usage::Effective::Windows(windows) => windows
                        .iter()
                        .map(|window| window.used_percent / 100.0)
                        .reduce(f64::max),
                    leon_usage::Effective::Unknown(_) => None,
                })
                .reduce(f64::max);
            Standing {
                agent: adapter.id.to_owned(),
                installed,
                used,
            }
        })
        .collect()
}

/// The agents' usage windows on this computer, as Leon last read them.
fn usage_windows(store: Option<&Store>, now_ms: u64) -> Vec<AgentWindows> {
    let now = (now_ms / 1000) as i64;
    let readings: Vec<leon_usage::AgentUsage> = store
        .and_then(|store| store.usage_readings().ok())
        .unwrap_or_default()
        .iter()
        .filter(|row| row.machine == MachineId::local())
        .filter_map(|row| serde_json::from_str(&row.payload).ok())
        .collect();
    let mut agents: Vec<AgentWindows> = Vec::new();
    for usage in &readings {
        let leon_usage::Effective::Windows(windows) = usage.effective(now) else {
            continue;
        };
        let mut label = usage.agent.as_str().to_owned();
        if let Some(plan) = &usage.plan {
            label = format!("{label} ({plan})");
        }
        agents.push(AgentWindows {
            agent: label,
            windows: windows
                .iter()
                .map(|window| Window {
                    label: window.kind.short(),
                    used: window.used_percent / 100.0,
                    resets_in_secs: window.resets_at.map(|at| at - now),
                })
                .collect(),
        });
    }
    agents
}

/// Asks every installed agent which models it offers, the way the window's
/// conversations ask (`leon_turn::Adapter::options`).
fn probe_models(spawner: &dyn Spawner) -> (Vec<Available>, Vec<AvailableMode>) {
    let mut found = Vec::new();
    let mut modes = Vec::new();
    for adapter in leon_turn::ADAPTERS {
        if !installed(adapter.program) {
            continue;
        }
        let mut argv = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            adapter.options.script.to_owned(),
            "leon".to_owned(),
            adapter.program.to_owned(),
        ];
        argv.extend((adapter.options.arguments)());
        let start = Start {
            machine: Machine {
                id: MachineId::local(),
                name: "This computer".to_owned(),
                kind: MachineKind::Local,
            },
            argv,
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            env: Vec::new(),
        };
        let Ok((_transport, output)) = spawner.spawn(start) else {
            continue;
        };
        let mut printed = String::new();
        while let Ok(said) = output.recv_timeout(Duration::from_secs(60)) {
            match said {
                Output::Line(line) => {
                    printed.push_str(&line);
                    printed.push('\n');
                }
                Output::Noise(_) => {}
                Output::Exited(_) => break,
            }
        }
        let options = (adapter.options.parse)(&printed);
        modes.extend(options.modes.into_iter().map(|mode| AvailableMode {
            agent: adapter.id.to_owned(),
            id: mode.id,
            label: mode.label,
            description: mode.description,
            dangerous: mode.dangerous,
        }));
        found.extend(options.models.into_iter().map(|model| Available {
            agent: adapter.id.to_owned(),
            id: model.id,
            label: model.label,
            description: model.description,
            efforts: model.efforts,
        }));
    }
    (found, modes)
}

/// Whether `program` is on the login shell's path.
fn installed(program: &str) -> bool {
    let argv = leon_turn::shell::login_argv(vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "command -v \"$1\" >/dev/null".to_owned(),
        "sh".to_owned(),
        program.to_owned(),
    ]);
    let mut argv = argv.into_iter();
    let mut command = leon_remote::spawn::std_child(argv.next().unwrap_or_default());
    command
        .args(argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.status().is_ok_and(|status| status.success())
}

/// A branch for a worker: `king/` and its title in a few words.
fn branch_for(title: &str, now_ms: u64) -> String {
    let words: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug: Vec<&str> = words
        .split('-')
        .filter(|word| !word.is_empty())
        .take(5)
        .collect();
    let slug = if slug.is_empty() {
        "work".to_owned()
    } else {
        slug.join("-")
    };
    format!("king/{slug}-{}", (now_ms / 1000) % 100_000)
}

/// Makes a worktree of `root` on a new branch, `branch`, from what the
/// project's checkout has, and returns its folder. Never on the default
/// branch nor on a `protected` one.
fn add_worktree(root: &str, branch: &str, protected: &[String]) -> Result<String, String> {
    let git = |args: &[&str]| -> Result<String, String> {
        let output = leon_remote::spawn::std_child("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|error| format!("git: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    };
    let default = default_branch(root);
    if default.as_deref() == Some(branch) || protected.iter().any(|name| name == branch) {
        return Err(format!(
            "{branch} is protected (the default branch, or [guards] protected_branches); a \
             worker needs one of its own"
        ));
    }
    let path = crate::address::worktree_location("", root, branch)?;
    let exists = git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ])
    .is_ok();
    if exists {
        git(&["worktree", "add", &path, branch])?;
    } else {
        git(&["worktree", "add", "-b", branch, &path])?;
    }
    Ok(path)
}

/// The branch `origin/HEAD` names, else the one checked out at `root`.
pub(crate) fn default_branch(root: &str) -> Option<String> {
    let run = |args: &[&str]| {
        leon_remote::spawn::std_child("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    run(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .map(|name| name.trim_start_matches("origin/").to_owned())
        .or_else(|| run(&["symbolic-ref", "--short", "HEAD"]))
}

/// Tells the person on their desktop, when the desktop has a way.
pub(crate) fn desktop_notification(title: &str, body: &str) {
    let body = leon_king::transcript::cut(body, 300);
    #[cfg(target_os = "linux")]
    let command = leon_remote::spawn::std_child("notify-send")
        .args(["--app-name=Leon", title, &body])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    #[cfg(target_os = "macos")]
    let command = leon_remote::spawn::std_child("osascript")
        .arg("-e")
        .arg(format!(
            "display notification {} with title {}",
            applescript_text(&body),
            applescript_text(title)
        ))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Ok(mut child) = command {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(target_os = "macos")]
fn applescript_text(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_watches_until_it_says_so_or_goes_away() {
        let mut watchers = Watchers::default();
        let (window, window_end) = flume::unbounded::<String>();
        let (phone, phone_end) = flume::unbounded::<String>();
        assert!(!watchers.any());
        watchers.set(&window, true);
        // Said twice, kept once.
        watchers.set(&window, true);
        watchers.set(&phone, true);
        assert_eq!(watchers.list.len(), 2);
        watchers.set(&window, false);
        assert!(watchers.any(), "the phone still looks");
        drop(phone_end);
        assert!(!watchers.any(), "a connection that closed stops counting");
        drop(window_end);
    }

    #[test]
    fn a_workers_branch_is_its_title_in_a_few_words() {
        assert_eq!(
            branch_for("Fix the login bug, in the API!", 1_700_000_123_000),
            "king/fix-the-login-bug-in-123"
        );
        assert_eq!(branch_for("¿?", 5_000), "king/work-5");
    }

    #[cfg(leon_posix_tests)]
    #[test]
    fn a_worktree_is_made_on_a_new_branch_and_never_on_the_default_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            let status = leon_remote::spawn::std_child("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(status.status.success(), "{status:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "start",
        ]);
        let root = root.to_string_lossy().into_owned();
        let protected = ["release".to_owned()];
        assert!(add_worktree(&root, "main", &protected).is_err());
        assert!(add_worktree(&root, "release", &protected).is_err());
        let path = add_worktree(&root, "king/fix", &protected).unwrap();
        assert_eq!(path, format!("{root}-worktrees/king-fix"));
        assert!(Path::new(&path).join(".git").exists());
    }
}

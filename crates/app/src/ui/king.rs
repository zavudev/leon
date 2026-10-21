//! King Leon in the window: what the shell does with it.
//!
//! King's conversation and its workers live in King's service (`leon king
//! serve`, `crate::king`), never in the window: the window is one more client
//! of the service's socket, as the phone and `leon king …` are. Closing the
//! window stops nothing King does.
//!
//! The view takes the place of the main pane ([`Main::King`]), as the board
//! of tasks does, and the sidebar stays. On the left is King's conversation,
//! with the field the person speaks to King in; on the right the workers
//! King runs, as cards. A card opens the worker's own conversation in the
//! same column, live, with what can be done to it: tell it something (King
//! hears that the person stepped in), stop its turn, stop it, or take it
//! over. A worker taken over leaves King: the service lets it go and the
//! window opens it as a session of its own in the sidebar, which continues
//! the agent's conversation where the worker left it. Until then no worker
//! of King's is in the sidebar.
//!
//! **Reading.** The window asks the service for the status and for what
//! each conversation said after the last entry it has, and is told whenever
//! anything changes: every notice carries the new status, and a
//! conversation with entries the window does not have yet is asked for them
//! ([`wanted`]). Nothing is read on a timer.
//!
//! The drawing is `king_view.rs`.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, ScrollHandle, Subscription, Task, Window};
use leon_core::{AgentId, MachineId};
use leon_king::coordinator::{Form, Open, Reply as Answer, Target};
use leon_king::history::Past;
use leon_king::protocol::{Entries, Notice, Request, Status, TerminalAsk};
use leon_king::transcript::{Entry, KEPT};
use serde_json::Value;

use super::home::Nav;
use super::shell::{Main, Pane, Shell};
use super::steps::{KingInfo, KingMode, KingModel};
use super::terminals::Place;
use crate::engine::StatusKind;
use crate::king::link::Heard;
use crate::launch::Launch;

/// What a conversation said, as far as the window has it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Feed {
    /// Its entries, oldest first.
    pub entries: Vec<Entry>,
    /// A request for what came after the last one is on its way.
    pub asking: bool,
}

impl Feed {
    /// The number of the newest entry the window has.
    pub fn last(&self) -> Option<u64> {
        self.entries.last().map(|entry| entry.seq)
    }

    /// Adds what the service sent: only what comes after what is there, so
    /// that an answer that crossed another is never kept twice.
    pub fn add(&mut self, entries: Entries) {
        let last = self.last();
        self.entries.extend(
            entries
                .into_iter()
                .filter(|entry| last.is_none_or(|last| entry.seq > last)),
        );
        if self.entries.len() > KEPT {
            let extra = self.entries.len() - KEPT;
            self.entries.drain(..extra);
        }
    }
}

/// Which conversations have entries the window does not have yet and is
/// not already asking for: King's always, a worker's while its card is open.
pub fn wanted(status: &Status, brain: &Feed, open: Option<(u32, &Feed)>) -> Vec<Target> {
    let behind = |newest: Option<u64>, feed: &Feed| {
        !feed.asking && newest.is_some() && newest != feed.last()
    };
    let mut targets = Vec::new();
    if behind(status.brain.last_seq, brain) {
        targets.push(Target::Brain);
    }
    if let Some((id, feed)) = open {
        let newest = status
            .workers
            .iter()
            .find(|worker| worker.id == id)
            .and_then(|worker| worker.conversation.last_seq);
        if behind(newest, feed) {
            targets.push(Target::Worker(id));
        }
    }
    targets
}

/// How an answer of a question looks: the one that lets the agent go on,
/// another one, or the one that stops it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Lets it go on.
    Go,
    /// Another answer.
    Plain,
    /// Refuses.
    Stop,
}

/// What an answer does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// Answers with this.
    Reply(Answer),
    /// Answers with what is written in the conversation's field.
    Words,
}

/// One answer of a question, as a button and as Alt with its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pickable {
    /// What the button says.
    pub label: &'static str,
    /// How it looks.
    pub tone: Tone,
    /// What it does.
    pub pick: Pick,
}

/// The answers of a question, in the order they are drawn and numbered:
/// its own (the choices of a permission, or start and not yet for a plan),
/// then what is written in the field, which answers anything.
pub fn picks(open: &Open) -> Vec<Pickable> {
    let reply = |choice: Option<String>, approve: Option<bool>| {
        Pick::Reply(Answer {
            choice,
            approve,
            text: None,
        })
    };
    let mut all: Vec<Pickable> = match &open.form {
        Form::Permission { choices } => choices
            .iter()
            .map(|(id, effect)| {
                let (label, tone) = match effect.as_str() {
                    "allow" => ("ALLOW", Tone::Go),
                    "always" => ("ALWAYS ALLOW", Tone::Plain),
                    _ => ("DENY", Tone::Stop),
                };
                Pickable {
                    label,
                    tone,
                    pick: reply(Some(id.clone()), None),
                }
            })
            .collect(),
        Form::Plan => vec![
            Pickable {
                label: "START",
                tone: Tone::Go,
                pick: reply(None, Some(true)),
            },
            Pickable {
                label: "NOT YET",
                tone: Tone::Plain,
                pick: reply(None, Some(false)),
            },
        ],
        Form::Choices { .. } => Vec::new(),
    };
    all.push(Pickable {
        label: "ANSWER WITH WHAT IS WRITTEN",
        tone: Tone::Plain,
        pick: Pick::Words,
    });
    all
}

/// The models of the service's `models` answer.
fn models_of(value: &Value) -> Vec<KingModel> {
    // How each model did, per kind of job, as the service's record says.
    let record = |agent: &str, id: &str| -> Vec<String> {
        value["record"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|summary| {
                summary["agent"].as_str() == Some(agent) && summary["model"].as_str() == Some(id)
            })
            .filter_map(|summary| {
                Some(format!(
                    "{}: {}",
                    summary["job"].as_str()?,
                    summary["line"].as_str()?
                ))
            })
            .collect()
    };
    value["models"]
        .as_array()
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    let agent = model["agent"].as_str()?;
                    let id = model["id"].as_str()?;
                    Some(KingModel {
                        record: record(agent, id),
                        agent: agent.to_owned(),
                        id: id.to_owned(),
                        label: model["label"].as_str().or(model["id"].as_str())?.to_owned(),
                        efforts: model["efforts"]
                            .as_array()
                            .map(|efforts| {
                                efforts
                                    .iter()
                                    .filter_map(|effort| effort.as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The permission modes of the service's `models` answer.
fn modes_of(value: &Value) -> Vec<KingMode> {
    value["modes"]
        .as_array()
        .map(|modes| {
            modes
                .iter()
                .filter_map(|mode| {
                    Some(KingMode {
                        agent: mode["agent"].as_str()?.to_owned(),
                        id: mode["id"].as_str()?.to_owned(),
                        label: mode["label"].as_str().or(mode["id"].as_str())?.to_owned(),
                        description: mode["description"].as_str().unwrap_or_default().to_owned(),
                        dangerous: mode["dangerous"].as_bool().unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What is said with files attached: the words, then the files' paths, one
/// a line, for the agent to open. `None` when there is nothing to say.
pub fn with_attached(typed: &str, files: &[std::path::PathBuf]) -> Option<String> {
    if files.is_empty() {
        return (!typed.is_empty()).then(|| typed.to_owned());
    }
    let mut text = if typed.is_empty() {
        "Look at the attached files.".to_owned()
    } else {
        typed.to_owned()
    };
    text.push_str("\n\nAttached (open each path):");
    for file in files {
        text.push_str(&format!("\n- {}", file.display()));
    }
    Some(text)
}

/// What King is told of a task given to it from the board.
pub fn task_for_king(task: &leon_core::Task, place: &str) -> String {
    let mut text = format!(
        "Task {} {place}: {}\n\nWork this task (id {}).",
        task.short_id(),
        task.title,
        task.id
    );
    if !task.body.trim().is_empty() {
        text.push_str(&format!("\n\nWhat the task says:\n{}", task.body.trim()));
    }
    if let Some(branch) = &task.branch {
        text.push_str(&format!("\n\nIts branch: {branch}."));
    }
    text
}

/// The card to open from `open` going down (or up) the cards `ids`, as
/// drawn; `None` is none open (above the first one).
pub fn step_card(ids: &[u32], open: Option<u32>, down: bool) -> Option<u32> {
    let at = open.and_then(|open| ids.iter().position(|id| *id == open));
    match (at, down) {
        (None, true) => ids.first().copied(),
        (None, false) => None,
        (Some(at), true) => ids.get(at + 1).or(ids.get(at)).copied(),
        (Some(0), false) => None,
        (Some(at), false) => ids.get(at - 1).copied(),
    }
}

/// What a reply that comes back is the answer to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asked {
    /// The status.
    Status,
    /// Entries of a conversation.
    Transcript(Target),
    /// A worker the person takes over.
    Release(u32),
    /// The models the agents offer.
    Models,
    /// The past conversations.
    Conversations,
    /// What a past conversation said.
    Past(u64),
    /// A past conversation taken up again.
    Resumed,
    /// A run of a task's steps started.
    Run,
    /// A terminal King asked for, this window's to open.
    Terminal,
    /// Anything else: only a refusal is said.
    Done,
}

/// What the window keeps of King.
pub struct KingUi {
    /// What was said to King before the line was open: said once it is.
    pending: Vec<Request>,
    /// The terminals King asked for that this window asked to take.
    taking: HashSet<u64>,
    /// The line to the service, once it is open.
    link: Option<flume::Sender<(u64, Request)>>,
    /// A connection is being opened.
    pub connecting: bool,
    /// Why there is no line, when there is none.
    pub error: Option<String>,
    /// King and its workers, as the service last said.
    pub status: Option<Status>,
    /// King's conversation.
    pub brain: Feed,
    /// The conversations of the workers whose card was opened.
    pub workers: HashMap<u32, Feed>,
    /// The worker whose card is open.
    pub open: Option<u32>,
    /// Where the person speaks to King.
    pub input: Option<Entity<TextareaState>>,
    /// Where the person speaks to the open worker.
    pub worker_input: Option<Entity<TextareaState>>,
    /// The scroll of King's conversation.
    pub scroll: ScrollHandle,
    /// The scroll of the open worker's conversation.
    pub worker_scroll: ScrollHandle,
    /// The runs of tools opened by hand, by the number of their first entry.
    pub open_tools: HashSet<(Target, u64)>,
    /// The models the agents of this computer offer, as the service found
    /// them: what King can be moved to.
    pub models: Vec<KingModel>,
    /// The permission modes they offer.
    pub modes: Vec<KingMode>,
    /// King's past conversations, newest first, while the column of the
    /// workers shows them instead.
    pub history: Option<Vec<Past>>,
    /// The files attached to what is said next to King or to a worker
    /// (pasted images among them), shown over the field and sent with it.
    pub attached: HashMap<Target, Vec<std::path::PathBuf>>,
    /// A past conversation on screen instead of the one now, read only.
    pub viewing: Option<(Past, Vec<Entry>)>,
    /// What the service was last told: the person is looking at King here.
    pub(super) watching: bool,
    /// The last thing King's service had for the person.
    pub note: Option<(String, String)>,
    /// The list of speech models is open over King's field.
    pub voices: bool,
    /// What the main pane showed before, to put back.
    pub back: Option<(Main, Pane)>,
    asked: HashMap<u64, Asked>,
    next_tag: u64,
    reader: Option<Task<()>>,
    /// Draws the view again while a turn goes on, for its spinner.
    spinning: Option<Task<()>>,
    subscriptions: Vec<Subscription>,
}

impl Default for KingUi {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            taking: HashSet::new(),
            link: None,
            connecting: false,
            error: None,
            status: None,
            brain: Feed::default(),
            workers: HashMap::new(),
            open: None,
            input: None,
            worker_input: None,
            scroll: ScrollHandle::new(),
            worker_scroll: ScrollHandle::new(),
            open_tools: HashSet::new(),
            models: Vec::new(),
            modes: Vec::new(),
            history: None,
            viewing: None,
            attached: HashMap::new(),
            watching: false,
            note: None,
            voices: false,
            back: None,
            asked: HashMap::new(),
            next_tag: 1,
            reader: None,
            spinning: None,
            subscriptions: Vec::new(),
        }
    }
}

impl KingUi {
    /// Whether the line to the service is open.
    pub fn connected(&self) -> bool {
        self.link.is_some()
    }

    /// The feed of whichever conversation `target` is.
    pub fn feed(&self, target: Target) -> Option<&Feed> {
        match target {
            Target::Brain => Some(&self.brain),
            Target::Worker(id) => self.workers.get(&id),
        }
    }
}

/// Whether a scroll is at its end, or near enough that what comes next
/// should keep it there.
fn at_end(scroll: &ScrollHandle) -> bool {
    scroll.max_offset().y + scroll.offset().y < gpui_kit::px(48.)
}

impl Shell {
    /// Whether King's view is what the main pane shows.
    pub(super) fn king_open(&self) -> bool {
        matches!(self.main, Main::King)
    }

    /// Opens King's view, or closes it when it is open.
    pub(super) fn toggle_king(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.king_open() {
            self.close_king(window, cx);
            return;
        }
        self.close_overlay(window, cx);
        if self.tasks_open() {
            self.close_tasks(window, cx);
        }
        let back = if self.den_open() {
            let back = self.den_take_back();
            self.den_leave(cx);
            back.unwrap_or((Main::Empty, Pane::Sidebar))
        } else {
            (std::mem::replace(&mut self.main, Main::Empty), self.pane)
        };
        self.main = Main::King;
        self.king_ui.back = Some(back);
        self.king_spin(cx);
        self.home.nav = Some(Nav::King);
        self.pane = Pane::Main;
        self.king_fields(window, cx);
        self.king_connect(window, cx);
        self.king_watch(cx);
        self.king_ui.scroll.scroll_to_bottom();
        self.sync_focus(window, cx);
        cx.notify();
    }

    /// Closes King's view and puts back what the main pane showed. King
    /// goes on: the line to its service stays open for the sidebar's row.
    pub(super) fn close_king(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.king_open() {
            return;
        }
        let (main, pane) = self
            .king_ui
            .back
            .take()
            .unwrap_or((Main::Empty, Pane::Sidebar));
        match main {
            Main::Live(id) if self.live.get(id).is_some() => {
                self.main = Main::Empty;
                self.open_live(id, window, cx);
                self.pane = pane;
            }
            Main::Live(_) | Main::Den | Main::Tasks | Main::King => {
                self.main = Main::Empty;
                self.pane = Pane::Sidebar;
            }
            other => {
                self.main = other;
                self.pane = pane;
            }
        }
        self.home.nav = matches!(self.main, Main::Empty).then_some(Nav::Home);
        self.king_watch(cx);
        self.sync_focus(window, cx);
        cx.notify();
    }

    /// Tells the service whether the person is looking at King here: King's
    /// view is open in the window that is in front. While they are, the
    /// service sends nothing to the desktop that the view shows already.
    pub(super) fn king_watch(&mut self, cx: &mut Context<Self>) {
        let looking = self.king_open() && self.window_active;
        if !self.king_ui.connected() || looking == self.king_ui.watching {
            return;
        }
        self.king_ui.watching = looking;
        self.king_ask(Request::Watch { on: looking }, Asked::Done, cx);
    }

    /// What the palette's questions know of King.
    pub(super) fn king_info(&self) -> Option<KingInfo> {
        let brain = &self.king_ui.status.as_ref()?.brain;
        Some(KingInfo {
            agent: brain.agent.clone(),
            model: brain.model.clone(),
            effort: brain.effort.clone(),
            mode: brain.mode.clone(),
            models: self.king_ui.models.clone(),
            modes: self.king_ui.modes.clone(),
        })
    }

    /// Runs King on another agent, model or effort, from its next turn.
    pub(super) fn king_set_brain(
        &mut self,
        agent: String,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.king_ask(
            Request::SetBrain {
                agent,
                model,
                effort,
                mode,
            },
            Asked::Done,
            cx,
        );
    }

    /// The question the keys answer: the open worker's, else King's. The
    /// other workers' questions are King's to answer.
    pub(super) fn king_question_in_view(&self) -> Option<(Target, &Open)> {
        let status = self.king_ui.status.as_ref()?;
        if let Some(id) = self.king_ui.open {
            let worker = status.workers.iter().find(|worker| worker.id == id)?;
            if let Some(open) = worker.conversation.questions.first() {
                return Some((Target::Worker(id), open));
            }
        }
        status
            .brain
            .questions
            .first()
            .map(|open| (Target::Brain, open))
    }

    /// The fields the person writes in, made the first time the view opens.
    fn king_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.king_ui.input.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .submit_on_enter(true)
                .placeholder("Ask King anything: a question, a feature, a review")
        });
        let worker_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder("Tell this worker something; King hears you stepped in")
        });
        let to_king = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                this.king_send(window, cx);
            }
        });
        let to_worker = cx.subscribe_in(
            &worker_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.king_tell_worker(window, cx);
                }
            },
        );
        self.king_ui.input = Some(input);
        self.king_ui.worker_input = Some(worker_input);
        self.king_ui.subscriptions = vec![to_king, to_worker];
    }

    /// Whether one of King's fields has the keyboard.
    pub(super) fn king_typing(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        use gpui_kit::Focusable as _;
        self.king_open()
            && [&self.king_ui.input, &self.king_ui.worker_input]
                .into_iter()
                .flatten()
                .any(|field| field.focus_handle(cx).is_focused(window))
    }

    /// Gives the keyboard to the field of what is open: the worker's when
    /// its card is, else King's. `true` when King's view has the keyboard.
    pub(super) fn king_focus(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.king_open() || self.pane != Pane::Main {
            return false;
        }
        if self.king_typing(window, cx) {
            return true;
        }
        let field = match self.king_ui.open {
            Some(_) => self.king_ui.worker_input.clone(),
            None => self.king_ui.input.clone(),
        };
        if let Some(field) = field {
            field.update(cx, |field, cx| field.focus(window, cx));
        }
        true
    }

    /// Opens the line to King's service, starting the service when nothing
    /// answers. Nothing happens while one is open or being opened, or where
    /// the window has no link (tests that do not need one).
    pub(super) fn king_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.king_connect_as(true, window, cx);
    }

    /// Opens the line to King's service when the service runs, without
    /// starting it: the window follows King (its workers in the Den, the
    /// terminals it asks for) from then on. Nothing is said when nothing
    /// answers.
    pub(super) fn king_join(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.king_connect_as(false, window, cx);
    }

    fn king_connect_as(&mut self, start: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.king_ui.link.is_some() || self.king_ui.connecting {
            return;
        }
        let Some(link) = self.options.king.clone() else {
            if start {
                self.king_ui.error = Some("King is not available in this window.".to_owned());
            }
            return;
        };
        self.king_ui.connecting = true;
        if start {
            self.king_ui.error = None;
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            let opened = cx
                .background_spawn(async move {
                    if start {
                        link.open()
                    } else {
                        link.join()
                    }
                })
                .await;
            let heard = match opened {
                Ok(connection) => {
                    tracing::info!(started = start, "on the line to King's service");
                    let heard = connection.heard;
                    let ok = this.update(cx, |this, cx| {
                        this.king_ui.connecting = false;
                        this.king_ui.link = Some(connection.ask);
                        this.king_ui.brain.asking = false;
                        for feed in this.king_ui.workers.values_mut() {
                            feed.asking = false;
                        }
                        this.king_ask(Request::Status, Asked::Status, cx);
                        this.king_ask(Request::Models, Asked::Models, cx);
                        for request in std::mem::take(&mut this.king_ui.pending) {
                            let asked = match request {
                                Request::RunStart { .. } => Asked::Run,
                                _ => Asked::Done,
                            };
                            this.king_ask(request, asked, cx);
                        }
                        this.king_ui.watching = false;
                        this.king_watch(cx);
                        cx.notify();
                    });
                    if ok.is_err() {
                        return;
                    }
                    heard
                }
                Err(error) => {
                    if !start {
                        tracing::debug!(%error, "King's service is not there to join");
                    }
                    this.update(cx, |this, cx| {
                        this.king_ui.connecting = false;
                        if start {
                            this.king_ui.error = Some(error);
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            while let Ok(said) = heard.recv_async().await {
                let lost = matches!(said, Heard::Lost(_));
                if this
                    .update_in(cx, |this, window, cx| this.king_heard(said, window, cx))
                    .is_err()
                    || lost
                {
                    break;
                }
            }
        });
        self.king_ui.reader = Some(task);
    }

    /// Sends a request, remembering what its reply is for. A line that is
    /// gone is said, and opened again the next time the view opens.
    fn king_ask(&mut self, request: Request, asked: Asked, cx: &mut Context<Self>) {
        let Some(link) = &self.king_ui.link else {
            return;
        };
        let tag = self.king_ui.next_tag;
        self.king_ui.next_tag += 1;
        if link.send((tag, request)).is_err() {
            self.king_lost("The line to King's service closed.".to_owned(), cx);
            return;
        }
        self.king_ui.asked.insert(tag, asked);
    }

    /// The line is gone.
    fn king_lost(&mut self, why: String, cx: &mut Context<Self>) {
        self.king_ui.link = None;
        self.king_ui.reader = None;
        self.king_ui.asked.clear();
        self.king_ui.watching = false;
        self.king_ui.error = Some(why);
        cx.notify();
    }

    /// What the service said.
    fn king_heard(&mut self, said: Heard, window: &mut Window, cx: &mut Context<Self>) {
        match said {
            Heard::Lost(why) => self.king_lost(format!("King's service went away: {why}"), cx),
            Heard::Notice(Notice::Changed { status }) => self.king_status(*status, cx),
            // What the conversation shows already is not said twice.
            Heard::Notice(Notice::Notify { echo: true, .. }) => {}
            Heard::Notice(Notice::Notify { title, body, .. }) => {
                self.king_ui.note = Some((title, body));
                cx.notify();
            }
            Heard::Reply { tag, result } => {
                let Some(asked) = self.king_ui.asked.remove(&tag) else {
                    return;
                };
                self.king_replied(asked, result, window, cx);
            }
        }
    }

    /// The answer to something the window asked.
    fn king_replied(
        &mut self,
        asked: Asked,
        result: Result<Value, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (asked, result) {
            (Asked::Status, Ok(value)) => {
                if let Ok(status) = serde_json::from_value(value) {
                    self.king_status(status, cx);
                }
            }
            (Asked::Transcript(target), result) => {
                let entries: Entries = result
                    .ok()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default();
                let scroll = match target {
                    Target::Brain => self.king_ui.scroll.clone(),
                    Target::Worker(_) => self.king_ui.worker_scroll.clone(),
                };
                let follow = at_end(&scroll);
                let feed = match target {
                    Target::Brain => Some(&mut self.king_ui.brain),
                    Target::Worker(id) => self.king_ui.workers.get_mut(&id),
                };
                if let Some(feed) = feed {
                    feed.asking = false;
                    feed.add(entries);
                }
                if follow {
                    scroll.scroll_to_bottom();
                }
                // More may have come while this was on its way.
                if let Some(status) = self.king_ui.status.clone() {
                    self.king_fetch(&status, cx);
                }
                cx.notify();
            }
            (Asked::Release(_), Ok(value)) => self.king_extract(&value, window, cx),
            (Asked::Conversations, Ok(value)) => {
                self.king_ui.history = Some(serde_json::from_value(value).unwrap_or_default());
                cx.notify();
            }
            (Asked::Past(id), Ok(value)) => {
                let past = self
                    .king_ui
                    .history
                    .as_ref()
                    .and_then(|list| list.iter().find(|past| past.id == id).cloned());
                if let (Some(past), Ok(entries)) = (past, serde_json::from_value(value)) {
                    self.king_ui.viewing = Some((past, entries));
                    self.king_ui.scroll.scroll_to_bottom();
                }
                cx.notify();
            }
            (Asked::Resumed, Ok(_)) => {
                self.king_ui.viewing = None;
                self.king_ask(Request::Conversations, Asked::Conversations, cx);
                cx.notify();
            }
            (Asked::Run, Ok(value)) => {
                let steps = value["steps"].as_array().map_or(0, Vec::len);
                self.engine.report(
                    StatusKind::Info,
                    format!(
                        "Run #{} started: {steps} steps, each once what it needs is done",
                        value["run"]
                    ),
                );
                cx.notify();
            }
            (Asked::Terminal, Ok(value)) => {
                if let Ok(ask) = serde_json::from_value::<TerminalAsk>(value) {
                    self.king_open_terminal(ask, window, cx);
                }
            }
            // Another window took it first.
            (Asked::Terminal, Err(error)) => {
                tracing::debug!(%error, "a terminal King asked for is not this window's");
            }
            (Asked::Models, Ok(value)) => {
                self.king_ui.models = models_of(&value);
                self.king_ui.modes = modes_of(&value);
                cx.notify();
            }
            (_, Err(error)) => {
                self.engine
                    .report(StatusKind::Error, format!("King's service: {error}"));
                cx.notify();
            }
            (Asked::Done, Ok(_)) => {}
        }
    }

    /// The service's new status: kept, and what it says the window lacks is
    /// asked for.
    fn king_status(&mut self, status: Status, cx: &mut Context<Self>) {
        // Another conversation now (a new one, or a past one taken up): what
        // the window had of King's is not of it.
        let before = self.king_ui.status.as_ref().map(|now| now.conversation);
        if before.is_some_and(|before| before != status.conversation) {
            self.king_ui.brain = Feed::default();
            self.king_ui
                .open_tools
                .retain(|(target, _)| *target != Target::Brain);
            self.king_ui.scroll.scroll_to_bottom();
            if self.king_ui.history.is_some() {
                self.king_ask(Request::Conversations, Asked::Conversations, cx);
            }
        }
        // A card whose worker is gone (stopped and forgotten, or taken over)
        // closes.
        if let Some(open) = self.king_ui.open {
            if !status.workers.iter().any(|worker| worker.id == open) {
                self.king_ui.open = None;
            }
        }
        self.king_fetch(&status, cx);
        // The terminals King asked for are this window's to open, once the
        // service gives them to it.
        self.king_ui
            .taking
            .retain(|id| status.terminals.iter().any(|ask| ask.id == *id));
        for ask in &status.terminals {
            if self.king_ui.taking.insert(ask.id) {
                self.king_ask(
                    Request::TerminalTaken { terminal: ask.id },
                    Asked::Terminal,
                    cx,
                );
            }
        }
        self.king_ui.status = Some(status);
        self.king_spin(cx);
        cx.notify();
    }

    /// Opens a terminal King asked for: a session of its own, the command
    /// typed into its shell as a project's script is.
    fn king_open_terminal(
        &mut self,
        ask: TerminalAsk,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let machine = ask
            .machine
            .clone()
            .map(MachineId::from_string)
            .unwrap_or_else(MachineId::local);
        let agent = ask.agent.as_deref().and_then(leon_core::AgentId::parse);
        let Some(kind) = agent else {
            self.engine.report(
                StatusKind::Info,
                format!("King opened a terminal: {}", ask.title),
            );
            self.start_live_first(
                &ask.command,
                crate::launch::First::Script,
                Some(ask.title),
                Launch::Shell,
                &machine,
                &ask.cwd,
                Place::Session,
                window,
                cx,
            );
            return;
        };
        // A worker of an agent that only runs in a terminal: the agent is
        // started on its prompt when it takes one on its command line, else
        // the prompt is shown above it, for the person to give it.
        self.engine.report(
            StatusKind::Info,
            format!(
                "King started {} in a terminal: {}",
                kind.as_str(),
                ask.title
            ),
        );
        let account = super::terminals::default_account(kind, cx);
        if kind.spec().is_some_and(|spec| spec.prompt.is_some()) {
            let launch = Launch::Prompted {
                kind,
                prompt: ask.command,
                account,
            };
            self.start_live(launch, &machine, &ask.cwd, Place::Session, None, window, cx);
            return;
        }
        let shown = format!(
            "printf '%s\\n\\n' {}",
            leon_remote::sh_quote_typed(&format!("King's task:\n\n{}", ask.command))
        );
        self.start_live_first(
            &shown,
            crate::launch::First::Script,
            Some(ask.title),
            Launch::Agent {
                kind,
                resume: None,
                account,
            },
            &machine,
            &ask.cwd,
            Place::Session,
            window,
            cx,
        );
    }

    /// Keeps the spinner turning while King or a worker is in a turn: the
    /// view is drawn again every frame, and the loop ends with the turns.
    fn king_spin(&mut self, cx: &mut Context<Self>) {
        let busy = |status: &Status| {
            status.brain.busy
                || status.workers.iter().any(|worker| {
                    worker.conversation.busy
                        || worker.check.as_ref().is_some_and(|check| {
                            check.state == leon_king::verify::CheckState::Running
                        })
                })
        };
        if !self.king_ui.status.as_ref().is_some_and(busy) {
            self.king_ui.spinning = None;
            return;
        }
        if self.king_ui.spinning.is_some() {
            return;
        }
        self.king_ui.spinning = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(super::king_view::SPIN).await;
            let going = this.update(cx, |this, cx| {
                let going = this.king_open() && this.king_ui.status.as_ref().is_some_and(busy);
                if going {
                    cx.notify();
                } else {
                    this.king_ui.spinning = None;
                }
                going
            });
            if !going.unwrap_or(false) {
                return;
            }
        }));
    }

    /// Asks for the entries the window does not have.
    fn king_fetch(&mut self, status: &Status, cx: &mut Context<Self>) {
        let open = self
            .king_ui
            .open
            .and_then(|id| self.king_ui.workers.get(&id).map(|feed| (id, feed)));
        for target in wanted(status, &self.king_ui.brain, open) {
            let since = self.king_ui.feed(target).and_then(Feed::last);
            match target {
                Target::Brain => self.king_ui.brain.asking = true,
                Target::Worker(id) => {
                    if let Some(feed) = self.king_ui.workers.get_mut(&id) {
                        feed.asking = true;
                    }
                }
            }
            self.king_ask(
                Request::Transcript {
                    target,
                    since,
                    limit: Some(KEPT),
                },
                Asked::Transcript(target),
                cx,
            );
        }
    }

    /// What the person wrote to King is said to it.
    pub(super) fn king_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.king_ui.input.clone() else {
            return;
        };
        let typed = input.read(cx).value().trim().to_owned();
        // Dictation is Leon's own: its models are listed here, not asked of
        // King.
        if typed.eq_ignore_ascii_case("/voice") {
            input.update(cx, |input, cx| input.set_value("", window, cx));
            self.dictation.installed = self.options.voice.installed();
            self.king_ui.voices = true;
            cx.notify();
            return;
        }
        let files = self
            .king_ui
            .attached
            .get(&Target::Brain)
            .cloned()
            .unwrap_or_default();
        let Some(text) = with_attached(&typed, &files) else {
            return;
        };
        if !self.king_ui.connected() {
            self.king_connect(window, cx);
            self.engine.report(
                StatusKind::Error,
                "King's service is not connected yet; what you wrote is still there.",
            );
            cx.notify();
            return;
        }
        input.update(cx, |input, cx| input.set_value("", window, cx));
        self.king_ui.attached.remove(&Target::Brain);
        self.king_ask(Request::Say { text }, Asked::Done, cx);
        self.king_ui.note = None;
        self.king_ui.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Says `text` to King for the person, from elsewhere in the window (a
    /// task given to King): now when the line is open, else as soon as it
    /// is. King's view opens on it.
    pub(super) fn king_say(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.king_open() {
            self.toggle_king(window, cx);
        }
        if self.king_ui.connected() {
            self.king_ask(Request::Say { text }, Asked::Done, cx);
        } else {
            self.king_ui.pending.push(Request::Say { text });
            self.king_connect(window, cx);
        }
        self.king_ui.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Carries out the card the board's cursor is on as its steps: a run,
    /// which King's service starts without King ([`leon_king::run`]).
    pub(super) fn tasks_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.tasks_ui.board.selected().cloned() else {
            return;
        };
        let request = Request::RunStart {
            task: task.id.clone(),
            max_parallel: None,
            budget_usd: None,
        };
        if self.king_ui.connected() {
            self.king_ask(request, Asked::Run, cx);
        } else {
            self.king_ui.pending.push(request);
            self.king_connect(window, cx);
        }
        cx.notify();
    }

    /// Gives the card the board's cursor is on to King: King is told what
    /// it is and where, and works it (its instructions say how).
    pub(super) fn tasks_send_to_king(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.tasks_ui.board.selected().cloned() else {
            return;
        };
        let place = match &task.scope {
            leon_core::MemoryScope::Project(root) => {
                let name = self
                    .local_project(root)
                    .and_then(|(id, _)| self.snapshot.project(&id))
                    .map(|entry| entry.project.name.clone());
                match name {
                    Some(name) => format!("of {name} (board of {root})"),
                    None => format!("of the project at {root}"),
                }
            }
            leon_core::MemoryScope::Global => "of the global board".to_owned(),
        };
        self.king_say(task_for_king(&task, &place), window, cx);
    }

    /// The field of `target`: King's, or the open worker's.
    fn king_field(&self, target: Target) -> Option<Entity<TextareaState>> {
        match target {
            Target::Brain => self.king_ui.input.clone(),
            Target::Worker(_) => self.king_ui.worker_input.clone(),
        }
    }

    /// Gives the keyboard back to the field of `target`.
    pub(super) fn king_focus_field(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.king_field(target) {
            field.update(cx, |field, cx| field.focus(window, cx));
        }
    }

    /// The field of King's view that has the keyboard, as its target.
    fn king_typing_in(&self, window: &Window, cx: &gpui_kit::App) -> Option<Target> {
        use gpui_kit::Focusable as _;
        let focused = |field: &Option<Entity<TextareaState>>| {
            field
                .as_ref()
                .is_some_and(|field| field.focus_handle(cx).is_focused(window))
        };
        if focused(&self.king_ui.input) {
            Some(Target::Brain)
        } else if focused(&self.king_ui.worker_input) {
            self.king_ui.open.map(Target::Worker)
        } else {
            None
        }
    }

    /// The field of King's view dictation goes into: the one with the
    /// keyboard, else King's.
    pub(super) fn king_dictation_target(&self, window: &Window, cx: &gpui_kit::App) -> Target {
        self.king_typing_in(window, cx).unwrap_or(Target::Brain)
    }

    /// Writes `text` where the cursor is in a field of King's view, apart
    /// from what is around it: what was dictated, the path of a pasted image
    /// or of an attached file.
    pub(super) fn king_write(
        &mut self,
        target: Target,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.king_field(target) else {
            return;
        };
        field.update(cx, |input, cx| {
            let (value, cursor) =
                crate::chat::complete::dictated(&input.value(), input.cursor(), text);
            input.set_value(value, window, cx);
            input.set_selected_range(cursor..cursor, cx);
            input.focus(window, cx);
        });
    }

    /// Pastes the clipboard's image into the King field that has the
    /// keyboard: it is kept in a file and attached to what is said next,
    /// which names its path for King (and any worker it is passed to) to
    /// open. `false` when there is no
    /// image, so the field pastes text as it does.
    pub(super) fn king_paste_image(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(target) = self.king_typing_in(window, cx) else {
            return false;
        };
        let Some(image) = (self.options.clipboard_image)(cx) else {
            return false;
        };
        let stamp = (self.options.now)().format("%Y%m%d-%H%M%S%3f");
        let path = self
            .options
            .attachments
            .join(format!("pasted-{stamp}.{}", image.extension));
        let kept = std::fs::create_dir_all(&self.options.attachments)
            .and_then(|()| std::fs::write(&path, &image.bytes));
        match kept {
            Ok(()) => self.king_ui.attached.entry(target).or_default().push(path),
            Err(error) => self.engine.report(
                StatusKind::Error,
                format!("The image could not be kept for King: {error}."),
            ),
        }
        cx.notify();
        true
    }

    /// Asks for files to attach to what is said to `target`: they are shown
    /// over its field and sent with what is written.
    pub(super) fn king_attach(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picked = (self.options.pick_files)(cx);
        cx.spawn_in(window, async move |this, cx| {
            let paths = picked.await;
            if paths.is_empty() {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                let attached = this.king_ui.attached.entry(target).or_default();
                for path in paths {
                    if !attached.contains(&path) {
                        attached.push(path);
                    }
                }
                this.king_focus_field(target, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What the person wrote to the open worker is told to it.
    pub(super) fn king_tell_worker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(worker), Some(input)) = (self.king_ui.open, self.king_ui.worker_input.clone())
        else {
            return;
        };
        let typed = input.read(cx).value().trim().to_owned();
        let target = Target::Worker(worker);
        let files = self
            .king_ui
            .attached
            .get(&target)
            .cloned()
            .unwrap_or_default();
        let Some(text) = with_attached(&typed, &files) else {
            return;
        };
        if !self.king_ui.connected() {
            return;
        }
        input.update(cx, |input, cx| input.set_value("", window, cx));
        self.king_ui.attached.remove(&target);
        self.king_ask(Request::TellWorker { worker, text }, Asked::Done, cx);
        self.king_ui.worker_scroll.scroll_to_bottom();
        cx.notify();
    }

    /// A key of King's view that is its own: Escape stops King's turn, else
    /// closes the open card, else closes the view. `true` when the key was
    /// taken.
    pub(super) fn king_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Alt with the arrows goes through the workers' cards, top to
        // bottom as they are drawn (newest first).
        let only_alt = stroke.modifiers.alt
            && !stroke.modifiers.control
            && !stroke.modifiers.platform
            && !stroke.modifiers.shift;
        // While the microphone is open for a field of King's, Enter ends the
        // dictation and Escape drops it.
        if matches!(
            self.dictating_into(),
            Some(super::chat_voice::Field::King(_))
        ) && !stroke.modifiers.modified()
        {
            match stroke.key.as_str() {
                "enter" => {
                    self.dictation_finish(window, cx);
                    return true;
                }
                "escape" => {
                    self.dictation_cancel(cx);
                    return true;
                }
                _ => {}
            }
        }
        // A paste with an image on the clipboard: the image, as a file.
        let paste = stroke.key == "v"
            && !stroke.modifiers.alt
            && if crate::platform::is_mac() {
                stroke.modifiers.platform && !stroke.modifiers.control
            } else {
                stroke.modifiers.control && !stroke.modifiers.platform
            };
        if paste && self.king_paste_image(window, cx) {
            return true;
        }
        // Alt with a number picks that answer of the question in view.
        if let Some(place) = only_alt
            .then(|| stroke.key.parse::<usize>().ok())
            .flatten()
            .filter(|n| (1..=9).contains(n))
        {
            let Some((target, open)) = self.king_question_in_view() else {
                return false;
            };
            let question = open.id.clone();
            match picks(open).into_iter().nth(place - 1).map(|pick| pick.pick) {
                Some(Pick::Reply(reply)) => self.king_answer(target, question, reply, cx),
                Some(Pick::Words) => self.king_answer_with_words(target, question, window, cx),
                None => return false,
            }
            return true;
        }
        if only_alt && matches!(stroke.key.as_str(), "down" | "up") {
            let ids: Vec<u32> = self
                .king_ui
                .status
                .as_ref()
                .map(|status| {
                    status
                        .workers
                        .iter()
                        .rev()
                        .map(|worker| worker.id)
                        .collect()
                })
                .unwrap_or_default();
            if let Some(id) = step_card(&ids, self.king_ui.open, stroke.key == "down") {
                self.king_open_worker(id, window, cx);
            } else {
                self.king_close_worker(window, cx);
            }
            return true;
        }
        if stroke.key != "escape" || stroke.modifiers.modified() {
            return false;
        }
        let busy = |target: Target| {
            self.king_ui
                .status
                .as_ref()
                .is_some_and(|status| match target {
                    Target::Brain => status.brain.busy,
                    Target::Worker(id) => status
                        .workers
                        .iter()
                        .any(|worker| worker.id == id && worker.conversation.busy),
                })
        };
        if self.king_ui.viewing.is_some() && self.king_ui.open.is_none() {
            self.king_back_to_now(cx);
            return true;
        }
        match self.king_ui.open {
            Some(id) if busy(Target::Worker(id)) => self.king_interrupt(Target::Worker(id), cx),
            Some(_) => self.king_close_worker(window, cx),
            None if busy(Target::Brain) => self.king_interrupt(Target::Brain, cx),
            None => self.close_king(window, cx),
        }
        true
    }

    /// Puts King's conversation away and starts a new one.
    pub(super) fn king_new_conversation(&mut self, cx: &mut Context<Self>) {
        self.king_ui.viewing = None;
        self.king_ask(Request::NewConversation, Asked::Done, cx);
        cx.notify();
    }

    /// Shows King's past conversations in the column of the workers, or
    /// the workers again.
    pub(super) fn king_toggle_history(&mut self, show: bool, cx: &mut Context<Self>) {
        if show {
            self.king_ui.open = None;
            self.king_ui.history.get_or_insert_with(Vec::new);
            self.king_ask(Request::Conversations, Asked::Conversations, cx);
        } else {
            // A past conversation stays on screen: its workers are shown.
            self.king_ui.history = None;
        }
        cx.notify();
    }

    /// Shows what a past conversation said, in place of the one now.
    pub(super) fn king_view_past(&mut self, id: u64, cx: &mut Context<Self>) {
        self.king_ask(
            Request::PastConversation { conversation: id },
            Asked::Past(id),
            cx,
        );
    }

    /// Back to the conversation now.
    pub(super) fn king_back_to_now(&mut self, cx: &mut Context<Self>) {
        self.king_ui.viewing = None;
        self.king_ui.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Takes the past conversation on screen up again.
    pub(super) fn king_resume(&mut self, cx: &mut Context<Self>) {
        let Some((past, _)) = &self.king_ui.viewing else {
            return;
        };
        let id = past.id;
        self.king_ask(
            Request::ResumeConversation { conversation: id },
            Asked::Resumed,
            cx,
        );
    }

    /// Opens a worker's card: its conversation, live.
    pub(super) fn king_open_worker(
        &mut self,
        id: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.king_ui.open = Some(id);
        self.king_ui.workers.entry(id).or_default();
        self.king_ui.worker_scroll.scroll_to_bottom();
        if let Some(status) = self.king_ui.status.clone() {
            self.king_fetch(&status, cx);
        }
        if let Some(field) = self.king_ui.worker_input.clone() {
            field.update(cx, |field, cx| field.focus(window, cx));
        }
        cx.notify();
    }

    /// Closes the open card: back to the cards.
    pub(super) fn king_close_worker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.king_ui.open = None;
        if let Some(field) = self.king_ui.input.clone() {
            field.update(cx, |field, cx| field.focus(window, cx));
        }
        cx.notify();
    }

    /// Stops the turn of King or of a worker; the conversation stays.
    pub(super) fn king_interrupt(&mut self, target: Target, cx: &mut Context<Self>) {
        self.king_ask(Request::Interrupt { target }, Asked::Done, cx);
    }

    /// Ends a process a turn left running.
    pub(super) fn king_stop_process(&mut self, pid: u32, cx: &mut Context<Self>) {
        self.king_ask(Request::StopProcess { pid }, Asked::Done, cx);
    }

    /// Stops a worker.
    pub(super) fn king_stop_worker(&mut self, worker: u32, cx: &mut Context<Self>) {
        self.king_ask(Request::StopWorker { worker }, Asked::Done, cx);
    }

    /// The person takes a worker over: the service lets it go, and its
    /// reply opens it as a session of the window's ([`Self::king_extract`]).
    pub(super) fn king_take_over(&mut self, worker: u32, cx: &mut Context<Self>) {
        self.king_ask(Request::Release { worker }, Asked::Release(worker), cx);
    }

    /// Answers a question King or a worker waits on.
    pub(super) fn king_answer(
        &mut self,
        target: Target,
        question: String,
        reply: Answer,
        cx: &mut Context<Self>,
    ) {
        self.king_ask(
            Request::Answer {
                target,
                question,
                reply,
            },
            Asked::Done,
            cx,
        );
    }

    /// Answers a question with the words in the field of its conversation.
    pub(super) fn king_answer_with_words(
        &mut self,
        target: Target,
        question: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field = match target {
            Target::Brain => self.king_ui.input.clone(),
            Target::Worker(_) => self.king_ui.worker_input.clone(),
        };
        let Some(field) = field else {
            return;
        };
        let text = field.read(cx).value().trim().to_owned();
        if text.is_empty() {
            self.engine
                .report(StatusKind::Info, "Write the answer in the field first.");
            cx.notify();
            return;
        }
        field.update(cx, |field, cx| field.set_value("", window, cx));
        self.king_answer(
            target,
            question,
            Answer {
                text: Some(text),
                ..Answer::default()
            },
            cx,
        );
    }

    /// A worker the service let go opens as a session of this window, in
    /// its folder, continuing its agent's conversation.
    fn king_extract(&mut self, released: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let agent = released["agent"].as_str().and_then(AgentId::parse);
        let cwd = released["cwd"].as_str().unwrap_or_default().to_owned();
        let (Some(kind), false) = (agent, cwd.is_empty()) else {
            self.engine.report(
                StatusKind::Error,
                "King let the worker go, but did not say where it works.",
            );
            cx.notify();
            return;
        };
        let resume = released["session_id"].as_str().map(str::to_owned);
        self.king_ui.open = None;
        let launch = Launch::Agent {
            kind,
            resume,
            account: None,
        };
        // The session takes the main pane: King's view closes behind it.
        self.king_ui.back = None;
        self.main = Main::Empty;
        self.home.nav = None;
        if !self.start_live(
            launch,
            &MachineId::local(),
            &cwd,
            Place::Session,
            None,
            window,
            cx,
        ) {
            // Nothing opened: the status line says why, and King's view is
            // back.
            self.main = Main::King;
            self.home.nav = Some(Nav::King);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use leon_king::protocol::{ConversationStatus, WorkerStatus};
    use leon_king::transcript::Kind;

    use super::*;

    fn entry(seq: u64) -> Entry {
        Entry {
            seq,
            at_ms: seq,
            kind: Kind::Agent,
            text: format!("entry {seq}"),
            report: None,
        }
    }

    fn conversation(last_seq: Option<u64>) -> ConversationStatus {
        ConversationStatus {
            agent: "claude".into(),
            model: None,
            effort: None,
            mode: None,
            session_id: None,
            busy: false,
            busy_since_ms: None,
            queued: 0,
            questions: Vec::new(),
            cost_usd: 0.0,
            last_seq,
        }
    }

    fn status(brain: Option<u64>, workers: &[(u32, Option<u64>)]) -> Status {
        Status {
            conversation: 0,
            brain: conversation(brain),
            workers: workers
                .iter()
                .map(|(id, last)| WorkerStatus {
                    id: *id,
                    title: format!("worker {id}"),
                    project: None,
                    cwd: "/tmp".into(),
                    machine: None,
                    terminal: false,
                    job: Default::default(),
                    tier: None,
                    state: leon_king::coordinator::WorkerState::Working,
                    conversation: conversation(*last),
                    last_answer: None,
                    updated_ms: 0,
                    started_in: None,
                    check: None,
                    pull_request: None,
                })
                .collect(),
            news: 0,
            spent_today_usd: 0.0,
            daily_budget_usd: None,
            running: Vec::new(),
            terminals: Vec::new(),
            runs: Vec::new(),
        }
    }

    #[test]
    fn a_question_is_answered_by_its_own_answers_then_by_words() {
        let permission = Open {
            id: "q1".into(),
            text: "May it run ls?".into(),
            form: Form::Permission {
                choices: vec![
                    ("a".into(), "allow".into()),
                    ("b".into(), "always".into()),
                    ("c".into(), "deny".into()),
                ],
            },
        };
        let labels: Vec<&str> = picks(&permission).iter().map(|p| p.label).collect();
        assert_eq!(
            labels,
            [
                "ALLOW",
                "ALWAYS ALLOW",
                "DENY",
                "ANSWER WITH WHAT IS WRITTEN"
            ]
        );
        assert_eq!(
            picks(&permission)[2].pick,
            Pick::Reply(Answer {
                choice: Some("c".into()),
                ..Answer::default()
            })
        );
        let plan = Open {
            form: Form::Plan,
            ..permission.clone()
        };
        assert_eq!(
            picks(&plan)[0].pick,
            Pick::Reply(Answer {
                approve: Some(true),
                ..Answer::default()
            })
        );
        let options = Open {
            form: Form::Choices {
                questions: vec!["x".into()],
            },
            ..permission
        };
        assert_eq!(picks(&options).len(), 1);
        assert_eq!(picks(&options)[0].pick, Pick::Words);
    }

    #[test]
    fn the_models_are_read_from_the_services_answer() {
        let value = serde_json::json!({"models": [
            {"agent": "claude", "id": "opus", "label": "Opus", "efforts": ["low", "high"]},
            {"agent": "codex", "id": "gpt-5.5", "label": null, "efforts": []},
            {"id": "no agent"},
        ], "record": [
            {"agent": "claude", "model": "opus", "job": "plan",
             "line": "3 of 3 judged did well (3 workers)"},
            {"agent": "claude", "model": "haiku", "job": "code", "line": "not this one"},
        ], "modes": [
            {"agent": "claude", "id": "bypassPermissions", "label": "Bypass",
             "description": null, "dangerous": true},
        ]});
        assert_eq!(
            modes_of(&value),
            [KingMode {
                agent: "claude".into(),
                id: "bypassPermissions".into(),
                label: "Bypass".into(),
                description: String::new(),
                dangerous: true,
            }]
        );
        assert_eq!(
            models_of(&value),
            [
                KingModel {
                    agent: "claude".into(),
                    id: "opus".into(),
                    label: "Opus".into(),
                    efforts: vec!["low".into(), "high".into()],
                    record: vec!["plan: 3 of 3 judged did well (3 workers)".into()],
                },
                KingModel {
                    agent: "codex".into(),
                    id: "gpt-5.5".into(),
                    label: "gpt-5.5".into(),
                    efforts: Vec::new(),
                    record: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn alt_and_the_arrows_go_through_the_cards() {
        let ids = [7, 5, 2];
        assert_eq!(step_card(&ids, None, true), Some(7));
        assert_eq!(step_card(&ids, Some(7), true), Some(5));
        assert_eq!(
            step_card(&ids, Some(2), true),
            Some(2),
            "the last one stays"
        );
        assert_eq!(step_card(&ids, Some(5), false), Some(7));
        assert_eq!(
            step_card(&ids, Some(7), false),
            None,
            "above the first: none"
        );
        assert_eq!(step_card(&[], None, true), None);
        // A card whose worker is gone starts again from the top.
        assert_eq!(step_card(&ids, Some(9), true), Some(7));
    }

    #[test]
    fn a_feed_keeps_only_what_comes_after_what_it_has() {
        let mut feed = Feed::default();
        feed.add(vec![entry(0), entry(1)]);
        // An answer that crossed another brings some of it again.
        feed.add(vec![entry(1), entry(2)]);
        let seqs: Vec<u64> = feed.entries.iter().map(|entry| entry.seq).collect();
        assert_eq!(seqs, [0, 1, 2]);
        assert_eq!(feed.last(), Some(2));
        // No more than the service keeps.
        feed.add((3..(KEPT as u64 + 10)).map(entry).collect());
        assert_eq!(feed.entries.len(), KEPT);
        assert_eq!(feed.last(), Some(KEPT as u64 + 9));
    }

    #[test]
    fn only_what_the_window_lacks_and_is_not_asking_for_is_asked() {
        let mut brain = Feed::default();
        // Nothing said yet: nothing to ask.
        assert!(wanted(&status(None, &[]), &brain, None).is_empty());
        // King said something the window does not have.
        assert_eq!(wanted(&status(Some(3), &[]), &brain, None), [Target::Brain]);
        // Being asked for already: not twice.
        brain.asking = true;
        assert!(wanted(&status(Some(3), &[]), &brain, None).is_empty());
        brain.asking = false;
        brain.add(vec![entry(3)]);
        assert!(wanted(&status(Some(3), &[]), &brain, None).is_empty());
        // A worker's conversation only while its card is open.
        let worker = Feed::default();
        let now = status(Some(3), &[(2, Some(5)), (4, Some(1))]);
        assert!(wanted(&now, &brain, None).is_empty());
        assert_eq!(
            wanted(&now, &brain, Some((2, &worker))),
            [Target::Worker(2)]
        );
        // A worker that is gone is not asked about.
        assert!(wanted(&now, &brain, Some((9, &worker))).is_empty());
    }
}

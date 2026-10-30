//! Conversations in Leon's own interface: what the shell does with them.
//!
//! A conversation is a leaf of a tab, like a terminal, a file or the changes
//! of a worktree. `Layout`, `Tab` and `Workspaces` hold its `LiveId`; what
//! tells it apart is the side table `Shell.chats`. Unlike a file it is a
//! session: it has a row in the tree, with the light of what its agent is
//! doing.
//!
//! The agent is not a program in a terminal here. Each prompt is one turn of
//! Leon's unified agent protocol (`crate::chat`): a process on a pipe, read
//! into a [`Conversation`] that is the same data for every agent, and drawn
//! by `chat_view.rs`. The next prompt resumes the agent's own conversation by
//! the id it gave it, so the history the agent keeps is one session.
//!
//! In beta, behind the setting `agent_interface`: sessions on any machine,
//! for the agents the protocol knows. Everything else starts in a terminal as
//! before.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, ScrollHandle, Subscription, Task, Window};
use leon_core::{AgentId, Machine, MachineId, MachineKind, Role, SessionId};
use leon_turn::{
    Action, AgentCommand, AgentOptions, Answer, ChoiceAnswer, QuestionKind, TurnEvent, TurnSpec,
};

use super::activity::Activity;
use super::chat_diff::DiffPanel;
use super::chat_git::GitCell;
use super::chat_menu::Menu;
use super::chat_rewind::Rewinding;
use super::live::{LiveId, LiveState};
use super::shell::{Main, Pane, Shell};
use super::terminals::Place;
use super::tree;
use crate::chat::checkpoint;
use crate::chat::model::{Conversation, Phase};
use crate::chat::rewind::{Mark, Memory};
use crate::chat::turn::{Output, Start, Turn};
use crate::engine::{MachineState, StatusKind};
use crate::launch::{self, Launch};
use crate::settings;

/// What a conversation is titled from: its first prompt and the first answer,
/// and where its agent runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TitleAsk {
    /// The machine the conversation runs on.
    pub machine: MachineId,
    /// The folder it runs in.
    pub path: String,
    /// The agent that answered.
    pub agent: AgentId,
    /// What the person first said, as words.
    pub prompt: String,
    /// What the agent first answered.
    pub reply: String,
}

/// How a title is made: the answer comes later, and a failure is a message
/// nobody is shown.
pub type Titler = std::rc::Rc<
    dyn Fn(&crate::engine::Engine, TitleAsk, &mut gpui_kit::App) -> Task<Result<String, String>>,
>;

/// The real [`Titler`]: the agent is asked, headless and cheaply, on the
/// machine the conversation runs on.
pub fn title_by_agent(
    engine: &crate::engine::Engine,
    ask: TitleAsk,
    cx: &mut gpui_kit::App,
) -> Task<Result<String, String>> {
    let asked = engine.suggest_title(ask.machine, ask.path, ask.agent, ask.prompt, ask.reply);
    cx.spawn(async move |_| {
        asked
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())
    })
}

/// How long the checkpoints of a chat that was never closed are kept, in
/// seconds: a crash leaves them behind.
const CHECKPOINTS_KEPT_SECS: i64 = 30 * 24 * 60 * 60;
/// How many checkpoints a chat keeps: the newest.
const CHECKPOINTS_PER_CHAT: usize = 200;

/// What the agent a conversation went on with is told of what came before,
/// and whether it has been yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    /// The block ([`crate::chat::handoff::text`]).
    pub text: String,
    /// The turn it went with. Going back to before that turn takes it away
    /// with the turn: it goes with the next message again.
    pub sent_in: Option<u32>,
}

/// One conversation with an agent.
pub struct Chat {
    /// The machine it runs on.
    pub machine: MachineId,
    /// The folder it runs in.
    pub cwd: String,
    /// The agent.
    pub agent: AgentId,
    /// The id of the account the agent runs as; `None` is its own setup.
    pub account: Option<String>,
    /// The history session it continues, as the store knows it.
    pub history: Option<SessionId>,
    /// What was said and done.
    pub conversation: Conversation,
    /// Where the person writes.
    pub input: Entity<TextareaState>,
    /// Where the person says what to do instead, with a refusal.
    pub reason: Entity<InputState>,
    /// The scroll position of the conversation.
    pub scroll: ScrollHandle,
    /// Files sent with what is said next: images pasted, as files.
    pub attached: Vec<std::path::PathBuf>,
    /// The stretches of work opened by hand, by the index of their first item.
    pub open_work: HashSet<usize>,
    /// The tools whose detail is shown, by id.
    pub open_tools: HashSet<String>,
    /// What is picked of a question with options and not sent yet: by the
    /// question's id and the id of one of its questions.
    pub picks: HashMap<(String, String), Vec<String>>,
    /// The permission mode the next turn starts in; `None` is the agent's own.
    pub mode: Option<String>,
    /// The model the next turn starts with; `None` is the agent's own.
    pub model: Option<String>,
    /// How hard the model thinks in the next turn, one of the efforts of
    /// the model in use; `None` is as the agent is set up.
    pub effort: Option<String>,
    /// What the agent offers on this machine, once it was asked.
    pub options: Option<AgentOptions>,
    /// What a prompt can ask the agent for by name: what it listed when it
    /// was asked what it offers, or since then in a turn.
    pub commands: Vec<AgentCommand>,
    /// The files of the folder it works in, once they were listed.
    pub files: Option<Arc<Vec<String>>>,
    /// The list open over the field.
    pub menu: Option<Menu>,
    /// Where the list of modes or models is narrowed.
    pub filter: Entity<InputState>,
    /// Whether the person has looked at it since its agent last stopped.
    pub seen: bool,
    /// The name the person gave it: it is what the tab and the tree say,
    /// over anything generated.
    pub name: Option<String>,
    /// The title generated for it after its first turn.
    pub title: Option<String>,
    /// A title was asked for: once per conversation, whatever came of it.
    pub titled: bool,
    /// Names its checkpoints in the repository of its folder: unlike its id
    /// in the window, no other run of Leon makes the same.
    pub key: String,
    /// Where each of its turns began, for going back to before one.
    pub marks: Vec<Mark>,
    /// The number the next turn's checkpoint is made under.
    pub next_turn: u32,
    /// The turn is being made ready: its checkpoint is being taken, and the
    /// agent is not started yet.
    pub launching: bool,
    /// Going back to before one of its messages, asked or under way.
    pub rewinding: Option<Rewinding>,
    /// The bar that finds words in it, when open.
    pub find: Option<super::chat_find::Find>,
    /// Where the bar's words are written.
    pub find_input: Entity<InputState>,
    /// The panel of diffs beside it, when open.
    pub diff: Option<DiffPanel>,
    /// Where the person is in the prompts they sent before, while browsing
    /// them with the arrows in the field.
    pub recall: Option<super::chat_recall::Recall>,
    /// Counts the readings of the panel, so an answer that is no longer the
    /// latest is dropped.
    pub diff_reads: u64,
    /// What each turn changed, as git counted it between the snapshots of
    /// the worktree, by the turn's number. A turn without an entry is
    /// counted from what its tools said.
    pub measured: HashMap<u32, Vec<crate::chat::changed::Stat>>,
    /// Its branch and the steps that take its changes away.
    pub git: GitCell,
    /// The other agents it can go on with (installed on its machine, and
    /// driven by the protocol), as last looked up: asking the machine takes
    /// a look for each program, so it is when the list is about to be
    /// wanted.
    pub switchable: Vec<AgentId>,
    /// What the agent it went on with is told before the next message, after
    /// a switch of agents.
    pub handoff: Option<Handoff>,
    /// The turn in flight.
    turn: Option<Turn>,
    /// Reads what the turn in flight prints.
    reader: Option<Task<()>>,
    /// When the turn in flight started, in milliseconds since the epoch.
    pub since: Option<u64>,
    /// Draws the conversation again while its turn goes on, for its spinner.
    spin: Option<Task<()>>,
    /// Asks the agent what it offers.
    probe: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Chat {
    /// What the tree and the tab call it: the name the person gave it, else
    /// the title generated for it, else the first thing the person said,
    /// else the agent's name.
    pub fn label(&self) -> String {
        if let Some(name) = self.name.as_ref().or(self.title.as_ref()) {
            return name.clone();
        }
        self.conversation
            .items
            .iter()
            .find_map(|item| match item {
                crate::chat::model::Item::User { text, .. } => text
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(|line| crate::format::truncate_chars(line, 60).0.to_owned()),
                _ => None,
            })
            .unwrap_or_else(|| crate::format::agent_name(self.agent).to_owned())
    }

    /// The state the tree shows beside it. A conversation whose agent waits
    /// for the person is running: there is nothing to say of it.
    pub fn state(&self) -> LiveState {
        match self.conversation.phase() {
            Phase::Starting => LiveState::Starting,
            Phase::Idle | Phase::Working | Phase::Asking => LiveState::Running,
        }
    }

    /// How it is doing, as the dots of its worktree and project read it.
    pub fn activity(&self) -> Activity {
        match self.conversation.phase() {
            Phase::Starting | Phase::Working => Activity::Working,
            Phase::Asking => Activity::NeedsYou,
            Phase::Idle if self.conversation.failed && !self.seen => Activity::Failed,
            Phase::Idle if !self.seen => Activity::TurnOver,
            Phase::Idle => Activity::Idle,
        }
    }

    /// Whether one of its fields has the keyboard.
    pub fn typing(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        use gpui_kit::Focusable as _;
        self.input.focus_handle(cx).is_focused(window)
            || self.reason.focus_handle(cx).is_focused(window)
            || self.filter.focus_handle(cx).is_focused(window)
            || self.find_input.focus_handle(cx).is_focused(window)
            || self.git.typing(window, cx)
    }

    /// Whether a turn's process is still there.
    pub(super) fn turn_is_open(&self) -> bool {
        self.turn.is_some()
    }

    /// Stops asking the agent what it offers: the answer would be the
    /// previous agent's.
    pub(super) fn probe_stop(&mut self) {
        self.probe = None;
    }

    /// The turn in flight.
    pub(super) fn turn_mut(&mut self) -> Option<&mut Turn> {
        self.turn.as_mut()
    }

    /// Whether the conversation is scrolled to its end, give or take a line.
    fn at_end(&self) -> bool {
        super::widgets::at_end(&self.scroll)
    }
}

impl Shell {
    /// Whether `launch` on `machine` is a conversation rather than a
    /// terminal: the setting asks for it, the protocol knows the agent, and
    /// the machine is known. Which machine it is makes no difference: the
    /// turn's process is started there (`chat::reach`).
    pub(super) fn wants_chat(
        &self,
        launch: &Launch,
        machine: &MachineId,
        cx: &gpui_kit::App,
    ) -> bool {
        let Launch::Agent { kind, .. } = launch else {
            return false;
        };
        crate::schema::Platform::Unix.here()
            && settings::text(cx, "agent_interface") == crate::chat::UNIFIED
            && crate::chat::adapter_of(*kind).is_some()
            && self.snapshot.machine(machine).is_some()
    }

    /// The conversation on screen, which has the keyboard when the main pane
    /// does.
    pub(super) fn focused_chat(&self) -> Option<LiveId> {
        match self.main {
            Main::Live(id) if self.chats.contains_key(&id) => Some(id),
            _ => None,
        }
    }

    /// Whether a field of the conversation on screen has the keyboard: what
    /// is typed there is text, not a shortcut.
    pub(super) fn chat_typing(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.focused_chat()
            .and_then(|id| self.chats.get(&id))
            .is_some_and(|chat| chat.typing(window, cx))
    }

    /// Starts a conversation with `kind` in `cwd`, continuing the agent's
    /// session `resume` when given. Says whether it was started: when it was
    /// not, the status line says why and nothing else has changed.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_chat(
        &mut self,
        kind: AgentId,
        resume: Option<String>,
        account: Option<String>,
        machine: &MachineId,
        cwd: &str,
        place: Place,
        history: Option<SessionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(found) = self.snapshot.machine(machine).cloned() else {
            self.engine
                .report(StatusKind::Error, "That machine is not known.");
            return false;
        };
        // What a terminal would be refused for, a conversation is too: the
        // agent is not installed, the folder is gone, the account is unknown.
        let asked = Launch::Agent {
            kind,
            resume: resume.clone(),
            account: account.clone(),
        };
        let report = match self.engine.machine_state(&found.id) {
            MachineState::Online(Some(report)) => Some(report),
            _ => None,
        };
        if let Err(error) = launch::plan_with(
            &found,
            report.as_ref(),
            cwd,
            &asked,
            &self.engine.ssh(),
            &*self.options.system,
            &Self::launch_prefs(cx),
        ) {
            self.engine.report(StatusKind::Error, error.to_string());
            cx.notify();
            return false;
        }
        let id = self.live.next_id();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .submit_on_enter(true)
                .placeholder(format!(
                    "Ask {} anything  \u{b7}  / for commands  \u{b7}  @ for files",
                    crate::format::agent_name(kind)
                ))
        });
        let reason = cx.new(|cx| InputState::new(window, cx).placeholder("What to do instead"));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Type to narrow"));
        let find_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find in the conversation"));
        // Its words changed, or Enter went to the next match (Shift+Enter
        // to the one before).
        let finding = cx.subscribe_in(
            &find_input,
            window,
            move |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => this.chat_find_changed(id, cx),
                InputEvent::PressEnter { shift, .. } => this.chat_find_step(id, !*shift, cx),
                _ => {}
            },
        );
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => this.chat_send(id, window, cx),
                InputEvent::Change => this.chat_typed(id, cx),
                _ => {}
            },
        );
        // What narrows the list changed: its first row is the best again.
        let narrowed = cx.subscribe_in(
            &filter,
            window,
            move |this, _, event: &InputEvent, _, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                if let Some(menu) = this.chats.get_mut(&id).and_then(|chat| chat.menu.as_mut()) {
                    menu.selected = 0;
                    cx.notify();
                }
            },
        );
        let conversation = match &resume {
            Some(session) => Conversation::resumed(session.clone(), std::iter::empty()),
            None => Conversation::new(),
        };
        self.chats.insert(
            id,
            Chat {
                machine: machine.clone(),
                cwd: cwd.to_owned(),
                agent: kind,
                account,
                history: history.clone(),
                conversation,
                input,
                reason,
                scroll: ScrollHandle::new(),
                attached: Vec::new(),
                open_work: HashSet::new(),
                open_tools: HashSet::new(),
                picks: HashMap::new(),
                mode: None,
                model: None,
                effort: None,
                options: None,
                commands: Vec::new(),
                files: None,
                menu: None,
                filter,
                seen: true,
                name: None,
                title: None,
                titled: false,
                key: format!(
                    "{:x}-{}",
                    (self.options.now)().timestamp_millis().max(0),
                    id.0
                ),
                marks: Vec::new(),
                next_turn: 1,
                launching: false,
                rewinding: None,
                find: None,
                find_input,
                diff: None,
                recall: None,
                diff_reads: 0,
                measured: HashMap::new(),
                git: GitCell::new(window, cx),
                switchable: Vec::new(),
                handoff: None,
                turn: None,
                reader: None,
                since: None,
                spin: None,
                probe: None,
                _subscriptions: vec![subscription, narrowed, finding],
            },
        );
        match place {
            Place::Split(of, axis) if self.workspaces.locate(of).is_some() => {
                self.workspaces.split(of, axis, id);
            }
            Place::Tab(of) if self.workspaces.add_tab(of, id) => {}
            _ => {
                let root = tree::workspace_root(&self.snapshot, machine, cwd);
                self.workspaces.add_session(machine.as_str(), &root, id);
            }
        }
        if let Some(history) = history {
            self.chat_load_earlier(id, history, cx);
        }
        self.chat_probe(id, cx);
        self.chat_look_for_agents(id);
        self.refresh_live();
        self.open_live(id, window, cx);
        true
    }

    /// Reads what was said before in the session a conversation continues,
    /// so that it is on screen above what is said next.
    fn chat_load_earlier(&mut self, id: LiveId, history: SessionId, cx: &mut Context<Self>) {
        let store = self.engine.store().clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { store.session_messages(&history) })
                .await;
            let Ok(messages) = loaded else {
                return;
            };
            this.update(cx, |this, cx| {
                let Some(chat) = this.chats.get_mut(&id) else {
                    return;
                };
                // Only while nothing was said here yet: the history is what
                // came before, never something to put in the middle.
                if !chat.conversation.items.is_empty() {
                    return;
                }
                let Some(session) = chat.conversation.session_id.clone() else {
                    return;
                };
                let earlier = messages.iter().filter_map(|message| match message.role {
                    Role::User => Some((true, message.text.clone(), Some(message.at.timestamp()))),
                    Role::Assistant => {
                        Some((false, message.text.clone(), Some(message.at.timestamp())))
                    }
                    Role::Tool | Role::System => None,
                });
                chat.conversation = Conversation::resumed(session, earlier);
                chat.scroll.scroll_to_bottom();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks the agent what it offers on its machine (its modes and models),
    /// off the interface thread. Until it answers, and when it cannot, the
    /// conversation runs with the agent's own.
    pub(super) fn chat_probe(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let Some(adapter) = crate::chat::adapter_of(chat.agent) else {
            return;
        };
        let Some(machine) = self.snapshot.machine(&chat.machine).cloned() else {
            return;
        };
        let program = self.chat_program(chat.agent, cx);
        let mut probe = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            adapter.options.script.to_owned(),
            "leon".to_owned(),
            program,
        ];
        probe.extend((adapter.options.arguments)());
        let start = Start {
            env: self.chat_env(&machine, chat.agent, chat.account.as_deref(), &chat.cwd, cx),
            machine,
            argv: probe,
            cwd: chat.cwd.clone(),
        };
        let parse = adapter.options.parse;
        let spawner = self.options.chat_spawner.clone();
        let task = cx.spawn(async move |this, cx| {
            let printed = cx
                .background_spawn(async move {
                    let (_transport, output) = spawner.spawn(start).ok()?;
                    let mut text = String::new();
                    while let Ok(said) = output.recv_async().await {
                        match said {
                            Output::Line(line) => {
                                text.push_str(&line);
                                text.push('\n');
                            }
                            Output::Noise(_) => {}
                            Output::Exited(_) => break,
                        }
                    }
                    Some(text)
                })
                .await;
            let Some(printed) = printed else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(chat) = this.chats.get_mut(&id) {
                    let options = parse(&printed);
                    // What a turn listed since is newer than this.
                    if chat.commands.is_empty() {
                        chat.commands = options.commands.clone();
                    }
                    chat.options = Some(options);
                    chat.probe = None;
                }
                this.chat_look_for_agents(id);
                cx.notify();
            })
            .ok();
        });
        if let Some(chat) = self.chats.get_mut(&id) {
            chat.probe = Some(task);
        }
    }

    /// The program that starts `agent`: the one the settings name, else the
    /// protocol's.
    pub(super) fn chat_program(&self, agent: AgentId, cx: &gpui_kit::App) -> String {
        Self::launch_prefs(cx)
            .agent(agent)
            .executable
            .clone()
            .or_else(|| crate::chat::adapter_of(agent).map(|adapter| adapter.program.to_owned()))
            .unwrap_or_default()
    }

    /// What the agent's process is told through its environment. On this
    /// computer: what a terminal here is told (Leon's own, then the user's),
    /// then the account's. On another machine only the account's, with the
    /// home folder of that machine: the rest names files of this computer.
    pub(super) fn chat_env(
        &self,
        machine: &Machine,
        agent: AgentId,
        account: Option<&str>,
        cwd: &str,
        cx: &gpui_kit::App,
    ) -> Vec<(String, String)> {
        let prefs = Self::launch_prefs(cx);
        let local = matches!(machine.kind, MachineKind::Local);
        let (mut env, home) = if local {
            let mut env = self.memory_env(cwd);
            env.extend(prefs.env.iter().cloned());
            (env, self.options.system.home_dir())
        } else {
            let home = match self.engine.machine_state(&machine.id) {
                MachineState::Online(Some(report)) => report.home,
                _ => None,
            };
            (Vec::new(), home)
        };
        if let Ok(vars) = launch::account_env(account, agent, &prefs.accounts, home.as_deref()) {
            env.extend(vars);
        }
        env
    }

    /// Sends what is written in a conversation's field: a new turn, more for
    /// the turn in flight where the agent takes it, or the next turn's prompt
    /// where it does not.
    pub(super) fn chat_send(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let typed = chat.input.read(cx).value().trim().to_owned();
        let Some(text) = super::king::with_attached(&typed, &chat.attached) else {
            // Nothing to say: Enter is the answer the agent waits for.
            self.chat_accept(id, window, cx);
            return;
        };
        chat.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        if let Some(chat) = self.chats.get_mut(&id) {
            chat.attached.clear();
        }
        self.chat_say(id, text, cx);
    }

    /// The person says `text` in a conversation.
    pub(super) fn chat_say(&mut self, id: LiveId, text: String, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        chat.seen = true;
        // The agent is not started yet: what is said now is the next turn.
        if chat.launching {
            chat.conversation.queued.push_back(text);
            cx.notify();
            return;
        }
        if chat.turn.is_none() {
            self.chat_begin(id, text, cx);
            return;
        }
        // The person asked for what is said meanwhile to wait: it is the next
        // turn, and the agent is told nothing until then.
        if settings::text(cx, "chat_busy") == "queue" {
            chat.conversation.queued.push_back(text);
            cx.notify();
            return;
        }
        if self.chat_tell(id, &text, cx).is_err() {
            // This agent takes nothing more while it works, or the turn just
            // ended: it is the next turn.
            if let Some(chat) = self.chats.get_mut(&id) {
                chat.conversation.queued.push_back(text);
            }
            cx.notify();
        }
    }

    /// Tells the turn in flight `text`, where the agent takes it: the
    /// message is in the conversation at once. An error says why not (the
    /// agent takes nothing more while it works, or there is no turn).
    fn chat_tell(
        &mut self,
        id: LiveId,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), leon_turn::TurnError> {
        let now = Some(self.now().timestamp());
        let Some(chat) = self.chats.get_mut(&id) else {
            return Err(leon_turn::TurnError::TurnEnded);
        };
        let Some(turn) = chat.turn.as_mut() else {
            return Err(leon_turn::TurnError::TurnEnded);
        };
        let events = turn.act(Action::Prompt(text.to_owned()))?;
        chat.conversation.said_at(text, now);
        chat.scroll.scroll_to_bottom();
        self.chat_events(id, events, cx);
        Ok(())
    }

    /// Takes the queued message at `index` out of the queue: it is gone.
    pub(super) fn chat_queue_remove(&mut self, id: LiveId, index: usize, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.get_mut(&id) {
            if chat.conversation.queued.remove(index).is_some() {
                cx.notify();
            }
        }
    }

    /// Puts the queued message at `index` back in the field, with the files
    /// that went with it, to be changed and sent again: it leaves the queue.
    /// What is in the field already stays above it.
    pub(super) fn chat_queue_edit(
        &mut self,
        id: LiveId,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let Some(text) = chat.conversation.queued.remove(index) else {
            return;
        };
        let (words, files) = super::king::split_attached(&text);
        for file in files {
            if !chat.attached.contains(&file) {
                chat.attached.push(file);
            }
        }
        let typed = chat.input.read(cx).value().to_string();
        let value = if typed.trim().is_empty() {
            words
        } else {
            format!("{typed}\n{words}")
        };
        chat.input.update(cx, |input, cx| {
            input.set_value(value, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Sends the queued message at `index` into the running turn now, where
    /// the agent takes it, instead of at the end of the turn. An agent that
    /// takes nothing more while it works keeps it queued, and the status line
    /// says so. With no turn in flight it is the next turn already.
    pub(super) fn chat_queue_steer(&mut self, id: LiveId, index: usize, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let Some(text) = chat.conversation.queued.get(index).cloned() else {
            return;
        };
        if chat.turn.is_none() {
            // Between turns, or while one is being made ready: it waits for
            // the turn that is about to start.
            return;
        }
        match self.chat_tell(id, &text, cx) {
            Ok(()) => {
                if let Some(chat) = self.chats.get_mut(&id) {
                    // `chat_events` may have moved the queue: take out the
                    // message that was sent, wherever it is now.
                    if let Some(at) = chat
                        .conversation
                        .queued
                        .iter()
                        .position(|said| *said == text)
                    {
                        chat.conversation.queued.remove(at);
                    }
                }
                cx.notify();
            }
            Err(_) => {
                let name = self
                    .chats
                    .get(&id)
                    .map_or("This agent", |chat| crate::format::agent_name(chat.agent));
                self.engine.report(
                    StatusKind::Info,
                    format!("{name} cannot be told more while it works: the message waits for the next turn."),
                );
                cx.notify();
            }
        }
    }

    /// Starts a turn of a conversation with `prompt`: the words are on
    /// screen at once, the worktree is snapshotted when there is one, and then
    /// the agent is started ([`Self::chat_launch`]).
    fn chat_begin(&mut self, id: LiveId, prompt: String, cx: &mut Context<Self>) {
        let projects = self.engine.prefs().roots.claude_projects;
        let now = Some(self.now().timestamp());
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let Some(adapter) = crate::chat::adapter_of(chat.agent) else {
            return;
        };
        if self.snapshot.machine(&chat.machine).is_none() {
            return;
        }
        let local = chat.machine.is_local();
        let memory = Memory::of(
            adapter.rewind,
            chat.conversation.session_id.as_deref(),
            projects.as_deref(),
            &chat.cwd,
            local,
        );
        chat.conversation.sent_at(&prompt, now);
        let turn = chat.next_turn;
        chat.next_turn += 1;
        chat.marks.push(Mark {
            item: chat.conversation.items.len().saturating_sub(1),
            turn,
            checkpoint: None,
            memory,
        });
        chat.scroll.scroll_to_bottom();
        if !(local && checkpoint::in_worktree(std::path::Path::new(&chat.cwd))) {
            self.chat_launch(id, prompt, cx);
            return;
        }
        // The files as they are before the agent touches them. It takes as
        // long as the repository is big, so the interface does not wait.
        chat.launching = true;
        let (key, cwd) = (chat.key.clone(), chat.cwd.clone());
        let now = (self.options.now)().timestamp();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let commit = cx
                .background_spawn(async move {
                    let repo = checkpoint::Repo::of(std::path::Path::new(&cwd))?;
                    repo.forget_older_than(now - CHECKPOINTS_KEPT_SECS, &key);
                    let commit = repo.capture(&key, turn).ok()?;
                    repo.keep_newest(&key, CHECKPOINTS_PER_CHAT);
                    Some(commit)
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(chat) = this.chats.get_mut(&id) else {
                    return;
                };
                chat.launching = false;
                if let Some(mark) = chat.marks.iter_mut().find(|mark| mark.turn == turn) {
                    mark.checkpoint = commit;
                }
                this.chat_launch(id, prompt, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Starts the agent's process for the turn whose prompt was just said.
    fn chat_launch(&mut self, id: LiveId, prompt: String, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let Some(adapter) = crate::chat::adapter_of(chat.agent) else {
            return;
        };
        let Some(machine) = self.snapshot.machine(&chat.machine).cloned() else {
            return;
        };
        let program = self.chat_program(chat.agent, cx);
        let env = self.chat_env(&machine, chat.agent, chat.account.as_deref(), &chat.cwd, cx);
        let resume = chat.conversation.session_id.clone();
        // After a switch of agents the first message carries what was said
        // before, out of sight: the conversation shows only the person's
        // words.
        let handed = chat
            .handoff
            .as_ref()
            .filter(|handoff| handoff.sent_in.is_none())
            .map(|handoff| format!("{}\n\n{prompt}", handoff.text));
        let prompt = handed.clone().unwrap_or(prompt);
        let spec = TurnSpec {
            program: &program,
            prompt: &prompt,
            resume: resume.as_deref(),
            mode: chat.mode.as_deref(),
            model: chat.model.as_deref(),
            effort: chat.effort.as_deref(),
            cwd: Some(&chat.cwd),
            ..TurnSpec::default()
        };
        let start = Start {
            machine,
            argv: (adapter.argv)(&spec),
            cwd: chat.cwd.clone(),
            env,
        };
        let unique = (self.options.now)().timestamp_millis().max(0) as u64;
        let started = Turn::start(adapter, &spec, start, unique, &*self.options.chat_spawner);
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if let (Some(handoff), Some(_), true) = (chat.handoff.as_mut(), &handed, started.is_ok()) {
            handoff.sent_in = Some(chat.next_turn.saturating_sub(1));
        }
        match started {
            Ok((turn, output, events)) => {
                chat.since = Some(unique);
                chat.turn = Some(turn);
                chat.reader = Some(cx.spawn(async move |this, cx| {
                    while let Ok(said) = output.recv_async().await {
                        let last = matches!(said, Output::Exited(_));
                        if this
                            .update(cx, |this, cx| this.chat_output(id, said, cx))
                            .is_err()
                            || last
                        {
                            break;
                        }
                    }
                }));
                chat.scroll.scroll_to_bottom();
                self.chat_events(id, events, cx);
                self.chat_spin(id, cx);
            }
            Err(error) => {
                chat.conversation
                    .refused(&format!("The agent could not be started: {error}."));
                chat.scroll.scroll_to_bottom();
                self.refresh_live();
                cx.notify();
            }
        }
    }

    /// Keeps a conversation's spinner turning while its turn goes on: it is
    /// drawn again every frame, and the loop ends with the turn.
    fn chat_spin(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if chat.spin.is_some() {
            return;
        }
        chat.spin = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(super::king_view::SPIN).await;
            let going = this.update(cx, |this, cx| {
                let Some(chat) = this.chats.get_mut(&id) else {
                    return false;
                };
                let going = chat.conversation.running();
                if going {
                    cx.notify();
                } else {
                    chat.spin = None;
                }
                going
            });
            if !going.unwrap_or(false) {
                return;
            }
        }));
    }

    /// The process of a conversation's turn said something.
    pub(super) fn chat_output(&mut self, id: LiveId, output: Output, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let Some(turn) = chat.turn.as_mut() else {
            return;
        };
        let ended = matches!(output, Output::Exited(_));
        let events = turn.output(output);
        if let Some(listed) = turn.take_commands() {
            chat.commands = listed;
        }
        if ended {
            chat.turn = None;
        }
        self.chat_events(id, events, cx);
        if ended {
            // The agent wrote its session: the history has it at the next
            // import, and what the person wrote meanwhile is the next turn.
            self.request_import(cx);
            let next = self
                .chats
                .get_mut(&id)
                .and_then(|chat| chat.conversation.queued.pop_front());
            if let Some(next) = next {
                self.chat_begin(id, next, cx);
            }
        }
    }

    /// Puts the events of a turn in its conversation, and keeps the tree and
    /// the scroll position with them.
    pub(super) fn chat_events(
        &mut self,
        id: LiveId,
        events: Vec<TurnEvent>,
        cx: &mut Context<Self>,
    ) {
        let on_screen = self.window_active && self.focused_chat() == Some(id);
        let now = Some(self.now().timestamp());
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let follow = chat.at_end();
        let before = (chat.state(), chat.activity(), chat.label());
        let mut finished = false;
        // What the person is told when they are not looking: the last thing
        // of the batch that wants them.
        let mut said = None;
        for event in events {
            if let TurnEvent::ModeChanged { mode } = &event {
                chat.mode = Some(mode.clone());
            }
            match &event {
                TurnEvent::Finished {
                    ok,
                    error,
                    interrupted: false,
                    ..
                } => {
                    said = Some(if *ok {
                        super::notify::ChatEvent::Finished
                    } else {
                        super::notify::ChatEvent::Failed(
                            error
                                .as_deref()
                                .and_then(|error| error.lines().next())
                                .unwrap_or_default()
                                .to_owned(),
                        )
                    });
                }
                TurnEvent::Question { kind, .. } => {
                    said = Some(super::notify::ChatEvent::Asks(match kind {
                        QuestionKind::Permission { tool, summary, .. } => {
                            format!("permission \u{b7} {tool} {summary}")
                                .trim_end()
                                .to_owned()
                        }
                        QuestionKind::Choices { .. } => "a question".to_owned(),
                        QuestionKind::Plan { .. } => "you to approve a plan".to_owned(),
                    }));
                }
                _ => {}
            }
            if matches!(event, TurnEvent::Finished { .. }) {
                chat.seen = on_screen;
                finished = true;
            }
            chat.conversation.apply_at(event, now);
        }
        if follow {
            chat.scroll.scroll_to_bottom();
        }
        if before != (chat.state(), chat.activity(), chat.label()) {
            self.rebuild_rows();
        }
        if let (Some(said), false) = (said, on_screen) {
            self.raise_chat(id, &said, cx);
        }
        // The agent may have opened a pull request: its branch's is asked
        // for now, not at the next minute, for the foot to show it.
        let project = self
            .chats
            .get(&id)
            .filter(|_| finished)
            .and_then(|chat| self.snapshot.worktree_at(&chat.machine, &chat.cwd))
            .map(|worktree| worktree.project_id.clone());
        if let Some(project) = project {
            self.engine.check_open_pull_requests(project);
        }
        // What the turn changed, counted in the worktree, for the card at its
        // end.
        if finished {
            self.chat_measure(id, cx);
            self.chat_title(id, cx);
        }
        cx.notify();
    }

    /// After a conversation's first turn, asks for a short title of it, unless
    /// the setting is off, the person named it or it was asked already. It
    /// never waits: the answer is put in by [`Self::chat_titled`], and a
    /// failure changes nothing and says nothing.
    fn chat_title(&mut self, id: LiveId, cx: &mut Context<Self>) {
        if !settings::flag(cx, "chat_titles") {
            return;
        }
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if chat.titled || chat.name.is_some() || chat.conversation.failed {
            return;
        }
        let first = |wanted: bool| {
            chat.conversation.items.iter().find_map(|item| match item {
                crate::chat::model::Item::User { text, .. } if wanted => {
                    Some(super::king::split_attached(text).0)
                }
                crate::chat::model::Item::Agent { text, .. } if !wanted => Some(text.clone()),
                _ => None,
            })
        };
        let (Some(prompt), Some(reply)) = (first(true), first(false)) else {
            return;
        };
        chat.titled = true;
        let ask = TitleAsk {
            machine: chat.machine.clone(),
            path: chat.cwd.clone(),
            agent: chat.agent,
            prompt,
            reply,
        };
        let titles = self.options.titles.clone();
        let made = titles(&self.engine, ask, cx);
        cx.spawn(async move |this, cx| {
            let made = made.await;
            this.update(cx, |this, cx| this.chat_titled(id, made, cx))
                .ok();
        })
        .detach();
    }

    /// The title that was asked for came back. A name the person gave in the
    /// meantime stays, and a failure is only noted in the log.
    fn chat_titled(&mut self, id: LiveId, made: Result<String, String>, cx: &mut Context<Self>) {
        let title = match made {
            Ok(title) if !title.trim().is_empty() => title.trim().to_owned(),
            Ok(_) => return,
            Err(error) => {
                tracing::debug!(%error, "no title for the conversation");
                return;
            }
        };
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if chat.name.is_some() {
            return;
        }
        chat.title = Some(title.clone());
        // The session its agent wrote keeps it too, when it is known by now:
        // otherwise it is kept when the two are linked ([`Self::link_chats`]).
        if let Some(session) = chat.history.clone() {
            self.engine
                .submit(crate::engine::Op::TitleSession { session, title });
        }
        self.rebuild_rows();
        cx.notify();
    }

    /// Answers what a conversation's agent asks.
    pub(super) fn chat_answer(
        &mut self,
        id: LiveId,
        question: &str,
        answer: Answer,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let Some(turn) = chat.turn.as_mut() else {
            return;
        };
        match turn.act(Action::Answer(question.to_owned(), answer)) {
            Ok(events) => {
                chat.picks.retain(|(of, _), _| of != question);
                self.chat_events(id, events, cx);
            }
            Err(error) => {
                self.engine.report(StatusKind::Error, error.to_string());
                cx.notify();
            }
        }
    }

    /// Answers a permission question with one of its choices; a refusal takes
    /// what is written in the reason field with it.
    pub(super) fn chat_permit(
        &mut self,
        id: LiveId,
        question: &str,
        choice: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let takes_reason = chat.conversation.questions.iter().any(|asked| {
            asked.id == question
                && matches!(&asked.kind, QuestionKind::Permission { choices, .. }
                    if choices.iter().any(|c| c.id == choice && c.takes_reason))
        });
        let reason = chat.reason.read(cx).value().trim().to_owned();
        let reason = (takes_reason && !reason.is_empty()).then_some(reason);
        if reason.is_some() {
            chat.reason
                .update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.chat_answer(
            id,
            question,
            Answer::Permission {
                choice_id: choice.to_owned(),
                reason,
            },
            cx,
        );
    }

    /// Answers the oldest open question the way its accented button does:
    /// allows this one use, starts working on the plan, or sends what is
    /// picked. Nothing happens when the question has no such answer.
    pub(super) fn chat_accept(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(asked) = self
            .chats
            .get(&id)
            .and_then(|chat| chat.conversation.questions.first().cloned())
        else {
            return;
        };
        match &asked.kind {
            QuestionKind::Permission { choices, .. } => {
                let once = choices
                    .iter()
                    .find(|choice| choice.effect == leon_turn::PermissionEffect::AllowOnce);
                if let Some(once) = once {
                    let choice = once.id.clone();
                    self.chat_permit(id, &asked.id, &choice, window, cx);
                }
            }
            QuestionKind::Plan { .. } => self.chat_plan(id, &asked.id, true, window, cx),
            QuestionKind::Choices { .. } => self.chat_submit_picks(id, &asked.id, cx),
        }
    }

    /// Answers a plan: start working, or keep planning with what is written
    /// in the reason field.
    pub(super) fn chat_plan(
        &mut self,
        id: LiveId,
        question: &str,
        approve: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let feedback = chat.reason.read(cx).value().trim().to_owned();
        let feedback = (!approve && !feedback.is_empty()).then_some(feedback);
        chat.reason
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.chat_answer(id, question, Answer::Plan { approve, feedback }, cx);
    }

    /// Picks (or, where several may be picked, also unpicks) an option of a
    /// question. A question that is alone and takes one option is answered
    /// by the pick itself.
    pub(super) fn chat_pick(
        &mut self,
        id: LiveId,
        question: &str,
        of: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let Some(QuestionKind::Choices { questions }) = chat
            .conversation
            .questions
            .iter()
            .find(|asked| asked.id == question)
            .map(|asked| asked.kind.clone())
        else {
            return;
        };
        let Some(asked) = questions.iter().find(|asked| asked.id == of) else {
            return;
        };
        let picked = chat
            .picks
            .entry((question.to_owned(), of.to_owned()))
            .or_default();
        if asked.multi_select {
            match picked.iter().position(|have| have == label) {
                Some(at) => {
                    picked.remove(at);
                }
                None => picked.push(label.to_owned()),
            }
        } else {
            *picked = vec![label.to_owned()];
        }
        if questions.len() == 1 && !asked.multi_select {
            self.chat_submit_picks(id, question, cx);
        } else {
            cx.notify();
        }
    }

    /// Sends what is picked of a question with options, once each of its
    /// questions has a pick.
    pub(super) fn chat_submit_picks(&mut self, id: LiveId, question: &str, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let Some(QuestionKind::Choices { questions }) = chat
            .conversation
            .questions
            .iter()
            .find(|asked| asked.id == question)
            .map(|asked| &asked.kind)
        else {
            return;
        };
        let mut answers = Vec::new();
        for asked in questions {
            let labels = chat
                .picks
                .get(&(question.to_owned(), asked.id.clone()))
                .cloned()
                .unwrap_or_default();
            if labels.is_empty() {
                return;
            }
            answers.push(ChoiceAnswer {
                question_id: asked.id.clone(),
                labels,
                other: None,
            });
        }
        self.chat_answer(id, question, Answer::Choices { answers }, cx);
    }

    /// Stops the turn in flight; the conversation stays.
    pub(super) fn chat_interrupt(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        chat.conversation.queued.clear();
        let Some(turn) = chat.turn.as_mut() else {
            return;
        };
        match turn.act(Action::Interrupt) {
            Ok(events) => self.chat_events(id, events, cx),
            // An agent that cannot be asked to stop is ended.
            Err(_) => turn.kill(),
        }
    }

    /// Opens or folds a stretch of work.
    pub(super) fn chat_toggle_work(&mut self, id: LiveId, from: usize, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.get_mut(&id) {
            if !chat.open_work.remove(&from) {
                chat.open_work.insert(from);
            }
            cx.notify();
        }
    }

    /// Shows or hides the detail of a tool.
    pub(super) fn chat_toggle_tool(&mut self, id: LiveId, tool: &str, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.get_mut(&id) {
            if !chat.open_tools.remove(tool) {
                chat.open_tools.insert(tool.to_owned());
            }
            cx.notify();
        }
    }

    /// The person looks at a conversation: what its agent last did is seen,
    /// and its field has the keyboard.
    pub(super) fn chat_shown(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if !chat.seen {
            chat.seen = true;
            self.rebuild_rows();
        }
        if let Some(chat) = self.chats.get(&id) {
            if !chat.typing(window, cx) {
                chat.input.update(cx, |input, cx| input.focus(window, cx));
            }
        }
    }

    /// A key of the conversation on screen that is its own: the keys of the
    /// list over its field while one is open, else Escape, which stops the
    /// turn in flight. `true` when the key was taken.
    pub(super) fn chat_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(id) = self.focused_chat() else {
            return false;
        };
        if self.chats.get(&id).is_some_and(|chat| chat.menu.is_some()) {
            return self.chat_menu_key(id, stroke, window, cx);
        }
        // Up and Down in the field walk the prompts sent before.
        if self.chat_recall_key(id, stroke, window, cx) {
            return true;
        }
        // While the microphone is open, Enter ends the dictation and Escape
        // drops it: neither sends the prompt nor stops the turn.
        if self.dictating_in() == Some(id) && !stroke.modifiers.modified() {
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
        if paste && self.chat_paste_image(id, cx) {
            return true;
        }
        // Escape closes the bar that finds words before it does anything else.
        if stroke.key == "escape"
            && !stroke.modifiers.modified()
            && self.chats.get(&id).is_some_and(|chat| chat.find.is_some())
        {
            self.chat_find_close(id, window, cx);
            return true;
        }
        // Escape closes the question of going back before it stops anything.
        if stroke.key == "escape"
            && !stroke.modifiers.modified()
            && self
                .chats
                .get(&id)
                .is_some_and(|chat| chat.rewinding.is_some_and(|asked| !asked.busy))
        {
            self.chat_rewind_cancel(id, cx);
            return true;
        }
        // Escape closes what is open beside the conversation (the diffs, the
        // git cell) before it stops anything.
        if stroke.key == "escape" && !stroke.modifiers.modified() {
            let open = self
                .chats
                .get(&id)
                .map(|chat| (chat.diff.is_some(), chat.git.open));
            match open {
                Some((true, _)) => {
                    self.chat_diff_close(id, cx);
                    return true;
                }
                Some((false, true)) => {
                    self.chat_git_close(id, cx);
                    return true;
                }
                _ => {}
            }
        }
        let running = self
            .chats
            .get(&id)
            .is_some_and(|chat| chat.conversation.running());
        if stroke.key == "escape" && !stroke.modifiers.modified() && running {
            self.chat_interrupt(id, cx);
            return true;
        }
        false
    }

    /// Copies the text selected in the conversation or King's view on
    /// screen, when the chord to copy is pressed and something is selected:
    /// the window's selection has no owner of its own to copy it. `false`
    /// otherwise, so the field copies what is selected in it.
    pub(super) fn copy_selection(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let copy = stroke.key == "c"
            && !stroke.modifiers.alt
            && !stroke.modifiers.shift
            && if crate::platform::is_mac() {
                stroke.modifiers.platform && !stroke.modifiers.control
            } else {
                stroke.modifiers.control && !stroke.modifiers.platform
            };
        if !copy || (self.focused_chat().is_none() && !self.king_open()) {
            return false;
        }
        let text = gpui_kit::base::TextSelection::selected_text(window, cx);
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.to_owned()));
        true
    }

    /// Attaches files of this computer to what is said next in a
    /// conversation: picked, dropped on it or copied in the file manager.
    /// An agent on another machine cannot open them, which is said instead.
    /// `true`, the files having been dealt with either way.
    pub(super) fn chat_attach_paths(
        &mut self,
        id: LiveId,
        paths: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(chat) = self.chats.get_mut(&id) else {
            return false;
        };
        if !chat.machine.is_local() {
            self.engine.report(
                StatusKind::Info,
                "Attaching files needs the agent to run on this computer; the files are here.",
            );
            return true;
        }
        for path in paths {
            if !chat.attached.contains(&path) {
                chat.attached.push(path);
            }
        }
        cx.notify();
        true
    }

    /// Asks for files to attach to what is said next in a conversation.
    pub(super) fn chat_attach(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        let picked = (self.options.pick_files)(cx);
        cx.spawn_in(window, async move |this, cx| {
            let paths = picked.await;
            if paths.is_empty() {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                this.chat_attach_paths(id, paths, cx);
                if let Some(chat) = this.chats.get(&id) {
                    chat.input.update(cx, |input, cx| input.focus(window, cx));
                }
            })
            .ok();
        })
        .detach();
    }

    /// Pastes the clipboard's image into a conversation's field: it is kept
    /// in a file and attached to what is said next, which names its path for
    /// the agent to open. `false` when there is no image, so the field
    /// pastes text as it does.
    fn chat_paste_image(&mut self, id: LiveId, cx: &mut Context<Self>) -> bool {
        // Files copied in the file manager are attached as they are.
        let files = crate::clip_image::files(cx);
        if !files.is_empty() {
            return self.chat_attach_paths(id, files, cx);
        }
        let Some(image) = (self.options.clipboard_image)(cx) else {
            return false;
        };
        let local = self
            .chats
            .get(&id)
            .is_some_and(|chat| chat.machine.is_local());
        if !local {
            self.engine.report(
                StatusKind::Info,
                "Image paste needs the agent to run on this computer; the clipboard is local.",
            );
            return true;
        }
        let stamp = (self.options.now)().format("%Y%m%d-%H%M%S%3f");
        let path = self
            .options
            .attachments
            .join(format!("pasted-{stamp}.{}", image.extension));
        let kept = std::fs::create_dir_all(&self.options.attachments)
            .and_then(|()| std::fs::write(&path, &image.bytes));
        match (kept, self.chats.get_mut(&id)) {
            (Ok(()), Some(chat)) => chat.attached.push(path),
            (Ok(()), None) => {}
            (Err(error), _) => self.engine.report(
                StatusKind::Error,
                format!("The image could not be kept: {error}."),
            ),
        }
        cx.notify();
        true
    }

    /// Closes a conversation, ending the turn in flight. The pane's sibling
    /// takes the room; the last pane of the workspace returns the main pane
    /// to what the folder shows. The agent keeps its own session, which the
    /// history has: opening that resumes it.
    pub(super) fn close_chat(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.remove(&id) else {
            return;
        };
        let was_open = matches!(self.main, Main::Live(open) if open == id);
        let closed = self.workspaces.close(id);
        self.refresh_live();
        self.chat_forget_checkpoints(&chat, cx);
        self.after_close(closed, was_open, &chat.machine, &chat.cwd, window, cx);
        if self.pane == Pane::Main && self.focused_chat().is_none() {
            self.sync_focus(window, cx);
        }
    }

    /// Links each conversation to the history session its agent wrote, once
    /// the importer has stored it: the tree then shows one row for the two
    /// (live now, history later), and opening the session lands on the
    /// conversation. The agent says its session's id itself, so there is
    /// nothing to guess. `true` when a link was made.
    pub(super) fn link_chats(&mut self) -> bool {
        let mut linked = false;
        for chat in self.chats.values_mut() {
            if chat.history.is_some() {
                continue;
            }
            let Some(external) = chat.conversation.session_id.as_deref() else {
                continue;
            };
            let found = self.snapshot.sessions.iter().find(|stored| {
                stored.machine_id == chat.machine
                    && stored.agent == chat.agent
                    && stored.external_id == external
            });
            if let Some(found) = found {
                chat.history = Some(found.id.clone());
                linked = true;
                // A title generated before the link goes to the session now.
                if let (Some(title), None) = (&chat.title, &chat.name) {
                    self.engine.submit(crate::engine::Op::TitleSession {
                        session: found.id.clone(),
                        title: title.clone(),
                    });
                }
            }
        }
        linked
    }

    /// Closes the conversations whose folder is `root` or lies inside it on
    /// `machine`: the worktree they ran in was removed.
    pub(super) fn close_chats_in(
        &mut self,
        machine: &MachineId,
        root: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inside: Vec<LiveId> = self
            .chats
            .iter()
            .filter(|(_, chat)| {
                &chat.machine == machine && leon_core::path::is_within(&chat.cwd, root)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in inside {
            self.close_chat(id, window, cx);
        }
    }

    /// What runs here for this history session: the terminal that has it, or
    /// the conversation. A history row with either is that session's row, and
    /// is brought forward, never started a second time.
    pub(super) fn running_of_history(&self, history: &SessionId) -> Option<LiveId> {
        self.live
            .of_history(history)
            .map(|live| live.id)
            .or_else(|| self.chat_of_history(history))
    }

    /// The conversation started from this history session, if one is open.
    pub(super) fn chat_of_history(&self, history: &SessionId) -> Option<LiveId> {
        self.chats
            .iter()
            .find_map(|(id, chat)| (chat.history.as_ref() == Some(history)).then_some(*id))
    }
}

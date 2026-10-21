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
use super::chat_menu::Menu;
use super::live::{LiveId, LiveState};
use super::shell::{Main, Pane, Shell};
use super::terminals::Place;
use super::tree;
use crate::chat::model::{Conversation, Phase};
use crate::chat::turn::{Output, Start, Turn};
use crate::engine::{MachineState, StatusKind};
use crate::launch::{self, Launch};
use crate::settings;

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
    /// What the tree and the tab call it: the first thing the person said,
    /// else the agent's name.
    pub fn label(&self) -> String {
        self.conversation
            .items
            .iter()
            .find_map(|item| match item {
                crate::chat::model::Item::User { text } => text
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
                turn: None,
                reader: None,
                since: None,
                spin: None,
                probe: None,
                _subscriptions: vec![subscription, narrowed],
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
                    Role::User => Some((true, message.text.clone())),
                    Role::Assistant => Some((false, message.text.clone())),
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
    fn chat_probe(&mut self, id: LiveId, cx: &mut Context<Self>) {
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
                    cx.notify();
                }
            })
            .ok();
        });
        if let Some(chat) = self.chats.get_mut(&id) {
            chat.probe = Some(task);
        }
    }

    /// The program that starts `agent`: the one the settings name, else the
    /// protocol's.
    fn chat_program(&self, agent: AgentId, cx: &gpui_kit::App) -> String {
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
    fn chat_env(
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
        if chat.turn.is_none() {
            self.chat_begin(id, text, cx);
            return;
        }
        let told = chat
            .turn
            .as_mut()
            .map(|turn| turn.act(Action::Prompt(text.clone())));
        match told {
            Some(Ok(events)) => {
                chat.conversation.said(&text);
                chat.scroll.scroll_to_bottom();
                self.chat_events(id, events, cx);
            }
            // This agent takes nothing more while it works, or the turn just
            // ended: it is the next turn.
            _ => {
                chat.conversation.queued.push_back(text);
                cx.notify();
            }
        }
    }

    /// Starts a turn of a conversation with `prompt`.
    fn chat_begin(&mut self, id: LiveId, prompt: String, cx: &mut Context<Self>) {
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
        match started {
            Ok((turn, output, events)) => {
                chat.conversation.sent(&prompt);
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
                chat.conversation.said(&prompt);
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
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let follow = chat.at_end();
        let before = (chat.state(), chat.activity(), chat.label());
        let mut finished = false;
        for event in events {
            if let TurnEvent::ModeChanged { mode } = &event {
                chat.mode = Some(mode.clone());
            }
            if matches!(event, TurnEvent::Finished { .. }) {
                chat.seen = on_screen;
                finished = true;
            }
            chat.conversation.apply(event);
        }
        if follow {
            chat.scroll.scroll_to_bottom();
        }
        if before != (chat.state(), chat.activity(), chat.label()) {
            self.rebuild_rows();
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

    /// Pastes the clipboard's image into a conversation's field: it is kept
    /// in a file and attached to what is said next, which names its path for
    /// the agent to open. `false` when there is no image, so the field
    /// pastes text as it does.
    fn chat_paste_image(&mut self, id: LiveId, cx: &mut Context<Self>) -> bool {
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

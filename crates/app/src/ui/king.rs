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
use leon_king::coordinator::{Reply as Answer, Target};
use leon_king::protocol::{Entries, Notice, Request, Status};
use leon_king::transcript::{Entry, KEPT};
use serde_json::Value;

use super::home::Nav;
use super::shell::{Main, Pane, Shell};
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
    /// Anything else: only a refusal is said.
    Done,
}

/// What the window keeps of King.
pub struct KingUi {
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
    /// The last thing King's service had for the person.
    pub note: Option<(String, String)>,
    /// What the main pane showed before, to put back.
    pub back: Option<(Main, Pane)>,
    asked: HashMap<u64, Asked>,
    next_tag: u64,
    reader: Option<Task<()>>,
    subscriptions: Vec<Subscription>,
}

impl Default for KingUi {
    fn default() -> Self {
        Self {
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
            note: None,
            back: None,
            asked: HashMap::new(),
            next_tag: 1,
            reader: None,
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
        self.home.nav = Some(Nav::King);
        self.pane = Pane::Main;
        self.king_fields(window, cx);
        self.king_connect(window, cx);
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
        self.sync_focus(window, cx);
        cx.notify();
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
        if self.king_ui.link.is_some() || self.king_ui.connecting {
            return;
        }
        let Some(link) = self.options.king.clone() else {
            self.king_ui.error = Some("King is not available in this window.".to_owned());
            return;
        };
        self.king_ui.connecting = true;
        self.king_ui.error = None;
        let task = cx.spawn_in(window, async move |this, cx| {
            let opened = cx.background_spawn(async move { link.open() }).await;
            let heard = match opened {
                Ok(connection) => {
                    let heard = connection.heard;
                    let ok = this.update(cx, |this, cx| {
                        this.king_ui.connecting = false;
                        this.king_ui.link = Some(connection.ask);
                        this.king_ui.brain.asking = false;
                        for feed in this.king_ui.workers.values_mut() {
                            feed.asking = false;
                        }
                        this.king_ask(Request::Status, Asked::Status, cx);
                        cx.notify();
                    });
                    if ok.is_err() {
                        return;
                    }
                    heard
                }
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.king_ui.connecting = false;
                        this.king_ui.error = Some(error);
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
        self.king_ui.error = Some(why);
        cx.notify();
    }

    /// What the service said.
    fn king_heard(&mut self, said: Heard, window: &mut Window, cx: &mut Context<Self>) {
        match said {
            Heard::Lost(why) => self.king_lost(format!("King's service went away: {why}"), cx),
            Heard::Notice(Notice::Changed { status }) => self.king_status(status, cx),
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
        // A card whose worker is gone (stopped and forgotten, or taken over)
        // closes.
        if let Some(open) = self.king_ui.open {
            if !status.workers.iter().any(|worker| worker.id == open) {
                self.king_ui.open = None;
            }
        }
        self.king_fetch(&status, cx);
        self.king_ui.status = Some(status);
        cx.notify();
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
        let text = input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
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
        self.king_ask(Request::Say { text }, Asked::Done, cx);
        self.king_ui.note = None;
        self.king_ui.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// What the person wrote to the open worker is told to it.
    pub(super) fn king_tell_worker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(worker), Some(input)) = (self.king_ui.open, self.king_ui.worker_input.clone())
        else {
            return;
        };
        let text = input.read(cx).value().trim().to_owned();
        if text.is_empty() || !self.king_ui.connected() {
            return;
        }
        input.update(cx, |input, cx| input.set_value("", window, cx));
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
        match self.king_ui.open {
            Some(id) if busy(Target::Worker(id)) => self.king_interrupt(Target::Worker(id), cx),
            Some(_) => self.king_close_worker(window, cx),
            None if busy(Target::Brain) => self.king_interrupt(Target::Brain, cx),
            None => self.close_king(window, cx),
        }
        true
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
        }
    }

    fn conversation(last_seq: Option<u64>) -> ConversationStatus {
        ConversationStatus {
            agent: "claude".into(),
            model: None,
            effort: None,
            session_id: None,
            busy: false,
            queued: 0,
            questions: Vec::new(),
            cost_usd: 0.0,
            last_seq,
        }
    }

    fn status(brain: Option<u64>, workers: &[(u32, Option<u64>)]) -> Status {
        Status {
            brain: conversation(brain),
            workers: workers
                .iter()
                .map(|(id, last)| WorkerStatus {
                    id: *id,
                    title: format!("worker {id}"),
                    project: None,
                    cwd: "/tmp".into(),
                    job: Default::default(),
                    tier: None,
                    state: leon_king::coordinator::WorkerState::Working,
                    conversation: conversation(*last),
                    last_answer: None,
                    updated_ms: 0,
                })
                .collect(),
            news: 0,
            spent_today_usd: 0.0,
            daily_budget_usd: None,
        }
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

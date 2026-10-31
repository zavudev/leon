//! Drawing King's view: King's conversation on the left, its workers on the
//! right.
//!
//! The conversation is drawn by the view the window's own conversations are
//! (`transcript.rs`): the words, the work folded into a line, the todos, the
//! plan. King adds Leon's report of each answer in small mono under it, its
//! news of the workers, and a question framed in the accent above the field.
//! A worker's card, opened, draws its conversation the same way in the right
//! column, with what can be done to the worker over it.

use gpui_kit::component::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, Context, Div, FontWeight, SharedString, Stateful};
use leon_conversation::model::{blocks, Block, Item};
use leon_core::AgentId;
use leon_king::coordinator::{Open, Target, WorkerState};
use leon_king::history::Past;
use leon_king::protocol::{ConversationStatus, Status, WorkerStatus};
use leon_king::report::{count, span, Report};
use leon_king::run::RunState;
use leon_king::verify::{self, CheckState, Ci};

use super::activity::Activity;
use super::chat_voice::Field;
use super::edits::Subject;
use super::king::{picks, Feed, Pick, Tone};
use super::shell::Shell;
use super::transcript::{
    column, plan_card, question_frame, status_line, Folder, Folds, Ids, Speaker, Surface,
    Transcript,
};
use super::widgets::{activity_dot, attached_files, led, mark, mono, with_latest, Lion};
use crate::icons::{agent_icon, icon, IconName};
use crate::theme::{fonts, metrics, px, Palette};

/// How wide the column of the workers is.
const WORKERS: f32 = 380.;

/// The dot of a worker's card.
pub(super) fn worker_activity(state: WorkerState) -> Activity {
    match state {
        WorkerState::Working => Activity::Working,
        WorkerState::Asking => Activity::NeedsYou,
        WorkerState::Done => Activity::TurnOver,
        WorkerState::Failed => Activity::Failed,
        WorkerState::Paused => Activity::Waiting,
        WorkerState::Stopped | WorkerState::Released => Activity::Off,
    }
}

/// A worker's state, as its card says it.
fn state_word(state: WorkerState) -> &'static str {
    match state {
        WorkerState::Working => "WORKING",
        WorkerState::Asking => "ASKS",
        WorkerState::Done => "DONE",
        WorkerState::Failed => "FAILED",
        WorkerState::Paused => "STOPPED, WAITS",
        WorkerState::Stopped => "STOPPED",
        WorkerState::Released => "TAKEN OVER",
    }
}

/// An agent by its tag, its model and its effort, in mono capitals:
/// `CLAUDE OPUS HIGH`.
pub(super) fn runs_on(conversation: &ConversationStatus) -> String {
    let agent = match AgentId::parse(&conversation.agent) {
        Some(agent) => agent.tag().to_uppercase(),
        None => conversation.agent.to_uppercase(),
    };
    let mut words = vec![agent];
    words.extend(conversation.model.iter().map(|model| model.to_uppercase()));
    words.extend(
        conversation
            .effort
            .iter()
            .map(|effort| effort.to_uppercase()),
    );
    words.join(" ")
}

/// What was spent today, and of how much.
fn spent(status: &Status) -> String {
    match status.daily_budget_usd {
        Some(budget) => format!("${:.2} of ${budget:.2} today", status.spent_today_usd),
        None => format!("${:.2} today", status.spent_today_usd),
    }
}

/// The frames of the spinner of a turn in flight.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// How long one frame of the spinner shows.
pub(super) const SPIN: std::time::Duration = std::time::Duration::from_millis(100);

/// The spinner's frame now: the clock picks it, so every spinner on screen
/// turns together, and the view only has to be drawn again.
pub(super) fn spinner_frame() -> &'static str {
    let ticks = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() / SPIN.as_millis());
    SPINNER[(ticks % SPINNER.len() as u128) as usize]
}

/// How a button looks.
#[derive(Clone, Copy)]
enum Look {
    Primary,
    Outline,
    Danger,
}

/// A button: mono capitals in a control-high box.
fn button(name: String, label: &str, look: Look, c: &Palette) -> Stateful<Div> {
    let hover = c.surface_2;
    let selector = name.clone();
    let base = div()
        .id(SharedString::from(name))
        .debug_selector(move || selector.clone())
        .flex_none()
        .h(px(24.))
        .px(px(9.))
        .flex()
        .items_center()
        .rounded(metrics::RADIUS())
        .border_1()
        .cursor_pointer()
        .child(mono(label.to_owned()));
    match look {
        Look::Primary => base
            .border_color(c.primary_fill)
            .bg(c.primary_fill)
            .text_color(c.on_primary)
            .hover(|style| style.opacity(0.88)),
        Look::Outline => base
            .border_color(c.elevated_border)
            .text_color(c.text)
            .hover(move |style| style.bg(hover)),
        Look::Danger => base
            .border_color(c.error)
            .text_color(c.error)
            .hover(move |style| style.bg(hover)),
    }
}

/// How Alt with a number is written on this platform.
fn alt_key(n: usize) -> String {
    if crate::platform::is_mac() {
        format!("\u{2325}{n}")
    } else {
        format!("ALT+{n}")
    }
}

/// The selector part of a conversation: `brain` or `worker-3`.
fn key_of(target: Target) -> String {
    match target {
        Target::Brain => "brain".to_owned(),
        Target::Worker(id) => format!("worker-{id}"),
    }
}

impl Shell {
    /// What the header says of King's view.
    pub(super) fn king_heading(&self) -> (&'static str, String, String) {
        let ui = &self.king_ui;
        let meta = match &ui.status {
            Some(status) => {
                let brain = &status.brain;
                let doing = if !brain.questions.is_empty() {
                    "asks you".to_owned()
                } else if brain.busy {
                    "working".to_owned()
                } else {
                    "listening".to_owned()
                };
                let workers = match status.workers.len() {
                    0 => "no workers".to_owned(),
                    1 => "1 worker".to_owned(),
                    many => format!("{many} workers"),
                };
                format!(
                    "{} · {doing} · {workers} · {}",
                    runs_on(brain),
                    spent(status)
                )
            }
            None if ui.connecting => "Reaching Orchestrator's service…".to_owned(),
            None => ui
                .error
                .clone()
                .unwrap_or_else(|| "Not connected".to_owned()),
        };
        ("Orchestrator", "The Orchestrator".to_owned(), meta)
    }

    /// How King stands, for the sidebar's row: its dot and how many workers
    /// it runs. `None` before the window has heard from it.
    pub(super) fn king_glance(&self) -> Option<(Activity, usize)> {
        let status = self.king_ui.status.as_ref()?;
        // What a worker asks is King's to answer, not the person's.
        let activity = if !status.brain.questions.is_empty() {
            Activity::NeedsYou
        } else if status.brain.busy
            || status
                .workers
                .iter()
                .any(|worker| matches!(worker.state, WorkerState::Working | WorkerState::Asking))
        {
            Activity::Working
        } else {
            Activity::Idle
        };
        Some((activity, status.workers.len()))
    }

    /// King's view, as the main pane's body.
    pub(super) fn render_king(&self, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.king_ui;
        let Some(status) = &ui.status else {
            return self.king_unreached(c, cx);
        };
        div()
            .debug_selector(|| "king".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .child(
                div()
                    .debug_selector(|| "king-brain".into())
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(match &ui.viewing {
                        Some((_, feed)) => self.king_timeline(Target::Brain, feed, c, cx),
                        None => self.king_timeline(Target::Brain, &ui.brain, c, cx),
                    })
                    .child(match &ui.viewing {
                        Some((past, _)) => column()
                            .flex_none()
                            .px(px(16.))
                            .pb(px(12.))
                            .child(self.king_past_bar(past, c, cx))
                            .into_any_element(),
                        None => column()
                            .flex_none()
                            .px(px(16.))
                            .pb(px(12.))
                            .gap(px(8.))
                            .children(
                                ui.note
                                    .as_ref()
                                    .map(|(title, body)| note(title, body, c, cx)),
                            )
                            .children(ui.voices.then(|| self.king_voices(c, cx)))
                            .children(self.king_questions(
                                Target::Brain,
                                &status.brain.questions,
                                c,
                                cx,
                            ))
                            // What the workers ask is here too, so it is
                            // seen without opening them (the open one asks
                            // in its own column).
                            .children(
                                status
                                    .workers
                                    .iter()
                                    .filter(|worker| ui.open != Some(worker.id))
                                    .flat_map(|worker| {
                                        self.king_questions(
                                            Target::Worker(worker.id),
                                            &worker.conversation.questions,
                                            c,
                                            cx,
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                            )
                            .child(self.king_composer(status, c, cx))
                            .into_any_element(),
                    }),
            )
            .children(
                ui.open
                    .and_then(|id| self.chat_diff_panel(Subject::Worker(id), c, cx)),
            )
            .child(
                div()
                    .debug_selector(|| "king-side".into())
                    .flex_none()
                    .w(px(WORKERS))
                    .h_full()
                    .min_h_0()
                    .border_l_1()
                    .border_color(c.border)
                    .bg(c.surface)
                    .flex()
                    .flex_col()
                    .children(self.king_running(status, c, cx))
                    .child(
                        div().flex_1().min_h_0().flex().flex_col().child(
                            match ui
                                .open
                                .and_then(|id| status.workers.iter().find(|worker| worker.id == id))
                            {
                                Some(worker) => self.king_worker(worker, c, cx),
                                None => match &ui.history {
                                    Some(list) => self.king_history(status, list, c, cx),
                                    None => self.king_cards(status, c, cx),
                                },
                            },
                        ),
                    )
                    // What is left of each agent's usage, as Leon's newest
                    // report on King's answers has it.
                    .children(
                        ui.brain
                            .latest_report()
                            .map(|report| usage_panel(report, c)),
                    ),
            )
            .into_any_element()
    }

    /// Before the service answered: reaching it, or why it could not be.
    fn king_unreached(&self, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.king_ui;
        let (title, line) = if ui.connecting {
            (
                "Reaching Orchestrator…",
                "Starting Orchestrator's service if it is not running. It keeps working with the window \
                 closed."
                    .to_owned(),
            )
        } else {
            (
                "Orchestrator is not connected",
                ui.error.clone().unwrap_or_else(|| {
                    "The window has not reached Orchestrator's service.".to_owned()
                }),
            )
        };
        div()
            .debug_selector(|| "king-unreached".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .px(px(24.))
            .child(mark(px(44.), c, cx, Lion::new("king-mark")))
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.text)
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(520.))
                    .text_color(c.text_muted)
                    .text_size(metrics::TEXT_SMALL())
                    .child(line),
            )
            .when(!ui.connecting && self.options.king.is_some(), |this| {
                this.child(
                    button("king-connect".into(), "CONNECT", Look::Primary, c).on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.king_connect(window, cx);
                            cx.notify();
                        }),
                    ),
                )
            })
            .into_any_element()
    }

    /// What a conversation said, oldest first, scrolling: drawn as the
    /// window's conversations are (`transcript.rs`), with King's report under
    /// an answer and its news of the workers where it came.
    fn king_timeline(
        &self,
        target: Target,
        feed: &Feed,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = key_of(target);
        let worker = matches!(target, Target::Worker(_));
        let items = &feed.conversation().items;
        let found = match target {
            Target::Brain => None,
            Target::Worker(id) => self
                .king_ui
                .status
                .as_ref()
                .and_then(|status| status.workers.iter().find(|worker| worker.id == id))
                .map(|worker| &worker.conversation),
        };
        let (since, busy, scroll, agent) = match target {
            Target::Brain => {
                let brain = self.king_ui.status.as_ref().map(|status| &status.brain);
                (
                    brain.and_then(|brain| brain.busy_since_ms),
                    brain.is_some_and(|brain| brain.busy),
                    &self.king_ui.scroll,
                    None,
                )
            }
            Target::Worker(_) => (
                found.and_then(|conversation| conversation.busy_since_ms),
                found.is_some_and(|conversation| conversation.busy),
                &self.king_ui.worker_scroll,
                found.and_then(|conversation| AgentId::parse(&conversation.agent)),
            ),
        };
        let speaker = |_: usize| Speaker {
            agent,
            label: agent.map_or_else(|| "ORCHESTRATOR".to_owned(), |agent| agent.tag().to_owned()),
        };
        let none = Folds::default();
        // King's own folder is the service's: only absolute paths count for
        // it. A worker's is where it works, when that is this computer.
        let scene = match target {
            Target::Brain => None,
            Target::Worker(id) => self.scene(Subject::Worker(id)),
        };
        let folder = match target {
            Target::Brain => Some(Folder { cwd: None }),
            Target::Worker(_) => scene.as_ref().map(|scene| Folder {
                cwd: Some(scene.cwd),
            }),
        };
        let transcript = Transcript {
            ids: Ids::new("king", key.clone()),
            surface: Surface::King(target),
            folds: self.king_ui.folds.get(&target).unwrap_or(&none),
            folder,
            speaker: &speaker,
            said_actions: None,
        };
        let mut list = column().px(px(16.)).pt(px(18.)).pb(px(12.)).gap(px(12.));
        if items.is_empty() && !busy {
            list = list.child(self.king_empty(worker, c, cx));
        }
        // A worker's turns that changed files each end with a card.
        let cards = match target {
            Target::Worker(id) => {
                match (
                    self.scene(Subject::Worker(id)),
                    self.edits(Subject::Worker(id)),
                ) {
                    (Some(scene), Some(edits)) => edits.cards(scene.items, scene.cwd, scene.going),
                    _ => Vec::new(),
                }
            }
            Target::Brain => Vec::new(),
        };
        for block in blocks(items, busy) {
            let end = match block {
                Block::Single(at) => at + 1,
                Block::Work { to, .. } => to,
            };
            list = list.child(match block {
                Block::Single(at) => match &items[at] {
                    // Leon's line under an answer: drawn by its facts.
                    Item::Notice { .. } if feed.report_at(at).is_some() => report_strip(
                        target,
                        at as u64,
                        feed.report_at(at).unwrap_or(&Report::default()),
                        c,
                    ),
                    // What Leon told King of its workers.
                    Item::Notice { text, .. } if text.starts_with("[Leon]") => {
                        self.king_news(target, at, text, c, cx)
                    }
                    item => self.transcript_item(&transcript, at, item, c, cx),
                },
                Block::Work { from, to, live } => {
                    self.transcript_work(&transcript, from, &items[from..to], live, c, cx)
                }
            });
            if let Target::Worker(id) = target {
                for (_, turn, stats) in cards.iter().filter(|(at, ..)| *at == end) {
                    list = list.child(self.chat_changed_card(
                        Subject::Worker(id),
                        *turn,
                        stats,
                        c,
                        cx,
                    ));
                }
            }
        }
        if busy {
            let now = (self.options.now)().timestamp_millis().max(0) as u64;
            list = list.child(status_line(
                &transcript.ids,
                None,
                if worker {
                    "WORKING"
                } else {
                    "ORCHESTRATOR IS WORKING"
                },
                since,
                now,
                c,
            ));
        }
        let latest = format!("king-latest-{key}");
        let scrolling = div()
            .id(SharedString::from(format!("king-scroll-{key}")))
            .debug_selector(move || format!("king-scroll-{key}"))
            .track_scroll(scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(list);
        with_latest(scrolling, scroll, latest, c, cx)
    }

    /// A conversation nothing was said in yet.
    fn king_empty(&self, worker: bool, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if worker {
            return div()
                .text_color(c.text_faint)
                .text_size(metrics::TEXT_SMALL())
                .child("Nothing said yet.")
                .into_any_element();
        }
        div()
            .debug_selector(|| "king-empty".into())
            .pt(px(48.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .child(mark(px(44.), c, cx, Lion::new("king-empty-mark")))
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.text)
                    .child("What should Orchestrator do?"),
            )
            .child(
                div()
                    .max_w(px(520.))
                    .text_center()
                    .text_color(c.text_muted)
                    .text_size(metrics::TEXT_SMALL())
                    .child(
                        "Orchestrator answers for every project. It reads to answer short questions, and \
                         gives the long work to other agents in worktrees of their own: the \
                         cheapest that can do it, the best ones for plans. It commits, pushes and \
                         opens pull requests; it never merges.",
                    ),
            )
            .into_any_element()
    }

    /// What Leon told King of its workers: its first line, and the rest
    /// when it is opened.
    fn king_news(
        &self,
        target: Target,
        at: usize,
        text: &str,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let seq = at as u64;
        let open = self.king_ui.open_news.contains(&(target, seq));
        let key = key_of(target);
        let hover = c.surface_2;
        let mut lines = text
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with("[Leon]"));
        let first = lines
            .next()
            .unwrap_or_default()
            .trim_start_matches("- ")
            .to_owned();
        let more = lines.count();
        div()
            .id(SharedString::from(format!("king-news-{key}-{seq}")))
            .debug_selector(move || format!("king-news-{key}-{seq}"))
            .w_full()
            .min_w_0()
            .px(px(6.))
            .py(px(4.))
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let at = (target, seq);
                if !this.king_ui.open_news.remove(&at) {
                    this.king_ui.open_news.insert(at);
                }
                cx.notify();
            }))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(icon(
                        if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        px(12.),
                        c.text_faint,
                    ))
                    .child(mono("NEWS OF THE WORKERS").text_color(c.info))
                    .when(!open, |this| {
                        this.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(c.text_muted)
                                .child(first.clone()),
                        )
                        .when(more > 0, |this| {
                            this.child(mono(format!("+{more}")).text_color(c.text_faint))
                        })
                    }),
            )
            .when(open, |this| {
                this.child(
                    div()
                        .pl(px(20.))
                        .min_w_0()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(c.text_muted)
                        .child(text.trim_start_matches("[Leon] ").to_owned()),
                )
            })
            .into_any_element()
    }

    /// The questions a conversation waits on, each with its answers.
    fn king_questions(
        &self,
        target: Target,
        questions: &[Open],
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let key = key_of(target);
        questions
            .iter()
            .map(|open| {
                let mut answers = div().flex().flex_wrap().items_center().gap(px(6.));
                // A worker's question is King's to answer: it is shown, and
                // the person answers it only by opening the worker.
                let deciding = match target {
                    Target::Worker(id) => (self.king_ui.open != Some(id)).then_some(id),
                    Target::Brain => None,
                };
                if let Some(id) = deciding {
                    answers = answers
                        .child(mono("ORCHESTRATOR DECIDES").text_color(c.text_faint))
                        .child(div().flex_1())
                        .child(
                            button(
                                format!("king-decide-{key}"),
                                "DECIDE MYSELF",
                                Look::Outline,
                                c,
                            )
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, window, cx| {
                                    this.king_open_worker(id, window, cx);
                                },
                            )),
                        );
                }
                // The question the keys answer says which key is which.
                let keyed = self
                    .king_question_in_view()
                    .is_some_and(|(on, first)| on == target && first.id == open.id);
                for (place, pickable) in picks(open)
                    .into_iter()
                    .enumerate()
                    .filter(|_| deciding.is_none())
                {
                    let n = place + 1;
                    let look = match pickable.tone {
                        Tone::Go => Look::Primary,
                        Tone::Plain => Look::Outline,
                        Tone::Stop => Look::Danger,
                    };
                    let label = if keyed && n <= 9 {
                        format!("{}  {}", pickable.label, alt_key(n))
                    } else {
                        pickable.label.to_owned()
                    };
                    let question = open.id.clone();
                    answers = answers.child(
                        button(format!("king-answer-{key}-{n}"), &label, look, c).on_click(
                            cx.listener(move |this, _: &ClickEvent, window, cx| {
                                match pickable.pick.clone() {
                                    Pick::Reply(reply) => {
                                        this.king_answer(target, question.clone(), reply, cx)
                                    }
                                    Pick::Words => this.king_answer_with_words(
                                        target,
                                        question.clone(),
                                        window,
                                        cx,
                                    ),
                                }
                            }),
                        ),
                    );
                }
                let ids = Ids::new("king", key.clone());
                let label =
                    match target {
                        Target::Brain => "ASKS YOU".to_owned(),
                        Target::Worker(id) => {
                            match self.king_ui.status.as_ref().and_then(|status| {
                                status.workers.iter().find(|worker| worker.id == id)
                            }) {
                                Some(worker) => {
                                    format!("WORKER #{id} ASKS · {}", worker.title).to_uppercase()
                                }
                                None => format!("WORKER #{id} ASKS"),
                            }
                        }
                    };
                let frame = question_frame(&ids, &open.id, &label, c);
                match leon_king::transcript::plan_of(&open.text) {
                    // A plan is read as a document, as the chat shows it.
                    Some(plan) => plan_card(frame, &ids, plan, c),
                    None => {
                        frame.child(div().min_w_0().text_color(c.text).child(open.text.clone()))
                    }
                }
                .child(answers)
                .into_any_element()
            })
            .collect()
    }

    /// The speech models dictation can use, to choose one or download it.
    fn king_voices(&self, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hover = c.surface_2;
        let mut list = div()
            .debug_selector(|| "king-voices".into())
            .w_full()
            .min_w_0()
            .px(px(12.))
            .py(px(8.))
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(c.border)
            .bg(c.surface)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pb(px(4.))
                    .child(
                        mono(self.voices_title(cx))
                            .flex_1()
                            .min_w_0()
                            .text_color(c.text_faint),
                    )
                    .child(
                        div()
                            .id("king-voices-close")
                            .debug_selector(|| "king-voices-close".into())
                            .flex_none()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(metrics::RADIUS())
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.king_ui.voices = false;
                                cx.notify();
                            }))
                            .child(icon(IconName::X, px(12.), c.text_faint)),
                    ),
            );
        for row in crate::chat::complete::voice_rows("", self.voices(cx)) {
            let crate::chat::complete::Pick::Voice(model) = row.pick.clone() else {
                continue;
            };
            let name = format!("king-voice-{model}");
            list = list.child(
                div()
                    .id(SharedString::from(name.clone()))
                    .debug_selector(move || name.clone())
                    .w_full()
                    .min_w_0()
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(metrics::RADIUS())
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .when(row.current, |this| this.bg(c.surface_2))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.dictation_choose(None, &model, window, cx);
                    }))
                    .child(div().flex_none().text_color(c.text).child(row.label))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(c.text_muted)
                            .child(row.detail.unwrap_or_default()),
                    )
                    .children(row.note.map(|note| mono(note).text_color(c.text_faint))),
            );
        }
        list.into_any_element()
    }

    /// Where the person speaks to King.
    fn king_composer(&self, status: &Status, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let brain = &status.brain;
        let action = if brain.busy {
            button("king-stop".into(), "STOP  ESC", Look::Outline, c).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    this.king_interrupt(Target::Brain, cx);
                },
            ))
        } else {
            button("king-send".into(), "SEND  \u{23ce}", Look::Primary, c).on_click(cx.listener(
                |this, _: &ClickEvent, window, cx| {
                    this.king_send(window, cx);
                },
            ))
        };
        let chip_hover = c.surface_2;
        let mut foot = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                // What King runs on; a click asks what it should run on.
                div()
                    .id("king-brain-chip")
                    .debug_selector(|| "king-brain-chip".into())
                    .flex_initial()
                    .min_w_0()
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(metrics::RADIUS())
                    .border_1()
                    .border_color(c.border)
                    .cursor_pointer()
                    .hover(move |style| style.bg(chip_hover))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.begin_flow(crate::keys::Command::KingBrain, window, cx);
                    }))
                    .child(icon(IconName::Crown, px(12.), c.text_faint))
                    .child(
                        mono(runs_on(brain))
                            .debug_selector(|| "king-brain-runs-on".into())
                            .flex_initial()
                            .min_w_0()
                            .truncate()
                            .text_color(c.text_muted),
                    ),
            );
        // How much King asks; a click asks how much it should.
        let mode = brain.mode.as_ref().map(|id| {
            self.king_ui
                .modes
                .iter()
                .find(|mode| mode.agent == brain.agent && &mode.id == id)
                .map_or_else(
                    || (id.to_uppercase(), false),
                    |mode| (mode.label.to_uppercase(), mode.dangerous),
                )
        });
        let (mode_label, dangerous) = mode.unwrap_or_else(|| ("DEFAULT MODE".to_owned(), false));
        foot = foot.child(
            div()
                .id("king-mode-chip")
                .debug_selector(|| "king-mode-chip".into())
                .flex_none()
                .h(px(22.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if dangerous { c.warning } else { c.border })
                .cursor_pointer()
                .hover(move |style| style.bg(chip_hover))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.begin_flow(crate::keys::Command::KingMode, window, cx);
                }))
                .child(mono(mode_label).text_color(if dangerous {
                    c.warning
                } else {
                    c.text_muted
                })),
        );
        foot = foot.child(
            div()
                .id("king-new")
                .debug_selector(|| "king-new".into())
                .flex_none()
                .h(px(22.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(metrics::RADIUS())
                .cursor_pointer()
                .hover(move |style| style.bg(chip_hover))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.king_new_conversation(cx);
                }))
                .child(icon(IconName::Plus, px(12.), c.text_faint))
                .child(mono("NEW").text_color(c.text_muted)),
        );
        if brain.queued > 0 {
            foot = foot.child(
                mono(format!("{} QUEUED", brain.queued))
                    .debug_selector(|| "king-queued".into())
                    .text_color(c.text_faint),
            );
        }
        div()
            .debug_selector(|| "king-composer".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(10.))
            .pt(px(8.))
            .pb(px(8.))
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(c.elevated_border)
            .bg(c.surface)
            .children(self.king_queue_list(Target::Brain, c, cx))
            .children(self.king_attached(Target::Brain, c, cx))
            .children(self.king_ui.input.as_ref().map(|input| {
                div()
                    .debug_selector(|| "king-input".into())
                    .w_full()
                    .min_w_0()
                    .child(Textarea::new(input).appearance(false).bordered(false))
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(foot)
                    .child(self.king_attach_button(Target::Brain, c, cx))
                    .child(self.dictate_button(
                        Field::King(Target::Brain),
                        (
                            "king-dictate-brain".to_owned(),
                            "king-dictate-problem-brain".to_owned(),
                        ),
                        c,
                        cx,
                    ))
                    .child(action),
            )
            .into_any_element()
    }

    /// The workers, as cards, newest first.
    /// The workers of the conversation on screen (the one now, or the past
    /// one being read), and, beside the one now, those of earlier
    /// conversations that still work or ask.
    fn king_shown_workers<'a>(
        &self,
        status: &'a Status,
    ) -> (Vec<&'a WorkerStatus>, Vec<&'a WorkerStatus>) {
        let viewed = self.king_ui.viewing.as_ref().map(|(past, _)| past.id);
        let on_screen = viewed.unwrap_or(status.conversation);
        let (mine, others): (Vec<&WorkerStatus>, Vec<&WorkerStatus>) = status
            .workers
            .iter()
            .partition(|worker| worker.started_in.is_none_or(|id| id == on_screen));
        let still = if viewed.is_some() {
            Vec::new()
        } else {
            others
                .into_iter()
                .filter(|worker| matches!(worker.state, WorkerState::Working | WorkerState::Asking))
                .collect()
        };
        (mine, still)
    }

    fn king_cards(&self, status: &Status, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (mine, earlier) = self.king_shown_workers(status);
        let mut cards = div()
            .w_full()
            .min_w_0()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.));
        if mine.is_empty() && self.king_ui.viewing.is_some() {
            cards = cards.child(
                div()
                    .debug_selector(|| "king-no-workers".into())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_faint)
                    .child("This conversation started no workers."),
            );
        } else if mine.is_empty() {
            cards = cards.child(
                div()
                    .debug_selector(|| "king-no-workers".into())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_faint)
                    .child(
                        "No workers. Orchestrator starts them for long work, each in a worktree of its \
                         own; they show here, not in the sidebar, until you take one over.",
                    ),
            );
        }
        for worker in mine.into_iter().rev() {
            cards = cards.child(self.king_card(worker, c, cx));
        }
        if !earlier.is_empty() {
            cards = cards.child(
                mono("STILL WORKING, FROM EARLIER CONVERSATIONS")
                    .debug_selector(|| "king-earlier-workers".into())
                    .pt(px(8.))
                    .text_color(c.text_faint),
            );
            for worker in earlier.into_iter().rev() {
                cards = cards.child(self.king_card(worker, c, cx));
            }
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.king_side_tabs(status, false, c, cx))
            .child(
                div()
                    .id("king-cards")
                    .debug_selector(|| "king-cards".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(cards),
            )
            .into_any_element()
    }

    /// What turns left running (a server, a watcher), over the right
    /// column, each with whose it is, since when, and the way to stop it.
    fn king_running(
        &self,
        status: &Status,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if status.running.is_empty() {
            return None;
        }
        let now = (self.options.now)().timestamp_millis().max(0) as u64;
        let mut rows = div()
            .debug_selector(|| "king-running".into())
            .flex_none()
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(led(c.success))
                    .child(
                        mono(format!("RUNNING {}", status.running.len())).text_color(c.text_muted),
                    ),
            );
        for process in &status.running {
            let pid = process.pid;
            let whose = match process.of {
                Target::Brain => "ORCHESTRATOR".to_owned(),
                Target::Worker(id) => format!("#{id}"),
            };
            rows = rows.child(
                div()
                    .debug_selector(move || format!("king-running-{pid}"))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(fonts::mono())
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(c.text)
                                    .child(process.command.clone()),
                            )
                            .child(
                                mono(format!(
                                    "{whose} · PID {pid} · {}",
                                    span((now.saturating_sub(process.since_ms) / 1000) as i64)
                                ))
                                .text_color(c.text_faint),
                            ),
                    )
                    .child(
                        button(format!("king-running-stop-{pid}"), "STOP", Look::Danger, c)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.king_stop_process(pid, cx);
                            })),
                    ),
            );
        }
        Some(rows.into_any_element())
    }

    /// The files attached to what is said next to `target`, over its field:
    /// an image as a thumbnail, any other file as its name, each with the
    /// button that takes it off.
    fn king_attached(
        &self,
        target: Target,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let files = self.king_ui.attached.get(&target)?;
        let key = key_of(target);
        attached_files(
            files,
            (
                format!("king-attached-{key}"),
                format!("king-attached-remove-{key}"),
            ),
            c,
            cx,
            move |this, gone, window, cx| {
                if let Some(files) = this.king_ui.attached.get_mut(&target) {
                    files.retain(|file| *file != gone);
                }
                this.king_focus_field(target, window, cx);
            },
        )
    }

    /// The button that attaches files to what is said to `target`.
    fn king_attach_button(
        &self,
        target: Target,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = format!("king-attach-{}", key_of(target));
        let hover = c.surface_2;
        div()
            .id(SharedString::from(name.clone()))
            .debug_selector(move || name.clone())
            .flex_none()
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(c.elevated_border)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.king_attach(target, window, cx);
            }))
            .child(icon(IconName::Paperclip, px(13.), c.text_muted))
            .into_any_element()
    }

    /// The head of the right column: the workers or the past
    /// conversations, and what was spent today.
    fn king_side_tabs(
        &self,
        status: &Status,
        history: bool,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = c.surface_2;
        let tab = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex_none()
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(metrics::RADIUS())
                .cursor_pointer()
                .when(on, |this| this.bg(c.surface_2))
                .hover(move |style| style.bg(hover))
                .child(mono(label).text_color(if on { c.text } else { c.text_muted }))
        };
        div()
            .flex_none()
            .h(px(36.))
            .px(px(8.))
            .border_b_1()
            .border_color(c.border)
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                tab(
                    "king-tab-workers",
                    format!("WORKERS {}", self.king_shown_workers(status).0.len()),
                    !history,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.king_toggle_history(false, cx);
                })),
            )
            .child(
                tab("king-tab-history", "HISTORY".to_owned(), history).on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        this.king_toggle_history(true, cx);
                    },
                )),
            )
            .child(div().flex_1())
            .child(
                mono(spent(status).to_uppercase())
                    .debug_selector(|| "king-spent".into())
                    .text_color(c.text_faint),
            )
            .into_any_element()
    }

    /// King's past conversations, newest first; one opened is read in the
    /// conversation's place.
    fn king_history(
        &self,
        status: &Status,
        list: &[Past],
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = c.surface_2;
        let viewing = self.king_ui.viewing.as_ref().map(|(past, _)| past.id);
        let mut rows = div()
            .w_full()
            .min_w_0()
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(2.));
        rows = rows.child(
            div().px(px(6.)).pb(px(6.)).child(
                button(
                    "king-new-conversation".into(),
                    "NEW CONVERSATION",
                    Look::Outline,
                    c,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.king_new_conversation(cx);
                })),
            ),
        );
        if list.is_empty() {
            rows = rows.child(
                div()
                    .debug_selector(|| "king-no-history".into())
                    .px(px(6.))
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_faint)
                    .child(
                        "No past conversations. Start a new one and the one now is kept here, to \
                         read or to take up again.",
                    ),
            );
        }
        let offset = super::shelf::local_offset();
        for past in list {
            let id = past.id;
            let when = chrono::DateTime::from_timestamp_millis(past.ended_ms as i64)
                .map(|at| {
                    at.with_timezone(&offset)
                        .format("%b %-d %H:%M")
                        .to_string()
                        .to_uppercase()
                })
                .unwrap_or_default();
            let runs = match &past.model {
                Some(model) => format!("{} {}", past.agent, model).to_uppercase(),
                None => past.agent.to_uppercase(),
            };
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("king-past-{id}")))
                    .debug_selector(move || format!("king-past-{id}"))
                    .w_full()
                    .min_w_0()
                    .px(px(8.))
                    .py(px(7.))
                    .rounded(metrics::RADIUS())
                    .cursor_pointer()
                    .when(viewing == Some(id), |this| this.bg(c.surface_2))
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.king_view_past(id, cx);
                    }))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .max_h(px(40.))
                            .overflow_hidden()
                            .text_color(c.text)
                            .child(past.title.clone()),
                    )
                    .child(
                        mono(format!("{when} · {runs} · {} ENTRIES", past.entries))
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_color(c.text_faint),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.king_side_tabs(status, true, c, cx))
            .child(
                div()
                    .id("king-history")
                    .debug_selector(|| "king-history".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(rows),
            )
            .into_any_element()
    }

    /// Under a past conversation on screen: what it is, taking it up again,
    /// and the way back to the one now.
    fn king_past_bar(&self, past: &Past, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "king-past-bar".into())
            .w_full()
            .min_w_0()
            .px(px(12.))
            .py(px(10.))
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(c.elevated_border)
            .bg(c.surface)
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(mono("PAST CONVERSATION").text_color(c.text_faint))
                    .child(
                        div()
                            .truncate()
                            .text_color(c.text)
                            .child(past.title.clone()),
                    ),
            )
            .child(
                button("king-back-to-now".into(), "BACK", Look::Outline, c)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.king_back_to_now(cx))),
            )
            .child(
                button("king-resume".into(), "TAKE IT UP AGAIN", Look::Primary, c)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.king_resume(cx))),
            )
            .into_any_element()
    }

    /// One worker's card.
    fn king_card(&self, worker: &WorkerStatus, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = worker.id;
        let hover = c.surface_2;
        let agent = AgentId::parse(&worker.conversation.agent);
        let mut where_ = vec![worker.job.as_str().to_uppercase()];
        where_.extend(worker.tier.iter().map(|tier| tier.to_uppercase()));
        where_.extend(worker.project.iter().map(|project| project.to_uppercase()));
        div()
            .id(SharedString::from(format!("king-card-{id}")))
            .debug_selector(move || format!("king-card-{id}"))
            .w_full()
            .min_w_0()
            .px(px(10.))
            .py(px(8.))
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(if worker.state == WorkerState::Asking {
                c.signal
            } else {
                c.border
            })
            .bg(c.background)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.king_open_worker(id, window, cx);
            }))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(activity_dot(worker_activity(worker.state), c))
                    .child(mono(format!("#{id}")).text_color(c.text_faint))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(c.text)
                            .child(worker.title.clone()),
                    )
                    .child(mono(state_word(worker.state)).text_color(c.text_muted)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .children(agent.map(|agent| agent_icon(agent, px(12.), c)))
                    .child(
                        mono(runs_on(&worker.conversation))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c.text_muted),
                    )
                    .child(
                        mono(format!("${:.2}", worker.conversation.cost_usd))
                            .text_color(c.text_faint),
                    ),
            )
            .child(
                mono(where_.join(" · "))
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_color(c.text_faint),
            )
            .children(self.king_evidence(worker, c, cx))
            .children(worker.last_answer.as_ref().map(|answer| {
                div()
                    .max_h(px(36.))
                    .overflow_hidden()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_muted)
                    .child(answer.clone())
            }))
            .into_any_element()
    }

    /// What shows a worker's work is right: its last check, and its pull
    /// request's CI, which opens on a click. A pull request King did not
    /// open (the worker did, or the person) is the one GitHub has for the
    /// worker's branch.
    fn king_evidence(
        &self,
        worker: &WorkerStatus,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = worker.id;
        let step = self.king_ui.status.as_ref().and_then(|status| {
            status.runs.iter().rev().find_map(|run| {
                run.step_of(id)
                    .map(|index| (run.id, run.place(index), run.state))
            })
        });
        let branch_pull_request = worker
            .pull_request
            .is_none()
            .then(|| {
                let machine = worker
                    .machine
                    .clone()
                    .map(leon_core::MachineId::from_string)
                    .unwrap_or_else(leon_core::MachineId::local);
                self.snapshot.pull_request_at(&machine, &worker.cwd)
            })
            .flatten();
        if worker.check.is_none()
            && worker.pull_request.is_none()
            && branch_pull_request.is_none()
            && step.is_none()
        {
            return None;
        }
        let now = (self.options.now)().timestamp_millis().max(0) as u64;
        let mut row = div()
            .debug_selector(move || format!("king-evidence-{id}"))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .min_w_0();
        if let Some((run, place, state)) = step {
            let colour = match state {
                RunState::Blocked => c.error,
                RunState::Done => c.success,
                RunState::Running | RunState::Canceled => c.text_muted,
            };
            row = row.child(
                mono(format!("RUN #{run} · {}", place.to_uppercase()))
                    .debug_selector(move || format!("king-step-{id}"))
                    .text_color(colour),
            );
        }
        if let Some(check) = &worker.check {
            let (text, colour) = match check.state {
                CheckState::Running => (
                    format!(
                        "{} CHECKING {}",
                        spinner_frame(),
                        verify::duration(now.saturating_sub(check.started_ms))
                    ),
                    c.text_muted,
                ),
                CheckState::Passed => (
                    format!(
                        "✓ CHECKED {} · {}",
                        verify::short(&check.commit),
                        verify::duration(check.duration_ms.unwrap_or_default())
                    ),
                    c.success,
                ),
                CheckState::Failed => (
                    format!("✗ CHECK FAILED {}", verify::short(&check.commit)),
                    c.error,
                ),
            };
            row = row.child(
                mono(text)
                    .debug_selector(move || format!("king-check-{id}"))
                    .text_color(colour),
            );
        }
        if let Some(pr) = &worker.pull_request {
            let (ci, colour) = match (pr.closed.as_deref(), pr.ci) {
                (Some(how), _) => (how.to_uppercase(), c.text_muted),
                (None, Ci::Pending) => ("CI RUNNING".to_owned(), c.text_muted),
                (None, Ci::Passed) => ("CI ✓".to_owned(), c.success),
                (None, Ci::Failed) => (format!("CI ✗ {}", pr.failing.join(", ")), c.error),
                (None, Ci::None) => ("NO CI".to_owned(), c.text_faint),
            };
            let url = pr.url.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("king-pr-{id}")))
                    .debug_selector(move || format!("king-pr-{id}"))
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        (this.options.open_url)(cx, &url);
                    }))
                    .child(mono(format!("PR {} · {ci}", pr.name())).text_color(colour)),
            );
        }
        if let Some(pull_request) = branch_pull_request {
            row = row.child(super::chat_view::pull_request_chip(
                format!("king-pr-{id}"),
                pull_request,
                c,
                cx,
            ));
        }
        Some(row.into_any_element())
    }

    /// An open card: the worker's conversation, live, and what can be done
    /// to it.
    fn king_worker(
        &self,
        worker: &WorkerStatus,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = worker.id;
        let target = Target::Worker(id);
        let feed = self.king_ui.workers.get(&id).cloned().unwrap_or_default();
        let busy = worker.conversation.busy;
        let mut actions = div().flex().flex_wrap().items_center().gap(px(6.));
        if busy {
            actions = actions.child(
                button(
                    format!("king-interrupt-{id}"),
                    "STOP TURN",
                    Look::Outline,
                    c,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.king_interrupt(target, cx);
                })),
            );
        }
        if !matches!(worker.state, WorkerState::Stopped) {
            actions = actions.child(
                button(format!("king-stop-{id}"), "STOP WORKER", Look::Danger, c).on_click(
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.king_stop_worker(id, cx);
                    }),
                ),
            );
        }
        if !busy {
            actions = actions.child(
                button(
                    format!("king-take-over-{id}"),
                    "TAKE OVER",
                    Look::Primary,
                    c,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.king_take_over(id, cx);
                })),
            );
        }
        let hover = c.surface_2;
        div()
            .debug_selector(move || format!("king-worker-{id}"))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(c.border)
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .min_w_0()
                            .child(
                                div()
                                    .id("king-back")
                                    .debug_selector(|| "king-back".into())
                                    .flex_none()
                                    .h(px(22.))
                                    .px(px(4.))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .rounded(metrics::RADIUS())
                                    .cursor_pointer()
                                    .hover(move |style| style.bg(hover))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.king_close_worker(window, cx);
                                    }))
                                    .child(mono("‹ WORKERS").text_color(c.text_muted)),
                            )
                            .child(div().flex_1())
                            .child(activity_dot(worker_activity(worker.state), c))
                            .child(mono(state_word(worker.state)).text_color(c.text_muted)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c.text)
                            .child(format!("#{id} {}", worker.title)),
                    )
                    .child(
                        mono(format!(
                            "{} · ${:.2}",
                            runs_on(&worker.conversation),
                            worker.conversation.cost_usd
                        ))
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .text_color(c.text_muted),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::mono())
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(c.text_faint)
                            .child(worker.cwd.clone()),
                    )
                    .child(actions),
            )
            .child(self.king_timeline(target, &feed, c, cx))
            .child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .pb(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(self.king_questions(target, &worker.conversation.questions, c, cx))
                    .children(self.king_attached(target, c, cx))
                    .children(self.chat_git_cell(Subject::Worker(id), c, cx))
                    .children(self.king_queue_list(target, c, cx))
                    .child(
                        div()
                            .debug_selector(move || format!("king-worker-composer-{id}"))
                            .w_full()
                            .min_w_0()
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(metrics::RADIUS())
                            .border_1()
                            .border_color(c.elevated_border)
                            .bg(c.background)
                            .flex()
                            .items_end()
                            .gap(px(8.))
                            .children(self.king_ui.worker_input.as_ref().map(|input| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Textarea::new(input).appearance(false).bordered(false))
                            }))
                            .child(self.king_attach_button(Target::Worker(id), c, cx))
                            .child(self.dictate_button(
                                Field::King(Target::Worker(id)),
                                (
                                    format!("king-dictate-worker-{id}"),
                                    format!("king-dictate-problem-worker-{id}"),
                                ),
                                c,
                                cx,
                            ))
                            .child(
                                button(format!("king-tell-{id}"), "TELL", Look::Outline, c)
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.king_tell_worker(window, cx);
                                    })),
                            ),
                    )
                    .children(self.king_foot(id, c, cx)),
            )
            .into_any_element()
    }

    /// The messages that wait for the turn of `target` to end, one row each
    /// above its field, as the chat shows them: what was said (its first
    /// line), and what can be done with it. Edit puts it back in the field,
    /// remove drops it, and send now tells the running turn instead of
    /// waiting for its end.
    fn king_queue_list(
        &self,
        target: Target,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let queue = self.king_queue(target);
        if queue.is_empty() {
            return None;
        }
        let key = key_of(target);
        let action = |part: &'static str, label: &'static str, index: usize| {
            let hover = c.surface_2;
            let name = format!("king-queue-{part}-{key}-{index}");
            div()
                .id(SharedString::from(name.clone()))
                .debug_selector(move || name.clone())
                .flex_none()
                .h(px(20.))
                .px(px(6.))
                .flex()
                .items_center()
                .rounded(metrics::RADIUS())
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(mono(label).text_color(c.text_muted))
        };
        let rows = queue.into_iter().enumerate().map(|(index, text)| {
            let (words, files) = super::king::split_attached(&text);
            let first = words
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or("")
                .to_owned();
            let shown = if files.is_empty() {
                first
            } else {
                format!("{first}  ({} attached)", files.len())
            };
            let item = format!("king-queue-item-{key}-{index}");
            let (for_edit, for_remove, for_steer) = (text.clone(), text.clone(), text);
            div()
                .debug_selector(move || item.clone())
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(mono(format!("{}", index + 1)).text_color(c.text_faint))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(c.text_muted)
                        .child(shown),
                )
                .child(action("edit", "EDIT", index).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        this.king_queue_edit(target, index, for_edit.clone(), cx);
                    },
                )))
                .child(action("remove", "REMOVE", index).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        this.king_queue_remove(target, index, for_remove.clone(), cx);
                    },
                )))
                .child(action("steer", "SEND NOW", index).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        this.king_queue_steer(target, index, for_steer.clone(), cx);
                    },
                )))
        });
        Some(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .pb(px(4.))
                .border_b_1()
                .border_color(c.border)
                .children(rows)
                .into_any_element(),
        )
    }

    /// The line under a worker's field: the branch of its worktree, and the
    /// buttons that open the diffs and the git cell. Only for a worker on
    /// this computer.
    fn king_foot(&self, id: u32, c: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let subject = Subject::Worker(id);
        self.scene(subject)?;
        let open = self
            .edits(subject)
            .is_some_and(|edits| edits.diff.is_some());
        let hover = c.surface_2;
        let diff = div()
            .id(SharedString::from(format!("king-diff-{id}")))
            .debug_selector(move || format!("king-diff-{id}"))
            .flex_none()
            .h(px(20.))
            .px(px(6.))
            .flex()
            .items_center()
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(if open { c.signal } else { c.border })
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.chat_diff_toggle(subject, cx);
            }))
            .child(mono("DIFF").text_color(c.text_muted));
        Some(
            div()
                .debug_selector(move || format!("king-foot-{id}"))
                .px(px(2.))
                .flex()
                .items_center()
                .gap(px(8.))
                .children(self.chat_branch_label(subject, c))
                .child(div().flex_1())
                .child(diff)
                .child(self.chat_git_button(subject, c, cx))
                .into_any_element(),
        )
    }
}

/// What King's service had for the person, over the field, until it is
/// put away or the person speaks.
fn note(title: &str, body: &str, c: &Palette, cx: &mut Context<Shell>) -> AnyElement {
    let hover = c.surface_2;
    div()
        .debug_selector(|| "king-note".into())
        .w_full()
        .min_w_0()
        .px(px(12.))
        .py(px(8.))
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(c.border)
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(led(c.info))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.text)
                        .child(title.to_owned()),
                )
                .child(
                    div()
                        .id("king-note-close")
                        .debug_selector(|| "king-note-close".into())
                        .flex_none()
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(metrics::RADIUS())
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.king_ui.note = None;
                            cx.notify();
                        }))
                        .child(icon(IconName::X, px(12.), c.text_faint)),
                ),
        )
        .when(!body.is_empty(), |this| {
            this.child(
                div()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_muted)
                    .child(body.to_owned()),
            )
        })
        .into_any_element()
}

/// A number of the report: the figure, then what it counts.
fn stat(value: String, unit: &'static str, c: &Palette) -> Div {
    div()
        .flex_none()
        .flex()
        .items_baseline()
        .gap(px(4.))
        .child(mono(value).text_color(c.text_muted))
        .child(mono(unit).text_color(c.text_faint))
}

/// The colour of what is left of a window: plenty, getting low, nearly out.
fn left_colour(left: f64, c: &Palette) -> gpui_kit::Hsla {
    if left > 0.5 {
        c.success
    } else if left > 0.2 {
        c.warning
    } else {
        c.error
    }
}

/// Leon's report under an answer, drawn: the turn's numbers on one line. One
/// of a turn that used no tokens is left out; what is left of the usage is in
/// the side column ([`usage_panel`]).
fn report_strip(target: Target, seq: u64, report: &Report, c: &Palette) -> AnyElement {
    let key = key_of(target);
    let Some(turn) = report.turn.filter(|turn| turn.total() > 0) else {
        return div().into_any_element();
    };
    let mut numbers = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(14.))
        .gap_y(px(4.))
        .child(stat(count(turn.total()), "TOKENS", c))
        .child(stat(count(turn.cached_input), "CACHED", c))
        .child(stat(count(turn.output), "WRITTEN", c));
    if let Some(rate) = turn.per_second() {
        numbers = numbers.child(stat(format!("{rate:.0}"), "TOK/S", c));
    }
    if turn.duration_ms > 0 {
        numbers = numbers.child(stat(span((turn.duration_ms / 1000) as i64), "", c));
    }
    if let Some(cost) = turn.cost_usd {
        numbers = numbers.child(mono(format!("${cost:.3}")).text_color(c.text));
    }
    div()
        .debug_selector(move || format!("king-report-{key}-{seq}"))
        .w_full()
        .min_w_0()
        .px(px(10.))
        .child(numbers)
        .into_any_element()
}

/// The foot of King's side column, from Leon's newest report: the day's
/// spending and tokens, and a bar for each usage window with what is left of
/// it and when it resets.
fn usage_panel(report: &Report, c: &Palette) -> AnyElement {
    let today = match report.budget_usd {
        Some(budget) => format!("${:.2} OF ${budget:.2}", report.spent_today_usd),
        None => format!("${:.2}", report.spent_today_usd),
    };
    let mut panel = div()
        .debug_selector(|| "king-usage".into())
        .flex_none()
        .w_full()
        .min_w_0()
        .px(px(14.))
        .py(px(10.))
        .border_t_1()
        .border_color(c.border)
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap(px(4.))
                .child(mono("TODAY").text_color(c.text_faint))
                .child(mono(today).text_color(c.text_muted))
                .child(
                    mono(format!(
                        "{} TOKENS",
                        count(report.king_today.total() + report.workers_today.total())
                    ))
                    .text_color(c.text_faint),
                ),
        );
    for agent in report
        .usage
        .iter()
        .filter(|agent| !agent.windows.is_empty())
    {
        let name = AgentId::parse(&agent.agent)
            .map_or_else(|| agent.agent.to_uppercase(), |id| id.tag().to_uppercase());
        let mut group = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(8.))
            .gap_y(px(2.))
            .child(mono(name).text_color(c.text_muted));
        for window in &agent.windows {
            let left = (1.0 - window.used).clamp(0.0, 1.0);
            group = group.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(mono(window.label.to_uppercase()).text_color(c.text_faint))
                    .child(
                        div()
                            .w(px(44.))
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(c.surface_2)
                            .child(
                                div()
                                    .h_full()
                                    .w(px(44. * left as f32))
                                    .rounded(px(2.))
                                    .bg(left_colour(left, c)),
                            ),
                    )
                    .child(mono(format!("{:.0}%", left * 100.0)).text_color(c.text_muted))
                    .children(window.resets_in_secs.map(|secs| {
                        mono(format!("\u{21bb}{}", span(secs))).text_color(c.text_faint)
                    })),
            );
        }
        panel = panel.child(group);
    }
    panel.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_says_what_it_runs_on() {
        let conversation = ConversationStatus {
            agent: "claude".into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            mode: None,
            session_id: None,
            busy: false,
            busy_since_ms: None,
            queued: 0,
            queue: Vec::new(),
            questions: Vec::new(),
            cost_usd: 0.0,
            last_seq: None,
            events_seq: None,
        };
        assert_eq!(runs_on(&conversation), "CLAUDE CODE OPUS HIGH");
        let unknown = ConversationStatus {
            agent: "newagent".into(),
            model: None,
            effort: None,
            ..conversation
        };
        assert_eq!(runs_on(&unknown), "NEWAGENT");
    }
}

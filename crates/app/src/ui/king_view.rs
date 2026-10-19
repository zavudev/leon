//! Drawing King's view: King's conversation on the left, its workers on the
//! right.
//!
//! The conversation is drawn as the window's own conversations are
//! (`chat_view.rs`): the person's words in a cell on the right, King's as
//! Markdown on the page, the tools between them folded into one line, a
//! question framed in the accent above the field, and Leon's report of each
//! answer in small mono under it. A worker's card, opened, draws its
//! conversation the same way in the right column, with what can be done to
//! the worker over it.

use gpui_kit::component::input::Textarea;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, Context, Div, FontWeight, SharedString, Stateful};
use leon_core::AgentId;
use leon_king::coordinator::{Open, Target, WorkerState};
use leon_king::history::Past;
use leon_king::protocol::{ConversationStatus, Status, WorkerStatus};
use leon_king::report::{count, span, Report};
use leon_king::transcript::{Entry, Kind};

use super::activity::Activity;
use super::chat_voice::Field;
use super::king::{picks, Pick, Tone};
use super::shell::Shell;
use super::widgets::{activity_dot, key_cap, led, mark, mono, Lion};
use crate::icons::{agent_icon, icon, IconName};
use crate::theme::{fonts, metrics, px, Palette};

/// The widest King's conversation gets.
const COLUMN: f32 = 760.;

/// How wide the column of the workers is.
const WORKERS: f32 = 380.;

/// The line height of an answer, in lines of its text: answers are long,
/// and read rather than glanced at.
const ANSWER_LINE: f32 = 1.65;

/// How an answer's Markdown is laid out: more room between paragraphs and
/// lists than a chat's, and headings that mark a section without shouting.
fn answer_style() -> TextViewStyle {
    TextViewStyle::default()
        .paragraph_gap(gpui_kit::rems(1.2))
        .heading_font_size(|level, base| match level {
            1 => base * 1.45,
            2 => base * 1.3,
            3 => base * 1.15,
            _ => base * 1.05,
        })
}

/// How many tools of a folded run are named under it when it is open.
const TOOLS_SHOWN: usize = 40;

/// A run of entries as they are drawn: one entry, or tools in a row.
enum Piece<'a> {
    One(&'a Entry),
    Tools(&'a [Entry]),
}

/// The entries, with each run of tools made one piece.
fn pieces(entries: &[Entry]) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut at = 0;
    while at < entries.len() {
        if entries[at].kind == Kind::Tool {
            let from = at;
            while at < entries.len() && entries[at].kind == Kind::Tool {
                at += 1;
            }
            pieces.push(Piece::Tools(&entries[from..at]));
        } else {
            pieces.push(Piece::One(&entries[at]));
            at += 1;
        }
    }
    pieces
}

/// The dot of a worker's card.
pub(super) fn worker_activity(state: WorkerState) -> Activity {
    match state {
        WorkerState::Working => Activity::Working,
        WorkerState::Asking => Activity::NeedsYou,
        WorkerState::Done => Activity::TurnOver,
        WorkerState::Failed => Activity::Failed,
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

/// The column every part of King's conversation sits in.
fn column() -> Div {
    div()
        .w_full()
        .max_w(px(COLUMN))
        .mx_auto()
        .min_w_0()
        .flex()
        .flex_col()
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
            None if ui.connecting => "Reaching King's service…".to_owned(),
            None => ui
                .error
                .clone()
                .unwrap_or_else(|| "Not connected".to_owned()),
        };
        ("King", "The King".to_owned(), meta)
    }

    /// How King stands, for the sidebar's row: its dot and how many workers
    /// it runs. `None` before the window has heard from it.
    pub(super) fn king_glance(&self) -> Option<(Activity, usize)> {
        let status = self.king_ui.status.as_ref()?;
        let asks = !status.brain.questions.is_empty()
            || status
                .workers
                .iter()
                .any(|worker| worker.state == WorkerState::Asking);
        let activity = if asks {
            Activity::NeedsYou
        } else if status.brain.busy
            || status
                .workers
                .iter()
                .any(|worker| worker.state == WorkerState::Working)
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
                        Some((_, entries)) => self.king_timeline(Target::Brain, entries, c, cx),
                        None => self.king_timeline(Target::Brain, &ui.brain.entries, c, cx),
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
                    .child(
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
            .into_any_element()
    }

    /// Before the service answered: reaching it, or why it could not be.
    fn king_unreached(&self, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.king_ui;
        let (title, line) = if ui.connecting {
            (
                "Reaching King…",
                "Starting King's service if it is not running. It keeps working with the window \
                 closed."
                    .to_owned(),
            )
        } else {
            (
                "King is not connected",
                ui.error
                    .clone()
                    .unwrap_or_else(|| "The window has not reached King's service.".to_owned()),
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

    /// What a conversation said, oldest first, scrolling.
    fn king_timeline(
        &self,
        target: Target,
        entries: &[Entry],
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = key_of(target);
        let worker = matches!(target, Target::Worker(_));
        let (scroll, busy, agent) = match target {
            Target::Brain => (
                &self.king_ui.scroll,
                self.king_ui
                    .status
                    .as_ref()
                    .is_some_and(|status| status.brain.busy),
                None,
            ),
            Target::Worker(id) => {
                let found = self
                    .king_ui
                    .status
                    .as_ref()
                    .and_then(|status| status.workers.iter().find(|worker| worker.id == id));
                (
                    &self.king_ui.worker_scroll,
                    found.is_some_and(|worker| worker.conversation.busy),
                    found.and_then(|worker| AgentId::parse(&worker.conversation.agent)),
                )
            }
        };
        let mut list = if worker {
            div().w_full().min_w_0().flex().flex_col()
        } else {
            column()
        }
        .px(px(16.))
        .pt(px(18.))
        .pb(px(12.))
        .gap(px(12.));
        if entries.is_empty() && !busy {
            list = list.child(self.king_empty(worker, c, cx));
        }
        for piece in pieces(entries) {
            list = list.child(match piece {
                Piece::One(entry) if entry.kind == Kind::News => {
                    self.king_news(target, entry, c, cx)
                }
                Piece::One(entry) => king_entry(target, entry, agent, c),
                Piece::Tools(run) => self.king_tools(target, run, c, cx),
            });
        }
        if busy {
            list = list.child(
                div()
                    .debug_selector({
                        let key = key.clone();
                        move || format!("king-busy-{key}")
                    })
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(activity_dot(Activity::Working, c))
                    .child(
                        mono(if worker { "WORKING" } else { "KING IS WORKING" })
                            .text_color(c.text_muted),
                    ),
            );
        }
        div()
            .id(SharedString::from(format!("king-scroll-{key}")))
            .debug_selector(move || format!("king-scroll-{key}"))
            .track_scroll(scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(list)
            .into_any_element()
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
                    .child("What should King do?"),
            )
            .child(
                div()
                    .max_w(px(520.))
                    .text_center()
                    .text_color(c.text_muted)
                    .text_size(metrics::TEXT_SMALL())
                    .child(
                        "King answers for every project. It reads to answer short questions, and \
                         gives the long work to other agents in worktrees of their own: the \
                         cheapest that can do it, the best ones for plans. It commits, pushes and \
                         opens pull requests; it never merges.",
                    ),
            )
            .into_any_element()
    }

    /// A run of tools: one line that says how many, and each of them when it
    /// is opened.
    fn king_tools(
        &self,
        target: Target,
        run: &[Entry],
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let first = run.first().map_or(0, |entry| entry.seq);
        let open = self.king_ui.open_tools.contains(&(target, first));
        let key = key_of(target);
        let last = run
            .last()
            .map(|entry| entry.text.clone())
            .unwrap_or_default();
        let hover = c.surface_2;
        let label = match run.len() {
            1 => "1 TOOL".to_owned(),
            many => format!("{many} TOOLS"),
        };
        let head = div()
            .id(SharedString::from(format!("king-tools-{key}-{first}")))
            .debug_selector({
                let key = key.clone();
                move || format!("king-tools-{key}-{first}")
            })
            .h(px(22.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let at = (target, first);
                if !this.king_ui.open_tools.remove(&at) {
                    this.king_ui.open_tools.insert(at);
                }
                cx.notify();
            }))
            .child(icon(
                if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                px(12.),
                c.text_faint,
            ))
            .child(mono(label).text_color(c.text_muted))
            .when(!open, |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::mono())
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(c.text_faint)
                        .child(last),
                )
            });
        let mut block = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(head);
        if open {
            let skip = run.len().saturating_sub(TOOLS_SHOWN);
            for entry in &run[skip..] {
                block = block.child(
                    div()
                        .pl(px(26.))
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::mono())
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(c.text_muted)
                        .child(entry.text.clone()),
                );
            }
        }
        block.into_any_element()
    }

    /// What Leon told King of its workers: its first line, and the rest
    /// when it is opened.
    fn king_news(
        &self,
        target: Target,
        entry: &Entry,
        c: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let seq = entry.seq;
        let open = self.king_ui.open_tools.contains(&(target, seq));
        let key = key_of(target);
        let hover = c.surface_2;
        let mut lines = entry
            .text
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
                if !this.king_ui.open_tools.remove(&at) {
                    this.king_ui.open_tools.insert(at);
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
                        .child(entry.text.trim_start_matches("[Leon] ").to_owned()),
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
                // The question the keys answer says which key is which.
                let keyed = self
                    .king_question_in_view()
                    .is_some_and(|(on, first)| on == target && first.id == open.id);
                for (place, pickable) in picks(open).into_iter().enumerate() {
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
                div()
                    .debug_selector({
                        let key = key.clone();
                        let id = open.id.clone();
                        move || format!("king-question-{key}-{id}")
                    })
                    .w_full()
                    .min_w_0()
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(metrics::RADIUS())
                    .border_1()
                    .border_color(c.signal)
                    .bg(c.surface)
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        mono(match target {
                            Target::Brain => "ASKS YOU".to_owned(),
                            Target::Worker(id) => {
                                match self.king_ui.status.as_ref().and_then(|status| {
                                    status.workers.iter().find(|worker| worker.id == id)
                                }) {
                                    Some(worker) => format!("WORKER #{id} ASKS · {}", worker.title)
                                        .to_uppercase(),
                                    None => format!("WORKER #{id} ASKS"),
                                }
                            }
                        })
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .text_color(c.signal),
                    )
                    .child(div().min_w_0().text_color(c.text).child(open.text.clone()))
                    .child(answers)
                    .into_any_element()
            })
            .collect()
    }

    /// Where the person speaks to King.
    fn king_composer(&self, status: &Status, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let brain = &status.brain;
        let action = if brain.busy {
            button("king-stop".into(), "STOP", Look::Outline, c).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    this.king_interrupt(Target::Brain, cx);
                },
            ))
        } else {
            button("king-send".into(), "SEND", Look::Primary, c).on_click(cx.listener(
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
                    .child(key_cap(if brain.busy { "ESC" } else { "ENTER" }, c))
                    .child(action),
            )
            .into_any_element()
    }

    /// The workers, as cards, newest first.
    fn king_cards(&self, status: &Status, c: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut cards = div()
            .w_full()
            .min_w_0()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.));
        if status.workers.is_empty() {
            cards = cards.child(
                div()
                    .debug_selector(|| "king-no-workers".into())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_faint)
                    .child(
                        "No workers. King starts them for long work, each in a worktree of its \
                         own; they show here, not in the sidebar, until you take one over.",
                    ),
            );
        }
        for worker in status.workers.iter().rev() {
            cards = cards.child(self.king_card(worker, c, cx));
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
                    format!("WORKERS {}", status.workers.len()),
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
        let entries = self
            .king_ui
            .workers
            .get(&id)
            .map(|feed| feed.entries.as_slice())
            .unwrap_or_default();
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
            .child(self.king_timeline(target, entries, c, cx))
            .child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .pb(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(self.king_questions(target, &worker.conversation.questions, c, cx))
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
                    ),
            )
            .into_any_element()
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

/// Leon's report under an answer, drawn: the turn's numbers on one line,
/// the day's beside them, and under them a bar for each usage window with
/// what is left of it and when it resets.
fn report_strip(target: Target, seq: u64, report: &Report, c: &Palette) -> AnyElement {
    let key = key_of(target);
    let mut numbers = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(14.))
        .gap_y(px(4.));
    if let Some(turn) = report.turn {
        numbers = numbers
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
    }
    let today = match report.budget_usd {
        Some(budget) => format!("${:.2} OF ${budget:.2}", report.spent_today_usd),
        None => format!("${:.2}", report.spent_today_usd),
    };
    numbers = numbers.child(div().flex_1()).child(
        div()
            .flex_none()
            .flex()
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
    let mut windows = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(16.))
        .gap_y(px(4.));
    for agent in report
        .usage
        .iter()
        .filter(|agent| !agent.windows.is_empty())
    {
        let name = AgentId::parse(&agent.agent)
            .map_or_else(|| agent.agent.to_uppercase(), |id| id.tag().to_uppercase());
        let mut group = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
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
        windows = windows.child(group);
    }
    div()
        .debug_selector(move || format!("king-report-{key}-{seq}"))
        .w_full()
        .min_w_0()
        .px(px(10.))
        .py(px(7.))
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(c.border)
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(numbers)
        .when(
            report.usage.iter().any(|agent| !agent.windows.is_empty()),
            |this| {
                this.child(div().w_full().h(px(1.)).bg(c.border))
                    .child(windows)
            },
        )
        .into_any_element()
}

/// One entry that is not a tool.
fn king_entry(target: Target, entry: &Entry, agent: Option<AgentId>, c: &Palette) -> AnyElement {
    let key = key_of(target);
    let seq = entry.seq;
    let selector = move |kind: &'static str| {
        let key = key.clone();
        move || format!("king-{kind}-{key}-{seq}")
    };
    match entry.kind {
        Kind::Prompt => div()
            .debug_selector(selector("prompt"))
            .w_full()
            .flex()
            .justify_end()
            .child(
                div()
                    .max_w(gpui_kit::relative(0.86))
                    .min_w_0()
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(metrics::RADIUS())
                    .border_1()
                    .border_color(c.border)
                    .bg(c.surface)
                    .text_color(c.text)
                    .child(entry.text.clone()),
            )
            .into_any_element(),
        Kind::Agent => div()
            .debug_selector(selector("agent"))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(match agent {
                        Some(agent) => agent_icon(agent, px(12.), c).into_any_element(),
                        None => icon(IconName::Crown, px(12.), c.signal).into_any_element(),
                    })
                    .child(
                        mono(match agent {
                            Some(agent) => agent.tag().to_owned(),
                            None => "KING".to_owned(),
                        })
                        .text_color(c.text_faint),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_color(c.text)
                    .line_height(gpui_kit::relative(ANSWER_LINE))
                    .child(
                        TextView::markdown(
                            SharedString::from(format!("king-{}-message-{seq}", key_of(target))),
                            entry.text.clone(),
                        )
                        .style(answer_style())
                        .selectable(true),
                    ),
            )
            .into_any_element(),
        Kind::Question => div()
            .debug_selector(selector("asked"))
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(led(c.signal))
            .child(
                div()
                    .min_w_0()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_muted)
                    .child(entry.text.clone()),
            )
            .into_any_element(),
        Kind::Notice => div()
            .debug_selector(selector("notice"))
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(led(c.text_faint))
            .child(
                div()
                    .min_w_0()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_muted)
                    .child(entry.text.clone()),
            )
            .into_any_element(),
        // Leon's report under an answer: tokens, speed, cost, what is left.
        Kind::Report if entry.report.is_some() => report_strip(
            target,
            seq,
            entry.report.as_ref().unwrap_or(&Report::default()),
            c,
        ),
        Kind::Report => div()
            .debug_selector(selector("report"))
            .w_full()
            .min_w_0()
            .pl(px(18.))
            .border_l_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .children(entry.text.lines().map(|line| {
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(fonts::mono())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(c.text_faint)
                    .child(line.to_owned())
            }))
            .into_any_element(),
        // Drawn by `king_tools` and `king_news`.
        Kind::Tool | Kind::News => div().into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seq: u64, kind: Kind) -> Entry {
        Entry {
            seq,
            at_ms: 0,
            kind,
            text: String::new(),
            report: None,
        }
    }

    #[test]
    fn tools_in_a_row_are_one_piece() {
        let entries = [
            entry(0, Kind::Prompt),
            entry(1, Kind::Tool),
            entry(2, Kind::Tool),
            entry(3, Kind::Agent),
            entry(4, Kind::Tool),
        ];
        let shape: Vec<usize> = pieces(&entries)
            .iter()
            .map(|piece| match piece {
                Piece::One(_) => 1,
                Piece::Tools(run) => run.len() * 10,
            })
            .collect();
        assert_eq!(shape, [1, 20, 1, 10]);
    }

    #[test]
    fn a_conversation_says_what_it_runs_on() {
        let conversation = ConversationStatus {
            agent: "claude".into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            mode: None,
            session_id: None,
            busy: false,
            queued: 0,
            questions: Vec::new(),
            cost_usd: 0.0,
            last_seq: None,
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

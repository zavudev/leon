//! Drawing what a conversation said: one view for the chat, King and its
//! workers.
//!
//! Every conversation is the same data (`leon_conversation::Conversation`),
//! so every one is drawn by the same code: the person's words as a command
//! typed at a prompt, the agent's as Markdown on the page with its mark and
//! the time it said them, its list of things to do as a card, the work
//! between them as one folded line (each edit standing out of it with its
//! diff), and the plan it proposes as a card of its own.
//!
//! What belongs to one surface only is given to the view as small hooks, not
//! drawn here: the chat's rewind over the person's words, King's report under
//! an answer and its news of the workers. Each surface loops over
//! [`blocks`](crate::chat::model::blocks) itself, so it can put its own
//! things between the blocks, and asks [`Shell::transcript_item`] and
//! [`Shell::transcript_work`] for the rest.

use std::collections::HashSet;

use gpui_kit::component::text::TextView;
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, Context, Div, FontWeight, SharedString};
use leon_core::AgentId;
use leon_king::coordinator::Target;
use leon_turn::{ChangeKind, DiffLine, FileChange, LineKind, TodoStatus};

use super::live::LiveId;
use super::shell::Shell;
use super::widgets::{led, mono};
use crate::chat::model::{
    diff_rows, segments, todo_title, work_label, Item, Row, Segment, Tone, Tool,
};
use crate::icons::{icon, IconName};
use crate::theme::{fonts, metrics, px, Palette, DIFF_TINT};

/// How many lines of a diff are drawn before the rest is counted.
const DIFF_LINES: usize = 200;

/// Which conversation of the window a transcript draws: what its folds
/// belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Surface {
    /// A chat of a pane.
    Chat(LiveId),
    /// King's own conversation, or a worker's.
    King(Target),
}

/// What the person opened or folded by hand in one conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folds {
    /// The stretches of work, by the item they begin with: open while they
    /// happen, folded after, unless toggled.
    pub work: HashSet<usize>,
    /// The tools, by id: a tool shows its detail when toggled, an edit
    /// shows its diff until toggled.
    pub tools: HashSet<String>,
}

/// Who says an answer: its mark, and the words under it.
pub(super) struct Speaker {
    /// The agent whose mark it is; `None` is King's own (a crown).
    pub agent: Option<AgentId>,
    /// What is written next to the mark.
    pub label: String,
}

/// The names that tell the parts of one transcript from the parts of another
/// on screen: `chat-agent-3-0`, `king-agent-worker-2-0`.
#[derive(Debug, Clone)]
pub(super) struct Ids {
    prefix: &'static str,
    key: String,
}

impl Ids {
    /// The names of the transcript `key` of the surface called `prefix`.
    pub fn new(prefix: &'static str, key: impl Into<String>) -> Self {
        Self {
            prefix,
            key: key.into(),
        }
    }

    /// The name of a kind of part: `chat-todos-3`.
    pub fn of(&self, kind: &str) -> String {
        format!("{}-{kind}-{}", self.prefix, self.key)
    }

    /// The name of one part of a kind: `chat-agent-3-0`.
    pub fn at(&self, kind: &str, at: impl std::fmt::Display) -> String {
        format!("{}-{kind}-{}-{at}", self.prefix, self.key)
    }

    /// The surface's prefix.
    pub fn prefix(&self) -> &'static str {
        self.prefix
    }

    /// The conversation's key.
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// Where a conversation works, when it works on this computer: the files its
/// messages name are looked for there.
#[derive(Debug, Clone, Copy)]
pub(super) struct Folder<'a> {
    /// What a relative path starts from; none where the conversation has no
    /// folder of its own (only absolute paths and `~` count).
    pub cwd: Option<&'a str>,
}

/// What a surface gives the view to draw one conversation.
pub(super) struct Transcript<'a> {
    /// The names of its parts.
    pub ids: Ids,
    /// What its folds belong to.
    pub surface: Surface,
    /// What the person opened or folded by hand.
    pub folds: &'a Folds,
    /// Where it works, to make the files its messages name links; `None` for
    /// a conversation on another machine, whose paths are not this
    /// computer's.
    pub folder: Option<Folder<'a>>,
    /// Who said the answer at an index.
    pub speaker: &'a dyn Fn(usize) -> Speaker,
    /// What goes on the person's words at an index, besides the words: the
    /// chat's rewind. It gets the name of the hover group of the words.
    #[allow(clippy::type_complexity)]
    pub said_actions:
        Option<&'a dyn Fn(&Shell, usize, SharedString, &mut Context<Shell>) -> Option<AnyElement>>,
}

impl Shell {
    /// The Markdown of an agent's message as it is drawn: normalised, and
    /// with the files it names that exist as links. What is still being
    /// written is not looked at: its words are not whole.
    fn transcript_markdown(
        &self,
        t: &Transcript,
        text: &str,
        streaming: bool,
        cx: &mut Context<Self>,
    ) -> String {
        let normal = super::chat_code::normalize(text).into_owned();
        let Some(folder) = t.folder.filter(|_| !streaming) else {
            return normal;
        };
        let home = self.options.system.home_dir().map(std::path::PathBuf::from);
        self.links.linked(&normal, folder.cwd, home.as_deref(), cx)
    }

    /// The folds of a conversation, to change them.
    fn folds_of(&mut self, surface: Surface) -> Option<&mut Folds> {
        match surface {
            Surface::Chat(id) => self.chats.get_mut(&id).map(|chat| &mut chat.folds),
            Surface::King(target) => Some(self.king_ui.folds.entry(target).or_default()),
        }
    }

    /// Opens or folds a stretch of work.
    pub(super) fn transcript_toggle_work(
        &mut self,
        surface: Surface,
        from: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some(folds) = self.folds_of(surface) {
            if !folds.work.remove(&from) {
                folds.work.insert(from);
            }
            cx.notify();
        }
    }

    /// Shows or hides the detail of a tool.
    pub(super) fn transcript_toggle_tool(
        &mut self,
        surface: Surface,
        tool: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(folds) = self.folds_of(surface) {
            if !folds.tools.remove(tool) {
                folds.tools.insert(tool.to_owned());
            }
            cx.notify();
        }
    }

    /// A message, a list of todos or a note: one item that is not work.
    pub(super) fn transcript_item(
        &self,
        t: &Transcript,
        at: usize,
        item: &Item,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ids = &t.ids;
        match item {
            // What the person said reads as a command typed at a prompt:
            // the accent's `>` and a bar of it down the left.
            Item::User { text, at: said_at } => {
                let (words, files) = super::king::split_attached(text);
                let group = SharedString::from(ids.at("said", at));
                div()
                    .debug_selector({
                        let name = ids.at("user", at);
                        move || name.clone()
                    })
                    .group(group.clone())
                    .w_full()
                    .min_w_0()
                    .flex()
                    .gap(px(10.))
                    .py(px(2.))
                    .pl(px(12.))
                    .border_l_2()
                    .border_color(colours.signal)
                    .font_family(fonts::mono())
                    .font_features(fonts::mono_features())
                    .child(div().flex_none().text_color(colours.signal).child(">"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .text_color(colours.text)
                            // Selected and copied like the agent's answers;
                            // the list of paths the agent reads is not shown
                            // as text: the files are below.
                            .child(gpui_kit::base::SelectableText::new(
                                SharedString::from(ids.at("said-text", at)),
                                words,
                            ))
                            .children(self.transcript_files(ids, at, &files, colours, cx)),
                    )
                    .children(self.transcript_time(ids, at, *said_at, colours))
                    .children(
                        t.said_actions
                            .and_then(|actions| actions(self, at, group, cx)),
                    )
                    .into_any_element()
            }
            Item::Agent {
                text,
                at: said_at,
                streaming,
            } => {
                let who = (t.speaker)(at);
                let shown = self.transcript_markdown(t, text, *streaming, cx);
                let palette = *colours;
                let code = ids.at("code", at);
                div()
                    .debug_selector({
                        let name = ids.at("agent", at);
                        move || name.clone()
                    })
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
                            .child(agent_mark(who.agent, colours))
                            .child(mono(who.label).text_color(colours.text_faint))
                            .child(div().flex_1())
                            .children(self.transcript_time(ids, at, *said_at, colours)),
                    )
                    .child(
                        div().w_full().min_w_0().text_color(colours.text).child(
                            TextView::markdown(
                                SharedString::from(ids.at("message", at)),
                                SharedString::from(shown),
                            )
                            .selectable(true)
                            .on_link_click(super::links::on_link(cx, t.folder.is_some()))
                            .code_block_actions(move |block, _, _| {
                                super::chat_code::corner(
                                    code.clone(),
                                    block.lang().as_deref(),
                                    block.code(),
                                    &palette,
                                )
                            }),
                        ),
                    )
                    .into_any_element()
            }
            Item::Todos(todos) => self.transcript_todos(ids, at, todos, colours),
            Item::Notice { tone, text } => div()
                .debug_selector({
                    let name = ids.at("notice", at);
                    move || name.clone()
                })
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(led(match tone {
                    Tone::Plain => colours.text_faint,
                    Tone::Bad => colours.error,
                }))
                .child(
                    div()
                        .min_w_0()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(match tone {
                            Tone::Plain => colours.text_muted,
                            Tone::Bad => colours.error,
                        })
                        .child(text.clone()),
                )
                .into_any_element(),
            // A rule with the switch said on it: what is above is the first
            // agent's, what is below the second's.
            Item::Switched { from, to } => {
                let rule = || div().flex_1().h(px(1.)).bg(colours.border);
                div()
                    .debug_selector({
                        let name = ids.at("switch", at);
                        move || name.clone()
                    })
                    .w_full()
                    .py(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(rule())
                    .child(
                        mono(switched_said(*from, *to))
                            .flex_none()
                            .text_color(colours.text_faint),
                    )
                    .child(rule())
                    .into_any_element()
            }
            // Work is drawn by `transcript_work`.
            Item::Thought { .. } | Item::Tool(_) => div().into_any_element(),
        }
    }

    /// When a message was said, small and muted, at the end of its line:
    /// the time of day, and the date when it was not today.
    fn transcript_time(
        &self,
        ids: &Ids,
        at: usize,
        said: Option<i64>,
        colours: &Palette,
    ) -> Option<AnyElement> {
        let said = said?;
        let words = crate::chat::stamp::label(
            said,
            self.now().timestamp(),
            crate::chat::stamp::local_offset(said),
        );
        (!words.is_empty()).then(|| {
            let name = ids.at("time", at);
            div()
                .debug_selector(move || name.clone())
                .flex_none()
                .font_family(fonts::mono())
                .font_features(fonts::mono_features())
                .text_size(metrics::TEXT_LABEL())
                .text_color(colours.text_faint)
                .child(words)
                .into_any_element()
        })
    }

    /// The agent's list of things to do, as a compact card: a mark for how
    /// far each one is, the one it is on in the accent, the done ones struck
    /// and muted, and how many are done in the title.
    fn transcript_todos(
        &self,
        ids: &Ids,
        at: usize,
        todos: &[leon_turn::Todo],
        colours: &Palette,
    ) -> AnyElement {
        let row = |n: usize, todo: &leon_turn::Todo| {
            let (sign, text) = match todo.status {
                TodoStatus::Completed => (
                    icon(IconName::Check, px(12.), colours.success).into_any_element(),
                    colours.text_faint,
                ),
                TodoStatus::InProgress => (
                    mono(super::king_view::spinner_frame())
                        .text_color(colours.signal)
                        .into_any_element(),
                    colours.text,
                ),
                TodoStatus::Pending => (
                    icon(IconName::Circle, px(12.), colours.text_faint).into_any_element(),
                    colours.text_muted,
                ),
            };
            let name = format!("{}-{n}", ids.at("todo", at));
            div()
                .debug_selector(move || name.clone())
                .flex()
                .items_start()
                .gap(px(8.))
                .child(
                    div()
                        .flex_none()
                        .w(px(14.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(sign),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(text)
                        .when(todo.status == TodoStatus::InProgress, |this| {
                            this.font_weight(FontWeight::SEMIBOLD)
                        })
                        .when(todo.status == TodoStatus::Completed, |this| {
                            this.line_through()
                        })
                        .child(todo.text.clone()),
                )
        };
        let (card, title) = (ids.at("todos", at), ids.at("todos-title", at));
        div()
            .debug_selector(move || card.clone())
            .w_full()
            .min_w_0()
            .p(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(colours.border)
            .bg(colours.surface)
            .child(
                mono(todo_title(todos).to_uppercase())
                    .debug_selector(move || title.clone())
                    .text_color(colours.text_muted),
            )
            .children(todos.iter().enumerate().map(|(n, todo)| row(n, todo)))
            .into_any_element()
    }

    /// A stretch of work, as the terminal draws it: what is not an edit
    /// folds into a muted line that says what it did, and every edit stands
    /// out of it with its diff. `items` are the stretch, that begins at the
    /// index `from` of the conversation.
    pub(super) fn transcript_work(
        &self,
        t: &Transcript,
        from: usize,
        items: &[Item],
        live: bool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let parts = segments(items);
        let last = parts.len().saturating_sub(1);
        let mut out = div().w_full().min_w_0().flex().flex_col().gap(px(2.));
        for (n, part) in parts.into_iter().enumerate() {
            out = out.child(match part {
                Segment::Edit(at) => match &items[at] {
                    Item::Tool(tool) => self.transcript_edit(t, tool, colours, cx),
                    _ => div().into_any_element(),
                },
                Segment::Run { from: a, to: b } => {
                    self.transcript_run(t, from + a, &items[a..b], live && n == last, colours, cx)
                }
            });
        }
        out.into_any_element()
    }

    /// Thoughts and tools that change no file: one line that says what they
    /// did, and under it, when it is open, each of them.
    fn transcript_run(
        &self,
        t: &Transcript,
        from: usize,
        items: &[Item],
        live: bool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ids = &t.ids;
        let surface = t.surface;
        // Open while it happens; folded once the conversation moved on,
        // unless it was opened by hand (and the other way round while live).
        let toggled = t.folds.work.contains(&from);
        let open = live != toggled;
        let last = items.iter().rev().find_map(|item| match item {
            Item::Tool(tool) => Some(format!("{} {}", tool.name, tool.summary)),
            _ => None,
        });
        let hover = colours.surface_2;
        let tools = || {
            items.iter().filter_map(|item| match item {
                Item::Tool(tool) => Some(tool),
                _ => None,
            })
        };
        let run_light = if tools().any(|tool| matches!(&tool.outcome, Some(o) if !o.ok)) {
            colours.error
        } else if tools().any(|tool| tool.outcome.is_none()) && live {
            colours.signal
        } else {
            colours.text_faint
        };
        let name = ids.at("work", from);
        let head = div()
            .id(SharedString::from(name.clone()))
            .debug_selector(move || name.clone())
            .h(px(24.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.transcript_toggle_work(surface, from, cx);
            }))
            .child(icon(
                if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                px(12.),
                colours.text_faint,
            ))
            .child(led(run_light))
            .child(mono(work_label(items)).text_color(colours.text_muted))
            .when(!open, |this| {
                this.children(last.map(|last| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::mono())
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_faint)
                        .child(last)
                }))
            });
        // Open, the work is a framed block of the drawing: its corners are
        // marked, in the accent while it happens.
        let ticks = format!("{}-work", ids.prefix());
        let mut body = div()
            .relative()
            .px(px(12.))
            .py(px(8.))
            .border_1()
            .border_color(colours.border)
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(if live {
                super::lines::focus_ticks(&ticks, colours)
            } else {
                super::lines::frame_ticks(&ticks, colours)
            });
        if open {
            for (offset, item) in items.iter().enumerate() {
                body = body.child(match item {
                    Item::Tool(tool) => self.transcript_tool(t, tool, colours, cx),
                    Item::Thought { text } => {
                        let name = ids.at("thought", from + offset);
                        div()
                            .debug_selector(move || name.clone())
                            .py(px(4.))
                            .min_w_0()
                            .italic()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(text.trim().to_owned())
                            .into_any_element()
                    }
                    _ => div().into_any_element(),
                });
            }
        }
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(head)
            .when(open, |this| this.child(body))
            .into_any_element()
    }

    /// One tool: its light, its name and what with; under it, when asked,
    /// everything it was given and everything that came back.
    fn transcript_tool(
        &self,
        t: &Transcript,
        tool: &Tool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let surface = t.surface;
        let open = t.folds.tools.contains(&tool.id);
        let light = match &tool.outcome {
            None => colours.signal,
            Some(outcome) if outcome.ok => colours.success,
            Some(_) => colours.error,
        };
        let name = t.ids.at("tool", &tool.id);
        let key = tool.id.clone();
        let hover = colours.surface_2;
        let result = tool
            .outcome
            .as_ref()
            .map(|outcome| outcome.summary.clone())
            .filter(|summary| !summary.is_empty());
        let detail = [
            tool.detail.as_str(),
            tool.outcome
                .as_ref()
                .map_or("", |outcome| outcome.detail.as_str()),
        ]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
        let has_detail = !detail.is_empty();
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(name.clone()))
                    .debug_selector(move || name.clone())
                    .h(px(24.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(metrics::RADIUS())
                    .when(has_detail, |this| {
                        this.cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.transcript_toggle_tool(surface, &key, cx);
                            }))
                    })
                    .child(led(light))
                    .child(mono(tool.name.to_uppercase()).text_color(colours.text))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::mono())
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(tool.summary.clone()),
                    )
                    .children(result.map(|result| {
                        div()
                            .flex_none()
                            .max_w(px(260.))
                            .truncate()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_faint)
                            .child(result)
                    })),
            )
            .when(open && has_detail, |this| {
                this.child(code_cell(&detail, colours).ml(px(14.)).my(px(4.)))
            })
            .into_any_element()
    }

    /// A tool that changes files, as the terminal draws it: for each file a
    /// line of what was done to it and how much, and under it, open unless
    /// it was folded by hand, the diff.
    fn transcript_edit(
        &self,
        t: &Transcript,
        tool: &Tool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let surface = t.surface;
        let open = !t.folds.tools.contains(&tool.id);
        let light = match &tool.outcome {
            None => colours.signal,
            Some(outcome) if outcome.ok => colours.success,
            Some(_) => colours.error,
        };
        let hover = colours.surface_2;
        let failure = tool
            .outcome
            .as_ref()
            .filter(|outcome| !outcome.ok)
            .map(|outcome| {
                if outcome.detail.trim().is_empty() {
                    outcome.summary.clone()
                } else {
                    outcome.detail.clone()
                }
            })
            .filter(|said| !said.trim().is_empty());
        let mut out = div().w_full().min_w_0().flex().flex_col();
        for (at, change) in tool.changes.iter().enumerate() {
            let key = tool.id.clone();
            let name = if at == 0 {
                t.ids.at("tool", &tool.id)
            } else {
                format!("{}-{at}", t.ids.at("tool", &tool.id))
            };
            out = out
                .child(
                    div()
                        .id(SharedString::from(name.clone()))
                        .debug_selector(move || name.clone())
                        .h(px(24.))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(metrics::RADIUS())
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.transcript_toggle_tool(surface, &key, cx);
                        }))
                        .child(led(light))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(fonts::mono())
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(colours.text_muted)
                                .child(format!("{} ", change_verb(change.kind)))
                                .child(
                                    div()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colours.text)
                                        .child(change.path.clone()),
                                )
                                .child(
                                    div()
                                        .text_color(colours.text_faint)
                                        .child(format!(" ({})", change_counts(change))),
                                ),
                        ),
                )
                .when(open, |this| {
                    this.child(diff_block(&t.ids, &tool.id, at, change, colours))
                });
        }
        out.when_some(failure, |this, said| {
            this.child(code_cell(&said, colours).ml(px(14.)).my(px(4.)))
        })
        .into_any_element()
    }

    /// The files that went with a message: a picture as a thumbnail, any
    /// other file as a chip with its name. Each opens in the system's viewer
    /// when clicked.
    fn transcript_files(
        &self,
        ids: &Ids,
        at: usize,
        files: &[std::path::PathBuf],
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if files.is_empty() {
            return None;
        }
        let tiles = files.iter().enumerate().map(|(place, file)| {
            let name = format!("{}-{place}", ids.at("sent-image", at));
            if super::widgets::is_image(file) {
                return super::widgets::thumbnail(file, px(96.), name, cx);
            }
            let name = format!("{}-{place}", ids.at("sent-file", at));
            let opened = file.clone();
            let hover = colours.surface_2;
            div()
                .id(SharedString::from(name.clone()))
                .debug_selector(move || name.clone())
                .flex_none()
                .max_w(px(220.))
                .h(px(28.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.border)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(icon(IconName::File, px(12.), colours.text_muted))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(
                            file.file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| file.display().to_string()),
                        ),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let open = this.options.open_file.clone();
                    open(cx, &opened);
                }))
                .into_any_element()
        });
        Some(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(tiles)
                .into_any_element(),
        )
    }
}

/// The frame a question is asked in: in the accent, above the field, with
/// what it is in its title.
pub(super) fn question_frame(ids: &Ids, asked: &str, label: &str, colours: &Palette) -> Div {
    let name = ids.at("question", asked);
    div()
        .debug_selector(move || name.clone())
        .w_full()
        .min_w_0()
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(colours.signal)
        .bg(colours.surface)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(led(colours.signal))
                .child(mono(format!("[ {label} ]")).text_color(colours.signal)),
        )
}

/// The plan an agent proposes, in the frame of a question: the plan itself
/// sits in a page of its own (as long as it needs, up to a scroll), and what
/// it says can be copied. What answers it is the surface's, under it.
pub(super) fn plan_card(frame: Div, ids: &Ids, plan: &str, colours: &Palette) -> Div {
    let palette = *colours;
    let copied = plan.to_owned();
    let hover = colours.surface_2;
    let (card, page, code, copy) = (
        ids.of("plan-card"),
        ids.of("plan-page"),
        ids.of("plan-code"),
        ids.of("plan-copy"),
    );
    frame
        .debug_selector(move || card.clone())
        .child(
            div()
                .id(SharedString::from(ids.of("plan")))
                .debug_selector(move || page.clone())
                .max_h(px(360.))
                .overflow_y_scroll()
                .min_w_0()
                .px(px(12.))
                .py(px(8.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.border)
                .bg(colours.background)
                .child(
                    TextView::markdown(
                        SharedString::from(format!("{}-{}-plan", ids.prefix(), ids.key())),
                        SharedString::from(super::chat_code::normalize(plan).into_owned()),
                    )
                    .selectable(true)
                    .code_block_actions(move |block, _, _| {
                        super::chat_code::corner(
                            code.clone(),
                            block.lang().as_deref(),
                            block.code(),
                            &palette,
                        )
                    }),
                ),
        )
        .child(
            div().flex().child(
                div()
                    .id(SharedString::from(copy.clone()))
                    .debug_selector(move || copy.clone())
                    .h(px(20.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded(metrics::RADIUS())
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .child(mono("COPY PLAN").text_color(colours.text_muted))
                    .on_click(move |_: &ClickEvent, _, cx| {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copied.clone()));
                    }),
            ),
        )
}

/// What the rule of a switch of agents says.
pub(super) fn switched_said(from: AgentId, to: AgentId) -> String {
    format!(
        "Switched from {} to {} \u{b7} context handed off",
        crate::format::agent_name(from),
        crate::format::agent_name(to)
    )
}

/// The mark of who says an answer: the agent's, or King's crown.
fn agent_mark(agent: Option<AgentId>, colours: &Palette) -> AnyElement {
    match agent {
        Some(agent) => crate::icons::agent_icon(agent, px(12.), colours).into_any_element(),
        None => icon(IconName::Crown, px(12.), colours.signal).into_any_element(),
    }
}

/// What was done to a file, as a verb.
fn change_verb(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Created => "Created",
        ChangeKind::Updated => "Updated",
        ChangeKind::Deleted => "Deleted",
    }
}

/// How many lines came and went, as the header says it: "+3 -1", "+12".
fn change_counts(change: &FileChange) -> String {
    match change.kind {
        ChangeKind::Created => format!("+{}", change.added()),
        ChangeKind::Deleted => format!("-{}", change.removed()),
        ChangeKind::Updated => format!("+{} -{}", change.added(), change.removed()),
    }
}

/// The lines of the change of one file as a diff: a gutter with the number
/// of each line where the agent said it (the new file's for kept and added
/// lines, the old's for removed ones), the `+` or `-`, and the text; the
/// added lines on the success tint and the removed ones on the error's.
fn diff_block(ids: &Ids, tool: &str, at: usize, change: &FileChange, colours: &Palette) -> Div {
    let width = change
        .lines()
        .flat_map(|line| [line.old, line.new])
        .flatten()
        .max()
        .map_or(0, |most| most.to_string().len());
    let number = |line: &DiffLine| match line.kind {
        LineKind::Removed => line.old,
        _ => line.new,
    };
    let (rows, hidden) = diff_rows(change, DIFF_LINES);
    let mut lines = div().flex().flex_col();
    for row in rows {
        let Row::Line(line) = row else {
            lines = lines.child(
                div()
                    .px(px(8.))
                    .text_color(colours.text_faint)
                    .child("\u{22ef}"),
            );
            continue;
        };
        let (sign_colour, band, sign) = match line.kind {
            LineKind::Added => (
                colours.success,
                Some(colours.success.opacity(DIFF_TINT)),
                "+",
            ),
            LineKind::Removed => (colours.error, Some(colours.error.opacity(DIFF_TINT)), "-"),
            LineKind::Context => (colours.text_faint, None, " "),
        };
        lines = lines.child(
            div()
                .flex()
                .min_h(px(16.))
                .px(px(8.))
                .gap(px(8.))
                .text_color(colours.text)
                .when_some(band, |row, band| row.bg(band))
                .when(width > 0, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .w(px(width as f32 * 8.))
                            .flex()
                            .justify_end()
                            .text_color(colours.text_faint)
                            .child(number(line).map(|n| n.to_string()).unwrap_or_default()),
                    )
                })
                .child(div().flex_none().text_color(sign_colour).child(sign))
                .child(div().flex_1().min_w_0().child(if line.text.is_empty() {
                    " ".to_owned()
                } else {
                    line.text.clone()
                })),
        );
    }
    let selector = if at == 0 {
        ids.at("diff", tool)
    } else {
        format!("{}-{at}", ids.at("diff", tool))
    };
    div()
        .debug_selector(move || selector.clone())
        .min_w_0()
        .ml(px(14.))
        .my(px(4.))
        .py(px(4.))
        .rounded(metrics::RADIUS())
        .bg(colours.surface_2)
        .overflow_hidden()
        .font_family(fonts::mono())
        .font_features(fonts::mono_features())
        .text_size(metrics::TEXT_SMALL())
        .child(lines)
        .when(hidden > 0, |this| {
            this.child(
                div()
                    .px(px(8.))
                    .pt(px(2.))
                    .text_color(colours.text_faint)
                    .child(format!("\u{2026} {hidden} more lines")),
            )
        })
}

/// Text as the agent wrote it, in a cell: mono, on the raised surface, as
/// tall as it needs up to a few lines and scrolling beyond.
pub(super) fn code_cell(text: &str, colours: &Palette) -> Div {
    let mut lines = div().flex().flex_col();
    for line in text.lines() {
        let colour = if line.starts_with("+ ") {
            colours.success
        } else if line.starts_with("- ") {
            colours.error
        } else {
            colours.text_muted
        };
        lines = lines.child(
            div()
                .min_h(px(16.))
                .text_color(colour)
                .child(if line.is_empty() {
                    " ".to_owned()
                } else {
                    line.to_owned()
                }),
        );
    }
    div()
        .min_w_0()
        .max_h(px(240.))
        .overflow_hidden()
        .px(px(10.))
        .py(px(8.))
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(colours.border)
        .bg(colours.surface_2)
        .font_family(fonts::mono())
        .font_features(fonts::mono_features())
        .text_size(metrics::TEXT_SMALL())
        .child(lines)
}

/// The column every part of a conversation sits in: the whole width of the
/// pane, so code and tables keep their lines on any screen.
pub(super) fn column() -> Div {
    div().w_full().min_w_0().flex().flex_col()
}

/// What the agent is doing, under the last thing it did, while a turn is in
/// flight: a spinner that turns while it works (or a light, when it waits for
/// the person), what it is, and how long the turn has gone on (`now` and
/// `since` in milliseconds since the epoch).
pub(super) fn status_line(
    ids: &Ids,
    light: Option<gpui_kit::Hsla>,
    words: &str,
    since: Option<u64>,
    now: u64,
    colours: &Palette,
) -> AnyElement {
    let sign = match light {
        Some(light) => led(light).into_any_element(),
        None => {
            let name = ids.of("spinner");
            mono(super::king_view::spinner_frame())
                .debug_selector(move || name.clone())
                .text_color(colours.signal)
                .into_any_element()
        }
    };
    let name = ids.of("status");
    div()
        .debug_selector(move || name.clone())
        .flex()
        .items_center()
        .gap(px(8.))
        .child(sign)
        .child(mono(words.to_owned()).text_color(colours.text_muted))
        .children(since.map(|since| {
            mono(leon_king::report::span(
                (now.saturating_sub(since) / 1000) as i64,
            ))
            .text_color(colours.text_faint)
        }))
        .into_any_element()
}

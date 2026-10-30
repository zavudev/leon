//! Drawing a conversation: one interface for every agent.
//!
//! The leaf is a column on the page's centre line: what was said, oldest
//! first, then what the agent asks, then the field. Nothing here knows which
//! agent it draws beyond its name and its mark, because the conversation is
//! the unified protocol's data (`crate::chat::model`).
//!
//! The look is the brand's: the page's own background, hairlines instead of
//! bubbles, mono capitals for what is a label, and the accent kept for the
//! one thing to do next (send, or the answer the agent waits for).
//!
//! * the person's words sit in a cell on the right;
//! * the agent's are Markdown on the page itself;
//! * the work between them (thoughts and tools) is one folded line, open
//!   while it happens and whenever it is clicked;
//! * a question is a framed cell above the field, in the accent.

use gpui_kit::component::input::Textarea;
use gpui_kit::component::text::TextView;
use gpui_kit::prelude::*;
use gpui_kit::{
    div, AnyElement, ClickEvent, Context, Div, FontWeight, MouseDownEvent, SharedString, Stateful,
};
use leon_turn::{
    ChangeKind, ChoiceQuestion, DiffLine, FileChange, LineKind, PermissionChoice, PermissionEffect,
    QuestionKind,
};

use super::chat::Chat;
use super::chat_find::Mark;
use super::chat_menu::MenuKind;
use super::chat_rewind::Files;
use super::live::LiveId;
use super::shell::Shell;
use super::widgets::{attached_files, key_cap, led, mark, mono, with_latest, Lion};
use crate::chat::model::{
    blocks, cost_text, diff_rows, duration_text, segments, todo_title, work_label, Block, Item,
    Level, Phase, Question, Row, Segment, Tone, Tool,
};
use crate::format;
use crate::icons::{agent_icon, icon, IconName};
use crate::theme::{fonts, metrics, px, Palette, DIFF_TINT};

impl Shell {
    /// The leaf of a conversation, with the accent outline when it is the
    /// focused one of several.
    pub(super) fn render_chat_pane(
        &self,
        id: LiveId,
        marked: bool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let chat = self.chats.get(&id)?;
        let body = if chat.conversation.items.is_empty() {
            self.chat_empty(id, chat, colours, cx)
        } else {
            self.chat_timeline(id, chat, colours, cx)
        };
        let questions = chat
            .conversation
            .questions
            .iter()
            .map(|question| self.chat_question(id, chat, question, colours, cx))
            .collect::<Vec<_>>();
        let panel = self.chat_diff_panel(id, chat, colours, cx);
        let find = self.chat_find_bar(id, chat, colours, cx);
        let column_of_chat = div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .flex()
            .flex_col()
            .child(body)
            .children(find)
            .child(
                column()
                    .flex_none()
                    .px(px(16.))
                    .pb(px(12.))
                    .gap(px(8.))
                    .children(questions)
                    .children(self.chat_rewind_cell(id, chat, colours, cx))
                    .children(self.chat_git_cell(id, chat, colours, cx))
                    .children(self.chat_menu(id, chat, colours, cx))
                    .child(self.chat_composer(id, chat, colours, cx))
                    .child(chat_foot(
                        id,
                        chat,
                        [
                            self.chat_branch_label(id, chat, colours),
                            self.chat_pull_request(id, chat, colours, cx),
                            Some(self.chat_git_button(id, chat, colours, cx)),
                        ],
                        colours,
                    )),
            );
        Some(
            div()
                .debug_selector(move || format!("pane-{}", id.0))
                .relative()
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_row()
                .bg(colours.background)
                .rounded(metrics::RADIUS_CELL())
                .border_1()
                .border_color(if marked {
                    colours.signal
                } else {
                    gpui_kit::transparent_black()
                })
                // A click anywhere in it makes it the focused pane of its tab.
                .capture_any_mouse_down(cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    if this.focused_chat() != Some(id) {
                        this.pane = super::shell::Pane::Main;
                        this.open_live(id, window, cx);
                    }
                }))
                .child(column_of_chat)
                .children(panel)
                .into_any_element(),
        )
    }

    /// The open pull request of the branch a conversation works on, whose
    /// number opens it; `None` when GitHub knows of none for its folder.
    fn chat_pull_request(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pull_request = self.snapshot.pull_request_at(&chat.machine, &chat.cwd)?;
        Some(pull_request_chip(
            format!("chat-pr-{}", id.0),
            pull_request,
            colours,
            cx,
        ))
    }

    /// A conversation nothing was said in yet: the lion, and what can be
    /// asked of whom, where.
    fn chat_empty(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .debug_selector(move || format!("chat-empty-{}", id.0))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .child(mark(px(44.), colours, cx, Lion::new("chat-empty")))
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colours.text)
                    .child(format!(
                        "What should {} do?",
                        format::agent_name(chat.agent)
                    )),
            )
            .child(
                div()
                    .min_w_0()
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(agent_icon(chat.agent, px(14.), colours))
                    .child(mono(format::agent_tag(chat.agent)).text_color(colours.text_muted))
                    .child(mono("IN").text_color(colours.text_faint))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::mono())
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(folder_name(&chat.cwd)),
                    ),
            )
            .into_any_element()
    }

    /// What was said and done, oldest first, scrolling.
    fn chat_timeline(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = &chat.conversation.items;
        let running = chat.conversation.running();
        // The list is the scrolling element itself, so that each of its
        // children can be scrolled to (the first one is the space above it,
        // and the gap after the last is the space below).
        let mut list = vec![div().flex_none().h(px(4.)).into_any_element()];
        let cards = chat.cards();
        let found = chat.found();
        let marked =
            super::chat_find::marks(&found, chat.find.as_ref().map_or(0, |bar| bar.current));
        for block in blocks(items, running) {
            let end = match block {
                Block::Single(at) => at + 1,
                Block::Work { to, .. } => to,
            };
            let mark = match block {
                Block::Single(at) => marked.get(&at).copied(),
                Block::Work { .. } => None,
            };
            let drawn = match block {
                Block::Single(at) => self.chat_item(id, at, &items[at], chat, colours, cx),
                Block::Work { from, to, live } => {
                    self.chat_work(id, chat, from, &items[from..to], live, colours, cx)
                }
            };
            list.push(
                div()
                    .flex_none()
                    .w_full()
                    .min_w_0()
                    .rounded(metrics::RADIUS())
                    .when_some(mark, |this, mark| {
                        this.bg(colours.signal.opacity(match mark {
                            Mark::Current => 0.2,
                            Mark::Some => 0.07,
                        }))
                    })
                    .child(drawn)
                    .into_any_element(),
            );
            // The turn that ends here changed files: its card closes it.
            for (_, turn, stats) in cards.iter().filter(|(at, ..)| *at == end) {
                list.push(
                    div()
                        .flex_none()
                        .w_full()
                        .child(self.chat_changed_card(id, chat, *turn, stats, colours, cx))
                        .into_any_element(),
                );
            }
        }
        let now = (self.options.now)().timestamp_millis().max(0) as u64;
        if let Some(status) = chat_status(id, chat, now, colours) {
            list.push(div().flex_none().child(status).into_any_element());
        }
        list.push(div().flex_none().h(px(0.)).into_any_element());
        let scrolling = column()
            .id(SharedString::from(format!("chat-scroll-{}", id.0)))
            .debug_selector(move || format!("chat-scroll-{}", id.0))
            .track_scroll(&chat.scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .px(px(16.))
            .gap(px(12.))
            .children(list);
        with_latest(
            scrolling,
            &chat.scroll,
            format!("chat-latest-{}", id.0),
            colours,
            cx,
        )
    }

    /// A message or a notice.
    fn chat_item(
        &self,
        id: LiveId,
        at: usize,
        item: &Item,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match item {
            // What the person said reads as a command typed at a prompt:
            // the accent's `>` and a bar of it down the left.
            Item::User { text, at: said_at } => {
                let (words, files) = super::king::split_attached(text);
                let group = SharedString::from(format!("chat-said-{}-{at}", id.0));
                div()
                    .debug_selector(move || format!("chat-user-{}-{at}", id.0))
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
                                SharedString::from(format!("chat-{}-said-{at}", id.0)),
                                words,
                            ))
                            .children(self.chat_sent_files(id, at, &files, colours, cx)),
                    )
                    .children(self.chat_time(id, at, *said_at, colours))
                    .children(
                        chat.can_rewind(at)
                            .then(|| self.chat_said_actions(id, at, group, colours, cx)),
                    )
                    .into_any_element()
            }
            Item::Agent {
                text, at: said_at, ..
            } => {
                let (lines, palette) = (id.0, *colours);
                let who = crate::chat::model::agent_at(&chat.conversation.items, at, chat.agent);
                div()
                    .debug_selector(move || format!("chat-agent-{}-{at}", id.0))
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
                            .child(agent_icon(who, px(12.), colours))
                            .child(
                                mono(if who == chat.agent {
                                    answered_by(chat)
                                } else {
                                    // Said by the agent before a switch: the
                                    // model in use now is not its.
                                    crate::chat::complete::answered_by(None, format::agent_tag(who))
                                })
                                .text_color(colours.text_faint),
                            )
                            .child(div().flex_1())
                            .children(self.chat_time(id, at, *said_at, colours)),
                    )
                    .child(
                        div().w_full().min_w_0().text_color(colours.text).child(
                            TextView::markdown(
                                SharedString::from(format!("chat-{}-message-{at}", id.0)),
                                SharedString::from(super::chat_code::normalize(text).into_owned()),
                            )
                            .selectable(true)
                            .code_block_actions(move |block, _, _| {
                                super::chat_code::corner(
                                    format!("chat-code-{lines}-{at}"),
                                    block.lang().as_deref(),
                                    block.code(),
                                    &palette,
                                )
                            }),
                        ),
                    )
                    .into_any_element()
            }
            Item::Todos(todos) => self.chat_todos(id, at, todos, colours),
            Item::Notice { tone, text } => div()
                .debug_selector(move || format!("chat-notice-{}-{at}", id.0))
                .w_full()
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
                    .debug_selector(move || format!("chat-switch-{}-{at}", id.0))
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
            // Work is drawn by `chat_work`.
            Item::Thought { .. } | Item::Tool(_) => div().into_any_element(),
        }
    }

    /// When a message was said, small and muted, at the end of its line:
    /// the time of day, and the date when it was not today.
    fn chat_time(
        &self,
        id: LiveId,
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
            div()
                .debug_selector(move || format!("chat-time-{}-{at}", id.0))
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
    fn chat_todos(
        &self,
        id: LiveId,
        at: usize,
        todos: &[leon_turn::Todo],
        colours: &Palette,
    ) -> AnyElement {
        use leon_turn::TodoStatus;
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
            div()
                .debug_selector(move || format!("chat-todo-{}-{at}-{n}", id.0))
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
        div()
            .debug_selector(move || format!("chat-todos-{}-{at}", id.0))
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
                    .debug_selector(move || format!("chat-todos-title-{}-{at}", id.0))
                    .text_color(colours.text_muted),
            )
            .children(todos.iter().enumerate().map(|(n, todo)| row(n, todo)))
            .into_any_element()
    }

    /// A stretch of work, as the terminal draws it: what is not an edit
    /// folds into a muted line that says what it did, and every edit stands
    /// out of it with its diff.
    #[allow(clippy::too_many_arguments)]
    fn chat_work(
        &self,
        id: LiveId,
        chat: &Chat,
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
                    Item::Tool(tool) => self.chat_edit(id, chat, tool, colours, cx),
                    _ => div().into_any_element(),
                },
                Segment::Run { from: a, to: b } => self.chat_run(
                    id,
                    chat,
                    from + a,
                    &items[a..b],
                    live && n == last,
                    colours,
                    cx,
                ),
            });
        }
        out.into_any_element()
    }

    /// Thoughts and tools that change no file: one line that says what they
    /// did, and under it, when it is open, each of them.
    #[allow(clippy::too_many_arguments)]
    fn chat_run(
        &self,
        id: LiveId,
        chat: &Chat,
        from: usize,
        items: &[Item],
        live: bool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Open while it happens; folded once the conversation moved on,
        // unless it was opened by hand (and the other way round while live).
        let toggled = chat.open_work.contains(&from);
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
        let head = div()
            .id(SharedString::from(format!("chat-work-{}-{from}", id.0)))
            .debug_selector(move || format!("chat-work-{}-{from}", id.0))
            .h(px(24.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.chat_toggle_work(id, from, cx);
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
                super::lines::focus_ticks("chat-work", colours)
            } else {
                super::lines::frame_ticks("chat-work", colours)
            });
        if open {
            for (offset, item) in items.iter().enumerate() {
                body = body.child(match item {
                    Item::Tool(tool) => self.chat_tool(id, chat, tool, colours, cx),
                    Item::Thought { text } => div()
                        .debug_selector(move || format!("chat-thought-{}-{}", id.0, from + offset))
                        .py(px(4.))
                        .min_w_0()
                        .italic()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(text.trim().to_owned())
                        .into_any_element(),
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
    fn chat_tool(
        &self,
        id: LiveId,
        chat: &Chat,
        tool: &Tool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = chat.open_tools.contains(&tool.id);
        let light = match &tool.outcome {
            None => colours.signal,
            Some(outcome) if outcome.ok => colours.success,
            Some(_) => colours.error,
        };
        let name = tool.id.clone();
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
                    .id(SharedString::from(format!("chat-tool-{}-{name}", id.0)))
                    .debug_selector(move || format!("chat-tool-{}-{name}", id.0))
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
                                this.chat_toggle_tool(id, &key, cx);
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
    fn chat_edit(
        &self,
        id: LiveId,
        chat: &Chat,
        tool: &Tool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = !chat.open_tools.contains(&tool.id);
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
                format!("chat-tool-{}-{}", id.0, tool.id)
            } else {
                format!("chat-tool-{}-{}-{at}", id.0, tool.id)
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
                            this.chat_toggle_tool(id, &key, cx);
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
                    this.child(diff_block(&tool.id, at, id, change, colours))
                });
        }
        out.when_some(failure, |this, said| {
            this.child(code_cell(&said, colours).ml(px(14.)).my(px(4.)))
        })
        .into_any_element()
    }

    /// What the agent asks, with the ways to answer it.
    fn chat_question(
        &self,
        id: LiveId,
        chat: &Chat,
        question: &Question,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let asked = question.id.clone();
        let frame = |label: &str| {
            let name = asked.clone();
            div()
                .debug_selector(move || format!("chat-question-{}-{name}", id.0))
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
        };
        match &question.kind {
            QuestionKind::Permission {
                tool,
                summary,
                detail,
                reason,
                choices,
            } => {
                let takes_reason = choices.iter().any(|choice| choice.takes_reason);
                // The tool it asks about is the one waiting in the
                // conversation: that one says the change as lines that go and
                // come, where the question only has the tool's arguments.
                let waiting = chat
                    .conversation
                    .items
                    .iter()
                    .rev()
                    .find_map(|item| match item {
                        Item::Tool(waiting)
                            if waiting.outcome.is_none()
                                && &waiting.name == tool
                                && !waiting.detail.trim().is_empty() =>
                        {
                            Some(waiting.detail.as_str())
                        }
                        _ => None,
                    });
                let detail = waiting.unwrap_or(detail.as_str());
                let mut answers = div().flex().flex_wrap().items_center().gap(px(8.));
                for choice in choices {
                    answers = answers.child(self.chat_choice(id, &asked, choice, colours, cx));
                }
                // Enter in an empty field is the accented answer.
                let first = chat.conversation.questions.first().map(|first| &first.id);
                if first == Some(&asked)
                    && choices
                        .iter()
                        .any(|choice| choice.effect == PermissionEffect::AllowOnce)
                {
                    answers = answers
                        .child(div().flex_1())
                        .child(key_cap("ENTER", colours))
                        .child(mono("ALLOWS ONCE").text_color(colours.text_faint));
                }
                frame("PERMISSION")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .min_w_0()
                            .child(mono(tool.to_uppercase()).text_color(colours.text))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(fonts::mono())
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(colours.text_muted)
                                    .child(summary.clone()),
                            ),
                    )
                    .when(!detail.trim().is_empty(), |this| {
                        this.child(code_cell(detail, colours))
                    })
                    .children(reason.clone().map(|reason| {
                        div()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(reason)
                    }))
                    .when(takes_reason, |this| {
                        this.child(reason_field(&chat.reason, colours))
                    })
                    .child(answers)
                    .into_any_element()
            }
            QuestionKind::Choices { questions } => {
                let alone = questions.len() == 1 && !questions[0].multi_select;
                let complete = questions.iter().all(|of| {
                    chat.picks
                        .get(&(asked.clone(), of.id.clone()))
                        .is_some_and(|picked| !picked.is_empty())
                });
                let mut cell = frame("QUESTION");
                for of in questions {
                    cell = cell.child(self.chat_options(id, chat, &asked, of, colours, cx));
                }
                let send = asked.clone();
                cell.when(!alone, |this| {
                    this.child(
                        div().flex().child(
                            button("chat-answer", id, "ANSWER", Look::Primary, colours)
                                .when(!complete, |this| this.opacity(0.4))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.chat_submit_picks(id, &send, cx);
                                })),
                        ),
                    )
                })
                .into_any_element()
            }
            QuestionKind::Plan { plan } => {
                let start = asked.clone();
                let keep = asked.clone();
                let palette = *colours;
                let copied = plan.clone();
                let hover = colours.surface_2;
                // A plan is a document to read before it is approved: the
                // frame names it, the plan itself sits in a page of its own
                // (as long as it needs, up to a scroll), and what it says can
                // be copied.
                frame("PROPOSED PLAN")
                    .debug_selector(move || format!("chat-plan-card-{}", id.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("chat-plan-{}", id.0)))
                            .debug_selector(move || format!("chat-plan-page-{}", id.0))
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
                                    SharedString::from(format!("chat-{}-plan", id.0)),
                                    SharedString::from(
                                        super::chat_code::normalize(plan).into_owned(),
                                    ),
                                )
                                .selectable(true)
                                .code_block_actions(
                                    move |block, _, _| {
                                        super::chat_code::corner(
                                            format!("chat-plan-code-{}", id.0),
                                            block.lang().as_deref(),
                                            block.code(),
                                            &palette,
                                        )
                                    },
                                ),
                            ),
                    )
                    .child(
                        div().flex().child(
                            div()
                                .id(SharedString::from(format!("chat-plan-copy-{}", id.0)))
                                .debug_selector(move || format!("chat-plan-copy-{}", id.0))
                                .h(px(20.))
                                .px(px(6.))
                                .flex()
                                .items_center()
                                .rounded(metrics::RADIUS())
                                .cursor_pointer()
                                .hover(move |style| style.bg(hover))
                                .child(mono("COPY PLAN").text_color(colours.text_muted))
                                .on_click(move |_: &ClickEvent, _, cx| {
                                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                        copied.clone(),
                                    ));
                                }),
                        ),
                    )
                    .child(reason_field(&chat.reason, colours))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                button(
                                    "chat-plan-start",
                                    id,
                                    "START WORKING",
                                    Look::Primary,
                                    colours,
                                )
                                .on_click(cx.listener(
                                    move |this, _: &ClickEvent, window, cx| {
                                        this.chat_plan(id, &start, true, window, cx);
                                    },
                                )),
                            )
                            .child(
                                button(
                                    "chat-plan-keep",
                                    id,
                                    "KEEP PLANNING",
                                    Look::Outline,
                                    colours,
                                )
                                .on_click(cx.listener(
                                    move |this, _: &ClickEvent, window, cx| {
                                        this.chat_plan(id, &keep, false, window, cx);
                                    },
                                )),
                            ),
                    )
                    .into_any_element()
            }
        }
    }

    /// One way of answering a permission question.
    fn chat_choice(
        &self,
        id: LiveId,
        question: &str,
        choice: &PermissionChoice,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let look = match choice.effect {
            PermissionEffect::AllowOnce => Look::Primary,
            PermissionEffect::AllowAlways => Look::Outline,
            PermissionEffect::Deny => Look::Quiet,
        };
        // A rule can be a whole command: it is cut to fit the card.
        let label = match &choice.scope {
            Some(scope) if !scope.is_empty() => {
                format!("{} ({})", choice.label, cut(scope, SCOPE_CHARS))
            }
            _ => choice.label.clone(),
        };
        let question = question.to_owned();
        let picked = choice.id.clone();
        let selector: SharedString = format!("chat-choice-{}", choice.id).into();
        button(selector, id, label.to_uppercase(), look, colours).on_click(cx.listener(
            move |this, _: &ClickEvent, window, cx| {
                this.chat_permit(id, &question, &picked, window, cx);
            },
        ))
    }

    /// One question with options: its words, then each option as a row to
    /// pick.
    fn chat_options(
        &self,
        id: LiveId,
        chat: &Chat,
        question: &str,
        of: &ChoiceQuestion,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let picked = chat.picks.get(&(question.to_owned(), of.id.clone()));
        let hover = colours.surface_2;
        let mut rows = div().flex().flex_col().gap(px(2.));
        for (index, option) in of.options.iter().enumerate() {
            let chosen = picked.is_some_and(|picked| picked.contains(&option.label));
            let question = question.to_owned();
            let part = of.id.clone();
            let label = option.label.clone();
            let name = format!("chat-option-{}-{}-{index}", id.0, of.id);
            rows = rows.child(
                div()
                    .id(SharedString::from(name.clone()))
                    .debug_selector(move || name.clone())
                    .min_h(px(28.))
                    .px(px(8.))
                    .py(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .rounded(metrics::RADIUS())
                    .cursor_pointer()
                    .when(chosen, |this| this.bg(colours.surface_2))
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.chat_pick(id, &question, &part, &label, cx);
                    }))
                    .child(
                        div()
                            .flex_none()
                            .size(px(10.))
                            .border_1()
                            .border_color(if chosen {
                                colours.signal
                            } else {
                                colours.elevated_border
                            })
                            .when(chosen, |this| this.bg(colours.signal)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(colours.text)
                                    .child(option.label.clone()),
                            )
                            .children(option.description.clone().map(|description| {
                                div()
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(colours.text_muted)
                                    .child(description)
                            })),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(
                of.header
                    .clone()
                    .map(|header| mono(header.to_uppercase()).text_color(colours.text_faint)),
            )
            .child(div().text_color(colours.text).child(of.question.clone()))
            .child(rows)
    }

    /// The field, with what the next turn runs as under it and the one thing
    /// to do next on its right: send, or stop.
    fn chat_composer(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let running = chat.conversation.running();
        let queued = chat.conversation.queued.len();
        let mode = chat.options.as_ref().and_then(|options| {
            let current = chat.mode.as_ref().or(options.default_mode.as_ref())?;
            options.modes.iter().find(|mode| &mode.id == current)
        });
        // What the next turn runs with: what was asked for here, else what
        // the agent itself is set up with. "Default" is only said while
        // nothing is known of the agent yet.
        let model = crate::chat::complete::model_shown(
            chat.options.as_ref(),
            chat.model.as_deref(),
            chat.conversation.model.as_deref(),
        );
        let can_pick_mode = chat
            .options
            .as_ref()
            .is_some_and(|options| options.modes.len() > 1);
        let can_pick_model = chat
            .options
            .as_ref()
            .is_some_and(|options| !options.models.is_empty());
        let mut foot = div().flex().items_center().gap(px(6.)).min_w_0();
        // The agent, first of the chips: a click lists the others to go on
        // with. While something goes on it is there and does not open.
        if !chat.agents_to_switch_to().is_empty() {
            let label = format::agent_name(chat.agent).to_uppercase();
            let agent = chip("chat-agent-chip", id, label, false, colours);
            foot = foot.child(if chat.can_switch() {
                agent.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.chat_open_menu(id, MenuKind::Agents, window, cx);
                }))
            } else {
                agent.opacity(0.5).cursor_default()
            });
        }
        if can_pick_mode {
            let dangerous = mode.is_some_and(|mode| mode.dangerous);
            let label = mode.map_or_else(
                || "DEFAULT MODE".to_owned(),
                |mode| mode.label.to_uppercase(),
            );
            foot = foot.child(chip("chat-mode", id, label, dangerous, colours).on_click(
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.chat_open_menu(id, MenuKind::Modes, window, cx);
                }),
            ));
        }
        if can_pick_model {
            let label =
                model.map_or_else(|| "DEFAULT MODEL".to_owned(), |model| model.to_uppercase());
            foot = foot.child(
                chip("chat-model", id, label, false, colours).on_click(cx.listener(
                    move |this, _: &ClickEvent, window, cx| {
                        this.chat_open_menu(id, MenuKind::Models, window, cx);
                    },
                )),
            );
        }
        if !chat.efforts().is_empty() {
            let effort =
                crate::chat::complete::effort_shown(chat.options.as_ref(), chat.effort.as_deref());
            let label = effort.map_or_else(
                || "DEFAULT EFFORT".to_owned(),
                |effort| {
                    format!(
                        "{} EFFORT",
                        crate::chat::complete::effort_label(effort).to_uppercase()
                    )
                },
            );
            foot = foot.child(chip("chat-effort", id, label, false, colours).on_click(
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.chat_open_menu(id, MenuKind::Efforts, window, cx);
                }),
            ));
        }
        if queued > 0 {
            foot = foot.child(
                mono(if queued == 1 {
                    "1 QUEUED".to_owned()
                } else {
                    format!("{queued} QUEUED")
                })
                .debug_selector(move || format!("chat-queued-{}", id.0))
                .text_color(colours.text_faint),
            );
        }
        let action = if running {
            button("chat-stop", id, "STOP", Look::Outline, colours)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.chat_interrupt(id, cx);
                }))
                .into_any_element()
        } else {
            button("chat-send", id, "SEND", Look::Primary, colours)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.chat_send(id, window, cx);
                }))
                .into_any_element()
        };
        let drop_over = colours.surface_2;
        div()
            .id(SharedString::from(format!("chat-composer-{}", id.0)))
            .debug_selector(move || format!("chat-composer-{}", id.0))
            // Files dropped on the field go with what is said next.
            .drag_over::<gpui_kit::ExternalPaths>(move |style, _, _, _| style.bg(drop_over))
            .on_drop(
                cx.listener(move |this, paths: &gpui_kit::ExternalPaths, window, cx| {
                    this.chat_attach_paths(id, paths.paths().to_vec(), cx);
                    if let Some(chat) = this.chats.get(&id) {
                        chat.input.update(cx, |input, cx| input.focus(window, cx));
                    }
                }),
            )
            .relative()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(10.))
            .pt(px(8.))
            .pb(px(8.))
            .border_1()
            .border_color(colours.elevated_border)
            .bg(colours.surface)
            .children(super::lines::focus_ticks("chat-composer", colours))
            .children(self.chat_queue_list(id, chat, colours, cx))
            .children(attached_files(
                &chat.attached,
                (
                    format!("chat-attached-{}", id.0),
                    format!("chat-attached-remove-{}", id.0),
                ),
                colours,
                cx,
                move |this, gone, window, cx| {
                    if let Some(chat) = this.chats.get_mut(&id) {
                        chat.attached.retain(|file| *file != gone);
                        chat.input.update(cx, |input, cx| input.focus(window, cx));
                    }
                },
            ))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_none()
                            .pt(px(6.))
                            .pl(px(2.))
                            .font_family(fonts::mono())
                            .font_features(fonts::mono_features())
                            .text_color(colours.signal)
                            .child(">"),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("chat-input-{}", id.0))
                            .flex_1()
                            .min_w_0()
                            .child(Textarea::new(&chat.input).appearance(false).bordered(false)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(foot.flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(6.))
                            .child(self.chat_attach_button(id, colours, cx))
                            .child(self.chat_dictate_button(id, colours, cx))
                            .child(key_cap(if running { "ESC" } else { "ENTER" }, colours))
                            .child(action),
                    ),
            )
            .into_any_element()
    }
}

impl Shell {
    /// The files that went with a message: a picture as a thumbnail, any
    /// other file as a chip with its name. Each opens in the system's viewer
    /// when clicked.
    fn chat_sent_files(
        &self,
        id: LiveId,
        at: usize,
        files: &[std::path::PathBuf],
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if files.is_empty() {
            return None;
        }
        let tiles = files.iter().enumerate().map(|(place, file)| {
            if super::widgets::is_image(file) {
                return super::widgets::thumbnail(
                    file,
                    px(96.),
                    format!("chat-sent-image-{}-{at}-{place}", id.0),
                    cx,
                );
            }
            let opened = file.clone();
            let hover = colours.surface_2;
            div()
                .id(SharedString::from(format!(
                    "chat-sent-file-{}-{at}-{place}",
                    id.0
                )))
                .debug_selector(move || format!("chat-sent-file-{}-{at}-{place}", id.0))
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

    /// The messages that wait for the turn to end, one row each above the
    /// field: what was said (its first line), and what can be done with it.
    /// Edit puts it back in the field, remove drops it, and send now tells
    /// the running turn instead of waiting for its end.
    fn chat_queue_list(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if chat.conversation.queued.is_empty() {
            return None;
        }
        let action = |part: &'static str, label: &'static str, index: usize| {
            let hover = colours.surface_2;
            div()
                .id(SharedString::from(format!(
                    "chat-queue-{part}-{}-{index}",
                    id.0
                )))
                .debug_selector(move || format!("chat-queue-{part}-{}-{index}", id.0))
                .flex_none()
                .h(px(20.))
                .px(px(6.))
                .flex()
                .items_center()
                .rounded(metrics::RADIUS())
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(mono(label).text_color(colours.text_muted))
        };
        let rows = chat
            .conversation
            .queued
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let (words, files) = super::king::split_attached(text);
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
                div()
                    .debug_selector(move || format!("chat-queue-item-{}-{index}", id.0))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(mono(format!("{}", index + 1)).text_color(colours.text_faint))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(shown),
                    )
                    .child(action("edit", "EDIT", index).on_click(cx.listener(
                        move |this, _: &ClickEvent, window, cx| {
                            this.chat_queue_edit(id, index, window, cx);
                        },
                    )))
                    .child(action("remove", "REMOVE", index).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            this.chat_queue_remove(id, index, cx);
                        },
                    )))
                    .child(action("steer", "SEND NOW", index).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            this.chat_queue_steer(id, index, cx);
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
                .border_color(colours.border)
                .children(rows)
                .into_any_element(),
        )
    }

    /// What can be done with a message the person sent, shown while it is
    /// hovered: put it back in the field to change it, or send it again.
    fn chat_said_actions(
        &self,
        id: LiveId,
        at: usize,
        group: SharedString,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = |name: &'static str, label: &'static str, resend: bool| {
            let hover = colours.surface_2;
            div()
                .id(SharedString::from(format!("{name}-{}-{at}", id.0)))
                .debug_selector(move || format!("{name}-{}-{at}", id.0))
                .flex_none()
                .h(px(20.))
                .px(px(6.))
                .flex()
                .items_center()
                .rounded(metrics::RADIUS())
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(mono(label).text_color(colours.text_muted))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.chat_rewind_ask(id, at, resend, cx);
                }))
        };
        div()
            .flex_none()
            .flex()
            .items_start()
            .gap(px(2.))
            .invisible()
            .group_hover(group, |style| style.visible())
            .child(action("chat-edit", "EDIT FROM HERE", false))
            .child(action("chat-retry", "RETRY", true))
            .into_any_element()
    }

    /// The question of what to do about the files, above the field, while
    /// the conversation is asked to go back to before one of its messages.
    fn chat_rewind_cell(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let asked = chat.rewinding?;
        let files = self.chat_rewind_files(id, asked.at);
        let what = if asked.resend { "Retry" } else { "Edit" };
        let (head, body) = match files {
            Files::Offered => (
                format!("{what} from here?"),
                "The conversation goes back to before this message. The files can go back to how they were then, or stay as they are.",
            ),
            Files::NoCheckpoint => (
                format!("{what} from here?"),
                "The conversation goes back to before this message. The files stay as they are: none were kept of them here.",
            ),
            Files::Shared => (
                format!("{what} from here?"),
                "The conversation goes back to before this message. Another session works in this folder, so the files stay as they are.",
            ),
        };
        let mut answers = div().flex().flex_wrap().items_center().gap(px(8.));
        if asked.busy {
            answers = answers.child(mono("GOING BACK\u{2026}").text_color(colours.text_muted));
        } else {
            if files == Files::Offered {
                answers = answers.child(
                    button(
                        "chat-rewind-files",
                        id,
                        "REVERT FILES TOO",
                        Look::Primary,
                        colours,
                    )
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, window, cx| {
                            this.chat_rewind(id, true, window, cx);
                        },
                    )),
                );
            }
            answers = answers
                .child(
                    button(
                        "chat-rewind-keep",
                        id,
                        if files == Files::Offered {
                            "KEEP FILES"
                        } else {
                            "CONTINUE"
                        },
                        if files == Files::Offered {
                            Look::Outline
                        } else {
                            Look::Primary
                        },
                        colours,
                    )
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, window, cx| {
                            this.chat_rewind(id, false, window, cx);
                        },
                    )),
                )
                .child(
                    button("chat-rewind-cancel", id, "CANCEL", Look::Quiet, colours).on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.chat_rewind_cancel(id, cx);
                        }),
                    ),
                );
        }
        Some(
            div()
                .debug_selector(move || format!("chat-rewind-{}", id.0))
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
                        .child(
                            mono(format!("[ {} ]", head.to_uppercase())).text_color(colours.signal),
                        ),
                )
                .child(
                    div()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(body),
                )
                .child(answers)
                .into_any_element(),
        )
    }

    /// The paperclip of a conversation's field: files to attach to what is
    /// said next.
    fn chat_attach_button(
        &self,
        id: LiveId,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = format!("chat-attach-{}", id.0);
        let hover = colours.surface_2;
        div()
            .id(SharedString::from(name.clone()))
            .debug_selector(move || name.clone())
            .flex_none()
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Attach files (or paste or drop them)")
                    .build(window, cx)
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.chat_attach(id, window, cx);
            }))
            .child(icon(IconName::Paperclip, px(13.), colours.text_muted))
            .into_any_element()
    }
}

/// Who an answer is from (see [`crate::chat::complete::answered_by`]).
fn answered_by(chat: &Chat) -> String {
    crate::chat::complete::answered_by(
        chat.conversation.model.as_deref(),
        format::agent_tag(chat.agent),
    )
}

/// The column every part of a conversation sits in: the whole width of the
/// pane, so code and tables keep their lines on any screen.
fn column() -> Div {
    div().w_full().min_w_0().flex().flex_col()
}

/// What the agent is doing, under the last thing it did, while a turn is in
/// flight: a spinner that turns while it works, as King's does, and how long
/// the turn has gone on (`now` in milliseconds since the epoch).
fn chat_status(id: LiveId, chat: &Chat, now: u64, colours: &Palette) -> Option<AnyElement> {
    let (light, words) = match chat.conversation.phase() {
        Phase::Idle => return None,
        Phase::Starting => (None, "STARTING"),
        Phase::Working => (None, "WORKING"),
        Phase::Asking => (Some(colours.warning), "WAITING FOR YOUR ANSWER"),
    };
    let sign = match light {
        Some(light) => led(light).into_any_element(),
        None => mono(super::king_view::spinner_frame())
            .debug_selector(move || format!("chat-spinner-{}", id.0))
            .text_color(colours.signal)
            .into_any_element(),
    };
    Some(
        div()
            .debug_selector(move || format!("chat-status-{}", id.0))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(sign)
            .child(mono(words).text_color(colours.text_muted))
            .children(chat.since.map(|since| {
                mono(leon_king::report::span(
                    (now.saturating_sub(since) / 1000) as i64,
                ))
                .text_color(colours.text_faint)
            }))
            .into_any_element(),
    )
}

/// The line under the field: the agent, the folder, and what the
/// conversation cost and took so far, then its branch's pull request.
fn chat_foot(id: LiveId, chat: &Chat, controls: [Option<AnyElement>; 3], colours: &Palette) -> Div {
    let mut parts = vec![format::agent_tag(chat.agent).to_owned()];
    if let Some(account) = &chat.account {
        parts.push(format::account_tag(account));
    }
    parts.push(folder_name(&chat.cwd));
    if let Some(ms) = chat.conversation.last_duration_ms {
        parts.push(duration_text(ms).to_uppercase());
    }
    if let Some(tokens) = chat.conversation.last_tokens.as_ref().and_then(|tokens| {
        crate::chat::model::tokens_text(tokens, chat.conversation.last_duration_ms)
    }) {
        parts.push(tokens.to_uppercase());
    }
    if let Some(cost) = chat.conversation.cost_usd {
        parts.push(cost_text(cost));
    }
    let meter = chat.conversation.context.map(|meter| {
        let fill = match meter.level() {
            Level::Roomy => colours.text_muted,
            Level::Tight => colours.warning,
            Level::Full => colours.error,
        };
        div()
            .id(SharedString::from(format!("chat-context-tip-{}", id.0)))
            .debug_selector(move || format!("chat-context-{}", id.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .debug_selector(move || format!("chat-context-bar-{}", id.0))
                    .w(px(36.))
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(colours.border)
                    .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(meter.fraction()))
                            .rounded(px(2.))
                            .bg(fill),
                    ),
            )
            .child(
                mono(format!("{}%", meter.percent()))
                    .debug_selector(move || format!("chat-context-percent-{}", id.0))
                    .text_color(fill),
            )
            .tooltip({
                let tip = SharedString::from(format!("Context window: {} tokens", meter.label()));
                move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                }
            })
    });
    div()
        .debug_selector(move || format!("chat-foot-{}", id.0))
        .px(px(2.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(fonts::mono())
                .text_size(metrics::TEXT_LABEL())
                .text_color(colours.text_faint)
                .child(parts.join("  \u{b7}  ")),
        )
        .children(meter)
        .children(controls.into_iter().flatten())
        .child(mono("BETA").text_color(colours.text_faint))
}

/// A pull request as one mono line, `PR #123 · APPROVED · CHECKS PASSING`,
/// coloured by its checks; a click opens it. `name` is its selector.
pub(super) fn pull_request_chip(
    name: String,
    pull_request: &leon_core::PullRequest,
    colours: &Palette,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let tone = super::checkout::pull_request_tone(pull_request);
    let colour = super::widgets::checkout_colour(tone, colours);
    let words = format!(
        "PR {}",
        super::checkout::pull_request_words(pull_request).to_uppercase()
    );
    let chip = mono(words).min_w_0().truncate().text_color(colour);
    let Some(url) = super::checkout::openable_url(pull_request) else {
        return chip.debug_selector(move || name.clone()).into_any_element();
    };
    let url = url.to_owned();
    let tip: SharedString = format!("Open {url}").into();
    let hover = colours.surface_2;
    chip.id(SharedString::from(name.clone()))
        .debug_selector(move || name.clone())
        .px(px(4.))
        .rounded(metrics::RADIUS())
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            (this.options.open_url)(cx, &url);
        }))
        .into_any_element()
}

/// A folder as the foot of a conversation names it: its last two parts, which
/// is the project and the worktree where the whole path is the header's.
fn folder_name(path: &str) -> String {
    let parts: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    match parts.len() {
        0 => path.to_owned(),
        1 | 2 => path.to_owned(),
        count => format!("\u{2026}/{}/{}", parts[count - 2], parts[count - 1]),
    }
}

/// How many lines of a diff are drawn before the rest is counted.
const DIFF_LINES: usize = 200;

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
fn diff_block(tool: &str, at: usize, id: LiveId, change: &FileChange, colours: &Palette) -> Div {
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
        format!("chat-diff-{}-{tool}", id.0)
    } else {
        format!("chat-diff-{}-{tool}-{at}", id.0)
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
fn code_cell(text: &str, colours: &Palette) -> Div {
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

/// The field for what to do instead, in its own hairline box.
pub(super) fn reason_field(
    state: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
    colours: &Palette,
) -> Div {
    div()
        .px(px(8.))
        .h(px(30.))
        .flex()
        .items_center()
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(colours.border)
        .bg(colours.background)
        .child(super::widgets::text_input(state))
}

/// How a button looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Look {
    /// The one thing to do next: filled with the accent.
    Primary,
    /// Something else that can be done: an outline.
    Outline,
    /// The way out: words only.
    Quiet,
}

/// A button: mono capitals in a control-high box.
pub(super) fn button(
    selector: impl Into<SharedString>,
    id: LiveId,
    label: impl Into<SharedString>,
    look: Look,
    colours: &Palette,
) -> Stateful<Div> {
    let selector: SharedString = selector.into();
    let name = format!("{selector}-{}", id.0);
    let hover = colours.surface_2;
    let base = div()
        .id(SharedString::from(name.clone()))
        .debug_selector(move || name.clone())
        .flex_none()
        .h(px(26.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(metrics::RADIUS())
        .border_1()
        .cursor_pointer()
        .child(mono(label));
    match look {
        Look::Primary => base
            .border_color(colours.primary_fill)
            .bg(colours.primary_fill)
            .text_color(colours.on_primary)
            .hover(|style| style.opacity(0.88)),
        Look::Outline => base
            .border_color(colours.elevated_border)
            .text_color(colours.text)
            .hover(move |style| style.bg(hover)),
        Look::Quiet => base
            .border_color(gpui_kit::transparent_black())
            .text_color(colours.text_muted)
            .hover(move |style| style.bg(hover)),
    }
}

/// A chip of the field's foot: what the next turn runs as. A click opens the
/// list to change it.
fn chip(
    selector: &'static str,
    id: LiveId,
    label: String,
    warn: bool,
    colours: &Palette,
) -> Stateful<Div> {
    let name = format!("{selector}-{}", id.0);
    let hover = colours.surface_2;
    div()
        .id(SharedString::from(name.clone()))
        .debug_selector(move || name.clone())
        .flex_none()
        .max_w(px(220.))
        .h(px(22.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(if warn {
            colours.warning
        } else {
            colours.border
        })
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(mono(label).truncate().text_color(if warn {
            colours.warning
        } else {
            colours.text_muted
        }))
        .child(icon(IconName::ChevronDown, px(10.), colours.text_faint))
}

/// What the rule of a switch of agents says.
pub(super) fn switched_said(from: leon_core::AgentId, to: leon_core::AgentId) -> String {
    format!(
        "Switched from {} to {} \u{b7} context handed off",
        format::agent_name(from),
        format::agent_name(to)
    )
}

/// How much of an "always allow" rule its button shows.
const SCOPE_CHARS: usize = 60;

/// `text` cut to `most` characters, with an ellipsis when it was longer.
fn cut(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_owned();
    }
    let kept: String = text.chars().take(most.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::cut;

    #[test]
    fn a_long_rule_is_cut_and_a_short_one_kept() {
        assert_eq!(cut("git add *", 60), "git add *");
        assert_eq!(cut("abcdef", 4), "abc\u{2026}");
    }
}

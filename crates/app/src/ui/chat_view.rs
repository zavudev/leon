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
use leon_turn::{ChoiceQuestion, PermissionChoice, PermissionEffect, QuestionKind};

use super::chat::Chat;
use super::chat_menu::MenuKind;
use super::live::LiveId;
use super::shell::Shell;
use super::widgets::{key_cap, led, mark, mono, Lion};
use crate::chat::model::{
    blocks, cost_text, duration_text, work_label, Block, Item, Phase, Question, Tone, Tool,
};
use crate::format;
use crate::icons::{agent_icon, icon, IconName};
use crate::theme::{fonts, metrics, px, Palette};

/// The widest the column of a conversation gets.
const COLUMN: f32 = 820.;

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
        Some(
            div()
                .debug_selector(move || format!("pane-{}", id.0))
                .relative()
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
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
                .child(body)
                .child(
                    column()
                        .flex_none()
                        .px(px(16.))
                        .pb(px(12.))
                        .gap(px(8.))
                        .children(questions)
                        .children(self.chat_menu(id, chat, colours, cx))
                        .child(self.chat_composer(id, chat, colours, cx))
                        .child(chat_foot(
                            id,
                            chat,
                            self.chat_pull_request(id, chat, colours, cx),
                            colours,
                        )),
                )
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
                    .max_w(px(COLUMN))
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
        let mut list = column().px(px(16.)).pt(px(20.)).pb(px(12.)).gap(px(14.));
        for block in blocks(items, running) {
            list = list.child(match block {
                Block::Single(at) => self.chat_item(id, at, &items[at], chat, colours),
                Block::Work { from, to, live } => {
                    self.chat_work(id, chat, from, &items[from..to], live, colours, cx)
                }
            });
        }
        if let Some(status) = chat_status(id, chat, colours) {
            list = list.child(status);
        }
        div()
            .id(SharedString::from(format!("chat-scroll-{}", id.0)))
            .debug_selector(move || format!("chat-scroll-{}", id.0))
            .track_scroll(&chat.scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(list)
            .into_any_element()
    }

    /// A message or a notice.
    fn chat_item(
        &self,
        id: LiveId,
        at: usize,
        item: &Item,
        chat: &Chat,
        colours: &Palette,
    ) -> AnyElement {
        match item {
            Item::User { text } => div()
                .debug_selector(move || format!("chat-user-{}-{at}", id.0))
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
                        .border_color(colours.border)
                        .bg(colours.surface)
                        .text_color(colours.text)
                        .child(text.clone()),
                )
                .into_any_element(),
            Item::Agent { text, .. } => div()
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
                        .child(agent_icon(chat.agent, px(12.), colours))
                        .child(mono(format::agent_tag(chat.agent)).text_color(colours.text_faint)),
                )
                .child(
                    div().w_full().min_w_0().text_color(colours.text).child(
                        TextView::markdown(
                            SharedString::from(format!("chat-{}-message-{at}", id.0)),
                            text.clone(),
                        )
                        .selectable(true),
                    ),
                )
                .into_any_element(),
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
            // Work is drawn by `chat_work`.
            Item::Thought { .. } | Item::Tool(_) => div().into_any_element(),
        }
    }

    /// A stretch of work: one line that says how much, and under it, when it
    /// is open, each thought and tool.
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
        // Open while it happens; folded once the conversation moved on,
        // unless it was opened by hand (and the other way round while live).
        let toggled = chat.open_work.contains(&from);
        let open = live != toggled;
        let last = items.iter().rev().find_map(|item| match item {
            Item::Tool(tool) => Some(format!("{} {}", tool.name, tool.summary)),
            _ => None,
        });
        let hover = colours.surface_2;
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
            .child(mono(work_label(items).to_uppercase()).text_color(colours.text_muted))
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
        let mut body = div()
            .ml(px(11.))
            .pl(px(12.))
            .border_l_1()
            .border_color(colours.border)
            .flex()
            .flex_col()
            .gap(px(2.));
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
                frame("PLAN")
                    .child(
                        div()
                            .id(SharedString::from(format!("chat-plan-{}", id.0)))
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .min_w_0()
                            .child(TextView::markdown(
                                SharedString::from(format!("chat-{}-plan", id.0)),
                                plan.clone(),
                            )),
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
        let label = match &choice.scope {
            Some(scope) if !scope.is_empty() => format!("{} ({scope})", choice.label),
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
        div()
            .debug_selector(move || format!("chat-composer-{}", id.0))
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
            .border_color(colours.elevated_border)
            .bg(colours.surface)
            .child(
                div()
                    .debug_selector(move || format!("chat-input-{}", id.0))
                    .w_full()
                    .min_w_0()
                    .child(Textarea::new(&chat.input).appearance(false).bordered(false)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(foot.flex_1())
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(self.chat_dictate_button(id, colours, cx))
                            .child(key_cap(if running { "ESC" } else { "ENTER" }, colours))
                            .child(action),
                    ),
            )
            .into_any_element()
    }
}

/// The column every part of a conversation sits in: as wide as the pane up
/// to [`COLUMN`], on its centre line.
fn column() -> Div {
    div()
        .w_full()
        .max_w(px(COLUMN))
        .mx_auto()
        .min_w_0()
        .flex()
        .flex_col()
}

/// What the agent is doing, under the last thing it did, while a turn is in
/// flight.
fn chat_status(id: LiveId, chat: &Chat, colours: &Palette) -> Option<AnyElement> {
    let (light, words) = match chat.conversation.phase() {
        Phase::Idle => return None,
        Phase::Starting => (colours.text_faint, "STARTING"),
        Phase::Working => (colours.success, "WORKING"),
        Phase::Asking => (colours.warning, "WAITING FOR YOUR ANSWER"),
    };
    Some(
        div()
            .debug_selector(move || format!("chat-status-{}", id.0))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(led(light))
            .child(mono(words).text_color(colours.text_muted))
            .into_any_element(),
    )
}

/// The line under the field: the agent, the folder, and what the
/// conversation cost and took so far, then its branch's pull request.
fn chat_foot(id: LiveId, chat: &Chat, pull_request: Option<AnyElement>, colours: &Palette) -> Div {
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
        .children(pull_request)
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
enum Look {
    /// The one thing to do next: filled with the accent.
    Primary,
    /// Something else that can be done: an outline.
    Outline,
    /// The way out: words only.
    Quiet,
}

/// A button: mono capitals in a control-high box.
fn button(
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

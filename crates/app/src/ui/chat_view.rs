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
use gpui_kit::prelude::*;
use gpui_kit::{
    div, AnyElement, ClickEvent, Context, Div, FontWeight, MouseDownEvent, SharedString, Stateful,
};
use leon_turn::{ChoiceQuestion, PermissionChoice, PermissionEffect, QuestionKind};

use super::chat::Chat;
use super::chat_find::Mark;
use super::chat_menu::MenuKind;
use super::chat_rewind::Files;
use super::edits::Subject;
use super::live::LiveId;
use super::shell::Shell;
use super::transcript::{
    code_cell, column, plan_card, question_frame, status_line, Folder, Ids, Speaker, Surface,
    Transcript,
};
use super::widgets::{attached_files, key_cap, led, mark, mono, with_latest, Lion};
use crate::chat::model::{blocks, cost_text, duration_text, Block, Item, Level, Phase, Question};
use crate::format;
use crate::icons::{agent_icon, icon, IconName};
use crate::theme::{fonts, metrics, px, Palette};

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
        let panel = self.chat_diff_panel(Subject::Chat(id), colours, cx);
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
                    .children(self.chat_git_cell(Subject::Chat(id), colours, cx))
                    .children(self.chat_menu(id, chat, colours, cx))
                    .child(self.chat_composer(id, chat, colours, cx))
                    .child(chat_foot(
                        id,
                        chat,
                        [
                            self.chat_branch_label(Subject::Chat(id), colours),
                            self.chat_pull_request(id, chat, colours, cx),
                            Some(self.chat_git_button(Subject::Chat(id), colours, cx)),
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
        let speaker = |at: usize| {
            let who = crate::chat::model::agent_at(items, at, chat.agent);
            Speaker {
                agent: Some(who),
                label: if who == chat.agent {
                    answered_by(chat)
                } else {
                    // Said by the agent before a switch: the model in use now
                    // is not its.
                    crate::chat::complete::answered_by(None, format::agent_tag(who))
                },
            }
        };
        // What is done with the words of the person: put them back to change
        // them, or say them again.
        let said_actions =
            |shell: &Shell, at: usize, group: SharedString, cx: &mut Context<Shell>| {
                chat.can_rewind(at)
                    .then(|| shell.chat_said_actions(id, at, group, colours, cx))
            };
        let transcript = Transcript {
            ids: chat_ids(id),
            surface: Surface::Chat(id),
            folds: &chat.folds,
            folder: chat.machine.is_local().then_some(Folder {
                cwd: Some(chat.cwd.as_str()),
            }),
            speaker: &speaker,
            said_actions: Some(&said_actions),
        };
        // The list is the scrolling element itself, so that each of its
        // children can be scrolled to (the first one is the space above it,
        // and the gap after the last is the space below).
        let mut list = vec![div().flex_none().h(px(4.)).into_any_element()];
        let cards = chat
            .edits
            .cards(items, &chat.cwd, running || chat.launching);
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
                Block::Single(at) => self.transcript_item(&transcript, at, &items[at], colours, cx),
                Block::Work { from, to, live } => {
                    self.transcript_work(&transcript, from, &items[from..to], live, colours, cx)
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
                        .child(self.chat_changed_card(Subject::Chat(id), *turn, stats, colours, cx))
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
        let ids = chat_ids(id);
        let frame = |label: &str| question_frame(&ids, &asked, label, colours);
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
                // A plan is a document to read before it is approved: the
                // frame names it, the plan itself sits in a page of its own,
                // and what it says can be copied.
                plan_card(frame("PROPOSED PLAN"), &ids, plan, colours)
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

/// What the agent is doing, under the last thing it did, while a turn is in
/// flight (`now` in milliseconds since the epoch).
fn chat_status(id: LiveId, chat: &Chat, now: u64, colours: &Palette) -> Option<AnyElement> {
    let (light, words) = match chat.conversation.phase() {
        Phase::Idle => return None,
        Phase::Starting => (None, "STARTING"),
        Phase::Working => (None, "WORKING"),
        Phase::Asking => (Some(colours.warning), "WAITING FOR YOUR ANSWER"),
    };
    Some(status_line(
        &chat_ids(id),
        light,
        words,
        chat.since,
        now,
        colours,
    ))
}

/// The names of the parts of the conversation of a pane.
fn chat_ids(id: LiveId) -> Ids {
    Ids::new("chat", id.0.to_string())
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
    button_tagged(selector, &id.0.to_string(), label, look, colours)
}

/// A button whose selector ends with `tag`: the conversation's.
pub(super) fn button_tagged(
    selector: impl Into<SharedString>,
    tag: &str,
    label: impl Into<SharedString>,
    look: Look,
    colours: &Palette,
) -> Stateful<Div> {
    let selector: SharedString = selector.into();
    let name = format!("{selector}-{tag}");
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

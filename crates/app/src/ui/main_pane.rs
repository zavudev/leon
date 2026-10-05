//! The main pane: a live terminal filling it, or the open session's
//! transcript, or the detail of the open project or worktree, and the status
//! line of the engine.

use super::lines::{empty_frame, frame_ticks};
use super::shell::{Main, Pane, Shell, Transcript};
use super::tree::worktree_label;
use super::widgets::{focus_rule, key_cap, mono, section_label};
use crate::format;
use crate::icons::agent_icon;
use crate::keys::{self, Command};
use crate::theme::{fonts, metrics, px, Palette};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::prelude::*;
use gpui_kit::{div, list, AnyElement, App, Context, Div, FontWeight};
use leon_core::{AgentKind, Message, Role};

/// The most characters of one message drawn: a transcript can hold a whole
/// file pasted by a tool, and laying that out every frame would cost more
/// than reading it is worth.
const MAX_MESSAGE_CHARS: usize = 6_000;

impl Shell {
    pub(super) fn render_main_pane(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let focused = self.pane == Pane::Main;
        let (kind, title, meta) = self.main_heading(cx);
        let body = match &self.main {
            Main::Empty => self.render_empty(colours).into_any_element(),
            Main::Project(id) => self.render_project(id, colours),
            Main::Worktree(project, worktree) => self.render_worktree(project, worktree, colours),
            Main::Session(transcript) => self.render_transcript(transcript, colours, cx),
            Main::Live(id) => self.render_live(*id, colours, cx),
        };
        let agent = self.main_agent();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| "main-header".into())
                    .relative()
                    .flex_none()
                    .h(metrics::HEADER_HEIGHT())
                    .px_4()
                    .py(metrics::HEADER_PAD())
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .flex_col()
                    .gap(metrics::HEADER_GAP())
                    .child(
                        div()
                            .debug_selector(|| "header-title-row".into())
                            .flex_none()
                            .h(metrics::HEADER_TITLE_LINE())
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.sidebar_toggle_button("sidebar-toggle-main", colours, cx))
                            // With the sidebar away the product's mark stays.
                            .when(!crate::settings::get(cx).sidebar_visible, |this| {
                                this.child(super::widgets::mark(
                                    px(20.),
                                    colours,
                                    cx,
                                    super::widgets::Lion::new("main-header-mark")
                                        .mood(self.mark_mood())
                                        .hover(),
                                ))
                            })
                            .children(
                                self.main_project().map(|id| {
                                    self.logo(&id, metrics::PROJECT_ICON(), "main", colours)
                                }),
                            )
                            .child(section_label(kind, colours))
                            .children(agent.map(|agent| {
                                div()
                                    .debug_selector(|| "main-agent-icon".into())
                                    .flex_none()
                                    .child(agent_icon(agent, px(14.), colours))
                            }))
                            .child(
                                div()
                                    .debug_selector(|| "main-title".into())
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(title),
                            )
                            .children(self.header_usage_chip(colours, cx)),
                    )
                    .child({
                        // What it is, where, and how it is doing: the path gives
                        // way with an ellipsis, the state words stay.
                        let (head, path, tail) = self.meta_parts(meta);
                        div()
                            .debug_selector(|| "header-meta-row".into())
                            .flex_none()
                            .h(metrics::HEADER_META_LINE())
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .text_color(colours.text_muted)
                            .child(
                                mono(head)
                                    .debug_selector(|| "header-meta".into())
                                    .line_height(metrics::HEADER_META_LINE())
                                    .whitespace_nowrap(),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "header-path".into())
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(mono(path).line_height(metrics::HEADER_META_LINE())),
                            )
                            .child(
                                mono(tail)
                                    .line_height(metrics::HEADER_META_LINE())
                                    .whitespace_nowrap(),
                            )
                    })
                    .when(focused, |this| this.child(focus_rule(colours))),
            )
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .child(self.render_status_bar(colours, cx))
    }

    /// The header's metadata as one line, for tests.
    #[cfg(test)]
    #[cfg_attr(not(unix), allow(dead_code))] // used by the Unix-only tests
    pub(super) fn main_heading_for_test(&self, cx: &App) -> String {
        self.main_heading(cx).2
    }

    /// The header's metadata split around the path it carries: what comes
    /// before it, the path, what comes after. Without a path, all of it is the
    /// first part.
    fn meta_parts(&self, meta: String) -> (String, String, String) {
        let path = match &self.main {
            Main::Project(id) => self.snapshot.project(id).map(|e| e.project.root.clone()),
            Main::Worktree(project, worktree) => self.snapshot.project(project).and_then(|e| {
                e.worktrees
                    .iter()
                    .find(|w| &w.id == worktree)
                    .map(|w| w.path.clone())
            }),
            Main::Live(id) => self.live.get(*id).map(|session| session.cwd.clone()),
            Main::Session(transcript) => Some(transcript.session.cwd.clone()),
            Main::Empty => None,
        };
        match path.filter(|path| !path.is_empty()).and_then(|path| {
            meta.split_once(&path)
                .map(|(head, tail)| (head.to_owned(), path.clone(), tail.to_owned()))
        }) {
            Some(parts) => parts,
            None => (meta, String::new(), String::new()),
        }
    }

    /// The project of what is open, for the logo in the header.
    fn main_project(&self) -> Option<leon_core::ProjectId> {
        match &self.main {
            Main::Project(id) | Main::Worktree(id, _) => Some(id.clone()),
            _ => None,
        }
    }

    /// Where the logo in effect came from, as the header's small print.
    fn logo_note(&self, project: &leon_core::ProjectId) -> Option<String> {
        use leon_core::IconKind;
        let icon = self.snapshot.icons.get(project)?;
        match icon.kind {
            IconKind::Detected => Some(format!("LOGO {}", icon.source.to_uppercase())),
            IconKind::Avatar => Some(format!("LOGO AVATAR {}", icon.source.to_uppercase())),
            IconKind::Custom => Some(format!("LOGO CUSTOM {}", icon.source.to_uppercase())),
            IconKind::Folder => None,
        }
    }

    /// The agent of what is open, for the icon in the header.
    fn main_agent(&self) -> Option<AgentKind> {
        match &self.main {
            Main::Session(transcript) => Some(transcript.session.agent),
            Main::Live(id) => self.live.get(*id).and_then(|session| session.shown_agent()),
            _ => None,
        }
    }

    /// What the header says: the kind of thing, its name and a line about it.
    pub(super) fn main_heading(&self, cx: &App) -> (&'static str, String, String) {
        match &self.main {
            Main::Empty => ("Home", "Nothing open".to_owned(), String::new()),
            Main::Project(id) => match self.snapshot.project(id) {
                Some(entry) => (
                    "Project",
                    self.project_label(&entry.project),
                    match self.logo_note(id) {
                        Some(note) => format!("{} · {note}", entry.project.root),
                        None => entry.project.root.clone(),
                    },
                ),
                None => ("Project", "Gone".to_owned(), String::new()),
            },
            Main::Worktree(project, worktree) => {
                let found = self.snapshot.project(project).and_then(|entry| {
                    entry
                        .worktrees
                        .iter()
                        .find(|w| &w.id == worktree)
                        .map(|w| (entry, w))
                });
                match found {
                    Some((entry, worktree)) => (
                        "Worktree",
                        worktree_label(worktree),
                        format!(
                            "{} · {}",
                            self.project_label(&entry.project).to_uppercase(),
                            worktree.path
                        ),
                    ),
                    None => ("Worktree", "Gone".to_owned(), String::new()),
                }
            }
            Main::Live(id) => match self.live.get(*id) {
                Some(session) => {
                    let who = session.agent.map_or("SHELL", format::agent_tag);
                    // A session the person resumed here anyway, while another
                    // process still holds it, says so.
                    let also = session
                        .history
                        .as_ref()
                        .and_then(|id| self.snapshot.sessions.iter().find(|s| &s.id == id))
                        .and_then(|stored| self.elsewhere_of(stored))
                        .map_or_else(String::new, |found| {
                            format!(
                                " · WARNING: {} {} HOLDS THIS SESSION TOO",
                                if found.is_certain() {
                                    "PID"
                                } else {
                                    "PROBABLY PID"
                                },
                                found.pid
                            )
                        });
                    // The state is read from the terminal itself; the title
                    // the program set (a path, a task) is the heading.
                    (
                        "Session",
                        session.label(),
                        format!(
                            "{who} · {} · {} · {}{}",
                            session.machine_name.to_uppercase(),
                            session.cwd,
                            session.state(cx).label(),
                            if session.resumed.is_some() {
                                " · RESUMED"
                            } else {
                                ""
                            }
                        ) + &also,
                    )
                }
                None => ("Session", "Gone".to_owned(), String::new()),
            },
            Main::Session(transcript) => {
                let session = &transcript.session;
                let held = match self.elsewhere_of(session) {
                    Some(found) if found.is_certain() => {
                        format!(" · RUNNING IN ANOTHER TERMINAL (PID {})", found.pid)
                    }
                    Some(found) => format!(
                        " · PROBABLY RUNNING IN ANOTHER TERMINAL (PID {})",
                        found.pid
                    ),
                    None => String::new(),
                };
                (
                    "Session",
                    session.title.clone(),
                    format!(
                        "{} · {} MESSAGES · {}{held}",
                        format::agent_tag(session.agent),
                        session.message_count,
                        session.cwd
                    ),
                )
            }
        }
    }

    /// What is on screen before anything is open: the keys that start things,
    /// read from the registry.
    fn render_empty(&self, colours: &Palette) -> Div {
        let line = |command: Command| {
            let label = keys::label(command);
            div()
                .debug_selector(move || format!("hint-{command:?}"))
                .flex()
                .items_center()
                .justify_between()
                .gap_6()
                .w(px(360.))
                .child(div().text_color(colours.text_muted).child(label))
                .children(keys::keys_label(command).map(|text| key_cap(text, colours)))
        };
        let hints = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.))
            .child(line(Command::OpenProject))
            .child(line(Command::GoTo))
            .child(line(Command::Commands))
            .child(line(Command::SearchHistory))
            .child(line(Command::NewSession))
            .child(line(Command::AddMachine))
            .child(line(Command::Shortcuts));
        // What there is to open, as the dimension of the drawing.
        let caption = format!(
            "{} PROJECTS \u{b7} {} SESSIONS",
            self.snapshot.projects.len(),
            self.snapshot.sessions.len()
        );
        div()
            .debug_selector(|| "main-empty".into())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .child(section_label("Nothing open", colours))
            .child(empty_frame("main-empty", hints, Some(caption), colours))
            .child(
                div()
                    .debug_selector(|| "main-empty-remote".into())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_muted)
                    .child(format!(
                        "Projects on another computer work too: connect a machine ({}) and drive it over SSH.",
                        keys::keys_label(Command::AddMachine).unwrap_or_default()
                    )),
            )
            .child(
                div().pt_4().child(
                    // The product's name is a name, not a label: as it is
                    // written, in the interface font.
                    div()
                        .debug_selector(|| "maker".into())
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_faint)
                        .child(format!(
                            "{} {} · by {}",
                            crate::product::PRODUCT_NAME,
                            crate::product::VERSION,
                            crate::product::MAKER
                        )),
                ),
            )
    }

    fn field(label: &'static str, value: String, colours: &Palette) -> Div {
        div()
            .flex()
            .gap_4()
            .child(
                div()
                    .flex_none()
                    .w(px(96.))
                    .child(mono(label).text_color(colours.text_muted)),
            )
            .child(
                div()
                    .debug_selector(move || format!("field-{label}"))
                    .flex_1()
                    .min_w_0()
                    .child(value),
            )
    }

    fn render_project(&self, id: &leon_core::ProjectId, colours: &Palette) -> AnyElement {
        let Some(entry) = self.snapshot.project(id) else {
            return div().into_any_element();
        };
        let machine = self
            .snapshot
            .machine(&entry.project.machine_id)
            .map_or_else(String::new, |machine| machine.name.clone());
        let mut worktrees = div().flex().flex_col().gap(px(4.));
        for worktree in &entry.worktrees {
            worktrees = worktrees.child(
                div()
                    .flex()
                    .gap_3()
                    .child(div().min_w_0().truncate().child(worktree_label(worktree)))
                    .child(mono(worktree.path.clone()).text_color(colours.text_faint)),
            );
        }
        div()
            .debug_selector(|| "project-detail".into())
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(Self::field("PATH", entry.project.root.clone(), colours))
            .child(Self::field("MACHINE", machine, colours))
            .child(Self::field(
                "WORKTREES",
                entry.worktrees.len().to_string(),
                colours,
            ))
            .child(div().pt_2().child(section_label("Worktrees", colours)))
            .child(worktrees)
            .into_any_element()
    }

    fn render_worktree(
        &self,
        project: &leon_core::ProjectId,
        worktree: &leon_core::WorktreeId,
        colours: &Palette,
    ) -> AnyElement {
        let Some(entry) = self.snapshot.project(project) else {
            return div().into_any_element();
        };
        let Some(wt) = entry.worktrees.iter().find(|w| &w.id == worktree) else {
            return div().into_any_element();
        };
        let now = self.now();
        let mut sessions = div().flex().flex_col();
        let mut shown = 0;
        for place in self.placement.of_worktree(&wt.id).iter().take(8) {
            let session = &self.snapshot.sessions[*place];
            shown += 1;
            sessions = sessions.child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex_none().w(px(64.)).child(
                        mono(format::agent_tag(session.agent)).text_color(colours.text_muted),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(session.title.clone()),
                    )
                    .child(
                        mono(format::age(now, session.updated_at)).text_color(colours.text_faint),
                    ),
            );
        }
        if shown == 0 {
            sessions = sessions.child(empty_frame(
                "worktree-empty",
                div()
                    .debug_selector(|| "worktree-no-sessions".into())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint)
                    .child("No sessions ran here yet."),
                Some("0 SESSIONS".to_owned()),
                colours,
            ));
        }
        div()
            .debug_selector(|| "worktree-detail".into())
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(Self::field("PROJECT", entry.project.name.clone(), colours))
            .child(Self::field("PATH", wt.path.clone(), colours))
            .child(Self::field("BRANCH", worktree_label(wt), colours))
            .child(Self::field(
                "HEAD",
                wt.head.as_deref().map_or_else(
                    || "unknown".to_owned(),
                    |head| format::short_head(head).to_owned(),
                ),
                colours,
            ))
            .child(Self::field(
                "KIND",
                if wt.is_main { "MAIN" } else { "LINKED" }.to_owned(),
                colours,
            ))
            .child(div().pt_2().child(section_label("Sessions", colours)))
            .child(sessions)
            .into_any_element()
    }

    /// The stored transcript, under a line that says how to resume the
    /// session, or why it could not be resumed and what to do instead.
    fn render_transcript(
        &self,
        transcript: &Transcript,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bar = div()
            .flex_none()
            .px_4()
            .py(px(6.))
            .border_b_1()
            .border_color(colours.border)
            .flex()
            .items_center()
            .gap_3()
            .text_size(metrics::TEXT_SMALL());
        let bar = match &transcript.notice {
            Some(notice) => {
                bar.debug_selector(|| "transcript-notice".into())
                    .relative()
                    .children(frame_ticks("transcript-notice", colours))
                    .text_color(if notice.elsewhere.is_some() {
                        colours.elsewhere
                    } else {
                        colours.warning
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(notice.text.clone()),
                    )
                    .when(notice.other_folder, |this| {
                        this.child(
                            div()
                                .id("resume-in")
                                .debug_selector(|| "transcript-resume-in".into())
                                .flex_none()
                                .cursor_pointer()
                                .text_color(colours.text)
                                .child(mono("RESUME IN...").text_color(colours.text))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pane = Pane::Main;
                                    this.run_command(Command::ResumeIn, window, cx);
                                })),
                        )
                    })
                    .children(notice.elsewhere.as_ref().map(|note| {
                        let button = |id: &'static str, label: &'static str| {
                            div()
                                .id(id)
                                .debug_selector(move || id.to_owned())
                                .flex_none()
                                .cursor_pointer()
                                .child(mono(label).text_color(colours.text))
                        };
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(button("transcript-open", "OPEN TRANSCRIPT").on_click(
                                cx.listener(|this, _, _, cx| {
                                    // The transcript is already shown: this puts
                                    // the keyboard on it.
                                    this.pane = Pane::Main;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                button("transcript-resume-anyway", "RESUME HERE ANYWAY").on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.pane = Pane::Main;
                                        this.run_command(Command::ResumeAnyway, window, cx);
                                    }),
                                ),
                            )
                            .when(note.can_reveal, |this| {
                                this.child(button("transcript-reveal", "REVEAL").on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.run_command(Command::RevealTerminal, window, cx);
                                    }),
                                ))
                            })
                    }))
            }
            None => bar
                .debug_selector(|| "transcript-hint".into())
                .text_color(colours.text_muted)
                .child(mono("RESUME IN A TERMINAL").text_color(colours.text_muted))
                .children(keys::keys_label(Command::Open).map(|text| key_cap(text, colours)))
                .child(div().flex_1()),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.transcript_body(transcript, colours)),
            )
            .into_any_element()
    }

    fn transcript_body(&self, transcript: &Transcript, colours: &Palette) -> AnyElement {
        if let Some(why) = &transcript.failed {
            return div()
                .debug_selector(|| "transcript-failed".into())
                .p_4()
                .text_color(colours.error)
                .child(format!("Could not read this session: {why}"))
                .into_any_element();
        }
        let Some(messages) = transcript.messages.clone() else {
            return div()
                .debug_selector(|| "transcript-loading".into())
                .p_4()
                .child(mono("LOADING...").text_color(colours.text_muted))
                .into_any_element();
        };
        if messages.is_empty() {
            return div()
                .debug_selector(|| "transcript-empty".into())
                .p_4()
                .text_color(colours.text_faint)
                .child("This session has no messages.")
                .into_any_element();
        }
        let colours = *colours;
        let hit = transcript.hit_index();
        div()
            .debug_selector(|| "transcript".into())
            .relative()
            .size_full()
            .child(
                list(
                    transcript.list.clone(),
                    move |index, _window, _cx| match messages.get(index) {
                        Some(message) => message_row(index, message, hit == Some(index), &colours),
                        None => div().into_any_element(),
                    },
                )
                .size_full(),
            )
            .child(Scrollbar::vertical(&transcript.list))
            .into_any_element()
    }
}

/// One message of a transcript: the speaker and time on the left in mono, the
/// text on the right. The message a search hit pointed at is marked by the
/// accent's bar.
fn message_row(index: usize, message: &Message, hit: bool, colours: &Palette) -> AnyElement {
    let (text, omitted) = format::truncate_chars(&message.text, MAX_MESSAGE_CHARS);
    let text = if omitted > 0 {
        format!("{text}\n[ {omitted} MORE CHARACTERS NOT SHOWN ]")
    } else {
        text.to_owned()
    };
    let tag_colour = match message.role {
        Role::User => colours.text,
        Role::Assistant => colours.text_muted,
        Role::Tool | Role::System => colours.text_faint,
    };
    let mono_body = matches!(message.role, Role::Tool);
    div()
        .id(("message", index))
        .debug_selector(move || format!("message-{index}"))
        .relative()
        .w_full()
        .px_4()
        .py(px(10.))
        .flex()
        .gap_4()
        .border_b_1()
        .border_color(colours.border)
        .when(hit, |this| {
            this.bg(colours.surface_2).child(
                div()
                    .debug_selector(|| "message-hit".into())
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(colours.signal),
            )
        })
        .child(
            div()
                .flex_none()
                .w(px(96.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(mono(format::role_tag(message.role)).text_color(tag_colour))
                .child(mono(format::moment(message.at)).text_color(colours.text_faint)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .when(mono_body, |this| {
                    this.font_family(fonts::mono())
                        .font_features(fonts::mono_features())
                        .text_size(metrics::TEXT_SMALL())
                })
                .text_color(if message.role == Role::Tool {
                    colours.text_muted
                } else {
                    colours.text
                })
                .child(text),
        )
        .into_any_element()
}

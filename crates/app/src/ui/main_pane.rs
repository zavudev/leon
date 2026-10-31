//! The main pane: a live terminal filling it, or the open session's
//! transcript, or the detail of the open project or worktree, and the status
//! line of the engine.

use super::activity::Activity;
use super::checkout;
use super::lines::{empty_frame, frame_ticks};
use super::live::{LiveId, LiveState};
use super::shell::{Main, Pane, Shell, Transcript};
use super::sidebar::live_light;
use super::tree::worktree_label;
use super::widgets::{
    activity_dot, checkout_colour, focus_rule, key_cap, led, mono, section_label,
};
use crate::format;
use crate::icons::{agent_icon, icon, IconName};
use crate::keys::{self, Command};
use crate::launch::Launch;
use crate::theme::{fonts, hairline, metrics, px, Palette};
use chrono::{DateTime, Utc};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    div, list, AnyElement, App, Context, Div, FontWeight, SharedString, Stateful, Window,
};
use leon_core::{AgentId, MachineId, Message, ProjectId, Role, WorktreeId};

/// The most characters of one message drawn: a transcript can hold a whole
/// file pasted by a tool, and laying that out every frame would cost more
/// than reading it is worth.
const MAX_MESSAGE_CHARS: usize = 6_000;

/// How many sessions the worktree screen lists; past this the sidebar holds
/// the rest, and the screen says how many.
const WORKTREE_SESSIONS: usize = 12;

impl Shell {
    pub(super) fn render_main_pane(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let focused = self.pane == Pane::Main;
        let (_, title, meta) = self.main_heading(cx);
        let body = match &self.main {
            Main::Empty => self.render_home(colours, cx),
            Main::Project(id) => self.render_project(id, colours, cx),
            Main::Worktree(project, worktree) => {
                self.render_worktree(project, worktree, colours, cx)
            }
            Main::Session(transcript) => self.render_transcript(transcript, colours, cx),
            Main::Live(id) => self.render_live(*id, colours, cx),
            Main::Den => self.render_den(colours, cx),
            Main::Tasks => self.render_tasks(colours, cx),
            Main::King => self.render_king(colours, cx),
        };
        let agent = self.main_agent();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child({
                // One line: the toggles at the left, and in the middle what is
                // open as a prompt reads: its icon, its name, where it is and
                // how it is doing. The whole line about it is the tooltip.
                let (_, path, tail) = self.meta_parts(meta.clone());
                let (place, state) = crumb_parts(&path, &tail);
                // A project named after its folder is not said twice.
                let place = place.filter(|place| *place != title);
                let tip: SharedString = meta.into();
                div()
                    .debug_selector(|| "main-header".into())
                    .relative()
                    .flex_none()
                    .h(metrics::HEADER_HEIGHT())
                    .px_4()
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .debug_selector(|| "header-title-row".into())
                            .flex_1()
                            .min_w_0()
                            .h(metrics::HEADER_TITLE_LINE())
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.sidebar_toggle_button("sidebar-toggle-main", colours, cx))
                            .child(self.files_toggle_button(colours, cx))
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
                            }),
                    )
                    .child(
                        div()
                            .id("header-crumb")
                            .debug_selector(|| "header-crumb".into())
                            .flex_none()
                            .max_w(metrics::HEADER_CRUMB_MAX())
                            .h(metrics::HEADER_TITLE_LINE() + px(8.))
                            .px(px(12.))
                            .border_1()
                            .border_color(colours.border)
                            .rounded(metrics::RADIUS())
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .gap_2()
                            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                            .children(
                                self.main_project().map(|id| {
                                    self.logo(&id, metrics::PROJECT_ICON(), "main", colours)
                                }),
                            )
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
                            .children(place.map(|place| {
                                div()
                                    .debug_selector(|| "header-place".into())
                                    .flex_shrink(1.)
                                    .min_w_0()
                                    .truncate()
                                    .font_family(fonts::mono())
                                    .font_features(fonts::mono_features())
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(colours.text_faint)
                                    .child(format!("@ {place}"))
                            }))
                            .children(state.map(|(word, live)| {
                                div()
                                    .debug_selector(|| "header-state".into())
                                    .flex_none()
                                    .whitespace_nowrap()
                                    .font_family(fonts::mono())
                                    .font_features(fonts::mono_features())
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(if live {
                                        colours.signal
                                    } else {
                                        colours.text_muted
                                    })
                                    .child(word)
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .justify_end()
                            .items_center()
                            .children(self.header_usage_chip(colours, cx)),
                    )
                    .when(focused, |this| this.child(focus_rule(colours)))
            })
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .child(self.render_status_bar(colours, cx))
    }

    /// The header's metadata as one line, for tests.
    #[cfg(test)]
    #[cfg_attr(not(leon_posix_tests), allow(dead_code))] // used by the Unix-only tests
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
            Main::Live(id) => match (self.files.get(id), self.changes.get(id)) {
                (Some(doc), _) => Some(doc.path.clone()),
                (None, Some(view)) => Some(view.target.path.clone()),
                (None, None) => self
                    .live
                    .get(*id)
                    .map(|session| session.cwd.clone())
                    .or_else(|| self.chats.get(id).map(|chat| chat.cwd.clone())),
            },
            Main::Session(transcript) => Some(transcript.session.cwd.clone()),
            Main::Empty | Main::Den | Main::Tasks | Main::King => None,
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
    fn main_agent(&self) -> Option<AgentId> {
        match &self.main {
            Main::Session(transcript) => Some(transcript.session.agent),
            Main::Live(id) => self
                .live
                .get(*id)
                .and_then(|session| session.shown_agent())
                .or_else(|| self.chats.get(id).map(|chat| chat.agent)),
            _ => None,
        }
    }

    /// What the header says: the kind of thing, its name and a line about it.
    pub(super) fn main_heading(&self, cx: &App) -> (&'static str, String, String) {
        match &self.main {
            Main::Empty => ("Home", "Home".to_owned(), self.home_headline()),
            Main::Den => self.den_heading(),
            Main::King => self.king_heading(),
            Main::Tasks => {
                let (kind, title, meta) = self.tasks_heading();
                (kind, title, meta)
            }
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
            Main::Live(id) if self.files.contains_key(id) => self.file_heading(&self.files[id]),
            Main::Live(id) if self.changes.contains_key(id) => {
                self.changes_heading(&self.changes[id])
            }
            Main::Live(id) if self.chats.contains_key(id) => {
                let chat = &self.chats[id];
                let who = format::agent_tag(chat.agent).to_owned()
                    + &chat
                        .account
                        .as_deref()
                        .map(|account| format!(" \u{b7} {}", format::account_tag(account)))
                        .unwrap_or_default();
                let machine = self
                    .snapshot
                    .machine(&chat.machine)
                    .map_or_else(String::new, |machine| machine.name.to_uppercase());
                (
                    "Session",
                    chat.label(),
                    format!("{who} \u{b7} {machine} \u{b7} {}", chat.cwd),
                )
            }
            Main::Live(id) => match self.live.get(*id) {
                Some(session) => {
                    let who = session.agent.map_or("SHELL", format::agent_tag).to_owned()
                        + &session
                            .account
                            .as_deref()
                            .filter(|_| session.agent.is_some())
                            .map_or_else(String::new, |account| {
                                format!(" · {}", format::account_tag(account))
                            });
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
                        "{}{} · {} MESSAGES · {}{held}",
                        format::agent_tag(session.agent),
                        session
                            .account
                            .as_deref()
                            .map_or_else(String::new, |account| {
                                format!(" · {}", format::account_tag(account))
                            }),
                        session.message_count,
                        session.cwd
                    ),
                )
            }
        }
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
                    .truncate()
                    .child(value),
            )
    }

    /// A project's screen: the question of what to build in it, over a field
    /// that starts a session there, and the ways to start something else; its
    /// facts in small print under them.
    fn render_project(
        &self,
        id: &leon_core::ProjectId,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let root = entry.project.root.clone();
        let on = |machine: &MachineId, cwd: &str| {
            machine == &entry.project.machine_id && leon_core::path::is_within(cwd, &root)
        };
        let sessions = self
            .snapshot
            .sessions
            .iter()
            .filter(|session| on(&session.machine_id, &session.cwd))
            .count();
        let running = self
            .live
            .all()
            .iter()
            .filter(|live| on(&live.machine, &live.cwd))
            .count();
        let name = self.project_label(&entry.project);
        let project_id = entry.project.id.clone();
        let hover = colours.surface;
        let mono_text = |text: String| {
            div()
                .font_family(fonts::mono())
                .font_features(fonts::mono_features())
                .child(text)
        };
        let chip = |id: &'static str,
                    lead: AnyElement,
                    text: &'static str,
                    tip: String,
                    run: Box<Run>,
                    cx: &mut Context<Self>| {
            let tip: SharedString = tip.into();
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex_none()
                .h(metrics::CONTROL())
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(6.))
                .border_1()
                .border_color(colours.border)
                .rounded(metrics::RADIUS())
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .font_family(fonts::mono())
                .font_features(fonts::mono_features())
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text_muted)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pane = Pane::Main;
                    run(this, window, cx);
                }))
                .child(lead)
                .child(text)
        };
        let new_worktree = project_id.clone();
        div()
            .debug_selector(|| "project-detail".into())
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(super::widgets::mark(
                metrics::HERO_MARK(),
                colours,
                cx,
                super::widgets::Lion::new("project-mark").mood(self.mark_mood()),
            ))
            .child(
                div()
                    .debug_selector(|| "project-question".into())
                    .flex()
                    .items_baseline()
                    .text_size(metrics::TEXT_HERO())
                    .child(mono_text("what are we building in ".to_owned()))
                    .child(mono_text(name).text_color(colours.signal))
                    .child(mono_text("?".to_owned())),
            )
            .child(
                mono_text(format!(
                    "{} worktree{} \u{b7} {sessions} session{} \u{b7} {running} running",
                    entry.worktrees.len(),
                    if entry.worktrees.len() == 1 { "" } else { "s" },
                    if sessions == 1 { "" } else { "s" },
                ))
                .debug_selector(|| "project-counts".into())
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text_faint),
            )
            .child(prompt_field(
                "project-prompt",
                "start a session here",
                super::sidebar::tooltip_text(Command::NewSession),
                colours,
                cx,
                |this, window, cx| {
                    // The flow reads its target from what is open.
                    this.pane = Pane::Main;
                    this.run_command(Command::NewSession, window, cx);
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .child(chip(
                        "project-new-worktree",
                        mono_text("\u{2387}".to_owned()).into_any_element(),
                        "new worktree",
                        super::sidebar::tooltip_text(Command::NewWorktree),
                        Box::new(move |this, window, cx| {
                            this.open_new_worktree_for(&new_worktree, window, cx)
                        }),
                        cx,
                    ))
                    .child(chip(
                        "project-ask-king",
                        icon(IconName::Crown, px(12.), colours.signal).into_any_element(),
                        "ask the orchestrator to plan it",
                        super::sidebar::tooltip_text(Command::ShowKing),
                        Box::new(|this, window, cx| {
                            this.run_command(Command::ShowKing, window, cx);
                        }),
                        cx,
                    ))
                    .child(chip(
                        "project-open-shell",
                        mono_text("$_".to_owned()).into_any_element(),
                        "open a shell",
                        super::sidebar::tooltip_text(Command::OpenShell),
                        Box::new(|this, window, cx| {
                            this.run_command(Command::OpenShell, window, cx);
                        }),
                        cx,
                    )),
            )
            .child(
                div()
                    .debug_selector(|| "project-facts".into())
                    .mt_6()
                    .w_full()
                    .max_w(metrics::HEADER_CRUMB_MAX() + px(220.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_muted)
                    .child(Self::field("PATH", entry.project.root.clone(), colours))
                    .child(Self::field("MACHINE", machine, colours))
                    .child(Self::field(
                        "WORKTREES",
                        entry.worktrees.len().to_string(),
                        colours,
                    ))
                    .child(div().pt_2().child(section_label("Worktrees", colours)))
                    .child(worktrees),
            )
            .into_any_element()
    }

    /// The worktree screen: who it is, what can be done with it, the sessions
    /// that ran here and the details. Every action works on the worktree on
    /// screen, not on the tree's cursor.
    fn render_worktree(
        &self,
        project: &leon_core::ProjectId,
        worktree: &leon_core::WorktreeId,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.snapshot.project(project) else {
            return div().into_any_element();
        };
        let Some(wt) = entry.worktrees.iter().find(|w| &w.id == worktree) else {
            return div().into_any_element();
        };
        let now = self.now();
        let machine = self
            .snapshot
            .machine(&entry.project.machine_id)
            .map_or_else(String::new, |machine| machine.name.to_uppercase());
        let activity = self.worktree_activity(&wt.id);
        let places = self.placement.of_worktree(&wt.id);
        let live = places
            .iter()
            .filter(|place| self.running_here(&self.snapshot.sessions[**place].id))
            .count();
        let local = entry.project.machine_id.is_local();

        // ----- the hero: what to build on this branch, where it is and how
        // its terminals are, over the field that starts a session here.
        let mono_text = |text: String| {
            div()
                .font_family(fonts::mono())
                .font_features(fonts::mono_features())
                .child(text)
        };
        let hero = div()
            .debug_selector(|| "worktree-hero".into())
            .flex_none()
            .px_4()
            .pt(px(36.))
            .pb_3()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .text_size(metrics::TEXT_HERO())
                    .child(mono_text("what are we building on ".to_owned()))
                    .child(mono_text(worktree_label(wt)).text_color(colours.signal))
                    .child(mono_text("?".to_owned())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint)
                    .child(self.logo(project, metrics::PROJECT_ICON(), "worktree", colours))
                    .child(mono_text(format!(
                        "{} \u{b7} {} \u{b7} {}",
                        self.project_label(&entry.project).to_lowercase(),
                        if wt.is_main {
                            "main checkout"
                        } else {
                            "linked"
                        },
                        machine.to_lowercase(),
                    )))
                    .child(activity_dot(activity, colours))
                    .child(mono_text(activity_word(activity).to_lowercase())),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .pt_2()
                    .child(prompt_field(
                        "worktree-new-session",
                        "start a session on this branch",
                        super::sidebar::tooltip_text(Command::NewSession),
                        colours,
                        cx,
                        |this, window, cx| {
                            // The flow reads its target from what is open.
                            this.pane = Pane::Main;
                            this.run_command(Command::NewSession, window, cx);
                        },
                    )),
            );

        // ----- the actions: one accent call to action, then the rest.
        let mut actions = div()
            .debug_selector(|| "worktree-actions".into())
            .flex_none()
            .px_4()
            .pb_3()
            .border_b_1()
            .border_color(colours.border)
            .flex()
            .flex_wrap()
            .justify_center()
            .items_center()
            .gap_2();
        // The agents this machine has (the catalogue's, not a fixed three);
        // a machine nobody probed yet shows the built-in ones.
        for agent in self.agents_for_buttons(&entry.project.machine_id) {
            let id = format!("worktree-new-{}", agent.as_str());
            let tip = format!("New {} session here", format::agent_name(agent));
            actions = actions.child(worktree_button(
                id,
                tip,
                colours,
                cx,
                agent_icon(agent, px(16.), colours).into_any_element(),
                false,
                move |this, window, cx| {
                    this.start_in_open_worktree(
                        Launch::Agent {
                            kind: agent,
                            resume: None,
                            account: None,
                        },
                        window,
                        cx,
                    );
                },
            ));
        }
        actions = actions
            .child(worktree_divider(colours))
            .child(worktree_button(
                "worktree-changes",
                super::sidebar::tooltip_text(Command::OpenChanges),
                colours,
                cx,
                action_label("changes")
                    .text_color(colours.text_muted)
                    .into_any_element(),
                false,
                |this, window, cx| this.open_changes_of_open_worktree(window, cx),
            ))
            .child(worktree_button(
                "worktree-shell",
                super::sidebar::tooltip_text(Command::OpenShell),
                colours,
                cx,
                action_label("$_ open a shell")
                    .text_color(colours.text_muted)
                    .into_any_element(),
                false,
                |this, window, cx| this.start_in_open_worktree(Launch::Shell, window, cx),
            ))
            .child(worktree_divider(colours))
            .child(worktree_button(
                "worktree-copy-path",
                super::sidebar::tooltip_text(Command::CopyPath),
                colours,
                cx,
                action_label("copy path")
                    .text_color(colours.text_muted)
                    .into_any_element(),
                false,
                |this, _, cx| this.copy_open_worktree_path(cx),
            ))
            .child(worktree_button(
                "worktree-copy-branch",
                super::sidebar::tooltip_text(Command::CopyBranch),
                colours,
                cx,
                action_label("copy branch")
                    .text_color(colours.text_muted)
                    .into_any_element(),
                false,
                |this, _, cx| this.copy_open_worktree_branch(cx),
            ));
        if local {
            actions = actions.child(worktree_button(
                "worktree-reveal",
                super::sidebar::tooltip_text(Command::Reveal),
                colours,
                cx,
                action_label("reveal")
                    .text_color(colours.text_muted)
                    .into_any_element(),
                false,
                |this, _, cx| this.reveal_open_worktree(cx),
            ));
        }
        actions = actions
            .child(worktree_divider(colours))
            .child(worktree_button(
                "worktree-new-worktree",
                super::sidebar::tooltip_text(Command::NewWorktree),
                colours,
                cx,
                action_label("new worktree")
                    .text_color(colours.text_muted)
                    .into_any_element(),
                false,
                |this, window, cx| this.new_worktree_in_open_project(window, cx),
            ));
        if leon_core::cleanup::offered(wt) {
            actions = actions.child(worktree_button(
                "worktree-remove-merged",
                super::sidebar::tooltip_text(Command::RemoveMergedWorktrees),
                colours,
                cx,
                action_label("remove merged")
                    .text_color(colours.success)
                    .into_any_element(),
                false,
                |this, window, cx| this.begin_flow(Command::RemoveMergedWorktrees, window, cx),
            ));
        }
        if !wt.is_main {
            actions = actions.child(worktree_button(
                "worktree-remove",
                super::sidebar::tooltip_text(Command::RemoveWorktree),
                colours,
                cx,
                action_label("remove worktree")
                    .text_color(colours.error)
                    .into_any_element(),
                false,
                |this, window, cx| this.remove_open_worktree(window, cx),
            ));
        }

        // ----- the sessions that ran here, newest first. Terminals running
        // here go first, even when they did not resume history: the sidebar
        // shows them under this worktree too, and one that resumed a history
        // session of its own folder has no row of its own (that session's
        // row below is it).
        let running: Vec<(LiveId, Option<AgentId>)> = self
            .placement
            .live_of_worktree(&wt.id)
            .iter()
            .filter_map(|place| {
                let entry = &self.placement.live[*place];
                let merged = entry
                    .history
                    .as_ref()
                    .is_some_and(|history| self.placement.merged.contains(history));
                (!merged && self.live.get(entry.id).is_some()).then_some((entry.id, entry.agent))
            })
            .collect();
        let mut list = div().flex().flex_col();
        if places.is_empty() && running.is_empty() {
            list = list.child(empty_frame(
                "worktree-empty",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .debug_selector(|| "worktree-no-sessions".into())
                            .text_color(colours.text_muted)
                            .child("No sessions ran in this worktree yet."),
                    )
                    .child(
                        div()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_faint)
                            .child("Start one with the buttons above, or open a shell here."),
                    ),
                Some("0 SESSIONS".to_owned()),
                colours,
            ));
        } else {
            for (index, (id, agent)) in running.iter().enumerate() {
                list = list.child(self.worktree_live_row(index, *id, *agent, colours, cx));
            }
            for (offset, place) in places.iter().take(WORKTREE_SESSIONS).enumerate() {
                list = list.child(self.worktree_session_row(
                    running.len() + offset,
                    &self.snapshot.sessions[*place],
                    now,
                    colours,
                    cx,
                ));
            }
            if places.len() > WORKTREE_SESSIONS {
                list = list.child(
                    div()
                        .debug_selector(|| "worktree-more-sessions".into())
                        .px_2()
                        .py(px(6.))
                        .child(
                            mono(format!(
                                "{} MORE IN THE SIDEBAR",
                                places.len() - WORKTREE_SESSIONS
                            ))
                            .text_color(colours.text_faint),
                        ),
                );
            }
        }
        let sessions = self
            .card("worktree-sessions", colours)
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(section_label("Sessions", colours))
                    .child(
                        mono((places.len() + running.len()).to_string())
                            .text_color(colours.text_faint),
                    )
                    .child(div().flex_1())
                    .children((live + running.len() > 0).then(|| {
                        mono(format!("{} LIVE", live + running.len())).text_color(colours.success)
                    })),
            )
            .child(list);

        // ----- the state of the checkout: changes, upstream, pull request.
        let checkout = self.worktree_checkout(&wt.id, colours, cx);

        // ----- the details: where it is and what it is.
        let details = self
            .card("worktree-fields", colours)
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(section_label("Details", colours))
            .child(Self::field(
                "PROJECT",
                self.project_label(&entry.project),
                colours,
            ))
            .child(Self::field("MACHINE", machine, colours))
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
            ));

        div()
            .debug_selector(|| "worktree-detail".into())
            .size_full()
            .flex()
            .flex_col()
            .child(hero)
            .child(actions)
            .child(
                div()
                    .id("worktree-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .py_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(checkout)
                    .child(sessions)
                    .child(details),
            )
            .into_any_element()
    }

    /// What is known about the checkout of a worktree, in words: the changed
    /// files, the distance from the upstream and the open pull request, whose
    /// number opens it. `None` when nothing is known, which is how a machine
    /// without git or `gh` looks: the card is simply not there.
    fn worktree_checkout(
        &self,
        worktree: &WorktreeId,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let status = self.snapshot.status(worktree)?;
        let row = |label: &'static str, content: AnyElement| {
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
                        .debug_selector(move || format!("checkout-{label}"))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(content),
                )
        };
        let mut card = self
            .card("worktree-checkout", colours)
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(section_label("State", colours));
        let mut any = false;
        if let Some(changed) = status.changed {
            any = true;
            let tone = if changed > 0 {
                checkout::Tone::Warning
            } else {
                checkout::Tone::Muted
            };
            card = card.child(row(
                "CHANGES",
                div()
                    .text_color(checkout_colour(tone, colours))
                    .child(checkout::changes_words(changed))
                    .into_any_element(),
            ));
        }
        if let Some(divergence) = status.divergence {
            any = true;
            card = card.child(row(
                "UPSTREAM",
                div()
                    .child(checkout::upstream_words(
                        divergence.ahead,
                        divergence.behind,
                    ))
                    .into_any_element(),
            ));
        }
        if let Some(pull_request) = &status.pull_request {
            any = true;
            let tone = checkout::pull_request_tone(pull_request);
            let words: SharedString = checkout::pull_request_words(pull_request).into();
            let colour = checkout_colour(tone, colours);
            let content = match checkout::openable_url(pull_request) {
                Some(url) => {
                    let url = url.to_owned();
                    let tip: SharedString = format!("Open {url}").into();
                    let hover = colours.surface_2;
                    div()
                        .id("checkout-pull-request")
                        .debug_selector(|| "checkout-pull-request".into())
                        .min_w_0()
                        .truncate()
                        .cursor_pointer()
                        .rounded(metrics::RADIUS())
                        .hover(move |style| style.bg(hover))
                        .text_color(colour)
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            (this.options.open_url)(cx, &url);
                        }))
                        .child(words)
                        .into_any_element()
                }
                None => div()
                    .min_w_0()
                    .truncate()
                    .text_color(colour)
                    .child(words)
                    .into_any_element(),
            };
            card = card.child(row("PULL REQUEST", content));
        }
        any.then_some(card)
    }

    /// The agents offered as buttons on the worktree screen: those installed on
    /// the machine, else (nothing is known of it) the built-in three.
    pub(super) fn agents_for_buttons(&self, machine: &MachineId) -> Vec<AgentId> {
        self.installed_agents()
            .into_iter()
            .find(|(id, _)| id == machine)
            .map(|(_, agents)| agents)
            .unwrap_or_else(|| vec![AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE])
    }

    /// Whether a terminal of Leon runs this history session in its own
    /// folder: what the sidebar marks with a green light.
    fn running_here(&self, id: &leon_core::SessionId) -> bool {
        (self.live.of_history(id).is_some() || self.chat_of_history(id).is_some())
            && self.placement.merged.contains(id)
    }

    /// One terminal running in the worktree screen: its agent, label and
    /// state. A click focuses it; it has no transcript to open.
    fn worktree_live_row(
        &self,
        index: usize,
        id: LiveId,
        agent: Option<AgentId>,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (label, state) = self.live_row(id, cx);
        let lead = match agent {
            Some(agent) => agent_icon(agent, px(14.), colours).into_any_element(),
            None => icon(IconName::Terminal, px(14.), colours.text_muted).into_any_element(),
        };
        let hover = colours.surface_2;
        div()
            .id(("worktree-live", index))
            .debug_selector(move || format!("worktree-live-{id}"))
            .h(px(34.))
            .px_2()
            .flex()
            .items_center()
            .gap_3()
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_live(id, window, cx);
            }))
            .child(lead)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(label),
            )
            .when(!matches!(state, LiveState::Running), |this| {
                this.child(
                    mono(state.label())
                        .debug_selector(move || format!("worktree-live-state-{id}"))
                        .text_color(colours.text_faint),
                )
            })
            .child(
                div()
                    .debug_selector(move || format!("worktree-live-led-{id}"))
                    .child(led(live_light(state, colours))),
            )
    }

    /// One session of the worktree screen: its agent, title, model, size and
    /// age. A click opens the stored transcript; nothing is started.
    fn worktree_session_row(
        &self,
        index: usize,
        session: &leon_core::Session,
        now: DateTime<Utc>,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let running = self.running_here(&session.id);
        // Held by a terminal outside Leon: the row says so rather than offer
        // it as free; a session restored but not resumed yet says it waits.
        let elsewhere = !running && self.elsewhere_of(session).is_some();
        let paused = self
            .live
            .of_history(&session.id)
            .is_some_and(|live| live.is_paused());
        let hover = colours.surface_2;
        let session = session.clone();
        let opened = session.clone();
        div()
            .id(("worktree-session", index))
            .debug_selector(move || format!("worktree-session-{index}"))
            .h(px(34.))
            .px_2()
            .flex()
            .items_center()
            .gap_3()
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_session(opened.clone(), None, cx);
            }))
            .child(
                div()
                    .flex_none()
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(agent_icon(session.agent, px(14.), colours)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(session.title.clone()),
            )
            .children(session.model.as_ref().map(|model| {
                div()
                    .flex_none()
                    .max_w(px(160.))
                    .truncate()
                    .child(mono(model.clone()).text_color(colours.text_faint))
            }))
            .child(
                mono(format!("{} MESSAGES", session.message_count)).text_color(colours.text_faint),
            )
            .when(paused, |this| {
                this.child(mono("PAUSED").text_color(colours.warning))
            })
            .when(elsewhere, |this| {
                this.child(
                    mono("ELSEWHERE")
                        .debug_selector(move || format!("worktree-session-elsewhere-{index}"))
                        .text_color(colours.elsewhere),
                )
            })
            .when(running && !paused, |this| this.child(led(colours.success)))
            .child(mono(format::age(now, session.updated_at)).text_color(colours.text_faint))
    }

    /// The worktree the main pane is showing.
    fn shown_worktree(&self) -> Option<OpenWorktree> {
        let Main::Worktree(project, worktree) = &self.main else {
            return None;
        };
        let entry = self.snapshot.project(project)?;
        let found = entry.worktrees.iter().find(|found| &found.id == worktree)?;
        Some(OpenWorktree {
            machine: entry.project.machine_id.clone(),
            project: project.clone(),
            worktree: worktree.clone(),
            path: found.path.clone(),
            branch: found.branch.clone(),
        })
    }

    /// Starts a shell or an agent in the worktree on screen.
    fn start_in_open_worktree(
        &mut self,
        launch: Launch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.shown_worktree() else {
            return;
        };
        self.start_new(
            launch,
            &open.machine,
            &open.path,
            super::terminals::Place::Session,
            window,
            cx,
        );
    }

    /// Opens the changes of the worktree on screen in a tab.
    fn open_changes_of_open_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.shown_worktree() {
            self.open_changes(
                super::changes::Target {
                    machine: open.machine,
                    project: open.project,
                    worktree: Some(open.worktree),
                    path: open.path,
                },
                window,
                cx,
            );
        }
    }

    fn copy_open_worktree_path(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.shown_worktree() {
            self.copy_text("path", open.path, cx);
        }
    }

    fn copy_open_worktree_branch(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.shown_worktree() else {
            return;
        };
        match open.branch {
            Some(branch) => self.copy_text("branch name", branch, cx),
            None => self.engine.report(
                crate::engine::StatusKind::Info,
                "This worktree has no branch: its head is detached.",
            ),
        }
    }

    fn reveal_open_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.shown_worktree() else {
            return;
        };
        if open.machine.is_local() {
            let reveal = self.options.reveal.clone();
            reveal(cx, std::path::Path::new(&open.path));
        } else {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "Showing a folder in the file manager only works on this computer.",
            );
        }
    }

    fn new_worktree_in_open_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.shown_worktree() {
            self.begin_flow_with(
                Command::NewWorktree,
                vec![open.project.as_str().to_owned()],
                window,
                cx,
            );
        }
    }

    fn remove_open_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.shown_worktree() {
            self.begin_flow_with(
                Command::RemoveWorktree,
                vec![format!("{}|{}", open.project, open.worktree)],
                window,
                cx,
            );
        }
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
                            .when(note.can_take_over, |this| {
                                this.child(button("transcript-take-over", "TAKE OVER").on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.pane = Pane::Main;
                                        this.run_command(Command::TakeOver, window, cx);
                                    }),
                                ))
                            })
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

/// The worktree the main pane shows, and the little its actions need from it.
struct OpenWorktree {
    machine: MachineId,
    project: ProjectId,
    worktree: WorktreeId,
    path: String,
    branch: Option<String>,
}

/// A button of the worktree screen: a hairline frame, a mono label or an
/// agent's mark, and its shortcut in the tooltip. `primary` fills it with the
/// accent, for the one call to action of the screen.
fn worktree_button(
    id: impl Into<SharedString>,
    tip: String,
    colours: &Palette,
    cx: &mut Context<Shell>,
    content: AnyElement,
    primary: bool,
    run: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
) -> Stateful<Div> {
    let hover = colours.surface_2;
    let tip: SharedString = tip.into();
    let id: SharedString = id.into();
    let selector = id.to_string();
    let button = div()
        .id(id)
        .debug_selector(move || selector.clone())
        .flex_none()
        .h(metrics::CONTROL())
        .px(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .rounded(metrics::RADIUS())
        .cursor_pointer()
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(cx.listener(move |this, _, window, cx| run(this, window, cx)))
        .child(content);
    if primary {
        button
            .bg(colours.primary_fill)
            .text_color(colours.on_primary)
    } else {
        button
            .border_1()
            .border_color(colours.border)
            .hover(move |style| style.bg(hover))
    }
}

/// The thin rule between two groups of the worktree screen's actions.
fn worktree_divider(colours: &Palette) -> Div {
    div()
        .flex_none()
        .w(hairline())
        .h(px(18.))
        .bg(colours.border)
}

/// The word next to a worktree's activity dot.
fn activity_word(activity: Activity) -> &'static str {
    match activity {
        Activity::Off => "NO LIVE SESSION",
        Activity::Idle => "IDLE",
        Activity::Working => "WORKING",
        Activity::TurnOver => "READY",
        Activity::Waiting => "WAITING",
        Activity::NeedsYou => "ASKS",
        Activity::Failed => "FAILED",
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

/// The words of an action of a screen, in lower-case mono as a command is
/// typed.
fn action_label(text: &'static str) -> Div {
    div()
        .flex_none()
        .font_family(fonts::mono())
        .font_features(fonts::mono_features())
        .text_size(metrics::TEXT_SMALL())
        .child(text)
}

/// The field of an empty screen that starts something there, drawn as a
/// prompt: `> ▌ what it does` with the accent's run button at its end and
/// the accent's corner ticks. A click runs `run`.
pub(super) fn prompt_field(
    id: &'static str,
    words: &'static str,
    tip: String,
    colours: &Palette,
    cx: &mut Context<Shell>,
    run: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
) -> Stateful<Div> {
    let tip: SharedString = tip.into();
    let mono_text = |text: &'static str| {
        div()
            .font_family(fonts::mono())
            .font_features(fonts::mono_features())
            .child(text)
    };
    div()
        .id(id)
        .debug_selector(move || id.into())
        .relative()
        .w_full()
        .max_w(metrics::HEADER_CRUMB_MAX() + px(220.))
        .h(metrics::CONTROL() + px(18.))
        .px_4()
        .flex()
        .items_center()
        .gap_3()
        .border_1()
        .border_color(colours.border)
        .bg(colours.surface)
        .cursor_pointer()
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(cx.listener(move |this, _, window, cx| run(this, window, cx)))
        .children(super::lines::focus_ticks(id, colours))
        .child(mono_text(">").text_color(colours.signal))
        .child(div().flex_none().w(px(8.)).h(px(16.)).bg(colours.signal))
        .child(
            mono_text(words)
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(colours.text_faint),
        )
        .child(
            div()
                .flex_none()
                .h(metrics::CONTROL())
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(metrics::RADIUS())
                .bg(colours.primary_fill)
                .child(mono("\u{23ce} RUN").text_color(colours.on_primary)),
        )
}

/// What a button of a screen does when it is clicked.
type Run = dyn Fn(&mut Shell, &mut Window, &mut Context<Shell>);

/// What the header's line says after the name: the folder it is in (`@ atlas`)
/// and its first state word in lower case, with whether that word is one of
/// work under way (drawn in the accent). Both come from the metadata split
/// around its path ([`Shell::meta_parts`]).
pub(super) fn crumb_parts(path: &str, tail: &str) -> (Option<String>, Option<(String, bool)>) {
    let place = (!path.is_empty()).then(|| super::tree::folder_name(path).to_owned());
    let state = tail
        .split('\u{b7}')
        .map(str::trim)
        .find(|word| !word.is_empty())
        .map(|word| {
            let word = word.to_lowercase();
            let live = matches!(word.as_str(), "running" | "starting" | "working");
            (word, live)
        });
    (place, state)
}

#[cfg(test)]
mod crumb_tests {
    use super::crumb_parts;

    #[test]
    fn the_crumb_says_the_folder_and_the_first_state_word() {
        let (place, state) = crumb_parts("/home/ada/code/atlas", " \u{b7} RUNNING \u{b7} RESUMED");
        assert_eq!(place.as_deref(), Some("atlas"));
        assert_eq!(state, Some(("running".to_owned(), true)));
    }

    #[test]
    fn a_finished_state_is_not_drawn_as_work_and_no_path_means_no_place() {
        let (place, state) = crumb_parts("", " \u{b7} EXITED 1");
        assert_eq!(place, None);
        assert_eq!(state, Some(("exited 1".to_owned(), false)));
        assert_eq!(crumb_parts("/srv/api/", ""), (Some("api".to_owned()), None));
    }
}

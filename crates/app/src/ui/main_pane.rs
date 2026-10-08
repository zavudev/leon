//! The main pane: a live terminal filling it, or the open session's
//! transcript, or the detail of the open project or worktree, and the status
//! line of the engine.

use super::activity::Activity;
use super::lines::{empty_frame, frame_ticks};
use super::live::{LiveId, LiveState};
use super::shell::{Main, Pane, Shell, Transcript};
use super::sidebar::live_light;
use super::tree::worktree_label;
use super::widgets::{activity_dot, focus_rule, key_cap, led, mono, section_label};
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
        let (kind, title, meta) = self.main_heading(cx);
        let body = match &self.main {
            Main::Empty => self.render_home(colours, cx),
            Main::Project(id) => self.render_project(id, colours),
            Main::Worktree(project, worktree) => {
                self.render_worktree(project, worktree, colours, cx)
            }
            Main::Session(transcript) => self.render_transcript(transcript, colours, cx),
            Main::Live(id) => self.render_live(*id, colours, cx),
            Main::Den => self.render_den(colours, cx),
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
            Main::Live(id) => match self.files.get(id) {
                Some(doc) => Some(doc.path.clone()),
                None => self.live.get(*id).map(|session| session.cwd.clone()),
            },
            Main::Session(transcript) => Some(transcript.session.cwd.clone()),
            Main::Empty | Main::Den => None,
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
            Main::Live(id) => self.live.get(*id).and_then(|session| session.shown_agent()),
            _ => None,
        }
    }

    /// What the header says: the kind of thing, its name and a line about it.
    pub(super) fn main_heading(&self, cx: &App) -> (&'static str, String, String) {
        match &self.main {
            Main::Empty => ("Home", "Home".to_owned(), self.home_headline()),
            Main::Den => self.den_heading(),
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

        // ----- the hero: the project, the branch and how the terminals are.
        let hero = div()
            .debug_selector(|| "worktree-hero".into())
            .flex_none()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(colours.border)
            .flex()
            .items_center()
            .gap_3()
            .child(self.logo(project, px(24.), "worktree", colours))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(self.project_label(&entry.project)),
                            )
                            .child(
                                mono(worktree_label(wt))
                                    .px(px(6.))
                                    .py(px(1.))
                                    .rounded(metrics::RADIUS())
                                    .border_1()
                                    .border_color(colours.elevated_border)
                                    .text_color(colours.text),
                            )
                            .child(
                                mono(if wt.is_main { "MAIN" } else { "LINKED" })
                                    .text_color(colours.text_faint),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_faint)
                            .child(format!("{machine} \u{b7} {}", wt.path)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(activity_dot(activity, colours))
                    .child(mono(activity_word(activity)).text_color(colours.text_faint)),
            );

        // ----- the actions: one accent call to action, then the rest.
        let mut actions = div()
            .debug_selector(|| "worktree-actions".into())
            .flex_none()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(colours.border)
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(worktree_button(
                "worktree-new-session",
                super::sidebar::tooltip_text(Command::NewSession),
                colours,
                cx,
                mono("NEW SESSION")
                    .text_color(colours.on_primary)
                    .into_any_element(),
                true,
                |this, window, cx| {
                    // The flow reads its target from what is open.
                    this.pane = Pane::Main;
                    this.run_command(Command::NewSession, window, cx);
                },
            ));
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
                "worktree-shell",
                super::sidebar::tooltip_text(Command::OpenShell),
                colours,
                cx,
                mono("OPEN SHELL")
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
                mono("COPY PATH")
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
                mono("COPY BRANCH")
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
                mono("REVEAL")
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
                mono("NEW WORKTREE")
                    .text_color(colours.text_muted)
                    .into_any_element(),
                false,
                |this, window, cx| this.new_worktree_in_open_project(window, cx),
            ));
        if !wt.is_main {
            actions = actions.child(worktree_button(
                "worktree-remove",
                super::sidebar::tooltip_text(Command::RemoveWorktree),
                colours,
                cx,
                mono("REMOVE WORKTREE")
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
                    .child(sessions)
                    .child(details),
            )
            .into_any_element()
    }

    /// The agents offered as buttons on the worktree screen: those installed on
    /// the machine, else (nothing is known of it) the built-in three.
    fn agents_for_buttons(&self, machine: &MachineId) -> Vec<AgentId> {
        self.installed_agents()
            .into_iter()
            .find(|(id, _)| id == machine)
            .map(|(_, agents)| agents)
            .unwrap_or_else(|| vec![AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE])
    }

    /// Whether a terminal of Leon runs this history session in its own
    /// folder: what the sidebar marks with a green light.
    fn running_here(&self, id: &leon_core::SessionId) -> bool {
        self.live.of_history(id).is_some() && self.placement.merged.contains(id)
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
        let (label, state) = self.live.get(id).map_or_else(
            || (String::new(), LiveState::Starting),
            |session| (session.label(), session.state(cx)),
        );
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
        self.start_live(
            launch,
            &open.machine,
            &open.path,
            super::terminals::Place::Session,
            None,
            window,
            cx,
        );
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
        Activity::Waiting => "WAITING",
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

//! The cleaner, in the window: the overlay the broom beside the project filter
//! opens, with the linked worktrees (what each one weighs, why it is suggested
//! or kept) and the old sessions of the history, each list filtered by what is
//! typed and ticked row by row.
//!
//! The rules are in `sweep.rs`. Opening the overlay measures every linked
//! worktree in the background ([`Engine::measure_folder`]), all at once, and
//! each size shows as it arrives; a size measured before is shown until its new
//! one comes. Removing goes through [`Shell::remove_worktrees`], the same as
//! "Remove merged worktrees": `git worktree remove` without `--force`, one at a
//! time, the status line saying what went and what git kept. Old sessions are
//! settled ([`Op::SetShelves`]), never deleted.
//!
//! Keys while it is open: up and down move, Enter ticks the row, Tab switches
//! the list; what else is typed goes to the filter.

use super::shell::{Overlay, Shell};
use super::sweep::{self, Age, Candidate, Measure, Order, Show, Tab};
use super::updates_view::pill;
use super::widgets::{mono, section_label};
use crate::disk::size_words;
use crate::engine::Op;
use crate::format;
use crate::icons::{icon, IconName};
use crate::theme::{fonts, metrics, px, Palette};
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, Entity, ScrollHandle, Stateful, Task, Window};
use leon_core::{Session, SessionId, Shelf, WorktreeId};
use std::collections::{HashMap, HashSet};

/// What the window keeps of the cleaner.
pub struct SweepUi {
    /// The list on screen.
    pub tab: Tab,
    /// Which worktrees are shown.
    pub show: Show,
    /// How they are ordered.
    pub order: Order,
    /// How old a session must be.
    pub age: Age,
    /// The filter's field.
    pub search: Entity<InputState>,
    /// What the field says.
    pub query: String,
    /// What each worktree was measured at.
    pub sizes: HashMap<WorktreeId, Measure>,
    /// The ticked worktrees, by `project|worktree`.
    pub ticks: HashSet<String>,
    /// The ticked sessions.
    pub session_ticks: HashSet<SessionId>,
    /// The row the keyboard is on.
    pub cursor: usize,
    scroll: ScrollHandle,
    measuring: Vec<Task<()>>,
}

impl SweepUi {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        Self {
            tab: Tab::default(),
            show: Show::default(),
            order: Order::default(),
            age: Age::default(),
            search: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Filter by project, branch or folder")
            }),
            query: String::new(),
            sizes: HashMap::new(),
            ticks: HashSet::new(),
            session_ticks: HashSet::new(),
            cursor: 0,
            scroll: ScrollHandle::new(),
            measuring: Vec::new(),
        }
    }
}

impl Shell {
    /// Opens the cleaner, or closes it when it is open.
    pub(super) fn toggle_sweep(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay == Overlay::Cleanup {
            self.close_overlay(window, cx);
            return;
        }
        self.close_overlay(window, cx);
        self.overlay = Overlay::Cleanup;
        self.sweep.ticks.clear();
        self.sweep.session_ticks.clear();
        self.sweep.cursor = 0;
        self.sweep_measure(cx);
        let search = self.sweep.search.clone();
        search.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Measures every linked worktree again, in the background.
    fn sweep_measure(&mut self, cx: &mut Context<Self>) {
        let targets: Vec<_> = self
            .snapshot
            .projects
            .iter()
            .flat_map(|entry| {
                entry
                    .worktrees
                    .iter()
                    .filter(|worktree| !worktree.is_main)
                    .map(move |worktree| {
                        (
                            worktree.id.clone(),
                            entry.project.machine_id.clone(),
                            worktree.path.clone(),
                        )
                    })
            })
            .collect();
        // Started together, each answer shown as it comes: the runtime
        // measures them side by side, and a large worktree does not hold
        // back the sizes of the small ones.
        self.sweep.measuring = targets
            .into_iter()
            .map(|(id, machine, path)| {
                self.sweep
                    .sizes
                    .entry(id.clone())
                    .or_insert(Measure::Pending);
                let job = self.engine.measure_folder(machine, path);
                cx.spawn(async move |this, cx| {
                    let measured = job.await.ok().flatten();
                    this.update(cx, |this, cx| {
                        this.sweep
                            .sizes
                            .insert(id, measured.map_or(Measure::Unknown, Measure::Known));
                        cx.notify();
                    })
                    .ok();
                })
            })
            .collect();
    }

    /// The text of the cleaner's field changed.
    pub(super) fn sweep_search_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.sweep.search.read(cx).value().to_string();
        if text == self.sweep.query {
            return;
        }
        self.sweep.query = text;
        self.sweep.cursor = 0;
        self.sweep.scroll.scroll_to_item(0);
        cx.notify();
    }

    /// The linked worktrees with what the cleaner weighs.
    fn sweep_candidates(&self) -> Vec<Candidate> {
        sweep::candidates(
            &self.snapshot,
            &self.running_per_worktree(),
            &self.sweep.sizes,
            self.now(),
        )
    }

    /// The worktrees the list shows.
    fn sweep_worktrees(&self) -> Vec<Candidate> {
        sweep::visible(
            &self.sweep_candidates(),
            self.sweep.show,
            self.sweep.order,
            &self.sweep.query,
        )
    }

    /// The old sessions the list shows.
    fn sweep_sessions(&self) -> Vec<Session> {
        sweep::old_sessions(
            &self.snapshot.sessions,
            &self.running_histories(),
            &self.shelves.shelf,
            self.sweep.age,
            &self.sweep.query,
            self.now(),
        )
        .into_iter()
        .cloned()
        .collect()
    }

    fn sweep_rows(&self) -> usize {
        match self.sweep.tab {
            Tab::Worktrees => self.sweep_worktrees().len(),
            Tab::Sessions => self.sweep_sessions().len(),
        }
    }

    fn sweep_set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.sweep.tab = tab;
        self.sweep.cursor = 0;
        self.sweep.scroll.scroll_to_item(0);
        cx.notify();
    }

    /// Ticks the row at `at` of the list on screen, or unticks it.
    fn sweep_toggle(&mut self, at: usize, cx: &mut Context<Self>) {
        match self.sweep.tab {
            Tab::Worktrees => {
                if let Some(candidate) = self.sweep_worktrees().get(at) {
                    if candidate.removable() && !self.sweep.ticks.remove(&candidate.key) {
                        self.sweep.ticks.insert(candidate.key.clone());
                    }
                }
            }
            Tab::Sessions => {
                if let Some(session) = self.sweep_sessions().get(at) {
                    if !self.sweep.session_ticks.remove(&session.id) {
                        self.sweep.session_ticks.insert(session.id.clone());
                    }
                }
            }
        }
        self.sweep.cursor = at;
        cx.notify();
    }

    /// Ticks what the cleaner suggests among the rows shown (every session,
    /// in the sessions list), or unticks them all when they already are.
    fn sweep_tick_suggested(&mut self, cx: &mut Context<Self>) {
        match self.sweep.tab {
            Tab::Worktrees => {
                let suggested = sweep::suggested(&self.sweep_worktrees());
                if !suggested.is_empty() && suggested.is_subset(&self.sweep.ticks) {
                    self.sweep.ticks.retain(|key| !suggested.contains(key));
                } else {
                    self.sweep.ticks.extend(suggested);
                }
            }
            Tab::Sessions => {
                let all: HashSet<SessionId> = self
                    .sweep_sessions()
                    .into_iter()
                    .map(|session| session.id)
                    .collect();
                if !all.is_empty() && all.is_subset(&self.sweep.session_ticks) {
                    self.sweep.session_ticks.retain(|id| !all.contains(id));
                } else {
                    self.sweep.session_ticks.extend(all);
                }
            }
        }
        cx.notify();
    }

    /// Removes the ticked worktrees, or settles the ticked sessions.
    fn sweep_clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.sweep.tab {
            Tab::Worktrees => {
                let candidates = self.sweep_candidates();
                let (chosen, _) = sweep::ticked(&candidates, &self.sweep.ticks);
                let list: Vec<_> = chosen
                    .iter()
                    .map(|candidate| (candidate.project.clone(), candidate.worktree.clone()))
                    .collect();
                if list.is_empty() {
                    return;
                }
                self.sweep.ticks.clear();
                self.remove_worktrees(list, window, cx);
            }
            Tab::Sessions => {
                let changes: Vec<(SessionId, Option<Shelf>)> = self
                    .sweep_sessions()
                    .into_iter()
                    .filter(|session| self.sweep.session_ticks.contains(&session.id))
                    .map(|session| (session.id, Some(Shelf::Settled)))
                    .collect();
                if changes.is_empty() {
                    return;
                }
                let done = sweep::settled_words(changes.len());
                self.sweep.session_ticks.clear();
                self.engine.submit(Op::SetShelves {
                    changes,
                    done: Some(done),
                });
            }
        }
        self.sweep.cursor = 0;
        cx.notify();
    }

    pub(super) fn sweep_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &stroke.modifiers;
        if m.platform || m.control || m.alt || m.function {
            return false;
        }
        let rows = self.sweep_rows();
        match (stroke.key.as_str(), m.shift) {
            ("down", false) if rows > 0 => {
                self.sweep.cursor = (self.sweep.cursor + 1).min(rows - 1);
                self.sweep.scroll.scroll_to_item(self.sweep.cursor);
            }
            ("up", false) => {
                self.sweep.cursor = self.sweep.cursor.saturating_sub(1);
                self.sweep.scroll.scroll_to_item(self.sweep.cursor);
            }
            ("enter", false) if rows > 0 => self.sweep_toggle(self.sweep.cursor.min(rows - 1), cx),
            ("enter", true) => self.sweep_clean(window, cx),
            ("tab", _) => {
                let next = match self.sweep.tab {
                    Tab::Worktrees => Tab::Sessions,
                    Tab::Sessions => Tab::Worktrees,
                };
                self.sweep_set_tab(next, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    // ----- drawing ----------------------------------------------------------------------------

    pub(super) fn render_sweep(&self, colours: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let all = self.sweep_candidates();
        let worktrees = sweep::visible(&all, self.sweep.show, self.sweep.order, &self.sweep.query);
        let sessions = self.sweep_sessions();
        let (chosen, freed) = sweep::ticked(&all, &self.sweep.ticks);
        let measured: u64 = all.iter().filter_map(|c| c.size.total()).sum();
        let pending = all.iter().any(|c| c.size == Measure::Pending);
        let removing = self.merged_busy;

        let tab = |id: &'static str, label: String, this_tab: Tab| {
            let on = self.sweep.tab == this_tab;
            mono(label)
                .id(id)
                .debug_selector(move || id.into())
                .px(px(10.))
                .py(px(4.))
                .cursor_pointer()
                .border_b_1()
                .border_color(if on { colours.signal } else { colours.surface })
                .text_color(if on { colours.text } else { colours.text_muted })
                .on_click(cx.listener(move |this, _, _, cx| this.sweep_set_tab(this_tab, cx)))
        };

        let header = div()
            .flex_none()
            .px_4()
            .pt(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon(IconName::BrushCleaning, px(14.), colours.signal))
                    .child(section_label("Clean up", colours))
                    .child(div().flex_1())
                    .child(
                        mono(if pending {
                            format!("{} in worktrees · measuring…", size_words(measured))
                        } else {
                            format!("{} in worktrees", size_words(measured))
                        })
                        .debug_selector(|| "sweep-total".into())
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted),
                    ),
            )
            .child(
                div()
                    .h(metrics::CONTROL())
                    .px(px(8.))
                    .rounded(metrics::RADIUS())
                    .border_1()
                    .border_color(colours.border)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon(IconName::Search, px(14.), colours.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(super::widgets::text_input(&self.sweep.search)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_end()
                    .border_b_1()
                    .border_color(colours.border)
                    .child(tab(
                        "sweep-tab-worktrees",
                        format!("WORKTREES {}", all.len()),
                        Tab::Worktrees,
                    ))
                    .child(tab(
                        "sweep-tab-sessions",
                        format!("OLD SESSIONS {}", sessions.len()),
                        Tab::Sessions,
                    )),
            )
            .child(self.render_sweep_chips(&all, colours, cx));

        let mut body = div()
            .id("sweep-rows")
            .debug_selector(|| "sweep-rows".into())
            .track_scroll(&self.sweep.scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .px_2()
            .py_2()
            .flex()
            .flex_col();
        let empty = match self.sweep.tab {
            Tab::Worktrees => worktrees.is_empty(),
            Tab::Sessions => sessions.is_empty(),
        };
        if empty {
            body = body.child(
                div()
                    .px_2()
                    .py_3()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_muted)
                    .child(match self.sweep.tab {
                        Tab::Worktrees => "No linked worktree matches.",
                        Tab::Sessions => "No old session matches: everything here is recent, pinned, running or on a shelf.",
                    }),
            );
        }
        match self.sweep.tab {
            Tab::Worktrees => {
                for (at, candidate) in worktrees.iter().enumerate() {
                    body = body.child(self.render_sweep_worktree(at, candidate, colours, cx));
                }
            }
            Tab::Sessions => {
                for (at, session) in sessions.iter().enumerate() {
                    body = body.child(self.render_sweep_session(at, session, colours, cx));
                }
            }
        }

        let (action, ticked_label, hint) = match self.sweep.tab {
            Tab::Worktrees => (
                if removing {
                    "REMOVING…".to_owned()
                } else {
                    format!("REMOVE {}", chosen.len())
                },
                if chosen.is_empty() {
                    "Nothing ticked".to_owned()
                } else {
                    format!("{} to free", size_words(freed))
                },
                "git worktree remove: the branches stay, nothing is forced",
            ),
            Tab::Sessions => {
                let count = sessions
                    .iter()
                    .filter(|session| self.sweep.session_ticks.contains(&session.id))
                    .count();
                (
                    format!("SETTLE {count}"),
                    if count == 0 {
                        "Nothing ticked".to_owned()
                    } else {
                        format!("{count} ticked")
                    },
                    "settled sessions stay on the Settled shelf",
                )
            }
        };
        let footer = div()
            .flex_none()
            .px_4()
            .py(px(8.))
            .border_t_1()
            .border_color(colours.border)
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                pill("sweep-clean", action, colours)
                    .when(!removing, |this| {
                        this.bg(colours.primary_fill).text_color(colours.on_primary)
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.sweep_clean(window, cx))),
            )
            .child(
                pill(
                    "sweep-suggested",
                    match self.sweep.tab {
                        Tab::Worktrees => "TICK SUGGESTED",
                        Tab::Sessions => "TICK ALL",
                    },
                    colours,
                )
                .on_click(cx.listener(|this, _, _, cx| this.sweep_tick_suggested(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .items_end()
                    .text_size(metrics::TEXT_SMALL())
                    .child(
                        mono(ticked_label)
                            .debug_selector(|| "sweep-ticked".into())
                            .text_color(colours.text),
                    )
                    .child(
                        div()
                            .text_color(colours.text_faint)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(hint),
                    ),
            );

        self.card("sweep", colours)
            .w(px(760.))
            .max_w(self.viewport.width - px(32.))
            .h(self.viewport.height - px(112.))
            .max_h(px(720.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .child(footer)
    }

    /// The chips under the tabs: the filters and the order of the worktrees,
    /// or the age of the sessions.
    fn render_sweep_chips(
        &self,
        all: &[Candidate],
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let chip = |id: String, label: String, on: bool| {
            mono(label)
                .id(gpui_kit::ElementId::Name(id.clone().into()))
                .debug_selector(move || id.clone())
                .px(px(8.))
                .py(px(2.))
                .rounded(metrics::RADIUS())
                .cursor_pointer()
                .text_size(metrics::TEXT_SMALL())
                .when(on, |this| {
                    this.bg(colours.surface_2).text_color(colours.signal)
                })
                .when(!on, |this| this.text_color(colours.text_muted))
        };
        let mut row = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(4.))
            .pb(px(6.));
        match self.sweep.tab {
            Tab::Worktrees => {
                for show in Show::ALL {
                    let count = all.iter().filter(|c| show.admits(c)).count();
                    row = row.child(
                        chip(
                            format!("sweep-show-{}", show.label().to_lowercase()),
                            format!("{} {count}", show.label()),
                            self.sweep.show == show,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sweep.show = show;
                            this.sweep.cursor = 0;
                            cx.notify();
                        })),
                    );
                }
                row = row.child(div().flex_1()).child(
                    chip(
                        "sweep-order".to_owned(),
                        format!("\u{2195} {}", self.sweep.order.label()),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sweep.order = this.sweep.order.next();
                        cx.notify();
                    })),
                );
            }
            Tab::Sessions => {
                for age in Age::ALL {
                    row = row.child(
                        chip(
                            format!("sweep-age-{}", age.days()),
                            age.label().to_owned(),
                            self.sweep.age == age,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sweep.age = age;
                            this.sweep.cursor = 0;
                            cx.notify();
                        })),
                    );
                }
                row = row.child(
                    div()
                        .pl(px(6.))
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_faint)
                        .child("not pinned, not running, not on a shelf"),
                );
            }
        }
        row
    }

    /// The box a row is ticked in; a row that cannot be ticked has none.
    fn sweep_tick(on: bool, can: bool, colours: &Palette) -> Div {
        let tick = div()
            .flex_none()
            .size(px(14.))
            .rounded(px(3.))
            .border_1()
            .flex()
            .items_center()
            .justify_center();
        if !can {
            return tick.border_color(colours.border);
        }
        if on {
            tick.border_color(colours.signal)
                .bg(colours.signal)
                .child(icon(IconName::Check, px(10.), colours.surface))
        } else {
            tick.border_color(colours.text_muted)
        }
    }

    fn sweep_row(&self, id: &'static str, at: usize, colours: &Palette) -> Stateful<Div> {
        let on = self.sweep.cursor == at;
        let hover = colours.surface_2;
        div()
            .id((id, at))
            .debug_selector(move || format!("{id}-{at}"))
            .flex_none()
            .px_2()
            .py(px(5.))
            .rounded(metrics::RADIUS())
            .flex()
            .items_center()
            .gap(px(10.))
            .when(on, |this| this.bg(colours.surface_2))
            .hover(move |style| style.bg(hover))
    }

    fn render_sweep_worktree(
        &self,
        at: usize,
        candidate: &Candidate,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let can = candidate.removable();
        let on = self.sweep.ticks.contains(&candidate.key);
        let (size, build) = match candidate.size {
            Measure::Known(usage) => (
                size_words(usage.total),
                (usage.rebuildable > 0).then(|| format!("{} build", size_words(usage.rebuildable))),
            ),
            Measure::Pending => ("…".to_owned(), None),
            Measure::Unknown => ("?".to_owned(), None),
        };
        let reasons = candidate.reasons().join(" · ");
        let reason_colour = if !can {
            colours.warning
        } else if candidate.recommended() {
            colours.success
        } else {
            colours.text_muted
        };
        let activity = match (candidate.sessions, candidate.last_active) {
            (0, _) | (_, None) => "no sessions".to_owned(),
            (count, Some(at)) => format!(
                "{count} {} · {} ago",
                if count == 1 { "session" } else { "sessions" },
                format::age(self.now(), at)
            ),
        };
        self.sweep_row("sweep-worktree", at, colours)
            .when(can, |this| this.cursor_pointer())
            .on_click(cx.listener(move |this, _, _, cx| this.sweep_toggle(at, cx)))
            .child(Self::sweep_tick(on, can, colours))
            .child(icon(
                if candidate.merged {
                    IconName::GitMerge
                } else {
                    IconName::GitBranch
                },
                px(14.),
                if candidate.merged {
                    colours.signal
                } else {
                    colours.text_muted
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .when(!can, |this| this.text_color(colours.text_muted))
                                    .child(candidate.label.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(colours.text_faint)
                                    .child(candidate.project_name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(metrics::TEXT_SMALL())
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(reason_colour)
                            .child(reasons),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(150.))
                    .flex()
                    .flex_col()
                    .items_end()
                    .font_family(fonts::mono())
                    .text_size(metrics::TEXT_SMALL())
                    .child(div().text_color(colours.text_muted).child(activity)),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(90.))
                    .flex()
                    .flex_col()
                    .items_end()
                    .font_family(fonts::mono())
                    .child(
                        div()
                            .debug_selector(move || format!("sweep-size-{at}"))
                            .child(size),
                    )
                    .children(build.map(|text| {
                        div()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_faint)
                            .child(text)
                    })),
            )
    }

    fn render_sweep_session(
        &self,
        at: usize,
        session: &Session,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let on = self.sweep.session_ticks.contains(&session.id);
        let title = format::plain_title(&session.title);
        self.sweep_row("sweep-session", at, colours)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.sweep_toggle(at, cx)))
            .child(Self::sweep_tick(on, true, colours))
            .child(crate::icons::agent_icon(session.agent, px(14.), colours))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(if title.is_empty() {
                                "(untitled)".to_owned()
                            } else {
                                title
                            }),
                    )
                    .child(
                        div()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_faint)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(session.cwd.clone()),
                    ),
            )
            .child(
                mono(format!(
                    "{} msgs · {} ago",
                    session.message_count,
                    format::age(self.now(), session.updated_at)
                ))
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text_muted),
            )
    }
}

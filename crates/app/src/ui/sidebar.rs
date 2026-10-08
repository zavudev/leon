//! The sidebar: the Leon glare and the product's name on top, the tree of
//! machines, projects, worktrees and sessions as one virtualised list, and the
//! tools at the bottom.
//!
//! Every row has the same height and is drawn from the flat list the shell
//! keeps (see `tree.rs`), so only the rows in view cost anything however long
//! the history is. Long names give way with an ellipsis; the tags and ages
//! never move.

use super::activity::Activity;
use super::live::LiveState;
use super::shell::{Pane, RowDrag, Shell};
use super::tree::{folder_name, worktree_label, Kind, Row};
use super::widgets::{
    activity_light, elsewhere_mark, focus_rule, key_cap, led, mark, mono, section_label, Lion,
};
use crate::elsewhere::Holder;
use crate::engine::MachineState;
use crate::format;
use crate::fuzzy::matched_chars;
use crate::icons::{agent_icon, agent_icon_in, icon, IconName, Tone};
use crate::keys::{self, Command};
use crate::product;
use crate::theme::{fonts, metrics, px, Appearance, Palette};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    div, uniform_list, AnyElement, Context, Div, DragMoveEvent, FontWeight, Hsla, Render,
    SharedString, Stateful,
};
use gpui_kit::{HighlightStyle, StyledText};

/// The group every row of the tree belongs to: a part of a row can show only
/// while the row is hovered (the pin of a session, see `pin_mark`).
const ROW_GROUP: &str = "tree-row";

/// The ghost shown while a row is dragged: nothing is drawn, the target's
/// line says where the drop goes.
struct RowDragGhost;

impl Render for RowDragGhost {
    fn render(&mut self, _: &mut gpui_kit::Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The colour of a machine's status light: green while it is reachable,
/// amber while it is being probed, red when it is not, grey when nobody has
/// asked yet.
pub fn state_colour(state: &MachineState, colours: &Palette) -> Hsla {
    match state {
        MachineState::Online(_) => colours.success,
        MachineState::Probing => colours.warning,
        MachineState::Offline(_) => colours.error,
        MachineState::Unknown => colours.text_faint,
    }
}

/// The word next to a machine's light.
pub fn state_label(state: &MachineState) -> &'static str {
    match state {
        MachineState::Online(_) => "ONLINE",
        MachineState::Probing => "PROBING",
        MachineState::Offline(_) => "OFFLINE",
        MachineState::Unknown => "UNKNOWN",
    }
}

/// The colour of the light of a live terminal: its state, never the icon.
pub(super) fn live_light(state: LiveState, colours: &Palette) -> Hsla {
    match state {
        LiveState::Running => colours.success,
        LiveState::Starting => colours.warning,
        LiveState::Exited(0) => colours.text_faint,
        LiveState::Exited(_) => colours.error,
    }
}

/// What a tool's tooltip says: what it does and the keys that do the same.
pub fn tooltip_text(command: Command) -> String {
    match keys::keys_label(command) {
        Some(keys) => format!("{}  {keys}", keys::label(command)),
        None => keys::label(command).to_owned(),
    }
}

impl Shell {
    pub(super) fn render_sidebar(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let focused = self.pane == Pane::Sidebar;
        let palette = *colours;
        div()
            .debug_selector(|| "sidebar".into())
            .relative()
            .flex_none()
            .w(metrics::SIDEBAR_WIDTH())
            .h_full()
            .border_r_1()
            .border_color(colours.border)
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| "sidebar-header".into())
                    .relative()
                    .flex_none()
                    .h(metrics::HEADER_HEIGHT())
                    .px_4()
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(mark(
                        px(20.),
                        colours,
                        cx,
                        Lion::new("header-mark")
                            .mood(self.mark_mood())
                            .hover()
                            // The entrance plays once, when the window opens.
                            .intro(!self.intro_played.replace(true)),
                    ))
                    .child(
                        div()
                            .debug_selector(|| "sidebar-title".into())
                            .font_weight(FontWeight::MEDIUM)
                            .child(product::PRODUCT_NAME),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("sidebar-add-project")
                            .debug_selector(|| "sidebar-add-project".into())
                            .flex_none()
                            .size(metrics::CONTROL())
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(metrics::RADIUS())
                            .cursor_pointer()
                            .hover({
                                let hover = colours.surface;
                                move |style| style.bg(hover)
                            })
                            .tooltip({
                                let tip: SharedString = tooltip_text(Command::OpenProject).into();
                                move |window, cx| Tooltip::new(tip.clone()).build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_add_project(window, cx);
                            }))
                            .child(icon(IconName::Plus, px(14.), colours.text_muted)),
                    )
                    .when(focused, |this| this.child(focus_rule(colours))),
            )
            .child(self.render_filter(colours, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div().flex_1().min_w_0().h_full().child(
                            uniform_list(
                                "tree",
                                self.rows.len(),
                                cx.processor(
                                    move |this, range: std::ops::Range<usize>, _window, cx| {
                                        range
                                            .filter_map(|index| {
                                                this.render_row(index, &palette, cx)
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                ),
                            )
                            .track_scroll(&self.tree_scroll)
                            .size_full(),
                        ),
                    )
                    // The scrollbar has a column of its own: laid over the rows
                    // it would cover the marks at their end.
                    .child(
                        div()
                            .debug_selector(|| "tree-scrollbar".into())
                            .relative()
                            .flex_none()
                            .w(metrics::SCROLLBAR_GUTTER())
                            .h_full()
                            .child(Scrollbar::vertical(&self.tree_scroll)),
                    ),
            )
            .child(self.render_connect_row(colours, cx))
            .child(self.render_share_row(colours, cx))
            .child(self.render_tools(colours, cx))
    }

    /// The row after the tree that says remote computers can be added, with
    /// its chord: discoverable without knowing the shortcut.
    fn render_connect_row(&self, colours: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let hover = colours.surface;
        div()
            .id("sidebar-connect")
            .debug_selector(|| "sidebar-connect".into())
            .flex_none()
            .h(metrics::ROW_HEIGHT())
            .px(px(12.))
            .border_t_1()
            .border_color(colours.border)
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(|this, _, window, cx| {
                this.run_command(Command::AddMachine, window, cx);
            }))
            .child(mono("+").text_color(colours.text_muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(colours.text_muted)
                    .child("Connect a machine"),
            )
            .children(keys::keys_label(Command::AddMachine).map(|text| key_cap(text, colours)))
    }

    /// The row under it: share this computer.
    fn render_share_row(&self, colours: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let hover = colours.surface;
        div()
            .id("sidebar-share")
            .debug_selector(|| "sidebar-share".into())
            .flex_none()
            .h(metrics::ROW_HEIGHT())
            .px(px(12.))
            .border_t_1()
            .border_color(colours.border)
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(|this, _, window, cx| {
                this.run_command(Command::ShareMachine, window, cx);
            }))
            .child(mono("\u{2197}").text_color(colours.text_muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(colours.text_muted)
                    .child("Share this machine"),
            )
    }

    /// The button that shows or hides the sidebar, with its chord in its
    /// tooltip. It is in the main pane's header only, open or hidden.
    pub(super) fn sidebar_toggle_button(
        &self,
        id: &'static str,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let hover = colours.surface;
        let tip: SharedString = tooltip_text(Command::ToggleSidebar).into();
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex_none()
            .size(metrics::HEADER_TITLE_LINE())
            .flex()
            .items_center()
            .justify_center()
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.run_command(Command::ToggleSidebar, window, cx);
            }))
            .child(icon(
                if crate::settings::get(cx).sidebar_visible {
                    IconName::PanelLeftClose
                } else {
                    IconName::PanelLeft
                },
                px(14.),
                colours.text_muted,
            ))
    }

    /// The field above the tree that filters the projects, with the +
    /// that adds one.
    fn render_filter(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let empty = self.filter_query.is_empty();
        div()
            .debug_selector(|| "sidebar-filter".into())
            .flex_none()
            .h(metrics::CONTROL())
            .px_4()
            .border_b_1()
            .border_color(colours.border)
            .flex()
            .items_center()
            .gap(px(8.))
            .child(icon(IconName::Search, px(14.), colours.text_muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.filter_input).appearance(false)),
            )
            .when(empty, |this| {
                this.children(
                    keys::keys_label(Command::FilterProjects)
                        .and_then(|text| text.split(" / ").next().map(str::to_owned))
                        .map(|text| key_cap(text, colours)),
                )
            })
            // "Active only": the sessions that are running, and nothing else.
            .child(self.render_active_toggle(colours, cx))
            // The same "+" as the header's: add a project without knowing the
            // chord.
            .child(
                div()
                    .id("filter-add-project")
                    .debug_selector(|| "filter-add-project".into())
                    .flex_none()
                    .size(metrics::CONTROL())
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(metrics::RADIUS())
                    .cursor_pointer()
                    .hover({
                        let hover = colours.surface;
                        move |style| style.bg(hover)
                    })
                    .tooltip({
                        let tip: SharedString = "Add a project\u{2026}".into();
                        move |window, cx| Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_add_project(window, cx);
                    }))
                    .child(icon(IconName::Plus, px(14.), colours.text_muted)),
            )
    }

    /// The toggle beside the filter: only the active sessions (live terminals
    /// and agents running elsewhere) are listed while it is on.
    fn render_active_toggle(&self, colours: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let on = self.active_only;
        let hover = colours.surface;
        let tip: SharedString = if on {
            "Showing only active sessions \u{b7} click to show all".into()
        } else {
            "Show only active sessions".into()
        };
        div()
            .id("filter-active-only")
            .debug_selector(move || {
                if on {
                    "filter-active-only-on".to_owned()
                } else {
                    "filter-active-only".to_owned()
                }
            })
            .flex_none()
            .size(metrics::CONTROL())
            .flex()
            .items_center()
            .justify_center()
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .when(on, |this| this.bg(colours.surface_2))
            .when(!on, |this| this.hover(move |style| style.bg(hover)))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.run_command(Command::ToggleActiveOnly, window, cx);
            }))
            .child(icon(
                IconName::CircleDot,
                px(14.),
                if on {
                    colours.signal
                } else {
                    colours.text_muted
                },
            ))
    }

    /// The tools at the foot of the sidebar, each with its shortcut in its
    /// tooltip.
    fn render_tools(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let tool = |id: &'static str, glyph: IconName, command: Command| {
            let hover = colours.surface;
            let tip: SharedString = tooltip_text(command).into();
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex_none()
                .size(metrics::CONTROL())
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_command(command, window, cx);
                }))
                .child(icon(glyph, px(16.), colours.text_muted))
        };
        div()
            .debug_selector(|| "sidebar-tools".into())
            .flex_none()
            .h(metrics::TOOLS_HEIGHT())
            .px(px(8.))
            .border_t_1()
            .border_color(colours.border)
            .flex()
            .items_center()
            .child(tool(
                "tool-theme",
                match colours.appearance {
                    Appearance::Dark => IconName::Sun,
                    Appearance::Light => IconName::Moon,
                },
                Command::ToggleAppearance,
            ))
            .child(tool(
                "tool-shortcuts",
                IconName::Keyboard,
                Command::Shortcuts,
            ))
            .child(tool("tool-settings", IconName::Settings, Command::Settings))
    }

    /// The row at `index` of the tree.
    fn render_row(
        &self,
        index: usize,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let row = self.rows.get(index)?;
        let on = self.cursor == Some(index);
        let focused = self.pane == Pane::Sidebar;
        let hover = colours.surface;
        let base = div()
            .id(("row", index))
            .debug_selector(move || format!("tree-row-{index}"))
            .group(ROW_GROUP)
            .relative()
            .h(metrics::ROW_HEIGHT())
            .w_full()
            .pl(px(12.) + metrics::INDENT() * f32::from(row.depth))
            .pr(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .overflow_hidden()
            .cursor_pointer()
            .when(on, |this| this.bg(colours.surface_2))
            .when(!on, |this| this.hover(move |style| style.bg(hover)))
            // The row the keyboard is on carries the accent as a bar.
            .when(on && focused, |this| {
                this.child(
                    div()
                        .debug_selector(|| "tree-cursor".into())
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(colours.signal),
                )
            })
            // Levels, apart: a hairline above every project but the first of
            // its machine, and room above its row so the blocks breathe.
            .when(self.starts_a_project_group(index), |this| this.pt(px(8.)))
            .when(self.starts_a_project_group(index), |this| {
                this.child(
                    div()
                        .debug_selector(move || format!("tree-divider-{index}"))
                        .absolute()
                        .top_0()
                        .left(px(12.))
                        .right(px(12.))
                        .h(px(1.))
                        .bg(colours.border),
                )
            })
            .on_mouse_down(
                gpui_kit::MouseButton::Right,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_menu_at(index, Some(event.position), window, cx);
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.cursor = Some(index);
                this.pane = Pane::Sidebar;
                // A click in the tree takes the keyboard out of the filter.
                this.focus.focus(window, cx);
                this.activate(index, window, cx);
                cx.notify();
            }));
        Some(self.render_row_content(index, row, base, colours, cx))
    }

    /// Whether a hairline goes above this row: a project that follows another
    /// row of its machine, so the projects read as blocks.
    pub(super) fn starts_a_project_group(&self, index: usize) -> bool {
        matches!(
            self.rows.get(index).map(|row| &row.kind),
            Some(Kind::Project { .. })
        ) && index > 0
            && !matches!(self.rows[index - 1].kind, Kind::Machine(_))
    }

    /// How a movable row (`Order::Project`, `Worktree` or `Session`) drags:
    /// the row reports every drag move over its own list (see
    /// `note_row_drag_over`), a line marks where the drop would go, and the
    /// window-wide release listener completes it. Rows that cannot move (the
    /// only one of their list) get no drag at all.
    fn draggable_row(
        &self,
        row: &Row,
        index: usize,
        base: Stateful<Div>,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(tree_order) = self.tree_order(&row.kind, &row.machine) else {
            return base;
        };
        let order = tree_order.order;
        if !self.can_order(&order) {
            return base;
        }
        let source = order.clone();
        let target = order;
        let target_for_move = target.clone();
        let show_line = self
            .row_drop_target
            .as_ref()
            .filter(|drop| drop.target == target)
            .map(|drop| drop.after);
        let line_colour = *colours;
        let debug = format!("tree-drop-line-{index}");
        base.on_drag(RowDrag(source), |_, _, _, cx| cx.new(|_| RowDragGhost))
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<RowDrag>, _, cx| {
                    // This listener runs for every movable row on every move
                    // of the drag (capture phase, no hover gating), so only
                    // the row under the pointer answers. The check is
                    // geometric on purpose: while anything is dragged the
                    // sheet over the window takes the pointer, so no row
                    // reports hovered.
                    if !event.bounds.contains(&event.event.position) {
                        return;
                    }
                    let dragged = event.drag(cx).0.clone();
                    let after = event.event.position.y > event.bounds.center().y;
                    if this.note_row_drag_over(&dragged, &target_for_move, after) {
                        cx.notify();
                    }
                }),
            )
            .when(show_line.is_some(), |this| {
                let after = show_line.unwrap_or(false);
                this.child(
                    div()
                        .debug_selector(move || debug.clone())
                        .absolute()
                        .left(px(8.))
                        .right(px(8.))
                        .when(after, |this| this.bottom_0())
                        .when(!after, |this| this.top_0())
                        .h(px(2.))
                        .bg(line_colour.signal),
                )
            })
    }

    fn render_row_content(
        &self,
        index: usize,
        row: &Row,
        base: Stateful<Div>,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The slot before every label: a chevron where the row has children, so
        // that the labels of rows of one level line up.
        let slot = || {
            div()
                .flex_none()
                .size(px(16.))
                .flex()
                .items_center()
                .justify_center()
        };
        let chevron = match row.open {
            Some(open) => slot()
                .id(("chevron", index))
                .debug_selector(move || format!("tree-chevron-{index}"))
                .cursor_pointer()
                .child(icon(
                    if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    },
                    px(12.),
                    colours.text_faint,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.cursor = Some(index);
                    this.toggle(index);
                    cx.notify();
                }))
                .into_any_element(),
            None => slot().into_any_element(),
        };
        let count = |sessions: usize| {
            mono(sessions.to_string())
                .when(sessions == 0, |this| this.invisible())
                .text_color(colours.text_faint)
        };
        let label = || div().flex_1().min_w_0().pr(px(6.)).truncate();
        match &row.kind {
            Kind::Machine(machine) => {
                let state = self.engine.machine_state(&machine.id);
                base.child(chevron)
                    .child(
                        label()
                            .font_family(fonts::mono())
                            .font_features(fonts::mono_features())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(metrics::TEXT_LABEL())
                            .text_color(colours.text_muted)
                            .child(machine.name.to_uppercase()),
                    )
                    .child(led(state_colour(&state, colours)))
                    .child(mono(state_label(&state)).text_color(colours.text_faint))
                    .into_any_element()
            }
            Kind::Project { project, sessions } => {
                let base = self.draggable_row(row, index, base, colours, cx);
                let sessions = *sessions;
                let activity = self.project_activity(&project.id);
                let deleting = self
                    .deleting
                    .as_ref()
                    .is_some_and(|(id, _)| id == &project.id);
                base.child(chevron)
                    .child(self.logo(
                        &project.id,
                        metrics::PROJECT_ICON(),
                        &format!("tree-{index}"),
                        colours,
                    ))
                    .child(
                        label()
                            .debug_selector(move || format!("tree-label-{index}"))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.emphasised(&self.project_label(project), colours)),
                    )
                    .when(deleting, |this| {
                        this.child(
                            mono("DELETING")
                                .debug_selector(move || format!("tree-deleting-{index}"))
                                .text_color(colours.text_faint),
                        )
                    })
                    .child(self.dot(index, activity, colours))
                    .child(count(sessions))
                    .into_any_element()
            }
            Kind::Worktree { worktree, sessions } => {
                let base = self.draggable_row(row, index, base, colours, cx);
                let _ = sessions;
                let activity = self.worktree_activity(&worktree.id);
                let deleting = self
                    .deleting
                    .as_ref()
                    .is_some_and(|(_, id)| id == &worktree.id);
                // Two things, apart: the git mark leads (what the branch is)
                // and the agents' light closes the row (what is running in
                // it, with a word when it needs the person).
                base.child(chevron)
                    .child(self.git_mark(index, worktree, colours))
                    .child(
                        label()
                            .debug_selector(move || format!("tree-label-{index}"))
                            .child(self.emphasised(&worktree_label(worktree), colours)),
                    )
                    .when(worktree.is_main, |this| {
                        this.child(mono("MAIN").text_color(colours.text_faint))
                    })
                    .when(deleting, |this| {
                        this.child(
                            mono("DELETING")
                                .debug_selector(move || format!("tree-deleting-{index}"))
                                .text_color(colours.text_faint),
                        )
                    })
                    .children(activity_word(activity).map(|word| {
                        mono(word)
                            .debug_selector(move || format!("tree-activity-word-{index}"))
                            .text_color(match activity {
                                Activity::Failed => colours.error,
                                _ => colours.warning,
                            })
                    }))
                    .child(self.dot(index, activity, colours))
                    .into_any_element()
            }
            Kind::Session(session) => {
                let base = self.draggable_row(row, index, base, colours, cx);
                let name: SharedString = format::agent_name(session.agent).into();
                let pinned = session.sort_order.is_some();
                // A session with a terminal of its own folder is that
                // terminal's row: it shows the terminal's state, not its age.
                let running = self
                    .live
                    .of_history(&session.id)
                    .filter(|_| self.placement.merged.contains(&session.id))
                    .map(|live| (live.id, live.state(cx)));
                // Four states, told apart at a glance: running here (the
                // agent's colour, a light), running elsewhere (the agent's
                // colour, a badge that says where), asleep (dimmed, a moon)
                // and history only (grey, its age alone).
                let held = if running.is_none() {
                    self.elsewhere_of(session)
                } else {
                    None
                };
                let asleep =
                    running.is_none() && held.is_none() && self.slept.contains(&session.id);
                let tone = match (running.is_some() || held.is_some(), asleep) {
                    (true, _) => Tone::Full,
                    (false, true) => Tone::Asleep,
                    (false, false) => Tone::Faded,
                };
                let text_colour = match tone {
                    Tone::Full => colours.text,
                    Tone::Asleep => colours.text_muted,
                    Tone::Faded => colours.text_faint,
                };
                let name: SharedString = match (&held, tone) {
                    (Some(found), _) => {
                        format!("{name} · {}", self.holder_tip(found, session)).into()
                    }
                    (None, Tone::Full) => name,
                    (None, Tone::Asleep) => format!("{name} · asleep").into(),
                    (None, Tone::Faded) => format!("{name} · history").into(),
                };
                base.tooltip(move |window, cx| Tooltip::new(name.clone()).build(window, cx))
                    .child(self.pin_mark(index, pinned, colours, cx))
                    .child(
                        div()
                            .debug_selector(move || format!("tree-agent-{index}"))
                            .flex_none()
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(agent_icon_in(session.agent, px(14.), colours, tone)),
                    )
                    .child(
                        label()
                            .debug_selector(move || format!("tree-label-{index}"))
                            .text_color(text_colour)
                            .when(running.is_some() || held.is_some(), |this| {
                                this.font_weight(FontWeight::MEDIUM)
                            })
                            .child(session.title.clone()),
                    )
                    .when(asleep, |this| {
                        this.child(
                            div()
                                .debug_selector(move || format!("tree-asleep-{index}"))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(icon(IconName::Moon, px(11.), colours.text_faint))
                                .child(mono("SLEEP").text_color(colours.text_faint)),
                        )
                    })
                    .children(held.as_ref().map(|found| {
                        let key = session.id.to_string();
                        let other_leon = matches!(found.holder, Holder::OtherLeon(_));
                        div()
                            .debug_selector(move || format!("tree-held-{key}"))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(icon(IconName::ExternalLink, px(11.), colours.elsewhere))
                            .child(
                                mono(if other_leon {
                                    format!("OTHER {}", crate::product::PRODUCT_NAME.to_uppercase())
                                } else {
                                    "ELSEWHERE".to_owned()
                                })
                                .text_color(colours.elsewhere),
                            )
                            .into_any_element()
                    }))
                    .children(self.elsewhere_light(index, session, held.as_ref(), colours))
                    .children(match running {
                        Some((id, state)) => {
                            let light = live_light(state, colours);
                            let state_label = (!matches!(state, LiveState::Running)).then(|| {
                                mono(state.label())
                                    .debug_selector(move || format!("tree-live-state-{id}"))
                                    .text_color(colours.text_faint)
                                    .into_any_element()
                            });
                            let led = div()
                                .debug_selector(move || format!("tree-live-led-{id}"))
                                .child(led(light))
                                .into_any_element();
                            state_label.into_iter().chain([led]).collect()
                        }
                        None => vec![mono(format::age(self.now(), session.updated_at))
                            .text_color(colours.text_faint)
                            .into_any_element()],
                    })
                    .into_any_element()
            }
            Kind::Live(entry) => {
                let session = self.live.get(entry.id);
                let state = session.map_or(LiveState::Starting, |session| session.state(cx));
                let text = session.map_or_else(String::new, |session| session.label());
                let name: SharedString = entry.agent.map_or("Shell", format::agent_name).into();
                let lead = match entry.agent {
                    Some(agent) => agent_icon(agent, px(14.), colours).into_any_element(),
                    None => {
                        icon(IconName::Terminal, px(14.), colours.text_muted).into_any_element()
                    }
                };
                // The state is the light's colour, never the icon's.
                let light = live_light(state, colours);
                let id = entry.id;
                base.tooltip(move |window, cx| Tooltip::new(name.clone()).build(window, cx))
                    .child(chevron)
                    .child(
                        div()
                            .debug_selector(move || format!("tree-agent-{index}"))
                            .flex_none()
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(lead),
                    )
                    .child(label().font_weight(FontWeight::MEDIUM).child(text))
                    .when(!matches!(state, LiveState::Running), |this| {
                        this.child(
                            mono(state.label())
                                .debug_selector(move || format!("tree-live-state-{id}"))
                                .text_color(colours.text_faint),
                        )
                    })
                    .child(
                        div()
                            .debug_selector(move || format!("tree-live-led-{id}"))
                            .child(led(light)),
                    )
                    .into_any_element()
            }
            Kind::More { hidden } => base
                .child(chevron)
                .child(mono(format!("SHOW {hidden} MORE")).text_color(colours.text_muted))
                .into_any_element(),
            Kind::Pinned { sessions } => base
                .child(chevron)
                .child(icon(IconName::Pin, px(12.), colours.signal))
                .child(label().child(section_label("Pinned", colours)))
                .child(count(*sessions))
                .into_any_element(),
            Kind::Unsorted { sessions } => base
                .child(chevron)
                .child(label().child(section_label("Unsorted", colours)))
                .child(count(*sessions))
                .into_any_element(),
            Kind::Folder { cwd, sessions } => base
                .child(chevron)
                .child(icon(IconName::Folder, px(14.), colours.text_muted))
                .child(
                    label()
                        .text_color(colours.text_muted)
                        .child(folder_name(cwd).to_owned()),
                )
                .child(count(*sessions))
                .into_any_element(),
            Kind::NoMatch => base
                .child(chevron)
                .child(
                    label()
                        .debug_selector(|| "tree-no-match".into())
                        .text_color(colours.text_muted)
                        .child("No projects match"),
                )
                .into_any_element(),
            Kind::NoActive => base
                .child(chevron)
                .child(
                    label()
                        .debug_selector(|| "tree-no-active".into())
                        .text_color(colours.text_muted)
                        .child("No active sessions"),
                )
                .into_any_element(),
            Kind::Open => base
                .child(chevron)
                .child(
                    label()
                        .text_color(colours.text_muted)
                        .child("Open a project"),
                )
                .children(keys::keys_label(Command::OpenProject).map(|text| key_cap(text, colours)))
                .into_any_element(),
        }
    }

    /// The pin of a session, in the room a chevron takes: a pinned session
    /// always shows it, in the accent colour, and any other shows it while its
    /// row is hovered. Pressing it pins the session at the top of the Pinned
    /// section, or unpins it.
    fn pin_mark(
        &self,
        index: usize,
        pinned: bool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tip: SharedString = tooltip_text(if pinned {
            Command::UnpinSession
        } else {
            Command::PinSession
        })
        .into();
        let hover = colours.surface_2;
        div()
            .id(("pin", index))
            .debug_selector(move || format!("tree-pin-{index}"))
            .flex_none()
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .when(!pinned, |this| {
                this.invisible()
                    .group_hover(ROW_GROUP, |style| style.visible())
            })
            .hover(move |style| style.bg(hover))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.cursor = Some(index);
                this.pane = Pane::Sidebar;
                this.focus.focus(window, cx);
                this.pin_session_here(!pinned);
                cx.notify();
            }))
            .child(
                div()
                    .when(pinned, |this| {
                        this.debug_selector(move || format!("tree-pinned-{index}"))
                    })
                    .child(icon(
                        IconName::Pin,
                        px(12.),
                        if pinned {
                            colours.signal
                        } else {
                            colours.text_muted
                        },
                    )),
            )
            .into_any_element()
    }
}

/// How a project is called: its label, unless it is told apart another way.
impl Shell {
    /// What a project is called in the tree and the palette.
    pub(super) fn project_label(&self, project: &leon_core::Project) -> String {
        self.labels
            .get(&project.id)
            .cloned()
            .unwrap_or_else(|| project.name.clone())
    }

    /// A text with the characters that answer the filter emphasised in the
    /// text tokens: the text colour, in bold, over the muted row.
    fn emphasised(&self, text: &str, colours: &Palette) -> AnyElement {
        let Some(filter) = &self.filter else {
            return text.to_owned().into_any_element();
        };
        let places = matched_chars(&filter.query, text);
        if places.is_empty() {
            return text.to_owned().into_any_element();
        }
        let offsets: Vec<(usize, char)> = text.char_indices().collect();
        let style = HighlightStyle {
            color: Some(colours.text),
            font_weight: Some(FontWeight::BOLD),
            ..HighlightStyle::default()
        };
        let highlights = places
            .into_iter()
            .filter_map(|at| offsets.get(at))
            .map(|(start, c)| (*start..*start + c.len_utf8(), style))
            .collect::<Vec<_>>();
        StyledText::new(text.to_owned())
            .with_highlights(highlights)
            .into_any_element()
    }

    /// How the terminals of a worktree are doing: the most urgent state.
    pub(super) fn worktree_activity(&self, worktree: &leon_core::WorktreeId) -> Activity {
        self.placement
            .live_of_worktree(worktree)
            .iter()
            .filter_map(|place| self.live.get(self.placement.live[*place].id))
            .fold(Activity::Off, |all, session| {
                all.most_urgent(session.activity)
            })
    }

    /// How the terminals of a project are doing: the most urgent state of its
    /// worktrees and of the terminals no worktree holds.
    pub(super) fn project_activity(&self, project: &leon_core::ProjectId) -> Activity {
        let Some(entry) = self.snapshot.project(project) else {
            return Activity::Off;
        };
        let loose = self
            .placement
            .live_in_project
            .get(project)
            .into_iter()
            .flatten()
            .filter_map(|place| self.live.get(self.placement.live[*place].id))
            .fold(Activity::Off, |all, session| {
                all.most_urgent(session.activity)
            });
        entry
            .worktrees
            .iter()
            .map(|worktree| self.worktree_activity(&worktree.id))
            .fold(loose, Activity::most_urgent)
    }

    /// The activity a row's dot shows, for the rows that have one.
    #[cfg(test)]
    #[cfg_attr(not(leon_posix_tests), allow(dead_code))] // used by the Unix-only tests
    pub(super) fn row_activity(&self, row: &Row) -> Option<Activity> {
        match &row.kind {
            Kind::Worktree { worktree, .. } => Some(self.worktree_activity(&worktree.id)),
            Kind::Project { project, .. } => Some(self.project_activity(&project.id)),
            _ => None,
        }
    }

    /// The mark of a history session that a process in another terminal
    /// holds, with what it says on hover. A session live in Leon and also held
    /// elsewhere says that too.
    fn elsewhere_light(
        &self,
        index: usize,
        session: &leon_core::Session,
        held: Option<&crate::elsewhere::Found>,
        colours: &Palette,
    ) -> Option<AnyElement> {
        let found = match held {
            Some(found) => found.clone(),
            // Running here and held elsewhere too.
            None => self
                .live
                .of_history(&session.id)
                .and_then(|_| self.elsewhere_of(session))?,
        };
        let certain = found.is_certain();
        let tip: SharedString = if held.is_some() {
            self.holder_tip(&found, session).into()
        } else if certain {
            format!(
                "Another process holds this session too \u{b7} pid {}",
                found.pid
            )
            .into()
        } else {
            "Another process probably holds this session too".into()
        };
        let key = session.id.to_string();
        Some(
            div()
                .id(("elsewhere", index))
                .debug_selector(move || format!("tree-elsewhere-{key}"))
                .flex_none()
                .child(elsewhere_mark(certain, colours))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .into_any_element(),
        )
    }

    /// What a session held elsewhere says on hover: where it runs.
    fn holder_tip(&self, found: &crate::elsewhere::Found, session: &leon_core::Session) -> String {
        let probably = if found.is_certain() { "" } else { "probably " };
        match found.holder {
            Holder::OtherLeon(_) => {
                let machine = self
                    .snapshot
                    .machine(&session.machine_id)
                    .map_or("this machine", |machine| machine.name.as_str());
                format!(
                    "{probably}running in another Leon on {machine} \u{b7} pid {}",
                    found.pid
                )
            }
            Holder::Terminal => format!(
                "{probably}running in another terminal \u{b7} pid {}",
                found.pid
            ),
        }
    }

    /// The mark before a worktree's name: what its branch is. A branch icon
    /// for a branch (stronger for the main worktree), GitHub's merge icon in
    /// the accent — which no activity light wears, so a merged worktree never
    /// reads as a running one — when its pull request is merged, and a commit
    /// icon for a detached head. Its tooltip says it in words.
    ///
    /// Git alone cannot say a branch landed: a branch inside the base looks
    /// the same whether its work merged or it never had any. So "merged" comes
    /// from GitHub's record or from nowhere, and nothing is claimed for a
    /// worktree nobody could ask about. What the model does not hold (commits
    /// ahead or behind, uncommitted files, an open or draft pull request) is
    /// not drawn.
    fn git_mark(
        &self,
        index: usize,
        worktree: &leon_core::Worktree,
        colours: &Palette,
    ) -> AnyElement {
        let mark = GitMark::of(worktree);
        let (name, colour) = match mark {
            GitMark::Main => (IconName::GitBranch, colours.text),
            GitMark::Branch => (IconName::GitBranch, colours.text_muted),
            GitMark::Merged => (IconName::GitMerge, colours.signal),
            GitMark::Detached => (IconName::GitCommitHorizontal, colours.text_muted),
        };
        let tip: SharedString = mark.tip(worktree).into();
        div()
            .flex_none()
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .id(("git-mark", index))
            .debug_selector(move || format!("tree-git-{index}"))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .when(mark == GitMark::Merged, |this| {
                        this.debug_selector(move || format!("tree-merged-{index}"))
                    })
                    .child(icon(name, px(13.), colour)),
            )
            .into_any_element()
    }

    fn dot(&self, index: usize, activity: Activity, colours: &Palette) -> AnyElement {
        let tip: SharedString = activity.tooltip().into();
        activity_light(activity, colours)
            .id(("activity", index))
            .debug_selector(move || format!("tree-activity-{index}"))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .into_any_element()
    }
}

/// What a worktree's branch is, for the mark that leads its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GitMark {
    /// The repository's main worktree.
    Main,
    /// A branch, nothing known of its pull request or it is not merged.
    Branch,
    /// A branch whose pull request GitHub says is merged.
    Merged,
    /// A head that is on no branch.
    Detached,
}

impl GitMark {
    pub(super) fn of(worktree: &leon_core::Worktree) -> Self {
        if worktree.merged_pull_request == Some(true) {
            Self::Merged
        } else if worktree.is_main {
            Self::Main
        } else if worktree.branch.is_none() {
            Self::Detached
        } else {
            Self::Branch
        }
    }

    /// The tooltip: plain words about the branch.
    pub(super) fn tip(self, worktree: &leon_core::Worktree) -> String {
        let branch = worktree.branch.as_deref().unwrap_or("(no branch)");
        match self {
            Self::Main => format!("Main worktree \u{b7} {branch}"),
            Self::Branch => format!("Branch {branch}"),
            Self::Merged => format!("Branch {branch} \u{b7} pull request merged"),
            Self::Detached => match worktree.head.as_deref() {
                Some(head) => format!(
                    "Detached head at {}",
                    head.chars().take(7).collect::<String>()
                ),
                None => "Detached head".to_owned(),
            },
        }
    }
}

/// The word a row says beside its light when the agents in it need the person
/// or failed; the quiet states are the light alone.
pub(super) fn activity_word(activity: Activity) -> Option<&'static str> {
    match activity {
        Activity::Waiting => Some("WAITING"),
        Activity::Failed => Some("FAILED"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_says_in_a_word_whether_it_answers() {
        assert_eq!(state_label(&MachineState::Online(None)), "ONLINE");
        assert_eq!(state_label(&MachineState::Offline("x".into())), "OFFLINE");
        assert_eq!(state_label(&MachineState::Unknown), "UNKNOWN");
        assert_eq!(state_label(&MachineState::Probing), "PROBING");
    }

    #[test]
    fn each_state_has_its_own_colour_and_only_the_unknown_is_grey() {
        let colours = crate::theme::ThemeId::DEFAULT.palette(crate::theme::Appearance::Dark);
        let online = state_colour(&MachineState::Online(None), &colours);
        let offline = state_colour(&MachineState::Offline(String::new()), &colours);
        assert_ne!(online, offline);
        assert_eq!(
            state_colour(&MachineState::Unknown, &colours),
            colours.text_faint
        );
    }

    #[test]
    fn a_tool_tooltip_names_the_action_and_its_shortcut() {
        for command in [
            Command::ToggleAppearance,
            Command::Shortcuts,
            Command::Settings,
        ] {
            let tip = tooltip_text(command);
            assert!(tip.contains(keys::label(command)), "{tip}");
            assert!(
                tip.contains(&keys::keys_label(command).expect("these have keys")),
                "{tip}"
            );
        }
    }

    #[test]
    fn a_command_without_keys_has_a_tooltip_of_its_name_alone() {
        assert_eq!(
            tooltip_text(Command::RemoveWorktree),
            keys::label(Command::RemoveWorktree)
        );
    }

    fn worktree(branch: Option<&str>, is_main: bool, merged: Option<bool>) -> leon_core::Worktree {
        leon_core::Worktree {
            id: leon_core::WorktreeId::from_string("w"),
            project_id: leon_core::ProjectId::from_string("p"),
            path: "/srv/api".into(),
            branch: branch.map(str::to_owned),
            head: Some("0123456789abcdef".into()),
            is_main,
            merged_pull_request: merged,
        }
    }

    #[test]
    fn a_worktrees_git_mark_says_what_its_branch_is() {
        let of = |worktree| GitMark::of(&worktree);
        assert_eq!(of(worktree(Some("main"), true, None)), GitMark::Main);
        assert_eq!(of(worktree(Some("feat/x"), false, None)), GitMark::Branch);
        assert_eq!(
            of(worktree(Some("feat/x"), false, Some(false))),
            GitMark::Branch
        );
        assert_eq!(
            of(worktree(Some("feat/x"), false, Some(true))),
            GitMark::Merged
        );
        assert_eq!(of(worktree(None, false, None)), GitMark::Detached);
    }

    #[test]
    fn the_git_mark_tooltip_is_plain_words() {
        let tip = |w: leon_core::Worktree| GitMark::of(&w).tip(&w);
        assert_eq!(
            tip(worktree(Some("main"), true, None)),
            "Main worktree \u{b7} main"
        );
        assert_eq!(tip(worktree(Some("feat/x"), false, None)), "Branch feat/x");
        assert_eq!(
            tip(worktree(Some("feat/x"), false, Some(true))),
            "Branch feat/x \u{b7} pull request merged"
        );
        assert_eq!(tip(worktree(None, false, None)), "Detached head at 0123456");
    }

    #[test]
    fn only_the_states_that_need_the_person_say_a_word() {
        assert_eq!(activity_word(Activity::Waiting), Some("WAITING"));
        assert_eq!(activity_word(Activity::Failed), Some("FAILED"));
        for quiet in [Activity::Off, Activity::Idle, Activity::Working] {
            assert_eq!(activity_word(quiet), None);
        }
    }
}

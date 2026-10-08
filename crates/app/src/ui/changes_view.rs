//! Drawing the changes tab: the list of changed files on the left, the
//! diff of the selected one beside it, and under both the commit message, the
//! buttons of the steps, the pull request's fields and what the last step
//! printed.
//!
//! Nothing here runs a command: the view draws what [`ChangesView`] holds and
//! asks the shell to act (`changes.rs`). Colours are the theme's: added lines
//! are the `success` token, removed ones `error`, hunk headers `info`, with the
//! band behind them the same token at [`DIFF_TINT`].

use super::changes::{chosen_words, Allowed, ChangesView, DiffState, Steps};
use super::live::LiveId;
use super::shell::{Pane, Shell};
use super::widgets::{mono, section_label};
use crate::icons::agent_icon;
use crate::theme::{hairline, metrics, px, Palette, DIFF_TINT};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    div, uniform_list, AnyElement, ClickEvent, Context, Div, FontWeight, Hsla, MouseDownEvent,
    SharedString, Stateful,
};
use leon_remote::{ChangedFile, DiffLine, FileDiff, FileStatus, LineKind, Suggest};

/// The width of the list of files.
const LIST_WIDTH: f32 = 300.;

/// The height of a line of a diff.
const LINE_HEIGHT: f32 = 18.;

/// The colour of the letter of a file's status.
fn status_colour(status: FileStatus, colours: &Palette) -> Hsla {
    match status {
        FileStatus::Modified => colours.warning,
        FileStatus::Added | FileStatus::Untracked => colours.success,
        FileStatus::Deleted | FileStatus::Conflicted => colours.error,
        FileStatus::Renamed | FileStatus::Copied => colours.info,
    }
}

/// The text colour and the band behind a line of a diff.
fn line_colours(kind: LineKind, colours: &Palette) -> (Hsla, Option<Hsla>) {
    match kind {
        LineKind::Added => (colours.success, Some(colours.success.opacity(DIFF_TINT))),
        LineKind::Removed => (colours.error, Some(colours.error.opacity(DIFF_TINT))),
        LineKind::Hunk => (colours.info, Some(colours.info.opacity(DIFF_TINT))),
        LineKind::Meta | LineKind::Note => (colours.text_faint, None),
        LineKind::Context => (colours.text, None),
    }
}

/// A line number for the gutter, or blanks.
fn gutter(number: Option<u32>) -> String {
    number.map_or_else(String::new, |number| number.to_string())
}

/// The sign at the start of a line.
fn sign(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Added => "+",
        LineKind::Removed => "-",
        _ => "",
    }
}

/// A size in bytes, as a person reads it.
fn size_words(bytes: usize) -> String {
    match bytes {
        0..=1023 => format!("{bytes} bytes"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// The directory of a path with its last separator, and its file name.
fn split_name(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(at) => path.split_at(at + 1),
        None => ("", path),
    }
}

/// A button of the tab: hairline frame and a mono label; `primary` fills it
/// with the accent. A button that is not allowed is faint and does nothing.
#[allow(clippy::too_many_arguments)]
fn button(
    id: &'static str,
    label: impl Into<SharedString>,
    tip: impl Into<SharedString>,
    enabled: bool,
    primary: bool,
    colours: &Palette,
    cx: &mut Context<Shell>,
    run: impl Fn(&mut Shell, &mut gpui_kit::Window, &mut Context<Shell>) + 'static,
) -> Stateful<Div> {
    let hover = colours.surface_2;
    let tip: SharedString = tip.into();
    let text = if !enabled {
        colours.text_faint
    } else if primary {
        colours.on_primary
    } else {
        colours.text_muted
    };
    let base = div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex_none()
        .h(metrics::CONTROL())
        .px(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .rounded(metrics::RADIUS())
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(mono(label).text_color(text));
    let base = if primary && enabled {
        base.bg(colours.primary_fill)
    } else {
        base.border_1().border_color(colours.border)
    };
    if enabled {
        base.cursor_pointer()
            .when(!primary, |this| this.hover(move |style| style.bg(hover)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| run(this, window, cx)))
    } else {
        base
    }
}

impl Shell {
    /// The tab of a changes view: its name and how many files changed.
    pub(in crate::ui) fn changes_label(view: &ChangesView) -> String {
        match view.files.as_ref().map(Vec::len) {
            Some(count) if count > 0 => format!("Changes {count}"),
            _ => "Changes".to_owned(),
        }
    }

    /// What the header says of a changes view: the kind of thing, the branch
    /// and a line about it.
    pub(in crate::ui) fn changes_heading(
        &self,
        view: &ChangesView,
    ) -> (&'static str, String, String) {
        let machine = self
            .snapshot
            .machine(&view.target.machine)
            .map_or_else(String::new, |machine| machine.name.to_uppercase());
        let branch = view
            .branch
            .clone()
            .unwrap_or_else(|| "detached head".to_owned());
        (
            "Changes",
            branch,
            format!("{machine} \u{b7} {}", view.target.path),
        )
    }

    /// The leaf of a changes view, with the accent outline when it is the
    /// focused one of several.
    pub(in crate::ui) fn render_changes_pane(
        &self,
        id: LiveId,
        marked: bool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.changes.get(&id)?;
        let body = div()
            .flex_1()
            .min_h(px(160.))
            .flex()
            .child(self.render_changes_list(id, view, colours, cx))
            .child(self.render_changes_diff(id, view, colours, cx));
        Some(
            div()
                .debug_selector(move || format!("pane-{}", id.0))
                .relative()
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .rounded(metrics::RADIUS_CELL())
                .border_1()
                .border_color(if marked {
                    colours.signal
                } else {
                    gpui_kit::transparent_black()
                })
                // A click anywhere in it makes it the focused pane of its tab.
                .capture_any_mouse_down(cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.pane = Pane::Main;
                    this.open_live(id, window, cx);
                }))
                .child(self.render_changes_bar(id, view, colours, cx))
                .child(body)
                .child(self.render_changes_actions(id, view, colours, cx))
                .into_any_element(),
        )
    }

    /// The strip above the list and the diff: where the branch stands.
    fn render_changes_bar(
        &self,
        id: LiveId,
        view: &ChangesView,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let upstream = match (&view.branch, view.has_upstream) {
            (None, _) => "DETACHED HEAD: CHECK OUT A BRANCH TO PUSH".to_owned(),
            (Some(_), true) => "HAS AN UPSTREAM".to_owned(),
            (Some(_), false) => "NO UPSTREAM: PUSH SETS IT ON ORIGIN".to_owned(),
        };
        div()
            .debug_selector(|| "changes-bar".into())
            .flex_none()
            .h(px(34.))
            .px_3()
            .gap_3()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(colours.border)
            .child(section_label("Changes", colours))
            .child(
                mono(view.branch.clone().unwrap_or_else(|| "DETACHED".to_owned()))
                    .debug_selector(|| "changes-branch".into())
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(metrics::RADIUS())
                    .border_1()
                    .border_color(colours.elevated_border)
                    .text_color(colours.text),
            )
            .child(mono(upstream).text_color(colours.text_faint))
            .child(div().flex_1())
            .child(button(
                "changes-refresh",
                "REFRESH",
                "Read the changes again",
                view.working.is_none(),
                false,
                colours,
                cx,
                move |this, window, cx| this.changes_refresh(id, window, cx),
            ))
    }

    /// The list of changed files, each with the box that puts it in the
    /// commit.
    fn render_changes_list(
        &self,
        id: LiveId,
        view: &ChangesView,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let files = view.files.as_deref();
        let total = files.map_or(0, <[ChangedFile]>::len);
        let chosen = view.chosen().len();
        let palette = *colours;
        let heading = div()
            .flex_none()
            .h(px(30.))
            .px_3()
            .gap_2()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(colours.border)
            .child(
                mono(chosen_words(total, chosen))
                    .debug_selector(|| "changes-count".into())
                    .text_color(colours.text_muted),
            )
            .child(div().flex_1())
            .when(total > 0, |this| {
                this.child(
                    div()
                        .id("changes-toggle-all")
                        .debug_selector(|| "changes-toggle-all".into())
                        .cursor_pointer()
                        .text_color(colours.text_faint)
                        .hover(|style| style.text_color(palette.text))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.changes_toggle_all(id, cx);
                        }))
                        .child(mono(if view.unchosen.is_empty() {
                            "NONE"
                        } else {
                            "ALL"
                        })),
                )
            });
        let content = match files {
            None => placeholder("changes-reading", "Reading the changes\u{2026}", colours),
            Some([]) => match &view.failed {
                Some(why) => failure("changes-failed", why, colours),
                None => placeholder("changes-clean", "No changes: the tree is clean.", colours),
            },
            Some(files) => {
                let count = files.len();
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "changes-files",
                            count,
                            cx.processor(
                                move |this, range: std::ops::Range<usize>, _window, cx| {
                                    range
                                        .filter_map(|index| {
                                            this.render_changes_row(id, index, &palette, cx)
                                        })
                                        .collect::<Vec<_>>()
                                },
                            ),
                        )
                        .track_scroll(&view.list_scroll)
                        .size_full(),
                    )
                    .child(Scrollbar::vertical(&view.list_scroll))
                    .into_any_element()
            }
        };
        div()
            .debug_selector(|| "changes-list".into())
            .flex_none()
            .w(px(LIST_WIDTH))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colours.border)
            .child(heading)
            .child(content)
    }

    /// One file of the list.
    fn render_changes_row(
        &self,
        id: LiveId,
        index: usize,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.changes.get(&id)?;
        let file = view.files.as_ref()?.get(index)?;
        let on = view.selected.as_deref() == Some(file.path.as_str());
        let in_commit = !view.unchosen.contains(&file.path);
        let focused = self.pane == Pane::Main && self.focused_changes() == Some(id);
        let hover = colours.surface;
        let (directory, name) = split_name(&file.path);
        let path = file.path.clone();
        let toggled = file.path.clone();
        let tip: SharedString = match &file.from {
            Some(from) => format!("{} \u{2190} {from} ({})", file.path, file.status.words()),
            None => format!("{} ({})", file.path, file.status.words()),
        }
        .into();
        Some(
            div()
                .id(("changes-file", index))
                .debug_selector(move || format!("changes-file-{index}"))
                .relative()
                .flex_none()
                .h(px(26.))
                .w_full()
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .overflow_hidden()
                .cursor_pointer()
                .when(on, |this| this.bg(colours.surface_2))
                .when(!on, |this| this.hover(move |style| style.bg(hover)))
                .when(on && focused, |this| {
                    this.child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(2.))
                            .bg(colours.signal),
                    )
                })
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.pane = Pane::Main;
                    this.changes_select(id, path.clone(), window, cx);
                    if event.click_count() >= 2 {
                        this.changes_open(id, Some(path.clone()), window, cx);
                    }
                }))
                .child(
                    div()
                        .id(("changes-pick", index))
                        .debug_selector(move || format!("changes-pick-{index}"))
                        .flex_none()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.changes_toggle(id, &toggled, cx);
                        }))
                        .child(mono(if in_commit { "[x]" } else { "[ ]" }).text_color(
                            if in_commit {
                                colours.text
                            } else {
                                colours.text_faint
                            },
                        )),
                )
                .child(
                    mono(file.status.letter().to_string())
                        .debug_selector(move || format!("changes-status-{index}"))
                        .text_color(status_colour(file.status, colours)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(
                            div()
                                .text_color(colours.text_faint)
                                .child(directory.to_owned()),
                        )
                        .child(div().text_color(colours.text).child(name.to_owned())),
                )
                .into_any_element(),
        )
    }

    /// The diff of the selected file.
    fn render_changes_diff(
        &self,
        id: LiveId,
        view: &ChangesView,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let heading = match view.selected_file() {
            Some(file) => {
                let from = file
                    .from
                    .as_ref()
                    .map(|from| format!("  \u{2190} {from}"))
                    .unwrap_or_default();
                format!("{}{from}  \u{b7}  {}", file.path, file.status.words())
            }
            None => "No file selected".to_owned(),
        };
        let palette = *colours;
        let content = match &view.diff {
            DiffState::None => placeholder(
                "changes-no-diff",
                "Select a file to see what changed in it.",
                colours,
            ),
            DiffState::Loading => placeholder("changes-diff-loading", "Reading the diff\u{2026}", colours),
            DiffState::Failed(why) => failure("changes-diff-failed", why, colours),
            DiffState::Ready(FileDiff::Binary) => placeholder(
                "changes-binary",
                "This file is not text: there is no diff to show.",
                colours,
            ),
            DiffState::Ready(FileDiff::TooLarge { bytes }) => placeholder(
                "changes-too-large",
                &format!(
                    "The diff of this file is too large to show here ({}). Open the file to read it.",
                    size_words(*bytes)
                ),
                colours,
            ),
            DiffState::Ready(FileDiff::Empty) => placeholder(
                "changes-empty-diff",
                "Nothing to show: the file is empty, or only its mode changed.",
                colours,
            ),
            DiffState::Ready(FileDiff::Lines(lines)) => {
                let count = lines.len();
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "changes-diff",
                            count,
                            cx.processor(
                                move |this, range: std::ops::Range<usize>, _window, cx| {
                                    let font = Self::terminal_font(cx).family;
                                    let size = crate::theme::px(
                                        crate::settings::int(cx, "editor_font_size") as f32,
                                    );
                                    let Some(DiffState::Ready(FileDiff::Lines(lines))) =
                                        this.changes.get(&id).map(|view| &view.diff)
                                    else {
                                        return Vec::new();
                                    };
                                    range
                                        .filter_map(|index| {
                                            lines.get(index).map(|line| {
                                                diff_line(
                                                    index,
                                                    line,
                                                    font.clone(),
                                                    size,
                                                    &palette,
                                                )
                                            })
                                        })
                                        .collect::<Vec<_>>()
                                },
                            ),
                        )
                        .track_scroll(&view.diff_scroll)
                        .size_full(),
                    )
                    .child(Scrollbar::vertical(&view.diff_scroll))
                    .into_any_element()
            }
        };
        div()
            .debug_selector(|| "changes-diff-pane".into())
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(px(30.))
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(colours.border)
                    .min_w_0()
                    .truncate()
                    .text_size(metrics::TEXT_SMALL())
                    .font_weight(FontWeight::MEDIUM)
                    .debug_selector(|| "changes-diff-title".into())
                    .child(heading),
            )
            .child(content)
    }

    /// The message, the buttons, the pull request's fields and the outcome.
    fn render_changes_actions(
        &self,
        id: LiveId,
        view: &ChangesView,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let ok: Allowed = self.changes_allowed(id, cx);
        let busy = view.working.is_some();
        let field = |content: AnyElement| {
            div()
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.border)
                .px(px(4.))
                .child(content)
        };
        let message = field(
            Textarea::new(&view.message)
                .appearance(false)
                .bordered(false)
                .h(px(64.))
                .into_any_element(),
        );
        let candidates =
            super::changes::headless_agents(&self.agents_for_buttons(&view.target.machine));
        let suggest = view.agent.map(|agent| {
            let words = if view.suggesting {
                "ASKING\u{2026}"
            } else {
                "SUGGEST A MESSAGE"
            };
            let tip = format!(
                "Ask {} to write the message from the diff. It never blocks committing.",
                agent.name()
            );
            let chip = div()
                .id("changes-agent")
                .debug_selector(|| "changes-agent".into())
                .flex_none()
                .flex()
                .items_center()
                .h(metrics::CONTROL())
                .px(px(6.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.border)
                .child(agent_icon(agent, px(14.), colours))
                .when(candidates.len() > 1, |this| {
                    this.cursor_pointer()
                        .tooltip(move |window, cx| {
                            Tooltip::new("Ask another agent").build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.changes_next_agent(id, cx);
                        }))
                });
            div().flex().gap(px(6.)).child(chip).child(button(
                "changes-suggest",
                words,
                tip,
                !view.suggesting,
                false,
                colours,
                cx,
                move |this, window, cx| this.changes_suggest(id, Suggest::Commit, window, cx),
            ))
        });
        let buttons = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(button(
                "changes-commit",
                "COMMIT",
                "git add and git commit of the files in the commit",
                ok.commit,
                false,
                colours,
                cx,
                move |this, window, cx| this.changes_ship(id, Steps::Commit, window, cx),
            ))
            .child(button(
                "changes-push",
                "PUSH",
                match view.has_upstream {
                    true => "git push",
                    false => "git push --set-upstream origin, since the branch has no upstream",
                },
                ok.push,
                false,
                colours,
                cx,
                move |this, window, cx| this.changes_ship(id, Steps::Push, window, cx),
            ))
            .child(button(
                "changes-pull-request",
                if view.form {
                    "HIDE PULL REQUEST"
                } else {
                    "PULL REQUEST\u{2026}"
                },
                "Open a pull request with gh",
                !busy,
                false,
                colours,
                cx,
                move |this, window, cx| this.changes_toggle_form(id, window, cx),
            ))
            .child(button(
                "changes-ship",
                "COMMIT, PUSH AND OPEN A PULL REQUEST",
                super::sidebar::tooltip_text(crate::keys::Command::ShipChanges),
                ok.all,
                true,
                colours,
                cx,
                move |this, window, cx| this.changes_ship(id, Steps::All, window, cx),
            ))
            .children(suggest)
            .children(view.working.map(|words| {
                mono(words)
                    .debug_selector(|| "changes-working".into())
                    .text_color(colours.warning)
            }));
        let form = view.form.then(|| {
            let pull_request_words = if view.suggesting {
                "ASKING\u{2026}"
            } else {
                "SUGGEST TITLE AND BODY"
            };
            let suggest_pull_request = view.agent.map(|agent| {
                button(
                    "changes-suggest-pr",
                    pull_request_words,
                    format!("Ask {} to write them from the commits", agent.name()),
                    !view.suggesting,
                    false,
                    colours,
                    cx,
                    move |this, window, cx| {
                        this.changes_suggest(id, Suggest::PullRequest, window, cx)
                    },
                )
            });
            div()
                .debug_selector(|| "changes-form".into())
                .flex()
                .flex_col()
                .gap_2()
                .child(field(
                    Input::new(&view.title)
                        .appearance(false)
                        .bordered(false)
                        .into_any_element(),
                ))
                .child(field(
                    Textarea::new(&view.body)
                        .appearance(false)
                        .bordered(false)
                        .h(px(64.))
                        .into_any_element(),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(mono("INTO").text_color(colours.text_faint))
                        .child(
                            div().w(px(160.)).child(field(
                                Input::new(&view.base)
                                    .appearance(false)
                                    .bordered(false)
                                    .into_any_element(),
                            )),
                        )
                        .child(
                            div()
                                .id("changes-draft")
                                .debug_selector(|| "changes-draft".into())
                                .flex_none()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.changes_toggle_draft(id, cx);
                                }))
                                .child(
                                    mono(if view.draft { "[x] DRAFT" } else { "[ ] DRAFT" })
                                        .text_color(colours.text_muted),
                                ),
                        )
                        .child(div().flex_1())
                        .children(suggest_pull_request)
                        .child(button(
                            "changes-open-pull-request",
                            "OPEN PULL REQUEST",
                            "gh pr create with this title and body",
                            ok.pull_request,
                            true,
                            colours,
                            cx,
                            move |this, window, cx| {
                                this.changes_ship(id, Steps::PullRequest, window, cx)
                            },
                        )),
                )
        });
        div()
            .debug_selector(|| "changes-actions".into())
            .flex_none()
            .p_3()
            .gap_2()
            .flex()
            .flex_col()
            .border_t(hairline())
            .border_color(colours.border)
            .child(message)
            .child(buttons)
            .children(form)
            .children(
                view.outcome
                    .as_ref()
                    .map(|outcome| self.render_outcome(outcome, colours, cx)),
            )
    }

    /// What the last step came to: its title in the colour of how it went, the
    /// address of a pull request that was opened, and what the commands printed.
    fn render_outcome(
        &self,
        outcome: &super::changes::Outcome,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let colour = if outcome.ok {
            colours.success
        } else {
            colours.error
        };
        let link = outcome.url.clone().map(|url| {
            let target = url.clone();
            div()
                .id("changes-url")
                .debug_selector(|| "changes-url".into())
                .cursor_pointer()
                .text_color(colours.signal)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.changes_open_url(&target, cx);
                }))
                .child(url)
        });
        div()
            .debug_selector(|| "changes-outcome".into())
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .debug_selector(|| "changes-outcome-title".into())
                    .text_color(colour)
                    .font_weight(FontWeight::MEDIUM)
                    .child(outcome.title.clone()),
            )
            .children(link)
            .when(!outcome.text.is_empty(), |this| {
                this.child(
                    div()
                        .id("changes-outcome-text")
                        .debug_selector(|| "changes-outcome-text".into())
                        .max_h(px(140.))
                        .overflow_y_scroll()
                        .font_family(crate::theme::fonts::mono())
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(outcome.text.clone()),
                )
            })
    }
}

/// One line of a diff: the numbers of both sides, the sign and the text, on
/// the band of its kind.
fn diff_line(
    index: usize,
    line: &DiffLine,
    font: SharedString,
    size: gpui_kit::Pixels,
    colours: &Palette,
) -> AnyElement {
    let (text, band) = line_colours(line.kind, colours);
    let numbers = matches!(
        line.kind,
        LineKind::Added | LineKind::Removed | LineKind::Context
    );
    div()
        .debug_selector(move || format!("changes-line-{index}"))
        .flex_none()
        .h(px(LINE_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .font_family(font)
        .text_size(size)
        .text_color(text)
        .when_some(band, |this, band| this.bg(band))
        .child(
            div()
                .flex_none()
                .w(px(44.))
                .pr(px(6.))
                .flex()
                .justify_end()
                .text_color(colours.text_faint)
                .child(if numbers {
                    gutter(line.old)
                } else {
                    String::new()
                }),
        )
        .child(
            div()
                .flex_none()
                .w(px(44.))
                .pr(px(6.))
                .flex()
                .justify_end()
                .text_color(colours.text_faint)
                .child(if numbers {
                    gutter(line.new)
                } else {
                    String::new()
                }),
        )
        .child(div().flex_none().w(px(14.)).child(sign(line.kind)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(line.text.clone()),
        )
        .into_any_element()
}

/// A short sentence in the middle of a pane.
fn placeholder(selector: &'static str, text: &str, colours: &Palette) -> AnyElement {
    div()
        .debug_selector(move || selector.to_owned())
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .px(px(16.))
        .text_size(metrics::TEXT_SMALL())
        .text_color(colours.text_faint)
        .child(text.to_owned())
        .into_any_element()
}

/// What git said when it could not answer, whole, in the error colour.
fn failure(selector: &'static str, text: &str, colours: &Palette) -> AnyElement {
    div()
        .debug_selector(move || selector.to_owned())
        .flex_1()
        .p(px(12.))
        .font_family(crate::theme::fonts::mono())
        .text_size(metrics::TEXT_SMALL())
        .text_color(colours.error)
        .child(text.to_owned())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_told_by_its_sign_and_its_numbers_by_blanks() {
        assert_eq!(sign(LineKind::Added), "+");
        assert_eq!(sign(LineKind::Removed), "-");
        assert_eq!(sign(LineKind::Context), "");
        assert_eq!(gutter(Some(12)), "12");
        assert_eq!(gutter(None), "");
    }

    #[test]
    fn a_path_is_split_into_its_directory_and_its_name() {
        assert_eq!(split_name("src/ui/lib.rs"), ("src/ui/", "lib.rs"));
        assert_eq!(split_name("README.md"), ("", "README.md"));
    }

    #[test]
    fn sizes_are_said_the_way_a_person_reads_them() {
        assert_eq!(size_words(12), "12 bytes");
        assert_eq!(size_words(2048), "2.0 KB");
        assert_eq!(size_words(3 * 1_048_576), "3.0 MB");
    }
}

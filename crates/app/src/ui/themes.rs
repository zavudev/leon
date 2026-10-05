//! Themes from files, in the window: the themes folder, live reloading, the
//! commands that write theme files and the list of problems.
//!
//! The data side (`theme/`) reads and checks files; this is the part that
//! touches the user. The folder is `themes/` beside `settings.json`; once a
//! second the shell lists it (see `theme/watch.rs` for why it polls) and, when
//! a file changed for good, reads the folder again and puts the theme in use
//! back on screen, terminals included. A theme whose file became invalid keeps
//! the last good version on screen and the status line says what is wrong.

use super::shell::{Overlay, Shell};
use super::widgets::{mono, section_label};
use crate::engine::StatusKind;
use crate::settings;
use crate::theme::author;
use crate::theme::registry;
use crate::theme::user;
use crate::theme::watch::{self, Watcher};
use crate::theme::{metrics, px, Palette, ThemeId};
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, Stateful, Task};
use std::path::{Path, PathBuf};

/// The folder of theme files beside the settings.
pub const FOLDER: &str = "themes";

/// What the shell keeps to follow the folder.
pub struct ThemeFiles {
    /// The folder; none while the settings live in memory only.
    pub folder: Option<PathBuf>,
    watcher: Watcher,
    _poll: Option<Task<()>>,
}

impl ThemeFiles {
    /// Follows `folder`, as it is now.
    pub fn new(folder: Option<PathBuf>) -> Self {
        let now = folder.as_deref().map(watch::snapshot).unwrap_or_default();
        Self {
            folder,
            watcher: Watcher::new(now),
            _poll: None,
        }
    }
}

/// A file name for a theme with this name.
fn id_for(name: &str) -> String {
    crate::theme::file_slug(name)
}

impl Shell {
    /// Starts listing the folder every [`watch::POLL`], unless the options ask
    /// for no timer (tests drive [`Self::poll_themes`] themselves).
    pub(super) fn watch_themes(&mut self, cx: &mut Context<Self>) {
        let Some(every) = self.options.theme_poll else {
            return;
        };
        if self.themes.folder.is_none() {
            return;
        }
        self.themes._poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(every).await;
            let alive = this.update(cx, |this, cx| {
                this.poll_themes(cx);
                this.poll_settings(cx);
            });
            if alive.is_err() {
                break;
            }
        }));
    }

    /// Lists the folder; when it changed for good, reads it again.
    pub(super) fn poll_themes(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.themes.folder.clone() else {
            return;
        };
        if self.themes.watcher.step(watch::snapshot(&folder)) {
            self.reload_themes(cx, false);
        }
    }

    /// Reads the folder again and puts the theme in use back on screen.
    pub(super) fn reload_themes(&mut self, cx: &mut Context<Self>, announce: bool) {
        let Some(folder) = self.themes.folder.clone() else {
            if announce {
                self.engine.report(
                    StatusKind::Info,
                    "There is no themes folder: the settings are kept in memory.",
                );
            }
            return;
        };
        let summary = user::reload(&folder);
        // Whatever is on screen, user theme or not, is drawn again.
        settings::reapply(cx);
        let kept = settings::kept_theme_id(cx);
        let broken = (!kept.is_usable()).then(|| kept.problem()).flatten();
        match broken {
            Some(reason) => self.engine.report(
                StatusKind::Error,
                format!(
                    "Theme {} has a problem: {reason} (Show theme problems has the rest).",
                    kept.name()
                ),
            ),
            None if announce => self.engine.report(
                StatusKind::Info,
                format!(
                    "Reloaded {} theme{}{}.",
                    summary.loaded.len(),
                    if summary.loaded.len() == 1 { "" } else { "s" },
                    if summary.invalid.is_empty() {
                        String::new()
                    } else {
                        format!(", {} invalid", summary.invalid.len())
                    }
                ),
            ),
            None => {
                let warned = registry::problems()
                    .iter()
                    .filter(|p| p.file_of(kept))
                    .count();
                if warned > 0 && !kept.is_builtin() {
                    self.engine.report(
                        StatusKind::Info,
                        format!("Theme {} reloaded with {warned} warning(s).", kept.name()),
                    );
                }
            }
        }
        cx.notify();
    }

    /// Makes sure the folder exists.
    fn themes_folder(&self) -> Option<PathBuf> {
        let folder = self.themes.folder.clone()?;
        match std::fs::create_dir_all(&folder) {
            Ok(()) => Some(folder),
            Err(error) => {
                self.engine.report(
                    StatusKind::Error,
                    format!("Could not create {}: {error}.", folder.display()),
                );
                None
            }
        }
    }

    /// Shows the themes folder in the file manager.
    pub(super) fn open_themes_folder(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.themes_folder() else {
            self.engine.report(
                StatusKind::Info,
                "There is no themes folder: the settings are kept in memory.",
            );
            return;
        };
        let reveal = self.options.reveal.clone();
        reveal(cx, &folder);
        self.engine.report(
            StatusKind::Info,
            format!("Themes folder: {}", folder.display()),
        );
    }

    fn write_theme(&mut self, file: &Path, text: &str, what: &str, cx: &mut Context<Self>) {
        if file.exists() {
            self.engine.report(
                StatusKind::Error,
                format!("{} exists already: it was not overwritten.", file.display()),
            );
            return;
        }
        if let Err(error) = std::fs::write(file, text) {
            self.engine.report(
                StatusKind::Error,
                format!("Could not write {}: {error}.", file.display()),
            );
            return;
        }
        self.reload_themes(cx, false);
        let reveal = self.options.reveal.clone();
        reveal(cx, file);
        self.engine
            .report(StatusKind::Info, format!("{what}: {}", file.display()));
    }

    /// Writes `themes/<name>.toml`: a file that extends the theme in use and
    /// lists every token, commented out.
    pub(super) fn create_theme(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(folder) = self.themes_folder() else {
            self.engine.report(
                StatusKind::Info,
                "There is no themes folder: the settings are kept in memory.",
            );
            return;
        };
        let id = id_for(name);
        if !crate::theme::valid_id(&id) {
            self.engine.report(
                StatusKind::Error,
                "Use a name with letters or digits in it.",
            );
            return;
        }
        if registry::RESERVED.contains(&id.as_str()) {
            self.engine.report(
                StatusKind::Error,
                format!("`{id}` is a built-in theme's id: choose another name."),
            );
            return;
        }
        let parent = settings::kept_theme_id(cx);
        let text = author::render(&parent.theme(), &id, name.trim(), "", Some(parent.slug()));
        let file = folder.join(format!("{id}.toml"));
        self.write_theme(
            &file,
            &text,
            &format!("Created a theme that extends {}", parent.name()),
            cx,
        );
    }

    /// Writes a complete standalone file of the theme in use.
    pub(super) fn export_theme(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.themes_folder() else {
            self.engine.report(
                StatusKind::Info,
                "There is no themes folder: the settings are kept in memory.",
            );
            return;
        };
        let current: ThemeId = settings::kept_theme_id(cx);
        let mut number = 1;
        let id = loop {
            let id = if number == 1 {
                format!("{}-copy", current.slug())
            } else {
                format!("{}-copy-{number}", current.slug())
            };
            if !folder.join(format!("{id}.toml")).exists() {
                break id;
            }
            number += 1;
        };
        let name = format!("{} copy", current.name());
        let text = author::render(&current.theme(), &id, &name, "", None);
        let file = folder.join(format!("{id}.toml"));
        self.write_theme(&file, &text, &format!("Exported {}", current.name()), cx);
    }

    /// Opens the list of problems of the theme files.
    pub(super) fn show_theme_problems(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(window, cx);
        self.overlay = Overlay::Problems;
        self.sheet_scroll
            .set_offset(gpui_kit::point(px(0.), px(0.)));
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// The list of problems: every error and warning of every file, with what
    /// was measured and what is required.
    pub(super) fn render_problems(&self, colours: &Palette) -> Stateful<Div> {
        let problems = registry::problems();
        let errors = problems.iter().filter(|p| p.is_error()).count();
        let mut body = div()
            .id("problems-scroll")
            .debug_selector(|| "problems-scroll".into())
            .track_scroll(&self.sheet_scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap(px(8.));
        if problems.is_empty() {
            body = body.child(
                div()
                    .debug_selector(|| "problems-none".into())
                    .text_color(colours.text_muted)
                    .child("No problems: every theme file loads cleanly."),
            );
        }
        for (index, problem) in problems.iter().enumerate() {
            let colour = if problem.is_error() {
                colours.error
            } else {
                colours.warning
            };
            let measured = match (problem.measured, problem.required) {
                (Some(measured), Some(required)) => {
                    format!("measured {measured:.2}, required {required:.2}")
                }
                _ => String::new(),
            };
            body = body.child(
                div()
                    .debug_selector(move || format!("problem-{index}"))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                mono(if problem.is_error() {
                                    "ERROR"
                                } else {
                                    "WARNING"
                                })
                                .text_color(colour),
                            )
                            .child(mono(problem.file.clone()).text_color(colours.text_muted))
                            .child(mono(problem.key.clone()).text_color(colours.text_faint)),
                    )
                    .child(
                        div()
                            .text_size(metrics::TEXT_SMALL())
                            .child(problem.message.clone()),
                    )
                    .when(!measured.is_empty(), |this| {
                        this.child(mono(measured).text_color(colours.text_faint))
                    }),
            );
        }
        self.card("theme-problems", colours)
            .w(px(640.))
            .max_w(self.viewport.width - px(32.))
            .max_h(self.viewport.height - px(112.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(px(44.))
                    .px_4()
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_label("Theme problems", colours))
                    .child(
                        mono(format!("{errors} ERRORS · {} TOTAL", problems.len()))
                            .text_color(colours.text_muted),
                    ),
            )
            .child(body)
    }
}

//! Updates in the window: the footer's item, the release notes, the
//! commands and the timer. What each state says is in `crate::updates`; the
//! work is `leon-update`'s. This file is the glue: it watches the updater,
//! asks it at the right times, and draws.
//!
//! Never restarts the application by itself. A restart ends every terminal,
//! so it is always the person's ("Restart to update", confirmed with the
//! number of sessions it closes); the one thing done without asking is
//! installing a ready update on the way out of a quit that was happening
//! anyway, in the automatic mode, with nothing running.

use super::shell::{Overlay, Shell};
use super::widgets::{mono, section_label};
use crate::engine::StatusKind;
use crate::settings;
use crate::theme::{metrics, px, Palette};
use crate::updates::{self, ItemAction, Mode, NoteLine, Service};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, FontWeight, ScrollHandle, SharedString, Stateful, Task, Window};
use leon_update::{Snapshot, State};
use std::sync::Arc;

/// What the window keeps about updates.
pub struct UpdateUi {
    /// The state as the window last saw it.
    pub snapshot: Option<Snapshot>,
    /// The scroll of the release notes.
    pub notes_scroll: ScrollHandle,
    /// Follows the updater's changes.
    watcher: Option<Task<()>>,
    /// The first check and the ones after it.
    ticker: Option<Task<()>>,
    /// The check a person asked for, until it answers.
    checking: Option<Task<()>>,
}

impl Default for UpdateUi {
    fn default() -> Self {
        Self {
            snapshot: None,
            notes_scroll: ScrollHandle::new(),
            watcher: None,
            ticker: None,
            checking: None,
        }
    }
}

/// A small button: a mono label in a hairline box.
pub(super) fn pill(
    id: &'static str,
    label: impl Into<SharedString>,
    colours: &Palette,
) -> Stateful<Div> {
    mono(label)
        .id(id)
        .debug_selector(move || id.into())
        .px(px(10.))
        .py(px(4.))
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(colours.elevated_border)
        .text_color(colours.text)
        .cursor_pointer()
}

impl Shell {
    /// The updater, when this window has one.
    fn updates_service(&self) -> Option<Arc<Service>> {
        self.options.updates.clone()
    }

    /// What the settings say about updates.
    pub(super) fn update_mode(cx: &gpui_kit::App) -> Mode {
        Mode::parse(&settings::text(cx, "updates_mode"))
    }

    /// The version of the update that is downloaded and waiting, if any.
    pub(super) fn ready_update_version(&self) -> Option<String> {
        match self.update_state()? {
            State::Ready { offer, .. } | State::RestartRequired(offer) => Some(offer.version),
            _ => None,
        }
    }

    /// The About card's line about updates.
    pub(super) fn about_update_line(&self, cx: &gpui_kit::App) -> Option<String> {
        let service = self.updates_service()?;
        Some(updates::about_line(
            &service.snapshot(),
            Self::update_mode(cx),
            service.install_kind(),
        ))
    }

    fn update_state(&self) -> Option<State> {
        self.updates_service()
            .map(|service| service.snapshot().state)
    }

    /// Follows the updater and, unless a test turned the timer off, asks it a
    /// few seconds after the window is up and every few hours after that.
    pub(super) fn watch_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.updates_service() else {
            return;
        };
        self.updates.snapshot = Some(service.snapshot());
        // What happened without the person (an update that did not start and
        // was undone) is said once, in the status line.
        if let Some(notice) = service.take_notice() {
            self.engine.report(StatusKind::Info, notice);
        }
        let mut changes = service.subscribe();
        self.updates.watcher = Some(cx.spawn(async move |this, cx| {
            while changes.changed().await.is_ok() {
                let snapshot = changes.borrow_and_update().clone();
                let alive = this.update(cx, |this, cx| {
                    this.updates.snapshot = Some(snapshot);
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
        }));
        if !self.options.update_timer || !service.checks_enabled() {
            return;
        }
        self.updates.ticker = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(leon_update::FIRST_CHECK_AFTER)
                .await;
            loop {
                if this.update(cx, |this, cx| this.update_tick(cx)).is_err() {
                    return;
                }
                let entropy = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                cx.background_executor()
                    .timer(leon_update::jittered_interval(entropy))
                    .await;
            }
        }));
    }

    /// One look, if the settings allow: only one is ever in flight (the
    /// updater sees to that), and `off` asks GitHub nothing.
    pub(super) fn update_tick(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.updates_service() else {
            return;
        };
        let mode = Self::update_mode(cx);
        if mode == Mode::Off {
            return;
        }
        service.background(mode, settings::flag(cx, "updates_prereleases"));
    }

    // ----- the commands ------------------------------------------------------------------

    /// Check for updates: asks now and says what it found in the status line.
    pub(super) fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.updates_service() else {
            self.engine.report(
                StatusKind::Info,
                "Updates are not available in this window.",
            );
            return;
        };
        if let leon_update::Install::Manual(
            why @ (leon_update::Why::Development | leon_update::Why::Disabled),
        ) = service.install_kind()
        {
            self.engine.report(StatusKind::Info, why.explain());
            return;
        }
        self.engine
            .report(StatusKind::Busy, "Checking for updates…");
        let mode = Self::update_mode(cx);
        let answer = service.check_now(mode, settings::flag(cx, "updates_prereleases"));
        self.updates.checking = Some(cx.spawn(async move |this, cx| {
            let Ok(snapshot) = answer.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let (kind, text) = updates::check_report(&snapshot, crate::product::VERSION, mode);
                this.engine.report(kind, text);
                this.updates.snapshot = Some(snapshot);
                cx.notify();
            });
        }));
    }

    /// Restart to update. Ready: asks, naming what it closes, then restarts.
    /// Only on offer: starts the download. Not installable here: the page.
    pub(super) fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.update_state() else {
            self.engine.report(
                StatusKind::Info,
                "Updates are not available in this window.",
            );
            return;
        };
        match state {
            State::Ready { .. } | State::RestartRequired(_) => {
                self.begin_flow(crate::keys::Command::RestartToUpdate, window, cx)
            }
            State::Available(offer) => {
                self.engine.report(
                    StatusKind::Info,
                    format!(
                        "Downloading Leon {}; use Restart to update when it is ready.",
                        offer.version
                    ),
                );
                if let Some(service) = self.updates_service() {
                    drop(service.download_now());
                }
            }
            State::Downloading { offer, .. } => self.engine.report(
                StatusKind::Info,
                format!("Leon {} is still downloading.", offer.version),
            ),
            State::Manual { .. } => self.open_download_page(cx),
            _ => self.engine.report(
                StatusKind::Info,
                "No update is ready. Check for updates first.",
            ),
        }
    }

    /// The person confirmed the restart: the update is put in place and tried,
    /// the terminals are hung up and the window closes; `main` starts the new
    /// version when this process has ended.
    pub(super) fn restart_now(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.updates_service() else {
            return;
        };
        match service.install() {
            Ok(applied) => {
                service.request_restart(applied.version);
                self.quit_now_without_update(cx);
            }
            Err(error) => {
                self.engine
                    .report(StatusKind::Error, format!("Could not update: {error}."));
            }
        }
    }

    /// Show release notes: the notes of the version on offer.
    pub(super) fn show_release_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has = self
            .update_state()
            .as_ref()
            .and_then(updates::offer_of)
            .is_some();
        if !has {
            self.engine.report(
                StatusKind::Info,
                "There are no release notes to show: no newer version is known. Check for updates.",
            );
            return;
        }
        self.updates
            .notes_scroll
            .set_offset(gpui_kit::point(px(0.), px(0.)));
        self.overlay = Overlay::Notes;
        self.focus.focus(window, cx);
    }

    /// Skip this version: it is not offered again until a newer one is out.
    pub(super) fn skip_version(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.updates_service() else {
            return;
        };
        let version = self
            .update_state()
            .as_ref()
            .and_then(updates::offer_of)
            .map(|offer| offer.version.clone());
        match version {
            Some(version) => {
                service.skip();
                self.engine.report(
                    StatusKind::Info,
                    format!("Leon {version} is skipped until a newer version is out."),
                );
            }
            None => self
                .engine
                .report(StatusKind::Info, "There is no version on offer to skip."),
        }
        cx.notify();
    }

    /// Open the download page: the release's page, in the browser.
    pub(super) fn open_download_page(&mut self, cx: &mut Context<Self>) {
        let page = match self.updates_service() {
            Some(service) => service.download_page(),
            None => leon_update::release::RELEASES_PAGE.to_owned(),
        };
        (self.options.open_url)(cx, &page);
    }

    /// Quitting: a ready update is put in place on the way out, when the
    /// settings say so and nothing is running. Returns whether it was.
    pub(super) fn install_on_quit(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(service) = self.updates_service() else {
            return false;
        };
        if Self::update_mode(cx) != Mode::Automatic
            || self.busy_sessions(cx) > 0
            || !matches!(service.snapshot().state, State::Ready { .. })
        {
            return false;
        }
        match service.install() {
            Ok(applied) => {
                tracing::info!(version = %applied.version, "the update was installed on the way out");
                true
            }
            Err(error) => {
                tracing::warn!(%error, "the update could not be installed on the way out");
                false
            }
        }
    }

    // ----- drawing -----------------------------------------------------------------------

    /// The footer's item: what an update needs from the person, if anything.
    pub(super) fn render_update_item(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let item = updates::footer_item(&self.update_state()?)?;
        let ready = item.action == ItemAction::Restart;
        let failed = item.action == ItemAction::Check;
        let tip = item.tooltip.clone();
        let action = item.action;
        let hover = colours.surface;
        let mut row = div()
            .id("update-item")
            .debug_selector(|| "update-item".into())
            .flex_none()
            .h_full()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(4.))
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| match action {
                ItemAction::Notes => this.show_release_notes(window, cx),
                ItemAction::Restart => this.restart_to_update(window, cx),
                ItemAction::Check => this.check_for_updates(cx),
            }));
        if let Some(share) = item.progress {
            // A thin bar: the share downloaded, in the accent.
            row = row.child(
                div()
                    .debug_selector(|| "update-progress".into())
                    .flex_none()
                    .w(px(48.))
                    .h(px(3.))
                    .rounded(px(2.))
                    .bg(colours.border)
                    .child(
                        div()
                            .h_full()
                            .w(px(48. * f32::from(share) / 100.))
                            .rounded(px(2.))
                            .bg(colours.signal),
                    ),
            );
        }
        Some(
            row.child(
                div()
                    .debug_selector(|| "update-text".into())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(if failed {
                        colours.error
                    } else if ready {
                        colours.signal
                    } else {
                        colours.text_muted
                    })
                    .when(ready, |this| this.font_weight(FontWeight::SEMIBOLD))
                    .child(item.text),
            ),
        )
    }

    /// The release notes of the version on offer, as plain text.
    pub(super) fn render_notes(&self, colours: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let state = self.update_state();
        let offer = state.as_ref().and_then(updates::offer_of).cloned();
        let (title, lines, page_known) = match &offer {
            Some(offer) => (
                format!(
                    "Leon {}{}",
                    offer.version,
                    if offer.prerelease {
                        " (pre-release)"
                    } else {
                        ""
                    }
                ),
                updates::note_lines(&offer.notes),
                true,
            ),
            None => ("Release notes".to_owned(), Vec::new(), false),
        };
        let mut body = div()
            .id("notes-scroll")
            .debug_selector(|| "notes-scroll".into())
            .track_scroll(&self.updates.notes_scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap(px(4.));
        if lines.is_empty() {
            body = body.child(
                div()
                    .text_color(colours.text_faint)
                    .child("The release has no notes."),
            );
        }
        for line in lines {
            body = body.child(match line {
                NoteLine::Heading(level, text) => div()
                    .pt(px(6.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(if level == 1 {
                        px(16.)
                    } else {
                        metrics::TEXT_BODY()
                    })
                    .child(text),
                NoteLine::Bullet(text) => div()
                    .flex()
                    .gap(px(8.))
                    .child(div().flex_none().text_color(colours.text_faint).child("•"))
                    .child(div().flex_1().min_w_0().child(text)),
                NoteLine::Text(text) => div().child(text),
                NoteLine::Code(text) => div()
                    .font_family(crate::theme::fonts::mono())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_muted)
                    .child(text),
                NoteLine::Blank => div().h(px(6.)),
            });
        }
        let note = match state.as_ref() {
            Some(State::Ready {
                trust: leon_update::Trust::Unsigned,
                ..
            }) => "This build is not signed. It was checked against the release's SHA256SUMS.",
            Some(State::Manual { why, .. }) => why.explain(),
            _ => "",
        };
        let actionable = page_known;
        self.card("release-notes", colours)
            .w(px(560.))
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
                    .gap_3()
                    .child(section_label("Release notes", colours))
                    .child(
                        div()
                            .debug_selector(|| "notes-title".into())
                            .text_color(colours.text_muted)
                            .child(title),
                    ),
            )
            .child(body)
            .when(!note.is_empty(), |this| {
                this.child(
                    div()
                        .flex_none()
                        .px_4()
                        .py(px(6.))
                        .border_t_1()
                        .border_color(colours.border)
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(note),
                )
            })
            .when(actionable, |this| {
                this.child(
                    div()
                        .flex_none()
                        .px_4()
                        .py(px(8.))
                        .border_t_1()
                        .border_color(colours.border)
                        .flex()
                        .gap(px(8.))
                        .child(
                            pill("notes-update", "UPDATE", colours).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.close_overlay(window, cx);
                                    this.restart_to_update(window, cx);
                                },
                            )),
                        )
                        .child(pill("notes-skip", "SKIP THIS VERSION", colours).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.skip_version(cx);
                                this.close_overlay(window, cx);
                            }),
                        ))
                        .child(
                            pill("notes-page", "DOWNLOAD PAGE", colours).on_click(
                                cx.listener(|this, _, _, cx| this.open_download_page(cx)),
                            ),
                        ),
                )
            })
    }
}

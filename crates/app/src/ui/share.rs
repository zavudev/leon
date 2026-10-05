//! "Share this machine": the screen and the in-process service behind it.
//!
//! While the switch is on, this Leon is a host: computers paired with it can
//! run commands and open terminals here, through the relay, end to end
//! encrypted. The screen says so plainly (a paired computer gets a terminal as
//! you), shows the pairing code and its countdown, lists the paired computers
//! with Revoke, and asks before a new one is let in. The service runs inside
//! this process; one that survives the window is the next stage.

use super::shell::{Overlay, Shell};
use super::widgets::{key_cap, led, mono, section_label};
use crate::keys;
use crate::schema;
use crate::settings;
use crate::share::RISK;
use crate::theme::{metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, Keystroke, Stateful, Task, Window};
use leon_host::{ApprovalRequest, RelayState};
use std::time::Duration;

/// What the screen remembers.
#[derive(Default)]
pub struct ShareUi {
    /// A computer waiting for the person's answer.
    pub pending: Option<ApprovalRequest>,
    /// The timer that refreshes the screen while sharing is on.
    pub ticker: Option<Task<()>>,
    /// What the running service was started with, to notice a change.
    pub started_with: Option<(String, String, bool)>,
    /// Whether the sidebar's filter had the keyboard when the screen opened.
    pub restore_filter: bool,
}

/// `mm:ss` of a duration.
pub fn countdown(left: Duration) -> String {
    let secs = left.as_secs();
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// What the relay state means for a person.
pub fn relay_line(state: &RelayState, url: &str) -> (String, bool) {
    match state {
        RelayState::Online => (format!("Online through {url}. The other computer can reach this one."), true),
        RelayState::Connecting => (format!("Connecting to {url}…"), false),
        RelayState::Offline { reason, .. } => (
            format!("Cannot reach the relay at {url} ({reason}). Retrying. The relay service is operated by Zavu and may not be live yet."),
            false,
        ),
        RelayState::Stopped => ("Stopped.".to_owned(), false),
    }
}

impl Shell {
    /// Opens the screen.
    pub(super) fn open_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay == Overlay::Share {
            return;
        }
        let from_filter = self.overlay == Overlay::None && self.filter_focused(window, cx);
        self.close_overlay(window, cx);
        self.share_ui.restore_filter = from_filter;
        self.overlay = Overlay::Share;
        if let Some(remote) = &self.options.remote {
            if remote.share.is_running() && remote.share.pairing().is_none() {
                remote.share.new_code();
            }
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn close_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Overlay::None;
        if self.share_ui.restore_filter {
            self.share_ui.restore_filter = false;
            self.focus_filter(window, cx);
        } else {
            self.focus.focus(window, cx);
            self.sync_focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn share_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = stroke.modifiers;
        if m.alt || m.function || m.platform || m.control {
            return false;
        }
        match stroke.key.as_str() {
            "y" if self.share_ui.pending.is_some() => self.share_answer(true, cx),
            "n" if self.share_ui.pending.is_some() => self.share_answer(false, cx),
            "enter" | "space" => self.share_toggle(window, cx),
            "n" => {
                if let Some(remote) = &self.options.remote {
                    remote.share.new_code();
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn share_answer(&mut self, allow: bool, cx: &mut Context<Self>) {
        if let Some(ask) = self.share_ui.pending.take() {
            let _ = ask.reply.send(allow);
        }
        cx.notify();
    }

    /// Flips the setting that switches sharing.
    pub(super) fn share_toggle(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(def) = schema::find("remote_share") {
            let on = settings::flag(cx, "remote_share");
            settings::set_value(cx, def, schema::Value::Bool(!on));
        }
        self.sync_settings(cx);
        cx.notify();
    }

    /// Starts, stops or restarts the service so it matches the settings. Runs
    /// when a setting changes and when the window starts.
    pub(super) fn sync_share(&mut self, cx: &mut Context<Self>) {
        let Some(remote) = self.options.remote.clone() else {
            return;
        };
        let want = settings::flag(cx, "remote_share");
        let url = settings::text(cx, "remote_relay_url");
        let own = settings::text(cx, "remote_device_name");
        let name = if own.trim().is_empty() {
            leon_host::cli::computer_name()
        } else {
            own
        };
        let ask = settings::flag(cx, "remote_require_approval");
        let config = (url.clone(), name.clone(), ask);
        if remote.share.is_running()
            && (!want || self.share_ui.started_with.as_ref() != Some(&config))
        {
            remote.share.stop();
            self.share_ui.ticker = None;
            self.share_ui.started_with = None;
        }
        if want && !remote.share.is_running() {
            match remote.share.start(&url, &name, ask) {
                Ok(()) => {
                    remote.share.new_code();
                    self.share_ui.started_with = Some(config);
                    self.share_ui.ticker = Some(cx.spawn(async move |this, cx| loop {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        if this.update(cx, |this, cx| this.share_tick(cx)).is_err() {
                            break;
                        }
                    }));
                }
                Err(error) => self.engine.report(crate::engine::StatusKind::Error, &error),
            }
        }
        cx.notify();
    }

    fn share_tick(&mut self, cx: &mut Context<Self>) {
        let Some(remote) = self.options.remote.clone() else {
            return;
        };
        if self.share_ui.pending.is_none() {
            if let Some(ask) = remote.share.poll_approval() {
                self.share_ui.pending = Some(ask);
                // A computer is waiting: bring the question to the front.
                if self.overlay == Overlay::None {
                    self.overlay = Overlay::Share;
                }
            }
        }
        if self.overlay == Overlay::Share {
            cx.notify();
        }
    }

    pub(super) fn render_share(&self, colours: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let muted = colours.text_muted;
        let on = settings::flag(cx, "remote_share");
        let url = settings::text(cx, "remote_relay_url");
        let share = self.options.remote.as_ref().map(|r| r.share.clone());
        let status = share.as_ref().and_then(|s| s.status());
        let pairing = share.as_ref().and_then(|s| s.pairing());
        let devices = share.as_ref().map(|s| s.devices()).unwrap_or_default();

        let mut body = div().flex().flex_col().gap(px(12.));
        body = body
            .child(div().debug_selector(|| "share-intro".into()).child(
                "Let your other computers open terminals and run commands on this one, from behind any router, with nothing to set up. They connect through a relay; everything is end-to-end encrypted.",
            ))
            .child(div().debug_selector(|| "share-risk".into()).text_color(colours.warning).child(RISK))
            .child(
                mono(if on { "SHARING: ON" } else { "SHARING: OFF" })
                    .id("share-switch")
                    .debug_selector(|| "share-switch".into())
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(metrics::RADIUS())
                    .border_1()
                    .border_color(if on { colours.signal } else { colours.elevated_border })
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.share_toggle(window, cx))),
            );
        if let Some(status) = &status {
            let (line, ok) = relay_line(&status.relay, &url);
            body = body.child(
                div()
                    .debug_selector(|| "share-relay".into())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(led(if ok { colours.success } else { colours.warning }))
                    .child(div().text_color(muted).child(line)),
            );
        }
        if let Some(ask) = &self.share_ui.pending {
            body = body.child(
                div()
                    .debug_selector(|| "share-approval".into())
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .border_1()
                    .border_color(colours.signal)
                    .child(div().child(format!(
                        "“{}” wants to pair. Its fingerprint is {}. Allow it only if you started this just now.",
                        ask.request.device_name, ask.request.device_id
                    )))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                mono("ALLOW (Y)")
                                    .id("share-allow")
                                    .debug_selector(|| "share-allow".into())
                                    .px(px(10.))
                                    .py(px(4.))
                                    .border_1()
                                    .border_color(colours.signal)
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.share_answer(true, cx))),
                            )
                            .child(
                                mono("DENY (N)")
                                    .id("share-deny")
                                    .debug_selector(|| "share-deny".into())
                                    .px(px(10.))
                                    .py(px(4.))
                                    .border_1()
                                    .border_color(colours.elevated_border)
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.share_answer(false, cx))),
                            ),
                    ),
            );
        }
        if on {
            body = body.child(match &pairing {
                Some(info) => div()
                    .debug_selector(|| "share-code".into())
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(mono("PAIRING CODE").text_color(muted))
                    .child(
                        div()
                            .font_family(crate::theme::fonts::mono())
                            .text_size(px(34.))
                            .child(info.code.clone()),
                    )
                    .child(div().text_color(muted).child(format!(
                        "Works once. Expires in {}. {} wrong attempts allowed. On the other computer: Connect a machine, With a code.",
                        countdown(info.remaining),
                        info.attempts_left
                    ))),
                None => div()
                    .debug_selector(|| "share-code".into())
                    .text_color(muted)
                    .child("The code was used, expired or burned by wrong attempts. Press N for a new one."),
            });
            body = body.child(
                mono("NEW CODE (N)")
                    .id("share-new-code")
                    .debug_selector(|| "share-new-code".into())
                    .px(px(10.))
                    .py(px(4.))
                    .border_1()
                    .border_color(colours.elevated_border)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(remote) = &this.options.remote {
                            remote.share.new_code();
                        }
                        cx.notify();
                    })),
            );
        }
        let mut list = div()
            .debug_selector(|| "share-devices".into())
            .flex()
            .flex_col()
            .gap(px(4.));
        list = list.child(mono("PAIRED COMPUTERS").text_color(muted));
        if devices.is_empty() {
            list = list.child(div().text_color(muted).child("None yet."));
        }
        for (at, device) in devices.iter().enumerate() {
            let id = device.device_id().full();
            let name = device.name.clone();
            let state = if device.revoked {
                "revoked".to_owned()
            } else {
                format!("last seen {}", ago(device.last_seen_unix))
            };
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(div().child(format!("{name} · {} · {state}", device.device_id())))
                    .when(!device.revoked, |row| {
                        row.child(
                            mono("REVOKE")
                                .id(("share-revoke", at))
                                .px(px(8.))
                                .py(px(2.))
                                .border_1()
                                .border_color(colours.elevated_border)
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(remote) = &this.options.remote {
                                        let _ = remote.share.revoke(&id);
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            );
        }
        body = body.child(list);
        if self.options.remote.is_none() {
            body = body.child(
                div()
                    .text_color(muted)
                    .child("Sharing is not available in this build."),
            );
        }
        self.card("share", colours)
            .w(px(640.))
            .max_w(self.viewport.width - px(32.))
            .max_h(self.viewport.height - px(72.))
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
                    .child(
                        div()
                            .debug_selector(|| "share-title".into())
                            .child(section_label("Share this machine", colours)),
                    )
                    .child(key_cap(keys::key_label("escape"), colours)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .px_4()
                    .py_3()
                    .child(body),
            )
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(colours.border)
                    .child(
                        mono("ENTER SWITCH · N NEW CODE · Y/N ANSWER · ESC CLOSE")
                            .text_color(muted),
                    ),
            )
    }
}

fn ago(unix: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match now.saturating_sub(unix) {
        0..=59 => "just now".into(),
        s @ 60..=3599 => format!("{} min ago", s / 60),
        s @ 3600..=86399 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_countdown_reads_minutes_and_seconds() {
        assert_eq!(countdown(Duration::from_secs(600)), "10:00");
        assert_eq!(countdown(Duration::from_secs(65)), "01:05");
    }

    #[test]
    fn an_unreachable_relay_is_named_with_its_address() {
        let (line, ok) = relay_line(
            &RelayState::Offline {
                reason: "connection refused".into(),
                attempt: 2,
            },
            "wss://relay.getleon.dev",
        );
        assert!(!ok);
        assert!(line.contains("wss://relay.getleon.dev") && line.contains("may not be live yet"));
        assert!(relay_line(&RelayState::Online, "wss://x").1);
    }

    #[test]
    fn the_risk_is_stated_in_plain_words() {
        assert!(RISK.contains("terminal as you"));
    }
}

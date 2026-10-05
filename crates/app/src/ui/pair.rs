//! "Connect a machine, with a code": the default way to add another computer.
//!
//! The person types the code another Leon shows under "Share this machine"
//! (and, optionally, a name); the checklist of `crate::pair` runs on the
//! engine's runtime and reports back here. "SSH (advanced)" switches to the
//! older screen, which is unchanged. The same card also explains a relay
//! machine that is offline ("Why is it offline?"), from the state of its
//! connection.

use super::shell::{Overlay, Shell};
use super::widgets::{key_cap, led, mono, section_label};
use crate::keys;
use crate::pair::{self as model, Failure, Step, StepState};
use crate::settings;
use crate::theme::{metrics, px, Palette};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, Entity, Keystroke, Stateful, Subscription, Task, Window};
use leon_core::{MachineId, MachineKind};
use leon_link::client::{ConnState, Failure as ConnFailure};
use leon_link::PairingCode;

/// What the background pairing reports.
enum Event {
    Step(Step, StepState),
    Done(Result<leon_link::PairedHost, Failure>),
}

/// What the screen remembers while it is open.
pub struct PairUi {
    /// The code field.
    pub code: Entity<InputState>,
    /// The optional name field.
    pub name: Entity<InputState>,
    /// Whether the name field (not the code) has the keyboard.
    pub in_name: bool,
    /// The checklist.
    pub steps: [StepState; 5],
    /// Why it stopped, when it did.
    pub failure: Option<Failure>,
    /// The machine that was saved.
    pub saved: Option<MachineId>,
    /// The pairing under way; dropping it stops listening to it.
    pub running: Option<Task<()>>,
    /// Whether this explains an existing machine instead of adding one.
    pub why: Option<MachineId>,
    /// Whether the sidebar's filter had the keyboard when the screen opened.
    pub restore_filter: bool,
    _subscriptions: Vec<Subscription>,
}

impl PairUi {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("ABCD-EFG-HJK"));
        let name =
            cx.new(|cx| InputState::new(window, cx).placeholder("name of the other computer"));
        let subscriptions = [&code, &name]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(input, window, |_, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                })
            })
            .collect();
        Self {
            code,
            name,
            in_name: false,
            steps: [StepState::Pending; 5],
            failure: None,
            saved: None,
            running: None,
            why: None,
            restore_filter: false,
            _subscriptions: subscriptions,
        }
    }

    /// Whether a pairing is under way.
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }
}

impl Shell {
    /// Opens the screen to add a machine with a code.
    pub(super) fn open_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay == Overlay::Pair && self.pair_ui.why.is_none() {
            return;
        }
        let from_filter = self.overlay == Overlay::None && self.filter_focused(window, cx);
        self.close_overlay(window, cx);
        let ui = &mut self.pair_ui;
        ui.steps = [StepState::Pending; 5];
        ui.failure = None;
        ui.saved = None;
        ui.running = None;
        ui.why = None;
        ui.in_name = false;
        ui.restore_filter = from_filter;
        for input in [ui.code.clone(), ui.name.clone()] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.overlay = Overlay::Pair;
        self.pair_focus(window, cx);
        cx.notify();
    }

    /// Opens the screen to explain why a relay machine is offline.
    pub(super) fn open_pair_why(
        &mut self,
        machine: MachineId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_pair(window, cx);
        let state = self
            .options
            .remote
            .as_ref()
            .and_then(|remote| match &self.snapshot.machine(&machine)?.kind {
                MachineKind::Relay { host_id, .. } => remote.hub.state(host_id),
                _ => None,
            })
            .unwrap_or(ConnState::Connecting);
        let url = match self.snapshot.machine(&machine).map(|m| &m.kind) {
            Some(MachineKind::Relay { relay_url, .. }) => relay_url.clone(),
            _ => String::new(),
        };
        let ui = &mut self.pair_ui;
        ui.why = Some(machine);
        let (steps, failure) = explain(&state, &url);
        ui.steps = steps;
        ui.failure = failure;
        cx.notify();
    }

    /// Gives the keyboard to the field that is current.
    pub(super) fn pair_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = if self.pair_ui.in_name {
            self.pair_ui.name.clone()
        } else {
            self.pair_ui.code.clone()
        };
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    pub(super) fn close_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pair_ui.running = None;
        self.overlay = Overlay::None;
        if self.pair_ui.restore_filter {
            self.pair_ui.restore_filter = false;
            self.focus_filter(window, cx);
        } else {
            self.focus.focus(window, cx);
            self.sync_focus(window, cx);
        }
        cx.notify();
    }

    /// Keys of the screen. Escape is the shell's; here Enter connects and Tab
    /// moves between the two fields.
    pub(super) fn pair_key(
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
            "tab" => {
                self.pair_ui.in_name = !self.pair_ui.in_name;
                self.pair_focus(window, cx);
            }
            "enter" => self.pair_connect(window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Starts pairing with what was typed.
    pub(super) fn pair_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pair_ui.is_running() || self.pair_ui.why.is_some() {
            return;
        }
        let typed = self.pair_ui.code.read(cx).value().to_string();
        let Some(code) = PairingCode::parse(&typed) else {
            self.engine
                .report(crate::engine::StatusKind::Info, &model::code_hint(&typed).0);
            return;
        };
        let Some(remote) = self.options.remote.clone() else {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "Connecting with a code is not available here.",
            );
            return;
        };
        let url = settings::text(cx, "remote_relay_url");
        let own = settings::text(cx, "remote_device_name");
        let device = if own.trim().is_empty() {
            leon_host::cli::computer_name()
        } else {
            own
        };
        let typed_name = self.pair_ui.name.read(cx).value().to_string();
        self.pair_ui.steps = [StepState::Pending; 5];
        self.pair_ui.failure = None;
        self.pair_ui.saved = None;
        let (tx, rx) = flume::unbounded::<Event>();
        let hub = remote.hub.clone();
        let relay = url.clone();
        remote.handle.spawn(async move {
            let progress = tx.clone();
            let result = model::pair(&relay, &hub, &code, &device, move |step, state| {
                let _ = progress.send(Event::Step(step, state));
            })
            .await;
            let _ = tx.send(Event::Done(result));
        });
        self.pair_ui.running = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv_async().await {
                let done = matches!(event, Event::Done(_));
                let url = url.clone();
                let name = typed_name.clone();
                if this
                    .update_in(cx, |this, window, cx| {
                        this.pair_event(event, &url, &name, window, cx)
                    })
                    .is_err()
                    || done
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn pair_event(
        &mut self,
        event: Event,
        url: &str,
        name: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::Step(step, state) => self.pair_ui.steps[step.index()] = state,
            Event::Done(result) => {
                self.pair_ui.running = None;
                let saved = match &result {
                    Ok(paired) => Some(paired),
                    // The pairing itself worked: keep the machine so it connects later.
                    Err(f) if f.step == Step::Channel => None,
                    Err(_) => None,
                };
                if let Err(failure) = &result {
                    self.pair_ui.failure = Some(failure.clone());
                }
                if let Some(paired) = saved {
                    self.save_paired(paired, url, name);
                }
            }
        }
        cx.notify();
    }

    fn save_paired(&mut self, paired: &leon_link::PairedHost, url: &str, name: &str) {
        match self.engine.save_relay_machine(
            name,
            &paired.host_id.to_string(),
            &leon_remote::relay::key_hex(&paired.host_key),
            url,
            &paired.host_name,
        ) {
            Ok(machine) => {
                self.pair_ui.steps[Step::Agents.index()] = StepState::Running;
                self.pair_ui.saved = Some(machine.id.clone());
                self.engine.submit(crate::engine::Op::Probe(machine.id));
            }
            Err(error) => self
                .engine
                .report(crate::engine::StatusKind::Error, error.to_string()),
        }
    }

    /// The agents step, from the engine's probe of the machine just saved.
    fn agents_line(&self) -> (StepState, Option<String>) {
        let Some(id) = &self.pair_ui.saved else {
            return (self.pair_ui.steps[Step::Agents.index()], None);
        };
        match self.engine.machine_state(id) {
            crate::engine::MachineState::Online(Some(report)) => {
                let mut found: Vec<&str> = Vec::new();
                for (label, path) in [
                    ("Claude Code", &report.claude),
                    ("Codex", &report.codex),
                    ("opencode", &report.opencode),
                ] {
                    if path.is_some() {
                        found.push(label);
                    }
                }
                let text = if found.is_empty() {
                    "no coding agent found there yet".to_owned()
                } else {
                    found.join(", ")
                };
                (StepState::Passed, Some(text))
            }
            crate::engine::MachineState::Online(None) => (StepState::Passed, None),
            crate::engine::MachineState::Offline(why) => (StepState::Failed, Some(why)),
            _ => (StepState::Running, None),
        }
    }

    pub(super) fn render_pair(&self, colours: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let ui = &self.pair_ui;
        let typed = ui.code.read(cx).value().to_string();
        let (hint, fine) = model::code_hint(&typed);
        let muted = colours.text_muted;
        let (agents_state, agents_note) = self.agents_line();
        let title = if ui.why.is_some() {
            "Why is it offline?"
        } else {
            "Connect a machine"
        };
        let method = |id: &'static str, label: &'static str, on: bool| {
            mono(label)
                .id(id)
                .debug_selector(move || id.into())
                .px(px(10.))
                .py(px(3.))
                .border_1()
                .border_color(if on {
                    colours.signal
                } else {
                    colours.elevated_border
                })
                .when(on, |this| this.bg(colours.surface_2))
                .text_color(if on { colours.text } else { muted })
                .cursor_pointer()
        };
        let field = |label: &'static str, input: &Entity<InputState>, selector: &'static str| {
            div()
                .debug_selector(move || selector.into())
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(mono(label).text_color(muted))
                .child(
                    div()
                        .px(px(8.))
                        .py(px(4.))
                        .border_1()
                        .border_color(colours.border)
                        .rounded(metrics::RADIUS())
                        .child(Input::new(input).appearance(false)),
                )
        };
        let mut steps = div()
            .debug_selector(|| "pair-steps".into())
            .flex()
            .flex_col()
            .gap(px(4.));
        for step in Step::ALL {
            let state = if step == Step::Agents && ui.why.is_none() {
                agents_state
            } else {
                ui.steps[step.index()]
            };
            let (tag, colour) = match state {
                StepState::Pending => ("WAIT", colours.text_faint),
                StepState::Running => ("…", colours.signal),
                StepState::Passed => ("PASS", colours.success),
                StepState::Failed => ("FAIL", colours.error),
            };
            let note = (step == Step::Agents && ui.why.is_none())
                .then(|| agents_note.clone())
                .flatten();
            if ui.why.is_some() && step == Step::Agents {
                continue;
            }
            steps = steps.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(led(colour))
                    .child(mono(tag).w(px(40.)).text_color(colour))
                    .child(div().child(step.label()))
                    .children(note.map(|n| div().text_color(muted).child(format!("· {n}")))),
            );
        }
        let mut body = div().flex().flex_col().gap(px(12.));
        if ui.why.is_none() {
            body = body
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .debug_selector(|| "pair-intro".into())
                                .child(model::WHAT_IT_IS),
                        )
                        .child(div().text_color(muted).child(model::HOW_IT_IS_SAFE)),
                )
                .child(field("CODE", &ui.code, "pair-code"))
                .child(
                    div()
                        .debug_selector(|| "pair-hint".into())
                        .text_color(if fine { colours.success } else { muted })
                        .child(hint),
                )
                .child(field("NAME (OPTIONAL)", &ui.name, "pair-name"))
                .child(
                    div().flex().gap(px(8.)).child(
                        mono(if ui.is_running() {
                            "CONNECTING…"
                        } else {
                            "CONNECT"
                        })
                        .id("pair-connect")
                        .debug_selector(|| "pair-connect".into())
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(metrics::RADIUS())
                        .border_1()
                        .border_color(if fine {
                            colours.signal
                        } else {
                            colours.elevated_border
                        })
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.pair_connect(window, cx))),
                    ),
                );
        }
        body = body.child(steps);
        if let Some(failure) = &ui.failure {
            body = body.child(
                div()
                    .debug_selector(|| "pair-failure".into())
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_color(colours.error)
                            .child(failure.headline.clone()),
                    )
                    .child(div().text_color(muted).child(failure.advice.clone())),
            );
        }
        if ui.saved.is_some() && ui.failure.is_none() {
            body = body.child(
                div()
                    .debug_selector(|| "pair-saved".into())
                    .child("Connected. The machine is in the sidebar."),
            );
        }
        self.card("pair", colours)
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
                            .debug_selector(|| "pair-title".into())
                            .child(section_label(title, colours)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .when(ui.why.is_none(), |this| {
                                this.child(method("pair-method-code", "WITH A CODE", true))
                                    .child(
                                        method("pair-method-ssh", "SSH (ADVANCED)", false)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_connect(None, false, window, cx)
                                            })),
                                    )
                            })
                            .child(key_cap(keys::key_label("escape"), colours)),
                    ),
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
                    .child(mono("TAB NEXT FIELD · ENTER CONNECT · ESC CLOSE").text_color(muted)),
            )
    }
}

/// The checklist and the failure that a relay connection's state implies.
pub(crate) fn explain(state: &ConnState, url: &str) -> ([StepState; 5], Option<Failure>) {
    use StepState::{Failed, Passed, Pending};
    let say = |step: Step, headline: String, advice: &str| Failure {
        step,
        headline,
        advice: advice.to_owned(),
    };
    match state {
        ConnState::Online => ([Passed, Passed, Passed, Passed, Pending], None),
        ConnState::Offline { failure, reason, .. } => match failure {
            ConnFailure::RelayUnreachable => (
                [Failed, Pending, Pending, Pending, Pending],
                Some(say(
                    Step::Relay,
                    format!("Cannot reach the relay at {url} ({reason})."),
                    "The relay service is operated by Zavu and may not be live yet. This is about the relay, not about your computer.",
                )),
            ),
            ConnFailure::HostOffline => (
                [Passed, Failed, Pending, Pending, Pending],
                Some(say(
                    Step::Find,
                    "The other computer is not connected to the relay.".into(),
                    "It is off, asleep, or Leon is not sharing it. Open Leon there and turn on Share this machine.",
                )),
            ),
            ConnFailure::Revoked => (
                [Passed, Passed, Failed, Pending, Pending],
                Some(say(
                    Step::Verify,
                    "The other computer no longer accepts this one.".into(),
                    "It was revoked there. Pair again with a new code: choose Connect a machine.",
                )),
            ),
            ConnFailure::VersionMismatch | ConnFailure::Other => (
                [Passed, Passed, Passed, Failed, Pending],
                Some(say(Step::Channel, reason.clone(), "Make sure both computers run a recent Leon, then try again.")),
            ),
        },
        ConnState::Connecting | ConnState::Closed => (
            [Passed, Pending, Pending, Pending, Pending],
            Some(say(Step::Find, "Still connecting to the other computer.".into(), "Give it a moment.")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreachable_relay_is_explained_with_its_address_and_without_blaming_the_computer() {
        let state = ConnState::Offline {
            failure: ConnFailure::RelayUnreachable,
            reason: "connection refused".into(),
            attempt: 1,
        };
        let (steps, failure) = explain(&state, "wss://relay.getleon.dev");
        assert_eq!(steps[0], StepState::Failed);
        let failure = failure.unwrap();
        assert!(failure.headline.contains("wss://relay.getleon.dev"));
        assert!(failure.advice.contains("not about your computer"));
    }

    #[test]
    fn a_host_that_is_not_sharing_is_a_find_failure_and_a_revoked_device_a_verify_failure() {
        let off = ConnState::Offline {
            failure: ConnFailure::HostOffline,
            reason: String::new(),
            attempt: 1,
        };
        assert_eq!(explain(&off, "").0[1], StepState::Failed);
        let revoked = ConnState::Offline {
            failure: ConnFailure::Revoked,
            reason: String::new(),
            attempt: 1,
        };
        let (steps, failure) = explain(&revoked, "");
        assert_eq!(steps[2], StepState::Failed);
        assert!(failure.unwrap().advice.contains("Pair again"));
    }

    #[test]
    fn an_online_connection_has_nothing_to_explain() {
        assert!(explain(&ConnState::Online, "").1.is_none());
    }
}

//! What the "Connect a machine" screen draws. Its state and keys are in
//! `connect.rs`; what it says is in `crate::connect`.

use super::connect::{Phase, Stop};
use super::shell::Shell;
use super::widgets::{key_cap, led, mono, section_label};
use crate::connect::{self as model, Field, Platform};
use crate::icons::{icon, IconName};
use crate::keys;
use crate::theme::{metrics, px, Palette};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, SharedString, Stateful};
use leon_remote::connect::{Check, CheckId, CheckState};
use leon_remote::DiagnosisKind;

/// A small button: a mono label in a hairline box, outlined in the accent
/// while the keyboard is on it.
fn pill(
    id: &'static str,
    label: impl Into<SharedString>,
    on: bool,
    colours: &Palette,
) -> Stateful<Div> {
    mono(label)
        .id(id)
        .debug_selector(move || id.into())
        .px(px(10.))
        .py(px(4.))
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(if on {
            colours.signal
        } else {
            colours.elevated_border
        })
        .text_color(colours.text)
        .cursor_pointer()
}

/// A command as it would be typed: mono, in a hairline box.
fn command_box(text: String, selector: &'static str, colours: &Palette) -> Div {
    div()
        .debug_selector(move || selector.into())
        .min_w_0()
        .flex_1()
        .px(px(8.))
        .py(px(5.))
        .border_1()
        .border_color(colours.border)
        .rounded(metrics::RADIUS())
        .bg(colours.background)
        .font_family(crate::theme::fonts::mono())
        .text_size(metrics::TEXT_SMALL())
        .child(text)
}

fn tag(state: &CheckState, colours: &Palette) -> (&'static str, gpui_kit::Hsla) {
    match state {
        CheckState::Pending => ("WAIT", colours.text_faint),
        CheckState::Running => ("…", colours.signal),
        CheckState::Passed => ("PASS", colours.success),
        CheckState::Warned => ("NOTE", colours.warning),
        CheckState::Failed => ("FAIL", colours.error),
        CheckState::Skipped => ("SKIP", colours.text_faint),
    }
}

fn check_name(id: CheckId) -> &'static str {
    match id {
        CheckId::Reach => "reach",
        CheckId::HostKey => "hostkey",
        CheckId::Login => "login",
        CheckId::Shell => "shell",
        CheckId::Git => "git",
        CheckId::Agents => "agents",
    }
}

impl Shell {
    pub(super) fn render_connect(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let ui = &self.connect_ui;
        if ui.follow.get() > 0 {
            // The content grew: show its end. The size is known a frame
            // late, so this runs twice.
            ui.follow.set(ui.follow.get() - 1);
            let mut offset = ui.scroll.offset();
            offset.y = -ui.scroll.max_offset().y;
            ui.scroll.set_offset(offset);
            cx.notify();
        }
        let title = match (ui.phase, ui.editing.is_some()) {
            (Phase::Form, false) => "Connect a machine",
            (Phase::Form, true) => "Edit machine",
            (Phase::Next, _) => "Connected",
            (Phase::Why, _) => "Why is it offline?",
        };
        let body = match ui.phase {
            Phase::Form => self.connect_form(colours, cx),
            Phase::Next => vec![self.connect_next(colours, cx).into_any_element()],
            Phase::Why => vec![self.connect_why(colours, cx).into_any_element()],
        };
        let hints = match ui.phase {
            Phase::Form => format!(
                "TAB MOVE · {} TEST · {} SAVE · {} COPY SSH-COPY-ID",
                keys::CONNECT_TEST.label(),
                keys::CONNECT_SAVE.label(),
                keys::CONNECT_COPY_ID.label()
            ),
            Phase::Next => "TAB MOVE · ENTER CHOOSE · ESC CLOSE".to_owned(),
            Phase::Why => format!(
                "TAB MOVE · {} TEST AGAIN · ESC CLOSE",
                keys::CONNECT_TEST.label()
            ),
        };
        self.card("connect", colours)
            .w(px(780.))
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
                            .debug_selector(|| "connect-title".into())
                            .child(section_label(title, colours)),
                    )
                    .child(key_cap(keys::key_label("escape"), colours)),
            )
            .child(
                div()
                    .id("connect-scroll")
                    .debug_selector(|| "connect-scroll".into())
                    .track_scroll(&ui.scroll)
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .children(body),
            )
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(colours.border)
                    .child(mono(hints).text_color(colours.text_muted)),
            )
    }

    // ----- the form -------------------------------------------------------------------------------

    fn connect_form(&self, colours: &Palette, cx: &mut Context<Self>) -> Vec<gpui_kit::AnyElement> {
        let ui = &self.connect_ui;
        let shown = ui.form.shown_target(&ui.login);
        let howtos = model::howtos(ui.platform, &shown, &ui.keys);
        let muted = colours.text_muted;

        let intro = div()
            .debug_selector(|| "connect-intro".into())
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(div().child(model::WHAT_IT_IS))
            .child(div().text_color(muted).child(model::HOW_IT_REACHES));

        // The segmented choice of the other computer's platform.
        let platforms = div()
            .flex_none()
            .flex()
            .children(Platform::ALL.iter().enumerate().map(|(at, platform)| {
                let platform = *platform;
                let on = ui.platform == platform;
                let selector = format!("connect-platform-{at}");
                let focused = ui.stop == Stop::Platform && on;
                div()
                    .id(("connect-platform", at))
                    .debug_selector(move || selector.clone())
                    .px(px(10.))
                    .py(px(3.))
                    .border_1()
                    .border_color(if focused {
                        colours.signal
                    } else {
                        colours.elevated_border
                    })
                    .when(on, |this| this.bg(colours.surface_2))
                    .cursor_pointer()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(if on { colours.text } else { muted })
                    .child(platform.label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.connect_ui.platform = platform;
                        this.connect_ui.stop = Stop::Platform;
                        this.connect_focus(window, cx);
                        cx.notify();
                    }))
            }));

        let mut rows = div().flex().flex_col();
        for (at, howto) in howtos.iter().enumerate() {
            let open = ui.open_howto == Some(at);
            let focused = ui.stop == Stop::How(at);
            let selector = format!("connect-how-{at}");
            let mut row = div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(colours.border)
                .child(
                    div()
                        .id(("connect-how", at))
                        .debug_selector(move || selector.clone())
                        .h(px(30.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .cursor_pointer()
                        .when(focused, |this| this.bg(colours.surface_2))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(icon(
                                    if open {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    },
                                    px(12.),
                                    muted,
                                ))
                                .child(mono(format!("{}", at + 1)).text_color(muted))
                                .child(div().child(howto.title)),
                        )
                        .child(
                            div()
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(muted)
                                .child(howto.question),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.connect_ui.stop = Stop::How(at);
                            this.connect_ui.open_howto =
                                (this.connect_ui.open_howto != Some(at)).then_some(at);
                            this.connect_focus(window, cx);
                            cx.notify();
                        })),
                );
            if open {
                let mut body = div()
                    .debug_selector(move || format!("connect-how-body-{at}"))
                    .pl(px(28.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(metrics::TEXT_SMALL());
                for line in &howto.lines {
                    body = body.child(div().text_color(muted).child(line.text.clone()));
                    if let Some(command) = &line.command {
                        let copy = command.clone();
                        body = body.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(command_box(command.clone(), "connect-how-command", colours))
                                .child(pill("connect-how-copy", "COPY", false, colours).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.copy_text("command", copy.clone(), cx);
                                    }),
                                )),
                        );
                    }
                }
                row = row.child(body);
            }
            rows = rows.child(row);
        }

        let requirements = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(div().child(model::THREE_THINGS))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(mono("THE OTHER COMPUTER RUNS").text_color(muted))
                            .child(platforms),
                    ),
            )
            .child(rows);

        let fields = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(self.connect_field(Field::Name, colours, cx))
                    .child(self.connect_field(Field::Host, colours, cx)),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(self.connect_field(Field::User, colours, cx))
                    .child(self.connect_field(Field::Port, colours, cx)),
            )
            .child(self.connect_field(Field::Identity, colours, cx))
            .child(self.connect_field(Field::Folder, colours, cx));

        let line = ui.shown_ssh_line();
        let command = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(metrics::TEXT_SMALL())
                    .child("This is the command Leon runs. Try it in a terminal.")
                    .text_color(muted),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(command_box(line, "connect-command", colours))
                    .child(
                        pill("connect-copy-ssh", "COPY", false, colours)
                            .on_click(cx.listener(|this, _, _, cx| this.connect_copy_ssh(cx))),
                    )
                    .child(key_cap(keys::CONNECT_COPY_SSH.label(), colours)),
            );

        let testing = ui.is_testing();
        let save_label = if ui.last_test_failed() {
            "SAVE ANYWAY"
        } else {
            "SAVE"
        };
        let buttons = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                pill(
                    "connect-test",
                    if testing {
                        "CANCEL".to_owned()
                    } else {
                        "TEST CONNECTION".to_owned()
                    },
                    ui.stop == Stop::Test,
                    colours,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.connect_ui.stop = Stop::Test;
                    if this.connect_ui.is_testing() {
                        this.connect_cancel(window, cx);
                    } else {
                        this.connect_test(false, window, cx);
                    }
                })),
            )
            .child(
                pill("connect-save", save_label, ui.stop == Stop::Save, colours).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.connect_ui.stop = Stop::Save;
                        this.connect_save(window, cx);
                    }),
                ),
            )
            .children(ui.notice.clone().map(|text| {
                div()
                    .debug_selector(|| "connect-notice".into())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(muted)
                    .child(text)
            }));

        // Beside a checklist the instructions give way, so that it is in view.
        let helping = ui.checklist.is_none() || ui.show_help;
        let help_toggle = ui.checklist.is_some().then(|| {
            pill(
                "connect-help",
                if ui.show_help {
                    "HIDE INSTRUCTIONS"
                } else {
                    "INSTRUCTIONS"
                },
                false,
                colours,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.connect_ui.show_help = !this.connect_ui.show_help;
                cx.notify();
            }))
        });
        // The children of the card's scroll are these six, always, so that
        // the checklist is the one to scroll to (see `CHECKLIST_AT`).
        let hide = |element: Div, hidden: bool| {
            element
                .when(hidden, |this| this.hidden())
                .into_any_element()
        };
        vec![
            hide(intro, !helping),
            hide(requirements, !helping),
            fields.into_any_element(),
            command.into_any_element(),
            buttons.children(help_toggle).into_any_element(),
            hide(
                div().children(
                    ui.checklist
                        .as_ref()
                        .map(|list| self.connect_checklist(list, colours, cx)),
                ),
                ui.checklist.is_none(),
            ),
        ]
    }

    /// One labelled field with its problem and its suggestions.
    fn connect_field(&self, field: Field, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let ui = &self.connect_ui;
        let focused = ui.stop == Stop::Field(field);
        let problem = ui
            .shown_problems
            .then(|| ui.form.problems().into_iter().find(|p| p.field == field))
            .flatten();
        let border = if problem.is_some() {
            colours.error
        } else if focused {
            colours.signal
        } else {
            colours.border
        };
        let suggestions = if focused {
            ui.suggestions(field)
        } else {
            Vec::new()
        };
        let selector = format!("connect-field-{field:?}").to_lowercase();
        let mut input = div().flex().items_center().gap(px(6.)).child(
            div()
                .debug_selector(move || selector.clone())
                .flex_1()
                .min_w_0()
                .h(metrics::CONTROL())
                .px(px(8.))
                .border_1()
                .border_color(border)
                .rounded(metrics::RADIUS())
                .flex()
                .items_center()
                .child(
                    div()
                        .w_full()
                        .child(Input::new(ui.input(field)).appearance(false)),
                ),
        );
        if field == Field::Identity {
            input = input.child(
                pill("connect-choose-key", "CHOOSE…", false, colours).on_click(
                    cx.listener(|this, _, window, cx| this.connect_pick_identity(window, cx)),
                ),
            );
        }
        let mut column = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(mono(field.label().to_uppercase()).text_color(colours.text_muted))
            .child(input);
        if let Some(problem) = problem {
            let selector = format!("connect-problem-{field:?}").to_lowercase();
            column = column.child(
                div()
                    .debug_selector(move || selector.clone())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.error)
                    .child(problem.message),
            );
        }
        if !suggestions.is_empty() {
            let chips = suggestions.into_iter().enumerate().map(|(at, text)| {
                let on = ui.suggest_at == Some(at);
                let pick = text.clone();
                let selector = format!("connect-suggestion-{at}");
                div()
                    .id(("connect-suggestion", at))
                    .debug_selector(move || selector.clone())
                    .px(px(6.))
                    .py(px(1.))
                    .border_1()
                    .border_color(if on { colours.signal } else { colours.border })
                    .rounded(metrics::RADIUS())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_muted)
                    .cursor_pointer()
                    .child(text)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.connect_set(field, pick.clone(), window, cx);
                        cx.notify();
                    }))
            });
            column = column.child(div().flex().flex_wrap().gap(px(4.)).children(chips));
        }
        column
    }

    // ----- the checklist ----------------------------------------------------------------------------

    fn connect_checklist(
        &self,
        list: &leon_remote::connect::Checklist,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let ui = &self.connect_ui;
        let mut column = div()
            .debug_selector(|| "connect-checklist".into())
            .flex()
            .flex_col()
            .border_1()
            .border_color(colours.border)
            .rounded(metrics::RADIUS());
        for check in &list.checks {
            column = column.child(self.connect_check(check, list, colours, cx));
        }
        if !list.raw.is_empty() {
            let open = ui.details;
            let mut details = div()
                .flex()
                .flex_col()
                .px(px(10.))
                .py(px(6.))
                .border_t_1()
                .border_color(colours.border)
                .child(
                    pill(
                        "connect-details",
                        if open { "HIDE DETAILS" } else { "DETAILS" },
                        ui.stop == Stop::Details,
                        colours,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.connect_ui.details = !this.connect_ui.details;
                        cx.notify();
                    })),
                );
            if open {
                details = details.child(
                    div()
                        .debug_selector(|| "connect-raw".into())
                        .pt(px(6.))
                        .font_family(crate::theme::fonts::mono())
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(list.raw.clone()),
                );
            }
            column = column.child(details);
        }
        column
    }

    fn connect_check(
        &self,
        check: &Check,
        list: &leon_remote::connect::Checklist,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let ui = &self.connect_ui;
        let (word, colour) = tag(&check.state, colours);
        let name = check_name(check.id);
        let mut row = div()
            .debug_selector(move || format!("connect-check-{name}"))
            .px(px(10.))
            .py(px(6.))
            .border_b_1()
            .border_color(colours.border)
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(div().w(px(44.)).child(mono(word).text_color(colour)))
                    .child(div().w(px(170.)).flex_none().child(check.id.label()))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(check.note.clone()),
                    ),
            );
        let Some(diagnosis) = &check.diagnosis else {
            return row;
        };
        let muted = colours.text_muted;
        let mut why = div()
            .debug_selector(|| "connect-diagnosis".into())
            .ml(px(54.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_size(metrics::TEXT_SMALL())
            .child(div().child(diagnosis.explanation.clone()))
            .child(
                div()
                    .debug_selector(|| "connect-fix".into())
                    .flex()
                    .gap(px(6.))
                    .child(mono("FIX").text_color(colours.signal))
                    .child(div().min_w_0().flex_1().child(diagnosis.fix.clone())),
            );
        match diagnosis.kind {
            DiagnosisKind::PublicKeyRefused
            | DiagnosisKind::PasswordOnly
            | DiagnosisKind::InteractiveOnly
            | DiagnosisKind::TooManyAttempts => {
                let line = ui.form.shown_target(&ui.login).copy_id_line();
                let tried = match ui.form.target().identity {
                    Some(path) => format!("Key tried: {path}"),
                    None if ui.keys.is_empty() => {
                        "No key was found in ~/.ssh: create one with ssh-keygen -t ed25519."
                            .to_owned()
                    }
                    None => format!(
                        "Keys offered: {}.",
                        ui.keys
                            .iter()
                            .map(|k| k.public.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                why = why.child(div().text_color(muted).child(tried)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(command_box(line, "connect-copyid", colours))
                        .child(
                            pill("connect-copy-id", "COPY", false, colours)
                                .on_click(cx.listener(|this, _, _, cx| this.connect_copy_id(cx))),
                        )
                        .child(key_cap(keys::CONNECT_COPY_ID.label(), colours)),
                );
            }
            DiagnosisKind::HostKeyUnknown => {
                if let Some(found) = &list.host_keys {
                    for key in &found.keys {
                        why = why.child(
                            div()
                                .debug_selector(|| "connect-fingerprint".into())
                                .flex()
                                .gap(px(8.))
                                .child(mono(key.kind.to_uppercase()).text_color(muted))
                                .child(
                                    div()
                                        .font_family(crate::theme::fonts::mono())
                                        .child(key.fingerprint.clone()),
                                ),
                        );
                    }
                    why = why.child(div().text_color(muted).child(
                        "Why it matters: the host key is how Leon recognises this computer \
                         next time, so nobody can pose as it.",
                    ));
                    if ui.trust {
                        why = why.child(
                            div()
                                .debug_selector(|| "connect-trust-ask".into())
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(div().child(
                                    "Add these keys to ~/.ssh/known_hosts? Only do this if the \
                                     fingerprint matches the one on that computer.",
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(8.))
                                        .child(
                                            pill(
                                                "connect-trust-yes",
                                                "YES, TRUST IT",
                                                ui.stop == Stop::TrustYes,
                                                colours,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.connect_trust_yes(window, cx)
                                                }),
                                            ),
                                        )
                                        .child(
                                            pill(
                                                "connect-trust-no",
                                                "NOT NOW",
                                                ui.stop == Stop::TrustNo,
                                                colours,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.connect_ui.trust = false;
                                                    this.connect_ui.stop = Stop::Trust;
                                                    this.connect_focus(window, cx);
                                                    cx.notify();
                                                }),
                                            ),
                                        ),
                                ),
                        );
                    } else {
                        why = why.child(
                            div().flex().child(
                                pill(
                                    "connect-trust",
                                    "TRUST THIS COMPUTER…",
                                    ui.stop == Stop::Trust,
                                    colours,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.connect_ask_trust(window, cx),
                                )),
                            ),
                        );
                    }
                }
            }
            _ => {}
        }
        row = row.child(why);
        row
    }

    // ----- after saving --------------------------------------------------------------------------------

    fn connect_next(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let ui = &self.connect_ui;
        let muted = colours.text_muted;
        let name = ui
            .saved
            .as_ref()
            .and_then(|id| self.snapshot.machine(id))
            .map_or_else(|| ui.form.name.clone(), |machine| machine.name.clone());
        let state = ui
            .saved
            .as_ref()
            .map(|id| self.engine.machine_state(id))
            .map(|state| match state {
                crate::engine::MachineState::Online(Some(report)) => {
                    let mut text = String::from("It answers.");
                    if let Some(home) = &report.home {
                        text.push_str(&format!(" Home folder {home}."));
                    }
                    text
                }
                crate::engine::MachineState::Offline(why) => format!("It is offline: {why}"),
                _ => "Checking it now.".to_owned(),
            })
            .unwrap_or_default();
        let found = ui.saved.as_ref().and_then(|id| ui.repos.get(id));
        let repos = match (found, ui.searching.is_some()) {
            (_, true) => "Looking for git repositories there…".to_owned(),
            (Some(list), _) if !list.is_empty() => {
                let shown: Vec<&str> = list.iter().take(5).map(String::as_str).collect();
                format!(
                    "Found {} git repositories: {}.",
                    list.len(),
                    shown.join(", ")
                )
            }
            _ => "No git repositories found near its home folder. You can still type a path."
                .to_owned(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .debug_selector(|| "connect-next".into())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(led(colours.success))
                    .child(div().child(format!("{name} is saved and selected in the tree."))),
            )
            .child(div().text_color(muted).child(state))
            .child(
                div()
                    .debug_selector(|| "connect-repos".into())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(muted)
                    .child(repos),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        pill(
                            "connect-add-project",
                            "ADD A PROJECT ON THIS MACHINE",
                            ui.stop == Stop::AddProject,
                            colours,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.connect_add_project(window, cx)),
                        ),
                    )
                    .child(
                        pill(
                            "connect-open-shell",
                            "OPEN A SHELL",
                            ui.stop == Stop::Shell,
                            colours,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.connect_open_shell(window, cx)),
                        ),
                    )
                    .child(
                        pill("connect-done", "DONE", ui.stop == Stop::Done, colours).on_click(
                            cx.listener(|this, _, window, cx| this.close_connect(window, cx)),
                        ),
                    ),
            )
    }

    // ----- why offline -----------------------------------------------------------------------------------

    fn connect_why(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let ui = &self.connect_ui;
        let muted = colours.text_muted;
        let destination = ui.form.target().destination();
        let testing = ui.is_testing();
        let summary = match &ui.checklist {
            Some(list) if list.connected() => "It answers now.".to_owned(),
            Some(list) if list.finished => "It does not answer. This is where it stops:".to_owned(),
            _ => "Testing the connection…".to_owned(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .debug_selector(|| "connect-why".into())
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().child(ui.form.name.clone()))
                    .child(mono(destination).text_color(muted)),
            )
            .child(div().text_color(muted).child(summary))
            .children(
                ui.checklist
                    .as_ref()
                    .map(|list| self.connect_checklist(list, colours, cx)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        pill(
                            "connect-test",
                            if testing { "CANCEL" } else { "TEST AGAIN" },
                            ui.stop == Stop::Test,
                            colours,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.connect_ui.stop = Stop::Test;
                            if this.connect_ui.is_testing() {
                                this.connect_cancel(window, cx);
                            } else {
                                this.connect_test(false, window, cx);
                            }
                        })),
                    )
                    .child(
                        pill(
                            "connect-edit",
                            "EDIT THE MACHINE",
                            ui.stop == Stop::Edit,
                            colours,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.connect_ui.stop = Stop::Edit;
                            this.connect_press(window, cx);
                        })),
                    )
                    .child(
                        pill("connect-done", "CLOSE", ui.stop == Stop::Done, colours).on_click(
                            cx.listener(|this, _, window, cx| this.close_connect(window, cx)),
                        ),
                    ),
            )
            .children(ui.notice.clone().map(|text| {
                div()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(muted)
                    .child(text)
            }))
    }
}

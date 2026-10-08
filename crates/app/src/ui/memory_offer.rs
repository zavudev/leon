//! The question "Turn on agent memory for <project>?": the window's part.
//!
//! The engine asks it: after somebody adds a project on this computer by
//! hand it looks at the project's files off the interface thread, decides
//! (`memory_offer::decide`) and queues an [`Offer`]. The window takes the
//! offer when no other overlay is up and shows it as a small card in the
//! style of the restore question. Nothing here reads a file: which files
//! would be written is in the offer.
//!
//! The answer goes back to the engine, which is the one that writes: turning
//! it on is the palette command's own operation (`Op::ProjectMemory`), "never
//! for this project" is `Op::MemoryNever`. "Do not ask again" turns the
//! setting off. "Not now" (Escape, or a click outside) does nothing: the
//! project is asked about again the next time it is added, in another run.

use super::shell::{Overlay, Shell};
use super::updates_view::pill;
use super::widgets::section_label;
use crate::engine::{Op, StatusKind};
use crate::memory_offer::{self, Choice, Offer, BENEFITS};
use crate::schema::{self, Value};
use crate::settings;
use crate::theme::{metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, Stateful, Window};

impl Shell {
    /// Shows the next question the engine has, when nothing else is up.
    pub(super) fn show_memory_offer(&mut self, cx: &mut Context<Self>) {
        if self.overlay != Overlay::None {
            return;
        }
        // A question that another overlay took the place of is still to be
        // answered; else the engine may have one.
        if self.memory_offer.is_none() {
            let Some(offer) = self.engine.take_memory_offer() else {
                return;
            };
            // The setting may have been turned off since the engine asked.
            if !settings::flag(cx, memory_offer::SETTING_KEY) {
                return;
            }
            self.memory_offer = Some(offer);
        }
        self.overlay = Overlay::MemoryOffer;
        cx.notify();
    }

    /// Closes the question without an answer: "not now".
    pub(super) fn dismiss_memory_offer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.memory_offer = None;
        self.overlay = Overlay::None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Carries out an answer and closes the question.
    pub(super) fn answer_memory_offer(
        &mut self,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(offer) = self.memory_offer.take() else {
            return;
        };
        match choice {
            Choice::TurnOn => self.engine.submit(Op::ProjectMemory {
                project: offer.project,
                on: true,
            }),
            Choice::Never => self.engine.submit(Op::MemoryNever(offer.project)),
            Choice::StopAsking => {
                if let Some(def) = schema::find(memory_offer::SETTING_KEY) {
                    settings::set_value(cx, def, Value::Bool(false));
                }
                self.engine
                    .report(StatusKind::Info, memory_offer::STOPPED_LINE);
            }
            Choice::NotNow => {}
        }
        self.dismiss_memory_offer(window, cx);
        // Another project may be waiting to be asked about.
        self.show_memory_offer(cx);
    }

    /// A key while the question is up. `true` when it was the question's.
    pub(super) fn memory_offer_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &stroke.modifiers;
        if m.platform || m.control || m.alt || m.shift {
            return false;
        }
        let Some(choice) = memory_offer::choice_of(stroke.key.as_str()) else {
            return false;
        };
        self.answer_memory_offer(choice, window, cx);
        true
    }

    pub(super) fn render_memory_offer(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (title, change) = self
            .memory_offer
            .as_ref()
            .map(|offer: &Offer| (offer.title(), offer.change()))
            .unwrap_or_default();
        let mut benefits = div()
            .debug_selector(|| "memory-offer-benefits".into())
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_size(metrics::TEXT_SMALL());
        for line in BENEFITS {
            benefits = benefits.child(div().child(line));
        }
        let mut buttons = div()
            .flex_none()
            .px_4()
            .py(px(8.))
            .border_t_1()
            .border_color(colours.border)
            .flex()
            .flex_wrap()
            .gap(px(8.));
        for (index, choice) in Choice::ALL.into_iter().enumerate() {
            let id = [
                "memory-offer-on",
                "memory-offer-not-now",
                "memory-offer-never",
                "memory-offer-stop",
            ][index];
            buttons = buttons.child(
                pill(
                    id,
                    format!("{}  {}", choice.label(), choice.key_label()),
                    colours,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.answer_memory_offer(choice, window, cx)
                })),
            );
        }
        self.card("memory-offer", colours)
            .w(px(620.))
            .max_w(self.viewport.width - px(32.))
            .max_h(self.viewport.height - px(112.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py(px(10.))
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section_label("Agent memory", colours))
                    .child(
                        div()
                            .debug_selector(|| "memory-offer-title".into())
                            .child(title),
                    ),
            )
            .child(benefits)
            .child(
                div()
                    .debug_selector(|| "memory-offer-change".into())
                    .px_4()
                    .pb_3()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_muted)
                    .child(change),
            )
            .child(buttons)
    }
}

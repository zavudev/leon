//! "Why is a session missing?": the history report in an overlay.
//!
//! The text is built by `crate::history_report` (the very report of `leon
//! --diagnose history`) off the UI thread; this file asks for it and draws
//! it in monospace, line by line.

use super::shell::{Overlay, Shell};
use super::widgets::section_label;
use crate::engine::Op;
use crate::theme::{metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, Stateful, Window};

impl Shell {
    /// Opens the overlay and collects the report.
    pub(super) fn open_history_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.history_scroll
            .set_offset(gpui_kit::point(px(0.), px(0.)));
        self.overlay = Overlay::History;
        self.engine.submit(Op::DiagnoseHistory);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// The report: a heading, then one monospace line per line of the report.
    pub(super) fn render_history_report(
        &self,
        colours: &Palette,
        _cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let lines = self.engine.history_report().unwrap_or_default();
        let mut body = div()
            .id("history-scroll")
            .debug_selector(|| "history-scroll".into())
            .track_scroll(&self.history_scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .px_4()
            .py_3()
            .flex()
            .flex_col();
        if lines.is_empty() {
            body = body.child(
                div()
                    .debug_selector(|| "history-loading".into())
                    .text_color(colours.text_faint)
                    .child("Looking at the history sources..."),
            );
        }
        for line in lines {
            let heading = line.starts_with("== ");
            body = body.child(
                div()
                    .when(heading, |this| this.pt(px(10.)))
                    .min_h(px(16.))
                    .font_family(crate::theme::fonts::mono())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(if heading || line.starts_with("PROBLEM") {
                        colours.text
                    } else {
                        colours.text_muted
                    })
                    .child(line),
            );
        }
        self.card("history-report", colours)
            .w(px(760.))
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
                    .child(section_label("Why is a session missing?", colours))
                    .child(
                        div()
                            .text_color(colours.text_muted)
                            .child("Counts and paths only; no content"),
                    ),
            )
            .child(body)
    }
}

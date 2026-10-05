//! Find in the terminal: the bar over a pane and the keys that drive it.
//!
//! The matches come from `leon_term::find` (the emulator's own search over the
//! scrollback and the screen); this module is the bar and its state. Each pane
//! has its own [`FindBar`]: its query, how it is read, its matches. It survives
//! switching to another tab or pane and goes with the pane when it closes. One
//! text field serves whichever bar is showing and holds that pane's query.
//!
//! * `Find` opens the bar of the pane that has the keyboard (`Cmd+F`,
//!   `Ctrl+Shift+F`) and gives the keyboard to its field; the terminal's size
//!   does not change, the bar floats over the pane's top right corner. The
//!   previous query is back, selected; a selection in the terminal becomes the
//!   query instead.
//! * Typing searches as it goes. The first match is the newest one at or above
//!   the bottom of the screen; `Enter` (or `Find next`) goes up to the
//!   previous output, `Shift+Enter` down, both round the ends. The view
//!   scrolls to the current match.
//! * Output that arrives while the bar is open is searched too, once per
//!   wake-up of the terminal, and the current match stays on its line of text.
//! * `Escape` closes the bar and the highlights and gives the keyboard back to
//!   the terminal, leaving the view where the last match put it; the next key
//!   that reaches the program snaps it back to the bottom as always.
//!
//! While the field has the keyboard, keys go to it and never to the program.

use super::live::LiveId;
use super::shell::{Main, Overlay, Pane, Shell};
use super::widgets::mono;
use crate::theme::{metrics, px, Palette};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, Entity, Focusable as _, Keystroke, Window};
use leon_term::FindState;

/// The most matches the counter prints exactly.
const COUNT_CAP: usize = 999;

/// One pane's find bar.
#[derive(Default)]
pub struct FindBar {
    /// The query, its options and its matches.
    pub state: FindState,
    /// Whether the bar is showing.
    pub open: bool,
    /// How much output the matches were found in.
    generation: u64,
}

/// What the counter says for a search.
pub fn counter(state: &FindState) -> String {
    if state.query().is_empty() {
        return String::new();
    }
    if state.error().is_some() {
        return "Invalid expression".to_owned();
    }
    let total = state.matches().len();
    match state.current() {
        None => "No results".to_owned(),
        Some(at) => {
            let all = if total > COUNT_CAP {
                format!("{COUNT_CAP}+")
            } else {
                total.to_string()
            };
            if at + 1 > COUNT_CAP {
                format!("{COUNT_CAP}+ of {all}")
            } else {
                format!("{} of {all}", at + 1)
            }
        }
    }
}

/// Creates the field of the find bar.
pub fn new_input(window: &mut Window, cx: &mut Context<Shell>) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder("Find"))
}

impl Shell {
    /// Whether the bar of the pane on screen is showing.
    pub(super) fn find_visible(&self) -> bool {
        match self.main {
            Main::Live(id) => self.find.get(&id).is_some_and(|bar| bar.open),
            _ => false,
        }
    }

    /// Whether the bar's field has the keyboard.
    pub(super) fn find_focused(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.find_visible() && self.find_input.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Opens the bar of the pane that has the keyboard.
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, terminal)) = self.screen_terminal(cx) else {
            self.engine
                .report(crate::engine::StatusKind::Info, "Open a terminal first.");
            return;
        };
        if self.overlay != Overlay::None {
            self.close_overlay(window, cx);
        }
        self.pane = Pane::Main;
        let selected = terminal
            .selection_text()
            .filter(|text| !text.contains('\n'));
        let bar = self.find.entry(id).or_default();
        bar.open = true;
        let query = selected.unwrap_or_else(|| bar.state.query().to_owned());
        let options = bar.state.options();
        bar.state.set(&query, options);
        self.find_shown = Some(id);
        self.find_input.update(cx, |field, cx| {
            field.set_value(query.clone(), window, cx);
            field.focus(window, cx);
            field.select_all(window, cx);
        });
        self.run_find(id, true, cx);
        cx.notify();
    }

    /// Closes the bar of the pane on screen and gives the keyboard back to the
    /// terminal.
    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Main::Live(id) = self.main else { return };
        if let Some(bar) = self.find.get_mut(&id) {
            bar.open = false;
        }
        if let Some((_, terminal)) = self.screen_terminal(cx) {
            terminal.set_highlights(None);
        }
        self.sync_focus(window, cx);
        cx.notify();
    }

    /// Searches the pane's buffer again and shows the matches; `reveal` scrolls
    /// to the current one.
    pub(super) fn run_find(&mut self, id: LiveId, reveal: bool, cx: &mut Context<Self>) {
        let Some(terminal) = self
            .live
            .get(id)
            .map(|session| session.view.read(cx).terminal().clone())
        else {
            return;
        };
        let Some(bar) = self.find.get_mut(&id) else {
            return;
        };
        bar.generation = terminal.output_generation();
        let state = &mut bar.state;
        terminal.with_term(|term| {
            state.search(term);
            if reveal {
                state.reveal(term);
            }
        });
        terminal.set_highlights((!state.query().is_empty()).then(|| state.highlights()));
    }

    /// The field's text changed: the query is searched for as it stands.
    pub(super) fn find_changed(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .find_shown
            .filter(|id| self.find.get(id).is_some_and(|b| b.open))
        else {
            return;
        };
        let text = self.find_input.read(cx).value().to_string();
        let Some(bar) = self.find.get_mut(&id) else {
            return;
        };
        if bar.state.query() == text {
            return;
        }
        let options = bar.state.options();
        bar.state.set(&text, options);
        self.run_find(id, true, cx);
        cx.notify();
    }

    /// Flips one of the bar's options.
    pub(super) fn find_toggle(&mut self, which: &str, cx: &mut Context<Self>) {
        let Main::Live(id) = self.main else { return };
        let Some(bar) = self.find.get_mut(&id) else {
            return;
        };
        let mut options = bar.state.options();
        match which {
            "case" => options.case_sensitive = !options.case_sensitive,
            "word" => options.whole_word = !options.whole_word,
            _ => options.regex = !options.regex,
        }
        let query = bar.state.query().to_owned();
        bar.state.set(&query, options);
        self.run_find(id, true, cx);
        cx.notify();
    }

    /// Goes to the next match: up the history when `older`, down otherwise.
    pub(super) fn find_step(&mut self, older: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Main::Live(id) = self.main else { return };
        if !self.find_visible() {
            // `Find next` with no bar open opens it first.
            self.open_find(window, cx);
            return;
        }
        let Some(terminal) = self
            .live
            .get(id)
            .map(|session| session.view.read(cx).terminal().clone())
        else {
            return;
        };
        let Some(bar) = self.find.get_mut(&id) else {
            return;
        };
        let state = &mut bar.state;
        terminal.with_term(|term| {
            if older {
                state.older(term);
            } else {
                state.newer(term);
            }
            state.reveal(term);
        });
        terminal.set_highlights(Some(state.highlights()));
        cx.notify();
    }

    /// The terminal printed: the bars that show are searched again.
    pub(super) fn refresh_find(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(bar) = self.find.get(&id).filter(|bar| bar.open) else {
            return;
        };
        let current = self
            .live
            .get(id)
            .map(|session| session.view.read(cx).terminal().output_generation());
        if current.is_some_and(|generation| generation != bar.generation) {
            self.run_find(id, false, cx);
            cx.notify();
        }
    }

    /// A keystroke while the bar's field has the keyboard: Enter and Shift+Enter
    /// step, Escape closes. `true` when taken.
    pub(super) fn find_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let held = &stroke.modifiers;
        if held.platform || held.control || held.alt {
            return false;
        }
        match stroke.key.as_str() {
            "escape" => self.close_find(window, cx),
            "enter" => self.find_step(!held.shift, window, cx),
            _ => return false,
        }
        true
    }

    /// The bar, for the pane `id`, when it is showing there.
    pub(super) fn render_find_bar(
        &self,
        id: LiveId,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let bar = self.find.get(&id).filter(|bar| bar.open)?;
        let options = bar.state.options();
        let invalid = bar.state.error().is_some();
        let text = counter(&bar.state);
        let none = !bar.state.query().is_empty() && bar.state.current().is_none();
        let toggle = |name: &'static str, label: &'static str, on: bool| {
            div()
                .id(name)
                .debug_selector(move || format!("find-{name}"))
                .flex_none()
                .px(px(5.))
                .py(px(1.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if on {
                    colours.signal
                } else {
                    colours.elevated_border
                })
                .text_color(if on { colours.text } else { colours.text_muted })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.find_toggle(name, cx)))
                .child(mono(label))
        };
        let step = |name: &'static str, glyph: &'static str, older: bool| {
            div()
                .id(name)
                .debug_selector(move || format!("find-{name}"))
                .flex_none()
                .px(px(5.))
                .cursor_pointer()
                .text_color(colours.text_muted)
                .on_click(cx.listener(move |this, _, window, cx| this.find_step(older, window, cx)))
                .child(mono(glyph))
        };
        Some(
            div()
                .debug_selector(|| "find-bar".into())
                .absolute()
                .top(px(8.))
                .right(px(16.))
                .w(px(420.))
                .h(px(34.))
                .px_2()
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if invalid {
                    colours.error
                } else {
                    colours.elevated_border
                })
                .bg(colours.surface)
                .text_color(colours.text)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.find_input).appearance(false)),
                )
                .child(
                    div()
                        .debug_selector(|| "find-count".into())
                        .flex_none()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(if invalid || none {
                            colours.error
                        } else {
                            colours.text_muted
                        })
                        .child(text),
                )
                .children(bar.state.wrapped().then(|| {
                    div()
                        .debug_selector(|| "find-wrapped".into())
                        .flex_none()
                        .child(mono("WRAPPED").text_color(colours.text_faint))
                }))
                .child(toggle("case", "Aa", options.case_sensitive))
                .child(toggle("word", "W", options.whole_word))
                .child(toggle("regex", ".*", options.regex))
                .child(step("older", "\u{2191}", true))
                .child(step("newer", "\u{2193}", false)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_term::FindOptions;

    #[test]
    fn the_counter_is_empty_without_a_query_and_names_the_empty_and_the_invalid() {
        let state = FindState::new();
        assert_eq!(counter(&state), "");
        let mut state = FindState::new();
        state.set("zzz", FindOptions::default());
        // No search ran: a query with no current match reads as no results.
        assert_eq!(counter(&state), "No results");
    }
}

//! The list a conversation's field opens: commands, files, modes, models,
//! efforts, speech models.
//!
//! One list for the six, above the field, the same for every agent:
//!
//! * a slash that opens the prompt lists the commands (Leon's own, then the
//!   agent's), an `@` lists the files of the folder the agent works in, and
//!   what is typed after the sign narrows the list;
//! * the chips under the field (and `/mode`, `/model`, `/effort`) list the
//!   agent's modes, its models and how hard the one in use can be told to
//!   think, with a field of their own to narrow them;
//! * `/voice` (and the button of dictation, until there is a model to
//!   dictate with) lists the speech models offered for the person's
//!   language: choosing one downloads it, and the list shows how far.
//!
//! While it is open the arrows move in it, Enter or Tab choose and Escape
//! closes it; the keyboard stays where the person types. Which rows there
//! are is `crate::chat::complete`'s business; this file keeps the list's
//! state, does what a row says, and draws it.

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{
    div, AnyElement, ClickEvent, Context, FontWeight, ScrollHandle, SharedString, Window,
};

use super::chat::Chat;
use super::chat_view::reason_field;
use super::live::LiveId;
use super::shell::Shell;
use super::widgets::{key_cap, led, mono};
use crate::chat::complete::{self, Pick, Row, Token, Trigger, Voices};
use crate::theme::{fonts, metrics, px, Palette};
use leon_turn::Action;

/// What a list is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    /// Commands, narrowed by what follows the slash in the field.
    Commands,
    /// Files, narrowed by what follows the `@` in the field.
    Files,
    /// The agent's modes, narrowed by the list's own field.
    Modes,
    /// The agent's models, narrowed by the list's own field.
    Models,
    /// The efforts of the model in use, narrowed by the list's own field.
    Efforts,
    /// The speech models of dictation, narrowed by the list's own field.
    Voices,
}

impl MenuKind {
    /// Whether it is narrowed by the list's own field rather than by a word
    /// of the prompt.
    fn has_field(self) -> bool {
        matches!(
            self,
            Self::Modes | Self::Models | Self::Efforts | Self::Voices
        )
    }
}

/// The list open over a conversation's field.
pub struct Menu {
    /// What it lists.
    pub kind: MenuKind,
    /// The row the keyboard is on.
    pub selected: usize,
    /// Keeps that row in view.
    pub scroll: ScrollHandle,
}

impl Menu {
    fn new(kind: MenuKind) -> Self {
        Self {
            kind,
            selected: 0,
            scroll: ScrollHandle::new(),
        }
    }
}

impl Chat {
    /// The word of the prompt that names something, where the cursor is.
    fn token(&self, cx: &gpui_kit::App) -> Option<Token> {
        let input = self.input.read(cx);
        complete::token(&input.value(), input.cursor())
    }

    /// The efforts the next turn can be asked for, the lightest first.
    pub fn efforts(&self) -> &[String] {
        complete::efforts(
            self.options.as_ref(),
            self.model.as_deref(),
            self.conversation.model.as_deref(),
        )
    }

    /// The rows of the open list, best first. `voices` is what the window
    /// knows of the speech models, which are nobody's conversation's.
    pub fn menu_rows(&self, voices: Voices, cx: &gpui_kit::App) -> Vec<Row> {
        let Some(menu) = &self.menu else {
            return Vec::new();
        };
        let typed = || self.token(cx).map(|token| token.query).unwrap_or_default();
        let filter = || self.filter.read(cx).value().trim().to_owned();
        match menu.kind {
            MenuKind::Commands => complete::command_rows(
                &typed(),
                self.options.as_ref(),
                self.efforts(),
                &self.commands,
            ),
            MenuKind::Files => match &self.files {
                Some(files) => complete::file_rows(&typed(), files),
                None => Vec::new(),
            },
            MenuKind::Modes => self.options.as_ref().map_or_else(Vec::new, |options| {
                complete::mode_rows(&filter(), options, self.mode.as_deref())
            }),
            MenuKind::Models => self.options.as_ref().map_or_else(Vec::new, |options| {
                complete::model_rows(&filter(), options, self.model.as_deref())
            }),
            MenuKind::Efforts => complete::effort_rows(
                &filter(),
                self.efforts(),
                self.effort.as_deref(),
                self.options
                    .as_ref()
                    .and_then(|options| options.default_effort.as_deref()),
            ),
            MenuKind::Voices => complete::voice_rows(&filter(), voices),
        }
    }
}

impl Shell {
    /// The text of a conversation's field changed: the list follows the
    /// word being typed, opening for a command or a file and closing when
    /// the word is neither any more.
    pub(super) fn chat_typed(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let wanted = chat.token(cx).map(|token| match token.trigger {
            Trigger::Slash => MenuKind::Commands,
            Trigger::At => MenuKind::Files,
        });
        let open = chat.menu.as_ref().map(|menu| menu.kind);
        match wanted {
            Some(kind) => {
                chat.menu = Some(Menu::new(kind));
                // The folder is listed again each time the list opens: the
                // agent makes files while the conversation goes on.
                if kind == MenuKind::Files && open != Some(kind) {
                    self.chat_load_files(id, cx);
                }
            }
            None if open.is_some_and(|kind| !kind.has_field()) => chat.menu = None,
            None => {}
        }
        cx.notify();
    }

    /// Lists the files of the folder a conversation's agent works in, off
    /// the interface thread. Until it answers the list before it stays.
    fn chat_load_files(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let listing = self
            .engine
            .project_files(chat.machine.clone(), chat.cwd.clone());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(files)) = listing.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(chat) = this.chats.get_mut(&id) {
                    chat.files = Some(Arc::new(files));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Opens the list of a conversation's modes, models or efforts, or closes it
    /// when it is the one open.
    pub(super) fn chat_open_menu(
        &mut self,
        id: LiveId,
        kind: MenuKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if chat.menu.as_ref().is_some_and(|menu| menu.kind == kind) {
            self.chat_close_menu(id, window, cx);
            return;
        }
        chat.menu = Some(Menu::new(kind));
        if kind.has_field() {
            chat.filter.update(cx, |field, cx| {
                field.set_value("", window, cx);
                field.focus(window, cx);
            });
        }
        cx.notify();
    }

    /// Closes a conversation's list; the keyboard is the field's again.
    pub(super) fn chat_close_menu(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        chat.menu = None;
        chat.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Moves to the next or the previous row of a conversation's list,
    /// going round at its ends.
    pub(super) fn chat_menu_move(&mut self, id: LiveId, down: bool, cx: &mut Context<Self>) {
        let Some(count) = self
            .chats
            .get(&id)
            .map(|chat| chat.menu_rows(self.voices(cx), cx).len())
        else {
            return;
        };
        let Some(menu) = self.chats.get_mut(&id).and_then(|chat| chat.menu.as_mut()) else {
            return;
        };
        if count == 0 {
            return;
        }
        menu.selected = match (down, menu.selected.min(count - 1)) {
            (true, at) => (at + 1) % count,
            (false, 0) => count - 1,
            (false, at) => at - 1,
        };
        menu.scroll.scroll_to_item(menu.selected);
        cx.notify();
    }

    /// Does what the row `at` of a conversation's list says.
    pub(super) fn chat_menu_pick(
        &mut self,
        id: LiveId,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let Some(row) = chat.menu_rows(self.voices(cx), cx).into_iter().nth(at) else {
            return;
        };
        let typed = chat.token(cx).map(|token| token.query).unwrap_or_default();
        match row.pick {
            Pick::Models => {
                self.chat_write(id, None, window, cx);
                self.chat_open_menu(id, MenuKind::Models, window, cx);
            }
            Pick::Efforts => {
                self.chat_write(id, None, window, cx);
                self.chat_open_menu(id, MenuKind::Efforts, window, cx);
            }
            Pick::Voices => {
                self.chat_write(id, None, window, cx);
                self.dictation.installed = self.options.voice.installed();
                self.chat_open_menu(id, MenuKind::Voices, window, cx);
            }
            Pick::Voice(model) => self.dictation_choose(Some(id), &model, window, cx),
            Pick::Modes => {
                self.chat_write(id, None, window, cx);
                self.chat_open_menu(id, MenuKind::Modes, window, cx);
            }
            Pick::Command(insert) => {
                // Written out in full already, choosing it is sending it.
                let whole = insert
                    .strip_prefix('/')
                    .is_some_and(|name| name.eq_ignore_ascii_case(&typed));
                self.chat_write(id, Some(&insert), window, cx);
                if whole {
                    self.chat_send(id, window, cx);
                }
            }
            Pick::File(path) => self.chat_write(id, Some(&format!("@{path}")), window, cx),
            Pick::Mode(mode) => {
                self.chat_set_mode(id, mode, cx);
                self.chat_close_menu(id, window, cx);
            }
            Pick::Model(model) => {
                self.chat_set_model(id, model, cx);
                self.chat_close_menu(id, window, cx);
            }
            Pick::Effort(effort) => {
                // No agent takes it while it works: it is the next turn's.
                if let Some(chat) = self.chats.get_mut(&id) {
                    chat.effort = effort;
                }
                self.chat_close_menu(id, window, cx);
            }
        }
    }

    /// Writes `with` in the place of the word being typed in a
    /// conversation's field, or takes the word away, and closes the list.
    fn chat_write(
        &mut self,
        id: LiveId,
        with: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        chat.menu = None;
        let Some(token) = chat.token(cx) else {
            cx.notify();
            return;
        };
        chat.input.update(cx, |input, cx| {
            let (text, cursor) = (input.value().to_string(), input.cursor());
            let (text, cursor) = match with {
                Some(with) => complete::replaced(&text, token.start, cursor, with),
                None => (
                    format!("{}{}", &text[..token.start], &text[cursor..]),
                    token.start,
                ),
            };
            input.set_value(text, window, cx);
            input.set_selected_range(cursor..cursor, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// A conversation works in `mode`: at once where the agent takes it
    /// while it works, with the next turn where it does not.
    pub(super) fn chat_set_mode(&mut self, id: LiveId, mode: String, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        chat.mode = Some(mode.clone());
        let live = chat
            .options
            .as_ref()
            .is_some_and(|options| options.live_changes);
        if let Some(turn) = chat.turn_mut().filter(|_| live) {
            if let Ok(events) = turn.act(Action::SetMode(mode)) {
                self.chat_events(id, events, cx);
                return;
            }
        }
        cx.notify();
    }

    /// A conversation answers with `model`, as [`Self::chat_set_mode`] does
    /// for modes; `None` is the agent's own again, from the next turn.
    pub(super) fn chat_set_model(
        &mut self,
        id: LiveId,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        chat.model = model.clone();
        // An effort the model now in use lacks is not asked of it.
        if chat
            .effort
            .as_ref()
            .is_some_and(|effort| !chat.efforts().contains(effort))
        {
            chat.effort = None;
        }
        let live = chat
            .options
            .as_ref()
            .is_some_and(|options| options.live_changes);
        if let (Some(turn), Some(model)) = (chat.turn_mut().filter(|_| live), model) {
            if let Ok(events) = turn.act(Action::SetModel(model)) {
                self.chat_events(id, events, cx);
                return;
            }
        }
        cx.notify();
    }

    /// A key while a conversation's list is open. `true` when the list took
    /// it.
    pub(super) fn chat_menu_key(
        &mut self,
        id: LiveId,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(chat) = self.chats.get(&id) else {
            return false;
        };
        let Some((kind, selected)) = chat.menu.as_ref().map(|menu| (menu.kind, menu.selected))
        else {
            return false;
        };
        let keys = &stroke.modifiers;
        let plain = !keys.modified();
        let control = keys.control && !keys.alt && !keys.shift && !keys.platform;
        match stroke.key.as_str() {
            "escape" if plain => self.chat_close_menu(id, window, cx),
            "down" if plain => self.chat_menu_move(id, true, cx),
            "up" if plain => self.chat_menu_move(id, false, cx),
            "n" if control => self.chat_menu_move(id, true, cx),
            "p" if control => self.chat_menu_move(id, false, cx),
            "enter" | "tab" if plain => {
                let count = chat.menu_rows(self.voices(cx), cx).len();
                if count == 0 {
                    // Nothing is called that: in the prompt the key is the
                    // field's (what was typed is sent as it is).
                    return kind.has_field();
                }
                self.chat_menu_pick(id, selected.min(count - 1), window, cx);
            }
            _ => return false,
        }
        true
    }

    /// The list open over a conversation's field.
    pub(super) fn chat_menu(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = chat.menu.as_ref()?;
        let rows = chat.menu_rows(self.voices(cx), cx);
        let selected = menu.selected.min(rows.len().saturating_sub(1));
        let (title, nothing) = match menu.kind {
            MenuKind::Commands => ("COMMANDS", "No command is called that."),
            MenuKind::Files if chat.files.is_none() => ("FILES", "Listing the files\u{2026}"),
            MenuKind::Files => ("FILES", "No file is called that."),
            MenuKind::Modes => ("MODE", "No mode is called that."),
            MenuKind::Models => ("MODEL", "No model is called that."),
            MenuKind::Efforts => ("EFFORT", "No effort is called that."),
            MenuKind::Voices => ("DICTATION", "No speech model is called that."),
        };
        let title = match menu.kind {
            MenuKind::Voices => self.voices_title(cx),
            _ => title.to_owned(),
        };
        let head = div()
            .h(px(28.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .border_b_1()
            .border_color(colours.border)
            .child(mono(format!("[ {title} ]")).text_color(colours.text_muted))
            .child(div().flex_1())
            .child(key_cap("\u{2191}\u{2193}", colours))
            .child(key_cap("ENTER", colours))
            .child(key_cap("ESC", colours));
        let mut list = div()
            .id(SharedString::from(format!("chat-menu-list-{}", id.0)))
            .track_scroll(&menu.scroll)
            .overflow_y_scroll()
            .max_h(px(248.))
            .p(px(4.))
            .flex()
            .flex_col();
        for (at, row) in rows.iter().enumerate() {
            list = list.child(self.chat_menu_row(id, at, row, at == selected, colours, cx));
        }
        if rows.is_empty() {
            list = list.child(
                div()
                    .debug_selector(move || format!("chat-menu-nothing-{}", id.0))
                    .h(px(28.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint)
                    .child(nothing),
            );
        }
        Some(
            div()
                .debug_selector(move || format!("chat-menu-{}", id.0))
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.elevated_border)
                .bg(colours.surface)
                .child(head)
                .when(menu.kind.has_field(), |this| {
                    this.child(
                        div()
                            .debug_selector(move || format!("chat-menu-filter-{}", id.0))
                            .px(px(6.))
                            .pt(px(6.))
                            .child(reason_field(&chat.filter, colours)),
                    )
                })
                .child(list)
                .into_any_element(),
        )
    }

    /// One row of the list: what it is called, what it is, and what may
    /// follow it; the one the keyboard is on is marked in the accent.
    fn chat_menu_row(
        &self,
        id: LiveId,
        at: usize,
        row: &Row,
        selected: bool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = format!("chat-menu-row-{}-{at}", id.0);
        let hover = colours.surface_2;
        // A file is its name, then the folder it is in.
        let (label, detail) = match &row.pick {
            Pick::File(path) => match path.rsplit_once('/') {
                Some((folder, file)) => (file.to_owned(), Some(folder.to_owned())),
                None => (path.clone(), None),
            },
            _ => (row.label.clone(), row.detail.clone()),
        };
        let typed = matches!(
            row.pick,
            Pick::Models
                | Pick::Efforts
                | Pick::Modes
                | Pick::Voices
                | Pick::Command(_)
                | Pick::File(_)
        );
        let colour = if row.warn {
            colours.warning
        } else {
            colours.text
        };
        div()
            .id(SharedString::from(name.clone()))
            .debug_selector(move || name.clone())
            .flex_none()
            .h(px(28.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .when(selected, |this| this.bg(colours.surface_2))
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.chat_menu_pick(id, at, window, cx);
            }))
            .child(div().flex_none().w(px(2.)).h(px(14.)).bg(if selected {
                colours.signal
            } else {
                gpui_kit::transparent_black()
            }))
            .child(if typed {
                div()
                    .flex_none()
                    .max_w(gpui_kit::relative(0.6))
                    .truncate()
                    .font_family(fonts::mono())
                    .font_features(fonts::mono_features())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colour)
                    .child(label)
            } else {
                div()
                    .flex_none()
                    .max_w(gpui_kit::relative(0.6))
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colour)
                    .child(label)
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_muted)
                    .children(detail),
            )
            .children(row.note.clone().map(|note| {
                div()
                    .flex_none()
                    .max_w(px(220.))
                    .truncate()
                    .font_family(fonts::mono())
                    .text_size(metrics::TEXT_LABEL())
                    .text_color(colours.text_faint)
                    .child(note)
            }))
            .when(row.current, |this| {
                this.child(led(colours.signal))
                    .child(mono("IN USE").text_color(colours.text_muted))
            })
            .into_any_element()
    }
}

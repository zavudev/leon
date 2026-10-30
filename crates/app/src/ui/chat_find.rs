//! Finding words in a conversation: the bar over its top right corner, the
//! messages it marks and the jump from one match to the next.
//!
//! `Cmd+F` (`Ctrl+F`) while a conversation is on screen opens the bar; Enter
//! and Shift+Enter go to the next and the previous match, Escape closes it.
//! Which messages match and which match is current are `crate::chat::find`'s;
//! the messages that hold a match are tinted, the current one more, and the
//! list scrolls to it.

use std::collections::HashMap;

use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, Context, SharedString, Window};

use super::chat::Chat;
use super::live::LiveId;
use super::shell::{Pane, Shell};
use super::widgets::mono;
use crate::chat::find::{self, Match};
use crate::theme::{metrics, px, Palette};

/// The bar of one conversation, while it is open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Find {
    /// What is looked for.
    pub query: String,
    /// The match shown, by its position among the matches.
    pub current: usize,
}

/// How strongly a message is marked by the search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// It holds a match.
    Some,
    /// It holds the match the bar is on.
    Current,
}

/// The messages that hold a match, and which of them holds the current one.
pub fn marks(found: &[Match], current: usize) -> HashMap<usize, Mark> {
    let mut marks: HashMap<usize, Mark> = HashMap::new();
    for m in found {
        marks.entry(m.item).or_insert(Mark::Some);
    }
    if let Some(m) = found.get(current) {
        marks.insert(m.item, Mark::Current);
    }
    marks
}

/// For each item of a conversation, the index of the child of the list that
/// draws it: every block is a child, and each card of what a turn changed
/// that closes it is another. `blocks` are the ends of the blocks (one past
/// their last item), `cards` the item each card follows. The first child of
/// the list is the space above it.
pub fn child_of_items(blocks: &[(usize, usize)], cards: &[usize]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut child = 1;
    for (from, to) in blocks {
        out.extend(std::iter::repeat_n(child, to - from));
        child += 1 + cards.iter().filter(|at| *at == to).count();
    }
    out
}

impl Chat {
    /// For each item of the conversation, the child of its list that draws
    /// it: what the list scrolls to.
    pub fn item_children(&self) -> Vec<usize> {
        use crate::chat::model::{blocks, Block};
        let blocks: Vec<(usize, usize)> =
            blocks(&self.conversation.items, self.conversation.running())
                .iter()
                .map(|block| match block {
                    Block::Single(at) => (*at, at + 1),
                    Block::Work { from, to, .. } => (*from, *to),
                })
                .collect();
        let cards: Vec<usize> = self.cards().iter().map(|(at, ..)| *at).collect();
        child_of_items(&blocks, &cards)
    }

    /// The matches of what the bar looks for, in the conversation as it is.
    pub fn found(&self) -> Vec<Match> {
        match &self.find {
            Some(bar) => find::matches(&self.conversation.items, &bar.query),
            None => Vec::new(),
        }
    }
}

impl Shell {
    /// Opens the bar of a conversation, with the words that are selected in
    /// its field or the ones it looked for last, and gives it the keyboard.
    pub(super) fn chat_find_open(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay != super::shell::Overlay::None {
            self.close_overlay(window, cx);
        }
        self.pane = Pane::Main;
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let bar = chat.find.get_or_insert_with(Find::default);
        let query = bar.query.clone();
        chat.find_input.update(cx, |field, cx| {
            field.set_value(query, window, cx);
            field.focus(window, cx);
            field.select_all(window, cx);
        });
        cx.notify();
    }

    /// Closes the bar and gives the keyboard back to the field.
    pub(super) fn chat_find_close(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if chat.find.take().is_none() {
            return;
        }
        chat.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// The bar's field changed: what it says is looked for, from the start.
    pub(super) fn chat_find_changed(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let text = chat.find_input.read(cx).value().to_string();
        let Some(bar) = chat.find.as_mut() else {
            return;
        };
        if bar.query == text {
            return;
        }
        bar.query = text;
        bar.current = 0;
        let found = chat.found();
        if let Some(first) = find::first_from(&found, 0) {
            if let Some(bar) = chat.find.as_mut() {
                bar.current = first;
            }
        }
        self.chat_find_reveal(id, cx);
        cx.notify();
    }

    /// Goes to the next match, or the previous one, and shows it.
    pub(super) fn chat_find_step(&mut self, id: LiveId, forward: bool, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let total = chat.found().len();
        let Some(bar) = chat.find.as_mut() else {
            return;
        };
        if let Some(next) = find::step(bar.current, total, forward) {
            bar.current = next;
        }
        self.chat_find_reveal(id, cx);
        cx.notify();
    }

    /// Scrolls the list to the message of the current match.
    fn chat_find_reveal(&mut self, id: LiveId, _cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let found = chat.found();
        let Some(bar) = &chat.find else { return };
        let Some(m) = found.get(bar.current.min(found.len().saturating_sub(1))) else {
            return;
        };
        let children = chat.item_children();
        if let Some(child) = children.get(m.item) {
            chat.scroll.scroll_to_top_of_item(*child);
        }
    }

    /// The bar, over the top right corner of the conversation.
    pub(super) fn chat_find_bar(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let bar = chat.find.as_ref()?;
        let total = chat.found().len();
        let none = total == 0 && !bar.query.trim().is_empty();
        let words = if bar.query.trim().is_empty() {
            String::new()
        } else {
            find::count_text(bar.current, total)
        };
        let button = |name: &'static str, glyph: &'static str| {
            div()
                .id(SharedString::from(format!("{name}-{}", id.0)))
                .debug_selector(move || format!("{name}-{}", id.0))
                .flex_none()
                .px(px(5.))
                .cursor_pointer()
                .text_color(colours.text_muted)
                .child(mono(glyph))
        };
        Some(
            div()
                .debug_selector(move || format!("chat-find-{}", id.0))
                .absolute()
                .top(px(8.))
                .right(px(16.))
                .w(px(340.))
                .h(px(34.))
                .px_2()
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if none {
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
                        .child(super::widgets::text_input(&chat.find_input)),
                )
                .child(
                    div()
                        .debug_selector(move || format!("chat-find-count-{}", id.0))
                        .flex_none()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(if none {
                            colours.error
                        } else {
                            colours.text_muted
                        })
                        .child(words),
                )
                .child(button("chat-find-previous", "\u{2191}").on_click(
                    cx.listener(move |this, _, _, cx| this.chat_find_step(id, false, cx)),
                ))
                .child(
                    button("chat-find-next", "\u{2193}").on_click(
                        cx.listener(move |this, _, _, cx| this.chat_find_step(id, true, cx)),
                    ),
                )
                .child(button("chat-find-close", "\u{d7}").on_click(
                    cx.listener(move |this, _, window, cx| this.chat_find_close(id, window, cx)),
                ))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(item: usize) -> Match {
        Match { item, range: 0..1 }
    }

    #[test]
    fn the_messages_with_a_match_are_marked_and_the_current_one_stands_out() {
        let found = [at(0), at(2), at(2), at(5)];
        let marked = marks(&found, 2);
        assert_eq!(marked.len(), 3);
        assert_eq!(marked[&0], Mark::Some);
        assert_eq!(marked[&2], Mark::Current);
        assert_eq!(marked[&5], Mark::Some);
        assert!(marks(&[], 0).is_empty());
    }

    #[test]
    fn each_item_knows_the_child_of_the_list_that_draws_it() {
        // Items 0 | 1 2 3 | 4, a card after item 3's block, then nothing.
        let blocks = [(0, 1), (1, 4), (4, 5)];
        let cards = [4];
        assert_eq!(child_of_items(&blocks, &cards), [1, 2, 2, 2, 4]);
        assert_eq!(child_of_items(&[], &[]), Vec::<usize>::new());
    }
}

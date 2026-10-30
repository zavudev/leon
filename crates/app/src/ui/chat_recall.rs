//! The prompts the person sent, back in the field: Up at the first line of
//! the field recalls the one before, Down walks forward again and ends on
//! the draft that was being written, as a shell or Claude Code do.
//!
//! The history is the conversation's own: what the person said in it, a
//! restored conversation's messages included, newest first and without the
//! same words twice in a row. The files that went with a message come back
//! with it.
//!
//! The arrows stay the field's own while a list or a question has them, and
//! in the middle of several lines, where they move the caret.

use gpui_kit::{Context, Focusable as _, Window};

use super::chat::Chat;
use super::king::split_attached;
use super::live::LiveId;
use super::shell::Shell;
use crate::chat::model::Item;

/// Browsing the history of a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recall {
    /// How far back the prompt on show is: 0 is the newest.
    at: usize,
    /// The words of the prompt on show: when the field says anything else,
    /// the person edited it and browsing starts over from what they wrote.
    shown: String,
    /// What was being written before browsing began.
    draft: String,
    /// The files that were waiting to be sent then.
    draft_files: Vec<std::path::PathBuf>,
}

/// The prompts of `items`, newest first, a prompt said twice in a row once.
pub fn history(items: &[Item]) -> Vec<String> {
    let mut prompts: Vec<&str> = Vec::new();
    for item in items {
        if let Item::User { text, .. } = item {
            if !text.trim().is_empty() && prompts.last() != Some(&text.as_str()) {
                prompts.push(text);
            }
        }
    }
    prompts.into_iter().rev().map(str::to_owned).collect()
}

impl Chat {
    /// Whether the field has the keyboard (not the reason field, the filter
    /// or the find bar).
    fn field_has_keyboard(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.input.focus_handle(cx).is_focused(window)
    }

    /// Whether another part of the conversation has the arrows: a list over
    /// the field, or a question the agent waits on.
    fn arrows_are_taken(&self) -> bool {
        self.menu.is_some() || !self.conversation.questions.is_empty()
    }

    /// Puts `words` and `files` in the field, the caret at the end.
    fn put_in_field(
        &mut self,
        words: &str,
        files: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        self.attached = files;
        self.input.update(cx, |input, cx| {
            input.set_value(words.to_owned(), window, cx);
            input.set_selected_range(words.len()..words.len(), cx);
        });
    }
}

impl Shell {
    /// Up or Down in the field of conversation `id`: the history, where it
    /// applies. `true` when the key was taken; otherwise the field moves its
    /// caret as it always did.
    pub(super) fn chat_recall_key(
        &mut self,
        id: LiveId,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let older = match stroke.key.as_str() {
            "up" => true,
            "down" => false,
            _ => return false,
        };
        if stroke.modifiers.modified() {
            return false;
        }
        let Some(chat) = self.chats.get_mut(&id) else {
            return false;
        };
        if chat.arrows_are_taken() || !chat.field_has_keyboard(window, cx) {
            return false;
        }
        let (text, caret) = {
            let input = chat.input.read(cx);
            (input.value().to_string(), input.cursor())
        };
        // Several lines: the arrows move the caret until it is on the edge.
        let before = text.get(..caret).unwrap_or(&text);
        let after = text.get(caret..).unwrap_or("");
        let on_edge = if older {
            !before.contains('\n')
        } else {
            !after.contains('\n')
        };
        if !on_edge {
            return false;
        }
        // Browsing goes on only while the field still says what was put in it.
        let browsing = chat.recall.take().filter(|recall| recall.shown == text);
        let prompts = history(&chat.conversation.items);
        let next = match (browsing, older) {
            (None, true) => {
                if prompts.is_empty() {
                    return false;
                }
                Recall {
                    at: 0,
                    shown: String::new(),
                    draft: text,
                    draft_files: chat.attached.clone(),
                }
            }
            // Not browsing: Down is the field's.
            (None, false) => return false,
            (Some(recall), true) => Recall {
                at: (recall.at + 1).min(prompts.len().saturating_sub(1)),
                ..recall
            },
            (Some(recall), false) => match recall.at.checked_sub(1) {
                Some(at) => Recall { at, ..recall },
                None => {
                    // Past the newest: the draft is back.
                    chat.put_in_field(&recall.draft, recall.draft_files, window, cx);
                    cx.notify();
                    return true;
                }
            },
        };
        let Some(prompt) = prompts.get(next.at) else {
            return false;
        };
        let (words, files) = split_attached(prompt);
        chat.put_in_field(&words, files, window, cx);
        chat.recall = Some(Recall {
            shown: words,
            ..next
        });
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Item {
        Item::User {
            text: text.to_owned(),
            at: None,
        }
    }

    #[test]
    fn the_history_is_what_the_person_said_newest_first_without_repeats_in_a_row() {
        let items = [
            said("one"),
            Item::Agent {
                text: "ok".into(),
                streaming: false,
                at: None,
            },
            said("two"),
            said("two"),
            said("  "),
            said("one"),
        ];
        assert_eq!(history(&items), ["one", "two", "one"]);
        assert!(history(&[]).is_empty());
    }
}

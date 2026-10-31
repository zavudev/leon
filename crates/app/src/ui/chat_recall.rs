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

/// Which way the key goes through the history: `Some(true)` for Up (older),
/// `Some(false)` for Down, `None` for a key that is not the history's.
pub(super) fn direction(stroke: &gpui_kit::Keystroke) -> Option<bool> {
    if stroke.modifiers.modified() {
        return None;
    }
    match stroke.key.as_str() {
        "up" => Some(true),
        "down" => Some(false),
        _ => None,
    }
}

/// What Up (`older`) or Down does to a field that says `text` with the caret
/// at `caret`, `attached` waiting to be sent: the words and files to put in
/// it, or `None` when the key is the field's own (the caret is in the middle
/// of several lines, or there is nothing to go back to). `recall` is where
/// the browsing stands, and is moved with it.
pub(super) fn step(
    recall: &mut Option<Recall>,
    text: &str,
    caret: usize,
    older: bool,
    prompts: &[String],
    attached: &[std::path::PathBuf],
) -> Option<(String, Vec<std::path::PathBuf>)> {
    // Several lines: the arrows move the caret until it is on the edge.
    let before = text.get(..caret).unwrap_or(text);
    let after = text.get(caret..).unwrap_or("");
    let on_edge = if older {
        !before.contains('\n')
    } else {
        !after.contains('\n')
    };
    if !on_edge {
        return None;
    }
    // Browsing goes on only while the field still says what was put in it.
    let browsing = recall.take().filter(|recall| recall.shown == text);
    let next = match (browsing, older) {
        (None, true) => {
            if prompts.is_empty() {
                return None;
            }
            Recall {
                at: 0,
                shown: String::new(),
                draft: text.to_owned(),
                draft_files: attached.to_vec(),
            }
        }
        // Not browsing: Down is the field's.
        (None, false) => return None,
        (Some(recall), true) => Recall {
            at: (recall.at + 1).min(prompts.len().saturating_sub(1)),
            ..recall
        },
        (Some(recall), false) => match recall.at.checked_sub(1) {
            Some(at) => Recall { at, ..recall },
            // Past the newest: the draft is back.
            None => return Some((recall.draft, recall.draft_files)),
        },
    };
    let (words, files) = split_attached(prompts.get(next.at)?);
    *recall = Some(Recall {
        shown: words.clone(),
        ..next
    });
    Some((words, files))
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
        let Some(older) = direction(stroke) else {
            return false;
        };
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
        let prompts = history(&chat.conversation.items);
        let Some((words, files)) = step(
            &mut chat.recall,
            &text,
            caret,
            older,
            &prompts,
            &chat.attached,
        ) else {
            return false;
        };
        chat.put_in_field(&words, files, window, cx);
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

    fn prompts() -> Vec<String> {
        vec!["newest".into(), "older".into(), "oldest".into()]
    }

    #[test]
    fn up_walks_back_through_the_prompts_and_down_ends_on_the_draft() {
        let mut recall = None;
        let prompts = prompts();
        let mut shown = "draft".to_owned();
        let go = |older: bool, shown: &mut String, recall: &mut Option<Recall>| {
            let at = shown.len();
            let (words, _) = step(recall, shown, at, older, &prompts, &[])?;
            *shown = words.clone();
            Some(words)
        };
        assert_eq!(go(true, &mut shown, &mut recall).as_deref(), Some("newest"));
        assert_eq!(go(true, &mut shown, &mut recall).as_deref(), Some("older"));
        assert_eq!(go(true, &mut shown, &mut recall).as_deref(), Some("oldest"));
        // The oldest is the end of it.
        assert_eq!(go(true, &mut shown, &mut recall).as_deref(), Some("oldest"));
        assert_eq!(go(false, &mut shown, &mut recall).as_deref(), Some("older"));
        assert_eq!(
            go(false, &mut shown, &mut recall).as_deref(),
            Some("newest")
        );
        assert_eq!(go(false, &mut shown, &mut recall).as_deref(), Some("draft"));
        // Back on the draft, Down is the field's again.
        assert_eq!(go(false, &mut shown, &mut recall), None);
    }

    #[test]
    fn the_arrows_are_the_fields_in_the_middle_of_several_lines_and_with_no_history() {
        let mut recall = None;
        let text = "one\ntwo";
        // The caret on the second line: Up moves it, Down is at the end.
        assert_eq!(step(&mut recall, text, 6, true, &prompts(), &[]), None);
        assert!(step(&mut recall, text, 3, true, &prompts(), &[]).is_some());
        let mut recall = None;
        assert_eq!(step(&mut recall, "", 0, true, &[], &[]), None);
        assert_eq!(step(&mut recall, "", 0, false, &prompts(), &[]), None);
    }

    #[test]
    fn what_was_edited_after_a_recall_starts_the_browsing_over_and_files_come_back() {
        let mut recall = None;
        let prompts = vec![
            "second".to_owned(),
            crate::ui::king::with_attached("first", &[std::path::PathBuf::from("/tmp/a.png")])
                .unwrap(),
        ];
        let draft = [std::path::PathBuf::from("/tmp/draft.png")];
        let (words, files) = step(&mut recall, "mine", 4, true, &prompts, &draft).unwrap();
        assert_eq!((words.as_str(), files.len()), ("second", 0));
        // The person changed what was recalled: Up begins again from there.
        let (words, files) = step(&mut recall, "second!", 7, true, &prompts, &[]).unwrap();
        assert_eq!(words, "second");
        assert!(files.is_empty());
        let (words, files) = step(&mut recall, "second", 6, true, &prompts, &[]).unwrap();
        assert_eq!(words, "first");
        assert_eq!(files, [std::path::PathBuf::from("/tmp/a.png")]);
        // Down twice: the draft of the first browsing is gone with it, the
        // one that was begun on is back.
        let (words, _) = step(&mut recall, "first", 5, false, &prompts, &[]).unwrap();
        assert_eq!(words, "second");
        let (words, files) = step(&mut recall, "second", 6, false, &prompts, &[]).unwrap();
        assert_eq!(words, "second!");
        assert!(files.is_empty());
    }
}

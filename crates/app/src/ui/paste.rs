//! Paste by what the clipboard holds.
//!
//! Agent CLIs (Claude Code, Codex, opencode) read an image from the system
//! clipboard themselves when they receive Ctrl+V (byte 0x16); a text paste only
//! delivers text, so an image pastes nothing. One key therefore does the right
//! thing here, decided by [`plan`] from the clipboard's entries:
//!
//! * an image and no text: send Ctrl+V and let the program fetch the image;
//! * text: paste it, bracketed when the program asked for it;
//! * both: the image where an agent is in front of the shell, the text
//!   otherwise (`Paste as text` and `Paste image` force either);
//! * copied files: their paths as text, shell-quoted, separated by spaces;
//! * nothing usable: nothing is sent.
//!
//! The toolkit reads text, images and file lists from the clipboard on macOS
//! and Windows. On Linux it reads text and files only: no image is ever
//! reported there, so such a paste falls back to text (or "nothing to paste").
//!
//! An agent on another machine cannot read this computer's clipboard, so for a
//! remote terminal an image paste explains that instead of sending Ctrl+V.
//! Files dropped on a pane paste their quoted paths the same way.

use super::shell::Shell;
use crate::engine::StatusKind;
use gpui_kit::{App, ClipboardEntry, ClipboardItem, Context};
use std::path::PathBuf;
use std::rc::Rc;

/// Reads the system clipboard: injected so that tests never touch the real one.
pub type ReadClipboard = Rc<dyn Fn(&App) -> Option<ClipboardItem>>;

/// What a paste does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// By what the clipboard holds.
    Auto,
    /// The text, whatever else there is.
    Text,
    /// An image: Ctrl+V to the program.
    Image,
    /// The image when the clipboard has one, else the text.
    ImageFirst,
}

/// What to do with the clipboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Paste this text (bracketed when the program enabled it).
    Text(String),
    /// Send Ctrl+V.
    CtrlV,
    /// Nothing usable.
    Nothing,
}

/// A path as a shell reads it: plain when it is made of safe characters, else
/// in single quotes.
pub fn quote(path: &str) -> String {
    let plain = !path.is_empty()
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@%~".contains(c));
    if plain {
        path.to_owned()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// Paths as one line: quoted, separated by spaces.
pub fn quoted_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| quote(&path.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What `item` calls for. `agent` is whether an agent CLI is in front of the
/// shell; `image_seen` is whether this platform can tell an image at all.
pub fn plan(item: Option<&ClipboardItem>, how: How, agent: bool) -> Plan {
    let Some(item) = item else {
        return Plan::Nothing;
    };
    let mut text = String::new();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut image = false;
    for entry in item.entries() {
        match entry {
            ClipboardEntry::String(string) => text.push_str(string.text()),
            ClipboardEntry::Image(_) => image = true,
            ClipboardEntry::ExternalPaths(paths) => files.extend(paths.paths().iter().cloned()),
        }
    }
    let text = if text.is_empty() && !files.is_empty() {
        quoted_paths(&files)
    } else {
        text
    };
    match how {
        How::Image if image => Plan::CtrlV,
        How::Image => Plan::Nothing,
        How::ImageFirst if image => Plan::CtrlV,
        How::Text | How::ImageFirst if text.is_empty() => Plan::Nothing,
        How::Text | How::ImageFirst => Plan::Text(text),
        How::Auto => match (image, text.is_empty()) {
            (true, true) => Plan::CtrlV,
            (true, false) if agent => Plan::CtrlV,
            (_, false) => Plan::Text(text),
            (false, true) => Plan::Nothing,
        },
    }
}

impl Shell {
    /// Pastes into the terminal on screen.
    pub(super) fn paste_terminal(&mut self, how: How, cx: &mut Context<Self>) {
        let Some((id, terminal)) = self.screen_terminal(cx) else {
            self.engine
                .report(StatusKind::Info, "Open a terminal first.");
            return;
        };
        let item = (self.options.read_clipboard)(cx);
        let (agent, local) = self.live.get(id).map_or((false, true), |session| {
            (session.shown_agent().is_some(), session.machine.is_local())
        });
        match plan(item.as_ref(), how, agent) {
            Plan::Text(text) => {
                terminal.scroll_to_bottom();
                terminal.paste(&text);
            }
            Plan::CtrlV if !local => self.engine.report(
                StatusKind::Info,
                "Image paste needs the agent to run on this computer; the clipboard is local.",
            ),
            Plan::CtrlV => {
                terminal.scroll_to_bottom();
                terminal.write(vec![0x16]);
                self.engine.report(
                    StatusKind::Info,
                    "Sent Ctrl+V: the program reads the image from the clipboard.",
                );
            }
            Plan::Nothing => self.engine.report(
                StatusKind::Info,
                if how == How::Image {
                    "There is no image on the clipboard."
                } else {
                    "There is nothing to paste."
                },
            ),
        }
        cx.notify();
    }

    /// Files dropped on a pane: their quoted paths are pasted.
    pub(super) fn paste_dropped(
        &mut self,
        id: super::live::LiveId,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.live.get(id) {
            let terminal = session.view.read(cx).terminal().clone();
            terminal.scroll_to_bottom();
            terminal.paste(&quoted_paths(paths));
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Image, ImageFormat};

    fn image() -> ClipboardEntry {
        ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]))
    }

    fn text(text: &str) -> ClipboardEntry {
        ClipboardEntry::String(gpui_kit::ClipboardString::new(text.to_owned()))
    }

    fn item(entries: Vec<ClipboardEntry>) -> ClipboardItem {
        ClipboardItem { entries }
    }

    #[test]
    fn an_image_alone_is_ctrl_v_and_text_alone_is_text() {
        assert_eq!(
            plan(Some(&item(vec![image()])), How::Auto, false),
            Plan::CtrlV
        );
        assert_eq!(
            plan(Some(&item(vec![text("hi")])), How::Auto, true),
            Plan::Text("hi".into())
        );
        assert_eq!(plan(None, How::Auto, true), Plan::Nothing);
        assert_eq!(plan(Some(&item(vec![])), How::Auto, true), Plan::Nothing);
    }

    #[test]
    fn with_both_kinds_the_image_wins_only_in_front_of_an_agent() {
        let both = item(vec![image(), text("file.png")]);
        assert_eq!(plan(Some(&both), How::Auto, true), Plan::CtrlV);
        assert_eq!(
            plan(Some(&both), How::Auto, false),
            Plan::Text("file.png".into())
        );
    }

    #[test]
    fn the_explicit_commands_force_either_kind() {
        let both = item(vec![image(), text("x")]);
        assert_eq!(plan(Some(&both), How::Text, true), Plan::Text("x".into()));
        assert_eq!(plan(Some(&both), How::Image, false), Plan::CtrlV);
        assert_eq!(
            plan(Some(&item(vec![text("x")])), How::Image, true),
            Plan::Nothing
        );
        assert_eq!(
            plan(Some(&item(vec![image()])), How::Text, true),
            Plan::Nothing
        );
    }

    #[test]
    fn copied_files_are_their_quoted_paths_separated_by_spaces() {
        let files = item(vec![ClipboardEntry::ExternalPaths(
            gpui_kit::ExternalPaths(
                [
                    PathBuf::from("/tmp/a.png"),
                    PathBuf::from("/tmp/my shot's.png"),
                ]
                .into_iter()
                .collect(),
            ),
        )]);
        assert_eq!(
            plan(Some(&files), How::Auto, false),
            Plan::Text("/tmp/a.png '/tmp/my shot'\\''s.png'".into())
        );
    }

    #[test]
    fn quoting_leaves_safe_paths_alone_and_protects_the_rest() {
        assert_eq!(quote("/a/b-c_d.png"), "/a/b-c_d.png");
        assert_eq!(quote("/a b"), "'/a b'");
        assert_eq!(quote("$HOME;x"), "'$HOME;x'");
        assert_eq!(quote(""), "''");
    }
}

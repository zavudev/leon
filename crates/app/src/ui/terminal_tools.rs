//! What can be done with a terminal's buffer: clear it, copy it, select it all,
//! save it to a file, and the menu of right-click on a pane.
//!
//! The text itself comes from `leon_term::buffer` (wrapped lines joined, colours
//! as ANSI on request); this module is the part that talks to the user: the
//! status line, the clipboard and the save dialog.
//!
//! * **Clear** works inside the emulator and sends the program nothing, so it
//!   also works while a program runs. A program that owns the whole screen
//!   (the alternate screen) keeps it: nothing is cleared, and the status line
//!   says so.
//! * **Saving** asks the system's save dialog, through [`SaveDialog`] so that
//!   tests answer instead of a person. The suggested name is
//!   `<project>-<worktree>-<agent or shell>-<YYYYMMDD-HHMMSS>.txt` (`.ansi` with
//!   colours), in the user's Downloads folder, or Documents when there is none.
//!   The text is read when the file is chosen and written off the interface's
//!   thread; a failure is a status line. The buffer lives in the emulator on
//!   this computer, so a terminal on a remote machine is saved here.

use super::live::LiveId;
use super::menu::{Item, Menu};
use super::shell::{Overlay, Pane, Picked, Shell};
use super::tree::NodeId;
use crate::engine::StatusKind;
use crate::keys::Command;
use gpui_kit::{App, ClipboardItem, Context, Pixels, Point, Task, Window};
use leon_term::{Cleared, Extent, Terminal};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// Asks where to save a file, suggesting a name: the chosen path, or that the
/// person cancelled.
pub type SaveDialog = Rc<dyn Fn(&mut App, &str) -> Task<Picked>>;

/// The system's save dialog, opened in the Downloads folder, or Documents.
pub fn system_save_dialog(cx: &mut App, suggested: &str) -> Task<Picked> {
    let directory = dirs::download_dir()
        .or_else(dirs::document_dir)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    tracing::info!("opening the system save dialog (prompt_for_new_path)");
    let chosen = cx.prompt_for_new_path(&directory, Some(suggested));
    cx.spawn(async move |_| match chosen.await {
        Ok(Ok(Some(path))) => Picked::Folder(path),
        Ok(Ok(None)) => Picked::Cancelled,
        _ => Picked::Unavailable,
    })
}

/// A number with thousands separators: `1,284`.
pub fn grouped(number: usize) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A name part: lower case letters and digits, dashes between.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// The items of the menu of a terminal pane.
pub fn terminal_items() -> Vec<Item> {
    use Command as C;
    [
        ("Copy", C::Copy),
        ("Paste", C::Paste),
        ("Paste as text", C::PasteText),
        ("Paste image (send Ctrl+V)", C::PasteImage),
        ("Select all", C::SelectAll),
        ("Copy all output", C::CopyAll),
        ("Find…", C::Find),
        ("Clear buffer", C::ClearBuffer),
        ("Save output to file…", C::SaveOutput),
        ("Split right", C::SplitRight),
        ("Split down", C::SplitDown),
        ("Close", C::CloseSession),
    ]
    .into_iter()
    .map(|(label, command)| Item {
        label: label.to_owned(),
        command,
        agent: None,
        children: Vec::new(),
    })
    .collect()
}

impl Shell {
    /// The terminal on screen, with its id.
    pub(super) fn screen_terminal(&self, cx: &App) -> Option<(LiveId, Arc<Terminal>)> {
        match self.main {
            super::shell::Main::Live(id) => self
                .live
                .get(id)
                .map(|session| (id, session.view.read(cx).terminal().clone())),
            _ => None,
        }
    }

    fn say(&self, kind: StatusKind, text: impl Into<String>) {
        self.engine.report(kind, text);
    }

    /// Clears the terminal on screen: the scrollback only, or the scrollback and
    /// the screen but the line the cursor is on.
    pub(super) fn clear_terminal(&mut self, whole: bool, cx: &mut Context<Self>) {
        let Some((_, terminal)) = self.screen_terminal(cx) else {
            self.say(StatusKind::Info, "Open a terminal first.");
            return;
        };
        let cleared = if whole {
            terminal.clear_buffer()
        } else {
            terminal.clear_scrollback()
        };
        match (cleared, whole) {
            (Cleared::FullScreenProgram, _) => self.say(
                StatusKind::Info,
                "A full-screen program is running: nothing was cleared.",
            ),
            (Cleared::Done, true) => self.say(StatusKind::Info, "Cleared the terminal."),
            (Cleared::Done, false) => self.say(StatusKind::Info, "Cleared the scrollback."),
        }
        cx.notify();
    }

    /// Copies the whole buffer, or the screen, as plain text.
    pub(super) fn copy_terminal_text(&mut self, extent: Extent, cx: &mut Context<Self>) {
        let Some((_, terminal)) = self.screen_terminal(cx) else {
            self.say(StatusKind::Info, "Open a terminal first.");
            return;
        };
        let text = terminal.buffer_text(extent);
        let lines = leon_term::buffer::line_count(&text);
        if lines == 0 {
            self.say(StatusKind::Info, "There is nothing to copy.");
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.say(
            StatusKind::Info,
            format!(
                "Copied {} line{}.",
                grouped(lines),
                if lines == 1 { "" } else { "s" }
            ),
        );
    }

    /// Selects everything the terminal holds.
    pub(super) fn select_all_terminal(&mut self, cx: &mut Context<Self>) {
        match self.screen_terminal(cx) {
            Some((_, terminal)) => terminal.select_all(),
            None => self.say(StatusKind::Info, "Open a terminal first."),
        }
        cx.notify();
    }

    /// The file name suggested for the terminal on screen.
    pub(super) fn suggested_name(&self, id: LiveId, ansi: bool) -> String {
        let Some(session) = self.live.get(id) else {
            return "terminal.txt".to_owned();
        };
        let root = super::tree::workspace_root(&self.snapshot, &session.machine, &session.cwd);
        let folder = |path: &str| {
            std::path::Path::new(path)
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
        };
        let (project, worktree) =
            match super::tree::detail_of_root(&self.snapshot, &session.machine, &root) {
                Some((id, _)) => match self.snapshot.project(&id) {
                    Some(entry) => (
                        entry.project.name.clone(),
                        entry
                            .worktrees
                            .iter()
                            .find(|worktree| worktree.path.trim_end_matches('/') == session.cwd)
                            .map_or_else(|| folder(&session.cwd), super::tree::worktree_label),
                    ),
                    None => (folder(&root), folder(&session.cwd)),
                },
                None => (folder(&root), folder(&session.cwd)),
            };
        let who = session
            .agent
            .map_or("shell", |agent| crate::format::agent_name(agent));
        let parts: Vec<String> = [slug(&project), slug(&worktree), slug(who)]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        format!(
            "{}-{}.{}",
            parts.join("-"),
            self.now().format("%Y%m%d-%H%M%S"),
            if ansi { "ansi" } else { "txt" }
        )
    }

    /// Asks where, then writes the whole buffer there: plain text, or with
    /// its colours as ANSI.
    pub(super) fn save_terminal(&mut self, ansi: bool, cx: &mut Context<Self>) {
        let Some((id, terminal)) = self.screen_terminal(cx) else {
            self.say(StatusKind::Info, "Open a terminal first.");
            return;
        };
        let name = self.suggested_name(id, ansi);
        let dialog = (self.options.save_file)(cx, &name);
        let engine = self.engine.clone();
        self.saving = Some(cx.spawn(async move |_, cx| {
            let path = match dialog.await {
                Picked::Folder(path) => path,
                Picked::Cancelled => return,
                Picked::Unavailable => {
                    engine.report(StatusKind::Error, "This system has no save dialog.");
                    return;
                }
            };
            // Read when the file is chosen, so it holds what the terminal
            // holds then.
            let (text, lines) = {
                let text = if ansi {
                    terminal.buffer_ansi(Extent::All)
                } else {
                    terminal.buffer_text(Extent::All)
                };
                let lines = leon_term::buffer::line_count(&text);
                (text + "\n", lines)
            };
            let target = path.clone();
            let written = cx
                .background_executor()
                .spawn(async move { std::fs::write(&target, text) })
                .await;
            match written {
                Ok(()) => engine.report(
                    StatusKind::Info,
                    format!("Saved {} lines to {}.", grouped(lines), path.display()),
                ),
                Err(error) => engine.report(
                    StatusKind::Error,
                    format!("Could not save to {}: {error}.", path.display()),
                ),
            }
        }));
    }

    /// Opens the menu of the terminal pane on screen at `at`.
    pub(super) fn open_terminal_menu(
        &mut self,
        id: LiveId,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay != Overlay::None {
            self.close_overlay(window, cx);
        }
        if self.live.get(id).is_none() {
            return;
        }
        self.open_live(id, window, cx);
        self.pane = Pane::Main;
        let mut menu = Menu::new(NodeId::Live(id), terminal_items(), at);
        menu.terminal = true;
        self.overlay = Overlay::Menu;
        self.menu = Some(menu);
        self.focus.focus(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_grouped_by_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1284), "1,284");
        assert_eq!(grouped(1_000_000), "1,000,000");
    }

    #[test]
    fn name_parts_are_lower_case_words_joined_by_dashes() {
        assert_eq!(slug("My Project!"), "my-project");
        assert_eq!(slug("feature/login"), "feature-login");
        assert_eq!(slug("___"), "");
    }

    #[test]
    fn the_terminal_menu_lists_copy_paste_find_clear_save_and_the_splits() {
        let labels: Vec<String> = terminal_items()
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert_eq!(
            labels,
            [
                "Copy",
                "Paste",
                "Paste as text",
                "Paste image (send Ctrl+V)",
                "Select all",
                "Copy all output",
                "Find…",
                "Clear buffer",
                "Save output to file…",
                "Split right",
                "Split down",
                "Close"
            ]
        );
    }
}

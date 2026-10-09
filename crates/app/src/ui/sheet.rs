//! The sheet that lists every shortcut, generated from the registry.

use super::shell::Shell;
use super::widgets::{key_cap, section_label};
use crate::keys::{self, Command, Section, BINDINGS};
use crate::theme::{metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{div, App, Div, Keystroke, Stateful, Window};

/// One line of the sheet: what it does and the keys that do it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetRow {
    /// What the command is called.
    pub label: String,
    /// Its keys, as this platform writes them.
    pub keys: Vec<String>,
}

/// The sheet's content: every section with its rows, in the registry's order.
/// The nine "switch to machine" bindings are one row.
pub fn sheet_rows(mac: bool) -> Vec<(Section, Vec<SheetRow>)> {
    Section::ALL
        .iter()
        .map(|section| {
            let rows = BINDINGS
                .iter()
                .filter(|binding| binding.section == *section)
                .filter_map(|binding| match binding.command {
                    Command::Machine(1) => Some(SheetRow {
                        label: "Jump to machine 1 to 9".to_owned(),
                        keys: vec![format!("{}…9", binding.chords.first()?.label_for(mac))],
                    }),
                    Command::Machine(_) => None,
                    _ => Some(SheetRow {
                        label: binding.label.to_owned(),
                        keys: keys::chords_on(binding, mac)
                            .iter()
                            .map(|chord| chord.label_for(mac))
                            .collect(),
                    }),
                })
                .collect();
            (*section, rows)
        })
        .collect()
}

/// What stays Leon's while a terminal has the keyboard: each command with
/// only the chords that are still its, and nothing else. Everything not
/// listed goes to the program.
pub fn terminal_rows(mac: bool) -> Vec<SheetRow> {
    let kept = keys::terminal_chords(mac);
    BINDINGS
        .iter()
        .filter_map(|binding| {
            let chords: Vec<String> = kept
                .iter()
                .filter(|(command, _)| *command == binding.command)
                .map(|(_, chord)| chord.label_for(mac))
                .collect();
            (!chords.is_empty()).then(|| SheetRow {
                label: binding.label.to_owned(),
                keys: chords,
            })
        })
        .collect()
}

/// The keys of the Settings screen, which are its own and not the registry's.
pub const SETTINGS_KEYS: &[(&str, &str)] = &[
    ("Next area: sections, search, options", "Tab"),
    ("Previous area", "Shift+Tab"),
    ("Move between options", "J K ↑ ↓"),
    ("Change the option: flip, ask, type or run", "Enter Space"),
    ("Step a choice, a number or a switch", "← →"),
    ("Put the option back to its default", "R"),
    ("Search every setting", "/"),
    ("Cancel an edit, clear the search, close", "Esc"),
];

/// The keys of the Connect screen, which are its own and not the registry's.
pub fn connect_keys(mac: bool) -> Vec<(&'static str, String)> {
    vec![
        ("Next field or button", "Tab".to_owned()),
        ("Previous", "Shift+Tab".to_owned()),
        ("Take the highlighted suggestion", "↑ ↓ Enter".to_owned()),
        ("Test the connection", keys::CONNECT_TEST.label_for(mac)),
        ("Save the machine", keys::CONNECT_SAVE.label_for(mac)),
        (
            "Copy the ssh-copy-id line",
            keys::CONNECT_COPY_ID.label_for(mac),
        ),
        ("Copy the ssh line", keys::CONNECT_COPY_SSH.label_for(mac)),
        ("Cancel a test, back out, close", "Esc".to_owned()),
    ]
}

/// Whether a line answers the sheet's search: every word of the query is in
/// its label or among its keys, ignoring case. An empty query answers every
/// line.
pub fn answers(query: &str, label: &str, keys: &[String]) -> bool {
    let haystack = format!("{label} {}", keys.join(" ")).to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

/// One line of a block that has keys only: its label, and its keys on the
/// right.
fn key_line(row: SheetRow, colours: &Palette) -> Div {
    div()
        .h(px(28.))
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(div().min_w_0().truncate().child(row.label))
        .child(
            div()
                .flex_none()
                .flex()
                .gap(px(4.))
                .children(row.keys.into_iter().map(|text| key_cap(text, colours))),
        )
}

/// A block of the sheet that has keys only, under its title and an optional
/// note. `None` when the search leaves it no line.
fn key_block(
    title: &str,
    note: Option<String>,
    selector: &'static str,
    rows: Vec<SheetRow>,
    colours: &Palette,
) -> Option<Div> {
    if rows.is_empty() {
        return None;
    }
    let mut block = div()
        .flex()
        .flex_col()
        .child(div().pb(px(4.)).child(section_label(title, colours)));
    if let Some(note) = note {
        block = block.child(
            div()
                .pb(px(6.))
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text_faint)
                .child(note),
        );
    }
    for row in rows {
        block = block.child(key_line(row, colours));
    }
    Some(block.debug_selector(|| selector.into()))
}

impl Shell {
    /// The search field of the sheet changed: the lines it answers are shown
    /// from the top.
    pub(super) fn sheet_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.sheet_search.read(cx).value().to_string();
        if text == self.sheet_query {
            return;
        }
        self.sheet_query = text;
        self.sheet_scroll
            .set_offset(gpui_kit::point(px(0.), px(0.)));
        cx.notify();
    }

    /// The keys of the sheet while its search has the keyboard: the arrows,
    /// the page keys and home and end move the sheet, and Escape clears a
    /// search before it closes the sheet. `true` when taken.
    pub(super) fn sheet_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &stroke.modifiers;
        if m.control || m.alt || m.platform || m.function || m.shift {
            return false;
        }
        let command = match stroke.key.as_str() {
            "down" => Command::Down,
            "up" => Command::Up,
            "pagedown" => Command::PageDown,
            "pageup" => Command::PageUp,
            "home" => Command::Top,
            "end" => Command::Bottom,
            "escape" if !self.sheet_query.is_empty() => {
                self.sheet_query.clear();
                self.sheet_search
                    .update(cx, |field, cx| field.set_value("", window, cx));
                self.sheet_scroll
                    .set_offset(gpui_kit::point(px(0.), px(0.)));
                cx.notify();
                return true;
            }
            _ => return false,
        };
        self.move_cursor(command, window, cx);
        true
    }

    pub(super) fn render_sheet(&self, colours: &Palette) -> Stateful<Div> {
        let query = self.sheet_query.as_str();
        let mac = crate::platform::is_mac();
        let mut body = div()
            .id("sheet-scroll")
            .debug_selector(|| "sheet-scroll".into())
            .track_scroll(&self.sheet_scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_4();
        let mut shown = 0;
        for (section, rows) in sheet_rows(mac) {
            let rows: Vec<SheetRow> = rows
                .into_iter()
                .filter(|row| answers(query, &row.label, &row.keys))
                .collect();
            if rows.is_empty() {
                continue;
            }
            shown += rows.len();
            let mut block = div().flex().flex_col().child(
                div()
                    .pb(px(4.))
                    .child(section_label(section.title(), colours)),
            );
            for row in rows {
                let none = row.keys.is_empty();
                block = block.child(
                    div()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(div().min_w_0().truncate().child(row.label))
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .gap(px(4.))
                                .children(row.keys.into_iter().map(|text| key_cap(text, colours)))
                                .when(none, |this| {
                                    this.text_size(metrics::TEXT_SMALL())
                                        .text_color(colours.text_faint)
                                        .child("palette only")
                                }),
                        ),
                );
            }
            body = body.child(block);
        }
        // The chords that stay Leon's inside a terminal.
        let inside: Vec<SheetRow> = terminal_rows(mac)
            .into_iter()
            .filter(|row| answers(query, &row.label, &row.keys))
            .collect();
        let inside_note = if mac {
            "Only these Cmd chords are Leon's. Every other key, Ctrl chords included, goes to the program."
        } else {
            "Only these Ctrl+Shift chords are Leon's. Every other key, plain Ctrl chords included, goes to the program."
        };
        // The keys of the Settings screen.
        let settings: Vec<SheetRow> = SETTINGS_KEYS
            .iter()
            .map(|(label, keys)| SheetRow {
                label: (*label).to_owned(),
                keys: vec![(*keys).to_owned()],
            })
            .filter(|row| answers(query, &row.label, &row.keys))
            .collect();
        // The keys of the Connect screen.
        let connect: Vec<SheetRow> = connect_keys(mac)
            .into_iter()
            .map(|(label, keys)| SheetRow {
                label: label.to_owned(),
                keys: vec![keys],
            })
            .filter(|row| answers(query, &row.label, &row.keys))
            .collect();
        let blocks = [
            key_block(
                "While a terminal has the keyboard",
                Some(inside_note.to_owned()),
                "sheet-terminal",
                inside,
                colours,
            ),
            key_block(
                "In the Settings screen",
                None,
                "sheet-settings",
                settings,
                colours,
            ),
            key_block(
                "In the Connect screen",
                None,
                "sheet-connect",
                connect,
                colours,
            ),
        ];
        for block in blocks.into_iter().flatten() {
            shown += 1;
            body = body.child(block);
        }
        if shown == 0 {
            body = body.child(
                div()
                    .debug_selector(|| "sheet-empty".into())
                    .text_color(colours.text_faint)
                    .child(format!(
                        "No shortcut matches \u{201c}{}\u{201d}.",
                        query.trim()
                    )),
            );
        }
        self.card("shortcuts-sheet", colours)
            .w(px(560.))
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
                    .justify_between()
                    .child(section_label("Keyboard shortcuts", colours))
                    .child(key_cap(keys::key_label("escape"), colours)),
            )
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(colours.border)
                    .debug_selector(|| "sheet-search".into())
                    .child(super::widgets::text_input(&self.sheet_search)),
            )
            .child(body)
    }
}

impl Shell {
    /// The About panel: the mark, the name, the version and who made it.
    pub(super) fn render_about(&self, colours: &Palette, cx: &App) -> Stateful<Div> {
        self.card("about", colours)
            .w(px(360.))
            .p(px(24.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .child(super::widgets::mark(
                px(48.),
                colours,
                cx,
                super::widgets::Lion::new("about-mark").mood(self.mark_mood()),
            ))
            .child(
                div()
                    .debug_selector(|| "about-name".into())
                    .text_size(px(20.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(crate::product::PRODUCT_NAME),
            )
            .child(
                super::widgets::mono(format!("VERSION {}", crate::product::VERSION))
                    .debug_selector(|| "about-version".into())
                    .text_color(colours.text_muted),
            )
            .child(
                super::widgets::mono(format!("BY {}", crate::product::MAKER.to_uppercase()))
                    .text_color(colours.text_muted),
            )
            .child(
                super::widgets::mono(format!("LICENCE {}", env!("CARGO_PKG_LICENSE")))
                    .text_color(colours.text_faint),
            )
            .children(self.about_update_line(cx).map(|line| {
                super::widgets::mono(line)
                    .debug_selector(|| "about-update".into())
                    .text_color(colours.text_muted)
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_answers_the_lines_by_label_and_by_keys_ignoring_case() {
        let keys = ["Ctrl+P".to_owned(), "Ctrl+K".to_owned()];
        assert!(answers("", "Command palette", &[]));
        assert!(answers("palette", "Command palette", &[]));
        assert!(answers("PALETTE", "Command palette", &[]));
        assert!(answers("ctrl+p", "Go to a project", &keys));
        assert!(answers("go project", "Go to a project", &keys));
        assert!(!answers("palette", "Go to a project", &keys));
        assert!(!answers("zzzz", "Command palette", &[]));
    }

    #[test]
    fn the_search_shows_only_the_sections_it_answers() {
        let rows = sheet_rows(false);
        let found: Vec<&SheetRow> = rows
            .iter()
            .flat_map(|(_, rows)| rows)
            .filter(|row| answers("sidebar", &row.label, &row.keys))
            .collect();
        assert!(found
            .iter()
            .any(|row| row.label == "Make the sidebar wider"));
        assert!(found
            .iter()
            .all(|row| row.label.to_lowercase().contains("sidebar")
                || row
                    .keys
                    .iter()
                    .any(|key| key.to_lowercase().contains("sidebar"))));
        assert!(
            !found.is_empty() && found.len() < rows.iter().map(|(_, r)| r.len()).sum::<usize>()
        );
    }

    #[test]
    fn the_sheet_lists_every_binding_of_the_registry_once() {
        let rows = sheet_rows(false);
        let labels: Vec<&str> = rows
            .iter()
            .flat_map(|(_, rows)| rows.iter().map(|row| row.label.as_str()))
            .collect();
        for binding in BINDINGS {
            match binding.command {
                Command::Machine(1) => assert!(labels.contains(&"Jump to machine 1 to 9")),
                Command::Machine(_) => {}
                _ => assert_eq!(
                    labels
                        .iter()
                        .filter(|label| **label == binding.label)
                        .count(),
                    1,
                    "{}",
                    binding.label
                ),
            }
        }
    }

    #[test]
    fn the_connect_screens_keys_are_listed_for_the_platform() {
        let mac = connect_keys(true);
        assert!(mac
            .iter()
            .any(|(l, k)| *l == "Test the connection" && k == "⌘↩"));
        let other = connect_keys(false);
        assert!(other
            .iter()
            .any(|(l, k)| *l == "Save the machine" && k == "Ctrl+S"));
    }

    #[test]
    fn the_sections_come_in_the_registrys_order() {
        let titles: Vec<&str> = sheet_rows(false)
            .iter()
            .map(|(section, _)| section.title())
            .collect();
        let expected: Vec<&str> = Section::ALL.iter().map(|s| s.title()).collect();
        assert_eq!(titles, expected);
    }

    #[test]
    fn keys_are_written_for_the_platform_asked_for() {
        let rows = sheet_rows(true);
        let go_to = rows
            .iter()
            .flat_map(|(_, rows)| rows)
            .find(|row| row.label.starts_with("Go to"))
            .unwrap();
        assert_eq!(go_to.keys, ["⌘P", "⌘K", "⇧⌘K"]);
        let rows = sheet_rows(false);
        let go_to = rows
            .iter()
            .flat_map(|(_, rows)| rows)
            .find(|row| row.label.starts_with("Go to"))
            .unwrap();
        assert_eq!(go_to.keys, ["Ctrl+P", "Ctrl+K", "Ctrl+Shift+K"]);
    }

    #[test]
    fn a_command_without_keys_is_listed_with_none() {
        let rows = sheet_rows(false);
        let remove = rows
            .iter()
            .flat_map(|(_, rows)| rows)
            .find(|row| row.label == "Remove a worktree")
            .unwrap();
        assert!(remove.keys.is_empty());
    }

    #[test]
    fn the_terminal_block_lists_only_chords_that_stay_leons() {
        let off = terminal_rows(false);
        for row in &off {
            for text in &row.keys {
                let plain_ctrl = text.starts_with("Ctrl+") && !text.contains("Shift+");
                assert!(
                    !plain_ctrl || matches!(row.label.as_str(), "Copy the selection" | "Paste"),
                    "{}: {text}",
                    row.label
                );
            }
        }
        let labels: Vec<&str> = off.iter().map(|row| row.label.as_str()).collect();
        assert!(labels.contains(&"Command palette"));
        assert!(labels.contains(&"Copy the selection"));
        assert!(!labels.contains(&"Down"), "bare keys are the program's");
        let mac = terminal_rows(true);
        let new = mac
            .iter()
            .find(|row| row.label == "New agent session")
            .unwrap();
        assert_eq!(new.keys, ["⌘N", "⇧⌘A"]);
    }
}

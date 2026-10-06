//! The sheet that lists every shortcut, generated from the registry.

use super::shell::Shell;
use super::widgets::{key_cap, section_label};
use crate::keys::{self, Command, Section, BINDINGS};
use crate::theme::{metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{div, App, Div, Stateful};

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

impl Shell {
    pub(super) fn render_sheet(&self, colours: &Palette) -> Stateful<Div> {
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
        for (section, rows) in sheet_rows(crate::platform::is_mac()) {
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
        let mut inside = div().flex().flex_col().child(
            div()
                .pb(px(4.))
                .child(section_label("While a terminal has the keyboard", colours)),
        );
        inside = inside.child(
            div()
                .pb(px(6.))
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text_faint)
                .child(if crate::platform::is_mac() {
                    "Only these Cmd chords are Leon's. Every other key, Ctrl chords included, goes to the program."
                } else {
                    "Only these Ctrl+Shift chords are Leon's. Every other key, plain Ctrl chords included, goes to the program."
                }),
        );
        for row in terminal_rows(crate::platform::is_mac()) {
            inside = inside.child(
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
                    ),
            );
        }
        body = body.child(inside.debug_selector(|| "sheet-terminal".into()));
        // The keys of the Settings screen.
        let mut settings = div().flex().flex_col().child(
            div()
                .pb(px(4.))
                .child(section_label("In the Settings screen", colours)),
        );
        for (label, keys) in SETTINGS_KEYS {
            settings = settings.child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(div().min_w_0().truncate().child(*label))
                    .child(div().flex_none().child(key_cap(*keys, colours))),
            );
        }
        body = body.child(settings.debug_selector(|| "sheet-settings".into()));
        // The keys of the Connect screen.
        let mut connect = div().flex().flex_col().child(
            div()
                .pb(px(4.))
                .child(section_label("In the Connect screen", colours)),
        );
        for (label, keys) in connect_keys(crate::platform::is_mac()) {
            connect = connect.child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(div().min_w_0().truncate().child(label))
                    .child(div().flex_none().child(key_cap(keys, colours))),
            );
        }
        body = body.child(connect.debug_selector(|| "sheet-connect".into()));
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
                assert!(!plain_ctrl, "{}: {text}", row.label);
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

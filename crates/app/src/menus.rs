//! The application menu of macOS, generated from the registry of `keys.rs`.
//!
//! There is one source of truth for what the application can do: the table
//! `keys::BINDINGS`. The command palette, the shortcuts sheet, the context
//! menus and this menu bar all read it, so they cannot disagree about a name or
//! a chord. [`LAYOUT`] only says which commands go in which menu and in what
//! order; the label of an item is the command's label, its shortcut is the
//! command's chord on this platform and whether it is enabled comes from
//! [`Availability`] ([`needs_terminal`]).
//!
//! The commands that are not in any menu are listed, with the reason, in
//! [`NOT_IN_MENUS`]: a test fails for a command that is in neither.
//!
//! **How a click becomes a command.** The application does not use GPUI's
//! actions for its own work: it runs [`keys::Command`]s. One action type,
//! [`Run`], carries a command; the shell handles it with the very code that
//! handles the chord and the palette's row, after checking that the command
//! applies right now (so the menu's key equivalents cannot do what the chord's
//! context rules forbid, such as finding in a terminal while typing in the
//! palette). The standard items of the platform (hide, minimise, zoom, full
//! screen, the Services submenu, undo to select all) are the toolkit's.
//!
//! **Where there is no menu bar.** Only macOS has one for the whole
//! application. On Linux and Windows no menu is drawn and every command stays
//! reachable by its chord and the palette.

use crate::keys::{self, Chord, Command};
use crate::product::PRODUCT_NAME;
use gpui_kit::{Action, App, KeyBinding, Keystroke, Menu, MenuItem, OsAction, SystemMenuType};

gpui_kit::actions!(
    leon,
    [
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        ToggleFullScreen,
        BringAllToFront
    ]
);

/// A command of the registry, as an action: what a menu item dispatches.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = leon, no_json)]
pub struct Run(pub Command);

/// The items the platform provides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    /// Hide the application.
    Hide,
    /// Hide the other applications.
    HideOthers,
    /// Show all applications.
    ShowAll,
    /// The Services submenu.
    Services,
    /// Minimise the window.
    Minimize,
    /// Zoom the window.
    Zoom,
    /// Enter or leave full screen.
    FullScreen,
    /// Bring the application's windows to the front.
    BringAllToFront,
    /// Undo, in the text field that has the keyboard.
    Undo,
    /// Redo.
    Redo,
    /// Cut.
    Cut,
    /// Copy: the terminal's selection, or the text field's.
    Copy,
    /// Paste.
    Paste,
    /// Select all: the terminal's buffer, or the text field's text.
    SelectAll,
}

/// One entry of a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A command of the registry.
    Command(Command),
    /// An item of the platform.
    Standard(Standard),
    /// A line between groups.
    Separator,
}

use Command as C;
use Entry::{Command as Cmd, Separator as Sep, Standard as Std};

/// The menus, in order, with what is in them.
pub const LAYOUT: &[(&str, &[Entry])] = &[
    (
        PRODUCT_NAME,
        &[
            Cmd(C::About),
            Cmd(C::CheckForUpdates),
            Cmd(C::RestartToUpdate),
            Sep,
            Cmd(C::Settings),
            Cmd(C::ChooseTheme),
            Sep,
            Std(Standard::Services),
            Sep,
            Std(Standard::Hide),
            Std(Standard::HideOthers),
            Std(Standard::ShowAll),
            Sep,
            Cmd(C::Quit),
        ],
    ),
    (
        "File",
        &[
            Cmd(C::NewSession),
            Cmd(C::AddAgent),
            Cmd(C::RemoveAgent),
            Cmd(C::OpenShell),
            Cmd(C::NewWorktree),
            Cmd(C::OpenProject),
            Cmd(C::CloneProject),
            Cmd(C::NewProject),
            Cmd(C::AddMachine),
            Cmd(C::ShareMachine),
            Sep,
            Cmd(C::OpenFile),
            Cmd(C::QuickOpen),
            Cmd(C::SaveFile),
            Cmd(C::CloseFile),
            Cmd(C::RevealInTree),
            Sep,
            Cmd(C::SaveOutput),
            Cmd(C::SaveOutputAnsi),
            Sep,
            Cmd(C::SleepSession),
            Cmd(C::CloseSession),
            Cmd(C::CloseWindow),
        ],
    ),
    (
        "Edit",
        &[
            Std(Standard::Undo),
            Std(Standard::Redo),
            Sep,
            Std(Standard::Cut),
            Std(Standard::Copy),
            Std(Standard::Paste),
            Std(Standard::SelectAll),
            Sep,
            Cmd(C::PasteText),
            Cmd(C::PasteImage),
            Sep,
            Cmd(C::CopyAll),
            Cmd(C::CopyScreen),
            Sep,
            Cmd(C::Find),
            Cmd(C::FindNext),
            Cmd(C::FindPrevious),
            Sep,
            Cmd(C::FindInFile),
            Cmd(C::ReplaceInFile),
            Sep,
            Cmd(C::FilterProjects),
            Cmd(C::SearchHistory),
            Cmd(C::SearchProject),
        ],
    ),
    (
        "View",
        &[
            Cmd(C::Commands),
            Cmd(C::GoTo),
            Sep,
            Cmd(C::ToggleSidebar),
            Cmd(C::ToggleFiles),
            Cmd(C::TogglePreview),
            Cmd(C::ShowUsage),
            Cmd(C::RefreshUsage),
            Cmd(C::FocusSidebar),
            Cmd(C::FocusMain),
            Cmd(C::FocusTerminal),
            Sep,
            Cmd(C::ToggleAppearance),
            Cmd(C::NextTheme),
            Cmd(C::PreviousTheme),
            Sep,
            Cmd(C::NewThemeFromCurrent),
            Cmd(C::ExportTheme),
            Cmd(C::OpenThemesFolder),
            Cmd(C::ReloadThemes),
            Cmd(C::ShowThemeProblems),
            Sep,
            Cmd(C::OpenSettingsFile),
            Cmd(C::RevealSettingsFolder),
            Sep,
            Cmd(C::Larger),
            Cmd(C::Smaller),
            Cmd(C::ActualSize),
        ],
    ),
    (
        "Session",
        &[
            Cmd(C::SplitRight),
            Cmd(C::SplitDown),
            Sep,
            Cmd(C::NextSplit),
            Cmd(C::PreviousSplit),
            Cmd(C::FocusPaneLeft),
            Cmd(C::FocusPaneRight),
            Cmd(C::FocusPaneUp),
            Cmd(C::FocusPaneDown),
            Sep,
            Cmd(C::ResizeLeft),
            Cmd(C::ResizeRight),
            Cmd(C::ResizeUp),
            Cmd(C::ResizeDown),
            Cmd(C::EqualizeSplits),
            Cmd(C::ToggleZoom),
            Sep,
            Cmd(C::NextTab),
            Cmd(C::PreviousTab),
            Sep,
            Cmd(C::ClearBuffer),
            Cmd(C::ClearScrollback),
            Sep,
            Cmd(C::OpenTranscript),
            Cmd(C::Refresh),
            Cmd(C::ProbeMachine),
            Cmd(C::WhyMissing),
            Cmd(C::RestoreSessions),
        ],
    ),
    (
        "Window",
        &[
            Std(Standard::Minimize),
            Std(Standard::Zoom),
            Std(Standard::FullScreen),
            Sep,
            Std(Standard::BringAllToFront),
        ],
    ),
    (
        "Help",
        &[
            Cmd(C::Shortcuts),
            Sep,
            Cmd(C::ShowReleaseNotes),
            Cmd(C::SkipVersion),
            Cmd(C::OpenDownloadPage),
        ],
    ),
];

/// The commands that are in no menu, and why. Everything else in the registry
/// is in [`LAYOUT`].
#[cfg(test)]
pub const NOT_IN_MENUS: &[(Command, &str)] = &[
    (
        C::Machine(1),
        "a number key: the palette and the chords jump",
    ),
    (C::Tab(1), "a number key: the chords jump"),
    (C::NextPane, "a bare key of the tree"),
    (C::PreviousPane, "a bare key of the tree"),
    (C::Down, "a bare key of the tree"),
    (C::Up, "a bare key of the tree"),
    (C::Top, "a bare key of the tree"),
    (C::Bottom, "a bare key of the tree"),
    (C::PageDown, "a bare key of the tree"),
    (C::PageUp, "a bare key of the tree"),
    (C::Expand, "a bare key of the tree"),
    (C::Collapse, "a bare key of the tree"),
    (C::Open, "a bare key of the tree"),
    (C::Close, "Escape: closes what is open"),
    (C::ContextMenu, "the tree's own menu"),
    (C::ScrollPageUp, "a key of the terminal"),
    (C::ScrollPageDown, "a key of the terminal"),
    (C::Copy, "Edit > Copy, the platform's item"),
    (C::Paste, "Edit > Paste, the platform's item"),
    (C::SelectAll, "Edit > Select All, the platform's item"),
    (C::ResumeSession, "palette and the tree's menu: needs a row"),
    (C::ResumeIn, "palette and the tree's menu: needs a row"),
    (
        C::ResumeAnyway,
        "palette and the tree's menu: needs a session running elsewhere",
    ),
    (
        C::TakeOver,
        "palette and the tree's menu: needs a session running elsewhere",
    ),
    (
        C::RevealTerminal,
        "palette and the tree's menu: needs a session running elsewhere",
    ),
    (C::AddProject, "palette: asks for a path"),
    (C::RemoveProject, "palette and the tree's menu: needs a row"),
    (
        C::RemoveWorktree,
        "palette and the tree's menu: needs a row",
    ),
    (C::RemoveMachine, "palette and the tree's menu: needs a row"),
    (
        C::EditMachine,
        "palette, the tree's menu and the Settings screen: needs a machine",
    ),
    (
        C::WhyOffline,
        "palette and the tree's menu: needs a machine",
    ),
    (C::Rename, "palette and the tree's menu: needs a row"),
    (C::MoveRowUp, "the tree's menu: needs a movable row"),
    (C::MoveRowDown, "the tree's menu: needs a movable row"),
    (C::PinSession, "the tree's menu: needs a session row"),
    (C::UnpinSession, "the tree's menu: needs a pinned session"),
    (C::CopyPath, "palette and the tree's menu: needs a row"),
    (C::CopyBranch, "palette and the tree's menu: needs a row"),
    (C::CopySessionId, "palette and the tree's menu: needs a row"),
    (C::Reveal, "palette and the tree's menu: needs a row"),
    (C::RefreshIcon, "palette and the tree's menu: needs a row"),
    (C::ChooseIcon, "palette and the tree's menu: needs a row"),
    (C::ResetIcon, "palette and the tree's menu: needs a row"),
    (
        C::RemoveFromHistory,
        "palette and the tree's menu: needs a row",
    ),
    (C::SetAppearance, "palette: a question"),
    (C::SetInterfaceSize, "palette: a question"),
    (C::ToggleActiveOnly, "palette and the sidebar's toggle"),
    (C::WidenSidebar, "palette: a step of a drag"),
    (C::NarrowSidebar, "palette: a step of a drag"),
    (C::ResetSidebarWidth, "palette: a step of a drag"),
];

/// Whether a command is in [`NOT_IN_MENUS`] (the numbered ones by any number).
#[cfg(test)]
pub fn is_left_out(command: Command) -> bool {
    NOT_IN_MENUS.iter().any(|(left, _)| {
        matches!(
            (left, command),
            (C::Machine(_), C::Machine(_)) | (C::Tab(_), C::Tab(_))
        ) || *left == command
    })
}

/// What decides whether an item is enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Availability {
    /// A terminal is on screen.
    pub terminal: bool,
    /// The sidebar is showing (its item says "Hide" or "Show").
    pub sidebar: bool,
    /// A file is on screen: the pane commands apply to it too, and the file
    /// commands apply at all.
    pub file: bool,
}

/// Whether a pane command also applies to a file on screen.
fn works_on_file(command: Command) -> bool {
    matches!(
        command,
        C::SplitRight
            | C::SplitDown
            | C::NextSplit
            | C::PreviousSplit
            | C::FocusPaneLeft
            | C::FocusPaneRight
            | C::FocusPaneUp
            | C::FocusPaneDown
            | C::ResizeLeft
            | C::ResizeRight
            | C::ResizeUp
            | C::ResizeDown
            | C::EqualizeSplits
            | C::ToggleZoom
            | C::NextTab
            | C::PreviousTab
            | C::CloseSession
    )
}

/// Whether a command does something only for a terminal on screen.
pub fn needs_terminal(command: Command) -> bool {
    matches!(
        command,
        C::Find
            | C::FindNext
            | C::FindPrevious
            | C::ClearBuffer
            | C::ClearScrollback
            | C::CopyAll
            | C::CopyScreen
            | C::PasteText
            | C::PasteImage
            | C::SaveOutput
            | C::SaveOutputAnsi
            | C::SplitRight
            | C::SplitDown
            | C::NextSplit
            | C::PreviousSplit
            | C::FocusPaneLeft
            | C::FocusPaneRight
            | C::FocusPaneUp
            | C::FocusPaneDown
            | C::ResizeLeft
            | C::ResizeRight
            | C::ResizeUp
            | C::ResizeDown
            | C::EqualizeSplits
            | C::ToggleZoom
            | C::NextTab
            | C::PreviousTab
            | C::CloseSession
            | C::SleepSession
    )
}

/// One item as the menu shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A command.
    Command {
        /// What it is called: the registry's label.
        label: String,
        /// The command.
        command: Command,
        /// The chord shown, written as the platform writes it.
        chord: Option<String>,
        /// The keystroke the platform's menu binds (never a bare key).
        keystroke: Option<String>,
        /// Whether it can be chosen now.
        enabled: bool,
    },
    /// An item of the platform.
    Standard(Standard),
    /// A line between groups.
    Separator,
}

/// One menu as data.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    /// Its title.
    pub name: &'static str,
    /// Its items.
    pub items: Vec<Item>,
}

/// The first chord of `command` on this platform that a menu may bind: one that
/// holds a modifier, because a bare key (`?`, `/`) is a character in a text
/// field.
///
/// `Filter the projects` and `Find in the file` share `Cmd+F` with finding in
/// a terminal, which wins while a terminal has the keyboard; a menu binds a
/// key equivalent to one item only, so theirs have no shortcut in the menu
/// (their chords work as always where they apply).
pub fn menu_chord(command: Command, mac: bool) -> Option<Chord> {
    // The same goes for saving the terminal's output, which shares Cmd+S
    // with saving a file: the menu's Cmd+S saves the file, and a terminal
    // that has the keyboard still gets its own (the key is resolved first).
    if matches!(command, C::FilterProjects | C::SaveOutput | C::FindInFile) {
        return None;
    }
    let binding = keys::binding(command)?;
    keys::chords_on(binding, mac)
        .into_iter()
        .find(|chord| chord.secondary || chord.alt || chord.control)
}

/// A chord as GPUI writes keystrokes: `cmd-shift-k`.
pub fn keystroke_of(chord: &Chord) -> String {
    let mut text = String::new();
    if chord.control {
        text.push_str("ctrl-");
    }
    if chord.alt {
        text.push_str("alt-");
    }
    if chord.secondary {
        text.push_str("cmd-");
    }
    if chord.shift && !matches!(chord.key, "+" | "?" | ":" | "<" | ">" | "_" | "~" | "!") {
        text.push_str("shift-");
    }
    text.push_str(chord.key);
    text
}

/// The menus for a platform that is, or is not, macOS, with what is
/// available.
pub fn spec(available: Availability, mac: bool) -> Vec<Spec> {
    LAYOUT
        .iter()
        .map(|(name, entries)| Spec {
            name,
            items: entries
                .iter()
                .map(|entry| match entry {
                    Entry::Separator => Item::Separator,
                    Entry::Standard(standard) => Item::Standard(*standard),
                    Entry::Command(command) => {
                        let chord = menu_chord(*command, mac);
                        // The sidebar's item says what it will do.
                        let label = match command {
                            C::ToggleSidebar if available.sidebar => "Hide the sidebar",
                            C::ToggleSidebar => "Show the sidebar",
                            other => keys::label(*other),
                        };
                        Item::Command {
                            label: label.to_owned(),
                            command: *command,
                            chord: chord.as_ref().map(|chord| chord.label_for(mac)),
                            keystroke: chord.as_ref().map(keystroke_of),
                            enabled: if keys::needs_file(*command) {
                                available.file
                            } else {
                                available.terminal
                                    || !needs_terminal(*command)
                                    || (available.file && works_on_file(*command))
                            },
                        }
                    }
                })
                .collect(),
        })
        .collect()
}

fn standard_item(standard: Standard) -> MenuItem {
    use gpui_kit::base::input as text;
    match standard {
        Standard::Hide => MenuItem::action(format!("Hide {PRODUCT_NAME}"), Hide),
        Standard::HideOthers => MenuItem::action("Hide Others", HideOthers),
        Standard::ShowAll => MenuItem::action("Show All", ShowAll),
        Standard::Services => MenuItem::os_submenu("Services", SystemMenuType::Services),
        Standard::Minimize => MenuItem::action("Minimize", Minimize),
        Standard::Zoom => MenuItem::action("Zoom", Zoom),
        Standard::FullScreen => MenuItem::action("Enter Full Screen", ToggleFullScreen),
        Standard::BringAllToFront => MenuItem::action("Bring All to Front", BringAllToFront),
        Standard::Undo => MenuItem::os_action("Undo", text::Undo, OsAction::Undo),
        Standard::Redo => MenuItem::os_action("Redo", text::Redo, OsAction::Redo),
        Standard::Cut => MenuItem::os_action("Cut", text::Cut, OsAction::Cut),
        Standard::Copy => MenuItem::os_action("Copy", text::Copy, OsAction::Copy),
        Standard::Paste => MenuItem::os_action("Paste", text::Paste, OsAction::Paste),
        Standard::SelectAll => {
            MenuItem::os_action("Select All", text::SelectAll, OsAction::SelectAll)
        }
    }
}

/// The platform's menus for `specs`.
fn menus_of(specs: Vec<Spec>) -> Vec<Menu> {
    specs
        .into_iter()
        .map(|spec| {
            Menu::new(spec.name).items(spec.items.into_iter().map(|item| match item {
                Item::Separator => MenuItem::separator(),
                Item::Standard(standard) => standard_item(standard),
                Item::Command {
                    label,
                    command,
                    enabled,
                    ..
                } => MenuItem::action(label, Run(command)).disabled(!enabled),
            }))
        })
        .collect()
}

/// The keys of the standard items, and the chords of the commands in the
/// menus: what the platform shows next to an item and binds as its key
/// equivalent.
fn bindings(specs: &[Spec]) -> Vec<KeyBinding> {
    let mut all = vec![
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
    ];
    for spec in specs {
        for item in &spec.items {
            if let Item::Command {
                command,
                keystroke: Some(keystroke),
                ..
            } = item
            {
                if Keystroke::parse(keystroke).is_ok() {
                    all.push(KeyBinding::new(keystroke, Run(*command), None));
                }
            }
        }
    }
    all
}

/// Installs the menu bar on macOS and the handlers of the platform's items;
/// elsewhere it does nothing. Call once, after the settings are in.
pub fn install(available: Availability, cx: &mut App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &BringAllToFront, cx| cx.activate(true));
    let specs = spec(available, true);
    cx.bind_keys(bindings(&specs));
    cx.set_menus(menus_of(specs));
    tracing::info!("the application menu is installed");
}

/// Replaces the menus when what is available changed.
pub fn refresh(available: Availability, cx: &mut App) {
    if cfg!(target_os = "macos") {
        cx.set_menus(menus_of(spec(available, true)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn everything() -> Availability {
        Availability {
            terminal: true,
            sidebar: true,
            file: true,
        }
    }

    fn commands(specs: &[Spec]) -> Vec<Command> {
        specs
            .iter()
            .flat_map(|spec| &spec.items)
            .filter_map(|item| match item {
                Item::Command { command, .. } => Some(*command),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_command_of_the_registry_is_in_a_menu_or_left_out_with_a_reason() {
        let in_menus: HashSet<Command> = commands(&spec(everything(), true)).into_iter().collect();
        for binding in keys::BINDINGS {
            let command = binding.command;
            let present = in_menus.contains(&command);
            let left_out = is_left_out(command);
            assert!(
                present ^ left_out,
                "{command:?} ({}) is {} menus",
                binding.label,
                if present {
                    "in the menus and also marked as left out of"
                } else {
                    "in no menu and not marked as left out of the"
                }
            );
        }
        // Nothing in a menu is outside the registry.
        for command in &in_menus {
            assert!(keys::binding(*command).is_some(), "{command:?}");
        }
    }

    #[test]
    fn every_item_carries_the_registrys_label_and_chord() {
        for spec in spec(everything(), true) {
            for item in spec.items {
                let Item::Command {
                    label,
                    command,
                    chord,
                    ..
                } = item
                else {
                    continue;
                };
                if command != Command::ToggleSidebar {
                    assert_eq!(label, keys::label(command), "{command:?}");
                }
                let expected = menu_chord(command, true).map(|chord| chord.label_for(true));
                assert_eq!(chord, expected, "{command:?}");
            }
        }
    }

    #[test]
    fn no_chord_is_on_two_menu_items() {
        let mut seen: HashMap<String, Command> = HashMap::new();
        for spec in spec(everything(), true) {
            for item in spec.items {
                if let Item::Command {
                    command,
                    keystroke: Some(keystroke),
                    ..
                } = item
                {
                    if let Some(other) = seen.insert(keystroke.clone(), command) {
                        panic!("{keystroke} is on {other:?} and {command:?}");
                    }
                }
            }
        }
        // The platform's own keys are not the commands'.
        for own in ["cmd-h", "alt-cmd-h", "cmd-m", "ctrl-cmd-f"] {
            assert!(!seen.contains_key(own), "{own}");
        }
    }

    #[test]
    fn a_menu_never_binds_a_bare_key() {
        for spec in spec(everything(), true) {
            for item in spec.items {
                if let Item::Command {
                    keystroke: Some(keystroke),
                    command,
                    ..
                } = item
                {
                    assert!(
                        keystroke.contains("cmd-")
                            || keystroke.contains("alt-")
                            || keystroke.contains("ctrl-"),
                        "{command:?}: {keystroke}"
                    );
                    assert!(Keystroke::parse(&keystroke).is_ok(), "{keystroke}");
                }
            }
        }
    }

    #[test]
    fn quit_is_command_q_on_macos_and_ctrl_shift_q_elsewhere_and_is_in_the_app_menu() {
        let mac = spec(everything(), true);
        let app = mac.iter().find(|menu| menu.name == PRODUCT_NAME).unwrap();
        let quit = app.items.iter().find_map(|item| match item {
            Item::Command {
                command: Command::Quit,
                chord,
                keystroke,
                ..
            } => Some((chord.clone(), keystroke.clone())),
            _ => None,
        });
        assert_eq!(
            quit,
            Some((Some("⌘Q".to_owned()), Some("cmd-q".to_owned())))
        );
        let binding = keys::binding(Command::Quit).unwrap();
        let other: Vec<String> = keys::chords_on(binding, false)
            .iter()
            .map(|chord| chord.label_for(false))
            .collect();
        assert_eq!(other, ["Ctrl+Shift+Q"]);
        // A plain Ctrl+Q stays the program's: XON.
        assert!(!keys::terminal_chords(false)
            .iter()
            .any(|(command, chord)| *command == Command::Quit && !chord.shift));
    }

    #[test]
    fn terminal_items_are_disabled_without_a_terminal_and_the_sidebar_item_says_what_it_does() {
        let none = spec(
            Availability {
                terminal: false,
                sidebar: false,
                file: false,
            },
            true,
        );
        for item in none.iter().flat_map(|spec| &spec.items) {
            if let Item::Command {
                command,
                enabled,
                label,
                ..
            } = item
            {
                assert_eq!(
                    *enabled,
                    !needs_terminal(*command) && !keys::needs_file(*command),
                    "{command:?}"
                );
                if *command == Command::ToggleSidebar {
                    assert_eq!(label, "Show the sidebar");
                }
            }
        }
        let shown = spec(everything(), true);
        assert!(shown.iter().flat_map(|spec| &spec.items).any(
            |item| matches!(item, Item::Command { label, .. } if label == "Hide the sidebar")
        ));
    }

    #[test]
    fn the_layout_follows_the_macos_menu_conventions() {
        let names: Vec<&str> = LAYOUT.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            [
                PRODUCT_NAME,
                "File",
                "Edit",
                "View",
                "Session",
                "Window",
                "Help"
            ]
        );
        let standard = |menu: &str| -> Vec<Standard> {
            LAYOUT
                .iter()
                .find(|(name, _)| *name == menu)
                .unwrap()
                .1
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Standard(standard) => Some(*standard),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            standard(PRODUCT_NAME),
            [
                Standard::Services,
                Standard::Hide,
                Standard::HideOthers,
                Standard::ShowAll
            ]
        );
        assert_eq!(
            standard("Edit"),
            [
                Standard::Undo,
                Standard::Redo,
                Standard::Cut,
                Standard::Copy,
                Standard::Paste,
                Standard::SelectAll
            ]
        );
        assert_eq!(
            standard("Window"),
            [
                Standard::Minimize,
                Standard::Zoom,
                Standard::FullScreen,
                Standard::BringAllToFront
            ]
        );
    }

    #[test]
    fn keystrokes_are_written_the_way_the_toolkit_reads_them() {
        let chord = |command| menu_chord(command, true).unwrap();
        assert_eq!(keystroke_of(&chord(Command::Settings)), "cmd-,");
        assert_eq!(keystroke_of(&chord(Command::Commands)), "cmd-shift-p");
        assert_eq!(keystroke_of(&chord(Command::ClearScrollback)), "alt-cmd-k");
        assert_eq!(keystroke_of(&chord(Command::ResizeLeft)), "ctrl-cmd-left");
        assert_eq!(keystroke_of(&chord(Command::Larger)), "cmd-=");
    }
}

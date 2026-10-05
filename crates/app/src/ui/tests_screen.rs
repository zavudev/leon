//! Tests of the Settings screen: how it opens and closes, where the keyboard
//! is in it, and that every kind of control can be used from the keyboard and
//! with the mouse. The scripted terminals of `tests.rs` stand in for real
//! ones; every settings file is in a temporary folder.

use super::live::{open_live, real_worktree, terminal_of, wait_until};
use super::*;
use crate::schema::{self, Section, Value};
use crate::ui::settings_screen::{Entry, Zone};
use leon_core::MachineKind;

fn window(cx: &mut TestAppContext, dir: &tempfile::TempDir) -> Harness {
    open_with(
        cx,
        ScriptedRunner::new(),
        Some(dir.path().join(settings::FILE_NAME)),
    )
}

fn open_screen(h: &Harness, cx: &mut TestAppContext) {
    h.press("ctrl-,", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
}

fn zone(h: &Harness, cx: &mut TestAppContext) -> Zone {
    h.shell(cx, |s| s.settings_ui.zone)
}

/// Moves the cursor to the option of this key by pressing `down`.
fn go_to(h: &Harness, key: &str, cx: &mut TestAppContext) {
    h.press("home", cx);
    for _ in 0..80 {
        let here = h.shell(cx, |s| {
            match s.settings_entries().get(s.settings_ui.cursor) {
                Some(Entry::Setting(def)) => def.key == key,
                _ => false,
            }
        });
        if here {
            return;
        }
        h.press("down", cx);
    }
    panic!("{key} is not reachable in this section");
}

fn section(h: &Harness, section: Section, cx: &mut TestAppContext) {
    cx.update(|cx| {
        h.shell
            .update(cx, |shell, cx| shell.settings_pick_section(section, cx))
    });
    h.settle(cx);
}

fn int(cx: &mut TestAppContext, key: &str) -> i64 {
    cx.update(|cx| settings::int(cx, key))
}

fn text(cx: &mut TestAppContext, key: &str) -> String {
    cx.update(|cx| settings::text(cx, key))
}

fn flag(cx: &mut TestAppContext, key: &str) -> bool {
    cx.update(|cx| settings::flag(cx, key))
}

#[gpui_kit::test]
fn pressing_secondary_comma_opens_settings_and_escape_restores_focus(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell's prompt", |h, cx| {
        terminal_of(h, cx, 1).is_some_and(|t| t.screen_text().contains("READY>"))
    });
    assert!(
        h.shell(cx, |s| s.terminal_focused()),
        "the terminal has the keyboard"
    );
    if crate::platform::is_mac() {
        h.press("ctrl-,", cx);
    } else {
        // Off macOS a plain Ctrl+, belongs to the program in a terminal and
        // Settings has no Ctrl+Shift chord: it is reached through the palette.
        h.press("ctrl-shift-p", cx);
        h.type_text("Settings", cx);
        h.press("enter", cx);
    }
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
    assert!(h.shows("settings", cx));
    assert!(
        !h.shell(cx, |s| s.terminal_focused()),
        "the card has the keyboard"
    );
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert!(!h.shows("settings", cx));
    assert!(
        h.shell(cx, |s| s.terminal_focused()),
        "the terminal has it again"
    );
    // What is typed now reaches the program, not the screen.
    h.type_text("echo", cx);
    assert!(super::live::script_of(&h, 1)
        .written_text()
        .contains("echo"));
}

#[gpui_kit::test]
fn escape_returns_the_keyboard_to_the_sidebars_filter_when_it_had_it(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-f", cx);
    assert!(cx
        .update_window(h.window.into(), |_, w, cx| h
            .shell
            .read(cx)
            .filter_focused(w, cx))
        .unwrap());
    open_screen(&h, cx);
    h.press("escape", cx);
    assert!(cx
        .update_window(h.window.into(), |_, w, cx| h
            .shell
            .read(cx)
            .filter_focused(w, cx))
        .unwrap());
}

#[gpui_kit::test]
fn the_gear_the_palette_and_the_menu_command_open_the_screen(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.mouse_on("tool-settings".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
    h.press("escape", cx);
    h.press("ctrl-shift-p", cx);
    h.type_text("settings", cx);
    assert_eq!(h.palette_titles(cx)[1], "Settings");
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
    h.press("escape", cx);
    // The macOS menu runs the command of the registry.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            let _ = cx;
            assert!(crate::menus::LAYOUT
                .iter()
                .flat_map(|(_, entries)| entries.iter())
                .any(|entry| *entry == crate::menus::Entry::Command(Command::Settings)));
            let _ = shell;
        })
    });
}

#[gpui_kit::test]
fn the_screen_lists_every_section_and_the_options_of_the_one_chosen(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    for (number, _) in Section::ALL.iter().enumerate() {
        assert!(h.shows_dynamic(format!("settings-section-{number}"), cx));
    }
    for key in ["theme_id", "theme", "interface_scale", "blueprint_lines"] {
        assert!(h.shows_dynamic(format!("settings-row-{key}"), cx), "{key}");
    }
    assert!(!h.shows_dynamic("settings-row-terminal_scrollback".to_owned(), cx));
    section(&h, Section::Terminal, cx);
    assert!(h.shows_dynamic("settings-row-terminal_scrollback".to_owned(), cx));
}

#[gpui_kit::test]
fn tab_moves_between_the_sections_the_search_and_the_options(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    assert_eq!(zone(&h, cx), Zone::Options);
    h.press("tab", cx);
    assert_eq!(zone(&h, cx), Zone::Sections);
    h.press("tab", cx);
    assert_eq!(zone(&h, cx), Zone::Search);
    h.press("tab", cx);
    assert_eq!(zone(&h, cx), Zone::Options);
    h.press("shift-tab", cx);
    assert_eq!(zone(&h, cx), Zone::Search);
    // In the sections j and k change the section.
    h.press("shift-tab", cx);
    assert_eq!(zone(&h, cx), Zone::Sections);
    h.press("j", cx);
    assert_eq!(h.shell(cx, |s| s.settings_ui.section), Section::Terminal);
    h.press("k", cx);
    assert_eq!(h.shell(cx, |s| s.settings_ui.section), Section::Appearance);
}

#[gpui_kit::test]
fn the_search_filters_options_across_sections_and_typing_j_is_text(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    h.press("/", cx);
    assert_eq!(zone(&h, cx), Zone::Search);
    h.type_text("scrollback", cx);
    assert_eq!(h.shell(cx, |s| s.settings_ui.query.clone()), "scrollback");
    assert!(h.shows_dynamic("settings-row-terminal_scrollback".to_owned(), cx));
    assert!(!h.shows_dynamic("settings-row-theme_id".to_owned(), cx));
    // j and k are letters while the search has the keyboard.
    h.type_text("jk", cx);
    assert_eq!(h.shell(cx, |s| s.settings_ui.query.clone()), "scrollbackjk");
    assert!(h.shows("settings-empty", cx));
    // Escape clears the search before it closes the screen.
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
    assert_eq!(h.shell(cx, |s| s.settings_ui.query.clone()), "");
    assert!(h.shows_dynamic("settings-row-theme_id".to_owned(), cx));
    // Searching by what a setting does, not only by its name.
    h.type_text("github", cx);
    assert!(h.shows_dynamic("settings-row-fetch_avatars".to_owned(), cx));
    // Enter leaves the field for the results.
    h.press("enter", cx);
    assert_eq!(zone(&h, cx), Zone::Options);
}

#[gpui_kit::test]
fn a_toggle_flips_with_space_and_enter_and_a_click(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    section(&h, Section::Terminal, cx);
    go_to(&h, "terminal_copy_on_select", cx);
    assert!(!flag(cx, "terminal_copy_on_select"));
    h.press("space", cx);
    assert!(flag(cx, "terminal_copy_on_select"));
    h.press("enter", cx);
    assert!(!flag(cx, "terminal_copy_on_select"));
    h.mouse_on(
        "settings-toggle-terminal_copy_on_select".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(flag(cx, "terminal_copy_on_select"), "a click on the switch");
    h.press("right", cx);
    assert!(
        !flag(cx, "terminal_copy_on_select"),
        "right steps a toggle too"
    );
}

#[gpui_kit::test]
fn left_and_right_step_a_choice_and_a_number_and_enter_opens_the_choices(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    section(&h, Section::Terminal, cx);
    go_to(&h, "terminal_cursor", cx);
    h.press("right", cx);
    assert_eq!(text(cx, "terminal_cursor"), "beam");
    h.press("left", cx);
    h.press("left", cx);
    assert_eq!(text(cx, "terminal_cursor"), "underline");
    go_to(&h, "terminal_font_size", cx);
    h.press("right", cx);
    assert_eq!(int(cx, "terminal_font_size"), 14);
    h.press("left", cx);
    h.press("left", cx);
    assert_eq!(int(cx, "terminal_font_size"), 12);
    // Enter on a choice asks in the palette and the screen is back after it.
    go_to(&h, "terminal_cursor", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert_eq!(h.palette_titles(cx), ["Block", "Beam", "Underline"]);
    h.type_text("block", cx);
    h.press("enter", cx);
    assert_eq!(text(cx, "terminal_cursor"), "block");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
}

#[gpui_kit::test]
fn a_number_and_a_text_are_typed_in_place_and_a_bad_one_stays_open_with_its_reason(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    section(&h, Section::Terminal, cx);
    go_to(&h, "terminal_font_size", cx);
    h.press("enter", cx);
    assert!(h.shows_dynamic("settings-edit-terminal_font_size".to_owned(), cx));
    // The field holds the keyboard: j is a character, not a move.
    cx.update_window(h.window.into(), |_, w, cx| {
        for _ in 0..2 {
            w.press("backspace", cx);
        }
    })
    .unwrap();
    h.type_text("99", cx);
    h.press("enter", cx);
    assert_eq!(int(cx, "terminal_font_size"), 13);
    assert!(h.shows_dynamic("settings-error-terminal_font_size".to_owned(), cx));
    cx.update_window(h.window.into(), |_, w, cx| {
        for _ in 0..2 {
            w.press("backspace", cx);
        }
    })
    .unwrap();
    h.type_text("16", cx);
    h.press("enter", cx);
    assert_eq!(int(cx, "terminal_font_size"), 16);
    assert!(!h.shows_dynamic("settings-edit-terminal_font_size".to_owned(), cx));

    go_to(&h, "terminal_font_family", cx);
    h.press("enter", cx);
    h.type_text("JetBrains Mono", cx);
    h.press("enter", cx);
    assert_eq!(text(cx, "terminal_font_family"), "JetBrains Mono");
    h.press("enter", cx);
    h.press("escape", cx);
    assert_eq!(
        h.shell(cx, |s| s.overlay),
        Overlay::Settings,
        "escape only cancels the edit"
    );
    assert_eq!(text(cx, "terminal_font_family"), "JetBrains Mono");
}

#[gpui_kit::test]
fn a_list_is_edited_through_the_palette_and_a_button_runs(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    section(&h, Section::Terminal, cx);
    go_to(&h, "terminal_env", cx);
    h.press("enter", cx);
    h.press("enter", cx);
    h.type_text("A=1", cx);
    h.press("enter", cx);
    assert_eq!(cx.update(|cx| settings::list(cx, "terminal_env")), ["A=1"]);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
    // A button runs its action.
    section(&h, Section::Sessions, cx);
    go_to(&h, "reimport", cx);
    h.press("enter", cx);
    assert!(!h.status().is_empty());
}

#[gpui_kit::test]
fn a_modified_option_wears_a_marker_and_resets_to_its_default(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    section(&h, Section::Terminal, cx);
    assert!(!h.shows_dynamic("settings-modified-terminal_font_size".to_owned(), cx));
    assert!(!h.shows_dynamic("settings-reset-terminal_font_size".to_owned(), cx));
    go_to(&h, "terminal_font_size", cx);
    h.press("right", cx);
    assert!(h.shows_dynamic("settings-modified-terminal_font_size".to_owned(), cx));
    h.press("r", cx);
    assert_eq!(int(cx, "terminal_font_size"), 13);
    assert!(!h.shows_dynamic("settings-modified-terminal_font_size".to_owned(), cx));
    // The per-option button does the same with the mouse.
    h.press("right", cx);
    h.press("right", cx);
    assert_eq!(int(cx, "terminal_font_size"), 15);
    h.mouse_on(
        "settings-reset-terminal_font_size".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(int(cx, "terminal_font_size"), 13);
    let saved = schema::Store::parse(&std::fs::read(dir.path().join(settings::FILE_NAME)).unwrap())
        .unwrap();
    assert_eq!(saved.value("terminal_font_size"), Value::Int(13));
}

#[gpui_kit::test]
fn changes_are_live_the_theme_and_the_size_change_while_the_screen_is_open(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    go_to(&h, "interface_scale", cx);
    h.press("right", cx);
    assert_eq!(cx.update(|cx| settings::get(cx).interface_scale), 110);
    h.press("left", cx);
    h.press("left", cx);
    assert_eq!(cx.update(|cx| settings::get(cx).interface_scale), 90);
    h.press("r", cx);
    assert_eq!(cx.update(|cx| settings::get(cx).interface_scale), 100);
    go_to(&h, "theme_id", cx);
    h.press("right", cx);
    assert_eq!(
        cx.update(|cx| crate::theme::current(cx)),
        crate::theme::ThemeId::Zavu
    );
}

#[gpui_kit::test]
fn resetting_everything_asks_to_confirm_first(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    section(&h, Section::Terminal, cx);
    go_to(&h, "terminal_font_size", cx);
    h.press("right", cx);
    section(&h, Section::Advanced, cx);
    go_to(&h, "reset_all", cx);
    h.press("enter", cx);
    assert_eq!(
        int(cx, "terminal_font_size"),
        14,
        "the first Enter only asks"
    );
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
    assert_eq!(int(cx, "terminal_font_size"), 14, "Escape cancels it");
    h.press("enter", cx);
    h.press("enter", cx);
    assert_eq!(int(cx, "terminal_font_size"), 13);
}

#[gpui_kit::test]
fn the_keyboard_section_lists_the_commands_with_a_note_that_it_is_read_only(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    section(&h, Section::Keyboard, cx);
    assert!(h.shows("settings-keyboard-note", cx));
    let rows = h.shell(cx, |s| s.settings_entries().len());
    assert!(rows > 50, "{rows}");
    // Enter on a row changes nothing and nothing is a button here.
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
    h.press("/", cx);
    h.type_text("split", cx);
    let narrowed = h.shell(cx, |s| s.settings_entries().len());
    assert!(narrowed > 0 && narrowed < rows);
}

#[gpui_kit::test]
fn the_screen_is_the_same_set_of_options_as_the_schema(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    for def in schema::SETTINGS.iter().filter(|d| d.platform.here()) {
        section(&h, def.section, cx);
        assert!(
            h.shows_dynamic(format!("settings-row-{}", def.key), cx),
            "{} is not drawn",
            def.key
        );
    }
}

fn machine_rows(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
    h.shell(cx, |s| {
        s.settings_entries()
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Machine(machine) => Some(machine.name),
                _ => None,
            })
            .collect()
    })
}

#[gpui_kit::test]
fn the_machines_section_lists_the_ssh_machines_and_probes_edits_them(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = open_with(
        cx,
        ScriptedRunner::new()
            .reply(Output::ok(PROBE_OUTPUT))
            // The test the Connect screen runs before saving.
            .reply(Output::ok(""))
            .reply(Output::ok(PROBE_OUTPUT)),
        Some(dir.path().join(settings::FILE_NAME)),
    );
    open_screen(&h, cx);
    section(&h, Section::Machines, cx);
    let names = machine_rows(&h, cx);
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(
        h.shows_dynamic("settings-machine-3".to_owned(), cx)
            || (0..12).any(|n| h.shows_dynamic(format!("settings-machine-{n}"), cx))
    );
    // Enter on the machine probes it.
    let at = h.shell(cx, |s| {
        s.settings_entries()
            .iter()
            .position(|e| matches!(e, Entry::Machine(_)))
            .unwrap()
    });
    cx.update(|cx| h.shell.update(cx, |s, _| s.settings_ui.cursor = at));
    h.press("enter", cx);
    let id = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|m| !m.id.is_local())
        .unwrap()
        .id;
    assert!(matches!(
        h.engine.machine_state(&id),
        MachineState::Online(Some(_))
    ));
    // E opens the Connect screen on the machine, filled in.
    h.press("e", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Connect);
    assert_eq!(
        h.shell(cx, |s| s.connect_ui.form.host.clone()),
        "build.example"
    );
    h.press("tab", cx);
    h.type_text(".new", cx);
    h.press("ctrl-s", cx);
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    let machine = h.store.machine(&id).unwrap();
    assert!(
        matches!(machine.kind, MachineKind::Ssh { ref host, .. } if host == "build.example.new")
    );
    // X removes it, after asking.
    open_screen(&h, cx);
    section(&h, Section::Machines, cx);
    cx.update(|cx| h.shell.update(cx, |s, _| s.settings_ui.cursor = at));
    h.press("x", cx);
    assert_eq!(h.palette_titles(cx).len(), 2);
    h.press("enter", cx);
    assert!(machine_rows(&h, cx).is_empty());
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
}

#[gpui_kit::test]
fn removed_project_roots_are_listed_in_projects_and_can_be_allowed_again(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    let project = h.store.projects(None).unwrap().remove(0);
    h.store.remove_project(&project.id).unwrap();
    h.settle(cx);
    open_screen(&h, cx);
    section(&h, Section::Projects, cx);
    let roots = |h: &Harness, cx: &mut TestAppContext| {
        h.shell(cx, |s| {
            s.settings_entries()
                .into_iter()
                .filter_map(|e| match e {
                    Entry::Dismissed(d) => Some(d.root),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(roots(&h, cx), std::slice::from_ref(&project.root));
    let at = h.shell(cx, |s| {
        s.settings_entries()
            .iter()
            .position(|e| matches!(e, Entry::Dismissed(_)))
            .unwrap()
    });
    cx.update(|cx| h.shell.update(cx, |s, _| s.settings_ui.cursor = at));
    h.press("enter", cx);
    h.settle(cx);
    assert!(roots(&h, cx).is_empty(), "allowed again");
    assert!(h
        .store
        .dismissed_roots(&project.machine_id)
        .unwrap()
        .is_empty());
}

#[gpui_kit::test]
fn the_about_button_shows_the_about_panel_and_the_folder_buttons_reveal(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    open_screen(&h, cx);
    section(&h, Section::Advanced, cx);
    go_to(&h, "open_data_folder", cx);
    h.press("enter", cx);
    assert_eq!(*h.revealed.borrow(), [dir.path().to_path_buf()]);
    go_to(&h, "about", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::About);
}

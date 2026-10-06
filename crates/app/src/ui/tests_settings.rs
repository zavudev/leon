//! Tests of the settings in the window: the palette finds and changes every
//! kind of setting, the settings file is followed live, and the commands that
//! open it. Every file is in a temporary folder; the real configuration
//! directory is never read.

use super::*;
use crate::schema::{self, Value};

fn window(cx: &mut TestAppContext, dir: &tempfile::TempDir) -> Harness {
    open_with(
        cx,
        ScriptedRunner::new(),
        Some(dir.path().join(settings::FILE_NAME)),
    )
}

fn saved(dir: &tempfile::TempDir) -> schema::Store {
    schema::Store::parse(&std::fs::read(dir.path().join(settings::FILE_NAME)).unwrap()).unwrap()
}

fn int(cx: &mut TestAppContext, key: &str) -> i64 {
    cx.update(|cx| settings::int(cx, key))
}

#[gpui_kit::test]
fn every_setting_is_found_in_the_palette_by_its_label_and_by_a_keyword(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    // One palette, the field replaced in one step per setting: the loop goes
    // through all of them, and typing is covered by the tests below and by
    // the two typed keywords at its end.
    h.press("ctrl-shift-p", cx);
    for def in schema::settings().iter().filter(|d| d.platform.here()) {
        h.set_palette_text(&format!(">{}", def.label), cx);
        let found = h.palette_titles(cx);
        assert!(
            found.contains(&format!("setting:{}", def.key)),
            "{} is not found by its label: {found:?}",
            def.key
        );
    }
    h.press("escape", cx);
    h.press("ctrl-shift-p", cx);
    h.type_text("scrollback", cx);
    assert!(h
        .palette_titles(cx)
        .contains(&"setting:terminal_scrollback".to_owned()));
    h.press("escape", cx);
    h.press("ctrl-shift-p", cx);
    h.type_text("beep", cx);
    assert!(h
        .palette_titles(cx)
        .contains(&"setting:terminal_bell_mark".to_owned()));
}

#[gpui_kit::test]
fn a_number_is_changed_from_the_palette_and_saved(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("scrollback lines", cx);
    h.press("enter", cx);
    h.type_text("5000", cx);
    h.press("enter", cx);
    assert_eq!(int(cx, "terminal_scrollback"), 5000);
    assert_eq!(saved(&dir).value("terminal_scrollback"), Value::Int(5000));
    assert!(
        !h.shell(cx, |s| s.overlay == Overlay::Palette),
        "the palette closed"
    );
}

#[gpui_kit::test]
fn a_number_out_of_range_is_not_accepted_by_the_palette(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("font size", cx);
    h.press("enter", cx);
    h.type_text("400", cx);
    h.press("enter", cx);
    assert_eq!(int(cx, "terminal_font_size"), 13);
    assert!(
        h.shell(cx, |s| s.overlay == Overlay::Palette),
        "it asks again"
    );
}

#[gpui_kit::test]
fn a_toggle_and_a_choice_are_changed_from_the_palette(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("copy on select", cx);
    h.press("enter", cx);
    assert_eq!(h.palette_titles(cx), ["On", "Off"]);
    // The question opens on the value in use.
    h.press("up", cx);
    h.press("enter", cx);
    assert!(cx.update(|cx| settings::flag(cx, "terminal_copy_on_select")));

    h.press("ctrl-shift-p", cx);
    h.type_text("cursor shape", cx);
    h.press("enter", cx);
    h.type_text("beam", cx);
    h.press("enter", cx);
    assert_eq!(
        cx.update(|cx| settings::text(cx, "terminal_cursor")),
        "beam"
    );
}

#[gpui_kit::test]
fn a_list_gets_an_entry_from_the_palette_and_a_bad_one_is_refused(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("environment variables", cx);
    h.press("enter", cx);
    h.press("enter", cx);
    h.type_text("EDITOR=nvim", cx);
    h.press("enter", cx);
    assert_eq!(
        cx.update(|cx| settings::list(cx, "terminal_env")),
        ["EDITOR=nvim"]
    );
    h.press("ctrl-shift-p", cx);
    h.type_text("environment variables", cx);
    h.press("enter", cx);
    h.press("enter", cx);
    h.type_text("nonsense", cx);
    h.press("enter", cx);
    assert_eq!(
        cx.update(|cx| settings::list(cx, "terminal_env")),
        ["EDITOR=nvim"]
    );
    assert!(h.status().contains("NAME=value"), "{}", h.status());
}

#[gpui_kit::test]
fn a_font_family_that_does_not_exist_is_refused_and_a_bundled_one_is_kept(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("font family", cx);
    h.press("enter", cx);
    h.type_text("Nonexistent Mono", cx);
    h.press("enter", cx);
    assert_eq!(
        cx.update(|cx| settings::text(cx, "terminal_font_family")),
        ""
    );
    assert!(h.status().contains("no font family"), "{}", h.status());

    h.press("ctrl-shift-p", cx);
    h.type_text("font family", cx);
    h.press("enter", cx);
    h.type_text("JetBrains Mono", cx);
    h.press("enter", cx);
    assert_eq!(
        cx.update(|cx| settings::text(cx, "terminal_font_family")),
        "JetBrains Mono"
    );
}

#[gpui_kit::test]
fn open_settings_json_creates_the_file_and_asks_the_editor_to_open_it(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    let file = dir.path().join(settings::FILE_NAME);
    assert!(!file.exists());
    h.press("ctrl-shift-p", cx);
    h.type_text("open settings.json", cx);
    h.press("enter", cx);
    assert!(file.exists());
    assert_eq!(*h.opened.borrow(), [file]);
}

#[gpui_kit::test]
fn reveal_settings_folder_shows_the_folder_of_the_file(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("reveal settings folder", cx);
    h.press("enter", cx);
    assert_eq!(*h.revealed.borrow(), [dir.path().to_path_buf()]);
}

#[gpui_kit::test]
fn an_edit_of_the_file_applies_live_and_unusable_values_are_reported(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = window(cx, &dir);
    let file = dir.path().join(settings::FILE_NAME);
    std::fs::write(
        &file,
        br#"{"terminal_scrollback": 2000, "terminal_cursor": "star"}"#,
    )
    .unwrap();
    for _ in 0..2 {
        cx.update(|cx| h.shell.update(cx, |shell, cx| shell.poll_settings(cx)));
    }
    assert_eq!(int(cx, "terminal_scrollback"), 2000);
    assert_eq!(
        cx.update(|cx| settings::text(cx, "terminal_cursor")),
        "block"
    );
    assert!(h.status().contains("terminal_cursor"), "{}", h.status());
}

#[gpui_kit::test]
fn a_file_with_a_bad_value_opens_with_that_key_defaulted_and_says_so(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(settings::FILE_NAME),
        br#"{"terminal_font_size": "huge", "theme": "light"}"#,
    )
    .unwrap();
    let h = window(cx, &dir);
    assert_eq!(int(cx, "terminal_font_size"), 13);
    // (The window of the tests is started with `--theme dark`: the file's own
    // value is what is kept.)
    assert_eq!(
        cx.update(|cx| settings::get(cx).theme),
        AppearanceChoice::Light
    );
    assert!(h.status().contains("terminal_font_size"), "{}", h.status());
}

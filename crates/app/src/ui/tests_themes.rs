//! Tests of themes from files in the window: the palette lists them, previews
//! and applies them, the folder is followed live, and the commands that write
//! theme files. Every folder is a temporary one; the real configuration
//! directory is never read.

use super::*;
use crate::theme::{self, registry, user, Appearance, ThemeId};

const OCEAN: &str =
    "id = \"ocean\"\nname = \"Ocean\"\nextends = \"leon\"\n[dark]\naccent = \"#4DA3FF\"\n";
const BROKEN: &str =
    "id = \"broken\"\nname = \"Broken\"\nextends = \"leon\"\n[dark]\ntext = \"#202020\"\n";

/// A data directory with a themes folder holding `files`, read the way the
/// application reads it at start.
fn data_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let themes = dir.path().join("themes");
    std::fs::create_dir_all(&themes).unwrap();
    for (name, text) in files {
        std::fs::write(themes.join(name), text).unwrap();
    }
    registry::clear_user();
    user::load_folder(&themes, &|name| ["Inter", "JetBrains Mono"].contains(&name));
    dir
}

fn window(cx: &mut TestAppContext, dir: &tempfile::TempDir) -> Harness {
    open_with(
        cx,
        ScriptedRunner::new(),
        Some(dir.path().join(settings::FILE_NAME)),
    )
}

fn colour_of(cx: &mut TestAppContext) -> (u8, u8, u8) {
    let accent = cx.update(|cx| theme::palette(cx).signal);
    let c = gpui_kit::Rgba::from(accent);
    (
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    )
}

#[gpui_kit::test]
fn the_palette_lists_user_themes_marked_as_such_and_invalid_ones_with_their_first_reason(
    cx: &mut TestAppContext,
) {
    let dir = data_dir(&[("ocean.toml", OCEAN), ("broken.toml", BROKEN)]);
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("choose theme", cx);
    h.press("enter", cx);
    let rows = h.shell(cx, |s| {
        match s.palette.flow.as_ref().map(|f| &f.step.kind) {
            Some(StepKind::Choices { choices, .. }) => choices
                .iter()
                .map(|c| (c.label.clone(), c.detail.clone(), c.swatch.is_some()))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        }
    });
    let ocean = rows
        .iter()
        .find(|r| r.0 == "Ocean")
        .expect("Ocean is listed");
    assert!(ocean.1.starts_with("user theme"), "{ocean:?}");
    assert!(ocean.2, "it has a swatch");
    let broken = rows
        .iter()
        .find(|r| r.0 == "Broken")
        .expect("Broken is listed");
    assert!(broken.1.starts_with("invalid: dark.text"), "{broken:?}");
    assert!(!broken.2, "an invalid theme has no swatch");
    // The built-in ones come first.
    assert_eq!(rows[0].0, "Leon");
    assert_eq!(rows[1].0, "Zavu");
}

#[gpui_kit::test]
fn moving_onto_a_user_theme_previews_it_and_enter_keeps_and_saves_it(cx: &mut TestAppContext) {
    let dir = data_dir(&[("ocean.toml", OCEAN)]);
    let h = window(cx, &dir);
    assert_eq!(colour_of(cx), (0xFF, 0xEA, 0x00), "Leon's yellow");
    h.press("ctrl-shift-p", cx);
    h.type_text("theme: ocean", cx);
    h.press("down", cx);
    h.press("up", cx);
    // The row under the selection is on screen, previewed.
    assert_eq!(cx.update(|cx| theme::current(cx)).slug(), "ocean");
    assert_eq!(
        cx.update(|cx| settings::get(cx).theme_id),
        ThemeId::Leon,
        "not kept yet"
    );
    h.press("enter", cx);
    assert_eq!(cx.update(|cx| settings::get(cx).theme_id).slug(), "ocean");
    assert_eq!(colour_of(cx), (0x4D, 0xA3, 0xFF));
    let saved = std::fs::read_to_string(dir.path().join(settings::FILE_NAME)).unwrap();
    assert!(saved.contains("\"ocean\""), "{saved}");
}

#[gpui_kit::test]
fn choosing_a_user_theme_in_the_theme_step_applies_it(cx: &mut TestAppContext) {
    let dir = data_dir(&[("ocean.toml", OCEAN)]);
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("choose theme", cx);
    h.press("enter", cx);
    h.type_text("ocean", cx);
    // The selection previews it...
    assert_eq!(colour_of(cx), (0x4D, 0xA3, 0xFF));
    h.press("enter", cx);
    assert_eq!(cx.update(|cx| settings::kept_theme_id(cx)).slug(), "ocean");
}

#[gpui_kit::test]
fn an_invalid_theme_cannot_be_chosen_and_the_status_line_says_why(cx: &mut TestAppContext) {
    let dir = data_dir(&[("broken.toml", BROKEN)]);
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("theme: broken", cx);
    h.press("enter", cx);
    assert_eq!(cx.update(|cx| settings::kept_theme_id(cx)), ThemeId::Leon);
    assert!(
        h.status().contains("invalid") && h.status().contains("dark.text"),
        "{}",
        h.status()
    );
}

#[gpui_kit::test]
fn the_cycle_chords_step_through_user_themes_too(cx: &mut TestAppContext) {
    let dir = data_dir(&[("ocean.toml", OCEAN)]);
    let h = window(cx, &dir);
    h.press("ctrl-shift-j", cx); // Zavu
    h.press("ctrl-shift-j", cx); // Ocean
    assert_eq!(cx.update(|cx| settings::kept_theme_id(cx)).slug(), "ocean");
    assert!(h.status().contains("Ocean"), "{}", h.status());
    h.press("ctrl-shift-j", cx);
    assert_eq!(
        cx.update(|cx| settings::kept_theme_id(cx)),
        ThemeId::Leon,
        "wraps"
    );
}

#[gpui_kit::test]
fn a_saved_theme_whose_file_is_gone_falls_back_to_the_default_with_a_status_message(
    cx: &mut TestAppContext,
) {
    let dir = data_dir(&[]);
    std::fs::write(
        dir.path().join(settings::FILE_NAME),
        r#"{"theme":"dark","theme_id":"vanished","interface_scale":100}"#,
    )
    .unwrap();
    // The settings are read when the window's application starts.
    let h = window(cx, &dir);
    assert_eq!(cx.update(|cx| settings::kept_theme_id(cx)), ThemeId::Leon);
    assert!(
        h.status().contains("vanished") && h.status().contains("not installed"),
        "{}",
        h.status()
    );
}

#[gpui_kit::test]
fn saving_the_file_of_the_theme_in_use_puts_it_on_screen_without_a_restart(
    cx: &mut TestAppContext,
) {
    let dir = data_dir(&[("ocean.toml", OCEAN)]);
    let h = window(cx, &dir);
    cx.update(|cx| settings::set_theme_id(cx, registry::id_of("ocean")));
    assert_eq!(colour_of(cx), (0x4D, 0xA3, 0xFF));
    let file = dir.path().join("themes").join("ocean.toml");
    let poll = |h: &Harness, cx: &mut TestAppContext| {
        cx.update(|cx| h.shell.update(cx, |s, cx| s.poll_themes(cx)));
        h.settle(cx);
    };
    // An edit: noticed once two listings agree, not at the first.
    std::fs::write(&file, OCEAN.replace("#4DA3FF", "#FF8040")).unwrap();
    poll(&h, cx);
    assert_eq!(
        colour_of(cx),
        (0x4D, 0xA3, 0xFF),
        "not yet: it may still be being written"
    );
    poll(&h, cx);
    assert_eq!(
        colour_of(cx),
        (0xFF, 0x80, 0x40),
        "the window follows the file"
    );
    // A broken save keeps what was on screen and reports it.
    std::fs::write(&file, OCEAN.replace("#4DA3FF", "not a colour at all")).unwrap();
    poll(&h, cx);
    poll(&h, cx);
    assert_eq!(
        colour_of(cx),
        (0xFF, 0x80, 0x40),
        "the last good version stays"
    );
    assert_eq!(h.engine.status().unwrap().kind, StatusKind::Error);
    assert!(
        h.status().contains("Ocean") && h.status().contains("dark.accent"),
        "{}",
        h.status()
    );
    // Fixed.
    std::fs::write(&file, OCEAN.replace("#4DA3FF", "#B070FF")).unwrap();
    poll(&h, cx);
    poll(&h, cx);
    assert_eq!(colour_of(cx), (0xB0, 0x70, 0xFF));
}

#[gpui_kit::test]
fn a_new_file_in_the_folder_appears_in_the_palette_after_the_next_listings(
    cx: &mut TestAppContext,
) {
    let dir = data_dir(&[]);
    let h = window(cx, &dir);
    std::fs::write(
        dir.path().join("themes").join("late.toml"),
        "id = \"late\"\nname = \"Late\"\nextends = \"zavu\"\n",
    )
    .unwrap();
    for _ in 0..2 {
        cx.update(|cx| h.shell.update(cx, |s, cx| s.poll_themes(cx)));
    }
    assert!(registry::all().iter().any(|id| id.slug() == "late"));
}

#[gpui_kit::test]
fn new_theme_from_current_asks_for_a_name_writes_a_commented_file_and_reveals_it(
    cx: &mut TestAppContext,
) {
    let dir = data_dir(&[]);
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("new theme from", cx);
    h.press("enter", cx);
    h.type_text("My Night", cx);
    h.press("enter", cx);
    let file = dir.path().join("themes").join("my-night.toml");
    let text = std::fs::read_to_string(&file).expect("the file was written");
    assert!(
        text.contains("id = \"my-night\"") && text.contains("name = \"My Night\""),
        "{text}"
    );
    assert!(text.contains("extends = \"leon\""));
    assert!(
        text.contains("# accent = \"#FFEA00\""),
        "every token, commented"
    );
    assert_eq!(
        h.revealed.borrow().as_slice(),
        std::slice::from_ref(&file),
        "shown in the file manager"
    );
    assert!(h.status().contains("Created"), "{}", h.status());
    // It is a theme at once, equal to the one it extends.
    let id = ThemeId::parse("my-night").expect("listed");
    assert!(id.is_usable());
    assert_eq!(id.theme().shape, ThemeId::Leon.theme().shape);
}

#[gpui_kit::test]
fn new_theme_refuses_a_built_in_id_and_a_name_that_exists(cx: &mut TestAppContext) {
    let dir = data_dir(&[("ocean.toml", OCEAN)]);
    let h = window(cx, &dir);
    for name in ["Leon", "Ocean"] {
        h.press("ctrl-shift-p", cx);
        h.type_text("new theme from", cx);
        h.press("enter", cx);
        h.type_text(name, cx);
        h.press("enter", cx);
        assert_eq!(
            h.engine.status().unwrap().kind,
            StatusKind::Error,
            "{name}: {}",
            h.status()
        );
    }
    let original = std::fs::read_to_string(dir.path().join("themes").join("ocean.toml")).unwrap();
    assert_eq!(original, OCEAN, "nothing was overwritten");
    assert!(!dir.path().join("themes").join("leon.toml").exists());
}

#[gpui_kit::test]
fn export_current_theme_writes_a_standalone_file_that_loads_back_the_same(cx: &mut TestAppContext) {
    let dir = data_dir(&[]);
    let h = window(cx, &dir);
    cx.update(|cx| settings::set_theme_id(cx, ThemeId::Zavu));
    h.press("ctrl-shift-p", cx);
    h.type_text("export current theme", cx);
    h.press("enter", cx);
    let file = dir.path().join("themes").join("zavu-copy.toml");
    let text = std::fs::read_to_string(&file).expect("exported");
    assert!(!text.contains("extends"));
    let id = ThemeId::parse("zavu-copy").expect("listed");
    assert!(id.is_usable(), "{:?}", id.problem());
    assert_eq!(
        id.palette(Appearance::Dark).terminal.cursor,
        ThemeId::Zavu.palette(Appearance::Dark).terminal.cursor
    );
    // A second export does not overwrite the first.
    h.press("ctrl-shift-p", cx);
    h.type_text("export current theme", cx);
    h.press("enter", cx);
    assert!(dir.path().join("themes").join("zavu-copy-2.toml").exists());
}

#[gpui_kit::test]
fn open_themes_folder_creates_it_and_reveals_it_and_reload_themes_reads_it(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    registry::clear_user();
    let h = window(cx, &dir);
    assert!(!dir.path().join("themes").exists());
    h.press("ctrl-shift-p", cx);
    h.type_text("open themes folder", cx);
    h.press("enter", cx);
    assert!(dir.path().join("themes").is_dir());
    assert_eq!(h.revealed.borrow().as_slice(), [dir.path().join("themes")]);
    std::fs::write(
        dir.path().join("themes").join("x.toml"),
        "id = \"xray\"\nextends = \"leon\"\n",
    )
    .unwrap();
    h.press("ctrl-shift-p", cx);
    h.type_text("reload themes", cx);
    h.press("enter", cx);
    assert!(registry::all().iter().any(|id| id.slug() == "xray"));
    assert!(h.status().starts_with("Reloaded 1 theme"), "{}", h.status());
}

#[gpui_kit::test]
fn show_theme_problems_lists_each_finding_with_what_was_measured_and_required(
    cx: &mut TestAppContext,
) {
    let dir = data_dir(&[
        ("broken.toml", BROKEN),
        ("leon.toml", "id = \"leon\"\nextends = \"zavu\"\n"),
        (
            "typo.toml",
            "id = \"typo\"\nextends = \"leon\"\n[dark]\naccnt = \"#FFFFFF\"\n",
        ),
    ]);
    let h = window(cx, &dir);
    h.press("ctrl-shift-p", cx);
    h.type_text("show theme problems", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Problems);
    assert!(h.shows("theme-problems", cx));
    let listed = registry::problems();
    assert!(listed
        .iter()
        .any(|p| p.file == "broken.toml" && p.key == "dark.text" && p.measured.is_some()));
    assert!(listed
        .iter()
        .any(|p| p.file == "leon.toml" && p.summary().contains("built-in")));
    assert!(listed
        .iter()
        .any(|p| p.file == "typo.toml" && p.summary().contains("did you mean `accent`")));
    for index in 0..listed.len() {
        assert!(
            h.shows_dynamic(format!("problem-{index}"), cx),
            "problem {index} is drawn"
        );
    }
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    // With nothing wrong, it says so.
    let clean = data_dir(&[("ocean.toml", OCEAN)]);
    let _ = clean;
    registry::clear_user();
    h.press("ctrl-shift-p", cx);
    h.type_text("show theme problems", cx);
    h.press("enter", cx);
    assert!(h.shows("problems-none", cx));
}

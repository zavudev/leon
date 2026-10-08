//! Tests of the Den's editor in the window: its buttons and its keys, that
//! a change to a built-in den makes a den of the user's in a file beside
//! the settings, and that dens are chosen, saved, renamed and deleted.
//!
//! No terminal is needed: the room is edited whether or not anybody is in
//! it. The settings are a file in a folder of the test's, so that the dens
//! have a folder beside it.

use super::*;
use crate::schema::{self, Value};
use leon_den::editor::Brush;
use leon_den::layout::Placed;
use leon_den::prefabs::{default_layout, prefab};
use leon_den::DenLayout;

fn den(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let h = open_with(
        cx,
        ScriptedRunner::new(),
        Some(dir.path().join("settings.json")),
    );
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    assert_eq!(h.main_kind(cx), "den");
    (h, dir)
}

fn layout(h: &Harness, cx: &mut TestAppContext) -> DenLayout {
    cx.update(|cx| {
        let view = h.shell.read(cx).den.view.clone().expect("the Den is open");
        let layout = view.read(cx).layout();
        layout
    })
}

fn editing(h: &Harness, cx: &mut TestAppContext) -> bool {
    cx.update(|cx| h.shell.read(cx).den_editing(cx))
}

fn editor<R>(
    h: &Harness,
    cx: &mut TestAppContext,
    read: impl FnOnce(&leon_den::editor::Editor) -> R,
) -> R {
    cx.update(|cx| {
        let view = h.shell.read(cx).den.view.clone().expect("the Den is open");
        let read = view.read(cx).editor(read).expect("the editor is open");
        read
    })
}

fn said(h: &Harness, cx: &mut TestAppContext) -> String {
    // What the box says is decided when the room is painted.
    assert!(h.shows("den", cx));
    cx.update(|cx| {
        let view = h.shell.read(cx).den.view.clone().expect("the Den is open");
        let refused = view.read(cx).refusal();
        refused.map(|why| why.text().to_owned()).unwrap_or_default()
    })
}

fn den_setting(cx: &mut TestAppContext) -> String {
    cx.update(|cx| settings::text(cx, "den"))
}

fn active(h: &Harness, cx: &mut TestAppContext) -> (String, String, bool) {
    h.shell(cx, |shell| {
        let active = shell.den.active.clone().expect("a den is in use");
        (active.id, active.name, active.user)
    })
}

fn click(h: &Harness, selector: &str, cx: &mut TestAppContext) {
    h.mouse_on(selector.to_owned(), gpui_kit::MouseButton::Left, cx);
}

/// Clicks the middle of a tile of the room.
fn click_tile(h: &Harness, x: i32, y: i32, cx: &mut TestAppContext) {
    assert!(h.shows("den", cx), "the room is painted");
    let at = cx
        .update(|cx| {
            let view = h.shell.read(cx).den.view.clone().expect("the Den is open");
            let at = view.read(cx).window_point(x * 16 + 8, y * 16 + 8);
            at
        })
        .expect("the room was painted");
    let mut visual = VisualTestContext::from_window(h.window.into(), cx);
    visual.simulate_click(at, gpui_kit::Modifiers::none());
    visual.run_until_parked();
    h.settle(cx);
}

fn read_den(dir: &tempfile::TempDir, id: &str) -> Option<DenLayout> {
    let text = std::fs::read_to_string(dir.path().join("dens").join(format!("{id}.json"))).ok()?;
    let loaded = DenLayout::load(&text, &default_layout());
    assert_eq!(
        loaded.notes,
        Vec::<String>::new(),
        "a den Leon wrote is right"
    );
    Some(loaded.layout)
}

#[gpui_kit::test]
fn the_den_says_which_den_it_is_and_e_opens_its_editor(cx: &mut TestAppContext) {
    let (h, _dir) = den(cx);
    assert!(h.shows("den-bar", cx));
    assert!(h.shows("den-edit", cx) && h.shows("den-choose", cx));
    assert!(!h.shows("den-strip", cx) && !h.shows("den-done", cx));
    assert_eq!(
        active(&h, cx),
        ("office".to_owned(), "The office".to_owned(), false),
        "nobody chose a den: it is the office"
    );
    assert_eq!(layout(&h, cx), default_layout());

    h.press("e", cx);
    assert!(editing(&h, cx));
    assert!(h.shows("den-strip", cx) && h.shows("den-done", cx));
    assert!(!h.shows("den-edit", cx));
    // The strip starts on the first category of the catalogue.
    assert!(h.shows("den-piece-desk", cx) && h.shows("den-tab-floors", cx));
    assert_eq!(h.main_kind(cx), "den");

    // Escape leaves the editor, and only then the Den.
    h.press("escape", cx);
    assert!(!editing(&h, cx));
    assert_eq!(h.main_kind(cx), "den");
    h.press("escape", cx);
    assert_eq!(h.main_kind(cx), "empty");
}

#[gpui_kit::test]
fn the_command_opens_the_den_and_its_editor_and_done_leaves_it(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = open_with(
        cx,
        ScriptedRunner::new(),
        Some(dir.path().join("settings.json")),
    );
    h.press("ctrl-shift-p", cx);
    h.type_text("edit the den", cx);
    assert_eq!(h.palette_titles(cx)[1], "Edit the Den");
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "den");
    assert!(editing(&h, cx));
    // "furniture" finds it too.
    click(&h, "den-done", cx);
    assert!(!editing(&h, cx));
    h.press("ctrl-shift-p", cx);
    h.type_text("furniture", cx);
    assert_eq!(h.palette_titles(cx)[1], "Edit the Den");
    h.press("enter", cx);
    assert!(editing(&h, cx));
    // Closing the Den closes the editor with it.
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    assert_eq!(h.main_kind(cx), "empty");
    assert!(!editing(&h, cx));
}

#[gpui_kit::test]
fn a_piece_is_picked_from_the_strip_and_put_down_with_a_click(cx: &mut TestAppContext) {
    let (h, dir) = den(cx);
    click(&h, "den-edit", cx);
    assert!(editing(&h, cx));
    click(&h, "den-tab-decor", cx);
    assert!(h.shows("den-piece-pot", cx) && !h.shows("den-piece-desk", cx));
    click(&h, "den-piece-pot", cx);
    assert_eq!(
        editor(&h, cx, |editor| editor.brush().clone()),
        Brush::Piece { id: "pot", turn: 0 }
    );
    assert!(read_den(&dir, "my-office").is_none(), "nothing changed yet");

    click_tile(&h, 5, 8, cx);
    let room = layout(&h, cx);
    assert_eq!(room.items.last(), Some(&Placed::new("pot", 5, 8)));
    // The office is built in: the change made a den of the user's from it,
    // in a file beside the settings, and that is the den in use now.
    assert_eq!(
        active(&h, cx),
        ("my-office".to_owned(), "My office".to_owned(), true)
    );
    assert_eq!(den_setting(cx), "my-office");
    let kept = read_den(&dir, "my-office").expect("the den was written");
    assert_eq!(kept.name, "My office");
    assert_eq!(kept.based_on.as_deref(), Some("office"));
    assert_eq!(kept.items, room.items);
    assert!(
        h.status().starts_with("My office is yours now: "),
        "{}",
        h.status()
    );

    // Where it does not fit it is refused, and said.
    click_tile(&h, 5, 8, cx);
    assert_eq!(layout(&h, cx).items.len(), room.items.len());
    assert_eq!(said(&h, cx), "Something is in the way.");

    // Undo takes it back, in the room and in the file.
    click(&h, "den-undo", cx);
    assert_eq!(layout(&h, cx).items, default_layout().items);
    assert_eq!(
        read_den(&dir, "my-office").unwrap().items,
        default_layout().items
    );
    click(&h, "den-redo", cx);
    assert_eq!(read_den(&dir, "my-office").unwrap().items, room.items);

    // The same piece again puts it down: a click then selects.
    click(&h, "den-piece-pot", cx);
    assert_eq!(editor(&h, cx, |editor| editor.brush().clone()), Brush::Hand);
    click_tile(&h, 5, 8, cx);
    assert_eq!(
        editor(&h, cx, |editor| editor.selected()),
        Some(room.items.len() - 1)
    );
    click(&h, "den-delete", cx);
    assert_eq!(layout(&h, cx).items, default_layout().items);

    // "Reset to prefab" is the office again, and one step back.
    click(&h, "den-wall", cx);
    assert_ne!(layout(&h, cx).wall, "rock");
    click(&h, "den-reset", cx);
    assert_eq!(layout(&h, cx).wall, "rock");
    assert_eq!(read_den(&dir, "my-office").unwrap().name, "My office");
    click(&h, "den-undo", cx);
    assert_ne!(layout(&h, cx).wall, "rock");
}

#[gpui_kit::test]
fn the_room_is_edited_from_the_keyboard_alone(cx: &mut TestAppContext) {
    let (h, dir) = den(cx);
    h.press("e", cx);
    // The next thing of the strip in hand: a desk, the first table.
    h.press(".", cx);
    assert_eq!(
        editor(&h, cx, |editor| editor.brush().clone()),
        Brush::Piece {
            id: "desk",
            turn: 0
        }
    );
    h.press(",", cx);
    h.press(".", cx);
    assert_eq!(
        editor(&h, cx, |editor| editor.brush().clone()),
        Brush::Piece {
            id: "desk",
            turn: 0
        }
    );
    // The keyboard's pointer starts in the middle of the room, where a
    // desk stands already.
    h.press("down", cx);
    h.press("up", cx);
    assert_eq!(
        editor(&h, cx, |editor| editor.pointer()),
        Some(leon_den::world::Tile::new(7, 5))
    );
    let pieces = default_layout().items.len();
    h.press("enter", cx);
    assert_eq!(layout(&h, cx).items.len(), pieces, "refused");
    assert_eq!(said(&h, cx), "Something is in the way.");
    for _ in 0..3 {
        h.press("down", cx);
    }
    h.press("enter", cx);
    assert_eq!(
        layout(&h, cx).items.last(),
        Some(&Placed::new("desk", 7, 7))
    );
    assert_eq!(said(&h, cx), "", "an accepted change clears what was said");
    assert_eq!(read_den(&dir, "my-office").unwrap().items.len(), pieces + 1);

    // Escape puts the piece in hand down; Enter then takes what is there.
    h.press("escape", cx);
    assert!(editing(&h, cx));
    assert_eq!(editor(&h, cx, |editor| editor.brush().clone()), Brush::Hand);
    h.press("space", cx);
    assert_eq!(editor(&h, cx, |editor| editor.selected()), Some(pieces));
    // The arrows move the selected piece, as far as it may go.
    h.press("left", cx);
    assert_eq!(layout(&h, cx).items[pieces], Placed::new("desk", 6, 7));
    h.press("right", cx);
    h.press("right", cx);
    assert_eq!(
        layout(&h, cx).items[pieces],
        Placed::new("desk", 7, 7),
        "the bin is in the way"
    );
    assert_eq!(said(&h, cx), "Something is in the way.");
    // Undo and redo.
    h.press("ctrl-z", cx);
    assert_eq!(layout(&h, cx).items[pieces], Placed::new("desk", 6, 7));
    h.press("ctrl-shift-z", cx);
    assert_eq!(layout(&h, cx).items[pieces], Placed::new("desk", 7, 7));
    // Delete removes what is selected.
    h.press("space", cx);
    h.press("delete", cx);
    assert_eq!(layout(&h, cx).items.len(), pieces);
    // Escape with a selection lets go of it first.
    h.press("space", cx);
    h.press("up", cx);
    h.press("up", cx);
    h.press("up", cx);
    h.press("space", cx);
    assert!(editor(&h, cx, |editor| editor.selected()).is_some());
    h.press("escape", cx);
    assert!(editing(&h, cx));
    assert_eq!(editor(&h, cx, |editor| editor.selected()), None);

    // Shift and an arrow move a wall; F and W change the floor and walls.
    h.press("shift-right", cx);
    h.press("shift-down", cx);
    assert_eq!((layout(&h, cx).cols, layout(&h, cx).rows), (15, 12));
    h.press("shift-left", cx);
    h.press("shift-up", cx);
    assert_eq!((layout(&h, cx).cols, layout(&h, cx).rows), (14, 11));
    h.press("shift-left", cx);
    assert_eq!(layout(&h, cx).cols, 14, "not onto the racks");
    assert_eq!(said(&h, cx), "It does not fit in the room there.");
    h.press("f", cx);
    h.press("w", cx);
    assert_eq!(
        (layout(&h, cx).floor.as_str(), layout(&h, cx).wall.as_str()),
        ("walnut", "navy")
    );
    h.press("shift-f", cx);
    assert_eq!(layout(&h, cx).floor, "wood");

    // Tab goes through the strips and around.
    let tabs = crate::ui::den_edit::Tab::all();
    assert_eq!(h.shell(cx, |shell| shell.den.tab), tabs[0]);
    h.press("tab", cx);
    assert_eq!(h.shell(cx, |shell| shell.den.tab), tabs[1]);
    h.press("shift-tab", cx);
    h.press("shift-tab", cx);
    assert_eq!(
        h.shell(cx, |shell| shell.den.tab),
        crate::ui::den_edit::Tab::Dens
    );
    h.press("shift-tab", cx);
    // On the floors, `.` takes a carpet in hand and Enter lays it.
    assert!(h.shows("den-carpet-red", cx) && h.shows("den-carpet-none", cx));
    h.press(".", cx);
    assert_eq!(
        editor(&h, cx, |editor| editor.brush().clone()),
        Brush::Carpet("wood")
    );
    h.press(".", cx);
    h.press("enter", cx);
    let room = layout(&h, cx);
    let under = editor(&h, cx, |editor| editor.pointer()).unwrap();
    assert_eq!(
        room.floor_at(under),
        leon_den::assets::floor("walnut").unwrap()
    );
    // E is Done.
    h.press("e", cx);
    assert!(!editing(&h, cx));
    assert_eq!(layout(&h, cx), room, "the room stays as it was left");
}

#[gpui_kit::test]
fn a_den_is_chosen_from_pictures_of_the_dens_and_from_the_palette(cx: &mut TestAppContext) {
    let (h, dir) = den(cx);
    click(&h, "den-choose", cx);
    assert!(h.shows("den-strip", cx));
    for prefab in leon_den::prefabs::prefabs() {
        assert!(
            h.shows_dynamic(format!("den-pick-{}", prefab.id), cx),
            "{}",
            prefab.id
        );
    }
    click(&h, "den-pick-nook", cx);
    assert_eq!(layout(&h, cx), prefab("nook").unwrap().layout);
    assert_eq!(
        active(&h, cx),
        ("nook".to_owned(), "The nook".to_owned(), false)
    );
    assert_eq!(den_setting(cx), "nook");
    assert_eq!(h.status(), "Den: The nook.");
    assert!(!dir.path().join("dens").exists(), "choosing writes no den");
    // `.` on that strip is the next den.
    h.press(".", cx);
    assert_eq!(den_setting(cx), "office");
    h.press("escape", cx);

    // The palette lists them by name, the one in use marked.
    h.press("ctrl-shift-p", cx);
    h.type_text("choose a den", cx);
    h.press("enter", cx);
    let titles = h.palette_titles(cx);
    assert!(
        titles.contains(&"The office".to_owned()) && titles.contains(&"The library".to_owned()),
        "{titles:?}"
    );
    h.type_text("library", cx);
    h.press("enter", cx);
    assert_eq!(den_setting(cx), "library");
    assert_eq!(layout(&h, cx), prefab("library").unwrap().layout);

    // The den chosen is the one the Den opens with the next time.
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    assert_eq!(layout(&h, cx).name, "The library");
}

#[gpui_kit::test]
fn a_den_is_saved_under_a_name_renamed_and_deleted(cx: &mut TestAppContext) {
    let (h, dir) = den(cx);
    let dens = dir.path().join("dens");
    // A built-in den has no name to change and cannot be deleted.
    h.press("ctrl-shift-p", cx);
    h.type_text("rename the den", cx);
    h.press("enter", cx);
    assert_eq!(
        h.status(),
        "The office is built in: save it under a name first."
    );
    h.press("escape", cx);
    h.press("ctrl-shift-p", cx);
    h.type_text("delete the den", cx);
    h.press("enter", cx);
    assert_eq!(h.status(), "The office is built in: it cannot be deleted.");
    h.press("escape", cx);

    // Saved under a name it is a den of the user's: a copy of the office.
    h.press("ctrl-shift-p", cx);
    h.type_text("save the den as", cx);
    h.press("enter", cx);
    h.type_text("Attic", cx);
    h.press("enter", cx);
    assert_eq!(
        active(&h, cx),
        ("attic".to_owned(), "Attic".to_owned(), true)
    );
    assert_eq!(den_setting(cx), "attic");
    let kept = read_den(&dir, "attic").unwrap();
    assert_eq!(
        (kept.name.as_str(), kept.based_on.as_deref()),
        ("Attic", Some("office"))
    );
    assert_eq!(kept.items, default_layout().items);
    assert!(
        h.status().starts_with("Saved the den as Attic: "),
        "{}",
        h.status()
    );

    // A change is written to it, not to a new den.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.den_tool(crate::ui::den_edit::Tool::Edit, cx);
            shell.den_tool(crate::ui::den_edit::Tool::Wall(false), cx);
        })
    });
    h.settle(cx);
    assert_eq!(read_den(&dir, "attic").unwrap().wall, "navy");
    assert_eq!(std::fs::read_dir(&dens).unwrap().count(), 1);

    // The same name again is another den.
    cx.update(|cx| {
        h.shell
            .update(cx, |shell, cx| shell.den_save_as("Attic", cx))
    });
    assert_eq!(active(&h, cx).0, "attic-2");
    assert_eq!(read_den(&dir, "attic-2").unwrap().wall, "navy");
    // A name with no letter in it is none.
    cx.update(|cx| {
        h.shell
            .update(cx, |shell, cx| shell.den_save_as(" !! ", cx))
    });
    assert_eq!(h.status(), "Use a name with letters or digits in it.");
    assert_eq!(active(&h, cx).0, "attic-2");

    // Renamed, its file is renamed with it.
    h.press("ctrl-shift-p", cx);
    h.type_text("rename the den", cx);
    h.press("enter", cx);
    h.type_text("Loft", cx);
    h.press("enter", cx);
    assert_eq!(active(&h, cx), ("loft".to_owned(), "Loft".to_owned(), true));
    assert_eq!(den_setting(cx), "loft");
    assert!(dens.join("loft.json").exists() && !dens.join("attic-2.json").exists());
    assert_eq!(read_den(&dir, "loft").unwrap().name, "Loft");
    assert_eq!(h.status(), "Attic is called Loft now.");

    // Deleting asks first, and "keep" keeps.
    h.press("ctrl-shift-p", cx);
    h.type_text("delete the den", cx);
    h.press("enter", cx);
    assert_eq!(h.palette_titles(cx), ["Delete Loft", "Keep it"]);
    h.press("down", cx);
    h.press("enter", cx);
    assert!(dens.join("loft.json").exists());
    h.press("ctrl-shift-p", cx);
    h.type_text("delete the den", cx);
    h.press("enter", cx);
    h.press("enter", cx);
    assert!(!dens.join("loft.json").exists());
    // The den it was made from takes its place; the other den is still there.
    assert_eq!(
        active(&h, cx),
        ("office".to_owned(), "The office".to_owned(), false)
    );
    assert_eq!(den_setting(cx), "office");
    assert_eq!(layout(&h, cx), default_layout());
    assert!(dens.join("attic.json").exists());
    let (dens_listed, current) = cx.update(|cx| h.shell.read(cx).dens(cx));
    assert_eq!(current, "office");
    assert_eq!(
        dens_listed
            .iter()
            .filter(|den| den.user)
            .map(|den| den.name.as_str())
            .collect::<Vec<_>>(),
        ["Attic"]
    );
}

#[gpui_kit::test]
fn the_den_of_the_setting_is_read_from_its_file_when_the_den_opens(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let h = open_with(
        cx,
        ScriptedRunner::new(),
        Some(dir.path().join("settings.json")),
    );
    let dens = dir.path().join("dens");
    std::fs::create_dir_all(&dens).unwrap();
    // A den written by hand, with a mistake in it.
    std::fs::write(
        dens.join("cave.json"),
        r#"{"name": "Cave", "cols": 12, "rows": 9, "floor": "stone", "wall": "moss",
            "items": [{"id": "desk", "x": 4, "y": 4}, {"id": "throne", "x": 2, "y": 2}]}"#,
    )
    .unwrap();
    cx.update(|cx| {
        settings::set_value(cx, schema::find("den").unwrap(), Value::Text("cave".into()));
    });
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    let room = layout(&h, cx);
    assert_eq!((room.name.as_str(), room.cols, room.rows), ("Cave", 12, 9));
    assert_eq!(room.items, [Placed::new("desk", 4, 4)]);
    assert_eq!(active(&h, cx), ("cave".to_owned(), "Cave".to_owned(), true));
    assert_eq!(h.status(), "Den: There is no piece called `throne`.");
    assert!(
        h.shows("den", cx),
        "a den that lacks nearly everything is drawn"
    );

    // Changed outside Leon, it is read again the next time the Den opens.
    h.press("escape", cx);
    std::fs::write(dens.join("cave.json"), "{ this is not json").unwrap();
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    assert_eq!(layout(&h, cx), default_layout(), "the office stands in");
    assert!(
        h.status().starts_with("Den: cave.json: This is not a den"),
        "{}",
        h.status()
    );
    assert_eq!(
        den_setting(cx),
        "cave",
        "the setting is the user's to put right"
    );
    // A den that does not exist, and an id that is no id.
    h.press("escape", cx);
    for missing in ["gone", "../../etc/passwd"] {
        cx.update(|cx| {
            settings::set_value(
                cx,
                schema::find("den").unwrap(),
                Value::Text(missing.into()),
            );
        });
        h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
        assert_eq!(layout(&h, cx), default_layout());
        assert_eq!(
            h.status(),
            format!("Den: There is no den called `{missing}`: The office is used instead.")
        );
        h.press("escape", cx);
    }
}

#[gpui_kit::test]
fn without_a_settings_file_a_changed_den_is_kept_until_the_window_closes(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    h.press("e", cx);
    h.press("w", cx);
    assert_eq!(layout(&h, cx).wall, "navy");
    assert_eq!(
        h.status(),
        "My office is yours until Leon closes: the settings are kept in memory."
    );
    // Closed and opened again, the room is as it was left.
    h.press("escape", cx);
    h.press("escape", cx);
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    assert_eq!(layout(&h, cx).wall, "navy");
    cx.update(|cx| h.shell.update(cx, |shell, cx| shell.open_dens_folder(cx)));
    assert_eq!(
        h.status(),
        "There is no dens folder: the settings are kept in memory."
    );
}

#[gpui_kit::test]
fn the_feed_is_read_from_the_keyboard(cx: &mut TestAppContext) {
    let (h, _dir) = den(cx);
    let view = h.shell(cx, |shell| shell.den.view.clone().unwrap());
    cx.update(|cx| {
        view.update(cx, |den, cx| {
            for n in 0..30 {
                let text = format!("Message {n}.\n\n{}", "It goes on and on. ".repeat(20));
                den.speak(1, "moss", &text, None, cx);
            }
        })
    });
    h.settle(cx);
    assert!(h.shows("den", cx), "painted, so the feed knows its height");
    let feed = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            view.read(cx).read(|den| {
                let feed = den.feed();
                (
                    feed.follows(),
                    feed.cursor(),
                    feed.entries().filter(|e| feed.is_open(e.id)).count(),
                )
            })
        })
    };
    assert_eq!(feed(cx), (true, None, 0));
    // Page Up goes back, End comes back to the end, Home goes to the start.
    h.press("pageup", cx);
    assert!(!feed(cx).0);
    h.press("pagedown", cx);
    assert!(feed(cx).0, "a page down from a page up is the end again");
    h.press("home", cx);
    assert!(!feed(cx).0);
    assert!(h.shows("den", cx));
    let first = cx.update(|cx| view.read(cx).scene(|scene| scene.feed_rows[0].1));
    assert_eq!(first, 0);
    h.press("end", cx);
    assert!(feed(cx).0);
    // Shift and Up walks the entries from the newest; Enter opens the
    // message the keyboard is on, and does not leave the Den.
    h.press("shift-up", cx);
    assert_eq!(feed(cx).1, Some(29));
    h.press("shift-up", cx);
    assert_eq!(feed(cx).1, Some(28));
    h.press("enter", cx);
    assert_eq!(feed(cx).2, 1);
    assert_eq!(h.main_kind(cx), "den");
    h.press("enter", cx);
    assert_eq!(feed(cx).2, 0, "and cuts it again");
    // Escape lets go of the entry first, and only then closes the Den.
    h.press("escape", cx);
    assert_eq!((feed(cx).1, h.main_kind(cx).as_str()), (None, "den"));
    h.press("escape", cx);
    assert_eq!(h.main_kind(cx), "empty");
}

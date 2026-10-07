//! Tests of finding things in files: quick open, the search bar of an editor
//! and the search of the whole project. The folders are real ones in a
//! temporary directory, listed and read through the engine; git is the
//! scripted runner of `tests.rs`, which has no answer for the local file
//! list, so the folder is walked. The remote project is answered by queueing
//! what its machine would print.

use super::live::{open_live, real_worktree, wait_until};
use super::*;
use crate::ui::live::LiveId;
use gpui_kit::MouseButton;
use leon_remote::Output;

/// A project folder with a few files, and the keyboard on its worktree.
fn project(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, String) {
    let h = open_live(cx);
    let (dir, path) = real_worktree(&h, cx);
    for (name, text) in [
        ("README.md", "# hi\nsecond\nthird line\n"),
        ("Cargo.toml", "[package]\n"),
        ("src/main.rs", "fn main() {\n    let needle = 1;\n}\n"),
        ("src/ui/tree.rs", "// the Needle tree\n// needle again\n"),
        ("blob.bin", "\0needle\n"),
    ] {
        let file = std::path::Path::new(&path).join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    (h, dir, path)
}

fn quick_open(h: &Harness, cx: &mut TestAppContext) {
    h.press_chord("cmd-alt-p", "ctrl-shift-alt-p", cx);
}

fn search_project(h: &Harness, cx: &mut TestAppContext) {
    h.press_chord("cmd-alt-shift-f", "ctrl-shift-alt-f", cx);
}

fn rows(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
    h.palette_titles(cx)
}

fn wait_for_row(h: &Harness, cx: &mut TestAppContext, what: &str, prefix: &'static str) {
    wait_until(h, cx, what, move |h, cx| {
        h.palette_titles(cx)
            .iter()
            .any(|row| row.starts_with(prefix))
    });
}

fn opened_file(h: &Harness, cx: &mut TestAppContext) -> Option<(String, u32)> {
    cx.update(|cx| {
        let shell = h.shell.read(cx);
        let id = shell.focused_file()?;
        let doc = shell.files.get(&id)?;
        let line = doc.state()?.read(cx).cursor_position().line;
        Some((doc.path.clone(), line))
    })
}

fn wait_for_file(h: &Harness, cx: &mut TestAppContext) -> (String, u32) {
    wait_until(h, cx, "the file's tab", |h, cx| {
        opened_file(h, cx).is_some()
    });
    opened_file(h, cx).unwrap()
}

// ----- quick open ------------------------------------------------------------------

#[gpui_kit::test]
fn quick_open_lists_the_files_of_the_project_and_narrows_them_as_you_type(cx: &mut TestAppContext) {
    let (h, _dir, _path) = project(cx);
    quick_open(&h, cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    wait_for_row(&h, cx, "the file list", "file:");
    let all = rows(&h, cx);
    assert_eq!(all[0], "[Files]");
    for name in ["README.md", "Cargo.toml", "src/main.rs", "src/ui/tree.rs"] {
        assert!(all.contains(&format!("file:{name}")), "{all:?}");
    }

    h.type_text("mrs", cx);
    let narrowed = rows(&h, cx);
    assert_eq!(narrowed[1], "file:src/main.rs", "{narrowed:?}");
    assert!(!narrowed.contains(&"file:Cargo.toml".to_owned()));

    // The path counts when there is a slash.
    h.press("ctrl-a", cx);
    h.type_text("~ui/tree", cx);
    assert_eq!(rows(&h, cx)[1], "file:src/ui/tree.rs");

    // Nothing matches: a line says so.
    h.type_text("zzzz", cx);
    assert!(rows(&h, cx).contains(&"line:No file matches.".to_owned()));
}

#[gpui_kit::test]
fn enter_opens_the_file_and_a_line_after_the_name_goes_to_that_line(cx: &mut TestAppContext) {
    let (h, _dir, path) = project(cx);
    quick_open(&h, cx);
    wait_for_row(&h, cx, "the file list", "file:");
    h.type_text("readme:3", cx);
    assert_eq!(rows(&h, cx)[1], "file:README.md:3");
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    let (opened, line) = wait_for_file(&h, cx);
    assert_eq!(opened, format!("{path}/README.md"));
    assert_eq!(line, 2, "the cursor is on the third line, from 0");
    assert_eq!(h.shell(cx, |s| s.files.len()), 1);

    // Again: the tab that has it is shown, and goes to the new line.
    quick_open(&h, cx);
    wait_for_row(&h, cx, "the file list", "file:");
    h.type_text("readme:1", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.files.len()), 1);
    assert_eq!(opened_file(&h, cx).unwrap().1, 0);
}

#[gpui_kit::test]
fn escape_closes_quick_open_and_a_click_on_a_row_opens_it(cx: &mut TestAppContext) {
    let (h, _dir, path) = project(cx);
    quick_open(&h, cx);
    wait_for_row(&h, cx, "the file list", "file:");
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(h.shell(cx, |s| s.files.len()), 0);

    quick_open(&h, cx);
    wait_for_row(&h, cx, "the file list", "file:");
    h.type_text("cargo", cx);
    h.mouse_on("palette-item-1".to_owned(), MouseButton::Left, cx);
    assert_eq!(wait_for_file(&h, cx).0, format!("{path}/Cargo.toml"));
}

#[gpui_kit::test]
fn quick_open_is_in_the_palette_and_asks_the_machine_again_each_time_it_opens(
    cx: &mut TestAppContext,
) {
    let (h, _dir, path) = project(cx);
    h.press_chord("cmd-shift-p", "ctrl-shift-p", cx);
    h.type_text("by name", cx);
    assert!(rows(&h, cx).contains(&"Open a file of the project by name\u{2026}".to_owned()));
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    wait_for_row(&h, cx, "the file list", "file:");
    h.press("escape", cx);

    // A file made meanwhile is there the next time, and the old list showed
    // meanwhile.
    std::fs::write(std::path::Path::new(&path).join("fresh.txt"), "x").unwrap();
    quick_open(&h, cx);
    assert!(
        rows(&h, cx).contains(&"file:README.md".to_owned()),
        "the last list is shown at once"
    );
    wait_until(&h, cx, "the list again", |h, cx| {
        rows(h, cx).contains(&"file:fresh.txt".to_owned())
    });
}

#[gpui_kit::test]
fn the_tilde_prefix_reaches_quick_open_from_the_ordinary_palette(cx: &mut TestAppContext) {
    let (h, _dir, _path) = project(cx);
    h.press_chord("cmd-p", "ctrl-p", cx);
    h.type_text("~", cx);
    wait_for_row(&h, cx, "the file list", "file:");
    h.type_text("tree", cx);
    assert_eq!(rows(&h, cx)[1], "file:src/ui/tree.rs");
    // Deleting the prefix goes back to the places.
    for _ in 0..5 {
        h.press("backspace", cx);
    }
    assert!(!rows(&h, cx).iter().any(|row| row.starts_with("file:")));
}

// ----- the search bar of a file ------------------------------------------------------

fn with_file(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, String) {
    let (h, dir, path) = project(cx);
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell's prompt", |h, cx| {
        super::live::terminal_of(h, cx, 1).is_some_and(|t| t.screen_text().contains("READY>"))
    });
    h.press_chord("cmd-shift-o", "ctrl-shift-alt-o", cx);
    h.type_text("README.md", cx);
    h.press("enter", cx);
    wait_for_file(&h, cx);
    (h, dir, path)
}

fn search_bar(h: &Harness, cx: &mut TestAppContext) -> Option<(bool, bool)> {
    cx.update(|cx| {
        let shell = h.shell.read(cx);
        let state = shell.files.get(&LiveId(2))?.state()?;
        let session = state.read(cx).search_session();
        Some((session.open, session.replace_mode))
    })
}

fn bar_has_keyboard(h: &Harness, cx: &mut TestAppContext) -> bool {
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.read(cx).editor_search_has_keyboard(window, cx)
    })
    .unwrap()
}

#[gpui_kit::test]
fn find_opens_the_editors_search_bar_only_while_a_file_has_the_keyboard(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_file(cx);
    assert_eq!(search_bar(&h, cx), Some((false, false)));
    h.press_chord("cmd-f", "ctrl-f", cx);
    assert_eq!(search_bar(&h, cx), Some((true, false)));
    assert!(bar_has_keyboard(&h, cx), "the bar's field has the keyboard");
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);

    // Escape closes the bar (it is the bar's, not "back to the sidebar") and
    // the file keeps the keyboard.
    h.press("escape", cx);
    assert_eq!(search_bar(&h, cx), Some((false, false)));
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    assert!(h.shell(cx, |s| s.file_has_keyboard()));
    // Escape with no bar open is still "back to the sidebar".
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);

    // On the terminal of the same tab the chord is not the editor's.
    h.press_chord("cmd-alt-1", "ctrl-shift-1", cx);
    assert_eq!(h.main_kind(cx), "live:1");
    h.press_chord("cmd-f", "ctrl-f", cx);
    assert_eq!(search_bar(&h, cx), Some((false, false)));
}

#[gpui_kit::test]
fn replace_opens_the_bar_with_the_replacement_field(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_file(cx);
    h.press_chord("cmd-alt-f", "ctrl-h", cx);
    assert_eq!(search_bar(&h, cx), Some((true, true)));
    assert!(bar_has_keyboard(&h, cx));
    // Typing goes to the bar's field, not into the text.
    h.type_text("second", cx);
    assert_eq!(
        cx.update(|cx| h.shell.read(cx).files[&LiveId(2)].text(cx).unwrap()),
        "# hi\nsecond\nthird line\n"
    );
    let found = cx.update(|cx| {
        let shell = h.shell.read(cx);
        let state = shell.files[&LiveId(2)].state().unwrap();
        state.read(cx).search_session().query.clone()
    });
    assert_eq!(found, "second");
}

#[gpui_kit::test]
fn the_find_commands_are_offered_only_while_a_file_is_on_screen(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_file(cx);
    let titles = |h: &Harness, cx: &mut TestAppContext| {
        h.press("ctrl-shift-p", cx);
        h.type_text("in the file", cx);
        let titles = h.palette_titles(cx);
        h.press("escape", cx);
        titles
    };
    let with = titles(&h, cx);
    assert!(
        with.contains(&"Find in the file\u{2026}".to_owned()),
        "{with:?}"
    );
    assert!(
        with.contains(&"Replace in the file\u{2026}".to_owned()),
        "{with:?}"
    );
    h.press_chord("cmd-alt-1", "ctrl-shift-1", cx);
    let without = titles(&h, cx);
    assert!(
        !without
            .iter()
            .any(|t| t.starts_with("Find in the file") || t.starts_with("Replace in the file")),
        "{without:?}"
    );
    // Run from the palette it works too.
    h.press_chord("cmd-alt-2", "ctrl-shift-2", cx);
    h.press("ctrl-shift-p", cx);
    h.type_text("find in the file", cx);
    h.press("enter", cx);
    assert_eq!(search_bar(&h, cx), Some((true, false)));
    assert!(bar_has_keyboard(&h, cx));
}

// ----- project search ------------------------------------------------------------------

#[gpui_kit::test]
fn the_search_lists_the_lines_by_file_and_enter_opens_the_file_at_the_line(
    cx: &mut TestAppContext,
) {
    let (h, _dir, path) = project(cx);
    search_project(&h, cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert!(rows(&h, cx)
        .iter()
        .any(|row| row == "line:Type to search the text of the project's files."));
    h.type_text("needle", cx);
    wait_for_row(&h, cx, "the lines", "match:");
    let found = rows(&h, cx);
    // Both spellings, a binary file not at all, and in the order of the list.
    assert_eq!(
        found,
        [
            "[src/main.rs]",
            "match:src/main.rs:2",
            "[src/ui/tree.rs]",
            "match:src/ui/tree.rs:1",
            "match:src/ui/tree.rs:2",
        ],
        "{found:?}"
    );
    // The first line is where the keyboard is.
    assert_eq!(h.shell(cx, |s| s.palette.cursor), 1);
    h.press("down", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    let (opened, line) = wait_for_file(&h, cx);
    assert_eq!(opened, format!("{path}/src/ui/tree.rs"));
    assert_eq!(line, 0, "the first line of that file, from 0");
}

#[gpui_kit::test]
fn a_click_on_a_line_opens_its_file_with_the_cursor_there(cx: &mut TestAppContext) {
    let (h, _dir, path) = project(cx);
    search_project(&h, cx);
    h.type_text("needle", cx);
    wait_for_row(&h, cx, "the lines", "match:");
    h.mouse_on("palette-item-1".to_owned(), MouseButton::Left, cx);
    let (opened, line) = wait_for_file(&h, cx);
    assert_eq!(opened, format!("{path}/src/main.rs"));
    assert_eq!(line, 1, "the second line, from 0");
}

#[gpui_kit::test]
fn the_case_and_regex_switches_change_what_is_found(cx: &mut TestAppContext) {
    let (h, _dir, _path) = project(cx);
    search_project(&h, cx);
    h.type_text("Needle", cx);
    wait_for_row(&h, cx, "the lines", "match:");
    assert_eq!(
        rows(&h, cx)
            .iter()
            .filter(|r| r.starts_with("match:"))
            .count(),
        3,
        "case does not matter at first"
    );

    // Case matters: the click on the switch and the chord do the same.
    h.mouse_on("search-case".to_owned(), MouseButton::Left, cx);
    assert!(h.shell(cx, |s| s.palette.text.case_sensitive));
    wait_until(&h, cx, "the lines", |h, cx| {
        rows(h, cx)
            .iter()
            .filter(|r| r.starts_with("match:"))
            .count()
            == 1
    });
    assert_eq!(rows(&h, cx)[1], "match:src/ui/tree.rs:1");
    h.press_chord("alt-c", "alt-c", cx);
    assert!(!h.shell(cx, |s| s.palette.text.case_sensitive));

    // A regular expression: the text is a pattern, and a bad one says why.
    h.press("ctrl-a", cx);
    h.type_text("%ne+dle =", cx);
    wait_until(&h, cx, "no line", |h, cx| {
        rows(h, cx).contains(&"line:No matches.".to_owned())
    });
    h.press_chord("alt-r", "alt-r", cx);
    assert!(h.shell(cx, |s| s.palette.text.regex));
    wait_for_row(&h, cx, "the lines", "match:");
    assert_eq!(rows(&h, cx)[1], "match:src/main.rs:2");
    h.press("ctrl-a", cx);
    h.type_text("%(", cx);
    wait_until(&h, cx, "the problem", |h, cx| {
        rows(h, cx).iter().any(|r| r.contains("unclosed group"))
    });
    assert!(!rows(&h, cx).iter().any(|r| r.starts_with("match:")));
}

#[gpui_kit::test]
fn a_new_query_drops_the_lines_of_the_one_before_and_closing_ends_the_search(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _path) = project(cx);
    search_project(&h, cx);
    h.type_text("needle", cx);
    wait_for_row(&h, cx, "the lines", "match:");
    h.type_text("zzz", cx);
    wait_until(&h, cx, "no line", |h, cx| {
        rows(h, cx).contains(&"line:No matches.".to_owned())
    });
    assert!(!rows(&h, cx).iter().any(|r| r.starts_with("match:")));
    h.press("escape", cx);
    assert!(h.shell(cx, |s| s.palette.text.hits.is_empty()));
    assert!(!h.shell(cx, |s| s.palette.text.running));
}

#[gpui_kit::test]
fn the_search_of_a_remote_project_is_one_command_on_its_machine(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let remote = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|machine| !machine.id.is_local())
        .unwrap();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.show(&NodeId::Machine(remote.id.clone()));
        })
    });
    h.settle(cx);
    search_project(&h, cx);
    // What the machine would print for `needle`, queued once the window has
    // asked for everything else it asks.
    h.runner.queue(Output::ok(
        "LEON-FILE 1\nRG\n./src/a.rs\u{0}3:5:  needle here\n./docs/b c.md\u{0}1:1:needle\n",
    ));
    h.type_text("needle", cx);
    wait_for_row(&h, cx, "the lines", "match:");
    assert_eq!(
        rows(&h, cx),
        [
            "[src/a.rs]",
            "match:src/a.rs:3",
            "[docs/b c.md]",
            "match:docs/b c.md:1"
        ]
    );
    let call = h
        .runner
        .calls()
        .into_iter()
        .rev()
        .find(|call| call.program == "ssh")
        .unwrap();
    let line = call.args.last().unwrap();
    assert!(line.contains("' sh /opt/infra needle F i"), "{line}");
    assert!(line.contains("rg --null"), "{line}");

    // Enter reads the file where it is and puts the cursor on the line.
    h.runner.queue(Output::ok(
        "LEON-FILE 1\nTEXT 1 2\nYQpiCm5lZWRsZSBoZXJlCnoK\n",
    ));
    h.press("enter", cx);
    let (opened, line) = wait_for_file(&h, cx);
    assert_eq!(opened, "/opt/infra/src/a.rs");
    assert_eq!(line, 2);
    assert_eq!(
        h.shell(cx, |s| s.files.values().next().unwrap().machine.clone()),
        remote.id
    );
}

#[gpui_kit::test]
fn a_machine_that_does_not_answer_the_search_says_so_in_the_list(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let remote = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|machine| !machine.id.is_local())
        .unwrap();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.show(&NodeId::Machine(remote.id.clone()));
        })
    });
    h.settle(cx);
    search_project(&h, cx);
    h.runner.queue(Output::failed(
        255,
        "ssh: connect to host build.example port 22: Connection refused\n",
    ));
    h.type_text("needle", cx);
    wait_until(&h, cx, "the reason", |h, cx| {
        rows(h, cx).iter().any(|r| r.contains("Connection refused"))
    });
}

#[gpui_kit::test]
fn the_plain_palette_lists_matching_files_too_and_opens_them(cx: &mut TestAppContext) {
    let (h, _dir, path) = project(cx);
    h.press_chord("cmd-p", "ctrl-p", cx);
    h.type_text("cargo.", cx);
    wait_for_row(&h, cx, "the files", "file:");
    let found = rows(&h, cx);
    assert_eq!(found.last().unwrap(), "file:Cargo.toml", "{found:?}");
    assert!(found.contains(&"[Files]".to_owned()));
    h.press("enter", cx);
    assert_eq!(wait_for_file(&h, cx).0, format!("{path}/Cargo.toml"));
}

//! Tests of the file tree panel: showing it, expanding folders, opening a
//! file from it, git's marks and the refresh after a save. The folders are
//! real ones in a temporary directory, listed through the engine; git is the
//! scripted runner of `tests.rs`, so its answers are queued by the tests.

use super::live::{open_live, real_worktree, wait_until};
use super::*;
use gpui_kit::{Modifiers, MouseButton};
use leon_remote::Output;

/// The folder of the worktree: a few files and a folder.
fn project(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, String) {
    let h = open_live(cx);
    let (dir, path) = real_worktree(&h, cx);
    for (name, text) in [
        ("README.md", "# hi\n"),
        ("Cargo.toml", "[package]\n"),
        ("src/main.rs", "fn main() {}\n"),
        ("src/ui/tree.rs", "// tree\n"),
    ] {
        let file = std::path::Path::new(&path).join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    (h, dir, path)
}

fn toggle(h: &Harness, cx: &mut TestAppContext) {
    h.press_chord("cmd-shift-e", "ctrl-shift-alt-e", cx);
}

/// The panel, shown and listed.
fn shown(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, String) {
    let (h, dir, path) = project(cx);
    toggle(&h, cx);
    wait_for_rows(&h, cx, "the root's files");
    (h, dir, path)
}

fn wait_for_rows(h: &Harness, cx: &mut TestAppContext, what: &str) {
    wait_until(h, cx, what, |h, cx| {
        h.shell(cx, |s| {
            s.file_tree_now()
                .is_some_and(|tree| tree.rows.iter().all(|row| row.name != "Loading\u{2026}"))
        })
    });
}

/// The rows, indented by depth, folders ending in a slash.
fn names(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
    h.shell(cx, |s| {
        s.file_tree_now().map_or_else(Vec::new, |tree| {
            tree.rows
                .iter()
                .map(|row| {
                    let slash = if row.kind == super::super::editor::RowKind::Dir {
                        "/"
                    } else {
                        ""
                    };
                    format!("{}{}{slash}", "  ".repeat(usize::from(row.depth)), row.name)
                })
                .collect()
        })
    })
}

fn selected(h: &Harness, cx: &mut TestAppContext) -> Option<String> {
    h.shell(cx, |s| s.file_tree_now().and_then(|t| t.selected.clone()))
}

fn git_status_calls(h: &Harness) -> usize {
    h.runner
        .calls()
        .iter()
        .filter(|call| call.program == "git" && call.args.first().is_some_and(|a| a == "status"))
        .count()
}

#[gpui_kit::test]
fn the_chord_shows_the_files_of_the_worktree_with_the_keyboard_and_hides_them_again(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _path) = project(cx);
    assert!(!h.shows("files-panel", cx), "hidden until asked for");
    assert!(!cx.update(|cx| crate::settings::get(cx).files_visible));

    toggle(&h, cx);
    assert!(cx.update(|cx| crate::settings::get(cx).files_visible));
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Files);
    wait_for_rows(&h, cx, "the files");
    assert!(h.shows("files-panel", cx));
    assert_eq!(names(&h, cx), ["src/", "Cargo.toml", "README.md"]);
    // The column is laid out between the sidebar and the main pane.
    let sidebar = h.bounds_of("sidebar".to_owned(), cx).unwrap();
    let panel = h.bounds_of("files-panel".to_owned(), cx).unwrap();
    assert!(panel.left() >= sidebar.right());
    assert!(h.shows("file-row-0", cx));

    toggle(&h, cx);
    assert!(!h.shows("files-panel", cx));
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    assert!(!cx.update(|cx| crate::settings::get(cx).files_visible));
}

#[gpui_kit::test]
fn the_header_button_and_the_palette_show_and_hide_the_panel_too(cx: &mut TestAppContext) {
    let (h, _dir, _path) = project(cx);
    h.mouse_on("files-toggle".to_owned(), MouseButton::Left, cx);
    assert!(h.shows("files-panel", cx));
    h.press_chord("cmd-shift-p", "ctrl-shift-p", cx);
    h.type_text("file tree", cx);
    assert!(h
        .palette_titles(cx)
        .contains(&"Show or hide the file tree".to_owned()));
    h.press("enter", cx);
    assert!(!h.shows("files-panel", cx));
}

#[gpui_kit::test]
fn the_arrows_open_and_close_folders_lazily_and_enter_opens_a_file_in_a_tab(
    cx: &mut TestAppContext,
) {
    let (h, _dir, path) = shown(cx);
    // Only the root was listed: a folder is asked for when it is opened.
    let listed = |h: &Harness, cx: &mut TestAppContext| {
        h.shell(cx, |s| {
            let mut keys: Vec<String> = s
                .file_tree_now()
                .unwrap()
                .listings
                .keys()
                .cloned()
                .collect();
            keys.sort();
            keys
        })
    };
    assert_eq!(listed(&h, cx), [""]);

    h.press("down", cx);
    assert_eq!(selected(&h, cx).as_deref(), Some("src"));
    h.press("right", cx);
    wait_for_rows(&h, cx, "src");
    assert_eq!(
        names(&h, cx),
        ["src/", "  ui/", "  main.rs", "Cargo.toml", "README.md"]
    );
    assert_eq!(listed(&h, cx), ["", "src"]);

    // Right on an open folder goes into it; Left goes back out, then closes.
    h.press("right", cx);
    assert_eq!(selected(&h, cx).as_deref(), Some("src/ui"));
    h.press("left", cx);
    assert_eq!(selected(&h, cx).as_deref(), Some("src"));
    h.press("left", cx);
    assert_eq!(names(&h, cx), ["src/", "Cargo.toml", "README.md"]);
    // Enter on a folder toggles it, and j/k move like the arrows.
    h.press("enter", cx);
    assert_eq!(names(&h, cx).len(), 5);
    h.press("j", cx);
    h.press("j", cx);
    h.press("j", cx);
    h.press("k", cx);
    assert_eq!(selected(&h, cx).as_deref(), Some("src/main.rs"));

    // Enter on a file opens it in a tab and gives it the keyboard.
    h.press("enter", cx);
    wait_until(&h, cx, "the file's tab", |h, cx| {
        h.shell(cx, |s| s.focused_file().is_some())
    });
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    let opened = h.shell(cx, |s| {
        s.files.values().map(|d| d.path.clone()).collect::<Vec<_>>()
    });
    assert_eq!(opened, [format!("{path}/src/main.rs")]);
    // The panel stays, with the selection where it was.
    assert!(h.shows("files-panel", cx));
    assert_eq!(selected(&h, cx).as_deref(), Some("src/main.rs"));
}

#[gpui_kit::test]
fn escape_gives_the_keyboard_to_the_main_pane_and_tab_visits_the_panel(cx: &mut TestAppContext) {
    let (h, _dir, _path) = shown(cx);
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    assert!(h.shows("files-panel", cx), "Esc does not hide it");

    // The panes go round: sidebar, tree, main.
    h.shell_set_pane(Pane::Sidebar, cx);
    h.press("tab", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Files);
    h.press("tab", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    h.press("tab", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
    h.press("shift-tab", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    h.press("shift-tab", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Files);
}

#[gpui_kit::test]
fn a_click_selects_and_toggles_a_folder_and_a_double_click_opens_a_file(cx: &mut TestAppContext) {
    let (h, _dir, _path) = shown(cx);
    h.shell_set_pane(Pane::Main, cx);
    h.mouse_on("file-row-0".to_owned(), MouseButton::Left, cx);
    assert_eq!(
        h.shell(cx, |s| s.pane),
        Pane::Files,
        "a click takes the keyboard"
    );
    assert_eq!(selected(&h, cx).as_deref(), Some("src"));
    wait_for_rows(&h, cx, "src");
    assert_eq!(names(&h, cx).len(), 5, "the click opened the folder");

    // A single click on a file only selects it.
    let row = names(&h, cx)
        .iter()
        .position(|n| n == "Cargo.toml")
        .unwrap();
    h.mouse_on(format!("file-row-{row}"), MouseButton::Left, cx);
    assert_eq!(selected(&h, cx).as_deref(), Some("Cargo.toml"));
    assert!(h.shell(cx, |s| s.files.is_empty()));

    // The double click opens it.
    let bounds = h.bounds_of(format!("file-row-{row}"), cx).unwrap();
    let mut visual = VisualTestContext::from_window(h.window.into(), cx);
    visual.simulate_mouse_move(bounds.center(), None, Modifiers::none());
    for count in [1, 2] {
        visual.simulate_event(gpui_kit::MouseDownEvent {
            position: bounds.center(),
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: count,
            first_mouse: false,
        });
        visual.simulate_event(gpui_kit::MouseUpEvent {
            position: bounds.center(),
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: count,
        });
    }
    visual.run_until_parked();
    h.settle(cx);
    wait_until(&h, cx, "the file's tab", |h, cx| {
        h.shell(cx, |s| s.focused_file().is_some())
    });
}

#[gpui_kit::test]
fn git_marks_are_drawn_on_files_and_roll_up_to_their_folders(cx: &mut TestAppContext) {
    let (h, _dir, _path) = project(cx);
    h.runner.queue(Output::ok(
        " M src/ui/tree.rs\0?? README.md\0!! Cargo.toml\0",
    ));
    h.runner.queue(Output::ok("\n"));
    toggle(&h, cx);
    wait_until(&h, cx, "the marks", |h, cx| {
        h.shell(cx, |s| {
            s.file_tree_now().is_some_and(|tree| !tree.marks.is_empty())
        })
    });
    let mark = |h: &Harness, cx: &mut TestAppContext, name: &str| {
        h.shell(cx, |s| {
            s.file_tree_now()
                .unwrap()
                .rows
                .iter()
                .find(|row| row.name == name)
                .and_then(|row| row.mark)
        })
    };
    use crate::files::GitMark;
    assert_eq!(
        mark(&h, cx, "src"),
        Some(GitMark::Modified),
        "a folder holds changes"
    );
    assert_eq!(mark(&h, cx, "README.md"), Some(GitMark::Untracked));
    assert_eq!(mark(&h, cx, "Cargo.toml"), Some(GitMark::Ignored));
    // Rows 0 (src) and 2 (README.md) carry a letter or a dot; the ignored
    // file is only dimmed.
    assert!(h.shows("file-mark-0", cx));
    assert!(!h.shows("file-mark-1", cx));
    assert!(h.shows("file-mark-2", cx));
}

#[gpui_kit::test]
fn saving_lists_the_folder_and_asks_git_again(cx: &mut TestAppContext) {
    let (h, _dir, path) = shown(cx);
    let before = git_status_calls(&h);
    assert_eq!(before, 1);

    // Open a file from the tree and change it; a new file appears meanwhile.
    h.press("down", cx);
    h.press("down", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the file's tab", |h, cx| {
        h.shell(cx, |s| s.focused_file().is_some())
    });
    std::fs::write(std::path::Path::new(&path).join("NEW.md"), "new\n").unwrap();
    assert!(
        !names(&h, cx).contains(&"NEW.md".to_owned()),
        "not listed yet"
    );
    h.type_text("x", cx);
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the save", |h, cx| {
        h.shell(cx, |s| s.files.values().all(|doc| !doc.dirty))
    });
    wait_until(&h, cx, "the new file in the tree", |h, cx| {
        names(h, cx).contains(&"NEW.md".to_owned())
    });
    assert!(git_status_calls(&h) > before, "git was asked again");
}

#[gpui_kit::test]
fn what_is_open_and_selected_is_kept_for_each_folder(cx: &mut TestAppContext) {
    let (h, _dir, first) = shown(cx);
    h.press("down", cx);
    h.press("right", cx);
    wait_for_rows(&h, cx, "src");
    assert_eq!(names(&h, cx).len(), 5);

    // Another worktree on the same machine: its own tree.
    let (_other_dir, second) = real_worktree(&h, cx);
    h.shell_set_pane(Pane::Sidebar, cx);
    std::fs::write(std::path::Path::new(&second).join("only-here.txt"), "x").unwrap();
    cx.update(|cx| h.shell.update(cx, |s, cx| s.files_refresh(cx)));
    h.settle(cx);
    wait_until(&h, cx, "the other tree", |h, cx| {
        names(h, cx) == ["only-here.txt"]
    });
    assert!(selected(&h, cx).is_none());

    // Back: the folder is still open and the row still selected.
    let node = h.shell(cx, |s| {
        s.snapshot
            .projects
            .iter()
            .flat_map(|entry| entry.worktrees.iter())
            .find(|worktree| worktree.path == first)
            .map(|worktree| NodeId::Worktree(worktree.id.clone()))
            .unwrap()
    });
    cx.update(|cx| h.shell.update(cx, |s, _| s.show(&node)));
    h.shell_set_pane(Pane::Sidebar, cx);
    h.settle(cx);
    wait_until(&h, cx, "the first tree", |h, cx| names(h, cx).len() == 5);
    assert_eq!(selected(&h, cx).as_deref(), Some("src"));
}

impl Harness {
    fn shell_set_pane(&self, pane: Pane, cx: &mut TestAppContext) {
        cx.update(|cx| {
            self.shell.update(cx, |s, cx| {
                s.pane = pane;
                cx.notify();
            })
        });
        self.settle(cx);
    }
}

//! Tests of what survives a restart (the open files, their cursor and
//! Markdown view, the file tree), of revealing a file in the tree, and of the
//! editor settings applying to files that are open.

use super::super::editor::ViewMode;
use super::live::{real_worktree, real_worktree_in};
use super::*;
use crate::ui::live::LiveId;

/// Waits for `condition`, letting the engine's threads run meanwhile: what
/// is awaited here (a restore, a listing) is answered by them.
fn wait_until(
    h: &Harness,
    cx: &mut TestAppContext,
    what: &str,
    condition: impl Fn(&Harness, &mut TestAppContext) -> bool,
) {
    for _ in 0..200 {
        h.settle(cx);
        if condition(h, cx) {
            return;
        }
    }
    super::live::wait_until(h, cx, what, condition);
}

/// A window whose settings are in `settings`.
fn window_in(cx: &mut TestAppContext, settings: &tempfile::TempDir) -> Harness {
    cx.executor().allow_parking();
    open_with(
        cx,
        ScriptedRunner::new(),
        Some(settings.path().join(crate::settings::FILE_NAME)),
    )
}

fn open_by_chord(h: &Harness, cx: &mut TestAppContext, typed: &str) {
    let before = h.shell(cx, |s| s.files.len());
    h.press_chord("cmd-shift-o", "ctrl-shift-alt-o", cx);
    h.type_text(typed, cx);
    h.press("enter", cx);
    wait_until(h, cx, "the file's tab", |h, cx| {
        h.shell(cx, |s| s.files.len() == before + 1)
    });
}

/// The folder, with a few files, and a window on it.
fn project(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, tempfile::TempDir, String) {
    let settings = tempfile::tempdir().unwrap();
    let h = window_in(cx, &settings);
    let (dir, path) = real_worktree(&h, cx);
    for (name, text) in [
        ("README.md", "# hi\n\none\ntwo\nthree\n"),
        ("a.txt", "a\nb\nc\nd\n"),
        ("src/ui/tree.rs", "// tree\n"),
    ] {
        let file = std::path::Path::new(&path).join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    (h, settings, dir, path)
}

/// The window is closed the way a quit does it.
fn end(h: &Harness, cx: &mut TestAppContext) {
    cx.update(|cx| h.shell.update(cx, |shell, cx| shell.flush(cx)));
}

fn files(h: &Harness, cx: &mut TestAppContext) -> Vec<(String, ViewMode, u32)> {
    cx.update(|cx| {
        let shell = h.shell.read(cx);
        let mut ids: Vec<LiveId> = shell.files.keys().copied().collect();
        ids.sort_by_key(|id| id.0);
        ids.into_iter()
            .map(|id| {
                let doc = &shell.files[&id];
                let line = doc.state().unwrap().read(cx).cursor_position().line;
                (doc.name().to_owned(), doc.mode, line)
            })
            .collect()
    })
}

#[gpui_kit::test]
fn the_open_files_come_back_after_a_restart_with_cursor_and_view(cx: &mut TestAppContext) {
    let (h, settings, _dir, path) = project(cx);
    open_by_chord(&h, cx, "README.md");
    h.press_chord("cmd-alt-v", "ctrl-shift-alt-v", cx);
    assert_eq!(h.shell(cx, |s| s.files[&LiveId(1)].mode), ViewMode::Split);
    open_by_chord(&h, cx, "a.txt");
    // The cursor goes to the third line.
    h.press("down", cx);
    h.press("down", cx);
    end(&h, cx);
    let saved = std::fs::read_to_string(settings.path().join("open_files.json")).unwrap();
    assert!(
        saved.contains("README.md") && saved.contains("a.txt"),
        "{saved}"
    );

    // The next run.
    let second = window_in(cx, &settings);
    wait_until(&second, cx, "both files", |h, cx| {
        h.shell(cx, |s| s.files.len() == 2)
    });
    let restored = files(&second, cx);
    assert_eq!(restored[0].0, "README.md");
    assert_eq!(restored[0].1, ViewMode::Split);
    assert_eq!(restored[1].0, "a.txt");
    assert_eq!(restored[1].1, ViewMode::Edit);
    wait_until(&second, cx, "the cursor", |h, cx| files(h, cx)[1].2 == 2);
    // What was on screen is the tab on screen in its workspace; the main
    // pane is not taken over.
    assert_eq!(second.shell(cx, |s| s.workspaces.all().len()), 1, "{path}");
    assert!(second.shell(cx, |s| s.focused_file().is_none()));
}

#[gpui_kit::test]
fn a_file_that_is_gone_is_skipped_without_a_word(cx: &mut TestAppContext) {
    let (h, settings, _dir, path) = project(cx);
    open_by_chord(&h, cx, "a.txt");
    open_by_chord(&h, cx, "README.md");
    end(&h, cx);
    std::fs::remove_file(std::path::Path::new(&path).join("a.txt")).unwrap();
    let second = window_in(cx, &settings);
    wait_until(&second, cx, "the file that is left", |h, cx| {
        h.shell(cx, |s| s.files.len() == 1 && !s.restoring_files)
    });
    assert_eq!(files(&second, cx)[0].0, "README.md");
    assert_eq!(second.status(), "", "nothing was said");
}

#[gpui_kit::test]
fn a_setting_can_turn_the_reopening_off(cx: &mut TestAppContext) {
    let (h, settings, _dir, _path) = project(cx);
    open_by_chord(&h, cx, "a.txt");
    end(&h, cx);
    std::fs::write(
        settings.path().join(crate::settings::FILE_NAME),
        br#"{"editor_restore_files": false}"#,
    )
    .unwrap();
    let second = window_in(cx, &settings);
    second.settle(cx);
    assert_eq!(second.shell(cx, |s| s.files.len()), 0);
}

#[gpui_kit::test]
fn an_unreadable_or_future_file_of_open_files_is_ignored(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    std::fs::write(settings.path().join("open_files.json"), b"{ nope").unwrap();
    let h = window_in(cx, &settings);
    h.settle(cx);
    assert_eq!(h.shell(cx, |s| s.files.len()), 0);
}

#[gpui_kit::test]
fn the_open_folders_of_the_tree_come_back_after_a_restart(cx: &mut TestAppContext) {
    let (h, settings, dir, _path) = project(cx);
    h.press_chord("cmd-shift-e", "ctrl-shift-alt-e", cx);
    wait_until(&h, cx, "the root's files", |h, cx| {
        h.shell(cx, |s| {
            s.file_tree_now().is_some_and(|t| !t.rows.is_empty())
        })
    });
    // Select `src` and open it.
    h.press("down", cx);
    h.press("right", cx);
    wait_until(&h, cx, "src listed", |h, cx| {
        h.shell(cx, |s| {
            s.file_tree_now().is_some_and(|t| {
                t.open.contains("src") && t.rows.iter().any(|r| r.path == "src/ui")
            })
        })
    });
    end(&h, cx);

    // The same folder in the next run.
    let second = window_in(cx, &settings);
    let _same_folder = real_worktree_in(&second, cx, dir);
    // The panel is still showing: that setting was kept too.
    cx.update(|cx| second.shell.update(cx, |_, cx| cx.notify()));
    for _ in 0..20 {
        second.settle(cx);
    }
    wait_until(&second, cx, "the tree as it was left", |h, cx| {
        h.shell(cx, |s| {
            s.file_tree_now().is_some_and(|t| {
                t.open.contains("src")
                    && t.selected.as_deref() == Some("src")
                    && t.rows.iter().any(|r| r.path == "src/ui")
            })
        })
    });
}

#[gpui_kit::test]
fn reveal_in_the_tree_opens_the_folders_above_and_selects_the_file(cx: &mut TestAppContext) {
    let (h, _settings, _dir, _path) = project(cx);
    open_by_chord(&h, cx, "src/ui/tree.rs");
    // The tree is hidden: revealing shows it.
    assert!(!h.shell(cx, |s| s.file_tree_now().is_some()));
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |s, cx| {
            s.run_command(crate::keys::Command::RevealInTree, window, cx)
        })
    })
    .unwrap();
    h.settle(cx);
    wait_until(&h, cx, "the file selected", |h, cx| {
        h.shell(cx, |s| {
            s.file_tree_now().is_some_and(|t| {
                t.selected.as_deref() == Some("src/ui/tree.rs")
                    && t.open.contains("src")
                    && t.open.contains("src/ui")
                    && t.selected_index().is_some()
            })
        })
    });
}

#[gpui_kit::test]
fn the_editor_settings_apply_to_files_that_are_open(cx: &mut TestAppContext) {
    let (h, _settings, _dir, _path) = project(cx);
    open_by_chord(&h, cx, "a.txt");
    let set = |key: &str, value: crate::schema::Value, cx: &mut TestAppContext| {
        cx.update(|cx| crate::settings::set_value(cx, crate::schema::find(key).unwrap(), value));
        h.settle(cx);
    };
    // A tab is two spaces now.
    set("editor_tab_size", crate::schema::Value::Int(2), cx);
    h.press("tab", cx);
    let text = cx.update(|cx| h.shell.read(cx).files[&LiveId(1)].text(cx).unwrap());
    assert!(text.starts_with("  a\n"), "{text:?}");
    // Highlighting off puts the language away.
    let language = |h: &Harness, cx: &mut TestAppContext| {
        cx.update(|cx| {
            h.shell.read(cx).files[&LiveId(1)]
                .state()
                .unwrap()
                .read(cx)
                .language_name()
                .to_string()
        })
    };
    open_by_chord(&h, cx, "src/ui/tree.rs");
    let rust = if cfg!(feature = "languages") {
        "rust"
    } else {
        "text"
    };
    assert_eq!(
        cx.update(|cx| h.shell.read(cx).files[&LiveId(2)]
            .state()
            .unwrap()
            .read(cx)
            .language_name()
            .to_string()),
        rust
    );
    set("editor_highlight", crate::schema::Value::Bool(false), cx);
    assert_eq!(
        cx.update(|cx| h.shell.read(cx).files[&LiveId(2)]
            .state()
            .unwrap()
            .read(cx)
            .language_name()
            .to_string()),
        "text"
    );
    set("editor_highlight", crate::schema::Value::Bool(true), cx);
    assert_eq!(
        cx.update(|cx| h.shell.read(cx).files[&LiveId(2)]
            .state()
            .unwrap()
            .read(cx)
            .language_name()
            .to_string()),
        rust
    );
    let _ = language;
}

//! Tests of what keeps text from being lost: drafts, a file that changed on
//! disk under the editor, quitting with unsaved files, and saving over a file
//! that was deleted.

use super::super::editor::{Draft, EditorDoc};
use super::live::{real_worktree, wait_until};
use super::*;
use crate::ui::live::LiveId;

/// A window whose settings (and so its drafts) are kept in a folder, with a
/// worktree and its shell (leaf 1). Nothing is open yet.
fn window(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, tempfile::TempDir, String) {
    cx.executor().allow_parking();
    let settings = tempfile::tempdir().unwrap();
    let h = open_with(
        cx,
        ScriptedRunner::new(),
        Some(settings.path().join(crate::settings::FILE_NAME)),
    );
    show_inactive(cx);
    let (dir, path) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell's prompt", |h, cx| {
        super::live::terminal_of(h, cx, 1).is_some_and(|t| t.screen_text().contains("READY>"))
    });
    (h, settings, dir, path)
}

fn open_by_chord(h: &Harness, cx: &mut TestAppContext, typed: &str) {
    h.press_chord("cmd-shift-o", "ctrl-shift-alt-o", cx);
    h.type_text(typed, cx);
    h.press("enter", cx);
    wait_until(h, cx, "the file's tab", |h, cx| {
        h.shell(cx, |shell| shell.focused_file().is_some())
    });
}

fn doc<R>(h: &Harness, cx: &mut TestAppContext, read: impl FnOnce(&EditorDoc) -> R) -> R {
    h.shell(cx, |shell| read(&shell.files[&LiveId(2)]))
}

fn editor_text(h: &Harness, cx: &mut TestAppContext) -> String {
    cx.update(|cx| h.shell.read(cx).files[&LiveId(2)].text(cx).unwrap())
}

fn drafts_in(settings: &tempfile::TempDir) -> Vec<std::path::PathBuf> {
    match std::fs::read_dir(settings.path().join("drafts")) {
        Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

/// What a crashed run left behind for `file`.
fn keep_draft(
    settings: &tempfile::TempDir,
    file: &std::path::Path,
    text: &str,
    revision: Option<&str>,
) {
    super::super::editor::write_draft(
        &settings.path().join("drafts"),
        &Draft {
            machine: "local".into(),
            path: file.to_string_lossy().into_owned(),
            text: text.into(),
            revision: revision.map(str::to_owned),
        },
    )
    .unwrap();
}

fn overlay_asks(h: &Harness, cx: &mut TestAppContext) -> String {
    h.shell(cx, |shell| {
        shell
            .palette
            .flow
            .as_ref()
            .map(|flow| flow.step.prompt.to_owned())
            .unwrap_or_default()
    })
}

/// Looks at the open files as the window does when it comes to the front.
fn come_to_the_front(h: &Harness, cx: &mut TestAppContext) {
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell
            .update(cx, |shell, cx| shell.check_external_changes(window, cx))
    })
    .unwrap();
    h.settle(cx);
}

#[gpui_kit::test]
fn unsaved_text_is_kept_in_a_draft_until_it_is_saved(cx: &mut TestAppContext) {
    let (h, settings, _dir, path) = window(cx);
    let file = std::path::Path::new(&path).join("notes.txt");
    std::fs::write(&file, "hello\n").unwrap();
    open_by_chord(&h, cx, "notes.txt");
    assert!(drafts_in(&settings).is_empty(), "nothing changed yet");
    h.type_text("x", cx);
    wait_until(&h, cx, "the draft", |_, _| drafts_in(&settings).len() == 1);
    let draft: Draft =
        serde_json::from_slice(&std::fs::read(&drafts_in(&settings)[0]).unwrap()).unwrap();
    assert_eq!(draft.text, "xhello\n");
    assert_eq!(draft.path, file.to_string_lossy());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\n");
    // Undone back to what is on disk, the draft has nothing to keep.
    h.press("ctrl-z", cx);
    wait_until(&h, cx, "the draft to go", |_, _| {
        drafts_in(&settings).is_empty()
    });
    // Saved, it goes too.
    h.type_text("y", cx);
    wait_until(&h, cx, "the draft", |_, _| drafts_in(&settings).len() == 1);
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the save", |h, cx| !doc(h, cx, |d| d.dirty));
    assert!(drafts_in(&settings).is_empty());
}

#[gpui_kit::test]
fn discarding_a_file_removes_its_draft(cx: &mut TestAppContext) {
    let (h, settings, _dir, path) = window(cx);
    std::fs::write(std::path::Path::new(&path).join("notes.txt"), "hello\n").unwrap();
    open_by_chord(&h, cx, "notes.txt");
    h.type_text("x", cx);
    wait_until(&h, cx, "the draft", |_, _| drafts_in(&settings).len() == 1);
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    h.press("down", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the file closed", |h, cx| {
        h.shell(cx, |s| s.files.is_empty())
    });
    assert!(drafts_in(&settings).is_empty());
}

#[gpui_kit::test]
fn a_file_opened_again_after_a_crash_comes_back_with_its_unsaved_text(cx: &mut TestAppContext) {
    let (h, settings, _dir, path) = window(cx);
    let file = std::path::Path::new(&path).join("notes.txt");
    std::fs::write(&file, "hello\n").unwrap();
    // What a crashed run left behind.
    let revision = match crate::files::read(&file).unwrap() {
        crate::files::FileContent::Text { revision, .. } => revision,
        other => panic!("{other:?}"),
    };
    keep_draft(
        &settings,
        &file,
        "hello, from before the crash\n",
        Some(revision.as_str()),
    );
    open_by_chord(&h, cx, "notes.txt");
    assert_eq!(editor_text(&h, cx), "hello, from before the crash\n");
    assert!(doc(&h, cx, |d| d.dirty), "the text is unsaved changes");
    assert!(!doc(&h, cx, |d| d.conflict), "the file is as it was");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\n");
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the save", |h, cx| !doc(h, cx, |d| d.dirty));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "hello, from before the crash\n"
    );
    assert!(drafts_in(&settings).is_empty());
}

#[gpui_kit::test]
fn a_draft_of_a_file_that_changed_since_shows_the_conflict(cx: &mut TestAppContext) {
    let (h, settings, _dir, path) = window(cx);
    let file = std::path::Path::new(&path).join("notes.txt");
    std::fs::write(&file, "the file changed meanwhile\n").unwrap();
    keep_draft(
        &settings,
        &file,
        "my text\n",
        Some("a revision the file no longer has"),
    );
    open_by_chord(&h, cx, "notes.txt");
    assert_eq!(editor_text(&h, cx), "my text\n");
    assert!(doc(&h, cx, |d| d.dirty && d.conflict));
    assert!(h.shows_dynamic("changed-banner-2".into(), cx));
    // Saving asks what to do with the two versions.
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the question", |h, cx| {
        overlay_asks(h, cx) == "Changed on disk"
    });
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "the file changed meanwhile\n"
    );
}

#[gpui_kit::test]
fn a_clean_file_that_changed_on_disk_is_read_again_silently(cx: &mut TestAppContext) {
    let (h, _settings, _dir, path) = window(cx);
    let file = std::path::Path::new(&path).join("notes.txt");
    std::fs::write(&file, "one\ntwo\n").unwrap();
    open_by_chord(&h, cx, "notes.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    come_to_the_front(&h, cx);
    wait_until(&h, cx, "the new text", |h, cx| {
        editor_text(h, cx) == "one\ntwo\nthree\n"
    });
    assert!(!doc(&h, cx, |d| d.dirty || d.conflict));
    assert!(!h.shows_dynamic("changed-banner-2".into(), cx));
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn a_dirty_file_that_changed_on_disk_gets_a_banner_and_keeps_its_text(cx: &mut TestAppContext) {
    let (h, _settings, _dir, path) = window(cx);
    let file = std::path::Path::new(&path).join("notes.txt");
    std::fs::write(&file, "hello\n").unwrap();
    open_by_chord(&h, cx, "notes.txt");
    h.type_text("x", cx);
    std::fs::write(&file, "somebody else\n").unwrap();
    come_to_the_front(&h, cx);
    wait_until(&h, cx, "the banner", |h, cx| {
        h.shows_dynamic("changed-banner-2".into(), cx)
    });
    assert_eq!(editor_text(&h, cx), "xhello\n", "the text is untouched");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None, "not a dialog");

    // Keep mine: the banner goes and a save overwrites without asking.
    h.mouse_on("changed-keep-2".into(), gpui_kit::MouseButton::Left, cx);
    assert!(!h.shows_dynamic("changed-banner-2".into(), cx));
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the save", |h, cx| !doc(h, cx, |d| d.dirty));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xhello\n");
}

#[gpui_kit::test]
fn reload_on_the_banner_takes_the_version_on_disk(cx: &mut TestAppContext) {
    let (h, settings, _dir, path) = window(cx);
    let file = std::path::Path::new(&path).join("notes.txt");
    std::fs::write(&file, "hello\n").unwrap();
    open_by_chord(&h, cx, "notes.txt");
    h.type_text("x", cx);
    wait_until(&h, cx, "the draft", |_, _| drafts_in(&settings).len() == 1);
    std::fs::write(&file, "somebody else\n").unwrap();
    come_to_the_front(&h, cx);
    wait_until(&h, cx, "the banner", |h, cx| {
        h.shows_dynamic("changed-banner-2".into(), cx)
    });
    h.mouse_on("changed-reload-2".into(), gpui_kit::MouseButton::Left, cx);
    wait_until(&h, cx, "the reload", |h, cx| {
        editor_text(h, cx) == "somebody else\n"
    });
    assert!(!doc(&h, cx, |d| d.dirty));
    assert!(!h.shows_dynamic("changed-banner-2".into(), cx));
    assert!(drafts_in(&settings).is_empty());
}

#[gpui_kit::test]
fn quitting_with_unsaved_files_can_save_them_all_first(cx: &mut TestAppContext) {
    let (h, _settings, _dir, path) = window(cx);
    let a = std::path::Path::new(&path).join("a.txt");
    std::fs::write(&a, "a\n").unwrap();
    open_by_chord(&h, cx, "a.txt");
    h.type_text("1", cx);
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    assert_eq!(overlay_asks(&h, cx), "Unsaved changes");
    assert_eq!(h.quits.get(), 0);
    h.press("enter", cx);
    wait_until(&h, cx, "the quit", |h, _| h.quits.get() == 1);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "1a\n");
}

#[gpui_kit::test]
fn quitting_without_saving_leaves_the_files_and_drops_the_drafts(cx: &mut TestAppContext) {
    let (h, settings, _dir, path) = window(cx);
    let a = std::path::Path::new(&path).join("a.txt");
    std::fs::write(&a, "a\n").unwrap();
    open_by_chord(&h, cx, "a.txt");
    h.type_text("1", cx);
    wait_until(&h, cx, "the draft", |_, _| drafts_in(&settings).len() == 1);
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    h.press("down", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the quit", |h, _| h.quits.get() == 1);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a\n");
    assert!(drafts_in(&settings).is_empty());
}

#[gpui_kit::test]
fn cancelling_the_quit_keeps_everything(cx: &mut TestAppContext) {
    let (h, _settings, _dir, path) = window(cx);
    std::fs::write(std::path::Path::new(&path).join("a.txt"), "a\n").unwrap();
    open_by_chord(&h, cx, "a.txt");
    h.type_text("1", cx);
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    h.press("down", cx);
    h.press("down", cx);
    h.press("enter", cx);
    assert_eq!(h.quits.get(), 0);
    assert!(doc(&h, cx, |d| d.dirty));
}

#[gpui_kit::test]
fn a_save_all_that_meets_a_changed_file_does_not_quit(cx: &mut TestAppContext) {
    let (h, _settings, _dir, path) = window(cx);
    let a = std::path::Path::new(&path).join("a.txt");
    std::fs::write(&a, "a\n").unwrap();
    open_by_chord(&h, cx, "a.txt");
    h.type_text("1", cx);
    std::fs::write(&a, "somebody else\n").unwrap();
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the conflict question", |h, cx| {
        overlay_asks(h, cx) == "Changed on disk"
    });
    assert_eq!(h.quits.get(), 0, "nothing is lost by quitting");
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "somebody else\n");
}

#[gpui_kit::test]
fn overwriting_a_file_that_was_deleted_creates_it_again(cx: &mut TestAppContext) {
    let (h, _settings, _dir, path) = window(cx);
    let file = std::path::Path::new(&path).join("notes.txt");
    std::fs::write(&file, "hello\n").unwrap();
    open_by_chord(&h, cx, "notes.txt");
    h.type_text("x", cx);
    std::fs::remove_file(&file).unwrap();
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the question", |h, cx| {
        overlay_asks(h, cx) == "Changed on disk"
    });
    h.press("down", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the file to be back", |h, cx| {
        file.exists() && !doc(h, cx, |d| d.dirty)
    });
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xhello\n");
}

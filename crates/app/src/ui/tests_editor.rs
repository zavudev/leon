//! Tests of files open in tabs: opening one beside a terminal, editing,
//! saving, closing and what happens when the file changed on disk. The
//! terminals are the scripted computer of `tests.rs`; the files are real ones
//! in a temporary folder, read and written through the engine.

use super::live::{open_live, real_worktree, wait_until};
use super::*;
use crate::ui::live::LiveId;

/// A shell in a worktree (leaf 1), and a file of the folder opened beside it
/// (leaf 2) through the same chord and question a person uses.
fn with_file(
    cx: &mut TestAppContext,
    name: &str,
    contents: &[u8],
) -> (Harness, tempfile::TempDir, std::path::PathBuf) {
    let h = open_live(cx);
    let (dir, path) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell's prompt", |h, cx| {
        super::live::terminal_of(h, cx, 1).is_some_and(|t| t.screen_text().contains("READY>"))
    });
    let file = std::path::Path::new(&path).join(name);
    std::fs::write(&file, contents).unwrap();
    open_by_chord(&h, cx, name);
    (h, dir, file)
}

fn open_by_chord(h: &Harness, cx: &mut TestAppContext, typed: &str) {
    h.press_chord("cmd-shift-o", "ctrl-shift-alt-o", cx);
    h.type_text(typed, cx);
    h.press("enter", cx);
    wait_until(h, cx, "the file's tab", |h, cx| {
        h.shell(cx, |shell| shell.focused_file().is_some())
    });
}

fn doc<R>(h: &Harness, cx: &mut TestAppContext, read: impl FnOnce(&Shell, &EditorDoc) -> R) -> R {
    h.shell(cx, |shell| read(shell, &shell.files[&LiveId(2)]))
}

use super::super::editor::{EditorDoc, ViewMode};

fn dirty(h: &Harness, cx: &mut TestAppContext) -> bool {
    doc(h, cx, |_, doc| doc.dirty)
}

fn editor_text(h: &Harness, cx: &mut TestAppContext) -> String {
    cx.update(|cx| h.shell.read(cx).files[&LiveId(2)].text(cx).unwrap())
}

fn files_open(h: &Harness, cx: &mut TestAppContext) -> usize {
    h.shell(cx, |shell| shell.files.len())
}

fn overlay(h: &Harness, cx: &mut TestAppContext) -> Overlay {
    h.shell(cx, |shell| shell.overlay)
}

/// The question the palette asks, by its title.
fn asked(h: &Harness, cx: &mut TestAppContext) -> String {
    h.shell(cx, |shell| {
        shell
            .palette
            .flow
            .as_ref()
            .map(|flow| flow.step.prompt.to_owned())
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn a_file_opens_in_a_tab_beside_the_terminal_of_its_folder(cx: &mut TestAppContext) {
    let (h, _dir, _file) = with_file(cx, "notes.txt", b"hello\n");
    assert_eq!(h.main_kind(cx), "live:2");
    assert_eq!(files_open(&h, cx), 1);
    assert_eq!(
        h.shell(cx, |s| s.workspaces.tab_position(LiveId(2))),
        Some((1, 2)),
        "the second tab of the workspace the terminal is in"
    );
    assert_eq!(h.shell(cx, |s| s.workspaces.len()), 1);
    assert!(h.shows("file-2", cx), "the editor is drawn");
    assert!(h.shows("terminal-tabs", cx));
    assert_eq!(editor_text(&h, cx), "hello\n");
    assert!(!dirty(&h, cx));
    // The terminal is still there, and still a terminal.
    assert!(h.shell(cx, |s| s.live.get(LiveId(1)).is_some()));
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
}

#[gpui_kit::test]
fn opening_a_file_again_shows_its_tab_instead_of_adding_one(cx: &mut TestAppContext) {
    let (h, _dir, _file) = with_file(cx, "notes.txt", b"hello\n");
    h.press_chord("cmd-alt-1", "ctrl-shift-1", cx);
    assert_eq!(h.main_kind(cx), "live:1");
    open_by_chord(&h, cx, "notes.txt");
    assert_eq!(h.main_kind(cx), "live:2");
    assert_eq!(files_open(&h, cx), 1);
    assert_eq!(
        h.shell(cx, |s| s.workspaces.tab_position(LiveId(2))),
        Some((1, 2))
    );
}

#[gpui_kit::test]
fn typing_makes_the_file_dirty_and_the_tab_says_so(cx: &mut TestAppContext) {
    let (h, _dir, file) = with_file(cx, "notes.txt", b"hello\n");
    h.type_text("x", cx);
    assert!(dirty(&h, cx));
    assert_eq!(editor_text(&h, cx), "xhello\n");
    assert_eq!(
        doc(&h, cx, |_, d| Shell::file_label(d)),
        "notes.txt \u{2022}"
    );
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        "hello\n",
        "not saved yet"
    );
    assert!(h.shell(cx, |s| s.unsaved_files()) == ["notes.txt"]);
}

#[gpui_kit::test]
fn saving_writes_the_file_and_makes_it_clean(cx: &mut TestAppContext) {
    let (h, _dir, file) = with_file(cx, "notes.txt", b"hello\n");
    h.type_text("x", cx);
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the save", |h, cx| !dirty(h, cx));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xhello\n");
    assert_eq!(h.status(), "Saved notes.txt.");
    // The revision moved on: a second save is not a conflict.
    h.type_text("y", cx);
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the second save", |h, cx| !dirty(h, cx));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xyhello\n");
    assert!(!doc(&h, cx, |_, d| d.conflict));
}

#[gpui_kit::test]
fn a_file_with_a_mark_and_crlf_lines_keeps_both_when_saved(cx: &mut TestAppContext) {
    let (h, _dir, file) = with_file(cx, "win.txt", "\u{feff}a\r\nb\r\n".as_bytes());
    assert_eq!(
        editor_text(&h, cx),
        "a\nb\n",
        "the editor shows plain lines"
    );
    h.type_text("x", cx);
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the save", |h, cx| !dirty(h, cx));
    assert_eq!(
        std::fs::read(&file).unwrap(),
        "\u{feff}xa\r\nb\r\n".as_bytes()
    );
}

#[gpui_kit::test]
fn undoing_back_to_what_was_saved_makes_the_file_clean_again(cx: &mut TestAppContext) {
    let (h, _dir, _file) = with_file(cx, "notes.txt", b"hello\n");
    h.type_text("x", cx);
    assert!(dirty(&h, cx));
    h.press("ctrl-z", cx);
    assert_eq!(editor_text(&h, cx), "hello\n");
    assert!(!dirty(&h, cx), "the text is what was read");
    assert_eq!(doc(&h, cx, |_, d| Shell::file_label(d)), "notes.txt");
}

#[gpui_kit::test]
fn closing_a_clean_file_asks_nothing_and_shows_the_folder_again(cx: &mut TestAppContext) {
    let (h, _dir, _file) = with_file(cx, "notes.txt", b"hello\n");
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    assert_eq!(overlay(&h, cx), Overlay::None);
    assert_eq!(files_open(&h, cx), 0);
    assert_eq!(
        h.main_kind(cx),
        "live:1",
        "the terminal has the focus again"
    );
    assert!(h.shell(cx, |s| s.workspaces.tab_position(LiveId(1))) == Some((0, 1)));
}

#[gpui_kit::test]
fn closing_a_dirty_file_asks_to_save_discard_or_cancel(cx: &mut TestAppContext) {
    let (h, _dir, file) = with_file(cx, "notes.txt", b"hello\n");
    h.type_text("x", cx);
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    assert_eq!(overlay(&h, cx), Overlay::Palette);
    assert_eq!(asked(&h, cx), "Unsaved changes");
    // Cancel keeps it open, dirty.
    h.press("down", cx);
    h.press("down", cx);
    h.press("enter", cx);
    assert_eq!(files_open(&h, cx), 1);
    assert!(dirty(&h, cx));
    // Discard closes it and leaves the file alone.
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    h.press("down", cx);
    h.press("enter", cx);
    assert_eq!(files_open(&h, cx), 0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\n");
}

#[gpui_kit::test]
fn saving_from_the_close_question_writes_the_file_and_closes_it(cx: &mut TestAppContext) {
    let (h, _dir, file) = with_file(cx, "notes.txt", b"hello\n");
    h.type_text("x", cx);
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the file closed", |h, cx| files_open(h, cx) == 0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xhello\n");
}

/// A file somebody else rewrote after it was read, with a change of the
/// person's own waiting to be saved; the save has been tried.
fn in_conflict(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, std::path::PathBuf) {
    let (h, dir, file) = with_file(cx, "notes.txt", b"hello\n");
    h.type_text("x", cx);
    std::fs::write(&file, "somebody else wrote this\n").unwrap();
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the question", |h, cx| {
        asked(h, cx) == "Changed on disk"
    });
    assert!(doc(&h, cx, |_, d| d.conflict));
    assert!(dirty(&h, cx), "nothing was saved");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "somebody else wrote this\n"
    );
    (h, dir, file)
}

#[gpui_kit::test]
fn a_file_changed_on_disk_asks_before_it_is_overwritten(cx: &mut TestAppContext) {
    let (h, _dir, file) = in_conflict(cx);
    // The first answer leaves everything as it is.
    h.press("enter", cx);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "somebody else wrote this\n"
    );
    assert_eq!(editor_text(&h, cx), "xhello\n");
    assert!(dirty(&h, cx));
}

#[gpui_kit::test]
fn overwrite_saves_the_editors_text_over_the_other_version(cx: &mut TestAppContext) {
    let (h, _dir, file) = in_conflict(cx);
    h.press("down", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the overwrite", |h, cx| !dirty(h, cx));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xhello\n");
    assert!(!doc(&h, cx, |_, d| d.conflict));
}

#[gpui_kit::test]
fn reload_reads_the_other_version_into_the_editor(cx: &mut TestAppContext) {
    let (h, _dir, file) = in_conflict(cx);
    h.press("down", cx);
    h.press("down", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the reload", |h, cx| {
        editor_text(h, cx) == "somebody else wrote this\n"
    });
    assert!(!dirty(&h, cx));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "somebody else wrote this\n"
    );
    // And it can be saved again: the revision is the one that was read. The
    // cursor stayed where it was (after the first character).
    h.type_text("y", cx);
    h.press("ctrl-s", cx);
    wait_until(&h, cx, "the save", |h, cx| !dirty(h, cx));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "syomebody else wrote this\n"
    );
}

#[gpui_kit::test]
fn a_file_splits_beside_a_new_terminal_in_the_same_folder(cx: &mut TestAppContext) {
    let (h, _dir, file) = with_file(cx, "notes.txt", b"hello\n");
    h.press_chord("cmd-d", "ctrl-shift-d", cx);
    wait_until(&h, cx, "the new shell", |h, cx| {
        super::live::terminal_of(h, cx, 3).is_some()
    });
    let layout = h.shell(cx, |s| {
        s.workspaces.tab_of(LiveId(2)).unwrap().layout.clone()
    });
    assert_eq!(layout.leaves(), [LiveId(2), LiveId(3)]);
    assert_eq!(h.main_kind(cx), "live:3", "the new pane has the focus");
    assert_eq!(
        super::live::script_of(&h, 2).spec().cwd.as_deref(),
        file.parent().and_then(|p| p.to_str())
    );
    // Both panes are drawn: the editor and the terminal.
    assert!(h.shows("file-2", cx));
    assert!(h.shows("live-terminal", cx));
    // Closing the file leaves the terminal beside it.
    h.press_chord("cmd-[", "ctrl-shift-[", cx);
    assert_eq!(h.main_kind(cx), "live:2");
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    assert_eq!(files_open(&h, cx), 0);
    assert_eq!(h.main_kind(cx), "live:3");
}

#[gpui_kit::test]
fn a_file_that_is_not_text_says_so_instead_of_opening_an_editor(cx: &mut TestAppContext) {
    let (h, _dir, file) = with_file(cx, "blob.bin", &[0, 1, 2, 0, 255]);
    assert!(h.shows("file-binary", cx));
    assert!(!h.shows("file-2", cx));
    h.press("ctrl-s", cx);
    assert_eq!(std::fs::read(&file).unwrap(), [0, 1, 2, 0, 255]);
    assert!(!dirty(&h, cx));
}

#[gpui_kit::test]
fn the_file_commands_apply_only_while_a_file_is_on_screen(cx: &mut TestAppContext) {
    let (h, _dir, _file) = with_file(cx, "notes.txt", b"hello\n");
    let titles = |h: &Harness, cx: &mut TestAppContext| {
        h.press("ctrl-shift-p", cx);
        h.type_text("file", cx);
        let titles = h.palette_titles(cx);
        h.press("escape", cx);
        titles
    };
    let with = titles(&h, cx);
    assert!(with.iter().any(|t| t == "Save the file"), "{with:?}");
    assert!(with.iter().any(|t| t == "Close the file"), "{with:?}");
    h.press_chord("cmd-alt-1", "ctrl-shift-1", cx);
    let without = titles(&h, cx);
    assert!(
        without.iter().any(|t| t == "Open a file\u{2026}"),
        "{without:?}"
    );
    assert!(!without.iter().any(|t| t == "Save the file"), "{without:?}");
    assert!(!cx.update(|cx| h.shell.read(cx).availability(cx).file));
}

#[gpui_kit::test]
fn quitting_asks_while_a_file_has_unsaved_changes(cx: &mut TestAppContext) {
    let (h, _dir, _file) = with_file(cx, "notes.txt", b"hello\n");
    h.type_text("x", cx);
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    assert_eq!(overlay(&h, cx), Overlay::Palette);
    assert_eq!(asked(&h, cx), "Unsaved changes");
}

#[gpui_kit::test]
fn a_source_file_is_highlighted_in_its_language_and_a_plain_one_is_not(cx: &mut TestAppContext) {
    let (h, _dir, _file) = with_file(cx, "main.rs", b"fn main() {}\n");
    let language = cx.update(|cx| {
        let shell = h.shell.read(cx);
        shell.files[&LiveId(2)]
            .state()
            .unwrap()
            .read(cx)
            .language_name()
    });
    let expected = if cfg!(feature = "languages") {
        "rust"
    } else {
        "text"
    };
    assert_eq!(language.as_ref(), expected);
}

// ----- Markdown preview ---------------------------------------------------------------

fn mode(h: &Harness, cx: &mut TestAppContext) -> ViewMode {
    doc(h, cx, |_, d| d.mode)
}

fn page_text(h: &Harness, cx: &mut TestAppContext) -> String {
    cx.update(|cx| {
        h.shell.read(cx).files[&LiveId(2)]
            .preview
            .as_ref()
            .map(|page| page.read(cx).rendered_text().as_str().to_owned())
            .unwrap_or_default()
    })
}

fn toggle_preview(h: &Harness, cx: &mut TestAppContext) {
    h.press_chord("cmd-alt-v", "ctrl-shift-alt-v", cx);
}

#[gpui_kit::test]
fn a_markdown_file_goes_from_text_to_both_to_the_page_and_back(cx: &mut TestAppContext) {
    use ViewMode::*;
    let (h, _dir, _file) = with_file(cx, "README.md", b"# Title\n\nsome *words*\n");
    assert_eq!(mode(&h, cx), Edit);
    assert!(h.shows("file-2", cx) && !h.shows_dynamic("preview-2".into(), cx));
    assert!(h.shows_dynamic("preview-toolbar-2".into(), cx));

    toggle_preview(&h, cx);
    assert_eq!(mode(&h, cx), Split);
    assert!(h.shows("file-2", cx), "the editor stays");
    assert!(
        h.shows_dynamic("preview-2".into(), cx),
        "and the page is beside it"
    );
    wait_until(&h, cx, "the page", |h, cx| {
        page_text(h, cx).contains("Title")
    });
    assert!(page_text(&h, cx).contains("some words"));

    toggle_preview(&h, cx);
    assert_eq!(mode(&h, cx), Preview);
    assert!(h.shows_dynamic("preview-2".into(), cx));
    assert!(!h.shows("file-2", cx), "the editor gives way to the page");

    toggle_preview(&h, cx);
    assert_eq!(mode(&h, cx), Edit);
    assert!(!h.shows_dynamic("preview-2".into(), cx));
    assert!(h.shows("file-2", cx));

    // The toolbar does the same.
    h.mouse_on("preview-split-2".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(mode(&h, cx), Split);
    h.mouse_on("preview-edit-2".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(mode(&h, cx), Edit);
}

#[gpui_kit::test]
fn a_file_that_is_not_markdown_or_svg_has_no_preview(cx: &mut TestAppContext) {
    let (h, _dir, _file) = with_file(cx, "notes.txt", b"hello\n");
    assert!(!h.shows_dynamic("preview-toolbar-2".into(), cx));
    toggle_preview(&h, cx);
    assert_eq!(mode(&h, cx), ViewMode::Edit);
    assert_eq!(h.status(), "Only a Markdown or SVG file has a preview.");
}

#[gpui_kit::test]
fn the_page_follows_the_text_as_it_changes(cx: &mut TestAppContext) {
    let (h, _dir, _file) = with_file(cx, "README.md", b"hello\n");
    toggle_preview(&h, cx);
    wait_until(&h, cx, "the page", |h, cx| {
        page_text(h, cx).contains("hello")
    });
    h.type_text("brand new ", cx);
    wait_until(&h, cx, "the page to follow", |h, cx| {
        page_text(h, cx).contains("brand new hello")
    });
    // Back to the text, typing there, and the page is up to date when it
    // comes back (the same page, with the same scroll).
    toggle_preview(&h, cx);
    toggle_preview(&h, cx);
    assert_eq!(mode(&h, cx), ViewMode::Edit);
    h.type_text("more ", cx);
    toggle_preview(&h, cx);
    wait_until(&h, cx, "the page to follow again", |h, cx| {
        page_text(h, cx).contains("brand new more hello")
    });
}

#[gpui_kit::test]
fn a_relative_link_in_the_page_opens_the_file_in_a_tab(cx: &mut TestAppContext) {
    let (h, dir, _file) = with_file(cx, "README.md", b"[guide](docs/guide.md#top)\n");
    let _ = &dir;
    std::fs::create_dir_all(_file.parent().unwrap().join("docs")).unwrap();
    std::fs::write(_file.parent().unwrap().join("docs/guide.md"), "# Guide\n").unwrap();
    toggle_preview(&h, cx);
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.preview_link(LiveId(2), "docs/guide.md#top", window, cx)
        })
    })
    .unwrap();
    h.settle(cx);
    wait_until(&h, cx, "the linked file", |h, cx| files_open(h, cx) == 2);
    let opened = h.shell(cx, |s| {
        s.files
            .values()
            .map(|d| d.name().to_owned())
            .collect::<Vec<_>>()
    });
    assert!(opened.contains(&"guide.md".to_owned()), "{opened:?}");
    // A web address goes to the browser instead.
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let sink = seen.clone();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.open_url =
                std::rc::Rc::new(move |_, url| sink.borrow_mut().push(url.to_owned()));
        })
    });
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.preview_link(LiveId(2), "https://example.com/a", window, cx)
        })
    })
    .unwrap();
    assert_eq!(*seen.borrow(), ["https://example.com/a"]);
}

#[gpui_kit::test]
fn open_anyway_shows_a_file_that_is_not_text_read_only_and_never_saves_it(cx: &mut TestAppContext) {
    let (h, _dir, file) = with_file(cx, "blob.bin", &[b'a', 0, b'b', 0xff, b'c']);
    assert!(h.shows("file-binary", cx));
    h.mouse_on("open-anyway-2".into(), gpui_kit::MouseButton::Left, cx);
    wait_until(&h, cx, "the lossy view", |h, cx| {
        h.shows_dynamic("file-lossy-2".into(), cx)
    });
    let shown = cx.update(|cx| match &h.shell.read(cx).files[&LiveId(2)].body {
        super::super::editor::Body::Lossy { state, shown, size } => {
            Some((state.read(cx).value().to_string(), *shown, *size))
        }
        _ => None,
    });
    let (text, shown, size) = shown.expect("a lossy body");
    assert_eq!(text, "a\0b\u{fffd}c");
    assert_eq!((shown, size), (5, 5));
    // Nothing is saved from it.
    h.press("ctrl-s", cx);
    assert_eq!(std::fs::read(&file).unwrap(), [b'a', 0, b'b', 0xff, b'c']);
    assert!(!dirty(&h, cx));
}

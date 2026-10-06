//! Tests of what Leon does with a terminal's buffer and with quitting: find,
//! clear, copy, save, the menu of a pane and the question before quitting.
//! The terminals are the scripted computer of `tests.rs`: no process, no real
//! clipboard, no system dialog.

use super::live::{open_live, real_worktree, script_of, terminal_of, wait_until};
use super::*;
use crate::ui::live::LiveId;
use gpui_kit::{Modifiers, MouseButton};
use leon_term::Extent;
use std::cell::{Cell, RefCell};

/// A shell in a pane that has the keyboard, with `lines` lines of output.
fn terminal_with(
    cx: &mut TestAppContext,
    lines: usize,
) -> (Harness, tempfile::TempDir, std::rc::Rc<Cell<usize>>) {
    let h = open_live(cx);
    let (dir, _) = real_worktree(&h, cx);
    let quits = std::rc::Rc::new(Cell::new(0));
    let counted = quits.clone();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.quit = Rc::new(move |_| counted.set(counted.get() + 1));
        })
    });
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell's prompt", |h, cx| {
        terminal_of(h, cx, 1).is_some_and(|t| t.screen_text().contains("READY>"))
    });
    for n in 1..=lines {
        script_of(&h, 1).print(format!("line {n} of the output\r\n"));
    }
    cx.run_until_parked();
    (h, dir, quits)
}

fn text(h: &Harness, cx: &mut TestAppContext, extent: Extent) -> String {
    terminal_of(h, cx, 1).unwrap().buffer_text(extent)
}

fn clipboard(cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

#[gpui_kit::test]
fn clearing_the_buffer_empties_the_scrollback_and_keeps_the_prompt_line(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 120);
    // The program prints its prompt again, as a shell does after a command.
    script_of(&h, 1).print("READY> ");
    cx.run_until_parked();
    let before = text(&h, cx, Extent::All);
    assert!(
        before.contains("line 1 of the output"),
        "the history is there"
    );
    let sent = script_of(&h, 1).written_text();
    h.press_chord("cmd-k", "ctrl-shift-x", cx);
    let after = text(&h, cx, Extent::All);
    assert_eq!(after, "READY>");
    assert!(!after.contains("line 1 of the output"), "{after}");
    assert!(!after.contains("line 120"), "{after}");
    assert_eq!(
        script_of(&h, 1).written_text(),
        sent,
        "the program was sent nothing"
    );
    assert!(h.status().contains("Cleared"), "{}", h.status());
}

#[gpui_kit::test]
fn clearing_the_scrollback_alone_keeps_what_is_on_screen(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 120);
    let screen = text(&h, cx, Extent::Screen);
    h.press_chord("cmd-alt-k", "ctrl-shift-z", cx);
    assert_eq!(text(&h, cx, Extent::All), screen);
}

#[gpui_kit::test]
fn a_full_screen_program_is_not_cleared_and_the_status_line_says_so(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 5);
    script_of(&h, 1).print("\x1b[?1049hfull screen");
    cx.run_until_parked();
    h.press_chord("cmd-k", "ctrl-shift-x", cx);
    assert!(h.status().contains("full-screen"), "{}", h.status());
    assert!(text(&h, cx, Extent::Screen).contains("full screen"));
}

#[gpui_kit::test]
fn copy_all_output_puts_the_whole_scrollback_on_the_clipboard(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 90);
    h.press("ctrl-shift-p", cx);
    h.type_text("copy all terminal", cx);
    h.press("enter", cx);
    let copied = clipboard(cx).expect("something was copied");
    assert!(copied.contains("line 1 of the output"), "the oldest line");
    assert!(copied.contains("line 90 of the output"), "the newest line");
    assert!(!copied.ends_with('\n'), "no trailing blank lines");
    assert!(h.status().starts_with("Copied "), "{}", h.status());
    assert!(h.status().contains("lines"), "{}", h.status());
}

#[gpui_kit::test]
fn copy_the_visible_screen_leaves_the_history_out(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 90);
    h.press("ctrl-shift-p", cx);
    h.type_text("copy the visible", cx);
    h.press("enter", cx);
    let copied = clipboard(cx).expect("something was copied");
    assert!(!copied.contains("line 1 of the output"), "{copied}");
    assert!(copied.contains("line 90 of the output"));
}

#[gpui_kit::test]
fn select_all_selects_the_whole_buffer_so_the_ordinary_copy_takes_it(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 60);
    h.press_chord("cmd-a", "ctrl-shift-p", cx);
    if !crate::platform::is_mac() {
        // Without a chord on this platform: through the palette.
        h.type_text("select all of the terminal", cx);
        h.press("enter", cx);
    }
    let selected = terminal_of(&h, cx, 1).unwrap().selection_text().unwrap();
    assert!(selected.contains("line 1 of the output") && selected.contains("line 60"));
}

fn save_to(
    cx: &mut TestAppContext,
    h: &Harness,
    path: std::path::PathBuf,
) -> Rc<RefCell<Vec<String>>> {
    let names = Rc::new(RefCell::new(Vec::new()));
    let seen = names.clone();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.save_file = Rc::new(move |_, name| {
                seen.borrow_mut().push(name.to_owned());
                Task::ready(Picked::Folder(path.clone()))
            });
        })
    });
    names
}

fn run_save(h: &Harness, cx: &mut TestAppContext) {
    h.press("ctrl-shift-p", cx);
    h.type_text("save the terminal output to", cx);
    h.press("enter", cx);
    cx.run_until_parked();
    h.settle(cx);
}

#[gpui_kit::test]
fn saving_the_output_writes_the_scrollback_to_the_chosen_file(cx: &mut TestAppContext) {
    let (h, dir, _) = terminal_with(cx, 100);
    let target = dir.path().join("out.txt");
    let names = save_to(cx, &h, target.clone());
    run_save(&h, cx);
    let written = std::fs::read_to_string(&target).expect("the file was written");
    assert!(written.contains("line 1 of the output"));
    assert!(written.contains("line 100 of the output"));
    assert!(written.ends_with('\n'));
    assert!(h.status().starts_with("Saved "), "{}", h.status());
    // <project>-<worktree>-<agent or shell>-<date and time>.txt
    assert_eq!(
        names.borrow().as_slice(),
        ["real-trunk-shell-20261004-120500.txt"]
    );
}

#[gpui_kit::test]
fn saving_with_colours_writes_ansi_text(cx: &mut TestAppContext) {
    let (h, dir, _) = terminal_with(cx, 3);
    script_of(&h, 1).print("\x1b[31mred words\x1b[0m\r\n");
    cx.run_until_parked();
    let target = dir.path().join("out.ansi");
    let names = save_to(cx, &h, target.clone());
    h.press("ctrl-shift-p", cx);
    h.type_text("save the terminal output with", cx);
    h.press("enter", cx);
    h.settle(cx);
    let written = std::fs::read_to_string(&target).unwrap();
    assert!(written.contains("\x1b["), "{written:?}");
    assert!(written.contains("red words"));
    assert!(names.borrow()[0].ends_with(".ansi"), "{:?}", names.borrow());
}

#[gpui_kit::test]
fn a_cancelled_save_dialog_writes_nothing(cx: &mut TestAppContext) {
    let (h, dir, _) = terminal_with(cx, 10);
    // The harness's own dialog answers "cancelled".
    run_save(&h, cx);
    let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(files.is_empty(), "{files:?}");
    assert!(!h.status().contains("Saved"), "{}", h.status());
}

#[gpui_kit::test]
fn a_write_error_is_reported_in_the_status_line(cx: &mut TestAppContext) {
    let (h, dir, _) = terminal_with(cx, 10);
    save_to(cx, &h, dir.path().join("no-such-folder").join("out.txt"));
    run_save(&h, cx);
    assert!(h.status().starts_with("Could not save"), "{}", h.status());
    assert_eq!(h.engine.status().unwrap().kind, StatusKind::Error);
}

#[gpui_kit::test]
fn right_click_in_a_terminal_opens_its_menu_unless_the_program_reports_the_mouse(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _) = terminal_with(cx, 3);
    h.mouse_on("pane-1".to_owned(), MouseButton::Right, cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Menu);
    assert!(h.shell(cx, |s| s.menu.as_ref().is_some_and(|menu| menu.terminal)));
    let labels = h.shell(cx, |s| {
        s.menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|item| item.label.clone())
            .collect::<Vec<_>>()
    });
    for wanted in [
        "Copy",
        "Paste",
        "Select all",
        "Copy all output",
        "Find…",
        "Clear buffer",
    ] {
        assert!(
            labels.iter().any(|label| label == wanted),
            "{wanted}: {labels:?}"
        );
    }
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);

    // A program that asked for the mouse keeps the right button...
    script_of(&h, 1).print("\x1b[?1000h\x1b[?1006h");
    cx.run_until_parked();
    h.mouse_on("pane-1".to_owned(), MouseButton::Right, cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    // ...unless Shift is held.
    let bounds = h.bounds_of("pane-1".to_owned(), cx).unwrap();
    let mut visual = VisualTestContext::from_window(h.window.into(), cx);
    visual.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::shift());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Right, Modifiers::shift());
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Menu);
}

#[gpui_kit::test]
fn choosing_clear_in_the_pane_menu_runs_the_same_command_as_its_chord(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 100);
    h.mouse_on("pane-1".to_owned(), MouseButton::Right, cx);
    h.type_text("clear", cx);
    h.press("enter", cx);
    assert!(!text(&h, cx, Extent::All).contains("line 1 of the output"));
}

// ----- find -------------------------------------------------------------------------------

fn open_find(h: &Harness, cx: &mut TestAppContext) {
    h.press_chord("cmd-f", "ctrl-shift-f", cx);
    assert!(h.shows("find-bar", cx), "the bar is over the pane");
}

fn bar<R>(
    h: &Harness,
    cx: &mut TestAppContext,
    read: impl FnOnce(&crate::ui::find::FindBar) -> R,
) -> R {
    h.shell(cx, |s| {
        read(s.find.get(&LiveId(1)).expect("a bar for pane 1"))
    })
}

#[gpui_kit::test]
fn the_find_bar_opens_over_the_focused_pane_without_resizing_the_grid(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 30);
    let before = terminal_of(&h, cx, 1).unwrap().size();
    open_find(&h, cx);
    let pane = h.bounds_of("pane-1".to_owned(), cx).unwrap();
    let find = h.bounds_of("find-bar".to_owned(), cx).unwrap();
    assert!(find.right() <= pane.right() && find.top() >= pane.top());
    assert!(
        find.right() > pane.right() - px(40.),
        "anchored to the top right"
    );
    let after = terminal_of(&h, cx, 1).unwrap().size();
    assert_eq!((before.cols, before.rows), (after.cols, after.rows));
}

#[gpui_kit::test]
fn typing_a_query_highlights_and_counts_and_next_and_previous_move_and_wrap(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _) = terminal_with(cx, 0);
    for n in 1..=5 {
        script_of(&h, 1).print(format!("needle {n}\r\n"));
    }
    cx.run_until_parked();
    open_find(&h, cx);
    h.type_text("needle", cx);
    assert_eq!(bar(&h, cx, |b| b.state.matches().len()), 5);
    // The first is the newest, and the counter says where it is.
    assert_eq!(bar(&h, cx, |b| b.state.current()), Some(4));
    assert_eq!(
        h.shell(cx, |s| crate::ui::find::counter(&s.find[&LiveId(1)].state)),
        "5 of 5"
    );
    let shown = terminal_of(&h, cx, 1)
        .unwrap()
        .highlights()
        .expect("matches are shown");
    assert_eq!(shown.matches.len(), 5);
    assert_eq!(shown.current, Some(4));
    h.press("enter", cx);
    assert_eq!(bar(&h, cx, |b| b.state.current()), Some(3), "enter goes up");
    h.press("shift-enter", cx);
    assert_eq!(
        bar(&h, cx, |b| b.state.current()),
        Some(4),
        "shift+enter goes down"
    );
    h.press("shift-enter", cx);
    assert_eq!(
        bar(&h, cx, |b| b.state.current()),
        Some(0),
        "wrapped to the oldest"
    );
    assert!(bar(&h, cx, |b| b.state.wrapped()));
    assert!(h.shows("find-wrapped", cx), "the wrap is shown");
}

#[gpui_kit::test]
fn keys_typed_in_the_bar_never_reach_the_program_and_escape_gives_it_the_keyboard_back(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _) = terminal_with(cx, 3);
    let sent = script_of(&h, 1).written_text();
    open_find(&h, cx);
    h.type_text("abc", cx);
    h.press("enter", cx);
    assert_eq!(
        script_of(&h, 1).written_text(),
        sent,
        "nothing reached the PTY"
    );
    h.press("escape", cx);
    assert!(!h.shows("find-bar", cx), "the bar is closed");
    assert!(
        terminal_of(&h, cx, 1).unwrap().highlights().is_none(),
        "the highlights are gone"
    );
    h.type_text("x", cx);
    assert_eq!(
        script_of(&h, 1).written_text(),
        format!("{sent}x"),
        "typing reaches the program again"
    );
}

#[gpui_kit::test]
fn a_match_in_the_scrollback_scrolls_into_view(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 0);
    script_of(&h, 1).print("the-old-needle\r\n");
    for n in 1..=100 {
        script_of(&h, 1).print(format!("filler {n}\r\n"));
    }
    cx.run_until_parked();
    assert_eq!(terminal_of(&h, cx, 1).unwrap().display_offset(), 0);
    open_find(&h, cx);
    h.type_text("the-old-needle", cx);
    assert!(
        terminal_of(&h, cx, 1).unwrap().display_offset() > 0,
        "scrolled back to the match"
    );
}

#[gpui_kit::test]
fn output_that_arrives_while_the_bar_is_open_updates_the_count(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 0);
    script_of(&h, 1).print("hit one\r\n");
    cx.run_until_parked();
    open_find(&h, cx);
    h.type_text("hit", cx);
    assert_eq!(bar(&h, cx, |b| b.state.matches().len()), 1);
    script_of(&h, 1).print("hit two\r\nhit three\r\n");
    h.settle(cx);
    assert_eq!(bar(&h, cx, |b| b.state.matches().len()), 3);
}

#[gpui_kit::test]
fn an_invalid_regular_expression_is_reported_without_a_panic(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 3);
    open_find(&h, cx);
    cx.update(|cx| h.shell.update(cx, |s, cx| s.find_toggle("regex", cx)));
    h.type_text("(", cx);
    assert!(bar(&h, cx, |b| b.state.error().is_some()));
    assert_eq!(
        h.shell(cx, |s| crate::ui::find::counter(&s.find[&LiveId(1)].state)),
        "Invalid expression"
    );
}

#[gpui_kit::test]
fn reopening_the_bar_brings_the_previous_query_back(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 3);
    open_find(&h, cx);
    h.type_text("line", cx);
    h.press("escape", cx);
    open_find(&h, cx);
    assert_eq!(bar(&h, cx, |b| b.state.query().to_owned()), "line");
    let value = cx.update(|cx| h.shell.read(cx).find_input.read(cx).value().to_string());
    assert_eq!(value, "line");
}

#[gpui_kit::test]
fn the_find_chord_means_filter_projects_in_the_sidebar_and_find_in_a_terminal(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _) = terminal_with(cx, 3);
    // From the sidebar: the filter. (A plain Ctrl+L belongs to the terminal
    // off macOS, so the sidebar is reached with its Ctrl+Shift chord there.)
    h.press_chord("cmd-l", "ctrl-shift-s", cx);
    h.press_chord("cmd-f", "ctrl-f", cx);
    assert!(!h.shows("find-bar", cx));
    let filtering = cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.read(cx).filter_focused(window, cx)
    });
    assert!(filtering.unwrap());
    // From the terminal: the bar.
    h.press("escape", cx);
    h.press("ctrl-j", cx);
    assert!(h.shell(cx, |s| s.pane == Pane::Main));
    open_find(&h, cx);
}

// ----- quitting -----------------------------------------------------------------------------

#[gpui_kit::test]
fn quitting_with_nothing_running_does_not_ask(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let quits = Rc::new(Cell::new(0));
    let counted = quits.clone();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.quit = Rc::new(move |_| counted.set(counted.get() + 1));
        })
    });
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    assert_eq!(quits.get(), 1);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn quitting_with_a_program_running_asks_and_escape_cancels_and_enter_quits_and_hangs_up(
    cx: &mut TestAppContext,
) {
    let (h, _dir, quits) = terminal_with(cx, 1);
    // A program in front of the shell: busy.
    script_of(&h, 1).set_foreground(false);
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    assert_eq!(quits.get(), 0, "not yet");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    let titles = h.palette_titles(cx);
    assert!(
        titles.iter().any(|title| title == "Quit Leon"),
        "{titles:?}"
    );
    h.press("escape", cx);
    assert_eq!(quits.get(), 0, "escape cancels");
    assert!(!script_of(&h, 1).hung_up());
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    h.press("enter", cx);
    assert_eq!(quits.get(), 1, "enter confirms");
    assert!(script_of(&h, 1).hung_up(), "the terminal was hung up");
}

#[gpui_kit::test]
fn a_menu_item_runs_the_same_command_as_its_chord(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 120);
    // The action a menu item dispatches.
    cx.update_window(h.window.into(), |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::menus::Run(crate::keys::Command::ClearBuffer)),
            cx,
        );
    })
    .unwrap();
    h.settle(cx);
    assert!(!text(&h, cx, Extent::All).contains("line 1 of the output"));
}

#[gpui_kit::test]
fn the_about_panel_names_the_product_and_its_version(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("about leon", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::About);
    assert!(h.shows("about-name", cx) && h.shows("about-version", cx));
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

// ----- the sidebar --------------------------------------------------------------------------

fn sidebar_shown(h: &Harness, cx: &mut TestAppContext) -> bool {
    h.shows("sidebar", cx)
}

#[gpui_kit::test]
fn toggling_the_sidebar_hides_it_and_the_main_pane_reaches_the_window_edge(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    assert!(sidebar_shown(&h, cx));
    let width = h.bounds_of("sidebar".to_owned(), cx).unwrap().size.width;
    assert_eq!(width, px(320.));
    h.press_chord("cmd-b", "ctrl-shift-b", cx);
    assert!(!sidebar_shown(&h, cx), "hidden");
    let main = h.bounds_of("main-header".to_owned(), cx).unwrap();
    assert_eq!(
        main.left(),
        px(0.),
        "the main pane starts at the window's edge"
    );
    assert_eq!(
        h.shell(cx, |s| s.pane),
        Pane::Main,
        "the keyboard went to the main pane"
    );
    // Back: the same width, and the tree has the keyboard.
    h.press_chord("cmd-b", "ctrl-shift-b", cx);
    assert!(sidebar_shown(&h, cx));
    assert_eq!(
        h.bounds_of("sidebar".to_owned(), cx).unwrap().size.width,
        width
    );
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
}

#[gpui_kit::test]
fn there_is_one_toggle_button_in_the_main_header_in_both_states(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    assert!(h.shows("sidebar-toggle-main", cx));
    assert!(
        !h.shows("sidebar-toggle-side", cx),
        "one button only, outside the sidebar"
    );
    h.press_chord("cmd-b", "ctrl-shift-b", cx);
    assert!(h.shows("sidebar-toggle-main", cx), "the way back stays");
    // A click on it brings the sidebar back.
    h.mouse_on("sidebar-toggle-main".to_owned(), MouseButton::Left, cx);
    assert!(sidebar_shown(&h, cx));
}

#[gpui_kit::test]
fn hiding_the_sidebar_resizes_a_live_terminal_to_the_wider_pane(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 3);
    let before = terminal_of(&h, cx, 1).unwrap().size();
    h.press_chord("cmd-b", "ctrl-shift-b", cx);
    cx.run_until_parked();
    let after = terminal_of(&h, cx, 1).unwrap().size();
    assert!(
        after.cols > before.cols,
        "{} -> {}",
        before.cols,
        after.cols
    );
    assert_eq!(
        script_of(&h, 1).size().cols,
        after.cols,
        "the program was told"
    );
}

#[gpui_kit::test]
fn the_filter_key_opens_a_hidden_sidebar_and_focuses_the_field(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press_chord("cmd-b", "ctrl-shift-b", cx);
    assert!(!sidebar_shown(&h, cx));
    h.press("/", cx);
    assert!(sidebar_shown(&h, cx));
    let filtering = cx
        .update_window(h.window.into(), |_, window, cx| {
            h.shell.read(cx).filter_focused(window, cx)
        })
        .unwrap();
    assert!(filtering);
}

#[gpui_kit::test]
fn jumping_to_a_machine_opens_a_hidden_sidebar(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press_chord("cmd-b", "ctrl-shift-b", cx);
    h.press("ctrl-2", cx);
    assert!(sidebar_shown(&h, cx));
    assert_eq!(h.machine_name(cx), "build box");
}

#[gpui_kit::test]
fn the_sidebar_state_round_trips_through_the_settings_file(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join(settings::FILE_NAME);
    let h = open_with(cx, ScriptedRunner::new(), Some(file.clone()));
    cx.update(|cx| h.shell.update(cx, |s, cx| s.set_sidebar_width(400, cx)));
    h.press_chord("cmd-b", "ctrl-shift-b", cx);
    let saved = settings::Settings::load(&file);
    assert!(!saved.sidebar_visible);
    assert_eq!(saved.sidebar_width, 400);
    // Restored at start.
    cx.update(|cx| {
        settings::init(Some(file.clone()), settings::Overrides::default(), cx);
    });
    assert_eq!(crate::theme::metrics::SIDEBAR_WIDTH(), px(0.), "hidden");
    cx.update(|cx| settings::update(cx, |s| s.sidebar_visible = true));
    assert_eq!(crate::theme::metrics::SIDEBAR_WIDTH(), px(400.));
}

#[gpui_kit::test]
fn the_width_is_kept_between_the_minimum_and_the_maximum(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let width = |cx: &mut TestAppContext| cx.update(|cx| settings::get(cx).sidebar_width);
    cx.update(|cx| h.shell.update(cx, |s, cx| s.step_sidebar(100, cx)));
    assert_eq!(width(cx), crate::theme::SIDEBAR_MAX);
    cx.update(|cx| h.shell.update(cx, |s, cx| s.step_sidebar(-100, cx)));
    assert_eq!(width(cx), crate::theme::SIDEBAR_MIN);
    cx.update(|cx| h.shell.update(cx, |s, cx| s.step_sidebar(1, cx)));
    assert_eq!(
        width(cx),
        crate::theme::SIDEBAR_MIN + crate::theme::SIDEBAR_STEP
    );
    // The palette has the commands.
    h.press("ctrl-shift-p", cx);
    h.type_text("reset the sidebar", cx);
    h.press("enter", cx);
    assert_eq!(width(cx), crate::theme::SIDEBAR_DEFAULT);
}

fn pointer(h: &Harness, cx: &mut TestAppContext) -> VisualTestContext {
    VisualTestContext::from_window(h.window.into(), cx)
}

fn at(x: f32, y: f32) -> gpui_kit::Point<gpui_kit::Pixels> {
    gpui_kit::point(px(x), px(y))
}

fn width_now(h: &Harness, cx: &mut TestAppContext) -> gpui_kit::Pixels {
    h.bounds_of("sidebar".to_owned(), cx).unwrap().size.width
}

#[gpui_kit::test]
fn a_real_drag_of_the_edge_follows_the_pointer_far_outside_the_handle_and_ends_on_release_anywhere(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _) = terminal_with(cx, 3);
    // A program that asked for the mouse, and a selection in the making.
    script_of(&h, 1).print("\x1b[?1000h\x1b[?1006h");
    cx.run_until_parked();
    let sent = script_of(&h, 1).written();
    let edge = h.bounds_of("sidebar-resize".to_owned(), cx).unwrap();
    assert!(edge.size.width >= px(8.), "a comfortable handle: {edge:?}");
    let y = 300.;
    let mut visual = pointer(&h, cx);
    // The pointer goes down in the middle of the handle, on the rule...
    visual.simulate_mouse_down(
        at(edge.center().x.as_f32(), y),
        MouseButton::Left,
        Modifiers::none(),
    );
    // ...and at once leaves it, across the terminal, 5 px and then 300 px away.
    for x in [330., 360., 500., 620.] {
        visual.simulate_mouse_move(at(x, y + 40.), Some(MouseButton::Left), Modifiers::none());
        visual.run_until_parked();
    }
    assert_eq!(
        width_now(&h, cx),
        px(560.),
        "the width followed the pointer to the maximum"
    );
    // Released over the terminal, far from the handle.
    pointer(&h, cx).simulate_mouse_up(at(700., y + 40.), MouseButton::Left, Modifiers::none());
    h.settle(cx);
    let kept = cx.update(|cx| settings::get(cx).sidebar_width);
    assert_eq!(
        kept,
        crate::theme::SIDEBAR_MAX,
        "clamped at the maximum and kept on release"
    );
    // The drag is over: moving no longer resizes.
    pointer(&h, cx).simulate_mouse_move(at(400., y), None, Modifiers::none());
    h.settle(cx);
    assert_eq!(width_now(&h, cx), px(560.));
    // The terminal saw none of it: no selection, no mouse reports.
    assert!(terminal_of(&h, cx, 1).unwrap().selection_text().is_none());
    assert_eq!(
        script_of(&h, 1).written(),
        sent,
        "nothing was reported to the program"
    );
}

#[gpui_kit::test]
fn the_drag_resizes_live_on_every_move_clamps_at_the_minimum_and_closes_far_below_it(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    let edge = h.bounds_of("sidebar-resize".to_owned(), cx).unwrap();
    let y = 300.;
    let mut visual = pointer(&h, cx);
    visual.simulate_mouse_down(
        at(edge.center().x.as_f32(), y),
        MouseButton::Left,
        Modifiers::none(),
    );
    visual.simulate_mouse_move(at(400., y), Some(MouseButton::Left), Modifiers::none());
    visual.run_until_parked();
    assert_eq!(width_now(&h, cx), px(400.), "live, before the button is up");
    assert_eq!(
        cx.update(|cx| settings::get(cx).sidebar_width),
        320,
        "not saved until the release"
    );
    visual.simulate_mouse_move(at(190., y), Some(MouseButton::Left), Modifiers::none());
    visual.run_until_parked();
    assert_eq!(width_now(&h, cx), px(220.), "clamped at the minimum");
    visual.simulate_mouse_move(at(90., y), Some(MouseButton::Left), Modifiers::none());
    visual.simulate_mouse_up(at(90., y), MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    assert!(!sidebar_shown(&h, cx), "far below the minimum it closes");
}

#[gpui_kit::test]
fn a_button_released_outside_the_window_ends_the_drag_at_the_next_move(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let edge = h.bounds_of("sidebar-resize".to_owned(), cx).unwrap();
    let mut visual = pointer(&h, cx);
    visual.simulate_mouse_down(
        at(edge.center().x.as_f32(), 300.),
        MouseButton::Left,
        Modifiers::none(),
    );
    visual.simulate_mouse_move(at(420., 300.), Some(MouseButton::Left), Modifiers::none());
    // The up was lost: the next move says no button is held.
    visual.simulate_mouse_move(at(500., 300.), None, Modifiers::none());
    visual.run_until_parked();
    assert_eq!(
        width_now(&h, cx),
        px(420.),
        "the drag ended where the button was last seen down"
    );
}

#[gpui_kit::test]
fn a_double_click_on_the_edge_resets_the_width(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    cx.update(|cx| h.shell.update(cx, |s, cx| s.set_sidebar_width(420, cx)));
    h.settle(cx);
    let edge = h.bounds_of("sidebar-resize".to_owned(), cx).unwrap();
    let p = at(edge.center().x.as_f32(), 300.);
    let mut visual = pointer(&h, cx);
    for count in [1, 2] {
        visual.simulate_event(gpui_kit::MouseDownEvent {
            position: p,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: count,
            first_mouse: false,
        });
        visual.simulate_event(gpui_kit::MouseUpEvent {
            position: p,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: count,
        });
    }
    visual.run_until_parked();
    assert_eq!(
        cx.update(|cx| settings::get(cx).sidebar_width),
        crate::theme::SIDEBAR_DEFAULT
    );
}

#[gpui_kit::test]
fn a_press_on_the_handle_reaches_neither_the_tree_row_nor_the_terminal_beneath_it(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _) = terminal_with(cx, 3);
    let before = h.shell(cx, |s| (s.cursor, s.pane));
    let edge = h.bounds_of("sidebar-resize".to_owned(), cx).unwrap();
    // The handle spans the rule: its right half is over the terminal's pane,
    // its left half over the tree's rows.
    let rows = h.bounds_of("sidebar-tools".to_owned(), cx).unwrap();
    let ys = [140., 200., rows.top().as_f32() - 20.];
    for x in [edge.left().as_f32() + 1., edge.right().as_f32() - 1.] {
        for y in ys {
            let mut visual = pointer(&h, cx);
            visual.simulate_mouse_down(at(x, y), MouseButton::Left, Modifiers::none());
            visual.simulate_mouse_up(at(x, y), MouseButton::Left, Modifiers::none());
            visual.run_until_parked();
        }
    }
    assert_eq!(
        h.shell(cx, |s| (s.cursor, s.pane)),
        before,
        "no row was activated, no pane focused"
    );
    assert!(terminal_of(&h, cx, 1).unwrap().selection_text().is_none());
}

#[gpui_kit::test]
fn the_handle_is_above_the_crosshair_and_every_pane_and_is_drawn_after_them(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    let edge = h.bounds_of("sidebar-resize".to_owned(), cx).unwrap();
    let cross = h.bounds_of("crosshair-header".to_owned(), cx).unwrap();
    assert!(
        edge.contains(&cross.center()),
        "the handle covers the crosshair"
    );
    let main = h.bounds_of("main-header".to_owned(), cx).unwrap();
    assert!(edge.right() > main.left(), "it reaches into the main pane");
    let window = h.bounds_of("shell".to_owned(), cx).unwrap_or(edge);
    assert!(
        edge.size.height >= window.size.height - px(1.),
        "full height: {edge:?} in {window:?}"
    );
}

#[gpui_kit::test]
fn a_plain_ctrl_b_still_reaches_a_focused_terminal(cx: &mut TestAppContext) {
    let (h, _dir, _) = terminal_with(cx, 1);
    let sent = script_of(&h, 1).written_text();
    // The physical Ctrl key, whatever the platform: tmux's prefix.
    cx.update_window(h.window.into(), |_, window, cx| window.press("ctrl-b", cx))
        .unwrap();
    h.settle(cx);
    assert_eq!(script_of(&h, 1).written_text(), format!("{sent}\u{2}"));
    assert!(sidebar_shown(&h, cx), "the sidebar did not move");
}

#[gpui_kit::test]
fn the_sidebars_menu_item_names_what_it_will_do(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let shown = cx.update(|cx| h.shell.read(cx).availability(cx));
    assert!(shown.sidebar);
    h.press_chord("cmd-b", "ctrl-shift-b", cx);
    let hidden = cx.update(|cx| h.shell.read(cx).availability(cx));
    assert!(!hidden.sidebar);
}

// ----- paste by what the clipboard holds ------------------------------------------------------

mod pasting {
    use super::*;
    use gpui_kit::{ClipboardEntry, ClipboardItem, Image, ImageFormat};

    fn png() -> ClipboardEntry {
        ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]))
    }

    fn words(text: &str) -> ClipboardEntry {
        ClipboardEntry::String(gpui_kit::ClipboardString::new(text.to_owned()))
    }

    /// The clipboard the shell reads from now on.
    fn clipboard_holds(cx: &mut TestAppContext, h: &Harness, entries: Vec<ClipboardEntry>) {
        let item = ClipboardItem { entries };
        cx.update(|cx| {
            h.shell.update(cx, |shell, _| {
                shell.options.read_clipboard = Rc::new(move |_| Some(item.clone()));
            })
        });
    }

    #[gpui_kit::test]
    fn pasting_with_an_image_on_the_clipboard_sends_ctrl_v_to_the_program(cx: &mut TestAppContext) {
        let (h, _dir, _) = terminal_with(cx, 1);
        clipboard_holds(cx, &h, vec![png()]);
        let sent = script_of(&h, 1).written();
        h.press_chord("cmd-v", "ctrl-shift-v", cx);
        let now = script_of(&h, 1).written();
        assert_eq!(&now[sent.len()..], [0x16], "only Ctrl+V, no image bytes");
        assert!(h.status().contains("Ctrl+V"), "{}", h.status());
    }

    #[gpui_kit::test]
    fn pasting_text_pastes_the_text_bracketed_when_enabled(cx: &mut TestAppContext) {
        let (h, _dir, _) = terminal_with(cx, 1);
        clipboard_holds(cx, &h, vec![words("hello there")]);
        let sent = script_of(&h, 1).written_text();
        h.press_chord("cmd-v", "ctrl-shift-v", cx);
        assert_eq!(
            &script_of(&h, 1).written_text()[sent.len()..],
            "hello there"
        );
        // A program that asked for bracketed paste gets the markers.
        script_of(&h, 1).print("\x1b[?2004h");
        cx.run_until_parked();
        let sent = script_of(&h, 1).written_text();
        h.press_chord("cmd-v", "ctrl-shift-v", cx);
        assert_eq!(
            &script_of(&h, 1).written_text()[sent.len()..],
            "\x1b[200~hello there\x1b[201~"
        );
    }

    #[gpui_kit::test]
    fn pasting_copied_files_pastes_their_quoted_paths(cx: &mut TestAppContext) {
        let (h, _dir, _) = terminal_with(cx, 1);
        let files = gpui_kit::ExternalPaths(
            [
                std::path::PathBuf::from("/tmp/a.png"),
                std::path::PathBuf::from("/tmp/two words.png"),
            ]
            .into_iter()
            .collect(),
        );
        clipboard_holds(cx, &h, vec![ClipboardEntry::ExternalPaths(files)]);
        let sent = script_of(&h, 1).written_text();
        h.press_chord("cmd-v", "ctrl-shift-v", cx);
        assert_eq!(
            &script_of(&h, 1).written_text()[sent.len()..],
            "/tmp/a.png '/tmp/two words.png'"
        );
    }

    #[gpui_kit::test]
    fn an_empty_clipboard_sends_nothing_and_says_so(cx: &mut TestAppContext) {
        let (h, _dir, _) = terminal_with(cx, 1);
        let sent = script_of(&h, 1).written();
        h.press_chord("cmd-v", "ctrl-shift-v", cx);
        assert_eq!(script_of(&h, 1).written(), sent);
        assert!(h.status().contains("nothing to paste"), "{}", h.status());
    }

    #[gpui_kit::test]
    fn an_image_paste_on_a_remote_terminal_explains_why_it_cannot_work(cx: &mut TestAppContext) {
        let (h, _dir, _) = terminal_with(cx, 1);
        cx.update(|cx| {
            h.shell.update(cx, |shell, _| {
                if let Some(session) = shell.live.get_mut(crate::ui::live::LiveId(1)) {
                    session.machine = MachineId::from_string("remote-box");
                }
            })
        });
        clipboard_holds(cx, &h, vec![png()]);
        let sent = script_of(&h, 1).written();
        h.press_chord("cmd-v", "ctrl-shift-v", cx);
        assert_eq!(script_of(&h, 1).written(), sent, "nothing is sent");
        assert!(
            h.status()
                .contains("needs the agent to run on this computer"),
            "{}",
            h.status()
        );
    }

    #[gpui_kit::test]
    fn plain_ctrl_v_reaches_the_program_as_0x16(cx: &mut TestAppContext) {
        let (h, _dir, _) = terminal_with(cx, 1);
        clipboard_holds(cx, &h, vec![words("must not be pasted")]);
        let sent = script_of(&h, 1).written();
        // The physical Ctrl+V, on every platform (on macOS Cmd is another key).
        cx.update_window(h.window.into(), |_, window, cx| window.press("ctrl-v", cx))
            .unwrap();
        h.settle(cx);
        assert_eq!(&script_of(&h, 1).written()[sent.len()..], [0x16]);
    }

    #[gpui_kit::test]
    fn with_both_kinds_an_agent_gets_the_image_and_a_shell_the_text_and_the_commands_force_either(
        cx: &mut TestAppContext,
    ) {
        let (h, _dir, _) = terminal_with(cx, 1);
        clipboard_holds(cx, &h, vec![png(), words("photo.png")]);
        // A plain shell: the text.
        let sent = script_of(&h, 1).written_text();
        h.press_chord("cmd-v", "ctrl-shift-v", cx);
        assert_eq!(&script_of(&h, 1).written_text()[sent.len()..], "photo.png");
        // The explicit commands, from the palette.
        let sent = script_of(&h, 1).written();
        h.press("ctrl-shift-p", cx);
        h.type_text("paste image", cx);
        h.press("enter", cx);
        assert_eq!(&script_of(&h, 1).written()[sent.len()..], [0x16]);
        let sent = script_of(&h, 1).written_text();
        h.press("ctrl-shift-p", cx);
        h.type_text("paste as text", cx);
        h.press("enter", cx);
        assert_eq!(&script_of(&h, 1).written_text()[sent.len()..], "photo.png");
        // An agent in front: the image.
        cx.update(|cx| {
            h.shell.update(cx, |shell, _| {
                if let Some(session) = shell.live.get_mut(crate::ui::live::LiveId(1)) {
                    session.agent = Some(AgentKind::Claude);
                    session.phase = crate::ui::live::AgentPhase::Running;
                }
            })
        });
        let sent = script_of(&h, 1).written();
        h.press_chord("cmd-v", "ctrl-shift-v", cx);
        assert_eq!(&script_of(&h, 1).written()[sent.len()..], [0x16]);
    }

    #[gpui_kit::test]
    fn the_terminal_menu_lists_both_explicit_pastes(cx: &mut TestAppContext) {
        let (h, _dir, _) = terminal_with(cx, 1);
        h.mouse_on("pane-1".to_owned(), MouseButton::Right, cx);
        let labels = h.shell(cx, |s| {
            s.menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .map(|i| i.label.clone())
                .collect::<Vec<_>>()
        });
        assert!(labels.iter().any(|l| l == "Paste as text"), "{labels:?}");
        assert!(
            labels.iter().any(|l| l == "Paste image (send Ctrl+V)"),
            "{labels:?}"
        );
    }

    #[gpui_kit::test]
    fn files_dropped_on_a_pane_paste_their_quoted_paths(cx: &mut TestAppContext) {
        let (h, _dir, _) = terminal_with(cx, 1);
        let sent = script_of(&h, 1).written_text();
        cx.update(|cx| {
            h.shell.update(cx, |shell, cx| {
                shell.paste_dropped(
                    crate::ui::live::LiveId(1),
                    &[std::path::PathBuf::from("/tmp/dropped file.png")],
                    cx,
                )
            })
        });
        assert_eq!(
            &script_of(&h, 1).written_text()[sent.len()..],
            "'/tmp/dropped file.png'"
        );
    }
}

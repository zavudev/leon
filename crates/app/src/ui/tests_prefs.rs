//! Tests that each setting changes what it says it changes: the terminals,
//! the agents, the history, the engine, the window. The terminals are the
//! scripted computer of `tests.rs`, the settings live in memory or in a
//! temporary folder, and nothing waits on the clock.

use super::live::{
    local_session, open_live, put_cursor_on, real_worktree, screen, script_of, stored_session,
    terminal_of, wait_until,
};
use super::*;
use crate::schema::{self, Value};
use crate::theme::{self, Crosshairs};
use crate::ui::live::LiveId;
use gpui_kit::{Modifiers, MouseButton};
use leon_core::{AgentId, MachineId};

/// Chooses a value for a setting, as the screen does, and lets the window
/// react.
fn set(h: &Harness, cx: &mut TestAppContext, key: &str, value: Value) {
    let def = schema::find(key).unwrap_or_else(|| panic!("{key} is not a setting"));
    cx.update(|cx| settings::set_value(cx, def, value));
    h.settle(cx);
}

/// A shell in a pane that has the keyboard.
fn shell_open(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, String) {
    let h = open_live(cx);
    let (dir, path) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell's prompt", |h, cx| {
        terminal_of(h, cx, 1).is_some_and(|t| t.screen_text().contains("READY>"))
    });
    (h, dir, path)
}

fn view_of(h: &Harness, cx: &gpui_kit::App, id: u64) -> gpui_kit::Entity<leon_term::TerminalView> {
    h.shell.read(cx).live.get(LiveId(id)).unwrap().view.clone()
}

// ----- appearance ---------------------------------------------------------------------------------

#[gpui_kit::test]
fn blueprint_lines_on_draws_them_for_a_theme_that_has_few_and_off_removes_them(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    set(&h, cx, "theme_id", Value::Text("zavu".into()));
    assert_eq!(theme::lines().crosshairs, Crosshairs::Header, "Zavu's own");
    assert!(!theme::lines().corner_ticks);
    set(&h, cx, "blueprint_lines", Value::Text("on".into()));
    assert_eq!(theme::lines().crosshairs, Crosshairs::All);
    assert!(theme::lines().corner_ticks && theme::lines().empty_motif);
    set(&h, cx, "theme_id", Value::Text("leon".into()));
    set(&h, cx, "blueprint_lines", Value::Text("off".into()));
    assert_eq!(theme::lines().crosshairs, Crosshairs::Off);
    assert!(!theme::lines().corner_ticks && !theme::lines().empty_motif);
    set(&h, cx, "blueprint_lines", Value::Text("theme".into()));
    assert_eq!(theme::lines().crosshairs, Crosshairs::All, "Leon's own");
}

// ----- terminal -------------------------------------------------------------------------------------

#[gpui_kit::test]
fn changing_the_terminal_font_size_resizes_a_live_terminal(cx: &mut TestAppContext) {
    let (h, _dir, _) = shell_open(cx);
    wait_until(&h, cx, "the first paint to size it", |h, cx| {
        terminal_of(h, cx, 1).unwrap().size().cell_width > 0
    });
    let before = terminal_of(&h, cx, 1).unwrap().size();
    set(&h, cx, "terminal_font_size", Value::Int(24));
    wait_until(&h, cx, "the terminal to be resized", |h, cx| {
        terminal_of(h, cx, 1).unwrap().size().cols < before.cols
    });
    let after = terminal_of(&h, cx, 1).unwrap().size();
    assert!(after.cell_width > before.cell_width, "{before:?} {after:?}");
    assert_eq!(
        cx.update(|cx| view_of(&h, cx, 1).read(cx).font().size),
        theme::px(24.)
    );
}

#[gpui_kit::test]
fn the_line_height_setting_changes_the_rows_of_a_live_terminal(cx: &mut TestAppContext) {
    let (h, _dir, _) = shell_open(cx);
    wait_until(&h, cx, "the first paint to size it", |h, cx| {
        terminal_of(h, cx, 1).unwrap().size().cell_height > 0
    });
    let before = terminal_of(&h, cx, 1).unwrap().size();
    set(&h, cx, "terminal_line_height", Value::Int(200));
    wait_until(&h, cx, "the terminal to be resized", |h, cx| {
        terminal_of(h, cx, 1).unwrap().size().rows < before.rows
    });
}

#[gpui_kit::test]
fn the_font_family_setting_is_the_terminals_font(cx: &mut TestAppContext) {
    let (h, _dir, _) = shell_open(cx);
    let theme_mono = cx.update(|cx| view_of(&h, cx, 1).read(cx).font().family.to_string());
    set(
        &h,
        cx,
        "terminal_font_family",
        Value::Text("Geist Mono".into()),
    );
    let family = cx.update(|cx| view_of(&h, cx, 1).read(cx).font().family.to_string());
    assert_eq!(family, "Geist Mono");
    assert_ne!(family, theme_mono);
    // A family that cannot be drawn falls back to the theme's.
    set(
        &h,
        cx,
        "terminal_font_family",
        Value::Text("Not Installed".into()),
    );
    let family = cx.update(|cx| view_of(&h, cx, 1).read(cx).font().family.to_string());
    assert_eq!(family, theme_mono);
}

#[gpui_kit::test]
fn the_scrollback_setting_bounds_the_buffer_of_live_and_new_terminals(cx: &mut TestAppContext) {
    let (h, _dir, _) = shell_open(cx);
    for n in 0..400 {
        script_of(&h, 1).print(format!("line {n}\r\n"));
    }
    cx.run_until_parked();
    let history = |h: &Harness, cx: &mut TestAppContext, id: u64| {
        terminal_of(h, cx, id).unwrap().history_size()
    };
    assert!(history(&h, cx, 1) > 300);
    set(&h, cx, "terminal_scrollback", Value::Int(50));
    assert!(history(&h, cx, 1) <= 50);
    // A terminal started afterwards has the bound from its first line.
    h.press_chord("cmd-t", "ctrl-shift-t", cx);
    wait_until(&h, cx, "the second shell", |h, cx| {
        terminal_of(h, cx, 2).is_some_and(|t| t.screen_text().contains("READY>"))
    });
    for n in 0..400 {
        script_of(&h, 2).print(format!("line {n}\r\n"));
    }
    cx.run_until_parked();
    assert_eq!(history(&h, cx, 2), 50);
}

#[gpui_kit::test]
fn the_cursor_shape_setting_reaches_live_terminals(cx: &mut TestAppContext) {
    let (h, _dir, _) = shell_open(cx);
    let shape = |h: &Harness, cx: &mut TestAppContext| {
        terminal_of(h, cx, 1)
            .unwrap()
            .with_term(|term| term.cursor_style().shape)
    };
    assert_eq!(shape(&h, cx), leon_term::CursorShape::Block);
    set(&h, cx, "terminal_cursor", Value::Text("beam".into()));
    assert_eq!(shape(&h, cx), leon_term::CursorShape::Beam);
    set(&h, cx, "terminal_cursor", Value::Text("underline".into()));
    assert_eq!(shape(&h, cx), leon_term::CursorShape::Underline);
}

#[gpui_kit::test]
fn copy_on_select_copies_the_selection_when_the_mouse_is_released(cx: &mut TestAppContext) {
    let (h, _dir, _) = shell_open(cx);
    script_of(&h, 1).print("select this please\r\n");
    cx.run_until_parked();
    let pane = h.bounds_of("live-terminal".to_owned(), cx).unwrap();
    let drag = |h: &Harness, cx: &mut TestAppContext| {
        let mut visual = VisualTestContext::from_window(h.window.into(), cx);
        let y = pane.origin.y + px(8. + 18. * 0.5);
        let from = gpui_kit::point(pane.origin.x + px(9.), y);
        let to = gpui_kit::point(pane.origin.x + px(120.), y);
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        visual.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        visual.run_until_parked();
    };
    cx.update(|cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("before".into())));
    drag(&h, cx);
    assert!(
        terminal_of(&h, cx, 1).unwrap().selection_text().is_some(),
        "something was selected"
    );
    let clip =
        |cx: &mut TestAppContext| cx.update(|cx| cx.read_from_clipboard().and_then(|i| i.text()));
    assert_eq!(
        clip(cx).as_deref(),
        Some("before"),
        "off: nothing is copied"
    );
    set(&h, cx, "terminal_copy_on_select", Value::Bool(true));
    drag(&h, cx);
    let copied = clip(cx).unwrap();
    assert_ne!(copied, "before");
    assert!(copied.contains("sel") || !copied.is_empty(), "{copied:?}");
}

#[gpui_kit::test]
fn option_as_meta_off_leaves_the_composed_character_to_the_text_input(cx: &mut TestAppContext) {
    let (h, _dir, _) = shell_open(cx);
    let view = cx.update(|cx| view_of(&h, cx, 1));
    let mut stroke = gpui_kit::Keystroke::parse("alt-b").unwrap();
    stroke.key_char = Some("∫".to_owned());
    let handled = |cx: &mut TestAppContext| {
        let stroke = stroke.clone();
        let view = view.clone();
        cx.update(|cx| view.update(cx, |view, cx| view.handle_keystroke(&stroke, cx)))
    };
    let before = script_of(&h, 1).written();
    assert!(handled(cx), "Option is Meta by default");
    assert_eq!(&script_of(&h, 1).written()[before.len()..], b"\x1bb");
    set(&h, cx, "terminal_option_as_meta", Value::Bool(false));
    let before = script_of(&h, 1).written();
    assert!(!handled(cx), "the text input path gets it");
    assert_eq!(
        script_of(&h, 1).written(),
        before,
        "no escape sequence was made"
    );
}

#[gpui_kit::test]
fn the_paste_setting_decides_what_a_mixed_clipboard_pastes(cx: &mut TestAppContext) {
    use gpui_kit::{ClipboardEntry, ClipboardItem, Image, ImageFormat};
    let (h, _dir, _) = shell_open(cx);
    let item = ClipboardItem {
        entries: vec![
            ClipboardEntry::String(gpui_kit::ClipboardString::new("some text".to_owned())),
            ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, vec![1, 2, 3])),
        ],
    };
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.read_clipboard = Rc::new(move |_| Some(item.clone()));
        })
    });
    let pasted = |h: &Harness, cx: &mut TestAppContext| {
        let sent = script_of(h, 1).written();
        h.press_chord("cmd-v", "ctrl-v", cx);
        script_of(h, 1).written()[sent.len()..].to_vec()
    };
    // The shell is in front: automatic pastes the text.
    assert_eq!(pasted(&h, cx), b"some text");
    set(&h, cx, "terminal_paste", Value::Text("image".into()));
    assert_eq!(pasted(&h, cx), [0x16], "the image, when there is one");
    set(&h, cx, "terminal_paste", Value::Text("text".into()));
    assert_eq!(pasted(&h, cx), b"some text");
}

#[gpui_kit::test]
fn closing_a_running_pane_asks_only_while_the_setting_says_so(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-n", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette, "it asks");
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.live.ids().len()), 1);
    set(&h, cx, "terminal_confirm_close", Value::Bool(false));
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    assert!(
        h.shell(cx, |s| s.live.ids().is_empty()),
        "closed without asking"
    );
}

#[gpui_kit::test]
fn a_chosen_shell_and_extra_environment_start_new_terminals(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    set(&h, cx, "terminal_shell", Value::Text("/bin/sh".into()));
    set(
        &h,
        cx,
        "terminal_shell_args",
        Value::Text("-c 'exec sh -i'".into()),
    );
    set(
        &h,
        cx,
        "terminal_env",
        Value::List(vec!["EDITOR=nvim".into(), "WITH_EQUALS=a=b".into()]),
    );
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell", |h, cx| terminal_of(h, cx, 1).is_some());
    let spec = script_of(&h, 1).spec().clone();
    assert_eq!(spec.program, "/bin/sh");
    assert_eq!(spec.args, ["-c", "exec sh -i"]);
    assert!(spec.env.contains(&("EDITOR".to_owned(), "nvim".to_owned())));
    assert!(spec
        .env
        .contains(&("WITH_EQUALS".to_owned(), "a=b".to_owned())));
    // A program that is not there is refused and changes nothing.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            let def = schema::find("terminal_shell").unwrap();
            assert!(!shell.choose_setting(def, Value::Text("/no/such/shell".into()), cx));
        })
    });
    assert_eq!(
        cx.update(|cx| settings::text(cx, "terminal_shell")),
        "/bin/sh"
    );
}

#[gpui_kit::test]
fn a_bell_marks_a_background_session_only_when_the_setting_says_so(cx: &mut TestAppContext) {
    let (h, _dir, _) = shell_open(cx);
    h.press_chord("cmd-t", "ctrl-shift-t", cx);
    wait_until(&h, cx, "the second shell", |h, cx| {
        terminal_of(h, cx, 2).is_some()
    });
    let rings = |h: &Harness, cx: &mut TestAppContext| {
        script_of(h, 1).print("\x07");
        cx.run_until_parked();
        h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().bell)
    };
    assert!(
        rings(&h, cx),
        "the first shell is in the background and rang"
    );
    set(&h, cx, "terminal_bell_mark", Value::Bool(false));
    cx.update(|cx| {
        h.shell
            .update(cx, |s, _| s.live.get_mut(LiveId(1)).unwrap().bell = false)
    });
    assert!(!rings(&h, cx), "off: no mark");
}

// ----- agents -----------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_disabled_agent_is_not_offered_for_new_sessions(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    set(&h, cx, "agent_codex_enabled", Value::Bool(false));
    h.press("ctrl-n", cx);
    let titles = h.palette_titles(cx);
    assert_eq!(titles[..2], ["Claude Code", "opencode"].map(str::to_owned));
    assert!(
        !titles.contains(&"Codex".to_owned()),
        "a disabled agent is gone, not dimmed"
    );
}

#[gpui_kit::test]
fn the_long_agent_list_is_filtered_as_you_type_and_an_agent_that_is_missing_says_where_to_get_it(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-n", cx);
    let all = h.palette_titles(cx);
    assert!(
        all.len() > 30,
        "the whole catalogue is on offer: {}",
        all.len()
    );
    h.type_text("mistral", cx);
    let narrowed = h.palette_titles(cx);
    assert_eq!(
        narrowed.first().map(String::as_str),
        Some("Mistral Vibe"),
        "{narrowed:?}"
    );
    assert!(narrowed.len() < 6, "{narrowed:?}");
    h.press("enter", cx); // the fake computer has no `vibe`
    h.settle(cx);
    assert_eq!(
        h.status(),
        "Mistral Vibe is not installed on This machine. Install it from https://github.com/mistralai/mistral-vibe."
    );
}

#[gpui_kit::test]
fn the_default_agent_starts_without_a_question(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    set(&h, cx, "default_agent", Value::Text("codex".into()));
    h.press("ctrl-n", cx);
    wait_until(&h, cx, "codex", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CODEX")
    });
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn a_new_claude_session_starts_without_the_permission_questions(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Claude Code
    wait_until(&h, cx, "the line typed", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --dangerously-skip-permissions")
    });
}

#[gpui_kit::test]
fn a_new_codex_session_starts_without_the_approval_questions(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    set(&h, cx, "agent_claude_enabled", Value::Bool(false));
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Codex, the only agent on offer
    wait_until(&h, cx, "the line typed", |h, cx| {
        screen(h, cx, 1).contains("codex --ask-for-approval never")
    });
}

#[gpui_kit::test]
fn clearing_the_codex_arguments_brings_the_approval_questions_back(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    set(&h, cx, "agent_claude_enabled", Value::Bool(false));
    set(&h, cx, "agent_codex_args", Value::Text(String::new()));
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Codex
    wait_until(&h, cx, "the line typed", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CODEX")
    });
    assert!(
        !screen(&h, cx, 1).contains("--ask-for-approval"),
        "{}",
        screen(&h, cx, 1)
    );
}

#[gpui_kit::test]
fn arguments_of_the_user_replace_the_default_of_the_agent(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    set(&h, cx, "agent_claude_enabled", Value::Bool(false));
    set(
        &h,
        cx,
        "agent_codex_args",
        Value::Text("--ask-for-approval on-request --model o3".into()),
    );
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Codex
    wait_until(&h, cx, "the line typed", |h, cx| {
        screen(h, cx, 1).contains("codex --ask-for-approval on-request --model o3")
    });
    assert!(
        !screen(&h, cx, 1).contains("--ask-for-approval never"),
        "{}",
        screen(&h, cx, 1)
    );
}

#[gpui_kit::test]
fn clearing_the_arguments_brings_the_permission_questions_back(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    set(&h, cx, "agent_claude_args", Value::Text(String::new()));
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Claude Code
    wait_until(&h, cx, "the line typed", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    assert!(
        !screen(&h, cx, 1).contains("--dangerously-skip-permissions"),
        "{}",
        screen(&h, cx, 1)
    );
}

#[gpui_kit::test]
fn the_executable_override_and_the_extra_arguments_are_typed_for_a_new_session(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    set(
        &h,
        cx,
        "agent_claude_executable",
        Value::Text("/bin/sh".into()),
    );
    set(
        &h,
        cx,
        "agent_claude_args",
        Value::Text("--model 'big one'".into()),
    );
    h.press("ctrl-n", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the line typed", |h, cx| {
        screen(h, cx, 1).contains("/bin/sh --model 'big one'")
    });
}

#[gpui_kit::test]
fn extra_resume_arguments_are_typed_into_the_shell(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _path, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
    set(
        &h,
        cx,
        "agent_claude_resume_args",
        Value::Text("--model opus".into()),
    );
    set(
        &h,
        cx,
        "agent_claude_args",
        Value::Text("--not-this".into()),
    );
    put_cursor_on(&h, cx, NodeId::Session(id));
    h.press("enter", cx);
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3 --model opus")
    });
    assert!(!screen(&h, cx, 1).contains("--not-this"));
}

#[gpui_kit::test]
fn open_transcript_mode_makes_enter_show_the_transcript(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _path, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
    set(&h, cx, "history_open", Value::Text("transcript".into()));
    put_cursor_on(&h, cx, NodeId::Session(id));
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "session:alpha");
    assert_eq!(
        h.shell(cx, |s| s.live.ids().len()),
        0,
        "nothing was started"
    );
    // Enter in the transcript still resumes it.
    h.press("tab", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
}

// ----- sessions and history --------------------------------------------------------------------------

#[gpui_kit::test]
fn sessions_per_worktree_sets_how_many_show_before_show_more(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    for n in 0..6 {
        stored_session(
            &h,
            cx,
            AgentId::CLAUDE,
            &MachineId::local(),
            &path,
            &format!("history {n}"),
        );
    }
    h.settle(cx);
    let rows = |h: &Harness, cx: &mut TestAppContext| {
        let outline = h.outline(cx);
        let shown = outline
            .iter()
            .filter(|r| r.contains("session:history"))
            .count();
        let more = outline
            .iter()
            .find_map(|r| r.trim_start().strip_prefix("more:").map(str::to_owned));
        (shown, more)
    };
    // The worktree is open (a session of the week ran in it) and shows all six.
    assert_eq!(rows(&h, cx), (6, None));
    set(&h, cx, "sessions_per_worktree", Value::Int(3));
    assert_eq!(rows(&h, cx), (3, Some("3".to_owned())));
    set(&h, cx, "sessions_per_worktree", Value::Int(4));
    assert_eq!(rows(&h, cx), (4, Some("2".to_owned())));
}

#[gpui_kit::test]
fn detecting_elsewhere_off_stops_the_scans_and_the_interval_is_the_settings(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            // Not scanning without a scanner: the setting is the second gate.
            assert!(!shell.detecting_elsewhere(cx));
            assert_eq!(
                Shell::elsewhere_interval(cx),
                std::time::Duration::from_secs(5)
            );
        })
    });
    set(&h, cx, "elsewhere_interval", Value::Int(30));
    assert_eq!(
        cx.update(|cx| Shell::elsewhere_interval(cx)),
        std::time::Duration::from_secs(30),
        "the timer waits what the setting says"
    );
    set(&h, cx, "detect_elsewhere", Value::Bool(false));
    assert!(!cx.update(|cx| settings::flag(cx, "detect_elsewhere")));
}

// ----- window -----------------------------------------------------------------------------------------

#[gpui_kit::test]
fn quit_confirmation_never_quits_without_asking(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-n", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    let quits = Rc::new(std::cell::Cell::new(0));
    let counted = quits.clone();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.quit = Rc::new(move |_| counted.set(counted.get() + 1));
        })
    });
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    assert_eq!(
        h.shell(cx, |s| s.overlay),
        Overlay::Palette,
        "running: it asks"
    );
    assert_eq!(quits.get(), 0);
    h.press("escape", cx);
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    set(&h, cx, "quit_confirmation", Value::Text("never".into()));
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    // No question; the running agent is given its chance to save first, and
    // the quit follows within the grace (never held up longer).
    // Let the watcher register its timer before the clock moves.
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(quits.get(), 1, "quit without asking");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn quit_confirmation_always_asks_even_with_nothing_running(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let quits = Rc::new(std::cell::Cell::new(0));
    let counted = quits.clone();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.quit = Rc::new(move |_| counted.set(counted.get() + 1));
        })
    });
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    assert_eq!(quits.get(), 1, "nothing runs: no question by default");
    set(&h, cx, "quit_confirmation", Value::Text("always".into()));
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert_eq!(quits.get(), 1);
    h.press("enter", cx);
    assert_eq!(quits.get(), 2);
}

#[gpui_kit::test]
fn the_sidebar_settings_hide_and_size_the_sidebar(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    set(&h, cx, "sidebar_width", Value::Int(400));
    assert_eq!(cx.update(|cx| settings::get(cx).sidebar_width), 400);
    assert_eq!(
        h.bounds_of("sidebar".to_owned(), cx).unwrap().size.width,
        px(400.)
    );
    set(&h, cx, "sidebar_visible", Value::Bool(false));
    assert!(h.bounds_of("sidebar".to_owned(), cx).is_none());
}

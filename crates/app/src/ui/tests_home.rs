//! Tests of the home in the window: King's and the Den's rows of the sidebar
//! are always there, the machine's menu, the command and the chord go home from anywhere, every action
//! of the home does what its command does, and the keyboard walks it.
//!
//! The terminals that survive a visit to the home are in `tests_den.rs`,
//! with the other tests that need the scripted computer.

use super::*;
use crate::ui::home::{Item, ACTIONS};

fn go_home(h: &Harness, cx: &mut TestAppContext) {
    h.press_chord("cmd-alt-shift-h", "ctrl-alt-shift-h", cx);
}

fn click(h: &Harness, selector: &str, cx: &mut TestAppContext) {
    h.mouse_on(selector.to_owned(), gpui_kit::MouseButton::Left, cx);
}

fn at(h: &Harness, cx: &mut TestAppContext) -> usize {
    h.shell(cx, |shell| shell.home.at)
}

fn open_project(h: &Harness, cx: &mut TestAppContext) {
    h.press("ctrl-p", cx);
    h.type_text("api", cx);
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "project:api");
}

#[gpui_kit::test]
fn king_is_pinned_over_the_filter_whatever_is_filtered_or_scrolled(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    assert!(h.shows("sidebar-nav", cx) && h.shows("sidebar-king", cx));
    // Home and the board are in the machine's menu, not rows; the Den is a
    // row next to King's.
    for gone in ["sidebar-home", "sidebar-tasks"] {
        assert!(!h.shows(gone, cx), "{gone}");
    }
    assert!(h.shows("sidebar-den", cx));
    // Header, King, the Den, filter, tree.
    let bounds = |selector: &str, cx: &mut TestAppContext| {
        h.bounds_of(selector.to_owned(), cx)
            .unwrap_or_else(|| panic!("{selector} is drawn"))
    };
    let (header, king, den, filter, first) = (
        bounds("sidebar-header", cx),
        bounds("sidebar-king", cx),
        bounds("sidebar-den", cx),
        bounds("sidebar-filter", cx),
        bounds("tree-row-0", cx),
    );
    assert!(header.bottom() <= king.top());
    assert!(king.bottom() <= den.top(), "the Den is under King");
    assert!(den.bottom() <= filter.top() && filter.bottom() <= first.top());
    // Their rows are rows of the tree: the same height, the same edges.
    for row in [king, den] {
        assert_eq!(row.size.height, first.size.height);
        assert_eq!((row.left(), row.right()), (first.left(), first.right()));
    }

    // A filter that matches nothing leaves it there.
    h.press("/", cx);
    h.type_text("no project is called this", cx);
    assert!(!h.outline(cx).iter().any(|row| row.contains("project:")));
    assert_eq!(bounds("sidebar-king", cx), king);
    assert_eq!(bounds("sidebar-den", cx), den);
    h.press("escape", cx);
    h.press("escape", cx);

    // So does the tree scrolled to its end.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            let last = shell.rows.len() - 1;
            shell.move_cursor_to(last);
            shell.tree_scroll.scroll_to_reveal_item(last);
            cx.notify();
        })
    });
    h.settle(cx);
    assert_eq!(bounds("sidebar-king", cx), king);
    assert_eq!(bounds("sidebar-den", cx), den);

    // No chord is printed in a row: the tooltip says it, as the header's
    // "+" does.
    use crate::ui::home::Nav;
    for nav in Nav::ALL {
        let tip = crate::ui::sidebar::tooltip_text(nav.command());
        let chord = crate::keys::keys_label(nav.command()).expect("it has a chord");
        assert!(
            tip.starts_with(crate::keys::label(nav.command())) && tip.ends_with(&chord),
            "{tip}"
        );
    }
    assert_eq!(
        (Nav::Home.label(), Nav::Den.label()),
        ("Home", "The Office")
    );
    assert_eq!(Nav::ALL, [Nav::King, Nav::Den], "King, then the Den");
}

#[gpui_kit::test]
fn the_cursor_of_the_sidebar_follows_what_the_main_pane_shows(cx: &mut TestAppContext) {
    use crate::ui::home::Nav;
    let h = open(cx, ScriptedRunner::new());
    let nav = |cx: &mut TestAppContext| h.shell(cx, |shell| shell.home.nav);
    assert_eq!(nav(cx), None, "at the start the cursor is in the tree");
    // The machine's menu goes to the places without a row.
    h.pick_in_machine_menu(Command::ShowDen, cx);
    assert_eq!((h.main_kind(cx).as_str(), nav(cx)), ("den", Some(Nav::Den)));
    h.pick_in_machine_menu(Command::GoHome, cx);
    assert_eq!(
        (h.main_kind(cx).as_str(), nav(cx)),
        ("empty", Some(Nav::Home))
    );
    // The chords move it too.
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    assert_eq!(nav(cx), Some(Nav::Den));
    h.press("escape", cx);
    assert_eq!(
        (h.main_kind(cx).as_str(), nav(cx)),
        ("empty", Some(Nav::Home))
    );
    // Something of the tree opened: the cursor is in the tree again, and
    // closing the Den over it leaves it there.
    open_project(&h, cx);
    assert_eq!(nav(cx), None);
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    assert_eq!(nav(cx), Some(Nav::Den));
    h.press("escape", cx);
    assert_eq!((h.main_kind(cx).as_str(), nav(cx)), ("project:api", None));
    // At home there is no row to stand on: the tree keeps its cursor.
    go_home(&h, cx);
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.pane = Pane::Sidebar;
            cx.notify();
        })
    });
    assert!(!h.shows("sidebar-nav-cursor", cx) && h.shows("tree-cursor", cx));
    // One cursor at a time: on King's row, the tree draws none.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.home.nav = Some(Nav::King);
            cx.notify();
        })
    });
    assert!(h.shows("sidebar-nav-cursor", cx) && !h.shows("tree-cursor", cx));
    // An empty den: nobody waits.
    assert!(!h.shows("sidebar-den-waits", cx));
}

#[gpui_kit::test]
fn the_menu_the_chord_and_the_command_go_home_from_anywhere(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    // The machine's menu.
    open_project(&h, cx);
    h.pick_in_machine_menu(Command::GoHome, cx);
    assert_eq!(h.main_kind(cx), "empty");
    assert!(h.shows("main-empty", cx) && h.shows("home-actions", cx));
    assert_eq!(
        h.shell(cx, |shell| shell.pane),
        Pane::Main,
        "the home has the keyboard"
    );
    // The chord.
    open_project(&h, cx);
    go_home(&h, cx);
    assert_eq!(h.main_kind(cx), "empty");
    // The palette.
    open_project(&h, cx);
    h.press("ctrl-shift-p", cx);
    h.type_text("go home", cx);
    assert_eq!(h.palette_titles(cx)[1], "Go home");
    assert!(
        h.shows("palette-keys-1", cx),
        "its chord is printed beside it"
    );
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "empty");
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::None);
    // From the Den: the Den is left, and its chord opens it again.
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    assert_eq!(h.main_kind(cx), "den");
    go_home(&h, cx);
    assert_eq!(h.main_kind(cx), "empty");
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    assert_eq!(h.main_kind(cx), "den");
    h.press("escape", cx);
    assert_eq!(
        h.main_kind(cx),
        "empty",
        "closing the Den comes back to the home"
    );
    // Over a sheet: the sheet closes.
    h.press("ctrl-shift-p", cx);
    go_home(&h, cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::None);
    // At home already, it stays.
    go_home(&h, cx);
    assert_eq!(h.main_kind(cx), "empty");
    // The heading says where one is.
    let heading = cx.update(|cx| h.shell.read(cx).main_heading(cx));
    assert_eq!(
        (heading.0, heading.1.as_str(), heading.2.as_str()),
        ("Home", "Home", "No session is running")
    );
}

#[gpui_kit::test]
fn king_is_the_first_stop_of_the_sidebar(cx: &mut TestAppContext) {
    use crate::ui::home::Nav;
    let h = open(cx, ScriptedRunner::new());
    open_project(&h, cx);
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.pane = Pane::Sidebar;
            shell.move_cursor_to(0);
            cx.notify();
        })
    });
    let nav = |cx: &mut TestAppContext| h.shell(cx, |shell| shell.home.nav);
    assert!(!h.shows("sidebar-nav-cursor", cx));
    // Up from the first row of the tree is the Den's, the nearest row of the
    // navigation; King's is above it, and there the cursor stays.
    h.press("up", cx);
    assert_eq!(nav(cx), Some(Nav::Den));
    assert!(h.shows("sidebar-nav-cursor", cx));
    assert!(
        !h.shows("tree-cursor", cx),
        "the tree has no cursor meanwhile"
    );
    h.press("up", cx);
    assert_eq!(nav(cx), Some(Nav::King));
    h.press("up", cx);
    assert_eq!(nav(cx), Some(Nav::King));
    assert_eq!(
        h.main_kind(cx),
        "project:api",
        "moving the cursor opens nothing"
    );
    // Down walks to the Den and then back into the tree, at its first row.
    h.press("down", cx);
    assert_eq!(nav(cx), Some(Nav::Den));
    h.press("down", cx);
    assert_eq!(nav(cx), None);
    assert_eq!(h.cursor_row(cx), "machine:This machine");
    assert!(h.shows("tree-cursor", cx));
    // The key that goes to the first row of the tree still does, from the
    // rows of the navigation too.
    h.press("up", cx);
    h.press("up", cx);
    h.press("end", cx);
    h.press("home", cx);
    assert_eq!(h.cursor_row(cx), "machine:This machine");
    assert_eq!(nav(cx), None);
    // Enter on King's row opens King.
    h.press("up", cx);
    h.press("up", cx);
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "king");
}

#[gpui_kit::test]
fn the_den_is_a_row_of_the_sidebar_that_opens_it_and_shows_it_selected(cx: &mut TestAppContext) {
    use crate::ui::home::Nav;
    let h = open(cx, ScriptedRunner::new());
    let nav = |cx: &mut TestAppContext| h.shell(cx, |shell| shell.home.nav);
    // At the top of the sidebar, outside any machine, with its own name.
    assert!(h.shows("sidebar-den", cx));
    assert_eq!(nav(cx), None);
    assert_eq!(h.main_kind(cx), "empty");

    // A click opens it, and its row is the selected one.
    click(&h, "sidebar-den", cx);
    assert_eq!(h.main_kind(cx), "den");
    assert_eq!(nav(cx), Some(Nav::Den));
    // With the keyboard in the sidebar, the cursor's bar is on that row.
    h.press_chord("cmd-l", "ctrl-l", cx);
    assert!(h.shows("sidebar-nav-cursor", cx) && !h.shows("tree-cursor", cx));
    // Another click on it leaves the Den open: it is a place, not a switch.
    click(&h, "sidebar-den", cx);
    assert_eq!(h.main_kind(cx), "den");

    // Its chord closes it and the row is not selected any more.
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    assert_eq!(h.main_kind(cx), "empty");
    open_project(&h, cx);
    assert_eq!(nav(cx), None);
    assert!(!h.shows("sidebar-nav-cursor", cx));

    // From the keyboard: Up from the tree to the Den's row, Enter on it.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.pane = Pane::Sidebar;
            shell.move_cursor_to(0);
            cx.notify();
        })
    });
    h.press("up", cx);
    assert_eq!(nav(cx), Some(Nav::Den));
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "den");
    assert_eq!(nav(cx), Some(Nav::Den));
    // The row says how many lions there are; none, none.
    assert!(!h.shows("sidebar-den-count", cx));
}

#[gpui_kit::test]
fn every_action_of_the_home_is_a_button_that_runs_its_command(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    go_home(&h, cx);
    let before = h.bounds_of("hint-ShowDen".to_owned(), cx).unwrap();
    for command in ACTIONS {
        go_home(&h, cx);
        let selector = format!("hint-{command:?}");
        assert!(h.shows_dynamic(selector.clone(), cx), "{selector}");
        let status = h.status();
        click(&h, &selector, cx);
        let (overlay, main) = (h.shell(cx, |shell| shell.overlay), h.main_kind(cx));
        let expected = match command {
            Command::ShowDen => main == "den",
            // The folder picker of the tests answers "cancelled": nothing
            // to see. That the click reached the button is checked below.
            Command::OpenProject => true,
            Command::AddMachine => matches!(overlay, Overlay::Connect | Overlay::Pair),
            Command::Shortcuts => overlay == Overlay::Shortcuts,
            Command::ShowUsage => overlay == Overlay::Usage,
            Command::Settings => overlay == Overlay::Settings,
            Command::GoTo | Command::Commands | Command::SearchHistory => {
                overlay == Overlay::Palette
            }
            // With the keyboard on no worktree it asks where, or says why not.
            _ => overlay != Overlay::None || h.status() != status,
        };
        assert!(
            expected,
            "{command:?}: {overlay:?}, {main}, {:?}",
            h.status()
        );
        // The keyboard is on the button that was clicked.
        let on = ACTIONS.iter().position(|one| *one == command).unwrap();
        assert_eq!(at(&h, cx), on, "{command:?}");
    }
    // The Den is first, and every action is a chip of one height, under
    // the field that goes anywhere.
    go_home(&h, cx);
    let den = h.bounds_of("hint-ShowDen".to_owned(), cx).unwrap();
    let next = h.bounds_of("hint-NewSession".to_owned(), cx).unwrap();
    let prompt = h.bounds_of("home-prompt".to_owned(), cx).unwrap();
    assert_eq!(den, before);
    assert!(den.left() <= next.left() || den.top() < next.top());
    assert_eq!(den.size.height, next.size.height);
    assert!(prompt.bottom() <= den.top());
}

#[gpui_kit::test]
fn the_arrows_walk_the_home_and_enter_does_what_a_click_does(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    go_home(&h, cx);
    assert_eq!(at(&h, cx), 0, "the keyboard starts on the Den");
    h.press("up", cx);
    assert_eq!(at(&h, cx), 0);
    h.press("down", cx);
    h.press("down", cx);
    assert_eq!(at(&h, cx), 2);
    h.press("k", cx);
    assert_eq!(at(&h, cx), 1);
    // Past the actions come the recent sessions of the history.
    h.press("end", cx);
    let last = at(&h, cx);
    let recents = h.shell(cx, |shell| shell.snapshot.sessions.len().min(5));
    assert!(recents > 0);
    assert_eq!(last, ACTIONS.len() + recents - 1);
    assert!(h.shows_dynamic(format!("home-recent-{}", recents - 1), cx));
    h.press("down", cx);
    assert_eq!(at(&h, cx), last, "it stops at the end");
    h.press("home", cx);
    assert_eq!(at(&h, cx), 0);
    // Enter on the first opens the Den.
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "den");
    // Enter on an action further down runs that one.
    go_home(&h, cx);
    let shortcuts = ACTIONS
        .iter()
        .position(|command| *command == Command::Shortcuts)
        .unwrap();
    for _ in 0..shortcuts {
        h.press("down", cx);
    }
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Shortcuts);
    // While the keyboard is in the sidebar the arrows are the tree's.
    h.press("escape", cx);
    go_home(&h, cx);
    cx.update(|cx| h.shell.update(cx, |shell, _| shell.pane = Pane::Sidebar));
    h.press("down", cx);
    assert_eq!(at(&h, cx), 0);
}

#[gpui_kit::test]
fn a_recent_session_opens_from_the_home_as_from_the_sidebar(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    go_home(&h, cx);
    assert!(h.shows("home-sessions", cx) && h.shows("home-recent", cx));
    assert!(h.shows("home-headline", cx));
    // What its row of the sidebar does on Enter.
    let (id, title) = h.shell(cx, |shell| {
        let session = &shell.snapshot.sessions[0];
        (session.id.clone(), session.title.clone())
    });
    put_cursor(&h, cx, NodeId::Session(id.clone()));
    h.press("enter", cx);
    let from_sidebar = (
        h.shell(cx, |shell| shell.overlay),
        h.main_kind(cx),
        h.palette_titles(cx),
    );
    h.press("escape", cx);

    go_home(&h, cx);
    click(&h, "home-recent-0", cx);
    let from_home = (
        h.shell(cx, |shell| shell.overlay),
        h.main_kind(cx),
        h.palette_titles(cx),
    );
    assert_eq!(from_home, from_sidebar, "{title}");
    assert_ne!(
        (from_home.0, from_home.1.as_str()),
        (Overlay::None, "empty"),
        "something opened or was asked"
    );
    // The cursor of the sidebar is on that session, as if it had been
    // chosen there.
    assert_eq!(h.cursor_row(cx), format!("session:{title}"));
    assert_eq!(
        h.shell(cx, |shell| shell.home.at),
        ACTIONS.len(),
        "the first item after the actions: {:?}",
        Item::Recent(0)
    );
}

#[gpui_kit::test]
fn the_home_fits_a_narrow_pane_and_a_very_wide_one(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    go_home(&h, cx);
    let mut seen = Vec::new();
    for width in [620., 700., 1280., 3440.] {
        {
            let visual = VisualTestContext::from_window(h.window.into(), cx);
            visual.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(900.)));
            visual.run_until_parked();
        }
        h.settle(cx);
        let pane = h.bounds_of("main-empty".to_owned(), cx).unwrap();
        seen.push(pane.size.width);
        for command in ACTIONS {
            let button = h
                .bounds_of(format!("hint-{command:?}"), cx)
                .unwrap_or_else(|| panic!("{command:?} at {width}"));
            assert!(
                button.left() >= pane.left() && button.right() <= pane.right(),
                "{command:?} at {width}: {button:?} in {pane:?}"
            );
            // A chip is as wide as its word and its chord (macOS's chords
            // are the shortest), never squeezed under them.
            assert!(
                button.size.width > gpui_kit::px(96.),
                "{command:?} at {width}: {button:?}"
            );
        }
        for block in ["home-sessions", "home-recent", "maker"] {
            let bounds = h.bounds_of(block.to_owned(), cx).unwrap();
            assert!(
                bounds.left() >= pane.left() && bounds.right() <= pane.right(),
                "{block} at {width}"
            );
        }
    }
    assert!(
        seen[0] < gpui_kit::px(400.),
        "a narrow pane was really tried: {seen:?}"
    );
    assert!(
        seen[3] > gpui_kit::px(3000.),
        "and a very wide one: {seen:?}"
    );
}

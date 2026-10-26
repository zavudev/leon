//! The board of tasks in the window: how it opens, the keys that carry a
//! card, the drag with the pointer, the question that creates a task, and the
//! reading of what another process wrote.

use super::*;
use crate::task_board::Which;
use crate::ui::tasks_view::REFRESH;
use leon_core::{MemoryScope, NewTask, TaskStatus};

/// The window with the keyboard on the project "api": its board is shown.
fn on_api(cx: &mut TestAppContext) -> Harness {
    let h = open(cx, ScriptedRunner::new());
    let api = project_named(&h, "api");
    put_cursor(&h, cx, NodeId::Project(api));
    h
}

fn board_scope(h: &Harness, cx: &mut TestAppContext) -> MemoryScope {
    match h.shell(cx, |s| s.tasks_ui.which.clone()) {
        Which::Project(root) => MemoryScope::project(root),
        Which::Global => MemoryScope::Global,
    }
}

fn open_board(h: &Harness, cx: &mut TestAppContext) {
    h.press_chord("cmd-shift-i", "ctrl-shift-alt-i", cx);
    assert_eq!(h.main_kind(cx), "tasks");
}

/// What `leon task add` does from a terminal, on the board that is open:
/// a write the window's own engine knows nothing about, and the board
/// reading it at its next turn.
fn written_elsewhere(h: &Harness, title: &str, at: i64, cx: &mut TestAppContext) -> String {
    let scope = board_scope(h, cx);
    let id = h
        .store
        .add_task(&NewTask::titled(scope, title), at)
        .unwrap()
        .into_task()
        .id;
    wait_for_the_board(h, cx);
    id
}

fn wait_for_the_board(h: &Harness, cx: &mut TestAppContext) {
    cx.executor().advance_clock(REFRESH * 2);
    h.settle(cx);
}

fn status_of(h: &Harness, id: &str) -> TaskStatus {
    h.store.task(id).unwrap().status
}

fn titles(h: &Harness, column: usize, cx: &mut TestAppContext) -> Vec<String> {
    h.shell(cx, |s| {
        s.tasks_ui.board.columns()[column]
            .iter()
            .map(|task| task.title.clone())
            .collect()
    })
}

/// Drags the card drawn at `from` to a point of `to`, `fraction` of the way
/// down it, and lets go there.
fn drag_card(h: &Harness, from: &str, to: &str, fraction: f32, cx: &mut TestAppContext) {
    let from = h
        .bounds_of(from.to_owned(), cx)
        .unwrap_or_else(|| panic!("{from} is not drawn"));
    let to = h
        .bounds_of(to.to_owned(), cx)
        .unwrap_or_else(|| panic!("{to} is not drawn"));
    let point = gpui_kit::point(to.center().x, to.origin.y + to.size.height * fraction);
    cx.update_window(h.window.into(), |_, window, cx| {
        window.drag(from.center(), point, cx);
    })
    .unwrap();
    h.settle(cx);
    wait_for_the_board(h, cx);
}

#[gpui_kit::test]
fn the_chord_opens_the_board_of_the_project_and_escape_puts_back_what_was_there(
    cx: &mut TestAppContext,
) {
    let h = on_api(cx);
    let before = h.main_kind(cx);
    open_board(&h, cx);
    assert!(h.shows("tasks", cx));
    // An empty board is the lion and what to do; the columns come with the
    // first task.
    assert!(h.shows("tasks-hint", cx));
    written_elsewhere(&h, "First", 1, cx);
    for column in 0..5 {
        assert!(
            h.shows_dynamic(format!("tasks-column-{column}"), cx),
            "{column}"
        );
    }
    assert!(matches!(
        h.shell(cx, |s| s.tasks_ui.which.clone()),
        Which::Project(_)
    ));
    // The sidebar stays beside it.
    assert!(h.shows("sidebar-nav", cx) && h.shows("sidebar-machine", cx));
    h.press("escape", cx);
    assert_eq!(h.main_kind(cx), before);
    // The same chord closes it.
    open_board(&h, cx);
    h.press_chord("cmd-shift-i", "ctrl-shift-alt-i", cx);
    assert_eq!(h.main_kind(cx), before);
}

#[gpui_kit::test]
fn the_machine_menu_opens_and_closes_the_board(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.pick_in_machine_menu(crate::keys::Command::ShowTasks, cx);
    assert_eq!(h.main_kind(cx), "tasks");
    h.pick_in_machine_menu(crate::keys::Command::ShowTasks, cx);
    assert_ne!(h.main_kind(cx), "tasks");
}

#[gpui_kit::test]
fn the_palette_offers_the_board_and_a_new_task(cx: &mut TestAppContext) {
    let h = on_api(cx);
    h.press_chord("cmd-shift-p", "ctrl-shift-p", cx);
    h.set_palette_text(">task", cx);
    let titles = h.palette_titles(cx);
    assert!(
        titles.iter().any(|t| t == "Show the task board"),
        "{titles:?}"
    );
    assert!(titles.iter().any(|t| t == "New task\u{2026}"), "{titles:?}");
}

#[gpui_kit::test]
fn from_the_home_the_first_project_of_this_computer_is_shown(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_board(&h, cx);
    assert!(matches!(
        h.shell(cx, |s| s.tasks_ui.which.clone()),
        Which::Project(_)
    ));
    // `g` goes to the global board and back.
    h.press("g", cx);
    assert_eq!(h.shell(cx, |s| s.tasks_ui.which.clone()), Which::Global);
    h.press("g", cx);
    assert!(matches!(
        h.shell(cx, |s| s.tasks_ui.which.clone()),
        Which::Project(_)
    ));
}

#[gpui_kit::test]
fn the_project_chips_switch_the_board(cx: &mut TestAppContext) {
    let h = on_api(cx);
    let web_root = h
        .store
        .projects(Some(&MachineId::local()))
        .unwrap()
        .into_iter()
        .find(|project| project.name == "web")
        .unwrap()
        .root;
    h.store
        .add_task(
            &NewTask::titled(MemoryScope::project(web_root), "Only for web"),
            1,
        )
        .unwrap();
    open_board(&h, cx);
    assert!(h.shell(cx, |s| s.tasks_ui.board.is_empty()));
    // The chips are the projects of this computer by name, then Global.
    let chips = h.shell(cx, |s| s.board_projects().len());
    assert!(chips >= 2);
    let web = (0..chips)
        .find(|index| {
            h.shell(cx, |s| {
                s.board_projects()
                    .get(*index)
                    .is_some_and(|(_, name, _)| name == "web")
            })
        })
        .unwrap();
    h.mouse_on(
        format!("tasks-project-{web}"),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(titles(&h, 1, cx), ["Only for web"]);
    h.mouse_on("tasks-global".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.shell(cx, |s| s.tasks_ui.which.clone()), Which::Global);
}

#[gpui_kit::test]
fn a_task_written_by_another_process_appears_when_the_board_reads_again(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    assert!(h.shell(cx, |s| s.tasks_ui.board.is_empty()));
    assert!(h.shows("tasks-hint", cx));
    written_elsewhere(&h, "Cover the retry path", 1, cx);
    assert_eq!(h.shell(cx, |s| s.tasks_ui.board.len()), 1);
    assert!(h.shows_dynamic("tasks-card-1-0".to_owned(), cx));
}

#[gpui_kit::test]
fn the_keys_carry_a_card_between_columns_and_finish_it(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    let id = written_elsewhere(&h, "Ship the board", 1, cx);
    assert_eq!(h.shell(cx, |s| s.tasks_ui.board.cursor()), (1, 0));
    h.press("shift-right", cx);
    wait_for_the_board(&h, cx);
    assert_eq!(status_of(&h, &id), TaskStatus::Doing);
    assert!(h.status().contains("Moved"), "{}", h.status());
    // The cursor followed the card to its new column.
    assert_eq!(h.shell(cx, |s| s.tasks_ui.board.cursor()), (2, 0));
    h.press("d", cx);
    wait_for_the_board(&h, cx);
    assert_eq!(status_of(&h, &id), TaskStatus::Done);
    assert!(h.store.task(&id).unwrap().closed_at.is_some());
}

#[gpui_kit::test]
fn a_card_moves_up_and_down_inside_its_column_by_key(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    for (n, title) in ["A", "B", "C"].into_iter().enumerate() {
        written_elsewhere(&h, title, n as i64, cx);
    }
    assert_eq!(titles(&h, 1, cx), ["A", "B", "C"]);
    h.press("shift-down", cx);
    wait_for_the_board(&h, cx);
    assert_eq!(titles(&h, 1, cx), ["B", "A", "C"]);
    h.press("shift-down", cx);
    wait_for_the_board(&h, cx);
    assert_eq!(titles(&h, 1, cx), ["B", "C", "A"]);
    h.press("shift-up", cx);
    wait_for_the_board(&h, cx);
    assert_eq!(titles(&h, 1, cx), ["B", "A", "C"]);
}

#[gpui_kit::test]
fn p_raises_the_priority_of_the_card(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    let id = written_elsewhere(&h, "Tune the query", 1, cx);
    h.press("p", cx);
    assert_eq!(
        h.store.task(&id).unwrap().priority,
        leon_core::TaskPriority::Low
    );
    assert!(h.status().contains("Priority"), "{}", h.status());
}

/// Whether a field of the board has the keyboard.
fn typing(h: &Harness, cx: &mut TestAppContext) -> bool {
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.read(cx).tasks_typing(window, cx)
    })
    .unwrap()
}

fn create_field(
    h: &Harness,
    cx: &mut TestAppContext,
    pick: impl Fn(
        &crate::ui::task_create::CreateUi,
    ) -> gpui_kit::Entity<gpui_kit::component::input::InputState>,
) -> gpui_kit::Entity<gpui_kit::component::input::InputState> {
    h.shell(cx, |s| pick(s.tasks_ui.create.as_ref().expect("the panel")))
}

#[gpui_kit::test]
fn n_opens_the_panel_for_a_new_task_with_the_title_ready_to_type(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    written_elsewhere(&h, "Already here", 1, cx);
    // The cursor is in the doing column: the new task starts there.
    h.press("right", cx);
    h.press("n", cx);
    // The panel, not the palette, and the keyboard is already in the title.
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert!(h.shows("tasks-create", cx));
    assert!(typing(&h, cx), "the title field has the keyboard");
    h.type_text("Write the migration guide", cx);
    h.press("enter", cx);
    assert!(!h.shows("tasks-create", cx));
    assert_eq!(h.main_kind(cx), "tasks");
    wait_for_the_board(&h, cx);
    let scope = board_scope(&h, cx);
    let tasks = h
        .store
        .tasks(&scope, &leon_core::TaskFilter::default(), 10)
        .unwrap();
    let made = tasks
        .iter()
        .find(|task| task.title == "Write the migration guide")
        .expect("the task was made");
    assert_eq!(made.status, TaskStatus::Doing);
    // The cursor is on the card that was just made, and the keys work again.
    assert_eq!(
        h.shell(cx, |s| s.tasks_ui.board.selected().map(|t| t.title.clone())),
        Some("Write the migration guide".to_owned())
    );
    assert!(!typing(&h, cx));
    h.press("left", cx);
    assert_eq!(h.shell(cx, |s| s.tasks_ui.board.cursor().0), 1);
}

#[gpui_kit::test]
fn the_buttons_open_the_same_panel_in_the_column_they_are_in(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    written_elsewhere(&h, "Seed", 1, cx);
    // The + of the review column.
    h.mouse_on("tasks-add-3".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert!(h.shows("tasks-create", cx));
    assert!(typing(&h, cx));
    assert_eq!(
        h.shell(cx, |s| s.tasks_ui.create.as_ref().map(|c| c.status)),
        Some(TaskStatus::Review)
    );
    h.type_text("Look at the dashboards", cx);
    h.press("enter", cx);
    wait_for_the_board(&h, cx);
    assert_eq!(titles(&h, 3, cx), ["Look at the dashboards"]);
    // The New task button of the header, from the todo column.
    h.mouse_on("tasks-new".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert!(h.shows("tasks-create", cx));
    assert!(typing(&h, cx));
}

#[gpui_kit::test]
fn escape_throws_the_new_task_away_and_an_empty_title_is_refused(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    written_elsewhere(&h, "Seed", 1, cx);
    h.press("n", cx);
    // Nothing typed: refused where it is typed, the panel stays.
    h.press("enter", cx);
    assert!(h.shows("tasks-create", cx));
    assert!(h.shows("new-task-error", cx));
    h.type_text("Never made", cx);
    h.press("escape", cx);
    assert!(!h.shows("tasks-create", cx));
    assert_eq!(
        h.main_kind(cx),
        "tasks",
        "Escape closed the panel, not the board"
    );
    wait_for_the_board(&h, cx);
    let scope = board_scope(&h, cx);
    assert!(h
        .store
        .tasks(&scope, &leon_core::TaskFilter::default(), 10)
        .unwrap()
        .iter()
        .all(|task| task.title != "Never made"));
}

#[gpui_kit::test]
fn the_panel_makes_a_task_with_everything_filled_in(cx: &mut TestAppContext) {
    let h = on_api(cx);
    let h = {
        VisualTestContext::from_window(h.window.into(), cx)
            .simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
        h.settle(cx);
        h
    };
    open_board(&h, cx);
    written_elsewhere(&h, "Seed", 1, cx);
    h.press("n", cx);
    let title = create_field(&h, cx, |c| c.title.clone());
    set_text(&h, cx, &title, "Rotate the signing key");
    let body = h.shell(cx, |s| s.tasks_ui.create.as_ref().unwrap().body.clone());
    cx.update_window(h.window.into(), |_, window, cx| {
        body.update(cx, |field, cx| {
            field.set_value(
                "Expires at the end of the month.\nKeep the old one 24 hours.",
                window,
                cx,
            )
        });
    })
    .unwrap();
    let assignee = create_field(&h, cx, |c| c.assignee.clone());
    set_text(&h, cx, &assignee, "ana");
    h.mouse_on(
        "new-status-review".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.mouse_on(
        "new-priority-high".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    // A label is typed and added with Enter, and another is left in the
    // field: it is not lost.
    let label = create_field(&h, cx, |c| c.label.clone());
    set_text(&h, cx, &label, "Security");
    {
        use gpui_kit::Focusable as _;
        cx.update_window(h.window.into(), |_, window, cx| {
            let handle = label.read(cx).focus_handle(cx);
            handle.focus(window, cx);
        })
        .unwrap();
    }
    h.press("enter", cx);
    set_text(&h, cx, &label, "backend");
    h.mouse_on(
        "new-task-create".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    wait_for_the_board(&h, cx);
    let scope = board_scope(&h, cx);
    let task = h
        .store
        .tasks(&scope, &leon_core::TaskFilter::default(), 10)
        .unwrap()
        .into_iter()
        .find(|task| task.title == "Rotate the signing key")
        .expect("the task was made");
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.priority, leon_core::TaskPriority::High);
    assert_eq!(task.assignee.as_deref(), Some("ana"));
    assert_eq!(task.labels, ["security", "backend"]);
    assert_eq!(
        task.body,
        "Expires at the end of the month.\nKeep the old one 24 hours."
    );
}

#[gpui_kit::test]
fn what_is_typed_in_the_new_task_panel_is_text_and_not_commands(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    let id = written_elsewhere(&h, "Seed", 1, cx);
    h.press("n", cx);
    for key in ["d", "p", "g", "w", "e", "?"] {
        h.press(key, cx);
    }
    assert!(h.shows("tasks-create", cx), "still the panel");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(status_of(&h, &id), TaskStatus::Todo);
    assert!(matches!(
        h.shell(cx, |s| s.tasks_ui.which.clone()),
        Which::Project(_)
    ));
}

#[gpui_kit::test]
fn a_card_dragged_to_another_column_changes_its_status(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    let id = written_elsewhere(&h, "Fix the redirect", 1, cx);
    // The doing column is empty: its whole body takes the drop.
    drag_card(&h, "tasks-card-1-0", "tasks-tail-2", 0.5, cx);
    assert_eq!(status_of(&h, &id), TaskStatus::Doing);
    assert_eq!(titles(&h, 2, cx), ["Fix the redirect"]);
    assert!(h.status().contains("Moved"), "{}", h.status());
}

#[gpui_kit::test]
fn a_card_dragged_above_another_goes_above_it(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    for (n, title) in ["A", "B", "C"].into_iter().enumerate() {
        written_elsewhere(&h, title, n as i64, cx);
    }
    // C onto the upper half of A: first.
    drag_card(&h, "tasks-card-1-2", "tasks-card-1-0", 0.2, cx);
    assert_eq!(titles(&h, 1, cx), ["C", "A", "B"]);
    // C onto the lower half of B (now the last): the end.
    drag_card(&h, "tasks-card-1-0", "tasks-card-1-2", 0.8, cx);
    assert_eq!(titles(&h, 1, cx), ["A", "B", "C"]);
}

#[gpui_kit::test]
fn a_card_let_go_outside_the_columns_stays_where_it_was(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    let id = written_elsewhere(&h, "Stay put", 1, cx);
    let from = h.bounds_of("tasks-card-1-0".to_owned(), cx).unwrap();
    // Over the footer of the board: no column there.
    let footer = h.bounds_of("tasks-footer".to_owned(), cx).unwrap();
    cx.update_window(h.window.into(), |_, window, cx| {
        window.drag(from.center(), footer.center(), cx);
    })
    .unwrap();
    wait_for_the_board(&h, cx);
    assert_eq!(status_of(&h, &id), TaskStatus::Todo);
    assert_eq!(titles(&h, 1, cx), ["Stay put"]);
}

#[gpui_kit::test]
fn the_boards_keys_are_not_taken_while_another_overlay_is_up(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    h.press_chord("cmd-shift-p", "ctrl-shift-p", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    h.type_text("n", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
}

// ----- the details of a card -------------------------------------------------------------

/// A board of the project "api" with one task, whose drawer is open.
fn with_the_drawer_open(cx: &mut TestAppContext) -> (Harness, String) {
    let h = on_api(cx);
    open_board(&h, cx);
    let id = written_elsewhere(&h, "Fix the login", 1, cx);
    h.press("e", cx);
    assert!(h.shows("tasks-drawer", cx));
    (h, id)
}

fn field(
    h: &Harness,
    cx: &mut TestAppContext,
    pick: impl Fn(
        &crate::ui::task_detail::DetailUi,
    ) -> gpui_kit::Entity<gpui_kit::component::input::InputState>,
) -> gpui_kit::Entity<gpui_kit::component::input::InputState> {
    h.shell(cx, |s| {
        pick(s.tasks_ui.detail.as_ref().expect("the drawer"))
    })
}

fn set_text(
    h: &Harness,
    cx: &mut TestAppContext,
    input: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
    text: &str,
) {
    cx.update_window(h.window.into(), |_, window, cx| {
        input.update(cx, |field, cx| field.set_value(text.to_owned(), window, cx));
    })
    .unwrap();
    h.settle(cx);
}

fn focus(
    h: &Harness,
    cx: &mut TestAppContext,
    input: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
) {
    use gpui_kit::Focusable as _;
    cx.update_window(h.window.into(), |_, window, cx| {
        let handle = input.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    })
    .unwrap();
    h.settle(cx);
}

#[gpui_kit::test]
fn e_opens_the_details_and_escape_closes_them_before_the_board(cx: &mut TestAppContext) {
    let (h, _id) = with_the_drawer_open(cx);
    assert!(h.shows("task-detail-id", cx));
    // The actions are in view in the drawer itself.
    for action in [
        "task-action-start",
        "task-action-done",
        "task-action-cancel",
        "task-action-delete",
        "task-action-save",
    ] {
        assert!(h.shows(action, cx), "{action}");
    }
    h.press("escape", cx);
    assert!(!h.shows("tasks-drawer", cx));
    assert_eq!(h.main_kind(cx), "tasks");
    h.press("escape", cx);
    assert_ne!(h.main_kind(cx), "tasks");
}

#[gpui_kit::test]
fn what_is_typed_in_a_field_is_text_and_not_commands_of_the_board(cx: &mut TestAppContext) {
    let (h, id) = with_the_drawer_open(cx);
    let title = field(&h, cx, |d| d.title.clone());
    focus(&h, cx, &title);
    // `n` would ask for a task, `d` finish it, `p` raise it, `g` switch the
    // board, `w` start work and `?` open the shortcuts.
    for key in ["n", "d", "p", "g", "w", "?"] {
        h.press(key, cx);
    }
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(h.main_kind(cx), "tasks");
    wait_for_the_board(&h, cx);
    let task = h.store.task(&id).unwrap();
    assert_eq!(task.status, TaskStatus::Todo);
    assert_eq!(task.priority, leon_core::TaskPriority::None);
    assert!(h.shell(cx, |s| s.tasks_ui.detail.is_some()));
    assert!(matches!(
        h.shell(cx, |s| s.tasks_ui.which.clone()),
        Which::Project(_)
    ));
}

#[gpui_kit::test]
fn the_title_and_the_description_are_saved_as_one_patch(cx: &mut TestAppContext) {
    let (h, id) = with_the_drawer_open(cx);
    let title = field(&h, cx, |d| d.title.clone());
    set_text(&h, cx, &title, "  Fix the SSO login ");
    let body = h.shell(cx, |s| s.tasks_ui.detail.as_ref().unwrap().body.clone());
    cx.update_window(h.window.into(), |_, window, cx| {
        body.update(cx, |field, cx| {
            field.set_value(
                "Users land back on the form.\nUse a cleared jar.",
                window,
                cx,
            )
        });
    })
    .unwrap();
    h.settle(cx);
    // The save button says there is something to save, and saves.
    assert!(h.shows("task-action-save", cx));
    h.mouse_on(
        "task-action-save".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    wait_for_the_board(&h, cx);
    let task = h.store.task(&id).unwrap();
    assert_eq!(task.title, "Fix the SSO login");
    assert_eq!(
        task.body,
        "Users land back on the form.\nUse a cleared jar."
    );
    assert_eq!(task.status, TaskStatus::Todo);
}

#[gpui_kit::test]
fn closing_the_details_saves_what_was_typed(cx: &mut TestAppContext) {
    let (h, id) = with_the_drawer_open(cx);
    let assignee = field(&h, cx, |d| d.assignee.clone());
    set_text(&h, cx, &assignee, "ana");
    h.press("escape", cx);
    wait_for_the_board(&h, cx);
    assert_eq!(h.store.task(&id).unwrap().assignee.as_deref(), Some("ana"));
    assert!(!h.shows("tasks-drawer", cx));
}

#[gpui_kit::test]
fn a_title_left_empty_is_refused_in_the_drawer_and_keeps_it_open(cx: &mut TestAppContext) {
    let (h, id) = with_the_drawer_open(cx);
    let title = field(&h, cx, |d| d.title.clone());
    set_text(&h, cx, &title, "   ");
    h.press("escape", cx);
    assert!(h.shows("tasks-drawer", cx), "it stays open");
    assert!(h.shows("task-detail-error", cx));
    assert_eq!(h.store.task(&id).unwrap().title, "Fix the login");
}

#[gpui_kit::test]
fn status_priority_and_labels_apply_at_once(cx: &mut TestAppContext) {
    let h = on_api(cx);
    // A window of the height people have: the whole drawer is in view.
    VisualTestContext::from_window(h.window.into(), cx)
        .simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
    h.settle(cx);
    open_board(&h, cx);
    let id = written_elsewhere(&h, "Fix the login", 1, cx);
    h.press("e", cx);
    assert!(h.shows("tasks-drawer", cx));
    h.mouse_on(
        "task-status-review".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    wait_for_the_board(&h, cx);
    assert_eq!(status_of(&h, &id), TaskStatus::Review);
    h.mouse_on(
        "task-priority-urgent".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    wait_for_the_board(&h, cx);
    assert_eq!(
        h.store.task(&id).unwrap().priority,
        leon_core::TaskPriority::Urgent
    );
    // A label is typed and added with Enter, and has its own way out.
    let label = field(&h, cx, |d| d.label.clone());
    focus(&h, cx, &label);
    set_text(&h, cx, &label, "  Needs Review ");
    h.press("enter", cx);
    wait_for_the_board(&h, cx);
    assert_eq!(h.store.task(&id).unwrap().labels, ["needs review"]);
    h.mouse_on(
        "task-label-remove-0".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    wait_for_the_board(&h, cx);
    assert!(h.store.task(&id).unwrap().labels.is_empty());
}

#[gpui_kit::test]
fn delete_asks_twice_and_removes_the_task(cx: &mut TestAppContext) {
    let (h, id) = with_the_drawer_open(cx);
    h.mouse_on(
        "task-action-delete".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(h.store.task(&id).is_ok(), "the first press only asks");
    assert!(h.shows("tasks-drawer", cx));
    h.mouse_on(
        "task-action-delete".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    wait_for_the_board(&h, cx);
    assert!(h.store.task(&id).is_err());
    assert!(!h.shows("tasks-drawer", cx));
    assert_eq!(h.main_kind(cx), "tasks");
}

#[gpui_kit::test]
fn the_drawer_follows_the_card_that_is_chosen_and_saves_the_one_it_leaves(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    let first = written_elsewhere(&h, "First", 1, cx);
    let second = written_elsewhere(&h, "Second", 2, cx);
    h.press("e", cx);
    let assignee = field(&h, cx, |d| d.assignee.clone());
    set_text(&h, cx, &assignee, "ana");
    // Another card is chosen with the pointer.
    h.mouse_on("tasks-card-1-1".to_owned(), gpui_kit::MouseButton::Left, cx);
    wait_for_the_board(&h, cx);
    assert_eq!(
        h.store.task(&first).unwrap().assignee.as_deref(),
        Some("ana")
    );
    assert_eq!(
        h.shell(cx, |s| s.tasks_ui.detail.as_ref().map(|d| d.id.clone())),
        Some(second)
    );
}

#[gpui_kit::test]
fn a_card_can_still_be_dragged_while_the_details_are_open(cx: &mut TestAppContext) {
    let (h, id) = with_the_drawer_open(cx);
    drag_card(&h, "tasks-card-1-0", "tasks-tail-2", 0.5, cx);
    assert_eq!(status_of(&h, &id), TaskStatus::Doing);
    assert!(h.shows("tasks-drawer", cx));
}

#[gpui_kit::test]
fn the_drawer_floats_over_the_columns_and_pushes_nothing(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    let id = written_elsewhere(&h, "Fix the login", 1, cx);
    let columns_before = h.bounds_of("tasks-columns".to_owned(), cx).unwrap();
    let card_before = h.bounds_of("tasks-card-1-0".to_owned(), cx).unwrap();
    h.press("e", cx);
    assert!(h.shows("tasks-drawer", cx));
    // The columns and the card are where they were.
    let columns = h.bounds_of("tasks-columns".to_owned(), cx).unwrap();
    assert_eq!(columns, columns_before);
    assert_eq!(
        h.bounds_of("tasks-card-1-0".to_owned(), cx).unwrap(),
        card_before
    );
    // The drawer is over the board's right edge, inside it, and its buttons
    // are all inside it.
    let board = h.bounds_of("tasks".to_owned(), cx).unwrap();
    let drawer = h.bounds_of("tasks-drawer".to_owned(), cx).unwrap();
    assert_eq!(drawer.right(), columns.right());
    assert!(drawer.left() > board.left() && drawer.left() < drawer.right());
    // It covers the last column, which is still drawn under it.
    let last = h.bounds_of("tasks-column-4".to_owned(), cx).unwrap();
    assert!(last.right() > drawer.left(), "{last:?} {drawer:?}");
    for selector in [
        "task-status-done",
        "task-priority-urgent",
        "task-action-start",
        "task-action-delete",
        "task-action-save",
        "task-detail-close",
    ] {
        let bounds = h
            .bounds_of(selector.to_owned(), cx)
            .unwrap_or_else(|| panic!("{selector} is not drawn"));
        assert!(
            bounds.left() >= drawer.left() && bounds.right() <= drawer.right(),
            "{selector} {bounds:?} is outside the drawer {drawer:?}"
        );
    }
    let _ = id;
}

#[gpui_kit::test]
fn a_click_on_a_card_opens_its_details_with_every_action(cx: &mut TestAppContext) {
    let h = on_api(cx);
    open_board(&h, cx);
    let id = written_elsewhere(&h, "Fix the login", 1, cx);
    assert!(!h.shows("tasks-drawer", cx));
    h.mouse_on("tasks-card-1-0".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert!(h.shows("tasks-drawer", cx));
    assert_eq!(
        h.shell(cx, |s| s.tasks_ui.detail.as_ref().map(|d| d.id.clone())),
        Some(id)
    );
    for action in [
        "task-action-start",
        "task-action-done",
        "task-action-cancel",
        "task-action-delete",
        "task-action-save",
        "task-detail-close",
    ] {
        assert!(h.shows(action, cx), "{action}");
    }
    // The close button closes it, and the board is as it was.
    h.mouse_on(
        "task-detail-close".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(!h.shows("tasks-drawer", cx));
    assert_eq!(h.main_kind(cx), "tasks");
}

#[gpui_kit::test]
fn a_card_is_given_to_the_king_who_is_told_what_and_where(cx: &mut TestAppContext) {
    use leon_king::protocol::Request;
    let h = on_api(cx);
    open_board(&h, cx);
    written_elsewhere(&h, "Fix the login", 1, cx);
    h.press("a", cx);
    assert_eq!(h.main_kind(cx), "king", "King's view opens on it");
    let end = h.king.take().pop().expect("a line to King's service");
    h.settle(cx);
    let said: Vec<String> = end
        .drain()
        .into_iter()
        .filter_map(|(_, request)| match request {
            Request::Say { text } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].starts_with("Task "), "{}", said[0]);
    assert!(said[0].contains("of api"), "{}", said[0]);
    assert!(said[0].contains("Fix the login"), "{}", said[0]);
}

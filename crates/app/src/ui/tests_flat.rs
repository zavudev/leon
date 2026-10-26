//! The flat sidebar, the default: the sessions of a project are listed under
//! its name with no row for their worktree, and the ones off the main checkout
//! carry their worktree. The setting `sidebar_tree` brings the levels back.
//! Also the note at the foot that says how many worktrees the cleaner would
//! remove.

use super::*;

fn flat(h: &Harness, cx: &mut TestAppContext) {
    cx.update(|cx| {
        settings::set_value(
            cx,
            crate::schema::find("sidebar_tree").unwrap(),
            crate::schema::Value::Bool(false),
        )
    });
    h.settle(cx);
}

#[gpui_kit::test]
fn the_flat_sidebar_lists_sessions_under_their_project_with_the_worktree_beside(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    flat(&h, cx);
    assert_eq!(
        h.outline(cx),
        [
            "machine:This machine",
            "  project:api",
            "    session:fix the login bug",
            "    session:add oauth",
            "  project:web",
            "machine:build box",
            "  project:infra",
            "    session:rotate the keys",
        ]
    );
    // The one off the main checkout says its worktree; the others say none.
    let row = |h: &Harness, cx: &mut TestAppContext, title: &str| {
        h.shell(cx, |shell| {
            shell
                .rows
                .iter()
                .position(|row| matches!(&row.kind, Kind::Session(s) if s.title == title))
                .unwrap()
        })
    };
    let linked = row(&h, cx, "add oauth");
    let main = row(&h, cx, "fix the login bug");
    assert!(h.shows_dynamic(format!("tree-branch-{linked}"), cx));
    assert!(!h.shows_dynamic(format!("tree-branch-{main}"), cx));
    assert_eq!(
        h.shell(cx, |shell| shell
            .row_branches
            .values()
            .cloned()
            .collect::<Vec<_>>()),
        ["feature-login"]
    );
}

#[gpui_kit::test]
fn the_setting_brings_the_worktree_levels_back(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    flat(&h, cx);
    assert!(!h.outline(cx).iter().any(|row| row.contains("worktree:")));
    cx.update(|cx| {
        settings::set_value(
            cx,
            crate::schema::find("sidebar_tree").unwrap(),
            crate::schema::Value::Bool(true),
        )
    });
    h.settle(cx);
    assert!(h
        .outline(cx)
        .iter()
        .any(|row| row.contains("worktree:feature/login")));
    assert!(h.shell(cx, |shell| shell.row_branches.is_empty()));
}

#[gpui_kit::test]
fn the_cleaners_note_opens_it_and_its_cross_hides_it_until_more_can_go(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let suggest = |h: &Harness, cx: &mut TestAppContext, count: usize| {
        cx.update(|cx| {
            h.shell.update(cx, |shell, cx| {
                shell.sweep_suggested = count;
                cx.notify();
            })
        });
        cx.run_until_parked();
    };
    suggest(&h, cx, 0);
    assert!(
        !h.shows("sidebar-sweep-note", cx),
        "nothing to clean, no note"
    );
    suggest(&h, cx, 2);
    assert!(h.shows("sidebar-sweep-note", cx));
    // It sits over the foot, under the tree.
    let note = h.bounds_of("sidebar-sweep-note".to_owned(), cx).unwrap();
    let foot = h.bounds_of("sidebar-tools".to_owned(), cx).unwrap();
    assert_eq!(note.bottom(), foot.top());
    h.mouse_on(
        "sidebar-sweep-note".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Cleanup);
    h.press("escape", cx);
    suggest(&h, cx, 2);
    h.mouse_on(
        "sidebar-sweep-dismiss".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(
        h.shell(cx, |s| s.overlay),
        Overlay::None,
        "the cross opens nothing"
    );
    assert!(!h.shows("sidebar-sweep-note", cx));
    // One more worktree that can go brings it back.
    suggest(&h, cx, 3);
    assert!(h.shows("sidebar-sweep-note", cx));
}

#[gpui_kit::test]
fn a_filtered_flat_list_keeps_the_heading_that_matched(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    flat(&h, cx);
    h.press("/", cx);
    h.type_text("api", cx);
    let heading = h.shell(cx, |shell| {
        shell
            .rows
            .iter()
            .position(
                |row| matches!(&row.kind, Kind::Project { project, .. } if project.name == "api"),
            )
            .unwrap()
    });
    assert!(h.shows_dynamic(format!("tree-label-{heading}"), cx));
    assert!(
        h.shell(cx, |shell| shell.filter.is_some()),
        "the filter is on"
    );
}

/// The sidebar as it ships: flat, and only the active sessions. A terminal
/// opened in a linked worktree is listed under its project's heading, with
/// its worktree, and no worktree has a row.
#[cfg(leon_posix_tests)]
#[gpui_kit::test]
fn as_shipped_a_terminal_in_a_linked_worktree_sits_under_its_project_with_its_worktree(
    cx: &mut TestAppContext,
) {
    use super::live::{open_live, wait_until};
    let h = open_live(cx);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let linked = root.join("linked");
    std::fs::create_dir(&linked).unwrap();
    let (root, linked) = (
        root.to_string_lossy().into_owned(),
        linked.to_string_lossy().into_owned(),
    );
    let project = h
        .store
        .add_project(&MachineId::local(), "real", &root)
        .unwrap();
    h.store
        .replace_worktrees(
            &project.id,
            vec![
                NewWorktree {
                    path: root.clone(),
                    branch: Some("trunk".into()),
                    head: None,
                    is_main: true,
                },
                NewWorktree {
                    path: linked.clone(),
                    branch: Some("feature/x".into()),
                    head: None,
                    is_main: false,
                },
            ],
        )
        .unwrap();
    h.settle(cx);
    let worktree = h
        .store
        .worktrees(&project.id)
        .unwrap()
        .into_iter()
        .find(|w| !w.is_main)
        .unwrap()
        .id;
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.show(&NodeId::Worktree(worktree));
            shell.pane = Pane::Sidebar;
        })
    });
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell", |h, cx| {
        h.shell(cx, |shell| {
            shell.live.get(super::super::live::LiveId(1)).is_some()
        })
    });
    cx.update(|cx| {
        for key in ["sidebar_tree", "sidebar_show_inactive"] {
            settings::set_value(
                cx,
                crate::schema::find(key).unwrap(),
                crate::schema::Value::Bool(false),
            );
        }
    });
    h.settle(cx);
    let outline = h.outline(cx);
    assert!(
        !outline.iter().any(|row| row.contains("worktree:")),
        "{outline:?}"
    );
    let heading = outline.iter().position(|row| row.trim() == "project:real");
    let heading = heading.unwrap_or_else(|| panic!("{outline:?}"));
    assert_eq!(
        outline.get(heading + 1).map(String::as_str),
        Some("    live:1"),
        "{outline:?}"
    );
    assert_eq!(
        h.shell(cx, |shell| shell.row_branches.get("live:1").cloned()),
        Some("linked".to_owned())
    );
}

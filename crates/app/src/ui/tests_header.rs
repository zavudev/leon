//! The headers hold their two lines: at every interface scale, in both built-in
//! themes and for every kind of thing the main pane can show.

use super::live::{open_live, real_worktree, terminal_of, wait_until};
use super::*;
use crate::theme::ThemeId;

fn within(
    inner: gpui_kit::Bounds<gpui_kit::Pixels>,
    outer: gpui_kit::Bounds<gpui_kit::Pixels>,
) -> bool {
    inner.top() >= outer.top() && inner.bottom() <= outer.bottom()
}

/// Both lines lie inside the header, above its rule, with the padding to spare.
fn assert_contained(h: &Harness, cx: &mut TestAppContext, what: &str) {
    let header = h
        .bounds_of("main-header".to_owned(), cx)
        .expect("the header");
    let title = h
        .bounds_of("header-title-row".to_owned(), cx)
        .expect("the title row");
    let meta = h
        .bounds_of("header-meta-row".to_owned(), cx)
        .expect("the metadata row");
    let rule_top = header.bottom() - crate::theme::hairline();
    let pad = crate::theme::metrics::HEADER_PAD();
    assert!(
        title.top() >= header.top() + pad - px(0.01),
        "{what}: title row {title:?} in {header:?}"
    );
    assert!(
        meta.bottom() + pad <= rule_top + px(0.01),
        "{what}: the metadata row ends at {:?}, the rule starts at {rule_top:?}",
        meta.bottom()
    );
    assert!(
        title.bottom() <= meta.top(),
        "{what}: the lines do not overlap"
    );
    // The text itself fits the row: nothing is sliced by the rule.
    if let Some(text) = h.bounds_of("header-meta".to_owned(), cx) {
        assert!(
            within(text, meta),
            "{what}: the text {text:?} in its row {meta:?}"
        );
    }
    let side = h.bounds_of("sidebar-header".to_owned(), cx);
    if let Some(side) = side {
        assert_eq!(
            side.size.height, header.size.height,
            "{what}: one rule across the window"
        );
    }
}

fn each_scale_and_theme(
    cx: &mut TestAppContext,
    h: &Harness,
    what: &str,
    check: impl Fn(&Harness, &mut TestAppContext, &str),
) {
    for id in ThemeId::ALL {
        for percent in [80, 100, 125] {
            cx.update(|cx| {
                settings::set_theme_id(cx, id);
                settings::update(cx, |s| s.interface_scale = percent);
            });
            h.settle(cx);
            check(h, cx, &format!("{what}, {} at {percent}%", id.slug()));
        }
    }
    cx.update(|cx| settings::update(cx, |s| s.interface_scale = 100));
}

#[gpui_kit::test]
fn a_live_sessions_header_contains_both_lines_at_every_scale_in_both_themes(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell", |h, cx| {
        terminal_of(h, cx, 1).is_some_and(|t| t.screen_text().contains("READY>"))
    });
    each_scale_and_theme(cx, &h, "live session", assert_contained);
}

#[gpui_kit::test]
fn a_transcripts_header_contains_both_lines_at_every_scale_in_both_themes(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_session_titled(&h, "fix the login bug", cx);
    each_scale_and_theme(cx, &h, "transcript", assert_contained);
}

#[gpui_kit::test]
fn a_worktrees_and_a_projects_header_contain_both_lines(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let worktree = worktree_id(&h, "feature/login");
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            let project = shell
                .snapshot
                .projects
                .iter()
                .find(|e| e.worktrees.iter().any(|w| w.id == worktree))
                .map(|e| e.project.id.clone())
                .expect("its project");
            shell.open_worktree(&project, &worktree, cx)
        })
    });
    h.settle(cx);
    assert!(h.main_kind(cx).starts_with("worktree:"));
    each_scale_and_theme(cx, &h, "worktree", assert_contained);
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            let project = shell.snapshot.projects[0].project.id.clone();
            shell.open_project(&project, cx)
        })
    });
    h.settle(cx);
    each_scale_and_theme(cx, &h, "project", assert_contained);
}

#[gpui_kit::test]
fn the_empty_state_header_contains_its_lines_and_matches_the_sidebars(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    each_scale_and_theme(cx, &h, "nothing open", assert_contained);
}

#[gpui_kit::test]
fn a_very_long_path_does_not_change_the_height_and_the_state_words_stay(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell", |h, cx| {
        terminal_of(h, cx, 1).is_some_and(|t| t.screen_text().contains("READY>"))
    });
    let before = h.bounds_of("main-header".to_owned(), cx).unwrap();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            let long = format!("/{}", "very-long-folder-name/".repeat(40));
            if let Some(session) = shell.live.get_mut(crate::ui::live::LiveId(1)) {
                session.cwd = long;
            }
        })
    });
    cx.update_window(h.window.into(), |_, window, _| window.refresh())
        .unwrap();
    h.settle(cx);
    let after = h.bounds_of("main-header".to_owned(), cx).unwrap();
    assert_eq!(before.size.height, after.size.height);
    assert_contained(&h, cx, "a very long path");
    let row = h.bounds_of("header-meta-row".to_owned(), cx).unwrap();
    let path = h.bounds_of("header-path".to_owned(), cx).unwrap();
    assert!(
        path.right() <= row.right(),
        "the path gives way: {path:?} in {row:?}"
    );
    assert!(path.size.width > px(0.));
    // The state words (RUNNING...) come after the path and stay in the row.
    let meta = cx.update(|cx| h.shell.read(cx).main_heading_for_test(cx));
    assert!(
        meta.contains("RUNNING") || meta.contains("STARTING"),
        "{meta}"
    );
}

#[gpui_kit::test]
fn the_header_is_the_sum_of_its_parts_and_the_sidebars_is_the_same(cx: &mut TestAppContext) {
    use crate::theme::metrics as m;
    let h = open(cx, ScriptedRunner::new());
    for percent in [80, 100, 125] {
        cx.update(|cx| settings::update(cx, |s| s.interface_scale = percent));
        h.settle(cx);
        let want = m::HEADER_PAD() * 2.
            + m::HEADER_TITLE_LINE()
            + m::HEADER_GAP()
            + m::HEADER_META_LINE()
            + crate::theme::hairline();
        assert_eq!(m::HEADER_HEIGHT(), want);
        let side = h.bounds_of("sidebar-header".to_owned(), cx).unwrap();
        let main = h.bounds_of("main-header".to_owned(), cx).unwrap();
        assert_eq!(side.size.height, main.size.height, "{percent}%");
        assert_eq!(side.size.height, want, "{percent}%");
    }
    cx.update(|cx| settings::update(cx, |s| s.interface_scale = 100));
}

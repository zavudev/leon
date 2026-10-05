//! Tests of how the Settings screen lays its rows out: a row is as tall as
//! its content whatever the width of the card and the interface size, its text
//! and its controls never share a place, and moving the keyboard through the
//! options brings a tall row fully into view.

use super::*;
use crate::schema::{self, Section};
use crate::ui::settings_screen::Entry;

/// A pixel of tolerance for the rounding of the layout.
const EPS: f32 = 0.75;

const WIDTHS: [f32; 3] = [1240.0, 1000.0, 820.0];

type Rect = Bounds<gpui_kit::Pixels>;

fn top(r: &Rect) -> f32 {
    r.origin.y.as_f32()
}

fn bottom(r: &Rect) -> f32 {
    (r.origin.y + r.size.height).as_f32()
}

fn left(r: &Rect) -> f32 {
    r.origin.x.as_f32()
}

fn right(r: &Rect) -> f32 {
    (r.origin.x + r.size.width).as_f32()
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    top(inner) >= top(outer) - EPS
        && bottom(inner) <= bottom(outer) + EPS
        && left(inner) >= left(outer) - EPS
        && right(inner) <= right(outer) + EPS
}

fn opened(cx: &mut TestAppContext) -> Harness {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-,", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
    h
}

fn arrange(h: &Harness, width: f32, scale: u16, cx: &mut TestAppContext) {
    cx.update(|cx| settings::update(cx, |values| values.interface_scale = scale));
    VisualTestContext::from_window(h.window.into(), cx).simulate_resize(size(px(width), px(800.)));
    h.settle(cx);
}

fn pick(h: &Harness, section: Section, cx: &mut TestAppContext) {
    cx.update(|cx| {
        h.shell
            .update(cx, |shell, cx| shell.settings_pick_section(section, cx))
    });
    h.settle(cx);
}

fn rect(h: &Harness, selector: String, cx: &mut TestAppContext) -> Option<Rect> {
    h.bounds_of(selector, cx)
}

fn settings_of(h: &Harness, cx: &mut TestAppContext) -> Vec<&'static schema::Def> {
    h.shell(cx, |s| s.settings_entries())
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Setting(def) => Some(def),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn every_option_of_every_section_fits_its_row_at_every_width_and_size(cx: &mut TestAppContext) {
    let h = opened(cx);
    for width in WIDTHS {
        for scale in [80, 100, 125] {
            arrange(&h, width, scale, cx);
            for section in Section::ALL {
                pick(&h, section, cx);
                let defs = settings_of(&h, cx);
                let at = format!("{section:?} at {width} wide, {scale}%");
                let mut previous: Option<(&str, Rect)> = None;
                for def in &defs {
                    let key = def.key;
                    let get = |what: &str, cx: &mut TestAppContext| {
                        rect(&h, format!("settings-{what}-{key}"), cx)
                            .unwrap_or_else(|| panic!("{key} has no {what} drawn ({at})"))
                    };
                    let row = get("row", cx);
                    let text = get("text", cx);
                    let label = get("label", cx);
                    let desc = get("desc", cx);
                    let control = get("control", cx);
                    assert!(
                        contains(&row, &label),
                        "{key}: the row holds its label ({at})"
                    );
                    assert!(
                        contains(&row, &desc),
                        "{key}: the row holds its whole description ({at}): row {row:?}, description {desc:?}"
                    );
                    assert!(
                        contains(&row, &text),
                        "{key}: the row holds its text ({at})"
                    );
                    assert!(
                        contains(&row, &control),
                        "{key}: the row holds its controls ({at})"
                    );
                    assert!(
                        bottom(&label) <= top(&desc) + EPS,
                        "{key}: the label is on its own line above the description ({at})"
                    );
                    // Beside the text, or (a narrow card) under it: never over it.
                    let apart = |t: &Rect| {
                        right(t) <= left(&control) + EPS || bottom(t) <= top(&control) + EPS
                    };
                    assert!(
                        apart(&text),
                        "{key}: the text never runs under the controls ({at}): text {text:?}, controls {control:?}"
                    );
                    assert!(
                        apart(&desc) && apart(&label),
                        "{key}: label and description stay clear of the controls ({at})"
                    );
                    if let Some((before, above)) = &previous {
                        assert!(
                            bottom(above) <= top(&row) + EPS,
                            "{before} and {key} overlap ({at}): {above:?} then {row:?}"
                        );
                    }
                    previous = Some((key, row));
                }
                // The last row is not clipped by the footer.
                if let Some(last) = defs.last() {
                    h.press("end", cx);
                    let row = rect(&h, format!("settings-row-{}", last.key), cx).unwrap();
                    let list = rect(&h, "settings-options".to_owned(), cx).unwrap();
                    let footer = rect(&h, "settings-hints".to_owned(), cx).unwrap();
                    assert!(
                        bottom(&row) <= bottom(&list) + EPS && top(&row) >= top(&list) - EPS,
                        "{}: the last row is inside the list ({at}): {row:?} in {list:?}",
                        last.key
                    );
                    assert!(
                        bottom(&list) <= top(&footer) + EPS,
                        "the list ends above the footer ({at})"
                    );
                    h.press("home", cx);
                }
            }
        }
    }
    crate::theme::set_scale(100);
}

#[gpui_kit::test]
fn the_arrows_bring_a_tall_row_fully_into_view_going_down_and_up(cx: &mut TestAppContext) {
    let h = opened(cx);
    arrange(&h, 820.0, 125, cx);
    pick(&h, Section::Usage, cx);
    let defs = settings_of(&h, cx);
    let visible = |h: &Harness, key: &str, cx: &mut TestAppContext| {
        let row = rect(h, format!("settings-row-{key}"), cx).unwrap();
        let list = rect(h, "settings-options".to_owned(), cx).unwrap();
        (
            top(&row) >= top(&list) - EPS && bottom(&row) <= bottom(&list) + EPS,
            row,
            list,
        )
    };
    h.press("home", cx);
    for def in defs.iter().skip(1) {
        h.press("down", cx);
        let (ok, row, list) = visible(&h, def.key, cx);
        assert!(
            ok,
            "{} is fully in view going down: {row:?} in {list:?}",
            def.key
        );
    }
    for def in defs.iter().rev().skip(1) {
        h.press("up", cx);
        let (ok, row, list) = visible(&h, def.key, cx);
        assert!(
            ok,
            "{} is fully in view going up: {row:?} in {list:?}",
            def.key
        );
    }
    crate::theme::set_scale(100);
}

#[gpui_kit::test]
fn a_search_result_with_a_tall_description_has_the_same_layout(cx: &mut TestAppContext) {
    let h = opened(cx);
    arrange(&h, 820.0, 100, cx);
    h.press("/", cx);
    h.type_text("read", cx);
    let defs = settings_of(&h, cx);
    assert!(
        defs.iter().any(|d| d.key == "usage_claude_network"),
        "the search finds the Claude source"
    );
    let mut previous: Option<Rect> = None;
    for def in defs {
        let row = rect(&h, format!("settings-row-{}", def.key), cx).unwrap();
        let desc = rect(&h, format!("settings-desc-{}", def.key), cx).unwrap();
        assert!(
            contains(&row, &desc),
            "{}: the row holds its description",
            def.key
        );
        if let Some(above) = previous {
            assert!(
                bottom(&above) <= top(&row) + EPS,
                "{}: rows overlap",
                def.key
            );
        }
        previous = Some(row);
    }
}

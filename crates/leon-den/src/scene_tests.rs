//! Tests of the layout and of the chrome.

use std::time::Duration;

use crate::feed::Past;
use crate::model::CubState;
use crate::model::{Event, Happening};
use crate::pose::TILE;
use crate::scene::{
    build, empty_feed, wrap_detail, Layout, Rect, Scene, TextMetrics, BELOW_MIN_VIEW, CARD_CHARS,
    ROSTER_ROWS, SIDE_CHARS, SIDE_MIN_VIEW,
};
use crate::sim::Den;
use crate::testing::{at, cub, palette};

const METRICS: TextMetrics = TextMetrics {
    char_w: 8.,
    line_h: 19.,
};

fn layout(width: i32, height: i32, scale: f32, lions: usize) -> Layout {
    let metrics = TextMetrics {
        char_w: METRICS.char_w * scale,
        line_h: METRICS.line_h * scale,
    };
    // The default room.
    Layout::compute(width, height, scale, metrics, 14, 11, lions)
}

fn apart(a: Rect, b: Rect) -> bool {
    a.x >= b.x + b.w || a.x + a.w <= b.x || a.y >= b.y + b.h || a.y + a.h <= b.y
}

fn inside(a: Rect, b: Rect) -> bool {
    a.x >= b.x && a.y >= b.y && a.x + a.w <= b.x + b.w && a.y + a.h <= b.y + b.h
}

#[test]
fn an_art_pixel_is_a_whole_number_of_device_pixels_and_no_part_covers_another() {
    for (width, height) in [
        (1280, 800),
        (1600, 1000),
        (720, 900),
        (901, 1382),
        (560, 420),
        (3440, 1440),
        (1100, 300),
    ] {
        for scale in [1., 1.25, 1.5, 2.] {
            for lions in [0, 1, 6, 16, 40] {
                let l = layout(width, height, scale, lions);
                let at = format!("{width}x{height}@{scale} with {lions}");
                assert!(l.unit >= 1);
                assert_eq!(l.map.w, l.cols * TILE * l.unit);
                assert_eq!(l.map.h, l.rows * TILE * l.unit);
                assert!(l.unit as f32 <= 5. * scale, "an art pixel is at most 5 px");
                let parts: Vec<Rect> = [Some(l.field), l.roster, l.feed]
                    .into_iter()
                    .flatten()
                    .collect();
                for (index, part) in parts.iter().enumerate() {
                    assert!(inside(*part, l.view), "{at}: {part:?} leaves the view");
                    assert!(part.w > 0 && part.h > 0, "{at}");
                    for other in &parts[index + 1..] {
                        assert!(apart(*part, *other), "{at}: {part:?} over {other:?}");
                    }
                }
                if l.map.w <= l.field.w && l.map.h <= l.field.h {
                    assert!(inside(l.map, l.field), "{at}");
                }
            }
        }
    }
}

#[test]
fn the_room_is_as_large_as_whole_pixels_allow_whatever_its_size() {
    // The default room in an ordinary window.
    let office = layout(1280, 800, 1., 6);
    assert_eq!((office.cols, office.rows, office.unit), (14, 11, 4));
    // The same view on a display twice as dense: pixels twice as many.
    let dense = layout(2560, 1600, 2., 6);
    assert_eq!((dense.cols, dense.rows, dense.unit), (14, 11, 8));
    // A larger room in the same view has smaller pixels, a smaller one
    // larger, up to the limit.
    let room = |cols: i32, rows: i32| Layout::compute(1280, 800, 1., METRICS, cols, rows, 6);
    assert_eq!(room(26, 15).unit, 2);
    assert_eq!(room(10, 9).unit, 5);
    assert_eq!(room(8, 8).unit, 5, "an art pixel is at most 5 px");
    for (cols, rows) in [(26, 15), (10, 9), (30, 19)] {
        let l = room(cols, rows);
        assert_eq!((l.cols, l.rows), (cols, rows), "the room is the layout's");
        assert_eq!(l.map.w, cols * TILE * l.unit);
    }
}

#[test]
fn a_view_too_small_for_the_den_still_has_a_layout() {
    let tiny = layout(120, 90, 1., 3);
    assert_eq!((tiny.cols, tiny.rows, tiny.unit), (14, 11, 1));
    assert!(tiny.roster.is_none() && tiny.feed.is_none());
    let none = layout(0, 0, 0., 0);
    assert_eq!(none.unit, 1);
}

#[test]
fn a_wide_view_has_a_column_at_its_side_a_narrow_one_the_feed_under_the_room() {
    // Wide: the room on the left, the roster over the feed on the right.
    let wide = layout(1280, 800, 1., 4);
    let (roster, feed) = (wide.roster.unwrap(), wide.feed.unwrap());
    assert!(wide.field.x + wide.field.w <= roster.x);
    assert_eq!((roster.x, roster.w), (feed.x, feed.w));
    assert!(roster.y + roster.h < feed.y, "the roster is on top");
    assert_eq!(
        feed.y + feed.h,
        800 - 12,
        "the feed goes down to the margin"
    );
    assert_eq!(roster.w, SIDE_CHARS as i32 * 8 + 20);
    // The breakpoint is a width in logical pixels, whatever the display.
    assert!(layout(SIDE_MIN_VIEW as i32, 800, 1., 4).roster.is_some());
    assert!(layout(SIDE_MIN_VIEW as i32 - 1, 800, 1., 4)
        .roster
        .is_none());
    assert!(layout(SIDE_MIN_VIEW as i32 * 2, 1600, 2., 4)
        .roster
        .is_some());
    assert!(layout(SIDE_MIN_VIEW as i32 * 2 - 2, 1600, 2., 4)
        .roster
        .is_none());

    // Narrow: no roster, the feed under the room, as wide as the view.
    let narrow = layout(700, 860, 1., 4);
    let feed = narrow.feed.unwrap();
    assert!(narrow.roster.is_none());
    assert!(narrow.field.y + narrow.field.h <= feed.y);
    assert_eq!((feed.x, feed.w), (12, 700 - 24));
    assert!(feed.h >= 6 * 19 && feed.h <= 860 / 2, "{feed:?}");
    assert!(narrow.unit >= 3, "the room keeps most of the view");
    // Too low for that, the view is all room.
    assert!(layout(700, BELOW_MIN_VIEW as i32, 1., 4).feed.is_some());
    let low = layout(700, BELOW_MIN_VIEW as i32 - 1, 1., 4);
    assert!(low.feed.is_none() && low.roster.is_none());
    assert_eq!(low.field.h, BELOW_MIN_VIEW as i32 - 1 - 24);
    // A wide view that is very low keeps the room whole too.
    let strip = layout(1400, 200, 1., 4);
    assert!(strip.feed.is_none() && strip.roster.is_none());
}

#[test]
fn the_roster_is_as_high_as_its_lions_and_leaves_the_feed_most_of_the_column() {
    let heights: Vec<i32> = [0, 1, 2, 6, 8, 9, 40]
        .into_iter()
        .map(|lions| layout(1280, 800, 1., lions).roster.unwrap().h)
        .collect();
    assert_eq!(heights[0], heights[1], "an empty den has room for one");
    assert!(heights[1] < heights[2] && heights[2] < heights[3] && heights[3] < heights[4]);
    assert_eq!(
        heights[4], heights[5],
        "at most {ROSTER_ROWS} lions are listed"
    );
    assert_eq!(heights[5], heights[6]);
    for lions in [0, 6, 40] {
        for height in [300, 420, 800, 1400] {
            let l = layout(1280, height, 1., lions);
            let feed = l.feed.unwrap();
            if let Some(roster) = l.roster {
                assert!(
                    feed.h >= roster.h,
                    "{lions} lions in {height}: the feed is the larger"
                );
            }
            assert!(feed.h >= 5 * 19, "{lions} lions in {height}: {feed:?}");
        }
    }
}

#[test]
fn a_point_of_the_view_is_a_pixel_of_the_art_only_on_the_room() {
    let l = layout(1280, 800, 1., 4);
    assert_eq!(l.art_point(l.map.x, l.map.y), Some((0, 0)));
    assert_eq!(
        l.art_point(l.map.x + 5 * l.unit + 1, l.map.y + 9 * l.unit),
        Some((5, 9))
    );
    assert_eq!(l.art_point(l.map.x - 1, l.map.y), None);
    let feed = l.feed.unwrap();
    assert_eq!(l.art_point(feed.x + 5, feed.y + 5), None);
}

fn den(_: &Layout) -> Den {
    let mut den = Den::new();
    let mut moss = cub(1, "moss", CubState::Running);
    moss.detail = Some("cargo test -p leon-den".to_owned());
    den.update(
        &[moss, cub(2, "fern", CubState::NeedsPermission)],
        Duration::ZERO,
    );
    den
}

#[test]
fn the_chrome_is_the_page_the_frame_the_roster_and_the_feed() {
    let l = layout(1280, 800, 1., 2);
    let den = den(&l);
    let p = palette(true);
    let scene = build(&den, &den.frame(at(400)), &l, &p, None);
    assert_eq!(scene.room, l.map);
    assert_eq!(scene.under[0].rect, l.view);
    assert_eq!(scene.under[0].color, p.ground);
    // Eight arms of four corner marks around the room.
    assert_eq!(scene.under.len(), 9);
    assert!(scene.under[1..].iter().all(|quad| quad.color == p.tick));
    for quad in scene.quads.iter().chain(&scene.under) {
        assert!(quad.rect.w > 0 && quad.rect.h > 0);
    }
    // No rectangle of the chrome covers the room: the picture is whole.
    for quad in &scene.quads {
        assert!(apart(quad.rect, l.map), "{:?} is over the room", quad.rect);
    }
    let texts: Vec<&str> = scene.texts.iter().map(|text| text.text.as_str()).collect();
    for expected in [
        "THE PRIDE",
        "moss",
        "fern",
        "Lv.3",
        "RUN",
        "ASKS",
        "THE FEED",
        "ALL",
    ] {
        assert!(texts.contains(&expected), "{expected} is not in {texts:?}");
    }
    assert_eq!(scene.rows.len(), 2);
    let (row, id) = scene.rows[1];
    assert_eq!(id, 2);
    assert_eq!(scene.row_at(row.x + 3, row.y + 3), Some(2));
    assert_eq!(scene.row_at(l.map.x + 3, l.map.y + 3), None);
}

#[test]
fn the_truth_card_of_the_selected_lion_says_the_exact_detail() {
    let l = layout(1280, 800, 1., 2);
    let mut den = den(&l);
    let p = palette(false);
    let before = build(&den, &den.frame(at(400)), &l, &p, None);
    den.select(Some(1));
    let scene = build(&den, &den.frame(at(400)), &l, &p, None);
    // What the card adds is drawn last.
    let _ = before;
    let added: Vec<&str> = scene.texts[scene.texts.len() - 3..]
        .iter()
        .map(|text| text.text.as_str())
        .collect();
    assert_eq!(
        added,
        vec!["moss  Lv.3", "Running a command", "cargo test -p leon-den"]
    );
    // The pointer wins over the selection.
    den.hover(Some(2));
    let scene = build(&den, &den.frame(at(400)), &l, &p, None);
    let texts: Vec<&str> = scene.texts.iter().map(|text| text.text.as_str()).collect();
    assert!(texts.contains(&"Needs your permission"));
    assert!(!texts.contains(&"Running a command"));
    let status = scene
        .texts
        .iter()
        .find(|text| text.text == "Needs your permission")
        .unwrap();
    assert_eq!(status.color, p.warning, "what is urgent carries its colour");
}

#[test]
fn a_long_detail_is_wrapped_whole_and_only_then_cut() {
    assert_eq!(wrap_detail("cargo test", CARD_CHARS, 4), vec!["cargo test"]);
    let command =
        "cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::pedantic";
    let rows = wrap_detail(command, 30, 4);
    assert!(rows.iter().all(|row| row.chars().count() <= 30), "{rows:?}");
    assert_eq!(rows.join(" "), command, "nothing is lost");
    // A word wider than the card is broken, not dropped.
    let path = "crates/app/src/ui/a/very/deep/tree/of/directories/that/never/ends/file.rs";
    let rows = wrap_detail(path, 20, 6);
    assert_eq!(rows.concat(), path);
    // Beyond its rows it ends in an ellipsis.
    let cut = wrap_detail(&"word ".repeat(60), 20, 2);
    assert_eq!(cut.len(), 2);
    assert!(cut[1].ends_with('…') && cut[1].chars().count() <= 20);
    assert!(wrap_detail("", 20, 2).is_empty());
    // A word that fills a row to its last character leaves no room for the
    // space before the next one.
    assert_eq!(wrap_detail("abcde fghij k", 5, 4), ["abcde", "fghij", "k"]);
    assert!(wrap_detail("anything", 0, 4).is_empty());
}

#[test]
fn the_truth_card_covers_neither_its_lion_nor_a_neighbour_when_it_can_be_put_elsewhere() {
    let l = layout(1280, 800, 1., 4);
    let mut den = Den::new();
    // Three lions at neighbouring desks, one at the board over them.
    den.update(
        &[
            cub(1, "moss", CubState::Editing),
            cub(2, "fern", CubState::Editing),
            cub(3, "ash", CubState::Editing),
            cub(4, "juniper", CubState::Planning),
        ],
        Duration::ZERO,
    );
    let p = palette(true);
    for selected in [1, 2, 3, 4] {
        den.select(Some(selected));
        let frame = den.frame(at(400));
        let scene = build(&den, &frame, &l, &p, None);
        // The card is the last box drawn on the room.
        let card = scene
            .quads
            .iter()
            .rev()
            .map(|quad| quad.rect)
            .find(|rect| rect.w > 100 && rect.h > 40 && rect.x + rect.w <= l.field.x + l.field.w)
            .expect("the card is drawn");
        for actor in &frame.actors {
            let (left, top, w, h) = actor.bounds();
            let (x, y) = (l.map.x + left * l.unit, l.map.y + top * l.unit);
            let apart = card.x >= x + w * l.unit
                || card.x + card.w <= x
                || card.y >= y + h * l.unit
                || card.y + card.h <= y;
            assert!(
                apart,
                "the card of {selected} at {card:?} covers lion {}",
                actor.id
            );
        }
    }
}

fn texts(scene: &Scene) -> Vec<&str> {
    scene.texts.iter().map(|text| text.text.as_str()).collect()
}

/// A den whose lions have told and said things: enough for a feed longer
/// than its box.
fn busy(l: &Layout) -> Den {
    let mut den = den(l);
    den.set_wall_time(10_000);
    for round in 0..12u64 {
        den.set_wall_time(10_000 + round as i64 * 200);
        let (id, name) = if round % 2 == 0 {
            (1, "moss")
        } else {
            (2, "fern")
        };
        den.happen(
            &Happening::new(
                id,
                name,
                Event::ToolStarted {
                    kind: crate::model::ToolKind::Read,
                    tool: "Read".into(),
                    detail: Some(format!("file_{round}.rs")),
                },
            ),
            at(round * 20),
        );
        den.speak(
            id,
            name,
            &format!("Message {round}.\n\n{}", "It goes on and on. ".repeat(14)),
            None,
            at(round * 20 + 5),
        );
    }
    den
}

#[test]
fn the_feed_shows_its_newest_rows_each_under_the_lion_that_it_is_about() {
    let l = layout(1280, 800, 1., 2);
    let den = busy(&l);
    let p = palette(true);
    let scene = build(&den, &den.frame(at(2000)), &l, &p, None);
    let feed = l.feed.unwrap();
    assert_eq!(scene.feed, Some(feed));
    assert!(scene.feed_height >= 10);
    assert!(
        scene.feed_lines.len() > scene.feed_height,
        "longer than its box"
    );
    assert!(scene.feed_rows.len() <= scene.feed_height);
    assert!(scene.feed_rows.iter().all(|(rect, _)| inside(*rect, feed)));
    let shown = texts(&scene);
    // The newest message, cut to a few rows with the way to the rest.
    assert!(shown.contains(&"Message 11."), "{shown:?}");
    assert!(shown
        .iter()
        .any(|text| text.starts_with('+') && text.ends_with("more rows")));
    assert!(!shown.contains(&"Message 0."), "the oldest is above");
    // Its heading: the name as the narrator writes it, "said", the time.
    assert!(shown.contains(&"FERN") && shown.contains(&"said") && shown.contains(&"now"));
    // The swatch of a heading is the lion's mane.
    let tint = crate::testing::cub(2, "fern", CubState::Idle).species.tint;
    assert!(scene
        .quads
        .iter()
        .any(|quad| quad.color == tint && inside(quad.rect, feed)));
    // It follows its end: nothing to go back to.
    assert_eq!(scene.jump, None);
    assert_eq!(scene.all, None, "the feed is everybody's");
    // A click on a row is a click on its entry.
    let (rect, entry) = *scene.feed_rows.last().unwrap();
    assert_eq!(scene.entry_at(rect.x + 4, rect.y + 2), Some(entry));
    assert_eq!(scene.entry_at(l.map.x + 4, l.map.y + 4), None);
}

#[test]
fn what_is_urgent_carries_its_colour_and_its_glyph_in_the_feed() {
    let l = layout(1280, 800, 1., 2);
    let mut den = den(&l);
    let p = palette(true);
    den.happen(&Happening::new(2, "fern", Event::PermissionPrompt), at(400));
    let scene = build(&den, &den.frame(at(2000)), &l, &p, None);
    let feed = l.feed.unwrap();
    let line = scene
        .texts
        .iter()
        .find(|text| text.text == "A wild PERMISSION PROMPT appeared.")
        .expect("the line is in the feed");
    assert_eq!(line.color, p.warning);
    assert!(texts(&scene).contains(&"FERN is staring at you."));
    let sign = |quad: &&crate::scene::Quad| quad.color == p.warning && inside(quad.rect, feed);
    assert!(
        scene.quads.iter().filter(sign).count() >= 3,
        "the warning sign"
    );
    // While it is typed, only what was typed shows.
    let early = build(&den, &den.frame(at(401)), &l, &p, None);
    let typed = texts(&early);
    assert!(!typed.contains(&"FERN is staring at you."), "{typed:?}");
    assert!(typed.iter().any(
        |text| "A wild PERMISSION PROMPT appeared.".starts_with(text)
            && !text.is_empty()
            && text.len() < 12
    ));
}

#[test]
fn a_selected_lion_narrows_the_feed_to_itself_and_its_little_ones() {
    let l = layout(1280, 800, 1., 3);
    let mut den = busy(&l);
    let mut little = crate::testing::little(7, 1, CubState::Reading);
    little.name = "explore".to_owned();
    let mut cubs = vec![
        cub(1, "moss", CubState::Running),
        cub(2, "fern", CubState::NeedsPermission),
        little,
    ];
    cubs.push(cub(3, "ash", CubState::Idle));
    den.update(&cubs, at(2000));
    den.speak(7, "explore", "Report: nothing found.", None, at(2001));
    let p = palette(true);

    den.select(Some(1));
    let scene = build(&den, &den.frame(at(3000)), &l, &p, None);
    let shown = texts(&scene);
    assert!(shown.contains(&"MOSS x"), "{shown:?}");
    assert!(!shown.contains(&"ALL"));
    assert!(
        shown.contains(&"Report: nothing found."),
        "its little one's too"
    );
    assert!(shown.contains(&"EXPLORE") && shown.contains(&"MOSS"));
    assert!(!shown.contains(&"FERN"), "nobody else's");
    // The name is the way back to everybody.
    let all = scene.all.expect("the name can be clicked");
    assert!(inside(all, l.feed.unwrap()));
    // The truth card is still there.
    assert!(shown.contains(&"Running a command"));

    // A lion with nothing in the feed says so, in the narrator's voice.
    den.select(Some(3));
    let scene = build(&den, &den.frame(at(3000)), &l, &p, None);
    assert!(texts(&scene).contains(&"ASH has not said a word yet."));
    assert!(scene.feed_rows.is_empty() && scene.feed_lines.is_empty());
    // One that Leon cannot read says why there will be nothing but the
    // narrator, with the plain fact under it.
    let mut ghost = cub(4, "ghost", CubState::Mystery);
    ghost.mystery = true;
    cubs.push(ghost);
    den.update(&cubs, at(3000));
    den.select(Some(4));
    assert_eq!(
        empty_feed(&den),
        vec![
            "GHOST keeps its thoughts to itself.".to_owned(),
            String::new(),
            "No transcript is followed for this session: the feed has only what its terminal \
             shows."
                .to_owned()
        ]
    );
    let scene = build(&den, &den.frame(at(3000)), &l, &p, None);
    let shown = texts(&scene);
    assert!(shown.contains(&"GHOST keeps its thoughts to itself."));
    assert!(
        shown.contains(&"No transcript is followed for this"),
        "{shown:?}"
    );
    den.select(None);
    assert_eq!(
        empty_feed(&Den::new()),
        vec!["Nothing to tell yet.".to_owned()]
    );
}

#[test]
fn a_reader_who_went_back_stays_there_and_is_shown_the_way_to_the_end() {
    let l = layout(1280, 800, 1., 2);
    let mut den = busy(&l);
    let p = palette(false);
    let end = build(&den, &den.frame(at(2000)), &l, &p, None);
    let (total, height) = (end.feed_lines.len(), end.feed_height);
    den.feed_mut().scroll(-(height as i64), total, height);
    let back = build(&den, &den.frame(at(2000)), &l, &p, None);
    assert_ne!(texts(&back), texts(&end));
    assert!(!texts(&back).contains(&"Message 11."));
    let jump = back.jump.expect("the way back is shown");
    assert!(inside(jump, l.feed.unwrap()));
    assert!(texts(&back).contains(&"v latest"));
    // What arrives meanwhile is counted, and moves nothing.
    den.speak(1, "moss", "One more thing.", None, at(2100));
    let still = build(&den, &den.frame(at(2200)), &l, &p, None);
    assert!(texts(&still).contains(&"v 1 new"));
    let first = |scene: &Scene| scene.feed_rows[0].1;
    assert_eq!(first(&still), first(&back));
    // Back at the end it follows again.
    den.feed_mut().follow();
    let again = build(&den, &den.frame(at(2200)), &l, &p, None);
    assert_eq!(again.jump, None);
    assert!(texts(&again).contains(&"One more thing."));
}

#[test]
fn with_the_narrator_off_its_lines_are_plain_and_the_agents_words_stay() {
    let l = layout(1280, 800, 1., 2);
    let mut den = den(&l);
    let p = palette(true);
    den.happen(
        &Happening::new(
            1,
            "moss",
            Event::ToolStarted {
                kind: crate::model::ToolKind::Run,
                tool: "Bash".into(),
                detail: Some("cargo test -p leon-den".into()),
            },
        ),
        at(10),
    );
    den.speak(1, "moss", "All 147 tests pass.", None, at(20));
    den.remember(2, "fern", &[Past::Tool, Past::Tool, Past::TurnEnded]);
    let voiced = build(&den, &den.frame(at(2000)), &l, &p, None);
    let shown = texts(&voiced);
    assert!(shown.contains(&"MOSS used CARGO TEST!"), "{shown:?}");
    assert!(shown.contains(&"FERN used 2 tools."));
    den.set_plain_status(true);
    let plain = build(&den, &den.frame(at(2000)), &l, &p, None);
    let shown = texts(&plain);
    assert!(
        shown.contains(&"Running `cargo test -p leon-den`"),
        "{shown:?}"
    );
    assert!(shown.contains(&"Used 2 tools."));
    assert!(!shown.iter().any(|text| text.contains("CARGO TEST")));
    assert!(
        shown.contains(&"All 147 tests pass."),
        "what the agent said is its own"
    );
    // The foot is the plain count of the den.
    assert!(shown.contains(&"2 live sessions: 1 working, 0 waiting."));
}

#[test]
fn the_foot_of_the_feed_is_what_the_den_says_of_itself() {
    let l = layout(1280, 800, 1., 0);
    let den = Den::new();
    let p = palette(true);
    let scene = build(&den, &den.frame(at(2000)), &l, &p, None);
    let shown = texts(&scene);
    assert!(shown.contains(&"The den is quiet. Too quiet."), "{shown:?}");
    assert!(shown.contains(&"Nothing to tell yet."));
    // In a narrow view the feed is under the room, with the same things.
    let narrow = layout(700, 860, 1., 0);
    let scene = build(&den, &den.frame(at(2000)), &narrow, &p, None);
    assert!(texts(&scene).contains(&"THE FEED"));
    assert!(!texts(&scene).contains(&"THE PRIDE"));
    // And in one with no feed at all, nothing of it is drawn.
    let small = layout(520, 360, 1., 0);
    let scene = build(&den, &den.frame(at(2000)), &small, &p, None);
    assert!(scene.texts.is_empty() && scene.feed.is_none());
}

#[test]
fn while_the_room_is_edited_the_editors_note_is_pinned_over_the_feed() {
    let l = layout(1280, 800, 1., 2);
    let den = busy(&l);
    let p = palette(true);
    let note = vec![
        "No shelf. Readers will stand around.".to_owned(),
        "No bookshelf: lions that read or search stand on free floor.".to_owned(),
    ];
    let plain = build(&den, &den.frame(at(2000)), &l, &p, None);
    let scene = build(&den, &den.frame(at(2000)), &l, &p, Some(&note));
    let shown = texts(&scene);
    assert!(shown.contains(&note[0].as_str()), "{shown:?}");
    // The plain fact is wrapped to the column.
    assert!(shown.contains(&"No bookshelf: lions that read or"));
    assert!(
        scene.feed_height < plain.feed_height,
        "the feed gives it room"
    );
    assert!(shown.contains(&"Message 11."), "and goes on under it");
}

#[test]
fn no_size_of_view_and_no_text_of_a_session_makes_the_scene_panic() {
    // Names, details and messages as sessions really have them: long,
    // with one word that fills a row and more, in other scripts, empty.
    let texts = [
        "",
        "x",
        "a-name-that-is-exactly-as-long-as-a-row-of-the-card-is",
        "crates/app/src/ui/a/very/deep/tree/of/directories/that/never/ends/file.rs and more words",
        "完了しました。テストはすべて通ります。完了しました。テストはすべて通ります。",
        "\u{1f981}\u{1f981}\u{1f981} line\none\n\n\ttabbed   spaced   ",
    ];
    let p = palette(true);
    for (round, text) in texts.iter().enumerate() {
        let mut den = Den::new();
        let mut cubs = Vec::new();
        for (index, state) in CubState::ALL.iter().enumerate() {
            let mut one = cub(index as u64 + 1, text, *state);
            one.detail = Some((*text).to_owned());
            one.level = u32::MAX;
            one.mystery = index % 2 == 0;
            cubs.push(one);
        }
        cubs.push(crate::testing::little(900, 1, CubState::Reading));
        den.update(&cubs, at(0));
        den.set_wall_time(i64::MAX - round as i64);
        for id in 1..=4u64 {
            den.speak(
                id,
                text,
                &format!("{text} {}", text.repeat(40)),
                Some(i64::MIN),
                at(1),
            );
            den.happen(&Happening::new(id, *text, Event::PermissionPrompt), at(2));
            den.happen(
                &Happening::new(
                    id,
                    *text,
                    Event::ToolStarted {
                        kind: crate::model::ToolKind::Other,
                        tool: (*text).to_owned(),
                        detail: Some((*text).to_owned()),
                    },
                ),
                at(3),
            );
        }
        den.remember(
            2,
            text,
            &[Past::Tool, Past::Speech((*text).to_owned(), None)],
        );
        let note = vec![(*text).to_owned(), text.repeat(9)];
        for selected in [None, Some(1), Some(2), Some(900), Some(12_345)] {
            den.select(selected);
            den.hover(selected);
            for (width, height) in [
                (0, 0),
                (1, 1),
                (37, 900),
                (900, 37),
                (360, 300),
                (360, 640),
                (819, 419),
                (820, 420),
                (821, 261),
                (1280, 800),
                (5120, 2880),
            ] {
                for scale in [0.5, 1., 1.25, 3.] {
                    for (char_w, line_h) in [(0.1, 0.1), (8., 19.), (40., 90.), (2000., 3000.)] {
                        let metrics = TextMetrics { char_w, line_h };
                        let l = Layout::compute(width, height, scale, metrics, 14, 11, cubs.len());
                        let frame = den.frame(at(4 + round as u64));
                        for note in [None, Some(note.as_slice())] {
                            let scene = build(&den, &frame, &l, &p, note);
                            // Reading it back is safe too.
                            let (total, rows) = (scene.feed_lines.len(), scene.feed_height);
                            let mut feed = den.feed().clone();
                            feed.scroll(-3, total, rows);
                            feed.step_cursor(&scene.feed_lines, rows, true);
                            let _ = feed.window(total, rows);
                        }
                    }
                }
            }
        }
    }
    // The pieces of it one by one, at the widths where a word fills the row.
    for width in 0..40 {
        for word in 0..40 {
            let text = format!("{} {}", "w".repeat(word), "v".repeat(width));
            let _ = wrap_detail(&text, width.max(1), 3);
            let _ = wrap_detail(&text, width.max(1), 0);
            let _ = crate::feed::wrap_text(&text, width);
        }
    }
}

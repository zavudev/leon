//! Tests of the feed: what is kept and where it goes, how the past of a
//! session is summed up, what is laid out for a reader, and where the
//! reader is.

use std::time::Duration;

use gpui_kit::hsla;

use crate::feed::{
    ago, backfill, lay, tools_line, wrap_text, Feed, Item, Kind, Past, Row, RowKind, Show,
    CLAMP_ROWS, PER_SESSION, SAME_BREATH, TOTAL,
};
use crate::narrator::{TICK, TYPED_PER_TICK};

fn item(cub: u64, name: &str, kind: Kind, text: &str, at: Option<i64>) -> Item {
    Item {
        cub,
        owner: cub,
        name: name.to_owned(),
        tint: Some(hsla(0.1, 0.5, 0.5, 1.)),
        at,
        kind,
        text: text.to_owned(),
        plain: format!("plain: {text}"),
    }
}

fn told(cub: u64, name: &str, text: &str, at: i64) -> Item {
    item(cub, name, Kind::Narration { urgent: false }, text, Some(at))
}

fn said(cub: u64, name: &str, text: &str, at: i64) -> Item {
    Item {
        plain: String::new(),
        ..item(cub, name, Kind::Speech, text, Some(at))
    }
}

fn texts(feed: &Feed) -> Vec<&str> {
    feed.entries()
        .map(|entry| entry.item.text.as_str())
        .collect()
}

fn show(width: usize) -> Show {
    Show {
        width,
        ..Show::default()
    }
}

fn body(rows: &[Row]) -> Vec<&str> {
    rows.iter()
        .filter(|row| !matches!(row.kind, RowKind::Heading { .. } | RowKind::Gap))
        .map(|row| row.text.as_str())
        .collect()
}

const T0: Duration = Duration::ZERO;

#[test]
fn what_happens_is_added_at_the_end_and_the_same_line_twice_counts() {
    let mut feed = Feed::new();
    assert!(feed.is_empty() && feed.follows());
    let first = feed.push(told(1, "moss", "MOSS used READ!", 100), T0);
    let again = feed.push(told(1, "moss", "MOSS used READ!", 130), T0);
    assert_eq!(first, again, "one entry");
    assert_eq!(feed.len(), 1);
    let entry = feed.entries().next().unwrap();
    assert_eq!((entry.count, entry.item.at), (2, Some(130)));
    assert_eq!(body(&lay(&feed, &show(40))), ["MOSS used READ! (x2)"]);
    // Another lion's same words, another line, urgent lines and speech are
    // entries of their own.
    feed.push(told(2, "fern", "MOSS used READ!", 131), T0);
    feed.push(told(2, "fern", "FERN is done.", 132), T0);
    feed.push(told(2, "fern", "MOSS used READ!", 133), T0);
    let urgent = item(
        2,
        "fern",
        Kind::Narration { urgent: true },
        "FERN fainted!",
        Some(134),
    );
    feed.push(urgent.clone(), T0);
    feed.push(urgent, T0);
    feed.push(said(2, "fern", "Yes.", 135), T0);
    feed.push(said(2, "fern", "Yes.", 136), T0);
    assert_eq!(feed.len(), 8);
    assert!(feed.entries().skip(1).all(|entry| entry.count == 1));
    // Ids go up in the order of arrival.
    let ids: Vec<u64> = feed.entries().map(|entry| entry.id).collect();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn the_feed_keeps_a_number_of_entries_and_drops_the_oldest() {
    let mut feed = Feed::new();
    let mut first = 0;
    for n in 0..TOTAL + 25 {
        let id = feed.push(told(1, "moss", &format!("line {n}"), n as i64), T0);
        if n == 0 {
            first = id;
            feed.toggle(id);
        }
    }
    assert_eq!(feed.len(), TOTAL);
    assert_eq!(texts(&feed)[0], "line 25");
    assert_eq!(
        *texts(&feed).last().unwrap(),
        format!("line {}", TOTAL + 24)
    );
    assert!(!feed.is_open(first), "what is gone is forgotten whole");
    feed.clear();
    assert!(feed.is_empty());
}

#[test]
fn the_past_of_a_session_is_its_messages_and_one_line_for_the_tools_of_a_turn() {
    let past = [
        Past::Tool,
        Past::Tool,
        Past::Tool,
        Past::Speech("I read three files.".into(), Some(100)),
        Past::Tool,
        Past::Speech("And fixed one.".into(), Some(160)),
        Past::TurnEnded,
        // A turn of tools with no word at its end.
        Past::Tool,
        Past::Tool,
        Past::TurnEnded,
        // A turn with no tool.
        Past::Speech("Hello.".into(), None),
        Past::TurnEnded,
        // Tools still running when the file ends.
        Past::Tool,
    ];
    let items = backfill(7, 3, "moss", None, &past, PER_SESSION);
    let lines: Vec<(&str, Option<i64>, bool)> = items
        .iter()
        .map(|item| (item.text.as_str(), item.at, item.kind == Kind::Speech))
        .collect();
    assert_eq!(
        lines,
        vec![
            ("MOSS used 3 tools.", Some(100), false),
            ("I read three files.", Some(100), true),
            ("MOSS used a tool.", Some(160), false),
            ("And fixed one.", Some(160), true),
            ("MOSS used 2 tools.", Some(160), false),
            // With no time of its own, it has the time of what came before.
            ("Hello.", Some(160), true),
            ("MOSS used a tool.", Some(160), false),
        ]
    );
    assert!(items.iter().all(|item| (item.cub, item.owner) == (7, 3)));
    assert_eq!(items[0].plain, "Used 3 tools.");
    assert_eq!(items[1].plain, "", "speech is plain already");
    assert_eq!(
        tools_line("a very long name indeed", 14).0,
        "A VERY LONG… used 14 tools."
    );
    // Only the last are kept.
    let last = backfill(7, 3, "moss", None, &past, 2);
    assert_eq!(last.len(), 2);
    assert_eq!(last[0].text, "Hello.");
    assert!(backfill(7, 3, "moss", None, &[], 9).is_empty());
    assert!(backfill(7, 3, "moss", None, &[Past::TurnEnded], 9).is_empty());
    // A message of nothing is no entry, but ends the count of tools.
    let blank = backfill(
        7,
        3,
        "m",
        None,
        &[Past::Tool, Past::Speech(" \n".into(), Some(5))],
        9,
    );
    assert_eq!(blank.len(), 1);
    // A long session is one line a turn, not one a tool.
    let mut long = Vec::new();
    for _ in 0..300 {
        long.extend(std::iter::repeat_n(Past::Tool, 12));
        long.push(Past::Speech("ok".into(), Some(1)));
        long.push(Past::TurnEnded);
    }
    assert_eq!(
        backfill(1, 1, "m", None, &long, PER_SESSION).len(),
        PER_SESSION
    );
}

#[test]
fn the_past_goes_where_its_time_says_among_what_is_there() {
    let mut feed = Feed::new();
    feed.push(told(1, "moss", "at 100", 100), T0);
    feed.push(told(1, "moss", "at 300", 300), T0);
    // No time: it is as late as the one before it.
    feed.push(item(1, "moss", Kind::Speech, "after 300", None), T0);
    feed.push(told(1, "moss", "at 500", 500), T0);
    feed.insert_past(vec![
        told(2, "fern", "fern at 50", 50),
        told(2, "fern", "fern at 200", 200),
        // Without a time it follows the one before it.
        item(2, "fern", Kind::Speech, "fern after 200", None),
        told(2, "fern", "fern at 300", 300),
        told(2, "fern", "fern at 900", 900),
    ]);
    assert_eq!(
        texts(&feed),
        vec![
            "fern at 50",
            "at 100",
            "fern at 200",
            "fern after 200",
            "at 300",
            "after 300",
            "fern at 300",
            "at 500",
            "fern at 900"
        ]
    );
    // The past is not typed out, and does not count as news.
    assert!(feed.entries().filter(|entry| entry.born.is_none()).count() == 5);
    assert_eq!(feed.unseen(), 0);
    // With no time at all it goes at the end, in its order.
    let mut bare = Feed::new();
    bare.push(told(1, "moss", "now", 10), T0);
    bare.insert_past(vec![
        item(2, "fern", Kind::Speech, "one", None),
        item(2, "fern", Kind::Speech, "two", None),
    ]);
    assert_eq!(texts(&bare), vec!["now", "one", "two"]);
}

#[test]
fn a_time_is_said_in_a_few_letters_from_the_time_it_is() {
    assert_eq!(ago(1000, 1000), "now");
    assert_eq!(ago(1000, 1059), "now");
    assert_eq!(ago(1000, 1060), "1m");
    assert_eq!(ago(1000, 1000 + 59 * 60 + 59), "59m");
    assert_eq!(ago(1000, 1000 + 3600), "1h");
    assert_eq!(ago(1000, 1000 + 86_399), "23h");
    assert_eq!(ago(1000, 1000 + 86_400), "1d");
    assert_eq!(ago(1000, 1000 + 86_400 * 40), "40d");
    assert_eq!(ago(2000, 1000), "now", "a clock that is behind is no past");
    // No number in a transcript makes the sum overflow.
    assert_eq!(ago(i64::MAX, i64::MIN), "now");
    assert!(ago(i64::MIN, i64::MAX).ends_with('d'));
    let mut feed = Feed::new();
    feed.push(told(1, "moss", "a", i64::MIN), T0);
    feed.push(told(1, "moss", "b", i64::MAX), T0);
    assert_eq!(
        lay(&feed, &show(10)).len(),
        5,
        "two headings, a gap, two rows"
    );
}

#[test]
fn a_text_is_wrapped_at_its_breaks_at_spaces_and_inside_what_is_too_long() {
    assert_eq!(wrap_text("Done.", 20), ["Done."]);
    assert_eq!(
        wrap_text("The quick brown fox jumps over the lazy dog", 15),
        ["The quick brown", "fox jumps over", "the lazy dog"]
    );
    // Its own line breaks stay; several empty lines are one.
    assert_eq!(
        wrap_text("One.\n\n\n\nTwo.\nThree.\n\n", 20),
        ["One.", "", "Two.", "Three."]
    );
    // A list and a block of code keep their indent; a tab is two spaces.
    assert_eq!(
        wrap_text("- a\n  - b\n\tlet x = 1;", 20),
        ["- a", "  - b", "  let x = 1;"]
    );
    // A word longer than a row is cut, not lost.
    let path = "crates/app/src/ui/a/very/deep/tree/file.rs";
    let rows = wrap_text(&format!("see {path} now"), 12);
    assert!(rows.iter().all(|row| row.chars().count() <= 12), "{rows:?}");
    assert_eq!(rows.join("").replace(' ', ""), format!("see{path}now"));
    // Other scripts are counted in characters.
    let rows = wrap_text("完了しました。テストはすべて通ります。", 6);
    assert!(rows.iter().all(|row| row.chars().count() <= 6));
    assert_eq!(rows.concat(), "完了しました。テストはすべて通ります。");
    assert!(wrap_text("", 10).is_empty());
    assert!(wrap_text(" \n \n", 10).is_empty());
    // Never a panic, never an endless loop, whatever the width.
    for width in 0..6 {
        let rows = wrap_text("      a bb ccc dddd eeeee", width);
        assert!(
            rows.iter().all(|row| row.chars().count() <= width.max(1)),
            "{width}: {rows:?}"
        );
    }
}

#[test]
fn entries_are_laid_out_under_a_heading_for_each_lion_and_each_kind() {
    let mut feed = Feed::new();
    feed.push(told(1, "moss", "MOSS used READ!", 1000), T0);
    feed.push(told(1, "moss", "MOSS is typing furiously.", 1040), T0);
    feed.push(said(1, "moss", "Done.", 1050), T0);
    feed.push(told(2, "fern", "FERN joined the pride.", 1060), T0);
    feed.push(told(2, "fern", "FERN is done.", 1060 + SAME_BREATH + 1), T0);
    let rows = lay(
        &feed,
        &Show {
            width: 30,
            now: Some(1300),
            ..Show::default()
        },
    );
    let kinds: Vec<String> = rows
        .iter()
        .map(|row| match &row.kind {
            RowKind::Heading {
                name, time, speech, ..
            } => format!("[{name}{} {time}]", if *speech { " said" } else { "" }),
            RowKind::Gap => "-".to_owned(),
            _ => row.text.clone(),
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "[MOSS 5m]",
            "MOSS used READ!",
            "MOSS is typing furiously.",
            "-",
            "[MOSS said 4m]",
            "Done.",
            "-",
            "[FERN 4m]",
            "FERN joined the pride.",
            "-",
            // The same lion, much later: a heading of its own.
            "[FERN 1m]",
            "FERN is done.",
        ]
    );
    // Every row knows its entry.
    let ids: Vec<u64> = feed.entries().map(|entry| entry.id).collect();
    assert_eq!(rows[1].entry, ids[0]);
    assert_eq!(rows[5].entry, ids[2]);
    // With no time said, a heading has none.
    let rows = lay(&feed, &show(30));
    assert!(rows.iter().all(|row| match &row.kind {
        RowKind::Heading { time, .. } => time.is_empty(),
        _ => true,
    }));
}

#[test]
fn a_long_message_is_cut_to_a_few_rows_until_it_is_opened() {
    let mut feed = Feed::new();
    let long = (1..=9)
        .map(|n| format!("Row {n}."))
        .collect::<Vec<_>>()
        .join("\n");
    let id = feed.push(said(1, "moss", &long, 10), T0);
    // One row more than the cut is shown whole: "+1 more" would save none.
    let near = (1..=CLAMP_ROWS + 1)
        .map(|n| format!("Row {n}."))
        .collect::<Vec<_>>()
        .join("\n");
    feed.push(said(2, "fern", &near, 11), T0);
    // The narrator's lines are never cut.
    feed.push(told(3, "ash", "a\nb\nc\nd\ne\nf\ng", 12), T0);

    let rows = lay(&feed, &show(30));
    let moss: Vec<&Row> = rows.iter().filter(|row| row.entry == id).collect();
    assert_eq!(
        moss.len(),
        1 + CLAMP_ROWS + 1,
        "heading, the rows, the rest"
    );
    assert_eq!(moss.last().unwrap().kind, RowKind::More(9 - CLAMP_ROWS));
    assert_eq!(moss.last().unwrap().text, "+5 more rows");
    assert_eq!(
        body(&rows)
            .iter()
            .filter(|text| text.starts_with("Row"))
            .count(),
        4 + 5
    );
    assert_eq!(body(&rows).iter().filter(|text| text.len() == 1).count(), 7);

    feed.toggle(id);
    assert!(feed.is_open(id));
    let open = lay(&feed, &show(30));
    assert_eq!(open.iter().filter(|row| row.entry == id).count(), 1 + 9);
    assert!(!open.iter().any(|row| matches!(row.kind, RowKind::More(_))));
    feed.toggle(id);
    assert_eq!(lay(&feed, &show(30)), rows, "and cut again");
}

#[test]
fn the_feed_is_narrowed_to_a_lion_and_its_little_ones() {
    let mut feed = Feed::new();
    feed.push(told(1, "moss", "MOSS sent out EXPLORE!", 10), T0);
    feed.push(
        Item {
            owner: 1,
            ..said(9, "explore", "Found it.", 11)
        },
        T0,
    );
    feed.push(told(2, "fern", "FERN is done.", 12), T0);
    let only = |only: u64| {
        lay(
            &feed,
            &Show {
                only: Some(only),
                width: 30,
                ..Show::default()
            },
        )
    };
    assert_eq!(body(&only(1)), ["MOSS sent out EXPLORE!", "Found it."]);
    assert_eq!(body(&only(2)), ["FERN is done."]);
    // The little one alone is itself.
    assert_eq!(body(&only(9)), ["Found it."]);
    assert!(only(5).is_empty());
    assert_eq!(body(&lay(&feed, &show(30))).len(), 3);
    // The first row of a narrowed feed is a heading, never a gap.
    assert!(matches!(only(2)[0].kind, RowKind::Heading { .. }));
}

#[test]
fn with_the_narrator_off_its_lines_are_the_plain_ones_and_speech_is_as_it_was() {
    let mut feed = Feed::new();
    feed.push(told(1, "moss", "MOSS used CARGO TEST!", 10), T0);
    feed.push(said(1, "moss", "All green.", 11), T0);
    // A line with no plain twin is itself.
    feed.push(
        Item {
            plain: String::new(),
            ..told(1, "moss", "The den is quiet.", 12)
        },
        T0,
    );
    let plain = lay(
        &feed,
        &Show {
            plain: true,
            width: 40,
            ..Show::default()
        },
    );
    assert_eq!(
        body(&plain),
        [
            "plain: MOSS used CARGO TEST!",
            "All green.",
            "The den is quiet."
        ]
    );
}

#[test]
fn the_newest_line_of_the_narrator_is_typed_a_few_characters_a_tick() {
    let mut feed = Feed::new();
    assert_eq!(feed.typing(T0), None);
    let born = Duration::from_secs(3);
    let id = feed.push(told(1, "moss", "MOSS fell asleep.", 10), born);
    assert_eq!(feed.typing(born), Some((id, TYPED_PER_TICK)));
    assert_eq!(feed.next_typed(born), Some(born + TICK));
    assert_eq!(feed.typing(born + TICK * 2), Some((id, TYPED_PER_TICK * 3)));
    assert_eq!(
        feed.next_typed(born + TICK * 2 + TICK / 2),
        Some(born + TICK * 3)
    );
    // Only what was typed is laid out.
    let rows = lay(
        &feed,
        &Show {
            width: 30,
            typing: feed.typing(born + TICK),
            ..Show::default()
        },
    );
    assert_eq!(body(&rows), ["MOSS f"]);
    // Typed, it is whole, and nothing is due.
    let done = born + TICK * 6;
    assert_eq!((feed.typing(done), feed.next_typed(done)), (None, None));
    // A line said again, the past, and speech are there at once.
    feed.push(told(1, "moss", "MOSS fell asleep.", 11), done);
    assert_eq!(feed.typing(done), None);
    feed.push(said(1, "moss", "A long answer to the user.", 12), done);
    assert_eq!(feed.typing(done), None);
    let mut past = Feed::new();
    past.insert_past(vec![told(1, "moss", "MOSS used 14 tools.", 5)]);
    assert_eq!(past.typing(T0), None);
}

#[test]
fn the_feed_follows_its_end_until_the_reader_goes_back() {
    let mut feed = Feed::new();
    // 100 rows of which 10 show.
    assert_eq!(feed.window(100, 10), 90);
    assert_eq!(feed.window(4, 10), 0, "shorter than its box");
    assert!(feed.follows());
    feed.scroll(-3, 100, 10);
    assert!(!feed.follows());
    assert_eq!(feed.window(100, 10), 87);
    // It stays there while the feed grows, and counts what arrives.
    feed.push(told(1, "moss", "one", 1), T0);
    feed.push(told(1, "moss", "two", 2), T0);
    feed.push(told(1, "moss", "two", 3), T0);
    assert_eq!(feed.window(104, 10), 87);
    assert_eq!(feed.unseen(), 2, "a line said again is no new entry");
    // Not past the start.
    feed.scroll(-1000, 104, 10);
    assert_eq!(feed.window(104, 10), 0);
    feed.scroll(-1, 104, 10);
    assert_eq!(feed.window(104, 10), 0);
    // Down to the end it follows again, and nothing is unseen.
    feed.scroll(50, 104, 10);
    assert_eq!((feed.window(104, 10), feed.follows()), (50, false));
    feed.scroll(1000, 104, 10);
    assert!(feed.follows());
    assert_eq!((feed.window(104, 10), feed.unseen()), (94, 0));
    // A feed that got shorter than where the reader was (another lion's)
    // shows its end.
    feed.scroll(-60, 104, 10);
    assert_eq!(feed.window(20, 10), 10);
    feed.follow();
    assert!(feed.follows());
    // In a feed shorter than its box there is nowhere to go.
    feed.scroll(-5, 6, 10);
    assert!(feed.follows());
}

#[test]
fn the_keyboard_walks_the_entries_and_brings_each_into_view() {
    let mut feed = Feed::new();
    // Ten entries of three rows each (a heading and two rows), entry n on
    // the rows 3n..3n+3; six rows show.
    let lines: Vec<u64> = (0..10u64).flat_map(|id| [id, id, id]).collect();
    assert_eq!(feed.cursor(), None);
    feed.step_cursor(&lines, 6, true);
    assert_eq!(feed.cursor(), Some(9), "from nothing, the newest");
    assert!(feed.follows(), "which is in view already");
    feed.step_cursor(&lines, 6, true);
    assert_eq!(feed.cursor(), Some(8));
    assert!(feed.follows());
    feed.step_cursor(&lines, 6, true);
    assert_eq!(feed.cursor(), Some(7));
    // Its rows are 21..24: the first of them is the first in view.
    assert_eq!(feed.window(lines.len(), 6), 21);
    for _ in 0..20 {
        feed.step_cursor(&lines, 6, true);
    }
    assert_eq!((feed.cursor(), feed.window(lines.len(), 6)), (Some(0), 0));
    // Forwards again, each comes in at the bottom.
    feed.step_cursor(&lines, 6, false);
    feed.step_cursor(&lines, 6, false);
    assert_eq!(feed.cursor(), Some(2));
    assert_eq!(feed.window(lines.len(), 6), 3);
    // Past the newest the keyboard lets go and the feed follows.
    for _ in 0..7 {
        feed.step_cursor(&lines, 6, false);
    }
    assert_eq!(feed.cursor(), Some(9));
    feed.step_cursor(&lines, 6, false);
    assert_eq!(feed.cursor(), None);
    assert!(feed.follows());
    // An entry that is no longer shown (the feed was narrowed) is nothing.
    feed.step_cursor(&lines, 6, true);
    feed.step_cursor(&[40, 40, 41], 6, true);
    assert_eq!(feed.cursor(), Some(41));
    assert!(feed.drop_cursor());
    assert!(!feed.drop_cursor());
    feed.step_cursor(&[], 6, true);
    assert_eq!(feed.cursor(), None);
}

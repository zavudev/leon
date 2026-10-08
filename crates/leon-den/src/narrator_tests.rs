//! Tests of the narrator: what it says, how it fits the box, and how the
//! queue paces it.

use std::time::Duration;

use crate::model::{Event, Happening, ToolKind};
use crate::narrator::{
    busy_line, move_name, narrate, pondering_line, quiet_line, shout, typing_time, wrap, Line,
    Narrator, NAME_CHARS, ROWS, ROW_CHARS, SHORT_HOLD, TICK, TYPED_PER_TICK, URGENT_HOLD,
};

fn tool(kind: ToolKind, tool: &str, detail: Option<&str>) -> Event {
    Event::ToolStarted {
        kind,
        tool: tool.to_owned(),
        detail: detail.map(str::to_owned),
    }
}

fn say(name: &str, event: Event, seed: u64) -> String {
    narrate(&Happening::new(1, name, event), seed)
        .expect("a line")
        .text()
}

fn every_event() -> Vec<Event> {
    let long = "crates/a-very-long-directory/with/many/parts/and_a_long_file_name_indeed.rs";
    let mut events = vec![
        Event::Joined,
        Event::ToolFinished {
            kind: ToolKind::Run,
            ok: true,
        },
        Event::ToolFinished {
            kind: ToolKind::Edit,
            ok: false,
        },
        Event::SentOut {
            little: "explore-the-whole-codebase".to_owned(),
        },
        Event::CameBack {
            little: "explore".to_owned(),
        },
        Event::PermissionPrompt,
        Event::TurnEnded,
        Event::Mysterious,
        Event::FellAsleep,
        Event::Fainted { exit: Some(101) },
        Event::Fainted { exit: None },
        Event::WentHome,
    ];
    for kind in [
        ToolKind::Edit,
        ToolKind::Read,
        ToolKind::Search,
        ToolKind::Run,
        ToolKind::Web,
        ToolKind::Plan,
        ToolKind::Other,
    ] {
        for detail in [
            None,
            Some("shell.rs"),
            Some(long),
            Some("cargo test -p leon-den --all-targets -- --nocapture some_test_name"),
            Some("a pattern with spaces and \"quotes\" that goes on and on and on and on"),
            Some("https://docs.rs/gpui/latest/gpui/struct.Window.html#method.paint_image"),
        ] {
            events.push(tool(kind, "mcp__server__a_tool_with_a_long_name", detail));
        }
    }
    events
}

#[test]
fn the_lines_of_the_voice_document_are_said_as_written() {
    let edit = tool(ToolKind::Edit, "Edit", Some("shell.rs"));
    let lines: Vec<String> = (0..2).map(|seed| say("moss", edit.clone(), seed)).collect();
    assert!(
        lines.contains(&"MOSS used EDIT on shell.rs!".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"MOSS is typing furiously at\nshell.rs.".to_owned()),
        "{lines:?}"
    );
    let read = tool(ToolKind::Read, "Read", Some("keys.rs"));
    let lines: Vec<String> = (0..2).map(|seed| say("moss", read.clone(), seed)).collect();
    assert!(lines.contains(&"MOSS is sniffing around keys.rs.".to_owned()));
    let search = tool(ToolKind::Search, "Grep", Some("Overlay"));
    let lines: Vec<String> = (0..2)
        .map(|seed| say("moss", search.clone(), seed))
        .collect();
    assert!(lines.contains(&"MOSS empties the shelf looking for\n\"Overlay\"...".to_owned()));
    let run = tool(ToolKind::Run, "Bash", Some("cargo test -p leon-den"));
    let lines: Vec<String> = (0..2).map(|seed| say("moss", run.clone(), seed)).collect();
    assert!(lines.contains(&"MOSS used CARGO TEST!\nWaiting for it to land...".to_owned()));
    assert_eq!(
        say("moss", Event::PermissionPrompt, 0),
        "A wild PERMISSION PROMPT appeared.\nMOSS is staring at you."
    );
    assert_eq!(
        say("moss", Event::TurnEnded, 0),
        "MOSS is done.\nMOSS wants a new order."
    );
    assert_eq!(say("moss", Event::FellAsleep, 0), "MOSS fell asleep.");
    assert_eq!(
        say("moss", Event::Fainted { exit: Some(101) }, 0),
        "MOSS fainted! (exit 101)"
    );
    assert_eq!(say("moss", Event::WentHome, 0), "MOSS went home.");
    assert_eq!(
        say(
            "moss",
            Event::CameBack {
                little: "explore".into()
            },
            0
        ),
        "EXPLORE came back with a report."
    );
    let sent = Event::SentOut {
        little: "explore".into(),
    };
    let lines: Vec<String> = (0..2).map(|seed| say("moss", sent.clone(), seed)).collect();
    assert!(lines.contains(&"MOSS sent out EXPLORE!".to_owned()));
    assert!(lines.contains(&"MOSS sent out EXPLORE!\nAn egg is hatching...".to_owned()));
    assert_eq!(quiet_line().text(), "The den is quiet. Too quiet.");
    assert_eq!(
        busy_line().text(),
        "The pride is busy.\nYou may go get coffee."
    );
    assert_eq!(
        pondering_line("moss").text(),
        "MOSS is thinking very hard.\nOr napping."
    );
}

#[test]
fn every_line_fits_two_rows_of_thirty_four_whatever_the_name_and_the_detail() {
    for name in [
        "m",
        "moss",
        "a-session-with-a-very-long-name",
        "ünïcödé nämé hëre",
        "",
    ] {
        for event in every_event() {
            for seed in 0..3 {
                let Some(line) = narrate(&Happening::new(7, name, event.clone()), seed) else {
                    continue;
                };
                assert!(!line.is_empty(), "{name:?} {event:?}");
                assert!(
                    line.rows.len() <= ROWS,
                    "{name:?} {event:?}: {:?}",
                    line.rows
                );
                for row in &line.rows {
                    assert!(
                        row.chars().count() <= ROW_CHARS,
                        "{name:?} {event:?}: {row:?} is {} wide",
                        row.chars().count()
                    );
                }
                // One exclamation mark, at most.
                assert!(line.text().matches('!').count() <= 1, "{:?}", line.text());
            }
        }
    }
}

#[test]
fn a_name_is_shouted_and_a_long_one_is_cut() {
    assert_eq!(shout("moss"), "MOSS");
    assert_eq!(shout("  fix login  "), "FIX LOGIN");
    let long = shout("a-session-with-a-very-long-name");
    assert_eq!(long.chars().count(), NAME_CHARS);
    assert!(long.ends_with('…'));
    assert_eq!(shout(""), "A CUB");
}

#[test]
fn a_command_is_a_move_its_program_and_its_subcommand() {
    assert_eq!(move_name("cargo test -p leon-den"), "CARGO TEST");
    assert_eq!(move_name("ls"), "LS");
    assert_eq!(move_name("ls -la"), "LS");
    assert_eq!(move_name("RUST_LOG=debug cargo run"), "CARGO RUN");
    assert_eq!(
        move_name("./scripts/generate-icons.sh out"),
        "GENERATE-ICONS.SH"
    );
    assert_eq!(move_name("git status --short"), "GIT STATUS");
    assert_eq!(move_name("python3 scripts/x.py"), "PYTHON3");
    assert_eq!(move_name("   "), "A COMMAND");
}

#[test]
fn a_detail_too_long_is_cut_to_its_file_name_and_then_from_the_end() {
    let edit = |detail: &str| say("moss", tool(ToolKind::Edit, "Edit", Some(detail)), 0);
    let line = edit("crates/app/src/ui/shell.rs");
    assert!(line.contains("crates/app/src/ui/shell.rs") || line.contains("shell.rs"));
    let long = edit("crates/a/very/long/path/that/does/not/fit/in/the/box/at/all/shell.rs");
    assert!(
        long.contains("shell.rs"),
        "the file name survives: {long:?}"
    );
    assert!(!long.contains("crates/a/very"));
    let endless = edit("a_file_name_that_is_on_its_own_far_too_long_for_any_row_of_the_box.rs");
    assert!(endless.contains('…'), "{endless:?}");
    assert!(endless.starts_with("MOSS "));
}

#[test]
fn nothing_is_made_up() {
    let narrated = |event| narrate(&Happening::new(1, "moss", event), 0).map(|line| line.text());
    // Only a command that passed is super effective.
    assert_eq!(
        narrated(Event::ToolFinished {
            kind: ToolKind::Run,
            ok: true
        })
        .as_deref(),
        Some("It's super effective!")
    );
    for kind in [
        ToolKind::Edit,
        ToolKind::Read,
        ToolKind::Search,
        ToolKind::Web,
    ] {
        assert_eq!(narrated(Event::ToolFinished { kind, ok: true }), None);
        assert_eq!(
            narrated(Event::ToolFinished { kind, ok: false }).as_deref(),
            Some("It's not very effective...")
        );
    }
    assert_eq!(
        narrated(Event::Mysterious).as_deref(),
        Some("MOSS is doing something\nmysterious.")
    );
    // A tool with no name is a mystery, not an invention.
    let line = narrated(tool(ToolKind::Other, "", None)).unwrap();
    assert!(line.contains("mysterious"));
    assert_eq!(
        narrated(tool(ToolKind::Other, "WebSearch", None)).as_deref(),
        Some("MOSS used WEBSEARCH!")
    );
}

#[test]
fn the_same_session_and_seed_read_the_same_and_the_next_seed_reads_the_other_way() {
    let edit = tool(ToolKind::Edit, "Edit", Some("shell.rs"));
    assert_eq!(say("moss", edit.clone(), 4), say("moss", edit.clone(), 4));
    assert_ne!(say("moss", edit.clone(), 4), say("moss", edit.clone(), 5));
    assert_eq!(say("moss", edit.clone(), 4), say("moss", edit, 6));
}

#[test]
fn only_a_permission_prompt_and_a_faint_are_urgent() {
    for event in every_event() {
        let urgent = matches!(event, Event::PermissionPrompt | Event::Fainted { .. });
        if let Some(line) = narrate(&Happening::new(1, "moss", event.clone()), 0) {
            assert_eq!(line.urgent, urgent, "{event:?}");
        }
    }
}

#[test]
fn wrapping_breaks_at_spaces_and_keeps_the_breaks_it_is_given() {
    assert_eq!(wrap("one two"), Some(vec!["one two".to_owned()]));
    assert_eq!(
        wrap("one\ntwo"),
        Some(vec!["one".to_owned(), "two".to_owned()])
    );
    let rows = wrap("MOSS is typing furiously at shell.rs.").unwrap();
    assert_eq!(rows, vec!["MOSS is typing furiously at", "shell.rs."]);
    assert_eq!(wrap(&"word ".repeat(30)), None, "three rows do not fit");
    assert_eq!(
        wrap(&"x".repeat(ROW_CHARS + 1)),
        None,
        "a word wider than the box"
    );
}

fn line(text: &str) -> Line {
    Line {
        rows: text.split('\n').map(str::to_owned).collect(),
        urgent: false,
    }
}

fn urgent(text: &str) -> Line {
    Line {
        urgent: true,
        ..line(text)
    }
}

fn secs(seconds: f32) -> Duration {
    Duration::from_secs_f32(seconds)
}

#[test]
fn a_line_is_typed_a_few_characters_a_tick_and_then_stays() {
    let mut narrator = Narrator::new();
    assert_eq!(narrator.said(secs(1.)).rows, Vec::<String>::new());
    narrator.say(line("MOSS fell asleep."), secs(10.));
    let said = narrator.said(secs(10.));
    assert_eq!(said.rows, vec!["MOS"]);
    assert!(!said.complete);
    assert_eq!(said.next, Some(secs(10.) + TICK));
    let said = narrator.said(secs(10.) + TICK * 2);
    assert_eq!(said.rows[0].chars().count(), 3 * TYPED_PER_TICK);
    let done = secs(10.) + typing_time(&line("MOSS fell asleep."));
    let said = narrator.said(done);
    assert_eq!(said.rows, vec!["MOSS fell asleep."]);
    assert!(said.complete && !said.more);
    assert_eq!(said.next, None, "nothing follows: nothing is scheduled");
    assert_eq!(said.since, Some(done));
    // It is still there a minute later.
    assert_eq!(narrator.said(secs(70.)).rows, vec!["MOSS fell asleep."]);
}

#[test]
fn the_second_row_is_typed_after_the_first() {
    let mut narrator = Narrator::new();
    let two = line("MOSS is done.\nMOSS wants a new order.");
    narrator.say(two.clone(), secs(0.));
    let said = narrator.said(TICK * 5);
    assert_eq!(said.rows[0], "MOSS is done.");
    assert_eq!(said.rows[1].chars().count(), 6 * TYPED_PER_TICK - 13);
    assert_eq!(narrator.said(typing_time(&two)).rows, two.rows);
}

#[test]
fn with_reduced_motion_a_line_is_whole_at_once() {
    let mut narrator = Narrator::new();
    narrator.set_reduced_motion(true);
    narrator.say(line("MOSS fell asleep."), secs(3.));
    let said = narrator.said(secs(3.));
    assert_eq!(said.rows, vec!["MOSS fell asleep."]);
    assert!(said.complete);
    assert_eq!(said.next, None);
}

#[test]
fn a_line_waits_for_the_one_before_to_be_read_but_only_a_moment() {
    let mut narrator = Narrator::new();
    let first = line("MOSS used EDIT on shell.rs!");
    narrator.say(first.clone(), secs(0.));
    narrator.say(line("FERN went home."), secs(0.1));
    let turn = typing_time(&first) + SHORT_HOLD;
    let before = narrator.said(turn - TICK);
    assert_eq!(before.rows, first.rows);
    assert!(before.more, "another line waits");
    assert_eq!(before.next, Some(turn));
    assert_eq!(narrator.said(turn).rows, vec!["FER"]);
    assert_eq!(narrator.waiting(secs(0.2)), 1);
    assert_eq!(narrator.waiting(turn), 0);
}

#[test]
fn of_the_chatter_that_waits_only_the_newest_is_said() {
    let mut narrator = Narrator::new();
    narrator.say(line("MOSS used EDIT on a.rs!"), secs(0.));
    for (index, name) in ["b", "c", "d", "e"].iter().enumerate() {
        narrator.say(
            line(&format!("MOSS used EDIT on {name}.rs!")),
            secs(0.1 + index as f32 * 0.05),
        );
    }
    assert_eq!(narrator.waiting(secs(0.3)), 1, "it never falls far behind");
    let mut seen = Vec::new();
    let mut at = Duration::ZERO;
    while at < secs(8.) {
        let said = narrator.said(at);
        if said.complete && seen.last() != Some(&said.rows[0]) {
            seen.push(said.rows[0].clone());
        }
        at += TICK;
    }
    assert_eq!(
        seen,
        vec!["MOSS used EDIT on a.rs!", "MOSS used EDIT on e.rs!"]
    );
}

#[test]
fn what_is_urgent_is_never_dropped_goes_first_and_is_held_longer() {
    let mut narrator = Narrator::new();
    let first = line("MOSS used EDIT on a.rs!");
    narrator.say(first.clone(), secs(0.));
    let prompt = urgent("A wild PERMISSION PROMPT appeared.\nMOSS is staring at you.");
    let faint = urgent("FERN fainted! (exit 101)");
    narrator.say(line("ASH is sniffing around keys.rs."), secs(0.1));
    narrator.say(prompt.clone(), secs(0.2));
    narrator.say(line("ASH went home."), secs(0.3));
    narrator.say(faint.clone(), secs(0.4));
    narrator.say(line("WREN joined the pride."), secs(0.5));
    let mut seen: Vec<String> = Vec::new();
    let mut at = Duration::ZERO;
    while at < secs(20.) {
        let said = narrator.said(at);
        if said.complete && seen.last() != Some(&said.rows.join("\n")) {
            seen.push(said.rows.join("\n"));
        }
        at += TICK;
    }
    assert_eq!(
        seen,
        vec![
            first.text(),
            prompt.text(),
            faint.text(),
            "WREN joined the pride.".to_owned()
        ]
    );
    // The prompt is on screen, whole, for at least its hold.
    let start = typing_time(&first) + SHORT_HOLD;
    let whole = start + typing_time(&prompt);
    assert!(narrator.said(whole).urgent);
    assert_eq!(narrator.said(whole + URGENT_HOLD - TICK).rows, prompt.rows);
}

#[test]
fn the_same_line_twice_in_a_row_is_said_once() {
    let mut narrator = Narrator::new();
    narrator.say(line("It's super effective!"), secs(0.));
    narrator.say(line("It's super effective!"), secs(0.1));
    assert_eq!(narrator.waiting(secs(0.2)), 0);
    narrator.say(
        Line {
            rows: vec![],
            urgent: false,
        },
        secs(0.3),
    );
    assert_eq!(narrator.waiting(secs(0.3)), 0, "an empty line is no line");
}

#[test]
fn every_line_of_the_narrator_has_a_plain_twin_that_says_the_fact_and_nothing_else() {
    use crate::model::{Event, Happening, ToolKind};
    use crate::narrator::{narrate, plainly};
    let tool = |kind: ToolKind, tool: &str, detail: Option<&str>| Event::ToolStarted {
        kind,
        tool: tool.to_owned(),
        detail: detail.map(str::to_owned),
    };
    let cases: Vec<(Event, &str)> = vec![
        (Event::Joined, "Session started."),
        (
            tool(ToolKind::Edit, "Edit", Some("src/shell.rs")),
            "Editing src/shell.rs",
        ),
        (tool(ToolKind::Edit, "Edit", None), "Editing a file."),
        (
            tool(ToolKind::Read, "Read", Some(" keys.rs ")),
            "Reading keys.rs",
        ),
        (
            tool(ToolKind::Search, "Grep", Some("Overlay")),
            "Searching for \"Overlay\"",
        ),
        (
            tool(ToolKind::Run, "Bash", Some("cargo  test\n -p leon")),
            "Running `cargo test -p leon`",
        ),
        (
            tool(ToolKind::Web, "WebFetch", Some("docs.rs/gpui")),
            "Fetching docs.rs/gpui",
        ),
        (tool(ToolKind::Plan, "ExitPlanMode", None), "Planning."),
        (
            tool(ToolKind::Other, "mcp__db__query", Some("select 1")),
            "Using mcp__db__query: select 1",
        ),
        (
            tool(ToolKind::Other, "mcp__db__query", Some("  ")),
            "Using mcp__db__query.",
        ),
        (tool(ToolKind::Other, " ", None), "Using a tool."),
        (
            Event::ToolFinished {
                kind: ToolKind::Run,
                ok: true,
            },
            "The command passed.",
        ),
        (
            Event::ToolFinished {
                kind: ToolKind::Edit,
                ok: false,
            },
            "It failed.",
        ),
        (
            Event::SentOut {
                little: "explore".into(),
            },
            "Started the sub-agent explore.",
        ),
        (
            Event::CameBack {
                little: "explore".into(),
            },
            "The sub-agent explore finished.",
        ),
        (Event::PermissionPrompt, "Needs your permission (inferred)."),
        (Event::TurnEnded, "Finished its turn: waiting for you."),
        (Event::Mysterious, "Working (no details available)."),
        (Event::FellAsleep, "Quiet for a long while."),
        (
            Event::Fainted { exit: Some(101) },
            "Exited with an error (exit 101).",
        ),
        (Event::Fainted { exit: None }, "Exited with an error."),
        (Event::WentHome, "Exited."),
    ];
    for (event, plain) in cases {
        let happening = Happening::new(1, "moss", event);
        assert_eq!(plainly(&happening).as_deref(), Some(plain));
        assert!(
            narrate(&happening, 0).is_some(),
            "{plain}: the narrator says it too"
        );
        assert!(!plain.contains("MOSS") && !plain.contains('!'), "{plain}");
    }
    // Where the narrator says nothing, so does its twin.
    let quiet = Happening::new(
        1,
        "moss",
        Event::ToolFinished {
            kind: ToolKind::Read,
            ok: true,
        },
    );
    assert_eq!((narrate(&quiet, 0), plainly(&quiet)), (None, None));
}

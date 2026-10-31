//! King Leon in the window: the view opens on the service's line, shows
//! King's conversation and its workers as it is told, says what is written
//! to King and to a worker, and opens a worker the person takes over as a
//! session of its own. The service is played by the test
//! (`crate::king::link::testing`).

use super::*;
use crate::king::link::testing::End;
use crate::king::link::Heard;
use leon_king::coordinator::Target;
use leon_king::protocol::{Notice, Request};
use serde_json::{json, Value};

fn open_king(h: &Harness, cx: &mut TestAppContext) -> End {
    h.press_chord("cmd-shift-alt-k", "ctrl-shift-alt-k", cx);
    assert_eq!(h.main_kind(cx), "king");
    let mut ends = h.king.take();
    assert_eq!(ends.len(), 1, "one line to the service");
    ends.remove(0)
}

fn conversation(busy: bool, last_seq: Option<u64>) -> Value {
    json!({
        "agent": "claude", "model": "opus", "effort": "high", "session_id": "s-king",
        "busy": busy, "queued": 0, "questions": [], "cost_usd": 0.12,
        "last_seq": last_seq, "events_seq": last_seq,
    })
}

fn status(brain_seq: Option<u64>, workers: Value) -> Value {
    json!({
        "version": crate::product::VERSION, "events": true,
        "brain": conversation(false, brain_seq),
        "workers": workers,
        "news": 0,
        "spent_today_usd": 0.42,
        "daily_budget_usd": 20.0,
    })
}

fn worker(id: u32, state: &str, last_seq: Option<u64>) -> Value {
    json!({
        "id": id, "title": "Fix the calculator", "project": "api",
        "cwd": "/home/user/api-king-fix-1", "job": "code", "tier": "cheap", "state": state,
        "conversation": {
            "agent": "opencode", "model": "free", "effort": null, "session_id": "ses-7",
            "busy": state == "working", "queued": 0, "questions": [], "cost_usd": 0.0,
            "last_seq": last_seq, "events_seq": last_seq,
        },
        "last_answer": "Found it: the sum is off by one.", "updated_ms": 0,
    })
}

/// What the service answers to `Request::Events`: the records of a log, by
/// number, kind (`prompt`, `agent`, `tool` as `name: summary`, `notice`,
/// `news`) and text.
fn records(items: &[(u64, &str, &str)]) -> Value {
    let record = |(seq, kind, text): &(u64, &str, &str)| match *kind {
        "prompt" => json!({"seq": seq, "at_ms": 0, "record": "prompt", "text": text}),
        "agent" => json!({"seq": seq, "at_ms": 0, "record": "event",
                          "event": {"type": "text", "text": text}}),
        "tool" => {
            let (name, summary) = text.split_once(": ").unwrap_or(("tool", text));
            json!({"seq": seq, "at_ms": 0, "record": "event",
                   "event": {"type": "tool_use", "id": format!("t{seq}"), "name": name,
                             "summary": summary}})
        }
        "notice" => json!({"seq": seq, "at_ms": 0, "record": "notice", "text": text}),
        "news" => json!({"seq": seq, "at_ms": 0, "record": "news", "text": text}),
        other => panic!("{other}"),
    };
    json!({
        "conversation": 0,
        "events": items.iter().map(record).collect::<Vec<_>>(),
        "last_seq": items.last().map(|(seq, ..)| seq),
        "more": false,
    })
}

/// A record of Leon's line under an answer, with `tokens` in its turn.
fn report_record(seq: u64, tokens: u64) -> Value {
    json!({"seq": seq, "at_ms": 0, "record": "report", "report": {
        "turn": {"input": tokens, "cached_input": 0, "output": 0, "duration_ms": 0,
                 "cost_usd": null},
        "workers_today": {"input": 0, "cached_input": 0, "output": 0},
        "king_today": {"input": 0, "cached_input": 0, "output": 0},
        "spent_today_usd": 1.5,
        "budget_usd": null,
        "usage": [{"agent": "claude",
                   "windows": [{"label": "5h", "used": 0.1, "resets_in_secs": 60}]}],
    }})
}

/// What the service answers to `Request::Transcript`.
fn entries(texts: &[(u64, &str, &str)]) -> Value {
    Value::Array(
        texts
            .iter()
            .map(|(seq, kind, text)| json!({"seq": seq, "at_ms": 0, "kind": kind, "text": text}))
            .collect(),
    )
}

/// What the window asks of King's log when it has none yet.
fn events_of(target: Target) -> Request {
    Request::Events {
        target,
        since: None,
        limit: None,
        conversation: None,
    }
}

fn models() -> Value {
    json!({"models": [
        {"agent": "claude", "id": "opus", "label": "Opus", "efforts": ["low", "high"]},
        {"agent": "claude", "id": "haiku", "label": "Haiku", "efforts": []},
        {"agent": "codex", "id": "gpt-5.5", "label": "GPT-5.5", "efforts": ["medium", "high"]},
    ], "modes": [
        {"agent": "claude", "id": "acceptEdits", "label": "Accept edits",
         "description": "edits files without asking", "dangerous": false},
        {"agent": "claude", "id": "bypassPermissions", "label": "Bypass permissions",
         "description": "never asks", "dangerous": true},
    ]})
}

/// Answers the window's requests: `answer` gives the result of each, by
/// what was asked. Returns what was asked.
fn serve(
    h: &Harness,
    end: &End,
    cx: &mut TestAppContext,
    mut answer: impl FnMut(&Request) -> Value,
) -> Vec<Request> {
    h.settle(cx);
    let asked = end.drain();
    for (tag, request) in &asked {
        end.tell
            .send(Heard::Reply {
                tag: *tag,
                result: Ok(answer(request)),
            })
            .unwrap();
    }
    h.settle(cx);
    asked.into_iter().map(|(_, request)| request).collect()
}

/// The view opened, the service told the status and King's conversation.
fn with_conversation(cx: &mut TestAppContext, workers: Value) -> (Harness, End) {
    let h = open(cx, ScriptedRunner::new());
    let end = open_king(&h, cx);
    let asked = serve(&h, &end, cx, |request| match request {
        Request::Status => status(Some(2), workers.clone()),
        Request::Models => models(),
        _ => Value::Null,
    });
    let asked: Vec<Request> = asked
        .into_iter()
        .filter(|request| !matches!(request, Request::Watch { .. }))
        .collect();
    assert_eq!(asked, [Request::Status, Request::Models]);
    let asked = serve(&h, &end, cx, |request| match request {
        Request::Events { .. } => records(&[
            (0, "prompt", "What is the state of api?"),
            (1, "tool", "projects_list: all"),
            (2, "agent", "Everything is green."),
        ]),
        _ => Value::Null,
    });
    assert_eq!(asked, [events_of(Target::Brain)]);
    (h, end)
}

#[gpui_kit::test]
fn the_view_shows_kings_conversation_as_the_service_tells_it(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    assert!(h.shows("king", cx));
    assert!(h.shows_dynamic("king-user-brain-0".into(), cx));
    assert!(h.shows_dynamic("king-work-brain-1".into(), cx));
    assert!(h.shows_dynamic("king-agent-brain-2".into(), cx));
    assert!(h.shows("king-no-workers", cx));
    let (_, title, meta) = h.shell(cx, |s| s.king_heading());
    assert_eq!(title, "The Orchestrator");
    assert!(meta.contains("CLAUDE CODE OPUS HIGH"), "{meta}");
    assert!(meta.contains("$0.42 of $20.00 today"), "{meta}");

    // A notice with more: only what comes after is asked for.
    end.tell
        .send(Heard::Notice(
            serde_json::from_value::<Notice>(json!({
                "notice": "changed", "status": status(Some(4), json!([])),
            }))
            .unwrap(),
        ))
        .unwrap();
    let asked = serve(&h, &end, cx, |_| {
        let mut page = records(&[(3, "agent", "And the tests pass.")]);
        page["events"]
            .as_array_mut()
            .unwrap()
            .push(report_record(4, 1_000));
        page["last_seq"] = json!(4);
        page
    });
    assert_eq!(
        asked,
        [Request::Events {
            target: Target::Brain,
            since: Some(2),
            limit: None,
            conversation: None,
        }]
    );
    assert!(h.shows_dynamic("king-agent-brain-3".into(), cx));
    assert!(h.shows_dynamic("king-report-brain-4".into(), cx));
}

/// A record of an event of the agent, as the service's log keeps it.
fn event_record(seq: u64, event: &leon_turn::TurnEvent) -> Value {
    json!({"seq": seq, "at_ms": 0, "record": "event", "event": event})
}

/// A conversation of King's with a plan, todos and an edit in it, as an
/// agent's events say them.
fn work_conversation(cx: &mut TestAppContext) -> Harness {
    use leon_turn::{
        ChangeKind, DiffLine, FileChange, Hunk, LineKind, Todo, TodoStatus, TurnEvent,
    };
    let h = open(cx, ScriptedRunner::new());
    let end = open_king(&h, cx);
    serve(&h, &end, cx, |request| match request {
        Request::Status => status(Some(5), json!([])),
        Request::Models => models(),
        _ => Value::Null,
    });
    let line = |kind, text: &str, new| DiffLine {
        kind,
        text: text.to_owned(),
        old: None,
        new: Some(new),
    };
    let events = [
        json!({"seq": 0, "at_ms": 0, "record": "prompt", "text": "Fix the sum"}),
        event_record(
            1,
            &TurnEvent::Todos {
                todos: vec![
                    Todo {
                        text: "Find it".into(),
                        status: TodoStatus::Completed,
                    },
                    Todo {
                        text: "Fix it".into(),
                        status: TodoStatus::Pending,
                    },
                ],
            },
        ),
        event_record(
            2,
            &TurnEvent::ToolUse {
                id: "e1".into(),
                name: "Edit".into(),
                summary: "calc.rs".into(),
                detail: String::new(),
            },
        ),
        event_record(
            3,
            &TurnEvent::ToolChange {
                id: "e1".into(),
                changes: vec![FileChange {
                    path: "calc.rs".into(),
                    kind: ChangeKind::Updated,
                    hunks: vec![Hunk {
                        lines: vec![
                            line(LineKind::Removed, "a + b", 1),
                            line(LineKind::Added, "a + b + 1", 1),
                        ],
                    }],
                }],
            },
        ),
        event_record(
            4,
            &TurnEvent::Text {
                text: "Fixed.\n\n```sh\ncargo test\n```".into(),
            },
        ),
    ];
    serve(&h, &end, cx, |request| match request {
        Request::Events { .. } => json!({
            "conversation": 0, "events": events, "last_seq": 4, "more": false,
        }),
        _ => Value::Null,
    });
    h
}

#[gpui_kit::test]
fn kings_conversation_draws_todos_and_edits_as_the_chat_does(cx: &mut TestAppContext) {
    let h = work_conversation(cx);
    // The card of the list, a row each, with how many are done in its title.
    assert!(h.shows_dynamic("king-todos-brain-1".into(), cx));
    assert!(h.shows_dynamic("king-todo-brain-1-0".into(), cx));
    assert!(h.shows_dynamic("king-todo-brain-1-1".into(), cx));
    // The edit stands out of the work with its line and its diff.
    assert!(h.shows_dynamic("king-tool-brain-e1".into(), cx));
    assert!(h.shows_dynamic("king-diff-brain-e1".into(), cx));
    assert!(h.shows_dynamic("king-agent-brain-3".into(), cx));
    // Folding the edit by hand takes its diff away.
    h.mouse_on(
        "king-tool-brain-e1".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.settle(cx);
    assert!(!h.shows_dynamic("king-diff-brain-e1".into(), cx));
}

#[gpui_kit::test]
fn a_run_of_tools_of_king_opens_by_hand_and_shows_each_tool(cx: &mut TestAppContext) {
    let (h, _end) = with_conversation(cx, json!([]));
    // Folded: the line of the run, and not its tools.
    assert!(h.shows_dynamic("king-work-brain-1".into(), cx));
    assert!(!h.shows_dynamic("king-tool-brain-t1".into(), cx));
    h.mouse_on(
        "king-work-brain-1".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.settle(cx);
    assert!(h.shows_dynamic("king-tool-brain-t1".into(), cx));
    assert_eq!(
        h.shell(cx, |shell| shell.king_ui.folds[&Target::Brain].work.len()),
        1
    );
}

#[gpui_kit::test]
fn kings_conversation_is_not_held_to_a_column_of_its_own(cx: &mut TestAppContext) {
    let (h, _end) = with_conversation(cx, json!([]));
    VisualTestContext::from_window(h.window.into(), cx)
        .simulate_resize(gpui_kit::size(px(2400.), px(1000.)));
    h.settle(cx);
    let answer = h
        .bounds_of("king-agent-brain-2".into(), cx)
        .expect("the answer is drawn");
    // It was held to 760 pixels, less its padding.
    assert!(
        f32::from(answer.size.width) > 800.,
        "the answer is {:?} wide",
        answer.size.width
    );
}

#[gpui_kit::test]
fn the_usage_is_in_the_side_column_and_each_report_is_its_numbers(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    end.tell
        .send(Heard::Notice(
            serde_json::from_value::<Notice>(json!({
                "notice": "changed", "status": status(Some(6), json!([])),
            }))
            .unwrap(),
        ))
        .unwrap();
    serve(&h, &end, cx, |_| {
        json!({
            "conversation": 0,
            "events": [
                report_record(3, 1_000),
                report_record(4, 0),
                records(&[(5, "agent", "Fine.")])["events"][0].clone(),
                report_record(6, 2_000),
            ],
            "last_seq": 6,
            "more": false,
        })
    });
    assert!(h.shows_dynamic("king-report-brain-3".into(), cx));
    assert!(!h.shows_dynamic("king-report-brain-4".into(), cx));
    assert!(h.shows_dynamic("king-report-brain-6".into(), cx));
    assert!(h.shows_dynamic("king-usage".into(), cx));
}

#[gpui_kit::test]
fn what_is_written_is_said_to_king_and_escape_closes_the_view(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    h.type_text("Review the open pull requests", cx);
    h.press("enter", cx);
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::Say {
            text: "Review the open pull requests".into()
        }]
    );
    let left = cx.update(|cx| {
        let shell = h.shell.read(cx);
        shell
            .king_ui
            .input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
    });
    assert_eq!(left.as_deref(), Some(""), "the field is empty again");
    h.press("escape", cx);
    assert_eq!(h.main_kind(cx), "empty");
    // The line stays: opening again does not open another.
    h.press_chord("cmd-shift-alt-k", "ctrl-shift-alt-k", cx);
    assert_eq!(h.main_kind(cx), "king");
    assert!(h.king.take().is_empty());
}

fn king_field_text(h: &Harness, cx: &mut TestAppContext) -> String {
    cx.update(|cx| {
        h.shell
            .read(cx)
            .king_ui
            .input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn up_and_down_in_kings_field_walk_the_prompts_said_before(cx: &mut TestAppContext) {
    let (h, _end) = with_conversation(cx, json!([]));
    h.type_text("half a thought", cx);
    // The only prompt of the conversation, then the draft again.
    h.press("up", cx);
    assert_eq!(king_field_text(&h, cx), "What is the state of api?");
    h.press("up", cx);
    assert_eq!(king_field_text(&h, cx), "What is the state of api?");
    h.press("down", cx);
    assert_eq!(king_field_text(&h, cx), "half a thought");
    // Down with nothing to go forward to is the field's own.
    h.press("down", cx);
    assert_eq!(king_field_text(&h, cx), "half a thought");
}

#[gpui_kit::test]
fn a_message_that_waits_is_taken_back_dropped_or_sent_into_the_turn(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    let mut waiting = status(Some(2), json!([]));
    waiting["brain"]["busy"] = json!(true);
    waiting["brain"]["queued"] = json!(2);
    waiting["brain"]["queue"] = json!(["check the tests", "then the docs"]);
    tell_status(&end, waiting);
    h.settle(cx);
    assert!(h.shows_dynamic("king-queue-item-brain-0".into(), cx));
    assert!(h.shows_dynamic("king-queue-item-brain-1".into(), cx));
    // Edit takes the first back: it goes in the field once the service says
    // it was still waiting.
    h.mouse_on(
        "king-queue-edit-brain-0".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    let asked = serve(&h, &end, cx, |request| match request {
        Request::Events { .. } => Value::Null,
        _ => json!({"removed": true}),
    });
    assert!(asked.contains(&Request::Unqueue {
        target: Target::Brain,
        index: 0,
        text: "check the tests".into()
    }));
    assert_eq!(king_field_text(&h, cx), "check the tests");
    // One that went in the meantime is not put in the field.
    h.mouse_on(
        "king-queue-edit-brain-1".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    serve(&h, &end, cx, |_| json!({"removed": false}));
    assert_eq!(king_field_text(&h, cx), "check the tests");
    // Remove and send now say what they mean.
    h.mouse_on(
        "king-queue-remove-brain-0".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::Unqueue {
            target: Target::Brain,
            index: 0,
            text: "check the tests".into()
        }]
    );
    h.mouse_on(
        "king-queue-steer-brain-1".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::Steer {
            target: Target::Brain,
            index: 1,
            text: "then the docs".into()
        }]
    );
}

#[gpui_kit::test]
fn a_workers_queue_has_its_own_rows(cx: &mut TestAppContext) {
    let mut busy = worker(1, "working", Some(0));
    busy["conversation"]["queued"] = json!(1);
    busy["conversation"]["queue"] = json!(["and the tests"]);
    let (h, end) = with_conversation(cx, json!([busy]));
    h.mouse_on("king-card-1".into(), gpui_kit::MouseButton::Left, cx);
    serve(&h, &end, cx, |_| records(&[(0, "prompt", "Find the bug")]));
    assert!(h.shows_dynamic("king-queue-item-worker-1-0".into(), cx));
    h.mouse_on(
        "king-queue-steer-worker-1-0".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::Steer {
            target: Target::Worker(1),
            index: 0,
            text: "and the tests".into()
        }]
    );
}

#[gpui_kit::test]
fn a_workers_card_opens_its_conversation_and_takes_what_is_told(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([worker(1, "working", Some(0))]));
    assert!(h.shows_dynamic("king-card-1".into(), cx));
    // King's workers are never rows of the sidebar.
    assert!(!h
        .outline(cx)
        .iter()
        .any(|row| row.contains("Fix the calculator")));
    h.mouse_on("king-card-1".into(), gpui_kit::MouseButton::Left, cx);
    let asked = serve(&h, &end, cx, |_| {
        records(&[(0, "prompt", "Find the bug in calc.py")])
    });
    assert_eq!(asked, [events_of(Target::Worker(1))]);
    assert!(h.shows_dynamic("king-worker-1".into(), cx));
    assert!(h.shows_dynamic("king-user-worker-1-0".into(), cx));
    // The person steps in.
    h.type_text("Look at the tests too", cx);
    h.press("enter", cx);
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::TellWorker {
            worker: 1,
            text: "Look at the tests too".into()
        }]
    );
    // Its turn is stopped with Escape; the card stays open.
    h.press("escape", cx);
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::Interrupt {
            target: Target::Worker(1)
        }]
    );
    assert!(h.shows_dynamic("king-worker-1".into(), cx));
}

/// What a worker's log says of a turn that edited `calc.rs`, with the
/// checkpoint the service took before it.
fn worker_edit_records() -> Value {
    use leon_turn::{ChangeKind, DiffLine, FileChange, Hunk, LineKind, TurnEvent};
    let line = |kind, text: &str, new| DiffLine {
        kind,
        text: text.to_owned(),
        old: None,
        new: Some(new),
    };
    let events = [
        json!({"seq": 0, "at_ms": 0, "record": "prompt", "text": "Fix the sum"}),
        json!({"seq": 1, "at_ms": 0, "record": "checkpoint", "commit": "abc123"}),
        event_record(
            2,
            &TurnEvent::ToolUse {
                id: "e1".into(),
                name: "Edit".into(),
                summary: "calc.rs".into(),
                detail: String::new(),
            },
        ),
        event_record(
            3,
            &TurnEvent::ToolChange {
                id: "e1".into(),
                changes: vec![FileChange {
                    path: "calc.rs".into(),
                    kind: ChangeKind::Updated,
                    hunks: vec![Hunk {
                        lines: vec![
                            line(LineKind::Removed, "a + b", 1),
                            line(LineKind::Added, "a + b + 1", 1),
                        ],
                    }],
                }],
            },
        ),
        event_record(
            4,
            &TurnEvent::Text {
                text: "Fixed.".into(),
            },
        ),
        event_record(
            5,
            &TurnEvent::Finished {
                ok: true,
                error: None,
                cost_usd: None,
                duration_ms: None,
                interrupted: false,
                denied_tools: Vec::new(),
            },
        ),
    ];
    json!({"conversation": 0, "events": events, "last_seq": 5, "more": false})
}

#[gpui_kit::test]
fn a_workers_turn_that_changed_files_ends_with_the_chats_card_and_opens_its_diff(
    cx: &mut TestAppContext,
) {
    let (h, end) = with_conversation(cx, json!([worker(1, "done", Some(5))]));
    h.mouse_on("king-card-1".into(), gpui_kit::MouseButton::Left, cx);
    serve(&h, &end, cx, |_| worker_edit_records());
    assert!(h.shows_dynamic("king-worker-1".into(), cx));
    // The card of the turn, with the file it changed; the checkpoint is no
    // item of the conversation.
    assert!(h.shows_dynamic("chat-changed-w1-0".into(), cx));
    assert!(h.shows_dynamic("chat-changed-name-w1-0-0".into(), cx));
    assert!(!h.shows_dynamic("chat-diff-panel-w1".into(), cx));
    // The foot of the field: the diffs and the git cell.
    assert!(h.shows_dynamic("king-foot-1".into(), cx));
    assert!(h.shows_dynamic("chat-git-w1".into(), cx));
    // A click on the card opens the panel beside the conversation, on the
    // diff of the file the tools said (the folder is no repository).
    h.mouse_on(
        "chat-changed-file-w1-0-0".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.settle(cx);
    assert!(h.shows_dynamic("chat-diff-panel-w1".into(), cx));
    assert!(h.shows_dynamic("chat-diff-line-0".into(), cx));
    // Escape closes the panel before it closes the card.
    h.press("escape", cx);
    h.settle(cx);
    assert!(!h.shows_dynamic("chat-diff-panel-w1".into(), cx));
    assert!(h.shows_dynamic("king-worker-1".into(), cx));
}

#[gpui_kit::test]
fn the_files_a_workers_message_names_are_links_and_open_where_they_belong(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join("notes.txt"), "hello\nthere\n").unwrap();
    std::fs::write(root.join("clip.mp4"), [0u8, 1, 2, 0, 255]).unwrap();
    std::fs::create_dir(root.join("out")).unwrap();
    let mut here = worker(1, "done", Some(2));
    here["cwd"] = json!(root.to_string_lossy());
    let (h, end) = with_conversation(cx, json!([here]));
    h.mouse_on("king-card-1".into(), gpui_kit::MouseButton::Left, cx);
    let said = "Wrote clip.mp4: 52 s, see `notes.txt:2`, [the folder](out) and gone.bin, v1.2.3.";
    serve(&h, &end, cx, |_| {
        records(&[(0, "prompt", "Render it"), (1, "agent", said)])
    });
    h.settle(cx);
    let (clip, notes, out, gone) = (
        root.join("clip.mp4"),
        root.join("notes.txt"),
        root.join("out"),
        root.join("gone.bin"),
    );
    use crate::ui::links::Opens;
    h.shell(cx, |shell| {
        // Looked at once, off the draw: what is there, and where it opens.
        assert_eq!(shell.links.known(&clip), Some(Some(Opens::System)));
        assert_eq!(shell.links.known(&notes), Some(Some(Opens::Here)));
        assert_eq!(shell.links.known(&out), Some(Some(Opens::System)));
        assert_eq!(shell.links.known(&gone), Some(None));
        assert_eq!(
            shell.links.known(&root.join("v1.2.3")),
            None,
            "a version is no path"
        );
    });
    let cwd = root.to_string_lossy().into_owned();
    let linked = cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.links.linked(said, Some(&cwd), None, cx)
        })
    });
    let url =
        |path: &std::path::Path, line: Option<u32>| crate::chat::paths::url_of(path, line, None);
    assert!(
        linked.contains(&format!("[clip.mp4]({})", url(&clip, None))),
        "{linked}"
    );
    assert!(
        linked.contains(&format!("[`notes.txt:2`]({})", url(&notes, Some(2)))),
        "{linked}"
    );
    assert!(
        linked.contains(&format!("[the folder]({})", url(&out, None))),
        "{linked}"
    );
    assert!(linked.contains("gone.bin,"), "{linked}");
    assert!(!linked.contains(&url(&gone, None)));

    // A click on one: media and folders go to the system, as web links do.
    let open = |h: &Harness, cx: &mut TestAppContext, address: String| {
        cx.update_window(h.window.into(), |_, window, cx| {
            h.shell
                .update(cx, |shell, cx| shell.open_link(&address, window, cx))
        })
        .unwrap();
        h.settle(cx);
    };
    open(&h, cx, url(&clip, None));
    open(&h, cx, url(&out, None));
    assert_eq!(*h.urls.borrow(), [url(&clip, None), url(&out, None)]);
    // Text opens in Leon's editor, at its line, and is no address for the
    // browser.
    open(&h, cx, url(&notes, Some(2)));
    assert_eq!(h.urls.borrow().len(), 2);
    h.shell(cx, |shell| {
        assert!(shell
            .files
            .values()
            .any(|doc| doc.path == notes.to_string_lossy()));
    });
    // A web address goes where it always went.
    open(&h, cx, "https://example.com/a".into());
    assert_eq!(
        h.urls.borrow().last().map(String::as_str),
        Some("https://example.com/a")
    );
    // A file that was deleted is said to be gone and opens nothing.
    std::fs::remove_file(&clip).unwrap();
    open(&h, cx, url(&clip, None));
    assert_eq!(h.urls.borrow().len(), 3);
    assert!(h.status().contains("not there any more"), "{}", h.status());
}

/// A worker that edited `files` in one turn, in a folder that holds them:
/// the records of its log, and the counts git is said to have made.
fn worker_with_files(
    cx: &mut TestAppContext,
    files: &[&str],
) -> (Harness, End, std::path::PathBuf) {
    use leon_turn::{ChangeKind, DiffLine, FileChange, Hunk, LineKind, TurnEvent};
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    // The folder lives as long as the test: the harness keeps it.
    std::mem::forget(dir);
    let mut records = vec![json!({"seq": 0, "at_ms": 0, "record": "prompt", "text": "Go"})];
    for (at, file) in files.iter().enumerate() {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes: &[u8] = if file.ends_with(".mp3") {
            &[0, 1, 0, 2]
        } else {
            b"text\n"
        };
        std::fs::write(&path, bytes).unwrap();
        let id = format!("e{at}");
        records.push(event_record(
            1 + 2 * at as u64,
            &TurnEvent::ToolUse {
                id: id.clone(),
                name: "Write".into(),
                summary: (*file).into(),
                detail: String::new(),
            },
        ));
        records.push(event_record(
            2 + 2 * at as u64,
            &TurnEvent::ToolChange {
                id,
                changes: vec![FileChange {
                    path: (*file).into(),
                    kind: ChangeKind::Created,
                    hunks: vec![Hunk {
                        lines: vec![DiffLine {
                            kind: LineKind::Added,
                            text: "x".into(),
                            old: None,
                            new: Some(1),
                        }],
                    }],
                }],
            },
        ));
    }
    let last = records.len() as u64;
    let mut here = worker(1, "done", Some(last));
    here["cwd"] = json!(root.to_string_lossy());
    let (h, end) = with_conversation(cx, json!([here]));
    h.mouse_on("king-card-1".into(), gpui_kit::MouseButton::Left, cx);
    serve(
        &h,
        &end,
        cx,
        |_| json!({"conversation": 0, "events": records, "last_seq": last, "more": false}),
    );
    // What git counted: the sound files are not text.
    let counted: Vec<crate::chat::changed::Stat> = files
        .iter()
        .map(|file| crate::chat::changed::Stat {
            path: (*file).into(),
            added: 1,
            removed: 0,
            binary: file.ends_with(".mp3"),
        })
        .collect();
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            if let Some(edits) = shell.king_ui.edits.get_mut(&1) {
                edits.measured.insert(0, counted);
            }
            cx.notify();
        })
    });
    h.settle(cx);
    (h, end, root)
}

fn scroll_to_end(h: &Harness, cx: &mut TestAppContext) {
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell
                .king_ui
                .worker_scroll
                .set_offset(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-10_000.)));
            cx.notify();
        })
    });
    h.settle(cx);
}

const TEN: [&str; 10] = [
    "a0.txt",
    "a1.txt",
    "a2.txt",
    "a3.txt",
    "a4.txt",
    "a5.txt",
    "a6.txt",
    "a7.txt",
    "public/audio/approve.mp3",
    "public/audio/reject.mp3",
];

#[gpui_kit::test]
fn a_changed_files_card_shows_more_on_request_says_binary_and_opens_its_files(
    cx: &mut TestAppContext,
) {
    let (h, _end, root) = worker_with_files(cx, &TEN);
    assert!(h.shows_dynamic("chat-changed-w1-0".into(), cx));
    // Eight rows, and a button for the other two.
    assert!(h.shows_dynamic("chat-changed-file-w1-0-7".into(), cx));
    assert!(!h.shows_dynamic("chat-changed-file-w1-0-8".into(), cx));
    assert!(h.shows_dynamic("chat-changed-more-w1-0".into(), cx));
    h.mouse_on(
        "chat-changed-more-w1-0".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(h.shows_dynamic("chat-changed-file-w1-0-8".into(), cx));
    assert!(h.shows_dynamic("chat-changed-file-w1-0-9".into(), cx));
    // A sound is no lines: it says so, and the others count theirs.
    assert!(h.shows_dynamic("chat-changed-counts-w1-0-8-binary".into(), cx));
    assert!(h.shows_dynamic("chat-changed-counts-w1-0-0".into(), cx));
    assert!(!h.shows_dynamic("chat-changed-counts-w1-0-0-binary".into(), cx));
    // Cmd or Ctrl opens the file itself: a sound by the system.
    let secondary = gpui_kit::Modifiers::secondary_key();
    h.mouse_with("chat-changed-file-w1-0-8".into(), secondary, cx);
    let sound = crate::chat::paths::url_of(&root.join("public/audio/approve.mp3"), None, None);
    assert_eq!(*h.urls.borrow(), std::slice::from_ref(&sound));
    // The button of a hovered row does the same.
    scroll_to_end(&h, cx);
    h.hover_on("chat-changed-file-w1-0-9".into(), cx);
    h.mouse_on(
        "chat-changed-open-file-w1-0-9".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    let other = crate::chat::paths::url_of(&root.join("public/audio/reject.mp3"), None, None);
    assert_eq!(*h.urls.borrow(), [sound, other]);
    // Show less takes them away again (the button is at the end of the
    // conversation, which is scrolled to).
    scroll_to_end(&h, cx);
    h.mouse_on(
        "chat-changed-more-w1-0".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(!h.shows_dynamic("chat-changed-file-w1-0-8".into(), cx));

    // A plain click on a row shows what changed in it, in the panel, and
    // opens nothing.
    h.mouse_on(
        "chat-changed-file-w1-0-1".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(h.shows_dynamic("chat-diff-panel-w1".into(), cx));
    assert_eq!(h.urls.borrow().len(), 2);
    // Ctrl or Cmd on a text file opens it in Leon.
    h.mouse_with("chat-changed-file-w1-0-1".into(), secondary, cx);
    h.shell(cx, |shell| {
        assert!(shell
            .files
            .values()
            .any(|doc| doc.path == root.join("a1.txt").to_string_lossy()));
    });
    assert_eq!(h.urls.borrow().len(), 2, "not by the system");
}

#[gpui_kit::test]
fn the_panels_rows_are_one_line_each_and_never_overlap_and_a_binary_file_says_so(
    cx: &mut TestAppContext,
) {
    let (h, _end, root) = worker_with_files(cx, &TEN);
    h.mouse_on(
        "chat-changed-open-w1-0".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(h.shows_dynamic("chat-diff-panel-w1".into(), cx));
    let bounds: Vec<_> = (0..TEN.len())
        .map(|at| {
            h.bounds_of(format!("chat-diff-file-w1-{at}"), cx)
                .unwrap_or_else(|| panic!("row {at} is not drawn"))
        })
        .collect();
    for pair in bounds.windows(2) {
        assert_eq!(
            pair[0].size.height, pair[1].size.height,
            "one height for every row"
        );
        assert!(
            pair[1].origin.y >= pair[0].origin.y + pair[0].size.height,
            "a row starts below the one before: {pair:?}"
        );
    }
    // One line: no taller than the line of the list.
    assert!(bounds[0].size.height <= gpui_kit::px(24.));
    // The list says binary where git counted no lines.
    assert!(h.shows_dynamic("chat-diff-counts-w1-8-binary".into(), cx));
    assert!(h.shows_dynamic("chat-diff-counts-w1-0".into(), cx));

    // The pane of a sound says what it is and opens it.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            if let Some(panel) = shell
                .king_ui
                .edits
                .get_mut(&1)
                .and_then(|edits| edits.diff.as_mut())
            {
                panel.selected = Some("public/audio/approve.mp3".into());
                panel.diff = crate::ui::changes::DiffState::Ready(leon_remote::FileDiff::Binary);
                panel.note = Some("84.2 KB".into());
            }
            cx.notify();
        })
    });
    h.settle(cx);
    assert!(h.shows_dynamic("chat-diff-binary".into(), cx) || h.shows("chat-diff-binary", cx));
    h.mouse_on(
        "chat-diff-binary-open-w1".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    let sound = crate::chat::paths::url_of(&root.join("public/audio/approve.mp3"), None, None);
    assert_eq!(*h.urls.borrow(), [sound]);
    // And so does the button of the title line.
    h.mouse_on(
        "chat-diff-open-selected-w1".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(h.urls.borrow().len(), 2);
}

#[gpui_kit::test]
fn a_workers_files_on_another_machine_are_no_links(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join("a.txt"), "x").unwrap();
    let mut there = worker(1, "done", Some(1));
    there["cwd"] = json!(root.to_string_lossy());
    there["machine"] = json!("build");
    let (h, end) = with_conversation(cx, json!([there]));
    h.mouse_on("king-card-1".into(), gpui_kit::MouseButton::Left, cx);
    serve(&h, &end, cx, |_| {
        records(&[(0, "prompt", "Go"), (1, "agent", "Wrote a.txt")])
    });
    h.settle(cx);
    // Nothing of that machine is looked for on this one.
    h.shell(cx, |shell| {
        assert_eq!(shell.links.known(&root.join("a.txt")), None);
    });
}

#[gpui_kit::test]
fn a_worker_without_changes_has_no_card_and_a_checkpoint_is_no_item(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([worker(1, "done", Some(2))]));
    h.mouse_on("king-card-1".into(), gpui_kit::MouseButton::Left, cx);
    serve(&h, &end, cx, |_| {
        json!({"conversation": 0, "last_seq": 2, "more": false, "events": [
            {"seq": 0, "at_ms": 0, "record": "prompt", "text": "Look around"},
            {"seq": 1, "at_ms": 0, "record": "checkpoint", "commit": "abc123"},
            {"seq": 2, "at_ms": 0, "record": "event",
             "event": {"type": "text", "text": "Nothing to change."}},
        ]})
    });
    assert!(h.shows_dynamic("king-agent-worker-1-1".into(), cx));
    assert!(!h.shows_dynamic("chat-changed-w1-0".into(), cx));
}

#[gpui_kit::test]
fn a_worker_taken_over_opens_as_a_session_of_the_window(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let mut done = worker(3, "done", Some(0));
    done["cwd"] = json!(cwd);
    let (h, end) = with_conversation(cx, json!([done]));
    h.mouse_on("king-card-3".into(), gpui_kit::MouseButton::Left, cx);
    serve(&h, &end, cx, |_| records(&[(0, "agent", "Done.")]));
    h.mouse_on("king-take-over-3".into(), gpui_kit::MouseButton::Left, cx);
    let asked = serve(
        &h,
        &end,
        cx,
        |_| json!({"worker": 3, "agent": "claude", "session_id": "ses-7", "cwd": cwd}),
    );
    assert_eq!(asked, [Request::Release { worker: 3 }]);
    let kind = h.main_kind(cx);
    let id: u64 = kind
        .strip_prefix("live:")
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("the worker is a session of the window now: {kind}"));
    // Its agent's line is typed into the shell from a thread of its own:
    // the test waits for it rather than end while that thread still writes.
    super::live::wait_until(&h, cx, "the worker's conversation resumed", |h, cx| {
        super::live::screen(h, cx, id).contains("--resume ses-7")
    });
}

#[gpui_kit::test]
fn a_line_that_cannot_be_opened_says_why_and_can_be_tried_again(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.king
        .refuse("Orchestrator's service did not start: no such file");
    h.press_chord("cmd-shift-alt-k", "ctrl-shift-alt-k", cx);
    assert!(h.shows("king-unreached", cx));
    let (_, _, meta) = h.shell(cx, |s| s.king_heading());
    assert!(meta.contains("did not start"), "{meta}");
    h.mouse_on("king-connect".into(), gpui_kit::MouseButton::Left, cx);
    let end = h.king.take().pop().expect("a second try");
    serve(&h, &end, cx, |_| status(None, json!([])));
    assert!(h.shows("king-empty", cx));
    // The service goes away: the view says so.
    end.tell.send(Heard::Lost("closed".into())).unwrap();
    h.settle(cx);
    assert!(h.shell(cx, |s| !s.king_ui.connected()));
}

#[gpui_kit::test]
fn the_sidebar_row_opens_king_and_shows_how_it_stands(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    assert!(h.shows("sidebar-king", cx));
    h.mouse_on("sidebar-king".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.main_kind(cx), "king");
    let end = h.king.take().pop().unwrap();
    serve(&h, &end, cx, |_| {
        status(
            None,
            json!([worker(1, "working", None), worker(2, "asking", None)]),
        )
    });
    assert!(h.shows("sidebar-king-glance", cx));
    assert_eq!(
        h.shell(cx, |s| s.king_glance()),
        // A worker's question is King's to answer, not the person's.
        Some((crate::ui::activity::Activity::Working, 2))
    );
}

/// The status with a question King waits on: may it run a command.
fn asking() -> Value {
    let mut now = status(Some(2), json!([]));
    now["brain"]["busy"] = json!(true);
    now["brain"]["questions"] = json!([{
        "id": "q1", "text": "May it run ls?",
        "form": {"type": "permission", "choices": [["a", "allow"], ["b", "always"], ["c", "deny"]]},
    }]);
    now
}

fn tell_status(end: &End, status: Value) {
    end.tell
        .send(Heard::Notice(
            serde_json::from_value::<Notice>(json!({"notice": "changed", "status": status}))
                .unwrap(),
        ))
        .unwrap();
}

#[gpui_kit::test]
fn alt_and_a_number_answer_the_question_in_view(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    tell_status(&end, asking());
    h.settle(cx);
    assert!(h.shows_dynamic("king-question-brain-q1".into(), cx));
    assert!(h.shows_dynamic("king-answer-brain-4".into(), cx));
    // The third answer is the refusal.
    h.press("alt-3", cx);
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::Answer {
            target: Target::Brain,
            question: "q1".into(),
            reply: leon_king::coordinator::Reply {
                choice: Some("c".into()),
                ..Default::default()
            },
        }]
    );
    // The last answers with what is written.
    h.type_text("use the tests instead", cx);
    h.press("alt-4", cx);
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::Answer {
            target: Target::Brain,
            question: "q1".into(),
            reply: leon_king::coordinator::Reply {
                text: Some("use the tests instead".into()),
                ..Default::default()
            },
        }]
    );
    // With nothing asked, the keys are nobody's.
    tell_status(&end, status(Some(2), json!([])));
    h.settle(cx);
    h.press("alt-1", cx);
    assert!(serve(&h, &end, cx, |_| Value::Null).is_empty());
}

#[gpui_kit::test]
fn kings_model_is_changed_from_the_window(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    h.mouse_on("king-brain-chip".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    // The agent in use is first: Enter keeps it.
    h.press("enter", cx);
    h.set_palette_text("haiku", cx);
    h.press("enter", cx);
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::SetBrain {
            agent: "claude".into(),
            model: Some("haiku".into()),
            effort: None,
            mode: None,
        }]
    );
    // How much it asks: a mode of its agent, the rest kept.
    h.mouse_on("king-mode-chip".into(), gpui_kit::MouseButton::Left, cx);
    h.set_palette_text("accept", cx);
    h.press("enter", cx);
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::SetBrain {
            agent: "claude".into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            mode: Some("acceptEdits".into()),
        }]
    );
}

#[gpui_kit::test]
fn the_service_is_told_while_the_person_looks_at_king(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    let watch = |h: &Harness, cx: &mut TestAppContext| -> Vec<bool> {
        serve(h, &end, cx, |_| Value::Null)
            .into_iter()
            .filter_map(|request| match request {
                Request::Watch { on } => Some(on),
                _ => None,
            })
            .collect()
    };
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.window_active = true;
            shell.king_watch(cx);
        })
    });
    let _ = watch(&h, cx);
    assert!(h.shell(cx, |s| s.king_ui.watching));
    // Another window in front: not looking.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.window_active = false;
            shell.king_watch(cx);
        })
    });
    assert_eq!(watch(&h, cx), [false]);
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.window_active = true;
            shell.king_watch(cx);
        })
    });
    assert_eq!(watch(&h, cx), [true]);
    // The view closed: not looking.
    h.press("escape", cx);
    assert_eq!(watch(&h, cx), [false]);
}

#[gpui_kit::test]
fn past_conversations_are_listed_read_and_taken_up_again(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    // A new conversation is asked for from the field's foot.
    h.mouse_on("king-new".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(
        serve(&h, &end, cx, |_| json!({"archived": 0})),
        [Request::NewConversation]
    );
    // The service says so: another conversation, nothing said in it yet.
    let mut fresh = status(None, json!([]));
    fresh["conversation"] = json!(1);
    tell_status(&end, fresh);
    h.settle(cx);
    assert!(
        h.shows("king-empty", cx),
        "what was said before is not shown"
    );

    let past = json!([{
        "id": 0, "title": "What is the state of api?", "started_ms": 0, "ended_ms": 0,
        "agent": "claude", "model": "opus", "session_id": "s-king", "entries": 3,
    }]);
    h.mouse_on("king-tab-history".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(
        serve(&h, &end, cx, |_| past.clone()),
        [Request::Conversations]
    );
    assert!(h.shows_dynamic("king-past-0".into(), cx));
    h.mouse_on("king-past-0".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(
        serve(&h, &end, cx, |_| records(&[(
            0,
            "prompt",
            "What is the state of api?"
        )])),
        [Request::Events {
            target: Target::Brain,
            since: None,
            limit: None,
            conversation: Some(0),
        }]
    );
    assert!(h.shows("king-past-bar", cx));
    assert!(h.shows_dynamic("king-user-brain-0".into(), cx));
    h.mouse_on("king-resume".into(), gpui_kit::MouseButton::Left, cx);
    let asked = serve(&h, &end, cx, |request| match request {
        Request::Conversations => json!([]),
        _ => Value::Null,
    });
    assert_eq!(asked[0], Request::ResumeConversation { conversation: 0 });
    assert!(
        !h.shows("king-past-bar", cx),
        "the one taken up is the one now"
    );
}

#[gpui_kit::test]
fn a_workers_question_is_kings_to_answer_unless_the_person_opens_it(cx: &mut TestAppContext) {
    let mut asking = worker(2, "asking", Some(0));
    asking["conversation"]["questions"] = json!([{
        "id": "d.0", "text": "May it use edit: app.js?",
        "form": {"type": "permission", "choices": [["d.0", "allow"], ["d.1", "always"], ["d.2", "deny"]]},
    }]);
    let (h, end) = with_conversation(cx, json!([asking]));
    // Seen without opening the worker, with nothing to answer it with.
    assert!(h.shows_dynamic("king-question-worker-2-d.0".into(), cx));
    assert!(!h.shows_dynamic("king-answer-worker-2-1".into(), cx));
    assert!(h.shows_dynamic("king-decide-worker-2".into(), cx));
    // The keys answer nothing of it.
    h.press("alt-2", cx);
    assert!(!serve(&h, &end, cx, |_| Value::Null)
        .iter()
        .any(|request| matches!(request, Request::Answer { .. })));
    // Deciding it opens the worker, where it is answered.
    h.mouse_on(
        "king-decide-worker-2".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    serve(&h, &end, cx, |_| json!([]));
    h.press("alt-2", cx);
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert!(asked.contains(&Request::Answer {
        target: Target::Worker(2),
        question: "d.0".into(),
        reply: leon_king::coordinator::Reply {
            choice: Some("d.1".into()),
            ..Default::default()
        },
    }));
}

fn king_field(h: &Harness, cx: &mut TestAppContext) -> String {
    cx.update(|cx| {
        let shell = h.shell.read(cx);
        shell
            .king_ui
            .input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn an_image_on_the_clipboard_is_attached_and_sent_as_a_file_king_opens(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    *h.clip_image.borrow_mut() = Some(crate::clip_image::ClipImage {
        bytes: vec![137, 80, 78, 71],
        extension: "png",
    });
    h.type_text("Look at this", cx);
    h.press_chord("cmd-v", "ctrl-v", cx);
    // The field keeps the words; the image is shown over it.
    assert_eq!(king_field(&h, cx), "Look at this");
    assert!(h.shows_dynamic("king-attached-brain-0".into(), cx));
    let path = h.shell(cx, |s| s.king_ui.attached[&Target::Brain][0].clone());
    assert!(path.starts_with(h.attachments.path()), "{path:?}");
    assert_eq!(std::fs::read(&path).unwrap(), [137, 80, 78, 71]);
    // Sent with the words, as a path to open; then gone from over the field.
    h.press("enter", cx);
    let asked = serve(&h, &end, cx, |_| Value::Null);
    let [Request::Say { text }] = asked.as_slice() else {
        panic!("{asked:?}");
    };
    assert!(text.starts_with("Look at this\n\nAttached"), "{text}");
    assert!(text.contains(&*path.to_string_lossy()), "{text}");
    assert!(!h.shows_dynamic("king-attached-brain-0".into(), cx));
}

#[gpui_kit::test]
fn attached_files_are_shown_can_be_taken_off_and_are_sent(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    *h.picked_files.borrow_mut() = vec![
        std::path::PathBuf::from("/home/ada/spec.pdf"),
        std::path::PathBuf::from("/home/ada/notes.md"),
    ];
    h.mouse_on("king-attach-brain".into(), gpui_kit::MouseButton::Left, cx);
    assert!(h.shows_dynamic("king-attached-brain-1".into(), cx));
    h.mouse_on(
        "king-attached-remove-brain-0".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    // Nothing written: the files alone are sent.
    h.press("enter", cx);
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert_eq!(
        asked,
        [Request::Say {
            text: "Look at the attached files.\n\nAttached (open each path):\n- /home/ada/notes.md"
                .into()
        }]
    );
}

#[gpui_kit::test]
fn what_is_dictated_is_written_in_kings_field(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    {
        let mut told = h.voice.0.lock().unwrap();
        told.installed = vec!["base".into()];
        told.says = "revisa los tests".into();
    }
    h.mouse_on("king-dictate-brain".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.voice.0.lock().unwrap().open, 1);
    h.press("enter", cx);
    assert_eq!(h.voice.0.lock().unwrap().open, 0);
    assert_eq!(king_field(&h, cx), "revisa los tests");
    // Written, not said: Enter again says it.
    assert!(serve(&h, &end, cx, |_| Value::Null).is_empty());
    h.press("enter", cx);
    assert_eq!(
        serve(&h, &end, cx, |_| Value::Null),
        [Request::Say {
            text: "revisa los tests".into()
        }]
    );
}

#[gpui_kit::test]
fn with_no_speech_model_king_lists_them_and_voice_is_not_said_to_king(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    // `/voice` is Leon's: it lists them, and King is told nothing.
    h.type_text("/voice", cx);
    h.press("enter", cx);
    assert!(h.shows("king-voices", cx));
    assert_eq!(king_field(&h, cx), "");
    assert!(serve(&h, &end, cx, |_| Value::Null).is_empty());
    h.mouse_on("king-voices-close".into(), gpui_kit::MouseButton::Left, cx);
    assert!(!h.shows("king-voices", cx));
    // Dictating with none lists them too.
    h.mouse_on("king-dictate-brain".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.voice.0.lock().unwrap().open, 0);
    assert!(h.shows("king-voices", cx));
    // One chosen is downloaded, and dictation uses it.
    // English has models of its own; the rest share the others.
    let base = ["base.en", "base"]
        .into_iter()
        .find(|id| h.shows_dynamic(format!("king-voice-{id}"), cx))
        .expect("a model is listed");
    h.mouse_on(
        format!("king-voice-{base}"),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.settle(cx);
    assert_eq!(h.voice.0.lock().unwrap().installed, [base]);
    h.mouse_on("king-dictate-brain".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.voice.0.lock().unwrap().open, 1);
}

#[gpui_kit::test]
fn scrolled_up_a_button_goes_to_the_newest(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    end.tell
        .send(Heard::Notice(
            serde_json::from_value::<Notice>(json!({
                "notice": "changed", "status": status(Some(80), json!([])),
            }))
            .unwrap(),
        ))
        .unwrap();
    serve(&h, &end, cx, |_| {
        let answers: Vec<(u64, &str, &str)> = (3..=80)
            .map(|seq| (seq, "agent", "A long answer.\n\nWith a second paragraph."))
            .collect();
        records(&answers)
    });
    let scroll = |to_top: bool, cx: &mut TestAppContext| {
        cx.update(|cx| {
            h.shell.update(cx, |shell, cx| {
                if to_top {
                    shell
                        .king_ui
                        .scroll
                        .set_offset(gpui_kit::point(px(0.), px(0.)));
                } else {
                    shell.king_ui.scroll.scroll_to_bottom();
                }
                cx.notify();
            })
        });
        h.settle(cx);
    };
    scroll(false, cx);
    assert!(!h.shows_dynamic("king-latest-brain".into(), cx));
    scroll(true, cx);
    assert!(h.shows_dynamic("king-latest-brain".into(), cx));
    h.mouse_on("king-latest-brain".into(), gpui_kit::MouseButton::Left, cx);
    assert!(!h.shows_dynamic("king-latest-brain".into(), cx));
}

#[gpui_kit::test]
fn what_a_turn_left_running_is_listed_and_stopped(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    let mut now = status(Some(2), json!([]));
    now["running"] = json!([{
        "pid": 4242, "command": "python3 app/server.py --port 8790",
        "of": "brain", "since_ms": 0,
    }]);
    tell_status(&end, now);
    h.settle(cx);
    assert!(h.shows_dynamic("king-running-4242".into(), cx));
    h.mouse_on(
        "king-running-stop-4242".into(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(
        serve(&h, &end, cx, |_| Value::Null),
        [Request::StopProcess { pid: 4242 }]
    );
}

#[gpui_kit::test]
fn a_workers_check_and_its_pull_requests_ci_show_on_its_card(cx: &mut TestAppContext) {
    let mut checked = worker(1, "done", None);
    checked["check"] = json!({
        "command": "scripts/check.sh", "commit": "abc1234def", "dirty": false,
        "state": "passed", "started_ms": 0, "duration_ms": 254_000, "tail": "",
    });
    checked["pull_request"] = json!({
        "url": "https://github.com/zavudev/api/pull/62", "number": 62, "branch": "king/fix",
        "pushed_ms": 0, "ci": "failed", "failing": ["clippy"],
    });
    let (h, _end) = with_conversation(cx, json!([checked, worker(2, "working", None)]));
    assert!(h.shows_dynamic("king-check-1".into(), cx));
    assert!(h.shows_dynamic("king-pr-1".into(), cx));
    // A worker with neither shows no line for them.
    assert!(!h.shows_dynamic("king-evidence-2".into(), cx));
    h.mouse_on("king-pr-1".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(
        *h.urls.borrow(),
        ["https://github.com/zavudev/api/pull/62".to_owned()]
    );
    // The click opened the pull request, not the worker.
    assert_eq!(h.main_kind(cx), "king");
}

#[gpui_kit::test]
fn the_workers_shown_are_the_conversations_on_screen(cx: &mut TestAppContext) {
    let mut earlier = worker(1, "done", None);
    earlier["started_in"] = json!(0);
    let mut busy = worker(2, "working", None);
    busy["started_in"] = json!(0);
    let mut now = worker(3, "done", None);
    now["started_in"] = json!(1);
    let workers = json!([earlier, busy, now]);
    let (h, end) = with_conversation(cx, workers.clone());
    let mut fresh = status(Some(2), workers);
    fresh["conversation"] = json!(1);
    tell_status(&end, fresh);
    h.settle(cx);
    serve(&h, &end, cx, |_| records(&[]));
    // The one now's, and an earlier one's that still works, apart.
    assert!(h.shows_dynamic("king-card-3".into(), cx));
    assert!(!h.shows_dynamic("king-card-1".into(), cx));
    assert!(h.shows_dynamic("king-card-2".into(), cx));
    assert!(h.shows("king-earlier-workers", cx));

    // Reading the past conversation: its workers, and the tab stays on it.
    let past = json!([{
        "id": 0, "title": "Restyle", "started_ms": 0, "ended_ms": 0,
        "agent": "claude", "model": "opus", "session_id": "s", "entries": 1,
    }]);
    h.mouse_on("king-tab-history".into(), gpui_kit::MouseButton::Left, cx);
    serve(&h, &end, cx, |_| past.clone());
    h.mouse_on("king-past-0".into(), gpui_kit::MouseButton::Left, cx);
    serve(&h, &end, cx, |_| records(&[(0, "prompt", "Restyle")]));
    h.mouse_on("king-tab-workers".into(), gpui_kit::MouseButton::Left, cx);
    h.settle(cx);
    assert!(h.shows("king-past-bar", cx));
    assert!(h.shows_dynamic("king-card-1".into(), cx));
    assert!(h.shows_dynamic("king-card-2".into(), cx));
    assert!(!h.shows_dynamic("king-card-3".into(), cx));
}

#[gpui_kit::test]
fn a_terminal_king_asks_for_opens_here_once_this_window_took_it(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    let folder = tempfile::tempdir().unwrap();
    let ask = json!({
        "id": 1, "cwd": folder.path().to_str().unwrap(), "command": "echo ORCHESTRATOR-RAN",
        "title": "Dev server", "since_ms": 0,
    });
    let mut told = status(Some(2), json!([]));
    told["terminals"] = json!([ask.clone()]);
    let notice = |told: &Value| {
        Heard::Notice(
            serde_json::from_value::<Notice>(json!({"notice": "changed", "status": told})).unwrap(),
        )
    };
    end.tell.send(notice(&told)).unwrap();
    let asked = serve(&h, &end, cx, |request| match request {
        Request::TerminalTaken { terminal: 1 } => ask.clone(),
        _ => Value::Null,
    });
    assert!(
        asked.contains(&Request::TerminalTaken { terminal: 1 }),
        "{asked:?}"
    );
    super::live::wait_until(&h, cx, "Orchestrator's terminal", |h, cx| {
        h.shell(cx, |shell| shell.live.all().len() == 1)
    });
    let (id, cwd) = h.shell(cx, |shell| {
        let session = &shell.live.all()[0];
        (session.id.0, session.cwd.clone())
    });
    assert_eq!(cwd, folder.path().to_str().unwrap());
    // The command is typed into the shell from a thread of its own: the test
    // waits for it rather than go on while that thread still writes.
    super::live::wait_until(&h, cx, "Orchestrator's command typed", |h, cx| {
        super::live::screen(h, cx, id).contains("echo ORCHESTRATOR-RAN")
    });

    // Told again before the service forgot it: it is not asked for twice.
    end.tell.send(notice(&told)).unwrap();
    let asked = serve(&h, &end, cx, |_| Value::Null);
    assert!(
        !asked.contains(&Request::TerminalTaken { terminal: 1 }),
        "{asked:?}"
    );
}

#[gpui_kit::test]
fn a_worker_of_an_agent_that_only_runs_in_a_terminal_opens_that_agent_here(
    cx: &mut TestAppContext,
) {
    let (h, end) = with_conversation(cx, json!([]));
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.system = FakeSystem::with(&["kimi"])
        })
    });
    let folder = tempfile::tempdir().unwrap();
    let ask = json!({
        "id": 3, "cwd": folder.path().to_str().unwrap(), "command": "Add the billing schema",
        "agent": "kimi", "title": "Billing schema", "since_ms": 0,
    });
    let mut told = status(Some(2), json!([]));
    told["terminals"] = json!([ask.clone()]);
    end.tell
        .send(Heard::Notice(
            serde_json::from_value::<Notice>(json!({"notice": "changed", "status": told})).unwrap(),
        ))
        .unwrap();
    serve(&h, &end, cx, |request| match request {
        Request::TerminalTaken { terminal: 3 } => ask.clone(),
        _ => Value::Null,
    });
    super::live::wait_until(&h, cx, "the agent's terminal", |h, cx| {
        h.shell(cx, |shell| shell.live.all().len() == 1)
    });
    let (id, cwd, agent) = h.shell(cx, |shell| {
        let session = &shell.live.all()[0];
        (session.id.0, session.cwd.clone(), session.agent)
    });
    assert_eq!(cwd, folder.path().to_str().unwrap());
    assert_eq!(agent, Some(leon_core::AgentId::KIMI));
    super::live::wait_until(&h, cx, "the agent's line typed", |h, cx| {
        super::live::screen(h, cx, id).contains("kimi")
    });
}

#[gpui_kit::test]
fn kings_workers_are_lions_of_the_den_and_a_lion_opens_its_card(cx: &mut TestAppContext) {
    let (h, _end) = with_conversation(cx, json!([worker(2, "working", None)]));
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
    h.settle(cx);
    let lions: Vec<(u64, String, leon_den::CubState)> = h.shell(cx, |shell| {
        shell
            .den
            .cubs
            .iter()
            .map(|cub| (cub.id, cub.name.clone(), cub.state))
            .collect()
    });
    assert_eq!(
        lions,
        [(
            super::super::den::king_id(2),
            "Fix the calculator".to_owned(),
            leon_den::CubState::Thinking
        )]
    );
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.den_open_lion(super::super::den::king_id(2), window, cx)
        })
    })
    .unwrap();
    h.settle(cx);
    assert_eq!(h.main_kind(cx), "king");
    assert_eq!(h.shell(cx, |shell| shell.king_ui.open), Some(2));
}

#[gpui_kit::test]
fn a_pull_request_king_did_not_open_is_the_one_of_the_workers_branch(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([worker(1, "done", None)]));
    assert!(!h.shows_dynamic("king-pr-1".into(), cx));
    let linked = worktree_id(&h, "feature/login");
    let worktree = h
        .store
        .all_worktrees()
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.id == linked)
        .unwrap();
    let project = h.store.project(&worktree.project_id).unwrap();
    h.store
        .update_worktree_status(&linked, |status| {
            status.pull_request = Some(open_pull_request(77));
        })
        .unwrap();
    let mut own = worker(1, "done", None);
    own["cwd"] = json!(worktree.path);
    own["machine"] = json!(project.machine_id.as_str());
    tell_status(
        &end,
        status(Some(2), json!([own, worker(2, "working", None)])),
    );
    h.settle(cx);
    assert!(h.shows_dynamic("king-pr-1".into(), cx), "its branch's");
    assert!(
        !h.shows_dynamic("king-pr-2".into(), cx),
        "a folder GitHub knows nothing of has none"
    );
    h.mouse_on("king-pr-1".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(
        *h.urls.borrow(),
        ["https://github.com/zavudev/leon/pull/77".to_owned()]
    );
    assert_eq!(h.main_kind(cx), "king");
}

/// A status as a service answers that does not keep logs or say its
/// version (one from before the log).
fn old_status(brain_seq: Option<u64>) -> Value {
    let mut old = status(brain_seq, json!([]));
    let fields = old.as_object_mut().unwrap();
    fields.remove("version");
    fields.remove("events");
    old["brain"].as_object_mut().unwrap().remove("events_seq");
    old
}

/// What the window asks after the status it was told, answered with nothing.
fn quiet(h: &Harness, end: &End, cx: &mut TestAppContext) -> Vec<Request> {
    serve(h, end, cx, |request| match request {
        Request::Events { .. } => records(&[]),
        Request::Transcript { .. } => entries(&[]),
        _ => Value::Null,
    })
}

#[gpui_kit::test]
fn an_older_service_is_asked_to_stop_when_idle_and_started_again(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let end = open_king(&h, cx);
    let mut old = status(None, json!([]));
    old["version"] = json!("0.0.1");
    serve(&h, &end, cx, |request| match request {
        Request::Status => old.clone(),
        Request::Models => models(),
        _ => Value::Null,
    });
    let asked = quiet(&h, &end, cx);
    assert!(
        asked.contains(&Request::Shutdown { when_idle: true }),
        "{asked:?}"
    );
    // The service goes when it is idle: the window starts another, as it
    // does when the view opens and nothing answers.
    end.tell.send(Heard::Lost("closed".into())).unwrap();
    h.settle(cx);
    let mut ends = h.king.take();
    assert_eq!(ends.len(), 1, "a new line, to the service that is started");
    let end = ends.remove(0);
    // That one is old as well (it was not replaced): it is not asked again.
    serve(&h, &end, cx, |request| match request {
        Request::Status => old.clone(),
        _ => Value::Null,
    });
    let asked = quiet(&h, &end, cx);
    assert!(
        !asked
            .iter()
            .any(|request| matches!(request, Request::Shutdown { .. })),
        "{asked:?}"
    );
}

#[gpui_kit::test]
fn a_service_of_the_same_or_a_newer_version_is_left_running(cx: &mut TestAppContext) {
    for version in [crate::product::VERSION, "99.0.0"] {
        let h = open(cx, ScriptedRunner::new());
        let end = open_king(&h, cx);
        let mut told = status(None, json!([]));
        told["version"] = json!(version);
        serve(&h, &end, cx, |request| match request {
            Request::Status => told.clone(),
            _ => Value::Null,
        });
        let asked = quiet(&h, &end, cx);
        assert!(
            !asked
                .iter()
                .any(|request| matches!(request, Request::Shutdown { .. })),
            "{version}: {asked:?}"
        );
    }
}

#[gpui_kit::test]
fn a_service_that_says_no_version_is_asked_to_stop_and_read_by_its_transcript(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    let end = open_king(&h, cx);
    let asked = serve(&h, &end, cx, |request| match request {
        Request::Status => old_status(Some(2)),
        _ => Value::Null,
    });
    assert!(asked.contains(&Request::Status));
    // It keeps no log: what it said is read from the entries of its
    // transcript, drawn as the same conversation.
    let asked = serve(&h, &end, cx, |request| match request {
        Request::Transcript { .. } => entries(&[
            (0, "prompt", "What is the state of api?"),
            (1, "tool", "projects_list: all"),
            (2, "agent", "Everything is green."),
        ]),
        _ => Value::Null,
    });
    assert!(
        asked.contains(&Request::Transcript {
            target: Target::Brain,
            since: None,
            limit: Some(leon_king::transcript::KEPT),
        }),
        "{asked:?}"
    );
    assert!(asked.contains(&Request::Shutdown { when_idle: true }));
    assert!(h.shows_dynamic("king-user-brain-0".into(), cx));
    assert!(h.shows_dynamic("king-work-brain-1".into(), cx));
    assert!(h.shows_dynamic("king-agent-brain-2".into(), cx));
}

#[gpui_kit::test]
fn a_past_conversation_the_service_has_no_log_of_is_read_from_its_entries(cx: &mut TestAppContext) {
    let (h, end) = with_conversation(cx, json!([]));
    let past = json!([{
        "id": 0, "title": "Before the log", "started_ms": 0, "ended_ms": 0,
        "agent": "claude", "model": "opus", "session_id": "s-king", "entries": 1,
    }]);
    h.mouse_on("king-tab-history".into(), gpui_kit::MouseButton::Left, cx);
    serve(&h, &end, cx, |_| past.clone());
    h.mouse_on("king-past-0".into(), gpui_kit::MouseButton::Left, cx);
    // No records of it: its transcript is asked for.
    let asked = serve(&h, &end, cx, |_| records(&[]));
    assert_eq!(
        asked,
        [Request::Events {
            target: Target::Brain,
            since: None,
            limit: None,
            conversation: Some(0),
        }]
    );
    let asked = serve(&h, &end, cx, |_| {
        entries(&[(0, "prompt", "Before the log")])
    });
    assert_eq!(asked, [Request::PastConversation { conversation: 0 }]);
    assert!(h.shows("king-past-bar", cx));
    assert!(h.shows_dynamic("king-user-brain-0".into(), cx));
}

#[gpui_kit::test]
fn only_what_comes_after_the_last_record_is_asked_and_a_cut_log_starts_again(
    cx: &mut TestAppContext,
) {
    let (h, end) = with_conversation(cx, json!([]));
    // The log was cut: its newest record is before what the window has.
    tell_status(&end, status(Some(1), json!([])));
    let asked = serve(&h, &end, cx, |_| {
        records(&[(1, "agent", "Only this is left.")])
    });
    assert_eq!(
        asked,
        [Request::Events {
            target: Target::Brain,
            since: Some(2),
            limit: None,
            conversation: None,
        }]
    );
    // Nothing held now: it is asked from the start.
    let asked = serve(&h, &end, cx, |_| {
        records(&[(1, "agent", "Only this is left.")])
    });
    assert_eq!(asked, [events_of(Target::Brain)]);
    assert!(h.shows_dynamic("king-agent-brain-0".into(), cx));
    assert!(!h.shows_dynamic("king-user-brain-0".into(), cx));
}

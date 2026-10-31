//! Tests of conversations in Leon's own interface: the setting, a turn from
//! the prompt to its end with a scripted agent in the place of the process,
//! the agent's questions, and that the agents share one interface.

use super::live::{real_worktree, wait_until};
use super::*;
use crate::chat::model::{Item, Phase};
use crate::chat::turn::Output;
use crate::launch::Launch;
use crate::schema::{self, Value};
use crate::ui::activity::Activity;
use crate::ui::live::LiveId;
use crate::ui::terminals::Place;

const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-test"}"#;
const SAID: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello **there**."}]}}"#;
const BASH: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
const ASK: &str = r#"{"type":"control_request","request_id":"q1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"tool_use_id":"t1"}}"#;
const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s1","queued_turn_count":0,"total_cost_usd":0.02,"duration_ms":1500}"#;

/// A window with the setting turned on, and a folder to work in. `None`
/// where Leon's own interface does not exist (a test run that plays another
/// operating system).
fn window(cx: &mut TestAppContext) -> Option<(Harness, tempfile::TempDir, String)> {
    if !schema::Platform::Unix.here() {
        return None;
    }
    cx.executor().allow_parking();
    let h = open(cx, ScriptedRunner::new());
    cx.update(|cx| {
        settings::set_value(
            cx,
            schema::find("agent_interface").unwrap(),
            Value::Text(crate::chat::UNIFIED.into()),
        )
    });
    let (dir, path) = real_worktree(&h, cx);
    Some((h, dir, path))
}

/// Starts a session of `agent` in `path` the way every command does.
fn start(h: &Harness, cx: &mut TestAppContext, agent: AgentId, path: &str) -> bool {
    let started = cx
        .update_window(h.window.into(), |_, window, cx| {
            h.shell.update(cx, |shell, cx| {
                shell.start_live(
                    Launch::Agent {
                        kind: agent,
                        resume: None,
                        account: None,
                    },
                    &MachineId::local(),
                    path,
                    Place::Session,
                    None,
                    window,
                    cx,
                )
            })
        })
        .unwrap();
    h.settle(cx);
    started
}

/// The person says `text` in the conversation `id`.
fn say(h: &Harness, cx: &mut TestAppContext, id: u64, text: &str) {
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.chat_say(LiveId(id), text.to_owned(), cx);
        })
    });
    h.settle(cx);
}

/// The processes that are turns (not the question of what the agent offers),
/// oldest first: what each was started with.
fn turns(h: &Harness) -> Vec<(usize, crate::chat::turn::Start)> {
    let told = h.chat.0.lock().unwrap();
    told.started
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, start)| start.argv.first().map(String::as_str) != Some("sh"))
        .collect()
}

/// The agent of the newest turn prints `lines`.
fn prints(h: &Harness, cx: &mut TestAppContext, lines: &[&str]) {
    let (at, _) = turns(h).pop().expect("a turn was started");
    {
        let told = h.chat.0.lock().unwrap();
        for line in lines {
            told.outputs[at]
                .send(Output::Line((*line).to_owned()))
                .unwrap();
        }
    }
    h.settle(cx);
}

/// The process of the newest turn ends.
fn exits(h: &Harness, cx: &mut TestAppContext, code: u32) {
    let (at, _) = turns(h).pop().expect("a turn was started");
    h.chat.0.lock().unwrap().outputs[at]
        .send(Output::Exited(code))
        .unwrap();
    h.settle(cx);
}

fn written(h: &Harness) -> Vec<String> {
    h.chat.0.lock().unwrap().written.clone()
}

#[gpui_kit::test]
fn with_the_setting_an_agent_starts_as_a_conversation_and_not_in_a_terminal(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    assert!(start(&h, cx, AgentId::CLAUDE, &path));
    h.shell(cx, |shell| {
        assert_eq!(shell.chats.len(), 1);
        assert!(shell.live.all().is_empty(), "no terminal was started");
        let chat = &shell.chats[&LiveId(1)];
        assert_eq!(chat.agent, AgentId::CLAUDE);
        assert_eq!(chat.cwd, path);
        assert_eq!(shell.focused_chat(), Some(LiveId(1)));
        assert_eq!(shell.pane, Pane::Main);
    });
    assert_eq!(h.main_kind(cx), "live:1");
    assert!(h.shows("chat-empty-1", cx));
    assert!(h.shows("chat-composer-1", cx));
    assert!(h.shows("chat-send-1", cx));
    // It is a session: it has its row in the tree, and nothing ran yet.
    assert!(h.row_of(NodeId::Live(LiveId(1)), cx).is_some());
    assert!(turns(&h).is_empty(), "nothing is started before a prompt");
}

/// The machine of a conversation is where its turn is started: the same
/// conversation, with the other machine's folder and none of this
/// computer's own variables.
#[gpui_kit::test]
fn a_session_on_another_machine_is_a_conversation_whose_turn_starts_there(cx: &mut TestAppContext) {
    let Some((h, _dir, _path)) = window(cx) else {
        return;
    };
    let remote = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|machine| matches!(machine.kind, leon_core::MachineKind::Ssh { .. }))
        .expect("the world has a machine reached over SSH");
    let started = cx
        .update_window(h.window.into(), |_, window, cx| {
            h.shell.update(cx, |shell, cx| {
                shell.start_live(
                    Launch::Agent {
                        kind: AgentId::CLAUDE,
                        resume: None,
                        account: None,
                    },
                    &remote.id,
                    "/srv/app",
                    Place::Session,
                    None,
                    window,
                    cx,
                )
            })
        })
        .unwrap();
    h.settle(cx);
    assert!(started);
    h.shell(cx, |shell| {
        assert_eq!(shell.chats.len(), 1);
        assert!(shell.live.all().is_empty(), "no terminal was started");
        assert_eq!(shell.chats[&LiveId(1)].machine, remote.id);
    });
    say(&h, cx, 1, "hello over there");
    let (_, turn) = turns(&h).pop().expect("a turn was started");
    assert_eq!(turn.machine, remote);
    assert_eq!(turn.cwd, "/srv/app");
    assert!(turn.env.is_empty(), "{:?}", turn.env);
    // What the agent offers is asked of the agent of that machine too.
    let told = h.chat.0.lock().unwrap();
    assert!(told.started.iter().all(|start| start.machine == remote));
}

#[gpui_kit::test]
fn without_the_setting_an_agent_starts_in_a_terminal_as_before(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let h = open(cx, ScriptedRunner::new());
    let (_dir, path) = real_worktree(&h, cx);
    assert!(start(&h, cx, AgentId::CLAUDE, &path));
    h.shell(cx, |shell| {
        assert!(shell.chats.is_empty());
        assert_eq!(shell.live.all().len(), 1);
    });
}

#[gpui_kit::test]
fn a_prompt_typed_and_sent_with_enter_is_a_turn_read_into_the_conversation(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    // The field has the keyboard: what is typed is the prompt.
    h.type_text("say hello", cx);
    h.press("enter", cx);
    let started = turns(&h);
    assert_eq!(started.len(), 1, "one process for the turn");
    let turn = &started[0].1;
    assert_eq!(turn.argv[0], "claude");
    assert!(turn.argv.contains(&"stream-json".to_owned()), "{turn:?}");
    assert!(
        !turn.argv.iter().any(|arg| arg.contains("say hello")),
        "the prompt is never an argument: {turn:?}"
    );
    assert_eq!(turn.cwd, path);
    assert!(
        written(&h).iter().any(|line| line.contains("say hello")),
        "the prompt is written to the agent: {:?}",
        written(&h)
    );
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        assert_eq!(chat.conversation.phase(), Phase::Starting);
        assert_eq!(chat.label(), "say hello");
    });
    let left = cx.update(|cx| {
        let shell = h.shell.read(cx);
        shell.chats[&LiveId(1)].input.read(cx).value().to_string()
    });
    assert_eq!(left, "", "the field is empty again");
    assert!(
        h.shows("chat-stop-1", cx),
        "a turn in flight can be stopped"
    );

    prints(&h, cx, &[INIT, SAID, RESULT]);
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        assert_eq!(chat.conversation.session_id.as_deref(), Some("s1"));
        assert_eq!(chat.conversation.model.as_deref(), Some("claude-test"));
        // What was said is stamped with the time it was said.
        assert!(matches!(
            &chat.conversation.items[..],
            [
                Item::User { text, at: Some(said) },
                Item::Agent { text: answer, streaming: false, at: Some(answered) },
            ] if text == "say hello" && answer == "Hello **there**." && answered >= said
        ));
    });
    assert!(h.shows("chat-time-1-0", cx));
    assert!(h.shows("chat-time-1-1", cx));
    assert!(h.shows("chat-user-1-0", cx));
    assert!(h.shows("chat-agent-1-1", cx));
    assert!(
        h.chat.0.lock().unwrap().closed,
        "an agent that finished its turn is told there is no more"
    );
    exits(&h, cx, 0);
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].conversation.phase(), Phase::Idle);
    });
    assert!(h.shows("chat-send-1", cx));

    // The next prompt continues the agent's own conversation.
    say(&h, cx, 1, "and again");
    let started = turns(&h);
    assert_eq!(started.len(), 2);
    let again = &started[1].1.argv;
    let resume = again.iter().position(|arg| arg == "--resume").unwrap();
    assert_eq!(again[resume + 1], "s1");
}

#[gpui_kit::test]
fn what_the_agent_asks_is_answered_from_the_conversation(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "list the files");
    prints(&h, cx, &[INIT, BASH, ASK]);
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        assert_eq!(chat.conversation.phase(), Phase::Asking);
        assert_eq!(chat.activity(), Activity::NeedsYou);
    });
    assert!(h.shows("chat-question-1-q1", cx));
    assert!(h.shows("chat-choice-once-1", cx));
    // The work it is in the middle of is open, as it happens.
    assert!(h.shows("chat-tool-1-t1", cx));

    let before = written(&h).len();
    h.mouse_on(
        "chat-choice-once-1".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    let answers = written(&h)[before..].to_vec();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert!(answers[0].contains("control_response"), "{answers:?}");
    assert!(answers[0].contains("\"allow\""), "{answers:?}");
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        assert!(chat.conversation.questions.is_empty());
        assert_eq!(chat.conversation.phase(), Phase::Working);
    });
    assert!(!h.shows("chat-question-1-q1", cx));
}

#[gpui_kit::test]
fn an_edit_is_drawn_as_a_diff_and_the_work_says_what_it_did_in_words(cx: &mut TestAppContext) {
    const EDIT: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"a.rs","old_string":"one","new_string":"two"}}]}}"#;
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "rename it");
    prints(&h, cx, &[INIT, BASH, EDIT]);
    h.shell(cx, |shell| {
        let items = &shell.chats[&LiveId(1)].conversation.items;
        assert_eq!(
            crate::chat::model::work_label(&items[1..]),
            "Ran 1 shell command, changed 1 file"
        );
        let Some(Item::Tool(tool)) = items.last() else {
            panic!("{items:?}");
        };
        assert_eq!(tool.changes.len(), 1);
        assert_eq!(tool.changes[0].path, "a.rs");
    });
    // The shell command folds into a line of its own, the edit stands out
    // of it with its diff already open.
    assert!(h.shows("chat-work-1-1", cx));
    assert!(!h.shows("chat-tool-1-t1", cx), "the command is folded");
    assert!(h.shows("chat-tool-1-t2", cx));
    assert!(h.shows("chat-diff-1-t2", cx));
    // Its rows: what went and what came, on their tints.
    h.shell(cx, |shell| {
        let Some(Item::Tool(tool)) = shell.chats[&LiveId(1)].conversation.items.last() else {
            panic!()
        };
        assert_eq!(tool.changes[0].added(), 1);
        assert_eq!(tool.changes[0].removed(), 1);
    });
    let header = h.bounds_of("chat-tool-1-t2".to_owned(), cx).unwrap();
    let diff = h.bounds_of("chat-diff-1-t2".to_owned(), cx).unwrap();
    assert!(
        diff.top() >= header.bottom(),
        "the diff is under its header"
    );
    assert!(diff.size.height >= gpui_kit::px(2. * 16.), "a row each way");
    // A click on the header folds the diff, and another opens it again.
    h.mouse_on("chat-tool-1-t2".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert!(!h.shows("chat-diff-1-t2", cx));
    h.mouse_on("chat-tool-1-t2".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert!(h.shows("chat-diff-1-t2", cx));
    // The folded line opens to the command.
    h.mouse_on("chat-work-1-1".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert!(h.shows("chat-tool-1-t1", cx));
}

#[gpui_kit::test]
fn a_claude_patch_is_drawn_with_the_numbers_of_its_lines(cx: &mut TestAppContext) {
    const EDIT: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"a.rs","old_string":"one","new_string":"two"}}]}}"#;
    const MADE: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"updated"}]},"tool_use_result":{"filePath":"a.rs","structuredPatch":[{"oldStart":41,"oldLines":1,"newStart":41,"newLines":1,"lines":["-one","+two"]}]}}"#;
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "rename it");
    prints(&h, cx, &[INIT, EDIT]);
    h.shell(cx, |shell| {
        let Some(Item::Tool(tool)) = shell.chats[&LiveId(1)].conversation.items.last() else {
            panic!()
        };
        assert_eq!(tool.changes[0].hunks[0].lines[0].new, None, "as given");
    });
    prints(&h, cx, &[MADE]);
    h.shell(cx, |shell| {
        let Some(Item::Tool(tool)) = shell.chats[&LiveId(1)].conversation.items.last() else {
            panic!()
        };
        assert_eq!(tool.changes[0].hunks[0].lines[0].old, Some(41), "as made");
        assert_eq!(tool.changes[0].hunks[0].lines[1].new, Some(41));
    });
    assert!(h.shows("chat-diff-1-t2", cx));
}

#[gpui_kit::test]
fn the_conversation_and_the_field_use_the_whole_width_of_the_pane(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    VisualTestContext::from_window(h.window.into(), cx)
        .simulate_resize(gpui_kit::size(gpui_kit::px(2400.), gpui_kit::px(900.)));
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "say hello");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    let scroll = h.bounds_of("chat-scroll-1".to_owned(), cx).unwrap();
    let said = h.bounds_of("chat-user-1-0".to_owned(), cx).unwrap();
    let answer = h.bounds_of("chat-agent-1-1".to_owned(), cx).unwrap();
    let field = h.bounds_of("chat-composer-1".to_owned(), cx).unwrap();
    assert!(scroll.size.width > gpui_kit::px(1100.), "{scroll:?}");
    // Every part sits inside the pane's padding, on both sides.
    for part in [said, answer, field] {
        assert_eq!(part.left(), scroll.left() + gpui_kit::px(16.), "{part:?}");
        assert_eq!(part.right(), scroll.right() - gpui_kit::px(16.), "{part:?}");
    }
}

#[gpui_kit::test]
fn enter_in_an_empty_field_allows_once_and_the_card_shows_the_change(cx: &mut TestAppContext) {
    const EDIT: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"a.rs","old_string":"one","new_string":"two"}}]}}"#;
    const ASK_EDIT: &str = r#"{"type":"control_request","request_id":"q2","request":{"subtype":"can_use_tool","tool_name":"Edit","input":{"file_path":"a.rs","old_string":"one","new_string":"two"},"tool_use_id":"t2"}}"#;
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "rename it");
    prints(&h, cx, &[INIT, EDIT, ASK_EDIT]);
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        // The tool waiting in the conversation says the change as lines.
        let Some(Item::Tool(tool)) = chat.conversation.items.last() else {
            panic!("{:?}", chat.conversation.items);
        };
        assert!(tool.detail.contains("- one"), "{}", tool.detail);
        assert!(tool.detail.contains("+ two"), "{}", tool.detail);
    });
    let before = written(&h).len();
    // Nothing is written in the field: Enter is the accented answer.
    h.press("enter", cx);
    let answers = written(&h)[before..].to_vec();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert!(answers[0].contains("\"allow\""), "{answers:?}");
    assert!(
        turns(&h).len() == 1,
        "an empty field never starts a turn of its own"
    );
    h.shell(cx, |shell| {
        assert!(shell.chats[&LiveId(1)].conversation.questions.is_empty());
    });
    // With nothing asked and nothing written, Enter does nothing.
    let before = written(&h).len();
    h.press("enter", cx);
    assert_eq!(written(&h).len(), before);
}

#[gpui_kit::test]
fn escape_stops_the_turn_and_what_was_written_meanwhile_is_the_next_one(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::OPENCODE, &path);
    say(&h, cx, 1, "one");
    // opencode takes nothing more while it works: it waits its turn.
    say(&h, cx, 1, "two");
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].conversation.queued.len(), 1);
    });
    assert!(h.shows("chat-queued-1", cx));
    assert_eq!(turns(&h).len(), 1);
    exits(&h, cx, 0);
    assert_eq!(turns(&h).len(), 2, "the queued prompt became a turn");
    h.shell(cx, |shell| {
        assert!(shell.chats[&LiveId(1)].conversation.queued.is_empty());
    });

    // Stopping it clears what waits; an agent that cannot be asked is ended.
    h.press("escape", cx);
    exits(&h, cx, 143);
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        assert_eq!(chat.conversation.phase(), Phase::Idle);
        assert_eq!(shell.focused_chat(), Some(LiveId(1)), "it stays on screen");
    });
}

#[gpui_kit::test]
fn every_agent_of_the_protocol_gets_the_same_interface(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    for (number, agent) in [AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE]
        .into_iter()
        .enumerate()
    {
        let id = number as u64 + 1;
        assert!(start(&h, cx, agent, &path), "{agent:?}");
        h.shell(cx, |shell| {
            assert_eq!(shell.chats[&LiveId(id)].agent, agent);
        });
        for part in ["chat-empty", "chat-composer", "chat-send", "chat-foot"] {
            assert!(
                h.shows_dynamic(format!("{part}-{id}"), cx),
                "{agent:?} {part}"
            );
        }
        say(&h, cx, id, "hello");
        let started = turns(&h);
        let turn = &started.last().unwrap().1;
        assert_eq!(turn.argv[0], agent.as_str(), "{turn:?}");
        assert!(h.shows_dynamic(format!("chat-user-{id}-0"), cx));
        assert!(h.shows_dynamic(format!("chat-stop-{id}"), cx));
    }
}

#[gpui_kit::test]
fn a_conversation_and_the_session_its_agent_wrote_are_one_row(cx: &mut TestAppContext) {
    const INIT_HELLO: &str =
        r#"{"type":"system","subtype":"init","session_id":"hello-3","model":"claude-test"}"#;
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "say hello");
    prints(&h, cx, &[INIT_HELLO, SAID, RESULT]);
    exits(&h, cx, 0);
    h.shell(cx, |shell| {
        assert!(shell.chats[&LiveId(1)].history.is_none())
    });
    // The importer stores the session the agent wrote, under the id it said.
    let session =
        super::live::stored_session(&h, cx, AgentId::CLAUDE, &MachineId::local(), &path, "hello");
    h.settle(cx);
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].history.as_ref(), Some(&session));
        assert_eq!(shell.chat_of_history(&session), Some(LiveId(1)));
    });
    // One row for the two: the session's, which the conversation is behind.
    assert!(h.row_of(NodeId::Session(session.clone()), cx).is_some());
    assert!(h.row_of(NodeId::Live(LiveId(1)), cx).is_none());
    h.shell(cx, |shell| {
        assert!(shell.placement.merged.contains(&session));
        assert!(shell.running_histories().contains(&session));
    });
    assert!(
        h.shows("tree-live-led-1", cx),
        "the row carries the conversation's light"
    );

    // Looked away from and come back to by that row, it is the conversation
    // again: nothing asks to resume what never stopped.
    let row = h.row_of(NodeId::Session(session.clone()), cx).unwrap();
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.main = Main::Empty;
            shell.activate(row, window, cx);
        })
    })
    .unwrap();
    h.settle(cx);
    assert_eq!(h.main_kind(cx), "live:1");
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::None);
    h.shell(cx, |shell| {
        assert_eq!(shell.chats.len(), 1);
        assert!(shell.live.all().is_empty(), "no terminal was started");
        // The keys that act on the session under the cursor find it too.
        assert_eq!(shell.running_of_history(&session), Some(LiveId(1)));
    });
}

#[gpui_kit::test]
fn closing_a_conversation_ends_its_turn_and_removes_its_row(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "work");
    assert!(!h.chat.0.lock().unwrap().killed);
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell
            .update(cx, |shell, cx| shell.close_live(LiveId(1), window, cx))
    })
    .unwrap();
    h.settle(cx);
    assert!(h.chat.0.lock().unwrap().killed, "the process is ended");
    h.shell(cx, |shell| {
        assert!(shell.chats.is_empty());
        assert!(shell.workspaces.workspace_of(LiveId(1)).is_none());
        assert!(!matches!(shell.main, Main::Live(_)));
    });
    assert!(h.row_of(NodeId::Live(LiveId(1)), cx).is_none());
}

#[gpui_kit::test]
fn an_agent_the_protocol_does_not_know_still_starts_in_a_terminal(cx: &mut TestAppContext) {
    let Some((h, _dir, _path)) = window(cx) else {
        return;
    };
    let wants = |cx: &mut TestAppContext, launch: Launch| {
        cx.update(|cx| {
            h.shell
                .read(cx)
                .wants_chat(&launch, &MachineId::local(), cx)
        })
    };
    let agent = |kind| Launch::Agent {
        kind,
        resume: None,
        account: None,
    };
    assert!(wants(cx, agent(AgentId::CLAUDE)));
    assert!(!wants(cx, agent(AgentId::parse("my-agent").unwrap())));
    assert!(!wants(cx, Launch::Shell), "a shell is a terminal");
}

/// What Claude Code answers when it is asked what it offers: two modes, two
/// models (the first can be told how hard to think) and three commands, one
/// of them called like one of Leon's.
const OFFERS: &str = r#"leon-version:2.1.296 (Claude Code)
leon-modes:"default", "acceptEdits"

{"type":"control_response","response":{"subtype":"success","request_id":"o","response":{"commands":[{"name":"compact","description":"Keep a summary","argumentHint":"<instructions>"},{"name":"context","description":"What fills the context","argumentHint":""},{"name":"model","description":"Its own picker","argumentHint":""}],"models":[{"value":"default","displayName":"Default (recommended)","description":"Opus","supportsEffort":true,"supportedEffortLevels":["low","high","xhigh"]},{"value":"haiku","displayName":"Haiku","description":"Fastest"}],"current_permission_mode":"default"}}}"#;

/// The agent answers the question of what it offers with [`OFFERS`].
fn offers(h: &Harness, cx: &mut TestAppContext) {
    {
        let told = h.chat.0.lock().unwrap();
        let at = told
            .started
            .iter()
            .position(|start| start.argv.first().map(String::as_str) == Some("sh"))
            .expect("the agent was asked what it offers");
        for line in OFFERS.lines() {
            told.outputs[at]
                .send(Output::Line(line.to_owned()))
                .unwrap();
        }
        told.outputs[at].send(Output::Exited(0)).unwrap();
    }
    h.settle(cx);
}

/// The labels of the rows of the list open over the field of `id`.
fn listed(h: &Harness, cx: &mut TestAppContext, id: u64) -> Vec<String> {
    cx.update(|cx| {
        h.shell.read(cx).chats[&LiveId(id)]
            .menu_rows(h.shell.read(cx).voices(cx), cx)
            .into_iter()
            .map(|row| row.label)
            .collect()
    })
}

/// What is written in the field of `id`.
fn field(h: &Harness, cx: &mut TestAppContext, id: u64) -> String {
    cx.update(|cx| {
        let shell = h.shell.read(cx);
        shell.chats[&LiveId(id)].input.read(cx).value().to_string()
    })
}

fn click(h: &Harness, cx: &mut TestAppContext, selector: &str) {
    h.mouse_on(selector.to_owned(), gpui_kit::MouseButton::Left, cx);
}

#[gpui_kit::test]
fn a_slash_lists_leons_commands_then_the_agents_and_the_one_chosen_is_written(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    offers(&h, cx);
    assert!(!h.shows("chat-menu-1", cx));

    h.type_text("/", cx);
    assert!(h.shows("chat-menu-1", cx));
    // The agent's own `/model` is Leon's list of its models here.
    assert_eq!(
        listed(&h, cx, 1),
        ["/model", "/effort", "/mode", "/agent", "/voice", "/compact", "/context"]
    );
    h.type_text("co", cx);
    assert_eq!(listed(&h, cx, 1), ["/compact", "/context"]);
    h.press("down", cx);
    h.press("up", cx);
    h.press("up", cx);
    h.shell(cx, |shell| {
        let menu = shell.chats[&LiveId(1)].menu.as_ref().unwrap();
        assert_eq!(menu.selected, 1, "the list goes round at its ends");
    });
    h.press("tab", cx);
    assert_eq!(field(&h, cx, 1), "/context ");
    assert!(!h.shows("chat-menu-1", cx));
    assert!(turns(&h).is_empty(), "choosing a command writes it");
    // What follows is the command's own, and Enter sends the whole.
    h.type_text("now", cx);
    assert!(!h.shows("chat-menu-1", cx));
    h.press("enter", cx);
    assert_eq!(turns(&h).len(), 1);
    assert!(
        written(&h).iter().any(|line| line.contains("/context now")),
        "{:?}",
        written(&h)
    );
}

#[gpui_kit::test]
fn a_command_written_in_full_is_sent_by_enter_and_escape_only_closes_the_list(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    offers(&h, cx);
    h.type_text("/comp", cx);
    h.press("escape", cx);
    assert!(!h.shows("chat-menu-1", cx));
    assert_eq!(field(&h, cx, 1), "/comp", "what was typed stays");
    h.type_text("act", cx);
    assert_eq!(listed(&h, cx, 1), ["/compact"]);
    h.press("enter", cx);
    assert_eq!(field(&h, cx, 1), "");
    assert_eq!(turns(&h).len(), 1);
    assert!(written(&h).iter().any(|line| line.contains("/compact")));

    // While the turn runs, Escape closes an open list before it stops it.
    h.type_text("/", cx);
    assert!(h.shows("chat-menu-1", cx));
    h.press("escape", cx);
    assert!(!h.shows("chat-menu-1", cx));
    assert!(
        !written(&h).iter().any(|line| line.contains("interrupt")),
        "{:?}",
        written(&h)
    );
    // A name nothing is called by is a prompt like any other.
    h.press("backspace", cx);
    h.type_text("/nothing-like-it", cx);
    assert!(h.shows("chat-menu-nothing-1", cx));
    h.press("enter", cx);
    assert_eq!(field(&h, cx, 1), "");
}

#[gpui_kit::test]
fn an_at_lists_the_files_of_the_folder_and_writes_the_path_of_the_one_chosen(
    cx: &mut TestAppContext,
) {
    let Some((h, dir, path)) = window(cx) else {
        return;
    };
    std::fs::create_dir_all(dir.path().join("src/ui")).unwrap();
    for file in ["src/main.rs", "src/ui/chat.rs", "notes.md"] {
        std::fs::write(dir.path().join(file), "x").unwrap();
    }
    start(&h, cx, AgentId::CLAUDE, &path);
    h.type_text("look at @", cx);
    assert!(h.shows("chat-menu-1", cx));
    let all = listed(&h, cx, 1);
    for file in ["src/main.rs", "src/ui/chat.rs", "notes.md"] {
        assert!(all.contains(&file.to_owned()), "{file} in {all:?}");
    }
    h.type_text("chat", cx);
    assert_eq!(listed(&h, cx, 1), ["src/ui/chat.rs"]);
    h.press("enter", cx);
    assert_eq!(field(&h, cx, 1), "look at @src/ui/chat.rs ");
    assert!(!h.shows("chat-menu-1", cx));
    assert!(turns(&h).is_empty());
    h.type_text("please", cx);
    h.press("enter", cx);
    assert!(
        written(&h)
            .iter()
            .any(|line| line.contains("look at @src/ui/chat.rs please")),
        "{:?}",
        written(&h)
    );
}

#[gpui_kit::test]
fn the_chips_open_lists_of_the_agents_modes_and_models_narrowed_by_typing(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    assert!(!h.shows("chat-model-1", cx), "nothing to choose from yet");
    offers(&h, cx);

    click(&h, cx, "chat-model-1");
    assert!(h.shows("chat-menu-filter-1", cx));
    assert_eq!(
        listed(&h, cx, 1),
        ["Default", "Default (recommended)", "Haiku"]
    );
    // The list's own field has the keyboard: the prompt is left alone.
    h.type_text("hai", cx);
    assert_eq!(field(&h, cx, 1), "");
    assert_eq!(listed(&h, cx, 1), ["Haiku"]);
    h.press("enter", cx);
    assert!(!h.shows("chat-menu-1", cx));
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].model.as_deref(), Some("haiku"));
    });
    // The keyboard is the prompt's again, and the turn asks for the model.
    h.type_text("hi", cx);
    assert_eq!(field(&h, cx, 1), "hi");
    h.press("enter", cx);
    let turn = &turns(&h)[0].1;
    assert!(turn.argv.contains(&"haiku".to_owned()), "{turn:?}");

    // `/mode` is the same list as the chip's; while the turn runs the agent
    // is told at once.
    h.type_text("/mode", cx);
    h.press("enter", cx);
    assert_eq!(field(&h, cx, 1), "");
    assert_eq!(listed(&h, cx, 1), ["Ask first", "Accept edits"]);
    click(&h, cx, "chat-menu-row-1-1");
    assert!(!h.shows("chat-menu-1", cx));
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].mode.as_deref(), Some("acceptEdits"));
    });
    assert!(
        written(&h)
            .iter()
            .any(|line| line.contains("set_permission_mode") && line.contains("acceptEdits")),
        "{:?}",
        written(&h)
    );
    // The chip closes the list it opened.
    click(&h, cx, "chat-mode-1");
    assert!(h.shows("chat-menu-1", cx));
    click(&h, cx, "chat-mode-1");
    assert!(!h.shows("chat-menu-1", cx));
}

#[gpui_kit::test]
fn the_effort_chosen_is_the_next_turns_and_goes_with_a_model_that_lacks_it(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    assert!(!h.shows("chat-effort-1", cx), "nothing to choose from yet");
    offers(&h, cx);

    // The model it answers with unless told can be told how hard to think.
    click(&h, cx, "chat-effort-1");
    assert!(h.shows("chat-menu-filter-1", cx));
    assert_eq!(listed(&h, cx, 1), ["Default", "Low", "High", "Extra high"]);
    h.type_text("extra", cx);
    assert_eq!(listed(&h, cx, 1), ["Extra high"]);
    h.press("enter", cx);
    assert!(!h.shows("chat-menu-1", cx));
    h.type_text("hi", cx);
    h.press("enter", cx);
    let turn = &turns(&h)[0].1;
    let at = turn.argv.iter().position(|word| word == "--effort");
    assert_eq!(
        at.map(|at| turn.argv[at + 1].as_str()),
        Some("xhigh"),
        "{turn:?}"
    );

    // `/effort` is the same list as the chip's, with the one in use marked.
    h.type_text("/eff", cx);
    assert_eq!(listed(&h, cx, 1), ["/effort"]);
    h.press("enter", cx);
    assert_eq!(field(&h, cx, 1), "");
    let current = cx.update(|cx| {
        h.shell.read(cx).chats[&LiveId(1)]
            .menu_rows(h.shell.read(cx).voices(cx), cx)
            .into_iter()
            .find(|row| row.current)
            .map(|row| row.label)
    });
    assert_eq!(current.as_deref(), Some("Extra high"));
    h.press("escape", cx);

    // A model without the setting is not asked for an effort, and has no
    // chip to ask it with.
    click(&h, cx, "chat-model-1");
    h.type_text("hai", cx);
    h.press("enter", cx);
    assert!(!h.shows("chat-effort-1", cx));
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].effort, None);
    });
    h.type_text("/eff", cx);
    assert!(!listed(&h, cx, 1).contains(&"/effort".to_owned()));
}

/// What the scripted voice is told.
fn voice<T>(h: &Harness, tell: impl FnOnce(&mut crate::voice::testing::Told) -> T) -> T {
    tell(&mut h.voice.0.lock().unwrap())
}

/// What the notes of the list's rows say.
fn notes(h: &Harness, cx: &mut TestAppContext, id: u64) -> Vec<String> {
    cx.update(|cx| {
        let shell = h.shell.read(cx);
        shell.chats[&LiveId(id)]
            .menu_rows(shell.voices(cx), cx)
            .into_iter()
            .filter_map(|row| row.note)
            .collect()
    })
}

#[gpui_kit::test]
fn dictation_asks_for_a_model_first_and_then_writes_what_was_said(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    voice(&h, |told| {
        told.language = Some("es");
        told.says = " arregla el login ".into();
    });
    start(&h, cx, AgentId::CLAUDE, &path);

    // No model on this computer: the button offers those of the language,
    // and the microphone stays closed.
    click(&h, cx, "chat-dictate-1");
    assert!(h.shows("chat-menu-1", cx));
    assert_eq!(listed(&h, cx, 1), ["Fast", "Balanced", "Accurate"]);
    assert_eq!(
        notes(&h, cx, 1),
        [
            "DOWNLOAD \u{b7} 78 MB",
            "DOWNLOAD \u{b7} 148 MB",
            "DOWNLOAD \u{b7} 190 MB"
        ]
    );
    assert_eq!(voice(&h, |told| told.open), 0);
    assert!(voice(&h, |told| told.downloaded.is_empty()));

    // Choosing one downloads it, and it is the one dictation uses.
    click(&h, cx, "chat-menu-row-1-1");
    assert_eq!(voice(&h, |told| told.downloaded.clone()), ["base"]);
    assert_eq!(
        cx.update(|cx| settings::text(cx, "dictation_model")),
        "base"
    );
    assert_eq!(notes(&h, cx, 1)[1], "ON THIS COMPUTER");
    h.press("escape", cx);
    assert!(!h.shows("chat-menu-1", cx));

    // Now the button opens the microphone; Enter closes it and what was
    // said is in the field, not sent.
    h.type_text("por favor", cx);
    click(&h, cx, "chat-dictate-1");
    assert_eq!(voice(&h, |told| told.open), 1);
    h.shell(cx, |shell| {
        assert_eq!(shell.dictating_in(), Some(LiveId(1)))
    });
    h.press("enter", cx);
    assert_eq!(voice(&h, |told| told.open), 0);
    assert_eq!(
        voice(&h, |told| told.asked.clone()),
        [("base".to_owned(), 16_000, Some("es".to_owned()))]
    );
    assert_eq!(field(&h, cx, 1), "por favor arregla el login");
    assert!(
        turns(&h).is_empty(),
        "what was said is read before it is sent"
    );
    h.press("enter", cx);
    assert_eq!(turns(&h).len(), 1);
}

#[gpui_kit::test]
fn escape_drops_a_dictation_and_what_fails_is_said_by_the_button(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    voice(&h, |told| {
        told.language = Some("en");
        told.installed = vec!["base.en".into()];
        told.says = "never written".into();
    });
    start(&h, cx, AgentId::CLAUDE, &path);

    // Escape closes the microphone: nothing is made of what it heard.
    click(&h, cx, "chat-dictate-1");
    assert_eq!(voice(&h, |told| told.open), 1);
    h.press("escape", cx);
    assert_eq!(voice(&h, |told| told.open), 0);
    assert!(voice(&h, |told| told.asked.is_empty()));
    assert_eq!(field(&h, cx, 1), "");

    // The button closes it as Enter does; a silence is not given a model.
    voice(&h, |told| told.heard = Some(vec![0.0; 16_000]));
    click(&h, cx, "chat-dictate-1");
    click(&h, cx, "chat-dictate-1");
    assert!(voice(&h, |told| told.asked.is_empty()));
    assert!(h.shows("chat-dictate-problem-1", cx));
    assert_eq!(
        h.shell(cx, |shell| shell.dictation.problem.clone())
            .as_deref(),
        Some("Nothing was heard.")
    );

    // A computer without a microphone says so, and asking again clears it.
    voice(&h, |told| {
        told.deaf = Some(crate::voice::Error::Microphone(
            "this computer has none".into(),
        ))
    });
    click(&h, cx, "chat-dictate-1");
    assert_eq!(
        h.shell(cx, |shell| shell.dictation.problem.clone())
            .as_deref(),
        Some("No microphone to record from: this computer has none.")
    );
    h.shell(cx, |shell| assert_eq!(shell.dictating_in(), None));

    // `/voice` lists the models of the language spoken: English has its own.
    h.type_text("/voice", cx);
    h.press("enter", cx);
    assert_eq!(field(&h, cx, 1), "");
    assert_eq!(
        notes(&h, cx, 1),
        [
            "DOWNLOAD \u{b7} 78 MB",
            "ON THIS COMPUTER",
            "DOWNLOAD \u{b7} 488 MB"
        ]
    );
    // A download that fails leaves things as they were, and says why.
    voice(&h, |told| told.refuse = Some(crate::voice::Error::Checksum));
    click(&h, cx, "chat-menu-row-1-0");
    assert_eq!(voice(&h, |told| told.installed.clone()), ["base.en"]);
    assert_eq!(
        h.shell(cx, |shell| shell.dictation.problem.clone())
            .as_deref(),
        Some("The downloaded file is not the model that was meant.")
    );
}

#[gpui_kit::test]
fn the_language_of_dictation_is_the_setting_before_the_systems(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    voice(&h, |told| {
        told.language = Some("en");
        told.installed = vec!["small".into(), "base.en".into()];
        told.says = "hola".into();
    });
    start(&h, cx, AgentId::CLAUDE, &path);
    let language = |cx: &mut TestAppContext, value: &str| {
        cx.update(|cx| {
            settings::set_value(
                cx,
                schema::find("dictation_language").unwrap(),
                Value::Text(value.into()),
            )
        });
    };
    let dictate = |cx: &mut TestAppContext| {
        click(&h, cx, "chat-dictate-1");
        h.press("enter", cx);
        voice(&h, |told| {
            told.asked
                .pop()
                .map(|(model, _, language)| (model, language))
        })
    };
    // The system speaks English, and so does its own model.
    assert_eq!(dictate(cx), Some(("base.en".into(), Some("en".into()))));
    // Spanish by the setting: the model of many, told which language.
    language(cx, "es");
    assert_eq!(dictate(cx), Some(("small".into(), Some("es".into()))));
    // Whichever is heard: the model of many tells.
    language(cx, "auto");
    assert_eq!(dictate(cx), Some(("small".into(), None)));
}

#[gpui_kit::test]
fn the_pull_request_of_a_conversations_branch_is_in_its_foot_and_opens(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    assert!(!h.shows("chat-pr-1", cx), "nothing known, nothing said");
    let worktree = h
        .store
        .all_worktrees()
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.path == path)
        .expect("the worktree of the test");
    h.store
        .update_worktree_status(&worktree.id, |status| {
            status.pull_request = Some(leon_core::PullRequest {
                number: 565,
                state: leon_core::PullState::Open,
                url: "https://github.com/zavudev/zavu/pull/565".into(),
                draft: false,
                review: leon_core::Review::None,
                checks: leon_core::Checks::Passing,
            });
        })
        .unwrap();
    h.settle(cx);
    assert!(h.shows("chat-pr-1", cx));
    h.mouse_on("chat-pr-1".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(
        *h.urls.borrow(),
        ["https://github.com/zavudev/zavu/pull/565".to_owned()]
    );
}

#[gpui_kit::test]
fn a_turn_that_ends_asks_github_for_its_branchs_pull_request_at_once(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "open the pull request");
    let before = h.runner.calls().len();
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    h.settle(cx);
    let asked: Vec<_> = h.runner.calls()[before..]
        .iter()
        .map(|call| format!("{} {}", call.program, call.args.join(" ")))
        .collect();
    assert!(
        asked
            .iter()
            .any(|call| call.contains("remote") && call.contains("origin")),
        "where its origin is, for GitHub: {asked:?}"
    );
}

#[gpui_kit::test]
fn an_image_pasted_in_a_conversation_is_sent_as_a_file_the_agent_opens(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    *h.clip_image.borrow_mut() = Some(crate::clip_image::ClipImage {
        bytes: vec![137, 80, 78, 71],
        extension: "png",
    });
    h.type_text("Look at this", cx);
    h.press_chord("cmd-v", "ctrl-v", cx);
    // The field keeps the words; the image is shown over it.
    assert!(h.shows_dynamic("chat-attached-1-0".into(), cx));
    let image = h.shell(cx, |s| s.chats[&LiveId(1)].attached[0].clone());
    assert!(image.starts_with(h.attachments.path()), "{image:?}");
    assert_eq!(std::fs::read(&image).unwrap(), [137, 80, 78, 71]);
    // Sent with the words, as a path to open; then gone from over the field.
    h.press("enter", cx);
    let said = written(&h).join("\n");
    assert!(said.contains("Look at this"), "{said}");
    assert!(said.contains(&*image.to_string_lossy()), "{said}");
    assert!(!h.shows_dynamic("chat-attached-1-0".into(), cx));
}

#[gpui_kit::test]
fn what_went_wrong_with_dictation_never_covers_the_fields_foot(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    offers(&h, cx);
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.dictation.problem = Some("Something went wrong with dictation. ".repeat(8));
            cx.notify();
        })
    });
    h.settle(cx);
    let foot = h.bounds_of("chat-mode-1".into(), cx).expect("the mode");
    let problem = h
        .bounds_of("chat-dictate-problem-1".into(), cx)
        .expect("the problem");
    let dictate = h.bounds_of("chat-dictate-1".into(), cx).expect("dictate");
    assert!(problem.left() >= foot.right(), "{problem:?} over {foot:?}");
    assert!(
        problem.right() <= dictate.left(),
        "{problem:?} over {dictate:?}"
    );
}

#[gpui_kit::test]
fn a_turn_at_work_turns_a_spinner_and_says_for_how_long(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "work");
    assert!(h.shows("chat-spinner-1", cx), "starting turns it");
    prints(&h, cx, &[INIT, BASH]);
    assert!(h.shows("chat-spinner-1", cx), "working turns it");
    h.shell(cx, |shell| assert!(shell.chats[&LiveId(1)].since.is_some()));
    prints(&h, cx, &[SAID, RESULT]);
    exits(&h, cx, 0);
    assert!(
        !h.shows("chat-status-1", cx),
        "a turn that ended says nothing"
    );
    assert!(!h.shows("chat-spinner-1", cx));
}

#[gpui_kit::test]
fn what_the_agent_wrote_can_be_selected_by_dragging_and_copied(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "say hello");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    let message = h
        .bounds_of("chat-agent-1-1".to_owned(), cx)
        .expect("the answer is on screen");
    let mut visual = VisualTestContext::from_window(h.window.into(), cx);
    let from = gpui_kit::point(
        message.left() + gpui_kit::px(1.),
        message.bottom() - gpui_kit::px(8.),
    );
    let to = gpui_kit::point(
        message.right() - gpui_kit::px(1.),
        message.bottom() - gpui_kit::px(4.),
    );
    visual.simulate_mouse_down(
        from,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::default(),
    );
    visual.simulate_mouse_move(
        to,
        Some(gpui_kit::MouseButton::Left),
        gpui_kit::Modifiers::default(),
    );
    visual.simulate_mouse_up(
        to,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::default(),
    );
    visual.run_until_parked();
    let selected = visual.update(gpui_kit::base::TextSelection::selected_text);
    assert!(selected.contains("Hello"), "selected: {selected:?}");

    // The usual chord copies it, though the field has the keyboard.
    let chord = if crate::platform::is_mac() {
        "cmd-c"
    } else {
        "ctrl-c"
    };
    h.press(chord, cx);
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some(selected.trim()));

    // What the person said is selected the same way.
    let said = h
        .bounds_of("chat-user-1-0".to_owned(), cx)
        .expect("the prompt is on screen");
    let mut visual = VisualTestContext::from_window(h.window.into(), cx);
    let from = gpui_kit::point(said.left() + gpui_kit::px(30.), said.center().y);
    // Past the words, before the time at the end of the line.
    let to = gpui_kit::point(said.right() - gpui_kit::px(90.), said.center().y);
    visual.simulate_mouse_down(
        from,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::default(),
    );
    visual.simulate_mouse_move(
        to,
        Some(gpui_kit::MouseButton::Left),
        gpui_kit::Modifiers::default(),
    );
    visual.simulate_mouse_up(
        to,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::default(),
    );
    visual.run_until_parked();
    let selected = visual.update(gpui_kit::base::TextSelection::selected_text);
    assert!(selected.contains("hello"), "selected: {selected:?}");
}

#[gpui_kit::test]
fn files_are_attached_by_the_paperclip_and_by_pasting_them_from_the_file_manager(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    // The paperclip asks for files; the ones picked show over the field.
    let picked = std::path::PathBuf::from(&path).join("notes.md");
    *h.picked_files.borrow_mut() = vec![picked.clone()];
    click(&h, cx, "chat-attach-1");
    h.settle(cx);
    assert_eq!(
        h.shell(cx, |s| s.chats[&LiveId(1)].attached.clone()),
        std::slice::from_ref(&picked)
    );
    // Files copied in the file manager are attached as they are, not pasted
    // as their paths.
    let copied = std::path::PathBuf::from(&path).join("shot.png");
    cx.update(|cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem {
            entries: vec![gpui_kit::ClipboardEntry::ExternalPaths(
                gpui_kit::ExternalPaths(vec![copied.clone()].into()),
            )],
        })
    });
    h.press_chord("cmd-v", "ctrl-v", cx);
    assert_eq!(
        h.shell(cx, |s| s.chats[&LiveId(1)].attached.clone()),
        [picked.clone(), copied.clone()]
    );
    // Sent with what is said, as paths to open.
    h.type_text("read these", cx);
    h.press("enter", cx);
    let said = written(&h).join("\n");
    assert!(said.contains(&*picked.to_string_lossy()), "{said}");
    assert!(said.contains(&*copied.to_string_lossy()), "{said}");
}

// ---- Edit from here --------------------------------------------------

/// Where Claude Code's sessions are, for this test: a folder of its own.
fn sessions_in(h: &Harness, folder: &std::path::Path) {
    let mut prefs = h.engine.prefs();
    prefs.roots.claude_projects = Some(folder.to_owned());
    h.engine.set_prefs(prefs);
}

/// A turn that the agent answers and ends.
fn answered(h: &Harness, cx: &mut TestAppContext, id: u64, prompt: &str) {
    say(h, cx, id, prompt);
    prints(h, cx, &[INIT, SAID, RESULT]);
    exits(h, cx, 0);
}

/// The items of a conversation, as the kinds of words they are.
fn said_in(h: &Harness, cx: &mut TestAppContext, id: u64) -> Vec<String> {
    h.shell(cx, |shell| {
        shell.chats[&LiveId(id)]
            .conversation
            .items
            .iter()
            .map(|item| match item {
                Item::User { text, .. } => format!("user: {text}"),
                Item::Agent { text, .. } => format!("agent: {text}"),
                Item::Notice { text, .. } => format!("notice: {text}"),
                Item::Switched { from, to } => format!("switch: {from} to {to}"),
                _ => "work".to_owned(),
            })
            .collect()
    })
}

fn git(folder: &str, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(folder)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A repository in `path` with `a.txt` committed.
fn git_repo(path: &str) {
    git(path, &["init", "-q", "-b", "trunk"]);
    std::fs::write(format!("{path}/a.txt"), "a0\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-q", "-m", "first"]);
}

/// Hovers the words at `at` of conversation 1 and uses the action `name`.
fn from_here(h: &Harness, cx: &mut TestAppContext, name: &str, at: usize) {
    h.hover_on(format!("chat-user-1-{at}"), cx);
    click(h, cx, &format!("{name}-1-{at}"));
}

#[gpui_kit::test]
fn what_the_person_sent_can_be_edited_or_retried_but_not_while_a_turn_runs(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "first");
    prints(&h, cx, &[INIT, SAID]);
    assert!(!h.shows_dynamic("chat-edit-1-0".into(), cx), "a turn runs");
    prints(&h, cx, &[RESULT]);
    exits(&h, cx, 0);
    h.hover_on("chat-user-1-0".into(), cx);
    assert!(h.shows_dynamic("chat-edit-1-0".into(), cx));
    assert!(h.shows_dynamic("chat-retry-1-0".into(), cx));
    // Only what the person sent has them.
    assert!(!h.shows_dynamic("chat-edit-1-1".into(), cx));
}

#[gpui_kit::test]
fn edit_from_here_cuts_the_conversation_and_puts_the_words_back_in_the_field(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    let nothing = tempfile::tempdir().unwrap();
    sessions_in(&h, nothing.path());
    start(&h, cx, AgentId::CLAUDE, &path);
    answered(&h, cx, 1, "first");
    answered(&h, cx, 1, "second");
    from_here(&h, cx, "chat-edit", 2);
    // A folder that is no repository has no files to put back.
    assert!(h.shows("chat-rewind-1", cx));
    assert!(!h.shows("chat-rewind-files-1", cx));
    assert!(h.shows("chat-rewind-keep-1", cx));
    click(&h, cx, "chat-rewind-keep-1");
    let said = said_in(&h, cx, 1);
    assert_eq!(said.len(), 3, "{said:?}");
    assert_eq!(said[0], "user: first");
    assert_eq!(said[1], "agent: Hello **there**.");
    // The agent's session could not be copied: it is said, and it starts over.
    assert!(said[2].starts_with("notice: Context was reset"), "{said:?}");
    assert_eq!(field(&h, cx, 1), "second");
    assert!(!h.shows("chat-rewind-1", cx));
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        assert_eq!(chat.conversation.session_id, None);
        assert_eq!(chat.conversation.phase(), Phase::Idle);
        assert_eq!(chat.marks.len(), 1, "the turn that is gone has no mark");
        assert!(chat.rewinding.is_none());
    });
    // Nothing was sent: the words wait in the field to be changed.
    assert_eq!(turns(&h).len(), 2);
    // The first message goes back to an empty conversation.
    from_here(&h, cx, "chat-edit", 0);
    click(&h, cx, "chat-rewind-keep-1");
    assert!(said_in(&h, cx, 1).is_empty());
    assert_eq!(field(&h, cx, 1), "first");
}

#[gpui_kit::test]
fn cancelling_the_question_leaves_the_conversation_as_it_was(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    answered(&h, cx, 1, "first");
    from_here(&h, cx, "chat-edit", 0);
    assert!(h.shows("chat-rewind-1", cx));
    click(&h, cx, "chat-rewind-cancel-1");
    assert!(!h.shows("chat-rewind-1", cx));
    assert_eq!(said_in(&h, cx, 1).len(), 2);
    // Escape closes it too.
    from_here(&h, cx, "chat-edit", 0);
    assert!(h.shows("chat-rewind-1", cx));
    h.press("escape", cx);
    assert!(!h.shows("chat-rewind-1", cx));
    assert_eq!(said_in(&h, cx, 1).len(), 2);
}

#[gpui_kit::test]
fn retry_goes_back_and_sends_the_same_words_again(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    let nothing = tempfile::tempdir().unwrap();
    sessions_in(&h, nothing.path());
    start(&h, cx, AgentId::CLAUDE, &path);
    answered(&h, cx, 1, "first");
    answered(&h, cx, 1, "second");
    from_here(&h, cx, "chat-retry", 2);
    click(&h, cx, "chat-rewind-keep-1");
    let said = said_in(&h, cx, 1);
    assert_eq!(
        said.last().map(String::as_str),
        Some("user: second"),
        "{said:?}"
    );
    assert_eq!(
        said.iter().filter(|line| *line == "user: second").count(),
        1
    );
    assert_eq!(turns(&h).len(), 3, "a turn was started again");
    assert!(written(&h).last().unwrap().contains("second"));
    assert_eq!(field(&h, cx, 1), "");
    // It starts a new session: the agent could not take the old one back.
    let argv = &turns(&h).pop().unwrap().1.argv;
    assert!(!argv.contains(&"--resume".to_owned()), "{argv:?}");
}

#[gpui_kit::test]
fn claudes_session_is_forked_where_the_turn_began_and_the_next_turn_resumes_the_fork(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    let projects = tempfile::tempdir().unwrap();
    sessions_in(&h, projects.path());
    let folder = projects
        .path()
        .join(leon_history::live::claude_project_dir_name(&path));
    std::fs::create_dir_all(&folder).unwrap();
    let transcript = folder.join("s1.jsonl");
    start(&h, cx, AgentId::CLAUDE, &path);
    answered(&h, cx, 1, "first");
    // What Claude Code wrote of the first turn, and then of the second.
    let first = "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"s1\"}\n";
    std::fs::write(&transcript, first).unwrap();
    say(&h, cx, 1, "second");
    let both = format!("{first}{{\"type\":\"user\",\"uuid\":\"u2\",\"sessionId\":\"s1\"}}\n");
    std::fs::write(&transcript, &both).unwrap();
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    from_here(&h, cx, "chat-edit", 2);
    click(&h, cx, "chat-rewind-keep-1");
    let new = h.shell(cx, |shell| {
        shell.chats[&LiveId(1)].conversation.session_id.clone()
    });
    let new = new.expect("the fork is the session now");
    assert_ne!(new, "s1");
    let forked = std::fs::read_to_string(folder.join(format!("{new}.jsonl"))).unwrap();
    assert!(forked.contains("u1") && !forked.contains("u2"), "{forked}");
    assert!(forked.contains(&new), "{forked}");
    assert_eq!(std::fs::read_to_string(&transcript).unwrap(), both);
    // Nothing is lost of what came before: no notice of a reset.
    assert!(
        !said_in(&h, cx, 1)
            .iter()
            .any(|line| line.starts_with("notice:")),
        "{:?}",
        said_in(&h, cx, 1)
    );
    say(&h, cx, 1, "other words");
    let argv = &turns(&h).pop().unwrap().1.argv;
    let resume = argv.iter().position(|arg| arg == "--resume").unwrap();
    assert_eq!(argv[resume + 1], new);
}

#[gpui_kit::test]
fn a_turn_in_a_repository_is_snapshotted_before_the_agent_starts_and_the_files_can_go_back(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    let nothing = tempfile::tempdir().unwrap();
    sessions_in(&h, nothing.path());
    git_repo(&path);
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "first");
    // The agent was started, after the snapshot.
    assert_eq!(turns(&h).len(), 1);
    let (key, commit) = h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        (chat.key.clone(), chat.marks[0].checkpoint.clone())
    });
    let commit = commit.expect("the files were kept");
    assert_eq!(
        git(
            &path,
            &["rev-parse", &crate::chat::checkpoint::ref_name(&key, 1)]
        ),
        commit
    );
    let head = git(&path, &["rev-parse", "HEAD"]);
    let status = git(&path, &["status", "--porcelain"]);
    assert_eq!(status, "", "the snapshot left the worktree as it was");
    // The agent works, the turn ends, and a second one begins.
    std::fs::write(format!("{path}/a.txt"), "a1\n").unwrap();
    std::fs::write(format!("{path}/new.txt"), "new\n").unwrap();
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    say(&h, cx, 1, "second");
    std::fs::write(format!("{path}/a.txt"), "a2\n").unwrap();
    std::fs::write(format!("{path}/other.txt"), "other\n").unwrap();
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);

    // Back to before the second message, files too.
    from_here(&h, cx, "chat-edit", 2);
    assert!(h.shows("chat-rewind-files-1", cx));
    assert!(h.shows("chat-rewind-keep-1", cx));
    click(&h, cx, "chat-rewind-files-1");
    let read = |name: &str| std::fs::read_to_string(format!("{path}/{name}")).ok();
    assert_eq!(read("a.txt").as_deref(), Some("a1\n"));
    assert_eq!(read("new.txt").as_deref(), Some("new\n"));
    assert_eq!(read("other.txt"), None);
    let said = said_in(&h, cx, 1);
    assert!(
        said.last()
            .is_some_and(|line| line.starts_with("notice: Files put back")),
        "{said:?}"
    );
    // The branch and the index were not touched.
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&path, &["diff", "--cached"]), "");
    // The checkpoint of the turn that is gone is forgotten.
    assert_eq!(
        git(
            &path,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/leon/checkpoints"
            ]
        ),
        crate::chat::checkpoint::ref_name(&key, 1)
    );

    // Going back to the first message, but keeping the files.
    from_here(&h, cx, "chat-edit", 0);
    click(&h, cx, "chat-rewind-keep-1");
    assert_eq!(read("a.txt").as_deref(), Some("a1\n"));
    assert_eq!(field(&h, cx, 1), "first");

    // A closed conversation leaves no checkpoint behind.
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell
            .update(cx, |shell, cx| shell.close_chat(LiveId(1), window, cx));
    })
    .unwrap();
    h.settle(cx);
    assert_eq!(
        git(
            &path,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/leon/checkpoints"
            ]
        ),
        ""
    );
}

#[gpui_kit::test]
fn files_stay_where_another_conversation_works_in_the_same_folder(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    let nothing = tempfile::tempdir().unwrap();
    sessions_in(&h, nothing.path());
    git_repo(&path);
    start(&h, cx, AgentId::CLAUDE, &path);
    answered(&h, cx, 1, "first");
    std::fs::write(format!("{path}/a.txt"), "changed\n").unwrap();
    // Alone, the files can go back.
    from_here(&h, cx, "chat-edit", 0);
    assert!(h.shows("chat-rewind-files-1", cx));
    click(&h, cx, "chat-rewind-cancel-1");
    // With another conversation in the folder, only the conversation does.
    start(&h, cx, AgentId::CODEX, &path);
    h.shell(cx, |shell| assert_eq!(shell.chats.len(), 2));
    h.shell(cx, |shell| {
        assert_eq!(shell.focused_chat(), Some(LiveId(2)));
    });
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.chat_rewind_ask(LiveId(1), 0, false, cx);
        })
    });
    h.settle(cx);
    assert!(h.shell(cx, |s| s.chats[&LiveId(1)].rewinding.is_some()));
    assert_eq!(
        h.shell(cx, |shell| shell.chat_rewind_files(LiveId(1), 0)),
        crate::ui::chat_rewind::Files::Shared
    );
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            // Even asked for, the files are not touched.
            shell.chat_rewind(LiveId(1), true, window, cx);
        })
    })
    .unwrap();
    h.settle(cx);
    assert_eq!(
        std::fs::read_to_string(format!("{path}/a.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(field(&h, cx, 1), "first");
}

// ----- the changed files card, the diff panel and git from the foot -------------------

const EDIT_A: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"a.rs","old_string":"one","new_string":"two\nthree"}}]}}"#;
const WRITE_B: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"e2","name":"Write","input":{"file_path":"b.rs","content":"x\ny"}}]}}"#;

const RUN_FORMAT: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"r1","name":"Bash","input":{"command":"cargo fmt"}}]}}"#;

/// An edit of a file whose path is far longer than any card is wide.
fn edit_of_a_long_path() -> String {
    let path = format!(
        "graft/.cache/session/{}3b947915-d89e-4514-9ef8-1cc93002439a.json",
        "a-very-long-directory-name/".repeat(9)
    );
    format!(
        r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"e3","name":"Edit","input":{{"file_path":"{path}","old_string":"one","new_string":"two"}}}}]}}}}"#
    )
}

/// What `git status` says in the tests of the git cell: three changed files
/// on a branch with an upstream.
const STATUS: &str = "## trunk...origin/trunk\0 M a.rs\0A  b.rs\0?? c.txt\0";

/// A diff of `a.rs`, as git prints it.
const DIFF_OF_A: &str = "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1,2 @@\n-one\n+two\n+three\n";

/// A turn that edits two files and ends, in conversation 1.
fn edited(h: &Harness, cx: &mut TestAppContext, prompt: &str) {
    say(h, cx, 1, prompt);
    prints(h, cx, &[INIT, EDIT_A, WRITE_B, SAID, RESULT]);
    exits(h, cx, 0);
}

fn panel_scope(h: &Harness, cx: &mut TestAppContext) -> Option<crate::ui::chat_diff::Scope> {
    h.shell(cx, |shell| {
        shell.chats[&LiveId(1)]
            .diff
            .as_ref()
            .map(|panel| panel.scope())
    })
}

#[gpui_kit::test]
fn a_turn_that_changed_files_ends_with_a_card_that_opens_the_diffs_beside_it(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "rename it");
    prints(&h, cx, &[INIT, EDIT_A, WRITE_B]);
    assert!(
        !h.shows_dynamic("chat-changed-1-1".into(), cx),
        "the turn is not over"
    );
    prints(&h, cx, &[SAID, RESULT]);
    exits(&h, cx, 0);
    assert!(h.shows_dynamic("chat-changed-1-1".into(), cx));
    // From what the tools said: a.rs got 2 lines for 1, b.rs is new.
    let stats = h.shell(cx, |shell| shell.chats[&LiveId(1)].turn_stats(1));
    assert_eq!(
        crate::chat::changed::title(&stats),
        "2 files changed (+4 -1)"
    );
    assert!(h.shows_dynamic("chat-changed-file-1-1-0".into(), cx));
    assert!(h.shows_dynamic("chat-changed-file-1-1-1".into(), cx));
    assert!(!h.shows_dynamic("chat-diff-panel-1".into(), cx));

    // The card opens the panel on the turn, with its first file's diff.
    click(&h, cx, "chat-changed-open-1-1");
    assert!(h.shows_dynamic("chat-diff-panel-1".into(), cx));
    assert_eq!(panel_scope(&h, cx), Some(crate::ui::chat_diff::Scope::Turn));
    assert!(h.shows_dynamic("chat-diff-file-1-0".into(), cx));
    assert!(h.shows_dynamic("chat-diff-file-1-1".into(), cx));
    assert!(h.shows_dynamic("chat-diff-line-0".into(), cx), "a hunk");
    assert!(h.shows_dynamic("chat-diff-line-3".into(), cx), "its lines");
    // The other file's diff replaces it.
    click(&h, cx, "chat-diff-file-1-1");
    h.shell(cx, |shell| {
        let panel = shell.chats[&LiveId(1)].diff.as_ref().unwrap();
        assert_eq!(panel.selected.as_deref(), Some("b.rs"));
    });

    // Escape closes it, and the command opens it again.
    h.press("escape", cx);
    assert!(!h.shows_dynamic("chat-diff-panel-1".into(), cx));
    h.press_chord("cmd-alt-d", "ctrl-shift-alt-d", cx);
    assert!(h.shows_dynamic("chat-diff-panel-1".into(), cx));
    click(&h, cx, "chat-diff-close-1");
    assert!(!h.shows_dynamic("chat-diff-panel-1".into(), cx));
}

#[gpui_kit::test]
fn a_file_with_a_very_long_path_stays_on_one_line_inside_the_card(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "move the cache");
    let long = edit_of_a_long_path();
    prints(&h, cx, &[INIT, &long, WRITE_B, SAID, RESULT]);
    exits(&h, cx, 0);
    let card = h
        .bounds_of("chat-changed-1-1".to_owned(), cx)
        .expect("the card");
    let long_row = h
        .bounds_of("chat-changed-file-1-1-0".to_owned(), cx)
        .expect("the long path's row");
    let short_row = h
        .bounds_of("chat-changed-file-1-1-1".to_owned(), cx)
        .expect("the short path's row");
    let within = |inner: gpui_kit::Bounds<gpui_kit::Pixels>| {
        inner.left() >= card.left()
            && inner.right() <= card.right()
            && inner.top() >= card.top()
            && inner.bottom() <= card.bottom()
    };
    assert!(within(long_row), "{long_row:?} is not inside {card:?}");
    assert!(within(short_row), "{short_row:?} is not inside {card:?}");
    assert_eq!(
        long_row.size.height, short_row.size.height,
        "the long path wrapped onto a second line"
    );
    let text = h
        .bounds_of("chat-changed-path-1-1-0".to_owned(), cx)
        .expect("the path");
    assert!(
        text.left() >= long_row.left() && text.right() <= long_row.right(),
        "{text:?} runs out of {long_row:?}"
    );
    assert!(
        text.size.height <= long_row.size.height,
        "{text:?} is taller than {long_row:?}"
    );
    let name = h
        .bounds_of("chat-changed-name-1-1-0".to_owned(), cx)
        .expect("the file name is drawn");
    assert!(
        name.right() <= long_row.right(),
        "the name is cut off: {name:?}"
    );
}

#[gpui_kit::test]
fn the_panel_shows_a_single_file_or_everything_uncommitted(cx: &mut TestAppContext) {
    use crate::ui::chat_diff::Scope;
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    edited(&h, cx, "rename it");
    // A file of the card opens the panel on that file alone.
    click(&h, cx, "chat-changed-file-1-1-1");
    assert_eq!(panel_scope(&h, cx), Some(Scope::File));
    assert!(!h.shows_dynamic("chat-diff-file-1-0".into(), cx), "no list");
    h.shell(cx, |shell| {
        let panel = shell.chats[&LiveId(1)].diff.as_ref().unwrap();
        assert_eq!(panel.selected.as_deref(), Some("b.rs"));
    });
    // The turn's files again.
    click(&h, cx, "chat-diff-scope-turn-1");
    assert_eq!(panel_scope(&h, cx), Some(Scope::Turn));
    assert!(h.shows_dynamic("chat-diff-file-1-0".into(), cx));
    // Everything uncommitted is the engine's reading of the worktree, as the
    // changes tab does it: the status, then the diff of its first file.
    h.runner.queue(leon_remote::Output::ok(STATUS));
    h.runner.queue(leon_remote::Output::ok(DIFF_OF_A));
    click(&h, cx, "chat-diff-scope-all-1");
    h.settle(cx);
    assert_eq!(panel_scope(&h, cx), Some(Scope::All));
    h.shell(cx, |shell| {
        let panel = shell.chats[&LiveId(1)].diff.as_ref().unwrap();
        let files: Vec<&str> = panel
            .all
            .iter()
            .flatten()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(files, ["a.rs", "b.rs", "c.txt"]);
        assert_eq!(panel.selected.as_deref(), Some("b.rs"), "the file stays");
    });
    assert!(h.shows_dynamic("chat-diff-file-1-2".into(), cx));
    assert!(h.shows_dynamic("chat-diff-line-0".into(), cx));
    let ran: Vec<_> = h
        .runner
        .calls()
        .iter()
        .filter(|call| call.program == "git" && call.args.contains(&"HEAD".to_owned()))
        .map(|call| call.args.last().cloned())
        .collect();
    assert_eq!(ran, [Some("b.rs".to_owned())], "diffed against HEAD");
    // The single file of that scope.
    click(&h, cx, "chat-diff-scope-file-1");
    assert_eq!(panel_scope(&h, cx), Some(Scope::File));
}

#[gpui_kit::test]
fn what_a_command_changed_in_a_repository_is_counted_from_its_snapshots(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    let nothing = tempfile::tempdir().unwrap();
    sessions_in(&h, nothing.path());
    git_repo(&path);
    start(&h, cx, AgentId::CLAUDE, &path);
    // The agent's tools say nothing of the files: a shell command did it all.
    say(&h, cx, 1, "format everything");
    std::fs::write(format!("{path}/a.txt"), "a0\nmore\n").unwrap();
    std::fs::write(format!("{path}/made.txt"), "1\n2\n3\n").unwrap();
    prints(&h, cx, &[INIT, RUN_FORMAT, SAID, RESULT]);
    exits(&h, cx, 0);
    wait_until(&h, cx, "the count of turn 1", |h, cx| {
        h.shell(cx, |shell| {
            shell.chats[&LiveId(1)].measured.contains_key(&1)
        })
    });
    let stats = h.shell(cx, |shell| shell.chats[&LiveId(1)].turn_stats(1));
    assert_eq!(
        crate::chat::changed::title(&stats),
        "2 files changed (+4 -0)"
    );
    assert!(h.shows_dynamic("chat-changed-1-1".into(), cx));
    // The panel reads a file's patch between the snapshots.
    click(&h, cx, "chat-changed-open-1-1");
    wait_until(&h, cx, "the patch", |h, cx| {
        h.shell(cx, |shell| {
            matches!(
                shell.chats[&LiveId(1)]
                    .diff
                    .as_ref()
                    .map(|panel| &panel.diff),
                Some(crate::ui::changes::DiffState::Ready(_))
            )
        })
    });
    assert!(h.shows_dynamic("chat-diff-line-0".into(), cx));
    h.shell(cx, |shell| {
        let panel = shell.chats[&LiveId(1)].diff.as_ref().unwrap();
        assert_eq!(panel.selected.as_deref(), Some("a.txt"));
        let crate::ui::changes::DiffState::Ready(leon_remote::FileDiff::Lines(lines)) = &panel.diff
        else {
            panic!("a patch was read");
        };
        assert!(lines.iter().any(|line| line.text == "more"), "{lines:?}");
    });
    // A turn that changed nothing has no card.
    say(&h, cx, 1, "just look");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    wait_until(&h, cx, "the count of turn 2", |h, cx| {
        h.shell(cx, |shell| {
            shell.chats[&LiveId(1)].measured.contains_key(&2)
        })
    });
    assert_eq!(
        h.shell(cx, |shell| shell.chats[&LiveId(1)].turn_stats(2)),
        []
    );
    assert!(!h.shows_dynamic("chat-changed-1-2".into(), cx));
}

#[gpui_kit::test]
fn a_file_a_background_process_wrote_during_a_turn_that_only_talked_is_no_card(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    let nothing = tempfile::tempdir().unwrap();
    sessions_in(&h, nothing.path());
    git_repo(&path);
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "hola");
    // A hook or an indexer writes while the agent only answers.
    std::fs::write(format!("{path}/indexed.json"), "1\n2\n3\n").unwrap();
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    wait_until(&h, cx, "the count of turn 1", |h, cx| {
        h.shell(cx, |shell| {
            shell.chats[&LiveId(1)].measured.contains_key(&1)
        })
    });
    assert_eq!(
        h.shell(cx, |shell| shell.chats[&LiveId(1)].turn_stats(1)),
        []
    );
    assert!(!h.shows_dynamic("chat-changed-1-1".into(), cx));
}

#[gpui_kit::test]
fn the_foot_says_the_branch_and_how_far_it_is_from_its_upstream(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    assert!(h.shows_dynamic("chat-branch-1".into(), cx));
    let worktree = h
        .store
        .all_worktrees()
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.path == path)
        .expect("the worktree of the test");
    h.store
        .update_worktree_status(&worktree.id, |status| {
            status.divergence = Some(leon_core::Divergence {
                ahead: 2,
                behind: 1,
            });
        })
        .unwrap();
    h.settle(cx);
    let said = h.shell(cx, |shell| {
        let worktree = shell
            .snapshot
            .worktree_at(&MachineId::local(), &path)
            .unwrap();
        let divergence = shell
            .snapshot
            .status(&worktree.id)
            .and_then(|s| s.divergence);
        crate::ui::chat_git::branch_words(worktree.branch.as_deref().unwrap(), divergence)
    });
    assert_eq!(said, "trunk  \u{2191}2 \u{2193}1");
    // A folder no project knows has no branch to say, and no git to do.
    let elsewhere = tempfile::tempdir().unwrap();
    let folder = elsewhere.path().canonicalize().unwrap();
    assert!(start(&h, cx, AgentId::CODEX, &folder.to_string_lossy()));
    assert!(!h.shows_dynamic("chat-branch-2".into(), cx));
    click(&h, cx, "chat-git-2");
    assert!(!h.shows_dynamic("chat-git-cell-2".into(), cx));
}

/// Opens the cell of conversation 1 with what git says of the worktree.
fn open_git_cell(h: &Harness, cx: &mut TestAppContext, status: &str) {
    h.runner.queue(leon_remote::Output::ok(status.to_owned()));
    click(h, cx, "chat-git-1");
    h.settle(cx);
}

fn git_calls(h: &Harness) -> Vec<String> {
    h.runner
        .calls()
        .iter()
        .filter(|call| call.program == "git" || call.program == "gh")
        .map(|call| format!("{} {}", call.program, call.args.join(" ")))
        .collect()
}

fn write_message(h: &Harness, cx: &mut TestAppContext, text: &str) {
    let field = h.shell(cx, |shell| shell.chats[&LiveId(1)].git.message.clone());
    cx.update_window(h.window.into(), |_, window, cx| {
        field.update(cx, |field, cx| field.set_value(text.to_owned(), window, cx));
    })
    .unwrap();
    h.settle(cx);
}

#[gpui_kit::test]
fn the_git_cell_commits_and_pushes_with_the_same_commands_as_the_changes_tab(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    assert!(!h.shows_dynamic("chat-git-cell-1".into(), cx));
    open_git_cell(&h, cx, STATUS);
    assert!(h.shows_dynamic("chat-git-cell-1".into(), cx));
    h.shell(cx, |shell| {
        let git = &shell.chats[&LiveId(1)].git;
        assert_eq!(git.files.as_ref().map(Vec::len), Some(3));
        assert_eq!(git.branch.as_deref(), Some("trunk"));
    });
    let before = git_calls(&h).len();
    // Without a message there is nothing to commit with.
    click(&h, cx, "chat-git-commit-1");
    assert_eq!(git_calls(&h).len(), before, "nothing ran");
    h.shell(cx, |shell| {
        let outcome = shell.chats[&LiveId(1)].git.outcome.as_ref().unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.title, "Write a commit message first.");
    });
    // With one: `git add -A` and `git commit`, then push, in that order.
    write_message(&h, cx, "Fix the tabs");
    h.runner.queue(leon_remote::Output::ok(""));
    h.runner
        .queue(leon_remote::Output::ok("[trunk 1a2b3c4] Fix the tabs\n"));
    h.runner
        .queue(leon_remote::Output::ok("## trunk...origin/trunk\0"));
    h.runner.queue(leon_remote::Output::ok(""));
    click(&h, cx, "chat-git-commit-push-1");
    h.settle(cx);
    let ran = git_calls(&h)[before..].to_vec();
    assert_eq!(ran[0], "git --literal-pathspecs add -A");
    assert_eq!(ran[1], "git --literal-pathspecs commit -F -");
    assert_eq!(
        h.runner.calls()[before + 1].stdin.as_deref(),
        Some(b"Fix the tabs\n".as_slice()),
        "the message goes in on stdin, as the tab sends it"
    );
    assert!(ran[2].contains("status"), "{ran:?}");
    assert_eq!(ran[3], "git push");
    h.shell(cx, |shell| {
        let git = &shell.chats[&LiveId(1)].git;
        let outcome = git.outcome.as_ref().unwrap();
        assert!(outcome.ok, "{}: {}", outcome.title, outcome.text);
        assert_eq!(outcome.title, "Committed and pushed");
        assert!(git.working.is_none());
    });
    assert_eq!(
        cx.update(|cx| h.shell.read(cx).chats[&LiveId(1)]
            .git
            .message
            .read(cx)
            .value()
            .to_string()),
        "",
        "the message was used"
    );
}

#[gpui_kit::test]
fn the_git_cell_pulls_with_a_fast_forward_only_and_says_why_when_it_cannot(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    open_git_cell(&h, cx, "## trunk...origin/trunk [behind 2]\0");
    assert!(h.shows("chat-git-pull-1", cx), "the cell has a PULL button");
    let before = git_calls(&h).len();
    h.runner.queue(leon_remote::Output::ok(
        "## trunk...origin/trunk [behind 2]\0",
    ));
    h.runner.queue(leon_remote::Output::ok(
        "Updating 1a2b3c4..5d6e7f8\nFast-forward\n",
    ));
    h.runner
        .queue(leon_remote::Output::ok("## trunk...origin/trunk\0"));
    h.runner
        .queue(leon_remote::Output::ok("## trunk...origin/trunk\0"));
    click(&h, cx, "chat-git-pull-1");
    h.settle(cx);
    let ran = git_calls(&h)[before..].to_vec();
    assert!(ran[0].contains("status"), "{ran:?}");
    assert_eq!(ran[1], "git pull --ff-only", "never a merge or a rebase");
    assert!(
        !ran.iter()
            .any(|call| call.contains("commit") || call.contains("push")),
        "{ran:?}"
    );
    h.shell(cx, |shell| {
        let git = &shell.chats[&LiveId(1)].git;
        let outcome = git.outcome.as_ref().unwrap();
        assert!(outcome.ok, "{}: {}", outcome.title, outcome.text);
        assert_eq!(outcome.title, "Pulled");
        assert!(outcome.text.contains("Fast-forward"));
        assert!(git.working.is_none());
    });

    // A branch that diverged is refused by git, and what it said is shown.
    let before = git_calls(&h).len();
    h.runner.queue(leon_remote::Output::ok(
        "## trunk...origin/trunk [ahead 1, behind 2]\0",
    ));
    h.runner.queue(leon_remote::Output::failed(
        128,
        "fatal: Not possible to fast-forward, aborting.\n",
    ));
    click(&h, cx, "chat-git-pull-1");
    h.settle(cx);
    assert_eq!(git_calls(&h)[before + 1], "git pull --ff-only");
    h.shell(cx, |shell| {
        let outcome = shell.chats[&LiveId(1)].git.outcome.as_ref().unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.title, "The pull failed");
        assert!(outcome.text.contains("fast-forward"), "{}", outcome.text);
    });
}

#[gpui_kit::test]
fn the_git_cell_offers_nothing_to_commit_in_a_clean_tree(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    open_git_cell(&h, cx, "## trunk...origin/trunk\0");
    write_message(&h, cx, "Nothing here");
    let before = git_calls(&h).len();
    click(&h, cx, "chat-git-commit-1");
    click(&h, cx, "chat-git-commit-push-1");
    assert_eq!(git_calls(&h).len(), before, "no commit was asked for");
    // A branch that is committed can still be pushed.
    h.runner.queue(leon_remote::Output::ok(
        "## trunk...origin/trunk [ahead 1]\0",
    ));
    h.runner.queue(leon_remote::Output::ok(""));
    click(&h, cx, "chat-git-push-1");
    h.settle(cx);
    let ran = git_calls(&h)[before..].to_vec();
    assert!(ran.iter().any(|call| call == "git push"), "{ran:?}");
    assert!(
        !ran.iter().any(|call| call.starts_with("git commit")),
        "{ran:?}"
    );
    // Escape closes the cell.
    h.press("escape", cx);
    assert!(!h.shows_dynamic("chat-git-cell-1".into(), cx));
}

#[gpui_kit::test]
fn opening_a_pull_request_from_the_cell_shows_it_in_the_chip_of_the_foot(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    open_git_cell(&h, cx, STATUS);
    assert!(!h.shows_dynamic("chat-pr-1".into(), cx));
    write_message(&h, cx, "Fix the tabs\n\nThey leaked.");
    let open = r#"[{"number":41,"url":"https://github.com/zavudev/leon/pull/41","headRefName":"trunk","isDraft":false,"reviewDecision":"","statusCheckRollup":[]}]"#;
    let before = git_calls(&h).len();
    // The start of the pull request: the branch, its base, its commits.
    h.runner
        .queue(leon_remote::Output::ok("## trunk...origin/trunk\0"));
    h.runner.queue(leon_remote::Output::ok("origin/main\n"));
    h.runner.queue(leon_remote::Output::ok(""));
    // add, commit, the branch and push, and gh pr create.
    h.runner.queue(leon_remote::Output::ok(""));
    h.runner
        .queue(leon_remote::Output::ok("[trunk 1a2b3c4] Fix the tabs\n"));
    h.runner
        .queue(leon_remote::Output::ok("## trunk...origin/trunk\0"));
    h.runner.queue(leon_remote::Output::ok(""));
    h.runner.queue(leon_remote::Output::ok(
        "https://github.com/zavudev/leon/pull/41\n",
    ));
    // The engine reads the checkout again, then GitHub for the pull request.
    h.runner
        .queue(leon_remote::Output::ok("## trunk...origin/trunk\n"));
    h.runner
        .queue(leon_remote::Output::ok("git@github.com:zavudev/leon.git\n"));
    h.runner.queue(leon_remote::Output::ok(open));
    // The cell reads the files again.
    h.runner
        .queue(leon_remote::Output::ok("## trunk...origin/trunk\0"));
    click(&h, cx, "chat-git-ship-1");
    h.settle(cx);
    let ran = git_calls(&h)[before..].to_vec();
    let pr = ran
        .iter()
        .find(|call| call.starts_with("gh pr create"))
        .unwrap_or_else(|| panic!("{ran:?}"));
    assert!(pr.contains("Fix the tabs") && pr.contains("main"), "{pr}");
    h.shell(cx, |shell| {
        let outcome = shell.chats[&LiveId(1)].git.outcome.as_ref().unwrap();
        assert!(outcome.ok, "{}: {}", outcome.title, outcome.text);
        assert_eq!(
            outcome.url.as_deref(),
            Some("https://github.com/zavudev/leon/pull/41")
        );
    });
    assert!(
        h.shows_dynamic("chat-pr-1".into(), cx),
        "the chip refreshed"
    );
}

const TODOS: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"w1","name":"TodoWrite","input":{"todos":[{"content":"Read the code","status":"completed","activeForm":"Reading"},{"content":"Write the test","status":"in_progress","activeForm":"Writing"},{"content":"Ship it","status":"pending","activeForm":"Shipping"}]}}]}}"#;
const TODOS_LATER: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"w2","name":"TodoWrite","input":{"todos":[{"content":"Read the code","status":"completed","activeForm":"Reading"},{"content":"Write the test","status":"completed","activeForm":"Writing"},{"content":"Ship it","status":"in_progress","activeForm":"Shipping"},{"content":"Celebrate","status":"pending","activeForm":"Celebrating"}]}}]}}"#;
const TODOS_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"w1","content":"Todos have been modified successfully"}]}}"#;

#[gpui_kit::test]
fn the_agents_todos_are_one_card_that_the_next_list_replaces_and_not_a_tool_row(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "plan it");
    prints(&h, cx, &[INIT, TODOS, TODOS_RESULT]);
    let titles = |h: &Harness, cx: &mut TestAppContext| {
        h.shell(cx, |shell| {
            shell.chats[&LiveId(1)]
                .conversation
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Todos(todos) => Some(crate::chat::model::todo_title(todos)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(titles(&h, cx), ["Todos 1/3"]);
    assert!(h.shows_dynamic("chat-todos-1-1".into(), cx));
    assert!(h.shows_dynamic("chat-todos-title-1-1".into(), cx));
    for row in 0..3 {
        assert!(h.shows_dynamic(format!("chat-todo-1-1-{row}"), cx), "{row}");
    }
    // It is not a tool: there is no folded line of work for it.
    assert!(!h.shows_dynamic("chat-work-1-1".into(), cx));
    h.shell(cx, |shell| {
        let items = &shell.chats[&LiveId(1)].conversation.items;
        assert!(!items.iter().any(|item| matches!(item, Item::Tool(_))));
    });

    // The next list is the same card, longer.
    prints(&h, cx, &[TODOS_LATER]);
    assert_eq!(titles(&h, cx), ["Todos 2/4"]);
    assert!(h.shows_dynamic("chat-todo-1-1-3".into(), cx));
}

const CODE: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Run:\n\n```sh\ncargo test\n```\n"}]}}"#;

#[gpui_kit::test]
fn a_code_block_says_its_language_and_its_button_copies_the_code(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "how do I test");
    prints(&h, cx, &[INIT, CODE, RESULT]);
    assert!(h.shows_dynamic("chat-code-1-1".into(), cx));
    assert!(h.shows_dynamic("chat-code-1-1-lang".into(), cx));
    assert!(h.shows_dynamic("chat-code-1-1-copy".into(), cx));
    click(&h, cx, "chat-code-1-1-copy");
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref().map(str::trim), Some("cargo test"));
}

const PLAN_QUESTION: &str = r#"{"type":"control_request","request_id":"p1","request":{"subtype":"can_use_tool","tool_name":"ExitPlanMode","display_name":"ExitPlanMode","input":{"plan":"1. Create hello.txt\n2. Verify it\n"},"tool_use_id":"toolu_p","requires_user_interaction":true}}"#;

#[gpui_kit::test]
fn a_proposed_plan_is_a_card_of_its_own_with_its_actions_and_a_copy_button(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "plan it");
    prints(&h, cx, &[INIT, PLAN_QUESTION]);
    assert!(h.shows_dynamic("chat-plan-card-1".into(), cx));
    assert!(h.shows_dynamic("chat-plan-page-1".into(), cx));
    assert!(h.shows("chat-plan-start-1", cx));
    assert!(h.shows("chat-plan-keep-1", cx));
    click(&h, cx, "chat-plan-copy-1");
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(
        copied.as_deref(),
        Some("1. Create hello.txt\n2. Verify it\n")
    );
}

const USAGE: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":49990,"output_tokens":0}}}"#;
const RESULT_WINDOW: &str = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s1","total_cost_usd":0.02,"duration_ms":1500,"usage":{"input_tokens":10,"cache_read_input_tokens":49990,"output_tokens":5},"modelUsage":{"claude-test":{"contextWindow":200000}}}"#;

#[gpui_kit::test]
fn the_foot_shows_how_full_the_context_is_and_nothing_when_the_window_is_not_known(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "hello");
    prints(&h, cx, &[INIT, USAGE, RESULT]);
    exits(&h, cx, 0);
    // The agent said what fills the context but not how large it is.
    assert!(!h.shows("chat-context-1", cx));

    say(&h, cx, 1, "again");
    prints(&h, cx, &[INIT, USAGE, RESULT_WINDOW]);
    exits(&h, cx, 0);
    assert!(h.shows("chat-context-1", cx));
    assert!(h.shows("chat-context-bar-1", cx));
    h.shell(cx, |shell| {
        let meter = shell.chats[&LiveId(1)].conversation.context.unwrap();
        assert_eq!(meter.label(), "50k / 200k");
        assert_eq!(meter.percent(), 25);
    });
    // It stays while the next turn begins, until that one says its own.
    say(&h, cx, 1, "and again");
    assert!(h.shows("chat-context-1", cx));
}

#[gpui_kit::test]
fn the_find_bar_marks_the_messages_that_match_and_goes_from_one_to_the_next(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "fix the parser");
    prints(
        &h,
        cx,
        &[
            INIT,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"The Parser is fixed, and the parser tests pass."}]}}"#,
            RESULT,
        ],
    );
    exits(&h, cx, 0);
    assert!(!h.shows("chat-find-1", cx));

    // The chord opens the bar with the keyboard in its field.
    h.press_chord("cmd-f", "ctrl-f", cx);
    assert!(h.shows("chat-find-1", cx));
    h.type_text("parser", cx);
    let state = |h: &Harness, cx: &mut TestAppContext| {
        h.shell(cx, |shell| {
            let chat = &shell.chats[&LiveId(1)];
            (
                chat.found().len(),
                chat.find.as_ref().map(|bar| bar.current),
            )
        })
    };
    // One in what was said, two in the answer.
    assert_eq!(state(&h, cx), (3, Some(0)));
    assert!(h.shows("chat-find-count-1", cx));

    // Enter goes to the next, Shift+Enter to the one before, round the ends.
    h.press("enter", cx);
    assert_eq!(state(&h, cx).1, Some(1));
    h.press("enter", cx);
    h.press("enter", cx);
    assert_eq!(state(&h, cx).1, Some(0));
    h.press("shift-enter", cx);
    assert_eq!(state(&h, cx).1, Some(2));

    // Escape closes it and nothing was sent.
    h.press("escape", cx);
    assert!(!h.shows("chat-find-1", cx));
    assert!(h.shell(cx, |shell| shell.chats[&LiveId(1)].find.is_none()));
    assert_eq!(turns(&h).len(), 1);
}

#[gpui_kit::test]
fn the_chord_to_find_in_a_conversation_leaves_the_sidebar_filter_alone(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    // With the sidebar in the keyboard the same chord still filters.
    h.press_chord("cmd-j", "ctrl-j", cx);
    h.press_chord("cmd-l", "ctrl-l", cx);
    h.press_chord("cmd-f", "ctrl-f", cx);
    assert!(!h.shows("chat-find-1", cx));
}

/// Sets how the conversations of the window answer what is said while the
/// agent works.
fn when_busy(h: &Harness, cx: &mut TestAppContext, how: &str) {
    cx.update(|cx| {
        settings::set_value(
            cx,
            schema::find("chat_busy").unwrap(),
            Value::Text(how.into()),
        )
    });
    h.settle(cx);
}

fn queued(h: &Harness, cx: &mut TestAppContext, id: u64) -> Vec<String> {
    h.shell(cx, |shell| {
        shell.chats[&LiveId(id)]
            .conversation
            .queued
            .iter()
            .cloned()
            .collect()
    })
}

#[gpui_kit::test]
fn what_is_said_while_the_agent_works_steers_it_by_default_and_queues_when_asked(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    assert_eq!(
        cx.update(|cx| settings::text(cx, "chat_busy")),
        "steer",
        "as it has always been"
    );
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "one");
    say(&h, cx, 1, "and also two");
    assert!(queued(&h, cx, 1).is_empty());
    assert!(written(&h).iter().any(|line| line.contains("and also two")));

    // Asked to queue, the next thing waits for the turn to end and then
    // becomes the turn after it: the agent is told nothing meanwhile.
    when_busy(&h, cx, "queue");
    say(&h, cx, 1, "then three");
    assert_eq!(queued(&h, cx, 1), ["then three"]);
    assert!(!written(&h).iter().any(|line| line.contains("then three")));
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    assert_eq!(turns(&h).len(), 2, "the queued message became a turn");
    assert!(queued(&h, cx, 1).is_empty());
    assert!(written(&h).iter().any(|line| line.contains("then three")));
}

#[gpui_kit::test]
fn the_queue_is_listed_above_the_field_and_each_message_can_be_edited_removed_or_sent_now(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    when_busy(&h, cx, "queue");
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "one");
    say(&h, cx, 1, "two");
    say(&h, cx, 1, "three");
    say(&h, cx, 1, "four");
    assert_eq!(queued(&h, cx, 1), ["two", "three", "four"]);
    for index in 0..3 {
        for part in ["item", "edit", "remove", "steer"] {
            assert!(
                h.shows_dynamic(format!("chat-queue-{part}-1-{index}"), cx),
                "{part} {index}"
            );
        }
    }

    // Removed: it is gone, and nothing was said to the agent.
    click(&h, cx, "chat-queue-remove-1-1");
    assert_eq!(queued(&h, cx, 1), ["two", "four"]);
    assert!(!h.shows_dynamic("chat-queue-item-1-2".into(), cx));

    // Edited: it goes back to the field, to be changed and sent again.
    click(&h, cx, "chat-queue-edit-1-1");
    assert_eq!(queued(&h, cx, 1), ["two"]);
    assert_eq!(field(&h, cx, 1), "four");

    // Sent now: the running turn is told, and it is no longer waiting.
    click(&h, cx, "chat-queue-steer-1-0");
    assert!(queued(&h, cx, 1).is_empty());
    assert!(written(&h).iter().any(|line| line.contains("two")));
    h.shell(cx, |shell| {
        let items = &shell.chats[&LiveId(1)].conversation.items;
        assert!(matches!(
            items.last(),
            Some(Item::User { text, .. }) if text == "two"
        ));
    });
    assert!(!h.shows("chat-queued-1", cx), "nothing waits any more");
}

#[gpui_kit::test]
fn an_agent_that_cannot_be_steered_keeps_the_message_queued_and_says_so(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::OPENCODE, &path);
    say(&h, cx, 1, "one");
    // Steering is asked for, and opencode takes nothing more while it works.
    say(&h, cx, 1, "two");
    assert_eq!(queued(&h, cx, 1), ["two"]);
    click(&h, cx, "chat-queue-steer-1-0");
    assert_eq!(queued(&h, cx, 1), ["two"], "still waiting its turn");
    assert!(h.status().contains("cannot be told more"), "{}", h.status());
}

#[gpui_kit::test]
fn the_chord_steers_the_first_queued_message(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    when_busy(&h, cx, "queue");
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "one");
    say(&h, cx, 1, "two");
    say(&h, cx, 1, "three");
    h.press_chord("cmd-enter", "ctrl-enter", cx);
    assert_eq!(queued(&h, cx, 1), ["three"], "the first one went");
    assert!(written(&h).iter().any(|line| line.contains("two")));
}

#[gpui_kit::test]
fn an_attached_image_is_a_thumbnail_that_opens_and_any_other_file_stays_a_chip(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    let shot = std::path::PathBuf::from(&path).join("shot.png");
    let notes = std::path::PathBuf::from(&path).join("notes.md");
    *h.picked_files.borrow_mut() = vec![shot.clone(), notes.clone()];
    click(&h, cx, "chat-attach-1");
    h.settle(cx);
    // Over the field: the image has a thumbnail to click, the note does not.
    assert!(h.shows_dynamic("chat-attached-1-0".into(), cx));
    assert!(h.shows_dynamic("chat-attached-1-preview-0".into(), cx));
    assert!(!h.shows_dynamic("chat-attached-1-preview-1".into(), cx));
    click(&h, cx, "chat-attached-1-preview-0");
    assert_eq!(
        *h.opened.borrow(),
        std::slice::from_ref(&shot),
        "the system's viewer"
    );

    // Sent, the message shows the same: a thumbnail of the image, a chip
    // for the note, and the words without the list of paths the agent reads.
    h.type_text("read these", cx);
    h.press("enter", cx);
    assert!(h.shows_dynamic("chat-sent-image-1-0-0".into(), cx));
    assert!(!h.shows_dynamic("chat-sent-image-1-0-1".into(), cx));
    assert!(h.shows_dynamic("chat-sent-file-1-0-1".into(), cx));
    h.opened.borrow_mut().clear();
    click(&h, cx, "chat-sent-image-1-0-0");
    click(&h, cx, "chat-sent-file-1-0-1");
    assert_eq!(*h.opened.borrow(), [shot, notes]);
    assert!(
        written(&h).join("\n").contains("Attached (open each path)"),
        "the agent still reads the paths"
    );
}

/// What the generator of titles answers next, and what it was asked so far.
fn titler_says(h: &Harness, answer: Result<&str, &str>) {
    *h.title_says.borrow_mut() = answer.map(str::to_owned).map_err(str::to_owned);
}

fn first_turn(h: &Harness, cx: &mut TestAppContext, prompt: &str) {
    say(h, cx, 1, prompt);
    prints(h, cx, &[INIT, SAID, RESULT]);
    exits(h, cx, 0);
}

#[gpui_kit::test]
fn after_its_first_turn_a_conversation_is_titled_and_the_title_is_its_label(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    titler_says(&h, Ok("Say hello to the world"));
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "say hello");
    assert!(
        h.title_asked.borrow().is_empty(),
        "not before the turn ends"
    );
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    let asked = h.title_asked.borrow().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].agent, AgentId::CLAUDE);
    assert_eq!(asked[0].prompt, "say hello");
    assert_eq!(asked[0].reply, "Hello **there**.");
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].label(), "Say hello to the world");
    });
    assert_eq!(
        cx.update(|cx| h.shell.read(cx).live_row(LiveId(1), cx).0),
        "Say hello to the world",
        "the sidebar's row and the tab say it"
    );
    // A second turn does not ask again.
    say(&h, cx, 1, "and again");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    assert_eq!(h.title_asked.borrow().len(), 1);
}

#[gpui_kit::test]
fn a_title_that_cannot_be_made_changes_nothing_and_says_nothing(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    titler_says(&h, Err("claude is not installed"));
    start(&h, cx, AgentId::CLAUDE, &path);
    let status = h.status();
    first_turn(&h, cx, "say hello");
    assert_eq!(h.title_asked.borrow().len(), 1);
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].label(), "say hello");
    });
    assert_eq!(h.status(), status, "failure is silent");
}

#[gpui_kit::test]
fn the_setting_turns_titles_off_and_a_name_given_by_hand_wins(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    titler_says(&h, Ok("A generated title"));
    cx.update(|cx| {
        settings::set_value(cx, schema::find("chat_titles").unwrap(), Value::Bool(false))
    });
    start(&h, cx, AgentId::CLAUDE, &path);
    first_turn(&h, cx, "say hello");
    assert!(h.title_asked.borrow().is_empty(), "off: nothing is asked");

    // On again, but the person named the conversation first.
    cx.update(|cx| {
        settings::set_value(cx, schema::find("chat_titles").unwrap(), Value::Bool(true))
    });
    cx.update(|cx| {
        h.shell
            .update(cx, |shell, cx| shell.rename_live(LiveId(1), "Mine", cx))
    });
    say(&h, cx, 1, "again");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    assert!(
        h.title_asked.borrow().is_empty(),
        "named by hand: not asked"
    );
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].label(), "Mine");
    });
}

#[gpui_kit::test]
fn the_title_of_a_conversation_is_kept_on_the_session_its_agent_wrote(cx: &mut TestAppContext) {
    const INIT_HELLO: &str =
        r#"{"type":"system","subtype":"init","session_id":"hello-3","model":"claude-test"}"#;
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    titler_says(&h, Ok("Say hello to the world"));
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "say hello");
    prints(&h, cx, &[INIT_HELLO, SAID, RESULT]);
    exits(&h, cx, 0);
    let session =
        super::live::stored_session(&h, cx, AgentId::CLAUDE, &MachineId::local(), &path, "hello");
    h.settle(cx);
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].history.as_ref(), Some(&session));
    });
    let stored = h
        .engine
        .store()
        .session(&session)
        .map(|stored| stored.title)
        .unwrap();
    assert_eq!(
        stored, "Say hello to the world",
        "the session's row says it too"
    );
}

// ---- Notifications ---------------------------------------------------

const FAILED: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"s1","queued_turn_count":0,"result":"the build broke"}"#;

/// The window has the focus, as a person looking at it would have it.
fn in_front(h: &Harness, cx: &mut TestAppContext) {
    VisualTestContext::from_window(h.window.into(), cx)
        .update(|window, _| window.activate_window());
    h.settle(cx);
}

fn desktop_notes(h: &Harness) -> Vec<crate::ui::notify::Note> {
    h.notes.borrow().clone()
}

fn setting(h: &Harness, cx: &mut TestAppContext, key: &str, value: Value) {
    cx.update(|cx| settings::set_value(cx, schema::find(key).unwrap(), value));
    h.settle(cx);
}

#[gpui_kit::test]
fn a_turn_that_ends_while_the_window_is_in_the_background_is_said_on_the_desktop_and_in_the_window(
    cx: &mut TestAppContext,
) {
    use crate::ui::notify::Kind;
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "say hello");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    let notes = desktop_notes(&h);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].kind, Kind::TurnOver);
    assert_eq!(notes[0].session, Some(LiveId(1)));
    assert!(
        notes[0].title.starts_with("say hello"),
        "{}",
        notes[0].title
    );
    assert!(notes[0].body.contains("finished"), "{}", notes[0].body);
    assert_eq!(
        h.shell(cx, |shell| shell.banners.len()),
        1,
        "the banner of the window too"
    );
    // The row of the conversation carries the dot until it is looked at.
    exits(&h, cx, 0);
    assert_eq!(
        h.shell(cx, |shell| shell.chats[&LiveId(1)].activity()),
        Activity::TurnOver
    );
}

#[gpui_kit::test]
fn a_failed_turn_and_a_question_are_said_too_each_in_its_own_words(cx: &mut TestAppContext) {
    use crate::ui::notify::Kind;
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "build it");
    prints(&h, cx, &[INIT, ASK]);
    let notes = desktop_notes(&h);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].kind, Kind::NeedsAnswer);
    assert!(notes[0].body.contains("asks"), "{}", notes[0].body);
    prints(&h, cx, &[FAILED]);
    let notes = desktop_notes(&h);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!(notes[1].kind, Kind::Failed);
    assert!(
        notes[1].body.contains("the build broke"),
        "{}",
        notes[1].body
    );
}

#[gpui_kit::test]
fn nothing_is_said_of_the_conversation_the_person_is_looking_at(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    in_front(&h, cx);
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "say hello");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    assert!(desktop_notes(&h).is_empty());
    assert!(h.shell(cx, |shell| shell.banners.is_empty()));
    assert_eq!(
        h.shell(cx, |shell| shell.chats[&LiveId(1)].activity()),
        Activity::Idle
    );
}

#[gpui_kit::test]
fn a_conversation_in_another_tab_is_said_even_with_the_window_in_front(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    in_front(&h, cx);
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "first");
    // A second conversation in a new tab beside the first's.
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.start_live(
                Launch::Agent {
                    kind: AgentId::CLAUDE,
                    resume: None,
                    account: None,
                },
                &MachineId::local(),
                &path,
                Place::Tab(LiveId(1)),
                None,
                window,
                cx,
            )
        })
    })
    .unwrap();
    h.settle(cx);
    assert_eq!(h.main_kind(cx), "live:2", "the second one is in view");
    prints_of(&h, cx, 0, &[INIT, SAID, RESULT]);
    // In front, the banner of the window says it; the desktop waits for the
    // window to lose the focus.
    assert!(desktop_notes(&h).is_empty());
    let banners = h.shell(cx, |shell| {
        shell
            .banners
            .iter()
            .map(|b| b.note.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(banners.len(), 1, "{banners:?}");
    assert_eq!(banners[0].session, Some(LiveId(1)));
    // Its tab has a dot, and the banner takes the person to it.
    exit_of(&h, cx, 0);
    assert!(h.shows("terminal-tab-dot-0", cx));
    let banner = h.shell(cx, |shell| shell.banners[0].id);
    click(&h, cx, &format!("notify-{banner}"));
    assert_eq!(h.main_kind(cx), "live:1");
    assert!(!h.shows("terminal-tab-dot-0", cx), "looked at: no dot");
}

#[gpui_kit::test]
fn the_settings_turn_the_conversation_notifications_off_and_the_sound_on(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    // A sound is off unless asked for.
    say(&h, cx, 1, "one");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    assert_eq!(desktop_notes(&h).len(), 1);
    assert_eq!(h.chimes.get(), 0);
    exits(&h, cx, 0);

    setting(&h, cx, "notify_sound", Value::Bool(true));
    say(&h, cx, 1, "two");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    assert_eq!(desktop_notes(&h).len(), 2);
    assert_eq!(h.chimes.get(), 1, "one sound for one notification");
    exits(&h, cx, 0);

    setting(&h, cx, "notify_chats", Value::Bool(false));
    say(&h, cx, 1, "three");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    assert_eq!(desktop_notes(&h).len(), 2, "off: nothing more");
    assert_eq!(h.chimes.get(), 1);
}

#[gpui_kit::test]
fn the_notification_centre_takes_the_person_to_the_conversation_it_was_about(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "say hello");
    prints(&h, cx, &[INIT, SAID, RESULT]);
    let tag = crate::ui::notify::tag(&desktop_notes(&h)[0]);
    assert_eq!(crate::ui::notify::session_of_tag(&tag), Some(LiveId(1)));
    assert_eq!(crate::ui::notify::session_of_tag("leon"), None);
    assert_eq!(crate::ui::notify::session_of_tag("leon-session-x"), None);
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.main = Main::Empty;
            shell.notification_activated(&tag, window, cx);
        })
    })
    .unwrap();
    h.settle(cx);
    assert_eq!(h.main_kind(cx), "live:1");
}

/// The process of the turn `at` ends.
fn exit_of(h: &Harness, cx: &mut TestAppContext, at: usize) {
    let (index, _) = turns(h)[at].clone();
    h.chat.0.lock().unwrap().outputs[index]
        .send(Output::Exited(0))
        .unwrap();
    h.settle(cx);
}

/// The agent of the turn `at` prints `lines`.
fn prints_of(h: &Harness, cx: &mut TestAppContext, at: usize, lines: &[&str]) {
    let (index, _) = turns(h)[at].clone();
    {
        let told = h.chat.0.lock().unwrap();
        for line in lines {
            told.outputs[index]
                .send(Output::Line((*line).to_owned()))
                .unwrap();
        }
    }
    h.settle(cx);
}

/// The conversation `id` goes on with `agent`, as the list of the field
/// does it.
fn switch(h: &Harness, cx: &mut TestAppContext, id: u64, agent: AgentId) -> bool {
    let done = cx
        .update_window(h.window.into(), |_, window, cx| {
            h.shell.update(cx, |shell, cx| {
                shell.chat_switch_agent(LiveId(id), agent, window, cx)
            })
        })
        .unwrap();
    h.settle(cx);
    done
}

#[gpui_kit::test]
fn the_chip_of_the_agent_lists_the_others_and_choosing_one_hands_the_conversation_over(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    let nothing = tempfile::tempdir().unwrap();
    sessions_in(&h, nothing.path());
    start(&h, cx, AgentId::CLAUDE, &path);
    answered(&h, cx, 1, "first question");
    assert!(h.shows("chat-agent-chip-1", cx));
    click(&h, cx, "chat-agent-chip-1");
    // The others that are installed, not the one in use.
    let rows = listed(&h, cx, 1);
    assert!(rows.contains(&"Codex".to_owned()), "{rows:?}");
    assert!(rows.contains(&"opencode".to_owned()), "{rows:?}");
    assert!(!rows.contains(&"Claude Code".to_owned()), "{rows:?}");
    let codex = rows.iter().position(|row| row == "Codex").unwrap();
    click(&h, cx, &format!("chat-menu-row-1-{codex}"));
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        assert_eq!(chat.agent, AgentId::CODEX);
        assert_eq!(chat.conversation.session_id, None);
        assert_eq!(chat.conversation.model, None);
        assert_eq!(
            (&chat.mode, &chat.model, &chat.effort),
            (&None, &None, &None)
        );
        assert!(chat.menu.is_none());
    });
    // The divider says it, and nothing else was added to the conversation.
    assert!(h.shows("chat-switch-1-2", cx));
    assert_eq!(
        said_in(&h, cx, 1),
        [
            "user: first question",
            "agent: Hello **there**.",
            "switch: claude to codex"
        ]
    );
    // Claude's turn was over and no other was started: the next is Codex's,
    // a new session of its own in the same folder.
    assert_eq!(turns(&h).len(), 1);
    say(&h, cx, 1, "and now go on");
    let started = turns(&h);
    assert_eq!(started.len(), 2);
    let turn = &started[1].1;
    assert_eq!(turn.argv[0], "codex", "{turn:?}");
    assert_eq!(turn.cwd, path);
    assert!(
        !turn.argv.iter().any(|arg| arg == "--resume" || arg == "s1"),
        "{turn:?}"
    );
    // Only what the person typed is in the conversation.
    let said = said_in(&h, cx, 1);
    assert_eq!(said.last().unwrap(), "user: and now go on", "{said:?}");
    assert!(
        said.iter().all(|line| !line.contains("leon-handoff")),
        "{said:?}"
    );
}

#[gpui_kit::test]
fn the_next_message_goes_with_what_was_said_before_and_only_that_one(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    // A conversation that was Codex's, made without a process: it is the
    // transcript that is handed over.
    start(&h, cx, AgentId::CODEX, &path);
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            let items = &mut shell.chats.get_mut(&LiveId(1)).unwrap().conversation.items;
            items.push(Item::User {
                text: "what is in lib.rs".into(),
                at: None,
            });
            items.push(Item::Agent {
                text: "It holds the parser.".into(),
                streaming: false,
                at: None,
            });
        })
    });
    assert!(switch(&h, cx, 1, AgentId::CLAUDE));
    say(&h, cx, 1, "now rename it");
    let first: Vec<String> = written(&h)
        .into_iter()
        .filter(|line| line.contains("now rename it"))
        .collect();
    assert_eq!(first.len(), 1, "{first:?}");
    for part in [
        "<leon-handoff",
        "from=",
        "what is in lib.rs",
        "It holds the parser.",
        "</leon-handoff>",
    ] {
        assert!(first[0].contains(part), "{part}: {}", first[0]);
    }
    assert!(
        first[0].find("</leon-handoff>").unwrap() < first[0].find("now rename it").unwrap(),
        "the block goes before the message"
    );
    // The turn after it is one more message of the same session.
    prints(&h, cx, &[INIT, SAID, RESULT]);
    exits(&h, cx, 0);
    say(&h, cx, 1, "and the tests");
    let second: Vec<String> = written(&h)
        .into_iter()
        .filter(|line| line.contains("and the tests"))
        .collect();
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(!second[0].contains("leon-handoff"), "{}", second[0]);
}

#[gpui_kit::test]
fn the_agent_cannot_be_switched_while_a_turn_runs_and_the_chip_does_not_open(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "work");
    prints(&h, cx, &[INIT, SAID]);
    assert!(h.shows("chat-agent-chip-1", cx));
    click(&h, cx, "chat-agent-chip-1");
    h.shell(cx, |shell| assert!(shell.chats[&LiveId(1)].menu.is_none()));
    assert!(!switch(&h, cx, 1, AgentId::CODEX));
    h.shell(cx, |shell| {
        assert_eq!(shell.chats[&LiveId(1)].agent, AgentId::CLAUDE);
    });
    // `/agent` is not offered either, and is offered again at the end.
    assert!(!command_labels(&h, cx).contains(&"/agent".to_owned()));
    prints(&h, cx, &[RESULT]);
    exits(&h, cx, 0);
    assert!(command_labels(&h, cx).contains(&"/agent".to_owned()));
}

/// The commands the slash lists in conversation 1.
fn command_labels(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            let chat = shell.chats.get_mut(&LiveId(1)).unwrap();
            chat.menu = Some(crate::ui::chat_menu::Menu {
                kind: crate::ui::chat_menu::MenuKind::Commands,
                selected: 0,
                scroll: gpui_kit::ScrollHandle::new(),
            });
        })
    });
    let rows = listed(h, cx, 1);
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.chats.get_mut(&LiveId(1)).unwrap().menu = None;
        })
    });
    rows
}

// ----- the history of what was sent, in the field --------------------------------------

/// What the person said in conversation 1, oldest first, as the conversation
/// holds it (a restored one is no different).
fn said_before(h: &Harness, cx: &mut TestAppContext, prompts: &[&str]) {
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            let chat = shell.chats.get_mut(&LiveId(1)).unwrap();
            for prompt in prompts {
                chat.conversation.said_at(prompt, None);
            }
        })
    });
}

/// Writes `text` in the field of conversation 1, the caret at `caret`
/// (the end without one).
fn write_field(h: &Harness, cx: &mut TestAppContext, text: &str, caret: Option<usize>) {
    cx.update_window(h.window.into(), |_, window, cx| {
        let input = h.shell.read(cx).chats[&LiveId(1)].input.clone();
        input.update(cx, |input, cx| {
            input.set_value(text.to_owned(), window, cx);
            let at = caret.unwrap_or(text.len());
            input.set_selected_range(at..at, cx);
            input.focus(window, cx);
        });
    })
    .unwrap();
    h.settle(cx);
}

fn caret(h: &Harness, cx: &mut TestAppContext) -> usize {
    cx.update(|cx| h.shell.read(cx).chats[&LiveId(1)].input.read(cx).cursor())
}

#[gpui_kit::test]
fn up_recalls_the_last_prompt_and_up_again_the_one_before(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    said_before(&h, cx, &["first", "second", "third"]);
    h.press("up", cx);
    assert_eq!(field(&h, cx, 1), "third");
    h.press("up", cx);
    assert_eq!(field(&h, cx, 1), "second");
    h.press("up", cx);
    assert_eq!(field(&h, cx, 1), "first");
    h.press("up", cx);
    assert_eq!(field(&h, cx, 1), "first", "the oldest stays");
    assert_eq!(caret(&h, cx), "first".len(), "the caret is at the end");
}

#[gpui_kit::test]
fn down_walks_forward_and_finally_gives_back_the_draft(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    said_before(&h, cx, &["first", "second"]);
    write_field(&h, cx, "half a thought", None);
    h.press("up", cx);
    h.press("up", cx);
    assert_eq!(field(&h, cx, 1), "first");
    h.press("down", cx);
    assert_eq!(field(&h, cx, 1), "second");
    h.press("down", cx);
    assert_eq!(field(&h, cx, 1), "half a thought", "the draft is back");
    // Out of browsing, Down is the field's own again and changes nothing.
    h.press("down", cx);
    assert_eq!(field(&h, cx, 1), "half a thought");
}

#[gpui_kit::test]
fn a_prompt_sent_twice_in_a_row_is_recalled_once_and_attachments_come_back(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    let with_file = crate::ui::king::with_attached(
        "look at this",
        &[std::path::PathBuf::from("/tmp/shot.png")],
    )
    .unwrap();
    said_before(&h, cx, &["older", "again", "again", &with_file]);
    h.press("up", cx);
    assert_eq!(field(&h, cx, 1), "look at this");
    assert_eq!(
        h.shell(cx, |s| s.chats[&LiveId(1)].attached.clone()),
        [std::path::PathBuf::from("/tmp/shot.png")]
    );
    h.press("up", cx);
    assert_eq!(field(&h, cx, 1), "again");
    assert!(h.shell(cx, |s| s.chats[&LiveId(1)].attached.is_empty()));
    h.press("up", cx);
    assert_eq!(field(&h, cx, 1), "older", "no second `again`");
}

#[gpui_kit::test]
fn up_with_the_caret_below_the_first_line_only_moves_the_caret(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    said_before(&h, cx, &["old"]);
    write_field(&h, cx, "line one\nline two", None);
    let before = caret(&h, cx);
    h.press("up", cx);
    assert_eq!(
        field(&h, cx, 1),
        "line one\nline two",
        "nothing was recalled"
    );
    assert!(caret(&h, cx) < before, "the caret went up a line");
    // Now on the first line: Up recalls.
    h.press("up", cx);
    assert_eq!(field(&h, cx, 1), "old");
}

#[gpui_kit::test]
fn up_belongs_to_the_slash_menu_while_it_is_open(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    offers(&h, cx);
    said_before(&h, cx, &["old"]);
    h.type_text("/", cx);
    assert!(h.shows("chat-menu-1", cx));
    h.press("down", cx);
    h.press("down", cx);
    assert_eq!(
        h.shell(cx, |s| s.chats[&LiveId(1)].menu.as_ref().unwrap().selected),
        2
    );
    h.press("up", cx);
    assert_eq!(
        h.shell(cx, |s| s.chats[&LiveId(1)].menu.as_ref().unwrap().selected),
        1
    );
    assert_eq!(field(&h, cx, 1), "/", "no prompt was recalled");
}

#[gpui_kit::test]
fn up_belongs_to_a_question_the_agent_waits_on(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    say(&h, cx, 1, "old");
    prints(&h, cx, &[INIT, BASH, ASK]);
    assert!(
        h.shell(cx, |s| !s.chats[&LiveId(1)]
            .conversation
            .questions
            .is_empty()),
        "the agent asks"
    );
    h.press("up", cx);
    assert_eq!(field(&h, cx, 1), "", "the question has the arrows");
}

#[gpui_kit::test]
fn the_slash_agent_command_opens_the_same_list(cx: &mut TestAppContext) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    start(&h, cx, AgentId::CLAUDE, &path);
    h.type_text("/agent", cx);
    let rows = listed(&h, cx, 1);
    assert!(rows.contains(&"/agent".to_owned()), "{rows:?}");
    let at = rows.iter().position(|row| row == "/agent").unwrap();
    click(&h, cx, &format!("chat-menu-row-1-{at}"));
    let agents = listed(&h, cx, 1);
    assert!(agents.contains(&"Codex".to_owned()), "{agents:?}");
    assert_eq!(field(&h, cx, 1), "");
}

#[gpui_kit::test]
fn the_messages_before_a_switch_cannot_be_edited_and_the_ones_after_it_can(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    let nothing = tempfile::tempdir().unwrap();
    sessions_in(&h, nothing.path());
    start(&h, cx, AgentId::CLAUDE, &path);
    answered(&h, cx, 1, "before");
    h.hover_on("chat-user-1-0".into(), cx);
    assert!(h.shows_dynamic("chat-edit-1-0".into(), cx));
    assert!(switch(&h, cx, 1, AgentId::OPENCODE));
    h.hover_on("chat-user-1-0".into(), cx);
    assert!(!h.shows_dynamic("chat-edit-1-0".into(), cx));
    assert!(!h.shows_dynamic("chat-retry-1-0".into(), cx));
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        assert!(!chat.can_rewind(0));
        assert!(chat.rewind_blocked_by_switch(0));
        assert!(!chat.rewind_blocked_by_switch(3));
    });
    // A message to the new agent can: it began a turn of its own.
    say(&h, cx, 1, "after");
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        let at = chat.conversation.items.len() - 1;
        assert!(chat.mark_at(at).is_some());
    });
}

#[gpui_kit::test]
fn going_back_to_the_first_message_after_a_switch_hands_the_conversation_over_again(
    cx: &mut TestAppContext,
) {
    let Some((h, _dir, path)) = window(cx) else {
        return;
    };
    let nothing = tempfile::tempdir().unwrap();
    sessions_in(&h, nothing.path());
    start(&h, cx, AgentId::CLAUDE, &path);
    answered(&h, cx, 1, "before");
    assert!(switch(&h, cx, 1, AgentId::OPENCODE));
    say(&h, cx, 1, "after");
    exits(&h, cx, 0);
    h.shell(cx, |shell| {
        let handoff = shell.chats[&LiveId(1)]
            .handoff
            .clone()
            .expect("handed over");
        assert!(handoff.sent_in.is_some(), "the first message took it");
        assert!(handoff.text.contains("before"));
    });
    // That message, edited: its turn is gone and so is what it carried.
    from_here(&h, cx, "chat-edit", 3);
    click(&h, cx, "chat-rewind-keep-1");
    h.shell(cx, |shell| {
        let chat = &shell.chats[&LiveId(1)];
        assert_eq!(chat.handoff.as_ref().unwrap().sent_in, None);
        assert_eq!(chat.agent, AgentId::OPENCODE);
        // The rule stays: what is before it is still the first agent's.
        assert!(chat
            .conversation
            .items
            .iter()
            .any(|item| matches!(item, Item::Switched { .. })));
    });
    assert_eq!(field(&h, cx, 1), "after");
}

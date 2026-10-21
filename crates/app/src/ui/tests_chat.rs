//! Tests of conversations in Leon's own interface: the setting, a turn from
//! the prompt to its end with a scripted agent in the place of the process,
//! the agent's questions, and that the agents share one interface.

use super::live::real_worktree;
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
        assert_eq!(
            chat.conversation.items,
            [
                Item::User {
                    text: "say hello".into()
                },
                Item::Agent {
                    text: "Hello **there**.".into(),
                    streaming: false
                },
            ]
        );
    });
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
        ["/model", "/effort", "/mode", "/voice", "/compact", "/context"]
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
        asked.iter().any(|call| call.contains("remote") && call.contains("origin")),
        "where its origin is, for GitHub: {asked:?}"
    );
}

//! The Grok adapter against what grok 1.0.52 printed in real runs (`grok
//! agent stdio`, driven over its standard streams in a scratch folder). The
//! fixtures are what it printed, in order, with the folder shortened to
//! `/work`, the lists of models and commands cut to two, its `_meta` members
//! left out and one of each kind of its own notifications kept.

use super::*;
use crate::agents::{Action, Outgoing, Step};
use crate::events::{Answer, PermissionEffect, QuestionKind, TurnEvent};
use serde_json::{json, Value};

/// A new conversation: it writes a file, and is allowed to.
const ALLOW: &str = include_str!("fixtures/allow.jsonl");
/// The same conversation continued with another model and effort: it wants
/// to run a command, and is refused.
const REJECT: &str = include_str!("fixtures/reject.jsonl");
/// What `grok models` printed.
const MODELS: &str = "leon-version:grok 1.0.52 (0920c8169e27)\nleon-models:\nYou are logged in with grok.com.\n\nDefault model: grok-4.7\n\nAvailable models:\n  * grok-4.7 (default)\n  - grok-4.7-build-fast\n  - grok-4.6\n  - grok-4.5\n";

const SESSION: &str = "01a1328e-79a5-7151-b703-49d0fcc757cc";

fn spec<'a>(prompt: &'a str) -> TurnSpec<'a> {
    TurnSpec {
        program: "grok",
        prompt,
        cwd: Some("/work"),
        ..TurnSpec::default()
    }
}

fn value(outgoing: &Outgoing) -> Value {
    serde_json::from_str(&outgoing.line).unwrap()
}

/// Reads a fixture to its end, or up to and including the first line that
/// holds `until`. What the session wanted written is acknowledged at once.
fn play(session: &mut dyn AgentSession, fixture: &str, until: Option<&str>) -> Step {
    let mut all = Step::default();
    for line in fixture.lines() {
        let mut step = Step::default();
        session.line(&serde_json::from_str(line).unwrap(), &mut step);
        all.events.append(&mut step.events);
        if step.commands.is_some() {
            all.commands = step.commands.take();
        }
        for outgoing in step.send {
            let mut acked = Step::default();
            session.delivered(&outgoing.tag, &mut acked);
            all.events.append(&mut acked.events);
            all.send.push(outgoing);
        }
        if until.is_some_and(|mark| line.contains(mark)) {
            break;
        }
    }
    all
}

fn methods(sent: &[Outgoing]) -> Vec<String> {
    sent.iter()
        .map(|sent| {
            value(sent)["method"]
                .as_str()
                .unwrap_or("answer")
                .to_owned()
        })
        .collect()
}

#[test]
fn a_turn_is_grok_on_its_standard_streams_and_asks_for_no_sign_in() {
    assert_eq!(argv(&spec("hi")), ["grok", "agent", "stdio"]);
    let first = session(1).first(&spec("hi"));
    assert_eq!(methods(&first), ["initialize", "session/new"]);
    assert_eq!(value(&first[1])["params"]["cwd"], "/work");
}

#[test]
fn a_change_is_asked_about_shown_and_made_when_allowed() {
    let mut session = session(1);
    session.first(&spec("Create made.txt"));
    let asked = play(&mut *session, ALLOW, Some("session/request_permission"));
    assert!(asked.events.contains(&TurnEvent::Started {
        session_id: SESSION.into(),
        model: Some("grok-4.7".into()),
    }));
    // Its own commands are listed inside the conversation.
    assert_eq!(asked.commands.map(|listed| listed.len()), Some(2));
    assert_eq!(methods(&asked.send), ["session/prompt"]);
    assert!(asked
        .events
        .iter()
        .any(|event| matches!(event, TurnEvent::Thought { .. })));
    assert!(asked.events.iter().any(|event| matches!(
        event,
        TurnEvent::ToolUse { name, detail, .. } if name == "write" && detail.contains("/work/made.txt")
    )));
    let Some(TurnEvent::Question {
        id,
        kind:
            QuestionKind::Permission {
                tool,
                summary,
                detail,
                choices,
                ..
            },
    }) = asked.events.last()
    else {
        panic!("{:?}", asked.events.last());
    };
    assert_eq!(tool, "edit");
    assert_eq!(summary, "Write `/work/made.txt`");
    // Grok asks with what the tool was given and no change beside it.
    assert_eq!(detail, "/work/made.txt\n+++ new file\nok");
    let offered: Vec<(&str, PermissionEffect)> = choices
        .iter()
        .map(|choice| (choice.id.as_str(), choice.effect))
        .collect();
    assert_eq!(
        offered,
        [
            ("allow-edits-session", PermissionEffect::AllowAlways),
            ("allow-once", PermissionEffect::AllowOnce),
            ("reject-once", PermissionEffect::Deny),
        ]
    );
    // A refusal ends Grok's turn, and the choice says so.
    assert!(choices[2].label.ends_with(", and stop the turn"));

    let answer = session
        .encode(Action::Answer(
            id.clone(),
            Answer::Permission {
                choice_id: "allow-once".into(),
                reason: None,
            },
        ))
        .unwrap();
    assert_eq!(
        value(&answer[0]),
        json!({"jsonrpc": "2.0", "id": 0, "result": {"outcome": {"outcome": "selected", "optionId": "allow-once"}}})
    );

    let after = ALLOW.split_once("session/request_permission").unwrap().1;
    let after = after.split_once('\n').unwrap().1;
    let rest = play(&mut *session, after, None);
    assert!(rest
        .events
        .iter()
        .any(|event| matches!(event, TurnEvent::ToolResult { ok: true, .. })));
    assert!(rest
        .events
        .iter()
        .any(|event| matches!(event, TurnEvent::Text { text } if text == "Done.")));
    assert!(matches!(
        rest.events.last(),
        Some(TurnEvent::Finished {
            ok: true,
            error: None,
            interrupted: false,
            ..
        })
    ));
    assert!(session.idle());
}

#[test]
fn a_conversation_is_continued_with_the_model_and_the_effort_asked_for() {
    let mut session = session(1);
    let first = session.first(&TurnSpec {
        resume: Some(SESSION),
        model: Some("grok-4.7-build-fast"),
        effort: Some("low"),
        mode: Some(MODE_ASK),
        ..spec("And now?")
    });
    assert_eq!(methods(&first), ["initialize", "session/resume"]);
    assert_eq!(value(&first[1])["params"]["sessionId"], SESSION);

    let played = play(&mut *session, REJECT, Some("session/request_permission"));
    // Each option once the one before was answered, then the prompt. Grok
    // has no option that is a mode, and is asked for none.
    let options: Vec<(String, String)> = played
        .send
        .iter()
        .map(value)
        .filter(|sent| sent["method"] == "session/set_config_option")
        .map(|sent| {
            (
                sent["params"]["configId"].as_str().unwrap().to_owned(),
                sent["params"]["value"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        options,
        [
            ("model".to_owned(), "grok-4.7-build-fast".to_owned()),
            ("reasoning_effort".to_owned(), "low".to_owned()),
        ]
    );
    assert_eq!(
        methods(&played.send).last().map(String::as_str),
        Some("session/prompt")
    );
    let Some(TurnEvent::Question {
        id,
        kind: QuestionKind::Permission { tool, summary, .. },
    }) = played.events.last()
    else {
        panic!("{:?}", played.events.last());
    };
    assert_eq!(tool, "shell");
    assert_eq!(summary, "Execute `rm made.txt`");

    let refusal = session
        .encode(Action::Answer(
            id.clone(),
            Answer::Permission {
                choice_id: "deny".into(),
                reason: None,
            },
        ))
        .unwrap();
    assert_eq!(
        value(&refusal[0])["result"]["outcome"]["optionId"],
        "reject-once"
    );
    let after = REJECT.split_once("session/request_permission").unwrap().1;
    let rest = play(&mut *session, after.split_once('\n').unwrap().1, None);
    assert!(rest
        .events
        .iter()
        .any(|event| matches!(event, TurnEvent::ToolResult { ok: false, .. })));
    assert!(matches!(
        rest.events.last(),
        Some(TurnEvent::Finished {
            ok: false,
            interrupted: true,
            ..
        })
    ));
}

#[test]
fn never_asking_allows_what_grok_asks_at_once_and_tells_grok_nothing() {
    let mut session = session(1);
    let first = session.first(&TurnSpec {
        mode: Some(acp::MODE_AUTO),
        ..spec("Create made.txt")
    });
    assert_eq!(methods(&first), ["initialize", "session/new"]);
    let played = play(&mut *session, ALLOW, None);
    assert!(!played
        .events
        .iter()
        .any(|event| matches!(event, TurnEvent::Question { .. })));
    assert!(played
        .send
        .iter()
        .map(value)
        .any(|sent| { sent["id"] == 0 && sent["result"]["outcome"]["optionId"] == "allow-once" }));
    assert!(!played
        .send
        .iter()
        .map(value)
        .any(|sent| sent["method"] == "session/set_config_option"));
    assert!(matches!(
        played.events.last(),
        Some(TurnEvent::Finished { ok: true, .. })
    ));
}

#[test]
fn a_mode_changed_while_it_works_is_this_librarys_to_keep() {
    let mut session = session(1);
    session.first(&spec("Create made.txt"));
    play(&mut *session, ALLOW, Some("\"leon-session\""));
    let told = session
        .encode(Action::SetMode(acp::MODE_AUTO.into()))
        .unwrap();
    assert!(told.is_empty());
    // From here on nothing is asked.
    let after = ALLOW.split_once("\"leon-session\"").unwrap().1;
    let rest = play(&mut *session, after.split_once('\n').unwrap().1, None);
    assert!(!rest
        .events
        .iter()
        .any(|event| matches!(event, TurnEvent::Question { .. })));
}

#[test]
fn its_models_are_read_from_its_own_list() {
    let options = parse_options(MODELS);
    assert_eq!(options.version.as_deref(), Some("1.0.52"));
    let models: Vec<&str> = options
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect();
    assert_eq!(
        models,
        ["grok-4.7", "grok-4.7-build-fast", "grok-4.6", "grok-4.5"]
    );
    assert!(options.models[0].efforts.contains(&"xhigh".to_owned()));
    // The one it marks is the one it answers with unasked.
    assert_eq!(options.default_model.as_deref(), Some("grok-4.7"));
    let modes: Vec<&str> = options.modes.iter().map(|mode| mode.id.as_str()).collect();
    assert_eq!(modes, [MODE_ASK, acp::MODE_AUTO]);
    assert_eq!(options.default_mode.as_deref(), Some(MODE_ASK));
    assert!(!options.assumed);
    // A Grok that listed nothing: what is offered is a guess.
    assert!(parse_options("leon-version:\nleon-models:\n").assumed);
}

/// What Grok printed when it read a file: the input is its own shape, and
/// the line shown is its own words for it.
#[test]
fn a_tool_whose_input_this_library_does_not_read_is_shown_in_groks_words() {
    let mut session = session(1);
    session.first(&spec("Read it"));
    play(&mut *session, ALLOW, Some("\"leon-session\""));
    let update = |update: Value| json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": SESSION, "update": update}});
    let mut step = Step::default();
    session.line(
        &update(
            json!({"sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "read_file",
            "rawInput": {"target_file": "/work/leon.txt"}}),
        ),
        &mut step,
    );
    session.line(
        &update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "call-1", "kind": "read",
            "title": "Read `/work/leon.txt`",
            "rawInput": {"variant": "ReadFile", "target_file": "/work/leon.txt"}}),
        ),
        &mut step,
    );
    assert!(
        step.events.iter().any(|event| matches!(
            event,
            TurnEvent::ToolUse { name, summary, .. }
                if name == "read_file" && summary == "Read `/work/leon.txt`"
        )),
        "{:?}",
        step.events
    );
}

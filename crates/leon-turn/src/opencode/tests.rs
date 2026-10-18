//! The opencode adapter against what opencode 2.0.24 printed in real runs
//! (`opencode acp`, driven over its standard streams in a scratch folder
//! whose configuration asks before commands and edits). The fixtures are
//! what it printed, in order, with the folder shortened to `/work`, the list
//! of models cut to two and the list of commands left out.

use super::*;
use crate::agents::{Action, Outgoing, Step};
use crate::error::TurnError;
use crate::events::*;
use serde_json::json;

const PLAIN: &str = include_str!("fixtures/plain.jsonl");
const ALLOW: &str = include_str!("fixtures/allow.jsonl");
const REJECT: &str = include_str!("fixtures/reject.jsonl");
const CANCEL: &str = include_str!("fixtures/cancel.jsonl");
const EDIT: &str = include_str!("fixtures/edit.jsonl");
const OPTIONS: &str = include_str!("fixtures/options.txt");

const SESSION: &str = "ses_ee6eb1fe8ffexRUoyXY86yli8U";

fn fresh() -> Box<dyn AgentSession> {
    session(100)
}

fn spec<'a>(prompt: &'a str) -> TurnSpec<'a> {
    TurnSpec {
        program: "opencode",
        prompt,
        cwd: Some("/work"),
        ..TurnSpec::default()
    }
}

fn value(outgoing: &Outgoing) -> Value {
    serde_json::from_str(&outgoing.line).unwrap()
}

fn read(session: &mut dyn AgentSession, line: &str) -> Step {
    let mut step = Step::default();
    session.line(&serde_json::from_str(line).unwrap(), &mut step);
    step
}

/// Reads a fixture to its end, or up to and including the first line that
/// holds `until`. What the session wanted written is acknowledged at once.
fn play(session: &mut dyn AgentSession, fixture: &str, until: Option<&str>) -> Step {
    let mut all = Step::default();
    for line in fixture.lines() {
        let mut step = read(session, line);
        all.events.append(&mut step.events);
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

fn methods(step: &Step) -> Vec<String> {
    step.send
        .iter()
        .map(|outgoing| {
            let value = value(outgoing);
            value["method"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("answer to {}", value["id"]))
        })
        .collect()
}

fn permission(choice: &str) -> Answer {
    Answer::Permission {
        choice_id: choice.into(),
        reason: None,
    }
}

#[test]
fn a_turn_is_its_server_whatever_is_asked_of_it() {
    let turn = TurnSpec {
        mode: Some(MODE_AUTO),
        model: Some("opencode/big-pickle"),
        resume: Some(SESSION),
        ..spec("--help; rm -rf ~")
    };
    assert_eq!(argv(&turn), ["opencode", "acp"]);
}

#[test]
fn its_session_is_given_the_servers_of_tools() {
    let mut session = fresh();
    let servers = [crate::agents::McpServer {
        name: "leon-king".into(),
        command: "/bin/leon".into(),
        args: vec!["king".into(), "mcp".into()],
        env: vec![("LEON_DATA_DIR".into(), "/data".into())],
    }];
    let turn = TurnSpec {
        mcp_servers: &servers,
        ..spec("hi")
    };
    let new = value(&session.first(&turn)[1]);
    assert_eq!(
        new["params"]["mcpServers"],
        json!([{
            "name": "leon-king",
            "command": "/bin/leon",
            "args": ["king", "mcp"],
            "env": [{ "name": "LEON_DATA_DIR", "value": "/data" }],
        }])
    );
}

#[test]
fn it_is_greeted_and_given_a_session_in_the_folder_then_the_prompt_when_the_session_answered() {
    let mut session = fresh();
    let first = session.first(&spec("Reply with exactly: hola"));
    let tags: Vec<&str> = first.iter().map(|outgoing| outgoing.tag.as_str()).collect();
    assert_eq!(tags, ["h.init", "h.session"]);
    let hello = value(&first[0]);
    assert_eq!(hello["jsonrpc"], "2.0");
    assert_eq!(hello["method"], "initialize");
    // No files and no terminal are offered: the agent uses its own.
    assert_eq!(hello["params"]["clientCapabilities"], json!({}));
    let new = value(&first[1]);
    assert_eq!(new["method"], "session/new");
    assert_eq!(new["params"], json!({ "cwd": "/work", "mcpServers": [] }));

    let step = play(session.as_mut(), PLAIN, None);
    assert_eq!(methods(&step), ["session/prompt"]);
    let prompt = value(&step.send[0]);
    assert_eq!(step.send[0].tag, "h.turn");
    assert_eq!(prompt["id"], "leon-turn");
    assert_eq!(
        prompt["params"],
        json!({
            "sessionId": SESSION,
            "prompt": [{ "type": "text", "text": "Reply with exactly: hola" }],
        })
    );
    assert_eq!(
        step.events,
        [
            TurnEvent::Started {
                session_id: SESSION.into(),
                model: Some("opencode/claude-haiku-5-5".into()),
            },
            TurnEvent::ModeChanged {
                mode: "build".into()
            },
            TurnEvent::TextDelta {
                text: "hola".into()
            },
            TurnEvent::Text {
                text: "hola".into()
            },
            TurnEvent::Usage {
                tokens: Tokens {
                    input: 14632,
                    cached_input: 5215,
                    output: 4,
                    reasoning: 0,
                    context_window: Some(1_000_000),
                },
            },
            TurnEvent::Finished {
                ok: true,
                error: None,
                // A conversation that began with this turn cost what the turn did.
                cost_usd: Some(0.001287075),
                duration_ms: None,
                denied_tools: vec![],
                interrupted: false,
            },
        ]
    );
    // Its input is closed now, which ends it.
    assert!(session.idle());
    assert_eq!(session.exited(0), [TurnEvent::Exited { code: 0 }]);
}

#[test]
fn a_conversation_is_continued_by_its_id_and_its_cost_so_far_is_not_this_turns() {
    let mut session = fresh();
    let turn = TurnSpec {
        resume: Some(SESSION),
        ..spec("go on")
    };
    let first = session.first(&turn);
    let resume = value(&first[1]);
    assert_eq!(resume["method"], "session/resume");
    assert_eq!(
        resume["params"],
        json!({ "sessionId": SESSION, "cwd": "/work", "mcpServers": [] })
    );
    // Its answer does not name the session again; the turn is started in
    // the one that was asked for.
    let step = play(session.as_mut(), ALLOW, Some("leon-session"));
    assert_eq!(value(&step.send[0])["params"]["sessionId"], SESSION);
    assert!(matches!(
        &step.events[0],
        TurnEvent::Started { session_id, .. } if session_id == SESSION
    ));
    let rest = play(session.as_mut(), REJECT, None);
    assert!(matches!(
        rest.events.last(),
        Some(TurnEvent::Finished { cost_usd: None, .. })
    ));
}

#[test]
fn the_model_and_the_effort_wait_for_its_providers_and_go_one_after_the_other() {
    let mut session = fresh();
    let turn = TurnSpec {
        model: Some("opencode-go/claude-haiku-5-5"),
        effort: Some("high"),
        mode: Some("plan"),
        ..spec("write the poem")
    };
    session.first(&turn);
    // The session answered: its list of models is not the whole one yet,
    // and nothing is set.
    let answered = play(session.as_mut(), EDIT, Some("leon-session"));
    assert!(answered.send.is_empty(), "{:?}", answered.send);
    // Its first update says the providers are known: the model goes first.
    let update = EDIT
        .lines()
        .find(|line| line.contains("config_option_update"))
        .unwrap();
    let step = read(session.as_mut(), update);
    assert_eq!(step.send.len(), 1);
    let model = value(&step.send[0]);
    assert_eq!(step.send[0].tag, "h.model");
    assert_eq!(model["method"], "session/set_config_option");
    assert_eq!(model["params"]["configId"], "model");
    assert_eq!(model["params"]["value"], "opencode-go/claude-haiku-5-5");
    // A second update changes nothing: the model's answer is what is waited for.
    assert!(read(session.as_mut(), update).send.is_empty());
    let ok = r#"{"jsonrpc":"2.0","id":"leon-model","result":{"configOptions":[]}}"#;
    let step = read(session.as_mut(), ok);
    assert_eq!(value(&step.send[0])["params"]["configId"], "effort");
    // An effort this model lacks is no reason not to run.
    let refused = r#"{"jsonrpc":"2.0","id":"leon-effort","error":{"code":-32602,"message":"Invalid params: effort not found: high"}}"#;
    let step = read(session.as_mut(), refused);
    assert_eq!(value(&step.send[0])["params"]["configId"], "mode");
    assert_eq!(value(&step.send[0])["params"]["value"], "plan");
    let step = read(
        session.as_mut(),
        r#"{"jsonrpc":"2.0","id":"leon-mode","result":{"configOptions":[{"id":"mode","currentValue":"plan","options":[]}]}}"#,
    );
    assert_eq!(
        step.events,
        [TurnEvent::ModeChanged {
            mode: "plan".into()
        }]
    );
    assert_eq!(methods(&step), ["session/prompt"]);
}

#[test]
fn a_mode_alone_does_not_wait_for_the_providers() {
    let mut session = fresh();
    let turn = TurnSpec {
        mode: Some("plan"),
        ..spec("look around")
    };
    session.first(&turn);
    let step = play(session.as_mut(), PLAIN, Some("leon-session"));
    assert_eq!(methods(&step), ["session/set_config_option"]);
    assert_eq!(value(&step.send[0])["params"]["value"], "plan");
}

#[test]
fn a_command_it_may_not_run_by_itself_is_a_question_with_its_own_options() {
    let mut session = fresh();
    session.first(&TurnSpec {
        resume: Some(SESSION),
        ..spec("make the file")
    });
    let step = play(session.as_mut(), ALLOW, Some("session/request_permission"));
    // The tool is shown once it is known what it does, not when it is
    // first mentioned.
    assert_eq!(
        step.events[2..],
        [
            TurnEvent::ToolUse {
                id: "toolu_01KWrNmqWJshpAvYDE2prQnw".into(),
                name: "shell".into(),
                summary: "echo leon-ok > made.txt".into(),
                detail: String::new(),
            },
            TurnEvent::Question {
                id: "0".into(),
                kind: QuestionKind::Permission {
                    tool: "shell".into(),
                    summary: "echo leon-ok > made.txt".into(),
                    detail:
                        "{\n  \"command\": \"echo leon-ok > made.txt\",\n  \"cwd\": \"/work\"\n}"
                            .into(),
                    reason: None,
                    choices: vec![
                        PermissionChoice {
                            id: "once".into(),
                            effect: PermissionEffect::AllowOnce,
                            label: "Allow once".into(),
                            scope: None,
                            takes_reason: false,
                        },
                        PermissionChoice {
                            id: "always".into(),
                            effect: PermissionEffect::AllowAlways,
                            label: "Always allow".into(),
                            scope: None,
                            takes_reason: false,
                        },
                        PermissionChoice {
                            id: "reject".into(),
                            effect: PermissionEffect::Deny,
                            label: "Reject, and stop the turn".into(),
                            scope: None,
                            takes_reason: false,
                        },
                    ],
                },
            },
        ]
    );
    assert!(!session.idle());

    // The answer goes back under the number it asked with, as a number.
    let out = session
        .encode(Action::Answer("0".into(), permission("once")))
        .unwrap();
    assert_eq!(out[0].tag, "a.0");
    assert_eq!(
        value(&out[0]),
        json!({
            "jsonrpc": "2.0",
            "id": 0,
            "result": { "outcome": { "outcome": "selected", "optionId": "once" } },
        })
    );
    // An option it did not offer is no answer; a plain refusal is its own.
    assert_eq!(
        session.encode(Action::Answer("0".into(), permission("forever"))),
        Err(TurnError::BadAnswer)
    );
    let refusal = session
        .encode(Action::Answer("0".into(), permission("deny")))
        .unwrap();
    assert_eq!(
        value(&refusal[0])["result"]["outcome"]["optionId"],
        "reject"
    );
    assert_eq!(
        session.encode(Action::Answer("7".into(), permission("once"))),
        Err(TurnError::UnknownQuestion)
    );

    // Delivered, the question is closed; the rest of the turn reads on.
    let mut step = Step::default();
    session.delivered("a.0", &mut step);
    assert_eq!(
        step.events,
        [TurnEvent::QuestionResolved { id: "0".into() }]
    );
    let after: Vec<&str> = ALLOW
        .lines()
        .skip_while(|line| !line.contains("session/request_permission"))
        .skip(1)
        .collect();
    let rest = play(session.as_mut(), &after.join("\n"), None);
    assert_eq!(
        rest.events,
        [
            TurnEvent::ToolResult {
                id: "toolu_01KWrNmqWJshpAvYDE2prQnw".into(),
                ok: true,
                summary: "(no output)".into(),
                detail: String::new(),
            },
            TurnEvent::TextDelta {
                text: "done".into()
            },
            TurnEvent::Text {
                text: "done".into()
            },
            TurnEvent::Usage {
                tokens: Tokens {
                    input: 32893,
                    cached_input: 29292,
                    output: 190,
                    reasoning: 130,
                    context_window: Some(1_000_000),
                },
            },
            TurnEvent::Finished {
                ok: true,
                error: None,
                cost_usd: None,
                duration_ms: None,
                denied_tools: vec![],
                interrupted: false,
            },
        ]
    );
}

#[test]
fn a_refusal_ends_the_turn_as_opencode_ends_it() {
    let mut session = fresh();
    session.first(&TurnSpec {
        resume: Some(SESSION),
        ..spec("make the file")
    });
    let step = play(session.as_mut(), REJECT, None);
    let tail = &step.events[step.events.len() - 4..];
    assert_eq!(
        tail,
        [
            TurnEvent::ToolResult {
                id: "toolu_0152Gx3QVEeMcTHYy7fZk2bP".into(),
                ok: false,
                summary: "The user declined this tool call".into(),
                detail: String::new(),
            },
            // Nobody answered it here: the end of the turn closes it.
            TurnEvent::QuestionResolved { id: "0".into() },
            TurnEvent::Usage {
                tokens: Tokens {
                    input: 19854,
                    cached_input: 18223,
                    output: 113,
                    reasoning: 58,
                    context_window: Some(1_000_000),
                },
            },
            TurnEvent::Finished {
                ok: false,
                error: None,
                cost_usd: None,
                duration_ms: None,
                denied_tools: vec![],
                interrupted: true,
            },
        ]
    );
}

#[test]
fn a_change_of_a_file_is_asked_with_the_change_and_thoughts_stay_apart() {
    let mut session = fresh();
    session.first(&spec("write the poem"));
    let step = play(session.as_mut(), EDIT, None);
    let question = step
        .events
        .iter()
        .find_map(|event| match event {
            TurnEvent::Question { kind, .. } => Some(kind.clone()),
            _ => None,
        })
        .unwrap();
    let QuestionKind::Permission {
        tool,
        summary,
        detail,
        ..
    } = question
    else {
        panic!("not a permission");
    };
    assert_eq!((tool.as_str(), summary.as_str()), ("edit", "poem.txt"));
    assert_eq!(detail, "/work/poem.txt\n+++ new file\nleon");
    assert!(step.events.contains(&TurnEvent::ToolUse {
        id: "toolu_01YU64bAyC29s74rBc1SiiFV".into(),
        name: "write".into(),
        summary: "poem.txt".into(),
        detail: "poem.txt\n+ leon".into(),
    }));
    assert!(step.events.contains(&TurnEvent::ToolResult {
        id: "toolu_01YU64bAyC29s74rBc1SiiFV".into(),
        ok: true,
        summary: "Created file successfully: poem.txt".into(),
        detail: String::new(),
    }));
    let thought = step
        .events
        .iter()
        .position(|event| matches!(event, TurnEvent::Thought { text } if text.contains("reply")))
        .unwrap();
    assert_eq!(
        step.events[thought + 1],
        TurnEvent::TextDelta {
            text: "written".into()
        }
    );
    assert!(matches!(
        step.events.last(),
        Some(TurnEvent::Finished {
            ok: true,
            cost_usd: Some(_),
            ..
        })
    ));
}

#[test]
fn stopping_it_cancels_the_session_and_what_it_still_asks() {
    let mut session = fresh();
    session.first(&TurnSpec {
        resume: Some(SESSION),
        ..spec("sleep")
    });
    play(session.as_mut(), CANCEL, Some("session/request_permission"));
    let out = session.encode(Action::Interrupt).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(
        value(&out[0]),
        json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": { "sessionId": SESSION },
        })
    );
    assert_eq!(out[1].tag, "a.0");
    assert_eq!(
        value(&out[1])["result"]["outcome"],
        json!({ "outcome": "cancelled" })
    );
    // It cannot be told more while it works.
    assert_eq!(
        session.encode(Action::Prompt("also say hi".into())),
        Err(TurnError::NotSupported)
    );
    let after: Vec<&str> = CANCEL
        .lines()
        .skip_while(|line| !line.contains("session/request_permission"))
        .skip(1)
        .collect();
    let rest = play(session.as_mut(), &after.join("\n"), None);
    assert_eq!(
        rest.events,
        [
            TurnEvent::ToolResult {
                id: "toolu_01LCs3ZQVmiGroARs96T8CDk".into(),
                ok: false,
                summary: "Tool execution interrupted".into(),
                detail: String::new(),
            },
            TurnEvent::QuestionResolved { id: "0".into() },
            TurnEvent::Usage {
                tokens: Tokens {
                    input: 20211,
                    cached_input: 18390,
                    output: 115,
                    reasoning: 64,
                    context_window: Some(1_000_000),
                },
            },
            TurnEvent::Finished {
                ok: false,
                error: None,
                cost_usd: None,
                duration_ms: None,
                denied_tools: vec![],
                interrupted: true,
            },
        ]
    );
    assert_eq!(session.encode(Action::Interrupt), Err(TurnError::TurnEnded));
}

#[test]
fn in_its_never_asking_mode_every_permission_is_allowed_at_once() {
    let mut session = fresh();
    session.first(&TurnSpec {
        resume: Some(SESSION),
        mode: Some(MODE_AUTO),
        ..spec("make the file")
    });
    let step = play(session.as_mut(), ALLOW, None);
    // The mode set is its default agent's; the question never reaches the chat.
    let mode = step
        .send
        .iter()
        .map(value)
        .find(|value| value["id"] == "leon-mode")
        .unwrap();
    assert_eq!(mode["params"]["value"], "build");
    assert!(!step
        .events
        .iter()
        .any(|event| matches!(event, TurnEvent::Question { .. })));
    let answer = step
        .send
        .iter()
        .find(|outgoing| outgoing.tag == "a.0")
        .map(value)
        .unwrap();
    assert_eq!(answer["result"]["outcome"]["optionId"], "once");
    assert!(step.events.contains(&TurnEvent::ModeChanged {
        mode: MODE_AUTO.into()
    }));
}

#[test]
fn its_mode_and_model_can_be_changed_while_it_works() {
    let mut session = fresh();
    session.first(&spec("go"));
    play(session.as_mut(), PLAIN, Some("leon-session"));
    let out = session.encode(Action::SetMode("plan".into())).unwrap();
    let set = value(&out[0]);
    assert_eq!(set["method"], "session/set_config_option");
    assert_eq!(
        set["params"],
        json!({ "sessionId": SESSION, "configId": "mode", "value": "plan" })
    );
    let out = session
        .encode(Action::SetModel("opencode/big-pickle".into()))
        .unwrap();
    assert_eq!(value(&out[0])["params"]["configId"], "model");
    assert_ne!(value(&out[0])["id"], set["id"]);
}

#[test]
fn a_turn_found_again_is_read_from_its_start_without_knowing_the_prompt() {
    // No `first`: this process was never told the prompt.
    let mut session = fresh();
    let step = play(session.as_mut(), ALLOW, Some("session/request_permission"));
    // In place of the prompt it writes a request under the prompt's tag,
    // which the engine drops when the prompt had been acknowledged.
    assert_eq!(step.send.len(), 1);
    assert_eq!(step.send[0].tag, "h.turn");
    assert_eq!(value(&step.send[0])["id"], "leon-lost");
    // The session is named by what it prints, and its question is open.
    assert!(step.events.iter().any(|event| matches!(
        event,
        TurnEvent::Started { session_id, .. } if session_id == SESSION
    )));
    assert!(matches!(
        step.events.last(),
        Some(TurnEvent::Question { id, .. }) if id == "0"
    ));
    let out = session
        .encode(Action::Answer("0".into(), permission("always")))
        .unwrap();
    assert_eq!(value(&out[0])["result"]["outcome"]["optionId"], "always");
}

#[test]
fn a_turn_nobody_started_says_so_and_ends() {
    let mut session = fresh();
    let step = play(session.as_mut(), ALLOW, Some("leon-session"));
    assert_eq!(value(&step.send[0])["method"], "leon/lost");
    // `play` acknowledged it as if it had been the prompt; a session that
    // really wrote it reads the refusal of a method nobody has.
    let mut session = fresh();
    read(session.as_mut(), ALLOW.lines().nth(1).unwrap());
    let lost = r#"{"jsonrpc":"2.0","id":"leon-lost","error":{"code":-32601,"message":"\"Method not found\": leon/lost","data":{"method":"leon/lost"}}}"#;
    let step = read(session.as_mut(), lost);
    assert_eq!(
        step.events,
        [TurnEvent::Finished {
            ok: false,
            error: Some(crate::acp::never_started("opencode")),
            cost_usd: None,
            duration_ms: None,
            denied_tools: vec![],
            interrupted: false,
        }]
    );
    assert!(session.idle());
}

#[test]
fn a_request_nobody_here_serves_is_refused_at_once() {
    let mut session = fresh();
    let step = read(
        session.as_mut(),
        r#"{"jsonrpc":"2.0","id":4,"method":"fs/read_text_file","params":{"sessionId":"s","path":"/etc/passwd"}}"#,
    );
    assert!(step.events.is_empty());
    assert_eq!(
        value(&step.send[0]),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "error": { "code": -32601, "message": "not supported by this client" },
        })
    );
}

#[test]
fn a_session_or_a_prompt_it_refuses_is_a_failed_turn_with_its_words() {
    let mut session = fresh();
    session.first(&TurnSpec {
        resume: Some("ses_gone"),
        ..spec("go on")
    });
    let step = read(
        session.as_mut(),
        r#"{"jsonrpc":"2.0","id":"leon-session","error":{"code":-32602,"message":"Invalid params: session not found: ses_gone","data":{"sessionId":"ses_gone"}}}"#,
    );
    assert!(matches!(
        step.events.last(),
        Some(TurnEvent::Finished { ok: false, error: Some(error), .. })
            if error == "Invalid params: session not found: ses_gone"
    ));
    assert!(step.send.is_empty() && session.idle());

    // What it said when the account behind the model had no funds.
    let mut session = fresh();
    session.first(&spec("go"));
    play(session.as_mut(), PLAIN, Some("leon-session"));
    let step = read(
        session.as_mut(),
        r#"{"jsonrpc":"2.0","id":"leon-turn","error":{"code":-32603,"message":"Internal error: Upstream request failed: Insufficient account funds","data":{"service":"session","errorName":"provider.quota"}}}"#,
    );
    assert_eq!(
        step.events,
        [TurnEvent::Finished {
            ok: false,
            error: Some("Upstream request failed: Insufficient account funds".into()),
            cost_usd: None,
            duration_ms: None,
            denied_tools: vec![],
            interrupted: false,
        }]
    );
}

#[test]
fn an_agent_that_is_not_there_is_a_failed_turn_with_the_shells_words() {
    let mut session = fresh();
    session.first(&spec("go"));
    session.noise("zsh:1: command not found: opencode");
    assert_eq!(
        session.exited(127),
        [
            TurnEvent::Finished {
                ok: false,
                error: Some("zsh:1: command not found: opencode".into()),
                cost_usd: None,
                duration_ms: None,
                denied_tools: vec![],
                interrupted: false,
            },
            TurnEvent::Exited { code: 127 },
        ]
    );
}

#[test]
fn it_says_what_it_offers_without_starting_a_conversation() {
    let options = parse_options(OPTIONS);
    assert_eq!(options.version.as_deref(), Some("2.0.24"));
    let modes: Vec<(&str, &str, bool)> = options
        .modes
        .iter()
        .map(|mode| (mode.id.as_str(), mode.label.as_str(), mode.dangerous))
        .collect();
    // Its agents a person works with, not its helpers nor its hidden ones;
    // and the one mode that is this library's.
    assert_eq!(
        modes,
        [
            ("build", "Build", false),
            ("plan", "Plan", false),
            (MODE_AUTO, "Build, never asking", true),
        ]
    );
    assert_eq!(options.default_mode.as_deref(), Some("build"));
    let models: Vec<(&str, &str)> = options
        .models
        .iter()
        .map(|model| (model.id.as_str(), model.label.as_str()))
        .collect();
    assert_eq!(
        models[..2],
        [
            ("ollama/gemma4:26b", "gemma4:26b · ollama"),
            (
                "opencode-go/claude-haiku-5-5",
                "claude-haiku-5-5 · opencode-go"
            ),
        ]
    );
    assert!(options.questions && !options.follow_ups && options.live_changes && !options.assumed);

    // Nothing answered (it is not installed): nothing is known.
    let nothing = parse_options("leon-version:\nleon-models:\n\n\nleon-agents:\n");
    assert!(nothing.assumed && nothing.modes.is_empty() && nothing.models.is_empty());
    assert_eq!(nothing.version, None);
}

fn update(update: Value) -> String {
    json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": SESSION, "update": update}})
        .to_string()
}

#[test]
fn an_edit_is_shown_as_the_lines_that_went_and_came_and_its_output_whole() {
    let mut session = fresh();
    session.first(&spec("fix it"));
    let started = update(json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "in_progress",
        "kind": "edit", "title": "edit",
        "rawInput": {"filePath": "/work/a.txt", "oldString": "one\ntwo", "newString": "one\n2"},
    }));
    // (The summary of an input without a key it knows is the input itself.)
    let events = read(session.as_mut(), &started).events;
    assert!(events.iter().any(|event| matches!(
        event,
        TurnEvent::ToolUse { id, name, detail, .. }
            if id == "call-1" && name == "edit" && detail == "/work/a.txt\n- two\n+ 2"
    )));
    // The change said again with the result is not shown twice.
    let done = update(json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "completed",
        "content": [
            {"type": "content", "content": {"type": "text", "text": "Edit applied.\n1 warning"}},
            {"type": "diff", "path": "/work/a.txt", "oldText": "one\ntwo", "newText": "one\n2"},
        ],
    }));
    assert_eq!(
        read(session.as_mut(), &done).events,
        [TurnEvent::ToolResult {
            id: "call-1".into(),
            ok: true,
            summary: "Edit applied.".into(),
            detail: "Edit applied.\n1 warning".into(),
        }]
    );
}

#[test]
fn a_change_said_as_a_diff_is_shown_when_it_is_said() {
    let mut session = fresh();
    session.first(&spec("fix it"));
    let started = update(json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "in_progress",
        "kind": "edit", "title": "patch",
        "content": [
            {"type": "diff", "path": "/work/a.txt", "oldText": "one\ntwo\nthree", "newText": "one\n2\nthree"},
            {"type": "diff", "path": "/work/new.txt", "oldText": null, "newText": "hi\n"},
        ],
    }));
    assert!(read(session.as_mut(), &started)
        .events
        .contains(&TurnEvent::ToolUse {
            id: "call-1".into(),
            name: "patch".into(),
            summary: String::new(),
            detail: "/work/a.txt\n- two\n+ 2\n\n/work/new.txt\n+ hi".into(),
        }));

    // Said only once it was made, it comes with the result.
    let started = update(json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "call-2", "status": "in_progress",
        "kind": "edit", "title": "patch",
    }));
    read(session.as_mut(), &started);
    let done = update(json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "call-2", "status": "completed",
        "content": [{"type": "diff", "path": "/work/b.txt", "oldText": "a", "newText": "b"}],
    }));
    assert_eq!(
        read(session.as_mut(), &done).events,
        [TurnEvent::ToolResult {
            id: "call-2".into(),
            ok: true,
            summary: String::new(),
            detail: "/work/b.txt\n- a\n+ b".into(),
        }]
    );
}

/// The script itself, run by a real `sh` against a stand-in.
#[cfg(unix)]
#[test]
fn the_options_script_asks_again_when_the_models_were_not_known_yet() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("opencode");
    let asked = dir.path().join("asked");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) echo v9.9.9 ;;\nmodels) if [ -e '{0}' ]; then echo fake/one; else : > '{0}'; fi ;;\ndebug) echo '[{{\"id\":\"build\",\"name\":\"Build\",\"mode\":\"primary\",\"hidden\":false}}]' ;;\nesac\n",
            asked.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", OPTIONS_SCRIPT, "leon"])
        .arg(&program)
        .output()
        .unwrap();
    let options = parse_options(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(options.version.as_deref(), Some("9.9.9"));
    assert_eq!(options.models.len(), 1);
    assert_eq!(options.models[0].id, "fake/one");
    assert_eq!(options.modes[0].id, "build");
}

#[test]
fn its_commands_are_the_three_it_always_has_until_a_session_lists_them() {
    let assumed = parse_options("leon-version:\nleon-models:\n\n\nleon-agents:\n");
    let names: Vec<&str> = assumed
        .commands
        .iter()
        .map(|command| command.insert.as_str())
        .collect();
    assert_eq!(names, ["/init", "/review", "/compact"]);

    let mut session = fresh();
    let step = read(
        session.as_mut(),
        &update(json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [
                {"name": "init", "description": "guided AGENTS.md setup"},
                {"name": "ship", "description": "", "input": {"hint": "branch"}},
            ],
        })),
    );
    let listed = step.commands.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1].insert, "/ship");
    assert_eq!(listed[1].description, None);
    assert_eq!(listed[1].hint.as_deref(), Some("branch"));
    // Anything else it says leaves the list as it was.
    let other = read(
        session.as_mut(),
        &update(json!({"sessionUpdate": "usage_update"})),
    );
    assert!(other.commands.is_none());
}

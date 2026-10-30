//! The Codex adapter against what Codex 0.160.1's application server printed
//! and read in real runs (`codex app-server`, driven over its standard
//! streams in a scratch folder), cut down to the fields that matter. The
//! ids of this client's own requests were numbers in those runs and are the
//! adapter's fixed ones here; a run with these very ids answered the same.

use super::*;
use crate::change::{ChangeKind, FileChange};
use crate::events::ChoiceAnswer;

const THREAD: &str = r#"{"id":"leon-thread","result":{"thread":{"id":"01a11902-6cec","sessionId":"01a11902-6cec","preview":"","ephemeral":false,"historyMode":"paginated","modelProvider":"openai","model":"gpt-5.6-sol","reasoningEffort":"medium","status":{"type":"idle"},"cliVersion":"0.160.1","originator":"leon-mobile","turns":[]},"model":"gpt-5.6-sol","modelProvider":"openai","serviceTier":null,"approvalPolicy":"untrusted","approvalsReviewer":"user","sandbox":{"type":"readOnly","networkAccess":false},"reasoningEffort":"medium"}}"#;
const TURN: &str = r#"{"id":"leon-turn","result":{"turn":{"id":"01a11902-6fb7","items":[],"itemsView":"notLoaded","status":"inProgress","error":null,"startedAt":null,"completedAt":null,"durationMs":null}}}"#;
const TURN_STARTED: &str = r#"{"method":"turn/started","params":{"threadId":"01a11902-6cec","turn":{"id":"01a11902-6fb7","items":[],"itemsView":"notLoaded","status":"inProgress","error":null,"startedAt":1791420952,"completedAt":null,"durationMs":null}}}"#;
const SAYING: &str = r#"{"method":"item/started","params":{"item":{"type":"agentMessage","id":"msg_02f1","text":"","phase":"commentary","memoryCitation":null,"delivery":null,"questions":null},"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","startedAtMs":1791420956484}}"#;
const DELTA: &str = r#"{"method":"item/agentMessage/delta","params":{"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","itemId":"msg_02f1","delta":"I’m creating"}}"#;
const SAID: &str = r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_02f1","text":"I’m creating the requested probe file now.","phase":"commentary","memoryCitation":null,"delivery":null,"questions":null},"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","completedAtMs":1791420956600}}"#;
const COMMAND: &str = r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"exec-b3f0","pluginId":null,"scriptPath":null,"command":"/usr/bin/bash -lc 'touch leon-probe.txt'","cwd":"/work","processId":null,"source":"agent","status":"inProgress","commandActions":[{"type":"unknown","command":"touch leon-probe.txt"}],"aggregatedOutput":null,"exitCode":null,"durationMs":null},"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","startedAtMs":1791420957915}}"#;
const ASK_COMMAND: &str = r#"{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"01a11902-6cec","turnId":"01a11902-6fb7","itemId":"exec-b3f0","startedAtMs":1791420957913,"environmentId":"local","reason":"Allow creating the requested leon-probe.txt file in the workspace?","command":"/usr/bin/bash -lc 'touch leon-probe.txt'","cwd":"/work","commandActions":[{"type":"unknown","command":"touch leon-probe.txt"}],"proposedExecpolicyAmendment":["touch","leon-probe.txt"],"availableDecisions":["accept",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["touch","leon-probe.txt"]}},"cancel"]}}"#;
const RESOLVED: &str =
    r#"{"method":"serverRequest/resolved","params":{"threadId":"01a11902-6cec","requestId":0}}"#;
const COMMAND_DONE: &str = r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"exec-b3f0","pluginId":null,"scriptPath":null,"command":"/usr/bin/bash -lc 'touch leon-probe.txt'","cwd":"/work","processId":"55296","source":"unifiedExecStartup","status":"completed","commandActions":[{"type":"unknown","command":"touch leon-probe.txt"}],"aggregatedOutput":null,"exitCode":0,"durationMs":0},"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","completedAtMs":1791420957979}}"#;
const COMMAND_DECLINED: &str = r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"exec-b3f0","pluginId":null,"scriptPath":null,"command":"/usr/bin/bash -lc 'touch leon-probe.txt'","cwd":"/work","processId":null,"source":"agent","status":"declined","commandActions":[{"type":"unknown","command":"touch leon-probe.txt"}],"aggregatedOutput":null,"exitCode":null,"durationMs":null},"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","completedAtMs":1791420990344}}"#;
const DONE: &str = r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_02f2","text":"done","phase":"final_answer","memoryCitation":null,"delivery":null,"questions":null},"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","completedAtMs":1791420959386}}"#;
const COMPLETED: &str = r#"{"method":"turn/completed","params":{"threadId":"01a11902-6cec","turn":{"id":"01a11902-6fb7","items":[{"type":"agentMessage","id":"msg_02f2","text":"done","phase":"final_answer","memoryCitation":null,"delivery":null,"questions":null}],"itemsView":"summary","status":"completed","error":null,"startedAt":1791420952,"completedAt":1791420959,"durationMs":6906}}}"#;
const INTERRUPTED: &str = r#"{"method":"turn/completed","params":{"threadId":"01a11902-6cec","turn":{"id":"01a11902-6fb7","items":[],"itemsView":"notLoaded","status":"interrupted","error":null,"startedAt":1791420984,"completedAt":1791420989,"durationMs":5688}}}"#;
const PATCH: &str = r#"{"method":"item/started","params":{"item":{"type":"fileChange","id":"exec-061b","changes":[{"path":"/work/note.txt","kind":{"type":"add"},"diff":"hi\n"}],"status":"inProgress"},"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","startedAtMs":1791420989004}}"#;
const ASK_PATCH: &str = r#"{"method":"item/fileChange/requestApproval","id":0,"params":{"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","itemId":"exec-061b","startedAtMs":1791420989012,"reason":null,"grantRoot":null}}"#;
const PATCH_DONE: &str = r#"{"method":"item/completed","params":{"item":{"type":"fileChange","id":"exec-061b","changes":[{"path":"/work/note.txt","kind":{"type":"add"},"diff":"hi\n"}],"status":"completed"},"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","completedAtMs":1791420989049}}"#;
/// What its `model/list` and `config/read` answered, two models kept.
const MODELS: &str = r#"{"id":"leon-models","result":{"data":[{"id":"gpt-6.1-sol","model":"gpt-6.1-sol","upgrade":null,"displayName":"GPT-6.1-Sol","description":"Latest workhorse model for coding and everyday work.","hidden":false,"supportedReasoningEfforts":[{"reasoningEffort":"low","description":"Fast"},{"reasoningEffort":"medium","description":"Balanced"},{"reasoningEffort":"high","description":"Deep"}],"defaultReasoningEffort":"low","isDefault":true},{"id":"gpt-6-luna","model":"gpt-6-luna","displayName":"GPT-6-Luna","description":"","hidden":false,"supportedReasoningEfforts":[],"defaultReasoningEffort":"medium","isDefault":false},{"id":"internal","model":"internal","displayName":"Internal","description":"","hidden":true,"supportedReasoningEfforts":[],"isDefault":false}],"nextCursor":null}}"#;
const CONFIG: &str = r#"{"id":"leon-config","result":{"config":{"model":"gpt-5.6-sol","approval_policy":"on-request","sandbox_mode":"workspace-write","model_reasoning_effort":"medium"}}}"#;

fn fresh() -> Box<dyn AgentSession> {
    session(100)
}

fn read(session: &mut dyn AgentSession, line: &str) -> Step {
    let mut step = Step::default();
    session.line(&serde_json::from_str(line).unwrap(), &mut step);
    step
}

fn value(outgoing: &Outgoing) -> Value {
    serde_json::from_str(&outgoing.line).unwrap()
}

fn spec<'a>(prompt: &'a str) -> TurnSpec<'a> {
    TurnSpec {
        program: "codex",
        prompt,
        ..TurnSpec::default()
    }
}

/// A session whose turn is running: greeted, the thread and the turn named.
fn running() -> Box<dyn AgentSession> {
    let mut session = fresh();
    session.first(&spec("touch the probe"));
    read(session.as_mut(), THREAD);
    read(session.as_mut(), TURN);
    session
}

fn permission(choice: &str) -> Answer {
    Answer::Permission {
        choice_id: choice.into(),
        reason: None,
    }
}

#[test]
fn a_turn_is_the_server_whatever_is_asked_of_it() {
    let turn = TurnSpec {
        program: "/opt/codex",
        prompt: "--help; rm -rf ~",
        resume: Some("t-1"),
        mode: Some("auto"),
        model: Some("gpt-6-luna"),
        effort: Some("high"),
        add_dirs: &[],
        cwd: None,
        mcp_servers: &[],
        instructions: None,
    };
    assert_eq!(argv(&turn), ["/opt/codex", "app-server"]);
}

#[test]
fn its_thread_is_given_the_servers_of_tools_and_the_instructions() {
    let mut session = fresh();
    let servers = [crate::agents::McpServer {
        name: "leon-king".into(),
        command: "/bin/leon".into(),
        args: vec!["king".into(), "mcp".into()],
        env: vec![("LEON_DATA_DIR".into(), "/data".into())],
    }];
    let turn = TurnSpec {
        mcp_servers: &servers,
        instructions: Some("You coordinate."),
        ..spec("hi")
    };
    let thread = value(&session.first(&turn)[2]);
    assert_eq!(thread["params"]["developerInstructions"], "You coordinate.");
    assert_eq!(
        thread["params"]["config"],
        json!({ "mcp_servers": { "leon-king": {
            "command": "/bin/leon",
            "args": ["king", "mcp"],
            "env": { "LEON_DATA_DIR": "/data" },
        } } })
    );
}

#[test]
fn it_is_greeted_and_given_its_thread_at_once_then_the_prompt_when_the_thread_has_an_id() {
    let mut session = fresh();
    let turn = TurnSpec {
        mode: Some("auto"),
        model: Some("gpt-6-luna"),
        effort: Some("high"),
        ..spec("say \"hi\"\nplease")
    };
    let first = session.first(&turn);
    let tags: Vec<&str> = first.iter().map(|outgoing| outgoing.tag.as_str()).collect();
    assert_eq!(tags, ["h.init", "h.ready", "h.thread"]);
    let hello = value(&first[0]);
    assert_eq!(hello["method"], "initialize");
    assert_eq!(hello["params"]["clientInfo"]["name"], "leon-mobile");
    assert_eq!(hello["params"]["capabilities"]["experimentalApi"], true);
    assert_eq!(first[1].line, r#"{"method":"initialized"}"#);
    let thread = value(&first[2]);
    assert_eq!(thread["method"], "thread/start");
    assert_eq!(
        thread["params"],
        json!({ "approvalPolicy": "on-request", "sandbox": "workspace-write", "model": "gpt-6-luna" })
    );
    assert!(first.iter().all(|outgoing| !outgoing.line.contains('\n')));

    let step = read(session.as_mut(), THREAD);
    assert_eq!(
        step.events,
        [
            TurnEvent::Started {
                session_id: "01a11902-6cec".into(),
                model: Some("gpt-5.6-sol".into()),
            },
            // Asked every time, in a sandbox that only reads: none of the
            // modes offered, so none is named.
        ]
    );
    assert_eq!(step.send.len(), 1);
    assert_eq!(step.send[0].tag, "h.turn");
    let start = value(&step.send[0]);
    assert_eq!(start["id"], "leon-turn");
    assert_eq!(start["method"], "turn/start");
    assert_eq!(
        start["params"],
        json!({
            "threadId": "01a11902-6cec",
            "input": [{ "type": "text", "text": "say \"hi\"\nplease", "text_elements": [] }],
            "effort": "high",
        })
    );
}

#[test]
fn a_conversation_is_continued_by_its_id_without_its_history() {
    let mut session = fresh();
    let turn = TurnSpec {
        resume: Some("01a11903-7af3"),
        ..spec("go on")
    };
    let thread = value(&session.first(&turn)[2]);
    assert_eq!(thread["method"], "thread/resume");
    assert_eq!(
        thread["params"],
        json!({ "threadId": "01a11903-7af3", "excludeTurns": true })
    );
}

#[test]
fn the_mode_it_runs_in_is_told_when_it_is_one_of_those_offered() {
    let mut session = fresh();
    session.first(&spec("hi"));
    let auto = THREAD
        .replace(
            r#""approvalPolicy":"untrusted""#,
            r#""approvalPolicy":"on-request""#,
        )
        .replace(r#""type":"readOnly""#, r#""type":"workspaceWrite""#);
    let step = read(session.as_mut(), &auto);
    assert_eq!(
        step.events[1],
        TurnEvent::ModeChanged {
            mode: "auto".into()
        }
    );
    // Said again, it is not told again; changed, it is.
    let settings = |policy: &str| {
        format!(
            r#"{{"method":"thread/settings/updated","params":{{"threadId":"t","threadSettings":{{"approvalPolicy":"on-request","sandboxPolicy":{{"type":"{policy}"}}}}}}}}"#
        )
    };
    assert!(read(session.as_mut(), &settings("workspaceWrite"))
        .events
        .is_empty());
    assert_eq!(
        read(session.as_mut(), &settings("readOnly")).events,
        [TurnEvent::ModeChanged {
            mode: "read-only".into()
        }]
    );
}

#[test]
fn a_command_it_may_not_run_by_itself_is_a_question_with_the_servers_own_decisions() {
    let mut session = running();
    let mut events = Vec::new();
    for line in [TURN_STARTED, SAYING, DELTA, SAID, COMMAND, ASK_COMMAND] {
        let step = read(session.as_mut(), line);
        assert!(step.send.is_empty());
        events.extend(step.events);
    }
    assert_eq!(
        events[..3],
        [
            TurnEvent::TextDelta {
                text: "I’m creating".into()
            },
            TurnEvent::Text {
                text: "I’m creating the requested probe file now.".into()
            },
            TurnEvent::ToolUse {
                id: "exec-b3f0".into(),
                name: "shell".into(),
                summary: "touch leon-probe.txt".into(),
                detail: String::new(),
            },
        ]
    );
    let TurnEvent::Question { id, kind } = &events[3] else {
        panic!("{events:?}");
    };
    assert_eq!(id, "0");
    let QuestionKind::Permission {
        tool,
        summary,
        detail,
        reason,
        choices,
    } = kind
    else {
        panic!("{kind:?}");
    };
    assert_eq!(
        (tool.as_str(), summary.as_str()),
        ("shell", "touch leon-probe.txt")
    );
    assert!(
        detail.contains("/usr/bin/bash -lc 'touch leon-probe.txt'") && !detail.contains("turnId")
    );
    assert_eq!(
        reason.as_deref(),
        Some("Allow creating the requested leon-probe.txt file in the workspace?")
    );
    let shown: Vec<(&str, PermissionEffect, &str)> = choices
        .iter()
        .map(|choice| (choice.id.as_str(), choice.effect, choice.label.as_str()))
        .collect();
    assert_eq!(
        shown,
        [
            ("d.0", PermissionEffect::AllowOnce, "Allow once"),
            (
                "d.1",
                PermissionEffect::AllowAlways,
                "Always allow commands starting with touch leon-probe.txt"
            ),
            ("d.2", PermissionEffect::Deny, "Deny, and stop the turn"),
        ]
    );
    // Its answers take no reason: what to do instead is the next message.
    assert!(choices.iter().all(|choice| !choice.takes_reason));
    assert!(!session.idle());
}

#[test]
fn a_decision_goes_back_as_the_server_wrote_it_under_the_number_it_asked_with() {
    let mut session = running();
    read(session.as_mut(), ASK_COMMAND);
    let answered = |session: &mut dyn AgentSession, choice: &str| {
        let outgoing = session
            .encode(Action::Answer("0".into(), permission(choice)))
            .unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].tag, "a.0");
        value(&outgoing[0])
    };
    assert_eq!(
        answered(session.as_mut(), "d.0"),
        json!({ "id": 0, "result": { "decision": "accept" } })
    );
    assert_eq!(
        answered(session.as_mut(), "d.1"),
        json!({ "id": 0, "result": { "decision": {
            "acceptWithExecpolicyAmendment": { "execpolicy_amendment": ["touch", "leon-probe.txt"] }
        } } })
    );
    assert_eq!(
        answered(session.as_mut(), "d.2"),
        json!({ "id": 0, "result": { "decision": "cancel" } })
    );
    // A refusal by any other name is the refusal it offers.
    assert_eq!(
        answered(session.as_mut(), "deny"),
        json!({ "id": 0, "result": { "decision": "cancel" } })
    );
    for wrong in ["d.3", "d.x", "always.0"] {
        assert_eq!(
            session
                .encode(Action::Answer("0".into(), permission(wrong)))
                .map(|_| ()),
            Err(TurnError::BadAnswer)
        );
    }
    assert_eq!(
        session
            .encode(Action::Answer("7".into(), permission("d.0")))
            .map(|_| ()),
        Err(TurnError::UnknownQuestion)
    );
    assert_eq!(
        session
            .encode(Action::Answer(
                "0".into(),
                Answer::Plan {
                    approve: true,
                    feedback: None
                }
            ))
            .map(|_| ()),
        Err(TurnError::BadAnswer)
    );
}

#[test]
fn a_question_is_closed_by_its_answer_arriving_by_the_server_and_by_the_turn_ending() {
    // Its answer reached the server.
    let mut session = running();
    read(session.as_mut(), ASK_COMMAND);
    let mut step = Step::default();
    session.delivered("a.0", &mut step);
    assert_eq!(
        step.events,
        [TurnEvent::QuestionResolved { id: "0".into() }]
    );
    // The server says so again: once is enough.
    assert!(read(session.as_mut(), RESOLVED).events.is_empty());

    // The server withdrew it.
    let mut session = running();
    read(session.as_mut(), ASK_COMMAND);
    assert_eq!(
        read(session.as_mut(), RESOLVED).events,
        [TurnEvent::QuestionResolved { id: "0".into() }]
    );

    // The turn ended under it.
    let mut session = running();
    read(session.as_mut(), ASK_COMMAND);
    let ended = read(session.as_mut(), INTERRUPTED).events;
    assert_eq!(ended[0], TurnEvent::QuestionResolved { id: "0".into() });
    assert!(session.idle());
}

#[test]
fn a_whole_turn_reads_as_words_a_tool_and_an_end() {
    let mut session = running();
    let mut events = Vec::new();
    for line in [
        TURN_STARTED,
        COMMAND,
        ASK_COMMAND,
        RESOLVED,
        COMMAND_DONE,
        DONE,
    ] {
        events.extend(read(session.as_mut(), line).events);
    }
    assert!(!session.idle());
    events.extend(read(session.as_mut(), COMPLETED).events);
    assert_eq!(
        events[3..],
        [
            TurnEvent::ToolResult {
                id: "exec-b3f0".into(),
                ok: true,
                summary: String::new(),
                detail: String::new(),
            },
            TurnEvent::Text {
                text: "done".into()
            },
            TurnEvent::Finished {
                ok: true,
                error: None,
                cost_usd: None,
                duration_ms: Some(6906),
                denied_tools: Vec::new(),
                interrupted: false,
            },
        ]
    );
    // It waits for more for ever: its input is to be closed.
    assert!(session.idle());
    assert_eq!(session.exited(0), [TurnEvent::Exited { code: 0 }]);
}

#[test]
fn a_command_refused_is_a_tool_that_did_not_run() {
    let mut session = running();
    read(session.as_mut(), COMMAND);
    assert_eq!(
        read(session.as_mut(), COMMAND_DECLINED).events,
        [TurnEvent::ToolResult {
            id: "exec-b3f0".into(),
            ok: false,
            summary: "declined".into(),
            detail: String::new(),
        }]
    );
}

#[test]
fn a_command_and_what_it_printed_are_kept_whole() {
    let mut session = running();
    let started = json!({"method": "item/started", "params": {"item": {
        "type": "commandExecution", "id": "exec-1", "status": "inProgress",
        "command": "/usr/bin/bash -lc 'cargo test \\\n  --all'",
        "commandActions": [{"type": "unknown", "command": "cargo test \\\n  --all"}],
    }}});
    assert_eq!(
        read(session.as_mut(), &started.to_string()).events,
        [TurnEvent::ToolUse {
            id: "exec-1".into(),
            name: "shell".into(),
            summary: "cargo test \\".into(),
            detail: "cargo test \\\n  --all".into(),
        }]
    );
    let done = json!({"method": "item/completed", "params": {"item": {
        "type": "commandExecution", "id": "exec-1", "status": "failed", "exitCode": 101,
        "command": "/usr/bin/bash -lc 'cargo test \\\n  --all'",
        "commandActions": [{"type": "unknown", "command": "cargo test \\\n  --all"}],
        "aggregatedOutput": "running 2 tests\ntest a ... FAILED\nerror: test failed\n",
    }}});
    assert_eq!(
        read(session.as_mut(), &done.to_string()).events,
        [TurnEvent::ToolResult {
            id: "exec-1".into(),
            ok: false,
            summary: "error: test failed".into(),
            detail: "running 2 tests\ntest a ... FAILED\nerror: test failed".into(),
        }]
    );
}

#[test]
fn a_change_of_files_is_shown_as_the_lines_that_went_and_came() {
    let mut session = running();
    let started = json!({"method": "item/started", "params": {"item": {
        "type": "fileChange", "id": "patch-1", "status": "inProgress",
        "changes": [
            {
                "path": "/work/a.rs",
                "kind": {"type": "update", "move_path": null},
                "diff": "@@ -1,3 +1,3 @@\n fn a() {\n-    one();\n+    two();\n }\n",
            },
            {"path": "/work/old.txt", "kind": {"type": "delete"}, "diff": "bye\n"},
        ],
    }}});
    let events = read(session.as_mut(), &started.to_string()).events;
    assert_eq!(
        events,
        [
            TurnEvent::ToolUse {
                id: "patch-1".into(),
                name: "edit".into(),
                summary: "/work/a.rs, /work/old.txt".into(),
                detail: concat!(
                    "/work/a.rs\n@@ -1,3 +1,3 @@\n  fn a() {\n-     one();\n+     two();\n  }\n\n",
                    "/work/old.txt\n- bye"
                )
                .into(),
            },
            TurnEvent::ToolChange {
                id: "patch-1".into(),
                changes: vec![
                    FileChange::from_diff(
                        "/work/a.rs",
                        ChangeKind::Updated,
                        "@@ -1,3 +1,3 @@\n fn a() {\n-    one();\n+    two();\n }\n",
                    ),
                    FileChange::from_diff("/work/old.txt", ChangeKind::Deleted, "bye\n"),
                ],
            }
        ]
    );
    // The lines carry the numbers its header gave.
    let TurnEvent::ToolChange { changes, .. } = &events[1] else {
        panic!()
    };
    assert_eq!(changes[0].hunks[0].lines[1].old, Some(2));
    assert_eq!(
        (changes[1].kind, changes[1].removed()),
        (ChangeKind::Deleted, 1)
    );
}

#[test]
fn what_a_tool_of_a_server_answered_is_its_result() {
    let mut session = running();
    let done = json!({"method": "item/completed", "params": {"item": {
        "type": "mcpToolCall", "id": "mcp-1", "status": "completed",
        "server": "docs", "tool": "search",
        "arguments": {"query": "uniffi", "limit": 3},
        "result": {"content": [{"type": "text", "text": "one\ntwo"}]},
        "error": null,
    }}});
    assert_eq!(
        read(session.as_mut(), &done.to_string()).events,
        [
            TurnEvent::ToolUse {
                id: "mcp-1".into(),
                name: "docs.search".into(),
                summary: "uniffi".into(),
                detail: "query: uniffi\nlimit: 3".into(),
            },
            TurnEvent::ToolResult {
                id: "mcp-1".into(),
                ok: true,
                summary: String::new(),
                detail: "one\ntwo".into(),
            },
        ]
    );
}

#[test]
fn a_change_of_files_is_asked_with_the_change_and_every_decision_the_protocol_has() {
    let mut session = running();
    assert_eq!(
        read(session.as_mut(), PATCH).events,
        [
            TurnEvent::ToolUse {
                id: "exec-061b".into(),
                name: "edit".into(),
                summary: "/work/note.txt".into(),
                detail: "/work/note.txt\n+ hi".into(),
            },
            TurnEvent::ToolChange {
                id: "exec-061b".into(),
                changes: vec![FileChange::written("/work/note.txt", "hi\n")],
            }
        ]
    );
    let asked = read(session.as_mut(), ASK_PATCH).events;
    let [TurnEvent::Question {
        id,
        kind:
            QuestionKind::Permission {
                tool,
                summary,
                detail,
                reason,
                choices,
            },
    }] = &asked[..]
    else {
        panic!("{asked:?}");
    };
    assert_eq!((id.as_str(), tool.as_str()), ("0", "edit"));
    assert_eq!(summary, "/work/note.txt");
    assert_eq!(detail, "add /work/note.txt\nhi");
    assert_eq!(reason, &None);
    let shown: Vec<(&str, PermissionEffect)> = choices
        .iter()
        .map(|choice| (choice.label.as_str(), choice.effect))
        .collect();
    assert_eq!(
        shown,
        [
            ("Allow once", PermissionEffect::AllowOnce),
            ("Allow for this session", PermissionEffect::AllowAlways),
            ("Deny", PermissionEffect::Deny),
            ("Deny, and stop the turn", PermissionEffect::Deny),
        ]
    );
    let outgoing = session
        .encode(Action::Answer("0".into(), permission("deny")))
        .unwrap();
    assert_eq!(
        value(&outgoing[0]),
        json!({ "id": 0, "result": { "decision": "decline" } })
    );
    assert_eq!(
        read(session.as_mut(), PATCH_DONE).events,
        [TurnEvent::ToolResult {
            id: "exec-061b".into(),
            ok: true,
            summary: String::new(),
            detail: String::new(),
        }]
    );
}

#[test]
fn stopping_it_is_asked_of_the_turn_and_is_not_a_failure() {
    let mut session = running();
    read(session.as_mut(), COMMAND);
    let outgoing = session.encode(Action::Interrupt).unwrap();
    assert_eq!(outgoing[0].tag, "i.leon-stop-101");
    assert_eq!(
        value(&outgoing[0]),
        json!({
            "id": "leon-stop-101",
            "method": "turn/interrupt",
            "params": { "threadId": "01a11902-6cec", "turnId": "01a11902-6fb7" },
        })
    );
    assert!(
        read(session.as_mut(), r#"{"id":"leon-stop-101","result":{}}"#)
            .events
            .is_empty()
    );
    assert_eq!(
        read(session.as_mut(), INTERRUPTED).events,
        [
            // What it was doing is not left running on the screen.
            TurnEvent::ToolResult {
                id: "exec-b3f0".into(),
                ok: false,
                summary: "stopped".into(),
                detail: String::new(),
            },
            TurnEvent::Finished {
                ok: false,
                error: None,
                cost_usd: None,
                duration_ms: Some(5688),
                denied_tools: Vec::new(),
                interrupted: true,
            },
        ]
    );
    assert_eq!(
        session.encode(Action::Interrupt).map(|_| ()),
        Err(TurnError::TurnEnded)
    );
}

#[test]
fn what_is_asked_before_the_turn_has_an_id_is_written_when_it_has_one() {
    let mut session = fresh();
    session.first(&spec("hi"));
    read(session.as_mut(), THREAD);
    assert!(session.encode(Action::Interrupt).unwrap().is_empty());
    assert!(session
        .encode(Action::Prompt("and be quick".into()))
        .unwrap()
        .is_empty());
    let step = read(session.as_mut(), TURN);
    let methods: Vec<String> = step
        .send
        .iter()
        .map(|outgoing| text_of(&value(outgoing)["method"]))
        .collect();
    assert_eq!(methods, ["turn/interrupt", "turn/steer"]);
    // Named again by its notification, nothing is written again.
    assert!(read(session.as_mut(), TURN_STARTED).send.is_empty());
}

#[test]
fn saying_more_steers_the_turn_in_flight() {
    let mut session = running();
    let outgoing = session
        .encode(Action::Prompt("Also say banana.".into()))
        .unwrap();
    assert_eq!(outgoing[0].tag, "u.leon-say-101");
    assert_eq!(
        value(&outgoing[0]),
        json!({
            "id": "leon-say-101",
            "method": "turn/steer",
            "params": {
                "threadId": "01a11902-6cec",
                "expectedTurnId": "01a11902-6fb7",
                "input": [{ "type": "text", "text": "Also say banana.", "text_elements": [] }],
            },
        })
    );
    // A turn that ended meanwhile refuses it: no failure of the turn.
    let refused =
        r#"{"id":"leon-say-101","error":{"code":-32600,"message":"no active turn to steer"}}"#;
    assert!(read(session.as_mut(), refused).events.is_empty());
    read(session.as_mut(), COMPLETED);
    assert_eq!(
        session
            .encode(Action::Prompt("too late".into()))
            .map(|_| ()),
        Err(TurnError::TurnEnded)
    );
}

#[test]
fn its_mode_and_model_cannot_be_changed_while_it_works() {
    let mut session = running();
    for action in [
        Action::SetMode("auto".into()),
        Action::SetModel("gpt-6-luna".into()),
    ] {
        assert_eq!(
            session.encode(action).map(|_| ()),
            Err(TurnError::NotSupported)
        );
    }
}

#[test]
fn a_turn_found_again_is_read_from_its_start_without_knowing_the_prompt() {
    // A process that was never told the prompt reads the same output.
    let mut session = fresh();
    let step = read(session.as_mut(), THREAD);
    // In place of `turn/start` it would write a harmless question, under
    // the same tag: the engine drops it, that tag being acknowledged.
    assert_eq!(step.send[0].tag, "h.turn");
    assert_eq!(value(&step.send[0])["method"], "thread/loaded/list");
    let mut events = step.events;
    for line in [TURN, TURN_STARTED, COMMAND, ASK_COMMAND] {
        let step = read(session.as_mut(), line);
        assert!(step.send.is_empty(), "{:?}", step.send);
        events.extend(step.events);
    }
    assert!(matches!(events.last(), Some(TurnEvent::Question { id, .. }) if id == "0"));
    // The question is open, and answered under the number it was asked with.
    let outgoing = session
        .encode(Action::Answer("0".into(), permission("d.0")))
        .unwrap();
    assert_eq!(outgoing[0].tag, "a.0");
    // It can be stopped: the replay named the thread and the turn.
    let stop = session.encode(Action::Interrupt).unwrap();
    assert_eq!(
        value(&stop[0])["params"],
        json!({ "threadId": "01a11902-6cec", "turnId": "01a11902-6fb7" })
    );
}

#[test]
fn a_turn_nobody_started_says_so_and_ends() {
    let mut session = fresh();
    read(session.as_mut(), THREAD);
    // The harmless question was written, so `turn/start` never was.
    let step = read(
        session.as_mut(),
        r#"{"id":"leon-lost","result":{"data":["01a11902-6cec"]}}"#,
    );
    assert!(
        matches!(
            &step.events[..],
            [TurnEvent::Finished { ok: false, error: Some(error), interrupted: false, .. }]
                if error.contains("Send it again")
        ),
        "{:?}",
        step.events
    );
    assert!(session.idle());
    assert_eq!(session.exited(0), [TurnEvent::Exited { code: 0 }]);
}

#[test]
fn more_permissions_are_granted_as_asked_for_the_turn_or_the_session_or_not() {
    // The shape of `item/permissions/requestApproval` in the protocol of
    // 0.160.1; no run raised one.
    const ASK: &str = r#"{"method":"item/permissions/requestApproval","id":4,"params":{"threadId":"t","turnId":"u","itemId":"i","environmentId":"local","startedAtMs":1,"cwd":"/work","reason":"It needs to fetch a package","permissions":{"network":{"enabled":true},"fileSystem":null}}}"#;
    let mut session = running();
    let asked = read(session.as_mut(), ASK).events;
    let [TurnEvent::Question {
        id,
        kind:
            QuestionKind::Permission {
                tool,
                summary,
                choices,
                ..
            },
    }] = &asked[..]
    else {
        panic!("{asked:?}");
    };
    assert_eq!(
        (id.as_str(), tool.as_str(), summary.as_str()),
        ("4", "permissions", "the network")
    );
    let ids: Vec<&str> = choices.iter().map(|choice| choice.id.as_str()).collect();
    assert_eq!(ids, ["turn", "session", "deny"]);
    let answered = |session: &mut dyn AgentSession, choice: &str| {
        value(
            &session
                .encode(Action::Answer("4".into(), permission(choice)))
                .unwrap()[0],
        )
    };
    assert_eq!(
        answered(session.as_mut(), "session"),
        json!({ "id": 4, "result": { "permissions": { "network": { "enabled": true } }, "scope": "session" } })
    );
    assert_eq!(
        answered(session.as_mut(), "deny"),
        json!({ "id": 4, "result": { "permissions": {}, "scope": "turn" } })
    );
}

#[test]
fn its_own_questions_are_answered_by_question() {
    // The shape of `item/tool/requestUserInput` in the protocol of 0.160.1
    // (experimental); no run raised one.
    const ASK: &str = r#"{"method":"item/tool/requestUserInput","id":"req-9","params":{"threadId":"t","turnId":"u","itemId":"i","isBlocking":true,"autoResolutionMs":null,"questions":[{"id":"color","header":"Color","question":"Which color?","isOther":true,"isSecret":false,"options":[{"label":"Red","description":"Warm"},{"label":"Blue","description":""}]},{"id":"name","header":"","question":"What is it called?","isOther":false,"isSecret":false,"options":null}]}}"#;
    let mut session = running();
    let asked = read(session.as_mut(), ASK).events;
    let [TurnEvent::Question {
        id,
        kind: QuestionKind::Choices { questions },
    }] = &asked[..]
    else {
        panic!("{asked:?}");
    };
    assert_eq!(id, "req-9");
    assert_eq!(questions[0].id, "color");
    assert_eq!(questions[0].header.as_deref(), Some("Color"));
    assert_eq!(questions[0].options[0].description.as_deref(), Some("Warm"));
    assert_eq!(questions[0].options[1].description, None);
    assert!(questions[0].allow_other && !questions[0].multi_select);
    // A question without options can only be answered in one's own words.
    assert!(questions[1].options.is_empty() && questions[1].allow_other);
    let outgoing = session
        .encode(Action::Answer(
            "req-9".into(),
            Answer::Choices {
                answers: vec![
                    ChoiceAnswer {
                        question_id: "color".into(),
                        labels: vec!["Blue".into()],
                        other: None,
                    },
                    ChoiceAnswer {
                        question_id: "name".into(),
                        labels: Vec::new(),
                        other: Some(" probe ".into()),
                    },
                ],
            },
        ))
        .unwrap();
    assert_eq!(outgoing[0].tag, "a.req-9");
    assert_eq!(
        value(&outgoing[0]),
        json!({ "id": "req-9", "result": { "answers": {
            "color": { "answers": ["Blue"] },
            "name": { "answers": ["probe"] },
        } } })
    );
}

#[test]
fn a_tool_servers_question_can_be_agreed_to_only_when_there_is_nothing_to_fill_in() {
    // The shapes of `mcpServer/elicitation/request` in the protocol of
    // 0.160.1; no run raised one.
    let ask = |rest: &str| {
        format!(
            r#"{{"method":"mcpServer/elicitation/request","id":5,"params":{{"threadId":"t","turnId":null,"serverName":"tracker","_meta":null,"message":"Sign in to the tracker",{rest}}}}}"#
        )
    };
    let ids = |events: &[TurnEvent]| -> Vec<String> {
        let [TurnEvent::Question {
            kind: QuestionKind::Permission { tool, choices, .. },
            ..
        }] = events
        else {
            panic!("{events:?}");
        };
        assert_eq!(tool, "mcp:tracker");
        choices.iter().map(|choice| choice.id.clone()).collect()
    };
    let mut session = running();
    let page = read(
        session.as_mut(),
        &ask(r#""mode":"url","url":"https://tracker.example/login","elicitationId":"e1""#),
    );
    assert_eq!(ids(&page.events), ["accept", "deny"]);
    let form = read(
        session.as_mut(),
        &ask(
            r#""mode":"form","requestedSchema":{"type":"object","properties":{"token":{"type":"string"}},"required":["token"]}"#,
        ),
    );
    assert_eq!(ids(&form.events), ["deny"]);
    let outgoing = session
        .encode(Action::Answer("5".into(), permission("deny")))
        .unwrap();
    assert_eq!(
        value(&outgoing[0]),
        json!({ "id": 5, "result": { "action": "decline", "content": null, "_meta": null } })
    );
}

#[test]
fn a_request_nobody_here_serves_is_refused_at_once() {
    let mut session = running();
    let step = read(
        session.as_mut(),
        r#"{"method":"item/tool/call","id":9,"params":{"threadId":"t","turnId":"u","callId":"c","tool":"x","arguments":{}}}"#,
    );
    assert!(step.events.is_empty());
    assert_eq!(step.send[0].tag, "r.9");
    assert_eq!(
        value(&step.send[0]),
        json!({ "id": 9, "error": { "code": -32601, "message": "not supported by this client" } })
    );
}

#[test]
fn a_thread_or_a_turn_it_refuses_is_a_failed_turn_with_its_words() {
    let mut session = fresh();
    session.first(&spec("hi"));
    let step = read(
        session.as_mut(),
        r#"{"id":"leon-thread","error":{"code":-32600,"message":"no rollout found for thread id t-9"}}"#,
    );
    assert_eq!(
        step.events,
        [TurnEvent::Finished {
            ok: false,
            error: Some("no rollout found for thread id t-9".into()),
            cost_usd: None,
            duration_ms: None,
            denied_tools: Vec::new(),
            interrupted: false,
        }]
    );
    assert!(step.send.is_empty() && session.idle());

    let mut session = running();
    let failed = COMPLETED
        .replace(r#""status":"completed""#, r#""status":"failed""#)
        .replace(
            r#""error":null"#,
            r#""error":{"message":"You've hit your usage limit.","codexErrorInfo":"usageLimitExceeded","additionalDetails":null}"#,
        );
    assert!(matches!(
        &read(session.as_mut(), &failed).events[..],
        [TurnEvent::Finished { ok: false, error: Some(error), interrupted: false, .. }]
            if error == "You've hit your usage limit."
    ));
}

#[test]
fn an_agent_that_is_not_there_is_a_failed_turn_with_the_shells_words() {
    let mut session = fresh();
    session.first(&spec("hi"));
    session.noise("zsh:1: command not found: codex");
    assert_eq!(
        session.exited(127),
        [
            TurnEvent::Finished {
                ok: false,
                error: Some("zsh:1: command not found: codex".into()),
                cost_usd: None,
                duration_ms: None,
                denied_tools: Vec::new(),
                interrupted: false,
            },
            TurnEvent::Exited { code: 127 },
        ]
    );
    // Ended before any turn began, even cleanly, it answered nothing.
    let mut session = fresh();
    session.first(&spec("hi"));
    assert!(matches!(
        &session.exited(0)[0],
        TurnEvent::Finished { ok: false, .. }
    ));
}

#[test]
fn its_end_closes_what_was_still_asked() {
    let mut session = running();
    read(session.as_mut(), ASK_COMMAND);
    let events = session.exited(1);
    assert_eq!(events[0], TurnEvent::QuestionResolved { id: "0".into() });
    assert!(matches!(events[1], TurnEvent::Finished { ok: false, .. }));
    assert_eq!(events[2], TurnEvent::Exited { code: 1 });
}

#[test]
fn what_it_offers_is_its_servers_models_and_the_modes_of_the_protocol() {
    let text = format!(
        "mise ~/.config/mise/config.toml tools: codex@0.160.1\r\nleon-version:codex-cli 0.160.1\r\n{}\r\n{MODELS}\r\n{CONFIG}\r\n",
        r#"{"id":"leon-init","result":{"userAgent":"leon-mobile/0.160.1","platformFamily":"unix","platformOs":"linux"}}"#
    );
    let options = parse_options(&text);
    assert_eq!(options.version.as_deref(), Some("codex-cli 0.160.1"));
    let modes: Vec<(&str, bool)> = options
        .modes
        .iter()
        .map(|mode| (mode.id.as_str(), mode.dangerous))
        .collect();
    assert_eq!(
        modes,
        [
            ("read-only", false),
            ("auto", false),
            ("untrusted", false),
            ("full-access", true),
        ]
    );
    assert_eq!(options.default_mode.as_deref(), Some("auto"));
    // What its configuration names is what a turn runs with.
    assert_eq!(options.default_model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(options.default_effort.as_deref(), Some("medium"));
    // A configuration that names neither: its own choice, and how hard
    // that one thinks unasked.
    let unset = parse_options(&format!(
        "{MODELS}\n{}\n",
        r#"{"id":"leon-config","result":{"config":{"approval_policy":"on-request","sandbox_mode":"workspace-write"}}}"#
    ));
    assert_eq!(unset.default_model.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(unset.default_effort.as_deref(), Some("low"));
    let models: Vec<(&str, &str, Vec<&str>)> = options
        .models
        .iter()
        .map(|model| {
            (
                model.id.as_str(),
                model.label.as_str(),
                model.efforts.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        models,
        [
            ("gpt-6.1-sol", "GPT-6.1-Sol", vec!["low", "medium", "high"]),
            ("gpt-6-luna", "GPT-6-Luna", vec![]),
        ]
    );
    assert_eq!(options.models[1].description, None);
    assert!(options.questions && options.follow_ups && !options.live_changes && !options.assumed);
}

#[test]
fn a_server_that_does_not_answer_leaves_the_modes_and_says_the_rest_is_assumed() {
    let options = parse_options("leon-version:\nzsh: command not found: codex\n");
    assert_eq!(options.version, None);
    assert_eq!(options.modes.len(), 4);
    assert!(options.models.is_empty() && options.assumed);
    assert_eq!(options.default_mode, None);
    // Every mode is a pair the protocol knows, and reads back as itself.
    for mode in MODES {
        let (approval, sandbox) = mode_parts(mode.0).unwrap();
        assert_eq!(mode_named(&json!(approval), &json!(sandbox)), Some(mode.0));
    }
    assert_eq!(mode_parts("plan"), None);
}

#[test]
fn the_script_waits_for_the_answers_to_the_questions_it_is_given() {
    let arguments = options_arguments();
    for id in [
        r#""id":"leon-models""#,
        r#""id":"leon-config""#,
        r#""id":"leon-skills""#,
    ] {
        assert!(arguments.iter().any(|line| line.contains(id)), "{id}");
        assert!(OPTIONS_SCRIPT.contains(id), "{id}");
    }
    assert!(arguments.iter().all(|line| !line.contains('\n')));
}

/// The script itself, run by a real `sh` against a stand-in that answers
/// the second question first and then waits for ever, as the server does.
#[cfg(unix)]
#[test]
fn the_script_reads_answers_in_any_order_and_then_lets_the_server_go() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("leon-codex-options-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("codex");
    std::fs::write(
        &program,
        r#"#!/bin/sh
case "$1" in --version) echo 'codex-cli 1.2.3'; exit 0 ;; esac
while IFS= read -r line; do
    case "$line" in
    *'"method":"config/read"'*)
        printf '{"id":"leon-config","result":{"config":{"approval_policy":"never","sandbox_mode":"danger-full-access"}}}\n'
        printf '{"id":"leon-models","result":{"data":[{"id":"m","model":"m","displayName":"M","hidden":false,"supportedReasoningEfforts":[]}]}}\n' ;;
    # A server from before skills answers that it does not know the question.
    *'"method":"skills/list"'*)
        printf '{"id":"leon-skills","error":{"code":-32601,"message":"Method not found"}}\n' ;;
    esac
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", OPTIONS_SCRIPT, "leon"])
        .arg(&program)
        .args(options_arguments())
        .env("TMPDIR", &dir)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let options = parse_options(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(options.version.as_deref(), Some("codex-cli 1.2.3"));
    assert_eq!(options.default_mode.as_deref(), Some("full-access"));
    assert_eq!(options.models.len(), 1);
    assert!(!options.assumed);
    // Its pipe is gone with it.
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name() != "codex")
        .collect();
    assert!(left.is_empty(), "{left:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// What its `skills/list` answered, two skills kept and one turned off.
const SKILLS: &str = r#"{"id":"leon-skills","result":{"data":[{"cwd":"/work","errors":[],"skills":[{"name":"diagnose-crash","description":"Diagnose why a program crashed on this machine, from a core dump.","shortDescription":"Why a program crashed","enabled":true,"path":"/home/me/.codex/skills/diagnose-crash/SKILL.md","scope":"user"},{"name":"humanizer","description":"Rewrite text so it reads like the writer.","shortDescription":null,"enabled":true,"path":"/home/me/.codex/skills/humanizer/SKILL.md","scope":"user"},{"name":"off","description":"Not used.","enabled":false,"path":"/home/me/.codex/skills/off/SKILL.md","scope":"user"}]}]}}"#;

#[test]
fn it_lists_what_the_server_does_itself_then_its_skills_by_their_dollar_names() {
    let options = parse_options(&format!("leon-version:codex 0.160.1\n{SKILLS}\n"));
    let listed: Vec<(&str, &str)> = options
        .commands
        .iter()
        .map(|command| (command.name.as_str(), command.insert.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            ("compact", "/compact"),
            ("review", "/review"),
            ("diagnose-crash", "$diagnose-crash"),
            ("humanizer", "$humanizer"),
        ]
    );
    assert_eq!(
        options.commands[2].description.as_deref(),
        Some("Why a program crashed")
    );
    assert_eq!(
        options.commands[3].description.as_deref(),
        Some("Rewrite text so it reads like the writer.")
    );
    // A server that could not be asked still has what it does itself.
    let alone = parse_options("leon-version:\n");
    assert_eq!(alone.commands.len(), 2);
}

#[test]
fn compacting_and_reviewing_are_the_servers_own_requests_and_anything_else_is_a_turn() {
    let asked = |prompt: &str| {
        let mut session = fresh();
        session.first(&TurnSpec {
            resume: Some("01a11902-6cec"),
            ..spec(prompt)
        });
        let step = read(session.as_mut(), THREAD);
        value(&step.send[0])
    };
    let compact = asked("/compact");
    assert_eq!(compact["method"], "thread/compact/start");
    assert_eq!(compact["params"], json!({ "threadId": "01a11902-6cec" }));

    let review = asked("/review");
    assert_eq!(review["method"], "review/start");
    assert_eq!(
        review["params"]["target"],
        json!({ "type": "uncommittedChanges" })
    );
    let about = asked("/review  the retry loop ");
    assert_eq!(
        about["params"]["target"],
        json!({ "type": "custom", "instructions": "the retry loop" })
    );

    // A path, or a word that only starts like one, is said to the model.
    for prompt in [
        "/compact the notes",
        "/reviewer",
        "look at /review",
        "$humanizer this",
    ] {
        let turn = asked(prompt);
        assert_eq!(turn["method"], "turn/start", "{prompt}");
        assert_eq!(turn["params"]["input"][0]["text"], prompt);
    }
}

#[test]
fn a_compaction_has_no_turn_in_its_answer_and_ends_like_any_turn() {
    let mut session = fresh();
    session.first(&TurnSpec {
        resume: Some("01a11902-6cec"),
        ..spec("/compact")
    });
    read(session.as_mut(), THREAD);
    let answered = read(session.as_mut(), r#"{"id":"leon-turn","result":{}}"#);
    assert!(answered.events.is_empty());
    read(session.as_mut(), TURN_STARTED);
    let done = read(session.as_mut(), COMPLETED);
    assert!(done
        .events
        .iter()
        .any(|event| matches!(event, TurnEvent::Finished { ok: true, .. })));
    assert!(session.idle());
}

#[test]
fn the_turns_tokens_are_what_the_thread_added_since_it_began() {
    let mut session = fresh();
    session.first(&spec("hi"));
    let update = |total: (u64, u64, u64, u64), last: (u64, u64, u64, u64)| {
        let breakdown = |(input, cached, output, reasoning): (u64, u64, u64, u64)| {
            json!({
                "inputTokens": input, "cachedInputTokens": cached,
                "outputTokens": output, "reasoningOutputTokens": reasoning,
                "totalTokens": input + output,
            })
        };
        json!({
            "method": "thread/tokenUsage/updated",
            "params": { "threadId": "t", "turnId": "u", "tokenUsage": {
                "total": breakdown(total), "last": breakdown(last), "modelContextWindow": 400000,
            } },
        })
        .to_string()
    };
    // A thread resumed with 1000 in and 100 out from before.
    let first = read(
        session.as_mut(),
        &update((1500, 800, 150, 20), (500, 300, 50, 20)),
    );
    assert_eq!(
        first.events,
        [TurnEvent::Usage {
            tokens: Tokens {
                input: 500,
                cached_input: 300,
                output: 50,
                reasoning: 20,
                context_window: Some(400_000),
                context_used: Some(550),
            }
        }]
    );
    let second = read(
        session.as_mut(),
        &update((2100, 1200, 210, 30), (600, 400, 60, 10)),
    );
    let [TurnEvent::Usage { tokens }] = &second.events[..] else {
        panic!("{:?}", second.events);
    };
    assert_eq!(
        (tokens.input, tokens.output, tokens.reasoning),
        (1100, 110, 30)
    );
}

#[test]
fn a_plan_update_is_the_agent_s_list_of_todos() {
    let mut session = running();
    let update = r#"{"method":"turn/plan/updated","params":{"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","explanation":null,"plan":[{"step":"Read the code","status":"completed"},{"step":"Write the test","status":"inProgress"},{"step":"Ship it","status":"pending"}]}}"#;
    let step = read(session.as_mut(), update);
    assert_eq!(
        step.events,
        [TurnEvent::Todos {
            todos: vec![
                crate::events::Todo {
                    text: "Read the code".into(),
                    status: crate::events::TodoStatus::Completed
                },
                crate::events::Todo {
                    text: "Write the test".into(),
                    status: crate::events::TodoStatus::InProgress
                },
                crate::events::Todo {
                    text: "Ship it".into(),
                    status: crate::events::TodoStatus::Pending
                },
            ]
        }]
    );
}

#[test]
fn a_plan_item_of_checkboxes_is_a_list_and_one_of_prose_is_an_answer() {
    let item = |text: &str| {
        serde_json::json!({"method":"item/completed","params":{"item":{"type":"plan","id":"p1","text":text},"threadId":"t","turnId":"u"}})
            .to_string()
    };
    let mut session = running();
    let step = read(session.as_mut(), &item("- [x] Read\n- [ ] Write"));
    assert_eq!(
        step.events,
        [TurnEvent::Todos {
            todos: vec![
                crate::events::Todo {
                    text: "Read".into(),
                    status: crate::events::TodoStatus::Completed
                },
                crate::events::Todo {
                    text: "Write".into(),
                    status: crate::events::TodoStatus::Pending
                },
            ]
        }]
    );
    let step = read(session.as_mut(), &item("# Plan\n\nDo the thing."));
    assert_eq!(
        step.events,
        [TurnEvent::Text {
            text: "# Plan\n\nDo the thing.".into()
        }]
    );
}

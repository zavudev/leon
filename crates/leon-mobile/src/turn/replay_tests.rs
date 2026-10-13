//! A turn found again, with a real agent's protocol in the engine: what was
//! already written is not written twice, and what the agent still asks can
//! be answered. The lines are those of `leon-turn`'s own tests.

use std::time::Instant;

use leon_turn::{Action, AgentSession, Answer, TurnEvent};

use super::Engine;

fn permission(choice: &str) -> Answer {
    Answer::Permission {
        choice_id: choice.into(),
        reason: None,
    }
}

mod codex {
    use super::*;

    const THREAD: &str = r#"{"id":"leon-thread","result":{"thread":{"id":"01a11902-6cec","sessionId":"01a11902-6cec","preview":"","ephemeral":false,"historyMode":"paginated","modelProvider":"openai","model":"gpt-5.6-sol","reasoningEffort":"medium","status":{"type":"idle"},"cliVersion":"0.160.1","originator":"leon-mobile","turns":[]},"model":"gpt-5.6-sol","modelProvider":"openai","serviceTier":null,"approvalPolicy":"untrusted","approvalsReviewer":"user","sandbox":{"type":"readOnly","networkAccess":false},"reasoningEffort":"medium"}}"#;
    const TURN: &str = r#"{"id":"leon-turn","result":{"turn":{"id":"01a11902-6fb7","items":[],"itemsView":"notLoaded","status":"inProgress","error":null,"startedAt":null,"completedAt":null,"durationMs":null}}}"#;
    const COMMAND: &str = r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"exec-b3f0","pluginId":null,"scriptPath":null,"command":"/usr/bin/bash -lc 'touch leon-probe.txt'","cwd":"/work","processId":null,"source":"agent","status":"inProgress","commandActions":[{"type":"unknown","command":"touch leon-probe.txt"}],"aggregatedOutput":null,"exitCode":null,"durationMs":null},"threadId":"01a11902-6cec","turnId":"01a11902-6fb7","startedAtMs":1791420957915}}"#;
    const ASK_COMMAND: &str = r#"{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"01a11902-6cec","turnId":"01a11902-6fb7","itemId":"exec-b3f0","startedAtMs":1791420957913,"environmentId":"local","reason":"Allow creating the requested leon-probe.txt file in the workspace?","command":"/usr/bin/bash -lc 'touch leon-probe.txt'","cwd":"/work","commandActions":[{"type":"unknown","command":"touch leon-probe.txt"}],"proposedExecpolicyAmendment":["touch","leon-probe.txt"],"availableDecisions":["accept",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["touch","leon-probe.txt"]}},"cancel"]}}"#;

    fn fresh() -> Box<dyn AgentSession> {
        leon_turn::codex::session(100)
    }

    #[test]
    fn in_the_engine_a_replayed_turn_writes_nothing_again_and_keeps_its_question_open() {
        let now = Instant::now();
        // The first process wrote `turn/start` (acknowledged), was asked,
        // and went away.
        let replay = format!(
            "mise noise\r\n\x1b]5379;leon-ready\x07{{\"id\":\"leon-init\",\"result\":{{}}}}\r\n{THREAD}\r\n\x1b]5379;leon-ack:5:h.turn\x07{TURN}\r\n{COMMAND}\r\n{ASK_COMMAND}\r\n"
        );
        let mut engine = Engine::attached(fresh(), 9000, replay.len() as u64, now);
        let read = engine.data(replay.as_bytes(), now);
        assert!(read.write.is_empty(), "{:?}", read.write);
        assert!(matches!(
            read.events.last(),
            Some(TurnEvent::Question { id, .. }) if id == "0"
        ));
        assert_eq!(engine.unsent(), 0);
        let answered = engine
            .act(Action::Answer("0".into(), permission("d.0")), now)
            .unwrap();
        let written: String = answered
            .write
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect();
        assert!(written.contains("E 9001 1 a.0\n"), "{written}");

        // The same turn, its `turn/start` never acknowledged: once caught up,
        // the question in its place is what gets written.
        let replay = format!("\x1b]5379;leon-ready\x07{THREAD}\r\n");
        let mut engine = Engine::attached(fresh(), 9000, replay.len() as u64, now);
        let read = engine.data(replay.as_bytes(), now);
        let written: String = read
            .write
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect();
        assert!(written.contains("thread/loaded/list"), "{written}");
        assert!(written.contains(" h.turn\n"), "{written}");
    }
}

mod opencode {
    use super::*;

    const ALLOW: &str = include_str!("../../../leon-turn/src/opencode/fixtures/allow.jsonl");

    fn fresh() -> Box<dyn AgentSession> {
        leon_turn::opencode::session(100)
    }

    #[test]
    fn in_the_engine_a_replayed_turn_writes_nothing_again_and_keeps_its_question_open() {
        let now = Instant::now();
        let lines: Vec<&str> = ALLOW.lines().collect();
        let asked = lines
            .iter()
            .position(|line| line.contains("session/request_permission"))
            .unwrap();
        // The first process wrote the prompt (acknowledged), was asked, and
        // went away.
        let replay = format!(
            "shell noise\r\n\x1b]5379;leon-ready\x07{}\r\n{}\r\n\x1b]5379;leon-ack:5:h.turn\x07{}\r\n",
            lines[0],
            lines[1],
            lines[2..=asked].join("\r\n")
        );
        let mut engine = Engine::attached(fresh(), 9000, replay.len() as u64, now);
        let read = engine.data(replay.as_bytes(), now);
        assert!(read.write.is_empty(), "{:?}", read.write);
        assert!(matches!(
            read.events.last(),
            Some(TurnEvent::Question { id, .. }) if id == "0"
        ));
        assert_eq!(engine.unsent(), 0);
        let answered = engine
            .act(Action::Answer("0".into(), permission("once")), now)
            .unwrap();
        let written: String = answered
            .write
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect();
        assert!(written.contains("E 9001 1 a.0\n"), "{written}");

        // The same turn, its prompt never acknowledged: once caught up, the
        // question in its place is what gets written.
        let replay = format!("\x1b]5379;leon-ready\x07{}\r\n{}\r\n", lines[0], lines[1]);
        let mut engine = Engine::attached(fresh(), 9000, replay.len() as u64, now);
        let read = engine.data(replay.as_bytes(), now);
        let written: String = read
            .write
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect();
        assert!(written.contains("leon/lost"), "{written}");
        assert!(written.contains(" h.turn\n"), "{written}");
    }
}

//! A phone's whole path, in this process: a relay on localhost, a real host,
//! and this crate's API pairing with it and running turns.
//!
//! The agent is a shell script that speaks as Claude Code does (it reads its
//! standard input and prints one JSON value per line), so the terminal, the
//! bridge, the engine and the listener are exercised without any agent
//! installed; the login shell is bypassed, so nobody's shell is involved.

#![cfg(unix)]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use leon_host::{Backoff, Host, HostConfig, RelayState};
use leon_link::test_support::TestRelay;
use leon_link::Identity;
use leon_mobile::{
    Answer, ConnectionListener, ConnectionState, HostSession, KingListener, LeonMobile, LogoFormat,
    MobileError, PermissionEffect, ProjectLogo, QuestionKind, SpawnOverrides, TurnEvent,
    TurnListener, TurnRequest, Worktree,
};

const WAIT: Duration = Duration::from_secs(30);

/// Answers at once: says what it was told on its input, as Claude Code
/// would. Asked for its version, its help or what it offers, it says that.
const QUICK_AGENT: &str = r#"#!/bin/sh
case "$1" in
--version) echo '9.9.9 (Fake Code)'; exit 0 ;;
--help)
    printf '  --model <model>            Model for the session\n'
    printf '  --permission-mode <mode>   Permission mode to use for the session\n'
    printf '                             (choices: "acceptEdits", "plan",\n'
    printf '                             "bypassPermissions")\n'
    printf '  --print                    Print (and exit)\n'
    exit 0 ;;
esac
printf 'a line that is not an event\n'
while IFS= read -r line; do
    case "$line" in
    *'"subtype":"initialize"'*)
        printf '{"type":"control_response","response":{"subtype":"success","request_id":"init","response":{"models":[{"value":"default","displayName":"Default","description":"The usual one","supportedEffortLevels":["low","high"]},{"value":"small","displayName":"Small"}],"current_permission_mode":"default"}}}\n' ;;
    *'"type":"user"'*)
        prompt=$(printf '%s' "$line" | sed -e 's/.*"text":"\(.*\)","type":"text".*/\1/' -e 's/.*"type":"text","text":"\(.*\)"}\].*/\1/')
        printf '{"type":"system","subtype":"init","session_id":"fake-1","model":"fake-model"}\n'
        printf '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"you "}},"parent_tool_use_id":null}\n'
        printf '{"type":"assistant","message":{"content":[{"type":"text","text":"you said: %s"}]},"parent_tool_use_id":null}\n' "$prompt"
        printf '{"type":"result","subtype":"success","is_error":false,"duration_ms":5,"total_cost_usd":0.5,"session_id":"fake-1","permission_denials":[],"queued_turn_count":0}\n' ;;
    esac
done
"#;

/// Starts, then works until the file named by its prompt exists; told its
/// prompt on its input.
const SLOW_SESSION: &str = r#"#!/bin/sh
while IFS= read -r line; do
    case "$line" in
    *'"type":"user"'*)
        gate=$(printf '%s' "$line" | sed -e 's/.*"text":"\(.*\)","type":"text".*/\1/' -e 's/.*"type":"text","text":"\(.*\)"}\].*/\1/')
        printf '{"type":"system","subtype":"init","session_id":"slow-1","model":"fake-model"}\n'
        while [ ! -e "$gate" ]; do sleep 0.05; done
        printf '{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]},"parent_tool_use_id":null}\n'
        printf '{"type":"result","subtype":"success","is_error":false,"session_id":"slow-1","queued_turn_count":0}\n' ;;
    esac
done
"#;

/// Asks before it acts and waits for the answer, as Claude Code does over
/// its control protocol; says what it was answered and how many `Z` the
/// answer held; stops when told to.
const ASKING_AGENT: &str = r#"#!/bin/sh
while IFS= read -r line; do
    case "$line" in
    *'"type":"user"'*)
        printf '{"type":"system","subtype":"init","session_id":"ask-1","model":"fake-model","permissionMode":"default"}\n'
        printf '{"type":"control_request","request_id":"q-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"make test"},"tool_use_id":"tool-1","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}]}}\n' ;;
    *'"subtype":"interrupt"'*)
        printf '{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"ask-1","terminal_reason":"aborted_tools","queued_turn_count":0}\n' ;;
    *'"type":"control_response"'*)
        behavior=allow
        case "$line" in *'"behavior":"deny"'*) behavior=deny ;; esac
        count=$(printf '%s' "$line" | tr -cd Z | wc -c | tr -d ' ')
        printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s with %s Z"}]},"parent_tool_use_id":null}\n' "$behavior" "$count"
        printf '{"type":"result","subtype":"success","is_error":false,"session_id":"ask-1","queued_turn_count":0}\n' ;;
    esac
done
"#;

/// Speaks as Codex's application server does: JSON-RPC on its standard
/// streams. It names a thread, starts the turn it is given, asks before
/// running a command and waits for the decision; says which decision came
/// back; stops when told to; never ends by itself. Asked what it offers, it
/// lists two models (one hidden) and its configuration.
const CODEX_AGENT: &str = r#"#!/bin/sh
case "$1" in --version) echo 'codex-cli 9.9.9'; exit 0 ;; esac
printf 'tools: codex@9.9.9\n'
while IFS= read -r line; do
    case "$line" in
    *'"method":"initialize"'*)
        printf '{"id":"leon-init","result":{"userAgent":"fake","platformOs":"linux"}}\n' ;;
    *'"method":"model/list"'*)
        printf '{"id":"leon-models","result":{"data":[{"id":"big","model":"big","displayName":"Big","description":"The big one","hidden":false,"supportedReasoningEfforts":[{"reasoningEffort":"low","description":""},{"reasoningEffort":"high","description":""}],"isDefault":true},{"id":"secret","model":"secret","displayName":"Secret","description":"","hidden":true,"supportedReasoningEfforts":[],"isDefault":false}],"nextCursor":null}}\n' ;;
    *'"method":"config/read"'*)
        printf '{"id":"leon-config","result":{"config":{"approval_policy":"on-request","sandbox_mode":"workspace-write"}}}\n' ;;
    *'"method":"thread/start"'*|*'"method":"thread/resume"'*)
        printf '{"id":"leon-thread","result":{"thread":{"id":"th-1"},"model":"big","approvalPolicy":"untrusted","sandbox":{"type":"workspaceWrite"}}}\n' ;;
    *'"method":"turn/start"'*)
        printf '{"id":"leon-turn","result":{"turn":{"id":"tu-1","status":"inProgress"}}}\n'
        printf '{"method":"item/started","params":{"item":{"type":"commandExecution","id":"ex-1","command":"/bin/sh -lc make","commandActions":[{"type":"unknown","command":"make test"}],"status":"inProgress"}}}\n'
        printf '{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","itemId":"ex-1","command":"/bin/sh -lc make","commandActions":[{"type":"unknown","command":"make test"}],"reason":"It builds","availableDecisions":["accept",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["make"]}},"cancel"]}}\n' ;;
    *'"method":"turn/interrupt"'*)
        printf '{"method":"turn/completed","params":{"turn":{"id":"tu-1","status":"interrupted","error":null}}}\n' ;;
    *'"id":0,"result"'*)
        decision=$(printf '%s' "$line" | sed 's/.*"decision":\(.*\)}}$/\1/' | tr -d '"{}')
        printf '{"method":"serverRequest/resolved","params":{"requestId":0}}\n'
        printf '{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"ex-1","command":"/bin/sh -lc make","status":"completed","exitCode":0,"aggregatedOutput":"ok\\n"}}}\n'
        printf '{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m-1","text":"decided %s"}}}\n' "$decision"
        printf '{"method":"turn/completed","params":{"turn":{"id":"tu-1","status":"completed","error":null,"durationMs":7}}}\n' ;;
    esac
done
"#;

/// Speaks as opencode does over the Agent Client Protocol: a session in
/// the folder it is given, a tool it asks before using, the answer said
/// back; stops when its session is cancelled; never ends by itself. Asked
/// outside a conversation, it lists its models and its agents.
const OPENCODE_AGENT: &str = r#"#!/bin/sh
case "$1" in
--version) echo 'v9.9.9'; exit 0 ;;
models) printf 'fake/small\nfake/big\n'; exit 0 ;;
debug) echo '[{"id":"build","name":"Build","description":"Builds","mode":"primary","hidden":false},{"id":"title","name":"Title","mode":"primary","hidden":true},{"id":"plan","name":"Plan","mode":"primary","hidden":false},{"id":"explore","name":"Explore","mode":"subagent","hidden":false}]'; exit 0 ;;
esac
options='[{"id":"model","currentValue":"fake/small","options":[]},{"id":"mode","currentValue":"build","options":[]}]'
while IFS= read -r line; do
    case "$line" in
    *'"method":"initialize"'*)
        printf '{"jsonrpc":"2.0","id":"leon-init","result":{"protocolVersion":1}}\n' ;;
    *'"method":"session/new"'*)
        folder=$(printf '%s' "$line" | sed 's/.*"cwd":"\([^"]*\)".*/\1/')
        printf '{"jsonrpc":"2.0","id":"leon-session","result":{"sessionId":"ses_1","configOptions":%s}}\n' "$options"
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_1","update":{"sessionUpdate":"config_option_update","configOptions":%s}}}\n' "$options" ;;
    *'"method":"session/set_config_option"'*)
        id=$(printf '%s' "$line" | sed 's/.*"id":"\([^"]*\)".*/\1/')
        printf '{"jsonrpc":"2.0","id":"%s","result":{"configOptions":[]}}\n' "$id" ;;
    *'"method":"session/prompt"'*)
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_1","update":{"sessionUpdate":"tool_call","toolCallId":"call-1","title":"shell","kind":"execute","status":"pending","rawInput":{}}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_1","update":{"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"in_progress","kind":"execute","title":"make test","rawInput":{"command":"make test","cwd":"%s"}}}}\n' "$folder"
        printf '{"jsonrpc":"2.0","id":0,"method":"session/request_permission","params":{"sessionId":"ses_1","toolCall":{"toolCallId":"call-1","title":"make test","kind":"execute","status":"pending","rawInput":{"command":"make test"}},"options":[{"optionId":"once","kind":"allow_once","name":"Allow once"},{"optionId":"always","kind":"allow_always","name":"Always allow"},{"optionId":"reject","kind":"reject_once","name":"Reject"}]}}\n' ;;
    *'"method":"session/cancel"'*)
        printf '{"jsonrpc":"2.0","id":"leon-turn","result":{"stopReason":"cancelled"}}\n' ;;
    *'"id":0,'*'"result"'*)
        case "$line" in *'"outcome":"cancelled"'*) continue ;; esac
        option=$(printf '%s' "$line" | sed 's/.*"optionId":"\([^"]*\)".*/\1/')
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_1","update":{"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"ok"}}]}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_1","update":{"sessionUpdate":"agent_message_chunk","messageId":"m-1","content":{"type":"text","text":"decided %s in %s"}}}}\n' "$option" "${folder##*/}"
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_1","update":{"sessionUpdate":"usage_update","used":10,"size":100,"cost":{"amount":0.25,"currency":"USD"}}}}\n'
        printf '{"jsonrpc":"2.0","id":"leon-turn","result":{"stopReason":"end_turn"}}\n' ;;
    esac
done
"#;

struct States(Mutex<Sender<ConnectionState>>);

impl ConnectionListener for States {
    fn on_state(&self, state: ConnectionState) {
        let _ = self.0.lock().unwrap().send(state);
    }
}

struct Events(Mutex<Sender<TurnEvent>>);

impl TurnListener for Events {
    fn on_event(&self, event: TurnEvent) {
        let _ = self.0.lock().unwrap().send(event);
    }
}

fn listener() -> (Box<dyn TurnListener>, Receiver<TurnEvent>) {
    let (tx, rx) = channel();
    (Box::new(Events(Mutex::new(tx))), rx)
}

/// Every event up to and including the last one of a turn.
fn whole_turn(events: &Receiver<TurnEvent>) -> Vec<TurnEvent> {
    let mut seen = Vec::new();
    loop {
        let event = events.recv_timeout(WAIT).expect("the turn goes on");
        let last = matches!(event, TurnEvent::Exited { .. } | TurnEvent::Gone);
        seen.push(event);
        if last {
            return seen;
        }
    }
}

/// Every event up to and including the first one `last` accepts.
fn until(events: &Receiver<TurnEvent>, last: impl Fn(&TurnEvent) -> bool) -> Vec<TurnEvent> {
    let mut seen = Vec::new();
    loop {
        let event = events.recv_timeout(WAIT).expect("the turn goes on");
        let done = last(&event);
        seen.push(event);
        if done {
            return seen;
        }
    }
}

fn texts(events: &[TurnEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            TurnEvent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn script(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

struct World {
    /// Nothing while a test has taken it down.
    relay: Option<TestRelay>,
    host: Host,
    mobile: Arc<LeonMobile>,
    session: Arc<HostSession>,
    states: Receiver<ConnectionState>,
    dir: tempfile::TempDir,
}

impl World {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn request(&self, agent: &str, prompt: &str, chat: &str) -> TurnRequest {
        TurnRequest {
            agent: agent.into(),
            cwd: Some(self.dir.path().to_string_lossy().into_owned()),
            prompt: prompt.into(),
            resume_id: None,
            mode: None,
            model: None,
            effort: None,
            add_dirs: Vec::new(),
            chat_id: chat.into(),
        }
    }

    /// Starts the stand-in that asks, under Claude's tag, and waits for its
    /// question.
    async fn asking(
        &self,
        chat: &str,
    ) -> (Arc<leon_mobile::Turn>, Receiver<TurnEvent>, Vec<TurnEvent>) {
        self.session.set_spawn_overrides(SpawnOverrides {
            direct: true,
            programs: HashMap::from([(
                "claude".to_owned(),
                script(self.dir.path(), "asking", ASKING_AGENT),
            )]),
        });
        let (listener, events) = listener();
        let turn = self
            .session
            .start_turn(self.request("claude", "run the tests", chat), listener)
            .await
            .unwrap();
        let seen = until(&events, |event| matches!(event, TurnEvent::Question { .. }));
        (turn, events, seen)
    }

    async fn stop(self) {
        self.session.close();
        self.host.stop();
        if let Some(relay) = self.relay {
            relay.stop().await;
        }
    }
}

/// A repository with one commit and a worktree beside it, both under `dir`
/// and outside any repository `dir` may be in. Nothing when git is not there
/// to make them: the caller skips that part.
fn repo_with_worktree(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let dir = dir.canonicalize().ok()?;
    let (repo, tree) = (dir.join("repo"), dir.join("repo-tree"));
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CEILING_DIRECTORIES", &dir)
            .output()
            .is_ok_and(|output| output.status.success())
    };
    let (repo_text, tree_text) = (repo.to_str()?, tree.to_str()?);
    let made = git(&["init", "-q", repo_text])
        && git(&[
            "-C",
            repo_text,
            "-c",
            "user.name=Leon",
            "-c",
            "user.email=leon@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ])
        && git(&[
            "-C", repo_text, "worktree", "add", "-q", "-b", "side", tree_text,
        ]);
    made.then_some((repo, tree))
}

async fn host_on(url: &str, dir: &Path) -> Host {
    let identity = Arc::new(Identity::load_or_create(&dir.join("host")).unwrap());
    let mut config = HostConfig::new(url, "build-box");
    config.backoff = Backoff {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(200),
    };
    config.king = Some(leon_host::King(Arc::new(FakeKing)));
    let host = Host::start(config, identity, Some(dir.join("devices.json"))).unwrap();
    let mut state = host.watch_relay();
    while *state.borrow_and_update() != RelayState::Online {
        state.changed().await.unwrap();
    }
    host
}

/// Pairs with `code`. The host announces the code's room to the relay a
/// moment after it hands the code out: a phone quicker than that is told
/// nobody shows the code, and tries again as a person would.
async fn pair_when_shown(
    mobile: &LeonMobile,
    relay: &str,
    code: &str,
) -> Result<leon_mobile::PairedHost, MobileError> {
    let mut tries = 0;
    loop {
        match mobile.pair(relay.to_owned(), code.to_owned()).await {
            Err(MobileError::CodeNotFound) if tries < 100 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            paired => return paired,
        }
    }
}

/// A relay, a host, and a phone paired with it and online. "claude" is the
/// quick stand-in, "codex" one that speaks as its application server does,
/// and "opencode" one that speaks the Agent Client Protocol.
async fn paired() -> World {
    let relay = TestRelay::start().await;
    let dir = tempfile::tempdir().unwrap();
    let host = host_on(&relay.url(), dir.path()).await;
    let phone = dir.path().join("phone").to_string_lossy().into_owned();
    let mobile = LeonMobile::new(phone, "Ana's phone".into()).unwrap();

    let code = host.new_pairing_code().await.code;
    let paired = pair_when_shown(&mobile, &relay.url(), &code).await.unwrap();
    assert_eq!(paired.host_name, "build-box");
    assert_eq!(paired.host_id, host.host_id().to_string());

    let (tx, states) = channel();
    let session = mobile
        .connect(paired, Box::new(States(Mutex::new(tx))))
        .unwrap();
    loop {
        match states.recv_timeout(WAIT).expect("the session comes online") {
            ConnectionState::Online => break,
            ConnectionState::Connecting => {}
            other => panic!("unexpected state {other:?}"),
        }
    }
    session.set_spawn_overrides(SpawnOverrides {
        direct: true,
        programs: HashMap::from([
            (
                "claude".to_owned(),
                script(dir.path(), "quick", QUICK_AGENT),
            ),
            ("codex".to_owned(), script(dir.path(), "codex", CODEX_AGENT)),
            (
                "opencode".to_owned(),
                script(dir.path(), "opencode", OPENCODE_AGENT),
            ),
        ]),
    });
    World {
        relay: Some(relay),
        host,
        mobile,
        session,
        states,
        dir,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paired_phone_is_a_device_of_the_host_and_asks_an_agent() {
    let world = paired().await;
    let devices = world.host.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Ana's phone");
    assert_eq!(world.session.host_name().as_deref(), Some("build-box"));
    assert_eq!(world.session.state(), ConnectionState::Online);

    let (listener, events) = listener();
    // Nothing JSON would need escaped: the stand-in prints it as it is.
    let prompt = "--is it safe; $(id) 'here'?";
    let turn = world
        .session
        .start_turn(world.request("claude", prompt, "chat-1"), listener)
        .await
        .unwrap();
    assert_eq!(
        whole_turn(&events),
        vec![
            TurnEvent::Started {
                session_id: "fake-1".into(),
                model: Some("fake-model".into()),
            },
            TurnEvent::TextDelta {
                text: "you ".into()
            },
            TurnEvent::Text {
                text: format!("you said: {prompt}"),
            },
            TurnEvent::Finished {
                ok: true,
                error: None,
                cost_usd: Some(0.5),
                duration_ms: Some(5),
                denied_tools: Vec::new(),
                interrupted: false,
            },
            TurnEvent::Exited { code: 0 },
        ]
    );

    // It stays listed for a while, ended, under its chat.
    let listed = world.session.running_turns().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].pty, turn.pty_id());
    assert_eq!(listed[0].agent, "claude");
    assert_eq!(listed[0].chat_id, "chat-1");
    assert_eq!(listed[0].exit_code, Some(0));
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_left_running_is_found_again_and_replayed_from_its_start() {
    let world = paired().await;
    let gate = world.path("gate");
    let (first, first_events) = listener();

    // The slow agent under claude's tag, so that its events are understood.
    world.session.set_spawn_overrides(SpawnOverrides {
        direct: true,
        programs: HashMap::from([(
            "claude".to_owned(),
            script(world.dir.path(), "slow-session", SLOW_SESSION),
        )]),
    });
    let turn = world
        .session
        .start_turn(
            world.request("claude", &gate.to_string_lossy(), "chat-7"),
            first,
        )
        .await
        .unwrap();
    assert_eq!(
        first_events.recv_timeout(WAIT).unwrap(),
        TurnEvent::Started {
            session_id: "slow-1".into(),
            model: Some("fake-model".into()),
        }
    );

    // The application is closed and opened again: only the host knows.
    let running = world.session.running_turns().await.unwrap();
    let found = running
        .iter()
        .find(|turn| turn.chat_id == "chat-7")
        .expect("the turn is listed");
    assert_eq!(found.pty, turn.pty_id());
    assert_eq!(found.exit_code, None);
    let (second, second_events) = listener();
    world.session.attach_turn(found.pty, second).await.unwrap();
    std::fs::write(&gate, b"").unwrap();

    let replayed = whole_turn(&second_events);
    assert_eq!(
        replayed,
        vec![
            TurnEvent::Started {
                session_id: "slow-1".into(),
                model: Some("fake-model".into()),
            },
            TurnEvent::Text {
                text: "done".into()
            },
            TurnEvent::Finished {
                ok: true,
                error: None,
                cost_usd: None,
                duration_ms: None,
                denied_tools: Vec::new(),
                interrupted: false,
            },
            TurnEvent::Exited { code: 0 },
        ]
    );
    // One listener follows a turn at a time: the first was let go.
    assert!(first_events.recv_timeout(WAIT).is_err());

    let missing = world
        .session
        .attach_turn(9_999, Box::new(Events(Mutex::new(channel().0))))
        .await;
    assert!(matches!(missing, Err(MobileError::TurnNotFound)));
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_turn_ends_as_a_failed_one() {
    let world = paired().await;
    let (listener, events) = listener();
    let turn = world
        .session
        .start_turn(
            world.request("opencode", "run the tests", "chat-2"),
            listener,
        )
        .await
        .unwrap();
    until(&events, |event| matches!(event, TurnEvent::Question { .. }));
    // It cannot be told more while it works, only answered or stopped.
    assert_eq!(
        turn.send("and then?".into()),
        Err(MobileError::NotSupported)
    );
    turn.cancel();
    let seen = whole_turn(&events);
    assert!(
        matches!(seen.last(), Some(TurnEvent::Exited { code }) if *code != 0),
        "{seen:?}"
    );
    assert!(
        matches!(
            &seen[seen.len() - 2],
            TurnEvent::Finished {
                ok: false,
                error: Some(_),
                ..
            }
        ),
        "{seen:?}"
    );
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opencode_asks_before_a_command_and_does_what_the_phone_decides() {
    let world = paired().await;
    let (listener, events) = listener();
    // Its mode is set before the prompt, and the folder it was started in
    // is the one its session is given.
    let mut request = world.request("opencode", "run the tests", "chat-oc");
    request.mode = Some("plan".into());
    let turn = world.session.start_turn(request, listener).await.unwrap();
    let asked = until(&events, |event| matches!(event, TurnEvent::Question { .. }));
    assert_eq!(
        asked[0],
        TurnEvent::Started {
            session_id: "ses_1".into(),
            model: Some("fake/small".into()),
        }
    );
    assert!(asked.contains(&TurnEvent::ToolUse {
        id: "call-1".into(),
        name: "shell".into(),
        summary: "make test".into(),
        detail: String::new(),
    }));
    let Some(TurnEvent::Question {
        id,
        kind: QuestionKind::Permission { tool, choices, .. },
    }) = asked.last()
    else {
        panic!("{asked:?}");
    };
    assert_eq!((id.as_str(), tool.as_str()), ("0", "shell"));
    let effects: Vec<PermissionEffect> = choices.iter().map(|choice| choice.effect).collect();
    assert_eq!(
        effects,
        [
            PermissionEffect::AllowOnce,
            PermissionEffect::AllowAlways,
            PermissionEffect::Deny
        ]
    );
    turn.answer("0".into(), decision("always")).unwrap();
    let rest = whole_turn(&events);
    let folder = world.dir.path().file_name().unwrap().to_string_lossy();
    assert_eq!(
        texts(&rest),
        [format!("decided always in {folder}")],
        "{rest:?}"
    );
    assert!(rest.contains(&TurnEvent::QuestionResolved { id: "0".into() }));
    // Its server waits for more for ever: its input was closed for it.
    assert!(
        matches!(
            &rest[rest.len() - 2..],
            [
                TurnEvent::Finished {
                    ok: true,
                    cost_usd: Some(_),
                    ..
                },
                TurnEvent::Exited { code: 0 }
            ]
        ),
        "{rest:?}"
    );

    // Stopped while it asks: it ends its turn itself.
    let (stopping, events) = self::listener();
    let turn = world
        .session
        .start_turn(world.request("opencode", "again", "chat-oc-stop"), stopping)
        .await
        .unwrap();
    until(&events, |event| matches!(event, TurnEvent::Question { .. }));
    turn.interrupt().unwrap();
    let rest = whole_turn(&events);
    assert!(
        matches!(
            &rest[rest.len() - 2..],
            [
                TurnEvent::Finished {
                    ok: false,
                    error: None,
                    interrupted: true,
                    ..
                },
                TurnEvent::Exited { code: 0 }
            ]
        ),
        "{rest:?}"
    );
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_host_is_asked_what_it_has() {
    let world = paired().await;
    // A headless host shares nothing; that is an empty answer, not an error.
    let shared = world.session.share_state().await.unwrap();
    assert!(shared.projects.is_empty() && shared.sessions.is_empty());
    assert_eq!(
        world
            .session
            .transcript("claude".into(), "nothing".into())
            .await
            .unwrap(),
        None
    );

    std::fs::create_dir(world.path("beta")).unwrap();
    std::fs::create_dir(world.path("alpha project")).unwrap();
    std::fs::create_dir(world.path(".hidden")).unwrap();
    let dirs = world
        .session
        .subdirs(world.dir.path().to_string_lossy().into_owned())
        .await
        .unwrap();
    assert_eq!(dirs, ["alpha project", "beta", "host", "phone"]);
    let nowhere = world.session.subdirs("/no/such/folder".into()).await;
    assert!(matches!(nowhere, Err(MobileError::Host { .. })));

    // A repository names its worktrees, the main one first, whichever of
    // them (or folder inside one) is asked; a folder outside any, and one
    // that is gone, name none.
    if let Some((repo, tree)) = repo_with_worktree(world.dir.path()) {
        let inside = repo.join("src");
        std::fs::create_dir(&inside).unwrap();
        let text = |path: &Path| path.to_string_lossy().into_owned();
        let found = world
            .session
            .worktrees(vec![
                text(&repo),
                text(&world.path("beta")),
                text(&inside),
                "/no/such/folder".into(),
                text(&tree),
                String::new(),
            ])
            .await
            .unwrap();
        let listed = |trees: &[Worktree]| -> Vec<(String, Option<String>, bool)> {
            trees
                .iter()
                .map(|tree| (tree.path.clone(), tree.branch.clone(), tree.main))
                .collect()
        };
        let both = listed(&found[0]);
        assert_eq!(both.len(), 2);
        assert_eq!(
            (both[0].0.as_str(), both[0].2),
            (text(&repo).as_str(), true)
        );
        assert!(both[0].1.is_some(), "the main checkout is on a branch");
        assert_eq!(both[1], (text(&tree), Some("side".to_owned()), false));
        assert_eq!(listed(&found[2]), both);
        assert_eq!(listed(&found[4]), both);
        assert!(found[1].is_empty() && found[3].is_empty() && found[5].is_empty());
    }
    assert!(world
        .session
        .worktrees(Vec::new())
        .await
        .unwrap()
        .is_empty());

    // The stand-ins exist; an agent whose program does not is not listed.
    // The agents without a stand-in are whatever this computer has, and are
    // left out of what is compared.
    let installed: Vec<String> = world
        .session
        .installed_agents()
        .await
        .unwrap()
        .into_iter()
        .filter(|agent| !matches!(agent.as_str(), "grok" | "cursor"))
        .collect();
    assert_eq!(installed, ["claude", "codex", "opencode"]);
    world.session.set_spawn_overrides(SpawnOverrides {
        direct: true,
        programs: HashMap::from([
            ("codex".to_owned(), "/no/such/program".to_owned()),
            ("opencode".to_owned(), "/no/such/program".to_owned()),
        ]),
    });
    let unknown = world
        .session
        .start_turn(
            world.request("vim", "hi", "chat-3"),
            Box::new(Events(Mutex::new(channel().0))),
        )
        .await;
    assert!(matches!(unknown, Err(MobileError::UnknownAgent)));
    world.stop().await;
}

/// A one-pixel PNG.
const PIXEL: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[tokio::test(flavor = "multi_thread")]
async fn a_project_shows_the_logo_the_desktop_would_find() {
    let world = paired().await;
    let text = |path: &Path| path.to_string_lossy().into_owned();

    // An image file of the project is what stands for it.
    let raster = world.path("raster");
    std::fs::create_dir(&raster).unwrap();
    std::fs::write(raster.join("logo.png"), PIXEL).unwrap();
    let logo = world.session.project_logo(text(&raster)).await.unwrap();
    let image = logo.image.expect("the logo file");
    assert_eq!(image.format, LogoFormat::Png);
    assert!(image.data.starts_with("iVBORw0KGgo"), "{}", image.data);
    assert!(image.source.ends_with("logo.png"), "{}", image.source);
    assert!(!image.hash.is_empty());
    assert_eq!(logo.avatar_url, None);

    // A vector one travels as its source.
    let vector = world.path("vector");
    std::fs::create_dir(&vector).unwrap();
    std::fs::write(
        vector.join("logo.svg"),
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 1 1'><rect width='1' height='1'/></svg>",
    )
    .unwrap();
    let image = world
        .session
        .project_logo(text(&vector))
        .await
        .unwrap()
        .image
        .expect("the vector logo");
    assert_eq!(image.format, LogoFormat::Svg);
    assert!(image.data.starts_with("<svg"), "{}", image.data);

    // A folder with nothing to show: the caller draws the folder glyph.
    let bare = world.path("bare");
    std::fs::create_dir(&bare).unwrap();
    assert_eq!(
        world.session.project_logo(text(&bare)).await.unwrap(),
        ProjectLogo::default()
    );

    // Without an image, a repository on GitHub offers its owner's avatar.
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&bare)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .is_ok_and(|output| output.status.success())
    };
    if git(&["init", "-q"])
        && git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/zavudev/leon.git",
        ])
    {
        let logo = world.session.project_logo(text(&bare)).await.unwrap();
        assert_eq!(logo.image, None);
        assert_eq!(
            logo.avatar_url.as_deref(),
            Some("https://github.com/zavudev.png?size=64")
        );
    }
    world.stop().await;
}

fn deny(reason: &str) -> Answer {
    Answer::Permission {
        choice_id: "deny".into(),
        reason: Some(reason.into()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn what_the_agent_asks_is_answered_from_the_phone_however_long_the_answer() {
    let world = paired().await;
    let (turn, events, asked) = world.asking("chat-ask").await;
    assert_eq!(
        asked[..2],
        [
            TurnEvent::ModeChanged {
                mode: "default".into()
            },
            TurnEvent::Started {
                session_id: "ask-1".into(),
                model: Some("fake-model".into()),
            },
        ]
    );
    let TurnEvent::Question { id, kind } = &asked[2] else {
        panic!("{asked:?}");
    };
    assert_eq!(id, "q-1");
    let QuestionKind::Permission {
        tool,
        summary,
        choices,
        ..
    } = kind
    else {
        panic!("{kind:?}");
    };
    assert_eq!((tool.as_str(), summary.as_str()), ("Bash", "make test"));
    let effects: Vec<PermissionEffect> = choices.iter().map(|choice| choice.effect).collect();
    assert_eq!(
        effects,
        [
            PermissionEffect::AllowOnce,
            PermissionEffect::AllowAlways,
            PermissionEffect::Deny
        ]
    );
    assert_eq!(
        turn.answer("nothing asked".into(), deny("")),
        Err(MobileError::UnknownQuestion)
    );

    // Far longer than a terminal hands over in one line, with what a
    // terminal or a shell might act on.
    let reason = format!(
        "{} é \\ $(id) `id` 'q' \"q\" * ~ \u{7f} end",
        "Z".repeat(100_000)
    );
    turn.answer("q-1".into(), deny(&reason)).unwrap();
    let rest = whole_turn(&events);
    assert_eq!(texts(&rest), ["deny with 100000 Z"], "{rest:?}");
    assert!(rest.contains(&TurnEvent::QuestionResolved { id: "q-1".into() }));
    assert!(matches!(
        rest[rest.len() - 2],
        TurnEvent::Finished { ok: true, .. }
    ));
    assert_eq!(rest.last(), Some(&TurnEvent::Exited { code: 0 }));
    assert_eq!(turn.unsent(), 0);
    assert_eq!(
        turn.answer("q-1".into(), deny("")),
        Err(MobileError::TurnEnded)
    );
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_given_while_offline_reaches_the_agent_when_the_connection_is_back() {
    let mut world = paired().await;
    let (turn, events, _) = world.asking("chat-offline").await;

    let relay = world.relay.take().unwrap();
    let addr = relay.addr();
    relay.stop().await;
    until(&events, |event| *event == TurnEvent::Disconnected);
    turn.answer("q-1".into(), deny("ZZZ")).unwrap();
    assert_eq!(turn.unsent(), 1);
    // Nothing reaches anybody: the question stays open.
    assert!(events.recv_timeout(Duration::from_millis(300)).is_err());

    world.relay = Some(TestRelay::start_on(addr).await);
    let rest = whole_turn(&events);
    assert!(rest.contains(&TurnEvent::Reconnected), "{rest:?}");
    assert!(rest.contains(&TurnEvent::QuestionResolved { id: "q-1".into() }));
    assert_eq!(texts(&rest), ["deny with 3 Z"], "{rest:?}");
    assert_eq!(rest.last(), Some(&TurnEvent::Exited { code: 0 }));
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_found_again_still_asks_what_was_not_answered() {
    let world = paired().await;
    let (first, first_events, _) = world.asking("chat-again").await;

    // The application is closed and opened again.
    let (listener, events) = listener();
    let turn = world
        .session
        .attach_turn(first.pty_id(), listener)
        .await
        .unwrap();
    let replayed = until(&events, |event| matches!(event, TurnEvent::Question { .. }));
    assert!(
        matches!(
            &replayed[..],
            [
                TurnEvent::ModeChanged { .. },
                TurnEvent::Started { .. },
                TurnEvent::Question { id, .. }
            ] if id == "q-1"
        ),
        "{replayed:?}"
    );
    assert!(first_events.recv_timeout(WAIT).is_err());

    turn.answer(
        "q-1".into(),
        Answer::Permission {
            choice_id: "always.0".into(),
            reason: None,
        },
    )
    .unwrap();
    let rest = whole_turn(&events);
    // Answered once: the agent went on once.
    assert_eq!(texts(&rest), ["allow with 0 Z"], "{rest:?}");
    assert!(rest.contains(&TurnEvent::QuestionResolved { id: "q-1".into() }));
    assert_eq!(rest.last(), Some(&TurnEvent::Exited { code: 0 }));

    // Found once more after its end: the question is closed in the replay.
    let (third, events) = self::listener();
    world
        .session
        .attach_turn(first.pty_id(), third)
        .await
        .unwrap();
    let again = whole_turn(&events);
    let open = again
        .iter()
        .filter(|event| matches!(event, TurnEvent::Question { .. }))
        .count();
    let closed = again
        .iter()
        .filter(|event| matches!(event, TurnEvent::QuestionResolved { .. }))
        .count();
    assert_eq!((open, closed), (1, 1), "{again:?}");
    assert_eq!(texts(&again), ["allow with 0 Z"]);
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_asked_to_stop_ends_its_turn_itself() {
    let world = paired().await;
    let (turn, events, _) = world.asking("chat-stop").await;
    turn.interrupt().unwrap();
    let rest = whole_turn(&events);
    assert!(
        matches!(
            &rest[..],
            [
                TurnEvent::QuestionResolved { .. },
                TurnEvent::Finished {
                    ok: false,
                    error: None,
                    interrupted: true,
                    ..
                },
                TurnEvent::Exited { code: 0 }
            ]
        ),
        "{rest:?}"
    );
    world.stop().await;
}

/// Starts the stand-in for Codex and waits for its question.
async fn codex_asking(
    world: &World,
    chat: &str,
) -> (Arc<leon_mobile::Turn>, Receiver<TurnEvent>, Vec<TurnEvent>) {
    let (listener, events) = listener();
    let turn = world
        .session
        .start_turn(world.request("codex", "run the tests", chat), listener)
        .await
        .unwrap();
    let seen = until(&events, |event| matches!(event, TurnEvent::Question { .. }));
    (turn, events, seen)
}

fn decision(choice: &str) -> Answer {
    Answer::Permission {
        choice_id: choice.into(),
        reason: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_asks_before_a_command_and_does_what_the_phone_decides() {
    let world = paired().await;
    let (turn, events, asked) = codex_asking(&world, "chat-codex").await;
    assert_eq!(
        asked[..2],
        [
            TurnEvent::Started {
                session_id: "th-1".into(),
                model: Some("big".into()),
            },
            TurnEvent::ModeChanged {
                mode: "untrusted".into()
            },
        ]
    );
    assert_eq!(
        asked[2],
        TurnEvent::ToolUse {
            id: "ex-1".into(),
            name: "shell".into(),
            summary: "make test".into(),
            detail: String::new(),
        }
    );
    let TurnEvent::Question {
        id,
        kind:
            QuestionKind::Permission {
                tool,
                summary,
                reason,
                choices,
                ..
            },
    } = &asked[3]
    else {
        panic!("{asked:?}");
    };
    assert_eq!(
        (id.as_str(), tool.as_str(), summary.as_str()),
        ("0", "shell", "make test")
    );
    assert_eq!(reason.as_deref(), Some("It builds"));
    let effects: Vec<PermissionEffect> = choices.iter().map(|choice| choice.effect).collect();
    assert_eq!(
        effects,
        [
            PermissionEffect::AllowOnce,
            PermissionEffect::AllowAlways,
            PermissionEffect::Deny
        ]
    );
    // It cannot be given another mode while it works.
    assert_eq!(turn.set_mode("auto".into()), Err(MobileError::NotSupported));

    turn.answer("0".into(), decision("d.1")).unwrap();
    let rest = whole_turn(&events);
    assert_eq!(
        texts(&rest),
        ["decided acceptWithExecpolicyAmendment:execpolicy_amendment:[make]"],
        "{rest:?}"
    );
    assert!(rest.contains(&TurnEvent::QuestionResolved { id: "0".into() }));
    assert!(rest.contains(&TurnEvent::ToolResult {
        id: "ex-1".into(),
        ok: true,
        summary: "ok".into(),
        detail: String::new(),
    }));
    // Its server waits for more for ever: its input was closed for it.
    assert!(
        matches!(
            &rest[rest.len() - 2..],
            [
                TurnEvent::Finished {
                    ok: true,
                    duration_ms: Some(7),
                    ..
                },
                TurnEvent::Exited { code: 0 }
            ]
        ),
        "{rest:?}"
    );
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_refused_and_codex_stopped_end_their_turns_themselves() {
    let world = paired().await;
    let (turn, events, _) = codex_asking(&world, "chat-codex-no").await;
    turn.answer("0".into(), decision("d.2")).unwrap();
    let rest = whole_turn(&events);
    assert_eq!(texts(&rest), ["decided cancel"], "{rest:?}");
    assert_eq!(rest.last(), Some(&TurnEvent::Exited { code: 0 }));

    let (turn, events, _) = codex_asking(&world, "chat-codex-stop").await;
    turn.interrupt().unwrap();
    let rest = whole_turn(&events);
    assert!(
        matches!(
            &rest[..],
            [
                TurnEvent::QuestionResolved { .. },
                TurnEvent::ToolResult { ok: false, .. },
                TurnEvent::Finished {
                    ok: false,
                    error: None,
                    interrupted: true,
                    ..
                },
                TurnEvent::Exited { code: 0 }
            ]
        ),
        "{rest:?}"
    );
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_turn_found_again_still_asks_and_is_answered_once() {
    let world = paired().await;
    let (first, first_events, _) = codex_asking(&world, "chat-codex-again").await;

    // The application is closed and opened again: the new process was never
    // told the prompt.
    let (listener, events) = listener();
    let turn = world
        .session
        .attach_turn(first.pty_id(), listener)
        .await
        .unwrap();
    let replayed = until(&events, |event| matches!(event, TurnEvent::Question { .. }));
    assert!(
        matches!(
            &replayed[..],
            [
                TurnEvent::Started { .. },
                TurnEvent::ModeChanged { .. },
                TurnEvent::ToolUse { .. },
                TurnEvent::Question { id, .. }
            ] if id == "0"
        ),
        "{replayed:?}"
    );
    assert!(first_events
        .recv_timeout(Duration::from_millis(300))
        .is_err());

    turn.answer("0".into(), decision("d.0")).unwrap();
    let rest = whole_turn(&events);
    // The turn was not started a second time, and went on once.
    assert_eq!(texts(&rest), ["decided accept"], "{rest:?}");
    assert_eq!(rest.last(), Some(&TurnEvent::Exited { code: 0 }));
    let started = rest
        .iter()
        .filter(|event| matches!(event, TurnEvent::ToolUse { .. }))
        .count();
    assert_eq!(started, 0, "{rest:?}");
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_says_what_it_offers() {
    let world = paired().await;
    let options = world.session.agent_options("claude".into()).await.unwrap();
    assert_eq!(options.version.as_deref(), Some("9.9.9 (Fake Code)"));
    let modes: Vec<&str> = options.modes.iter().map(|mode| mode.id.as_str()).collect();
    assert_eq!(modes, ["acceptEdits", "plan", "bypassPermissions"]);
    assert!(options.modes[2].dangerous && !options.modes[0].dangerous);
    assert_eq!(options.default_mode.as_deref(), Some("default"));
    let models: Vec<(&str, &str, usize)> = options
        .models
        .iter()
        .map(|model| (model.id.as_str(), model.label.as_str(), model.efforts.len()))
        .collect();
    assert_eq!(models, [("default", "Default", 2), ("small", "Small", 0)]);
    assert!(options.questions && options.follow_ups && !options.assumed);

    // Codex's server is asked for its models; its modes are the protocol's.
    let codex = world.session.agent_options("codex".into()).await.unwrap();
    assert_eq!(codex.version.as_deref(), Some("codex-cli 9.9.9"));
    let modes: Vec<&str> = codex.modes.iter().map(|mode| mode.id.as_str()).collect();
    assert_eq!(modes, ["read-only", "auto", "untrusted", "full-access"]);
    assert_eq!(codex.default_mode.as_deref(), Some("auto"));
    let models: Vec<(&str, usize)> = codex
        .models
        .iter()
        .map(|model| (model.id.as_str(), model.efforts.len()))
        .collect();
    assert_eq!(models, [("big", 2)]);
    assert!(codex.questions && codex.follow_ups && !codex.live_changes && !codex.assumed);

    // opencode lists its models and its agents; one mode is this library's.
    let opencode = world
        .session
        .agent_options("opencode".into())
        .await
        .unwrap();
    assert_eq!(opencode.version.as_deref(), Some("9.9.9"));
    let modes: Vec<&str> = opencode.modes.iter().map(|mode| mode.id.as_str()).collect();
    assert_eq!(modes, ["build", "plan", "leon-auto"]);
    assert!(opencode.modes[2].dangerous);
    let models: Vec<&str> = opencode.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(models, ["fake/small", "fake/big"]);
    assert!(opencode.questions && !opencode.follow_ups && opencode.live_changes);
    assert!(!opencode.assumed);
    assert!(matches!(
        world.session.agent_options("vim".into()).await,
        Err(MobileError::UnknownAgent)
    ));
    world.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_code_and_a_code_nobody_shows_are_told_apart() {
    let relay = TestRelay::start().await;
    let dir = tempfile::tempdir().unwrap();
    let host = host_on(&relay.url(), dir.path()).await;
    let phone = dir.path().join("phone").to_string_lossy().into_owned();
    let mobile = LeonMobile::new(phone, "Ana's phone".into()).unwrap();

    let code = host.new_pairing_code().await.code;
    let last = code.chars().last().unwrap();
    let other = if last == 'A' { 'B' } else { 'A' };
    let wrong = format!("{}{other}", &code[..code.len() - 1]);
    assert_eq!(
        pair_when_shown(&mobile, &relay.url(), &wrong).await,
        Err(MobileError::WrongCode)
    );
    assert!(host.devices().is_empty());

    // The right one still works: one wrong attempt does not burn a code.
    let paired = mobile.pair(relay.url(), code).await.unwrap();
    assert_eq!(paired.relay_url, relay.url());

    let first = paired.host_id.chars().next().unwrap();
    let room = if first == 'Z' { "YYYY" } else { "ZZZZ" };
    let nobody = mobile.pair(relay.url(), format!("{room}-AAA-AAA")).await;
    assert!(
        matches!(
            nobody,
            Err(MobileError::CodeNotFound | MobileError::RelayRefused { .. })
        ),
        "{nobody:?}"
    );

    let unreachable = mobile
        .pair("ws://127.0.0.1:1".into(), "ABCD-EFG-HJK".into())
        .await;
    assert!(
        matches!(unreachable, Err(MobileError::RelayUnreachable { .. })),
        "{unreachable:?}"
    );
    host.stop();
    relay.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_session_says_so_and_a_revoked_phone_is_told() {
    let world = paired().await;
    world.host.revoke("Ana's phone").unwrap();
    loop {
        match world.states.recv_timeout(WAIT).expect("the state changes") {
            ConnectionState::Offline {
                reason: leon_mobile::OfflineReason::Revoked,
                ..
            } => break,
            _ => continue,
        }
    }
    world.session.close();
    loop {
        if world.states.recv_timeout(WAIT).expect("the session closes") == ConnectionState::Closed {
            break;
        }
    }
    let _ = &world.mobile;
    world.host.stop();
    world.relay.unwrap().stop().await;
}

/// King's service as far as a phone sees it: answers `status`, refuses what
/// it does not know, tells a subscriber a notice, and hangs up on `bye`.
struct FakeKing;

impl leon_host::KingDoor for FakeKing {
    fn open(&self) -> Result<leon_host::KingPipe, String> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        let (ours, theirs) = UnixStream::pair().map_err(|error| error.to_string())?;
        std::thread::spawn(move || {
            let mut answer = theirs.try_clone().unwrap();
            for line in BufReader::new(theirs).lines() {
                let Ok(line) = line else { break };
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_u64().unwrap();
                let reply = match request["op"].as_str() {
                    Some("bye") => break,
                    Some("status") => serde_json::json!({"id": id, "ok": true,
                        "result": {"brain": {"busy": false}}}),
                    Some("subscribe") => serde_json::json!({"id": id, "ok": true}),
                    _ => serde_json::json!({"id": id, "ok": false, "error": "unknown request"}),
                };
                writeln!(answer, "{reply}").unwrap();
                if request["op"] == "subscribe" {
                    writeln!(answer, r#"{{"notice":"notify","title":"PR opened"}}"#).unwrap();
                }
            }
        });
        let closer = ours.try_clone().unwrap();
        Ok(leon_host::KingPipe {
            write: Box::new(ours.try_clone().unwrap()),
            read: Box::new(BufReader::new(ours)),
            close: Box::new(move || {
                let _ = closer.shutdown(std::net::Shutdown::Both);
            }),
        })
    }
}

/// What a watch was told.
#[derive(Debug, PartialEq)]
enum Told {
    Notice(String),
    Closed,
}

struct Watching(Mutex<Sender<Told>>);

impl KingListener for Watching {
    fn on_notice(&self, notice: String) {
        let _ = self.0.lock().unwrap().send(Told::Notice(notice));
    }

    fn on_closed(&self, _reason: String) {
        let _ = self.0.lock().unwrap().send(Told::Closed);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_asks_king_and_hears_its_notices_again_after_the_line_closed() {
    let world = paired().await;
    assert!(world.session.share_state().await.unwrap().king);
    let status = world
        .session
        .king_ask(r#"{"op":"status"}"#.into())
        .await
        .unwrap();
    assert_eq!(status, r#"{"brain":{"busy":false}}"#);
    assert_eq!(
        world.session.king_ask(r#"{"op":"dance"}"#.into()).await,
        Err(MobileError::Host {
            message: "unknown request".into()
        })
    );

    let (tx, told) = channel();
    let watch = world.session.king_watch(Box::new(Watching(Mutex::new(tx))));
    let notice = Told::Notice(r#"{"notice":"notify","title":"PR opened"}"#.into());
    assert_eq!(told.recv_timeout(WAIT).unwrap(), notice);
    // King's service goes away: the watch says so and subscribes again.
    assert!(world
        .session
        .king_ask(r#"{"op":"bye"}"#.into())
        .await
        .is_err());
    assert_eq!(told.recv_timeout(WAIT).unwrap(), Told::Closed);
    assert_eq!(told.recv_timeout(WAIT).unwrap(), notice);
    watch.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_of_the_phone_is_kept_on_the_computer_whole_for_the_agent() {
    let world = paired().await;
    // Larger than one command's input: it goes in pieces.
    let bytes: Vec<u8> = (0..5 * 1024 * 1024 + 17)
        .map(|at| (at % 251) as u8)
        .collect();
    let path = world
        .session
        .keep_attachment("chat/1".into(), "my photo.png".into(), bytes.clone())
        .await
        .unwrap();
    assert!(
        path.contains("/leon-king-attachments/phone/chat-1/") && path.ends_with("-my-photo.png"),
        "{path}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(&path).unwrap();

    // Too large is said before anything is sent.
    let refused = world
        .session
        .keep_attachment("c".into(), "big.bin".into(), vec![0; 21 * 1024 * 1024])
        .await;
    assert!(
        matches!(refused, Err(MobileError::Host { .. })),
        "{refused:?}"
    );
    world.stop().await;
}

//! Gathering one machine's readings.
//!
//! Usage belongs to the account on the machine where the agent runs, so the
//! collection is per machine: one bounded command, run through the same
//! [`Runner`] as everything else, reads the files the agents already keep on
//! that machine (Codex's session log) and says which agents are there. A
//! source that needs a credential is not part of that command: it runs only on
//! this computer, only when its switch is on, and never copies a credential
//! anywhere (see [`crate::network`]). On a remote machine such a source is
//! reported as not supported; it is never run by sending a credential across.

use leon_core::{AgentKind, Machine, MachineKind};
use leon_remote::{run_on, CommandSpec, Runner, SshOptions};
use sha2::{Digest, Sha256};

use crate::codex;
use crate::forecast::Sample;
use crate::model::{AgentUsage, Collected, Reason, WindowKind};
use crate::network::{self, Credentials, Http, NetworkPolicy};

/// How many of the newest session logs are read, and how many token-count
/// lines of each.
const FILES: usize = 4;
const LINES: usize = 40;

/// What is printed for the agents' own files: which agents are there and the
/// newest limit observations of Codex. Everything it prints is a marker or a
/// line the parsers pick apart; it prints no credential, no path and no
/// conversation text beyond the token-count lines themselves.
fn script() -> String {
    format!(
        r#"PATH="$PATH:$HOME/.local/bin:$HOME/.opencode/bin:$HOME/.bun/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin"
for tool in claude codex opencode; do
  if command -v "$tool" >/dev/null 2>&1; then printf 'has=%s\n' "$tool"; fi
done
auth="${{XDG_DATA_HOME:-$HOME/.local/share}}/opencode/auth.json"
if [ -f "$auth" ] && grep -q '"opencode-go"' "$auth" 2>/dev/null; then printf 'go=opencode\n'; fi
sessions="${{CODEX_HOME:-$HOME/.codex}}/sessions"
if [ -d "$sessions" ]; then
  printf '@codex\n'
  for f in $(find "$sessions" -type f -name 'rollout-*.jsonl' 2>/dev/null | sort | tail -n {FILES}); do
    grep '"token_count"' "$f" 2>/dev/null | grep '"used_percent"' | tail -n {LINES}
  done
fi
exit 0"#
    )
}

/// The collection as a command for the machine.
pub fn collect_command() -> CommandSpec {
    CommandSpec::new("sh").args(["-c", &script()])
}

/// What the command printed, understood.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// Agents installed.
    pub has: Vec<AgentKind>,
    /// Whether opencode has a Go subscription key.
    pub go: bool,
    /// The lines after the `@codex` marker.
    pub codex_lines: String,
}

/// Reads the command's output. Unknown lines (login banners) are ignored.
pub fn parse_found(output: &str) -> Found {
    let mut found = Found::default();
    let mut in_codex = false;
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if line == "@codex" {
            in_codex = true;
            continue;
        }
        if in_codex {
            found.codex_lines.push_str(line);
            found.codex_lines.push('\n');
        } else if let Some(tool) = line.strip_prefix("has=") {
            if let Some(agent) = AgentKind::parse(tool) {
                found.has.push(agent);
            }
        } else if line == "go=opencode" {
            found.go = true;
        }
    }
    found
}

/// Whether the collection command can run on a machine: it needs a POSIX
/// shell, which every remote machine is assumed to have (see `leon-remote`)
/// and this computer has unless it is Windows.
pub fn can_collect(local: bool, windows: bool) -> bool {
    !(local && windows)
}

/// One machine's readings and the observations that came with them.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MachineUsage {
    /// One reading per agent, in display order.
    pub readings: Vec<AgentUsage>,
    /// Observations per agent and window, for the stored history.
    pub samples: Vec<(AgentKind, WindowKind, Sample)>,
}

/// A local hash standing in for the account in the stored history: the same
/// agent, machine and plan always give the same key, and nothing identifies
/// the account.
pub fn series_key(machine: &str, agent: AgentKind, plan: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(machine.as_bytes());
    hasher.update([0]);
    hasher.update(agent.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(plan.unwrap_or("").as_bytes());
    hasher
        .finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Collects the readings of `machine`.
///
/// The one command is run through `runner`; when it cannot be run every agent
/// is unknown ([`Reason::Unreachable`]). The sources that need a credential
/// run only for the local machine and only as `policy` allows.
pub async fn collect_machine<R: Runner>(
    runner: &R,
    machine: &Machine,
    ssh: &SshOptions,
    policy: NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    now: i64,
) -> MachineUsage {
    let id = machine.id.as_str();
    if !can_collect(machine.kind == MachineKind::Local, cfg!(windows)) {
        return MachineUsage {
            readings: AgentKind::ALL
                .iter()
                .map(|agent| AgentUsage::unknown(*agent, id, Reason::NotSupported))
                .collect(),
            samples: Vec::new(),
        };
    }
    let spec = run_on(machine, &collect_command(), ssh);
    let found = match runner.run(&spec).await {
        Ok(output) if output.success() => parse_found(&output.stdout),
        _ => {
            return MachineUsage {
                readings: AgentKind::ALL
                    .iter()
                    .map(|agent| AgentUsage::unknown(*agent, id, Reason::Unreachable))
                    .collect(),
                samples: Vec::new(),
            }
        }
    };
    let local = machine.kind == MachineKind::Local;
    let mut out = MachineUsage::default();
    for agent in AgentKind::ALL {
        if !found.has.contains(&agent)
            && !(agent == AgentKind::Codex && !found.codex_lines.is_empty())
        {
            out.readings
                .push(AgentUsage::unknown(agent, id, Reason::NotInstalled));
            continue;
        }
        let reading = match agent {
            AgentKind::Codex => {
                let Collected { usage, samples } =
                    codex::parse_rollout_lines(&found.codex_lines, id);
                out.samples.extend(
                    samples
                        .into_iter()
                        .map(|(kind, sample)| (agent, kind, sample)),
                );
                usage
            }
            AgentKind::Claude if !policy.claude => {
                AgentUsage::unknown(agent, id, Reason::SourceDisabled)
            }
            AgentKind::Claude if !local => AgentUsage::unknown(agent, id, Reason::NotSupported),
            AgentKind::Claude => network::claude_usage(policy, credentials, http, id, now).await,
            AgentKind::Opencode if !found.go => {
                AgentUsage::unknown(agent, id, Reason::NotSupported)
            }
            AgentKind::Opencode if !policy.opencode => {
                AgentUsage::unknown(agent, id, Reason::SourceDisabled)
            }
            AgentKind::Opencode if !local => AgentUsage::unknown(agent, id, Reason::NotSupported),
            AgentKind::Opencode => {
                network::opencode_usage(policy, credentials, http, id, now).await
            }
        };
        out.readings.push(reading);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Source, State};
    use crate::network::{Read, ScriptedHttp};
    use crate::secret::Secret;
    use leon_core::{MachineId, MachineKind};
    use leon_remote::{Output, ScriptedRunner};

    struct NoCredentials;
    impl Credentials for NoCredentials {
        fn claude(&self) -> Read<crate::claude::Credential> {
            Read::Missing
        }
        fn opencode_go(&self) -> Read<Secret> {
            Read::Missing
        }
    }

    fn local() -> Machine {
        Machine {
            id: MachineId::local(),
            name: "This computer".into(),
            kind: MachineKind::Local,
        }
    }

    fn remote() -> Machine {
        Machine {
            id: MachineId::from_string("box"),
            name: "box".into(),
            kind: MachineKind::Ssh {
                host: "box.example".into(),
                user: None,
                port: None,
                identity_file: None,
            },
        }
    }

    fn relayed() -> Machine {
        Machine {
            id: MachineId::from_string("relayed"),
            name: "relayed".into(),
            kind: MachineKind::Relay {
                host_id: "host-id".into(),
                host_key: "00".repeat(32),
                relay_url: "wss://relay.example".into(),
                name: "Their computer".into(),
            },
        }
    }

    const CODEX_LINE: &str = r#"{"timestamp":"2026-10-04T18:59:06.753Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":97.0,"window_minutes":300,"resets_at":1791154068},"secondary":{"used_percent":15.0,"window_minutes":10080,"resets_at":1791678919},"plan_type":"plus"}}}"#;

    fn output(has: &[&str], go: bool, codex: Option<&str>) -> Output {
        let mut text = String::from("Welcome banner\n");
        for tool in has {
            text.push_str(&format!("has={tool}\n"));
        }
        if go {
            text.push_str("go=opencode\n");
        }
        if let Some(lines) = codex {
            text.push_str("@codex\n");
            text.push_str(lines);
            text.push('\n');
        }
        Output::ok(text)
    }

    async fn run(
        machine: &Machine,
        out: Output,
        policy: NetworkPolicy,
        http: &ScriptedHttp,
    ) -> (MachineUsage, ScriptedRunner) {
        let runner = ScriptedRunner::new().reply(out);
        let got = collect_machine(
            &runner,
            machine,
            &SshOptions::without_multiplexing(),
            policy,
            &NoCredentials,
            http,
            1_791_000_000,
        )
        .await;
        (got, runner)
    }

    #[tokio::test]
    async fn one_command_per_machine_gives_a_reading_for_every_agent() {
        let http = ScriptedHttp::new();
        let (got, runner) = run(
            &local(),
            output(&["claude", "codex"], false, Some(CODEX_LINE)),
            NetworkPolicy::default(),
            &http,
        )
        .await;
        assert_eq!(runner.calls().len(), 1);
        assert_eq!(got.readings.len(), 3);
        let by = |a| got.readings.iter().find(|r| r.agent == a).unwrap();
        assert!(matches!(by(AgentKind::Codex).state, State::Known { .. }));
        assert_eq!(by(AgentKind::Codex).source, Some(Source::Local));
        assert_eq!(
            by(AgentKind::Claude).state,
            State::Unknown {
                reason: Reason::SourceDisabled
            }
        );
        assert_eq!(
            by(AgentKind::Opencode).state,
            State::Unknown {
                reason: Reason::NotInstalled
            }
        );
        assert_eq!(got.samples.len(), 2);
        assert_eq!(http.calls().len(), 0);
    }

    #[tokio::test]
    async fn a_remote_machine_runs_the_command_over_ssh_and_never_calls_a_vendor() {
        let http = ScriptedHttp::new();
        let policy = NetworkPolicy {
            claude: true,
            opencode: true,
        };
        let (got, runner) = run(
            &remote(),
            output(&["claude", "codex", "opencode"], true, Some(CODEX_LINE)),
            policy,
            &http,
        )
        .await;
        assert_eq!(runner.calls()[0].program, "ssh");
        let by = |a| got.readings.iter().find(|r| r.agent == a).unwrap();
        assert_eq!(
            by(AgentKind::Claude).state,
            State::Unknown {
                reason: Reason::NotSupported
            }
        );
        assert_eq!(
            by(AgentKind::Opencode).state,
            State::Unknown {
                reason: Reason::NotSupported
            }
        );
        assert!(matches!(by(AgentKind::Codex).state, State::Known { .. }));
        assert_eq!(got.readings[1].machine, "box");
        assert_eq!(http.calls().len(), 0);
    }

    #[tokio::test]
    async fn a_relay_machine_is_read_through_its_route_and_never_calls_a_vendor() {
        let http = ScriptedHttp::new();
        let policy = NetworkPolicy {
            claude: true,
            opencode: true,
        };
        let (got, runner) = run(
            &relayed(),
            output(&["claude", "codex"], false, Some(CODEX_LINE)),
            policy,
            &http,
        )
        .await;
        let call = &runner.calls()[0];
        assert!(
            call.route.is_some(),
            "the command must go through the relay"
        );
        let by = |a| got.readings.iter().find(|r| r.agent == a).unwrap();
        assert!(matches!(by(AgentKind::Codex).state, State::Known { .. }));
        assert_eq!(
            by(AgentKind::Claude).state,
            State::Unknown {
                reason: Reason::NotSupported
            }
        );
        assert_eq!(http.calls().len(), 0);
    }

    #[tokio::test]
    async fn opencode_without_a_go_key_has_nothing_to_read() {
        let http = ScriptedHttp::new();
        let policy = NetworkPolicy {
            claude: false,
            opencode: true,
        };
        let (got, _) = run(&local(), output(&["opencode"], false, None), policy, &http).await;
        let reading = got
            .readings
            .iter()
            .find(|r| r.agent == AgentKind::Opencode)
            .unwrap();
        assert_eq!(
            reading.state,
            State::Unknown {
                reason: Reason::NotSupported
            }
        );
        assert_eq!(http.calls().len(), 0);
    }

    #[tokio::test]
    async fn opencode_with_a_go_key_waits_for_its_switch() {
        let http = ScriptedHttp::new();
        let (got, _) = run(
            &local(),
            output(&["opencode"], true, None),
            NetworkPolicy::default(),
            &http,
        )
        .await;
        let reading = got
            .readings
            .iter()
            .find(|r| r.agent == AgentKind::Opencode)
            .unwrap();
        assert_eq!(
            reading.state,
            State::Unknown {
                reason: Reason::SourceDisabled
            }
        );
    }

    #[tokio::test]
    async fn codex_that_has_never_run_has_no_data_yet() {
        let http = ScriptedHttp::new();
        let (got, _) = run(
            &local(),
            output(&["codex"], false, None),
            NetworkPolicy::default(),
            &http,
        )
        .await;
        let reading = got
            .readings
            .iter()
            .find(|r| r.agent == AgentKind::Codex)
            .unwrap();
        assert_eq!(
            reading.state,
            State::Unknown {
                reason: Reason::NoData
            }
        );
    }

    #[tokio::test]
    async fn a_command_that_fails_makes_every_agent_unreachable() {
        let runner = ScriptedRunner::new().reply(Output::failed(255, "ssh: connect refused"));
        let got = collect_machine(
            &runner,
            &remote(),
            &SshOptions::without_multiplexing(),
            NetworkPolicy::default(),
            &NoCredentials,
            &ScriptedHttp::new(),
            1,
        )
        .await;
        assert_eq!(got.readings.len(), 3);
        assert!(got.readings.iter().all(|r| r.state
            == State::Unknown {
                reason: Reason::Unreachable
            }));
    }

    #[test]
    fn a_windows_computer_has_no_posix_shell_to_collect_with() {
        assert!(!can_collect(true, true));
        assert!(can_collect(true, false));
        assert!(
            can_collect(false, true),
            "a remote machine is POSIX wherever Leon runs"
        );
        assert!(can_collect(false, false));
    }

    #[test]
    fn the_command_prints_markers_and_no_credential() {
        let script = script();
        assert!(script.contains("grep -q '\"opencode-go\"'"));
        // `grep -q` is the only thing done with the auth file: nothing of it is printed.
        assert!(!script.contains("cat "));
        assert!(!script.contains("accessToken"));
    }

    #[test]
    fn found_lines_are_read_around_banners() {
        let found = parse_found(
            "Last login: x\nhas=codex\nhas=nope\ngo=opencode\n@codex\nline one\nline two\n",
        );
        assert_eq!(found.has, [AgentKind::Codex]);
        assert!(found.go);
        assert_eq!(found.codex_lines, "line one\nline two\n");
    }

    #[test]
    fn a_series_key_is_stable_short_and_names_nothing() {
        let a = series_key("local", AgentKind::Codex, Some("plus"));
        assert_eq!(a, series_key("local", AgentKind::Codex, Some("plus")));
        assert_ne!(a, series_key("local", AgentKind::Codex, Some("pro")));
        assert_ne!(a, series_key("box", AgentKind::Codex, Some("plus")));
        assert_eq!(a.len(), 12);
        assert!(!a.contains("local"));
    }
}

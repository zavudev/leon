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
//!
//! One source is the agent's own command: Antigravity's `agy -p /usage`. It
//! runs through the runner on whichever machine the agent is, after the
//! collection saw a version of `agy` that answers it without a model turn.

use std::time::Duration;

use leon_core::{AgentId, Machine, MachineKind};
use leon_remote::{run_on, CommandSpec, Runner, SshOptions};
use sha2::{Digest, Sha256};

use crate::forecast::Sample;
use crate::model::{AgentUsage, Collected, Reason, State, WindowKind};
use crate::network::{self, switchable_agents, Credentials, Http, NetworkPolicy};
use crate::{antigravity, codex};

/// How many of the newest session logs are read, and how many token-count
/// lines of each.
const FILES: usize = 4;
const LINES: usize = 40;

/// How long `agy` may take to answer `/usage` (its own limit is shorter).
const AGY_LIMIT: Duration = Duration::from_secs(30);

const PATH: &str = r#"PATH="$PATH:$HOME/.local/bin:$HOME/.opencode/bin:$HOME/.bun/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin""#;

/// What is printed for the agents' own files: which agents are there, the
/// version of `agy`, and the newest limit observations of Codex. Everything it
/// prints is a marker or a line the parsers pick apart; it prints no
/// credential, no path and no conversation text beyond the token-count lines
/// themselves.
fn script() -> String {
    let mut text = format!("{PATH}\n");
    for agent in switchable_agents() {
        let Some(spec) = agent.spec() else { continue };
        let names: Vec<String> = spec
            .detect
            .iter()
            .map(|name| format!("command -v {name} >/dev/null 2>&1"))
            .collect();
        text.push_str(&format!(
            "if {}; then printf 'has=%s\\n' {}; fi\n",
            names.join(" || "),
            agent.as_str()
        ));
    }
    text.push_str(&format!(
        r#"if command -v agy >/dev/null 2>&1; then printf 'agy=%s\n' "$(agy --version 2>/dev/null | head -n 1)"; fi
sessions="${{CODEX_HOME:-$HOME/.codex}}/sessions"
if [ -d "$sessions" ]; then
  printf '@codex\n'
  for f in $(find "$sessions" -type f -name 'rollout-*.jsonl' 2>/dev/null | sort | tail -n {FILES}); do
    grep '"token_count"' "$f" 2>/dev/null | grep '"used_percent"' | tail -n {LINES}
  done
fi
exit 0"#
    ));
    text
}

/// The collection as a command for the machine.
pub fn collect_command() -> CommandSpec {
    CommandSpec::new("sh").args(["-c", &script()])
}

/// The command that asks `agy` for its limits. It never carries
/// `--disable-slash-commands`, which would turn `/usage` into a model turn.
pub fn antigravity_command() -> CommandSpec {
    let args = antigravity::USAGE_ARGS.join(" ");
    CommandSpec::new("sh").args(["-c", &format!("{PATH}\nexec agy {args} 20s 2>&1")])
}

/// What the command printed, understood.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// Agents installed.
    pub has: Vec<AgentId>,
    /// The version of `agy`, when it printed one.
    pub agy: Option<(u32, u32, u32)>,
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
            // Only the agents that have a usage source are looked for; any
            // other line is noise.
            if let Some(agent) = AgentId::parse(tool).filter(|a| switchable_agents().contains(a)) {
                found.has.push(agent);
            }
        } else if let Some(version) = line.strip_prefix("agy=") {
            found.agy = antigravity::parse_version(version);
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

/// What one network source that was really called came to: the reason when it
/// failed or answered nothing, `None` when it gave a reading. The engine backs
/// the source off on a failure.
pub type Called = (AgentId, Option<Reason>);

/// One machine's readings and the observations that came with them.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MachineUsage {
    /// One reading per agent that has a usage provider, in catalogue order.
    pub readings: Vec<AgentUsage>,
    /// Observations per agent and window, for the stored history.
    pub samples: Vec<(AgentId, WindowKind, Sample)>,
    /// The network sources that were called on this computer, and what they
    /// came to.
    pub called: Vec<Called>,
}

/// A local hash standing in for the account in the stored history: the same
/// agent, machine and plan always give the same key, and nothing identifies
/// the account.
pub fn series_key(machine: &str, agent: AgentId, plan: Option<&str>) -> String {
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

fn all_unknown(id: &str, reason: Reason) -> MachineUsage {
    MachineUsage {
        readings: switchable_agents()
            .into_iter()
            .map(|agent| AgentUsage::unknown(agent, id, reason))
            .collect(),
        ..Default::default()
    }
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
    policy: &NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    now: i64,
) -> MachineUsage {
    collect_machine_on(
        runner,
        machine,
        !cfg!(windows),
        ssh,
        policy,
        credentials,
        http,
        now,
    )
    .await
}

/// [`collect_machine`] for a computer that has, or has not, a POSIX shell for
/// its own (`local_posix`: false on Windows). Only the local machine depends
/// on it: the decision follows the machine that is read, never the computer
/// Leon runs on, so a Windows client reads a Linux server like any other.
#[allow(clippy::too_many_arguments)] // `collect_machine` plus the one platform fact
pub async fn collect_machine_on<R: Runner>(
    runner: &R,
    machine: &Machine,
    local_posix: bool,
    ssh: &SshOptions,
    policy: &NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    now: i64,
) -> MachineUsage {
    let id = machine.id.as_str();
    if !can_collect(machine.kind == MachineKind::Local, !local_posix) {
        return all_unknown(id, Reason::NotSupported);
    }
    let spec = run_on(machine, &collect_command(), ssh);
    let found = match runner.run(&spec).await {
        Ok(output) if output.success() => parse_found(&output.stdout),
        _ => return all_unknown(id, Reason::Unreachable),
    };
    let local = machine.kind == MachineKind::Local;
    let mut out = MachineUsage::default();
    for agent in switchable_agents() {
        let installed = found.has.contains(&agent)
            || (agent == AgentId::CODEX && !found.codex_lines.is_empty());
        if !installed {
            out.readings
                .push(AgentUsage::unknown(agent, id, Reason::NotInstalled));
            continue;
        }
        let off = |reason| AgentUsage::unknown(agent, id, reason);
        let reading = match agent {
            AgentId::CODEX => {
                let Collected { usage, samples } =
                    codex::parse_rollout_lines(&found.codex_lines, id);
                out.samples.extend(
                    samples
                        .into_iter()
                        .map(|(kind, sample)| (agent, kind, sample)),
                );
                // The log is as fresh as the backend while Codex is in use;
                // when it is not, the backend may know more. A backend that
                // fails leaves the log's reading in place.
                let fresh = usage
                    .observed_at
                    .is_some_and(|at| now - at <= codex::LOG_FRESH);
                if policy.allows(agent) && local && !fresh {
                    let backend = network::codex_usage(policy, credentials, http, id, now).await;
                    let reason = match &backend.state {
                        State::Unknown { reason } => Some(*reason),
                        State::Known { windows } => {
                            out.samples.extend(windows.iter().map(|w| {
                                (
                                    agent,
                                    w.kind.clone(),
                                    Sample {
                                        at: now,
                                        used_percent: w.used_percent,
                                    },
                                )
                            }));
                            None
                        }
                    };
                    out.called.push((agent, reason));
                    if reason.is_none() {
                        out.readings.push(backend);
                        continue;
                    }
                }
                usage
            }
            AgentId::ANTIGRAVITY => {
                if !policy.allows(agent) {
                    off(Reason::SourceDisabled)
                } else if antigravity::latched(id) {
                    off(Reason::SpendsATurn)
                } else if !found.agy.is_some_and(antigravity::supports_usage) {
                    off(Reason::NotSupported)
                } else {
                    let spec = run_on(machine, &antigravity_command(), ssh);
                    match tokio::time::timeout(AGY_LIMIT, runner.run(&spec)).await {
                        Ok(Ok(output)) => {
                            let usage = antigravity::parse_output(&output.stdout, id, now);
                            if usage.state
                                == (State::Unknown {
                                    reason: Reason::SpendsATurn,
                                })
                            {
                                antigravity::latch(id);
                            }
                            usage
                        }
                        Ok(Err(_)) => off(Reason::Unreachable),
                        Err(_) => off(Reason::Offline),
                    }
                }
            }
            _ if !policy.allows(agent) => off(Reason::SourceDisabled),
            _ if !local => off(Reason::NotSupported),
            _ if network::has_network_source(agent) => {
                let usage = network::network_usage(agent, policy, credentials, http, id, now).await;
                let reason = match &usage.state {
                    State::Unknown { reason } => Some(*reason),
                    State::Known { .. } => None,
                };
                out.called.push((agent, reason));
                usage
            }
            _ => off(Reason::NotSupported),
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

    fn output(has: &[&str], codex: Option<&str>) -> Output {
        let mut text = String::from("Welcome banner\n");
        for tool in has {
            text.push_str(&format!("has={tool}\n"));
        }
        if let Some(lines) = codex {
            text.push_str("@codex\n");
            text.push_str(lines);
            text.push('\n');
        }
        Output::ok(text)
    }

    /// Collects as a computer with a POSIX shell of its own (the scripted
    /// runner needs no real shell), whatever computer the tests run on.
    async fn run(
        machine: &Machine,
        out: Output,
        policy: &NetworkPolicy,
        http: &ScriptedHttp,
    ) -> (MachineUsage, ScriptedRunner) {
        run_on_computer(machine, true, out, policy, http).await
    }

    async fn run_on_computer(
        machine: &Machine,
        local_posix: bool,
        out: Output,
        policy: &NetworkPolicy,
        http: &ScriptedHttp,
    ) -> (MachineUsage, ScriptedRunner) {
        let runner = ScriptedRunner::new().reply(out);
        let got = collect_machine_on(
            &runner,
            machine,
            local_posix,
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
            output(&["claude", "codex"], Some(CODEX_LINE)),
            &NetworkPolicy::default(),
            &http,
        )
        .await;
        assert_eq!(runner.calls().len(), 1);
        assert_eq!(got.readings.len(), switchable_agents().len());
        let by = |a| got.readings.iter().find(|r| r.agent == a).unwrap();
        assert!(matches!(by(AgentId::CODEX).state, State::Known { .. }));
        assert_eq!(by(AgentId::CODEX).source, Some(Source::Local));
        assert_eq!(
            by(AgentId::CLAUDE).state,
            State::Unknown {
                reason: Reason::SourceDisabled
            }
        );
        assert_eq!(
            by(AgentId::OPENCODE).state,
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
        let policy = NetworkPolicy::none()
            .with(AgentId::CLAUDE)
            .with(AgentId::OPENCODE);
        let (got, runner) = run(
            &remote(),
            output(&["claude", "codex", "opencode"], Some(CODEX_LINE)),
            &policy,
            &http,
        )
        .await;
        assert_eq!(runner.calls()[0].program, "ssh");
        let by = |a| got.readings.iter().find(|r| r.agent == a).unwrap();
        assert_eq!(
            by(AgentId::CLAUDE).state,
            State::Unknown {
                reason: Reason::NotSupported
            }
        );
        assert_eq!(
            by(AgentId::OPENCODE).state,
            State::Unknown {
                reason: Reason::NotSupported
            }
        );
        assert!(matches!(by(AgentId::CODEX).state, State::Known { .. }));
        assert_eq!(got.readings[1].machine, "box");
        assert_eq!(http.calls().len(), 0);
    }

    #[tokio::test]
    async fn a_relay_machine_is_read_through_its_route_and_never_calls_a_vendor() {
        let http = ScriptedHttp::new();
        let policy = NetworkPolicy::none()
            .with(AgentId::CLAUDE)
            .with(AgentId::OPENCODE);
        let (got, runner) = run(
            &relayed(),
            output(&["claude", "codex"], Some(CODEX_LINE)),
            &policy,
            &http,
        )
        .await;
        let call = &runner.calls()[0];
        assert!(
            call.route.is_some(),
            "the command must go through the relay"
        );
        let by = |a| got.readings.iter().find(|r| r.agent == a).unwrap();
        assert!(matches!(by(AgentId::CODEX).state, State::Known { .. }));
        assert_eq!(
            by(AgentId::CLAUDE).state,
            State::Unknown {
                reason: Reason::NotSupported
            }
        );
        assert_eq!(http.calls().len(), 0);
    }

    #[tokio::test]
    async fn opencode_without_any_key_has_nothing_to_read() {
        let http = ScriptedHttp::new();
        let policy = NetworkPolicy::none().with(AgentId::OPENCODE);
        let (got, _) = run(&local(), output(&["opencode"], None), &policy, &http).await;
        let reading = got
            .readings
            .iter()
            .find(|r| r.agent == AgentId::OPENCODE)
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
    async fn opencode_waits_for_its_switch() {
        let http = ScriptedHttp::new();
        let (got, _) = run(
            &local(),
            output(&["opencode"], None),
            &NetworkPolicy::default(),
            &http,
        )
        .await;
        let reading = got
            .readings
            .iter()
            .find(|r| r.agent == AgentId::OPENCODE)
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
            output(&["codex"], None),
            &NetworkPolicy::default(),
            &http,
        )
        .await;
        let reading = got
            .readings
            .iter()
            .find(|r| r.agent == AgentId::CODEX)
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
            &NetworkPolicy::default(),
            &NoCredentials,
            &ScriptedHttp::new(),
            1,
        )
        .await;
        assert_eq!(got.readings.len(), switchable_agents().len());
        assert!(got.readings.iter().all(|r| r.state
            == State::Unknown {
                reason: Reason::Unreachable
            }));
    }

    #[tokio::test]
    async fn on_windows_the_local_machine_is_unsupported_and_runs_nothing() {
        let http = ScriptedHttp::new();
        let (got, runner) = run_on_computer(
            &local(),
            false,
            output(&["claude", "codex"], Some(CODEX_LINE)),
            &NetworkPolicy::default(),
            &http,
        )
        .await;
        assert!(runner.calls().is_empty(), "no shell, so no command");
        assert_eq!(got.readings.len(), switchable_agents().len());
        assert!(got.readings.iter().all(|r| r.state
            == State::Unknown {
                reason: Reason::NotSupported
            }));
        assert!(got.samples.is_empty());
    }

    #[tokio::test]
    async fn on_windows_a_remote_machine_is_still_read_over_ssh() {
        let http = ScriptedHttp::new();
        let (got, runner) = run_on_computer(
            &remote(),
            false,
            output(&["codex"], Some(CODEX_LINE)),
            &NetworkPolicy::default(),
            &http,
        )
        .await;
        assert_eq!(runner.calls()[0].program, "ssh");
        let codex = got
            .readings
            .iter()
            .find(|r| r.agent == AgentId::CODEX)
            .unwrap();
        assert!(matches!(codex.state, State::Known { .. }));
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
        // The script never touches a credential file: opencode's key is read
        // by Leon itself, on this computer only.
        assert!(!script.contains("auth.json"));
        assert!(!script.contains("cat "));
        assert!(!script.contains("accessToken"));
    }

    #[test]
    fn found_lines_are_read_around_banners() {
        let found = parse_found("Last login: x\nhas=codex\nhas=nope\n@codex\nline one\nline two\n");
        assert_eq!(found.has, [AgentId::CODEX]);
        assert_eq!(found.codex_lines, "line one\nline two\n");
    }

    #[test]
    fn a_series_key_is_stable_short_and_names_nothing() {
        let a = series_key("local", AgentId::CODEX, Some("plus"));
        assert_eq!(a, series_key("local", AgentId::CODEX, Some("plus")));
        assert_ne!(a, series_key("local", AgentId::CODEX, Some("pro")));
        assert_ne!(a, series_key("box", AgentId::CODEX, Some("plus")));
        assert_eq!(a.len(), 12);
        assert!(!a.contains("local"));
    }

    struct CodexCreds;
    impl Credentials for CodexCreds {
        fn claude(&self) -> Read<crate::claude::Credential> {
            Read::Missing
        }
        fn opencode_go(&self) -> Read<Secret> {
            Read::Missing
        }
        fn codex(&self) -> Read<crate::codex::Credential> {
            crate::codex::parse_credential(r#"{"tokens":{"access_token":"t"}}"#)
                .map_or(Read::Missing, Read::Found)
        }
    }

    const BACKEND: &str = r#"{"plan_type":"pro","rate_limit":{"primary_window":{"used_percent":7,"limit_window_seconds":18000,"reset_at":1791300000}}}"#;

    async fn with_codex_backend(now: i64, http: &ScriptedHttp) -> (MachineUsage, ScriptedRunner) {
        let runner = ScriptedRunner::new().reply(output(&["codex"], Some(CODEX_LINE)));
        let got = collect_machine_on(
            &runner,
            &local(),
            true,
            &SshOptions::without_multiplexing(),
            &NetworkPolicy::none().with(AgentId::CODEX),
            &CodexCreds,
            http,
            now,
        )
        .await;
        (got, runner)
    }

    #[tokio::test]
    async fn a_stale_codex_log_is_refreshed_from_the_backend_when_that_source_is_on() {
        // The log line is from 2026-10-04T18:59Z; this is days later.
        let http = ScriptedHttp::new().reply(200, BACKEND);
        let (got, _) = with_codex_backend(1_791_500_000, &http).await;
        let codex = got
            .readings
            .iter()
            .find(|r| r.agent == AgentId::CODEX)
            .unwrap();
        assert_eq!(codex.source, Some(Source::VendorApi));
        assert_eq!(codex.plan.as_deref(), Some("pro"));
        assert_eq!(http.calls(), [crate::codex::BACKEND_URL]);
        assert_eq!(got.called, [(AgentId::CODEX, None)]);
        assert!(got.samples.iter().any(|(_, _, s)| s.at == 1_791_500_000));
    }

    #[tokio::test]
    async fn a_fresh_codex_log_is_enough_and_the_backend_is_not_called() {
        let http = ScriptedHttp::new().reply(200, BACKEND);
        // Two minutes after the log line (1791140346).
        let (got, _) = with_codex_backend(1_791_140_466, &http).await;
        let codex = got
            .readings
            .iter()
            .find(|r| r.agent == AgentId::CODEX)
            .unwrap();
        assert_eq!(codex.source, Some(Source::Local));
        assert!(http.calls().is_empty());
        assert!(got.called.is_empty());
    }

    #[tokio::test]
    async fn a_failing_backend_leaves_the_log_in_place_and_says_it_failed() {
        let http = ScriptedHttp::new().reply(500, "oops");
        let (got, _) = with_codex_backend(1_791_500_000, &http).await;
        let codex = got
            .readings
            .iter()
            .find(|r| r.agent == AgentId::CODEX)
            .unwrap();
        assert_eq!(codex.source, Some(Source::Local), "the log's reading stays");
        assert_eq!(
            got.called,
            [(AgentId::CODEX, Some(Reason::VendorError(500)))]
        );
    }

    #[tokio::test]
    async fn the_backend_is_never_asked_for_a_remote_machine() {
        let runner = ScriptedRunner::new().reply(output(&["codex"], Some(CODEX_LINE)));
        let http = ScriptedHttp::new().reply(200, BACKEND);
        let got = collect_machine_on(
            &runner,
            &remote(),
            true,
            &SshOptions::without_multiplexing(),
            &NetworkPolicy::all(),
            &CodexCreds,
            &http,
            1_791_500_000,
        )
        .await;
        assert!(http.calls().is_empty());
        assert!(got.called.is_empty());
    }

    #[tokio::test]
    async fn a_network_provider_runs_only_here_and_only_when_installed() {
        let http = ScriptedHttp::new();
        let (got, _) = run(
            &remote(),
            output(&["grok"], None),
            &NetworkPolicy::all(),
            &http,
        )
        .await;
        let by = |a| got.readings.iter().find(|r| r.agent == a).unwrap();
        assert_eq!(
            by(AgentId::GROK).state,
            State::Unknown {
                reason: Reason::NotSupported
            }
        );
        assert_eq!(
            by(AgentId::CURSOR).state,
            State::Unknown {
                reason: Reason::NotInstalled
            }
        );
        assert!(http.calls().is_empty());
        let (got, _) = run(
            &local(),
            output(&["grok"], None),
            &NetworkPolicy::all(),
            &http,
        )
        .await;
        let grok = got
            .readings
            .iter()
            .find(|r| r.agent == AgentId::GROK)
            .unwrap();
        assert_eq!(
            grok.state,
            State::Unknown {
                reason: Reason::NotSignedIn
            }
        );
        assert_eq!(got.called, [(AgentId::GROK, Some(Reason::NotSignedIn))]);
    }

    const AGY: &str = r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[{"name":"Gemini Models","buckets":[{"id":"w","name":"Weekly","window":"weekly","remaining_fraction":0.5}]}]}}}"#;

    fn agy_output(version: &str) -> Output {
        Output::ok(format!("has=antigravity\nagy={version}\n"))
    }

    #[tokio::test]
    async fn antigravity_is_asked_through_the_runner_on_any_machine_when_new_enough() {
        for machine in [local(), remote(), relayed()] {
            let runner = ScriptedRunner::new()
                .reply(agy_output("agy 1.2.11"))
                .reply(Output::ok(AGY));
            let got = collect_machine_on(
                &runner,
                &machine,
                true,
                &SshOptions::without_multiplexing(),
                &NetworkPolicy::none().with(AgentId::ANTIGRAVITY),
                &NoCredentials,
                &ScriptedHttp::new(),
                1_791_000_000,
            )
            .await;
            let agy = got
                .readings
                .iter()
                .find(|r| r.agent == AgentId::ANTIGRAVITY)
                .unwrap();
            assert_eq!(agy.source, Some(Source::Cli), "{:?}", machine.id);
            assert_eq!(runner.calls().len(), 2);
        }
    }

    #[tokio::test]
    async fn antigravity_is_not_asked_when_off_or_too_old_or_unreadable() {
        for (policy, version, reason) in [
            (NetworkPolicy::none(), "agy 1.2.11", Reason::SourceDisabled),
            (NetworkPolicy::all(), "agy 1.1.10", Reason::NotSupported),
            (NetworkPolicy::all(), "", Reason::NotSupported),
        ] {
            let runner = ScriptedRunner::new().reply(agy_output(version));
            let got = collect_machine_on(
                &runner,
                &local(),
                true,
                &SshOptions::without_multiplexing(),
                &policy,
                &NoCredentials,
                &ScriptedHttp::new(),
                1,
            )
            .await;
            let agy = got
                .readings
                .iter()
                .find(|r| r.agent == AgentId::ANTIGRAVITY)
                .unwrap();
            assert_eq!(agy.state, State::Unknown { reason }, "{version:?}");
            assert_eq!(runner.calls().len(), 1, "the command was not run");
        }
    }

    #[test]
    fn the_script_looks_for_every_agent_with_a_usage_provider_and_the_command_is_safe() {
        let script = script();
        for agent in switchable_agents() {
            assert!(
                script.contains(&format!("printf 'has=%s\\n' {}", agent.as_str())),
                "{agent}"
            );
        }
        assert!(script.contains("kimi-code"), "aliases are detected too");
        let call = antigravity_command().args.join(" ");
        assert!(call.contains("-p /usage --output-format json"));
        assert!(!call.contains("disable-slash-commands"));
    }

    #[tokio::test]
    async fn an_agy_that_answers_with_a_model_turn_is_never_asked_again() {
        let mut machine = remote();
        machine.id = MachineId::from_string("latch-box");
        antigravity::unlatch("latch-box");
        let prompt =
            r#"{"status":"SUCCESS","response":"Hello","num_turns":1,"conversation_id":"abc"}"#;
        let policy = NetworkPolicy::none().with(AgentId::ANTIGRAVITY);
        let runner = ScriptedRunner::new()
            .reply(agy_output("agy 1.2.11"))
            .reply(Output::ok(prompt))
            .reply(agy_output("agy 1.2.11"));
        for round in 0..2 {
            let got = collect_machine_on(
                &runner,
                &machine,
                true,
                &SshOptions::without_multiplexing(),
                &policy,
                &NoCredentials,
                &ScriptedHttp::new(),
                1_791_000_000,
            )
            .await;
            let agy = got
                .readings
                .iter()
                .find(|r| r.agent == AgentId::ANTIGRAVITY)
                .unwrap();
            assert_eq!(
                agy.state,
                State::Unknown {
                    reason: Reason::SpendsATurn
                },
                "round {round}"
            );
        }
        // The first round ran the collection and the command; the second only
        // the collection: the command that spends a turn was not run again.
        assert_eq!(runner.calls().len(), 3);
        antigravity::unlatch("latch-box");
    }
}

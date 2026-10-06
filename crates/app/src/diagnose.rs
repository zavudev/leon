//! The hidden `--diagnose terminal` run: a program in a pseudo-terminal
//! without a window.
//!
//! Screenshots cannot show whether the terminal works, so this does what the
//! window does, minus the drawing: it spawns the program in a PTY with the
//! application's own terminal model, types the given input once the program
//! has started, waits for it to end, and prints what its screen shows and how
//! it ended. It is how the emulator, the PTY and the exit handling are
//! checked against a real agent (`--diagnose terminal -- claude --version`).
//!
//! `--diagnose resume` checks the other half without a window: it reads the
//! real local history of an agent into a throwaway in-memory store (nothing
//! is written to the agent's files or to Leon's database), takes the newest
//! session whose folder still exists, builds the very plan opening it makes
//! (`launch::plan`), and prints only what would be started: the program, the
//! folder and the line that would be typed into the shell. It starts nothing,
//! so no real session is resumed or written to.
//!
//! `--diagnose sessions-elsewhere` runs the very scan the engine runs (one
//! command through the real runner) over the real history in a throwaway
//! store, and prints for each agent process its pid, agent, start, whether it
//! runs below a running Leon, the session id it holds (ids only: no titles,
//! no content), how sure that is and which signal said so, then how long the
//! scan took.
//!
//! `--diagnose usage` runs the engine's own collection of the agents' usage
//! limits for this computer and prints, per agent, the source, the windows
//! with their percentages and reset times, how fresh they are, or why nothing
//! is known. It prints nothing that identifies an account or a place (no
//! account, no e-mail, no token, no path), and it calls a network source only
//! when `--network <agent>` names it.
//!
//! `--diagnose update` makes the real update check against the GitHub
//! releases (`leon_update::diagnose`) and prints the running version, the
//! latest release, the file it would pick for this platform with its size,
//! whether `SHA256SUMS` lists it, and the decision. `--pretend-version`
//! compares with another version, and `--download` fetches, verifies and
//! unpacks the file into a folder of its own, installing nothing.

use crate::cli::Diagnose;
use crate::launch::{self, Launch, RealSystem, System};
use leon_core::{AgentId, MachineId, SessionFilter, Store};
use leon_remote::SshOptions;
use leon_term::{GridSize, SpawnSpec, Terminal};
use std::time::{Duration, Instant};

/// How long to wait for a program's first output before typing anyway.
const FIRST_OUTPUT_WAIT: Duration = Duration::from_secs(3);

/// How long the screen may still change after the program has ended.
const SETTLE: Duration = Duration::from_millis(300);

/// The bytes a typed text stands for: `\n`, `\r`, `\t`, `\e`, `\\` and
/// `\xNN` are escapes, everything else is itself.
pub fn unescape(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buffer = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('e') => out.push(0x1b),
            Some('\\') => out.push(b'\\'),
            Some('x') => {
                let hex: String = (0..2)
                    .filter_map(|_| chars.next_if(char::is_ascii_hexdigit))
                    .collect();
                match u8::from_str_radix(&hex, 16) {
                    Ok(byte) if hex.len() == 2 => out.push(byte),
                    _ => {
                        out.extend_from_slice(b"\\x");
                        out.extend_from_slice(hex.as_bytes());
                    }
                }
            }
            Some(other) => {
                out.push(b'\\');
                let mut buffer = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
}

/// Asks for the system's folder dialog through the very function every
/// "Open project" entry point uses, and reports that the call was reached.
/// The dialog is left up for `timeout` seconds (nobody is there to choose a
/// folder) and the run then ends. Opens no window of its own.
pub fn run_folder_dialog(timeout: u64) -> i32 {
    println!("leon diagnose folder-dialog");
    gpui_kit::application().run(move |cx: &mut gpui_kit::App| {
        let asked = Instant::now();
        println!("calling the system folder dialog (GPUI prompt_for_paths)...");
        let picking = crate::ui::system_picker(cx);
        println!("prompt_for_paths was called: the dialog is on screen");
        cx.spawn(async move |cx| {
            let ended = cx.background_executor().timer(Duration::from_secs(timeout));
            futures_lite_select(picking, ended, timeout).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
        let _ = asked;
    });
    0
}

/// Waits for whichever of the dialog's answer and the timer comes first and
/// says which.
async fn futures_lite_select(
    picking: gpui_kit::Task<crate::ui::Picked>,
    ended: gpui_kit::Task<()>,
    timeout: u64,
) {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::Poll;
    let mut picking = picking;
    let mut ended = ended;
    std::future::poll_fn(|cx| {
        if let Poll::Ready(answer) = Pin::new(&mut picking).poll(cx) {
            println!("the dialog answered: {answer:?}");
            return Poll::Ready(());
        }
        if Pin::new(&mut ended).poll(cx).is_ready() {
            println!("no answer after {timeout} s; the dialog was reached and is closed now");
            return Poll::Ready(());
        }
        Poll::Pending
    })
    .await;
}

/// The program to run and what to type into it: the request's own, or a shell
/// echoing one line.
pub fn plan(args: &Diagnose, system: &dyn System) -> (SpawnSpec, Vec<u8>) {
    match args.command.split_first() {
        Some((program, rest)) => {
            let found = if program.contains(['/', '\\']) {
                program.clone()
            } else {
                system.find_program(program).map_or_else(
                    || program.clone(),
                    |path| path.to_string_lossy().into_owned(),
                )
            };
            let spec = SpawnSpec {
                program: found,
                args: rest.to_vec(),
                env: Vec::new(),
                cwd: None,
                route: None,
            };
            (
                spec,
                args.input.as_deref().map(unescape).unwrap_or_default(),
            )
        }
        None => {
            let spec = if crate::platform::is_windows() {
                SpawnSpec {
                    program: "cmd".into(),
                    args: vec!["/c".into(), "set /p x= & echo echo:%x%".into()],
                    env: Vec::new(),
                    cwd: None,
                    route: None,
                }
            } else {
                SpawnSpec {
                    program: "sh".into(),
                    args: vec!["-c".into(), r#"read x; printf 'echo:%s\n' "$x""#.into()],
                    env: Vec::new(),
                    cwd: None,
                    route: None,
                }
            };
            let input = args
                .input
                .as_deref()
                .map_or_else(|| b"hello\r".to_vec(), unescape);
            (spec, input)
        }
    }
}

/// What opening the newest history session of `agent` on this computer would
/// start, as the lines `--diagnose resume` prints: the program, the folder and
/// the typed line. Nothing of the session but its id (inside the typed line)
/// is shown.
pub fn resume_lines(
    store: &Store,
    agent: AgentId,
    system: &dyn System,
) -> Result<Vec<String>, String> {
    let filter = SessionFilter {
        agent: Some(agent),
        machine_id: Some(MachineId::local()),
        project_id: None,
    };
    let sessions = store
        .recent_sessions(&filter, 500)
        .map_err(|e| e.to_string())?;
    let total = sessions.len();
    let session = sessions
        .into_iter()
        .find(|session| system.dir_exists(&session.cwd))
        .ok_or_else(|| {
            format!(
                "No {} session on this computer has a folder that still exists ({total} found).",
                crate::format::agent_name(agent)
            )
        })?;
    let machine = store
        .machine(&MachineId::local())
        .map_err(|e| e.to_string())?;
    let launch = Launch::Agent {
        kind: agent,
        resume: Some(session.external_id.clone()),
    };
    let plan = launch::plan(
        &machine,
        None,
        &session.cwd,
        &launch,
        &SshOptions::without_multiplexing(),
        system,
    )
    .map_err(|error| error.to_string())?;
    let spawn = std::iter::once(plan.spawn.program.as_str())
        .chain(plan.spawn.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    let typed = plan.send.unwrap_or_default();
    Ok(vec![
        format!("spawn: {spawn}"),
        format!("cwd: {}", plan.spawn.cwd.unwrap_or_default()),
        format!("typed: {}", typed.trim_end_matches('\r')),
        "then: Enter (\\r)".to_owned(),
    ])
}

/// `--diagnose resume`: imports the real local history of `agent` into memory
/// and prints [`resume_lines`]. Exit code 0 when a session was found, 1 when
/// none could be resumed.
pub fn run_resume(agent: AgentId) -> i32 {
    println!(
        "leon diagnose resume ({})",
        crate::format::agent_name(agent)
    );
    let store = match Store::open_in_memory() {
        Ok(store) => store,
        Err(error) => {
            println!("cannot open a store: {error}");
            return 1;
        }
    };
    leon_history::Importer::run(&store, &MachineId::local(), &leon_history::default_roots());
    match resume_lines(&store, agent, &RealSystem) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            0
        }
        Err(why) => {
            println!("{why}");
            1
        }
    }
}

/// The report of `--diagnose sessions-elsewhere`, as lines: one per agent
/// process. `leon_roots` are the pids of running Leons; what runs below one is
/// Leon's own.
fn elsewhere_lines(
    scan: &leon_remote::processes::Scan,
    found: &[crate::elsewhere::Found],
    leon_roots: &[u32],
    took: Duration,
) -> Vec<String> {
    let mut lines = vec![format!(
        "{} agent process{} found",
        found.len(),
        if found.len() == 1 { "" } else { "es" }
    )];
    for process in found {
        let owner = leon_roots
            .iter()
            .find(|root| scan.descends_from(process.pid, **root));
        let started = process
            .started
            .and_then(|at| chrono::DateTime::from_timestamp(at, 0))
            .map(|at| {
                at.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|| "unknown".to_owned());
        let held = match (&process.external_id, process.link) {
            (Some(id), Some(link)) => format!(
                "session {id} \u{b7} {} \u{b7} signal: {}",
                match link.confidence {
                    crate::elsewhere::Confidence::Certain => "certain",
                    crate::elsewhere::Confidence::Likely => "likely",
                },
                link.signal.label()
            ),
            _ => "session: none (no signal ties it to one)".to_owned(),
        };
        lines.push(format!(
            "pid {} \u{b7} {} \u{b7} started {started} \u{b7} {} \u{b7} {held}",
            process.pid,
            crate::format::agent_name(process.agent),
            match owner {
                Some(root) => format!("Leon descendant (running Leon pid {root}): its own"),
                None => "not a Leon descendant: elsewhere".to_owned(),
            },
        ));
    }
    lines.push(format!("scan took {} ms", took.as_millis()));
    lines
}

/// Runs `--diagnose connect`: the checklist of the Connect screen against a
/// real destination, printed line by line, with the diagnosis of what failed.
/// Only an SSH connection in batch mode is attempted (and `ssh-keyscan` when
/// the host key is unknown); nothing is written anywhere.
pub fn run_connect(destination: &str) -> i32 {
    use leon_remote::connect::{CheckState, Target};
    println!("leon diagnose connect {destination}");
    let parsed = match crate::address::parse_destination(destination) {
        Ok(parsed) => parsed,
        Err(error) => {
            println!("not a destination: {error}");
            return 2;
        }
    };
    let target = Target {
        host: parsed.host,
        user: parsed.user,
        port: parsed.port,
        identity: None,
    };
    println!("command: {}", target.ssh_line());
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            println!("cannot start a runtime: {error}");
            return 1;
        }
    };
    let started = Instant::now();
    let runner = leon_remote::ProcessRunner::new();
    let list = runtime.block_on(leon_remote::connect::run_checks(&runner, &target, |_| {}));
    for check in &list.checks {
        let word = match check.state {
            CheckState::Passed => "PASS",
            CheckState::Warned => "NOTE",
            CheckState::Failed => "FAIL",
            CheckState::Skipped => "SKIP",
            CheckState::Running | CheckState::Pending => "....",
        };
        println!("[{word}] {}: {}", check.id.label(), check.note);
        if let Some(diagnosis) = &check.diagnosis {
            println!("       what:  {}", diagnosis.explanation);
            println!("       fix:   {}", diagnosis.fix);
        }
    }
    if let Some(keys) = &list.host_keys {
        for key in &keys.keys {
            println!("host key: {} {}", key.kind, key.fingerprint);
        }
    }
    if !list.raw.is_empty() {
        println!("details: {}", list.raw.replace('\n', "\n         "));
    }
    println!(
        "{} in {} ms",
        if list.connected() {
            "connected"
        } else {
            "not connected"
        },
        started.elapsed().as_millis()
    );
    i32::from(!list.connected())
}

/// Runs `--diagnose sessions-elsewhere`: the real scan of this computer.
pub fn run_sessions_elsewhere() -> i32 {
    use leon_remote::Runner;
    println!("leon diagnose sessions-elsewhere");
    let store = match Store::open_in_memory() {
        Ok(store) => store,
        Err(error) => {
            println!("cannot open a store: {error}");
            return 1;
        }
    };
    leon_history::Importer::run(&store, &MachineId::local(), &leon_history::default_roots());
    let started = Instant::now();
    let spec = leon_remote::processes::scan_command(crate::platform::local_has_posix_shell());
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            println!("cannot start a runtime: {error}");
            return 1;
        }
    };
    let output = runtime
        .block_on(leon_remote::ProcessRunner::with_time_limit(Duration::from_secs(30)).run(&spec));
    let output = match output {
        Ok(output) if output.success() => output,
        Ok(output) => {
            println!(
                "the scan failed (status {:?}): {}",
                output.status,
                output.stderr.trim()
            );
            return 1;
        }
        Err(error) => {
            println!("the scan could not run: {error}");
            return 1;
        }
    };
    let scan = leon_remote::processes::parse_scan(&output.stdout);
    let sessions = store
        .recent_sessions(
            &leon_core::SessionFilter {
                agent: None,
                machine_id: Some(MachineId::local()),
                project_id: None,
            },
            100_000,
        )
        .unwrap_or_default();
    // Leon's own are told by the process tree: below any running `leon`.
    let found = crate::elsewhere::resolve(&scan, &sessions, None);
    let roots: Vec<u32> = scan
        .pids_named("leon")
        .into_iter()
        .filter(|pid| *pid != std::process::id())
        .collect();
    for line in elsewhere_lines(&scan, &found, &roots, started.elapsed()) {
        println!("{line}");
    }
    0
}

/// The report of `--diagnose usage` for one machine's readings, one block per
/// agent. Nothing in it identifies an account or a place.
pub fn usage_lines(collected: &leon_usage::MachineUsage, now: i64) -> Vec<String> {
    use leon_usage::{view, Body, Thresholds};
    let mut lines = Vec::new();
    for reading in &collected.readings {
        let v = view(reading, now, Thresholds::default());
        let name = crate::ui::agent_display_name(reading.agent);
        match &v.body {
            Body::Unknown(reason) => {
                lines.push(format!("{name}: unknown: {}", reason.short()));
                lines.push(format!("    {}", reason.text()));
            }
            Body::Ready { meters, .. } => {
                let plan = v
                    .plan
                    .as_deref()
                    .map(|p| format!(", plan {p}"))
                    .unwrap_or_default();
                lines.push(format!("{name}: {}{plan}", v.provenance()));
                for meter in meters {
                    let reset = meter
                        .reset_text()
                        .map(|text| format!(", {}", text.to_lowercase()))
                        .unwrap_or_default();
                    lines.push(format!(
                        "    {:<14} {:>3}% used{reset}",
                        meter.kind.long(),
                        leon_usage::percent_round(meter.percent)
                    ));
                }
                if let Some(footer) = crate::ui::footer_text(
                    reading,
                    now,
                    Thresholds::default(),
                    leon_usage::PercentDisplay::Used,
                ) {
                    lines.push(format!("    footer: {footer}"));
                }
            }
        }
    }
    lines
}

/// Which network sources a `--diagnose usage` run calls: what the settings
/// file says (on by default) with the run's own overrides on top.
pub fn usage_policy_of(
    file: &std::path::Path,
    network: &[AgentId],
    no_network: &[AgentId],
) -> leon_usage::network::NetworkPolicy {
    let store = std::fs::read(file)
        .ok()
        .and_then(|bytes| crate::schema::Store::parse(&bytes).ok())
        .unwrap_or_default();
    let from_settings = |key: &str| matches!(store.value(key), crate::schema::Value::Bool(true));
    let mut policy = leon_usage::network::NetworkPolicy::none();
    for agent in leon_usage::network::switchable_agents() {
        let on = if no_network.contains(&agent) {
            false
        } else {
            network.contains(&agent) || from_settings(&crate::schema::usage_setting(agent, true))
        };
        policy.set(agent, on);
    }
    policy
}

/// How an agent's usage is read, in a few words, and whether that has been
/// checked against the live service.
pub fn usage_source_text(agent: AgentId) -> (&'static str, bool) {
    match agent {
        AgentId::CLAUDE => ("Anthropic's usage endpoint", true),
        AgentId::CODEX => (
            "its session log, then OpenAI's backend when the log is old",
            true,
        ),
        AgentId::OPENCODE => ("the opencode Go usage endpoint", false),
        AgentId::GROK => ("xAI's billing endpoint", false),
        AgentId::CURSOR => ("cursor.com's usage summary", false),
        AgentId::KIMI => ("Moonshot's usages endpoint", false),
        AgentId::ZCODE => ("the GLM Coding Plan quota endpoint", false),
        AgentId::ANTIGRAVITY => ("its own `agy -p /usage` command", false),
        _ => ("an unknown source", false),
    }
}

/// Runs `--diagnose usage`: the real collection for this computer, through the
/// real runner. A network source is called as the settings file says (on by
/// default), `network` and `no_network` overriding it for this run.
pub fn run_usage(
    network: &[AgentId],
    no_network: &[AgentId],
    settings_file: &std::path::Path,
) -> i32 {
    use leon_usage::network::{CurlHttp, SystemCredentials};
    println!("leon diagnose usage");
    let policy = usage_policy_of(settings_file, network, no_network);
    for agent in leon_usage::network::switchable_agents() {
        let (source, verified) = usage_source_text(agent);
        println!(
            "source of {}: {source}; {}{}",
            crate::ui::agent_display_name(agent),
            if policy.allows(agent) {
                "on (settings, or --network)"
            } else {
                "off (settings, or --no-network)"
            },
            if verified {
                ""
            } else {
                "; implemented from Orca's reference, unverified against the live service"
            }
        );
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            println!("cannot start a runtime: {error}");
            return 1;
        }
    };
    let machine = leon_core::Machine {
        id: MachineId::local(),
        name: "This computer".to_owned(),
        kind: leon_core::MachineKind::Local,
    };
    let now = chrono::Utc::now().timestamp();
    let started = Instant::now();
    let collected = runtime.block_on(leon_usage::collect_machine(
        &leon_remote::ProcessRunner::with_time_limit(Duration::from_secs(30)),
        &machine,
        &SshOptions::without_multiplexing(),
        &policy,
        &SystemCredentials {
            home: dirs::home_dir().unwrap_or_default(),
        },
        &CurlHttp,
        now,
    ));
    for line in usage_lines(&collected, now) {
        println!("{line}");
    }
    println!("took {} ms", started.elapsed().as_millis());
    0
}

/// Runs the diagnostic and prints its report. The exit code is 0 when the
/// program ran to its end, whatever its own code was, and 1 when it could not
/// be started or did not end in time.
pub fn run(args: &Diagnose) -> i32 {
    run_with(args, &RealSystem, SETTLE)
}

/// [`run`] on a given computer, with a given settling time after the end.
fn run_with(args: &Diagnose, system: &dyn System, settle: Duration) -> i32 {
    let (spec, input) = plan(args, system);
    let line = std::iter::once(spec.program.as_str())
        .chain(spec.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    println!("leon diagnose terminal");
    println!("command: {line}");
    let size = GridSize::new(100, 30);
    println!("grid: {}x{}", size.cols, size.rows);
    let terminal = match Terminal::spawn(
        &spec,
        size,
        crate::theme::ThemeId::DEFAULT
            .palette(crate::theme::Appearance::Dark)
            .terminal,
        || {},
    ) {
        Ok(terminal) => terminal,
        Err(error) => {
            println!("--- spawn failed ---\n{error}");
            return 1;
        }
    };

    let started = Instant::now();
    while !terminal.has_output()
        && terminal.exit_info().is_none()
        && started.elapsed() < FIRST_OUTPUT_WAIT
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !input.is_empty() && terminal.exit_info().is_none() {
        println!("input: {:?}", String::from_utf8_lossy(&input));
        terminal.write(input);
    }
    let deadline = started + Duration::from_secs(args.timeout);
    while terminal.exit_info().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let exit = terminal.exit_info();
    if exit.is_some() {
        // The last of the output may still be on its way to the grid.
        std::thread::sleep(settle);
    }
    println!("--- screen ---");
    println!("{}", terminal.screen_text());
    println!("--- end of screen ---");
    if let Some(title) = terminal.title() {
        println!("title: {title}");
    }
    match exit {
        Some(info) => {
            match &info.signal {
                Some(signal) => println!("exit: code {} (signal {signal})", info.code),
                None => println!("exit: code {}", info.code),
            }
            0
        }
        None => {
            println!("exit: still running after {} s; it is killed", args.timeout);
            1
        }
    }
}

/// Runs `--diagnose update`: the real check, and with `--download` the real
/// download into a scratch folder, verified and unpacked, never installed.
pub fn run_update(request: &crate::cli::UpdateDiagnose) -> i32 {
    use leon_update::{Install, Platform, SystemTools, Tools};
    println!("leon diagnose update");
    if let Some(file) = &request.check_archive {
        return match leon_update::diagnose::check_archive(file, &SystemTools) {
            Ok(lines) => {
                for line in lines {
                    println!("{line}");
                }
                0
            }
            Err(error) => {
                println!("{error}");
                1
            }
        };
    }
    let running = crate::product::VERSION;
    println!("running: {} {running}", crate::product::PRODUCT_NAME);
    let install = leon_update::install::detect();
    println!(
        "this install: {}",
        match &install {
            Install::Updatable(target) => format!(
                "{} is replaced in place by an update ({})",
                target.path.display(),
                match target.kind {
                    leon_update::package::Kind::Bundle => "an application bundle",
                    leon_update::package::Kind::File => "a program file",
                }
            ),
            Install::Manual(why) => format!("not replaced by Leon: {}", why.explain()),
        }
    );
    let current = request.pretend_version.as_deref().unwrap_or(running);
    if request.pretend_version.is_some() {
        println!("pretending to run version {current} for the comparison");
    }
    let Some(current) = leon_update::version::parse_running(current) else {
        println!("{current:?} is not a version");
        return 1;
    };
    let platform = match request.platform.as_deref() {
        Some(id) => Platform::parse(id).unwrap_or_else(Platform::current),
        None => Platform::current(),
    };
    let tools = SystemTools;
    let running_signature = match install.target() {
        Some(target) => tools.signature(&target.path).unwrap_or_default(),
        None => leon_update::tools::Signature::none(),
    };
    let download_to = request.download.then(|| {
        request
            .dir
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("leon-update-diagnose"))
    });
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            println!("cannot start a runtime: {error}");
            return 1;
        }
    };
    let report = runtime.block_on(leon_update::diagnose::run(
        &leon_update::CurlHttp::new(),
        &tools,
        &leon_update::diagnose::Request {
            current,
            prereleases: request.prerelease,
            platform,
            download_to,
            running: running_signature,
        },
    ));
    for line in &report.lines {
        println!("{line}");
    }
    i32::from(!report.ok)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_usage_report_follows_the_settings_file_and_the_overrides() {
        use leon_core::AgentId;
        let (claude, opencode) = (AgentId::CLAUDE, AgentId::OPENCODE);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        // No file: the defaults, on.
        let policy = super::usage_policy_of(&file, &[], &[]);
        assert!(policy.allows(claude) && policy.allows(opencode));
        assert!(
            policy.allows(AgentId::GROK),
            "every provider is on by default"
        );
        std::fs::write(&file, br#"{"usage_claude_network": false}"#).unwrap();
        let policy = super::usage_policy_of(&file, &[], &[]);
        assert!(
            !policy.allows(claude) && policy.allows(opencode),
            "the file's off is kept"
        );
        assert!(
            super::usage_policy_of(&file, &[claude], &[]).allows(claude),
            "--network wins"
        );
        let policy = super::usage_policy_of(&file, &[], &[opencode]);
        assert!(!policy.allows(opencode), "--no-network wins");
    }

    #[test]
    fn the_usage_report_names_the_source_the_windows_and_the_reasons_and_nothing_else() {
        use leon_usage::{
            AgentUsage, MachineUsage, Reason, Source, State, UsageWindow, WindowKind,
        };
        const NOW: i64 = 1_790_000_000;
        let known = AgentUsage {
            agent: AgentId::CODEX,
            machine: "local".into(),
            account_label: Some("secret-label@example.com".into()),
            plan: Some("plus".into()),
            source: Some(Source::Local),
            observed_at: Some(NOW - 180),
            state: State::Known {
                windows: vec![
                    UsageWindow {
                        kind: WindowKind::FiveHour,
                        used_percent: 38.0,
                        resets_at: Some(NOW + 8940),
                        window_length: None,
                    },
                    UsageWindow {
                        kind: WindowKind::Weekly,
                        used_percent: 97.0,
                        resets_at: Some(NOW - 5),
                        window_length: None,
                    },
                ],
            },
        };
        let collected = MachineUsage {
            called: Vec::new(),
            readings: vec![
                AgentUsage::unknown(AgentId::CLAUDE, "local", Reason::SourceDisabled),
                known,
            ],
            samples: vec![],
        };
        let text = super::usage_lines(&collected, NOW).join("\n");
        assert!(text.contains("Claude Code: unknown: source off"), "{text}");
        assert!(
            text.contains("Codex: from Codex's own session log, 3 min ago, plan plus"),
            "{text}"
        );
        assert!(text.contains("38% used, resets in 2h 29m"), "{text}");
        assert!(
            text.contains("Weekly") && text.contains("0% used, reset since last seen"),
            "{text}"
        );
        assert!(
            text.contains("footer: 38% used 2h 29m · 0% used now"),
            "{text}"
        );
        assert!(!text.contains("secret-label"), "{text}");
        assert!(!text.contains('@') && !text.contains("/Users"), "{text}");
    }

    use super::*;
    use std::path::PathBuf;

    struct Nothing;
    impl System for Nothing {
        fn find_program(&self, name: &str) -> Option<PathBuf> {
            (name == "claude").then(|| PathBuf::from("/opt/bin/claude"))
        }
        fn login_shell(&self) -> (String, Vec<String>) {
            ("sh".into(), Vec::new())
        }
        fn dir_exists(&self, _: &str) -> bool {
            true
        }
    }

    fn args(command: &[&str], input: Option<&str>) -> Diagnose {
        Diagnose {
            command: command.iter().map(|s| (*s).to_owned()).collect(),
            input: input.map(str::to_owned),
            timeout: 5,
        }
    }

    #[test]
    fn typed_escapes_become_bytes() {
        assert_eq!(unescape("a\\nb"), b"a\nb");
        assert_eq!(unescape("\\r\\t\\e[A"), b"\r\t\x1b[A");
        assert_eq!(unescape("\\x03\\x7f"), vec![3, 0x7f]);
        assert_eq!(unescape("back\\\\slash"), b"back\\slash");
        assert_eq!(unescape("é"), "é".as_bytes());
    }

    #[test]
    fn a_malformed_escape_is_kept_as_typed() {
        assert_eq!(unescape("\\xZ"), b"\\xZ");
        assert_eq!(unescape("\\q"), b"\\q");
        assert_eq!(unescape("end\\"), b"end\\");
    }

    #[test]
    fn without_a_command_a_shell_echoes_one_line() {
        let (spec, input) = plan(&args(&[], None), &Nothing);
        let program = if crate::platform::is_windows() {
            "cmd"
        } else {
            "sh"
        };
        assert_eq!(spec.program, program);
        assert_eq!(input, b"hello\r");
    }

    #[test]
    fn a_bare_program_is_looked_up_and_a_path_is_kept() {
        let (spec, input) = plan(&args(&["claude", "--version"], None), &Nothing);
        assert_eq!(spec.program, "/opt/bin/claude");
        assert_eq!(spec.args, ["--version"]);
        assert!(input.is_empty());
        let (spec, _) = plan(&args(&["/usr/bin/env", "x"], Some("y")), &Nothing);
        assert_eq!(spec.program, "/usr/bin/env");
        let (spec, _) = plan(&args(&["unknown-tool"], None), &Nothing);
        assert_eq!(spec.program, "unknown-tool");
    }

    #[test]
    fn the_resume_diagnosis_prints_the_program_the_folder_and_the_typed_line_only() {
        use chrono::{TimeZone, Utc};
        struct Computer;
        impl System for Computer {
            fn find_program(&self, name: &str) -> Option<PathBuf> {
                Some(PathBuf::from(format!("/bin/{name}")))
            }
            fn login_shell(&self) -> (String, Vec<String>) {
                ("/bin/zsh".into(), vec!["-l".into(), "-i".into()])
            }
            fn dir_exists(&self, path: &str) -> bool {
                path == "/work/here"
            }
        }
        let store = Store::open_in_memory().unwrap();
        let at = Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap();
        for (cwd, id, title, minutes) in [
            ("/work/gone", "newest-but-gone", "secret title one", 1),
            ("/work/here", "it's-odd", "secret title two", 0),
        ] {
            store
                .upsert_session(
                    &leon_core::NewSession {
                        agent: AgentId::CLAUDE,
                        external_id: id.into(),
                        machine_id: MachineId::local(),
                        cwd: cwd.into(),
                        title: title.into(),
                        model: None,
                        started_at: at,
                        updated_at: at + chrono::Duration::minutes(minutes),
                    },
                    &[leon_core::NewMessage {
                        role: leon_core::Role::User,
                        text: "private words".into(),
                        at,
                    }],
                )
                .unwrap();
        }
        let lines = resume_lines(&store, AgentId::CLAUDE, &Computer).unwrap();
        assert_eq!(
            lines,
            [
                "spawn: /bin/zsh -l -i",
                "cwd: /work/here",
                "typed: claude --resume 'it'\\''s-odd'",
                "then: Enter (\\r)"
            ]
        );
        let shown = lines.join("\n");
        assert!(!shown.contains("secret") && !shown.contains("private"));
        assert!(resume_lines(&store, AgentId::CODEX, &Computer).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_real_run_reports_the_screen_and_the_exit() {
        let args = Diagnose {
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf 'ready'; read x; printf '|%s|' \"$x\"; exit 4".into(),
            ],
            input: Some("typed\\r".into()),
            timeout: 10,
        };
        // The run prints; its exit code says the program ended on its own.
        // The terminal reports the end only after the last output arrived, so
        // there is nothing to settle.
        assert_eq!(run_with(&args, &Nothing, Duration::ZERO), 0);
    }

    #[test]
    fn the_sessions_elsewhere_report_says_who_holds_what_and_how_sure_it_is() {
        use crate::elsewhere::resolve;
        use leon_remote::processes::parse_scan;
        let scan = parse_scan(
            "now=1000000\n\
             T 100 1 /x/leon\nT 101 100 sh\nT 102 101 claude\nT 200 1 zsh\nT 201 200 claude\n\
             A 102 101 ttys1 00:10 claude --resume 0a1b2c3d-7777-4888-9999-aaaabbbbcccc\n\
             A 201 200 ttys2 01:00 claude --resume 0a1b2c3d-1111-4222-8333-444455556666\n\
             A 202 200 ttys3 00:30 claude\n",
        );
        let found = resolve(&scan, &[], None);
        let lines = elsewhere_lines(&scan, &found, &[100], Duration::from_millis(7));
        assert_eq!(lines[0], "3 agent processes found");
        assert!(
            lines[1].starts_with("pid 102 \u{b7} Claude Code"),
            "{}",
            lines[1]
        );
        assert!(lines[1].contains("Leon descendant (running Leon pid 100): its own"));
        assert!(lines[1].contains("session 0a1b2c3d-7777-4888-9999-aaaabbbbcccc"));
        assert!(lines[2].contains("not a Leon descendant: elsewhere"));
        assert!(lines[2].contains("certain \u{b7} signal: arguments"));
        assert!(lines[3].contains("session: none"));
        assert_eq!(lines.last().unwrap(), "scan took 7 ms");
    }

    /// The only test that runs the real `ps`: it checks the shipped script
    /// runs on this computer and prints something the parser reads.
    #[cfg(unix)]
    #[test]
    fn smoke_the_real_process_scan_runs_on_this_computer() {
        use leon_remote::Runner;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let output = runtime
            .block_on(
                leon_remote::ProcessRunner::new().run(&leon_remote::processes::scan_command(true)),
            )
            .unwrap();
        assert!(output.success(), "{}", output.stderr);
        let scan = leon_remote::processes::parse_scan(&output.stdout);
        assert!(scan.now.is_some());
        assert!(scan.descends_from(std::process::id(), std::process::id()));
    }
}

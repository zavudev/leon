//! Which coding-agent processes run on a machine, and what they say about the
//! session they hold.
//!
//! [`scan_command`] is one command for the machine (a POSIX `sh` script, or
//! PowerShell for this computer on Windows) that prints, in one round trip:
//!
//! ```text
//! now=<unix seconds>
//! T <pid> <ppid> <executable>        every process: the tree, for ancestry
//! A <pid> <ppid> <tty> <etime> <command line>   candidates that mention an agent
//! C <pid> <working directory>        of the agents' own processes
//! S <json>                           Claude Code's ~/.claude/sessions/<pid>.json
//! ```
//!
//! [`parse_scan`] reads that output into a [`Scan`] of [`AgentProcess`]es. The
//! parsing is pure and table-tested; nothing here starts a process.
//!
//! What ties a process to a session, per agent (checked against the CLIs'
//! `--help` and the files each leaves on disk):
//!
//! | Agent    | Certain                                              | Otherwise |
//! |----------|------------------------------------------------------|-----------|
//! | Claude   | `~/.claude/sessions/<pid>.json` names the session; or `--resume`/`-r`/`--session-id <uuid>` | `--continue`/`-c` = latest of the folder |
//! | Codex    | `codex resume <uuid>`                                | `resume --last` = latest of the folder |
//! | opencode | `--session`/`-s <id>`                                | `--continue`/`-c` = latest of the folder |
//!
//! A forked resume (`--fork-session`, `codex fork`, `--fork`) writes a new
//! session, so the id in its arguments is not claimed.

use std::collections::{HashMap, HashSet};

use leon_core::AgentId;

use crate::command::CommandSpec;

/// The scan for a POSIX machine. `etime` is read instead of a start date: it
/// has the same shape on macOS and Linux and no time zone.
const POSIX_SCAN: &str = r#"LC_ALL=C; export LC_ALL
procs=$(ps -A -o pid= -o ppid= -o tty= -o etime= -o command= 2>/dev/null) || exit 3
printf 'now=%s\n' "$(date +%s)"
printf '%s\n' "$procs" | awk '
  { print "T " $1 " " $2 " " $5 }
  { for (i = 5; i <= NF; i++) if ($i ~ /claude|codex|opencode/) { print "A " $0; break } }'
pids=$(printf '%s\n' "$procs" | awk '
  { n = split($5, a, "/"); b = a[n]; m = split($6, c, "/"); w = c[m]
    if (b == "claude" || b == "codex" || b == "opencode" || ((b == "node" || b == "bun") && (w == "claude" || w == "codex" || w == "opencode"))) print $1 }')
if [ -n "$pids" ]; then
  if [ -r /proc/self/cwd ]; then
    for p in $pids; do c=$(readlink "/proc/$p/cwd" 2>/dev/null) && printf 'C %s %s\n' "$p" "$c"; done
  else
    lsof -a -d cwd -Fpn -p "$(printf '%s\n' "$pids" | paste -sd, -)" 2>/dev/null | awk '/^p/ { pid = substr($0, 2) } /^n/ { print "C " pid " " substr($0, 2) }'
  fi
fi
for f in "$HOME"/.claude/sessions/*.json; do [ -f "$f" ] && printf 'S %s\n' "$(tr -d '\r\n' < "$f")"; done
exit 0"#;

/// The scan for this computer on Windows: processes and Claude's state files.
/// Not run by any test; see the module notes.
const WINDOWS_SCAN: &str = r#"$now=[DateTimeOffset]::UtcNow.ToUnixTimeSeconds(); "now=$now"
Get-CimInstance Win32_Process | ForEach-Object { $age=$now-([DateTimeOffset]$_.CreationDate).ToUnixTimeSeconds(); "T $($_.ProcessId) $($_.ParentProcessId) $($_.Name)"; if ($_.CommandLine -match 'claude|codex|opencode') { "A $($_.ProcessId) $($_.ParentProcessId) - $age $($_.CommandLine)" } }
Get-ChildItem "$env:USERPROFILE\.claude\sessions\*.json" -ErrorAction SilentlyContinue | ForEach-Object { "S " + ((Get-Content -Raw $_.FullName) -replace '\s+',' ') }"#;

/// The scan as a command for the machine; `posix` is false only for this
/// computer when it runs Windows.
pub fn scan_command(posix: bool) -> CommandSpec {
    if posix {
        CommandSpec::new("sh").args(["-c", POSIX_SCAN])
    } else {
        CommandSpec::new("powershell").args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            WINDOWS_SCAN,
        ])
    }
}

/// What a process's arguments say about the session it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resumes {
    /// It names the session by id.
    Id(String),
    /// `--continue` or `--last`: the latest session of its folder.
    Latest,
    /// Nothing says which: a fresh session, a picker, or a fork.
    Unnamed,
}

/// An agent process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProcess {
    /// Its pid.
    pub pid: u32,
    /// Its parent's pid.
    pub ppid: u32,
    /// Which agent it is.
    pub agent: AgentId,
    /// The terminal device it runs on, when it has one.
    pub tty: Option<String>,
    /// When it started, in unix seconds, when the scan knew the time.
    pub started: Option<i64>,
    /// What its arguments say.
    pub resumes: Resumes,
    /// Its working directory, when the scan could read it.
    pub cwd: Option<String>,
    /// The session Claude Code's own state file names for this pid.
    pub state_session: Option<String>,
}

/// A macOS application that owns a process: the terminal it runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppBundle {
    /// The application's name, as its bundle is called.
    pub name: String,
    /// The bundle's path, for `open`.
    pub path: String,
}

/// The parsed output of the scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// The machine's clock when it ran, in unix seconds.
    pub now: Option<i64>,
    parents: HashMap<u32, (u32, String)>,
    /// The agent processes, outermost only (a wrapper's child is not listed).
    pub agents: Vec<AgentProcess>,
}

const MAX_DEPTH: usize = 64;

impl Scan {
    /// Whether `pid` is `root` or runs below it.
    pub fn descends_from(&self, pid: u32, root: u32) -> bool {
        let mut at = pid;
        for _ in 0..MAX_DEPTH {
            if at == root {
                return true;
            }
            match self.parents.get(&at) {
                Some((parent, _)) if *parent != at && *parent != 0 => at = *parent,
                _ => return false,
            }
        }
        false
    }

    /// The pids of the processes whose program is called `name` (`leon`, say).
    pub fn pids_named(&self, name: &str) -> Vec<u32> {
        let mut pids: Vec<u32> = self
            .parents
            .iter()
            .filter(|(_, (_, exe))| base_name(exe) == name)
            .map(|(pid, _)| *pid)
            .collect();
        pids.sort_unstable();
        pids
    }

    /// The application bundle nearest above `pid` (macOS), if one is known.
    pub fn owning_app(&self, pid: u32) -> Option<AppBundle> {
        let mut at = pid;
        for _ in 0..MAX_DEPTH {
            let (parent, _) = self.parents.get(&at)?;
            if *parent == at || *parent == 0 {
                return None;
            }
            at = *parent;
            if let Some((_, exe)) = self.parents.get(&at) {
                if let Some(bundle) = bundle_of(exe) {
                    return Some(bundle);
                }
            }
        }
        None
    }
}

/// The `.app` bundle an executable path lies in.
fn bundle_of(exe: &str) -> Option<AppBundle> {
    let end = exe.find(".app/Contents/MacOS/")?;
    let path = format!("{}.app", &exe[..end]);
    let name = path.rsplit('/').next()?.trim_end_matches(".app").to_owned();
    (!name.is_empty()).then_some(AppBundle { name, path })
}

/// Seconds in a `ps` `etime`: `[[dd-]hh:]mm:ss`, or plain seconds.
pub fn parse_etime(text: &str) -> Option<i64> {
    if !text.contains([':', '-']) {
        return text.parse().ok();
    }
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<i64>().ok()?, clock),
        None => (0, text),
    };
    let mut seconds = 0i64;
    let parts: Vec<&str> = clock.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    for part in parts {
        seconds = seconds * 60 + part.parse::<i64>().ok()?;
    }
    Some(days * 86_400 + seconds)
}

/// Whether `text` is a UUID.
pub fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(at, c)| match at {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// The words of a command line, honouring double quotes (Windows).
fn words(line: &str) -> Vec<String> {
    let (mut out, mut word, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            c => {
                word.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(word);
    }
    out
}

fn base_name(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.strip_suffix(".exe").unwrap_or(name)
}

fn agent_named(name: &str) -> Option<AgentId> {
    match name {
        "claude" => Some(AgentId::CLAUDE),
        "codex" => Some(AgentId::CODEX),
        "opencode" => Some(AgentId::OPENCODE),
        _ => None,
    }
}

/// The value of a flag written `--name value` or `--name=value`, advancing
/// `at` past a separate value.
fn flag_value<'a>(tokens: &'a [String], at: &mut usize, names: &[&str]) -> Option<&'a str> {
    let token = tokens[*at].as_str();
    for name in names {
        if token == *name {
            if let Some(next) = tokens.get(*at + 1).filter(|next| !next.starts_with('-')) {
                *at += 1;
                return Some(next.as_str());
            }
            return Some("");
        }
        if let Some(value) = token
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
        {
            return Some(value);
        }
    }
    None
}

const CLAUDE_OTHER: &[&str] = &[
    "agents",
    "attach",
    "auth",
    "auto-mode",
    "doctor",
    "gateway",
    "import",
    "install",
    "logs",
    "mcp",
    "plugin",
    "plugins",
    "purge",
    "respawn",
    "rm",
    "setup-token",
    "stop",
    "kill",
    "ultrareview",
    "update",
    "upgrade",
    "migrate-installer",
    "config",
];

const CODEX_SUBCOMMANDS: &[&str] = &[
    "agents",
    "exec",
    "e",
    "review",
    "login",
    "logout",
    "mcp",
    "plugin",
    "app-server",
    "remote-control",
    "app",
    "completion",
    "update",
    "doctor",
    "sandbox",
    "debug",
    "apply",
    "a",
    "resume",
    "queue",
    "archive",
    "delete",
    "migrate-rollouts",
    "unarchive",
    "fork",
    "cloud",
    "exec-server",
    "features",
    "help",
];

const OPENCODE_OTHER: &[&str] = &[
    "completion",
    "acp",
    "mcp",
    "attach",
    "debug",
    "providers",
    "auth",
    "agent",
    "upgrade",
    "uninstall",
    "serve",
    "web",
    "models",
    "stats",
    "export",
    "import",
    "github",
    "pr",
    "session",
    "plugin",
    "plug",
    "db",
];

/// What the arguments of an `agent` process say about its session; `None`
/// when it is not an interactive or one-shot session at all (a subcommand
/// such as `mcp`, `--version`).
pub fn session_use(agent: AgentId, tokens: &[String]) -> Option<Resumes> {
    if tokens
        .iter()
        .any(|t| matches!(t.as_str(), "--version" | "-v" | "-V" | "--help" | "-h"))
    {
        return None;
    }
    match agent {
        AgentId::CLAUDE => {
            if tokens
                .first()
                .is_some_and(|first| CLAUDE_OTHER.contains(&first.as_str()))
            {
                return None;
            }
            let (mut resume, mut session, mut cont, mut fork) = (None, None, false, false);
            let mut at = 0;
            while at < tokens.len() {
                if let Some(value) = flag_value(tokens, &mut at, &["--resume", "-r"]) {
                    resume = Some(value.to_owned());
                } else if let Some(value) = flag_value(tokens, &mut at, &["--session-id"]) {
                    session = Some(value.to_owned());
                } else {
                    match tokens[at].as_str() {
                        "--continue" | "-c" => cont = true,
                        "--fork-session" => fork = true,
                        _ => {}
                    }
                }
                at += 1;
            }
            Some(match (session, resume) {
                (Some(id), _) if is_uuid(&id) => Resumes::Id(id),
                (_, Some(id)) if is_uuid(&id) && !fork => Resumes::Id(id),
                _ if cont && !fork => Resumes::Latest,
                _ => Resumes::Unnamed,
            })
        }
        AgentId::CODEX => {
            let Some(at) = tokens
                .iter()
                .position(|t| CODEX_SUBCOMMANDS.contains(&t.as_str()))
            else {
                return Some(Resumes::Unnamed);
            };
            let resume = match tokens[at].as_str() {
                "resume" => at,
                "exec" | "e" if tokens.get(at + 1).map(String::as_str) == Some("resume") => at + 1,
                "fork" | "exec" | "e" | "review" => return Some(Resumes::Unnamed),
                _ => return None,
            };
            let rest = &tokens[resume + 1..];
            Some(match rest.iter().find(|t| is_uuid(t)) {
                Some(id) => Resumes::Id(id.clone()),
                None if rest.iter().any(|t| t == "--last") => Resumes::Latest,
                None => Resumes::Unnamed,
            })
        }
        AgentId::OPENCODE => {
            if tokens
                .first()
                .is_some_and(|first| OPENCODE_OTHER.contains(&first.as_str()))
            {
                return None;
            }
            let (mut id, mut cont, mut fork) = (None, false, false);
            let mut at = 0;
            while at < tokens.len() {
                if let Some(value) = flag_value(tokens, &mut at, &["--session", "-s"]) {
                    id = Some(value.to_owned());
                } else {
                    match tokens[at].as_str() {
                        "--continue" | "-c" => cont = true,
                        "--fork" => fork = true,
                        _ => {}
                    }
                }
                at += 1;
            }
            Some(match id {
                Some(id) if id.starts_with("ses_") && !fork => Resumes::Id(id),
                _ if cont && !fork => Resumes::Latest,
                _ => Resumes::Unnamed,
            })
        }
        // The other agents hold no history Leon reads, so no session is
        // known of their processes.
        _ => None,
    }
}

/// The agent process a command line is, if it is one: the program itself, or
/// an interpreter running a script of that name.
fn agent_of(command: &str) -> Option<(AgentId, Vec<String>)> {
    let tokens = words(command);
    let first = tokens.first()?;
    if let Some(agent) = agent_named(base_name(first)) {
        return Some((agent, tokens[1..].to_vec()));
    }
    if matches!(base_name(first), "node" | "bun" | "deno") {
        // `node [flags] /path/to/claude args`: the script is the first
        // argument that is not a flag.
        let at = tokens[1..].iter().position(|t| !t.starts_with('-'))? + 1;
        let agent = agent_named(base_name(&tokens[at]))?;
        return Some((agent, tokens[at + 1..].to_vec()));
    }
    None
}

/// The next whitespace-separated field, advancing `rest` past it.
fn field<'a>(rest: &mut &'a str) -> &'a str {
    let trimmed = rest.trim_start();
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let (word, tail) = trimmed.split_at(end);
    *rest = tail;
    word
}

/// Reads the output of [`scan_command`]. Lines that are not understood (a
/// login banner) are ignored.
pub fn parse_scan(output: &str) -> Scan {
    let mut scan = Scan::default();
    let mut found: Vec<AgentProcess> = Vec::new();
    let mut cwds: HashMap<u32, String> = HashMap::new();
    let mut states: HashMap<u32, (String, Option<i64>)> = HashMap::new();
    let mut ages: HashMap<u32, i64> = HashMap::new();
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(now) = line.strip_prefix("now=") {
            scan.now = now.trim().parse().ok();
        } else if let Some(rest) = line.strip_prefix("T ") {
            let mut rest = rest;
            let (pid, ppid) = (field(&mut rest), field(&mut rest));
            let exe = field(&mut rest);
            if let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) {
                scan.parents.insert(pid, (ppid, exe.to_owned()));
            }
        } else if let Some(rest) = line.strip_prefix("A ") {
            let mut rest = rest;
            let (pid, ppid, tty, etime) = (
                field(&mut rest),
                field(&mut rest),
                field(&mut rest),
                field(&mut rest),
            );
            let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else {
                continue;
            };
            let Some((agent, tokens)) = agent_of(rest) else {
                continue;
            };
            let Some(resumes) = session_use(agent, &tokens) else {
                continue;
            };
            if let Some(age) = parse_etime(etime) {
                ages.insert(pid, age);
            }
            found.push(AgentProcess {
                pid,
                ppid,
                agent,
                tty: (!matches!(tty, "?" | "??" | "-" | "")).then(|| tty.to_owned()),
                started: None,
                resumes,
                cwd: None,
                state_session: None,
            });
        } else if let Some(rest) = line.strip_prefix("C ") {
            let mut rest = rest;
            let pid = field(&mut rest);
            if let Ok(pid) = pid.parse::<u32>() {
                let path = rest.trim();
                if !path.is_empty() {
                    cwds.insert(pid, path.to_owned());
                }
            }
        } else if let Some(json) = line.strip_prefix("S ") {
            // Only the pid, the session and the start are read.
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
                let pid = value.get("pid").and_then(serde_json::Value::as_u64);
                let session = value.get("sessionId").and_then(serde_json::Value::as_str);
                let started = value.get("startedAt").and_then(serde_json::Value::as_i64);
                if let (Some(pid), Some(session)) = (pid, session) {
                    states.insert(
                        pid as u32,
                        (session.to_owned(), started.map(|ms| ms / 1000)),
                    );
                }
            }
        }
    }
    for process in &mut found {
        process.cwd = cwds.get(&process.pid).cloned();
        process.started = match (scan.now, ages.get(&process.pid)) {
            (Some(now), Some(age)) => Some(now - age),
            _ => None,
        };
        // A state file of a process that has since been replaced by another
        // with the same pid names a different start: it is not trusted.
        if let Some((session, started)) = states.get(&process.pid) {
            let same = match (process.started, started) {
                (Some(a), Some(b)) => (a - b).abs() <= STATE_START_TOLERANCE,
                _ => true,
            };
            if same && process.agent == AgentId::CLAUDE {
                process.state_session = Some(session.clone());
            }
        }
    }
    // A wrapper and the program it starts are one session: keep the outer.
    let ids: HashSet<(u32, AgentId)> = found.iter().map(|p| (p.pid, p.agent)).collect();
    let nested: Vec<u32> = found
        .iter()
        .filter(|p| {
            let mut at = p.ppid;
            for _ in 0..MAX_DEPTH {
                if ids.contains(&(at, p.agent)) {
                    return true;
                }
                match scan.parents.get(&at) {
                    Some((parent, _)) if *parent != at && *parent != 0 => at = *parent,
                    _ => return false,
                }
            }
            false
        })
        .map(|p| p.pid)
        .collect();
    found.retain(|p| !nested.contains(&p.pid));
    scan.agents = found;
    scan
}

/// How far a state file's start may be from the process's own, in seconds.
const STATE_START_TOLERANCE: i64 = 120;

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(line: &str) -> Vec<String> {
        words(line)
    }

    const UUID: &str = "0a1b2c3d-1111-4222-8333-444455556666";

    #[test]
    fn the_arguments_of_each_agent_say_which_session_it_holds() {
        let (claude, codex, opencode) = (AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE);
        let id = || Resumes::Id(UUID.to_owned());
        let table: Vec<(AgentId, String, Option<Resumes>)> = vec![
            (claude, "".into(), Some(Resumes::Unnamed)),
            (claude, format!("--resume {UUID}"), Some(id())),
            (claude, format!("-r {UUID}"), Some(id())),
            (claude, format!("--resume={UUID}"), Some(id())),
            (claude, format!("--session-id {UUID}"), Some(id())),
            (claude, format!("--model opus --resume {UUID}"), Some(id())),
            (claude, "--continue".into(), Some(Resumes::Latest)),
            (claude, "-c".into(), Some(Resumes::Latest)),
            (claude, "--resume".into(), Some(Resumes::Unnamed)),
            (
                claude,
                "--resume my-named-session".into(),
                Some(Resumes::Unnamed),
            ),
            // A fork writes a new session: the id is not the one held.
            (
                claude,
                format!("--resume {UUID} --fork-session"),
                Some(Resumes::Unnamed),
            ),
            (
                claude,
                "--continue --fork-session".into(),
                Some(Resumes::Unnamed),
            ),
            (claude, "mcp serve".into(), None),
            (claude, "--version".into(), None),
            (claude, "update".into(), None),
            (codex, "".into(), Some(Resumes::Unnamed)),
            (codex, format!("resume {UUID}"), Some(id())),
            (codex, format!("-c model=o3 resume {UUID}"), Some(id())),
            (codex, format!("exec resume {UUID}"), Some(id())),
            (codex, "resume --last".into(), Some(Resumes::Latest)),
            (codex, "resume".into(), Some(Resumes::Unnamed)),
            (codex, format!("fork {UUID}"), Some(Resumes::Unnamed)),
            (codex, "exec fix the bug".into(), Some(Resumes::Unnamed)),
            (codex, "app-server --listen unix://".into(), None),
            (codex, "login".into(), None),
            (opencode, "".into(), Some(Resumes::Unnamed)),
            (
                opencode,
                "--session ses_1a2b".into(),
                Some(Resumes::Id("ses_1a2b".into())),
            ),
            (
                opencode,
                "-s ses_1a2b".into(),
                Some(Resumes::Id("ses_1a2b".into())),
            ),
            (
                opencode,
                "--session=ses_1a2b".into(),
                Some(Resumes::Id("ses_1a2b".into())),
            ),
            (
                opencode,
                "run -s ses_1a2b hello".into(),
                Some(Resumes::Id("ses_1a2b".into())),
            ),
            (opencode, "--continue".into(), Some(Resumes::Latest)),
            (
                opencode,
                "--session ses_1a2b --fork".into(),
                Some(Resumes::Unnamed),
            ),
            (opencode, "/work/project".into(), Some(Resumes::Unnamed)),
            (opencode, "serve".into(), None),
            (opencode, "models".into(), None),
        ];
        for (agent, line, expected) in &table {
            assert_eq!(
                session_use(*agent, &tokens(line)),
                *expected,
                "{agent:?} {line:?}"
            );
        }
    }

    #[test]
    fn etime_is_read_in_every_shape_ps_prints() {
        for (text, seconds) in [
            ("00:05", 5),
            ("53:27", 53 * 60 + 27),
            ("01:02:03", 3723),
            ("01-22:28:36", 86_400 + 22 * 3600 + 28 * 60 + 36),
            ("2-00:00:00", 172_800),
            ("90", 90),
        ] {
            assert_eq!(parse_etime(text), Some(seconds), "{text}");
        }
        assert_eq!(parse_etime("soon"), None);
        assert_eq!(parse_etime("1:2:3:4"), None);
    }

    #[test]
    fn a_program_or_an_interpreter_running_its_script_is_an_agent() {
        let (agent, args) = agent_of("/Users/me/.local/bin/claude --resume x").unwrap();
        assert_eq!((agent, args), (AgentId::CLAUDE, tokens("--resume x")));
        let (agent, args) = agent_of("node /usr/local/bin/codex resume y").unwrap();
        assert_eq!((agent, args), (AgentId::CODEX, tokens("resume y")));
        let (agent, _) = agent_of("\"C:\\Program Files\\x\\opencode.exe\" -c").unwrap();
        assert_eq!(agent, AgentId::OPENCODE);
        for not in [
            "/bin/zsh -c source /Users/me/.claude/shell-snapshots/x.sh",
            "rg claude",
            "node /srv/app/server.js",
            "/Applications/Claude.app/Contents/MacOS/Claude",
            "/path/codex-code-mode-host",
        ] {
            assert!(agent_of(not).is_none(), "{not}");
        }
    }

    const OUTPUT: &str = "\
now=1000000
T 1 0 /sbin/launchd
T 2165 1 /Applications/iTerm.app/Contents/MacOS/iTerm2
T 2167 2165 Application
T 2168 2167 login
T 2169 2168 -zsh
T 70645 2169 claude
T 61204 2167 login
T 61205 61204 -zsh
T 93967 61205 target/debug/leon
T 97903 93967 /bin/zsh
T 98757 97903 claude
T 98800 98757 /bin/sh
T 5000 2169 node
T 5001 5000 /opt/codex/codex
A 70645 2169 ttys000 01-22:28:36 claude --resume 0a1b2c3d-1111-4222-8333-444455556666
A 98757 97903 ttys011      49:04 claude --resume 0a1b2c3d-7777-4888-9999-aaaabbbbcccc
A 56048 54685 ttys002   03:00:00 claude
A 27316 56048 ?? 00:01 /bin/zsh -c source /Users/me/.claude/shell-snapshots/snapshot.sh
A 5000 2169 ttys004 10:00 node /usr/local/bin/codex resume 11111111-2222-3333-4444-555555555555
A 5001 5000 ttys004 10:00 /opt/codex/codex resume 11111111-2222-3333-4444-555555555555
A 25755 1 ?? 02-00:00:00 /home/me/codex app-server --listen unix://
C 70645 /Users/me/Code/monorepo
C 56048 /Users/me/Code/leon
S {\"pid\":56048,\"sessionId\":\"0a1b2c3d-dddd-4eee-8fff-000011112222\",\"cwd\":\"/Users/me/Code/leon\",\"startedAt\":989200000,\"name\":\"x\"}
S {\"pid\":70645,\"sessionId\":\"0a1b2c3d-1111-4222-8333-444455556666\",\"startedAt\":832684000}
S not json at all
";

    fn parsed() -> Scan {
        parse_scan(OUTPUT)
    }

    fn by_pid(scan: &Scan, pid: u32) -> &AgentProcess {
        scan.agents.iter().find(|p| p.pid == pid).unwrap()
    }

    #[test]
    fn only_agent_sessions_are_listed_and_a_wrapper_counts_once() {
        let scan = parsed();
        let mut pids: Vec<u32> = scan.agents.iter().map(|p| p.pid).collect();
        pids.sort_unstable();
        // The shell that merely mentions `.claude`, `app-server` and the
        // wrapper's child are not sessions; the wrapper is.
        assert_eq!(pids, [5000, 56048, 70645, 98757]);
    }

    #[test]
    fn a_process_carries_its_tty_start_folder_and_id() {
        let scan = parsed();
        let p = by_pid(&scan, 70645);
        assert_eq!(p.tty.as_deref(), Some("ttys000"));
        assert_eq!(
            p.started,
            Some(1_000_000 - (86_400 + 22 * 3600 + 28 * 60 + 36))
        );
        assert_eq!(p.cwd.as_deref(), Some("/Users/me/Code/monorepo"));
        assert_eq!(p.resumes, Resumes::Id(UUID.to_owned()));
        assert_eq!(scan.now, Some(1_000_000));
        let wrapper = by_pid(&scan, 5000);
        assert_eq!(
            wrapper.resumes,
            Resumes::Id("11111111-2222-3333-4444-555555555555".to_owned())
        );
    }

    #[test]
    fn claudes_state_file_names_the_session_of_a_bare_process() {
        let scan = parsed();
        let bare = by_pid(&scan, 56048);
        assert_eq!(bare.resumes, Resumes::Unnamed);
        assert_eq!(
            bare.state_session.as_deref(),
            Some("0a1b2c3d-dddd-4eee-8fff-000011112222")
        );
    }

    #[test]
    fn a_state_file_of_a_recycled_pid_is_not_believed() {
        // The file says the process began a day before it did.
        let output = "now=1000000\n\
            A 7 1 ttys0 00:10 claude\n\
            S {\"pid\":7,\"sessionId\":\"0a1b2c3d-dddd-4eee-8fff-000011112222\",\"startedAt\":900000000}\n";
        assert_eq!(parse_scan(output).agents[0].state_session, None);
    }

    #[test]
    fn ancestry_tells_a_descendant_from_a_stranger() {
        let scan = parsed();
        // Under the Leon at 93967, through a shell.
        assert!(scan.descends_from(98757, 93967));
        assert!(scan.descends_from(98800, 93967));
        assert!(scan.descends_from(93967, 93967));
        // A sibling terminal's agent is not.
        assert!(!scan.descends_from(70645, 93967));
        assert!(!scan.descends_from(56048, 93967));
        // Unknown pids and loops end the walk.
        assert!(!scan.descends_from(424242, 93967));
    }

    #[test]
    fn the_terminal_application_is_found_above_a_process_when_it_is_known() {
        let scan = parsed();
        let app = scan.owning_app(70645).unwrap();
        assert_eq!(app.name, "iTerm");
        assert_eq!(app.path, "/Applications/iTerm.app");
        assert_eq!(scan.owning_app(1), None);
        assert_eq!(scan.owning_app(424242), None);
    }

    #[test]
    fn processes_are_found_by_the_name_of_their_program() {
        let scan = parsed();
        assert_eq!(scan.pids_named("leon"), [93967]);
        assert_eq!(scan.pids_named("claude"), [70645, 98757]);
        assert!(scan.pids_named("nothing").is_empty());
    }

    #[test]
    fn a_bundle_is_named_by_its_app_folder() {
        let bundle =
            bundle_of("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal");
        assert_eq!(
            bundle,
            Some(AppBundle {
                name: "Terminal".into(),
                path: "/System/Applications/Utilities/Terminal.app".into()
            })
        );
        assert_eq!(bundle_of("/usr/bin/tmux"), None);
    }

    #[test]
    fn noise_and_a_missing_clock_are_survived() {
        let scan = parse_scan("Last login: today\nA 9 1 ttys1 00:09 claude -c\n");
        assert_eq!(scan.agents.len(), 1);
        assert_eq!(scan.agents[0].started, None);
        assert_eq!(scan.agents[0].resumes, Resumes::Latest);
        assert!(parse_scan("").agents.is_empty());
    }

    #[test]
    fn the_scan_is_one_command_for_each_platform() {
        let posix = scan_command(true);
        assert_eq!(posix.program, "sh");
        assert!(posix.args[1].contains("ps -A"));
        let windows = scan_command(false);
        assert_eq!(windows.program, "powershell");
    }

    #[cfg(unix)]
    #[test]
    fn the_posix_script_is_valid_shell() {
        let status = std::process::Command::new("sh")
            .args(["-n", "-c", POSIX_SCAN])
            .status()
            .unwrap();
        assert!(status.success());
    }
}

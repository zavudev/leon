//! Everything "Connect a machine" needs that is not drawing.
//!
//! * [`Target`]: where a machine is, and the exact `ssh` and `ssh-copy-id`
//!   command lines Leon will run or advise, quoted for a shell.
//! * [`SshDir`]: the one seam to the user's `~/.ssh`. The real one reads
//!   `config`, `known_hosts` and the names of `*.pub` files (never a key's
//!   contents) and, only on an explicit confirmation, appends to
//!   `known_hosts`; tests give it a temporary directory or memory.
//! * [`run_checks`]: the checklist behind "Test connection". It goes through
//!   a [`Runner`], reports progress after each step and ends with a
//!   [`Checklist`] whose failures carry a [`Diagnosis`].
//! * [`find_repositories`]: the git repositories near a folder of the
//!   machine, for "Add a project on this machine".

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use leon_core::{Machine, MachineId, MachineKind};
use sha2::{Digest, Sha256};

use crate::command::{run_on, CommandSpec, SshOptions};
use crate::diagnosis::{classify, Diagnosis, DiagnosisKind, Stage};
use crate::probe::{probe, ProbeError, ProbeReport, RemoteOs};
use crate::quote::{sh_join, sh_quote};
use crate::runner::{Output, RunError, Runner};

/// How long `ssh` waits to connect during a test, in seconds.
pub const TEST_CONNECT_SECONDS: u32 = 8;

/// How long one step of the test may take in all.
pub const STEP_LIMIT: Duration = Duration::from_secs(25);

/// Quotes a word for showing: like [`sh_quote`], but `=` stays readable.
fn display_quote(word: &str) -> String {
    if word.contains('=')
        && !word.contains(|c: char| !(c.is_ascii_alphanumeric() || "_-./:@%+,=".contains(c)))
    {
        word.to_owned()
    } else {
        sh_quote(word)
    }
}

// ----- the target -------------------------------------------------------------------------------

/// Where a machine is and how to log in, as typed into the form.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Target {
    /// Host name, address or alias.
    pub host: String,
    /// Login name.
    pub user: Option<String>,
    /// TCP port.
    pub port: Option<u16>,
    /// Private key file on this computer.
    pub identity: Option<String>,
}

impl Target {
    /// The target of an SSH machine; `None` for the local one.
    pub fn of(machine: &Machine) -> Option<Self> {
        match &machine.kind {
            MachineKind::Ssh {
                host,
                user,
                port,
                identity_file,
            } => Some(Self {
                host: host.clone(),
                user: user.clone(),
                port: *port,
                identity: identity_file.clone(),
            }),
            MachineKind::Local | MachineKind::Relay { .. } => None,
        }
    }

    /// A throw-away machine for running commands on this target.
    pub fn machine(&self) -> Machine {
        Machine {
            id: MachineId::from_string("connect-test"),
            name: self.host.clone(),
            kind: MachineKind::Ssh {
                host: self.host.clone(),
                user: self.user.clone(),
                port: self.port,
                identity_file: self.identity.clone(),
            },
        }
    }

    /// `user@host`, or the host alone.
    pub fn destination(&self) -> String {
        match &self.user {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// The `ssh` command Leon runs to try the connection, as a line to paste
    /// into a terminal.
    pub fn ssh_line(&self) -> String {
        let options = SshOptions {
            connect_timeout: Some(TEST_CONNECT_SECONDS),
            ..SshOptions::without_multiplexing()
        };
        let spec = run_on(&self.machine(), &CommandSpec::new("true"), &options);
        std::iter::once(spec.program)
            .chain(spec.args)
            .map(|word| display_quote(&word))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The `ssh-copy-id` command that authorises a key on that computer.
    /// With no host typed yet it shows placeholders.
    pub fn copy_id_line(&self) -> String {
        let mut words = vec!["ssh-copy-id".to_owned()];
        if let Some(port) = self.port {
            words.extend(["-p".to_owned(), port.to_string()]);
        }
        if let Some(identity) = &self.identity {
            let public = if identity.ends_with(".pub") {
                identity.clone()
            } else {
                format!("{identity}.pub")
            };
            words.extend(["-i".to_owned(), public]);
        }
        let destination = if self.host.is_empty() {
            "user@host".to_owned()
        } else {
            self.destination()
        };
        words.push(destination);
        // A placeholder is shown as written; a typed value is quoted.
        sh_join(words).replace("'user@host'", "user@host")
    }
}

// ----- ~/.ssh -------------------------------------------------------------------------------------

/// A public key file of `~/.ssh`, by name only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFile {
    /// The file name, such as `id_ed25519.pub`.
    pub public: String,
    /// The private key's path: the same without `.pub`.
    pub private: String,
}

impl KeyFile {
    /// The kind of key, read from the file name (`id_ed25519` → `ed25519`).
    pub fn kind(&self) -> String {
        let stem = self.public.trim_end_matches(".pub");
        stem.strip_prefix("id_").unwrap_or(stem).to_owned()
    }
}

/// The user's `~/.ssh` folder, as far as Leon looks at it.
pub trait SshDir: Send + Sync {
    /// The text of `config`, when there is one.
    fn config(&self) -> Option<String>;
    /// The text of `known_hosts`, when there is one.
    fn known_hosts(&self) -> Option<String>;
    /// The `*.pub` files, never their contents.
    fn public_keys(&self) -> Vec<KeyFile>;
    /// Appends lines to `known_hosts`, creating it (and the folder, private to
    /// the user) when needed. Only called after the user confirmed.
    fn append_known_hosts(&self, lines: &[String]) -> std::io::Result<()>;
}

/// The real folder.
#[derive(Debug, Clone)]
pub struct RealSshDir {
    dir: PathBuf,
}

impl RealSshDir {
    /// The folder `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `~/.ssh`, when the home folder is known.
    pub fn home() -> Option<Self> {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(Self::new(PathBuf::from(home).join(".ssh")))
    }
}

impl SshDir for RealSshDir {
    fn config(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("config")).ok()
    }

    fn known_hosts(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("known_hosts")).ok()
    }

    fn public_keys(&self) -> Vec<KeyFile> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut keys: Vec<KeyFile> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let stem = name.strip_suffix(".pub")?;
                Some(KeyFile {
                    public: name.clone(),
                    private: self.dir.join(stem).to_string_lossy().into_owned(),
                })
            })
            .collect();
        keys.sort_by(|a, b| a.public.cmp(&b.public));
        keys
    }

    fn append_known_hosts(&self, lines: &[String]) -> std::io::Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700));
        }
        let path = self.dir.join("known_hosts");
        let needs_newline = std::fs::read(&path)
            .map(|bytes| !bytes.is_empty() && !bytes.ends_with(b"\n"))
            .unwrap_or(false);
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        if needs_newline {
            file.write_all(b"\n")?;
        }
        for line in lines {
            writeln!(file, "{line}")?;
        }
        Ok(())
    }
}

/// The host names offered as suggestions: the `Host` entries of `config`
/// without wildcards or negations, then the names of `known_hosts`. Hashed
/// entries, addresses and every key are left out; each name appears once.
pub fn suggested_hosts(dir: &dyn SshDir) -> Vec<String> {
    let mut hosts = dir.config().map(|t| config_hosts(&t)).unwrap_or_default();
    for host in dir
        .known_hosts()
        .map(|t| known_hosts_names(&t))
        .unwrap_or_default()
    {
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
}

/// The `Host` aliases of an ssh config, wildcards and negations left out.
pub fn config_hosts(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(head) = line.get(..4) else {
            continue;
        };
        let rest = &line[4..];
        if !head.eq_ignore_ascii_case("host")
            || !rest.starts_with(|c: char| c.is_whitespace() || c == '=')
        {
            continue;
        }
        let words = rest.split_whitespace();
        for word in words {
            let word = word.trim_start_matches('=');
            if word.is_empty()
                || word.starts_with('#')
                || word.contains(['*', '?', '!'])
                || found.iter().any(|known| known == word)
            {
                continue;
            }
            found.push(word.to_owned());
        }
    }
    found
}

/// The host names of a `known_hosts` file. Hashed entries (`|1|…`), wildcard
/// patterns and plain IP addresses are skipped; the key material is never
/// looked at.
pub fn known_hosts_names(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let mut first = words.next().unwrap_or_default();
        if first.starts_with('@') {
            first = words.next().unwrap_or_default();
        }
        for entry in first.split(',') {
            if entry.starts_with('|') || entry.contains(['*', '?', '!']) {
                continue;
            }
            let name = match entry.strip_prefix('[') {
                Some(rest) => rest.split_once(']').map_or(rest, |(host, _)| host),
                None => entry,
            };
            if name.is_empty()
                || name.parse::<std::net::IpAddr>().is_ok()
                || found.iter().any(|known| known == name)
            {
                continue;
            }
            found.push(name.to_owned());
        }
    }
    found
}

// ----- host keys ----------------------------------------------------------------------------------

/// A host key the other computer showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    /// Key type, such as `ssh-ed25519`.
    pub kind: String,
    /// The fingerprint as `ssh-keygen -l` prints it: `SHA256:…`.
    pub fingerprint: String,
}

/// What `ssh-keyscan` found: the fingerprints to show, and the lines that
/// would be appended to `known_hosts` if the user trusts the computer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostKeys {
    /// The fingerprints.
    pub keys: Vec<HostKey>,
    /// The `known_hosts` lines.
    pub lines: Vec<String>,
}

/// Reads the output of `ssh-keyscan`.
pub fn parse_keyscan(output: &str) -> HostKeys {
    let mut found = HostKeys::default();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let (Some(_host), Some(kind), Some(blob)) = (words.next(), words.next(), words.next())
        else {
            continue;
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(blob) else {
            continue;
        };
        let digest = Sha256::digest(&bytes);
        found.keys.push(HostKey {
            kind: kind.to_owned(),
            fingerprint: format!(
                "SHA256:{}",
                base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
            ),
        });
        found.lines.push(line.to_owned());
    }
    found
}

/// The `ssh-keyscan` command for a target.
pub fn keyscan_command(target: &Target) -> CommandSpec {
    let mut spec = CommandSpec::new("ssh-keyscan").args(["-T", "8"]);
    if let Some(port) = target.port {
        spec = spec.args(["-p".to_owned(), port.to_string()]);
    }
    spec.arg("--").arg(target.host.clone())
}

// ----- the checklist ------------------------------------------------------------------------------

/// One line of the checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckId {
    /// The computer answers on its SSH port.
    Reach,
    /// Its host key is known.
    HostKey,
    /// Our key is accepted.
    Login,
    /// A POSIX shell runs commands.
    Shell,
    /// `git` is installed.
    Git,
    /// The agents that are installed.
    Agents,
}

impl CheckId {
    /// Every line, in order.
    pub const ALL: [CheckId; 6] = [
        Self::Reach,
        Self::HostKey,
        Self::Login,
        Self::Shell,
        Self::Git,
        Self::Agents,
    ];

    /// What the line is called.
    pub fn label(self) -> &'static str {
        match self {
            Self::Reach => "Reach the computer",
            Self::HostKey => "Recognise its host key",
            Self::Login => "Sign in with your key",
            Self::Shell => "Run commands there",
            Self::Git => "Git is installed",
            Self::Agents => "Agents are installed",
        }
    }

    fn of_stage(stage: Stage) -> Self {
        match stage {
            Stage::Reach => Self::Reach,
            Stage::HostKey => Self::HostKey,
            Stage::Login => Self::Login,
            Stage::Shell => Self::Shell,
        }
    }
}

/// How a line stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckState {
    /// Not started.
    Pending,
    /// Under way.
    Running,
    /// Fine.
    Passed,
    /// Works, but something is worth knowing.
    Warned,
    /// Stopped here.
    Failed,
    /// Not tried because an earlier line failed.
    Skipped,
}

/// One line with its result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Which line.
    pub id: CheckId,
    /// How it stands.
    pub state: CheckState,
    /// What was found, in plain words.
    pub note: String,
    /// The explanation and fix, when it failed.
    pub diagnosis: Option<Diagnosis>,
}

/// The whole test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checklist {
    /// The lines.
    pub checks: Vec<Check>,
    /// What the machine reported, once it did.
    pub report: Option<ProbeReport>,
    /// The host keys, when the computer is not trusted yet.
    pub host_keys: Option<HostKeys>,
    /// What ssh printed, trimmed: "Details".
    pub raw: String,
    /// Whether the test ended.
    pub finished: bool,
}

impl Checklist {
    /// A test that has not started.
    pub fn new() -> Self {
        Self {
            checks: CheckId::ALL
                .into_iter()
                .map(|id| Check {
                    id,
                    state: CheckState::Pending,
                    note: String::new(),
                    diagnosis: None,
                })
                .collect(),
            report: None,
            host_keys: None,
            raw: String::new(),
            finished: false,
        }
    }

    /// A finished test that stopped with `diagnosis`, for a failure that
    /// happened earlier (a machine that went offline): `raw` is what was
    /// recorded.
    pub fn from_failure(diagnosis: Diagnosis, raw: impl Into<String>) -> Self {
        let mut list = Self::new();
        list.raw = raw.into();
        list.fail(diagnosis);
        list
    }

    /// A line.
    pub fn get(&self, id: CheckId) -> &Check {
        self.checks
            .iter()
            .find(|check| check.id == id)
            .expect("every line is there")
    }

    fn set(&mut self, id: CheckId, state: CheckState, note: impl Into<String>) {
        if let Some(check) = self.checks.iter_mut().find(|check| check.id == id) {
            check.state = state;
            check.note = note.into();
        }
    }

    /// The first failure's diagnosis.
    pub fn failure(&self) -> Option<&Diagnosis> {
        self.checks
            .iter()
            .find_map(|check| check.diagnosis.as_ref())
    }

    /// Whether the machine can be used: the test ended and nothing failed.
    pub fn connected(&self) -> bool {
        self.finished
            && self
                .checks
                .iter()
                .all(|check| check.state != CheckState::Failed)
    }

    /// Marks the failing stage and skips what would have come after.
    fn fail(&mut self, diagnosis: Diagnosis) {
        let at = CheckId::of_stage(diagnosis.kind.stage());
        let mut after = false;
        for check in &mut self.checks {
            if check.id == at {
                check.state = CheckState::Failed;
                check.note = diagnosis.title.clone();
                check.diagnosis = Some(diagnosis.clone());
                after = true;
            } else if after {
                check.state = CheckState::Skipped;
                check.note.clear();
            } else {
                check.state = CheckState::Passed;
                if check.note.is_empty() {
                    check.note = "ok".to_owned();
                }
            }
        }
        self.finished = true;
    }
}

impl Default for Checklist {
    fn default() -> Self {
        Self::new()
    }
}

/// How to install each agent, in one line.
pub fn install_hint(agent: &str) -> &'static str {
    match agent {
        "claude" => "Claude Code: https://docs.claude.com/en/docs/claude-code",
        "codex" => "Codex: npm install -g @openai/codex",
        "opencode" => "opencode: https://opencode.ai",
        _ => "",
    }
}

async fn run_limited<R: Runner>(runner: &R, spec: &CommandSpec) -> Result<Output, RunError> {
    match tokio::time::timeout(STEP_LIMIT, runner.run(spec)).await {
        Ok(done) => done,
        Err(_) => Err(RunError::TimedOut {
            program: spec.program.clone(),
            limit: STEP_LIMIT,
        }),
    }
}

fn run_error_diagnosis(error: &RunError) -> Diagnosis {
    match error {
        RunError::Spawn { .. } => classify(None, "cannot start ssh"),
        RunError::TimedOut { .. } => classify(None, "Operation timed out"),
    }
}

/// Runs the checklist against `target`, calling `progress` whenever a line
/// changes. The first step is one `ssh … true`: ssh ends with 255 for its own
/// failures, so its message tells which of reach, host key and login stopped;
/// any other status means it got in. The second is the machine probe, which
/// tells the shell, `git` and the agents.
pub async fn run_checks<R: Runner>(
    runner: &R,
    target: &Target,
    mut progress: impl FnMut(&Checklist),
) -> Checklist {
    let mut list = Checklist::new();
    let options = SshOptions {
        connect_timeout: Some(TEST_CONNECT_SECONDS),
        ..SshOptions::without_multiplexing()
    };
    let machine = target.machine();
    list.set(CheckId::Reach, CheckState::Running, "");
    progress(&list);

    let first = run_limited(
        runner,
        &run_on(&machine, &CommandSpec::new("true"), &options),
    )
    .await;
    let output = match first {
        Err(error) => {
            list.fail(run_error_diagnosis(&error));
            list.raw = error.to_string();
            progress(&list);
            return list;
        }
        Ok(output) => output,
    };
    list.raw = output.stderr.trim().to_owned();
    let ssh_failed = matches!(output.status, Some(255) | None);
    let windows = output
        .stderr
        .to_lowercase()
        .contains("is not recognized as an internal");
    if ssh_failed || windows {
        let diagnosis = classify(output.status, &output.stderr);
        if diagnosis.kind == DiagnosisKind::HostKeyUnknown {
            if let Ok(scan) = run_limited(runner, &keyscan_command(target)).await {
                let keys = parse_keyscan(&scan.stdout);
                if !keys.keys.is_empty() {
                    list.host_keys = Some(keys);
                }
            }
        }
        list.fail(diagnosis);
        progress(&list);
        return list;
    }
    list.set(
        CheckId::Reach,
        CheckState::Passed,
        "It answers on its SSH port.",
    );
    list.set(CheckId::HostKey, CheckState::Passed, "Known and unchanged.");
    list.set(CheckId::Login, CheckState::Passed, "Your key is accepted.");
    list.set(CheckId::Shell, CheckState::Running, "");
    progress(&list);

    let probed = probe(&RunnerRef(runner), &machine, &options).await;
    match probed {
        Err(ProbeError::Run(error)) => {
            list.fail(run_error_diagnosis(&error));
        }
        Err(ProbeError::Failed { status, stderr }) => {
            list.raw = stderr.clone();
            list.fail(classify(status, &stderr));
        }
        Err(ProbeError::Unrecognised) => {
            list.fail(Diagnosis {
                kind: DiagnosisKind::Unknown,
                title: "The shell did not answer as expected".to_owned(),
                explanation: "You can sign in, but the commands Leon sends did not give a \
                              usable answer. It needs a POSIX shell (macOS or Linux)."
                    .to_owned(),
                fix: "Check that the login shell runs plain `sh` commands and prints nothing \
                      surprising at login."
                    .to_owned(),
            });
        }
        Ok(report) => {
            conclude(&mut list, report);
        }
    }
    progress(&list);
    list
}

/// A borrowed runner is a runner.
struct RunnerRef<'a, R>(&'a R);

impl<R: Runner> Runner for RunnerRef<'_, R> {
    async fn run(&self, spec: &CommandSpec) -> Result<Output, RunError> {
        run_limited(self.0, spec).await
    }
}

fn conclude(list: &mut Checklist, report: ProbeReport) {
    let windows_like = matches!(&report.os, RemoteOs::Other(name)
        if ["MINGW", "MSYS", "CYGWIN", "Windows"].iter().any(|p| name.contains(p)));
    if windows_like {
        list.fail(classify(
            None,
            "is not recognized as an internal or external command",
        ));
        return;
    }
    let os = match &report.os {
        RemoteOs::Linux => "Linux".to_owned(),
        RemoteOs::MacOs => "macOS".to_owned(),
        RemoteOs::Other(name) => name.clone(),
    };
    let shell_state = if matches!(report.os, RemoteOs::Other(_)) {
        CheckState::Warned
    } else {
        CheckState::Passed
    };
    let mut shell_note = os.to_string();
    if let Some(arch) = &report.arch {
        shell_note.push_str(&format!(" ({arch})"));
    }
    if let Some(home) = &report.home {
        shell_note.push_str(&format!(", home {home}"));
    }
    list.set(CheckId::Shell, shell_state, shell_note);
    match &report.git {
        Some(path) => list.set(CheckId::Git, CheckState::Passed, path.clone()),
        None => list.set(
            CheckId::Git,
            CheckState::Warned,
            "git was not found. Install it (macOS: `xcode-select --install`; Linux: \
             `sudo apt install git`) to use projects and worktrees there.",
        ),
    }
    let agents = ["claude", "codex", "opencode"].map(|name| (name, report.tool(name)));
    let mut found: Vec<String> = agents
        .iter()
        .filter_map(|(name, path)| path.map(|path| format!("{name} {path}")))
        .collect();
    // The other agents of the catalogue that the machine has: named by their
    // commands, without paths, so the line stays short.
    let others: Vec<&str> = report
        .tools
        .keys()
        .map(String::as_str)
        .filter(|name| !["claude", "codex", "opencode"].contains(name))
        .collect();
    if !others.is_empty() {
        found.push(format!("also {}", others.join(", ")));
    }
    let missing: Vec<&str> = agents
        .iter()
        .filter(|(_, path)| path.is_none())
        .map(|(name, _)| install_hint(name))
        .collect();
    let mut note = String::new();
    if found.is_empty() {
        note.push_str("No agent found there yet.");
    } else {
        note.push_str(&found.join(", "));
    }
    if !missing.is_empty() {
        note.push_str(" Missing: ");
        note.push_str(&missing.join("; "));
    }
    let state = if found.is_empty() {
        CheckState::Warned
    } else {
        CheckState::Passed
    };
    list.set(CheckId::Agents, state, note);
    list.report = Some(report);
    list.finished = true;
}

// ----- repositories -------------------------------------------------------------------------------

/// How many repositories a search returns at most.
pub const MAX_REPOSITORIES: usize = 60;

const FIND_SCRIPT: &str = r#"base="${1:-$HOME}"
find "$base" -maxdepth 4 \( -name node_modules -o -name target -o -name .cache -o -name Library \) -prune -o -name .git -prune -print 2>/dev/null | head -n 200
exit 0"#;

/// The command that lists `.git` entries a few levels below `base` (the home
/// folder when absent).
pub fn find_repositories_command(base: Option<&str>) -> CommandSpec {
    let mut spec = CommandSpec::new("sh").args(["-c", FIND_SCRIPT, "sh"]);
    if let Some(base) = base.filter(|base| !base.trim().is_empty()) {
        spec = spec.arg(base.trim());
    }
    spec
}

/// The repository folders in the output of [`find_repositories_command`].
pub fn parse_repositories(output: &str) -> Vec<String> {
    let mut folders: Vec<String> = output
        .lines()
        .filter_map(|line| line.trim().strip_suffix("/.git"))
        .filter(|folder| !folder.is_empty())
        .map(str::to_owned)
        .collect();
    folders.sort();
    folders.dedup();
    folders.truncate(MAX_REPOSITORIES);
    folders
}

/// The git repositories found near `base` on the machine.
pub async fn find_repositories<R: Runner>(
    runner: &R,
    machine: &Machine,
    ssh: &SshOptions,
    base: Option<&str>,
) -> Result<Vec<String>, RunError> {
    let spec = run_on(machine, &find_repositories_command(base), ssh);
    let output = run_limited(runner, &spec).await?;
    Ok(parse_repositories(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::ScriptedRunner;

    fn target(host: &str, user: Option<&str>, port: Option<u16>, key: Option<&str>) -> Target {
        Target {
            host: host.into(),
            user: user.map(str::to_owned),
            port,
            identity: key.map(str::to_owned),
        }
    }

    #[test]
    fn the_ssh_line_is_what_leon_runs_and_is_quoted() {
        let line = target("box", Some("dev"), Some(2222), Some("/k/my key")).ssh_line();
        assert!(
            line.starts_with("ssh -o BatchMode=yes -o ConnectTimeout=8"),
            "{line}"
        );
        assert!(line.contains("-p 2222"));
        assert!(line.contains("-i '/k/my key'"));
        assert!(line.contains("-l dev"));
        assert!(line.ends_with("-- box 'exec true'"), "{line}");
    }

    #[test]
    fn the_copy_id_line_follows_the_typed_values() {
        assert_eq!(
            target("box", Some("dev"), None, None).copy_id_line(),
            "ssh-copy-id dev@box"
        );
        assert_eq!(
            target("box", Some("dev"), Some(2222), Some("/k/id_ed25519")).copy_id_line(),
            "ssh-copy-id -p 2222 -i /k/id_ed25519.pub dev@box"
        );
        assert_eq!(
            target("box", None, None, Some("/k/a b.pub")).copy_id_line(),
            "ssh-copy-id -i '/k/a b.pub' box"
        );
        assert_eq!(Target::default().copy_id_line(), "ssh-copy-id user@host");
        assert_eq!(
            target("h; rm -rf /", Some("dev"), None, None).copy_id_line(),
            "ssh-copy-id 'dev@h; rm -rf /'"
        );
    }

    #[test]
    fn config_hosts_are_aliases_without_patterns() {
        let text = "Host *\n  User x\nHost build staging\n  HostName 10.0.0.1\nhost web !bad *.corp\nHost=eq\n# Host commented\nMatch host zzz\n";
        assert_eq!(config_hosts(text), ["build", "staging", "web", "eq"]);
    }

    #[test]
    fn known_hosts_names_skip_hashes_addresses_and_keys() {
        let text = "\
|1|abc=|def= ssh-ed25519 AAAAhashed
github.com,140.82.1.1 ssh-ed25519 AAAAsecretkeymaterial
[box.example]:2222 ssh-rsa AAAAother
@cert-authority *.corp ssh-rsa AAAAca
@revoked old.example ssh-rsa AAAAx
192.168.1.5 ssh-ed25519 AAAAip
github.com ssh-rsa AAAAagain
# comment
";
        let names = known_hosts_names(text);
        assert_eq!(names, ["github.com", "box.example", "old.example"]);
        assert!(names.iter().all(|name| !name.contains("AAAA")));
    }

    #[test]
    fn suggestions_merge_both_files_once() {
        struct Fake;
        impl SshDir for Fake {
            fn config(&self) -> Option<String> {
                Some("Host box\n".into())
            }
            fn known_hosts(&self) -> Option<String> {
                Some("box ssh-ed25519 AAAA\nother ssh-ed25519 AAAA\n".into())
            }
            fn public_keys(&self) -> Vec<KeyFile> {
                Vec::new()
            }
            fn append_known_hosts(&self, _: &[String]) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(suggested_hosts(&Fake), ["box", "other"]);
    }

    #[test]
    fn the_real_folder_lists_key_names_and_appends_privately() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("id_ed25519.pub"),
            "ssh-ed25519 AAAASECRET me",
        )
        .unwrap();
        std::fs::write(dir.path().join("id_ed25519"), "PRIVATE").unwrap();
        std::fs::write(dir.path().join("known_hosts"), "a ssh-rsa AAAA").unwrap();
        let ssh = RealSshDir::new(dir.path());
        let keys = ssh.public_keys();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].public, "id_ed25519.pub");
        assert!(keys[0].private.ends_with("id_ed25519"));
        assert_eq!(keys[0].kind(), "ed25519");
        assert!(!format!("{keys:?}").contains("SECRET"));

        ssh.append_known_hosts(&["b ssh-ed25519 BBBB".to_owned()])
            .unwrap();
        let text = std::fs::read_to_string(dir.path().join("known_hosts")).unwrap();
        assert_eq!(text, "a ssh-rsa AAAA\nb ssh-ed25519 BBBB\n");
    }

    #[test]
    fn a_fingerprint_is_the_sha256_of_the_key_blob() {
        // sha256("abc") is ba7816bf…; "YWJj" is base64 of "abc".
        let keys = parse_keyscan("# box:22 SSH-2.0-OpenSSH_9\nbox ssh-ed25519 YWJj\nbroken line\n");
        assert_eq!(keys.keys.len(), 1);
        assert_eq!(keys.keys[0].kind, "ssh-ed25519");
        assert_eq!(
            keys.keys[0].fingerprint,
            "SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0"
        );
        assert_eq!(keys.lines, ["box ssh-ed25519 YWJj"]);
    }

    const LINUX: &str = "os=Linux\narch=x86_64\nhome=/home/dev\ntool=git=/usr/bin/git\ntool=claude=/home/dev/.local/bin/claude\ntool=codex=/usr/bin/codex\n";

    fn states(list: &Checklist) -> Vec<CheckState> {
        list.checks.iter().map(|c| c.state.clone()).collect()
    }

    #[tokio::test]
    async fn a_full_success_lists_the_agents_and_reports_progress() {
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(LINUX));
        let mut steps = 0;
        let list = run_checks(&runner, &target("box", Some("dev"), None, None), |_| {
            steps += 1
        })
        .await;
        assert!(list.connected());
        assert!(steps >= 3);
        assert!(list
            .get(CheckId::Agents)
            .note
            .contains("claude /home/dev/.local/bin/claude"));
        assert!(list.get(CheckId::Agents).note.contains("opencode"));
        assert_eq!(list.get(CheckId::Git).note, "/usr/bin/git");
        assert!(list.get(CheckId::Shell).note.contains("home /home/dev"));
        assert_eq!(runner.calls().len(), 2);
    }

    #[tokio::test]
    async fn a_missing_git_is_a_warning_not_a_failure() {
        let runner = ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok("os=Darwin\nhome=/Users/d\n"));
        let list = run_checks(&runner, &target("box", None, None, None), |_| {}).await;
        assert!(list.connected());
        assert_eq!(list.get(CheckId::Git).state, CheckState::Warned);
        assert_eq!(list.get(CheckId::Agents).state, CheckState::Warned);
    }

    #[tokio::test]
    async fn a_refused_connection_stops_at_the_first_line() {
        let runner = ScriptedRunner::new().reply(Output::failed(
            255,
            "ssh: connect to host box port 22: Connection refused\n",
        ));
        let list = run_checks(&runner, &target("box", None, None, None), |_| {}).await;
        assert!(!list.connected());
        assert_eq!(list.get(CheckId::Reach).state, CheckState::Failed);
        assert_eq!(list.get(CheckId::Login).state, CheckState::Skipped);
        assert_eq!(
            list.failure().unwrap().kind,
            DiagnosisKind::ConnectionRefused
        );
        assert!(list.raw.contains("Connection refused"));
    }

    #[tokio::test]
    async fn a_refused_key_passes_reach_and_host_key_and_stops_at_login() {
        let runner = ScriptedRunner::new().reply(Output::failed(
            255,
            "dev@box: Permission denied (publickey).\n",
        ));
        let list = run_checks(&runner, &target("box", Some("dev"), None, None), |_| {}).await;
        assert_eq!(
            states(&list)[..4],
            [
                CheckState::Passed,
                CheckState::Passed,
                CheckState::Failed,
                CheckState::Skipped
            ]
        );
        assert_eq!(
            list.failure().unwrap().kind,
            DiagnosisKind::PublicKeyRefused
        );
    }

    #[tokio::test]
    async fn an_unknown_host_key_comes_with_its_fingerprint() {
        let runner = ScriptedRunner::new()
            .reply(Output::failed(255, "Host key verification failed.\n"))
            .reply(Output::ok("box ssh-ed25519 YWJj\n"));
        let list = run_checks(&runner, &target("box", None, None, None), |_| {}).await;
        assert_eq!(list.get(CheckId::HostKey).state, CheckState::Failed);
        let keys = list.host_keys.expect("the keys were scanned");
        assert_eq!(keys.lines, ["box ssh-ed25519 YWJj"]);
        assert_eq!(runner.calls()[1].program, "ssh-keyscan");
    }

    #[tokio::test]
    async fn a_changed_host_key_is_not_scanned_for_trust() {
        let runner = ScriptedRunner::new().reply(Output::failed(
            255,
            "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!\nHost key verification failed.\n",
        ));
        let list = run_checks(&runner, &target("box", None, None, None), |_| {}).await;
        assert_eq!(list.failure().unwrap().kind, DiagnosisKind::HostKeyChanged);
        assert!(list.host_keys.is_none());
        assert_eq!(runner.calls().len(), 1);
    }

    #[tokio::test]
    async fn a_windows_remote_is_not_supported_yet() {
        let runner = ScriptedRunner::new().reply(Output::failed(
            1,
            "'true' is not recognized as an internal or external command,\n",
        ));
        let list = run_checks(&runner, &target("win", None, None, None), |_| {}).await;
        assert_eq!(list.get(CheckId::Shell).state, CheckState::Failed);
        assert_eq!(list.failure().unwrap().kind, DiagnosisKind::WindowsRemote);
    }

    #[tokio::test]
    async fn an_ssh_that_cannot_start_is_explained() {
        let runner = ScriptedRunner::new();
        let list = run_checks(&runner, &target("box", None, None, None), |_| {}).await;
        assert_eq!(list.failure().unwrap().kind, DiagnosisKind::SshMissing);
    }

    #[test]
    fn a_recorded_failure_becomes_a_finished_checklist() {
        let why = classify(
            Some(255),
            "probe failed (status Some(255)): Permission denied (publickey).",
        );
        let list = Checklist::from_failure(why, "raw");
        assert!(list.finished && !list.connected());
        assert_eq!(list.get(CheckId::Login).state, CheckState::Failed);
        assert_eq!(list.get(CheckId::Reach).state, CheckState::Passed);
        assert_eq!(list.raw, "raw");
    }

    #[test]
    fn repositories_are_the_parents_of_git_entries() {
        let out = "/home/d/a/.git\n/home/d/b/c/.git\n/home/d/a/.git\nnoise\n";
        assert_eq!(parse_repositories(out), ["/home/d/a", "/home/d/b/c"]);
        let spec = find_repositories_command(Some(" /srv "));
        assert_eq!(spec.args.last().unwrap(), "/srv");
        assert_eq!(find_repositories_command(None).args.len(), 3);
    }
}

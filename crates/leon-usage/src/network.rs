//! The opt-in network sources, and the one place that calls out.
//!
//! A source here reads the agent's own credential and sends it to that
//! vendor's usage endpoint, so it runs only when its switch is on
//! ([`NetworkPolicy`]); with the switch off nothing is read and nothing is
//! called. Rules that hold for every call:
//!
//! * the credential is read at the moment of the call, kept in a
//!   [`Secret`](crate::secret::Secret), and dropped with the request; it is
//!   never stored, logged, shown or part of an error;
//! * it is sent only to the vendor's own host, over HTTPS, by [`Http`]; every
//!   host is named in [`ALLOWED_HOSTS`] and a request to any other is refused
//!   before anything starts;
//! * every call has a time limit, redirects are not followed, and a failure
//!   backs off ([`Throttle`]);
//! * a stored sign-in that has expired is reported, never refreshed: the
//!   agent refreshes its own;
//! * any failure is an explicit unknown ([`Reason`]), never a made-up number.
//!
//! [`CurlHttp`] sends the request with the system's `curl`, the headers on
//! standard input so that the token never shows in a process list. Tests use
//! [`ScriptedHttp`], and nothing in the test suite can reach a network.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use leon_core::AgentId;

use crate::model::{AgentUsage, Reason};
use crate::secret::Secret;
use crate::{claude, codex, cursor, grok, kimi, opencode, zcode};

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The hosts a credential may be sent to: one or three per provider.
pub const ALLOWED_HOSTS: [&str; 9] = [
    claude::HOST,
    opencode::HOST,
    codex::BACKEND_HOST,
    grok::HOST,
    cursor::HOST,
    kimi::HOST,
    zcode::HOSTS[0],
    zcode::HOSTS[1],
    zcode::HOSTS[2],
];

/// How long one call may take.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// The largest answer read.
const MAX_BYTES: u64 = 1_000_000;

/// The agents that have a usage source with a switch of its own in the
/// settings: every built-in agent with a usage provider.
pub fn switchable_agents() -> Vec<AgentId> {
    leon_core::agent::builtin()
        .iter()
        .filter(|spec| spec.usage.is_some())
        .map(|spec| spec.id)
        .collect()
}

/// Which usage sources are switched on, by agent: the network sources, and
/// the command-line one of Antigravity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkPolicy {
    on: BTreeSet<AgentId>,
}

impl NetworkPolicy {
    /// Everything off.
    pub fn none() -> Self {
        Self::default()
    }

    /// Everything on.
    pub fn all() -> Self {
        Self {
            on: switchable_agents().into_iter().collect(),
        }
    }

    /// Whether the source of `agent` is on.
    pub fn allows(&self, agent: AgentId) -> bool {
        self.on.contains(&agent)
    }

    /// Switches the source of `agent`.
    pub fn set(&mut self, agent: AgentId, on: bool) {
        if on {
            self.on.insert(agent);
        } else {
            self.on.remove(&agent);
        }
    }

    /// The same policy with the source of `agent` on.
    pub fn with(mut self, agent: AgentId) -> Self {
        self.on.insert(agent);
        self
    }

    /// The agents whose source is on.
    pub fn agents(&self) -> impl Iterator<Item = AgentId> + '_ {
        self.on.iter().copied()
    }
}

/// How a request proves who is asking.
pub enum Auth<'a> {
    /// `Authorization: Bearer <secret>`.
    Bearer(&'a Secret),
    /// A header whose whole value is the secret (a cookie, or the bare key
    /// some services take as `Authorization`).
    Header {
        /// The header's name.
        name: &'static str,
        /// Its value.
        value: &'a Secret,
    },
}

/// A request with a credential.
pub struct Request<'a> {
    /// The URL; its host must be in [`ALLOWED_HOSTS`].
    pub url: &'a str,
    /// The credential.
    pub auth: Auth<'a>,
    /// Other headers, which hold nothing secret.
    pub headers: Vec<(&'static str, String)>,
}

impl std::fmt::Debug for Request<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// What came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// The HTTP status.
    pub status: u16,
    /// The body, as text.
    pub body: String,
    /// The wait the vendor asked for with `Retry-After`, in seconds, when it
    /// gave one as a number.
    pub retry_after: Option<u32>,
}

/// Why nothing came back. It carries no text, so it cannot quote a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpError {
    /// The URL's host is not one a credential may be sent to.
    Refused,
    /// Offline, a timeout, a refused connection or no `curl`.
    Unreachable,
}

/// Makes the one kind of call there is.
pub trait Http: Send + Sync {
    /// Sends a GET.
    fn get<'a>(&'a self, request: &'a Request<'a>) -> BoxFuture<'a, Result<Response, HttpError>>;
}

/// Whether `url` is HTTPS on an allowed host.
pub fn host_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    ALLOWED_HOSTS.contains(&host)
}

/// Quotes a value for a `curl` config file.
fn config_quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The `curl` configuration of a request, which goes to standard input.
pub fn curl_config(request: &Request<'_>) -> String {
    let mut config = format!("url = {}\n", config_quote(request.url));
    let (name, value) = match &request.auth {
        Auth::Bearer(secret) => ("Authorization", format!("Bearer {}", secret.expose())),
        Auth::Header { name, value } => (*name, value.expose().to_owned()),
    };
    config.push_str(&format!(
        "header = {}\n",
        config_quote(&format!("{name}: {value}"))
    ));
    for (name, value) in &request.headers {
        config.push_str(&format!(
            "header = {}\n",
            config_quote(&format!("{name}: {value}"))
        ));
    }
    config
}

/// Reads what `curl --include --write-out '\n%{http_code}'` printed: the
/// header block(s), the body and the status. `None` when there is no status.
pub fn parse_curl_output(text: &str) -> Option<Response> {
    let (rest, status) = text.rsplit_once('\n')?;
    let status: u16 = status.trim().parse().ok().filter(|status| *status != 0)?;
    let mut remaining = rest;
    let mut retry_after = None;
    // An interim `100 Continue` has a block of its own before the answer's.
    while remaining.starts_with("HTTP/") {
        let (head, body) = remaining
            .split_once("\r\n\r\n")
            .or_else(|| remaining.split_once("\n\n"))
            .unwrap_or((remaining, ""));
        for line in head.lines() {
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("retry-after") {
                    retry_after = value.trim().parse().ok();
                }
            }
        }
        remaining = body;
    }
    Some(Response {
        status,
        body: remaining.to_owned(),
        retry_after,
    })
}

/// Calls with the system's `curl`.
#[derive(Debug, Default, Clone, Copy)]
pub struct CurlHttp;

impl Http for CurlHttp {
    fn get<'a>(&'a self, request: &'a Request<'a>) -> BoxFuture<'a, Result<Response, HttpError>> {
        Box::pin(async move {
            use tokio::io::AsyncWriteExt;
            if !host_allowed(request.url) {
                return Err(HttpError::Refused);
            }
            let mut command = leon_remote::spawn::child("curl");
            command
                .args([
                    "--silent",
                    "--include",
                    "--proto",
                    "=https",
                    "--max-redirs",
                    "0",
                ])
                .args(["--max-time", &TIMEOUT.as_secs().to_string()])
                .args(["--max-filesize", &MAX_BYTES.to_string()])
                .args(["--user-agent", "Leon"])
                .args(["--write-out", "\n%{http_code}"])
                .args(["--config", "-"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            let mut child = command.spawn().map_err(|_| HttpError::Unreachable)?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(curl_config(request).as_bytes())
                    .await
                    .map_err(|_| HttpError::Unreachable)?;
            }
            let output =
                tokio::time::timeout(TIMEOUT + Duration::from_secs(2), child.wait_with_output())
                    .await
                    .map_err(|_| HttpError::Unreachable)?
                    .map_err(|_| HttpError::Unreachable)?;
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            parse_curl_output(&text).ok_or(HttpError::Unreachable)
        })
    }
}

/// A scripted client for tests: answers in order and counts its calls.
#[derive(Debug, Default)]
pub struct ScriptedHttp {
    replies: Mutex<Vec<Result<Response, HttpError>>>,
    calls: Mutex<Vec<String>>,
}

impl ScriptedHttp {
    /// A client with nothing queued.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues an answer.
    pub fn reply(self, status: u16, body: &str) -> Self {
        self.reply_after(status, body, None)
    }

    /// Queues an answer that carries a `Retry-After`.
    pub fn reply_after(self, status: u16, body: &str, retry_after: Option<u32>) -> Self {
        self.replies.lock().unwrap().push(Ok(Response {
            status,
            body: body.to_owned(),
            retry_after,
        }));
        self
    }

    /// Queues a failure.
    pub fn fail(self, error: HttpError) -> Self {
        self.replies.lock().unwrap().push(Err(error));
        self
    }

    /// The URLs called so far.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Http for ScriptedHttp {
    fn get<'a>(&'a self, request: &'a Request<'a>) -> BoxFuture<'a, Result<Response, HttpError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(request.url.to_owned());
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() {
                Err(HttpError::Unreachable)
            } else {
                replies.remove(0)
            }
        })
    }
}

/// What reading a credential came to.
#[derive(Debug)]
pub enum Read<T> {
    /// Found.
    Found(T),
    /// There is none: not signed in.
    Missing,
    /// The store could not be read.
    Unavailable,
    /// The system refused to hand it over (a keychain prompt that was denied
    /// or dismissed).
    Denied,
}

/// What `security find-generic-password` came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeychainOutcome {
    /// It printed the item.
    Found,
    /// There is no such item.
    NotFound,
    /// Access was denied, or the permission prompt was dismissed.
    Denied,
    /// Anything else.
    Other,
}

/// Classifies the end of `security find-generic-password`. The text it printed
/// is looked at only to tell a refusal from another failure; nothing of it is
/// kept.
pub fn classify_security(code: Option<i32>, stderr: &str) -> KeychainOutcome {
    match code {
        Some(0) => return KeychainOutcome::Found,
        Some(44) => return KeychainOutcome::NotFound,
        Some(128 | 36 | 51) => return KeychainOutcome::Denied,
        _ => {}
    }
    let text = stderr.to_ascii_lowercase();
    if ["denied", "cancel", "not allowed", "interaction"]
        .iter()
        .any(|word| text.contains(word))
    {
        KeychainOutcome::Denied
    } else {
        KeychainOutcome::Other
    }
}

/// Where the agents keep their credentials, read only at the moment of a call.
/// The methods of the newer providers answer "not signed in" unless
/// implemented, so a stand-in for tests names only what it needs.
pub trait Credentials: Send + Sync {
    /// Claude Code's OAuth credential.
    fn claude(&self) -> Read<claude::Credential>;
    /// The opencode Go API key.
    fn opencode_go(&self) -> Read<Secret>;
    /// Codex's ChatGPT sign-in.
    fn codex(&self) -> Read<codex::Credential> {
        Read::Missing
    }
    /// Grok's session, at the time `now` (Unix seconds).
    fn grok(&self, _now: i64) -> Read<grok::Credential> {
        Read::Missing
    }
    /// Cursor's session, at the time `now`.
    fn cursor(&self, _now: i64) -> Read<cursor::Session> {
        Read::Missing
    }
    /// Kimi Code's sign-in, at the time `now`.
    fn kimi(&self, _now: i64) -> Read<kimi::Credential> {
        Read::Missing
    }
    /// The ZCode plan's key and host.
    fn zcode(&self) -> Read<zcode::Credential> {
        Read::Missing
    }
}

/// The credentials of this computer's user.
#[derive(Debug, Clone)]
pub struct SystemCredentials {
    /// The home directory.
    pub home: PathBuf,
}

/// The text of a file: `Missing` when there is none.
fn read_file(path: &std::path::Path) -> Read<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Read::Found(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Read::Missing,
        Err(_) => Read::Unavailable,
    }
}

/// A directory named by an environment variable when it is set and not empty.
fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Reads a file under a directory and parses it: signed out when the file is
/// there but is not what the agent writes.
fn parse_file<T>(path: PathBuf, parse: impl FnOnce(&str) -> Option<T>) -> Read<T> {
    match read_file(&path) {
        Read::Found(text) => parse(&text).map_or(Read::Missing, Read::Found),
        Read::Missing => Read::Missing,
        Read::Unavailable | Read::Denied => Read::Unavailable,
    }
}

impl SystemCredentials {
    /// The token `cursor-agent` keeps in the macOS keychain.
    #[cfg(target_os = "macos")]
    fn cursor_keychain(&self) -> Read<String> {
        let output = std::process::Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                "cursor-access-token",
                "-a",
                "cursor-user",
                "-w",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output();
        match output {
            Ok(output) => match classify_security(
                output.status.code(),
                &String::from_utf8_lossy(&output.stderr),
            ) {
                KeychainOutcome::Found => {
                    Read::Found(String::from_utf8_lossy(&output.stdout).trim().to_owned())
                }
                KeychainOutcome::Denied => Read::Denied,
                KeychainOutcome::NotFound | KeychainOutcome::Other => Read::Missing,
            },
            Err(_) => Read::Missing,
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn cursor_keychain(&self) -> Read<String> {
        Read::Missing
    }

    /// The folder of `cursor-agent`'s own files.
    fn cursor_cli_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home.join(".cursor")
        } else if cfg!(windows) {
            env_dir("APPDATA")
                .unwrap_or_else(|| self.home.join("AppData").join("Roaming"))
                .join("Cursor")
        } else {
            env_dir("XDG_CONFIG_HOME")
                .unwrap_or_else(|| self.home.join(".config"))
                .join("cursor")
        }
    }
}

impl Credentials for SystemCredentials {
    fn claude(&self) -> Read<claude::Credential> {
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut denied = false;
        #[cfg(target_os = "macos")]
        {
            // The keychain item the CLI itself writes. The first read may make
            // macOS ask the user for permission.
            let output = std::process::Command::new("security")
                .args([
                    "find-generic-password",
                    "-s",
                    "Claude Code-credentials",
                    "-w",
                ])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .output();
            if let Ok(output) = output {
                match classify_security(
                    output.status.code(),
                    &String::from_utf8_lossy(&output.stderr),
                ) {
                    KeychainOutcome::Found => {
                        if let Some(found) =
                            claude::parse_credential(&String::from_utf8_lossy(&output.stdout))
                        {
                            return Read::Found(found);
                        }
                    }
                    KeychainOutcome::Denied => denied = true,
                    KeychainOutcome::NotFound | KeychainOutcome::Other => {}
                }
            }
        }
        let dir = env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| self.home.join(".claude"));
        let missing = || if denied { Read::Denied } else { Read::Missing };
        match read_file(&dir.join(".credentials.json")) {
            Read::Found(text) => claude::parse_credential(&text).map_or_else(missing, Read::Found),
            Read::Missing => missing(),
            Read::Unavailable | Read::Denied => Read::Unavailable,
        }
    }

    fn opencode_go(&self) -> Read<Secret> {
        let data =
            env_dir("XDG_DATA_HOME").unwrap_or_else(|| self.home.join(".local").join("share"));
        parse_file(data.join("opencode").join("auth.json"), opencode::parse_key)
    }

    fn codex(&self) -> Read<codex::Credential> {
        let dir = env_dir("CODEX_HOME").unwrap_or_else(|| self.home.join(".codex"));
        parse_file(dir.join("auth.json"), codex::parse_credential)
    }

    fn grok(&self, now: i64) -> Read<grok::Credential> {
        let dir = env_dir("GROK_HOME").unwrap_or_else(|| self.home.join(".grok"));
        parse_file(dir.join("auth.json"), |text| {
            grok::parse_credential(text, now)
        })
    }

    fn cursor(&self, now: i64) -> Read<cursor::Session> {
        // The keychain first, then the older file; a live session wins over
        // an expired one, wherever it is.
        let mut expired = None;
        let mut denied = false;
        let tokens = [
            self.cursor_keychain(),
            parse_file(
                self.cursor_cli_dir().join("auth.json"),
                cursor::parse_auth_file,
            ),
        ];
        for token in tokens {
            match token {
                Read::Found(token) => {
                    if let Some(session) = cursor::parse_session(&token, now) {
                        if !session.expired {
                            return Read::Found(session);
                        }
                        expired.get_or_insert(session);
                    }
                }
                Read::Denied => denied = true,
                Read::Missing | Read::Unavailable => {}
            }
        }
        match expired {
            Some(session) => Read::Found(session),
            None if denied => Read::Denied,
            None => Read::Missing,
        }
    }

    fn kimi(&self, now: i64) -> Read<kimi::Credential> {
        let dir = env_dir("KIMI_CODE_HOME").unwrap_or_else(|| self.home.join(".kimi-code"));
        parse_file(dir.join("credentials").join("kimi-code.json"), |text| {
            kimi::parse_credential(text, now)
        })
    }

    fn zcode(&self) -> Read<zcode::Credential> {
        parse_file(
            self.home.join(".zcode").join("cli").join("config.json"),
            zcode::parse_credential,
        )
    }
}

/// Slows a source that keeps failing: after each failure the wait doubles, up
/// to a limit; a success clears it. A vendor's `Retry-After` is honoured.
#[derive(Debug, Default, Clone, Copy)]
pub struct Throttle {
    failures: u32,
    not_before: i64,
}

impl Throttle {
    /// The first wait after a failure, in seconds.
    pub const FIRST: i64 = 60;
    /// The longest wait, in seconds, when the vendor named none.
    pub const LONGEST: i64 = 30 * 60;
    /// The longest `Retry-After` honoured, in seconds.
    pub const LONGEST_RETRY_AFTER: i64 = 60 * 60;

    /// Whether a call may be made at `now`.
    pub fn allows(&self, now: i64) -> bool {
        now >= self.not_before
    }

    /// When the next call may be made, after a failure.
    pub fn retry_at(&self) -> Option<i64> {
        (self.failures > 0).then_some(self.not_before)
    }

    /// Notes a call's outcome.
    pub fn record(&mut self, ok: bool, now: i64) {
        self.record_with(ok, now, None, 0);
    }

    /// Notes a call's outcome with the wait the vendor asked for and a jitter
    /// of `jitter_percent` (0 to 10) of the wait, so that many installs do not
    /// call at the same moment.
    pub fn record_with(
        &mut self,
        ok: bool,
        now: i64,
        retry_after: Option<i64>,
        jitter_percent: i64,
    ) {
        if ok {
            *self = Self::default();
            return;
        }
        self.failures = self.failures.saturating_add(1);
        let backoff = (Self::FIRST << (self.failures - 1).min(10)).min(Self::LONGEST);
        let asked = retry_after.unwrap_or(0).clamp(0, Self::LONGEST_RETRY_AFTER);
        let wait = backoff.max(asked);
        self.not_before = now + wait + wait * jitter_percent.clamp(0, 10) / 100;
    }

    /// Forgets a back-off: the user asked again, or the source was switched.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// What a source answered that is not a success.
fn failure(agent: AgentId, machine: &str, answer: Result<Response, HttpError>) -> AgentUsage {
    let reason = match answer {
        Ok(response) if matches!(response.status, 401 | 403) => Reason::NotSignedIn,
        Ok(response) if response.status == 429 => {
            Reason::RateLimited(response.retry_after.unwrap_or(0))
        }
        Ok(response) => Reason::VendorError(response.status),
        Err(HttpError::Unreachable) => Reason::Offline,
        Err(HttpError::Refused) => Reason::Unreachable,
    };
    AgentUsage::unknown(agent, machine, reason)
}

/// The credential, or the reading that says why there is none.
#[allow(clippy::result_large_err)] // the reading is what the caller returns as it is
fn credential<T>(
    read: Read<T>,
    missing: Reason,
    agent: AgentId,
    machine: &str,
) -> Result<T, AgentUsage> {
    let reason = match read {
        Read::Found(found) => return Ok(found),
        Read::Missing => missing,
        Read::Unavailable => Reason::Unreachable,
        Read::Denied => Reason::KeychainDenied,
    };
    Err(AgentUsage::unknown(agent, machine, reason))
}

/// The answer of a call when it is a success (HTTP 200), else the reading that
/// says what went wrong.
#[allow(clippy::result_large_err)] // the reading is what the caller returns as it is
async fn call(
    http: &dyn Http,
    request: &Request<'_>,
    agent: AgentId,
    machine: &str,
) -> Result<String, AgentUsage> {
    match http.get(request).await {
        Ok(response) if response.status == 200 => Ok(response.body),
        other => Err(failure(agent, machine, other)),
    }
}

/// Claude's usage, if its source is on.
pub async fn claude_usage(
    policy: &NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    machine: &str,
    now: i64,
) -> AgentUsage {
    let agent = AgentId::CLAUDE;
    if !policy.allows(agent) {
        return AgentUsage::unknown(agent, machine, Reason::SourceDisabled);
    }
    let credential = match credential(credentials.claude(), Reason::NotSignedIn, agent, machine) {
        Ok(credential) => credential,
        Err(usage) => return usage,
    };
    let request = Request {
        url: claude::URL,
        auth: Auth::Bearer(&credential.token),
        headers: vec![
            ("anthropic-beta", "oauth-2025-04-20".to_owned()),
            ("Accept", "application/json".to_owned()),
        ],
    };
    match call(http, &request, agent, machine).await {
        Ok(body) => claude::parse_usage(&body, machine, credential.plan, now),
        Err(usage) => usage,
    }
}

/// The opencode Go subscription's usage, if its source is on.
pub async fn opencode_usage(
    policy: &NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    machine: &str,
    now: i64,
) -> AgentUsage {
    let agent = AgentId::OPENCODE;
    if !policy.allows(agent) {
        return AgentUsage::unknown(agent, machine, Reason::SourceDisabled);
    }
    let key = match credential(
        credentials.opencode_go(),
        Reason::NotSupported,
        agent,
        machine,
    ) {
        Ok(key) => key,
        Err(usage) => return usage,
    };
    let request = Request {
        url: opencode::URL,
        auth: Auth::Bearer(&key),
        headers: vec![("Accept", "application/json".to_owned())],
    };
    match call(http, &request, agent, machine).await {
        Ok(body) => opencode::parse_usage(&body, machine, now),
        Err(usage) => usage,
    }
}

/// Codex's limits from the backend its own usage screen reads: fresher than
/// the session log when Codex has not run lately. It reads `auth.json` and
/// makes one GET; it starts no session and writes nothing.
pub async fn codex_usage(
    policy: &NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    machine: &str,
    now: i64,
) -> AgentUsage {
    let agent = AgentId::CODEX;
    if !policy.allows(agent) {
        return AgentUsage::unknown(agent, machine, Reason::SourceDisabled);
    }
    let credential = match credential(credentials.codex(), Reason::NotSignedIn, agent, machine) {
        Ok(credential) => credential,
        Err(usage) => return usage,
    };
    let mut headers = vec![("Accept", "application/json".to_owned())];
    if let Some(id) = &credential.account_id {
        headers.push(("ChatGPT-Account-Id", id.clone()));
    }
    let request = Request {
        url: codex::BACKEND_URL,
        auth: Auth::Bearer(&credential.token),
        headers,
    };
    match call(http, &request, agent, machine).await {
        Ok(body) => codex::parse_backend(&body, machine, now),
        Err(usage) => usage,
    }
}

/// Grok's limits, if its source is on: the credits view of the billing
/// endpoint, and the default view when only that one carries the budget.
pub async fn grok_usage(
    policy: &NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    machine: &str,
    now: i64,
) -> AgentUsage {
    let agent = AgentId::GROK;
    if !policy.allows(agent) {
        return AgentUsage::unknown(agent, machine, Reason::SourceDisabled);
    }
    let credential = match credential(credentials.grok(now), Reason::NotSignedIn, agent, machine) {
        Ok(credential) => credential,
        Err(usage) => return usage,
    };
    if credential.expired {
        return AgentUsage::unknown(agent, machine, Reason::SessionExpired);
    }
    let get = |url: &'static str| {
        let mut headers = vec![
            (grok::AUTH_HEADER.0, grok::AUTH_HEADER.1.to_owned()),
            ("Accept", "application/json".to_owned()),
        ];
        if let Some(id) = &credential.user_id {
            headers.push(("x-userid", id.clone()));
        }
        (url, headers)
    };
    let (url, headers) = get(grok::CREDITS_URL);
    let request = Request {
        url,
        auth: Auth::Bearer(&credential.token),
        headers,
    };
    let body = match call(http, &request, agent, machine).await {
        Ok(body) => body,
        Err(usage) => return usage,
    };
    let Some(mut billing) = grok::parse_billing(&body) else {
        return AgentUsage::unknown(agent, machine, Reason::NoData);
    };
    if billing.weekly.is_none() && billing.monthly.is_none() {
        // Some unified-billing accounts carry the budget only in the default
        // view.
        let (url, headers) = get(grok::DEFAULT_URL);
        let request = Request {
            url,
            auth: Auth::Bearer(&credential.token),
            headers,
        };
        match call(http, &request, agent, machine).await {
            Ok(body) => {
                if let Some(second) = grok::parse_billing(&body) {
                    billing.monthly = second.monthly;
                    billing.tier = billing.tier.or(second.tier);
                    billing.reports_amounts |= second.reports_amounts;
                }
            }
            Err(usage) => return usage,
        }
    }
    grok::reading(&billing, machine, now)
}

/// Cursor's limits, if its source is on: the usage summary and, for a plan
/// that reports nothing there, the older request-quota endpoint.
pub async fn cursor_usage(
    policy: &NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    machine: &str,
    now: i64,
) -> AgentUsage {
    let agent = AgentId::CURSOR;
    if !policy.allows(agent) {
        return AgentUsage::unknown(agent, machine, Reason::SourceDisabled);
    }
    let session = match credential(credentials.cursor(now), Reason::NotSignedIn, agent, machine) {
        Ok(session) => session,
        Err(usage) => return usage,
    };
    if session.expired {
        return AgentUsage::unknown(agent, machine, Reason::SessionExpired);
    }
    let headers = || {
        vec![
            ("Accept", "application/json".to_owned()),
            ("Origin", "https://cursor.com".to_owned()),
            ("Referer", "https://cursor.com/dashboard".to_owned()),
        ]
    };
    let summary_request = Request {
        url: cursor::SUMMARY_URL,
        auth: Auth::Header {
            name: "Cookie",
            value: &session.cookie,
        },
        headers: headers(),
    };
    let answer = http.get(&summary_request).await;
    // The dashboard answers 401 to a session it no longer knows.
    if matches!(&answer, Ok(response) if response.status == 401) {
        return AgentUsage::unknown(agent, machine, Reason::SessionExpired);
    }
    let body = match answer {
        Ok(response) if response.status == 200 => response.body,
        other => return failure(agent, machine, other),
    };
    let Some(summary) = cursor::parse_summary(&body) else {
        return AgentUsage::unknown(agent, machine, Reason::ParseError);
    };
    if summary.unlimited || !summary.windows.is_empty() {
        return cursor::reading(&summary, None, machine, now);
    }
    let url = format!(
        "{}?user={}",
        cursor::LEGACY_URL,
        cursor::escape(&session.subject)
    );
    let legacy_request = Request {
        url: &url,
        auth: Auth::Header {
            name: "Cookie",
            value: &session.cookie,
        },
        headers: headers(),
    };
    match call(http, &legacy_request, agent, machine).await {
        Ok(body) => cursor::reading(&summary, cursor::parse_legacy(&body), machine, now),
        Err(usage) => usage,
    }
}

/// Kimi Code's limits, if its source is on.
pub async fn kimi_usage(
    policy: &NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    machine: &str,
    now: i64,
) -> AgentUsage {
    let agent = AgentId::KIMI;
    if !policy.allows(agent) {
        return AgentUsage::unknown(agent, machine, Reason::SourceDisabled);
    }
    let credential = match credential(credentials.kimi(now), Reason::NotSignedIn, agent, machine) {
        Ok(credential) => credential,
        Err(usage) => return usage,
    };
    if credential.expired {
        return AgentUsage::unknown(agent, machine, Reason::SessionExpired);
    }
    let request = Request {
        url: kimi::URL,
        auth: Auth::Bearer(&credential.token),
        headers: vec![("Accept", "application/json".to_owned())],
    };
    match call(http, &request, agent, machine).await {
        Ok(body) => kimi::parse_usage(&body, machine, now),
        Err(usage) => usage,
    }
}

/// The ZCode plan's quota, if its source is on.
pub async fn zcode_usage(
    policy: &NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    machine: &str,
    now: i64,
) -> AgentUsage {
    let agent = AgentId::ZCODE;
    if !policy.allows(agent) {
        return AgentUsage::unknown(agent, machine, Reason::SourceDisabled);
    }
    let credential = match credential(credentials.zcode(), Reason::NotSignedIn, agent, machine) {
        Ok(credential) => credential,
        Err(usage) => return usage,
    };
    let request = Request {
        url: &credential.url,
        auth: Auth::Header {
            name: "Authorization",
            value: &credential.key,
        },
        headers: vec![
            ("Accept-Language", "en-US,en".to_owned()),
            ("Content-Type", "application/json".to_owned()),
        ],
    };
    match call(http, &request, agent, machine).await {
        Ok(body) => zcode::parse_usage(&body, machine, now),
        Err(usage) => usage,
    }
}

/// The reading of agent `agent` from its network source: the switch, the
/// credential, the call. Codex and Antigravity are not read here (see
/// [`codex_usage`] and the collection's command for `agy`).
pub async fn network_usage(
    agent: AgentId,
    policy: &NetworkPolicy,
    credentials: &dyn Credentials,
    http: &dyn Http,
    machine: &str,
    now: i64,
) -> AgentUsage {
    match agent {
        AgentId::CLAUDE => claude_usage(policy, credentials, http, machine, now).await,
        AgentId::OPENCODE => opencode_usage(policy, credentials, http, machine, now).await,
        AgentId::CODEX => codex_usage(policy, credentials, http, machine, now).await,
        AgentId::GROK => grok_usage(policy, credentials, http, machine, now).await,
        AgentId::CURSOR => cursor_usage(policy, credentials, http, machine, now).await,
        AgentId::KIMI => kimi_usage(policy, credentials, http, machine, now).await,
        AgentId::ZCODE => zcode_usage(policy, credentials, http, machine, now).await,
        other => AgentUsage::unknown(other, machine, Reason::NotSupported),
    }
}

/// Whether `agent` has a source that is called over the network from this
/// computer (and so reads a credential and needs the switch).
pub fn has_network_source(agent: AgentId) -> bool {
    matches!(
        agent,
        AgentId::CLAUDE
            | AgentId::OPENCODE
            | AgentId::CODEX
            | AgentId::GROK
            | AgentId::CURSOR
            | AgentId::KIMI
            | AgentId::ZCODE
    )
}

/// Whether `agent` has a source that asks the agent's own command (`agy`).
pub fn has_command_source(agent: AgentId) -> bool {
    agent == AgentId::ANTIGRAVITY
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Source, State};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeCredentials {
        claude_json: Option<&'static str>,
        key: Option<&'static str>,
        reads: AtomicUsize,
    }

    impl Credentials for FakeCredentials {
        fn claude(&self) -> Read<claude::Credential> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.claude_json
                .and_then(claude::parse_credential)
                .map_or(Read::Missing, Read::Found)
        }
        fn opencode_go(&self) -> Read<Secret> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.key
                .map_or(Read::Missing, |k| Read::Found(Secret::new(k)))
        }
    }

    const CLAUDE_OK: &str =
        r#"{"claudeAiOauth":{"accessToken":"tok-secret","subscriptionType":"max"}}"#;
    const ANSWER: &str = r#"{"five_hour":{"utilization":10,"resets_at":1791300000},"seven_day":{"utilization":91,"resets_at":1791300000}}"#;

    #[tokio::test]
    async fn a_disabled_claude_source_reads_nothing_and_calls_nothing() {
        let credentials = FakeCredentials {
            claude_json: Some(CLAUDE_OK),
            ..Default::default()
        };
        let http = ScriptedHttp::new().reply(200, ANSWER);
        let usage = claude_usage(&NetworkPolicy::default(), &credentials, &http, "local", 1).await;
        assert_eq!(
            usage.state,
            State::Unknown {
                reason: Reason::SourceDisabled
            }
        );
        assert_eq!(http.calls().len(), 0);
        assert_eq!(credentials.reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_disabled_opencode_source_reads_nothing_and_calls_nothing() {
        let credentials = FakeCredentials {
            key: Some("k"),
            ..Default::default()
        };
        let http = ScriptedHttp::new();
        let usage =
            opencode_usage(&NetworkPolicy::default(), &credentials, &http, "local", 1).await;
        assert_eq!(
            usage.state,
            State::Unknown {
                reason: Reason::SourceDisabled
            }
        );
        assert_eq!(http.calls().len(), 0);
        assert_eq!(credentials.reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_enabled_claude_source_calls_the_vendor_once_and_parses() {
        let credentials = FakeCredentials {
            claude_json: Some(CLAUDE_OK),
            ..Default::default()
        };
        let http = ScriptedHttp::new().reply(200, ANSWER);
        let policy = NetworkPolicy::none().with(AgentId::CLAUDE);
        let usage = claude_usage(&policy, &credentials, &http, "local", 1_790_000_000).await;
        assert_eq!(http.calls(), [claude::URL]);
        assert_eq!(usage.source, Some(Source::VendorApi));
        assert_eq!(usage.plan.as_deref(), Some("max"));
        assert!(matches!(usage.state, State::Known { .. }));
    }

    #[tokio::test]
    async fn a_missing_credential_is_signed_out_and_makes_no_call() {
        let credentials = FakeCredentials::default();
        let http = ScriptedHttp::new();
        let policy = NetworkPolicy::none()
            .with(AgentId::CLAUDE)
            .with(AgentId::OPENCODE);
        let usage = claude_usage(&policy, &credentials, &http, "local", 1).await;
        assert_eq!(
            usage.state,
            State::Unknown {
                reason: Reason::NotSignedIn
            }
        );
        assert_eq!(http.calls().len(), 0);
    }

    #[tokio::test]
    async fn failures_are_unknown_never_a_number() {
        let credentials = FakeCredentials {
            claude_json: Some(CLAUDE_OK),
            ..Default::default()
        };
        let policy = NetworkPolicy::none().with(AgentId::CLAUDE);
        for (http, reason) in [
            (ScriptedHttp::new().reply(401, "{}"), Reason::NotSignedIn),
            (ScriptedHttp::new().reply(429, "{}"), Reason::RateLimited(0)),
            (
                ScriptedHttp::new().reply_after(429, "{}", Some(120)),
                Reason::RateLimited(120),
            ),
            (
                ScriptedHttp::new().reply(500, "oops"),
                Reason::VendorError(500),
            ),
            (
                ScriptedHttp::new().fail(HttpError::Unreachable),
                Reason::Offline,
            ),
            (ScriptedHttp::new().reply(200, "<html>"), Reason::ParseError),
        ] {
            let usage = claude_usage(&policy, &credentials, &http, "local", 1).await;
            assert_eq!(usage.state, State::Unknown { reason }, "{reason:?}");
        }
    }

    #[tokio::test]
    async fn an_error_never_carries_the_credential() {
        let credentials = FakeCredentials {
            claude_json: Some(CLAUDE_OK),
            ..Default::default()
        };
        let http = ScriptedHttp::new().reply(500, "tok-secret echoed");
        let policy = NetworkPolicy::none().with(AgentId::CLAUDE);
        let usage = claude_usage(&policy, &credentials, &http, "local", 1).await;
        assert!(!format!("{usage:?}").contains("tok-secret"));
    }

    #[tokio::test]
    async fn the_opencode_go_source_parses_its_meters() {
        let credentials = FakeCredentials {
            key: Some("k"),
            ..Default::default()
        };
        let http = ScriptedHttp::new().reply(
            200,
            r#"{"usage":{"rolling":{"percent":5,"resetsAt":"2026-10-05T12:00:00Z"},"weekly":{"percent":6,"resetsAt":"2026-10-09T00:00:00Z"}}}"#,
        );
        let policy = NetworkPolicy::none().with(AgentId::OPENCODE);
        let usage = opencode_usage(&policy, &credentials, &http, "local", 1_790_000_000).await;
        assert_eq!(http.calls(), [opencode::URL]);
        assert!(matches!(usage.state, State::Known { .. }));
    }

    #[test]
    fn a_credential_is_only_ever_sent_to_an_allowed_host() {
        assert!(host_allowed("https://api.anthropic.com/api/oauth/usage"));
        assert!(host_allowed("https://opencode.ai/zen/go/v1/usage"));
        for url in [
            "http://api.anthropic.com/api/oauth/usage",
            "https://api.anthropic.com.evil.example/x",
            "https://evil.example/https://api.anthropic.com/",
            "https://api.anthropic.com@evil.example/",
            "ftp://opencode.ai/",
            "",
        ] {
            assert!(!host_allowed(url), "{url}");
        }
    }

    #[tokio::test]
    async fn curl_refuses_a_foreign_host_before_starting_anything() {
        let secret = Secret::new("tok");
        let request = Request {
            url: "https://evil.example/",
            auth: Auth::Bearer(&secret),
            headers: vec![],
        };
        assert_eq!(CurlHttp.get(&request).await, Err(HttpError::Refused));
    }

    #[test]
    fn the_curl_config_carries_the_header_and_escapes_quotes() {
        let secret = Secret::new("a\"b\\c");
        let request = Request {
            url: claude::URL,
            auth: Auth::Bearer(&secret),
            headers: vec![("Accept", "application/json".to_owned())],
        };
        let config = curl_config(&request);
        assert!(config.contains(r#"url = "https://api.anthropic.com/api/oauth/usage""#));
        assert!(config.contains(r#"header = "Authorization: Bearer a\"b\\c""#));
        assert!(config.contains(r#"header = "Accept: application/json""#));
    }

    #[test]
    fn a_request_never_shows_its_credential_in_debug_output() {
        let secret = Secret::new("tok-very-secret");
        let request = Request {
            url: claude::URL,
            auth: Auth::Bearer(&secret),
            headers: vec![],
        };
        assert!(!format!("{request:?}").contains("tok-very-secret"));
    }

    #[test]
    fn a_retry_after_longer_than_the_backoff_is_honoured_and_capped() {
        let mut throttle = Throttle::default();
        throttle.record_with(false, 1000, Some(300), 0);
        assert_eq!(throttle.retry_at(), Some(1300));
        assert!(!throttle.allows(1299));
        throttle.record_with(false, 2000, Some(1_000_000), 0);
        assert_eq!(
            throttle.retry_at(),
            Some(2000 + Throttle::LONGEST_RETRY_AFTER)
        );
        throttle.clear();
        assert_eq!(throttle.retry_at(), None);
        assert!(throttle.allows(0));
    }

    #[test]
    fn the_jitter_only_ever_lengthens_the_wait_by_a_tenth_at_most() {
        let mut throttle = Throttle::default();
        throttle.record_with(false, 0, None, 10);
        assert_eq!(throttle.retry_at(), Some(66));
        throttle.clear();
        throttle.record_with(false, 0, None, 99);
        assert_eq!(throttle.retry_at(), Some(66), "capped at ten percent");
    }

    #[test]
    fn curl_output_gives_the_status_the_body_and_the_retry_after() {
        let out = "HTTP/2 429\r\nretry-after: 90\r\ncontent-type: x\r\n\r\n{\"e\":1}\n429";
        let r = parse_curl_output(out).unwrap();
        assert_eq!((r.status, r.retry_after), (429, Some(90)));
        assert_eq!(r.body, "{\"e\":1}");
        let interim = "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n\r\nbody\n200";
        let r = parse_curl_output(interim).unwrap();
        assert_eq!(
            (r.status, r.body.as_str(), r.retry_after),
            (200, "body", None)
        );
        assert!(parse_curl_output("\n000").is_none());
        let date = "HTTP/2 503\r\nRetry-After: Wed, 21 Oct 2026 07:28:00 GMT\r\n\r\n\n503";
        assert_eq!(parse_curl_output(date).unwrap().retry_after, None);
    }

    #[test]
    fn a_refused_keychain_is_told_from_a_missing_item() {
        assert_eq!(classify_security(Some(0), ""), KeychainOutcome::Found);
        assert_eq!(classify_security(Some(44), ""), KeychainOutcome::NotFound);
        assert_eq!(classify_security(Some(128), ""), KeychainOutcome::Denied);
        assert_eq!(
            classify_security(Some(1), "User interaction is not allowed."),
            KeychainOutcome::Denied
        );
        assert_eq!(classify_security(Some(1), "boom"), KeychainOutcome::Other);
    }

    #[tokio::test]
    async fn a_denied_keychain_is_its_own_reason_and_makes_no_call() {
        struct Denied;
        impl Credentials for Denied {
            fn claude(&self) -> Read<claude::Credential> {
                Read::Denied
            }
            fn opencode_go(&self) -> Read<Secret> {
                Read::Missing
            }
        }
        let http = ScriptedHttp::new();
        let policy = NetworkPolicy::none().with(AgentId::CLAUDE);
        let usage = claude_usage(&policy, &Denied, &http, "local", 1).await;
        assert_eq!(
            usage.state,
            State::Unknown {
                reason: Reason::KeychainDenied
            }
        );
        assert_eq!(http.calls().len(), 0);
    }

    #[test]
    fn the_throttle_backs_off_and_resets_on_success() {
        let mut throttle = Throttle::default();
        assert!(throttle.allows(0));
        throttle.record(false, 0);
        assert!(!throttle.allows(59));
        assert!(throttle.allows(60));
        throttle.record(false, 60);
        assert!(!throttle.allows(60 + 119));
        assert!(throttle.allows(60 + 120));
        for _ in 0..20 {
            throttle.record(false, 1000);
        }
        assert!(!throttle.allows(1000 + Throttle::LONGEST - 1));
        assert!(throttle.allows(1000 + Throttle::LONGEST));
        throttle.record(true, 5000);
        assert!(throttle.allows(5000));
    }
}

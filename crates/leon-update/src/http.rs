//! How Leon talks to GitHub: one small trait and the system's `curl` behind it.
//!
//! The trait ([`Http`]) makes two kinds of call, both a single request that
//! follows no redirect: a bounded request whose answer is kept in memory
//! ([`Http::request`]: the release, `SHA256SUMS`, a `HEAD` that asks where a file
//! is), and a download into a file that can start part-way through
//! ([`Http::fetch`]). Redirects are followed by the caller one hop at a time, so
//! that every address, the ones GitHub redirects to included, is checked
//! against the allow-list ([`HostPolicy`]) before anything is asked of it.
//!
//! [`CurlHttp`] is the real client: the `curl` every supported system has (it
//! ships with macOS, with Windows 10 and later, and with nearly every Linux),
//! started through `leon_remote::spawn` so that no console window flashes on
//! Windows, exactly as the usage readings do. It takes `--proto =https`,
//! `--proto-redir =https`, no redirects of its own, timeouts, a speed floor
//! and no credentials of any kind. `curl` resumes (`--continue-at`) on all
//! three systems and the file it writes grows as it goes, which is how
//! progress is read; so there is no reason to carry a TLS client of our own.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// A future that can be sent between threads.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a progress callback is told: the bytes the file holds now.
pub type Progress = Arc<dyn Fn(u64) + Send + Sync>;

/// The most a call kept in memory may hold (a page of releases is far less).
pub const MAX_BODY: u64 = 4 * 1024 * 1024;

/// The hosts that may be asked, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPolicy {
    /// GitHub: the API for this repository's releases, `github.com` for this
    /// repository's release downloads, and the hosts GitHub redirects release
    /// assets to. Over HTTPS only.
    Github,
    /// A server on this computer, over plain HTTP. Tests only.
    #[cfg(test)]
    Loopback,
}

/// The parts of a URL the policy reads.
struct Url<'a> {
    scheme: &'a str,
    host: String,
    path: &'a str,
}

fn split_url(url: &str) -> Option<Url<'_>> {
    if url.bytes().any(|b| b <= b' ' || b == 0x7f || b == b'\\') || !url.is_ascii() {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    Some(Url {
        scheme,
        host: authority.to_ascii_lowercase(),
        path,
    })
}

/// The hosts release assets are served from after GitHub's redirect.
const CDN_HOSTS: [&str; 2] = [
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

impl HostPolicy {
    /// Whether `url` may be asked.
    pub fn allows(&self, url: &str) -> bool {
        let Some(parts) = split_url(url) else {
            return false;
        };
        match self {
            HostPolicy::Github => {
                if parts.scheme != "https" {
                    return false;
                }
                match parts.host.as_str() {
                    "api.github.com" => parts.path.starts_with("/repos/zavudev/leon/releases"),
                    "github.com" => parts.path.starts_with("/zavudev/leon/releases/download/"),
                    host => CDN_HOSTS.contains(&host),
                }
            }
            #[cfg(test)]
            HostPolicy::Loopback => parts.scheme == "http" && parts.host.starts_with("127.0.0.1:"),
        }
    }

    /// The value of curl's `--proto` for it.
    fn proto(&self) -> &'static str {
        match self {
            HostPolicy::Github => "=https",
            #[cfg(test)]
            HostPolicy::Loopback => "=http",
        }
    }
}

/// A call whose answer is kept in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Get {
    /// Where.
    pub url: String,
    /// Extra request headers (`If-None-Match`, `Accept`).
    pub headers: Vec<(String, String)>,
    /// Ask for the headers only.
    pub head: bool,
}

impl Get {
    /// A plain `GET`.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            head: false,
        }
    }

    /// A `HEAD`.
    pub fn head(url: impl Into<String>) -> Self {
        Self {
            head: true,
            ..Self::new(url)
        }
    }

    /// With a header.
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_owned(), value.into()));
        self
    }
}

/// A download into a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileGet {
    /// Where from.
    pub url: String,
    /// The file written (appended to when `resume_from` is above zero).
    pub dest: PathBuf,
    /// The bytes the file already holds: the transfer asks for the rest.
    pub resume_from: u64,
    /// The most the whole file may be: a transfer that would go past it is
    /// stopped.
    pub max_total: u64,
}

/// An answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Response {
    /// The HTTP status.
    pub status: u16,
    /// The headers, names in lower case.
    pub headers: Vec<(String, String)>,
    /// The body (empty for a `HEAD` and for a download).
    pub body: Vec<u8>,
}

impl Response {
    /// A header's value.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body as text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Why nothing came back.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    /// The address is not one that may be asked.
    #[error("the address is not on the list of hosts Leon asks: {0}")]
    Refused(String),
    /// Offline, a timeout, a refused connection, a stalled transfer or no `curl`.
    #[error("cannot reach the server: {0}")]
    Unreachable(String),
    /// The server does not do byte ranges, so the transfer cannot resume.
    #[error("the server does not resume a download")]
    RangeUnsupported,
    /// The answer, or the file, is larger than allowed.
    #[error("the answer is larger than allowed")]
    TooLarge,
    /// A local file could not be written.
    #[error("cannot write the download: {0}")]
    Io(String),
}

/// The two kinds of call.
pub trait Http: Send + Sync {
    /// One request, no redirect followed, the answer in memory.
    fn request<'a>(&'a self, get: Get) -> BoxFuture<'a, Result<Response, HttpError>>;

    /// One download, no redirect followed. The file is written only for a
    /// `200` (from its start) or a `206` (appended); any other status leaves it
    /// as it was. The answer carries the status and no body.
    fn fetch<'a>(
        &'a self,
        get: FileGet,
        progress: Progress,
    ) -> BoxFuture<'a, Result<Response, HttpError>>;
}

// ----- the real client ----------------------------------------------------------

/// The `User-Agent` of every call: GitHub's API refuses a call without one.
pub fn user_agent() -> String {
    format!(
        "Leon/{} (+https://github.com/zavudev/leon)",
        env!("CARGO_PKG_VERSION")
    )
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

/// The `curl` configuration of a call: its address and headers, on standard
/// input.
fn curl_config(url: &str, headers: &[(String, String)]) -> String {
    let mut config = format!("url = {}\n", config_quote(url));
    for (name, value) in headers {
        config.push_str(&format!(
            "header = {}\n",
            config_quote(&format!("{name}: {value}"))
        ));
    }
    config
}

/// Reads what `curl --include --write-out '\n%{http_code}'` printed.
pub fn parse_curl_output(bytes: &[u8]) -> Option<Response> {
    let split = bytes.iter().rposition(|b| *b == b'\n')?;
    let (rest, status) = (&bytes[..split], &bytes[split + 1..]);
    let status: u16 = std::str::from_utf8(status)
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|status| *status != 0)?;
    let mut remaining = rest;
    let mut headers = Vec::new();
    // An interim `100 Continue`, or a proxy's `200 Connection established`,
    // has a block of its own before the answer's: the last block counts.
    while remaining.starts_with(b"HTTP/") {
        let (head, body) = match find(remaining, b"\r\n\r\n") {
            Some(at) => (&remaining[..at], &remaining[at + 4..]),
            None => match find(remaining, b"\n\n") {
                Some(at) => (&remaining[..at], &remaining[at + 2..]),
                None => (remaining, &remaining[remaining.len()..]),
            },
        };
        headers.clear();
        for line in String::from_utf8_lossy(head).lines().skip(1) {
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
            }
        }
        remaining = body;
    }
    Some(Response {
        status,
        headers,
        body: remaining.to_vec(),
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Calls with the system's `curl`.
#[derive(Clone, Copy, Debug)]
pub struct CurlHttp {
    policy: HostPolicy,
}

impl Default for CurlHttp {
    fn default() -> Self {
        Self::new()
    }
}

/// How long a call kept in memory may take.
const REQUEST_TIME: Duration = Duration::from_secs(30);

/// How long a download may take in all (a stalled one is stopped long before:
/// below 1 KiB/s for 30 s).
const FETCH_TIME: Duration = Duration::from_secs(3600);

impl CurlHttp {
    /// The client for GitHub.
    pub fn new() -> Self {
        Self {
            policy: HostPolicy::Github,
        }
    }

    #[cfg(test)]
    fn loopback() -> Self {
        Self {
            policy: HostPolicy::Loopback,
        }
    }

    /// The flags every call has: no `.curlrc`, the protocol limits, no
    /// redirect, no credentials, an agent.
    fn base(&self) -> tokio::process::Command {
        let mut command = leon_remote::spawn::child("curl");
        command
            // First, or it is not read: leave the user's `.curlrc` out of it.
            .arg("-q")
            .args(["--silent", "--proto", self.policy.proto()])
            .args(["--proto-redir", self.policy.proto()])
            .args(["--max-redirs", "0"])
            .args(["--connect-timeout", "15"])
            .args(["--user-agent", &user_agent()])
            .args(["--config", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        command
    }
}

async fn feed(child: &mut tokio::process::Child, config: String) -> Result<(), HttpError> {
    use tokio::io::AsyncWriteExt as _;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| HttpError::Unreachable("no standard input".into()))?;
    stdin
        .write_all(config.as_bytes())
        .await
        .map_err(|error| HttpError::Unreachable(error.to_string()))
}

fn curl_failure(code: Option<i32>, stderr: &[u8]) -> HttpError {
    match code {
        Some(33) => HttpError::RangeUnsupported,
        Some(63) => HttpError::TooLarge,
        _ => HttpError::Unreachable(format!(
            "curl: {}",
            String::from_utf8_lossy(stderr)
                .trim()
                .lines()
                .last()
                .unwrap_or("failed")
        )),
    }
}

impl Http for CurlHttp {
    fn request<'a>(&'a self, get: Get) -> BoxFuture<'a, Result<Response, HttpError>> {
        Box::pin(async move {
            if !self.policy.allows(&get.url) {
                return Err(HttpError::Refused(get.url));
            }
            let mut command = self.base();
            command
                .args(["--include", "--write-out", "\n%{http_code}"])
                .args(["--max-time", &REQUEST_TIME.as_secs().to_string()])
                .args(["--max-filesize", &MAX_BODY.to_string()]);
            if get.head {
                command.arg("--head");
            }
            let mut child = command
                .spawn()
                .map_err(|error| HttpError::Unreachable(format!("curl: {error}")))?;
            feed(&mut child, curl_config(&get.url, &get.headers)).await?;
            let output = tokio::time::timeout(
                REQUEST_TIME + Duration::from_secs(5),
                child.wait_with_output(),
            )
            .await
            .map_err(|_| HttpError::Unreachable("timed out".into()))?
            .map_err(|error| HttpError::Unreachable(error.to_string()))?;
            match parse_curl_output(&output.stdout) {
                Some(response) => Ok(response),
                None => Err(curl_failure(output.status.code(), &output.stderr)),
            }
        })
    }

    fn fetch<'a>(
        &'a self,
        get: FileGet,
        progress: Progress,
    ) -> BoxFuture<'a, Result<Response, HttpError>> {
        Box::pin(async move {
            if !self.policy.allows(&get.url) {
                return Err(HttpError::Refused(get.url));
            }
            let mut command = self.base();
            command
                .args(["--show-error", "--fail"])
                .args(["--write-out", "%{http_code}"])
                .args(["--speed-limit", "1024", "--speed-time", "30"])
                .args(["--max-time", &FETCH_TIME.as_secs().to_string()])
                .args([
                    "--max-filesize",
                    &get.max_total
                        .saturating_sub(get.resume_from)
                        .max(1)
                        .to_string(),
                ])
                .arg("--output")
                .arg(&get.dest);
            if get.resume_from > 0 {
                command.args(["--continue-at", &get.resume_from.to_string()]);
            }
            let mut child = command
                .spawn()
                .map_err(|error| HttpError::Unreachable(format!("curl: {error}")))?;
            feed(&mut child, curl_config(&get.url, &[])).await?;
            let mut stdout = child.stdout.take();
            let mut stderr = child.stderr.take();
            let read_out = tokio::spawn(async move {
                use tokio::io::AsyncReadExt as _;
                let mut bytes = Vec::new();
                if let Some(out) = stdout.as_mut() {
                    let _ = out.read_to_end(&mut bytes).await;
                }
                bytes
            });
            let read_err = tokio::spawn(async move {
                use tokio::io::AsyncReadExt as _;
                let mut bytes = Vec::new();
                if let Some(err) = stderr.as_mut() {
                    let _ = err.read_to_end(&mut bytes).await;
                }
                bytes
            });
            // The file grows as curl writes it: that is the progress, and the
            // check that a server cannot send more than was declared.
            let status = loop {
                tokio::select! {
                    status = child.wait() => {
                        break status.map_err(|error| HttpError::Unreachable(error.to_string()))?;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(150)) => {
                        let size = std::fs::metadata(&get.dest).map(|meta| meta.len()).unwrap_or(0);
                        progress(size);
                        if size > get.max_total {
                            let _ = child.kill().await;
                            return Err(HttpError::TooLarge);
                        }
                    }
                }
            };
            let out = read_out.await.unwrap_or_default();
            let err = read_err.await.unwrap_or_default();
            let size = std::fs::metadata(&get.dest)
                .map(|meta| meta.len())
                .unwrap_or(0);
            progress(size);
            let code: Option<u16> = String::from_utf8_lossy(&out)
                .trim()
                .parse()
                .ok()
                .filter(|code| *code != 0);
            match (status.success(), code) {
                (true, Some(code)) => Ok(Response {
                    status: code,
                    ..Response::default()
                }),
                // `--fail`: an error status is an answer, not a transfer failure.
                (false, Some(code)) if status.code() == Some(22) => Ok(Response {
                    status: code,
                    ..Response::default()
                }),
                _ => Err(curl_failure(status.code(), &err)),
            }
        })
    }
}

// ----- a scripted client, for tests ----------------------------------------------

/// What a scripted route answers.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug)]
pub enum Reply {
    /// A status with headers and a body, for [`Http::request`]; a
    /// [`Http::fetch`] of it writes the body.
    Status {
        /// The status.
        status: u16,
        /// The headers.
        headers: Vec<(String, String)>,
        /// The body.
        body: Vec<u8>,
    },
    /// A file that supports byte ranges. `cut` ends the transfer after that
    /// many bytes of it with a failure, as a lost connection does.
    File {
        /// The whole file.
        bytes: Vec<u8>,
        /// Where a lost connection cuts it, counted from the start of the file.
        cut: Option<usize>,
        /// Whether the server honours `Range`.
        ranges: bool,
        /// How many bytes are really sent beyond the file's length (a server
        /// that sends too much).
        extra: usize,
    },
    /// The call fails.
    Fail(HttpError),
}

#[cfg(any(test, feature = "test-support"))]
impl Reply {
    /// A status with a text body.
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::Status {
            status: 200,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// A status with headers and no body.
    pub fn status(status: u16, headers: &[(&str, &str)]) -> Self {
        Self::Status {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: Vec::new(),
        }
    }

    /// A redirect.
    pub fn redirect(to: &str) -> Self {
        Self::status(302, &[("location", to)])
    }

    /// A file that resumes.
    pub fn file(bytes: Vec<u8>) -> Self {
        Self::File {
            bytes,
            cut: None,
            ranges: true,
            extra: 0,
        }
    }
}

/// A client that answers from a script: each address has a queue of
/// replies, one used per call (the last stays for the calls after it), and
/// every call is written down.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct ScriptedHttp {
    routes: std::sync::Mutex<Vec<(String, std::collections::VecDeque<Reply>)>>,
    calls: std::sync::Mutex<Vec<String>>,
}

#[cfg(any(test, feature = "test-support"))]
impl ScriptedHttp {
    /// A client with no routes: every call is unreachable.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a reply for an address.
    pub fn route(self, url: &str, reply: Reply) -> Self {
        {
            let mut routes = self.routes.lock().unwrap();
            match routes.iter_mut().find(|(known, _)| known == url) {
                Some((_, queue)) => queue.push_back(reply),
                None => routes.push((url.to_owned(), std::iter::once(reply).collect())),
            }
        }
        self
    }

    /// Replaces everything scripted for an address by one reply.
    pub fn set(&self, url: &str, reply: Reply) {
        let mut routes = self.routes.lock().unwrap();
        routes.retain(|(known, _)| known != url);
        routes.push((url.to_owned(), std::iter::once(reply).collect()));
    }

    /// Queues a reply on a client already shared.
    pub fn push(&self, url: &str, reply: Reply) {
        let mut routes = self.routes.lock().unwrap();
        match routes.iter_mut().find(|(known, _)| known == url) {
            Some((_, queue)) => queue.push_back(reply),
            None => routes.push((url.to_owned(), std::iter::once(reply).collect())),
        }
    }

    /// The calls so far: `GET url`, `HEAD url`, `FETCH url from=N`.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// The reply for an address; `consume` takes it off the queue (a `HEAD`
    /// only looks).
    fn next(&self, url: &str, consume: bool) -> Option<Reply> {
        let mut routes = self.routes.lock().unwrap();
        let (_, queue) = routes.iter_mut().find(|(known, _)| known == url)?;
        if consume && queue.len() > 1 {
            queue.pop_front()
        } else {
            queue.front().cloned()
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Http for ScriptedHttp {
    fn request<'a>(&'a self, get: Get) -> BoxFuture<'a, Result<Response, HttpError>> {
        Box::pin(async move {
            // A real call takes time: other work gets to run meanwhile.
            tokio::task::yield_now().await;
            let kind = if get.head { "HEAD" } else { "GET" };
            let conditional = get
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("if-none-match"))
                .map(|(_, value)| format!(" if-none-match={value}"))
                .unwrap_or_default();
            self.calls
                .lock()
                .unwrap()
                .push(format!("{kind} {}{conditional}", get.url));
            match self.next(&get.url, !get.head) {
                None => Err(HttpError::Unreachable("no route".into())),
                Some(Reply::Fail(error)) => Err(error),
                Some(Reply::Status {
                    status,
                    headers,
                    body,
                }) => Ok(Response {
                    status,
                    headers,
                    body: if get.head { Vec::new() } else { body },
                }),
                Some(Reply::File { bytes, .. }) => Ok(Response {
                    status: 200,
                    headers: vec![
                        ("content-length".into(), bytes.len().to_string()),
                        ("accept-ranges".into(), "bytes".into()),
                    ],
                    body: if get.head { Vec::new() } else { bytes },
                }),
            }
        })
    }

    fn fetch<'a>(
        &'a self,
        get: FileGet,
        progress: Progress,
    ) -> BoxFuture<'a, Result<Response, HttpError>> {
        Box::pin(async move {
            use std::io::Write as _;
            self.calls
                .lock()
                .unwrap()
                .push(format!("FETCH {} from={}", get.url, get.resume_from));
            let io = |error: std::io::Error| HttpError::Io(error.to_string());
            match self.next(&get.url, true) {
                None => Err(HttpError::Unreachable("no route".into())),
                Some(Reply::Fail(error)) => Err(error),
                Some(Reply::Status { status, .. }) => Ok(Response {
                    status,
                    ..Response::default()
                }),
                Some(Reply::File {
                    bytes,
                    cut,
                    ranges,
                    extra,
                }) => {
                    if get.resume_from > 0 && !ranges {
                        return Err(HttpError::RangeUnsupported);
                    }
                    let from = get.resume_from as usize;
                    if from > bytes.len() {
                        return Ok(Response {
                            status: 416,
                            ..Response::default()
                        });
                    }
                    let mut sent = bytes[from..].to_vec();
                    sent.extend(std::iter::repeat_n(0u8, extra));
                    let status = if from > 0 { 206 } else { 200 };
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .append(from > 0)
                        .truncate(from == 0)
                        .open(&get.dest)
                        .map_err(io)?;
                    let end =
                        cut.map_or(sent.len(), |cut| cut.saturating_sub(from).min(sent.len()));
                    file.write_all(&sent[..end]).map_err(io)?;
                    file.flush().map_err(io)?;
                    let size = std::fs::metadata(&get.dest)
                        .map(|meta| meta.len())
                        .unwrap_or(0);
                    progress(size);
                    if size > get.max_total {
                        return Err(HttpError::TooLarge);
                    }
                    if cut.is_some() && end < sent.len() {
                        return Err(HttpError::Unreachable("connection lost".into()));
                    }
                    Ok(Response {
                        status,
                        ..Response::default()
                    })
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_repositorys_release_addresses_are_asked() {
        let github = HostPolicy::Github;
        for good in [
            "https://api.github.com/repos/zavudev/leon/releases/latest",
            "https://api.github.com/repos/zavudev/leon/releases?per_page=15",
            "https://github.com/zavudev/leon/releases/download/v0.2.1/SHA256SUMS",
            "https://objects.githubusercontent.com/github-production-release-asset/1/2?sig=x",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/2?sp=r",
            "https://RELEASE-ASSETS.githubusercontent.com/x",
        ] {
            assert!(github.allows(good), "{good}");
        }
        for bad in [
            "http://api.github.com/repos/zavudev/leon/releases/latest",
            "https://api.github.com/repos/other/leon/releases/latest",
            "https://api.github.com/user",
            "https://github.com/other/leon/releases/download/v1/x",
            "https://github.com/zavudev/leon/archive/main.tar.gz",
            "https://evil.example/zavudev/leon/releases/download/v1/x",
            "https://github.com.evil.example/zavudev/leon/releases/download/v1/x",
            "https://objects.githubusercontent.com.evil.example/x",
            "https://user@github.com/zavudev/leon/releases/download/v1/x",
            "https://github.com:8443/zavudev/leon/releases/download/v1/x",
            "https://raw.githubusercontent.com/zavudev/leon/main/x",
            "https://github.com\\@evil.example/zavudev/leon/releases/download/v1/x",
            "https://github.com/zavudev/leon/releases/download/v1/x y",
            "ftp://github.com/zavudev/leon/releases/download/v1/x",
            "file:///etc/passwd",
            "//github.com/zavudev/leon/releases/download/v1/x",
            "",
        ] {
            assert!(!github.allows(bad), "{bad}");
        }
    }

    #[test]
    fn curl_output_gives_status_headers_and_body() {
        let out =
            b"HTTP/2 200\r\netag: W/\"abc\"\r\nX-RateLimit-Remaining: 59\r\n\r\n{\"a\":1}\n200";
        let response = parse_curl_output(out).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.header("etag"), Some("W/\"abc\""));
        assert_eq!(response.header("x-ratelimit-remaining"), Some("59"));
        assert_eq!(response.body, b"{\"a\":1}");
    }

    #[test]
    fn an_interim_block_is_skipped_and_a_head_has_no_body() {
        let out = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 304 Not Modified\r\nETag: x\r\n\r\n\n304";
        let response = parse_curl_output(out).unwrap();
        assert_eq!((response.status, response.header("etag")), (304, Some("x")));
        assert!(response.body.is_empty());
        assert!(parse_curl_output(b"\n000").is_none());
        assert!(parse_curl_output(b"nothing").is_none());
    }

    #[test]
    fn the_curl_config_quotes_what_it_carries() {
        let config = curl_config("https://x/y", &[("If-None-Match".into(), "W/\"a\"".into())]);
        assert!(config.contains("url = \"https://x/y\""));
        assert!(config.contains(r#"header = "If-None-Match: W/\"a\"""#));
    }

    #[tokio::test]
    async fn curl_refuses_a_foreign_address_before_starting_anything() {
        let http = CurlHttp::new();
        assert_eq!(
            http.request(Get::new("https://evil.example/")).await,
            Err(HttpError::Refused("https://evil.example/".into()))
        );
        let get = FileGet {
            url: "http://github.com/zavudev/leon/releases/download/v1/x".into(),
            dest: std::env::temp_dir().join("never"),
            resume_from: 0,
            max_total: 10,
        };
        assert!(matches!(
            http.fetch(get, Arc::new(|_| {})).await,
            Err(HttpError::Refused(_))
        ));
    }

    // ----- the real curl, against a server on this computer ----------------------

    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    /// A server that answers each connection from a closure of the request
    /// text, once, and says how many it served.
    fn serve(
        handler: impl Fn(&str) -> Vec<u8> + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let handle = std::thread::spawn(move || {
            listener.set_nonblocking(false).unwrap();
            // Serve until a connection that sends `STOP`.
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                if request.starts_with("STOP") {
                    return;
                }
                let _ = stream.write_all(&handler(&request));
            }
        });
        (base, handle)
    }

    fn stop(base: &str, handle: std::thread::JoinHandle<()>) {
        let address = base.trim_start_matches("http://");
        if let Ok(mut stream) = std::net::TcpStream::connect(address) {
            let _ = stream.write_all(b"STOP");
        }
        handle.join().unwrap();
    }

    fn curl_present() -> bool {
        std::process::Command::new("curl")
            .arg("--version")
            .output()
            .is_ok()
    }

    #[tokio::test]
    async fn the_real_curl_gets_a_status_headers_and_a_body_and_sends_what_it_should() {
        if !curl_present() {
            return;
        }
        let (base, handle) = serve(|request| {
            let agent = request.to_ascii_lowercase().contains("user-agent: leon/");
            let conditional = request.contains("If-None-Match: \"v1\"");
            let body = format!("agent={agent} conditional={conditional}");
            format!(
                "HTTP/1.1 200 OK\r\nETag: \"v2\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_bytes()
        });
        let http = CurlHttp::loopback();
        let response = http
            .request(Get::new(format!("{base}/x")).header("If-None-Match", "\"v1\""))
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.header("etag"), Some("\"v2\""));
        assert_eq!(response.text(), "agent=true conditional=true");
        stop(&base, handle);
    }

    #[tokio::test]
    async fn the_real_curl_does_not_follow_a_redirect() {
        if !curl_present() {
            return;
        }
        let (base, handle) = serve(|_| {
            b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec()
        });
        let response = CurlHttp::loopback()
            .request(Get::head(format!("{base}/x")))
            .await
            .unwrap();
        assert_eq!(response.status, 302);
        assert_eq!(
            response.header("location"),
            Some("http://127.0.0.1:1/elsewhere")
        );
        stop(&base, handle);
    }

    #[tokio::test]
    async fn the_real_curl_writes_a_file_and_resumes_it() {
        if !curl_present() {
            return;
        }
        let data: Vec<u8> = (0..100_000u32).map(|n| (n % 251) as u8).collect();
        let served = data.clone();
        let (base, handle) = serve(move |request| {
            let from = request
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("range: bytes=")
                        .map(str::to_owned)
                })
                .and_then(|range| range.trim_end_matches('-').parse::<usize>().ok());
            let mut out = match from {
                Some(from) => format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {from}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    served.len() - 1,
                    served.len(),
                    served.len() - from
                )
                .into_bytes(),
                None => format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    served.len()
                )
                .into_bytes(),
            };
            out.extend_from_slice(&served[from.unwrap_or(0)..]);
            out
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.part");
        // Half of it is already there.
        std::fs::write(&dest, &data[..40_000]).unwrap();
        let seen = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let seen_in = seen.clone();
        let response = CurlHttp::loopback()
            .fetch(
                FileGet {
                    url: format!("{base}/asset"),
                    dest: dest.clone(),
                    resume_from: 40_000,
                    max_total: 100_000,
                },
                Arc::new(move |size| seen_in.store(size, std::sync::atomic::Ordering::SeqCst)),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 206);
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 100_000);
        stop(&base, handle);
    }

    #[tokio::test]
    async fn the_real_curl_reports_an_error_status_without_writing_the_body() {
        if !curl_present() {
            return;
        }
        let (base, handle) = serve(|_| {
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
                .to_vec()
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f");
        let response = CurlHttp::loopback()
            .fetch(
                FileGet {
                    url: format!("{base}/x"),
                    dest: dest.clone(),
                    resume_from: 0,
                    max_total: 1000,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 404);
        assert!(!dest.exists() || std::fs::metadata(&dest).unwrap().len() == 0);
        stop(&base, handle);
    }

    #[tokio::test]
    async fn the_real_curl_refuses_a_file_larger_than_allowed() {
        if !curl_present() {
            return;
        }
        let (base, handle) = serve(|_| {
            let mut out =
                b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\nConnection: close\r\n\r\n".to_vec();
            out.extend(std::iter::repeat_n(7u8, 5000));
            out
        });
        let dir = tempfile::tempdir().unwrap();
        let result = CurlHttp::loopback()
            .fetch(
                FileGet {
                    url: format!("{base}/x"),
                    dest: dir.path().join("f"),
                    resume_from: 0,
                    max_total: 1000,
                },
                Arc::new(|_| {}),
            )
            .await;
        assert_eq!(result, Err(HttpError::TooLarge));
        stop(&base, handle);
    }
}

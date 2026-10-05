//! The one network call Leon makes from this computer: downloading the avatar
//! of a repository's owner from its Git host.
//!
//! [`IconFetcher`] is what the engine asks; [`CurlFetcher`] answers it with
//! the system's `curl` (HTTPS only, redirects kept on HTTPS, a size cap and a
//! time limit, no credentials), [`NoFetch`] answers nothing and is what every
//! test and the default engine use, so nothing in the test suite can reach a
//! network. The caller tells a host that has no such image ([`Fetched::Missing`])
//! from one that could not be reached ([`Fetched::Unreachable`]): the first is
//! remembered, the second is tried again at the next refresh.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use leon_core::icon::MAX_ICON_BYTES;

/// How long a download may take, `curl` and the wait for it together.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// What asking for an image came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// The image's bytes.
    Bytes(Vec<u8>),
    /// The host answered that there is none.
    Missing,
    /// Nobody answered: offline, a timeout, a refused connection.
    Unreachable,
}

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Downloads an image.
pub trait IconFetcher: Send + Sync {
    /// Fetches `url`.
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Fetched>;
}

/// Fetches nothing.
pub struct NoFetch;

impl IconFetcher for NoFetch {
    fn fetch<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, Fetched> {
        Box::pin(async { Fetched::Unreachable })
    }
}

/// Fetches with the system's `curl`.
pub struct CurlFetcher;

impl IconFetcher for CurlFetcher {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Fetched> {
        Box::pin(async move {
            if !url.starts_with("https://") {
                return Fetched::Missing;
            }
            let mut command = tokio::process::Command::new("curl");
            command
                .args(["--fail", "--silent", "--location"])
                .args(["--proto", "=https", "--proto-redir", "=https"])
                .args(["--max-time", &FETCH_TIMEOUT.as_secs().to_string()])
                .args(["--max-filesize", &MAX_ICON_BYTES.to_string()])
                .args(["--user-agent", "Leon"])
                .arg(url)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true);
            let run =
                tokio::time::timeout(FETCH_TIMEOUT + Duration::from_secs(2), command.output());
            match run.await {
                Ok(Ok(output)) if output.status.success() => Fetched::Bytes(output.stdout),
                // 22: the server answered with an error status; 63: too big.
                Ok(Ok(output)) if matches!(output.status.code(), Some(22 | 63)) => Fetched::Missing,
                _ => Fetched::Unreachable,
            }
        })
    }
}

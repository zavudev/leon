//! Getting a release's files: `SHA256SUMS` into memory, the archive into a
//! file that can be resumed.
//!
//! GitHub answers a release download with a redirect to a signed address on
//! another host. Every hop is asked for by hand and checked against the
//! allow-list ([`HostPolicy`]) before it is followed, up to four; an address
//! outside it ends the download. The archive is written to `<name>.part`,
//! which a later attempt carries on from (`Range`), and renamed only when it
//! holds exactly the size the release states. A file larger than declared, or
//! larger than any release of Leon could be, is deleted.

use crate::http::{FileGet, Get, HostPolicy, Http, HttpError, Progress, Response};
use std::path::{Path, PathBuf};

/// The most an archive may be, whatever the release says.
pub const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;

/// The most `SHA256SUMS` may be.
pub const MAX_CHECKSUMS_BYTES: u64 = 64 * 1024;

/// How many redirects are followed.
const MAX_HOPS: usize = 4;

/// How many times a download that was cut off is carried on.
const ATTEMPTS: usize = 3;

/// Why a download failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// An address outside the allow-list.
    #[error("refusing an address that is not GitHub's release storage: {0}")]
    Refused(String),
    /// The transfer failed.
    #[error(transparent)]
    Http(#[from] HttpError),
    /// The server answered with a status that is not a file.
    #[error("the server answered {0}")]
    Status(u16),
    /// GitHub asks to come back later.
    #[error("GitHub asks to come back later")]
    RateLimited,
    /// More redirects than are followed.
    #[error("too many redirects")]
    Redirects,
    /// The size the release states is not one to download.
    #[error("the release states a size that is not acceptable ({0} bytes)")]
    Size(u64),
    /// The file ended before its size.
    #[error("the download stopped at {have} of {want} bytes")]
    Truncated {
        /// What is there.
        have: u64,
        /// What the release says.
        want: u64,
    },
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn is_limited(status: u16) -> bool {
    matches!(status, 403 | 429)
}

/// The address a redirect points to, made absolute against `from`.
fn location(response: &Response, from: &str) -> Option<String> {
    let to = response.header("location")?;
    if to.contains("://") {
        return Some(to.to_owned());
    }
    // A path on the same host: not what GitHub sends, but harmless to read.
    let scheme_end = from.find("://")? + 3;
    let host_end = from[scheme_end..]
        .find('/')
        .map_or(from.len(), |at| scheme_end + at);
    to.starts_with('/')
        .then(|| format!("{}{to}", &from[..host_end]))
}

/// A `GET` of a small answer, following redirects one hop at a time.
pub async fn get_small(http: &dyn Http, policy: &HostPolicy, url: &str) -> Result<Vec<u8>, Error> {
    let mut url = url.to_owned();
    for _ in 0..=MAX_HOPS {
        if !policy.allows(&url) {
            return Err(Error::Refused(url));
        }
        let response = http.request(Get::new(url.clone())).await?;
        match response.status {
            200 => {
                if response.body.len() as u64 > MAX_CHECKSUMS_BYTES {
                    return Err(Error::Http(HttpError::TooLarge));
                }
                return Ok(response.body);
            }
            status if is_redirect(status) => {
                url = location(&response, &url).ok_or(Error::Status(status))?;
            }
            status if is_limited(status) => return Err(Error::RateLimited),
            status => return Err(Error::Status(status)),
        }
    }
    Err(Error::Redirects)
}

/// Where a file really is: `HEAD` through the redirects. Gives the final
/// address and the length the server states there, when it does.
pub async fn resolve(
    http: &dyn Http,
    policy: &HostPolicy,
    url: &str,
) -> Result<(String, Option<u64>), Error> {
    let mut url = url.to_owned();
    for _ in 0..=MAX_HOPS {
        if !policy.allows(&url) {
            return Err(Error::Refused(url));
        }
        let response = http.request(Get::head(url.clone())).await?;
        match response.status {
            200 => {
                let length = response
                    .header("content-length")
                    .and_then(|length| length.parse().ok());
                return Ok((url, length));
            }
            status if is_redirect(status) => {
                url = location(&response, &url).ok_or(Error::Status(status))?;
            }
            status if is_limited(status) => return Err(Error::RateLimited),
            status => return Err(Error::Status(status)),
        }
    }
    Err(Error::Redirects)
}

/// Downloads `name` from `url` into `dir`, resuming what an earlier attempt
/// left, and gives its path once it holds exactly `size` bytes.
pub async fn fetch_asset(
    http: &dyn Http,
    policy: &HostPolicy,
    url: &str,
    dir: &Path,
    name: &str,
    size: u64,
    progress: Progress,
) -> Result<PathBuf, Error> {
    if size == 0 || size > MAX_ASSET_BYTES {
        return Err(Error::Size(size));
    }
    std::fs::create_dir_all(dir).map_err(|error| HttpError::Io(error.to_string()))?;
    let done = dir.join(name);
    let part = dir.join(format!("{name}.part"));
    if let Ok(meta) = std::fs::metadata(&done) {
        if meta.len() == size {
            progress(size);
            return Ok(done);
        }
        let _ = std::fs::remove_file(&done);
    }
    let mut last = Error::Truncated {
        have: 0,
        want: size,
    };
    for attempt in 0..ATTEMPTS {
        let (final_url, length) = resolve(http, policy, url).await?;
        if length.is_some_and(|length| length != size) {
            // The server says another size than the release does.
            return Err(Error::Size(length.unwrap_or_default()));
        }
        let have = std::fs::metadata(&part).map_or(0, |meta| meta.len());
        let have = if have > size {
            let _ = std::fs::remove_file(&part);
            0
        } else {
            have
        };
        if have < size {
            let get = FileGet {
                url: final_url,
                dest: part.clone(),
                resume_from: have,
                max_total: size,
            };
            match http.fetch(get, progress.clone()).await {
                Ok(response) if matches!(response.status, 200 | 206) => {}
                Ok(response) if response.status == 416 => {
                    // The server holds less than is there: start again.
                    let _ = std::fs::remove_file(&part);
                    last = Error::Status(416);
                    continue;
                }
                Ok(response) if is_limited(response.status) => return Err(Error::RateLimited),
                Ok(response) => return Err(Error::Status(response.status)),
                Err(HttpError::RangeUnsupported) => {
                    let _ = std::fs::remove_file(&part);
                    last = Error::Http(HttpError::RangeUnsupported);
                    continue;
                }
                Err(HttpError::TooLarge) => {
                    let _ = std::fs::remove_file(&part);
                    return Err(Error::Http(HttpError::TooLarge));
                }
                Err(error @ HttpError::Refused(_)) => return Err(Error::Http(error)),
                Err(error) => {
                    // A lost connection: what was written is kept, and the
                    // next attempt asks for the rest.
                    last = Error::Http(error);
                    if attempt + 1 < ATTEMPTS {
                        continue;
                    }
                    return Err(last);
                }
            }
        }
        let have = std::fs::metadata(&part).map_or(0, |meta| meta.len());
        if have == size {
            std::fs::rename(&part, &done).map_err(|error| HttpError::Io(error.to_string()))?;
            return Ok(done);
        }
        if have > size {
            let _ = std::fs::remove_file(&part);
            return Err(Error::Http(HttpError::TooLarge));
        }
        last = Error::Truncated { have, want: size };
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{Reply, ScriptedHttp};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    const START: &str = "https://github.com/zavudev/leon/releases/download/v0.2.1/a.tar.gz";
    const CDN: &str = "https://release-assets.githubusercontent.com/x/y?sig=1";

    fn bytes(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 253) as u8).collect()
    }

    fn quiet() -> Progress {
        Arc::new(|_| {})
    }

    fn github() -> ScriptedHttp {
        ScriptedHttp::new().route(START, Reply::redirect(CDN))
    }

    #[tokio::test]
    async fn a_file_is_downloaded_through_the_redirect_and_renamed_when_whole() {
        let data = bytes(5000);
        let http = github().route(CDN, Reply::file(data.clone()));
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(AtomicU64::new(0));
        let seen_in = seen.clone();
        let path = fetch_asset(
            &http,
            &HostPolicy::Github,
            START,
            dir.path(),
            "a.tar.gz",
            5000,
            Arc::new(move |n| seen_in.store(n, Ordering::SeqCst)),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        assert!(!dir.path().join("a.tar.gz.part").exists());
        assert_eq!(seen.load(Ordering::SeqCst), 5000);
    }

    #[tokio::test]
    async fn a_download_cut_off_is_resumed_from_where_it_stopped() {
        let data = bytes(5000);
        let http = github()
            .route(
                CDN,
                Reply::File {
                    bytes: data.clone(),
                    cut: Some(2000),
                    ranges: true,
                    extra: 0,
                },
            )
            .route(CDN, Reply::file(data.clone()));
        let dir = tempfile::tempdir().unwrap();
        let path = fetch_asset(
            &http,
            &HostPolicy::Github,
            START,
            dir.path(),
            "a.tar.gz",
            5000,
            quiet(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), data);
        let fetches: Vec<String> = http
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("FETCH"))
            .collect();
        assert_eq!(fetches.len(), 2);
        assert!(fetches[0].ends_with("from=0"), "{fetches:?}");
        assert!(fetches[1].ends_with("from=2000"), "{fetches:?}");
    }

    #[tokio::test]
    async fn a_part_file_from_an_earlier_run_is_carried_on() {
        let data = bytes(3000);
        let http = github().route(CDN, Reply::file(data.clone()));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.tar.gz.part"), &data[..1200]).unwrap();
        let path = fetch_asset(
            &http,
            &HostPolicy::Github,
            START,
            dir.path(),
            "a.tar.gz",
            3000,
            quiet(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), data);
        assert!(http.calls().iter().any(|call| call.ends_with("from=1200")));
    }

    #[tokio::test]
    async fn a_server_that_does_not_resume_makes_the_download_start_again() {
        let data = bytes(3000);
        let http = github().route(
            CDN,
            Reply::File {
                bytes: data.clone(),
                cut: None,
                ranges: false,
                extra: 0,
            },
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.tar.gz.part"), &data[..1000]).unwrap();
        let path = fetch_asset(
            &http,
            &HostPolicy::Github,
            START,
            dir.path(),
            "a.tar.gz",
            3000,
            quiet(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), data);
    }

    #[tokio::test]
    async fn a_download_that_keeps_stopping_short_is_given_up_and_keeps_its_part() {
        let data = bytes(5000);
        let http = github().route(
            CDN,
            Reply::File {
                bytes: data,
                cut: Some(100),
                ranges: true,
                extra: 0,
            },
        );
        let dir = tempfile::tempdir().unwrap();
        let error = fetch_asset(
            &http,
            &HostPolicy::Github,
            START,
            dir.path(),
            "a.tar.gz",
            5000,
            quiet(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, Error::Http(HttpError::Unreachable(_))),
            "{error:?}"
        );
        assert!(dir.path().join("a.tar.gz.part").exists());
        assert!(!dir.path().join("a.tar.gz").exists());
    }

    #[tokio::test]
    async fn a_file_larger_than_declared_is_deleted() {
        let http = github().route(
            CDN,
            Reply::File {
                bytes: bytes(1000),
                cut: None,
                ranges: true,
                extra: 500,
            },
        );
        let dir = tempfile::tempdir().unwrap();
        let error = fetch_asset(
            &http,
            &HostPolicy::Github,
            START,
            dir.path(),
            "a.tar.gz",
            1000,
            quiet(),
        )
        .await
        .unwrap_err();
        assert_eq!(error, Error::Http(HttpError::TooLarge));
        assert!(!dir.path().join("a.tar.gz.part").exists());
        assert!(!dir.path().join("a.tar.gz").exists());
    }

    #[tokio::test]
    async fn a_declared_size_that_is_absurd_is_refused_before_any_request() {
        let http = github();
        let dir = tempfile::tempdir().unwrap();
        for size in [0, MAX_ASSET_BYTES + 1] {
            let error = fetch_asset(
                &http,
                &HostPolicy::Github,
                START,
                dir.path(),
                "a",
                size,
                quiet(),
            )
            .await
            .unwrap_err();
            assert_eq!(error, Error::Size(size));
        }
        assert!(http.calls().is_empty());
    }

    #[tokio::test]
    async fn a_server_that_states_another_length_is_refused() {
        let http = github().route(CDN, Reply::file(bytes(900)));
        let dir = tempfile::tempdir().unwrap();
        let error = fetch_asset(
            &http,
            &HostPolicy::Github,
            START,
            dir.path(),
            "a.tar.gz",
            1000,
            quiet(),
        )
        .await
        .unwrap_err();
        assert_eq!(error, Error::Size(900));
    }

    #[tokio::test]
    async fn a_redirect_to_a_foreign_host_is_refused_and_nothing_is_fetched() {
        let http = ScriptedHttp::new()
            .route(START, Reply::redirect("https://evil.example/leon.tar.gz"))
            .route("https://evil.example/leon.tar.gz", Reply::file(bytes(10)));
        let dir = tempfile::tempdir().unwrap();
        let error = fetch_asset(
            &http,
            &HostPolicy::Github,
            START,
            dir.path(),
            "a.tar.gz",
            10,
            quiet(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            Error::Refused("https://evil.example/leon.tar.gz".into())
        );
        assert!(
            http.calls().iter().all(|call| !call.contains("evil")),
            "{:?}",
            http.calls()
        );
        // Nor a downgrade to plain http on an allowed host.
        let http = ScriptedHttp::new().route(
            START,
            Reply::redirect("http://release-assets.githubusercontent.com/x"),
        );
        assert!(matches!(
            fetch_asset(
                &http,
                &HostPolicy::Github,
                START,
                dir.path(),
                "a.tar.gz",
                10,
                quiet()
            )
            .await,
            Err(Error::Refused(_))
        ));
    }

    #[tokio::test]
    async fn an_address_outside_the_release_downloads_is_never_asked() {
        let http = ScriptedHttp::new();
        let dir = tempfile::tempdir().unwrap();
        let error = fetch_asset(
            &http,
            &HostPolicy::Github,
            "https://github.com/other/leon/releases/download/v1/a",
            dir.path(),
            "a",
            10,
            quiet(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, Error::Refused(_)));
        assert!(http.calls().is_empty());
    }

    #[tokio::test]
    async fn redirect_loops_end() {
        let http = ScriptedHttp::new().route(START, Reply::redirect(START));
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            fetch_asset(
                &http,
                &HostPolicy::Github,
                START,
                dir.path(),
                "a",
                10,
                quiet()
            )
            .await,
            Err(Error::Redirects)
        );
    }

    #[tokio::test]
    async fn rate_limits_and_errors_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let http = ScriptedHttp::new().route(START, Reply::status(429, &[]));
        assert_eq!(
            fetch_asset(
                &http,
                &HostPolicy::Github,
                START,
                dir.path(),
                "a",
                10,
                quiet()
            )
            .await,
            Err(Error::RateLimited)
        );
        let http = ScriptedHttp::new().route(START, Reply::status(404, &[]));
        assert_eq!(
            fetch_asset(
                &http,
                &HostPolicy::Github,
                START,
                dir.path(),
                "a",
                10,
                quiet()
            )
            .await,
            Err(Error::Status(404))
        );
    }

    #[tokio::test]
    async fn the_checksum_file_follows_redirects_and_is_bounded() {
        let sums = "https://github.com/zavudev/leon/releases/download/v0.2.1/SHA256SUMS";
        let http = ScriptedHttp::new()
            .route(sums, Reply::redirect(CDN))
            .route(CDN, Reply::ok("abc  file\n"));
        assert_eq!(
            get_small(&http, &HostPolicy::Github, sums).await.unwrap(),
            b"abc  file\n"
        );
        let big = ScriptedHttp::new().route(
            sums,
            Reply::ok(vec![b'a'; MAX_CHECKSUMS_BYTES as usize + 1]),
        );
        assert!(get_small(&big, &HostPolicy::Github, sums).await.is_err());
    }

    #[tokio::test]
    async fn a_complete_file_already_there_is_not_downloaded_again() {
        let http = ScriptedHttp::new();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.tar.gz"), bytes(100)).unwrap();
        fetch_asset(
            &http,
            &HostPolicy::Github,
            START,
            dir.path(),
            "a.tar.gz",
            100,
            quiet(),
        )
        .await
        .unwrap();
        assert!(http.calls().is_empty());
    }
}

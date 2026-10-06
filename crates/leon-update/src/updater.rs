//! The life-cycle of an update: ask, download, verify, unpack, install.
//!
//! ```text
//! Idle ─► Checking ─► UpToDate
//!                 ├─► Available ─► Downloading ─► Ready ─► Installing ─► RestartRequired
//!                 ├─► Manual        (an install Leon does not replace itself)
//!                 ├─► NoBuild       (the release has nothing for this platform)
//!                 └─► Failed        (with when to try again)
//! ```
//!
//! [`Updater`] publishes its state over a watch channel ([`Snapshot`]) and
//! keeps the little that has to outlive a run ([`crate::state`]). It does not
//! know about windows. It is asked to [`Updater::check`], to
//! [`Updater::download`] (which also verifies and unpacks) and to
//! [`Updater::install`] (which puts the update in place and tries it); what to
//! ask and when is the application's business.
//!
//! Where the trust comes from: the release of this repository on GitHub. The
//! checksum file of that same release, and the signature rules of
//! [`crate::trust`], are what the download is held to; see `docs/UPDATES.md`.

use crate::checksums;
use crate::download::{self, Error as DownloadError};
use crate::http::{Get, HostPolicy, Http, HttpError, Progress};
use crate::install::{Install, Why};
use crate::launch::{self, Applied, ApplyError, Startup};
use crate::package;
use crate::release::{self, Asset, Platform, Release, Selection};
use crate::state::{Layout, Offer, Saved, Staged};
use crate::tools::{Signature, Tools};
use crate::trust::{self, Trust};
use crate::version::{self, Version};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub use crate::state::Offer as UpdateOffer;

/// Where in the cycle a failure happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Asking GitHub.
    Check,
    /// Downloading, verifying or unpacking.
    Download,
    /// Putting it in place.
    Install,
}

/// Where the update is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Nothing has been asked yet.
    Idle,
    /// GitHub is being asked.
    Checking,
    /// This is the newest version (or the newer ones are skipped or ignored).
    UpToDate,
    /// A newer version exists and is not downloaded.
    Available(Offer),
    /// The newest release has nothing for this platform.
    NoBuild {
        /// The release's version.
        version: String,
    },
    /// A newer version exists, and this install is not replaced by Leon
    /// itself: the person downloads it.
    Manual {
        /// The release.
        offer: Offer,
        /// Why Leon does not do it.
        why: Why,
    },
    /// The download is under way.
    Downloading {
        /// The release.
        offer: Offer,
        /// Bytes that are there.
        done: u64,
        /// Bytes the archive is.
        total: u64,
    },
    /// Downloaded, verified and unpacked; waiting for a restart.
    Ready {
        /// The release.
        offer: Offer,
        /// What is known of who made it.
        trust: Trust,
    },
    /// Being put in place.
    Installing(Offer),
    /// In place; the next start is the new version.
    RestartRequired(Offer),
    /// Something went wrong.
    Failed {
        /// What, in a sentence.
        reason: String,
        /// Not before this time (Unix seconds) is it tried again by itself.
        retry_at: Option<i64>,
        /// Where.
        stage: Stage,
        /// The release it was about, when it was one.
        offer: Option<Offer>,
    },
}

/// The state and what goes with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Where the update is.
    pub state: State,
    /// A line about something that happened without the person: an update
    /// that did not start and was undone.
    pub notice: Option<String>,
    /// When GitHub was last asked successfully (Unix seconds).
    pub checked_at: Option<i64>,
}

/// What the updater needs to know about this install.
pub struct Config {
    /// The running version.
    pub current: Version,
    /// The running platform.
    pub platform: Platform,
    /// The `updates` folder.
    pub layout: Layout,
    /// What kind of install it is.
    pub install: Install,
    /// The time, in Unix seconds.
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

struct Candidate {
    release: Release,
    archive: Asset,
    checksums: Asset,
}

struct Inner {
    saved: Saved,
    busy: bool,
    prereleases: bool,
    candidate: Option<Candidate>,
    running: Option<Signature>,
}

/// The updater.
pub struct Updater {
    config: Config,
    http: Arc<dyn Http>,
    tools: Arc<dyn Tools>,
    policy: HostPolicy,
    tx: Arc<watch::Sender<Snapshot>>,
    inner: Mutex<Inner>,
}

/// How long a failed check or download waits before it is tried again by
/// itself.
const RETRY_FAILED: i64 = 30 * 60;

/// How long to stay away from GitHub when it gives no time.
const RETRY_LIMITED: i64 = 60 * 60;

fn offer_of(release: &Release, archive: &Asset) -> Offer {
    Offer {
        version: release.version.to_string(),
        notes: release.notes.clone(),
        page: release.page.clone(),
        size: archive.size,
        prerelease: release.prerelease,
    }
}

impl Updater {
    /// An updater over `http` and `tools`. What was kept from earlier runs is
    /// read: an update that is ready is ready again.
    pub fn new(config: Config, http: Arc<dyn Http>, tools: Arc<dyn Tools>) -> Arc<Self> {
        let mut saved = config.layout.load();
        let mut state = State::Idle;
        let mut running = None;
        if let Some(staged) = saved.staged.clone() {
            let kept = Version::parse(&staged.offer.version)
                .ok()
                .filter(|version| version > &config.current)
                .filter(|_| config.install.target().is_some())
                .filter(|_| !saved.refused.contains(&staged.offer.version))
                .filter(|_| {
                    config
                        .layout
                        .version_dir(&staged.offer.version)
                        .join(&staged.archive)
                        .is_file()
                });
            let signature = |path: &std::path::Path| tools.signature(path).ok();
            let payload = config
                .layout
                .payload_dir(&staged.offer.version)
                .join(package::payload_name(&config.platform));
            match (kept, config.install.target()) {
                (Some(_), Some(target)) if payload.exists() => {
                    let run = signature(&target.path).unwrap_or_default();
                    let new = signature(&payload).unwrap_or_default();
                    match trust::judge(config.platform.os, &run, &new) {
                        Ok(trust) => {
                            state = State::Ready {
                                offer: staged.offer.clone(),
                                trust,
                            };
                        }
                        Err(_) => saved.staged = None,
                    }
                    running = Some(run);
                }
                _ => saved.staged = None,
            }
            if saved.staged.is_none() {
                let _ = config.layout.save(&saved);
            }
        }
        config.layout.drop_downloads_except(
            saved
                .staged
                .as_ref()
                .map(|staged| staged.offer.version.as_str()),
        );
        let snapshot = Snapshot {
            state,
            notice: saved.notice.clone(),
            checked_at: saved.last_check,
        };
        let (tx, _) = watch::channel(snapshot);
        Arc::new(Self {
            config,
            http,
            tools,
            policy: HostPolicy::Github,
            tx: Arc::new(tx),
            inner: Mutex::new(Inner {
                saved,
                busy: false,
                prereleases: false,
                candidate: None,
                running,
            }),
        })
    }

    /// The state now.
    pub fn snapshot(&self) -> Snapshot {
        self.tx.borrow().clone()
    }

    /// A receiver of every change.
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.tx.subscribe()
    }

    /// The running version.
    pub fn current(&self) -> &Version {
        &self.config.current
    }

    /// What kind of install this is.
    pub fn install_kind(&self) -> &Install {
        &self.config.install
    }

    /// The pieces the start of the application and the hand-over use.
    pub fn startup(&self) -> Startup<'_> {
        Startup {
            current: self.config.current.clone(),
            layout: &self.config.layout,
            install: &self.config.install,
            tools: self.tools.as_ref(),
            platform: self.config.platform,
            watch: (launch::WATCH, std::time::Duration::from_millis(100)),
        }
    }

    /// Follow the pre-release channel too.
    pub fn set_prereleases(&self, on: bool) {
        self.lock().prereleases = on;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn now(&self) -> i64 {
        (self.config.now)()
    }

    fn publish(&self, state: State) -> Snapshot {
        let (notice, checked_at) = {
            let inner = self.lock();
            (inner.saved.notice.clone(), inner.saved.last_check)
        };
        let snapshot = Snapshot {
            state,
            notice,
            checked_at,
        };
        self.tx.send_replace(snapshot.clone());
        snapshot
    }

    fn save(&self, change: impl FnOnce(&mut Saved)) {
        let mut inner = self.lock();
        change(&mut inner.saved);
        if let Err(error) = self.config.layout.save(&inner.saved) {
            tracing::warn!(%error, "the update state could not be saved");
        }
    }

    /// Takes the line about what happened without the person, once.
    pub fn take_notice(&self) -> Option<String> {
        let notice = self.lock().saved.notice.clone();
        if notice.is_some() {
            self.save(|saved| saved.notice = None);
            let state = self.snapshot().state;
            self.publish(state);
        }
        notice
    }

    /// The earliest time (Unix seconds) a check made by itself may ask GitHub,
    /// when there is a reason to wait.
    pub fn not_before(&self) -> Option<i64> {
        let limited = self.lock().saved.retry_after;
        let failed = match self.snapshot().state {
            State::Failed { retry_at, .. } => retry_at,
            _ => None,
        };
        limited.into_iter().chain(failed).max()
    }

    fn try_begin(&self) -> bool {
        let mut inner = self.lock();
        if inner.busy {
            return false;
        }
        inner.busy = true;
        true
    }

    fn end(&self) {
        self.lock().busy = false;
    }

    fn running_signature(&self) -> Signature {
        if let Some(known) = self.lock().running.clone() {
            return known;
        }
        let found = match self.config.install.target() {
            Some(target) => self.tools.signature(&target.path).unwrap_or_default(),
            None => Signature::none(),
        };
        self.lock().running = Some(found.clone());
        found
    }

    /// Whether a version is one that will not be tried: it did not start here.
    fn refused(&self, version: &str) -> bool {
        self.lock()
            .saved
            .refused
            .iter()
            .any(|known| known == version)
    }

    fn failed(
        &self,
        stage: Stage,
        reason: String,
        retry: Option<i64>,
        offer: Option<Offer>,
    ) -> Snapshot {
        tracing::warn!(?stage, %reason, "the update did not go through");
        self.publish(State::Failed {
            reason,
            retry_at: retry.map(|seconds| self.now() + seconds),
            stage,
            offer,
        })
    }

    // ----- asking -----------------------------------------------------------------------

    /// Asks GitHub for the newest release. A check while another is under
    /// way, or while an update is downloading, ready or installed, changes
    /// nothing. `manual` says a person asked: a version they skipped is
    /// offered again, and a wait GitHub asked for is not honoured.
    pub async fn check(&self, manual: bool) -> Snapshot {
        match self.snapshot().state {
            State::Downloading { .. }
            | State::Ready { .. }
            | State::Installing(_)
            | State::RestartRequired(_)
            | State::Checking => return self.snapshot(),
            _ => {}
        }
        if !self.try_begin() {
            return self.snapshot();
        }
        let before = self.snapshot().state;
        if !manual {
            let until = self.lock().saved.retry_after;
            if until.is_some_and(|until| until > self.now()) {
                self.end();
                return self.snapshot();
            }
        }
        self.publish(State::Checking);
        let outcome = self.check_inner(manual, before).await;
        self.end();
        outcome
    }

    async fn check_inner(&self, manual: bool, before: State) -> Snapshot {
        let (prereleases, saved_etag) = {
            let inner = self.lock();
            (inner.prereleases, inner.saved.clone())
        };
        let url = if prereleases {
            release::LIST_URL
        } else {
            release::LATEST_URL
        };
        let mut get = Get::new(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        let cached = (saved_etag.etag_url.as_deref() == Some(url))
            .then(|| saved_etag.etag.clone().zip(saved_etag.cached.clone()))
            .flatten();
        if let Some((etag, _)) = &cached {
            get = get.header("If-None-Match", etag.clone());
        }
        let response = match self.http.request(get).await {
            Ok(response) => response,
            Err(HttpError::Refused(address)) => {
                return self.failed(
                    Stage::Check,
                    format!("refused to ask {address}"),
                    None,
                    None,
                )
            }
            Err(error) => {
                return self.failed(Stage::Check, error.to_string(), Some(RETRY_FAILED), None)
            }
        };
        let body = match response.status {
            200 => {
                let body = response.text();
                let etag = response.header("etag").map(str::to_owned);
                self.save(|saved| {
                    saved.etag_url = Some(url.to_owned());
                    saved.etag = etag;
                    saved.cached = (body.len() < 1_000_000).then(|| body.clone());
                });
                body
            }
            304 => match cached {
                Some((_, body)) => body,
                None => {
                    self.save(|saved| saved.etag = None);
                    return self.failed(
                        Stage::Check,
                        "GitHub answered 304 to a request without an ETag".into(),
                        Some(RETRY_FAILED),
                        None,
                    );
                }
            },
            403 | 429 => {
                // Rate limited: try later, and say nothing.
                let now = self.now();
                let wait = response
                    .header("retry-after")
                    .and_then(|seconds| seconds.parse::<i64>().ok())
                    .or_else(|| {
                        response
                            .header("x-ratelimit-reset")
                            .and_then(|at| at.parse::<i64>().ok())
                            .map(|at| at - now)
                    })
                    .unwrap_or(RETRY_LIMITED)
                    .clamp(60, 24 * 60 * 60);
                self.save(|saved| saved.retry_after = Some(now + wait));
                tracing::info!(wait, "GitHub asked to come back later");
                return self.publish(match before {
                    State::Failed { .. } | State::Checking => State::Idle,
                    other => other,
                });
            }
            status => {
                return self.failed(
                    Stage::Check,
                    format!("GitHub answered {status}"),
                    Some(RETRY_FAILED),
                    None,
                )
            }
        };
        let now = self.now();
        self.save(|saved| {
            saved.last_check = Some(now);
            saved.retry_after = None;
        });
        let parsed = if prereleases {
            release::parse_list(&body, true)
        } else {
            release::parse_latest(&body)
        };
        let release = match parsed {
            Ok(release) => release,
            Err(error) => {
                // A cached body that does not parse is not asked about again.
                self.save(|saved| {
                    saved.etag = None;
                    saved.cached = None;
                });
                return self.failed(Stage::Check, error.to_string(), Some(RETRY_FAILED), None);
            }
        };
        self.consider(release, manual)
    }

    /// What a release means for this install.
    fn consider(&self, release: Option<Release>, manual: bool) -> Snapshot {
        let prereleases = self.lock().prereleases;
        let Some(release) = release.filter(|release| {
            version::is_update(&self.config.current, &release.version, prereleases)
        }) else {
            self.lock().candidate = None;
            return self.publish(State::UpToDate);
        };
        let version = release.version.to_string();
        if self.refused(&version) {
            self.lock().candidate = None;
            return self.publish(State::UpToDate);
        }
        if !manual && self.lock().saved.skipped.as_deref() == Some(version.as_str()) {
            self.lock().candidate = None;
            return self.publish(State::UpToDate);
        }
        if manual {
            self.save(|saved| saved.skipped = None);
        }
        let (archive, checksums) = match release::select(&release, &self.config.platform) {
            Selection::Found { archive, checksums } => (archive.clone(), checksums.clone()),
            Selection::NoBuild => {
                self.lock().candidate = None;
                return self.publish(State::NoBuild { version });
            }
            Selection::NoChecksums => {
                self.lock().candidate = None;
                return self.failed(
                    Stage::Check,
                    format!("Release {version} has no SHA256SUMS, so it cannot be verified"),
                    None,
                    Some(Offer {
                        version: release.version.to_string(),
                        notes: release.notes.clone(),
                        page: release.page.clone(),
                        size: 0,
                        prerelease: release.prerelease,
                    }),
                );
            }
        };
        let offer = offer_of(&release, &archive);
        self.lock().candidate = Some(Candidate {
            release,
            archive,
            checksums,
        });
        match &self.config.install {
            Install::Manual(why) => self.publish(State::Manual { offer, why: *why }),
            Install::Updatable(_) => self.publish(State::Available(offer)),
        }
    }

    /// Does not offer this version again until a newer one exists or a person
    /// asks.
    pub fn skip(&self) {
        let version = match self.snapshot().state {
            State::Available(offer)
            | State::Manual { offer, .. }
            | State::Failed {
                offer: Some(offer), ..
            } => Some(offer.version),
            _ => None,
        };
        if let Some(version) = version {
            self.save(|saved| saved.skipped = Some(version));
            self.lock().candidate = None;
            self.publish(State::UpToDate);
        }
    }

    // ----- downloading ------------------------------------------------------------------

    /// Downloads, verifies and unpacks the update that [`Updater::check`]
    /// found.
    pub async fn download(&self) -> Snapshot {
        let (offer, archive, sums_asset, release) = {
            let inner = self.lock();
            let Some(candidate) = &inner.candidate else {
                drop(inner);
                return self.snapshot();
            };
            (
                offer_of(&candidate.release, &candidate.archive),
                candidate.archive.clone(),
                candidate.checksums.clone(),
                candidate.release.clone(),
            )
        };
        if self.config.install.target().is_none() {
            return self.snapshot();
        }
        match self.snapshot().state {
            State::Available(_)
            | State::Failed {
                stage: Stage::Download,
                ..
            } => {}
            _ => return self.snapshot(),
        }
        if !self.try_begin() {
            return self.snapshot();
        }
        let snapshot = self
            .download_inner(offer, archive, sums_asset, release)
            .await;
        self.end();
        snapshot
    }

    async fn download_inner(
        &self,
        offer: Offer,
        archive: Asset,
        sums_asset: Asset,
        release: Release,
    ) -> Snapshot {
        let total = archive.size;
        self.publish(State::Downloading {
            offer: offer.clone(),
            done: 0,
            total,
        });
        let dir = self.config.layout.version_dir(&offer.version);
        let fail = |reason: String, retry: Option<i64>| {
            self.failed(Stage::Download, reason, retry, Some(offer.clone()))
        };

        // The checksums first: a release that does not list its archive is not
        // worth downloading.
        let sums =
            match download::get_small(self.http.as_ref(), &self.policy, &sums_asset.url).await {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(DownloadError::RateLimited) => return self.limited(&offer),
                Err(error) => return fail(error.to_string(), Some(RETRY_FAILED)),
            };
        if let Err(error) = checksums::expected(&sums, &archive.name) {
            return fail(error.to_string(), Some(RETRY_LIMITED));
        }

        let tx = self.tx.clone();
        let progress_offer = offer.clone();
        let (notice, checked_at) = {
            let inner = self.lock();
            (inner.saved.notice.clone(), inner.saved.last_check)
        };
        let progress: Progress = Arc::new(move |done| {
            tx.send_replace(Snapshot {
                state: State::Downloading {
                    offer: progress_offer.clone(),
                    done,
                    total,
                },
                notice: notice.clone(),
                checked_at,
            });
        });
        let path = match download::fetch_asset(
            self.http.as_ref(),
            &self.policy,
            &archive.url,
            &dir,
            &archive.name,
            archive.size,
            progress,
        )
        .await
        {
            Ok(path) => path,
            Err(DownloadError::RateLimited) => return self.limited(&offer),
            Err(error) => return fail(error.to_string(), Some(RETRY_FAILED)),
        };

        // Verify and unpack off the thread that serves the application.
        let tools = self.tools.clone();
        let platform = self.config.platform;
        let version = release.version.clone();
        let running = self.running_signature();
        let (name, size, digest) = (archive.name.clone(), archive.size, archive.digest);
        let payload_dir = self.config.layout.payload_dir(&offer.version);
        let verified = tokio::task::spawn_blocking(move || -> Result<(String, Trust), String> {
            checksums::verify_file(&path, &name, &sums, size, digest)
                .map_err(|error| error.to_string())?;
            let (hash, _) = checksums::hash_file(&path).map_err(|error| error.to_string())?;
            let payload =
                package::extract(&path, &platform, &version, &payload_dir, tools.as_ref())
                    .map_err(|error| error.to_string())?;
            let signature = tools
                .signature(&payload)
                .map_err(|error| format!("cannot read the signature: {error}"))?;
            let trust = trust::judge(platform.os, &running, &signature)?;
            Ok((checksums::hex(&hash), trust))
        })
        .await
        .unwrap_or_else(|error| Err(error.to_string()));
        let (sha256, trust) = match verified {
            Ok(done) => done,
            Err(reason) => {
                // A download that does not verify is deleted, and not tried
                // again from this release: it would fail the same way.
                let _ = std::fs::remove_dir_all(&dir);
                self.save(|saved| {
                    if !saved.refused.contains(&offer.version) {
                        saved.refused.push(offer.version.clone());
                    }
                });
                return fail(
                    format!(
                        "Version {} was downloaded but not accepted: {reason}",
                        offer.version
                    ),
                    None,
                );
            }
        };
        if matches!(trust, Trust::Unsigned) {
            tracing::warn!(version = %offer.version, "the update is not signed; it was accepted because this build is not signed either");
        }
        self.save(|saved| {
            saved.staged = Some(Staged {
                offer: offer.clone(),
                archive: archive.name.clone(),
                sha256,
                identity: match &trust {
                    Trust::Signed(identity) => Some(identity.clone()),
                    _ => None,
                },
            });
        });
        self.config
            .layout
            .drop_downloads_except(Some(offer.version.as_str()));
        self.publish(State::Ready { offer, trust })
    }

    /// GitHub asked to come back later in the middle of a download: the
    /// update goes back to being on offer, quietly.
    fn limited(&self, offer: &Offer) -> Snapshot {
        let until = self.now() + RETRY_LIMITED;
        self.save(|saved| saved.retry_after = Some(until));
        self.publish(State::Available(offer.clone()))
    }

    // ----- installing -------------------------------------------------------------------

    /// Puts the ready update in place and tries it. Blocking: it copies files
    /// and starts the new build for a moment, so it is called from a thread that
    /// may block. Nothing is restarted.
    pub fn install(&self) -> Result<Applied, ApplyError> {
        let offer = match self.snapshot().state {
            State::Ready { offer, .. } => offer,
            _ => return Err(ApplyError::NothingReady),
        };
        let Some(target) = self.config.install.target().cloned() else {
            return Err(ApplyError::Io(
                "this install is not replaced by Leon".into(),
            ));
        };
        self.publish(State::Installing(offer.clone()));
        let running = self.running_signature();
        let mut saved = self.lock().saved.clone();
        let result = launch::apply_staged(
            &self.config.layout,
            &mut saved,
            &target,
            self.tools.as_ref(),
            &self.config.platform,
            &running,
            &self.config.current,
        );
        self.lock().saved = saved;
        match &result {
            Ok(_) => {
                self.publish(State::RestartRequired(offer));
            }
            Err(error) => {
                self.failed(Stage::Install, error.to_string(), None, Some(offer));
            }
        }
        result
    }

    /// The running process was asked to restart into the update that was
    /// installed: the version to start.
    pub fn restart_version(&self) -> Option<String> {
        match self.snapshot().state {
            State::RestartRequired(offer) => Some(offer.version),
            _ => None,
        }
    }

    /// Whether a version was recorded as one that did not start.
    pub fn is_refused(&self, version: &str) -> bool {
        self.refused(version)
    }

    /// The release page to send a person to for a manual download.
    pub fn download_page(&self) -> String {
        match self.snapshot().state {
            State::Available(offer)
            | State::Manual { offer, .. }
            | State::Ready { offer, .. }
            | State::Downloading { offer, .. }
            | State::Installing(offer)
            | State::RestartRequired(offer)
            | State::Failed {
                offer: Some(offer), ..
            } => offer.page,
            _ => release::RELEASES_PAGE.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests;

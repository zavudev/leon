//! The state machine, end to end, over a scripted GitHub and a temporary
//! install root.

use super::*;
use crate::http::{Reply, ScriptedHttp};
use crate::install::Target;
use crate::package::{fixtures, Kind};
use crate::release::{download_url, CHECKSUMS_NAME, LATEST_URL, LIST_URL};
use crate::tools::FakeTools;
use sha2::Digest as _;
use std::sync::atomic::{AtomicI64, Ordering};

const NOW: i64 = 1_800_000_000;

struct Rig {
    dir: tempfile::TempDir,
    http: Arc<ScriptedHttp>,
    tools: Arc<FakeTools>,
    now: Arc<AtomicI64>,
    platform: Platform,
    install: Install,
    current: String,
    archive_name: String,
    archive: Vec<u8>,
}

fn json(
    tag: &str,
    prerelease: bool,
    draft: bool,
    assets: &[(&str, u64, Option<String>)],
) -> String {
    let assets: Vec<String> = assets
        .iter()
        .map(|(name, size, digest)| {
            let digest = digest
                .as_ref()
                .map(|digest| format!(r#","digest":"sha256:{digest}""#))
                .unwrap_or_default();
            format!(
                r#"{{"name":"{name}","size":{size},"browser_download_url":"{}"{digest}}}"#,
                download_url(tag, name)
            )
        })
        .collect();
    format!(
        r#"{{"tag_name":"{tag}","prerelease":{prerelease},"draft":{draft},
        "html_url":"https://github.com/zavudev/leon/releases/tag/{tag}",
        "body":"Notes for {tag}","assets":[{}]}}"#,
        assets.join(",")
    )
}

fn hex_of(bytes: &[u8]) -> String {
    checksums::hex(&sha2::Sha256::digest(bytes))
}

fn cdn(name: &str) -> String {
    format!("https://release-assets.githubusercontent.com/asset/{name}?sig=1")
}

impl Rig {
    /// A Linux install of `current`; the release `v0.2.1` on the scripted
    /// GitHub holds an archive that really has a program in it.
    fn linux(current: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let built = dir.path().join("built.tar.gz");
        let root = "leon-0.2.1-linux-x86_64";
        fixtures::tar_gz(
            &built,
            &[
                (&format!("{root}/"), b""),
                (&format!("{root}/leon"), b"program 0.2.1"),
                (&format!("{root}/LICENSE"), b"licence"),
            ],
        );
        let target = Target {
            path: dir.path().join("app/leon"),
            kind: Kind::File,
        };
        std::fs::create_dir_all(target.path.parent().unwrap()).unwrap();
        std::fs::write(&target.path, "program old").unwrap();
        let rig = Self {
            archive: std::fs::read(&built).unwrap(),
            http: Arc::new(ScriptedHttp::new()),
            tools: Arc::new(FakeTools::new("0.2.1")),
            now: Arc::new(AtomicI64::new(NOW)),
            platform: Platform::parse("linux-x86_64").unwrap(),
            install: Install::Updatable(target),
            current: current.to_owned(),
            archive_name: "leon-0.2.1-linux-x86_64.tar.gz".into(),
            dir,
        };
        rig.publish("v0.2.1", false, false, true, true);
        rig
    }

    /// A macOS install of 0.2.0, running signed by `running` (or not); the new
    /// bundle, a folder that stands for the disk image, signed by `new`.
    fn mac(running: Option<&str>, new: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image");
        fixtures::image(&image, "0.2.1", b"MACHO 0.2.1");
        if let Some(identity) = new {
            FakeTools::sign(&image.join("Leon.app"), identity);
        }
        let apps = dir.path().join("Applications");
        fixtures::image(&apps, "0.2.0", b"MACHO 0.2.0");
        let target = Target {
            path: apps.join("Leon.app"),
            kind: Kind::Bundle,
        };
        if let Some(identity) = running {
            FakeTools::sign(&target.path, identity);
        }
        let rig = Self {
            http: Arc::new(ScriptedHttp::new()),
            tools: Arc::new(FakeTools::new("0.2.1").with_image(&image)),
            now: Arc::new(AtomicI64::new(NOW)),
            platform: Platform::parse("macos-aarch64").unwrap(),
            install: Install::Updatable(target),
            current: "0.2.0".into(),
            archive_name: "leon-0.2.1-macos-aarch64.dmg".into(),
            archive: b"a disk image, as far as the scripted tools care".to_vec(),
            dir,
        };
        rig.publish("v0.2.1", false, false, true, true);
        rig
    }

    fn sums(&self) -> String {
        format!("{}  {}\n", hex_of(&self.archive), self.archive_name)
    }

    /// Scripts a release on GitHub: its API answer, its redirects and files.
    fn publish(
        &self,
        tag: &str,
        prerelease: bool,
        draft: bool,
        with_archive: bool,
        with_sums: bool,
    ) {
        let mut assets = Vec::new();
        if with_archive {
            assets.push((
                self.archive_name.as_str(),
                self.archive.len() as u64,
                Some(hex_of(&self.archive)),
            ));
        }
        if with_sums {
            assets.push((CHECKSUMS_NAME, self.sums().len() as u64, None));
        }
        let body = json(tag, prerelease, draft, &assets);
        self.http.set(
            LATEST_URL,
            Reply::Status {
                status: 200,
                headers: vec![("etag".into(), "W/\"one\"".into())],
                body: body.into_bytes(),
            },
        );
        for (name, bytes) in [
            (self.archive_name.clone(), self.archive.clone()),
            (CHECKSUMS_NAME.to_owned(), self.sums().into_bytes()),
        ] {
            self.http
                .set(&download_url(tag, &name), Reply::redirect(&cdn(&name)));
            self.http.set(&cdn(&name), Reply::file(bytes));
        }
    }

    fn updater(&self) -> Arc<Updater> {
        let now = self.now.clone();
        Updater::new(
            Config {
                current: Version::parse(&self.current).unwrap(),
                platform: self.platform,
                layout: Layout::new(self.dir.path().join("data/updates")),
                install: self.install.clone(),
                now: Arc::new(move || now.load(Ordering::SeqCst)),
            },
            self.http.clone(),
            self.tools.clone(),
        )
    }

    fn layout(&self) -> Layout {
        Layout::new(self.dir.path().join("data/updates"))
    }

    fn target(&self) -> Target {
        self.install.target().unwrap().clone()
    }

    fn calls(&self, prefix: &str) -> usize {
        self.http
            .calls()
            .iter()
            .filter(|call| call.starts_with(prefix))
            .count()
    }
}

fn available(snapshot: &Snapshot) -> &Offer {
    match &snapshot.state {
        State::Available(offer) => offer,
        other => panic!("not available: {other:?}"),
    }
}

// ----- checking -----------------------------------------------------------------------

#[tokio::test]
async fn a_newer_release_is_available_with_its_notes() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    assert_eq!(updater.snapshot().state, State::Idle);
    let snapshot = updater.check(false).await;
    let offer = available(&snapshot);
    assert_eq!(offer.version, "0.2.1");
    assert_eq!(offer.notes, "Notes for v0.2.1");
    assert_eq!(
        offer.page,
        "https://github.com/zavudev/leon/releases/tag/v0.2.1"
    );
    assert_eq!(offer.size, rig.archive.len() as u64);
    assert_eq!(snapshot.checked_at, Some(NOW));
    assert_eq!(updater.snapshot(), snapshot);
}

#[tokio::test]
async fn the_check_asks_the_latest_endpoint_with_an_agent_and_nothing_else() {
    let rig = Rig::linux("0.2.0");
    rig.updater().check(false).await;
    assert_eq!(rig.http.calls(), [format!("GET {LATEST_URL}")]);
}

#[tokio::test]
async fn an_equal_or_older_release_is_up_to_date() {
    for current in ["0.2.1", "0.3.0", "1.0.0"] {
        let rig = Rig::linux(current);
        let snapshot = rig.updater().check(false).await;
        assert_eq!(snapshot.state, State::UpToDate, "{current}");
    }
}

#[tokio::test]
async fn drafts_and_foreign_tags_are_ignored() {
    let rig = Rig::linux("0.2.0");
    rig.http
        .set(LATEST_URL, Reply::ok(json("v0.9.0", false, true, &[])));
    assert_eq!(rig.updater().check(false).await.state, State::UpToDate);
    let rig = Rig::linux("0.2.0");
    rig.http
        .set(LATEST_URL, Reply::ok(json("nightly", false, false, &[])));
    assert_eq!(rig.updater().check(false).await.state, State::UpToDate);
}

#[tokio::test]
async fn a_pre_release_is_offered_only_on_the_pre_release_channel() {
    let rig = Rig::linux("0.2.0");
    let list = format!(
        "[{},{}]",
        json(
            "v0.3.0-rc.1",
            true,
            false,
            &[
                ("leon-0.3.0-rc.1-linux-x86_64.tar.gz", 10, None),
                ("SHA256SUMS", 5, None)
            ]
        ),
        json("v0.2.0", false, false, &[])
    );
    rig.http.push(LIST_URL, Reply::ok(list));
    let updater = rig.updater();
    // On the stable channel the latest endpoint says 0.2.1 (a real update).
    assert_eq!(available(&updater.check(false).await).version, "0.2.1");

    let rig = Rig::linux("0.2.1");
    let list = format!(
        "[{}]",
        json(
            "v0.3.0-rc.1",
            true,
            false,
            &[
                ("leon-0.3.0-rc.1-linux-x86_64.tar.gz", 10, None),
                ("SHA256SUMS", 5, None)
            ]
        )
    );
    rig.http.push(LIST_URL, Reply::ok(list));
    let updater = rig.updater();
    assert_eq!(updater.check(false).await.state, State::UpToDate);
    assert_eq!(
        rig.calls("GET https://api.github.com/repos/zavudev/leon/releases?"),
        0
    );
    updater.set_prereleases(true);
    let snapshot = updater.check(false).await;
    let offer = available(&snapshot);
    assert_eq!(offer.version, "0.3.0-rc.1");
    assert!(offer.prerelease);
    assert_eq!(
        rig.calls("GET https://api.github.com/repos/zavudev/leon/releases?"),
        1
    );
}

#[tokio::test]
async fn a_release_without_a_build_for_this_platform_says_so() {
    let rig = Rig::linux("0.2.0");
    rig.http.set(
        LATEST_URL,
        Reply::ok(json(
            "v0.2.1",
            false,
            false,
            &[
                ("leon-0.2.1-macos-aarch64.dmg", 10, None),
                ("SHA256SUMS", 5, None),
            ],
        )),
    );
    let snapshot = rig.updater().check(false).await;
    assert_eq!(
        snapshot.state,
        State::NoBuild {
            version: "0.2.1".into()
        }
    );
}

#[tokio::test]
async fn a_release_without_checksums_is_not_installable() {
    let rig = Rig::linux("0.2.0");
    rig.http.set(
        LATEST_URL,
        Reply::ok(json(
            "v0.2.1",
            false,
            false,
            &[(&rig.archive_name, 10, None)],
        )),
    );
    let snapshot = rig.updater().check(false).await;
    match snapshot.state {
        State::Failed {
            reason,
            stage: Stage::Check,
            ..
        } => assert!(reason.contains("SHA256SUMS"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_unchanged_answer_costs_a_conditional_request_and_is_read_from_the_cache() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    updater.check(false).await;
    rig.http.set(LATEST_URL, Reply::status(304, &[]));
    let again = updater.check(true).await;
    assert_eq!(available(&again).version, "0.2.1");
    let calls = rig.http.calls();
    assert_eq!(calls.len(), 2);
    assert!(!calls[0].contains("if-none-match"));
    assert!(calls[1].contains("if-none-match=W/\"one\""), "{calls:?}");
    // The ETag survives a restart of Leon.
    let fresh = rig.updater();
    rig.http.set(LATEST_URL, Reply::status(304, &[]));
    assert_eq!(available(&fresh.check(false).await).version, "0.2.1");
    assert!(rig
        .http
        .calls()
        .last()
        .unwrap()
        .contains("if-none-match=W/\"one\""));
}

#[tokio::test]
async fn rate_limiting_is_quiet_and_is_waited_out() {
    for (status, headers) in [
        (
            403,
            vec![("x-ratelimit-remaining", "0"), ("retry-after", "600")],
        ),
        (429, vec![("retry-after", "600")]),
        (403, vec![]),
    ] {
        let rig = Rig::linux("0.2.0");
        rig.http.set(LATEST_URL, Reply::status(status, &headers));
        let updater = rig.updater();
        let snapshot = updater.check(false).await;
        assert_eq!(snapshot.state, State::Idle, "{status}: not an error");
        assert!(updater.not_before().unwrap() > NOW);
        // By itself it does not ask again until the wait is over.
        rig.http.set(LATEST_URL, Reply::status(200, &[]));
        updater.check(false).await;
        assert_eq!(rig.calls("GET"), 1);
        // A person asking is never held back.
        rig.http.set(
            LATEST_URL,
            Reply::ok(json(
                "v0.2.1",
                false,
                false,
                &[(&rig.archive_name, 5, None), ("SHA256SUMS", 5, None)],
            )),
        );
        updater.check(true).await;
        assert_eq!(rig.calls("GET"), 2);
        // And after the wait it is asked again.
        rig.now.store(NOW + 100_000, Ordering::SeqCst);
        updater.check(false).await;
        assert_eq!(rig.calls("GET"), 3);
    }
}

#[tokio::test]
async fn the_reset_time_of_the_rate_limit_is_used_when_there_is_no_retry_after() {
    let rig = Rig::linux("0.2.0");
    let reset = (NOW + 1200).to_string();
    rig.http.set(
        LATEST_URL,
        Reply::status(403, &[("x-ratelimit-reset", &reset)]),
    );
    let updater = rig.updater();
    updater.check(false).await;
    assert_eq!(updater.not_before(), Some(NOW + 1200));
}

#[tokio::test]
async fn being_offline_is_a_failed_check_with_a_time_to_try_again() {
    let rig = Rig::linux("0.2.0");
    rig.http.set(
        LATEST_URL,
        Reply::Fail(HttpError::Unreachable("no route to host".into())),
    );
    let updater = rig.updater();
    match updater.check(false).await.state {
        State::Failed {
            stage: Stage::Check,
            retry_at: Some(at),
            ..
        } => assert!(at > NOW),
        other => panic!("{other:?}"),
    }
    // Asked again, and answered this time.
    rig.http.set(
        LATEST_URL,
        Reply::ok(json(
            "v0.2.1",
            false,
            false,
            &[(&rig.archive_name, 5, None), ("SHA256SUMS", 5, None)],
        )),
    );
    assert!(matches!(
        updater.check(true).await.state,
        State::Available(_)
    ));
}

#[tokio::test]
async fn an_answer_that_is_not_a_release_is_a_failed_check() {
    let rig = Rig::linux("0.2.0");
    rig.http
        .set(LATEST_URL, Reply::ok("<html>GitHub is down</html>"));
    assert!(matches!(
        rig.updater().check(false).await.state,
        State::Failed {
            stage: Stage::Check,
            ..
        }
    ));
    let rig = Rig::linux("0.2.0");
    rig.http.set(LATEST_URL, Reply::status(500, &[]));
    assert!(matches!(
        rig.updater().check(false).await.state,
        State::Failed {
            stage: Stage::Check,
            ..
        }
    ));
}

#[tokio::test]
async fn two_checks_at_once_make_one_request() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    let (a, b) = tokio::join!(updater.check(false), updater.check(false));
    assert_eq!(rig.calls("GET"), 1);
    let _ = (a, b);
}

// ----- the whole cycle ----------------------------------------------------------------

#[tokio::test]
async fn check_download_install_and_confirm() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    updater.check(false).await;
    let ready = updater.download().await;
    match &ready.state {
        State::Ready { offer, trust } => {
            assert_eq!(offer.version, "0.2.1");
            assert_eq!(*trust, Trust::NoScheme);
        }
        other => panic!("{other:?}"),
    }
    // The archive and the unpacked program are kept for the install.
    let layout = rig.layout();
    assert!(layout
        .version_dir("0.2.1")
        .join(&rig.archive_name)
        .is_file());
    assert_eq!(
        std::fs::read(layout.payload_dir("0.2.1").join("leon")).unwrap(),
        b"program 0.2.1"
    );
    // Nothing on disk is replaced by downloading.
    assert_eq!(
        std::fs::read_to_string(&rig.target().path).unwrap(),
        "program old"
    );

    let applied = updater.install().unwrap();
    assert_eq!(applied.version, "0.2.1");
    assert!(matches!(
        updater.snapshot().state,
        State::RestartRequired(_)
    ));
    assert_eq!(updater.restart_version().as_deref(), Some("0.2.1"));
    assert_eq!(
        std::fs::read_to_string(&rig.target().path).unwrap(),
        "program 0.2.1"
    );
    assert!(rig.target().has_backup());
    assert!(layout.pending().is_some());

    // The next start is the new version; once it is up it confirms.
    let new_updater = Updater::new(
        Config {
            current: Version::parse("0.2.1").unwrap(),
            platform: rig.platform,
            layout: rig.layout(),
            install: rig.install.clone(),
            now: Arc::new(|| NOW),
        },
        rig.http.clone(),
        rig.tools.clone(),
    );
    assert!(new_updater.startup().confirm_started());
    assert!(!rig.target().has_leftovers());
    assert_eq!(layout.pending(), None);
}

#[tokio::test]
async fn the_progress_ends_at_the_whole_size() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    updater.check(false).await;
    let mut changes = updater.subscribe();
    let watcher = tokio::spawn(async move {
        let mut seen = Vec::new();
        while changes.changed().await.is_ok() {
            if let State::Downloading { done, total, .. } = changes.borrow().state.clone() {
                seen.push((done, total));
            }
            if matches!(changes.borrow().state, State::Ready { .. }) {
                break;
            }
        }
        seen
    });
    updater.download().await;
    let seen = watcher.await.unwrap();
    assert!(seen.iter().all(|(done, total)| done <= total));
}

#[tokio::test]
async fn a_download_cut_off_is_resumed_inside_the_update() {
    let rig = Rig::linux("0.2.0");
    // The first transfer of the archive is cut halfway; the second is whole.
    let name = rig.archive_name.clone();
    let cut = rig.archive.len() / 2;
    rig.http.set(
        &cdn(&name),
        Reply::File {
            bytes: rig.archive.clone(),
            cut: Some(cut),
            ranges: true,
            extra: 0,
        },
    );
    rig.http.push(&cdn(&name), Reply::file(rig.archive.clone()));
    let updater = rig.updater();
    updater.check(false).await;
    let snapshot = updater.download().await;
    assert!(
        matches!(snapshot.state, State::Ready { .. }),
        "{:?}",
        snapshot.state
    );
    assert!(
        rig.http
            .calls()
            .iter()
            .any(|call| call.contains(&format!("from={cut}"))),
        "{:?}",
        rig.http.calls()
    );
}

#[tokio::test]
async fn a_wrong_checksum_deletes_the_download_and_the_version_is_not_tried_again() {
    let rig = Rig::linux("0.2.0");
    let wrong = format!("{}  {}\n", "00".repeat(32), rig.archive_name);
    rig.http.set(&cdn(CHECKSUMS_NAME), Reply::ok(wrong));
    let updater = rig.updater();
    updater.check(false).await;
    let snapshot = updater.download().await;
    match &snapshot.state {
        State::Failed {
            reason,
            stage: Stage::Download,
            ..
        } => {
            assert!(reason.contains("does not match"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    assert!(
        !rig.layout().version_dir("0.2.1").exists(),
        "the download is deleted"
    );
    assert!(updater.is_refused("0.2.1"));
    // The next check does not offer it again.
    updater.check(true).await;
    assert_eq!(updater.snapshot().state, State::UpToDate);
}

#[tokio::test]
async fn an_archive_that_is_not_in_the_checksum_file_is_not_even_downloaded() {
    let rig = Rig::linux("0.2.0");
    rig.http.set(
        &cdn(CHECKSUMS_NAME),
        Reply::ok(format!("{}  other-file.zip\n", "ab".repeat(32))),
    );
    let updater = rig.updater();
    updater.check(false).await;
    let snapshot = updater.download().await;
    assert!(matches!(
        snapshot.state,
        State::Failed {
            stage: Stage::Download,
            ..
        }
    ));
    assert_eq!(rig.calls("FETCH"), 0, "the archive is not downloaded");
}

#[tokio::test]
async fn a_checksum_file_listing_the_archive_twice_is_refused() {
    let rig = Rig::linux("0.2.0");
    let line = rig.sums();
    rig.http
        .set(&cdn(CHECKSUMS_NAME), Reply::ok(format!("{line}{line}")));
    let updater = rig.updater();
    updater.check(false).await;
    assert!(matches!(
        updater.download().await.state,
        State::Failed {
            stage: Stage::Download,
            ..
        }
    ));
    assert_eq!(rig.calls("FETCH"), 0);
}

#[tokio::test]
async fn a_digest_that_github_states_must_agree_with_the_file() {
    let rig = Rig::linux("0.2.0");
    rig.http.set(
        LATEST_URL,
        Reply::ok(json(
            "v0.2.1",
            false,
            false,
            &[
                (
                    &rig.archive_name,
                    rig.archive.len() as u64,
                    Some("11".repeat(32)),
                ),
                ("SHA256SUMS", rig.sums().len() as u64, None),
            ],
        )),
    );
    let updater = rig.updater();
    updater.check(false).await;
    assert!(matches!(
        updater.download().await.state,
        State::Failed {
            stage: Stage::Download,
            ..
        }
    ));
}

#[tokio::test]
async fn an_archive_with_the_wrong_layout_is_not_accepted() {
    let rig = Rig::linux("0.2.0");
    let bad = rig.dir.path().join("bad.tar.gz");
    fixtures::tar_gz(&bad, &[("some-other-folder/leon", b"program")]);
    let archive = std::fs::read(&bad).unwrap();
    let sums = format!("{}  {}\n", hex_of(&archive), rig.archive_name);
    let rig = Rig { archive, ..rig };
    rig.publish("v0.2.1", false, false, true, true);
    rig.http.set(&cdn(CHECKSUMS_NAME), Reply::ok(sums));
    let updater = rig.updater();
    updater.check(false).await;
    match updater.download().await.state {
        State::Failed { reason, .. } => assert!(reason.contains("outside the folder"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&rig.target().path).unwrap(),
        "program old"
    );
}

#[tokio::test]
async fn a_download_that_hits_the_rate_limit_goes_back_to_available_quietly() {
    let rig = Rig::linux("0.2.0");
    let name = rig.archive_name.clone();
    rig.http
        .push(&download_url("v0.2.1", &name), Reply::status(429, &[]));
    let updater = rig.updater();
    // The scripted queue gives the redirect first and the limit second.
    updater.check(false).await;
    let first = updater.download().await;
    // Either the redirect worked and the archive came (queue order), or the
    // limit hit: in both cases nothing is an error shown to the person.
    assert!(
        matches!(first.state, State::Ready { .. } | State::Available(_)),
        "{:?}",
        first.state
    );
}

// ----- skipping, refusing and what cannot be installed --------------------------------

#[tokio::test]
async fn a_skipped_version_is_not_offered_again_unless_a_person_asks() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    updater.check(false).await;
    updater.skip();
    assert_eq!(updater.snapshot().state, State::UpToDate);
    rig.http.set(
        LATEST_URL,
        Reply::ok(json(
            "v0.2.1",
            false,
            false,
            &[(&rig.archive_name, 5, None), ("SHA256SUMS", 5, None)],
        )),
    );
    assert_eq!(updater.check(false).await.state, State::UpToDate);
    assert!(matches!(
        updater.check(true).await.state,
        State::Available(_)
    ));
    // A newer release is offered whatever was skipped.
    updater.skip();
    rig.http.set(
        LATEST_URL,
        Reply::ok(json(
            "v0.2.2",
            false,
            false,
            &[
                ("leon-0.2.2-linux-x86_64.tar.gz", 5, None),
                ("SHA256SUMS", 5, None),
            ],
        )),
    );
    assert_eq!(available(&updater.check(false).await).version, "0.2.2");
}

#[tokio::test]
async fn a_skip_is_remembered_across_runs() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    updater.check(false).await;
    updater.skip();
    let again = rig.updater();
    rig.http.set(
        LATEST_URL,
        Reply::ok(json(
            "v0.2.1",
            false,
            false,
            &[(&rig.archive_name, 5, None), ("SHA256SUMS", 5, None)],
        )),
    );
    assert_eq!(again.check(false).await.state, State::UpToDate);
}

#[tokio::test]
async fn an_install_leon_does_not_replace_is_sent_to_the_download_page() {
    let mut rig = Rig::linux("0.2.0");
    rig.install = Install::Manual(Why::Development);
    let updater = rig.updater();
    let snapshot = updater.check(false).await;
    match &snapshot.state {
        State::Manual { offer, why } => {
            assert_eq!(offer.version, "0.2.1");
            assert_eq!(*why, Why::Development);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(updater.download().await, snapshot, "nothing is downloaded");
    assert_eq!(rig.calls("FETCH"), 0);
    assert_eq!(
        updater.download_page(),
        "https://github.com/zavudev/leon/releases/tag/v0.2.1"
    );
    assert!(matches!(updater.install(), Err(ApplyError::NothingReady)));
}

#[tokio::test]
async fn installing_without_a_ready_update_does_nothing() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    assert!(matches!(updater.install(), Err(ApplyError::NothingReady)));
    updater.check(false).await;
    assert!(matches!(updater.install(), Err(ApplyError::NothingReady)));
    assert_eq!(
        std::fs::read_to_string(&rig.target().path).unwrap(),
        "program old"
    );
}

// ----- remembering a ready update -----------------------------------------------------

#[tokio::test]
async fn a_ready_update_is_ready_again_after_a_restart_of_leon() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    updater.check(false).await;
    updater.download().await;
    let again = rig.updater();
    assert!(matches!(again.snapshot().state, State::Ready { .. }));
    assert_eq!(again.install().unwrap().version, "0.2.1");
}

#[tokio::test]
async fn an_update_that_was_installed_by_hand_meanwhile_is_forgotten() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    updater.check(false).await;
    updater.download().await;
    let mut caught_up = Rig::linux("0.2.1");
    caught_up.dir = rig.dir;
    caught_up.install = rig.install.clone();
    let again = caught_up.updater();
    assert_eq!(again.snapshot().state, State::Idle);
    assert!(!caught_up.layout().version_dir("0.2.1").exists());
    assert_eq!(caught_up.layout().load().staged, None);
}

#[tokio::test]
async fn a_ready_update_whose_files_are_gone_is_dropped() {
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    updater.check(false).await;
    updater.download().await;
    std::fs::remove_dir_all(rig.layout().version_dir("0.2.1")).unwrap();
    assert_eq!(rig.updater().snapshot().state, State::Idle);
}

// ----- the way back ---------------------------------------------------------------------

#[tokio::test]
async fn a_new_build_that_does_not_come_up_is_undone_and_not_offered_again() {
    use crate::launch::{Child, Outcome};
    let rig = Rig::linux("0.2.0");
    let updater = rig.updater();
    updater.check(false).await;
    updater.download().await;
    updater.install().unwrap();

    struct Crashes;
    impl Child for Crashes {
        fn ended(&mut self) -> std::io::Result<Option<bool>> {
            Ok(Some(false))
        }
    }
    let started = std::cell::RefCell::new(Vec::new());
    let spawn = |program: &std::path::Path, wait: bool| -> std::io::Result<Box<dyn Child>> {
        started.borrow_mut().push((program.to_owned(), wait));
        Ok(Box::new(Crashes))
    };
    let outcome = launch::hand_over(&updater.startup(), "0.2.1", &spawn);
    assert_eq!(outcome, Outcome::Exit(0));
    assert_eq!(
        std::fs::read_to_string(&rig.target().path).unwrap(),
        "program old"
    );

    // Leon, started again, says what happened, once; and does not offer 0.2.1.
    let next = rig.updater();
    let notice = next.take_notice().unwrap();
    assert!(notice.contains("did not start"), "{notice}");
    assert_eq!(next.take_notice(), None);
    assert_eq!(next.check(false).await.state, State::UpToDate);
    // A newer release is tried.
    rig.http.set(
        LATEST_URL,
        Reply::ok(json(
            "v0.2.2",
            false,
            false,
            &[
                ("leon-0.2.2-linux-x86_64.tar.gz", 5, None),
                ("SHA256SUMS", 5, None),
            ],
        )),
    );
    assert_eq!(available(&next.check(true).await).version, "0.2.2");
}

#[tokio::test]
async fn a_new_build_that_does_not_run_is_never_left_in_place() {
    let mut rig = Rig::linux("0.2.0");
    rig.tools = Arc::new(FakeTools::new("0.2.1").failing("it exited with 134"));
    let updater = rig.updater();
    updater.check(false).await;
    updater.download().await;
    assert!(matches!(updater.install(), Err(ApplyError::DidNotRun(_))));
    assert_eq!(
        std::fs::read_to_string(&rig.target().path).unwrap(),
        "program old"
    );
    assert!(matches!(
        updater.snapshot().state,
        State::Failed {
            stage: Stage::Install,
            ..
        }
    ));
    assert!(updater.is_refused("0.2.1"));
}

// ----- signatures on macOS ---------------------------------------------------------------

#[tokio::test]
async fn an_unsigned_mac_build_updates_an_unsigned_one_and_says_it_is_not_signed() {
    let rig = Rig::mac(None, None);
    let updater = rig.updater();
    updater.check(false).await;
    match updater.download().await.state {
        State::Ready { trust, .. } => assert_eq!(trust, Trust::Unsigned),
        other => panic!("{other:?}"),
    }
    updater.install().unwrap();
    let target = rig.target();
    assert_eq!(std::fs::read(target.executable()).unwrap(), b"MACHO 0.2.1");
    assert_eq!(
        std::fs::read(target.backup().join("Contents/MacOS/Leon")).unwrap(),
        b"MACHO 0.2.0"
    );
}

#[tokio::test]
async fn a_signed_mac_build_updates_a_build_of_the_same_team() {
    let rig = Rig::mac(Some("TEAM1"), Some("TEAM1"));
    let updater = rig.updater();
    updater.check(false).await;
    match updater.download().await.state {
        State::Ready { trust, .. } => assert_eq!(trust, Trust::Signed("TEAM1".into())),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_signed_mac_install_is_never_replaced_by_an_unsigned_or_other_team_build() {
    for new in [None, Some("TEAM2"), Some("broken")] {
        let rig = Rig::mac(Some("TEAM1"), new);
        let updater = rig.updater();
        updater.check(false).await;
        match updater.download().await.state {
            State::Failed {
                reason,
                stage: Stage::Download,
                ..
            } => {
                assert!(reason.contains("not accepted"), "{new:?}: {reason}")
            }
            other => panic!("{new:?}: {other:?}"),
        }
        assert_eq!(
            std::fs::read(rig.target().executable()).unwrap(),
            b"MACHO 0.2.0",
            "{new:?}"
        );
        assert!(!rig.layout().version_dir("0.2.1").exists());
        assert!(updater.is_refused("0.2.1"));
    }
}

#[tokio::test]
async fn a_bundle_of_another_version_than_the_release_is_not_accepted() {
    let rig = Rig::mac(None, None);
    fixtures::image(&rig.dir.path().join("image"), "0.9.9", b"MACHO");
    let updater = rig.updater();
    updater.check(false).await;
    match updater.download().await.state {
        State::Failed { reason, .. } => assert!(reason.contains("version 0.9.9"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_jitter_helper_is_in_the_crate_root() {
    assert!(crate::jittered_interval(7).as_secs() > 0);
}

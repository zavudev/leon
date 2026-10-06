//! A scripted GitHub and a temporary install, for the tests of the
//! application's window (`features = ["test-support"]`). The platform is
//! always Linux's, so that what is tested is the same on every computer.

use crate::checksums;
use crate::http::{Reply, ScriptedHttp};
use crate::install::{Install, Target};
use crate::package::Kind;
use crate::release::{download_url, Platform, CHECKSUMS_NAME, LATEST_URL};
use crate::state::{Layout, Offer, Saved, Staged};
use crate::tools::FakeTools;
use crate::updater::{Config, Updater};
use crate::version::Version;
use std::sync::Arc;

/// An install of a program in a temporary folder, a GitHub that answers from a
/// script, and tools that touch nothing real.
pub struct Fixture {
    /// The folder everything is in.
    pub dir: tempfile::TempDir,
    /// The scripted GitHub.
    pub http: Arc<ScriptedHttp>,
    /// The fake tools.
    pub tools: Arc<FakeTools>,
    /// The installed program: `<dir>/app/leon`, whose text is `program old`.
    pub target: Target,
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixture {
    /// A fresh fixture.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let target = Target {
            path: dir.path().join("app/leon"),
            kind: Kind::File,
        };
        std::fs::create_dir_all(target.path.parent().expect("a parent")).expect("the app folder");
        std::fs::write(&target.path, "program old").expect("the program");
        Self {
            dir,
            http: Arc::new(ScriptedHttp::new()),
            tools: Arc::new(FakeTools::new("0.2.1")),
            target,
        }
    }

    /// The folder of the updater's files.
    pub fn layout(&self) -> Layout {
        Layout::new(self.dir.path().join("data/updates"))
    }

    /// An updater that thinks it is `current`, over an install that Leon
    /// replaces itself, or one it does not (`install`).
    pub fn updater_with(&self, current: &str, install: Install) -> Arc<Updater> {
        Updater::new(
            Config {
                current: Version::parse(current).expect("a version"),
                platform: Platform::parse("linux-x86_64").expect("a platform"),
                layout: self.layout(),
                install,
                now: Arc::new(|| 1_800_000_000),
            },
            self.http.clone(),
            self.tools.clone(),
        )
    }

    /// An updater over the install in the fixture.
    pub fn updater(&self, current: &str) -> Arc<Updater> {
        self.updater_with(current, Install::Updatable(self.target.clone()))
    }

    /// Makes GitHub's latest release `v<version>`, with a Linux archive and
    /// a checksum file listed (their bytes are not served).
    pub fn announce(&self, version: &str, notes: &str) {
        let tag = format!("v{version}");
        let name = format!("leon-{version}-linux-x86_64.tar.gz");
        let body = serde_json::json!({
            "tag_name": tag,
            "prerelease": false,
            "draft": false,
            "html_url": format!("https://github.com/zavudev/leon/releases/tag/{tag}"),
            "body": notes,
            "assets": [
                {"name": name, "size": 1000, "browser_download_url": download_url(&tag, &name)},
                {"name": CHECKSUMS_NAME, "size": 100, "browser_download_url": download_url(&tag, CHECKSUMS_NAME)},
            ],
        });
        self.http.set(LATEST_URL, Reply::ok(body.to_string()));
    }

    /// Leaves a downloaded, verified and unpacked update of `version` where
    /// the updater finds it: an updater made afterwards starts `Ready`.
    pub fn stage(&self, version: &str, notes: &str) {
        // The program that is installed says its version when it is tried.
        *self.tools.probe_answer.lock().expect("the probe") = Ok(format!("Leon {version}"));
        let layout = self.layout();
        let archive = layout.version_dir(version).join("a.tar.gz");
        std::fs::create_dir_all(archive.parent().expect("a parent")).expect("the version folder");
        std::fs::write(&archive, b"archive bytes").expect("the archive");
        let payload = layout.payload_dir(version);
        std::fs::create_dir_all(&payload).expect("the payload folder");
        std::fs::write(payload.join("leon"), format!("program {version}")).expect("the payload");
        let (hash, _) = checksums::hash_file(&archive).expect("the hash");
        layout
            .save(&Saved {
                staged: Some(Staged {
                    offer: Offer {
                        version: version.to_owned(),
                        notes: notes.to_owned(),
                        page: format!("https://github.com/zavudev/leon/releases/tag/v{version}"),
                        size: 13,
                        prerelease: false,
                    },
                    archive: "a.tar.gz".into(),
                    sha256: checksums::hex(&hash),
                    identity: None,
                }),
                ..Saved::default()
            })
            .expect("the state");
    }
}

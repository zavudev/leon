//! The real check, as lines of text: what `leon --diagnose update` prints.
//!
//! It asks GitHub the way the updater does (with its own ETag-free, plain
//! call), reads the answer with the same code, and says what the updater would
//! do on this platform: which file it would take, whether `SHA256SUMS` lists
//! it, and the decision. With a directory it also downloads, verifies and
//! unpacks the file there, and holds the signature to the running build;
//! it never installs anything.

use crate::checksums;
use crate::download;
use crate::http::{Get, HostPolicy, Http};
use crate::package;
use crate::release::{self, Platform, Selection};
use crate::tools::{Signature, Tools};
use crate::trust;
use crate::version::{self, Version};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What to find out.
pub struct Request {
    /// The version to compare the release with (the running one, or a
    /// pretended one).
    pub current: Version,
    /// Whether pre-releases are followed.
    pub prereleases: bool,
    /// The platform to pick the file for.
    pub platform: Platform,
    /// Where to download to; no download when `None`.
    pub download_to: Option<PathBuf>,
    /// The signature of the running build.
    pub running: Signature,
}

/// What was found.
pub struct Report {
    /// The lines to print.
    pub lines: Vec<String>,
    /// Whether everything that was tried went well.
    pub ok: bool,
}

struct Out {
    lines: Vec<String>,
    ok: bool,
}

impl Out {
    fn say(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn fail(mut self, line: impl Into<String>) -> Report {
        self.say(line);
        Report {
            lines: self.lines,
            ok: false,
        }
    }
}

/// Asks, reads, decides and (when asked) downloads and verifies.
pub async fn run(http: &dyn Http, tools: &dyn Tools, request: &Request) -> Report {
    let mut out = Out {
        lines: Vec::new(),
        ok: true,
    };
    let policy = HostPolicy::Github;
    let url = if request.prereleases {
        release::LIST_URL
    } else {
        release::LATEST_URL
    };
    out.say(format!("asking {url}"));
    let response = match http
        .request(
            Get::new(url)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28"),
        )
        .await
    {
        Ok(response) => response,
        Err(error) => return out.fail(format!("cannot ask GitHub: {error}")),
    };
    out.say(format!(
        "GitHub answered {} (rate limit left: {})",
        response.status,
        response
            .header("x-ratelimit-remaining")
            .unwrap_or("unknown")
    ));
    if response.status != 200 {
        return out.fail("that is not an answer with a release: nothing to decide");
    }
    let body = response.text();
    let parsed = if request.prereleases {
        release::parse_list(&body, true)
    } else {
        release::parse_latest(&body)
    };
    let release = match parsed {
        Ok(Some(release)) => release,
        Ok(None) => {
            return out.fail("no release that counts (a draft, or a tag that is not v<semver>)")
        }
        Err(error) => return out.fail(error.to_string()),
    };
    out.say(format!(
        "latest release: {} ({}) {}",
        release.tag,
        if release.prerelease {
            "pre-release"
        } else {
            "stable"
        },
        release.page
    ));
    let platform = request.platform;
    out.say(format!("this platform: {}", platform.id()));
    let (archive, sums_asset) = match release::select(&release, &platform) {
        Selection::Found { archive, checksums } => (archive.clone(), checksums.clone()),
        Selection::NoBuild => {
            return out.fail(format!(
                "no build for this platform in {}: the updater would say so and stop",
                release.tag
            ))
        }
        Selection::NoChecksums => {
            return out.fail("the release has no SHA256SUMS: the updater would refuse it")
        }
    };
    out.say(format!(
        "asset it would pick: {} ({} bytes{})",
        archive.name,
        archive.size,
        archive
            .digest
            .map(|digest| format!(", GitHub's digest sha256:{}", checksums::hex(&digest)))
            .unwrap_or_default()
    ));
    let sums = match download::get_small(http, &policy, &sums_asset.url).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => return out.fail(format!("cannot read SHA256SUMS: {error}")),
    };
    let listed = match checksums::expected(&sums, &archive.name) {
        Ok(hash) => hash,
        Err(error) => return out.fail(format!("SHA256SUMS: {error}")),
    };
    out.say(format!(
        "SHA256SUMS: present, one entry for the asset: {}{}",
        checksums::hex(&listed),
        match archive.digest {
            Some(digest) if digest == listed => " (agrees with GitHub's digest)",
            Some(_) => " (DISAGREES with GitHub's digest)",
            None => "",
        }
    ));
    if archive.digest.is_some_and(|digest| digest != listed) {
        out.ok = false;
    }

    let newer = version::is_update(&request.current, &release.version, request.prereleases);
    if newer {
        out.say(format!(
            "decision: {} is newer than {}: an update is available",
            release.version, request.current
        ));
    } else if request.current > release.version {
        out.say(format!(
            "decision: {} is newer than {}: nothing to do",
            request.current, release.tag
        ));
    } else if request.current == release.version {
        out.say(format!(
            "decision: {} is the latest ({}): nothing to do",
            request.current, release.tag
        ));
    } else {
        out.say(format!(
            "decision: {} is a pre-release and the pre-release channel is off: nothing to do",
            release.tag
        ));
    }
    let Some(dir) = &request.download_to else {
        return Report {
            lines: out.lines,
            ok: out.ok,
        };
    };
    if !newer {
        out.say("not downloading: there is no update (use --pretend-version to try the path)");
        return Report {
            lines: out.lines,
            ok: out.ok,
        };
    }

    // The download, held to the checksum, unpacked, and the signature compared.
    let target = dir.join(release.version.to_string());
    out.say(format!("downloading into {}", target.display()));
    let progress: crate::http::Progress = Arc::new(|_| {});
    let path = match download::fetch_asset(
        http,
        &policy,
        &archive.url,
        &target,
        &archive.name,
        archive.size,
        progress,
    )
    .await
    {
        Ok(path) => path,
        Err(error) => return out.fail(format!("download failed: {error}")),
    };
    match checksums::verify_file(&path, &archive.name, &sums, archive.size, archive.digest) {
        Ok(()) => out.say(format!(
            "verified: the file's SHA-256 is the one in SHA256SUMS{} and its size is {} bytes",
            if archive.digest.is_some() {
                " and GitHub's digest"
            } else {
                ""
            },
            archive.size
        )),
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            return out.fail(format!("NOT verified, and deleted: {error}"));
        }
    }
    let payload_dir = target.join("payload");
    let payload = match package::extract(&path, &platform, &release.version, &payload_dir, tools) {
        Ok(payload) => payload,
        Err(error) => return out.fail(format!("unpacking failed: {error}")),
    };
    out.say(format!("unpacked: {}", payload.display()));
    match tools.signature(&payload) {
        Ok(signature) => {
            out.say(format!(
                "signature of the new build: {}",
                describe(&signature)
            ));
            out.say(format!(
                "signature of the running build: {}",
                describe(&request.running)
            ));
            match trust::judge(platform.os, &request.running, &signature) {
                Ok(trust::Trust::Signed(identity)) => {
                    out.say(format!("trust: signed by {identity}, accepted"))
                }
                Ok(trust::Trust::Unsigned) => out.say(
                    "trust: not signed, and neither is the running build: accepted, and the window says it is not signed",
                ),
                Ok(trust::Trust::NoScheme) => {
                    out.say("trust: this system has no signature to check; the checksum is what there is")
                }
                Err(why) => return out.fail(format!("trust: REFUSED: {why}")),
            }
        }
        Err(error) => return out.fail(format!("cannot read the signature: {error}")),
    }
    out.say("nothing was installed");
    Report {
        lines: out.lines,
        ok: out.ok,
    }
}

fn describe(signature: &Signature) -> String {
    match (&signature.identity, signature.valid) {
        (Some(identity), true) => format!("signed by {identity}, whole"),
        (Some(identity), false) => format!("claims {identity}, signature not whole"),
        (None, _) => "not signed by anybody".to_owned(),
    }
}

/// The platform and version a release archive's name says:
/// `leon-0.2.1-macos-aarch64.dmg` is 0.2.1 for macos-aarch64.
pub fn parse_archive_name(name: &str) -> Option<(Version, Platform)> {
    let stem = name.strip_prefix("leon-")?;
    let stem = stem
        .strip_suffix(".tar.gz")
        .or_else(|| stem.strip_suffix(".dmg"))
        .or_else(|| stem.strip_suffix(".zip"))?;
    for id in [
        "linux-x86_64",
        "macos-aarch64",
        "macos-x86_64",
        "windows-x86_64",
    ] {
        if let Some(version) = stem.strip_suffix(&format!("-{id}")) {
            let platform = Platform::parse(id)?;
            if !name.ends_with(&format!(".{}", platform.extension())) {
                return None;
            }
            return Some((Version::parse(version).ok()?, platform));
        }
    }
    None
}

/// Takes the program out of a built release archive with the updater's own
/// extraction code, into a temporary folder that is removed again. What the
/// release pipeline runs on every archive it builds.
pub fn check_archive(path: &Path, tools: &dyn Tools) -> Result<Vec<String>, String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("the archive has no name")?;
    let (version, platform) = parse_archive_name(name)
        .ok_or_else(|| format!("{name} is not named leon-<version>-<platform>.<ext>"))?;
    let scratch = std::env::temp_dir().join(format!("leon-check-archive-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let result = package::extract(path, &platform, &version, &scratch, tools);
    let outcome = match result {
        Ok(payload) => {
            let mut lines = vec![
                format!("{name}: version {version} for {}", platform.id()),
                format!(
                    "the updater's own extraction found {} in it",
                    package::payload_name(&platform)
                ),
            ];
            match tools.signature(&payload) {
                Ok(signature) => lines.push(format!("signature: {}", describe(&signature))),
                Err(error) => lines.push(format!("signature: cannot be read ({error})")),
            }
            Ok(lines)
        }
        Err(error) => Err(format!("{name}: {error}")),
    };
    let _ = std::fs::remove_dir_all(&scratch);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{Reply, ScriptedHttp};
    use crate::package::fixtures;
    use crate::release::{download_url, CHECKSUMS_NAME, LATEST_URL};
    use crate::tools::FakeTools;
    use sha2::Digest as _;

    fn sha(bytes: &[u8]) -> String {
        checksums::hex(&sha2::Sha256::digest(bytes))
    }

    /// A scripted GitHub with a real linux archive of 0.1.0.
    fn github(dir: &Path) -> (ScriptedHttp, Vec<u8>) {
        let built = dir.join("built.tar.gz");
        fixtures::tar_gz(
            &built,
            &[("leon-0.1.0-linux-x86_64/leon", b"program 0.1.0")],
        );
        let archive = std::fs::read(&built).unwrap();
        let name = "leon-0.1.0-linux-x86_64.tar.gz";
        let sums = format!("{}  {name}\n", sha(&archive));
        let body = format!(
            r#"{{"tag_name":"v0.1.0","html_url":"https://github.com/zavudev/leon/releases/tag/v0.1.0","assets":[
            {{"name":"{name}","size":{},"browser_download_url":"{}","digest":"sha256:{}"}},
            {{"name":"SHA256SUMS","size":{},"browser_download_url":"{}"}}]}}"#,
            archive.len(),
            download_url("v0.1.0", name),
            sha(&archive),
            sums.len(),
            download_url("v0.1.0", CHECKSUMS_NAME)
        );
        let cdn = |n: &str| format!("https://release-assets.githubusercontent.com/{n}");
        let http = ScriptedHttp::new()
            .route(LATEST_URL, Reply::ok(body))
            .route(&download_url("v0.1.0", name), Reply::redirect(&cdn(name)))
            .route(&cdn(name), Reply::file(archive.clone()))
            .route(
                &download_url("v0.1.0", CHECKSUMS_NAME),
                Reply::redirect(&cdn("sums")),
            )
            .route(&cdn("sums"), Reply::ok(sums));
        (http, archive)
    }

    fn request(current: &str, download_to: Option<PathBuf>) -> Request {
        Request {
            current: Version::parse(current).unwrap(),
            prereleases: false,
            platform: Platform::parse("linux-x86_64").unwrap(),
            download_to,
            running: Signature::none(),
        }
    }

    #[tokio::test]
    async fn a_newer_running_version_has_nothing_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let (http, _) = github(dir.path());
        let report = run(&http, &FakeTools::new("0.1.0"), &request("0.2.0", None)).await;
        assert!(report.ok);
        let text = report.lines.join("\n");
        assert!(text.contains("latest release: v0.1.0 (stable)"), "{text}");
        assert!(text.contains("leon-0.1.0-linux-x86_64.tar.gz"), "{text}");
        assert!(text.contains("agrees with GitHub's digest"), "{text}");
        assert!(
            text.contains("0.2.0 is newer than v0.1.0: nothing to do"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_pretended_older_version_takes_the_update_path_and_verifies_the_download() {
        let dir = tempfile::tempdir().unwrap();
        let (http, _) = github(dir.path());
        let out = dir.path().join("scratch");
        let report = run(
            &http,
            &FakeTools::new("0.1.0"),
            &request("0.0.9", Some(out.clone())),
        )
        .await;
        let text = report.lines.join("\n");
        assert!(report.ok, "{text}");
        assert!(text.contains("an update is available"), "{text}");
        assert!(text.contains("verified:"), "{text}");
        assert!(text.contains("nothing was installed"), "{text}");
        assert_eq!(
            std::fs::read(out.join("0.1.0/payload/leon")).unwrap(),
            b"program 0.1.0"
        );
    }

    #[tokio::test]
    async fn a_download_is_not_made_when_there_is_no_update() {
        let dir = tempfile::tempdir().unwrap();
        let (http, _) = github(dir.path());
        let out = dir.path().join("scratch");
        let report = run(
            &http,
            &FakeTools::new("0.1.0"),
            &request("0.2.0", Some(out.clone())),
        )
        .await;
        assert!(report.ok);
        assert!(!out.exists());
        assert!(http.calls().iter().all(|call| !call.starts_with("FETCH")));
    }

    #[tokio::test]
    async fn a_file_that_does_not_match_the_checksum_is_reported_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let (http, archive) = github(dir.path());
        let mut bad = archive.clone();
        bad[20] ^= 0xff;
        http.set(
            "https://release-assets.githubusercontent.com/leon-0.1.0-linux-x86_64.tar.gz",
            Reply::file(bad),
        );
        let out = dir.path().join("scratch");
        let report = run(
            &http,
            &FakeTools::new("0.1.0"),
            &request("0.0.9", Some(out.clone())),
        )
        .await;
        let text = report.lines.join("\n");
        assert!(!report.ok);
        assert!(text.contains("NOT verified, and deleted"), "{text}");
        assert!(!out.join("0.1.0/leon-0.1.0-linux-x86_64.tar.gz").exists());
    }

    #[tokio::test]
    async fn a_missing_build_and_an_unreachable_github_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (http, _) = github(dir.path());
        let mut mac = request("0.0.9", None);
        mac.platform = Platform::parse("macos-aarch64").unwrap();
        let report = run(&http, &FakeTools::new("0.1.0"), &mac).await;
        assert!(!report.ok);
        assert!(report
            .lines
            .last()
            .unwrap()
            .contains("no build for this platform"));
        let nothing = ScriptedHttp::new();
        let report = run(&nothing, &FakeTools::new("0.1.0"), &request("0.0.9", None)).await;
        assert!(!report.ok);
    }

    #[test]
    fn archive_names_say_their_version_and_platform() {
        let (version, platform) = parse_archive_name("leon-0.2.1-macos-aarch64.dmg").unwrap();
        assert_eq!(
            (version.to_string(), platform.id()),
            ("0.2.1".into(), "macos-aarch64".into())
        );
        let (version, _) = parse_archive_name("leon-1.0.0-rc.1-windows-x86_64.zip").unwrap();
        assert_eq!(version.to_string(), "1.0.0-rc.1");
        for bad in [
            "leon-0.2.1-macos-aarch64.zip",
            "leon-0.2.1-freebsd-x86_64.tar.gz",
            "leon-x-linux-x86_64.tar.gz",
            "SHA256SUMS",
            "other-0.2.1-linux-x86_64.tar.gz",
        ] {
            assert!(parse_archive_name(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_built_archive_is_checked_with_the_updaters_own_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("leon-0.2.1-linux-x86_64.tar.gz");
        fixtures::tar_gz(
            &good,
            &[
                ("leon-0.2.1-linux-x86_64/leon", b"program"),
                ("leon-0.2.1-linux-x86_64/NOTICE", b"n"),
            ],
        );
        let lines = check_archive(&good, &FakeTools::new("0.2.1")).unwrap();
        assert!(lines[0].contains("version 0.2.1 for linux-x86_64"));
        assert!(lines.join("\n").contains("found leon in it"));

        let wrong = dir.path().join("leon-0.2.2-linux-x86_64.tar.gz");
        std::fs::copy(&good, &wrong).unwrap();
        assert!(
            check_archive(&wrong, &FakeTools::new("0.2.1")).is_err(),
            "the folder is another version's"
        );

        let zip = dir.path().join("leon-0.2.1-windows-x86_64.zip");
        fixtures::zip(&zip, &[("leon-0.2.1-windows-x86_64/leon.exe", b"MZ")]);
        assert!(check_archive(&zip, &FakeTools::new("0.2.1")).is_ok());
        let hollow = dir.path().join("leon-0.2.3-windows-x86_64.zip");
        fixtures::zip(&hollow, &[("leon-0.2.3-windows-x86_64/LICENSE", b"x")]);
        assert!(check_archive(&hollow, &FakeTools::new("0.2.3")).is_err());
        assert!(check_archive(&dir.path().join("nothing.txt"), &FakeTools::new("0.2.1")).is_err());
    }
}

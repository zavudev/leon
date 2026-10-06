//! What GitHub says about a release, and which of its files is ours.
//!
//! The answer of `GET /repos/zavudev/leon/releases/latest` (or of the list,
//! for the pre-release channel) is parsed into a [`Release`]. Nothing in it is
//! trusted beyond what is checked here: a tag that is not `v<semver>`, a draft,
//! an asset whose address is not the release's own download address for that
//! name, all drop out. The file to download is chosen by name from the
//! platform ([`Platform`]); its address is built here from the tag and the
//! name, and the one in the answer has to be the same.

use crate::version::{self, Version};
use serde::Deserialize;

/// The repository releases are read from.
pub const REPOSITORY: &str = "zavudev/leon";

/// The page of the releases, for a person to download from.
pub const RELEASES_PAGE: &str = "https://github.com/zavudev/leon/releases";

/// The checksum file every release carries.
pub const CHECKSUMS_NAME: &str = "SHA256SUMS";

/// The API address of the newest stable release.
pub const LATEST_URL: &str = "https://api.github.com/repos/zavudev/leon/releases/latest";

/// The API address of the most recent releases, pre-releases included.
pub const LIST_URL: &str = "https://api.github.com/repos/zavudev/leon/releases?per_page=15";

/// The operating systems a release is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    /// macOS.
    Macos,
    /// Linux.
    Linux,
    /// Windows.
    Windows,
}

/// The processors a release is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    /// 64-bit ARM (Apple silicon).
    Aarch64,
    /// 64-bit x86.
    X86_64,
    /// Anything else: there is no build.
    Other,
}

/// A system and a processor: the name of a build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Platform {
    /// The system.
    pub os: Os,
    /// The processor.
    pub arch: Arch,
}

impl Platform {
    /// This build's own platform.
    pub fn current() -> Self {
        let os = if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else {
            Os::Linux
        };
        let arch = match std::env::consts::ARCH {
            "aarch64" => Arch::Aarch64,
            "x86_64" => Arch::X86_64,
            _ => Arch::Other,
        };
        Self { os, arch }
    }

    /// The platform a release's name for it (`macos-aarch64`) says.
    pub fn parse(id: &str) -> Option<Self> {
        let (os, arch) = id.split_once('-')?;
        let os = match os {
            "macos" => Os::Macos,
            "linux" => Os::Linux,
            "windows" => Os::Windows,
            _ => return None,
        };
        let arch = match arch {
            "aarch64" => Arch::Aarch64,
            "x86_64" => Arch::X86_64,
            _ => return None,
        };
        Some(Self { os, arch })
    }

    /// The platform's part of a file name: `macos-aarch64`, `windows-x86_64`.
    pub fn id(&self) -> String {
        let os = match self.os {
            Os::Macos => "macos",
            Os::Linux => "linux",
            Os::Windows => "windows",
        };
        let arch = match self.arch {
            Arch::Aarch64 => "aarch64",
            Arch::X86_64 => "x86_64",
            Arch::Other => "other",
        };
        format!("{os}-{arch}")
    }

    /// The extension of the platform's archive.
    pub fn extension(&self) -> &'static str {
        match self.os {
            Os::Macos => "dmg",
            Os::Linux => "tar.gz",
            Os::Windows => "zip",
        }
    }

    /// `leon-<version>-<platform>.<ext>`: the file of a version.
    pub fn asset_name(&self, version: &Version) -> String {
        format!("leon-{version}-{}.{}", self.id(), self.extension())
    }

    /// Whether releases are built for it.
    pub fn is_built(&self) -> bool {
        matches!(
            self.id().as_str(),
            "macos-aarch64" | "macos-x86_64" | "linux-x86_64" | "windows-x86_64"
        )
    }
}

/// One file of a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    /// Its name.
    pub name: String,
    /// Its size in bytes, as GitHub states it.
    pub size: u64,
    /// Where it is downloaded from: built here, and equal to the answer's.
    pub url: String,
    /// The SHA-256 GitHub states for it (`digest`), when it does.
    pub digest: Option<[u8; 32]>,
}

/// A release that is a candidate for an update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Its version.
    pub version: Version,
    /// Its tag.
    pub tag: String,
    /// Whether it is marked as a pre-release.
    pub prerelease: bool,
    /// Its page.
    pub page: String,
    /// The notes as written.
    pub notes: String,
    /// Its files.
    pub assets: Vec<Asset>,
}

/// What is wrong with an answer.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    /// Not the JSON of a release.
    #[error("the answer is not a release: {0}")]
    Json(String),
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

/// The address a release's file is downloaded from.
pub fn download_url(tag: &str, name: &str) -> String {
    format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}")
}

fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        && !name.starts_with('.')
}

/// `sha256:<64 hex>` as GitHub writes a digest.
pub fn parse_digest(text: &str) -> Option<[u8; 32]> {
    crate::checksums::decode_hex(text.strip_prefix("sha256:")?)
}

fn convert(raw: RawRelease) -> Option<Release> {
    if raw.draft {
        return None;
    }
    let version = version::parse_tag(&raw.tag_name)?;
    // A pre-release version and the flag have to agree on one thing: a flagged
    // release is never taken for stable, whatever its tag says.
    let prerelease = raw.prerelease || !version.pre.is_empty();
    let page = if raw
        .html_url
        .starts_with(&format!("https://github.com/{REPOSITORY}/"))
    {
        raw.html_url
    } else {
        RELEASES_PAGE.to_owned()
    };
    let assets = raw
        .assets
        .into_iter()
        .filter(|asset| plain_name(&asset.name))
        .filter(|asset| asset.browser_download_url == download_url(&raw.tag_name, &asset.name))
        .map(|asset| Asset {
            url: download_url(&raw.tag_name, &asset.name),
            digest: asset.digest.as_deref().and_then(parse_digest),
            name: asset.name,
            size: asset.size,
        })
        .collect();
    Some(Release {
        version,
        tag: raw.tag_name,
        prerelease,
        page,
        notes: raw.body.unwrap_or_default(),
        assets,
    })
}

/// The release in the answer of `releases/latest`; `None` for a draft or a
/// tag that is not a version.
pub fn parse_latest(body: &str) -> Result<Option<Release>, ParseError> {
    let raw: RawRelease =
        serde_json::from_str(body).map_err(|error| ParseError::Json(error.to_string()))?;
    Ok(convert(raw))
}

/// The newest release of a list, pre-releases only when `prereleases`.
pub fn parse_list(body: &str, prereleases: bool) -> Result<Option<Release>, ParseError> {
    let raws: Vec<RawRelease> =
        serde_json::from_str(body).map_err(|error| ParseError::Json(error.to_string()))?;
    Ok(raws
        .into_iter()
        .filter_map(convert)
        .filter(|release| prereleases || !release.prerelease)
        .max_by(|a, b| a.version.cmp(&b.version)))
}

/// Which files of a release a platform needs.
#[derive(Debug, PartialEq, Eq)]
pub enum Selection<'a> {
    /// The archive and the checksum file.
    Found {
        /// The platform's archive.
        archive: &'a Asset,
        /// `SHA256SUMS`.
        checksums: &'a Asset,
    },
    /// The release has no build for this platform.
    NoBuild,
    /// The release has the archive but no checksum file: it cannot be
    /// verified, so it is not installed.
    NoChecksums,
}

/// Picks the archive of `platform` and the checksum file.
pub fn select<'a>(release: &'a Release, platform: &Platform) -> Selection<'a> {
    if !platform.is_built() {
        return Selection::NoBuild;
    }
    let wanted = platform.asset_name(&release.version);
    let Some(archive) = release.assets.iter().find(|asset| asset.name == wanted) else {
        return Selection::NoBuild;
    };
    match release
        .assets
        .iter()
        .find(|asset| asset.name == CHECKSUMS_NAME)
    {
        Some(checksums) => Selection::Found { archive, checksums },
        None => Selection::NoChecksums,
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::json;
    use super::*;

    const ALL: [&str; 4] = [
        "linux-x86_64",
        "macos-aarch64",
        "macos-x86_64",
        "windows-x86_64",
    ];

    #[test]
    fn the_latest_release_is_read() {
        let release = parse_latest(&json("v0.2.1", false, false, &ALL))
            .unwrap()
            .unwrap();
        assert_eq!(release.version, Version::new(0, 2, 1));
        assert_eq!(release.tag, "v0.2.1");
        assert!(!release.prerelease);
        assert_eq!(
            release.page,
            "https://github.com/zavudev/leon/releases/tag/v0.2.1"
        );
        assert!(release.notes.contains("- two"));
        assert_eq!(release.assets.len(), 5);
        assert_eq!(release.assets[0].digest, Some([0xab; 32]));
        assert_eq!(release.assets[4].digest, None);
    }

    #[test]
    fn a_draft_and_a_foreign_tag_are_not_releases() {
        assert_eq!(parse_latest(&json("v0.2.1", false, true, &ALL)), Ok(None));
        assert_eq!(parse_latest(&json("nightly", false, false, &ALL)), Ok(None));
        assert_eq!(parse_latest(&json("0.2.1", false, false, &ALL)), Ok(None));
    }

    #[test]
    fn a_flagged_or_suffixed_release_is_a_pre_release() {
        let flagged = parse_latest(&json("v0.3.0", true, false, &ALL))
            .unwrap()
            .unwrap();
        assert!(flagged.prerelease);
        let suffixed = parse_latest(&json("v0.3.0-rc.1", false, false, &ALL))
            .unwrap()
            .unwrap();
        assert!(suffixed.prerelease);
    }

    #[test]
    fn bad_json_is_an_error_and_missing_fields_are_tolerated() {
        assert!(parse_latest("<html>").is_err());
        assert!(parse_latest("{}").is_err());
        let release = parse_latest(r#"{"tag_name":"v1.0.0"}"#).unwrap().unwrap();
        assert!(release.assets.is_empty());
        assert_eq!(release.notes, "");
        assert_eq!(release.page, RELEASES_PAGE);
    }

    #[test]
    fn an_asset_with_a_foreign_address_or_a_strange_name_is_dropped() {
        let body = r#"{"tag_name":"v0.2.1","assets":[
            {"name":"leon-0.2.1-linux-x86_64.tar.gz","size":1,
             "browser_download_url":"https://evil.example/leon-0.2.1-linux-x86_64.tar.gz"},
            {"name":"../x","size":1,"browser_download_url":"https://github.com/zavudev/leon/releases/download/v0.2.1/../x"},
            {"name":"leon-0.2.1-macos-aarch64.dmg","size":1,
             "browser_download_url":"https://github.com/zavudev/leon/releases/download/v0.2.1/leon-0.2.1-macos-aarch64.dmg"}]}"#;
        let release = parse_latest(body).unwrap().unwrap();
        assert_eq!(release.assets.len(), 1);
        assert_eq!(release.assets[0].name, "leon-0.2.1-macos-aarch64.dmg");
    }

    #[test]
    fn a_foreign_page_is_replaced_by_the_releases_page() {
        let body = r#"{"tag_name":"v0.2.1","html_url":"https://evil.example/x"}"#;
        assert_eq!(parse_latest(body).unwrap().unwrap().page, RELEASES_PAGE);
    }

    #[test]
    fn the_list_gives_the_newest_that_the_channel_allows() {
        let list = format!(
            "[{},{},{},{}]",
            json("v0.3.0-rc.1", true, false, &ALL),
            json("v0.2.1", false, false, &ALL),
            json("v0.2.2", false, true, &ALL),
            json("nightly", false, false, &ALL),
        );
        let stable = parse_list(&list, false).unwrap().unwrap();
        assert_eq!(stable.version, Version::new(0, 2, 1));
        let any = parse_list(&list, true).unwrap().unwrap();
        assert_eq!(any.tag, "v0.3.0-rc.1");
        assert_eq!(parse_list("[]", true), Ok(None));
    }

    #[test]
    fn platforms_are_named_as_the_release_names_them() {
        let mac = Platform::parse("macos-aarch64").unwrap();
        assert_eq!(mac.id(), "macos-aarch64");
        let v = Version::new(0, 2, 1);
        assert_eq!(mac.asset_name(&v), "leon-0.2.1-macos-aarch64.dmg");
        assert_eq!(
            Platform::parse("macos-x86_64").unwrap().asset_name(&v),
            "leon-0.2.1-macos-x86_64.dmg"
        );
        assert_eq!(
            Platform::parse("windows-x86_64").unwrap().asset_name(&v),
            "leon-0.2.1-windows-x86_64.zip"
        );
        assert_eq!(
            Platform::parse("linux-x86_64").unwrap().asset_name(&v),
            "leon-0.2.1-linux-x86_64.tar.gz"
        );
        assert_eq!(Platform::parse("freebsd-x86_64"), None);
        assert_eq!(Platform::parse("macos"), None);
    }

    #[test]
    fn the_archive_of_the_platform_is_selected_among_extra_assets() {
        let mut release = parse_latest(&json("v0.2.1", false, false, &ALL))
            .unwrap()
            .unwrap();
        release.assets.push(Asset {
            name: "leon-0.2.1-macos-aarch64.app.tar.gz".into(),
            size: 1,
            url: download_url("v0.2.1", "leon-0.2.1-macos-aarch64.app.tar.gz"),
            digest: None,
        });
        for platform in ALL {
            let platform = Platform::parse(platform).unwrap();
            match select(&release, &platform) {
                Selection::Found { archive, checksums } => {
                    assert_eq!(archive.name, platform.asset_name(&release.version));
                    assert_eq!(checksums.name, "SHA256SUMS");
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_platform_without_a_build_is_said_so() {
        let release = parse_latest(&json("v0.2.1", false, false, &["linux-x86_64"]))
            .unwrap()
            .unwrap();
        let mac = Platform::parse("macos-aarch64").unwrap();
        assert_eq!(select(&release, &mac), Selection::NoBuild);
        let other = Platform {
            os: Os::Linux,
            arch: Arch::Other,
        };
        assert_eq!(select(&release, &other), Selection::NoBuild);
        let arm_linux = Platform {
            os: Os::Linux,
            arch: Arch::Aarch64,
        };
        assert_eq!(select(&release, &arm_linux), Selection::NoBuild);
    }

    #[test]
    fn an_archive_without_checksums_is_not_installable() {
        let mut release = parse_latest(&json("v0.2.1", false, false, &ALL))
            .unwrap()
            .unwrap();
        release.assets.retain(|asset| asset.name != "SHA256SUMS");
        let linux = Platform::parse("linux-x86_64").unwrap();
        assert_eq!(select(&release, &linux), Selection::NoChecksums);
    }

    #[test]
    fn the_stated_digest_is_read_strictly() {
        assert_eq!(
            parse_digest(&format!("sha256:{}", "0f".repeat(32))),
            Some([0x0f; 32])
        );
        assert_eq!(parse_digest("sha1:abcd"), None);
        assert_eq!(parse_digest("sha256:abcd"), None);
        assert_eq!(parse_digest(&"0f".repeat(32)), None);
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// A release the way GitHub answers, trimmed to what is read.
    pub(crate) fn json(tag: &str, prerelease: bool, draft: bool, platforms: &[&str]) -> String {
        let version = tag.trim_start_matches('v');
        let mut assets: Vec<String> = platforms
            .iter()
            .map(|platform| {
                let ext = Platform::parse(platform).map_or("tar.gz", |p| p.extension());
                let name = format!("leon-{version}-{platform}.{ext}");
                format!(
                    r#"{{"name":"{name}","size":1000,"browser_download_url":"{}","digest":"sha256:{}"}}"#,
                    download_url(tag, &name),
                    "ab".repeat(32)
                )
            })
            .collect();
        assets.push(format!(
            r#"{{"name":"SHA256SUMS","size":300,"browser_download_url":"{}"}}"#,
            download_url(tag, "SHA256SUMS")
        ));
        format!(
            r###"{{"tag_name":"{tag}","prerelease":{prerelease},"draft":{draft},
            "html_url":"https://github.com/zavudev/leon/releases/tag/{tag}",
            "body":"## Notes\n\n- one\n- two","assets":[{}]}}"###,
            assets.join(",")
        )
    }
}

//! Which versions count, and which is newer.
//!
//! A release is a tag `v<semver>` and nothing else: `v0.2.1`, `v1.0.0-rc.1`.
//! Anything that does not read that way (`0.2.1` without the `v`, `latest`,
//! `v1.2`, `v1.2.3+build`) is not a release of Leon and is ignored.

pub use semver::Version;

/// The version a tag names, when the tag is `v` and a full semantic version
/// without build metadata.
pub fn parse_tag(tag: &str) -> Option<Version> {
    let text = tag.strip_prefix('v')?;
    if text.contains('+') {
        return None;
    }
    Version::parse(text).ok()
}

/// The version a running build reports (`CARGO_PKG_VERSION`), as it is.
pub fn parse_running(text: &str) -> Option<Version> {
    Version::parse(text.trim()).ok()
}

/// Whether `candidate` is an update for `current`: strictly newer, and not a
/// pre-release unless the user follows the pre-release channel. An older or
/// equal version is never an update, whatever the channel.
pub fn is_update(current: &Version, candidate: &Version, prereleases: bool) -> bool {
    candidate > current && (prereleases || candidate.pre.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn only_a_v_and_a_full_semantic_version_is_a_release_tag() {
        assert_eq!(parse_tag("v0.2.1"), Some(v("0.2.1")));
        assert_eq!(parse_tag("v1.0.0-rc.1"), Some(v("1.0.0-rc.1")));
        for bad in [
            "0.2.1",
            "V0.2.1",
            "v1.2",
            "v1",
            "latest",
            "",
            "v",
            "vv1.2.3",
            "v1.2.3+b5",
            "v01.2.3",
            "v1.2.3 ",
            " v1.2.3",
            "release-1.2.3",
        ] {
            assert_eq!(parse_tag(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn ordering_is_semantic_not_textual() {
        assert!(is_update(&v("0.9.0"), &v("0.10.0"), false));
        assert!(is_update(&v("0.2.0"), &v("0.2.1"), false));
        assert!(is_update(&v("0.2.0"), &v("1.0.0"), false));
        assert!(!is_update(&v("0.10.0"), &v("0.9.0"), false));
    }

    #[test]
    fn an_equal_or_older_version_is_never_an_update() {
        for channel in [false, true] {
            assert!(!is_update(&v("0.2.0"), &v("0.2.0"), channel));
            assert!(!is_update(&v("0.2.0"), &v("0.1.9"), channel));
            assert!(!is_update(&v("0.2.0"), &v("0.2.0-rc.1"), channel));
            assert!(!is_update(&v("1.0.0"), &v("0.9.9-beta.1"), channel));
        }
    }

    #[test]
    fn pre_releases_are_ignored_unless_the_user_asked_for_them() {
        assert!(!is_update(&v("0.2.0"), &v("0.3.0-rc.1"), false));
        assert!(is_update(&v("0.2.0"), &v("0.3.0-rc.1"), true));
    }

    #[test]
    fn a_pre_release_build_is_updated_to_its_own_final_release() {
        assert!(is_update(&v("0.3.0-rc.1"), &v("0.3.0"), false));
        assert!(is_update(&v("0.3.0-rc.1"), &v("0.3.0-rc.2"), true));
        assert!(!is_update(&v("0.3.0-rc.2"), &v("0.3.0-rc.1"), true));
    }

    #[test]
    fn the_running_version_is_read_leniently_about_whitespace_only() {
        assert_eq!(parse_running(" 0.2.0\n"), Some(v("0.2.0")));
        assert_eq!(parse_running("v0.2.0"), None);
    }
}

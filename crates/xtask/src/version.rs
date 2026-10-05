//! Versions as the release tooling reads them: `MAJOR.MINOR.PATCH`, with an
//! optional `-pre.release` suffix. Build metadata (`+...`) is not used by Leon.

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// The text after `-`, when there is one.
    pub pre: Option<String>,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: None,
        }
    }

    pub fn is_prerelease(&self) -> bool {
        self.pre.is_some()
    }

    /// The numbers only, for ordering releases.
    pub fn core(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    /// A pre-release sorts before its release; two pre-releases by text.
    fn cmp(&self, other: &Self) -> Ordering {
        self.core()
            .cmp(&other.core())
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

fn number(text: &str, what: &str, whole: &str) -> Result<u64, String> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "`{whole}` is not a version: {what} is not a number"
        ));
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err(format!(
            "`{whole}` is not a version: {what} has a leading zero"
        ));
    }
    text.parse()
        .map_err(|_| format!("`{whole}` is not a version: {what} is too large"))
}

/// Parses `X.Y.Z` or `X.Y.Z-pre`.
pub fn parse(text: &str) -> Result<Version, String> {
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let [major, minor, patch] = parts[..] else {
        return Err(format!("`{text}` is not a version: expected X.Y.Z"));
    };
    let pre = match pre {
        None => None,
        Some(pre) => {
            let valid = !pre.is_empty()
                && pre.split('.').all(|id| {
                    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                });
            if !valid {
                return Err(format!("`{text}` is not a version: bad pre-release suffix"));
            }
            Some(pre.to_owned())
        }
    };
    Ok(Version {
        major: number(major, "the major part", text)?,
        minor: number(minor, "the minor part", text)?,
        patch: number(patch, "the patch part", text)?,
        pre,
    })
}

/// The tag of a release is `v` followed by the workspace's version.
pub fn check_tag(tag: &str, version: &Version) -> Result<(), String> {
    let expected = format!("v{version}");
    if tag == expected {
        Ok(())
    } else {
        Err(format!(
            "the tag is {tag} and the workspace's version is {version}: the tag of this release is {expected}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_pre_release_versions() {
        assert_eq!(parse("0.1.0").unwrap(), Version::new(0, 1, 0));
        let pre = parse("1.2.3-rc.1").unwrap();
        assert_eq!(pre.pre.as_deref(), Some("rc.1"));
        assert!(pre.is_prerelease());
        assert_eq!(pre.to_string(), "1.2.3-rc.1");
    }

    #[test]
    fn refuses_what_is_not_a_version() {
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "a.b.c",
            "01.2.3",
            "1.2.-3",
            "1.2.3-",
            "1.2.3-a..b",
            "v1.2.3",
            "1.2.3+build",
            " 1.2.3",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_pre_release_sorts_before_its_release() {
        assert!(parse("1.0.0-rc.1").unwrap() < parse("1.0.0").unwrap());
        assert!(parse("0.9.9").unwrap() < parse("1.0.0-rc.1").unwrap());
        assert!(parse("0.1.0").unwrap() < parse("0.1.1").unwrap());
        assert!(parse("0.2.0").unwrap() > parse("0.1.9").unwrap());
    }

    #[test]
    fn the_tag_is_v_and_the_version() {
        let version = parse("0.1.0").unwrap();
        assert!(check_tag("v0.1.0", &version).is_ok());
        assert!(check_tag("0.1.0", &version).is_err());
        assert!(check_tag("v0.1.1", &version).is_err());
        assert!(check_tag("v0.1.0-rc.1", &version).is_err());
        let pre = parse("0.2.0-beta.1").unwrap();
        assert!(check_tag("v0.2.0-beta.1", &pre).is_ok());
    }
}

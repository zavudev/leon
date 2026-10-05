//! `cargo xtask bump`: the arithmetic, and the edit of the workspace's
//! `Cargo.toml`.

use crate::version::{self, Version};

/// The version a bump request leads to. `request` is `major`, `minor`,
/// `patch`, or an explicit `X.Y.Z` that must be above `current`.
///
/// Pre-releases are refused on both sides: a `-suffix` has no natural next
/// number, so a pre-release is set by editing `Cargo.toml` by hand.
pub fn next(current: &Version, request: &str) -> Result<Version, String> {
    if current.is_prerelease() {
        return Err(format!(
            "the current version {current} is a pre-release: give the release to cut as X.Y.Z, \
             or edit Cargo.toml by hand"
        ));
    }
    let next = match request {
        "major" => Version::new(current.major + 1, 0, 0),
        "minor" => Version::new(current.major, current.minor + 1, 0),
        "patch" => Version::new(current.major, current.minor, current.patch + 1),
        explicit => {
            let version = version::parse(explicit)
                .map_err(|_| format!("`{explicit}` is neither major, minor, patch nor X.Y.Z"))?;
            if version.is_prerelease() {
                return Err(format!(
                    "{version} is a pre-release: bump takes release versions only (edit Cargo.toml by hand for a pre-release)"
                ));
            }
            version
        }
    };
    if next <= *current {
        return Err(format!("{next} is not above the current version {current}"));
    }
    Ok(next)
}

/// Where the `version = "..."` line of `[workspace.package]` is in `text`:
/// the byte range of the quoted value, without the quotes.
fn workspace_version_span(text: &str) -> Result<(usize, usize), String> {
    let mut in_section = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.package]";
        } else if in_section {
            if let Some(rest) = trimmed.strip_prefix("version") {
                if let Some(value) = rest.trim_start().strip_prefix('=') {
                    let value = value.trim();
                    if let Some(inner) = value.strip_prefix('"').and_then(|v| v.split('"').next()) {
                        let start = offset + line.find('"').unwrap_or(0) + 1;
                        return Ok((start, start + inner.len()));
                    }
                }
            }
        }
        offset += line.len();
    }
    Err("no `version = \"...\"` in [workspace.package] of Cargo.toml".into())
}

/// The workspace's version as written in `Cargo.toml`.
pub fn workspace_version(text: &str) -> Result<String, String> {
    let (start, end) = workspace_version_span(text)?;
    Ok(text[start..end].to_owned())
}

/// `text` with the workspace's version replaced; nothing else changes.
pub fn set_workspace_version(text: &str, version: &Version) -> Result<String, String> {
    let (start, end) = workspace_version_span(text)?;
    Ok(format!("{}{version}{}", &text[..start], &text[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        version::parse(text).unwrap()
    }

    #[test]
    fn keywords_advance_the_right_part_and_reset_the_rest() {
        assert_eq!(next(&v("0.1.0"), "patch").unwrap(), v("0.1.1"));
        assert_eq!(next(&v("0.1.7"), "minor").unwrap(), v("0.2.0"));
        assert_eq!(next(&v("1.4.9"), "major").unwrap(), v("2.0.0"));
    }

    #[test]
    fn an_explicit_version_must_be_above_the_current_one() {
        assert_eq!(next(&v("0.1.0"), "0.3.0").unwrap(), v("0.3.0"));
        assert!(next(&v("0.3.0"), "0.3.0").is_err());
        assert!(next(&v("0.3.0"), "0.2.9").is_err());
    }

    #[test]
    fn pre_releases_are_refused_on_both_sides() {
        assert!(next(&v("0.1.0"), "0.2.0-rc.1").is_err());
        assert!(next(&v("0.2.0-rc.1"), "patch").is_err());
        assert!(next(&v("0.2.0-rc.1"), "0.2.0").is_err());
    }

    #[test]
    fn nonsense_is_refused() {
        for bad in ["", "Patch", "1.2", "v1.0.0", "next"] {
            assert!(next(&v("0.1.0"), bad).is_err(), "{bad:?} was accepted");
        }
    }

    const MANIFEST: &str = r#"[workspace]
members = ["a"]

[workspace.package]
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
serde = { version = "1" }
"#;

    #[test]
    fn reads_the_version_of_workspace_package_only() {
        assert_eq!(workspace_version(MANIFEST).unwrap(), "0.1.0");
        assert!(workspace_version("[package]\nversion = \"1.0.0\"\n").is_err());
    }

    #[test]
    fn rewrites_only_that_line() {
        let updated = set_workspace_version(MANIFEST, &v("0.2.0")).unwrap();
        assert_eq!(
            updated,
            MANIFEST.replace("version = \"0.1.0\"", "version = \"0.2.0\"")
        );
        assert!(updated.contains("serde = { version = \"1\" }"));
    }

    #[test]
    fn handles_crlf_and_extra_spaces() {
        let text = "[workspace.package]\r\nversion   =   \"1.0.0\"  # now\r\n";
        assert_eq!(workspace_version(text).unwrap(), "1.0.0");
        let updated = set_workspace_version(text, &v("1.0.1")).unwrap();
        assert_eq!(
            updated,
            "[workspace.package]\r\nversion   =   \"1.0.1\"  # now\r\n"
        );
    }
}

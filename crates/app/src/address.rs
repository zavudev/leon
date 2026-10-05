//! Parsing of what people type into the machine and project steps.
//!
//! Three small, pure pieces of input handling live here so they can be tested
//! without a window: the `user@host[:port]` destination of an SSH machine,
//! the check that a project path is absolute (on the machine's operating
//! system, which may not be the one Leon runs on), and the path a new git
//! worktree is given.

/// Where an SSH machine is, as typed: `user@host[:port]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// Login name, when given.
    pub user: Option<String>,
    /// Host name, address or SSH configuration alias.
    pub host: String,
    /// TCP port, when given.
    pub port: Option<u16>,
}

/// Why a destination was refused. The text is shown to the user as is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AddressError(pub &'static str);

/// Reads `user@host`, `host`, `user@host:port`, `host:port`, the bracketed
/// IPv6 forms `[::1]` and `user@[::1]:22`, and any of them as an
/// `ssh://` address.
pub fn parse_destination(input: &str) -> Result<Destination, AddressError> {
    let input = input.trim();
    let input = match input.strip_prefix("ssh://") {
        Some(rest) => rest.trim_end_matches('/'),
        None => input,
    };
    if input.is_empty() {
        return Err(AddressError("Type user@host or host."));
    }
    if input.chars().any(char::is_whitespace) {
        return Err(AddressError("A destination has no spaces."));
    }
    let (user, rest) = match input.split_once('@') {
        Some((user, rest)) => {
            if user.is_empty() {
                return Err(AddressError("The user name is empty."));
            }
            if rest.contains('@') {
                return Err(AddressError("Only one @ is allowed."));
            }
            (Some(user.to_owned()), rest)
        }
        None => (None, input),
    };
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        let (host, after) = inner
            .split_once(']')
            .ok_or(AddressError("A bracketed address needs a closing ]."))?;
        match after {
            "" => (host, None),
            _ => match after.strip_prefix(':') {
                Some(port) => (host, Some(port)),
                None => return Err(AddressError("Only a :port may follow ].")),
            },
        }
    } else if rest.matches(':').count() > 1 {
        // A bare IPv6 address: its colons are not a port.
        (rest, None)
    } else {
        match rest.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        }
    };
    if host.is_empty() {
        return Err(AddressError("The host is empty."));
    }
    if host.starts_with('-') {
        return Err(AddressError("A host cannot start with a dash."));
    }
    let port = match port {
        None => None,
        Some(text) => match text.parse::<u16>() {
            Ok(port) if port != 0 => Some(port),
            _ => return Err(AddressError("The port must be a number from 1 to 65535.")),
        },
    };
    Ok(Destination {
        user,
        host: host.to_owned(),
        port,
    })
}

/// Whether `path` is absolute on a POSIX machine or a Windows one.
pub fn is_absolute_path(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with("\\\\") {
        return true;
    }
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// The last component of a path, used as the default project name.
pub fn default_project_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Why a branch name was refused.
pub fn validate_branch(name: &str) -> Result<(), &'static str> {
    if name.trim().is_empty() {
        return Err("Type a branch name.");
    }
    if name != name.trim() || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("A branch name has no spaces.");
    }
    if name.starts_with('-') {
        return Err("A branch name cannot start with a dash.");
    }
    if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return Err("A branch name cannot have an empty part.");
    }
    if name == "@" || name.contains("@{") || name.contains("..") || name.ends_with(".lock") {
        return Err("Git does not allow that branch name.");
    }
    if name.chars().any(|c| "~^:?*[\\".contains(c)) {
        return Err("A branch name cannot contain ~ ^ : ? * [ or a backslash.");
    }
    Ok(())
}

/// Where a new worktree for `branch` goes: next to the project, in
/// `<name>-worktrees/<branch>`, with the branch's slashes turned into dashes.
pub fn worktree_path(root: &str, branch: &str) -> String {
    let windows = root.contains('\\') && !root.contains('/');
    let separator = if windows { '\\' } else { '/' };
    let trimmed = root.trim_end_matches(['/', '\\']);
    let folder = branch.replace('/', "-");
    format!("{trimmed}-worktrees{separator}{folder}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(user: Option<&str>, host: &str, port: Option<u16>) -> Destination {
        Destination {
            user: user.map(str::to_owned),
            host: host.to_owned(),
            port,
        }
    }

    #[test]
    fn a_bare_host_is_a_destination() {
        assert_eq!(
            parse_destination("build.example"),
            Ok(destination(None, "build.example", None))
        );
    }

    #[test]
    fn ssh_urls_are_accepted() {
        assert_eq!(
            parse_destination("ssh://dev@box:2222"),
            Ok(destination(Some("dev"), "box", Some(2222)))
        );
        assert_eq!(
            parse_destination(" ssh://box/ "),
            Ok(destination(None, "box", None))
        );
        assert_eq!(
            parse_destination("ssh://dev@[::1]:22/"),
            Ok(destination(Some("dev"), "::1", Some(22)))
        );
        assert!(parse_destination("ssh://").is_err());
    }

    #[test]
    fn user_host_and_port_are_split() {
        assert_eq!(
            parse_destination("dev@build.example:2222"),
            Ok(destination(Some("dev"), "build.example", Some(2222)))
        );
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            parse_destination("  dev@box  "),
            Ok(destination(Some("dev"), "box", None))
        );
    }

    #[test]
    fn bracketed_ipv6_addresses_keep_their_colons() {
        assert_eq!(
            parse_destination("dev@[fe80::1]:22"),
            Ok(destination(Some("dev"), "fe80::1", Some(22)))
        );
        assert_eq!(
            parse_destination("[::1]"),
            Ok(destination(None, "::1", None))
        );
    }

    #[test]
    fn an_unbracketed_ipv6_address_is_a_host_without_a_port() {
        assert_eq!(
            parse_destination("fe80::1"),
            Ok(destination(None, "fe80::1", None))
        );
    }

    #[test]
    fn malformed_destinations_are_refused() {
        for bad in [
            "",
            "   ",
            "@host",
            "user@",
            "a b@host",
            "user@ho st",
            "host:",
            "host:0",
            "host:70000",
            "host:ssh",
            "-oProxyCommand=x",
            "user@-host",
            "a@b@c",
            "[::1",
        ] {
            assert!(parse_destination(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn absolute_paths_are_recognised_for_both_operating_systems() {
        for good in [
            "/srv/api",
            "/",
            "C:\\code\\api",
            "c:/code/api",
            "\\\\host\\share\\x",
        ] {
            assert!(is_absolute_path(good), "{good:?}");
        }
        for bad in ["", "api", "./api", "../api", "~/api", "C:api", "code/api"] {
            assert!(!is_absolute_path(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_default_project_name_is_the_last_path_component() {
        assert_eq!(default_project_name("/srv/api"), "api");
        assert_eq!(default_project_name("/srv/api/"), "api");
        assert_eq!(default_project_name("C:\\code\\api"), "api");
        assert_eq!(default_project_name("/"), "");
    }

    #[test]
    fn branch_names_that_git_would_misread_are_refused() {
        for bad in [
            "", "  ", "-x", "a b", "a..b", "a~1", "a^", "a:b", "a?", "a*", "a[", "a\\b", "/a",
            "a/", "a//b", "a.lock", "@{x}", "@",
        ] {
            assert!(validate_branch(bad).is_err(), "{bad:?} should be refused");
        }
        for good in ["feature/login", "fix-12", "release_1.2", "UPPER"] {
            assert!(validate_branch(good).is_ok(), "{good:?} should be accepted");
        }
    }

    #[test]
    fn a_worktree_goes_in_a_sibling_directory_named_after_the_project() {
        assert_eq!(
            worktree_path("/srv/api", "feature/login"),
            "/srv/api-worktrees/feature-login"
        );
        assert_eq!(worktree_path("/srv/api/", "fix"), "/srv/api-worktrees/fix");
        assert_eq!(
            worktree_path("C:\\code\\api", "fix"),
            "C:\\code\\api-worktrees\\fix"
        );
    }
}

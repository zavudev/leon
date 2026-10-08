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

/// The name a clone of `url` takes: git's own rule, the last path segment
/// without a trailing `.git`. Both URL shapes count (`https://host/owner/repo.git`
/// and `git@host:owner/repo.git`), and an empty answer means the URL names
/// nothing to clone.
pub fn default_project_name_from_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    // A URL has a path after its last slash; the scp-like `host:path` form
    // has none, so its colon is what separates the path.
    let tail = match trimmed.rsplit_once('/') {
        Some((_, last)) => last,
        None => trimmed.rsplit(':').next().unwrap_or_default(),
    }
    .trim_end_matches(".git");
    if tail.is_empty() || tail == "." || tail == ".." {
        String::new()
    } else {
        tail.to_owned()
    }
}

/// `parent` and `name` joined with the separator the parent was written
/// with, so a Windows path stays a Windows path. A bare root keeps its
/// separator (`/` + `api` is `/api`, not `//api`).
pub fn join_path(parent: &str, name: &str) -> String {
    let windows = parent.contains('\\') && !parent.contains('/');
    let separator = if windows { '\\' } else { '/' };
    let trimmed = parent.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        // The parent itself is a root ("/" or "C:\").
        if parent.ends_with(['/', '\\']) {
            format!("{parent}{name}")
        } else {
            format!("{parent}{separator}{name}")
        }
    } else {
        format!("{trimmed}{separator}{name}")
    }
}

/// Why a project name was refused.
pub fn validate_project_name(name: &str) -> Result<(), &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Type a name.");
    }
    if name.contains('/') || name.contains('\\') {
        return Err("A project name cannot contain a slash.");
    }
    if name == "." || name == ".." {
        return Err("A project name cannot be . or ..");
    }
    if name.starts_with('-') {
        return Err("A project name cannot start with a dash.");
    }
    Ok(())
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

/// The setting `worktree_location` as it is when nothing was chosen: next to
/// the project, in `<name>-worktrees/<branch>`.
pub const DEFAULT_WORKTREE_LOCATION: &str = "{root}-worktrees/{branch}";

/// Where a new worktree for `branch` goes with the default location: next to
/// the project, in `<name>-worktrees/<branch>`, with the branch's slashes
/// turned into dashes. The engine reads the setting; this is the default, for
/// the tests that must know where a worktree lands.
#[cfg(test)]
pub fn worktree_path(root: &str, branch: &str) -> String {
    // The default template has only names it knows.
    expand_location(DEFAULT_WORKTREE_LOCATION, root, branch).unwrap_or_default()
}

/// Where a new worktree for `branch` goes under a location template: the text
/// of the setting `worktree_location`, in which `{root}` is the project's root
/// (without a trailing separator) and `{branch}` the branch with its slashes
/// turned into dashes. A slash written in the template follows the root's own
/// separator, so a Windows project stays a Windows path. An empty template is
/// the default. The template must name `{branch}` (two worktrees cannot share
/// a folder) and give an absolute path.
pub fn worktree_location(template: &str, root: &str, branch: &str) -> Result<String, String> {
    let template = match template.trim() {
        "" => DEFAULT_WORKTREE_LOCATION,
        given => given,
    };
    if !template.contains("{branch}") {
        return Err(format!(
            "The worktree location {template:?} must contain {{branch}}, or every worktree would be in the same folder."
        ));
    }
    let path = expand_location(template, root, branch)?;
    if !is_absolute_path(&path) {
        return Err(format!(
            "The worktree location {template:?} gives {path:?}, which is not an absolute path: start it with {{root}} or a full path."
        ));
    }
    Ok(path)
}

fn expand_location(template: &str, root: &str, branch: &str) -> Result<String, String> {
    let windows = root.contains('\\') && !root.contains('/');
    let separator = if windows { '\\' } else { '/' };
    let trimmed = root.trim_end_matches(['/', '\\']);
    let folder = branch.replace('/', "-");
    let mut path = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        push_literal(&mut path, &rest[..open], separator);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Err(format!(
                "The worktree location {template:?} has a {{ without a }}."
            ));
        };
        match &after[..close] {
            "root" => path.push_str(trimmed),
            "branch" => path.push_str(&folder),
            other => {
                return Err(format!(
                    "The worktree location has {{{other}}}; only {{root}} and {{branch}} are known."
                ))
            }
        }
        rest = &after[close + 1..];
    }
    push_literal(&mut path, rest, separator);
    Ok(path)
}

fn push_literal(path: &mut String, literal: &str, separator: char) {
    path.extend(
        literal
            .chars()
            .map(|c| if c == '/' { separator } else { c }),
    );
}

/// The address a `#123` in a terminal opens for a git remote on GitHub: the
/// pull request page, where GitHub sends an issue number to its issue. `None`
/// for any other host.
pub fn github_pull_base(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let path = remote
        .strip_prefix("git@github.com:")
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))
        .or_else(|| remote.strip_prefix("https://github.com/"))
        .or_else(|| remote.strip_prefix("http://github.com/"))
        .or_else(|| {
            // `https://user@github.com/...` and `https://user:token@github.com/...`.
            let rest = remote.strip_prefix("https://")?;
            let (_, after) = rest.split_once('@')?;
            after.strip_prefix("github.com/")
        })?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    };
    (valid(owner) && valid(repo)).then(|| format!("https://github.com/{owner}/{repo}/pull/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_github_remote_gives_its_pull_request_base() {
        let base = Some("https://github.com/zavudev/leon/pull/".to_owned());
        for remote in [
            "git@github.com:zavudev/leon.git",
            "https://github.com/zavudev/leon",
            "https://github.com/zavudev/leon.git\n",
            "ssh://git@github.com/zavudev/leon.git",
            "https://me:token@github.com/zavudev/leon.git",
        ] {
            assert_eq!(github_pull_base(remote), base, "{remote:?}");
        }
        for remote in [
            "git@gitlab.com:zavudev/leon.git",
            "https://github.com/zavudev",
            "https://github.com/a b/c",
            "/srv/git/leon.git",
            "",
        ] {
            assert_eq!(github_pull_base(remote), None, "{remote:?}");
        }
    }

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
    fn a_clone_takes_its_name_from_the_url_like_git_does() {
        assert_eq!(
            default_project_name_from_url("https://github.com/zavudev/leon.git"),
            "leon"
        );
        assert_eq!(
            default_project_name_from_url("git@github.com:zavudev/leon.git"),
            "leon"
        );
        assert_eq!(
            default_project_name_from_url("ssh://git@host/owner/repo"),
            "repo"
        );
        assert_eq!(
            default_project_name_from_url("/srv/git/thing.git/"),
            "thing"
        );
        assert_eq!(default_project_name_from_url("https://host/"), "host");
        assert_eq!(default_project_name_from_url("  "), "");
    }

    #[test]
    fn paths_join_with_the_separator_they_were_written_with() {
        assert_eq!(join_path("/srv", "api"), "/srv/api");
        assert_eq!(join_path("/srv/", "api"), "/srv/api");
        assert_eq!(join_path("/", "api"), "/api");
        assert_eq!(join_path("C:\\code", "api"), "C:\\code\\api");
        assert_eq!(join_path("C:\\", "api"), "C:\\api");
    }

    #[test]
    fn project_names_with_a_slash_or_a_dot_are_refused() {
        for bad in ["", "  ", "a/b", "a\\b", ".", "..", "-x"] {
            assert!(validate_project_name(bad).is_err(), "{bad:?}");
        }
        for good in ["api", "my project", "leon-2"] {
            assert!(validate_project_name(good).is_ok(), "{good:?}");
        }
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

    #[test]
    fn the_default_location_is_exactly_the_path_of_before() {
        // What `worktree_path` was before it read a template.
        fn before(root: &str, branch: &str) -> String {
            let windows = root.contains('\\') && !root.contains('/');
            let separator = if windows { '\\' } else { '/' };
            let trimmed = root.trim_end_matches(['/', '\\']);
            let folder = branch.replace('/', "-");
            format!("{trimmed}-worktrees{separator}{folder}")
        }
        for root in [
            "/srv/api",
            "/srv/api/",
            "/home/me/my project",
            "C:\\code\\api",
            "C:\\code\\api\\",
            "D:/code/api",
            "//server/share/api",
        ] {
            for branch in ["fix", "feature/login", "a/b/c", "release_1.2"] {
                assert_eq!(
                    worktree_path(root, branch),
                    before(root, branch),
                    "{root} {branch}"
                );
                assert_eq!(
                    worktree_location(DEFAULT_WORKTREE_LOCATION, root, branch).unwrap(),
                    before(root, branch),
                    "{root} {branch}"
                );
                assert_eq!(
                    worktree_location("", root, branch).unwrap(),
                    before(root, branch),
                    "an empty setting is the default"
                );
            }
        }
    }

    #[test]
    fn a_location_template_puts_worktrees_where_it_says() {
        assert_eq!(
            worktree_location("{root}/.worktrees/{branch}", "/srv/api/", "feature/login").unwrap(),
            "/srv/api/.worktrees/feature-login"
        );
        assert_eq!(
            worktree_location("/work/trees/{branch}", "/srv/api", "fix").unwrap(),
            "/work/trees/fix"
        );
        assert_eq!(
            worktree_location("{root}/../wt/{branch}", "C:\\code\\api", "fix").unwrap(),
            "C:\\code\\api\\..\\wt\\fix",
            "slashes of the template follow the root"
        );
    }

    #[test]
    fn a_location_template_that_cannot_work_says_why() {
        for (template, mentions) in [
            ("{root}-worktrees", "{branch}"),
            ("{root}/{name}/{branch}", "{name}"),
            ("{root}/{branch}/{oops", "without"),
            ("wt/{branch}", "absolute"),
            ("~/wt/{branch}", "absolute"),
        ] {
            let why = worktree_location(template, "/srv/api", "fix").expect_err(template);
            assert!(why.contains(mentions), "{template}: {why}");
        }
    }
}

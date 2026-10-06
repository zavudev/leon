//! Path identity: when two spellings are the same folder.
//!
//! A path lives on a machine, and the same folder is spelled differently by
//! the programs that name it: git prints `C:/Users/me/code/api`, an agent CLI
//! records `C:\Users\me\code\api`, a drive letter is `c:` or `C:`, a path may
//! carry the verbatim prefix `\\?\`, a trailing separator, and the file system
//! does not tell `Api` from `api`. [`key`] turns any spelling into one
//! identity string, to **compare paths and to use them as map keys**. It is
//! never shown and never handed back to the operating system or to git, which
//! keep the string they were given.
//!
//! The style follows the path, not the computer Leon runs on: a path with a
//! drive letter, a UNC share or the verbatim prefix is a Windows path
//! ([`PathStyle::of`]), whatever computer reads it, so a Windows client reading
//! a Linux server, and a Linux box with a Windows machine in its store, both
//! decide per path. Anything else is POSIX, where a backslash is an ordinary
//! character of a file name.
//!
//! * **Windows**: the `\\?\` and `\\?\UNC\` prefixes are dropped, `/` becomes
//!   `\`, repeated separators collapse, trailing ones are dropped (a bare
//!   `C:\` keeps its one), and the whole path is lower-cased with Unicode
//!   simple case folding (`char::to_lowercase`; NTFS compares with its own
//!   upcase table, which this approximates for every name seen in practice).
//! * **POSIX**: case-sensitive, trailing slashes dropped, `/` kept.
//!
//! macOS: its default file system is case-insensitive but case-preserving,
//! yet git and the agents record a folder with the case it has on disk, so
//! the same folder is spelled one way and the failure this module fixes does
//! not occur; macOS paths stay case-sensitive.

/// How a path is spelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStyle {
    /// `C:\dir`, `\\server\share\dir`, `\\?\C:\dir`; `/` is accepted too.
    Windows,
    /// `/dir`; a backslash is part of a name.
    Posix,
}

impl PathStyle {
    /// The style a path is spelled in, from the path alone.
    pub fn of(path: &str) -> Self {
        let bytes = path.as_bytes();
        let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        let verbatim = path.starts_with("\\\\?\\") || path.starts_with("//?/");
        let unc = (path.starts_with("\\\\") || path.starts_with("//"))
            && path.len() > 2
            && !path[2..].starts_with(['\\', '/']);
        // A drive letter followed by anything but a separator (`C:x`) is a
        // drive-relative path, still Windows.
        if drive || verbatim || unc {
            PathStyle::Windows
        } else {
            PathStyle::Posix
        }
    }

    /// The separator the key uses.
    pub fn separator(self) -> char {
        match self {
            PathStyle::Windows => '\\',
            PathStyle::Posix => '/',
        }
    }
}

/// The identity of a path: equal for every spelling of one folder.
pub fn key(path: &str) -> String {
    match PathStyle::of(path) {
        PathStyle::Posix => {
            let trimmed = path.trim_end_matches('/');
            if trimmed.is_empty() && path.starts_with('/') {
                "/".to_owned()
            } else {
                trimmed.to_owned()
            }
        }
        PathStyle::Windows => windows_key(path),
    }
}

fn windows_key(path: &str) -> String {
    let unified: String = path.replace('/', "\\");
    let (unc, rest) = if let Some(rest) = unified.strip_prefix("\\\\?\\UNC\\") {
        (true, rest.to_owned())
    } else if let Some(rest) = unified.strip_prefix("\\\\?\\") {
        (false, rest.to_owned())
    } else if let Some(rest) = unified.strip_prefix("\\\\") {
        (true, rest.to_owned())
    } else {
        (false, unified)
    };
    let parts: Vec<&str> = rest.split('\\').filter(|part| !part.is_empty()).collect();
    let mut out = String::new();
    if unc {
        out.push_str("\\\\");
    }
    out.push_str(&parts.join("\\"));
    // A bare drive keeps its separator: `C:` alone means "the current folder
    // of C:", `C:\` the root.
    if !unc && parts.len() == 1 && parts[0].ends_with(':') && rest.contains('\\') {
        out.push('\\');
    }
    out.to_lowercase()
}

/// Whether `path` is `root` or lies below it, by [`key`].
pub fn is_within(path: &str, root: &str) -> bool {
    within_keys(&key(path), &key(root))
}

/// [`is_within`] for keys already made.
pub fn within_keys(path: &str, root: &str) -> bool {
    let Some(rest) = path.strip_prefix(root) else {
        return false;
    };
    let separator = PathStyle::of(root).separator();
    rest.is_empty() || rest.starts_with(separator) || root.ends_with(separator)
}

/// The keys of a path and of every folder above it, longest first, down to
/// the root (`/`, `c:\`, or a share).
pub fn ancestor_keys(path: &str) -> Vec<String> {
    let mut current = key(path);
    if current.is_empty() {
        return Vec::new();
    }
    let separator = PathStyle::of(&current).separator();
    let mut out = vec![current.clone()];
    while let Some(cut) = current.rfind(separator) {
        let parent = &current[..cut];
        // Stop above a share; keep the root of a drive or of `/`.
        if parent.is_empty() {
            if separator == '/' && current != "/" {
                out.push("/".to_owned());
            }
            break;
        }
        if parent == "\\" || (parent.starts_with("\\\\") && !parent[2..].contains('\\')) {
            break;
        }
        let drive_root = parent.ends_with(':');
        let parent = if drive_root {
            format!("{parent}{separator}")
        } else {
            parent.to_owned()
        };
        if out.last() == Some(&parent) {
            break;
        }
        out.push(parent.clone());
        if drive_root {
            break;
        }
        current = parent;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_of_one_windows_folder_share_a_key() {
        let same = [
            r"C:\Users\me\code\api",
            "C:/Users/me/code/api",
            r"c:\users\me\code\api",
            r"C:\Users\me\code\api\",
            "C:/Users/me/code/api/",
            r"\\?\C:\Users\me\code\api",
            r"\\?\c:\USERS\me\code\api\\",
            "C:/Users//me/code/api",
        ];
        let first = key(same[0]);
        for spelling in same {
            assert_eq!(key(spelling), first, "{spelling}");
        }
        assert_eq!(first, r"c:\users\me\code\api");
    }

    #[test]
    fn unc_shares_keep_their_server_and_share() {
        assert_eq!(key(r"\\Server\Share\Dir"), r"\\server\share\dir");
        assert_eq!(key("//server/share/dir/"), r"\\server\share\dir");
        assert_eq!(key(r"\\?\UNC\Server\Share\Dir"), r"\\server\share\dir");
        assert_ne!(key(r"\\server\share\a"), key(r"\\server\other\a"));
        assert_eq!(PathStyle::of(r"\\server\share"), PathStyle::Windows);
    }

    #[test]
    fn a_drive_root_keeps_its_separator() {
        for spelling in [r"C:\", "c:/", r"\\?\C:\", r"C:\\"] {
            assert_eq!(key(spelling), r"c:\", "{spelling}");
        }
        assert_eq!(key("C:"), "c:");
        assert!(is_within(r"C:\code", r"c:/"));
        assert!(!is_within(r"D:\code", r"C:\"));
    }

    #[test]
    fn posix_paths_are_case_sensitive_and_a_backslash_is_a_name_character() {
        assert_eq!(key("/srv/api/"), "/srv/api");
        assert_eq!(key("/"), "/");
        assert_eq!(key("///"), "/");
        assert_ne!(key("/srv/Api"), key("/srv/api"));
        assert_eq!(key(r"/srv/we\ird"), r"/srv/we\ird");
        assert_ne!(key(r"/srv/a\b"), key("/srv/a/b"));
        assert_eq!(PathStyle::of(r"/srv/a\b"), PathStyle::Posix);
        assert_eq!(PathStyle::of("relative\\thing"), PathStyle::Posix);
        assert!(!is_within(r"/srv/a\b", "/srv/a"));
    }

    #[test]
    fn windows_folders_differ_by_case_only_do_not_differ_on_linux_they_do() {
        assert!(is_within(r"C:\Code\API\src", r"c:\code\api"));
        assert!(!is_within("/code/API/src", "/code/api"));
    }

    #[test]
    fn within_means_a_whole_folder_not_a_name_prefix() {
        let table = [
            ("C:/Users/me/code/api", r"C:\Users\me\code\api", true),
            (r"C:\Users\me\code\api\src", "C:/Users/me/code/api", true),
            (r"C:\Users\me\code\api-old", "C:/Users/me/code/api", false),
            ("/srv/api", "/srv/api", true),
            ("/srv/api/src", "/srv/api", true),
            ("/srv/api-old", "/srv/api", false),
            ("/srv", "/", true),
            ("/srv", "/srv/api", false),
            (r"\\srv\share\a\b", r"\\SRV\Share\a", true),
            (r"C:\a", "/a", false),
        ];
        for (path, root, expected) in table {
            assert_eq!(is_within(path, root), expected, "{path} in {root}");
        }
    }

    #[test]
    fn ancestors_walk_up_to_the_root_in_either_style() {
        assert_eq!(
            ancestor_keys("C:/Users/Me/api/"),
            [r"c:\users\me\api", r"c:\users\me", r"c:\users", r"c:\"]
        );
        assert_eq!(
            ancestor_keys("/srv/api/src"),
            ["/srv/api/src", "/srv/api", "/srv", "/"]
        );
        assert_eq!(ancestor_keys("/"), ["/"]);
        assert_eq!(ancestor_keys(""), Vec::<String>::new());
        assert_eq!(
            ancestor_keys(r"\\srv\share\a"),
            [r"\\srv\share\a", r"\\srv\share"]
        );
        assert_eq!(
            ancestor_keys(r"/srv/we\ird/x"),
            [r"/srv/we\ird/x", r"/srv/we\ird", "/srv", "/"]
        );
    }
}

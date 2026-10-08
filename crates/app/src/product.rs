//! The product's identity, in one place.
//!
//! Everything that shows or derives from the name (window title, data
//! directory, application id) goes through this module. Renaming the product
//! means changing the constants here and the package name in this crate's
//! `Cargo.toml`, which names the binary; `product::tests` checks they agree.
//!
//! The name is the product's; the look is the maker's. The brand assets are
//! in `brand.rs` and the design tokens in `theme.rs`.

use std::path::{Path, PathBuf};

/// The name shown to the user.
pub const PRODUCT_NAME: &str = "Leon";

/// The name in a form safe for paths. It is also the package and binary name.
pub const SLUG: &str = "leon";

/// The application id: the window class on Wayland and X11, the desktop
/// entry's name and the bundle identifier.
pub const APP_ID: &str = "dev.zavu.leon";

/// Who makes it: the brand the application wears.
pub const MAKER: &str = "Zavu";

/// The version of this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The directory that holds the local database and the settings.
///
/// Follows each platform's convention: `$XDG_DATA_HOME` (or `~/.local/share`)
/// on Linux, `~/Library/Application Support` on macOS, `%APPDATA%` on
/// Windows. Falls back to the temporary directory when none can be found.
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(SLUG)
}

/// The most bytes the path of a shared connection's socket may have.
/// Unix socket paths are limited to 104 bytes on macOS and 108 on Linux; the
/// part `ssh` adds is the 40 characters of `%C`, a slash and the 17 of the
/// temporary name it binds first.
#[cfg(unix)]
const SOCKET_BUDGET: usize = 100;
#[cfg(unix)]
const SOCKET_SUFFIX: usize = 1 + 40 + 17;

/// The directory for sockets shared by SSH connections: short, private and
/// per user, because the operating system limits socket paths to about a
/// hundred bytes.
pub fn runtime_dir() -> PathBuf {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        let uid = unsafe { libc::geteuid() };
        runtime_dir_for(dirs::runtime_dir(), uid, Path::new("/tmp"))
    }
    #[cfg(not(unix))]
    {
        // Windows has no connection sharing: the directory is never used for
        // sockets, so any private place will do.
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("{SLUG}-ssh"))
    }
}

/// [`runtime_dir`] from its inputs: the platform's runtime directory
/// (`/run/user/1000` on Linux, absent on macOS) when a socket path under it
/// fits, else a short directory named after the user in `tmp`.
#[cfg(unix)]
pub fn runtime_dir_for(runtime: Option<PathBuf>, uid: u32, tmp: &Path) -> PathBuf {
    if let Some(base) = runtime {
        let candidate = base.join(format!("{SLUG}-ssh"));
        if candidate.as_os_str().len() + SOCKET_SUFFIX <= SOCKET_BUDGET {
            return candidate;
        }
    }
    tmp.join(format!("{SLUG}-ssh-{uid}"))
}

/// Creates `path` for the user alone and checks that it is: a real directory
/// (not a link somebody else planted), owned by this user, closed to everyone
/// else. Other users on a shared machine must not be able to reach the
/// sockets of the shared connections.
pub fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        match std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
        {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let meta = std::fs::symlink_metadata(path)?;
        if !meta.is_dir() {
            return Err(std::io::Error::other(format!(
                "{} is not a directory",
                path.display()
            )));
        }
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err(std::io::Error::other(format!(
                "{} belongs to somebody else",
                path.display()
            )));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_the_same_everywhere() {
        // The binary is named after the package.
        assert_eq!(env!("CARGO_PKG_NAME"), SLUG);
        assert_eq!(PRODUCT_NAME.to_lowercase(), SLUG);
        assert_eq!(APP_ID, "dev.zavu.leon");
        assert_eq!(MAKER, "Zavu");
    }

    #[test]
    fn the_data_directory_is_named_after_the_product() {
        assert!(data_dir().ends_with(SLUG));
    }

    #[cfg(unix)]
    #[test]
    fn the_connection_sockets_have_a_short_home() {
        let socket = runtime_dir().join("0123456789012345678901234567890123456789");
        assert!(
            socket.as_os_str().len() + 17 <= 100,
            "{} is too long for a unix socket",
            socket.display()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_short_runtime_directory_is_used_and_a_long_one_is_not() {
        let tmp = Path::new("/tmp");
        assert_eq!(
            runtime_dir_for(Some(PathBuf::from("/run/user/1000")), 1000, tmp),
            Path::new("/run/user/1000/leon-ssh")
        );
        let long = PathBuf::from("/var/folders/g4/0123456789abcdefghijklmnopqrstuvwxyz/T");
        assert_eq!(
            runtime_dir_for(Some(long), 501, tmp),
            Path::new("/tmp/leon-ssh-501")
        );
        assert_eq!(runtime_dir_for(None, 7, tmp), Path::new("/tmp/leon-ssh-7"));
    }

    #[cfg(unix)]
    #[test]
    fn the_connection_directory_is_created_for_the_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("leon-ssh");
        ensure_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_open_directory_is_closed_and_a_file_or_a_link_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let open = base.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&open).unwrap();
        assert_eq!(
            std::fs::metadata(&open).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let file = base.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(ensure_private_dir(&file).is_err());

        let link = base.path().join("link");
        std::os::unix::fs::symlink(&open, &link).unwrap();
        assert!(ensure_private_dir(&link).is_err(), "a link is not trusted");
    }

    /// The product's name as it is written for people: `Leon`.
    #[test]
    fn the_names_the_interface_shows_are_made_from_the_product_name() {
        for (shown, verb) in [
            (crate::keys::label(crate::keys::Command::Quit), "Quit"),
            (crate::keys::label(crate::keys::Command::About), "About"),
        ] {
            assert_eq!(shown, format!("{verb} {PRODUCT_NAME}"));
        }
        assert_eq!(PRODUCT_NAME, "Leon");
        let menus = crate::menus::LAYOUT;
        assert_eq!(menus[0].0, PRODUCT_NAME, "the application menu's title");
        let (_, _, text) = (0, 0, crate::cli::usage());
        assert!(
            text.contains(&format!("{PRODUCT_NAME} {VERSION}")),
            "{text}"
        );
    }

    /// No user-visible string spells the product `LEON` or `leon`: the name is
    /// `Leon`, and a label in mono caps is for categories (`[ SESSION ]`), not
    /// for the name. Identifiers that must be lower case are listed.
    #[test]
    fn no_interface_text_spells_the_product_name_in_capitals_or_lower_case() {
        // Where lower case is the identifier, not the name.
        const ALLOWED: [&str; 15] = [
            "\"leon\"",
            "LEON-ICON",
            "leon --",
            "leon=info",
            "leon diagnose",
            "leon.db",
            "dev.zavu.leon",
            "leon-",
            "leon_",
            "SLUG",
            "\"--theme-name\"",
            "theme: leon",
            "id = \\\"leon",
            "`leon`",
            "leon host",
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        let mut found = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if !name.ends_with(".rs") || name.starts_with("tests") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                let code = source.split("#[cfg(test)]").next().unwrap_or("");
                for (number, line) in code.lines().enumerate() {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("//") || !line.contains('"') {
                        continue;
                    }
                    let mut rest = line;
                    while let Some(at) = rest.find('"') {
                        let after = &rest[at + 1..];
                        let Some(end) = after.find('"') else { break };
                        let literal = &after[..end];
                        rest = &after[end + 1..];
                        let words = literal
                            .split(|c: char| {
                                !c.is_alphanumeric() && c != '-' && c != '_' && c != '.'
                            })
                            .any(|word| word == "LEON" || word == "leon");
                        let quoted = format!("\"{literal}\"");
                        let allowed = ALLOWED
                            .iter()
                            .any(|ok| quoted == *ok || line.contains(ok) && literal != "LEON");
                        if words && !allowed {
                            found.push(format!("{name}:{}: {literal}", number + 1));
                        }
                    }
                }
            }
        }
        assert!(found.is_empty(), "the product is `Leon`: {found:#?}");
    }
}

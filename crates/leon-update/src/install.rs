//! Where Leon is installed and what may be done about it.
//!
//! The unit that an update replaces is one thing: the `Leon.app` folder on
//! macOS, the program file on Linux and Windows. Replacing it is two renames
//! on the same volume: the running one aside (it is kept beside it, hidden, as
//! `.Leon.app.leon-old`), the new one, copied next to it first as
//! `.Leon.app.leon-new`, into its place. There is no moment when half of one
//! and half of the other is there; if the second rename fails the first is
//! undone, and the old one stays until the new one has been seen to start.
//! That works for a running program on all three systems: macOS and Linux
//! keep the open file, and Windows lets a running executable be renamed (it
//! cannot be overwritten or deleted, which is why the old one is only cleaned
//! at a later start).
//!
//! Some installs are not Leon's to change: a build under `target/`, an
//! application translocated by Gatekeeper, one running from a disk image, a
//! read-only folder, a package manager's or a sandbox's. For those no update
//! is installed; the person is sent to the download page. Nothing here ever
//! asks for more rights than the user has.

use crate::package::Kind;
use crate::release::Os;
use crate::tools::Tools;
use std::path::{Path, PathBuf};

/// Why Leon does not update this install itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A development build, run from a `target/` folder.
    Development,
    /// Switched off with `LEON_NO_UPDATE`.
    Disabled,
    /// macOS runs the application from a randomised read-only copy because
    /// it was never moved out of the folder it was downloaded to.
    Translocated,
    /// Run from a mounted disk image.
    DiskImage,
    /// The folder it is in cannot be written by this user.
    NotWritable,
    /// Installed by a package manager (`/usr`, Nix, a distribution).
    PackageManager,
    /// A sandbox or a bundle format with its own updates (AppImage, Flatpak,
    /// Snap, a Windows Store package).
    Packaged,
    /// The executable is not where a release puts it.
    Unknown,
}

impl Why {
    /// A sentence for the person.
    pub fn explain(self) -> &'static str {
        match self {
            Why::Development => "This is a development build, which is not updated.",
            Why::Disabled => "Updates are switched off (LEON_NO_UPDATE).",
            Why::Translocated => {
                "macOS is running Leon from a temporary copy. Move Leon to the Applications folder and open it from there."
            }
            Why::DiskImage => {
                "Leon is running from a disk image. Drag it to the Applications folder first."
            }
            Why::NotWritable => {
                "Leon is in a folder this user cannot change, so it cannot update itself there."
            }
            Why::PackageManager => {
                "Leon was installed by a package manager, which is what updates it."
            }
            Why::Packaged => "Leon runs from a package that has its own updates.",
            Why::Unknown => "Leon is not installed where a release puts it.",
        }
    }
}

/// What can be done about an install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// Leon replaces it itself.
    Updatable(Target),
    /// Leon does not; the person downloads the new one.
    Manual(Why),
}

impl Install {
    /// The target, when Leon may replace it.
    pub fn target(&self) -> Option<&Target> {
        match self {
            Install::Updatable(target) => Some(target),
            Install::Manual(_) => None,
        }
    }
}

/// What an install detection looks at, so that every system's rules run on
/// any computer.
pub struct Env<'a> {
    /// The system the paths are written for.
    pub os: Os,
    /// The running executable, as a string.
    pub exe: &'a str,
    /// An environment variable.
    pub var: &'a dyn Fn(&str) -> Option<String>,
    /// Whether this user can create files in a folder.
    pub writable: &'a dyn Fn(&Path) -> bool,
    /// The text of a file, when it can be read.
    pub read: &'a dyn Fn(&Path) -> Option<String>,
}

fn parts(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect()
}

/// A `target/debug`, `target/release` or `target/<triple>/release` folder.
fn is_cargo_target(parts: &[&str]) -> bool {
    parts.iter().enumerate().any(|(at, part)| {
        *part == "target"
            && parts[at + 1..]
                .iter()
                .take(2)
                .any(|next| matches!(*next, "debug" | "release"))
    })
}

/// The install that the executable at `env.exe` is.
pub fn detect_in(env: &Env<'_>) -> Install {
    if (env.var)("LEON_NO_UPDATE").is_some_and(|value| !value.is_empty()) {
        return Install::Manual(Why::Disabled);
    }
    let parts = parts(env.exe);
    if is_cargo_target(&parts) {
        return Install::Manual(Why::Development);
    }
    match env.os {
        Os::Macos => detect_macos(env, &parts),
        Os::Windows => {
            if parts
                .iter()
                .any(|part| part.eq_ignore_ascii_case("WindowsApps"))
            {
                return Install::Manual(Why::Packaged);
            }
            detect_file(env, Kind::File)
        }
        Os::Linux => {
            if (env.var)("APPIMAGE").is_some_and(|value| !value.is_empty()) {
                return Install::Manual(Why::Packaged);
            }
            let first = parts.first().copied().unwrap_or("");
            let second = parts.get(1).copied().unwrap_or("");
            if first == "snap"
                || first == "app"
                || (first == "nix" && second == "store")
                || (first == "var" && second == "lib" && parts.get(2) == Some(&"flatpak"))
            {
                return Install::Manual(Why::Packaged);
            }
            if matches!(first, "bin" | "sbin" | "lib" | "lib64")
                || (first == "usr" && second != "local")
            {
                return Install::Manual(Why::PackageManager);
            }
            detect_file(env, Kind::File)
        }
    }
}

fn detect_file(env: &Env<'_>, kind: Kind) -> Install {
    let exe = PathBuf::from(env.exe);
    let Some(dir) = exe.parent() else {
        return Install::Manual(Why::Unknown);
    };
    if !(env.writable)(dir) {
        return Install::Manual(Why::NotWritable);
    }
    Install::Updatable(Target { path: exe, kind })
}

fn detect_macos(env: &Env<'_>, parts: &[&str]) -> Install {
    // …/<Name>.app/Contents/MacOS/<exe>
    let n = parts.len();
    if n < 4
        || parts[n - 2] != "MacOS"
        || parts[n - 3] != "Contents"
        || !parts[n - 4].ends_with(".app")
    {
        return Install::Manual(Why::Unknown);
    }
    if parts.contains(&"AppTranslocation") {
        return Install::Manual(Why::Translocated);
    }
    let exe = PathBuf::from(env.exe);
    let Some(bundle) = exe.ancestors().nth(3).map(Path::to_path_buf) else {
        return Install::Manual(Why::Unknown);
    };
    let plist = (env.read)(&bundle.join("Contents/Info.plist")).unwrap_or_default();
    if plist.contains("LeonDevelopmentBuild") {
        return Install::Manual(Why::Development);
    }
    let Some(parent) = bundle.parent() else {
        return Install::Manual(Why::Unknown);
    };
    if !(env.writable)(parent) {
        return if parts.first() == Some(&"Volumes") {
            Install::Manual(Why::DiskImage)
        } else {
            Install::Manual(Why::NotWritable)
        };
    }
    Install::Updatable(Target {
        path: bundle,
        kind: Kind::Bundle,
    })
}

/// Whether this user can create a file in `dir`.
pub fn writable_dir(dir: &Path) -> bool {
    let probe = dir.join(format!(".leon-write-test-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The install of the running program.
pub fn detect() -> Install {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe.to_string_lossy().into_owned(),
        Err(_) => return Install::Manual(Why::Unknown),
    };
    let os = crate::release::Platform::current().os;
    detect_in(&Env {
        os,
        exe: &exe,
        var: &|name| std::env::var(name).ok(),
        writable: &writable_dir,
        read: &|path| std::fs::read_to_string(path).ok(),
    })
}

/// What is replaced: a bundle or a program file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The `Leon.app` folder, or the program file.
    pub path: PathBuf,
    /// Which of the two.
    pub kind: Kind,
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> std::io::Result<()> {
    Ok(())
}

impl Target {
    fn sibling(&self, suffix: &str) -> PathBuf {
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.path.with_file_name(format!(".{name}.leon-{suffix}"))
    }

    /// The program to run: inside the bundle, or the file.
    pub fn executable(&self) -> PathBuf {
        Self::executable_in(&self.path, self.kind)
    }

    fn executable_in(path: &Path, kind: Kind) -> PathBuf {
        match kind {
            Kind::Bundle => path.join("Contents/MacOS/Leon"),
            Kind::File => path.to_path_buf(),
        }
    }

    /// Where the new one is copied to before it takes the place.
    pub fn staging(&self) -> PathBuf {
        self.sibling("new")
    }

    /// Where the one that was running waits.
    pub fn backup(&self) -> PathBuf {
        self.sibling("old")
    }

    /// Where a new one that did not start is moved to.
    pub fn failed(&self) -> PathBuf {
        self.sibling("failed")
    }

    /// Whether the previous version is kept.
    pub fn has_backup(&self) -> bool {
        std::fs::symlink_metadata(self.backup()).is_ok()
    }

    /// Puts the program at `payload` in place of the installed one, keeping the
    /// installed one as the backup. On failure nothing has changed.
    pub fn replace_with(&self, payload: &Path, tools: &dyn Tools) -> std::io::Result<()> {
        let staging = self.staging();
        remove_any(&staging)?;
        let staged = (|| {
            match self.kind {
                Kind::Bundle => tools.copy_bundle(payload, &staging)?,
                Kind::File => {
                    std::fs::copy(payload, &staging)?;
                    make_executable(&staging)?;
                }
            }
            if Self::executable_in(&staging, self.kind).is_file() {
                Ok(())
            } else {
                Err(std::io::Error::other("the copy has no program in it"))
            }
        })();
        if let Err(error) = staged {
            let _ = remove_any(&staging);
            return Err(error);
        }
        let backup = self.backup();
        let swapped = (|| {
            // Whatever an earlier update left is no longer wanted.
            remove_any(&backup)?;
            std::fs::rename(&self.path, &backup)?;
            if let Err(error) = std::fs::rename(&staging, &self.path) {
                let _ = std::fs::rename(&backup, &self.path);
                return Err(error);
            }
            Ok(())
        })();
        if swapped.is_err() {
            let _ = remove_any(&staging);
        }
        swapped
    }

    /// Puts the backup back, the installed one (which did not work) moved
    /// out of the way and removed.
    pub fn restore(&self) -> std::io::Result<()> {
        let backup = self.backup();
        if std::fs::symlink_metadata(&backup).is_err() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no previous version is kept",
            ));
        }
        let failed = self.failed();
        remove_any(&failed)?;
        let moved = std::fs::symlink_metadata(&self.path).is_ok();
        if moved {
            std::fs::rename(&self.path, &failed)?;
        }
        if let Err(error) = std::fs::rename(&backup, &self.path) {
            if moved {
                let _ = std::fs::rename(&failed, &self.path);
            }
            return Err(error);
        }
        let _ = remove_any(&failed);
        Ok(())
    }

    /// Removes what an update leaves beside the program: the old version, a
    /// half-copied new one, a failed one. `false` while something could not be
    /// removed (a file Windows still holds); it is tried again at a later start.
    pub fn clean(&self) -> bool {
        let mut clean = true;
        for path in [self.backup(), self.staging(), self.failed()] {
            clean &= remove_any(&path).is_ok();
        }
        clean
    }

    /// Whether anything of an update is left beside the program.
    pub fn has_leftovers(&self) -> bool {
        [self.backup(), self.staging(), self.failed()]
            .iter()
            .any(|path| std::fs::symlink_metadata(path).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::FakeTools;

    fn detect_for(
        os: Os,
        exe: &str,
        vars: &[(&str, &str)],
        writable: bool,
        plist: &str,
    ) -> Install {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        detect_in(&Env {
            os,
            exe,
            var: &move |name| {
                vars.iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.clone())
            },
            writable: &move |_| writable,
            read: &|_| Some(plist.to_owned()),
        })
    }

    const APP: &str = "/Applications/Leon.app/Contents/MacOS/Leon";

    #[test]
    fn a_bundle_in_applications_is_updated_in_place() {
        let install = detect_for(Os::Macos, APP, &[], true, "<plist/>");
        assert_eq!(
            install,
            Install::Updatable(Target {
                path: PathBuf::from("/Applications/Leon.app"),
                kind: Kind::Bundle
            })
        );
        assert_eq!(install.target().unwrap().executable(), PathBuf::from(APP));
    }

    #[test]
    fn the_macos_installs_that_are_not_ours_to_change_say_why() {
        let manual = |exe: &str, writable: bool, plist: &str| {
            detect_for(Os::Macos, exe, &[], writable, plist)
        };
        assert_eq!(
            manual(
                "/Users/v/Code/leon/target/debug/Leon.app/Contents/MacOS/Leon",
                true,
                ""
            ),
            Install::Manual(Why::Development)
        );
        assert_eq!(
            manual("/Users/v/Code/leon/target/debug/leon", true, ""),
            Install::Manual(Why::Development)
        );
        assert_eq!(
            manual(APP, true, "<key>LeonDevelopmentBuild</key><true/>"),
            Install::Manual(Why::Development)
        );
        assert_eq!(
            manual(
                "/private/var/folders/x/AppTranslocation/ABC/d/Leon.app/Contents/MacOS/Leon",
                false,
                ""
            ),
            Install::Manual(Why::Translocated)
        );
        assert_eq!(
            manual("/Volumes/Leon/Leon.app/Contents/MacOS/Leon", false, ""),
            Install::Manual(Why::DiskImage)
        );
        assert_eq!(manual(APP, false, ""), Install::Manual(Why::NotWritable));
        assert_eq!(
            manual("/usr/local/bin/leon", true, ""),
            Install::Manual(Why::Unknown)
        );
        // A writable external drive is a fine place.
        assert!(matches!(
            manual("/Volumes/USB/Leon.app/Contents/MacOS/Leon", true, ""),
            Install::Updatable(_)
        ));
    }

    #[test]
    fn linux_updates_a_tarball_install_and_leaves_packages_alone() {
        let home = detect_for(Os::Linux, "/home/v/leon/leon", &[], true, "");
        assert_eq!(
            home,
            Install::Updatable(Target {
                path: PathBuf::from("/home/v/leon/leon"),
                kind: Kind::File
            })
        );
        assert!(matches!(
            detect_for(Os::Linux, "/usr/local/bin/leon", &[], true, ""),
            Install::Updatable(_)
        ));
        for (exe, why) in [
            ("/usr/bin/leon", Why::PackageManager),
            ("/usr/lib/leon/leon", Why::PackageManager),
            ("/bin/leon", Why::PackageManager),
            ("/nix/store/abc-leon/bin/leon", Why::Packaged),
            ("/snap/leon/1/leon", Why::Packaged),
            ("/app/bin/leon", Why::Packaged),
            ("/var/lib/flatpak/app/dev.zavu.leon/x/leon", Why::Packaged),
        ] {
            assert_eq!(
                detect_for(Os::Linux, exe, &[], true, ""),
                Install::Manual(why),
                "{exe}"
            );
        }
        assert_eq!(
            detect_for(
                Os::Linux,
                "/tmp/.mount_Leon/leon",
                &[("APPIMAGE", "/x/Leon.AppImage")],
                true,
                ""
            ),
            Install::Manual(Why::Packaged)
        );
        assert_eq!(
            detect_for(Os::Linux, "/opt/leon/leon", &[], false, ""),
            Install::Manual(Why::NotWritable)
        );
        assert_eq!(
            detect_for(
                Os::Linux,
                "/home/v/Code/leon/target/release/leon",
                &[],
                true,
                ""
            ),
            Install::Manual(Why::Development)
        );
    }

    #[test]
    fn windows_paths_are_read_with_backslashes() {
        let ok = detect_for(
            Os::Windows,
            r"C:\Users\v\AppData\Local\Leon\leon.exe",
            &[],
            true,
            "",
        );
        assert_eq!(ok.target().map(|target| target.kind), Some(Kind::File));
        assert_eq!(
            detect_for(
                Os::Windows,
                r"C:\Program Files\Leon\leon.exe",
                &[],
                false,
                ""
            ),
            Install::Manual(Why::NotWritable)
        );
        assert_eq!(
            detect_for(
                Os::Windows,
                r"C:\Program Files\WindowsApps\Zavu.Leon_1\leon.exe",
                &[],
                true,
                ""
            ),
            Install::Manual(Why::Packaged)
        );
        assert_eq!(
            detect_for(
                Os::Windows,
                r"C:\src\leon\target\debug\leon.exe",
                &[],
                true,
                ""
            ),
            Install::Manual(Why::Development)
        );
    }

    #[test]
    fn the_environment_switches_updates_off() {
        for os in [Os::Macos, Os::Linux, Os::Windows] {
            assert_eq!(
                detect_for(os, APP, &[("LEON_NO_UPDATE", "1")], true, ""),
                Install::Manual(Why::Disabled)
            );
            assert!(
                detect_for(os, APP, &[("LEON_NO_UPDATE", "")], true, "")
                    .target()
                    .is_some()
                    || os != Os::Macos
            );
        }
    }

    #[test]
    fn a_target_dir_that_is_not_cargos_is_not_a_development_build() {
        let install = detect_for(Os::Linux, "/home/v/target/leon", &[], true, "");
        assert!(install.target().is_some());
    }

    // ----- the swap, on temporary folders ---------------------------------------------

    fn file_target(dir: &Path, old: &str) -> Target {
        let path = dir.join("leon");
        std::fs::write(&path, old).unwrap();
        Target {
            path,
            kind: Kind::File,
        }
    }

    fn payload(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("payload");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_program_is_replaced_and_the_old_one_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let target = file_target(dir.path(), "old");
        let new = payload(dir.path(), "new");
        target.replace_with(&new, &FakeTools::new("1")).unwrap();
        assert_eq!(std::fs::read_to_string(&target.path).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(target.backup()).unwrap(), "old");
        assert!(!target.staging().exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&target.path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0o111
            );
        }
    }

    #[test]
    fn restoring_puts_the_old_one_back_and_drops_the_new() {
        let dir = tempfile::tempdir().unwrap();
        let target = file_target(dir.path(), "old");
        target
            .replace_with(&payload(dir.path(), "new"), &FakeTools::new("1"))
            .unwrap();
        target.restore().unwrap();
        assert_eq!(std::fs::read_to_string(&target.path).unwrap(), "old");
        assert!(!target.has_backup());
        assert!(!target.failed().exists());
        assert!(!target.has_leftovers());
    }

    #[test]
    fn restoring_with_nothing_kept_is_an_error_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let target = file_target(dir.path(), "current");
        assert!(target.restore().is_err());
        assert_eq!(std::fs::read_to_string(&target.path).unwrap(), "current");
    }

    #[test]
    fn cleaning_removes_the_backup_and_what_else_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let target = file_target(dir.path(), "old");
        target
            .replace_with(&payload(dir.path(), "new"), &FakeTools::new("1"))
            .unwrap();
        std::fs::write(target.staging(), "half").unwrap();
        std::fs::write(target.failed(), "x").unwrap();
        assert!(target.has_leftovers());
        assert!(target.clean());
        assert!(!target.has_leftovers());
        assert_eq!(std::fs::read_to_string(&target.path).unwrap(), "new");
    }

    #[test]
    fn an_update_over_a_stale_backup_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = file_target(dir.path(), "v1");
        std::fs::write(target.backup(), "v0 left from before").unwrap();
        target
            .replace_with(&payload(dir.path(), "v2"), &FakeTools::new("1"))
            .unwrap();
        assert_eq!(std::fs::read_to_string(target.backup()).unwrap(), "v1");
    }

    #[test]
    fn a_failed_copy_leaves_everything_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let target = file_target(dir.path(), "old");
        let missing = dir.path().join("no-such-payload");
        assert!(target.replace_with(&missing, &FakeTools::new("1")).is_err());
        assert_eq!(std::fs::read_to_string(&target.path).unwrap(), "old");
        assert!(!target.has_leftovers());
    }

    fn bundle(path: &Path, program: &str) {
        std::fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
        std::fs::write(path.join("Contents/MacOS/Leon"), program).unwrap();
        std::fs::write(path.join("Contents/Info.plist"), "<plist/>").unwrap();
    }

    #[test]
    fn a_bundle_is_swapped_whole_and_rolled_back_whole() {
        let dir = tempfile::tempdir().unwrap();
        let target = Target {
            path: dir.path().join("Leon.app"),
            kind: Kind::Bundle,
        };
        bundle(&target.path, "old");
        let new = dir.path().join("payload/Leon.app");
        bundle(&new, "new");
        std::fs::write(new.join("Contents/extra"), "x").unwrap();
        target.replace_with(&new, &FakeTools::new("1")).unwrap();
        assert_eq!(std::fs::read_to_string(target.executable()).unwrap(), "new");
        assert!(target.path.join("Contents/extra").exists());
        assert_eq!(
            std::fs::read_to_string(target.backup().join("Contents/MacOS/Leon")).unwrap(),
            "old"
        );
        target.restore().unwrap();
        assert_eq!(std::fs::read_to_string(target.executable()).unwrap(), "old");
        assert!(!target.path.join("Contents/extra").exists());
        assert!(!target.has_leftovers());
    }

    #[test]
    fn a_bundle_without_a_program_is_not_put_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let target = Target {
            path: dir.path().join("Leon.app"),
            kind: Kind::Bundle,
        };
        bundle(&target.path, "old");
        let hollow = dir.path().join("hollow/Leon.app");
        std::fs::create_dir_all(hollow.join("Contents")).unwrap();
        assert!(target.replace_with(&hollow, &FakeTools::new("1")).is_err());
        assert_eq!(std::fs::read_to_string(target.executable()).unwrap(), "old");
        assert!(!target.has_leftovers());
    }

    #[test]
    fn the_names_beside_the_program_are_hidden_and_stable() {
        let target = Target {
            path: PathBuf::from("/Applications/Leon.app"),
            kind: Kind::Bundle,
        };
        assert_eq!(
            target.backup(),
            PathBuf::from("/Applications/.Leon.app.leon-old")
        );
        assert_eq!(
            target.staging(),
            PathBuf::from("/Applications/.Leon.app.leon-new")
        );
        assert_eq!(
            target.failed(),
            PathBuf::from("/Applications/.Leon.app.leon-failed")
        );
    }

    #[test]
    fn a_folder_is_writable_when_a_file_can_be_made_in_it() {
        let dir = tempfile::tempdir().unwrap();
        assert!(writable_dir(dir.path()));
        assert!(!writable_dir(&dir.path().join("missing")));
    }
}

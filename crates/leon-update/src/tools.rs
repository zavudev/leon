//! What the system is asked to do besides talk: mount a disk image, copy an
//! application bundle, read a code signature, start the new build to see that
//! it runs.
//!
//! All behind [`Tools`], so that every other part is tested on temporary
//! folders on any computer, and the real tools (`hdiutil`, `ditto`,
//! `codesign` on macOS; PowerShell's `Get-AuthenticodeSignature` on Windows)
//! are run only by [`SystemTools`]. Every process is started through
//! `leon_remote::spawn`, so none opens a console window on Windows.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Who signed a program, and whether the signature holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signature {
    /// The signer: the Team ID on macOS, the certificate's subject on
    /// Windows. `None` for a program that is not signed by anybody (an ad-hoc
    /// signature is none).
    pub identity: Option<String>,
    /// Whether the signature is whole: the seal of a bundle, a valid
    /// Authenticode chain.
    pub valid: bool,
}

impl Signature {
    /// Not signed by anybody.
    pub fn none() -> Self {
        Self::default()
    }

    /// Signed by `identity`, and whole.
    pub fn good(identity: &str) -> Self {
        Self {
            identity: Some(identity.to_owned()),
            valid: true,
        }
    }
}

/// The system's help.
pub trait Tools: Send + Sync {
    /// macOS: opens the disk image read-only without showing it, copies
    /// `Leon.app` out of it into `into`, and closes it again, whatever
    /// happened. Gives the path of the copy.
    fn extract_bundle(&self, image: &Path, into: &Path) -> std::io::Result<PathBuf>;

    /// Copies an application bundle with everything it carries: links,
    /// modes, extended attributes (the code signature's seal among them).
    fn copy_bundle(&self, from: &Path, to: &Path) -> std::io::Result<()>;

    /// The signature of a bundle (macOS) or a program (Windows).
    fn signature(&self, path: &Path) -> Result<Signature, String>;

    /// Runs the program with `--version` and gives what it printed: a build
    /// that does not even do that is not put in place.
    fn probe(&self, program: &Path) -> Result<String, String>;
}

/// The real tools.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemTools;

/// Copies a folder and everything in it, links kept as links where the system
/// has them. What [`SystemTools::copy_bundle`] does off macOS, and what the
/// tests use for a bundle.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        {
            let target = std::fs::read_link(from)?;
            return std::os::unix::fs::symlink(target, to);
        }
        #[cfg(not(unix))]
        {
            return std::fs::copy(from, to).map(|_| ());
        }
    }
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        std::fs::set_permissions(to, meta.permissions())?;
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// The `TeamIdentifier` that `codesign -dv` prints, when there is one.
pub fn parse_team_identifier(display: &str) -> Option<String> {
    display.lines().find_map(|line| {
        let team = line.trim().strip_prefix("TeamIdentifier=")?.trim();
        (!team.is_empty() && team != "not set").then(|| team.to_owned())
    })
}

/// What the PowerShell snippet of [`SystemTools::signature`] printed on Windows.
pub fn parse_authenticode(output: &str) -> Signature {
    let mut status = "";
    let mut subject = "";
    for line in output.lines() {
        if let Some(value) = line.trim().strip_prefix("STATUS=") {
            status = value.trim();
        } else if let Some(value) = line.trim().strip_prefix("SUBJECT=") {
            subject = value.trim();
        }
    }
    Signature {
        identity: (!subject.is_empty() && status != "NotSigned").then(|| subject.to_owned()),
        valid: status == "Valid",
    }
}

/// How long a build may take to print its version.
const PROBE_TIME: Duration = Duration::from_secs(15);

fn run(mut command: std::process::Command) -> std::io::Result<std::process::Output> {
    command.stdin(std::process::Stdio::null());
    command.output()
}

#[cfg(target_os = "macos")]
fn mac_extract_bundle(image: &Path, into: &Path) -> std::io::Result<PathBuf> {
    let mount = into.join("mount");
    std::fs::create_dir_all(&mount)?;
    let mut attach = leon_remote::spawn::std_child("hdiutil");
    attach
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noverify",
            "-noautoopen",
        ])
        .arg("-mountpoint")
        .arg(&mount)
        .arg(image);
    let attached = run(attach)?;
    if !attached.status.success() {
        return Err(std::io::Error::other(format!(
            "hdiutil attach: {}",
            String::from_utf8_lossy(&attached.stderr).trim()
        )));
    }
    let copied = (|| {
        let source = mount.join("Leon.app");
        if !source.is_dir() {
            return Err(std::io::Error::other("the disk image has no Leon.app"));
        }
        let to = into.join("Leon.app");
        if to.exists() {
            std::fs::remove_dir_all(&to)?;
        }
        let mut ditto = leon_remote::spawn::std_child("ditto");
        ditto.arg(&source).arg(&to);
        let copied = run(ditto)?;
        if copied.status.success() {
            Ok(to)
        } else {
            Err(std::io::Error::other(format!(
                "ditto: {}",
                String::from_utf8_lossy(&copied.stderr).trim()
            )))
        }
    })();
    // Always let go of the image, a forced detach when it is busy.
    for force in [false, true] {
        let mut detach = leon_remote::spawn::std_child("hdiutil");
        detach.arg("detach").arg(&mount);
        if force {
            detach.arg("-force");
        }
        if run(detach).is_ok_and(|out| out.status.success()) {
            break;
        }
    }
    let _ = std::fs::remove_dir(&mount);
    copied
}

impl Tools for SystemTools {
    fn extract_bundle(&self, image: &Path, into: &Path) -> std::io::Result<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            mac_extract_bundle(image, into)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (image, into);
            Err(std::io::Error::other("disk images are a macOS format"))
        }
    }

    fn copy_bundle(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            let mut ditto = leon_remote::spawn::std_child("ditto");
            ditto.arg(from).arg(to);
            let copied = run(ditto)?;
            if copied.status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "ditto: {}",
                    String::from_utf8_lossy(&copied.stderr).trim()
                )))
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            copy_tree(from, to)
        }
    }

    fn signature(&self, path: &Path) -> Result<Signature, String> {
        #[cfg(target_os = "macos")]
        {
            let mut display = leon_remote::spawn::std_child("codesign");
            display.args(["-dv", "--verbose=2"]).arg(path);
            let shown = run(display).map_err(|error| format!("codesign: {error}"))?;
            let text = String::from_utf8_lossy(&shown.stderr).into_owned();
            let identity = parse_team_identifier(&text);
            let mut verify = leon_remote::spawn::std_child("codesign");
            verify.args(["--verify", "--deep", "--strict"]).arg(path);
            let valid = run(verify)
                .map(|out| out.status.success())
                .map_err(|error| format!("codesign: {error}"))?;
            Ok(Signature { identity, valid })
        }
        #[cfg(windows)]
        {
            let mut command = leon_remote::spawn::std_child("powershell");
            command
                .args(["-NoProfile", "-NonInteractive", "-Command"])
                .arg(
                    "$s = Get-AuthenticodeSignature -LiteralPath $env:LEON_CHECK_PATH; \
                     'STATUS=' + $s.Status; 'SUBJECT=' + $s.SignerCertificate.Subject",
                )
                .env("LEON_CHECK_PATH", path);
            let out = run(command).map_err(|error| format!("powershell: {error}"))?;
            Ok(parse_authenticode(&String::from_utf8_lossy(&out.stdout)))
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = path;
            Ok(Signature::none())
        }
    }

    fn probe(&self, program: &Path) -> Result<String, String> {
        let mut command = leon_remote::spawn::std_child(program);
        command
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start it: {error}"))?;
        let deadline = Instant::now() + PROBE_TIME;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut text = String::new();
                    if let Some(mut out) = child.stdout.take() {
                        use std::io::Read as _;
                        let _ = out.read_to_string(&mut text);
                    }
                    return if status.success() {
                        Ok(text.trim().to_owned())
                    } else {
                        Err(format!("it exited with {status}"))
                    };
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("it did not answer".into());
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

/// Tools that touch nothing real, for tests: a bundle is copied as a folder,
/// signatures and the probe are whatever the test sets.
#[cfg(any(test, feature = "test-support"))]
pub struct FakeTools {
    /// What `signature` answers for a path that has a file `signed-by` inside
    /// (a bundle) or is a file that holds that text; otherwise unsigned.
    pub unsigned_everywhere: bool,
    /// What the probe answers: the version text, or why it fails.
    pub probe_answer: std::sync::Mutex<Result<String, String>>,
    /// The programs probed so far.
    pub probed: std::sync::Mutex<Vec<PathBuf>>,
    /// The folder that stands for the disk image, whatever file is asked
    /// about (a download is a file, an image is a folder).
    pub image: Option<PathBuf>,
}

#[cfg(any(test, feature = "test-support"))]
impl FakeTools {
    /// Tools whose probe says `version` and whose signatures are read from the
    /// files (see [`FakeTools::sign`]).
    pub fn new(version: &str) -> Self {
        Self {
            unsigned_everywhere: false,
            probe_answer: std::sync::Mutex::new(Ok(format!("Leon {version}"))),
            probed: std::sync::Mutex::new(Vec::new()),
            image: None,
        }
    }

    /// The folder that is the disk image of every archive.
    pub fn with_image(mut self, image: &Path) -> Self {
        self.image = Some(image.to_owned());
        self
    }

    /// Makes the probe fail.
    pub fn failing(self, why: &str) -> Self {
        *self.probe_answer.lock().unwrap() = Err(why.to_owned());
        self
    }

    /// Marks a payload as signed by `identity`: a bundle holds a file
    /// `signed-by`, a program is a file whose text starts `signed-by:`.
    pub fn sign(path: &Path, identity: &str) {
        if path.is_dir() {
            std::fs::write(path.join("signed-by"), identity).unwrap();
        } else {
            let mut text = format!("signed-by:{identity}\n").into_bytes();
            text.extend(std::fs::read(path).unwrap_or_default());
            std::fs::write(path, text).unwrap();
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Tools for FakeTools {
    fn extract_bundle(&self, image: &Path, into: &Path) -> std::io::Result<PathBuf> {
        // A "disk image" in tests is a folder that holds Leon.app.
        let to = into.join("Leon.app");
        let image = self.image.as_deref().unwrap_or(image);
        copy_tree(&image.join("Leon.app"), &to)?;
        Ok(to)
    }

    fn copy_bundle(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        copy_tree(from, to)
    }

    fn signature(&self, path: &Path) -> Result<Signature, String> {
        if self.unsigned_everywhere {
            return Ok(Signature::none());
        }
        let text = if path.is_dir() {
            std::fs::read_to_string(path.join("signed-by")).unwrap_or_default()
        } else {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|text| {
                    text.lines()
                        .next()
                        .and_then(|line| line.strip_prefix("signed-by:"))
                        .map(str::to_owned)
                })
                .unwrap_or_default()
        };
        Ok(match text.trim() {
            "" => Signature::none(),
            "broken" => Signature {
                identity: Some("TEAM1".into()),
                valid: false,
            },
            identity => Signature::good(identity),
        })
    }

    fn probe(&self, program: &Path) -> Result<String, String> {
        self.probed.lock().unwrap().push(program.to_owned());
        self.probe_answer.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_team_is_read_from_the_codesign_display() {
        let signed = "Executable=/A/Leon.app/Contents/MacOS/Leon\nIdentifier=dev.zavu.leon\nAuthority=Developer ID Application: Zavu (ABCDE12345)\nTeamIdentifier=ABCDE12345\nSealed Resources version=2\n";
        assert_eq!(parse_team_identifier(signed), Some("ABCDE12345".into()));
        let adhoc = "Signature=adhoc\nTeamIdentifier=not set\n";
        assert_eq!(parse_team_identifier(adhoc), None);
        assert_eq!(
            parse_team_identifier("code object is not signed at all"),
            None
        );
    }

    #[test]
    fn authenticode_output_is_read() {
        let signed = parse_authenticode("STATUS=Valid\r\nSUBJECT=CN=Zavu, O=Zavu\r\n");
        assert_eq!(signed, Signature::good("CN=Zavu, O=Zavu"));
        let unsigned = parse_authenticode("STATUS=NotSigned\nSUBJECT=\n");
        assert_eq!(unsigned, Signature::none());
        let broken = parse_authenticode("STATUS=HashMismatch\nSUBJECT=CN=Zavu\n");
        assert_eq!(
            broken,
            Signature {
                identity: Some("CN=Zavu".into()),
                valid: false
            }
        );
        assert_eq!(parse_authenticode(""), Signature::none());
    }

    #[test]
    fn a_folder_is_copied_whole() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        std::fs::create_dir_all(from.join("a/b")).unwrap();
        std::fs::write(from.join("a/b/f"), "x").unwrap();
        std::fs::write(from.join("top"), "y").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("top", from.join("link")).unwrap();
        let to = dir.path().join("to");
        copy_tree(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(to.join("a/b/f")).unwrap(), "x");
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(to.join("link")).unwrap(),
            Path::new("top")
        );
    }

    /// Builds a disk image with `hdiutil`, as the release does, and takes the
    /// bundle out of it with the real tools: the one test that mounts
    /// something. It needs nothing but the system's own tools.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_real_disk_image_gives_up_its_bundle_and_an_unsigned_bundle_reads_as_unsigned() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        crate::package::fixtures::image(&source, "0.2.1", b"#!/bin/sh\necho Leon 0.2.1\n");
        let program = source.join("Leon.app/Contents/MacOS/Leon");
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::os::unix::fs::symlink("/Applications", source.join("Applications")).unwrap();
        let image = dir.path().join("leon.dmg");
        let made = std::process::Command::new("hdiutil")
            .args([
                "create",
                "-volname",
                "Leon",
                "-ov",
                "-format",
                "UDZO",
                "-srcfolder",
            ])
            .arg(&source)
            .arg(&image)
            .output();
        let Ok(made) = made else {
            eprintln!("skipped: hdiutil is not available");
            return;
        };
        if !made.status.success() {
            eprintln!("skipped: hdiutil could not make an image here");
            return;
        }
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let bundle = SystemTools.extract_bundle(&image, &out).unwrap();
        assert_eq!(bundle, out.join("Leon.app"));
        assert!(bundle.join("Contents/Info.plist").is_file());
        assert!(!out.join("mount").exists(), "the image is let go of");
        assert_eq!(
            SystemTools.probe(&bundle.join("Contents/MacOS/Leon")),
            Ok("Leon 0.2.1".into())
        );
        assert_eq!(SystemTools.signature(&bundle).unwrap().identity, None);
        // The bundle is copied the way the install copies it.
        let copy = dir.path().join("copy.app");
        SystemTools.copy_bundle(&bundle, &copy).unwrap();
        assert!(copy.join("Contents/MacOS/Leon").is_file());
        // And the whole of it through the package code.
        let mac = crate::release::Platform::parse("macos-aarch64").unwrap();
        let again = dir.path().join("again");
        let payload = crate::package::extract(
            &image,
            &mac,
            &crate::version::Version::new(0, 2, 1),
            &again,
            &SystemTools,
        )
        .unwrap();
        assert!(payload.join("Contents/MacOS/Leon").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn the_real_probe_reads_the_output_and_fails_for_a_program_that_fails() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good");
        let bad = dir.path().join("bad");
        for (path, body) in [
            (&good, "#!/bin/sh\necho Leon 9.9.9\n"),
            (&bad, "#!/bin/sh\nexit 3\n"),
        ] {
            std::fs::write(path, body).unwrap();
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(SystemTools.probe(&good), Ok("Leon 9.9.9".into()));
        assert!(SystemTools.probe(&bad).is_err());
        assert!(SystemTools.probe(&dir.path().join("missing")).is_err());
    }
}

//! Taking the program out of a release archive.
//!
//! The layout is the release's contract (`docs/RELEASING.md`):
//!
//! | Platform | Archive | The program |
//! | --- | --- | --- |
//! | macOS | `leon-<v>-macos-<arch>.dmg` | `Leon.app` at the top of the volume |
//! | Linux | `leon-<v>-linux-x86_64.tar.gz` | `leon-<v>-linux-x86_64/leon` |
//! | Windows | `leon-<v>-windows-x86_64.zip` | `leon-<v>-windows-x86_64/leon.exe` |
//!
//! Extraction is strict. A tar or a zip has to hold one top folder with the
//! archive's name and nothing outside it; an entry that climbs out of it,
//! is absolute or is a link refuses the whole archive; only the program is
//! written to disk, to the one place asked for, with a ceiling on its size.

use crate::release::{Os, Platform};
use crate::tools::Tools;
use crate::version::Version;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

/// The most a program may be once unpacked.
pub const MAX_PROGRAM_BYTES: u64 = 512 * 1024 * 1024;

/// The most entries an archive may hold.
const MAX_ENTRIES: usize = 20_000;

/// What the payload of a platform is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A folder: the macOS application bundle.
    Bundle,
    /// One file: the program.
    File,
}

/// The name of the payload inside the archive and on disk.
pub fn payload_name(platform: &Platform) -> &'static str {
    match platform.os {
        Os::Macos => "Leon.app",
        Os::Linux => "leon",
        Os::Windows => "leon.exe",
    }
}

/// Whether the payload is a folder or a file.
pub fn kind(platform: &Platform) -> Kind {
    match platform.os {
        Os::Macos => Kind::Bundle,
        _ => Kind::File,
    }
}

/// The folder of a tar or a zip: the archive's name without its extension.
pub fn root_name(platform: &Platform, version: &Version) -> String {
    format!("leon-{version}-{}", platform.id())
}

/// Why an archive was refused.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The archive is not laid out as a release's is.
    #[error("the archive is not laid out as a Leon release: {0}")]
    Layout(String),
    /// The archive could not be read or written.
    #[error("the archive could not be unpacked: {0}")]
    Io(String),
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io(error.to_string())
    }
}

fn layout(why: impl Into<String>) -> Error {
    Error::Layout(why.into())
}

/// The path inside the archive, as `/`-separated parts, when it is a plain
/// relative one.
fn plain_parts(path: &Path) -> Result<Vec<String>, Error> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => {
                return Err(layout(format!(
                    "{} leaves the archive's folder",
                    path.display()
                )))
            }
        }
    }
    Ok(parts)
}

fn check_root(parts: &[String], root: &str) -> Result<(), Error> {
    match parts.first() {
        Some(first) if first == root => Ok(()),
        Some(first) => Err(layout(format!(
            "it holds {first:?} outside the folder {root:?}"
        ))),
        None => Ok(()),
    }
}

/// Copies at most `MAX_PROGRAM_BYTES` of `from` to `to` and gives how many.
fn write_program(from: &mut impl std::io::Read, to: &Path) -> Result<u64, Error> {
    let mut file = std::fs::File::create(to)?;
    let mut limited = from.take(MAX_PROGRAM_BYTES + 1);
    let written = std::io::copy(&mut limited, &mut file)?;
    if written > MAX_PROGRAM_BYTES {
        drop(file);
        let _ = std::fs::remove_file(to);
        return Err(layout("the program is larger than any Leon release"));
    }
    if written == 0 {
        drop(file);
        let _ = std::fs::remove_file(to);
        return Err(layout("the program is empty"));
    }
    file.sync_all()?;
    Ok(written)
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

fn unpack_tar_gz(archive: &Path, root: &str, name: &str, to: &Path) -> Result<(), Error> {
    let file = std::fs::File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let wanted = format!("{root}/{name}");
    let mut found = false;
    for (count, entry) in tar.entries()?.enumerate() {
        if count >= MAX_ENTRIES {
            return Err(layout("it holds too many entries"));
        }
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let parts = plain_parts(&path)?;
        check_root(&parts, root)?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(layout(format!(
                "{} is neither a file nor a folder",
                path.display()
            )));
        }
        if kind.is_file() && parts.join("/") == wanted {
            if found {
                return Err(layout(format!("{name} is in it twice")));
            }
            write_program(&mut entry, to)?;
            found = true;
        }
    }
    if found {
        Ok(())
    } else {
        Err(layout(format!("{wanted} is not in it")))
    }
}

fn unpack_zip(archive: &Path, root: &str, name: &str, to: &Path) -> Result<(), Error> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|error| Error::Io(error.to_string()))?;
    if zip.len() > MAX_ENTRIES {
        return Err(layout("it holds too many entries"));
    }
    let wanted = format!("{root}/{name}");
    let mut found = false;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| Error::Io(error.to_string()))?;
        let path = entry
            .enclosed_name()
            .ok_or_else(|| layout(format!("{:?} leaves the archive's folder", entry.name())))?;
        let parts = plain_parts(&path)?;
        check_root(&parts, root)?;
        // A link is stored as a file with the link bit in its mode.
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(layout(format!("{} is a link", path.display())));
        }
        if entry.is_file() && parts.join("/") == wanted {
            if found {
                return Err(layout(format!("{name} is in it twice")));
            }
            write_program(&mut entry, to)?;
            found = true;
        }
    }
    if found {
        Ok(())
    } else {
        Err(layout(format!("{wanted} is not in it")))
    }
}

/// The `CFBundleShortVersionString` of an XML `Info.plist`.
pub fn bundle_version(plist: &str) -> Option<String> {
    let after = plist
        .split("<key>CFBundleShortVersionString</key>")
        .nth(1)?;
    let value = after.split("<string>").nth(1)?.split("</string>").next()?;
    Some(value.trim().to_owned())
}

fn check_bundle(bundle: &Path, version: &Version) -> Result<(), Error> {
    let executable = bundle.join("Contents/MacOS/Leon");
    if !executable.is_file() {
        return Err(layout("Leon.app has no Contents/MacOS/Leon"));
    }
    let plist = std::fs::read_to_string(bundle.join("Contents/Info.plist"))
        .map_err(|_| layout("Leon.app has no readable Contents/Info.plist"))?;
    if !plist.contains("dev.zavu.leon") {
        return Err(layout("Leon.app is not Leon's bundle"));
    }
    match bundle_version(&plist) {
        Some(found) if found == version.to_string() => Ok(()),
        Some(found) => Err(layout(format!(
            "Leon.app is version {found}, not {version}"
        ))),
        None => Ok(()),
    }
}

/// Takes the program of `version` for `platform` out of `archive` into the
/// folder `into` and gives its path there: `Leon.app` on macOS (through the
/// system's tools), `leon` or `leon.exe` elsewhere.
pub fn extract(
    archive: &Path,
    platform: &Platform,
    version: &Version,
    into: &Path,
    tools: &dyn Tools,
) -> Result<PathBuf, Error> {
    std::fs::create_dir_all(into)?;
    let name = payload_name(platform);
    let out = into.join(name);
    if out.exists() {
        if out.is_dir() {
            std::fs::remove_dir_all(&out)?;
        } else {
            std::fs::remove_file(&out)?;
        }
    }
    match platform.os {
        Os::Macos => {
            let bundle = tools.extract_bundle(archive, into)?;
            check_bundle(&bundle, version)?;
            Ok(bundle)
        }
        Os::Linux => {
            unpack_tar_gz(archive, &root_name(platform, version), name, &out)?;
            make_executable(&out)?;
            Ok(out)
        }
        Os::Windows => {
            unpack_zip(archive, &root_name(platform, version), name, &out)?;
            Ok(out)
        }
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Archives built the way `cargo xtask package` builds them.

    use super::*;
    use std::io::Write as _;

    /// A tar.gz of `entries` (path, bytes), directories written as paths
    /// ending in `/`.
    pub fn tar_gz(path: &Path, entries: &[(&str, &[u8])]) {
        let encoder = flate2::write::GzEncoder::new(
            std::fs::File::create(path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(encoder);
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            if name.ends_with('/') {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
                header.set_size(0);
            } else {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(0o755);
                header.set_size(bytes.len() as u64);
            }
            header.set_cksum();
            // `append_data` refuses a `..` path, so the path is written raw.
            let raw = header.as_mut_bytes();
            raw[..100].fill(0);
            raw[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            builder.append(&header, *bytes).unwrap();
        }
        builder
            .into_inner()
            .unwrap()
            .finish()
            .unwrap()
            .flush()
            .unwrap();
    }

    /// A zip of `entries`.
    pub fn zip(path: &Path, entries: &[(&str, &[u8])]) {
        use zip::write::SimpleFileOptions;
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, bytes) in entries {
            if name.ends_with('/') {
                writer
                    .add_directory(*name, SimpleFileOptions::default())
                    .unwrap();
            } else {
                writer
                    .start_file(*name, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
        }
        writer.finish().unwrap();
    }

    /// A folder that stands for a disk image: it holds `Leon.app`.
    pub fn image(path: &Path, version: &str, executable: &[u8]) {
        let bundle = path.join("Leon.app/Contents");
        std::fs::create_dir_all(bundle.join("MacOS")).unwrap();
        std::fs::write(bundle.join("MacOS/Leon"), executable).unwrap();
        std::fs::write(
            bundle.join("Info.plist"),
            format!(
                "<plist><dict><key>CFBundleIdentifier</key><string>dev.zavu.leon</string>\
                 <key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"
            ),
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::tools::FakeTools;

    fn v() -> Version {
        Version::new(0, 2, 1)
    }

    fn linux() -> Platform {
        Platform::parse("linux-x86_64").unwrap()
    }

    fn windows() -> Platform {
        Platform::parse("windows-x86_64").unwrap()
    }

    #[test]
    fn the_linux_program_comes_out_of_the_tarball_with_the_rest_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.tar.gz");
        let root = "leon-0.2.1-linux-x86_64";
        tar_gz(
            &archive,
            &[
                (&format!("{root}/"), b""),
                (&format!("{root}/leon"), b"BINARY"),
                (
                    &format!("{root}/share/applications/dev.zavu.leon.desktop"),
                    b"[Desktop]",
                ),
                (&format!("{root}/INSTALL.txt"), b"hi"),
            ],
        );
        let out = extract(
            &archive,
            &linux(),
            &v(),
            &dir.path().join("out"),
            &FakeTools::new("0.2.1"),
        )
        .unwrap();
        assert_eq!(out, dir.path().join("out/leon"));
        assert_eq!(std::fs::read(&out).unwrap(), b"BINARY");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&out).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }
        assert!(
            !dir.path().join("out/share").exists(),
            "only the program is written"
        );
    }

    #[test]
    fn the_windows_program_comes_out_of_the_zip() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        let root = "leon-0.2.1-windows-x86_64";
        zip(
            &archive,
            &[
                (&format!("{root}/"), b""),
                (&format!("{root}/leon.exe"), b"MZ-EXE"),
                (&format!("{root}/LICENSE"), b"x"),
            ],
        );
        let out = extract(
            &archive,
            &windows(),
            &v(),
            &dir.path().join("out"),
            &FakeTools::new("0.2.1"),
        )
        .unwrap();
        assert_eq!(out, dir.path().join("out/leon.exe"));
        assert_eq!(std::fs::read(&out).unwrap(), b"MZ-EXE");
    }

    #[test]
    fn an_archive_with_a_second_top_folder_or_the_wrong_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let tools = FakeTools::new("0.2.1");
        for (entries, why) in [
            (
                vec![
                    ("leon-0.2.1-linux-x86_64/leon", &b"X"[..]),
                    ("evil/leon", b"Y"),
                ],
                "outside the folder",
            ),
            (vec![("leon", b"X")], "outside the folder"),
            (
                vec![("leon-0.2.0-linux-x86_64/leon", b"X")],
                "outside the folder",
            ),
            (
                vec![("leon-0.2.1-linux-x86_64/share/x", b"X")],
                "is not in it",
            ),
            (
                vec![("leon-0.2.1-linux-x86_64/../escape", b"X")],
                "leaves the archive",
            ),
            (vec![("/etc/leon", b"X")], "leaves the archive"),
            (vec![("leon-0.2.1-linux-x86_64/leon", b"")], "is empty"),
        ] {
            let archive = dir.path().join("a.tar.gz");
            tar_gz(&archive, &entries);
            let error = extract(&archive, &linux(), &v(), &dir.path().join("out"), &tools)
                .unwrap_err()
                .to_string();
            assert!(error.contains(why), "{entries:?}: {error}");
        }
    }

    #[test]
    fn a_link_in_a_tarball_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.tar.gz");
        let encoder = flate2::write::GzEncoder::new(
            std::fs::File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder
            .append_link(&mut header, "leon-0.2.1-linux-x86_64/leon", "/bin/sh")
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        let error = extract(
            &archive,
            &linux(),
            &v(),
            &dir.path().join("out"),
            &FakeTools::new("0.2.1"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("neither a file nor a folder"), "{error}");
    }

    #[test]
    fn a_zip_with_an_escaping_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        zip(
            &archive,
            &[
                ("leon-0.2.1-windows-x86_64/leon.exe", b"MZ"),
                ("../escape.txt", b"x"),
            ],
        );
        assert!(extract(
            &archive,
            &windows(),
            &v(),
            &dir.path().join("out"),
            &FakeTools::new("0.2.1")
        )
        .is_err());
    }

    #[test]
    fn something_that_is_not_an_archive_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        std::fs::write(&archive, b"not a zip").unwrap();
        assert!(extract(
            &archive,
            &windows(),
            &v(),
            &dir.path().join("out"),
            &FakeTools::new("0.2.1")
        )
        .is_err());
        let archive = dir.path().join("a.tar.gz");
        std::fs::write(&archive, b"not a tar").unwrap();
        assert!(extract(
            &archive,
            &linux(),
            &v(),
            &dir.path().join("out"),
            &FakeTools::new("0.2.1")
        )
        .is_err());
    }

    #[test]
    fn the_bundle_comes_out_of_the_image_and_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image");
        fixtures::image(&image, "0.2.1", b"MACHO");
        let mac = Platform::parse("macos-aarch64").unwrap();
        let out = extract(
            &image,
            &mac,
            &v(),
            &dir.path().join("out"),
            &FakeTools::new("0.2.1"),
        )
        .unwrap();
        assert_eq!(out, dir.path().join("out/Leon.app"));
        assert_eq!(
            std::fs::read(out.join("Contents/MacOS/Leon")).unwrap(),
            b"MACHO"
        );
    }

    #[test]
    fn a_bundle_of_another_version_or_without_its_program_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mac = Platform::parse("macos-aarch64").unwrap();
        let tools = FakeTools::new("0.2.1");
        let image = dir.path().join("image");
        fixtures::image(&image, "0.2.0", b"MACHO");
        let error = extract(&image, &mac, &v(), &dir.path().join("o1"), &tools)
            .unwrap_err()
            .to_string();
        assert!(error.contains("version 0.2.0"), "{error}");
        std::fs::remove_file(image.join("Leon.app/Contents/MacOS/Leon")).unwrap();
        assert!(extract(&image, &mac, &v(), &dir.path().join("o2"), &tools).is_err());
    }

    #[test]
    fn the_bundle_version_is_read_from_the_plist() {
        let plist = "<key>CFBundleName</key><string>Leon</string>\n<key>CFBundleShortVersionString</key>\n    <string>0.2.1</string>";
        assert_eq!(bundle_version(plist), Some("0.2.1".into()));
        assert_eq!(bundle_version("<plist/>"), None);
    }

    #[test]
    fn names_follow_the_release_layout() {
        assert_eq!(root_name(&linux(), &v()), "leon-0.2.1-linux-x86_64");
        assert_eq!(payload_name(&windows()), "leon.exe");
        assert_eq!(
            kind(&Platform::parse("macos-x86_64").unwrap()),
            Kind::Bundle
        );
        assert_eq!(kind(&linux()), Kind::File);
    }
}

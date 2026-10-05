//! The release archives and `SHA256SUMS`.
//!
//! An archive holds one directory, `leon-<version>-<platform>/`, with the
//! inputs inside it. The same inputs give the same bytes: times are fixed and
//! no owners or host details go in.

use crate::version::Version;
use crate::NAME;
use sha2::{Digest as _, Sha256};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The platforms a release is built for.
pub const PLATFORMS: [&str; 4] = [
    "linux-x86_64",
    "macos-aarch64",
    "macos-x86_64",
    "windows-x86_64",
];

/// The checksum file, as `sha256sum -c` reads it.
pub const CHECKSUMS: &str = "SHA256SUMS";

fn check_platform(platform: &str) -> Result<(), String> {
    if PLATFORMS.contains(&platform) {
        Ok(())
    } else {
        Err(format!(
            "unknown platform `{platform}`: one of {}",
            PLATFORMS.join(", ")
        ))
    }
}

/// The extension of a platform's archive: a zip on Windows, a gzipped tar
/// elsewhere.
pub fn extension(platform: &str) -> &'static str {
    if platform.starts_with("windows-") {
        "zip"
    } else {
        "tar.gz"
    }
}

/// `leon-<version>-<platform>.<ext>`
pub fn archive_name(version: &Version, platform: &str) -> Result<String, String> {
    check_platform(platform)?;
    Ok(format!(
        "{NAME}-{version}-{platform}.{}",
        extension(platform)
    ))
}

/// The directory inside the archive: the archive's name without its extension.
pub fn root_name(version: &Version, platform: &str) -> Result<String, String> {
    check_platform(platform)?;
    Ok(format!("{NAME}-{version}-{platform}"))
}

/// One thing to put in an archive.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Dir(PathBuf),
    File {
        path: PathBuf,
        source: PathBuf,
        executable: bool,
    },
}

#[cfg(unix)]
fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

/// On Windows the file system does not say: what has no extension, or
/// `.exe`, is a program.
#[cfg(not(unix))]
fn is_program(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        None => true,
        Some(extension) => extension.eq_ignore_ascii_case("exe"),
    }
}

fn collect(source: &Path, inside: &Path, out: &mut Vec<Entry>) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        return Err(std::io::Error::other(format!(
            "{} is a symbolic link; archives hold files and directories only",
            source.display()
        )));
    }
    if meta.is_dir() {
        out.push(Entry::Dir(inside.to_owned()));
        let mut children: Vec<_> = std::fs::read_dir(source)?.collect::<Result<_, _>>()?;
        children.sort_by_key(|child| child.file_name());
        for child in children {
            collect(&child.path(), &inside.join(child.file_name()), out)?;
        }
    } else {
        out.push(Entry::File {
            path: inside.to_owned(),
            source: source.to_owned(),
            executable: is_program(source),
        });
    }
    Ok(())
}

fn entries(inputs: &[PathBuf], root: &str) -> std::io::Result<Vec<Entry>> {
    let mut out = vec![Entry::Dir(PathBuf::from(root))];
    for input in inputs {
        let name = input
            .file_name()
            .ok_or_else(|| std::io::Error::other(format!("{} has no name", input.display())))?;
        collect(input, &Path::new(root).join(name), &mut out)?;
    }
    Ok(out)
}

/// Packs `inputs` under `root/` into `out`: a zip when `out` ends in `.zip`,
/// a gzipped tar otherwise.
pub fn pack(inputs: &[PathBuf], root: &str, out: &Path) -> std::io::Result<()> {
    let entries = entries(inputs, root)?;
    if out.extension().is_some_and(|e| e == "zip") {
        write_zip(&entries, out)
    } else {
        write_tar_gz(&entries, out)
    }
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn write_tar_gz(entries: &[Entry], out: &Path) -> std::io::Result<()> {
    let file = std::fs::File::create(out)?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::best());
    let mut builder = tar::Builder::new(encoder);
    for entry in entries {
        let mut header = tar::Header::new_gnu();
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        match entry {
            Entry::Dir(path) => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
                header.set_size(0);
                builder.append_data(
                    &mut header,
                    format!("{}/", slash_path(path)),
                    std::io::empty(),
                )?;
            }
            Entry::File {
                path,
                source,
                executable,
            } => {
                let bytes = std::fs::read(source)?;
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(if *executable { 0o755 } else { 0o644 });
                header.set_size(bytes.len() as u64);
                builder.append_data(&mut header, slash_path(path), bytes.as_slice())?;
            }
        }
    }
    let mut file = builder.into_inner()?.finish()?;
    file.flush()?;
    file.sync_all()
}

fn write_zip(entries: &[Entry], out: &Path) -> std::io::Result<()> {
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(out)?;
    let mut writer = zip::ZipWriter::new(file);
    let base = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for entry in entries {
        match entry {
            Entry::Dir(path) => {
                writer
                    .add_directory(
                        format!("{}/", slash_path(path)),
                        base.unix_permissions(0o755),
                    )
                    .map_err(std::io::Error::other)?;
            }
            Entry::File {
                path,
                source,
                executable,
            } => {
                let mode = if *executable { 0o755 } else { 0o644 };
                writer
                    .start_file(slash_path(path), base.unix_permissions(mode))
                    .map_err(std::io::Error::other)?;
                writer.write_all(&std::fs::read(source)?)?;
            }
        }
    }
    writer.finish().map_err(std::io::Error::other)?.sync_all()
}

/// The hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// `<digest>  <name>` lines, sorted by name, as `sha256sum -c` reads them.
pub fn render_checksums(mut files: Vec<(String, String)>) -> String {
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
        .into_iter()
        .map(|(name, digest)| format!("{digest}  {name}\n"))
        .collect()
}

/// The text of `SHA256SUMS` for the regular files directly in `dir`
/// (the checksum file itself excluded).
pub fn checksums_of(dir: &Path) -> std::io::Result<String> {
    let mut files = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name == CHECKSUMS || !item.file_type()?.is_file() {
            continue;
        }
        files.push((name, sha256_file(&item.path())?));
    }
    if files.is_empty() {
        return Err(std::io::Error::other("no files to take checksums of"));
    }
    Ok(render_checksums(files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version;
    use std::io::Read as _;

    fn v(text: &str) -> Version {
        version::parse(text).unwrap()
    }

    #[test]
    fn archives_are_named_by_version_and_platform() {
        assert_eq!(
            archive_name(&v("0.1.0"), "linux-x86_64").unwrap(),
            "leon-0.1.0-linux-x86_64.tar.gz"
        );
        assert_eq!(
            archive_name(&v("0.1.0"), "macos-aarch64").unwrap(),
            "leon-0.1.0-macos-aarch64.tar.gz"
        );
        assert_eq!(
            archive_name(&v("1.2.3-rc.1"), "windows-x86_64").unwrap(),
            "leon-1.2.3-rc.1-windows-x86_64.zip"
        );
        assert_eq!(
            root_name(&v("0.1.0"), "windows-x86_64").unwrap(),
            "leon-0.1.0-windows-x86_64"
        );
        assert!(archive_name(&v("0.1.0"), "plan9-mips").is_err());
    }

    #[test]
    fn checksums_are_sorted_and_in_sha256sum_format() {
        let text = render_checksums(vec![
            ("b.zip".into(), "bb".into()),
            ("a.tar.gz".into(), "aa".into()),
        ]);
        assert_eq!(text, "aa  a.tar.gz\nbb  b.zip\n");
    }

    #[test]
    fn sha256_of_a_known_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abc");
        std::fs::write(&file, "abc").unwrap();
        assert_eq!(
            sha256_file(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn checksums_skip_the_checksum_file_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("one"), "abc").unwrap();
        std::fs::write(dir.path().join(CHECKSUMS), "stale").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let text = checksums_of(dir.path()).unwrap();
        assert_eq!(
            text,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  one\n"
        );
        let empty = tempfile::tempdir().unwrap();
        assert!(checksums_of(empty.path()).is_err());
    }

    fn sample_inputs(dir: &Path) -> Vec<PathBuf> {
        let binary = dir.join("leon");
        std::fs::write(&binary, "binary").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let notes = dir.join("README.txt");
        std::fs::write(&notes, "notes").unwrap();
        let bundle = dir.join("Leon.app");
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        std::fs::write(bundle.join("Contents/Info.plist"), "plist").unwrap();
        vec![binary, notes, bundle]
    }

    #[test]
    fn a_tar_gz_holds_one_directory_with_the_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = sample_inputs(dir.path());
        let out = dir.path().join("out.tar.gz");
        pack(&inputs, "leon-0.1.0-linux-x86_64", &out).unwrap();

        let decoder = flate2::read::GzDecoder::new(std::fs::File::open(&out).unwrap());
        let mut archive = tar::Archive::new(decoder);
        let mut names = Vec::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().into_owned();
            if name == "leon-0.1.0-linux-x86_64/Leon.app/Contents/Info.plist" {
                let mut text = String::new();
                entry.read_to_string(&mut text).unwrap();
                assert_eq!(text, "plist");
            }
            #[cfg(unix)]
            if name == "leon-0.1.0-linux-x86_64/leon" {
                assert_eq!(entry.header().mode().unwrap() & 0o111, 0o111);
            }
            names.push(name);
        }
        assert_eq!(names[0], "leon-0.1.0-linux-x86_64/");
        for expected in [
            "leon-0.1.0-linux-x86_64/leon",
            "leon-0.1.0-linux-x86_64/README.txt",
            "leon-0.1.0-linux-x86_64/Leon.app/",
            "leon-0.1.0-linux-x86_64/Leon.app/Contents/Info.plist",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "{expected} missing in {names:?}"
            );
        }
    }

    #[test]
    fn a_zip_holds_the_same_layout() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = sample_inputs(dir.path());
        let out = dir.path().join("out.zip");
        pack(&inputs, "leon-0.1.0-windows-x86_64", &out).unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut text = String::new();
        zip.by_name("leon-0.1.0-windows-x86_64/Leon.app/Contents/Info.plist")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "plist");
        assert!(zip.by_name("leon-0.1.0-windows-x86_64/leon").is_ok());
    }

    #[test]
    fn the_same_inputs_give_the_same_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = sample_inputs(dir.path());
        let (a, b) = (dir.path().join("a.tar.gz"), dir.path().join("b.tar.gz"));
        pack(&inputs, "root", &a).unwrap();
        pack(&inputs, "root", &b).unwrap();
        assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
        let (a, b) = (dir.path().join("a.zip"), dir.path().join("b.zip"));
        pack(&inputs, "root", &a).unwrap();
        pack(&inputs, "root", &b).unwrap();
        assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(pack(&[link], "root", &dir.path().join("o.tar.gz")).is_err());
    }
}

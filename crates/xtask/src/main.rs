//! Release tooling, so that cutting a release needs nothing but `cargo`.
//!
//! ```text
//! cargo xtask version                       the workspace's version
//! cargo xtask check-tag v1.2.3              fails unless the tag is the version
//! cargo xtask package --platform ...        one platform's release archive
//! cargo xtask checksums --dir dist          SHA256SUMS of the files in a directory
//! cargo xtask bump patch                    the next version, in one command
//! ```
//!
//! `docs/RELEASING.md` says when each is used.

mod archive;
mod bump;
mod version;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The name archives and the binary go by.
const NAME: &str = "leon";

/// The version of the workspace: the one source of truth for a release.
const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "Release tooling for Leon

Usage: cargo xtask <COMMAND>

Commands:
  version
      Prints the workspace's version.
  check-tag <TAG>
      Fails unless <TAG> is `v` followed by the workspace's version.
  package --platform <ID> --input <PATH> [--input <PATH>...] --out-dir <DIR>
      Packs the inputs (files, or a directory such as Leon.app) into
      <DIR>/leon-<version>-<ID>.<tar.gz|zip> and prints that path.
      Platforms: linux-x86_64, macos-aarch64, macos-x86_64, windows-x86_64.
  checksums --dir <DIR>
      Writes <DIR>/SHA256SUMS for every file in <DIR>.
  bump <major|minor|patch|X.Y.Z> [--no-lock]
      Sets the workspace version in Cargo.toml, refreshes Cargo.lock and
      prints the tag to create. Pre-release versions are refused.";

/// The arguments after a command: `--flag value` pairs and bare words.
struct Args {
    values: Vec<(String, String)>,
    switches: Vec<String>,
    bare: Vec<String>,
}

impl Args {
    fn parse(args: &[String], switches: &[&str]) -> Result<Self, String> {
        let mut parsed = Self {
            values: Vec::new(),
            switches: Vec::new(),
            bare: Vec::new(),
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if let Some(name) = arg.strip_prefix("--") {
                if switches.contains(&name) {
                    parsed.switches.push(name.to_owned());
                } else if let Some((name, value)) = name.split_once('=') {
                    parsed.values.push((name.to_owned(), value.to_owned()));
                } else {
                    let value = args.next().ok_or(format!("--{name} needs a value"))?;
                    parsed.values.push((name.to_owned(), value.clone()));
                }
            } else {
                parsed.bare.push(arg.clone());
            }
        }
        Ok(parsed)
    }

    fn all(&self, name: &str) -> Vec<&str> {
        self.values
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    fn need(&self, name: &str) -> Result<&str, String> {
        self.all(name)
            .last()
            .copied()
            .ok_or(format!("--{name} is needed"))
    }

    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|switch| switch == name)
    }

    /// Fails on an option that is not in `known`.
    fn only(&self, known: &[&str]) -> Result<(), String> {
        match self
            .values
            .iter()
            .find(|(key, _)| !known.contains(&key.as_str()))
        {
            Some((key, _)) => Err(format!("unknown option --{key}")),
            None => Ok(()),
        }
    }

    /// Fails unless exactly `count` bare words were given.
    fn bare_exactly(&self, count: usize, what: &str) -> Result<(), String> {
        if self.bare.len() == count {
            Ok(())
        } else {
            Err(format!("expected {what}"))
        }
    }
}

fn run(args: &[String]) -> Result<String, String> {
    let Some((command, rest)) = args.split_first() else {
        return Err(USAGE.to_owned());
    };
    match command.as_str() {
        "version" => {
            let args = Args::parse(rest, &[])?;
            args.bare_exactly(0, "no arguments")?;
            Ok(VERSION.to_owned())
        }
        "check-tag" => {
            let args = Args::parse(rest, &[])?;
            args.bare_exactly(1, "one argument: the tag")?;
            let version = version::parse(VERSION)?;
            version::check_tag(&args.bare[0], &version)?;
            Ok(format!("{} is the tag of {version}", args.bare[0]))
        }
        "package" => package(&Args::parse(rest, &[])?).map(|path| path.display().to_string()),
        "checksums" => checksums(&Args::parse(rest, &[])?),
        "bump" => bump_command(&Args::parse(rest, &["no-lock"])?),
        "help" | "--help" | "-h" => Ok(USAGE.to_owned()),
        other => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    }
}

fn package(args: &Args) -> Result<PathBuf, String> {
    args.only(&["platform", "input", "out-dir"])?;
    args.bare_exactly(0, "no bare arguments")?;
    let platform = args.need("platform")?;
    let version = version::parse(VERSION)?;
    let inputs: Vec<PathBuf> = args.all("input").into_iter().map(PathBuf::from).collect();
    if inputs.is_empty() {
        return Err("--input is needed".into());
    }
    if let Some(missing) = inputs.iter().find(|input| !input.exists()) {
        return Err(format!("{} is not there", missing.display()));
    }
    let out_dir = PathBuf::from(args.need("out-dir")?);
    std::fs::create_dir_all(&out_dir).map_err(|error| error.to_string())?;
    let name = archive::archive_name(&version, platform)?;
    let root = archive::root_name(&version, platform)?;
    let out = out_dir.join(name);
    archive::pack(&inputs, &root, &out).map_err(|error| format!("{}: {error}", out.display()))?;
    Ok(out)
}

fn checksums(args: &Args) -> Result<String, String> {
    args.only(&["dir"])?;
    args.bare_exactly(0, "no bare arguments")?;
    let dir = PathBuf::from(args.need("dir")?);
    let text =
        archive::checksums_of(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let out = dir.join(archive::CHECKSUMS);
    std::fs::write(&out, &text).map_err(|error| format!("{}: {error}", out.display()))?;
    Ok(out.display().to_string())
}

fn bump_command(args: &Args) -> Result<String, String> {
    args.only(&[])?;
    args.bare_exactly(1, "one argument: major, minor, patch or X.Y.Z")?;
    let manifest = workspace_root().join("Cargo.toml");
    let text =
        std::fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let current = version::parse(&bump::workspace_version(&text)?)?;
    let next = bump::next(&current, &args.bare[0])?;
    let updated = bump::set_workspace_version(&text, &next)?;
    std::fs::write(&manifest, updated).map_err(|e| format!("{}: {e}", manifest.display()))?;
    if !args.has("no-lock") {
        refresh_lock(&workspace_root())?;
    }
    Ok(format!(
        "{current} -> {next}\n\
         Review the change, commit it, then create and push the tag:\n  \
         git tag v{next} && git push origin v{next}"
    ))
}

/// Brings the workspace members' own entries in `Cargo.lock` to the new
/// version, touching no dependency.
fn refresh_lock(root: &Path) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = std::process::Command::new(cargo)
        .args(["update", "--workspace"])
        .current_dir(root)
        .status()
        .map_err(|error| format!("cargo update: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("cargo update --workspace failed".into())
    }
}

/// The repository root: two levels above this crate.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .components()
        .collect()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn version_prints_the_workspace_version() {
        assert_eq!(run(&strings(&["version"])).unwrap(), VERSION);
    }

    #[test]
    fn check_tag_accepts_only_the_current_version() {
        let tag = format!("v{VERSION}");
        assert!(run(&strings(&["check-tag", &tag])).is_ok());
        assert!(run(&strings(&["check-tag", VERSION])).is_err());
        assert!(run(&strings(&["check-tag", "v999.0.0"])).is_err());
        assert!(run(&strings(&["check-tag"])).is_err());
    }

    #[test]
    fn unknown_commands_and_options_are_refused() {
        assert!(run(&strings(&["nope"])).is_err());
        assert!(run(&strings(&[
            "package",
            "--platform",
            "linux-x86_64",
            "--wat",
            "1"
        ]))
        .is_err());
        assert!(run(&[]).is_err());
    }

    #[test]
    fn options_accept_both_spellings() {
        let args = Args::parse(&strings(&["--a", "1", "--b=2", "--a", "3", "word"]), &[]).unwrap();
        assert_eq!(args.all("a"), ["1", "3"]);
        assert_eq!(args.need("b").unwrap(), "2");
        assert_eq!(args.bare, ["word"]);
        assert!(Args::parse(&strings(&["--a"]), &[]).is_err());
    }
}

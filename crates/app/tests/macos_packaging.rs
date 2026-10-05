//! The macOS packaging scripts, run for real on dummy executables in a
//! temporary directory: the bundle script writes `Contents/MacOS/Leon` and a
//! plist the system accepts, and the cargo runner wraps the application binary
//! in a development bundle while every other executable passes through
//! untouched. Nothing here starts the application or touches the user's files.
#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// An executable shell script that prints the path it was started as, then its
/// arguments one per line, and exits with `status`.
fn dummy(path: &Path, status: i32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!("#!/bin/sh\necho \"exe=$0\"\nfor a in \"$@\"; do echo \"arg=$a\"; done\nexit {status}\n"),
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn runner(exe: &Path, args: &[&str]) -> Output {
    Command::new(root().join("scripts/cargo-runner-macos.sh"))
        .arg(exe)
        .args(args)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn plist_value(plist: &Path, key: &str) -> String {
    let out = Command::new("plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist)
        .output()
        .unwrap();
    assert!(out.status.success(), "no key {key}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn both_scripts_are_valid_shell() {
    for script in [
        "scripts/cargo-runner-macos.sh",
        "scripts/run-macos.sh",
        "packaging/macos/bundle.sh",
    ] {
        let shell = if script.ends_with("run-macos.sh") || script.ends_with("bundle.sh") {
            "bash"
        } else {
            "sh"
        };
        let out = Command::new(shell)
            .arg("-n")
            .arg(root().join(script))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{script}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn the_bundle_script_names_the_executable_leon_and_writes_a_valid_plist() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("in/leon");
    dummy(&binary, 0);
    let out = Command::new(root().join("packaging/macos/bundle.sh"))
        .args([&binary, Path::new("9.8.7"), dir.path()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let app = dir.path().join("Leon.app");
    let inner = app.join("Contents/MacOS/Leon");
    assert!(inner.is_file());
    assert_eq!(
        fs::metadata(&inner).unwrap().permissions().mode() & 0o111,
        0o111
    );
    assert!(app.join("Contents/Resources/AppIcon.icns").is_file());
    let plist = app.join("Contents/Info.plist");
    let lint = Command::new("plutil")
        .arg("-lint")
        .arg(&plist)
        .output()
        .unwrap();
    assert!(lint.status.success());
    for (key, value) in [
        ("CFBundleExecutable", "Leon"),
        ("CFBundleName", "Leon"),
        ("CFBundleDisplayName", "Leon"),
        ("CFBundleIdentifier", "dev.zavu.leon"),
        ("CFBundleIconFile", "AppIcon"),
        ("CFBundlePackageType", "APPL"),
        ("CFBundleShortVersionString", "9.8.7"),
        ("CFBundleVersion", "9.8.7"),
        ("LSMinimumSystemVersion", "11.0"),
        (
            "LSApplicationCategoryType",
            "public.app-category.developer-tools",
        ),
        ("NSHighResolutionCapable", "true"),
    ] {
        assert_eq!(plist_value(&plist, key), value, "{key}");
    }
    // A packaged bundle is not marked as the development one.
    let missing = Command::new("plutil")
        .args(["-extract", "LeonDevelopmentBuild", "raw", "-o", "-"])
        .arg(&plist)
        .output()
        .unwrap();
    assert!(!missing.status.success());
}

#[test]
fn another_executable_passes_through_with_its_arguments_and_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    for path in [
        "target/debug/deps/leon-0123abcd",
        "target/debug/xtask",
        "target/debug/examples/leon",
    ] {
        let exe = dir.path().join(path);
        dummy(&exe, 7);
        let out = runner(&exe, &["one", "two words"]);
        assert_eq!(out.status.code(), Some(7), "{path}");
        let shown = text(&out);
        assert!(shown.contains(&format!("exe={}", exe.display())), "{shown}");
        assert!(shown.contains("arg=one\narg=two words\n"), "{shown}");
        assert!(!dir.path().join("target/debug/Leon.app").exists(), "{path}");
    }
}

#[test]
fn the_application_binary_runs_from_a_development_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("target/debug/leon");
    dummy(&exe, 3);
    let out = runner(&exe, &["--data-dir", "a b"]);
    assert_eq!(out.status.code(), Some(3));
    let shown = text(&out);
    let app = dir.path().join("target/debug/Leon.app");
    assert!(
        shown.contains(&format!("exe={}/Contents/MacOS/Leon\n", app.display())),
        "{shown}"
    );
    // The binary's own name is not among the arguments; the others are, intact.
    assert!(shown.ends_with("arg=--data-dir\narg=a b\n"), "{shown}");
    let plist = app.join("Contents/Info.plist");
    assert_eq!(plist_value(&plist, "CFBundleExecutable"), "Leon");
    assert_eq!(plist_value(&plist, "LeonDevelopmentBuild"), "true");
    assert!(app.join("Contents/Resources/AppIcon.icns").is_file());
}

#[test]
fn a_newer_build_refreshes_the_bundles_executable() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("target/debug/leon");
    dummy(&exe, 0);
    runner(&exe, &[]);
    let inner = dir.path().join("target/debug/Leon.app/Contents/MacOS/Leon");
    // Rebuilt: other bytes and a later modification time.
    fs::write(&exe, "#!/bin/sh\necho rebuilt\n").unwrap();
    assert!(Command::new("touch")
        .args(["-t", "203001010000"])
        .arg(&exe)
        .status()
        .unwrap()
        .success());
    assert_eq!(text(&runner(&exe, &[])), "rebuilt\n");
    assert!(fs::read_to_string(&inner).unwrap().contains("rebuilt"));
}

#[test]
fn the_runs_that_open_no_window_skip_the_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("target/debug/leon");
    dummy(&exe, 0);
    for args in [
        &["--help"][..],
        &["--version"],
        &["--diagnose", "connect", "dev@box"],
    ] {
        let out = runner(&exe, args);
        assert!(
            text(&out).contains(&format!("exe={}\n", exe.display())),
            "{args:?}"
        );
    }
    assert!(!dir.path().join("target/debug/Leon.app").exists());
}

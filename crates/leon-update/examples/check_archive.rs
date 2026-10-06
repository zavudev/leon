//! Checks that a built release archive can be consumed by the updater: its
//! own extraction code takes the program out of it.
//!
//!     cargo run -p leon-update --example check_archive -- dist/leon-0.2.1-macos-aarch64.dmg
//!
//! The release workflow runs it on every archive it builds, before anything is
//! published (`docs/RELEASING.md`). Exits with 1 when an archive is refused.

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: check_archive <leon-<version>-<platform>.<ext>>...");
        std::process::exit(2);
    }
    let mut failed = false;
    for file in files {
        match leon_update::diagnose::check_archive(
            std::path::Path::new(&file),
            &leon_update::SystemTools,
        ) {
            Ok(lines) => {
                for line in lines {
                    println!("{line}");
                }
            }
            Err(error) => {
                eprintln!("error: {error}");
                failed = true;
            }
        }
    }
    std::process::exit(i32::from(failed));
}

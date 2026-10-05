# Contributing to Leon

Thank you for helping. Small, focused changes are the easiest to review; for
anything large, open an issue first so we can agree on the direction.

## Build prerequisites

Leon is a Rust workspace (stable toolchain, Rust 1.85 or newer) with a GPUI
interface. `rust-toolchain.toml` selects the stable channel with `rustfmt` and
`clippy`.

| OS | You need |
| --- | --- |
| macOS | Xcode command line tools (`xcode-select --install`) and the Metal toolchain that comes with Xcode |
| Linux | A C toolchain and `pkg-config`; on Debian or Ubuntu `sudo apt install build-essential pkg-config libfontconfig1-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libx11-xcb-dev`. A GPU driver with Vulkan to run the app |
| Windows | Visual Studio Build Tools with the C++ workload and the Windows SDK |

SQLite is built from source by cargo; no other system library is needed.

## The fast loop

```sh
cargo check
cargo test -p leon <filter>        # the app: UI, keys, launch plans
cargo test -p leon-term <filter>   # the terminal crate
cargo run -p leon                  # run from source
```

The tests need no display (they use the toolkit's test platform), no `$SHELL`
and no network.

**Run `scripts/check.sh` before every push.** CI is a confirmation, not the
first test: if the gate is green on your computer, CI should be too. The script
runs what CI runs for your platform, in the same order, and stops at the first
failure:

```sh
scripts/check.sh            # or: cargo xtask check
scripts/check.sh --quick    # only format, hygiene and the generated docs
scripts/install-hooks.sh    # opt in: run it automatically before every push
```

That is `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
--locked -- -D warnings` and `cargo test --workspace --locked --no-fail-fast`,
plus guards for the platforms you are not on: no CRLF in tracked text files,
`Cargo.lock` current, the generated documents current, the shortcut registry
checked under both chord tables, the application's tests run again as the
other desktop platforms (`LEON_SIMULATE_OS=linux|windows`: the chords, the key
routing in a terminal and "this computer has no POSIX shell", not the real
operating system), and a type check of the crates without C
dependencies for Linux and Windows (when those Rust targets are installed). It
prints a PASS or FAIL summary with the time of each step and says which crates
it could not check.

Pull requests start as drafts; CI runs the build matrix when you mark one ready
for review, and skips it for changes to documentation only.

## Rules of the code

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) has the full list, in "Rules". The
short version: the UI only reads the local store and the engine writes it; every
colour, size and font is a token in `theme/`; there is one shortcut registry
(`keys::BINDINGS`); what can be decided without a window is a pure function with
tests; English everywhere, with a prose `//!` header on every file. Add or change
a test with the behaviour you change. A test must not depend on your shell, your
home directory or the clock, and must not read files outside the repository's
tracked tree.

## Commits and pull requests

* Use [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat: ...`, `fix: ...`, `docs: ...`, `refactor: ...`, `test: ...`, `chore: ...`).
* **No AI attribution lines** in commit messages or pull request descriptions:
  no `Co-Authored-By` for a tool, no "generated with". You are the author and
  answerable for what you submit.
* Keep a change to one concern, and say in the description what you ran and on
  which operating system. Linux and Windows are the least exercised: reports and
  fixes there are especially welcome.

## Licence

By contributing you agree that your contribution is licensed under the
[Apache License 2.0](LICENSE). The Leon name and mark are not part of that
licence (see [`NOTICE`](NOTICE)).

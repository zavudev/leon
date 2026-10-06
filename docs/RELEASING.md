# Releasing Leon

A release is a tag. The workflow `.github/workflows/release.yml` builds, packages,
signs (when it can) and publishes everything from it.

## Numbering

The version lives in one place: `version` under `[workspace.package]` in the root
`Cargo.toml`. Every crate inherits it, and the tag has to be `v` followed by it
(`cargo xtask check-tag` and the workflow both enforce that).

Run `scripts/check.sh` first (and before every push, release or not): CI is a
confirmation, not the first test. `scripts/install-hooks.sh` installs it as an
opt-in `pre-push` hook.

```sh
cargo xtask bump patch        # or minor, major, or an explicit X.Y.Z
git diff                      # Cargo.toml and Cargo.lock
# update CHANGELOG.md
git commit -am "chore: release 0.1.1"
git tag v0.1.1
git push origin main v0.1.1
```

`bump` edits `Cargo.toml`, refreshes the workspace entries of `Cargo.lock`
(`cargo update --workspace`) and prints the tag to create. It refuses a
pre-release version, on either side, and an explicit version that is not above
the current one. To cut a pre-release (`0.2.0-rc.1`), edit the version by hand;
the workflow publishes it marked as a pre-release.

Other commands: `cargo xtask version`, `cargo xtask check-tag <TAG>`,
`cargo xtask package --platform <id> --input <path> --out-dir <dir>` and
`cargo xtask checksums --dir <dir>` (what the workflow runs).

## What a release holds

| File | Built on | Contents |
| --- | --- | --- |
| `leon-<v>-macos-aarch64.dmg` | macOS (Apple Silicon) | `Leon.app` and an Applications link |
| `leon-<v>-macos-x86_64.dmg` | macOS (Apple silicon, cross build for Intel) | the same |
| `leon-<v>-linux-x86_64.tar.gz` | Ubuntu 22.04 | binary, desktop entry, icons, `INSTALL.txt`, `LICENSE`, `NOTICE` |
| `install.sh` | Ubuntu 22.04 | rootless Linux installer under `~/.local`; verifies the tarball with `SHA256SUMS` |
| `leon_<v>_amd64.deb`, `leon-<v>-1.x86_64.rpm` | Ubuntu 22.04 | the Linux archive installed under `/usr`, owned by the package manager |
| `leon-<v>-windows-x86_64.zip` | Windows | `leon.exe` (icon embedded), `LICENSE`, `NOTICE` |
| `SHA256SUMS` | | checksum of every file above |

The release is created with generated notes, and marked pre-release when the
version has a `-suffix`. Nothing is published unless all four platforms built.
There is no auto-updater, so there is no update manifest or update key. Linux
users rerun `install.sh` to upgrade a home install, or install the next `.deb` or
`.rpm`. The publish job builds those packages with nFPM and copies `install.sh`
into `dist` before `SHA256SUMS` is generated; a packaging failure stops the
release.

## Secrets

Add them under Settings, Secrets and variables, Actions, Secrets. All are
optional: without them the build still succeeds, is unsigned, and the job summary
says so loudly.

| Secret | What |
| --- | --- |
| `MACOS_CERTIFICATE_P12` | The "Developer ID Application" certificate with its private key, exported as `.p12`, base64-encoded (`base64 -i cert.p12 \| pbcopy`) |
| `MACOS_CERTIFICATE_PASSWORD` | The password of that `.p12` |
| `MACOS_SIGNING_IDENTITY` | The identity, for example `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID` | The Apple ID used for notarization |
| `APPLE_APP_PASSWORD` | An app-specific password for that Apple ID |
| `APPLE_TEAM_ID` | The ten-character team id |
| `WINDOWS_CERTIFICATE_PFX` | (optional) An Authenticode certificate as `.pfx`, base64-encoded |
| `WINDOWS_CERTIFICATE_PASSWORD` | (optional) Its password |

The macOS app is signed with the hardened runtime and a secure timestamp and no
entitlements file: Leon starts shells and agents as ordinary child processes
(hardened runtime does not restrict that), does not generate code at run time and
loads no plug-ins or third-party libraries, so none of the exceptions
(`allow-jit`, `disable-library-validation`, ...) is needed. If a notarized build
ever fails at launch with a hardened-runtime message, add an
`entitlements.plist` and pass `--entitlements` to the `codesign` step.

The signing steps run only when the secrets of that platform are all present.

## Runners and speed

Public repositories get the standard GitHub-hosted runners free (and larger runners
are not available on the free organization plan). Every job's `runs-on` is a
repository variable with a standard fallback, so changing the machine is a
settings change, not a code change. Set them under Settings, Secrets and
variables, Actions, Variables:

| Variable | Fallback | Used by |
| --- | --- | --- |
| `RUNNER_LINUX` | `ubuntu-24.04` (`ubuntu-22.04` in the release build) | fmt, clippy, tests, release build, publish |
| `RUNNER_MACOS_ARM` | `macos-latest` | clippy, tests, the Intel check, release build of both Macs |
| `RUNNER_WINDOWS` | `windows-latest` | clippy, tests, release build |

There is no Intel runner variable: the Intel Mac is never built on an Intel
machine. The hosted Intel runner is small and slow (a test run on it lost its
connection to GitHub after 52 minutes). Instead the CI job "Check (macOS
x86_64, cross)" type-checks the whole workspace for `x86_64-apple-darwin` on the
Apple-silicon runner (`cargo check --workspace --all-targets --target
x86_64-apple-darwin`), and the release builds the Intel binary there too, with
`cargo build --release --target x86_64-apple-darwin` (the Apple compiler
compiles the C dependencies, the bundled SQLite, for Intel). The bundle, the
signature and the disk image are made from `target/x86_64-apple-darwin/release/leon`
exactly as for the native build, and the step prints `lipo -archs` of the
result. The Intel build is therefore compiled and linked on every release, but
the test suite does not run on an Intel processor; the code is the same as the
Apple-silicon build except for the compiler target.

Ubuntu is pinned to `ubuntu-24.04` in CI so that the move of the `ubuntu-latest`
label to a newer image never changes a build by itself. Every job has a
`timeout-minutes`, well above a cold run, so a job that never gets a machine or
hangs fails early.

Note: the release's Linux binary links against the glibc of the machine that
builds it. The `ubuntu-22.04` fallback is deliberate (it runs on any distribution
that old or newer); a custom `RUNNER_LINUX` decides the baseline instead.

The same Linux files reach users in three forms. The tarball remains the portable
manual option. `packaging/linux/install.sh` downloads that tarball, checks its
entry in `SHA256SUMS`, installs the binary and desktop assets under `~/.local`,
and writes an absolute `Exec` path so graphical sessions do not depend on the
shell's `PATH`. `packaging/linux/package.sh` turns the tarball into `.deb` and
`.rpm` packages with nFPM; those install under `/usr` and are owned by the system
package manager.

What the workflows already do for speed: `fmt` is its own job and fails in
seconds; clippy runs beside the tests, not before them; `--locked`;
`Swatinem/rust-cache` with `cache-on-failure`, written from `main` only (pull
requests and tags read it); line-tables-only debug information in CI. `sccache`
is deliberately not used: `rust-cache` already restores the whole `target` of the
dependencies, and a second compiler cache would add network round trips for the
same objects and compete for the 10 GB cache quota. `mold` is not needed either:
current stable Rust links with `lld` by default on x86_64 Linux.

Ways to go faster, with rough expectations (a cold build of this workspace is
dominated by GPUI and its dependencies; a warm cache brings every job to a few
minutes):

* **GitHub larger runners** (for example `ubuntu-latest-8-cores`): 2 to 4 times
  faster cold builds. They need a Team or Enterprise plan; create the runner in
  the organization settings and put its label in the variable.
* **Third-party runner providers** (Namespace, Blacksmith, Depot, BuildJet, Warp
  and similar): usually 2 to 3 times faster and cheaper per minute than GitHub's
  larger runners, with Apple Silicon and Windows options. Install the provider's
  GitHub app and put the label it documents in the variable, for example
  `RUNNER_LINUX=blacksmith-8vcpu-ubuntu-2404` or a `namespace-profile-...` label.
  Check their glibc baseline before using one for the release.
* **A self-hosted runner**: a fast machine you own (a Mac mini for the two macOS
  jobs is the common choice). Register it with a label and use that label. It
  keeps its `target` directory between runs, so builds are incremental, but only
  run trusted code on it: do not let pull requests from forks use it.

## Running a failed release again

* A failed build before anything was published: fix the cause, then either
  re-run the failed jobs from the Actions page (the tag still points to the same
  commit), or delete the tag and push it again on the fixed commit
  (`git push --delete origin v0.1.1`, `git tag -f v0.1.1`, `git push origin v0.1.1`).
  Keep the version unless a file was already published.
* A broken release that is already published: do not reuse the number. Fix
  forward with the next patch version, and delete or edit the bad release on
  GitHub if needed.
* To try everything without publishing, run the "Release" workflow by hand
  (Actions, Release, Run workflow, "dry run" on): it builds and packages every
  platform and attaches the files to the run as artifacts.
* A tag whose version does not match `Cargo.toml` fails at the first job with
  the expected tag in the message.

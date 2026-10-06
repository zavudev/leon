# Updates

Leon updates itself from the GitHub releases of this repository
(`https://github.com/zavudev/leon/releases`) and from nowhere else. There is no
update server, no manifest we host, no extra signing key and no secret: the
release is the source of truth, and what an update is held to is described
below, including what that does **not** protect against.

The code is the `leon-update` crate (no window, no GPUI); the window's side is
`crates/app/src/updates.rs` and `ui/updates_view.rs`.

## How it works

1. **Check.** `GET https://api.github.com/repos/zavudev/leon/releases/latest`
   (the list endpoint when pre-releases are on), unauthenticated, with a
   `User-Agent`. The answer's `ETag` is kept with its body and sent back as
   `If-None-Match`, so asking again is a `304` that costs nothing against the rate
   limit. A `403` or `429` (rate limit) is not an error: Leon stays away until
   `Retry-After` or `X-RateLimit-Reset` (an hour when GitHub says nothing) and
   shows nothing. The first check is a few seconds after the window is up, then
   every six hours, spread by up to a sixth either way; only one is ever in
   flight.
2. **Decide.** Only a tag that is exactly `v<semver>` counts; a draft or a
   tag like `nightly` is ignored. The release is an update only when its version
   is strictly newer in semantic-version order (`0.10.0` is above `0.9.0`) and it
   is not a pre-release (unless you follow that channel). An equal or older
   version is never installed, whatever GitHub says.
3. **Select.** The file is picked by name from the platform:
   `leon-<version>-<platform>.<ext>` with `macos-aarch64.dmg`,
   `macos-x86_64.dmg`, `linux-x86_64.tar.gz` or `windows-x86_64.zip`. Its address
   is built from the tag and the name, and the one in GitHub's answer has to be
   the same. A release without a file for this platform says "no build for this
   platform" and stops; one without `SHA256SUMS` is refused.
4. **Download.** The file and `SHA256SUMS` of that release go into
   `<data folder>/updates/<version>/`. A download is resumed (`Range`) where an
   earlier one stopped, shows its progress, and is refused if its size is not the
   one GitHub states, if the server says another length, or if it grows past it;
   no archive may be over 512 MiB. Every hop is asked for by hand and must be
   `https://github.com/zavudev/leon/releases/download/…` or one of the hosts GitHub
   redirects release assets to (`release-assets.githubusercontent.com`, and
   `objects.githubusercontent.com` which it used before). Any other address
   ends the download before anything is asked of it. The redirect currently seen
   for `v0.1.0` assets is `github.com` → `release-assets.githubusercontent.com`.
5. **Verify.** The file's SHA-256 has to equal the one entry for its name in
   `SHA256SUMS` (compared in constant time; the name must be listed exactly once;
   any malformed line spoils the file; CRLF is fine) and, when GitHub states a
   `digest` for the asset, that as well. On any mismatch the download is deleted
   and the version is not tried again until a newer release exists.
6. **Unpack.** Only the program is taken out of the archive (an archive that
   has another top folder, a path that climbs out, a link, or no program is
   refused): `Leon.app` from the disk image on macOS (mounted read-only,
   invisible, and always let go of), `leon-<v>-linux-x86_64/leon` from the tarball,
   `leon-<v>-windows-x86_64/leon.exe` from the zip. A bundle has to be Leon's and
   of the version the release says.
7. **Sign-check** (below), then the update is **ready**.
8. **Install**, when you choose *Restart to update* (or quit, in the automatic
   mode, with nothing running): the program is copied next to the installed one,
   the installed one is renamed aside, the new one renamed into its place (two
   renames on one volume: never half of each), and the new build is started once
   with `--version` to see that it runs at all. If it does not, the old one is put
   back at once.
9. **Hand over** (restart only): the new build is started and told to wait until
   this process has let go of the data; this process, which has no window by then,
   watches it. When the new build's window has been up for a few seconds it
   confirms (`confirm_started`), and the kept old version is removed. If it
   exits with an error before it confirms, the old one is put back and started,
   and the failure is recorded. If it is still starting after 45 seconds it is
   left alone (a slow computer is not a failure) and keeps count of its own
   unconfirmed starts (step 10).
10. **Roll back.** A version that did not run, did not start, or failed to
    verify is recorded as *refused*: it is not tried again until a newer release
    exists, and Leon says so once in the status line. A new build that starts but
    keeps dying before it confirms puts the old one back itself after three starts.

The state is `Idle → Checking → UpToDate | Available | Manual | NoBuild →
Downloading → Ready → Installing → RestartRequired`, or `Failed` with when to
try again, published over a watch channel; what is kept between runs (last
check, ETag, skipped and refused versions, the update that is ready) is in
`<data folder>/updates/state.json`.

## Modes (Settings, Advanced & About)

| Setting | Values | What it does |
| --- | --- | --- |
| `updates_mode` | `automatic` (default) | Look, download in the background, install when you restart to update, or on quitting with nothing running, or at the next start. |
| | `notify` | Look and tell you; nothing is downloaded until you choose *Restart to update* (or the release notes' *Update*). |
| | `off` | Never ask GitHub. |
| `updates_prereleases` | off (default) | Also offer pre-releases (release candidates, betas). |

Commands (palette; on macOS also the Leon and Help menus): *Check for updates…*,
*Restart to update*, *Show release notes*, *Skip this version*, *Open the
download page*. The footer shows the update when there is something to do about
it (*Update available · 0.2.1*, *Downloading 0.2.1 · 42%*, *Restart to update ·
0.2.1*); the status line answers a check you asked for (*Leon 0.2.0 is the
latest.*); the About card says where updates stand. The release notes are drawn
as text: headings, bullets and code; tags, links and images are reduced to their
words and nothing is fetched from an address in them.

Leon **never restarts by itself.** A restart ends every terminal, so *Restart to
update* asks first and names what it closes ("2 running sessions will be
closed"). Quitting with an update ready installs it on the way out only in the
automatic mode and only when no session is running.

A check never runs in a development build (one under `target/`), and not when
`LEON_NO_UPDATE` is set to anything but empty; both also make *Check for
updates…* explain why instead.

## What is verified, and what is not

**Verified**

* **Corruption and truncation**: size and SHA-256 against the release.
* **A tampered mirror or a man in the middle on the way to the file**: the file
  has to match `SHA256SUMS`, fetched from `github.com` over HTTPS, and addresses
  outside GitHub's release storage are refused (the download is not allowed to
  go anywhere else).
* **A downgrade or a replay of an old release**: the version has to be strictly
  newer, and the bundle's own version has to be the one the release says.
* **A signed install replaced by an unsigned or differently signed build**
  (macOS and Windows). If the running build is signed with a Team ID (macOS) or an
  Authenticode certificate (Windows), the new one has to pass `codesign --verify
  --deep --strict` (a whole Authenticode signature) and carry the **same** Team ID
  (the same certificate subject). Anything else is refused and deleted.
* **A broken update**: it must print its version when started, it must come up
  and confirm, and otherwise the old one is put back.

**Not verified**

* **Anyone who can publish a release in this repository.** The checksum file and
  the archives come from the same place, so a person (or a stolen token) that can
  create a release can publish a malicious update, and, for an unsigned install,
  Leon will accept it: there is no key of ours that the release has to be signed
  with, by design ("only the GitHub releases"). Protect the repository's write
  access accordingly (branch and tag protection, two-person review of releases,
  signing the macOS and Windows builds: with a Developer ID or certificate in
  place, only a build signed by the same identity is accepted, which narrows this
  to whoever also holds the signing secrets).
* **Unsigned builds** (all of them until the certificates are configured in
  `docs/RELEASING.md`) have no signer to compare, so "same signer" cannot be
  enforced. Leon says *This build is not signed* in the footer's tooltip, the
  release notes and About, and in the log.
* **GitHub itself** (the account, the CDN, the TLS roots of the computer) is
  trusted.
* Linux has no signature scheme here: the checksum is all there is.

Gatekeeper and quarantine: what `curl` downloads has no `com.apple.quarantine`
(only the browsers and apps that opt in set it; the file Leon fetches carries
`com.apple.provenance` only, which was checked with the real `v0.1.0` disk
image). So the app taken out of the image does not trigger Gatekeeper's first-open
question again, exactly as the app you already approved does not, and Leon leaves
attributes as they are (it copies the bundle with `ditto`, which keeps the
signature's seal). A signed and notarized build is assessed by the system on its
own.

## Where it works

| Platform | The unit replaced | Notes |
| --- | --- | --- |
| macOS | the `Leon.app` folder | Found from the executable's path. Not touched when it is under `target/` or marked as a development bundle, translocated by Gatekeeper (run from where it was downloaded), on a read-only disk image (`/Volumes/…`), or in a folder this user cannot write; Leon then offers *Open the download page*. The old bundle waits beside it as `.Leon.app.leon-old`. |
| Windows | `leon.exe` | A running executable can be renamed but not overwritten: the old one is renamed aside (`.leon.exe.leon-old`), the new one moved in, and the old one is removed at the next start (it is still running until then). Not Program Files without rights, and not a Microsoft Store package. Authenticode is read with PowerShell's `Get-AuthenticodeSignature`. |
| Linux | the `leon` binary | Replaced in place when its folder is writable (a tarball install); `/usr`, Nix, Snap, Flatpak and AppImage installs are package-managed and are never touched. Only the binary is replaced; the desktop entry and the icons of the tarball stay as they are. |

Leon never asks for more rights than the user has: no `sudo`, no UAC prompt.

## HTTP

Through the system's `curl`, started through `leon_remote::spawn` so that no
console window flashes on Windows (the same way the usage readings and avatars
are fetched), with `-q` (no `.curlrc`), `--proto =https`, `--proto-redir =https`,
no redirects of its own, a connect timeout, a speed floor (1 KiB/s for 30 s) and
no credentials. It resumes with `--continue-at` and the file it writes grows as
it goes, which is how progress is read; `curl.exe` ships with Windows 10 and
later, macOS and nearly every Linux. A pure-Rust client would add a TLS stack for
no gain here. If `curl` is missing, the check fails quietly and is tried again
later.

## Files

Under the data folder (`~/Library/Application Support/leon` on macOS,
`$XDG_DATA_HOME/leon` or `~/.local/share/leon` on Linux, `%APPDATA%\leon` on
Windows), in `updates/`:

* `state.json`: the last check, the ETag and its body, skipped and refused
  versions, the update that is ready.
* `pending.json`: an update that is installed and not yet confirmed.
* `<version>/`: the download (`*.part` while it lasts), `SHA256SUMS`, and the
  unpacked program in `payload/`. Removed once installed or refused.

Beside the program, while an update is unconfirmed: `.<name>.leon-old` (the
previous version), `.<name>.leon-new` (a copy being put in place) and
`.<name>.leon-failed`.

## How to turn it off

Settings, `Updates`, `Off`; or `"updates_mode": "off"` in `settings.json`; or
`LEON_NO_UPDATE=1` in the environment, which also stops the check being offered
by hand.

## How to roll back by hand

While an update is unconfirmed the previous version is still beside the program:
quit Leon, delete the new `Leon.app` (or `leon`, `leon.exe`) and rename
`.Leon.app.leon-old` (or `.leon.leon-old`, `.leon.exe.leon-old`) to its name, and
delete `updates/pending.json`. After it has been confirmed the previous version
is gone: download it from the releases page (`leon-<version>-<platform>.<ext>`,
checked with `SHA256SUMS`) and install it over the new one. To stop Leon offering
a version, *Skip this version*, or set `updates_mode` to `off`.

## Checking it without installing anything

```sh
cargo run -p leon -- --diagnose update                          # the real check
cargo run -p leon -- --diagnose update --pretend-version 0.0.9  # take the update path
cargo run -p leon -- --diagnose update --pretend-version 0.0.9 --download --dir /some/scratch
cargo run -p leon -- --diagnose update --check-archive dist/leon-0.2.1-linux-x86_64.tar.gz
```

The first prints the running version, the latest release, the file it would take
for this platform with its size, whether `SHA256SUMS` lists it and the decision.
`--download` fetches the file into `--dir`, verifies it, unpacks it and compares
the signature, and installs nothing. `--check-archive` is what the release
workflow runs on every archive it builds, with the same extraction code (see
`docs/RELEASING.md`).

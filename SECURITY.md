# Security policy

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it privately
through GitHub: the **Security** tab of this repository, **Report a
vulnerability** (a private security advisory). Include what you found, the
version of Leon (`leon --version`), your operating system, and the steps to
reproduce it.

You will get an answer as soon as a maintainer can read it. Fixes are released
as a new version, and the advisory credits you unless you prefer otherwise.

## What to know about Leon's attack surface

Leon is a desktop application that runs programs on your behalf, so a few things
are by design and worth knowing when you assess a report:

* **It starts shells.** Every terminal is your login shell (`$SHELL -l -i`,
  PowerShell on Windows) in a real pseudo-terminal, and agents (`claude`,
  `codex`, `opencode`) are started by typing their command line into it.
* **It runs your system's `ssh`.** Remote machines are reached with `ssh` as you,
  with the keys and configuration you already have. Leon never stores a
  password or reads the contents of a private key; it reads host names from
  `~/.ssh/config` and `known_hosts` and appends to `known_hosts` only after you
  confirm. It never offers to override a changed host key.
* **It reads the agents' own history files** (under `~/.claude`, `~/.codex`,
  opencode's data directory) into a local SQLite database in Leon's data
  directory. Those files can contain anything an agent saw, so treat that
  database like the originals.
* **It can share this computer, and reach others, through a relay.** When you
  switch on **Share this machine** (or run `leon host`), computers you pair
  with a code get a terminal as you. That is the feature, not a hole: pair only
  your own devices and revoke what you lose. Connections are end-to-end
  encrypted (Noise, with keys pinned at pairing; see `docs/REMOTE.md`), so the
  relay operated by Zavu sees only metadata and ciphertext. The code in
  `leon-wire`, `leon-link` and `leon-host` has had no independent security
  review yet; reports about it are especially welcome.
* **It updates itself from this repository's GitHub releases, and nowhere
  else.** A few seconds after start and every six hours it asks the GitHub API for
  the latest release (the setting `updates_mode` turns it to `notify` or `off`,
  and `LEON_NO_UPDATE=1` stops it). A newer version is downloaded only from
  `github.com/zavudev/leon/releases/download/…` and the hosts GitHub redirects
  release assets to, and is installed only if its SHA-256 and size are the ones the
  release states in `SHA256SUMS`, it is strictly newer, and (when the running
  build is signed) it carries the same macOS Team ID or Windows certificate. It is
  never installed without the person's restart or quit and never restarts Leon on
  its own. The old version is kept until the new one has started and put back if
  it does not.

  **The trust model is the repository.** There is no update key of ours: whoever
  can publish a release in `zavudev/leon` can publish an update, and an unsigned
  install (every build until signing certificates are configured, see
  `docs/RELEASING.md`) cannot tell it from a genuine one. That is the owner's
  decision (updates only from the GitHub releases), so protecting the
  repository's write access and the release workflow's secrets is what protects
  updates. What is checked, and what is not, in full: `docs/UPDATES.md`. Reports
  about the updater (a way to make it install something the release does not
  list, to downgrade, to leave its allowed hosts, or to cross from a signed
  build to an unsigned one) are in scope and welcome.
* **It has no account or telemetry.** The servers it talks to are GitHub (for
  updates and, optionally, project avatars), the services of the agents whose
  usage you let it read (optional), and the optional relay, used only when you
  connect or share with a code.

Reports about a shell command, a path or a host name from stored data or from a
remote machine that Leon quotes or executes unsafely are in scope and welcome.
Behaviour that needs you to run a malicious command yourself in a terminal is
not a vulnerability in Leon.

## Supported versions

Only the latest release is supported.

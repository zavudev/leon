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
* **It has no account or telemetry.** The only server is the optional relay, used
  only when you connect or share with a code.

Reports about a shell command, a path or a host name from stored data or from a
remote machine that Leon quotes or executes unsafely are in scope and welcome.
Behaviour that needs you to run a malicious command yourself in a terminal is
not a vulnerability in Leon.

## Supported versions

Only the latest release is supported.

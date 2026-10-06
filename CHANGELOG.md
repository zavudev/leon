# Changelog

## [Unreleased]

### Added

* Usage limits: a footer with each agent's primary window (five-hour, weekly,
  per-model) for the machine in context, a usage view (`Show usage`) with
  Detailed and Compact modes, per-machine readings, a burn-rate estimate, a
  notice before starting an agent that is nearly at its limit, and
  `leon --diagnose usage`. Codex is read from its own session log; Claude Code
  and the opencode Go subscription need an opt-in network source (Settings,
  Usage), off by default.
* **Connect a machine, with a code.** Install Leon on the other computer, choose
  Share this machine, type the short code it shows: both computers dial out to a
  relay, so there is no SSH setup and nothing to open on any network. SSH stays
  as the second, advanced method.
* **Share this machine** (File menu, palette, Settings, sidebar) and
  `leon host` (`pair`, `devices`, `revoke`, `status`): the pairing code with its
  countdown, paired computers with Revoke, an approval prompt.
* End-to-end encryption (Noise `IK`, pairing with SPAKE2), pinned keys, revocable
  devices, durable terminals on the shared computer that can be re-attached
  without gaps or duplicates. See `docs/REMOTE.md`.
* New crates `leon-wire`, `leon-link`, `leon-pty` and `leon-host`; settings for
  the relay address, the device name, sharing and approval.

### Changed

* The worktree screen is a dashboard now: a hero with the project's logo, the
  branch and the state of its terminals; buttons to start a session (the flow,
  or one agent directly) or a shell there, to copy its path or branch, to
  reveal it, to add a worktree to its project and to remove it; and the
  sessions that ran in it with their agent, model, size and age, each opening
  its stored transcript with a click.

### Fixed

* opencode 2.x sessions appear in the history again: the database moved its
  sessions to `session_v2` and `session_message`, and Leon now reads that
  layout as well as the older `session`/`message`/`part` one.

### Not yet

* The relay service at `wss://relay.zavu.dev` is not deployed.
* Sharing stops when Leon closes (a background service is the next stage).

## [0.1.0]

First public release.

### What it does

* One tree for everything you work on: machine, project, worktree and agent
  session, with a status light on live terminals and the history of each
  worktree beneath them.
* Runs Claude Code, Codex and opencode in real terminals inside the window
  (an embedded emulator over a pseudo-terminal, ConPTY on Windows), in tabs of
  split panes. Sessions run in the background while you look at something else.
* Imports the agents' own session history into a local SQLite database, with
  search from the command palette, a read-only transcript view and
  "resume this session" for each agent.
* Detects agent processes started outside Leon and ties them to history
  sessions, so the same session is not resumed twice by accident.
* Remote machines over your system's `ssh`: a connection screen with a
  checklist and a diagnosis for each failure (reach, host key, login, shell,
  git, agents), connection sharing, and `leon --diagnose connect`.
* Terminal conveniences: find, clear, copy and save of the buffer, paste of text,
  files and (locally) images, a context menu and a menu bar.
* One shortcut registry shown in the palette and the shortcuts sheet; a
  keyboard-first interface throughout.
* Themes (`leon` and `zavu`, light and dark, and your own as TOML files),
  settings, per-project logos and a worktree activity indicator.
* Builds for macOS (Apple Silicon and Intel), Linux x86_64 and Windows x86_64.

### Known limitations

* The Linux and Windows builds have not been tried by a human. They are built
  and tested by CI, but the window, the terminal and the packaging have only
  been run on macOS.
* Real SSH paths have not been tested against real servers: the remote code is
  covered by scripted tests.
* Remote history import is not built: only sessions started from Leon on a
  remote machine are known. Pasting an image over SSH is not built either.
* Sessions end when the app quits; reattaching needs a terminal multiplexer on
  the server and is not wired up.
* Remote Windows hosts are not supported (a POSIX shell is assumed).
* No auto-update: download the next release to upgrade.
* macOS builds are unsigned and not notarized until the signing secrets are
  configured, so Gatekeeper asks for confirmation. Windows builds are unsigned
  and SmartScreen warns.

[0.1.0]: https://github.com/zavudev/leon/releases/tag/v0.1.0

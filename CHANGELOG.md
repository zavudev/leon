# Changelog

## [Unreleased]

### Fixed

* **`wss://` relays work.** The first connection to a TLS relay panicked in the
  TLS library (no process-level crypto provider was installed), leaving the
  host and the Connect screens at "connecting" for ever; this shipped in 0.1.0,
  so "With a code" could not reach `wss://relay.getleon.dev`. The crypto
  provider is now chosen on purpose (`ring`), the client configuration is built
  with it explicitly, it is also installed once at the start of the app and of
  `leon host`, and a failure while setting up TLS is reported as "The secure
  connection could not be set up" and retried instead of hanging. Tests now run
  the whole path (pairing, a command, a terminal) over TLS.

### Added

* **Agents are a catalogue, not three.** Leon now knows 43 command line agents
  (everything in Orca's list: Grok, Cursor, GitHub Copilot, Muse, DeepSeek
  Harness, ZCode, MiMo Code, Amp, OpenClaude, Antigravity, Pi, oh-my-pi, Hermes
  Agent, Devin, Goose, Auggie, Autohand Code, Charm Crush, Cline, CodeBuddy,
  Codebuff, Freebuff, Command Code, Continue, Droid, Kilocode, Kimi, Kiro,
  Mistral Vibe, Qwen Code, Rovo Dev and more) with their launch commands and,
  where Orca's source gives one, their resume form (the others are launch
  only, never a guessed flag). **New agent session** lists what the machine has
  first, then the rest dimmed with "not installed" and a docs link, and filters
  as you type. Settings ▸ Agents is generated from the catalogue, with the
  agents that are not installed folded; **Add a custom agent…** takes any
  command line tool (name, command, arguments, optional resume arguments with
  `{id}`). A bundled logo is used only where its licence allows (Simple Icons,
  CC0); every other agent gets a letter-mark tile. History import stays Claude
  Code, Codex and opencode. Stored data and settings of the three original
  agents load unchanged.
* **Usage limits for more agents**, from Orca's reference: Grok, Cursor, Kimi,
  ZCode and Antigravity, and a fresher Codex source (the backend its own usage
  screen reads, only when its session log is more than ten minutes old; it
  starts no session and writes nothing). On by default and switchable per agent
  in Settings ▸ Usage; an expired sign-in is reported, not refreshed. **These
  are implemented from Orca's source and have not been verified against the live
  services** (only Claude Code and the Codex log have). With many agents the bar
  shows the ones with numbers and folds the rest into a `+N`; the view groups
  **This machine's agents** and **Not installed / no data**. `leon --diagnose
  usage` lists every provider with its source and state.

* Usage limits: a footer with each agent's primary window (five-hour, weekly,
  per-model) for the machine in context, a usage view (`Show usage`) with
  Detailed and Compact modes, per-machine readings, a burn-rate estimate, a
  notice before starting an agent that is nearly at its limit, and
  `leon --diagnose usage`. Codex is read from its own session log; Claude Code
  and the opencode Go subscription use a network source that is on by default
  and can be turned off in Settings, Usage (macOS may ask once for keychain
  access). Limits are read every 60 seconds (at least 30) while the window is
  focused, with back-off, `Retry-After` and jitter.
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

### Fixed

* Settings: an option with a long description is as tall as its text; it was
  drawn at a fixed height and overlapped the next one.
* Usage: switching a network source on reads at once and the view and the
  footer update when it ends, with "Reading…", the specific reason of a failure
  and when the next read is, and a "Try again" for a refused keychain prompt.

### Changed

* The default relay address is `wss://relay.getleon.dev`.
* `usage_interval` (minutes) is replaced by `usage_refresh_seconds`.

### Not yet

* The relay service at `wss://relay.getleon.dev` is not deployed.
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

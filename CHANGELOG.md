# Changelog

## [Unreleased]

### Added

* A `#123` printed in a local terminal (a pull request Claude Code just
  opened, say) is a link to that pull request when the terminal's folder is a
  checkout of a GitHub repository; colours (`#123456`), anchors (`page#12`) and
  headings stay plain text. Clicking offers **Open link** like any other link.
* Rename works on every session row (running, asleep or history only, also from
  `F2` and the palette) and asks first, showing the old and the new name. The
  name is Leon's own: it is stored apart from the agent's title, so importing
  the history again never overwrites it, and a resumed session shows it. When
  the session runs in a terminal and the agent has a verified rename command
  (Claude Code: `/rename <name>`), Leon types it only while the agent waits at
  its prompt; a busy agent keeps its name and the status line says so. Other
  agents get Leon's name only.
* **Active only** no longer forces the tree open: projects and worktrees fold and open
  as in the normal view, and only a text filter opens everything on its own.
* Take over: a session running in another terminal or another Leon offers
  `Take over` (in its menu, in the question a click asks and on the
  transcript's notice). It asks the process that holds the session to end
  (SIGTERM, never forced), waits up to five seconds, and only then resumes the
  session in Leon; a process that does not end leaves the session alone and
  says so. Only for processes of this computer, and not on Windows.
* Show only active sessions: a toggle beside the sidebar's filter (also in the
  palette and the settings, `sidebar_active_only`) lists only the live
  terminals and the sessions running elsewhere, with the nodes above them, and
  says `No active sessions` when there is none.
* Sleep: a terminal, or a session that runs in one, can be put to sleep from
  its menu, the File menu or the palette. Its program is stopped and its
  terminal ended, and the session stays in the sidebar to be resumed.

### Fixed

* The `/` key did not focus the sidebar's filter on keyboards that type the
  slash with Shift (Spanish, German, Italian...): it only worked where `/` has
  a key of its own.

### Changed

* The sidebar says where it is: with a single machine its header row is left
  out and the projects sit at the top level (a second machine brings the
  headers back); a hairline and some room set each project apart, and the
  rows nested in it are told by their indentation alone.
* A worktree row separates two things: a git mark at its start (branch, main,
  merged pull request or detached head, with a tooltip in words) and the
  agents' light at its end, which draws nothing when no terminal is live and
  says `WAITING` or `FAILED` in a word. The bare grey squares are gone.
* A session that runs somewhere else is a state of its own: it keeps the
  agent's colour and wears a badge, `ELSEWHERE` for a plain terminal and
  `OTHER LEON` for a process below another Leon (a `leon` or, over SSH,
  `leon-host` ancestor). A click on it asks `Open transcript` (the default),
  `Resume anyway` or `Cancel`, and its menu starts with Open transcript and
  Resume here anyway… instead of Resume.
* A click (or `Enter`) on a history session no longer starts anything at
  once: it asks first (`Resume` or `Cancel`). A session that already has a
  terminal is only brought forward, and the transcript mode of the settings
  never asks.
* A session row tells its state at a glance: running (the agent's colour, a
  bold title and a light), asleep (dimmed, with a moon and `SLEEP`) and
  history only (grey, its age alone). Its menu starts with what that state
  does: Focus, Rename, Sleep and Close; Wake and Close; or Resume and Remove
  from history. `Open` of the menu is now `Resume`.
* Close now closes for good. Closing a terminal or a session also takes its
  history row out of the sidebar, so nothing is left dimmed behind; what Close
  used to do, keeping the row, is now Sleep. Closing a worktree (the menu's
  `Remove worktree` is now `Close`) also removes the sessions of the history
  that ran inside it. The agent's own session files are never touched, and a
  program still running, or a worktree with modified files, asks first as before.

## [0.4.0] - 2026-10-07

### Added

* Share this machine, mirrored: a computer paired with a code now sees what
  the host's own Leon holds — its projects and the sessions of its history —
  in the same places its own are. The other machine's section of the sidebar
  shows its projects, worktrees and sessions ready to open, resume and search,
  without typing a path anywhere; reconnecting keeps them up to date. Nothing
  is shared that the paired device could not read with the terminal it already
  holds, and a headless `leon host` shares nothing.
* The README shows what Leon looks like: a gallery of screenshots and four
  short clips under `docs/media/`, and `scripts/media/` regenerates them from
  a demo workspace.
* Removing a worktree that git refuses because it holds modified or untracked
  files leaves it whole and asks before they are deleted with it: only that
  answer passes `--force`.

### Changed

* The usage bar shows one figure per agent — its logo and the percentage of the
  window closest to its limit, with the level's marker — instead of every
  window of every agent; hovering still lists them all and the usage view is
  unchanged. The `Usage bar: Detailed / Compact` setting is gone. Where the
  detailed bar held two or three agents, ten now fit; with more than four
  agents the ones without numbers fold into a `+N` whose hover lists them, and
  on a narrower window the agent closest to its limit stays and the others fold
  into the count.

### Fixed

* A session running in a terminal can be closed from its row's menu, and no
  longer offers "Remove from history", which only brought the terminal back as
  a live row.
* A terminal that learned its session's id before the session was stored is
  linked to its history row once it appears, instead of showing the same
  session twice.
* A worktree row the store no longer has (taken by another window or by a
  terminal, behind Leon's back) is not an error when it is removed any more:
  the list is read again, the row goes, and the status says the worktree was
  already gone.
* Back in the window, the tree is read again: what another process (a second
  window, for one) wrote to the store shows up without waiting for a change
  of Leon's own.
* Removing a worktree closes the sessions that were running in it: the folder
  goes with the worktree, so they no longer stay in the tree under an unsorted
  folder (their programs are hung up with it). The project's row and the
  worktree's say `DELETING` while the removal runs, so a slow forced removal
  is visible.
* `Ctrl+C` in a terminal copies only when there is a selection; without one it
  reaches the program again, so a running command can be interrupted.
* A terminal whose agent moves to another session inside it (opencode's session
  list, for one) follows the title the program sets: the tree keeps one row for
  the session on screen, and opening it lands on that terminal instead of
  starting a second process on the same session.

## [0.3.0] - 2026-10-06

### Added

* Linux: the installer sets up the complete desktop app (binary, icon and
  desktop entry), not only the executable.
* Sidebar: projects, worktrees and sessions can be reordered by dragging, and
  the add, clone and create dialogs are there for new projects. A worktree
  whose pull request is merged says so, as GitHub reports it.
* The new-worktree dialog starts on the agent of the last session you started,
  and its rows answer the mouse.
* Copy and paste use the chords of the platform (Cmd on macOS, Ctrl+Shift on
  Linux and Windows terminals).

### Changed

* New agent sessions start without the agent's own questions: Claude Code is
  typed with `--dangerously-skip-permissions` and Codex with
  `--ask-for-approval never`. Both are only the default of each agent's launch
  arguments in Settings; clearing the field brings the questions back.
* A fresh install no longer turns every folder of the agents' history into a
  project. *Discover projects from session folders* is now **off** by default:
  the history is still imported and found with the history search, and the
  projects you open by hand keep their sessions. Projects you already have
  stay; turn the setting on to get the old behaviour.

### Fixed

* The sessions of a worktree that was removed no longer come back on their own.
  A worktree git stops reporting (removed from Leon, from a terminal, or by an
  agent) is remembered like a removed project, so the project that contains its
  folder does not take its sessions again; they stay in the history. A
  worktree whose folder was deleted behind git's back (git still lists it as
  prunable) is no longer kept in the tree.

## [0.2.0] - 2026-10-05

### Added

* Quitting is gentle: before the terminals are hung up, an agent in front of its
  shell is told to exit with its own command (`/exit` for Claude Code and
  opencode), then sent SIGTERM, with a short bounded grace (2.5 s in all, the
  status line says "Closing N sessions…") so it can save its session. Quitting
  never hangs.
* A session started in Leon shows up in the tree within seconds: history is
  imported again (only what changed) when an agent starts, goes quiet or ends,
  when the window regains the focus and every minute. Leon also learns the
  agent's own session id for terminals started fresh (Claude Code's state file,
  else the newest session of the folder created after the terminal started), so
  the tree shows one row and a restore can resume it.
* **Sessions come back.** Leon remembers the open terminals as they change (not
  only at quit, so a crash or a power cut loses at most half a second): machine,
  folder, agent and its session id, your renames, the tabs and panes with their
  ratios, and which tab and pane had the focus. At the next start the setting
  *Restore the last sessions* decides: **Ask me** (default; a small question
  listing the sessions, with Restore all, Choose and Not now, and "Leon did not
  close normally" after a crash), **Always**, or **Never** (the palette's
  *Restore last sessions* still brings them back). Agents are resumed through
  their own resume command and stay **paused** (`paused · press Enter to
  resume`) until their tab is first shown or you press Enter, so ten restored
  agents spend nothing until you look (*Resume restored agents* = all at once
  resumes them in the background, three a second). A terminal that cannot be
  reopened (folder gone, machine unknown, agent missing or launch only, session
  id unknown, or the same session already running in another terminal) is
  listed with the reason instead of becoming a broken shell. Scrollback is not
  restored: the agent redraws its own conversation.
* **Why is a session missing?** A palette command and `leon --diagnose history
  [--agent <id>]` report where each agent's history is looked for (defaults,
  environment variables, Settings), what format was found (SQLite tables and
  columns, `user_version`, journal mode, the `-wal` file), how many sessions
  the source holds against how many Leon imported, why the rest are skipped,
  and when the last import ran. Counts, paths and times only.
* opencode 2.x: sessions live in `session_v2` and `session_message`, and the
  old tables stay behind, empty. Leon reads every generation a database holds
  (and both, with different sessions in each), counts each in the history report
  and imports a session held by both only once.
* opencode: databases of other channels (`opencode-<channel>.db`) and
  `OPENCODE_DB` are found, the older JSON `storage/` layout is read too, an
  unknown database layout is reported instead of looking like "no sessions",
  and a change that only reached the `-wal` file moves the source's stamp.
* **The usage counters follow Orca's rules.** The footer now shows every window
  of each agent, Orca's way (`10% used 2h 29m · 91% used 1d 11h !! · 0% used
  Fable`: the countdown to the reset for the five-hour and weekly windows,
  floored; a model's name for its own), and gives way to the single worst window
  as the window narrows (Settings, Usage: `Usage bar`, `Detailed` or
  `Compact`); the usage view lists the agent nearest a limit first. `Show limits
  as` (`used` or `left`) applies to the bar, the view, the header chip and the
  notice before a session. The default warning and critical levels are Orca's 60
  and 80 percent (they were 75 and 90; a value you set keeps). Figures round half
  away from zero everywhere (12.5 is 13). An expired, rejected or rate-limited
  read no longer erases the numbers: they stay, marked with their age, with what
  to do (run the agent once so it refreshes its own sign-in) or when the next
  read is. Distinct reasons for a sign-in without the permission to read usage,
  an API-key account (no limits apply), an opencode key with no Go subscription
  or a rejected key. Claude's per-model windows come only from the scoped limits
  and a short list of known keys; Codex windows are told apart by length within a
  minute, else by position; ZCode shows the session, week and `MCP` windows;
  Kimi reset times in any of the usual forms; opencode's key from
  `OPENCODE_AUTH_CONTENT`, `auth.json`, OpenCode 2's credential database (read
  only) or `OPENCODE_API_KEY`; Claude's keychain item scoped by
  `CLAUDE_CONFIG_DIR`; the Cursor IDE's own session (read only). Antigravity: a
  build that answers `/usage` with a model turn is never asked again that
  session. Differences from Orca (no hidden sessions, no credential refreshing,
  an honest `User-Agent: Leon/<version>`, no pasted cookies) are in the README's
  Usage section.

* **Updates, from the GitHub releases of this repository and nowhere else.** A
  few seconds after the window opens and every six hours Leon asks the GitHub API
  for the latest release (conditional requests with the `ETag`, so asking again
  costs nothing; a rate limit is waited out quietly), picks the file for the
  platform by name, downloads it (resumable, with progress, only from GitHub's
  release storage), holds it to the release's `SHA256SUMS` and the size GitHub
  states, unpacks it, and installs it when you **Restart to update** or, with
  nothing running, when you quit. A signed macOS or Windows install is replaced
  only by a build with the same Team ID or certificate; an unsigned install takes
  an unsigned update and says so. The old version is kept until the new one has
  started and put back if it does not (and that version is not tried again until
  a newer release exists). Leon never restarts by itself, and the confirmation
  names how many sessions a restart closes. Settings, Advanced & About: `Updates`
  (`Automatic`, `Tell me`, `Off`) and `Pre-release versions`; commands `Check for
  updates…`, `Restart to update`, `Show release notes`, `Skip this version` and
  `Open the download page` (palette and the macOS menu); a footer item, release
  notes as plain text, and the update's state in About. Not done in a development
  build, `LEON_NO_UPDATE=1`, a disk image, a translocated or read-only install, a
  package manager's install. New crate `leon-update`; `leon --diagnose update`
  (`--pretend-version`, `--download`, `--check-archive`); the release workflow
  checks every archive it builds with the updater's own extraction. What this
  protects against and what it does not (anyone who can publish a release here):
  `docs/UPDATES.md`, `SECURITY.md`.
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

* Notifications: a geek banner over the window and a desktop notification when
  an agent finishes its turn and waits, when a session ends, or when it fails.
  The Notifications settings choose the events, the banner, the desktop note,
  and whether the desktop is only told while Leon is not in front.
* Usage limits: a footer with each agent's primary window (five-hour, weekly,
  per-model) for the machine in context, a usage view (`Show usage`) with
  Detailed and Compact modes, per-machine readings, a burn-rate estimate, a
  notice before starting an agent that is nearly at its limit, and
  `leon --diagnose usage`. Codex is read from its own session log; Claude Code
  and the opencode Go subscription use a network source that is on by default
  and can be turned off in Settings, Usage (macOS may ask once for keychain
  access). Limits are read every 10 minutes (at least 30 seconds) while the window is
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
* **The local gate**: `scripts/check.sh` (also `cargo xtask check`) runs what CI
  runs for this computer's platform, in the same order, with a summary and the
  time of every step; `scripts/install-hooks.sh` installs it as an opt-in
  `pre-push` hook. `cargo xtask bump` also regenerates the generated docs that
  embed the version.

### Changed

* The usage refresh default is **10 minutes** (it was a minute; an explicit
  setting is kept). A source that calls a vendor is never called by the schedule
  more than once a minute, opening the usage view reads when the reading is
  older than the interval, and after a 429 a source rests at least 5 minutes and
  keeps the last numbers.

* The default relay address is `wss://relay.getleon.dev`.
* Usage limits are on by default and are read every 10 minutes (at least 30 seconds)
  while the window is focused. `usage_interval` (minutes) is replaced by
  `usage_refresh_seconds`.
* Settings ▸ Agents and the new-session list are generated from the agent
  catalogue; `docs/SETTINGS.md` is generated from the settings schema.

### Fixed

* **Windows: no console windows.** Every child process Leon starts (git, ssh,
  agent probes, usage readers, host commands) now goes through a console-less
  helper, so no black window flashes open and closes.
* Settings: an option with a long description is as tall as its text; it was
  drawn at a fixed height and overlapped the next one.
* Usage: switching a network source on reads at once and the view and the
  footer update when it ends, with "Reading…", the specific reason of a failure
  and when the next read is, and a "Try again" for a refused keychain prompt.

* **Windows: a session appears under its worktree again.** Sessions were
  matched to projects and worktrees by comparing paths as raw text, and on
  Windows the same folder is spelled `C:/Users/me/code/api` by git and
  `C:\Users\me\code\api` by the agent (also `c:` for `C:`, a `\\?\` prefix, a
  trailing separator, any case), so no session matched and it fell into the
  collapsed Unsorted node. Paths are now compared by one identity function
  (`leon_core::path::key`: the style follows the path, Windows paths are
  case-insensitive, POSIX paths unchanged) in the store, the tree, discovery,
  dismissed roots, workspaces and "sessions elsewhere". Store version 6 merges
  projects and worktrees that differ only by spelling and links the sessions
  again, so an existing database heals on the next start. History import was
  checked on Windows: a transcript cut mid-line by a power loss is imported up
  to its last complete line and again when it grows; the opencode database is
  also looked for under `%APPDATA%` when it is not under `~/.local/share`.

* **`wss://` relays work.** The first connection to a TLS relay panicked in the
  TLS library (no process-level crypto provider was installed), leaving the
  host and the Connect screens at "connecting" for ever; this shipped in 0.1.0,
  so "With a code" could not reach `wss://relay.getleon.dev`. The crypto
  provider is now chosen on purpose (`ring`), the client configuration is built
  with it explicitly, it is also installed once at the start of the app and of
  `leon host`, and a failure while setting up TLS is reported as "The secure
  connection could not be set up" and retried instead of hanging. Tests now run
  the whole path (pairing, a command, a terminal) over TLS.

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

# Architecture

Leon is a Rust workspace. The rules at the end are the ones that keep it
simple; the tests enforce the ones that can be enforced.

## Crates

```text
app (leon)  ── ui, engine, keys, theme, launch, diagnose
 │   ├── leon-term     the embedded terminal (GPUI, PTY, emulator)
 │   ├── leon-mark     the animated lion (GPUI)
 │   ├── leon-remote   commands on this machine or over SSH
 │   ├── leon-history  importers for the agents' own session files
│   ├── leon-usage    the agents' usage limits, per machine
 │   ├── leon-update   updates from the GitHub releases (no window)
 │   └── leon-core     the model and the SQLite store
leon-remote ── leon-core
leon-history ── leon-core
leon-usage ── leon-core, leon-remote
leon-update ── leon-remote (only for `spawn`)
leon-term ── gpui-kit only (no other Leon crate)
leon-mark ── gpui-kit only (no other Leon crate)
```

| Crate | What it is |
| --- | --- |
| `leon-core` | Machines, projects, worktrees, sessions and messages, and the `Store` (SQLite) with change notification. No UI, no processes. |
| `leon-history` | Reads the history Claude Code, Codex and opencode keep on disk and turns it into sessions and messages. |
| `leon-remote` | A `CommandSpec` says what to run and where; `run_on` and `interactive_on` place it on a machine (unchanged locally, `ssh` remotely, with shell quoting and optional connection sharing). Git worktree operations, how each agent starts and resumes, the machine probe and the connection checklist (`connect`, `diagnosis`) are built on it. A scripted runner makes all of it testable without a process. |
| `leon-usage` | How much of each agent's limits is left. A provider-neutral model (`AgentUsage`: windows with a used percentage, a reset time and a length, or an explicit `Reason` why nothing is known; staleness is part of it), pure parsers for each source (`codex`, `claude`, `opencode`), the burn-rate `forecast`, the wording (`present`, `view`), `collect_machine` (one bounded command per machine through a `Runner`) and the opt-in `network` sources behind an `Http` trait. No UI. |
| `leon-update` | Updates from the GitHub releases of this repository. `version` (which tags count, which is newer), `release` (the API's answer, the platform's file), `http` (an `Http` trait and the system `curl` behind it, with the host allow-list), `download` (redirects by hand, resumable, bounded), `checksums` (`SHA256SUMS`, constant-time), `package` (the program out of the dmg, tarball or zip, strictly), `trust` (the signature rules), `install` (where Leon is installed and the swap with its way back), `launch` (the hand-over, the watch, the confirmation, the rollback), `updater` (the state machine published over a watch channel) and `state` (what is kept). No UI; every system tool (`hdiutil`, `ditto`, `codesign`, PowerShell) is behind a `Tools` trait. |
| `leon-wire` | The wire protocol: versioned length-prefixed frames, the application messages (run a command, terminals, re-attach) and the relay rendezvous messages. Pure (`postcard` over `serde`); every decoder is bounded and fuzz-style tested. |
| `leon-link` | Everything that keeps a remote session private: identity, the short pairing code (SPAKE2 then Noise `XXpsk3`), the Noise `IK` session with fragmentation and rekeying, the device registry, the relay WebSocket adapter, the durable `Client` (reconnection, exact terminal re-attach) and, behind `test-support`, an in-process test relay. |
| `leon-pty` | The GPUI-free part of terminals: `SpawnSpec`, grid maths and `PtyProcess` (a child in a pseudo-terminal driven by channels). `leon-term` re-exports it. |
| `leon-host` | The sharing service: executes commands, owns durable terminals with a replay ring, serves pairing and sessions through a relay, reconnects, cuts off revoked devices; the `leon host` command line. |
| `leon-term` | A terminal for GPUI: `Terminal` (PTY, emulator, child) and `TerminalView` (the GPUI entity). Depends on `gpui-kit` and nothing of Leon; the application maps its own command type onto `SpawnSpec`. |
| `leon-mark` | The Leon lion, always animated: `geometry` (the glare's four contours as point lists, exactly the owner's SVG at rest), `motion` (a pure, deterministic time-to-pose function: blink, glare, glance, breath, nose twitch, intro, and a `Mood` that biases them), `element` (`AnimatedMark`, a GPUI element painted from vector paths) and `svg` (the same gestures written as animated SVG for the web). Depends on `gpui-kit` and nothing of Leon. |
| `app` (`leon`) | The window (`ui/`, with `panes.rs` and `workspace.rs` for the terminal layout and `menu.rs` for the context menu), the engine that keeps the store fresh (`engine.rs`), the one shortcut registry (`keys.rs`), the themes and their design tokens (`theme/`), what a live session runs (`launch.rs`), the hidden `--diagnose` run (`diagnose.rs`). |

## Connecting a machine

The "Connect a machine" screen (`ui/connect.rs` state and keys,
`ui/connect_view.rs` drawing, `connect.rs` what it says) is an overlay like
Settings: `Overlay::Connect`, closed with `Esc` to where the keyboard was. It
adds a machine, edits one (same form, pre-filled), explains why a machine is
offline (`Phase::Why`) and says what to do next (`Phase::Next`). It replaces
the old two-step palette flow; `Command::AddMachine`, `EditMachine` and
`WhyOffline` all open it.

* **Pure parts.** `connect::split_host` splits the host field while it is typed;
  `connect::Form` validates the fields and yields a `leon_remote::connect::Target`;
  `Target::ssh_line` and `Target::copy_id_line` are the exact, quoted command
  lines shown and copied. `leon_remote::diagnosis::classify(status, stderr)` is a
  table-tested classifier from ssh's exit status and message to a `Diagnosis`
  (kind, title, explanation, fix); it never suggests overriding a changed host
  key.
* **The checklist.** `leon_remote::connect::run_checks` goes through a `Runner`
  (so tests script it): one `ssh -o BatchMode=yes ... true` (ssh ends with 255
  for its own failures, so its message says which of reach, host key and login
  stopped), then the machine probe for the shell, `git` and the agents. It reports
  progress after each step. `Engine::check_connection` runs it on the engine's
  runtime and returns a `CheckRun` (a `watch` of the checklist); dropping the run
  cancels it and kills the `ssh`. `Engine::save_machine`, `mark_online` and
  `find_repositories` (a bounded `find` for `.git` below the home or start
  folder, also cancellable) complete it.
* **`~/.ssh`.** Everything the screen reads of it goes through the `SshDir`
  trait (`Options::ssh_dir`): host names of `config` and `known_hosts` (hashed
  entries, wildcards and addresses skipped), the *names* of the `*.pub` files,
  never any key's contents. The only write is `append_known_hosts`, called after
  the person confirms "Trust this computer" with the lines `ssh-keyscan` returned
  (their SHA256 fingerprints are shown first). Tests use an in-memory
  `SshDir`.
* **An offline machine** explains itself where it is seen: activating its row or
  `Why is it offline?` opens the screen with the checklist rebuilt from the
  recorded failure (`Checklist::from_failure`), and `Test again` runs the real
  one. The start folder is not stored; it only tells the search for repositories
  (offered as folders by "Add a project", through `World::repositories`) where to
  start.

## Remote machines through a relay

Leon drives a machine through two primitives: run a command, and an interactive
terminal. A host offers just those over an encrypted channel; everything above
them (git, probing, icons, the process scan, history) is unchanged.

* `MachineKind::Relay { host_id, host_key, relay_url, name }` is stored in
  three extra columns of `machine` (migration 4; existing rows are untouched).
* `run_on` / `interactive_on` mark a relay machine's command with a route (host,
  pinned key, relay) and leave it as written. `leon_remote::RoutingRunner`, the
  engine's one runner, starts unmarked commands here and sends marked ones
  through the machine's durable `leon_link::client::Client` in a
  `leon_remote::RelayHub`. SSH and local machines are untouched.
* The terminal `Backend` of the application, `remote::RoutingBackend`, starts
  unmarked commands in a PTY and marked ones as `Terminal::remote`: the same
  emulator fed by the client's ordered, de-duplicated output stream, with input,
  resizes and hang-up sent to the host. Dropping or quitting detaches (the
  program keeps running on the host); closing a tab hangs it up.
* `pair.rs` (what "With a code" says and does), `share.rs` (the in-process host
  behind "Share this machine"), `ui/pair.rs` and `ui/share.rs` are the two
  overlays. Connection state per machine comes from the hub and is explained by
  `Why is it offline?`.

The relay server is operated by Zavu and is not part of this repository; the
protocol it speaks is specified in `docs/REMOTE.md` and `leon-wire`'s `relay`
module. See `docs/REMOTE.md` for the security model.

## Rules

1. **The UI only reads the local store.** `Snapshot::load` reads machines,
   projects, worktrees and sessions; views never run a command and never wait
   on a machine. The store announces each write and the UI reads again.
2. **The engine writes the store.** Imports, git, SSH probes and worktree
   changes run on a Tokio runtime and end in store writes and a status line.
   The UI asks for work (`Engine::submit`) and never receives data from it.
   Live terminals are the one thing the UI owns directly: they are processes
   attached to the window, not data, and they live in `ui::live::Sessions`.
3. **Tokens only in `theme/`.** Every colour, size, radius and font the UI
   uses is data of a theme, defined there once per theme (see "Themes");
   so are the terminal's 16 ANSI colours (`Palette::terminal`). `leon-term` has no colour literal except the xterm
   256-colour cube.
4. **One shortcut registry.** `keys::BINDINGS` is the only place a chord is
   written. The key handling, the palette, the shortcuts sheet and the README
   table read it; tests check that no two commands share a chord and that the
   README lists each one.
5. **Pure first.** What can be decided without a window is a pure function with
   tests: the tree (`ui/tree.rs`), the palette's questions (`ui/steps.rs`), what
   a session runs (`launch::plan`), key to bytes (`leon_term::keys`), cells to
   runs (`leon_term::layout`), resize maths (`leon_term::size`).
6. **English everywhere, prose `//!` header on every file.**

## Usage limits

How much of each agent's limit is left is read **per machine**, because usage
belongs to the account on the machine where the agent runs. It follows the rules
above: the engine writes the store and the UI reads it.

* **Collection.** `Engine::collect_usage` (`Op::CollectUsage`, also run in each
  machine's refresh) calls `leon_usage::collect_machine` through the engine's
  runner: one POSIX command per machine prints which agents are there and the
  newest `token_count` lines of Codex's own session log (a bounded read, no
  credential, no path, no conversation text beyond those lines). Remote machines
  take the same path through `ssh`. A machine that cannot be reached keeps its
  earlier reading (`agent_usage::merge`), whose age is shown and judged.
* **Sources, least intrusive first.** Local files the agent already writes
  (Codex's session log); the agent's own command line (Antigravity's
  `agy -p /usage`, only after `agy --version` says it is a metadata read, run
  through the runner on any machine); the vendor's usage endpoint with the
  agent's own credential (Claude Code, the opencode Go subscription, Codex's
  backend when its log is more than ten minutes old, Grok, Cursor, Kimi, ZCode),
  **on by default, off per agent in Settings** (the switches are generated from
  the catalogue: `usage_<id>_network`). Only Claude Code and the Codex log have
  been verified against a live service; the rest are implemented from Orca's
  source (`src/main/rate-limits/`) and say so. Each provider is a module of
  pure parsers (`grok`, `cursor`, `kimi`, `zcode`, `antigravity`, `codex`,
  `claude`, `opencode`) plus one function in `network.rs`; every vendor host is
  in `ALLOWED_HOSTS` and a test pins that an unlisted host is refused. An
  expired stored sign-in (a 401, a 403 that is not a missing scope, or a credential with only a refresh token) is `Reason::SessionExpired` and is never refreshed; `Reason::keeps_numbers` says which failures leave the earlier numbers on show (`agent_usage::merge`), marked with their age; `NotSignedIn` is only for no credential at all, with `MissingScope`, `ApiKeyBilling`, `NoSubscription`, `KeyRejected` and `SpendsATurn` (Antigravity's latch, per machine, for the session) as their own reasons. Those run on this computer only and through the `Http` trait
  (`CurlHttp`: the system `curl`, the header on standard input so the token is
  never in a process list, HTTPS to one allowed host, no redirects, a time limit);
  a disabled source reads no credential and makes no call (tested with a
  scripted client that counts). The engine holds the live `NetworkPolicy`
  (`Engine::set_usage_policy`, applied from the settings on every change, which
  also asks for a read at once, `Op::CollectUsageNow`), at most one collection
  at a time (so one request per source), a `Throttle` per source (exponential
  back-off with jitter, `Retry-After` honoured, at least `Throttle::RATE_LIMIT_MIN` (5 minutes) after a 429, cleared when the source is
  switched or the user chooses Try again), and a keychain refusal remembered for
  the session. Nothing is read until the window is up (`defer_usage` /
  `start_usage`); the schedule (`agent_usage::due`) reads every
  `usage_refresh_seconds` (600, at least 30) in a focused window only; a scheduled read never calls a vendor within `network::MIN_GAP` (60 s) of its last call (`UsageSetup::last_called`), opening the usage view reads once when the reading is older than the interval, and a manual read goes through. The credential is a `Secret` (no `Display`, no
  `Serialize`, `Debug` prints `***`), read at the moment of the call, dropped with
  the request, and a failed call backs off (`Throttle`) and is shown as unknown,
  never as a number.
* **Staleness.** `AgentUsage::effective(now)` turns a reading into what it means
  now: a window whose reset time has passed reads 0% and says it reset since it
  was last seen; a window with no reset time is trusted for two hours; a reading
  older than eight days is unknown (`DataTooOld`). Time is always injected.
* **Store.** Migration 4: `usage_reading` (the latest reading per machine and
  agent, one JSON document) and `usage_history` (window key, time, percentage,
  under a local hash of the account, bounded to 14 days and 300 points a series;
  `Store::forget_usage_history`). `StoreChange::Usage` announces both.
* **UI.** `agent_usage::Board` is what the window reads. `ui/usage_view.rs` holds
  the pure model (`bar_model` with its `BarStyle`: detailed, every window, as Orca's footer, or compact; `density`; `usage_rows`, worst first; `step_scope`; tested
  without a window) and the drawing: the footer strip under the main pane (it
  absorbs the status line, and sits level with the sidebar's tools), the usage
  view overlay (`Overlay::Usage`, `Command::ShowUsage`) and the chip in a live
  session's header. A level is a colour token of the theme **and** a marker. Percentages go through one function (`present::percent_round`, half away from zero, clamped) and `PercentDisplay` (`usage_percentage_display`: used or left; levels always judge what is used). The default thresholds are Orca's, 60 and 80.
  `agent_usage::start_notice` words the line shown when a session of an agent at
  its critical limit starts.
* **Settings.** The `Usage` section of the schema: the bar, the agents shown,
  the interval, the thresholds, the notice, the two network opt-ins (each with
  the exact text of what is read and where it is sent) and "Forget stored usage
  history".
* **Differences from Orca, on purpose** (also in the README's Usage section): no hidden sessions, PTY scraping or `codex app-server`; no refreshing or rewriting of any CLI's credentials; no pasted cookies or multi-account switching; an honest `User-Agent: Leon/<version>` instead of imitating `claude-code` or `codex-cli` (only the protocol headers `anthropic-beta`, `OpenAI-Beta` and `ChatGPT-Account-Id` are sent); a 10 minute default refresh with a 60 second per-vendor floor; the `CLAUDE_CONFIG_DIR` keychain suffix is taken without NFC normalisation.
* **`leon --diagnose usage`** runs the real collection for this computer and
  prints no account, e-mail, token or path; its network sources follow the
  settings (on by default), overridden by `--network <agent>` and
  `--no-network <agent|all>`.

## Finding agent history (`leon-history`)

Each agent keeps its sessions its own way and Leon reads them without ever
writing to them:

* **Claude Code**: `<CLAUDE_CONFIG_DIR or ~/.claude>/projects/<project>/<id>.jsonl`;
  files deeper down are sub-agent transcripts (counted, not listed).
* **Codex**: `<CODEX_HOME or ~/.codex>/sessions/Y/M/D/rollout-*.jsonl`.
* **opencode**, from the source of `sst/opencode` and a real 2.0.14 database:
  the data folder is `xdg-basedir`'s `xdgData` + `opencode` on every OS
  (`$XDG_DATA_HOME`, else `~/.local/share`; Windows included). The database is
  `opencode.db`, but a build of another channel names it
  `opencode-<channel>.db` and `OPENCODE_DB` can move it (absolute, or relative
  to the data folder). Leon reads every `opencode*.db` beside the configured
  one. The database runs in WAL mode with `synchronous = NORMAL`: a committed
  turn survives a hang-up and readers see it through the `-wal` file. A
  database holds up to two **generations** of tables, and Leon reads every
  generation it finds: **v1** (`session`, `message`, `part`, opencode 1.x) and
  **v2** (`session_v2`, `session_message`, opencode 2.x, one row per turn with a
  `type` and a JSON payload whose assistant content is a list of parts). An
  upgraded database keeps the v1 tables behind, usually empty, so the sessions
  of a 2.x install are only in the v2 tables; a mixed one can hold different
  sessions in each. A session id present in both is imported once, from v2.
  (An earlier version of this audit read the repository's `session` table and
  concluded the v1 tables were still the ones written; they are not for 2.x,
  where the v1 tables stay empty. That is what made new opencode sessions
  vanish.) The oldest layout is JSON files under `storage/`
  (`session/<project>/<id>.json`, `message/<session>/<id>.json`,
  `part/<message>/<id>.json`); opencode leaves that folder behind when it moves
  to SQLite, so a session in both is imported once, from the database. A session
  row exists before its first message; it is imported as soon as it gains one
  (the fingerprint covers the session's update time, its latest message and its
  parts or turns). Sub-agent sessions (`parent_id`) are not listed but are
  counted in the report, per generation.

`HistorySource::survey` produces the counts and reasons behind `leon --diagnose
history`; `HistorySource::stamp` is a metadata-only change marker (it includes
the `-wal` file) so a poll can skip an import when nothing moved. A source whose
layout is not known is an error that is reported, never "no sessions".

## Remembering and restoring the open terminals

`leon-core`'s `store/workspace.rs` keeps a `SavedState` (migration 8): one row per
terminal, tab and workspace, in two slots, `Current` (what the running window
writes) and `Previous` (the last run's, copied at start so declining or opening
something new never loses it). It holds machine, folder, agent id, the agent's
own session id with how sure Leon is (`resumed`, `state-file`,
`newest-in-folder`), the history row, the user's name, the tab layout tree with
ratios, focus, zoom, and a clean-shutdown flag; never scrollback.

In the window, `restore_view.rs` compares `snapshot_state` with the last write on
every render and writes after `Options::save_debounce` (500 ms); programs that
ended are left out with their pane. `flush` (quit, close, update restart) writes
once more with `clean_shutdown = true`, before anything is hung up; the start
clears the flag. `restore.rs` is the pure part (layout conversion, the rows and
sentence of the question).

Restoring builds every terminal at once as the base shell (`spawn_live`), puts
them in the saved layout, and decides per terminal with `launch::plan`: a plain
shell or a launch-only agent becomes a shell (the latter with a note, never a
fresh agent), a resumable agent with a known session id becomes a **paused**
session whose resume line is held in `LiveSession::pending`. It is typed when
its tab is shown (`open_live`), on Enter in it, or in the background three a
second with `restore_resume = all`. Anything that cannot be reopened (unknown
machine, missing folder, agent not installed, unknown session id, the session
already running in another terminal per the elsewhere scan) is listed with its
reason. A paused session has no agent for the activity dot and sends no
notification. Known limits: the sidebar selection is restored only as the
terminal that was on screen; relay terminals are not re-attached yet (the host
keeps them: `Client::pty_list` / `pty_attach`); scrollback is not stored (the
seam is `SavedTerminal`, which can carry a screen snapshot later).

### Learning a fresh session's id, and importing promptly

A session started fresh in Leon has no id. `learn.rs` (pure) matches it without
reading the screen: **by process** (the process scan names the session a live
agent holds, from Claude Code's own `~/.claude/sessions/<pid>.json` or the
agent's arguments, and the terminal's shell is an ancestor of that agent; stored
as `state-file` / `arguments`) and **by folder** (Codex, opencode and the rest:
the sessions of that folder created after the terminal started; with one
terminal the newest, with several only a session that exactly one of them could
own, never two terminals on one session; stored as `newest-in-folder`). The link
also sets the terminal's history row, so the tree shows one row, live now and
history later. `history_sync.rs` keeps the history fresh: `Op::SyncHistory` (an
incremental import, quiet unless a source is unreadable, skipping sources whose
`stamp` did not move, `-wal` file included) is asked for, after a 2 s pause that
merges bursts, when an agent starts, goes quiet after output, or ends, when the
window regains the focus, and every minute.

### Quitting gently

Quitting (the Quit command, closing the window, restarting to update) first
writes the state of what is open (marked as ended normally), so the agents that
were running are what a restore resumes; then each local terminal with an agent
in front of its shell is sent the agent's own exit line (the catalogue's `exit`:
`/exit` for Claude Code, verified in its command table, and for opencode,
verified in its source; Codex and the rest have none and skip this step), the
ones still in front after 1.2 s get SIGTERM on the terminal's foreground process
group, and after 2.5 s in all whatever is left is hung up as before (SIGHUP,
then SIGKILL of the group). All terminals are handled in parallel, the wait ends
when every agent is gone, the status line says "Closing N sessions…", and
nothing can hold the quit longer than the grace. Terminals on other computers
are let go of (their program keeps running there), as before. Windows cannot
tell the foreground program, so there is no gesture or signal there: the
hang-up is as before.

## Notifications

When a session wants the user or ends, Leon says so twice: the **geek banner**
over the window (monospace, the agent, the folder, the exit code) and the
**desktop notification** of the system. Both are the same [`Note`], and which
events and which of the two ways are said is the `Notifications` section of the
settings (`notify`, `notify_waiting`, `notify_finished`, `notify_failed`,
`notify_how`, `notify_only_unfocused`), read by `settings::notifications`.

* **The events are pure** (`ui/notify.rs`): `Event::of_exit(code)` decides
  between a clean end and a failure, `Event::line` words it, `notify::note`
  composes the title (`label · folder`) and the body (`what happened · machine`
  for a remote one), and `notify::system_notification` turns it into the
  notification GPUI shows, with the session as its stable tag so a newer note
  replaces the older one in the notification centre.
* **Where it is raised** (`ui/terminals.rs`): `Shell::set_activity` watches the
  activity transitions, so the change *to waiting* (quiet, or the bell) is one
  event; `ViewEvent::Exited` is the other, with the exit code. `Shell::raise`
  drops the event when the settings do not ask for it or when the session is on
  screen with the window in front (the dot and the lion are already saying it),
  then pushes a `Banner` and/or hands the note to `Options::notify`.
* **The banner** is shell state: at most four, the oldest goes first, each with
  when it expires. `Shell::keep_banners` runs one timer while any is on screen
  and ends itself with the last one; the clock is the executor's
  (`BackgroundExecutor::now`) so tests advance it. Clicking a banner opens the
  session it came from; the cross dismisses it.
* **The desktop note** is `cx.show_system_notification` (GPUI: notify-rust on
  Linux, the Notification Center on macOS, a toast on Windows);
  `main` sets the application identity first, which is what names Leon in the
  notification. `Options::notify` is the seam: tests record the notes instead.
  `notify_only_unfocused` keeps the desktop for when the window is not in
  front; the banner still appears for a session that is not on screen.
* **Tests** (`ui/tests_notify.rs`) drive the scripted terminals through
  waiting, a clean exit and a failure, and hold the settings, the focus rule,
  the dismissal and the expiry to what they say.

## Project logos, the filter and the activity dot

**Logos.** `leon_core::icon` says which images are accepted and recognises them
by content (PNG, WebP, ICO, JPEG, SVG; 256 KiB). The store keeps up to two rows
per project in `project_icon` (migration 3): `detected`, written by the engine,
and `custom`, the user's file, which wins when reading; the bytes sit beside
their content hash and the snapshot carries only the metadata
(`Snapshot::icons`), the UI reading the bytes by hash, once, on a background
thread (`ui/logos.rs`). Detection is `leon_remote::icon::detect`: a table of
ordered `IconRule`s (marker: a file or a `package.json` dependency; candidate
files and config files that declare an icon) evaluated per package root, then
a generic list; one POSIX script, run through `run_on`, so a remote project
takes the same path as a local one, reports the files that exist, the text of
the small config files and the images that pass a magic-byte check, in one round
trip (a second only for an icon a config file names that was not looked at). The
engine (`Engine::detect_icon`) adds the owner's avatar from the origin remote
through an injected `IconFetcher` (`avatar.rs`; the default `NoFetch` touches
no network, `CurlFetcher` is the only network call from the UI machine), and
stores the result with the rule that matched (`nextjs-app:app/icon.png`). It runs
when a project is added or discovered, for projects never detected at refresh,
and on `Op::DetectIcon`; `Op::SetIcon` and `Op::ResetIcon` are the user's choice.
Images are drawn by the toolkit's `img`, which decodes off the UI thread and
caches by the image's content id.

**Filter.** `ui/filter.rs` is pure: `filter(snapshot, labels, query)` and
`project_labels(snapshot)` read the in-memory snapshot, and
`tree::build_rows_filtered` flattens the matches without touching the saved
expansion. The field is a text input above the list; the shell recognises that
it has the keyboard from its focus handle.

**Activity.** `ui/activity.rs` is a pure function from what a terminal says
(exit, quiet time, foreground, bell) to `Off < Idle < Working < Waiting <
Failed`; a worktree shows the most urgent of its terminals, a project of its
worktrees. A session's state is refreshed on its terminal's wake-ups and by one
coarse timer that exists only while a terminal is live; the shell repaints only
when a state changed. Thresholds are in `Options::activity`.

**Agent colours** are theme tokens (`Palette::agent_*`), a deliberate exception
to a theme's accent rule, set per theme so that each theme can choose its own
(monochrome included). Nothing outside `theme/` assumes a particular theme.

## Settings

Settings are a typed schema, not a struct. `crates/app/src/schema.rs` holds
`SETTINGS`, one `Def` per setting: stable key (the key of `settings.json`),
section, label, one-line description, keywords, kind (toggle, choice, number
with range and step, text, path, list, action), default and platform. Everything
else is derived from it:

* `schema::Store` reads and writes `settings.json`. It keeps every key of the
  file, the unknown ones included, so saving never loses what a newer or older
  build wrote; a missing key is the default, a value of the wrong type or outside
  its choices is the default for that key alone with a `Problem` the window
  reports, a number outside its range is clamped. A value equal to its default
  is not written. A file that is not a JSON object leaves what was in force.
* `settings.rs` owns the values in use (a GPUI global): `value`, `flag`, `int`,
  `text`, `list` read, `set_value`, `reset_value`, `reset_all` change, save
  atomically and bump `generation`. `poll_file` applies an edit made outside Leon
  once two reads agree on it. The five settings of the window's look keep their
  `Settings` view (`get`), and `--theme` / `--theme-name` still win until a pick.
* `ui/prefs.rs` is the window's side: it polls the file with the themes folder's
  timer, reports problems, and `Shell::sync_settings` (run from `render` when
  `generation` changed) puts the engine's `Prefs`, each live terminal's
  scrollback, cursor, Option-as-Meta and copy-on-select, and the tree's length on
  what already exists; the terminal font is read from the settings on every
  render and re-measured by the terminal once per paint. New terminals and
  sessions read the settings when they start (`launch::plan_with`).
* `ui/settings_screen.rs` draws the screen from the schema, so a new `Def`
  appears with its control, its default and its modified marker. Keyboard
  routing is `Shell::settings_key`; the entries (`entries`) are pure and tested.
* The palette lists every `Def` under `[ SETTINGS ]` (`Item::Setting`) and edits
  it with `steps::advance_setting`, a step of the kind's own shape.
* `docs/SETTINGS.md` is `schema::render_docs()`; a test fails when it is stale
  (`LEON_BLESS=1 cargo test -p leon the_settings_reference_is_current` rewrites it).

**Adding a setting**

1. Add a `def(...)` to `SETTINGS` with a label, a description that ends with a
   full stop, keywords, a kind and a default; the schema tests fail without them.
2. Read it where it matters (`settings::flag(cx, "key")` and friends), and if it
   changes something that is already alive, apply it in `Shell::sync_settings`
   (or `settings::apply_look` for what is global to the interface thread).
3. Write the test that proves the effect, regenerate `docs/SETTINGS.md`.
4. A value that must be checked against the computer (a font, a program, a
   folder) goes in `prefs::check_value`.

## Themes

Themes are data. `theme/mod.rs` holds the model and the registry, and one file
per theme holds its values (`theme/zavu.rs`, `theme/leon.rs`).

* `ThemeId` is a small copyable handle on a theme's string id (`theme/registry.rs`).
  The built-in ids (`leon`, `zavu`) are reserved constants; user themes loaded
  from files live in the registry next to them, replaced as a whole whenever the
  themes folder is read. `slug()` is the id saved in `settings.json` and typed
  after `--theme-name`; `ThemeId::DEFAULT` is what a fresh install or an
  unknown saved id wears (the unknown id is noted so that the window says so).
  `registry::all()` lists every theme (built-in first, then user themes by
  name, invalid ones included) and `usable()` the ones that can be chosen: the
  palette, the cycle chords and `--theme-name` all read these.
* A `Theme` is a light and a dark `Palette` (every colour token, the agents'
  marks and the whole `TerminalTheme`), a `Typography` (UI and mono families
  and their OpenType features), a `Shape` (the metrics that differ between
  brands: corner radius, cell radius, label size, crosshair arm) and its `Lines`
  (the blueprint: which crosshairs, corner ticks, guides and empty-state frames
  it draws, how long and how heavy). Colours of the lines are palette tokens
  (`border`, `grid_mark`, `guide`).
* Views read what is active through `theme::palette(cx)`, `theme::metrics` and
  `theme::fonts`. `theme::apply(id, appearance, cx)` installs a theme, aligns
  gpui-component's `Theme` (colours, fonts, size, radius) and refreshes every
  window; the shell hands the new terminal colours and font to each live
  terminal on that repaint, and a terminal re-measures its cells and resizes
  its PTY only if the grid changed. Nothing is loaded when switching: every
  bundled font is registered at start.
* Appearance (system, light, dark) is a separate setting. `settings.rs` keeps
  both, plus the command line's overrides and a *preview*: the palette's
  "Choose theme..." and "Choose appearance..." steps show the selected value
  on screen without saving it, and closing the step clears the preview, which
  restores exactly what was kept.
* A theme's own rules belong to its file and its tests. Zavu spends one accent
  on focus, selection and the active marker and keeps its logo and primary
  button monochrome; Leon fills its primary button with the accent. That is a
  property of the theme, not of the application.
* The product's mark is the round-2 glare in every theme, Zavu's included:
  `brand::MARK` is one single-colour SVG layer (one silhouette with cuts)
  tinted with the palette's `logo` token and drawn by `widgets::mark`; the
  cuts show the surface behind it. At 20 px and below (after scaling)
  `brand::mark_for` picks the fitted `brand::MARK_SMALL`. A theme only
  chooses the one token; the contrast test
  `the_glare_is_legible_in_every_theme_and_appearance` holds it to 3:1.
* The application icon comes from one SVG (`assets/brand/app-icon.svg`):
  `scripts/generate-icons.sh` writes the PNGs, the `.ico` and the `.icns`
  into `assets/icons`, and `brand.rs` tests hold the files to the sizes each
  platform needs. What each platform does with it: macOS reads the bundle
  (`packaging/macos/bundle.sh`, executable `Contents/MacOS/Leon` so that the
  Dock label, Activity Monitor and the menu title read `Leon`; `cargo run`
  gets a development bundle from the runner `scripts/cargo-runner-macos.sh`,
  wired in `.cargo/config.toml`, which `exec`s the bundled binary and passes
  every other executable through; only a hand-started unbundled binary sets
  the Dock icon through `NSApplication.setApplicationIconImage`,
  `brand::set_dock_icon`, which skips a path inside `.app/Contents/MacOS/`;
  `tests/macos_packaging.rs` runs both scripts on dummy executables), Windows
  loads icon resource 1 of the executable (embedded by `build.rs`), Wayland
  reads the desktop entry named after the application id, and only X11 takes
  `WindowOptions.icon` (GPUI 0.3.7, `brand::window_icon`).

**Themes as files** (`docs/THEMES.md` is the user's guide).

* `theme/tokens.rs` is the one list of tokens: public key, table, kind, doc,
  how it is read and written. The file loader, the export, the new-theme
  template and the generated reference in `docs/THEMES.md` all read it, and a
  test fails when the docs' copy is stale.
* `theme/file.rs` parses a file (`toml`, a dependency the GPUI stack already
  compiles), resolves `extends` (cycles and missing parents are errors),
  lays the keys over the parent's theme and derives what the built-ins derive
  from the accent when the file sets only that. `theme/check.rs` holds the
  rules: errors make a theme invalid (not applied), warnings are reported. A
  file never panics the loader.
* `theme/user.rs` reads the folder (`themes/` beside `settings.json`) into the
  registry; a theme whose file became invalid keeps its last good version, so
  a broken save never leaves the window unreadable.
* `theme/watch.rs` decides, from listings of the folder, when a file changed
  for good (two listings agree). The shell lists it once a second
  (`ui/themes.rs`): polling needs no dependency and behaves the same on every
  platform, and the folder holds a few small files. Tests call
  `Shell::poll_themes` themselves with `Options::theme_poll = None`.
* `theme/author.rs` writes the new-theme template and the export.
* `ui/themes.rs` has the commands (new from current, export, open folder,
  reload, show problems) and the problems card.

**Adding a built-in theme**

1. Write `theme/<name>.rs` with both palettes, the typography, the shape and
   the lines, and add it to `registry.rs` (`RESERVED`, `ThemeId::ALL`,
   `theme()`, `name()`, `detail()`). `Palette` has no default: a missing token
   does not compile.
2. Bundle any new font in `assets/fonts` with its licence, list it in
   `brand::fonts()` and `assets/ASSETS.md`.
3. Add the id to `theme_ids_are_unique_and_stable` (`theme/tests.rs`) and the
   theme's own values to its file's tests.

The tests that fail until the theme is complete (they run over `ThemeId::ALL`
in both appearances): `text_stays_legible_on_every_surface_in_every_theme`,
`glyphs_and_marks_reach_the_graphics_contrast_in_every_theme`,
`the_accent_and_the_four_states_are_pairwise_distinguishable_in_every_theme`
(CIE76 distance of at least 20 for every pair of accent, success, warning, error
and info), `the_accent_fill_and_the_text_on_it_work_in_every_theme`,
`agent_marks_reach_the_component_contrast_on_every_surface_in_every_theme`,
`terminal_text_is_legible_on_the_terminal_page_in_every_theme`, the ANSI tests,
`every_theme_defines_every_token_of_its_own_with_nothing_borrowed_from_another`,
`every_font_family_a_theme_names_is_bundled`, `theme_ids_are_unique_and_stable`,
and the README test for the shortcuts if a command was added.

## The blueprint lines

`ui/lines.rs` draws wuapi's line system as part of a theme. The geometry is
pure (`lines::marks`, `lines::dividers`): where the window's rules are, derived
from the metrics and the pane layout that lay the panes out, so marks follow
the sidebar's width, the interface scale and the dividers. The drawing is a few
absolutely positioned boxes with no handlers (they take no mouse event) on
whole device pixels. `leon` draws crosshairs at every intersection, corner
ticks on framed surfaces and a frame and dimension line around empty states;
`zavu` draws only its original crosshair. The values are in `brand/tokens.md`.

## Menus, quitting and the sidebar

`menus.rs` generates the macOS menu bar from the registry of `keys.rs`: a
layout says which command goes in which menu; the label, the chord and whether
the item is enabled come from the registry. One action type, `menus::Run`,
carries a `keys::Command`; the shell runs it with the code its chord and the
palette use, after checking that it applies (the chord's context rules).
Hide, minimise, zoom, full screen, Services and Edit's text items are the
toolkit's. A test fails for a command in no menu and not listed in
`NOT_IN_MENUS`. Linux and Windows have no menu bar: every command stays on its
chord and in the palette.

Quitting (`Cmd+Q`, `Ctrl+Shift+Q`, closing the window; with an update ready, in the
automatic mode and with nothing running, it also puts the update in place on the
way out) asks, by the
`quit_confirmation` setting, only while a program runs in a terminal (or cannot be known, as over SSH; the default),
always, or never; then it hangs
the terminals up and flushes the state beside the settings. The sidebar's
visibility and width are in `settings.json`; `theme::metrics::SIDEBAR_WIDTH` is
zero while it is hidden, so every layout that reads it follows.

## Updates

`leon-update` does the work and knows nothing of windows; `updates.rs` is the
`Service` the application holds (it runs the updater on the engine's runtime and
hands results back through a channel) with the words for each state, and
`ui/updates_view.rs` is the glue: the footer's item, the release notes overlay, the
commands, the timer. The updater publishes a `Snapshot` over a `tokio::sync::watch`
channel; the window follows it.

The life-cycle is `Idle → Checking → UpToDate | Available | Manual | NoBuild →
Downloading → Ready → Installing → RestartRequired | Failed`. Applying an update is
done by a process that works, so that going back is dependable: *Restart to update*
puts the new build in place and tries it (`--version`) in the running process, the
window quits, and what is left of the process (`main` after the application's run
returns) starts the new build, lets it go ahead (the pipe it waits on is closed:
`wait_for_parent`) and watches it until it confirms or exits badly, in which case the
old one is put back. Quitting with an update ready in the automatic mode does the
same swap without starting anything; at the next start, in the automatic mode, an
update that is still ready is applied before anything is opened. A new build that
starts and keeps failing before it confirms takes itself out after three starts. All
of it is exercised on temporary folders through scripted GitHub, `Http` and `Tools`
(`launch.rs` tests, `updater/tests.rs`, and the window's `tests_updates.rs`).

Not in a development build (`target/`), not with `LEON_NO_UPDATE`, and no process is
started except through `leon_remote::spawn`. See `docs/UPDATES.md`.

## The terminal's buffer: find, clear, copy, save

`leon-term` owns the logic and the app owns the dialogs. `buffer.rs` reads the
scrollback and the screen as plain text or ANSI (wrapped lines joined, trailing
blanks trimmed), clears the history or the buffer inside the emulator, and
selects everything. `find.rs` searches with the emulator's own `RegexSearch`
(smart case, whole word, literal or regex, 10 000 matches at most) and keeps
the current match; the painter shows the matches in the same batched pass as
the selection. `ui/find.rs` is the bar, `ui/terminal_tools.rs` the commands, the
save dialog seam and the menu of a pane.

## Paste

`ui/paste.rs` decides a paste from the clipboard's entries (`plan`, pure):
image only sends Ctrl+V (byte 0x16) so that an agent CLI reads the image from
the system clipboard itself; text is pasted (bracketed on request); copied files
become quoted paths; both kinds prefer the image only with an agent in front.
The clipboard is read through `Options::read_clipboard` so tests inject theirs.
gpui-kit reports text, images and file lists on macOS and Windows and no image
on Linux. Remote terminals cannot use the local clipboard, so an image paste
there only explains that. Files dropped on a pane paste their quoted paths.

## The tree and the main pane

`Placement` decides which worktree each history session and each live session
belongs to (the deepest project root or worktree containing its folder wins);
`build_rows` flattens the open nodes into rows. Live sessions are rows of kind
`Live`, listed above the history of their worktree, never hidden behind "show
more". A row is identified by what it is (`NodeId`), not by its place, so the
cursor stays on its node.

A live terminal that was started from a history session (`LiveSession::history`)
whose folder and machine are the terminal's own has no `Live` row: the
history session's row is its row. `Placement::merged` holds those sessions;
`build_rows` skips their `Live` entries and keeps their `Session` rows past the
"show more" cap, and the sidebar draws the terminal's state and light on the
session row (the same `tree-live-led-<id>` the `Live` row has). The worktree
and project dots still read the terminal, because the `Live` entry stays in the
placement. A terminal resumed in another folder ("Resume in…") keeps its own
`Live` row, in the folder it runs in, still linked to its history session.

The main pane shows one of: nothing, a project, a worktree, a history
transcript, or a live terminal. The transcript is opened read-only and starts
nothing; it carries a line that says how to resume the session, or, when a
resume was asked for and could not be done, why.

### Opening a history session

`Enter` or a click on a session row (and a palette result that is the
session itself, or a history-search hit on its title) calls
`Shell::resume_session` (`ui/terminals.rs`):

1. **No duplicates.** `Sessions::of_history` finds the terminal started from
   this session, if any, and `open_live` focuses it (its tab, its pane, the
   keyboard). The link is `LiveSession::history`; it goes with the terminal, so
   after the terminal is closed the next open resumes again.
2. **What can be known at once.** `launch::plan` is asked for the resume
   `Launch::Agent { resume: Some(external_id) }`: on this computer the agent
   must be found and the folder must exist (`System`), over SSH the last probe
   must have the agent. `NoSuchFolder` and `NotInstalled` go to the fallback
   below.
3. **A remote machine** is asked in the background (`Engine::check_target`,
   a `JoinHandle<Target>`): the machine is probed unless it has been (the
   report is kept), the agent must be in the report, and the folder is looked
   at through the same runner by running `cd <folder> && exec true` there
   (exit 255 is ssh itself failing, any other failure is a folder that is
   gone). The UI is never blocked; the status line says "Checking…" meanwhile.
4. **Start.** `start_live` with the session's machine and folder spawns the
   base shell exactly as for any terminal and types the agent's resume line
   once the prompt is quiet; the terminal goes to the workspace of the
   session's worktree (`tree::workspace_root`, or the folder itself for an
   unsorted session): a new tab when it already has panes, else its first
   pane. The keyboard goes to the terminal.
5. **Fallback.** For a folder that is gone, a machine that is off or an agent
   that is not installed, `show_transcript_instead` opens the stored
   transcript with a `Notice` (shown at its top) and an error in the status
   line. For a folder that is gone the notice offers "Resume in…" (the
   `ResumeIn` flow in `steps.rs`: the other worktrees of the session's project,
   or every worktree of its machine for a session no project contains).

`OpenTranscript` (`Cmd+Shift+L` / `Ctrl+Shift+L`, kept in a terminal) shows the
transcript of the row's session, or of the session the focused terminal
resumed, without starting anything; `Enter` in the transcript resumes. A
history-search hit on a message opens the transcript scrolled to that message;
a hit on a title is the session and is resumed. When the agent writes new
messages the history importer picks them up at its next import, as before:
there is no second mechanism.

### Sessions running in another terminal

A session may already be held by an agent process Leon did not start (iTerm,
Terminal, tmux, another Leon). Opening it would start a second writer on one
history file, so Leon looks first.

* **The scan** (`leon_remote::processes`) is one command per machine through
  the same `Runner`: a POSIX `sh` script (PowerShell for this computer on
  Windows) that prints the process tree (`T pid ppid exe`), the processes that
  mention an agent (`A pid ppid tty etime command`), their working directories
  (`C`, from `lsof` on macOS or `/proc` on Linux, one call for all) and Claude
  Code's `~/.claude/sessions/<pid>.json` (`S`). `etime` is read instead of a
  start date: it has one shape on macOS and Linux and no time zone. Parsing
  (`parse_scan`), argument reading per agent (`session_use`), ancestry
  (`Scan::descends_from`) and the terminal application above a process
  (`Scan::owning_app`) are pure and table-tested.
* **What ties a process to a session.** Claude Code: its state file names the
  session for the pid (the file keeps the process's start, so a recycled pid is
  not believed), else `--resume`/`-r`/`--session-id <uuid>`. Codex:
  `codex resume <uuid>`. opencode: `--session`/`-s <id>`. `--continue`/`-c` and
  `resume --last` mean the latest session of the folder. A forked resume writes a
  new session, so its id is not claimed. None of the agents holds its history
  file open (`lsof` shows nothing), and Codex's interactive process talks to a
  shared app-server daemon, so a bare `codex` or `opencode` has no certain
  signal on disk.
* **Confidence** (`elsewhere::resolve`, pure): *certain* is a name (state file
  or arguments); *likely* is the only unnamed process of its agent in a folder
  and the latest session of that folder (`--continue`), or the one written after
  the process started. Two competing processes, or a session another process
  holds for certain, get no guess.
* **Whose it is.** On this computer a process below this Leon's pid is Leon's own
  (`Found::leon_child`); a second Leon's is not. Over SSH Leon cannot see its
  own processes, so `elsewhere::foreign` takes off one process per live Leon
  terminal of that machine (by session, else agent and folder).
* **The engine** keeps the last result per machine (`Engine::elsewhere`),
  announces a change as `EngineEvent::Elsewhere` and forgets what a failed scan
  knew (nothing is claimed that is not known). It scans only when a scanner was
  installed (`set_process_scanner`): `main` installs one, tests do not. It runs
  on `Refresh` for every machine that answers, on `Op::Scan`, on a timer while
  the window is focused (this computer every five seconds, SSH machines every
  sixth tick), when the window regains focus, and before a session is opened.
* **The UI** never counts these as live sessions (they are not in `Sessions`,
  the activity roll-up or the quit count). The tree row wears
  `widgets::elsewhere_mark` in the `elsewhere` theme token (filled when certain,
  outlined when likely) with a tooltip; `Shell::resume_in` scans first, off the UI
  thread, and a held session opens its transcript with a notice
  (`ElsewhereNote`) and the choices *Open transcript*, *Resume here anyway*
  (`Command::ResumeAnyway`, a flow that asks first when certain) and *Reveal*
  (`Command::RevealTerminal`, macOS, only when the process tree names a `.app`).
  A scan that fails opens the session as before. A session live in Leon and
  also held elsewhere says so in the header.
* `leon --diagnose sessions-elsewhere` runs the real scan and prints what it
  found: pids, agents, starts, ancestry, session ids, confidence and signal.

## Live sessions, panes and tabs

`Shell` owns `Sessions`, a list of `LiveSession`s each holding the
`Entity<TerminalView>` of a running terminal, and `Workspaces`, pure data that
says how they are laid out (`ui/workspace.rs`, `ui/panes.rs`):

* a **workspace** is one folder of one machine: the worktree a terminal was
  started in (the deepest worktree or project root that contains its folder,
  `tree::workspace_root`), or the folder itself;
* a workspace has **tabs**; a tab is a **layout**, a binary tree
  `Split { axis, ratio, a, b } | Leaf(terminal)` with the focused pane and a
  maximised flag;
* every operation the keyboard and mouse offer is a method on the tree with
  unit tests: split, close (the sibling takes the room), neighbour by
  direction (the pane that touches the edge and shares most of it), next and
  previous in reading order, resizing a divider in fixed steps and never below
  the minimum pane (20 by 5 cells), dragging a divider (the pointer's
  position as a ratio, clamped the same way), equalising, maximising.

Selecting another node swaps what the main pane draws; the terminals not on
screen keep running and keep their grids current on their own threads, so
nothing restarts. The tab on screen draws its layout as nested flex boxes
with one-pixel dividers (a wider invisible handle takes the mouse); each
terminal paints into its pane and resizes itself, PTY and emulator, when the
bounds it is painted in change. That resize happens inside the paint, so
however many events a drag produces, there is at most one per frame.

### A terminal is a base shell

`launch::plan(machine, probe report, folder, what)` returns a `Plan`:

* `spawn`: the machine's **interactive login shell** in the folder. Locally
  `$SHELL -l -i` (PowerShell, else `COMSPEC`, on Windows); remotely `ssh -t`
  running `cd <folder> && exec "${SHELL:-/bin/sh}" -l -i`, so the remote
  login shell lands in the folder (the `cd` is part of the remote command
  rather than typed, so nothing shows on screen and it cannot race the
  prompt; this is a deliberate deviation from "then `cd`");
* `send`: for an agent, its command line quoted for that shell (`claude`,
  `claude --resume <id>`, `codex resume <id>`, `opencode --session <id>`;
  POSIX, PowerShell and `cmd` quoting), typed once the shell has printed its
  prompt and been quiet for 250 ms (or after 5 s). "Open shell here" and
  splits send nothing.

The shell's own `PATH` resolves the agent. The agent is still checked first
(this computer searched, an SSH machine's probe consulted) and refused with a
clear status when it is not installed. When the agent exits the terminal
stays at the prompt; only the shell ending (exit code shown in the row) or
closing the pane ends it.

What the row says is only what is known. The label is the name the user gave,
else the title the program set (OSC 0 or 2), else the agent's name *while it
was started as one*, else "Shell". On this computer the terminal can ask
whether its own process (the shell) is the foreground process group of the
PTY (`tcgetpgrp`): the phases *launched*, *running* (a program was seen in
front of the shell) and *returned* (the shell got the terminal back) are
claimed only in that order and only from that evidence. No marker is printed
into the terminal. Over SSH the PTY's process is `ssh` whatever happens on the
other side, so the phase stays *launched* and Leon says nothing more.
Closing asks only when something is running in front (or, where that cannot
be told, always).

### Keyboard

While a terminal has the keyboard (`Pane::Main` with a live session open and
no overlay) the shell's key interceptor first asks the registry
(`keys::resolve` with `Context { terminal: true }`) and only chords that
`keys::kept_in_terminal` allows can match: on macOS every `Cmd` chord, on
Linux and Windows only chords that also hold `Shift`, plus the scrollback
keys. Chords are per platform (`mac(..)` and `other(..)` in the registry,
and `alt` / `control` modifiers), which is how the iTerm2 chords coexist with
`Ctrl+Shift` equivalents. Every other key goes to `TerminalView`, which turns
named keys, Ctrl and Alt into escape sequences; plain text is left to the
platform text input path (`EntityInputHandler`), which is what makes dead
keys, input methods and non-US layouts work. The full table is in the header
of `keys.rs` and in the README.

### The context menu

`ui/menu.rs`. `items_for(kind, local)` is a pure function from a row to its
items; each item is a `keys::Command` (plus an agent for the agent submenu),
so the chip is `keys::keys_label(command)` and the commands are the
palette's. The menu moves the tree's cursor to its row and the command then
acts on the cursor, whether it came from the menu, the palette or its own
chord; removals reuse the palette's confirmation flows with the "which one"
question already answered.

### Opening a project

On this computer every entry (`Cmd+O`, the palette, the empty-state row, the
menu) ends in `Shell::open_project_on`, which calls
`system_picker` -> GPUI's `prompt_for_paths`; there is no typed-path flow for
this computer, and no fallback to one when the platform has no dialog (that
is an error status). Typing a path remains for SSH machines.
`leon --diagnose folder-dialog` calls the same function and prints that the
prompt was reached.

## The lion (`leon-mark`)

The product's mark is always the animated lion: `ui::widgets::mark` paints
`leon_mark::AnimatedMark` in the theme's `logo` colour (no picture is bundled;
`assets/brand/leon-mark*.svg` are the sources its shapes are tested against, point
for point). At rest it is exactly the owner's drawing. It does one small thing at
a time, with rests between: a blink every four to nine seconds, now and then a
glare (the eyes narrow and hold), a glance with a lean of the head, a breath, a
twitch of the nose; on launch it resolves from two slits opening in the dark. It
is never cute: no bouncing, no smile.

The header's lion carries the state of the work (`ui/lion.rs`): working while any
session is, else waiting while any wants the user, a few seconds of error when a
session exits with one, asleep (eyes nearly closed, nothing moves) when the window
has no focus. Hovering it narrows its eyes. "Animate the lion" and "Reduce motion"
(follow system, on, off) still it; still, it is the rest pose.

Cost: a frame is requested only while a part moves; between gestures one timer
sleeps until the next one starts, and nothing is drawn for it. An idle lion asks
for about 400 frames a minute at 60 Hz, about a ninth of the time. A still lion,
an asleep one, a window without focus, a mark that is not drawn: nothing at all.
The motion is a pure function of time (`Timeline::frame` returns the pose and a
`Wake`), so it is tested without a window, and a test follows a simulated minute
of frames and timers. `cargo run -p leon-mark --example preview` shows every mood
at every size, on dark and light; `cargo run -p leon-mark --example export-svg`
writes `brand/logo/final/mark-animated.svg` and `app-icon-animated.svg` (for the
website and docs; the app does not use them) from the same geometry and gestures,
and a test fails when the committed files differ from what it writes.

## The terminal (`leon-term`)

```text
child <-> PTY <-> reader thread --chunks--> parser thread --> Term (grid, scrollback)
                      ^                          |                    |
                      |                          +-- replies -------->|
                 writer thread <-- keys, paste, replies          TerminalView paint
```

* **Emulator**: `alacritty_terminal` 0.26 (`Term` plus the VTE parser), 10 000
  lines of scrollback. **PTY**: `portable-pty` 0.9 (ConPTY on Windows).
* **Threads**: a reader forwards PTY output over a bounded channel (a
  flooding child is slowed, not buffered without limit); a parser thread
  applies it to the grid under a lock and flushes a synchronized update (DEC
  2026) that never ended; a writer owns the PTY's input so neither the UI nor
  the parser can block on a full PTY; a waiter reaps the child.
* **Replies**: the emulator's answers to a program's questions (cursor
  position, device attributes, colour queries, window size) go back through
  the writer. Colour queries are answered from the theme, which is how
  full-screen programs choose a light or a dark style. A program reading the
  clipboard (OSC 52 read) is refused.
* **Repaints**: output sets a dirty flag and wakes the UI only on the change
  from clean to dirty; the paint clears it. A flood causes at most one repaint
  per frame. Title, bell and exit use a second flag so that a terminal nobody
  is drawing still reports them.
* **Painting** (`view.rs`, `layout.rs`): the visible cells are streamed under
  the lock into a `Batcher` whose buffers live across frames: neighbouring
  cells of one background become one rectangle, neighbouring cells of one
  style become one run shaped once with every glyph forced onto its cell; a
  wide character is its own run; blank cells are never shaped. The lock is
  released before anything is drawn. The cursor (block, beam, underline,
  hollow when unfocused), selection, bold, italic, underline, strike-through,
  inverse, dim and 16 / 256 / truecolor are drawn from a `TerminalTheme` the
  application supplies.
* **Mouse**: the wheel scrolls the scrollback, is reported to a program that
  asked for the mouse, or becomes arrow keys on the alternate screen. Click
  and drag select (double click a word, triple click a line); Shift always
  selects.
* **Cleanup**: dropping a `Terminal` hangs the child up (SIGHUP, or
  termination on Windows) and, if it ignores that, kills its process group
  after 1.5 s; the waiter thread reaps it, so no zombie is left.

## Not built yet

* Cursor blinking, hyperlinks (OSC 8) and URL detection, kitty keyboard
  protocol, images (sixel, kitty graphics).
* Copying the last command's output (needs the shell's OSC 133 prompt marks,
  which the emulator does not read), logging a terminal to a file as output
  arrives, and clearing the shell's history while a full-screen program owns
  the screen.
* Reopening a hidden sidebar by dragging from the window's left edge (the
  button and the chord do it), and a sparse dotted grid in empty states (they
  are framed by corner ticks and a dimension line instead).
* Dragging a pane to rearrange it, broadcasting input to several panes, saved layouts.
* Local sessions do not survive quitting Leon (the processes are hung up with
  the window). Terminals on a relay machine do survive on the host, and
  `Client::pty_list`/`pty_attach` can re-attach to them, but listing the ones
  still alive in the tree after a restart is not built.
* Sharing runs inside the open application; a background service that survives
  the window (launchd, systemd, a Windows service) is the next stage.
* Remote Windows hosts (a POSIX shell is assumed).
* Windows terminals are built but not exercised by the test suite here: the
  PTY round-trip tests run on Unix.
* Visual rendering is verified by tests of the batching, the metrics and the
  paint pass running, not by comparing pixels.

# Architecture

Leon is a Rust workspace. The rules at the end are the ones that keep it
simple; the tests enforce the ones that can be enforced.

## Crates

```text
app (leon)  ── ui, engine, keys, theme, launch, diagnose
 │   ├── leon-term     the embedded terminal (GPUI, PTY, emulator)
 │   ├── leon-mark     the animated lion (GPUI)
 │   ├── leon-den      the Den: live sessions as pixel-art lions (GPUI)
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
leon-den ── gpui-kit and image only (no other Leon crate)
```

| Crate | What it is |
| --- | --- |
| `leon-core` | Machines, projects, worktrees, sessions and messages, and the `Store` (SQLite) with change notification. No UI, no processes. |
| `leon-history` | Reads the history Claude Code, Codex and opencode keep on disk and turns it into sessions and messages. |
| `leon-remote` | A `CommandSpec` says what to run and where; `run_on` and `interactive_on` place it on a machine (unchanged locally, `ssh` remotely, with shell quoting and optional connection sharing). Git worktree operations, the changed files and diffs of a checkout and the commit, push and pull request (`changes`, `ship`), how each agent starts and resumes, the machine probe and the connection checklist (`connect`, `diagnosis`) are built on it. A scripted runner makes all of it testable without a process. |
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
* `host_service/` is `leon host service install | uninstall | status | logs`: the
  text of the systemd unit and the launchd agent (`unit.rs`), the plan of files
  and commands (`plan.rs`) and the reading of the manager's answers (`status.rs`)
  are pure functions; the commands run through `leon_remote::Runner`, so the
  tests script `systemctl` and `launchctl`.

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

## Accounts

An account (`leon_core::Account`) is an agent id, a name and environment
variables, kept in the settings like the custom agents (`agent_accounts`) and
registered in `leon_core::account` so an id can be turned into a name anywhere.
The variable that moves an agent's configuration folder is a column of the agent
table (`AgentSpec::config_env`), filled only where it is verified: Claude Code
and Codex.

* **Deciding is pure.** `account::resolve` (agent, accounts, the
  `default_accounts` lines) says plain, this account, or ask; `launch::account_env`
  turns an account id into the variables for a machine (`~` is that machine's
  home, a removed account is an error, never the agent's own setup);
  `launch::plan_with` puts them in the terminal's `CommandSpec.env`. Locally that
  is the shell's environment; over SSH `remote_shell_command` renders them as
  `env 'NAME=value'` words, and a relay host receives them in the spawn
  request.
* **Remembered in the store** by id: `session.account`, `saved_terminal.account`
  (and `dormant.json`), so resuming, restoring and "Resume in…" start the same
  account. A session found in an account's folder is tagged by the importer; a
  live session's link to its history row tags the row through the engine
  (`Op::SetSessionAccount`).
* **History and limits** are read for Claude Code and Codex only, on this
  computer: `HistoryRoots::accounts` adds one source per account folder
  (`ForAccount`), and `leon_usage::collect_accounts` reads each account's sign-in
  from its own folder (`AccountCredentials`, never the shared keychain item). Its
  readings are stored per account (`usage_reading.account`) and shown as
  separate lines. Another machine's accounts and any other agent's are not read:
  there is no line, and the UI says so. A session's header chip reads
  `Board::get_for(machine, agent, session.account)`, so it shows the account's
  own reading or nothing, never the agent's. Removing an account only asks the
  engine (`Op::DropAccountUsage`), which drops its readings and the history
  series of the plan the reading last had.

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
* **Tokens and cost.** Migration 14: `token_usage` (per session, model and UTC
  day: input, output, cache read, cache write, one-hour cache write; no text, no
  price), and the import cursors of transcript files are cleared once so every
  transcript is read again and counted. The pure parsers (`leon-history`:
  `claude`, `codex`, `opencode`, `opencode_json`, with the per-agent meaning of
  the numbers in `tokens.rs`) put the counts in `ParsedSession::tokens`; the
  importer replaces a session's rows in the same transaction as its messages
  and cursor (`set_session_tokens_in`), which makes a re-import idempotent. A
  Claude Code session's sub-agent files (`<session>/subagents/agent-*.jsonl`)
  are folded into the session's item by `ClaudeFiles` (fingerprint and load),
  so their usage counts without becoming sessions. `Store::token_usage` sums
  them. `leon_usage::pricing` holds the prices (the bundled `data/prices.json`,
  with source URL and date per entry, and the user's `prices.json` next to the
  settings laid over it) and `leon_usage::spend` the pure sums; a model without a price has tokens and no cost, never a guess. The
  view only reads the store (`ui/usage_tokens.rs`).
* **UI.** `agent_usage::Board` is what the window reads. `ui/usage_view.rs` holds
  the pure model (`bar_model`: one figure per agent, its logo and the window
  closest to its limit, with everything else on hover; `usage_rows`, worst first;
  `step_scope`; tested
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
* **Differences from Orca, on purpose** (also in the README's Usage section): no hidden sessions, PTY scraping or `codex app-server`; no refreshing or rewriting of any CLI's credentials; no pasted cookies or switching of an agent's sign-in (several accounts of one agent are the user's own, each read from its own configuration folder: see "Accounts"); an honest `User-Agent: Leon/<version>` instead of imitating `claude-code` or `codex-cli` (only the protocol headers `anthropic-beta`, `OpenAI-Beta` and `ChatGPT-Account-Id` are sent); a 10 minute default refresh with a 60 second per-vendor floor; the `CLAUDE_CONFIG_DIR` keychain suffix is taken without NFC normalisation.
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
as `state-file` / `arguments`), **by folder** (Codex, opencode and the rest:
the sessions of that folder created after the terminal started; with one
terminal the newest, with several only a session that exactly one of them could
own, never two terminals on one session; stored as `newest-in-folder`) and
**by title** (`by_title`, for a terminal that already has a link: the title a
program sets is its session's own — opencode's `OC | <title>` — so a terminal
whose agent moved to another session inside it, or resumed an old one, follows
it; stored as `title`). The link also sets the terminal's history row, so the
tree shows one row, live now and history later, and opening the session lands
on that terminal — `resume_in` also checks the title directly, for the seconds
before the relink.
`history_sync.rs` keeps the history fresh: `Op::SyncHistory` (an
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
  activity transitions, so the change *to a state that wants the user* is one
  event (`Activity::is_news_after`: from a state that does not, plus an answered
  question that ended its turn). Which event it is comes from the state:
  `TurnOver` (`finished its turn · your move`) and `NeedsAnswer` (`needs an
  answer · ... probably a permission prompt`) when the agent's transcript is
  followed, `Waiting` (`waiting for you · quiet, or the bell rang`) when it
  is not; all three are governed by `notify_waiting`; `ViewEvent::Exited` is the other, with the exit code. `Shell::raise`
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
(exit, quiet time, foreground, bell) to `Off < Idle < Working < TurnOver <
Waiting < NeedsYou < Failed` (`terminal_activity`, the heuristic); a worktree
shows the most urgent of its terminals, a project of its worktrees. A session's
state is refreshed on its terminal's wake-ups and by one coarse timer that
exists only while a terminal is live; the shell repaints only when a state
changed. Thresholds are in `Options::activity`.

`session_activity` refines the heuristic with `Transcript`, what the agent's
own transcript says (`Turn`, `Call { asks }`, `TurnOver`, from a `Pulse`), with
the precedence documented in the module and held by table tests: the terminal
ended, the shell in front, not an agent or a terminal that cannot tell the
foreground (`transcript_applies`) means the heuristic; no beat read means the
heuristic; a turn over is `TurnOver`; a question or plan among the calls in
flight (any of them, not only the newest) is `NeedsYou`; another call without a
result is `NeedsYou` once the terminal is quiet or rang (inferred) and
`Working` otherwise; a turn with no call open is `Working`.
`ui/transcript_watch.rs` keeps `LiveSession::transcript` current: one loop
(`Shell::status_watch`, started by the coarse timer, `Thresholds::tick`, two
seconds, and by the history sync learning an id, ended with the last followed
session) polls a `Followers` of its own, off the window's thread, once every
`Options::den_tick` (a second). It follows a session that runs on this
computer, whose agent is in front as the terminal can tell
(`transcript_applies`, so nothing is read on Windows, where the pseudo-console
has no foreground), whose transcript Leon reads (Claude Code, Codex) and whose
id is known for sure (`followed_id`). That id is asked again of the process
scan (`learn::by_process_again`: the one agent below the terminal's shell that
names another session), because the agent can move to another session in the
terminal (`/clear`, `/resume`, a new run by hand); until the scan sees it the
old transcript is the one read. It starts at the last `RECENT_BYTES` of the
file, then reads what is appended, and looks again for a file it did not find
every `LOOK_AGAIN_EVERY` polls (a Codex rollout is found by walking folders).
A `Pulse` reports its turn over until a beat says otherwise, and a read that
starts in the middle of a turn may only see results and numbers, so a
`Reading` says nothing until a beat opened a turn or ended one. Nothing else is
followed, and a report that is missing leaves the heuristic. The Den's
followers are separate (they read from the start for the level and the past),
and the Den is given the heuristic alone (`terminal_activity`) so that it adds
the transcript itself, as before. What is covered is the written transcript of
tests; the behaviour against long real sessions of either agent is not.

**Agent colours** are theme tokens (`Palette::agent_*`), a deliberate exception
to a theme's accent rule, set per theme so that each theme can choose its own
(monochrome included). Nothing outside `theme/` assumes a particular theme.

## The project file (`project.rs`, `trust.rs`, `ui/scripts.rs`)

`leon.toml` at a project's root (format in [PROJECT.md](PROJECT.md)) follows the
rules above:

* **The engine reads it.** `Engine::read_project_file` reads it where the project
  is (`std::fs` on this computer, `leon_remote::files::read_command` through the
  runner elsewhere, so SSH and relay machines work), parses it with the pure
  `project::parse` (errors name the line) and keeps the outcome as a
  `ProjectState` that the window reads with `Engine::project_state`. A missing
  file is `Absent` and says nothing. The window asks for a read when the project
  in view changes, when **Run a script...** is used and when a worktree appears.
* **Pure first.** `project::parse`, `ScriptKey` (the chord, and which command of
  `keys::BINDINGS` already has it), `trust::verdict`, `launch::chain` (the line
  that types a command and then the agent only if it succeeded) and
  `address::worktree_location` (the path template) are functions with tests.
* **A script key never shadows a command.** `handle_key` asks the registry first;
  a script's chord is looked at only when no command has it, and
  `ProjectFile::script_for` does not answer a key that collides.
* **Nothing runs untrusted.** A command goes through `trust::verdict` (project id
  and the SHA-256 of the exact text, in `trusted.json`); an unknown one opens
  `Overlay::Trust` with the command in full. The card can come from a background
  task, so `scripts::key_answer` (pure) lets only a `Cmd`/`Ctrl` chord answer it
  and a plain Enter does nothing; questions queue (`ScriptsUi::queued`) and are
  shown one at a time, never over a palette or a menu.
* **Setup before the agent.** `Shell::start_live_first` starts the terminal with
  the agent's line held back and types `sh -c '<setup>' && <agent>`, or
  `sh -c '<setup>'` for a shell session (`launch::chain` with `First::Setup`;
  PowerShell and `cmd.exe` have their own spelling and type a setup as written),
  so the shell sequences it on any machine and a failed setup leaves the terminal
  open at its prompt. A script (`First::Script`) is typed as written. A file that
  is wrong or unreadable gives no setup, and `scripts::setup_notice` says so.

## One prompt, several agents (`fanout.rs`, `ui/fanout.rs`)

**One prompt, several agents…** follows the rules above:

* **Pure first.** `fanout::plan` turns the request (prompt, agents, project root,
  `worktree_location`, the branches taken, the shell's flavour) into the plan:
  per agent a branch (`prompt_slug` and the agent's id, numbered when the
  worktrees or git's own branches already have the name:
  `fanout::taken_branches`), a folder (`address::worktree_location`, the
  function the engine uses for any worktree) and the exact line typed into its
  terminal
  (`launch::command_line_prompted`). An agent that cannot take the prompt is in
  `Plan::left` with the reason, never in the plan.
* **The prompt is on the launch line.** `AgentSpec::prompt` is the form an agent
  takes a first prompt in (`["{prompt}"]`, `["--prompt", "{prompt}"]`); it is
  filled only for agents whose `--help` was read, and a custom agent declares it
  as `prompt_args`. `Launch::Prompted` carries the prompt to `launch::plan_with`,
  which types the line like any agent's. `leon_remote::sh_quote_typed` quotes
  the prompt for typing into an interactive shell of any kind (a backslash
  outside the quotes for fish, several lines on one physical line without a
  `!`), and `launch::clean_prompt` refuses what a terminal would take as keys.
* **The engine makes the worktrees.** `Engine::add_worktrees` makes them one
  after the other through the path of `Op::AddWorktree`, and answers how each
  went; the window first asks git for the project's branches
  (`Engine::base_refs`), waits for the local store to list the worktrees, reads
  `leon.toml` once, and `Shell::start_worktree_sessions` starts the sessions,
  asking about the setup command once for the batch.

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

### Shelves and undo (`ui/shelf.rs`, `ui/shelving.rs`)

A history session is in its list, on the **Settled** shelf or on the
**Snoozed** shelf (until a time). Where it stands is one row in the store's
`session_shelf` table (migration 14: `settled`, `snoozed` with a time, or
`returned`, which is "taken back by hand" and keeps the automatic settling away),
keyed by the session's public id and removed with the session; the engine
writes it (`Op::SetShelves`, which writes nothing and announces nothing when a
row already says so) and the window reads it with the rest. `shelf::section`
decides the place from that row, whether the session runs and the time, which
is an argument: a settled session that runs is in its list, a snooze that has
ended is no snooze, a running snoozed session stays away until it needs you.
`Placement::with_shelves` applies it after `with_live`; a pinned session on a
shelf stays in `Placement::pinned` (the pin operations rewrite that whole order
and would otherwise unpin it) and only the Pinned section leaves it out. The
shelves are `Kind::Shelf` rows at the bottom of a machine, folded by default, and
survive the "only active" view because the person put them there.

`shelf::snooze_until` reads a length of time, `tomorrow`, `next week` or a date
against an injected `now` and UTC offset (`local_offset`, which is UTC in a test
build). A timer places the sessions again when the next snooze ends, looking at
the clock at least once a minute. `shelf::woken` is the early end: the three
notification events (`Waiting`, `Finished`, `Failed`) reach `Shell::raise`, which
ends the snooze before its notification settings are consulted. The automatic
settling (`sidebar_settle_merged`) is `shelf::settle_merged`, a pure choice over
the worktrees whose pull request is merged, run when the store, a terminal or
the engine's scans change and asked of the engine once per session. Only a
session on a machine with a scan result is settled, and not one that scan finds
in another terminal: without a scan nothing is known of other terminals, so
nothing is claimed. "Running" is `history_of_live`: the link a terminal was
started with or the session its learned id names.

Undo is `shelf::Undo`: what one step changed, `Undo::restore` the pure rule for
what puts it back, `Pending` the banner and its deadline on the toolkit's clock
(`UNDO_WINDOW`, five seconds). Closing a whole session hides its history row in
the window and removes it from the store only when the banner goes, so Undo has
something to restore. `Shell::flush` finishes it on the way out
(`Engine::forget_sessions`, called directly because a submitted operation may
not run once the process ends); only a kill or a crash inside the window leaves
the row in the history. An undone settle or snooze of a session with no row
writes `Returned`, not no row, or the automatic settling would take it again.

The main pane shows one of: nothing, a project, a worktree, a history
transcript, or a live terminal. The transcript is opened read-only and starts
nothing; it carries a line that says how to resume the session, or, when a
resume was asked for and could not be done, why.

### The state of a checkout

`leon_core::WorktreeStatus` is what is known about one worktree: the changed
files, the distance from its upstream and the open pull request of its branch
with its review and the summary of its checks. Every part is optional, because
every part can be unknown. It lives in the `worktree_status` table (a JSON
value per worktree, gone with the worktree), `Store::update_worktree_status`
edits one part without overwriting the others and wakes the listeners only when
the result differs, and `Snapshot::status` hands it to the views. It is the
type the changes, commit and pull request flow and the cleaning of merged
worktrees read.

The engine writes it through the `Runner`: every command is placed on the
machine of the worktree by `run_on`, as for the rest of Leon. The git half
(`git status --porcelain=v1 --branch --untracked-files=all`, parsed by
`leon_remote::parse_status`) is read for every worktree of a project at each
sync and by the timer that watches local projects with a live terminal; it
lists untracked files one by one so that its count is the number of files the
changes tab lists. The GitHub half (`gh pr list --state open`, parsed by
`leon_remote::parse_open_pull_requests`) is one question per project, asked
together with the merged one, at most once a minute per project
(`Engine::probe_github`), and only when `origin` is on GitHub: a refresh asked
for within a minute of the last question reads git again and keeps the
GitHub answer of that one. A missing `gh`, a signed-out one or an offline
machine leaves what was stored untouched.
`ui/checkout.rs` decides, as pure functions, what a row and the worktree screen
say.

### Cleaning merged worktrees

`leon_core::cleanup` decides, as pure functions, which merged worktrees may be
removed (`offered` and `blockers`: nothing uncommitted, no commits the upstream
lacks, no session running, the checkout read) and which just became merged
(`newly_merged`: GitHub had been asked and its merged list did not have the
branch, `Some(false)`, and now it does; a branch first seen merged is not
announced). The palette flow
`Remove merged worktrees…` (`ui/steps.rs`, `remove_merged_worktrees`) lists the
offered worktrees, reads each tick as an answer that toggles one of them, and
ends in `Action::RemoveWorktrees`. The shell removes the ticked ones one after
another, each through `Engine::remove_worktree` with `force` unset, so a
worktree git refuses is kept; a second batch asked for while one runs is
refused (`Shell::merged_busy`), because starting it would replace the task and
drop the rest of the first. A removed worktree takes with it the terminals
running in its folder and its changes tab (`Shell::close_live_in`); the files
open from it stay, as they may hold unsaved text. The banner that offers the cleanup is decided by
`steps::merged_offer` each time the store is read, and only when the setting
`offer_merged_cleanup` is on; showing it removes nothing.

### Changes, commit and pull request

A worktree's changes are a tab, a leaf like a file's: `Shell::changes` maps the
leaf's `LiveId` to a `ChangesView` (`ui/changes.rs` holds the state, the pure
decisions and the actions; `ui/changes_view.rs` draws it), and what treats a
leaf as a terminal asks `Shell::live` first and finds nothing, as for
`Shell::files`. The view keeps what the engine answered (the files, the diff of
the selected one, the outcome of the last step) and nothing of it is stored: the
store only has the worktree's status, which the engine writes again after a step.

`leon_remote::changes` reads the changed files (`git status --porcelain=v1 -z
--branch`: `-z` keeps names as they are, a rename is two fields, the new name
first) and the diff of one file (`git diff HEAD -- path`, or against `/dev/null`
for an untracked file, which git answers with status 1), and parses a diff into
lines of a kind with the numbers of both sides; a binary file and a diff over
`MAX_DIFF_BYTES` are answers of their own. `leon_remote::ship` builds the
commands of a commit (`git add`, then `git commit -F -` with the message on
standard input, naming the same paths so what else is staged stays out), a push
(setting the upstream on `origin` when the branch has none) and a pull request
(`gh pr create --body-file -`), the title and body a pull request starts with
from its commits, and what an agent is asked and how its answer is cleaned. A
failure keeps what the command printed, standard output and standard error
together (`ShipError::Failed`), because `git commit` says "nothing to commit"
on the first and a hook writes to either. Nothing in it can force, amend, reset
or discard. Its tests check, with a scripted runner, that every step is placed
on an SSH machine (one `ssh` command that changes to the folder, with the
input piped) and carries the route of a relay machine; no real SSH or relay
machine and no real `gh` has been tried.

`Engine` (`engine/changes.rs`) is the side the view awaits, as for files:
`read_changes`, `read_diff`, `pull_request_start`, `suggest`, and `ship`, which
takes a `ShipRequest` (the commit, the push and the pull request that are
wanted) in that order, stops at the first failure and answers a `ShipReport`.
The palette's whole chain (`Shell::ship_here`) opens the tab and starts only
when the tab has read its files and the start of the pull request, commits only
when files changed (asking for the message when there is none), and so also
pushes and opens a branch that is already committed.
Afterwards it reads the worktree's checkout again and, when a pull request was
opened, asks GitHub for it at once instead of waiting for the once-a-minute
allowance. `ship` and `suggest` use the slow runner (`Engine::set_slow_runner`,
ten minutes in `main.rs`), because hooks and agents outlast the thirty seconds
of the usual one.

The agent table has one optional field for this, `AgentSpec::headless`: the
arguments, before the instruction, that make the agent answer once without a
terminal and exit while reading standard input; the instruction that follows
them tells the agent not to run commands or change files, and the form takes
the means away where the agent can: Claude Code runs with no tools at all and
Codex in its read-only sandbox, because the diff it reads is text of the
repository and not to be trusted. It is filled only for the agents whose form
was run against the installed CLI (`claude --tools "" -p --permission-mode
dontAsk --no-session-persistence`, `codex exec --sandbox read-only`); the
others have none and are never asked. Gemini's `-p` is documented but was not signed in to
run, so it has none.

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
`keys::kept_in_terminal` allows can match: on macOS every `Cmd` chord; on
Linux and Windows the native `Ctrl+C` / `Ctrl+V` editing chords and chords
that also hold `Shift`, plus the scrollback keys. A copy chord is taken only
when the terminal has a selection: `handle_key` gives it back to the program
when there is none, so plain `Ctrl+C` keeps interrupting. Chords are per
platform
(`mac(..)` and `other(..)` in the registry,
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

## The home (`ui/home.rs`)

`Main::Empty` is the home. `Shell::go_home` shows it from anywhere (the
pinned row of the sidebar, the command `GoHome`, its chord): it closes the
sheet that is open, leaves the Den (`den_leave`: no back-stack is kept, the
transcripts stop being read) and sets `Main::Empty` with the keyboard in the
main pane. It never touches `LiveSessions`: a terminal is shown by
`Main::Live(id)` and lives in `self.live` whatever the main pane shows, so
going home closes nothing.

The sidebar's navigation (`home::Nav`: Home, The Den) is drawn between the
header and the filter, outside the tree's `uniform_list`, so neither the
scroll nor the filter moves it. Its rows are built like a project's row
(same height, columns, hover and cursor bar); the Den's closes with the count
of its lions and the waiting dot (`den_glance`). `HomeUi::nav` says the
sidebar's cursor is on one of them (set by going there by any route, and by
Up from the first row of the tree; `move_cursor_to` clears it), and the tree
then draws no cursor: one cursor at a time.

What the home shows is decided without a window: `home::pride` counts this
window's terminals by their `Activity` (the sidebar's dot) and the sessions
the engine's last process scan found elsewhere (`elsewhere::foreign`, every
machine), and picks the few rows listed; `home::items` is the order the
keyboard walks (the actions of `home::ACTIONS`, the sessions that wait, the
ones elsewhere that the history knows, the recent ones) and `home::step`
moves over it. The recent sessions are the first of `Snapshot::sessions`,
which the window already holds: nothing is queried on the render path. An
action runs its command (`run_command`); a session of the history is opened
by putting the sidebar's cursor on its row and activating it, so it behaves
exactly as there (a session held elsewhere asks before anything starts).

## The Den (`leon-den`, `ui/den*.rs`)

The Den shows every live session as a lion in a pixel-art office. It is a
`Main` variant (`Main::Den`): it takes the main pane and leaves the sidebar;
`Shell::toggle_den` keeps what the main pane showed and `close_den` puts it
back. Three layers, each pure where it can be:

* **`leon-den`** knows nothing of Leon. `sim::Den` is a pure function from
  what it was told (`Cub`s and `Happening`s, each with a time) and a time to a
  `Frame`, with the `Wake` that says when the picture changes next;
  `layout::DenLayout` is the room as data (size, floor, walls, carpets,
  pieces by catalogue id; versioned JSON whose reading never fails, with
  notes of what it repaired); `catalogue` says of each piece its footprint,
  where it goes (floor, wall, on a table) and its role; `world` derives
  everything else from a layout (the ground, the spots of each place with
  their facing, which computer a seat lights, the paths, the backdrop);
  `editor::Editor` is every change a user can make as a pure operation with
  undo; `prefabs` are the built-in dens. Placement in `sim` is by capacity:
  every lion owns a home (a work seat given when it joins and kept while it
  is a seat; a tile of bare floor apart from the others when the seats are
  taken), `sim::wanted` names the place a state asks for, and a lion goes
  there only while it has a free spot (`World::free_spot`; three in the
  line at the entrance), otherwise it stays home and shows its state there.
  `paint::plates` lays the name plates out so that none covers another, a
  face or a bubble, and leaves one out rather than overlap;
  `narrator` turns a happening into a line of at most two rows of 34
  characters, and into its plain twin; `feed::Feed` is the log of those
  lines and of what the agents wrote to the user (speech, kept verbatim):
  appending with coalescing, the caps (500 entries; the last 40 of a
  session's past), `backfill` that sums a past turn's tools up in one line,
  insertion of the past by time, the rows for a width (`lay`: headings,
  wrapping, long speech cut to four rows until opened, the filter to one
  lion and its little ones) and where the reader is (following the end, or
  parked with a count of what is new). Time is data: the host says what
  time it is (`DenView::set_wall_time`); `paint::compose` composites the room in software at the size of its
  art; `scene` lays out the chrome in device pixels: in a view at least
  820 px wide a column on the right with the roster over the feed, in a
  narrower one the feed under the room and no roster, in one lower than
  420 px the room alone; the truth card; `view::DenView` scales the picture by a whole number of device
  pixels, hands it to GPUI as one image, and sets one timer per wake (never an
  animation frame; none at all when nothing moves). The pictures are PNG files
  compiled in: pixel-agents' furniture and tiles, and lion sheets that
  `atelier` derives from its character sheets (`cargo run -p leon-den
  --example make_lions`; a test fails when the committed sheets differ). The
  mane is drawn in key colours and dyed per agent, clamped so that any tint
  stays a mane.
* **`ui/den.rs`** is the mapping, pure and tested: `Facts` (what the terminal
  says: the `Activity` of the sidebar's dot, paused, exit code, how long it
  has been quiet) and an optional `Pulse` (what the transcript says) give the
  `CubState`, the level (tool calls) and the plain truth of the card. Its
  header holds the whole state table. `tell` turns transcript beats into the
  narrator's happenings with the real tool name and subject; `changes` tells
  what no beat does (joined, went home, fainted with its exit code, fell
  asleep, the permission prompt on the moment it is inferred).
* **The feed's facts.** `leon_history::live` gives the speech beat its
  text (`Beat::Said { text, at }`: the message whole but for control
  characters, at most 4000 characters, with the line's timestamp).
  `ui/den_view.rs` turns each poll into what the feed is told, in order:
  a happening for the narrator, the agent's words, or, for what is read
  for the first time and for what was written while the Den was closed,
  the past (`den::past`), which the feed sums up. Lions are refreshed
  before the feed is told, so a sub-agent's words land under its parent.
* **Sessions that run elsewhere.** `Shell::den_away` takes the engine's
  last process scan of this computer (`Engine::elsewhere`, the scan the
  sidebar's "running elsewhere" mark uses, at its cadence) through
  `elsewhere::foreign` and keeps the processes that are **certain** matches
  (the process names its session), of an agent with a transcript `Format`,
  not held by a terminal of this window, each session once. Each is followed
  like an own session under an id of its own (`den::away_id`, bit 62) and
  becomes a lion once its transcript is found. `den::away_state` is its
  state table (in the header of `ui/den.rs`): the transcript's `Pulse` and
  the time since the transcript's file was written (`Report::written`). No
  permission prompt is inferred without a terminal, and a process that left
  the scan leaves the den (the exit code is unknown: never `Fainted`).
  Opening one calls the existing `show_elsewhere` (the stored transcript and
  the notice of who holds it). Likely matches, agents without a `Format`
  and remote machines are left out.
* **`ui/den_follow.rs`** follows the transcripts: one
  `leon_history::live::Follower` per local Claude Code or Codex session whose
  own session id is known (`LiveSession::learned`, else the id it resumed),
  and one per sub-agent that is alive (found by the `meta.json` beside its
  file). `Followers::poll` reads files, so `ui/den_view.rs` calls it on the
  background executor, once every `Options::den_tick` (one second) **while the
  Den is open and a session is live**; closed, nothing is read. The first read
  of a session is from the start of its file (that is what makes `Lv.` right)
  and is not narrated: it is the past.

* **`ui/den_edit.rs`** and **`ui/den_store.rs`** are the customising. The
  view owns the editor and turns the pointer into its operations
  (`DenView::edit`); the shell adds the bar and the strip of things to pick
  (thumbnails made from the Den's own art, scaled by whole device pixels),
  the editor's keys, and the files. A den of the user's is
  `dens/<id>.json` beside `settings.json`; the setting `den` holds the id in
  use. Like the theme files, they are written by the shell itself with
  `std::fs` on the window's thread, not by the engine: a den is a few
  kilobytes, written to a file beside it and renamed over the old one, on
  every accepted change (`DenEvent::LayoutChanged`). A built-in den is never
  written: the first change to one makes the user's copy. The file is read
  when the Den opens. The format and the rules are in `docs/DEN.md`.

**The live tail** (`leon_history::live`) is the reader under it: `Tail` turns
appended bytes into `Beat`s (tool started and finished, turn ended, sub-agent
detached and ended...), `Pulse` reduces beats to the current picture. It is
documented in that module.

**The permission prompt is inferred, and can be wrong.** A transcript holds
the tool call but nothing about the question asked before it runs. The Den
says "needs permission" when a call has no result **and** the terminal's
own `Activity` (the heuristic) is `Waiting` (quiet beyond the threshold, or
the bell rang): an
agent at work redraws its status line constantly, so a quiet terminal with a
call in flight is nearly always a question. The limits: it is late by the
quiet threshold; a tool that runs long while the agent's interface draws
nothing reads as a prompt; and a session without a transcript (remote, another
agent, an id not learned yet) can only say "waiting". An `AskUserQuestion` or
`ExitPlanMode` call in flight is "waiting for you" whatever the terminal does.
## Files in tabs (`ui/editor/`)

A text file is a leaf of a tab like a terminal is. `Layout`, `Tab` and
`Workspaces` are unchanged: they hold `LiveId`s, and a file's id comes from the
same counter as the terminals'. What tells the two apart is a side table,
`Shell.files: HashMap<LiveId, EditorDoc>`; a leaf in it is not in `Shell.live`.
`Main::Live(id)` is therefore also "a file is on screen". Every user of a leaf
asks one table or the other: `render_pane`, the tab bar, the header, `open_live`
(a file has no row in the tree), `sync_focus` (the editor's focus handle),
`split_pane`, `close`, the stale-main check, `current_machine` and `here`. The
saved layout (`restore_view.rs`) lists only live terminals; open files are kept
by `session.rs` instead (below). `quit_gently.rs` only looks at terminals;
unsaved files are asked about by the `Quit` flow.

* `document.rs`: `EditorDoc` is the machine, path, the engine's `FileRevision`,
  a fingerprint of the saved text, the dirty flag, the language and the byte
  order mark and line ends that the editor does not hold (a file whose lines do
  not all end alike keeps its `\r`s as text). The dirty flag compares the text
  with what was saved, a pause (`Options::editor_debounce`) after the last
  change, so undoing back to clean clears it.
* `view.rs`: the leaf (gpui-component's `Editor`, or a placeholder for a binary
  or too big file), the tab's dot and the header.
* `actions.rs`: `Shell::open_file(machine, path, line)` shows the existing tab
  of that file or reads it through the engine and adds one with
  `Workspaces::add_tab`; saving sends the revision the file was read with, and a
  conflict opens the `SaveFile` flow (overwrite re-reads the revision, reload
  replaces the editor's text).
* `OpenFile`, `SaveFile` and `CloseFile` are ordinary `Command`s with flows in
  `steps.rs` (`World::file` says what is focused). `SaveFile` and `CloseFile`
  are listed in the palette and enabled in the menu only while a file is on
  screen. While an editor has the keyboard the key context is "typing", so bare
  keys are text. `Ctrl+S` is not kept in a terminal (it is XOFF), and on macOS
  `Cmd+S` saves the terminal's output while a terminal has the keyboard.

* `preview.rs`: a `.md` / `.markdown` file has a `ViewMode` (`Edit`, `Split`,
  `Preview`), cycled by `TogglePreview` (`Cmd+Opt+V`, `Ctrl+Shift+Alt+V`; a
  `When::File` chord, so it never reaches a terminal, and `Ctrl+Shift+V` is the
  terminal's paste) and by three buttons above the file. The page is
  gpui-component's `TextView` over a `TextViewState` kept in the document, so
  each half keeps its scroll position across modes; a change of the text is
  passed on after `Options::editor_debounce`. The text view does not read
  files, so `prepare` settles images first: a relative image of a file on this
  computer is embedded as a `data:` URL (up to 4 MiB, PNG, JPEG, GIF, WebP,
  BMP), on another machine it is replaced by `*[image: alt]*`. A clicked link
  goes through `resolve_link`: `http(s)`/`mailto` open in the browser
  (`Options::open_url`), a path (relative to the document's folder, `..`
  resolved, `%20` decoded, `#fragment` and `?query` dropped) opens through
  `open_file` on the document's machine, anything else is ignored.
* `drafts.rs`, `guard.rs`: the text of a file with unsaved changes is written
  to `drafts/<sha256(machine, path)>.json` beside the settings, a pause after
  the last change (`Options::draft_debounce`, 1 s) and when the application
  ends; saving, discarding (closing without saving, "Quit without saving") and
  reloading remove it. `finish_open` looks for one and puts the text back as
  unsaved changes; if the file's revision is not the one the draft began from,
  the document is marked in conflict and the banner below is shown at once, so
  the next save asks what to do. When the window comes to the front every open
  text file is read again (`Engine::check_file`, quietly) and its revision
  compared: a clean document is replaced by the new text (cursor kept); a
  dirty one gets a banner above the editor (`Reload` / `Keep mine`, not a
  dialog). A save still compares the revision where the file is, so a change
  that lands after the look is caught as a conflict. Overwriting a file that
  was deleted creates it again (a save with no expected revision refuses if
  something appears there meanwhile). Quit with unsaved files asks "Save all
  N files and quit / Quit without saving / Cancel" (`steps::quit`;
  `Shell::save_all_and_quit` quits when the last save has landed, and not at
  all if one conflicted or failed). The window's close button goes through the
  same flow.
* `session.rs`: `open_files.json` beside the settings (`version`, the open
  files in tab order with machine, path, cursor line, Markdown mode and
  whether it was shown in its workspace, and per folder of the tree the open
  folders and selection). Every field has a default and unknown fields are
  ignored. It is written when the shape changes and as the application ends
  (cursor lines only then); at start the files are opened one after another
  without taking the keyboard (`editor_restore_files`), and the ones that are
  gone or no longer text are skipped without a message.
* Editor settings (`editor_tab_size`, `editor_soft_wrap`, `editor_line_numbers`,
  `editor_indent_guides`, `editor_highlight`, `editor_font_size`) are read when
  an editor is made and applied to every open one when the settings change
  (`Shell::sync_editor_prefs`); the family is the terminal's. `Open anyway`
  on the placeholder of a binary or too-big file on this computer shows its
  first 8 MiB as lossy text in a read-only editor (`Body::Lossy`); on other
  machines the bytes would have to cross the wire, so it is not offered.
  `RevealInTree` shows the tree, opens the folders above the focused file and
  selects it.

### The file tree (`files_panel.rs`, `filetree.rs`, `icons_map.rs`)

A column between the sidebar and the main pane, drawn while the setting
`files_visible` (default off) is on. The keyboard model gained a third pane:
`Pane::Files` sits between `Sidebar` and `Main` in `NextPane`/`PreviousPane`
(Tab cycles `editor::next_pane`, two panes while the column is hidden). A new
pane was the least invasive way in: every keyboard command already branches on
`Shell::pane` (`move_cursor`, `open_here`, `Expand`, `Collapse`, `Close`), the
tree is not a text field so it needs no focus handle of its own (the shell's
holds the keyboard, as for the sidebar), and `Esc` from it goes to `Main`.
`ToggleFiles` (`Cmd+Shift+E`, `Ctrl+Shift+Alt+E`: every `Ctrl+Shift+letter` was
taken, and `Ctrl+B` is the sidebar's on macOS only) shows it with the keyboard
or hides it, and `metrics::FILES_WIDTH()` (zero while hidden, like
`SIDEBAR_WIDTH`) is subtracted wherever the main pane's width is computed
(`line_frame`, the terminals' pane area, the usage strip).

* `filetree.rs` is pure: `Tree` holds the listings (folder to entries or
  failure), the open folders, the selected path, git's marks and `rows()`, the
  flat list of visible rows (`depth`, `open`, `mark`, notes for a folder that is
  loading, empty or failed), with `step`, `right` and `left` for the
  keyboard. Paths are relative to the root and `/` separated, like git's.
* `files_panel.rs` keeps `FileTreeUi`: a `Tree` for each
  `workspace::key_of(machine, root)`, so the open folders and the selection are
  as they were left. The root is the workspace folder `Shell::here` points at
  (the sidebar's row, or what the main pane shows), else the first project of
  the machine in view; `files_sync` runs in `render` and switches trees when it
  changes. `files_refresh` lists the root and every open folder again
  (`Engine::list_dir`) and reads `Engine::git_marks`; it runs on a timer
  (`Options::files_interval`, 5 s, only while visible), when the window gets the
  focus and after a save, never asks for what is already in flight, and a
  failed refresh keeps what was shown.
* `icons_map.rs`: `glyph(name, is_dir, open)` gives a codepoint of the bundled
  Symbols Nerd Font Mono and a colour *token* (the theme's text, accent or ANSI
  hues), by exact file name, then extension, then a generic file or folder. If
  the family is not among the text system's fonts the Lucide `File`, `Folder`
  and `FolderOpen` are drawn instead.

Git marks for a folder inside a repository: `git status --porcelain` names paths
from the work tree's root, so `GitMarks::from_status_in(status, prefix)` makes
them relative to the folder (`prefix` is `git rev-parse --show-prefix`) and
drops what lies outside it. The local engine runs the prefix command after
`status`; the remote script prints the prefix on the line after `GIT`.

### Finding things (`quick.rs`, `search.rs`)

Quick open and the search of the project are scopes of the palette, not
overlays of their own: `Scope::Files` (`~`) and `Scope::Text` (`%`), whose rows
are `Item::File`, `Item::FileHeading` and `Item::Match`; the plain palette
(`Scope::All`) lists the files that match too, in a "Files" group after the
places. `QuickOpen` and `SearchProject` just open the palette with the prefix
typed. `ui/editor/quick.rs` holds the state (`QuickFiles`: the last file list
of each `key_of(machine, root)`, asked for again each time the palette opens;
`TextSearch`: switches, hits, the flag and abort handle of the search under
way), the pure ranking (`file_score`: the file's own name counts for more than
its folders; `split_line` reads `path:line`) and the grouping of hits. The root
is `files_target` of the file tree. Rows lead with `Shell::file_icon`
(`icons_map`).

`Engine::search_project` is the one call. On this computer it takes the file
list of `project_files` (so `.gitignore` is honoured) and scans it in-process
with `regex` (`crate::search`; literal text is escaped, an empty match is not a
hit, files above 1 MiB and those with a NUL near the start are skipped, 1000
hits at most) on a blocking thread that looks at a cancel flag between files;
elsewhere it is `leon_remote::search` (see `docs/REMOTE.md`) under a 30 s
limit. A new query raises the flag, aborts the job and drops the hits; an answer
carries the generation it was asked for and is ignored when stale. The palette
lists 300 lines of the 1000.

The search bar of the editor is gpui-component's (`open_search`). `FindInFile`
(`Cmd/Ctrl+F`) and `ReplaceInFile` (`Cmd+Opt+F`, `Ctrl+H`) have `When::File`
chords: `keys::Context::file` is true while an editor has the keyboard, and
those chords listed before the terminal's find and the sidebar's filter win only
there. Leon sees every key before the editor, so while the bar's fields have the
keyboard `Escape` is let through to the bar (`editor_search_has_keyboard`) and
`sync_focus` does not take the keyboard back. `go_to_line` puts the cursor again
two frames after opening, when the editor has been laid out and can scroll.

Languages are a Cargo feature, `languages` (on by default), that turns on the
tree-sitter grammars of gpui-component through `gpui-kit`; JSON comes with the
base. SQL is not included: its grammar pins an older `cc` than GPUI needs.

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

* Files in tabs: no replace across the project, no side-by-side comparison of
  a draft or a changed file with the editor's text, "Open anyway" only for
  files on this computer, the editor's soft wrap, tab size and guides cannot
  be set per language, and the editor has no language server.
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

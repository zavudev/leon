# Changelog

## [0.7.0] - 2026-10-08

### Added

* **The Den in 2.5D** (setting `den_3d`, **The Den in 2.5D**, on by default). The
  Den is drawn as an isometric office in the colours of the theme, light and
  dark, by a graphics device of its own rendered off screen. The pixel art
  stays as the fall back: when no adapter is found, when a draw fails, or with
  the setting off; a fall back is said in the status bar, in the setting and
  in the log. The default office is larger (20x15 tiles, 12 seats, from 14x11
  and 6), and the editor and the thumbnails of the dens are isometric. Lions
  of one agent differ in mane, coat, cut, build and what they wear. It was
  seen running on Linux only; on macOS and Windows it builds and its tests
  pass, and nobody has looked at it yet.
* **What you can do to a lion.** Selecting a lion shows its actions as a strip
  over the foot of the room, each with its key, and the right button, `M`,
  `Shift+F10` or the menu key open the same list as a menu: Open, Message…,
  Queued messages…, Interrupt, Rename…, Pin, Send home, Close. A message is
  typed into the lion's agent as a prompt, waits in a queue of its session
  while the agent works, and is never delivered into a permission prompt or a
  question; when an Enter seems lost the queue holds and says so. **Send
  home** stops the agent and leaves the session asleep in the sidebar; it
  always asks first. A lion that runs outside this window can only be opened:
  the rest is shown dimmed, and a click says why. Typing into a real Claude
  Code or Codex was tested with a scripted agent only.
* Keys of the Den: `N` the next lion that needs you, `I` message, `P` message
  the pride, `Q` queued messages, `X` interrupt (Claude Code and Codex), `R`
  rename, `H` send home, `W` wake a lion that is at home, `A` hatch a lion
  (the selected lion's worktree is offered first, and still asked), `/` or `G`
  go to a lion, `?` every key of the Den. While the Den has the keyboard, `?`
  and `/` are its own; the sheet of shortcuts and the filter stay in the
  palette.
* The card of a lion says what a permission prompt or a question asks, your
  first and last prompt, the agent's last words, the agent and model, the
  project and branch, the tools used and the context.
* `leon --den` opens on the Den.

* **Keep local sessions running** (setting `durable_sessions`, off by default;
  macOS and Linux only). The terminals, and the agents in them, that Leon opens
  on this computer are held by a small background process (`leon keeper`, the
  same program started in a session of its own the first time one is needed), so
  they keep running when the window closes, when Leon quits and when it crashes.
  The next start attaches to them again, in their saved tab, pane and focus,
  with the output they printed meanwhile, and does not ask about them or type
  anything into them. Quitting no longer types `/exit` or signals anything to
  them; the quit question says that those sessions keep running. Closing or
  putting a session to sleep still ends it, and the palette's **Quit and end
  every session** is the old quit for all of them. The activity light and
  transcript status, the "running in another terminal" scan, learning an
  agent's session id, file and `#123` links and image paste work for these
  sessions as for the others; the keeper is asked who is in front of each
  terminal twice a second. A running terminal the saved layout does not know
  appears in a session of its own. A session whose terminal is gone (the computer
  restarted, the program ended) is restored the usual way, with a paused resume
  line, and Leon says so; a keeper that is killed takes its terminals with it,
  which is printed in them. Limits: only the last 2 MiB of each terminal's
  output is kept, so older scrollback is not restored; sessions on SSH or relay
  machines are not held; the keeper ends 30 seconds after its last program ended
  with no window connected, runs the Leon it was started from until then (an
  update is picked up by the next keeper), and can be ended with its launcher or
  by logging out; turning it off leaves the sessions already kept running
  until they are closed; with it off from the start, and on Windows, everything
  is as before. It is
  tested with a scripted keeper over a real socket and a scripted window; closing
  the window and reopening Leon on a real machine has not been tried.
  Before it talks to the keeper the window checks the socket's folder (a real
  folder of its own, closed to other users: the fallback folder under `/tmp` has a
  predictable name another user could create first) and, once connected, that
  the other end is its own user, and sends nothing otherwise; the session then
  starts in the window. New terminals appear at once and are filled in as the
  keeper answers (nothing waits more than a second and a half, and a keeper that
  did not answer is not tried again for 20 seconds); keystrokes and hang-ups are
  never dropped on a full queue; "Quit and end every session" ends only what
  this window holds and says when the keeper did not confirm it. A saved tab
  with a surviving and a gone terminal is restored in one piece; a session
  another window of the same data directory is attached to is left to it; a
  keeper found with the setting off is not left stranded. Also fixed for relay
  machines: the terminal's answers to a program's questions (where is the cursor,
  which colours) now reach the program. A replay can start mid-escape-sequence
  and look garbled until the program draws again.
* The light of a session now says what the agent is doing, not only that its
  terminal went quiet. For Claude Code and Codex sessions of this computer
  Leon reads the agent's own transcript while it grows, **whether or not the
  Den is open**, and tells apart: **working** (a turn is going on, a tool is
  running, or the model is thinking in silence), **finished its turn** (your
  move: a square in the theme's info colour, `READY` beside the worktree) and **needs you** (a
  question, or a tool call without a result in a quiet terminal: a ringed
  orange square, `ASKS`), besides the existing idle, waiting, failed and off.
  The same words are in the home (`1 need an answer · 2 finished` and a note on
  each row), the lion in the header and the banners and system notifications,
  which now say `finished its turn · your move` or `needs an answer · a
  question, or probably a permission prompt`; both are governed by the
  existing setting `notify_waiting`. A permission prompt is still **inferred**
  (the transcript does not record the question) and can be wrong for a tool
  that runs a long time while the agent draws nothing.
  Only a session whose id Leon knows for sure is followed (started with
  `--resume`, or learned from the agent's state file or arguments, and asked
  again of the process scan so that an agent that moves to another session in
  the same terminal, with `/clear` or `/resume`, is followed there; until the
  scan sees it, the light can still be that of the old session). Every other
  session (another agent, SSH and relay sessions, **Windows**, where the
  terminal cannot say whether the agent is in front, a plain shell, a session
  whose transcript is missing or still empty) keeps the previous behaviour,
  and its notification now says `waiting for you · quiet, or the bell rang`
  instead of claiming a finished turn. A transcript is read from its last
  megabyte, then only what is appended about once a second, and nothing is
  read when no agent is followed; a turn that began before that megabyte is
  not guessed at: the light stays the terminal's until the transcript shows
  where the agent is. The first look can come a couple of seconds after the
  agent starts. It was built on the transcripts' format and tested with
  written transcripts; it has not been run against long real sessions of
  either agent yet, so tell us where it reads wrong.
* Worktree status in the tree. Every worktree row now ends with the state of its
  checkout: the open pull request of its branch (`#123`, coloured by its checks:
  green passing, amber running, red failing, grey for a draft), the number of
  changed files (`~3`, the files the Changes tab lists, untracked ones counted
  one by one) and the distance from its upstream (`↑2↓1`). Pointing at
  the row tells all of it in words, and the worktree's own screen has a **State**
  card with the same, the review state included; the pull request there opens
  on GitHub when clicked. On a narrow sidebar (under 300 px) a row keeps only
  the first of those it knows. The changes and the distance are read with git
  every time the worktrees are checked (about every 10 seconds for a local
  project with a terminal running in it, and on `Import history and sync
  worktrees` for every project, remote ones included, which are not watched on
  a timer). The pull request and its checks come from the GitHub CLI (`gh`),
  only for a project whose `origin` is on GitHub, and a project is asked at
  most once a minute, so a refresh within a minute of the last question reads
  git again but keeps the pull requests of that answer; without `gh`, signed
  out, offline or without git a row simply shows what it could read, and
  nothing is reported. A pull request
  from a fork that shares a branch name with one of yours is not told apart.
* Changes, commit, push and pull request. `Show the changes of the worktree`
  (`Cmd+Opt+G`, `Ctrl+Shift+Alt+G`; the **Changes** button of a worktree's
  screen, the **Changes** chip of the file tree, or the palette, which asks which
  worktree when none is in view) opens a tab with the files git says changed and
  the diff of the selected one: read only, added and removed lines in the
  theme's colours, the hunk headers and both line numbers; a binary file or a
  diff over 1 MB is said instead of drawn, and long lines are cut (no horizontal
  scrolling yet). `Enter` or a double click opens the file in the editor; `Space`
  takes a file out of the commit and back in. Under the list: **Commit** (`git
  add` of the files in the box, then `git commit` with your message, hooks
  included), **Push** (`git push`, setting the upstream on `origin` when the
  branch has none), **Pull request…** (title, body, base and a draft switch,
  prefilled from the branch's commits, then `gh pr create`, the address shown as
  a link and the worktree's pull request in the tree updated at once) and
  **Commit, push and open a pull request**, which does the three in order and
  stops at the first that fails (`Push the branch` and that one are palette
  commands too). From the palette it opens the tab if needed and starts when the
  tab has read its files; it commits only when files changed (and asks for the
  message first when there is none), so a branch already committed is just
  pushed and opened. Nothing is destructive: no force push, amend, reset or
  discarding. What git, `gh` and the hooks print is shown whole. Everything runs
  through the machine's runner, so it is meant to work on this computer, over SSH
  and over the relay. What the tests check is the placement of each command (one
  `ssh` command in the worktree's folder with its input piped, the route of a
  relay machine) with a scripted runner, and the steps against a real
  repository on this computer; no real SSH or relay machine and no real `gh`
  was tried. `gh` has to be installed and signed in there. The commands that may take long are stopped
  after ten minutes, the others after thirty seconds. The tab is not reopened at
  the next start.
* **Suggest a message** and **Suggest title and body** ask an agent to word the
  commit or the pull request from the diff or the commits, and put the text in
  the field to edit. Only Claude Code (`claude --tools "" -p --permission-mode
  dontAsk --no-session-persistence`) and Codex (`codex exec --sandbox
  read-only`) are asked, because their non-interactive form was run here and
  checked against the CLI's own `--help`; for any other agent (Gemini's `-p`
  form is documented but could not be run: it was not signed in) the button is
  not there. The diff is text of your repository, so the agent is not trusted
  with it: Claude Code runs with no tools at all and Codex in its read-only
  sandbox, and the instruction also tells it to answer with the text alone. The
  diff is cut at 60 KB and is that of the files in the commit (files git does
  not track yet are only named, and when those are all that is in the commit
  the agent gets just their names), the agent gets two minutes, its failure is a plain message that
  blocks nothing, and a suggestion that arrives after you started writing is not
  used.
* Remove merged worktrees. **Remove merged worktrees…** in the palette, or the
  **REMOVE MERGED** button on the screen of a merged worktree, lists the linked
  worktrees whose pull request GitHub reports as merged and lets you tick the
  ones to remove. Each ticked worktree goes through `git worktree remove`, one
  after another, and is never forced: a worktree git refuses is kept and named
  in the status line. A second removal asked for while one is running is
  refused with a message, so the first always reaches its end. The terminals
  running in a removed worktree and its Changes tab close, as they do for
  **Remove a worktree**; files open from it stay open, because they may hold
  text you did not save. A worktree with uncommitted or untracked files, commits
  its upstream does not have, a session running in its folder, or a checkout
  nobody has read yet is listed with the reason and cannot be ticked. A branch
  whose remote GitHub deleted after the merge is not held back for that: its
  commits went up with the pull request. The branch and its commits stay in the
  repository.
* The `offer_merged_cleanup` setting (**Offer to remove merged worktrees** in
  Settings, Projects; off by default). When a worktree's pull request turns
  merged while Leon is open, a banner offers the list above, and a click opens
  it. The banner removes nothing. Only a change Leon saw is announced: a
  worktree whose branch GitHub's last answer did not list as merged and whose
  next answer does. A branch first seen merged is not announced, and a branch
  with no pull request counts as not merged, so a pull request opened and merged
  between two answers is announced too. GitHub is asked at most once a minute
  per project, so the banner can come a minute late.
* A project file, `leon.toml`, at the root of a repository: `[[script]]`
  entries (`name`, `command`, and optionally an `icon` and a `key`) and
  `[worktree] setup = "..."`, a command for every new worktree. Leon reads it
  where the project is (this computer, over SSH or through a relay), and a
  mistake is reported with the line it is on. The format, with the rules and
  the limits, is in `docs/PROJECT.md`.
* **Run a script…** in the palette lists the scripts of the project in view and
  runs the chosen one in a new terminal tab of the worktree in view. A script
  with a `key` (`mod+shift+u`: `mod` is `Cmd` on macOS and `Ctrl` elsewhere,
  and `shift` is always required, so the chord stays Leon's while a terminal
  has the keyboard) answers that chord. A key that one of Leon's own commands
  already has is not bound: the status line and the palette say so, and the
  script still runs from the palette. A key uses the file as it was when the
  project came into view; the palette reads it again first.
* A worktree's `setup` command runs, in a terminal of the new worktree, before
  the agent starts: in a POSIX shell the terminal is typed `sh -c '<setup>' &&
  <the agent>`, or `sh -c '<setup>'` alone for a shell session, so the setup
  means the same in every login shell and the agent starts only if it
  succeeded. When it fails the terminal stays open with the output and no agent
  is started. What the setup exports does not reach the agent, and a multi-line
  setup is refused (join the commands with `&&`). PowerShell and `cmd.exe` have
  no `sh`: the setup is typed there as written. A script, by contrast, is typed
  as written into the shell of its terminal, so that shell's aliases and its
  own syntax apply.
* If `leon.toml` is wrong or cannot be read when a worktree is made, the session
  still starts without the setup and the status line says why (`No setup was
  run: leon.toml line 7: ...`), also when several agents were started at once.
* The commands of `leon.toml` come from the repository, so Leon asks before
  the first one runs, and again whenever its text changes: a card shows the
  exact command, where it comes from, where it would run and how it is typed.
  The card can appear by itself a moment after a worktree is made, so no plain
  key answers it: `Ctrl+Enter` (`Cmd+Enter` on macOS) runs the command once,
  `Ctrl+Shift+Enter` (`Cmd+Shift+Enter`) runs it and remembers it, `Esc` does
  not run it, and the card has a button for each. A plain `Enter` does nothing.
  Cards wait their turn instead of replacing a palette or another card. What is
  remembered is a hash of the command for that project, in `trusted.json` next
  to `settings.json`; there is no screen to take an answer back, delete the file
  (with Leon closed) to forget them all. A worktree whose setup was not run still
  gets its session, and the status line says the setup was skipped.
* The setting `worktree_location` (Settings, Projects) says where new
  worktrees go: a template with `{root}`, the project's folder, and `{branch}`,
  the branch with its slashes turned into dashes. The default,
  `{root}-worktrees/{branch}`, is exactly where worktrees have always gone;
  `{root}/.worktrees/{branch}` keeps them inside the project. A template that
  does not name `{branch}` or does not give an absolute path is refused when a
  worktree is made.
* **One prompt, several agents…** (palette, File menu, and the menu of a project row): type
  a prompt, tick two or more agents, pick the base, and Leon makes one worktree
  per agent and starts each agent in its own with the prompt already on its
  launch line, so the sessions sit side by side in the tree and can be compared.
  The branches are a slug of the prompt and the agent (`fix-the-login-bug-claude`,
  `fix-the-login-bug-codex`), made one after the other the way **New worktree**
  makes them: `worktree_location` and the `setup` of `leon.toml` apply, and the
  setup is asked about once for the whole batch. A branch name that the project
  already has in git, with or without a worktree, gets a number before the agent
  (`fix-the-login-bug-2-claude`), so giving the same prompt again works. An
  agent that is not installed, cannot take the prompt or whose worktree git
  refuses is told by name in the status line (`Started 1 of 2 agents: Codex:
  ...`) and the others go on. If git cannot list the branches, only the names of
  the worktrees are avoided and git's own refusal is what you read.
* Only agents whose way of taking a prompt was read in the `--help` of the
  program are offered there: Claude Code, Codex, Grok and Cursor (the prompt as
  their argument), opencode (`--prompt`) and Gemini (`--prompt-interactive`). A
  custom agent declares its form in the new last question of **Add a custom
  agent…**, the prompt arguments (`--ask {prompt}`); left empty, it is not
  offered the prompt.
* The prompt is typed into the shell as one quoted argument, on this computer
  and over SSH or a relay: quotes, `$`, backticks, `!`, backslashes and several
  lines arrive as written. It cannot hold control characters (a tab becomes a
  space), start with a dash, be longer than 8000 characters or be a single word
  (an agent would take `apply` or `update` for one of its own commands), and the
  palette's field takes one line. It is not offered when the shell is
  PowerShell or `cmd.exe`.
* Settle, snooze and undo in the sidebar. **Settle** (a session's context
  menu or the palette) moves a session onto a folded **Settled** shelf at the
  bottom of its machine; **Snooze…** hides it on a folded **Snoozed** shelf
  until a time you pick or type (`45m`, `2h`, `3 days`, `tomorrow`,
  `next week`, `2026-10-12 14:00`), and its row says when it comes back. A
  snoozed session returns by itself, and early when its terminal needs you,
  fails or finishes (a session running in another terminal cannot end its own
  snooze, because Leon only sees it from outside). **Bring back** puts either
  kind where it was; a pinned session keeps its pin on the shelf. The shelves
  honour the filter and are kept in the local store, so they survive a restart.
  A running session cannot be settled (that includes one whose terminal
  Leon only learned the agent's session id for), and a sleeping plain shell,
  which has no history session, can be neither settled nor snoozed.
* `sidebar_settle_merged` (off by default): settle a session by itself once the
  pull request of its worktree is merged and its terminal is not live. Pinned
  sessions, ones you brought back by hand and ones running in another terminal
  are left alone. Leon knows about other terminals only from its look at the
  machine (`detect_elsewhere`), so nothing is settled on a machine it has not
  been able to look at.
* Undo for closing, sleeping, settling, snoozing and unpinning: a banner offers
  it for five seconds, and `⌥⌘Z` (`Ctrl+Shift+Alt+Z`) and the palette's **Undo**
  do the same. Undoing a settle or a snooze puts the session back in its list
  and an unpin puts it back among the pinned sessions in its old place. A
  session that was never on a shelf comes back as one you took back by hand,
  which `sidebar_settle_merged` leaves alone. A closed terminal's program
  cannot come back, so undoing a close restores the row as a sleeping session (a
  closed shell that had no sleeping row gets a new one after the others) and
  undoing a sleep wakes it with a new program; the banner says so. A closed
  session leaves the history when the banner goes or when you quit; if Leon is
  killed or crashes inside those five seconds, it stays listed.
* Tokens and an estimated cost in the usage view (`⇧⌘U`, `Ctrl+Shift+Alt+U`).
  Under the limits it now lists what the agents used, per agent, per model and
  per UTC day, over the last 7, 30 or 90 days or all time (`T`, or a click on
  the span), with the share of the input that came from the cache. The counts
  are read from the agents' own transcripts by the history import (Claude Code,
  Codex and opencode), once per file however often it is read, so a transcript
  that grows is never counted twice; the first start after the update reads each
  transcript once more to count the sessions already imported. The cost is an
  **estimate at the providers' API list prices, not what a subscription is
  billed**, and a model with no price shows its tokens and `no price`. The
  prices are a file bundled with Leon, each entry with the page it was copied
  from and the day (Anthropic and OpenAI models, 2026-10-07); a `prices.json`
  next to `settings.json` overrides or extends it, with model aliases. Not
  counted: machines connected through the relay, opencode's sub-agent sessions,
  and models whose price depends on the length of each prompt (Claude Haiku
  5.5), which have tokens and no price. A Claude Code sub-agent's calls
  (`<session>/subagents/agent-*.jsonl`) are counted into the session that
  started it, but a reply that a resumed or forked Claude Code or Codex session
  repeats from its parent is counted in each file, so those sessions are
  counted twice.
* `leon host service install | uninstall | status | logs`: the host as a
  background service of your own session, so a computer stays shared with no
  window or terminal open. On Linux it is a systemd user unit
  (`~/.config/systemd/user/leon-host.service`, driven with `systemctl --user`);
  on macOS a launchd agent in `~/Library/LaunchAgents`; on Windows the command
  says it is not supported yet and exits with an error. It never needs or
  accepts root, runs the file that installed it (an update that replaces that
  file in place takes effect the next time the service starts; with Homebrew or
  Nix, which keep each version in its own folder, run `install` again after an
  update, and `install` says so) and restarts after a failure, with a growing
  delay under systemd 254 or newer and at least 30 seconds apart under launchd.
  `install` waits two seconds and exits with an error if the host is not
  running then; `status` says whether it is installed and running, since when,
  its relay and how many devices are paired; `uninstall` stops the service
  first, keeps its definition and exits with an error if it cannot confirm that
  the service is stopped, and leaves your pairings and settings alone. A
  `leon host` started by hand makes the service wait its turn, but **Share this
  machine** in the app does not, so use one or the other on a computer.
  On Linux the service stops when you log out of your last session unless you
  install with `--linger`, which runs `loginctl enable-linger` for your own
  user; nothing touches lingering without that flag. The terminals the host
  serves to a paired device live in the service; the desktop app's own local
  sessions still end with its window, and the macOS side has not been run on a
  Mac yet. See `docs/REMOTE.md`.
* Several accounts of one agent. The palette's **Add an account…**, **Rename an
  account…** and **Remove an account…** (and Settings, Agents) keep named
  accounts: an agent, a name and the environment variables that make the agent
  another account, usually its configuration folder (`CLAUDE_CONFIG_DIR` for
  Claude Code, `CODEX_HOME` for Codex; a leading `~` is the home folder of the
  machine the terminal is on). A new session of an agent that has accounts asks
  which one to start, the agent's own setup being one of the choices; the setting
  `default_accounts` (`claude=work`, or `claude=default`) skips the question. **One prompt, several agents…** and the session of a new worktree never ask: each agent starts as the account `default_accounts` names for it, else as its own setup. The
  variables are added to that terminal only: to the shell's environment here, and
  to the `env` of the command that runs over SSH or a relay, each quoted as one
  word. A session remembers its account, so resuming it, **Resume the session in
  another worktree…**, waking a sleeping session and restoring the last sessions
  all use the same one. If the account was removed the session is never started
  as another: a history session shows its transcript with the reason, a sleeping
  one stays asleep (add the account again with the same name to wake it), and a
  session of the last run is listed among those that could not be reopened. The
  session's header and its sidebar row show the account's name, and the header's
  limit figure is that account's own, or absent. Removing an account takes its
  line of limits and its history chart with it. For Claude Code and Codex, on this
  computer, each account's folder is imported like the agent's own (its sessions
  appear in the tree) and its limits get a line of their own, read with the
  sign-in kept in that folder. For other agents, and for the accounts of another
  machine, the account only changes the launch: no history and no limits are read
  for it, and the usage view says so instead of showing the agent's own numbers.

### Changed

* `leon host` now stops cleanly on `SIGTERM` as well as Ctrl-C, and its
  heartbeat file also says when it started.

### Fixed

* The Den no longer shows a lion for a terminal without an agent, and its
  lions no longer all look alike.
* A click in the Den gives it the keyboard back from the sidebar.
* `leon host` no longer refuses to start because of the heartbeat a crashed or
  killed host left behind: a heartbeat now counts only while its process still
  exists, so `leon host status` and `leon host pair` do not report a dead host
  as running either.

## [0.6.0] - 2026-10-07

### Added

* The Den (`⌥⇧⌘L`, `Ctrl+Shift+Alt+L`, the View menu and the palette's **Open
  the Den**): every live session as a pixel-art lion at work in an office,
  in the place of the main pane. A lion types at its desk while its agent
  edits, empties the shelf while it searches, minds the rack while a command
  runs, and stands on the rug with a `!` when it waits for you; a sub-agent
  hatches from an egg and works at the small table. A text box tells it in
  the voice of an old creature game ("MOSS used CARGO TEST!"), a roster lists
  the pride with each lion's `Lv.` (the tools it has used), and pointing at a
  lion shows the plain truth: the tool, the file, the command. For Claude
  Code and Codex sessions of this computer the facts are read from the
  agent's own transcript while it grows; other sessions show only whether
  they work or wait. A pending permission prompt is inferred (a tool call
  without a result in a quiet terminal) and the card says so. Arrows and Tab
  select, Enter or a double click opens the session, Escape goes back.
  `den_narrator` turns the narrator off for a plain count; reduced motion
  holds the lions still. The art is from pixel-agents (MIT), with lion heads
  drawn for Leon.
* The Den's room can be changed. **Edit the Den** (a button over the room,
  the palette, the View menu, `E` in the Den) opens an editor: furniture is
  picked from a strip of pictures and put down, dragged, turned, removed;
  carpets are laid, the floor and the walls repainted, the room made wider or
  deeper, with undo and redo and every step possible from the keyboard. The
  lions keep living in the room while it changes and go where the furniture
  says: a seat that faces a computer is a place to work, a bookshelf a place
  to read, a rack a place to run commands. A room that lacks one still works,
  and the editor says what is missing. **Choose a den…** switches between six
  built-in dens (the office, the open plan, the library, the server room, the
  lounge, the nook) and your own; a built-in den is never changed, the first
  change makes a den of yours from it. Dens are JSON files in a `dens` folder
  beside the settings (**Save the den as…**, **Rename the den…**, **Delete
  the den…**, **Open dens folder**); the setting `den` holds the one in use.
  The format, the rules and the catalogue are in `docs/DEN.md`.
* The Den's narrator moved from a box under the room to **the feed**, a
  log on the right under the roster, with history. Besides the narrator's
  lines it shows what each agent **wrote to you**, in its own words (Claude
  Code and Codex sessions of this computer): marked `said`, never rewritten,
  long messages cut to a few rows until clicked. Entries carry the lion's
  mane colour, its name and how long ago. When the Den opens the feed is
  already filled from each session's transcript (its last messages, and one
  line per past turn such as `MOSS used 14 tools.`), and what happened while
  the Den was closed is summed up the same way. It follows its end until you
  scroll back, then shows how many entries are new; `Page Up`, `Page Down`,
  `Home`, `End` scroll it, `Shift+Up`/`Shift+Down` walk its entries and
  `Enter` opens a long message. Selecting a lion narrows it to that lion and
  its sub-agents. In a narrow pane it sits under the room. With
  `den_narrator` off its narration is plain and the agents' words stay.
* In the Den nobody is piled on anybody any more. Every lion has a desk of
  its own and sleeps there; the sofa, the shelf, the rack and the entrance
  hold as many as they have places (three wait in line), and a lion that
  finds its place full stays at its desk and shows there what it does. More
  lions than desks stand apart on free floor. Name plates say the first
  meaningful words of a title in plain capitals (accents are folded, so
  "móvil" reads `MOVIL`), never cover each other, a face or a bubble, and are
  left out where the room is too crowded. The library's desks face their
  screens.
* The Den also shows the Claude Code and Codex sessions of this computer
  that run **outside this window**: in another Leon window or in a plain
  terminal. They are found by the existing look at the processes and told by
  their transcripts alone (tool, `Lv.`, sub-agents, their words and their past
  in the feed); the card says where each runs and its pid, and for how long
  nothing was written rather than guessing a permission prompt. `Enter` opens
  the stored transcript with who holds it. `den_elsewhere` turns them off.
* The **home** is a place to act from, and always one step away. **Home** and
  **The Den** are rows at the top of the sidebar, over the filter (no scroll
  or filter hides them; the Den's row counts its lions),
  and **Go home** (`⌥⇧⌘H`, `Ctrl+Shift+Alt+H`, the View menu, the palette)
  goes there from anywhere without closing a terminal. The home has buttons
  for what starts things (Open the Den first, a new session, open a project,
  go to, search the history, the palette, connect a machine, shortcuts,
  usage, settings), the sessions at a glance (how many run here and
  elsewhere, how many work, and the ones that wait for you, each a click
  away) and the recent sessions. The arrows walk it and `Enter` opens.
* A file engine for the editor, on this computer and on every other machine:
  read, save, list a folder, list a project's files and ask git for each file's
  mark. A save refuses to overwrite a file that changed since it was read, keeps
  its mode, never leaves half a file and follows a symbolic link to its target.
  Other computers run short `sh` scripts over SSH or the relay (they need `sh`,
  `cksum`, `base64` and `wc`).
* Text files open in a tab next to the terminals of their project, on any
  machine: **Open a file…** (`⇧⌘O`, `Ctrl+Shift+Alt+O`) asks for a path,
  **Save the file** is `⌘S` / `Ctrl+S` and **Close the file** is in the palette
  (`Close the pane` closes one too). A file splits beside a terminal like any
  pane. The editor has syntax highlighting (Rust, JavaScript, TypeScript, TSX,
  Python, Go, JSON, TOML, YAML, shell, Markdown, CSS, HTML, C, C++, Lua, Make,
  diff, Java and Ruby), line numbers, search and indent guides. The tab shows a
  dot while there are unsaved changes (undoing back to what was saved clears
  it), a byte order mark and `\r\n` line ends are kept when saving, and a file
  that is not text or is over 2 MiB says so instead of opening. Closing or
  quitting with unsaved changes asks first, and so does saving a file that
  somebody else changed (overwrite, reload or cancel).

* A file tree for the project or worktree in view: a column between the
  sidebar and the main pane, hidden until asked for (**Show or hide the file
  tree**: `⇧⌘E`, `Ctrl+Shift+Alt+E`, the folder button of the main header, the
  View menu and the palette; the choice is kept in `settings.json` as
  `files_visible`). Folders are listed when opened, on the machine the project
  is on, and again every five seconds, when the window gets the focus and after
  a save; the open folders and the selection are kept for each project. The
  arrows (or `j` `k` `h` `l`) move and open, `Enter` opens a file in a tab,
  `Esc` returns to the main pane, `Tab` visits it between the sidebar and the
  main pane, a click selects and a double click opens. Icons are glyphs of a
  bundled symbols font (Symbols Nerd Font Mono, MIT) in the theme's colours,
  chosen by file name and then by extension; Git's state is a letter on each
  file (`M A D R ? U`) and a dot on the folders that hold changes, and ignored
  files are dimmed.
* **Open a file of the project by name…** (`⌥⌘P`, `Ctrl+Shift+Alt+P`; `~` in
  the palette) finds a file of the project in view by part of its name, with
  the file's icon and its path, and opens it; `name:42` goes to a line. The
  ordinary `⌘P` / `Ctrl+P` palette lists matching files too, below the
  projects, sessions and machines. The list is asked for again each time it
  opens, on the machine the project is on.
* **Find in the file…** (`⌘F`, `Ctrl+F`) and **Replace in the file…**
  (`⌥⌘F`, `Ctrl+H`) open the editor's search bar; they apply only while a file
  has the keyboard, so `⌘F` still finds in a terminal and filters the sidebar.
  `Esc` closes the bar.
* **Search the text of the project…** (`⌥⇧⌘F`, `Ctrl+Shift+Alt+F`; `%` in the
  palette): the lines that match, grouped by file, with a switch for case
  (`Alt+C`) and for regular expressions (`Alt+R`). `Enter` or a click opens the
  file with the cursor on the line, a new query stops the search before it.
  On this computer it runs in Leon over the project's file list (so
  `.gitignore` is honoured; binary files and files above 1 MiB are skipped); on
  another machine it is one command there, using `rg`, else `git grep`, else
  `grep -r`. It keeps the first 1000 lines (300 are listed).

* A Markdown file can be previewed: **Preview the Markdown file** (`⌥⌘V`,
  `Ctrl+Shift+Alt+V`, or three buttons above the file) goes from the text to
  text and page side by side to the page. Tables, task lists and code blocks
  are rendered; the page follows the text as it is typed and keeps its scroll
  position when the mode changes. Web links open in the browser, relative links
  open the file in a tab (on the document's machine), relative images show when
  the document is on this computer.
* Text that was not saved survives a crash: a draft is kept beside the
  settings and put back when the file is opened again (a file that changed on
  disk meanwhile asks what to do when saved). When the window gets the focus a
  file that changed on disk is read again if it has no unsaved changes, and
  gets a **Reload** / **Keep mine** banner if it has. Quitting with unsaved
  files offers **Save all and quit**, **Quit without saving** and **Cancel**.
* The files that were open (cursor, Markdown view, the one on screen in each
  workspace) are opened again at the next start, and the file tree keeps its
  open folders and selection. **Reveal the file in the file tree** selects the
  focused file in the tree. **Open anyway** shows a file that is not text or is
  too big, on this computer, as read-only lossy text.
* An Editor section in the settings: tab size, wrapping, line numbers, indent
  guides, syntax highlighting, font size and reopening files at start, applied
  live to the files that are open.

* A **Pinned** section at the top of every machine, above its projects. A
  pinned session moves there, on top of the section: the pin at the left of its
  row, "Pin" in its menu, or dropping a session onto a pinned one pins it, and
  "Unpin" sends it back to its worktree or folder by recency. Dragging a pinned
  session, or "Move up" / "Move down", reorders the section. Every session row
  shows its pin: a pinned one always, the others while the row is hovered.

### Fixed

* Saving over a file that was deleted ("Overwrite" after "changed on disk")
  creates it again instead of doing nothing.
* Git's marks of a project whose folder is inside a larger repository (a
  monorepo's package) are read relative to that folder, on this computer and
  on other machines; they were taken as relative to the repository's root, so
  none matched.

* Unpinning a session says so in the status line ("Unpinned the session."),
  where it said "Pinned the session.".
* The sidebar's scrollbar has a column of its own: it no longer covers the
  activity lights, the WAITING and FAILED words and the counts at the end of the
  rows.

### Changed

* The sidebar lists only the active sessions by default. The toggle beside the
  filter is now **Show inactive sessions** (off by default; setting
  `sidebar_show_inactive`, replacing `sidebar_active_only`, which is ignored:
  everyone starts from the new default).
* A new worktree always starts a session at once (the default agent, else a
  shell), so it is never empty and never hidden as inactive.
* The "New agent session" menu offers every agent installed on the machine,
  Grok included, and more agents have their logo (Grok, Muse, MiMo Code,
  Antigravity, Pi, Hermes Agent, Devin, Auggie, CodeBuddy, Kilocode, Kiro,
  Trae, Qoder).

* Protocol 2: a command sent to another computer (`Exec`) can carry standard
  input, up to 4 MiB, which the host writes while it reads the output and
  closes afterwards. Over SSH the same input reaches the remote command. The
  protocol version changes from 1 to 2, so a Leon that speaks 1.x and one that
  speaks 2.x refuse each other at the version check: update both computers
  together.
* The release binary carries the grammars of the editor's languages; build
  with `--no-default-features` to leave them out (files then open as plain
  text).

* Pins are one order per machine, not one per worktree or project: a pinned
  session leaves its worktree or folder for the Pinned section. Pins already set
  keep their order there. Dropping a session onto one that is not pinned no
  longer pins it. "Move up" and "Move down" are offered for a pinned session
  only.

## [0.5.0] - 2026-10-07

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

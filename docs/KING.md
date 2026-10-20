# King Leon

King Leon is one agent that answers for every project Leon knows. You talk to
King the way you would talk to a principal engineer: ask how something works,
what state a project is in, what to build next, or to build it. King answers
short questions itself. Long investigations and changes of code go to other
agents, its **workers**. Each worker runs in a worktree of its own, on the
agent and model that cost least for the job, and King tells you what they
found or did.

King is a conversation with a coding agent you choose: Claude Code, Codex,
opencode, Grok or Cursor's agent. Leon gives that agent King's tools and
keeps the conversation in a service that runs without the window.

> Status: the service, the command line, King's tools and the window's King
> view work on macOS and Linux. Workers on other machines and the phone's
> screen come next (see "Not built yet").

## Talking to King

```sh
leon king say "What is the state of every project, and what should we do next?"
leon king status            # King and its workers
leon king log               # what King said lately
leon king log 3             # what worker #3 said lately
leon king answer q1 allow           # answer a question King waits on
leon king brain codex gpt-5.6 high   # run King on another agent or model
```

`say` prints King's answer as it comes and returns when King is done. The
service starts by itself the first time a command needs it (`leon king serve`
runs it in the foreground instead). What you say while King is working waits,
and King hears all of it together when its turn ends.

To keep King and its workers going with the window closed, from login on and
back after a failure, install the service in your session:

```sh
leon king service install [--linger]   # a systemd user unit, or a launchd agent
leon king service status
leon king service logs
leon king service uninstall
```

It is `leon host service` for King: the same rules (your own user, never
root; `--linger` keeps it running when you log out), a unit of its own
(`leon-king.service`, `dev.zavu.leon.king`), and it runs
`leon --data-dir <dir> king serve` with the `PATH` of the session that
installed it, which King's workers need.

## In the window

**The King** is a row of the sidebar's navigation, above The Den
(Ctrl+Shift+Alt+K, the palette's "Open the King"). It opens King's view in
the main pane:

* **King's conversation** on the left, with the field you speak to King in.
  The tools King used between two answers are one folded line, what Leon
  told King of its workers is a folded "news" line, and Leon's report sits
  under each answer, drawn as a strip: the turn's tokens, cache, speed,
  time and cost, the day's spending, and a bar for each agent's usage window
  with what is left and when it resets. A question King waits on is a
  framed card with its answers (allow, always, deny; start or not yet for a plan), and any
  question can be answered with what is written in the field. What a
  worker asks is framed over the field too, with the worker's number and
  title, so it is seen and answered without opening the worker (King is
  told, and may answer it as well: the first answer counts). Alt and the
  answer's number (written on its button) answer from the keyboard: the
  open worker's question first, else King's, else the first worker's. Escape stops King's turn, or
  closes the view when King is not working.
* **Images, files and dictation.** An image pasted into King's field
  (Ctrl+V, ⌘V on macOS) is kept as a file in `king/attachments/` of Leon's
  data folder and shown over the field as a thumbnail (× takes it off); it
  is sent with the words as a path, for King, and any worker King passes
  it to, to open. On Linux the image is read with
  `wl-paste` (Wayland) or `xclip` (X11), because the toolkit does not see
  images there. The paperclip attaches files the same way: shown
  by name over the field, sent as their paths.
  DICTATE uses the same microphone and speech model as the conversations
  (choose a model with `/voice` in one of them, or in the settings); Enter
  ends the dictation, Escape drops it, and what was said is written in the
  field, to be read before it is sent.
* **What King runs on** is the chip under the field. A click, or the
  palette's "Change the model King runs on…", asks for the agent, then one
  of the models the agents of this computer offer, then the effort; the
  agent's own model or effort is always on offer. King's next turn runs
  there, and the change lasts until `king.toml` is edited (see Settings).
* **How much King asks** is the chip beside it ("DEFAULT MODE" until one is
  chosen), or the palette's "Change how much King asks…": the permission
  modes of the agent King runs on, as the agent lists them (for Claude
  Code: accept edits, plan, bypass permissions…). A mode that never asks is
  drawn in the warning colour. It is kept like the model, and is
  `[brain] mode` in `king.toml`; the workers' modes are `[modes]`.
* **The workers** on the right, as cards, newest first: what each is for,
  what it runs on, its job and tier, what it cost and the last thing it
  said. Workers are not in the sidebar while they are King's.
* **A card opened** (a click, or Alt+Down and Alt+Up) shows the worker's
  conversation live, with what you can do to it: write to it (King is told
  you stepped in), stop its turn (Escape), stop the worker, or **take it
  over**.
* **Taking a worker over** takes it from King and opens it as a session of
  the window in its worktree, continuing the agent's conversation where the
  worker left it. From then on it is yours, in the sidebar like any other.

**Tasks.** King puts on each project's board what it finds and does not do
now, and the steps of its plans. A card of the board is given to King with
`a` (or "Give it to the King" in its details): King moves it to doing, gives
it to a worker, sets its branch, and moves it to review when its pull
request is open. Done is yours, after the merge.

**Conversations.** NEW under the field puts the conversation away and starts
a new one (`leon king new`). HISTORY, beside WORKERS, lists the past ones
(title, when, what they ran on); one opened is read in the conversation's
place, and TAKE IT UP AGAIN continues it: the agent's own session goes on,
and the conversation it replaces is put away in turn (`leon king history`,
`leon king resume <n>`). Each keeps its last 400 entries, in
`king/history/` in Leon's data folder.

Workers belong to the conversation that started them. WORKERS shows the
ones of the conversation on screen: the current one, or a past one you are
reading. Below them it lists the workers of earlier conversations that are
still working or asking. A conversation that goes on in a new one because it
grew too long takes its workers along.

**What a turn leaves running.** An agent may start a program that outlives
its answer (a development server, a watcher). Its turn is over once it has
said its result, even while its process goes on; RUNNING, over the right
column, lists what such turns left running (the command, whose turn, since
when) with STOP. A turn in flight shows a spinner and for how long it has
gone on.

**Terminals King opens for you.** What should keep running where you see it
(a dev server, a watcher, a preview to try) King starts with its
`open_terminal` tool: the folder and one line of command. It opens as a
session of the window, the command typed into its shell as a project's
script is, and it is yours to stop. The service keeps the request until a
window takes it, so with no window open it opens in the next one; only one
window opens it.

**King's workers in the Den.** Each worker that is still King's is a lion of
the Den: how King's status says it stands (working, asking, failed, done),
and, while it works, what its transcript says it does (the file it edits,
the command it runs), as for any session. Opening the lion opens its card in
King's view. A paired phone's Den shows them too.

The window joins King's service whenever the service runs, without starting
it, so that the Den and the terminals follow King with King's view closed.

The sidebar's row shows how King stands when it is not on screen: a dot
while King or a worker works, the waiting dot when one of them asks you
something, and how many workers King runs.

The window is one more client of King's service, as the phone and
`leon king …` are: it starts the service when it is not running, and
closing the window stops nothing King does. King's answers, questions and
failures reach the desktop as notifications only while you are not looking
at King: the window tells the service when King's view is open in the
window in front, and while it is (or the phone shows King), the
conversation is enough. What King sends you with `notify_user` always
reaches the desktop, and shows over the field.

## What every answer tells you

Under every answer of King's, Leon writes a line from what the agents and
the usage readings say, never from what the model claims:

```text
  ── This answer: 48.2k tokens (read 45.1k, 31.0k cached · wrote 3.1k) · 62 tok/s · 50s · $0.042
  ── Today: 150.2k tokens (King 48.2k, workers 102.0k) · $1.27 of $20.00 ($18.73 left)
  ── Left: claude (max): 5h 66% left, resets in 2h10m, wk 88% left · codex (plus): 5h 97% left
```

The tokens are what each agent reports for the turn. Claude Code gives them
in its result, Codex counts them as its thread goes, and opencode, Grok and
Cursor give them in their answer to the prompt. Tokens per second are the
tokens written over the whole turn. The usage windows are Leon's own
readings for this computer, the same as the usage meters. King reads the same
numbers in its instructions before every turn, and through `usage_status`,
so it can advise you: what to run now, what can wait for a reset, which agent
to save.

## Models, talents and benchmarks

Before every turn King is told every model the agents of this computer offer.
Leon asks each agent, the same way the window's conversations do. With each
model King gets what is known of it:

* **talents**, from 1 to 5: planning, architecture, coding, review,
  research, speed, economy;
* **benchmarks**, each with its source and the month it was read.

The catalogue built into Leon (`crates/leon-king/src/models.toml`) holds a
starting estimate of each family's talents and **no benchmark**: a score is
kept only with its source. King adds what it finds out with its
`model_note` tool, and you can edit the file yourself. Both write to
`king-models.toml` in Leon's data folder, which wins over the built-in
catalogue:

```toml
[[model]]
match = "deepseek-v4"          # a part of the model's id; the longest match wins
name = "DeepSeek V4"
talents = { coding = 4, economy = 5 }
notes = "Good at well-specified changes; weak at planning across modules."

[model.benchmarks.swe-bench-verified]
score = 70.1
source = "https://…"
as_of = "2026-10"
```

## Planning goes to the best models

A worker with the job `plan` (`leon king start … --plan`, or
`job: "plan"` in `worker_start`) runs where `[jobs] plan` says. By default
that is the `planning` tier, `strict`: Claude Opus at high effort, then Codex
at high effort. Asking for a cheaper tier or agent for a plan is refused.
Put your best models in that tier (see the settings below). King's
instructions say to plan with the best model and then execute step by step
on cheaper workers, with the best model reviewing their work. King plans
in its own conversation only when it runs on a planning-tier model itself.

## Spending less

Every turn of King's reads its whole conversation again, so what costs is
how often King is woken and how long its conversation is. Two things keep
both down:

* **Leon answers the workers' routine questions itself.** An edit of a file
  in the worker's own worktree, a command that only reads (`ls`, `rg`,
  `git log`, `cat`…), and tests, builds and servers run in the worktree are
  allowed without waking King; the worker's transcript says "Leon allowed
  it". A push, a write out of the worktree, a `git reset --hard`, or a
  command Leon cannot read for sure (`$VAR`, `$(…)`, an unknown program)
  still goes to King. A tool of an MCP server (Ahrefs, a database, a
  browser) is allowed when its name only reads. A name with a word of
  `mcp_writes` in it (`create`, `delete`, `send`, `pay`, `generate`…) still
  goes to King. The programs and the words are lists in `[permissions]`;
  `routine = false` sends every question to King.
* **A conversation that grew too long goes on in a new one.** When a turn
  of King's reads `[limits] compact_at_tokens` or more (600k by default),
  Leon puts the conversation in HISTORY and King goes on in a new one,
  told what was asked last and what it answered, so the next turn reads a
  fraction. NEW does the same by hand.

## Workers

King starts workers with its `worker_start` tool. You can also start one
yourself:

```sh
leon king start "fix the login bug" --project leon --tier cheap \
    --prompt "Users are logged out after 5 minutes. Find out why and fix it."
leon king tell 1 "Add a test for it too."
leon king stop 1
leon king release 1    # take the worker over: King lets it go
```

A worker works in a new worktree of the project, on a branch of its own
(`king/<title>-<n>`, under `worktree_location`'s default place). A `code` job
changes code and commits on that branch. A `research` job only reads and
reports. A `plan` job writes a plan and changes nothing. A worker is never started in a project's main checkout, and never on
the default branch.

### Workers on other machines

A project on another machine (one reached over SSH, or a computer paired
through a relay, see [REMOTE.md](REMOTE.md)) gets its workers there. The
service makes the worktree there with `git`, starts the agent there the way
the window's conversations do (over `ssh`, or in a terminal of the other
computer's host behind the bridge), and runs `worktree_check`, `worktree_diff`,
`pr_open` and `project_status` there. `gh` then needs to be signed in on that
machine. A worker of an existing folder there names the machine:
`worker_start` and `open_terminal` take `machine`, by the name Leon shows.
The agent has to be installed on that machine; the router only knows what is
installed here, so a missing agent is a worker that fails to start, and King
hears why. A check that runs past its time limit over SSH loses its
connection, and may go on running on the other machine.

King is never asked to poll. When a worker finishes, fails or asks something,
Leon gathers the news for a few seconds and then wakes King with it. If you
speak first, the news comes with your message. King answers a worker's
questions (`worker_answer`), tells it more (`worker_send`), reads what it did
(`worker_transcript`, `worktree_diff`), and opens a pull request when the work
is right (`pr_open`).

## Evidence, then a pull request, then CI

A worker saying it is done is not evidence, so King checks the work before it
reports it done:

1. **The project's check.** `worktree_check` runs the project's own check in
   the worker's worktree, on its last commit. Leon picks the check from the
   project's files: `scripts/check.sh`, a `check:` target in the `Makefile`,
   `cargo test`, the package's `check` or `test` script, `go test ./...` or
   `pytest`. You can name a different one under `[checks]`. The tool answers
   at once. King hears how the check ended as news, with the end of what it
   printed. The whole output is in `<data dir>/king/checks/<worker>.log`. A
   check that runs past `check_timeout_secs` is ended, along with everything
   it started.
2. **The pull request.** `pr_open` opens a pull request only for a branch
   whose last commit passed the check. The description ends with what was
   run, on which commit and how long it took ("Verified by Leon:
   `scripts/check.sh` passed on abc1234 in 4m14s."). When a project cannot be
   checked on this computer, King must say why (`unverified`), and that
   reason goes in the description instead. Calling `pr_open` again on a
   branch whose pull request is already open pushes to it.
3. **CI.** From then on Leon watches the pull request's checks with `gh`,
   every `ci_poll_secs`, with no King turns spent. When the checks settle,
   King hears either that they all passed or which ones failed. If they
   failed, King has the worker fix them on the same branch, checks again and
   pushes. If they passed, you get a desktop notification that the pull
   request is ready for your review. A pull request with no checks after ten
   minutes is taken as one its project runs no CI on.

Each worker's card shows the evidence. The check line reads `✓ CHECKED
abc1234 · 4m14s`, `✗ CHECK FAILED`, or `CHECKING` while it runs. The pull
request line reads `PR #62 · CI ✓`, `CI ✗ clippy`, `CI RUNNING`, `NO CI` or
`MERGED`. Click the pull request line to open it.

## Runs: a task carried out as its steps

Work that takes several workers one after another (a schema, then the API
on it, then the page) is a **run**. Its steps are tasks of the board whose
parent is the task to carry out, each with the steps it needs done first:

```text
leon task edit <step> --parent <task> --need <other step>
```

King makes them with `task_create` (`parent`, `needs`), and the drawer of a
task lists its steps and its run. `leon task` refuses a parent or a need on
another board, a task that would be its own parent or step, and needs that
go round in a circle.

A run starts from the board (**Run its steps**, or `r` on the card), from
King (`run_start`) or from the command line:

```text
leon king run <task> [--parallel 2] [--budget 5]
leon king run continue <run> [what to tell the failed steps' workers]
leon king run cancel <run>
```

Then Leon carries it out without King deciding each start:

1. **A step starts once every step it needs is done**, at most
   `run_parallel` of a run at once, within `max_workers`. Its worktree's
   branch, `king/run<n>-<step>`, starts from the branch of a step it needs,
   with the branches of the others merged in, so the work it builds on is in
   its folder. When a merge conflicts, the worker is told which files
   conflict and resolves them first. A step with no needs starts from the
   project's checkout.
2. **Its worker comes from its tier.** A step's label that names a tier
   (`cheap`, `premium`…) picks it; a `research` label makes it a research
   job. Otherwise it is a code job, on the tier `[jobs]` gives code.
3. **When the worker finishes, Leon commits what it left** (the step's
   title is the message) **and runs the project's check** on that commit.
   The step is done when the check passes, and its task moves to *done*. A
   failed check goes back to the same worker, with the end of what it
   printed, up to `run_fixes` times. A project Leon cannot check has its
   steps done unchecked, and the news says so.
4. **King hears twice at most.** When the run is done, the news says which
   branch holds the whole work: the last step's, ready for `worktree_diff`
   and `pr_open`. The task moves to *review*. When the run is blocked (a step
   failed, its worker was stopped, or its workers spent the run's budget),
   King hears why and goes on with `run_continue` or calls it off with
   `run_cancel`. Steps' own turns are not news; their questions are, as any
   worker's.

A worker that is a run's step shows `RUN #1 · STEP 2 OF 3` on its card. A
restart of the service tells the steps that were working to go on, and
checks again the ones that were being checked, with no King turn.

## What King may do

| King may | King never does |
| --- | --- |
| Create worktrees, start, steer and stop workers | Merge a pull request |
| Commit on a worktree's branch (through its workers) | Push to a project's default branch |
| Push a worker's branch and open a pull request | Force a push |
| Tell you on your desktop that something is done | Work in a project's main checkout |

These rules are written into King's instructions, and the tools enforce them
too. `pr_open` refuses the default branch, a worktree with changes that are
not committed, and a last commit with no passed check (`verify_before_pr`).
Nothing is pushed with `--force`, and no tool merges.

## Choosing agents to save

Every worker is either given a **tier** or a named agent and model:

* a tier (`cheap`, `standard`, `premium`, or one you define) takes the first
  of its agents that is installed and has room in its usage window;
* an agent King names is taken as named, unless it is not installed or is
  past the line. Then King is told why, and which agent would do instead.

"Room" is what Leon reads of each agent's usage limits (the same readings as
the usage meters, for this computer). An agent whose largest window is past
`limit_threshold` gets no new work. An agent Leon cannot read is not held
back. Once the day's spending reaches `daily_budget_usd` (as the agents
report their cost), no worker starts until you say so. King sees all of this
through `usage_status`.

**Leon learns from what workers did.** Every worker leaves an outcome: each
run of its project's check and whether it passed, whether the CI of its pull
request passed, whether a turn of it failed, what it cost and how long it
took. They are summed per agent, model and kind of job (`code`, `research`,
`plan`) into a record, kept with King's state. A worker is judged once a
check ran, its CI ended or it failed, and it did well when its last check
passed, its CI did not fail and no turn failed.

King reads the record in `models_list`, beside each model's talents. A tier
also reads it: a candidate that did poorly at the kind of job (of at least
`learn_after` judged workers, fewer than `learn_below` did well) is passed
over while a later candidate has room, and `worker_start` says so under
`passed_over`. When no other candidate has room, the tier's order wins. A
model King names is taken as named.

## Settings: `king.toml`

Nothing of King's choices is fixed in code. Leon's defaults are a file,
[`crates/leon-king/src/defaults.toml`](../crates/leon-king/src/defaults.toml),
with a comment on every key. Your `king.toml` in Leon's data folder is read
over it **key by key**, so you write only what you want different:

* a table you name (`[brain]`, `[limits]`…) changes only the keys you write
  in it;
* a tier you name replaces that tier alone; a tier set to `[]` is removed;
* a kind of job you name in `[jobs]` changes where that kind goes.

```toml
[brain]                 # King itself: Opus 5.5 at high effort
model = "opus"
effort = "high"

[limits]
daily_budget_usd = 20.0

[tiers]                 # plans on Fable first, then Opus, then GPT-6
planning = [
    { agent = "claude", model = "fable", effort = "high" },
    { agent = "claude", model = "opus", effort = "high" },
    { agent = "codex", model = "gpt-6.1-sol", effort = "high" },
]
local = [{ agent = "opencode", model = "ollama/gemma4:26b" }]

[jobs]                  # research on the local model; plans stay strict
research = { tier = "local" }

[guards]
protected_branches = ["main", "master", "release"]

[checks]                # how a project is checked, when its files do not say
api = "make lint test"

[modes]                 # the permission mode each agent's workers start in
claude = "auto"
```

| Section | What it sets | Default |
| --- | --- | --- |
| `[brain]` | The agent, model, effort and mode King runs on; `mode = ""` is the agent's own | Claude Code, its own model, mode `auto` |
| `[limits]` | `max_workers`, `daily_budget_usd`, `limit_threshold`, `wake_delay_secs`, `compact_at_tokens`, `check_timeout_secs`, `ci_poll_secs`, `run_parallel`, `run_fixes`, `learn_after`, `learn_below` | 6, none, 0.85, 5, 600000, 1800, 60, 2, 1, 5, 0.5 |
| `[tiers]` | Each tier's candidates in order: agent, model, effort | `cheap`, `standard`, `premium`, `planning` |
| `[jobs]` | The tier each kind of job (`code`, `research`, `plan`) goes to, and whether it is `strict` (only ever that tier) | code: standard, research: cheap, plan: planning (strict) |
| `[guards]` | Branches nothing is pushed to, besides each project's default branch; `verify_before_pr`: a pull request needs a passed check of its last commit | `main`, `master`; on |
| `[checks]` | The command that checks each project, by name, run with `sh -c` in a worktree | read from each project's files |
| `[permissions]` | Which worker questions Leon answers itself: `routine`, the `read_only` programs and the `in_worktree` ones, and MCP tools that only read (`mcp_reading`, `mcp_writes`) | on, with lists of common tools and words |
| `[modes]` | The permission mode of each agent's workers; `""` is the agent's own | codex `auto`, claude `acceptEdits`: a worker edits in its own worktree without asking |

`leon king config` prints the configuration in force (your file over the
defaults), and `leon king config --defaults` prints Leon's defaults with
their comments. A mistake in your file is an error that names the line, never
a silent default.

`leon king brain` changes King's agent without editing the file. If you edit
the file's `[brain]` later, the file wins. Changing King's agent starts a new
conversation; changing its model or effort keeps the same one.

## How it works

* **The service** (`leon king serve`) owns King's conversation and every
  worker. It runs each turn as a process of the agent, the same way the
  window's conversations do (`leon-turn`'s adapters on a pipe). It saves its
  state to `<data dir>/king/state.json` after every change. A turn does not
  survive a restart: King is told which workers were interrupted, and can
  tell them to go on.
* **The control socket** is a Unix socket in a folder only you can open. Only
  processes of your own user are served. It speaks one JSON request per line
  (`leon_king::protocol`), and clients may subscribe to changes.
* **King's turn** runs in `<data dir>/king`. Before every turn the service
  rewrites that folder's `AGENTS.md` and `CLAUDE.md` with King's
  instructions and the projects Leon knows. The agent is given three servers
  of tools: `leon king mcp` (King's own), `leon memory mcp` (the shared
  memory, see [MEMORY.md](MEMORY.md)) and `leon task mcp` (the board, see
  [TASKS.md](TASKS.md)). Claude Code is also given the roots of the local
  projects as folders it may read.
* **The core** is the `leon-king` crate: the configuration, the coordinator's
  state machine, the router, the transcripts and King's instructions. It is
  pure, and tested without processes.

## On the phone

A paired phone speaks to King of the computer it paired with, through that
computer's host (`King` lines, `docs/REMOTE.md`): the host of the window and
`leon host` both pass them on. The computer's screen in the phone shows a
King row when its host says it can; it opens King's conversation, a field
to say something, and King's workers and runs, followed live while the
screen is open. `leon-mobile` offers it as `king_ask` (one request of King's
protocol and its answer) and `king_watch` (King's notices).

## Not built yet

* From the phone: answering King's or a worker's questions, telling a
  worker more, stopping it, and starting or going on with runs. The phone
  reads King, its workers and its runs, and says things to King.
* King's notices on the phone while the application is closed: they come
  only while a King screen is open.
* Agents that only run in a terminal (Kimi, ZCode, Antigravity) as workers.
* Showing the tokens in the window's own conversations and on the phone:
  the event is read, and only King uses it so far.

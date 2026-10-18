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

> Status: the first stage. The service, the command line and King's tools
> work on macOS and Linux. The window's King view, workers on other machines
> and the phone's screen come next (see "Not built yet").

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
`job: "plan"` in `worker_start`) always runs on the `planning` tier. By
default that is Claude Opus at high effort, then Codex at high effort.
Asking for a cheaper tier or agent for a plan is refused. King's
instructions say to plan with the best model and then execute step by step
on cheaper workers, with the best model reviewing their work. King plans
in its own conversation only when it runs on a planning-tier model itself.

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

King is never asked to poll. When a worker finishes, fails or asks something,
Leon gathers the news for a few seconds and then wakes King with it. If you
speak first, the news comes with your message. King answers a worker's
questions (`worker_answer`), tells it more (`worker_send`), reads what it did
(`worker_transcript`, `worktree_diff`), and opens a pull request when the work
is right (`pr_open`).

## What King may do

| King may | King never does |
| --- | --- |
| Create worktrees, start, steer and stop workers | Merge a pull request |
| Commit on a worktree's branch (through its workers) | Push to a project's default branch |
| Push a worker's branch and open a pull request | Force a push |
| Tell you on your desktop that something is done | Work in a project's main checkout |

These rules are written into King's instructions, and the tools enforce them
too: `pr_open` refuses the default branch and a worktree with changes that
are not committed, nothing is pushed with `--force`, and no tool merges.

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

## Settings: `king.toml`

The file is `king.toml` in Leon's data folder. Every key is optional.

```toml
[brain]                 # the agent King runs on
agent = "claude"
model = "opus"
effort = "high"
# mode = "acceptEdits"

[limits]
max_workers = 6         # working at the same time
daily_budget_usd = 20.0 # no ceiling when absent
limit_threshold = 0.85  # share of a usage window past which an agent gets no new work
wake_delay_secs = 5     # how long news is gathered before King is woken

[tiers]                 # replaces the default tiers when present
cheap = [{ agent = "opencode", model = "deepseek/deepseek-v4" }, { agent = "cursor" }]
standard = [{ agent = "codex", effort = "medium" }, { agent = "claude" }]
premium = [{ agent = "claude", model = "opus", effort = "high" }]
planning = [{ agent = "claude", model = "opus", effort = "high" }, { agent = "codex", effort = "high" }]

[modes]                 # the permission mode each agent's workers start in
claude = "acceptEdits"
```

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

## Not built yet

* The window's King view: King's conversation as the main view, and workers
  as cards you can watch, step into and pull out to the sidebar. Workers stay
  out of the sidebar while they are King's.
* King on the phone, through the paired host.
* Installing the service as a background service of your session
  (`leon king service install`), the way `leon host service` is.
* Workers on SSH and relay machines, and agents that only run in a terminal
  (Kimi, ZCode, Antigravity) as workers.
* Showing the tokens in the window's own conversations and on the phone:
  the event is read, and only King uses it so far.

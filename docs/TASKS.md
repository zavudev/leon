# The board of tasks

Agents find work they do not do now, and people keep a list of what to do. Leon
keeps one board for both: tasks in Leon's own database, a command line to reach
them, and an MCP server so an agent can use them without a shell.

The board is in the window (`⇧⌘I`, `Ctrl+Shift+Alt+I` elsewhere), in the
command line (`leon task`) and in an MCP server (`leon task mcp`), all reading
and writing the same database. The synchronisation with GitHub issues and
Linear: GitHub issues are in (see below); Linear comes next and is not in Leon yet.

Nothing here writes into a project or into an agent's configuration. `leon task
mcp` is registered by the person who wants it.

## Tasks and boards

A task is a **title** (one line, at most 200 characters), a **body** (at most
20,000), a **status**, a **priority**, up to 10 **labels**, an **assignee** and,
when known, the **agent** that created it.

| Status | Meaning |
| --- | --- |
| `backlog` | Known, not planned |
| `todo` | Planned, not started (the default) |
| `doing` | Being worked on |
| `review` | Done, waiting to be looked at |
| `done` | Finished |
| `canceled` | Dropped without being done |

Priorities are `none`, `low`, `medium`, `high` and `urgent` (0 to 4). The words
the trackers use for the same columns are read too: `in-progress`, `started`,
`in-review`, `closed`, `cancelled`.

A task belongs to a board: the one of a **project**, which is the project the
current folder belongs to and is shared by all its git worktrees (resolved
exactly as the [shared memory](MEMORY.md#entries-and-scopes) resolves it), or
the **global** one (`--global`), which belongs to no project. A task of
another project is not found by its id from this one.

A task can be a **step** of another: it names its **parent**, and the steps
of the same task it **needs** done before it can start. Neither crosses a
board; a task is never its own parent or step, and needs never go round in a
circle. Deleting a task leaves its steps steps of nothing and drops it from
what others need. Orchestrator carries a task out as its steps in that order: a
[run](KING.md#runs-a-task-carried-out-as-its-steps), started from the card
(`r`, or **Run its steps** in its drawer, which lists the steps and how the
run goes).

Cards are ordered inside a column by a position: a new task goes to the bottom
of its column, `move --before <id>` places it above another card.

## Not duplicating

Creating a task whose title (case, spacing and surrounding punctuation
aside) an **open** task of the board already has creates nothing and answers
with that task. An agent that retries, or two that find the same chore, do not
fill the board. A done or canceled task does not count: the same chore can come
back. A board holds at most 100,000 tasks, a guard against an agent that
creates in a loop.

## The board in the window

The board is the main pane, as the Den is, and the sidebar stays beside it. It
opens from the **Tasks** row under Home, `⇧⌘I` (`Ctrl+Shift+Alt+I` on Linux and
Windows), **Show the task board** in the palette, or `leon --tasks` at the
start; the same command, `Esc` or the row closes it and puts back what the
pane showed. **New task…** in the palette creates a task from anywhere. The
columns are `backlog`, `todo`, `doing`, `review` and `done` (a canceled task
has no column; `leon task list --all` shows it).

The header names the project and counts what is open and done, and has one
chip per project of this computer and **Global**. A project on another
machine has no board here: its tasks live on that machine.

### Dragging

Drag a card with the pointer to any place of any column: a line marks where it
would land, the column it is over lights up, and an empty column takes the
card in all its height. Letting go outside the columns, or `Esc`, drops
nothing. Dropping a card where it already is changes nothing.

### Keys

| Key | What it does |
| --- | --- |
| `←` `→` or `h` `l` | Cursor to the column on the left or right |
| `↑` `↓` or `k` `j` | Cursor to the card above or below |
| `Shift` + those | Carry the card: to the bottom of the next column, or one place up or down in its own |
| `w` or `Enter` | Create the card's worktree (see below), or open the one it has |
| `e` | Open the card's details beside the columns, or close them |
| `n` | Open the panel for a new task, in the cursor's column, with the title ready to type |
| `s` | Sync with GitHub now, or ask to turn it on |
| `a` | Give the card to the Orchestrator (also a button of the details): Orchestrator's view opens, and Orchestrator is told the task, its project and its id, and works it (see [KING.md](KING.md)) |
| `d` | Finish the card |
| `p` | Raise its priority: none, low, medium, high, urgent, none |
| `g` | Switch between the project's board and the global one |
| `Esc` | Let go of a drag, then close the details, then close the board |

A click on a card opens its details (below). Every change goes through the engine, as every write of the window
does, so the status line says what was done (`Moved 1a2b3c4d to doing: ...`).
The open board reads the store every second and a half because an agent writes
from another process; a card that moved is followed by the cursor.

### A new task

`n`, **New task** in the header, the **+** of a column and the palette's
**New task…** (while the board is open) open the same panel as the details of a
card, over the right edge of the board, **with the title field already holding
the keyboard**: type and press `Enter`. The panel also takes the description,
the column it starts in (the one it was asked from), the priority, who it is
for and the labels, so a task is made whole. `Enter` in the title or the
assignee, `Cmd+Enter` / `Ctrl+Enter` anywhere and **Create task** make it;
`Enter` in the labels adds one; `Esc` throws it away. A title left empty is
refused where it is typed. The cursor goes to the new card when it shows up.

### The details of a card

A click on a card, or `e` on the one under the cursor, opens a drawer over the right edge of the board. It floats: the columns keep
their width and nothing is pushed, and the cards that are not under it can
still be dragged. It holds what a card cannot show:

* the **title** and the **description**, which has several lines;
* the **status** and the **priority**, as buttons that apply at once;
* who it is **assigned to** (the agent's mark when it is one);
* the **labels**: each with its way out, and a field that adds one with Enter;
* **where the work is**: the worktree it is linked to, whether it is active and
  what its session is doing, or that there is none yet;
* when it was created, by whom, and when it last changed;
* every **action**, always in view at the bottom: *Create worktree* (or *Open
  worktree*, and *Open session*), *Mark done* (or *Reopen*), *Cancel task*, *Delete* (it asks
  twice) and *Save changes*.

The text fields are saved as one patch of only what changed: when asked
(`Cmd+Enter` or `Ctrl+Enter`, `Enter` in the title or the assignee, or the
button), when the details close and when another card is chosen. A title that
would be empty is refused where it is typed. While a field has the keyboard,
`n`, `d`, `w` and the other keys of the board are typed, not run: only `Esc`
(save and close), `Enter` as said and `Cmd+Enter` are the drawer's.

The drawer is of one task by its id. A task that an agent moves or cancels
while it is open is still the one shown; one that is deleted elsewhere says
"removed elsewhere". Reading the board again never touches what is being
typed.

### Create the worktree of a task

`w` on a card of a project's board (or **Create worktree** in its details) asks
which agent codes it (the agents installed here that take a prompt, or **Only a
worktree**), then:

1. makes a worktree named from the title and the task's short id
   (`fix-the-login-1a2b3c4d`; a number is added if git has that name);
2. starts the agent in it with the task as its prompt: the title, the body and
   how to move the card to review when it is done (`"$LEON_BIN" task move <id>
   review`), behind the project's `setup` when its `leon.toml` has one;
3. **only then** writes the card: its branch, the column `doing` and, when
   nobody had it, the agent as assignee.

A worktree that git refuses leaves the card where it was and says why.

**The link.** A card with a branch is linked to the worktree of that branch,
and says so wherever it is seen:

* on the card: a dot and the branch, and under it what the work is doing,
  from the sessions that run in that worktree — `ACTIVE · WORKING`,
  `ACTIVE · READY FOR YOU`, `ACTIVE · WAITING FOR YOU`, `ACTIVE · ASKS YOU`,
  `ACTIVE · FAILED`, `ACTIVE · AT THE PROMPT`, or `WORKTREE · NO SESSION` when
  none is open;
* in the details: the same phrase, the branch, the folder and how many
  sessions are open, with **Open worktree** (selects it in the tree) and
  **Open session** (goes to its terminal);
* when the worktree is removed, the card says `worktree removed` and
  **Create worktree** makes it again.

`w` on a linked card opens the worktree instead of making another. The global
board cannot create worktrees: it has no project to make one in.

## GitHub issues

A project can be synced with the issues of its GitHub repository, through the
`gh` command line you are already signed in with (`gh auth login`). It is
**off** until you turn it on, per project, and it only works for projects of
this computer.

### Turning it on

The **GITHUB** chip in the header (or `s`) asks `gh` which repository the
project's folder is and opens a panel that names it and says what will
happen. Nothing is written until **Turn on**:

* the labels `status:backlog`, `status:todo`, `status:doing`, `status:review`
  and `priority:low` to `priority:urgent` are made in that repository;
* its issues are read, and open ones become tasks;
* from then on a task and its issue are kept equal.

The repository you confirmed is remembered and named on every call (`-R`), so
a fork never syncs with its upstream by accident. `Turn off` stops syncing and
changes nothing on GitHub; the links stay.

### What is kept equal

| Task | Issue |
| --- | --- |
| done | closed |
| canceled | closed as *not planned* |
| backlog, todo, doing, review | open, with the label `status:<column>` |
| priority low to urgent | the label `priority:<level>` |
| other labels | the same labels (lower case) |
| title, description | the same |
| assignee | read from the issue into an empty one; never written (agents are not GitHub users) |

An open issue with no `status:` label is `todo`. What a task cannot hold is
cut and never written back: titles of more than 200 characters, more than ten
labels, and a description of more than 20,000 characters (such a card says so
and its description stays on GitHub).

### How a conflict is decided

Each link remembers what both sides said at the last sync. A field changed on
one side only goes to the other; a field changed on both is decided by which
was changed last; labels are merged as sets, so an addition or a removal on
either side survives. Syncing twice with nothing changed in between writes
nothing on either side. If a task is edited while a sync runs, that write is
skipped and the pair is tried again at the next run.

### When it runs

When the board opens, every minute and a half while it is open, and on **Sync
now** (`s`). A failure (`gh` missing, not signed in, offline) applies nothing
and shows one line, such as `run gh auth login`.

### What it never does

* **Open an issue by itself.** An issue is opened by **Create GitHub issue** in
  one card's details, and only for that card: agents write tasks too, and
  those are not published on their own.
* **Delete or close an issue you did not close** (a done task closes its
  issue; a deleted task only unlinks it).
* **Import a closed issue**, or import again an issue whose task you deleted or
  unlinked (it is remembered).

On a card the issue shows as `GH #12`; in its details the GitHub group has
**Open on GitHub**, **Unlink** and, when it has none, **Create GitHub issue**.

## The command line

```text
leon task add [--global] [--status S] [--priority P] [--label L]... [--assignee A]
              [--agent A] [--body B | --body -] [--json] <title...>
leon task list [--global] [--status S] [--all] [--assignee A] [--label L] [--limit N] [--json]
leon task board [--global]
leon task show <id> [--json]
leon task edit <id> [--title T] [--body B | --body -] [--status S] [--priority P]
               [--label L]... | --no-labels] [--assignee A | --no-assignee]
               [--branch B | --no-branch] [--parent P | --no-parent]
               [--need N]... | --no-needs]
leon task move <id> <status> [--before <id>]
leon task done <id>
leon task rm <id>
leon task search [--global] [--limit N] [--json] <words...>
leon task mcp
```

| Command | What it does |
| --- | --- |
| `add` | Creates a task. The title is the remaining words. `--body -` reads the body from standard input. `--agent` names the creator; without it Claude Code, opencode and Gemini are recognised from the environment they give their commands. |
| `list` | The board's tasks in the order of the board. Done and canceled ones are left out unless `--all` or `--status` asks for them. |
| `board` | The columns, each with its tasks. |
| `show` | One task in full (`--json`: every field). |
| `edit` | Changes what is named; what is not stays. `--label` replaces the labels, `--no-labels` clears them. `--parent` makes it a step of another task; `--need` (repeated) replaces what it waits for, `--no-needs` clears it. |
| `move`, `done` | Moves a task to a column, to the bottom of it or above `--before`. `done` is `move <id> done`. |
| `rm` | Removes a task for good. |
| `search` | Full-text search of titles and bodies, of the project's board and the global one together. |
| `mcp` | Serves the board over MCP on standard input and output. |

An id is the one printed in the first column of a listing, or the end of an id
of at least six characters. `--json` prints the tasks as JSON with every field,
the times in milliseconds since the Unix epoch. The data folder is `--data-dir`
(before the word `task`), else the `LEON_DATA_DIR` a terminal of Leon was given,
else the platform's.

## The MCP server

`leon task mcp` speaks the Model Context Protocol over standard input and
output. Register it in the agent's own configuration, for example
`claude mcp add leon-tasks -- leon task mcp`. The project is the folder the
agent was started in (`CLAUDE_PROJECT_DIR` when the client sets it).

| Tool | What it does |
| --- | --- |
| `task_create` | Adds a task, or answers the open one with the same title (`created: false`); `parent` and `needs` make it a step. |
| `task_list` | The open tasks in the order of the board; `include_closed`, `status`, `assignee`, `label`, `parent` (only its steps), `scope`, `limit`. |
| `task_get` | One task in full. |
| `task_update` | Changes title, body, status, priority, labels, assignee, the branch of its worktree, its `parent` or its `needs`. |
| `task_move` | Moves a task to a column, to the bottom or above another task. |
| `task_search` | Searches titles and bodies of this project's board and the global one. |

No tool deletes a task: an agent that no longer wants one cancels it, and
removing for good is `leon task rm`, the person's.

`leon task mcp --anywhere` is the server Orchestrator is given: Orchestrator runs in no
project, so every board is its own, and `task_create`, `task_list` and
`task_search` take `project`, the root folder of the project whose board it
is. Every task's JSON names its board's `project`.

## Where it is stored

In `leon.db`, table `task` (migrations 20, 21 and 23) with a full-text index over the
titles and bodies, scoped like the memory's entries by the identity of the
project's root folder, so removing a project and adding it again loses nothing.
The store announces each write (`StoreChange::Tasks`) so a window reading it
can refresh.

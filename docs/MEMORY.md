# The shared memory

Agents forget. What one session decides, agrees on or finds out the hard way is
gone when the session ends, and another agent never knew it. Leon keeps a
memory that every agent started in one of its terminals can read and write:
Claude Code, Codex, opencode, an agent of your own. It is a small, local
thing: entries of a few lines in Leon's own database, a command line to reach
them, and a file an agent reads before it starts.

Nothing of it is on unless you turn it on. Leon writes nothing into a project
and nothing into an agent's configuration by itself; `leon memory enable` (or
the palette's **Turn on agent memory for this project**) is the step that
does, and it names every file it touches.

## Entries and scopes

An entry is one fact: a **kind** (`decision`, `convention`, `discovery`,
`preference` or `note`), a short **title** (the first line of the text when
none is given), the **text** (at most 8,000 characters; the title at most 120)
and, when known, the **agent** that wrote it last. An entry may have a
**topic**, a short key such as `auth/token-format` (letters, digits and
`- _ . / :`, at most 64 characters).

| Limit | Value | Why |
| --- | --- | --- |
| Text of an entry | 8,000 characters | One fact, with its reasons; the memory file shows its start and `show` the whole |
| Title | 120 characters | One line |
| Topic, agent tag | 64 characters | A key, a name |
| Pinned entries | none | The memory file's size is the limit |
| Live entries per scope | 100,000 | Not a size to reach: it stops an agent that saves in a loop |
| Entries in one `list` or `search` | 500 | |
| Memory file | 12,000 bytes, or the setting (2,000 to 200,000) | What an agent reads in every session |

An entry belongs to one of two scopes:

* **A project.** The project is the one the current folder belongs to: the
  project of Leon's sidebar that owns the folder, and for a folder Leon was
  never shown, the git repository it is in. Every git worktree of a project
  shares the project's memory, also a linked worktree that lives outside the
  main checkout. A folder outside any repository is its own project. The
  project is identified by the path of its root, so removing a project from
  the sidebar and adding it again loses nothing; moving or renaming the folder
  starts a new, empty memory.
* **Global** (`--global`): what holds for every project on this computer.

## Keeping it from degrading

A memory that is only added to gets worse with use: the same fact piles up,
the fact that changed sits beside its old self, and what is wrong stays. Five
rules work against that.

* **A topic revises.** Saving with `--topic <key>` when a live entry of the
  scope already has that topic changes that entry in place: the same id, the
  new text, kind and (unless one was given by hand) title, and a **revision**
  count one higher. So "the token format" is one entry that is always
  current, not a history of them. The same topic in another project, or in
  the global scope, is another entry. Without a topic nothing is revised.
* **The same text is saved once.** Each entry keeps a hash of its text with
  case folded, white space collapsed and the punctuation around it trimmed.
  Saving a text that a live entry of the scope already says adds nothing: the
  answer is `Already known as <id>`, and that entry counts the repeat and
  when it was last seen (`duplicates`, `last_seen_at` in `--json`). This is
  checked first, so a known text is not saved again under a new topic either.
* **Pins.** `pin <id>` marks an entry every session must see. Pinned entries
  come first in their scope's section of the memory file, under `### Pinned`,
  are marked `[pinned]` in `list` and `search`, and are the last to be left
  out when the file's size is tight: they are shown in full within half of
  the file, and as a line each beyond that. Any number of entries can be
  pinned; the size of the file is what limits how much of them is read.
* **Forgetting is soft.** `forget <id>` marks the entry forgotten: it leaves
  the memory file, `list`, `search` and the count of the scope's entries, and
  is no longer pinned. `list --forgotten` shows what is forgotten and
  `restore <id>` brings one back, unless a live entry has taken its topic or
  says its text in the meantime, or the scope is full. A forgotten entry does
  not stand in the way of saving its text or its topic anew. Forgotten
  entries are removed for good 30 days later: by `purge [--older-than
  <days>]`, and by the next `forget` anybody runs, so nothing piles up when
  nobody purges. `forget --hard <id>` removes an entry for good at once.
* **Edit instead of adding.** `edit <id>` changes an entry's text, kind,
  title or topic; what is not given stays. It counts as a revision. An edit
  into what another live entry of the scope already says, or into a topic
  another one has, is refused and names that entry.

## The command line

```text
leon memory add [--global] [--kind K] [--title T] [--topic KEY] [--agent A] <text...>
leon memory edit <id> [--kind K] [--title T] [--topic KEY | --no-topic] [<text...> | -]
leon memory show <id> [--json]
leon memory search [--global] [--limit N] [--json] <words...>
leon memory list [--global] [--forgotten] [--limit N] [--json]
leon memory pin <id>
leon memory unpin <id>
leon memory forget [--hard] <id>
leon memory restore <id>
leon memory purge [--older-than <days>]
leon memory context
leon memory path
leon memory mcp
leon memory enable [--mcp] [--dry-run]
leon memory disable [--dry-run]
leon memory status
```

| Command | What it does |
| --- | --- |
| `add` | Saves one entry in the project's memory (`--global`: the global one). The text is the remaining words; it is read from standard input when it is `-`, or absent while standard input is not a terminal. `--agent` names the writer; without it Claude Code, opencode and Gemini are recognised from the environment they give their commands. |
| `search` | Full-text search of the project's memory and the global one together (`--global`: the global one alone), best match first, with an excerpt. Every word must match, case and accents do not count, the last word matches as a prefix. |
| `edit` | Changes one entry: a new text (the remaining words, or `-` for standard input), `--kind`, `--title`, `--topic` or `--no-topic`. Without a text the text stays. |
| `show` | Prints one entry in full: every field, then the whole text (`--json`: the entry as JSON). The memory file and `search` show only the start of a long entry. Works for an entry of this project and for a global one, also a forgotten one, which it says. |
| `list` | The project's entries (`--global`: the global ones), the pinned ones first, then the last changed first. `--forgotten` lists the forgotten ones instead. |
| `pin`, `unpin` | Pins one entry, or unpins it. |
| `forget` | Forgets one entry, by its id or by the eight characters shown for it; `--hard` removes it for good. Only an entry of this project or a global one, like every command that takes an id. |
| `restore` | Brings a forgotten entry back. |
| `purge` | Removes for good every entry forgotten more than `--older-than` days ago (30 without it), in every project. |
| `context` | Prints the memory file of this project: what an agent reads. |
| `path` | Prints where that file is (and writes it, so it is there to be read). |
| `mcp` | Serves the memory to an MCP client over standard input and output. |
| `enable` | The opt-in, see below. `--dry-run` prints what would change and changes nothing. |
| `disable` | Removes what `enable` wrote. |
| `status` | The project's root, the files, how many entries there are (and how many of them pinned, and how many forgotten), which instruction files hold the block, whether `.mcp.json` registers the server, and what was answered when Leon asked about the project (or that it was turned on or off by hand). |

`add` says which of the three it did: `Saved <id> ...`, `Revised <id>
(revision 3) ...` or `Already known as <id> ...`. A line of `list` or `search`
shows `[pinned]` before the title of a pinned entry and, after it, the topic
and the revision when it is above 1.

`--json` prints entries as a JSON array (`id`, `scope`, `root`, `kind`,
`title`, `text`, `agent`, `topic`, `revision`, `duplicates`, `pinned`,
`created_at`, `updated_at`, `last_seen_at` and `forgotten_at` in milliseconds,
and `snippet` for a search). `--data-dir <path>` names Leon's data folder, before
or after the word `memory`; without it the `LEON_DATA_DIR` of a Leon terminal
is used, else the platform's folder.

The commands open Leon's database beside the running application (SQLite in
WAL mode); nothing has to be running. A database written by a newer Leon than
the `leon` that was called is refused with a message that says so: call the
one that is running, `"$LEON_BIN"`.

## The memory file

Every write regenerates one markdown file per project in `<data
dir>/memory/`, named after the root's folder and a short hash of its path
(`api-3f2a9c01d4e7.md`), and `global.md`. A project's file holds the global
entries first and then the project's.

The file is a **summary**, not a dump of the memory: what an agent needs to
know exists, at a cost it can pay in every session.

* A **pinned** entry is there in full, under `### Pinned`, first in its
  scope's section, as long as all the pinned text stays within half of the
  file (see below); a pinned entry that does not fit in that half is one line
  like any other entry, marked with `…` when its text is longer than the
  line. A pinned entry in full says nothing twice: when its title is only the
  start of its text, the bullet is the bracket and the text follows; a title
  of its own is shown before the bracket.
* **Every other entry is one line**, grouped by kind, the last changed first:
  its text when that fits in 160 characters, else the first line of its text
  cut at a word, with `…` at the end to say there is more. An entry with a
  title of its own shows `title: text`; when the title is just the start of
  the text, the words are not said twice.
* After each entry, in brackets: its id, its topic, its revision when it was
  revised, its day and its author.
* The file's first lines say that a line ending in `…` is read whole with
  `"$LEON_BIN" memory show <id>` and that more is found with `memory search`.

```markdown
## This project: /home/me/code/api

### Pinned

- [0c93b7e2, pinned convention, 2026-10-07, claude]
  Never push to main: open a pull request, the release train cuts from it.
  Hotfixes too.
- Release checklist [77a0c3d1, pinned note, 2026-10-05, claude]
  Tag, wait for CI, then publish the notes.
- The on-call runbook for the payments queue starts with checking the dead letter count, then the consumer lag, and only then… [4be2f9a6, pinned discovery, 2026-10-02, codex]

### Decisions

- Money is integer cents: floats rounded wrong in the invoice total. [5e8c1a07, topic money/representation, revision 2, 2026-10-07, claude]
- The importer streams the CSV in chunks of 5,000 rows because the whole file does not fit in the worker's memory on the small plan, and the… [91c2aa0e, 2026-10-06, codex]

4 more entries are not shown (1 global, 3 of the project). Find them with `"$LEON_BIN" memory search <words>`.
```

The file is at most 12,000 bytes (roughly 3,000 tokens), or what the setting
**Memory file size** (`memory_budget`, 2,000 to 200,000) says. When not
everything fits:

1. Every pinned entry gets its one line first, the project's before the
   global ones: a pin always appears. There is no limit on how many entries
   are pinned.
2. Pinned entries then get their whole text, in the same order, while
   everything pinned (lines and texts) stays within **half** of the room
   there is for entries. One that does not fit stays a line; a shorter one
   after it may still get its text. Without this bound one long pin, or a
   handful of middling ones, left room for almost nothing else.
3. The other half goes to the other entries, a line each, the newest first:
   a third to the global ones at first, then the project's, then what the
   project did not use to the global ones again.
4. What the other entries did not use goes back to the pins that are still a
   line, so a memory with few entries shows its pins whole.
5. A last line says how many entries are not shown and how to find them. A
   pin on one line is shown: it is not counted there.

Forgotten entries are not in the file.

The setting is read from `settings.json` in the data folder by the
application, by `leon memory` and by `leon memory mcp` alike (a value that
cannot be used is the default, one out of range the nearest bound), so the
same memory gives the same file whoever writes it. A change of the setting in
the running application writes every memory file again at once; a change made
by hand in the file while Leon is not running shows at the next write.

**Entries are notes, not instructions**, and the file says so. What an entry
holds was written by an agent or pasted by a person, so it is treated as
untrusted text when it is rendered: it can never be a heading, a code fence, a
rule or an HTML comment of the file (so it cannot forge the block's markers or
a section), control characters are dropped, and the text is indented under its
bullet. That keeps an entry from posing as the file's structure; it does not
make a false note true. Read `leon memory list` now and then, and `forget`
what is wrong.

The files are written whole (a temporary file renamed over the old one) and
never read back as the source of anything: the database is the memory.

## Telling the agents: `leon memory enable`

An agent uses the memory only when something tells it the memory exists.
`leon memory enable`, run in a project, writes a short **managed block** into
the project's instruction files:

```markdown
<!-- leon:memory:begin (managed by Leon; `leon memory disable` removes it) -->
## Shared memory

If `$LEON_MEMORY` is set, read that file before starting work: it holds notes from earlier sessions of every agent in this project.
- Search it: `"$LEON_BIN" memory search <words>`
- Save a durable decision, convention or discovery, one fact per entry: `"$LEON_BIN" memory add --kind <decision|convention|discovery|preference|note> "<the fact>"` (add `--topic <key>` to a fact that may change: the same key revises it)
- Outside Leon both variables are unset: ignore this section.
<!-- leon:memory:end -->
```

The block names no path, so it is the same on every computer and is safe to
commit: a colleague without Leon has neither variable set and their agent is
told to ignore it.

Which files:

* Every one of `AGENTS.md`, `CLAUDE.md` and `GEMINI.md` that exists gets the
  block, at its end. When none exists, `AGENTS.md` is created and the others
  are not; the block's first line then says the file was created by Leon.
* The files are the ones at the project's root. Run from a linked git
  worktree that lives outside the root, it is that worktree's own copies:
  the worktree shares the project's memory, but an agent started there reads
  the worktree's files. (The palette's command writes at the project's root;
  the other worktrees get the block through git, once it is committed.)
* When one of them is a symbolic link to another (`CLAUDE.md` to `AGENTS.md`,
  or the reverse), the block is written once, through the real file.
* A file that pulls another in with a line `@AGENTS.md` is left alone: the
  agent reads the block through the file it pulls in.
* A link that leads anywhere else is never followed, and nothing is written
  outside the project's root.

Everything else in a file stays byte for byte: its line endings (the block
uses `\r\n` in a file that does) and its final newline or lack of one. Running
`enable` again changes nothing, or brings an older block up to date in place
(the block of the first build, which did not mention `--topic`, is replaced
this way).
`leon memory disable` removes the block and the empty line before it, leaving
the file exactly as it was, also a file that was empty. The file Leon created
is removed, unless something else was written into it since.

In the application, the palette's **Turn on agent memory for this project** and
**Turn off agent memory for this project** do the same for a project of this
computer (without the MCP entry).

## Being asked

So that the memory is not something only people who know the palette command
find, Leon asks. Right after you add a project **on this computer by hand**
(Open project…, Clone project…, New project…), a small question comes up:

```text
[ AGENT MEMORY ]
Turn on agent memory for api?

What one agent learns, the next one knows: Claude Code, Codex, opencode and the rest share it.
It stays on this computer, in Leon's data folder.
Agents save decisions, conventions and discoveries, and find them again by search.

Leon adds a short section to AGENTS.md and CLAUDE.md in /home/me/code/api. Turn off agent memory for this project removes it again, byte for byte.

TURN IT ON  Enter    NOT NOW  Esc    NEVER FOR THIS PROJECT  N    DO NOT ASK AGAIN  D
```

The line about what changes is this project's truth: the instruction files
that are there and would get the block, or "Leon creates AGENTS.md in …" when
there is none. It is worked out with the same rule `leon memory enable` uses.

| Answer | Key | What happens |
| --- | --- | --- |
| Turn it on | `Enter` | The same as the palette's **Turn on agent memory for this project**; the status line names the files that were written. |
| Not now | `Esc` | Nothing. The project is asked about again the next time it is added, but not twice while Leon runs. |
| Never for this project | `N` | Nothing is written, and Leon does not ask about this project again, also after it is removed and added back. |
| Do not ask again | `D` | Turns the setting **Offer agent memory when a project is added** (`memory_offer`) off: no project is asked about any more. |

Leon does not ask:

* about a project it found by itself (in the agents' history, or on a paired
  computer): only a project somebody added is asked about, so discoveries
  never raise a queue of questions;
* about a project on another machine: the memory is this computer's;
* when the project's instruction files already hold the block (a repository
  whose team committed it): the status line says **Agent memory is already on
  for …** and nothing is written;
* about a project that was answered before, or whose memory was turned on or
  off by hand, from the palette or with `leon memory enable` / `disable`.

The answer is kept in Leon's database by the project's root, like the memory
itself; `leon memory status` prints it. With the setting off the palette's
commands and the command line work as before; in the palette's list of
projects, the ones whose memory is on say so.

## MCP

`leon memory mcp` is an MCP server over stdio (newline-delimited JSON-RPC 2.0;
protocol versions `2025-06-18`, `2025-03-26` and `2024-11-05`). Its tools:

| Tool | Arguments |
| --- | --- |
| `memory_search` | `query`, `scope` (`project`, the default, searches the project and the global memory; `global`), `limit` |
| `memory_get` | `id`: one entry in full, as `show` prints it |
| `memory_add` | `text`, `kind`, `title`, `topic`, `scope`; answers `Saved`, `Revised` or `Already known as` |
| `memory_update` | `id`, and any of `text`, `kind`, `title`, `topic` (an empty topic takes it away) |
| `memory_pin` | `id`, `pinned` (true or false) |
| `memory_list` | `scope`, `limit` |
| `memory_forget` | `id`: soft, as on the command line |
| `memory_context` | none: the memory file (a summary; `memory_get` reads an entry whole) |

The server's project is the one of the folder its client names
(`CLAUDE_PROJECT_DIR`, which Claude Code sets for the servers it starts), else
of the folder it is started in. The client's name (`clientInfo.name`)
is recorded as the author of what it adds (`claude-code` as `claude`).

The block and the server are two ways to the same memory; one is enough. The
block costs a few lines of context in every session and works with any agent
that can run a command; the server gives agents that speak MCP proper tools.

### Claude Code

`leon memory enable --mcp` also merges this entry into the project's
`.mcp.json`:

```json
{
  "mcpServers": {
    "leon-memory": {
      "command": "${LEON_BIN:-leon}",
      "args": ["memory", "mcp"]
    }
  }
}
```

Claude Code expands `${LEON_BIN:-leon}` from its environment, so the entry
names no path either: in a Leon terminal it is the running Leon, elsewhere a
`leon` on the `PATH` if there is one (if there is none, Claude Code reports
that the server did not start, and nothing else changes). Claude Code asks
once whether to trust a project's servers.

Every other key of the file stays, in its order, with the file's indentation,
line ending and final newline. A `.mcp.json` that is not valid JSON, or whose
`mcpServers` is not an object, is refused with a message and not touched.
`disable` removes the entry and leaves the rest of the file as it was, and
removes the file when it is exactly the one `enable --mcp` creates. JSON has
no place to remember two things, so they do not come back: a file that had no
`mcpServers` key keeps an empty one, and a file that held no server and no
other key is removed. A file that was not formatted the way a pretty-printer
formats it comes back pretty-printed.

### Codex and opencode

Leon does not edit their configuration; `enable --mcp` prints what to run or
paste, with the path of the Leon that printed it:

```sh
codex mcp add leon-memory -- /path/to/leon memory mcp
```

```toml
# ~/.codex/config.toml
[mcp_servers.leon-memory]
command = "/path/to/leon"
args = ["memory", "mcp"]
```

```json
{
  "mcp": {
    "leon-memory": {
      "type": "local",
      "command": ["/path/to/leon", "memory", "mcp"],
      "enabled": true
    }
  }
}
```

(the last in `opencode.json`, the project's or
`~/.config/opencode/opencode.json`).

## The environment of a Leon terminal

Leon sets these in every terminal it starts **on this computer**, before your
own `terminal_env` (so a variable of yours with the same name wins):

| Variable | Value |
| --- | --- |
| `LEON_BIN` | The absolute path of the running Leon. `leon` is not reliably on the `PATH`, so the block and the examples call `"$LEON_BIN"`. |
| `LEON_MEMORY` | The absolute path of the memory file of the project the terminal's folder belongs to. The file is written when the terminal starts, so it is there even for an empty memory. |
| `LEON_DATA_DIR` | Leon's data folder, set only when Leon runs with a `--data-dir` that is not the platform's, so that `"$LEON_BIN" memory` reaches the same database. |

They are set whether or not a project has the memory turned on: they change
nothing by themselves.

With **Keep local sessions running** (`durable_sessions`) a session outlives
the window, and Leon attaches to it again at the next start instead of
starting it. Such a session keeps the environment it was started with: its
`LEON_MEMORY` still names the right file (the data folder and the project's
root have not moved), but its `LEON_BIN` is the Leon that started it. After an
update that puts Leon somewhere else, a session kept from before still calls
the old path until it is closed and opened again.

## Where the data is

| What | Where |
| --- | --- |
| The entries | The `memory` table of `<data dir>/leon.db` (with a full-text index, `memory_fts`), the forgotten ones included until they are purged |
| The memory files | `<data dir>/memory/global.md` and `<data dir>/memory/<folder>-<hash>.md` |
| The block | The project's `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, only after `enable` |
| The MCP entry | The project's `.mcp.json`, only after `enable --mcp` |
| The answer to "turn it on?" | The `memory_choice` table of `<data dir>/leon.db`, by the project's root |

`<data dir>` is `~/.local/share/leon` on Linux, `~/Library/Application
Support/leon` on macOS and `%APPDATA%\leon` on Windows, or `--data-dir`.
Nothing leaves the computer.

## Limits

* **This computer only.** A terminal on a machine reached over SSH or through
  a relay is told nothing: no variable is set there, and the memory of this
  computer is not reachable from it. The palette's commands refuse a project
  on another machine.
* **Nothing is captured automatically.** The memory holds what an agent or a
  person saved with `add` (or `memory_add`); Leon does not distil transcripts.
  An agent saves when its instructions tell it to, which is what the block is
  for.
* **No view in the application yet.** The memory is read and curated from the
  command line (`list`, `search`, `forget`); the window has no screen for it.
* **No judging of conflicts.** Two entries that contradict each other in
  different words are two entries; only the same text, or the same topic, is
  recognised. The topic is not searched, and there is no search by meaning.
* **Restoring and purging are the command line's**: the MCP server forgets
  (softly) but does not restore, remove for good or purge.
* **Keyed by path.** A project that is moved or renamed starts with an empty
  memory; its old entries stay in the database under the old path.
* **Not a place for secrets.** Entries are plain text in a local database and
  are shown to every agent that works in the project.

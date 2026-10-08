# Project scripts and worktree setup

A repository can describe, in a file Leon reads, the commands its people run and
the command that makes a new worktree usable. The file is `leon.toml` at the
root of the project (the folder Leon lists as the project, not a worktree). It is
checked into the repository, written in TOML like the themes, and read through
the machine's own runner, so it works for a project on this computer, on an SSH
machine and on a machine reached through a relay.

```toml
[worktree]
setup = "npm install && cp .env.example .env"

[[script]]
name = "Test"
command = "npm test"
icon = "terminal"
key = "mod+shift+u"

[[script]]
name = "Lint"
command = "npm run lint"
```

## The format

| Key | Where | Meaning |
| --- | --- | --- |
| `name` | `[[script]]`, required | What the palette lists. One line, at most 48 characters, unique in the file (case ignored). |
| `command` | `[[script]]`, required | The command line typed into a new terminal. |
| `icon` | `[[script]]`, optional | One of `terminal`, `bot`, `git-branch`, `folder`, `file`, `hash`, `check`, `refresh`, `server`, `settings`, `search`, `trash`, `plus`, `command`, `pin`, `paw`, `laptop`, `house`. |
| `key` | `[[script]]`, optional | A shortcut, below. |
| `setup` | `[worktree]`, optional | The command run once in every new worktree. |

At most 40 scripts. A field Leon does not know is an error, so a typo does not
silently do nothing.

**A command is one line.** It is typed into a terminal, so it may not contain a
line break or any control character (an escape sequence or a carriage return
typed into a terminal is not what the prompt that shows the command would show),
and it is at most 2000 characters. Join several commands with `&&` or `;`, or put
them in a script file of the repository and run that.

**Errors name the line.** `leon.toml line 7: ...` appears on the status line when
the file is read and cannot be used. While it is wrong nothing in it runs; fix
it and switch to the project again, or use **Run a script...**, which reads the
file again. When a worktree is made while the file is wrong (**New worktree** or
**One prompt, several agents...**), the session still starts, without the setup,
and the status line says `No setup was run: leon.toml line 7: ...` (in the same
line as the count of agents, for several). If the file cannot be read at all, the
line says that instead.

## Running a script

**Run a script...** (palette, File menu) reads the file and lists the scripts of
the project in view. The chosen one is typed into a new terminal tab of the
worktree in view (a tab of the terminal on screen when that is in the same
worktree, else a session of its own in the worktree). The command is typed, as
written, into the shell of that terminal, so that shell's aliases and functions
work, and so does its own syntax: a `!` is history expansion in bash or zsh, and
PowerShell reads what it reads. The terminal stays open when the command ends.

### Keys

A `key` is `mod`, `shift`, optionally `alt`, and one key from `a` to `z`, `0` to
`9` or `f1` to `f12`, joined with `+`, in any order and case: `mod+shift+u`,
`shift+mod+alt+f5`. `mod` is Cmd on macOS and Ctrl on Linux and Windows (`cmd`
and `ctrl` are accepted for it). `shift` is always required, because off macOS it
is what keeps a chord Leon's while a terminal has the keyboard; the same file
therefore means the same thing everywhere.

A script run by its key uses the file as it was when the project came into view;
**Run a script...** and a new worktree read it again first.

A key is Leon's before it is a script's. If one of Leon's commands already has
the chord (`mod+shift+t` opens a shell, for one), the script is not bound: the
status line says which command has it, the palette shows the reason beside the
script, and the script still runs from the palette. A script never takes a
chord from the registry. Two scripts with the same key are an error, on the line
of the second.

## Worktree setup

After `git worktree add` and the listing that makes the worktree appear, the
session that starts in it (the agent asked for, else a shell) starts behind the
setup command. In a POSIX shell the terminal is typed one line, with an agent
after it:

```text
sh -c '<setup>' && <the agent's command line>
```

and `sh -c '<setup>'` alone for a shell session, so the setup means the same
whatever the login shell is (fish included) and a `!` in it is not history
expansion. The agent starts only when the setup exits with status 0. When it
fails the terminal stays open at its prompt with the output on the screen and the
agent is not started. In PowerShell and `cmd.exe` there is no `sh`: the setup is
typed as written, and with an agent the line is `<setup>; if ($?) { <agent> }` in
PowerShell and `<setup> && <agent>` in `cmd.exe`.

Limits, stated plainly:

* What the setup exports or `cd`s into does not reach the agent. Put environment
  the agent needs in the agent's own settings.
* There is no progress and no timeout: the terminal is the progress, and closing
  it ends the setup.
* The file is read from the project's folder, not from the new worktree, and
  again when the worktree appears.
* A file that is wrong or cannot be read gives no setup; the session starts and
  the status line says why (see "Errors name the line").

## Trust

The file is written by whoever can push to the repository, so Leon never runs one
of its commands without your say. The first time a project's setup command or
script would run, and again whenever its text changes by even one character,
Leon shows a card with the exact command, where it comes from, where it would run
and how it is typed (a script as written into the terminal's own shell, a setup
under `sh -c`).

The card can appear on its own: a new worktree is made, or several agents are
started, and a few seconds later the setup needs your answer while you are typing
somewhere else. So **no plain key answers it**:

| Answer | How |
| --- | --- |
| Run it once | the `RUN ONCE` button, or `Cmd+Enter` (macOS) / `Ctrl+Enter` |
| Run it and remember it | the `RUN AND TRUST` button, or `Cmd+Shift+Enter` / `Ctrl+Shift+Enter` |
| Do not run it | the `CANCEL` button, or `Esc`. A script says so on the status line; a new worktree still gets its session, without the setup. |

A plain `Enter`, or a letter, does nothing on the card. Only one card is shown at
a time; one that comes while a palette, a menu or another card is open waits its
turn, and a card for a command you have remembered in the meantime is not shown
at all.

What is remembered is the project and the SHA-256 of the command, in
`trusted.json` next to `settings.json`. The commands themselves are not stored. A
command trusted in one project is not trusted in another. There is no screen to
take an answer back: to forget what Leon remembers, delete `trusted.json` (or the
entry in it) while Leon is closed.

## Where new worktrees go

The setting `worktree_location` (Settings, Projects) is a template:

* `{root}` is the project's folder, without a trailing separator.
* `{branch}` is the branch name with its slashes turned into dashes.

The default, `{root}-worktrees/{branch}`, puts a worktree next to the project
exactly where Leon has always put it (`/srv/api` and `feature/login` give
`/srv/api-worktrees/feature-login`). `{root}/.worktrees/{branch}` keeps them
inside the project, `/work/trees/{branch}` puts them all in one place. A slash in
the template follows the project's own separator, so a Windows project stays a
Windows path. The template must name `{branch}` and give an absolute path; one
that does not is refused when a worktree is made, with the reason. An empty
value is the default.

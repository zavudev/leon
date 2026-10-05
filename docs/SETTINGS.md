# Settings

<!-- Generated from crates/app/src/schema.rs: do not edit by hand. -->

Open the Settings screen with `Cmd+,` (`Ctrl+,` on Linux and Windows), the gear in the
sidebar, the application menu or the command palette. Every option applies at once.
The choices are kept in `settings.json` in the data folder (`--data-dir`, or the
platform's); the palette's **Open settings.json** and **Reveal settings folder**
commands show it. The file may be edited by hand while Leon runs: a change is picked up
within a second or two. A value that cannot be used falls back to its default for that
key only and the status line says so; keys Leon does not know are kept when it saves.
A setting equal to its default is not written at all.

## Appearance

How Leon looks.

| Setting | Key | Values | Default | Description |
| --- | --- | --- | --- | --- |
| Theme | `theme_id` | a theme id | `leon` | The brand look: colours, fonts and shape. Built-in and your own theme files. |
| Appearance | `theme` | `system`, `light`, `dark` | `dark` | Light, dark, or whatever the desktop is set to. |
| Interface size | `interface_scale` | 80 to 125% | `100` | The size of the whole interface, in percent of the design: 80, 90, 100, 110 or 125. |
| Blueprint lines | `blueprint_lines` | `theme`, `on`, `off` | `theme` | The crosshairs, corner ticks and frames of the line system: the theme's own, always, or never. |
| Animate the lion | `animate_lion` | on, off | `on` | The Leon mark blinks, narrows its eyes and glances now and then, and shows how the sessions are doing. Off keeps it still. |
| Reduce motion | `reduce_motion` | `system`, `on`, `off` | `system` | Keep the interface still: follow the system's reduce-motion preference, always reduce motion, or never. Reduced motion shows the lion at rest. |

## Terminal

How terminals are drawn and started.

| Setting | Key | Values | Default | Description |
| --- | --- | --- | --- | --- |
| Font family | `terminal_font_family` | text | empty | A monospaced family, bundled or installed. Empty is the theme's own. |
| Font size | `terminal_font_size` | 8 to 32px | `13` | The size of terminal text in pixels at the 100% interface size. |
| Line height | `terminal_line_height` | 100 to 200% | `135` | The height of a terminal row as a percentage of the font size. |
| Scrollback lines | `terminal_scrollback` | 0 to 100000 | `10000` | How many lines of history each terminal keeps. Zero keeps none. |
| Cursor shape | `terminal_cursor` | `block`, `beam`, `underline` | `block` | The cursor a program gets until it asks for another. |
| Copy on select | `terminal_copy_on_select` | on, off | `off` | Copy the selection to the clipboard as soon as the mouse is released. |
| Paste with an image on the clipboard | `terminal_paste` | `auto`, `text`, `image` | `auto` | What paste does when the clipboard holds text and an image: decide by the program in front, always paste the text, or send the image. |
| Option as Meta | `terminal_option_as_meta` | on, off | `on` | Option sends Escape before the key, as shells and editors expect. Off types the composed character (å, é). (macOS only.) |
| Confirm before closing a running pane | `terminal_confirm_close` | on, off | `on` | Ask before closing a pane that has a program running in it. |
| Shell | `terminal_shell` | a path | empty | The program new terminals on this computer run. Empty is your login shell. |
| Shell arguments | `terminal_shell_args` | text | empty | Arguments of a custom shell, separated by spaces. Ignored with the login shell. |
| Extra environment variables | `terminal_env` | a list of text | empty | NAME=value lines added to the environment of terminals on this computer. |
| Mark a session that rings the bell | `terminal_bell_mark` | on, off | `on` | Show the bell mark on a background session whose program rang the terminal bell. |

## Agents

How each coding agent is started and resumed.

| Setting | Key | Values | Default | Description |
| --- | --- | --- | --- | --- |
| Agent for a new session | `default_agent` | `ask`, `claude`, `codex`, `opencode` | `ask` | The agent New agent session starts without asking, or ask each time. |
| Opening a history session | `history_open` | `resume`, `transcript` | `resume` | What Enter or a click on a history session does: resume it in a terminal, or show its transcript. |
| Claude Code | `agent_claude_enabled` | on, off | `on` | Offer Claude Code for new sessions. |
| Claude Code executable | `agent_claude_executable` | a path | empty | The program that starts Claude Code. Empty lets the login shell find `claude`. |
| Claude Code arguments, new session | `agent_claude_args` | text | empty | Extra arguments typed after the command of a new Claude Code session. |
| Claude Code arguments, resume | `agent_claude_resume_args` | text | empty | Extra arguments typed after the command that resumes a Claude Code session. |
| Codex | `agent_codex_enabled` | on, off | `on` | Offer Codex for new sessions. |
| Codex executable | `agent_codex_executable` | a path | empty | The program that starts Codex. Empty lets the login shell find `codex`. |
| Codex arguments, new session | `agent_codex_args` | text | empty | Extra arguments typed after the command of a new Codex session. |
| Codex arguments, resume | `agent_codex_resume_args` | text | empty | Extra arguments typed after the command that resumes a Codex session. |
| opencode | `agent_opencode_enabled` | on, off | `on` | Offer opencode for new sessions. |
| opencode executable | `agent_opencode_executable` | a path | empty | The program that starts opencode. Empty lets the login shell find `opencode`. |
| opencode arguments, new session | `agent_opencode_args` | text | empty | Extra arguments typed after the command of a new opencode session. |
| opencode arguments, resume | `agent_opencode_resume_args` | text | empty | Extra arguments typed after the command that resumes an opencode session. |

## Sessions & history

What is imported and shown of the agents' history.

| Setting | Key | Values | Default | Description |
| --- | --- | --- | --- | --- |
| Import history on start | `import_on_start` | on, off | `on` | Read the agents' history and refresh projects when Leon starts. Refresh still works by hand. |
| Claude Code history folder | `history_dir_claude` | a path | empty | Where Claude Code keeps its projects. Empty is ~/.claude/projects. |
| Codex history folder | `history_dir_codex` | a path | empty | Where Codex keeps its sessions. Empty is ~/.codex/sessions. |
| opencode database | `history_dir_opencode` | a path | empty | The opencode database file. Empty is its own location. |
| Sessions per worktree | `sessions_per_worktree` | 3 to 50 | `8` | How many history sessions a worktree lists before "show more". |
| Detect sessions running elsewhere | `detect_elsewhere` | on, off | `on` | Look for agent processes Leon did not start and mark the sessions they hold. |
| Detection interval | `elsewhere_interval` | 2 to 120s | `5` | How often, in seconds, this computer is looked at while the window is focused. |
| Re-import now |  | button |  | Read the agents' history again and sync every project's worktrees. |

## Projects

How projects are found and what they show.

| Setting | Key | Values | Default | Description |
| --- | --- | --- | --- | --- |
| Discover projects from session folders | `discover_projects` | on, off | `on` | Add the repositories your sessions ran in as projects. |
| Detect project logos | `detect_logos` | on, off | `on` | Look in the repository for an icon to show beside the project. |
| Fetch owner avatars from the Git host | `fetch_avatars` | on, off | `on` | When a repository has no icon, download its owner's avatar from the Git host (GitHub). This is the only network call Leon makes. |

## Machines

SSH machines and how Leon talks to them.

| Setting | Key | Values | Default | Description |
| --- | --- | --- | --- | --- |
| Share SSH connections | `ssh_multiplex` | on, off | `on` | Reuse one connection per machine for every command instead of a handshake each time. (macOS and Linux only.) |
| Keep a shared connection open | `ssh_persist_minutes` | 1 to 240min | `10` | How long an idle shared SSH connection stays open, in minutes. (macOS and Linux only.) |
| Connect timeout | `ssh_connect_timeout` | 0 to 120s | `0` | How long ssh waits to connect, in seconds. Zero leaves it to ssh. |
| Connect a machine… |  | button |  | Open the Connect screen: what a machine is, a form and a test of the connection. |
| Probe the machine on screen |  | button |  | Check that the machine answers and which agents it has. |

## Sidebar & window

The sidebar and what quitting asks.

| Setting | Key | Values | Default | Description |
| --- | --- | --- | --- | --- |
| Show the sidebar | `sidebar_visible` | on, off | `on` | The tree of machines, projects, worktrees and sessions. |
| Sidebar width | `sidebar_width` | 220 to 560px | `320` | The sidebar's width in pixels at the 100% interface size. |
| Confirm before quitting | `quit_confirmation` | `running`, `always`, `never` | `running` | Ask before quitting: only while programs run in terminals, always, or never. |

## Keyboard

Every command and its shortcuts.

Read-only: every command with its chords, generated from the shortcut registry.
Shortcuts are not customisable yet.

## Advanced & About

Folders, logging and starting over.

| Setting | Key | Values | Default | Description |
| --- | --- | --- | --- | --- |
| Log level | `log_level` | `error`, `warn`, `info`, `debug`, `trace` | `info` | How much Leon writes to its log (standard error). The RUST_LOG variable wins when set. |
| About Leon |  | button |  | Version 0.1.0, by Zavu: the About panel. |
| Data folder |  | button |  | Show the folder that holds the database and these settings. |
| Themes folder |  | button |  | Show the folder of your theme files. |
| Reset all settings |  | button |  | Put every setting back to its default. Theme files, projects and history are not touched. |

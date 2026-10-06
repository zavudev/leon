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
| Agent for a new session | `default_agent` | `ask`, `claude`, `codex`, `opencode`, `grok`, `cursor`, `copilot`, `muse`, `dsh`, `zcode`, `mimo-code`, `amp`, `openclaude`, `antigravity`, `pi`, `omp`, `hermes`, `devin`, `goose`, `auggie`, `autohand`, `crush`, `cline`, `codebuddy`, `codebuff`, `freebuff`, `command-code`, `continue`, `droid`, `kilo`, `kimi`, `kiro`, `mistral-vibe`, `qwen-code`, `rovo`, `gemini`, `aider`, `ante`, `trae`, `qoder`, `qoder-cn`, `prime-agent`, `openclaw`, `jcode` | `ask` | The agent New agent session starts without asking, or ask each time. |
| Opening a history session | `history_open` | `resume`, `transcript` | `resume` | What Enter or a click on a history session does: resume it in a terminal, or show its transcript. |
| Your own agents | `custom_agents` | a list of text | empty | Any command line tool, added with Add a custom agent: a name, the command, its arguments and how to resume a session. |
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
| opencode arguments, resume | `agent_opencode_resume_args` | text | empty | Extra arguments typed after the command that resumes a opencode session. |
| Grok | `agent_grok_enabled` | on, off | `on` | Offer Grok for new sessions. |
| Grok executable | `agent_grok_executable` | a path | empty | The program that starts Grok. Empty lets the login shell find `grok`. |
| Grok arguments, new session | `agent_grok_args` | text | empty | Extra arguments typed after the command of a new Grok session. |
| Grok arguments, resume | `agent_grok_resume_args` | text | empty | Extra arguments typed after the command that resumes a Grok session. |
| Cursor | `agent_cursor_enabled` | on, off | `on` | Offer Cursor for new sessions. |
| Cursor executable | `agent_cursor_executable` | a path | empty | The program that starts Cursor. Empty lets the login shell find `cursor-agent`. |
| Cursor arguments, new session | `agent_cursor_args` | text | empty | Extra arguments typed after the command of a new Cursor session. |
| Cursor arguments, resume | `agent_cursor_resume_args` | text | empty | Extra arguments typed after the command that resumes a Cursor session. |
| GitHub Copilot | `agent_copilot_enabled` | on, off | `on` | Offer GitHub Copilot for new sessions. |
| GitHub Copilot executable | `agent_copilot_executable` | a path | empty | The program that starts GitHub Copilot. Empty lets the login shell find `copilot`. |
| GitHub Copilot arguments, new session | `agent_copilot_args` | text | empty | Extra arguments typed after the command of a new GitHub Copilot session. |
| GitHub Copilot arguments, resume | `agent_copilot_resume_args` | text | empty | Extra arguments typed after the command that resumes a GitHub Copilot session. |
| Muse | `agent_muse_enabled` | on, off | `on` | Offer Muse for new sessions. |
| Muse executable | `agent_muse_executable` | a path | empty | The program that starts Muse. Empty lets the login shell find `muse`. |
| Muse arguments, new session | `agent_muse_args` | text | empty | Extra arguments typed after the command of a new Muse session. |
| Muse arguments, resume | `agent_muse_resume_args` | text | empty | Extra arguments typed after the command that resumes a Muse session. |
| DeepSeek Harness | `agent_dsh_enabled` | on, off | `on` | Offer DeepSeek Harness for new sessions. |
| DeepSeek Harness executable | `agent_dsh_executable` | a path | empty | The program that starts DeepSeek Harness. Empty lets the login shell find `dsh-tui`. |
| DeepSeek Harness arguments, new session | `agent_dsh_args` | text | empty | Extra arguments typed after the command of a new DeepSeek Harness session. |
| DeepSeek Harness arguments, resume | `agent_dsh_resume_args` | text | empty | Extra arguments typed after the command that resumes a DeepSeek Harness session. |
| ZCode | `agent_zcode_enabled` | on, off | `on` | Offer ZCode for new sessions. |
| ZCode executable | `agent_zcode_executable` | a path | empty | The program that starts ZCode. Empty lets the login shell find `zcode`. |
| ZCode arguments, new session | `agent_zcode_args` | text | empty | Extra arguments typed after the command of a new ZCode session. |
| ZCode arguments, resume | `agent_zcode_resume_args` | text | empty | Extra arguments typed after the command that resumes a ZCode session. |
| MiMo Code | `agent_mimo_code_enabled` | on, off | `on` | Offer MiMo Code for new sessions. |
| MiMo Code executable | `agent_mimo_code_executable` | a path | empty | The program that starts MiMo Code. Empty lets the login shell find `mimo`. |
| MiMo Code arguments, new session | `agent_mimo_code_args` | text | empty | Extra arguments typed after the command of a new MiMo Code session. |
| MiMo Code arguments, resume | `agent_mimo_code_resume_args` | text | empty | Extra arguments typed after the command that resumes a MiMo Code session. |
| Amp | `agent_amp_enabled` | on, off | `on` | Offer Amp for new sessions. |
| Amp executable | `agent_amp_executable` | a path | empty | The program that starts Amp. Empty lets the login shell find `amp`. |
| Amp arguments, new session | `agent_amp_args` | text | empty | Extra arguments typed after the command of a new Amp session. |
| OpenClaude | `agent_openclaude_enabled` | on, off | `on` | Offer OpenClaude for new sessions. |
| OpenClaude executable | `agent_openclaude_executable` | a path | empty | The program that starts OpenClaude. Empty lets the login shell find `openclaude`. |
| OpenClaude arguments, new session | `agent_openclaude_args` | text | empty | Extra arguments typed after the command of a new OpenClaude session. |
| Antigravity | `agent_antigravity_enabled` | on, off | `on` | Offer Antigravity for new sessions. |
| Antigravity executable | `agent_antigravity_executable` | a path | empty | The program that starts Antigravity. Empty lets the login shell find `agy`. |
| Antigravity arguments, new session | `agent_antigravity_args` | text | empty | Extra arguments typed after the command of a new Antigravity session. |
| Antigravity arguments, resume | `agent_antigravity_resume_args` | text | empty | Extra arguments typed after the command that resumes a Antigravity session. |
| Pi | `agent_pi_enabled` | on, off | `on` | Offer Pi for new sessions. |
| Pi executable | `agent_pi_executable` | a path | empty | The program that starts Pi. Empty lets the login shell find `pi`. |
| Pi arguments, new session | `agent_pi_args` | text | empty | Extra arguments typed after the command of a new Pi session. |
| oh-my-pi | `agent_omp_enabled` | on, off | `on` | Offer oh-my-pi for new sessions. |
| oh-my-pi executable | `agent_omp_executable` | a path | empty | The program that starts oh-my-pi. Empty lets the login shell find `omp`. |
| oh-my-pi arguments, new session | `agent_omp_args` | text | empty | Extra arguments typed after the command of a new oh-my-pi session. |
| Hermes Agent | `agent_hermes_enabled` | on, off | `on` | Offer Hermes Agent for new sessions. |
| Hermes Agent executable | `agent_hermes_executable` | a path | empty | The program that starts Hermes Agent. Empty lets the login shell find `hermes`. |
| Hermes Agent arguments, new session | `agent_hermes_args` | text | empty | Extra arguments typed after the command of a new Hermes Agent session. |
| Devin | `agent_devin_enabled` | on, off | `on` | Offer Devin for new sessions. |
| Devin executable | `agent_devin_executable` | a path | empty | The program that starts Devin. Empty lets the login shell find `devin`. |
| Devin arguments, new session | `agent_devin_args` | text | empty | Extra arguments typed after the command of a new Devin session. |
| Devin arguments, resume | `agent_devin_resume_args` | text | empty | Extra arguments typed after the command that resumes a Devin session. |
| Goose | `agent_goose_enabled` | on, off | `on` | Offer Goose for new sessions. |
| Goose executable | `agent_goose_executable` | a path | empty | The program that starts Goose. Empty lets the login shell find `goose`. |
| Goose arguments, new session | `agent_goose_args` | text | empty | Extra arguments typed after the command of a new Goose session. |
| Auggie | `agent_auggie_enabled` | on, off | `on` | Offer Auggie for new sessions. |
| Auggie executable | `agent_auggie_executable` | a path | empty | The program that starts Auggie. Empty lets the login shell find `auggie`. |
| Auggie arguments, new session | `agent_auggie_args` | text | empty | Extra arguments typed after the command of a new Auggie session. |
| Autohand Code | `agent_autohand_enabled` | on, off | `on` | Offer Autohand Code for new sessions. |
| Autohand Code executable | `agent_autohand_executable` | a path | empty | The program that starts Autohand Code. Empty lets the login shell find `autohand`. |
| Autohand Code arguments, new session | `agent_autohand_args` | text | empty | Extra arguments typed after the command of a new Autohand Code session. |
| Charm Crush | `agent_crush_enabled` | on, off | `on` | Offer Charm Crush for new sessions. |
| Charm Crush executable | `agent_crush_executable` | a path | empty | The program that starts Charm Crush. Empty lets the login shell find `crush`. |
| Charm Crush arguments, new session | `agent_crush_args` | text | empty | Extra arguments typed after the command of a new Charm Crush session. |
| Cline | `agent_cline_enabled` | on, off | `on` | Offer Cline for new sessions. |
| Cline executable | `agent_cline_executable` | a path | empty | The program that starts Cline. Empty lets the login shell find `cline`. |
| Cline arguments, new session | `agent_cline_args` | text | empty | Extra arguments typed after the command of a new Cline session. |
| CodeBuddy | `agent_codebuddy_enabled` | on, off | `on` | Offer CodeBuddy for new sessions. |
| CodeBuddy executable | `agent_codebuddy_executable` | a path | empty | The program that starts CodeBuddy. Empty lets the login shell find `codebuddy`. |
| CodeBuddy arguments, new session | `agent_codebuddy_args` | text | empty | Extra arguments typed after the command of a new CodeBuddy session. |
| CodeBuddy arguments, resume | `agent_codebuddy_resume_args` | text | empty | Extra arguments typed after the command that resumes a CodeBuddy session. |
| Codebuff | `agent_codebuff_enabled` | on, off | `on` | Offer Codebuff for new sessions. |
| Codebuff executable | `agent_codebuff_executable` | a path | empty | The program that starts Codebuff. Empty lets the login shell find `codebuff`. |
| Codebuff arguments, new session | `agent_codebuff_args` | text | empty | Extra arguments typed after the command of a new Codebuff session. |
| Freebuff | `agent_freebuff_enabled` | on, off | `on` | Offer Freebuff for new sessions. |
| Freebuff executable | `agent_freebuff_executable` | a path | empty | The program that starts Freebuff. Empty lets the login shell find `freebuff`. |
| Freebuff arguments, new session | `agent_freebuff_args` | text | empty | Extra arguments typed after the command of a new Freebuff session. |
| Command Code | `agent_command_code_enabled` | on, off | `on` | Offer Command Code for new sessions. |
| Command Code executable | `agent_command_code_executable` | a path | empty | The program that starts Command Code. Empty lets the login shell find `command-code`. |
| Command Code arguments, new session | `agent_command_code_args` | text | empty | Extra arguments typed after the command of a new Command Code session. |
| Continue | `agent_continue_enabled` | on, off | `on` | Offer Continue for new sessions. |
| Continue executable | `agent_continue_executable` | a path | empty | The program that starts Continue. Empty lets the login shell find `cn`. |
| Continue arguments, new session | `agent_continue_args` | text | empty | Extra arguments typed after the command of a new Continue session. |
| Droid | `agent_droid_enabled` | on, off | `on` | Offer Droid for new sessions. |
| Droid executable | `agent_droid_executable` | a path | empty | The program that starts Droid. Empty lets the login shell find `droid`. |
| Droid arguments, new session | `agent_droid_args` | text | empty | Extra arguments typed after the command of a new Droid session. |
| Droid arguments, resume | `agent_droid_resume_args` | text | empty | Extra arguments typed after the command that resumes a Droid session. |
| Kilocode | `agent_kilo_enabled` | on, off | `on` | Offer Kilocode for new sessions. |
| Kilocode executable | `agent_kilo_executable` | a path | empty | The program that starts Kilocode. Empty lets the login shell find `kilo`. |
| Kilocode arguments, new session | `agent_kilo_args` | text | empty | Extra arguments typed after the command of a new Kilocode session. |
| Kimi | `agent_kimi_enabled` | on, off | `on` | Offer Kimi for new sessions. |
| Kimi executable | `agent_kimi_executable` | a path | empty | The program that starts Kimi. Empty lets the login shell find `kimi`. |
| Kimi arguments, new session | `agent_kimi_args` | text | empty | Extra arguments typed after the command of a new Kimi session. |
| Kimi arguments, resume | `agent_kimi_resume_args` | text | empty | Extra arguments typed after the command that resumes a Kimi session. |
| Kiro | `agent_kiro_enabled` | on, off | `on` | Offer Kiro for new sessions. |
| Kiro executable | `agent_kiro_executable` | a path | empty | The program that starts Kiro. Empty lets the login shell find `kiro-cli`. |
| Kiro arguments, new session | `agent_kiro_args` | text | empty | Extra arguments typed after the command of a new Kiro session. |
| Mistral Vibe | `agent_mistral_vibe_enabled` | on, off | `on` | Offer Mistral Vibe for new sessions. |
| Mistral Vibe executable | `agent_mistral_vibe_executable` | a path | empty | The program that starts Mistral Vibe. Empty lets the login shell find `vibe`. |
| Mistral Vibe arguments, new session | `agent_mistral_vibe_args` | text | empty | Extra arguments typed after the command of a new Mistral Vibe session. |
| Qwen Code | `agent_qwen_code_enabled` | on, off | `on` | Offer Qwen Code for new sessions. |
| Qwen Code executable | `agent_qwen_code_executable` | a path | empty | The program that starts Qwen Code. Empty lets the login shell find `qwen`. |
| Qwen Code arguments, new session | `agent_qwen_code_args` | text | empty | Extra arguments typed after the command of a new Qwen Code session. |
| Qwen Code arguments, resume | `agent_qwen_code_resume_args` | text | empty | Extra arguments typed after the command that resumes a Qwen Code session. |
| Rovo Dev | `agent_rovo_enabled` | on, off | `on` | Offer Rovo Dev for new sessions. |
| Rovo Dev executable | `agent_rovo_executable` | a path | empty | The program that starts Rovo Dev. Empty lets the login shell find `rovo`. |
| Rovo Dev arguments, new session | `agent_rovo_args` | text | empty | Extra arguments typed after the command of a new Rovo Dev session. |
| Gemini | `agent_gemini_enabled` | on, off | `on` | Offer Gemini for new sessions. |
| Gemini executable | `agent_gemini_executable` | a path | empty | The program that starts Gemini. Empty lets the login shell find `gemini`. |
| Gemini arguments, new session | `agent_gemini_args` | text | empty | Extra arguments typed after the command of a new Gemini session. |
| Gemini arguments, resume | `agent_gemini_resume_args` | text | empty | Extra arguments typed after the command that resumes a Gemini session. |
| Aider | `agent_aider_enabled` | on, off | `on` | Offer Aider for new sessions. |
| Aider executable | `agent_aider_executable` | a path | empty | The program that starts Aider. Empty lets the login shell find `aider`. |
| Aider arguments, new session | `agent_aider_args` | text | empty | Extra arguments typed after the command of a new Aider session. |
| Ante | `agent_ante_enabled` | on, off | `on` | Offer Ante for new sessions. |
| Ante executable | `agent_ante_executable` | a path | empty | The program that starts Ante. Empty lets the login shell find `ante`. |
| Ante arguments, new session | `agent_ante_args` | text | empty | Extra arguments typed after the command of a new Ante session. |
| Trae | `agent_trae_enabled` | on, off | `on` | Offer Trae for new sessions. |
| Trae executable | `agent_trae_executable` | a path | empty | The program that starts Trae. Empty lets the login shell find `traecli`. |
| Trae arguments, new session | `agent_trae_args` | text | empty | Extra arguments typed after the command of a new Trae session. |
| Qoder CLI | `agent_qoder_enabled` | on, off | `on` | Offer Qoder CLI for new sessions. |
| Qoder CLI executable | `agent_qoder_executable` | a path | empty | The program that starts Qoder CLI. Empty lets the login shell find `qodercli`. |
| Qoder CLI arguments, new session | `agent_qoder_args` | text | empty | Extra arguments typed after the command of a new Qoder CLI session. |
| Qoder CLI arguments, resume | `agent_qoder_resume_args` | text | empty | Extra arguments typed after the command that resumes a Qoder CLI session. |
| Qoder CLI China | `agent_qoder_cn_enabled` | on, off | `on` | Offer Qoder CLI China for new sessions. |
| Qoder CLI China executable | `agent_qoder_cn_executable` | a path | empty | The program that starts Qoder CLI China. Empty lets the login shell find `qoderclicn`. |
| Qoder CLI China arguments, new session | `agent_qoder_cn_args` | text | empty | Extra arguments typed after the command of a new Qoder CLI China session. |
| Qoder CLI China arguments, resume | `agent_qoder_cn_resume_args` | text | empty | Extra arguments typed after the command that resumes a Qoder CLI China session. |
| Prime Agent | `agent_prime_agent_enabled` | on, off | `on` | Offer Prime Agent for new sessions. |
| Prime Agent executable | `agent_prime_agent_executable` | a path | empty | The program that starts Prime Agent. Empty lets the login shell find `prime-agent`. |
| Prime Agent arguments, new session | `agent_prime_agent_args` | text | empty | Extra arguments typed after the command of a new Prime Agent session. |
| OpenClaw | `agent_openclaw_enabled` | on, off | `on` | Offer OpenClaw for new sessions. |
| OpenClaw executable | `agent_openclaw_executable` | a path | empty | The program that starts OpenClaw. Empty lets the login shell find `openclaw`. |
| OpenClaw arguments, new session | `agent_openclaw_args` | text | empty | Extra arguments typed after the command of a new OpenClaw session. |
| Jcode | `agent_jcode_enabled` | on, off | `on` | Offer Jcode for new sessions. |
| Jcode executable | `agent_jcode_executable` | a path | empty | The program that starts Jcode. Empty lets the login shell find `jcode`. |
| Jcode arguments, new session | `agent_jcode_args` | text | empty | Extra arguments typed after the command of a new Jcode session. |
| Jcode arguments, resume | `agent_jcode_resume_args` | text | empty | Extra arguments typed after the command that resumes a Jcode session. |

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

Other computers: relay machines, SSH machines and sharing this one.

| Setting | Key | Values | Default | Description |
| --- | --- | --- | --- | --- |
| Share SSH connections | `ssh_multiplex` | on, off | `on` | Reuse one connection per machine for every command instead of a handshake each time. (macOS and Linux only.) |
| Keep a shared connection open | `ssh_persist_minutes` | 1 to 240min | `10` | How long an idle shared SSH connection stays open, in minutes. (macOS and Linux only.) |
| Connect timeout | `ssh_connect_timeout` | 0 to 120s | `0` | How long ssh waits to connect, in seconds. Zero leaves it to ssh. |
| Relay server | `remote_relay_url` | text | `wss://relay.getleon.dev` | The relay that connects two computers that reach it from behind their routers. The default service is operated by Zavu and is not live yet. |
| Name of this computer | `remote_device_name` | text | empty | The name other computers see when you share this one or connect from it. Empty uses the computer's own name. |
| Share this machine | `remote_share` | on, off | `off` | While Leon is open, let computers you pair with a code open terminals and run commands here. A paired computer gets a terminal as you. |
| Ask before pairing | `remote_require_approval` | on, off | `on` | When a computer pairs with the code, show its name and fingerprint here and wait for your answer. Turning this off makes the code itself the approval. |
| Share this machine… |  | button |  | Open the Share screen: the pairing code, the computers paired with this one and a switch to start or stop sharing. |
| Connect a machine… |  | button |  | Open the Connect screen: what a machine is, a form and a test of the connection. |
| Probe the machine on screen |  | button |  | Check that the machine answers and which agents it has. |

## Usage

How much of each agent's limits is left, and where the numbers come from.

| Setting | Key | Values | Default | Description |
| --- | --- | --- | --- | --- |
| Show the usage bar | `usage_bar` | on, off | `on` | A bar at the bottom of the window with each agent's limits. |
| Show Claude Code | `usage_claude` | on, off | `on` | Show Claude Code's limits in the bar and the usage view. |
| Show Codex | `usage_codex` | on, off | `on` | Show Codex's limits in the bar and the usage view. |
| Show opencode | `usage_opencode` | on, off | `on` | Show opencode's limits in the bar and the usage view. |
| Show Grok | `usage_grok` | on, off | `on` | Show Grok's limits in the bar and the usage view. |
| Show Cursor | `usage_cursor` | on, off | `on` | Show Cursor's limits in the bar and the usage view. |
| Show ZCode | `usage_zcode` | on, off | `on` | Show ZCode's limits in the bar and the usage view. |
| Show Antigravity | `usage_antigravity` | on, off | `on` | Show Antigravity's limits in the bar and the usage view. |
| Show Kimi | `usage_kimi` | on, off | `on` | Show Kimi's limits in the bar and the usage view. |
| Refresh interval | `usage_refresh_seconds` | 30 to 3600s | `60` | How often, in seconds, the limits are read again while the window is focused; it pauses in the background and reads once when you return. Network sources back off by themselves when the service asks for it. |
| Warn from | `usage_warn` | 10 to 99% | `75` | From this percentage a limit is shown as high, with a marker as well as a colour. |
| Critical from | `usage_critical` | 11 to 100% | `90` | From this percentage a limit is shown as near its end, and starting a session says so first. |
| Say so before starting a session | `usage_warn_before_session` | on, off | `on` | When an agent's limit is nearly used up, a line says so, with when it resets, before the session starts. It never blocks. |
| Read Claude Code's limits from Anthropic | `usage_claude_network` | on, off | `on` | Leon reads the sign-in token Claude Code already holds (the macOS keychain, or ~/.claude/.credentials.json) and sends it over HTTPS to api.anthropic.com only to ask for your usage; it is never stored, logged or shown. On by default; macOS may ask once for keychain access. |
| Ask OpenAI for Codex's fresher limits | `usage_codex_network` | on, off | `on` | Codex's own session log is read first. When it is more than ten minutes old, Leon reads the ChatGPT sign-in Codex holds (~/.codex/auth.json, read only) and sends it over HTTPS to chatgpt.com only to ask for your usage; it starts no session, writes nothing, and the token is never stored, logged or shown. On by default. Implemented from Orca's reference; not verified against the live service. |
| Read the opencode Go limits from opencode | `usage_opencode_network` | on, off | `on` | For an opencode Go subscription, Leon reads the API key opencode stored (~/.local/share/opencode/auth.json) and sends it over HTTPS to opencode.ai only to ask for the usage; it is never stored, logged or shown. On by default. |
| Read Grok's limits from xAI | `usage_grok_network` | on, off | `on` | Leon reads the sign-in Grok already holds (~/.grok/auth.json, read only) and sends it over HTTPS to cli-chat-proxy.grok.com only to ask for your usage; it is never stored, logged or shown, and an expired sign-in is reported, not refreshed. On by default. Implemented from Orca's reference; not verified against the live service. |
| Read Cursor's limits from Cursor | `usage_cursor_network` | on, off | `on` | Leon reads the session cursor-agent holds (the macOS keychain, or its auth.json) and sends it over HTTPS to cursor.com only to ask for your usage; it is never stored, logged or shown, and an expired session is reported, not refreshed. On by default; macOS may ask once for keychain access. Implemented from Orca's reference; not verified against the live service. |
| Read ZCode's limits from its plan | `usage_zcode_network` | on, off | `on` | Leon reads the plan key ZCode holds (~/.zcode/cli/config.json, read only) and sends it over HTTPS to the plan's own host (api.z.ai or open.bigmodel.cn) only to ask for the quota; it is never stored, logged or shown. On by default. Implemented from Orca's reference; not verified against the live service. |
| Ask agy for Antigravity's limits | `usage_antigravity_network` | on, off | `on` | Leon runs `agy -p /usage` on the machine where Antigravity is installed, only when `agy --version` says it is 1.1.11 or newer (older versions spend a model turn on it). Leon reads no credential. On by default. Implemented from Orca's reference; not verified against the live service. |
| Read Kimi's limits from Moonshot | `usage_kimi_network` | on, off | `on` | Leon reads the sign-in Kimi Code holds (~/.kimi-code/credentials/kimi-code.json, read only) and sends it over HTTPS to api.kimi.com only to ask for your usage; it is never stored, logged or shown, and an expired sign-in is reported, not refreshed. On by default. Implemented from Orca's reference; not verified against the live service. |
| Forget stored usage history |  | button |  | Delete the percentages and times kept for the trend lines and the burn-rate estimate. The latest readings stay. |

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
| About Leon |  | button |  | Version 0.1.1, by Zavu: the About panel. |
| Data folder |  | button |  | Show the folder that holds the database and these settings. |
| Themes folder |  | button |  | Show the folder of your theme files. |
| Reset all settings |  | button |  | Put every setting back to its default. Theme files, projects and history are not touched. |

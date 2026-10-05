//! The schema of the settings: the one list of everything that can be
//! configured.
//!
//! Each [`Def`] has a stable key (the key of `settings.json`), a section, a
//! label, a one-line description, search keywords, a kind, a default and the
//! platforms it exists on. Everything else is derived from this table: the
//! file's reading and writing ([`Store`]), the Settings screen, the palette's
//! entries, the generated `docs/SETTINGS.md` ([`render_docs`]) and the
//! tests. Adding a setting to [`SETTINGS`] makes it appear everywhere; its
//! effect is wired where the application reads it (`settings::get_*`).
//!
//! The file is forward and backward compatible. Keys this build does not know
//! are kept when the file is saved, a missing key is the default, and a value
//! of the wrong type or outside its choices falls back to the default for
//! that key only, with a [`Problem`] to report. A number outside its range is
//! clamped, not refused.

use serde_json::{Map, Number, Value as Json};

/// The sections of the Settings screen, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    /// Theme, appearance, size and blueprint lines.
    Appearance,
    /// Font, scrollback, cursor, clipboard and shell of terminals.
    Terminal,
    /// Claude Code, Codex and opencode.
    Agents,
    /// What is imported and shown of the history.
    Sessions,
    /// Discovery, logos and avatars.
    Projects,
    /// SSH machines.
    Machines,
    /// How much of each agent's limits is left.
    Usage,
    /// The sidebar and quitting.
    Window,
    /// Every command with its chords (read-only).
    Keyboard,
    /// Version, folders, logging and reset.
    Advanced,
}

impl Section {
    /// Every section, in order.
    pub const ALL: [Section; 10] = [
        Section::Appearance,
        Section::Terminal,
        Section::Agents,
        Section::Sessions,
        Section::Projects,
        Section::Machines,
        Section::Usage,
        Section::Window,
        Section::Keyboard,
        Section::Advanced,
    ];

    /// Its heading.
    pub fn title(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Terminal => "Terminal",
            Section::Agents => "Agents",
            Section::Sessions => "Sessions & history",
            Section::Projects => "Projects",
            Section::Machines => "Machines",
            Section::Usage => "Usage",
            Section::Window => "Sidebar & window",
            Section::Keyboard => "Keyboard",
            Section::Advanced => "Advanced & About",
        }
    }

    /// A line on what the section holds.
    pub fn blurb(self) -> &'static str {
        match self {
            Section::Appearance => "How Leon looks.",
            Section::Terminal => "How terminals are drawn and started.",
            Section::Agents => "How each coding agent is started and resumed.",
            Section::Sessions => "What is imported and shown of the agents' history.",
            Section::Projects => "How projects are found and what they show.",
            Section::Machines => {
                "Other computers: relay machines, SSH machines and sharing this one."
            }
            Section::Usage => {
                "How much of each agent's limits is left, and where the numbers come from."
            }
            Section::Window => "The sidebar and what quitting asks.",
            Section::Keyboard => "Every command and its shortcuts.",
            Section::Advanced => "Folders, logging and starting over.",
        }
    }
}

/// Where a setting exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// Everywhere.
    All,
    /// macOS only.
    Mac,
    /// Unix (macOS and Linux) only.
    Unix,
}

impl Platform {
    /// Whether it exists on the platform this build runs on.
    pub fn here(self) -> bool {
        match self {
            Platform::All => true,
            Platform::Mac => crate::platform::is_mac(),
            Platform::Unix => cfg!(unix) && !crate::platform::is_windows(),
        }
    }

    /// The words the docs use.
    #[cfg(test)]
    pub fn name(self) -> &'static str {
        match self {
            Platform::All => "all",
            Platform::Mac => "macOS",
            Platform::Unix => "macOS and Linux",
        }
    }
}

/// The answers a choice offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choices {
    /// A fixed list of `(value, label)`.
    Fixed(&'static [(&'static str, &'static str)]),
    /// The themes of the registry, built-in and user.
    Themes,
}

/// What kind of control a setting is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// On or off.
    Toggle,
    /// One of a list.
    Choice(Choices),
    /// An integer in a range, changed by `step`.
    Number {
        /// The least.
        min: i64,
        /// The most.
        max: i64,
        /// What left and right change it by.
        step: i64,
        /// What it is counted in, for display (`px`, `%`, empty).
        unit: &'static str,
    },
    /// Free text.
    Text {
        /// What the empty field says.
        placeholder: &'static str,
    },
    /// A file or folder path, as text.
    Path {
        /// What the empty field says.
        placeholder: &'static str,
    },
    /// A list of text entries.
    List {
        /// What the empty field says.
        placeholder: &'static str,
    },
    /// A button: it does something and holds no value.
    Action,
}

/// What a setting is when nothing was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initial {
    /// A toggle.
    Bool(bool),
    /// A number.
    Int(i64),
    /// Text, a path or a choice.
    Text(&'static str),
    /// An empty list.
    List,
    /// An action: no value.
    None,
}

/// A value of a setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A toggle.
    Bool(bool),
    /// A number.
    Int(i64),
    /// Text, a path or a choice.
    Text(String),
    /// A list.
    List(Vec<String>),
}

impl Value {
    /// The JSON it is written as.
    pub fn to_json(&self) -> Json {
        match self {
            Value::Bool(value) => Json::Bool(*value),
            Value::Int(value) => Json::Number(Number::from(*value)),
            Value::Text(value) => Json::String(value.clone()),
            Value::List(items) => Json::Array(items.iter().cloned().map(Json::String).collect()),
        }
    }

    /// The text of a value, for display and for matching.
    pub fn display(&self) -> String {
        match self {
            Value::Bool(true) => "On".to_owned(),
            Value::Bool(false) => "Off".to_owned(),
            Value::Int(value) => value.to_string(),
            Value::Text(value) => value.clone(),
            Value::List(items) => items.join(", "),
        }
    }

    /// The boolean, when it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The number, when it is one.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The text, when it is some.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(value) => Some(value),
            _ => None,
        }
    }

    /// The entries, when it is a list.
    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }
}

/// One setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Def {
    /// The key of `settings.json`, stable for ever.
    pub key: &'static str,
    /// The section it is listed in.
    pub section: Section,
    /// What it is called.
    pub label: &'static str,
    /// One line on what it does.
    pub description: &'static str,
    /// Words that find it besides its label.
    pub keywords: &'static str,
    /// The control.
    pub kind: Kind,
    /// What it is when nothing was chosen.
    pub default: Initial,
    /// Where it exists.
    pub platform: Platform,
}

const fn def(
    key: &'static str,
    section: Section,
    label: &'static str,
    description: &'static str,
    keywords: &'static str,
    kind: Kind,
    default: Initial,
) -> Def {
    Def {
        key,
        section,
        label,
        description,
        keywords,
        kind,
        default,
        platform: Platform::All,
    }
}

const fn only(mut d: Def, platform: Platform) -> Def {
    d.platform = platform;
    d
}

use Initial as D;
use Kind as K;
use Section as S;

const APPEARANCES: Choices =
    Choices::Fixed(&[("system", "System"), ("light", "Light"), ("dark", "Dark")]);
const LINES: Choices = Choices::Fixed(&[("theme", "Theme default"), ("on", "On"), ("off", "Off")]);
const MOTION: Choices =
    Choices::Fixed(&[("system", "Follow system"), ("on", "On"), ("off", "Off")]);
const CURSORS: Choices = Choices::Fixed(&[
    ("block", "Block"),
    ("beam", "Beam"),
    ("underline", "Underline"),
]);
const PASTES: Choices = Choices::Fixed(&[
    ("auto", "Automatic"),
    ("text", "Always text"),
    ("image", "Image when there is one"),
]);
const DEFAULT_AGENTS: Choices = Choices::Fixed(&[
    ("ask", "Ask each time"),
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("opencode", "opencode"),
]);
const OPEN_MODES: Choices = Choices::Fixed(&[
    ("resume", "Resume in a terminal"),
    ("transcript", "Open the transcript"),
]);
const QUIT: Choices = Choices::Fixed(&[
    ("running", "When programs are running"),
    ("always", "Always"),
    ("never", "Never"),
]);
const LEVELS: Choices = Choices::Fixed(&[
    ("error", "Error"),
    ("warn", "Warn"),
    ("info", "Info"),
    ("debug", "Debug"),
    ("trace", "Trace"),
]);

const NO_ARGS: Kind = K::Text {
    placeholder: "none",
};
const FOLDER: Kind = K::Path {
    placeholder: "the agent's default",
};
const PROGRAM: Kind = K::Path {
    placeholder: "found by the login shell",
};

/// The registry of settings.
pub const SETTINGS: &[Def] = &[
    // ----- appearance
    def(
        "theme_id",
        S::Appearance,
        "Theme",
        "The brand look: colours, fonts and shape. Built-in and your own theme files.",
        "colors colours look custom user brand",
        K::Choice(Choices::Themes),
        D::Text("leon"),
    ),
    def(
        "theme",
        S::Appearance,
        "Appearance",
        "Light, dark, or whatever the desktop is set to.",
        "dark light mode system night day",
        K::Choice(APPEARANCES),
        D::Text("dark"),
    ),
    def(
        "interface_scale",
        S::Appearance,
        "Interface size",
        "The size of the whole interface, in percent of the design: 80, 90, 100, 110 or 125.",
        "zoom scale larger smaller dpi",
        K::Number {
            min: 80,
            max: 125,
            step: 5,
            unit: "%",
        },
        D::Int(100),
    ),
    def(
        "blueprint_lines",
        S::Appearance,
        "Blueprint lines",
        "The crosshairs, corner ticks and frames of the line system: the theme's own, always, or never.",
        "crosshair ticks grid guides decoration",
        K::Choice(LINES),
        D::Text("theme"),
    ),
    def(
        "animate_lion",
        S::Appearance,
        "Animate the lion",
        "The Leon mark blinks, narrows its eyes and glances now and then, and shows how the sessions are doing. Off keeps it still.",
        "logo mark animation motion blink glare mascot",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "reduce_motion",
        S::Appearance,
        "Reduce motion",
        "Keep the interface still: follow the system's reduce-motion preference, always reduce motion, or never. Reduced motion shows the lion at rest.",
        "animation accessibility vestibular still calm motion",
        K::Choice(MOTION),
        D::Text("system"),
    ),
    // ----- terminal
    def(
        "terminal_font_family",
        S::Terminal,
        "Font family",
        "A monospaced family, bundled or installed. Empty is the theme's own.",
        "typeface mono monospace",
        K::Text {
            placeholder: "the theme's monospace font",
        },
        D::Text(""),
    ),
    def(
        "terminal_font_size",
        S::Terminal,
        "Font size",
        "The size of terminal text in pixels at the 100% interface size.",
        "text points px",
        K::Number {
            min: 8,
            max: 32,
            step: 1,
            unit: "px",
        },
        D::Int(13),
    ),
    def(
        "terminal_line_height",
        S::Terminal,
        "Line height",
        "The height of a terminal row as a percentage of the font size.",
        "leading spacing rows",
        K::Number {
            min: 100,
            max: 200,
            step: 5,
            unit: "%",
        },
        D::Int(135),
    ),
    def(
        "terminal_scrollback",
        S::Terminal,
        "Scrollback lines",
        "How many lines of history each terminal keeps. Zero keeps none.",
        "history buffer lines",
        K::Number {
            min: 0,
            max: 100_000,
            step: 1000,
            unit: "",
        },
        D::Int(10_000),
    ),
    def(
        "terminal_cursor",
        S::Terminal,
        "Cursor shape",
        "The cursor a program gets until it asks for another.",
        "caret block beam bar underline",
        K::Choice(CURSORS),
        D::Text("block"),
    ),
    def(
        "terminal_copy_on_select",
        S::Terminal,
        "Copy on select",
        "Copy the selection to the clipboard as soon as the mouse is released.",
        "clipboard selection mouse",
        K::Toggle,
        D::Bool(false),
    ),
    def(
        "terminal_paste",
        S::Terminal,
        "Paste with an image on the clipboard",
        "What paste does when the clipboard holds text and an image: decide by the program in front, always paste the text, or send the image.",
        "clipboard mixed image screenshot ctrl+v",
        K::Choice(PASTES),
        D::Text("auto"),
    ),
    only(
        def(
            "terminal_option_as_meta",
            S::Terminal,
            "Option as Meta",
            "Option sends Escape before the key, as shells and editors expect. Off types the composed character (å, é).",
            "alt meta escape compose accent",
            K::Toggle,
            D::Bool(true),
        ),
        Platform::Mac,
    ),
    def(
        "terminal_confirm_close",
        S::Terminal,
        "Confirm before closing a running pane",
        "Ask before closing a pane that has a program running in it.",
        "close kill ask running",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "terminal_shell",
        S::Terminal,
        "Shell",
        "The program new terminals on this computer run. Empty is your login shell.",
        "default shell zsh bash fish pwsh login",
        K::Path {
            placeholder: "your login shell",
        },
        D::Text(""),
    ),
    def(
        "terminal_shell_args",
        S::Terminal,
        "Shell arguments",
        "Arguments of a custom shell, separated by spaces. Ignored with the login shell.",
        "flags options",
        K::Text {
            placeholder: "-l -i",
        },
        D::Text(""),
    ),
    def(
        "terminal_env",
        S::Terminal,
        "Extra environment variables",
        "NAME=value lines added to the environment of terminals on this computer.",
        "env variables path",
        K::List {
            placeholder: "NAME=value",
        },
        D::List,
    ),
    def(
        "terminal_bell_mark",
        S::Terminal,
        "Mark a session that rings the bell",
        "Show the bell mark on a background session whose program rang the terminal bell.",
        "bell beep notification alert",
        K::Toggle,
        D::Bool(true),
    ),
    // ----- agents
    def(
        "default_agent",
        S::Agents,
        "Agent for a new session",
        "The agent New agent session starts without asking, or ask each time.",
        "default claude codex opencode new",
        K::Choice(DEFAULT_AGENTS),
        D::Text("ask"),
    ),
    def(
        "history_open",
        S::Agents,
        "Opening a history session",
        "What Enter or a click on a history session does: resume it in a terminal, or show its transcript.",
        "resume transcript enter click",
        K::Choice(OPEN_MODES),
        D::Text("resume"),
    ),
    def(
        "agent_claude_enabled",
        S::Agents,
        "Claude Code",
        "Offer Claude Code for new sessions.",
        "claude anthropic enabled",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "agent_claude_executable",
        S::Agents,
        "Claude Code executable",
        "The program that starts Claude Code. Empty lets the login shell find `claude`.",
        "claude path binary",
        PROGRAM,
        D::Text(""),
    ),
    def(
        "agent_claude_args",
        S::Agents,
        "Claude Code arguments, new session",
        "Extra arguments typed after the command of a new Claude Code session.",
        "claude flags options",
        NO_ARGS,
        D::Text(""),
    ),
    def(
        "agent_claude_resume_args",
        S::Agents,
        "Claude Code arguments, resume",
        "Extra arguments typed after the command that resumes a Claude Code session.",
        "claude flags options resume",
        NO_ARGS,
        D::Text(""),
    ),
    def(
        "agent_codex_enabled",
        S::Agents,
        "Codex",
        "Offer Codex for new sessions.",
        "codex openai enabled",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "agent_codex_executable",
        S::Agents,
        "Codex executable",
        "The program that starts Codex. Empty lets the login shell find `codex`.",
        "codex path binary",
        PROGRAM,
        D::Text(""),
    ),
    def(
        "agent_codex_args",
        S::Agents,
        "Codex arguments, new session",
        "Extra arguments typed after the command of a new Codex session.",
        "codex flags options",
        NO_ARGS,
        D::Text(""),
    ),
    def(
        "agent_codex_resume_args",
        S::Agents,
        "Codex arguments, resume",
        "Extra arguments typed after the command that resumes a Codex session.",
        "codex flags options resume",
        NO_ARGS,
        D::Text(""),
    ),
    def(
        "agent_opencode_enabled",
        S::Agents,
        "opencode",
        "Offer opencode for new sessions.",
        "opencode enabled",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "agent_opencode_executable",
        S::Agents,
        "opencode executable",
        "The program that starts opencode. Empty lets the login shell find `opencode`.",
        "opencode path binary",
        PROGRAM,
        D::Text(""),
    ),
    def(
        "agent_opencode_args",
        S::Agents,
        "opencode arguments, new session",
        "Extra arguments typed after the command of a new opencode session.",
        "opencode flags options",
        NO_ARGS,
        D::Text(""),
    ),
    def(
        "agent_opencode_resume_args",
        S::Agents,
        "opencode arguments, resume",
        "Extra arguments typed after the command that resumes an opencode session.",
        "opencode flags options resume",
        NO_ARGS,
        D::Text(""),
    ),
    // ----- sessions and history
    def(
        "import_on_start",
        S::Sessions,
        "Import history on start",
        "Read the agents' history and refresh projects when Leon starts. Refresh still works by hand.",
        "startup refresh sync",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "history_dir_claude",
        S::Sessions,
        "Claude Code history folder",
        "Where Claude Code keeps its projects. Empty is ~/.claude/projects.",
        "claude folder directory path",
        FOLDER,
        D::Text(""),
    ),
    def(
        "history_dir_codex",
        S::Sessions,
        "Codex history folder",
        "Where Codex keeps its sessions. Empty is ~/.codex/sessions.",
        "codex folder directory path",
        FOLDER,
        D::Text(""),
    ),
    def(
        "history_dir_opencode",
        S::Sessions,
        "opencode database",
        "The opencode database file. Empty is its own location.",
        "opencode database sqlite path",
        FOLDER,
        D::Text(""),
    ),
    def(
        "sessions_per_worktree",
        S::Sessions,
        "Sessions per worktree",
        "How many history sessions a worktree lists before \"show more\".",
        "show more limit rows",
        K::Number {
            min: 3,
            max: 50,
            step: 1,
            unit: "",
        },
        D::Int(8),
    ),
    def(
        "detect_elsewhere",
        S::Sessions,
        "Detect sessions running elsewhere",
        "Look for agent processes Leon did not start and mark the sessions they hold.",
        "processes other terminal iterm tmux",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "elsewhere_interval",
        S::Sessions,
        "Detection interval",
        "How often, in seconds, this computer is looked at while the window is focused.",
        "poll seconds scan",
        K::Number {
            min: 2,
            max: 120,
            step: 1,
            unit: "s",
        },
        D::Int(5),
    ),
    def(
        "reimport",
        S::Sessions,
        "Re-import now",
        "Read the agents' history again and sync every project's worktrees.",
        "refresh import sync reload",
        K::Action,
        D::None,
    ),
    // ----- projects
    def(
        "discover_projects",
        S::Projects,
        "Discover projects from session folders",
        "Add the repositories your sessions ran in as projects.",
        "find automatic folders repositories",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "detect_logos",
        S::Projects,
        "Detect project logos",
        "Look in the repository for an icon to show beside the project.",
        "icons images favicon",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "fetch_avatars",
        S::Projects,
        "Fetch owner avatars from the Git host",
        "When a repository has no icon, download its owner's avatar from the Git host (GitHub). This is the only network call Leon makes.",
        "network github internet download privacy offline",
        K::Toggle,
        D::Bool(true),
    ),
    // ----- machines
    only(
        def(
            "ssh_multiplex",
            S::Machines,
            "Share SSH connections",
            "Reuse one connection per machine for every command instead of a handshake each time.",
            "ssh multiplexing controlmaster connection",
            K::Toggle,
            D::Bool(true),
        ),
        Platform::Unix,
    ),
    only(
        def(
            "ssh_persist_minutes",
            S::Machines,
            "Keep a shared connection open",
            "How long an idle shared SSH connection stays open, in minutes.",
            "ssh controlpersist idle",
            K::Number {
                min: 1,
                max: 240,
                step: 1,
                unit: "min",
            },
            D::Int(10),
        ),
        Platform::Unix,
    ),
    def(
        "ssh_connect_timeout",
        S::Machines,
        "Connect timeout",
        "How long ssh waits to connect, in seconds. Zero leaves it to ssh.",
        "ssh timeout connect seconds",
        K::Number {
            min: 0,
            max: 120,
            step: 1,
            unit: "s",
        },
        D::Int(0),
    ),
    def(
        "remote_relay_url",
        S::Machines,
        "Relay server",
        "The relay that connects two computers that reach it from behind their routers. The default service is operated by Zavu and is not live yet.",
        "relay server url websocket wss connect code share network",
        K::Text {
            placeholder: "wss://relay.zavu.dev",
        },
        D::Text("wss://relay.zavu.dev"),
    ),
    def(
        "remote_device_name",
        S::Machines,
        "Name of this computer",
        "The name other computers see when you share this one or connect from it. Empty uses the computer's own name.",
        "device name hostname share pair",
        K::Text {
            placeholder: "this computer's name",
        },
        D::Text(""),
    ),
    def(
        "remote_share",
        S::Machines,
        "Share this machine",
        "While Leon is open, let computers you pair with a code open terminals and run commands here. A paired computer gets a terminal as you.",
        "share host service pair code relay remote access",
        K::Toggle,
        D::Bool(false),
    ),
    def(
        "remote_require_approval",
        S::Machines,
        "Ask before pairing",
        "When a computer pairs with the code, show its name and fingerprint here and wait for your answer. Turning this off makes the code itself the approval.",
        "approve pairing confirm security",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "share_machine",
        S::Machines,
        "Share this machine…",
        "Open the Share screen: the pairing code, the computers paired with this one and a switch to start or stop sharing.",
        "share host pair code devices revoke relay",
        K::Action,
        D::None,
    ),
    def(
        "add_machine",
        S::Machines,
        "Connect a machine…",
        "Open the Connect screen: what a machine is, a form and a test of the connection.",
        "add ssh server remote connect computer host new",
        K::Action,
        D::None,
    ),
    def(
        "probe_machine",
        S::Machines,
        "Probe the machine on screen",
        "Check that the machine answers and which agents it has.",
        "ssh check refresh",
        K::Action,
        D::None,
    ),
    // ----- usage
    def(
        "usage_bar",
        S::Usage,
        "Show the usage bar",
        "A bar at the bottom of the window with each agent's limits.",
        "limits quota status bar rate",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "usage_claude",
        S::Usage,
        "Show Claude Code",
        "Show Claude Code's limits in the bar and the usage view.",
        "limits anthropic quota",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "usage_codex",
        S::Usage,
        "Show Codex",
        "Show Codex's limits in the bar and the usage view.",
        "limits openai quota",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "usage_opencode",
        S::Usage,
        "Show opencode",
        "Show opencode's limits in the bar and the usage view.",
        "limits go quota",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "usage_interval",
        S::Usage,
        "Refresh interval",
        "How often, in minutes, the limits are read again while the window is focused.",
        "poll minutes limits refresh",
        K::Number {
            min: 1,
            max: 120,
            step: 1,
            unit: "min",
        },
        D::Int(5),
    ),
    def(
        "usage_warn",
        S::Usage,
        "Warn from",
        "From this percentage a limit is shown as high, with a marker as well as a colour.",
        "threshold warning percent limits",
        K::Number {
            min: 10,
            max: 99,
            step: 5,
            unit: "%",
        },
        D::Int(75),
    ),
    def(
        "usage_critical",
        S::Usage,
        "Critical from",
        "From this percentage a limit is shown as near its end, and starting a session says so first.",
        "threshold error percent limits",
        K::Number {
            min: 11,
            max: 100,
            step: 5,
            unit: "%",
        },
        D::Int(90),
    ),
    def(
        "usage_warn_before_session",
        S::Usage,
        "Say so before starting a session",
        "When an agent's limit is nearly used up, a line says so, with when it resets, before the session starts. It never blocks.",
        "notice warning start resume limits",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "usage_claude_network",
        S::Usage,
        "Read Claude Code's limits from Anthropic",
        "When on, Leon reads the sign-in token Claude Code already holds (the macOS keychain, or ~/.claude/.credentials.json), sends it over HTTPS to api.anthropic.com only to ask for your usage, and never stores, logs or shows it. Off by default; macOS may ask for permission the first time.",
        "network anthropic credential token oauth privacy limits",
        K::Toggle,
        D::Bool(false),
    ),
    def(
        "usage_opencode_network",
        S::Usage,
        "Read the opencode Go limits from opencode",
        "For an opencode Go subscription: when on, Leon reads the API key opencode stored for it (~/.local/share/opencode/auth.json) and sends it over HTTPS to opencode.ai only to ask for the usage; it is never stored, logged or shown. Off by default.",
        "network go credential key privacy limits",
        K::Toggle,
        D::Bool(false),
    ),
    def(
        "usage_forget_history",
        S::Usage,
        "Forget stored usage history",
        "Delete the percentages and times kept for the trend lines and the burn-rate estimate. The latest readings stay.",
        "clear delete sparkline history limits",
        K::Action,
        D::None,
    ),
    // ----- sidebar and window
    def(
        "sidebar_visible",
        S::Window,
        "Show the sidebar",
        "The tree of machines, projects, worktrees and sessions.",
        "tree hide panel",
        K::Toggle,
        D::Bool(true),
    ),
    def(
        "sidebar_width",
        S::Window,
        "Sidebar width",
        "The sidebar's width in pixels at the 100% interface size.",
        "panel resize",
        K::Number {
            min: 220,
            max: 560,
            step: 24,
            unit: "px",
        },
        D::Int(320),
    ),
    def(
        "quit_confirmation",
        S::Window,
        "Confirm before quitting",
        "Ask before quitting: only while programs run in terminals, always, or never.",
        "quit close exit ask",
        K::Choice(QUIT),
        D::Text("running"),
    ),
    // ----- advanced
    def(
        "log_level",
        S::Advanced,
        "Log level",
        "How much Leon writes to its log (standard error). The RUST_LOG variable wins when set.",
        "debug trace logging verbose",
        K::Choice(LEVELS),
        D::Text("info"),
    ),
    def(
        "about",
        S::Advanced,
        "About Leon",
        concat!("Version ", env!("CARGO_PKG_VERSION"), ", by Zavu: the About panel."),
        "version build licence maker",
        K::Action,
        D::None,
    ),
    def(
        "open_data_folder",
        S::Advanced,
        "Data folder",
        "Show the folder that holds the database and these settings.",
        "reveal finder directory",
        K::Action,
        D::None,
    ),
    def(
        "open_themes_folder",
        S::Advanced,
        "Themes folder",
        "Show the folder of your theme files.",
        "reveal finder directory custom",
        K::Action,
        D::None,
    ),
    def(
        "reset_all",
        S::Advanced,
        "Reset all settings",
        "Put every setting back to its default. Theme files, projects and history are not touched.",
        "default restore clear factory",
        K::Action,
        D::None,
    ),
];

/// The setting with this key.
pub fn find(key: &str) -> Option<&'static Def> {
    SETTINGS.iter().find(|def| def.key == key)
}

/// The settings of a section that exist on this platform.
pub fn of_section(section: Section) -> impl Iterator<Item = &'static Def> {
    SETTINGS
        .iter()
        .filter(move |def| def.section == section && def.platform.here())
}

impl Def {
    /// Whether it is a button.
    pub fn is_action(&self) -> bool {
        matches!(self.kind, Kind::Action)
    }

    /// What it is when nothing was chosen. `None` for an action.
    pub fn default_value(&self) -> Option<Value> {
        Some(match self.default {
            Initial::Bool(value) => Value::Bool(value),
            Initial::Int(value) => Value::Int(value),
            Initial::Text(value) => Value::Text(value.to_owned()),
            Initial::List => Value::List(Vec::new()),
            Initial::None => return None,
        })
    }

    /// The answers of a choice as `(value, label)`. Empty for other kinds.
    pub fn options(&self) -> Vec<(String, String)> {
        match self.kind {
            Kind::Choice(Choices::Fixed(list)) => list
                .iter()
                .map(|(value, label)| ((*value).to_owned(), (*label).to_owned()))
                .collect(),
            Kind::Choice(Choices::Themes) => crate::theme::registry::usable()
                .into_iter()
                .map(|id| (id.slug().to_owned(), id.name().to_owned()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// What `value` is called on screen: a choice's label, a number with its
    /// unit, a toggle's On or Off, a path or text as it is.
    pub fn show(&self, value: &Value) -> String {
        match (self.kind, value) {
            (Kind::Choice(_), Value::Text(text)) => self
                .options()
                .into_iter()
                .find(|(value, _)| value == text)
                .map_or_else(|| text.clone(), |(_, label)| label),
            (Kind::Number { unit, .. }, Value::Int(number)) => format!("{number}{unit}"),
            (_, Value::Text(text)) if text.is_empty() => match self.kind {
                Kind::Text { placeholder }
                | Kind::Path { placeholder }
                | Kind::List { placeholder } => format!("({placeholder})"),
                _ => String::new(),
            },
            (_, Value::List(items)) if items.is_empty() => "(none)".to_owned(),
            (_, value) => value.display(),
        }
    }

    /// Reads a JSON value as this setting's value, or says what is wrong.
    /// A number outside the range is clamped.
    pub fn read_json(&self, json: &Json) -> Result<Value, String> {
        let wrong = |expected: &str| format!("expected {expected}, found {}", short_json(json));
        match self.kind {
            Kind::Toggle => json
                .as_bool()
                .map(Value::Bool)
                .ok_or_else(|| wrong("true or false")),
            Kind::Number { min, max, .. } => json
                .as_i64()
                .or_else(|| {
                    json.as_f64()
                        .filter(|f| f.is_finite())
                        .map(|f| f.round() as i64)
                })
                .map(|number| Value::Int(number.clamp(min, max)))
                .ok_or_else(|| wrong("a number")),
            Kind::Choice(Choices::Themes) => json
                .as_str()
                .map(|text| Value::Text(text.to_owned()))
                .ok_or_else(|| wrong("a theme id")),
            Kind::Choice(Choices::Fixed(list)) => {
                let text = json.as_str().ok_or_else(|| wrong("text"))?;
                if list.iter().any(|(value, _)| *value == text) {
                    Ok(Value::Text(text.to_owned()))
                } else {
                    let names: Vec<&str> = list.iter().map(|(value, _)| *value).collect();
                    Err(format!(
                        "expected one of {}, found {}",
                        names.join(", "),
                        short_json(json)
                    ))
                }
            }
            Kind::Text { .. } | Kind::Path { .. } => json
                .as_str()
                .map(|text| Value::Text(text.to_owned()))
                .ok_or_else(|| wrong("text")),
            Kind::List { .. } => match json.as_array() {
                Some(items) if items.iter().all(Json::is_string) => Ok(Value::List(
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect(),
                )),
                _ => Err(wrong("a list of text")),
            },
            Kind::Action => Err("an action holds no value".to_owned()),
        }
    }

    /// Reads typed text as this setting's value (the palette and the screen's
    /// text fields): a number must be in range and on its step's grid is not
    /// required, only in range.
    pub fn parse_input(&self, text: &str) -> Result<Value, String> {
        let text = text.trim();
        match self.kind {
            Kind::Number { min, max, .. } => {
                let number: i64 = text
                    .trim_end_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .map_err(|_| format!("Type a number from {min} to {max}."))?;
                if (min..=max).contains(&number) {
                    Ok(Value::Int(number))
                } else {
                    Err(format!("Use a number from {min} to {max}."))
                }
            }
            Kind::Text { .. } | Kind::Path { .. } => Ok(Value::Text(text.to_owned())),
            Kind::Toggle => match text.to_ascii_lowercase().as_str() {
                "on" | "true" | "yes" => Ok(Value::Bool(true)),
                "off" | "false" | "no" => Ok(Value::Bool(false)),
                _ => Err("Type on or off.".to_owned()),
            },
            Kind::Choice(_) => {
                let wanted = text.to_ascii_lowercase();
                self.options()
                    .into_iter()
                    .find(|(value, label)| {
                        value.to_ascii_lowercase() == wanted || label.to_ascii_lowercase() == wanted
                    })
                    .map(|(value, _)| Value::Text(value))
                    .ok_or_else(|| "That is not one of the choices.".to_owned())
            }
            Kind::List { .. } | Kind::Action => Err("Not a typed value.".to_owned()),
        }
    }

    /// One step of left (`-1`) or right (`1`) on a value: the next choice,
    /// the number moved by its step within its range, or the toggle flipped.
    pub fn nudge(&self, value: &Value, direction: i64) -> Option<Value> {
        match (self.kind, value) {
            (Kind::Toggle, Value::Bool(on)) => Some(Value::Bool(!on)),
            (Kind::Number { min, max, step, .. }, Value::Int(number)) => {
                let next = if self.key == "interface_scale" {
                    i64::from(crate::theme::next_step(*number as u16, direction > 0))
                } else {
                    (number + direction * step).clamp(min, max)
                };
                Some(Value::Int(next))
            }
            (Kind::Choice(_), Value::Text(text)) => {
                let options = self.options();
                if options.is_empty() {
                    return None;
                }
                let at = options.iter().position(|(value, _)| value == text);
                let len = options.len() as i64;
                let next = match at {
                    Some(at) => (at as i64 + direction).rem_euclid(len) as usize,
                    None => 0,
                };
                Some(Value::Text(options[next].0.clone()))
            }
            _ => None,
        }
    }
}

fn short_json(json: &Json) -> String {
    let text = json.to_string();
    if text.chars().count() > 40 {
        format!("{}…", text.chars().take(40).collect::<String>())
    } else {
        text
    }
}

/// A value in the file that could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The key.
    pub key: String,
    /// What was wrong, ending with what is used instead.
    pub message: String,
}

impl Problem {
    /// The line the status bar shows.
    pub fn line(&self) -> String {
        format!("settings.json: {} {}", self.key, self.message)
    }
}

/// The contents of `settings.json`: every key it holds, the unknown ones
/// included, so that saving never loses what this build does not understand.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Store {
    map: Map<String, Json>,
}

impl Store {
    /// An empty store: every setting is its default.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads the file's bytes. Text that is not a JSON object is an error
    /// (the caller keeps what it had); bad values are reported by
    /// [`Self::problems`] and read as defaults.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        match serde_json::from_slice::<Json>(bytes) {
            Ok(Json::Object(map)) => Ok(Self { map }),
            Ok(_) => Err("the file is not a JSON object".to_owned()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// The file's text.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(&self.map).unwrap_or_else(|_| b"{}".to_vec());
        bytes.push(b'\n');
        bytes
    }

    /// The value of a setting: what the file says when it is usable, else the
    /// default. `None` for an action.
    pub fn get(&self, def: &Def) -> Option<Value> {
        let default = def.default_value()?;
        Some(
            self.map
                .get(def.key)
                .and_then(|json| def.read_json(json).ok())
                .unwrap_or(default),
        )
    }

    /// The value of the setting with this key, or its default. Panics in a
    /// test for a key that is not in the schema.
    pub fn value(&self, key: &str) -> Value {
        let def = find(key).unwrap_or_else(|| panic!("{key} is not a setting"));
        self.get(def)
            .unwrap_or_else(|| panic!("{key} is an action"))
    }

    /// Whether the setting differs from its default.
    pub fn is_modified(&self, def: &Def) -> bool {
        self.get(def) != def.default_value()
    }

    /// Chooses a value. The default is stored as no key at all, so a file
    /// holds what was chosen and nothing else.
    pub fn set(&mut self, def: &Def, value: Value) {
        if Some(&value) == def.default_value().as_ref() {
            self.map.remove(def.key);
        } else {
            self.map.insert(def.key.to_owned(), value.to_json());
        }
    }

    /// Puts a setting back to its default.
    pub fn reset(&mut self, def: &Def) {
        self.map.remove(def.key);
    }

    /// Puts every setting back to its default and drops unknown keys too.
    pub fn reset_all(&mut self) {
        self.map.clear();
    }

    /// The keys this build does not know.
    #[cfg(test)]
    pub fn unknown_keys(&self) -> Vec<&str> {
        self.map
            .keys()
            .filter(|key| find(key).is_none())
            .map(String::as_str)
            .collect()
    }

    /// The values that could not be used, each read as its default.
    pub fn problems(&self) -> Vec<Problem> {
        SETTINGS
            .iter()
            .filter(|def| !def.is_action())
            .filter_map(|def| {
                let json = self.map.get(def.key)?;
                let error = def.read_json(json).err()?;
                let used = def
                    .default_value()
                    .map(|value| def.show(&value))
                    .unwrap_or_default();
                Some(Problem {
                    key: def.key.to_owned(),
                    message: format!("{error}; using {used}."),
                })
            })
            .collect()
    }
}

// ----- the generated reference -----------------------------------------------

#[cfg(test)]
/// The text of `docs/SETTINGS.md`: every setting, by section, from the schema.
/// A test fails when the file is stale; `LEON_BLESS=1 cargo test -p leon
/// the_settings_reference_is_current` rewrites it.
pub fn render_docs() -> String {
    let mut out = String::new();
    out.push_str(
        "# Settings\n\n\
         <!-- Generated from crates/app/src/schema.rs: do not edit by hand. -->\n\n\
         Open the Settings screen with `Cmd+,` (`Ctrl+,` on Linux and Windows), the gear in the\n\
         sidebar, the application menu or the command palette. Every option applies at once.\n\
         The choices are kept in `settings.json` in the data folder (`--data-dir`, or the\n\
         platform's); the palette's **Open settings.json** and **Reveal settings folder**\n\
         commands show it. The file may be edited by hand while Leon runs: a change is picked up\n\
         within a second or two. A value that cannot be used falls back to its default for that\n\
         key only and the status line says so; keys Leon does not know are kept when it saves.\n\
         A setting equal to its default is not written at all.\n",
    );
    for section in Section::ALL {
        out.push_str(&format!(
            "\n## {}\n\n{}\n\n",
            section.title(),
            section.blurb()
        ));
        let defs: Vec<&Def> = SETTINGS.iter().filter(|d| d.section == section).collect();
        if defs.is_empty() {
            out.push_str(
                "Read-only: every command with its chords, generated from the shortcut registry.\n\
                 Shortcuts are not customisable yet.\n",
            );
            continue;
        }
        out.push_str(
            "| Setting | Key | Values | Default | Description |\n| --- | --- | --- | --- | --- |\n",
        );
        for def in defs {
            let values = match def.kind {
                Kind::Toggle => "on, off".to_owned(),
                Kind::Choice(Choices::Themes) => "a theme id".to_owned(),
                Kind::Choice(Choices::Fixed(list)) => list
                    .iter()
                    .map(|(value, _)| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                Kind::Number { min, max, unit, .. } => format!("{min} to {max}{unit}"),
                Kind::Text { .. } => "text".to_owned(),
                Kind::Path { .. } => "a path".to_owned(),
                Kind::List { .. } => "a list of text".to_owned(),
                Kind::Action => "button".to_owned(),
            };
            let default = match def.default_value() {
                Some(Value::Text(text)) if text.is_empty() => "empty".to_owned(),
                Some(Value::List(items)) if items.is_empty() => "empty".to_owned(),
                Some(value) => format!("`{}`", value.display().to_lowercase()),
                None => "".to_owned(),
            };
            let key = if def.is_action() {
                "".to_owned()
            } else {
                format!("`{}`", def.key)
            };
            let platform = match def.platform {
                Platform::All => String::new(),
                other => format!(" ({} only.)", other.name()),
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} | {}{} |\n",
                def.label, key, values, default, def.description, platform
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_a_label_a_description_and_a_section_and_a_unique_key() {
        let mut keys = std::collections::HashSet::new();
        for def in SETTINGS {
            assert!(!def.label.trim().is_empty(), "{} has no label", def.key);
            assert!(
                def.description.trim().ends_with('.'),
                "{} needs a one-line description ending with a full stop",
                def.key
            );
            assert!(!def.description.contains('\n'), "{}", def.key);
            assert!(
                !def.keywords.trim().is_empty(),
                "{} has no keywords",
                def.key
            );
            assert!(Section::ALL.contains(&def.section));
            assert!(keys.insert(def.key), "{} is listed twice", def.key);
            assert!(
                def.key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                "{}",
                def.key
            );
        }
    }

    #[test]
    fn every_default_is_valid_for_its_own_kind() {
        for def in SETTINGS.iter().filter(|d| !d.is_action()) {
            let value = def.default_value().expect("a default");
            assert_eq!(
                def.read_json(&value.to_json()).as_ref(),
                Ok(&value),
                "{}",
                def.key
            );
        }
        for def in SETTINGS.iter().filter(|d| d.is_action()) {
            assert!(def.default_value().is_none(), "{}", def.key);
        }
    }

    #[test]
    fn the_stable_keys_of_older_files_are_still_in_the_schema() {
        for key in [
            "theme",
            "theme_id",
            "interface_scale",
            "sidebar_visible",
            "sidebar_width",
        ] {
            assert!(find(key).is_some(), "{key}");
        }
    }

    #[test]
    fn a_missing_key_is_the_default_and_a_default_is_not_written() {
        let mut store = Store::new();
        let size = find("terminal_font_size").unwrap();
        assert_eq!(store.value("terminal_font_size"), Value::Int(13));
        store.set(size, Value::Int(15));
        assert!(store.is_modified(size));
        assert!(String::from_utf8(store.to_bytes()).unwrap().contains("15"));
        store.set(size, Value::Int(13));
        assert!(!store.is_modified(size));
        assert_eq!(store.to_bytes(), b"{}\n");
    }

    #[test]
    fn unknown_keys_survive_a_round_trip() {
        let mut store =
            Store::parse(br#"{"from_the_future": {"a": [1, 2]}, "terminal_font_size": 16}"#)
                .unwrap();
        store.set(find("sidebar_width").unwrap(), Value::Int(400));
        let again = Store::parse(&store.to_bytes()).unwrap();
        assert_eq!(again.unknown_keys(), ["from_the_future"]);
        assert_eq!(again.value("terminal_font_size"), Value::Int(16));
        assert_eq!(again.value("sidebar_width"), Value::Int(400));
        assert!(String::from_utf8(again.to_bytes())
            .unwrap()
            .contains("from_the_future"));
    }

    #[test]
    fn an_invalid_value_falls_back_for_that_key_only_with_a_message() {
        let store = Store::parse(
            br#"{"terminal_font_size": "big", "terminal_cursor": "star", "theme": "light"}"#,
        )
        .unwrap();
        assert_eq!(store.value("terminal_font_size"), Value::Int(13));
        assert_eq!(store.value("terminal_cursor"), Value::Text("block".into()));
        assert_eq!(store.value("theme"), Value::Text("light".into()));
        let problems = store.problems();
        assert_eq!(problems.len(), 2);
        assert!(problems
            .iter()
            .any(|p| p.key == "terminal_font_size" && p.message.contains("using 13px")));
    }

    #[test]
    fn a_number_out_of_range_is_clamped_not_refused() {
        let store =
            Store::parse(br#"{"terminal_scrollback": 99999999, "sidebar_width": 5}"#).unwrap();
        assert_eq!(store.value("terminal_scrollback"), Value::Int(100_000));
        assert_eq!(store.value("sidebar_width"), Value::Int(220));
        assert!(store.problems().is_empty());
    }

    #[test]
    fn a_file_that_is_not_an_object_is_an_error_for_the_caller_to_keep_what_it_had() {
        assert!(Store::parse(b"{ nope").is_err());
        assert!(Store::parse(b"[1]").is_err());
    }

    #[test]
    fn typed_input_is_read_by_kind() {
        let size = find("terminal_font_size").unwrap();
        assert_eq!(size.parse_input(" 14px "), Ok(Value::Int(14)));
        assert!(size.parse_input("99").is_err());
        assert!(size.parse_input("big").is_err());
        let cursor = find("terminal_cursor").unwrap();
        assert_eq!(cursor.parse_input("Beam"), Ok(Value::Text("beam".into())));
        assert!(cursor.parse_input("star").is_err());
    }

    #[test]
    fn left_and_right_move_choices_numbers_and_toggles() {
        let cursor = find("terminal_cursor").unwrap();
        assert_eq!(
            cursor.nudge(&Value::Text("block".into()), 1),
            Some(Value::Text("beam".into()))
        );
        assert_eq!(
            cursor.nudge(&Value::Text("block".into()), -1),
            Some(Value::Text("underline".into()))
        );
        let size = find("terminal_font_size").unwrap();
        assert_eq!(size.nudge(&Value::Int(32), 1), Some(Value::Int(32)));
        assert_eq!(size.nudge(&Value::Int(13), -1), Some(Value::Int(12)));
        let scale = find("interface_scale").unwrap();
        assert_eq!(scale.nudge(&Value::Int(100), 1), Some(Value::Int(110)));
        let copy = find("terminal_copy_on_select").unwrap();
        assert_eq!(copy.nudge(&Value::Bool(false), 1), Some(Value::Bool(true)));
    }

    #[test]
    fn the_settings_reference_is_current() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/SETTINGS.md");
        let expected = render_docs();
        if std::env::var_os("LEON_BLESS").is_some() {
            std::fs::write(&file, &expected).unwrap();
        }
        // A checkout may have turned the line endings into CRLF (Git on
        // Windows does by default); the text is the same.
        let actual = std::fs::read_to_string(&file)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            actual == expected,
            "docs/SETTINGS.md is stale: run `LEON_BLESS=1 cargo test -p leon the_settings_reference_is_current`"
        );
    }
}

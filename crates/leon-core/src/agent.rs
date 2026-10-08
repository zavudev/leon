//! The catalogue of coding agents.
//!
//! An agent is data, not a type: an [`AgentId`] (a stable string) and an
//! [`AgentSpec`] that says how it starts, how a session of it is resumed, how
//! to tell it is installed, what Leon can import of its history and read of its
//! usage limits, and how it is drawn. The built-in catalogue ([`builtin`])
//! holds every agent that Orca's README lists (with the commands its source
//! uses, and the resume forms of its `getAgentResumeArgv`), plus the three
//! that have importers. The user adds any other command line tool as a
//! [`CustomAgent`] in the settings.
//!
//! Resume forms are never guessed: an agent whose resume form is not known
//! from Orca's source or the CLI's own `--help` is *launch only*
//! ([`Resume::None`]).
//!
//! The built-in specs are static. The custom ones live in a small registry
//! ([`register_custom`]) that the application fills from the settings, so that a
//! name or a colour can be looked up from an [`AgentId`] anywhere.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, RwLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The most distinct ids that are remembered; a store with more is not the
/// store of a person.
const MAX_IDS: usize = 512;

/// The longest id, name and command accepted.
const MAX_ID: usize = 40;

/// A stable identifier of an agent: lower-case letters, digits, `-` and `_`,
/// starting with a letter or a digit.
///
/// It is `Copy` and compares as text. The ids of the built-in agents are
/// constants (`AgentId::CLAUDE`); any other valid text becomes an id with
/// [`AgentId::parse`], which is how the store's tags and the custom agents of
/// the settings are read.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentId(&'static str);

static INTERNED: LazyLock<Mutex<HashSet<&'static str>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn valid_id(tag: &str) -> bool {
    let mut chars = tag.chars();
    tag.len() <= MAX_ID
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

#[allow(missing_docs)]
impl AgentId {
    pub const CLAUDE: AgentId = AgentId("claude");
    pub const CODEX: AgentId = AgentId("codex");
    pub const OPENCODE: AgentId = AgentId("opencode");
    pub const GROK: AgentId = AgentId("grok");
    pub const CURSOR: AgentId = AgentId("cursor");
    pub const KIMI: AgentId = AgentId("kimi");
    pub const ZCODE: AgentId = AgentId("zcode");
    pub const ANTIGRAVITY: AgentId = AgentId("antigravity");

    /// The tag used in the database, in source keys and in `settings.json`.
    pub fn as_str(self) -> &'static str {
        self.0
    }

    /// The id with this tag, when the tag is shaped like one. A tag that is
    /// not in the catalogue is still an id (the history of an agent that was
    /// removed keeps its rows); [`AgentId::spec`] says whether it is known.
    pub fn parse(tag: &str) -> Option<Self> {
        if !valid_id(tag) {
            return None;
        }
        if let Some(spec) = builtin().iter().find(|spec| spec.id.0 == tag) {
            return Some(spec.id);
        }
        let mut seen = INTERNED.lock().ok()?;
        if let Some(known) = seen.get(tag) {
            return Some(AgentId(known));
        }
        if seen.len() >= MAX_IDS {
            return None;
        }
        let leaked: &'static str = Box::leak(tag.to_owned().into_boxed_str());
        seen.insert(leaked);
        Some(AgentId(leaked))
    }

    /// The spec of the agent, built-in or custom.
    pub fn spec(self) -> Option<&'static AgentSpec> {
        builtin()
            .iter()
            .find(|spec| spec.id == self)
            .or_else(|| custom_specs().into_iter().find(|spec| spec.id == self))
    }

    /// The name to show: the catalogue's, or the id for an agent that is no
    /// longer in it.
    pub fn name(self) -> &'static str {
        self.spec().map_or(self.0, |spec| spec.name.as_str())
    }

    /// The name in capitals, for the mono labels.
    pub fn tag(self) -> &'static str {
        self.spec().map_or(self.0, |spec| spec.tag.as_str())
    }
}

impl std::fmt::Debug for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AgentId({})", self.0)
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for AgentId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for AgentId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let tag = String::deserialize(deserializer)?;
        AgentId::parse(&tag)
            .ok_or_else(|| serde::de::Error::custom(format!("{tag:?} is not an agent id")))
    }
}

/// How a session of an agent is resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// Not known: the agent is launch only.
    None,
    /// By the agent's own session id: these arguments follow the command,
    /// with `{id}` standing for the id (`--resume {id}`, `--resume={id}`).
    ById(Vec<String>),
    /// The agent's "continue the latest session": these arguments follow the
    /// command and carry no id.
    Latest(Vec<String>),
}

impl Resume {
    /// The arguments that resume `id` (or the latest session), when the agent
    /// can be resumed.
    pub fn args(&self, id: &str) -> Option<Vec<String>> {
        match self {
            Resume::None => None,
            Resume::ById(args) => Some(args.iter().map(|arg| arg.replace("{id}", id)).collect()),
            Resume::Latest(args) => Some(args.clone()),
        }
    }

    /// Whether the form is well formed: `{id}` appears in a by-id form and in
    /// no other, and no other braces appear.
    pub fn is_well_formed(&self) -> bool {
        let (args, wants_id) = match self {
            Resume::None => return true,
            Resume::ById(args) => (args, true),
            Resume::Latest(args) => (args, false),
        };
        let stripped = args.iter().map(|arg| arg.replace("{id}", ""));
        let braces = stripped.clone().any(|arg| arg.contains(['{', '}']));
        let has_id = args.iter().any(|arg| arg.contains("{id}"));
        !args.is_empty() && !braces && has_id == wants_id
    }
}

/// Which importer reads an agent's history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Importer {
    /// Claude Code's `~/.claude/projects` transcripts.
    Claude,
    /// Codex's `~/.codex/sessions` rollouts.
    Codex,
    /// opencode's session store.
    Opencode,
}

/// Which usage provider reads an agent's limits (see `leon-usage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageProvider {
    /// Claude Code: Anthropic's OAuth usage endpoint.
    Claude,
    /// Codex: the rate limits in its own session log.
    Codex,
    /// The opencode Go subscription.
    OpencodeGo,
    /// Grok: the billing endpoint of its CLI proxy.
    Grok,
    /// Cursor: the dashboard's usage summary.
    Cursor,
    /// Kimi Code: the managed `usages` endpoint.
    Kimi,
    /// ZCode: the GLM Coding Plan quota endpoint.
    Zcode,
    /// Antigravity: its own `/usage` command.
    Antigravity,
}

/// The theme colour token of an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tint {
    /// The theme's neutral text colour: no brand colour of its own.
    Neutral,
    /// The `agent_claude` token.
    Claude,
    /// The `agent_codex` token.
    Codex,
    /// The `agent_opencode` token.
    Opencode,
}

/// The agent whose command or detect names include this program name (a
/// bare name or a path), if any.
pub fn by_program(program: &str) -> Option<AgentId> {
    let base = program.rsplit('/').next().unwrap_or(program);
    if base.is_empty() {
        return None;
    }
    all().into_iter().find_map(|spec| {
        let command = spec.command.rsplit('/').next().unwrap_or(&spec.command);
        (command == base || spec.detect.iter().any(|name| name == base)).then_some(spec.id)
    })
}

/// One agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    /// The stable id.
    pub id: AgentId,
    /// The name shown.
    pub name: String,
    /// The name in capitals, for the mono labels.
    pub tag: String,
    /// The program that starts it, found by the login shell.
    pub command: String,
    /// Arguments of a new session, after the command.
    pub args: Vec<String>,
    /// How a session is resumed.
    pub resume: Resume,
    /// The names of the binaries that tell it is installed (the command, and
    /// its aliases).
    pub detect: Vec<String>,
    /// The importer of its history, when Leon has one.
    pub history: Option<Importer>,
    /// The provider of its usage limits, when Leon has one.
    pub usage: Option<UsageProvider>,
    /// The file stem of a bundled mark (`agents/<mark>.svg`); a letter-mark
    /// tile is drawn for an agent without one.
    pub mark: Option<String>,
    /// Its theme colour token.
    pub tint: Tint,
    /// Where to read about it and install it.
    pub docs: Option<String>,
    /// Whether the user added it.
    pub custom: bool,
    /// The line typed (then Enter) to make it quit by itself and save its
    /// session, when that is known: `/exit` for Claude Code (its own command)
    /// and opencode (its `/exit`). Absent where it could not be verified.
    pub exit: Option<String>,
    /// The line typed (then Enter) to rename the session inside the agent,
    /// with `{name}` where the new name goes: `/rename {name}` for Claude
    /// Code. Absent where it could not be verified; those agents only get
    /// Leon's own label.
    pub rename: Option<String>,
    /// The arguments, after the command, that make the agent answer one
    /// instruction without a terminal and exit; the instruction is the last
    /// argument (it tells the agent not to run commands or change files) and
    /// what it reads, a diff, is its standard input, as the agent's own
    /// `--help` says. What it reads is text from the repository, so the form
    /// also takes away what the agent could do with it: Claude Code runs with
    /// no tools at all (`--tools ""`) and Codex in its read-only sandbox.
    /// Absent where that could not be checked against the installed CLI: such
    /// an agent is never asked to word a commit.
    pub headless: Option<Vec<String>>,
    /// How the agent takes a first prompt on its launch line: arguments that
    /// follow the command, with `{prompt}` as one whole word standing for the
    /// prompt (`["{prompt}"]` for an agent that reads it as its argument,
    /// `["--prompt", "{prompt}"]` for one that has a flag for it). Absent
    /// where the form could not be verified; those agents are not offered a
    /// prompt.
    pub prompt: Option<Vec<String>>,
    /// The environment variable that moves the agent's whole configuration
    /// folder (its sign-in, settings and sessions), which is what makes a
    /// second account of the agent: `CLAUDE_CONFIG_DIR` for Claude Code,
    /// `CODEX_HOME` for Codex. Absent where it was not verified; an account of
    /// such an agent can still set any variable, but its history and limits are
    /// not read per account.
    pub config_env: Option<String>,
}

/// The word that stands for the prompt in [`AgentSpec::prompt`].
pub const PROMPT_WORD: &str = "{prompt}";

/// Whether `args` are a well formed prompt form: `{prompt}` is a whole word,
/// once, and no other braces appear.
pub fn prompt_form_is_well_formed(args: &[String]) -> bool {
    args.iter().filter(|arg| *arg == PROMPT_WORD).count() == 1
        && !args
            .iter()
            .filter(|arg| *arg != PROMPT_WORD)
            .any(|arg| arg.contains(['{', '}']))
}

impl AgentSpec {
    /// The line that renames the session inside the agent, when that is known.
    pub fn rename_line(&self, name: &str) -> Option<String> {
        let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
        (!name.is_empty())
            .then_some(self.rename.as_ref())
            .flatten()
            .map(|line| line.replace("{name}", &name))
    }

    /// The program and arguments that start the agent, resuming `resume` when
    /// given and the agent can be resumed. `None` when a resume was asked of
    /// an agent that is launch only.
    pub fn launch(&self, resume: Option<&str>) -> Option<(String, Vec<String>)> {
        let args = match resume {
            None => self.args.clone(),
            Some(id) => self.resume.args(id)?,
        };
        Some((self.command.clone(), args))
    }

    /// Whether a session can be resumed.
    pub fn can_resume(&self) -> bool {
        self.resume != Resume::None
    }

    /// The letters of the letter-mark tile: the initials of the words of the
    /// name, at most two (`Mistral Vibe` is `MV`, `Cline` is `CL`).
    pub fn initials(&self) -> String {
        initials(&self.name)
    }
}

/// The initials of a name, at most two letters, in capitals.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let letters: String = match words.as_slice() {
        [] => "?".to_owned(),
        [one] => one.chars().take(2).collect(),
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
    };
    letters.to_uppercase()
}

struct Row {
    id: &'static str,
    name: &'static str,
    command: &'static str,
    args: &'static [&'static str],
    resume: &'static [&'static str],
    detect: &'static [&'static str],
    mark: Option<&'static str>,
    docs: &'static str,
    exit: &'static str,
    rename: &'static str,
    headless: &'static [&'static str],
    prompt: &'static [&'static str],
    config_env: &'static str,
}

const fn row(
    id: &'static str,
    name: &'static str,
    command: &'static str,
    docs: &'static str,
) -> Row {
    Row {
        id,
        name,
        command,
        args: &[],
        resume: &[],
        detect: &[],
        mark: None,
        docs,
        exit: "",
        rename: "",
        headless: &[],
        prompt: &[],
        config_env: "",
    }
}

impl Row {
    const fn args(mut self, args: &'static [&'static str]) -> Self {
        self.args = args;
        self
    }
    const fn resume(mut self, resume: &'static [&'static str]) -> Self {
        self.resume = resume;
        self
    }
    const fn detect(mut self, detect: &'static [&'static str]) -> Self {
        self.detect = detect;
        self
    }
    const fn mark(mut self, mark: &'static str) -> Self {
        self.mark = Some(mark);
        self
    }
    /// What is typed to make the agent quit by itself, so it can save its
    /// session first. Only for agents whose command is verified.
    const fn exit(mut self, exit: &'static str) -> Self {
        self.exit = exit;
        self
    }
    /// What is typed to rename the session inside the agent, `{name}` being
    /// the new name. Only for agents whose command is verified.
    const fn rename(mut self, rename: &'static str) -> Self {
        self.rename = rename;
        self
    }
    /// The arguments that make the agent answer one instruction and exit,
    /// reading standard input, without changing files. Only for agents whose
    /// form is verified.
    const fn headless(mut self, headless: &'static [&'static str]) -> Self {
        self.headless = headless;
        self
    }
    /// How the agent takes a first prompt on its launch line, `{prompt}`
    /// standing for it. Only for agents whose form was read in the `--help`
    /// of the installed program.
    const fn prompt(mut self, prompt: &'static [&'static str]) -> Self {
        self.prompt = prompt;
        self
    }
    /// The variable that moves the agent's configuration folder. Only for
    /// agents whose variable is verified (the importers and the usage readers
    /// already honour these two).
    const fn config_env(mut self, config_env: &'static str) -> Self {
        self.config_env = config_env;
        self
    }
}

/// The built-in agents, in display order. The prompt forms (`.prompt`) were
/// read in the `--help` of the installed programs: `claude [prompt]`,
/// `codex [PROMPT]`, `grok [PROMPT]` ("Initial prompt for the interactive
/// session"), `cursor-agent [prompt...]` ("Initial prompt for the agent"),
/// `opencode --prompt` and `gemini -i/--prompt-interactive` ("Execute the
/// provided prompt and continue in interactive mode"). Every other agent is
/// left without one until its form is verified. The commands are Orca's
/// (`src/shared/tui-agent-config.ts`) and the resume forms are those of its
/// `getAgentResumeArgv`; where Orca has none the agent is launch only. Orca's
/// pre-trust flags (`--trust`, `--trust-workspace`) are not passed: Leon does
/// not answer an agent's own trust question for the user.
const ROWS: &[Row] = &[
    row(
        "claude",
        "Claude Code",
        "claude",
        "https://code.claude.com/docs",
    )
    .resume(&["--resume", "{id}"])
    .exit("/exit")
    .rename("/rename {name}")
    .headless(&[
        "--tools",
        "",
        "-p",
        "--permission-mode",
        "dontAsk",
        "--no-session-persistence",
    ])
    .prompt(&["{prompt}"])
    .config_env("CLAUDE_CONFIG_DIR")
    .mark("claude"),
    row("codex", "Codex", "codex", "https://github.com/openai/codex")
        .resume(&["resume", "{id}"])
        .headless(&["exec", "--sandbox", "read-only"])
        .prompt(&["{prompt}"])
        .config_env("CODEX_HOME")
        .mark("codex"),
    row(
        "opencode",
        "opencode",
        "opencode",
        "https://opencode.ai/docs/cli/",
    )
    .resume(&["--session", "{id}"])
    .exit("/exit")
    .prompt(&["--prompt", "{prompt}"])
    .mark("opencode"),
    row("grok", "Grok", "grok", "https://x.ai/cli")
        .resume(&["--resume", "{id}"])
        .prompt(&["{prompt}"])
        .mark("grok"),
    row("cursor", "Cursor", "cursor-agent", "https://cursor.com/cli")
        .resume(&["--resume", "{id}"])
        .prompt(&["{prompt}"])
        .mark("cursor"),
    row(
        "copilot",
        "GitHub Copilot",
        "copilot",
        "https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli",
    )
    .resume(&["--resume={id}"])
    .mark("copilot"),
    row("muse", "Muse", "muse", "https://dev.meta.ai/docs/muse-code")
        .resume(&["resume", "{id}"])
        .mark("muse"),
    row(
        "dsh",
        "DeepSeek Harness",
        "dsh-tui",
        "https://deepseek-harness.github.io/deepseek-harness/",
    )
    .args(&["."])
    .resume(&["--resume", "{id}"])
    .detect(&["dsh-tui", "dst"]),
    row("zcode", "ZCode", "zcode", "https://zcode.z.ai/en/docs").resume(&["--resume", "{id}"]),
    row(
        "mimo-code",
        "MiMo Code",
        "mimo",
        "https://mimo.xiaomi.com/coder",
    )
    .resume(&["--session", "{id}"])
    .mark("mimo-code"),
    row("amp", "Amp", "amp", "https://ampcode.com/manual#install"),
    row(
        "openclaude",
        "OpenClaude",
        "openclaude",
        "https://openclaude.gitlawb.com/",
    ),
    row(
        "antigravity",
        "Antigravity",
        "agy",
        "https://antigravity.google/docs/cli-overview",
    )
    .resume(&["--conversation", "{id}"])
    .mark("antigravity"),
    row("pi", "Pi", "pi", "https://pi.dev").mark("pi"),
    row("omp", "oh-my-pi", "omp", "https://omp.sh"),
    row(
        "hermes",
        "Hermes Agent",
        "hermes",
        "https://hermes-agent.nousresearch.com/docs/",
    )
    .args(&["--tui"])
    .mark("hermes-agent"),
    row("devin", "Devin", "devin", "https://devin.ai/cli")
        .resume(&["--resume", "{id}"])
        .mark("devin"),
    row(
        "goose",
        "Goose",
        "goose",
        "https://block.github.io/goose/docs/quickstart/",
    ),
    row(
        "auggie",
        "Auggie",
        "auggie",
        "https://docs.augmentcode.com/cli/overview",
    )
    .mark("auggie"),
    row(
        "autohand",
        "Autohand Code",
        "autohand",
        "https://github.com/autohandai/code-cli",
    ),
    row(
        "crush",
        "Charm Crush",
        "crush",
        "https://github.com/charmbracelet/crush",
    ),
    row(
        "cline",
        "Cline",
        "cline",
        "https://docs.cline.bot/cline-cli/overview",
    )
    .mark("cline"),
    row(
        "codebuddy",
        "CodeBuddy",
        "codebuddy",
        "https://www.codebuddy.ai/cli",
    )
    .resume(&["--resume", "{id}"])
    .detect(&["codebuddy", "cbc"])
    .mark("codebuddy"),
    row(
        "codebuff",
        "Codebuff",
        "codebuff",
        "https://www.codebuff.com/docs/help/quick-start",
    ),
    row(
        "freebuff",
        "Freebuff",
        "freebuff",
        "https://freebuff.com/cli",
    ),
    row(
        "command-code",
        "Command Code",
        "command-code",
        "https://commandcode.ai/docs/quickstart",
    ),
    row(
        "continue",
        "Continue",
        "cn",
        "https://docs.continue.dev/guides/cli",
    ),
    row(
        "droid",
        "Droid",
        "droid",
        "https://docs.factory.ai/cli/getting-started/quickstart",
    )
    .resume(&["--resume", "{id}"]),
    row("kilo", "Kilocode", "kilo", "https://kilo.ai/docs/cli").mark("kilocode"),
    row(
        "kimi",
        "Kimi",
        "kimi",
        "https://www.kimi.com/code/docs/en/kimi-code-cli/getting-started.html",
    )
    .resume(&["--session", "{id}"])
    .detect(&["kimi", "kimi-code"])
    .mark("kimi"),
    row("kiro", "Kiro", "kiro-cli", "https://kiro.dev/docs/cli/")
        .args(&["chat", "--tui"])
        .mark("kiro"),
    row(
        "mistral-vibe",
        "Mistral Vibe",
        "vibe",
        "https://github.com/mistralai/mistral-vibe",
    )
    .detect(&["vibe", "mistral-vibe"])
    .mark("mistral-vibe"),
    row(
        "qwen-code",
        "Qwen Code",
        "qwen",
        "https://github.com/QwenLM/qwen-code",
    )
    .resume(&["--resume", "{id}"])
    .mark("qwen-code"),
    row(
        "rovo",
        "Rovo Dev",
        "rovo",
        "https://support.atlassian.com/rovo/docs/install-and-run-rovo-dev-cli-on-your-device/",
    ),
    row(
        "gemini",
        "Gemini",
        "gemini",
        "https://github.com/google-gemini/gemini-cli",
    )
    .resume(&["--resume", "{id}"])
    .prompt(&["--prompt-interactive", "{prompt}"])
    .mark("gemini"),
    row("aider", "Aider", "aider", "https://aider.chat/docs/"),
    row(
        "ante",
        "Ante",
        "ante",
        "https://github.com/AntigmaLabs/ante-preview",
    ),
    row(
        "trae",
        "Trae",
        "traecli",
        "https://docs.trae.cn/cli_get-started-with-trae-cli",
    )
    .mark("trae"),
    row(
        "qoder",
        "Qoder CLI",
        "qodercli",
        "https://docs.qoder.com/cli/overview",
    )
    .resume(&["--resume", "{id}"])
    .mark("qoder"),
    row(
        "qoder-cn",
        "Qoder CLI China",
        "qoderclicn",
        "https://docs.qoder.cn/cli/overview",
    )
    .resume(&["--resume", "{id}"])
    .detect(&["qoderclicn", "qodercn"])
    .mark("qoder"),
    row(
        "prime-agent",
        "Prime Agent",
        "prime-agent",
        "https://github.com/PrimeIntellect-ai/prime-agent",
    ),
    row(
        "openclaw",
        "OpenClaw",
        "openclaw",
        "https://github.com/openclaw/openclaw",
    ),
    row(
        "jcode",
        "Jcode",
        "jcode",
        "https://github.com/1jehuang/jcode",
    )
    .resume(&["--resume", "{id}"]),
];

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|word| (*word).to_owned()).collect()
}

fn from_row(row: &Row) -> AgentSpec {
    let id = AgentId(row.id);
    let (history, usage, tint) = match row.id {
        "claude" => (
            Some(Importer::Claude),
            Some(UsageProvider::Claude),
            Tint::Claude,
        ),
        "codex" => (
            Some(Importer::Codex),
            Some(UsageProvider::Codex),
            Tint::Codex,
        ),
        "opencode" => (
            Some(Importer::Opencode),
            Some(UsageProvider::OpencodeGo),
            Tint::Opencode,
        ),
        "grok" => (None, Some(UsageProvider::Grok), Tint::Neutral),
        "cursor" => (None, Some(UsageProvider::Cursor), Tint::Neutral),
        "kimi" => (None, Some(UsageProvider::Kimi), Tint::Neutral),
        "zcode" => (None, Some(UsageProvider::Zcode), Tint::Neutral),
        "antigravity" => (None, Some(UsageProvider::Antigravity), Tint::Neutral),
        _ => (None, None, Tint::Neutral),
    };
    AgentSpec {
        id,
        name: row.name.to_owned(),
        tag: row.name.to_uppercase(),
        command: row.command.to_owned(),
        args: words(row.args),
        resume: if row.resume.is_empty() {
            Resume::None
        } else {
            Resume::ById(words(row.resume))
        },
        detect: if row.detect.is_empty() {
            vec![row.command.to_owned()]
        } else {
            words(row.detect)
        },
        history,
        usage,
        mark: row.mark.map(str::to_owned),
        tint,
        docs: Some(row.docs.to_owned()),
        custom: false,
        exit: (!row.exit.is_empty()).then(|| row.exit.to_owned()),
        rename: (!row.rename.is_empty()).then(|| row.rename.to_owned()),
        headless: (!row.headless.is_empty()).then(|| words(row.headless)),
        prompt: (!row.prompt.is_empty()).then(|| words(row.prompt)),
        config_env: (!row.config_env.is_empty()).then(|| row.config_env.to_owned()),
    }
}

static BUILTIN: LazyLock<Vec<AgentSpec>> = LazyLock::new(|| ROWS.iter().map(from_row).collect());

/// The built-in agents, in display order.
pub fn builtin() -> &'static [AgentSpec] {
    &BUILTIN
}

static CUSTOM: RwLock<Vec<&'static AgentSpec>> = RwLock::new(Vec::new());

fn custom_specs() -> Vec<&'static AgentSpec> {
    CUSTOM.read().map(|list| list.clone()).unwrap_or_default()
}

/// Makes `spec` a custom agent known to [`AgentId::spec`] and [`all`], in
/// place of one of the same id. The application calls it for each custom agent
/// of the settings.
pub fn register_custom(spec: AgentSpec) {
    let leaked: &'static AgentSpec = Box::leak(Box::new(spec));
    if let Ok(mut list) = CUSTOM.write() {
        match list.iter().position(|known| known.id == leaked.id) {
            Some(at) => list[at] = leaked,
            None => list.push(leaked),
        }
    }
}

/// Takes the custom agent `id` out of the catalogue.
pub fn unregister_custom(id: AgentId) {
    if let Ok(mut list) = CUSTOM.write() {
        list.retain(|known| known.id != id);
    }
}

/// Every agent, the built-in ones first and then the custom ones.
pub fn all() -> Vec<&'static AgentSpec> {
    builtin().iter().chain(custom_specs()).collect()
}

/// An agent the user added: any command line tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomAgent {
    /// Its id, `custom-` and a slug of the name; kept when it is renamed.
    pub id: String,
    /// The name shown.
    pub name: String,
    /// The program that starts it.
    pub command: String,
    /// Arguments of a new session, as typed.
    #[serde(default)]
    pub args: String,
    /// Arguments that resume a session, as typed: with `{id}` they resume that
    /// session, without they continue the latest one; empty means the agent
    /// cannot be resumed.
    #[serde(default)]
    pub resume_args: String,
    /// How the agent takes a first prompt, as typed: the arguments that follow
    /// the command with `{prompt}` as one word standing for the prompt
    /// (`--prompt {prompt}`); empty means it takes none. It is written in the
    /// entry of `custom_agents` in `settings.json`.
    #[serde(default)]
    pub prompt_args: String,
}

/// What is wrong with an agent the user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomError {
    /// There is no name.
    NoName,
    /// The name is too long or has a control character.
    BadName,
    /// Another agent has that name.
    NameTaken,
    /// There is no command.
    NoCommand,
    /// The command has a control character or is too long.
    BadCommand,
    /// The resume arguments use braces other than a single `{id}` word.
    BadResume,
    /// The arguments have a quote that is never closed.
    BadArgs,
    /// The prompt arguments do not have `{prompt}` as one whole word, once, or
    /// use other braces.
    BadPrompt,
}

impl std::fmt::Display for CustomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CustomError::NoName => "Give the agent a name.",
            CustomError::BadName => "The name is too long or has a control character.",
            CustomError::NameTaken => "Another agent has that name.",
            CustomError::NoCommand => "Give the command that starts the agent.",
            CustomError::BadCommand => "The command is too long or has a control character.",
            CustomError::BadResume => {
                "Resume arguments may use {id} for the session id and no other braces."
            }
            CustomError::BadArgs => "A quote in the arguments is never closed.",
            CustomError::BadPrompt => {
                "Prompt arguments use {prompt} once, as a word of its own, and no other braces."
            }
        })
    }
}

/// Splits what a person typed into arguments the way a shell would: spaces
/// separate, single and double quotes keep words together and a backslash
/// escapes the next character outside single quotes.
pub fn split_words(text: &str) -> Vec<String> {
    split(text).0
}

/// [`split_words`] with whether every quote was closed.
fn split(text: &str) -> (Vec<String>, bool) {
    let (mut words, mut word, mut any) = (Vec::new(), String::new(), false);
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('\''), c) => word.push(c),
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    any = true;
                }
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any {
                    words.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            (None, c) => {
                word.push(c);
                any = true;
            }
        }
    }
    if any {
        words.push(word);
    }
    (words, quote.is_none())
}

/// The slug of a name: lower-case letters and digits, `-` between them.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(24).collect()
}

impl CustomAgent {
    /// A new custom agent as typed, with an id that no agent in `taken` has.
    pub fn new(
        name: &str,
        command: &str,
        args: &str,
        resume_args: &str,
        taken: &[AgentId],
    ) -> Result<Self, CustomError> {
        let slug = slug(name);
        let slug = if slug.is_empty() {
            "agent".into()
        } else {
            slug
        };
        let mut id = format!("custom-{slug}");
        let mut n = 2;
        while taken.iter().any(|t| t.as_str() == id) {
            id = format!("custom-{slug}-{n}");
            n += 1;
        }
        let agent = Self {
            id,
            name: name.trim().to_owned(),
            command: command.trim().to_owned(),
            args: args.trim().to_owned(),
            resume_args: resume_args.trim().to_owned(),
            prompt_args: String::new(),
        };
        agent.to_spec(&[])?;
        Ok(agent)
    }

    /// The agent, taking a first prompt as `prompt_args` says (see
    /// [`CustomAgent::prompt_args`]).
    pub fn with_prompt_args(mut self, prompt_args: &str) -> Result<Self, CustomError> {
        self.prompt_args = prompt_args.trim().to_owned();
        self.to_spec(&[])?;
        Ok(self)
    }

    /// Checks the agent and makes its spec. `others` are the names of the
    /// other agents of the catalogue, which this name must not repeat.
    pub fn to_spec(&self, others: &[&str]) -> Result<AgentSpec, CustomError> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(CustomError::NoName);
        }
        if name.chars().count() > MAX_ID || name.chars().any(char::is_control) {
            return Err(CustomError::BadName);
        }
        if others.iter().any(|other| other.eq_ignore_ascii_case(name)) {
            return Err(CustomError::NameTaken);
        }
        let command = self.command.trim();
        if command.is_empty() {
            return Err(CustomError::NoCommand);
        }
        if command.chars().count() > 200 || command.chars().any(char::is_control) {
            return Err(CustomError::BadCommand);
        }
        let id = AgentId::parse(&self.id).ok_or(CustomError::BadName)?;
        let (args, closed) = split(&self.args);
        let (resume_words, resume_closed) = split(&self.resume_args);
        let (prompt_words, prompt_closed) = split(&self.prompt_args);
        if !closed || !resume_closed || !prompt_closed {
            return Err(CustomError::BadArgs);
        }
        let prompt = if prompt_words.is_empty() {
            None
        } else if prompt_form_is_well_formed(&prompt_words) {
            Some(prompt_words)
        } else {
            return Err(CustomError::BadPrompt);
        };
        let resume = if resume_words.is_empty() {
            Resume::None
        } else if resume_words.iter().any(|word| word.contains("{id}")) {
            Resume::ById(resume_words)
        } else {
            Resume::Latest(resume_words)
        };
        if !resume.is_well_formed() {
            return Err(CustomError::BadResume);
        }
        // Only a plain program name can be looked for on a machine; a path is
        // the shell's to run.
        let plain = command
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c));
        Ok(AgentSpec {
            id,
            name: name.to_owned(),
            tag: name.to_uppercase(),
            command: command.to_owned(),
            args,
            resume,
            detect: if plain {
                vec![command.to_owned()]
            } else {
                Vec::new()
            },
            history: None,
            usage: None,
            mark: None,
            tint: Tint::Neutral,
            docs: None,
            custom: true,
            exit: None,
            rename: None,
            headless: None,
            prompt,
            config_env: None,
        })
    }
}

/// The custom agents the settings hold (one JSON object each), as specs. An
/// entry that is not understood or not valid is left out.
pub fn custom_from_entries(entries: &[String]) -> Vec<AgentSpec> {
    let mut specs: Vec<AgentSpec> = Vec::new();
    for entry in entries {
        let Ok(agent) = serde_json::from_str::<CustomAgent>(entry) else {
            continue;
        };
        let others: Vec<&str> = builtin()
            .iter()
            .map(|spec| spec.name.as_str())
            .chain(specs.iter().map(|spec| spec.name.as_str()))
            .collect();
        let taken = specs.iter().any(|spec| spec.id.as_str() == agent.id);
        if taken || builtin().iter().any(|spec| spec.id.as_str() == agent.id) {
            continue;
        }
        if let Ok(spec) = agent.to_spec(&others) {
            specs.push(spec);
        }
    }
    specs
}

/// The markdown list of the built-in agents, as the README has it: one row
/// per agent with its command, how a session is resumed, what Leon imports
/// of its history and reads of its limits.
pub fn render_markdown() -> String {
    let mut out = String::from(
        "| Agent | Command | Resume | History | Usage limits |\n| --- | --- | --- | --- | --- |\n",
    );
    for spec in builtin() {
        let name = match &spec.docs {
            Some(url) => format!("[{}]({url})", spec.name),
            None => spec.name.clone(),
        };
        let command = std::iter::once(spec.command.as_str())
            .chain(spec.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        let resume = match &spec.resume {
            Resume::None => "launch only".to_owned(),
            Resume::ById(args) => format!("`{} {}`", spec.command, args.join(" ")),
            Resume::Latest(args) => format!("`{} {}` (latest)", spec.command, args.join(" ")),
        };
        let history = match spec.history {
            Some(_) => "imported",
            None => "not yet",
        };
        let usage = match spec.usage {
            Some(UsageProvider::Claude | UsageProvider::Codex) => "yes",
            Some(_) => "yes, unverified",
            None => "no",
        };
        out.push_str(&format!(
            "| {name} | `{command}` | {resume} | {history} | {usage} |\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_every_spec_is_complete() {
        let mut seen = HashSet::new();
        for spec in builtin() {
            assert!(seen.insert(spec.id), "{} is repeated", spec.id);
            assert!(valid_id(spec.id.as_str()), "{} is not an id", spec.id);
            assert!(!spec.name.trim().is_empty());
            assert!(!spec.command.trim().is_empty());
            assert!(!spec.detect.is_empty(), "{} cannot be detected", spec.id);
            assert!(spec.detect.contains(&spec.command), "{}", spec.id);
            assert!(spec
                .docs
                .as_deref()
                .is_some_and(|d| d.starts_with("https://")));
            assert!(spec.resume.is_well_formed(), "{} resume form", spec.id);
            assert!(!spec.custom);
            for name in &spec.detect {
                assert!(
                    name.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c)),
                    "{name} is not safe for a shell word"
                );
            }
        }
        let names: HashSet<String> = builtin().iter().map(|s| s.name.to_lowercase()).collect();
        assert_eq!(names.len(), builtin().len(), "two agents share a name");
    }

    #[test]
    fn the_catalogue_has_every_agent_of_orcas_list() {
        for id in [
            "claude",
            "codex",
            "grok",
            "cursor",
            "copilot",
            "muse",
            "dsh",
            "zcode",
            "opencode",
            "mimo-code",
            "amp",
            "openclaude",
            "antigravity",
            "pi",
            "omp",
            "hermes",
            "devin",
            "goose",
            "auggie",
            "autohand",
            "crush",
            "cline",
            "codebuddy",
            "codebuff",
            "freebuff",
            "command-code",
            "continue",
            "droid",
            "kilo",
            "kimi",
            "kiro",
            "mistral-vibe",
            "qwen-code",
            "rovo",
        ] {
            assert!(AgentId::parse(id).and_then(AgentId::spec).is_some(), "{id}");
        }
        assert!(builtin().len() >= 35);
    }

    #[test]
    fn the_three_original_agents_keep_their_tags_commands_and_resume_forms() {
        let get = |id: AgentId| id.spec().unwrap();
        assert_eq!(AgentId::CLAUDE.as_str(), "claude");
        assert_eq!(get(AgentId::CLAUDE).launch(None).unwrap().0, "claude");
        assert_eq!(
            get(AgentId::CLAUDE).launch(Some("abc")).unwrap().1,
            ["--resume", "abc"]
        );
        assert_eq!(
            get(AgentId::CODEX).launch(Some("abc")).unwrap().1,
            ["resume", "abc"]
        );
        assert_eq!(
            get(AgentId::OPENCODE).launch(Some("abc")).unwrap().1,
            ["--session", "abc"]
        );
        assert_eq!(get(AgentId::CLAUDE).history, Some(Importer::Claude));
        assert_eq!(get(AgentId::CODEX).usage, Some(UsageProvider::Codex));
        assert_eq!(get(AgentId::OPENCODE).tint, Tint::Opencode);
    }

    #[test]
    fn a_program_name_finds_its_agent() {
        assert_eq!(by_program("claude"), Some(AgentId::CLAUDE));
        assert_eq!(by_program("/usr/bin/opencode"), Some(AgentId::OPENCODE));
        assert_eq!(by_program("cat"), None);
        assert_eq!(by_program(""), None);
    }

    #[test]
    fn a_launch_only_agent_cannot_be_asked_to_resume() {
        let amp = AgentId::parse("amp").unwrap().spec().unwrap();
        assert!(!amp.can_resume());
        assert_eq!(amp.launch(None), Some(("amp".into(), vec![])));
        assert_eq!(amp.launch(Some("x")), None);
        let kiro = AgentId::parse("kiro").unwrap().spec().unwrap();
        assert_eq!(kiro.launch(None).unwrap().1, ["chat", "--tui"]);
    }

    #[test]
    fn a_joined_resume_form_replaces_the_id_inside_the_word() {
        let copilot = AgentId::parse("copilot").unwrap().spec().unwrap();
        assert_eq!(copilot.launch(Some("s1")).unwrap().1, ["--resume=s1"]);
    }

    #[test]
    fn stored_tags_of_the_old_enum_are_ids_and_load_from_json() {
        for (tag, id) in [
            ("claude", AgentId::CLAUDE),
            ("codex", AgentId::CODEX),
            ("opencode", AgentId::OPENCODE),
        ] {
            assert_eq!(AgentId::parse(tag), Some(id));
            let json = format!("\"{tag}\"");
            assert_eq!(serde_json::from_str::<AgentId>(&json).unwrap(), id);
            assert_eq!(serde_json::to_string(&id).unwrap(), json);
        }
        assert!(serde_json::from_str::<AgentId>("\"Not An Id\"").is_err());
        assert!(AgentId::parse("").is_none());
        assert!(AgentId::parse("has space").is_none());
    }

    #[test]
    fn an_id_that_is_not_in_the_catalogue_is_kept_and_named_by_its_tag() {
        let id = AgentId::parse("removed-agent-x").unwrap();
        assert!(id.spec().is_none());
        assert_eq!(id.name(), "removed-agent-x");
        assert_eq!(AgentId::parse("removed-agent-x"), Some(id));
    }

    #[test]
    fn initials_make_a_two_letter_mark() {
        assert_eq!(initials("Mistral Vibe"), "MV");
        assert_eq!(initials("Cline"), "CL");
        assert_eq!(initials("oh-my-pi"), "OM");
        assert_eq!(initials("  "), "?");
    }

    #[test]
    fn a_custom_agent_is_validated() {
        let ok = CustomAgent::new("My Tool", "mytool", "--fast", "--resume {id}", &[]).unwrap();
        assert_eq!(ok.id, "custom-my-tool");
        let spec = ok.to_spec(&[]).unwrap();
        assert!(spec.custom);
        assert_eq!(spec.args, ["--fast"]);
        assert_eq!(
            spec.resume,
            Resume::ById(vec!["--resume".into(), "{id}".into()])
        );
        assert_eq!(spec.detect, ["mytool"]);
        assert_eq!(spec.launch(Some("9")).unwrap().1, ["--resume", "9"]);

        let latest = CustomAgent::new("Other", "o", "", "--continue", &[]).unwrap();
        assert_eq!(
            latest.to_spec(&[]).unwrap().resume,
            Resume::Latest(vec!["--continue".into()])
        );
        let none = CustomAgent::new("Plain", "/opt/bin/plain tool", "", "", &[]).unwrap();
        assert_eq!(none.to_spec(&[]).unwrap().resume, Resume::None);
        assert!(none.to_spec(&[]).unwrap().detect.is_empty());
    }

    #[test]
    fn a_custom_agent_that_is_not_valid_is_refused_with_a_reason() {
        let bad = |name, command, args, resume| CustomAgent::new(name, command, args, resume, &[]);
        assert_eq!(bad(" ", "x", "", ""), Err(CustomError::NoName));
        assert_eq!(bad("X", " ", "", ""), Err(CustomError::NoCommand));
        assert_eq!(bad("X", "a\nb", "", ""), Err(CustomError::BadCommand));
        assert_eq!(
            bad("X", "x", "", "--resume {id} {x}"),
            Err(CustomError::BadResume)
        );
        assert_eq!(bad("X", "x", "", "{id"), Err(CustomError::BadResume));
        assert_eq!(bad("X", "x", "'open", ""), Err(CustomError::BadArgs));
        assert_eq!(bad(&"n".repeat(41), "x", "", ""), Err(CustomError::BadName));
        let taken = CustomAgent::new("x", "x", "", "", &[]).unwrap();
        assert_eq!(taken.to_spec(&["X"]), Err(CustomError::NameTaken));
        assert_eq!(
            CustomAgent::new("Claude Code", "x", "", "", &[])
                .unwrap()
                .to_spec(&["claude code"]),
            Err(CustomError::NameTaken)
        );
    }

    #[test]
    fn ids_of_custom_agents_do_not_collide() {
        let first = CustomAgent::new("Tool", "tool", "", "", &[]).unwrap();
        let taken = [AgentId::parse(&first.id).unwrap()];
        let second = CustomAgent::new("Tool", "tool", "", "", &taken).unwrap();
        assert_eq!(second.id, "custom-tool-2");
    }

    #[test]
    fn entries_of_the_settings_become_specs_and_bad_ones_are_dropped() {
        let good = serde_json::to_string(
            &CustomAgent::new("Entry Tool", "entrytool", "", "--resume {id}", &[]).unwrap(),
        )
        .unwrap();
        let clash = r#"{"id":"claude","name":"Claude","command":"x"}"#.to_owned();
        let same_name = r#"{"id":"custom-z","name":"Grok","command":"x"}"#.to_owned();
        let specs = custom_from_entries(&[good, clash, same_name, "not json".into()]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id.as_str(), "custom-entry-tool");
    }

    #[test]
    fn a_registered_custom_agent_is_found_by_its_id() {
        let spec = CustomAgent::new("Registered Tool", "regtool", "", "", &[])
            .unwrap()
            .to_spec(&[])
            .unwrap();
        let id = spec.id;
        register_custom(spec);
        assert_eq!(id.name(), "Registered Tool");
        assert!(all().iter().any(|s| s.id == id));
        assert!(builtin().iter().all(|s| s.id != id));
    }

    #[test]
    fn the_readme_list_is_the_catalogue() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md");
        let text = std::fs::read_to_string(&path).unwrap();
        let begin = "<!-- agents:begin (generated by `render_markdown` in leon-core) -->\n";
        let end = "<!-- agents:end -->";
        let start = text.find(begin).expect("README.md has the agents block") + begin.len();
        let stop = text.find(end).expect("README.md ends the agents block");
        let fresh = format!("\n{}\n", render_markdown());
        if std::env::var_os("LEON_BLESS").is_some() && text[start..stop] != fresh {
            let updated = format!("{}{fresh}{}", &text[..start], &text[stop..]);
            std::fs::write(&path, updated).unwrap();
            return;
        }
        assert_eq!(
            &text[start..stop],
            fresh,
            "README.md is stale: run `LEON_BLESS=1 cargo test -p leon-core the_readme_list_is_the_catalogue`"
        );
    }

    #[test]
    fn only_the_agents_whose_exit_command_was_verified_have_one() {
        let with: Vec<&str> = builtin()
            .iter()
            .filter(|spec| spec.exit.is_some())
            .map(|spec| spec.id.as_str())
            .collect();
        assert_eq!(with, ["claude", "opencode"]);
        assert!(builtin()
            .iter()
            .filter_map(|spec| spec.exit.as_deref())
            .all(|line| line == "/exit"));
    }

    #[test]
    fn only_claude_code_renames_inside_the_agent() {
        let with: Vec<&str> = builtin()
            .iter()
            .filter(|spec| spec.rename.is_some())
            .map(|spec| spec.id.as_str())
            .collect();
        assert_eq!(with, ["claude"]);
        let claude = builtin().iter().find(|spec| spec.id == AgentId::CLAUDE);
        assert_eq!(
            claude
                .unwrap()
                .rename_line("  build   the thing ")
                .as_deref(),
            Some("/rename build the thing")
        );
        assert_eq!(claude.unwrap().rename_line("   "), None);
    }

    #[test]
    fn only_the_agents_whose_headless_form_was_verified_have_one() {
        let with: Vec<(&str, Vec<&str>)> = builtin()
            .iter()
            .filter_map(|spec| {
                let args = spec.headless.as_ref()?;
                Some((spec.id.as_str(), args.iter().map(String::as_str).collect()))
            })
            .collect();
        assert_eq!(
            with,
            [
                (
                    "claude",
                    vec![
                        "--tools",
                        "",
                        "-p",
                        "--permission-mode",
                        "dontAsk",
                        "--no-session-persistence"
                    ]
                ),
                ("codex", vec!["exec", "--sandbox", "read-only"]),
            ]
        );
        assert!(
            custom_specs().iter().all(|spec| spec.headless.is_none()),
            "a custom agent has no headless form"
        );
    }

    #[test]
    fn only_the_agents_whose_prompt_form_was_verified_have_one() {
        let with: Vec<(&str, Vec<&str>)> = builtin()
            .iter()
            .filter_map(|spec| {
                spec.prompt.as_ref().map(|form| {
                    (
                        spec.id.as_str(),
                        form.iter().map(String::as_str).collect::<Vec<_>>(),
                    )
                })
            })
            .collect();
        assert_eq!(
            with,
            [
                ("claude", vec!["{prompt}"]),
                ("codex", vec!["{prompt}"]),
                ("opencode", vec!["--prompt", "{prompt}"]),
                ("grok", vec!["{prompt}"]),
                ("cursor", vec!["{prompt}"]),
                ("gemini", vec!["--prompt-interactive", "{prompt}"]),
            ]
        );
        assert!(builtin()
            .iter()
            .filter_map(|spec| spec.prompt.as_ref())
            .all(|form| prompt_form_is_well_formed(form)));
    }

    #[test]
    fn a_custom_agent_declares_how_it_takes_a_prompt() {
        let agent = CustomAgent::new("Prompted", "prompted", "", "", &[])
            .unwrap()
            .with_prompt_args("--ask {prompt}")
            .unwrap();
        let spec = agent.to_spec(&[]).unwrap();
        assert_eq!(spec.prompt.unwrap(), ["--ask", "{prompt}"]);
        let bare = CustomAgent::new("Bare", "bare", "", "", &[]).unwrap();
        assert_eq!(bare.to_spec(&[]).unwrap().prompt, None);
    }

    #[test]
    fn a_prompt_form_that_is_not_well_formed_is_refused() {
        let agent = || CustomAgent::new("Odd", "odd", "", "", &[]).unwrap();
        for form in [
            "--ask",
            "{prompt} {prompt}",
            "--ask={prompt}",
            "{prompt} {id}",
        ] {
            assert_eq!(
                agent().with_prompt_args(form),
                Err(CustomError::BadPrompt),
                "{form}"
            );
        }
        assert_eq!(
            agent().with_prompt_args("'open {prompt}"),
            Err(CustomError::BadArgs)
        );
    }

    #[test]
    fn an_entry_saved_before_prompts_existed_still_loads() {
        let old = r#"{"id":"custom-old","name":"Old","command":"old","args":"","resume_args":""}"#;
        let specs = custom_from_entries(&[old.to_owned()]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].prompt, None);
    }
}

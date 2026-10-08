//! What the host tells the Den: who is in it and what just happened.
//!
//! All of it is plain data. A [`Cub`] is one live agent session as the host
//! knows it at this moment; the host hands the whole list on every change
//! ([`crate::Den::update`]). A [`Happening`] is one event, for the narrator
//! ([`crate::Den::happen`]). The Den never asks the host for anything.

use gpui_kit::Hsla;

/// What tells one kind of agent from another, and one cub from the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Species {
    /// The colour of the mane: the agent's colour in the theme.
    pub tint: Hsla,
    /// Picks the small differences of the individual: a number that stays the
    /// same for a session, such as a hash of its name.
    pub seed: u64,
}

/// What a session is doing, as far as the host knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CubState {
    /// Writing or editing a file.
    Editing,
    /// Reading a file.
    Reading,
    /// Searching the tree.
    Searching,
    /// Running a command.
    Running,
    /// Fetching from the web.
    Web,
    /// Waiting for a sub-agent it sent out.
    Delegating,
    /// Writing a plan.
    Planning,
    /// Using a tool that is none of the above: an MCP tool, say.
    UsingTool,
    /// Working with no tool in hand: the model is talking.
    Thinking,
    /// Working, and nothing more is known.
    Mystery,
    /// The turn ended: it waits for the user.
    WaitingForUser,
    /// It asks for a permission.
    NeedsPermission,
    /// A shell is in front and nothing runs.
    Idle,
    /// Nothing has happened for a long time.
    Asleep,
    /// The session exited with an error.
    Fainted,
    /// The session exited cleanly.
    Gone,
}

impl CubState {
    /// Every state, in the order of the enum.
    pub const ALL: [CubState; 16] = [
        CubState::Editing,
        CubState::Reading,
        CubState::Searching,
        CubState::Running,
        CubState::Web,
        CubState::Delegating,
        CubState::Planning,
        CubState::UsingTool,
        CubState::Thinking,
        CubState::Mystery,
        CubState::WaitingForUser,
        CubState::NeedsPermission,
        CubState::Idle,
        CubState::Asleep,
        CubState::Fainted,
        CubState::Gone,
    ];

    /// The state in plain words, for the truth card. No joke here.
    pub fn label(self) -> &'static str {
        match self {
            CubState::Editing => "Editing a file",
            CubState::Reading => "Reading a file",
            CubState::Searching => "Searching",
            CubState::Running => "Running a command",
            CubState::Web => "Fetching from the web",
            CubState::Delegating => "Waiting for a sub-agent",
            CubState::Planning => "Planning",
            CubState::UsingTool => "Using a tool",
            CubState::Thinking => "Thinking",
            CubState::Mystery => "Working (no details available)",
            CubState::WaitingForUser => "Waiting for you",
            CubState::NeedsPermission => "Needs your permission",
            CubState::Idle => "Idle",
            CubState::Asleep => "Asleep",
            CubState::Fainted => "Exited with an error",
            CubState::Gone => "Exited",
        }
    }

    /// The state in at most five letters, for the roster.
    pub fn tag(self) -> &'static str {
        match self {
            CubState::Editing => "EDIT",
            CubState::Reading => "READ",
            CubState::Searching => "SEEK",
            CubState::Running => "RUN",
            CubState::Web => "WEB",
            CubState::Delegating => "EGG",
            CubState::Planning => "PLAN",
            CubState::UsingTool => "TOOL",
            CubState::Thinking => "THINK",
            CubState::Mystery => "???",
            CubState::WaitingForUser => "DONE",
            CubState::NeedsPermission => "ASKS",
            CubState::Idle => "SUN",
            CubState::Asleep => "ZZZ",
            CubState::Fainted => "FNT",
            CubState::Gone => "HOME",
        }
    }

    /// Whether the agent is at work.
    pub fn is_working(self) -> bool {
        matches!(
            self,
            CubState::Editing
                | CubState::Reading
                | CubState::Searching
                | CubState::Running
                | CubState::Web
                | CubState::Delegating
                | CubState::Planning
                | CubState::UsingTool
                | CubState::Thinking
                | CubState::Mystery
        )
    }

    /// How pressing it is that the user looks at a session in this state:
    /// `None` when it does not need them, else a rank, the lower the more
    /// pressing. A permission prompt holds a turn up; a session that waits
    /// has finished or asks; one that fainted is over and can only be read.
    pub fn needs_user(self) -> Option<u8> {
        match self {
            CubState::NeedsPermission => Some(0),
            CubState::WaitingForUser => Some(1),
            CubState::Fainted => Some(2),
            _ => None,
        }
    }

    /// The status colour the state carries in the rest of the app, if any.
    pub fn status(self) -> Option<Status> {
        match self {
            CubState::WaitingForUser => Some(Status::Attention),
            CubState::NeedsPermission => Some(Status::Warning),
            CubState::Fainted => Some(Status::Error),
            CubState::Mystery => Some(Status::Info),
            _ => None,
        }
    }
}

/// A status that has a colour and a glyph of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// The accent: the user's turn.
    Attention,
    /// A command passed.
    Success,
    /// A permission prompt.
    Warning,
    /// A failure.
    Error,
    /// Nothing is known.
    Info,
}

/// One live agent session.
#[derive(Clone, Debug, PartialEq)]
pub struct Cub {
    /// The session, by a number that never changes while it lives.
    pub id: u64,
    /// The name of the session. The narrator writes it in capitals.
    pub name: String,
    /// Its agent and what makes it an individual.
    pub species: Species,
    /// What it is doing.
    pub state: CubState,
    /// How many tools it has used so far: its `Lv.`.
    pub level: u32,
    /// The plain fact: "Editing shell.rs", the command as typed. Shown as it
    /// is on the truth card.
    pub detail: Option<String>,
    /// The cub that sent this one out, for a sub-agent: a little one.
    pub parent: Option<u64>,
    /// No transcript: only working or quiet is known of it.
    pub mystery: bool,
}

/// What a tool does, as far as the narrator cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    /// Writes or edits a file.
    Edit,
    /// Reads a file.
    Read,
    /// Searches.
    Search,
    /// Runs a command.
    Run,
    /// Fetches from the web.
    Web,
    /// Writes a plan.
    Plan,
    /// Anything else: it is named as it is.
    Other,
}

/// What happened to a cub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A session appeared.
    Joined,
    /// A tool started. `tool` is its name as the agent calls it; `detail` is
    /// its subject: the file, the pattern, the command line, the address.
    ToolStarted {
        /// What kind of tool.
        kind: ToolKind,
        /// Its name.
        tool: String,
        /// Its subject.
        detail: Option<String>,
    },
    /// A tool ended.
    ToolFinished {
        /// What kind of tool.
        kind: ToolKind,
        /// Whether it really passed.
        ok: bool,
    },
    /// The cub sent a sub-agent out.
    SentOut {
        /// The name of the sub-agent.
        little: String,
    },
    /// A sub-agent of the cub came back.
    CameBack {
        /// The name of the sub-agent.
        little: String,
    },
    /// A permission prompt is on screen.
    PermissionPrompt,
    /// The turn ended.
    TurnEnded,
    /// A session with no transcript began to work: that is all that is known.
    Mysterious,
    /// It has been quiet for a long while.
    FellAsleep,
    /// The session exited with an error.
    Fainted {
        /// Its exit code, if there is one.
        exit: Option<i32>,
    },
    /// The session exited cleanly.
    WentHome,
}

/// One event of one cub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Happening {
    /// The cub it happened to.
    pub cub: u64,
    /// The cub's name, as the session is called.
    pub name: String,
    /// What happened.
    pub event: Event,
}

impl Happening {
    /// An event of the cub with this id and name.
    pub fn new(cub: u64, name: impl Into<String>, event: Event) -> Self {
        Self {
            cub,
            name: name.into(),
            event,
        }
    }
}

/// The happenings that can be told from two lists of cubs alone: who joined,
/// who went home or fainted, who fell asleep, whose turn ended, who asks for
/// a permission, and which mystery session began to work. A host that has a
/// real stream of events for a session should send those instead: they know
/// the tool and the exit code. This is for the sessions it knows less about.
pub fn happenings_between(before: &[Cub], after: &[Cub]) -> Vec<Happening> {
    let mut out = Vec::new();
    for cub in after {
        let old = before.iter().find(|old| old.id == cub.id);
        let was = old.map(|old| old.state);
        if was == Some(cub.state) {
            continue;
        }
        let event = match cub.state {
            CubState::Gone => Some(Event::WentHome),
            CubState::Fainted => Some(Event::Fainted { exit: None }),
            CubState::Asleep if was.is_some() => Some(Event::FellAsleep),
            CubState::WaitingForUser if was.is_some_and(CubState::is_working) => {
                Some(Event::TurnEnded)
            }
            CubState::NeedsPermission => Some(Event::PermissionPrompt),
            CubState::Mystery if !was.is_some_and(CubState::is_working) => Some(Event::Mysterious),
            _ if was.is_none() => Some(Event::Joined),
            _ => None,
        };
        if was.is_none() && !matches!(event, Some(Event::Joined)) && cub.state != CubState::Gone {
            out.push(Happening::new(cub.id, cub.name.clone(), Event::Joined));
        }
        if let Some(event) = event {
            out.push(Happening::new(cub.id, cub.name.clone(), event));
        }
    }
    for old in before {
        let left = !after.iter().any(|cub| cub.id == old.id);
        if left && !matches!(old.state, CubState::Gone | CubState::Fainted) {
            out.push(Happening::new(old.id, old.name.clone(), Event::WentHome));
        }
    }
    out
}

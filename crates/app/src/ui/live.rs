//! Live sessions: the terminals that are running, kept alive in the
//! background while another node is selected.
//!
//! [`Sessions`] is the registry the shell owns. Each [`LiveSession`] holds
//! the terminal's view entity, so switching to another node and back never
//! restarts a process: the view is simply not drawn meanwhile (its reader
//! threads keep the grid current). A session goes through three states, read
//! from the terminal itself: starting (nothing printed yet), running, and
//! exited with a code.

use super::activity::{terminal_activity, Activity, Signals, Thresholds};
use gpui_kit::{App, Entity, Subscription};
use leon_core::{AgentId, MachineId, SessionId};
use leon_term::TerminalView;

/// The identity of a live session, for as long as the application runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LiveId(pub u64);

impl std::fmt::Display for LiveId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// How a live session is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveState {
    /// Started, nothing printed yet.
    Starting,
    /// Running.
    Running,
    /// The program ended with this exit code.
    Exited(u32),
}

impl LiveState {
    /// The state of a terminal from what it reports.
    pub fn of(exit_code: Option<u32>, has_output: bool) -> Self {
        match (exit_code, has_output) {
            (Some(code), _) => Self::Exited(code),
            (None, true) => Self::Running,
            (None, false) => Self::Starting,
        }
    }

    /// The state in a mono label.
    pub fn label(self) -> String {
        match self {
            Self::Starting => "STARTING".to_owned(),
            Self::Running => "RUNNING".to_owned(),
            Self::Exited(code) => format!("EXIT {code}"),
        }
    }
}

/// What is known of the agent that was started in a session's shell.
///
/// Only a terminal on this computer can tell: there the shell is the
/// terminal's own process, and whether it is in front of the terminal (the
/// foreground process group) says whether a program runs in it. Over SSH the
/// terminal's process is `ssh` whatever happens on the other side, so nothing
/// is claimed: the phase stays `Launched`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentPhase {
    /// A plain shell: no agent was asked for.
    Shell,
    /// The agent's command line was typed into the shell; whether it is
    /// still running is not known (yet).
    Launched,
    /// The agent was seen running in front of the shell.
    Running,
    /// The agent was seen running and then the shell got the terminal back:
    /// the agent ended or was quit, and the terminal is at the prompt.
    Returned,
}

impl AgentPhase {
    /// The next phase, given whether the shell is in front of the terminal
    /// (`None` where that cannot be told). Only the sequence launched,
    /// running, returned is ever claimed.
    pub fn observe(self, shell_in_front: Option<bool>) -> Self {
        match (self, shell_in_front) {
            (Self::Launched, Some(false)) => Self::Running,
            (Self::Running, Some(true)) => Self::Returned,
            (phase, _) => phase,
        }
    }
}

/// What a live session is.
pub struct LiveSession {
    /// Its identity.
    pub id: LiveId,
    /// The machine it runs on.
    pub machine: MachineId,
    /// That machine's name.
    pub machine_name: String,
    /// The folder it runs in.
    pub cwd: String,
    /// The agent, or `None` for a plain shell.
    pub agent: Option<AgentId>,
    /// The agent's own id of the history session it resumed.
    pub resumed: Option<String>,
    /// The history session it was started from, as the store knows it: while
    /// the terminal lives, that session's row is this terminal's row and
    /// opening the session again focuses it.
    pub history: Option<SessionId>,
    /// The title the program set, if it did.
    pub title: Option<String>,
    /// A name the user gave it, over the title.
    pub name: Option<String>,
    /// What is known of the agent started in its shell.
    pub phase: AgentPhase,
    /// Whether the terminal can tell the shell from a program in it.
    pub can_detect: bool,
    /// The terminal.
    pub view: Entity<TerminalView>,
    /// The state last shown, to tell when the sidebar must change.
    pub shown: LiveState,
    /// Whether the bell rang since the session was last looked at.
    pub bell: bool,
    /// Whether the user has looked at the session since it failed.
    pub failure_seen: bool,
    /// How it is doing, as the worktree's dot reads it; kept up to date by
    /// terminal wake-ups and the coarse timer.
    pub activity: Activity,
    /// The line that resumes the agent, held back until the session is shown
    /// or the person asks: a restored session is "paused" while it is set.
    pub pending: Option<String>,
    /// The agent's own session id and how sure Leon is of it: `resumed` when
    /// it was started with it, `state-file` or `newest-in-folder` when it was
    /// learned afterwards.
    pub learned: Option<(String, String)>,
    /// When it was started, in milliseconds since the epoch.
    pub started_ms: i64,
    /// What keeps the shell told about the terminal.
    pub _subscriptions: Vec<Subscription>,
}

impl LiveSession {
    /// The size of one cell of its terminal in pixels, 0 before the first
    /// paint measured it.
    pub fn cell_size(&self, cx: &App) -> (f32, f32) {
        let size = self.view.read(cx).terminal().size();
        (f32::from(size.cell_width), f32::from(size.cell_height))
    }

    /// The session's state, read from its terminal.
    pub fn state(&self, cx: &App) -> LiveState {
        let terminal = self.view.read(cx).terminal();
        LiveState::of(
            terminal.exit_info().map(|info| info.code),
            terminal.has_output(),
        )
    }

    /// What the terminal says about itself, for [`terminal_activity`].
    pub fn signals(&self, cx: &App) -> Signals {
        let terminal = self.view.read(cx).terminal();
        Signals {
            exited: terminal.exit_info().map(|info| info.code),
            failure_seen: self.failure_seen,
            shell_in_front: self
                .can_detect
                .then(|| terminal.shell_is_foreground())
                .flatten(),
            // A restored session that is still paused has no agent running.
            agent: self.agent.is_some() && !self.is_paused(),
            quiet_for: terminal.quiet_for(),
            bell: self.bell,
        }
    }

    /// How it is doing now.
    pub fn read_activity(&self, cx: &App, thresholds: &Thresholds) -> Activity {
        terminal_activity(&self.signals(cx), thresholds)
    }

    /// Whether the session waits for the person (or its tab) before its agent
    /// is resumed.
    pub fn is_paused(&self) -> bool {
        self.pending.is_some()
    }

    /// Takes the agent seen in front of the shell as the session's own, when
    /// it is not the one shown: the user quit one agent and started another
    /// by hand. What belonged to the old agent (its session id, the history
    /// row, the title it set) is dropped; a name the user gave stays.
    /// `true` when the session changed.
    pub fn follow_foreground(&mut self, program: Option<&str>) -> bool {
        let Some(seen) = program.and_then(leon_core::agent::by_program) else {
            return false;
        };
        if self.shown_agent() == Some(seen) {
            return false;
        }
        if self.agent != Some(seen) {
            self.resumed = None;
            self.learned = None;
            self.history = None;
            self.title = None;
        }
        self.agent = Some(seen);
        self.phase = AgentPhase::Running;
        true
    }

    /// The agent to show for this session: the one that was started in its
    /// shell, until the shell is seen to have the terminal back.
    pub fn shown_agent(&self) -> Option<AgentId> {
        match self.phase {
            AgentPhase::Shell | AgentPhase::Returned => None,
            AgentPhase::Launched | AgentPhase::Running => self.agent,
        }
    }

    /// What the sidebar calls it: the name the user gave it, else the title
    /// the program set, else the agent's name while it was started as one,
    /// else "Shell".
    pub fn label(&self) -> String {
        if let Some(name) = self.name.as_deref().filter(|name| !name.trim().is_empty()) {
            return name.to_owned();
        }
        match (&self.title, self.shown_agent()) {
            // The agent's logo is shown beside the title: its own glyph is not.
            (Some(title), Some(_)) if !title.trim().is_empty() => crate::format::plain_title(title),
            (Some(title), None) if !title.trim().is_empty() => title.clone(),
            (_, Some(agent)) => crate::format::agent_name(agent).to_owned(),
            (_, None) => "Shell".to_owned(),
        }
    }

    /// Whether closing it would end a program that is running in it: asked
    /// of the terminal where it can tell, assumed when it cannot.
    pub fn busy(&self, cx: &App) -> bool {
        let terminal = self.view.read(cx).terminal();
        if terminal.exit_info().is_some() {
            return false;
        }
        match self
            .can_detect
            .then(|| terminal.shell_is_foreground())
            .flatten()
        {
            Some(in_front) => !in_front,
            None => true,
        }
    }
}

/// Every live session, in the order they were started.
#[derive(Default)]
pub struct Sessions {
    items: Vec<LiveSession>,
    next: u64,
}

impl Sessions {
    /// The id the next session gets.
    pub fn next_id(&mut self) -> LiveId {
        self.next += 1;
        LiveId(self.next)
    }

    /// Adds a session.
    pub fn push(&mut self, session: LiveSession) {
        self.items.push(session);
    }

    /// The sessions, oldest first.
    pub fn all(&self) -> &[LiveSession] {
        &self.items
    }

    /// The session with this id.
    pub fn get(&self, id: LiveId) -> Option<&LiveSession> {
        self.items.iter().find(|session| session.id == id)
    }

    /// The live session that was started from this history session.
    pub fn of_history(&self, history: &SessionId) -> Option<&LiveSession> {
        self.items
            .iter()
            .find(|session| session.history.as_ref() == Some(history))
    }

    /// Mutable access to the session with this id.
    pub fn get_mut(&mut self, id: LiveId) -> Option<&mut LiveSession> {
        self.items.iter_mut().find(|session| session.id == id)
    }

    /// Removes a session, dropping its terminal (which ends its program).
    pub fn remove(&mut self, id: LiveId) -> Option<LiveSession> {
        let at = self.items.iter().position(|session| session.id == id)?;
        Some(self.items.remove(at))
    }

    /// The ids, oldest first.
    pub fn ids(&self) -> Vec<LiveId> {
        self.items.iter().map(|session| session.id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_comes_from_the_exit_and_the_output() {
        assert_eq!(LiveState::of(None, false), LiveState::Starting);
        assert_eq!(LiveState::of(None, true), LiveState::Running);
        assert_eq!(LiveState::of(Some(0), true), LiveState::Exited(0));
        assert_eq!(LiveState::of(Some(130), false), LiveState::Exited(130));
    }

    #[test]
    fn an_exited_session_says_its_code() {
        assert_eq!(LiveState::Exited(3).label(), "EXIT 3");
        assert_eq!(LiveState::Running.label(), "RUNNING");
    }

    #[test]
    fn the_agent_is_only_claimed_to_have_returned_after_it_was_seen_running() {
        use AgentPhase as P;
        // The shell in front straight after typing: nothing is claimed.
        assert_eq!(P::Launched.observe(Some(true)), P::Launched);
        assert_eq!(P::Launched.observe(Some(false)), P::Running);
        assert_eq!(P::Running.observe(Some(false)), P::Running);
        assert_eq!(P::Running.observe(Some(true)), P::Returned);
        // Where nothing can be told, nothing changes.
        assert_eq!(P::Launched.observe(None), P::Launched);
        assert_eq!(P::Running.observe(None), P::Running);
        // A shell stays a shell, and a returned agent is not revived by the
        // next command the user runs.
        assert_eq!(P::Shell.observe(Some(false)), P::Shell);
        assert_eq!(P::Returned.observe(Some(false)), P::Returned);
    }

    #[test]
    fn ids_are_never_reused() {
        let mut sessions = Sessions::default();
        let a = sessions.next_id();
        let b = sessions.next_id();
        assert_ne!(a, b);
    }
}

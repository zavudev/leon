//! Notifications: what Leon says when a session wants the user or finishes.
//!
//! The decision and the wording are pure: [`Event`] is what just happened and
//! [`note`] words it. A session that wants the user says which of two it is
//! when its transcript tells (it finished its turn, or it needs an answer)
//! and only that it waits when nothing does. The window draws the *geek banner* (the monospace one:
//! what the session is, the folder, the exit code) and the desktop
//! notification is handed to GPUI, which speaks to the system's notification
//! centre (notify-rust on Linux, the Notification Center on macOS, a toast on
//! Windows). Tests replace
//! [`Options::notify`](super::shell::Options::notify) with a recorder and read
//! the banners.

use super::activity::Activity;
use super::live::{LiveId, LiveSession};
use gpui_kit::{App, SystemNotification};
use std::path::Path;
use std::time::Instant;

/// What a session just did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A program in front of a terminal went quiet or rang its bell: it wants
    /// the user, and nothing tells whether it finished or asks. What every
    /// session without a followed transcript says.
    Waiting,
    /// An agent finished its turn, as its transcript says: your move.
    TurnOver,
    /// An agent asks something: a question, or a tool call without a result
    /// in a quiet terminal, which is probably a permission prompt (inferred,
    /// a transcript does not record the question).
    NeedsAnswer,
    /// A session's program ended cleanly.
    Finished {
        /// The exit code, always zero.
        code: u32,
    },
    /// A session's program ended with an error.
    Failed {
        /// The exit code.
        code: u32,
    },
}

impl Event {
    /// The event of a program that ended with `code`.
    pub fn of_exit(code: u32) -> Self {
        if code == 0 {
            Self::Finished { code }
        } else {
            Self::Failed { code }
        }
    }

    /// The event of a session that just came to read as `activity`, if that
    /// is one the user is told about.
    pub fn of_activity(activity: Activity) -> Option<Self> {
        match activity {
            Activity::Waiting => Some(Self::Waiting),
            Activity::TurnOver => Some(Self::TurnOver),
            Activity::NeedsYou => Some(Self::NeedsAnswer),
            _ => None,
        }
    }

    /// What happened, as one line.
    pub fn line(self) -> String {
        match self {
            Self::Waiting => "waiting for you · quiet, or the bell rang".to_owned(),
            Self::TurnOver => "finished its turn · your move".to_owned(),
            Self::NeedsAnswer => {
                "needs an answer · a question, or probably a permission prompt".to_owned()
            }
            Self::Finished { code } => format!("exited cleanly · code {code}"),
            Self::Failed { code } => format!("failed · code {code}"),
        }
    }

    /// How it reads at a glance.
    pub fn kind(self) -> Kind {
        match self {
            Self::Waiting => Kind::Waiting,
            Self::TurnOver => Kind::TurnOver,
            Self::NeedsAnswer => Kind::NeedsAnswer,
            Self::Finished { .. } => Kind::Finished,
            Self::Failed { .. } => Kind::Failed,
        }
    }
}

/// The kind of a note: what its marker is coloured by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// It wants the user, and nothing tells why.
    Waiting,
    /// An agent finished its turn.
    TurnOver,
    /// An agent asks something.
    NeedsAnswer,
    /// It ended cleanly.
    Finished,
    /// It ended with an error.
    Failed,
}

/// A notification, ready to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// What kind of thing happened.
    pub kind: Kind,
    /// The live session it came from, to open it when the banner is clicked.
    pub session: Option<LiveId>,
    /// Who and where: "Claude Code · feat-notifications".
    pub title: String,
    /// What happened: "finished its turn · your move · mac-mini".
    pub body: String,
}

/// The note of an event of a session.
pub fn note(event: Event, id: LiveId, session: &LiveSession) -> Note {
    Note {
        kind: event.kind(),
        session: Some(id),
        title: title(&session.label(), &session.cwd),
        body: body(event, &session.machine_name, session.machine.is_local()),
    }
}

/// The first line of a note: what the session is called and the folder it
/// runs in.
pub fn title(label: &str, cwd: &str) -> String {
    let folder = folder_name(cwd);
    if folder.is_empty() {
        label.to_owned()
    } else {
        format!("{label} · {folder}")
    }
}

/// The second line: what happened, and on which computer when it is not this
/// one.
pub fn body(event: Event, machine: &str, local: bool) -> String {
    let line = event.line();
    if local || machine.is_empty() {
        line
    } else {
        format!("{line} · {machine}")
    }
}

/// The last component of a folder path; the whole path when it has none.
fn folder_name(cwd: &str) -> &str {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(cwd)
}

/// What a conversation of Leon's own interface just did out of sight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatEvent {
    /// Its agent finished the turn: the person's move.
    Finished,
    /// Its agent asks something and waits; the words say what.
    Asks(String),
    /// The turn failed; the first line of why, when the agent says.
    Failed(String),
}

impl ChatEvent {
    /// The event of a terminal session it counts as, for the settings that
    /// say which events are told.
    pub fn event(&self) -> Event {
        match self {
            Self::Finished => Event::TurnOver,
            Self::Asks(_) => Event::NeedsAnswer,
            Self::Failed(_) => Event::Failed { code: 1 },
        }
    }

    /// What happened, as one line.
    pub fn line(&self) -> String {
        match self {
            Self::Finished => "finished its turn \u{b7} your move".to_owned(),
            Self::Asks(what) if what.is_empty() => "asks you something".to_owned(),
            Self::Asks(what) => format!("asks {what}"),
            Self::Failed(why) if why.is_empty() => "the turn failed".to_owned(),
            Self::Failed(why) => format!("the turn failed \u{b7} {why}"),
        }
    }
}

/// The note of a conversation: its name and folder, and what it did.
pub fn chat_note(
    event: &ChatEvent,
    id: LiveId,
    label: &str,
    cwd: &str,
    machine: &str,
    local: bool,
) -> Note {
    let line = event.line();
    Note {
        kind: event.event().kind(),
        session: Some(id),
        title: title(label, cwd),
        body: if local || machine.is_empty() {
            line
        } else {
            format!("{line} \u{b7} {machine}")
        },
    }
}

/// The conversation or terminal a notification of the centre was about, from
/// its [`tag`].
pub fn session_of_tag(tag: &str) -> Option<LiveId> {
    tag.strip_prefix("leon-session-")?.parse().ok().map(LiveId)
}

/// A notification on screen, with when it goes away.
pub struct Banner {
    /// Its identity, for dismissing exactly this one.
    pub id: u64,
    /// What it says.
    pub note: Note,
    /// When it is dropped from the window.
    pub until: Instant,
}

/// How a desktop notification is shown; tests record the notes instead.
pub type Notify = std::rc::Rc<dyn Fn(&Note, &mut App)>;

/// The stable identity of a note. A newer note of the same session replaces
/// the one before it in the notification centre.
pub fn tag(note: &Note) -> String {
    match note.session {
        Some(id) => format!("leon-session-{}", id.0),
        None => "leon".to_owned(),
    }
}

/// The system notification a note becomes.
pub fn system_notification(note: &Note) -> SystemNotification {
    SystemNotification {
        tag: tag(note).into(),
        title: note.title.clone().into(),
        body: note.body.clone().into(),
        actions: Vec::new(),
    }
}

/// Plays a short system sound, for a note the person asked to hear. Only on
/// macOS, where the system sounds are files; elsewhere nothing is played.
pub fn chime(_cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        let _ = leon_remote::spawn::std_child("afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .spawn();
    }
}

/// Shows a note in the notification centre of this computer. A desktop
/// without one ignores it: a notification is never worth an error.
pub fn system(note: &Note, cx: &mut App) {
    cx.show_system_notification(system_notification(note));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(kind: Kind) -> Note {
        Note {
            kind,
            session: Some(LiveId(7)),
            title: "Claude Code · leon".to_owned(),
            body: "failed · code 2".to_owned(),
        }
    }

    #[test]
    fn the_exit_code_decides_between_a_clean_end_and_a_failure() {
        assert_eq!(Event::of_exit(0), Event::Finished { code: 0 });
        assert_eq!(Event::of_exit(2), Event::Failed { code: 2 });
        assert_eq!(Event::of_exit(130), Event::Failed { code: 130 });
    }

    #[test]
    fn every_event_reads_as_a_line_and_a_kind() {
        assert_eq!(
            Event::Waiting.line(),
            "waiting for you · quiet, or the bell rang"
        );
        assert_eq!(Event::TurnOver.line(), "finished its turn · your move");
        assert_eq!(
            Event::NeedsAnswer.line(),
            "needs an answer · a question, or probably a permission prompt"
        );
        assert_eq!(
            Event::Finished { code: 0 }.line(),
            "exited cleanly · code 0"
        );
        assert_eq!(Event::Failed { code: 3 }.line(), "failed · code 3");
        assert_eq!(Event::Waiting.kind(), Kind::Waiting);
        assert_eq!(Event::TurnOver.kind(), Kind::TurnOver);
        assert_eq!(Event::NeedsAnswer.kind(), Kind::NeedsAnswer);
        assert_eq!(Event::Finished { code: 0 }.kind(), Kind::Finished);
        assert_eq!(Event::Failed { code: 1 }.kind(), Kind::Failed);
    }

    #[test]
    fn only_the_states_that_want_the_user_become_an_event_and_each_its_own() {
        use Activity::*;
        assert_eq!(Event::of_activity(Waiting), Some(Event::Waiting));
        assert_eq!(Event::of_activity(TurnOver), Some(Event::TurnOver));
        assert_eq!(Event::of_activity(NeedsYou), Some(Event::NeedsAnswer));
        for quiet in [Off, Idle, Working, Failed] {
            assert_eq!(Event::of_activity(quiet), None, "{quiet:?}");
        }
        // Finished and asking are told apart in words.
        assert!(Event::TurnOver.line().contains("finished"));
        assert!(Event::NeedsAnswer.line().contains("needs an answer"));
        assert!(!Event::Waiting.line().contains("finished"));
    }

    #[test]
    fn a_note_names_the_session_and_the_folder_it_runs_in() {
        assert_eq!(
            title("Claude Code", "/home/dev/code/leon"),
            "Claude Code · leon"
        );
        assert_eq!(title("Shell", "/home/dev"), "Shell · dev");
        assert_eq!(title("Shell", ""), "Shell");
        assert_eq!(title("Shell", "/"), "Shell · /");
    }

    #[test]
    fn a_remote_session_says_which_computer() {
        assert_eq!(
            body(Event::Waiting, "this computer", true),
            Event::Waiting.line()
        );
        assert_eq!(
            body(Event::Failed { code: 1 }, "mac-mini", false),
            "failed · code 1 · mac-mini"
        );
    }

    #[test]
    fn the_system_notification_carries_the_note_and_a_stable_tag() {
        let shown = system_notification(&note(Kind::Failed));
        assert_eq!(shown.tag, "leon-session-7");
        assert_eq!(shown.title, "Claude Code · leon");
        assert_eq!(shown.body, "failed · code 2");
        assert!(shown.actions.is_empty());
        // A note without a session still has a tag.
        let orphan = Note {
            session: None,
            ..note(Kind::Finished)
        };
        assert_eq!(system_notification(&orphan).tag, "leon");
    }

    #[test]
    fn a_conversations_note_says_what_it_did_in_its_own_words() {
        let finished = chat_note(
            &ChatEvent::Finished,
            LiveId(2),
            "Fix tabs",
            "/srv/leon",
            "",
            true,
        );
        assert_eq!(finished.kind, Kind::TurnOver);
        assert_eq!(finished.title, "Fix tabs \u{b7} leon");
        assert_eq!(finished.body, "finished its turn \u{b7} your move");
        let asks = chat_note(
            &ChatEvent::Asks("permission \u{b7} Bash: ls".into()),
            LiveId(2),
            "Fix tabs",
            "/srv/leon",
            "mac-mini",
            false,
        );
        assert_eq!(asks.kind, Kind::NeedsAnswer);
        assert_eq!(asks.body, "asks permission \u{b7} Bash: ls \u{b7} mac-mini");
        let failed = chat_note(
            &ChatEvent::Failed("boom".into()),
            LiveId(2),
            "x",
            "/",
            "",
            true,
        );
        assert_eq!(failed.kind, Kind::Failed);
        assert_eq!(failed.body, "the turn failed \u{b7} boom");
        assert_eq!(ChatEvent::Failed(String::new()).line(), "the turn failed");
    }

    #[test]
    fn the_tag_of_a_note_leads_back_to_its_session() {
        assert_eq!(session_of_tag(&tag(&note(Kind::Failed))), Some(LiveId(7)));
        assert_eq!(session_of_tag("leon"), None);
        assert_eq!(session_of_tag("leon-session-x"), None);
    }
}

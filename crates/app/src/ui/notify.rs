//! Notifications: what Leon says when a session wants the user or finishes.
//!
//! The decision and the wording are pure: [`Event`] is what just happened and
//! [`note`] words it. The window draws the *geek banner* (the monospace one:
//! what the session is, the folder, the exit code) and the desktop
//! notification is handed to GPUI, which speaks to the system's notification
//! centre (notify-rust on Linux, the Notification Center on macOS, a toast on
//! Windows). Tests replace
//! [`Options::notify`](super::shell::Options::notify) with a recorder and read
//! the banners.

use super::live::{LiveId, LiveSession};
use gpui_kit::{App, SystemNotification};
use std::path::Path;
use std::time::Instant;

/// What a session just did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// An agent finished its turn, went quiet or rang its bell: it wants the
    /// user.
    Waiting,
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

    /// What happened, as one line.
    pub fn line(self) -> String {
        match self {
            Self::Waiting => "finished its turn · waiting for you".to_owned(),
            Self::Finished { code } => format!("exited cleanly · code {code}"),
            Self::Failed { code } => format!("failed · code {code}"),
        }
    }

    /// How it reads at a glance.
    pub fn kind(self) -> Kind {
        match self {
            Self::Waiting => Kind::Waiting,
            Self::Finished { .. } => Kind::Finished,
            Self::Failed { .. } => Kind::Failed,
        }
    }
}

/// The kind of a note: what its marker is coloured by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// It wants the user.
    Waiting,
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
    /// What happened: "finished its turn · waiting for you · this computer".
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
        assert_eq!(Event::Waiting.line(), "finished its turn · waiting for you");
        assert_eq!(
            Event::Finished { code: 0 }.line(),
            "exited cleanly · code 0"
        );
        assert_eq!(Event::Failed { code: 3 }.line(), "failed · code 3");
        assert_eq!(Event::Waiting.kind(), Kind::Waiting);
        assert_eq!(Event::Finished { code: 0 }.kind(), Kind::Finished);
        assert_eq!(Event::Failed { code: 1 }.kind(), Kind::Failed);
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
}

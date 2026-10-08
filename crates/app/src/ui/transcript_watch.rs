//! Following the agents' transcripts for the sessions' lights, whether or
//! not the Den is open.
//!
//! The sidebar's dot, the home's counts and the notifications tell an agent
//! that finished its turn from one that asks something from what the agent's
//! own transcript says ([`Transcript`], decided by
//! [`session_activity`](super::activity::session_activity)). This module
//! keeps that fact current: one loop reads, off the window's thread, what the
//! transcripts of the live Claude Code and Codex sessions of this computer
//! grew by, reduces it to a [`Pulse`] per session and tells the session what
//! it says.
//!
//! # Which sessions
//!
//! A session is followed when it runs on this computer, its agent is in front
//! of the shell **as the terminal can tell** ([`transcript_applies`]: the
//! terminal of Windows cannot, and nothing is read there), its transcript is
//! one Leon reads and its own session id is known for sure
//! ([`followed_id`]): started with it, or learned from the agent's state file
//! or arguments, and checked again against the process scan, so that an agent
//! that moved to another session in the terminal (`/clear`, `/resume`, a new
//! run) is followed there once the scan sees it. Until then the old
//! transcript is still the one read. A guess by the newest file of the folder
//! or by the title is not enough to colour a light. Every other session (SSH,
//! relay, another agent, a plain shell, an id not learned yet) is left to the
//! terminal's heuristic, and so is a followed one whose transcript is not
//! found, cannot be read or has said nothing yet.
//!
//! # A turn that was not seen
//!
//! A transcript is first read from its last [`RECENT_BYTES`], which can begin
//! in the middle of a turn. A [`Pulse`] reports its turn over until a beat
//! says otherwise, so a [`Reading`] says nothing until a beat placed the agent
//! in a turn or ended one, and the terminal's heuristic stands meanwhile.
//!
//! # Cost
//!
//! The loop exists only while a session is followed and ends with the last.
//! A transcript is read from its last [`RECENT_BYTES`] the first time (enough
//! to know whether a turn is going on, not the whole history), then only what
//! was appended: a poll of an unchanged file is an open and two `stat`s,
//! once every [`Options::den_tick`] (a second). The loop is started by the
//! window's coarse timer (`Thresholds::tick`, two seconds) or by an id being
//! learned, so a light can take that long to turn to the transcript. A
//! transcript that was not found is
//! looked for again every [`LOOK_AGAIN_EVERY`] polls. The Den keeps its own
//! followers, which read from the start, and is not affected by these.
//!
//! [`Options::den_tick`]: super::shell::Options

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use gpui_kit::{App, AppContext as _, Context, Task};
use leon_history::live::{Beat, Format, Pulse, Start};

use super::activity::{transcript_applies, Transcript};
use super::den_follow::{Followers, Policy, Report, Wanted};
use super::live::LiveSession;
use super::shell::Shell;

/// How much of the end of a transcript is read the first time.
pub const RECENT_BYTES: u64 = 1 << 20;

/// A transcript that was not found is looked for again every this many
/// polls.
pub const LOOK_AGAIN_EVERY: u32 = 8;

/// What the window keeps for the lights.
pub struct StatusUi {
    followers: Arc<Mutex<Followers>>,
    /// The loop that reads the transcripts while a session is followed.
    watch: Option<Task<()>>,
    /// The transcripts as far as they were read, by live session.
    readings: HashMap<u64, Reading>,
}

/// One transcript as far as it was read: the agent's session id it belongs
/// to, the [`Pulse`] of its beats, and whether those beats placed the agent
/// in a turn.
pub struct Reading {
    session: String,
    pulse: Pulse,
    placed: bool,
}

impl Reading {
    /// Nothing read yet of the transcript of `session`.
    pub fn new(session: &str) -> Self {
        Self {
            session: session.to_owned(),
            pulse: Pulse::new(),
            placed: false,
        }
    }

    /// Takes the next beat of the transcript.
    pub fn apply(&mut self, beat: &Beat) {
        self.pulse.apply(beat);
        // A result, a notice or a number says nothing of where the turn is: it
        // can trail a turn or sit in the middle of one.
        self.placed |= beat.opens_turn() || matches!(beat, Beat::TurnEnded | Beat::Interrupted);
    }

    /// What the transcript says, when the beats read place the agent in a
    /// turn (see the module's documentation), else nothing.
    pub fn transcript(&self) -> Option<Transcript> {
        self.placed.then(|| Transcript::of(&self.pulse)).flatten()
    }
}

impl Default for StatusUi {
    fn default() -> Self {
        Self {
            followers: Arc::new(Mutex::new(Followers::new(Policy {
                start: Start::Recent(RECENT_BYTES),
                retry_after: LOOK_AGAIN_EVERY,
            }))),
            watch: None,
            readings: HashMap::new(),
        }
    }
}

impl StatusUi {
    /// Whether no loop reads transcripts now.
    #[cfg(test)]
    #[cfg_attr(not(leon_posix_tests), allow(dead_code))] // used by the Unix-only tests
    pub(super) fn is_idle(&self) -> bool {
        self.watch.is_none()
    }
}

/// How an agent's session id was learned is sure enough to follow its
/// transcript: it was asked for (`resumed`), or read from the agent's own
/// state file or arguments. A guess is not.
pub fn trusted(how: &str) -> bool {
    matches!(how, "resumed" | "state-file" | "arguments")
}

/// The agent's own id of the session a terminal is followed by: the one it
/// learned, when learned for sure, else the one it was started with. A
/// terminal whose id is only a guess is not followed (and the id it was
/// started with is no longer its own once the agent moved to another).
pub fn followed_id(learned: Option<&(String, String)>, resumed: Option<&str>) -> Option<String> {
    match learned {
        Some((id, how)) => trusted(how).then(|| id.clone()),
        None => resumed.map(str::to_owned),
    }
}

/// The session id the lights follow in a live session, if it is followed.
fn followed(session: &LiveSession, cx: &App) -> Option<(leon_core::AgentId, String)> {
    if !session.machine.is_local() || session.is_paused() {
        return None;
    }
    let agent = session.shown_agent()?;
    Format::of(agent)?;
    if !transcript_applies(&session.signals(cx)) {
        return None;
    }
    let id = followed_id(session.learned.as_ref(), session.resumed.as_deref())?;
    Some((agent, id))
}

impl Shell {
    /// The sessions whose transcript is followed for their lights.
    pub(super) fn status_wanted(&self, cx: &App) -> Vec<Wanted> {
        self.live
            .all()
            .iter()
            .filter_map(|session| {
                let (agent, id) = followed(session, cx)?;
                Some(Wanted {
                    live: session.id.0,
                    agent,
                    cwd: session.cwd.clone(),
                    session: id,
                    subagents: Vec::new(),
                })
            })
            .collect()
    }

    /// Starts the loop that reads the transcripts, when a session is followed
    /// and it does not run already. Cheap to call: the timers and the
    /// session list changing call it, and it does nothing in the common case.
    pub(super) fn status_watch(&mut self, cx: &mut Context<Self>) {
        if self.status.watch.is_some() || self.status_wanted(cx).is_empty() {
            return;
        }
        let followers = self.status.followers.clone();
        self.status.watch = Some(cx.spawn(async move |this, cx| loop {
            let Ok(wanted) = this.read_with(cx, |this, cx| this.status_wanted(cx)) else {
                return;
            };
            if wanted.is_empty() {
                // Nobody left: the loop ends and with it the task, and the
                // terminals go back to the heuristic.
                this.update(cx, |this, cx| {
                    this.status.watch = None;
                    this.status_apply(Vec::new(), cx);
                })
                .ok();
                return;
            }
            let Ok(roots) = this.read_with(cx, |this, _| this.engine.prefs().roots) else {
                return;
            };
            let reading = followers.clone();
            let reports = cx
                .background_spawn(async move {
                    let mut reading = reading.lock().unwrap_or_else(PoisonError::into_inner);
                    reading.set_roots(roots);
                    reading.poll(&wanted)
                })
                .await;
            let Ok(tick) = this.update(cx, |this, cx| {
                this.status_apply(reports, cx);
                this.options.den_tick
            }) else {
                return;
            };
            cx.background_executor().timer(tick).await;
        }));
    }

    /// Takes what the transcripts grew by and tells each session what its
    /// transcript now says; a session with no report says nothing and its
    /// light is the terminal's. A session whose state changed is read again,
    /// and said if it now wants the user.
    pub(super) fn status_apply(&mut self, reports: Vec<Report>, cx: &mut Context<Self>) {
        self.status
            .readings
            .retain(|live, _| reports.iter().any(|report| report.live == *live));
        for report in reports {
            let entry = self
                .status
                .readings
                .entry(report.live)
                .or_insert_with(|| Reading::new(&report.session));
            if entry.session != report.session || report.restarted {
                *entry = Reading::new(&report.session);
            }
            for beat in &report.beats {
                entry.apply(beat);
            }
        }
        let mut moved = Vec::new();
        for session in self.live.all() {
            let said = followed(session, cx)
                .and_then(|(_, id)| {
                    self.status
                        .readings
                        .get(&session.id.0)
                        .filter(|reading| reading.session == id)
                })
                .and_then(Reading::transcript);
            if session.transcript != said {
                moved.push((session.id, said));
            }
        }
        if moved.is_empty() {
            return;
        }
        let thresholds = self.options.activity;
        let mut changed = false;
        for (id, said) in moved {
            if let Some(session) = self.live.get_mut(id) {
                session.transcript = said;
                changed |= self.set_activity(id, &thresholds, cx);
            }
        }
        if changed {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn learned(id: &str, how: &str) -> Option<(String, String)> {
        Some((id.to_owned(), how.to_owned()))
    }

    #[test]
    fn only_an_id_that_is_known_for_sure_is_followed() {
        for how in ["resumed", "state-file", "arguments"] {
            assert!(trusted(how), "{how}");
        }
        for how in ["newest-in-folder", "title", "id", ""] {
            assert!(!trusted(how), "{how}");
        }
    }

    #[test]
    fn the_id_a_terminal_is_followed_by_is_the_one_it_learned_or_else_the_one_it_started_with() {
        assert_eq!(
            followed_id(learned("a", "state-file").as_ref(), Some("b")).as_deref(),
            Some("a"),
            "what the agent said wins over what it was started with"
        );
        assert_eq!(
            followed_id(None, Some("b")).as_deref(),
            Some("b"),
            "started with a session to resume"
        );
        assert_eq!(followed_id(None, None), None, "nothing known yet");
        assert_eq!(
            followed_id(learned("a", "newest-in-folder").as_ref(), None),
            None,
            "a guess does not colour a light"
        );
        assert_eq!(
            followed_id(learned("a", "title").as_ref(), Some("b")),
            None,
            "and the id it started with is not its own once it moved on"
        );
    }

    fn finished(id: &str) -> Beat {
        Beat::ToolFinished {
            id: id.into(),
            failed: false,
            refused: false,
        }
    }

    fn read(beats: &[Beat]) -> Option<Transcript> {
        let mut reading = Reading::new("a");
        for beat in beats {
            reading.apply(beat);
        }
        reading.transcript()
    }

    #[test]
    fn a_reading_that_began_in_the_middle_of_a_turn_does_not_claim_it_is_over() {
        // The last megabyte began after the prompt, with the result of a tool
        // whose call was cut off: a bare pulse would report the turn over.
        let mut pulse = Pulse::new();
        pulse.apply(&finished("cut-off"));
        assert_eq!(Transcript::of(&pulse), Some(Transcript::TurnOver));
        assert_eq!(read(&[finished("cut-off")]), None, "unknown: the terminal");
        assert_eq!(
            read(&[
                finished("cut-off"),
                Beat::Usage {
                    context: 10,
                    output: 1,
                    window: None
                }
            ]),
            None
        );
        // Until a beat places the agent: it thinks, so the turn is going on.
        assert_eq!(
            read(&[finished("cut-off"), Beat::Thinking]),
            Some(Transcript::Turn)
        );
        // Or the turn is seen to end.
        assert_eq!(
            read(&[finished("cut-off"), Beat::TurnEnded]),
            Some(Transcript::TurnOver)
        );
        assert_eq!(
            read(&[finished("cut-off"), Beat::Interrupted]),
            Some(Transcript::TurnOver)
        );
    }

    #[test]
    fn a_reading_of_a_whole_turn_says_what_the_pulse_says() {
        assert_eq!(read(&[]), None);
        assert_eq!(read(&[Beat::Prompt]), Some(Transcript::Turn));
        assert_eq!(
            read(&[Beat::Prompt, Beat::Thinking, Beat::TurnEnded]),
            Some(Transcript::TurnOver)
        );
    }
}

//! From a live session to a lion of the Den: what is known, said plainly.
//!
//! The Den (`leon-den`) draws what it is told. This module decides what to
//! tell it, and it is pure: the window hands in the facts of a session
//! ([`Facts`], read from its terminal) and, when the session's transcript is
//! followed, its [`Pulse`]; out come the [`Cub`]s (the session and its
//! sub-agents) and the [`Happening`]s the narrator tells.
//!
//! # The state of a session
//!
//! Read top to bottom; the first row that applies is the state.
//!
//! | What is known | State | The plain truth shown |
//! | --- | --- | --- |
//! | the terminal ended, code not 0 | `Fainted` | the exit code |
//! | the terminal ended, code 0 | `Gone` | |
//! | restored and paused (its agent is not resumed yet) | `Asleep` | that it is paused |
//! | no agent in front (a plain shell, or the agent returned to the shell), a program printing | `Running` | that a program runs; which is not known |
//! | the same, a program quiet or the bell rang | `WaitingForUser` | as the sidebar's dot says |
//! | the same, at the prompt | `Idle`, or `Asleep` after [`ASLEEP_AFTER`] of silence | |
//! | an agent, **no transcript** (a remote session, an agent Leon cannot follow, an id not learned yet): printing | `Mystery` | nothing: only working or quiet is known |
//! | the same, quiet or the bell rang | `WaitingForUser` | |
//! | the same, at the prompt | `Idle` | |
//! | an agent with a transcript, the shell in front | `Idle` | |
//! | a tool call without a result that asks the user (`AskUserQuestion`, `ExitPlanMode`) | `WaitingForUser` | what it asks |
//! | a tool call without a result, and the terminal quiet or its bell rung | `NeedsPermission` | the tool, and that this is inferred |
//! | a tool call without a result, the terminal printing | by the tool's kind: `Editing`, `Reading`, `Searching`, `Running`, `Web`, `Delegating`, `Planning`, `UsingTool` | the tool's name and its subject |
//! | no tool in flight, the turn not over | `Thinking` | reading the message, thinking, writing, or reading a result |
//! | the turn over, background sub-agents alive | `Delegating` | how many |
//! | the turn over | `WaitingForUser`, or `Asleep` after [`ASLEEP_AFTER`] of silence | |
//!
//! # A session that runs elsewhere
//!
//! An agent that runs in another window of Leon or in a plain terminal has
//! no terminal here: nothing says whether it prints. Its lion ([`Away`]) is
//! told by its transcript alone and by how long ago the transcript's file
//! was last written. Read top to bottom:
//!
//! | What is known | State | The plain truth shown |
//! | --- | --- | --- |
//! | its process is no longer in the scan | it leaves the den (`went home`): the exit code is not known, so it never faints | |
//! | a tool call without a result that asks the user (`AskUserQuestion`, `ExitPlanMode`) | `WaitingForUser` | what it asks |
//! | a tool call without a result | by the tool's kind, as above | the tool and its subject; `quiet for N min` once the file has not grown for a minute |
//! | no tool in flight, the turn not over | `Thinking` | what it was last doing, and how long it has been quiet |
//! | the turn over, background sub-agents alive | `Delegating` | how many |
//! | the turn over | `WaitingForUser`, or `Asleep` after [`ASLEEP_AFTER`] without a write | |
//! | nothing in the transcript yet | `Idle` | |
//!
//! Every card ends with where it runs: `in another Leon window` or `in a
//! terminal outside Leon` (with the terminal application when the process
//! tree names one) and the pid.
//!
//! **No permission prompt is claimed for these.** Here the inference above
//! rests on the terminal being quiet, which is seen within seconds. A
//! transcript file is also silent for the whole length of any tool that
//! runs (a build, a test suite), so no length of silence tells a question
//! from work. The lion keeps showing the tool, and the card says for how
//! long nothing was written: the reader decides.
//!
//! Only a process that **names** its session becomes a lion (a certain
//! match: Claude Code's own state file, or the id in its arguments). A
//! guess by folder (`likely`) does not: the Den would be reading aloud a
//! transcript that may be another session's.
//!
//! # The permission prompt is inferred
//!
//! A transcript does not say that a permission question is on screen: the
//! tool call is written before the question and nothing follows until it is
//! answered. So "needs permission" is **a call without a result and a quiet
//! terminal**. An agent that works redraws its status line all the time, so
//! a quiet terminal with a call in flight is, almost always, a question. It
//! is wrong for a tool that runs long while the agent's interface draws
//! nothing, and it is late by the quiet threshold. The truth card says it is
//! inferred.
//!
//! # What is never made up
//!
//! A session without a transcript is a `mystery`: its level is 0, it has no
//! detail and no little ones. A tool that ended without news gets no line;
//! only a command that really passed is told as passed.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::Hsla;
use leon_den::feed::Past;
use leon_den::{Cub, CubState, Event, Happening, Species};
use leon_history::live::{Beat, Phase, Pulse, SubAgent, ToolInFlight, ToolKind};

use super::activity::Activity;

/// How long a session that waits must be silent before it is asleep.
pub const ASLEEP_AFTER: Duration = Duration::from_secs(15 * 60);

/// What the window knows of a live session without reading its transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    /// The lion's id: the live session's.
    pub id: u64,
    /// What the sidebar calls the session.
    pub name: String,
    /// The colour of its agent in the theme.
    pub tint: Hsla,
    /// Whether an agent is in front of its shell, as far as Leon knows.
    pub agent: bool,
    /// How its terminal is doing.
    pub activity: Activity,
    /// It was restored and waits to be resumed.
    pub paused: bool,
    /// The exit code, once its terminal ended.
    pub exit: Option<u32>,
    /// How long since it last printed.
    pub quiet_for: Option<Duration>,
}

/// The transcript of a session as far as it was read: its own pulse and one
/// per sub-agent whose transcript is followed, by the id of the call that
/// started it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    /// The session's own picture.
    pub pulse: Pulse,
    /// Its sub-agents' own pictures.
    pub subs: HashMap<String, Pulse>,
}

/// A stable number from a text (FNV-1a): the seed that makes a session an
/// individual, the id of a little one.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The id of the little one a tool call of a session started. Its top bit is
/// set, so it is never the id of a session.
pub fn little_id(parent: u64, tool: &str) -> u64 {
    (hash(tool) ^ parent.rotate_left(17)) | 1 << 63
}

/// What a sub-agent is called: its type, the first word of what its call
/// said of it.
pub fn little_name(label: &str) -> String {
    let name = label
        .split([':', ' ', '\n'])
        .find(|word| !word.is_empty())
        .unwrap_or("sub-agent");
    name.to_owned()
}

fn tool_line(tool: &ToolInFlight) -> String {
    if tool.detail.trim().is_empty() {
        tool.name.clone()
    } else {
        format!("{} {}", tool.name, tool.detail)
    }
}

/// Whether a call waits for the user by its nature: a question, or a plan
/// to approve.
pub(super) fn asks(tool: &ToolInFlight) -> bool {
    tool.kind == ToolKind::Ask || tool.name.eq_ignore_ascii_case("ExitPlanMode")
}

fn working_state(kind: ToolKind) -> CubState {
    match kind {
        ToolKind::Edit => CubState::Editing,
        ToolKind::Read => CubState::Reading,
        ToolKind::Search => CubState::Searching,
        ToolKind::Run => CubState::Running,
        ToolKind::Web => CubState::Web,
        ToolKind::Delegate => CubState::Delegating,
        ToolKind::Plan => CubState::Planning,
        ToolKind::Ask => CubState::WaitingForUser,
        ToolKind::Other => CubState::UsingTool,
    }
}

/// The state of a session and the plain truth about it: the table of this
/// module's documentation.
pub fn state(facts: &Facts, pulse: Option<&Pulse>) -> (CubState, Option<String>) {
    let some = |text: &str| Some(text.to_owned());
    if let Some(code) = facts.exit {
        return if code == 0 {
            (CubState::Gone, None)
        } else {
            (CubState::Fainted, Some(format!("Exited with code {code}")))
        };
    }
    if facts.paused {
        return (
            CubState::Asleep,
            some("Paused: its agent is resumed when the session is shown"),
        );
    }
    let asleep = facts.quiet_for.is_some_and(|quiet| quiet >= ASLEEP_AFTER);
    let rest = |state: CubState, detail: Option<String>| {
        if asleep {
            (
                CubState::Asleep,
                some("Nothing has happened for a long while"),
            )
        } else {
            (state, detail)
        }
    };
    if !facts.agent {
        return match facts.activity {
            Activity::Working => (CubState::Running, some("A program is running in the shell")),
            Activity::Waiting => (
                CubState::WaitingForUser,
                some("A program is quiet, or the bell rang"),
            ),
            _ => rest(CubState::Idle, some("At the shell prompt")),
        };
    }
    let Some(pulse) = pulse else {
        return match facts.activity {
            Activity::Working => (CubState::Mystery, None),
            Activity::Waiting => (CubState::WaitingForUser, some("Quiet, or the bell rang")),
            _ => (CubState::Idle, None),
        };
    };
    if facts.activity == Activity::Idle {
        return rest(CubState::Idle, some("At the shell prompt"));
    }
    if let Some(tool) = pulse.tools().last() {
        if asks(tool) {
            let what = if tool.kind == ToolKind::Ask {
                "Asked you a question"
            } else {
                "Waits for you to approve its plan"
            };
            let detail = match tool.detail.trim() {
                "" => what.to_owned(),
                subject => format!("{what}: {subject}"),
            };
            return (CubState::WaitingForUser, Some(detail));
        }
        if facts.activity == Activity::Waiting {
            return (
                CubState::NeedsPermission,
                Some(format!(
                    "{} has no result and the terminal is quiet: probably a permission prompt",
                    tool_line(tool)
                )),
            );
        }
        return (working_state(tool.kind), Some(tool_line(tool)));
    }
    if pulse.turn_over() {
        let background = pulse.subagents().len();
        return match background {
            0 => rest(CubState::WaitingForUser, some("Finished its turn")),
            1 => (
                CubState::Delegating,
                some("Its turn is over; 1 sub-agent still works in the background"),
            ),
            n => (
                CubState::Delegating,
                Some(format!(
                    "Its turn is over; {n} sub-agents still work in the background"
                )),
            ),
        };
    }
    let doing = match pulse.phase() {
        Phase::Prompted => "Reading your message",
        Phase::Speaking => "Writing its answer",
        Phase::Deciding => "Reading a tool's result",
        _ => "Thinking",
    };
    (CubState::Thinking, some(doing))
}

/// Where a session that runs elsewhere runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// Another window of Leon on this computer.
    OtherLeon,
    /// A terminal that is not Leon's, and the application when the process
    /// tree names it.
    Terminal(Option<String>),
}

/// A session that runs elsewhere on this computer, as the process scan and
/// the history know it.
#[derive(Debug, Clone, PartialEq)]
pub struct Away {
    /// The lion's id ([`away_id`]).
    pub id: u64,
    /// What the sidebar calls the session.
    pub name: String,
    /// The colour of its agent in the theme.
    pub tint: Hsla,
    /// Where it runs.
    pub place: Place,
    /// The pid of its process.
    pub pid: u32,
    /// How long ago its transcript was last written.
    pub quiet_for: Option<Duration>,
}

/// The id of the lion of a session that runs elsewhere: a number that stays
/// the same while it does, and that no live session of this window (small
/// numbers) and no little one (the top bit) has.
pub fn away_id(agent: &str, session: &str) -> u64 {
    (hash(&format!("{agent}\u{1f}{session}")) & !(1 << 63)) | 1 << 62
}

/// Whether a lion's id is that of a session that runs elsewhere.
pub fn is_away(id: u64) -> bool {
    id & (1 << 63) == 0 && id & (1 << 62) != 0
}

/// What a session that runs elsewhere is called when the history does not
/// know it yet: its agent and the folder it runs in.
pub fn away_name(agent: &str, cwd: Option<&str>) -> String {
    let folder = cwd
        .map(|cwd| cwd.trim_end_matches(['/', '\\']))
        .and_then(|cwd| cwd.rsplit(['/', '\\']).next())
        .filter(|folder| !folder.is_empty());
    match folder {
        Some(folder) => format!("{agent} in {folder}"),
        None => agent.to_owned(),
    }
}

fn quiet_words(quiet: Option<Duration>) -> Option<String> {
    let minutes = quiet?.as_secs() / 60;
    match minutes {
        0 => None,
        1..=119 => Some(format!("quiet for {minutes} min")),
        _ => Some(format!("quiet for {} h", minutes / 60)),
    }
}

/// The state of a session that runs elsewhere and the plain truth about it:
/// the second table of this module's documentation.
pub fn away_state(away: &Away, pulse: &Pulse) -> (CubState, Option<String>) {
    let quiet = quiet_words(away.quiet_for);
    let with_quiet = |text: String| match &quiet {
        Some(quiet) => format!("{text} \u{b7} {quiet}"),
        None => text,
    };
    let (state, detail) = if let Some(tool) = pulse.tools().last() {
        if asks(tool) {
            let what = if tool.kind == ToolKind::Ask {
                "Asked a question"
            } else {
                "Waits for its plan to be approved"
            };
            let detail = match tool.detail.trim() {
                "" => what.to_owned(),
                subject => format!("{what}: {subject}"),
            };
            (CubState::WaitingForUser, detail)
        } else {
            (working_state(tool.kind), with_quiet(tool_line(tool)))
        }
    } else if *pulse == Pulse::new() {
        (CubState::Idle, "Nothing in its transcript yet".to_owned())
    } else if pulse.turn_over() {
        match pulse.subagents().len() {
            0 if away.quiet_for.is_some_and(|quiet| quiet >= ASLEEP_AFTER) => {
                (CubState::Asleep, with_quiet("Finished its turn".to_owned()))
            }
            0 => (CubState::WaitingForUser, "Finished its turn".to_owned()),
            1 => (
                CubState::Delegating,
                "Its turn is over; 1 sub-agent still works in the background".to_owned(),
            ),
            n => (
                CubState::Delegating,
                format!("Its turn is over; {n} sub-agents still work in the background"),
            ),
        }
    } else {
        let doing = match pulse.phase() {
            Phase::Prompted => "Reading the message",
            Phase::Speaking => "Writing its answer",
            Phase::Deciding => "Reading a tool's result",
            _ => "Thinking",
        };
        (CubState::Thinking, with_quiet(doing.to_owned()))
    };
    let place = match &away.place {
        Place::OtherLeon => "in another Leon window".to_owned(),
        Place::Terminal(Some(app)) => format!("in {app}, outside Leon"),
        Place::Terminal(None) => "in a terminal outside Leon".to_owned(),
    };
    (
        state,
        Some(format!("{detail} \u{b7} runs {place} (pid {})", away.pid)),
    )
}

/// The lion of a session that runs elsewhere and its little ones.
pub fn away_cubs(away: &Away, reading: &Reading) -> Vec<Cub> {
    let (state, detail) = away_state(away, &reading.pulse);
    let mut out = vec![Cub {
        id: away.id,
        name: away.name.clone(),
        species: Species {
            tint: away.tint,
            seed: hash(&away.name) ^ away.id,
        },
        state,
        level: level(&reading.pulse),
        detail,
        parent: None,
        mystery: false,
    }];
    for sub in reading.pulse.subagents() {
        out.push(little(away.id, away.tint, sub, reading.subs.get(&sub.tool)));
    }
    out
}

fn level(pulse: &Pulse) -> u32 {
    u32::try_from(pulse.tool_calls()).unwrap_or(u32::MAX)
}

fn little(parent: u64, tint: Hsla, sub: &SubAgent, own: Option<&Pulse>) -> Cub {
    let current = sub
        .current
        .as_ref()
        .or_else(|| own.and_then(|pulse| pulse.tools().last()));
    let (state, detail) = match current {
        Some(tool) => (working_state(tool.kind), Some(tool_line(tool))),
        None => (CubState::Thinking, None),
    };
    // A little one that asks is its parent's business: it works meanwhile.
    let state = if state == CubState::WaitingForUser {
        CubState::Thinking
    } else {
        state
    };
    let about = sub.label.trim();
    Cub {
        id: little_id(parent, &sub.tool),
        name: little_name(&sub.label),
        species: Species {
            tint,
            seed: hash(&sub.tool),
        },
        state,
        level: own.map_or(0, level),
        detail: match (detail, about.is_empty()) {
            (Some(detail), true) => Some(detail),
            (Some(detail), false) => Some(format!("{detail} · {about}")),
            (None, true) => None,
            (None, false) => Some(about.to_owned()),
        },
        parent: Some(parent),
        mystery: false,
    }
}

/// The lion of a session and its little ones: the session first.
pub fn cubs(facts: &Facts, reading: Option<&Reading>) -> Vec<Cub> {
    let pulse = reading.map(|reading| &reading.pulse);
    let (state, detail) = state(facts, pulse);
    let mut out = vec![Cub {
        id: facts.id,
        name: facts.name.clone(),
        species: Species {
            tint: facts.tint,
            seed: hash(&facts.name) ^ facts.id,
        },
        state,
        level: pulse.map_or(0, level),
        detail,
        parent: None,
        mystery: facts.agent && pulse.is_none(),
    }];
    // A session that ended has no little ones left to show.
    if let (Some(reading), None) = (reading, facts.exit) {
        for sub in reading.pulse.subagents() {
            out.push(little(
                facts.id,
                facts.tint,
                sub,
                reading.subs.get(&sub.tool),
            ));
        }
    }
    out
}

fn den_kind(kind: ToolKind) -> leon_den::ToolKind {
    match kind {
        ToolKind::Edit => leon_den::ToolKind::Edit,
        ToolKind::Read => leon_den::ToolKind::Read,
        ToolKind::Search => leon_den::ToolKind::Search,
        ToolKind::Run => leon_den::ToolKind::Run,
        ToolKind::Web => leon_den::ToolKind::Web,
        ToolKind::Plan => leon_den::ToolKind::Plan,
        ToolKind::Delegate | ToolKind::Ask | ToolKind::Other => leon_den::ToolKind::Other,
    }
}

/// Applies the next beats of a session's transcript to its pulse and says
/// what the narrator tells of them: tools that start and end with their real
/// name and subject, sub-agents sent out and back, the end of the turn.
pub fn tell(id: u64, name: &str, pulse: &mut Pulse, beats: &[Beat]) -> Vec<Happening> {
    let mut told = Vec::new();
    for beat in beats {
        let event = match beat {
            Beat::ToolStarted {
                name: tool,
                kind,
                detail,
                ..
            } => match kind {
                ToolKind::Delegate => Some(Event::SentOut {
                    little: little_name(detail),
                }),
                // A question is told by the lion that waits, not as a move.
                ToolKind::Ask => None,
                kind => Some(Event::ToolStarted {
                    kind: den_kind(*kind),
                    tool: tool.clone(),
                    detail: Some(detail.trim().to_owned()).filter(|detail| !detail.is_empty()),
                }),
            },
            Beat::ToolFinished {
                id: call, failed, ..
            } => {
                let tool = pulse.tools().iter().find(|tool| tool.id == *call);
                let sub = pulse.subagents().iter().find(|sub| sub.tool == *call);
                match (tool.map(|tool| tool.kind), sub) {
                    // Launched into the background: it is not back yet.
                    (_, Some(sub)) if sub.background => None,
                    (Some(ToolKind::Delegate), sub) => Some(Event::CameBack {
                        little: little_name(sub.map_or("", |sub| sub.label.as_str())),
                    }),
                    (Some(ToolKind::Ask), _) | (None, _) => None,
                    (Some(kind), _) => Some(Event::ToolFinished {
                        kind: den_kind(kind),
                        ok: !failed,
                    }),
                }
            }
            Beat::TaskEnded { task, tool, .. } => pulse
                .subagents()
                .iter()
                .find(|sub| {
                    sub.agent.as_deref() == Some(task.as_str()) || Some(&sub.tool) == tool.as_ref()
                })
                .map(|sub| Event::CameBack {
                    little: little_name(&sub.label),
                }),
            Beat::TurnEnded => Some(Event::TurnEnded),
            _ => None,
        };
        if let Some(event) = event {
            told.push(Happening::new(id, name, event));
        }
        pulse.apply(beat);
    }
    told
}

/// The past of a session as the feed sums it up ([`leon_den::feed::backfill`]):
/// every tool it used, every message it wrote to the user with its time, and
/// where its turns ended. A question to the user is no tool.
pub fn past(beats: &[Beat]) -> Vec<Past> {
    beats
        .iter()
        .filter_map(|beat| match beat {
            Beat::ToolStarted { kind, .. } if *kind != ToolKind::Ask => Some(Past::Tool),
            Beat::Said { text, at } => Some(Past::Speech(text.clone(), *at)),
            Beat::TurnEnded | Beat::Interrupted => Some(Past::TurnEnded),
            _ => None,
        })
        .collect()
}

/// What the narrator tells of the change from one list of lions to the
/// next, which no beat says: who joined, who went home, who fainted and with
/// which code, who fell asleep, and the permission prompt, on the moment it
/// is inferred. For a session whose transcript is not followed (`followed`
/// answers `false`) also the end of its turn and that it works, since no
/// beat will say so.
pub fn changes(
    before: &[Cub],
    after: &[Cub],
    exit: impl Fn(u64) -> Option<u32>,
    followed: impl Fn(u64) -> bool,
) -> Vec<Happening> {
    let mut told = Vec::new();
    for cub in after.iter().filter(|cub| cub.parent.is_none()) {
        let was = before
            .iter()
            .find(|old| old.id == cub.id)
            .map(|old| old.state);
        let tell = |event: Event| Happening::new(cub.id, cub.name.clone(), event);
        if was.is_none() && cub.state != CubState::Gone {
            told.push(tell(Event::Joined));
        }
        if was == Some(cub.state) {
            continue;
        }
        let known = followed(cub.id);
        let event = match cub.state {
            CubState::NeedsPermission => Some(Event::PermissionPrompt),
            CubState::Fainted => Some(Event::Fainted {
                exit: exit(cub.id).and_then(|code| i32::try_from(code).ok()),
            }),
            CubState::Gone if was.is_some() => Some(Event::WentHome),
            CubState::Asleep if was.is_some() => Some(Event::FellAsleep),
            CubState::WaitingForUser if !known && was.is_some_and(CubState::is_working) => {
                Some(Event::TurnEnded)
            }
            CubState::Mystery if !was.is_some_and(CubState::is_working) => Some(Event::Mysterious),
            _ => None,
        };
        told.extend(event.map(tell));
    }
    for old in before.iter().filter(|old| old.parent.is_none()) {
        let left = !after.iter().any(|cub| cub.id == old.id);
        if left && !matches!(old.state, CubState::Gone | CubState::Fainted) {
            told.push(Happening::new(old.id, old.name.clone(), Event::WentHome));
        }
    }
    told
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::rgb;

    fn facts(activity: Activity) -> Facts {
        Facts {
            id: 7,
            name: "moss".to_owned(),
            tint: rgb(0xd97757).into(),
            agent: true,
            activity,
            paused: false,
            exit: None,
            quiet_for: Some(Duration::from_secs(1)),
        }
    }

    fn started(id: &str, name: &str, detail: &str) -> Beat {
        Beat::ToolStarted {
            id: id.to_owned(),
            name: name.to_owned(),
            kind: ToolKind::of(name),
            detail: detail.to_owned(),
        }
    }

    fn finished(id: &str, failed: bool) -> Beat {
        Beat::ToolFinished {
            id: id.to_owned(),
            failed,
            refused: false,
        }
    }

    fn pulse(beats: &[Beat]) -> Pulse {
        let mut pulse = Pulse::new();
        for beat in beats {
            pulse.apply(beat);
        }
        pulse
    }

    fn state_of(facts: &Facts, beats: &[Beat]) -> CubState {
        state(facts, Some(&pulse(beats))).0
    }

    #[test]
    fn a_tool_in_flight_is_the_state_of_its_kind_with_its_name_and_subject() {
        let working = facts(Activity::Working);
        for (tool, subject, expected) in [
            ("Edit", "shell.rs", CubState::Editing),
            ("Write", "notes.md", CubState::Editing),
            ("Read", "keys.rs", CubState::Reading),
            ("Grep", "Overlay", CubState::Searching),
            ("Bash", "cargo test -p leon-den", CubState::Running),
            ("WebFetch", "docs.rs", CubState::Web),
            ("Task", "Explore: find the tests", CubState::Delegating),
            ("TodoWrite", "", CubState::Planning),
            ("mcp__linear__list_issues", "", CubState::UsingTool),
        ] {
            let beats = [Beat::Prompt, started("t1", tool, subject)];
            let (state, detail) = state(&working, Some(&pulse(&beats)));
            assert_eq!(state, expected, "{tool}");
            let detail = detail.expect("the plain truth");
            assert!(detail.starts_with(tool), "{detail}");
            assert!(detail.contains(subject), "{detail}");
        }
    }

    #[test]
    fn with_several_calls_in_flight_the_newest_is_shown() {
        let beats = [
            Beat::Prompt,
            started("t1", "Bash", "cargo build"),
            started("t2", "Read", "keys.rs"),
        ];
        assert_eq!(
            state_of(&facts(Activity::Working), &beats),
            CubState::Reading
        );
    }

    #[test]
    fn a_call_without_a_result_and_a_quiet_terminal_is_a_permission_prompt_and_says_it_is_inferred()
    {
        let beats = [Beat::Prompt, started("t1", "Bash", "rm -rf target")];
        assert_eq!(
            state_of(&facts(Activity::Working), &beats),
            CubState::Running
        );
        let (state, detail) = state(&facts(Activity::Waiting), Some(&pulse(&beats)));
        assert_eq!(state, CubState::NeedsPermission);
        let detail = detail.unwrap();
        assert!(
            detail.contains("Bash rm -rf target") && detail.contains("probably"),
            "{detail}"
        );
        // The result arrives: it was allowed, the agent works again.
        let after = [beats[0].clone(), beats[1].clone(), finished("t1", false)];
        assert_eq!(
            state_of(&facts(Activity::Working), &after),
            CubState::Thinking
        );
        // A quiet terminal with no call in flight is not a prompt.
        assert_eq!(
            state_of(&facts(Activity::Waiting), &[Beat::Prompt, Beat::Thinking]),
            CubState::Thinking
        );
    }

    #[test]
    fn a_question_or_a_plan_to_approve_waits_for_the_user_whatever_the_terminal_does() {
        for activity in [Activity::Working, Activity::Waiting] {
            let ask = [
                Beat::Prompt,
                started("t1", "AskUserQuestion", "Which branch?"),
            ];
            let (state, detail) = state(&facts(activity), Some(&pulse(&ask)));
            assert_eq!(state, CubState::WaitingForUser);
            assert_eq!(detail.unwrap(), "Asked you a question: Which branch?");
            let plan = [Beat::Prompt, started("t1", "ExitPlanMode", "")];
            let (state, detail) = super::state(&facts(activity), Some(&pulse(&plan)));
            assert_eq!(state, CubState::WaitingForUser);
            assert_eq!(detail.unwrap(), "Waits for you to approve its plan");
        }
    }

    #[test]
    fn between_tools_the_agent_thinks_and_after_its_turn_it_waits() {
        let working = facts(Activity::Working);
        for (beats, doing) in [
            (vec![Beat::Prompt], "Reading your message"),
            (vec![Beat::Prompt, Beat::Thinking], "Thinking"),
            (vec![Beat::Prompt, Beat::said("ok")], "Writing its answer"),
            (
                vec![
                    Beat::Prompt,
                    started("t1", "Read", "a.rs"),
                    finished("t1", false),
                ],
                "Reading a tool's result",
            ),
        ] {
            let (state, detail) = state(&working, Some(&pulse(&beats)));
            assert_eq!(state, CubState::Thinking, "{doing}");
            assert_eq!(detail.as_deref(), Some(doing));
        }
        let over = [Beat::Prompt, Beat::said("ok"), Beat::TurnEnded];
        assert_eq!(
            state_of(&facts(Activity::Waiting), &over),
            CubState::WaitingForUser
        );
        // Nothing read yet is a turn that is over, too.
        assert_eq!(
            state_of(&facts(Activity::Waiting), &[]),
            CubState::WaitingForUser
        );
        // After a long silence it is asleep.
        let mut silent = facts(Activity::Waiting);
        silent.quiet_for = Some(ASLEEP_AFTER);
        assert_eq!(state_of(&silent, &over), CubState::Asleep);
        // But never while a tool is in flight or the model works.
        assert_eq!(
            state_of(&silent, &[Beat::Prompt, started("t1", "Bash", "make")]),
            CubState::NeedsPermission
        );
    }

    #[test]
    fn a_turn_that_ended_with_sub_agents_in_the_background_is_still_delegating() {
        let beats = [
            Beat::Prompt,
            started("t1", "Task", "Explore: map the crate"),
            Beat::Detached {
                id: "t1".to_owned(),
                task: "a1".to_owned(),
            },
            Beat::said("ok"),
            Beat::TurnEnded,
        ];
        let pulse = pulse(&beats);
        assert_eq!(
            pulse.subagents().len(),
            1,
            "the sub-agent outlives the turn"
        );
        let (state, detail) = state(&facts(Activity::Waiting), Some(&pulse));
        assert_eq!(state, CubState::Delegating);
        assert!(detail.unwrap().contains("1 sub-agent"));
    }

    #[test]
    fn what_the_terminal_says_comes_first_ended_paused_or_back_at_the_shell() {
        let beats = [Beat::Prompt, started("t1", "Bash", "make")];
        let pulse = pulse(&beats);
        let mut ended = facts(Activity::Failed);
        ended.exit = Some(101);
        assert_eq!(
            state(&ended, Some(&pulse)),
            (CubState::Fainted, Some("Exited with code 101".to_owned()))
        );
        ended.exit = Some(0);
        assert_eq!(state(&ended, Some(&pulse)), (CubState::Gone, None));
        let mut paused = facts(Activity::Idle);
        paused.paused = true;
        assert_eq!(state(&paused, Some(&pulse)).0, CubState::Asleep);
        // The agent returned to the shell: the transcript's last word is old.
        assert_eq!(
            state(&facts(Activity::Idle), Some(&pulse)).0,
            CubState::Idle
        );
    }

    #[test]
    fn a_plain_shell_says_what_its_terminal_says_and_is_no_mystery() {
        let mut shell = facts(Activity::Working);
        shell.agent = false;
        assert_eq!(state(&shell, None).0, CubState::Running);
        shell.activity = Activity::Waiting;
        assert_eq!(state(&shell, None).0, CubState::WaitingForUser);
        shell.activity = Activity::Idle;
        assert_eq!(state(&shell, None).0, CubState::Idle);
        let lion = &cubs(&shell, None)[0];
        assert!(!lion.mystery);
        assert_eq!(lion.level, 0);
    }

    #[test]
    fn an_agent_without_a_transcript_is_a_mystery_with_nothing_made_up() {
        for (activity, expected) in [
            (Activity::Working, CubState::Mystery),
            (Activity::Waiting, CubState::WaitingForUser),
            (Activity::Idle, CubState::Idle),
        ] {
            let lions = cubs(&facts(activity), None);
            assert_eq!(lions.len(), 1, "no little ones are known");
            let lion = &lions[0];
            assert_eq!(lion.state, expected);
            assert!(lion.mystery);
            assert_eq!(lion.level, 0, "no tool was counted");
            if activity == Activity::Working {
                assert_eq!(lion.detail, None, "nothing is known of what it does");
            }
        }
    }

    #[test]
    fn the_level_is_the_number_of_tool_calls_and_sub_agents_are_little_ones() {
        let beats = [
            Beat::Prompt,
            started("t1", "Read", "a.rs"),
            finished("t1", false),
            started("t2", "Edit", "a.rs"),
            finished("t2", false),
            started("t3", "Task", "Explore: find the tests"),
        ];
        let mut reading = Reading {
            pulse: pulse(&beats),
            subs: HashMap::new(),
        };
        let lions = cubs(&facts(Activity::Working), Some(&reading));
        assert_eq!(lions.len(), 2);
        let (moss, small) = (&lions[0], &lions[1]);
        assert_eq!(
            (moss.id, moss.level, moss.state),
            (7, 3, CubState::Delegating)
        );
        assert!(!moss.mystery && moss.parent.is_none());
        assert_eq!(small.parent, Some(7));
        assert_eq!(small.name, "Explore");
        assert_eq!(small.id, little_id(7, "t3"));
        assert!(small.id >> 63 == 1 && small.id != moss.id);
        assert_eq!(
            small.state,
            CubState::Thinking,
            "its own tool is not known yet"
        );
        assert_eq!(small.detail.as_deref(), Some("Explore: find the tests"));

        // Its own transcript is followed: its tool and its level are known.
        let own = [
            started("s1", "Grep", "fn test"),
            finished("s1", false),
            started("s2", "Read", "x.rs"),
        ];
        let mut sub = Pulse::new();
        for beat in &own {
            sub.apply(beat);
            reading.pulse.apply_to_subagent("t3", beat);
        }
        reading.subs.insert("t3".to_owned(), sub);
        let small = &cubs(&facts(Activity::Working), Some(&reading))[1];
        assert_eq!((small.state, small.level), (CubState::Reading, 2));
        assert!(small.detail.as_deref().unwrap().starts_with("Read x.rs"));

        // A session that ended shows no little one.
        let mut ended = facts(Activity::Failed);
        ended.exit = Some(2);
        assert_eq!(cubs(&ended, Some(&reading)).len(), 1);
    }

    #[test]
    fn a_sub_agent_is_called_by_its_type() {
        assert_eq!(little_name("Explore: find the tests"), "Explore");
        assert_eq!(
            little_name("general-purpose map the crate"),
            "general-purpose"
        );
        assert_eq!(little_name("  "), "sub-agent");
        assert_ne!(little_id(1, "t1"), little_id(1, "t2"));
        assert_ne!(little_id(1, "t1"), little_id(2, "t1"));
    }

    fn events(told: &[Happening]) -> Vec<Event> {
        told.iter()
            .map(|happening| happening.event.clone())
            .collect()
    }

    #[test]
    fn the_narrator_is_told_of_tools_with_their_real_name_and_subject() {
        let mut pulse = Pulse::new();
        let told = tell(
            7,
            "moss",
            &mut pulse,
            &[
                Beat::Prompt,
                Beat::Thinking,
                started("t1", "Bash", "cargo test -p leon-den"),
                finished("t1", false),
                started("t2", "Edit", "shell.rs"),
                finished("t2", true),
                started("t3", "Read", ""),
                finished("t3", false),
                Beat::said("ok"),
                Beat::TurnEnded,
            ],
        );
        assert!(told.iter().all(|one| one.cub == 7 && one.name == "moss"));
        assert_eq!(
            events(&told),
            vec![
                Event::ToolStarted {
                    kind: leon_den::ToolKind::Run,
                    tool: "Bash".to_owned(),
                    detail: Some("cargo test -p leon-den".to_owned()),
                },
                Event::ToolFinished {
                    kind: leon_den::ToolKind::Run,
                    ok: true,
                },
                Event::ToolStarted {
                    kind: leon_den::ToolKind::Edit,
                    tool: "Edit".to_owned(),
                    detail: Some("shell.rs".to_owned()),
                },
                Event::ToolFinished {
                    kind: leon_den::ToolKind::Edit,
                    ok: false,
                },
                Event::ToolStarted {
                    kind: leon_den::ToolKind::Read,
                    tool: "Read".to_owned(),
                    detail: None,
                },
                Event::ToolFinished {
                    kind: leon_den::ToolKind::Read,
                    ok: true,
                },
                Event::TurnEnded,
            ]
        );
        assert_eq!(pulse.tool_calls(), 3, "the beats were applied");
        assert!(pulse.turn_over());
    }

    #[test]
    fn a_sub_agent_is_sent_out_and_comes_back_and_a_background_one_only_when_it_ends() {
        let mut pulse = Pulse::new();
        let told = tell(
            7,
            "moss",
            &mut pulse,
            &[
                Beat::Prompt,
                started("t1", "Task", "Explore: find the tests"),
                finished("t1", false),
                started("t2", "Task", "Plan: write a plan"),
                Beat::Detached {
                    id: "t2".to_owned(),
                    task: "a2".to_owned(),
                },
                Beat::TurnEnded,
                Beat::TaskEnded {
                    task: "a2".to_owned(),
                    tool: Some("t2".to_owned()),
                    outcome: leon_history::live::TaskOutcome::Completed,
                },
            ],
        );
        assert_eq!(
            events(&told),
            vec![
                Event::SentOut {
                    little: "Explore".to_owned()
                },
                Event::CameBack {
                    little: "Explore".to_owned()
                },
                Event::SentOut {
                    little: "Plan".to_owned()
                },
                Event::TurnEnded,
                Event::CameBack {
                    little: "Plan".to_owned()
                },
            ]
        );
    }

    #[test]
    fn a_question_is_not_told_as_a_move_and_a_result_of_an_unknown_call_is_not_told() {
        let mut pulse = Pulse::new();
        let told = tell(
            7,
            "moss",
            &mut pulse,
            &[
                Beat::Prompt,
                started("t1", "AskUserQuestion", "Which?"),
                finished("t1", false),
                finished("t-unknown", true),
            ],
        );
        assert!(told.is_empty(), "{told:?}");
    }

    fn lion(id: u64, state: CubState) -> Cub {
        let mut facts = facts(Activity::Working);
        facts.id = id;
        Cub {
            state,
            ..cubs(&facts, None).remove(0)
        }
    }

    #[test]
    fn the_changes_no_beat_tells_are_told_from_the_lists() {
        let before = vec![
            lion(1, CubState::Running),
            lion(2, CubState::Running),
            lion(3, CubState::WaitingForUser),
            lion(4, CubState::Thinking),
            lion(5, CubState::Idle),
            lion(6, CubState::Mystery),
        ];
        let after = vec![
            lion(1, CubState::NeedsPermission),
            lion(2, CubState::Fainted),
            lion(3, CubState::Asleep),
            lion(4, CubState::Gone),
            // 5 is no longer listed.
            lion(6, CubState::WaitingForUser),
            lion(8, CubState::Mystery),
        ];
        let told = changes(
            &before,
            &after,
            |id| (id == 2).then_some(101),
            |id| id != 6 && id != 8,
        );
        let told: Vec<(u64, Event)> = told.into_iter().map(|one| (one.cub, one.event)).collect();
        assert_eq!(
            told,
            vec![
                (1, Event::PermissionPrompt),
                (2, Event::Fainted { exit: Some(101) }),
                (3, Event::FellAsleep),
                (4, Event::WentHome),
                // Not followed: the end of its turn is told from the list.
                (6, Event::TurnEnded),
                (8, Event::Joined),
                (8, Event::Mysterious),
                (5, Event::WentHome),
            ]
        );
        // Nothing changed, nothing told; and a followed session's turn is
        // told by its transcript, not twice.
        assert!(changes(&after, &after, |_| None, |_| true).is_empty());
        let turn = changes(
            &[lion(1, CubState::Thinking)],
            &[lion(1, CubState::WaitingForUser)],
            |_| None,
            |_| true,
        );
        assert!(turn.is_empty());
    }

    #[test]
    fn a_little_one_is_never_told_of_as_a_session() {
        let mut small = lion(9, CubState::Editing);
        small.parent = Some(1);
        let told = changes(
            &[],
            &[lion(1, CubState::Delegating), small.clone()],
            |_| None,
            |_| true,
        );
        assert_eq!(told.len(), 1, "only its parent joined");
        assert!(changes(&[small], &[], |_| None, |_| true).is_empty());
    }
}

#[cfg(test)]
mod past_tests {
    use super::*;

    #[test]
    fn the_past_of_a_session_is_its_tools_its_words_and_the_ends_of_its_turns() {
        let tool = |kind: ToolKind| Beat::ToolStarted {
            id: "t".into(),
            name: "Tool".into(),
            kind,
            detail: String::new(),
        };
        let beats = [
            Beat::Prompt,
            Beat::Thinking,
            tool(ToolKind::Read),
            Beat::ToolFinished {
                id: "t".into(),
                failed: false,
                refused: false,
            },
            tool(ToolKind::Run),
            // A question to the user is no tool.
            tool(ToolKind::Ask),
            Beat::Said {
                text: "Done.\n\nTwo files.".into(),
                at: Some(1_772_359_200),
            },
            Beat::Usage {
                context: 1,
                output: 1,
                window: None,
            },
            Beat::TurnEnded,
            Beat::Prompt,
            tool(ToolKind::Delegate),
            Beat::Interrupted,
        ];
        assert_eq!(
            past(&beats),
            vec![
                Past::Tool,
                Past::Tool,
                Past::Speech("Done.\n\nTwo files.".into(), Some(1_772_359_200)),
                Past::TurnEnded,
                Past::Tool,
                Past::TurnEnded,
            ]
        );
        assert!(past(&[]).is_empty());
        // The feed makes one line of the tools of each turn of it.
        let items = leon_den::feed::backfill(1, 1, "moss", None, &past(&beats), 40);
        let texts: Vec<&str> = items.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "MOSS used 2 tools.",
                "Done.\n\nTwo files.",
                "MOSS used a tool."
            ]
        );
    }
}

#[cfg(test)]
mod away_tests {
    use super::*;
    use gpui_kit::hsla;

    fn away(quiet: Option<u64>) -> Away {
        Away {
            id: away_id("claude", "0a1b"),
            name: "alpha".to_owned(),
            tint: hsla(0.1, 0.5, 0.5, 1.),
            place: Place::Terminal(None),
            pid: 70645,
            quiet_for: quiet.map(Duration::from_secs),
        }
    }

    fn pulse(beats: &[Beat]) -> Pulse {
        let mut pulse = Pulse::new();
        for beat in beats {
            pulse.apply(beat);
        }
        pulse
    }

    fn tool(id: &str, name: &str, kind: ToolKind, detail: &str) -> Beat {
        Beat::ToolStarted {
            id: id.into(),
            name: name.into(),
            kind,
            detail: detail.into(),
        }
    }

    const WHERE: &str = " \u{b7} runs in a terminal outside Leon (pid 70645)";

    #[test]
    fn the_state_of_a_session_elsewhere_is_what_its_transcript_says() {
        let state = |quiet: Option<u64>, beats: &[Beat]| {
            let (state, detail) = away_state(&away(quiet), &pulse(beats));
            let detail = detail.unwrap();
            assert!(detail.ends_with(WHERE), "{detail}");
            (state, detail.trim_end_matches(WHERE).to_owned())
        };
        let s = |text: &str| text.to_owned();
        // Nothing read yet.
        assert_eq!(
            state(None, &[]),
            (CubState::Idle, s("Nothing in its transcript yet"))
        );
        // A tool in flight is its kind, whatever the silence: a build is
        // silent too, so no permission prompt is claimed.
        let edit = [Beat::Prompt, tool("t1", "Edit", ToolKind::Edit, "shell.rs")];
        assert_eq!(
            state(Some(5), &edit),
            (CubState::Editing, s("Edit shell.rs"))
        );
        assert_eq!(
            state(Some(200), &edit),
            (CubState::Editing, s("Edit shell.rs \u{b7} quiet for 3 min"))
        );
        let run = [
            Beat::Prompt,
            tool("t1", "Bash", ToolKind::Run, "cargo test"),
        ];
        assert_eq!(
            state(Some(3 * 3600), &run),
            (CubState::Running, s("Bash cargo test \u{b7} quiet for 3 h"))
        );
        for quiet in [None, Some(0), Some(59), Some(60), Some(86_400)] {
            assert_ne!(state(quiet, &run).0, CubState::NeedsPermission);
            assert_ne!(state(quiet, &run).0, CubState::Asleep, "it is at work");
        }
        // A call that asks by its nature waits for the user.
        let asks = [
            Beat::Prompt,
            tool("t1", "AskUserQuestion", ToolKind::Ask, "Which one?"),
        ];
        assert_eq!(
            state(Some(400), &asks),
            (CubState::WaitingForUser, s("Asked a question: Which one?"))
        );
        let plan = [Beat::Prompt, tool("t1", "ExitPlanMode", ToolKind::Plan, "")];
        assert_eq!(
            state(None, &plan),
            (
                CubState::WaitingForUser,
                s("Waits for its plan to be approved")
            )
        );
        // No tool in flight, the turn not over.
        assert_eq!(
            state(None, &[Beat::Prompt]),
            (CubState::Thinking, s("Reading the message"))
        );
        assert_eq!(
            state(Some(120), &[Beat::Prompt, Beat::Thinking]),
            (CubState::Thinking, s("Thinking \u{b7} quiet for 2 min"))
        );
        // The turn over: waiting, and asleep after a long silence.
        let over = [Beat::Prompt, Beat::said("ok"), Beat::TurnEnded];
        assert_eq!(
            state(Some(30), &over),
            (CubState::WaitingForUser, s("Finished its turn"))
        );
        let nearly = ASLEEP_AFTER.as_secs() - 1;
        assert_eq!(state(Some(nearly), &over).0, CubState::WaitingForUser);
        assert_eq!(
            state(Some(ASLEEP_AFTER.as_secs()), &over),
            (
                CubState::Asleep,
                s("Finished its turn \u{b7} quiet for 15 min")
            )
        );
        // With no time of its file, it is never said to sleep.
        assert_eq!(state(None, &over).0, CubState::WaitingForUser);
        // No state of a session elsewhere is one only a terminal can tell.
        for beats in [&edit[..], &run, &asks, &over, &[]] {
            for quiet in [None, Some(10), Some(100_000)] {
                let found = state(quiet, beats).0;
                assert!(
                    !matches!(
                        found,
                        CubState::NeedsPermission
                            | CubState::Fainted
                            | CubState::Gone
                            | CubState::Mystery
                    ),
                    "{found:?}"
                );
            }
        }
    }

    #[test]
    fn the_card_of_a_session_elsewhere_says_where_it_runs() {
        let over = pulse(&[Beat::Prompt, Beat::TurnEnded]);
        let said = |place: Place| {
            away_state(
                &Away {
                    place,
                    ..away(None)
                },
                &over,
            )
            .1
            .unwrap()
        };
        assert_eq!(
            said(Place::OtherLeon),
            "Finished its turn \u{b7} runs in another Leon window (pid 70645)"
        );
        assert_eq!(
            said(Place::Terminal(Some("iTerm".into()))),
            "Finished its turn \u{b7} runs in iTerm, outside Leon (pid 70645)"
        );
        assert_eq!(
            said(Place::Terminal(None)),
            "Finished its turn \u{b7} runs in a terminal outside Leon (pid 70645)"
        );
    }

    #[test]
    fn its_lion_has_its_level_and_its_little_ones_and_is_no_mystery() {
        let mut reading = Reading::default();
        for beat in [
            Beat::Prompt,
            tool("t1", "Read", ToolKind::Read, "a.rs"),
            Beat::ToolFinished {
                id: "t1".into(),
                failed: false,
                refused: false,
            },
            tool("t2", "Task", ToolKind::Delegate, "explore: find the bug"),
        ] {
            reading.pulse.apply(&beat);
        }
        let one = away(Some(5));
        let cubs = away_cubs(&one, &reading);
        assert_eq!(cubs.len(), 2);
        assert_eq!(
            (cubs[0].id, cubs[0].name.as_str(), cubs[0].level),
            (one.id, "alpha", 2)
        );
        assert_eq!(cubs[0].state, CubState::Delegating);
        assert!(!cubs[0].mystery && cubs[0].parent.is_none());
        assert_eq!(cubs[1].parent, Some(one.id));
        assert_eq!(cubs[1].name, "explore");
        assert_eq!(cubs[1].id, little_id(one.id, "t2"));
    }

    #[test]
    fn its_id_is_its_own_and_its_name_is_the_agent_and_the_folder_when_nothing_better_is_known() {
        let id = away_id("claude", "0a1b");
        assert_eq!(id, away_id("claude", "0a1b"), "the same while it runs");
        assert_ne!(id, away_id("codex", "0a1b"));
        assert_ne!(id, away_id("claude", "0a1c"));
        assert!(is_away(id));
        // Not a live session of this window, not a little one.
        for live in [0u64, 1, 2, 500, u64::from(u32::MAX)] {
            assert!(!is_away(live));
        }
        assert!(!is_away(little_id(1, "t1")) && !is_away(little_id(id, "t1")));
        assert_eq!(
            away_name("Claude Code", Some("/srv/api")),
            "Claude Code in api"
        );
        assert_eq!(away_name("Codex", Some("/srv/api/")), "Codex in api");
        assert_eq!(away_name("Codex", Some("C:\\work\\api")), "Codex in api");
        assert_eq!(away_name("Codex", Some("/")), "Codex");
        assert_eq!(away_name("Codex", None), "Codex");
    }
}

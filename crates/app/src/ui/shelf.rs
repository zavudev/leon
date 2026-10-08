//! The sidebar's shelves and the undo, as pure rules.
//!
//! A session is in one of three places: in the list it belongs to (with the
//! Pinned section above it when it is pinned), on the Settled shelf, or on the
//! Snoozed shelf until a time. [`section`] decides which, from what the store
//! says ([`Shelf`]), whether the session is running and what time it is; the
//! clock is an argument, never read here. [`snooze_until`] turns what the person
//! typed ("2h", "tomorrow", "2026-10-12 14:00") into that time, [`woken`] says
//! what an event does to a snooze and [`settle_merged`] picks the sessions the
//! `sidebar_settle_merged` setting puts away.
//!
//! Undo is the same kind of rule: [`Undo`] holds what one step changed, and
//! [`Undo::restore`] says what puts it back. A terminal that was closed or put
//! to sleep cannot be brought back, so those steps are restored as a sleeping
//! row, and [`Undo::banner`] says so.

use super::dormant::Dormant;
use super::live::LiveId;
use super::model::Snapshot;
use super::tree::Placement;
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Utc};
use leon_core::{AgentId, MachineId, SessionId, Shelf};
use std::collections::{HashMap, HashSet};

/// How long the banner offers Undo.
pub const UNDO_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

/// The furthest a snooze can reach.
const LONGEST_SNOOZE_DAYS: i64 = 366;

/// The hour "tomorrow morning" is, on the person's clock.
const MORNING: u32 = 9;

/// Where a session is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    /// In its usual list (and the Pinned section, when it is pinned).
    Active,
    /// On the Settled shelf.
    Settled,
    /// On the Snoozed shelf.
    Snoozed,
}

/// A shelf of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShelfKind {
    /// Sessions hidden until a time.
    Snoozed,
    /// Sessions put away.
    Settled,
}

impl ShelfKind {
    /// What the shelf is called.
    pub fn title(self) -> &'static str {
        match self {
            ShelfKind::Snoozed => "Snoozed",
            ShelfKind::Settled => "Settled",
        }
    }

    /// A text that stays the same for the shelf, for the file that remembers
    /// what is open.
    pub fn tag(self) -> &'static str {
        match self {
            ShelfKind::Snoozed => "snoozed",
            ShelfKind::Settled => "settled",
        }
    }
}

/// Where a session is shown, given what the store says of it.
///
/// A settled session that is running is active: a shelf never hides what is
/// running (settling one is refused, so this is a session that was started from
/// the shelf). A snooze that has ended is no snooze, whatever the store still
/// says, and a running session stays snoozed until it needs the person.
pub fn section(state: Option<&Shelf>, running: bool, now: DateTime<Utc>) -> Section {
    match state {
        Some(Shelf::Settled) if !running => Section::Settled,
        Some(Shelf::Snoozed(until)) if *until > now => Section::Snoozed,
        Some(Shelf::Settled | Shelf::Snoozed(_) | Shelf::Returned) | None => Section::Active,
    }
}

/// Why a session cannot be settled, or `None` when it can.
pub fn cannot_settle(running: bool) -> Option<&'static str> {
    running.then_some("A running session is not settled: put it to sleep or close it first.")
}

/// What a snooze becomes when the session needs the person, fails or finishes:
/// `Some` is the state to write. Only a snooze that is still on wakes; the
/// state it leaves is `Returned`, so the session is not put away again behind
/// the person's back.
pub fn woken(state: Option<&Shelf>, now: DateTime<Utc>) -> Option<Shelf> {
    match state {
        Some(Shelf::Snoozed(until)) if *until > now => Some(Shelf::Returned),
        _ => None,
    }
}

/// When the next snooze ends, for the timer that brings it back.
pub fn next_wake<'a>(
    shelves: impl IntoIterator<Item = &'a Shelf>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    shelves
        .into_iter()
        .filter_map(|shelf| match shelf {
            Shelf::Snoozed(until) if *until > now => Some(*until),
            _ => None,
        })
        .min()
}

/// The sessions to settle because their worktree's pull request is merged: not
/// pinned, not running, and never put anywhere by hand (a session with a row
/// on the shelves, even a returned one, is left alone). Newest first, as the
/// snapshot lists them.
pub fn settle_merged(
    snapshot: &Snapshot,
    placement: &Placement,
    shelves: &HashMap<SessionId, Shelf>,
    running: &HashSet<SessionId>,
) -> Vec<SessionId> {
    let mut places: Vec<usize> = snapshot
        .projects
        .iter()
        .flat_map(|entry| &entry.worktrees)
        .filter(|worktree| worktree.merged_pull_request == Some(true) && !worktree.is_main)
        .flat_map(|worktree| placement.of_worktree(&worktree.id).iter().copied())
        .collect();
    places.sort_unstable();
    places
        .into_iter()
        .map(|place| &snapshot.sessions[place])
        .filter(|session| {
            session.sort_order.is_none()
                && !shelves.contains_key(&session.id)
                && !running.contains(&session.id)
        })
        .map(|session| session.id.clone())
        .collect()
}

// ----- snoozing ------------------------------------------------------------------

/// The quick choices the palette offers: the label, what it does, and the text
/// [`snooze_until`] reads for it.
pub const PRESETS: [(&str, &str, &str); 4] = [
    ("In an hour", "back at the same minute, an hour on", "1h"),
    ("In three hours", "later today", "3h"),
    ("Tomorrow morning", "at 09:00", "tomorrow"),
    ("Next week", "Monday at 09:00", "next week"),
];

/// What to type when none of the presets fits.
pub const SNOOZE_HELP: &str = "Say when: 45m, 2h, 3 days, tomorrow, next week or 2026-10-12 14:00.";

/// The time a snooze ends, from what was typed. `offset` is the person's
/// distance from UTC: "tomorrow" and a date without a zone are on their clock.
///
/// Understood: a length of time ("1h", "90 minutes", "in an hour", "2d 3h"),
/// "tomorrow" or "tomorrow morning" (09:00 tomorrow), "next week" (Monday at
/// 09:00) and a date, with or without a time ("2026-10-12", "2026-10-12
/// 14:00"; a date alone is 09:00). The time must be ahead, and within a year.
pub fn snooze_until(
    text: &str,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Result<DateTime<Utc>, String> {
    let text = text.trim().to_lowercase();
    let text = text.strip_prefix("in ").unwrap_or(&text).trim();
    if text.is_empty() {
        return Err(SNOOZE_HELP.to_owned());
    }
    let until = match text {
        "tomorrow" | "tomorrow morning" => {
            let today = now.with_timezone(&offset).date_naive();
            at_morning(today + Duration::days(1), offset)
        }
        "next week" => {
            // The Monday after today: a whole week when today is one.
            let today = now.with_timezone(&offset).date_naive();
            let ahead = 7 - i64::from(today.weekday().num_days_from_monday());
            at_morning(today + Duration::days(ahead), offset)
        }
        _ => match dated(text, offset) {
            Some(until) => until,
            None => Some(now + length(text)?),
        },
    }
    .ok_or_else(|| SNOOZE_HELP.to_owned())?;
    if until <= now {
        return Err("That time has passed.".to_owned());
    }
    if until - now > Duration::days(LONGEST_SNOOZE_DAYS) {
        return Err("A snooze lasts a year at most.".to_owned());
    }
    Ok(until)
}

/// 09:00 of `day` on the person's clock.
fn at_morning(day: NaiveDate, offset: FixedOffset) -> Option<DateTime<Utc>> {
    let morning = day.and_hms_opt(MORNING, 0, 0)?;
    offset
        .from_local_datetime(&morning)
        .single()
        .map(|local| local.with_timezone(&Utc))
}

/// A date, or a date and a time, on the person's clock.
fn dated(text: &str, offset: FixedOffset) -> Option<Option<DateTime<Utc>>> {
    let (day, time) = match text.split_once([' ', 't']) {
        Some((day, time)) => (day, Some(time.trim())),
        None => (text, None),
    };
    let mut parts = day.split('-');
    let (year, month, date) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let day = NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, date.parse().ok()?)?;
    let Some(time) = time else {
        return Some(at_morning(day, offset));
    };
    let (hour, minute) = time.split_once(':')?;
    let moment = day.and_hms_opt(hour.trim().parse().ok()?, minute.trim().parse().ok()?, 0)?;
    Some(
        offset
            .from_local_datetime(&moment)
            .single()
            .map(|local| local.with_timezone(&Utc)),
    )
}

/// A length of time: pairs of a number (or "a", "an") and a unit, with or
/// without spaces between them.
fn length(text: &str) -> Result<Duration, String> {
    let mut total = Duration::zero();
    let mut rest = text;
    let mut any = false;
    while !rest.is_empty() {
        let split = rest
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| SNOOZE_HELP.to_owned())?;
        let (digits, tail) = rest.split_at(split);
        let tail = tail.trim_start();
        let (count, tail) = if digits.is_empty() {
            let word = tail
                .split(|c: char| !c.is_alphabetic())
                .next()
                .unwrap_or_default();
            match word {
                "a" | "an" => (1, tail[word.len()..].trim_start()),
                _ => return Err(SNOOZE_HELP.to_owned()),
            }
        } else {
            (
                digits.parse::<i64>().map_err(|_| SNOOZE_HELP.to_owned())?,
                tail,
            )
        };
        let unit_end = tail
            .find(|c: char| !c.is_alphabetic())
            .unwrap_or(tail.len());
        let (unit, next) = tail.split_at(unit_end);
        let unit = match unit {
            "m" | "min" | "mins" | "minute" | "minutes" => Duration::minutes(1),
            "h" | "hr" | "hrs" | "hour" | "hours" => Duration::hours(1),
            "d" | "day" | "days" => Duration::days(1),
            "w" | "week" | "weeks" => Duration::weeks(1),
            _ => return Err(SNOOZE_HELP.to_owned()),
        };
        if count > LONGEST_SNOOZE_DAYS * 24 * 60 {
            return Err("A snooze lasts a year at most.".to_owned());
        }
        total += unit * count as i32;
        any = true;
        rest = next.trim_start_matches([' ', ',']);
        rest = rest.strip_prefix("and ").unwrap_or(rest);
    }
    if any {
        Ok(total)
    } else {
        Err(SNOOZE_HELP.to_owned())
    }
}

/// How long until a snooze ends, as one short token, rounded up: `20m`, `3h`,
/// `4d`.
pub fn back_in(until: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = ((until - now).num_seconds().max(0) + 59) / 60;
    match minutes {
        0..=59 => format!("{}m", minutes.max(1)),
        60..=2_879 => format!("{}h", (minutes + 59) / 60),
        _ => format!("{}d", (minutes + 1_439) / 1_440),
    }
}

/// The moment a snooze ends, on the person's clock, to the minute.
pub fn back_at(until: DateTime<Utc>, offset: FixedOffset) -> String {
    until
        .with_timezone(&offset)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

// ----- undo ----------------------------------------------------------------------

/// A session whose terminal ended, as far as it can be remembered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gone {
    /// The machine it ran on.
    pub machine: MachineId,
    /// The folder it ran in.
    pub cwd: String,
    /// The agent it ran, or `None` for a plain shell.
    pub agent: Option<AgentId>,
    /// The name its row had.
    pub label: Option<String>,
    /// The id of the account it ran as; `None` is the agent's own setup.
    pub account: Option<String>,
    /// The history session that was its row, when it had one.
    pub history: Option<SessionId>,
    /// The sleeping record that stands for it, when it has none in the history.
    pub sleeper: Option<LiveId>,
}

/// What one step changed, enough to take it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Undo {
    /// A whole session was closed: its terminal ended and its row left.
    Closed(Gone),
    /// A whole session was put to sleep: its terminal ended, its row stayed.
    Slept(Gone),
    /// A sleeping row without a history session was closed.
    ClosedSleeper(Dormant),
    /// A session was settled, snoozed or brought back.
    Shelved {
        /// The session.
        session: SessionId,
        /// Its row before: `None` for a session never put anywhere.
        was: Option<Shelf>,
        /// Its row now.
        to: Option<Shelf>,
    },
    /// A session was unpinned; `order` is the Pinned section of its machine as
    /// it was.
    Unpinned {
        /// The machine.
        machine: MachineId,
        /// The pinned sessions, first row first.
        order: Vec<SessionId>,
    },
}

/// What puts a step back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// Write these rows on the shelves.
    Shelves(Vec<(SessionId, Option<Shelf>)>),
    /// Pin these sessions on the machine, in this order.
    Pins {
        /// The machine.
        machine: MachineId,
        /// The pinned sessions, first row first.
        order: Vec<SessionId>,
    },
    /// Put this sleeping record back, with the number it had.
    Sleeper(Dormant),
    /// Add a sleeping row for a session that had none to go back to (a closed
    /// terminal was not a record), after the others.
    Asleep(Gone),
    /// Keep the history session, as a sleeping row.
    Sleeping(SessionId),
    /// Start a sleeping row again: the sleeping record, or the history session.
    Wake {
        /// The sleeping record.
        sleeper: Option<LiveId>,
        /// The history session.
        history: Option<SessionId>,
    },
    /// There is nothing left to restore.
    Nothing,
}

impl Undo {
    /// What puts the step back. `pinned` is the Pinned section of the machine
    /// now and `known` says whether the store still has a session.
    pub fn restore(&self, pinned: &[SessionId], known: impl Fn(&SessionId) -> bool) -> Restore {
        match self {
            Undo::Shelved { session, was, to } if known(session) => {
                // A session never put anywhere goes back to its list as one
                // taken back by hand: with no row at all the automatic settling
                // would take it straight away again.
                let back = match (was, to) {
                    (None, Some(Shelf::Settled | Shelf::Snoozed(_))) => Some(Shelf::Returned),
                    _ => *was,
                };
                Restore::Shelves(vec![(session.clone(), back)])
            }
            Undo::Shelved { .. } => Restore::Nothing,
            Undo::Unpinned { machine, order } => Restore::Pins {
                machine: machine.clone(),
                order: restored_pins(order, pinned, known),
            },
            Undo::ClosedSleeper(dormant) => Restore::Sleeper(dormant.clone()),
            Undo::Closed(gone) => match &gone.history {
                Some(history) if known(history) => Restore::Sleeping(history.clone()),
                _ => Restore::Asleep(gone.clone()),
            },
            Undo::Slept(gone) => Restore::Wake {
                sleeper: gone.sleeper,
                history: gone.history.clone(),
            },
        }
    }

    /// What the banner says of the step, `title` being the name of the session
    /// and `offset` the person's clock. The program of a terminal cannot be
    /// brought back: the banner says what comes back instead.
    pub fn banner(&self, title: &str, offset: FixedOffset) -> String {
        match self {
            Undo::Closed(_) => format!(
                "Closed {title}. Its program cannot be brought back; Undo restores the row, asleep."
            ),
            Undo::Slept(_) => format!("Put {title} to sleep. Undo wakes it, with a new program."),
            Undo::ClosedSleeper(_) => format!("Closed the sleeping {title}."),
            Undo::Shelved { to, .. } => match to {
                Some(Shelf::Settled) => format!("Settled {title}."),
                Some(Shelf::Snoozed(until)) => {
                    format!("Snoozed {title} until {}.", back_at(*until, offset))
                }
                Some(Shelf::Returned) | None => format!("Brought {title} back."),
            },
            Undo::Unpinned { .. } => format!("Unpinned {title}."),
        }
    }
}

/// The person's distance from UTC, for "tomorrow morning" and for dates typed
/// without a zone. A test build says UTC, so no test depends on the computer's
/// time zone.
pub fn local_offset() -> FixedOffset {
    #[cfg(test)]
    {
        FixedOffset::east_opt(0).expect("zero is an offset")
    }
    #[cfg(not(test))]
    {
        chrono::Offset::fix(chrono::Local::now().offset())
    }
}

/// The Pinned section after an unpin is undone: the order it had, without the
/// sessions that are gone, then whatever was pinned since, in its order.
pub fn restored_pins(
    before: &[SessionId],
    now: &[SessionId],
    known: impl Fn(&SessionId) -> bool,
) -> Vec<SessionId> {
    let mut out: Vec<SessionId> = before.iter().filter(|id| known(id)).cloned().collect();
    for id in now {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

/// The banner on screen, and when it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending<T> {
    /// What it takes back.
    pub undo: Undo,
    /// What the banner says.
    pub text: String,
    /// When it goes.
    pub until: T,
    /// The history sessions whose removal waits for the banner to go.
    pub forget: Vec<SessionId>,
}

impl<T: PartialOrd> Pending<T> {
    /// Whether the window to undo is over at `now`.
    pub fn over(&self, now: &T) -> bool {
        self.until <= *now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::{Project, ProjectId, Session, Worktree, WorktreeId};
    use std::time::Duration as Wait;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, minute, 0)
            .unwrap()
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn id(name: &str) -> SessionId {
        SessionId::from_string(name)
    }

    #[test]
    fn a_settled_session_is_on_its_shelf_unless_it_is_running() {
        let now = at(7, 12, 0);
        assert_eq!(section(Some(&Shelf::Settled), false, now), Section::Settled);
        assert_eq!(section(Some(&Shelf::Settled), true, now), Section::Active);
        assert_eq!(section(Some(&Shelf::Returned), false, now), Section::Active);
        assert_eq!(section(None, false, now), Section::Active);
    }

    #[test]
    fn a_snooze_hides_the_session_until_its_time_even_while_it_runs() {
        let until = at(7, 13, 0);
        let snoozed = Shelf::Snoozed(until);
        assert_eq!(
            section(Some(&snoozed), false, at(7, 12, 59)),
            Section::Snoozed
        );
        assert_eq!(
            section(Some(&snoozed), true, at(7, 12, 59)),
            Section::Snoozed
        );
        assert_eq!(section(Some(&snoozed), false, until), Section::Active);
        assert_eq!(section(Some(&snoozed), false, at(8, 0, 0)), Section::Active);
    }

    #[test]
    fn only_a_snooze_that_is_still_on_wakes_early_and_it_leaves_the_session_returned() {
        let now = at(7, 12, 0);
        assert_eq!(
            woken(Some(&Shelf::Snoozed(at(7, 13, 0))), now),
            Some(Shelf::Returned)
        );
        assert_eq!(woken(Some(&Shelf::Snoozed(at(7, 11, 0))), now), None);
        assert_eq!(woken(Some(&Shelf::Settled), now), None);
        assert_eq!(woken(None, now), None);
    }

    #[test]
    fn the_next_wake_is_the_nearest_snooze_still_ahead() {
        let now = at(7, 12, 0);
        let shelves = [
            Shelf::Snoozed(at(7, 18, 0)),
            Shelf::Snoozed(at(7, 13, 0)),
            Shelf::Snoozed(at(7, 9, 0)),
            Shelf::Settled,
        ];
        assert_eq!(next_wake(&shelves, now), Some(at(7, 13, 0)));
        assert_eq!(next_wake(&[Shelf::Settled], now), None);
    }

    #[test]
    fn lengths_of_time_are_read_in_the_ways_people_write_them() {
        let now = at(7, 12, 0);
        let read = |text: &str| snooze_until(text, now, utc());
        assert_eq!(read("1h"), Ok(at(7, 13, 0)));
        assert_eq!(read("in an hour"), Ok(at(7, 13, 0)));
        assert_eq!(read("90 minutes"), Ok(at(7, 13, 30)));
        assert_eq!(read("45m"), Ok(at(7, 12, 45)));
        assert_eq!(read("2d 3h"), Ok(at(9, 15, 0)));
        assert_eq!(read("1h30m"), Ok(at(7, 13, 30)));
        assert_eq!(read("2 days and 1 hour"), Ok(at(9, 13, 0)));
        assert_eq!(read("1 week"), Ok(at(14, 12, 0)));
        assert_eq!(read("  3 HOURS "), Ok(at(7, 15, 0)));
    }

    #[test]
    fn tomorrow_and_next_week_are_mornings_on_the_persons_clock() {
        // Wednesday 7 October 2026, 23:30 in UTC is already Thursday in +02:00.
        let now = at(7, 23, 30);
        let east = FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(snooze_until("tomorrow", now, east), Ok(at(9, 7, 0)));
        assert_eq!(snooze_until("tomorrow", now, utc()), Ok(at(8, 9, 0)));
        assert_eq!(
            snooze_until("Tomorrow morning", now, utc()),
            Ok(at(8, 9, 0))
        );
        // The next Monday after Wednesday the 7th is the 12th; after a Monday,
        // the Monday a week on.
        assert_eq!(snooze_until("next week", now, utc()), Ok(at(12, 9, 0)));
        assert_eq!(
            snooze_until("next week", at(12, 8, 0), utc()),
            Ok(at(19, 9, 0))
        );
    }

    #[test]
    fn a_date_is_on_the_persons_clock_and_defaults_to_the_morning() {
        let now = at(7, 12, 0);
        let west = FixedOffset::west_opt(5 * 3600).unwrap();
        assert_eq!(snooze_until("2026-10-12", now, utc()), Ok(at(12, 9, 0)));
        assert_eq!(
            snooze_until("2026-10-12 14:30", now, utc()),
            Ok(at(12, 14, 30))
        );
        assert_eq!(
            snooze_until("2026-10-12T14:30", now, west),
            Ok(at(12, 19, 30))
        );
    }

    #[test]
    fn a_time_that_makes_no_sense_is_refused_with_a_reason() {
        let now = at(7, 12, 0);
        let refused = |text: &str| snooze_until(text, now, utc()).unwrap_err();
        assert_eq!(refused(""), SNOOZE_HELP);
        assert_eq!(refused("soon"), SNOOZE_HELP);
        assert_eq!(refused("5"), SNOOZE_HELP);
        assert_eq!(refused("3 parsecs"), SNOOZE_HELP);
        assert_eq!(refused("2026-13-40"), SNOOZE_HELP);
        assert_eq!(refused("2026-10-12 25:00"), SNOOZE_HELP);
        assert_eq!(refused("2026-10-06"), "That time has passed.");
        assert_eq!(refused("2026-10-07 11:00"), "That time has passed.");
        assert_eq!(refused("2027-12-01"), "A snooze lasts a year at most.");
        assert_eq!(refused("99999999 d"), "A snooze lasts a year at most.");
    }

    #[test]
    fn every_preset_is_understood() {
        let now = at(7, 12, 0);
        for (label, _, value) in PRESETS {
            assert!(snooze_until(value, now, utc()).is_ok(), "{label}");
        }
    }

    #[test]
    fn the_time_left_reads_as_one_short_token() {
        let now = at(7, 12, 0);
        assert_eq!(back_in(at(7, 12, 20), now), "20m");
        assert_eq!(back_in(at(7, 15, 0), now), "3h");
        assert_eq!(back_in(at(7, 15, 1), now), "4h");
        assert_eq!(back_in(at(9, 12, 0), now), "2d");
        assert_eq!(back_in(at(7, 12, 0), now), "1m");
        assert_eq!(back_at(at(12, 14, 30), utc()), "2026-10-12 14:30");
    }

    fn worktree(id: &str, merged: Option<bool>, main: bool) -> Worktree {
        Worktree {
            id: WorktreeId::from_string(id),
            project_id: ProjectId::from_string("api"),
            path: format!("/srv/{id}"),
            branch: Some(id.to_owned()),
            head: None,
            is_main: main,
            merged_pull_request: merged,
        }
    }

    fn session(name: &str, cwd: &str, pinned: bool) -> Session {
        Session {
            id: id(name),
            agent: AgentId::CLAUDE,
            external_id: name.to_owned(),
            machine_id: MachineId::local(),
            cwd: cwd.to_owned(),
            project_id: None,
            title: name.to_owned(),
            model: None,
            started_at: at(1, 0, 0),
            updated_at: at(1, 0, 0),
            message_count: 1,
            sort_order: pinned.then_some(0),
            account: None,
        }
    }

    fn snapshot(sessions: Vec<Session>) -> Snapshot {
        Snapshot {
            machines: vec![leon_core::Machine {
                id: MachineId::local(),
                name: "This machine".to_owned(),
                kind: leon_core::MachineKind::Local,
            }],
            projects: vec![super::super::model::ProjectEntry {
                project: Project {
                    id: ProjectId::from_string("api"),
                    machine_id: MachineId::local(),
                    name: "api".to_owned(),
                    root: "/srv/main".to_owned(),
                },
                worktrees: vec![
                    worktree("main", None, true),
                    worktree("done", Some(true), false),
                    worktree("open", Some(false), false),
                    worktree("unknown", None, false),
                ],
            }],
            sessions,
            ..Snapshot::default()
        }
    }

    #[test]
    fn merged_work_is_settled_unless_it_is_pinned_running_or_was_put_somewhere_by_hand() {
        let snapshot = snapshot(vec![
            session("plain", "/srv/done", false),
            session("pinned", "/srv/done", true),
            session("running", "/srv/done", false),
            session("returned", "/srv/done", false),
            session("snoozed", "/srv/done", false),
            session("other-open", "/srv/open", false),
            session("other-unknown", "/srv/unknown", false),
            session("on-main", "/srv/main", false),
            session("deeper", "/srv/done/crates/app", false),
        ]);
        let placement = Placement::compute(&snapshot);
        let shelves = HashMap::from([
            (id("returned"), Shelf::Returned),
            (id("snoozed"), Shelf::Snoozed(at(9, 0, 0))),
        ]);
        let running = HashSet::from([id("running")]);
        assert_eq!(
            settle_merged(&snapshot, &placement, &shelves, &running),
            [id("plain"), id("deeper")]
        );
        assert!(settle_merged(&snapshot, &placement, &HashMap::new(), &HashSet::new()).len() > 2);
    }

    #[test]
    fn settling_a_running_session_is_refused_with_a_reason() {
        assert!(cannot_settle(true).is_some());
        assert_eq!(cannot_settle(false), None);
    }

    fn gone(history: Option<&str>) -> Gone {
        Gone {
            machine: MachineId::local(),
            cwd: "/srv/api".to_owned(),
            agent: Some(AgentId::CLAUDE),
            label: Some("build".to_owned()),
            account: None,
            history: history.map(id),
            sleeper: None,
        }
    }

    #[test]
    fn undoing_a_shelf_puts_back_exactly_the_row_it_had() {
        let known = |_: &SessionId| true;
        let was = Some(Shelf::Snoozed(at(9, 0, 0)));
        let undo = Undo::Shelved {
            session: id("a"),
            was,
            to: Some(Shelf::Settled),
        };
        assert_eq!(
            undo.restore(&[], known),
            Restore::Shelves(vec![(id("a"), was)])
        );
        assert_eq!(undo.restore(&[], |_| false), Restore::Nothing);
    }

    #[test]
    fn undoing_the_first_settle_or_snooze_leaves_the_session_returned_so_it_is_not_settled_again() {
        // With no row the automatic settling would take the session again at
        // once; a returned one is left alone, which is where the person put it.
        for to in [Shelf::Settled, Shelf::Snoozed(at(9, 0, 0))] {
            let never = Undo::Shelved {
                session: id("a"),
                was: None,
                to: Some(to),
            };
            assert_eq!(
                never.restore(&[], |_| true),
                Restore::Shelves(vec![(id("a"), Some(Shelf::Returned))])
            );
        }
        // Undoing a return to a session that had no row clears nothing else.
        let returned = Undo::Shelved {
            session: id("a"),
            was: None,
            to: Some(Shelf::Returned),
        };
        assert_eq!(
            returned.restore(&[], |_| true),
            Restore::Shelves(vec![(id("a"), None)])
        );
    }

    #[test]
    fn undoing_an_unpin_puts_the_session_back_in_its_old_place() {
        let undo = Undo::Unpinned {
            machine: MachineId::local(),
            order: vec![id("a"), id("b"), id("c")],
        };
        // "b" was unpinned; meanwhile "d" was pinned and "c" is gone.
        let now = [id("a"), id("d")];
        let Restore::Pins { order, .. } = undo.restore(&now, |s| s.as_str() != "c") else {
            panic!("pins");
        };
        assert_eq!(order, [id("a"), id("b"), id("d")]);
    }

    #[test]
    fn undoing_a_close_keeps_the_row_asleep_and_says_the_program_is_gone() {
        let with_history = Undo::Closed(gone(Some("h")));
        assert_eq!(
            with_history.restore(&[], |_| true),
            Restore::Sleeping(id("h"))
        );
        let banner = with_history.banner("build", utc());
        assert!(banner.contains("cannot be brought back"), "{banner}");
        assert!(banner.contains("asleep"), "{banner}");
        // The history session was forgotten meanwhile: a new sleeping row
        // stands for it, as the terminal had no record to go back to.
        let Restore::Asleep(record) = with_history.restore(&[], |_| false) else {
            panic!("a new sleeping row");
        };
        assert_eq!(record.cwd, "/srv/api");
        assert_eq!(record.agent.map(AgentId::as_str), Some("claude"));
        assert_eq!(record.label.as_deref(), Some("build"));
        let shell = Undo::Closed(Gone {
            agent: None,
            history: None,
            ..gone(None)
        });
        assert!(matches!(
            shell.restore(&[], |_| true),
            Restore::Asleep(Gone { agent: None, .. })
        ));
        assert!(shell
            .banner("a shell", utc())
            .contains("cannot be brought back"));
    }

    #[test]
    fn undoing_a_sleep_wakes_the_session_and_the_banner_says_the_program_is_new() {
        let slept = Undo::Slept(Gone {
            sleeper: Some(LiveId(u64::MAX - 3)),
            ..gone(None)
        });
        assert_eq!(
            slept.restore(&[], |_| true),
            Restore::Wake {
                sleeper: Some(LiveId(u64::MAX - 3)),
                history: None
            }
        );
        assert!(slept.banner("build", utc()).contains("new program"));
    }

    #[test]
    fn closing_a_sleeping_row_is_undone_with_the_number_it_had() {
        let record = Dormant {
            number: 4,
            machine: "local".to_owned(),
            cwd: "/srv/api".to_owned(),
            agent: None,
            label: None,
            account: None,
        };
        assert_eq!(
            Undo::ClosedSleeper(record.clone()).restore(&[], |_| true),
            Restore::Sleeper(record)
        );
    }

    #[test]
    fn every_banner_says_what_was_done_to_the_named_session() {
        let shelved = |to| Undo::Shelved {
            session: id("a"),
            was: None,
            to,
        };
        let say = |undo: Undo| undo.banner("build", utc());
        assert_eq!(say(shelved(Some(Shelf::Settled))), "Settled build.");
        assert_eq!(
            say(shelved(Some(Shelf::Snoozed(at(12, 14, 30))))),
            "Snoozed build until 2026-10-12 14:30."
        );
        assert_eq!(say(shelved(Some(Shelf::Returned))), "Brought build back.");
        assert_eq!(say(shelved(None)), "Brought build back.");
        assert_eq!(
            say(Undo::Unpinned {
                machine: MachineId::local(),
                order: Vec::new()
            }),
            "Unpinned build."
        );
        assert!(say(Undo::Closed(gone(None))).starts_with("Closed build."));
        assert!(say(Undo::Slept(gone(None))).starts_with("Put build to sleep."));
    }

    #[test]
    fn the_window_to_undo_is_over_exactly_when_it_ends() {
        let pending = Pending {
            undo: Undo::Closed(gone(None)),
            text: String::new(),
            until: Wait::from_secs(5),
            forget: Vec::new(),
        };
        assert!(!pending.over(&Wait::from_millis(4_999)));
        assert!(pending.over(&Wait::from_secs(5)));
        assert_eq!(UNDO_WINDOW, Wait::from_secs(5));
    }
}

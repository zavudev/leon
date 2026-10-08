//! The feed: what was told and what was said, newest at the bottom.
//!
//! The feed is the Den's log. It holds two kinds of entry, and keeps them
//! apart on screen:
//!
//! * what the **narrator** says of a lion (the comic voice of
//!   `brand/voice.md`, or its plain twin when the narrator is off), and the
//!   one line that sums up the tools of a past turn;
//! * what the **agent itself said** to the user: the text of its message as
//!   it is in the transcript, shown as speech. Nothing of it is rewritten;
//!   it is wrapped, and a long one is cut to [`CLAMP_ROWS`] rows until it is
//!   opened.
//!
//! Everything here is plain data and pure functions: an entry carries its
//! time (the host says what time it is, [`Feed`] never reads a clock), the
//! rows are laid out for a width in characters ([`lay`]), and what is in view
//! is a function of the rows and of where the reader is ([`Feed::window`]).
//!
//! # How much is kept
//!
//! At most [`TOTAL`] entries, the oldest dropped first. When the Den opens
//! on sessions that have a past, each is given its last [`PER_SESSION`]
//! entries ([`backfill`]): its messages as speech, and one line for the
//! tools of each turn rather than one for each tool.
//!
//! # Following
//!
//! The feed follows its end until the reader goes back; then it stays where
//! it was put, counts what arrives meanwhile, and follows again when the
//! reader comes back to the end.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use gpui_kit::Hsla;

use crate::narrator::{shout, TICK, TYPED_PER_TICK};

/// How many entries the feed keeps.
pub const TOTAL: usize = 500;
/// How many entries of its past a session is given when it is first read.
pub const PER_SESSION: usize = 40;
/// How many rows of a message are shown before it is opened.
pub const CLAMP_ROWS: usize = 4;
/// Entries of one lion that follow each other within this many seconds
/// share a heading.
pub const SAME_BREATH: i64 = 120;

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The narrator. `urgent` carries a status colour and a glyph: a
    /// permission prompt, a faint.
    Narration {
        /// It must be read.
        urgent: bool,
    },
    /// What the agent said to the user, in its own words.
    Speech,
}

/// One thing to put in the feed.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The lion it is about.
    pub cub: u64,
    /// The lion whose feed it belongs to: itself, or the one that sent it
    /// out for a little one.
    pub owner: u64,
    /// The lion's name, as the session is called.
    pub name: String,
    /// The colour of its mane, if the den knows the lion.
    pub tint: Option<Hsla>,
    /// When, in seconds since the Unix epoch, if that is known.
    pub at: Option<i64>,
    /// What it is.
    pub kind: Kind,
    /// The text: the narrator's line, or the agent's message.
    pub text: String,
    /// The same fact in plain words, for a den whose narrator is off. Empty
    /// for speech, which is plain already.
    pub plain: String,
}

/// An entry of the feed.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// A number of its own, in the order of arrival.
    pub id: u64,
    /// What it says.
    pub item: Item,
    /// How many times in a row the same line was said.
    pub count: u32,
    /// When it arrived, on the den's own clock: what its typing is timed
    /// from. `None` for what was there before the Den opened.
    pub born: Option<Duration>,
}

/// The log.
#[derive(Clone, Debug, Default)]
pub struct Feed {
    entries: VecDeque<Entry>,
    next_id: u64,
    /// The messages that were opened.
    open: HashSet<u64>,
    /// The first row in view, when the reader went back; `None` follows.
    top: Option<usize>,
    /// How many entries arrived since the reader went back.
    unseen: usize,
    /// The entry the keyboard is on.
    cursor: Option<u64>,
}

impl Feed {
    /// An empty feed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every entry, the oldest first.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.entries.iter()
    }

    /// How many entries there are.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there is none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn trim(&mut self) {
        while self.entries.len() > TOTAL {
            if let Some(gone) = self.entries.pop_front() {
                self.open.remove(&gone.id);
                if self.cursor == Some(gone.id) {
                    self.cursor = None;
                }
            }
        }
    }

    /// Adds what just happened at the end. The same narrator's line about
    /// the same lion twice in a row is one entry that counts. It answers
    /// the entry's id.
    pub fn push(&mut self, item: Item, born: Duration) -> u64 {
        if let Some(last) = self.entries.back_mut() {
            let same = matches!(item.kind, Kind::Narration { urgent: false })
                && last.item.kind == item.kind
                && last.item.cub == item.cub
                && last.item.text == item.text;
            if same {
                last.count += 1;
                last.item.at = item.at.or(last.item.at);
                return last.id;
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push_back(Entry {
            id,
            item,
            count: 1,
            born: Some(born),
        });
        if self.top.is_some() {
            self.unseen += 1;
        }
        self.trim();
        id
    }

    /// Puts entries of the past where their time says: after everything
    /// that is not later. One without a time goes where the one before it
    /// went, or at the end. They are not typed out and count as seen.
    pub fn insert_past(&mut self, items: Vec<Item>) {
        let mut after: Option<usize> = None;
        for item in items {
            let place = match (item.at, after) {
                (Some(at), _) => {
                    // The time of an entry without one is that of the one
                    // before it.
                    let mut known = i64::MIN;
                    let mut place = 0;
                    for (index, entry) in self.entries.iter().enumerate() {
                        known = entry.item.at.unwrap_or(known);
                        if known <= at {
                            place = index + 1;
                        }
                    }
                    place.max(after.unwrap_or(0))
                }
                (None, Some(after)) => after,
                (None, None) => self.entries.len(),
            };
            let id = self.next_id;
            self.next_id += 1;
            self.entries.insert(
                place,
                Entry {
                    id,
                    item,
                    count: 1,
                    born: None,
                },
            );
            after = Some(place + 1);
        }
        self.trim();
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The newest entry, if the narrator is still typing it at `now`: its
    /// id and how many characters of its text are on screen.
    pub fn typing(&self, now: Duration) -> Option<(u64, usize)> {
        let last = self.entries.back()?;
        let born = last.born?;
        if !matches!(last.item.kind, Kind::Narration { .. }) || last.count > 1 {
            return None;
        }
        let total = last.item.text.chars().count();
        let ticks = (now.saturating_sub(born).as_millis() / TICK.as_millis()) as usize + 1;
        let typed = ticks * TYPED_PER_TICK;
        (typed < total).then_some((last.id, typed))
    }

    /// When the typing of the newest entry shows more, if it is not done.
    pub fn next_typed(&self, now: Duration) -> Option<Duration> {
        let born = self.entries.back()?.born?;
        self.typing(now)?;
        let ticks = now.saturating_sub(born).as_millis() / TICK.as_millis() + 1;
        Some(born + TICK * ticks as u32)
    }

    /// Whether a message was opened.
    pub fn is_open(&self, id: u64) -> bool {
        self.open.contains(&id)
    }

    /// Opens a message that is cut, or cuts it again.
    pub fn toggle(&mut self, id: u64) {
        if !self.open.remove(&id) {
            self.open.insert(id);
        }
    }

    /// Whether the feed follows its end.
    pub fn follows(&self) -> bool {
        self.top.is_none()
    }

    /// How many entries arrived since the reader went back.
    pub fn unseen(&self) -> usize {
        self.unseen
    }

    /// The entry the keyboard is on.
    pub fn cursor(&self) -> Option<u64> {
        self.cursor
    }

    /// Goes to the end and follows it.
    pub fn follow(&mut self) {
        self.top = None;
        self.unseen = 0;
        self.cursor = None;
    }

    /// The first of `height` rows in view, of `total`.
    pub fn window(&self, total: usize, height: usize) -> usize {
        let last = total.saturating_sub(height);
        self.top.map_or(last, |top| top.min(last))
    }

    /// Moves the view by `rows` (up when negative) in a feed of `total`
    /// rows of which `height` show. At the end it follows again.
    pub fn scroll(&mut self, rows: i64, total: usize, height: usize) {
        let last = total.saturating_sub(height) as i64;
        let from = self.window(total, height) as i64;
        let to = (from + rows).clamp(0, last);
        if to >= last {
            self.top = None;
            self.unseen = 0;
        } else {
            self.top = Some(to as usize);
        }
    }

    /// Moves the keyboard's entry to the one before (`back`) or after, among
    /// the entries the rows belong to, and brings it into view. From
    /// nothing it takes the newest. Past the newest it lets go and follows.
    pub fn step_cursor(&mut self, lines: &[u64], height: usize, back: bool) {
        let mut ids: Vec<u64> = Vec::new();
        for id in lines {
            if ids.last() != Some(id) {
                ids.push(*id);
            }
        }
        let now = self
            .cursor
            .and_then(|cursor| ids.iter().position(|id| *id == cursor));
        let next = match (now, back) {
            (None, _) => ids.len().checked_sub(1),
            (Some(0), true) => Some(0),
            (Some(now), true) => Some(now - 1),
            (Some(now), false) if now + 1 < ids.len() => Some(now + 1),
            (Some(_), false) => None,
        };
        let Some(next) = next else {
            self.follow();
            return;
        };
        let id = ids[next];
        self.cursor = Some(id);
        let (Some(first), Some(last)) = (
            lines.iter().position(|line| *line == id),
            lines.iter().rposition(|line| *line == id),
        ) else {
            return;
        };
        let total = lines.len();
        let top = self.window(total, height);
        let wanted = if first < top {
            first
        } else if last >= top + height {
            (last + 1).saturating_sub(height).min(first)
        } else {
            return;
        };
        if wanted >= total.saturating_sub(height) {
            self.top = None;
            self.unseen = 0;
        } else {
            self.top = Some(wanted);
        }
    }

    /// Lets go of the keyboard's entry. `false` when there was none.
    pub fn drop_cursor(&mut self) -> bool {
        self.cursor.take().is_some()
    }
}

/// Something of a session's past, as its transcript tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Past {
    /// A tool was used.
    Tool,
    /// The agent said this to the user, at that time if it is known.
    Speech(String, Option<i64>),
    /// A turn ended.
    TurnEnded,
}

/// The line that sums up the tools of a turn: the narrator's, and the plain
/// one.
pub fn tools_line(name: &str, tools: usize) -> (String, String) {
    let name = shout(name);
    match tools {
        1 => (format!("{name} used a tool."), "Used 1 tool.".to_owned()),
        n => (
            format!("{name} used {n} tools."),
            format!("Used {n} tools."),
        ),
    }
}

/// The past of a session as entries: every message as speech, and the tools
/// used between two messages (or to the end of a turn) as one line. Only the
/// last `keep` are given. A line of tools has the time of the message after
/// it, or else of the one before.
pub fn backfill(
    cub: u64,
    owner: u64,
    name: &str,
    tint: Option<Hsla>,
    past: &[Past],
    keep: usize,
) -> Vec<Item> {
    let item = |kind: Kind, text: String, plain: String, at: Option<i64>| Item {
        cub,
        owner,
        name: name.to_owned(),
        tint,
        at,
        kind,
        text,
        plain,
    };
    let mut out: Vec<Item> = Vec::new();
    let mut tools = 0;
    let mut last_time: Option<i64> = None;
    let flush = |out: &mut Vec<Item>, tools: &mut usize, at: Option<i64>| {
        if *tools > 0 {
            let (voiced, plain) = tools_line(name, *tools);
            out.push(item(Kind::Narration { urgent: false }, voiced, plain, at));
            *tools = 0;
        }
    };
    for event in past {
        match event {
            Past::Tool => tools += 1,
            Past::Speech(text, at) => {
                let at = at.or(last_time);
                flush(&mut out, &mut tools, at);
                if !text.trim().is_empty() {
                    out.push(item(Kind::Speech, text.clone(), String::new(), at));
                }
                last_time = at;
            }
            Past::TurnEnded => flush(&mut out, &mut tools, last_time),
        }
    }
    flush(&mut out, &mut tools, last_time);
    let skip = out.len().saturating_sub(keep);
    out.drain(..skip);
    out
}

/// How long ago `at` was at `now`, both in seconds since the Unix epoch, in
/// a few characters: `now`, `5m`, `3h`, `2d`.
pub fn ago(at: i64, now: i64) -> String {
    let seconds = now.saturating_sub(at).max(0);
    match seconds {
        0..=59 => "now".to_owned(),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Breaks a text into rows of at most `width` characters: at its own line
/// breaks, at spaces where it can, inside a word where it must. Leading
/// spaces of a line are kept (code is indented), a tab is two spaces, and
/// several empty lines are one.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.replace('\t', "  ");
        let line = line.trim_end();
        if line.is_empty() {
            if rows.last().is_some_and(|last| !last.is_empty()) {
                rows.push(String::new());
            }
            continue;
        }
        let indent: String = line
            .chars()
            .take_while(|c| *c == ' ')
            .take(width / 2)
            .collect();
        let mut row = indent.clone();
        let mut used = indent.chars().count();
        let mut fresh = true;
        for word in line.split(' ').filter(|word| !word.is_empty()) {
            let mut word: Vec<char> = word.chars().collect();
            loop {
                let gap = usize::from(!fresh);
                let room = width.saturating_sub(used + gap);
                if word.len() <= room {
                    if !fresh {
                        row.push(' ');
                    }
                    row.extend(word.iter());
                    used += gap + word.len();
                    fresh = false;
                    break;
                }
                if fresh && room > 0 {
                    // Longer than a whole row: cut it.
                    row.extend(word.drain(..room));
                }
                rows.push(std::mem::take(&mut row));
                used = 0;
                fresh = true;
            }
        }
        rows.push(row);
    }
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows
}

/// What a row of the feed is.
#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    /// The heading of a lion's entries: its swatch, its name, the time.
    Heading {
        /// The colour of the mane.
        tint: Option<Hsla>,
        /// The name.
        name: String,
        /// How long ago, or empty.
        time: String,
        /// The entries under it are speech: "said".
        speech: bool,
    },
    /// A row of the narrator's line.
    Narration {
        /// It must be read.
        urgent: bool,
    },
    /// A row of the agent's message.
    Speech,
    /// The row that stands for the rest of a message that is cut: how many
    /// rows more there are.
    More(usize),
    /// An empty row between two lions.
    Gap,
}

/// A row of the feed, laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The entry it belongs to.
    pub entry: u64,
    /// What it is.
    pub kind: RowKind,
    /// Its text.
    pub text: String,
}

/// How the feed is to be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Show {
    /// Only the entries of this lion and of its little ones.
    pub only: Option<u64>,
    /// The narrator is off: its lines are the plain ones.
    pub plain: bool,
    /// The width of a row of text, in characters.
    pub width: usize,
    /// What time it is, in seconds since the Unix epoch, if the host said.
    pub now: Option<i64>,
    /// The entry being typed and how many of its characters show.
    pub typing: Option<(u64, usize)>,
}

/// The rows of the feed, the oldest first: every entry `show` lets through,
/// under a heading where the lion or the kind changes or time has passed.
pub fn lay(feed: &Feed, show: &Show) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut before: Option<&Entry> = None;
    for entry in feed.entries() {
        let item = &entry.item;
        if show
            .only
            .is_some_and(|only| item.owner != only && item.cub != only)
        {
            continue;
        }
        let speech = item.kind == Kind::Speech;
        let text = if show.plain && !speech && !item.plain.is_empty() {
            item.plain.clone()
        } else {
            item.text.clone()
        };
        let text = match show.typing {
            Some((id, typed)) if id == entry.id && !show.plain => {
                text.chars().take(typed).collect()
            }
            _ => text,
        };
        let text = if entry.count > 1 {
            format!("{text} (x{})", entry.count)
        } else {
            text
        };
        let joined = before.is_some_and(|before| {
            before.item.cub == item.cub
                && (before.item.kind == Kind::Speech) == speech
                && match (before.item.at, item.at) {
                    (Some(a), Some(b)) => b.saturating_sub(a).saturating_abs() <= SAME_BREATH,
                    (None, None) => true,
                    _ => false,
                }
        });
        if !joined {
            if before.is_some() {
                rows.push(Row {
                    entry: entry.id,
                    kind: RowKind::Gap,
                    text: String::new(),
                });
            }
            rows.push(Row {
                entry: entry.id,
                kind: RowKind::Heading {
                    tint: item.tint,
                    name: shout(&item.name),
                    time: match (item.at, show.now) {
                        (Some(at), Some(now)) => ago(at, now),
                        _ => String::new(),
                    },
                    speech,
                },
                text: String::new(),
            });
        }
        let mut wrapped = wrap_text(&text, show.width);
        let mut hidden = 0;
        if speech && wrapped.len() > CLAMP_ROWS + 1 && !feed.is_open(entry.id) {
            hidden = wrapped.len() - CLAMP_ROWS;
            wrapped.truncate(CLAMP_ROWS);
        }
        let kind = match item.kind {
            Kind::Speech => RowKind::Speech,
            Kind::Narration { urgent } => RowKind::Narration { urgent },
        };
        for text in wrapped {
            rows.push(Row {
                entry: entry.id,
                kind: kind.clone(),
                text,
            });
        }
        if hidden > 0 {
            rows.push(Row {
                entry: entry.id,
                kind: RowKind::More(hidden),
                text: format!(
                    "+{hidden} more {}",
                    if hidden == 1 { "row" } else { "rows" }
                ),
            });
        }
        before = Some(entry);
    }
    rows
}

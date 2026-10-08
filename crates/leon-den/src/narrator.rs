//! The narrator: what the text box of the Den says, and when.
//!
//! [`narrate`] turns a [`Happening`] into a [`Line`] in the voice of
//! `brand/voice.md`: the name in capitals, one fact, at most two rows of
//! [`ROW_CHARS`] characters. It never invents: a line exists only for what
//! really happened, the "super effective" one only for a command that passed,
//! and a session nothing is known of gets a line that says so. The choice
//! among the templates of a fact is a function of the cub's name and a seed,
//! so the same session and the same seed always read the same, and a caller
//! that counts the seed up gets the templates in turn.
//!
//! [`plainly`] is the same fact with no joke, for a den whose narrator is
//! off. Both go in the feed ([`crate::feed`]), which keeps them.
//!
//! A [`Narrator`] is the queue behind the one line the den says of itself as
//! a whole, at the foot of the feed (`The den is quiet. Too quiet.`). Lines are typed out a few
//! characters a tick and then held. It never falls far behind what happens:
//! while lines wait, each is held only a moment, and of the chatter that
//! waits only the newest is kept. A line about a permission prompt or a cub
//! that fainted is never dropped. Everything is scheduled when a line is
//! pushed, so what the box shows at a time is a pure function of that time.

use std::time::Duration;

use crate::model::{Event, Happening, ToolKind};
use crate::palette::scramble;

/// The width of the text box, in characters.
pub const ROW_CHARS: usize = 34;
/// The height of the text box, in rows.
pub const ROWS: usize = 2;
/// A name longer than this is cut.
pub const NAME_CHARS: usize = 12;
/// The length of a tick: the Den's clock for everything that moves.
pub const TICK: Duration = Duration::from_millis(100);
/// How many characters are typed per tick.
pub const TYPED_PER_TICK: usize = 3;
/// How long a line is held, once typed, while another one waits.
pub const SHORT_HOLD: Duration = Duration::from_millis(700);
/// The same for a line that must be read: a permission prompt, a faint.
pub const URGENT_HOLD: Duration = Duration::from_millis(1600);
/// How long the last line stays before the box may say something about the
/// den as a whole.
pub const LINGER: Duration = Duration::from_secs(8);

/// What the box says at once: one or two rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The rows, each at most [`ROW_CHARS`] characters.
    pub rows: Vec<String>,
    /// A permission prompt or a faint: it carries the status colour and is
    /// never dropped.
    pub urgent: bool,
}

impl Line {
    /// How many characters there are to type.
    pub fn len(&self) -> usize {
        self.rows.iter().map(|row| row.chars().count()).sum()
    }

    /// Whether there is nothing to type.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The first `typed` characters, row by row.
    pub fn typed(&self, typed: usize) -> Vec<String> {
        let mut left = typed;
        self.rows
            .iter()
            .map(|row| {
                let take = row.chars().count().min(left);
                left -= take;
                row.chars().take(take).collect()
            })
            .collect()
    }

    /// The rows as one text.
    pub fn text(&self) -> String {
        self.rows.join("\n")
    }
}

/// A name as the narrator writes it: in capitals, cut to [`NAME_CHARS`].
pub fn shout(name: &str) -> String {
    let name = name.trim();
    let upper: String = name.to_uppercase();
    let upper = if upper.is_empty() {
        "A CUB".to_owned()
    } else {
        upper
    };
    if upper.chars().count() <= NAME_CHARS {
        upper
    } else {
        let mut cut: String = upper.chars().take(NAME_CHARS - 1).collect();
        cut.push('…');
        cut
    }
}

/// A command line as a move: `cargo test -p leon-den` is `CARGO TEST`. The
/// program and, if it has one, its subcommand; no flags, no paths.
pub fn move_name(command: &str) -> String {
    let mut words = command
        .split_whitespace()
        .skip_while(|word| word.contains('=') && !word.starts_with('-'));
    let Some(program) = words.next() else {
        return "A COMMAND".to_owned();
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let mut name = program.to_uppercase();
    if let Some(sub) = words.next() {
        let plain = sub
            .chars()
            .all(|letter| letter.is_alphanumeric() || letter == '-' || letter == '_')
            && !sub.starts_with('-');
        if plain && name.chars().count() + sub.chars().count() < 18 {
            name.push(' ');
            name.push_str(&sub.to_uppercase());
        }
    }
    if name.chars().count() > 18 {
        let mut cut: String = name.chars().take(17).collect();
        cut.push('…');
        cut
    } else {
        name
    }
}

/// Breaks a text into rows of at most [`ROW_CHARS`], at spaces, keeping the
/// line breaks it already has. `None` if it does not fit in [`ROWS`] rows.
pub fn wrap(text: &str) -> Option<Vec<String>> {
    let mut rows: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut row = String::new();
        for word in paragraph.split(' ').filter(|word| !word.is_empty()) {
            let width = word.chars().count();
            if width > ROW_CHARS {
                return None;
            }
            let used = row.chars().count();
            if used > 0 && used + 1 + width > ROW_CHARS {
                rows.push(std::mem::take(&mut row));
            }
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(word);
        }
        rows.push(row);
    }
    (rows.len() <= ROWS).then_some(rows)
}

/// The shorter and shorter forms of a detail, the whole of it first: a path
/// is cut to its file name, then the text is cut from the end.
fn shortenings(detail: &str) -> Vec<String> {
    let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut forms = vec![detail.clone()];
    if detail.contains('/') && !detail.contains(' ') && !detail.contains("://") {
        if let Some(file) = detail
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|file| !file.is_empty())
        {
            forms.push(file.to_owned());
        }
    }
    let base: Vec<char> = forms.last().cloned().unwrap_or_default().chars().collect();
    for keep in (1..base.len()).rev() {
        let mut cut: String = base[..keep].iter().collect();
        let cut_len = cut.trim_end().len();
        cut.truncate(cut_len);
        cut.push('…');
        forms.push(cut);
    }
    forms
}

/// Writes a line from a template that takes a detail: the longest form of
/// the detail with which it fits.
fn fit(detail: &str, write: impl Fn(&str) -> String) -> Option<Vec<String>> {
    shortenings(detail)
        .iter()
        .find_map(|form| wrap(&write(form)))
}

/// Which of `count` templates to use: a number that depends on the name and
/// on the kind of fact, moved on by the seed.
fn pick(count: u64, name: &str, kind: u64, seed: u64) -> u64 {
    let hash = name.bytes().fold(kind, |hash, byte| {
        scramble(hash ^ u64::from(byte)).rotate_left(5)
    });
    (scramble(hash) % count + seed % count) % count
}

fn plain(rows: Option<Vec<String>>) -> Option<Line> {
    rows.map(|rows| Line {
        rows,
        urgent: false,
    })
}

fn urgent(rows: Option<Vec<String>>) -> Option<Line> {
    rows.map(|rows| Line { rows, urgent: true })
}

/// What the narrator says about a happening, or nothing: a tool that ends
/// without news gets no line.
pub fn narrate(happening: &Happening, seed: u64) -> Option<Line> {
    let name = shout(&happening.name);
    let n = name.as_str();
    let raw = happening.name.as_str();
    match &happening.event {
        Event::Joined => plain(wrap(&match pick(2, raw, 1, seed) {
            0 => format!("{n} joined the pride."),
            _ => format!("{n} padded into the den."),
        })),
        Event::ToolStarted { kind, tool, detail } => {
            let detail = detail.as_deref().map(str::trim).filter(|d| !d.is_empty());
            let variant = pick(2, raw, 2 + *kind as u64, seed);
            plain(match (kind, detail) {
                (ToolKind::Edit, Some(d)) => fit(d, |d| match variant {
                    0 => format!("{n} used EDIT on {d}!"),
                    _ => format!("{n} is typing furiously at {d}."),
                }),
                (ToolKind::Edit, None) => wrap(&format!("{n} is typing furiously.")),
                (ToolKind::Read, Some(d)) => fit(d, |d| match variant {
                    0 => format!("{n} is sniffing around {d}."),
                    _ => format!("{n} pulls {d} off the shelf."),
                }),
                (ToolKind::Read, None) => wrap(&format!("{n} is sniffing around the shelf.")),
                (ToolKind::Search, Some(d)) => fit(d, |d| match variant {
                    0 => format!("{n} empties the shelf looking for \"{d}\"..."),
                    _ => format!("{n} hunts for \"{d}\"..."),
                }),
                (ToolKind::Search, None) => wrap(&format!("{n} empties the shelf...")),
                (ToolKind::Run, Some(d)) => {
                    let command = move_name(d);
                    wrap(&match variant {
                        0 => format!("{n} used {command}!\nWaiting for it to land..."),
                        _ => format!("{n} used {command}!"),
                    })
                }
                (ToolKind::Run, None) => wrap(&format!("{n} went over to the rack.")),
                (ToolKind::Web, Some(d)) => fit(d, |d| format!("{n} points the telescope at {d}.")),
                (ToolKind::Web, None) => wrap(&format!("{n} is at the telescope.")),
                (ToolKind::Plan, _) => wrap(&format!("{n} is staring at the board.")),
                (ToolKind::Other, _) => {
                    let tool = tool.trim();
                    if tool.is_empty() {
                        wrap(&format!("{n} is doing something mysterious."))
                    } else {
                        fit(&tool.to_uppercase(), |tool| format!("{n} used {tool}!"))
                    }
                }
            })
        }
        Event::ToolFinished { ok: false, .. } => plain(wrap("It's not very effective...")),
        Event::ToolFinished {
            kind: ToolKind::Run,
            ok: true,
        } => plain(wrap("It's super effective!")),
        Event::ToolFinished { .. } => None,
        Event::SentOut { little } => {
            let little = shout(little);
            plain(wrap(&match pick(2, raw, 20, seed) {
                0 => format!("{n} sent out {little}!"),
                _ => format!("{n} sent out {little}!\nAn egg is hatching..."),
            }))
        }
        Event::CameBack { little } => {
            plain(wrap(&format!("{} came back with a report.", shout(little))))
        }
        Event::PermissionPrompt => urgent(wrap(&format!(
            "A wild PERMISSION PROMPT appeared.\n{n} is staring at you."
        ))),
        Event::TurnEnded => plain(wrap(&format!("{n} is done.\n{n} wants a new order."))),
        Event::Mysterious => plain(wrap(&format!("{n} is doing something mysterious."))),
        Event::FellAsleep => plain(wrap(&format!("{n} fell asleep."))),
        Event::Fainted { exit } => urgent(wrap(&match exit {
            Some(code) => format!("{n} fainted! (exit {code})"),
            None => format!("{n} fainted!"),
        })),
        Event::WentHome => plain(wrap(&format!("{n} went home."))),
    }
}

/// The same happening in plain words, for a den whose narrator is off: the
/// fact with no joke and no name (the feed writes the name over it). `None`
/// where [`narrate`] has nothing to say either.
pub fn plainly(happening: &Happening) -> Option<String> {
    let detail = |detail: &Option<String>| {
        detail
            .as_deref()
            .map(str::trim)
            .filter(|detail| !detail.is_empty())
            .map(|detail| detail.split_whitespace().collect::<Vec<_>>().join(" "))
    };
    Some(match &happening.event {
        Event::Joined => "Session started.".to_owned(),
        Event::ToolStarted {
            kind,
            tool,
            detail: subject,
        } => match (kind, detail(subject)) {
            (ToolKind::Edit, Some(d)) => format!("Editing {d}"),
            (ToolKind::Edit, None) => "Editing a file.".to_owned(),
            (ToolKind::Read, Some(d)) => format!("Reading {d}"),
            (ToolKind::Read, None) => "Reading a file.".to_owned(),
            (ToolKind::Search, Some(d)) => format!("Searching for \"{d}\""),
            (ToolKind::Search, None) => "Searching.".to_owned(),
            (ToolKind::Run, Some(d)) => format!("Running `{d}`"),
            (ToolKind::Run, None) => "Running a command.".to_owned(),
            (ToolKind::Web, Some(d)) => format!("Fetching {d}"),
            (ToolKind::Web, None) => "Fetching from the web.".to_owned(),
            (ToolKind::Plan, _) => "Planning.".to_owned(),
            (ToolKind::Other, _) if tool.trim().is_empty() => "Using a tool.".to_owned(),
            (ToolKind::Other, Some(d)) => format!("Using {}: {d}", tool.trim()),
            (ToolKind::Other, None) => format!("Using {}.", tool.trim()),
        },
        Event::ToolFinished { ok: false, .. } => "It failed.".to_owned(),
        Event::ToolFinished {
            kind: ToolKind::Run,
            ok: true,
        } => "The command passed.".to_owned(),
        Event::ToolFinished { .. } => return None,
        Event::SentOut { little } => format!("Started the sub-agent {little}."),
        Event::CameBack { little } => format!("The sub-agent {little} finished."),
        Event::PermissionPrompt => "Needs your permission (inferred).".to_owned(),
        Event::TurnEnded => "Finished its turn: waiting for you.".to_owned(),
        Event::Mysterious => "Working (no details available).".to_owned(),
        Event::FellAsleep => "Quiet for a long while.".to_owned(),
        Event::Fainted { exit: Some(code) } => format!("Exited with an error (exit {code})."),
        Event::Fainted { exit: None } => "Exited with an error.".to_owned(),
        Event::WentHome => "Exited.".to_owned(),
    })
}

/// What the box says when nobody is in the den.
pub fn quiet_line() -> Line {
    Line {
        rows: vec!["The den is quiet. Too quiet.".to_owned()],
        urgent: false,
    }
}

/// What the box says when many cubs work at once.
pub fn busy_line() -> Line {
    Line {
        rows: vec![
            "The pride is busy.".to_owned(),
            "You may go get coffee.".to_owned(),
        ],
        urgent: false,
    }
}

/// What the box says of a cub that has been quiet for a long while.
pub fn pondering_line(name: &str) -> Line {
    Line {
        rows: vec![
            format!("{} is thinking very hard.", shout(name)),
            "Or napping.".to_owned(),
        ],
        urgent: false,
    }
}

/// How long a line takes to type.
pub fn typing_time(line: &Line) -> Duration {
    TICK * line.len().div_ceil(TYPED_PER_TICK) as u32
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    line: Line,
    /// When it starts to be typed.
    start: Duration,
}

/// What the box shows at one moment.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Said {
    /// The rows as typed so far. Empty when nothing has been said yet.
    pub rows: Vec<String>,
    /// The line is about a permission prompt or a faint.
    pub urgent: bool,
    /// The whole line is on screen.
    pub complete: bool,
    /// Other lines wait behind this one.
    pub more: bool,
    /// When what is shown changes next, if it does.
    pub next: Option<Duration>,
    /// When the line was fully typed. `None` while it is being typed or
    /// when there is no line.
    pub since: Option<Duration>,
}

/// The queue behind the text box.
#[derive(Clone, Debug, Default)]
pub struct Narrator {
    entries: Vec<Entry>,
    reduced: bool,
}

impl Narrator {
    /// An empty box.
    pub fn new() -> Self {
        Self::default()
    }

    /// With reduced motion a line appears whole, at once.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced = reduced;
    }

    fn typing(&self, line: &Line) -> Duration {
        if self.reduced {
            Duration::ZERO
        } else {
            typing_time(line)
        }
    }

    fn hold(line: &Line) -> Duration {
        if line.urgent {
            URGENT_HOLD
        } else {
            SHORT_HOLD
        }
    }

    /// How many lines wait to be shown at a time.
    pub fn waiting(&self, now: Duration) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.start > now)
            .count()
    }

    /// Says a line. It is shown as soon as the lines before it have had
    /// their moment; chatter that was still waiting is dropped for it.
    pub fn say(&mut self, line: Line, now: Duration) {
        if line.is_empty() {
            return;
        }
        // Forget what is over: everything before the line now on screen.
        let current = self.entries.iter().rposition(|entry| entry.start <= now);
        if let Some(current) = current {
            self.entries.drain(..current);
        }
        let shown = usize::from(current.is_some());
        if self.entries.last().is_some_and(|last| last.line == line) {
            return;
        }
        // Of the chatter that waits, only the newest is worth saying.
        let mut waiting: Vec<Line> = self
            .entries
            .drain(shown..)
            .map(|entry| entry.line)
            .filter(|waiting| waiting.urgent || line.urgent)
            .collect();
        waiting.push(line);
        // What must be read goes first.
        waiting.sort_by_key(|line| !line.urgent);

        let mut at = match self.entries.first() {
            Some(entry) => {
                (entry.start + self.typing(&entry.line) + Self::hold(&entry.line)).max(now)
            }
            None => now,
        };
        for line in waiting {
            let next = at + self.typing(&line) + Self::hold(&line);
            self.entries.push(Entry { line, start: at });
            at = next;
        }
    }

    /// What the box shows at a time.
    pub fn said(&self, now: Duration) -> Said {
        let Some(index) = self.entries.iter().rposition(|entry| entry.start <= now) else {
            return Said {
                next: self.entries.first().map(|entry| entry.start),
                ..Said::default()
            };
        };
        let entry = &self.entries[index];
        let following = self.entries.get(index + 1).map(|next| next.start);
        let typed_at = entry.start + self.typing(&entry.line);
        let total = entry.line.len();
        let typed = if now >= typed_at {
            total
        } else {
            let ticks = ((now - entry.start).as_millis() / TICK.as_millis()) as usize + 1;
            (ticks * TYPED_PER_TICK).min(total)
        };
        let complete = typed >= total;
        let next = if complete {
            following
        } else {
            let ticks = (now - entry.start).as_millis() / TICK.as_millis() + 1;
            Some(entry.start + TICK * ticks as u32)
        };
        Said {
            rows: entry.line.typed(typed),
            urgent: entry.line.urgent,
            complete,
            more: following.is_some(),
            next,
            since: complete.then_some(typed_at),
        }
    }
}

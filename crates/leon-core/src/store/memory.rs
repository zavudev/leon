//! The memory agents share: what was decided, agreed and found out, kept
//! across sessions and across agents.
//!
//! An entry is one fact: a kind, a short title and a text, with the agent that
//! wrote it when that is known. It belongs to a [`MemoryScope`]: one project,
//! or everything. A project is named by the identity of its root folder
//! ([`path::key`](crate::path::key)) and not by its id in the store, because
//! that id changes when a project is removed and added again, and a folder
//! Leon was never shown has none; every git worktree of a project resolves to
//! the same root (see [`Store::memory_root`]) and so shares its memory.
//!
//! A memory that is only ever added to degrades: the same fact is saved again
//! and again, the fact that changed is saved beside its old self, and what is
//! wrong stays. Five rules keep it from that:
//!
//! * **A topic** is an optional key of an entry (`auth/token-format`). Saving
//!   under a topic that a live entry of the scope already has revises that
//!   entry in place: the same id, the new words, a revision count one higher.
//! * **The same text is one entry.** Every entry keeps a hash of its text
//!   with case, white space and surrounding punctuation taken out
//!   ([`text_hash`]). Saving a text a live entry of the scope already says
//!   adds nothing: that entry counts the repeat and the time it was last seen.
//! * **Pinned** entries are the ones an agent must not miss: first in a
//!   listing and in the memory file. There is no bound on them here: the
//!   memory file's size is the one limit, and it is the file's to keep.
//! * **Forgetting is soft.** A forgotten entry keeps its row and the time it
//!   was forgotten, is in no listing, no search and no count, and can be
//!   restored. [`Store::purge_memories`] removes for good the ones forgotten
//!   before a given time; [`Store::delete_memory`] removes one at once. The
//!   full-text index keeps a forgotten entry's words (its triggers know rows,
//!   not states): it is the search's query that leaves forgotten rows out.
//! * **An entry can be edited** ([`Store::update_memory`]), which counts as a
//!   revision and is refused when it would say what another live entry says.
//!
//! The store keeps what it is told and bounds it: the title, the text, the
//! topic and the agent tag have a longest length. A scope holds at most
//! [`MAX_ENTRIES`] live entries, a number no memory reaches by being used: it
//! is there to stop an agent that saves in a loop. What is refused is refused with a
//! [`MemoryError`]. The clock is the caller's: every write takes its time, in
//! milliseconds.
//!
//! Search reuses the history's: [`fts_query`](super::search::fts_query) makes
//! free text a query that cannot fail, and snippets carry the same two
//! markers.

use rusqlite::{params, OptionalExtension as _, Row, Transaction};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use super::history::sql_limit;
use super::projects::{owning_project, project_roots};
use super::search::{fts_query, SNIPPET_ELLIPSIS, SNIPPET_END, SNIPPET_START};
use super::Store;
use crate::change::StoreChange;
use crate::error::{Result, StoreError};
use crate::ids::MachineId;

/// The scope column of an entry that belongs to no project. No path has this
/// identity: a key is a path, and this is not one.
const GLOBAL_SCOPE: &str = "global:";

/// The longest title, in characters.
pub const MAX_TITLE_CHARS: usize = 120;
/// The longest text, in characters. An entry is one fact, not a document;
/// the memory file shows the start of it and `show` the whole.
pub const MAX_TEXT_CHARS: usize = 8_000;
/// The longest agent tag, in characters.
pub const MAX_AGENT_CHARS: usize = 64;
/// The longest topic, in characters.
pub const MAX_TOPIC_CHARS: usize = 64;
/// The most live entries one scope holds: a guard against a runaway agent,
/// not a size anybody is meant to reach.
pub const MAX_ENTRIES: usize = 100_000;
/// The most entries or hits one call returns, whatever it asks for.
pub const MAX_LISTED: usize = 500;
/// The shortest tail of an id that may stand for the id.
pub const MIN_ID_TAIL: usize = 6;

/// Roughly how many words a snippet contains.
const SNIPPET_WORDS: u32 = 24;

/// The columns of an entry, in the order [`memory_from_row`] reads them.
const COLUMNS: &str = "m.id, m.scope, m.root, m.kind, m.title, m.text, m.agent, \
                       m.created_at, m.updated_at, m.topic, m.revision, m.duplicates, \
                       m.last_seen_at, m.pinned, m.forgotten_at";
/// How many columns that is: where a query's own columns start.
const COLUMN_COUNT: usize = 15;

/// Why an entry was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MemoryError {
    /// There is no text.
    #[error("there is nothing to remember: the text is empty")]
    EmptyText,
    /// The text is longer than [`MAX_TEXT_CHARS`].
    #[error("the text has {found} characters and the most is {max}: one fact per entry")]
    TextTooLong {
        /// How long it is.
        found: usize,
        /// The most allowed.
        max: usize,
    },
    /// The title is longer than [`MAX_TITLE_CHARS`].
    #[error("the title has {found} characters and the most is {max}")]
    TitleTooLong {
        /// How long it is.
        found: usize,
        /// The most allowed.
        max: usize,
    },
    /// The agent tag is empty, too long or not a plain word.
    #[error("{0:?} is not an agent tag: use letters, digits, '-', '_' and '.', at most {MAX_AGENT_CHARS}")]
    BadAgent(String),
    /// The topic is empty, too long or not a plain key.
    #[error("{0:?} is not a topic: use a key such as auth/token-format (letters, digits, '-', '_', '.', '/' and ':', starting with a letter or a digit, at most {MAX_TOPIC_CHARS})")]
    BadTopic(String),
    /// The kind is not one of [`MemoryKind::ALL`].
    #[error("{0:?} is not a kind: use decision, convention, discovery, preference or note")]
    UnknownKind(String),
    /// The scope already holds [`MAX_ENTRIES`].
    #[error("this memory is full ({max} entries): forget some before adding more")]
    ScopeFull {
        /// The most a scope holds.
        max: usize,
    },
    /// Another live entry of the scope already says this text.
    #[error("the entry {id} already says this: change that one, or forget it")]
    Duplicate {
        /// The short id of the entry that says it.
        id: String,
    },
    /// Another live entry of the scope already has this topic.
    #[error("the entry {id} already has the topic {topic}: forget that one, or use another topic")]
    TopicTaken {
        /// The topic.
        topic: String,
        /// The short id of the entry that has it.
        id: String,
    },
    /// The entry is forgotten and the request is for a live one.
    #[error("the entry {id} is forgotten: restore it first")]
    Forgotten {
        /// Its short id.
        id: String,
    },
    /// The entry is live and the request is for a forgotten one.
    #[error("the entry {id} is not forgotten")]
    NotForgotten {
        /// Its short id.
        id: String,
    },
    /// A change that changes nothing.
    #[error("nothing to change: give a text, a kind, a title or a topic")]
    NothingToChange,
    /// The project's root is not an absolute folder.
    #[error("{0:?} is not the absolute path of a folder")]
    BadRoot(String),
    /// A tail of an id names more than one entry.
    #[error("{0:?} is the end of more than one id: give more of it")]
    AmbiguousId(String),
}

impl From<MemoryError> for StoreError {
    fn from(error: MemoryError) -> Self {
        StoreError::Invalid(error.to_string())
    }
}

/// What an entry is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryKind {
    /// Something that was decided, and why.
    Decision,
    /// How things are done here.
    Convention,
    /// Something found out the hard way.
    Discovery,
    /// How the person likes it.
    Preference,
    /// Anything else worth keeping.
    Note,
}

impl MemoryKind {
    /// Every kind, in the order a memory is read.
    pub const ALL: [MemoryKind; 5] = [
        MemoryKind::Decision,
        MemoryKind::Convention,
        MemoryKind::Discovery,
        MemoryKind::Preference,
        MemoryKind::Note,
    ];

    /// The kind as it is stored and typed.
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryKind::Decision => "decision",
            MemoryKind::Convention => "convention",
            MemoryKind::Discovery => "discovery",
            MemoryKind::Preference => "preference",
            MemoryKind::Note => "note",
        }
    }

    /// The kind a word names, in any case.
    pub fn parse(word: &str) -> Result<Self, MemoryError> {
        let lower = word.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == lower)
            .ok_or_else(|| MemoryError::UnknownKind(word.to_owned()))
    }
}

/// Whose memory an entry is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MemoryScope {
    /// Every project on this computer.
    Global,
    /// One project, with all its worktrees: the absolute path of its root.
    Project(String),
}

impl MemoryScope {
    /// The scope of the project rooted at `root`.
    pub fn project(root: impl Into<String>) -> Self {
        MemoryScope::Project(root.into())
    }

    /// The project's root as it was written, or `None` for the global scope.
    pub fn root(&self) -> Option<&str> {
        match self {
            MemoryScope::Global => None,
            MemoryScope::Project(root) => Some(root),
        }
    }

    /// The scope column: the identity of the root, or the global marker.
    fn key(&self) -> String {
        match self {
            MemoryScope::Global => GLOBAL_SCOPE.to_owned(),
            MemoryScope::Project(root) => crate::path::key(root),
        }
    }

    fn check(&self) -> Result<(), MemoryError> {
        match self {
            MemoryScope::Global => Ok(()),
            MemoryScope::Project(root) => {
                let key = crate::path::key(root);
                let absolute = match crate::path::PathStyle::of(root) {
                    crate::path::PathStyle::Posix => key.starts_with('/'),
                    crate::path::PathStyle::Windows => !key.is_empty(),
                };
                if absolute && !root.chars().any(char::is_control) {
                    Ok(())
                } else {
                    Err(MemoryError::BadRoot(root.clone()))
                }
            }
        }
    }
}

/// An entry to add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMemory {
    /// Whose memory it goes to.
    pub scope: MemoryScope,
    /// What it is about. Without one a new entry is a note, and a revised
    /// one keeps the kind it has.
    pub kind: Option<MemoryKind>,
    /// A short title; the first line of the text when absent.
    pub title: Option<String>,
    /// The fact.
    pub text: String,
    /// The agent that wrote it, when known (`claude`, `codex`, ...).
    pub agent: Option<String>,
    /// The topic it is the word on: a live entry of the scope with this topic
    /// is revised instead of a second one being added.
    pub topic: Option<String>,
}

impl NewMemory {
    /// A note for `scope` saying `text`.
    pub fn note(scope: MemoryScope, text: impl Into<String>) -> Self {
        Self {
            scope,
            kind: None,
            title: None,
            text: text.into(),
            agent: None,
            topic: None,
        }
    }
}

/// What to change of an entry; what is `None` stays.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryPatch {
    /// A new kind.
    pub kind: Option<MemoryKind>,
    /// A new title.
    pub title: Option<String>,
    /// A new text.
    pub text: Option<String>,
    /// A new topic (`Some(Some(..))`), or none any more (`Some(None)`).
    pub topic: Option<Option<String>>,
}

impl MemoryPatch {
    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// One stored entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory {
    /// Its id (a UUID v7).
    pub id: String,
    /// Whose memory it is.
    pub scope: MemoryScope,
    /// What it is about.
    pub kind: MemoryKind,
    /// The short title.
    pub title: String,
    /// The fact.
    pub text: String,
    /// The agent that wrote it last, when known.
    pub agent: Option<String>,
    /// When it was added, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When its words were last changed.
    pub updated_at: i64,
    /// The topic it is the word on, when it has one.
    pub topic: Option<String>,
    /// How many times it was written: 1, and one more for every revision by
    /// topic and every edit.
    pub revision: u32,
    /// How many times its text was saved again and not added.
    pub duplicates: u32,
    /// When it was last saved, revised or saved again.
    pub last_seen_at: i64,
    /// Whether it is pinned.
    pub pinned: bool,
    /// When it was forgotten; `None` while it is live.
    pub forgotten_at: Option<i64>,
}

impl Memory {
    /// The end of the id that is shown and accepted in its place: the random
    /// part of a UUID v7, where two ids made in the same moment differ.
    pub fn short_id(&self) -> &str {
        short_id(&self.id)
    }

    /// Whether it is forgotten.
    pub fn is_forgotten(&self) -> bool {
        self.forgotten_at.is_some()
    }
}

/// The last eight characters of an id; see [`Memory::short_id`].
pub fn short_id(id: &str) -> &str {
    let start = id.char_indices().rev().nth(7).map_or(0, |(index, _)| index);
    &id[start..]
}

/// What saving an entry came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    /// A new entry.
    New(Memory),
    /// The live entry of that topic, revised in place.
    Revised(Memory),
    /// Nothing new: a live entry already says this text. It is the entry,
    /// with the repeat counted.
    Known(Memory),
}

impl Saved {
    /// The entry, whichever way it was saved.
    pub fn memory(&self) -> &Memory {
        match self {
            Saved::New(memory) | Saved::Revised(memory) | Saved::Known(memory) => memory,
        }
    }

    /// The entry, owned.
    pub fn into_memory(self) -> Memory {
        match self {
            Saved::New(memory) | Saved::Revised(memory) | Saved::Known(memory) => memory,
        }
    }
}

/// What somebody decided about telling a project's agents of the memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryChoice {
    /// It was turned on: the managed block is in the project's files.
    On,
    /// It was turned off again by hand.
    Off,
    /// "Never for this project": do not ask about it again.
    Never,
}

impl MemoryChoice {
    /// The choice as it is stored.
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryChoice::On => "on",
            MemoryChoice::Off => "off",
            MemoryChoice::Never => "never",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        [MemoryChoice::On, MemoryChoice::Off, MemoryChoice::Never]
            .into_iter()
            .find(|choice| choice.as_str() == word)
    }
}

/// How many entries a scope holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryCounts {
    /// Live entries, the pinned ones among them.
    pub live: usize,
    /// Pinned entries.
    pub pinned: usize,
    /// Forgotten entries, not yet purged.
    pub forgotten: usize,
}

/// One match of a search.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHit {
    /// The entry.
    pub memory: Memory,
    /// An excerpt of the text around the match, with the matched words
    /// between [`SNIPPET_START`] and [`SNIPPET_END`]. The title's own matches
    /// are not marked.
    pub snippet: String,
    /// Relevance: lower is better, among the hits of one search.
    pub rank: f64,
}

/// One line of text: control characters and line breaks become spaces, runs
/// of white space one space.
fn one_line(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The text as it is kept: line endings as `\n`, no other control character
/// but the tab, no blank space around it.
fn clean_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A text as two sayings of one fact share it: lower case, every run of
/// white space one space, and no punctuation or space at either end. A text
/// of punctuation alone keeps it, so that two such texts stay two.
pub fn normalized(text: &str) -> String {
    let folded = text
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = folded.trim_matches(|c: char| !c.is_alphanumeric());
    if trimmed.is_empty() {
        folded
    } else {
        trimmed.to_owned()
    }
}

/// The hash two sayings of one fact share: of [`normalized`], as 32
/// hexadecimal characters.
pub fn text_hash(text: &str) -> String {
    Sha256::digest(normalized(text).as_bytes())[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The title of a text that was given none: its first line, cut at a word
/// when it is long.
pub fn title_of(text: &str) -> String {
    const LONGEST: usize = 72;
    let line = one_line(text.trim().lines().next().unwrap_or_default());
    if line.chars().count() <= LONGEST {
        return line;
    }
    let cut: String = line.chars().take(LONGEST).collect();
    let cut = match cut.rfind(' ') {
        Some(space) if space > LONGEST / 2 => &cut[..space],
        _ => cut.as_str(),
    };
    format!("{}{SNIPPET_ELLIPSIS}", cut.trim_end())
}

fn checked_text(text: &str) -> Result<String, MemoryError> {
    let text = clean_text(text);
    let found = text.chars().count();
    if found == 0 {
        return Err(MemoryError::EmptyText);
    }
    if found > MAX_TEXT_CHARS {
        return Err(MemoryError::TextTooLong {
            found,
            max: MAX_TEXT_CHARS,
        });
    }
    Ok(text)
}

fn checked_title(title: Option<&str>, text: &str) -> Result<String, MemoryError> {
    let title = title.map(one_line).filter(|title| !title.is_empty());
    let title = title.unwrap_or_else(|| title_of(text));
    let found = title.chars().count();
    if found > MAX_TITLE_CHARS {
        return Err(MemoryError::TitleTooLong {
            found,
            max: MAX_TITLE_CHARS,
        });
    }
    Ok(title)
}

/// The agent tag as it is kept: a plain lower-case word.
pub fn agent_tag(agent: &str) -> Result<String, MemoryError> {
    let tag = agent.trim().to_lowercase();
    let plain = tag
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if tag.is_empty() || !plain || tag.chars().count() > MAX_AGENT_CHARS {
        return Err(MemoryError::BadAgent(agent.to_owned()));
    }
    Ok(tag)
}

/// The topic as it is kept: a lower-case key of ASCII letters, digits and
/// `-`, `_`, `.`, `/`, `:`, starting with a letter or a digit.
pub fn topic_key(topic: &str) -> Result<String, MemoryError> {
    let key = topic.trim().to_ascii_lowercase();
    let plain = key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':'));
    let starts = key.starts_with(|c: char| c.is_ascii_alphanumeric());
    if !plain || !starts || key.len() > MAX_TOPIC_CHARS {
        return Err(MemoryError::BadTopic(topic.to_owned()));
    }
    Ok(key)
}

/// The time before which a forgotten entry is purged, `days` before `now`.
pub fn purge_before(now: i64, days: u32) -> i64 {
    now.saturating_sub(i64::from(days).saturating_mul(86_400_000))
}

fn count_of(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn memory_from_row(row: &Row<'_>) -> rusqlite::Result<Memory> {
    let scope: String = row.get(1)?;
    let root: String = row.get(2)?;
    let kind: String = row.get(3)?;
    Ok(Memory {
        id: row.get(0)?,
        scope: if scope == GLOBAL_SCOPE {
            MemoryScope::Global
        } else {
            MemoryScope::Project(root)
        },
        kind: MemoryKind::parse(&kind).map_err(|_| super::bad_tag(3, &kind))?,
        title: row.get(4)?,
        text: row.get(5)?,
        agent: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        topic: row.get(9)?,
        revision: count_of(row.get(10)?),
        duplicates: count_of(row.get(11)?),
        last_seen_at: row.get(12)?,
        pinned: row.get(13)?,
        forgotten_at: row.get(14)?,
    })
}

/// The entry with this exact id.
fn row_of(tx: &rusqlite::Connection, id: &str) -> Result<Memory> {
    tx.query_row(
        &format!("SELECT {COLUMNS} FROM memory m WHERE m.id = ?1"),
        [id],
        memory_from_row,
    )
    .optional()?
    .ok_or(StoreError::NotFound("memory"))
}

/// The live entry of `scope` that says `hash`, other than `except`.
fn live_with_hash(
    tx: &Transaction<'_>,
    scope: &str,
    hash: &str,
    except: Option<&str>,
) -> Result<Option<Memory>> {
    Ok(tx
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM memory m
                 WHERE m.scope = ?1 AND m.text_hash = ?2 AND m.forgotten_at IS NULL
                   AND (?3 IS NULL OR m.id <> ?3)
                 ORDER BY m.pk LIMIT 1"
            ),
            params![scope, hash, except],
            memory_from_row,
        )
        .optional()?)
}

/// The live entry of `scope` with `topic`, other than `except`.
fn live_with_topic(
    tx: &Transaction<'_>,
    scope: &str,
    topic: &str,
    except: Option<&str>,
) -> Result<Option<Memory>> {
    Ok(tx
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM memory m
                 WHERE m.scope = ?1 AND m.topic = ?2 AND m.forgotten_at IS NULL
                   AND (?3 IS NULL OR m.id <> ?3)"
            ),
            params![scope, topic, except],
            memory_from_row,
        )
        .optional()?)
}

fn live_count(tx: &Transaction<'_>, scope: &str) -> Result<usize> {
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM memory WHERE scope = ?1 AND forgotten_at IS NULL",
        [scope],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(count).unwrap_or_default())
}

/// Writes the words of an entry back: everything a revision or an edit may
/// have changed.
fn write_words(tx: &Transaction<'_>, memory: &Memory) -> Result<()> {
    tx.execute(
        "UPDATE memory SET kind = ?2, title = ?3, text = ?4, agent = ?5, topic = ?6,
                text_hash = ?7, revision = ?8, updated_at = ?9, last_seen_at = ?10
         WHERE id = ?1",
        params![
            memory.id,
            memory.kind.as_str(),
            memory.title,
            memory.text,
            memory.agent,
            memory.topic,
            text_hash(&memory.text),
            memory.revision,
            memory.updated_at,
            memory.last_seen_at
        ],
    )?;
    Ok(())
}

impl Store {
    /// Saves an entry at `at` (milliseconds since the Unix epoch). In this
    /// order: a text a live entry of the scope already says is not added
    /// ([`Saved::Known`], the repeat counted on that entry, whatever its
    /// topic); a topic a live entry of the scope already has revises that
    /// entry ([`Saved::Revised`]); anything else is a new entry.
    pub fn add_memory(&self, new: &NewMemory, at: i64) -> Result<Saved> {
        new.scope.check()?;
        let text = checked_text(&new.text)?;
        let agent = new.agent.as_deref().map(agent_tag).transpose()?;
        let topic = new.topic.as_deref().map(topic_key).transpose()?;
        // Checked before anything is looked up: a title that is too long is
        // refused whichever way the entry would be saved.
        let title = checked_title(new.title.as_deref(), &text)?;
        let key = new.scope.key();
        let hash = text_hash(&text);
        self.write(StoreChange::Memory, |tx| {
            if let Some(mut known) = live_with_hash(tx, &key, &hash, None)? {
                known.duplicates = known.duplicates.saturating_add(1);
                known.last_seen_at = at;
                tx.execute(
                    "UPDATE memory SET duplicates = ?2, last_seen_at = ?3 WHERE id = ?1",
                    params![known.id, known.duplicates, at],
                )?;
                return Ok(Saved::Known(known));
            }
            let revised = match &topic {
                Some(topic) => live_with_topic(tx, &key, topic, None)?,
                None => None,
            };
            if let Some(mut memory) = revised {
                // A title that was made from the text follows the text.
                let derived = memory.title == title_of(&memory.text);
                if new.title.is_some() || derived {
                    memory.title = title;
                }
                memory.text = text;
                if let Some(kind) = new.kind {
                    memory.kind = kind;
                }
                if agent.is_some() {
                    memory.agent = agent;
                }
                memory.revision = memory.revision.saturating_add(1);
                memory.updated_at = at;
                memory.last_seen_at = at;
                write_words(tx, &memory)?;
                return Ok(Saved::Revised(memory));
            }
            if live_count(tx, &key)? >= MAX_ENTRIES {
                return Err(MemoryError::ScopeFull { max: MAX_ENTRIES }.into());
            }
            let memory = Memory {
                id: uuid::Uuid::now_v7().to_string(),
                scope: new.scope.clone(),
                kind: new.kind.unwrap_or(MemoryKind::Note),
                title,
                text,
                agent,
                created_at: at,
                updated_at: at,
                topic,
                revision: 1,
                duplicates: 0,
                last_seen_at: at,
                pinned: false,
                forgotten_at: None,
            };
            tx.execute(
                "INSERT INTO memory (id, scope, root, kind, title, text, agent, topic, text_hash,
                                     created_at, updated_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?10)",
                params![
                    memory.id,
                    key,
                    memory.scope.root().unwrap_or_default(),
                    memory.kind.as_str(),
                    memory.title,
                    memory.text,
                    memory.agent,
                    memory.topic,
                    hash,
                    at
                ],
            )?;
            Ok(Saved::New(memory))
        })
    }

    /// Edits a live entry at `at` and returns it as it now is, its revision
    /// one higher. What the patch leaves out stays; a new text without a new
    /// title keeps a title somebody gave, and makes a new one where the title
    /// was the text's first line. Refused when another live entry of the
    /// scope already says the new text, or already has the new topic.
    pub fn update_memory(&self, id: &str, patch: &MemoryPatch, at: i64) -> Result<Memory> {
        if patch.is_empty() {
            return Err(MemoryError::NothingToChange.into());
        }
        let id = self.memory_id(id)?;
        let text = patch.text.as_deref().map(checked_text).transpose()?;
        let topic = match &patch.topic {
            Some(Some(topic)) => Some(Some(topic_key(topic)?)),
            Some(None) => Some(None),
            None => None,
        };
        self.write(StoreChange::Memory, |tx| {
            let mut memory = row_of(tx, &id)?;
            if memory.is_forgotten() {
                return Err(MemoryError::Forgotten {
                    id: memory.short_id().to_owned(),
                }
                .into());
            }
            let key = memory.scope.key();
            if let Some(kind) = patch.kind {
                memory.kind = kind;
            }
            // A title that was made from the text follows the text.
            let derived = memory.title == title_of(&memory.text);
            if let Some(text) = text {
                if let Some(other) = live_with_hash(tx, &key, &text_hash(&text), Some(&id))? {
                    return Err(MemoryError::Duplicate {
                        id: other.short_id().to_owned(),
                    }
                    .into());
                }
                memory.text = text;
            }
            if patch.title.is_some() || derived {
                memory.title = checked_title(patch.title.as_deref(), &memory.text)?;
            }
            if let Some(topic) = topic {
                if let Some(topic) = &topic {
                    if let Some(other) = live_with_topic(tx, &key, topic, Some(&id))? {
                        return Err(MemoryError::TopicTaken {
                            topic: topic.clone(),
                            id: other.short_id().to_owned(),
                        }
                        .into());
                    }
                }
                memory.topic = topic;
            }
            memory.revision = memory.revision.saturating_add(1);
            memory.updated_at = at;
            memory.last_seen_at = at;
            write_words(tx, &memory)?;
            Ok(memory)
        })
    }

    /// Pins a live entry, or unpins it. Pinning one that is pinned, or
    /// unpinning one that is not, changes nothing.
    pub fn pin_memory(&self, id: &str, pinned: bool) -> Result<Memory> {
        let id = self.memory_id(id)?;
        self.write(StoreChange::Memory, |tx| {
            let mut memory = row_of(tx, &id)?;
            if memory.is_forgotten() {
                return Err(MemoryError::Forgotten {
                    id: memory.short_id().to_owned(),
                }
                .into());
            }
            if memory.pinned == pinned {
                return Ok(memory);
            }
            tx.execute(
                "UPDATE memory SET pinned = ?2 WHERE id = ?1",
                params![id, pinned],
            )?;
            memory.pinned = pinned;
            Ok(memory)
        })
    }

    /// Forgets a live entry at `at`: it keeps its row, and is in no listing,
    /// no search and no count until it is restored. It is no longer pinned.
    pub fn forget_memory(&self, id: &str, at: i64) -> Result<Memory> {
        let id = self.memory_id(id)?;
        self.write(StoreChange::Memory, |tx| {
            let mut memory = row_of(tx, &id)?;
            if memory.is_forgotten() {
                return Err(MemoryError::Forgotten {
                    id: memory.short_id().to_owned(),
                }
                .into());
            }
            tx.execute(
                "UPDATE memory SET forgotten_at = ?2, pinned = 0 WHERE id = ?1",
                params![id, at],
            )?;
            memory.forgotten_at = Some(at);
            memory.pinned = false;
            Ok(memory)
        })
    }

    /// Brings a forgotten entry back. Refused when the scope is full, or when
    /// a live entry has taken its topic or says its text in the meantime.
    pub fn restore_memory(&self, id: &str) -> Result<Memory> {
        let id = self.memory_id(id)?;
        self.write(StoreChange::Memory, |tx| {
            let mut memory = row_of(tx, &id)?;
            if !memory.is_forgotten() {
                return Err(MemoryError::NotForgotten {
                    id: memory.short_id().to_owned(),
                }
                .into());
            }
            let key = memory.scope.key();
            if let Some(topic) = &memory.topic {
                if let Some(other) = live_with_topic(tx, &key, topic, None)? {
                    return Err(MemoryError::TopicTaken {
                        topic: topic.clone(),
                        id: other.short_id().to_owned(),
                    }
                    .into());
                }
            }
            if let Some(other) = live_with_hash(tx, &key, &text_hash(&memory.text), None)? {
                return Err(MemoryError::Duplicate {
                    id: other.short_id().to_owned(),
                }
                .into());
            }
            if live_count(tx, &key)? >= MAX_ENTRIES {
                return Err(MemoryError::ScopeFull { max: MAX_ENTRIES }.into());
            }
            tx.execute("UPDATE memory SET forgotten_at = NULL WHERE id = ?1", [&id])?;
            memory.forgotten_at = None;
            Ok(memory)
        })
    }

    /// Removes an entry for good, live or forgotten, and returns what it was.
    pub fn delete_memory(&self, id: &str) -> Result<Memory> {
        let id = self.memory_id(id)?;
        self.write(StoreChange::Memory, |tx| {
            let memory = row_of(tx, &id)?;
            tx.execute("DELETE FROM memory WHERE id = ?1", [&id])?;
            Ok(memory)
        })
    }

    /// Removes for good every entry, of every scope, that was forgotten
    /// before `before` (see [`purge_before`]). Returns how many went.
    pub fn purge_memories(&self, before: i64) -> Result<usize> {
        self.write(StoreChange::Memory, |tx| {
            Ok(tx.execute(
                "DELETE FROM memory WHERE forgotten_at IS NOT NULL AND forgotten_at < ?1",
                [before],
            )?)
        })
    }

    /// The entry with this id, or with an id that ends in it (see
    /// [`Memory::short_id`]), live or forgotten.
    pub fn memory(&self, id: &str) -> Result<Memory> {
        let id = self.memory_id(id)?;
        self.read(|connection| row_of(connection, &id))
    }

    /// The whole id that `given` names: itself, or the one id that ends in it
    /// when it is at least [`MIN_ID_TAIL`] characters of one.
    fn memory_id(&self, given: &str) -> Result<String> {
        let given = given.trim().to_ascii_lowercase();
        // Only what an id is made of: nothing in it can be a pattern.
        let plain = !given.is_empty() && given.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        if !plain {
            return Err(StoreError::NotFound("memory"));
        }
        self.read(|connection| {
            let exact: Option<String> = connection
                .query_row("SELECT id FROM memory WHERE id = ?1", [&given], |row| {
                    row.get(0)
                })
                .optional()?;
            if let Some(id) = exact {
                return Ok(id);
            }
            if given.len() < MIN_ID_TAIL {
                return Err(StoreError::NotFound("memory"));
            }
            let mut statement = connection
                .prepare_cached("SELECT id FROM memory WHERE id LIKE '%' || ?1 LIMIT 2")?;
            let found: Vec<String> = statement
                .query_map([&given], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            match found.as_slice() {
                [] => Err(StoreError::NotFound("memory")),
                [id] => Ok(id.clone()),
                _ => Err(MemoryError::AmbiguousId(given.clone()).into()),
            }
        })
    }

    /// The live entries of one scope: the pinned ones first, then the last
    /// changed first, at most `limit` (and never more than [`MAX_LISTED`]).
    pub fn memories(&self, scope: &MemoryScope, limit: usize) -> Result<Vec<Memory>> {
        self.listed(scope, limit, false)
    }

    /// The forgotten entries of one scope, the last forgotten first.
    pub fn forgotten_memories(&self, scope: &MemoryScope, limit: usize) -> Result<Vec<Memory>> {
        self.listed(scope, limit, true)
    }

    fn listed(&self, scope: &MemoryScope, limit: usize, forgotten: bool) -> Result<Vec<Memory>> {
        let key = scope.key();
        let which = if forgotten {
            "m.forgotten_at IS NOT NULL ORDER BY m.forgotten_at DESC, m.pk DESC"
        } else {
            "m.forgotten_at IS NULL ORDER BY m.pinned DESC, m.updated_at DESC, m.pk DESC"
        };
        self.read(|connection| {
            let mut statement = connection.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM memory m WHERE m.scope = ?1 AND {which} LIMIT ?2"
            ))?;
            let found = statement
                .query_map(
                    params![key, sql_limit(limit.min(MAX_LISTED))],
                    memory_from_row,
                )?
                .collect::<rusqlite::Result<_>>()?;
            Ok(found)
        })
    }

    /// How many live entries a scope holds.
    pub fn memory_count(&self, scope: &MemoryScope) -> Result<usize> {
        Ok(self.memory_counts(scope)?.live)
    }

    /// How many live, pinned and forgotten entries a scope holds.
    pub fn memory_counts(&self, scope: &MemoryScope) -> Result<MemoryCounts> {
        let key = scope.key();
        self.read(|connection| {
            let (live, pinned, forgotten): (i64, i64, i64) = connection.query_row(
                "SELECT count(*) FILTER (WHERE forgotten_at IS NULL),
                        count(*) FILTER (WHERE forgotten_at IS NULL AND pinned = 1),
                        count(*) FILTER (WHERE forgotten_at IS NOT NULL)
                 FROM memory WHERE scope = ?1",
                [&key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let count = |value: i64| usize::try_from(value).unwrap_or_default();
            Ok(MemoryCounts {
                live: count(live),
                pinned: count(pinned),
                forgotten: count(forgotten),
            })
        })
    }

    /// Searches the titles and texts of the live entries of a scope, best
    /// match first. A project is searched together with the global scope; the
    /// global scope alone. Words that hold nothing searchable find nothing.
    pub fn search_memories(
        &self,
        scope: &MemoryScope,
        words: &str,
        limit: usize,
    ) -> Result<Vec<MemoryHit>> {
        let Some(expression) = fts_query(words) else {
            return Ok(Vec::new());
        };
        let key = scope.key();
        let (start, end) = (SNIPPET_START.to_string(), SNIPPET_END.to_string());
        self.read(|connection| {
            // The index holds the words of forgotten entries too: they are
            // left out here.
            let mut statement = connection.prepare_cached(&format!(
                "SELECT {COLUMNS},
                        snippet(memory_fts, 1, ?3, ?4, ?5, {SNIPPET_WORDS}),
                        memory_fts.rank
                 FROM memory_fts
                 JOIN memory m ON m.pk = memory_fts.rowid
                 WHERE memory_fts MATCH ?1 AND m.scope IN (?2, '{GLOBAL_SCOPE}')
                   AND m.forgotten_at IS NULL
                 ORDER BY memory_fts.rank, m.updated_at DESC
                 LIMIT ?6"
            ))?;
            let hits = statement
                .query_map(
                    params![
                        expression,
                        key,
                        start,
                        end,
                        SNIPPET_ELLIPSIS,
                        sql_limit(limit.min(MAX_LISTED))
                    ],
                    |row| {
                        Ok(MemoryHit {
                            memory: memory_from_row(row)?,
                            snippet: row.get(COLUMN_COUNT)?,
                            rank: row.get(COLUMN_COUNT + 1)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<_>>()?;
            Ok(hits)
        })
    }

    /// Records what was decided about the project rooted at `root`, at `at`.
    /// The record is the root's (see [`MemoryScope`]), not the project's id:
    /// it outlives the project being removed and added again.
    pub fn set_memory_choice(&self, root: &str, choice: MemoryChoice, at: i64) -> Result<()> {
        let scope = MemoryScope::project(root);
        scope.check()?;
        self.write(StoreChange::Memory, |tx| {
            tx.execute(
                "INSERT INTO memory_choice (scope, root, answer, answered_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (scope) DO UPDATE
                 SET root = excluded.root, answer = excluded.answer,
                     answered_at = excluded.answered_at",
                params![scope.key(), root, choice.as_str(), at],
            )?;
            Ok(())
        })
    }

    /// What was decided about the project rooted at `root`, and when. `None`
    /// when nobody decided anything.
    pub fn memory_choice(&self, root: &str) -> Result<Option<(MemoryChoice, i64)>> {
        let key = MemoryScope::project(root).key();
        self.read(|connection| {
            let found: Option<(String, i64)> = connection
                .query_row(
                    "SELECT answer, answered_at FROM memory_choice WHERE scope = ?1",
                    [&key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            Ok(found.and_then(|(answer, at)| Some((MemoryChoice::parse(&answer)?, at))))
        })
    }

    /// Every recorded decision, by root as it was written.
    pub fn memory_choices(&self) -> Result<Vec<(String, MemoryChoice)>> {
        self.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT root, answer FROM memory_choice ORDER BY scope")?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows
                .into_iter()
                .filter_map(|(root, answer)| Some((root, MemoryChoice::parse(&answer)?)))
                .collect())
        })
    }

    /// The root of the project that owns `folder` on `machine`: the folder is
    /// the project's root, lies below it, or is (or lies below) one of its
    /// git worktrees, wherever that worktree is. `None` when no project of
    /// the store owns the folder.
    pub fn memory_root(&self, machine: &MachineId, folder: &str) -> Result<Option<String>> {
        self.read(|connection| {
            let roots = project_roots(connection, machine)?;
            let Some(project) = owning_project(&roots, folder) else {
                return Ok(None);
            };
            let root = connection
                .query_row(
                    "SELECT root FROM project WHERE id = ?1",
                    [project.as_str()],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(root)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewWorktree;
    use crate::store::search::snippet_segments;

    fn api() -> MemoryScope {
        MemoryScope::project("/srv/api")
    }

    fn add(store: &Store, scope: MemoryScope, text: &str, at: i64) -> Memory {
        store
            .add_memory(&NewMemory::note(scope, text), at)
            .unwrap()
            .into_memory()
    }

    fn texts(found: &[Memory]) -> Vec<&str> {
        found.iter().map(|memory| memory.text.as_str()).collect()
    }

    #[test]
    fn an_entry_is_kept_as_it_was_written() {
        let store = Store::open_in_memory().unwrap();
        let mut changes = store.subscribe();
        let added = store
            .add_memory(
                &NewMemory {
                    scope: api(),
                    kind: Some(MemoryKind::Decision),
                    title: Some("  Money is integer cents ".into()),
                    text: "Amounts are i64 cents.\r\nNever floats.\n".into(),
                    agent: Some("Claude".into()),
                    topic: None,
                },
                1_000,
            )
            .unwrap()
            .into_memory();
        assert_eq!(changes.try_next(), Some(StoreChange::Memory));
        assert_eq!(added.title, "Money is integer cents");
        assert_eq!(added.text, "Amounts are i64 cents.\nNever floats.");
        assert_eq!(added.agent.as_deref(), Some("claude"));
        assert_eq!((added.created_at, added.updated_at), (1_000, 1_000));
        assert_eq!(store.memory(&added.id).unwrap(), added);
        assert_eq!(store.memories(&api(), 10).unwrap(), [added]);
    }

    #[test]
    fn a_text_without_a_title_is_titled_by_its_first_line() {
        assert_eq!(title_of("Use pnpm.\nNot npm."), "Use pnpm.");
        let long = "word ".repeat(40);
        let title = title_of(&long);
        assert!(title.chars().count() <= 73, "{title:?}");
        assert!(title.ends_with(SNIPPET_ELLIPSIS));
        assert!(!title.contains("  "));
    }

    #[test]
    fn what_is_too_long_or_empty_is_refused_and_nothing_is_written() {
        let store = Store::open_in_memory().unwrap();
        let mut changes = store.subscribe();
        let refused = |new: NewMemory| store.add_memory(&new, 1).unwrap_err().to_string();
        assert!(refused(NewMemory::note(api(), " \n ")).contains("empty"));
        assert!(
            refused(NewMemory::note(api(), "x".repeat(MAX_TEXT_CHARS + 1)))
                .contains("one fact per entry")
        );
        let mut titled = NewMemory::note(api(), "fine");
        titled.title = Some("t".repeat(MAX_TITLE_CHARS + 1));
        assert!(refused(titled).contains("title"));
        let mut tagged = NewMemory::note(api(), "fine");
        tagged.agent = Some("not a tag".into());
        assert!(refused(tagged).contains("agent tag"));
        assert!(
            refused(NewMemory::note(MemoryScope::project("relative/x"), "fine"))
                .contains("absolute")
        );
        assert_eq!(changes.try_next(), None);
        assert_eq!(store.memory_count(&api()).unwrap(), 0);
        assert_eq!((MAX_TEXT_CHARS, MAX_TITLE_CHARS), (8_000, 120));
        assert!(store
            .add_memory(&NewMemory::note(api(), "y".repeat(7_999)), 1)
            .is_ok());
        // The longest of each is accepted.
        let mut longest = NewMemory::note(api(), "é".repeat(MAX_TEXT_CHARS));
        longest.title = Some("t".repeat(MAX_TITLE_CHARS));
        assert!(store.add_memory(&longest, 1).is_ok());
    }

    #[test]
    fn control_characters_never_reach_the_store() {
        let store = Store::open_in_memory().unwrap();
        let mut new = NewMemory::note(api(), "a\u{1b}[31mb\u{2}c\td\u{0}");
        new.title = Some("one\ntwo\u{7}three".into());
        let added = store.add_memory(&new, 1).unwrap().into_memory();
        assert_eq!(added.text, "a[31mbc\td");
        assert_eq!(added.title, "one two three");
    }

    #[test]
    fn a_kind_is_one_of_five_words() {
        for kind in MemoryKind::ALL {
            assert_eq!(MemoryKind::parse(kind.as_str()), Ok(kind));
        }
        assert_eq!(MemoryKind::parse(" Decision "), Ok(MemoryKind::Decision));
        assert!(matches!(
            MemoryKind::parse("rule"),
            Err(MemoryError::UnknownKind(_))
        ));
    }

    #[test]
    fn a_scope_lists_its_own_entries_the_last_changed_first() {
        let store = Store::open_in_memory().unwrap();
        add(&store, api(), "first", 1);
        add(&store, api(), "second", 2);
        add(&store, MemoryScope::Global, "everywhere", 3);
        add(&store, MemoryScope::project("/srv/web"), "other", 4);
        assert_eq!(
            texts(&store.memories(&api(), 10).unwrap()),
            ["second", "first"]
        );
        assert_eq!(
            texts(&store.memories(&MemoryScope::Global, 10).unwrap()),
            ["everywhere"]
        );
        assert_eq!(texts(&store.memories(&api(), 1).unwrap()), ["second"]);
        assert_eq!(store.memory_count(&api()).unwrap(), 2);
    }

    #[test]
    fn spellings_of_one_root_are_one_scope() {
        let store = Store::open_in_memory().unwrap();
        add(&store, MemoryScope::project("/srv/api/"), "posix", 1);
        add(&store, MemoryScope::project("C:\\Code\\Api"), "windows", 2);
        assert_eq!(texts(&store.memories(&api(), 10).unwrap()), ["posix"]);
        let found = store
            .memories(&MemoryScope::project("c:/code/api/"), 10)
            .unwrap();
        assert_eq!(texts(&found), ["windows"]);
        // The root is shown as it was written.
        assert_eq!(found[0].scope, MemoryScope::project("C:\\Code\\Api"));
    }

    #[test]
    fn a_folder_cannot_pose_as_the_global_scope() {
        let store = Store::open_in_memory().unwrap();
        add(&store, MemoryScope::Global, "everywhere", 1);
        assert!(store
            .add_memory(&NewMemory::note(MemoryScope::project("global:"), "x"), 1)
            .is_err());
        assert_eq!(store.memory_count(&MemoryScope::Global).unwrap(), 1);
    }

    #[test]
    fn an_entry_is_changed_and_forgotten_by_its_id_or_the_end_of_it() {
        let store = Store::open_in_memory().unwrap();
        let added = add(&store, api(), "use npm", 1);
        let other = add(&store, api(), "keep me", 2);
        let changed = store
            .update_memory(
                added.short_id(),
                &MemoryPatch {
                    kind: Some(MemoryKind::Convention),
                    title: None,
                    text: Some("use pnpm".into()),
                    topic: None,
                },
                5,
            )
            .unwrap();
        assert_eq!(changed.kind, MemoryKind::Convention);
        assert_eq!(
            (changed.text.as_str(), changed.title.as_str()),
            ("use pnpm", "use pnpm")
        );
        assert_eq!((changed.created_at, changed.updated_at), (1, 5));
        assert_eq!(changed.revision, 2);
        // The change moved it to the top.
        assert_eq!(store.memories(&api(), 10).unwrap()[0].id, added.id);

        let forgotten = store.forget_memory(&added.id, 9).unwrap();
        assert_eq!(forgotten.text, "use pnpm");
        assert_eq!(forgotten.forgotten_at, Some(9));
        assert_eq!(store.memories(&api(), 10).unwrap(), [other]);
        assert!(store
            .forget_memory(&added.id, 10)
            .unwrap_err()
            .to_string()
            .contains("is forgotten"));
        // For good: the row goes, and its id names nothing.
        assert_eq!(store.delete_memory(&added.id).unwrap().id, added.id);
        assert!(matches!(
            store.memory(&added.id),
            Err(StoreError::NotFound("memory"))
        ));
    }

    #[test]
    fn an_id_that_names_nothing_or_too_much_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let added = add(&store, api(), "one", 1);
        for given in ["", "%", "_______", "abc", "zzzzzzzz", "0000000000"] {
            assert!(
                matches!(store.memory(given), Err(StoreError::NotFound("memory"))),
                "{given:?}"
            );
        }
        // A tail shorter than the least is not looked up, even when it fits.
        let tail = &added.id[added.id.len() - (MIN_ID_TAIL - 1)..];
        assert!(store.memory(tail).is_err());
        // Two ids with one ending: the ending names neither.
        store
            .write(StoreChange::Memory, |tx| {
                tx.execute(
                    "INSERT INTO memory (id, scope, root, kind, title, text, text_hash, created_at, updated_at, last_seen_at)
                     VALUES ('aaaa-11112222', '/x', '/x', 'note', 't', 't', 'h', 1, 1, 1),
                            ('bbbb-11112222', '/x', '/x', 'note', 't', 't', 'h', 1, 1, 1)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(store
            .memory("11112222")
            .unwrap_err()
            .to_string()
            .contains("more than one"));
        assert!(store.memory("aaaa-11112222").is_ok());
    }

    #[test]
    fn a_search_covers_the_project_and_the_global_scope_and_nothing_else() {
        let store = Store::open_in_memory().unwrap();
        add(&store, api(), "the scheduler retries three times", 1);
        add(
            &store,
            MemoryScope::Global,
            "prefer a scheduler over cron",
            2,
        );
        add(&store, MemoryScope::project("/srv/web"), "web scheduler", 3);
        let hits = store.search_memories(&api(), "scheduler", 10).unwrap();
        let mut found: Vec<_> = hits.iter().map(|hit| hit.memory.text.as_str()).collect();
        found.sort_unstable();
        assert_eq!(
            found,
            [
                "prefer a scheduler over cron",
                "the scheduler retries three times"
            ]
        );
        let global = store
            .search_memories(&MemoryScope::Global, "scheduler", 10)
            .unwrap();
        assert_eq!(global.len(), 1);
        assert_eq!(global[0].memory.scope, MemoryScope::Global);
        assert_eq!(store.search_memories(&api(), "sched", 1).unwrap().len(), 1);
    }

    #[test]
    fn a_hit_carries_a_snippet_with_the_matched_words_marked() {
        let store = Store::open_in_memory().unwrap();
        add(
            &store,
            api(),
            "Résumé parsing lives in the importer crate",
            1,
        );
        let hits = store
            .search_memories(&api(), "resume IMPORTER", 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        let marked: Vec<&str> = snippet_segments(&hits[0].snippet)
            .into_iter()
            .filter_map(|(matched, piece)| matched.then_some(piece))
            .collect();
        assert_eq!(marked, ["Résumé", "importer"]);
    }

    #[test]
    fn a_title_is_searched_too_and_better_matches_come_first() {
        let store = Store::open_in_memory().unwrap();
        let mut titled = NewMemory::note(api(), "amounts are integer cents");
        titled.title = Some("Money".into());
        store.add_memory(&titled, 1).unwrap();
        add(
            &store,
            api(),
            "a long entry that mentions money once among many unrelated words about \
             servers, queues, workers, retries, deadlines, dashboards and reports",
            2,
        );
        let hits = store.search_memories(&api(), "money", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].memory.title, "Money");
        assert!(hits[0].rank <= hits[1].rank);
    }

    #[test]
    fn hostile_words_find_nothing_and_never_fail() {
        let store = Store::open_in_memory().unwrap();
        add(&store, api(), "plain text", 1);
        for words in ["", "\"", "(", "NEAR(", "text:plain", "a\0b", "* - ^", "🎉"] {
            assert!(
                store.search_memories(&api(), words, 10).is_ok(),
                "{words:?}"
            );
        }
    }

    #[test]
    fn changed_and_forgotten_text_is_no_longer_found() {
        let store = Store::open_in_memory().unwrap();
        let added = add(&store, api(), "the old wording", 1);
        let patch = MemoryPatch {
            text: Some("the new wording".into()),
            ..MemoryPatch::default()
        };
        store.update_memory(&added.id, &patch, 2).unwrap();
        assert!(store.search_memories(&api(), "old", 10).unwrap().is_empty());
        assert_eq!(store.search_memories(&api(), "new", 10).unwrap().len(), 1);
        // A title somebody gave stays when only the text changes.
        let mut titled = NewMemory::note(api(), "body one");
        titled.title = Some("Given".into());
        let titled = store.add_memory(&titled, 3).unwrap().into_memory();
        let patch = MemoryPatch {
            text: Some("body two".into()),
            ..MemoryPatch::default()
        };
        assert_eq!(
            store.update_memory(&titled.id, &patch, 4).unwrap().title,
            "Given"
        );
        store.forget_memory(&added.id, 5).unwrap();
        assert!(store
            .search_memories(&api(), "wording", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_scope_is_full_at_its_bound() {
        let store = Store::open_in_memory().unwrap();
        store
            .write(StoreChange::Memory, |tx| {
                let mut insert = tx.prepare(
                    "INSERT INTO memory (id, scope, root, kind, title, text, text_hash, created_at, updated_at, last_seen_at)
                     VALUES (?1, '/srv/api', '/srv/api', 'note', 't', 't', 'h', 1, 1, 1)",
                )?;
                for index in 0..MAX_ENTRIES {
                    insert.execute([format!("id-{index}")])?;
                }
                Ok(())
            })
            .unwrap();
        assert!(store
            .add_memory(&NewMemory::note(api(), "one more"), 2)
            .unwrap_err()
            .to_string()
            .contains("full"));
        // Another scope is not.
        assert!(store
            .add_memory(&NewMemory::note(MemoryScope::Global, "fine"), 2)
            .is_ok());
        assert_eq!(
            store.memories(&api(), usize::MAX).unwrap().len(),
            MAX_LISTED
        );
    }

    fn topical(scope: MemoryScope, topic: &str, text: &str) -> NewMemory {
        let mut new = NewMemory::note(scope, text);
        new.topic = Some(topic.to_owned());
        new
    }

    #[test]
    fn a_text_is_normalised_before_it_is_hashed() {
        assert_eq!(
            normalized("  Use   PNPM,\n not npm.  "),
            "use pnpm, not npm"
        );
        assert_eq!(normalized("\"Résumé parsing!\""), "résumé parsing");
        assert_eq!(normalized("(a) b [c]"), "a) b [c");
        // Punctuation alone is kept, so two such texts are two.
        assert_eq!(normalized(" ?! "), "?!");
        assert_ne!(text_hash("?!"), text_hash("!!"));
        assert_eq!(text_hash("Use pnpm."), text_hash("  use\tPNPM  "));
        assert_ne!(text_hash("use pnpm"), text_hash("use npm"));
        // Inside the text, punctuation counts.
        assert_ne!(text_hash("a.b"), text_hash("a b"));
        assert_eq!(text_hash("x").len(), 32);
    }

    #[test]
    fn a_topic_is_a_short_plain_key() {
        assert_eq!(
            topic_key(" Auth/Token-Format ").unwrap(),
            "auth/token-format"
        );
        assert_eq!(topic_key("db:pool_size.v2").unwrap(), "db:pool_size.v2");
        assert!(topic_key(&"a".repeat(MAX_TOPIC_CHARS)).is_ok());
        for bad in [
            "",
            " ",
            "two words",
            "/leading",
            "-x",
            "ünï",
            "a\nb",
            "a#b",
            "<!--",
            &"a".repeat(MAX_TOPIC_CHARS + 1),
        ] {
            assert!(
                matches!(topic_key(bad), Err(MemoryError::BadTopic(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn saving_under_a_live_topic_revises_that_entry_in_place() {
        let store = Store::open_in_memory().unwrap();
        let first = store
            .add_memory(&topical(api(), "auth/token-format", "Tokens are JWT."), 1)
            .unwrap();
        assert!(matches!(first, Saved::New(_)));
        let first = first.into_memory();
        assert_eq!(
            (first.revision, first.topic.as_deref()),
            (1, Some("auth/token-format"))
        );

        let mut second = topical(api(), "AUTH/Token-Format", "Tokens are opaque, 32 bytes.");
        second.kind = Some(MemoryKind::Decision);
        second.agent = Some("codex".into());
        let Saved::Revised(revised) = store.add_memory(&second, 7).unwrap() else {
            panic!("expected a revision");
        };
        assert_eq!(revised.id, first.id);
        assert_eq!(revised.text, "Tokens are opaque, 32 bytes.");
        assert_eq!(revised.title, "Tokens are opaque, 32 bytes.");
        assert_eq!(revised.kind, MemoryKind::Decision);
        assert_eq!(revised.agent.as_deref(), Some("codex"));
        assert_eq!(
            (revised.revision, revised.created_at, revised.updated_at),
            (2, 1, 7)
        );
        assert_eq!(
            store.memories(&api(), 10).unwrap(),
            std::slice::from_ref(&revised)
        );
        // The old words are gone from the search, the new ones are found.
        assert!(store.search_memories(&api(), "JWT", 10).unwrap().is_empty());
        assert_eq!(
            store.search_memories(&api(), "opaque", 10).unwrap().len(),
            1
        );

        let third = store
            .add_memory(
                &topical(api(), "auth/token-format", "Tokens are PASETO."),
                9,
            )
            .unwrap();
        assert_eq!(third.memory().revision, 3);
        assert_eq!(
            third.memory().kind,
            MemoryKind::Decision,
            "no kind given: it stays"
        );
        assert_eq!(
            third.memory().agent.as_deref(),
            Some("codex"),
            "nobody new said who"
        );
        assert_eq!(store.memory_count(&api()).unwrap(), 1);
    }

    #[test]
    fn two_scopes_can_hold_the_same_topic() {
        let store = Store::open_in_memory().unwrap();
        let web = MemoryScope::project("/srv/web");
        for (scope, text) in [
            (api(), "api says"),
            (web.clone(), "web says"),
            (MemoryScope::Global, "all say"),
        ] {
            assert!(matches!(
                store.add_memory(&topical(scope, "style", text), 1).unwrap(),
                Saved::New(_)
            ));
        }
        assert_eq!(texts(&store.memories(&api(), 10).unwrap()), ["api says"]);
        assert_eq!(texts(&store.memories(&web, 10).unwrap()), ["web says"]);
        // Without a topic nothing is revised.
        add(&store, api(), "one", 2);
        add(&store, api(), "two", 3);
        assert_eq!(store.memory_count(&api()).unwrap(), 3);
    }

    #[test]
    fn a_title_somebody_gave_survives_a_revision_and_a_bad_topic_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let mut first = topical(api(), "ports", "The api listens on 8080.");
        first.title = Some("Ports".into());
        store.add_memory(&first, 1).unwrap();
        let revised = store
            .add_memory(&topical(api(), "ports", "The api listens on 9090."), 2)
            .unwrap()
            .into_memory();
        assert_eq!(revised.title, "Ports");
        assert!(store
            .add_memory(&topical(api(), "two words", "x"), 3)
            .unwrap_err()
            .to_string()
            .contains("is not a topic"));
    }

    #[test]
    fn the_same_text_is_not_added_twice_whatever_its_case_and_spacing() {
        let store = Store::open_in_memory().unwrap();
        let first = add(&store, api(), "Use pnpm, not npm.", 1);
        let mut changes = store.subscribe();
        let Saved::Known(known) = store
            .add_memory(&NewMemory::note(api(), "  use   PNPM,\nnot npm  "), 5)
            .unwrap()
        else {
            panic!("expected a known entry");
        };
        assert_eq!(known.id, first.id);
        assert_eq!(known.text, "Use pnpm, not npm.", "the first wording stays");
        assert_eq!(
            (known.duplicates, known.last_seen_at, known.updated_at),
            (1, 5, 1)
        );
        assert_eq!(known.revision, 1);
        assert_eq!(changes.try_next(), Some(StoreChange::Memory));
        assert_eq!(store.memory_count(&api()).unwrap(), 1);
        assert_eq!(store.memory(&first.id).unwrap(), known);
        // Also under a topic: what is known is known.
        let again = store
            .add_memory(&topical(api(), "tooling", "USE PNPM, NOT NPM!"), 6)
            .unwrap();
        assert!(
            matches!(&again, Saved::Known(memory) if memory.duplicates == 2 && memory.topic.is_none())
        );
        // Another scope has not heard it.
        assert!(matches!(
            store
                .add_memory(
                    &NewMemory::note(MemoryScope::Global, "use pnpm, not npm"),
                    7
                )
                .unwrap(),
            Saved::New(_)
        ));
    }

    #[test]
    fn a_forgotten_duplicate_does_not_block_a_new_entry() {
        let store = Store::open_in_memory().unwrap();
        let first = add(&store, api(), "Use pnpm.", 1);
        store.forget_memory(&first.id, 2).unwrap();
        let Saved::New(second) = store
            .add_memory(&NewMemory::note(api(), "use pnpm"), 3)
            .unwrap()
        else {
            panic!("expected a new entry");
        };
        assert_ne!(second.id, first.id);
        // And a forgotten topic does not capture a save.
        let old = store
            .add_memory(&topical(api(), "ports", "8080"), 4)
            .unwrap()
            .into_memory();
        store.forget_memory(&old.id, 5).unwrap();
        let new = store
            .add_memory(&topical(api(), "ports", "9090"), 6)
            .unwrap();
        assert!(matches!(&new, Saved::New(memory) if memory.id != old.id && memory.revision == 1));
    }

    #[test]
    fn a_forgotten_entry_is_in_no_listing_search_or_count_and_can_be_restored() {
        let store = Store::open_in_memory().unwrap();
        let kept = add(&store, api(), "the scheduler retries", 1);
        let gone = add(&store, api(), "the scheduler is cron", 2);
        store.pin_memory(&gone.id, true).unwrap();
        let forgotten = store.forget_memory(gone.short_id(), 10).unwrap();
        assert!(forgotten.is_forgotten() && !forgotten.pinned);

        assert_eq!(
            store.memories(&api(), 10).unwrap(),
            std::slice::from_ref(&kept)
        );
        assert_eq!(
            store.forgotten_memories(&api(), 10).unwrap(),
            std::slice::from_ref(&forgotten)
        );
        assert_eq!(
            store.memory_counts(&api()).unwrap(),
            MemoryCounts {
                live: 1,
                pinned: 0,
                forgotten: 1
            }
        );
        let hits = store.search_memories(&api(), "scheduler", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory.id, kept.id);
        assert!(store
            .search_memories(&api(), "cron", 10)
            .unwrap()
            .is_empty());
        // It is still there by its id, and cannot be edited or pinned as it is.
        assert_eq!(store.memory(&gone.id).unwrap(), forgotten);
        let patch = MemoryPatch {
            kind: Some(MemoryKind::Decision),
            ..MemoryPatch::default()
        };
        assert!(store
            .update_memory(&gone.id, &patch, 11)
            .unwrap_err()
            .to_string()
            .contains("restore it first"));
        assert!(store.pin_memory(&gone.id, true).is_err());

        let restored = store.restore_memory(&gone.id).unwrap();
        assert!(!restored.is_forgotten());
        assert_eq!(store.search_memories(&api(), "cron", 10).unwrap().len(), 1);
        assert_eq!(store.memory_count(&api()).unwrap(), 2);
        assert!(store.forgotten_memories(&api(), 10).unwrap().is_empty());
        assert!(store
            .restore_memory(&gone.id)
            .unwrap_err()
            .to_string()
            .contains("is not forgotten"));
    }

    #[test]
    fn restoring_is_refused_when_a_live_entry_took_the_topic_or_says_the_text() {
        let store = Store::open_in_memory().unwrap();
        let old = store
            .add_memory(&topical(api(), "ports", "8080"), 1)
            .unwrap()
            .into_memory();
        store.forget_memory(&old.id, 2).unwrap();
        let new = store
            .add_memory(&topical(api(), "ports", "9090"), 3)
            .unwrap()
            .into_memory();
        let refused = store.restore_memory(&old.id).unwrap_err().to_string();
        assert!(refused.contains("already has the topic ports"), "{refused}");
        assert!(refused.contains(new.short_id()), "{refused}");
        // With the topic free again it comes back.
        store.forget_memory(&new.id, 4).unwrap();
        assert!(store.restore_memory(&old.id).is_ok());

        let said = add(&store, api(), "Use pnpm.", 5);
        store.forget_memory(&said.id, 6).unwrap();
        let again = add(&store, api(), "use pnpm", 7);
        let refused = store.restore_memory(&said.id).unwrap_err().to_string();
        assert!(refused.contains("already says this"), "{refused}");
        assert!(refused.contains(again.short_id()), "{refused}");
    }

    #[test]
    fn restoring_into_a_full_scope_is_refused_and_forgotten_entries_do_not_fill_it() {
        let store = Store::open_in_memory().unwrap();
        let first = add(&store, api(), "the first", 1);
        store.forget_memory(&first.id, 2).unwrap();
        store
            .write(StoreChange::Memory, |tx| {
                let mut insert = tx.prepare(
                    "INSERT INTO memory (id, scope, root, kind, title, text, text_hash, created_at, updated_at, last_seen_at)
                     VALUES (?1, '/srv/api', '/srv/api', 'note', 't', 't', ?1, 1, 1, 1)",
                )?;
                for index in 0..MAX_ENTRIES - 1 {
                    insert.execute([format!("id-{index}")])?;
                }
                Ok(())
            })
            .unwrap();
        // One below the bound, a forgotten one aside: there is room for one.
        add(&store, api(), "the last", 3);
        assert!(store
            .restore_memory(&first.id)
            .unwrap_err()
            .to_string()
            .contains("full"));
    }

    #[test]
    fn pinned_entries_come_first_and_there_is_no_bound_on_them() {
        let store = Store::open_in_memory().unwrap();
        let entries: Vec<Memory> = (0..40)
            .map(|n| add(&store, api(), &format!("fact {n}"), n))
            .collect();
        let pinned = store.pin_memory(entries[0].short_id(), true).unwrap();
        assert!(pinned.pinned);
        assert_eq!(pinned.updated_at, 0, "pinning is not a change of its words");
        // The oldest is first now, the rest last changed first.
        let listed = store.memories(&api(), 10).unwrap();
        assert_eq!(listed[0].id, entries[0].id);
        assert_eq!(listed[1].id, entries[39].id);
        // Pinning twice is pinning once.
        assert!(store.pin_memory(&entries[0].id, true).is_ok());
        // Every one of them can be pinned.
        for memory in &entries[1..] {
            store.pin_memory(&memory.id, true).unwrap();
        }
        assert_eq!(store.memory_counts(&api()).unwrap().pinned, 40);
        assert!(!store.pin_memory(&entries[0].id, false).unwrap().pinned);
        assert_eq!(store.memory_counts(&api()).unwrap().pinned, 39);
    }

    #[test]
    fn an_edit_of_the_metadata_keeps_the_text_and_counts_as_a_revision() {
        let store = Store::open_in_memory().unwrap();
        let added = add(&store, api(), "Tokens are opaque.", 1);
        let patch = MemoryPatch {
            kind: Some(MemoryKind::Decision),
            title: Some("Token format".into()),
            topic: Some(Some("Auth/Token-Format".into())),
            text: None,
        };
        let edited = store.update_memory(&added.id, &patch, 4).unwrap();
        assert_eq!(edited.text, "Tokens are opaque.");
        assert_eq!(edited.title, "Token format");
        assert_eq!(edited.kind, MemoryKind::Decision);
        assert_eq!(edited.topic.as_deref(), Some("auth/token-format"));
        assert_eq!((edited.revision, edited.updated_at), (2, 4));
        assert_eq!(store.memory(&added.id).unwrap(), edited);
        // The topic can be taken away again.
        let cleared = MemoryPatch {
            topic: Some(None),
            ..MemoryPatch::default()
        };
        let edited = store.update_memory(&added.id, &cleared, 5).unwrap();
        assert_eq!((edited.topic, edited.revision), (None, 3));
        // The hash follows an edit of the text: the old words are free again.
        let reworded = MemoryPatch {
            text: Some("Tokens are JWT.".into()),
            ..MemoryPatch::default()
        };
        store.update_memory(&added.id, &reworded, 6).unwrap();
        assert!(matches!(
            store
                .add_memory(&NewMemory::note(api(), "tokens are jwt"), 7)
                .unwrap(),
            Saved::Known(_)
        ));
        assert!(matches!(
            store
                .add_memory(&NewMemory::note(api(), "tokens are opaque"), 8)
                .unwrap(),
            Saved::New(_)
        ));
        // A change of nothing is refused.
        assert!(store
            .update_memory(&added.id, &MemoryPatch::default(), 9)
            .unwrap_err()
            .to_string()
            .contains("nothing to change"));
    }

    #[test]
    fn an_edit_into_what_another_entry_says_or_is_about_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let one = store
            .add_memory(&topical(api(), "ports", "The api listens on 8080."), 1)
            .unwrap()
            .into_memory();
        let two = add(&store, api(), "Use pnpm.", 2);
        let duplicate = MemoryPatch {
            text: Some("the API listens on 8080".into()),
            ..MemoryPatch::default()
        };
        let refused = store
            .update_memory(&two.id, &duplicate, 3)
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("already says this") && refused.contains(one.short_id()),
            "{refused}"
        );
        let taken = MemoryPatch {
            topic: Some(Some("ports".into())),
            ..MemoryPatch::default()
        };
        let refused = store
            .update_memory(&two.id, &taken, 3)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("already has the topic ports"), "{refused}");
        assert_eq!(store.memory(&two.id).unwrap(), two, "nothing was written");
        // Its own words and its own topic are not another's.
        let same = MemoryPatch {
            text: Some("the api listens on 8080!".into()),
            topic: Some(Some("ports".into())),
            ..MemoryPatch::default()
        };
        assert!(store.update_memory(&one.id, &same, 4).is_ok());
        // What a forgotten entry said is free.
        store.forget_memory(&one.id, 5).unwrap();
        assert!(store.update_memory(&two.id, &duplicate, 6).is_ok());
    }

    #[test]
    fn a_purge_removes_what_was_forgotten_before_a_time_and_nothing_else() {
        let store = Store::open_in_memory().unwrap();
        const DAY: i64 = 86_400_000;
        let live = add(&store, api(), "live", 0);
        let old = add(&store, api(), "forgotten long ago", 0);
        let recent = add(&store, MemoryScope::Global, "forgotten yesterday", 0);
        let now = 100 * DAY;
        store.forget_memory(&old.id, now - 31 * DAY).unwrap();
        store.forget_memory(&recent.id, now - DAY).unwrap();

        assert_eq!(purge_before(now, 30), 70 * DAY);
        assert_eq!(purge_before(now, 0), now);
        assert_eq!(purge_before(5, u32::MAX), 5 - i64::from(u32::MAX) * DAY);
        assert_eq!(store.purge_memories(purge_before(now, 30)).unwrap(), 1);
        assert!(matches!(
            store.memory(&old.id),
            Err(StoreError::NotFound("memory"))
        ));
        assert!(store.memory(&recent.id).is_ok());
        assert!(store.memory(&live.id).is_ok());
        // Its words left the index with its row.
        store.restore_memory(&recent.id).unwrap();
        assert!(store
            .search_memories(&api(), "long ago", 10)
            .unwrap()
            .is_empty());
        assert_eq!(store.purge_memories(purge_before(now, 0)).unwrap(), 0);
        assert_eq!(store.memory_count(&api()).unwrap(), 1);
    }

    #[test]
    fn a_decision_about_a_project_is_kept_by_its_root_and_outlives_the_project() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        assert_eq!(store.memory_choice("/srv/api").unwrap(), None);
        let project = store.add_project(&local, "api", "/srv/api").unwrap();
        store
            .set_memory_choice("/srv/api/", MemoryChoice::Never, 5)
            .unwrap();
        assert_eq!(
            store.memory_choice("/srv/api").unwrap(),
            Some((MemoryChoice::Never, 5))
        );
        // Removed and added again: another id, the same answer.
        store.remove_project(&project.id).unwrap();
        let again = store.add_project(&local, "api", "/srv/api").unwrap();
        assert_ne!(again.id, project.id);
        assert_eq!(
            store.memory_choice(&again.root).unwrap(),
            Some((MemoryChoice::Never, 5))
        );
        // The last decision is the one kept, and another root has none.
        store
            .set_memory_choice("/srv/api", MemoryChoice::On, 9)
            .unwrap();
        store
            .set_memory_choice("/srv/web", MemoryChoice::Off, 9)
            .unwrap();
        assert_eq!(
            store.memory_choice("/srv/api").unwrap(),
            Some((MemoryChoice::On, 9))
        );
        assert_eq!(
            store.memory_choices().unwrap(),
            [
                ("/srv/api".to_owned(), MemoryChoice::On),
                ("/srv/web".to_owned(), MemoryChoice::Off)
            ]
        );
        assert_eq!(store.memory_choice("/srv/other").unwrap(), None);
        assert!(store
            .set_memory_choice("relative", MemoryChoice::On, 1)
            .is_err());
    }

    #[test]
    fn a_folder_resolves_to_the_root_of_the_project_that_owns_it() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        let project = store.add_project(&local, "api", "/srv/api").unwrap();
        store
            .replace_worktrees(
                &project.id,
                vec![
                    NewWorktree {
                        path: "/srv/api".into(),
                        branch: Some("main".into()),
                        head: None,
                        is_main: true,
                    },
                    NewWorktree {
                        path: "/work/trees/api-fix".into(),
                        branch: Some("fix".into()),
                        head: None,
                        is_main: false,
                    },
                ],
            )
            .unwrap();
        let root = |folder: &str| store.memory_root(&local, folder).unwrap();
        assert_eq!(root("/srv/api").as_deref(), Some("/srv/api"));
        assert_eq!(root("/srv/api/src/deep").as_deref(), Some("/srv/api"));
        // A linked worktree outside the root shares the project's memory.
        assert_eq!(root("/work/trees/api-fix").as_deref(), Some("/srv/api"));
        assert_eq!(root("/work/trees/api-fix/src").as_deref(), Some("/srv/api"));
        assert_eq!(root("/srv/api-two"), None);
        assert_eq!(root("/elsewhere"), None);
    }
}

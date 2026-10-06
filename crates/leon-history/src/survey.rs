//! What a history source holds and why parts of it are not imported.
//!
//! A [`Survey`] is the answer to "why is my session missing?": every place a
//! source looked, what format it found there, how many sessions it holds and,
//! for the ones that are not imported, the reason. It carries counts, paths
//! and times only: never a title, a message or anything about an account.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use leon_core::AgentId;

/// Why an item of a source does not become a row of the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkipReason {
    /// A sub-agent session, spawned by another session. Counted, not listed.
    ChildSession,
    /// The session exists but holds no messages (yet).
    Empty,
    /// The same session id is held by two layouts; the database wins.
    DuplicateId,
    /// A file that is not a session transcript.
    NotATranscript,
    /// The item could not be read at all.
    Unreadable,
    /// The database has a layout Leon does not know.
    UnsupportedSchema,
}

impl SkipReason {
    /// The reason as the report words it.
    pub fn label(self) -> &'static str {
        match self {
            Self::ChildSession => "child (sub-agent) sessions",
            Self::Empty => "empty sessions (no messages)",
            Self::DuplicateId => "duplicate session ids",
            Self::NotATranscript => "files that are not transcripts",
            Self::Unreadable => "unreadable items",
            Self::UnsupportedSchema => "unsupported database layout",
        }
    }
}

/// One place a source looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// Where the location came from: `default`, `setting`, `environment` or
    /// `found beside`.
    pub origin: String,
    /// The path that was looked at.
    pub path: PathBuf,
    /// Whether it exists.
    pub exists: bool,
    /// What was found there: format, schema, sizes. No content.
    pub details: Vec<String>,
}

impl Place {
    /// A place with no details yet.
    pub fn new(origin: &str, path: impl Into<PathBuf>, exists: bool) -> Self {
        Self {
            origin: origin.to_owned(),
            path: path.into(),
            exists,
            details: Vec::new(),
        }
    }
}

/// The findings of one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    /// Whose sessions these are.
    pub agent: AgentId,
    /// Every place that was looked at.
    pub places: Vec<Place>,
    /// Top-level sessions the source holds.
    pub found: usize,
    /// How many of the source's items are not imported, by reason.
    pub skipped: BTreeMap<SkipReason, usize>,
    /// Lines or rows inside sessions that could not be understood.
    pub malformed: usize,
    /// Imported sessions without a usable absolute folder: they are kept but
    /// belong to no project.
    pub without_folder: usize,
    /// The latest update time of any session in the source.
    pub newest: Option<DateTime<Utc>>,
    /// Things that make the source unreadable, in words; no content.
    pub problems: Vec<String>,
}

impl Survey {
    /// An empty survey of an agent.
    pub fn new(agent: AgentId) -> Self {
        Self {
            agent,
            places: Vec::new(),
            found: 0,
            skipped: BTreeMap::new(),
            malformed: 0,
            without_folder: 0,
            newest: None,
            problems: Vec::new(),
        }
    }

    /// Counts one more skipped item.
    pub fn skip(&mut self, reason: SkipReason) {
        *self.skipped.entry(reason).or_insert(0) += 1;
    }

    /// Notes a session's update time.
    pub fn see_time(&mut self, at: DateTime<Utc>) {
        self.newest = Some(self.newest.map_or(at, |newest| newest.max(at)));
    }

    /// Whether `cwd` is a folder the tree can place a session under: not
    /// empty and absolute in either spelling (`/x` or `C:\x`).
    pub fn is_usable_folder(cwd: &str) -> bool {
        cwd.starts_with('/')
            || cwd.starts_with("\\\\")
            || cwd.len() >= 3
                && cwd.as_bytes()[0].is_ascii_alphabetic()
                && cwd.as_bytes()[1] == b':'
                && matches!(cwd.as_bytes()[2], b'\\' | b'/')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_usable_when_absolute_in_any_spelling() {
        for good in [
            "/srv/api",
            "C:\\code\\api",
            "c:/code/api",
            "\\\\host\\share\\x",
        ] {
            assert!(Survey::is_usable_folder(good), "{good}");
        }
        for bad in ["", "api", "./api", "C:", "~/api"] {
            assert!(!Survey::is_usable_folder(bad), "{bad}");
        }
    }

    #[test]
    fn skips_are_counted_per_reason() {
        let mut survey = Survey::new(AgentId::OPENCODE);
        survey.skip(SkipReason::Empty);
        survey.skip(SkipReason::Empty);
        survey.skip(SkipReason::ChildSession);
        assert_eq!(survey.skipped[&SkipReason::Empty], 2);
        assert_eq!(survey.skipped[&SkipReason::ChildSession], 1);
    }
}

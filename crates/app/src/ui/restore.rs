//! Restoring what was open: the pure part.
//!
//! The window writes a [`SavedState`] after every change that matters and
//! reads it back at the next start. This file holds what needs no window:
//! turning a pane layout into its stored form and back, the rows the
//! "Restore N sessions from last time?" question lists, and the reasons a
//! terminal cannot be reopened. The glue that starts the terminals is in
//! `restore_view.rs`.

use super::live::LiveId;
use super::panes::{Axis, Layout};
use leon_core::{AgentId, SavedLayout, SavedState, SavedTerminal};
use std::collections::HashMap;

/// A layout as stored. Ratios and the shape of the splits are kept.
pub fn to_saved(layout: &Layout) -> SavedLayout {
    match layout {
        Layout::Leaf(id) => SavedLayout::Leaf(id.0),
        Layout::Split { axis, ratio, a, b } => SavedLayout::Split {
            axis: match axis {
                Axis::Row => "row",
                Axis::Column => "column",
            }
            .to_owned(),
            ratio: *ratio,
            a: Box::new(to_saved(a)),
            b: Box::new(to_saved(b)),
        },
    }
}

/// A stored layout with its terminals mapped to the live ones that were
/// started for them. A terminal that was not started leaves its pane: the
/// sibling takes the room. `None` when no pane is left.
pub fn from_saved(saved: &SavedLayout, started: &HashMap<u64, LiveId>) -> Option<Layout> {
    match saved {
        SavedLayout::Leaf(id) => started.get(id).copied().map(Layout::Leaf),
        SavedLayout::Split { axis, ratio, a, b } => {
            let a = from_saved(a, started);
            let b = from_saved(b, started);
            match (a, b) {
                (Some(a), Some(b)) => Some(Layout::Split {
                    axis: if axis == "column" {
                        Axis::Column
                    } else {
                        Axis::Row
                    },
                    ratio: ratio.clamp(0.05, 0.95),
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            }
        }
    }
}

/// One line of the restore question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreRow {
    /// The saved terminal's id.
    pub id: u64,
    /// The agent's name, or `Shell`.
    pub what: String,
    /// What it was called: the user's name, else its last title.
    pub title: String,
    /// The folder's last component.
    pub folder: String,
    /// How long ago it was started, as `format::age` writes it.
    pub ago: String,
}

/// The rows of the question, in the order the terminals were opened.
pub fn rows(state: &SavedState, now: chrono::DateTime<chrono::Utc>) -> Vec<RestoreRow> {
    state
        .terminals
        .iter()
        .map(|t| RestoreRow {
            id: t.id,
            what: agent_of(t).map_or_else(
                || "Shell".to_owned(),
                |agent| crate::format::agent_name(agent).to_owned(),
            ),
            title: t
                .name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .or_else(|| t.title.clone())
                .unwrap_or_default(),
            folder: std::path::Path::new(&t.cwd)
                .file_name()
                .map_or_else(|| t.cwd.clone(), |n| n.to_string_lossy().into_owned()),
            ago: chrono::DateTime::from_timestamp_millis(t.started_at)
                .map_or_else(String::new, |at| crate::format::age(now, at)),
        })
        .collect()
}

/// The agent a saved terminal ran, when it is still in the catalogue.
pub fn agent_of(terminal: &SavedTerminal) -> Option<AgentId> {
    terminal.agent.as_deref().and_then(AgentId::parse)
}

/// The sentence of the question.
pub fn question(count: usize, unclean: bool) -> String {
    let what = if count == 1 {
        "Restore 1 session from last time?".to_owned()
    } else {
        format!("Restore {count} sessions from last time?")
    };
    if unclean {
        format!("Leon did not close normally. {what}")
    } else {
        what
    }
}

/// The note on a paused row.
pub const PAUSED_NOTE: &str = "paused · press Enter to resume";

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> SavedLayout {
        SavedLayout::Split {
            axis: "column".into(),
            ratio: 0.25,
            a: Box::new(SavedLayout::Leaf(1)),
            b: Box::new(SavedLayout::Split {
                axis: "row".into(),
                ratio: 0.7,
                a: Box::new(SavedLayout::Leaf(2)),
                b: Box::new(SavedLayout::Leaf(3)),
            }),
        }
    }

    #[test]
    fn a_layout_keeps_its_shape_and_ratios_through_the_stored_form() {
        let started: HashMap<u64, LiveId> =
            [(1, LiveId(10)), (2, LiveId(20)), (3, LiveId(30))].into();
        let layout = from_saved(&saved(), &started).unwrap();
        assert_eq!(layout.leaves(), [LiveId(10), LiveId(20), LiveId(30)]);
        let back = to_saved(&layout);
        let expected = SavedLayout::Split {
            axis: "column".into(),
            ratio: 0.25,
            a: Box::new(SavedLayout::Leaf(10)),
            b: Box::new(SavedLayout::Split {
                axis: "row".into(),
                ratio: 0.7,
                a: Box::new(SavedLayout::Leaf(20)),
                b: Box::new(SavedLayout::Leaf(30)),
            }),
        };
        assert_eq!(back, expected);
    }

    #[test]
    fn a_terminal_that_could_not_be_started_leaves_its_pane_to_its_sibling() {
        let started: HashMap<u64, LiveId> = [(1, LiveId(10)), (3, LiveId(30))].into();
        let layout = from_saved(&saved(), &started).unwrap();
        assert_eq!(layout.leaves(), [LiveId(10), LiveId(30)]);
        assert_eq!(from_saved(&saved(), &HashMap::new()), None);
    }

    #[test]
    fn the_question_names_the_count_and_an_unclean_end() {
        assert_eq!(question(1, false), "Restore 1 session from last time?");
        assert_eq!(
            question(3, true),
            "Leon did not close normally. Restore 3 sessions from last time?"
        );
    }

    #[test]
    fn the_rows_say_what_it_was_where_and_how_long_ago() {
        let state = SavedState {
            terminals: vec![
                SavedTerminal {
                    id: 1,
                    machine: "local".into(),
                    cwd: "/srv/api".into(),
                    agent: Some("claude".into()),
                    session: Some("s".into()),
                    confidence: None,
                    history: None,
                    name: Some("my fix".into()),
                    title: Some("ignored".into()),
                    started_at: 0,
                    account: None,
                    keeper: None,
                },
                SavedTerminal {
                    id: 2,
                    machine: "local".into(),
                    cwd: "/srv/web".into(),
                    agent: None,
                    session: None,
                    confidence: None,
                    history: None,
                    name: None,
                    title: None,
                    started_at: 0,
                    account: None,
                    keeper: None,
                },
            ],
            ..Default::default()
        };
        let now = chrono::DateTime::from_timestamp(7_200, 0).unwrap();
        let rows = rows(&state, now);
        assert_eq!(rows[0].what, "Claude Code");
        assert_eq!(rows[0].title, "my fix");
        assert_eq!(rows[0].folder, "api");
        assert_eq!(rows[0].ago, "2h");
        assert_eq!(rows[1].what, "Shell");
    }
}

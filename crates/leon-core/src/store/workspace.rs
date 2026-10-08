//! The open terminals, remembered so the next start can offer them back.
//!
//! The window keeps its terminals only for as long as it runs. This module
//! stores what is needed to open them again: for every terminal its machine,
//! folder, agent and the agent's own session id (so it can be resumed), the
//! name the user gave it and, for a durable session, the terminal the keeper
//! holds for it; the tabs with their pane layout and ratios; which
//! tab and pane had the focus; the sidebar selection; and whether the last
//! run ended normally.
//!
//! It is written whole after every change that matters (the caller debounces)
//! and is small: one row per terminal, tab and workspace. No scrollback and
//! no transcript is stored here.
//!
//! There are two slots. `Current` is what the running window keeps up to
//! date; at start the previous run's `Current` is copied to `Previous` (when
//! it holds terminals), so "Restore last sessions" still works after the
//! person declined, or after something new was opened over `Current`.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::Store;
use crate::error::Result;

/// Which copy of the remembered workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// What the running window writes.
    Current,
    /// What the previous run left, kept until the next one replaces it.
    Previous,
}

impl Slot {
    fn column(self) -> i64 {
        match self {
            Slot::Current => 0,
            Slot::Previous => 1,
        }
    }
}

/// A pane layout, as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SavedLayout {
    /// One terminal, by its saved id.
    Leaf(u64),
    /// Two layouts sharing an area.
    Split {
        /// `row` (side by side) or `column` (stacked).
        axis: String,
        /// The share of the first, between 0 and 1.
        ratio: f32,
        /// The first layout.
        a: Box<SavedLayout>,
        /// The second layout.
        b: Box<SavedLayout>,
    },
}

impl SavedLayout {
    /// The saved terminal ids, in reading order.
    pub fn leaves(&self) -> Vec<u64> {
        match self {
            SavedLayout::Leaf(id) => vec![*id],
            SavedLayout::Split { a, b, .. } => {
                let mut all = a.leaves();
                all.extend(b.leaves());
                all
            }
        }
    }
}

/// One tab of a workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedTab {
    /// Its panes.
    pub layout: SavedLayout,
    /// The pane that had the keyboard.
    pub focus: u64,
    /// Whether that pane filled the tab.
    pub zoomed: bool,
}

/// The tabs of one folder of one machine.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedWorkspace {
    /// The machine and folder it belongs to (the window's own key).
    pub key: String,
    /// The tab that was shown.
    pub active: usize,
    /// The tabs, in order.
    pub tabs: Vec<SavedTab>,
}

/// The terminal the keeper of durable local sessions holds for a saved
/// terminal: its number there, and the token it was opened under (the
/// keeper's label for it). Both must match what the keeper lists now, so a
/// number reused by another keeper never attaches to the wrong terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeeperRef {
    /// The keeper's number for the terminal.
    pub pty: u64,
    /// The token the terminal was opened under.
    pub token: String,
}

/// One terminal, with what it takes to open it again.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedTerminal {
    /// Its id within this saved workspace.
    pub id: u64,
    /// The machine it ran on.
    pub machine: String,
    /// The folder it ran in.
    pub cwd: String,
    /// The agent's catalogue id, or `None` for a plain shell.
    pub agent: Option<String>,
    /// The agent's own session id, when it is known.
    pub session: Option<String>,
    /// How sure that id is: `resumed`, `state-file` or `newest-in-folder`.
    pub confidence: Option<String>,
    /// The history session row it was started from or linked to.
    pub history: Option<String>,
    /// The name the user gave it.
    pub name: Option<String>,
    /// The title the program last set; shown while restoring.
    pub title: Option<String>,
    /// When it was started, in milliseconds since the epoch.
    pub started_at: i64,
    /// The id of the account it ran with; `None` is the agent's own setup.
    pub account: Option<String>,
    /// The terminal the keeper holds for it, when it is a durable session;
    /// `None` for one that lives in the application.
    pub keeper: Option<KeeperRef>,
}

/// Everything remembered about the open terminals.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SavedState {
    /// When it was written, in milliseconds since the epoch.
    pub saved_at: i64,
    /// Whether the run that wrote it ended normally.
    pub clean_shutdown: bool,
    /// The sidebar selection, as the window writes it.
    pub selection: Option<String>,
    /// The terminal that was on screen.
    pub main: Option<u64>,
    /// The workspaces, in the order they were opened.
    pub workspaces: Vec<SavedWorkspace>,
    /// Every terminal.
    pub terminals: Vec<SavedTerminal>,
}

impl SavedState {
    /// Whether nothing is remembered.
    pub fn is_empty(&self) -> bool {
        self.terminals.is_empty()
    }
}

impl Store {
    /// Replaces the remembered workspace of `slot`. The clean-shutdown flag
    /// is stored as given. Announces nothing: nothing displays it.
    pub fn save_workspace(&self, slot: Slot, state: &SavedState) -> Result<()> {
        self.transact(|tx| {
            let s = slot.column();
            for table in [
                "saved_terminal",
                "saved_tab",
                "saved_workspace",
                "saved_meta",
            ] {
                tx.execute(&format!("DELETE FROM {table} WHERE slot = ?1"), [s])?;
            }
            tx.execute(
                "INSERT INTO saved_meta (slot, saved_at, clean, selection, main)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    s,
                    state.saved_at,
                    state.clean_shutdown,
                    state.selection,
                    state.main.map(|id| id as i64)
                ],
            )?;
            for (position, workspace) in state.workspaces.iter().enumerate() {
                tx.execute(
                    "INSERT INTO saved_workspace (slot, position, key, active)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![s, position as i64, workspace.key, workspace.active as i64],
                )?;
                for (tab_position, tab) in workspace.tabs.iter().enumerate() {
                    tx.execute(
                        "INSERT INTO saved_tab (slot, workspace, position, layout, focus, zoomed)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            s,
                            position as i64,
                            tab_position as i64,
                            serde_json::to_string(&tab.layout)
                                .map_err(|e| crate::error::StoreError::Invalid(e.to_string()))?,
                            tab.focus as i64,
                            tab.zoomed
                        ],
                    )?;
                }
            }
            for (position, t) in state.terminals.iter().enumerate() {
                tx.execute(
                    "INSERT INTO saved_terminal (slot, position, id, machine, cwd, agent, session,
                         confidence, history, name, title, started_at, account,
                         keeper_pty, keeper_token)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        s,
                        position as i64,
                        t.id as i64,
                        t.machine,
                        t.cwd,
                        t.agent,
                        t.session,
                        t.confidence,
                        t.history,
                        t.name,
                        t.title,
                        t.started_at,
                        t.account,
                        t.keeper.as_ref().map(|k| k.pty as i64),
                        t.keeper.as_ref().map(|k| k.token.as_str())
                    ],
                )?;
            }
            Ok(())
        })
    }

    /// The remembered workspace of `slot`, or `None` when none was written.
    pub fn load_workspace(&self, slot: Slot) -> Result<Option<SavedState>> {
        self.read(|connection| {
            let s = slot.column();
            let meta = connection
                .query_row(
                    "SELECT saved_at, clean, selection, main FROM saved_meta WHERE slot = ?1",
                    [s],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, bool>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                        ))
                    },
                )
                .optional()?;
            let Some((saved_at, clean_shutdown, selection, main)) = meta else {
                return Ok(None);
            };
            let mut workspaces = Vec::new();
            let mut statement = connection.prepare(
                "SELECT position, key, active FROM saved_workspace WHERE slot = ?1 ORDER BY position",
            )?;
            let rows: Vec<(i64, String, i64)> = statement
                .query_map([s], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (position, key, active) in rows {
                let mut tabs = Vec::new();
                let mut statement = connection.prepare(
                    "SELECT layout, focus, zoomed FROM saved_tab
                     WHERE slot = ?1 AND workspace = ?2 ORDER BY position",
                )?;
                let tab_rows: Vec<(String, i64, bool)> = statement
                    .query_map(params![s, position], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                for (layout, focus, zoomed) in tab_rows {
                    // A layout that no longer parses drops its tab, not the run.
                    if let Ok(layout) = serde_json::from_str(&layout) {
                        tabs.push(SavedTab {
                            layout,
                            focus: focus as u64,
                            zoomed,
                        });
                    }
                }
                workspaces.push(SavedWorkspace {
                    key,
                    active: active as usize,
                    tabs,
                });
            }
            let mut statement = connection.prepare(
                "SELECT id, machine, cwd, agent, session, confidence, history, name, title, started_at,
                        account, keeper_pty, keeper_token
                 FROM saved_terminal WHERE slot = ?1 ORDER BY position",
            )?;
            let terminals = statement
                .query_map([s], |row| {
                    Ok(SavedTerminal {
                        id: row.get::<_, i64>(0)? as u64,
                        machine: row.get(1)?,
                        cwd: row.get(2)?,
                        agent: row.get(3)?,
                        session: row.get(4)?,
                        confidence: row.get(5)?,
                        history: row.get(6)?,
                        name: row.get(7)?,
                        title: row.get(8)?,
                        started_at: row.get(9)?,
                        account: row.get(10)?,
                        keeper: match (row.get::<_, Option<i64>>(11)?, row.get(12)?) {
                            (Some(pty), Some(token)) => Some(KeeperRef {
                                pty: pty as u64,
                                token,
                            }),
                            _ => None,
                        },
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Some(SavedState {
                saved_at,
                clean_shutdown,
                selection,
                main: main.map(|id| id as u64),
                workspaces,
                terminals,
            }))
        })
    }

    /// Marks the run that wrote `Current` as ended normally (or not).
    pub fn set_clean_shutdown(&self, clean: bool) -> Result<()> {
        self.transact(|tx| {
            tx.execute("UPDATE saved_meta SET clean = ?1 WHERE slot = 0", [clean])?;
            Ok(())
        })
    }

    /// Keeps the previous run's workspace in the `Previous` slot when it
    /// held terminals, and returns it. The `Current` slot is left as it is:
    /// the new run overwrites it with its own state.
    pub fn archive_workspace(&self) -> Result<Option<SavedState>> {
        let current = self.load_workspace(Slot::Current)?;
        match current {
            Some(state) if !state.is_empty() => {
                self.save_workspace(Slot::Previous, &state)?;
                Ok(Some(state))
            }
            other => Ok(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal(id: u64) -> SavedTerminal {
        SavedTerminal {
            id,
            machine: "local".into(),
            cwd: "/srv/api".into(),
            agent: Some("claude".into()),
            session: Some(format!("session-{id}")),
            confidence: Some("resumed".into()),
            history: None,
            name: Some("my name".into()),
            title: None,
            started_at: 1_000 + id as i64,
            // One of the three runs with an account, which must come back.
            account: (id == 2).then(|| "claude-work".to_owned()),
            // One held by the keeper, which must come back with its token.
            keeper: (id == 3).then(|| KeeperRef {
                pty: 7_340_033,
                token: "k-3".to_owned(),
            }),
        }
    }

    fn state() -> SavedState {
        SavedState {
            saved_at: 5_000,
            clean_shutdown: false,
            selection: Some("worktree:x".into()),
            main: Some(2),
            workspaces: vec![SavedWorkspace {
                key: "local\u{1f}/srv/api".into(),
                active: 1,
                tabs: vec![
                    SavedTab {
                        layout: SavedLayout::Leaf(1),
                        focus: 1,
                        zoomed: false,
                    },
                    SavedTab {
                        layout: SavedLayout::Split {
                            axis: "row".into(),
                            ratio: 0.3,
                            a: Box::new(SavedLayout::Leaf(2)),
                            b: Box::new(SavedLayout::Leaf(3)),
                        },
                        focus: 3,
                        zoomed: true,
                    },
                ],
            }],
            terminals: vec![terminal(1), terminal(2), terminal(3)],
        }
    }

    #[test]
    fn the_workspace_survives_closing_and_reopening_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leon.db");
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.load_workspace(Slot::Current).unwrap(), None);
            store.save_workspace(Slot::Current, &state()).unwrap();
        }
        let again = Store::open(&path).unwrap();
        let loaded = again.load_workspace(Slot::Current).unwrap().unwrap();
        assert_eq!(loaded, state());
        // Ratios and focus are part of it.
        match &loaded.workspaces[0].tabs[1].layout {
            SavedLayout::Split { ratio, .. } => assert!((ratio - 0.3).abs() < 1e-6),
            other => panic!("{other:?}"),
        }
        assert_eq!(loaded.workspaces[0].tabs[1].focus, 3);
        assert_eq!(loaded.main, Some(2));
    }

    #[test]
    fn saving_again_replaces_and_the_clean_flag_can_be_set_apart() {
        let store = Store::open_in_memory().unwrap();
        store.save_workspace(Slot::Current, &state()).unwrap();
        let mut smaller = state();
        smaller.terminals.truncate(1);
        smaller.workspaces[0].tabs.truncate(1);
        store.save_workspace(Slot::Current, &smaller).unwrap();
        let loaded = store.load_workspace(Slot::Current).unwrap().unwrap();
        assert_eq!(loaded.terminals.len(), 1);
        assert!(!loaded.clean_shutdown);
        store.set_clean_shutdown(true).unwrap();
        assert!(
            store
                .load_workspace(Slot::Current)
                .unwrap()
                .unwrap()
                .clean_shutdown
        );
    }

    #[test]
    fn the_previous_run_is_kept_when_it_had_terminals_and_slots_are_independent() {
        let store = Store::open_in_memory().unwrap();
        store.save_workspace(Slot::Current, &state()).unwrap();
        let kept = store.archive_workspace().unwrap().unwrap();
        assert_eq!(kept.terminals.len(), 3);
        // The new run writes an empty workspace over Current; Previous stays.
        store
            .save_workspace(Slot::Current, &SavedState::default())
            .unwrap();
        assert!(store.archive_workspace().unwrap().unwrap().is_empty());
        assert_eq!(
            store
                .load_workspace(Slot::Previous)
                .unwrap()
                .unwrap()
                .terminals
                .len(),
            3
        );
    }

    #[test]
    fn leaves_read_left_to_right() {
        let SavedTab { layout, .. } = state().workspaces[0].tabs[1].clone();
        assert_eq!(layout.leaves(), [2, 3]);
    }
}

//! The user interface: a sidebar tree and the main pane on a hairline grid,
//! drawn in Zavu's design system (see `theme.rs`).
//!
//! ```text
//! ┌──────────────────┬──────────────────────────────┐
//! │     sidebar      │             main             │
//! └──────────────────┴──────────────────────────────┘
//! ```
//!
//! [`Shell`] is the root view: it owns all UI state and the key handling. The
//! other modules are rendering helpers for one pane or overlay each, plus the
//! pure parts (`model`, `tree`, `expansion`, `steps`) that decide what is shown
//! and asked. Everything
//! shown comes from the local store; the views never run a command.

mod activity;
mod connect;
mod connect_view;
mod expansion;
mod filter;
mod find;
mod history_view;
mod lines;
mod lion;
mod live;
mod logos;
mod main_pane;
mod menu;
mod model;
pub(crate) mod notify;
mod pair;
mod palette;
mod panes;
mod paste;
mod prefs;
mod projects;
mod settings_screen;
mod share;
mod sheet;
mod shell;
mod sidebar;
mod steps;
mod terminal_tools;
mod terminals;
mod themes;
mod tree;
mod updates_view;
mod usage_view;
mod widgets;
mod workspace;

pub use shell::{system_picker, Options, Picked, Shell};
pub use themes::FOLDER as THEMES_FOLDER;
pub use tree::DEFAULT as DEFAULT_SESSIONS_SHOWN;
pub use usage_view::agent_name as agent_display_name;
pub use usage_view::footer_text;

#[cfg(test)]
mod tests;

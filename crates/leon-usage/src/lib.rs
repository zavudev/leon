//! How much of each agent's usage limits is left, per machine.
//!
//! The model ([`model`]) is provider neutral: windows with a used percentage
//! and a reset time, or an explicit reason why nothing is known, with
//! staleness handled honestly. The parsers ([`codex`], [`claude`],
//! [`opencode`]) are pure functions of text and a clock. [`collect`] gathers
//! a machine's readings through a [`leon_remote::Runner`], one bounded command
//! per machine, and calls a vendor's usage endpoint only for a source that was
//! switched on, through the [`network::Http`] trait, with a credential that is
//! read at the moment of the call and never stored, logged or shown.
//!
//! Nothing here draws anything.

#![warn(missing_docs)]

pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod collect;
pub mod cursor;
pub mod forecast;
pub mod grok;
pub mod kimi;
pub mod model;
pub mod network;
pub mod opencode;
pub mod present;
pub mod secret;
pub mod view;
pub mod zcode;

#[cfg(test)]
mod providers_tests;

pub use collect::{collect_machine, collect_machine_on, series_key, MachineUsage};
pub use forecast::{forecast, Forecast, Sample};
pub use model::{
    AgentUsage, Collected, Effective, EffectiveWindow, Reason, Source, State, UsageWindow,
    WindowKind,
};
pub use present::{ago, compact_duration, percent_fixed, Level, Thresholds};
pub use view::{view, AgentView, Body, Meter};

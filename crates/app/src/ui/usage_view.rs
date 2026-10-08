//! The usage bar and the usage view: how much of each agent's limits is left.
//!
//! The *model* half is pure and tested without a window: [`bar_model`] says
//! what the bar shows for the machine in context (one figure per agent, its
//! logo and the window closest to its limit, the others on hover),
//! [`usage_rows`] says what the view lists, with the burn-rate estimate and a
//! small history line per window. The *drawing* half turns those into GPUI
//! elements with the theme's tokens only: a level is a colour *and* a marker
//! (`!` and `!!`), never a colour alone.
//!
//! The bar is the footer under the main pane (level with the sidebar's tools,
//! so the two are one footer across the window, and the whole width when the
//! sidebar is hidden). It holds the engine's status line, so messages stay
//! visible, next to the meters; the view is an overlay opened by the bar, a
//! chord or the palette.

use std::collections::HashMap;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, FontWeight, Hsla, SharedString, Stateful, Task, Window};
use leon_core::{AgentId, MachineId, TokenRow, UsagePoint};
use leon_usage::{
    countdown, forecast, series_key_of, view, AgentUsage, AgentView, Body, Level, Meter,
    PercentDisplay, Period, PriceTable, Sample, Thresholds,
};

use super::shell::{Overlay, Shell};
use super::widgets::{key_cap, mono, section_label};
use crate::agent_usage::{failure_note, unknown_text, Board, Scope, Statuses};
use crate::engine::SourceStatus;
use crate::engine::{Op, StatusKind};
use crate::icons::{agent_icon, icon, IconName};
use crate::keys::{self, Command};
use crate::schema::Section;
use crate::settings;
use crate::theme::{metrics, px, Palette};

/// How many points of history a sparkline draws.
pub const SPARK_POINTS: usize = 24;

/// What is left of the bar for the meters: the window's width minus its
/// padding, the least the status line may have and the refresh affordance.
const RESERVED: f32 = 24.0 + 160.0 + 120.0;

/// What the bar's text takes per character, and an item's logo, padding and
/// gap, in pixels.
const CHAR: f32 = 7.0;
const ITEM: f32 = 13.0 + 6.0 + 8.0 + 12.0;

/// The room the bar's meters have at this window width, in pixels.
fn room(width: f32) -> f32 {
    (width - RESERVED).max(0.0)
}
/// One agent in the bar.
#[derive(Clone, Debug, PartialEq)]
pub struct BarItem {
    /// The agent.
    pub agent: AgentId,
    /// The machine's id.
    pub machine: String,
    /// The id of the account this is the line of; `None` is the agent's own
    /// setup.
    pub account: Option<String>,
    /// The window shown: `5h`, `wk`, or the reason when nothing is known.
    pub label: String,
    /// The percentage as a fixed-width figure, empty when unknown.
    pub figure: String,
    /// Whether the figure is what is left, not what is used.
    pub left: bool,
    /// How old the numbers are, in seconds.
    pub age: Option<i64>,
    /// When the numbers are kept after a failed read: how old they are.
    pub stale: Option<String>,
    /// The marker of the level: empty, `!` or `!!`.
    pub glyph: &'static str,
    /// How close to its limit.
    pub level: Level,
    /// The used percentage, 0 when unknown.
    pub percent: f32,
    /// Whether numbers are known.
    pub known: bool,
    /// What hovering says: every window, the source and how fresh.
    pub tip: String,
    /// Why nothing is known, when nothing is.
    pub reason: Option<leon_usage::Reason>,
}

impl BarItem {
    /// What the chip says of a known agent: ` 91% !!`, or `  9% left !!`.
    pub fn figure_text(&self) -> String {
        format!(
            "{}{}{}",
            self.figure,
            if self.left { " left" } else { "" },
            glyph_tail(self.glyph)
        )
    }

    /// What this agent takes of the bar, in pixels.
    fn cost(&self) -> f32 {
        let chars = if self.known {
            self.figure_text().chars().count()
        } else {
            self.label.chars().count()
        };
        ITEM + chars as f32 * CHAR
    }
}

/// What the view says about the user's accounts of the agents: where their
/// limits can and cannot be read.
pub const ACCOUNTS_NOTE: &str = "Accounts: the limits of a Claude Code or Codex account are read from its own folder, on this computer only; other agents' accounts have no line of their own, and no number is borrowed from the agent's own.";

/// How many agents the bar lists one by one. With more, the ones with numbers
/// come first and the rest (signed out, switched off, no data yet) fold into
/// a count.
pub const BAR_PLAIN: usize = 4;

/// What the bar shows.
#[derive(Clone, Debug, PartialEq)]
pub struct BarModel {
    /// The agents shown one by one, in display order; an agent that is not
    /// installed is absent.
    pub items: Vec<BarItem>,
    /// The agents that did not fit, folded into a `+N` that lists them on
    /// hover.
    pub folded: Vec<BarItem>,
    /// When the numbers were last read, as `3 min ago`.
    pub updated: Option<String>,
}

/// The line of hover text of one meter.
fn meter_line(meter: &Meter, display: PercentDisplay) -> String {
    let tail = meter
        .reset_text()
        .map(|text| format!(" · {text}"))
        .unwrap_or_default();
    format!(
        "{}: {}{}{}",
        meter.kind.long(),
        display.label(meter.percent),
        if meter.level.glyph().is_empty() {
            String::new()
        } else {
            format!(" ({})", meter.level.word())
        },
        tail
    )
}

/// What the bar shows for the machine in context: one figure per agent, its
/// logo and the window closest to its limit, the rest on hover. With many
/// agents the ones that have no numbers fold into a count, and when even one
/// figure per agent does not fit, the agent closest to its limit stays and the
/// rest fold too.
#[allow(clippy::too_many_arguments)]
pub fn bar_model(
    board: &Board,
    context: &MachineId,
    machine_name: &str,
    shown: &[AgentId],
    now: i64,
    thresholds: Thresholds,
    display: PercentDisplay,
    width: f32,
) -> BarModel {
    let mut items = Vec::new();
    for reading in board.select(&Scope::Context, context, shown) {
        let v = view(reading, now, thresholds);
        let name = leon_usage::heading(reading.agent, reading.account.as_deref());
        let item = match &v.body {
            Body::Unknown(leon_usage::Reason::NotInstalled) => continue,
            Body::Unknown(reason) => BarItem {
                agent: reading.agent,
                machine: reading.machine.clone(),
                account: reading.account.clone(),
                label: reason.short().to_owned(),
                figure: String::new(),
                left: false,
                age: None,
                stale: None,
                glyph: "",
                level: Level::Normal,
                percent: 0.0,
                known: false,
                tip: format!("{name} on {machine_name}\n{}", reason.text()),
                reason: Some(*reason),
            },
            Body::Ready { primary, meters } => {
                let mut tip = format!("{name} on {machine_name}");
                if let Some(plan) = &v.plan {
                    tip.push_str(&format!(" ({plan})"));
                }
                for meter in meters {
                    tip.push('\n');
                    tip.push_str(&meter_line(meter, display));
                }
                tip.push('\n');
                tip.push_str(&v.provenance());
                BarItem {
                    agent: reading.agent,
                    machine: reading.machine.clone(),
                    account: reading.account.clone(),
                    label: primary.kind.short(),
                    figure: display.fixed(primary.percent),
                    left: display == PercentDisplay::Remaining,
                    age: v.age,
                    stale: None,
                    glyph: primary.level.glyph(),
                    level: primary.level,
                    percent: primary.percent as f32,
                    known: true,
                    tip,
                    reason: None,
                }
            }
        };
        items.push(item);
    }
    // With many agents, the ones with numbers come first and the rest (signed
    // out, switched off, no data yet) fold into a count.
    let mut folded = Vec::new();
    if items.len() > BAR_PLAIN {
        let (known, rest): (Vec<BarItem>, Vec<BarItem>) =
            items.into_iter().partition(|item| item.known);
        items = known;
        folded = rest;
    }
    // One figure per agent is small. When even that does not fit, the agent
    // closest to its limit stays on show and the others fold into the count.
    if items.iter().map(BarItem::cost).sum::<f32>() > room(width) {
        let worst = items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.known)
            .max_by(|a, b| a.1.percent.total_cmp(&b.1.percent))
            .map(|(index, _)| index);
        let keep = worst.map(|index| items.remove(index));
        folded = items.drain(..).chain(folded).collect();
        items = keep.into_iter().collect();
    }
    BarModel {
        folded,
        items,
        updated: board
            .collected_at()
            .map(|at| leon_usage::ago((now - at).max(0))),
    }
}

/// What the bar says of one reading, as `--diagnose usage` prints it: the
/// figure of the window closest to its limit with its marker (`91% !!`).
/// `None` when nothing is known.
pub fn footer_text(
    reading: &AgentUsage,
    now: i64,
    thresholds: Thresholds,
    display: PercentDisplay,
) -> Option<String> {
    let board = Board::new_for(vec![reading.clone()]);
    let machine = MachineId::from_string(reading.machine.clone());
    let model = bar_model(
        &board,
        &machine,
        "",
        &[reading.agent],
        now,
        thresholds,
        display,
        f32::MAX,
    );
    model
        .items
        .first()
        .filter(|item| item.known)
        .map(|item| item.figure_text().trim_start().to_owned())
}

/// What an agent is called.
pub fn agent_name(agent: AgentId) -> &'static str {
    agent.name()
}

/// One line of the tooltip of the folded `+N`: what the item would have shown,
/// with the word for the figure so a hover needs no colour to be read.
fn folded_line(item: &BarItem) -> String {
    if item.known {
        format!(
            "{}{}{}",
            item.figure.trim_start(),
            if item.left { " left" } else { " used" },
            glyph_tail(item.glyph)
        )
    } else {
        item.label.clone()
    }
}

/// The colour of a level: tokens of the theme only.
pub fn level_colour(level: Level, colours: &Palette) -> Hsla {
    match level {
        Level::Normal => colours.text_muted,
        Level::Warning => colours.warning,
        Level::Critical => colours.error,
    }
}

/// The history key of one window.
pub type HistoryKey = (String, AgentId, String);

/// One window of a row of the view.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowRow {
    /// The meter.
    pub meter: Meter,
    /// The burn-rate estimate, worded as an estimate, when there is one.
    pub forecast: Option<String>,
    /// The history, oldest first, each 0 to 1.
    pub spark: Vec<f32>,
}

/// One agent on one machine in the view.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageRow {
    /// The reading, ready to draw.
    pub view: AgentView,
    /// The machine's name.
    pub machine_name: String,
    /// Its windows.
    pub windows: Vec<WindowRow>,
    /// What to say instead of the reason's sentence, or besides the numbers:
    /// a read under way, why the last one failed and when the next is.
    pub note: Option<String>,
    /// The setting that turns this agent's network source on, when it is the
    /// only thing missing.
    pub turn_on: Option<&'static str>,
    /// Whether choosing "Try again" may help.
    pub try_again: bool,
}

impl UsageRow {
    /// Whether the agent has nothing to show here: it is not installed (or
    /// has no usage source on this machine), or no limit was ever recorded.
    pub fn is_inactive(&self) -> bool {
        matches!(
            self.view.body,
            Body::Unknown(
                leon_usage::Reason::NotInstalled
                    | leon_usage::Reason::NotSupported
                    | leon_usage::Reason::NoData
            )
        )
    }
}

/// What the view lists for a scope.
#[allow(clippy::too_many_arguments)]
pub fn usage_rows(
    board: &Board,
    scope: &Scope,
    context: &MachineId,
    shown: &[AgentId],
    names: &dyn Fn(&str) -> String,
    now: i64,
    thresholds: Thresholds,
    history: &HashMap<HistoryKey, Vec<UsagePoint>>,
) -> Vec<UsageRow> {
    let mut rows: Vec<UsageRow> = board
        .select(scope, context, shown)
        .into_iter()
        .map(|reading: &AgentUsage| {
            let v = view(reading, now, thresholds);
            let windows = match &v.body {
                Body::Ready { meters, .. } => meters
                    .iter()
                    .map(|meter| {
                        let points = history
                            .get(&(reading.slot(), reading.agent, meter.kind.key()))
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        window_row(meter, points, now)
                    })
                    .collect(),
                Body::Unknown(_) => Vec::new(),
            };
            UsageRow {
                machine_name: names(&reading.machine),
                view: v,
                windows,
                note: None,
                turn_on: None,
                try_again: false,
            }
        })
        .collect();
    // Worst first, as Orca's roster does: the agent nearest a limit on top,
    // those with nothing known after the ones with numbers. The sort is stable,
    // so equal agents keep the catalogue's order.
    rows.sort_by(|a, b| worst_percent(b).total_cmp(&worst_percent(a)));
    rows
}

/// The used percentage of the window closest to its limit; below any real
/// figure when nothing is known.
fn worst_percent(row: &UsageRow) -> f64 {
    match &row.view.body {
        Body::Ready { primary, .. } => primary.percent,
        Body::Unknown(_) => -1.0,
    }
}

/// What the engine and the clock add to a reading: see [`annotate_rows`].
pub struct Note<'a> {
    /// What the engine knows of each agent's last read.
    pub statuses: &'a Statuses,
    /// The time, in Unix seconds.
    pub now: i64,
    /// When the schedule reads next.
    pub next_read: Option<i64>,
    /// Whether this is macOS (the keychain's prompt).
    pub mac: bool,
    /// The id of this computer: only its readings have a network source.
    pub local: &'a str,
}

impl Note<'_> {
    fn status(&self, agent: AgentId) -> SourceStatus {
        self.statuses.get(&agent).copied().unwrap_or_default()
    }
}

/// The setting that switches an agent's network source.
fn source_setting(agent: AgentId) -> Option<&'static str> {
    crate::schema::find(&crate::schema::usage_setting(agent, true)).map(|def| def.key)
}

/// Says what the last read of each network source came to: "Reading…" while
/// one is under way (not what the last one said), the reason and the time of
/// the next read after a failure, the setting that turns a source on.
pub fn annotate_rows(rows: &mut [UsageRow], note: &Note<'_>) {
    for row in rows {
        if row.view.machine != note.local {
            continue;
        }
        let agent = row.view.agent;
        let status = note.status(agent);
        match &row.view.body {
            Body::Unknown(reason) => {
                let reason = *reason;
                if matches!(
                    reason,
                    leon_usage::Reason::NotInstalled | leon_usage::Reason::NotSupported
                ) {
                    continue;
                }
                if status.reading {
                    row.note = Some(unknown_text(
                        agent,
                        reason,
                        &status,
                        note.now,
                        note.next_read,
                        note.mac,
                    ));
                } else if reason == leon_usage::Reason::SourceDisabled {
                    row.turn_on = source_setting(agent);
                } else if reason.is_failure()
                    || status.refused
                    || (reason == leon_usage::Reason::NotSignedIn
                        && crate::agent_usage::says_signed_out(agent))
                {
                    row.note = Some(unknown_text(
                        agent,
                        reason,
                        &status,
                        note.now,
                        note.next_read,
                        note.mac,
                    ));
                    row.try_again = true;
                }
            }
            Body::Ready { .. } => {
                if !status.reading {
                    row.note = failure_note(agent, &status, note.now, note.next_read, note.mac);
                    row.try_again = row.note.is_some();
                }
            }
        }
    }
}

/// The same for the footer's meters: an agent with nothing known says
/// "reading…" or its failure.
pub fn annotate_bar(model: &mut BarModel, note: &Note<'_>) {
    for item in &mut model.items {
        if item.machine != note.local {
            continue;
        }
        if item.known {
            // Numbers kept after a failed read stay on show, marked with their
            // age, and the tip says why they are not newer.
            let status = note.status(item.agent);
            if status.failed.is_some() && !status.reading {
                if let Some(age) = item.age {
                    item.stale = Some(leon_usage::ago(age));
                }
                if let Some(text) =
                    failure_note(item.agent, &status, note.now, note.next_read, note.mac)
                {
                    item.tip = format!("{}\n{text}", item.tip);
                }
            }
            continue;
        }
        let Some(reason) = item.reason else { continue };
        let status = note.status(item.agent);
        let text = if status.reading
            || status.refused
            || reason.is_failure()
            || (reason == leon_usage::Reason::NotSignedIn
                && crate::agent_usage::says_signed_out(item.agent))
        {
            unknown_text(
                item.agent,
                reason,
                &status,
                note.now,
                note.next_read,
                note.mac,
            )
        } else {
            continue;
        };
        if status.reading {
            item.label = "reading…".to_owned();
        } else if status.refused {
            item.label = leon_usage::Reason::KeychainDenied.short().to_owned();
        }
        item.tip = format!(
            "{}\n{text}",
            leon_usage::heading(item.agent, item.account.as_deref())
        );
    }
}

fn window_row(meter: &Meter, points: &[UsagePoint], now: i64) -> WindowRow {
    let forecast = (!meter.reset_since_seen)
        .then(|| {
            let samples: Vec<Sample> = points
                .iter()
                .map(|p| Sample {
                    at: p.at,
                    used_percent: p.used_percent,
                })
                .collect();
            forecast(
                &samples,
                meter.percent,
                now,
                meter.resets_in.map(|seconds| now + seconds),
            )
            .sentence()
        })
        .flatten();
    let start = points.len().saturating_sub(SPARK_POINTS);
    WindowRow {
        meter: meter.clone(),
        forecast,
        spark: points[start..]
            .iter()
            .map(|p| (p.used_percent / 100.0).clamp(0.0, 1.0) as f32)
            .collect(),
    }
}

/// Which scope comes after `current`, going through the context machine, the
/// other machines and every machine, and back.
pub fn step_scope(
    current: &Scope,
    machines: &[MachineId],
    context: &MachineId,
    delta: i32,
) -> Scope {
    let mut all = vec![Scope::Context];
    all.extend(
        machines
            .iter()
            .filter(|id| *id != context)
            .map(|id| Scope::Machine(id.clone())),
    );
    all.push(Scope::All);
    let at = all.iter().position(|s| s == current).unwrap_or(0) as i32;
    let next = (at + delta).rem_euclid(all.len() as i32) as usize;
    all.swap_remove(next)
}

/// The state of the view and of what feeds it.
pub struct UsageUi {
    /// The latest readings, read from the store.
    pub board: Board,
    /// Which machines the view lists.
    pub scope: Scope,
    /// Detailed or compact.
    pub detailed: bool,
    /// The history of each window, read when the view is open.
    pub history: HashMap<HistoryKey, Vec<UsagePoint>>,
    /// The timer that reads the limits again, while the window lives.
    pub ticker: Option<Task<()>>,
    /// When the last read was asked for (Unix seconds).
    pub last_request: Option<i64>,
    /// This install's jitter for the wait to the next one, in seconds.
    pub jitter: i64,
    /// When the schedule reads next, as far as it knows.
    pub next_at: Option<i64>,
    /// The settings the schedule was last given: which agents are shown and
    /// the interval, to tell what changed.
    pub applied: Option<(Vec<AgentId>, i64)>,
    /// The tokens counted from the transcripts in the period, read from the
    /// store while the view is open.
    pub tokens: Vec<TokenRow>,
    /// The span the tokens are summed over.
    pub period: Period,
    /// The prices the tokens are costed at.
    pub prices: PriceTable,
    /// What is wrong with the user's price file, when something is.
    pub price_notes: Vec<String>,
}

impl Default for UsageUi {
    fn default() -> Self {
        Self {
            board: Board::default(),
            scope: Scope::Context,
            detailed: true,
            history: HashMap::new(),
            ticker: None,
            last_request: None,
            jitter: 0,
            next_at: None,
            applied: None,
            tokens: Vec::new(),
            period: Period::default(),
            prices: PriceTable::bundled(),
            price_notes: Vec::new(),
        }
    }
}

impl Shell {
    // ----- state ----------------------------------------------------------------------------

    /// Reads the latest readings (and, while the view is open, their history)
    /// from the store.
    pub(super) fn usage_reload(&mut self) {
        self.usage.board = Board::load(self.engine.store());
        if self.overlay == Overlay::Usage {
            self.usage_load_history();
            self.usage_load_tokens();
        }
    }

    fn usage_load_history(&mut self) {
        let store = self.engine.store().clone();
        let since = (self.options.now)().timestamp() - crate::agent_usage::HISTORY_HORIZON;
        let mut history = HashMap::new();
        for reading in self.usage.board.all() {
            let leon_usage::State::Known { windows } = &reading.state else {
                continue;
            };
            let account = series_key_of(
                &reading.machine,
                reading.agent,
                reading.plan.as_deref(),
                reading.account.as_deref(),
            );
            for window in windows {
                let key = window.kind.key();
                if let Ok(points) = store.usage_history(
                    &MachineId::from_string(reading.machine.clone()),
                    reading.agent,
                    &account,
                    &key,
                    since,
                ) {
                    history.insert((reading.slot(), reading.agent, key), points);
                }
            }
        }
        self.usage.history = history;
    }

    pub(super) fn usage_now(&self) -> i64 {
        (self.options.now)().timestamp()
    }

    /// What the engine and the clock add to the readings.
    fn usage_note<'a>(&self, statuses: &'a Statuses) -> Note<'a> {
        Note {
            statuses,
            now: self.usage_now(),
            next_read: self.usage.next_at,
            mac: crate::platform::is_mac(),
            local: "local",
        }
    }

    /// What the bar shows now, for the machine in context.
    pub(super) fn usage_bar_model(&self, cx: &gpui_kit::App) -> BarModel {
        let context = self.current_machine();
        let name = self.machine_name_of(context.as_str());
        let statuses = crate::agent_usage::statuses(&self.engine);
        let mut model = bar_model(
            &self.usage.board,
            &context,
            &name,
            &settings::usage_agents(cx),
            self.usage_now(),
            settings::usage_thresholds(cx),
            settings::usage_percent_display(cx),
            self.strip_width(cx),
        );
        annotate_bar(&mut model, &self.usage_note(&statuses));
        model
    }

    /// The width of the footer strip: the window's, less the sidebar's and the
    /// file tree's.
    fn strip_width(&self, cx: &gpui_kit::App) -> f32 {
        let sidebar = if settings::get(cx).sidebar_visible {
            metrics::SIDEBAR_WIDTH().as_f32()
        } else {
            0.0
        };
        self.viewport.width.as_f32() - sidebar - metrics::FILES_WIDTH().as_f32()
    }

    pub(super) fn machine_name_of(&self, id: &str) -> String {
        self.snapshot
            .machines
            .iter()
            .find(|machine| machine.id.as_str() == id)
            .map(|machine| machine.name.clone())
            .unwrap_or_else(|| id.to_owned())
    }

    /// What the view lists now.
    pub(super) fn usage_view_rows(&self, cx: &gpui_kit::App) -> Vec<UsageRow> {
        let statuses = crate::agent_usage::statuses(&self.engine);
        let mut rows = usage_rows(
            &self.usage.board,
            &self.usage.scope,
            &self.current_machine(),
            &settings::usage_agents(cx),
            &|id| self.machine_name_of(id),
            self.usage_now(),
            settings::usage_thresholds(cx),
            &self.usage.history,
        );
        annotate_rows(&mut rows, &self.usage_note(&statuses));
        rows
    }

    /// The schedule: while the window is up, the limits are read every
    /// `usage_refresh_seconds` (with a little jitter), only while it is
    /// focused. It exists only when the engine collects at all, and the first
    /// read waits a second so that the window is on screen before anything,
    /// a keychain prompt included, can ask for attention.
    pub(super) fn watch_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.options.usage_timer || !self.engine.collects_usage() {
            return;
        }
        self.usage.ticker = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            loop {
                if this.update(cx, |this, cx| this.usage_tick(cx)).is_err() {
                    return;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(
                        crate::agent_usage::TICK_SECONDS,
                    ))
                    .await;
            }
        }));
    }

    /// One look at the clock: starts the reading the first time, then reads
    /// when [`crate::agent_usage::due`] says so. Also asked when the window
    /// comes back to the front.
    pub(super) fn usage_tick(&mut self, cx: &mut Context<Self>) {
        if !self.engine.collects_usage() {
            return;
        }
        if self.engine.start_usage() {
            self.usage_request(Op::CollectUsage, cx);
            return;
        }
        let interval = settings::usage_interval_seconds(cx);
        if crate::agent_usage::due(
            self.usage.last_request,
            self.usage_now(),
            interval,
            self.usage.jitter,
            self.window_active,
        ) {
            self.usage_request(Op::CollectUsage, cx);
        }
    }

    /// Asks the engine for a read and notes when, so that the next scheduled
    /// one is an interval (and a jitter) later.
    fn usage_request(&mut self, op: Op, cx: &mut Context<Self>) {
        let now = self.usage_now();
        let interval = settings::usage_interval_seconds(cx);
        let entropy = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        self.usage.jitter = crate::agent_usage::jitter_seconds(interval, entropy);
        self.usage.last_request = Some(now);
        self.usage.next_at = Some(now + interval + self.usage.jitter);
        self.engine.submit(op);
    }

    /// What a changed setting asks of the readings: a network source switched
    /// on or off, an agent shown that was hidden, another interval. Each reads
    /// at once for the agent concerned, not at the next tick.
    pub(super) fn usage_settings_changed(
        &mut self,
        before: leon_usage::network::NetworkPolicy,
        cx: &mut Context<Self>,
    ) {
        let policy = settings::usage_policy(cx);
        let shown = settings::usage_agents(cx);
        let interval = settings::usage_interval_seconds(cx);
        let previous = self.usage.applied.replace((shown.clone(), interval));
        let mut affected: Vec<AgentId> = Vec::new();
        for agent in leon_usage::network::switchable_agents() {
            if before.allows(agent) != policy.allows(agent) {
                affected.push(agent);
            }
        }
        let mut changed = !affected.is_empty();
        if let Some((was_shown, was_interval)) = previous {
            for agent in &shown {
                if !was_shown.contains(agent) {
                    changed = true;
                    if !affected.contains(agent) {
                        affected.push(*agent);
                    }
                }
            }
            changed |= was_interval != interval;
        }
        if changed && self.engine.collects_usage() {
            self.usage_request(Op::CollectUsageNow(affected), cx);
        }
    }

    // ----- commands -------------------------------------------------------------------------

    /// Opens the usage view, or closes it when it is open.
    pub(super) fn toggle_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay == Overlay::Usage {
            self.close_overlay(window, cx);
            return;
        }
        self.close_overlay(window, cx);
        self.overlay = Overlay::Usage;
        self.usage.scope = Scope::Context;
        self.usage_load_prices(cx);
        self.usage_reload();
        self.usage_read_if_stale(cx);
        self.focus.focus(window, cx);
    }

    /// Opening the view reads once when the last read is older than the
    /// interval, as coming back to the window does; a fresh reading is shown
    /// as it is.
    fn usage_read_if_stale(&mut self, cx: &mut Context<Self>) {
        if !self.engine.collects_usage() || self.engine.usage_collecting() {
            return;
        }
        let interval = settings::usage_interval_seconds(cx);
        let stale = self
            .usage
            .board
            .collected_at()
            .is_none_or(|at| self.usage_now() - at >= interval);
        if stale
            && crate::agent_usage::due(self.usage.last_request, self.usage_now(), interval, 0, true)
        {
            self.usage_request(Op::CollectUsage, cx);
        }
    }

    /// Reads the limits again now.
    pub(super) fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        self.usage_load_prices(cx);
        if self.engine.collects_usage() {
            // The setting in force now, even if the window has not drawn since
            // it changed; a source that backed off or was refused is asked
            // again.
            self.engine.set_usage_policy(settings::usage_policy(cx));
            self.usage_request(Op::CollectUsageNow(Vec::new()), cx);
        } else {
            self.engine
                .report(StatusKind::Info, "Usage limits are not read in this run.");
        }
        cx.notify();
    }

    /// The keys of the view. `true` when the key was taken.
    pub(super) fn usage_key(
        &mut self,
        stroke: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &stroke.modifiers;
        if m.platform || m.control || m.alt || m.shift || m.function {
            return false;
        }
        match stroke.key.as_str() {
            "r" => self.refresh_usage(cx),
            "m" | "d" | "c" => self.usage.detailed = !self.usage.detailed,
            "t" => {
                self.usage.period = self.usage.period.next();
                self.usage_load_tokens();
            }
            "left" => self.usage_step_scope(-1),
            "right" => self.usage_step_scope(1),
            "s" => {
                self.open_settings(window, cx);
                self.settings_pick_section(Section::Usage, cx);
            }
            "o" => {
                let target = self.usage_view_rows(cx).iter().find_map(|row| row.turn_on);
                match target {
                    Some(key) => self.settings_goto(key, window, cx),
                    None => return false,
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn usage_step_scope(&mut self, delta: i32) {
        let machines: Vec<MachineId> = self
            .snapshot
            .machines
            .iter()
            .map(|machine| machine.id.clone())
            .collect();
        self.usage.scope = step_scope(&self.usage.scope, &machines, &self.current_machine(), delta);
    }

    // ----- the bar --------------------------------------------------------------------------

    /// The footer under the main pane, level with the sidebar's tools so that
    /// the two read as one footer across the window: the engine's status line
    /// and, when the setting is on, each agent's limits and a refresh button.
    pub(super) fn render_status_bar(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let status = self.engine.status();
        let (text, colour) = match &status {
            Some(line) if line.kind == StatusKind::Error => (line.text.clone(), colours.error),
            Some(line) if line.kind == StatusKind::Busy => (line.text.clone(), colours.info),
            Some(line) => (line.text.clone(), colours.text_muted),
            None => ("Ready.".to_owned(), colours.text_faint),
        };
        let mut bar = div()
            .debug_selector(|| "status-strip".into())
            .flex_none()
            .w_full()
            .h(metrics::FOOTER_HEIGHT())
            .px_4()
            .border_t_1()
            .border_color(colours.border)
            .bg(colours.background)
            .flex()
            .items_center()
            .gap_3()
            .child(section_label("Status", colours))
            .child(
                div()
                    .debug_selector(|| "status-line".into())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colour)
                    .child(text),
            );
        if let Some(item) = self.render_update_item(colours, cx) {
            bar = bar.child(item);
        }
        if settings::flag(cx, "usage_bar") && self.engine.collects_usage() {
            bar = bar.child(self.render_usage_meters(colours, cx));
        }
        bar
    }

    fn render_usage_meters(&self, colours: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let model = self.usage_bar_model(cx);
        let collecting = self.engine.usage_collecting();
        let hover = colours.surface;
        let mut cluster = div()
            .id("usage-bar")
            .debug_selector(|| "usage-bar".into())
            .flex_none()
            .h_full()
            .flex()
            .items_center()
            .gap_3();
        for item in &model.items {
            cluster = cluster.child(self.render_bar_item(item, colours, cx));
        }
        if !model.folded.is_empty() {
            let tip = model
                .folded
                .iter()
                .map(|item| {
                    format!(
                        "{}: {}",
                        leon_usage::heading(item.agent, item.account.as_deref()),
                        folded_line(item)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            cluster = cluster.child(
                mono(format!("+{}", model.folded.len()))
                    .id("usage-folded")
                    .debug_selector(|| "usage-folded".into())
                    .text_color(colours.text_faint)
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)),
            );
        }
        if model.items.is_empty() && self.usage.board.is_empty() {
            cluster = cluster.child(
                div()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint)
                    .child("usage: not read yet"),
            );
        }
        cluster
            .child(
                div()
                    .id("usage-refresh")
                    .debug_selector(|| "usage-refresh".into())
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(4.))
                    .rounded(metrics::RADIUS())
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .tooltip(move |window, cx| {
                        Tooltip::new("Read the limits again now").build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_usage(cx);
                    }))
                    .child(icon(
                        IconName::RefreshCw,
                        px(12.),
                        if collecting {
                            colours.info
                        } else {
                            colours.text_muted
                        },
                    ))
                    .child(
                        div()
                            .debug_selector(|| "usage-updated".into())
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_faint)
                            .child(if collecting {
                                "reading…".to_owned()
                            } else {
                                model.updated.clone().unwrap_or_default()
                            }),
                    ),
            )
            .child(
                div()
                    .id("usage-open")
                    .debug_selector(|| "usage-open".into())
                    .flex_none()
                    .px(px(4.))
                    .rounded(metrics::RADIUS())
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .tooltip(|window, cx| {
                        Tooltip::new(format!(
                            "Usage details{}",
                            keys::keys_label(Command::ShowUsage)
                                .map(|k| format!("  {k}"))
                                .unwrap_or_default()
                        ))
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_usage(window, cx);
                    }))
                    .child(mono("USAGE").text_color(colours.text_muted)),
            )
    }

    fn render_bar_item(
        &self,
        item: &BarItem,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let tip: SharedString = item.tip.clone().into();
        let selector = format!("usage-agent-{}", item.agent.as_str());
        let id = SharedString::from(selector.clone());
        let colour = if item.known {
            level_colour(item.level, colours)
        } else {
            colours.text_faint
        };
        let hover = colours.surface;
        let mut chip = div()
            .id(id)
            .debug_selector(move || selector.clone())
            .flex_none()
            .h_full()
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(metrics::RADIUS())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.toggle_usage(window, cx);
            }))
            .child(agent_icon(item.agent, px(13.), colours));
        if !item.known {
            return chip.child(
                div()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colour)
                    .child(item.label.clone()),
            );
        }
        // One figure per agent, the window closest to its limit, in the colour
        // of its level and with its marker; everything else is in the tooltip.
        chip = chip.child(
            mono(item.figure_text())
                .debug_selector({
                    let name = format!("usage-figure-{}", item.agent.as_str());
                    move || name.clone()
                })
                .text_size(metrics::TEXT_SMALL())
                .text_color(colour),
        );
        if let Some(age) = &item.stale {
            chip = chip.child(
                mono(format!("({age})"))
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint),
            );
        }
        chip
    }

    /// The chip in the header of a live agent session: that agent's primary
    /// window, for the machine the session runs on. A session of one of the
    /// user's accounts shows that account's own reading, or nothing when it
    /// has none (never the agent's own setup's number, which is another
    /// account's).
    pub(super) fn header_usage_chip(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        if !settings::flag(cx, "usage_bar") || !self.engine.collects_usage() {
            return None;
        }
        let super::shell::Main::Live(id) = &self.main else {
            return None;
        };
        let session = self.live.get(*id)?;
        let agent = session.agent?;
        if !settings::usage_agents(cx).contains(&agent) {
            return None;
        }
        let reading =
            self.usage
                .board
                .get_for(&session.machine, agent, session.account.as_deref())?;
        let v = view(reading, self.usage_now(), settings::usage_thresholds(cx));
        let Body::Ready { primary, .. } = &v.body else {
            return None;
        };
        let colour = level_colour(primary.level, colours);
        let text = chip_text(primary, settings::usage_percent_display(cx));
        Some(
            div()
                .id("header-usage")
                .debug_selector(|| "header-usage".into())
                .flex_none()
                .ml_auto()
                .flex()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_usage(window, cx);
                }))
                .child(meter_bar(primary.percent as f32, 28.0, colour, colours))
                .child(
                    mono(text)
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colour),
                ),
        )
    }

    // ----- the view -------------------------------------------------------------------------

    /// The usage view.
    pub(super) fn render_usage(&self, colours: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let rows = self.usage_view_rows(cx);
        let detailed = self.usage.detailed;
        let context = self.current_machine();
        let scope_label = match &self.usage.scope {
            Scope::Context => self.machine_name_of(context.as_str()),
            Scope::Machine(id) => self.machine_name_of(id.as_str()),
            Scope::All => "All machines".to_owned(),
        };
        let updated = self
            .usage
            .board
            .collected_at()
            .map(|at| {
                format!(
                    "Updated {}",
                    leon_usage::ago((self.usage_now() - at).max(0))
                )
            })
            .unwrap_or_else(|| "Not read yet".to_owned());
        let hover = colours.surface_2;
        let chip = |id: &'static str, label: &str, on: bool| {
            let fill = if on {
                colours.surface_2
            } else {
                colours.surface
            };
            mono(label.to_owned())
                .id(id)
                .debug_selector(move || id.into())
                .px(px(8.))
                .py(px(3.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if on {
                    colours.signal
                } else {
                    colours.elevated_border
                })
                .bg(fill)
                .text_color(if on { colours.text } else { colours.text_muted })
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
        };
        let mut body = div()
            .id("usage-rows")
            .debug_selector(|| "usage-rows".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_3();
        if rows.is_empty() {
            body = body.child(
                div()
                    .debug_selector(|| "usage-empty".into())
                    .text_color(colours.text_faint)
                    .child("Nothing to show: no agent is installed here, or none is switched on in Settings."),
            );
        }
        let (active, inactive): (Vec<_>, Vec<_>) = rows
            .iter()
            .enumerate()
            .partition(|(_, row)| !row.is_inactive());
        let heading = match &self.usage.scope {
            Scope::Context => "This machine's agents".to_owned(),
            Scope::Machine(id) => format!("Agents on {}", self.machine_name_of(id.as_str())),
            Scope::All => "Agents on every machine".to_owned(),
        };
        if !active.is_empty() && !inactive.is_empty() {
            body = body.child(
                section_label(&heading, colours)
                    .id("usage-group-active")
                    .debug_selector(|| "usage-group-active".into()),
            );
        }
        for (index, row) in active {
            body = body.child(self.render_usage_row(index, row, detailed, colours, cx));
        }
        if !inactive.is_empty() {
            body = body.child(
                section_label("Not installed / no data", colours)
                    .id("usage-group-inactive")
                    .debug_selector(|| "usage-group-inactive".into()),
            );
            for (_, row) in inactive {
                let reason = match &row.view.body {
                    Body::Unknown(reason) => reason.short(),
                    Body::Ready { .. } => "",
                };
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(colours.text_faint)
                        .child(agent_icon(row.view.agent, px(14.), colours))
                        .child(format!(
                            "{} · {} · {}",
                            row.view.heading(),
                            row.machine_name,
                            reason
                        )),
                );
            }
        }
        body = body.child(self.render_usage_tokens(colours));
        if !leon_core::account::all().is_empty() {
            body = body.child(
                div()
                    .debug_selector(|| "usage-accounts-note".into())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint)
                    .child(ACCOUNTS_NOTE),
            );
        }
        self.card("usage-view", colours)
            .w(px(680.))
            .max_w(self.viewport.width - px(32.))
            .max_h(self.viewport.height - px(112.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .min_h(px(44.))
                    .px_4()
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(section_label("Usage", colours))
                    .child(
                        chip("usage-scope", &scope_label, true).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.usage_step_scope(1);
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        chip("usage-period", self.usage.period.label(), false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.usage.period = this.usage.period.next();
                                this.usage_load_tokens();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        chip("usage-mode-detailed", "Detailed", detailed).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.usage.detailed = true;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        chip("usage-mode-compact", "Compact", !detailed).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.usage.detailed = false;
                                cx.notify();
                            },
                        )),
                    )
                    .child(key_cap(keys::key_label("escape"), colours)),
            )
            .child(body)
            .child(
                div()
                    .flex_none()
                    .min_h(px(40.))
                    .px_4()
                    .border_t_1()
                    .border_color(colours.border)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint)
                    .child(div().debug_selector(|| "usage-updated-line".into()).child(
                        if self.engine.usage_collecting() {
                            "Reading…".to_owned()
                        } else {
                            updated
                        },
                    ))
                    .child(div().flex_1())
                    .child(hint("R", "Refresh now", colours))
                    .children(
                        rows.iter()
                            .any(|row| row.turn_on.is_some())
                            .then(|| hint("O", "Turn on", colours)),
                    )
                    .child(hint("M", "Mode", colours))
                    .child(hint("T", "Period", colours))
                    .child(hint("←/→", "Machine", colours))
                    .child(
                        hint("S", "Settings", colours)
                            .id("usage-settings-link")
                            .debug_selector(|| "usage-settings-link".into())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings(window, cx);
                                this.settings_pick_section(Section::Usage, cx);
                            })),
                    ),
            )
    }

    fn render_usage_row(
        &self,
        index: usize,
        row: &UsageRow,
        detailed: bool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let v = &row.view;
        let display = settings::usage_percent_display(cx);
        let name = format!(
            "{}{}",
            v.heading(),
            v.plan
                .as_ref()
                .map(|plan| format!(" · {plan}"))
                .unwrap_or_default()
        );
        let selector = format!("usage-row-{index}");
        let head = div()
            .flex()
            .items_center()
            .gap_2()
            .child(agent_icon(v.agent, px(14.), colours))
            .child(div().font_weight(FontWeight::MEDIUM).child(name))
            .child(
                mono(row.machine_name.to_uppercase())
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_faint),
            );
        let mut block = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pb_3()
            .border_b_1()
            .border_color(colours.border);
        match &v.body {
            Body::Unknown(reason) => {
                return block.child(head).child(
                    div()
                        .debug_selector(move || format!("usage-unknown-{index}"))
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(format!("Unknown: {}", reason.sentence())),
                );
            }
            Body::Ready { primary, .. } => {
                let reset = primary.reset_text().unwrap_or_default();
                if detailed {
                    block = block.child(
                        head.child(div().flex_1()).child(
                            div()
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(colours.text_muted)
                                .child(reset),
                        ),
                    );
                } else {
                    // Compact: the agent and all its windows on one line.
                    let mut line = head.child(div().flex_1());
                    for window in &row.windows {
                        let colour = level_colour(window.meter.level, colours);
                        line = line.child(
                            mono(window.meter.text_for(display))
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(colour),
                        );
                    }
                    return block.child(line);
                }
            }
        }
        for (n, window) in row.windows.iter().enumerate() {
            let colour = level_colour(window.meter.level, colours);
            let line = div()
                .debug_selector(move || format!("usage-window-{index}-{n}"))
                .flex()
                .items_center()
                .gap_3()
                .child(
                    mono(window.meter.kind.short())
                        .w(px(56.))
                        .text_color(colours.text_faint),
                )
                .child(meter_bar(
                    window.meter.percent as f32,
                    220.0,
                    colour,
                    colours,
                ))
                .child(
                    mono(format!(
                        "{}{}{}",
                        display.fixed(window.meter.percent),
                        if display == PercentDisplay::Remaining {
                            " left"
                        } else {
                            ""
                        },
                        glyph_tail(window.meter.level.glyph())
                    ))
                    .w(px(96.))
                    .text_color(colour),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(window.meter.reset_text().unwrap_or_default()),
                )
                .child(sparkline(&window.spark, colours));
            block = block.child(line);
            if let Some(sentence) = &window.forecast {
                block = block.child(
                    div()
                        .pl(px(68.))
                        .debug_selector(move || format!("usage-forecast-{index}-{n}"))
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_faint)
                        .child(sentence.clone()),
                );
            }
        }
        let mut block = block.child(
            div()
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text_faint)
                .child(v.provenance()),
        );
        if let Some(note) = &row.note {
            block = block.child(
                div()
                    .debug_selector(move || format!("usage-note-{index}"))
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_muted)
                    .child(note.clone()),
            );
            if row.try_again {
                block = block.child(self.try_again_hint(index, colours, cx));
            }
        }
        block
    }

    /// "R Try again", under a row whose last read failed.
    fn try_again_hint(
        &self,
        index: usize,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        hint("R", "Try again", colours)
            .id(("usage-try-again", index))
            .debug_selector(move || format!("usage-try-again-{index}"))
            .text_size(metrics::TEXT_SMALL())
            .text_color(colours.text_muted)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.refresh_usage(cx);
            }))
    }
}

/// What the header's chip says of a window: `wk 93% !! · 1d 11h`, or `wk 7%
/// left !! · 1d 11h` when the setting shows what is left, with the marker of
/// its level as well as its colour.
pub fn chip_text(primary: &Meter, display: PercentDisplay) -> String {
    format!(
        "{} {}%{}{}{}",
        primary.kind.short(),
        display.number(primary.percent),
        if display == PercentDisplay::Remaining {
            " left"
        } else {
            ""
        },
        glyph_tail(primary.level.glyph()),
        primary
            .resets_in
            .filter(|_| !primary.reset_since_seen)
            .map(|s| format!(" · {}", countdown(s)))
            .unwrap_or_default()
    )
}

/// ` !` or ` !!`, or nothing.
fn glyph_tail(glyph: &str) -> String {
    if glyph.is_empty() {
        String::new()
    } else {
        format!(" {glyph}")
    }
}

/// A key and what it does, in the view's footer.
fn hint(key: &str, label: &str, colours: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(key_cap(key.to_owned(), colours))
        .child(label.to_owned())
}

/// A track with a fill of `percent` of `width`.
fn meter_bar(percent: f32, width: f32, colour: Hsla, colours: &Palette) -> Div {
    let fill = (percent.clamp(0.0, 100.0) / 100.0) * width;
    div()
        .flex_none()
        .w(px(width))
        .h(px(4.))
        .rounded(px(1.))
        .bg(colours.border)
        .child(
            div()
                .h_full()
                .w(px(fill.max(if percent > 0.0 { 1.0 } else { 0.0 })))
                .rounded(px(1.))
                .bg(colour),
        )
}

/// A few bars: the history of one window.
fn sparkline(points: &[f32], colours: &Palette) -> Div {
    let mut line = div()
        .flex_none()
        .h(px(14.))
        .w(px(SPARK_POINTS as f32 * 3.0))
        .flex()
        .items_end()
        .gap(px(1.));
    for point in points {
        line = line.child(
            div()
                .w(px(2.))
                .h(px((point * 14.0).max(1.0)))
                .bg(colours.text_faint),
        );
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_usage::{Reason, Source, State, UsageWindow, WindowKind};

    const NOW: i64 = 1_790_000_000;

    fn reading(agent: AgentId, machine: &str, windows: Vec<(WindowKind, f64, i64)>) -> AgentUsage {
        AgentUsage {
            agent,
            machine: machine.into(),
            account_label: None,
            account: None,
            plan: Some("max".into()),
            source: Some(Source::Local),
            observed_at: Some(NOW - 60),
            state: State::Known {
                windows: windows
                    .into_iter()
                    .map(|(kind, used, reset_in)| UsageWindow {
                        kind,
                        used_percent: used,
                        resets_at: Some(NOW + reset_in),
                        window_length: None,
                    })
                    .collect(),
            },
        }
    }

    fn model(board: &Board, width: f32) -> BarModel {
        bar_model(
            board,
            &MachineId::local(),
            "This computer",
            &[AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE],
            NOW,
            Thresholds::default(),
            PercentDisplay::Used,
            width,
        )
    }

    #[test]
    fn when_even_one_figure_per_agent_does_not_fit_the_worst_stays_and_the_rest_fold() {
        let board = Board::new(vec![
            reading(
                AgentId::CLAUDE,
                "local",
                vec![(WindowKind::Weekly, 91.0, 100)],
            ),
            reading(
                AgentId::CODEX,
                "local",
                vec![(WindowKind::Weekly, 16.0, 100)],
            ),
            reading(
                AgentId::OPENCODE,
                "local",
                vec![(WindowKind::Weekly, 42.0, 100)],
            ),
        ]);
        let wide = model(&board, 1600.0);
        assert_eq!(wide.items.len(), 3);
        assert!(wide.folded.is_empty());
        // Too narrow for all three: the agent closest to its limit stays.
        let narrow = model(&board, 420.0);
        assert_eq!(narrow.items.len(), 1);
        assert_eq!(narrow.items[0].agent, AgentId::CLAUDE);
        assert_eq!(narrow.folded.len(), 2);
        assert!(narrow.folded.iter().all(|item| item.known));
    }

    #[test]
    fn ten_agents_fit_one_figure_each_on_a_laptop_window() {
        let agents: Vec<AgentId> = leon_core::agent::builtin()
            .iter()
            .map(|spec| spec.id)
            .take(10)
            .collect();
        assert_eq!(agents.len(), 10, "the catalogue holds at least ten agents");
        let board = Board::new(
            agents
                .iter()
                .map(|agent| {
                    reading(
                        *agent,
                        "local",
                        vec![
                            (WindowKind::FiveHour, 12.0, 4 * 3600),
                            (WindowKind::Weekly, 34.0, 5 * 86_400),
                        ],
                    )
                })
                .collect(),
        );
        // A 14-inch laptop's window: the whole footer, sidebar shown.
        let m = bar_model(
            &board,
            &MachineId::local(),
            "This computer",
            &agents,
            NOW,
            Thresholds::default(),
            PercentDisplay::Used,
            1512.0,
        );
        assert_eq!(m.items.len(), 10, "every agent, one figure each");
        assert!(m.folded.is_empty());
    }

    #[test]
    fn the_bar_shows_each_agents_primary_window() {
        let board = Board::new(vec![
            reading(
                AgentId::CLAUDE,
                "local",
                vec![
                    (WindowKind::FiveHour, 10.0, 8940),
                    (WindowKind::Weekly, 91.0, 126_000),
                ],
            ),
            reading(
                AgentId::CODEX,
                "local",
                vec![
                    (WindowKind::FiveHour, 0.0, 100),
                    (WindowKind::Weekly, 16.0, 100),
                ],
            ),
        ]);
        let m = model(&board, 1600.0);
        assert_eq!(m.items.len(), 2);
        assert_eq!(m.items[0].agent, AgentId::CLAUDE);
        assert_eq!(m.items[0].label, "wk");
        assert_eq!(m.items[0].figure, " 91%");
        assert_eq!(m.items[0].glyph, "!!");
        assert_eq!(m.items[0].level, Level::Critical);
        assert_eq!(m.items[0].figure_text(), " 91% !!");
        assert_eq!(m.items[1].label, "wk");
        assert_eq!(m.items[1].level, Level::Normal);
    }

    #[test]
    fn the_level_and_the_marker_change_at_the_thresholds() {
        let level_of = |used: f64| {
            let board = Board::new(vec![reading(
                AgentId::CODEX,
                "local",
                vec![(WindowKind::FiveHour, used, 100)],
            )]);
            let item = model(&board, 1600.0).items.remove(0);
            (item.level, item.glyph)
        };
        assert_eq!(level_of(59.0), (Level::Normal, ""));
        assert_eq!(level_of(60.0), (Level::Warning, "!"));
        assert_eq!(level_of(79.0), (Level::Warning, "!"));
        assert_eq!(level_of(80.0), (Level::Critical, "!!"));
    }

    #[test]
    fn colours_come_from_the_themes_tokens() {
        let palette = crate::theme::ThemeId::DEFAULT.theme().dark;
        assert_eq!(level_colour(Level::Warning, &palette), palette.warning);
        assert_eq!(level_colour(Level::Critical, &palette), palette.error);
        assert_eq!(level_colour(Level::Normal, &palette), palette.text_muted);
    }

    #[test]
    fn an_unknown_agent_shows_its_reason_and_one_that_is_not_installed_is_left_out() {
        let board = Board::new(vec![
            AgentUsage::unknown(AgentId::CLAUDE, "local", Reason::SourceDisabled),
            AgentUsage::unknown(AgentId::OPENCODE, "local", Reason::NotInstalled),
        ]);
        let m = model(&board, 1600.0);
        assert_eq!(m.items.len(), 1);
        assert_eq!(m.items[0].label, "source off");
        assert!(!m.items[0].known);
        assert!(m.items[0].tip.contains(Reason::SourceDisabled.sentence()));
    }

    #[test]
    fn the_tip_lists_every_window_the_source_and_the_age() {
        let board = Board::new(vec![reading(
            AgentId::CLAUDE,
            "local",
            vec![
                (WindowKind::FiveHour, 10.0, 8940),
                (WindowKind::ModelWeekly("Fable".into()), 0.0, 100),
            ],
        )]);
        let tip = model(&board, 1600.0).items.remove(0).tip;
        assert!(
            tip.contains("5-hour window: 10% used · Resets in 2h 29m"),
            "{tip}"
        );
        assert!(tip.contains("Weekly, Fable: 0% used"), "{tip}");
        assert!(
            tip.contains("from the agent's own files, 1 min ago"),
            "{tip}"
        );
        assert!(
            tip.starts_with("Claude Code on This computer (max)"),
            "{tip}"
        );
    }

    #[test]
    fn a_window_that_reset_since_it_was_seen_reads_zero_in_the_bar() {
        let board = Board::new(vec![reading(
            AgentId::CODEX,
            "local",
            vec![(WindowKind::FiveHour, 97.0, -3600)],
        )]);
        let item = model(&board, 1600.0).items.remove(0);
        assert_eq!(item.percent, 0.0);
        assert_eq!(item.level, Level::Normal);
    }

    #[test]
    fn only_the_machine_in_context_is_in_the_bar() {
        let board = Board::new(vec![
            reading(
                AgentId::CODEX,
                "local",
                vec![(WindowKind::FiveHour, 10.0, 100)],
            ),
            reading(
                AgentId::CODEX,
                "box",
                vec![(WindowKind::FiveHour, 80.0, 100)],
            ),
        ]);
        let m = model(&board, 1600.0);
        assert_eq!(m.items.len(), 1);
        assert_eq!(m.items[0].machine, "local");
    }

    #[test]
    fn the_bar_says_when_it_was_last_read() {
        let board = Board::new(vec![]).collected(NOW - 180);
        assert_eq!(model(&board, 1600.0).updated.as_deref(), Some("3 min ago"));
        assert_eq!(model(&Board::new(vec![]), 1600.0).updated, None);
    }

    #[test]
    fn the_view_lists_the_scope_with_a_forecast_and_a_history_line() {
        let board = Board::new(vec![
            reading(
                AgentId::CODEX,
                "local",
                vec![(WindowKind::FiveHour, 40.0, 5 * 3600)],
            ),
            reading(
                AgentId::CODEX,
                "box",
                vec![(WindowKind::FiveHour, 10.0, 5 * 3600)],
            ),
        ]);
        let mut history = HashMap::new();
        history.insert(
            ("local".to_owned(), AgentId::CODEX, "five_hour".to_owned()),
            vec![
                UsagePoint {
                    at: NOW - 3600,
                    used_percent: 30.0,
                },
                UsagePoint {
                    at: NOW,
                    used_percent: 40.0,
                },
            ],
        );
        let rows = |scope: &Scope| {
            usage_rows(
                &board,
                scope,
                &MachineId::local(),
                &[AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE],
                &|id| id.to_uppercase(),
                NOW,
                Thresholds::default(),
                &history,
            )
        };
        let context = rows(&Scope::Context);
        assert_eq!(context.len(), 1);
        assert_eq!(context[0].machine_name, "LOCAL");
        let window = &context[0].windows[0];
        assert_eq!(window.spark, [0.3, 0.4]);
        assert_eq!(
            window.forecast.as_deref(),
            Some("At this pace you will not hit the limit before the reset (estimate).")
        );
        assert_eq!(rows(&Scope::All).len(), 2);
        assert_eq!(
            rows(&Scope::Machine(MachineId::from_string("box"))).len(),
            1
        );
    }

    #[test]
    fn no_forecast_without_two_observations() {
        let meter = |reset_since_seen| Meter {
            kind: WindowKind::FiveHour,
            percent: 40.0,
            level: Level::Normal,
            resets_in: Some(3600),
            reset_since_seen,
        };
        let one = [UsagePoint {
            at: NOW,
            used_percent: 40.0,
        }];
        assert_eq!(window_row(&meter(false), &one, NOW).forecast, None);
        assert_eq!(window_row(&meter(false), &[], NOW).forecast, None);
        let two = [
            UsagePoint {
                at: NOW - 3600,
                used_percent: 30.0,
            },
            UsagePoint {
                at: NOW,
                used_percent: 40.0,
            },
        ];
        assert_eq!(window_row(&meter(true), &two, NOW).forecast, None);
    }

    #[test]
    fn a_forecast_that_hits_before_the_reset_says_when() {
        let meter = Meter {
            kind: WindowKind::FiveHour,
            percent: 70.0,
            level: Level::Normal,
            resets_in: Some(5 * 3600),
            reset_since_seen: false,
        };
        let points = [
            UsagePoint {
                at: NOW - 3600,
                used_percent: 30.0,
            },
            UsagePoint {
                at: NOW,
                used_percent: 70.0,
            },
        ];
        assert_eq!(
            window_row(&meter, &points, NOW).forecast.as_deref(),
            Some("At this pace: limit in ~45m (estimate).")
        );
    }

    #[test]
    fn the_history_line_keeps_only_the_newest_points() {
        let meter = Meter {
            kind: WindowKind::Weekly,
            percent: 10.0,
            level: Level::Normal,
            resets_in: None,
            reset_since_seen: false,
        };
        let points: Vec<UsagePoint> = (0..40)
            .map(|i| UsagePoint {
                at: NOW - 4000 + i * 60,
                used_percent: i as f64,
            })
            .collect();
        let row = window_row(&meter, &points, NOW);
        assert_eq!(row.spark.len(), SPARK_POINTS);
        assert!((row.spark.last().unwrap() - 0.39).abs() < 1e-6);
    }

    #[test]
    fn the_scope_steps_through_the_context_the_other_machines_and_all() {
        let local = MachineId::local();
        let box_ = MachineId::from_string("box");
        let machines = [local.clone(), box_.clone()];
        let s = Scope::Context;
        let s = step_scope(&s, &machines, &local, 1);
        assert_eq!(s, Scope::Machine(box_.clone()));
        let s = step_scope(&s, &machines, &local, 1);
        assert_eq!(s, Scope::All);
        let s = step_scope(&s, &machines, &local, 1);
        assert_eq!(s, Scope::Context);
        assert_eq!(step_scope(&s, &machines, &local, -1), Scope::All);
    }

    fn model_display(board: &Board, width: f32, display: PercentDisplay) -> BarModel {
        bar_model(
            board,
            &MachineId::local(),
            "This computer",
            &[AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE],
            NOW,
            Thresholds::default(),
            display,
            width,
        )
    }

    fn orca_like() -> Board {
        Board::new(vec![reading(
            AgentId::CLAUDE,
            "local",
            vec![
                (WindowKind::FiveHour, 10.0, 2 * 3600 + 29 * 60 + 40),
                (WindowKind::Weekly, 91.0, DAY_AND_11H + 59),
                (
                    WindowKind::ModelWeekly("Fable".into()),
                    0.0,
                    6 * 86_400 + 7 * 3600,
                ),
            ],
        )])
    }

    const DAY_AND_11H: i64 = 86_400 + 11 * 3600;

    #[test]
    fn the_bar_shows_the_window_closest_to_its_limit_and_the_tip_lists_them_all() {
        let item = model(&orca_like(), 2400.0).items.remove(0);
        assert_eq!((item.label.as_str(), item.figure.as_str()), ("wk", " 91%"));
        assert_eq!(item.figure_text(), " 91% !!");
        for line in [
            "5-hour window: 10% used · Resets in 2h 29m",
            "Weekly: 91% used (near the limit) · Resets in 1d 11h",
            "Weekly, Fable: 0% used · Resets in 6d 7h",
        ] {
            assert!(item.tip.contains(line), "{line} missing from {}", item.tip);
        }
    }

    #[test]
    fn the_tip_floors_countdowns_and_a_reset_that_has_passed_says_so() {
        let board = Board::new(vec![reading(
            AgentId::CLAUDE,
            "local",
            vec![
                (WindowKind::FiveHour, 20.0, 47 * 60 + 59),
                (WindowKind::Weekly, 30.0, 6 * 86_400 + 7 * 3600 + 3000),
                (WindowKind::Monthly, 40.0, -5),
            ],
        )]);
        let item = model(&board, 2400.0).items.remove(0);
        assert_eq!((item.label.as_str(), item.figure.as_str()), ("wk", " 30%"));
        assert!(
            item.tip.contains("5-hour window: 20% used · Resets in 47m"),
            "{}",
            item.tip
        );
        assert!(
            item.tip.contains("Weekly: 30% used · Resets in 6d 7h"),
            "{}",
            item.tip
        );
        assert!(
            item.tip
                .contains("Monthly: 0% used · Reset since last seen"),
            "{}",
            item.tip
        );
    }

    #[test]
    fn percent_left_is_the_complement_of_the_rounded_used_figure_in_every_surface() {
        let m = model_display(&orca_like(), 2400.0, PercentDisplay::Remaining);
        let item = &m.items[0];
        assert_eq!(item.figure_text(), "  9% left !!");
        assert_eq!((item.figure.as_str(), item.left), ("  9%", true));
        assert!(
            item.tip.contains("Weekly: 9% left (near the limit)"),
            "{}",
            item.tip
        );
        // The level is still judged on what is used.
        assert_eq!(item.level, Level::Critical);
        let rows = usage_rows(
            &orca_like(),
            &Scope::Context,
            &MachineId::local(),
            &[AgentId::CLAUDE],
            &|id| id.to_owned(),
            NOW,
            Thresholds::default(),
            &HashMap::new(),
        );
        let weekly = &rows[0].windows[1].meter;
        assert_eq!(
            weekly.text_for(PercentDisplay::Remaining),
            "wk   9% left !!"
        );
        assert_eq!(
            chip_text(weekly, PercentDisplay::Remaining),
            "wk 9% left !! · 1d 11h"
        );
        assert_eq!(
            chip_text(weekly, PercentDisplay::Used),
            "wk 91% !! · 1d 11h"
        );
    }

    #[test]
    fn a_half_percent_rounds_up_in_the_bar_the_tip_and_the_chip() {
        let board = Board::new(vec![reading(
            AgentId::CODEX,
            "local",
            vec![(WindowKind::FiveHour, 12.5, 3600)],
        )]);
        let item = model(&board, 2400.0).items.remove(0);
        assert_eq!(item.figure, " 13%");
        assert_eq!(item.figure_text(), " 13%");
        assert!(item.tip.contains("13% used"), "{}", item.tip);
        let left = model_display(&board, 2400.0, PercentDisplay::Remaining);
        assert_eq!(left.items[0].figure_text(), " 87% left");
    }

    #[test]
    fn the_view_lists_the_agent_nearest_a_limit_first() {
        let board = Board::new(vec![
            reading(
                AgentId::CLAUDE,
                "local",
                vec![(WindowKind::Weekly, 20.0, 100)],
            ),
            reading(
                AgentId::CODEX,
                "local",
                vec![(WindowKind::Weekly, 91.0, 100)],
            ),
            AgentUsage::unknown(AgentId::OPENCODE, "local", Reason::NotSignedIn),
        ]);
        let rows = usage_rows(
            &board,
            &Scope::Context,
            &MachineId::local(),
            &[AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE],
            &|id| id.to_owned(),
            NOW,
            Thresholds::default(),
            &HashMap::new(),
        );
        let order: Vec<AgentId> = rows.iter().map(|r| r.view.agent).collect();
        assert_eq!(order, [AgentId::CODEX, AgentId::CLAUDE, AgentId::OPENCODE]);
    }

    #[test]
    fn numbers_kept_after_a_failed_read_are_marked_with_their_age_and_say_why() {
        let board = Board::new(vec![reading(
            AgentId::CLAUDE,
            "local",
            vec![(WindowKind::Weekly, 40.0, 100_000)],
        )]);
        let mut m = model(&board, 2400.0);
        assert_eq!(m.items[0].stale, None);
        let mut statuses = Statuses::new();
        statuses.insert(
            AgentId::CLAUDE,
            SourceStatus {
                failed: Some(Reason::SessionExpired),
                retry_at: Some(NOW + 120),
                ..Default::default()
            },
        );
        let note = Note {
            statuses: &statuses,
            now: NOW,
            next_read: None,
            mac: false,
            local: "local",
        };
        annotate_bar(&mut m, &note);
        assert_eq!(m.items[0].stale.as_deref(), Some("1 min ago"));
        assert!(m.items[0].known, "the numbers stay");
        assert!(
            m.items[0].tip.contains("Run the agent once"),
            "{}",
            m.items[0].tip
        );
        let mut rows = usage_rows(
            &board,
            &Scope::Context,
            &MachineId::local(),
            &[AgentId::CLAUDE],
            &|id| id.to_owned(),
            NOW,
            Thresholds::default(),
            &HashMap::new(),
        );
        annotate_rows(&mut rows, &note);
        assert!(matches!(rows[0].view.body, Body::Ready { .. }));
        assert!(rows[0]
            .note
            .as_deref()
            .unwrap()
            .contains("Run the agent once"));
        assert!(rows[0].try_again);
    }
}

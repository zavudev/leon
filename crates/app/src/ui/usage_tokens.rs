//! The tokens section of the usage view: what the agents used, and what it
//! would cost at their providers' API prices.
//!
//! The *model* half is pure: [`token_section`] turns a [`Report`] into the
//! lines the view prints (a summary, then the tokens per agent, per model and
//! per day), and says in words that the cost is an estimate that a
//! subscription is not billed by. The *drawing* half reads the counts the
//! history import stored, so the view never reads a transcript, and uses the
//! theme's tokens only.
//!
//! The prices are the ones bundled with Leon, with the `prices.json` next to
//! the settings file laid over them when there is one.

use gpui_kit::prelude::*;
use gpui_kit::{div, Div, SharedString};
use leon_core::AgentId;
use leon_usage::spend::{share_text, tokens_text, Line, Report};
use leon_usage::{Cost, PriceTable};

use super::shell::Shell;
use super::usage_view::agent_name;
use super::widgets::{mono, section_label};
use crate::agent_usage::Scope;
use crate::icons::agent_icon;
use crate::theme::{metrics, px, Palette};

/// What a cost is, said wherever one is shown.
pub const ESTIMATE_NOTE: &str =
    "Estimated at the providers' API list prices for these tokens. Not what a subscription is billed.";

/// One printed line of a list.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenLine {
    /// The agent, to draw its logo; none for models and days.
    pub agent: Option<AgentId>,
    /// The agent's, model's or day's name.
    pub label: String,
    /// `in 1.2M · out 300k · cache 84%`.
    pub detail: String,
    /// All the tokens, short.
    pub tokens: String,
    /// The estimated cost, or `no price`.
    pub cost: String,
}

/// A list under its title.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenGroup {
    /// `By agent`, `By model`, `By day`.
    pub title: &'static str,
    /// Its lines.
    pub lines: Vec<TokenLine>,
}

/// What the section prints.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenSection {
    /// `last 30 days · This machine`.
    pub scope: String,
    /// The totals in one line; none when nothing was counted.
    pub summary: Option<String>,
    /// What to say instead of the lists when nothing was counted.
    pub empty: Option<&'static str>,
    /// The lists: all three when detailed, the agents only when compact.
    pub groups: Vec<TokenGroup>,
    /// What the figures leave out, one sentence each.
    pub notes: Vec<String>,
}

/// The cost as a phrase: `est. $12.34`, `est. $12.34 + unpriced`, `no price`.
fn estimate(cost: &Cost) -> String {
    match cost.text() {
        text if text.starts_with('$') || text.starts_with('<') => format!("est. {text}"),
        text => text,
    }
}

fn detail<K>(line: &Line<K>) -> String {
    format!(
        "in {} · out {} · cache {}",
        tokens_text(line.counts.all_input()),
        tokens_text(line.counts.output),
        line.cache_share().map_or("-".to_owned(), share_text),
    )
}

fn line<K>(line: &Line<K>, agent: Option<AgentId>, label: String) -> TokenLine {
    TokenLine {
        agent,
        label,
        detail: detail(line),
        tokens: tokens_text(line.counts.total()),
        cost: line.cost.text(),
    }
}

/// What the view prints for `report`, in the scope called `scope`.
/// `problems` are the entries of the user's price file that could not be
/// used.
pub fn token_section(
    report: &Report,
    scope: &str,
    detailed: bool,
    problems: &[String],
) -> TokenSection {
    let mut notes = problems.to_vec();
    if report.partial && !report.is_empty() {
        notes.push(
            "Some models have no price, so their tokens are counted and their cost is left out. \
             Add them to prices.json next to the settings file."
                .to_owned(),
        );
    }
    let scope = format!("{} · {scope}", report.period.label());
    if report.is_empty() {
        return TokenSection {
            scope,
            summary: None,
            empty: Some(
                "No tokens counted yet. They are read from the agents' own transcripts when \
                 the history is imported.",
            ),
            groups: Vec::new(),
            notes,
        };
    }
    let summary = format!(
        "{} tokens · {} of input from cache · {}",
        tokens_text(report.total.counts.total()),
        report.cache_share.map_or("-".to_owned(), share_text),
        estimate(&report.total.cost),
    );
    let mut groups = vec![TokenGroup {
        title: "By agent",
        lines: report
            .by_agent
            .iter()
            .map(|l| line(l, Some(l.key), agent_name(l.key).to_owned()))
            .collect(),
    }];
    if detailed {
        groups.push(TokenGroup {
            title: "By model",
            lines: report
                .by_model
                .iter()
                .map(|l| line(l, None, l.key.clone()))
                .collect(),
        });
        groups.push(TokenGroup {
            title: "By day (UTC)",
            lines: report
                .by_day
                .iter()
                .map(|l| line(l, None, l.key.clone()))
                .collect(),
        });
    }
    TokenSection {
        scope,
        summary: Some(summary),
        empty: None,
        groups,
        notes,
    }
}

/// The prices in force: Leon's, with the user's file laid over them. The
/// second value is what to tell the user about that file (`None` when there
/// is nothing to tell).
pub fn load_prices(user_file: Option<&str>) -> (PriceTable, Vec<String>) {
    let bundled = PriceTable::bundled();
    let Some(text) = user_file else {
        return (bundled, Vec::new());
    };
    match bundled.clone().with_overrides(text) {
        Ok((table, problems)) => (
            table,
            problems
                .into_iter()
                .map(|problem| format!("prices.json: {problem}."))
                .collect(),
        ),
        Err(error) => (
            bundled,
            vec![format!(
                "prices.json could not be read ({error}); the bundled prices are in use."
            )],
        ),
    }
}

impl Shell {
    /// Reads the counts of the period from the store.
    pub(super) fn usage_load_tokens(&mut self) {
        let since = self.usage.period.since_day(self.usage_now());
        match self.engine.store().token_usage(since.as_deref()) {
            Ok(rows) => self.usage.tokens = rows,
            Err(error) => tracing::warn!(%error, "could not read the token counts"),
        }
    }

    /// Reads the user's price file, next to the settings file, and lays it
    /// over the bundled prices.
    pub(super) fn usage_load_prices(&mut self, cx: &gpui_kit::App) {
        let text = crate::settings::sibling(leon_usage::pricing::FILE_NAME, cx)
            .and_then(|path| std::fs::read_to_string(path).ok());
        let (prices, notes) = load_prices(text.as_deref());
        self.usage.prices = prices;
        self.usage.price_notes = notes;
    }

    /// The machine the section sums, or none for every machine.
    fn usage_tokens_machine(&self) -> Option<leon_core::MachineId> {
        match &self.usage.scope {
            Scope::Context => Some(self.current_machine()),
            Scope::Machine(id) => Some(id.clone()),
            Scope::All => None,
        }
    }

    /// What the section prints now.
    pub(super) fn usage_tokens_section(&self) -> TokenSection {
        let machine = self.usage_tokens_machine();
        let report = leon_usage::spend_report(
            &self.usage.tokens,
            &self.usage.prices,
            machine.as_ref(),
            self.usage.period,
            self.usage_now(),
        );
        let scope = match &machine {
            Some(id) => self.machine_name_of(id.as_str()),
            None => "All machines".to_owned(),
        };
        token_section(
            &report,
            &scope,
            self.usage.detailed,
            &self.usage.price_notes,
        )
    }

    /// The section, under the agents' limits.
    pub(super) fn render_usage_tokens(&self, colours: &Palette) -> Div {
        let section = self.usage_tokens_section();
        let small = metrics::TEXT_SMALL();
        let mut block = div()
            .debug_selector(|| "usage-tokens".into())
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(section_label("Tokens and estimated cost", colours))
                    .child(
                        mono(section.scope.clone())
                            .debug_selector(|| "usage-tokens-scope".into())
                            .text_size(small)
                            .text_color(colours.text_faint),
                    ),
            );
        if let Some(summary) = &section.summary {
            block = block.child(
                mono(summary.clone())
                    .debug_selector(|| "usage-tokens-summary".into())
                    .text_color(colours.text),
            );
        }
        if let Some(empty) = section.empty {
            block = block.child(
                div()
                    .debug_selector(|| "usage-tokens-empty".into())
                    .text_size(small)
                    .text_color(colours.text_faint)
                    .child(empty),
            );
        }
        for (group, list) in section.groups.iter().enumerate() {
            block = block.child(
                div()
                    .mt(px(4.))
                    .text_size(small)
                    .text_color(colours.text_muted)
                    .child(list.title),
            );
            for (n, row) in list.lines.iter().enumerate() {
                block = block.child(token_row(group, n, row, colours));
            }
        }
        if section.summary.is_some() {
            block = block.child(
                div()
                    .debug_selector(|| "usage-tokens-estimate".into())
                    .text_size(small)
                    .text_color(colours.text_faint)
                    .child(ESTIMATE_NOTE),
            );
        }
        for (n, note) in section.notes.iter().enumerate() {
            block = block.child(
                div()
                    .debug_selector(move || format!("usage-tokens-note-{n}"))
                    .text_size(small)
                    .text_color(colours.text_muted)
                    .child(note.clone()),
            );
        }
        block
    }
}

/// One line of a list: logo or nothing, name, detail, tokens, cost.
fn token_row(group: usize, n: usize, row: &TokenLine, colours: &Palette) -> Div {
    let small = metrics::TEXT_SMALL();
    let mut line = div()
        .debug_selector(move || format!("usage-tokens-{group}-{n}"))
        .flex()
        .items_center()
        .gap_3();
    if let Some(agent) = row.agent {
        line = line.child(agent_icon(agent, px(14.), colours));
    }
    line.child(
        div()
            .w(px(150.))
            .flex_none()
            .truncate()
            .child(SharedString::from(row.label.clone())),
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_size(small)
            .text_color(colours.text_faint)
            .child(SharedString::from(row.detail.clone())),
    )
    .child(
        mono(row.tokens.clone())
            .w(px(64.))
            .text_color(colours.text_muted),
    )
    .child(mono(row.cost.clone()).w(px(150.)).text_color(colours.text))
}

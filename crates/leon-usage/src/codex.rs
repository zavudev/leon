//! Codex: the limits its own session log records.
//!
//! Codex writes a `token_count` event into the rollout file of a session after
//! each answer, with the account's `rate_limits`: a `primary` window (five
//! hours, 300 minutes) and a `secondary` one (a week, 10080 minutes), each with
//! the used percentage and the reset time in Unix seconds, and the plan. It is
//! a purely local source: no network, no credential. The numbers describe the
//! moment of the event, so the reading carries that time and the model decides
//! what it means now.

use chrono::DateTime;
use leon_core::AgentKind;
use serde_json::Value;

use crate::forecast::Sample;
use crate::model::{AgentUsage, Collected, Reason, Source, State, UsageWindow, WindowKind, MINUTE};

/// The limit id of the account's main limit.
const MAIN_LIMIT: &str = "codex";

struct Event {
    at: i64,
    limit_id: String,
    plan: Option<String>,
    windows: Vec<UsageWindow>,
}

/// The kind of a window from its length in minutes.
pub fn kind_for_minutes(minutes: i64) -> WindowKind {
    match minutes {
        300 => WindowKind::FiveHour,
        10_080 => WindowKind::Weekly,
        43_200 | 43_800 | 44_640 => WindowKind::Monthly,
        m if m % (24 * 60) == 0 => WindowKind::Custom(format!("{}d", m / (24 * 60))),
        m if m % 60 == 0 => WindowKind::Custom(format!("{}h", m / 60)),
        m => WindowKind::Custom(format!("{m}m")),
    }
}

fn window(raw: &Value) -> Option<UsageWindow> {
    let used = raw.get("used_percent")?.as_f64()?;
    if !used.is_finite() {
        return None;
    }
    let minutes = raw.get("window_minutes").and_then(Value::as_i64);
    Some(UsageWindow {
        kind: minutes.map_or(WindowKind::Custom("limit".into()), kind_for_minutes),
        used_percent: used.clamp(0.0, 100.0),
        resets_at: raw.get("resets_at").and_then(Value::as_i64),
        window_length: minutes.map(|m| m * MINUTE),
    })
}

fn event(line: &str) -> Option<Event> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "token_count" {
        return None;
    }
    let limits = payload.get("rate_limits")?;
    let windows: Vec<UsageWindow> = ["primary", "secondary"]
        .iter()
        .filter_map(|key| limits.get(*key).and_then(window))
        .collect();
    if windows.is_empty() {
        return None;
    }
    let at = DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?)
        .ok()?
        .timestamp();
    Some(Event {
        at,
        limit_id: limits
            .get("limit_id")
            .and_then(Value::as_str)
            .unwrap_or(MAIN_LIMIT)
            .to_owned(),
        plan: limits
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        windows,
    })
}

/// Reads the `token_count` lines of Codex rollout files (in any order, other
/// lines ignored) into the latest reading of the account's main limit and the
/// observations that came with it. With no usable line the reading is
/// unknown, [`Reason::NoData`].
pub fn parse_rollout_lines(text: &str, machine: &str) -> Collected {
    let events: Vec<Event> = text.lines().filter_map(event).collect();
    let chosen = if events.iter().any(|e| e.limit_id == MAIN_LIMIT) {
        MAIN_LIMIT.to_owned()
    } else {
        match events.first() {
            Some(first) => first.limit_id.clone(),
            None => {
                return Collected {
                    usage: AgentUsage::unknown(AgentKind::Codex, machine, Reason::NoData),
                    samples: Vec::new(),
                }
            }
        }
    };
    let mut events: Vec<Event> = events
        .into_iter()
        .filter(|e| e.limit_id == chosen)
        .collect();
    events.sort_by_key(|e| e.at);
    let samples = events
        .iter()
        .flat_map(|e| {
            e.windows.iter().map(|w| {
                (
                    w.kind.clone(),
                    Sample {
                        at: e.at,
                        used_percent: w.used_percent,
                    },
                )
            })
        })
        .collect();
    let Some(latest) = events.pop() else {
        return Collected {
            usage: AgentUsage::unknown(AgentKind::Codex, machine, Reason::NoData),
            samples: Vec::new(),
        };
    };
    Collected {
        usage: AgentUsage {
            agent: AgentKind::Codex,
            machine: machine.to_owned(),
            account_label: latest.plan.clone(),
            plan: latest.plan,
            source: Some(Source::Local),
            observed_at: Some(latest.at),
            state: State::Known {
                windows: latest.windows,
            },
        },
        samples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Effective;

    fn line(
        ts: &str,
        limit_id: &str,
        p: Option<(f64, i64, i64)>,
        s: Option<(f64, i64, i64)>,
    ) -> String {
        let w = |x: Option<(f64, i64, i64)>| match x {
            Some((u, m, r)) => {
                format!(r#"{{"used_percent":{u},"window_minutes":{m},"resets_at":{r}}}"#)
            }
            None => "null".into(),
        };
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":null,"rate_limits":{{"limit_id":"{limit_id}","primary":{},"secondary":{},"credits":null,"plan_type":"plus"}}}}}}"#,
            w(p),
            w(s)
        )
    }

    #[test]
    fn the_latest_event_gives_the_windows_and_the_plan() {
        let text = [
            line(
                "2026-10-04T00:36:41.733Z",
                "codex",
                Some((0.0, 300, 1_791_092_119)),
                Some((0.0, 10080, 1_791_678_919)),
            ),
            line(
                "2026-10-04T18:59:06.753Z",
                "codex",
                Some((97.0, 300, 1_791_154_068)),
                Some((15.0, 10080, 1_791_678_919)),
            ),
        ]
        .join("\n");
        let got = parse_rollout_lines(&text, "local");
        assert_eq!(got.usage.plan.as_deref(), Some("plus"));
        assert_eq!(got.usage.source, Some(Source::Local));
        let State::Known { windows } = &got.usage.state else {
            panic!("expected windows");
        };
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, WindowKind::FiveHour);
        assert_eq!(windows[0].used_percent, 97.0);
        assert_eq!(windows[0].resets_at, Some(1_791_154_068));
        assert_eq!(windows[0].window_length, Some(300 * 60));
        assert_eq!(windows[1].kind, WindowKind::Weekly);
        assert_eq!(got.samples.len(), 4);
    }

    #[test]
    fn events_without_windows_and_foreign_limits_are_skipped() {
        let text = [
            line("2026-10-04T18:59:08.909Z", "premium", None, None),
            line(
                "2026-10-04T18:00:00.000Z",
                "codex",
                Some((12.0, 300, 1_791_154_068)),
                None,
            ),
            line(
                "2026-10-04T19:00:00.000Z",
                "other",
                Some((99.0, 300, 1_791_154_068)),
                None,
            ),
        ]
        .join("\n");
        let got = parse_rollout_lines(&text, "local");
        let State::Known { windows } = &got.usage.state else {
            panic!("expected windows");
        };
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_percent, 12.0);
    }

    #[test]
    fn lines_in_any_order_still_give_the_newest() {
        let text = [
            line(
                "2026-10-04T19:00:00.000Z",
                "codex",
                Some((50.0, 300, 1_791_154_068)),
                None,
            ),
            line(
                "2026-10-04T18:00:00.000Z",
                "codex",
                Some((10.0, 300, 1_791_154_068)),
                None,
            ),
        ]
        .join("\n");
        let got = parse_rollout_lines(&text, "local");
        assert_eq!(got.usage.observed_at, Some(1_791_140_400));
        let State::Known { windows } = &got.usage.state else {
            panic!()
        };
        assert_eq!(windows[0].used_percent, 50.0);
        assert!(got.samples[0].1.at < got.samples[1].1.at);
    }

    #[test]
    fn garbage_and_empty_input_is_no_data() {
        for text in [
            "",
            "not json\n{}\n",
            r#"{"payload":{"type":"agent_message"}}"#,
        ] {
            let got = parse_rollout_lines(text, "m");
            assert_eq!(
                got.usage.state,
                State::Unknown {
                    reason: Reason::NoData
                }
            );
            assert!(got.samples.is_empty());
        }
    }

    #[test]
    fn a_day_old_reading_is_judged_by_its_reset_times() {
        let text = line(
            "2026-10-04T18:59:06.753Z",
            "codex",
            Some((97.0, 300, 1_791_154_068)),
            Some((15.0, 10080, 1_791_678_919)),
        );
        let got = parse_rollout_lines(&text, "local");
        // One day later the five-hour window has reset, the weekly one has not.
        let now = 1_791_154_800 + 86_400;
        let Effective::Windows(windows) = got.usage.effective(now) else {
            panic!("expected windows");
        };
        assert!(windows[0].reset_since_seen);
        assert_eq!(windows[0].used_percent, 0.0);
        assert!(!windows[1].reset_since_seen);
        assert_eq!(windows[1].used_percent, 15.0);
    }

    #[test]
    fn unusual_window_lengths_get_a_readable_kind() {
        assert_eq!(kind_for_minutes(300), WindowKind::FiveHour);
        assert_eq!(kind_for_minutes(1440), WindowKind::Custom("1d".into()));
        assert_eq!(kind_for_minutes(120), WindowKind::Custom("2h".into()));
        assert_eq!(kind_for_minutes(45), WindowKind::Custom("45m".into()));
    }
}

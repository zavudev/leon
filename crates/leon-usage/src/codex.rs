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
use leon_core::AgentId;
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

/// One window as a source gave it, before it is told what it is.
struct Raw {
    used: f64,
    minutes: Option<f64>,
    resets_at: Option<i64>,
}

/// The tolerance, in minutes, when matching a window's length to the session
/// or the weekly one: older Codex versions drift by one.
const TOLERANCE: f64 = 1.0;

fn near(minutes: Option<f64>, target: f64) -> bool {
    minutes.is_some_and(|m| (m - target).abs() <= TOLERANCE)
}

/// Tells the five-hour window from the weekly one the way Orca does: by length
/// within a minute, and, when a window's length is not one of those two (or
/// not given), by position: the primary one is the session, the secondary one
/// the week. A second window of an already taken kind is dropped.
fn classify(primary: Option<Raw>, secondary: Option<Raw>) -> Vec<UsageWindow> {
    #[derive(PartialEq)]
    enum Slot {
        Session,
        Weekly,
        Unknown,
    }
    let slot = |raw: &Raw| {
        if near(raw.minutes, 300.0) {
            Slot::Session
        } else if near(raw.minutes, 10_080.0) {
            Slot::Weekly
        } else {
            Slot::Unknown
        }
    };
    let mut session = None;
    let mut weekly = None;
    for (index, raw) in [primary, secondary].into_iter().enumerate() {
        let Some(raw) = raw else { continue };
        match (slot(&raw), index) {
            (Slot::Session, _) | (Slot::Unknown, 0) if session.is_none() => session = Some(raw),
            (Slot::Weekly, _) | (Slot::Unknown, 1) if weekly.is_none() => weekly = Some(raw),
            _ => {}
        }
    }
    let build = |raw: Raw, kind: WindowKind, minutes: i64| UsageWindow {
        kind,
        used_percent: raw.used.clamp(0.0, 100.0),
        resets_at: raw.resets_at,
        window_length: raw
            .minutes
            .map(|m| m.round() as i64 * MINUTE)
            .or(Some(minutes * MINUTE)),
    };
    let mut out = Vec::new();
    if let Some(raw) = session {
        out.push(build(raw, WindowKind::FiveHour, 300));
    }
    if let Some(raw) = weekly {
        out.push(build(raw, WindowKind::Weekly, 10_080));
    }
    out
}

/// Whether `auth.json` is an API-key login: a key and no ChatGPT tokens. Such
/// an account has no subscription limits to read.
pub fn is_api_key_login(json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return false;
    };
    let key = value
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .is_some_and(|key| !key.trim().is_empty());
    let token = value
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .is_some_and(|token| !token.trim().is_empty());
    key && !token
}

fn raw(value: &Value, minutes_key: &str, scale: f64, reset_key: &str) -> Option<Raw> {
    let value = value.as_object()?;
    let used = value
        .get("used_percent")?
        .as_f64()
        .filter(|u| u.is_finite())?;
    Some(Raw {
        used,
        minutes: value
            .get(minutes_key)
            .and_then(Value::as_f64)
            .filter(|m| m.is_finite() && *m > 0.0)
            .map(|m| m / scale),
        resets_at: value.get(reset_key).and_then(Value::as_i64),
    })
}

fn event(line: &str) -> Option<Event> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "token_count" {
        return None;
    }
    let limits = payload.get("rate_limits")?;
    let side = |key: &str| {
        limits
            .get(key)
            .and_then(|w| raw(w, "window_minutes", 1.0, "resets_at"))
    };
    let windows = classify(side("primary"), side("secondary"));
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
                    usage: AgentUsage::unknown(AgentId::CODEX, machine, Reason::NoData),
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
            usage: AgentUsage::unknown(AgentId::CODEX, machine, Reason::NoData),
            samples: Vec::new(),
        };
    };
    Collected {
        usage: AgentUsage {
            agent: AgentId::CODEX,
            machine: machine.to_owned(),
            account_label: latest.plan.clone(),
            account: None,
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

/// The host the backend call goes to, and nowhere else.
pub const BACKEND_HOST: &str = "chatgpt.com";
/// The endpoint Codex's own usage screen reads.
pub const BACKEND_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// How old the newest observation of the session log may be (seconds) before
/// the backend is asked for a fresher one: while Codex is being used its log is
/// as fresh as the backend, and costs nothing.
pub const LOG_FRESH: i64 = 10 * 60;

/// What Codex's `auth.json` held for the backend call.
#[derive(Debug)]
pub struct Credential {
    /// The ChatGPT access token.
    pub token: crate::secret::Secret,
    /// The account id the endpoint is also told (an identifier, not a secret).
    pub account_id: Option<String>,
}

/// Reads `auth.json` (`{"tokens":{"access_token":..,"account_id":..}}`). An
/// API-key login has no tokens: not signed in with an account.
pub fn parse_credential(json: &str) -> Option<Credential> {
    let value: Value = serde_json::from_str(json).ok()?;
    let tokens = value.get("tokens")?;
    let token = tokens.get("access_token")?.as_str()?.trim();
    if token.is_empty() {
        return None;
    }
    Some(Credential {
        token: crate::secret::Secret::new(token),
        account_id: tokens
            .get("account_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .map(str::to_owned),
    })
}

/// Parses the backend's answer: `plan_type` and `rate_limit.primary_window`
/// and `secondary_window`, each with `used_percent`, `limit_window_seconds`
/// and `reset_at` in Unix seconds. An answer without a plan or without a
/// window is a [`Reason::ParseError`].
pub fn parse_backend(body: &str, machine: &str, now: i64) -> AgentUsage {
    let fail = || AgentUsage::unknown(AgentId::CODEX, machine, Reason::ParseError);
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return fail();
    };
    let Some(plan) = value.get("plan_type").and_then(Value::as_str) else {
        return fail();
    };
    let side = |key: &str| {
        value
            .pointer(&format!("/rate_limit/{key}"))
            .and_then(|w| raw(w, "limit_window_seconds", 60.0, "reset_at"))
    };
    let windows = classify(side("primary_window"), side("secondary_window"));
    if windows.is_empty() {
        return fail();
    }
    AgentUsage {
        agent: AgentId::CODEX,
        machine: machine.to_owned(),
        account_label: Some(plan.to_owned()),
        account: None,
        plan: Some(plan.to_owned()),
        source: Some(Source::VendorApi),
        observed_at: Some(now),
        state: State::Known { windows },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backend_answer_gives_the_two_windows_and_the_plan() {
        let body = r#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":12.5,"limit_window_seconds":18000,"reset_at":1790003600},"secondary_window":{"used_percent":40,"limit_window_seconds":604800,"reset_at":1790500000}}}"#;
        let usage = parse_backend(body, "local", 1_790_000_000);
        assert_eq!(usage.plan.as_deref(), Some("plus"));
        assert_eq!(usage.source, Some(Source::VendorApi));
        let State::Known { windows } = usage.state else {
            panic!("expected windows")
        };
        assert_eq!(windows[0].kind, WindowKind::FiveHour);
        assert_eq!(windows[0].used_percent, 12.5);
        assert_eq!(windows[0].resets_at, Some(1_790_003_600));
        assert_eq!(windows[1].kind, WindowKind::Weekly);
        assert_eq!(windows[1].window_length, Some(7 * 86_400));
    }

    #[test]
    fn a_backend_answer_that_is_not_understood_is_a_parse_error() {
        for body in [
            "nope",
            "{}",
            r#"{"plan_type":"plus"}"#,
            r#"{"plan_type":"plus","rate_limit":{"primary_window":null}}"#,
            r#"{"rate_limit":{"primary_window":{"used_percent":1}}}"#,
        ] {
            assert_eq!(
                parse_backend(body, "local", 1).state,
                State::Unknown {
                    reason: Reason::ParseError
                },
                "{body}"
            );
        }
    }

    #[test]
    fn the_credential_is_the_account_token_and_an_api_key_login_has_none() {
        let json =
            r#"{"tokens":{"access_token":" tok ","account_id":"acct"},"OPENAI_API_KEY":null}"#;
        let credential = parse_credential(json).unwrap();
        assert_eq!(credential.token.expose(), "tok");
        assert_eq!(credential.account_id.as_deref(), Some("acct"));
        assert!(parse_credential(r#"{"OPENAI_API_KEY":"sk-x"}"#).is_none());
        assert!(parse_credential(r#"{"tokens":{"access_token":""}}"#).is_none());
        assert!(parse_credential("nope").is_none());
    }
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

    fn minutes_of(body: &str) -> Vec<(WindowKind, f64)> {
        let State::Known { windows } = parse_backend(body, "local", 1).state else {
            panic!("expected windows")
        };
        windows
            .into_iter()
            .map(|w| (w.kind, w.used_percent))
            .collect()
    }

    fn backend(primary: Option<i64>, secondary: Option<i64>) -> String {
        let w = |label: &str, seconds: Option<i64>, used: u32| {
            seconds.map_or(String::new(), |s| {
                format!(r#""{label}":{{"used_percent":{used},"limit_window_seconds":{s},"reset_at":1790003600}},"#)
            })
        };
        format!(
            r#"{{"plan_type":"plus","rate_limit":{{{}{}"x":null}}}}"#,
            w("primary_window", primary, 11),
            w("secondary_window", secondary, 22)
        )
    }

    #[test]
    fn windows_are_told_apart_by_length_within_a_minute_else_by_position() {
        use WindowKind::{FiveHour, Weekly};
        for (primary, secondary, expected) in [
            // Exact.
            (
                Some(300 * 60),
                Some(10_080 * 60),
                vec![(FiveHour, 11.0), (Weekly, 22.0)],
            ),
            // One minute of drift either way is still the same window.
            (
                Some(301 * 60),
                Some(10_079 * 60),
                vec![(FiveHour, 11.0), (Weekly, 22.0)],
            ),
            (
                Some(299 * 60),
                Some(10_081 * 60),
                vec![(FiveHour, 11.0), (Weekly, 22.0)],
            ),
            // Swapped: the length decides, not the position.
            (
                Some(10_080 * 60),
                Some(300 * 60),
                vec![(FiveHour, 22.0), (Weekly, 11.0)],
            ),
            // Unknown lengths: primary is the session, secondary the week.
            (
                Some(120 * 60),
                Some(1440 * 60),
                vec![(FiveHour, 11.0), (Weekly, 22.0)],
            ),
            // 303 minutes is outside the tolerance: unknown, so positional.
            (Some(303 * 60), None, vec![(FiveHour, 11.0)]),
            // Only a weekly window.
            (Some(10_080 * 60), None, vec![(Weekly, 11.0)]),
            // A second window of a taken kind is dropped.
            (Some(300 * 60), Some(300 * 60), vec![(FiveHour, 11.0)]),
        ] {
            assert_eq!(
                minutes_of(&backend(primary, secondary)),
                expected,
                "{primary:?} {secondary:?}"
            );
        }
    }

    #[test]
    fn the_session_log_follows_the_same_rule() {
        let text = line(
            "2026-10-04T18:59:06.753Z",
            "codex",
            Some((10.0, 301, 1_791_154_068)),
            Some((20.0, 777, 1_791_678_919)),
        );
        let got = parse_rollout_lines(&text, "local");
        let State::Known { windows } = &got.usage.state else {
            panic!()
        };
        assert_eq!(windows[0].kind, WindowKind::FiveHour);
        assert_eq!(windows[1].kind, WindowKind::Weekly);
    }

    #[test]
    fn an_api_key_login_is_told_from_an_empty_file() {
        assert!(is_api_key_login(r#"{"OPENAI_API_KEY":"sk-x"}"#));
        assert!(!is_api_key_login(
            r#"{"OPENAI_API_KEY":"sk-x","tokens":{"access_token":"t"}}"#
        ));
        assert!(!is_api_key_login(r#"{"OPENAI_API_KEY":null}"#));
        assert!(!is_api_key_login("nope"));
    }
}

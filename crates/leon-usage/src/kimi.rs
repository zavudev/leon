//! Kimi Code: the managed usage endpoint.
//!
//! **Implemented from Orca's reference (`src/main/rate-limits/kimi-fetcher.ts`),
//! unverified against the live service.** The Kimi Code CLI keeps its OAuth
//! credential in `<kimi home>/credentials/kimi-code.json` (the home is
//! `$KIMI_CODE_HOME` or `~/.kimi-code`): `access_token` and `expires_at` in
//! Unix seconds. The CLI refreshes the token itself; Leon never does, because
//! a rotated refresh token would sign a running `kimi` out. The token goes as
//! a bearer credential to `api.kimi.com` (`/coding/v1/usages`, the call the
//! CLI's own `/usage` makes) and nowhere else.

use leon_core::AgentId;
use serde_json::Value;

use crate::codex::kind_for_minutes;
use crate::model::{AgentUsage, Reason, Source, State, UsageWindow, WindowKind, DAY, MINUTE};
use crate::secret::Secret;

/// The host the token may be sent to, and nowhere else.
pub const HOST: &str = "api.kimi.com";
/// The usage endpoint.
pub const URL: &str = "https://api.kimi.com/coding/v1/usages";

/// A token that expires within this many seconds is not used.
const SKEW: i64 = 5;

/// What the credential file held.
#[derive(Debug)]
pub struct Credential {
    /// The access token.
    pub token: Secret,
    /// Whether it has expired: the CLI refreshes it on its next run.
    pub expired: bool,
}

/// Reads `kimi-code.json`. A token with no expiry is expired: the file is not
/// one the CLI wrote.
pub fn parse_credential(json: &str, now: i64) -> Option<Credential> {
    let value: Value = serde_json::from_str(json).ok()?;
    let token = value.get("access_token")?.as_str()?;
    if token.is_empty() {
        return None;
    }
    let expires = value.get("expires_at").and_then(Value::as_i64);
    Some(Credential {
        token: Secret::new(token),
        expired: expires.is_none_or(|at| at - now <= SKEW),
    })
}

fn int(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

fn window_of(detail: Option<&Value>, kind: WindowKind, length: i64) -> Option<UsageWindow> {
    let detail = detail?;
    let limit = int(detail.get("limit"))?;
    let used = int(detail.get("used")).or_else(|| Some(limit - int(detail.get("remaining"))?))?;
    if limit <= 0.0 {
        return None;
    }
    let reset = ["resetTime", "resetAt"]
        .iter()
        .find_map(|key| detail.get(*key).and_then(crate::claude::parse_reset));
    Some(UsageWindow {
        kind,
        used_percent: (used / limit * 100.0).clamp(0.0, 100.0),
        resets_at: reset,
        window_length: Some(length),
    })
}

fn minutes(window: Option<&Value>) -> Option<i64> {
    let window = window?;
    let duration = int(window.get("duration"))? as i64;
    let unit = window
        .get("timeUnit")
        .and_then(Value::as_str)?
        .to_uppercase();
    Some(if unit.contains("MINUTE") {
        duration
    } else if unit.contains("HOUR") {
        duration * 60
    } else if unit.contains("DAY") {
        duration * 60 * 24
    } else if unit.contains("SECOND") {
        duration / 60
    } else {
        duration
    })
}

/// Parses the answer: the top-level `usage` block is the weekly quota, and
/// the entry of `limits` closest to five hours is the session. An answer with
/// neither is a [`Reason::ParseError`].
pub fn parse_usage(body: &str, machine: &str, now: i64) -> AgentUsage {
    let fail = || AgentUsage::unknown(AgentId::KIMI, machine, Reason::ParseError);
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return fail();
    };
    let mut windows = Vec::new();
    let mut session: Option<(i64, UsageWindow)> = None;
    for limit in value
        .get("limits")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let length = minutes(limit.get("window")).unwrap_or(300);
        let Some(window) = window_of(
            limit.get("detail"),
            kind_for_minutes(length),
            length * MINUTE,
        ) else {
            continue;
        };
        if session
            .as_ref()
            .is_none_or(|(best, _)| (length - 300).abs() < (best - 300).abs())
        {
            session = Some((length, window));
        }
    }
    windows.extend(session.map(|(_, window)| window));
    windows.extend(window_of(value.get("usage"), WindowKind::Weekly, 7 * DAY));
    if windows.is_empty() {
        return fail();
    }
    AgentUsage {
        agent: AgentId::KIMI,
        machine: machine.to_owned(),
        account_label: None,
        plan: None,
        source: Some(Source::VendorApi),
        observed_at: Some(now),
        state: State::Known { windows },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    #[test]
    fn a_fresh_token_is_used_and_an_expiring_one_is_not() {
        let fresh = format!(r#"{{"access_token":"tok","expires_at":{}}}"#, NOW + 900);
        let credential = parse_credential(&fresh, NOW).unwrap();
        assert_eq!(credential.token.expose(), "tok");
        assert!(!credential.expired);
        let soon = format!(r#"{{"access_token":"tok","expires_at":{}}}"#, NOW + 3);
        assert!(parse_credential(&soon, NOW).unwrap().expired);
        assert!(
            parse_credential(r#"{"access_token":"tok"}"#, NOW)
                .unwrap()
                .expired
        );
        assert!(parse_credential(r#"{"access_token":""}"#, NOW).is_none());
        assert!(parse_credential("nope", NOW).is_none());
    }

    const BODY: &str = r#"{"usage":{"limit":"100","remaining":"40","resetTime":"2026-10-12T00:00:00Z"},"limits":[{"window":{"duration":300,"timeUnit":"MINUTE"},"detail":{"limit":50,"used":10,"resetAt":"2026-10-05T20:00:00Z"}},{"window":{"duration":1,"timeUnit":"DAY"},"detail":{"limit":10,"used":9}}]}"#;

    #[test]
    fn the_weekly_quota_and_the_window_closest_to_five_hours_are_read() {
        let usage = parse_usage(BODY, "local", NOW);
        let State::Known { windows } = usage.state else {
            panic!("expected windows")
        };
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, WindowKind::FiveHour);
        assert_eq!(windows[0].used_percent, 20.0);
        assert_eq!(windows[0].resets_at, Some(1_791_230_400));
        assert_eq!(windows[1].kind, WindowKind::Weekly);
        assert_eq!(windows[1].used_percent, 60.0);
        assert_eq!(windows[1].window_length, Some(7 * DAY));
    }

    #[test]
    fn units_and_missing_pieces_are_handled() {
        let body = r#"{"limits":[{"window":{"duration":3,"timeUnit":"TIME_UNIT_HOUR"},"detail":{"limit":4,"remaining":1}},{"detail":{"limit":0,"used":0}},{"detail":{}}]}"#;
        let usage = parse_usage(body, "local", NOW);
        let State::Known { windows } = usage.state else {
            panic!("expected windows")
        };
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].kind, WindowKind::Custom("3h".into()));
        assert_eq!(windows[0].used_percent, 75.0);
    }

    #[test]
    fn an_answer_without_quota_windows_is_a_parse_error() {
        for body in ["{}", "[]", "nope", r#"{"usage":{"limit":0}}"#] {
            assert_eq!(
                parse_usage(body, "local", NOW).state,
                State::Unknown {
                    reason: Reason::ParseError
                },
                "{body}"
            );
        }
    }

    #[test]
    fn a_reset_time_may_take_any_form_the_service_sends_and_a_bad_one_is_no_reset() {
        let body =
            |reset: &str| format!(r#"{{"usage":{{"limit":100,"used":10,"resetTime":{reset}}}}}"#);
        for (reset, at) in [
            (r#""2026-10-12T00:00:00Z""#, Some(1_791_763_200)),
            (r#""2026-10-12T00:00:00.123Z""#, Some(1_791_763_200)),
            (r#""2026-10-12T02:00:00+02:00""#, Some(1_791_763_200)),
            (r#""2026-10-12T00:00:00""#, Some(1_791_763_200)),
            ("1791763200", Some(1_791_763_200)),
            ("1791763200000", Some(1_791_763_200)),
            (r#""1791763200""#, Some(1_791_763_200)),
            (r#""next week""#, None),
            ("null", None),
        ] {
            let State::Known { windows } = parse_usage(&body(reset), "m", 1).state else {
                panic!("{reset}: a bad reset must not fail the read")
            };
            assert_eq!(windows[0].resets_at, at, "{reset}");
        }
    }
}

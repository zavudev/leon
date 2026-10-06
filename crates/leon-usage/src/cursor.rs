//! Cursor: the dashboard's usage summary.
//!
//! **Implemented from Orca's reference (`src/main/rate-limits/cursor-*.ts`),
//! unverified against the live service.** `cursor-agent` keeps its session
//! token (a JWT) in the macOS login keychain (service `cursor-access-token`,
//! account `cursor-user`), or in an `auth.json` of its configuration folder
//! on older versions and other systems. The dashboard routes
//! `cursor.com/api/usage-summary` (and the older `/api/usage`) take that token
//! as the session cookie the dashboard itself sends: the JWT's subject, a
//! separator and the JWT, URL-escaped. They check the request's origin, so the
//! two headers the dashboard sends are sent too. The cookie goes to
//! `cursor.com` and nowhere else. Cursor IDE's own session (a SQLite file) is
//! not read.
//!
//! This module holds the pure parts. The calls are in [`crate::network`].

use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use leon_core::AgentId;
use serde_json::Value;

use crate::model::{AgentUsage, Reason, Source, State, UsageWindow, WindowKind, DAY};
use crate::secret::Secret;

/// The host the cookie may be sent to, and nowhere else.
pub const HOST: &str = "cursor.com";
/// The usage summary.
pub const SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";
/// The request-quota endpoint of older plans; the user's subject follows
/// `?user=`.
pub const LEGACY_URL: &str = "https://cursor.com/api/usage";

/// The session as the dashboard wants it.
#[derive(Debug)]
pub struct Session {
    /// The cookie value: `WorkosCursorSessionToken=<subject>%3A%3A<jwt>`.
    pub cookie: Secret,
    /// The user's subject, for the older endpoint's query.
    pub subject: String,
    /// Whether the token has expired.
    pub expired: bool,
}

fn base64url(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.trim_end_matches('=').bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Percent-escapes everything but unreserved characters.
pub fn escape(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Reads a token: a JWT whose payload names the subject (`sub`) and maybe an
/// expiry (`exp`). Anything else is "not signed in".
pub fn parse_session(token: &str, now: i64) -> Option<Session> {
    let token = token.trim();
    let payload = token.split('.').nth(1).filter(|part| !part.is_empty())?;
    let payload: Value = serde_json::from_slice(&base64url(payload)?).ok()?;
    let subject = payload.get("sub")?.as_str()?.trim();
    if subject.is_empty() || token.contains(char::is_whitespace) {
        return None;
    }
    let expired = payload
        .get("exp")
        .and_then(Value::as_i64)
        .is_some_and(|exp| exp <= now);
    Some(Session {
        cookie: Secret::new(format!(
            "WorkosCursorSessionToken={}%3A%3A{token}",
            escape(subject)
        )),
        subject: subject.to_owned(),
        expired,
    })
}

/// The token of a legacy `auth.json` (`{"accessToken":".."}`).
pub fn parse_auth_file(json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json).ok()?;
    let token = value.get("accessToken")?.as_str()?;
    (!token.is_empty()).then(|| token.to_owned())
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(text) if !text.trim().is_empty() => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// A time as seconds, milliseconds or an ISO text, in Unix seconds. Cursor
/// mixes the three across its billing fields.
fn time(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => Some(scale(n.as_f64()?)),
        Value::String(text) => {
            let text = text.trim();
            if text.chars().all(|c| c.is_ascii_digit()) && !text.is_empty() {
                Some(scale(text.parse::<f64>().ok()?))
            } else {
                DateTime::parse_from_rfc3339(text)
                    .ok()
                    .map(|d| d.timestamp())
            }
        }
        _ => None,
    }
}

fn scale(n: f64) -> i64 {
    (if n > 1e12 { n / 1000.0 } else { n }) as i64
}

fn pool_percent(pool: Option<&Value>, field: &str) -> Option<f64> {
    let pool = pool?;
    match (number(pool.get("used")), number(pool.get("limit"))) {
        (Some(used), Some(limit)) if limit > 0.0 => Some(used / limit * 100.0),
        _ => number(pool.get(field)),
    }
}

/// What the usage summary says.
#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    /// The windows: the plan's total, the two pools and on-demand.
    pub windows: Vec<UsageWindow>,
    /// The membership (`pro`, `free`).
    pub plan: Option<String>,
    /// Whether the plan has no ceiling.
    pub unlimited: bool,
}

/// Reads `usage-summary`. A shape that is not understood has no windows.
pub fn parse_summary(body: &str) -> Option<Summary> {
    let value: Value = serde_json::from_str(body).ok()?;
    if !value.is_object() {
        return None;
    }
    let start = time(value.get("billingCycleStart"));
    let end = time(value.get("billingCycleEnd"));
    let length = match (start, end) {
        (Some(a), Some(b)) if b > a => b - a,
        _ => 30 * DAY,
    };
    let window = |kind: WindowKind, percent: f64| UsageWindow {
        kind,
        used_percent: percent.clamp(0.0, 100.0),
        resets_at: end,
        window_length: Some(length),
    };
    let usage = value.get("individualUsage").filter(|u| u.is_object());
    let plan = usage.and_then(|u| u.get("plan")).filter(|p| p.is_object());
    // A team-billed account reports pools it does not own, at zero: they are
    // not meters.
    let enabled = plan.is_some_and(|p| p.get("enabled").and_then(Value::as_bool) != Some(false));
    let mut windows = Vec::new();
    let total = if enabled {
        number(plan.and_then(|p| p.get("totalPercentUsed")))
            .or_else(|| pool_percent(plan, "totalPercentUsed"))
    } else {
        None
    };
    if let Some(percent) = total {
        windows.push(window(WindowKind::Monthly, percent));
    }
    if enabled {
        for (field, name) in [
            ("autoPercentUsed", "Cursor Models"),
            ("apiPercentUsed", "Other Models"),
        ] {
            if let Some(percent) = number(plan.and_then(|p| p.get(field))) {
                windows.push(window(WindowKind::Custom(name.into()), percent));
            }
        }
    }
    let on_demand = usage
        .and_then(|u| u.get("onDemand"))
        .filter(|p| p.is_object());
    if on_demand.is_some_and(|p| p.get("enabled").and_then(Value::as_bool) == Some(true)) {
        if let Some(percent) = pool_percent(on_demand, "totalPercentUsed") {
            windows.push(window(WindowKind::Custom("On-demand".into()), percent));
        }
    }
    Some(Summary {
        windows,
        plan: value
            .get("membershipType")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|plan| !plan.is_empty())
            .map(str::to_owned),
        unlimited: value.get("isUnlimited").and_then(Value::as_bool) == Some(true),
    })
}

/// One month after `at` (Unix seconds), the day clamped to the month's end:
/// a cycle that began on the 31st ends on the last day of the next month.
fn add_month(at: i64) -> Option<i64> {
    let start = Utc.timestamp_opt(at, 0).single()?;
    let (year, month) = if start.month() == 12 {
        (start.year() + 1, 1)
    } else {
        (start.year(), start.month() + 1)
    };
    let mut day = start.day();
    loop {
        if let Some(next) = Utc
            .with_ymd_and_hms(
                year,
                month,
                day,
                start.hour(),
                start.minute(),
                start.second(),
            )
            .single()
        {
            return Some(next.timestamp());
        }
        day -= 1;
        if day == 0 {
            return None;
        }
    }
}

/// Reads the older request-quota answer: per-model `numRequests` and
/// `maxRequestUsage`, and `startOfMonth`. The premium model's bucket (`gpt-4`)
/// is the headline, else the largest ceiling.
pub fn parse_legacy(body: &str) -> Option<UsageWindow> {
    let value: Value = serde_json::from_str(body).ok()?;
    let object = value.as_object()?;
    let mut quotas: Vec<(&str, f64, f64)> = object
        .iter()
        .filter_map(|(name, bucket)| {
            let limit = number(bucket.get("maxRequestUsage"))?;
            let used = number(bucket.get("numRequests"))?;
            (limit > 0.0).then_some((name.as_str(), used, limit))
        })
        .collect();
    quotas.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(b.0)));
    let (_, used, limit) = quotas
        .iter()
        .find(|(name, ..)| *name == "gpt-4")
        .or(quotas.first())
        .copied()?;
    Some(UsageWindow {
        kind: WindowKind::Monthly,
        used_percent: (used / limit * 100.0).clamp(0.0, 100.0),
        resets_at: time(object.get("startOfMonth")).and_then(add_month),
        window_length: Some(30 * DAY),
    })
}

/// The reading of what the summary said (or the older endpoint added).
pub fn reading(
    summary: &Summary,
    extra: Option<UsageWindow>,
    machine: &str,
    now: i64,
) -> AgentUsage {
    let mut windows = summary.windows.clone();
    windows.extend(extra);
    let unknown = |reason| {
        let mut usage = AgentUsage::unknown(AgentId::CURSOR, machine, reason);
        usage.plan = summary.plan.clone();
        usage
    };
    if windows.is_empty() {
        return unknown(if summary.unlimited {
            Reason::Unlimited
        } else {
            Reason::NoData
        });
    }
    AgentUsage {
        agent: AgentId::CURSOR,
        machine: machine.to_owned(),
        account_label: summary.plan.clone(),
        plan: summary.plan.clone(),
        source: Some(Source::VendorApi),
        observed_at: Some(now),
        state: State::Known { windows },
    }
}

/// A token of the right shape for synthetic data: `{"sub":"auth0|user_x",
/// "exp":4102444800}` as the payload.
#[cfg(test)]
pub(crate) fn test_token(payload: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in payload.as_bytes().chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
        }
    }
    format!("e30.{out}.sig")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    #[test]
    fn the_session_cookie_is_the_escaped_subject_then_the_token() {
        let jwt = test_token(r#"{"sub":"auth0|user_x","exp":4102444800}"#);
        let session = parse_session(&jwt, NOW).unwrap();
        assert_eq!(session.subject, "auth0|user_x");
        assert!(!session.expired);
        assert_eq!(
            session.cookie.expose(),
            format!("WorkosCursorSessionToken=auth0%7Cuser_x%3A%3A{jwt}")
        );
    }

    #[test]
    fn an_expired_token_says_so_and_a_malformed_one_is_signed_out() {
        let old = test_token(r#"{"sub":"u","exp":1000}"#);
        assert!(parse_session(&old, NOW).unwrap().expired);
        let no_exp = test_token(r#"{"sub":"u"}"#);
        assert!(!parse_session(&no_exp, NOW).unwrap().expired);
        for bad in [
            "",
            "abc",
            "a.b.c",
            "e30..sig",
            &test_token(r#"{"sub":""}"#),
            &test_token("{}"),
        ] {
            assert!(parse_session(bad, NOW).is_none(), "{bad}");
        }
    }

    #[test]
    fn the_legacy_auth_file_gives_the_token() {
        assert_eq!(
            parse_auth_file(r#"{"accessToken":"t"}"#).as_deref(),
            Some("t")
        );
        assert!(parse_auth_file(r#"{"accessToken":""}"#).is_none());
        assert!(parse_auth_file("[]").is_none());
    }

    const SUMMARY: &str = r#"{"billingCycleStart":"2026-10-01T00:00:00Z","billingCycleEnd":"2026-11-01T00:00:00Z","membershipType":"pro","isUnlimited":false,"individualUsage":{"plan":{"enabled":true,"used":1000,"limit":2000,"totalPercentUsed":50,"autoPercentUsed":20.5,"apiPercentUsed":80},"onDemand":{"enabled":true,"used":100,"limit":1000}}}"#;

    #[test]
    fn the_summary_gives_the_plan_total_the_pools_and_on_demand() {
        let summary = parse_summary(SUMMARY).unwrap();
        let kinds: Vec<(String, f64)> = summary
            .windows
            .iter()
            .map(|w| (w.kind.long(), w.used_percent))
            .collect();
        assert_eq!(
            kinds,
            [
                ("Monthly".to_owned(), 50.0),
                ("Cursor Models".to_owned(), 20.5),
                ("Other Models".to_owned(), 80.0),
                ("On-demand".to_owned(), 10.0),
            ]
        );
        assert_eq!(summary.windows[0].resets_at, Some(1_793_491_200));
        assert_eq!(summary.windows[0].window_length, Some(31 * DAY));
        assert_eq!(summary.plan.as_deref(), Some("pro"));
        let usage = reading(&summary, None, "local", NOW);
        assert_eq!(usage.plan.as_deref(), Some("pro"));
    }

    #[test]
    fn nulls_and_mixed_time_units_do_not_spoil_the_rest() {
        let body = r#"{"billingCycleStart":1759276800,"billingCycleEnd":1761955200000,"individualUsage":{"plan":{"totalPercentUsed":"12"},"onDemand":null}}"#;
        let summary = parse_summary(body).unwrap();
        assert_eq!(summary.windows.len(), 1);
        assert_eq!(summary.windows[0].used_percent, 12.0);
        assert_eq!(summary.windows[0].resets_at, Some(1_761_955_200));
    }

    #[test]
    fn a_team_billed_plan_reports_no_pools_of_its_own() {
        let body = r#"{"individualUsage":{"plan":{"enabled":false,"totalPercentUsed":0,"autoPercentUsed":0},"onDemand":{"enabled":false}}}"#;
        let summary = parse_summary(body).unwrap();
        assert!(summary.windows.is_empty());
        assert_eq!(
            reading(&summary, None, "local", NOW).state,
            State::Unknown {
                reason: Reason::NoData
            }
        );
    }

    #[test]
    fn an_unlimited_plan_says_there_is_no_limit() {
        let summary = parse_summary(r#"{"isUnlimited":true,"membershipType":"ultra"}"#).unwrap();
        let usage = reading(&summary, None, "local", NOW);
        assert_eq!(
            usage.state,
            State::Unknown {
                reason: Reason::Unlimited
            }
        );
        assert_eq!(usage.plan.as_deref(), Some("ultra"));
    }

    #[test]
    fn a_body_that_is_not_an_object_is_not_a_summary() {
        assert!(parse_summary("[]").is_none());
        assert!(parse_summary("nope").is_none());
    }

    #[test]
    fn the_legacy_quota_prefers_the_premium_bucket_then_the_largest() {
        let body = r#"{"gpt-4":{"numRequests":100,"maxRequestUsage":500},"gpt-3.5-turbo":{"numRequests":5,"maxRequestUsage":10000},"startOfMonth":"2026-01-31T00:00:00Z"}"#;
        let window = parse_legacy(body).unwrap();
        assert_eq!(window.used_percent, 20.0);
        // The 31st clamps to the end of February.
        assert_eq!(
            DateTime::from_timestamp(window.resets_at.unwrap(), 0)
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2026-02-28"
        );
        let no_premium = r#"{"a":{"numRequests":1,"maxRequestUsage":10},"b":{"numRequests":50,"maxRequestUsage":100}}"#;
        assert_eq!(parse_legacy(no_premium).unwrap().used_percent, 50.0);
        assert!(parse_legacy(r#"{"a":{"numRequests":1,"maxRequestUsage":0}}"#).is_none());
        assert!(parse_legacy("[]").is_none());
    }

    #[test]
    fn the_legacy_window_is_added_to_a_summary_without_windows() {
        let summary = parse_summary(r#"{"membershipType":"free_trial"}"#).unwrap();
        let extra = parse_legacy(r#"{"gpt-4":{"numRequests":1,"maxRequestUsage":4}}"#);
        let usage = reading(&summary, extra, "local", NOW);
        assert!(matches!(usage.state, State::Known { .. }));
    }
}

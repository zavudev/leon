//! opencode: the limits of the opencode Go subscription.
//!
//! opencode itself keeps no limits on disk, and the providers it fronts have
//! none to read. Only the opencode Go subscription has a usage endpoint, called
//! with the API key opencode stored when its owner connected the account
//! (`auth.json`, entry `opencode-go`). That reads a credential, so it is an
//! opt-in source; this module holds the pure parts.

use leon_core::AgentKind;
use serde_json::Value;

use crate::claude::parse_reset;
use crate::model::{AgentUsage, Reason, Source, State, UsageWindow, WindowKind, DAY, HOUR};
use crate::secret::Secret;

/// The host the key may be sent to, and nowhere else.
pub const HOST: &str = "opencode.ai";
/// The usage endpoint.
pub const URL: &str = "https://opencode.ai/zen/go/v1/usage";

/// The key stored for the Go subscription, from `auth.json`
/// (`{"opencode-go":{"type":"api","key":".."}}`).
pub fn parse_key(json: &str) -> Option<Secret> {
    let value: Value = serde_json::from_str(json).ok()?;
    let entry = value.get("opencode-go")?;
    if entry.get("type").and_then(Value::as_str) != Some("api") {
        return None;
    }
    let key = entry.get("key")?.as_str()?.trim();
    (!key.is_empty()).then(|| Secret::new(key))
}

fn meter(kind: WindowKind, raw: Option<&Value>, length: i64) -> Option<UsageWindow> {
    let raw = raw?;
    let percent = raw.get("percent")?.as_f64().filter(|p| p.is_finite())?;
    Some(UsageWindow {
        kind,
        used_percent: percent.clamp(0.0, 100.0),
        resets_at: raw.get("resetsAt").and_then(parse_reset),
        window_length: Some(length),
    })
}

/// Parses `{"usage":{"rolling":{"percent":..,"resetsAt":..},"weekly":..,
/// "monthly":..}}`. The rolling window is the five-hour one.
pub fn parse_usage(body: &str, machine: &str, now: i64) -> AgentUsage {
    let fail = || AgentUsage::unknown(AgentKind::Opencode, machine, Reason::ParseError);
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return fail();
    };
    let Some(usage) = value.get("usage") else {
        return fail();
    };
    let windows: Vec<UsageWindow> = [
        meter(WindowKind::FiveHour, usage.get("rolling"), 5 * HOUR),
        meter(WindowKind::Weekly, usage.get("weekly"), 7 * DAY),
        meter(WindowKind::Monthly, usage.get("monthly"), 30 * DAY),
    ]
    .into_iter()
    .flatten()
    .collect();
    if windows.is_empty() {
        return fail();
    }
    AgentUsage {
        agent: AgentKind::Opencode,
        machine: machine.to_owned(),
        account_label: Some("Go".into()),
        plan: Some("Go".into()),
        source: Some(Source::VendorApi),
        observed_at: Some(now),
        state: State::Known { windows },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_go_key_is_read_from_the_auth_file() {
        let json =
            r#"{"google":{"type":"api","key":"g"},"opencode-go":{"type":"api","key":"go-key-1"}}"#;
        assert_eq!(parse_key(json).unwrap().expose(), "go-key-1");
    }

    #[test]
    fn no_go_entry_or_another_kind_is_no_key() {
        for json in [
            "{}",
            r#"{"opencode-go":{"type":"oauth","key":"x"}}"#,
            r#"{"opencode-go":{"type":"api","key":""}}"#,
            "nope",
        ] {
            assert!(parse_key(json).is_none(), "{json}");
        }
    }

    #[test]
    fn the_three_meters_become_windows() {
        let body = r#"{"usage":{
            "rolling":{"status":"ok","percent":12.5,"resetsAt":"2026-10-05T12:00:00Z"},
            "weekly":{"status":"ok","percent":40,"resetsAt":"2026-10-09T00:00:00Z"},
            "monthly":{"status":"rate-limited","percent":100,"resetsAt":"2026-11-01T00:00:00Z"}}}"#;
        let usage = parse_usage(body, "local", 1_790_000_000);
        let State::Known { windows } = &usage.state else {
            panic!("expected windows");
        };
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].kind, WindowKind::FiveHour);
        assert_eq!(windows[0].used_percent, 12.5);
        assert_eq!(windows[2].kind, WindowKind::Monthly);
        assert_eq!(windows[2].used_percent, 100.0);
    }

    #[test]
    fn an_unexpected_answer_is_a_parse_error() {
        for body in ["", "{}", r#"{"usage":{}}"#, "<html></html>"] {
            assert_eq!(
                parse_usage(body, "m", 0).state,
                State::Unknown {
                    reason: Reason::ParseError
                },
                "{body}"
            );
        }
    }
}

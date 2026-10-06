//! Claude Code: the account's usage endpoint.
//!
//! Claude Code writes no limits to disk (the status line receives them on
//! standard input only while a session runs). The one source with real numbers
//! is the account usage endpoint the CLI itself calls, with the OAuth token it
//! keeps in the macOS keychain or in `~/.claude/.credentials.json`. Calling it
//! reads that credential, so it is an opt-in source: this module holds the
//! pure parts (reading the credential file, parsing the answer); the call
//! lives in [`crate::network`].

use leon_core::AgentId;
use serde_json::Value;

use crate::model::{AgentUsage, Reason, Source, State, UsageWindow, WindowKind, DAY, HOUR};
use crate::secret::Secret;

/// The host the credential may be sent to, and nowhere else.
pub const HOST: &str = "api.anthropic.com";
/// The usage endpoint.
pub const URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// What the credential store held: the token and the plan, nothing else.
#[derive(Debug)]
pub struct Credential {
    /// The OAuth access token.
    pub token: Secret,
    /// The plan the account is on, when the store says.
    pub plan: Option<String>,
}

/// Reads the JSON the CLI keeps (`{"claudeAiOauth":{"accessToken":..,
/// "subscriptionType":..}}`). Returns `None` for anything else, so a malformed
/// file is "not signed in" and never an error that could quote it.
pub fn parse_credential(json: &str) -> Option<Credential> {
    let value: Value = serde_json::from_str(json).ok()?;
    let oauth = value.get("claudeAiOauth")?;
    let token = oauth.get("accessToken")?.as_str()?.trim();
    if token.is_empty() {
        return None;
    }
    Some(Credential {
        token: Secret::new(token),
        plan: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// A reset time given as seconds, milliseconds or an ISO 8601 text, in Unix
/// seconds.
pub fn parse_reset(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => {
            let n = n.as_f64()?;
            Some(if n > 1e10 {
                (n / 1000.0) as i64
            } else {
                n as i64
            })
        }
        Value::String(text) => chrono::DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|d| d.timestamp())
            .or_else(|| text.trim().parse::<i64>().ok().filter(|n| *n > 0)),
        _ => None,
    }
}

fn percent_of(raw: &Value) -> Option<f64> {
    ["utilization", "used_percentage", "percent"]
        .iter()
        .find_map(|key| raw.get(*key).and_then(Value::as_f64))
        .filter(|p| p.is_finite())
        .map(|p| p.clamp(0.0, 100.0))
}

fn window(kind: WindowKind, raw: &Value, length: i64) -> Option<UsageWindow> {
    if !raw.is_object() {
        return None;
    }
    Some(UsageWindow {
        kind,
        used_percent: percent_of(raw)?,
        resets_at: raw.get("resets_at").and_then(parse_reset),
        window_length: Some(length),
    })
}

fn title(model: &str) -> String {
    let mut chars = model.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// Parses the endpoint's answer: `five_hour`, `seven_day`, per-model weekly
/// buckets (`seven_day_<model>` keys and `limits` entries of kind
/// `weekly_scoped`). An answer with no five-hour and no weekly window is a
/// [`Reason::ParseError`].
pub fn parse_usage(body: &str, machine: &str, plan: Option<String>, now: i64) -> AgentUsage {
    let fail = || AgentUsage::unknown(AgentId::CLAUDE, machine, Reason::ParseError);
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return fail();
    };
    let Some(object) = value.as_object() else {
        return fail();
    };
    let mut windows = Vec::new();
    if let Some(w) = object
        .get("five_hour")
        .and_then(|r| window(WindowKind::FiveHour, r, 5 * HOUR))
    {
        windows.push(w);
    }
    if let Some(w) = object
        .get("seven_day")
        .and_then(|r| window(WindowKind::Weekly, r, 7 * DAY))
    {
        windows.push(w);
    }
    if windows.is_empty() {
        return fail();
    }
    let mut keys: Vec<&String> = object.keys().collect();
    keys.sort();
    for key in keys {
        let model = key
            .strip_prefix("seven_day_")
            .or_else(|| key.strip_suffix("_seven_day"))
            .or_else(|| key.strip_suffix("_weekly"))
            .or_else(|| key.strip_prefix("weekly_"));
        if let Some(model) = model {
            let name = title(model);
            if let Some(w) = window(WindowKind::ModelWeekly(name), &object[key], 7 * DAY) {
                push_model(&mut windows, w);
            }
        }
    }
    if let Some(limits) = object.get("limits").and_then(Value::as_array) {
        for limit in limits {
            if limit.get("kind").and_then(Value::as_str) != Some("weekly_scoped") {
                continue;
            }
            let Some(name) = limit
                .pointer("/scope/model/display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|n| !n.is_empty())
            else {
                continue;
            };
            if let Some(w) = window(WindowKind::ModelWeekly(name.to_owned()), limit, 7 * DAY) {
                push_model(&mut windows, w);
            }
        }
    }
    AgentUsage {
        agent: AgentId::CLAUDE,
        machine: machine.to_owned(),
        account_label: plan.clone(),
        plan,
        source: Some(Source::VendorApi),
        observed_at: Some(now),
        state: State::Known { windows },
    }
}

fn push_model(windows: &mut Vec<UsageWindow>, new: UsageWindow) {
    let same = |existing: &UsageWindow| existing.kind.long().eq_ignore_ascii_case(&new.kind.long());
    if !windows.iter().any(same) {
        windows.push(new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    #[test]
    fn the_credential_file_gives_a_token_and_the_plan() {
        let json = r#"{"claudeAiOauth":{"accessToken":"tok-123","refreshToken":"r","expiresAt":1,"subscriptionType":"max"}}"#;
        let credential = parse_credential(json).unwrap();
        assert_eq!(credential.token.expose(), "tok-123");
        assert_eq!(credential.plan.as_deref(), Some("max"));
    }

    #[test]
    fn a_credential_never_appears_in_debug_output() {
        let json = r#"{"claudeAiOauth":{"accessToken":"tok-123-secret"}}"#;
        let text = format!("{:?}", parse_credential(json).unwrap());
        assert!(!text.contains("tok-123-secret"));
    }

    #[test]
    fn anything_else_is_not_a_credential() {
        for json in [
            "",
            "{}",
            "[]",
            r#"{"claudeAiOauth":{}}"#,
            r#"{"claudeAiOauth":{"accessToken":" "}}"#,
            "not json",
        ] {
            assert!(parse_credential(json).is_none(), "{json}");
        }
    }

    #[test]
    fn the_windows_and_the_model_bucket_are_read() {
        let body = r#"{
            "five_hour": {"utilization": 10.0, "resets_at": "2026-10-05T12:00:00+00:00"},
            "seven_day": {"utilization": 91, "resets_at": 1791300000},
            "seven_day_opus": null,
            "limits": [{"kind":"weekly_scoped","percent":0,"resets_at":"2026-10-12T00:00:00Z","scope":{"model":{"display_name":"Fable"}}}]
        }"#;
        let usage = parse_usage(body, "local", Some("max".into()), NOW);
        let State::Known { windows } = &usage.state else {
            panic!("expected windows");
        };
        let kinds: Vec<_> = windows.iter().map(|w| w.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                WindowKind::FiveHour,
                WindowKind::Weekly,
                WindowKind::ModelWeekly("Fable".into())
            ]
        );
        assert_eq!(windows[0].used_percent, 10.0);
        assert_eq!(windows[0].window_length, Some(5 * 3600));
        assert_eq!(windows[1].resets_at, Some(1_791_300_000));
        assert_eq!(usage.source, Some(Source::VendorApi));
        assert_eq!(usage.observed_at, Some(NOW));
    }

    #[test]
    fn a_model_bucket_may_be_a_top_level_key() {
        let body = r#"{"five_hour":{"utilization":1},"seven_day":{"utilization":2},"seven_day_sonnet":{"utilization":33,"resets_at":1791300000}}"#;
        let usage = parse_usage(body, "m", None, NOW);
        let State::Known { windows } = &usage.state else {
            panic!()
        };
        assert_eq!(windows[2].kind, WindowKind::ModelWeekly("Sonnet".into()));
        assert_eq!(windows[2].used_percent, 33.0);
    }

    #[test]
    fn an_answer_without_the_main_windows_is_a_parse_error() {
        for body in [
            "",
            "[]",
            "{}",
            r#"{"error":{"type":"x"}}"#,
            "<html>sign in</html>",
        ] {
            let usage = parse_usage(body, "m", None, NOW);
            assert_eq!(
                usage.state,
                State::Unknown {
                    reason: Reason::ParseError
                },
                "{body}"
            );
        }
    }

    #[test]
    fn reset_times_come_in_three_shapes() {
        assert_eq!(
            parse_reset(&serde_json::json!(1_791_300_000)),
            Some(1_791_300_000)
        );
        assert_eq!(
            parse_reset(&serde_json::json!(1_791_300_000_000_i64)),
            Some(1_791_300_000)
        );
        assert_eq!(
            parse_reset(&serde_json::json!("2026-10-05T12:00:00Z")),
            Some(1_791_201_600)
        );
        assert_eq!(parse_reset(&serde_json::json!("soon")), None);
        assert_eq!(parse_reset(&serde_json::json!(null)), None);
    }
}

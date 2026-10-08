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

/// The keychain service the CLI keeps its credential under.
pub const SERVICE: &str = "Claude Code-credentials";

/// The keychain service when `CLAUDE_CONFIG_DIR` is set: the CLI scopes its
/// item by the first eight hex digits of the SHA-256 of that folder. (Orca
/// normalises the text to NFC first; Leon takes the folder as the environment
/// gives it.)
pub fn scoped_service(config_dir: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(config_dir.as_bytes());
    let suffix: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{SERVICE}-{suffix}")
}

/// What the credential store held: the token and the plan, nothing else.
#[derive(Debug)]
pub struct Credential {
    /// The OAuth access token.
    pub token: Secret,
    /// The plan the account is on, when the store says.
    pub plan: Option<String>,
}

/// Whether the store holds a refresh token but no access token: the sign-in
/// exists and only the CLI can renew it.
pub fn refresh_only(json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return false;
    };
    let Some(oauth) = value.get("claudeAiOauth") else {
        return false;
    };
    let has = |key: &str| {
        oauth
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty())
    };
    has("refreshToken") && !has("accessToken")
}

/// Whether a 403 answer says the sign-in lacks the scope usage needs
/// (`user:profile`), as opposed to a token that is stale.
pub fn lacks_scope(body: &str) -> bool {
    body.contains("user:profile")
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

/// Unix seconds from a count that is seconds or, past ten digits,
/// milliseconds.
fn epoch(n: f64) -> Option<i64> {
    (n.is_finite() && n > 0.0).then(|| {
        if n > 1e10 {
            (n / 1000.0) as i64
        } else {
            n as i64
        }
    })
}

/// A reset time as the services send it, in Unix seconds: a number of seconds
/// or milliseconds (as a number or as text), RFC 3339 with or without fractional
/// seconds and with any offset, or a date and time with no offset, which is
/// read as UTC. These are the forms JavaScript's `new Date()` takes that the
/// services use; anything else is `None`, which is a window with no reset time
/// and never a failed read.
pub fn parse_reset(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => epoch(n.as_f64()?),
        Value::String(text) => {
            let text = text.trim();
            if let Ok(at) = chrono::DateTime::parse_from_rfc3339(text) {
                return Some(at.timestamp());
            }
            if let Ok(n) = text.parse::<f64>() {
                return epoch(n);
            }
            ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
                .iter()
                .find_map(|format| chrono::NaiveDateTime::parse_from_str(text, format).ok())
                .map(|at| at.and_utc().timestamp())
        }
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

/// The top-level keys that carry a per-model weekly window, with the model's
/// name: a short explicit list, so that an unrelated `seven_day_*` key is never
/// shown as a model.
const KNOWN_MODEL_KEYS: &[(&str, &str)] = &[
    ("seven_day_opus", "Opus"),
    ("seven_day_sonnet", "Sonnet"),
    ("fable_weekly", "Fable"),
    ("fable_seven_day", "Fable"),
    ("seven_day_fable", "Fable"),
];

/// Parses the endpoint's answer: `five_hour`, `seven_day`, per-model weekly
/// buckets (`limits` entries of kind `weekly_scoped` with their
/// display name, and the known top-level model keys). An answer with no five-hour and no weekly window is a
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
    // Per-model windows: the `limits` entries first (they carry the display
    // name), then the few top-level keys the service is known to use. Any
    // other `seven_day_*` key is an unrelated limit, never a model.
    let mut model_windows = Vec::new();
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
                push_model(&mut model_windows, w);
            }
        }
    }
    for (key, name) in KNOWN_MODEL_KEYS {
        if let Some(w) = object
            .get(*key)
            .and_then(|raw| window(WindowKind::ModelWeekly((*name).to_owned()), raw, 7 * DAY))
        {
            push_model(&mut model_windows, w);
        }
    }
    windows.extend(model_windows);
    AgentUsage {
        agent: AgentId::CLAUDE,
        machine: machine.to_owned(),
        account_label: plan.clone(),
        account: None,
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
        // The other forms `new Date()` takes: fractions, offsets, no offset
        // (read as UTC), epoch text.
        for (text, at) in [
            ("2026-10-05T12:00:00.250Z", 1_791_201_600),
            ("2026-10-05T14:00:00+02:00", 1_791_201_600),
            ("2026-10-05T12:00:00", 1_791_201_600),
            ("2026-10-05 12:00:00", 1_791_201_600),
            ("1791201600", 1_791_201_600),
            ("1791201600000", 1_791_201_600),
            ("1791201600.5", 1_791_201_600),
        ] {
            assert_eq!(parse_reset(&serde_json::json!(text)), Some(at), "{text}");
        }
        assert_eq!(parse_reset(&serde_json::json!("0")), None);
        assert_eq!(parse_reset(&serde_json::json!(-5)), None);
        assert_eq!(parse_reset(&serde_json::json!(null)), None);
    }

    #[test]
    fn an_unrelated_weekly_key_is_never_shown_as_a_model() {
        let body = r#"{"five_hour":{"utilization":1},"seven_day":{"utilization":2},
            "seven_day_oauth_apps":{"utilization":77},"seven_day_cowork":{"utilization":88},
            "extra_weekly":{"utilization":66},"weekly_credits":{"utilization":55}}"#;
        let State::Known { windows } = parse_usage(body, "m", None, NOW).state else {
            panic!()
        };
        assert_eq!(windows.len(), 2, "{windows:?}");
    }

    #[test]
    fn the_known_model_keys_and_the_scoped_limits_give_models() {
        let body = r#"{"five_hour":{"utilization":1},"seven_day":{"utilization":2},
            "seven_day_opus":{"utilization":10},"seven_day_sonnet":{"utilization":20},
            "fable_weekly":{"utilization":30},"seven_day_fable":{"utilization":31},
            "limits":[{"kind":"weekly_scoped","percent":5,"scope":{"model":{"display_name":"Haiku"}}},
                      {"kind":"daily_scoped","percent":9,"scope":{"model":{"display_name":"Nope"}}}]}"#;
        let State::Known { windows } = parse_usage(body, "m", None, NOW).state else {
            panic!()
        };
        let models: Vec<(String, f64)> = windows[2..]
            .iter()
            .map(|w| (w.kind.short(), w.used_percent))
            .collect();
        assert_eq!(
            models,
            [
                ("Haiku".to_owned(), 5.0),
                ("Opus".to_owned(), 10.0),
                ("Sonnet".to_owned(), 20.0),
                ("Fable".to_owned(), 30.0),
            ],
            "the Fable aliases are one window"
        );
    }

    #[test]
    fn a_refresh_token_without_an_access_token_is_a_sign_in_only_the_cli_can_renew() {
        assert!(refresh_only(r#"{"claudeAiOauth":{"refreshToken":"r"}}"#));
        assert!(!refresh_only(
            r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r"}}"#
        ));
        assert!(!refresh_only(r#"{"claudeAiOauth":{}}"#));
        assert!(!refresh_only("nope"));
    }

    #[test]
    fn a_403_names_the_missing_scope() {
        assert!(lacks_scope(
            r#"{"error":{"message":"needs scope user:profile"}}"#
        ));
        assert!(!lacks_scope("{}"));
    }

    #[test]
    fn the_keychain_item_is_scoped_by_the_config_folder() {
        // sha256("abc") starts ba7816bf.
        assert_eq!(scoped_service("abc"), "Claude Code-credentials-ba7816bf");
        assert_eq!(SERVICE, "Claude Code-credentials");
    }
}

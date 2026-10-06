//! Grok: the billing endpoint of its CLI.
//!
//! **Implemented from Orca's reference (`src/main/rate-limits/grok-fetcher.ts`
//! and `grok-auth.ts`), unverified against the live service.** The Grok CLI
//! keeps its sign-in in `~/.grok/auth.json` (`GROK_HOME` moves it): an object
//! keyed by the issuer, each entry with the access token (`key`), the user id,
//! the expiry and more. The token is sent as a bearer credential to
//! `cli-chat-proxy.grok.com`, with the two headers the CLI sends itself, and
//! never anywhere else. The credits view answers a weekly percentage
//! (`creditUsagePercent`) or, for unified-billing accounts, a monthly budget
//! (`monthlyLimit` and `used`, money values), which the default view carries.
//!
//! This module holds the pure parts: choosing the session, reading the
//! answer. The calls are in [`crate::network`].

use leon_core::AgentId;
use serde_json::Value;

use crate::claude::parse_reset;
use crate::model::{AgentUsage, Reason, Source, State, UsageWindow, WindowKind, DAY};
use crate::secret::Secret;

/// The host the credential may be sent to, and nowhere else.
pub const HOST: &str = "cli-chat-proxy.grok.com";
/// The credits view of the billing endpoint.
pub const CREDITS_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
/// The default view, which carries the monthly budget.
pub const DEFAULT_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing";
/// The header the CLI marks its calls with; the endpoint refuses calls
/// without it.
pub const AUTH_HEADER: (&str, &str) = ("X-XAI-Token-Auth", "xai-grok-cli");

/// A token this close to its expiry is not used.
const SKEW: i64 = 5 * 60;
/// The issuer of the default sign-in.
const PREFERRED_ISSUER: &str = "https://auth.x.ai";

/// What the CLI's credential file held.
#[derive(Debug)]
pub struct Credential {
    /// The access token.
    pub token: Secret,
    /// The user id the endpoint is also told.
    pub user_id: Option<String>,
    /// Whether the token has expired (or is about to): the CLI refreshes it on
    /// its next run, Leon never does.
    pub expired: bool,
}

fn entry_credential(entry: &Value, now: i64) -> Option<Credential> {
    let token = entry.get("key")?.as_str()?;
    if token.is_empty() {
        return None;
    }
    let expires = entry
        .get("expires_at")
        .and_then(Value::as_str)
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|at| at.timestamp());
    Some(Credential {
        token: Secret::new(token),
        user_id: entry
            .get("user_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .map(str::to_owned),
        expired: expires.is_some_and(|at| at - now <= SKEW),
    })
}

fn is_preferred(key: &str) -> bool {
    key == PREFERRED_ISSUER || key.starts_with(&format!("{PREFERRED_ISSUER}::"))
}

/// Chooses the session of `auth.json`: a fresh entry of the default issuer,
/// else the expired one, else (only when there is no default issuer at all)
/// the first of another. `None` is signed out.
pub fn parse_credential(json: &str, now: i64) -> Option<Credential> {
    let value: Value = serde_json::from_str(json).ok()?;
    let object = value.as_object()?;
    let (mut preferred_seen, mut expired, mut fallback) = (false, None, None);
    for (key, entry) in object {
        let preferred = is_preferred(key);
        preferred_seen |= preferred;
        let Some(credential) = entry_credential(entry, now) else {
            continue;
        };
        if preferred {
            if !credential.expired {
                return Some(credential);
            }
            expired.get_or_insert(credential);
        } else if fallback.is_none() {
            fallback = Some(credential);
        }
    }
    expired.or(if preferred_seen { None } else { fallback })
}

/// A money amount: `{"val":"12.5"}` or `{"val":12.5}`.
fn money(value: Option<&Value>) -> Option<f64> {
    let raw = value?.get("val")?;
    match raw {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// The billing fields the endpoint answers with, under `config` or flat.
const FIELDS: [&str; 10] = [
    "creditUsagePercent",
    "currentPeriod",
    "billingPeriodStart",
    "billingPeriodEnd",
    "subscriptionTier",
    "monthlyLimit",
    "used",
    "onDemandCap",
    "onDemandUsed",
    "prepaidBalance",
];

/// What one view of the billing endpoint says.
#[derive(Debug, Default, PartialEq)]
pub struct Billing {
    /// The weekly credit window, when a percentage is reported or confirmed.
    pub weekly: Option<UsageWindow>,
    /// The monthly budget window, when a limit and an amount used are.
    pub monthly: Option<UsageWindow>,
    /// The subscription tier.
    pub tier: Option<String>,
    /// Whether any amount of money is reported (so that a missing percentage
    /// is "not reported" and not "zero").
    pub reports_amounts: bool,
}

fn parse_time(value: Option<&Value>) -> Option<i64> {
    value.and_then(parse_reset)
}

/// Reads one answer. `None` when the answer has no billing fields at all.
pub fn parse_billing(body: &str) -> Option<Billing> {
    let value: Value = serde_json::from_str(body).ok()?;
    let config = match value.get("config") {
        Some(config) if config.is_object() => config,
        _ if FIELDS.iter().any(|field| value.get(*field).is_some()) => &value,
        _ => return None,
    };
    let period = config.get("currentPeriod");
    let start = parse_time(config.get("billingPeriodStart"));
    let end = parse_time(config.get("billingPeriodEnd"));
    let period_end = parse_time(period.and_then(|p| p.get("end"))).or(end);
    let scalars = [
        money(config.get("onDemandCap")),
        money(config.get("onDemandUsed")),
        money(config.get("prepaidBalance")),
        money(config.get("monthlyLimit")),
        money(config.get("used")),
    ];
    let monthly = match (money(config.get("monthlyLimit")), money(config.get("used"))) {
        (Some(limit), Some(used)) if limit > 0.0 => Some(UsageWindow {
            kind: WindowKind::Monthly,
            used_percent: (used / limit * 100.0).clamp(0.0, 100.0),
            resets_at: period_end,
            window_length: Some(30 * DAY),
        }),
        _ => None,
    };
    let weekly_percent = match config.get("creditUsagePercent") {
        Some(Value::Number(n)) => n.as_f64().filter(|p| p.is_finite()),
        Some(_) => None,
        None => {
            let zero_cap = money(config.get("onDemandCap")) == Some(0.0);
            let unreported = if zero_cap {
                [config.get("onDemandUsed"), config.get("used")]
                    .iter()
                    .any(|v| money(*v).unwrap_or(0.0) > 0.0)
            } else {
                scalars.contains(&Some(0.0))
            };
            let confirmed = period.and_then(|p| p.get("type")).and_then(Value::as_str)
                == Some("USAGE_PERIOD_TYPE_WEEKLY")
                && start.is_some()
                && start == parse_time(period.and_then(|p| p.get("start")))
                && end.is_some()
                && end == parse_time(period.and_then(|p| p.get("end")));
            if unreported || monthly.is_some() || !confirmed {
                None
            } else {
                Some(0.0)
            }
        }
    };
    Some(Billing {
        weekly: weekly_percent.map(|percent| UsageWindow {
            kind: WindowKind::Weekly,
            used_percent: percent.clamp(0.0, 100.0),
            resets_at: period_end,
            window_length: Some(7 * DAY),
        }),
        monthly,
        tier: config
            .get("subscriptionTier")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|tier| !tier.is_empty())
            .map(str::to_owned),
        reports_amounts: scalars.iter().any(Option::is_some),
    })
}

/// The reading of what the billing endpoint said: the weekly window, else the
/// monthly one, else why there is none.
pub fn reading(billing: &Billing, machine: &str, now: i64) -> AgentUsage {
    let windows: Vec<UsageWindow> = match &billing.weekly {
        Some(weekly) => vec![weekly.clone()],
        None => billing.monthly.iter().cloned().collect(),
    };
    if windows.is_empty() {
        return AgentUsage::unknown(AgentId::GROK, machine, Reason::NoData);
    }
    AgentUsage {
        agent: AgentId::GROK,
        machine: machine.to_owned(),
        account_label: billing.tier.clone(),
        plan: billing.tier.clone(),
        source: Some(Source::VendorApi),
        observed_at: Some(now),
        state: State::Known { windows },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    fn auth(entries: &str) -> String {
        format!("{{{entries}}}")
    }

    #[test]
    fn the_default_issuers_fresh_token_is_chosen() {
        let json = auth(
            r#""https://other.example":{"key":"other-token"},
               "https://auth.x.ai":{"key":"tok-default","user_id":"u1","expires_at":"2099-01-01T00:00:00Z"}"#,
        );
        let credential = parse_credential(&json, NOW).unwrap();
        assert_eq!(credential.token.expose(), "tok-default");
        assert_eq!(credential.user_id.as_deref(), Some("u1"));
        assert!(!credential.expired);
    }

    #[test]
    fn an_expired_default_token_is_reported_expired_never_replaced_by_a_stale_issuer() {
        let json = auth(
            r#""https://other.example":{"key":"other-token"},
               "https://auth.x.ai::client":{"key":"tok-old","expires_at":"2001-01-01T00:00:00Z"}"#,
        );
        let credential = parse_credential(&json, NOW).unwrap();
        assert_eq!(credential.token.expose(), "tok-old");
        assert!(credential.expired);
        // A token inside the skew is expired too.
        let soon = auth(&format!(
            r#""https://auth.x.ai":{{"key":"t","expires_at":"{}"}}"#,
            chrono::DateTime::from_timestamp(NOW + 60, 0)
                .unwrap()
                .to_rfc3339()
        ));
        assert!(parse_credential(&soon, NOW).unwrap().expired);
    }

    #[test]
    fn another_issuer_is_used_only_when_there_is_no_default_entry() {
        let json = auth(r#""https://other.example":{"key":"other-token"}"#);
        assert_eq!(
            parse_credential(&json, NOW).unwrap().token.expose(),
            "other-token"
        );
        let signed_out = auth(r#""https://auth.x.ai":{"user_id":"u"}"#);
        assert!(parse_credential(&signed_out, NOW).is_none());
        assert!(parse_credential("not json", NOW).is_none());
        assert!(parse_credential("[]", NOW).is_none());
        assert!(parse_credential("{}", NOW).is_none());
    }

    #[test]
    fn a_reported_weekly_percentage_is_a_weekly_window() {
        let body = r#"{"config":{"creditUsagePercent":37.5,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-10-12T00:00:00Z"},"subscriptionTier":"SuperGrok"}}"#;
        let billing = parse_billing(body).unwrap();
        let weekly = billing.weekly.clone().unwrap();
        assert_eq!(weekly.kind, WindowKind::Weekly);
        assert_eq!(weekly.used_percent, 37.5);
        assert_eq!(weekly.resets_at, Some(1_791_763_200));
        assert_eq!(weekly.window_length, Some(7 * DAY));
        let usage = reading(&billing, "local", NOW);
        assert_eq!(usage.plan.as_deref(), Some("SuperGrok"));
        assert_eq!(usage.source, Some(Source::VendorApi));
    }

    #[test]
    fn a_flat_answer_without_config_is_read_too() {
        let billing = parse_billing(r#"{"creditUsagePercent":120}"#).unwrap();
        assert_eq!(billing.weekly.unwrap().used_percent, 100.0);
        assert!(parse_billing(r#"{"hello":1}"#).is_none());
        assert!(parse_billing("nope").is_none());
    }

    #[test]
    fn a_monthly_budget_is_a_monthly_window_and_never_called_weekly() {
        let body = r#"{"config":{"monthlyLimit":{"val":"20"},"used":{"val":"5"},"billingPeriodEnd":"2026-11-01T00:00:00Z"}}"#;
        let billing = parse_billing(body).unwrap();
        assert!(billing.weekly.is_none());
        let monthly = billing.monthly.clone().unwrap();
        assert_eq!(monthly.kind, WindowKind::Monthly);
        assert_eq!(monthly.used_percent, 25.0);
        let usage = reading(&billing, "local", NOW);
        let State::Known { windows } = usage.state else {
            panic!("expected windows")
        };
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].kind, WindowKind::Monthly);
    }

    #[test]
    fn a_missing_percentage_is_zero_only_when_the_weekly_period_is_confirmed() {
        let confirmed = r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-10-05T00:00:00Z","end":"2026-10-12T00:00:00Z"},"billingPeriodStart":"2026-10-05T00:00:00Z","billingPeriodEnd":"2026-10-12T00:00:00Z"}}"#;
        assert_eq!(
            parse_billing(confirmed)
                .unwrap()
                .weekly
                .unwrap()
                .used_percent,
            0.0
        );
        let unconfirmed = r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-10-05T00:00:00Z","end":"2026-10-12T00:00:00Z"},"billingPeriodStart":"2026-10-04T00:00:00Z","billingPeriodEnd":"2026-10-12T00:00:00Z"}}"#;
        assert!(parse_billing(unconfirmed).unwrap().weekly.is_none());
        // An amount of zero is "not reported", not "nothing used".
        let zero = r#"{"config":{"onDemandCap":{"val":"0"},"onDemandUsed":{"val":"3"},"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-10-05T00:00:00Z","end":"2026-10-12T00:00:00Z"},"billingPeriodStart":"2026-10-05T00:00:00Z","billingPeriodEnd":"2026-10-12T00:00:00Z"}}"#;
        assert!(parse_billing(zero).unwrap().weekly.is_none());
    }

    #[test]
    fn amounts_without_a_percentage_are_no_data_never_a_made_up_zero() {
        let body = r#"{"config":{"prepaidBalance":{"val":"12"}}}"#;
        let billing = parse_billing(body).unwrap();
        assert!(billing.reports_amounts);
        assert_eq!(
            reading(&billing, "local", NOW).state,
            State::Unknown {
                reason: Reason::NoData
            }
        );
    }

    #[test]
    fn a_percentage_of_the_wrong_type_is_not_a_window() {
        let billing = parse_billing(r#"{"config":{"creditUsagePercent":"37"}}"#).unwrap();
        assert!(billing.weekly.is_none());
    }
}

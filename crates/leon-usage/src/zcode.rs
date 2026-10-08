//! ZCode: the GLM Coding Plan quota.
//!
//! **Implemented from Orca's reference (`src/main/rate-limits/zcode-usage-fetcher.ts`),
//! unverified against the live service.** ZCode keeps its provider settings in
//! `~/.zcode/cli/config.json`: `model.main` names `<provider>/<model>`, and
//! `provider.<provider>.options` holds the plan's `apiKey` and `baseURL`. Only
//! the provider of the selected model is read, so another configured account's
//! quota is never shown as this one's. The key is sent, as the bare
//! `Authorization` value the service takes, to the plan's own host
//! (`api.z.ai`, `open.bigmodel.cn` or `dev.bigmodel.cn`, HTTPS, no other port)
//! and nowhere else: a `baseURL` on any other host is not used.

use leon_core::AgentId;
use serde_json::Value;

use crate::model::{AgentUsage, Reason, Source, State, UsageWindow, WindowKind, MINUTE};
use crate::secret::Secret;

/// The hosts a plan's key may be sent to.
pub const HOSTS: [&str; 3] = ["api.z.ai", "open.bigmodel.cn", "dev.bigmodel.cn"];

/// The key and the quota URL of the selected plan.
#[derive(Debug)]
pub struct Credential {
    /// The plan's API key.
    pub key: Secret,
    /// The quota URL, on one of [`HOSTS`].
    pub url: String,
}

/// Reads `config.json`. `None` is "no plan configured": no main model, no key,
/// a `baseURL` that is not HTTPS on an allowed host, or a key with a control
/// character.
pub fn parse_credential(json: &str) -> Option<Credential> {
    let config: Value = serde_json::from_str(json).ok()?;
    let model = config.get("model")?;
    let name = model
        .as_str()
        .or_else(|| model.get("main").and_then(Value::as_str))?;
    let (provider, rest) = name.split_once('/')?;
    if provider.is_empty() || rest.is_empty() {
        return None;
    }
    let options = config.pointer(&format!("/provider/{provider}/options"))?;
    let key = options.get("apiKey")?.as_str()?.trim();
    let base = options.get("baseURL")?.as_str()?;
    if key.is_empty() || key.contains(|c: char| c.is_control()) {
        return None;
    }
    let rest = base.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let (host, port) = authority.split_once(':').unwrap_or((authority, ""));
    if !HOSTS.contains(&host) || !(port.is_empty() || port == "443") {
        return None;
    }
    Some(Credential {
        key: Secret::new(key),
        url: format!("https://{host}/api/monitor/usage/quota/limit"),
    })
}

fn number(value: Option<&Value>) -> Option<f64> {
    value?.as_f64().filter(|n| n.is_finite())
}

fn used_percent(limit: &Value) -> Option<f64> {
    if let Some(total) = number(limit.get("usage")).filter(|t| *t > 0.0) {
        let current = number(limit.get("currentValue"));
        let remaining = number(limit.get("remaining"));
        if current.is_some() || remaining.is_some() {
            let used = current.unwrap_or(total - remaining.unwrap_or(0.0));
            return Some((used / total * 100.0).clamp(0.0, 100.0));
        }
    }
    number(limit.get("percentage")).map(|p| p.clamp(0.0, 100.0))
}

/// The window's length in minutes from the service's unit code: 1 days, 3
/// hours, 5 minutes, 6 weeks. The monthly tool limit is encoded as one minute.
fn minutes(limit: &Value) -> Option<i64> {
    let unit = number(limit.get("unit"))? as i64;
    let count = number(limit.get("number"))?;
    if limit.get("type").and_then(Value::as_str) == Some("TIME_LIMIT") && unit == 5 && count == 1.0
    {
        return Some(30 * 24 * 60);
    }
    if count.fract() != 0.0 || count <= 0.0 {
        return None;
    }
    let per = match unit {
        1 => 1440,
        3 => 60,
        5 => 1,
        6 => 10_080,
        _ => return None,
    };
    Some(count as i64 * per)
}

/// Parses the answer: `{"success":true,"data":{"limits":[..],"level":".."}}`.
/// The plan's session (300 minutes) and weekly (10080 minutes) windows, then
/// the time limit, labelled "MCP" as Orca does.
pub fn parse_usage(body: &str, machine: &str, now: i64) -> AgentUsage {
    let fail = || AgentUsage::unknown(AgentId::ZCODE, machine, Reason::ParseError);
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return fail();
    };
    let ok = value.get("success").and_then(Value::as_bool) == Some(true)
        && value
            .get("code")
            .is_none_or(|code| matches!(code.as_i64(), Some(0 | 200)));
    let Some(limits) = value.pointer("/data/limits").and_then(Value::as_array) else {
        return fail();
    };
    if !ok {
        return fail();
    }
    // As Orca keeps them: of the plan's token and credit limits only the
    // 300-minute window (the session) and the 10080-minute one (the week), the
    // first of each; the time limit is the plan's MCP allowance.
    let window = |limit: &Value, kind: WindowKind, length: i64| -> Option<UsageWindow> {
        let reset = number(limit.get("nextResetTime"))
            .filter(|t| *t > 0.0)
            .map(|ms| (ms / 1000.0) as i64);
        // A five-hour window cannot reset more than five hours ahead: the
        // service's time is then not that window's.
        let reset = reset.filter(|at| length != 300 || *at <= now + 301 * 60);
        Some(UsageWindow {
            kind,
            used_percent: used_percent(limit)?,
            resets_at: reset,
            window_length: Some(length * MINUTE),
        })
    };
    let of_type = |kinds: &[&str]| {
        limits
            .iter()
            .filter(|limit| {
                limit
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kinds.contains(&kind))
            })
            .filter_map(|limit| Some((limit, minutes(limit)?)))
            .collect::<Vec<_>>()
    };
    let plan_limits = of_type(&["TOKENS_LIMIT", "CREDIT_LIMIT"]);
    let pick = |length: i64, kind: WindowKind| {
        plan_limits
            .iter()
            .filter(|(_, minutes)| *minutes == length)
            .find_map(|(limit, _)| window(limit, kind.clone(), length))
    };
    let mcp = of_type(&["TIME_LIMIT"])
        .into_iter()
        .find_map(|(limit, length)| window(limit, WindowKind::Custom("MCP".into()), length));
    let windows: Vec<UsageWindow> = [
        pick(300, WindowKind::FiveHour),
        pick(10_080, WindowKind::Weekly),
        mcp,
    ]
    .into_iter()
    .flatten()
    .collect();
    if windows.is_empty() {
        return fail();
    }
    let plan = value
        .pointer("/data/level")
        .and_then(Value::as_str)
        .map(str::to_owned);
    AgentUsage {
        agent: AgentId::ZCODE,
        machine: machine.to_owned(),
        account_label: plan.clone(),
        account: None,
        plan,
        source: Some(Source::VendorApi),
        observed_at: Some(now),
        state: State::Known { windows },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    fn config(base: &str) -> String {
        format!(
            r#"{{"model":{{"main":"glm/glm-4.6"}},"provider":{{"glm":{{"options":{{"apiKey":" key-1 ","baseURL":"{base}"}}}},"other":{{"options":{{"apiKey":"key-2","baseURL":"https://api.z.ai"}}}}}}}}"#
        )
    }

    #[test]
    fn the_selected_plans_key_and_host_are_read() {
        let credential = parse_credential(&config("https://api.z.ai/api/coding/paas/v4")).unwrap();
        assert_eq!(credential.key.expose(), "key-1");
        assert_eq!(
            credential.url,
            "https://api.z.ai/api/monitor/usage/quota/limit"
        );
        let china = parse_credential(&config("https://open.bigmodel.cn:443/x")).unwrap();
        assert_eq!(
            china.url,
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
        );
    }

    #[test]
    fn a_key_is_never_sent_to_a_host_off_the_list() {
        for base in [
            "http://api.z.ai",
            "https://evil.example",
            "https://api.z.ai.evil.example",
            "https://api.z.ai:8443",
            "https://user@api.z.ai",
            "api.z.ai",
            "",
        ] {
            assert!(parse_credential(&config(base)).is_none(), "{base}");
        }
    }

    #[test]
    fn a_config_without_a_usable_main_model_or_key_is_signed_out() {
        assert!(parse_credential("nope").is_none());
        assert!(parse_credential(r#"{"model":"nomodel"}"#).is_none());
        assert!(parse_credential(r#"{"model":"x/y","provider":{}}"#).is_none());
        let newline = r#"{"model":"glm/m","provider":{"glm":{"options":{"apiKey":"a\nb","baseURL":"https://api.z.ai"}}}}"#;
        assert!(parse_credential(newline).is_none());
        let bare = r#"{"model":"glm/m","provider":{"glm":{"options":{"apiKey":"k","baseURL":"https://api.z.ai"}}}}"#;
        assert!(parse_credential(bare).is_some());
    }

    const BODY: &str = r#"{"success":true,"code":200,"data":{"level":"pro","limits":[
        {"type":"TOKENS_LIMIT","unit":6,"number":1,"usage":1000,"currentValue":250,"nextResetTime":1791300000000},
        {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":42,"nextResetTime":1790003600000},
        {"type":"TIME_LIMIT","unit":5,"number":1,"usage":100,"remaining":90,"nextResetTime":1792000000000},
        {"type":"OTHER","unit":3,"number":1,"percentage":1}]}}"#;

    #[test]
    fn the_session_and_week_come_first_and_the_time_limit_is_labelled_mcp() {
        let usage = parse_usage(BODY, "local", NOW);
        assert_eq!(usage.plan.as_deref(), Some("pro"));
        let State::Known { windows } = usage.state else {
            panic!("expected windows")
        };
        let list: Vec<(WindowKind, f64)> = windows
            .iter()
            .map(|w| (w.kind.clone(), w.used_percent))
            .collect();
        assert_eq!(
            list,
            [
                (WindowKind::FiveHour, 42.0),
                (WindowKind::Weekly, 25.0),
                (WindowKind::Custom("MCP".into()), 10.0),
            ]
        );
        assert_eq!(windows[0].resets_at, Some(1_790_003_600));
        assert_eq!(windows[1].window_length, Some(7 * 86_400));
    }

    #[test]
    fn a_five_hour_reset_too_far_ahead_is_dropped() {
        let body = r#"{"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":1,"nextResetTime":1799999999000}]}}"#;
        let State::Known { windows } = parse_usage(body, "local", NOW).state else {
            panic!("expected windows")
        };
        assert_eq!(windows[0].resets_at, None);
    }

    #[test]
    fn failures_and_odd_answers_are_parse_errors() {
        for body in [
            "nope",
            "{}",
            r#"{"success":false,"data":{"limits":[]}}"#,
            r#"{"success":true,"code":500,"data":{"limits":[]}}"#,
            r#"{"success":true,"data":{"limits":[]}}"#,
            r#"{"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":9,"number":1,"percentage":1}]}}"#,
            // A plan window of another length is not kept.
            r#"{"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":1,"percentage":1}]}}"#,
        ] {
            assert_eq!(
                parse_usage(body, "local", NOW).state,
                State::Unknown {
                    reason: Reason::ParseError
                },
                "{body}"
            );
        }
    }
}

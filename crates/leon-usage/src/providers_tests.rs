//! The network providers, end to end through a scripted client and a
//! scripted credential store: nothing here can reach a network, a keychain or
//! the home directory.

use std::sync::atomic::{AtomicUsize, Ordering};

use leon_core::AgentId;

use crate::cursor::test_token;
use crate::model::{Reason, Source, State};
use crate::network::{
    host_allowed, network_usage, switchable_agents, Auth, Credentials, CurlHttp, Http, HttpError,
    NetworkPolicy, Read, Request, ScriptedHttp, ALLOWED_HOSTS,
};
use crate::secret::Secret;
use crate::{claude, codex, cursor, grok, kimi, zcode};

const NOW: i64 = 1_790_000_000;
const SECRET: &str = "tok-do-not-print";

/// Synthetic credentials for every provider, counting every read.
#[derive(Default)]
struct Creds {
    missing: bool,
    expired: bool,
    reads: AtomicUsize,
}

impl Creds {
    fn bump(&self) {
        self.reads.fetch_add(1, Ordering::SeqCst);
    }
}

impl Credentials for Creds {
    fn claude(&self) -> Read<claude::Credential> {
        self.bump();
        let json =
            format!(r#"{{"claudeAiOauth":{{"accessToken":"{SECRET}","subscriptionType":"max"}}}}"#);
        claude::parse_credential(&json).map_or(Read::Missing, Read::Found)
    }
    fn opencode_go(&self) -> Read<Secret> {
        self.bump();
        Read::Found(Secret::new(SECRET))
    }
    fn codex(&self) -> Read<codex::Credential> {
        self.bump();
        if self.missing {
            return Read::Missing;
        }
        let json = format!(r#"{{"tokens":{{"access_token":"{SECRET}","account_id":"acct"}}}}"#);
        codex::parse_credential(&json).map_or(Read::Missing, Read::Found)
    }
    fn grok(&self, now: i64) -> Read<grok::Credential> {
        self.bump();
        if self.missing {
            return Read::Missing;
        }
        let expires = if self.expired {
            "2001-01-01T00:00:00Z"
        } else {
            "2099-01-01T00:00:00Z"
        };
        let json = format!(
            r#"{{"https://auth.x.ai":{{"key":"{SECRET}","user_id":"u1","expires_at":"{expires}"}}}}"#
        );
        grok::parse_credential(&json, now).map_or(Read::Missing, Read::Found)
    }
    fn cursor(&self, now: i64) -> Read<cursor::Session> {
        self.bump();
        if self.missing {
            return Read::Missing;
        }
        let exp = if self.expired { 1000 } else { 4_102_444_800i64 };
        let token = test_token(&format!(r#"{{"sub":"auth0|u","exp":{exp}}}"#));
        cursor::parse_session(&token, now).map_or(Read::Missing, Read::Found)
    }
    fn kimi(&self, now: i64) -> Read<kimi::Credential> {
        self.bump();
        if self.missing {
            return Read::Missing;
        }
        let at = if self.expired { 1000 } else { 4_102_444_800i64 };
        let json = format!(r#"{{"access_token":"{SECRET}","expires_at":{at}}}"#);
        kimi::parse_credential(&json, now).map_or(Read::Missing, Read::Found)
    }
    fn zcode(&self) -> Read<zcode::Credential> {
        self.bump();
        if self.missing {
            return Read::Missing;
        }
        let json = format!(
            r#"{{"model":"glm/m","provider":{{"glm":{{"options":{{"apiKey":"{SECRET}","baseURL":"https://api.z.ai/x"}}}}}}}}"#
        );
        zcode::parse_credential(&json).map_or(Read::Missing, Read::Found)
    }
}

const NETWORK: [AgentId; 7] = [
    AgentId::CLAUDE,
    AgentId::CODEX,
    AgentId::OPENCODE,
    AgentId::GROK,
    AgentId::CURSOR,
    AgentId::KIMI,
    AgentId::ZCODE,
];

/// An answer each provider reads as a reading.
fn good(agent: AgentId) -> &'static str {
    match agent {
        AgentId::CLAUDE => r#"{"five_hour":{"utilization":10,"resets_at":1791300000}}"#,
        AgentId::CODEX => {
            r#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":18000,"reset_at":1791300000}}}"#
        }
        AgentId::OPENCODE => {
            r#"{"usage":{"rolling":{"percent":5,"resetsAt":"2026-10-05T12:00:00Z"}}}"#
        }
        AgentId::GROK => r#"{"config":{"creditUsagePercent":10}}"#,
        AgentId::CURSOR => r#"{"individualUsage":{"plan":{"totalPercentUsed":10}}}"#,
        AgentId::KIMI => r#"{"usage":{"limit":10,"used":1}}"#,
        AgentId::ZCODE => {
            r#"{"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":3}]}}"#
        }
        _ => unreachable!(),
    }
}

fn on(agent: AgentId) -> NetworkPolicy {
    NetworkPolicy::none().with(agent)
}

#[tokio::test]
async fn a_switched_off_source_reads_nothing_and_calls_nothing() {
    for agent in NETWORK {
        let creds = Creds::default();
        let http = ScriptedHttp::new().reply(200, good(agent));
        let usage = network_usage(agent, &NetworkPolicy::none(), &creds, &http, "local", NOW).await;
        assert_eq!(
            usage.state,
            State::Unknown {
                reason: Reason::SourceDisabled
            },
            "{agent}"
        );
        assert!(http.calls().is_empty(), "{agent} called");
        assert_eq!(
            creds.reads.load(Ordering::SeqCst),
            0,
            "{agent} read a credential"
        );
    }
}

#[tokio::test]
async fn a_switched_on_source_calls_its_vendor_over_https_on_an_allowed_host() {
    for agent in NETWORK {
        let creds = Creds::default();
        let http = ScriptedHttp::new().reply(200, good(agent));
        let usage = network_usage(agent, &on(agent), &creds, &http, "local", NOW).await;
        assert!(
            matches!(usage.state, State::Known { .. }),
            "{agent}: {usage:?}"
        );
        assert_eq!(usage.source, Some(Source::VendorApi), "{agent}");
        assert_eq!(usage.agent, agent);
        assert_eq!(http.calls().len(), 1, "{agent}");
        for url in http.calls() {
            assert!(host_allowed(&url), "{url}");
        }
        assert!(!format!("{usage:?}").contains(SECRET));
    }
}

#[tokio::test]
async fn failures_are_explicit_reasons_never_numbers() {
    for agent in NETWORK {
        for (http, reason) in [
            (ScriptedHttp::new().reply(401, "{}"), Reason::NotSignedIn),
            (ScriptedHttp::new().reply(403, "{}"), Reason::NotSignedIn),
            (ScriptedHttp::new().reply(429, "{}"), Reason::RateLimited(0)),
            (
                ScriptedHttp::new().reply_after(429, "{}", Some(90)),
                Reason::RateLimited(90),
            ),
            (
                ScriptedHttp::new().reply(503, "oops"),
                Reason::VendorError(503),
            ),
            (
                ScriptedHttp::new().fail(HttpError::Unreachable),
                Reason::Offline,
            ),
            (
                ScriptedHttp::new().fail(HttpError::Refused),
                Reason::Unreachable,
            ),
        ] {
            let creds = Creds::default();
            let usage = network_usage(agent, &on(agent), &creds, &http, "local", NOW).await;
            let expected = if agent == AgentId::CURSOR && reason == Reason::NotSignedIn {
                // The dashboard's 401 is an expired session; 403 stays "signed out".
                None
            } else {
                Some(reason)
            };
            if let Some(expected) = expected {
                assert_eq!(
                    usage.state,
                    State::Unknown { reason: expected },
                    "{agent} {expected:?}"
                );
            }
            assert!(!format!("{usage:?}").contains(SECRET));
        }
        let creds = Creds::default();
        let http = ScriptedHttp::new().reply(200, "<html>not json</html>");
        let usage = network_usage(agent, &on(agent), &creds, &http, "local", NOW).await;
        let State::Unknown { reason } = usage.state else {
            panic!("{agent}: a page that is not json must not be a reading");
        };
        assert!(
            matches!(reason, Reason::ParseError | Reason::NoData),
            "{agent}: {reason:?}"
        );
    }
}

#[tokio::test]
async fn a_missing_sign_in_makes_no_call() {
    for agent in [
        AgentId::CODEX,
        AgentId::GROK,
        AgentId::CURSOR,
        AgentId::KIMI,
        AgentId::ZCODE,
    ] {
        let creds = Creds {
            missing: true,
            ..Default::default()
        };
        let http = ScriptedHttp::new().reply(200, good(agent));
        let usage = network_usage(agent, &on(agent), &creds, &http, "local", NOW).await;
        assert_eq!(
            usage.state,
            State::Unknown {
                reason: Reason::NotSignedIn
            },
            "{agent}"
        );
        assert!(http.calls().is_empty(), "{agent}");
    }
}

#[tokio::test]
async fn an_expired_sign_in_is_reported_never_refreshed_and_makes_no_call() {
    for agent in [AgentId::GROK, AgentId::CURSOR, AgentId::KIMI] {
        let creds = Creds {
            expired: true,
            ..Default::default()
        };
        let http = ScriptedHttp::new().reply(200, good(agent));
        let usage = network_usage(agent, &on(agent), &creds, &http, "local", NOW).await;
        assert_eq!(
            usage.state,
            State::Unknown {
                reason: Reason::SessionExpired
            },
            "{agent}"
        );
        assert!(http.calls().is_empty(), "{agent}");
    }
}

#[tokio::test]
async fn the_dashboards_401_is_an_expired_session() {
    let creds = Creds::default();
    let http = ScriptedHttp::new().reply(401, "{}");
    let usage = network_usage(
        AgentId::CURSOR,
        &on(AgentId::CURSOR),
        &creds,
        &http,
        "local",
        NOW,
    )
    .await;
    assert_eq!(
        usage.state,
        State::Unknown {
            reason: Reason::SessionExpired
        }
    );
}

#[tokio::test]
async fn grok_asks_the_default_view_only_when_the_credits_view_has_no_budget() {
    let creds = Creds::default();
    let http = ScriptedHttp::new()
        .reply(200, r#"{"config":{"subscriptionTier":"T"}}"#)
        .reply(
            200,
            r#"{"config":{"monthlyLimit":{"val":"10"},"used":{"val":"2"}}}"#,
        );
    let usage = network_usage(
        AgentId::GROK,
        &on(AgentId::GROK),
        &creds,
        &http,
        "local",
        NOW,
    )
    .await;
    assert_eq!(http.calls(), [grok::CREDITS_URL, grok::DEFAULT_URL]);
    assert_eq!(usage.plan.as_deref(), Some("T"));
    let State::Known { windows } = usage.state else {
        panic!("expected a window")
    };
    assert_eq!(windows[0].used_percent, 20.0);
    // One call when the credits view already answers.
    let http = ScriptedHttp::new().reply(200, good(AgentId::GROK));
    network_usage(
        AgentId::GROK,
        &on(AgentId::GROK),
        &creds,
        &http,
        "local",
        NOW,
    )
    .await;
    assert_eq!(http.calls().len(), 1);
}

#[tokio::test]
async fn cursor_falls_back_to_the_older_endpoint_for_a_plan_with_no_pools() {
    let creds = Creds::default();
    let http = ScriptedHttp::new()
        .reply(200, r#"{"membershipType":"pro"}"#)
        .reply(200, r#"{"gpt-4":{"numRequests":50,"maxRequestUsage":500},"startOfMonth":"2026-10-01T00:00:00Z"}"#);
    let usage = network_usage(
        AgentId::CURSOR,
        &on(AgentId::CURSOR),
        &creds,
        &http,
        "local",
        NOW,
    )
    .await;
    let calls = http.calls();
    assert_eq!(calls[0], cursor::SUMMARY_URL);
    assert!(calls[1].starts_with(cursor::LEGACY_URL) && calls[1].contains("?user=auth0%7Cu"));
    assert!(matches!(usage.state, State::Known { .. }));
}

#[tokio::test]
async fn an_unlimited_cursor_plan_says_so() {
    let creds = Creds::default();
    let http = ScriptedHttp::new().reply(200, r#"{"isUnlimited":true}"#);
    let usage = network_usage(
        AgentId::CURSOR,
        &on(AgentId::CURSOR),
        &creds,
        &http,
        "local",
        NOW,
    )
    .await;
    assert_eq!(
        usage.state,
        State::Unknown {
            reason: Reason::Unlimited
        }
    );
    assert_eq!(http.calls().len(), 1);
}

#[test]
fn every_vendor_host_is_listed_and_nothing_else_is_allowed() {
    for url in [
        claude::URL,
        crate::opencode::URL,
        codex::BACKEND_URL,
        grok::CREDITS_URL,
        grok::DEFAULT_URL,
        cursor::SUMMARY_URL,
        cursor::LEGACY_URL,
        kimi::URL,
        "https://api.z.ai/api/monitor/usage/quota/limit",
        "https://open.bigmodel.cn/api/monitor/usage/quota/limit",
        "https://dev.bigmodel.cn/api/monitor/usage/quota/limit",
    ] {
        assert!(host_allowed(url), "{url}");
    }
    assert_eq!(ALLOWED_HOSTS.len(), 9);
    for url in [
        "https://grok.com/v1/billing",
        "https://api.x.ai/",
        "https://cursor.com.evil.example/api/usage",
        "https://www.cursor.com/api/usage",
        "https://chatgpt.com.evil.example/",
        "https://api.kimi.com@evil.example/",
        "http://cursor.com/api/usage",
        "https://evil.example/?https://cursor.com/",
        "https://z.ai/",
    ] {
        assert!(!host_allowed(url), "{url}");
    }
}

#[tokio::test]
async fn curl_refuses_an_unlisted_host_before_starting_anything_for_every_auth_kind() {
    let secret = Secret::new(SECRET);
    for auth in [
        Auth::Bearer(&secret),
        Auth::Header {
            name: "Cookie",
            value: &secret,
        },
        Auth::Header {
            name: "Authorization",
            value: &secret,
        },
    ] {
        let request = Request {
            url: "https://evil.example/api",
            auth,
            headers: vec![],
        };
        assert_eq!(CurlHttp.get(&request).await, Err(HttpError::Refused));
    }
}

#[test]
fn a_cookie_and_a_bare_key_are_sent_as_their_own_headers() {
    let secret = Secret::new("WorkosCursorSessionToken=x");
    let request = Request {
        url: cursor::SUMMARY_URL,
        auth: Auth::Header {
            name: "Cookie",
            value: &secret,
        },
        headers: vec![("Origin", "https://cursor.com".into())],
    };
    let config = crate::network::curl_config(&request);
    assert!(config.contains(r#"header = "Cookie: WorkosCursorSessionToken=x""#));
    assert!(config.contains(r#"header = "Origin: https://cursor.com""#));
    assert!(!config.contains("Bearer"));
    assert!(!format!("{request:?}").contains("WorkosCursorSessionToken"));
}

#[test]
fn every_catalogue_agent_with_a_usage_provider_has_a_switch() {
    let switchable = switchable_agents();
    for spec in leon_core::agent::builtin() {
        assert_eq!(
            spec.usage.is_some(),
            switchable.contains(&spec.id),
            "{}",
            spec.id
        );
    }
    assert!(NetworkPolicy::all().allows(AgentId::GROK));
    assert!(!NetworkPolicy::none().allows(AgentId::GROK));
}

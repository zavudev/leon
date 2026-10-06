//! Antigravity: its own `/usage` command.
//!
//! **Implemented from Orca's reference (`src/main/rate-limits/antigravity-*.ts`),
//! unverified against the live service.** `agy` keeps its Google credential in
//! the operating system's keyring and mints its own tokens, so nothing outside
//! `agy` can ask the quota endpoint. Its print mode answers the slash command
//! `/usage` as a JSON envelope: `agy -p /usage --output-format json`. From
//! `agy` 1.1.11 that is a metadata read (no model turn, no quota spent);
//! before that version the same text is a prompt and costs a turn, so the
//! collection runs the command only after `agy --version` has said it is new
//! enough. The command runs on the machine where the agent is, through the
//! same runner as everything else.
//!
//! The pure parts are here: the version check and the parser. `--disable-slash-commands` must never be added
//! to the command: it turns `/usage` into an ordinary prompt.

use chrono::DateTime;
use leon_core::AgentId;
use serde_json::Value;

use crate::model::{AgentUsage, Reason, Source, State, UsageWindow, WindowKind, DAY, HOUR};

/// The command that reads the limits, as its arguments.
pub const USAGE_ARGS: [&str; 5] = ["-p", "/usage", "--output-format", "json", "--print-timeout"];
/// The first `agy` where `/usage` is a metadata read.
pub const MIN_VERSION: (u32, u32, u32) = (1, 1, 11);

/// The first `x.y.z` in a version line (`agy 1.2.11`, `1.2.11 (build)`).
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find_map(|word| {
            let mut parts = word.split('.');
            let version = (
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
            );
            Some(version)
        })
}

/// Whether this `agy` answers `/usage` without a model turn.
pub fn supports_usage(version: (u32, u32, u32)) -> bool {
    version >= MIN_VERSION
}

/// The machines where `agy` answered `/usage` with a model turn: it treats the
/// command as a prompt, every read would spend quota, so Leon stops asking
/// until it restarts.
static LATCHED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Whether the usage command is latched off for `machine`.
pub fn latched(machine: &str) -> bool {
    LATCHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|m| m == machine)
}

/// Latches the usage command off for `machine` for the rest of the session.
pub fn latch(machine: &str) {
    let mut list = LATCHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !list.iter().any(|m| m == machine) {
        list.push(machine.to_owned());
    }
}

/// Clears the latch (tests only: a running session has no way back).
#[cfg(test)]
pub fn unlatch(machine: &str) {
    LATCHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|m| m != machine);
}

/// Whether the envelope shows a model ran a turn instead of the command
/// answering: a real command reply has no conversation and zero turns.
fn ran_model_turn(envelope: &Value) -> bool {
    envelope
        .get("num_turns")
        .and_then(Value::as_f64)
        .is_some_and(|turns| turns > 0.0)
        || envelope
            .get("conversation_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
}

/// Why the command failed, from its `AGY_ERROR:{json}` lines (the structured
/// status or code first) and, failing those, the sign-out phrases it prints.
fn classify_failure(output: &str) -> Option<Reason> {
    for line in output.lines() {
        let Some((_, payload)) = line.split_once("AGY_ERROR:") else {
            continue;
        };
        let Ok(error) = serde_json::from_str::<Value>(payload.trim()) else {
            continue;
        };
        let status = error
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_uppercase();
        let code = error.get("error_code").and_then(Value::as_i64);
        if status == "UNAUTHENTICATED" || code == Some(401) {
            return Some(Reason::NotSignedIn);
        }
        if status == "PERMISSION_DENIED" || code == Some(403) {
            return Some(Reason::NoSubscription);
        }
        if status == "RESOURCE_EXHAUSTED" || code == Some(429) {
            return Some(Reason::RateLimited(0));
        }
        if let Some(code @ 500..=599) = code {
            return Some(Reason::VendorError(code as u16));
        }
    }
    let text = output.to_ascii_lowercase();
    [
        "not logged into antigravity",
        "not logged in",
        "not signed in",
        "not authenticated",
        "unauthenticated",
        "run agy login",
        "please sign in",
        "please log in",
        "no credentials",
        "authentication required",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
    .then_some(Reason::NotSignedIn)
}

fn used_percent(fraction: f64) -> f64 {
    ((1.0 - fraction) * 100.0).round().clamp(0.0, 100.0)
}

/// Parses what the command printed: a JSON envelope (on one line, possibly
/// among log lines) with `status: "SUCCESS"` and the groups under
/// `command.data`. The groups are the contract, not the `response` text. A
/// disabled bucket is not metered and is left out; a window name other than
/// `weekly` and `5h` is shown as a named bucket without a length.
pub fn parse_output(output: &str, machine: &str, now: i64) -> AgentUsage {
    let unknown = |reason| AgentUsage::unknown(AgentId::ANTIGRAVITY, machine, reason);
    let envelope = output
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find(Value::is_object)
        .or_else(|| serde_json::from_str::<Value>(output.trim()).ok());
    // The usage command's reply names itself `usage` (`/quota` is an alias):
    // that is what proves the payload is a quota and not another command's.
    let groups = envelope
        .as_ref()
        .filter(|e| e.get("status").and_then(Value::as_str) == Some("SUCCESS"))
        .filter(|e| e.pointer("/command/name").and_then(Value::as_str) == Some("usage"))
        .and_then(|e| e.pointer("/command/data/groups"))
        .and_then(Value::as_array);
    let Some(groups) = groups else {
        // A model turn where the command's data should be: this build treats
        // `/usage` as a prompt.
        if envelope.as_ref().is_some_and(ran_model_turn) {
            return unknown(Reason::SpendsATurn);
        }
        return unknown(classify_failure(output).unwrap_or(Reason::ParseError));
    };
    let fail = || unknown(Reason::ParseError);
    let mut windows = Vec::new();
    for group in groups {
        let Some(name) = group
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty())
        else {
            continue;
        };
        let buckets: Vec<&Value> =
            group
                .get("buckets")
                .and_then(Value::as_array)
                .map_or(Vec::new(), |list| {
                    list.iter()
                        .filter(|b| b.get("disabled").and_then(Value::as_bool) != Some(true))
                        .collect()
                });
        for bucket in &buckets {
            let Some(fraction) = bucket
                .get("remaining_fraction")
                .and_then(Value::as_f64)
                .filter(|f| f.is_finite())
            else {
                continue;
            };
            let label = match (buckets.len(), bucket.get("name").and_then(Value::as_str)) {
                (n, Some(bucket_name)) if n > 1 => format!("{name} · {bucket_name}"),
                _ => name.to_owned(),
            };
            let length = match bucket.get("window").and_then(Value::as_str) {
                Some("weekly") => Some(7 * DAY),
                Some("5h") => Some(5 * HOUR),
                _ => None,
            };
            windows.push(UsageWindow {
                kind: WindowKind::Custom(label),
                used_percent: used_percent(fraction),
                resets_at: bucket
                    .get("reset_time")
                    .and_then(Value::as_str)
                    .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| t.timestamp()),
                window_length: length,
            });
        }
    }
    if windows.is_empty() {
        return fail();
    }
    AgentUsage {
        agent: AgentId::ANTIGRAVITY,
        machine: machine.to_owned(),
        account_label: None,
        plan: None,
        source: Some(Source::Cli),
        observed_at: Some(now),
        state: State::Known { windows },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    #[test]
    fn versions_are_found_in_a_line_and_compared() {
        assert_eq!(parse_version("agy 1.2.11"), Some((1, 2, 11)));
        assert_eq!(parse_version("1.1.11 (build 7)\n"), Some((1, 1, 11)));
        assert_eq!(parse_version("no version"), None);
        assert!(supports_usage((1, 1, 11)));
        assert!(supports_usage((1, 2, 0)));
        assert!(supports_usage((2, 0, 0)));
        assert!(!supports_usage((1, 1, 10)));
        assert!(!supports_usage((0, 9, 99)));
    }

    #[test]
    fn the_command_never_disables_slash_commands() {
        assert!(!USAGE_ARGS.contains(&"--disable-slash-commands"));
    }

    const ENVELOPE: &str = r#"{"status":"SUCCESS","response":"text","command":{"name":"usage","data":{"groups":[
        {"name":"Gemini Models","buckets":[{"id":"gemini-weekly","name":"Weekly Limit Remaining","window":"weekly","remaining_fraction":0.75,"reset_time":"2026-10-07T08:08:35Z"}]},
        {"name":"Claude and GPT models","buckets":[
            {"id":"3p-5h","name":"5 Hour Limit Remaining","window":"5h","remaining_fraction":1,"reset_time":"2026-10-05T12:00:00Z"},
            {"id":"3p-weekly","name":"Weekly Limit Remaining","window":"weekly","remaining_fraction":0.5,"reset_time":"2026-10-08T00:00:00Z"},
            {"id":"3p-x","name":"Off","window":"weekly","remaining_fraction":0,"disabled":true}]}]}}}"#;

    #[test]
    fn groups_become_named_windows_and_a_disabled_bucket_is_left_out() {
        let compact = ENVELOPE.split_whitespace().collect::<Vec<_>>().join(" ");
        let usage = parse_output(&format!("log line\n{compact}\n"), "local", NOW);
        assert_eq!(usage.source, Some(Source::Cli));
        let State::Known { windows } = usage.state else {
            panic!("expected windows")
        };
        let list: Vec<(String, f64, Option<i64>)> = windows
            .iter()
            .map(|w| (w.kind.long(), w.used_percent, w.window_length))
            .collect();
        assert_eq!(
            list,
            [
                ("Gemini Models".to_owned(), 25.0, Some(7 * DAY)),
                (
                    "Claude and GPT models · 5 Hour Limit Remaining".to_owned(),
                    0.0,
                    Some(5 * HOUR)
                ),
                (
                    "Claude and GPT models · Weekly Limit Remaining".to_owned(),
                    50.0,
                    Some(7 * DAY)
                ),
            ]
        );
        assert_eq!(windows[0].resets_at, Some(1_791_360_515));
    }

    #[test]
    fn anything_else_is_a_parse_error_never_a_zero() {
        for output in [
            "",
            "nope",
            r#"{"status":"ERROR"}"#,
            r#"{"status":"SUCCESS"}"#,
            r#"{"status":"SUCCESS","command":{"data":{"groups":[]}}}"#,
            r#"{"status":"SUCCESS","command":{"data":{"groups":[{"name":"G","buckets":[{"id":"x","disabled":true,"remaining_fraction":1}]}]}}}"#,
        ] {
            assert_eq!(
                parse_output(output, "local", NOW).state,
                State::Unknown {
                    reason: Reason::ParseError
                },
                "{output}"
            );
        }
    }

    #[test]
    fn a_window_of_an_unknown_name_has_no_length() {
        let body = r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[{"name":"G","buckets":[{"id":"x","window":"daily","remaining_fraction":0.9}]}]}}}"#;
        let State::Known { windows } = parse_output(body, "local", NOW).state else {
            panic!("expected windows")
        };
        assert_eq!(windows[0].window_length, None);
        assert_eq!(windows[0].used_percent, 10.0);
    }

    #[test]
    fn the_replys_command_must_be_usage() {
        let other = r#"{"status":"SUCCESS","command":{"name":"help","data":{"groups":[{"name":"G","buckets":[{"id":"x","remaining_fraction":0.5}]}]}}}"#;
        assert_eq!(
            parse_output(other, "m", NOW).state,
            State::Unknown {
                reason: Reason::ParseError
            }
        );
    }

    #[test]
    fn a_model_turn_instead_of_the_command_is_a_prompt_that_spends_quota() {
        for output in [
            r#"{"status":"SUCCESS","response":"Hello","num_turns":1,"conversation_id":""}"#,
            r#"{"status":"SUCCESS","response":"Hi","num_turns":0,"conversation_id":"28a5ca91-301f"}"#,
        ] {
            assert_eq!(
                parse_output(output, "m", NOW).state,
                State::Unknown {
                    reason: Reason::SpendsATurn
                },
                "{output}"
            );
        }
        // A real reading is never mistaken for a prompt.
        let real = r#"{"status":"SUCCESS","num_turns":0,"conversation_id":"","command":{"name":"usage","data":{"groups":[{"name":"G","buckets":[{"id":"x","remaining_fraction":0.5}]}]}}}"#;
        assert!(matches!(
            parse_output(real, "m", NOW).state,
            State::Known { .. }
        ));
    }

    #[test]
    fn the_latch_is_per_machine_and_lasts() {
        let machine = "latch-test-machine";
        unlatch(machine);
        assert!(!latched(machine));
        latch(machine);
        latch(machine);
        assert!(latched(machine));
        assert!(!latched("another-machine"));
        unlatch(machine);
    }

    #[test]
    fn errors_are_classified_instead_of_being_parse_errors() {
        for (output, reason) in [
            (
                r#"AGY_ERROR:{"status":"UNAUTHENTICATED"}"#,
                Reason::NotSignedIn,
            ),
            (r#"AGY_ERROR:{"error_code":401}"#, Reason::NotSignedIn),
            (
                r#"AGY_ERROR:{"status":"RESOURCE_EXHAUSTED"}"#,
                Reason::RateLimited(0),
            ),
            (r#"AGY_ERROR:{"error_code":429}"#, Reason::RateLimited(0)),
            (r#"AGY_ERROR:{"error_code":503}"#, Reason::VendorError(503)),
            (
                r#"AGY_ERROR:{"status":"PERMISSION_DENIED"}"#,
                Reason::NoSubscription,
            ),
            ("Please sign in to Antigravity", Reason::NotSignedIn),
            ("something odd", Reason::ParseError),
        ] {
            assert_eq!(
                parse_output(output, "m", NOW).state,
                State::Unknown { reason },
                "{output}"
            );
        }
    }
}

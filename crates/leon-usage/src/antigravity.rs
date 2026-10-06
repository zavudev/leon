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

fn used_percent(fraction: f64) -> f64 {
    ((1.0 - fraction) * 100.0).round().clamp(0.0, 100.0)
}

/// Parses what the command printed: a JSON envelope (on one line, possibly
/// among log lines) with `status: "SUCCESS"` and the groups under
/// `command.data`. The groups are the contract, not the `response` text. A
/// disabled bucket is not metered and is left out; a window name other than
/// `weekly` and `5h` is shown as a named bucket without a length.
pub fn parse_output(output: &str, machine: &str, now: i64) -> AgentUsage {
    let fail = || AgentUsage::unknown(AgentId::ANTIGRAVITY, machine, Reason::ParseError);
    let Some(envelope) = output
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find(Value::is_object)
        .or_else(|| serde_json::from_str::<Value>(output.trim()).ok())
    else {
        return fail();
    };
    if envelope.get("status").and_then(Value::as_str) != Some("SUCCESS") {
        return fail();
    }
    let Some(groups) = envelope
        .pointer("/command/data/groups")
        .and_then(Value::as_array)
    else {
        return fail();
    };
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
        let body = r#"{"status":"SUCCESS","command":{"data":{"groups":[{"name":"G","buckets":[{"id":"x","window":"daily","remaining_fraction":0.9}]}]}}}"#;
        let State::Known { windows } = parse_output(body, "local", NOW).state else {
            panic!("expected windows")
        };
        assert_eq!(windows[0].window_length, None);
        assert_eq!(windows[0].used_percent, 10.0);
    }
}

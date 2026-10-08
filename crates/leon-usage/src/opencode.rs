//! opencode: the limits of the opencode Go subscription.
//!
//! opencode itself keeps no limits on disk, and the providers it fronts have
//! none to read. Only the opencode Go subscription has a usage endpoint, called
//! with the API key opencode stored when its owner connected the account
//! (`auth.json`, entry `opencode-go`). That reads a credential, so it is an
//! opt-in source; this module holds the pure parts.

use leon_core::AgentId;
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

/// What a refusal of the usage endpoint means: the console names an
/// entitlement refusal (`EntitlementError`, or 403) as a key with no Go
/// subscription, and an authentication one (`AuthError`, or 401) as a key it
/// does not know.
pub fn auth_reason(status: u16, body: &str) -> Reason {
    let kind = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        v.pointer("/error/type")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    match (kind.as_deref(), status) {
        (Some("EntitlementError"), _) | (_, 403) => Reason::NoSubscription,
        _ => Reason::KeyRejected,
    }
}

/// Reads the key from OpenCode 2's credential database, read only: the
/// `credential` table, rows of the `opencode-go` integration, the active one
/// first and the newest next, each `value` a JSON text `{"type":"key","key":..}`.
/// `data_dir` is opencode's data folder; `db_override` its `OPENCODE_DB`.
pub fn database_key(data_dir: &std::path::Path, db_override: Option<&str>) -> Option<Secret> {
    let mut paths = Vec::new();
    match db_override.map(str::trim).filter(|path| !path.is_empty()) {
        Some(":memory:") => return None,
        Some(path) => {
            let path = std::path::Path::new(path);
            paths.push(if path.is_absolute() {
                path.to_path_buf()
            } else {
                data_dir.join(path)
            });
        }
        None => {
            let mut found: Vec<_> = std::fs::read_dir(data_dir)
                .ok()?
                .flatten()
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    entry.path().is_file()
                        && name.starts_with("opencode")
                        && name.ends_with(".db")
                        && name[8..name.len() - 3]
                            .chars()
                            .all(|c| c == '-' || c == '_' || c == '.' || c.is_ascii_alphanumeric())
                })
                .map(|entry| entry.path())
                .collect();
            found.sort();
            paths = found;
        }
    }
    paths.iter().find_map(|path| key_from_database(path))
}

fn key_from_database(path: &std::path::Path) -> Option<Secret> {
    use rusqlite::OpenFlags;
    let connection = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let mut statement = connection
        .prepare(
            "SELECT value FROM credential WHERE integration_id = ?1 \
             ORDER BY active DESC, time_created DESC LIMIT 8",
        )
        .ok()?;
    let rows = statement
        .query_map(["opencode-go"], |row| row.get::<_, String>(0))
        .ok()?;
    let key = rows.flatten().find_map(|value| {
        let value: Value = serde_json::from_str(&value).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("key") {
            return None;
        }
        let key = value.get("key")?.as_str()?.trim();
        (!key.is_empty()).then(|| Secret::new(key))
    });
    key
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
    let fail = || AgentUsage::unknown(AgentId::OPENCODE, machine, Reason::ParseError);
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
        agent: AgentId::OPENCODE,
        machine: machine.to_owned(),
        account_label: Some("Go".into()),
        account: None,
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

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("leon-usage-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn database(dir: &std::path::Path, file: &str, rows: &[(&str, &str, i64, i64)]) {
        let connection = rusqlite::Connection::open(dir.join(file)).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE credential (integration_id TEXT, value TEXT, active INTEGER, time_created INTEGER);",
            )
            .unwrap();
        for (integration, value, active, created) in rows {
            connection
                .execute(
                    "INSERT INTO credential VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![integration, value, active, created],
                )
                .unwrap();
        }
    }

    #[test]
    fn the_key_is_read_from_the_credential_table_active_row_first() {
        let dir = scratch("credential");
        database(
            &dir,
            "opencode.db",
            &[
                ("opencode-go", r#"{"type":"key","key":"newest"}"#, 0, 30),
                ("opencode-go", r#"{"type":"key","key":" active "}"#, 1, 10),
                (
                    "opencode-go",
                    r#"{"type":"oauth","key":"wrong-type"}"#,
                    1,
                    40,
                ),
                ("other", r#"{"type":"key","key":"other"}"#, 1, 50),
            ],
        );
        let key = database_key(&dir, None).unwrap();
        assert_eq!(key.expose(), "active");
        // The explicit database and a missing one.
        assert!(database_key(&dir, Some("opencode.db")).is_some());
        assert!(database_key(&dir, Some("absent.db")).is_none());
        assert!(database_key(&dir, Some(":memory:")).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_database_without_the_table_or_a_go_row_gives_no_key() {
        let dir = scratch("nokey");
        rusqlite::Connection::open(dir.join("opencode.db"))
            .unwrap()
            .execute_batch("CREATE TABLE other (a TEXT);")
            .unwrap();
        assert!(database_key(&dir, None).is_none());
        let rows = scratch("norow");
        database(&rows, "opencode-dev.db", &[("other", "{}", 1, 1)]);
        assert!(database_key(&rows, None).is_none());
        // Names that are not opencode databases are not opened.
        std::fs::write(rows.join("notes.db"), b"x").unwrap();
        assert!(database_key(&rows, None).is_none());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&rows).ok();
    }

    #[test]
    fn refusals_are_told_by_the_error_name_then_the_status() {
        assert_eq!(auth_reason(403, "{}"), Reason::NoSubscription);
        assert_eq!(
            auth_reason(402, r#"{"error":{"type":"EntitlementError"}}"#),
            Reason::NoSubscription
        );
        assert_eq!(
            auth_reason(401, r#"{"error":{"type":"AuthError"}}"#),
            Reason::KeyRejected
        );
        assert_eq!(auth_reason(401, "not json"), Reason::KeyRejected);
    }
}

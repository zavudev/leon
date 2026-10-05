//! Small text and JSON helpers shared by the parsers.
//!
//! These encode the presentation decisions of the unified history: what a
//! title looks like, how a tool call is reduced to one readable line, and how
//! the message content blocks that Claude Code and Codex have in common are
//! read.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

/// The longest title kept, in characters.
const MAX_TITLE_CHARS: usize = 80;

/// The longest tool-call summary kept, in characters.
const MAX_TOOL_DETAIL_CHARS: usize = 160;

/// The input fields that best describe what a tool call did, most telling
/// first.
const TOOL_DETAIL_KEYS: [&str; 10] = [
    "command",
    "cmd",
    "file_path",
    "filePath",
    "path",
    "pattern",
    "query",
    "url",
    "description",
    "prompt",
];

/// Parses an RFC 3339 timestamp and converts it to UTC.
pub(crate) fn parse_timestamp(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// Cuts `text` to at most `max_bytes` bytes without splitting a character.
pub(crate) fn clip(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Cuts `text` to at most `max_chars` characters. A cut text ends with an
/// ellipsis, which counts towards the limit.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().nth(max_chars).is_none() {
        return text.to_owned();
    }
    let end = text
        .char_indices()
        .nth(max_chars.saturating_sub(1))
        .map_or(text.len(), |(index, _)| index);
    format!("{}…", text[..end].trim_end())
}

/// Derives a one-line title from free text: its first non-blank line, with
/// runs of whitespace collapsed, cut to a displayable length.
pub(crate) fn title_from(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, MAX_TITLE_CHARS)
}

/// Reduces a tool call to one readable line such as `Bash: cargo test`.
///
/// The detail is the most telling field of the tool's input (see
/// [`TOOL_DETAIL_KEYS`]); when the input is plain text its first line is used,
/// and when nothing suitable exists the tool name stands alone.
pub(crate) fn tool_line(name: &str, input: Option<&Value>) -> String {
    let name = if name.is_empty() { "tool" } else { name };
    let detail = match input {
        Some(Value::Object(fields)) => TOOL_DETAIL_KEYS
            .iter()
            .find_map(|key| fields.get(*key).and_then(detail_text)),
        Some(other) => detail_text(other),
        None => None,
    };
    match detail {
        Some(detail) => format!("{name}: {detail}"),
        None => name.to_owned(),
    }
}

/// Renders a JSON value as a short single-line detail, if it has a natural
/// textual form.
fn detail_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(truncate_chars(line, MAX_TOOL_DETAIL_CHARS))
}

/// The `content` of a message as Claude Code and Codex write it: either plain
/// text or a list of typed blocks.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Content {
    /// Plain text.
    Text(String),
    /// A list of typed blocks.
    Blocks(Vec<Block>),
}

/// One content block. Only the fields Leon reads are declared; every other
/// field is ignored, and all are optional so an unfamiliar block type never
/// makes a line unreadable.
#[derive(Debug, Deserialize)]
pub(crate) struct Block {
    /// Block type, such as `text` or `tool_use`.
    #[serde(rename = "type")]
    pub(crate) kind: Option<String>,
    /// Text of a text block.
    pub(crate) text: Option<String>,
    /// Tool name of a tool-call block.
    pub(crate) name: Option<String>,
    /// Tool input of a tool-call block.
    pub(crate) input: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn timestamps_are_normalised_to_utc() {
        let at = parse_timestamp("2026-03-01T12:00:00+02:00").unwrap();
        assert_eq!(at.to_rfc3339(), "2026-03-01T10:00:00+00:00");
        assert_eq!(
            parse_timestamp("2026-03-01T10:00:00.250Z").unwrap(),
            at + chrono::Duration::milliseconds(250)
        );
        assert_eq!(parse_timestamp("yesterday"), None);
    }

    #[test]
    fn clipping_never_splits_a_character() {
        assert_eq!(clip("héllo", 2), "h");
        assert_eq!(clip("héllo", 3), "hé");
        assert_eq!(clip("short", 100), "short");
        assert_eq!(clip("", 0), "");
    }

    #[test]
    fn a_title_is_the_first_non_blank_line_with_collapsed_whitespace() {
        assert_eq!(
            title_from("\n\n  fix   the\tlogin flow  \nmore"),
            "fix the login flow"
        );
        assert_eq!(title_from("   \n  "), "");
    }

    #[test]
    fn a_long_title_is_cut_and_marked() {
        let title = title_from(&"word ".repeat(40));
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn a_tool_call_shows_its_most_telling_input_field() {
        assert_eq!(
            tool_line(
                "Bash",
                Some(&json!({"description": "run tests", "command": "cargo test"}))
            ),
            "Bash: cargo test"
        );
        assert_eq!(
            tool_line("Read", Some(&json!({"file_path": "/srv/api/main.rs"}))),
            "Read: /srv/api/main.rs"
        );
        assert_eq!(
            tool_line("shell", Some(&json!({"command": ["git", "status"]}))),
            "shell: git status"
        );
    }

    #[test]
    fn a_tool_call_with_plain_text_input_shows_its_first_line() {
        assert_eq!(
            tool_line("apply_patch", Some(&json!("\n*** Begin Patch\n*** more"))),
            "apply_patch: *** Begin Patch"
        );
    }

    #[test]
    fn a_tool_call_without_a_usable_detail_shows_only_its_name() {
        assert_eq!(
            tool_line("TodoWrite", Some(&json!({"todos": []}))),
            "TodoWrite"
        );
        assert_eq!(tool_line("Idle", None), "Idle");
        assert_eq!(tool_line("", Some(&json!(42))), "tool");
    }

    #[test]
    fn a_long_tool_detail_is_cut() {
        let line = tool_line("Bash", Some(&json!({"command": "x".repeat(500)})));
        assert_eq!(line.chars().count(), "Bash: ".len() + MAX_TOOL_DETAIL_CHARS);
        assert!(line.ends_with('…'));
    }
}

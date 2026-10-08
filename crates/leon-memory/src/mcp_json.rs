//! The `leon-memory` entry of a project's `.mcp.json`.
//!
//! `.mcp.json` at a project's root is where Claude Code reads the MCP servers
//! of that project. [`add`] merges one entry into it and [`remove`] takes it
//! out again; every other key stays, in its order, and the file keeps its
//! indentation, its line ending and its final newline. A file that does not
//! parse, or whose shape is not what is expected, is refused with a
//! [`McpJsonError`] instead of being written over.
//!
//! Like the managed block, the entry is the same on every computer (the file
//! is usually committed): the command is `${LEON_BIN:-leon}`, which Claude
//! Code expands from the environment Leon sets in its terminals, and which
//! falls back to a `leon` on the `PATH` elsewhere.
//!
//! What cannot be kept is what JSON does not carry: a file that was not
//! formatted the way a pretty-printer formats it comes back pretty-printed.

use serde_json::{json, Map, Value};
use thiserror::Error;

use crate::ENV_BIN;

/// The name of the file, at a project's root.
pub const FILE: &str = ".mcp.json";

/// The name the server is registered under.
pub const SERVER: &str = "leon-memory";

/// The key that holds the servers.
const SERVERS: &str = "mcpServers";

/// Why a `.mcp.json` was not touched.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum McpJsonError {
    /// The file is not JSON.
    #[error("it is not valid JSON ({0})")]
    NotJson(String),
    /// The file is JSON but not an object.
    #[error("it is not a JSON object")]
    NotAnObject,
    /// `mcpServers` is there but is not an object.
    #[error("its \"mcpServers\" is not an object")]
    ServersNotAnObject,
}

/// What to do with the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Nothing: it already is as asked.
    Unchanged,
    /// Write this text.
    Write(String),
    /// Remove the file: the entry was all it held.
    Delete,
}

/// The entry: how the server is started.
pub fn entry() -> Value {
    json!({
        "command": format!("${{{ENV_BIN}:-leon}}"),
        "args": ["memory", "mcp"],
    })
}

fn parse(text: &str) -> Result<Map<String, Value>, McpJsonError> {
    // An empty file is a file with nothing registered.
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(McpJsonError::NotAnObject),
        Err(error) => Err(McpJsonError::NotJson(error.to_string())),
    }
}

/// The white space one level is indented by: what the first indented line of
/// the text starts with, two spaces when there is none.
fn indent_of(text: &str) -> String {
    text.lines()
        .find_map(|line| {
            let indent = &line[..line.len() - line.trim_start().len()];
            (!indent.is_empty() && indent.len() < line.len()).then(|| indent.to_owned())
        })
        .unwrap_or_else(|| "  ".to_owned())
}

/// The object as text, formatted like `like` (or with two spaces, `\n` and a
/// final newline when there was no file).
fn write(map: Map<String, Value>, like: Option<&str>) -> String {
    use serde_json::ser::{PrettyFormatter, Serializer};
    let indent = like.map_or_else(|| "  ".to_owned(), indent_of);
    let mut bytes = Vec::new();
    let formatter = PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = Serializer::with_formatter(&mut bytes, formatter);
    serde::Serialize::serialize(&Value::Object(map), &mut serializer)
        .expect("a JSON value is written to memory");
    let mut text = String::from_utf8(bytes).expect("JSON is UTF-8");
    if like.is_none_or(|like| like.is_empty() || like.ends_with('\n')) {
        text.push('\n');
    }
    if like.is_some_and(|like| like.contains("\r\n")) {
        text = text.replace('\n', "\r\n");
    }
    text
}

/// Whether the file's text registers the server.
pub fn has(text: Option<&str>) -> Result<bool, McpJsonError> {
    let Some(text) = text else {
        return Ok(false);
    };
    match parse(text)?.get(SERVERS) {
        None => Ok(false),
        Some(Value::Object(servers)) => Ok(servers.contains_key(SERVER)),
        Some(_) => Err(McpJsonError::ServersNotAnObject),
    }
}

/// The file with the server registered. `text` is what the file holds, or
/// `None` when there is none.
pub fn add(text: Option<&str>) -> Result<Edit, McpJsonError> {
    let mut map = text.map(parse).transpose()?.unwrap_or_default();
    let servers = map
        .entry(SERVERS)
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(servers) = servers else {
        return Err(McpJsonError::ServersNotAnObject);
    };
    if servers.get(SERVER) == Some(&entry()) {
        return Ok(Edit::Unchanged);
    }
    servers.insert(SERVER.to_owned(), entry());
    Ok(Edit::Write(write(map, text)))
}

/// The file without the server. A file that is exactly what [`add`] makes
/// where there was none is removed; any other keeps everything else it holds,
/// an `mcpServers` left empty included. Two things JSON has no place to
/// remember therefore do not come back: a file without an `mcpServers` key
/// keeps an empty one, and a file that held no server and no other key is
/// removed like the one Leon makes.
pub fn remove(text: Option<&str>) -> Result<Edit, McpJsonError> {
    let Some(text) = text else {
        return Ok(Edit::Unchanged);
    };
    let mut map = parse(text)?;
    let servers = match map.get_mut(SERVERS) {
        None => return Ok(Edit::Unchanged),
        Some(Value::Object(servers)) => servers,
        Some(_) => return Err(McpJsonError::ServersNotAnObject),
    };
    if servers.shift_remove(SERVER).is_none() {
        return Ok(Edit::Unchanged);
    }
    // The file Leon made, untouched since: it goes as it came.
    if add(None) == Ok(Edit::Write(text.to_owned())) {
        return Ok(Edit::Delete);
    }
    Ok(Edit::Write(write(map, Some(text))))
}

/// The lines that register the server with Codex, for a Leon at `bin`.
pub fn codex_lines(bin: &str) -> String {
    format!(
        "codex mcp add {SERVER} -- {} memory mcp\n\
         or, in ~/.codex/config.toml:\n\n\
         [mcp_servers.{SERVER}]\n\
         command = {}\n\
         args = [\"memory\", \"mcp\"]\n",
        shell_word(bin),
        Value::String(bin.to_owned()),
    )
}

/// The lines that register the server with opencode, for a Leon at `bin`.
pub fn opencode_lines(bin: &str) -> String {
    let snippet = json!({
        "mcp": { SERVER: { "type": "local", "command": [bin, "memory", "mcp"], "enabled": true } }
    });
    format!(
        "in opencode.json (the project's, or ~/.config/opencode/opencode.json):\n\n{}\n",
        serde_json::to_string_pretty(&snippet).expect("a JSON value is written to memory")
    )
}

/// A word as a POSIX shell takes it.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.:/@%+,".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(edit: Edit) -> String {
        match edit {
            Edit::Write(text) => text,
            other => panic!("expected a write, got {other:?}"),
        }
    }

    #[test]
    fn a_project_without_the_file_gets_one_with_the_entry_alone() {
        let text = written(add(None).unwrap());
        assert_eq!(
            text,
            "{\n  \"mcpServers\": {\n    \"leon-memory\": {\n      \"command\": \"${LEON_BIN:-leon}\",\n      \"args\": [\n        \"memory\",\n        \"mcp\"\n      ]\n    }\n  }\n}\n"
        );
        assert_eq!(has(Some(&text)), Ok(true));
        assert_eq!(has(None), Ok(false));
        // And removing it removes the file.
        assert_eq!(remove(Some(&text)), Ok(Edit::Delete));
    }

    #[test]
    fn the_entry_names_no_path() {
        let entry = entry().to_string();
        assert!(!entry.contains('/'), "{entry}");
        assert!(entry.contains("${LEON_BIN:-leon}"));
    }

    #[test]
    fn every_other_key_keeps_its_place_and_removing_gives_the_file_back() {
        for before in [
            "{\n  \"mcpServers\": {\n    \"zeta\": {\n      \"command\": \"z\"\n    },\n    \"alpha\": {\n      \"url\": \"https://example.com/mcp\"\n    }\n  },\n  \"$schema\": \"x\",\n  \"about\": [\n    1,\n    2\n  ]\n}\n",
            // Four spaces, no final newline.
            "{\n    \"z\": true,\n    \"mcpServers\": {\n        \"other\": {\n            \"command\": \"o\"\n        }\n    },\n    \"a\": null\n}",
            // Tabs and CRLF.
            "{\r\n\t\"mcpServers\": {\r\n\t\t\"other\": {\r\n\t\t\t\"command\": \"o\"\r\n\t\t}\r\n\t}\r\n}\r\n",
            // A file whose servers were all taken out before.
            "{\n  \"mcpServers\": {},\n  \"note\": \"ours\"\n}\n",
        ] {
            let with = written(add(Some(before)).unwrap());
            assert_eq!(has(Some(&with)), Ok(true), "{before}");
            assert!(with.contains("\"leon-memory\""));
            assert_eq!(add(Some(&with)), Ok(Edit::Unchanged), "{before}");
            assert_eq!(written(remove(Some(&with)).unwrap()), before);
        }
    }

    #[test]
    fn a_file_without_servers_keeps_an_empty_list_of_them() {
        // The one thing that does not come back as it was: whether the key
        // was there is not kept anywhere.
        let before = "{\n  \"note\": \"ours\"\n}\n";
        let with = written(add(Some(before)).unwrap());
        assert_eq!(
            written(remove(Some(&with)).unwrap()),
            "{\n  \"note\": \"ours\",\n  \"mcpServers\": {}\n}\n"
        );
        // A file that held no server and nothing else becomes, with the
        // entry, exactly the file Leon makes: it goes like one.
        for empty in ["{}\n", "{\n  \"mcpServers\": {}\n}\n"] {
            let with = written(add(Some(empty)).unwrap());
            assert_eq!(remove(Some(&with)), Ok(Edit::Delete), "{empty}");
        }
    }

    #[test]
    fn the_entry_goes_last_among_the_servers() {
        let before = "{\n  \"mcpServers\": {\n    \"zeta\": {}\n  }\n}\n";
        let with = written(add(Some(before)).unwrap());
        assert!(with.find("zeta").unwrap() < with.find("leon-memory").unwrap());
    }

    #[test]
    fn an_entry_somebody_changed_is_put_right_and_only_ours_is_removed() {
        let stale = "{\n  \"mcpServers\": {\n    \"leon-memory\": {\n      \"command\": \"/old/leon\"\n    },\n    \"other\": {}\n  }\n}\n";
        let with = written(add(Some(stale)).unwrap());
        assert!(with.contains("${LEON_BIN:-leon}"));
        assert!(!with.contains("/old/leon"));
        let without = written(remove(Some(&with)).unwrap());
        assert_eq!(
            without,
            "{\n  \"mcpServers\": {\n    \"other\": {}\n  }\n}\n"
        );
        assert_eq!(remove(Some(&without)), Ok(Edit::Unchanged));
        assert_eq!(remove(None), Ok(Edit::Unchanged));
    }

    #[test]
    fn a_file_that_is_not_what_is_expected_is_refused_not_replaced() {
        for (text, expected) in [
            ("{ not json", "not valid JSON"),
            ("{\"mcpServers\": {},}", "not valid JSON"),
            ("// a comment\n{}", "not valid JSON"),
            ("[]", "not a JSON object"),
            ("\"text\"", "not a JSON object"),
            ("{\"mcpServers\": []}", "\"mcpServers\" is not an object"),
            ("{\"mcpServers\": \"x\"}", "\"mcpServers\" is not an object"),
        ] {
            let refused = add(Some(text)).unwrap_err().to_string();
            assert!(refused.contains(expected), "{text}: {refused}");
            assert!(remove(Some(text)).is_err(), "{text}");
            assert!(has(Some(text)).is_err(), "{text}");
        }
    }

    #[test]
    fn an_empty_file_is_a_file_with_nothing_registered() {
        assert_eq!(has(Some("")), Ok(false));
        let with = written(add(Some("")).unwrap());
        assert!(with.ends_with("}\n"));
        assert_eq!(remove(Some("\n")), Ok(Edit::Unchanged));
    }

    #[test]
    fn the_lines_for_the_other_agents_name_the_program_they_are_given() {
        let codex = codex_lines("/opt/My Apps/leon");
        assert!(codex.contains("codex mcp add leon-memory -- '/opt/My Apps/leon' memory mcp"));
        assert!(codex.contains("[mcp_servers.leon-memory]\ncommand = \"/opt/My Apps/leon\"\n"));
        let opencode = opencode_lines("C:\\Leon\\leon.exe");
        assert!(opencode.contains("\"C:\\\\Leon\\\\leon.exe\""));
        assert!(opencode.contains("\"type\": \"local\""));
    }
}

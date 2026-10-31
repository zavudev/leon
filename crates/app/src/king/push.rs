//! King's notices on a phone whose application is closed: Expo's push
//! service.
//!
//! The relay carries King's notices only to an application that is open. A
//! phone that wants them while it is closed gives King's service its Expo
//! push token ([`leon_king::protocol::Request::PushRegister`], from the
//! phone's settings, off until the person turns it on). When King tells the
//! person something and no client shows King, the service sends the title and
//! the body to `exp.host`, Expo's push service, which hands them to Apple's
//! or Google's for the phone. That is the one thing that leaves the
//! computer: the title and the body of the notice, with the token.
//!
//! The tokens are kept in `king/push.json`. A token Expo says is no longer
//! registered is dropped.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Expo's push service.
const PUSH_URL: &str = "https://exp.host/--/api/v2/push/send";
/// The most phones kept.
const MAX_DEVICES: usize = 10;
/// The longest token taken.
const MAX_TOKEN: usize = 200;
/// The longest title and body sent, in characters.
const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 1000;

/// A phone that asked for King's notices while closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Its Expo push token.
    pub token: String,
    /// What it calls itself.
    pub name: String,
    /// When it asked, in milliseconds since the epoch.
    pub since_ms: u64,
}

/// Whether `token` is an Expo push token.
pub fn valid_token(token: &str) -> bool {
    token.len() <= MAX_TOKEN
        && ["ExponentPushToken[", "ExpoPushToken["]
            .iter()
            .any(|start| token.starts_with(start))
        && token.ends_with(']')
        && !token.chars().any(char::is_control)
}

/// The phones of `king/push.json`.
pub struct Devices {
    path: PathBuf,
    list: Vec<Device>,
}

impl Devices {
    /// The phones kept in King's `folder`.
    pub fn load(folder: &Path) -> Self {
        let path = folder.join("push.json");
        let list = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, list }
    }

    /// The phones.
    pub fn list(&self) -> &[Device] {
        &self.list
    }

    /// Keeps `token`, once; the newest phones are kept.
    pub fn add(&mut self, token: &str, name: &str, now_ms: u64) -> Result<(), String> {
        if !valid_token(token) {
            return Err("that is not an Expo push token".to_owned());
        }
        self.list.retain(|device| device.token != token);
        self.list.push(Device {
            token: token.to_owned(),
            name: name.chars().filter(|c| !c.is_control()).take(80).collect(),
            since_ms: now_ms,
        });
        let excess = self.list.len().saturating_sub(MAX_DEVICES);
        self.list.drain(..excess);
        self.save();
        Ok(())
    }

    /// Forgets `token`. Whether it was kept.
    pub fn remove(&mut self, token: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|device| device.token != token);
        let removed = self.list.len() != before;
        if removed {
            self.save();
        }
        removed
    }

    fn save(&self) {
        let Ok(text) = serde_json::to_string_pretty(&self.list) else {
            return;
        };
        if let Err(error) = std::fs::write(&self.path, text) {
            tracing::warn!(%error, "the phones for Orchestrator's notices could not be saved");
        }
    }
}

/// What Expo is sent for one notice to `tokens`.
pub fn messages(tokens: &[String], title: &str, body: &str) -> Value {
    let cut = |text: &str, most: usize| -> String {
        let mut cut: String = text.chars().take(most).collect();
        if text.chars().count() > most {
            cut.push('…');
        }
        cut
    };
    Value::Array(
        tokens
            .iter()
            .map(|token| {
                json!({
                    "to": token,
                    "title": cut(title, MAX_TITLE),
                    "body": cut(body, MAX_BODY),
                    "sound": "default",
                    "data": { "king": true },
                })
            })
            .collect(),
    )
}

/// The tokens Expo's answer says are no longer registered: each ticket is
/// in the order of the messages.
pub fn gone(tokens: &[String], answer: &str) -> Vec<String> {
    let Ok(answer) = serde_json::from_str::<Value>(answer) else {
        return Vec::new();
    };
    answer["data"]
        .as_array()
        .into_iter()
        .flatten()
        .zip(tokens)
        .filter(|(ticket, _)| ticket["details"]["error"].as_str() == Some("DeviceNotRegistered"))
        .map(|(_, token)| token.clone())
        .collect()
}

/// Sends `messages` to Expo and gives its answer. Blocks: run it on a
/// thread of its own.
pub fn send(messages: &Value) -> Result<String, String> {
    let mut child = leon_remote::spawn::std_child("curl")
        .args([
            "--silent",
            "--show-error",
            "--max-time",
            "15",
            "--header",
            "Content-Type: application/json",
            "--header",
            "Accept: application/json",
            "--data-binary",
            "@-",
            PUSH_URL,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("curl could not start: {error}"))?;
    if let Some(mut input) = child.stdin.take() {
        input
            .write_all(messages.to_string().as_bytes())
            .map_err(|error| error.to_string())?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_expo_token_is_kept_and_the_newest_phones_stay() {
        assert!(valid_token("ExponentPushToken[xxxxxxxxxxxxxxxxxxxxxx]"));
        assert!(valid_token("ExpoPushToken[abc]"));
        assert!(!valid_token("https://evil.example/hook"));
        assert!(!valid_token("ExponentPushToken[a\n]"));
        let folder = tempfile::tempdir().unwrap();
        let mut devices = Devices::load(folder.path());
        assert!(devices.add("nope", "phone", 1).is_err());
        for n in 0..12 {
            devices
                .add(&format!("ExpoPushToken[{n}]"), "phone", n)
                .unwrap();
        }
        devices.add("ExpoPushToken[11]", "again", 20).unwrap();
        assert_eq!(devices.list().len(), MAX_DEVICES);
        assert_eq!(devices.list().last().unwrap().name, "again");
        assert_eq!(Devices::load(folder.path()).list(), devices.list());
        assert!(devices.remove("ExpoPushToken[11]"));
        assert!(!devices.remove("ExpoPushToken[11]"));
    }

    #[test]
    fn a_notice_goes_to_every_phone_and_a_gone_phone_is_told_apart() {
        let tokens = vec!["ExpoPushToken[a]".to_owned(), "ExpoPushToken[b]".to_owned()];
        let sent = messages(&tokens, "Done", &"x".repeat(2000));
        assert_eq!(sent.as_array().unwrap().len(), 2);
        assert_eq!(sent[1]["to"], "ExpoPushToken[b]");
        assert_eq!(
            sent[0]["body"].as_str().unwrap().chars().count(),
            MAX_BODY + 1
        );
        let answer = r#"{"data":[{"status":"ok","id":"1"},
            {"status":"error","details":{"error":"DeviceNotRegistered"}}]}"#;
        assert_eq!(gone(&tokens, answer), ["ExpoPushToken[b]"]);
        assert!(gone(&tokens, "not json").is_empty());
    }
}

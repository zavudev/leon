//! Small text helpers for what the UI prints: ages, tags and truncation. All pure.

use chrono::{DateTime, Local, Utc};
use leon_core::{AgentId, Role};

/// How long ago `then` was, as one short token: `now`, `5m`, `3h`, `2d`,
/// `6w`, `1y`. A time in the future reads as `now`.
pub fn age(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let seconds = (now - then).num_seconds().max(0);
    match seconds {
        0..=59 => "now".to_owned(),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        86_400..=604_799 => format!("{}d", seconds / 86_400),
        604_800..=31_535_999 => format!("{}w", seconds / 604_800),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

/// A moment in the user's time zone, to the minute.
pub fn moment(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// The first seven characters of a commit id.
pub fn short_head(head: &str) -> &str {
    let end = head
        .char_indices()
        .nth(7)
        .map_or(head.len(), |(index, _)| index);
    &head[..end]
}

/// The agent's name in the mono labels.
pub fn agent_tag(agent: AgentId) -> &'static str {
    agent.tag()
}

/// The agent's name in running text.
pub fn agent_name(agent: AgentId) -> &'static str {
    agent.name()
}

/// The speaker's name in a transcript's mono labels.
pub fn role_tag(role: Role) -> &'static str {
    match role {
        Role::User => "USER",
        Role::Assistant => "ASSISTANT",
        Role::Tool => "TOOL",
        Role::System => "SYSTEM",
    }
}

/// `text` cut to at most `max` characters, and how many characters were left
/// out. Cuts on a character boundary.
pub fn truncate_chars(text: &str, max: usize) -> (&str, usize) {
    match text.char_indices().nth(max) {
        Some((end, _)) => (&text[..end], text[end..].chars().count()),
        None => (text, 0),
    }
}

/// An OSC title without the decorative glyph a program puts before it (Claude
/// Code's `✳`, a braille spinner) and the space after it. A title that starts
/// with a letter, a digit or ASCII punctuation is left as it is, and the result
/// is never empty: a title made of nothing but glyphs stays whole.
pub fn plain_title(title: &str) -> String {
    let trimmed = title.trim();
    let rest = trimmed
        .trim_start_matches(|c: char| c.is_whitespace() || (!c.is_ascii() && !c.is_alphanumeric()))
        .trim_start();
    if rest.is_empty() {
        trimmed.to_owned()
    } else {
        rest.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn at(seconds_ago: i64) -> (DateTime<Utc>, DateTime<Utc>) {
        let now = Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap();
        (now, now - Duration::seconds(seconds_ago))
    }

    #[test]
    fn ages_are_one_short_token() {
        for (seconds, expected) in [
            (0, "now"),
            (59, "now"),
            (60, "1m"),
            (3_599, "59m"),
            (3_600, "1h"),
            (86_399, "23h"),
            (86_400, "1d"),
            (604_799, "6d"),
            (604_800, "1w"),
            (31_535_999, "52w"),
            (31_536_000, "1y"),
        ] {
            let (now, then) = at(seconds);
            assert_eq!(age(now, then), expected, "{seconds}s");
        }
    }

    #[test]
    fn a_time_in_the_future_reads_as_now() {
        let (now, then) = at(-500);
        assert_eq!(age(now, then), "now");
    }

    #[test]
    fn a_commit_id_is_cut_to_seven_characters() {
        assert_eq!(short_head("2f1c9a07bd5e"), "2f1c9a0");
        assert_eq!(short_head("abc"), "abc");
        assert_eq!(short_head(""), "");
    }

    #[test]
    fn truncation_counts_what_it_cut_and_respects_characters() {
        assert_eq!(truncate_chars("hello", 10), ("hello", 0));
        assert_eq!(truncate_chars("hello", 3), ("hel", 2));
        assert_eq!(truncate_chars("héllo", 2), ("hé", 3));
        assert_eq!(truncate_chars("", 3), ("", 0));
    }

    #[test]
    fn every_agent_and_role_has_a_tag() {
        for spec in leon_core::agent::builtin() {
            let agent = spec.id;
            assert!(!agent_tag(agent).is_empty());
            assert!(!agent_name(agent).is_empty());
        }
        for role in [Role::User, Role::Assistant, Role::Tool, Role::System] {
            assert_eq!(role_tag(role), role.as_str().to_uppercase());
        }
    }

    #[test]
    fn a_leading_decorative_glyph_is_stripped_from_a_title() {
        assert_eq!(
            plain_title("\u{2733} Google Ads análisis"),
            "Google Ads análisis"
        );
        assert_eq!(plain_title("\u{2802}  working"), "working");
        assert_eq!(plain_title("  \u{2733}\u{2733}   x "), "x");
    }

    #[test]
    fn a_title_that_starts_with_a_letter_a_digit_or_punctuation_is_untouched() {
        for title in [
            "Fix the bug",
            "3 files",
            "Ñandú",
            "~/code",
            "[wip] x",
            "$ ls",
            "日本語",
        ] {
            assert_eq!(plain_title(title), title);
        }
    }

    #[test]
    fn a_title_of_nothing_but_glyphs_is_never_made_empty() {
        assert_eq!(plain_title("\u{2733}"), "\u{2733}");
        assert_eq!(plain_title(" \u{2802}\u{2810} "), "\u{2802}\u{2810}");
        assert_eq!(plain_title(""), "");
    }
}

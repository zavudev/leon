//! What a worktree row and the worktree screen say about the state of a
//! checkout.
//!
//! Pure functions of a [`WorktreeStatus`], so what is said, in which words
//! and in which order is tested without a window. A row has little room: it
//! says the open pull request (`#123`), the changed files (`~3`) and the
//! distance from the upstream (`↑2↓1`), and on a narrow sidebar only the
//! first of those that is known. The screen says all of it in words.

use leon_core::{Checks, PullRequest, Review, WorktreeStatus};

/// How a part is coloured; the view maps each to a token of the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Information that needs no attention.
    Muted,
    /// Something to look at: changes, a check running, being behind.
    Warning,
    /// Good news: checks passing, an approval.
    Good,
    /// Bad news: a check failed, changes were requested.
    Bad,
    /// A draft: present and not asking for anything.
    Faint,
}

/// One piece of text of a worktree row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// What it says, short.
    pub text: String,
    /// How it is coloured.
    pub tone: Tone,
}

/// The tone of a pull request: its checks, unless it is a draft.
pub fn pull_request_tone(pull_request: &PullRequest) -> Tone {
    if pull_request.draft {
        return Tone::Faint;
    }
    match pull_request.checks {
        Checks::Failing => Tone::Bad,
        Checks::Running => Tone::Warning,
        Checks::Passing => Tone::Good,
        Checks::None => Tone::Muted,
    }
}

/// The parts a worktree row shows, most important first. With `roomy` false
/// (a narrow sidebar) only the first remains: the full account is the
/// tooltip's and the screen's.
pub fn row_parts(status: &WorktreeStatus, roomy: bool) -> Vec<Part> {
    let mut parts = Vec::new();
    if let Some(pull_request) = &status.pull_request {
        parts.push(Part {
            text: format!("#{}", pull_request.number),
            tone: pull_request_tone(pull_request),
        });
    }
    if let Some(changed) = status.changed.filter(|changed| *changed > 0) {
        parts.push(Part {
            text: format!("~{changed}"),
            tone: Tone::Warning,
        });
    }
    if let Some(divergence) = status
        .divergence
        .filter(|divergence| !divergence.is_level())
    {
        let mut text = String::new();
        if divergence.ahead > 0 {
            text.push_str(&format!("\u{2191}{}", divergence.ahead));
        }
        if divergence.behind > 0 {
            text.push_str(&format!("\u{2193}{}", divergence.behind));
        }
        parts.push(Part {
            text,
            tone: if divergence.behind > 0 {
                Tone::Warning
            } else {
                Tone::Muted
            },
        });
    }
    if !roomy {
        parts.truncate(1);
    }
    parts
}

/// "3 files changed", "1 file changed" or "No changes".
pub fn changes_words(changed: u32) -> String {
    match changed {
        0 => "No changes".to_owned(),
        1 => "1 file changed".to_owned(),
        n => format!("{n} files changed"),
    }
}

/// The distance from the upstream in words.
pub fn upstream_words(ahead: u32, behind: u32) -> String {
    let commits = |n: u32| {
        if n == 1 {
            "1 commit".to_owned()
        } else {
            format!("{n} commits")
        }
    };
    match (ahead, behind) {
        (0, 0) => "Level with its upstream".to_owned(),
        (ahead, 0) => format!("{} ahead of its upstream", commits(ahead)),
        (0, behind) => format!("{} behind its upstream", commits(behind)),
        (ahead, behind) => format!(
            "{} ahead, {} behind its upstream",
            commits(ahead),
            commits(behind)
        ),
    }
}

/// The review of a pull request in words; `None` when there is nothing to say.
pub fn review_words(review: Review) -> Option<&'static str> {
    match review {
        Review::None => None,
        Review::Required => Some("Review required"),
        Review::Approved => Some("Approved"),
        Review::ChangesRequested => Some("Changes requested"),
    }
}

/// The checks of a pull request in words; `None` when it has none.
pub fn checks_words(checks: Checks) -> Option<&'static str> {
    match checks {
        Checks::None => None,
        Checks::Passing => Some("Checks passing"),
        Checks::Failing => Some("Checks failing"),
        Checks::Running => Some("Checks running"),
    }
}

/// The pull request on one line: `#123 · Draft · Approved · Checks passing`.
pub fn pull_request_words(pull_request: &PullRequest) -> String {
    let mut words = vec![format!("#{}", pull_request.number)];
    if pull_request.draft {
        words.push("Draft".to_owned());
    }
    words.extend(review_words(pull_request.review).map(str::to_owned));
    words.extend(checks_words(pull_request.checks).map(str::to_owned));
    words.join(" \u{b7} ")
}

/// The row's tooltip: everything known, one thing per line; `None` when
/// nothing is.
pub fn row_tip(status: &WorktreeStatus) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(pull_request) = &status.pull_request {
        lines.push(format!("Pull request {}", pull_request_words(pull_request)));
    }
    if let Some(changed) = status.changed {
        lines.push(changes_words(changed));
    }
    if let Some(divergence) = status.divergence {
        lines.push(upstream_words(divergence.ahead, divergence.behind));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Only an address that opens in a browser is opened: what `gh` prints is
/// data, and a worktree row is not a place to run anything it says.
pub fn openable_url(pull_request: &PullRequest) -> Option<&str> {
    let url = pull_request.url.trim();
    url.starts_with("https://").then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::Divergence;

    fn pull_request(draft: bool, review: Review, checks: Checks) -> PullRequest {
        PullRequest {
            number: 123,
            url: "https://github.com/zavudev/leon/pull/123".into(),
            draft,
            review,
            checks,
        }
    }

    fn busy() -> WorktreeStatus {
        WorktreeStatus {
            changed: Some(3),
            divergence: Some(Divergence {
                ahead: 2,
                behind: 1,
            }),
            pull_request: Some(pull_request(false, Review::Approved, Checks::Failing)),
        }
    }

    fn texts(parts: &[Part]) -> Vec<&str> {
        parts.iter().map(|part| part.text.as_str()).collect()
    }

    #[test]
    fn a_roomy_row_says_the_pull_request_the_changes_and_the_distance() {
        let parts = row_parts(&busy(), true);
        assert_eq!(texts(&parts), ["#123", "~3", "\u{2191}2\u{2193}1"]);
        assert_eq!(
            parts[0].tone,
            Tone::Bad,
            "the failing check colours the pull request"
        );
        assert_eq!(parts[1].tone, Tone::Warning);
        assert_eq!(parts[2].tone, Tone::Warning, "being behind is worth a look");
    }

    #[test]
    fn a_narrow_row_keeps_only_the_most_important_part() {
        assert_eq!(texts(&row_parts(&busy(), false)), ["#123"]);
        let mut no_pull_request = busy();
        no_pull_request.pull_request = None;
        assert_eq!(texts(&row_parts(&no_pull_request, false)), ["~3"]);
    }

    #[test]
    fn a_clean_level_checkout_says_nothing_on_its_row() {
        let level = WorktreeStatus {
            changed: Some(0),
            divergence: Some(Divergence::default()),
            pull_request: None,
        };
        assert!(row_parts(&level, true).is_empty());
        assert!(row_parts(&WorktreeStatus::default(), true).is_empty());
        assert_eq!(row_tip(&WorktreeStatus::default()), None);
        assert_eq!(
            row_tip(&level).as_deref(),
            Some("No changes\nLevel with its upstream")
        );
    }

    #[test]
    fn only_ahead_is_quiet_and_only_behind_is_not() {
        let of = |ahead, behind| {
            row_parts(
                &WorktreeStatus {
                    divergence: Some(Divergence { ahead, behind }),
                    ..Default::default()
                },
                true,
            )
        };
        assert_eq!(texts(&of(4, 0)), ["\u{2191}4"]);
        assert_eq!(of(4, 0)[0].tone, Tone::Muted);
        assert_eq!(texts(&of(0, 2)), ["\u{2193}2"]);
    }

    #[test]
    fn a_pull_request_is_coloured_by_its_checks_unless_it_is_a_draft() {
        let tone = |draft, checks| pull_request_tone(&pull_request(draft, Review::None, checks));
        assert_eq!(tone(false, Checks::Passing), Tone::Good);
        assert_eq!(tone(false, Checks::Running), Tone::Warning);
        assert_eq!(tone(false, Checks::Failing), Tone::Bad);
        assert_eq!(tone(false, Checks::None), Tone::Muted);
        assert_eq!(tone(true, Checks::Failing), Tone::Faint);
    }

    #[test]
    fn the_screen_says_it_in_plain_words() {
        assert_eq!(changes_words(0), "No changes");
        assert_eq!(changes_words(1), "1 file changed");
        assert_eq!(changes_words(12), "12 files changed");
        assert_eq!(upstream_words(0, 0), "Level with its upstream");
        assert_eq!(upstream_words(1, 0), "1 commit ahead of its upstream");
        assert_eq!(upstream_words(0, 3), "3 commits behind its upstream");
        assert_eq!(
            upstream_words(2, 1),
            "2 commits ahead, 1 commit behind its upstream"
        );
        assert_eq!(
            pull_request_words(&pull_request(
                true,
                Review::ChangesRequested,
                Checks::Running
            )),
            "#123 \u{b7} Draft \u{b7} Changes requested \u{b7} Checks running"
        );
        assert_eq!(
            pull_request_words(&pull_request(false, Review::None, Checks::None)),
            "#123"
        );
    }

    #[test]
    fn only_a_secure_web_address_is_opened() {
        let mut pr = pull_request(false, Review::None, Checks::None);
        assert!(openable_url(&pr).is_some());
        pr.url = "file:///etc/passwd".into();
        assert_eq!(openable_url(&pr), None);
        pr.url = String::new();
        assert_eq!(openable_url(&pr), None);
    }
}

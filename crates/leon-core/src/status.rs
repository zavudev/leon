//! What is known about the state of a worktree's checkout.
//!
//! [`WorktreeStatus`] is the one reusable answer to "how is this worktree
//! doing": how many files changed, how far it is from its upstream and the
//! open pull request of its branch with the summary of its checks. The engine
//! reads it from git and from `gh` on the worktree's machine and writes it to
//! the store ([`Store::update_worktree_status`](crate::Store::update_worktree_status));
//! the views only read it back. Every part is optional because every part can
//! be unknown: a part nobody could read is `None`, never "zero" or "none".

use serde::{Deserialize, Serialize};

/// How far a checkout is from its upstream branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Divergence {
    /// Commits here that the upstream does not have.
    pub ahead: u32,
    /// Commits in the upstream that are not here.
    pub behind: u32,
}

impl Divergence {
    /// Whether the checkout and its upstream are at the same commit.
    pub fn is_level(&self) -> bool {
        self.ahead == 0 && self.behind == 0
    }
}

/// Where the reviews of a pull request stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Review {
    /// Nobody was asked for a review, or GitHub reports none.
    #[default]
    None,
    /// A review is required and not given yet.
    Required,
    /// The reviewers approved.
    Approved,
    /// A reviewer asked for changes.
    ChangesRequested,
}

/// The summary of the checks run on a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Checks {
    /// The pull request has no checks.
    #[default]
    None,
    /// Every check passed (or was skipped).
    Passing,
    /// At least one check failed.
    Failing,
    /// None failed and at least one has not finished.
    Running,
}

/// The open pull request of a worktree's branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    /// Its number on the repository (`#123`).
    pub number: u64,
    /// The address of its page, as GitHub printed it.
    pub url: String,
    /// Whether it is a draft.
    pub draft: bool,
    /// Where its reviews stand.
    pub review: Review,
    /// How its checks are doing.
    pub checks: Checks,
}

/// Everything known about the state of one worktree.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreeStatus {
    /// Files modified, added, removed or untracked, or `None` when git was not
    /// asked or could not answer.
    pub changed: Option<u32>,
    /// Distance from the upstream branch, or `None` when the branch has no
    /// upstream (or git could not say).
    pub divergence: Option<Divergence>,
    /// The open pull request of the branch, or `None` when there is none or
    /// GitHub could not be asked.
    pub pull_request: Option<PullRequest>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_survives_the_round_trip_the_store_gives_it() {
        let status = WorktreeStatus {
            changed: Some(3),
            divergence: Some(Divergence {
                ahead: 2,
                behind: 0,
            }),
            pull_request: Some(PullRequest {
                number: 12,
                url: "https://github.com/zavudev/leon/pull/12".into(),
                draft: true,
                review: Review::ChangesRequested,
                checks: Checks::Running,
            }),
        };
        let text = serde_json::to_string(&status).unwrap();
        assert_eq!(
            serde_json::from_str::<WorktreeStatus>(&text).unwrap(),
            status
        );
    }

    #[test]
    fn a_part_a_newer_version_wrote_and_this_one_lacks_reads_as_unknown() {
        assert_eq!(
            serde_json::from_str::<WorktreeStatus>("{}").unwrap(),
            WorktreeStatus::default()
        );
    }

    #[test]
    fn a_level_checkout_is_told_from_one_that_moved() {
        assert!(Divergence::default().is_level());
        assert!(!Divergence {
            ahead: 1,
            behind: 0
        }
        .is_level());
        assert!(!Divergence {
            ahead: 0,
            behind: 4
        }
        .is_level());
    }
}

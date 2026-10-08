//! Which merged worktrees are safe to remove, and why the others are not.
//!
//! A worktree is *offered* for cleanup when it is linked (never the main
//! worktree) and GitHub says its branch's pull request is merged
//! ([`Worktree::merged_pull_request`]). An offered worktree is *removable*
//! when nothing in it would be lost: no modified or untracked files, no commits
//! the upstream does not have, no program running in its folder, and its
//! checkout was read. [`blockers`] says what keeps one from being removed;
//! an empty answer is the removable case. Removal itself never forces: git
//! refuses a worktree with local changes, and the cleanup leaves it alone.
//!
//! [`newly_merged`] is the other question: which worktrees just became merged,
//! so that the window can offer the cleanup once, without removing anything.

use crate::ids::WorktreeId;
use crate::model::Worktree;
use crate::status::WorktreeStatus;

/// What keeps a merged worktree from being removed without a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// Its checkout was never read, so nothing is known about its files.
    NotRead,
    /// Git could not say whether its files have changed.
    ChangesUnknown,
    /// Modified, added or untracked files that removing it would delete.
    Changes(u32),
    /// Commits in it that the upstream does not have.
    Unpushed(u32),
    /// Programs running in its folder right now.
    Running(usize),
}

impl Blocker {
    /// What the blocker says to the person, in a few words.
    pub fn words(&self) -> String {
        match self {
            Blocker::NotRead => "its checkout has not been read yet".to_owned(),
            Blocker::ChangesUnknown => "git cannot say whether it has changes".to_owned(),
            Blocker::Changes(1) => "1 uncommitted change".to_owned(),
            Blocker::Changes(count) => format!("{count} uncommitted changes"),
            Blocker::Unpushed(1) => "1 commit not pushed".to_owned(),
            Blocker::Unpushed(count) => format!("{count} commits not pushed"),
            Blocker::Running(1) => "a session is running in it".to_owned(),
            Blocker::Running(count) => format!("{count} sessions are running in it"),
        }
    }
}

/// Whether the cleanup offers `worktree` at all: linked, and merged according
/// to GitHub. A worktree whose merge is not known is not offered.
pub fn offered(worktree: &Worktree) -> bool {
    !worktree.is_main && worktree.merged_pull_request == Some(true)
}

/// Everything that keeps a worktree from being removed; empty when it can be.
///
/// `status` is what was last read of its checkout and `running` is how many
/// sessions run in its folder. A status that is absent is "not read", never
/// clean. Unknown changes block, but an unknown upstream does not: after a
/// merge GitHub deletes the remote branch, git then says `[gone]`, which
/// leaves no count, and the commits of a merged pull request are on the
/// remote already. Only a known count of commits ahead is "not pushed".
pub fn blockers(status: Option<&WorktreeStatus>, running: usize) -> Vec<Blocker> {
    let mut found = Vec::new();
    match status {
        None => found.push(Blocker::NotRead),
        Some(status) => {
            match status.changed {
                None => found.push(Blocker::ChangesUnknown),
                Some(0) => {}
                Some(count) => found.push(Blocker::Changes(count)),
            }
            if let Some(divergence) = status.divergence {
                if divergence.ahead > 0 {
                    found.push(Blocker::Unpushed(divergence.ahead));
                }
            }
        }
    }
    if running > 0 {
        found.push(Blocker::Running(running));
    }
    found
}

/// The worktrees that became merged between two reads of the store: merged now,
/// and known to be unmerged before (`Some(false)`). A worktree first seen
/// merged is not among them, so a store that starts out knowing nothing does
/// not announce every merge it ever had. Worktrees that vanished are ignored.
pub fn newly_merged(before: &[Worktree], after: &[Worktree]) -> Vec<WorktreeId> {
    after
        .iter()
        .filter(|worktree| worktree.merged_pull_request == Some(true))
        .filter(|worktree| {
            before
                .iter()
                .find(|earlier| earlier.id == worktree.id)
                .is_some_and(|earlier| earlier.merged_pull_request == Some(false))
        })
        .map(|worktree| worktree.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ProjectId;
    use crate::status::Divergence;

    fn worktree(id: &str, is_main: bool, merged: Option<bool>) -> Worktree {
        Worktree {
            id: WorktreeId::from_string(id),
            project_id: ProjectId::from_string("p"),
            path: format!("/srv/{id}"),
            branch: Some(id.to_owned()),
            head: None,
            is_main,
            merged_pull_request: merged,
        }
    }

    fn clean() -> WorktreeStatus {
        WorktreeStatus {
            changed: Some(0),
            divergence: Some(Divergence::default()),
            pull_request: None,
        }
    }

    #[test]
    fn only_a_linked_worktree_whose_merge_github_reports_is_offered() {
        assert!(offered(&worktree("a", false, Some(true))));
        assert!(!offered(&worktree("a", false, Some(false))));
        assert!(!offered(&worktree("a", false, None)));
        assert!(!offered(&worktree("main", true, Some(true))));
    }

    #[test]
    fn a_clean_merged_checkout_with_nothing_running_is_removable() {
        assert!(blockers(Some(&clean()), 0).is_empty());
    }

    #[test]
    fn a_checkout_nobody_read_is_not_taken_for_a_clean_one() {
        assert_eq!(blockers(None, 0), [Blocker::NotRead]);
    }

    #[test]
    fn changes_git_reports_or_cannot_report_block_removal() {
        let changed = WorktreeStatus {
            changed: Some(3),
            ..clean()
        };
        assert_eq!(blockers(Some(&changed), 0), [Blocker::Changes(3)]);
        let unknown = WorktreeStatus {
            changed: None,
            ..clean()
        };
        assert_eq!(blockers(Some(&unknown), 0), [Blocker::ChangesUnknown]);
    }

    #[test]
    fn commits_the_upstream_does_not_have_block_removal() {
        let ahead = WorktreeStatus {
            divergence: Some(Divergence {
                ahead: 2,
                behind: 0,
            }),
            ..clean()
        };
        assert_eq!(blockers(Some(&ahead), 0), [Blocker::Unpushed(2)]);
    }

    #[test]
    fn a_branch_whose_remote_was_deleted_after_the_merge_is_not_blocked_by_it() {
        // `[gone]` reads as no divergence at all: the merged commits are on the
        // remote, so there is nothing known to lose.
        let gone = WorktreeStatus {
            divergence: None,
            ..clean()
        };
        assert!(blockers(Some(&gone), 0).is_empty());
        let behind = WorktreeStatus {
            divergence: Some(Divergence {
                ahead: 0,
                behind: 5,
            }),
            ..clean()
        };
        assert!(blockers(Some(&behind), 0).is_empty());
    }

    #[test]
    fn a_running_session_blocks_removal_and_every_reason_is_listed() {
        let busy = WorktreeStatus {
            changed: Some(1),
            divergence: Some(Divergence {
                ahead: 1,
                behind: 0,
            }),
            ..clean()
        };
        assert_eq!(
            blockers(Some(&busy), 2),
            [
                Blocker::Changes(1),
                Blocker::Unpushed(1),
                Blocker::Running(2)
            ]
        );
    }

    #[test]
    fn each_blocker_says_its_reason_in_words() {
        assert_eq!(Blocker::Changes(1).words(), "1 uncommitted change");
        assert_eq!(Blocker::Changes(4).words(), "4 uncommitted changes");
        assert_eq!(Blocker::Unpushed(1).words(), "1 commit not pushed");
        assert_eq!(Blocker::Running(1).words(), "a session is running in it");
        assert_eq!(Blocker::Running(3).words(), "3 sessions are running in it");
        assert_eq!(
            Blocker::NotRead.words(),
            "its checkout has not been read yet"
        );
    }

    #[test]
    fn a_worktree_that_turned_merged_since_the_last_read_is_newly_merged() {
        let before = [
            worktree("a", false, Some(false)),
            worktree("b", false, Some(true)),
            worktree("c", false, None),
        ];
        let after = [
            worktree("a", false, Some(true)),
            worktree("b", false, Some(true)),
            worktree("c", false, Some(true)),
        ];
        // `b` was merged already; `c` was never known open, so it does not count.
        assert_eq!(
            newly_merged(&before, &after),
            [WorktreeId::from_string("a")]
        );
    }

    #[test]
    fn a_first_read_announces_nothing_and_a_vanished_worktree_is_ignored() {
        let after = [worktree("a", false, Some(true))];
        assert!(newly_merged(&[], &after).is_empty());
        let before = [worktree("gone", false, Some(false))];
        assert!(newly_merged(&before, &after).is_empty());
    }
}

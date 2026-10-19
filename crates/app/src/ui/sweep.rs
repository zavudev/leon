//! The cleaner's rules: which worktrees and sessions it offers, which it
//! recommends, and how its lists are filtered and ordered.
//!
//! A linked worktree is *removable* when nothing in it would be lost
//! ([`cleanup::blockers`] is empty: no changes, no unpushed commits, nothing
//! running, its checkout read). Removing one runs `git worktree remove`, which
//! keeps its branch, so its commits stay in the repository. A removable
//! worktree is *recommended* when its pull request is merged or nobody worked
//! in it for [`IDLE_DAYS`]. The others are listed with what keeps them, and
//! cannot be ticked: the palette's "Remove a worktree" is the way to force one.
//!
//! An *old* session is one of the history that nobody touched for the age the
//! person picks, that is not pinned, not running and not on a shelf already.
//! The cleaner settles them (the Settled shelf holds them, and takes one back
//! with "Bring back"), it never deletes them.
//!
//! Everything here is a function of what it is given; the clock is an argument.

use crate::disk::DiskUse;
use crate::ui::model::Snapshot;
use crate::ui::tree::worktree_label;
use chrono::{DateTime, Utc};
use leon_core::cleanup::{self, Blocker};
use leon_core::{MachineId, ProjectId, Session, SessionId, Shelf, WorktreeId};
use std::collections::{HashMap, HashSet};

/// How long a worktree nobody worked in waits before it is recommended.
pub const IDLE_DAYS: i64 = 14;

/// Which of the two lists is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// The linked worktrees.
    #[default]
    Worktrees,
    /// The old sessions of the history.
    Sessions,
}

/// What a worktree's size is known as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    /// Being measured.
    Pending,
    /// Measured.
    Known(DiskUse),
    /// Could not be measured.
    Unknown,
}

impl Measure {
    /// Its bytes, when they are known.
    pub fn total(self) -> Option<u64> {
        match self {
            Measure::Known(usage) => Some(usage.total),
            _ => None,
        }
    }
}

/// A linked worktree, with everything the cleaner weighs.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// `project|worktree`, stable while the worktree exists.
    pub key: String,
    /// The project it belongs to.
    pub project: ProjectId,
    /// The worktree.
    pub worktree: WorktreeId,
    /// The machine it is on.
    pub machine: MachineId,
    /// The project's name.
    pub project_name: String,
    /// Its branch, or its folder's name.
    pub label: String,
    /// Its folder.
    pub path: String,
    /// Whether GitHub says its pull request is merged.
    pub merged: bool,
    /// How many sessions of the history ran in it.
    pub sessions: usize,
    /// The last time one of them did something.
    pub last_active: Option<DateTime<Utc>>,
    /// Whole days since then.
    pub idle_days: Option<i64>,
    /// What it takes on disk.
    pub size: Measure,
    /// What keeps it from being removed; empty when it can be.
    pub blockers: Vec<Blocker>,
}

impl Candidate {
    /// Whether removing it loses nothing.
    pub fn removable(&self) -> bool {
        self.blockers.is_empty()
    }

    /// Whether the cleaner suggests removing it.
    pub fn recommended(&self) -> bool {
        self.removable() && (self.merged || self.idle_days.is_some_and(|days| days >= IDLE_DAYS))
    }

    /// Why it is suggested, or what keeps it: a few words each.
    pub fn reasons(&self) -> Vec<String> {
        if !self.removable() {
            return self.blockers.iter().map(Blocker::words).collect();
        }
        let mut reasons = Vec::new();
        if self.merged {
            reasons.push("pull request merged".to_owned());
        }
        if let Some(days) = self.idle_days.filter(|days| *days >= IDLE_DAYS) {
            reasons.push(format!("idle {days} days"));
        }
        if reasons.is_empty() {
            reasons.push("clean, branch stays".to_owned());
        }
        reasons
    }
}

/// Every linked worktree of every project, with what the cleaner weighs.
///
/// `running` is how many sessions run in each worktree and `sizes` what was
/// measured; a worktree not measured yet is [`Measure::Pending`].
pub fn candidates(
    snapshot: &Snapshot,
    running: &HashMap<WorktreeId, usize>,
    sizes: &HashMap<WorktreeId, Measure>,
    now: DateTime<Utc>,
) -> Vec<Candidate> {
    snapshot
        .projects
        .iter()
        .flat_map(|entry| {
            entry
                .worktrees
                .iter()
                .filter(|worktree| !worktree.is_main)
                .map(move |worktree| (entry, worktree))
        })
        .map(|(entry, worktree)| {
            let machine = entry.project.machine_id.clone();
            let inside: Vec<&Session> = snapshot
                .sessions
                .iter()
                .filter(|session| {
                    session.machine_id == machine
                        && leon_core::path::is_within(&session.cwd, &worktree.path)
                })
                .collect();
            let last_active = inside.iter().map(|session| session.updated_at).max();
            Candidate {
                key: format!("{}|{}", entry.project.id, worktree.id),
                project: entry.project.id.clone(),
                worktree: worktree.id.clone(),
                machine,
                project_name: entry.project.name.clone(),
                label: worktree_label(worktree),
                path: worktree.path.clone(),
                merged: cleanup::offered(worktree),
                sessions: inside.len(),
                last_active,
                idle_days: last_active.map(|at| (now - at).num_days().max(0)),
                size: sizes.get(&worktree.id).copied().unwrap_or(Measure::Pending),
                blockers: cleanup::blockers(
                    snapshot.statuses.get(&worktree.id),
                    running.get(&worktree.id).copied().unwrap_or(0),
                ),
            }
        })
        .collect()
}

/// Which worktrees the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Show {
    /// All of them.
    #[default]
    All,
    /// The recommended ones.
    Recommended,
    /// The ones whose pull request is merged.
    Merged,
    /// The ones nobody worked in for [`IDLE_DAYS`].
    Idle,
    /// The ones that cannot be removed.
    Blocked,
}

impl Show {
    /// Every filter, in the order the chips show them.
    pub const ALL: [Show; 5] = [
        Show::All,
        Show::Recommended,
        Show::Merged,
        Show::Idle,
        Show::Blocked,
    ];

    /// What its chip says.
    pub fn label(self) -> &'static str {
        match self {
            Show::All => "ALL",
            Show::Recommended => "SUGGESTED",
            Show::Merged => "MERGED",
            Show::Idle => "IDLE",
            Show::Blocked => "KEPT",
        }
    }

    /// Whether `candidate` passes it.
    pub fn admits(self, candidate: &Candidate) -> bool {
        match self {
            Show::All => true,
            Show::Recommended => candidate.recommended(),
            Show::Merged => candidate.merged,
            Show::Idle => candidate.idle_days.is_some_and(|days| days >= IDLE_DAYS),
            Show::Blocked => !candidate.removable(),
        }
    }
}

/// How the worktrees are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// Heaviest first; the ones not measured yet last.
    #[default]
    Size,
    /// Longest idle first; the ones without sessions first of all.
    Idle,
    /// By project, then branch.
    Name,
}

impl Order {
    /// The next order, wrapping.
    pub fn next(self) -> Self {
        match self {
            Order::Size => Order::Idle,
            Order::Idle => Order::Name,
            Order::Name => Order::Size,
        }
    }

    /// What its button says.
    pub fn label(self) -> &'static str {
        match self {
            Order::Size => "BY SIZE",
            Order::Idle => "BY IDLE",
            Order::Name => "BY NAME",
        }
    }
}

/// Whether `text` holds every word of `query`, ignoring case.
fn matches(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

/// The candidates the list shows, in its order.
pub fn visible(candidates: &[Candidate], show: Show, order: Order, query: &str) -> Vec<Candidate> {
    let mut shown: Vec<Candidate> = candidates
        .iter()
        .filter(|candidate| show.admits(candidate))
        .filter(|candidate| {
            matches(
                query,
                &format!(
                    "{} {} {}",
                    candidate.project_name, candidate.label, candidate.path
                ),
            )
        })
        .cloned()
        .collect();
    let name = |candidate: &Candidate| {
        (
            candidate.project_name.to_lowercase(),
            candidate.label.to_lowercase(),
        )
    };
    match order {
        Order::Size => shown.sort_by(|a, b| {
            b.size
                .total()
                .cmp(&a.size.total())
                .then_with(|| name(a).cmp(&name(b)))
        }),
        Order::Idle => shown.sort_by(|a, b| {
            let idle = |candidate: &Candidate| candidate.idle_days.unwrap_or(i64::MAX);
            idle(b).cmp(&idle(a)).then_with(|| name(a).cmp(&name(b)))
        }),
        Order::Name => shown.sort_by_key(name),
    }
    shown
}

/// The keys of the shown candidates the cleaner suggests.
pub fn suggested(shown: &[Candidate]) -> HashSet<String> {
    shown
        .iter()
        .filter(|candidate| candidate.recommended())
        .map(|candidate| candidate.key.clone())
        .collect()
}

/// The ticked candidates that can still be removed, and the bytes they free.
pub fn ticked<'a>(
    candidates: &'a [Candidate],
    ticks: &HashSet<String>,
) -> (Vec<&'a Candidate>, u64) {
    let chosen: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| candidate.removable() && ticks.contains(&candidate.key))
        .collect();
    let bytes = chosen
        .iter()
        .filter_map(|candidate| candidate.size.total())
        .sum();
    (chosen, bytes)
}

/// How old a session must be to be offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Age {
    /// A week.
    Week,
    /// A month.
    #[default]
    Month,
    /// Three months.
    Quarter,
}

impl Age {
    /// Every age, in the order the chips show them.
    pub const ALL: [Age; 3] = [Age::Week, Age::Month, Age::Quarter];

    /// Its days.
    pub fn days(self) -> i64 {
        match self {
            Age::Week => 7,
            Age::Month => 30,
            Age::Quarter => 90,
        }
    }

    /// What its chip says.
    pub fn label(self) -> &'static str {
        match self {
            Age::Week => "7D+",
            Age::Month => "30D+",
            Age::Quarter => "90D+",
        }
    }
}

/// The sessions of the history older than `age` that the cleaner can settle:
/// not pinned, not running, not on a shelf. A session taken back from a shelf
/// by hand is offered again; the person ticks what goes. Oldest first.
pub fn old_sessions<'a>(
    sessions: &'a [Session],
    running: &HashSet<SessionId>,
    shelf: &HashMap<SessionId, Shelf>,
    age: Age,
    query: &str,
    now: DateTime<Utc>,
) -> Vec<&'a Session> {
    let mut old: Vec<&Session> = sessions
        .iter()
        .filter(|session| (now - session.updated_at).num_days() >= age.days())
        .filter(|session| session.sort_order.is_none())
        .filter(|session| !running.contains(&session.id))
        .filter(|session| {
            !matches!(
                shelf.get(&session.id),
                Some(Shelf::Settled | Shelf::Snoozed(_))
            )
        })
        .filter(|session| matches(query, &format!("{} {}", session.title, session.cwd)))
        .collect();
    old.sort_by_key(|session| session.updated_at);
    old
}

/// What the cleaner's status line says after settling `count` sessions.
pub fn settled_words(count: usize) -> String {
    match count {
        1 => "Settled 1 old session: the Settled shelf has it.".to_owned(),
        count => format!("Settled {count} old sessions: the Settled shelf has them."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::model::ProjectEntry;
    use chrono::{Duration, TimeZone};
    use leon_core::{AgentId, Project, Worktree};
    use leon_core::{Divergence, WorktreeStatus};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).unwrap()
    }

    fn worktree(id: &str, merged: Option<bool>) -> Worktree {
        Worktree {
            id: WorktreeId::from_string(id),
            project_id: ProjectId::from_string("p"),
            path: format!("/srv/{id}"),
            branch: Some(format!("feature/{id}")),
            head: None,
            is_main: false,
            merged_pull_request: merged,
        }
    }

    fn session(id: &str, cwd: &str, days_ago: i64) -> Session {
        let at = now() - Duration::days(days_ago);
        Session {
            id: SessionId::from_string(id),
            agent: AgentId::CLAUDE,
            external_id: id.to_owned(),
            machine_id: MachineId::local(),
            cwd: cwd.to_owned(),
            project_id: None,
            title: format!("talk {id}"),
            model: None,
            started_at: at,
            updated_at: at,
            message_count: 3,
            sort_order: None,
            account: None,
        }
    }

    fn clean() -> WorktreeStatus {
        WorktreeStatus {
            changed: Some(0),
            divergence: Some(Divergence::default()),
            pull_request: None,
        }
    }

    fn snapshot(worktrees: Vec<Worktree>, sessions: Vec<Session>) -> Snapshot {
        let mut main = worktree("main", None);
        main.is_main = true;
        let statuses = worktrees
            .iter()
            .map(|worktree| (worktree.id.clone(), clean()))
            .collect();
        Snapshot {
            projects: vec![ProjectEntry {
                project: Project {
                    id: ProjectId::from_string("p"),
                    machine_id: MachineId::local(),
                    name: "api".to_owned(),
                    root: "/srv/main".to_owned(),
                },
                worktrees: std::iter::once(main).chain(worktrees).collect(),
            }],
            sessions,
            statuses,
            ..Snapshot::default()
        }
    }

    fn of(snapshot: &Snapshot) -> Vec<Candidate> {
        candidates(snapshot, &HashMap::new(), &HashMap::new(), now())
    }

    #[test]
    fn the_main_worktree_is_never_offered() {
        let found = of(&snapshot(vec![worktree("a", None)], vec![]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "feature/a");
    }

    #[test]
    fn a_merged_or_long_idle_clean_worktree_is_recommended() {
        let snap = snapshot(
            vec![
                worktree("merged", Some(true)),
                worktree("idle", None),
                worktree("busy", None),
            ],
            vec![
                session("s1", "/srv/idle", 20),
                session("s2", "/srv/busy/src", 2),
            ],
        );
        let found = of(&snap);
        let get = |id: &str| found.iter().find(|c| c.worktree.as_str() == id).unwrap();
        assert!(get("merged").recommended());
        assert_eq!(get("merged").reasons(), ["pull request merged"]);
        assert!(get("idle").recommended());
        assert_eq!(get("idle").reasons(), ["idle 20 days"]);
        assert!(!get("busy").recommended());
        assert_eq!(get("busy").sessions, 1);
        assert_eq!(get("busy").idle_days, Some(2));
    }

    #[test]
    fn a_worktree_with_work_in_it_is_kept_with_the_reason() {
        let mut snap = snapshot(vec![worktree("a", Some(true))], vec![]);
        snap.statuses.insert(
            WorktreeId::from_string("a"),
            WorktreeStatus {
                changed: Some(3),
                ..clean()
            },
        );
        let found = of(&snap);
        assert!(!found[0].removable());
        assert!(!found[0].recommended());
        assert_eq!(found[0].reasons(), ["3 uncommitted changes"]);
        let running = HashMap::from([(WorktreeId::from_string("a"), 1)]);
        let found = candidates(&snap, &running, &HashMap::new(), now());
        assert_eq!(found[0].blockers.len(), 2);
    }

    #[test]
    fn the_filters_and_the_query_narrow_the_list() {
        let snap = snapshot(
            vec![worktree("merged", Some(true)), worktree("fresh", None)],
            vec![session("s", "/srv/fresh", 1)],
        );
        let found = of(&snap);
        let keys = |shown: Vec<Candidate>| -> Vec<String> {
            shown.into_iter().map(|c| c.worktree.to_string()).collect()
        };
        assert_eq!(
            keys(visible(&found, Show::Merged, Order::Name, "")),
            ["merged"]
        );
        assert_eq!(
            keys(visible(&found, Show::Recommended, Order::Name, "")),
            ["merged"]
        );
        assert_eq!(
            keys(visible(&found, Show::All, Order::Name, "FRE api")),
            ["fresh"]
        );
        assert!(visible(&found, Show::Blocked, Order::Name, "").is_empty());
    }

    #[test]
    fn heaviest_first_and_the_unmeasured_last() {
        let snap = snapshot(
            vec![
                worktree("a", None),
                worktree("b", None),
                worktree("c", None),
            ],
            vec![],
        );
        let used = |total| {
            Measure::Known(DiskUse {
                total,
                rebuildable: 0,
            })
        };
        let sizes = HashMap::from([
            (WorktreeId::from_string("a"), used(10)),
            (WorktreeId::from_string("c"), used(99)),
            (WorktreeId::from_string("b"), Measure::Unknown),
        ]);
        let found = candidates(&snap, &HashMap::new(), &sizes, now());
        let order: Vec<String> = visible(&found, Show::All, Order::Size, "")
            .into_iter()
            .map(|c| c.worktree.to_string())
            .collect();
        assert_eq!(order, ["c", "a", "b"]);
        let ticks = HashSet::from(["p|a".to_owned(), "p|c".to_owned()]);
        let (chosen, bytes) = ticked(&found, &ticks);
        assert_eq!((chosen.len(), bytes), (2, 109));
    }

    #[test]
    fn a_blocked_worktree_is_never_among_the_ticked() {
        let mut snap = snapshot(vec![worktree("a", None)], vec![]);
        snap.statuses.remove(&WorktreeId::from_string("a"));
        let found = of(&snap);
        let (chosen, _) = ticked(&found, &HashSet::from(["p|a".to_owned()]));
        assert!(chosen.is_empty());
        assert!(suggested(&found).is_empty());
    }

    #[test]
    fn old_sessions_leave_out_the_pinned_running_and_shelved_ones() {
        let mut pinned = session("pinned", "/a", 60);
        pinned.sort_order = Some(0);
        let sessions = vec![
            session("fresh", "/a", 3),
            session("old", "/a", 40),
            session("older", "/b", 100),
            pinned,
            session("running", "/a", 50),
            session("settled", "/a", 50),
            session("returned", "/a", 50),
        ];
        let running = HashSet::from([SessionId::from_string("running")]);
        let shelf = HashMap::from([
            (SessionId::from_string("settled"), Shelf::Settled),
            (SessionId::from_string("returned"), Shelf::Returned),
        ]);
        let ids = |age, query| -> Vec<String> {
            old_sessions(&sessions, &running, &shelf, age, query, now())
                .into_iter()
                .map(|s| s.id.to_string())
                .collect()
        };
        assert_eq!(ids(Age::Month, ""), ["older", "returned", "old"]);
        assert_eq!(ids(Age::Quarter, ""), ["older"]);
        assert_eq!(ids(Age::Month, "/b"), ["older"]);
    }
}

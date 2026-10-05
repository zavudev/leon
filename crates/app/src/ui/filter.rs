//! Looking for a project: what the sidebar's field filters the tree by, and
//! how projects with the same name are told apart.
//!
//! Both are pure functions of a [`Snapshot`], so typing never asks the store
//! for anything. A project matches a query when its name, its label, its root
//! path or the branch or folder name of any of its worktrees does, by the same
//! scorer the palette uses (`crate::fuzzy`); a path counts only when the query
//! occurs in it, because the letters of a short query are always somewhere in
//! a long path. When the project itself matches, all its worktrees stay;
//! otherwise only the worktrees that match do.

use super::model::Snapshot;
use super::tree::{folder_name, NodeId};
use crate::fuzzy::score;
use leon_core::{ProjectId, WorktreeId};
use std::collections::{HashMap, HashSet};

/// The score from which a path counts as containing the query (see
/// `fuzzy::score`: below it are the loose letters-in-order matches).
const PATH_SCORE: u32 = 400;

/// How many ancestor folders a label may take on to tell two projects apart.
const MAX_ANCESTORS: usize = 5;

/// How one project answers a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMatch {
    /// Whether the project itself matches (and so all its worktrees stay).
    pub own: bool,
    /// The worktrees that stay.
    pub worktrees: HashSet<WorktreeId>,
}

/// What a query leaves of the tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// The query, trimmed.
    pub query: String,
    /// The projects that stay.
    pub projects: HashMap<ProjectId, ProjectMatch>,
    /// The node that answers best: what Enter opens.
    pub best: Option<NodeId>,
}

impl Filter {
    /// Whether anything is left.
    pub fn is_empty(&self) -> bool {
        self.projects.is_empty()
    }
}

/// The projects and worktrees of `snapshot` that answer `query`; `None` for a
/// query with nothing in it.
pub fn filter(
    snapshot: &Snapshot,
    labels: &HashMap<ProjectId, String>,
    query: &str,
) -> Option<Filter> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let mut out = Filter {
        query: query.to_owned(),
        ..Filter::default()
    };
    let mut best: Option<(u32, NodeId)> = None;
    let mut consider = |points: u32, node: NodeId| {
        if best.as_ref().is_none_or(|(top, _)| points > *top) {
            best = Some((points, node));
        }
    };
    for entry in &snapshot.projects {
        let project = &entry.project;
        let label = labels.get(&project.id).unwrap_or(&project.name);
        let own = [
            score(query, &project.name),
            score(query, label),
            score(query, &project.root).filter(|points| *points >= PATH_SCORE),
        ]
        .into_iter()
        .flatten()
        .max();
        if let Some(points) = own {
            consider(points, NodeId::Project(project.id.clone()));
        }
        let mut worktrees = HashSet::new();
        for worktree in &entry.worktrees {
            let points = [
                worktree
                    .branch
                    .as_deref()
                    .and_then(|branch| score(query, branch)),
                score(query, folder_name(&worktree.path)),
            ]
            .into_iter()
            .flatten()
            .max();
            if let Some(points) = points {
                worktrees.insert(worktree.id.clone());
                consider(points, NodeId::Worktree(worktree.id.clone()));
            }
        }
        if own.is_none() && worktrees.is_empty() {
            continue;
        }
        if own.is_some() {
            worktrees = entry.worktrees.iter().map(|w| w.id.clone()).collect();
        }
        out.projects.insert(
            project.id.clone(),
            ProjectMatch {
                own: own.is_some(),
                worktrees,
            },
        );
    }
    out.best = best.map(|(_, node)| node);
    Some(out)
}

/// What each project is called in the tree and the palette: its name, unless
/// another project of the same machine has the same name. Then the repository
/// as its origin remote names it (`owner/repo`) when every project of that
/// name has one and they differ, else the folders above it (`zavu/monorepo`,
/// `acme.io/monorepo`), as many as it takes to tell them apart.
pub fn project_labels(snapshot: &Snapshot) -> HashMap<ProjectId, String> {
    let mut groups: HashMap<(&leon_core::MachineId, String), Vec<&leon_core::Project>> =
        HashMap::new();
    for entry in &snapshot.projects {
        groups
            .entry((&entry.project.machine_id, entry.project.name.to_lowercase()))
            .or_default()
            .push(&entry.project);
    }
    let mut labels = HashMap::new();
    for group in groups.into_values() {
        if let [only] = group.as_slice() {
            labels.insert(only.id.clone(), only.name.clone());
            continue;
        }
        let slugs: Vec<Option<String>> = group
            .iter()
            .map(|project| {
                let remote = snapshot.icons.get(&project.id)?.remote.as_deref()?;
                // `host/owner/repo` is shown as `owner/repo`.
                Some(remote.split_once('/')?.1.to_owned())
            })
            .collect();
        let distinct =
            |labels: &[String]| labels.iter().collect::<HashSet<_>>().len() == labels.len();
        if slugs.iter().all(Option::is_some) {
            let slugs: Vec<String> = slugs.into_iter().flatten().collect();
            if distinct(&slugs) {
                for (project, slug) in group.iter().zip(slugs) {
                    labels.insert(project.id.clone(), slug);
                }
                continue;
            }
        }
        let by_path = |depth: usize| -> Vec<String> {
            group
                .iter()
                .map(|project| {
                    let mut parents: Vec<&str> = super::tree::ancestors(&project.root)
                        .skip(1)
                        .take(depth)
                        .map(folder_name)
                        .collect();
                    parents.reverse();
                    parents.push(&project.name);
                    parents.join("/")
                })
                .collect()
        };
        let mut chosen = by_path(1);
        for depth in 2..=MAX_ANCESTORS {
            if distinct(&chosen) {
                break;
            }
            chosen = by_path(depth);
        }
        for (project, label) in group.iter().zip(chosen) {
            labels.insert(project.id.clone(), label);
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::model::ProjectEntry;
    use leon_core::{IconKind, MachineId, Project, ProjectIcon, Worktree};

    fn project(id: &str, machine: &MachineId, name: &str, root: &str) -> Project {
        Project {
            id: ProjectId::from_string(id),
            machine_id: machine.clone(),
            name: name.into(),
            root: root.into(),
        }
    }

    fn worktree(id: &str, project: &str, path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            id: WorktreeId::from_string(id),
            project_id: ProjectId::from_string(project),
            path: path.into(),
            branch: branch.map(str::to_owned),
            head: None,
            is_main: false,
        }
    }

    fn entry(project: Project, worktrees: Vec<Worktree>) -> ProjectEntry {
        ProjectEntry { project, worktrees }
    }

    fn snapshot() -> Snapshot {
        let local = MachineId::local();
        Snapshot {
            projects: vec![
                entry(
                    project("api", &local, "api", "/code/api"),
                    vec![
                        worktree("api-main", "api", "/code/api", Some("main")),
                        worktree(
                            "api-login",
                            "api",
                            "/code/api-wt/login",
                            Some("feature/login"),
                        ),
                    ],
                ),
                entry(
                    project("web", &local, "web", "/code/web"),
                    vec![worktree("web-main", "web", "/code/web", Some("main"))],
                ),
            ],
            ..Snapshot::default()
        }
    }

    fn labels(snapshot: &Snapshot) -> HashMap<ProjectId, String> {
        project_labels(snapshot)
    }

    fn kept(filter: &Filter) -> Vec<String> {
        let mut ids: Vec<String> = filter.projects.keys().map(|id| id.to_string()).collect();
        ids.sort();
        ids
    }

    #[test]
    fn an_empty_query_filters_nothing() {
        let snapshot = snapshot();
        assert_eq!(filter(&snapshot, &labels(&snapshot), "   "), None);
    }

    #[test]
    fn a_project_that_matches_keeps_all_its_worktrees() {
        let snapshot = snapshot();
        let found = filter(&snapshot, &labels(&snapshot), "api").unwrap();
        assert_eq!(kept(&found), ["api"]);
        let api = &found.projects[&ProjectId::from_string("api")];
        assert!(api.own);
        assert_eq!(api.worktrees.len(), 2);
    }

    #[test]
    fn a_worktree_that_matches_keeps_its_project_and_only_itself() {
        let snapshot = snapshot();
        let found = filter(&snapshot, &labels(&snapshot), "login").unwrap();
        assert_eq!(kept(&found), ["api"]);
        let api = &found.projects[&ProjectId::from_string("api")];
        assert!(!api.own);
        assert_eq!(
            api.worktrees,
            HashSet::from([WorktreeId::from_string("api-login")])
        );
        assert_eq!(
            found.best,
            Some(NodeId::Worktree(WorktreeId::from_string("api-login")))
        );
    }

    #[test]
    fn the_branch_of_a_detached_worktree_is_its_folder() {
        let mut snapshot = snapshot();
        snapshot.projects[1].worktrees.push(worktree(
            "web-wip",
            "web",
            "/code/web-wt/wip-spike",
            None,
        ));
        let found = filter(&snapshot, &labels(&snapshot), "spike").unwrap();
        assert_eq!(kept(&found), ["web"]);
    }

    #[test]
    fn the_root_path_matches_only_where_the_query_occurs_in_it() {
        let snapshot = snapshot();
        let labels = labels(&snapshot);
        // "code" occurs in both roots.
        assert_eq!(
            kept(&filter(&snapshot, &labels, "code").unwrap()),
            ["api", "web"]
        );
        // "cwb" is only loose letters of "/code/web": not a match.
        assert!(filter(&snapshot, &labels, "cwb").unwrap().is_empty());
    }

    #[test]
    fn nothing_matching_leaves_nothing_and_no_best_node() {
        let snapshot = snapshot();
        let found = filter(&snapshot, &labels(&snapshot), "zzzz").unwrap();
        assert!(found.is_empty());
        assert_eq!(found.best, None);
    }

    #[test]
    fn the_best_node_is_the_closest_match_not_the_first_in_the_tree() {
        let snapshot = snapshot();
        let found = filter(&snapshot, &labels(&snapshot), "web").unwrap();
        assert_eq!(
            found.best,
            Some(NodeId::Project(ProjectId::from_string("web")))
        );
        let found = filter(&snapshot, &labels(&snapshot), "main").unwrap();
        assert_eq!(
            found.best,
            Some(NodeId::Worktree(WorktreeId::from_string("api-main"))),
            "ties go to the first"
        );
    }

    #[test]
    fn a_unique_name_is_its_own_label() {
        let snapshot = snapshot();
        assert_eq!(labels(&snapshot)[&ProjectId::from_string("api")], "api");
    }

    #[test]
    fn two_projects_with_one_name_are_told_apart_by_their_parent_folder() {
        let local = MachineId::local();
        let snapshot = Snapshot {
            projects: vec![
                entry(
                    project("a", &local, "monorepo", "/code/zavu/monorepo"),
                    vec![],
                ),
                entry(
                    project("b", &local, "monorepo", "/code/acme.io/monorepo"),
                    vec![],
                ),
                entry(project("c", &local, "other", "/code/other"), vec![]),
            ],
            ..Snapshot::default()
        };
        let labels = labels(&snapshot);
        assert_eq!(labels[&ProjectId::from_string("a")], "zavu/monorepo");
        assert_eq!(labels[&ProjectId::from_string("b")], "acme.io/monorepo");
        assert_eq!(labels[&ProjectId::from_string("c")], "other");
    }

    #[test]
    fn the_remote_owner_and_repo_win_over_folders_when_every_project_has_one() {
        let local = MachineId::local();
        let mut snapshot = Snapshot {
            projects: vec![
                entry(project("a", &local, "app", "/code/one/app"), vec![]),
                entry(project("b", &local, "app", "/code/two/app"), vec![]),
            ],
            ..Snapshot::default()
        };
        for (id, remote) in [("a", "github.com/zavu/app"), ("b", "github.com/acme/app")] {
            snapshot.icons.insert(
                ProjectId::from_string(id),
                ProjectIcon {
                    project_id: ProjectId::from_string(id),
                    kind: IconKind::Folder,
                    source: String::new(),
                    format: None,
                    hash: None,
                    remote: Some(remote.into()),
                },
            );
        }
        let labels = labels(&snapshot);
        assert_eq!(labels[&ProjectId::from_string("a")], "zavu/app");
        assert_eq!(labels[&ProjectId::from_string("b")], "acme/app");
    }

    #[test]
    fn the_same_repository_cloned_twice_falls_back_to_folders_and_deeper_ones_when_needed() {
        let local = MachineId::local();
        let mut snapshot = Snapshot {
            projects: vec![
                entry(project("a", &local, "app", "/x/work/app"), vec![]),
                entry(project("b", &local, "app", "/y/work/app"), vec![]),
            ],
            ..Snapshot::default()
        };
        for id in ["a", "b"] {
            snapshot.icons.insert(
                ProjectId::from_string(id),
                ProjectIcon {
                    project_id: ProjectId::from_string(id),
                    kind: IconKind::Folder,
                    source: String::new(),
                    format: None,
                    hash: None,
                    remote: Some("github.com/zavu/app".into()),
                },
            );
        }
        let labels = labels(&snapshot);
        assert_eq!(labels[&ProjectId::from_string("a")], "x/work/app");
        assert_eq!(labels[&ProjectId::from_string("b")], "y/work/app");
    }

    #[test]
    fn projects_of_different_machines_do_not_need_telling_apart() {
        let remote = MachineId::from_string("box");
        let snapshot = Snapshot {
            projects: vec![
                entry(
                    project("a", &MachineId::local(), "api", "/code/api"),
                    vec![],
                ),
                entry(project("b", &remote, "api", "/srv/api"), vec![]),
            ],
            ..Snapshot::default()
        };
        let labels = labels(&snapshot);
        assert_eq!(labels[&ProjectId::from_string("a")], "api");
        assert_eq!(labels[&ProjectId::from_string("b")], "api");
    }
}

//! The sidebar tree as plain data.
//!
//! ```text
//! MACHINE            this machine · ● · ONLINE
//!   project          api · 12
//!     worktree       main · MAIN
//!       session      CLAUDE  fix the login bug  5m
//!   [ UNSORTED ]
//!     folder         notes
//! ```
//!
//! Two steps turn a [`Snapshot`] into the rows the sidebar draws.
//! [`Placement::compute`] decides, once per snapshot, which worktree each
//! session belongs to: the deepest project root or worktree that contains its
//! folder wins, and a session that no project contains goes under its
//! machine's unsorted node, grouped by folder. [`build_rows`] then flattens
//! the tree into a list for the nodes that are open. Neither asks the store
//! for anything, so both are linear in the number of sessions, and opening or
//! folding a node only repeats the second step.
//!
//! A live terminal that resumed a history session of its own folder has no
//! row: that session's row is its row ([`Placement::merged`]).
//!
//! Nodes are identified by what they are ([`NodeId`]), never by their place in
//! the list, so the cursor stays on its node when rows appear above it.

use super::expansion::Expansion;
use super::filter::{Filter, ProjectMatch};
use super::live::LiveId;
use super::model::{ProjectEntry, Snapshot};
use chrono::{DateTime, Duration, Utc};
use leon_core::{
    AgentId, Machine, MachineId, Project, ProjectId, Session, SessionId, SessionScope, Worktree,
    WorktreeId,
};
use std::collections::{HashMap, HashSet};

/// How many sessions a worktree or folder shows before "show more".
pub const SESSIONS_SHOWN: usize = 8;

/// The number the settings start from.
pub const DEFAULT: usize = SESSIONS_SHOWN;

/// A worktree is open by default when a session of this many days ago or
/// less ran in it.
const RECENT_DAYS: i64 = 7;

/// What a node of the tree is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeId {
    /// A machine.
    Machine(MachineId),
    /// A project.
    Project(ProjectId),
    /// A worktree.
    Worktree(WorktreeId),
    /// The sessions of a machine that no project contains.
    Unsorted(MachineId),
    /// One folder among the unsorted sessions.
    Folder(MachineId, String),
    /// The Pinned section of a machine: its pinned sessions, above the
    /// projects.
    Pinned(MachineId),
    /// A session.
    Session(SessionId),
    /// A live terminal session.
    Live(LiveId),
    /// The row that shows the rest of the sessions under the node.
    More(Box<NodeId>),
    /// The line that says how to open a project, under an empty machine.
    Open(MachineId),
    /// The line that says no project matches the filter.
    NoMatch,
    /// The line that says no session is active, with only the active ones
    /// shown.
    NoActive,
}

impl NodeId {
    /// A text that stays the same for the same node, for the file that
    /// remembers what is open.
    pub fn key(&self) -> String {
        match self {
            NodeId::Machine(id) => format!("machine:{id}"),
            NodeId::Project(id) => format!("project:{id}"),
            NodeId::Worktree(id) => format!("worktree:{id}"),
            NodeId::Unsorted(id) => format!("unsorted:{id}"),
            NodeId::Folder(id, cwd) => format!("folder:{id}:{cwd}"),
            NodeId::Pinned(id) => format!("pinned:{id}"),
            NodeId::Session(id) => format!("session:{id}"),
            NodeId::Live(id) => format!("live:{id}"),
            NodeId::More(parent) => format!("more:{}", parent.key()),
            NodeId::Open(id) => format!("open:{id}"),
            NodeId::NoMatch => "no-match".to_owned(),
            NodeId::NoActive => "no-active".to_owned(),
        }
    }
}

/// A live session as the tree needs to know it. Its state is read when the
/// row is drawn, never kept here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEntry {
    /// Its identity.
    pub id: LiveId,
    /// The machine it runs on.
    pub machine: MachineId,
    /// The folder it runs in.
    pub cwd: String,
    /// The agent, or `None` for a plain shell.
    pub agent: Option<AgentId>,
    /// The history session it was started from, when it resumed one.
    pub history: Option<SessionId>,
}

/// What a row shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A machine.
    Machine(Machine),
    /// A project, with how many sessions are under it.
    Project {
        /// The project.
        project: Project,
        /// Sessions under it, however deep.
        sessions: usize,
    },
    /// A worktree, with how many sessions are in it.
    Worktree {
        /// The worktree.
        worktree: Worktree,
        /// Sessions in it.
        sessions: usize,
    },
    /// The unsorted node of a machine.
    Unsorted {
        /// Sessions under it.
        sessions: usize,
    },
    /// The Pinned section of a machine, with how many sessions it holds.
    Pinned {
        /// The pinned sessions under it.
        sessions: usize,
    },
    /// A folder among the unsorted sessions.
    Folder {
        /// The folder on its machine.
        cwd: String,
        /// Sessions that ran in it.
        sessions: usize,
    },
    /// A session.
    Session(Session),
    /// A live terminal session.
    Live(LiveEntry),
    /// "Show n more".
    More {
        /// How many sessions are still hidden.
        hidden: usize,
    },
    /// "Open a project".
    Open,
    /// "No projects match": what the filtered tree says when nothing is left.
    NoMatch,
    /// "No active sessions": what the tree says when only the active ones
    /// are shown and none is.
    NoActive,
}

/// One row of the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The node it stands for.
    pub id: NodeId,
    /// The machine it is on.
    pub machine: MachineId,
    /// How deep it is: machines are 0.
    pub depth: u8,
    /// `Some(open)` when it has children and can be opened or folded.
    pub open: Option<bool>,
    /// What it shows.
    pub kind: Kind,
}

impl Row {
    /// The node's key, see [`NodeId::key`].
    pub fn key(&self) -> String {
        self.id.key()
    }
}

/// The folder a worktree or project is named after, when it has no better
/// name.
pub fn folder_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

/// What a worktree is called: its branch, or its folder when the head is
/// detached.
pub fn worktree_label(worktree: &Worktree) -> String {
    worktree
        .branch
        .clone()
        .unwrap_or_else(|| folder_name(&worktree.path).to_owned())
}

// ----- which worktree a session belongs to ---------------------------------------

/// A path and every folder above it, longest first. Both kinds of separator
/// count, because the machine may not run the operating system Leon does.
pub fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    let first = trim_separators(path);
    std::iter::successors((!first.is_empty()).then_some(first), |current| {
        let cut = current.rfind(['/', '\\'])?;
        let parent = &current[..cut];
        (!parent.is_empty()).then_some(parent)
    })
}

/// `path` without trailing separators; a bare root keeps its own.
fn trim_separators(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        path
    } else {
        trimmed
    }
}

/// Whether two spellings name one folder (see `leon_core::path`).
pub fn same_folder(a: &str, b: &str) -> bool {
    leon_core::path::key(a) == leon_core::path::key(b)
}

/// A folder of unsorted sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// The folder on its machine.
    pub cwd: String,
    /// The sessions that ran in it, as places in `Snapshot::sessions`.
    pub sessions: Vec<usize>,
}

/// Where every session of a snapshot goes. Sessions are named by their place
/// in `Snapshot::sessions`, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    /// The sessions of each worktree.
    pub in_worktree: HashMap<WorktreeId, Vec<usize>>,
    /// The sessions inside a project's root that no worktree of it contains
    /// (a project whose worktrees were never listed).
    pub in_project: HashMap<ProjectId, Vec<usize>>,
    /// The sessions no project contains, by machine and folder, the folder of
    /// the newest session first.
    pub unsorted: HashMap<MachineId, Vec<Folder>>,
    /// The pinned sessions of each machine, in their pinned order: the Pinned
    /// section shows them. They are in the lists above as well, which the main
    /// pane of a worktree counts; the tree leaves them out of those lists.
    pub pinned: HashMap<MachineId, Vec<usize>>,
    /// The live sessions, in the order they were started.
    pub live: Vec<LiveEntry>,
    /// The live sessions of each worktree, as places in `live`.
    pub live_in_worktree: HashMap<WorktreeId, Vec<usize>>,
    /// The live sessions inside a project that no worktree contains.
    pub live_in_project: HashMap<ProjectId, Vec<usize>>,
    /// The live sessions no project contains, by machine.
    pub live_loose: HashMap<MachineId, Vec<usize>>,
    /// The history sessions that have a live terminal in their own folder:
    /// their row is the terminal's row, so the terminal gets no row of its
    /// own, and the row is never left behind "show more".
    pub merged: HashSet<SessionId>,
}

/// Who owns a path.
enum Owner<'a> {
    Worktree(&'a WorktreeId),
    Project(&'a ProjectId),
    /// A folder inside a project that somebody removed (a worktree): the
    /// sessions that ran there are history only, and the project that
    /// contains the folder does not take them.
    Removed,
}

/// For each machine, which project or worktree owns each root path.
fn owner_tables(snapshot: &Snapshot) -> HashMap<&MachineId, HashMap<String, Owner<'_>>> {
    let mut owners: HashMap<&MachineId, HashMap<String, Owner>> = snapshot
        .machines
        .iter()
        .map(|machine| (&machine.id, HashMap::new()))
        .collect();
    // Roots first, so that a worktree at the very same path takes over.
    for entry in &snapshot.projects {
        if let Some(table) = owners.get_mut(&entry.project.machine_id) {
            table.insert(
                leon_core::path::key(&entry.project.root),
                Owner::Project(&entry.project.id),
            );
        }
    }
    for entry in &snapshot.projects {
        if let Some(table) = owners.get_mut(&entry.project.machine_id) {
            for worktree in &entry.worktrees {
                table.insert(
                    leon_core::path::key(&worktree.path),
                    Owner::Worktree(&worktree.id),
                );
            }
        }
    }
    // Only inside a project: a removed project's own folder has no owner above
    // it, and its sessions stay among the unsorted.
    for (machine, folder) in &snapshot.dismissed {
        let Some(table) = owners.get_mut(machine) else {
            continue;
        };
        let keys = leon_core::path::ancestor_keys(folder);
        let inside = keys.iter().skip(1).any(|key| table.contains_key(key));
        if let Some(key) = keys.into_iter().next().filter(|_| inside) {
            table.entry(key).or_insert(Owner::Removed);
        }
    }
    owners
}

impl Placement {
    /// The same placement with the live sessions placed too: each goes where
    /// the history session of its folder would.
    pub fn with_live(mut self, snapshot: &Snapshot, live: Vec<LiveEntry>) -> Self {
        let owners = owner_tables(snapshot);
        self.live_in_worktree.clear();
        self.live_in_project.clear();
        self.live_loose.clear();
        self.merged.clear();
        for entry in &live {
            let Some(history) = &entry.history else {
                continue;
            };
            let same_place = snapshot.sessions.iter().any(|session| {
                &session.id == history
                    && session.machine_id == entry.machine
                    && leon_core::path::key(&session.cwd) == leon_core::path::key(&entry.cwd)
            });
            if same_place {
                self.merged.insert(history.clone());
            }
        }
        for (index, entry) in live.iter().enumerate() {
            let owner = owners.get(&entry.machine).and_then(|table| {
                leon_core::path::ancestor_keys(&entry.cwd)
                    .iter()
                    .find_map(|folder| table.get(folder).filter(|o| !matches!(o, Owner::Removed)))
            });
            match owner {
                Some(Owner::Worktree(id)) => self
                    .live_in_worktree
                    .entry((*id).clone())
                    .or_default()
                    .push(index),
                Some(Owner::Project(id)) => self
                    .live_in_project
                    .entry((*id).clone())
                    .or_default()
                    .push(index),
                Some(Owner::Removed) | None => self
                    .live_loose
                    .entry(entry.machine.clone())
                    .or_default()
                    .push(index),
            }
        }
        self.live = live;
        self
    }

    /// The live sessions of a worktree.
    pub fn live_of_worktree(&self, id: &WorktreeId) -> &[usize] {
        self.live_in_worktree.get(id).map_or(&[], Vec::as_slice)
    }

    /// Places every session of `snapshot`. Each session costs a walk up its
    /// folder's ancestors, a handful of lookups. Inside every parent the
    /// pinned sessions go first, in their pinned order, then the rest
    /// newest-first (the snapshot already lists newest first, and the sort
    /// is stable). The pinned sessions of a machine are also gathered in
    /// `pinned`, in their pinned order.
    pub fn compute(snapshot: &Snapshot) -> Self {
        let owners = owner_tables(snapshot);
        let mut placement = Self::default();
        let mut folders: HashMap<(&MachineId, String), usize> = HashMap::new();
        for (index, session) in snapshot.sessions.iter().enumerate() {
            if session.sort_order.is_some() {
                placement
                    .pinned
                    .entry(session.machine_id.clone())
                    .or_default()
                    .push(index);
            }
            let Some(table) = owners.get(&session.machine_id) else {
                continue;
            };
            let owner = leon_core::path::ancestor_keys(&session.cwd)
                .iter()
                .find_map(|folder| table.get(folder));
            match owner {
                Some(Owner::Worktree(id)) => {
                    placement
                        .in_worktree
                        .entry((*id).clone())
                        .or_default()
                        .push(index);
                }
                Some(Owner::Project(id)) => {
                    placement
                        .in_project
                        .entry((*id).clone())
                        .or_default()
                        .push(index);
                }
                Some(Owner::Removed) => {}
                None => {
                    let list = placement
                        .unsorted
                        .entry(session.machine_id.clone())
                        .or_default();
                    let place = *folders
                        .entry((&session.machine_id, leon_core::path::key(&session.cwd)))
                        .or_insert_with(|| {
                            list.push(Folder {
                                cwd: session.cwd.clone(),
                                sessions: Vec::new(),
                            });
                            list.len() - 1
                        });
                    list[place].sessions.push(index);
                }
            }
        }
        for places in placement.in_worktree.values_mut() {
            pinned_first(snapshot, places);
        }
        for places in placement.in_project.values_mut() {
            pinned_first(snapshot, places);
        }
        for folders in placement.unsorted.values_mut() {
            for folder in folders {
                pinned_first(snapshot, &mut folder.sessions);
            }
        }
        for places in placement.pinned.values_mut() {
            places.sort_by_key(|place| snapshot.sessions[*place].sort_order);
        }
        placement
    }

    /// The sessions of a worktree.
    pub fn of_worktree(&self, id: &WorktreeId) -> &[usize] {
        self.in_worktree.get(id).map_or(&[], Vec::as_slice)
    }
}

/// Pinned sessions first, in their pinned order, then the rest as they were.
fn pinned_first(snapshot: &Snapshot, places: &mut [usize]) {
    places.sort_by_key(|place| {
        let order = snapshot.sessions[*place].sort_order;
        (order.is_none(), order.unwrap_or(0))
    });
}

/// The sessions of `places` that a list of the tree shows under its parent: a
/// pinned session is shown in the Pinned section instead.
fn listed(snapshot: &Snapshot, places: &[usize]) -> Vec<usize> {
    places
        .iter()
        .copied()
        .filter(|place| snapshot.sessions[*place].sort_order.is_none())
        .collect()
}

/// The sessions a filtered tree shows of a machine: those of the worktrees the
/// filter keeps. A filtered project shows no loose session, and so neither
/// does its pinned section.
fn filtered_places(
    snapshot: &Snapshot,
    placement: &Placement,
    machine: &MachineId,
    filter: &Filter,
) -> HashSet<usize> {
    let mut places = HashSet::new();
    for entry in snapshot
        .projects
        .iter()
        .filter(|entry| &entry.project.machine_id == machine)
    {
        let Some(matched) = filter.projects.get(&entry.project.id) else {
            continue;
        };
        for worktree in &entry.worktrees {
            if matched.worktrees.contains(&worktree.id) {
                places.extend(placement.of_worktree(&worktree.id).iter().copied());
            }
        }
    }
    places
}

/// The folder a terminal started in `cwd` on `machine` belongs to: the deepest
/// worktree or project root of the machine that contains it, else `cwd`
/// itself. Terminals of one folder share a workspace.
pub fn workspace_root(snapshot: &Snapshot, machine: &MachineId, cwd: &str) -> String {
    let above = leon_core::path::ancestor_keys(cwd);
    let mut best: Option<(&str, usize)> = None;
    for entry in snapshot
        .projects
        .iter()
        .filter(|entry| &entry.project.machine_id == machine)
    {
        let paths = entry
            .worktrees
            .iter()
            .map(|worktree| worktree.path.as_str())
            .chain([entry.project.root.as_str()]);
        for path in paths {
            let key = leon_core::path::key(path);
            if above.contains(&key) && best.is_none_or(|(_, longest)| key.len() > longest) {
                best = Some((path, key.len()));
            }
        }
    }
    best.map_or_else(|| trim_separators(cwd), |(path, _)| trim_separators(path))
        .to_owned()
}

/// The project, and its worktree when a worktree has this exact folder, that
/// a workspace root names.
pub fn detail_of_root(
    snapshot: &Snapshot,
    machine: &MachineId,
    root: &str,
) -> Option<(ProjectId, Option<WorktreeId>)> {
    for entry in snapshot
        .projects
        .iter()
        .filter(|entry| &entry.project.machine_id == machine)
    {
        if let Some(worktree) = entry
            .worktrees
            .iter()
            .find(|worktree| same_folder(&worktree.path, root))
        {
            return Some((entry.project.id.clone(), Some(worktree.id.clone())));
        }
        if same_folder(&entry.project.root, root) {
            return Some((entry.project.id.clone(), None));
        }
    }
    None
}

// ----- the rows ----------------------------------------------------------------------

/// Flattens the tree into the rows of the nodes that are open.
///
/// By default machines and projects are open, a worktree is open when a
/// session of the last week ran in it, and the unsorted node and its folders
/// are folded; whatever `expansion` says wins.
pub fn build_rows(
    snapshot: &Snapshot,
    placement: &Placement,
    expansion: &Expansion,
    now: DateTime<Utc>,
) -> Vec<Row> {
    build_rows_filtered(snapshot, placement, expansion, now, None)
}

/// [`build_rows`] through a filter (see `filter.rs`): only the projects and
/// worktrees that match, each machine as its header alone when none does, all
/// of them open whatever `expansion` says (it is not changed), and "No
/// projects match" when nothing is left. Without a filter, the whole tree.
pub fn build_rows_filtered(
    snapshot: &Snapshot,
    placement: &Placement,
    expansion: &Expansion,
    now: DateTime<Utc>,
    filter: Option<&Filter>,
) -> Vec<Row> {
    let mut by_machine: HashMap<&MachineId, Vec<&ProjectEntry>> = HashMap::new();
    for entry in &snapshot.projects {
        by_machine
            .entry(&entry.project.machine_id)
            .or_default()
            .push(entry);
    }
    let mut out = Rows {
        snapshot,
        expansion,
        merged: &placement.merged,
        rows: Vec::new(),
    };
    let headers = machine_headers(snapshot);
    for machine in &snapshot.machines {
        let id = NodeId::Machine(machine.id.clone());
        // Without its header a machine cannot be folded, so it is open.
        let open = !headers || expansion.is_open(&id.key(), true);
        let projects = by_machine.get(&machine.id).map_or(&[][..], Vec::as_slice);
        if let Some(filter) = filter {
            // A filtered machine is its header, the pinned sessions the filter
            // keeps, and the projects that match.
            out.push(id, &machine.id, 0, None, Kind::Machine(machine.clone()));
            let visible = filtered_places(snapshot, placement, &machine.id, filter);
            out.pinned(placement, &machine.id, 1, Some(&visible));
            for entry in projects {
                if let Some(matched) = filter.projects.get(&entry.project.id) {
                    out.project(entry, placement, now, Some(matched));
                }
            }
            continue;
        }
        out.push(
            id,
            &machine.id,
            0,
            Some(open),
            Kind::Machine(machine.clone()),
        );
        if !open {
            continue;
        }
        out.pinned(placement, &machine.id, 1, None);
        if projects.is_empty() {
            out.push(
                NodeId::Open(machine.id.clone()),
                &machine.id,
                1,
                None,
                Kind::Open,
            );
        }
        for place in placement.live_loose.get(&machine.id).into_iter().flatten() {
            out.live(placement, *place, &machine.id, 1);
        }
        for entry in projects {
            out.project(entry, placement, now, None);
        }
        let folders: Vec<Folder> = placement
            .unsorted
            .get(&machine.id)
            .into_iter()
            .flatten()
            .map(|folder| Folder {
                cwd: folder.cwd.clone(),
                sessions: listed(snapshot, &folder.sessions),
            })
            .filter(|folder| !folder.sessions.is_empty())
            .collect();
        if !folders.is_empty() {
            out.unsorted(&machine.id, &folders);
        }
    }
    if filter.is_some_and(Filter::is_empty) {
        let home = snapshot
            .machines
            .first()
            .map_or_else(MachineId::local, |machine| machine.id.clone());
        out.push(NodeId::NoMatch, &home, 1, None, Kind::NoMatch);
    }
    if headers {
        return out.rows;
    }
    // One machine: its header says nothing the window does not, so it is left
    // out and its projects sit at the top level.
    out.rows
        .into_iter()
        .filter(|row| !matches!(row.kind, Kind::Machine(_)))
        .map(|row| Row {
            depth: row.depth.saturating_sub(1),
            ..row
        })
        .collect()
}

/// Whether the machines get a header row: only when there is more than one
/// to tell apart. The depth of a project is 1 under a header and 0 without.
pub fn machine_headers(snapshot: &Snapshot) -> bool {
    snapshot.machines.len() > 1
}

/// The depth of the project rows (the first level that is not a machine).
pub fn project_depth(snapshot: &Snapshot) -> u8 {
    u8::from(machine_headers(snapshot))
}

struct Rows<'a> {
    snapshot: &'a Snapshot,
    expansion: &'a Expansion,
    /// The history sessions whose row is their live terminal's row.
    merged: &'a HashSet<SessionId>,
    rows: Vec<Row>,
}

impl Rows<'_> {
    fn push(&mut self, id: NodeId, machine: &MachineId, depth: u8, open: Option<bool>, kind: Kind) {
        self.rows.push(Row {
            id,
            machine: machine.clone(),
            depth,
            open,
            kind,
        });
    }

    fn project(
        &mut self,
        entry: &ProjectEntry,
        placement: &Placement,
        now: DateTime<Utc>,
        matched: Option<&ProjectMatch>,
    ) {
        let filtered = matched.is_some();
        let project = &entry.project;
        let loose = listed(
            self.snapshot,
            placement
                .in_project
                .get(&project.id)
                .map_or(&[][..], Vec::as_slice),
        );
        let sessions = loose.len()
            + entry
                .worktrees
                .iter()
                .map(|worktree| listed(self.snapshot, placement.of_worktree(&worktree.id)).len())
                .sum::<usize>();
        let live_loose = if filtered {
            &[][..]
        } else {
            placement
                .live_in_project
                .get(&project.id)
                .map_or(&[][..], Vec::as_slice)
        };
        let loose = if filtered { Vec::new() } else { loose };
        let has_children = !entry.worktrees.is_empty()
            || !loose.is_empty()
            || !self.shown_live(placement, live_loose).is_empty();
        let id = NodeId::Project(project.id.clone());
        // A filtered project is open, and its state is not the expansion's to
        // change: no chevron.
        let open = filtered || self.expansion.is_open(&id.key(), true);
        self.push(
            id.clone(),
            &project.machine_id,
            1,
            (has_children && !filtered).then_some(open),
            Kind::Project {
                project: project.clone(),
                sessions,
            },
        );
        if !open {
            return;
        }
        for worktree in &entry.worktrees {
            if matched.is_some_and(|matched| !matched.worktrees.contains(&worktree.id)) {
                continue;
            }
            let own = listed(self.snapshot, placement.of_worktree(&worktree.id));
            let live = self.shown_live(placement, placement.live_of_worktree(&worktree.id));
            // A worktree with something running in it is open.
            let recent = !live.is_empty()
                || own.first().is_some_and(|place| {
                    now - self.snapshot.sessions[*place].updated_at <= Duration::days(RECENT_DAYS)
                });
            let node = NodeId::Worktree(worktree.id.clone());
            let open = self.expansion.is_open(&node.key(), recent);
            self.push(
                node.clone(),
                &project.machine_id,
                2,
                (!filtered && (!own.is_empty() || !live.is_empty())).then_some(open),
                Kind::Worktree {
                    worktree: worktree.clone(),
                    sessions: own.len(),
                },
            );
            if open {
                for place in live {
                    self.live(placement, place, &project.machine_id, 3);
                }
                self.sessions(&own, &node, &project.machine_id, 3);
            }
        }
        for place in live_loose {
            self.live(placement, *place, &project.machine_id, 2);
        }
        self.sessions(&loose, &id, &project.machine_id, 2);
    }

    /// The row of a live session. Live sessions are never hidden behind
    /// "show more": they are what is running. One that resumed a history
    /// session of its own folder has no row: that session's row is it.
    fn live(&mut self, placement: &Placement, place: usize, machine: &MachineId, depth: u8) {
        let entry = &placement.live[place];
        if self.is_merged(entry) {
            return;
        }
        self.push(
            NodeId::Live(entry.id),
            machine,
            depth,
            None,
            Kind::Live(entry.clone()),
        );
    }

    fn unsorted(&mut self, machine: &MachineId, folders: &[Folder]) {
        let id = NodeId::Unsorted(machine.clone());
        let open = self.expansion.is_open(&id.key(), false);
        self.push(
            id,
            machine,
            1,
            Some(open),
            Kind::Unsorted {
                sessions: folders.iter().map(|folder| folder.sessions.len()).sum(),
            },
        );
        if !open {
            return;
        }
        for folder in folders {
            let node = NodeId::Folder(machine.clone(), folder.cwd.clone());
            let open = self.expansion.is_open(&node.key(), false);
            self.push(
                node.clone(),
                machine,
                2,
                Some(open),
                Kind::Folder {
                    cwd: folder.cwd.clone(),
                    sessions: folder.sessions.len(),
                },
            );
            if open {
                self.sessions(&folder.sessions, &node, machine, 3);
            }
        }
    }

    /// The Pinned section of a machine: its pinned sessions, in their pinned
    /// order, above its projects. In a filtered tree, only the ones the filter
    /// keeps (`visible`), and the section is open.
    fn pinned(
        &mut self,
        placement: &Placement,
        machine: &MachineId,
        depth: u8,
        visible: Option<&HashSet<usize>>,
    ) {
        let places: Vec<usize> = placement
            .pinned
            .get(machine)
            .into_iter()
            .flatten()
            .copied()
            .filter(|place| visible.is_none_or(|visible| visible.contains(place)))
            .collect();
        if places.is_empty() {
            return;
        }
        let id = NodeId::Pinned(machine.clone());
        let filtered = visible.is_some();
        let open = filtered || self.expansion.is_open(&id.key(), true);
        self.push(
            id,
            machine,
            depth,
            (!filtered).then_some(open),
            Kind::Pinned {
                sessions: places.len(),
            },
        );
        if !open {
            return;
        }
        for place in places {
            let session = &self.snapshot.sessions[place];
            self.push(
                NodeId::Session(session.id.clone()),
                machine,
                depth + 1,
                None,
                Kind::Session(session.clone()),
            );
        }
    }

    /// Whether a live terminal has no row of its own: it resumed a history
    /// session that the tree shows as its row.
    fn is_merged(&self, entry: &LiveEntry) -> bool {
        entry
            .history
            .as_ref()
            .is_some_and(|history| self.merged.contains(history))
    }

    /// The live sessions among `places` that get a row of their own.
    fn shown_live(&self, placement: &Placement, places: &[usize]) -> Vec<usize> {
        places
            .iter()
            .copied()
            .filter(|place| !self.is_merged(&placement.live[*place]))
            .collect()
    }

    /// The sessions under `parent`, up to the cap, then the row that shows
    /// the rest. A session with a live terminal is shown whatever the cap
    /// says: it is what is running.
    fn sessions(&mut self, places: &[usize], parent: &NodeId, machine: &MachineId, depth: u8) {
        let places = listed(self.snapshot, places);
        let all = self.expansion.shows_all_under(&parent.key());
        let shown = if all {
            places.len()
        } else {
            crate::settings::sessions_shown()
        };
        let mut hidden = 0;
        for (at, place) in places.iter().enumerate() {
            let session = &self.snapshot.sessions[*place];
            if at >= shown && !self.merged.contains(&session.id) {
                hidden += 1;
                continue;
            }
            self.push(
                NodeId::Session(session.id.clone()),
                machine,
                depth,
                None,
                Kind::Session(session.clone()),
            );
        }
        if hidden > 0 {
            self.push(
                NodeId::More(Box::new(parent.clone())),
                machine,
                depth,
                None,
                Kind::More { hidden },
            );
        }
    }
}

/// Opens everything above `target` (and shows the rest of a capped list that
/// holds it), so that its row exists. Does nothing when the node is not in the
/// tree.
pub fn reveal(
    snapshot: &Snapshot,
    placement: &Placement,
    target: &NodeId,
    expansion: &mut Expansion,
    now: DateTime<Utc>,
) {
    let everything = build_rows(snapshot, placement, &Expansion::everything(), now);
    let Some(at) = everything.iter().position(|row| &row.id == target) else {
        return;
    };
    let mut depth = everything[at].depth;
    let mut siblings_before = 0;
    for row in everything[..at].iter().rev() {
        if row.depth == everything[at].depth && depth == everything[at].depth {
            siblings_before += 1;
        }
        if row.depth < depth {
            expansion.set_open(&row.key(), true);
            if depth == everything[at].depth
                && matches!(target, NodeId::Session(_))
                && siblings_before >= crate::settings::sessions_shown()
            {
                expansion.show_all_under(&row.key());
            }
            depth = row.depth;
            if depth == 0 {
                break;
            }
        }
    }
}

// ----- moving through the rows ----------------------------------------------------

/// What list a sidebar row can be moved in, with the row itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Order {
    /// A project among the projects of its machine.
    Project(ProjectId, MachineId),
    /// A worktree among the worktrees of its project.
    Worktree(WorktreeId, ProjectId),
    /// A session among the sessions of one parent, pinned by dragging.
    Session(SessionId, SessionScope),
}

/// The sidebar order of `scope`, and the rows a drag can target in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeOrder {
    /// The scope: what `rows` holds.
    pub order: Order,
    /// The scope's rows in order: projects, worktrees, or sessions.
    pub rows: Vec<NodeId>,
}

/// The new sidebar order after `dragged` is dropped onto `target`: the
/// dragged row is taken out and put before the target, or after it when
/// `after`. `None` when either is not in `current` or both are the same.
pub fn dropped_order<T: PartialEq + Clone>(
    current: &[T],
    dragged: &T,
    target: &T,
    after: bool,
) -> Option<Vec<T>> {
    if !current.contains(dragged) {
        return None;
    }
    placed_order(current, dragged, target, after)
}

/// The order after `dragged` is dropped onto `target`, before it or after it
/// when `after`: `dragged` joins the order when it is not in it yet. `None`
/// when `target` is not in `current`, or when both are the same.
pub fn placed_order<T: PartialEq + Clone>(
    current: &[T],
    dragged: &T,
    target: &T,
    after: bool,
) -> Option<Vec<T>> {
    if dragged == target {
        return None;
    }
    let mut rest: Vec<T> = current
        .iter()
        .filter(|id| *id != dragged)
        .cloned()
        .collect();
    let at = rest.iter().position(|id| id == target)?;
    rest.insert(at + usize::from(after), dragged.clone());
    Some(rest)
}

/// The new sidebar order after the row at `at` moves one step: `delta`
/// is -1 (up) or 1 (down). At the ends the order is unchanged.
pub fn stepped_order<T: Clone>(current: &[T], at: usize, delta: isize) -> Vec<T> {
    let mut next = current.to_vec();
    if next.is_empty() {
        return next;
    }
    let last = next.len() - 1;
    let to = at.min(last).saturating_add_signed(delta).min(last);
    if to != at.min(last) {
        let moved = next.remove(at.min(last));
        next.insert(to, moved);
    }
    next
}

/// The row `delta` rows from `from` (negative is up), stopping at the ends.
pub fn step(rows: &[Row], from: usize, delta: isize) -> Option<usize> {
    let last = rows.len().checked_sub(1)?;
    Some(from.saturating_add_signed(delta).min(last))
}

/// The first row.
pub fn first(rows: &[Row]) -> Option<usize> {
    (!rows.is_empty()).then_some(0)
}

/// The last row.
pub fn last(rows: &[Row]) -> Option<usize> {
    rows.len().checked_sub(1)
}

/// Where the cursor goes after the rows were rebuilt: on the same node when it
/// is still there, else on the nearest row to the old place.
pub fn follow(rows: &[Row], key: Option<&str>, old_index: usize) -> Option<usize> {
    if let Some(found) = key.and_then(|key| rows.iter().position(|row| row.key() == key)) {
        return Some(found);
    }
    let last = rows.len().checked_sub(1)?;
    Some(old_index.min(last))
}

/// The row of the parent of the row at `at`.
pub fn parent_of(rows: &[Row], at: usize) -> Option<usize> {
    let depth = rows.get(at)?.depth;
    rows[..at].iter().rposition(|row| row.depth < depth)
}

/// The row of the first child of the row at `at`, when it is open.
pub fn first_child_of(rows: &[Row], at: usize) -> Option<usize> {
    let row = rows.get(at)?;
    let next = rows.get(at + 1)?;
    (row.open == Some(true) && next.depth > row.depth).then_some(at + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use leon_core::{AgentId, MachineKind};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
    }

    fn machine(id: &str, name: &str) -> Machine {
        Machine {
            id: MachineId::from_string(id),
            name: name.to_owned(),
            kind: if id == "local" {
                MachineKind::Local
            } else {
                MachineKind::Ssh {
                    host: "box".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                }
            },
        }
    }

    fn worktree(id: &str, project: &str, path: &str, branch: Option<&str>, main: bool) -> Worktree {
        Worktree {
            id: WorktreeId::from_string(id),
            project_id: ProjectId::from_string(project),
            path: path.to_owned(),
            branch: branch.map(str::to_owned),
            head: None,
            is_main: main,
            merged_pull_request: None,
        }
    }

    fn entry(
        id: &str,
        machine: &str,
        name: &str,
        root: &str,
        worktrees: Vec<Worktree>,
    ) -> ProjectEntry {
        ProjectEntry {
            project: Project {
                id: ProjectId::from_string(id),
                machine_id: MachineId::from_string(machine),
                name: name.to_owned(),
                root: root.to_owned(),
            },
            worktrees,
        }
    }

    fn session(id: &str, machine: &str, cwd: &str, minutes_ago: i64) -> Session {
        let at = now() - Duration::minutes(minutes_ago);
        Session {
            id: SessionId::from_string(id),
            agent: AgentId::CLAUDE,
            external_id: id.to_owned(),
            machine_id: MachineId::from_string(machine),
            cwd: cwd.to_owned(),
            project_id: None,
            title: id.to_owned(),
            model: None,
            started_at: at,
            updated_at: at,
            message_count: 1,
            sort_order: None,
        }
    }

    /// Local: api (main + a linked worktree) and web (main only), a box with
    /// nothing on it.
    fn fixture(sessions: Vec<Session>) -> Snapshot {
        let mut sessions = sessions;
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        Snapshot {
            machines: vec![machine("local", "This machine"), machine("box", "box")],
            projects: vec![
                entry(
                    "api",
                    "local",
                    "api",
                    "/srv/api",
                    vec![
                        worktree("api-main", "api", "/srv/api", Some("main"), true),
                        worktree("api-x", "api", "/srv/api-worktrees/x", Some("x"), false),
                    ],
                ),
                entry(
                    "web",
                    "local",
                    "web",
                    "/srv/web",
                    vec![worktree("web-main", "web", "/srv/web", Some("main"), true)],
                ),
            ],
            sessions,
            icons: Default::default(),
            dismissed: Vec::new(),
        }
    }

    fn rows_of(snapshot: &Snapshot, expansion: &Expansion) -> Vec<Row> {
        build_rows(snapshot, &Placement::compute(snapshot), expansion, now())
    }

    /// A row as a short text: indentation by depth, then what it is.
    fn outline(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                let what = match &row.kind {
                    Kind::Machine(machine) => format!("machine {}", machine.name),
                    Kind::Project { project, sessions } => {
                        format!("project {} ({sessions})", project.name)
                    }
                    Kind::Worktree { worktree, sessions } => {
                        format!("worktree {} ({sessions})", worktree_label(worktree))
                    }
                    Kind::Pinned { sessions } => format!("pinned ({sessions})"),
                    Kind::Unsorted { sessions } => format!("unsorted ({sessions})"),
                    Kind::Folder { cwd, sessions } => format!("folder {cwd} ({sessions})"),
                    Kind::Session(session) => format!("session {}", session.id),
                    Kind::Live(entry) => format!("live {}", entry.id),
                    Kind::More { hidden } => format!("more {hidden}"),
                    Kind::Open => "open".to_owned(),
                    Kind::NoMatch => "no match".to_owned(),
                    Kind::NoActive => "no active".to_owned(),
                };
                format!("{}{what}", "  ".repeat(usize::from(row.depth)))
            })
            .collect()
    }

    #[test]
    fn with_one_machine_the_header_is_left_out_and_the_projects_sit_at_the_top() {
        let mut snapshot = fixture(vec![]);
        snapshot.machines.truncate(1);
        let rows = rows_of(&snapshot, &Expansion::default());
        assert!(
            !rows.iter().any(|row| matches!(row.kind, Kind::Machine(_))),
            "{:?}",
            outline(&rows)
        );
        assert_eq!(
            outline(&rows)[..3],
            ["project api (0)", "  worktree main (0)", "  worktree x (0)"]
        );
        assert_eq!(project_depth(&snapshot), 0);
        // Folding the machine is not possible without its header: whatever the
        // expansion says, the projects are there.
        let mut folded = Expansion::default();
        folded.set_open(
            &NodeId::Machine(MachineId::from_string("local")).key(),
            false,
        );
        assert_eq!(outline(&rows_of(&snapshot, &folded)), outline(&rows));
    }

    #[test]
    fn with_two_machines_each_has_its_header_and_the_projects_sit_one_level_in() {
        let snapshot = fixture(vec![]);
        let rows = rows_of(&snapshot, &Expansion::default());
        assert!(matches!(rows[0].kind, Kind::Machine(_)));
        assert_eq!(outline(&rows)[1], "  project api (0)");
        assert_eq!(project_depth(&snapshot), 1);
    }

    #[test]
    fn a_filtered_tree_is_open_on_its_own_and_leaves_the_expansion_alone() {
        use crate::ui::filter::{filter, project_labels};
        let snapshot = fixture(vec![]);
        let mut expansion = Expansion::default();
        expansion.set_open(&NodeId::Project(ProjectId::from_string("api")).key(), false);
        let before = expansion.clone();
        let labels = project_labels(&snapshot);
        let found = filter(&snapshot, &labels, "api").unwrap();
        let rows = build_rows_filtered(
            &snapshot,
            &Placement::compute(&snapshot),
            &expansion,
            now(),
            Some(&found),
        );
        assert_eq!(
            outline(&rows),
            [
                "machine This machine",
                "  project api (0)",
                "    worktree main (0)",
                "    worktree x (0)",
                "machine box",
            ],
            "folded in the file, open in the filtered view, and no chevrons to change it"
        );
        assert!(rows.iter().all(|row| row.open.is_none()));
        assert_eq!(expansion, before);
        let nothing = filter(&snapshot, &labels, "zzzz").unwrap();
        let rows = build_rows_filtered(
            &snapshot,
            &Placement::compute(&snapshot),
            &expansion,
            now(),
            Some(&nothing),
        );
        assert_eq!(
            outline(&rows),
            ["machine This machine", "machine box", "  no match"]
        );
    }

    #[test]
    fn ancestors_walk_from_the_folder_up_with_either_separator() {
        assert_eq!(
            ancestors("/srv/api/src/").collect::<Vec<_>>(),
            ["/srv/api/src", "/srv/api", "/srv"]
        );
        assert_eq!(
            ancestors("C:\\code\\api").collect::<Vec<_>>(),
            ["C:\\code\\api", "C:\\code", "C:"]
        );
        assert_eq!(ancestors("/").collect::<Vec<_>>(), ["/"]);
        assert_eq!(ancestors("").count(), 0);
    }

    #[test]
    fn a_folder_is_named_after_its_last_part() {
        assert_eq!(folder_name("/srv/api-worktrees/x/"), "x");
        assert_eq!(folder_name("C:\\code\\api"), "api");
        assert_eq!(folder_name("/"), "/");
        let detached = worktree("w", "p", "/srv/wt/detached", None, false);
        assert_eq!(worktree_label(&detached), "detached");
        let branch = worktree("w", "p", "/srv/wt/x", Some("feature/x"), false);
        assert_eq!(worktree_label(&branch), "feature/x");
    }

    #[test]
    fn a_session_in_the_project_root_or_below_hangs_under_the_main_worktree() {
        let snapshot = fixture(vec![
            session("root", "local", "/srv/api", 1),
            session("below", "local", "/srv/api/src/deep", 2),
            session("sibling", "local", "/srv/api-old", 3),
        ]);
        let placement = Placement::compute(&snapshot);
        let main = WorktreeId::from_string("api-main");
        assert_eq!(placement.of_worktree(&main), [0, 1]);
        assert!(!placement
            .in_worktree
            .contains_key(&WorktreeId::from_string("api-x")));
        assert_eq!(
            placement.unsorted[&MachineId::local()][0].cwd,
            "/srv/api-old",
            "a folder that only shares a prefix is not inside"
        );
    }

    #[test]
    fn a_session_of_a_removed_worktree_inside_a_project_is_not_placed_again() {
        let mut snapshot = fixture(vec![
            session("kept", "local", "/srv/api/src", 1),
            session("gone", "local", "/srv/api/.claude/worktrees/x/src", 2),
            session("sibling", "local", "/srv/api-worktrees/old", 3),
        ]);
        snapshot.dismissed = vec![
            (
                MachineId::local(),
                "/srv/api/.claude/worktrees/x".to_owned(),
            ),
            (MachineId::local(), "/srv/api-worktrees/old".to_owned()),
        ];
        let placement = Placement::compute(&snapshot);
        assert_eq!(
            placement.of_worktree(&WorktreeId::from_string("api-main")),
            [0],
            "the main checkout does not take the sessions of a worktree that is gone"
        );
        assert_eq!(
            placement.unsorted[&MachineId::local()][0].cwd,
            "/srv/api-worktrees/old",
            "outside every project the history stays where it was"
        );
    }

    #[test]
    fn a_session_started_in_a_linked_worktree_hangs_under_that_worktree() {
        let snapshot = fixture(vec![session(
            "linked",
            "local",
            "/srv/api-worktrees/x/src",
            1,
        )]);
        let placement = Placement::compute(&snapshot);
        assert_eq!(
            placement.of_worktree(&WorktreeId::from_string("api-x")),
            [0]
        );
        assert!(placement
            .of_worktree(&WorktreeId::from_string("api-main"))
            .is_empty());
    }

    #[test]
    fn the_deepest_containing_path_wins_even_when_a_worktree_sits_inside_the_root() {
        let mut snapshot = fixture(vec![session("nested", "local", "/srv/api/.wt/y/src", 1)]);
        snapshot.projects[0].worktrees.push(worktree(
            "api-y",
            "api",
            "/srv/api/.wt/y",
            Some("y"),
            false,
        ));
        let placement = Placement::compute(&snapshot);
        assert_eq!(
            placement.of_worktree(&WorktreeId::from_string("api-y")),
            [0]
        );
        assert!(placement
            .of_worktree(&WorktreeId::from_string("api-main"))
            .is_empty());
    }

    #[test]
    fn the_same_path_on_another_machine_is_not_inside_the_project() {
        let snapshot = fixture(vec![session("far", "box", "/srv/api", 1)]);
        let placement = Placement::compute(&snapshot);
        assert!(placement.in_worktree.is_empty());
        assert_eq!(
            placement.unsorted[&MachineId::from_string("box")][0].cwd,
            "/srv/api"
        );
    }

    #[test]
    fn a_session_inside_a_project_whose_worktrees_were_never_listed_hangs_under_the_project() {
        let mut snapshot = fixture(vec![session("bare", "local", "/srv/new/src", 1)]);
        snapshot
            .projects
            .push(entry("new", "local", "new", "/srv/new", vec![]));
        let placement = Placement::compute(&snapshot);
        assert_eq!(placement.in_project[&ProjectId::from_string("new")], [0]);
    }

    #[test]
    fn sessions_of_a_machine_nobody_knows_are_left_out() {
        let snapshot = fixture(vec![session("ghost", "gone", "/srv/api", 1)]);
        let placement = Placement::compute(&snapshot);
        assert_eq!(placement, Placement::default());
    }

    #[test]
    fn unsorted_sessions_are_grouped_by_folder_the_newest_folder_first() {
        let snapshot = fixture(vec![
            session("a", "local", "/tmp/one", 5),
            session("b", "local", "/tmp/two", 1),
            session("c", "local", "/tmp/one", 3),
        ]);
        let placement = Placement::compute(&snapshot);
        let folders = &placement.unsorted[&MachineId::local()];
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].cwd, "/tmp/two");
        assert_eq!(folders[1].cwd, "/tmp/one");
        assert_eq!(folders[1].sessions.len(), 2);
    }

    #[test]
    fn the_rows_go_machine_project_worktree_session_each_one_level_deeper() {
        let snapshot = fixture(vec![
            session("fresh", "local", "/srv/api", 5),
            session("old", "local", "/srv/api-worktrees/x", 60 * 24 * 30),
            session("far", "box", "/tmp/far", 9),
        ]);
        let rows = rows_of(&snapshot, &Expansion::default());
        assert_eq!(
            outline(&rows),
            [
                "machine This machine",
                "  project api (2)",
                "    worktree main (1)",
                "      session fresh",
                "    worktree x (1)",
                "  project web (0)",
                "    worktree main (0)",
                "machine box",
                "  open",
                "  unsorted (1)",
            ]
        );
    }

    #[test]
    fn a_worktree_is_open_by_default_only_when_a_session_of_the_last_week_ran_in_it() {
        let snapshot = fixture(vec![
            session("fresh", "local", "/srv/api", 60 * 24 * 6),
            session("stale", "local", "/srv/api-worktrees/x", 60 * 24 * 8),
        ]);
        let rows = rows_of(&snapshot, &Expansion::default());
        let main = rows
            .iter()
            .find(|row| row.id == NodeId::Worktree(WorktreeId::from_string("api-main")))
            .unwrap();
        let linked = rows
            .iter()
            .find(|row| row.id == NodeId::Worktree(WorktreeId::from_string("api-x")))
            .unwrap();
        assert_eq!(main.open, Some(true));
        assert_eq!(linked.open, Some(false));
        assert!(!rows
            .iter()
            .any(|row| row.id == NodeId::Session(SessionId::from_string("stale"))));
    }

    #[test]
    fn a_worktree_without_sessions_has_nothing_to_open() {
        let snapshot = fixture(vec![]);
        let rows = rows_of(&snapshot, &Expansion::default());
        let web = rows
            .iter()
            .find(|row| row.id == NodeId::Worktree(WorktreeId::from_string("web-main")))
            .unwrap();
        assert_eq!(web.open, None);
    }

    #[test]
    fn machines_and_projects_are_open_by_default_and_the_unsorted_node_is_folded() {
        let snapshot = fixture(vec![session("loose", "local", "/tmp/x", 1)]);
        let rows = rows_of(&snapshot, &Expansion::default());
        let open = |wanted: &NodeId| rows.iter().find(|row| &row.id == wanted).unwrap().open;
        assert_eq!(open(&NodeId::Machine(MachineId::local())), Some(true));
        assert_eq!(
            open(&NodeId::Project(ProjectId::from_string("api"))),
            Some(true)
        );
        assert_eq!(open(&NodeId::Unsorted(MachineId::local())), Some(false));
        assert!(!rows
            .iter()
            .any(|row| matches!(row.kind, Kind::Folder { .. })));
    }

    #[test]
    fn a_folded_node_hides_everything_below_it_and_an_opened_one_shows_it() {
        let snapshot = fixture(vec![session("loose", "local", "/tmp/x", 1)]);
        let mut expansion = Expansion::default();
        expansion.set_open(&NodeId::Project(ProjectId::from_string("api")).key(), false);
        expansion.set_open(&NodeId::Unsorted(MachineId::local()).key(), true);
        expansion.set_open(
            &NodeId::Folder(MachineId::local(), "/tmp/x".into()).key(),
            true,
        );
        let rows = outline(&rows_of(&snapshot, &expansion));
        assert_eq!(
            rows[..7],
            [
                "machine This machine",
                "  project api (0)",
                "  project web (0)",
                "    worktree main (0)",
                "  unsorted (1)",
                "    folder /tmp/x (1)",
                "      session loose",
            ]
        );
    }

    #[test]
    fn a_machine_can_be_folded_and_stays_in_the_list() {
        let snapshot = fixture(vec![]);
        let mut expansion = Expansion::default();
        expansion.set_open(&NodeId::Machine(MachineId::local()).key(), false);
        let rows = outline(&rows_of(&snapshot, &expansion));
        assert_eq!(rows, ["machine This machine", "machine box", "  open"]);
    }

    #[test]
    fn a_machine_without_projects_says_how_to_open_one_even_when_it_has_unsorted_sessions() {
        let snapshot = fixture(vec![session("far", "box", "/tmp/far", 1)]);
        let rows = rows_of(&snapshot, &Expansion::default());
        let at = rows
            .iter()
            .position(|row| row.id == NodeId::Open(MachineId::from_string("box")))
            .unwrap();
        assert!(matches!(rows[at].kind, Kind::Open));
        assert!(matches!(rows[at + 1].kind, Kind::Unsorted { .. }));
    }

    fn crowded(count: usize) -> Snapshot {
        fixture(
            (0..count)
                .map(|n| session(&format!("s{n:02}"), "local", "/srv/api", n as i64 + 1))
                .collect(),
        )
    }

    #[test]
    fn a_worktree_shows_eight_sessions_and_a_row_for_the_rest() {
        let snapshot = crowded(11);
        let rows = rows_of(&snapshot, &Expansion::default());
        let sessions = rows
            .iter()
            .filter(|row| matches!(row.kind, Kind::Session(_)))
            .count();
        assert_eq!(sessions, 8);
        let more = rows
            .iter()
            .find(|row| matches!(row.kind, Kind::More { .. }))
            .unwrap();
        assert_eq!(more.kind, Kind::More { hidden: 3 });
        assert_eq!(more.depth, 3);
        assert_eq!(
            more.id,
            NodeId::More(Box::new(NodeId::Worktree(WorktreeId::from_string(
                "api-main"
            ))))
        );
    }

    #[test]
    fn showing_more_expands_the_list_in_place() {
        let snapshot = crowded(11);
        let mut expansion = Expansion::default();
        expansion.show_all_under(&NodeId::Worktree(WorktreeId::from_string("api-main")).key());
        let rows = rows_of(&snapshot, &expansion);
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(row.kind, Kind::Session(_)))
                .count(),
            11
        );
        assert!(!rows.iter().any(|row| matches!(row.kind, Kind::More { .. })));
    }

    #[test]
    fn pinned_sessions_come_first_in_their_worktree_in_pinned_order() {
        let mut old = session("old", "local", "/srv/api", 60);
        old.sort_order = Some(1);
        let mut older = session("older", "local", "/srv/api", 120);
        older.sort_order = Some(0);
        let snapshot = fixture(vec![old, older, session("new", "local", "/srv/api", 1)]);
        let placement = Placement::compute(&snapshot);
        let titles: Vec<&str> = placement
            .of_worktree(&WorktreeId::from_string("api-main"))
            .iter()
            .map(|place| snapshot.sessions[*place].title.as_str())
            .collect();
        assert_eq!(titles, ["older", "old", "new"]);
    }

    #[test]
    fn a_dropped_session_takes_its_place_in_the_pinned_order() {
        let id = SessionId::from_string;
        let pins = [id("a"), id("b"), id("c")];
        // Before or after the target, the dragged session leaves its old place.
        assert_eq!(
            placed_order(&pins, &id("c"), &id("a"), false),
            Some(vec![id("c"), id("a"), id("b")])
        );
        assert_eq!(
            placed_order(&pins, &id("a"), &id("c"), true),
            Some(vec![id("b"), id("c"), id("a")])
        );
        // A session that is not pinned yet joins the order where it is dropped.
        assert_eq!(
            placed_order(&pins, &id("x"), &id("b"), true),
            Some(vec![id("a"), id("b"), id("x"), id("c")])
        );
        // Nothing to do when it lands on itself, or on a row that is not pinned.
        assert_eq!(placed_order(&pins, &id("a"), &id("a"), false), None);
        assert_eq!(placed_order(&pins, &id("a"), &id("nope"), false), None);
    }

    #[test]
    fn the_pinned_sessions_of_a_machine_are_gathered_in_their_pinned_order() {
        let mut second = session("second", "local", "/srv/web", 60);
        second.sort_order = Some(1);
        let mut first = session("first", "local", "/srv/api", 120);
        first.sort_order = Some(0);
        let snapshot = fixture(vec![
            second,
            first,
            session("plain", "local", "/srv/api", 1),
        ]);
        let placement = Placement::compute(&snapshot);
        let titles: Vec<&str> = placement
            .pinned
            .values()
            .flatten()
            .map(|place| snapshot.sessions[*place].title.as_str())
            .collect();
        assert_eq!(titles, ["first", "second"]);
    }

    #[test]
    fn a_pinned_session_is_in_the_pinned_section_and_not_under_its_worktree() {
        let mut pinned = session("pinned", "local", "/srv/api", 5);
        pinned.sort_order = Some(0);
        let snapshot = fixture(vec![pinned, session("plain", "local", "/srv/api", 1)]);
        let lines = outline(&rows_of(&snapshot, &Expansion::everything()));
        assert!(lines.contains(&"  pinned (1)".to_owned()), "{lines:?}");
        assert!(
            lines.contains(&"    session pinned".to_owned()),
            "{lines:?}"
        );
        assert!(lines.contains(&"  project api (1)".to_owned()), "{lines:?}");
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("session pinned"))
                .count(),
            1,
            "{lines:?}"
        );
    }

    #[test]
    fn a_worktree_whose_only_session_is_pinned_has_nothing_to_open() {
        let mut pinned = session("pinned", "local", "/srv/api", 5);
        pinned.sort_order = Some(0);
        let snapshot = fixture(vec![pinned]);
        let rows = rows_of(&snapshot, &Expansion::everything());
        let main = rows
            .iter()
            .find(|row| {
                matches!(&row.id, NodeId::Worktree(id) if *id == WorktreeId::from_string("api-main"))
            })
            .expect("the worktree is listed");
        assert!(matches!(main.kind, Kind::Worktree { sessions: 0, .. }));
        assert_eq!(main.open, None);
    }

    #[test]
    fn a_filter_shows_the_pinned_sessions_of_the_worktrees_it_keeps() {
        use crate::ui::filter::{filter, project_labels};
        let mut in_api = session("in-api", "local", "/srv/api", 5);
        in_api.sort_order = Some(0);
        let mut in_web = session("in-web", "local", "/srv/web", 6);
        in_web.sort_order = Some(1);
        let snapshot = fixture(vec![in_api, in_web]);
        let found = filter(&snapshot, &project_labels(&snapshot), "api").unwrap();
        let rows = build_rows_filtered(
            &snapshot,
            &Placement::compute(&snapshot),
            &Expansion::default(),
            now(),
            Some(&found),
        );
        let lines = outline(&rows);
        assert!(
            lines.contains(&"    session in-api".to_owned()),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("in-web")),
            "{lines:?}"
        );
    }

    #[test]
    fn the_pinned_section_folds_like_any_other_node() {
        let mut pinned = session("pinned", "local", "/srv/api", 5);
        pinned.sort_order = Some(0);
        let snapshot = fixture(vec![pinned]);
        // Open by default, like a project; folded, it keeps only its header.
        let mut expansion = Expansion::default();
        let lines = outline(&rows_of(&snapshot, &expansion));
        assert!(
            lines.contains(&"    session pinned".to_owned()),
            "{lines:?}"
        );
        expansion.set_open(
            &NodeId::Pinned(MachineId::from_string("local")).key(),
            false,
        );
        let lines = outline(&rows_of(&snapshot, &expansion));
        assert!(lines.contains(&"  pinned (1)".to_owned()), "{lines:?}");
        assert!(
            !lines.iter().any(|line| line.contains("session pinned")),
            "{lines:?}"
        );
    }

    #[test]
    fn with_one_machine_the_pinned_section_is_a_top_level_row() {
        let mut pinned = session("pinned", "local", "/srv/api", 5);
        pinned.sort_order = Some(0);
        let mut snapshot = fixture(vec![pinned]);
        snapshot.machines.truncate(1);
        let rows = rows_of(&snapshot, &Expansion::everything());
        assert!(matches!(rows[0].kind, Kind::Pinned { sessions: 1 }));
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn sessions_stay_newest_first_under_their_worktree() {
        let snapshot = fixture(vec![
            session("older", "local", "/srv/api", 30),
            session("newer", "local", "/srv/api", 3),
        ]);
        let titles: Vec<String> = rows_of(&snapshot, &Expansion::default())
            .into_iter()
            .filter_map(|row| match row.kind {
                Kind::Session(session) => Some(session.title),
                _ => None,
            })
            .collect();
        assert_eq!(titles, ["newer", "older"]);
    }

    #[test]
    fn every_node_has_its_own_key() {
        let snapshot = crowded(11);
        let rows = rows_of(&snapshot, &Expansion::default());
        let mut keys: Vec<String> = rows.iter().map(Row::key).collect();
        let total = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), total);
        assert_ne!(
            NodeId::Unsorted(MachineId::local()).key(),
            NodeId::Machine(MachineId::local()).key()
        );
    }

    #[test]
    fn revealing_a_node_opens_everything_above_it() {
        let snapshot = fixture(vec![session(
            "old",
            "local",
            "/srv/api-worktrees/x",
            60 * 24 * 30,
        )]);
        let placement = Placement::compute(&snapshot);
        let mut expansion = Expansion::default();
        expansion.set_open(&NodeId::Project(ProjectId::from_string("api")).key(), false);
        expansion.set_open(&NodeId::Machine(MachineId::local()).key(), false);
        let target = NodeId::Session(SessionId::from_string("old"));
        assert!(!build_rows(&snapshot, &placement, &expansion, now())
            .iter()
            .any(|row| row.id == target));

        reveal(&snapshot, &placement, &target, &mut expansion, now());

        assert!(build_rows(&snapshot, &placement, &expansion, now())
            .iter()
            .any(|row| row.id == target));
    }

    #[test]
    fn revealing_a_session_past_the_cap_shows_the_rest_of_its_list() {
        let snapshot = crowded(12);
        let placement = Placement::compute(&snapshot);
        let mut expansion = Expansion::default();
        let target = NodeId::Session(SessionId::from_string("s10"));
        reveal(&snapshot, &placement, &target, &mut expansion, now());
        assert!(build_rows(&snapshot, &placement, &expansion, now())
            .iter()
            .any(|row| row.id == target));
        // A session inside the cap does not widen the list.
        let mut expansion = Expansion::default();
        reveal(
            &snapshot,
            &placement,
            &NodeId::Session(SessionId::from_string("s02")),
            &mut expansion,
            now(),
        );
        assert!(build_rows(&snapshot, &placement, &expansion, now())
            .iter()
            .any(|row| matches!(row.kind, Kind::More { .. })));
    }

    #[test]
    fn revealing_a_node_that_is_not_in_the_tree_changes_nothing() {
        let snapshot = fixture(vec![]);
        let placement = Placement::compute(&snapshot);
        let mut expansion = Expansion::default();
        reveal(
            &snapshot,
            &placement,
            &NodeId::Session(SessionId::from_string("nope")),
            &mut expansion,
            now(),
        );
        assert_eq!(expansion, Expansion::default());
    }

    #[test]
    fn the_cursor_moves_by_rows_and_stops_at_the_ends() {
        let rows = rows_of(&fixture(vec![]), &Expansion::default());
        assert_eq!(step(&rows, 0, 1), Some(1));
        assert_eq!(step(&rows, 0, -1), Some(0));
        assert_eq!(step(&rows, 2, 500), Some(rows.len() - 1));
        assert_eq!(step(&[], 0, 1), None);
        assert_eq!(first(&rows), Some(0));
        assert_eq!(last(&rows), Some(rows.len() - 1));
        assert_eq!((first(&[]), last(&[])), (None, None));
    }

    #[test]
    fn after_a_rebuild_the_cursor_follows_its_node_or_the_nearest_row() {
        let rows = rows_of(&fixture(vec![]), &Expansion::default());
        let key = rows[3].key();
        assert_eq!(follow(&rows, Some(&key), 0), Some(3));
        assert_eq!(follow(&rows, Some("gone"), 2), Some(2));
        assert_eq!(follow(&rows, None, 999), Some(rows.len() - 1));
        assert_eq!(follow(&[], None, 0), None);
    }

    #[test]
    fn a_row_knows_its_parent_and_first_child() {
        let snapshot = fixture(vec![session("fresh", "local", "/srv/api", 5)]);
        let rows = rows_of(&snapshot, &Expansion::default());
        let at = |id: NodeId| rows.iter().position(|row| row.id == id).unwrap();
        let project = at(NodeId::Project(ProjectId::from_string("api")));
        let main = at(NodeId::Worktree(WorktreeId::from_string("api-main")));
        let fresh = at(NodeId::Session(SessionId::from_string("fresh")));
        assert_eq!(parent_of(&rows, fresh), Some(main));
        assert_eq!(parent_of(&rows, main), Some(project));
        assert_eq!(parent_of(&rows, 0), None);
        assert_eq!(first_child_of(&rows, project), Some(project + 1));
        assert_eq!(
            first_child_of(&rows, fresh),
            None,
            "a session has no children"
        );
        let linked = at(NodeId::Worktree(WorktreeId::from_string("api-x")));
        assert_eq!(first_child_of(&rows, linked), None, "nothing under it");
    }

    #[test]
    fn a_folded_row_has_no_first_child_to_go_to() {
        let snapshot = fixture(vec![]);
        let mut expansion = Expansion::default();
        expansion.set_open(&NodeId::Project(ProjectId::from_string("api")).key(), false);
        let rows = rows_of(&snapshot, &expansion);
        assert_eq!(first_child_of(&rows, 1), None);
    }

    #[test]
    fn a_long_history_is_placed_without_asking_the_store_anything() {
        // Placement and rows are pure functions of the snapshot: 5000 sessions
        // across 200 folders come back in a blink.
        let sessions: Vec<Session> = (0..5_000)
            .map(|n| {
                session(
                    &format!("s{n}"),
                    "local",
                    &format!("/srv/api/dir{}/src", n % 200),
                    n,
                )
            })
            .collect();
        let snapshot = fixture(sessions);
        let started = std::time::Instant::now();
        let placement = Placement::compute(&snapshot);
        let rows = build_rows(&snapshot, &placement, &Expansion::default(), now());
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
        assert!(
            rows.len() < 40,
            "the cap keeps the list short: {}",
            rows.len()
        );
    }

    fn live(id: u64, machine: &str, cwd: &str) -> LiveEntry {
        LiveEntry {
            id: LiveId(id),
            machine: MachineId::from_string(machine),
            cwd: cwd.to_owned(),
            agent: Some(AgentId::CLAUDE),
            history: None,
        }
    }

    fn resumed(id: u64, machine: &str, cwd: &str, history: &str) -> LiveEntry {
        LiveEntry {
            history: Some(SessionId::from_string(history)),
            ..live(id, machine, cwd)
        }
    }

    fn live_rows(snapshot: &Snapshot, live: Vec<LiveEntry>) -> Vec<Row> {
        let placement = Placement::compute(snapshot).with_live(snapshot, live);
        build_rows(snapshot, &placement, &Expansion::default(), now())
    }

    #[test]
    fn a_terminal_that_resumed_a_session_of_its_folder_is_that_sessions_row() {
        let snapshot = fixture(vec![
            session("old", "local", "/srv/api", 30),
            session("other", "local", "/srv/api", 31),
        ]);
        let rows = live_rows(&snapshot, vec![resumed(1, "local", "/srv/api", "old")]);
        let lines = outline(&rows);
        assert!(
            !lines
                .iter()
                .any(|line| line.trim_start().starts_with("live ")),
            "no second row: {lines:?}"
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.trim_start().starts_with("session old"))
                .count(),
            1,
            "{lines:?}"
        );
    }

    #[test]
    fn a_terminal_that_resumed_a_session_elsewhere_keeps_its_own_row() {
        let snapshot = fixture(vec![session("old", "local", "/srv/api", 30)]);
        // It was resumed in the other worktree: the history row is not its row.
        let rows = live_rows(
            &snapshot,
            vec![resumed(1, "local", "/srv/api-worktrees/x", "old")],
        );
        let lines = outline(&rows);
        assert!(
            lines
                .iter()
                .any(|line| line.trim_start().starts_with("live 1")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_session_with_a_terminal_is_not_left_behind_show_more() {
        let sessions: Vec<Session> = (0..10)
            .map(|n| session(&format!("s{n}"), "local", "/srv/api", n))
            .collect();
        let snapshot = fixture(sessions);
        let rows = live_rows(&snapshot, vec![resumed(1, "local", "/srv/api", "s9")]);
        let lines = outline(&rows);
        assert!(
            lines
                .iter()
                .any(|line| line.trim_start().starts_with("session s9")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.trim_start().starts_with("more 1")),
            "one is still hidden: {lines:?}"
        );
    }

    #[test]
    fn a_live_session_hangs_under_its_worktree_above_the_history() {
        let snapshot = fixture(vec![session("old", "local", "/srv/api", 30)]);
        let rows = live_rows(&snapshot, vec![live(1, "local", "/srv/api/crates")]);
        let lines = outline(&rows);
        let main = lines
            .iter()
            .position(|l| l.trim_start().starts_with("worktree main"))
            .unwrap();
        assert!(
            lines[main + 1].trim_start().starts_with("live 1"),
            "{lines:?}"
        );
        assert!(
            lines[main + 2].trim_start().starts_with("session old"),
            "{lines:?}"
        );
        assert_eq!(rows[main + 1].depth, rows[main].depth + 1);
    }

    #[test]
    fn a_worktree_with_something_running_is_open_and_can_be_folded() {
        let snapshot = fixture(Vec::new());
        let rows = live_rows(&snapshot, vec![live(1, "local", "/srv/api-worktrees/x")]);
        let worktree = rows
            .iter()
            .find(|row| matches!(&row.kind, Kind::Worktree { worktree, .. } if worktree.id.as_str() == "api-x"))
            .unwrap();
        assert_eq!(worktree.open, Some(true));
        assert!(rows.iter().any(|row| matches!(row.kind, Kind::Live(_))));
    }

    #[test]
    fn a_live_session_outside_every_project_hangs_under_its_machine() {
        let snapshot = fixture(Vec::new());
        let rows = live_rows(&snapshot, vec![live(1, "local", "/tmp/scratch")]);
        let at = rows
            .iter()
            .position(|row| matches!(row.kind, Kind::Live(_)))
            .unwrap();
        assert_eq!(rows[at].depth, 1);
        assert!(matches!(rows[at - 1].kind, Kind::Machine(_)));
    }

    #[test]
    fn live_sessions_are_never_hidden_behind_show_more() {
        let snapshot = crowded(20);
        let many: Vec<LiveEntry> = (1..=12).map(|i| live(i, "local", "/srv/api")).collect();
        let rows = live_rows(&snapshot, many);
        let shown = rows
            .iter()
            .filter(|row| matches!(row.kind, Kind::Live(_)))
            .count();
        assert_eq!(shown, 12);
    }

    #[test]
    fn a_live_session_has_its_own_key_and_the_same_path_on_another_machine_does_not_match() {
        assert_eq!(NodeId::Live(LiveId(3)).key(), "live:3");
        let snapshot = fixture(Vec::new());
        let rows = live_rows(&snapshot, vec![live(1, "box", "/srv/api")]);
        let at = rows
            .iter()
            .position(|row| matches!(row.kind, Kind::Live(_)))
            .unwrap();
        assert_eq!(rows[at].machine, MachineId::from_string("box"));
        assert!(
            matches!(rows[at - 1].kind, Kind::Machine(_) | Kind::Open),
            "loose on the box, after its hint to open a project"
        );
    }

    #[test]
    fn a_folder_inside_a_worktree_belongs_to_that_worktree_and_others_to_themselves() {
        let snapshot = fixture(Vec::new());
        let local = MachineId::from_string("local");
        assert_eq!(
            workspace_root(&snapshot, &local, "/srv/api/crates/x"),
            "/srv/api"
        );
        assert_eq!(
            workspace_root(&snapshot, &local, "/srv/api-worktrees/x/src"),
            "/srv/api-worktrees/x"
        );
        assert_eq!(
            workspace_root(&snapshot, &local, "/tmp/scratch/"),
            "/tmp/scratch"
        );
        let other = MachineId::from_string("box");
        assert_eq!(workspace_root(&snapshot, &other, "/srv/api"), "/srv/api");
    }

    #[test]
    fn a_workspace_root_names_its_worktree_or_project() {
        let snapshot = fixture(Vec::new());
        let local = MachineId::from_string("local");
        let (project, worktree) =
            detail_of_root(&snapshot, &local, "/srv/api-worktrees/x").unwrap();
        assert_eq!(
            (project.as_str(), worktree.unwrap().as_str()),
            ("api", "api-x")
        );
        assert!(detail_of_root(&snapshot, &local, "/tmp/scratch").is_none());
    }

    #[test]
    fn folders_are_compared_by_identity_not_by_spelling() {
        assert!(same_folder(
            "C:/Users/me/code/api",
            r"c:\users\me\code\api\"
        ));
        assert!(same_folder(r"\\?\C:\a", "C:/a"));
        assert!(!same_folder("/srv/Api", "/srv/api"));
        assert!(!same_folder(r"/srv/a\b", "/srv/a/b"));
    }
}

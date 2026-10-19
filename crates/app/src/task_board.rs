//! The board of tasks as the window shows it, without a window.
//!
//! Which keys do what ([`action_of`]), where the cursor is and where it goes
//! ([`Board`]), what a key that carries a card asks the engine for
//! ([`Board::plan`]) and which board is shown ([`choose`]) are decided here,
//! as pure functions with tests; `ui/tasks_view.rs` only draws the result and
//! hands the plans to the engine.
//!
//! The columns are the board's open ones and done: `canceled` has no column
//! (a canceled task is out of the way, and `leon task list --all` shows it).
//! The cursor remembers the task it is on, not only its place, so a reload
//! that moved cards (an agent finished one, the engine placed another) keeps
//! the cursor on the same card, and a card the cursor carried to another
//! column is followed there.

use leon_core::{Task, TaskPatch, TaskPriority, TaskStatus};

/// The columns of the board, left to right.
pub const COLUMNS: [TaskStatus; 5] = [
    TaskStatus::Backlog,
    TaskStatus::Todo,
    TaskStatus::Doing,
    TaskStatus::Review,
    TaskStatus::Done,
];

/// Where the cursor starts: the column of tasks to do.
const FIRST_COLUMN: usize = 1;

/// The most tasks read for one column. A column is read on its own so that a
/// long backlog cannot push `doing` or `done` off the board.
pub const PER_COLUMN: usize = 200;

/// What a key does on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The cursor one column to the left.
    Left,
    /// The cursor one column to the right.
    Right,
    /// The cursor one card up.
    Up,
    /// The cursor one card down.
    Down,
    /// The card one column to the left.
    CardLeft,
    /// The card one column to the right.
    CardRight,
    /// The card one place up in its column.
    CardUp,
    /// The card one place down in its column.
    CardDown,
    /// Ask for the title of a new task in the cursor's column.
    New,
    /// The card to done.
    Done,
    /// The card's priority one step up, after urgent back to none.
    Priority,
    /// Switch between the project's board and the global one.
    Scope,
    /// Start work on the card: a worktree and a session, or open the one it
    /// has.
    Start,
    /// Open the card's details to read and edit them, or close them.
    Details,
    /// Sync with GitHub now, or ask to turn it on.
    Sync,
    /// Give the card to King Leon, who works it.
    King,
}

/// The action of a key pressed while the board has the keyboard, or `None`
/// when the key is not the board's. Bare keys only, plus Shift: a chord with
/// the platform key, Control or Alt belongs to the rest of the window.
pub fn action_of(key: &str, shift: bool, other_modifier: bool) -> Option<Action> {
    if other_modifier {
        return None;
    }
    let key = key.to_ascii_lowercase();
    let action = match (key.as_str(), shift) {
        ("left" | "h", false) => Action::Left,
        ("right" | "l", false) => Action::Right,
        ("up" | "k", false) => Action::Up,
        ("down" | "j", false) => Action::Down,
        ("left" | "h", true) => Action::CardLeft,
        ("right" | "l", true) => Action::CardRight,
        ("up" | "k", true) => Action::CardUp,
        ("down" | "j", true) => Action::CardDown,
        ("n", false) => Action::New,
        ("d", false) => Action::Done,
        ("p", false) => Action::Priority,
        ("g", false) => Action::Scope,
        ("w" | "enter", false) => Action::Start,
        ("e", false) => Action::Details,
        ("s", false) => Action::Sync,
        ("a", false) => Action::King,
        _ => return None,
    };
    Some(action)
}

/// What the engine is asked to do to a card: the status it goes to and the
/// card it goes above (`None`: the bottom of the column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The card.
    pub id: String,
    /// The column it goes to.
    pub status: TaskStatus,
    /// The card it is placed above in that column.
    pub before: Option<String>,
}

/// The tasks of the board in its columns, and the cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct Board {
    columns: Vec<Vec<Task>>,
    column: usize,
    row: usize,
    anchor: Option<String>,
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Board {
    /// An empty board, the cursor in the column of tasks to do.
    pub fn new() -> Self {
        Self {
            columns: vec![Vec::new(); COLUMNS.len()],
            column: FIRST_COLUMN,
            row: 0,
            anchor: None,
        }
    }

    /// The cards of every column, in the order of [`COLUMNS`].
    pub fn columns(&self) -> &[Vec<Task>] {
        &self.columns
    }

    /// The cursor: the column and the card in it.
    pub fn cursor(&self) -> (usize, usize) {
        (self.column, self.row)
    }

    /// The status of the cursor's column.
    pub fn status(&self) -> TaskStatus {
        COLUMNS[self.column]
    }

    /// The card under the cursor.
    pub fn selected(&self) -> Option<&Task> {
        self.columns.get(self.column)?.get(self.row)
    }

    /// How many cards the board holds.
    pub fn len(&self) -> usize {
        self.columns.iter().map(Vec::len).sum()
    }

    /// Whether the board holds no card.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Puts `tasks` on the board, each in the column of its status (a
    /// canceled one is left out) and in the order given. The cursor stays on
    /// the card it was on, wherever it is now; when that card is gone it
    /// stays at its place, brought back inside the column.
    pub fn set_tasks(&mut self, tasks: Vec<Task>) {
        let mut columns = vec![Vec::new(); COLUMNS.len()];
        for task in tasks {
            if let Some(index) = COLUMNS.iter().position(|status| *status == task.status) {
                columns[index].push(task);
            }
        }
        self.columns = columns;
        let found = self.anchor.as_deref().and_then(|anchor| {
            self.columns.iter().enumerate().find_map(|(column, cards)| {
                cards
                    .iter()
                    .position(|task| task.id == anchor)
                    .map(|row| (column, row))
            })
        });
        match found {
            Some((column, row)) => {
                self.column = column;
                self.row = row;
            }
            None => self.clamp(),
        }
        self.anchor = self.selected().map(|task| task.id.clone());
    }

    fn clamp(&mut self) {
        self.row = self
            .row
            .min(self.columns[self.column].len().saturating_sub(1));
    }

    /// Moves the cursor for one of the four cursor actions; every other
    /// action leaves it where it is.
    pub fn step(&mut self, action: Action) {
        match action {
            Action::Left => self.column = self.column.saturating_sub(1),
            Action::Right => self.column = (self.column + 1).min(COLUMNS.len() - 1),
            Action::Up => self.row = self.row.saturating_sub(1),
            Action::Down => self.row += 1,
            _ => return,
        }
        self.clamp();
        self.anchor = self.selected().map(|task| task.id.clone());
    }

    /// What the engine is asked for by an action that carries the card under
    /// the cursor, or `None` when there is nothing to carry or nowhere to
    /// carry it (the first column to the left, the top of a column).
    pub fn plan(&self, action: Action) -> Option<Plan> {
        let card = self.selected()?;
        let cards = &self.columns[self.column];
        let to = |status: TaskStatus, before: Option<&Task>| Plan {
            id: card.id.clone(),
            status,
            before: before.map(|task| task.id.clone()),
        };
        match action {
            Action::CardLeft => {
                let column = self.column.checked_sub(1)?;
                Some(to(COLUMNS[column], None))
            }
            Action::CardRight => {
                let column = self.column + 1;
                (column < COLUMNS.len()).then(|| to(COLUMNS[column], None))
            }
            Action::CardUp => {
                let above = cards.get(self.row.checked_sub(1)?)?;
                Some(to(card.status, Some(above)))
            }
            Action::CardDown => {
                cards.get(self.row + 1)?;
                // Above the card two below, which leaves it one place down;
                // the bottom when there is none.
                Some(to(card.status, cards.get(self.row + 2)))
            }
            Action::Done => (card.status != TaskStatus::Done).then(|| to(TaskStatus::Done, None)),
            _ => None,
        }
    }

    /// The column and the place of the card `id`, when it is on the board.
    pub fn find(&self, id: &str) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(column, cards)| {
            cards
                .iter()
                .position(|task| task.id == id)
                .map(|row| (column, row))
        })
    }

    /// What a card dropped in `column`, before the card at `index` of that
    /// column as it is drawn (the dragged card still in it, `index` equal to
    /// the number of cards for the end), asks of the engine. `None` when the
    /// drop leaves the card where it is, or when the card is not on the board.
    pub fn drop_plan(&self, id: &str, column: usize, index: usize) -> Option<Plan> {
        let column = column.min(COLUMNS.len() - 1);
        let (from_column, from_row) = self.find(id)?;
        let mut cards: Vec<&Task> = self.columns[column].iter().collect();
        let mut index = index.min(cards.len());
        if from_column == column {
            // Counted without the card: its own place is not one to drop at.
            cards.remove(from_row);
            if index > from_row {
                index -= 1;
            }
            if index == from_row {
                return None;
            }
        }
        Some(Plan {
            id: id.to_owned(),
            status: COLUMNS[column],
            before: cards.get(index).map(|task| task.id.clone()),
        })
    }

    /// How many open cards (everything but done) the board holds.
    pub fn open(&self) -> usize {
        self.columns[..COLUMNS.len() - 1].iter().map(Vec::len).sum()
    }

    /// Puts the cursor on a card the person pointed at (a click). A place
    /// outside the board is brought inside it.
    pub fn select(&mut self, column: usize, row: usize) {
        self.column = column.min(COLUMNS.len() - 1);
        self.row = row;
        self.clamp();
        self.anchor = self.selected().map(|task| task.id.clone());
    }

    /// Makes `id` the card the cursor follows, for a card that was just
    /// created and is not on the board yet.
    pub fn follow(&mut self, id: impl Into<String>) {
        self.anchor = Some(id.into());
    }
}

/// The text fields of a task's details, as the person typed them or as the
/// store holds them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields {
    /// The title.
    pub title: String,
    /// The description.
    pub body: String,
    /// Who it is for.
    pub assignee: String,
}

/// One line: control characters and line breaks as spaces, runs of white
/// space as one, none at either end. How the store keeps a title.
pub fn clean_line(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A text: line endings as `\n`, no control character but the tab, no blank
/// space around it. How the store keeps a description.
pub fn clean_body(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// What saving the fields changes: a patch of only what differs from `base`
/// once both are cleaned the way the store cleans them, so that a trailing
/// space is not a change. `Ok(None)` when nothing differs; `Err` with the
/// sentence to show when the title would be empty.
pub fn changes(base: &Fields, now: &Fields) -> Result<Option<TaskPatch>, &'static str> {
    let title = clean_line(&now.title);
    if title.is_empty() {
        return Err("A task needs a title.");
    }
    let body = clean_body(&now.body);
    let assignee = clean_line(&now.assignee);
    let mut patch = TaskPatch::default();
    if title != clean_line(&base.title) {
        patch.title = Some(title);
    }
    if body != clean_body(&base.body) {
        patch.body = Some(body);
    }
    if assignee != clean_line(&base.assignee) {
        patch.assignee = Some(Some(assignee).filter(|name| !name.is_empty()));
    }
    Ok((!patch.is_empty()).then_some(patch))
}

/// The labels of a task with `label` added: lower case, one line, and not
/// twice. `None` when there is nothing to add or it is there already.
pub fn with_label(labels: &[String], label: &str) -> Option<Vec<String>> {
    let label = clean_line(label).to_lowercase();
    if label.is_empty() || labels.contains(&label) {
        return None;
    }
    let mut all = labels.to_vec();
    all.push(label);
    Some(all)
}

/// The priority one step up: none, low, medium, high, urgent and round.
pub fn next_priority(priority: TaskPriority) -> TaskPriority {
    let all = TaskPriority::ALL;
    let index = all.iter().position(|p| *p == priority).unwrap_or(0);
    all[(index + 1) % all.len()]
}

/// Which board is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Which {
    /// The board of the project rooted at this folder, on this computer.
    Project(String),
    /// The board that belongs to no project.
    Global,
}

/// Which board to show, and a line that says why when it is not the one the
/// person may expect.
///
/// A project's board is only offered for a project of this computer: its
/// scope is the path of its root, and a path on another machine would be
/// taken for the same path here. `root` is the root of the project the
/// window is on, `None` when it is on none; `want_global` is the person's
/// switch.
pub fn choose(
    local: bool,
    root: Option<String>,
    want_global: bool,
) -> (Which, Option<&'static str>) {
    match (local, root, want_global) {
        (_, _, true) => (Which::Global, None),
        (true, Some(root), false) => (Which::Project(root), None),
        (false, _, false) => (
            Which::Global,
            Some("This project is on another machine; its board is kept there. Showing the global board."),
        ),
        (true, None, false) => (
            Which::Global,
            Some("No project is selected. Showing the global board."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::MemoryScope;

    fn task(id: &str, status: TaskStatus) -> Task {
        Task {
            id: id.to_owned(),
            scope: MemoryScope::project("/srv/api"),
            title: id.to_owned(),
            body: String::new(),
            status,
            priority: TaskPriority::None,
            position: 1.0,
            labels: Vec::new(),
            assignee: None,
            agent: None,
            created_at: 0,
            updated_at: 0,
            closed_at: None,
            branch: None,
        }
    }

    fn board() -> Board {
        use TaskStatus::*;
        let mut board = Board::new();
        board.set_tasks(vec![
            task("b1", Backlog),
            task("t1", Todo),
            task("t2", Todo),
            task("t3", Todo),
            task("d1", Doing),
            task("x1", Canceled),
            task("n1", Done),
        ]);
        board
    }

    fn id(board: &Board) -> &str {
        board.selected().map(|task| task.id.as_str()).unwrap_or("-")
    }

    #[test]
    fn keys_map_to_actions_and_a_modifier_other_than_shift_is_not_the_boards() {
        let table = [
            ("left", false, Action::Left),
            ("h", false, Action::Left),
            ("j", false, Action::Down),
            ("K", false, Action::Up),
            ("right", true, Action::CardRight),
            ("H", true, Action::CardLeft),
            ("down", true, Action::CardDown),
            ("n", false, Action::New),
            ("d", false, Action::Done),
            ("p", false, Action::Priority),
            ("g", false, Action::Scope),
            ("w", false, Action::Start),
            ("enter", false, Action::Start),
            ("e", false, Action::Details),
            ("s", false, Action::Sync),
        ];
        for (key, shift, action) in table {
            assert_eq!(action_of(key, shift, false), Some(action), "{key} {shift}");
        }
        assert_eq!(action_of("n", false, true), None);
        assert_eq!(action_of("x", false, false), None);
        // Shift on a key that has no shifted meaning is nothing.
        assert_eq!(action_of("n", true, false), None);
    }

    #[test]
    fn the_cards_go_to_their_columns_and_canceled_has_none() {
        let board = board();
        let sizes: Vec<usize> = board.columns().iter().map(Vec::len).collect();
        assert_eq!(sizes, [1, 3, 1, 0, 1]);
        assert_eq!(board.len(), 6);
        assert_eq!(board.status(), TaskStatus::Todo);
        assert_eq!(id(&board), "t1");
    }

    #[test]
    fn the_cursor_stays_inside_the_board() {
        let mut board = board();
        for _ in 0..9 {
            board.step(Action::Down);
        }
        assert_eq!(id(&board), "t3");
        board.step(Action::Right);
        // One card in the next column: the row comes back inside it.
        assert_eq!(id(&board), "d1");
        board.step(Action::Right);
        // An empty column holds no card, and the cursor can be in it.
        assert_eq!(board.selected(), None);
        assert_eq!(board.cursor(), (3, 0));
        for _ in 0..9 {
            board.step(Action::Right);
        }
        assert_eq!(board.cursor().0, 4);
        for _ in 0..9 {
            board.step(Action::Left);
        }
        assert_eq!(board.cursor().0, 0);
        board.step(Action::Up);
        assert_eq!(id(&board), "b1");
    }

    #[test]
    fn what_a_key_carries_is_a_plan_and_where_there_is_no_place_it_is_none() {
        let mut board = board();
        let plan = |board: &Board, action| board.plan(action);
        // Right and left go to the bottom of the next column.
        assert_eq!(
            plan(&board, Action::CardRight),
            Some(Plan {
                id: "t1".into(),
                status: TaskStatus::Doing,
                before: None
            })
        );
        assert_eq!(
            plan(&board, Action::CardLeft).map(|p| p.status),
            Some(TaskStatus::Backlog)
        );
        // The top card cannot go up; it goes down above the card two below.
        assert_eq!(plan(&board, Action::CardUp), None);
        assert_eq!(
            plan(&board, Action::CardDown),
            Some(Plan {
                id: "t1".into(),
                status: TaskStatus::Todo,
                before: Some("t3".into())
            })
        );
        board.step(Action::Down);
        assert_eq!(
            plan(&board, Action::CardUp).unwrap().before.as_deref(),
            Some("t1")
        );
        // The second to last goes to the bottom: nothing is two below.
        assert_eq!(
            plan(&board, Action::CardDown),
            Some(Plan {
                id: "t2".into(),
                status: TaskStatus::Todo,
                before: None
            })
        );
        board.step(Action::Down);
        assert_eq!(plan(&board, Action::CardDown), None);
        assert_eq!(plan(&board, Action::Done).unwrap().status, TaskStatus::Done);
        // At the edges of the board there is nowhere to carry a card.
        board.step(Action::Left);
        board.step(Action::Left);
        assert_eq!(plan(&board, Action::CardLeft), None);
        board.step(Action::Right);
        board.step(Action::Right);
        board.step(Action::Right);
        board.step(Action::Right);
        assert_eq!(id(&board), "n1");
        assert_eq!(plan(&board, Action::CardRight), None);
        assert_eq!(plan(&board, Action::Done), None);
        // Nothing under the cursor, nothing to carry.
        board.step(Action::Left);
        assert_eq!(board.selected(), None);
        assert_eq!(plan(&board, Action::CardRight), None);
    }

    #[test]
    fn a_reload_keeps_the_cursor_on_its_card_and_follows_it_to_another_column() {
        use TaskStatus::*;
        let mut board = board();
        board.step(Action::Down);
        assert_eq!(id(&board), "t2");
        // Another card arrived above it: the cursor is still on t2.
        board.set_tasks(vec![
            task("t0", Todo),
            task("t1", Todo),
            task("t2", Todo),
            task("t3", Todo),
        ]);
        assert_eq!((id(&board), board.cursor()), ("t2", (1, 2)));
        // The card was carried to doing: the cursor goes with it.
        board.set_tasks(vec![task("t0", Todo), task("t2", Doing)]);
        assert_eq!((id(&board), board.cursor()), ("t2", (2, 0)));
        // The card is gone (removed, or canceled): the cursor stays in its
        // column, inside it.
        board.set_tasks(vec![task("t0", Todo), task("e", Doing)]);
        assert_eq!(board.cursor(), (2, 0));
        board.set_tasks(Vec::new());
        assert_eq!(board.selected(), None);
        assert!(board.is_empty());
    }

    #[test]
    fn a_task_that_was_just_created_is_followed_when_it_arrives() {
        let mut board = board();
        board.follow("new");
        board.set_tasks(vec![
            task("t1", TaskStatus::Todo),
            task("new", TaskStatus::Doing),
        ]);
        assert_eq!(id(&board), "new");
    }

    #[test]
    fn a_drop_names_the_card_it_goes_above_and_none_where_nothing_changes() {
        use TaskStatus::*;
        let mut board = Board::new();
        board.set_tasks(vec![
            task("a", Todo),
            task("b", Todo),
            task("c", Todo),
            task("d", Doing),
        ]);
        let plan = |id: &str, column: usize, index: usize| board.drop_plan(id, column, index);
        let to = |id: &str, status: TaskStatus, before: Option<&str>| {
            Some(Plan {
                id: id.into(),
                status,
                before: before.map(str::to_owned),
            })
        };
        // Another column: above the card at the index, or the bottom.
        assert_eq!(plan("a", 2, 0), to("a", Doing, Some("d")));
        assert_eq!(plan("a", 2, 1), to("a", Doing, None));
        assert_eq!(plan("a", 2, 99), to("a", Doing, None));
        // Into an empty column.
        assert_eq!(plan("a", 3, 0), to("a", Review, None));
        // The same column, counted with the card still in it.
        assert_eq!(plan("a", 1, 3), to("a", Todo, None));
        assert_eq!(plan("a", 1, 2), to("a", Todo, Some("c")));
        assert_eq!(plan("c", 1, 0), to("c", Todo, Some("a")));
        assert_eq!(plan("c", 1, 1), to("c", Todo, Some("b")));
        assert_eq!(plan("b", 1, 3), to("b", Todo, None));
        // Where it already is: before itself, after itself, at the bottom.
        assert_eq!(plan("a", 1, 0), None);
        assert_eq!(plan("a", 1, 1), None);
        assert_eq!(plan("b", 1, 1), None);
        assert_eq!(plan("b", 1, 2), None);
        assert_eq!(plan("c", 1, 2), None);
        assert_eq!(plan("c", 1, 3), None);
        // A card that is not on the board.
        assert_eq!(plan("zzz", 1, 0), None);
    }

    #[test]
    fn the_board_counts_what_is_open() {
        let board = board();
        // backlog 1, todo 3, doing 1 and review 0 are open; done 1 is not.
        assert_eq!(board.open(), 5);
        assert_eq!(board.find("d1"), Some((2, 0)));
        assert_eq!(board.find("n1"), Some((4, 0)));
        assert_eq!(board.find("x1"), None);
    }

    #[test]
    fn saving_the_fields_changes_only_what_differs_once_cleaned() {
        let base = Fields {
            title: "Fix the login".into(),
            body: "Users land back.\nUse a cleared jar.".into(),
            assignee: "ana".into(),
        };
        // Nothing typed, or only blank space, or the way a terminal ends a
        // line: nothing to save.
        assert_eq!(changes(&base, &base), Ok(None));
        let spaced = Fields {
            title: "  Fix   the login ".into(),
            body: "Users land back.\r\nUse a cleared jar.\n\n".into(),
            assignee: " ana ".into(),
        };
        assert_eq!(changes(&base, &spaced), Ok(None));
        // One field changed: only that one is in the patch.
        let renamed = Fields {
            title: "Fix the SSO login".into(),
            ..base.clone()
        };
        let patch = changes(&base, &renamed).unwrap().unwrap();
        assert_eq!(patch.title.as_deref(), Some("Fix the SSO login"));
        assert_eq!((patch.body, patch.assignee), (None, None));
        // Emptied assignee takes it away; a new body is cleaned.
        let patch = changes(
            &base,
            &Fields {
                body: "  Now it has more.\r\n".into(),
                assignee: "   ".into(),
                ..base.clone()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(patch.body.as_deref(), Some("Now it has more."));
        assert_eq!(patch.assignee, Some(None));
        assert_eq!(patch.title, None);
        // A task needs its title.
        let untitled = Fields {
            title: " \n ".into(),
            ..base.clone()
        };
        assert_eq!(changes(&base, &untitled), Err("A task needs a title."));
    }

    #[test]
    fn a_label_is_added_once_and_in_lower_case() {
        let labels = vec!["bug".to_owned()];
        assert_eq!(
            with_label(&labels, "  Needs   Review "),
            Some(vec!["bug".to_owned(), "needs review".to_owned()])
        );
        assert_eq!(with_label(&labels, "BUG"), None);
        assert_eq!(with_label(&labels, "   "), None);
    }

    #[test]
    fn priority_goes_round() {
        let mut priority = TaskPriority::None;
        let mut seen = Vec::new();
        for _ in 0..5 {
            priority = next_priority(priority);
            seen.push(priority);
        }
        assert_eq!(
            seen,
            [
                TaskPriority::Low,
                TaskPriority::Medium,
                TaskPriority::High,
                TaskPriority::Urgent,
                TaskPriority::None
            ]
        );
    }

    #[test]
    fn a_projects_board_is_shown_only_for_a_project_of_this_computer() {
        let root = || Some("/srv/api".to_owned());
        assert_eq!(
            choose(true, root(), false),
            (Which::Project("/srv/api".into()), None)
        );
        assert_eq!(choose(true, root(), true), (Which::Global, None));
        let (which, note) = choose(false, root(), false);
        assert_eq!(which, Which::Global);
        assert!(note.unwrap().contains("another machine"));
        let (which, note) = choose(true, None, false);
        assert_eq!(which, Which::Global);
        assert!(note.unwrap().contains("No project"));
    }
}

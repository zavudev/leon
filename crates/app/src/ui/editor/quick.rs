//! Finding files: the two scopes of the palette that look inside a project.
//!
//! * **Quick open** (`~`, or the `QuickOpen` command): a fuzzy finder over the
//!   file list of the project in view. The list is the engine's
//!   `project_files` (what git tracks and has not ignored), kept for each
//!   `workspace::key_of(machine, root)` and asked for again every time the
//!   palette opens, so the last list shows at once and is replaced when the
//!   new one arrives. `path:line` opens the file at a line.
//! * **Project search** (`%`, or `SearchProject`): the text typed is looked for
//!   in the files of the project by the engine (in-process here, `rg`,
//!   `git grep` or `grep` on a remote machine). A new query ends the search
//!   under way (the flag the scan looks at, and the task aborted) and drops
//!   what it had found; the answer shows grouped by file.
//!
//! Both are rows of the palette (`Item::File`, `Item::Match`), so the list,
//! the cursor, the keys and the overlay are the palette's own; this module
//! holds the state, the ranking and the grouping, which are pure, and the
//! asking.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::super::palette::{parse, Item, Scope};
use super::super::shell::{Overlay, Shell};
use super::super::workspace;
use super::filetree;
use crate::fuzzy::score;
use crate::search::{Matcher, ProjectQuery, SearchHit};
use gpui_kit::{Context, Task, Window};
use leon_core::MachineId;
use tokio::task::AbortHandle;

/// How many files quick open lists.
pub const FILE_ROWS: usize = 100;

/// How many matching lines the palette lists. The search keeps up to
/// [`crate::search::MAX_HITS`]; a list of more rows than this is slow to lay
/// out and nobody reads it, so the rest is asked for with a narrower query.
pub const HIT_ROWS: usize = 300;

/// The files of the projects asked for, and the one being asked for.
#[derive(Default)]
pub struct QuickFiles {
    /// The last list of each tree key.
    cache: HashMap<String, Arc<Vec<String>>>,
    /// The key asked for since the palette opened: once is enough.
    requested: Option<String>,
    /// Where the files shown are.
    pub target: Option<(MachineId, String)>,
    /// Why the list could not be had.
    pub failed: Option<String>,
}

impl QuickFiles {
    /// Forgets what was asked since the palette opened: the next scope that
    /// needs the list asks again.
    pub fn reopened(&mut self) {
        self.requested = None;
        self.failed = None;
    }

    /// The list for the target, if there is one yet.
    pub fn list(&self) -> Option<&Arc<Vec<String>>> {
        let (machine, root) = self.target.as_ref()?;
        self.cache.get(&workspace::key_of(machine.as_str(), root))
    }
}

/// The text search in view.
pub struct TextSearch {
    /// Whether the query is a regular expression.
    pub regex: bool,
    /// Whether upper and lower case differ.
    pub case_sensitive: bool,
    /// Where the search looks.
    pub target: Option<(MachineId, String)>,
    /// What it found for [`TextSearch::searched`].
    pub hits: Vec<SearchHit>,
    /// Whether there was more than the search keeps.
    pub truncated: bool,
    /// Whether the search is under way.
    pub running: bool,
    /// Why it failed: a regular expression that is none, a machine that did
    /// not answer.
    pub error: Option<String>,
    /// The text the hits are for.
    pub searched: String,
    /// How to draw the match in a line.
    pub matcher: Option<Matcher>,
    /// Asks the scan under way to stop.
    cancel: Arc<AtomicBool>,
    generation: u64,
    task: Option<Task<()>>,
    abort: Option<AbortHandle>,
}

impl Default for TextSearch {
    fn default() -> Self {
        Self {
            regex: false,
            case_sensitive: false,
            target: None,
            hits: Vec::new(),
            truncated: false,
            running: false,
            error: None,
            searched: String::new(),
            matcher: None,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 0,
            task: None,
            abort: None,
        }
    }
}

impl TextSearch {
    /// Ends the search under way and forgets what it found.
    pub fn reset(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(abort) = self.abort.take() {
            abort.abort();
        }
        self.task = None;
        self.generation += 1;
        self.hits.clear();
        self.truncated = false;
        self.running = false;
        self.error = None;
        self.searched.clear();
        self.matcher = None;
    }
}

// ----- pure parts ----------------------------------------------------------------

/// A typed `path:line` (or `path:line:column`) as the path and the line; the
/// text itself when it does not end like that.
pub fn split_line(typed: &str) -> (&str, Option<u32>) {
    let numeric = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let Some((rest, last)) = typed.rsplit_once(':') else {
        return (typed, None);
    };
    if !numeric(last) {
        return (typed, None);
    }
    // `path:line:column`: the column is dropped.
    if let Some((path, line)) = rest.rsplit_once(':') {
        if numeric(line) {
            return (path, line.parse().ok().filter(|line| *line > 0));
        }
    }
    (rest, last.parse().ok().filter(|line| *line > 0))
}

/// How well `query` finds `path`: the file's own name counts for more than the
/// folders in front of it, and a query with a slash is matched on the whole
/// path.
pub fn file_score(query: &str, path: &str) -> Option<u32> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if query.contains('/') {
        return score(query, path);
    }
    let by_name = score(query, name).map(|found| found + 1000);
    let by_path = score(query, path);
    by_name.into_iter().chain(by_path).max()
}

/// The files that answer `query`, best first, at most [`FILE_ROWS`]; with
/// nothing typed, the first ones of the list. A `:line` after the name goes
/// to every row.
pub fn file_items(query: &str, files: &[String]) -> Vec<Item> {
    let (query, line) = split_line(query.trim());
    let query = query.trim();
    if query.is_empty() {
        return files
            .iter()
            .take(FILE_ROWS)
            .map(|path| Item::File(path.clone(), line))
            .collect();
    }
    let mut scored: Vec<(u32, &String)> = files
        .iter()
        .filter_map(|path| Some((file_score(query, path)?, path)))
        .collect();
    scored.sort_by_key(|(found, _)| std::cmp::Reverse(*found));
    scored
        .into_iter()
        .take(FILE_ROWS)
        .map(|(_, path)| Item::File(path.clone(), line))
        .collect()
}

/// The rows of a text search: each file's name as a heading, then its lines.
/// At most [`HIT_ROWS`] lines; `true` when there were more.
pub fn hit_items(hits: &[SearchHit]) -> (Vec<Item>, bool) {
    let mut items = Vec::new();
    let mut last: Option<&str> = None;
    for hit in hits.iter().take(HIT_ROWS) {
        if last != Some(hit.path.as_str()) {
            items.push(Item::FileHeading(hit.path.clone()));
            last = Some(hit.path.as_str());
        }
        items.push(Item::Match(Box::new(hit.clone())));
    }
    (items, hits.len() > HIT_ROWS)
}

// ----- asking --------------------------------------------------------------------

impl Shell {
    /// What the scope the palette is in needs: the list of files for quick
    /// open, the search for the text. Called when the palette opens and when
    /// what is typed changes.
    pub(in crate::ui) fn palette_scope_changed(&mut self, cx: &mut Context<Self>) {
        let typed = self.palette.input.read(cx).value().to_string();
        let (scope, query) = parse(&typed);
        if scope != Scope::Text {
            self.palette.text.reset();
        }
        match scope {
            Scope::Files => self.load_quick_files(cx),
            // The places are listed first; the files are asked for only
            // once something is typed, and never while a command asks a
            // question.
            Scope::All if !query.is_empty() && self.palette.flow.is_none() => {
                self.load_quick_files(cx)
            }
            Scope::Text => self.start_text_search(query.to_owned(), cx),
            _ => {}
        }
    }

    /// Ends the text search under way: the palette closed, or the query
    /// changed.
    pub(in crate::ui) fn cancel_text_search(&mut self) {
        self.palette.text.reset();
    }

    /// Asks for the files of the project in view, once for each time the
    /// palette opens.
    fn load_quick_files(&mut self, cx: &mut Context<Self>) {
        let Some((machine, root)) = self.files_target() else {
            self.palette.files.target = None;
            return;
        };
        let key = workspace::key_of(machine.as_str(), &root);
        self.palette.files.target = Some((machine.clone(), root.clone()));
        if self.palette.files.requested.as_ref() == Some(&key) {
            return;
        }
        self.palette.files.requested = Some(key.clone());
        self.palette.files.failed = None;
        let listing = self.engine.project_files(machine, root);
        cx.spawn(async move |this, cx| {
            let answer = listing.await;
            this.update(cx, |this, cx| {
                match answer {
                    Ok(Ok(list)) => {
                        this.palette.files.cache.insert(key, Arc::new(list));
                    }
                    Ok(Err(error)) => this.palette.files.failed = Some(error.to_string()),
                    Err(_) => return,
                }
                if this.overlay == Overlay::Palette {
                    this.fill_palette(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Flips the regular expression switch of the search and runs it again.
    pub(in crate::ui) fn toggle_search_regex(&mut self, cx: &mut Context<Self>) {
        self.palette.text.regex = !self.palette.text.regex;
        self.palette_scope_changed(cx);
        self.fill_palette(cx);
    }

    /// Flips the case switch of the search and runs it again.
    pub(in crate::ui) fn toggle_search_case(&mut self, cx: &mut Context<Self>) {
        self.palette.text.case_sensitive = !self.palette.text.case_sensitive;
        self.palette_scope_changed(cx);
        self.fill_palette(cx);
    }

    /// Searches the project in view for `text`, ending the search before.
    fn start_text_search(&mut self, text: String, cx: &mut Context<Self>) {
        let (regex, case_sensitive) = (self.palette.text.regex, self.palette.text.case_sensitive);
        self.palette.text.reset();
        let target = self.files_target();
        self.palette.text.target = target.clone();
        if text.is_empty() {
            return;
        }
        let Some((machine, root)) = target else {
            self.palette.text.error = Some("Open a project first.".to_owned());
            return;
        };
        let query = ProjectQuery {
            text: text.clone(),
            regex,
            case_sensitive,
        };
        let search = &mut self.palette.text;
        search.running = true;
        search.searched = text;
        search.matcher = Matcher::new(&query).ok();
        search.cancel = Arc::new(AtomicBool::new(false));
        let (generation, cancel) = (search.generation, search.cancel.clone());
        let (engine, debounce) = (self.engine.clone(), self.options.search_debounce);
        search.task = Some(cx.spawn(async move |this, cx| {
            if !debounce.is_zero() {
                cx.background_executor().timer(debounce).await;
            }
            let job = engine.search_project(machine, root, query, cancel);
            let abort = job.abort_handle();
            let current = this
                .update(cx, |this, _| {
                    let current = this.palette.text.generation == generation;
                    if current {
                        this.palette.text.abort = Some(abort.clone());
                    }
                    current
                })
                .unwrap_or(false);
            if !current {
                abort.abort();
                return;
            }
            let answer = job.await;
            this.update(cx, |this, cx| {
                let search = &mut this.palette.text;
                if search.generation != generation || this.overlay != Overlay::Palette {
                    return;
                }
                search.running = false;
                search.abort = None;
                match answer {
                    Ok(Ok(found)) => {
                        search.hits = found.hits;
                        search.truncated = found.truncated;
                    }
                    Ok(Err(error)) => search.error = Some(error.to_string()),
                    // Aborted: a newer query took over.
                    Err(_) => return,
                }
                this.fill_palette(cx);
            })
            .ok();
        }));
    }

    /// Opens a file found by quick open or by the search, at `line`.
    pub(in crate::ui) fn open_found(
        &mut self,
        target: Option<(MachineId, String)>,
        path: &str,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((machine, root)) = target else {
            return;
        };
        self.open_file(machine, filetree::absolute(&root, path), line, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(items: Vec<Item>) -> Vec<String> {
        items
            .into_iter()
            .map(|item| match item {
                Item::File(path, _) => path,
                other => panic!("not a file: {other:?}"),
            })
            .collect()
    }

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_line_after_the_name_is_split_off() {
        assert_eq!(split_line("src/main.rs"), ("src/main.rs", None));
        assert_eq!(split_line("src/main.rs:42"), ("src/main.rs", Some(42)));
        assert_eq!(split_line("src/main.rs:42:7"), ("src/main.rs", Some(42)));
        assert_eq!(split_line("main:"), ("main:", None));
        assert_eq!(split_line("a:b"), ("a:b", None));
        assert_eq!(split_line(":12"), ("", Some(12)));
        assert_eq!(split_line("a.rs:0"), ("a.rs", None));
        assert_eq!(split_line("a.rs:x:3"), ("a.rs:x", Some(3)));
    }

    #[test]
    fn the_name_of_a_file_counts_for_more_than_its_folders() {
        let files = list(&[
            "docs/ARCHITECTURE.md",
            "crates/app/src/ui/main_pane.rs",
            "crates/app/src/main.rs",
            "src/main_menu/readme.txt",
        ]);
        assert_eq!(
            paths(file_items("main", &files)),
            [
                "crates/app/src/main.rs",
                "crates/app/src/ui/main_pane.rs",
                "src/main_menu/readme.txt"
            ]
        );
    }

    #[test]
    fn a_slash_in_the_query_matches_the_whole_path() {
        let files = list(&["src/ui/tree.rs", "src/tree.rs", "tree/mod.rs"]);
        assert_eq!(
            paths(file_items("ui/tree", &files)),
            ["src/ui/tree.rs"],
            "folders are part of what is typed"
        );
        assert_eq!(
            paths(file_items("src/tree.rs", &files)),
            ["src/tree.rs", "src/ui/tree.rs"],
            "the whole path first, then the one that has its letters"
        );
    }

    #[test]
    fn letters_in_order_find_a_file_and_nothing_else_does() {
        let files = list(&["src/engine.rs", "src/settings.rs", "README.md"]);
        assert_eq!(
            paths(file_items("eng", &files)),
            ["src/engine.rs", "src/settings.rs"],
            "the name that starts with it first"
        );
        assert_eq!(paths(file_items("sts", &files)), ["src/settings.rs"]);
        assert!(file_items("zzz", &files).is_empty());
    }

    #[test]
    fn the_line_goes_to_every_row_and_nothing_typed_lists_the_first_files() {
        let files = list(&["a.rs", "b.rs"]);
        let rows = file_items("b:12", &files);
        assert!(matches!(
            rows.as_slice(),
            [Item::File(path, Some(12))] if path == "b.rs"
        ));
        assert_eq!(paths(file_items("", &files)), ["a.rs", "b.rs"]);
        let many: Vec<String> = (0..FILE_ROWS + 20).map(|n| format!("f{n}.txt")).collect();
        assert_eq!(file_items("", &many).len(), FILE_ROWS);
        assert_eq!(file_items("f", &many).len(), FILE_ROWS);
    }

    fn hit(path: &str, line: u32) -> SearchHit {
        SearchHit {
            path: path.into(),
            line,
            col: None,
            text: "x".into(),
        }
    }

    #[test]
    fn hits_are_grouped_under_the_file_they_are_in() {
        let (items, more) = hit_items(&[hit("a.rs", 1), hit("a.rs", 9), hit("b.rs", 2)]);
        assert!(!more);
        let shape: Vec<String> = items
            .iter()
            .map(|item| match item {
                Item::FileHeading(path) => format!("# {path}"),
                Item::Match(hit) => format!("{}:{}", hit.path, hit.line),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(shape, ["# a.rs", "a.rs:1", "a.rs:9", "# b.rs", "b.rs:2"]);
    }

    #[test]
    fn only_so_many_lines_are_listed_and_the_rest_is_counted() {
        let hits: Vec<SearchHit> = (0..HIT_ROWS as u32 + 5)
            .map(|n| hit("a.rs", n + 1))
            .collect();
        let (items, more) = hit_items(&hits);
        assert!(more);
        assert_eq!(items.len(), HIT_ROWS + 1);
        assert!(!hit_items(&hits[..HIT_ROWS]).1);
    }

    #[test]
    fn a_new_query_raises_the_flag_of_the_scan_under_way_and_forgets_its_lines() {
        let mut search = TextSearch::default();
        let flag = search.cancel.clone();
        search.hits.push(hit("a.rs", 1));
        search.running = true;
        search.error = Some("x".into());
        search.reset();
        assert!(flag.load(Ordering::Relaxed), "the scan is told to stop");
        assert!(search.hits.is_empty() && !search.running && search.error.is_none());
        assert_eq!(search.generation, 1, "an answer of the old one is stale");
    }
}

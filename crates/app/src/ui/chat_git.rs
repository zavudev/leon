//! Git from a conversation: the branch it works on, in its foot, and the
//! steps that take its changes away without leaving it.
//!
//! The label says the branch of the conversation's worktree and how far it
//! is from its upstream. The GIT button above it opens a small cell with a
//! field for the commit message and the steps: commit, commit and push,
//! commit, push and open a pull request, and push. They are the engine's own
//! (`Engine::ship`, which the changes tab uses as well), so the commands
//! that run, their order and the way a failure is shown are the same; what is
//! different is only where the buttons are. When a pull request is opened the
//! engine reads it again at once, and the chip beside the label shows it.
//!
//! What a button allows is a pure function with tests at the end of this file.

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, Context, Entity, SharedString, Window};
use leon_core::Divergence;
use leon_remote::{ChangedFile, PullRequestDraft, Step};

use super::changes::{outcome_of, pull_request_from_message, Outcome};
use super::chat::Chat;
use super::chat_view::{button, Look};
use super::live::LiveId;
use super::shell::Shell;
use super::widgets::{led, mono};
use crate::engine::{CommitRequest, ShipReport, ShipRequest};
use crate::theme::{fonts, metrics, px, Palette};

/// A step a button of the cell asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// `git add -A` and `git commit`.
    Commit,
    /// The commit, and `git push`.
    CommitPush,
    /// The commit (when files changed), `git push` and `gh pr create`.
    Ship,
    /// `git push`.
    Push,
    /// `git pull --ff-only`.
    Pull,
}

impl Action {
    /// Whether it commits, given how many files changed: the whole chain
    /// leaves a branch that is committed already as it is.
    fn commits(self, changed: usize) -> bool {
        match self {
            Self::Commit | Self::CommitPush => true,
            Self::Ship => changed > 0,
            Self::Push | Self::Pull => false,
        }
    }

    fn pushes(self) -> bool {
        !matches!(self, Self::Commit | Self::Pull)
    }

    fn pulls(self) -> bool {
        self == Self::Pull
    }

    fn opens_pull_request(self) -> bool {
        self == Self::Ship
    }

    /// What is being done, in a word, while it is.
    fn working(self) -> &'static str {
        match self {
            Self::Commit => "Committing\u{2026}",
            Self::CommitPush => "Committing and pushing\u{2026}",
            Self::Ship => "Committing, pushing and opening the pull request\u{2026}",
            Self::Push => "Pushing\u{2026}",
            Self::Pull => "Pulling\u{2026}",
        }
    }
}

/// What the cell knows of the worktree, to say what its buttons allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// The conversation's folder is a worktree the store knows: a git one.
    pub worktree: bool,
    /// How many files changed.
    pub changed: usize,
    /// Whether the head is on a branch.
    pub branch: bool,
    /// A step is being taken.
    pub busy: bool,
}

/// What each button allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allowed {
    /// There is something to commit.
    pub commit: bool,
    /// There is something to commit, and a branch to push it on.
    pub commit_push: bool,
    /// There is a branch to push and open a pull request from.
    pub ship: bool,
    /// There is a branch to push.
    pub push: bool,
    /// There is a branch to bring up to its upstream.
    pub pull: bool,
}

impl Allowed {
    fn of(self, action: Action) -> bool {
        match action {
            Action::Commit => self.commit,
            Action::CommitPush => self.commit_push,
            Action::Ship => self.ship,
            Action::Push => self.push,
            Action::Pull => self.pull,
        }
    }
}

/// What the buttons allow. Nothing is offered outside a git worktree or while
/// a step runs; the commits need files that changed; a push needs a branch.
pub fn allowed(facts: Facts) -> Allowed {
    let here = facts.worktree && !facts.busy;
    let commit = here && facts.changed > 0;
    Allowed {
        commit,
        commit_push: commit && facts.branch,
        ship: here && facts.branch,
        push: here && facts.branch,
        pull: here && facts.branch,
    }
}

/// The branch and its distance from the upstream, for the foot:
/// `topic  ↑2 ↓1`.
pub fn branch_words(branch: &str, divergence: Option<Divergence>) -> String {
    let mut words = branch.to_owned();
    if let Some(divergence) = divergence.filter(|divergence| !divergence.is_level()) {
        if divergence.ahead > 0 {
            words.push_str(&format!("  \u{2191}{}", divergence.ahead));
        }
        if divergence.behind > 0 {
            words.push_str(&format!(" \u{2193}{}", divergence.behind));
        }
    }
    words
}

/// The cell of a conversation that takes its changes away.
pub struct GitCell {
    /// The cell is open above the field.
    pub open: bool,
    /// The commit message.
    pub message: Entity<TextareaState>,
    /// The changed files; `None` until git answered.
    pub files: Option<Vec<ChangedFile>>,
    /// The branch checked out, as git said it last.
    pub branch: Option<String>,
    /// What is being done, in a word, while it is.
    pub working: Option<&'static str>,
    /// What the last step came to.
    pub outcome: Option<Outcome>,
    /// Counts readings, so an answer that is no longer the latest is dropped.
    reads: u64,
}

impl GitCell {
    /// A closed cell with an empty message.
    pub fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        Self {
            open: false,
            message: cx.new(|cx| TextareaState::new(window, cx).placeholder("Commit message")),
            files: None,
            branch: None,
            working: None,
            outcome: None,
            reads: 0,
        }
    }

    /// Whether the message field has the keyboard.
    pub fn typing(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        use gpui_kit::Focusable as _;
        self.open && self.message.focus_handle(cx).is_focused(window)
    }
}

impl Shell {
    /// What the cell knows, as the buttons judge it.
    fn chat_git_facts(&self, chat: &Chat) -> Facts {
        let worktree = self.snapshot.worktree_at(&chat.machine, &chat.cwd);
        let branch = chat
            .git
            .branch
            .as_ref()
            .or_else(|| worktree.and_then(|worktree| worktree.branch.as_ref()));
        Facts {
            worktree: worktree.is_some(),
            changed: chat.git.files.as_ref().map_or(0, Vec::len),
            branch: branch.is_some(),
            busy: chat.git.working.is_some(),
        }
    }

    /// Opens the cell, or closes it; opening reads the changed files.
    pub(super) fn chat_git_toggle(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        chat.git.open = !chat.git.open;
        let open = chat.git.open;
        cx.notify();
        if open {
            self.chat_git_read(id, cx);
        }
    }

    /// Closes the cell.
    pub(super) fn chat_git_close(&mut self, id: LiveId, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.get_mut(&id) {
            if std::mem::take(&mut chat.git.open) {
                cx.notify();
            }
        }
    }

    /// Reads the changed files and the branch again.
    fn chat_git_read(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if self
            .snapshot
            .worktree_at(&chat.machine, &chat.cwd)
            .is_none()
        {
            return;
        }
        chat.git.reads += 1;
        let read = chat.git.reads;
        let reading = self
            .engine
            .read_changes(chat.machine.clone(), chat.cwd.clone());
        cx.spawn(async move |this, cx| {
            let answer = reading.await;
            this.update(cx, |this, cx| {
                let Some(chat) = this.chats.get_mut(&id) else {
                    return;
                };
                if chat.git.reads != read {
                    return;
                }
                match answer {
                    Ok(Ok(changes)) => {
                        chat.git.branch = changes.branch;
                        chat.git.files = Some(changes.files);
                    }
                    // What a step came to is not replaced by a reading that
                    // failed after it.
                    Ok(Err(error)) => {
                        chat.git
                            .outcome
                            .get_or_insert_with(|| failed("The changes could not be read", error));
                    }
                    Err(error) => {
                        chat.git
                            .outcome
                            .get_or_insert_with(|| failed("The changes could not be read", error));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Takes the steps of a button, the way the changes tab does: through the
    /// engine's `ship`, which stops at the first that fails.
    pub(super) fn chat_git_run(
        &mut self,
        id: LiveId,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let Some(project) = self
            .snapshot
            .worktree_at(&chat.machine, &chat.cwd)
            .map(|worktree| worktree.project_id.clone())
        else {
            return;
        };
        if !allowed(self.chat_git_facts(chat)).of(action) {
            return;
        }
        let changed = chat.git.files.as_ref().map_or(0, Vec::len);
        let message = chat.git.message.read(cx).value().trim().to_owned();
        let commits = action.commits(changed);
        if commits && message.is_empty() {
            use gpui_kit::Focusable as _;
            let field = chat.git.message.clone();
            window.focus(&field.focus_handle(cx), cx);
            if let Some(chat) = self.chats.get_mut(&id) {
                chat.git.outcome = Some(refusal("Write a commit message first."));
            }
            cx.notify();
            return;
        }
        let (machine, cwd) = (chat.machine.clone(), chat.cwd.clone());
        let request = ShipRequest {
            machine: machine.clone(),
            project,
            path: cwd.clone(),
            commit: commits.then(|| CommitRequest {
                message: message.clone(),
                files: None,
            }),
            pull: action.pulls(),
            push: action.pushes(),
            pull_request: None,
        };
        // The pull request goes into the branch the repository says, and
        // takes its words from the message or, with nothing to commit, from
        // the commits there are.
        let start = action
            .opens_pull_request()
            .then(|| self.engine.pull_request_start(machine, cwd));
        if let Some(chat) = self.chats.get_mut(&id) {
            chat.git.working = Some(action.working());
            chat.git.outcome = None;
        }
        let engine = self.engine.clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut request = request;
            let mut refused = None;
            if let Some(start) = start {
                match start.await {
                    Ok(Ok(start)) => {
                        let (title, body) = if message.is_empty() {
                            (start.title, start.body)
                        } else {
                            pull_request_from_message(&message)
                        };
                        if title.is_empty() || start.base.is_empty() {
                            refused = Some(
                                "There is nothing to title the pull request with: write a commit message."
                                    .to_owned(),
                            );
                        } else {
                            request.pull_request = Some(PullRequestDraft {
                                title,
                                body,
                                base: start.base,
                                draft: false,
                            });
                        }
                    }
                    Ok(Err(error)) => refused = Some(error.to_string()),
                    Err(error) => refused = Some(error.to_string()),
                }
            }
            let report = match refused {
                Some(why) => ShipReport {
                    failed: Some((Step::PullRequest, why)),
                    ..ShipReport::default()
                },
                None => engine.ship(request).await.unwrap_or_else(|error| ShipReport {
                    failed: Some((Step::Commit, error.to_string())),
                    ..ShipReport::default()
                }),
            };
            this.update_in(cx, |this, window, cx| {
                this.chat_git_done(id, report, window, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The steps ended: what they came to is said in the cell, a message that
    /// was used is cleared, and the files are read again.
    fn chat_git_done(
        &mut self,
        id: LiveId,
        report: ShipReport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        chat.git.working = None;
        chat.git.outcome = Some(outcome_of(&report));
        if report.done.iter().any(|(step, _)| *step == Step::Commit) {
            chat.git
                .message
                .update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.chat_git_read(id, cx);
        cx.notify();
    }

    /// The branch of the conversation's worktree and its distance from the
    /// upstream; `None` when the store knows of no branch for the folder.
    pub(super) fn chat_branch_label(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
    ) -> Option<AnyElement> {
        let worktree = self.snapshot.worktree_at(&chat.machine, &chat.cwd)?;
        let branch = worktree.branch.as_deref()?;
        let divergence = self
            .snapshot
            .status(&worktree.id)
            .and_then(|status| status.divergence);
        let words = branch_words(branch, divergence);
        Some(
            div()
                .debug_selector(move || format!("chat-branch-{}", id.0))
                .flex_none()
                .max_w(px(200.))
                .truncate()
                .font_family(fonts::mono())
                .font_features(fonts::mono_features())
                .text_size(metrics::TEXT_LABEL())
                .text_color(colours.text_muted)
                .child(words)
                .into_any_element(),
        )
    }

    /// The button of the foot that opens the cell.
    pub(super) fn chat_git_button(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = format!("chat-git-{}", id.0);
        let hover = colours.surface_2;
        let here = self
            .snapshot
            .worktree_at(&chat.machine, &chat.cwd)
            .is_some();
        let open = chat.git.open;
        let button = div()
            .id(SharedString::from(name.clone()))
            .debug_selector(move || name.clone())
            .flex_none()
            .h(px(20.))
            .px(px(6.))
            .flex()
            .items_center()
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(if open { colours.signal } else { colours.border })
            .child(mono("GIT").text_color(if here {
                colours.text_muted
            } else {
                colours.text_faint
            }));
        if here {
            button
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.chat_git_toggle(id, cx);
                }))
                .into_any_element()
        } else {
            button.into_any_element()
        }
    }

    /// The cell above the field: the message, the steps and what the last
    /// one came to.
    pub(super) fn chat_git_cell(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !chat.git.open {
            return None;
        }
        let ok = allowed(self.chat_git_facts(chat));
        let changed = chat.git.files.as_ref().map(Vec::len);
        let state = match (&chat.git.working, changed) {
            (Some(working), _) => (*working).to_owned(),
            (None, None) => "Reading the changes\u{2026}".to_owned(),
            (None, Some(0)) => "No changes to commit.".to_owned(),
            (None, Some(1)) => "1 file changed.".to_owned(),
            (None, Some(n)) => format!("{n} files changed."),
        };
        let step = |name: &'static str,
                    label: &'static str,
                    action: Action,
                    look: Look,
                    cx: &mut Context<Self>| {
            let enabled = ok.of(action);
            let base = button(name, id, label, look, colours);
            if enabled {
                base.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.chat_git_run(id, action, window, cx);
                }))
            } else {
                // Not allowed: faint, and a click does nothing.
                base.opacity(0.4).cursor_default()
            }
        };
        Some(
            div()
                .debug_selector(move || format!("chat-git-cell-{}", id.0))
                .w_full()
                .min_w_0()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.border)
                .bg(colours.surface)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(led(colours.signal))
                        .child(mono("[ GIT ]").text_color(colours.signal))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(colours.text_muted)
                                .child(state),
                        ),
                )
                .child(
                    div()
                        .rounded(metrics::RADIUS())
                        .border_1()
                        .border_color(colours.border)
                        .px(px(4.))
                        .child(
                            Textarea::new(&chat.git.message)
                                .appearance(false)
                                .bordered(false)
                                .h(px(56.)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(step(
                            "chat-git-commit",
                            "COMMIT",
                            Action::Commit,
                            Look::Outline,
                            cx,
                        ))
                        .child(step(
                            "chat-git-commit-push",
                            "COMMIT & PUSH",
                            Action::CommitPush,
                            Look::Outline,
                            cx,
                        ))
                        .child(step(
                            "chat-git-ship",
                            "COMMIT, PUSH & OPEN PR",
                            Action::Ship,
                            Look::Primary,
                            cx,
                        ))
                        .child(step(
                            "chat-git-push",
                            "PUSH",
                            Action::Push,
                            Look::Outline,
                            cx,
                        ))
                        .child(step(
                            "chat-git-pull",
                            "PULL",
                            Action::Pull,
                            Look::Outline,
                            cx,
                        ))
                        .child(div().flex_1())
                        .child(
                            button("chat-git-close", id, "CLOSE", Look::Quiet, colours).on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.chat_git_close(id, cx);
                                }),
                            ),
                        ),
                )
                .children(
                    chat.git
                        .outcome
                        .as_ref()
                        .map(|outcome| self.render_outcome(outcome, colours, cx)),
                )
                .into_any_element(),
        )
    }
}

/// An outcome that says a step was not taken, and why.
fn refusal(why: &str) -> Outcome {
    Outcome {
        ok: false,
        title: why.to_owned(),
        text: String::new(),
        url: None,
    }
}

/// An outcome for a reading that failed.
fn failed(title: &str, error: impl std::fmt::Display) -> Outcome {
    Outcome {
        ok: false,
        title: title.to_owned(),
        text: error.to_string(),
        url: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(changed: usize, branch: bool) -> Facts {
        Facts {
            worktree: true,
            changed,
            branch,
            busy: false,
        }
    }

    #[test]
    fn nothing_is_offered_outside_a_worktree_or_while_a_step_runs() {
        let none = |allowed: Allowed| {
            !(allowed.commit || allowed.commit_push || allowed.ship || allowed.push || allowed.pull)
        };
        assert!(none(allowed(Facts {
            worktree: false,
            ..facts(3, true)
        })));
        assert!(none(allowed(Facts {
            busy: true,
            ..facts(3, true)
        })));
    }

    #[test]
    fn a_commit_needs_files_that_changed_and_a_push_needs_a_branch() {
        let clean = allowed(facts(0, true));
        assert!(!clean.commit && !clean.commit_push);
        assert!(clean.push && clean.ship, "a committed branch can still go");
        let changed = allowed(facts(2, true));
        assert!(changed.commit && changed.commit_push && changed.ship && changed.push);
        let detached = allowed(facts(2, false));
        assert!(detached.commit);
        assert!(!detached.commit_push && !detached.ship && !detached.push);
    }

    #[test]
    fn the_whole_chain_commits_only_when_files_changed() {
        assert!(Action::Ship.commits(1));
        assert!(!Action::Ship.commits(0));
        assert!(Action::Commit.commits(0));
        assert!(!Action::Push.commits(5));
        assert!(Action::Ship.pushes() && Action::Ship.opens_pull_request());
        assert!(!Action::Commit.pushes());
        assert!(!Action::CommitPush.opens_pull_request());
    }

    #[test]
    fn the_branch_says_how_far_it_is_from_its_upstream_only_when_it_is() {
        let of = |ahead, behind| Some(Divergence { ahead, behind });
        assert_eq!(branch_words("topic", None), "topic");
        assert_eq!(branch_words("topic", of(0, 0)), "topic");
        assert_eq!(branch_words("topic", of(2, 0)), "topic  \u{2191}2");
        assert_eq!(branch_words("topic", of(0, 1)), "topic \u{2193}1");
        assert_eq!(
            branch_words("topic", of(2, 1)),
            "topic  \u{2191}2 \u{2193}1"
        );
    }
}

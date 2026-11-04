//! `!` in a conversation's field: a command the person runs themselves, as
//! in Claude Code, without leaving the conversation.
//!
//! * **Asked first.** Enter on `!<command>` does not send it to the agent: a
//!   card above the field shows the command whole, and the folder and machine
//!   it runs in, and nothing runs until the person presses Enter again or
//!   **Run**. Escape, **Cancel** or a change to the text leaves it in the field.
//! * **In the conversation.** The command is an item of its own
//!   ([`Item::Shell`]), drawn like what the person says with the shell's `!`,
//!   and what it printed under it once it ended. It runs once, to the end, on
//!   the conversation's machine and in its folder ([`Engine::run_typed`]),
//!   beside whatever the agent is doing; **Stop** ends it.
//! * **Sent to the agent.** The agent did not see it run, so once it ended
//!   what it printed is sent to the agent, which reads it and goes on: at
//!   once when the agent waits, when its turn ends when it works (a message
//!   the person queued meanwhile carries it in front of their words). It is
//!   written in the tags Claude Code itself writes for its `!` commands
//!   (`<bash-input>`, `<bash-stdout>`, `<bash-stderr>`); the conversation
//!   shows the command's item, not the tags, and read back from the history
//!   they are a note, not something the person said
//!   (`leon_conversation::harness`).
//!
//! [`Item::Shell`]: crate::chat::model::Item::Shell
//! [`Engine::run_typed`]: crate::engine::Engine::run_typed

use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, Context, Task, Window};
use leon_remote::{Output, RunError};

use super::chat::Chat;
use super::chat_view::{button, Look};
use super::live::LiveId;
use super::shell::Shell;
use super::widgets::{led, mono};
use crate::chat::model::ShellResult;
use crate::engine::StatusKind;
use crate::theme::{fonts, metrics, px, Palette};

/// The most of each of a command's output and errors the agent is told: the
/// beginning and the end, what is between left out.
const TOLD_MOST: usize = 20_000;

/// The lines of a command's output shown under it: its last ones.
const SHOWN_LINES: usize = 14;

/// What a conversation keeps of the commands the person runs in it.
#[derive(Default)]
pub struct ShellUi {
    /// The command on the card, waiting for the person to say run it.
    pub ask: Option<String>,
    /// The command running now.
    running: Option<Running>,
    /// What the agent is sent and has not been yet: one block per command
    /// that ended since.
    pub told: Vec<String>,
}

impl ShellUi {
    /// The command running now, as the person wrote it.
    pub fn running(&self) -> Option<&str> {
        self.running
            .as_ref()
            .map(|running| running.command.as_str())
    }

    /// What goes in front of the next message: every block, oldest first.
    pub fn told_text(&self) -> Option<String> {
        (!self.told.is_empty()).then(|| self.told.join("\n\n"))
    }
}

/// A command that runs. Dropped (the conversation closed, or went back past
/// it), it ends the command too: the engine's task would go on without it.
struct Running {
    /// Its item in the conversation.
    item: usize,
    /// It, as the person wrote it.
    command: String,
    /// Ends it.
    abort: tokio::task::AbortHandle,
    /// Waits for its end.
    _waiting: Task<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        // Nothing happens to a command that ended already.
        self.abort.abort();
    }
}

/// The command a field's text asks to run: what follows a `!` at its start.
/// `None` for text that is not a command, and for a `!` alone.
pub fn command_of(text: &str) -> Option<&str> {
    let command = text.trim().strip_prefix('!')?.trim();
    (!command.is_empty()).then_some(command)
}

/// How a command's run came to an end.
#[derive(Debug)]
pub enum Ended {
    /// It ran to its end.
    Ran(Output),
    /// It was ended before its end, by the person or by the time limit: why.
    Stopped(String),
    /// It could not be started: why.
    NotRun(String),
}

impl Ended {
    /// How the engine's job ended, `cancelled` when the person stopped it.
    pub fn of(job: Result<Result<Output, RunError>, tokio::task::JoinError>) -> Self {
        match job {
            Ok(Ok(output)) => Self::Ran(output),
            Ok(Err(RunError::TimedOut { limit, .. })) => Self::Stopped(format!(
                "stopped after {} minutes, the most a command may run",
                limit.as_secs().div_ceil(60)
            )),
            Ok(Err(error)) => Self::NotRun(error.to_string()),
            Err(error) if error.is_cancelled() => Self::Stopped("stopped by the person".to_owned()),
            Err(error) => Self::NotRun(error.to_string()),
        }
    }
}

/// How a command ended, in the conversation: its output then its errors, or
/// why it ended early or did not run.
pub fn result_of(ended: &Ended) -> ShellResult {
    match ended {
        Ended::Ran(output) => {
            let printed = [output.stdout.trim_end(), output.stderr.trim_end()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            ShellResult {
                exit: output.status,
                stopped: false,
                output: printed,
            }
        }
        Ended::Stopped(why) => ShellResult {
            exit: None,
            stopped: true,
            output: format!("{why}: it may have done part of its work."),
        },
        Ended::NotRun(why) => ShellResult {
            exit: None,
            stopped: false,
            output: why.clone(),
        },
    }
}

/// What the agent is told of a command that ended, as Claude Code tells it of
/// one run with `!`. An exit status other than zero, a command stopped before
/// its end and one that did not run are said with its errors.
pub fn told_of(command: &str, ended: &Ended) -> String {
    let (stdout, stderr) = match ended {
        Ended::Ran(output) => {
            let mut stderr = output.stderr.trim_end().to_owned();
            match output.status {
                Some(0) => {}
                Some(code) => push_line(&mut stderr, &format!("[exit code {code}]")),
                None => push_line(&mut stderr, "[ended by a signal]"),
            }
            (output.stdout.trim_end().to_owned(), stderr)
        }
        Ended::Stopped(why) => (
            String::new(),
            format!("[{why} before it ended; it may have done part of its work]"),
        ),
        Ended::NotRun(why) => (String::new(), format!("[not run: {why}]")),
    };
    format!(
        "<bash-input>{command}</bash-input>\n<bash-stdout>{}</bash-stdout><bash-stderr>{}</bash-stderr>",
        clip(&stdout, TOLD_MOST),
        clip(&stderr, TOLD_MOST)
    )
}

fn push_line(text: &mut String, line: &str) {
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(line);
}

/// `text` cut to about `most` characters: its beginning and its end, with
/// how much was left out between them.
fn clip(text: &str, most: usize) -> String {
    let count = text.chars().count();
    if count <= most {
        return text.to_owned();
    }
    let half = most / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(count - half).collect();
    format!(
        "{head}\n[\u{2026} {} characters left out \u{2026}]\n{tail}",
        count - 2 * half
    )
}

/// The lines of `output` shown under its command: the last
/// [`SHOWN_LINES`], after a line that says how many are above.
pub fn shown_output(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= SHOWN_LINES {
        return output.to_owned();
    }
    let above = lines.len() - SHOWN_LINES;
    let mut shown = vec![if above == 1 {
        "[1 line above]".to_owned()
    } else {
        format!("[{above} lines above]")
    }];
    shown.extend(lines[above..].iter().map(|line| (*line).to_owned()));
    shown.join("\n")
}

impl Shell {
    /// The person pressed Enter on `!<command>`: the card asks before it runs.
    pub(super) fn chat_shell_ask(&mut self, id: LiveId, command: &str, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if let Some(running) = chat.shell.running() {
            let running = running.to_owned();
            self.engine.report(
                StatusKind::Info,
                format!("{running:?} is still running: stop it or wait for it first."),
            );
            cx.notify();
            return;
        }
        chat.shell.ask = Some(command.to_owned());
        cx.notify();
    }

    /// The person said no: the card goes, the command stays in the field.
    pub(super) fn chat_shell_cancel(&mut self, id: LiveId, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.get_mut(&id) {
            if chat.shell.ask.take().is_some() {
                cx.notify();
            }
        }
    }

    /// The field's text changed: a card that asks about other words goes.
    pub(super) fn chat_shell_typed(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let Some(asked) = chat.shell.ask.as_deref() else {
            return;
        };
        let text = chat.input.read(cx).value().to_string();
        if command_of(&text) != Some(asked) {
            chat.shell.ask = None;
            cx.notify();
        }
    }

    /// The person said run it: the command on the card runs, in the
    /// conversation, and the field is emptied.
    pub(super) fn chat_shell_run(
        &mut self,
        id: LiveId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = Some(self.now().timestamp());
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if chat.shell.running.is_some() {
            return;
        }
        let Some(command) = chat.shell.ask.take() else {
            return;
        };
        chat.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        chat.recall = None;
        let item = chat.conversation.ran_at(&command, now);
        chat.follow_end();
        let job = self
            .engine
            .run_typed(chat.machine.clone(), chat.cwd.clone(), command.clone());
        let abort = job.abort_handle();
        let waiting = cx.spawn(async move |this, cx| {
            let ended = Ended::of(job.await);
            this.update(cx, |this, cx| this.chat_shell_ended(id, ended, cx))
                .ok();
        });
        if let Some(chat) = self.chats.get_mut(&id) {
            chat.shell.running = Some(Running {
                item,
                command,
                abort,
                _waiting: waiting,
            });
        }
        // The spinners of the command turn while it runs.
        self.chat_spin(id, cx);
        cx.notify();
    }

    /// Ends the command that runs; its item says it was stopped.
    pub(super) fn chat_shell_stop(&mut self, id: LiveId) {
        if let Some(running) = self
            .chats
            .get(&id)
            .and_then(|chat| chat.shell.running.as_ref())
        {
            running.abort.abort();
        }
    }

    /// The command that ran ended: its item says how, and the agent is sent
    /// what it printed.
    fn chat_shell_ended(&mut self, id: LiveId, ended: Ended, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        let Some(running) = chat.shell.running.take() else {
            return;
        };
        chat.conversation
            .shell_ended(running.item, result_of(&ended));
        chat.shell.told.push(told_of(&running.command, &ended));
        chat.follow_end();
        self.chat_shell_deliver(id, cx);
        cx.notify();
    }

    /// Sends the agent what the person's commands printed, in a turn of its
    /// own, when it waits for the person: not while a turn runs or is being
    /// made ready (it gets it when that ends), and not when the person
    /// queued a message (that carries it).
    pub(super) fn chat_shell_deliver(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        if chat.shell.told.is_empty()
            || chat.turn_is_open()
            || chat.launching
            || !chat.conversation.queued.is_empty()
        {
            return;
        }
        self.chat_begin_turn(id, String::new(), false, cx);
    }

    /// Above the field: the card that asks before a command runs, or the
    /// command that runs, with what stops it.
    pub(super) fn chat_shell_cell(
        &self,
        id: LiveId,
        chat: &Chat,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(command) = chat.shell.running() {
            return Some(
                div()
                    .debug_selector(move || format!("chat-shell-running-{}", id.0))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        mono(super::king_view::spinner_frame())
                            .flex_none()
                            .text_color(colours.warning),
                    )
                    .child(mono("RUNNING").flex_none().text_color(colours.text_faint))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::mono())
                            .font_features(fonts::mono_features())
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(command.to_owned()),
                    )
                    .child(
                        button("chat-shell-stop", id, "STOP", Look::Outline, colours).on_click(
                            cx.listener(move |this, _: &ClickEvent, _, _| {
                                this.chat_shell_stop(id);
                            }),
                        ),
                    )
                    .into_any_element(),
            );
        }
        let command = chat.shell.ask.clone()?;
        let machine = self
            .snapshot
            .machine(&chat.machine)
            .map_or_else(|| "this machine".to_owned(), |machine| machine.name.clone());
        Some(
            div()
                .debug_selector(move || format!("chat-shell-ask-{}", id.0))
                .w_full()
                .min_w_0()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.warning)
                .bg(colours.surface)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(led(colours.warning))
                        .child(mono("[ RUN THIS COMMAND? ]").text_color(colours.warning)),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(metrics::RADIUS())
                        .border_1()
                        .border_color(colours.border)
                        .bg(colours.surface_2)
                        .font_family(fonts::mono())
                        .font_features(fonts::mono_features())
                        .text_color(colours.text)
                        .child(command),
                )
                .child(
                    div()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_muted)
                        .child(format!(
                            "In {} on {machine}. What it prints stays in the conversation, and {} reads it and goes on.",
                            chat.cwd,
                            crate::format::agent_name(chat.agent)
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            button("chat-shell-run", id, "RUN", Look::Primary, colours).on_click(
                                cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.chat_shell_run(id, window, cx);
                                }),
                            ),
                        )
                        .child(
                            button("chat-shell-cancel", id, "CANCEL", Look::Quiet, colours)
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.chat_shell_cancel(id, cx);
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            mono("ENTER RUNS \u{b7} ESC CANCELS").text_color(colours.text_faint),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_what_follows_the_bang() {
        assert_eq!(command_of("!ls -la"), Some("ls -la"));
        assert_eq!(command_of("  ! git status  "), Some("git status"));
        assert_eq!(command_of("!"), None);
        assert_eq!(command_of("fix the build!"), None);
    }

    #[test]
    fn the_agent_is_told_as_claude_code_tells_it() {
        let ok = Ended::Ran(Output::ok("a\nb\n"));
        assert_eq!(
            told_of("ls", &ok),
            "<bash-input>ls</bash-input>\n<bash-stdout>a\nb</bash-stdout><bash-stderr></bash-stderr>"
        );
        let failed = Ended::Ran(Output {
            status: Some(2),
            stdout: String::new(),
            stderr: "no such file\n".to_owned(),
        });
        assert!(told_of("cat x", &failed)
            .ends_with("<bash-stderr>no such file\n[exit code 2]</bash-stderr>"));
        assert!(told_of("x", &Ended::NotRun("cannot start sh".to_owned()))
            .contains("<bash-stderr>[not run: cannot start sh]</bash-stderr>"));
        // Stopped is not "not run": it may have done part of its work.
        let stopped = told_of(
            "cp -a big big.bak",
            &Ended::Stopped("stopped by the person".into()),
        );
        assert!(stopped.contains(
            "[stopped by the person before it ended; it may have done part of its work]"
        ));
        assert!(!stopped.contains("not run"));
    }

    #[test]
    fn a_long_output_keeps_its_beginning_and_its_end() {
        let long = "x".repeat(50) + &"y".repeat(50);
        let clipped = clip(&long, 20);
        assert!(clipped.starts_with("xxxxxxxxxx\n"));
        assert!(clipped.ends_with("\nyyyyyyyyyy"));
        assert!(clipped.contains("80 characters left out"));
        assert_eq!(clip("short", 20), "short");
    }

    #[test]
    fn the_conversation_shows_the_last_lines_of_a_long_output() {
        let output = (1..=20)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let shown = shown_output(&output);
        assert!(shown.starts_with("[6 lines above]\n7\n"));
        assert!(shown.ends_with("\n20"));
        assert_eq!(shown_output("one\ntwo"), "one\ntwo");
    }

    #[test]
    fn what_it_printed_is_its_output_then_its_errors() {
        let result = result_of(&Ended::Ran(Output {
            status: Some(1),
            stdout: "out\n".to_owned(),
            stderr: "err\n".to_owned(),
        }));
        assert_eq!(result.exit, Some(1));
        assert_eq!(result.output, "out\nerr");
        assert!(!result.ok());
        let stopped = result_of(&Ended::Stopped("stopped by the person".into()));
        assert!(stopped.stopped && stopped.exit.is_none());
    }

    #[test]
    fn a_time_limit_is_a_stop_and_a_cancelled_job_too() {
        let limit = std::time::Duration::from_secs(600);
        let timed_out = Ended::of(Ok(Err(RunError::TimedOut {
            program: "sh".into(),
            limit,
        })));
        assert!(matches!(&timed_out, Ended::Stopped(why) if why.contains("10 minutes")));
        let refused = Ended::of(Ok(Err(RunError::Spawn {
            program: "sh".into(),
            source: std::io::Error::other("no such machine"),
        })));
        assert!(matches!(refused, Ended::NotRun(_)));
    }
}

//! The application's side of updates.
//!
//! `leon-update` does the work (it knows nothing of windows); this module is
//! what the window and `main` hold: the [`Service`] that runs the work on the
//! engine's runtime and hands over the results, the three modes, the words
//! the footer, the status line and the About card use for each state, and
//! the release notes turned into something that can be drawn safely.
//!
//! Where the trust comes from, what is verified and what is not: see
//! `docs/UPDATES.md`.

use crate::engine::StatusKind;
use leon_update::install::Why;
use leon_update::{Install, Snapshot, State, Updater};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::{oneshot, watch};

/// What Leon does about updates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Look, download, and install at the next restart.
    #[default]
    Automatic,
    /// Look, and tell; the person decides.
    Notify,
    /// Never ask GitHub.
    Off,
}

impl Mode {
    /// Reads the setting's value.
    pub fn parse(text: &str) -> Self {
        match text {
            "notify" => Mode::Notify,
            "off" => Mode::Off,
            _ => Mode::Automatic,
        }
    }
}

/// The updater of this install: GitHub through `curl`, the system's tools,
/// the files in `<data folder>/updates`. It needs no runtime until it is
/// asked to do something, so it can be made before one exists.
pub fn build_updater(data_dir: &Path) -> Arc<Updater> {
    Updater::new(
        leon_update::Config {
            current: leon_update::Version::parse(crate::product::VERSION)
                .expect("the package version is a version"),
            platform: leon_update::Platform::current(),
            layout: leon_update::Layout::new(data_dir.join("updates")),
            install: leon_update::install::detect(),
            now: Arc::new(|| chrono::Utc::now().timestamp()),
        },
        Arc::new(leon_update::CurlHttp::new()),
        Arc::new(leon_update::SystemTools),
    )
}

/// The updater, the runtime it runs on, and the restart that was asked for.
pub struct Service {
    updater: Arc<Updater>,
    handle: tokio::runtime::Handle,
    /// Whether Leon looks for updates by itself: not in a development build
    /// and not with `LEON_NO_UPDATE` set.
    checks: bool,
    /// The version to start when this process has ended.
    restart: Mutex<Option<String>>,
}

impl Service {
    /// A service over an updater made elsewhere (tests give it a scripted
    /// GitHub).
    pub fn with_updater(updater: Arc<Updater>, handle: tokio::runtime::Handle) -> Arc<Self> {
        let checks = !matches!(
            updater.install_kind(),
            Install::Manual(Why::Development | Why::Disabled)
        );
        Arc::new(Self {
            updater,
            handle,
            checks,
            restart: Mutex::new(None),
        })
    }

    /// Whether Leon asks GitHub by itself.
    pub fn checks_enabled(&self) -> bool {
        self.checks
    }

    /// The updater, for what has no wrapper here.
    pub fn updater(&self) -> &Arc<Updater> {
        &self.updater
    }

    /// The state now.
    pub fn snapshot(&self) -> Snapshot {
        self.updater.snapshot()
    }

    /// A receiver of every change of the state.
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.updater.subscribe()
    }

    /// What the install is.
    pub fn install_kind(&self) -> &Install {
        self.updater.install_kind()
    }

    /// The look that happens by itself, in the background: asks, and in the
    /// automatic mode downloads what it finds.
    pub fn background(&self, mode: Mode, prereleases: bool) {
        if !self.checks || mode == Mode::Off {
            return;
        }
        self.updater.set_prereleases(prereleases);
        let updater = self.updater.clone();
        self.handle.spawn(async move {
            let snapshot = updater.check(false).await;
            if mode == Mode::Automatic && matches!(snapshot.state, State::Available(_)) {
                updater.download().await;
            }
        });
    }

    /// A check a person asked for. Answers with the state it ended in.
    pub fn check_now(&self, mode: Mode, prereleases: bool) -> oneshot::Receiver<Snapshot> {
        self.updater.set_prereleases(prereleases);
        let updater = self.updater.clone();
        let (tell, answer) = oneshot::channel();
        self.handle.spawn(async move {
            let mut snapshot = updater.check(true).await;
            if mode == Mode::Automatic && matches!(snapshot.state, State::Available(_)) {
                snapshot = updater.download().await;
            }
            let _ = tell.send(snapshot);
        });
        answer
    }

    /// Downloads the update on offer, whatever the mode says: a person asked.
    pub fn download_now(&self) -> oneshot::Receiver<Snapshot> {
        let updater = self.updater.clone();
        let (tell, answer) = oneshot::channel();
        self.handle.spawn(async move {
            let _ = tell.send(updater.download().await);
        });
        answer
    }

    /// Stops offering the version on offer.
    pub fn skip(&self) {
        self.updater.skip();
    }

    /// Puts the ready update in place and tries it (it blocks for as long as
    /// that takes: a copy of the program and a start of it for a moment).
    pub fn install(&self) -> Result<leon_update::launch::Applied, leon_update::launch::ApplyError> {
        self.updater.install()
    }

    /// Asks `main` to start the new build when this process has ended.
    pub fn request_restart(&self, version: String) {
        *self.restart.lock().unwrap_or_else(|e| e.into_inner()) = Some(version);
    }

    /// The restart that was asked for, once.
    pub fn take_restart(&self) -> Option<String> {
        self.restart
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// The window has been up for a while: an update that was installed is
    /// confirmed, and what is kept of the old version goes.
    pub fn confirm_started(&self) -> bool {
        self.updater.startup().confirm_started()
    }

    /// A line about what happened without the person, once.
    pub fn take_notice(&self) -> Option<String> {
        self.updater.take_notice()
    }

    /// The page to download a release from by hand.
    pub fn download_page(&self) -> String {
        self.updater.download_page()
    }
}

/// The offer in a state, when there is one.
pub fn offer_of(state: &State) -> Option<&leon_update::Offer> {
    match state {
        State::Available(offer)
        | State::Manual { offer, .. }
        | State::Downloading { offer, .. }
        | State::Ready { offer, .. }
        | State::Installing(offer)
        | State::RestartRequired(offer) => Some(offer),
        State::Failed { offer, .. } => offer.as_ref(),
        _ => None,
    }
}

/// What a click on the footer's item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemAction {
    /// Show the release notes.
    Notes,
    /// Restart to update.
    Restart,
    /// Check again.
    Check,
}

/// The footer's item for a state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What it says.
    pub text: String,
    /// The tooltip.
    pub tooltip: String,
    /// What a click does.
    pub action: ItemAction,
    /// The share downloaded, in percent, while downloading.
    pub progress: Option<u8>,
}

/// The percentage of a download, 0 to 100.
pub fn percent(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((done.min(total) as u128 * 100) / total as u128) as u8
}

fn unsigned_note(trust: &leon_update::Trust) -> &'static str {
    match trust {
        leon_update::Trust::Unsigned => " This build is not signed.",
        _ => "",
    }
}

/// What the footer shows for a state; nothing for most.
pub fn footer_item(state: &State) -> Option<Item> {
    Some(match state {
        State::Available(offer) => Item {
            text: format!("Update available · {}", offer.version),
            tooltip: "Read the release notes and decide".to_owned(),
            action: ItemAction::Notes,
            progress: None,
        },
        State::Manual { offer, why } => Item {
            text: format!("Update available · {}", offer.version),
            tooltip: format!(
                "{} Open the release notes for the download page.",
                why.explain()
            ),
            action: ItemAction::Notes,
            progress: None,
        },
        State::Downloading { offer, done, total } => {
            let share = percent(*done, *total);
            Item {
                text: format!("Downloading {} · {share}%", offer.version),
                tooltip: "Downloading the update; it is installed when you restart".to_owned(),
                action: ItemAction::Notes,
                progress: Some(share),
            }
        }
        State::Ready { offer, trust } => Item {
            text: format!("Restart to update · {}", offer.version),
            tooltip: format!(
                "Restart Leon into version {}.{}",
                offer.version,
                unsigned_note(trust)
            ),
            action: ItemAction::Restart,
            progress: None,
        },
        State::Installing(offer) => Item {
            text: format!("Installing {}…", offer.version),
            tooltip: "Putting the new version in place".to_owned(),
            action: ItemAction::Notes,
            progress: None,
        },
        State::RestartRequired(offer) => Item {
            text: format!("Restart to update · {}", offer.version),
            tooltip: "The new version is in place; restart Leon to use it".to_owned(),
            action: ItemAction::Restart,
            progress: None,
        },
        State::Failed {
            reason,
            stage: leon_update::Stage::Download | leon_update::Stage::Install,
            ..
        } => Item {
            text: "Update failed".to_owned(),
            tooltip: format!("{reason}. Click to check again."),
            action: ItemAction::Check,
            progress: None,
        },
        _ => return None,
    })
}

/// What the status line says after a check a person asked for.
pub fn check_report(snapshot: &Snapshot, current: &str, mode: Mode) -> (StatusKind, String) {
    let info = |text: String| (StatusKind::Info, text);
    match &snapshot.state {
        State::UpToDate => info(format!("Leon {current} is the latest.")),
        State::Available(offer) => info(match mode {
            Mode::Automatic => format!("Leon {} is available; downloading it.", offer.version),
            _ => format!(
                "Leon {} is available. Open the release notes to decide.",
                offer.version
            ),
        }),
        State::Downloading { offer, .. } => info(format!("Leon {} is downloading.", offer.version)),
        State::Ready { offer, .. } | State::RestartRequired(offer) => info(format!(
            "Leon {} is ready: restart to update.",
            offer.version
        )),
        State::Installing(offer) => info(format!("Installing Leon {}…", offer.version)),
        State::Manual { offer, why } => info(format!(
            "Leon {} is available. {} Open the download page.",
            offer.version,
            why.explain()
        )),
        State::NoBuild { version } => info(format!(
            "Leon {version} is out, but it has no build for this platform yet."
        )),
        State::Failed { reason, .. } => info(format!("Could not check for updates: {reason}.")),
        State::Idle | State::Checking => info(
            "GitHub asked Leon to wait before the next check; it will look again later.".to_owned(),
        ),
    }
}

/// The line the About card shows for updates.
pub fn about_line(snapshot: &Snapshot, mode: Mode, install: &Install) -> String {
    match install {
        Install::Manual(Why::Development) => return "UPDATES OFF · DEVELOPMENT BUILD".to_owned(),
        Install::Manual(Why::Disabled) => return "UPDATES OFF · LEON_NO_UPDATE".to_owned(),
        _ => {}
    }
    if mode == Mode::Off {
        return "UPDATES OFF".to_owned();
    }
    let text = match &snapshot.state {
        State::Idle => "NOT CHECKED YET".to_owned(),
        State::Checking => "CHECKING…".to_owned(),
        State::UpToDate => "UP TO DATE".to_owned(),
        State::Available(offer) => format!("{} AVAILABLE", offer.version),
        State::Manual { offer, .. } => format!("{} AVAILABLE · DOWNLOAD IT BY HAND", offer.version),
        State::NoBuild { version } => format!("{version} HAS NO BUILD FOR THIS PLATFORM"),
        State::Downloading { offer, done, total } => {
            format!(
                "DOWNLOADING {} · {}%",
                offer.version,
                percent(*done, *total)
            )
        }
        State::Ready { offer, trust } => format!(
            "RESTART TO UPDATE · {}{}",
            offer.version,
            if matches!(trust, leon_update::Trust::Unsigned) {
                " · NOT SIGNED"
            } else {
                ""
            }
        ),
        State::Installing(offer) => format!("INSTALLING {}", offer.version),
        State::RestartRequired(offer) => format!("RESTART TO UPDATE · {}", offer.version),
        State::Failed { stage, .. } => match stage {
            leon_update::Stage::Check => "COULD NOT CHECK".to_owned(),
            _ => "UPDATE FAILED".to_owned(),
        },
    };
    text
}

/// A line of release notes, as it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteLine {
    /// A heading, with its level (1 to 3).
    Heading(u8, String),
    /// A bullet.
    Bullet(String),
    /// A line of text.
    Text(String),
    /// A line of a code block.
    Code(String),
    /// A gap.
    Blank,
}

/// The most lines of notes that are drawn.
pub const MAX_NOTE_LINES: usize = 300;

/// Removes `<…>` tags and control characters.
fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            c if c.is_control() && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// `[text](url)` becomes `text`, `![alt](url)` becomes `alt`: nothing is
/// fetched from an address in the notes and nothing in them is a link.
fn flatten_links(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let (before, after) = rest.split_at(open);
        let image = before.ends_with('!');
        let before = if image {
            &before[..before.len() - 1]
        } else {
            before
        };
        out.push_str(before);
        let inner = &after[1..];
        let linked = inner.find("](").and_then(|close| {
            let url = &inner[close + 2..];
            url.find(')').map(|end| (&inner[..close], &url[end + 1..]))
        });
        match linked {
            Some((label, tail)) => {
                out.push_str(label);
                rest = tail;
            }
            None => {
                out.push('[');
                rest = inner;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Drops the emphasis marks of inline markdown.
fn plain(text: &str) -> String {
    flatten_links(&strip_markup(text))
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
}

/// Release notes as lines to draw: headings, bullets, text and code, with
/// every tag, link and image reduced to its words.
pub fn note_lines(notes: &str) -> Vec<NoteLine> {
    let mut lines = Vec::new();
    let mut in_code = false;
    let mut blank = true;
    for raw in notes.lines().take(MAX_NOTE_LINES * 2) {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            let code: String = line
                .chars()
                .filter(|c| !c.is_control() || *c == '\t')
                .collect();
            lines.push(NoteLine::Code(code));
            blank = false;
            continue;
        }
        let trimmed = line.trim_start();
        let item = if trimmed.is_empty() {
            if !blank {
                lines.push(NoteLine::Blank);
            }
            blank = true;
            continue;
        } else if let Some(level) = trimmed
            .strip_prefix("### ")
            .map(|t| (3, t))
            .or_else(|| trimmed.strip_prefix("## ").map(|t| (2, t)))
            .or_else(|| trimmed.strip_prefix("# ").map(|t| (1, t)))
        {
            NoteLine::Heading(level.0, plain(level.1).trim().to_owned())
        } else if let Some(text) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            NoteLine::Bullet(plain(text).trim().to_owned())
        } else {
            NoteLine::Text(plain(trimmed).trim().to_owned())
        };
        let empty = match &item {
            NoteLine::Heading(_, text) | NoteLine::Bullet(text) | NoteLine::Text(text) => {
                text.is_empty()
            }
            _ => false,
        };
        if !empty {
            lines.push(item);
            blank = false;
        }
        if lines.len() >= MAX_NOTE_LINES {
            lines.push(NoteLine::Text("…".to_owned()));
            break;
        }
    }
    while lines.last() == Some(&NoteLine::Blank) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_update::{Offer, Stage, Trust};

    fn offer(version: &str) -> Offer {
        Offer {
            version: version.to_owned(),
            notes: String::new(),
            page: "https://github.com/zavudev/leon/releases/tag/v0.2.1".into(),
            size: 10,
            prerelease: false,
        }
    }

    fn snap(state: State) -> Snapshot {
        Snapshot {
            state,
            notice: None,
            checked_at: None,
        }
    }

    #[test]
    fn the_modes_are_read_from_the_setting() {
        assert_eq!(Mode::parse("automatic"), Mode::Automatic);
        assert_eq!(Mode::parse("notify"), Mode::Notify);
        assert_eq!(Mode::parse("off"), Mode::Off);
        assert_eq!(Mode::parse("garbage"), Mode::Automatic);
    }

    #[test]
    fn the_footer_has_an_item_only_when_there_is_something_to_say() {
        for quiet in [
            State::Idle,
            State::Checking,
            State::UpToDate,
            State::NoBuild {
                version: "1".into(),
            },
            State::Failed {
                reason: "x".into(),
                retry_at: None,
                stage: Stage::Check,
                offer: None,
            },
        ] {
            assert_eq!(footer_item(&quiet), None, "{quiet:?}");
        }
        let available = footer_item(&State::Available(offer("0.2.1"))).unwrap();
        assert_eq!(available.text, "Update available · 0.2.1");
        assert_eq!(available.action, ItemAction::Notes);
        let downloading = footer_item(&State::Downloading {
            offer: offer("0.2.1"),
            done: 50,
            total: 200,
        })
        .unwrap();
        assert_eq!(downloading.text, "Downloading 0.2.1 · 25%");
        assert_eq!(downloading.progress, Some(25));
        let ready = footer_item(&State::Ready {
            offer: offer("0.2.1"),
            trust: Trust::Unsigned,
        })
        .unwrap();
        assert_eq!(ready.text, "Restart to update · 0.2.1");
        assert_eq!(ready.action, ItemAction::Restart);
        assert!(ready.tooltip.contains("not signed"));
        let failed = footer_item(&State::Failed {
            reason: "disk full".into(),
            retry_at: None,
            stage: Stage::Download,
            offer: None,
        })
        .unwrap();
        assert_eq!(failed.action, ItemAction::Check);
        assert!(failed.tooltip.contains("disk full"));
    }

    #[test]
    fn a_manual_check_says_what_it_found() {
        let say = |state: State, mode| check_report(&snap(state), "0.2.0", mode).1;
        assert_eq!(
            say(State::UpToDate, Mode::Automatic),
            "Leon 0.2.0 is the latest."
        );
        assert!(say(State::Available(offer("0.2.1")), Mode::Automatic).contains("downloading"));
        assert!(say(State::Available(offer("0.2.1")), Mode::Notify).contains("release notes"));
        assert!(say(State::Idle, Mode::Notify).contains("wait"));
        assert!(say(
            State::Manual {
                offer: offer("0.2.1"),
                why: Why::Translocated
            },
            Mode::Notify
        )
        .contains("Applications"));
    }

    #[test]
    fn the_about_line_names_the_state_and_the_reason_updates_are_off() {
        let install = Install::Manual(Why::NotWritable);
        let line = |state: State, mode, install: &Install| about_line(&snap(state), mode, install);
        assert_eq!(
            line(State::UpToDate, Mode::Automatic, &install),
            "UP TO DATE"
        );
        assert_eq!(line(State::UpToDate, Mode::Off, &install), "UPDATES OFF");
        assert_eq!(
            line(
                State::UpToDate,
                Mode::Automatic,
                &Install::Manual(Why::Development)
            ),
            "UPDATES OFF · DEVELOPMENT BUILD"
        );
        assert_eq!(
            line(
                State::Ready {
                    offer: offer("0.2.1"),
                    trust: Trust::Unsigned
                },
                Mode::Automatic,
                &install
            ),
            "RESTART TO UPDATE · 0.2.1 · NOT SIGNED"
        );
    }

    #[test]
    fn the_percentage_is_bounded() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(5, 0), 0);
        assert_eq!(percent(10, 10), 100);
        assert_eq!(percent(20, 10), 100);
        assert_eq!(percent(1, 3), 33);
    }

    #[test]
    fn notes_become_lines_without_markup_links_or_images() {
        let notes = "## What's new\n\n- **Faster** start, see [the guide](https://evil.example/x)\n- ![shot](https://evil.example/a.png) a picture\n\n<script>alert(1)</script>Plain <b>bold</b> text\n```\ncargo run\n```\n\n\n";
        let lines = note_lines(notes);
        assert_eq!(
            lines,
            vec![
                NoteLine::Heading(2, "What's new".into()),
                NoteLine::Blank,
                NoteLine::Bullet("Faster start, see the guide".into()),
                NoteLine::Bullet("shot a picture".into()),
                NoteLine::Blank,
                NoteLine::Text("alert(1)Plain bold text".into()),
                NoteLine::Code("cargo run".into()),
            ]
        );
        for line in &lines {
            let text = format!("{line:?}");
            assert!(
                !text.contains("evil.example") && !text.contains('<'),
                "{text}"
            );
        }
    }

    #[test]
    fn notes_are_bounded_and_control_characters_go() {
        let long = "line\n".repeat(5000);
        let lines = note_lines(&long);
        assert!(lines.len() <= MAX_NOTE_LINES + 1);
        assert_eq!(lines.last(), Some(&NoteLine::Text("…".into())));
        assert_eq!(
            note_lines("a\u{1b}[31mred\u{7}"),
            vec![NoteLine::Text("a[31mred".into())]
        );
        assert!(note_lines("").is_empty());
    }

    #[test]
    fn unterminated_links_and_brackets_are_left_as_text() {
        assert_eq!(
            note_lines("see [this and (that"),
            vec![NoteLine::Text("see [this and (that".into())]
        );
    }
}

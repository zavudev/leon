//! Putting an update in place, and the way back.
//!
//! An update is applied by a process that works, which is why going back is
//! dependable. It happens in one of three places:
//!
//! * **Restart to update** (the person's choice): the running Leon quits,
//!   and what is left of its process ([`hand_over`]) puts the new build in
//!   place, starts it, and watches it. The new build is told to wait until
//!   that process has let go of the data ([`wait_for_parent`]).
//! * **Quitting with an update ready**, in the automatic mode and with no
//!   session running: the same swap, without starting anything, on the way out
//!   ([`apply_staged`]).
//! * **Next start**, in the automatic mode, when an update was ready and the
//!   last run did not end the usual way: [`Startup::run`] applies it before
//!   anything is opened, starts the new build and watches it.
//!
//! Whichever applied it, the new build is tried before the old one is let
//! go of: it must print its version when asked ([`crate::tools::Tools::probe`]), and
//! when it is started for real it must say that it came up
//! ([`Startup::confirm_started`]) within [`WATCH`]. If it does not, the old
//! one is put back, the version is recorded as refused (it is not tried
//! again until a newer release exists) and the person is told. A new build
//! that starts but keeps failing before it says so takes itself out after
//! [`MAX_UNCONFIRMED_STARTS`] starts.

use crate::checksums;
use crate::install::{Install, Target};
use crate::package::payload_name;
use crate::release::{Os, Platform};
use crate::state::{Layout, Pending, Saved};
use crate::tools::{Signature, Tools};
use crate::trust;
use crate::version::Version;
use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq as _;

/// Set in the environment of a process started by [`hand_over`]: it waits for
/// the process that started it to let go before it does anything.
pub const WAIT_FOR_PARENT: &str = "LEON_WAIT_FOR_PARENT";

/// How long a new build has to say that it came up.
pub const WATCH: Duration = Duration::from_secs(45);

/// How many times a new build may start without reaching its window before it
/// takes itself out.
pub const MAX_UNCONFIRMED_STARTS: u32 = 3;

/// Why an update was not put in place.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// Nothing is ready.
    #[error("no update is ready")]
    NothingReady,
    /// What is ready is not newer than what runs.
    #[error("the downloaded version is not newer than this one")]
    NotNewer,
    /// The files no longer verify.
    #[error("the downloaded update no longer verifies: {0}")]
    Untrusted(String),
    /// Files could not be moved.
    #[error("the update could not be put in place: {0}")]
    Io(String),
    /// The new build did not run, and the old one was put back.
    #[error("the new version did not run ({0}); the previous one was kept")]
    DidNotRun(String),
}

/// What was put in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The version now installed.
    pub version: String,
}

fn version_of_probe(output: &str) -> Option<&str> {
    output.split_whitespace().last()
}

/// Puts the staged update in place of the installed build, tries it, and
/// leaves the old one beside it. Nothing is started. On failure the old one is
/// as it was; a download that no longer verifies, or is not newer, is dropped.
pub fn apply_staged(
    layout: &Layout,
    saved: &mut Saved,
    target: &Target,
    tools: &dyn Tools,
    platform: &Platform,
    running: &Signature,
    current: &Version,
) -> Result<Applied, ApplyError> {
    let result = try_apply(layout, saved, target, tools, platform, running, current);
    if matches!(
        result,
        Err(ApplyError::Untrusted(_)) | Err(ApplyError::NotNewer)
    ) {
        saved.staged = None;
        let _ = layout.save(saved);
        layout.drop_downloads_except(None);
    }
    result
}

fn try_apply(
    layout: &Layout,
    saved: &mut Saved,
    target: &Target,
    tools: &dyn Tools,
    platform: &Platform,
    running: &Signature,
    current: &Version,
) -> Result<Applied, ApplyError> {
    let staged = saved.staged.clone().ok_or(ApplyError::NothingReady)?;
    let version = Version::parse(&staged.offer.version)
        .map_err(|_| ApplyError::Untrusted("the staged version is not a version".into()))?;
    if &version <= current {
        return Err(ApplyError::NotNewer);
    }
    let untrusted = |why: String| ApplyError::Untrusted(why);

    // The archive is the one that was verified.
    let archive = layout
        .version_dir(&staged.offer.version)
        .join(&staged.archive);
    let (hash, _) = checksums::hash_file(&archive).map_err(|error| untrusted(error.to_string()))?;
    let recorded = checksums::decode_hex(&staged.sha256)
        .ok_or_else(|| untrusted("the recorded checksum is damaged".into()))?;
    if !bool::from(hash.ct_eq(&recorded)) {
        return Err(untrusted(
            "the archive changed after it was verified".into(),
        ));
    }
    // And so is the program, held to the running build once more.
    let payload = layout
        .payload_dir(&staged.offer.version)
        .join(payload_name(platform));
    if !payload.exists() {
        return Err(untrusted("the unpacked program is gone".into()));
    }
    let signature = tools.signature(&payload).map_err(untrusted)?;
    trust::judge(platform.os, running, &signature).map_err(untrusted)?;

    layout
        .set_pending(&Pending {
            version: staged.offer.version.clone(),
            previous: current.to_string(),
            starts: 0,
        })
        .map_err(|error| ApplyError::Io(error.to_string()))?;
    if let Err(error) = target.replace_with(&payload, tools) {
        layout.clear_pending();
        return Err(ApplyError::Io(error.to_string()));
    }
    // The new build has to run at all.
    let ran =
        tools
            .probe(&target.executable())
            .and_then(|output| match version_of_probe(&output) {
                Some(printed) if printed == staged.offer.version => Ok(()),
                Some(printed) => Err(format!("it says it is {printed}")),
                None => Err("it printed nothing".to_owned()),
            });
    if let Err(why) = ran {
        let _ = target.restore();
        layout.clear_pending();
        refuse(
            layout,
            saved,
            &staged.offer.version,
            format!(
                "Version {} did not run on this computer ({why}), so version {current} was kept. It will not be tried again; the next release will.",
                staged.offer.version
            ),
        );
        return Err(ApplyError::DidNotRun(why));
    }
    saved.staged = None;
    let _ = layout.save(saved);
    layout.drop_downloads_except(None);
    Ok(Applied {
        version: staged.offer.version,
    })
}

/// Records a version as not to be tried again, with a line for the person.
pub fn refuse(layout: &Layout, saved: &mut Saved, version: &str, notice: String) {
    if !saved.refused.iter().any(|known| known == version) {
        saved.refused.push(version.to_owned());
    }
    saved.staged = None;
    saved.notice = Some(notice);
    let _ = layout.save(saved);
    layout.drop_downloads_except(None);
}

/// A started process that can be asked whether it ended.
pub trait Child {
    /// `None` while it runs; then whether it ended well.
    fn ended(&mut self) -> std::io::Result<Option<bool>>;
    /// Lets the process go on: the end of the pipe it waits on is closed.
    fn release(&mut self) {}
}

/// Starts a program (with the arguments this process had).
pub type Spawn<'a> = &'a dyn Fn(&Path, bool) -> std::io::Result<Box<dyn Child>>;

struct RealChild(std::process::Child);

impl Child for RealChild {
    fn ended(&mut self) -> std::io::Result<Option<bool>> {
        Ok(self.0.try_wait()?.map(|status| status.success()))
    }

    fn release(&mut self) {
        drop(self.0.stdin.take());
    }
}

/// The usual [`Spawn`]: starts the program with `args`; with `wait` the child
/// holds until [`Child::release`] (or the end of this process).
pub fn spawn_with(args: Vec<OsString>) -> impl Fn(&Path, bool) -> std::io::Result<Box<dyn Child>> {
    move |program: &Path, wait: bool| {
        let mut command = leon_remote::spawn::std_child(program);
        command.args(&args);
        if wait {
            command
                .env(WAIT_FOR_PARENT, "1")
                .stdin(std::process::Stdio::piped());
        } else {
            command
                .env_remove(WAIT_FOR_PARENT)
                .stdin(std::process::Stdio::null());
        }
        Ok(Box::new(RealChild(command.spawn()?)) as Box<dyn Child>)
    }
}

/// In a process started with [`WAIT_FOR_PARENT`]: waits until the process that
/// started it lets go (closes the pipe, or ends), or `longest` has passed.
/// Anywhere else it returns at once.
pub fn wait_for_parent(longest: Duration) {
    if std::env::var_os(WAIT_FOR_PARENT).is_none() {
        return;
    }
    let (done, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read as _;
        let mut sink = [0u8; 64];
        let mut input = std::io::stdin().lock();
        while matches!(input.read(&mut sink), Ok(read) if read > 0) {}
        let _ = done.send(());
    });
    let _ = ended.recv_timeout(longest);
}

/// What the start of the application does next.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Go on starting, as the version this is.
    Continue,
    /// This process is done.
    Exit(i32),
}

/// How a started build got on.
#[derive(Debug, PartialEq, Eq)]
enum Watched {
    Confirmed,
    EndedWell,
    Failed,
    StillStarting,
}

fn watch(layout: &Layout, child: &mut dyn Child, longest: Duration, tick: Duration) -> Watched {
    let deadline = Instant::now() + longest;
    loop {
        if layout.pending().is_none() {
            return Watched::Confirmed;
        }
        match child.ended() {
            Ok(Some(true)) => return Watched::EndedWell,
            Ok(Some(false)) => return Watched::Failed,
            Ok(None) => {}
            Err(_) => return Watched::StillStarting,
        }
        if Instant::now() >= deadline {
            return Watched::StillStarting;
        }
        std::thread::sleep(tick);
    }
}

/// What the start of the application needs to know.
pub struct Startup<'a> {
    /// The version that is starting.
    pub current: Version,
    /// The update folder.
    pub layout: &'a Layout,
    /// What kind of install this is.
    pub install: &'a Install,
    /// The system's help.
    pub tools: &'a dyn Tools,
    /// The platform.
    pub platform: Platform,
    /// How long to watch a new build start, and how often to look.
    pub watch: (Duration, Duration),
}

impl Startup<'_> {
    /// The start of the application, before anything is opened. With
    /// `apply_ready`, an update that is ready is put in place, started and
    /// watched; without it, only what a previous run left is looked after.
    pub fn run(&self, apply_ready: bool, spawn: Spawn<'_>) -> Outcome {
        let Some(target) = self.install.target() else {
            return Outcome::Continue;
        };
        let mut saved = self.layout.load();
        if let Some(mut pending) = self.layout.pending() {
            if pending.version == self.current.to_string() {
                // This is the new version, not yet seen to get to its window.
                pending.starts += 1;
                let _ = self.layout.set_pending(&pending);
                if pending.starts > MAX_UNCONFIRMED_STARTS {
                    if let Some(outcome) = self.take_itself_out(target, &pending, &mut saved, spawn)
                    {
                        return outcome;
                    }
                }
                return Outcome::Continue;
            }
            // A mark for another version: the one before was started again.
            self.layout.clear_pending();
        }
        // Nothing waits to be confirmed: what an earlier update left goes.
        if target.has_leftovers() {
            let _ = target.clean();
        }
        if !apply_ready || saved.staged.is_none() {
            return Outcome::Continue;
        }
        let running = self
            .tools
            .signature(&target.path)
            .unwrap_or_else(|_| Signature::none());
        match apply_staged(
            self.layout,
            &mut saved,
            target,
            self.tools,
            &self.platform,
            &running,
            &self.current,
        ) {
            Ok(applied) => {
                tracing::info!(version = %applied.version, "the update is in place; starting it");
                self.start_and_watch(target, &applied.version, &mut saved, spawn)
            }
            Err(error) => {
                tracing::warn!(%error, "the update was not put in place");
                Outcome::Continue
            }
        }
    }

    /// The new build is installed: it is started (told to wait for this
    /// process to let go), released at once (this process opens nothing) and
    /// watched until it says it is up.
    pub fn start_and_watch(
        &self,
        target: &Target,
        version: &str,
        saved: &mut Saved,
        spawn: Spawn<'_>,
    ) -> Outcome {
        let mut child = match spawn(&target.executable(), true) {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!(%error, "the new version could not be started");
                return self.go_back(target, version, saved, spawn);
            }
        };
        child.release();
        match watch(self.layout, child.as_mut(), self.watch.0, self.watch.1) {
            Watched::Confirmed => Outcome::Exit(0),
            Watched::EndedWell => {
                // It ran and ended well before it got to say so: closed at once.
                self.layout.clear_pending();
                Outcome::Exit(0)
            }
            Watched::StillStarting => Outcome::Exit(0),
            Watched::Failed => self.go_back(target, version, saved, spawn),
        }
    }

    /// The new build did not come up: the old one is put back and started.
    fn go_back(
        &self,
        target: &Target,
        version: &str,
        saved: &mut Saved,
        spawn: Spawn<'_>,
    ) -> Outcome {
        self.layout.clear_pending();
        match target.restore() {
            Ok(()) => {
                refuse(
                    self.layout,
                    saved,
                    version,
                    format!(
                        "Version {version} did not start, so version {} was put back. It will not be tried again; the next release will.",
                        self.current
                    ),
                );
                tracing::warn!(%version, "the new version did not start; the previous one is back");
                match spawn(&target.executable(), false) {
                    Ok(_) => Outcome::Exit(0),
                    Err(_) => Outcome::Exit(1),
                }
            }
            Err(error) => {
                tracing::error!(%error, "the previous version could not be put back");
                eprintln!(
                    "Version {version} did not start and version {} could not be put back ({error}). Download Leon again from {}.",
                    self.current,
                    crate::release::RELEASES_PAGE
                );
                Outcome::Exit(1)
            }
        }
    }

    /// A new build that has started several times without reaching its
    /// window, with nobody watching it, puts the build before back itself.
    fn take_itself_out(
        &self,
        target: &Target,
        pending: &Pending,
        saved: &mut Saved,
        spawn: Spawn<'_>,
    ) -> Option<Outcome> {
        if !target.has_backup() {
            return None;
        }
        target.restore().ok()?;
        self.layout.clear_pending();
        refuse(
            self.layout,
            saved,
            &pending.version,
            format!(
                "Version {} did not come up, so version {} was put back. It will not be tried again; the next release will.",
                pending.version, pending.previous
            ),
        );
        Some(match spawn(&target.executable(), false) {
            Ok(_) => Outcome::Exit(0),
            Err(_) => Outcome::Exit(1),
        })
    }

    /// Called once the window of this build has been up for a while: the
    /// update, if this is the first start after one, is confirmed and the
    /// version before is removed. `true` when this was the first start after
    /// an update.
    pub fn confirm_started(&self) -> bool {
        let Some(pending) = self.layout.pending() else {
            return false;
        };
        if pending.version != self.current.to_string() {
            return false;
        }
        self.layout.clear_pending();
        if let Some(target) = self.install.target() {
            let _ = target.clean();
        }
        tracing::info!(version = %self.current, "the update started; it is confirmed");
        true
    }
}

/// What is left of the process that quit for "Restart to update". Whether the
/// platform keeps the file of a running program is not what matters here: the
/// swap was already made, and this starts the new build.
pub fn hand_over(startup: &Startup<'_>, version: &str, spawn: Spawn<'_>) -> Outcome {
    let Some(target) = startup.install.target() else {
        return Outcome::Exit(1);
    };
    let mut saved = startup.layout.load();
    startup.start_and_watch(target, version, &mut saved, spawn)
}

/// The system the paths and rules are of.
pub fn os() -> Os {
    Platform::current().os
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::Target;
    use crate::package::Kind;
    use crate::state::{Offer, Staged};
    use crate::tools::FakeTools;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    pub(crate) struct World {
        pub _dir: tempfile::TempDir,
        pub layout: Layout,
        pub target: Target,
        pub platform: Platform,
    }

    /// An install of `old` and a staged `new` (a program whose text says so).
    pub(crate) fn world(old: &str, new: &str) -> World {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path().join("updates"));
        let platform = Platform::parse("linux-x86_64").unwrap();
        let target = Target {
            path: dir.path().join("app/leon"),
            kind: Kind::File,
        };
        std::fs::create_dir_all(target.path.parent().unwrap()).unwrap();
        std::fs::write(&target.path, old).unwrap();
        let archive = layout.version_dir(new).join("a.tar.gz");
        std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
        std::fs::write(&archive, b"archive bytes").unwrap();
        let payload_dir = layout.payload_dir(new);
        std::fs::create_dir_all(&payload_dir).unwrap();
        std::fs::write(payload_dir.join("leon"), format!("program {new}")).unwrap();
        let (hash, _) = checksums::hash_file(&archive).unwrap();
        layout
            .save(&Saved {
                staged: Some(Staged {
                    offer: Offer {
                        version: new.to_owned(),
                        ..Offer::default()
                    },
                    archive: "a.tar.gz".into(),
                    sha256: checksums::hex(&hash),
                    identity: None,
                }),
                ..Saved::default()
            })
            .unwrap();
        World {
            _dir: dir,
            layout,
            target,
            platform,
        }
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn apply(w: &World, tools: &FakeTools, current: &str) -> Result<Applied, ApplyError> {
        let mut saved = w.layout.load();
        apply_staged(
            &w.layout,
            &mut saved,
            &w.target,
            tools,
            &w.platform,
            &Signature::none(),
            &v(current),
        )
    }

    #[test]
    fn a_staged_update_is_put_in_place_tried_and_marked_pending() {
        let w = world("old", "0.2.1");
        let tools = FakeTools::new("0.2.1");
        let applied = apply(&w, &tools, "0.2.0").unwrap();
        assert_eq!(applied.version, "0.2.1");
        assert_eq!(
            std::fs::read_to_string(&w.target.path).unwrap(),
            "program 0.2.1"
        );
        assert_eq!(std::fs::read_to_string(w.target.backup()).unwrap(), "old");
        assert_eq!(
            tools.probed.lock().unwrap().as_slice(),
            [w.target.executable()]
        );
        let pending = w.layout.pending().unwrap();
        assert_eq!(
            (
                pending.version.as_str(),
                pending.previous.as_str(),
                pending.starts
            ),
            ("0.2.1", "0.2.0", 0)
        );
        assert_eq!(w.layout.load().staged, None);
        assert!(!w.layout.version_dir("0.2.1").exists());
    }

    #[test]
    fn a_build_that_does_not_run_is_taken_out_at_once_and_refused() {
        let w = world("old", "0.2.1");
        let tools = FakeTools::new("0.2.1").failing("it exited with 1");
        let error = apply(&w, &tools, "0.2.0").unwrap_err();
        assert!(matches!(error, ApplyError::DidNotRun(_)), "{error:?}");
        assert_eq!(std::fs::read_to_string(&w.target.path).unwrap(), "old");
        assert!(!w.target.has_leftovers());
        assert_eq!(w.layout.pending(), None);
        let saved = w.layout.load();
        assert_eq!(saved.refused, ["0.2.1"]);
        assert!(saved.notice.unwrap().contains("did not run"));
        assert_eq!(saved.staged, None);
    }

    #[test]
    fn a_build_that_names_another_version_is_refused_too() {
        let w = world("old", "0.2.1");
        let tools = FakeTools::new("0.9.9");
        assert!(matches!(
            apply(&w, &tools, "0.2.0"),
            Err(ApplyError::DidNotRun(_))
        ));
        assert_eq!(std::fs::read_to_string(&w.target.path).unwrap(), "old");
    }

    #[test]
    fn a_changed_archive_or_a_missing_program_is_not_applied() {
        let w = world("old", "0.2.1");
        std::fs::write(w.layout.version_dir("0.2.1").join("a.tar.gz"), b"tampered").unwrap();
        assert!(matches!(
            apply(&w, &FakeTools::new("0.2.1"), "0.2.0"),
            Err(ApplyError::Untrusted(_))
        ));
        assert_eq!(std::fs::read_to_string(&w.target.path).unwrap(), "old");
        let w = world("old", "0.2.1");
        std::fs::remove_file(w.layout.payload_dir("0.2.1").join("leon")).unwrap();
        assert!(matches!(
            apply(&w, &FakeTools::new("0.2.1"), "0.2.0"),
            Err(ApplyError::Untrusted(_))
        ));
    }

    #[test]
    fn an_update_that_is_not_newer_is_not_applied() {
        let w = world("old", "0.2.1");
        assert!(matches!(
            apply(&w, &FakeTools::new("0.2.1"), "0.2.1"),
            Err(ApplyError::NotNewer)
        ));
        assert_eq!(
            w.layout.load().staged,
            None,
            "a download that is not newer is dropped"
        );
        let w = world("old", "0.2.1");
        assert!(matches!(
            apply(&w, &FakeTools::new("0.2.1"), "0.3.0"),
            Err(ApplyError::NotNewer)
        ));
    }

    #[test]
    fn a_signed_running_build_refuses_an_unsigned_update_at_the_door() {
        let w = world("old", "0.2.1");
        let mut saved = w.layout.load();
        let mac = Platform::parse("macos-aarch64").unwrap();
        // The unpacked "bundle" is a program file named for macOS' payload.
        std::fs::create_dir_all(w.layout.payload_dir("0.2.1").join("Leon.app")).unwrap();
        let result = apply_staged(
            &w.layout,
            &mut saved,
            &w.target,
            &FakeTools::new("0.2.1"),
            &mac,
            &Signature::good("TEAM1"),
            &v("0.2.0"),
        );
        assert!(
            matches!(result, Err(ApplyError::Untrusted(_))),
            "{result:?}"
        );
        assert_eq!(std::fs::read_to_string(&w.target.path).unwrap(), "old");
    }

    // ----- the start of the application -------------------------------------------------

    #[derive(Clone, Default)]
    struct Script {
        /// What each started child does: `up` writes "confirmed" (clears the
        /// pending mark) when it starts, `crash` exits badly, `hang` never ends.
        behaviours: Rc<RefCell<Vec<&'static str>>>,
        started: Rc<RefCell<Vec<(PathBuf, bool)>>>,
        layout: Option<Layout>,
    }

    struct Scripted {
        behaviour: &'static str,
        layout: Layout,
        released: bool,
    }

    impl Child for Scripted {
        fn ended(&mut self) -> std::io::Result<Option<bool>> {
            Ok(match self.behaviour {
                "crash" => Some(false),
                "quit-well" => Some(true),
                "up" => {
                    self.layout.clear_pending();
                    None
                }
                _ => None,
            })
        }
        fn release(&mut self) {
            self.released = true;
        }
    }

    impl Script {
        fn new(layout: &Layout, behaviours: &[&'static str]) -> Self {
            Self {
                behaviours: Rc::new(RefCell::new(behaviours.to_vec())),
                started: Rc::default(),
                layout: Some(layout.clone()),
            }
        }

        fn spawn(&self) -> impl Fn(&Path, bool) -> std::io::Result<Box<dyn Child>> + '_ {
            move |program: &Path, wait: bool| {
                self.started.borrow_mut().push((program.to_owned(), wait));
                let behaviour = if self.behaviours.borrow().len() > 1 {
                    self.behaviours.borrow_mut().remove(0)
                } else {
                    self.behaviours.borrow().first().copied().unwrap_or("hang")
                };
                Ok(Box::new(Scripted {
                    behaviour,
                    layout: self.layout.clone().unwrap(),
                    released: false,
                }) as Box<dyn Child>)
            }
        }
    }

    fn startup<'a>(
        w: &'a World,
        install: &'a Install,
        tools: &'a FakeTools,
        current: &str,
    ) -> Startup<'a> {
        Startup {
            current: v(current),
            layout: &w.layout,
            install,
            tools,
            platform: w.platform,
            watch: (Duration::from_millis(300), Duration::from_millis(5)),
        }
    }

    #[test]
    fn at_start_a_ready_update_is_applied_started_and_watched_until_it_is_up() {
        let w = world("old", "0.2.1");
        let install = Install::Updatable(w.target.clone());
        let tools = FakeTools::new("0.2.1");
        let script = Script::new(&w.layout, &["up"]);
        let outcome = startup(&w, &install, &tools, "0.2.0").run(true, &script.spawn());
        assert_eq!(outcome, Outcome::Exit(0));
        assert_eq!(
            std::fs::read_to_string(&w.target.path).unwrap(),
            "program 0.2.1"
        );
        assert_eq!(
            script.started.borrow().as_slice(),
            [(w.target.executable(), true)]
        );
        assert_eq!(w.layout.pending(), None);
    }

    #[test]
    fn at_start_a_new_build_that_crashes_is_undone_and_the_old_one_started() {
        let w = world("old", "0.2.1");
        let install = Install::Updatable(w.target.clone());
        let tools = FakeTools::new("0.2.1");
        let script = Script::new(&w.layout, &["crash"]);
        let outcome = startup(&w, &install, &tools, "0.2.0").run(true, &script.spawn());
        assert_eq!(outcome, Outcome::Exit(0));
        assert_eq!(std::fs::read_to_string(&w.target.path).unwrap(), "old");
        assert!(!w.target.has_leftovers());
        let started = script.started.borrow();
        assert_eq!(started.len(), 2);
        assert!(
            started[0].1 && !started[1].1,
            "the old one is started without waiting"
        );
        let saved = w.layout.load();
        assert_eq!(saved.refused, ["0.2.1"]);
        assert!(saved.notice.unwrap().contains("did not start"));
        assert_eq!(w.layout.pending(), None);
    }

    #[test]
    fn at_start_nothing_is_applied_unless_asked() {
        let w = world("old", "0.2.1");
        let install = Install::Updatable(w.target.clone());
        let tools = FakeTools::new("0.2.1");
        let script = Script::new(&w.layout, &["up"]);
        let outcome = startup(&w, &install, &tools, "0.2.0").run(false, &script.spawn());
        assert_eq!(outcome, Outcome::Continue);
        assert_eq!(std::fs::read_to_string(&w.target.path).unwrap(), "old");
        assert!(script.started.borrow().is_empty());
        assert!(w.layout.load().staged.is_some());
    }

    #[test]
    fn a_manual_install_is_left_alone() {
        let w = world("old", "0.2.1");
        let install = Install::Manual(crate::install::Why::Development);
        let tools = FakeTools::new("0.2.1");
        let script = Script::new(&w.layout, &["up"]);
        assert_eq!(
            startup(&w, &install, &tools, "0.2.0").run(true, &script.spawn()),
            Outcome::Continue
        );
        assert!(script.started.borrow().is_empty());
    }

    #[test]
    fn the_new_build_counts_its_starts_and_is_confirmed_when_up() {
        let w = world("old", "0.2.1");
        let install = Install::Updatable(w.target.clone());
        let tools = FakeTools::new("0.2.1");
        apply(&w, &tools, "0.2.0").unwrap();
        let script = Script::new(&w.layout, &["hang"]);
        let new = startup(&w, &install, &tools, "0.2.1");
        assert_eq!(new.run(true, &script.spawn()), Outcome::Continue);
        assert_eq!(w.layout.pending().unwrap().starts, 1);
        assert!(
            w.target.has_backup(),
            "the old one is kept until the new one is up"
        );
        assert!(new.confirm_started());
        assert_eq!(w.layout.pending(), None);
        assert!(!w.target.has_leftovers());
        assert!(
            !new.confirm_started(),
            "only the first start after an update confirms"
        );
    }

    #[test]
    fn a_new_build_that_keeps_failing_takes_itself_out() {
        let w = world("old", "0.2.1");
        let install = Install::Updatable(w.target.clone());
        let tools = FakeTools::new("0.2.1");
        apply(&w, &tools, "0.2.0").unwrap();
        let script = Script::new(&w.layout, &["hang"]);
        let new = startup(&w, &install, &tools, "0.2.1");
        for _ in 0..MAX_UNCONFIRMED_STARTS {
            assert_eq!(new.run(false, &script.spawn()), Outcome::Continue);
        }
        assert_eq!(new.run(false, &script.spawn()), Outcome::Exit(0));
        assert_eq!(std::fs::read_to_string(&w.target.path).unwrap(), "old");
        assert_eq!(
            script.started.borrow().as_slice(),
            [(w.target.executable(), false)]
        );
        let saved = w.layout.load();
        assert_eq!(saved.refused, ["0.2.1"]);
        assert!(saved.notice.unwrap().contains("did not come up"));
    }

    #[test]
    fn the_old_build_started_again_forgets_a_mark_that_is_not_its_own() {
        let w = world("old", "0.2.1");
        let install = Install::Updatable(w.target.clone());
        let tools = FakeTools::new("0.2.0");
        w.layout
            .set_pending(&Pending {
                version: "0.2.1".into(),
                previous: "0.2.0".into(),
                starts: 2,
            })
            .unwrap();
        std::fs::write(w.target.backup(), "leftover").unwrap();
        let script = Script::new(&w.layout, &["up"]);
        assert_eq!(
            startup(&w, &install, &tools, "0.2.0").run(false, &script.spawn()),
            Outcome::Continue
        );
        assert_eq!(w.layout.pending(), None);
        assert!(
            !w.target.has_leftovers(),
            "what an earlier update left is cleaned"
        );
    }

    #[test]
    fn restart_to_update_starts_the_new_build_and_goes_back_when_it_does_not_come_up() {
        let w = world("old", "0.2.1");
        let install = Install::Updatable(w.target.clone());
        let tools = FakeTools::new("0.2.1");
        apply(&w, &tools, "0.2.0").unwrap();
        let script = Script::new(&w.layout, &["crash"]);
        let outcome = hand_over(
            &startup(&w, &install, &tools, "0.2.0"),
            "0.2.1",
            &script.spawn(),
        );
        assert_eq!(outcome, Outcome::Exit(0));
        assert_eq!(std::fs::read_to_string(&w.target.path).unwrap(), "old");
        assert_eq!(w.layout.load().refused, ["0.2.1"]);
        assert_eq!(script.started.borrow().len(), 2);
    }

    #[test]
    fn restart_to_update_leaves_the_new_build_running_when_it_comes_up() {
        let w = world("old", "0.2.1");
        let install = Install::Updatable(w.target.clone());
        let tools = FakeTools::new("0.2.1");
        apply(&w, &tools, "0.2.0").unwrap();
        let script = Script::new(&w.layout, &["up"]);
        let outcome = hand_over(
            &startup(&w, &install, &tools, "0.2.0"),
            "0.2.1",
            &script.spawn(),
        );
        assert_eq!(outcome, Outcome::Exit(0));
        assert_eq!(
            std::fs::read_to_string(&w.target.path).unwrap(),
            "program 0.2.1"
        );
        assert_eq!(script.started.borrow().len(), 1);
    }

    #[test]
    fn a_new_build_that_ends_well_before_it_confirms_is_not_undone() {
        let w = world("old", "0.2.1");
        let install = Install::Updatable(w.target.clone());
        let tools = FakeTools::new("0.2.1");
        apply(&w, &tools, "0.2.0").unwrap();
        let script = Script::new(&w.layout, &["quit-well"]);
        assert_eq!(
            hand_over(
                &startup(&w, &install, &tools, "0.2.0"),
                "0.2.1",
                &script.spawn()
            ),
            Outcome::Exit(0)
        );
        assert_eq!(
            std::fs::read_to_string(&w.target.path).unwrap(),
            "program 0.2.1"
        );
        assert_eq!(w.layout.pending(), None);
    }

    #[test]
    fn waiting_for_a_parent_returns_at_once_outside_a_hand_over() {
        let before = Instant::now();
        wait_for_parent(Duration::from_secs(5));
        assert!(before.elapsed() < Duration::from_secs(1));
    }
}

//! Tests of the "Connect a machine" screen: how it opens, how the form
//! splits what is typed, what each scripted outcome of the test shows, that
//! the host key is only trusted after a confirmation, what saving does and
//! that an offline machine explains itself. Nothing here touches the real
//! `~/.ssh`, the network or `ssh`: the runner is scripted and `~/.ssh` is in
//! memory.

use super::*;
use crate::connect::Field;
use crate::ui::connect::{Phase, Stop};
use leon_remote::connect::{CheckId, CheckState, KeyFile, SshDir};
use leon_remote::DiagnosisKind;
use std::sync::Mutex;

/// The user's `~/.ssh`, in memory: what it holds, and what was appended to
/// `known_hosts`.
#[derive(Default)]
pub(super) struct FakeSsh {
    pub config: Mutex<String>,
    pub known: Mutex<String>,
    pub keys: Mutex<Vec<KeyFile>>,
    pub appended: Mutex<Vec<String>>,
}

impl SshDir for FakeSsh {
    fn config(&self) -> Option<String> {
        Some(self.config.lock().unwrap().clone())
    }
    fn known_hosts(&self) -> Option<String> {
        Some(self.known.lock().unwrap().clone())
    }
    fn public_keys(&self) -> Vec<KeyFile> {
        self.keys.lock().unwrap().clone()
    }
    fn append_known_hosts(&self, lines: &[String]) -> std::io::Result<()> {
        self.appended.lock().unwrap().extend(lines.iter().cloned());
        Ok(())
    }
}

const SUCCESS: &str = "os=Linux\narch=x86_64\nhome=/home/dev\ntool=git=/usr/bin/git\ntool=claude=/home/dev/.local/bin/claude\ntool=codex=/usr/bin/codex\n";

fn open_screen(h: &Harness, cx: &mut TestAppContext) {
    h.press("ctrl-shift-m", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Connect);
}

/// Types into the field that has the keyboard.
fn type_in(h: &Harness, text: &str, cx: &mut TestAppContext) {
    h.type_text(text, cx);
}

/// Fills the name and the host, ending in the host field.
fn fill(h: &Harness, name: &str, host: &str, cx: &mut TestAppContext) {
    type_in(h, name, cx);
    h.press("tab", cx);
    type_in(h, host, cx);
}

fn form(h: &Harness, cx: &mut TestAppContext) -> crate::connect::Form {
    h.shell(cx, |s| s.connect_ui.form.clone())
}

fn failure(h: &Harness, cx: &mut TestAppContext) -> Option<DiagnosisKind> {
    h.shell(cx, |s| {
        s.connect_ui
            .checklist
            .as_ref()
            .and_then(|list| list.failure().map(|d| d.kind))
    })
}

fn state(h: &Harness, id: CheckId, cx: &mut TestAppContext) -> CheckState {
    h.shell(cx, |s| {
        s.connect_ui
            .checklist
            .as_ref()
            .map(|list| list.get(id).state.clone())
            .expect("a checklist")
    })
}

#[gpui_kit::test]
fn the_chord_opens_the_screen_and_escape_gives_the_keyboard_back(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let before = h.cursor_row(cx);
    open_screen(&h, cx);
    assert!(h.shows("connect", cx));
    assert!(h.shows("connect-intro", cx));
    assert!(h.shows("connect-field-name", cx) && h.shows("connect-field-host", cx));
    assert!(h.shows("connect-command", cx));
    // The form is visible without scrolling at the default size.
    assert!(h.shows("connect-test", cx) && h.shows("connect-save", cx));
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert!(!h.shows("connect", cx));
    // The tree has the keyboard again: `j` moves its cursor.
    h.press("j", cx);
    assert_ne!(h.cursor_row(cx), before);
}

#[gpui_kit::test]
fn every_way_in_opens_the_same_screen(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    // The row after the tree, with its chord.
    assert!(h.shows("sidebar-connect", cx));
    h.mouse_on(
        "sidebar-connect".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Connect);
    h.press("escape", cx);
    // The palette.
    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">connect", cx);
    assert!(h
        .palette_titles(cx)
        .contains(&"Connect a machine…".to_owned()));
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Connect);
    h.press("escape", cx);
    // The empty state says remote computers can be added.
    assert!(h.shows("main-empty-remote", cx));
}

#[gpui_kit::test]
fn fields_are_reached_with_tab_and_the_host_field_splits_live(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_screen(&h, cx);
    assert_eq!(h.shell(cx, |s| s.connect_ui.stop), Stop::Field(Field::Name));
    type_in(&h, "box", cx);
    h.press("tab", cx);
    assert_eq!(h.shell(cx, |s| s.connect_ui.stop), Stop::Field(Field::Host));
    type_in(&h, "dev@box:2222", cx);
    let split = form(&h, cx);
    assert_eq!(
        (
            split.name.as_str(),
            split.host.as_str(),
            split.user.as_str(),
            split.port.as_str()
        ),
        ("box", "box", "dev", "2222")
    );
    h.press("shift-tab", cx);
    assert_eq!(h.shell(cx, |s| s.connect_ui.stop), Stop::Field(Field::Name));
    // The command on screen follows what was typed.
    let line = h.shell(cx, |s| s.connect_ui.shown_ssh_line());
    assert!(line.contains("-p 2222") && line.contains("-l dev") && line.contains("-- box"));
}

#[gpui_kit::test]
fn an_ssh_url_is_split_too(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_screen(&h, cx);
    h.press("tab", cx);
    type_in(&h, "ssh://dev@box:2200", cx);
    let split = form(&h, cx);
    assert_eq!(
        (
            split.host.as_str(),
            split.user.as_str(),
            split.port.as_str()
        ),
        ("box", "dev", "2200")
    );
}

#[gpui_kit::test]
fn problems_are_specific_and_nothing_runs_until_they_are_fixed(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_screen(&h, cx);
    h.press("ctrl-enter", cx);
    assert!(h.shows("connect-problem-host", cx));
    assert!(h.runner.calls().is_empty());
    assert_eq!(h.shell(cx, |s| s.connect_ui.stop), Stop::Field(Field::Host));
    h.press("ctrl-s", cx);
    assert!(h.shows("connect-problem-name", cx));
    assert!(h.runner.calls().is_empty());
    assert_eq!(h.store.machines().unwrap().len(), 2, "nothing was saved");
}

#[gpui_kit::test]
fn a_full_success_lists_each_check_and_the_agents(cx: &mut TestAppContext) {
    let runner = ScriptedRunner::new()
        .reply(Output::ok(""))
        .reply(Output::ok(SUCCESS));
    let h = open(cx, runner);
    open_screen(&h, cx);
    fill(&h, "box", "dev@box", cx);
    h.press("ctrl-enter", cx);
    assert!(h.shows("connect-checklist", cx));
    for id in ["reach", "hostkey", "login", "shell", "git", "agents"] {
        assert!(h.shows_dynamic(format!("connect-check-{id}"), cx), "{id}");
    }
    let (connected, agents, git) = h.shell(cx, |s| {
        let list = s.connect_ui.checklist.as_ref().unwrap();
        (
            list.connected(),
            list.get(CheckId::Agents).note.clone(),
            list.get(CheckId::Git).note.clone(),
        )
    });
    assert!(connected);
    assert!(agents.contains("claude /home/dev/.local/bin/claude"));
    assert!(agents.contains("opencode: https://opencode.ai"), "{agents}");
    assert_eq!(git, "/usr/bin/git");
    assert_eq!(state(&h, CheckId::Shell, cx), CheckState::Passed);
    // The test is the one command Leon showed.
    let first = &h.runner.calls()[0];
    assert_eq!(first.program, "ssh");
    assert!(first.args.contains(&"BatchMode=yes".to_owned()));
}

#[gpui_kit::test]
fn a_refused_key_shows_the_copy_id_fix_and_the_key_tried(cx: &mut TestAppContext) {
    let runner = ScriptedRunner::new().reply(Output::failed(
        255,
        "dev@box: Permission denied (publickey).\n",
    ));
    let h = open(cx, runner);
    *h.ssh.keys.lock().unwrap() = vec![KeyFile {
        public: "id_ed25519.pub".into(),
        private: "/h/.ssh/id_ed25519".into(),
    }];
    open_screen(&h, cx);
    fill(&h, "box", "dev@box", cx);
    h.press("ctrl-enter", cx);
    assert_eq!(failure(&h, cx), Some(DiagnosisKind::PublicKeyRefused));
    assert_eq!(state(&h, CheckId::Reach, cx), CheckState::Passed);
    assert_eq!(state(&h, CheckId::HostKey, cx), CheckState::Passed);
    assert_eq!(state(&h, CheckId::Login, cx), CheckState::Failed);
    assert_eq!(state(&h, CheckId::Shell, cx), CheckState::Skipped);
    assert!(h.shows("connect-diagnosis", cx) && h.shows("connect-fix", cx));
    assert!(h.shows("connect-copyid", cx));
    let fix = h.shell(cx, |s| {
        s.connect_ui
            .checklist
            .as_ref()
            .unwrap()
            .failure()
            .unwrap()
            .fix
            .clone()
    });
    assert!(fix.contains("ssh-copy-id"));
    // The raw output stays available under Details.
    assert!(!h.shows("connect-raw", cx));
    h.mouse_on(
        "connect-details".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(h.shows("connect-raw", cx));
    // The line is the one typed, and it copies.
    h.press_chord("cmd-shift-c", "ctrl-shift-c", cx);
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("ssh-copy-id dev@box"));
}

#[gpui_kit::test]
fn an_unknown_host_key_is_trusted_only_after_a_confirmation(cx: &mut TestAppContext) {
    let runner = ScriptedRunner::new()
        .reply(Output::failed(255, "Host key verification failed.\n"))
        .reply(Output::ok("box ssh-ed25519 YWJj\n"))
        // After trusting: the test runs again.
        .reply(Output::ok(""))
        .reply(Output::ok(SUCCESS));
    let h = open(cx, runner);
    open_screen(&h, cx);
    fill(&h, "box", "dev@box", cx);
    h.press("ctrl-enter", cx);
    assert_eq!(failure(&h, cx), Some(DiagnosisKind::HostKeyUnknown));
    assert!(h.shows("connect-fingerprint", cx));
    assert!(h.shows("connect-trust", cx));
    assert!(h.ssh.appended.lock().unwrap().is_empty());

    // Asking is not trusting; Escape backs out and writes nothing.
    h.mouse_on("connect-trust".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert!(h.shows("connect-trust-ask", cx));
    assert!(h.ssh.appended.lock().unwrap().is_empty());
    h.press("escape", cx);
    assert!(!h.shows("connect-trust-ask", cx));
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Connect);
    assert!(h.ssh.appended.lock().unwrap().is_empty());

    // Confirming writes exactly what the computer showed, then tests again.
    h.mouse_on("connect-trust".to_owned(), gpui_kit::MouseButton::Left, cx);
    h.mouse_on(
        "connect-trust-yes".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(
        *h.ssh.appended.lock().unwrap(),
        ["box ssh-ed25519 YWJj".to_owned()]
    );
    assert!(h.shell(cx, |s| s.connect_ui.checklist.as_ref().unwrap().connected()));
}

#[gpui_kit::test]
fn a_changed_host_key_is_explained_and_cannot_be_overridden(cx: &mut TestAppContext) {
    let runner = ScriptedRunner::new().reply(Output::failed(
        255,
        "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\nHost key verification failed.\n",
    ));
    let h = open(cx, runner);
    open_screen(&h, cx);
    fill(&h, "box", "dev@box", cx);
    h.press("ctrl-enter", cx);
    assert_eq!(failure(&h, cx), Some(DiagnosisKind::HostKeyChanged));
    assert!(h.shows("connect-diagnosis", cx));
    assert!(!h.shows("connect-trust", cx), "no way to override");
    assert!(!h.shell(cx, |s| s.connect_ui.offers_trust()));
    assert_eq!(h.runner.calls().len(), 1, "no key was fetched for trusting");
    // Even the keyboard has no such stop.
    assert!(!h.shell(cx, |s| s.connect_ui.stops().contains(&Stop::Trust)));
}

#[gpui_kit::test]
fn a_refused_connection_stops_at_the_first_line(cx: &mut TestAppContext) {
    let runner = ScriptedRunner::new().reply(Output::failed(
        255,
        "ssh: connect to host box port 22: Connection refused\n",
    ));
    let h = open(cx, runner);
    open_screen(&h, cx);
    fill(&h, "box", "box", cx);
    h.press("ctrl-enter", cx);
    assert_eq!(failure(&h, cx), Some(DiagnosisKind::ConnectionRefused));
    assert_eq!(state(&h, CheckId::Reach, cx), CheckState::Failed);
    assert_eq!(state(&h, CheckId::Login, cx), CheckState::Skipped);
    let fix = h.shell(cx, |s| {
        s.connect_ui
            .checklist
            .as_ref()
            .unwrap()
            .failure()
            .unwrap()
            .fix
            .clone()
    });
    assert!(fix.contains("Remote Login") && fix.contains("openssh-server"));
}

#[gpui_kit::test]
fn saving_tests_first_then_adds_the_machine_and_says_what_next(cx: &mut TestAppContext) {
    let runner = ScriptedRunner::new()
        .reply(Output::ok(""))
        .reply(Output::ok(SUCCESS))
        // The search for repositories after saving.
        .reply(Output::ok("/home/dev/api/.git\n/home/dev/web/.git\n"));
    let h = open(cx, runner);
    open_screen(&h, cx);
    fill(&h, "box", "dev@box:2200", cx);
    h.press("ctrl-s", cx);
    assert_eq!(h.shell(cx, |s| s.connect_ui.phase), Phase::Next);
    assert!(h.shows("connect-next", cx));
    let saved = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|m| m.name == "box")
        .expect("saved");
    assert_eq!(
        leon_remote::connect::Target::of(&saved),
        Some(leon_remote::connect::Target {
            host: "box".into(),
            user: Some("dev".into()),
            port: Some(2200),
            identity: None
        })
    );
    // It is in the tree, selected, and online with what the test found.
    assert_eq!(h.cursor_row(cx), "machine:box");
    assert!(matches!(
        h.engine.machine_state(&saved.id),
        MachineState::Online(Some(_))
    ));
    // Add a project there: the folders found on the machine are offered.
    h.mouse_on(
        "connect-add-project".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    let titles = h.palette_titles(cx);
    assert!(titles.contains(&"/home/dev/api".to_owned()), "{titles:?}");
    assert!(titles.contains(&"/home/dev/web".to_owned()));
}

#[gpui_kit::test]
fn a_failed_test_can_still_be_saved_anyway(cx: &mut TestAppContext) {
    let runner = ScriptedRunner::new().reply(Output::failed(
        255,
        "ssh: connect to host box port 22: Operation timed out\n",
    ));
    let h = open(cx, runner);
    open_screen(&h, cx);
    fill(&h, "box", "box", cx);
    h.press("ctrl-s", cx);
    // The test failed, so nothing was saved yet and the button says so.
    assert_eq!(h.shell(cx, |s| s.connect_ui.phase), Phase::Form);
    assert_eq!(failure(&h, cx), Some(DiagnosisKind::TimedOut));
    assert!(h.shell(cx, |s| s.connect_ui.last_test_failed()));
    assert!(h.store.machines().unwrap().iter().all(|m| m.name != "box"));
    h.press("ctrl-s", cx);
    assert_eq!(h.shell(cx, |s| s.connect_ui.phase), Phase::Next);
    assert!(h.store.machines().unwrap().iter().any(|m| m.name == "box"));
}

#[gpui_kit::test]
fn changing_what_was_tested_forgets_the_result(cx: &mut TestAppContext) {
    let runner = ScriptedRunner::new().reply(Output::failed(255, "Connection refused\n"));
    let h = open(cx, runner);
    open_screen(&h, cx);
    fill(&h, "box", "box", cx);
    h.press("ctrl-enter", cx);
    assert!(h.shows("connect-checklist", cx));
    // Back to the form with the keyboard: Test, Details, then the fields
    // upwards; the card scrolls to the top by itself.
    for _ in 0..6 {
        h.press("shift-tab", cx);
    }
    assert_eq!(h.shell(cx, |s| s.connect_ui.stop), Stop::Field(Field::Host));
    type_in(&h, "2", cx);
    assert!(!h.shows("connect-checklist", cx));
}

#[gpui_kit::test]
fn an_offline_machine_explains_itself_and_can_be_tested_again(cx: &mut TestAppContext) {
    let runner = ScriptedRunner::new()
        .reply(Output::failed(
            255,
            "ssh: connect to host build.example port 22: Connection refused\n",
        ))
        // "Test again" works this time.
        .reply(Output::ok(""))
        .reply(Output::ok(SUCCESS));
    let h = open(cx, runner);
    let remote = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|m| m.name == "build box")
        .unwrap();
    h.runtime
        .block_on(h.engine.run(crate::engine::Op::Probe(remote.id.clone())));
    h.settle(cx);
    assert!(matches!(
        h.engine.machine_state(&remote.id),
        MachineState::Offline(_)
    ));
    // Selecting the machine shows why.
    let at = h.row_of(NodeId::Machine(remote.id.clone()), cx).unwrap();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _cx| {
            shell.cursor = Some(at);
        });
    });
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Connect);
    assert_eq!(h.shell(cx, |s| s.connect_ui.phase), Phase::Why);
    assert!(h.shows("connect-why", cx));
    assert_eq!(failure(&h, cx), Some(DiagnosisKind::ConnectionRefused));
    assert!(h.shows("connect-fix", cx));
    // Test again.
    h.press("ctrl-enter", cx);
    assert!(h.shell(cx, |s| s.connect_ui.checklist.as_ref().unwrap().connected()));
    assert!(matches!(
        h.engine.machine_state(&remote.id),
        MachineState::Online(Some(_))
    ));
}

#[gpui_kit::test]
fn the_machines_menu_offers_the_explanation_and_the_edit(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let remote = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|m| m.name == "build box")
        .unwrap();
    let at = h.row_of(NodeId::Machine(remote.id), cx).unwrap();
    let labels = h.shell(cx, |s| s.menu_for_test(at));
    for wanted in ["Why is it offline?", "Edit machine…", "Connect a machine…"] {
        assert!(labels.contains(&wanted.to_owned()), "{labels:?}");
    }
    let local = h.shell(cx, |s| s.menu_for_test(0));
    assert!(!local.contains(&"Why is it offline?".to_owned()));
}

#[gpui_kit::test]
fn editing_a_machine_opens_the_same_screen_filled_in_and_saves_in_place(cx: &mut TestAppContext) {
    let runner = ScriptedRunner::new()
        .reply(Output::ok(""))
        .reply(Output::ok(SUCCESS));
    let h = open(cx, runner);
    let remote = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|m| m.name == "build box")
        .unwrap();
    let at = h.row_of(NodeId::Machine(remote.id.clone()), cx).unwrap();
    cx.update(|cx| h.shell.update(cx, |shell, _| shell.cursor = Some(at)));
    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">edit a machine", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Connect);
    let filled = form(&h, cx);
    assert_eq!(
        (
            filled.name.as_str(),
            filled.host.as_str(),
            filled.user.as_str()
        ),
        ("build box", "build.example", "dev")
    );
    assert_eq!(
        h.shell(cx, |s| s.connect_ui.editing.clone()),
        Some(remote.id.clone())
    );
    // Rename it and save: the same machine, not a second one.
    type_in(&h, " 2", cx);
    h.press("ctrl-s", cx);
    assert_eq!(h.shell(cx, |s| s.connect_ui.phase), Phase::Next);
    let machines = h.store.machines().unwrap();
    assert_eq!(machines.len(), 2);
    assert_eq!(h.store.machine(&remote.id).unwrap().name, "build box 2");
}

#[gpui_kit::test]
fn hosts_from_ssh_config_and_known_hosts_are_suggested_by_name_only(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    *h.ssh.config.lock().unwrap() =
        "Host buildhost\n  HostName 10.0.0.2\nHost *\n  User x\n".into();
    *h.ssh.known.lock().unwrap() =
        "buildbox ssh-ed25519 AAAASECRET\n|1|hashed= ssh-rsa AAAAx\n".into();
    open_screen(&h, cx);
    h.press("tab", cx);
    type_in(&h, "build", cx);
    let suggestions = h.shell(cx, |s| s.connect_ui.suggestions(Field::Host));
    assert_eq!(suggestions, ["buildhost", "buildbox"]);
    assert!(h.shows("connect-suggestion-0", cx));
    // Arrow and Enter take one.
    h.press("down", cx);
    h.press("down", cx);
    h.press("enter", cx);
    assert_eq!(form(&h, cx).host, "buildbox");
}

#[gpui_kit::test]
fn a_key_is_chosen_with_the_file_dialog_or_from_the_keys_found(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    *h.ssh.keys.lock().unwrap() = vec![KeyFile {
        public: "id_ed25519.pub".into(),
        private: "/h/.ssh/id_ed25519".into(),
    }];
    *h.key_answer.borrow_mut() = Picked::Folder("/k/work_key".into());
    open_screen(&h, cx);
    h.mouse_on(
        "connect-choose-key".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(form(&h, cx).identity, "/k/work_key");
    // The copy-id line follows the key.
    let line = h.shell(cx, |s| s.connect_ui.form.shown_target("me").copy_id_line());
    assert!(line.contains("-i /k/work_key.pub"));
}

#[gpui_kit::test]
fn the_explanation_is_per_platform_and_collapses_into_rows(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_screen(&h, cx);
    assert!(!h.shows("connect-how-body-0", cx));
    h.mouse_on("connect-how-0".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert!(h.shows("connect-how-body-0", cx));
    h.mouse_on(
        "connect-platform-0".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(
        h.shell(cx, |s| s.connect_ui.platform),
        crate::connect::Platform::Mac
    );
    h.mouse_on(
        "connect-platform-2".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(
        h.shell(cx, |s| s.connect_ui.platform),
        crate::connect::Platform::Windows
    );
    // One row at a time.
    h.mouse_on("connect-how-1".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert!(!h.shows("connect-how-body-0", cx) && h.shows("connect-how-body-1", cx));
}

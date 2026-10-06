//! Updates in the window: the footer's item for each state, the commands,
//! the confirmation that names what a restart closes, the modes, and that a
//! development build asks GitHub nothing. GitHub is a script and the install
//! is a temporary folder (`leon_update::testing`).

use super::*;
use crate::schema::{self, Value};
use crate::updates::{Mode, Service};
use leon_update::testing::Fixture;
use leon_update::{Install, Offer, Stage, State, Trust, Why};

fn offer(version: &str, notes: &str) -> Offer {
    Offer {
        version: version.to_owned(),
        notes: notes.to_owned(),
        page: format!("https://github.com/zavudev/leon/releases/tag/v{version}"),
        size: 1000,
        prerelease: false,
    }
}

/// Opens the window with an updater over `fixture`; the updater thinks it is
/// the running version, so "the latest" and "newer" mean what they say.
fn open_with_updates(
    cx: &mut TestAppContext,
    fixture: &Fixture,
    install: Install,
) -> (Harness, Arc<Service>) {
    let updater = fixture.updater_with(crate::product::VERSION, install);
    let slot = std::cell::RefCell::new(None);
    let h = open_core(
        cx,
        ScriptedRunner::new(),
        None,
        Picked::Cancelled,
        |_| {},
        |handle| {
            let service = Service::with_updater(updater, handle.clone());
            *slot.borrow_mut() = Some(service.clone());
            Some(service)
        },
    );
    let service = slot.into_inner().unwrap();
    (h, service)
}

fn set(h: &Harness, cx: &mut TestAppContext, key: &str, value: Value) {
    let def = schema::find(key).unwrap_or_else(|| panic!("{key} is not a setting"));
    cx.update(|cx| settings::set_value(cx, def, value));
    h.settle(cx);
}

/// Runs a command from the palette, by typing its name.
fn run(h: &Harness, cx: &mut TestAppContext, name: &str) {
    h.press_chord("cmd-shift-p", "ctrl-shift-p", cx);
    h.type_text(name, cx);
    h.press("enter", cx);
    h.settle(cx);
}

fn newer() -> String {
    // A version that is above whatever this build is.
    let mut v = leon_update::Version::parse(crate::product::VERSION).unwrap();
    v.minor += 1;
    v.to_string()
}

#[gpui_kit::test]
fn there_is_no_update_item_without_an_update(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    assert!(!h.shows("update-item", cx));
    let fixture = Fixture::new();
    let (h, service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    assert!(!h.shows("update-item", cx), "nothing is known yet");
    service.updater().publish_for_test(State::UpToDate);
    h.settle(cx);
    assert!(!h.shows("update-item", cx));
}

#[gpui_kit::test]
fn the_footer_item_follows_the_state_of_the_update(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (h, service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    let updater = service.updater().clone();
    let version = newer();
    let states = [
        (State::Available(offer(&version, "")), false, None),
        (
            State::Downloading {
                offer: offer(&version, ""),
                done: 500,
                total: 1000,
            },
            true,
            Some("Downloading"),
        ),
        (
            State::Ready {
                offer: offer(&version, ""),
                trust: Trust::Unsigned,
            },
            false,
            Some("Restart to update"),
        ),
        (
            State::Failed {
                reason: "disk full".into(),
                retry_at: None,
                stage: Stage::Download,
                offer: None,
            },
            false,
            Some("Update failed"),
        ),
    ];
    for (state, progress, word) in states {
        updater.publish_for_test(state.clone());
        h.settle(cx);
        assert!(h.shows("update-item", cx), "{state:?}");
        assert_eq!(h.shows("update-progress", cx), progress, "{state:?}");
        let text = crate::updates::footer_item(&state).unwrap().text;
        if let Some(word) = word {
            assert!(text.starts_with(word), "{text}");
        }
    }
    updater.publish_for_test(State::Ready {
        offer: offer("0.2.1", ""),
        trust: Trust::Unsigned,
    });
    assert_eq!(
        crate::updates::footer_item(&service.snapshot().state)
            .unwrap()
            .text,
        "Restart to update · 0.2.1"
    );
    // A failed check is not shown in the footer: it is for the status line.
    updater.publish_for_test(State::Failed {
        reason: "offline".into(),
        retry_at: None,
        stage: Stage::Check,
        offer: None,
    });
    h.settle(cx);
    assert!(!h.shows("update-item", cx));
}

#[gpui_kit::test]
fn check_for_updates_says_that_leon_is_the_latest(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.announce(crate::product::VERSION, "");
    let (h, _service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    run(&h, cx, "check for updates");
    assert_eq!(
        h.status(),
        format!("Leon {} is the latest.", crate::product::VERSION)
    );
    assert!(!h.shows("update-item", cx));
}

#[gpui_kit::test]
fn a_newer_release_is_announced_and_in_notify_mode_waits_for_the_person(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let version = newer();
    fixture.announce(&version, "## New\n\n- a thing");
    let (h, service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    set(&h, cx, "updates_mode", Value::Text("notify".into()));
    run(&h, cx, "check for updates");
    assert!(
        h.status().contains(&format!("Leon {version} is available")),
        "{}",
        h.status()
    );
    assert!(h.status().contains("release notes"));
    assert!(matches!(service.snapshot().state, State::Available(_)));
    assert!(h.shows("update-item", cx));
    // Nothing was downloaded: the person decides.
    assert!(h
        .http_calls(&fixture)
        .iter()
        .all(|call| !call.contains("FETCH")));
}

#[gpui_kit::test]
fn the_release_notes_open_in_an_overlay_and_close_with_escape(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (h, service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    // Nothing on offer: it says so instead of opening.
    run(&h, cx, "show release notes");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert!(h.status().contains("no release notes"), "{}", h.status());
    service.updater().publish_for_test(State::Available(offer(
        &newer(),
        "## What's new\n\n- [a link](https://evil.example/)\n- ![img](https://evil.example/a.png)\n\n<script>x</script>",
    )));
    h.settle(cx);
    run(&h, cx, "show release notes");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Notes);
    assert!(h.shows("release-notes", cx));
    assert!(h.shows("notes-title", cx));
    assert!(h.shows("notes-update", cx) && h.shows("notes-skip", cx) && h.shows("notes-page", cx));
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn clicking_the_footer_item_opens_the_notes_of_an_offer(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (h, service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    service
        .updater()
        .publish_for_test(State::Available(offer(&newer(), "notes")));
    h.settle(cx);
    h.click("update-item".to_owned(), cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Notes);
}

#[gpui_kit::test]
fn skipping_a_version_stops_the_offer(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let version = newer();
    fixture.announce(&version, "");
    let (h, service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    set(&h, cx, "updates_mode", Value::Text("notify".into()));
    run(&h, cx, "check for updates");
    assert!(h.shows("update-item", cx));
    run(&h, cx, "skip this version");
    assert!(
        h.status().contains(&format!("Leon {version} is skipped")),
        "{}",
        h.status()
    );
    assert_eq!(service.snapshot().state, State::UpToDate);
    assert!(!h.shows("update-item", cx));
    run(&h, cx, "skip this version");
    assert!(h.status().contains("no version on offer"), "{}", h.status());
}

#[gpui_kit::test]
fn the_download_page_is_the_releases_page_of_the_offer(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (h, service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    run(&h, cx, "open the download page");
    assert_eq!(
        h.urls.borrow().as_slice(),
        ["https://github.com/zavudev/leon/releases"]
    );
    let version = newer();
    service
        .updater()
        .publish_for_test(State::Available(offer(&version, "")));
    h.settle(cx);
    run(&h, cx, "open the download page");
    assert_eq!(
        h.urls.borrow().last().unwrap(),
        &format!("https://github.com/zavudev/leon/releases/tag/v{version}")
    );
}

#[gpui_kit::test]
fn an_install_leon_cannot_replace_offers_the_download_page(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let version = newer();
    fixture.announce(&version, "");
    let (h, service) = open_with_updates(cx, &fixture, Install::Manual(Why::NotWritable));
    run(&h, cx, "check for updates");
    assert!(matches!(service.snapshot().state, State::Manual { .. }));
    assert!(
        h.status().contains("Open the download page"),
        "{}",
        h.status()
    );
    run(&h, cx, "restart to update");
    assert_eq!(
        h.urls.borrow().len(),
        1,
        "restart to update opens the page here"
    );
}

// ----- restart to update -------------------------------------------------------------------------------

fn ready_window(cx: &mut TestAppContext) -> (Harness, Arc<Service>, Fixture, String) {
    let fixture = Fixture::new();
    let version = newer();
    fixture.stage(&version, "notes");
    let (h, service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    assert!(
        matches!(service.snapshot().state, State::Ready { .. }),
        "{:?}",
        service.snapshot().state
    );
    (h, service, fixture, version)
}

fn quits_counted(h: &Harness, cx: &mut TestAppContext) -> Rc<std::cell::Cell<usize>> {
    let quits = Rc::new(std::cell::Cell::new(0));
    let counted = quits.clone();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.quit = Rc::new(move |_| counted.set(counted.get() + 1));
        })
    });
    quits
}

#[gpui_kit::test]
fn restart_to_update_asks_first_and_cancelling_changes_nothing(cx: &mut TestAppContext) {
    let (h, service, fixture, version) = ready_window(cx);
    let quits = quits_counted(&h, cx);
    run(&h, cx, "restart to update");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    let titles = h.palette_titles(cx);
    assert_eq!(
        titles,
        [
            format!("Restart Leon and update to {version}"),
            "Cancel".to_owned()
        ]
    );
    h.press("down", cx);
    h.press("enter", cx);
    assert_eq!(quits.get(), 0);
    assert_eq!(service.take_restart(), None);
    assert_eq!(
        std::fs::read_to_string(&fixture.target.path).unwrap(),
        "program old"
    );
    assert!(matches!(service.snapshot().state, State::Ready { .. }));
}

#[gpui_kit::test]
fn confirming_the_restart_installs_the_update_and_quits_for_the_hand_over(cx: &mut TestAppContext) {
    let (h, service, fixture, version) = ready_window(cx);
    let quits = quits_counted(&h, cx);
    run(&h, cx, "restart to update");
    h.press("enter", cx);
    assert_eq!(quits.get(), 1);
    assert_eq!(service.take_restart().as_deref(), Some(version.as_str()));
    assert_eq!(
        std::fs::read_to_string(&fixture.target.path).unwrap(),
        format!("program {version}")
    );
    assert!(
        fixture.target.has_backup(),
        "the old one is kept until the new one is up"
    );
    assert!(fixture.layout().pending().is_some());
}

#[gpui_kit::test]
fn the_footer_item_asks_for_the_restart_too(cx: &mut TestAppContext) {
    let (h, _service, _fixture, _) = ready_window(cx);
    h.click("update-item".to_owned(), cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert!(h.palette_titles(cx)[0].starts_with("Restart Leon and update to"));
}

#[gpui_kit::test]
fn a_restart_with_nothing_ready_says_so(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (h, _service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    run(&h, cx, "restart to update");
    assert!(h.status().contains("No update is ready"), "{}", h.status());
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn a_restart_that_cannot_install_reports_it_and_stays(cx: &mut TestAppContext) {
    let (h, service, fixture, _) = ready_window(cx);
    let quits = quits_counted(&h, cx);
    // The program the update would replace is gone: the swap fails.
    std::fs::remove_file(&fixture.target.path).unwrap();
    run(&h, cx, "restart to update");
    h.press("enter", cx);
    assert_eq!(quits.get(), 0, "it does not quit into nothing");
    assert!(h.status().contains("Could not update"), "{}", h.status());
    assert_eq!(service.take_restart(), None);
}

// ----- quitting with an update ready ---------------------------------------------------------------------

#[gpui_kit::test]
fn quitting_with_an_update_ready_installs_it_in_the_automatic_mode(cx: &mut TestAppContext) {
    let (h, service, fixture, version) = ready_window(cx);
    let quits = quits_counted(&h, cx);
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    assert_eq!(quits.get(), 1);
    assert_eq!(
        std::fs::read_to_string(&fixture.target.path).unwrap(),
        format!("program {version}")
    );
    assert_eq!(service.take_restart(), None, "quitting never restarts");
}

#[gpui_kit::test]
fn quitting_installs_nothing_when_the_person_decides_or_updates_are_off(cx: &mut TestAppContext) {
    for mode in ["notify", "off"] {
        let (h, _service, fixture, _) = ready_window(cx);
        set(&h, cx, "updates_mode", Value::Text(mode.into()));
        let quits = quits_counted(&h, cx);
        h.press_chord("cmd-q", "ctrl-shift-q", cx);
        assert_eq!(quits.get(), 1, "{mode}");
        assert_eq!(
            std::fs::read_to_string(&fixture.target.path).unwrap(),
            "program old",
            "{mode}"
        );
    }
}

// ----- the modes and the timer -----------------------------------------------------------------------

fn tick(h: &Harness, cx: &mut TestAppContext) {
    cx.update(|cx| h.shell.update(cx, |shell, cx| shell.update_tick(cx)));
    h.settle(cx);
}

#[gpui_kit::test]
fn the_background_check_follows_the_mode(cx: &mut TestAppContext) {
    for (mode, asks) in [("automatic", true), ("notify", true), ("off", false)] {
        let fixture = Fixture::new();
        fixture.announce(&newer(), "");
        let (h, _service) =
            open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
        set(&h, cx, "updates_mode", Value::Text(mode.into()));
        tick(&h, cx);
        let asked = fixture
            .http
            .calls()
            .iter()
            .any(|call| call.starts_with("GET"));
        assert_eq!(asked, asks, "{mode}");
    }
}

#[gpui_kit::test]
fn a_development_build_never_asks_github(cx: &mut TestAppContext) {
    for why in [Why::Development, Why::Disabled] {
        let fixture = Fixture::new();
        fixture.announce(&newer(), "");
        let (h, service) = open_with_updates(cx, &fixture, Install::Manual(why));
        assert!(!service.checks_enabled());
        tick(&h, cx);
        assert!(
            fixture.http.calls().is_empty(),
            "{why:?}: {:?}",
            fixture.http.calls()
        );
        // Even asked by hand it explains instead of asking.
        run(&h, cx, "check for updates");
        assert!(fixture.http.calls().is_empty());
        assert_eq!(h.status(), why.explain());
    }
}

#[gpui_kit::test]
fn the_pre_release_setting_reaches_the_updater(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.announce(&newer(), "");
    let (h, _service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    tick(&h, cx);
    assert!(fixture.http.calls()[0].contains("/releases/latest"));
    set(&h, cx, "updates_prereleases", Value::Bool(true));
    run(&h, cx, "check for updates");
    assert!(
        fixture
            .http
            .calls()
            .iter()
            .any(|call| call.contains("releases?per_page")),
        "{:?}",
        fixture.http.calls()
    );
}

#[gpui_kit::test]
fn a_timer_is_set_only_when_the_window_asks_for_one(cx: &mut TestAppContext) {
    // The harness turns the timer off: nothing asks GitHub by itself.
    let fixture = Fixture::new();
    fixture.announce(&newer(), "");
    let (h, _service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    h.settle(cx);
    assert!(fixture.http.calls().is_empty());
}

// ----- the settings, the About card, a notice ---------------------------------------------------------------

#[gpui_kit::test]
fn the_update_settings_are_in_the_schema_and_in_the_palette(cx: &mut TestAppContext) {
    let mode = schema::find("updates_mode").unwrap();
    assert_eq!(
        mode.options()
            .iter()
            .map(|(v, _)| v.as_str())
            .collect::<Vec<_>>(),
        ["automatic", "notify", "off"]
    );
    assert_eq!(mode.default_value(), Some(Value::Text("automatic".into())));
    let pre = schema::find("updates_prereleases").unwrap();
    assert_eq!(pre.default_value(), Some(Value::Bool(false)));
    assert!(schema::find("check_for_updates").unwrap().is_action());
    let h = open(cx, ScriptedRunner::new());
    assert_eq!(cx.update(|cx| Shell::update_mode(cx)), Mode::Automatic);
    h.set_palette_text(">pre-release", cx);
    assert!(h
        .palette_titles(cx)
        .contains(&"setting:updates_prereleases".to_owned()));
}

#[gpui_kit::test]
fn the_check_button_of_the_settings_runs_the_command(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.announce(crate::product::VERSION, "");
    let (h, _service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.run_setting_action("check_for_updates", window, cx)
        })
    })
    .unwrap();
    h.settle(cx);
    assert!(h.status().contains("is the latest"), "{}", h.status());
}

#[gpui_kit::test]
fn the_about_card_shows_the_version_and_the_state_of_updates(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (h, service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    service.updater().publish_for_test(State::Ready {
        offer: offer("0.2.1", ""),
        trust: Trust::Unsigned,
    });
    h.settle(cx);
    run(&h, cx, "about leon");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::About);
    assert!(h.shows("about-version", cx) && h.shows("about-update", cx));
    let line = cx
        .update(|cx| h.shell.read(cx).about_update_line(cx))
        .unwrap();
    assert_eq!(line, "RESTART TO UPDATE · 0.2.1 · NOT SIGNED");
}

#[gpui_kit::test]
fn the_about_card_says_why_updates_are_off_in_a_development_build(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (h, _service) = open_with_updates(cx, &fixture, Install::Manual(Why::Development));
    run(&h, cx, "about leon");
    let line = cx
        .update(|cx| h.shell.read(cx).about_update_line(cx))
        .unwrap();
    assert_eq!(line, "UPDATES OFF · DEVELOPMENT BUILD");
    let plain = open(cx, ScriptedRunner::new());
    run(&plain, cx, "about leon");
    assert!(!plain.shows("about-update", cx), "no updater, no line");
}

#[gpui_kit::test]
fn an_update_that_was_undone_is_said_once_at_start(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let layout = fixture.layout();
    layout
        .save(&leon_update::state::Saved {
            notice: Some("Version 9.9.9 did not start, so version 0.2.0 was put back.".into()),
            ..Default::default()
        })
        .unwrap();
    let (h, service) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    assert!(h.status().contains("did not start"), "{}", h.status());
    assert_eq!(service.take_notice(), None, "said once");
}

impl Harness {
    /// The calls the scripted GitHub of a fixture has seen.
    fn http_calls(&self, fixture: &Fixture) -> Vec<String> {
        fixture.http.calls()
    }

    /// Clicks the middle of a drawn element.
    fn click(&self, selector: String, cx: &mut TestAppContext) {
        self.mouse_on(selector, gpui_kit::MouseButton::Left, cx);
    }
}

#[gpui_kit::test]
fn why_is_a_session_missing_opens_the_history_report(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (h, _) = open_with_updates(cx, &fixture, Install::Updatable(fixture.target.clone()));
    run(&h, cx, "why is a session missing");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::History);
    assert!(h.shows("history-report", cx));
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

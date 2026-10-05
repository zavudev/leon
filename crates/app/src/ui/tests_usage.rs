//! Tests of the usage bar and the usage view: what the footer shows for each
//! agent, how it changes at the thresholds and on a narrow window, how the view
//! opens and moves, and the notice before a session. The readings are written
//! to the in-memory store by the test, or collected through the scripted
//! runner; no network source is reachable (the HTTP client is a script that
//! counts its calls).

use super::live::{open_live, real_worktree, wait_until};
use super::*;
use crate::agent_usage::Scope;
use crate::ui::usage_view::{BarModel, Density};
use leon_usage::network::{Credentials, Read, ScriptedHttp};
use leon_usage::{AgentUsage, Reason, Source, State, UsageWindow, WindowKind};

struct NoCredentials;
impl Credentials for NoCredentials {
    fn claude(&self) -> Read<leon_usage::claude::Credential> {
        Read::Missing
    }
    fn opencode_go(&self) -> Read<leon_usage::secret::Secret> {
        Read::Missing
    }
}

fn now() -> i64 {
    fixed_now().timestamp()
}

fn reading(
    agent: AgentKind,
    machine: &MachineId,
    windows: &[(WindowKind, f64, i64)],
) -> AgentUsage {
    AgentUsage {
        agent,
        machine: machine.as_str().to_owned(),
        account_label: None,
        plan: Some("plus".into()),
        source: Some(Source::Local),
        observed_at: Some(now() - 180),
        state: State::Known {
            windows: windows
                .iter()
                .map(|(kind, used, reset_in)| UsageWindow {
                    kind: kind.clone(),
                    used_percent: *used,
                    resets_at: Some(now() + reset_in),
                    window_length: None,
                })
                .collect(),
        },
    }
}

fn put(h: &Harness, usage: &AgentUsage, cx: &mut TestAppContext) {
    h.store
        .put_usage_reading(
            &MachineId::from_string(usage.machine.clone()),
            usage.agent,
            &serde_json::to_string(usage).unwrap(),
            now() - 180,
        )
        .unwrap();
    h.settle(cx);
}

/// The window with usage collection set up and nothing network-capable in it.
fn open_usage(cx: &mut TestAppContext) -> (Harness, Arc<ScriptedHttp>) {
    let h = open_live(cx);
    let http = Arc::new(ScriptedHttp::new());
    h.engine
        .set_usage(Arc::new(NoCredentials), http.clone(), Arc::new(now));
    h.shell.update(cx, |_, cx| cx.notify());
    h.settle(cx);
    (h, http)
}

fn bar(h: &Harness, cx: &mut TestAppContext) -> BarModel {
    cx.update(|cx| h.shell.read(cx).usage_bar_model(cx))
}

fn local() -> MachineId {
    MachineId::local()
}

#[gpui_kit::test]
fn the_bar_shows_each_agents_primary_window_with_its_figure(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentKind::Claude,
            &local(),
            &[
                (WindowKind::FiveHour, 10.0, 8940),
                (WindowKind::Weekly, 91.0, 126_000),
            ],
        ),
        cx,
    );
    put(
        &h,
        &reading(
            AgentKind::Codex,
            &local(),
            &[
                (WindowKind::FiveHour, 0.0, 600),
                (WindowKind::Weekly, 16.0, 400_000),
            ],
        ),
        cx,
    );
    assert!(h.shows("usage-bar", cx));
    assert!(h.shows("usage-agent-claude", cx));
    assert!(h.shows("usage-agent-codex", cx));
    assert!(!h.shows("usage-agent-opencode", cx), "not read, not shown");
    let model = bar(&h, cx);
    assert_eq!(model.items[0].figure, " 91%");
    assert_eq!(model.items[0].label, "wk");
    assert_eq!(model.items[1].figure, " 16%");
}

#[gpui_kit::test]
fn the_marker_and_level_change_as_a_window_crosses_the_thresholds(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    let mut seen = Vec::new();
    for used in [60.0, 80.0, 95.0] {
        put(
            &h,
            &reading(
                AgentKind::Codex,
                &local(),
                &[(WindowKind::FiveHour, used, 3600)],
            ),
            cx,
        );
        let item = bar(&h, cx).items.remove(0);
        seen.push((item.level, item.glyph));
    }
    use leon_usage::Level::*;
    assert_eq!(seen, [(Normal, ""), (Warning, "!"), (Critical, "!!")]);
}

#[gpui_kit::test]
fn an_unknown_agent_shows_why_instead_of_a_number(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &AgentUsage::unknown(AgentKind::Claude, local().as_str(), Reason::SourceDisabled),
        cx,
    );
    assert!(h.shows("usage-agent-claude", cx));
    assert!(!h.shows("usage-figure-claude", cx), "no figure");
    let item = bar(&h, cx).items.remove(0);
    assert_eq!(item.label, "source off");
    assert!(item.tip.contains(Reason::SourceDisabled.sentence()));
}

#[gpui_kit::test]
fn a_narrow_window_collapses_the_bar_in_steps(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    for agent in AgentKind::ALL {
        put(
            &h,
            &reading(
                agent,
                &local(),
                &[(WindowKind::FiveHour, 20.0 + agent as u8 as f64, 3600)],
            ),
            cx,
        );
    }
    let at = |width: f32, cx: &mut TestAppContext| {
        VisualTestContext::from_window(h.window.into(), cx)
            .simulate_resize(size(px(width), px(800.)));
        h.settle(cx);
        let shown: Vec<bool> = AgentKind::ALL
            .iter()
            .map(|agent| h.shows_dynamic(format!("usage-agent-{}", agent.as_str()), cx))
            .collect();
        (bar(&h, cx).density, shown)
    };
    assert_eq!(at(1900.0, cx), (Density::Full, vec![true, true, true]));
    let (density, _) = at(1200.0, cx);
    assert!(
        matches!(density, Density::Short | Density::Minimal),
        "{density:?}"
    );
    let (density, shown) = at(560.0, cx);
    assert_eq!(density, Density::Single);
    assert_eq!(
        shown.iter().filter(|s| **s).count(),
        1,
        "one worst-case indicator"
    );
}

#[gpui_kit::test]
fn the_single_indicator_is_the_agent_closest_to_its_limit(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentKind::Claude,
            &local(),
            &[(WindowKind::Weekly, 30.0, 3600)],
        ),
        cx,
    );
    put(
        &h,
        &reading(
            AgentKind::Codex,
            &local(),
            &[(WindowKind::Weekly, 88.0, 3600)],
        ),
        cx,
    );
    VisualTestContext::from_window(h.window.into(), cx).simulate_resize(size(px(560.), px(800.)));
    h.settle(cx);
    assert!(h.shows("usage-agent-codex", cx));
    assert!(!h.shows("usage-agent-claude", cx));
}

#[gpui_kit::test]
fn the_status_message_stays_visible_next_to_the_meters(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentKind::Codex,
            &local(),
            &[(WindowKind::Weekly, 10.0, 3600)],
        ),
        cx,
    );
    h.engine.report(StatusKind::Info, "Hello from the engine.");
    h.settle(cx);
    assert!(h.shows("status-line", cx));
    assert!(h.shows("usage-bar", cx));
    assert_eq!(h.status(), "Hello from the engine.");
}

#[gpui_kit::test]
fn the_bar_is_hidden_by_its_setting_and_without_collection(cx: &mut TestAppContext) {
    let h = open_live(cx);
    assert!(
        !h.shows("usage-bar", cx),
        "an engine that collects nothing shows nothing"
    );
    let (h, _) = open_usage(cx);
    assert!(h.shows("usage-bar", cx));
    cx.update(|cx| {
        settings::set_value(
            cx,
            crate::schema::find("usage_bar").unwrap(),
            crate::schema::Value::Bool(false),
        )
    });
    h.settle(cx);
    assert!(!h.shows("usage-bar", cx));
    assert!(h.shows("status-line", cx), "the status strip stays");
}

#[gpui_kit::test]
fn an_agent_switched_off_in_settings_leaves_the_bar(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentKind::Codex,
            &local(),
            &[(WindowKind::Weekly, 10.0, 3600)],
        ),
        cx,
    );
    put(
        &h,
        &reading(
            AgentKind::Claude,
            &local(),
            &[(WindowKind::Weekly, 10.0, 3600)],
        ),
        cx,
    );
    cx.update(|cx| {
        settings::set_value(
            cx,
            crate::schema::find("usage_codex").unwrap(),
            crate::schema::Value::Bool(false),
        )
    });
    h.settle(cx);
    assert!(!h.shows("usage-agent-codex", cx));
    assert!(h.shows("usage-agent-claude", cx));
}

#[gpui_kit::test]
fn the_chord_opens_the_view_and_escape_closes_it(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentKind::Codex,
            &local(),
            &[(WindowKind::FiveHour, 38.0, 8940)],
        ),
        cx,
    );
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Usage);
    assert!(h.shows("usage-view", cx));
    assert!(h.shows("usage-row-0", cx));
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn clicking_the_bar_opens_the_view(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentKind::Codex,
            &local(),
            &[(WindowKind::FiveHour, 38.0, 8940)],
        ),
        cx,
    );
    h.mouse_on(
        "usage-agent-codex".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Usage);
    h.press("escape", cx);
    h.mouse_on("usage-open".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Usage);
}

#[gpui_kit::test]
fn the_palette_offers_show_usage(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">show usage", cx);
    assert!(
        h.palette_titles(cx).contains(&"Show usage".to_owned()),
        "{:?}",
        h.palette_titles(cx)
    );
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Usage);
}

#[gpui_kit::test]
fn the_view_shows_resets_provenance_and_the_unknown_reason(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentKind::Codex,
            &local(),
            &[
                (WindowKind::FiveHour, 38.0, 8940),
                (WindowKind::Weekly, 15.0, 400_000),
            ],
        ),
        cx,
    );
    put(
        &h,
        &AgentUsage::unknown(AgentKind::Claude, local().as_str(), Reason::SourceDisabled),
        cx,
    );
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    let rows = cx.update(|cx| h.shell.read(cx).usage_view_rows(cx));
    assert_eq!(rows.len(), 2);
    let claude = &rows[0];
    assert_eq!(claude.view.agent, AgentKind::Claude);
    assert_eq!(claude.view.provenance(), Reason::SourceDisabled.sentence());
    let codex = &rows[1];
    assert_eq!(codex.windows.len(), 2);
    assert_eq!(
        codex.windows[0].meter.reset_text().as_deref(),
        Some("Resets in 2h 29m")
    );
    assert_eq!(
        codex.view.provenance(),
        "from Codex's own session log, 3 min ago"
    );
    assert!(h.shows("usage-window-1-0", cx));
    assert!(h.shows("usage-window-1-1", cx));
    assert!(h.shows("usage-unknown-0", cx));
}

#[gpui_kit::test]
fn m_switches_between_detailed_and_compact_and_so_do_the_chips(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentKind::Codex,
            &local(),
            &[(WindowKind::FiveHour, 38.0, 8940)],
        ),
        cx,
    );
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    assert!(h.shell(cx, |s| s.usage.detailed));
    assert!(h.shows("usage-window-0-0", cx), "a labelled bar per window");
    h.press("m", cx);
    assert!(!h.shell(cx, |s| s.usage.detailed));
    assert!(
        !h.shows("usage-window-0-0", cx),
        "compact: one line per agent"
    );
    h.mouse_on(
        "usage-mode-detailed".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(h.shell(cx, |s| s.usage.detailed));
    h.mouse_on(
        "usage-mode-compact".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(!h.shell(cx, |s| s.usage.detailed));
}

#[gpui_kit::test]
fn the_arrows_choose_the_machine_the_view_lists(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    let remote = h
        .store
        .add_machine(
            "build box",
            MachineKind::Ssh {
                host: "box.example".into(),
                user: None,
                port: None,
                identity_file: None,
            },
        )
        .unwrap();
    put(
        &h,
        &reading(
            AgentKind::Codex,
            &local(),
            &[(WindowKind::FiveHour, 10.0, 3600)],
        ),
        cx,
    );
    put(
        &h,
        &reading(
            AgentKind::Codex,
            &remote.id,
            &[(WindowKind::FiveHour, 80.0, 3600)],
        ),
        cx,
    );
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    let listed = |h: &Harness, cx: &mut TestAppContext| {
        cx.update(|cx| h.shell.read(cx).usage_view_rows(cx))
            .iter()
            .map(|row| row.machine_name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(listed(&h, cx), ["This machine"]);
    // The window has other machines too: step to the one with the reading.
    let others = h.store.machines().unwrap().len() - 1;
    for _ in 0..others {
        if h.shell(cx, |s| s.usage.scope.clone()) == Scope::Machine(remote.id.clone()) {
            break;
        }
        h.press("right", cx);
    }
    assert_eq!(
        h.shell(cx, |s| s.usage.scope.clone()),
        Scope::Machine(remote.id.clone())
    );
    assert_eq!(listed(&h, cx), ["build box"]);
    for _ in 0..others {
        if h.shell(cx, |s| s.usage.scope.clone()) == Scope::All {
            break;
        }
        h.press("right", cx);
    }
    assert_eq!(h.shell(cx, |s| s.usage.scope.clone()), Scope::All);
    assert_eq!(listed(&h, cx).len(), 2);
    h.press("right", cx);
    assert_eq!(h.shell(cx, |s| s.usage.scope.clone()), Scope::Context);
}

#[gpui_kit::test]
fn refreshing_collects_through_the_runner_and_shows_what_it_found(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let found = format!(
        "has=codex\n@codex\n{}\n",
        r#"{"timestamp":"2026-10-04T11:59:00.000Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":42.0,"window_minutes":300,"resets_at":1791123000},"secondary":{"used_percent":15.0,"window_minutes":10080,"resets_at":1791678919},"plan_type":"plus"}}}"#
    );
    let h = open(cx, ScriptedRunner::new().reply(Output::ok(found)));
    // The scripted runner needs no shell: read this computer as one with a
    // POSIX shell on every platform.
    h.engine.set_local_posix_shell(true);
    h.engine.set_usage(
        Arc::new(NoCredentials),
        Arc::new(ScriptedHttp::new()),
        Arc::new(now),
    );
    h.shell.update(cx, |_, cx| cx.notify());
    h.settle(cx);
    assert!(h.runner.calls().is_empty());
    h.mouse_on("usage-refresh".to_owned(), gpui_kit::MouseButton::Left, cx);
    wait_until(&h, cx, "the collection", |h, cx| {
        h.shows("usage-agent-codex", cx)
    });
    let machines = h.store.machines().unwrap().len();
    assert_eq!(h.runner.calls().len(), machines, "one command per machine");
    assert_eq!(bar(&h, cx).items[0].figure, " 42%");
}

#[gpui_kit::test]
fn starting_an_agent_at_its_critical_limit_says_so_with_the_reset_and_never_blocks(
    cx: &mut TestAppContext,
) {
    let (h, _) = open_usage(cx);
    let (_dir, _) = real_worktree(&h, cx);
    put(
        &h,
        &reading(
            AgentKind::Claude,
            &local(),
            &[(WindowKind::FiveHour, 96.0, 2 * 3600 + 29 * 60)],
        ),
        cx,
    );
    h.press_chord("cmd-n", "ctrl-shift-a", cx);
    h.press("enter", cx); // Claude
    wait_until(&h, cx, "the session", |h, cx| {
        !h.shell(cx, |s| s.live.ids().is_empty())
    });
    assert_eq!(
        h.status(),
        "Claude Code: 5-hour window is 96% used, resets in 2h 29m."
    );
}

#[gpui_kit::test]
fn below_the_critical_limit_or_with_the_setting_off_nothing_is_said(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    let (_dir, _) = real_worktree(&h, cx);
    put(
        &h,
        &reading(
            AgentKind::Claude,
            &local(),
            &[(WindowKind::FiveHour, 80.0, 3600)],
        ),
        cx,
    );
    h.press_chord("cmd-n", "ctrl-shift-a", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the session", |h, cx| {
        !h.shell(cx, |s| s.live.ids().is_empty())
    });
    assert!(!h.status().contains("used"), "{}", h.status());

    cx.update(|cx| {
        settings::set_value(
            cx,
            crate::schema::find("usage_warn_before_session").unwrap(),
            crate::schema::Value::Bool(false),
        )
    });
    put(
        &h,
        &reading(
            AgentKind::Claude,
            &local(),
            &[(WindowKind::FiveHour, 99.0, 3600)],
        ),
        cx,
    );
    h.press_chord("cmd-n", "ctrl-shift-a", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "a second session", |h, cx| {
        h.shell(cx, |s| s.live.ids().len()) == 2
    });
    assert!(!h.status().contains("used"), "{}", h.status());
}

#[gpui_kit::test]
fn the_header_of_a_live_agent_session_shows_its_primary_window(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    let (_dir, _) = real_worktree(&h, cx);
    put(
        &h,
        &reading(
            AgentKind::Claude,
            &local(),
            &[
                (WindowKind::FiveHour, 20.0, 3600),
                (WindowKind::Weekly, 55.0, 90_000),
            ],
        ),
        cx,
    );
    assert!(!h.shows("header-usage", cx), "no session yet");
    h.press_chord("cmd-n", "ctrl-shift-a", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the session", |h, cx| h.shows("header-usage", cx));
}

#[gpui_kit::test]
fn the_settings_link_opens_settings_on_the_usage_section(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    h.press("s", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Settings);
    assert_eq!(
        h.shell(cx, |s| s.settings_ui.section),
        crate::schema::Section::Usage
    );
}

#[gpui_kit::test]
fn forgetting_the_history_is_a_button_of_the_settings(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    h.store
        .record_usage_points(
            &local(),
            AgentKind::Codex,
            "acct",
            &[(
                "five_hour".to_owned(),
                leon_core::UsagePoint {
                    at: now(),
                    used_percent: 5.0,
                },
            )],
            0,
        )
        .unwrap();
    h.shell.update(cx, |_, _| {});
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.run_setting_action("usage_forget_history", window, cx)
        })
    })
    .unwrap();
    h.settle(cx);
    assert!(h.status().starts_with("Forgot "), "{}", h.status());
    assert!(h
        .store
        .usage_history(&local(), AgentKind::Codex, "acct", "five_hour", 0)
        .unwrap()
        .is_empty());
}

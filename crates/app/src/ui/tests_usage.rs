//! Tests of the usage bar and the usage view: what the footer shows for each
//! agent, how it changes at the thresholds and on a narrow window, how the view
//! opens and moves, and the notice before a session. The readings are written
//! to the in-memory store by the test, or collected through the scripted
//! runner; no network source is reachable (the HTTP client is a script that
//! counts its calls).

use super::live::{open_live, real_worktree, wait_until};
use super::*;
use crate::agent_usage::{Board, Scope};
use crate::ui::usage_view::BarModel;
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

fn reading(agent: AgentId, machine: &MachineId, windows: &[(WindowKind, f64, i64)]) -> AgentUsage {
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
            AgentId::CLAUDE,
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
            AgentId::CODEX,
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
    for used in [59.0, 60.0, 80.0] {
        put(
            &h,
            &reading(
                AgentId::CODEX,
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
        &AgentUsage::unknown(AgentId::CLAUDE, local().as_str(), Reason::SourceDisabled),
        cx,
    );
    assert!(h.shows("usage-agent-claude", cx));
    assert!(!h.shows("usage-figure-claude", cx), "no figure");
    let item = bar(&h, cx).items.remove(0);
    assert_eq!(item.label, "source off");
    assert!(item.tip.contains(Reason::SourceDisabled.sentence()));
}

#[gpui_kit::test]
fn the_bar_keeps_a_figure_per_agent_and_folds_what_does_not_fit(cx: &mut TestAppContext) {
    fn fill(h: &Harness, cx: &mut TestAppContext) {
        for agent in [AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE] {
            put(
                h,
                &reading(
                    agent,
                    &local(),
                    &[(
                        WindowKind::FiveHour,
                        20.0 + agent.as_str().len() as f64,
                        3600,
                    )],
                ),
                cx,
            );
        }
    }
    let (h, _) = open_usage(cx);
    fill(&h, cx);
    let at = |width: f32, cx: &mut TestAppContext| {
        VisualTestContext::from_window(h.window.into(), cx)
            .simulate_resize(size(px(width), px(800.)));
        h.settle(cx);
        let shown: Vec<bool> = [AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE]
            .iter()
            .map(|agent| h.shows_dynamic(format!("usage-agent-{}", agent.as_str()), cx))
            .collect();
        let model = bar(&h, cx);
        (model.items.len(), model.folded.len(), shown)
    };
    assert_eq!(at(1900.0, cx), (3, 0, vec![true, true, true]));
    // Narrow: the agent closest to its limit stays, the rest fold into the
    // count (OpenCode has the highest figure here).
    let (items, folded, shown) = at(560.0, cx);
    assert_eq!((items, folded), (1, 2));
    assert_eq!(shown, vec![false, false, true]);
}

#[gpui_kit::test]
fn the_bar_shows_the_window_closest_to_its_limit_in_every_agent(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    for agent in [AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE] {
        put(
            &h,
            &reading(
                agent,
                &local(),
                &[
                    (WindowKind::FiveHour, 10.0, 8940),
                    (WindowKind::Weekly, 91.0, 126_000),
                    (WindowKind::ModelWeekly("Fable".into()), 0.0, 126_000),
                ],
            ),
            cx,
        );
    }
    let model = bar(&h, cx);
    assert_eq!(model.items.len(), 3);
    for item in &model.items {
        assert_eq!(item.label, "wk");
        assert_eq!(item.figure_text(), " 91% !!");
        assert!(
            item.tip.contains("Weekly, Fable: 0% used"),
            "the tooltip keeps every window: {}",
            item.tip
        );
    }
}

#[gpui_kit::test]
fn the_single_indicator_is_the_agent_closest_to_its_limit(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentId::CLAUDE,
            &local(),
            &[(WindowKind::Weekly, 30.0, 3600)],
        ),
        cx,
    );
    put(
        &h,
        &reading(
            AgentId::CODEX,
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
            AgentId::CODEX,
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
            AgentId::CODEX,
            &local(),
            &[(WindowKind::Weekly, 10.0, 3600)],
        ),
        cx,
    );
    put(
        &h,
        &reading(
            AgentId::CLAUDE,
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
    // Claude stays: listed, or (when this computer has no shell of its own and
    // every agent reads "unsupported") folded into the count.
    let model = bar(&h, cx);
    assert!(model
        .items
        .iter()
        .chain(&model.folded)
        .any(|item| item.agent == AgentId::CLAUDE));
    assert!(!model
        .items
        .iter()
        .chain(&model.folded)
        .any(|item| item.agent == AgentId::CODEX));
}

#[gpui_kit::test]
fn the_chord_opens_the_view_and_escape_closes_it(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentId::CODEX,
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
            AgentId::CODEX,
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
            AgentId::CODEX,
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
        &AgentUsage::unknown(AgentId::CLAUDE, local().as_str(), Reason::SourceDisabled),
        cx,
    );
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    let rows = cx.update(|cx| h.shell.read(cx).usage_view_rows(cx));
    assert_eq!(rows.len(), 2);
    // Worst first: the agent with numbers, then the one that has none.
    let claude = &rows[1];
    assert_eq!(claude.view.agent, AgentId::CLAUDE);
    assert_eq!(claude.view.provenance(), Reason::SourceDisabled.sentence());
    let codex = &rows[0];
    assert_eq!(codex.windows.len(), 2);
    assert_eq!(
        codex.windows[0].meter.reset_text().as_deref(),
        Some("Resets in 2h 29m")
    );
    assert_eq!(
        codex.view.provenance(),
        "from Codex's own session log, 3 min ago"
    );
    assert!(h.shows("usage-window-0-0", cx));
    assert!(h.shows("usage-window-0-1", cx));
    assert!(h.shows("usage-unknown-1", cx));
}

#[gpui_kit::test]
fn opening_the_view_reads_once_when_the_last_reading_is_older_than_the_interval(
    cx: &mut TestAppContext,
) {
    let (h, _) = open_usage(cx);
    let codex = reading(
        AgentId::CODEX,
        &local(),
        &[(WindowKind::FiveHour, 38.0, 8940)],
    );
    // Read three minutes ago, with a ten-minute interval: shown as it is.
    put(&h, &codex, cx);
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    assert_eq!(h.shell(cx, |s| s.usage.last_request), None);
    h.press("escape", cx);
    // Read eleven minutes ago: opening the view reads, once.
    h.store
        .put_usage_reading(
            &local(),
            AgentId::CODEX,
            &serde_json::to_string(&codex).unwrap(),
            now() - 11 * 60,
        )
        .unwrap();
    h.settle(cx);
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    let asked = h.shell(cx, |s| s.usage.last_request);
    assert!(asked.is_some(), "the view read the stale numbers");
    h.press("escape", cx);
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    assert_eq!(
        h.shell(cx, |s| s.usage.last_request),
        asked,
        "not again within the interval"
    );
}

#[gpui_kit::test]
fn m_switches_between_detailed_and_compact_and_so_do_the_chips(cx: &mut TestAppContext) {
    let (h, _) = open_usage(cx);
    put(
        &h,
        &reading(
            AgentId::CODEX,
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
            AgentId::CODEX,
            &local(),
            &[(WindowKind::FiveHour, 10.0, 3600)],
        ),
        cx,
    );
    put(
        &h,
        &reading(
            AgentId::CODEX,
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
            AgentId::CLAUDE,
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
            AgentId::CLAUDE,
            &local(),
            &[(WindowKind::FiveHour, 79.0, 3600)],
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
            AgentId::CLAUDE,
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
            AgentId::CLAUDE,
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
            AgentId::CODEX,
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
        .usage_history(&local(), AgentId::CODEX, "acct", "five_hour", 0)
        .unwrap()
        .is_empty());
}

// ----- the network sources, switched live ----------------------------------------------------

use leon_usage::network::{Http, HttpError, Request, Response};

const CLAUDE_ANSWER: &str = r#"{"five_hour":{"utilization":10,"resets_at":1791300000},"seven_day":{"utilization":93,"resets_at":1791300000}}"#;

/// A credential reader that answers as told and counts its reads.
struct Creds {
    denied: std::sync::atomic::AtomicBool,
    reads: std::sync::atomic::AtomicUsize,
}

impl Creds {
    fn new(denied: bool) -> Arc<Self> {
        Arc::new(Self {
            denied: denied.into(),
            reads: Default::default(),
        })
    }
}

impl Credentials for Creds {
    fn claude(&self) -> Read<leon_usage::claude::Credential> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.denied.load(std::sync::atomic::Ordering::SeqCst) {
            return Read::Denied;
        }
        leon_usage::claude::parse_credential(
            r#"{"claudeAiOauth":{"accessToken":"tok","subscriptionType":"max"}}"#,
        )
        .map_or(Read::Missing, Read::Found)
    }
    fn opencode_go(&self) -> Read<leon_usage::secret::Secret> {
        Read::Missing
    }
}

/// A client that answers only when the test lets it.
struct Gate {
    open: Arc<tokio::sync::Notify>,
    calls: std::sync::atomic::AtomicUsize,
}

impl Http for Gate {
    fn get<'a>(
        &'a self,
        _: &'a Request<'a>,
    ) -> leon_usage::network::BoxFuture<'a, Result<Response, HttpError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.open.notified().await;
            Ok(Response {
                status: 200,
                body: CLAUDE_ANSWER.to_owned(),
                retry_after: None,
            })
        })
    }
}

/// The window with Claude Code installed on this computer and the usage engine
/// reading through `creds` and `http`.
fn open_network(cx: &mut TestAppContext, creds: Arc<Creds>, http: Arc<dyn Http>) -> Harness {
    let mut runner = ScriptedRunner::new();
    for _ in 0..40 {
        runner = runner.reply(Output::ok("has=claude\n".to_owned()));
    }
    let h = open(cx, runner);
    h.engine.set_local_posix_shell(true);
    h.engine.set_usage(creds, http, Arc::new(now));
    h.shell.update(cx, |_, cx| cx.notify());
    h.settle(cx);
    h
}

fn claude_source(h: &Harness, on: bool, cx: &mut TestAppContext) {
    cx.update(|cx| {
        settings::set_value(
            cx,
            crate::schema::find("usage_claude_network").unwrap(),
            crate::schema::Value::Bool(on),
        )
    });
    h.shell.update(cx, |_, cx| cx.notify());
    h.settle(cx);
}

fn claude_row(h: &Harness, cx: &mut TestAppContext) -> crate::ui::usage_view::UsageRow {
    cx.update(|cx| h.shell.read(cx).usage_view_rows(cx))
        .into_iter()
        .find(|row| row.view.agent == AgentId::CLAUDE)
        .expect("a row for Claude Code")
}

fn claude_item(h: &Harness, cx: &mut TestAppContext) -> crate::ui::usage_view::BarItem {
    bar(h, cx)
        .items
        .into_iter()
        .find(|item| item.agent == AgentId::CLAUDE)
        .expect("Claude Code is in the bar")
}

#[gpui_kit::test]
fn the_network_sources_are_on_by_default_and_a_file_that_set_them_off_stays_off(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(settings::FILE_NAME),
        br#"{"usage_opencode_network": false}"#,
    )
    .unwrap();
    let _h = open_with(
        cx,
        ScriptedRunner::new(),
        Some(dir.path().join(settings::FILE_NAME)),
    );
    let policy = cx.update(|cx| settings::usage_policy(cx));
    assert!(
        policy.allows(AgentId::CLAUDE),
        "a file without the key gets the default: on"
    );
    assert!(
        !policy.allows(AgentId::OPENCODE),
        "a file that says off stays off"
    );
    assert_eq!(cx.update(|cx| settings::usage_interval_seconds(cx)), 600);
}

#[gpui_kit::test]
fn turning_the_claude_source_on_reads_at_once_and_shows_the_numbers(cx: &mut TestAppContext) {
    let http = Arc::new(ScriptedHttp::new().reply(200, CLAUDE_ANSWER));
    let h = open_network(cx, Creds::new(false), http.clone());
    claude_source(&h, false, cx);
    assert_eq!(http.calls().len(), 0, "off: nothing is called");
    assert_eq!(claude_row(&h, cx).turn_on, Some("usage_claude_network"));
    claude_source(&h, true, cx);
    assert_eq!(
        http.calls().len(),
        1,
        "on: read at once, not at the next tick"
    );
    let item = claude_item(&h, cx);
    assert!(
        item.known,
        "the footer shows the numbers without reopening anything"
    );
    assert_eq!(item.figure, " 93%");
    assert_eq!(item.glyph, "!!", "93% is critical, with a marker");
    let row = claude_row(&h, cx);
    assert!(matches!(row.view.body, leon_usage::Body::Ready { .. }));
    assert!(row.note.is_none());
    assert!(h.shell(cx, |s| s.usage.board.collected_at()).is_some());
}

#[gpui_kit::test]
fn while_the_first_read_is_in_flight_the_view_says_reading(cx: &mut TestAppContext) {
    let open = Arc::new(tokio::sync::Notify::new());
    let http = Arc::new(Gate {
        open: open.clone(),
        calls: Default::default(),
    });
    let h = open_network(cx, Creds::new(false), http.clone());
    claude_source(&h, false, cx);
    claude_source(&h, true, cx);
    assert!(h.engine.usage_status(AgentId::CLAUDE).reading);
    let row = claude_row(&h, cx);
    let note = row.note.expect("the row says what is going on");
    assert!(note.starts_with("Reading…"), "{note}");
    assert!(
        !note.contains("source is off"),
        "not the stale 'source is off': {note}"
    );
    if crate::platform::is_mac() {
        assert!(note.contains("macOS may ask for permission to read Claude Code's sign-in"));
    }
    assert_eq!(claude_item(&h, cx).label, "reading…");
    open.notify_one();
    h.settle(cx);
    assert!(!h.engine.usage_status(AgentId::CLAUDE).reading);
    assert!(
        claude_item(&h, cx).known,
        "the numbers replace it when the read ends"
    );
}

#[gpui_kit::test]
fn a_denied_keychain_prompt_is_explained_and_can_be_retried(cx: &mut TestAppContext) {
    let creds = Creds::new(true);
    let http = Arc::new(ScriptedHttp::new().reply(200, CLAUDE_ANSWER));
    let h = open_network(cx, creds.clone(), http.clone());
    claude_source(&h, false, cx);
    claude_source(&h, true, cx);
    let row = claude_row(&h, cx);
    let note = row.note.clone().expect("the refusal is explained");
    assert!(note.contains("Try again"), "{note}");
    assert!(row.try_again);
    if crate::platform::is_mac() {
        assert!(note.contains("denied or dismissed"), "{note}");
    }
    assert_eq!(http.calls().len(), 0);
    // Never asked again by itself, however often the schedule comes round.
    let asked = creds.reads.load(std::sync::atomic::Ordering::SeqCst);
    for _ in 0..3 {
        h.engine.submit(crate::engine::Op::CollectUsage);
        h.settle(cx);
    }
    assert_eq!(creds.reads.load(std::sync::atomic::Ordering::SeqCst), asked);
    // Try again (R in the view) asks once more, and now it is allowed.
    creds
        .denied
        .store(false, std::sync::atomic::Ordering::SeqCst);
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    h.press("r", cx);
    assert_eq!(http.calls().len(), 1);
    assert!(claude_item(&h, cx).known);
}

#[gpui_kit::test]
fn turning_the_source_off_stops_calls_and_says_so(cx: &mut TestAppContext) {
    let http = Arc::new(ScriptedHttp::new().reply(200, CLAUDE_ANSWER));
    let h = open_network(cx, Creds::new(false), http.clone());
    claude_source(&h, false, cx);
    claude_source(&h, true, cx);
    assert!(claude_item(&h, cx).known);
    claude_source(&h, false, cx);
    h.engine.submit(crate::engine::Op::CollectUsage);
    h.settle(cx);
    assert_eq!(http.calls().len(), 1, "no call once it is off");
    let row = claude_row(&h, cx);
    assert!(matches!(
        row.view.body,
        leon_usage::Body::Unknown(Reason::SourceDisabled)
    ));
    assert_eq!(
        row.turn_on,
        Some("usage_claude_network"),
        "the action is offered right there"
    );
}

#[gpui_kit::test]
fn refresh_now_uses_the_setting_in_force(cx: &mut TestAppContext) {
    let http = Arc::new(
        ScriptedHttp::new()
            .reply(200, CLAUDE_ANSWER)
            .reply(200, CLAUDE_ANSWER),
    );
    let h = open_network(cx, Creds::new(false), http.clone());
    claude_source(&h, false, cx);
    h.press_chord("cmd-shift-u", "ctrl-shift-alt-u", cx);
    h.press("r", cx);
    assert_eq!(
        http.calls().len(),
        0,
        "off: R reads nothing from the network"
    );
    cx.update(|cx| {
        settings::set_value(
            cx,
            crate::schema::find("usage_claude_network").unwrap(),
            crate::schema::Value::Bool(true),
        )
    });
    h.press("r", cx);
    assert!(!http.calls().is_empty(), "on: R reads");
    assert!(claude_item(&h, cx).known);
}

#[gpui_kit::test]
fn a_failure_shows_its_reason_and_the_retry_time(cx: &mut TestAppContext) {
    let http = Arc::new(ScriptedHttp::new().reply(503, "down"));
    let h = open_network(cx, Creds::new(false), http.clone());
    claude_source(&h, false, cx);
    claude_source(&h, true, cx);
    let row = claude_row(&h, cx);
    let note = row.note.expect("the failure is explained");
    assert!(note.contains("HTTP 503"), "{note}");
    assert!(note.contains("Next read in 1m"), "{note}");
    assert!(row.try_again);
    let tip = claude_item(&h, cx).tip;
    assert!(tip.contains("HTTP 503"), "{tip}");
}

#[gpui_kit::test]
fn a_rate_limit_says_so_and_when_the_next_read_is(cx: &mut TestAppContext) {
    let http = Arc::new(ScriptedHttp::new().reply_after(429, "{}", Some(300)));
    let h = open_network(cx, Creds::new(false), http.clone());
    claude_source(&h, false, cx);
    claude_source(&h, true, cx);
    let note = claude_row(&h, cx).note.expect("explained");
    assert!(note.starts_with("Rate limited"), "{note}");
    assert!(note.contains("Next read in 5m"), "{note}");
    assert!(!note.to_lowercase().contains("error"), "{note}");
}

#[test]
fn a_window_at_93_percent_is_critical_with_a_marker_in_the_view_the_bar_and_the_header() {
    let reading = reading(
        AgentId::CLAUDE,
        &local(),
        &[
            (WindowKind::FiveHour, 10.0, 8940),
            (WindowKind::Weekly, 93.0, 126_000),
        ],
    );
    let board = crate::agent_usage::Board::new(vec![reading]);
    let thresholds = leon_usage::Thresholds::default();
    let palette = crate::theme::ThemeId::DEFAULT.theme().dark;
    let rows = crate::ui::usage_view::usage_rows(
        &board,
        &Scope::Context,
        &local(),
        &[AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE],
        &|id| id.to_owned(),
        now(),
        thresholds,
        &Default::default(),
    );
    let weekly = &rows[0].windows[1].meter;
    assert_eq!(weekly.level, leon_usage::Level::Critical);
    assert_eq!(weekly.level.glyph(), "!!");
    assert!(weekly.text().ends_with("!!"), "{}", weekly.text());
    assert_eq!(
        crate::ui::usage_view::level_colour(weekly.level, &palette),
        palette.error,
        "a colour of the theme, not the neutral one"
    );
    let item = crate::ui::usage_view::bar_model(
        &board,
        &local(),
        "This computer",
        &[AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE],
        now(),
        thresholds,
        leon_usage::PercentDisplay::Used,
        1600.0,
    )
    .items
    .remove(0);
    assert_eq!(
        (item.level, item.glyph, item.figure.as_str()),
        (leon_usage::Level::Critical, "!!", " 93%")
    );
    let leon_usage::Body::Ready { primary, .. } = &rows[0].view.body else {
        panic!("numbers expected");
    };
    assert!(
        crate::ui::usage_view::chip_text(primary, leon_usage::PercentDisplay::Used)
            .contains("93% !!")
    );
    // And from 60 a warning, from the settings' own thresholds.
    assert_eq!(thresholds.classify(60.0).glyph(), "!");
}

#[gpui_kit::test]
fn a_settings_file_of_the_three_agent_era_still_applies_and_new_agents_get_their_defaults(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(settings::FILE_NAME),
        br#"{"agent_claude_args":"--model opus","agent_codex_enabled":false,"usage_codex":false,"default_agent":"opencode","usage_claude_network":false}"#,
    )
    .unwrap();
    let _h = open_with(
        cx,
        ScriptedRunner::new(),
        Some(dir.path().join(settings::FILE_NAME)),
    );
    let (prefs, steps, policy, shown, default) = cx.update(|cx| {
        (
            Shell::launch_prefs(cx),
            Shell::step_prefs(cx),
            settings::usage_policy(cx),
            settings::usage_agents(cx),
            settings::text(cx, "default_agent"),
        )
    });
    assert_eq!(prefs.agent(AgentId::CLAUDE).args, ["--model", "opus"]);
    assert!(!steps.offered().contains(&AgentId::CODEX));
    assert!(
        steps.offered().contains(&AgentId::GROK),
        "a new agent is on by default"
    );
    assert!(!policy.allows(AgentId::CLAUDE) && policy.allows(AgentId::GROK));
    assert!(!shown.contains(&AgentId::CODEX) && shown.contains(&AgentId::CURSOR));
    assert_eq!(default, "opencode");
}

#[gpui_kit::test]
fn a_custom_agent_is_added_validated_kept_and_offered_like_a_built_in_one(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(settings::FILE_NAME);
    let _h = open_with(cx, ScriptedRunner::new(), Some(file.clone()));
    let id = cx
        .update(|cx| {
            settings::add_custom_agent(cx, "Zed Zeta Tool", "zzt", "--fast", "--resume {id}")
        })
        .unwrap();
    assert_eq!(id.as_str(), "custom-zed-zeta-tool");
    // Everywhere a built-in one is: the catalogue, the offered list, the launch.
    cx.update(|cx| {
        assert!(Shell::step_prefs(cx).offered().contains(&id));
        assert_eq!(id.name(), "Zed Zeta Tool");
        assert_eq!(
            crate::launch::command_line_with(
                id,
                Some("s 1"),
                crate::launch::Flavor::Posix,
                &Shell::launch_prefs(cx).agent(id).clone()
            )
            .as_deref(),
            Some("zzt --resume 's 1'")
        );
    });
    // Kept in the file as one JSON object per agent.
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("custom_agents") && text.contains("zzt"),
        "{text}"
    );
    // Refused with a reason, and nothing is added.
    for (name, command, resume) in [
        ("Zed Zeta Tool", "other", ""),
        ("claude code", "x", ""),
        ("Bad Tool", "", ""),
        ("Bad Tool", "x", "{nope}"),
    ] {
        let refused = cx.update(|cx| settings::add_custom_agent(cx, name, command, "", resume));
        assert!(refused.is_err(), "{name} {command} {resume}");
    }
    cx.update(|cx| settings::remove_custom_agent(cx, id));
    cx.update(|cx| assert!(!Shell::step_prefs(cx).offered().contains(&id)));
}

#[test]
fn with_many_agents_the_bar_lists_those_with_numbers_and_folds_the_rest_into_a_count() {
    let mut readings = Vec::new();
    for (agent, used) in [(AgentId::CODEX, 30.0), (AgentId::GROK, 50.0)] {
        readings.push(known_reading(agent, used));
    }
    for agent in [
        AgentId::CLAUDE,
        AgentId::CURSOR,
        AgentId::KIMI,
        AgentId::ZCODE,
    ] {
        readings.push(AgentUsage::unknown(agent, "local", Reason::NotSignedIn));
    }
    // Not installed: left out altogether, as ever.
    readings.push(AgentUsage::unknown(
        AgentId::OPENCODE,
        "local",
        Reason::NotInstalled,
    ));
    let board = Board::new(readings);
    let all = leon_usage::network::switchable_agents();
    let model = crate::ui::usage_view::bar_model(
        &board,
        &MachineId::local(),
        "This computer",
        &all,
        1_790_000_000,
        leon_usage::Thresholds::default(),
        leon_usage::PercentDisplay::Used,
        1600.0,
    );
    let shown: Vec<AgentId> = model.items.iter().map(|i| i.agent).collect();
    assert_eq!(shown, [AgentId::CODEX, AgentId::GROK]);
    assert_eq!(model.folded.len(), 4);
    assert!(model.folded.iter().all(|i| !i.known));
    // With few agents nothing is folded: each says why.
    let few = Board::new(vec![
        known_reading(AgentId::CODEX, 30.0),
        AgentUsage::unknown(AgentId::CLAUDE, "local", Reason::NotSignedIn),
    ]);
    let model = crate::ui::usage_view::bar_model(
        &few,
        &MachineId::local(),
        "This computer",
        &all,
        1_790_000_000,
        leon_usage::Thresholds::default(),
        leon_usage::PercentDisplay::Used,
        1600.0,
    );
    assert_eq!(model.items.len(), 2);
    assert!(model.folded.is_empty());
}

fn known_reading(agent: AgentId, used: f64) -> AgentUsage {
    AgentUsage {
        agent,
        machine: "local".into(),
        account_label: None,
        plan: None,
        source: Some(Source::VendorApi),
        observed_at: Some(1_790_000_000),
        state: State::Known {
            windows: vec![UsageWindow {
                kind: WindowKind::Weekly,
                used_percent: used,
                resets_at: Some(1_790_100_000),
                window_length: None,
            }],
        },
    }
}

#[test]
fn the_view_tells_the_agents_with_numbers_from_those_not_installed_or_without_data() {
    use crate::ui::usage_view::usage_rows;
    let board = Board::new(vec![
        known_reading(AgentId::CODEX, 10.0),
        AgentUsage::unknown(AgentId::GROK, "local", Reason::NotInstalled),
        AgentUsage::unknown(AgentId::CURSOR, "local", Reason::NoData),
        AgentUsage::unknown(AgentId::CLAUDE, "local", Reason::NotSignedIn),
    ]);
    let rows = usage_rows(
        &board,
        &Scope::Context,
        &MachineId::local(),
        &leon_usage::network::switchable_agents(),
        &|_| "This computer".to_owned(),
        1_790_000_000,
        leon_usage::Thresholds::default(),
        &std::collections::HashMap::new(),
    );
    let inactive: Vec<AgentId> = rows
        .iter()
        .filter(|r| r.is_inactive())
        .map(|r| r.view.agent)
        .collect();
    let active: Vec<AgentId> = rows
        .iter()
        .filter(|r| !r.is_inactive())
        .map(|r| r.view.agent)
        .collect();
    assert_eq!(inactive, [AgentId::GROK, AgentId::CURSOR]);
    assert_eq!(
        active,
        [AgentId::CODEX, AgentId::CLAUDE],
        "signed out is not inactive: it needs you, after the agents with numbers"
    );
}

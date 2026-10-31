//! The coordinator against what the person, King's tools and the agents do,
//! in order, with the clock given.

use leon_turn::{PermissionChoice, PermissionEffect, QuestionKind, TurnEvent};

use super::*;

fn config() -> Config {
    let mut config = Config::default();
    config.limits.wake_delay_secs = 5;
    config.limits.max_workers = 2;
    config
}

fn starts(effects: &[Effect]) -> Vec<(Target, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Start { target, prompt } => Some((*target, prompt.clone())),
            _ => None,
        })
        .collect()
}

fn finished(ok: bool, cost: f64) -> TurnEvent {
    TurnEvent::Finished {
        ok,
        error: None,
        cost_usd: Some(cost),
        duration_ms: None,
        denied_tools: Vec::new(),
        interrupted: false,
    }
}

fn spec(title: &str) -> WorkerSpec {
    WorkerSpec {
        title: title.into(),
        prompt: format!("do {title}"),
        cwd: format!("/work/{title}"),
        machine: None,
        terminal: false,
        project: Some("leon".into()),
        job: Job::Code,
        choice: Choice {
            agent: "opencode".into(),
            model: Some("cheap-model".into()),
            effort: None,
            tier: Some("cheap".into()),
        },
        mode: None,
    }
}

/// A turn of `target` that answers `text` and ends well.
fn answer(king: &mut Coordinator, target: Target, text: &str, at: u64) -> Vec<Effect> {
    let mut effects = king.event(
        target,
        &TurnEvent::Started {
            session_id: format!("s-{target:?}"),
            model: None,
        },
        at,
    );
    effects.extend(king.event(target, &TurnEvent::Text { text: text.into() }, at));
    effects.extend(king.event(target, &finished(true, 0.25), at));
    effects.extend(king.event(target, &TurnEvent::Exited { code: 0 }, at));
    effects
}

#[test]
fn what_the_person_says_starts_kings_turn_or_waits_for_it_to_end() {
    let mut king = Coordinator::new(config());
    let effects = king.say("What is the state of leon?", 0);
    assert_eq!(
        starts(&effects),
        [(Target::Brain, "What is the state of leon?".to_owned())]
    );
    assert!(king.state().brain.busy);

    // Said while it works: it waits, and both come together.
    assert!(starts(&king.say("And of zavu?", 1)).is_empty());
    assert!(starts(&king.say("Briefly.", 2)).is_empty());
    let effects = answer(&mut king, Target::Brain, "leon is fine.", 3);
    assert_eq!(
        starts(&effects),
        [(Target::Brain, "And of zavu?\n\nBriefly.".to_owned())]
    );
    let plan = king.plan(Target::Brain).unwrap();
    assert_eq!(plan.resume.as_deref(), Some("s-Brain"));
    assert_eq!(plan.agent, "claude");
    assert_eq!(plan.cwd, None);

    // The person is told when King has answered and nothing more waits.
    let effects = answer(&mut king, Target::Brain, "zavu too.", 4);
    assert!(starts(&effects).is_empty());
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Notify { title, body, .. } if title == "Orchestrator answered" && body == "zavu too."
    )));
    assert!((king.spent_today(4) - 0.5).abs() < 1e-9);
}

#[test]
fn a_worker_runs_in_its_folder_and_king_is_woken_with_its_news() {
    let mut king = Coordinator::new(config());
    let (id, effects) = king.start_worker(spec("fix-login"), 10).unwrap();
    assert_eq!(id, 1);
    assert_eq!(
        starts(&effects),
        [(Target::Worker(1), "do fix-login".to_owned())]
    );
    let plan = king.plan(Target::Worker(1)).unwrap();
    assert_eq!(plan.agent, "opencode");
    assert_eq!(plan.model.as_deref(), Some("cheap-model"));
    assert_eq!(plan.cwd.as_deref(), Some("/work/fix-login"));
    assert!(plan.instructions.unwrap().contains("worker #1"));

    // It finishes: news, gathered for five seconds before King is woken.
    let effects = answer(&mut king, Target::Worker(1), "Fixed and committed.", 20);
    assert!(starts(&effects).is_empty());
    assert_eq!(king.worker(1).unwrap().state, WorkerState::Done);
    assert_eq!(king.due_ms(), Some(5020));
    assert!(starts(&king.tick(5019)).is_empty());
    let woken = starts(&king.tick(5020));
    assert_eq!(woken.len(), 1);
    assert_eq!(woken[0].0, Target::Brain);
    assert!(woken[0].1.starts_with("[Leon] News of your workers:"));
    assert!(woken[0]
        .1
        .contains("#1 \"fix-login\" (opencode, cheap-model) finished"));
    assert!(woken[0].1.contains("Fixed and committed."));
    assert!(king.state().news.is_empty());
    assert_eq!(king.due_ms(), None);
}

#[test]
fn news_comes_with_what_the_person_says_first_and_never_while_king_works() {
    let mut king = Coordinator::new(config());
    king.start_worker(spec("a"), 0).unwrap();
    king.say("hello", 0);
    answer(&mut king, Target::Worker(1), "A is done.", 1);
    // King works: the news waits even when it is due.
    assert!(starts(&king.tick(10_000)).is_empty());
    assert_eq!(king.due_ms(), None);
    let effects = answer(&mut king, Target::Brain, "hi", 10_001);
    let woken = starts(&effects);
    assert_eq!(woken.len(), 1, "{effects:?}");
    assert!(woken[0].1.contains("A is done."));

    // News that is not due yet goes with the person's message.
    answer(&mut king, Target::Brain, "ok", 10_002);
    king.start_worker(spec("b"), 10_003).unwrap();
    answer(&mut king, Target::Worker(2), "B is done.", 10_004);
    let said = starts(&king.say("status?", 10_005));
    assert!(said[0].1.contains("B is done."), "{said:?}");
    assert!(said[0].1.ends_with("status?"));
}

#[test]
fn a_worker_that_asks_is_news_and_kings_answer_reaches_it() {
    let mut king = Coordinator::new(config());
    king.start_worker(spec("a"), 0).unwrap();
    let asked = TurnEvent::Question {
        id: "q1".into(),
        kind: QuestionKind::Permission {
            tool: "Bash".into(),
            summary: "git push".into(),
            detail: String::new(),
            reason: None,
            choices: vec![
                PermissionChoice {
                    id: "once".into(),
                    effect: PermissionEffect::AllowOnce,
                    label: "Allow".into(),
                    scope: None,
                    takes_reason: false,
                },
                PermissionChoice {
                    id: "no".into(),
                    effect: PermissionEffect::Deny,
                    label: "Deny".into(),
                    scope: None,
                    takes_reason: true,
                },
            ],
        },
    };
    king.event(Target::Worker(1), &asked, 1);
    assert_eq!(king.worker(1).unwrap().state, WorkerState::Asking);
    assert!(king.state().news[0].text.contains("git push"));

    let effects = king
        .answer(
            Target::Worker(1),
            "q1",
            &Reply {
                choice: Some("deny".into()),
                text: Some("Orchestrator pushes, not you.".into()),
                ..Reply::default()
            },
        )
        .unwrap();
    assert_eq!(
        effects,
        [Effect::Act {
            target: Target::Worker(1),
            action: Action::Answer(
                "q1".into(),
                Answer::Permission {
                    choice_id: "no".into(),
                    reason: Some("Orchestrator pushes, not you.".into())
                }
            )
        }]
    );
    assert!(king
        .answer(Target::Worker(1), "q1", &Reply::default())
        .is_err());
    assert!(king
        .answer(Target::Worker(1), "q9", &Reply::default())
        .is_err());

    // Answered: no longer news, and working again.
    king.event(
        Target::Worker(1),
        &TurnEvent::QuestionResolved { id: "q1".into() },
        2,
    );
    assert_eq!(king.worker(1).unwrap().state, WorkerState::Working);
    assert!(king.state().news.is_empty());
}

#[test]
fn no_more_workers_than_allowed_work_at_once() {
    let mut king = Coordinator::new(config());
    king.start_worker(spec("a"), 0).unwrap();
    king.start_worker(spec("b"), 0).unwrap();
    let refused = king.start_worker(spec("c"), 0).unwrap_err();
    assert!(refused.contains("2 workers"), "{refused}");
    answer(&mut king, Target::Worker(1), "done", 1);
    assert_eq!(king.start_worker(spec("c"), 2).unwrap().0, 3);
}

#[test]
fn a_worker_is_told_more_stopped_or_released_to_the_person() {
    let mut king = Coordinator::new(config());
    king.start_worker(spec("a"), 0).unwrap();
    // More while it works waits for its turn to end.
    assert!(starts(&king.tell_worker(1, "also add a test", 1).unwrap()).is_empty());
    let effects = answer(&mut king, Target::Worker(1), "done", 2);
    assert_eq!(
        starts(&effects),
        [(Target::Worker(1), "also add a test".to_owned())]
    );
    assert!(king.state().news.is_empty());
    assert!(king.release(1, 3).is_err(), "not while it works");

    let effects = king.stop_worker(1, 3).unwrap();
    assert!(effects.contains(&Effect::Kill {
        target: Target::Worker(1)
    }));
    king.event(Target::Worker(1), &TurnEvent::Exited { code: 143 }, 4);
    assert_eq!(king.worker(1).unwrap().state, WorkerState::Stopped);
    assert!(king.state().news.is_empty(), "a stop is not news");

    let (released, _) = king.release(1, 5).unwrap();
    assert_eq!(
        released.conversation.session_id.as_deref(),
        Some("s-Worker(1)")
    );
    assert!(king.tell_worker(1, "more", 6).is_err());
    assert!(king.status(6).workers.is_empty());
    assert!(king.state().news[0].text.contains("taken over"));
}

#[test]
fn a_turn_that_cannot_start_or_ends_without_a_word_is_a_failure() {
    let mut king = Coordinator::new(config());
    king.start_worker(spec("a"), 0).unwrap();
    king.failed_to_start(Target::Worker(1), "opencode: not found", 1);
    assert_eq!(king.worker(1).unwrap().state, WorkerState::Failed);
    assert!(!king.worker(1).unwrap().conversation.busy);
    assert!(king.state().news[0].text.contains("not found"));

    king.tell_worker(1, "try again", 2).unwrap();
    king.event(Target::Worker(1), &TurnEvent::Exited { code: 1 }, 3);
    assert_eq!(king.worker(1).unwrap().state, WorkerState::Failed);
    assert!(king.state().news[0].text.starts_with("failed"));
}

#[test]
fn what_was_saved_comes_back_and_interrupted_turns_are_news() {
    let mut king = Coordinator::new(config());
    king.say("start", 0);
    king.start_worker(spec("a"), 0).unwrap();
    let saved = serde_json::to_string(king.state()).unwrap();
    let state: State = serde_json::from_str(&saved).unwrap();

    let (mut king, effects) = Coordinator::restored(config(), state, 100_000);
    assert!(starts(&effects).is_empty());
    assert!(!king.state().brain.busy);
    let worker = king.worker(1).unwrap();
    assert!(!worker.conversation.busy);
    assert_eq!(worker.state, WorkerState::Failed);
    // King is woken with it like with any news.
    let woken = starts(&king.tick(105_000));
    assert_eq!(woken.len(), 1);
    assert!(woken[0].1.contains("was interrupted"));
}

#[test]
fn a_new_agent_for_king_is_a_new_conversation_and_a_new_model_is_not() {
    let mut king = Coordinator::new(config());
    king.say("hi", 0);
    answer(&mut king, Target::Brain, "hello", 1);
    let opus = Choice {
        agent: "claude".into(),
        model: Some("opus".into()),
        effort: Some("high".into()),
        tier: None,
    };
    king.set_brain(&opus, None);
    assert_eq!(
        king.plan(Target::Brain).unwrap().resume.as_deref(),
        Some("s-Brain")
    );
    assert_eq!(
        king.plan(Target::Brain).unwrap().model.as_deref(),
        Some("opus")
    );
    let codex = Choice {
        agent: "codex".into(),
        model: None,
        effort: None,
        tier: None,
    };
    king.set_brain(&codex, None);
    assert_eq!(king.plan(Target::Brain).unwrap().resume, None);
    assert_eq!(king.config().brain.agent, "codex");
}

#[test]
fn kings_agent_is_the_one_chosen_last_unless_the_file_was_edited_since() {
    let mut king = Coordinator::new(config());
    let codex = Choice {
        agent: "codex".into(),
        model: Some("gpt".into()),
        effort: None,
        tier: None,
    };
    king.set_brain(&codex, None);
    let state = king.state().clone();
    let (again, _) = Coordinator::restored(config(), state.clone(), 1);
    assert_eq!(again.plan(Target::Brain).unwrap().agent, "codex");
    assert_eq!(again.config().brain.agent, "codex");

    let mut edited = config();
    edited.brain.agent = "opencode".into();
    let (again, _) = Coordinator::restored(edited, state, 1);
    assert_eq!(again.plan(Target::Brain).unwrap().agent, "opencode");
}

#[test]
fn king_never_asks_the_person_before_it_uses_leons_own_tools() {
    let mut king = Coordinator::new(config());
    king.say("go", 0);
    let ask = |tool: &str| TurnEvent::Question {
        id: "q".into(),
        kind: QuestionKind::Permission {
            tool: tool.into(),
            summary: String::new(),
            detail: String::new(),
            reason: None,
            choices: vec![
                PermissionChoice {
                    id: "once".into(),
                    effect: PermissionEffect::AllowOnce,
                    label: "Allow".into(),
                    scope: None,
                    takes_reason: false,
                },
                PermissionChoice {
                    id: "always.0".into(),
                    effect: PermissionEffect::AllowAlways,
                    label: "Always".into(),
                    scope: None,
                    takes_reason: false,
                },
            ],
        },
    };
    for tool in [
        "mcp__leon-king__worker_start",
        "leon-memory.memory_add",
        "leon-tasks/task_list",
    ] {
        let effects = king.event(Target::Brain, &ask(tool), 1);
        assert_eq!(
            effects,
            [Effect::Act {
                target: Target::Brain,
                action: Action::Answer(
                    "q".into(),
                    Answer::Permission {
                        choice_id: "always.0".into(),
                        reason: None
                    }
                )
            }],
            "{tool}"
        );
    }
    assert!(king.state().brain.questions.is_empty());
    // Anything else is the person's to answer, and so is a look-alike.
    for tool in ["Bash", "mcp__leon-kingdom__x"] {
        king.event(Target::Brain, &ask(tool), 2);
    }
    assert_eq!(king.state().brain.questions.len(), 2);
}

#[test]
fn every_turn_is_measured_and_added_to_the_days_counts() {
    let mut king = Coordinator::new(config());
    let day = 20_000 * DAY_MS;
    king.say("plan the release", day);
    let tokens = |input, output| TurnEvent::Usage {
        tokens: leon_turn::Tokens {
            input,
            cached_input: input / 2,
            output,
            reasoning: 0,
            context_window: None,
            context_used: None,
        },
    };
    // Each count replaces the one before it in the turn.
    king.event(Target::Brain, &tokens(1_000, 100), day + 1);
    king.event(Target::Brain, &tokens(4_000, 600), day + 2);
    king.event(
        Target::Brain,
        &TurnEvent::Finished {
            ok: true,
            error: None,
            cost_usd: Some(0.05),
            duration_ms: Some(10_000),
            denied_tools: Vec::new(),
            interrupted: false,
        },
        day + 3,
    );
    king.event(Target::Brain, &TurnEvent::Exited { code: 0 }, day + 4);
    let facts = king.facts(day + 5);
    let turn = facts.turn.unwrap();
    assert_eq!(
        (turn.input, turn.cached_input, turn.output),
        (4_000, 2_000, 600)
    );
    assert_eq!(turn.per_second(), Some(60.0));
    assert_eq!(facts.king_today.total(), 4_600);
    assert_eq!(facts.spent_today_usd, 0.05);

    // A worker's turn without a duration of its own is timed by the clock.
    king.start_worker(spec("a"), day + 10).unwrap();
    king.event(Target::Worker(1), &tokens(2_000, 200), day + 11);
    king.event(
        Target::Worker(1),
        &TurnEvent::Exited { code: 0 },
        day + 2_010,
    );
    let worker = &king.worker(1).unwrap().conversation;
    assert_eq!(worker.last_turn.unwrap().duration_ms, 2_000);
    assert_eq!(king.facts(day + 2_011).workers_today.total(), 2_200);
    // A new day starts from nothing.
    assert_eq!(king.facts(day + DAY_MS).king_today.total(), 0);
}

#[test]
fn a_new_conversation_puts_the_old_one_away_and_a_past_one_can_go_on() {
    let mut king = Coordinator::new(config());
    // Nothing said: nothing to put away.
    let (archived, _) = king.new_conversation(0).unwrap();
    assert!(archived.is_none());
    let first = king.status(0).conversation;

    king.say("How does the router work?", 10);
    // Not while King works.
    assert!(king.new_conversation(11).is_err());
    answer(&mut king, Target::Brain, "By tiers.", 12);
    let (archived, effects) = king.new_conversation(20).unwrap();
    let archived = archived.expect("the conversation is put away");
    assert!(effects.contains(&Effect::Changed));
    assert_eq!(archived.past.id, first);
    assert_eq!(archived.past.title, "How does the router work?");
    assert_eq!(archived.past.session_id.as_deref(), Some("s-Brain"));
    assert_eq!((archived.past.started_ms, archived.past.ended_ms), (10, 20));
    assert!(archived.entries.len() >= 2);
    // A fresh conversation: no session, nothing said, a number of its own.
    let now = king.status(20);
    assert_ne!(now.conversation, first);
    assert_eq!(now.brain.session_id, None);
    assert_eq!(now.brain.last_seq, None);
    assert_eq!(king.past().len(), 1);

    // Something said in the new one, then the old one taken up again.
    king.say("Another thing", 30);
    answer(&mut king, Target::Brain, "Done.", 31);
    let (put_away, _) = king
        .resume_conversation(first, archived.entries.clone(), 40)
        .unwrap();
    assert_eq!(
        put_away.expect("the new one is put away").past.title,
        "Another thing"
    );
    let now = king.status(40);
    assert_eq!(now.conversation, first);
    assert_eq!(now.brain.session_id.as_deref(), Some("s-Brain"));
    assert_eq!(
        now.brain.last_seq,
        archived.entries.last().map(|entry| entry.seq)
    );
    // The next turn goes on with the agent's session, after what was kept.
    king.say("And the budget?", 41);
    assert_eq!(
        king.plan(Target::Brain).unwrap().resume.as_deref(),
        Some("s-Brain")
    );
    let ids: Vec<u64> = king.past().iter().map(|past| past.id).collect();
    assert_eq!(ids.len(), 1, "the one taken up is not in the list any more");
    assert!(king.resume_conversation(999, Vec::new(), 50).is_err());
}

#[test]
fn a_worker_whose_turn_was_stopped_waits_rather_than_failing() {
    let mut king = Coordinator::new(config());
    let (id, _) = king.start_worker(spec("restyle"), 0).unwrap();
    let target = Target::Worker(id);
    king.event(
        target,
        &TurnEvent::Started {
            session_id: "s".into(),
            model: None,
        },
        1,
    );
    // Codex ends its turn when a permission is denied.
    king.event(
        target,
        &TurnEvent::Finished {
            ok: false,
            error: None,
            cost_usd: None,
            duration_ms: None,
            denied_tools: Vec::new(),
            interrupted: true,
        },
        2,
    );
    king.event(target, &TurnEvent::Exited { code: 0 }, 2);
    let worker = king.worker(id).unwrap();
    assert_eq!(worker.state, WorkerState::Paused);
    let news = &king.state().news;
    assert!(
        news.iter()
            .any(|news| news.text.contains("tell it how to go on")),
        "{news:?}"
    );
}

#[test]
fn a_workers_routine_question_is_answered_by_leon_and_king_hears_nothing() {
    let mut king = Coordinator::new(config());
    let (id, _) = king.start_worker(spec("style"), 0).unwrap();
    let target = Target::Worker(id);
    let question = |summary: &str| TurnEvent::Question {
        id: "q".into(),
        kind: QuestionKind::Permission {
            tool: "edit".into(),
            summary: summary.into(),
            detail: String::new(),
            reason: None,
            choices: vec![PermissionChoice {
                id: "d.0".into(),
                effect: PermissionEffect::AllowOnce,
                label: String::new(),
                scope: None,
                takes_reason: false,
            }],
        },
    };
    // An edit in its own worktree (`/work/style`): answered, no news.
    let effects = king.event(target, &question("/work/style/app.css"), 1);
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Act { target: t, action: Action::Answer(q, _) } if *t == target && q == "q"
    )));
    assert!(king.state().news.is_empty());
    assert_ne!(king.worker(id).unwrap().state, WorkerState::Asking);
    // An edit of the main checkout: King hears of it.
    king.event(target, &question("/work/other/app.css"), 2);
    assert_eq!(king.worker(id).unwrap().state, WorkerState::Asking);
    assert_eq!(king.state().news.len(), 1);
}

#[test]
fn by_default_kings_conversation_goes_on_however_long_it_grows() {
    let config = crate::config::Config::default();
    assert_eq!(config.limits.compact_at_tokens, 0);
    let mut king = Coordinator::new(config);
    king.say("Restyle the radar", 0);
    let usage = TurnEvent::Usage {
        tokens: leon_turn::Tokens {
            input: 5_000_000,
            ..Default::default()
        },
    };
    king.event(Target::Brain, &usage, 1);
    answer(&mut king, Target::Brain, "Worker #1 does it.", 2);
    assert!(king.compact_if_long(3).is_none());
}

#[test]
fn a_conversation_whose_turn_read_too_much_goes_on_in_a_new_one() {
    let mut config = config();
    config.limits.compact_at_tokens = 1_000;
    let mut king = Coordinator::new(config);
    king.say("Restyle the radar", 0);
    let usage = TurnEvent::Usage {
        tokens: leon_turn::Tokens {
            input: 5_000,
            ..Default::default()
        },
    };
    king.event(Target::Brain, &usage, 1);
    answer(&mut king, Target::Brain, "Worker #1 does it.", 2);
    let before = king.status(2).conversation;
    let archived = king.compact_if_long(3).expect("put away");
    assert_eq!(archived.past.title, "Restyle the radar");
    assert_ne!(king.status(3).conversation, before);
    assert_eq!(king.status(3).brain.session_id, None);
    // The next turn is told where it left off, before what is said.
    let effects = king.say("And now?", 4);
    let started = starts(&effects);
    assert_eq!(started.len(), 1);
    assert!(
        started[0].1.starts_with("[Leon] This conversation goes on"),
        "{}",
        started[0].1
    );
    assert!(started[0].1.contains("Worker #1 does it."));
    assert!(started[0].1.ends_with("And now?"));
    // A short turn is left alone.
    answer(&mut king, Target::Brain, "Fine.", 5);
    assert!(king.compact_if_long(6).is_none());
}

#[test]
fn a_workers_check_is_kept_as_evidence_and_king_hears_how_it_ended() {
    let mut king = Coordinator::new(config());
    king.start_worker(spec("a"), 0).unwrap();
    answer(&mut king, Target::Worker(1), "Done and committed.", 10);
    king.tick(10_000);
    answer(&mut king, Target::Brain, "Checking it.", 10_000);

    king.check_started(1, "cargo test", "0123456789", false, 20_000)
        .unwrap();
    // One at a time.
    assert!(king
        .check_started(1, "cargo test", "0123456789", false, 20_001)
        .is_err());
    assert!(king.check_started(9, "cargo test", "x", false, 0).is_err());

    let output = "running 3 tests\ntest a ... FAILED\nerror: 1 test failed\n";
    king.check_finished(1, false, output, 80_000);
    let check = king.worker(1).unwrap().check.clone().unwrap();
    assert_eq!(check.state, CheckState::Failed);
    assert_eq!(check.duration_ms, Some(60_000));
    assert!(!check.proves("0123456789"));
    let woken = starts(&king.tick(85_000));
    assert_eq!(woken.len(), 1);
    assert!(woken[0]
        .1
        .contains("failed its check: `cargo test` on 0123456"));
    assert!(woken[0].1.contains("error: 1 test failed"));
    answer(&mut king, Target::Brain, "Mending.", 85_000);

    // An end heard twice, or of no check, changes nothing.
    assert!(king.check_finished(1, true, "", 90_000).is_empty());

    king.check_started(1, "cargo test", "abcdef1234", false, 100_000)
        .unwrap();
    king.check_finished(1, true, "ok\n", 130_000);
    assert!(king
        .worker(1)
        .unwrap()
        .check
        .as_ref()
        .unwrap()
        .proves("abcdef1"));
    let woken = starts(&king.tick(135_000));
    assert!(woken[0]
        .1
        .contains("passed its check: `cargo test` on abcdef1, in 30s"));
}

#[test]
fn a_pull_requests_ci_is_watched_until_it_settles() {
    let mut king = Coordinator::new(config());
    king.start_worker(spec("a"), 0).unwrap();
    answer(&mut king, Target::Worker(1), "Done.", 10);
    king.tick(10_000);
    answer(&mut king, Target::Brain, "Opening it.", 10_000);

    let url = "https://github.com/zavudev/leon/pull/62";
    king.pr_opened(1, url, "king/a", 20_000).unwrap();
    assert_eq!(
        king.watched_prs(),
        [(1, url.to_owned(), "/work/a".to_owned())]
    );
    let seen = |ci, failing: &[&str]| Seen {
        ci,
        failing: failing.iter().map(|name| (*name).to_owned()).collect(),
        closed: None,
    };
    // Still running: nothing to say.
    assert!(king.ci_seen(1, seen(Ci::Pending, &[]), 80_000).is_empty());

    let effects = king.ci_seen(1, seen(Ci::Failed, &["clippy"]), 140_000);
    assert!(effects.contains(&Effect::Changed));
    assert!(king.watched_prs().is_empty());
    let woken = starts(&king.tick(145_000));
    assert!(woken[0]
        .1
        .contains("CI failing on its pull request #62: clippy"));
    answer(&mut king, Target::Brain, "Mending.", 145_000);

    // Pushed again, and it passes: the person is told it is theirs.
    king.pr_opened(1, url, "king/a", 200_000).unwrap();
    let effects = king.ci_seen(1, seen(Ci::Passed, &[]), 260_000);
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Notify { title, .. } if title == "#62 is ready for your review"
    )));
    let woken = starts(&king.tick(265_000));
    assert!(woken[0].1.contains("every CI check passed"));
    answer(&mut king, Target::Brain, "Told them.", 265_000);

    // A pull request that never shows a check: its project runs no CI.
    king.pr_opened(1, url, "king/a", 300_000).unwrap();
    assert!(king.ci_seen(1, seen(Ci::Pending, &[]), 360_000).is_empty());
    king.ci_seen(
        1,
        seen(Ci::Pending, &[]),
        300_000 + verify::NO_CI_AFTER_MS + 1,
    );
    let pr = king.worker(1).unwrap().pull_request.clone().unwrap();
    assert_eq!(pr.ci, Ci::None);
    assert!(!pr.watched());
}

#[test]
fn a_check_cut_short_by_a_restart_is_news() {
    let mut king = Coordinator::new(config());
    king.start_worker(spec("a"), 0).unwrap();
    answer(&mut king, Target::Worker(1), "Done.", 10);
    king.check_started(1, "make check", "abc", false, 20)
        .unwrap();
    let state: State = serde_json::from_str(&serde_json::to_string(king.state()).unwrap()).unwrap();
    let (king, effects) = Coordinator::restored(config(), state, 100_000);
    let check = king.worker(1).unwrap().check.clone().unwrap();
    assert_eq!(check.state, CheckState::Failed);
    // King is woken at once: its news waited long already.
    let woken = starts(&effects);
    assert!(woken[0]
        .1
        .contains("had its check (`make check`) cut short"));
}

#[test]
fn a_worker_belongs_to_the_conversation_that_started_it() {
    let mut config = config();
    config.limits.compact_at_tokens = 1_000;
    let mut king = Coordinator::new(config);
    let first = king.status(0).conversation;
    king.say("Restyle the radar", 0);
    king.start_worker(spec("a"), 0).unwrap();
    assert_eq!(king.worker(1).unwrap().started_in, Some(first));
    answer(&mut king, Target::Brain, "Worker #1 does it.", 1);

    king.new_conversation(10).unwrap();
    let second = king.status(10).conversation;
    king.start_worker(spec("b"), 10).unwrap();
    assert_eq!(king.worker(2).unwrap().started_in, Some(second));
    assert_eq!(king.worker(1).unwrap().started_in, Some(first));

    // A conversation that goes on in a new one takes its workers along.
    king.say("go on", 20);
    king.event(
        Target::Brain,
        &TurnEvent::Usage {
            tokens: leon_turn::Tokens {
                input: 5_000,
                ..Default::default()
            },
        },
        21,
    );
    answer(&mut king, Target::Brain, "Fine.", 22);
    king.compact_if_long(23).expect("put away");
    let third = king.status(23).conversation;
    assert_eq!(king.worker(2).unwrap().started_in, Some(third));
    assert_eq!(king.worker(1).unwrap().started_in, Some(first));

    // Saved before workers knew theirs: the one that ran when they started.
    let mut state = king.state().clone();
    for worker in state.workers.values_mut() {
        worker.started_in = None;
    }
    let (king, _) = Coordinator::restored(Config::default(), state, 30);
    let past_of = |at: u64| {
        king.past()
            .iter()
            .find(|past| (past.started_ms..=past.ended_ms).contains(&at))
            .map(|past| past.id)
    };
    assert_eq!(king.worker(1).unwrap().started_in, past_of(0));
    assert_eq!(past_of(0), Some(first));
}

fn run_spec(budget_usd: Option<f64>) -> crate::run::RunSpec {
    let step = |task: &str, needs: &[&str]| crate::run::StepSpec {
        task: task.into(),
        title: format!("Billing {task}"),
        body: String::new(),
        needs: needs.iter().map(|need| (*need).to_owned()).collect(),
        tier: None,
        job: Job::Code,
    };
    crate::run::RunSpec {
        task: "goal".into(),
        title: "Ship billing".into(),
        body: String::new(),
        project: "leon".into(),
        steps: vec![step("api", &["schema"]), step("schema", &[])],
        max_parallel: None,
        budget_usd,
    }
}

/// Gives the step at `index` of run 1 a worker, as the service does.
fn start_step(king: &mut Coordinator, index: usize, at: u64) -> u32 {
    let task = king.runs()[&1].steps[index].task.clone();
    let (worker, _) = king.start_worker(spec(&task), at).unwrap();
    king.step_started(1, index, worker, &format!("king/run-1-{task}"), at);
    worker
}

fn boards(effects: &[Effect]) -> Vec<(String, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Board { task, status } => Some((task.clone(), status.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_run_starts_each_step_once_its_needs_are_checked_and_wakes_king_when_done() {
    let mut king = Coordinator::new(config());
    let (id, effects) = king.start_run(run_spec(None), 0).unwrap();
    assert_eq!(id, 1);
    assert_eq!(boards(&effects), [("goal".to_owned(), "doing".to_owned())]);
    assert!(
        king.start_run(run_spec(None), 0).is_err(),
        "one run of a task"
    );

    // The schema first: the API needs it.
    let (ready, _) = king.steps_to_start(1);
    assert_eq!(ready, [(1, 0)]);
    assert_eq!(king.runs()[&1].steps[0].task, "schema");
    let schema = start_step(&mut king, 0, 1);
    assert!(king.steps_to_start(2).0.is_empty());

    // Its worker finishes: Leon commits and checks, King hears nothing.
    let effects = answer(&mut king, Target::Worker(schema), "Added the tables.", 10);
    assert!(effects.contains(&Effect::Verify { worker: schema }));
    assert!(king.state().news.is_empty());
    king.check_started(schema, "cargo test", "abc1234", false, 11)
        .unwrap();
    let effects = king.check_finished(schema, true, "ok", 20);
    assert_eq!(boards(&effects), [("schema".to_owned(), "done".to_owned())]);
    assert!(king.state().news.is_empty());

    // Then the API, which fails its check once and is told to mend it.
    assert_eq!(king.steps_to_start(21).0, [(1, 1)]);
    let api = start_step(&mut king, 1, 21);
    answer(&mut king, Target::Worker(api), "Wrote the API.", 30);
    king.check_started(api, "cargo test", "def5678", false, 31)
        .unwrap();
    let effects = king.check_finished(api, false, "test billing::total failed", 40);
    let told = starts(&effects);
    assert_eq!(told.len(), 1);
    assert!(told[0].1.contains("billing::total failed"), "{told:?}");
    assert!(king.state().news.is_empty(), "a mended check is not news");
    answer(&mut king, Target::Worker(api), "Fixed the total.", 50);
    king.check_started(api, "cargo test", "fed9876", false, 51)
        .unwrap();
    let effects = king.check_finished(api, true, "ok", 60);
    assert_eq!(
        boards(&effects),
        [
            ("api".to_owned(), "done".to_owned()),
            ("goal".to_owned(), "review".to_owned())
        ]
    );
    assert_eq!(king.runs()[&1].state, crate::run::RunState::Done);
    let news = &king.state().news;
    assert_eq!(news.len(), 1);
    assert_eq!(news[0].run, Some(1));
    assert!(news[0].text.contains("king/run-1-api"), "{}", news[0].text);
    let effects = king.tick(10_000);
    assert!(starts(&effects)[0]
        .1
        .contains("Run #1 of task \"Ship billing\" is done"));
}

#[test]
fn a_failed_step_blocks_its_run_until_king_goes_on_or_calls_it_off() {
    let mut config = config();
    config.limits.run_fixes = 0;
    let mut king = Coordinator::new(config);
    king.start_run(run_spec(None), 0).unwrap();
    let schema = start_step(&mut king, 0, 1);
    answer(&mut king, Target::Worker(schema), "Done.", 10);
    king.check_started(schema, "cargo test", "abc1234", false, 11)
        .unwrap();
    king.check_finished(schema, false, "it broke", 20);
    let run = &king.runs()[&1];
    assert_eq!(run.state, crate::run::RunState::Blocked);
    assert_eq!(run.steps[0].state, crate::run::StepState::Failed);
    assert!(king.steps_to_start(21).0.is_empty());
    assert!(king.state().news[0].text.contains("it broke"));

    // King says how to go on: the same worker is told, and the step works.
    let effects = king
        .run_continue(1, Some("Use the old column name."), None, 30)
        .unwrap();
    assert_eq!(
        starts(&effects),
        [(
            Target::Worker(schema),
            "Use the old column name.".to_owned()
        )]
    );
    assert_eq!(king.runs()[&1].state, crate::run::RunState::Running);

    // Called off: its worker stops and its tasks go back to the column.
    let effects = king.run_cancel(1, 40).unwrap();
    assert!(effects.contains(&Effect::Kill {
        target: Target::Worker(schema)
    }));
    assert_eq!(
        boards(&effects),
        [
            ("schema".to_owned(), "todo".to_owned()),
            ("goal".to_owned(), "todo".to_owned())
        ]
    );
    assert!(king.steps_to_start(41).0.is_empty());
    assert!(
        king.start_run(run_spec(None), 50).is_ok(),
        "a new run of it"
    );
}

#[test]
fn a_run_whose_workers_spent_its_budget_starts_nothing_more() {
    let mut king = Coordinator::new(config());
    king.start_run(run_spec(Some(0.2)), 0).unwrap();
    let schema = start_step(&mut king, 0, 1);
    // A turn costs 0.25 here: more than the budget.
    answer(&mut king, Target::Worker(schema), "Done.", 10);
    king.check_started(schema, "cargo test", "abc1234", false, 11)
        .unwrap();
    king.check_finished(schema, true, "ok", 20);
    let (ready, _) = king.steps_to_start(21);
    assert!(ready.is_empty());
    assert_eq!(king.runs()[&1].state, crate::run::RunState::Blocked);
    assert!(king.state().news[0].text.contains("budget"));
    king.run_continue(1, None, Some(5.0), 30).unwrap();
    assert_eq!(king.steps_to_start(31).0, [(1, 1)]);
}

#[test]
fn a_step_interrupted_by_a_restart_goes_on_by_itself() {
    let mut king = Coordinator::new(config());
    king.start_run(run_spec(None), 0).unwrap();
    let schema = start_step(&mut king, 0, 1);
    let saved = king.state().clone();
    let (mut king, effects) = Coordinator::restored(config(), saved, 100);
    let told = starts(&effects);
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].0, Target::Worker(schema));
    assert!(told[0].1.contains("restarted"), "{told:?}");
    assert!(king.state().news.is_empty());

    // One that was being checked is checked again.
    answer(&mut king, Target::Worker(schema), "Done.", 110);
    let saved = king.state().clone();
    let (_, effects) = Coordinator::restored(config(), saved, 200);
    assert!(effects.contains(&Effect::Verify { worker: schema }));
}

#[test]
fn a_workers_checks_and_failures_go_on_its_models_record() {
    let mut king = Coordinator::new(config());
    let (one, _) = king.start_worker(spec("one"), 0).unwrap();
    answer(&mut king, Target::Worker(one), "Done.", 60_000);
    assert!(king.record().is_empty() || king.record()[0].judged == 0);
    king.check_started(one, "cargo test", "0123456789", false, 61_000)
        .unwrap();
    king.check_finished(one, false, "1 failed", 62_000);
    king.check_started(one, "cargo test", "0123456789", false, 63_000)
        .unwrap();
    king.check_finished(one, true, "ok", 64_000);
    let (two, _) = king.start_worker(spec("two"), 70_000).unwrap();
    king.failed_to_start(Target::Worker(two), "no such model", 71_000);

    let record = king.record();
    assert_eq!(record.len(), 1, "one model, one kind of job: {record:?}");
    let summary = &record[0];
    assert_eq!(
        (
            summary.agent.as_str(),
            summary.model.as_deref(),
            summary.job.as_str()
        ),
        ("opencode", Some("cheap-model"), "code")
    );
    assert_eq!(
        (
            summary.workers,
            summary.judged,
            summary.did_well,
            summary.failed
        ),
        (2, 2, 1, 1)
    );
    assert_eq!((summary.checked, summary.first_try), (1, 0));
    assert_eq!(summary.minutes, Some(64_000.0 / 60_000.0));
    // It is kept with the rest of the state.
    let saved = serde_json::to_string(king.state()).unwrap();
    let state: State = serde_json::from_str(&saved).unwrap();
    assert_eq!(state.record.len(), 2);
}

#[test]
fn a_worker_in_a_terminal_gets_no_turn_and_cannot_be_told_more() {
    let mut king = Coordinator::new(config());
    let kimi = WorkerSpec {
        terminal: true,
        choice: Choice {
            agent: "kimi".into(),
            model: None,
            effort: None,
            tier: None,
        },
        ..spec("schema")
    };
    let (id, effects) = king.start_worker(kimi, 0).unwrap();
    assert!(starts(&effects).is_empty(), "Leon runs no turn of it");
    let worker = king.worker(id).unwrap();
    assert!(worker.terminal && worker.state == WorkerState::Working);
    let asked = worker.conversation.transcript.since(None, 10);
    assert_eq!(
        (asked.len(), asked[0].text.as_str()),
        (1, "do schema"),
        "what it was asked is in its transcript"
    );
    assert_eq!(king.working(), 0, "it takes no place of a worker Leon runs");
    let refused = king.tell_worker(id, "and a test", 1_000).unwrap_err();
    assert!(refused.contains("terminal"), "{refused}");
    king.check_started(id, "cargo test", "0123456789", false, 2_000)
        .unwrap();
    king.check_finished(id, true, "ok", 3_000);
    let record = king.record();
    assert_eq!((record[0].did_well, record[0].cost_usd), (1, None));
}

/// The events of King's turns and of a worker's, in a fixed order, with
/// every kind of thing a transcript keeps. The phone and the remote read
/// `Transcript` as these bytes, so what the event log (`crate::log`) does
/// beside it must never change them.
fn golden_run() -> Coordinator {
    use crate::report::{Report, TurnStats};
    use leon_turn::{ChoiceOption, ChoiceQuestion, Tokens};

    let mut king = Coordinator::new(config());
    let at = |event: TurnEvent, t: u64, king: &mut Coordinator| {
        king.event(Target::Brain, &event, t);
    };
    king.say("What is the state of leon?", 1_000);
    at(
        TurnEvent::Started {
            session_id: "s-1".into(),
            model: Some("opus".into()),
        },
        1_001,
        &mut king,
    );
    at(TurnEvent::Thought { text: "hm".into() }, 1_002, &mut king);
    at(
        TurnEvent::TextDelta {
            text: "Look".into(),
        },
        1_003,
        &mut king,
    );
    at(
        TurnEvent::TextDelta { text: "ing".into() },
        1_004,
        &mut king,
    );
    at(
        TurnEvent::Text {
            text: "Looking.".into(),
        },
        1_005,
        &mut king,
    );
    at(
        TurnEvent::ToolUse {
            id: "t1".into(),
            name: "Bash".into(),
            summary: "cargo test".into(),
            detail: "cargo test --all".into(),
        },
        1_006,
        &mut king,
    );
    at(
        TurnEvent::ToolResult {
            id: "t1".into(),
            ok: true,
            summary: "ok".into(),
            detail: "all green".into(),
        },
        1_007,
        &mut king,
    );
    at(
        TurnEvent::ToolResult {
            id: "t2".into(),
            ok: false,
            summary: "exit 1".into(),
            detail: String::new(),
        },
        1_008,
        &mut king,
    );
    at(
        TurnEvent::Usage {
            tokens: Tokens {
                input: 1_200,
                output: 300,
                ..Tokens::default()
            },
        },
        1_009,
        &mut king,
    );
    at(
        TurnEvent::Question {
            id: "q1".into(),
            kind: QuestionKind::Choices {
                questions: vec![ChoiceQuestion {
                    id: "c1".into(),
                    header: None,
                    question: "Which one?".into(),
                    options: vec![
                        ChoiceOption {
                            label: "a".into(),
                            description: None,
                        },
                        ChoiceOption {
                            label: "b".into(),
                            description: Some("the other".into()),
                        },
                    ],
                    multi_select: false,
                    allow_other: false,
                }],
            },
        },
        1_010,
        &mut king,
    );
    at(
        TurnEvent::QuestionResolved { id: "q1".into() },
        1_011,
        &mut king,
    );
    at(finished(true, 0.5), 1_012, &mut king);
    at(TurnEvent::Exited { code: 0 }, 1_013, &mut king);
    king.annotate(
        Target::Brain,
        Report {
            turn: Some(TurnStats {
                input: 1_200,
                output: 300,
                duration_ms: 12_000,
                ..TurnStats::default()
            }),
            spent_today_usd: 0.5,
            ..Report::default()
        },
        1_014,
    );
    // A turn whose answer was only ever a draft, and that failed.
    king.say("Again.", 2_000);
    at(
        TurnEvent::TextDelta {
            text: "Half".into(),
        },
        2_001,
        &mut king,
    );
    at(
        TurnEvent::Finished {
            ok: false,
            error: Some("rate limited".into()),
            cost_usd: None,
            duration_ms: None,
            denied_tools: Vec::new(),
            interrupted: false,
        },
        2_002,
        &mut king,
    );
    at(TurnEvent::Exited { code: 1 }, 2_003, &mut king);
    // A worker, stopped by the person.
    let (id, _) = king.start_worker(spec("fix"), 3_000).unwrap();
    let worker = Target::Worker(id);
    king.event(
        worker,
        &TurnEvent::Text {
            text: "On it.".into(),
        },
        3_001,
    );
    king.event(
        worker,
        &TurnEvent::ToolUse {
            id: "w1".into(),
            name: "Edit".into(),
            summary: "src/lib.rs".into(),
            detail: String::new(),
        },
        3_002,
    );
    king.event(
        worker,
        &TurnEvent::Finished {
            ok: false,
            error: None,
            cost_usd: None,
            duration_ms: None,
            denied_tools: Vec::new(),
            interrupted: true,
        },
        3_003,
    );
    king.event(worker, &TurnEvent::Exited { code: 0 }, 3_004);
    king.failed_to_start(worker, "no such agent", 3_005);
    king
}

/// `Transcript`'s reply for King, as the phone and the remote were given it
/// before the event log existed.
const GOLDEN_BRAIN: &str = r#"[{"seq":0,"at_ms":1000,"kind":"prompt","text":"What is the state of leon?"},{"seq":1,"at_ms":1005,"kind":"agent","text":"Looking."},{"seq":2,"at_ms":1006,"kind":"tool","text":"Bash: cargo test"},{"seq":3,"at_ms":1008,"kind":"tool","text":"failed: exit 1"},{"seq":4,"at_ms":1010,"kind":"question","text":"[q1] Which one? Options: a | b."},{"seq":5,"at_ms":1014,"kind":"report","text":"This answer: 1.5k tokens (read 1.2k, 0 cached · wrote 300) · 25 tok/s · 12s\nToday: 0 tokens (Orchestrator 0, workers 0) · $0.50\nUsage windows: none read yet (open Leon's usage meters once)","report":{"turn":{"input":1200,"cached_input":0,"output":300,"duration_ms":12000,"cost_usd":null},"workers_today":{"input":0,"cached_input":0,"output":0},"king_today":{"input":0,"cached_input":0,"output":0},"spent_today_usd":0.5,"budget_usd":null,"usage":[]}},{"seq":6,"at_ms":2000,"kind":"prompt","text":"Again."},{"seq":7,"at_ms":2002,"kind":"agent","text":"Half"},{"seq":8,"at_ms":2002,"kind":"notice","text":"The turn failed: rate limited"}]"#;

/// The same for the worker of [`golden_run`].
const GOLDEN_WORKER: &str = r#"[{"seq":0,"at_ms":3000,"kind":"prompt","text":"do fix"},{"seq":1,"at_ms":3001,"kind":"agent","text":"On it."},{"seq":2,"at_ms":3002,"kind":"tool","text":"Edit: src/lib.rs"},{"seq":3,"at_ms":3003,"kind":"notice","text":"The turn was stopped."},{"seq":4,"at_ms":3005,"kind":"notice","text":"The agent could not be started: no such agent"}]"#;

#[test]
fn the_transcript_clients_read_is_byte_for_byte_what_it_always_was() {
    let king = golden_run();
    let id = king.state().workers.keys().next().copied().unwrap();
    let json = |target| {
        serde_json::to_string(
            &king
                .conversation(target)
                .unwrap()
                .transcript
                .since(None, 50),
        )
        .unwrap()
    };
    assert_eq!(json(Target::Brain), GOLDEN_BRAIN);
    assert_eq!(json(Target::Worker(id)), GOLDEN_WORKER);
}

/// What `take_log` gave, numbered the way the service numbers it, as the
/// records of one log.
fn numbered(batches: &[Batch], of: Target, home: u64) -> Vec<crate::log::Logged> {
    let mut seq = 0;
    let mut logged = Vec::new();
    for batch in batches
        .iter()
        .filter(|batch| batch.target == of && batch.conversation == home)
    {
        for out in &batch.items {
            match out {
                crate::log::Out::Seed(entries) => {
                    for entry in entries {
                        logged.push(crate::log::Logged {
                            seq,
                            at_ms: entry.at_ms,
                            record: crate::log::Record::Legacy {
                                entry: entry.clone(),
                            },
                        });
                        seq += 1;
                    }
                }
                crate::log::Out::Record { at_ms, record } => {
                    logged.push(crate::log::Logged {
                        seq,
                        at_ms: *at_ms,
                        record: record.clone(),
                    });
                    seq += 1;
                }
            }
        }
    }
    logged
}

fn followed(logged: Vec<crate::log::Logged>) -> crate::follow::Followed {
    let mut followed = crate::follow::Followed::default();
    followed.apply(&crate::log::Events {
        conversation: 0,
        last_seq: logged.last().map(|logged| logged.seq),
        events: logged,
        more: false,
    });
    followed
}

#[test]
fn the_event_log_holds_what_the_transcript_leaves_out() {
    use leon_conversation::model::Item;
    let mut king = golden_run();
    let id = *king.state().workers.keys().next().unwrap();
    let batches = king.take_log();
    assert!(king.take_log().is_empty(), "taken once");
    assert!(batches.iter().all(|batch| batch.conversation == 0));

    let brain = followed(numbered(&batches, Target::Brain, 0));
    let items = &brain.conversation.items;
    // The person's words, what it thought, the whole message (not the
    // deltas), the tool with how it went, and the end of the second turn.
    assert!(matches!(&items[0], Item::User { text, .. } if text == "What is the state of leon?"));
    assert!(matches!(&items[1], Item::Thought { text } if text == "hm"));
    assert!(matches!(&items[2], Item::Agent { text, streaming: false, .. } if text == "Looking."));
    let Item::Tool(tool) = &items[3] else {
        panic!("{:?}", items[3]);
    };
    assert_eq!(
        (tool.name.as_str(), tool.detail.as_str()),
        ("Bash", "cargo test --all")
    );
    assert_eq!(
        tool.outcome.as_ref().map(|o| o.detail.as_str()),
        Some("all green")
    );
    assert!(
        items
            .iter()
            .any(|item| matches!(item, Item::Agent { text, .. } if text == "Half"),),
        "a draft left at the end is the message"
    );
    assert!(matches!(
        items.last(),
        Some(Item::Notice { text, .. }) if text == "rate limited"
    ));
    assert_eq!(brain.conversation.cost_usd, Some(0.5));
    assert!(!brain.conversation.running());

    let worker = followed(numbered(&batches, Target::Worker(id), 0));
    assert!(!worker.conversation.running());
    assert!(matches!(
        worker.conversation.items.last(),
        Some(Item::Notice { text, .. }) if text.contains("could not be started")
    ));
}

#[test]
fn what_waits_for_the_turn_to_end_is_taken_back_or_sent_into_the_turn() {
    let mut king = Coordinator::new(config());
    king.say("first", 1);
    king.say("second", 2);
    king.say("third", 3);
    let waiting = |king: &Coordinator| king.status(4).brain.queue;
    assert_eq!(waiting(&king), ["second", "third"]);
    assert_eq!(king.status(4).brain.queued, 2);

    // Taken back only while it is still the message at that place.
    assert_eq!(king.unqueue(Target::Brain, 0, "third"), Ok(false));
    assert_eq!(king.unqueue(Target::Brain, 9, "third"), Ok(false));
    assert_eq!(king.unqueue(Target::Brain, 0, "second"), Ok(true));
    assert_eq!(waiting(&king), ["third"]);

    // Sent now: out of the queue, to the turn in flight.
    let effects = king.steer(Target::Brain, 0, "third").unwrap();
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Act { target: Target::Brain, action: leon_turn::Action::Prompt(text) }
            if text == "third"
    )));
    assert!(waiting(&king).is_empty());
    assert!(king.steer(Target::Brain, 0, "third").is_err(), "gone");

    // Once the turn took it, it was said in the conversation; if it did not
    // it waits again, first.
    king.said(Target::Brain, "third", 6);
    king.say("fourth", 7);
    king.requeue(Target::Brain, "fifth".into());
    assert_eq!(waiting(&king), ["fifth", "fourth"]);
    let batches = king.take_log();
    let brain = followed(numbered(&batches, Target::Brain, 0));
    assert!(brain.conversation.items.iter().any(|item| matches!(
        item,
        leon_conversation::model::Item::User { text, .. } if text == "third"
    )));

    // With no turn in flight there is nothing to send it into.
    let mut idle = Coordinator::new(config());
    idle.say("a", 1);
    answer(&mut idle, Target::Brain, "done", 2);
    assert!(idle.steer(Target::Brain, 0, "a").is_err());
}

#[test]
fn a_workers_queue_is_taken_back_like_kings_and_a_stranger_is_refused() {
    let mut king = Coordinator::new(config());
    let (id, _) = king.start_worker(spec("fix-login"), 10).unwrap();
    king.tell_worker(id, "more", 11).unwrap();
    let queue = |king: &Coordinator| king.status(12).workers[0].conversation.queue.clone();
    assert_eq!(queue(&king), ["more"]);
    assert_eq!(king.unqueue(Target::Worker(id), 0, "more"), Ok(true));
    assert!(queue(&king).is_empty());
    assert!(king.unqueue(Target::Worker(99), 0, "more").is_err());
}

#[test]
fn a_workers_checkpoint_is_logged_after_the_prompt_of_its_turn() {
    let mut king = Coordinator::new(config());
    let (id, _) = king.start_worker(spec("fix-login"), 10).unwrap();
    let target = Target::Worker(id);
    assert_eq!(king.plan(target).unwrap().turn, 0);
    king.checkpointed(target, "abc123", 11);
    answer(&mut king, target, "Done.", 20);
    assert_eq!(king.plan(target).unwrap().turn, 1, "one turn is behind it");

    let batches = king.take_log();
    let worker = followed(numbered(&batches, target, 0));
    assert_eq!(worker.turns.len(), 1);
    assert_eq!(worker.turns[0].checkpoint.as_deref(), Some("abc123"));
    assert!(matches!(
        &worker.conversation.items[worker.turns[0].item],
        leon_conversation::model::Item::User { .. }
    ));
}

#[test]
fn a_conversation_put_away_hands_over_its_log_before_the_new_one_begins() {
    let mut king = Coordinator::new(config());
    king.say("first", 1);
    answer(&mut king, Target::Brain, "one", 2);
    king.event(
        Target::Brain,
        &TurnEvent::Thought {
            text: "late".into(),
        },
        3,
    );
    let (archived, _) = king.new_conversation(10).unwrap();
    assert!(archived.is_some());
    king.say("second", 11);
    let batches = king.take_log();
    let homes: Vec<(Target, u64)> = batches
        .iter()
        .map(|batch| (batch.target, batch.conversation))
        .collect();
    assert_eq!(homes, [(Target::Brain, 0), (Target::Brain, 1)]);
    let first = followed(numbered(&batches, Target::Brain, 0));
    assert_eq!(
        first.conversation.items.len(),
        3,
        "{:?}",
        first.conversation.items
    );
    let second = followed(numbered(&batches, Target::Brain, 1));
    assert_eq!(second.conversation.items.len(), 1);
}

#[test]
fn a_worker_keeps_its_log_where_it_began_when_the_conversation_goes_on() {
    let mut config = config();
    config.limits.compact_at_tokens = 1_000;
    let mut king = Coordinator::new(config);
    king.say("go", 0);
    king.start_worker(spec("a"), 0).unwrap();
    king.event(
        Target::Brain,
        &TurnEvent::Usage {
            tokens: leon_turn::Tokens {
                input: 5_000,
                ..Default::default()
            },
        },
        1,
    );
    answer(&mut king, Target::Brain, "ok", 2);
    king.compact_if_long(3).expect("put away");
    assert_eq!(king.worker(1).unwrap().started_in, Some(1));
    assert_eq!(king.log_home(Target::Worker(1)), Some(0));
    assert_eq!(king.log_home(Target::Brain), Some(1));
    assert_eq!(king.log_home(Target::Worker(9)), None);
    // And it is the same after what was saved comes back.
    let state: State = serde_json::from_str(&serde_json::to_string(king.state()).unwrap()).unwrap();
    let (king, _) = Coordinator::restored(config_with_compaction(), state, 9);
    assert_eq!(king.log_home(Target::Worker(1)), Some(0));
}

fn config_with_compaction() -> Config {
    let mut config = config();
    config.limits.compact_at_tokens = 1_000;
    config
}

#[test]
fn what_was_kept_before_the_log_is_its_first_record_and_the_interruption_its_next() {
    use crate::log::{Out, Record};
    let mut king = Coordinator::new(config());
    king.say("start", 5_000);
    king.start_worker(spec("a"), 5_000).unwrap();
    let state: State = serde_json::from_str(&serde_json::to_string(king.state()).unwrap()).unwrap();
    let (mut king, _) = Coordinator::restored(config(), state, 100_000);
    let batches = king.take_log();
    let brain = &batches
        .iter()
        .find(|batch| batch.target == Target::Brain)
        .unwrap()
        .items;
    let Out::Seed(entries) = &brain[0] else {
        panic!("{brain:?}");
    };
    assert_eq!(entries[0].text, "start");
    assert!(matches!(
        &brain[1],
        Out::Record { record: Record::Notice { ends_turn: true, text }, .. }
            if text.contains("interrupted")
    ));
    let worker = batches
        .iter()
        .find(|batch| batch.target == Target::Worker(1))
        .unwrap();
    assert!(matches!(worker.items[0], Out::Seed(_)));
    assert_eq!(worker.conversation, 0);
}

#[test]
fn what_was_asked_of_an_agent_in_a_terminal_begins_no_turn() {
    let mut king = Coordinator::new(config());
    let kimi = WorkerSpec {
        terminal: true,
        choice: Choice {
            agent: "kimi".into(),
            model: None,
            effort: None,
            tier: None,
        },
        ..spec("schema")
    };
    king.start_worker(kimi, 0).unwrap();
    let batches = king.take_log();
    let worker = followed(numbered(&batches, Target::Worker(1), 0));
    assert_eq!(worker.conversation.items.len(), 1);
    assert!(!worker.conversation.running());
}

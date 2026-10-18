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
        Effect::Notify { title, body } if title == "King answered" && body == "zavu too."
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
                text: Some("King pushes, not you.".into()),
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
                    reason: Some("King pushes, not you.".into())
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

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
        Effect::Notify { title, body, .. } if title == "King answered" && body == "zavu too."
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

//! Tests of the lion's motion: a pure function of time, so none of them needs
//! a window or a clock.

use std::time::Duration;

use crate::motion::{
    Ease, Frame, Gesture, GestureSet, Mood, Pose, Side, Timeline, Wake, INTRO_LEN,
};

fn secs(seconds: f64) -> Duration {
    Duration::from_secs_f64(seconds)
}

fn pose_at(timeline: &Timeline, seconds: f64) -> Pose {
    timeline.frame(secs(seconds), None).pose
}

const TEN_MINUTES: f64 = 600.;

/// The gestures a timeline plays in a span, with their start in seconds.
fn played(timeline: &Timeline, until: f64) -> Vec<(f64, Gesture)> {
    timeline
        .gestures(secs(until))
        .into_iter()
        .map(|(start, gesture)| (start.as_secs_f64(), gesture))
        .collect()
}

fn blinks(played: &[(f64, Gesture)]) -> Vec<f64> {
    played
        .iter()
        .filter(|(_, g)| {
            matches!(
                g,
                Gesture::Blink | Gesture::DoubleBlink | Gesture::SlowBlink
            )
        })
        .map(|(start, _)| *start)
        .collect()
}

#[test]
fn the_rest_pose_is_the_identity_of_every_part() {
    let rest = Pose::REST;
    assert_eq!(rest.eye_open, 1.);
    assert_eq!(
        (rest.narrow, rest.glance, rest.lean, rest.nose),
        (0., 0., 0., 0.)
    );
    assert_eq!((rest.scale, rest.plate, rest.glow), (1., 1., 0.));
    assert_eq!(Pose::default(), Pose::REST);
}

#[test]
fn every_gesture_starts_and_ends_at_rest() {
    for gesture in Gesture::catalogue() {
        assert_eq!(gesture.pose(0.), Pose::REST, "{gesture:?} starts at rest");
        assert_eq!(
            gesture.pose(gesture.duration()),
            Pose::REST,
            "{gesture:?} ends at rest"
        );
        assert_eq!(gesture.pose(gesture.duration() + 5.), Pose::REST);
    }
}

#[test]
fn every_gesture_is_short_and_none_is_longer_than_four_seconds() {
    for gesture in Gesture::catalogue() {
        let duration = gesture.duration();
        assert!(duration > 0.1 && duration <= 4., "{gesture:?}: {duration}");
    }
}

#[test]
fn every_easing_is_monotone_and_lands_on_its_ends() {
    for ease in [Ease::In, Ease::Out, Ease::InOut] {
        assert_eq!(ease.apply(0.), 0.);
        assert_eq!(ease.apply(1.), 1.);
        let mut before = 0.;
        for step in 0..=200 {
            let value = ease.apply(step as f32 / 200.);
            assert!(value >= before, "{ease:?} goes back at step {step}");
            before = value;
        }
        assert_eq!(ease.apply(-1.), 0.);
        assert_eq!(ease.apply(2.), 1.);
    }
}

#[test]
fn no_gesture_leaves_the_ranges_the_lion_is_allowed_to_move_in() {
    for gesture in Gesture::catalogue() {
        for step in 0..=200 {
            let pose = gesture.pose(gesture.duration() * step as f32 / 200.);
            assert!((0. ..=1.).contains(&pose.eye_open), "{gesture:?} {pose:?}");
            assert!((0. ..=1.).contains(&pose.narrow));
            assert!(pose.glance.abs() <= 2.);
            assert!(pose.lean.abs() <= 2.);
            assert!(
                (1. ..=1.015).contains(&pose.scale),
                "breathing stays under 1.5 %"
            );
            assert!((0. ..=1.).contains(&pose.nose));
            assert_eq!((pose.plate, pose.glow), (1., 0.));
        }
    }
}

#[test]
fn a_blink_closes_faster_than_it_opens() {
    let blink = Gesture::Blink;
    let duration = blink.duration();
    let samples: Vec<(f32, f32)> = (0..=1000)
        .map(|step| {
            let at = duration * step as f32 / 1000.;
            (at, blink.pose(at).eye_open)
        })
        .collect();
    let closed_at = samples.iter().find(|(_, open)| *open <= 0.05).unwrap().0;
    let reopening_from = samples
        .iter()
        .rev()
        .find(|(_, open)| *open <= 0.05)
        .unwrap()
        .0;
    let closing = closed_at;
    let opening = duration - reopening_from;
    assert!(
        closing < opening,
        "{closing} s to close, {opening} s to open"
    );
    assert!(duration < 0.4, "a blink is quick");
}

#[test]
fn a_double_blink_closes_the_eyes_twice() {
    let gesture = Gesture::DoubleBlink;
    let mut closings = 0;
    let mut was_closed = false;
    for step in 0..=2000 {
        let open = gesture
            .pose(gesture.duration() * step as f32 / 2000.)
            .eye_open;
        let closed = open < 0.2;
        if closed && !was_closed {
            closings += 1;
        }
        was_closed = closed;
    }
    assert_eq!(closings, 2);
}

#[test]
fn the_glare_tightens_holds_and_relaxes() {
    let narrow = Gesture::Narrow;
    let held: Vec<f32> = (0..=1000)
        .map(|step| narrow.pose(narrow.duration() * step as f32 / 1000.).narrow)
        .collect();
    let at_full = held.iter().filter(|value| **value >= 0.999).count();
    assert!(
        at_full as f32 / 1000. * narrow.duration() >= 0.5,
        "the glare holds for half a second at least"
    );
    assert_eq!(held[0], 0.);
    assert_eq!(*held.last().unwrap(), 0.);
}

#[test]
fn a_glance_shifts_the_eyes_and_leans_the_head_the_same_way_and_comes_back() {
    for (side, sign) in [(Side::Left, -1.), (Side::Right, 1.)] {
        let glance = Gesture::Glance(side);
        let held = glance.pose(0.18 + 0.2);
        assert!(held.glance * sign > 1., "{held:?}");
        assert!(held.lean * sign > 0.5 && held.lean.abs() <= 2., "{held:?}");
        assert_eq!(held.eye_open, 1.);
        assert_eq!(glance.pose(glance.duration()), Pose::REST);
    }
}

#[test]
fn the_same_seed_always_gives_the_same_timeline() {
    let a = Timeline::idle(7);
    let b = Timeline::idle(7);
    assert_eq!(played(&a, TEN_MINUTES), played(&b, TEN_MINUTES));
    for seconds in [0.5, 3.3, 12.9, 44.4, 100.1] {
        assert_eq!(a.frame(secs(seconds), None), b.frame(secs(seconds), None));
    }
}

#[test]
fn another_seed_gives_another_timeline() {
    assert_ne!(
        played(&Timeline::idle(7), TEN_MINUTES),
        played(&Timeline::idle(8), TEN_MINUTES)
    );
}

#[test]
fn an_idle_lion_blinks_every_four_to_nine_seconds_give_or_take() {
    let schedule = played(&Timeline::idle(11), TEN_MINUTES);
    let starts = blinks(&schedule);
    let gaps: Vec<f64> = starts.windows(2).map(|pair| pair[1] - pair[0]).collect();
    let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
    assert!((4. ..=9.).contains(&mean), "mean {mean}");
    let longest = gaps.iter().cloned().fold(0., f64::max);
    assert!(
        longest <= 11.,
        "the longest wait without a blink is {longest}"
    );
    let shortest = gaps.iter().cloned().fold(f64::MAX, f64::min);
    assert!(shortest < longest - 1.5, "the intervals are irregular");
}

#[test]
fn now_and_then_an_idle_blink_is_a_double_blink() {
    let schedule = played(&Timeline::idle(3), TEN_MINUTES);
    let doubles = schedule
        .iter()
        .filter(|(_, g)| *g == Gesture::DoubleBlink)
        .count();
    let singles = schedule
        .iter()
        .filter(|(_, g)| *g == Gesture::Blink)
        .count();
    assert!(doubles >= 2, "{doubles}");
    assert!(doubles * 3 < singles, "double blinks are the exception");
}

#[test]
fn an_idle_lion_over_half_an_hour_plays_every_gesture_it_has() {
    let schedule = played(&Timeline::idle(5), 1800.);
    let wanted: [fn(&Gesture) -> bool; 7] = [
        |g| matches!(g, Gesture::Blink),
        |g| matches!(g, Gesture::DoubleBlink),
        |g| matches!(g, Gesture::Narrow),
        |g| matches!(g, Gesture::Glance(Side::Left)),
        |g| matches!(g, Gesture::Glance(Side::Right)),
        |g| matches!(g, Gesture::Breathe),
        |g| matches!(g, Gesture::Twitch),
    ];
    for wanted in wanted {
        assert!(schedule.iter().any(|(_, g)| wanted(g)));
    }
    let twitches = schedule
        .iter()
        .filter(|(_, g)| *g == Gesture::Twitch)
        .count();
    let blinks = schedule
        .iter()
        .filter(|(_, g)| *g == Gesture::Blink)
        .count();
    assert!(twitches * 10 < blinks, "the nose twitch is rare");
}

#[test]
fn gestures_come_one_at_a_time_with_a_rest_between_them() {
    for mood in [Mood::Idle, Mood::Working, Mood::Waiting, Mood::Error] {
        let schedule = played(&Timeline::idle(21).with_mood(mood), TEN_MINUTES);
        assert!(schedule.len() > 20, "{mood:?} does something");
        for pair in schedule.windows(2) {
            let end = pair[0].0 + f64::from(pair[0].1.duration());
            assert!(
                pair[1].0 >= end + 0.2,
                "{mood:?}: {:?} at {} runs into {:?} at {}",
                pair[0].1,
                pair[0].0,
                pair[1].1,
                pair[1].0
            );
        }
    }
}

#[test]
fn the_first_gesture_waits_for_the_intro_to_end() {
    let schedule = played(&Timeline::intro(2), 30.);
    assert!(schedule[0].0 >= INTRO_LEN as f64 + 0.5, "{:?}", schedule[0]);
}

#[test]
fn between_gestures_the_lion_asks_for_a_timer_and_not_for_frames() {
    let timeline = Timeline::idle(9);
    let schedule = played(&timeline, 60.);
    let (first, second) = (schedule[0], schedule[1]);
    let gap_middle = (first.0 + f64::from(first.1.duration()) + second.0) / 2.;
    let frame = timeline.frame(secs(gap_middle), None);
    assert_eq!(frame.pose, Pose::REST);
    assert_eq!(frame.wake, Wake::At(secs(second.0)));
    // Before the first one the same.
    assert_eq!(timeline.frame(secs(0.), None).wake, Wake::At(secs(first.0)));
}

#[test]
fn while_a_part_moves_the_lion_asks_for_the_next_frame() {
    let timeline = Timeline::idle(9);
    let (start, _) = played(&timeline, 60.)[0];
    let during = timeline.frame(secs(start + 0.02), None);
    assert_eq!(during.wake, Wake::NextFrame);
}

#[test]
fn a_hold_in_the_middle_of_a_glare_waits_for_the_end_of_the_hold() {
    // Find a narrow in an idle timeline and look into its hold.
    let timeline = Timeline::idle(5);
    let (start, _) = *played(&timeline, 1800.)
        .iter()
        .find(|(_, g)| *g == Gesture::Narrow)
        .unwrap();
    let during_hold = timeline.frame(secs(start + 0.22 + 0.2), None);
    assert_eq!(during_hold.pose.narrow, 1.);
    let Wake::At(wake) = during_hold.wake else {
        panic!("a hold waits for a timer")
    };
    let expected = secs(start + 0.22 + 0.65);
    assert!(
        wake.abs_diff(expected) < Duration::from_millis(1),
        "{wake:?}"
    );
}

#[test]
fn a_full_minute_of_idle_asks_for_a_small_bounded_number_of_frames() {
    for seed in 0..8 {
        let timeline = Timeline::idle(seed);
        let (mut frames, mut timers) = (0u32, 0u32);
        let mut now = 0.;
        while now < 60. {
            match timeline.frame(secs(now), None).wake {
                Wake::NextFrame => {
                    frames += 1;
                    now += 1. / 60.;
                }
                Wake::At(at) => {
                    timers += 1;
                    now = at.as_secs_f64().max(now + 1e-6);
                }
                Wake::Never => break,
            }
        }
        // At 60 Hz, a quarter of the minute at the very most.
        assert!(frames <= 900, "seed {seed}: {frames} frames");
        assert!(timers <= 60, "seed {seed}: {timers} timers");
    }
}

#[test]
fn an_hour_of_idle_costs_the_same_per_minute_as_the_first_minute() {
    let timeline = Timeline::idle(4);
    let count = |from: f64| {
        let mut frames = 0;
        let mut now = from;
        while now < from + 60. {
            match timeline.frame(secs(now), None).wake {
                Wake::NextFrame => {
                    frames += 1;
                    now += 1. / 60.;
                }
                Wake::At(at) => now = at.as_secs_f64().max(now + 1e-6),
                Wake::Never => break,
            }
        }
        frames
    };
    assert!(count(3600.) <= 900);
    assert!(count(86_400.) <= 900);
}

#[test]
fn the_small_gesture_set_only_blinks_and_narrows() {
    let timeline = Timeline::idle(13).with_gestures(GestureSet::SMALL);
    let schedule = played(&timeline, 1800.);
    assert!(!schedule.is_empty());
    for (_, gesture) in &schedule {
        assert!(
            matches!(
                gesture,
                Gesture::Blink | Gesture::DoubleBlink | Gesture::Narrow
            ),
            "{gesture:?} is not visible at 16 px"
        );
    }
    assert!(schedule.iter().any(|(_, g)| *g == Gesture::Narrow));
}

#[test]
fn a_working_lion_keeps_its_eyes_narrowed_and_scans_left_and_right_in_turn() {
    let timeline = Timeline::idle(1).with_mood(Mood::Working);
    for seconds in [0.0, 0.5, 5.3, 31.7, 200.2] {
        // Past the first moments, always tightened.
        let narrow = pose_at(&timeline, seconds + 1.).narrow;
        assert!(narrow >= 0.5, "narrow {narrow} at {seconds}");
    }
    let scans: Vec<Side> = played(&timeline, TEN_MINUTES)
        .into_iter()
        .filter_map(|(_, g)| match g {
            Gesture::Scan(side) => Some(side),
            _ => None,
        })
        .collect();
    assert!(scans.len() > 50, "it scans steadily: {}", scans.len());
    assert!(
        scans.windows(2).all(|pair| pair[0] != pair[1]),
        "left, right, left..."
    );
    // And it does not play the idle gestures that read as relaxed.
    assert!(played(&timeline, TEN_MINUTES)
        .iter()
        .all(|(_, g)| !matches!(g, Gesture::Breathe | Gesture::Twitch | Gesture::Narrow)));
}

#[test]
fn a_waiting_lion_looks_straight_ahead_blinks_slowly_and_narrows_now_and_then() {
    let timeline = Timeline::idle(6).with_mood(Mood::Waiting);
    let schedule = played(&timeline, TEN_MINUTES);
    assert!(schedule
        .iter()
        .all(|(_, g)| matches!(g, Gesture::SlowBlink | Gesture::Beckon)));
    let beckons: Vec<f64> = schedule
        .iter()
        .filter(|(_, g)| *g == Gesture::Beckon)
        .map(|(start, _)| *start)
        .collect();
    assert!(beckons.len() >= 40, "a prompt for attention, regularly");
    assert!(beckons.windows(2).all(|pair| pair[1] - pair[0] <= 14.));
    assert!(Gesture::SlowBlink.duration() > Gesture::Blink.duration());
    // Its baseline is relaxed and straight.
    assert_eq!(pose_at(&timeline, 0.).glance, 0.);
}

#[test]
fn an_error_makes_one_sharp_narrow_and_holds_it() {
    let timeline = Timeline::idle(2).with_mood(Mood::Error);
    let schedule = played(&timeline, TEN_MINUTES);
    assert_eq!(schedule[0].1, Gesture::Glare);
    assert_eq!(schedule[0].0, 0.);
    assert_eq!(
        schedule
            .iter()
            .filter(|(_, g)| *g == Gesture::Glare)
            .count(),
        1,
        "once"
    );
    // Sharp: fully narrow within a fifth of a second; held for over a second.
    assert!(pose_at(&timeline, 0.2).narrow >= 0.99);
    assert!(pose_at(&timeline, 1.2).narrow >= 0.99);
    assert_eq!(pose_at(&timeline, 3.).narrow, 0.);
}

#[test]
fn an_asleep_lion_has_its_eyes_nearly_closed_and_stops_animating() {
    let timeline = Timeline::idle(2).with_mood(Mood::Asleep);
    for seconds in [0., 1., 30., 3000.] {
        let frame = timeline.frame(secs(seconds), None);
        assert!(frame.pose.eye_open <= 0.15, "{:?}", frame.pose);
        assert!(frame.pose.eye_open > 0., "nearly, not shut");
        assert_eq!(frame.wake, Wake::Never);
    }
    assert!(played(&timeline, TEN_MINUTES).is_empty());
}

#[test]
fn changing_mood_eases_between_the_two_baselines_and_then_rests() {
    let calm = Timeline::idle(2);
    let asleep = calm.mood_changed(Mood::Asleep, secs(10.));
    // Just after the change it is between awake and asleep, and moving.
    let midway = asleep.frame(secs(10.1), None);
    assert!(midway.pose.eye_open < 1. && midway.pose.eye_open > Mood::Asleep.baseline().eye_open);
    assert_eq!(midway.wake, Wake::NextFrame);
    // Settled, nothing moves.
    let settled = asleep.frame(secs(12.), None);
    assert_eq!(settled.pose, Mood::Asleep.baseline());
    assert_eq!(settled.wake, Wake::Never);
    // And waking up again plays gestures again.
    let awake = asleep.mood_changed(Mood::Idle, secs(20.));
    assert!(matches!(
        awake.frame(secs(25.), None).wake,
        Wake::At(_) | Wake::NextFrame
    ));
    assert_eq!(awake.mood(), Mood::Idle);
}

#[test]
fn the_intro_resolves_from_the_eyes_to_the_plate_once_in_under_a_second() {
    let timeline = Timeline::intro(1);
    assert!((0.6..=0.9).contains(&INTRO_LEN));
    // In the dark: only the eyes, glowing and opening.
    let start = pose_at(&timeline, 0.0);
    assert_eq!((start.plate, start.glow), (0., 1.));
    assert!(start.eye_open < 0.1);
    let eyes = pose_at(&timeline, 0.2);
    assert_eq!(eyes.plate, 0.);
    assert!(eyes.eye_open > 0.9 && eyes.glow >= 0.99);
    // Then the plate arrives while the glow gives way to the cuts.
    let middle = pose_at(&timeline, 0.5);
    assert!(middle.plate > 0. && middle.plate < 1.);
    assert!(middle.glow < 1.);
    // Resolved: the rest pose, for good.
    let end = timeline.frame(secs(f64::from(INTRO_LEN) + 0.01), None);
    assert_eq!(end.pose, Pose::REST);
    assert!(matches!(end.wake, Wake::At(_)));
    for later in [1.0, 10., 600.] {
        let pose = pose_at(&timeline, later);
        assert_eq!((pose.plate, pose.glow), (1., 0.), "it does not play again");
    }
    // It keeps asking for frames while it plays.
    assert_eq!(timeline.frame(secs(0.3), None).wake, Wake::NextFrame);
}

#[test]
fn a_lion_without_an_intro_is_at_rest_from_the_first_instant() {
    assert_eq!(pose_at(&Timeline::idle(1), 0.), Pose::REST);
    assert_eq!(Timeline::idle(1).frame(secs(0.), None).pose.plate, 1.);
}

#[test]
fn a_still_lion_is_the_rest_pose_and_asks_for_nothing_at_any_time() {
    let timeline = Timeline::still();
    for seconds in [0., 1., 9.9, 1000.] {
        assert_eq!(timeline.frame(secs(seconds), None), Frame::STILL);
    }
    assert_eq!(Frame::STILL.pose, Pose::REST);
    assert_eq!(Frame::STILL.wake, Wake::Never);
    assert!(played(&timeline, TEN_MINUTES).is_empty());
}

#[test]
fn the_pointer_arriving_plays_a_narrow_that_ends_by_itself() {
    let timeline = Timeline::idle(9);
    // In a long gap of the schedule.
    let schedule = played(&timeline, 60.);
    let start = schedule[0].0 + f64::from(schedule[0].1.duration()) + 0.3;
    let tightening = timeline.frame(secs(start + 0.1), Some(secs(start)));
    assert!(tightening.pose.narrow > 0.1 && tightening.pose.narrow < 1.);
    assert_eq!(tightening.wake, Wake::NextFrame);
    let holding = timeline.frame(secs(start + 0.5), Some(secs(start)));
    assert_eq!(holding.pose.narrow, 1.);
    assert!(matches!(holding.wake, Wake::At(_)));
    let gone = timeline.frame(secs(start + 2.), Some(secs(start)));
    assert_eq!(gone.pose.narrow, 0.);
}

#[test]
fn every_mood_has_a_still_baseline_that_is_the_rest_pose_unless_it_narrows_or_sleeps() {
    assert_eq!(Mood::Idle.baseline(), Pose::REST);
    assert_eq!(Mood::Waiting.baseline(), Pose::REST);
    assert_eq!(Mood::Error.baseline(), Pose::REST);
    assert!(Mood::Working.baseline().narrow >= 0.5);
    assert!(Mood::Asleep.baseline().eye_open <= 0.15);
    assert!(!Mood::Asleep.animates());
    assert!(Mood::Idle.animates() && Mood::Working.animates());
}

#[test]
fn the_small_pose_keeps_only_what_is_visible_at_sixteen_pixels() {
    let busy = Pose {
        eye_open: 0.4,
        narrow: 0.7,
        glance: 1.5,
        lean: 1.2,
        scale: 1.01,
        nose: 1.,
        ..Pose::REST
    };
    let small = busy.for_small();
    assert_eq!((small.eye_open, small.narrow), (0.4, 0.7));
    assert_eq!(
        (small.glance, small.lean, small.scale, small.nose),
        (0., 0., 1., 0.)
    );
}

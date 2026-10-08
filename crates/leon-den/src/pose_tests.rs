//! Tests of the poses: which frame of which sheet shows a lion.

use crate::atelier::{FRAMES, FRAME_ASLEEP, FRAME_H, FRAME_OUT, FRAME_STARE};
use crate::glyphs;
use crate::pose::{bounds, bubble, cel, head_top, Act, Bubble, Facing, Look, Sheet, SITTING, TILE};

const ACTS: [Act; 18] = [
    Act::Egg,
    Act::Hatch,
    Act::Walk,
    Act::Stand,
    Act::Type,
    Act::Think,
    Act::Wonder,
    Act::Browse,
    Act::Rummage,
    Act::Operate,
    Act::Peer,
    Act::Plan,
    Act::Guard,
    Act::Wait,
    Act::Stare,
    Act::Lounge,
    Act::Sleep,
    Act::Faint,
];
const FACINGS: [Facing; 4] = [Facing::Down, Facing::Up, Facing::Left, Facing::Right];
const BUBBLES: [Bubble; 7] = [
    Bubble::Bang,
    Bubble::Urgent,
    Bubble::Thought,
    Bubble::Question,
    Bubble::Cross,
    Bubble::Tick,
    Bubble::Zzz,
];

fn look(act: Act, facing: Facing) -> Look {
    Look {
        act,
        facing,
        little: false,
        seat: false,
        beat: 0,
        bubble: None,
    }
}

#[test]
fn every_look_is_a_frame_that_exists_drawn_near_its_tile() {
    for act in ACTS {
        for facing in FACINGS {
            for little in [false, true] {
                for seat in [false, true] {
                    for beat in 0..24 {
                        let look = Look {
                            act,
                            facing,
                            little,
                            seat,
                            beat,
                            bubble: None,
                        };
                        let drawn = cel(&look);
                        let frames = match drawn.sheet {
                            Sheet::Lion => FRAMES,
                            Sheet::Cub => 8,
                            Sheet::Egg => 3,
                        };
                        assert!((0..frames).contains(&drawn.frame), "{look:?}: {drawn:?}");
                        assert!((0..3).contains(&drawn.row), "{look:?}: {drawn:?}");
                        assert!((-8..=2).contains(&drawn.dx), "{look:?}: {drawn:?}");
                        assert!((-18..=8).contains(&drawn.dy), "{look:?}: {drawn:?}");
                        let (left, top, w, h) = bounds(&look);
                        assert!(w >= 12 && h >= 12, "{look:?}: {w}x{h}");
                        assert!(
                            left >= -8 && top >= -16 && top + h <= TILE + SITTING,
                            "{look:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_walk_plays_three_frames_in_four_beats_in_each_of_four_directions() {
    for little in [false, true] {
        for facing in FACINGS {
            let frames: Vec<i32> = (0..4)
                .map(|beat| {
                    cel(&Look {
                        beat,
                        little,
                        ..look(Act::Walk, facing)
                    })
                    .frame
                })
                .collect();
            assert_eq!(frames[1], frames[3], "the frame between two steps");
            assert_ne!(frames[0], frames[2], "two different steps");
            assert_ne!(frames[0], frames[1]);
        }
        let row = |facing| {
            cel(&Look {
                little,
                ..look(Act::Walk, facing)
            })
        };
        assert_eq!(row(Facing::Down).row, 0);
        assert_eq!(row(Facing::Up).row, 1);
        assert_eq!(row(Facing::Right).row, 2);
        // Left is right in a mirror.
        assert_eq!(
            (row(Facing::Left).row, row(Facing::Left).mirrored),
            (2, true)
        );
        assert!(!row(Facing::Right).mirrored);
    }
}

#[test]
fn a_lion_at_its_desk_types_seated_seen_from_behind_and_one_without_a_desk_stands() {
    let seated = |beat| {
        cel(&Look {
            seat: true,
            beat,
            ..look(Act::Type, Facing::Up)
        })
    };
    assert_eq!((seated(0).row, seated(0).frame), (1, 3));
    assert_eq!(seated(1).frame, 4, "the arms move");
    assert_eq!(seated(0).dy, TILE - FRAME_H + SITTING, "into its seat");
    assert_eq!(seated(0).depth, SITTING, "and in front of it");
    // No desk: on its feet, facing the viewer, a tablet in its paws.
    let standing = cel(&look(Act::Type, Facing::Up));
    assert_eq!((standing.row, standing.frame), (0, 5));
    assert_eq!((standing.dy, standing.depth), (TILE - FRAME_H, 0));
}

#[test]
fn the_states_that_matter_have_a_face_of_their_own() {
    assert_eq!(cel(&look(Act::Stare, Facing::Up)).frame, FRAME_STARE);
    assert_eq!(
        cel(&look(Act::Stare, Facing::Up)).row,
        0,
        "it stares at the viewer"
    );
    assert_eq!(cel(&look(Act::Sleep, Facing::Down)).frame, FRAME_ASLEEP);
    let out = cel(&look(Act::Faint, Facing::Left));
    assert_eq!(out.frame, FRAME_OUT);
    assert!(out.lying, "a lion that fainted is on the floor");
    assert!(!cel(&look(Act::Wait, Facing::Down)).lying);
    // A cub too.
    let cub = |act| {
        cel(&Look {
            little: true,
            ..look(act, Facing::Down)
        })
    };
    assert_eq!(cub(Act::Sleep).frame, 5);
    assert_eq!(cub(Act::Stare).frame, 6);
    assert_eq!(cub(Act::Faint).frame, 7);
    assert_eq!(cub(Act::Wait).sheet, Sheet::Cub);
}

#[test]
fn an_egg_wobbles_cracks_and_opens() {
    let egg = |act, beat| {
        cel(&Look {
            beat,
            little: true,
            ..look(act, Facing::Down)
        })
    };
    assert_eq!(egg(Act::Egg, 0).sheet, Sheet::Egg);
    let leans: Vec<i32> = (0..4).map(|beat| egg(Act::Egg, beat).dx).collect();
    assert_eq!(leans, vec![0, -1, 0, 1]);
    assert_eq!(egg(Act::Egg, 0).frame, 0);
    assert_eq!(egg(Act::Egg, 6).frame, 1, "it cracks");
    assert_eq!(egg(Act::Hatch, 0).frame, 2, "it opens");
}

#[test]
fn every_bubble_floats_over_the_head_and_never_on_the_face() {
    for kind in BUBBLES {
        for (act, seat, little) in [
            (Act::Wait, false, false),
            (Act::Think, true, false),
            (Act::Sleep, true, false),
            (Act::Faint, false, false),
            (Act::Wait, false, true),
            (Act::Type, true, true),
        ] {
            for beat in 0..8 {
                let look = Look {
                    act,
                    facing: Facing::Down,
                    little,
                    seat,
                    beat,
                    bubble: Some(kind),
                };
                let stamps = bubble(&look);
                assert!(!stamps.is_empty(), "{look:?}");
                let top = head_top(&look);
                for stamp in stamps {
                    let bottom = stamp.y + stamp.sprite.h;
                    // `Zzz` drifts beside the head; the others sit on it.
                    let limit = if kind == Bubble::Zzz {
                        top + 5
                    } else {
                        top + 1
                    };
                    assert!(bottom <= limit, "{look:?}: bubble down to {bottom}");
                }
            }
        }
    }
    assert!(bubble(&look(Act::Wait, Facing::Down)).is_empty());
}

#[test]
fn an_act_that_does_not_move_has_no_beat() {
    assert_eq!(Act::Faint.beat_ticks(), 0);
    assert_eq!(Act::Stand.beat_ticks(), 0);
    let fainted = |beat| {
        cel(&Look {
            beat,
            ..look(Act::Faint, Facing::Down)
        })
    };
    assert_eq!(fainted(0), fainted(7));
    // Walking is the fastest thing in the den; sleeping the slowest.
    assert!(Act::Walk.beat_ticks() < Act::Type.beat_ticks());
    assert!(Act::Type.beat_ticks() < Act::Sleep.beat_ticks());
    assert!(Act::Type.seated() && Act::Lounge.seated() && !Act::Wait.seated());
}

#[test]
fn the_little_font_has_every_letter_a_name_needs() {
    let blank = glyphs::glyph(' ');
    let unknown = glyphs::glyph('~');
    for letter in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-_!?:/+vz".chars() {
        let glyph = glyphs::glyph(letter);
        assert_ne!(glyph, blank, "{letter} is drawn");
        assert_ne!(glyph, unknown, "{letter} has its own drawing");
        assert!(glyph.iter().all(|row| *row < 8), "{letter} is three wide");
    }
    assert_eq!(glyphs::glyph('m'), glyphs::glyph('M'));
    assert_eq!(glyphs::text_width("MOSS"), 15);
    assert_eq!(glyphs::text_width(""), 0);
}

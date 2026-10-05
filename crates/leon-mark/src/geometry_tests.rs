//! Tests of the lion's shapes: the rest pose is the owner's SVG, and no pose
//! of any gesture breaks the silhouette.

use crate::geometry::{path_data, shapes, Pt, Variant, GRID};
use crate::motion::{Gesture, Pose};

const FULL_SVG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../app/assets/brand/leon-mark.svg"
);
const SMALL_SVG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../app/assets/brand/leon-mark-16.svg"
);

/// The `d` attribute of the single path of an SVG file.
fn d_of(file: &str) -> String {
    let text = std::fs::read_to_string(file).expect("the brand SVG is readable");
    let document = roxmltree::Document::parse(&text).expect("the brand SVG is XML");
    document
        .descendants()
        .find(|node| node.has_tag_name("path"))
        .and_then(|node| node.attribute("d"))
        .expect("the SVG has one path")
        .to_owned()
}

/// A path made only of `M`, `L` and `Z`, as its contours.
fn parse(d: &str) -> Vec<Vec<Pt>> {
    let mut contours: Vec<Vec<Pt>> = Vec::new();
    let mut rest = d;
    while let Some(command) = rest.chars().next() {
        rest = &rest[command.len_utf8()..];
        match command {
            'M' | 'L' => {
                let end = rest.find(['M', 'L', 'Z']).unwrap_or(rest.len());
                let (numbers, tail) = rest.split_at(end);
                let mut parts = numbers
                    .split_whitespace()
                    .map(|n| n.parse::<f32>().unwrap());
                let point = (parts.next().unwrap(), parts.next().unwrap());
                if command == 'M' {
                    contours.push(Vec::new());
                }
                contours.last_mut().unwrap().push(point);
                rest = tail;
            }
            'Z' => {}
            other => panic!("unexpected command {other}"),
        }
    }
    contours
}

fn area(polygon: &[Pt]) -> f32 {
    let twice: f32 = polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
        .sum();
    twice / 2.
}

fn inside(point: Pt, polygon: &[Pt]) -> bool {
    let mut inside = false;
    let mut previous = *polygon.last().unwrap();
    for &current in polygon {
        if (current.1 > point.1) != (previous.1 > point.1)
            && point.0
                < (previous.0 - current.0) * (point.1 - current.1) / (previous.1 - current.1)
                    + current.0
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

/// Poses that reach the extremes of every part, at both ends of every gesture.
fn extreme_poses() -> Vec<Pose> {
    let mut poses = vec![Pose::REST];
    for gesture in Gesture::catalogue() {
        let steps = 40;
        for step in 0..=steps {
            let at = gesture.duration() * step as f32 / steps as f32;
            poses.push(gesture.pose(at));
        }
    }
    for glance in [-2., 2.] {
        for lean in [-2., 2.] {
            for scale in [1., 1.015] {
                poses.push(Pose {
                    glance,
                    lean,
                    scale,
                    narrow: 1.,
                    eye_open: 0.,
                    nose: 1.,
                    ..Pose::REST
                });
            }
        }
    }
    poses
}

#[test]
fn at_rest_the_full_mark_is_the_path_of_leon_mark_svg() {
    let drawn = shapes(&Pose::REST, Variant::Full);
    assert_eq!(drawn.contours().to_vec(), parse(&d_of(FULL_SVG)));
}

#[test]
fn at_rest_the_small_mark_is_the_path_of_leon_mark_16_svg() {
    let drawn = shapes(&Pose::REST, Variant::Small);
    assert_eq!(drawn.contours().to_vec(), parse(&d_of(SMALL_SVG)));
}

#[test]
fn the_rest_path_data_is_the_svg_attribute_byte_for_byte() {
    for (variant, file) in [(Variant::Full, FULL_SVG), (Variant::Small, SMALL_SVG)] {
        let drawn = shapes(&Pose::REST, variant);
        assert_eq!(path_data(&drawn.contours()), d_of(file));
    }
}

#[test]
fn the_rest_pose_paints_the_plate_whole_and_no_glow() {
    let drawn = shapes(&Pose::REST, Variant::Full);
    assert_eq!(drawn.plate_opacity, 1.);
    assert_eq!(drawn.glow_opacity, 0.);
}

#[test]
fn every_pose_of_every_gesture_stays_inside_the_64_grid() {
    for variant in [Variant::Full, Variant::Small] {
        for pose in extreme_poses() {
            let drawn = shapes(&pose, variant);
            for contour in drawn.contours() {
                for (x, y) in contour {
                    assert!(
                        (0. ..=GRID).contains(&x) && (0. ..=GRID).contains(&y),
                        "({x}, {y}) leaves the grid in {pose:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_eye_cuts_never_invert_or_cross() {
    let rest = shapes(&Pose::REST, Variant::Full);
    let (left_sign, right_sign) = (
        area(&rest.left_eye).signum(),
        area(&rest.right_eye).signum(),
    );
    for pose in extreme_poses() {
        let drawn = shapes(&pose, Variant::Full);
        assert!(
            area(&drawn.left_eye) * left_sign >= 0.,
            "the left eye inverts in {pose:?}"
        );
        assert!(
            area(&drawn.right_eye) * right_sign >= 0.,
            "the right eye inverts in {pose:?}"
        );
        let left_edge = drawn.left_eye.iter().map(|p| p.0).fold(f32::MIN, f32::max);
        let right_edge = drawn.right_eye.iter().map(|p| p.0).fold(f32::MAX, f32::min);
        assert!(
            left_edge + 2. < right_edge,
            "the eyes meet in {pose:?}: {left_edge} against {right_edge}"
        );
    }
}

#[test]
fn a_closed_eye_is_a_hairline_not_a_negative_area() {
    let rest = shapes(&Pose::REST, Variant::Full);
    let sign = area(&rest.left_eye).signum();
    let closed = shapes(
        &Pose {
            eye_open: 0.,
            ..Pose::REST
        },
        Variant::Full,
    );
    for eye in [&closed.left_eye, &closed.right_eye] {
        let rest_area = area(&rest.left_eye).abs();
        let closed_area = area(eye).abs();
        assert!(closed_area > 0., "a closed eye still has a hairline");
        assert!(
            closed_area < rest_area / 10.,
            "a closed eye is thin: {closed_area} against {rest_area}"
        );
        assert!(area(eye) * sign >= 0., "a closed eye never inverts");
    }
}

#[test]
fn the_eye_cuts_stay_inside_the_plate() {
    for variant in [Variant::Full, Variant::Small] {
        for pose in extreme_poses() {
            let drawn = shapes(&pose, variant);
            for eye in [&drawn.left_eye, &drawn.right_eye] {
                for &point in eye {
                    assert!(
                        inside(point, &drawn.plate),
                        "{point:?} falls out of the plate in {pose:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn narrowing_lowers_the_brow_and_raises_the_lower_lid() {
    let rest = shapes(&Pose::REST, Variant::Full);
    let narrow = shapes(
        &Pose {
            narrow: 1.,
            ..Pose::REST
        },
        Variant::Full,
    );
    assert!(area(&narrow.left_eye).abs() < area(&rest.left_eye).abs() * 0.7);
    // The inner top corner (the brow) is lower; the inner bottom corner (the lid) is higher.
    assert!(narrow.left_eye[1].1 > rest.left_eye[1].1);
    assert!(narrow.left_eye[2].1 < rest.left_eye[2].1);
    // The plate itself is not redrawn.
    assert_eq!(narrow.plate, rest.plate);
}

#[test]
fn a_glance_moves_both_eyes_together_and_leaves_the_nose_and_the_plate() {
    let rest = shapes(&Pose::REST, Variant::Full);
    let glance = shapes(
        &Pose {
            glance: 1.5,
            ..Pose::REST
        },
        Variant::Full,
    );
    for (now, before) in glance.left_eye.iter().zip(&rest.left_eye) {
        assert!((now.0 - before.0 - 1.5).abs() < 1e-4 && (now.1 - before.1).abs() < 1e-4);
    }
    for (now, before) in glance.right_eye.iter().zip(&rest.right_eye) {
        assert!((now.0 - before.0 - 1.5).abs() < 1e-4 && (now.1 - before.1).abs() < 1e-4);
    }
    assert_eq!(glance.nose, rest.nose);
    assert_eq!(glance.plate, rest.plate);
}

#[test]
fn a_nose_twitch_lifts_the_chevron_and_keeps_the_stems_foot_on_the_chin() {
    let rest = shapes(&Pose::REST, Variant::Full);
    let twitch = shapes(
        &Pose {
            nose: 1.,
            ..Pose::REST
        },
        Variant::Full,
    );
    // The chevron's tip is up; the two points of the stem on the chin edge stay.
    assert!(twitch.nose[1].1 < rest.nose[1].1);
    assert_eq!(twitch.nose[5], rest.nose[5]);
    assert_eq!(twitch.nose[6], rest.nose[6]);
    assert!(twitch.nose[1].1 > rest.nose[1].1 - 2., "it is a small lift");
}

#[test]
fn leaning_and_breathing_move_the_whole_head_together() {
    let rest = shapes(&Pose::REST, Variant::Full);
    let leaning = shapes(
        &Pose {
            lean: 2.,
            ..Pose::REST
        },
        Variant::Full,
    );
    assert_ne!(leaning.plate, rest.plate);
    assert_ne!(leaning.nose, rest.nose);
    assert_ne!(leaning.left_eye, rest.left_eye);
    let breathing = shapes(
        &Pose {
            scale: 1.012,
            ..Pose::REST
        },
        Variant::Full,
    );
    // About the centre of the head: the top moves up, the chin down.
    assert!(breathing.plate[2].1 < rest.plate[2].1);
    assert!(breathing.plate[7].1 > rest.plate[7].1);
}

#[test]
fn before_the_plate_arrives_only_the_eyes_glow() {
    let drawn = shapes(
        &Pose {
            plate: 0.,
            glow: 1.,
            eye_open: 1.,
            ..Pose::REST
        },
        Variant::Full,
    );
    assert_eq!(drawn.plate_opacity, 0.);
    assert_eq!(drawn.glow_opacity, 1.);
    let rest = shapes(&Pose::REST, Variant::Full);
    assert_eq!(drawn.glow[0], rest.left_eye);
    assert_eq!(drawn.glow[1], rest.right_eye);
}

#[test]
fn the_fitted_geometry_is_for_twenty_pixels_and_below() {
    assert_eq!(Variant::for_side(16.), Variant::Small);
    assert_eq!(Variant::for_side(20.), Variant::Small);
    assert_eq!(Variant::for_side(20.5), Variant::Full);
    assert_eq!(Variant::for_side(64.), Variant::Full);
}

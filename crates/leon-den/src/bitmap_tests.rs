//! Tests of the bitmap: pixels in, pixels out.

use crate::bitmap::{hsl, Bitmap, Tone, CLEAR};

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

#[test]
fn a_pixel_set_is_a_pixel_read_and_outside_is_clear() {
    let mut picture = Bitmap::new(3, 2);
    picture.set(2, 1, RED);
    picture.set(9, 9, RED);
    assert_eq!(picture.get(2, 1), RED);
    assert_eq!(picture.get(0, 0), CLEAR);
    assert_eq!(picture.get(-1, 0), CLEAR);
    assert_eq!(picture.area(), 1);
    assert_eq!(picture.top(), Some(1));
    assert_eq!(Bitmap::new(2, 2).top(), None);
}

#[test]
fn drawing_lays_a_picture_over_another_and_leaves_what_it_does_not_cover() {
    let mut floor = Bitmap::new(4, 4);
    floor.fill(0, 0, 4, 4, BLUE);
    let mut lion = Bitmap::new(2, 2);
    lion.set(0, 0, RED);
    floor.draw(&lion, 3, 3);
    floor.draw(&lion, 1, 1);
    assert_eq!(floor.get(1, 1), RED);
    assert_eq!(floor.get(2, 2), BLUE, "a clear pixel covers nothing");
    assert_eq!(
        floor.get(3, 3),
        RED,
        "what falls outside is cut, not wrapped"
    );
    // Half a red over blue is purple, and stays opaque.
    floor.blend(0, 0, [255, 0, 0, 128]);
    assert_eq!(floor.get(0, 0), [128, 0, 127, 255]);
}

#[test]
fn a_picture_is_mirrored_turned_and_cut() {
    let mut picture = Bitmap::new(2, 3);
    picture.set(0, 0, RED);
    picture.set(1, 2, BLUE);
    let mirrored = picture.mirrored();
    assert_eq!((mirrored.get(1, 0), mirrored.get(0, 2)), (RED, BLUE));
    // What stood lies down, its head to the right.
    let turned = picture.turned();
    assert_eq!((turned.w, turned.h), (3, 2));
    assert_eq!(turned.get(2, 0), RED);
    assert_eq!(turned.get(0, 1), BLUE);
    let part = picture.part(1, 2, 1, 1);
    assert_eq!(part.get(0, 0), BLUE);
    // Drawing a part mirrored is drawing the mirror of the part.
    let mut out = Bitmap::new(2, 3);
    out.draw_part(&picture, 0, 0, 2, 3, 0, 0, true);
    assert_eq!(out, mirrored);
}

#[test]
fn scaling_makes_each_pixel_a_square_and_blurs_nothing() {
    let mut picture = Bitmap::new(2, 1);
    picture.set(0, 0, RED);
    picture.set(1, 0, BLUE);
    let large = picture.scaled(3);
    assert_eq!((large.w, large.h), (6, 3));
    for y in 0..3 {
        for x in 0..6 {
            assert_eq!(large.get(x, y), if x < 3 { RED } else { BLUE });
        }
    }
    assert_eq!(picture.scaled(1), picture);
    assert_eq!(
        picture.scaled(0),
        picture,
        "a den is never smaller than its art"
    );
}

#[test]
fn a_picture_survives_the_trip_through_a_png_file() {
    let mut picture = Bitmap::new(5, 4);
    picture.fill(1, 1, 3, 2, [10, 200, 30, 255]);
    picture.set(0, 0, [1, 2, 3, 77]);
    assert_eq!(Bitmap::from_png(&picture.to_png()), picture);
}

#[test]
fn a_grey_tile_takes_the_hue_of_its_tone_and_keeps_its_light_and_shade() {
    assert_eq!(hsl(0., 1., 0.5), [255, 0, 0]);
    assert_eq!(hsl(120., 1., 0.5), [0, 255, 0]);
    assert_eq!(hsl(200., 0., 0.5), [128, 128, 128]);
    let tone = Tone {
        h: 30.,
        s: 50.,
        b: 0.,
        c: 0.,
    };
    let mut tile = Bitmap::new(3, 1);
    tile.set(0, 0, [60, 60, 60, 255]);
    tile.set(1, 0, [200, 200, 200, 255]);
    let wood = tone.colorize(&tile);
    let (dark, light) = (wood.get(0, 0), wood.get(1, 0));
    assert!(
        dark[0] > dark[1] && dark[1] > dark[2],
        "an orange: {dark:?}"
    );
    assert!(
        light[0] > dark[0] && light[2] > dark[2],
        "the light stays light"
    );
    assert_eq!(wood.get(2, 0), CLEAR, "what was clear stays clear");
    // Brightness moves everything, contrast pulls it to the middle.
    let dim = Tone { b: -100., ..tone };
    assert!(dim.of(0.5)[0] < tone.of(0.5)[0]);
    let flat = Tone { c: -100., ..tone };
    assert_eq!(flat.of(0.1), flat.of(0.9));
}

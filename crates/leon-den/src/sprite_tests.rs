//! Tests of the sprites: a drawing and its rectangles are the same pixels.

use crate::palette::Role;
use crate::sprite::{Sprite, Stamp};

fn covered(sprite: &Sprite, x: i32, y: i32) -> Vec<Role> {
    sprite
        .runs()
        .iter()
        .filter(|run| x >= run.x && x < run.x + run.w && y >= run.y && y < run.y + run.h)
        .map(|run| run.role)
        .collect()
}

#[test]
fn the_runs_of_a_sprite_cover_each_pixel_once_and_nothing_else() {
    let sprite = Sprite::parse(&["oo.w", "oo.w", ".bbw", "o..."]);
    for y in 0..sprite.h {
        for x in 0..sprite.w {
            let roles = covered(&sprite, x, y);
            match sprite.get(x, y) {
                Some(role) => assert_eq!(roles, vec![role], "pixel {x},{y}"),
                None => assert!(roles.is_empty(), "pixel {x},{y} is empty"),
            }
        }
    }
    assert_eq!(sprite.area(), 10);
}

#[test]
fn equal_runs_of_consecutive_rows_are_one_rectangle() {
    let sprite = Sprite::parse(&["oo.w", "oo.w", ".bbw", "o..."]);
    // The 2x2 block, the column of three, the pair and the single pixel.
    assert_eq!(sprite.runs().len(), 4);
    let block = sprite.runs()[0];
    assert_eq!((block.x, block.y, block.w, block.h), (0, 0, 2, 2));
}

#[test]
fn a_stamp_moves_and_mirrors_the_runs() {
    let sprite = Sprite::parse(&["ow.", "..."]);
    let plain: Vec<_> = Stamp::at(&sprite, 10, 20).runs().collect();
    assert_eq!((plain[0].x, plain[0].y, plain[0].role), (10, 20, Role::Ink));
    let mirrored: Vec<_> = Stamp::at(&sprite, 10, 20).mirrored(true).runs().collect();
    assert_eq!((mirrored[0].x, mirrored[0].role), (12, Role::Ink));
    assert_eq!((mirrored[1].x, mirrored[1].role), (11, Role::Bubble));
    let upside: Vec<_> = Stamp::at(&sprite, 10, 20)
        .upside_down(true)
        .runs()
        .collect();
    assert_eq!(upside[0].y, 21);
}

#[test]
#[should_panic(expected = "not 3 wide")]
fn a_drawing_with_a_short_row_is_a_bug() {
    Sprite::parse(&["ooo", "oo"]);
}

#[test]
#[should_panic(expected = "unknown letter")]
fn a_drawing_with_a_letter_that_is_no_role_is_a_bug() {
    Sprite::parse(&["o?o"]);
}

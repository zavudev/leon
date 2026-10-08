//! The workshop: how the lion sheets are made from the character sheets.
//!
//! The characters of the Den are the lions of `assets/lions/`. They are not
//! drawn from nothing: each is one of the character sheets of
//! `assets/source/characters/` (pixel-agents, MIT; after MetroCity by
//! JIK-A-4, CC0), which gives the body, the clothes and the animation, with
//! the human head taken off and a lion's head put in its place, the skin of
//! the hands turned to fur, and a tail added. The heads and the tails are
//! drawn here, by hand, one letter per pixel.
//!
//! Nothing of this runs in the app. The example `make_lions` calls
//! [`lion_sheet`] and [`cub_sheet`] and writes the PNG files, which are
//! committed; a test checks that the committed files are what this module
//! makes, so the drawings below are the source of the lions.
//!
//! # The sheets
//!
//! A source sheet is 7 frames of 16 by 32 in 3 rows: facing down, up and
//! right (left is right in a mirror). The frames are: 0 to 2 the walk (played
//! 0, 1, 2, 1), 3 and 4 typing, seated, 5 and 6 reading. A lion sheet keeps
//! those and adds three frames to each row ([`FRAMES`] in all): 7 seated
//! with its eyes shut, 8 standing with its eyes wide, 9 standing with a
//! cross for each eye.
//!
//! The mane is painted in three key colours ([`MANE`]) that are in no source
//! sheet, so that the app can give each lion the mane of its agent.

use crate::bitmap::{Bitmap, Rgba, CLEAR};

/// The width of a frame.
pub const FRAME_W: i32 = 16;
/// The height of a frame.
pub const FRAME_H: i32 = 32;
/// How many frames a row of a source sheet has.
pub const SOURCE_FRAMES: i32 = 7;
/// How many frames a row of a lion sheet has.
pub const FRAMES: i32 = 10;
/// The frame of a lion seated with its eyes shut.
pub const FRAME_ASLEEP: i32 = 7;
/// The frame of a lion standing with its eyes wide.
pub const FRAME_STARE: i32 = 8;
/// The frame of a lion standing with a cross for each eye.
pub const FRAME_OUT: i32 = 9;

/// The key colours of the mane: light, middle and dark.
pub const MANE: [Rgba; 3] = [[255, 0, 255, 255], [200, 0, 200, 255], [120, 0, 120, 255]];

const OUTLINE: Rgba = [50, 25, 29, 255];
const FUR_LIGHT: Rgba = [244, 200, 124, 255];
const FUR: Rgba = [226, 166, 88, 255];
const FUR_DARK: Rgba = [178, 114, 56, 255];
const CREAM: Rgba = [252, 236, 204, 255];
const EYE: Rgba = [24, 14, 16, 255];
const WHITE: Rgba = [255, 255, 255, 255];
const NOSE: Rgba = [96, 46, 44, 255];
const EAR: Rgba = [204, 126, 104, 255];
const SHELL: Rgba = [250, 240, 220, 255];
const SHELL_SHADE: Rgba = [222, 204, 176, 255];
const TWIG: Rgba = [150, 104, 60, 255];
const TWIG_DARK: Rgba = [104, 68, 42, 255];
const TWIG_LIGHT: Rgba = [196, 150, 92, 255];
const METAL_DARK: Rgba = [46, 50, 62, 255];
const METAL: Rgba = [78, 84, 100, 255];
const METAL_LIGHT: Rgba = [142, 150, 166, 255];
const LED_GREEN: Rgba = [96, 224, 128, 255];
const LED_AMBER: Rgba = [255, 176, 64, 255];
const NIGHT: Rgba = [26, 32, 60, 255];
const NIGHT_LIGHT: Rgba = [48, 62, 104, 255];
const ACID: Rgba = [255, 234, 0, 255];
const ACID_SHADE: Rgba = [206, 180, 0, 255];
const INK: Rgba = [10, 10, 10, 255];

/// The colour a letter of a drawing stands for.
fn ink(letter: char) -> Rgba {
    match letter {
        'o' => OUTLINE,
        'M' => MANE[0],
        'm' => MANE[1],
        'n' => MANE[2],
        'F' => FUR_LIGHT,
        'f' => FUR,
        'd' => FUR_DARK,
        'c' => CREAM,
        'e' => EYE,
        'w' => WHITE,
        'p' => NOSE,
        'i' => EAR,
        's' => SHELL,
        'S' => SHELL_SHADE,
        't' => TWIG,
        'T' => TWIG_DARK,
        'l' => TWIG_LIGHT,
        'k' => METAL_DARK,
        'K' => METAL,
        'g' => METAL_LIGHT,
        'G' => LED_GREEN,
        'a' => LED_AMBER,
        'b' => NIGHT,
        'B' => NIGHT_LIGHT,
        'y' => ACID,
        'Y' => ACID_SHADE,
        'x' => INK,
        '.' => CLEAR,
        other => panic!("{other:?} is no colour of the workshop"),
    }
}

/// A drawing in letters as a picture.
pub fn drawing(rows: &[&str]) -> Bitmap {
    let w = rows.first().map_or(0, |row| row.chars().count()) as i32;
    let mut out = Bitmap::new(w, rows.len() as i32);
    for (y, row) in rows.iter().enumerate() {
        assert_eq!(
            row.chars().count() as i32,
            w,
            "row {y} is not {w} wide: {row:?}"
        );
        for (x, letter) in row.chars().enumerate() {
            out.set(x as i32, y as i32, ink(letter));
        }
    }
    out
}

/// The lion's head from the front. The eyes are at rows 8 and 9.
const HEAD_DOWN: [&str; 15] = [
    "..oo........oo..",
    ".ofFo.oooo.oFfo.",
    ".ofioMMMMMMoifo.",
    "oooMMMMmmMMMMooo",
    "oMMMMmmffmmMMMMo",
    ".oMmmffFFffmmMo.",
    "oMMmfFFFFFFfmMMo",
    "oMmfdFFFFFFdfmMo",
    ".omfedFFFFdefmo.",
    "oMmfewfccfwefmMo",
    "onmmffccccffmmno",
    ".onmfccppccfmno.",
    "onnmmfccccfmmnno",
    ".ononmmffmmnono.",
    "..o.oooooooo.o..",
];

/// The rows 7 to 9 of the front head with the eyes shut.
const EYES_SHUT: [&str; 3] = ["oMmfFFFFFFFFfmMo", ".omfdFFFFFFdfmo.", "oMmfeefccfeefmMo"];

/// The same rows with the eyes wide: the stare.
const EYES_WIDE: [&str; 3] = ["oMmfeeFFFFeefmMo", ".omfweFFFFwefmo.", "oMmfeefccfeefmMo"];

/// The same rows with a cross for each eye.
const EYES_OUT: [&str; 3] = ["oMmeFeFFFFeFemMo", ".omfeFFFFFFefmo.", "oMmeFefccfeFemMo"];

/// The lion's head from behind: all mane.
const HEAD_UP: [&str; 15] = [
    "..oo........oo..",
    ".ofFo.oooo.oFfo.",
    ".offoMMMMMMoffo.",
    "oooMMMMMMMMMMooo",
    "oMMMMmMMMMmMMMMo",
    ".oMMmMMMmMMmMMo.",
    "oMMMmMMmMMMmMMMo",
    "oMMmMMMmMMmMMmMo",
    ".oMmMMmMMMmMMmo.",
    "oMmMMmmMMmmMmMMo",
    "onmMmmMmmMmmMmno",
    ".onmmnmmmmnmmno.",
    "onnmnmmnmmnmnnno",
    ".ononnmmmmnnono.",
    "..o.oooooooo.o..",
];

/// The lion's head from the side, looking right: the mane behind, the
/// muzzle out in front. The eye is at row 7, column 10.
const HEAD_RIGHT: [&str; 14] = [
    "......oo........",
    ".....ofFo.......",
    "..oooofiooo.....",
    ".oMMMMMoMMfo....",
    "oMMMmMMMmFFFo...",
    ".oMMmMMmfFFFFo..",
    "oMMmMMmmfddFFoo.",
    "oMMmMmmfFFewFcco",
    ".oMmMmmfFFFccccp",
    "onmmMmmmfFcccco.",
    ".onmmmmmffccoo..",
    "onnmnmmmmffo....",
    ".ononmmmmoo.....",
    "..o.ooooo.......",
];

/// The tail seen from the front: it shows beside the legs.
const TAIL_DOWN: [&str; 6] = ["..oo", ".omm", ".omo", "ofo.", "ofo.", "oo.."];

/// The tail seen from behind: it hangs down the middle.
const TAIL_UP: [&str; 7] = [".oo.", "ofo.", "ofo.", ".ofo", ".ofo", "ommo", ".oo."];

/// The tail seen from the side, behind a lion that looks right.
const TAIL_RIGHT: [&str; 6] = ["oo...", "omo..", "ommo.", ".ofo.", "..ofo", "...oo"];

/// Where the lion's head goes in a frame: the row its drawing starts at.
///
/// The sheets do not all sit at the same height in every frame, so the place
/// is read from the frame itself: from the white of the eyes when the face
/// shows, and from the neck, the first narrow row under the hair, when the
/// character is seen from behind.
fn head_y(body: &Bitmap, row: usize) -> i32 {
    let white = |x: i32, y: i32| {
        let [r, g, b, a] = body.get(x, y);
        a > 128 && r == 255 && g == 255 && b == 255
    };
    let eye = (0..22).find(|y| (3..13).any(|x| white(x, *y)));
    let width = |y: i32| (0..FRAME_W).filter(|x| body.get(*x, y)[3] > 128).count();
    match row {
        // The eyes of the lion are higher in its head than a person's: its
        // muzzle takes the room.
        0 => eye.map_or(3, |y| y - 11),
        1 => (12..22).find(|y| width(*y) <= 8).map_or(3, |y| y - 13),
        _ => eye.map_or(6, |y| y - 10),
    }
}

fn is_skin([r, g, b, a]: Rgba) -> bool {
    // The skins of the six sheets: warm, red over green over blue, neither
    // a shadow nor a white.
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    a > 0 && r > g + 18 && g > b + 8 && r >= 120 && r - b < 150 && !(r > 240 && g > 240)
}

/// One frame of a source sheet as a lion: the head off, the lion's head on,
/// the hands in fur, a tail.
fn lion_frame(source: &Bitmap, row: usize, frame: usize, eyes: Option<&[&str; 3]>) -> Bitmap {
    let body = source.part(
        frame as i32 * FRAME_W,
        row as i32 * FRAME_H,
        FRAME_W,
        FRAME_H,
    );
    let head_y = head_y(&body, row);
    let head: &[&str] = match row {
        0 => &HEAD_DOWN,
        1 => &HEAD_UP,
        _ => &HEAD_RIGHT,
    };
    let neck = head_y + head.len() as i32;
    let mut out = Bitmap::new(FRAME_W, FRAME_H);
    // The body: everything under the neck, hands and all, the skin as fur.
    for y in neck..FRAME_H {
        for x in 0..FRAME_W {
            let pixel = body.get(x, y);
            if pixel[3] < 128 {
                continue;
            }
            let pixel = if is_skin(pixel) {
                let light = u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]);
                match light {
                    0..=420 => FUR_DARK,
                    421..=560 => FUR,
                    _ => FUR_LIGHT,
                }
            } else {
                [pixel[0], pixel[1], pixel[2], 255]
            };
            out.set(x, y, pixel);
        }
    }
    // The tail goes on before the head, so that the mane hangs over it.
    let (tail, tx, ty): (&[&str], i32, i32) = match row {
        0 => (&TAIL_DOWN, 12, head_y + 18),
        1 => (&TAIL_UP, 6, head_y + 20),
        _ => (&TAIL_RIGHT, 0, head_y + 13),
    };
    // Seated, seen from the front, the tail is behind the chair.
    if !(row == 0 && (3..=6).contains(&frame)) {
        let tail = drawing(tail);
        for y in 0..tail.h {
            for x in 0..tail.w {
                let (px, py) = (tx + x, ty + y);
                // Behind the body: only where nothing is drawn yet.
                if out.get(px, py)[3] == 0 || row == 1 {
                    out.blend(px, py, tail.get(x, y));
                }
            }
        }
    }
    out.draw(&drawing(head), 0, head_y);
    if let (Some(eyes), 0) = (eyes, row) {
        out.draw(&drawing(eyes), 0, head_y + 7);
    }
    out
}

/// The lion sheet made from a source sheet: [`FRAMES`] frames in 3 rows.
pub fn lion_sheet(source: &Bitmap) -> Bitmap {
    let mut sheet = Bitmap::new(FRAMES * FRAME_W, 3 * FRAME_H);
    for row in 0..3 {
        for frame in 0..SOURCE_FRAMES as usize {
            let lion = lion_frame(source, row, frame, None);
            sheet.draw(&lion, frame as i32 * FRAME_W, row as i32 * FRAME_H);
        }
        for (frame, from, eyes) in [
            (FRAME_ASLEEP, 3, &EYES_SHUT),
            (FRAME_STARE, 1, &EYES_WIDE),
            (FRAME_OUT, 1, &EYES_OUT),
        ] {
            let lion = lion_frame(source, row, from, Some(eyes));
            sheet.draw(&lion, frame * FRAME_W, row as i32 * FRAME_H);
        }
    }
    sheet
}

// ------------------------------------------------------------ the cub

/// The side of a frame of the cub sheet.
pub const CUB_FRAME: i32 = 16;
/// How many frames a row of the cub sheet has: 0 standing, 1 and 2 the two
/// steps of its walk, 3 and 4 typing, 5 asleep, 6 staring, 7 out cold.
pub const CUB_FRAMES: i32 = 8;

const CUB_HEAD_DOWN: [&str; 9] = [
    ".oo......oo.",
    "ofio.mm.oifo",
    "offommmmoffo",
    "ofFFFmmFFFfo",
    "ofFFFFFFFFfo",
    "ofeFFccFFefo",
    "odFccppccFdo",
    ".odfccccfdo.",
    "..oooooooo..",
];

const CUB_HEAD_UP: [&str; 9] = [
    ".oo......oo.",
    "ofdo.mm.odfo",
    "offommmmoffo",
    "offfmmmmfffo",
    "offffmmffffo",
    "offfffffFffo",
    "odffffffffdo",
    ".oddffffddo.",
    "..oooooooo..",
];

const CUB_HEAD_RIGHT: [&str; 9] = [
    "....oo......",
    "...ofio.mm..",
    "..offommmo..",
    ".offfffmFFo.",
    "offfffFFeFo.",
    "offfffFFFcco",
    "odfffffccccp",
    ".oddfffccco.",
    "..oooooooo..",
];

/// The cub's body from the front, standing: a scarf in the colour of its
/// mane to be, a cream belly.
const CUB_BODY_DOWN: [&str; 7] = [
    "..ommmmmmo..",
    ".offFFFFffo.",
    "ofoFccccFofo",
    ".ooFccccFoom",
    "..offooffo.o",
    "..offooffo..",
    "..ooo..ooo..",
];

const CUB_BODY_UP: [&str; 7] = [
    "..ommmmmmo..",
    ".offffffffo.",
    "ofoffffffofo",
    ".ooffoffffoo",
    "..offomfffo.",
    "..offooffo..",
    "..ooo..ooo..",
];

const CUB_BODY_RIGHT: [&str; 7] = [
    "...ommmmo...",
    "..offFFFo...",
    "mooffFFFfo..",
    ".ooffFFFoo..",
    "...offofo...",
    "...offofo...",
    "...ooo.oo...",
];

/// A frame of the cub: a head over a body, each moved by a pixel as the
/// frame asks.
fn cub_frame(row: usize, frame: i32) -> Bitmap {
    let (head, body): (&[&str], &[&str]) = match row {
        0 => (&CUB_HEAD_DOWN, &CUB_BODY_DOWN),
        1 => (&CUB_HEAD_UP, &CUB_BODY_UP),
        _ => (&CUB_HEAD_RIGHT, &CUB_BODY_RIGHT),
    };
    let mut out = Bitmap::new(CUB_FRAME, CUB_FRAME);
    let mut body = drawing(body);
    // A step: one foot is off the ground.
    let lifted = match frame {
        1 => Some(if row == 2 { 3..6 } else { 2..5 }),
        2 => Some(if row == 2 { 6..9 } else { 7..10 }),
        _ => None,
    };
    if let Some(columns) = lifted {
        for x in columns {
            body.set(x, 6, CLEAR);
            if body.get(x, 5)[3] > 0 {
                body.set(x, 5, OUTLINE);
            }
        }
    }
    let bob = i32::from(matches!(frame, 1 | 2 | 4));
    out.draw(&body, 2, 9);
    out.draw(&drawing(head), 2, bob);
    if row == 0 {
        let eyes: Option<[&str; 2]> = match frame {
            5 => Some(["ofFFFFFFFFfo", "ofeeFccFeefo"]),
            6 => Some(["ofeFFFFFFefo", "ofewFccFwefo"]),
            7 => Some(["oeFeFFFFeFeo", "ofeFFccFFefo"]),
            _ => None,
        };
        if let Some(eyes) = eyes {
            out.draw(&drawing(&eyes), 2, bob + 4);
        }
    }
    out
}

/// The sheet of the cub, the little one: [`CUB_FRAMES`] frames of 16 by 16
/// in 3 rows, facing down, up and right.
pub fn cub_sheet() -> Bitmap {
    let mut sheet = Bitmap::new(CUB_FRAMES * CUB_FRAME, 3 * CUB_FRAME);
    for row in 0..3 {
        for frame in 0..CUB_FRAMES {
            sheet.draw(
                &cub_frame(row, frame),
                frame * CUB_FRAME,
                row as i32 * CUB_FRAME,
            );
        }
    }
    sheet
}

// ---------------------------------------------------------- the props

const EGG: [&str; 13] = [
    "....oooo....",
    "...osssso...",
    "..osssmsso..",
    "..osssssso..",
    ".ossmssssso.",
    ".osssssmsso.",
    ".osssssssso.",
    ".osmsssssSo.",
    ".ossssmsSSo.",
    "..osssssSo..",
    "..oSssSSSo..",
    "...oSSSSo...",
    "....oooo....",
];

const EGG_CRACK: [&str; 3] = [".ossosssoso.", ".osososssoo.", ".oossosossso"];

const EGG_SHELL: [&str; 7] = [
    ".o.oo.oo.o..",
    ".ososssosso.",
    ".osmsssssSo.",
    ".ossssmsSSo.",
    "..osssssSo..",
    "..oSssSSSo..",
    "...oooooo...",
];

/// The egg, three frames of 16 by 16: whole, cracked, and open.
pub fn egg_sheet() -> Bitmap {
    let mut sheet = Bitmap::new(48, 16);
    let egg = drawing(&EGG);
    sheet.draw(&egg, 2, 3);
    sheet.draw(&egg, 18, 3);
    sheet.draw(&drawing(&EGG_CRACK), 18, 7);
    sheet.draw(&drawing(&EGG_SHELL), 34, 9);
    sheet
}

const NEST: [&str; 9] = [
    "...l..T...l..T..",
    "..tTtlttTtltTtl.",
    ".tltTttlTtTltTtt",
    "TtTtltTttlTtTltT",
    "tTltTtlTtTtlTtTt",
    "TtTtTlTtlTtTtlTT",
    ".TtTtTtTtTtTtTT.",
    "..TTtTTtTTtTTT..",
    "....TTTTTTTT....",
];

/// A nest of twigs: 16 by 16, the twigs in the lower part.
pub fn nest() -> Bitmap {
    let mut out = Bitmap::new(16, 16);
    out.draw(&drawing(&NEST), 0, 7);
    out
}

/// The rack, the big machine: three frames of 16 by 32. In the first it
/// idles, one light on; in the others its lights run.
pub fn rack_sheet() -> Bitmap {
    let mut sheet = Bitmap::new(48, 32);
    for frame in 0..3 {
        let left = frame * 16;
        sheet.fill(left + 1, 3, 14, 27, OUTLINE);
        sheet.fill(left + 2, 4, 12, 25, METAL);
        sheet.fill(left + 2, 4, 12, 1, METAL_LIGHT);
        for unit in 0..5 {
            let y = 6 + unit * 4;
            sheet.fill(left + 3, y, 10, 3, METAL_DARK);
            sheet.fill(left + 4, y + 1, 5, 1, METAL_LIGHT);
            let (first, second) = match (frame, unit) {
                (0, 0) => (Some(LED_GREEN), None),
                (0, _) => (None, None),
                (1, unit) if unit % 2 == 0 => (Some(LED_GREEN), Some(LED_AMBER)),
                (1, _) => (None, Some(LED_GREEN)),
                (_, unit) if unit % 2 == 0 => (None, Some(LED_GREEN)),
                (_, _) => (Some(LED_AMBER), Some(LED_GREEN)),
            };
            if let Some(color) = first {
                sheet.set(left + 10, y + 1, color);
            }
            if let Some(color) = second {
                sheet.set(left + 11, y + 1, color);
            }
        }
        // Two feet.
        sheet.fill(left + 2, 30, 2, 1, OUTLINE);
        sheet.fill(left + 12, 30, 2, 1, OUTLINE);
    }
    sheet
}

const TELESCOPE: [&str; 20] = [
    "...........oo...",
    "..........oggo..",
    ".........ogKko..",
    "........ogKko...",
    ".......ogKko....",
    "......ogKko.....",
    ".....ogKko......",
    "....olKko.......",
    "....oolTo.......",
    "......oTo.......",
    ".....otTto......",
    ".....oToTo......",
    "....oTo.oTo.....",
    "....oTo.oTo.....",
    "...oTo...oTo....",
    "...oTo...oTo....",
    "..oTo.....oTo...",
    "..oTo.....oTo...",
    ".oTo.......oTo..",
    ".oo.........oo..",
];

/// The telescope on its tripod: 16 by 32.
pub fn telescope() -> Bitmap {
    let mut out = Bitmap::new(16, 32);
    out.draw(&drawing(&TELESCOPE), 0, 11);
    out
}

/// The window of the lookout, for the wall: 32 by 32, night and stars.
pub fn window() -> Bitmap {
    let mut out = Bitmap::new(32, 32);
    out.fill(4, 6, 24, 19, TWIG_DARK);
    out.fill(5, 7, 22, 17, TWIG_LIGHT);
    out.fill(6, 8, 20, 15, NIGHT);
    out.fill(6, 17, 20, 6, NIGHT_LIGHT);
    for (x, y) in [
        (8, 10),
        (13, 9),
        (22, 11),
        (18, 14),
        (10, 15),
        (24, 16),
        (20, 9),
    ] {
        out.set(x, y, WHITE);
    }
    // A moon.
    out.fill(22, 12, 2, 2, SHELL);
    // The bars of the window, and its sill.
    out.fill(15, 8, 2, 15, TWIG_LIGHT);
    out.fill(6, 15, 20, 1, TWIG_LIGHT);
    out.fill(3, 25, 26, 2, TWIG);
    out.fill(3, 27, 26, 1, TWIG_DARK);
    out
}

const LION_PAINTING: [&str; 11] = [
    "..TTTTTTTTTTTT..",
    "..TllllllllllT..",
    "..TlxxxxxxxxlT..",
    "..TlyyxxxxyylT..",
    "..TlxyyxxyyxlT..",
    "..TlxxxxxxxxlT..",
    "..TlxxyxxyxxlT..",
    "..TlxxxyyxxxlT..",
    "..TlxxxxxxxxlT..",
    "..TllllllllllT..",
    "..TTTTTTTTTTTT..",
];

/// A painting of the glare, for the wall: 16 by 32.
pub fn lion_painting() -> Bitmap {
    let mut out = Bitmap::new(16, 32);
    out.draw(&drawing(&LION_PAINTING), 0, 11);
    out
}

const MARK: [&str; 9] = [
    "xx.........xx",
    "xxxx.....xxxx",
    ".xxxxx.xxxxx.",
    "...xxx.xxx...",
    ".............",
    "....x...x....",
    ".....x.x.....",
    "......x......",
    "......x......",
];

/// The rug of the entrance, in Leon's yellow, with the glare on it: 48 by
/// 32.
pub fn rug() -> Bitmap {
    let mut out = Bitmap::new(48, 32);
    out.fill(2, 4, 44, 25, ACID_SHADE);
    out.fill(1, 5, 46, 23, ACID_SHADE);
    out.fill(3, 5, 42, 23, ACID);
    out.fill(5, 7, 38, 1, ACID_SHADE);
    out.fill(5, 25, 38, 1, ACID_SHADE);
    out.fill(5, 7, 1, 19, ACID_SHADE);
    out.fill(42, 7, 1, 19, ACID_SHADE);
    out.draw(&drawing(&MARK), 17, 12);
    out
}

/// The source sheets a lion is made of. The second sheet of the six is left
/// out: its long hair covers the shoulders, and a head cannot be taken off
/// it cleanly.
pub const SOURCES: [usize; 5] = [0, 2, 3, 4, 5];

/// Every file the workshop makes: its path under `assets/` and its picture.
/// `source` reads the source sheet of a number.
pub fn everything(source: impl Fn(usize) -> Bitmap) -> Vec<(String, Bitmap)> {
    let mut files = Vec::new();
    for (lion, index) in SOURCES.iter().enumerate() {
        files.push((
            format!("lions/lion_{lion}.png"),
            lion_sheet(&source(*index)),
        ));
    }
    files.push(("lions/cub.png".to_owned(), cub_sheet()));
    for (name, bitmap) in [
        ("egg", egg_sheet()),
        ("nest", nest()),
        ("rack", rack_sheet()),
        ("telescope", telescope()),
        ("window", window()),
        ("rug", rug()),
        ("lion_painting", lion_painting()),
    ] {
        files.push((format!("props/{name}.png"), bitmap));
    }
    files
}

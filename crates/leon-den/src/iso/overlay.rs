//! What is written over the room in 2.5D: the name plates and the bubbles,
//! and where the pointer finds a lion.
//!
//! The picture of the room has no letters in it: they are the host's own
//! font, drawn over the picture where the [`Camera`] puts each lion.
//! [`lay`] answers with the labels, each a box with a line around it and a
//! word in it, and with a box around every lion for the pointer
//! ([`pick`]), the nearest to the eye winning where two overlap. All of it
//! is in the device pixels of the view, and none of it touches a window, so
//! a test can check where everything goes.

use gpui_kit::Hsla;

use crate::paint::tag_name;
use crate::palette::DenPalette;
use crate::pose::Bubble;
use crate::scene::Rect;

use super::camera::Camera;
use super::room::Stand;

/// A box with a word in it: a name plate, or a bubble.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// The lion it belongs to.
    pub id: u64,
    /// Where it is, in device pixels of the view.
    pub rect: Rect,
    /// What it says.
    pub text: String,
    /// Its fill.
    pub ground: Hsla,
    /// The line around it.
    pub line: Hsla,
    /// The colour of its word.
    pub ink: Hsla,
    /// A bubble, over the lion; otherwise a plate, under it.
    pub bubble: bool,
    /// The line from a plate that had to move to the feet of its lion.
    pub leader: Option<Rect>,
}

/// The box of a lion in the picture, for the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickBox {
    /// The lion.
    pub id: u64,
    /// Around it, in device pixels of the view.
    pub rect: Rect,
    /// How near the eye it is: the larger, the nearer.
    pub near: f32,
}

/// What is laid over a picture of the room.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
    /// The plates, then the bubbles.
    pub labels: Vec<Label>,
    /// A box a lion, for the pointer.
    pub boxes: Vec<PickBox>,
}

/// The measures of the letters of a label, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lettering {
    /// The advance of one character.
    pub char_w: f32,
    /// The height of a box.
    pub high: i32,
    /// The room left and right of the word.
    pub pad: i32,
}

/// How many lines down a plate may go to find a free place.
const REACH: i32 = 6;

/// What a bubble says, and in which colours: fill, line, word.
fn bubble(kind: Bubble, palette: &DenPalette) -> (&'static str, Hsla, Hsla, Hsla) {
    match kind {
        Bubble::Bang => ("!", palette.accent, palette.accent, palette.on_accent),
        Bubble::Urgent => ("!", palette.warning, palette.warning, palette.on_status),
        Bubble::Thought => ("...", palette.paper, palette.tick, palette.text),
        Bubble::Question => ("?", palette.paper, palette.info, palette.info),
        Bubble::Cross => ("x", palette.paper, palette.error, palette.error),
        Bubble::Tick => ("ok", palette.paper, palette.success, palette.success),
        Bubble::Zzz => ("Zz", palette.paper, palette.tick, palette.text_muted),
    }
}

/// Lays the plates, the bubbles and the pointer's boxes of the lions of a
/// room. `origin` is the top left of the picture in the view.
pub fn lay(
    stands: &[Stand],
    camera: &Camera,
    origin: (i32, i32),
    palette: &DenPalette,
    lettering: &Lettering,
) -> Overlay {
    let mut out = Overlay::default();
    let at = |point: [f32; 3]| {
        let (x, y) = camera.project(point);
        (origin.0 + x.round() as i32, origin.1 + y.round() as i32)
    };
    for stand in stands {
        let [x, _, z] = stand.feet;
        let (mut left, mut top, mut right, mut bottom) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for dx in [-stand.half, stand.half] {
            for dz in [-stand.half, stand.half] {
                for y in [0., stand.height - 0.1] {
                    let (px, py) = at([x + dx, y, z + dz]);
                    left = left.min(px);
                    right = right.max(px);
                    top = top.min(py);
                    bottom = bottom.max(py);
                }
            }
        }
        out.boxes.push(PickBox {
            id: stand.id,
            rect: Rect {
                x: left,
                y: top,
                w: (right - left).max(1),
                h: (bottom - top).max(1),
            },
            near: camera.nearness(stand.feet),
        });
    }

    let width = |text: &str| {
        (text.chars().count() as f32 * lettering.char_w).ceil() as i32 + lettering.pad * 2
    };
    // The bubbles first: a plate goes round them.
    let mut bubbles = Vec::new();
    for stand in stands {
        let Some(kind) = stand.bubble else {
            continue;
        };
        let (text, ground, line, ink) = bubble(kind, palette);
        let (x, y) = at([stand.feet[0], stand.height, stand.feet[2]]);
        let w = width(text).max(lettering.high);
        bubbles.push(Label {
            id: stand.id,
            rect: Rect {
                x: x - w / 2,
                y: y - lettering.high,
                w,
                h: lettering.high,
            },
            text: text.to_owned(),
            ground,
            line,
            ink,
            bubble: true,
            leader: None,
        });
    }
    // The plates: the selected lion's and the pointer's first, where they
    // belong; the others a line lower for each plate or bubble in the way.
    let mut order: Vec<&Stand> = stands.iter().filter(|stand| !stand.egg).collect();
    order.sort_by_key(|stand| (!stand.selected, !stand.hovered));
    for stand in order {
        let text = tag_name(&stand.name).to_uppercase();
        if text.is_empty() {
            continue;
        }
        let (x, y) = at(stand.feet);
        let w = width(&text);
        let step = lettering.high + 2;
        let mut rect = Rect {
            x: x - w / 2,
            y: y + lettering.high / 3,
            w,
            h: lettering.high,
        };
        let must = stand.selected || stand.hovered;
        let home = rect.y;
        if !must {
            // What a plate keeps off: the plates and bubbles already laid,
            // and the head and shoulders of every other lion. It goes down
            // a line at a time to the first free place, and where there is
            // none in reach it stays where it covers least.
            let faces: Vec<Rect> = out
                .boxes
                .iter()
                .filter(|pick| pick.id != stand.id)
                .map(|pick| Rect {
                    h: pick.rect.h * 6 / 10,
                    ..pick.rect
                })
                .collect();
            let covered = |rect: Rect| {
                let over = |other: Rect| {
                    let w = (rect.x + rect.w).min(other.x + other.w) - rect.x.max(other.x);
                    let h = (rect.y + rect.h).min(other.y + other.h) - rect.y.max(other.y);
                    i64::from(w.max(0)) * i64::from(h.max(0))
                };
                let labels: i64 = out
                    .labels
                    .iter()
                    .chain(&bubbles)
                    .map(|label| over(label.rect))
                    .sum();
                // A plate over a plate is worse than one over a face.
                labels * 4 + faces.iter().map(|face| over(*face)).sum::<i64>()
            };
            let mut best = (covered(rect), rect.y);
            for line in 1..=REACH {
                if best.0 == 0 {
                    break;
                }
                let lower = Rect {
                    y: home + step * line,
                    ..rect
                };
                let cost = covered(lower);
                if cost < best.0 {
                    best = (cost, lower.y);
                }
            }
            rect.y = best.1;
        }
        // A plate that left its lion is tied to it by a line.
        let leader = (rect.y > home).then_some(Rect {
            x,
            y: y + 1,
            w: 1,
            h: rect.y - y - 1,
        });
        let (ground, line, ink) = if stand.selected {
            (palette.accent, palette.on_accent, palette.on_accent)
        } else if stand.hovered {
            (palette.paper, palette.signal, palette.text)
        } else {
            (palette.paper, palette.tick, palette.text)
        };
        out.labels.push(Label {
            id: stand.id,
            rect,
            text,
            ground,
            line,
            ink,
            bubble: false,
            leader,
        });
    }
    // Back to front: the selected lion's plate is painted last, over the
    // others.
    out.labels.reverse();
    out.labels.extend(bubbles);
    out
}

/// The lion under a point of the view: on its own box or on its plate, the
/// nearest to the eye where two overlap.
pub fn pick(overlay: &Overlay, x: i32, y: i32) -> Option<u64> {
    if let Some(label) = overlay
        .labels
        .iter()
        .rev()
        .find(|label| !label.bubble && label.rect.contains(x, y))
    {
        return Some(label.id);
    }
    overlay
        .boxes
        .iter()
        .filter(|pick| pick.rect.contains(x, y))
        .max_by(|a, b| a.near.total_cmp(&b.near))
        .map(|pick| pick.id)
}

/// The box of a lion, if it is in the picture.
pub fn box_of(overlay: &Overlay, id: u64) -> Option<Rect> {
    overlay
        .boxes
        .iter()
        .find(|pick| pick.id == id)
        .map(|pick| pick.rect)
}

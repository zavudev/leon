//! The lion's parts on the 64-unit grid, and the shapes of a [`Pose`].
//!
//! The mark is one plate with cuts, drawn by the owner as a single
//! `evenodd` path (`leon-mark.svg`): the plate's outline, a left and a right
//! slit under a notched brow, and the chevron nose with its short stem. This
//! module takes that path apart into its four contours as point lists, so
//! each part can move, and puts them back: [`shapes`] at [`Pose::REST`]
//! returns exactly the points of the SVG, and [`path_data`] writes them as
//! the same `d` attribute, byte for byte.
//!
//! The origin is the top left and y grows downwards, as in the SVG.

use crate::motion::Pose;

/// A point on the 64-unit grid.
pub type Pt = (f32, f32);

/// The side of the grid.
pub const GRID: f32 = 64.;

/// The side, in logical pixels, at and below which the fitted small geometry
/// is drawn instead of the full one.
pub const SMALL_MAX: f32 = 20.;

/// The centre of the head: the point it breathes and leans about.
pub const CENTRE: Pt = (32., 34.);

/// The thinnest an eye gets, as a fraction of its open height: a closed eye
/// is a hairline, never nothing and never inside out.
pub const HAIRLINE: f32 = 0.04;

/// How far the outer corner of a slit drops at a full glare, in grid units.
const BROW_DROP: f32 = 0.7;
/// Where the top of the visible slit sits at a full glare, as a fraction of
/// its height from the top.
const NARROW_TOP: f32 = 0.18;
/// Where the bottom of the visible slit sits at a full glare.
const NARROW_BOTTOM: f32 = 0.70;
/// How far the nose chevron lifts at a full twitch, in grid units.
const NOSE_LIFT: f32 = 0.9;
/// How much smaller the plate is when it starts to arrive in the intro.
const ARRIVAL_SCALE: f32 = 0.6;

/// Which drawing of the lion: the full mark, or the one fitted to 16 to 20
/// pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variant {
    /// `leon-mark.svg`.
    Full,
    /// `leon-mark-16.svg`.
    Small,
}

impl Variant {
    /// The drawing for a mark `side` logical pixels wide.
    pub fn for_side(side: f32) -> Variant {
        if side <= SMALL_MAX {
            Variant::Small
        } else {
            Variant::Full
        }
    }
}

/// One slit, as the four corners that matter: the outer tip, the inner top
/// and bottom corners and the outer bottom corner. The first three of a
/// left eye in the SVG are its top edge and inner edge.
struct Slit {
    tip: Pt,
    top_inner: Pt,
    bottom_inner: Pt,
    bottom_outer: Pt,
}

/// The contours of one drawing of the lion.
struct Parts {
    plate: &'static [Pt],
    left: Slit,
    right: Slit,
    nose: [Pt; 9],
}

const PLATE: [Pt; 14] = [
    (32., 11.),
    (25., 6.),
    (5., 4.),
    (2., 16.),
    (11., 38.),
    (18., 54.),
    (27., 61.),
    (32., 61.),
    (37., 61.),
    (46., 54.),
    (53., 38.),
    (62., 16.),
    (59., 4.),
    (39., 6.),
];

const NOSE: [Pt; 9] = [
    (23., 43.),
    (32., 50.),
    (41., 43.),
    (41., 46.5),
    (33.25, 52.6),
    (33.25, 61.),
    (30.75, 61.),
    (30.75, 52.6),
    (23., 46.5),
];

/// `leon-mark.svg`.
const FULL: Parts = Parts {
    plate: &PLATE,
    left: Slit {
        tip: (8., 19.),
        top_inner: (28., 30.),
        bottom_inner: (28., 33.),
        bottom_outer: (13., 28.),
    },
    right: Slit {
        tip: (56., 19.),
        top_inner: (36., 30.),
        bottom_inner: (36., 33.),
        bottom_outer: (51., 28.),
    },
    nose: NOSE,
};

/// `leon-mark-16.svg`, the drawing fitted to small sizes. Today it is the
/// same drawing as the full one; it is a constant of its own so that the two
/// files can part ways without touching the motion.
const SMALL: Parts = Parts {
    plate: &PLATE,
    left: Slit {
        tip: (8., 19.),
        top_inner: (28., 30.),
        bottom_inner: (28., 33.),
        bottom_outer: (13., 28.),
    },
    right: Slit {
        tip: (56., 19.),
        top_inner: (36., 30.),
        bottom_inner: (36., 33.),
        bottom_outer: (51., 28.),
    },
    nose: NOSE,
};

impl Variant {
    fn parts(self) -> &'static Parts {
        match self {
            Variant::Full => &FULL,
            Variant::Small => &SMALL,
        }
    }
}

/// Everything to paint for one pose.
#[derive(Clone, Debug, PartialEq)]
pub struct Shapes {
    /// The plate's outline.
    pub plate: Vec<Pt>,
    /// The left slit, cut out of the plate. Same corner order as the SVG.
    pub left_eye: Vec<Pt>,
    /// The right slit, cut out of the plate. Same corner order as the SVG.
    pub right_eye: Vec<Pt>,
    /// The chevron nose and its stem, cut out of the plate.
    pub nose: Vec<Pt>,
    /// The two slits as solid shapes, where they are in the intro before the
    /// plate arrives: left, right.
    pub glow: [Vec<Pt>; 2],
    /// The opacity of the plate with its cuts: 1 once it has arrived.
    pub plate_opacity: f32,
    /// The opacity of the solid slits: 0 once the plate has arrived.
    pub glow_opacity: f32,
}

impl Shapes {
    /// The four contours of the one `evenodd` path: the plate, then the cuts.
    pub fn contours(&self) -> [Vec<Pt>; 4] {
        [
            self.plate.clone(),
            self.left_eye.clone(),
            self.right_eye.clone(),
            self.nose.clone(),
        ]
    }
}

/// The shapes of `pose` in the drawing `variant`. At [`Pose::REST`] they are
/// the points of the SVG, exactly.
pub fn shapes(pose: &Pose, variant: Variant) -> Shapes {
    let parts = variant.parts();
    let left = eye(&parts.left, pose);
    let right = eye(&parts.right, pose);
    let nose = nose(&parts.nose, pose);

    let arrival = ARRIVAL_SCALE + (1. - ARRIVAL_SCALE) * pose.plate.clamp(0., 1.);
    let scale = pose.scale * arrival;
    let place = |points: &[Pt]| -> Vec<Pt> {
        points
            .iter()
            .map(|&point| transform(point, scale, pose.lean))
            .collect()
    };

    // The SVG lists the right slit from its outer bottom corner round to its
    // tip: the reverse of the left one's order.
    let reversed = |slit: &[Pt; 4]| vec![slit[3], slit[2], slit[1], slit[0]];

    Shapes {
        plate: place(parts.plate),
        left_eye: place(&left),
        right_eye: place(&reversed(&right)),
        nose: place(&nose),
        // The glow is the slits before the plate arrives: where they are,
        // with their lids, but not leaning or breathing with the plate.
        glow: [left.to_vec(), reversed(&right)],
        plate_opacity: pose.plate.clamp(0., 1.),
        glow_opacity: pose.glow.clamp(0., 1.),
    }
}

/// A point scaled and leaned about the head's centre. The identity returns
/// the point untouched, so the rest pose is exact.
fn transform(point: Pt, scale: f32, lean: f32) -> Pt {
    if scale == 1. && lean == 0. {
        return point;
    }
    let (sin, cos) = lean.to_radians().sin_cos();
    let (dx, dy) = ((point.0 - CENTRE.0) * scale, (point.1 - CENTRE.1) * scale);
    (
        CENTRE.0 + dx * cos - dy * sin,
        CENTRE.1 + dx * sin + dy * cos,
    )
}

/// The four corners of a slit in a pose: `[tip, top inner, bottom inner,
/// bottom outer]`.
fn eye(slit: &Slit, pose: &Pose) -> [Pt; 4] {
    let narrow = pose.narrow.clamp(0., 1.);
    let open = pose.eye_open.clamp(0., 1.).max(HAIRLINE);
    if narrow == 0. && open == 1. && pose.glance == 0. {
        return [
            slit.tip,
            slit.top_inner,
            slit.bottom_inner,
            slit.bottom_outer,
        ];
    }

    // The visible part of the slit, as fractions of its height from the
    // top: a glare takes some off both lids, a blink closes it towards its
    // middle.
    let top = NARROW_TOP * narrow;
    let bottom = 1. - (1. - NARROW_BOTTOM) * narrow;
    let middle = (top + bottom) / 2.;
    let half = (bottom - top) / 2. * open;
    let (top, bottom) = (middle - half, middle + half);
    let thickness = bottom - top;

    // The top edge runs from the tip to the inner top corner, which drops
    // with the upper lid; the tip itself only drops with the brow.
    let tip = (slit.tip.0, slit.tip.1 + BROW_DROP * narrow);
    let inner_top = (
        slit.top_inner.0,
        lerp(slit.top_inner.1, slit.bottom_inner.1, top),
    );
    let top_edge = |x: f32| {
        let along = (x - tip.0) / (inner_top.0 - tip.0);
        tip.1 + along * (inner_top.1 - tip.1)
    };
    // How thick the open slit is at a given x, from the drawing.
    let rest_top_edge = |x: f32| {
        let along = (x - slit.tip.0) / (slit.top_inner.0 - slit.tip.0);
        slit.tip.1 + along * (slit.top_inner.1 - slit.tip.1)
    };
    let inner_height = slit.bottom_inner.1 - slit.top_inner.1;
    let outer_height = slit.bottom_outer.1 - rest_top_edge(slit.bottom_outer.0);

    let shift = pose.glance;
    [
        (tip.0 + shift, tip.1),
        (inner_top.0 + shift, inner_top.1),
        (
            slit.bottom_inner.0 + shift,
            inner_top.1 + thickness * inner_height,
        ),
        (
            slit.bottom_outer.0 + shift,
            top_edge(slit.bottom_outer.0) + thickness * outer_height,
        ),
    ]
}

/// The nose chevron in a pose: everything lifts except the two points of the
/// stem on the chin's edge.
fn nose(rest: &[Pt; 9], pose: &Pose) -> Vec<Pt> {
    let lift = NOSE_LIFT * pose.nose.clamp(0., 1.);
    rest.iter()
        .enumerate()
        .map(|(index, &(x, y))| {
            if lift == 0. || index == 5 || index == 6 {
                (x, y)
            } else {
                (x, y - lift)
            }
        })
        .collect()
}

fn lerp(a: f32, b: f32, amount: f32) -> f32 {
    a + (b - a) * amount
}

/// Writes contours as the `d` attribute of an SVG path: `M`, `L` and `Z`
/// only, numbers to two decimals without trailing zeros, nothing between
/// commands, exactly as `leon-mark.svg` is written.
pub fn path_data(contours: &[Vec<Pt>]) -> String {
    let mut d = String::new();
    for contour in contours {
        for (index, &(x, y)) in contour.iter().enumerate() {
            d.push(if index == 0 { 'M' } else { 'L' });
            d.push_str(&number(x));
            d.push(' ');
            d.push_str(&number(y));
        }
        d.push('Z');
    }
    d
}

/// A number to two decimals, shortest form, never `-0`.
pub fn number(value: f32) -> String {
    let rounded = (value * 100.).round() / 100.;
    if rounded == 0. {
        return "0".to_owned();
    }
    format!("{rounded}")
}

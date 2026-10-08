//! The eye of the room in 2.5D: turned an eighth, seen from above, with no
//! perspective.
//!
//! A [`Camera`] is fitted to a room and to a picture ([`Camera::fit`]): the
//! whole room shows, as large as it can, whatever the shape of the picture.
//! It says where a point of the room is in the picture ([`Camera::project`])
//! and, the other way, which point of the floor is under a pixel
//! ([`Camera::floor`]): what the pointer needs. The same numbers go to the
//! GPU as a matrix ([`Camera::matrix`]), with the sun's own view for the
//! shadows ([`Camera::sun_matrix`]).
//!
//! The room's `x` runs along the back wall, to the right; `z` comes out of
//! it, toward the way in; `y` is up. A tile is one unit, and tile `(c, r)`
//! is the square from `(c, r)` to `(c + 1, r + 1)`.

use super::mesh::{dot, unit};

/// How high the walls stand.
pub const WALL_HEIGHT: f32 = 2.6;
/// How far above the floor the camera looks down from, in degrees.
pub const TILT: f32 = 30.;
/// Where the sun stands: the way to it from anywhere in the room.
pub const SUN: [f32; 3] = [0.28, 0.82, 0.5];
/// How far apart the nearest and the farthest thing may be.
const DEPTH: f32 = 120.;

/// The eye, fitted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// The size of the picture, in its pixels.
    pub size: (f32, f32),
    /// How many pixels a unit of the room is.
    pub scale: f32,
    /// The pixel the room's origin falls on.
    offset: (f32, f32),
    right: [f32; 3],
    up: [f32; 3],
    /// The way to the eye.
    toward: [f32; 3],
    /// The middle of the room.
    centre: [f32; 3],
    /// Half of the room's longest reach, for the sun.
    reach: f32,
    tilt: (f32, f32),
}

impl Camera {
    /// The camera for a room of `cols` by `rows` tiles in a picture of
    /// `width` by `height` pixels, with `margin` pixels left free around it.
    pub fn fit(cols: i32, rows: i32, width: f32, height: f32, margin: f32) -> Camera {
        let (w, d) = (cols.max(1) as f32, rows.max(1) as f32);
        // What must show: the floor inside the walls, the walls, and a head
        // over whoever stands at the way in.
        let mut camera = Camera::around(
            [0.8, -0.3, 1.8],
            [w - 0.9, WALL_HEIGHT, d],
            width,
            height,
            margin,
        );
        camera.centre = [w / 2., 0.6, d / 2.];
        camera.reach = (w * w + d * d).sqrt() / 2. + 3.;
        camera
    }

    /// The camera that shows the whole of a box of the room, from its
    /// lowest corner `low` to its highest `high`, as large as it can in a
    /// picture of `width` by `height` pixels with `margin` left free: what
    /// a picture of one piece, or of a part of a room, is taken with.
    pub fn around(low: [f32; 3], high: [f32; 3], width: f32, height: f32, margin: f32) -> Camera {
        let c = std::f32::consts::FRAC_1_SQRT_2;
        let (sin, cos) = TILT.to_radians().sin_cos();
        let (right, up) = ([c, 0., -c], [-sin * c, cos, -sin * c]);
        let corners = [low[0], high[0]]
            .into_iter()
            .flat_map(|x| [low[2], high[2]].into_iter().map(move |z| (x, z)))
            .flat_map(|(x, z)| [low[1], high[1]].into_iter().map(move |y| [x, y, z]));
        let (mut left, mut top, mut right_most, mut bottom) =
            (f32::MAX, f32::MIN, f32::MIN, f32::MAX);
        for corner in corners {
            let (x, y) = (dot(corner, right), dot(corner, up));
            left = left.min(x);
            right_most = right_most.max(x);
            top = top.max(y);
            bottom = bottom.min(y);
        }
        let (wide, tall) = ((right_most - left).max(1e-3), (top - bottom).max(1e-3));
        let (free_w, free_h) = (
            (width - margin * 2.).max(8.),
            (height - margin * 2.).max(8.),
        );
        let scale = (free_w / wide).min(free_h / tall);
        let span = [high[0] - low[0], high[1] - low[1], high[2] - low[2]];
        Camera {
            size: (width, height),
            scale,
            offset: (
                width / 2. - scale * (left + right_most) / 2.,
                height / 2. + scale * (top + bottom) / 2.,
            ),
            right,
            up,
            toward: [cos * c, sin, cos * c],
            centre: [
                (low[0] + high[0]) / 2.,
                (low[1] + high[1]) / 2.,
                (low[2] + high[2]) / 2.,
            ],
            reach: dot(span, span).sqrt() / 2. + 3.,
            tilt: (sin, cos),
        }
    }

    /// Where a point of the room is in the picture, in its pixels.
    pub fn project(&self, point: [f32; 3]) -> (f32, f32) {
        (
            self.offset.0 + self.scale * dot(point, self.right),
            self.offset.1 - self.scale * dot(point, self.up),
        )
    }

    /// How near the eye a point is: the larger, the nearer.
    pub fn nearness(&self, point: [f32; 3]) -> f32 {
        dot(point, self.toward)
    }

    /// The point at height `y` that a pixel of the picture shows: the
    /// floor's, with `y` nothing.
    pub fn floor(&self, x: f32, y: f32, height: f32) -> (f32, f32) {
        let c = std::f32::consts::FRAC_1_SQRT_2;
        let (sin, cos) = self.tilt;
        let across = (x - self.offset.0) / self.scale;
        let above = (self.offset.1 - y) / self.scale;
        // across = c (x - z); above = cos y - sin c (x + z).
        let (difference, sum) = (across / c, (cos * height - above) / (sin * c));
        ((sum + difference) / 2., (sum - difference) / 2.)
    }

    fn rows(right: [f32; 4], up: [f32; 4], away: [f32; 4]) -> [f32; 16] {
        let mut out = [0.; 16];
        for (row, values) in [right, up, away].iter().enumerate() {
            for (column, value) in values.iter().enumerate() {
                out[column * 4 + row] = *value;
            }
        }
        out[15] = 1.;
        out
    }

    /// The camera as the GPU wants it, column first: from the room to the
    /// picture, the nearest at depth 0.
    pub fn matrix(&self) -> [f32; 16] {
        let (w, h) = self.size;
        let (kx, ky) = (2. * self.scale / w, 2. * self.scale / h);
        let away = self.toward.map(|part| -part / DEPTH);
        Self::rows(
            [
                self.right[0] * kx,
                self.right[1] * kx,
                self.right[2] * kx,
                2. * self.offset.0 / w - 1.,
            ],
            [
                self.up[0] * ky,
                self.up[1] * ky,
                self.up[2] * ky,
                1. - 2. * self.offset.1 / h,
            ],
            [away[0], away[1], away[2], 0.5 - dot(away, self.centre)],
        )
    }

    /// The sun's own view of the room, for the shadows.
    pub fn sun_matrix(&self) -> [f32; 16] {
        let sun = unit(SUN);
        let right = unit([sun[2], 0., -sun[0]]);
        let up = [
            sun[1] * right[2] - sun[2] * right[1],
            sun[2] * right[0] - sun[0] * right[2],
            sun[0] * right[1] - sun[1] * right[0],
        ];
        let scaled = |axis: [f32; 3], by: f32| axis.map(|part| part / by);
        let (right, up, away) = (
            scaled(right, self.reach),
            scaled(up, self.reach),
            scaled(sun, -DEPTH),
        );
        Self::rows(
            [right[0], right[1], right[2], -dot(right, self.centre)],
            [up[0], up[1], up[2], -dot(up, self.centre)],
            [away[0], away[1], away[2], 0.5 - dot(away, self.centre)],
        )
    }
}

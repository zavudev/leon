//! What a room in 2.5D is made of: flat-shaded boxes and faceted balls, and
//! the hairlines on their edges.
//!
//! A [`Mesh`] is two lists of numbers, ready for the GPU: triangles as
//! `position, normal, colour` per corner ([`FACE_FLOATS`] numbers) and lines
//! as `position, colour` per end ([`LINE_FLOATS`]). A normal of nothing marks
//! a face that gives its own light: a screen, a lamp, the rug of the
//! entrance. Everything is put through a [`Part`]: a place of the room with
//! its own turn and size, so that a piece is drawn once, about its own
//! origin, and stands wherever it is put.

/// A colour as red, green and blue from 0 to 1, in sRGB.
pub type Rgb = [f32; 3];

/// How many numbers a corner of a triangle is: position, normal, colour.
pub const FACE_FLOATS: usize = 9;
/// How many numbers an end of a line is: position, colour.
pub const LINE_FLOATS: usize = 6;

/// A colour from its six hexadecimal digits.
pub fn hex(value: u32) -> Rgb {
    [
        ((value >> 16) & 0xff) as f32 / 255.,
        ((value >> 8) & 0xff) as f32 / 255.,
        (value & 0xff) as f32 / 255.,
    ]
}

/// `from` moved `amount` of the way to `to`.
pub fn blend(from: Rgb, to: Rgb, amount: f32) -> Rgb {
    let amount = amount.clamp(0., 1.);
    [
        from[0] + (to[0] - from[0]) * amount,
        from[1] + (to[1] - from[1]) * amount,
        from[2] + (to[2] - from[2]) * amount,
    ]
}

/// A colour made darker (`by` under 1) or lighter.
pub fn shade(color: Rgb, by: f32) -> Rgb {
    color.map(|part| (part * by).clamp(0., 1.))
}

/// A vector of length one, or the vector itself when it has no length.
pub fn unit([x, y, z]: [f32; 3]) -> [f32; 3] {
    let length = (x * x + y * y + z * z).sqrt().max(1e-6);
    [x / length, y / length, z / length]
}

/// The product of two vectors, number by number, summed.
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn times(a: [f32; 3], by: f32) -> [f32; 3] {
    [a[0] * by, a[1] * by, a[2] * by]
}

/// A place of the room to build at: where its origin is, and where its own
/// right, up and front point. `x` is to the right along the back wall, `y`
/// is up and `z` comes out of the back wall toward the way in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    origin: [f32; 3],
    axes: [[f32; 3]; 3],
}

impl Part {
    /// The room itself.
    pub const ROOT: Part = Part {
        origin: [0.; 3],
        axes: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
    };

    /// A place of the room, not turned.
    pub fn at(x: f32, y: f32, z: f32) -> Part {
        Part {
            origin: [x, y, z],
            ..Part::ROOT
        }
    }

    /// Where a point of the part is in the room.
    pub fn point(self, [x, y, z]: [f32; 3]) -> [f32; 3] {
        add(
            self.origin,
            add(
                add(times(self.axes[0], x), times(self.axes[1], y)),
                times(self.axes[2], z),
            ),
        )
    }

    /// Where a direction of the part points in the room, at length one.
    pub fn direction(self, [x, y, z]: [f32; 3]) -> [f32; 3] {
        unit(add(
            add(times(self.axes[0], x), times(self.axes[1], y)),
            times(self.axes[2], z),
        ))
    }

    /// The same part, its origin moved to one of its own points.
    pub fn moved(self, to: [f32; 3]) -> Part {
        Part {
            origin: self.point(to),
            ..self
        }
    }

    /// Turned about its own up: a quarter turn (`PI / 2`) brings its front
    /// to where its right was.
    pub fn turned(self, angle: f32) -> Part {
        let (sin, cos) = angle.sin_cos();
        let [x, y, z] = self.axes;
        Part {
            axes: [
                add(times(x, cos), times(z, -sin)),
                y,
                add(times(x, sin), times(z, cos)),
            ],
            ..self
        }
    }

    /// Tipped about its own right: a positive angle brings its top forward.
    pub fn pitched(self, angle: f32) -> Part {
        let (sin, cos) = angle.sin_cos();
        let [x, y, z] = self.axes;
        Part {
            axes: [
                x,
                add(times(y, cos), times(z, sin)),
                add(times(y, -sin), times(z, cos)),
            ],
            ..self
        }
    }

    /// Leant about its own front: a positive angle brings its top to its
    /// left.
    pub fn rolled(self, angle: f32) -> Part {
        let (sin, cos) = angle.sin_cos();
        let [x, y, z] = self.axes;
        Part {
            axes: [
                add(times(x, cos), times(y, sin)),
                add(times(x, -sin), times(y, cos)),
                z,
            ],
            ..self
        }
    }

    /// Made larger or smaller about its origin.
    pub fn scaled(self, by: f32) -> Part {
        Part {
            axes: self.axes.map(|axis| times(axis, by)),
            ..self
        }
    }
}

/// How a box is finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// Lit, with hairlines on its edges.
    Lined,
    /// Lit.
    Plain,
    /// Its own light: it is the colour it is given, whatever the sun does.
    Glow,
}

/// The triangles and the lines of a room.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    /// The triangles: [`FACE_FLOATS`] numbers a corner.
    pub faces: Vec<f32>,
    /// The hairlines: [`LINE_FLOATS`] numbers an end.
    pub lines: Vec<f32>,
    /// The colour of the hairlines on the edges of a [`Finish::Lined`] box.
    pub edge: Rgb,
    /// Whether boxes get their hairlines at all.
    pub hairlines: bool,
}

const CUBE: [([f32; 3], [[f32; 3]; 4]); 6] = [
    (
        [0., 1., 0.],
        [[0., 1., 0.], [0., 1., 1.], [1., 1., 1.], [1., 1., 0.]],
    ),
    (
        [0., -1., 0.],
        [[0., 0., 0.], [1., 0., 0.], [1., 0., 1.], [0., 0., 1.]],
    ),
    (
        [1., 0., 0.],
        [[1., 0., 0.], [1., 1., 0.], [1., 1., 1.], [1., 0., 1.]],
    ),
    (
        [-1., 0., 0.],
        [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]],
    ),
    (
        [0., 0., 1.],
        [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]],
    ),
    (
        [0., 0., -1.],
        [[0., 0., 0.], [0., 1., 0.], [1., 1., 0.], [1., 0., 0.]],
    ),
];

const EDGES: [([f32; 3], [f32; 3]); 12] = [
    ([0., 0., 0.], [1., 0., 0.]),
    ([0., 0., 1.], [1., 0., 1.]),
    ([0., 1., 0.], [1., 1., 0.]),
    ([0., 1., 1.], [1., 1., 1.]),
    ([0., 0., 0.], [0., 0., 1.]),
    ([1., 0., 0.], [1., 0., 1.]),
    ([0., 1., 0.], [0., 1., 1.]),
    ([1., 1., 0.], [1., 1., 1.]),
    ([0., 0., 0.], [0., 1., 0.]),
    ([1., 0., 0.], [1., 1., 0.]),
    ([0., 0., 1.], [0., 1., 1.]),
    ([1., 0., 1.], [1., 1., 1.]),
];

impl Mesh {
    /// An empty mesh whose boxes are lined in `edge`.
    pub fn new(edge: Rgb, hairlines: bool) -> Mesh {
        Mesh {
            edge,
            hairlines,
            ..Mesh::default()
        }
    }

    /// How many triangles it has.
    pub fn triangles(&self) -> usize {
        self.faces.len() / (FACE_FLOATS * 3)
    }

    fn corner(&mut self, position: [f32; 3], normal: [f32; 3], color: Rgb) {
        self.faces.extend(position);
        self.faces.extend(normal);
        self.faces.extend(color);
    }

    /// A hairline between two points of the room.
    pub fn line(&mut self, from: [f32; 3], to: [f32; 3], color: Rgb) {
        self.lines.extend(from);
        self.lines.extend(color);
        self.lines.extend(to);
        self.lines.extend(color);
    }

    /// A box that stands on the middle of its base at `base`, a point of
    /// the part.
    pub fn solid(
        &mut self,
        part: Part,
        extent: [f32; 3],
        color: Rgb,
        base: [f32; 3],
        finish: Finish,
    ) {
        let [w, h, d] = extent;
        let part = part.moved(base);
        let place = |[u, v, t]: [f32; 3]| part.point([(u - 0.5) * w, v * h, (t - 0.5) * d]);
        for (normal, corners) in CUBE {
            let normal = if finish == Finish::Glow {
                [0.; 3]
            } else {
                part.direction(normal)
            };
            for index in [0, 1, 2, 0, 2, 3] {
                self.corner(place(corners[index]), normal, color);
            }
        }
        if finish == Finish::Lined && self.hairlines {
            let edge = self.edge;
            for (from, to) in EDGES {
                self.line(place(from), place(to), edge);
            }
        }
    }

    /// A box with hairlines on its edges.
    pub fn piece(&mut self, part: Part, extent: [f32; 3], color: Rgb, base: [f32; 3]) {
        self.solid(part, extent, color, base, Finish::Lined);
    }

    /// A box without.
    pub fn bit(&mut self, part: Part, extent: [f32; 3], color: Rgb, base: [f32; 3]) {
        self.solid(part, extent, color, base, Finish::Plain);
    }

    /// A box that gives its own light.
    pub fn glow(&mut self, part: Part, extent: [f32; 3], color: Rgb, base: [f32; 3]) {
        self.solid(part, extent, color, base, Finish::Glow);
    }

    /// A flat shape that gives its own light, lying in the part at height
    /// `y`: the corners of a convex outline, as `[x, z]`.
    pub fn shape(&mut self, part: Part, outline: &[[f32; 2]], y: f32, color: Rgb) {
        let place = |[x, z]: [f32; 2]| part.point([x, y, z]);
        for index in 1..outline.len().saturating_sub(1) {
            for corner in [outline[0], outline[index], outline[index + 1]] {
                self.corner(place(corner), [0.; 3], color);
            }
        }
    }

    /// A faceted ball about `centre`: an icosahedron, or one cut finer.
    pub fn ball(&mut self, part: Part, radius: [f32; 3], color: Rgb, centre: [f32; 3], fine: bool) {
        let t = (1. + 5f32.sqrt()) / 2.;
        let points = [
            [-1., t, 0.],
            [1., t, 0.],
            [-1., -t, 0.],
            [1., -t, 0.],
            [0., -1., t],
            [0., 1., t],
            [0., -1., -t],
            [0., 1., -t],
            [t, 0., -1.],
            [t, 0., 1.],
            [-t, 0., -1.],
            [-t, 0., 1.],
        ]
        .map(unit);
        const FACES: [[usize; 3]; 20] = [
            [0, 11, 5],
            [0, 5, 1],
            [0, 1, 7],
            [0, 7, 10],
            [0, 10, 11],
            [1, 5, 9],
            [5, 11, 4],
            [11, 10, 2],
            [10, 7, 6],
            [7, 1, 8],
            [3, 9, 4],
            [3, 4, 2],
            [3, 2, 6],
            [3, 6, 8],
            [3, 8, 9],
            [4, 9, 5],
            [2, 4, 11],
            [6, 2, 10],
            [8, 6, 7],
            [9, 8, 1],
        ];
        let mut triangles: Vec<[[f32; 3]; 3]> = FACES
            .iter()
            .map(|[a, b, c]| [points[*a], points[*b], points[*c]])
            .collect();
        if fine {
            let middle = |a: [f32; 3], b: [f32; 3]| unit(add(a, b));
            triangles = triangles
                .into_iter()
                .flat_map(|[a, b, c]| {
                    let (ab, bc, ca) = (middle(a, b), middle(b, c), middle(c, a));
                    [[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]
                })
                .collect();
        }
        let middle = part.point(centre);
        for triangle in triangles {
            let placed = triangle.map(|[x, y, z]| {
                part.point([
                    centre[0] + x * radius[0],
                    centre[1] + y * radius[1],
                    centre[2] + z * radius[2],
                ])
            });
            let (u, v) = (
                add(placed[1], times(placed[0], -1.)),
                add(placed[2], times(placed[0], -1.)),
            );
            let mut normal = unit([
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ]);
            // Out of the ball, whichever way the part is turned.
            if dot(normal, add(placed[0], times(middle, -1.))) < 0. {
                normal = times(normal, -1.);
            }
            for corner in placed {
                self.corner(corner, normal, color);
            }
        }
    }
}

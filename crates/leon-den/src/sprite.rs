//! A glyph as data: a drawing in letters, cut into as few rectangles as it
//! takes.
//!
//! A [`Sprite`] is written as rows of text, one letter per pixel, each letter
//! a [`Role`] of the palette (see [`Role::of`]). It is parsed once and kept as
//! [`Run`]s: rectangles of one role, merged along the row and then down the
//! column, so that a glyph is a handful of fills, whether the painter lays
//! it on the room or the scene draws it in the chrome. A [`Stamp`] places a
//! sprite, mirrored or upside down if asked.

use crate::palette::Role;

/// A rectangle of one role, in the pixels of the art.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
    /// What it is.
    pub role: Role,
}

/// A drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sprite {
    /// Width in pixels.
    pub w: i32,
    /// Height in pixels.
    pub h: i32,
    cells: Vec<Option<Role>>,
    runs: Vec<Run>,
}

impl Sprite {
    /// Reads a drawing. Every row must be as long as the first, and every
    /// letter must be `.`, a space or a role: a drawing is code, so a wrong
    /// one is a bug and panics (a test parses them all).
    pub fn parse(rows: &[&str]) -> Sprite {
        let w = rows.first().map_or(0, |row| row.chars().count());
        let mut cells = Vec::with_capacity(w * rows.len());
        for (y, row) in rows.iter().enumerate() {
            assert_eq!(
                row.chars().count(),
                w,
                "row {y} of a sprite is not {w} wide: {row:?}"
            );
            for letter in row.chars() {
                let role = Role::of(letter);
                assert!(
                    role.is_some() || letter == '.' || letter == ' ',
                    "unknown letter {letter:?} in a sprite row {row:?}"
                );
                cells.push(role);
            }
        }
        Sprite::from_cells(w as i32, rows.len() as i32, cells)
    }

    /// A drawing from its pixels, row by row.
    pub fn from_cells(w: i32, h: i32, cells: Vec<Option<Role>>) -> Sprite {
        assert_eq!(cells.len(), (w * h) as usize, "a sprite needs w*h cells");
        let runs = merge(w, h, &cells);
        Sprite { w, h, cells, runs }
    }

    /// The pixel at a place, or nothing outside the drawing.
    pub fn get(&self, x: i32, y: i32) -> Option<Role> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        self.cells[(y * self.w + x) as usize]
    }

    /// The drawing as rectangles.
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// How many pixels are drawn.
    pub fn area(&self) -> usize {
        self.cells.iter().flatten().count()
    }
}

/// Cuts a grid of pixels into rectangles: runs along each row, then equal
/// runs of consecutive rows joined.
fn merge(w: i32, h: i32, cells: &[Option<Role>]) -> Vec<Run> {
    let mut done: Vec<Run> = Vec::new();
    // Runs that ended on the previous row and may still grow down.
    let mut open: Vec<Run> = Vec::new();
    for y in 0..h {
        let mut row: Vec<Run> = Vec::new();
        let mut x = 0;
        while x < w {
            let Some(role) = cells[(y * w + x) as usize] else {
                x += 1;
                continue;
            };
            let start = x;
            while x < w && cells[(y * w + x) as usize] == Some(role) {
                x += 1;
            }
            row.push(Run {
                x: start,
                y,
                w: x - start,
                h: 1,
                role,
            });
        }
        let mut next_open = Vec::with_capacity(row.len());
        for run in row {
            let above = open
                .iter()
                .position(|o| o.x == run.x && o.w == run.w && o.role == run.role);
            match above {
                Some(index) => {
                    let mut grown = open.swap_remove(index);
                    grown.h += 1;
                    next_open.push(grown);
                }
                None => next_open.push(run),
            }
        }
        done.append(&mut open);
        open = next_open;
    }
    done.append(&mut open);
    done.sort_by_key(|run| (run.y, run.x));
    done
}

/// A sprite at a place.
#[derive(Clone, Copy, Debug)]
pub struct Stamp<'a> {
    /// What is drawn.
    pub sprite: &'a Sprite,
    /// Where its left edge goes.
    pub x: i32,
    /// Where its top edge goes.
    pub y: i32,
    /// Mirrored left to right.
    pub flip_h: bool,
    /// Upside down.
    pub flip_v: bool,
}

impl<'a> Stamp<'a> {
    /// The sprite as drawn, at a place.
    pub fn at(sprite: &'a Sprite, x: i32, y: i32) -> Self {
        Self {
            sprite,
            x,
            y,
            flip_h: false,
            flip_v: false,
        }
    }

    /// Mirrored left to right, if `flip`.
    pub fn mirrored(mut self, flip: bool) -> Self {
        self.flip_h = flip;
        self
    }

    /// Upside down, if `flip`.
    pub fn upside_down(mut self, flip: bool) -> Self {
        self.flip_v = flip;
        self
    }

    /// The rectangles of the stamp, where they land.
    pub fn runs(self) -> impl Iterator<Item = Run> + 'a {
        let Stamp {
            sprite,
            x,
            y,
            flip_h,
            flip_v,
        } = self;
        sprite.runs().iter().map(move |run| Run {
            x: x + if flip_h {
                sprite.w - run.x - run.w
            } else {
                run.x
            },
            y: y + if flip_v {
                sprite.h - run.y - run.h
            } else {
                run.y
            },
            ..*run
        })
    }
}

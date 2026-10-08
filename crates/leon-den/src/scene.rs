//! The layout of the view, and its chrome: what is painted around and over
//! the room.
//!
//! [`Layout::compute`] cuts the view into its parts for a size in device
//! pixels. A view at least [`SIDE_MIN_VIEW`] logical pixels wide has the
//! room on the left and a column on the right: the roster on top, as high as
//! its lions, and the feed under it. A narrower one has the feed under the
//! room and no roster, and one lower than [`BELOW_MIN_VIEW`] is all room.
//! The art pixel is a whole number of device pixels, the largest at which
//! the room fits in what is left.
//!
//! [`build`] then writes a [`Scene`]: where the picture of the room goes,
//! and every rectangle and piece of text of the chrome, in device pixels,
//! back to front. The room itself is one picture, painted by
//! [`crate::paint::compose`]. There is no GPUI here, so a test can check a
//! layout and an example can paint the same scene into a PNG file.

use gpui_kit::Hsla;

use crate::model::Status;
use crate::palette::DenPalette;
use crate::pose::TILE;
use crate::sim::{Actor, Den, Frame, RosterEntry, TruthCard};

/// A rectangle in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

impl Rect {
    /// Whether a point is inside.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// A filled rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    /// Where.
    pub rect: Rect,
    /// In what colour.
    pub color: Hsla,
}

/// A piece of text in the host's monospace font.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    /// The left edge of the first character, in device pixels.
    pub x: i32,
    /// The top of the line.
    pub y: i32,
    /// What it says.
    pub text: String,
    /// In what colour.
    pub color: Hsla,
    /// In the heavier weight.
    pub bold: bool,
}

/// The measures of the host's monospace font, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextMetrics {
    /// The advance of one character.
    pub char_w: f32,
    /// The height of a line.
    pub line_h: f32,
}

/// The parts of the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The whole view.
    pub view: Rect,
    /// Device pixels per logical pixel.
    pub scale: f32,
    /// Device pixels per pixel of the art: a whole number.
    pub unit: i32,
    /// The width of the map in tiles.
    pub cols: i32,
    /// The height of the map in tiles.
    pub rows: i32,
    /// Where the room may be: the view less the roster and the feed.
    pub field: Rect,
    /// The room, centred in the field.
    pub map: Rect,
    /// The roster, when the view is wide enough for a column at its side.
    pub roster: Option<Rect>,
    /// The feed: under the roster in a wide view, under the room in a
    /// narrow one, and nowhere in a view too small for it.
    pub feed: Option<Rect>,
    /// The font.
    pub metrics: TextMetrics,
}

/// The width of the column of the roster and the feed, in characters.
pub const SIDE_CHARS: usize = 38;
/// The width of the roster's own columns, in characters.
const ROSTER_CHARS: usize = 27;
/// A view at least this many logical pixels wide has the column at its
/// side; a narrower one has the feed under the room and no roster.
pub const SIDE_MIN_VIEW: f32 = 820.;
/// A narrow view at least this high has the feed under the room; a lower
/// one is all room.
pub const BELOW_MIN_VIEW: f32 = 420.;
/// A view lower than this has no column even when it is wide.
pub const SIDE_MIN_HEIGHT: f32 = 260.;
/// How many lions the roster lists at most before it says "+N more".
pub const ROSTER_ROWS: usize = 8;
/// An art pixel is at most this many logical pixels.
const MAX_UNIT: f32 = 5.;

impl Layout {
    /// The layout of a view of `width` by `height` device pixels at a
    /// display `scale`, for a room of `cols` by `rows` tiles in which
    /// `lions` are listed.
    pub fn compute(
        width: i32,
        height: i32,
        scale: f32,
        metrics: TextMetrics,
        cols: i32,
        rows: i32,
        lions: usize,
    ) -> Self {
        let scale = if scale > 0. { scale } else { 1. };
        let px = |logical: f32| (logical * scale).round() as i32;
        let (gap, pad) = (px(12.), px(10.));
        let line_h = metrics.line_h.ceil() as i32;
        let row_h = line_h + px(6.);
        let side_w = (metrics.char_w * SIDE_CHARS as f32).ceil() as i32 + pad * 2;
        let largest = (MAX_UNIT * scale).floor().max(1.) as i32;
        let (cols, rows) = (cols.max(1), rows.max(1));
        let logical = |device: i32| device as f32 / scale;

        let side = logical(width) >= SIDE_MIN_VIEW && logical(height) >= SIDE_MIN_HEIGHT;
        let below = !side && logical(height) >= BELOW_MIN_VIEW;
        let (field, roster, feed) = if side {
            let column = Rect {
                x: width - side_w - gap,
                y: gap,
                w: side_w,
                h: (height - gap * 2).max(1),
            };
            // The roster is as high as its lions, up to a number of them and
            // to two fifths of the column: the feed has the rest.
            let head = pad + row_h + px(4.);
            let most = (((column.h * 2 / 5) - head - pad) / row_h).max(1) as usize;
            let listed = lions.clamp(1, ROSTER_ROWS.min(most).max(1));
            let roster = Rect {
                h: (head + listed as i32 * row_h + pad).min(column.h),
                ..column
            };
            let feed = Rect {
                y: roster.y + roster.h + gap,
                h: column.h - roster.h - gap,
                ..column
            };
            let field = Rect {
                x: gap,
                y: gap,
                w: (width - side_w - gap * 3).max(1),
                h: (height - gap * 2).max(1),
            };
            // A column too low for both is all feed.
            if feed.h < line_h * 5 + pad * 2 {
                (field, None, Some(column))
            } else {
                (field, Some(roster), Some(feed))
            }
        } else if below {
            let feed_h = (height * 34 / 100).max(line_h * 6 + pad * 2);
            let feed = Rect {
                x: gap,
                y: height - gap - feed_h,
                w: (width - gap * 2).max(1),
                h: feed_h,
            };
            let field = Rect {
                x: gap,
                y: gap,
                w: feed.w,
                h: (height - feed_h - gap * 3).max(1),
            };
            (field, None, Some(feed))
        } else {
            let field = Rect {
                x: gap.min(width / 8),
                y: gap.min(height / 8),
                w: (width - gap.min(width / 8) * 2).max(1),
                h: (height - gap.min(height / 8) * 2).max(1),
            };
            (field, None, None)
        };

        // The size of an art pixel: the largest whole number of device
        // pixels at which the room fits in the field.
        let unit = (field.w / (cols * TILE))
            .min(field.h / (rows * TILE))
            .min(largest)
            .max(1);
        let (map_w, map_h) = (cols * TILE * unit, rows * TILE * unit);
        let map = Rect {
            x: field.x + (field.w - map_w) / 2,
            y: field.y + (field.h - map_h) / 2,
            w: map_w,
            h: map_h,
        };
        Self {
            view: Rect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            },
            scale,
            unit,
            cols,
            rows,
            field,
            map,
            roster,
            feed,
            metrics,
        }
    }

    /// A length in logical pixels as device pixels, at least one.
    pub fn px(&self, logical: f32) -> i32 {
        ((logical * self.scale).round() as i32).max(1)
    }

    /// The pixel of the art under a point of the view, if it is on the
    /// room.
    pub fn art_point(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        self.map.contains(x, y).then(|| {
            (
                (x - self.map.x).div_euclid(self.unit),
                (y - self.map.y).div_euclid(self.unit),
            )
        })
    }
}

/// What is painted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    /// Where the picture of the room goes.
    pub room: Rect,
    /// The rectangles under the room: the page and the frame.
    pub under: Vec<Quad>,
    /// The rectangles over it: the boxes of the chrome, back to front.
    pub quads: Vec<Quad>,
    /// The text, over the rectangles.
    pub texts: Vec<Text>,
    /// The rows of the roster, for the pointer: where, and whose.
    pub rows: Vec<(Rect, u64)>,
    /// The rows of the feed in view, for the pointer: where, and of which
    /// entry.
    pub feed_rows: Vec<(Rect, u64)>,
    /// The entry of every row of the feed, in view or not: what the
    /// keyboard walks.
    pub feed_lines: Vec<u64>,
    /// How many rows of the feed are in view at most.
    pub feed_height: usize,
    /// The feed, for the wheel.
    pub feed: Option<Rect>,
    /// The mark that goes back to the end of the feed, while it is shown.
    pub jump: Option<Rect>,
    /// The name of the lion the feed is narrowed to: a click shows all.
    pub all: Option<Rect>,
}

impl Scene {
    fn fill(&mut self, rect: Rect, color: Hsla) {
        if rect.w > 0 && rect.h > 0 {
            self.quads.push(Quad { rect, color });
        }
    }

    fn text(&mut self, x: i32, y: i32, text: impl Into<String>, color: Hsla, bold: bool) {
        self.texts.push(Text {
            x,
            y,
            text: text.into(),
            color,
            bold,
        });
    }

    /// The entry of the feed whose row is under a point.
    pub fn entry_at(&self, x: i32, y: i32) -> Option<u64> {
        self.feed_rows
            .iter()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, id)| *id)
    }

    /// The lion whose roster row is under a point.
    pub fn row_at(&self, x: i32, y: i32) -> Option<u64> {
        self.rows
            .iter()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, id)| *id)
    }
}

fn status_color(palette: &DenPalette, status: Status) -> Hsla {
    match status {
        Status::Attention => palette.signal,
        Status::Success => palette.success,
        Status::Warning => palette.warning,
        Status::Error => palette.error,
        Status::Info => palette.info,
    }
}

/// Writes the scene of a frame: everything but the picture of the room.
/// With `note`, that is pinned over the feed: the editor's advice.
pub fn build(
    den: &Den,
    frame: &Frame,
    layout: &Layout,
    palette: &DenPalette,
    note: Option<&[String]>,
) -> Scene {
    let mut scene = Scene {
        room: layout.map,
        ..Scene::default()
    };
    scene.under.push(Quad {
        rect: layout.view,
        color: palette.ground,
    });
    // The room is framed as the blueprint frames a drawing: a mark at each
    // corner, a little way out.
    let (arm, line, out) = (layout.px(10.), layout.px(1.), layout.px(5.));
    let map = layout.map;
    let (x0, y0) = (map.x - out, map.y - out);
    let (x1, y1) = (map.x + map.w + out, map.y + map.h + out);
    for (x, y, w, h) in [
        (x0, y0, arm, line),
        (x0, y0, line, arm),
        (x1 - arm, y0, arm, line),
        (x1 - line, y0, line, arm),
        (x0, y1 - line, arm, line),
        (x0, y1 - arm, line, arm),
        (x1 - arm, y1 - line, arm, line),
        (x1 - line, y1 - arm, line, arm),
    ] {
        scene.under.push(Quad {
            rect: Rect { x, y, w, h },
            color: palette.tick,
        });
    }

    if let Some(area) = layout.feed {
        feed_box(&mut scene, den, frame, area, layout, palette, note);
    }
    if let Some(area) = layout.roster {
        roster(&mut scene, &den.roster(), area, layout, palette);
    }
    let shown = den.hovered().or(den.selected());
    if let Some((card, actor)) = shown.and_then(|id| {
        let actor = frame.actors.iter().find(|actor| actor.id == id)?;
        Some((den.truth(id)?, actor))
    }) {
        truth_card(&mut scene, &card, actor, &frame.actors, layout, palette);
    }
    scene
}

/// A box of the blueprint: a fill, a 1 px rule and an L at each corner.
fn frame_box(scene: &mut Scene, rect: Rect, layout: &Layout, palette: &DenPalette) {
    let line = layout.px(1.);
    let arm = layout.px(8.);
    scene.fill(rect, palette.paper);
    let Rect { x, y, w, h } = rect;
    for (rx, ry, rw, rh) in [
        (x, y, w, line),
        (x, y + h - line, w, line),
        (x, y, line, h),
        (x + w - line, y, line, h),
    ] {
        scene.fill(
            Rect {
                x: rx,
                y: ry,
                w: rw,
                h: rh,
            },
            palette.rule,
        );
    }
    for (cx, cy) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
        let right = cx > x;
        let bottom = cy > y;
        scene.fill(
            Rect {
                x: if right { cx - arm } else { cx },
                y: if bottom { cy - line } else { cy },
                w: arm,
                h: line,
            },
            palette.tick,
        );
        scene.fill(
            Rect {
                x: if right { cx - line } else { cx },
                y: if bottom { cy - arm } else { cy },
                w: line,
                h: arm,
            },
            palette.tick,
        );
    }
}

/// What an empty feed says: of the den as a whole, or of the lion it is
/// narrowed to. The first row is the narrator's, the rest the plain fact.
pub fn empty_feed(den: &Den) -> Vec<String> {
    let Some(card) = den.selected().and_then(|id| den.truth(id)) else {
        return vec!["Nothing to tell yet.".to_owned()];
    };
    let name = crate::narrator::shout(&card.name);
    if card.mystery {
        vec![
            format!("{name} keeps its thoughts to itself."),
            String::new(),
            "No transcript is followed for this session: the feed has only what \
             its terminal shows."
                .to_owned(),
        ]
    } else {
        vec![format!("{name} has not said a word yet.")]
    }
}

/// The feed: a heading that says whose it is, the rows in view, what the
/// den says of itself at the foot, and the way back to the end.
fn feed_box(
    scene: &mut Scene,
    den: &Den,
    frame: &Frame,
    area: Rect,
    layout: &Layout,
    palette: &DenPalette,
    note: Option<&[String]>,
) {
    frame_box(scene, area, layout, palette);
    scene.feed = Some(area);
    let pad = layout.px(10.);
    let char_w = layout.metrics.char_w;
    let line_h = layout.metrics.line_h.ceil() as i32;
    let row_h = line_h + layout.px(6.);
    let chars = (((area.w - pad * 2) as f32 / char_w).floor() as usize).max(8);
    let at = |n: usize| area.x + pad + (char_w * n as f32).round() as i32;
    let rule = |scene: &mut Scene, y: i32| {
        scene.fill(
            Rect {
                x: area.x,
                y,
                w: area.w,
                h: layout.px(1.),
            },
            palette.rule,
        );
    };

    // The heading: whose feed this is.
    scene.text(at(0), area.y + pad, "THE FEED", palette.text_muted, true);
    let only = den.selected().and_then(|id| den.truth(id));
    let label = match &only {
        Some(card) => format!(
            "{} x",
            clip(
                &crate::narrator::shout(&card.name),
                chars.saturating_sub(12)
            )
        ),
        None => "ALL".to_owned(),
    };
    let label_at = at(chars.saturating_sub(label.chars().count()));
    if only.is_some() {
        scene.all = Some(Rect {
            x: label_at - layout.px(4.),
            y: area.y,
            w: area.x + area.w - label_at,
            h: pad + row_h,
        });
    }
    scene.text(
        label_at,
        area.y + pad,
        label,
        if only.is_some() {
            palette.signal
        } else {
            palette.text_muted
        },
        only.is_some(),
    );
    let mut top = area.y + pad + row_h;
    rule(scene, top - layout.px(4.));
    top += layout.px(4.);

    // The editor's note is pinned over the feed.
    if let Some(note) = note {
        for (index, text) in note.iter().enumerate() {
            for row in crate::feed::wrap_text(text, chars) {
                scene.text(
                    at(0),
                    top,
                    row,
                    if index == 0 {
                        palette.text
                    } else {
                        palette.text_muted
                    },
                    false,
                );
                top += line_h;
            }
        }
        top += layout.px(6.);
        rule(scene, top - layout.px(3.));
        top += layout.px(4.);
    }

    // The foot: what the den says of itself as a whole.
    let mut bottom = area.y + area.h - pad;
    let foot: Vec<&String> = frame
        .said
        .rows
        .iter()
        .filter(|row| !row.is_empty())
        .collect();
    if !foot.is_empty() {
        bottom -= line_h * foot.len() as i32;
        for (index, row) in foot.iter().enumerate() {
            scene.text(
                at(0),
                bottom + index as i32 * line_h,
                clip(row, chars),
                if frame.said.urgent {
                    palette.warning
                } else {
                    palette.text_muted
                },
                false,
            );
        }
        bottom -= layout.px(6.);
        rule(scene, bottom);
        bottom -= layout.px(4.);
    }

    let height = ((bottom - top) / line_h).max(0) as usize;
    let indent = 2;
    let show = crate::feed::Show {
        only: den.selected(),
        plain: den.plain_status(),
        width: chars.saturating_sub(indent).max(6),
        now: den.wall_time(),
        typing: frame.typing,
    };
    let rows = crate::feed::lay(den.feed(), &show);
    scene.feed_lines = rows.iter().map(|row| row.entry).collect();
    scene.feed_height = height;
    if rows.is_empty() {
        let mut y = top;
        for (index, text) in empty_feed(den).iter().enumerate() {
            if text.is_empty() {
                y += line_h / 2;
                continue;
            }
            for row in crate::feed::wrap_text(text, chars) {
                if y + line_h > bottom {
                    break;
                }
                scene.text(
                    at(0),
                    y,
                    row,
                    if index == 0 {
                        palette.text
                    } else {
                        palette.text_muted
                    },
                    false,
                );
                y += line_h;
            }
        }
        return;
    }

    let start = den.feed().window(rows.len(), height);
    let cursor = den.feed().cursor();
    let unit = (line_h / 9).max(1);
    for (index, row) in rows.iter().skip(start).take(height).enumerate() {
        let y = top + index as i32 * line_h;
        let rect = Rect {
            x: area.x + layout.px(1.),
            y,
            w: area.w - layout.px(2.),
            h: line_h,
        };
        if row.kind != crate::feed::RowKind::Gap {
            scene.feed_rows.push((rect, row.entry));
        }
        if cursor == Some(row.entry) && row.kind != crate::feed::RowKind::Gap {
            scene.fill(
                Rect {
                    w: layout.px(2.),
                    ..rect
                },
                palette.signal,
            );
        }
        match &row.kind {
            crate::feed::RowKind::Gap => {}
            crate::feed::RowKind::Heading {
                tint,
                name,
                time,
                speech,
            } => {
                // The swatch: the colour of the lion's mane.
                let side = unit * 6;
                let (sx, sy) = (at(0) + unit, y + (line_h - side) / 2);
                scene.fill(
                    Rect {
                        x: sx - layout.px(1.),
                        y: sy - layout.px(1.),
                        w: side + layout.px(2.),
                        h: side + layout.px(2.),
                    },
                    palette.rule,
                );
                scene.fill(
                    Rect {
                        x: sx,
                        y: sy,
                        w: side,
                        h: side,
                    },
                    tint.unwrap_or(palette.text_muted),
                );
                let room = chars.saturating_sub(indent + 6 + time.chars().count());
                let name = clip(name, room.max(4));
                let after = indent + name.chars().count() + 1;
                scene.text(at(indent), y, name, palette.text, true);
                if *speech {
                    scene.text(at(after), y, "said", palette.text_muted, false);
                }
                if !time.is_empty() {
                    scene.text(
                        at(chars.saturating_sub(time.chars().count())),
                        y,
                        time.clone(),
                        palette.text_muted,
                        false,
                    );
                }
            }
            crate::feed::RowKind::Narration { urgent } => {
                if *urgent {
                    // What is urgent carries its colour and its glyph.
                    let first = index == 0
                        || rows.get(start + index - 1).is_none_or(|before| {
                            before.entry != row.entry
                                || !matches!(before.kind, crate::feed::RowKind::Narration { .. })
                        });
                    if first {
                        let sign = &*crate::glyphs::BANG_URGENT;
                        let dot = (line_h / (sign.h + 2)).max(1);
                        let (gx, gy) = (at(0), y + (line_h - sign.h * dot) / 2);
                        for run in sign.runs() {
                            scene.fill(
                                Rect {
                                    x: gx + run.x * dot,
                                    y: gy + run.y * dot,
                                    w: run.w * dot,
                                    h: run.h * dot,
                                },
                                palette.color(run.role),
                            );
                        }
                    }
                }
                scene.text(
                    at(indent),
                    y,
                    row.text.clone(),
                    if *urgent {
                        palette.warning
                    } else {
                        palette.text
                    },
                    *urgent,
                );
            }
            crate::feed::RowKind::Speech | crate::feed::RowKind::More(_) => {
                // Speech is set apart: a raised ground and a bar at its left.
                scene.fill(
                    Rect {
                        x: at(0),
                        y,
                        w: area.w - pad * 2,
                        h: line_h,
                    },
                    palette.raised,
                );
                scene.fill(
                    Rect {
                        x: at(0),
                        y,
                        w: layout.px(2.),
                        h: line_h,
                    },
                    palette.tick,
                );
                let more = matches!(row.kind, crate::feed::RowKind::More(_));
                scene.text(
                    at(indent),
                    y,
                    row.text.clone(),
                    if more { palette.signal } else { palette.text },
                    false,
                );
            }
        }
    }

    // The way back to the end, while the reader is elsewhere.
    if !den.feed().follows() && rows.len() > height {
        let label = match den.feed().unseen() {
            0 => "v latest".to_owned(),
            n => format!("v {n} new"),
        };
        let w = (char_w * (label.chars().count() + 2) as f32).ceil() as i32;
        let rect = Rect {
            x: area.x + area.w - pad - w,
            y: bottom - line_h - layout.px(2.),
            w,
            h: line_h + layout.px(2.),
        };
        scene.fill(rect, palette.accent);
        scene.text(
            rect.x + (char_w.round() as i32),
            rect.y + layout.px(1.),
            label,
            palette.on_accent,
            true,
        );
        scene.jump = Some(rect);
    }
}

fn clip(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        text.to_owned()
    } else {
        let mut cut: String = text.chars().take(chars.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

fn roster(
    scene: &mut Scene,
    entries: &[RosterEntry],
    area: Rect,
    layout: &Layout,
    palette: &DenPalette,
) {
    frame_box(scene, area, layout, palette);
    let pad = layout.px(10.);
    let char_w = layout.metrics.char_w;
    let line_h = layout.metrics.line_h.ceil() as i32;
    let row_h = line_h + layout.px(6.);
    let at = |chars: usize| area.x + pad + (char_w * chars as f32).round() as i32;

    scene.text(at(0), area.y + pad, "THE PRIDE", palette.text_muted, true);
    let count = entries.iter().filter(|entry| !entry.little).count();
    let count = format!("{count:>3}");
    scene.text(
        at(ROSTER_CHARS - 3),
        area.y + pad,
        count,
        palette.text_muted,
        false,
    );
    let top = area.y + pad + row_h + layout.px(4.);
    scene.fill(
        Rect {
            x: area.x,
            y: top - layout.px(4.),
            w: area.w,
            h: layout.px(1.),
        },
        palette.rule,
    );

    let room = ((area.y + area.h - pad - top) / row_h).max(0) as usize;
    let shown = if entries.len() > room {
        room.saturating_sub(1)
    } else {
        entries.len()
    };
    let unit = (line_h / 9).max(1);
    for (index, entry) in entries.iter().take(shown).enumerate() {
        let y = top + index as i32 * row_h;
        let row = Rect {
            x: area.x + layout.px(1.),
            y,
            w: area.w - layout.px(2.),
            h: row_h,
        };
        scene.rows.push((row, entry.id));
        if entry.selected {
            scene.fill(row, palette.raised);
            scene.fill(
                Rect {
                    w: layout.px(2.),
                    ..row
                },
                palette.signal,
            );
        }
        // The swatch: the colour of the lion's mane.
        let indent = usize::from(entry.little);
        let side = if entry.little { unit * 5 } else { unit * 7 };
        let (sx, sy) = (at(indent) + unit, y + (row_h - side) / 2);
        scene.fill(
            Rect {
                x: sx - layout.px(1.),
                y: sy - layout.px(1.),
                w: side + layout.px(2.),
                h: side + layout.px(2.),
            },
            palette.rule,
        );
        scene.fill(
            Rect {
                x: sx,
                y: sy,
                w: side,
                h: side,
            },
            entry.tint,
        );
        let text_y = y + (row_h - line_h) / 2;
        let name_chars = 11 - indent;
        scene.text(
            at(3 + indent),
            text_y,
            clip(&entry.name, name_chars),
            palette.text,
            entry.selected,
        );
        scene.text(
            at(15),
            text_y,
            format!("Lv.{}", entry.level.min(999)),
            palette.text_muted,
            false,
        );
        let color = entry
            .status
            .map_or(palette.text_muted, |status| status_color(palette, status));
        scene.text(
            at(ROSTER_CHARS.saturating_sub(entry.tag.chars().count())),
            text_y,
            entry.tag,
            color,
            entry.status.is_some(),
        );
    }
    if shown < entries.len() {
        let y = top + shown as i32 * row_h;
        scene.text(
            at(0),
            y + (row_h - line_h) / 2,
            format!("+{} more", entries.len() - shown),
            palette.text_muted,
            false,
        );
    }
}

/// The width of the truth card, in characters.
pub const CARD_CHARS: usize = 40;
/// How many rows of detail the truth card shows at most.
pub const CARD_DETAIL_ROWS: usize = 4;

/// Breaks a text into rows of at most `chars` characters, at spaces when it
/// can and inside a word when it must, so that nothing of it is lost but
/// what is beyond `rows` rows: then the last row ends in an ellipsis.
pub fn wrap_detail(text: &str, chars: usize, rows: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if chars == 0 {
        return out;
    }
    let mut row = String::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            // A row filled to its last character has no room, not even for
            // the space before the next word.
            let room = chars.saturating_sub(used + usize::from(used > 0));
            if word.len() <= room {
                if used > 0 {
                    row.push(' ');
                    used += 1;
                }
                row.extend(word.iter());
                used += word.len();
                break;
            }
            if used == 0 || room > chars / 2 {
                // Too long for any row, or a good half of this one is
                // still free: cut the word here.
                if used > 0 {
                    row.push(' ');
                }
                row.extend(word.drain(..room));
            }
            out.push(std::mem::take(&mut row));
            used = 0;
        }
    }
    if !row.is_empty() {
        out.push(row);
    }
    if out.len() > rows {
        out.truncate(rows);
        if let Some(last) = out.last_mut() {
            *last = clip(&format!("{last}…"), chars);
            if !last.ends_with('…') {
                last.push('…');
            }
        }
    }
    out
}

fn truth_card(
    scene: &mut Scene,
    card: &TruthCard,
    actor: &Actor,
    others: &[Actor],
    layout: &Layout,
    palette: &DenPalette,
) {
    let pad = layout.px(10.);
    let char_w = layout.metrics.char_w;
    let line_h = layout.metrics.line_h.ceil() as i32;
    let limit = (((layout.field.w - pad * 4) as f32 / char_w) as usize).clamp(16, CARD_CHARS);

    let title = format!(
        "{}  Lv.{}",
        clip(&card.name, limit.saturating_sub(9)),
        card.level
    );
    let mut lines: Vec<(String, Hsla, bool)> = vec![(title, palette.text, true)];
    let state_color = card
        .status
        .map_or(palette.text_muted, |status| status_color(palette, status));
    lines.push((clip(card.label, limit), state_color, false));
    if let Some(parent) = &card.parent {
        lines.push((
            clip(&format!("Sub-agent of {parent}"), limit),
            palette.text_muted,
            false,
        ));
    }
    if let Some(detail) = &card.detail {
        for row in wrap_detail(detail, limit, CARD_DETAIL_ROWS) {
            lines.push((row, palette.text, false));
        }
    }
    if card.mystery {
        lines.push((
            clip("No transcript: only working or quiet.", limit),
            palette.text_muted,
            false,
        ));
    }

    let widest = lines
        .iter()
        .map(|(text, _, _)| text.chars().count())
        .max()
        .unwrap_or(0);
    let w = (char_w * widest as f32).ceil() as i32 + pad * 2;
    let h = line_h * lines.len() as i32 + pad * 2;
    let unit = layout.unit;
    let (left, top, cub_w, cub_h) = actor.bounds();
    let cub = Rect {
        x: layout.map.x + left * unit,
        y: layout.map.y + top * unit,
        w: cub_w * unit,
        h: cub_h * unit,
    };
    // Where it covers the least: over the lion, under it, or at a side,
    // never on the lion itself, and on as little of the other lions, their
    // bubbles and their names as can be.
    let margin = layout.px(6.);
    let around = |actor: &Actor, bubble: i32, name: i32| {
        let (left, top, w, h) = actor.bounds();
        Rect {
            x: layout.map.x + (left - 8) * unit,
            y: layout.map.y + (top - bubble) * unit,
            w: (w + 16) * unit,
            h: (h + bubble + name) * unit,
        }
    };
    let own = around(actor, 13, 10);
    let others: Vec<Rect> = others
        .iter()
        .filter(|other| other.id != actor.id)
        .map(|other| around(other, 11, 10))
        .collect();
    let overlap = |a: Rect, b: Rect| {
        let wide = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
        let high = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
        i64::from(wide.max(0)) * i64::from(high.max(0))
    };
    let (min_x, max_x) = (
        layout.field.x + margin,
        (layout.field.x + layout.field.w - w - margin).max(layout.field.x + margin),
    );
    let (min_y, max_y) = (margin, (layout.view.h - h - margin).max(margin));
    let centre_x = cub.x + cub.w / 2 - w / 2;
    let centre_y = cub.y + cub.h / 2 - h / 2;
    let (x, y) = [
        (centre_x, own.y - h),
        (centre_x, own.y + own.h),
        (own.x + own.w, centre_y),
        (own.x - w, centre_y),
        (own.x + own.w, own.y - h),
        (own.x - w, own.y - h),
    ]
    .into_iter()
    .map(|(x, y)| (x.clamp(min_x, max_x), y.clamp(min_y, max_y)))
    .min_by_key(|(x, y)| {
        let card = Rect { x: *x, y: *y, w, h };
        // Its own lion counts for much more than anybody else.
        overlap(card, own) * 16
            + others
                .iter()
                .map(|other| overlap(card, *other))
                .sum::<i64>()
    })
    .unwrap_or((centre_x, own.y - h));
    let rect = Rect { x, y, w, h };
    frame_box(scene, rect, layout, palette);
    for (index, (text, color, bold)) in lines.into_iter().enumerate() {
        scene.text(x + pad, y + pad + index as i32 * line_h, text, color, bold);
    }
}

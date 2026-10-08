//! A picture in memory: straight RGBA, eight bits a channel.
//!
//! The Den is composited in software at the size of its art, one byte
//! buffer for the whole room, and only then scaled up by a whole number
//! ([`Bitmap::scaled`]), so that a pixel of the art is a crisp square on any
//! display. Everything here is plain arithmetic on bytes: no GPUI, no clock.

/// A colour as red, green, blue and alpha.
pub type Rgba = [u8; 4];

/// No colour at all.
pub const CLEAR: Rgba = [0, 0, 0, 0];

/// A rectangle of pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    /// Width in pixels.
    pub w: i32,
    /// Height in pixels.
    pub h: i32,
    data: Vec<u8>,
}

impl Bitmap {
    /// A transparent picture.
    pub fn new(w: i32, h: i32) -> Self {
        let (w, h) = (w.max(0), h.max(0));
        Self {
            w,
            h,
            data: vec![0; (w * h * 4) as usize],
        }
    }

    /// A picture from its bytes, four a pixel, row by row.
    pub fn from_rgba(w: i32, h: i32, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), (w * h * 4) as usize, "a bitmap is w*h*4 bytes");
        Self { w, h, data }
    }

    /// Reads a PNG file's bytes. The files are compiled into the binary, so
    /// one that does not decode is a bug of the build, and panics.
    pub fn from_png(bytes: &[u8]) -> Self {
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .expect("a bundled PNG decodes")
            .to_rgba8();
        let (w, h) = image.dimensions();
        Self::from_rgba(w as i32, h as i32, image.into_raw())
    }

    /// The picture as the bytes of a PNG file.
    pub fn to_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut out),
            &self.data,
            self.w as u32,
            self.h as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .expect("a bitmap encodes as PNG");
        out
    }

    /// The bytes, four a pixel, row by row.
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    /// The bytes, to hand to something that wants to own them.
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }

    /// The pixel at a place; clear outside the picture.
    pub fn get(&self, x: i32, y: i32) -> Rgba {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return CLEAR;
        }
        let at = ((y * self.w + x) * 4) as usize;
        [
            self.data[at],
            self.data[at + 1],
            self.data[at + 2],
            self.data[at + 3],
        ]
    }

    /// Sets a pixel, replacing what was there. Outside nothing happens.
    pub fn set(&mut self, x: i32, y: i32, color: Rgba) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let at = ((y * self.w + x) * 4) as usize;
        self.data[at..at + 4].copy_from_slice(&color);
    }

    /// Paints a pixel over what is there, by its alpha.
    pub fn blend(&mut self, x: i32, y: i32, color: Rgba) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || color[3] == 0 {
            return;
        }
        let at = ((y * self.w + x) * 4) as usize;
        if color[3] == 255 {
            self.data[at..at + 4].copy_from_slice(&color);
            return;
        }
        let alpha = u32::from(color[3]);
        let under = u32::from(self.data[at + 3]);
        for (channel, top) in color.iter().take(3).enumerate() {
            let (top, bottom) = (u32::from(*top), u32::from(self.data[at + channel]));
            // Over a clear pixel the colour is the top one, undimmed.
            let bottom = if under == 0 { top } else { bottom };
            self.data[at + channel] = ((top * alpha + bottom * (255 - alpha) + 127) / 255) as u8;
        }
        self.data[at + 3] = (alpha + under * (255 - alpha) / 255).min(255) as u8;
    }

    /// Fills a rectangle, over what is there.
    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgba) {
        for row in y.max(0)..(y + h).min(self.h) {
            for col in x.max(0)..(x + w).min(self.w) {
                self.blend(col, row, color);
            }
        }
    }

    /// Paints another picture over this one, its top left at a place.
    pub fn draw(&mut self, other: &Bitmap, x: i32, y: i32) {
        self.draw_part(other, 0, 0, other.w, other.h, x, y, false);
    }

    /// Paints a part of another picture over this one, mirrored left to
    /// right if asked.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_part(
        &mut self,
        other: &Bitmap,
        sx: i32,
        sy: i32,
        w: i32,
        h: i32,
        x: i32,
        y: i32,
        mirrored: bool,
    ) {
        for row in 0..h {
            let ty = y + row;
            if ty < 0 || ty >= self.h {
                continue;
            }
            for col in 0..w {
                let from = if mirrored { w - 1 - col } else { col };
                self.blend(x + col, ty, other.get(sx + from, sy + row));
            }
        }
    }

    /// A part of the picture, as a picture of its own.
    pub fn part(&self, x: i32, y: i32, w: i32, h: i32) -> Bitmap {
        let mut out = Bitmap::new(w, h);
        for row in 0..h {
            for col in 0..w {
                out.set(col, row, self.get(x + col, y + row));
            }
        }
        out
    }

    /// The picture mirrored left to right.
    pub fn mirrored(&self) -> Bitmap {
        let mut out = Bitmap::new(self.w, self.h);
        for y in 0..self.h {
            for x in 0..self.w {
                out.set(self.w - 1 - x, y, self.get(x, y));
            }
        }
        out
    }

    /// The picture turned a quarter clockwise: what stood lies down, its
    /// head to the right.
    pub fn turned(&self) -> Bitmap {
        let mut out = Bitmap::new(self.h, self.w);
        for y in 0..self.h {
            for x in 0..self.w {
                out.set(self.h - 1 - y, x, self.get(x, y));
            }
        }
        out
    }

    /// The picture with every pixel run through a function of its colour.
    pub fn mapped(&self, map: impl Fn(Rgba) -> Rgba) -> Bitmap {
        let mut out = self.clone();
        for pixel in out.data.chunks_exact_mut(4) {
            let color = map([pixel[0], pixel[1], pixel[2], pixel[3]]);
            pixel.copy_from_slice(&color);
        }
        out
    }

    /// The picture with every pixel `factor` pixels wide and high, no
    /// smoothing: pixel art, larger.
    pub fn scaled(&self, factor: i32) -> Bitmap {
        let factor = factor.max(1);
        let (w, h) = (self.w * factor, self.h * factor);
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        let mut line = Vec::with_capacity((w * 4) as usize);
        for y in 0..self.h {
            line.clear();
            let row = &self.data[(y * self.w * 4) as usize..((y + 1) * self.w * 4) as usize];
            for pixel in row.chunks_exact(4) {
                for _ in 0..factor {
                    line.extend_from_slice(pixel);
                }
            }
            for _ in 0..factor {
                data.extend_from_slice(&line);
            }
        }
        Bitmap { w, h, data }
    }

    /// How many pixels are not clear.
    pub fn area(&self) -> usize {
        self.data
            .chunks_exact(4)
            .filter(|pixel| pixel[3] > 0)
            .count()
    }

    /// The first row that has a pixel, if any has.
    pub fn top(&self) -> Option<i32> {
        (0..self.h).find(|y| (0..self.w).any(|x| self.get(x, *y)[3] > 0))
    }
}

/// A colour from hue (degrees), saturation and lightness (0 to 1).
pub fn hsl(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1. - (2. * l - 1.).abs()) * s;
    let hp = h.rem_euclid(360.) / 60.;
    let x = c * (1. - (hp % 2. - 1.).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    let m = l - c / 2.;
    let byte = |v: f32| ((v + m) * 255.).round().clamp(0., 255.) as u8;
    [byte(r), byte(g), byte(b)]
}

/// How a grey tile is given a colour: a hue and a saturation for all of it,
/// and what is done to its lightness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    /// Hue, in degrees.
    pub h: f32,
    /// Saturation, 0 to 100.
    pub s: f32,
    /// Brightness, -100 to 100: added to the lightness, halved.
    pub b: f32,
    /// Contrast, -100 to 100: stretches the lightness around the middle.
    pub c: f32,
}

impl Tone {
    /// The colour of a grey of this lightness (0 to 1) in this tone.
    pub fn of(self, lightness: f32) -> [u8; 3] {
        let l = 0.5 + (lightness - 0.5) * ((100. + self.c) / 100.) + self.b / 200.;
        hsl(self.h, self.s / 100., l.clamp(0., 1.))
    }

    /// A grey picture in this tone: each pixel keeps its alpha and takes the
    /// tone's colour at its own lightness.
    pub fn colorize(self, tile: &Bitmap) -> Bitmap {
        tile.mapped(|[r, g, b, a]| {
            if a == 0 {
                return CLEAR;
            }
            let lightness =
                (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)) / 255.;
            let [r, g, b] = self.of(lightness);
            [r, g, b, a]
        })
    }
}

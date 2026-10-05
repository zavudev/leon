//! Resize maths: from pixels and cell metrics to a grid of columns and rows.

use portable_pty::PtySize;

/// The fewest columns a terminal is ever resized to: programs misbehave on a
/// grid narrower than this.
pub const MIN_COLS: u16 = 2;
/// The fewest rows a terminal is ever resized to.
pub const MIN_ROWS: u16 = 1;

/// The size of a terminal's grid and of one of its cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// Width of a cell in pixels (reported to programs that ask).
    pub cell_width: u16,
    /// Height of a cell in pixels.
    pub cell_height: u16,
}

impl GridSize {
    /// A grid of `cols` by `rows` with no pixel metrics (before the first
    /// paint, or in tests).
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.max(MIN_COLS),
            rows: rows.max(MIN_ROWS),
            cell_width: 0,
            cell_height: 0,
        }
    }

    /// The grid that fits in a `width` by `height` area (pixels) of cells
    /// `cell_width` by `cell_height`. `None` when a cell has no size, which
    /// is what a font that is not measured yet looks like.
    pub fn fit(width: f32, height: f32, cell_width: f32, cell_height: f32) -> Option<Self> {
        if !(cell_width > 0.0 && cell_height > 0.0) || !width.is_finite() || !height.is_finite() {
            return None;
        }
        let cols = (width.max(0.0) / cell_width)
            .floor()
            .min(f32::from(u16::MAX)) as u16;
        let rows = (height.max(0.0) / cell_height)
            .floor()
            .min(f32::from(u16::MAX)) as u16;
        Some(Self {
            cols: cols.max(MIN_COLS),
            rows: rows.max(MIN_ROWS),
            cell_width: cell_width.round().clamp(0.0, f32::from(u16::MAX)) as u16,
            cell_height: cell_height.round().clamp(0.0, f32::from(u16::MAX)) as u16,
        })
    }

    /// The same size as the PTY library states it.
    pub fn pty_size(&self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: self.cols.saturating_mul(self.cell_width),
            pixel_height: self.rows.saturating_mul(self.cell_height),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grid_fits_whole_cells_only() {
        let size = GridSize::fit(805.0, 479.0, 8.0, 16.0).unwrap();
        assert_eq!((size.cols, size.rows), (100, 29));
    }

    #[test]
    fn a_tiny_area_still_has_the_minimum_grid() {
        let size = GridSize::fit(3.0, 2.0, 8.0, 16.0).unwrap();
        assert_eq!((size.cols, size.rows), (MIN_COLS, MIN_ROWS));
    }

    #[test]
    fn an_unmeasured_font_gives_no_grid() {
        assert!(GridSize::fit(800.0, 600.0, 0.0, 16.0).is_none());
        assert!(GridSize::fit(800.0, 600.0, 8.0, -1.0).is_none());
        assert!(GridSize::fit(f32::NAN, 600.0, 8.0, 16.0).is_none());
    }

    #[test]
    fn a_huge_area_does_not_overflow() {
        let size = GridSize::fit(1e9, 1e9, 1.0, 1.0).unwrap();
        assert_eq!((size.cols, size.rows), (u16::MAX, u16::MAX));
    }

    #[test]
    fn the_pty_is_told_the_pixel_size_too() {
        let size = GridSize::fit(800.0, 480.0, 8.0, 16.0).unwrap();
        let pty = size.pty_size();
        assert_eq!((pty.cols, pty.rows), (100, 30));
        assert_eq!((pty.pixel_width, pty.pixel_height), (800, 480));
    }

    #[test]
    fn a_new_grid_is_clamped_to_the_minimum() {
        assert_eq!(GridSize::new(0, 0).cols, MIN_COLS);
        assert_eq!(GridSize::new(0, 0).rows, MIN_ROWS);
    }
}

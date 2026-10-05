//! The layout of the terminals of a workspace, as pure data.
//!
//! A [`Layout`] is a binary tree: a [`Leaf`](Layout::Leaf) is one terminal, a
//! [`Split`](Layout::Split) divides its area between two layouts along an
//! [`Axis`] at a `ratio`. Everything the keyboard and the mouse can do to the
//! panes is a method here, with no window and no process in sight:
//!
//! * splitting a leaf in two, and closing one so that its sibling takes the
//!   whole area;
//! * moving the focus to the neighbour in a direction, or to the next one in
//!   reading order;
//! * moving a divider in fixed steps, never below the minimum size of a pane
//!   (counted in cells);
//! * making every pane the same size;
//! * a [`Tab`], which also knows its focused pane and whether that pane is
//!   maximised.
//!
//! Areas are measured in cells, as [`Rect`]s: the window turns pixels into
//! cells with the terminal's cell size and back.

use super::live::LiveId;

/// Which way a split divides its area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Side by side: the first layout on the left, the second on the right.
    Row,
    /// One above the other: the first on top, the second below.
    Column,
}

/// A direction on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Left.
    Left,
    /// Right.
    Right,
    /// Up.
    Up,
    /// Down.
    Down,
}

impl Dir {
    /// The axis a divider has to lie across to be moved this way.
    pub fn axis(self) -> Axis {
        match self {
            Dir::Left | Dir::Right => Axis::Row,
            Dir::Up | Dir::Down => Axis::Column,
        }
    }

    /// `+1` towards the right or the bottom, `-1` towards the left or the top.
    fn sign(self) -> f32 {
        match self {
            Dir::Right | Dir::Down => 1.0,
            Dir::Left | Dir::Up => -1.0,
        }
    }
}

/// A rectangle in cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }

    fn length(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Row => self.w,
            Axis::Column => self.h,
        }
    }

    /// The two halves of this rectangle at `ratio` along `axis`.
    fn halves(&self, axis: Axis, ratio: f32) -> (Rect, Rect) {
        match axis {
            Axis::Row => {
                let first = (self.w * ratio).round().clamp(0.0, self.w);
                (
                    Rect::new(self.x, self.y, first, self.h),
                    Rect::new(self.x + first, self.y, self.w - first, self.h),
                )
            }
            Axis::Column => {
                let first = (self.h * ratio).round().clamp(0.0, self.h);
                (
                    Rect::new(self.x, self.y, self.w, first),
                    Rect::new(self.x, self.y + first, self.w, self.h - first),
                )
            }
        }
    }
}

/// The fewest columns and rows a pane may be dragged or stepped down to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinSize {
    /// Columns.
    pub cols: f32,
    /// Rows.
    pub rows: f32,
}

impl MinSize {
    fn along(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Row => self.cols,
            Axis::Column => self.rows,
        }
    }
}

/// The step a resize key moves a divider by, in cells.
pub const RESIZE_STEP: f32 = 2.0;

/// The path from the root of a layout to one of its splits: `false` is the
/// first child, `true` the second.
pub type Path = Vec<bool>;

/// A tree of panes.
#[derive(Clone, Debug, PartialEq)]
pub enum Layout {
    /// One terminal.
    Leaf(LiveId),
    /// Two layouts sharing an area.
    Split {
        /// Which way the area is divided.
        axis: Axis,
        /// The share of the first layout, between 0 and 1.
        ratio: f32,
        /// The left or top layout.
        a: Box<Layout>,
        /// The right or bottom layout.
        b: Box<Layout>,
    },
}

impl Layout {
    /// The terminals, in reading order: left to right, top to bottom.
    pub fn leaves(&self) -> Vec<LiveId> {
        match self {
            Layout::Leaf(id) => vec![*id],
            Layout::Split { a, b, .. } => {
                let mut all = a.leaves();
                all.extend(b.leaves());
                all
            }
        }
    }

    /// Whether the layout holds this terminal.
    pub fn contains(&self, id: LiveId) -> bool {
        match self {
            Layout::Leaf(leaf) => *leaf == id,
            Layout::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Splits the pane of `target` along `axis`: it keeps the first half and
    /// `new` gets the second. `false` when `target` is not in the layout.
    pub fn split(&mut self, target: LiveId, axis: Axis, new: LiveId) -> bool {
        match self {
            Layout::Leaf(leaf) if *leaf == target => {
                *self = Layout::Split {
                    axis,
                    ratio: 0.5,
                    a: Box::new(Layout::Leaf(target)),
                    b: Box::new(Layout::Leaf(new)),
                };
                true
            }
            Layout::Leaf(_) => false,
            Layout::Split { a, b, .. } => a.split(target, axis, new) || b.split(target, axis, new),
        }
    }

    /// The layout without the pane of `target`: the sibling of the closed pane
    /// takes its place. `None` when that was the last pane.
    pub fn without(self, target: LiveId) -> Option<Layout> {
        match self {
            Layout::Leaf(leaf) if leaf == target => None,
            Layout::Leaf(_) => Some(self),
            Layout::Split { axis, ratio, a, b } => match (a.without(target), b.without(target)) {
                (Some(a), Some(b)) => Some(Layout::Split {
                    axis,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
        }
    }

    /// Every pane with its area inside `bounds`.
    pub fn rects(&self, bounds: Rect) -> Vec<(LiveId, Rect)> {
        match self {
            Layout::Leaf(id) => vec![(*id, bounds)],
            Layout::Split { axis, ratio, a, b } => {
                let (first, second) = bounds.halves(*axis, *ratio);
                let mut all = a.rects(first);
                all.extend(b.rects(second));
                all
            }
        }
    }

    /// The pane next to `from` in direction `dir`: of those that touch its
    /// edge on that side, the one that shares the most of it (the first
    /// in reading order on a tie).
    pub fn neighbour(&self, from: LiveId, dir: Dir, bounds: Rect) -> Option<LiveId> {
        let rects = self.rects(bounds);
        let (_, at) = *rects.iter().find(|(id, _)| *id == from)?;
        let overlap =
            |lo1: f32, hi1: f32, lo2: f32, hi2: f32| (hi1.min(hi2) - lo1.max(lo2)).max(0.0);
        let mut best: Option<(LiveId, f32)> = None;
        for (id, other) in &rects {
            if *id == from {
                continue;
            }
            let touching = match dir {
                Dir::Left => (other.right() - at.x).abs() < 0.5,
                Dir::Right => (other.x - at.right()).abs() < 0.5,
                Dir::Up => (other.bottom() - at.y).abs() < 0.5,
                Dir::Down => (other.y - at.bottom()).abs() < 0.5,
            };
            if !touching {
                continue;
            }
            let shared = match dir.axis() {
                Axis::Row => overlap(at.y, at.bottom(), other.y, other.bottom()),
                Axis::Column => overlap(at.x, at.right(), other.x, other.right()),
            };
            if shared > 0.0 && best.is_none_or(|(_, most)| shared > most) {
                best = Some((*id, shared));
            }
        }
        best.map(|(id, _)| id)
    }

    /// The pane `delta` places from `from` in reading order, wrapping round.
    pub fn step(&self, from: LiveId, delta: isize) -> Option<LiveId> {
        let leaves = self.leaves();
        let at = leaves.iter().position(|id| *id == from)? as isize;
        let len = leaves.len() as isize;
        Some(leaves[(at + delta).rem_euclid(len) as usize])
    }

    /// Moves the nearest divider of `target` that lies across `dir`'s axis by
    /// `cells` towards `dir`, never leaving either side below `min`. `false`
    /// when there is no such divider or it could not move.
    pub fn resize(
        &mut self,
        target: LiveId,
        dir: Dir,
        cells: f32,
        bounds: Rect,
        min: MinSize,
    ) -> bool {
        let Some(path) = self.divider_path(target, dir.axis()) else {
            return false;
        };
        let Some((area, current)) = self.split_at(&path, bounds) else {
            return false;
        };
        let length = area.length(dir.axis());
        if length <= 0.0 {
            return false;
        }
        let wanted = current + dir.sign() * cells / length;
        self.set_ratio(&path, wanted, bounds, min)
    }

    /// The split whose divider `target`'s pane has on its `axis`: the nearest
    /// ancestor split along that axis.
    fn divider_path(&self, target: LiveId, axis: Axis) -> Option<Path> {
        fn walk(
            layout: &Layout,
            target: LiveId,
            axis: Axis,
            path: &mut Path,
            found: &mut Option<Path>,
        ) -> bool {
            match layout {
                Layout::Leaf(id) => *id == target,
                Layout::Split {
                    axis: own, a, b, ..
                } => {
                    path.push(false);
                    let in_a = walk(a, target, axis, path, found);
                    path.pop();
                    path.push(true);
                    let in_b = !in_a && walk(b, target, axis, path, found);
                    path.pop();
                    if (in_a || in_b) && *own == axis && found.is_none() {
                        *found = Some(path.clone());
                    }
                    in_a || in_b
                }
            }
        }
        let mut found = None;
        walk(self, target, axis, &mut Vec::new(), &mut found);
        found
    }

    /// The area and ratio of the split at `path`.
    fn split_at(&self, path: &[bool], bounds: Rect) -> Option<(Rect, f32)> {
        match (self, path) {
            (Layout::Split { ratio, .. }, []) => Some((bounds, *ratio)),
            (Layout::Split { axis, ratio, a, b }, [second, rest @ ..]) => {
                let (first_area, second_area) = bounds.halves(*axis, *ratio);
                if *second {
                    b.split_at(rest, second_area)
                } else {
                    a.split_at(rest, first_area)
                }
            }
            _ => None,
        }
    }

    /// Sets the ratio of the split at `path` to `wanted`, clamped so that both
    /// sides keep at least `min`. `true` when the ratio changed.
    pub fn set_ratio(&mut self, path: &[bool], wanted: f32, bounds: Rect, min: MinSize) -> bool {
        let Some((area, _)) = self.split_at(path, bounds) else {
            return false;
        };
        let Some(Layout::Split { axis, .. }) = self.node(path) else {
            return false;
        };
        let axis = *axis;
        let length = area.length(axis);
        let least = min.along(axis);
        // Too small to give both sides their minimum: split it evenly.
        let (low, high) = if length >= 2.0 * least {
            (least / length, 1.0 - least / length)
        } else {
            (0.5, 0.5)
        };
        let clamped = wanted.clamp(low, high);
        match self.node_mut(path) {
            Some(Layout::Split { ratio, .. }) if (*ratio - clamped).abs() > f32::EPSILON => {
                *ratio = clamped;
                true
            }
            _ => false,
        }
    }

    fn node(&self, path: &[bool]) -> Option<&Layout> {
        match (self, path) {
            (_, []) => Some(self),
            (Layout::Split { a, b, .. }, [second, rest @ ..]) => {
                if *second {
                    b.node(rest)
                } else {
                    a.node(rest)
                }
            }
            _ => None,
        }
    }

    fn node_mut(&mut self, path: &[bool]) -> Option<&mut Layout> {
        match (self, path) {
            (this, []) => Some(this),
            (Layout::Split { a, b, .. }, [second, rest @ ..]) => {
                if *second {
                    b.node_mut(rest)
                } else {
                    a.node_mut(rest)
                }
            }
            _ => None,
        }
    }

    /// Gives every pane the same share: each split divides its area in
    /// proportion to the number of panes on each side.
    pub fn equalize(&mut self) {
        if let Layout::Split { ratio, a, b, .. } = self {
            a.equalize();
            b.equalize();
            let (first, second) = (a.leaves().len() as f32, b.leaves().len() as f32);
            *ratio = first / (first + second);
        }
    }

    /// The ratio a pointer at `at` (in cells, along the split's axis from the
    /// split's own edge) asks for, over `length` cells.
    pub fn ratio_from_pointer(at: f32, length: f32) -> f32 {
        if length <= 0.0 {
            0.5
        } else {
            (at / length).clamp(0.0, 1.0)
        }
    }
}

/// One tab of a workspace: a layout, the pane that has the keyboard, and
/// whether that pane is maximised.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    /// The panes.
    pub layout: Layout,
    /// The pane that has the keyboard.
    pub focus: LiveId,
    /// The focused pane fills the tab and the others are hidden (and keep
    /// running).
    pub zoomed: bool,
}

impl Tab {
    /// A tab with one pane.
    pub fn new(id: LiveId) -> Self {
        Self {
            layout: Layout::Leaf(id),
            focus: id,
            zoomed: false,
        }
    }

    /// Splits the focused pane; the new pane gets the focus. A maximised tab
    /// is restored first.
    pub fn split(&mut self, axis: Axis, new: LiveId) {
        if self.layout.split(self.focus, axis, new) {
            self.focus = new;
            self.zoomed = false;
        }
    }

    /// Closes a pane. The focus goes to the pane before it in reading order
    /// (the one after it for the first), and a maximised tab with a single pane
    /// left is no longer maximised. `false` when it was the last one: the tab
    /// is then to be dropped.
    pub fn close(&mut self, id: LiveId) -> bool {
        let leaves = self.layout.leaves();
        let Some(at) = leaves.iter().position(|leaf| *leaf == id) else {
            return true;
        };
        let successor = if at > 0 {
            leaves[at - 1]
        } else {
            leaves.get(1).copied().unwrap_or(id)
        };
        match self.layout.clone().without(id) {
            Some(rest) => {
                self.layout = rest;
                if self.focus == id {
                    self.focus = successor;
                }
                if self.layout.leaves().len() == 1 {
                    self.zoomed = false;
                }
                true
            }
            None => false,
        }
    }

    /// Maximises the focused pane, or restores the layout. A tab with one
    /// pane has nothing to maximise.
    pub fn toggle_zoom(&mut self) {
        if self.zoomed {
            self.zoomed = false;
        } else if self.layout.leaves().len() > 1 {
            self.zoomed = true;
        }
    }

    /// What is drawn: every pane with its area, or only the focused one over
    /// the whole of `bounds` when it is maximised.
    #[cfg(test)]
    pub fn visible(&self, bounds: Rect) -> Vec<(LiveId, Rect)> {
        if self.zoomed {
            vec![(self.focus, bounds)]
        } else {
            self.layout.rects(bounds)
        }
    }

    /// Moves the focus to the neighbour in `dir`. A maximised tab is restored
    /// first, since the neighbour is not on screen.
    pub fn focus_dir(&mut self, dir: Dir, bounds: Rect) -> bool {
        match self.layout.neighbour(self.focus, dir, bounds) {
            Some(next) => {
                self.focus = next;
                self.zoomed = false;
                true
            }
            None => false,
        }
    }

    /// Moves the focus to the pane `delta` places on, wrapping round.
    pub fn focus_step(&mut self, delta: isize) -> bool {
        match self.layout.step(self.focus, delta) {
            Some(next) if next != self.focus => {
                self.focus = next;
                self.zoomed = false;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: LiveId = LiveId(1);
    const B: LiveId = LiveId(2);
    const C: LiveId = LiveId(3);
    const D: LiveId = LiveId(4);
    const SCREEN: Rect = Rect::new(0.0, 0.0, 100.0, 40.0);
    const MIN: MinSize = MinSize {
        cols: 10.0,
        rows: 4.0,
    };

    fn row(a: Layout, b: Layout) -> Layout {
        Layout::Split {
            axis: Axis::Row,
            ratio: 0.5,
            a: Box::new(a),
            b: Box::new(b),
        }
    }
    fn column(a: Layout, b: Layout) -> Layout {
        Layout::Split {
            axis: Axis::Column,
            ratio: 0.5,
            a: Box::new(a),
            b: Box::new(b),
        }
    }
    fn leaf(id: LiveId) -> Layout {
        Layout::Leaf(id)
    }

    #[test]
    fn splitting_a_leaf_puts_the_new_pane_second_at_half() {
        let mut layout = leaf(A);
        assert!(layout.split(A, Axis::Row, B));
        assert_eq!(layout, row(leaf(A), leaf(B)));
        assert!(layout.split(B, Axis::Column, C));
        assert_eq!(layout, row(leaf(A), column(leaf(B), leaf(C))));
        assert_eq!(layout.leaves(), [A, B, C]);
    }

    #[test]
    fn splitting_a_pane_that_is_not_there_changes_nothing() {
        let mut layout = row(leaf(A), leaf(B));
        assert!(!layout.split(C, Axis::Row, D));
        assert_eq!(layout, row(leaf(A), leaf(B)));
    }

    #[test]
    fn closing_a_pane_gives_its_area_to_its_sibling() {
        let layout = row(leaf(A), column(leaf(B), leaf(C)));
        assert_eq!(layout.clone().without(B), Some(row(leaf(A), leaf(C))));
        assert_eq!(layout.clone().without(A), Some(column(leaf(B), leaf(C))));
        assert_eq!(layout.without(D).unwrap().leaves(), [A, B, C]);
    }

    #[test]
    fn closing_the_last_pane_leaves_no_layout() {
        assert_eq!(leaf(A).without(A), None);
    }

    #[test]
    fn panes_share_the_area_by_their_ratios() {
        let layout = row(leaf(A), column(leaf(B), leaf(C)));
        let rects = layout.rects(SCREEN);
        assert_eq!(rects[0], (A, Rect::new(0.0, 0.0, 50.0, 40.0)));
        assert_eq!(rects[1], (B, Rect::new(50.0, 0.0, 50.0, 20.0)));
        assert_eq!(rects[2], (C, Rect::new(50.0, 20.0, 50.0, 20.0)));
    }

    #[test]
    fn the_neighbour_is_the_pane_that_touches_the_edge_and_shares_most_of_it() {
        // A is the left half; B above C on the right.
        let layout = row(leaf(A), column(leaf(B), leaf(C)));
        assert_eq!(
            layout.neighbour(A, Dir::Right, SCREEN),
            Some(B),
            "a tie goes to the first"
        );
        assert_eq!(layout.neighbour(B, Dir::Left, SCREEN), Some(A));
        assert_eq!(layout.neighbour(C, Dir::Left, SCREEN), Some(A));
        assert_eq!(layout.neighbour(B, Dir::Down, SCREEN), Some(C));
        assert_eq!(layout.neighbour(C, Dir::Up, SCREEN), Some(B));
        assert_eq!(layout.neighbour(A, Dir::Left, SCREEN), None);
        assert_eq!(layout.neighbour(A, Dir::Up, SCREEN), None);
        assert_eq!(layout.neighbour(C, Dir::Right, SCREEN), None);
    }

    #[test]
    fn a_neighbour_with_a_bigger_shared_edge_wins() {
        // B is 30 rows tall and C 10 on the right of A: from A, B is nearer.
        let mut layout = row(leaf(A), column(leaf(B), leaf(C)));
        layout.set_ratio(&[true], 0.75, SCREEN, MIN);
        assert_eq!(layout.neighbour(A, Dir::Right, SCREEN), Some(B));
        // From a pane in the lower right, left is A.
        assert_eq!(layout.neighbour(C, Dir::Left, SCREEN), Some(A));
    }

    #[test]
    fn panes_that_only_touch_at_a_corner_are_not_neighbours() {
        let layout = column(row(leaf(A), leaf(B)), row(leaf(C), leaf(D)));
        assert_eq!(layout.neighbour(A, Dir::Right, SCREEN), Some(B));
        assert_eq!(layout.neighbour(A, Dir::Down, SCREEN), Some(C));
        assert_eq!(layout.neighbour(B, Dir::Down, SCREEN), Some(D));
        assert_ne!(layout.neighbour(A, Dir::Down, SCREEN), Some(D));
    }

    #[test]
    fn stepping_goes_in_reading_order_and_wraps() {
        let layout = row(leaf(A), column(leaf(B), leaf(C)));
        assert_eq!(layout.step(A, 1), Some(B));
        assert_eq!(layout.step(C, 1), Some(A));
        assert_eq!(layout.step(A, -1), Some(C));
        assert_eq!(layout.step(D, 1), None);
    }

    #[test]
    fn a_divider_moves_in_fixed_steps_towards_the_direction() {
        let mut layout = row(leaf(A), leaf(B));
        // Focused A: its right divider; pressing right moves it right.
        assert!(layout.resize(A, Dir::Right, RESIZE_STEP, SCREEN, MIN));
        let (_, a) = layout.rects(SCREEN)[0];
        assert_eq!(a.w, 52.0);
        // From the other side the same divider moves left.
        assert!(layout.resize(B, Dir::Left, RESIZE_STEP, SCREEN, MIN));
        assert_eq!(layout.rects(SCREEN)[0].1.w, 50.0);
    }

    #[test]
    fn resizing_uses_the_divider_across_the_asked_axis() {
        let mut layout = row(leaf(A), column(leaf(B), leaf(C)));
        // B's vertical position: the divider between B and C.
        assert!(layout.resize(B, Dir::Down, RESIZE_STEP, SCREEN, MIN));
        let rects = layout.rects(SCREEN);
        assert_eq!(rects[1].1.h, 22.0);
        assert_eq!(rects[0].1.w, 50.0, "the vertical divider did not move");
        // A has no divider across the vertical axis.
        assert!(!layout.resize(A, Dir::Down, RESIZE_STEP, SCREEN, MIN));
    }

    #[test]
    fn a_divider_stops_at_the_minimum_pane_size() {
        let mut layout = row(leaf(A), leaf(B));
        for _ in 0..100 {
            layout.resize(A, Dir::Right, RESIZE_STEP, SCREEN, MIN);
        }
        let rects = layout.rects(SCREEN);
        assert_eq!(rects[1].1.w, 10.0, "B keeps its minimum");
        assert!(
            !layout.resize(A, Dir::Right, RESIZE_STEP, SCREEN, MIN),
            "no further"
        );
        for _ in 0..100 {
            layout.resize(A, Dir::Left, RESIZE_STEP, SCREEN, MIN);
        }
        assert_eq!(layout.rects(SCREEN)[0].1.w, 10.0, "A keeps its minimum");
    }

    #[test]
    fn dragging_past_the_ends_is_clamped_too() {
        let mut layout = row(leaf(A), leaf(B));
        assert!(layout.set_ratio(&[], 5.0, SCREEN, MIN));
        assert_eq!(layout.rects(SCREEN)[1].1.w, 10.0);
        assert!(layout.set_ratio(&[], -3.0, SCREEN, MIN));
        assert_eq!(layout.rects(SCREEN)[0].1.w, 10.0);
        assert_eq!(Layout::ratio_from_pointer(30.0, 100.0), 0.3);
        assert_eq!(Layout::ratio_from_pointer(-5.0, 100.0), 0.0);
        assert_eq!(Layout::ratio_from_pointer(5.0, 0.0), 0.5);
    }

    #[test]
    fn an_area_too_small_for_two_minimums_splits_evenly() {
        let mut layout = row(leaf(A), leaf(B));
        let tiny = Rect::new(0.0, 0.0, 15.0, 10.0);
        layout.set_ratio(&[], 0.9, tiny, MIN);
        let Layout::Split { ratio, .. } = layout else {
            panic!()
        };
        assert_eq!(ratio, 0.5);
    }

    #[test]
    fn equalizing_gives_every_pane_the_same_share() {
        let mut layout = row(leaf(A), column(leaf(B), row(leaf(C), leaf(D))));
        layout.set_ratio(&[], 0.2, SCREEN, MIN);
        layout.equalize();
        let widths: Vec<f32> = layout.rects(SCREEN).iter().map(|(_, r)| r.w).collect();
        // A takes 1 of 4 panes' share of the width; B/C/D the other three.
        assert_eq!(widths[0], 25.0);
        let Layout::Split { ratio, .. } = &layout else {
            panic!()
        };
        assert_eq!(*ratio, 0.25);
    }

    #[test]
    fn a_tab_splits_the_focused_pane_and_focuses_the_new_one() {
        let mut tab = Tab::new(A);
        tab.split(Axis::Row, B);
        assert_eq!((tab.focus, tab.layout.leaves()), (B, vec![A, B]));
        tab.split(Axis::Column, C);
        assert_eq!(tab.layout, row(leaf(A), column(leaf(B), leaf(C))));
    }

    #[test]
    fn closing_the_focused_pane_focuses_the_one_before_and_the_layout_collapses() {
        let mut tab = Tab::new(A);
        tab.split(Axis::Row, B);
        tab.split(Axis::Column, C);
        assert!(tab.close(C));
        assert_eq!((tab.focus, &tab.layout), (B, &row(leaf(A), leaf(B))));
        tab.focus = A;
        assert!(tab.close(A));
        assert_eq!((tab.focus, &tab.layout), (B, &leaf(B)));
        assert!(!tab.close(B), "the last pane: the tab goes");
    }

    #[test]
    fn closing_another_pane_keeps_the_focus() {
        let mut tab = Tab::new(A);
        tab.split(Axis::Row, B);
        tab.focus = A;
        tab.close(B);
        assert_eq!(tab.focus, A);
    }

    #[test]
    fn maximising_shows_the_focused_pane_alone_and_restoring_shows_all() {
        let mut tab = Tab::new(A);
        tab.toggle_zoom();
        assert!(!tab.zoomed, "one pane has nothing to maximise");
        tab.split(Axis::Row, B);
        tab.toggle_zoom();
        assert_eq!(tab.visible(SCREEN), [(B, SCREEN)]);
        tab.toggle_zoom();
        assert_eq!(tab.visible(SCREEN).len(), 2);
    }

    #[test]
    fn moving_the_focus_or_splitting_restores_a_maximised_tab() {
        let mut tab = Tab::new(A);
        tab.split(Axis::Row, B);
        tab.toggle_zoom();
        assert!(tab.focus_dir(Dir::Left, SCREEN));
        assert!(!tab.zoomed);
        assert_eq!(tab.focus, A);
        tab.toggle_zoom();
        tab.split(Axis::Column, C);
        assert!(!tab.zoomed);
    }

    #[test]
    fn closing_down_to_one_pane_ends_the_maximised_state() {
        let mut tab = Tab::new(A);
        tab.split(Axis::Row, B);
        tab.toggle_zoom();
        tab.close(A);
        assert!(!tab.zoomed);
    }

    #[test]
    fn the_focus_steps_in_order_and_ignores_a_single_pane() {
        let mut tab = Tab::new(A);
        assert!(!tab.focus_step(1));
        tab.split(Axis::Row, B);
        tab.focus = A;
        assert!(tab.focus_step(1));
        assert_eq!(tab.focus, B);
        assert!(tab.focus_step(1));
        assert_eq!(tab.focus, A, "wraps");
        assert!(!tab.focus_dir(Dir::Up, SCREEN));
    }
}

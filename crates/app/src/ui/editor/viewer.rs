//! Zoom and pan of a picture, and of the page of an SVG file.
//!
//! A [`Viewport`] lives with the document. At zoom 1 the picture is fitted in
//! the pane (never larger than it is); the wheel zooms about the pointer, a
//! drag pans and a double click fits it again. The pure parts (how big the
//! picture is drawn, where a zoom keeps the point under the pointer) are
//! tested below; [`Shell::render_viewer`] draws and listens.

use super::super::live::LiveId;
use super::super::shell::Shell;
use crate::theme::{metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{
    canvas, div, img, AnyElement, Bounds, Context, Image, MouseButton, MouseDownEvent,
    MouseMoveEvent, ObjectFit, Pixels, ScrollDelta, ScrollWheelEvent, Size,
};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

/// The smallest and the largest zoom, as a multiple of the fitted picture.
const ZOOM_RANGE: (f32, f32) = (0.1, 32.0);

/// The space left around the picture when it is fitted.
const MARGIN: f32 = 16.0;

/// How the person is looking at a picture.
pub struct Viewport {
    /// 1 is fitted; 2 is twice that.
    pub zoom: f32,
    /// How far the picture is moved from the centre of the pane, in pixels.
    pub pan: (f32, f32),
    /// The pane as it was last drawn, to size the picture from.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Where a drag began: the pointer and the pan then.
    drag: Option<((f32, f32), (f32, f32))>,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: (0.0, 0.0),
            bounds: Rc::new(Cell::new(Bounds::default())),
            drag: None,
        }
    }
}

impl Viewport {
    /// Fits the picture in the pane again.
    pub fn fit(&mut self) {
        self.zoom = 1.0;
        self.pan = (0.0, 0.0);
        self.drag = None;
    }

    /// Zooms by `factor` keeping the point under `at` (relative to the centre
    /// of the pane) where it is.
    pub fn zoom_about(&mut self, factor: f32, at: (f32, f32)) {
        let zoom = (self.zoom * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        let ratio = zoom / self.zoom;
        self.pan = (
            at.0 - (at.0 - self.pan.0) * ratio,
            at.1 - (at.1 - self.pan.1) * ratio,
        );
        self.zoom = zoom;
        if (self.zoom - 1.0).abs() < 0.001 {
            // Back at the fit: centred again, not a little off.
            self.fit();
        }
    }

    fn size(&self) -> Size<Pixels> {
        self.bounds.get().size
    }
}

/// The size the picture is drawn at: fitted in `pane` (less the margin, and
/// never larger than `natural` when it is known) and then zoomed.
pub fn drawn_size(pane: (f32, f32), natural: Option<(u32, u32)>, zoom: f32) -> (f32, f32) {
    let room = (
        (pane.0 - 2.0 * MARGIN).max(1.0),
        (pane.1 - 2.0 * MARGIN).max(1.0),
    );
    match natural {
        Some((width, height)) if width > 0 && height > 0 => {
            let (width, height) = (width as f32, height as f32);
            let fit = (room.0 / width).min(room.1 / height).min(1.0);
            (width * fit * zoom, height * fit * zoom)
        }
        // A vector has no size of its own: it fills the room.
        _ => (room.0 * zoom, room.1 * zoom),
    }
}

/// The zoom a person reads: of the picture's own size when it is known, else
/// of the fit.
pub fn percent(pane: (f32, f32), natural: Option<(u32, u32)>, zoom: f32) -> u32 {
    match natural {
        Some((width, _)) if width > 0 => {
            let (drawn, _) = drawn_size(pane, natural, zoom);
            (drawn / width as f32 * 100.0).round() as u32
        }
        _ => (zoom * 100.0).round() as u32,
    }
}

impl Shell {
    /// A picture (or the page of an SVG) the person can zoom and pan.
    pub(in crate::ui) fn render_viewer(
        &self,
        id: LiveId,
        image: Arc<Image>,
        natural: Option<(u32, u32)>,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(doc) = self.files.get(&id) else {
            return div().into_any_element();
        };
        let view = &doc.view;
        let pane = view.size();
        let pane = (pane.width.as_f32(), pane.height.as_f32());
        let measured = pane.0 > 0.0 && pane.1 > 0.0;
        let faint = colours.text_faint;
        let bounds = view.bounds.clone();
        let mut surface = div()
            .id(gpui_kit::SharedString::from(format!("viewer-{}", id.0)))
            .debug_selector(move || format!("viewer-{}", id.0))
            .relative()
            .size_full()
            .overflow_hidden()
            .cursor_grab()
            .child(
                canvas(|bounds, _, _| bounds, move |b, _, _, _| bounds.set(b))
                    .absolute()
                    .size_full(),
            )
            .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                let lines = match event.delta {
                    ScrollDelta::Pixels(delta) => delta.y.as_f32() / 40.0,
                    ScrollDelta::Lines(delta) => delta.y,
                };
                if let Some(doc) = this.files.get_mut(&id) {
                    let b = doc.view.bounds.get();
                    let at = (
                        (event.position.x - b.origin.x - b.size.width / 2.0).as_f32(),
                        (event.position.y - b.origin.y - b.size.height / 2.0).as_f32(),
                    );
                    // A wheel up zooms in.
                    doc.view.zoom_about(1.15f32.powf(lines), at);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if let Some(doc) = this.files.get_mut(&id) {
                        if event.click_count >= 2 {
                            doc.view.fit();
                        } else {
                            doc.view.drag = Some((
                                (event.position.x.as_f32(), event.position.y.as_f32()),
                                doc.view.pan,
                            ));
                        }
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let Some(doc) = this.files.get_mut(&id) else {
                    return;
                };
                if event.pressed_button != Some(MouseButton::Left) {
                    doc.view.drag = None;
                    return;
                }
                if let Some((from, pan)) = doc.view.drag {
                    doc.view.pan = (
                        pan.0 + event.position.x.as_f32() - from.0,
                        pan.1 + event.position.y.as_f32() - from.1,
                    );
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    if let Some(doc) = this.files.get_mut(&id) {
                        doc.view.drag = None;
                    }
                }),
            );
        let picture = img(image)
            .size_full()
            .object_fit(ObjectFit::Contain)
            .with_fallback(move || {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(faint)
                    .child("This cannot be drawn.")
                    .into_any_element()
            });
        if measured {
            let (width, height) = drawn_size(pane, natural, view.zoom);
            surface = surface.child(
                div()
                    .debug_selector(move || format!("file-image-{}", id.0))
                    .absolute()
                    .left(px(pane.0 / 2.0 - width / 2.0 + view.pan.0))
                    .top(px(pane.1 / 2.0 - height / 2.0 + view.pan.1))
                    .w(px(width))
                    .h(px(height))
                    .child(picture),
            );
        } else {
            // Not drawn yet, so the pane has no size: fitted, which is what
            // the first frame shows anyway.
            surface = surface.child(
                div()
                    .debug_selector(move || format!("file-image-{}", id.0))
                    .absolute()
                    .size_full()
                    .p(px(MARGIN))
                    .child(picture),
            );
        }
        if view.zoom != 1.0 {
            surface = surface.child(
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(10.))
                    .px(px(6.))
                    .rounded(metrics::RADIUS())
                    .bg(colours.surface)
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_muted)
                    .child(format!("{}%", percent(pane, natural, view.zoom))),
            );
        }
        surface.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_picture_is_not_enlarged_and_a_big_one_is_fitted() {
        let pane = (400.0, 300.0);
        assert_eq!(drawn_size(pane, Some((16, 16)), 1.0), (16.0, 16.0));
        let (width, height) = drawn_size(pane, Some((2000, 1000)), 1.0);
        assert_eq!((width, height), (368.0, 184.0));
        assert_eq!(drawn_size(pane, Some((16, 16)), 4.0), (64.0, 64.0));
        // A vector fills the room.
        assert_eq!(drawn_size(pane, None, 1.0), (368.0, 268.0));
    }

    #[test]
    fn the_percent_is_of_the_picture_itself() {
        let pane = (400.0, 300.0);
        assert_eq!(percent(pane, Some((16, 16)), 1.0), 100);
        assert_eq!(percent(pane, Some((16, 16)), 2.0), 200);
        assert_eq!(percent(pane, Some((2000, 1000)), 1.0), 18);
        assert_eq!(percent(pane, None, 1.5), 150);
    }

    #[test]
    fn a_zoom_keeps_the_point_under_the_pointer_and_stays_in_range() {
        let mut view = Viewport::default();
        // The pointer is 100 px right of the centre: the picture grows away
        // from it, so it moves left to keep that point still.
        view.zoom_about(2.0, (100.0, 0.0));
        assert_eq!((view.zoom, view.pan), (2.0, (-100.0, 0.0)));
        // Zooming back out to the fit centres it again.
        view.zoom_about(0.5, (100.0, 0.0));
        assert_eq!((view.zoom, view.pan), (1.0, (0.0, 0.0)));
        for _ in 0..40 {
            view.zoom_about(2.0, (0.0, 0.0));
        }
        assert_eq!(view.zoom, ZOOM_RANGE.1);
        for _ in 0..40 {
            view.zoom_about(0.5, (0.0, 0.0));
        }
        assert_eq!(view.zoom, ZOOM_RANGE.0);
        view.pan = (5.0, 5.0);
        view.fit();
        assert_eq!((view.zoom, view.pan), (1.0, (0.0, 0.0)));
    }
}

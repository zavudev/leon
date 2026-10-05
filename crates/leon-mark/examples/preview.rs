//! Shows the lion at every size and in every mood, on dark and on light.
//!
//! ```text
//! cargo run -p leon-mark --example preview
//! ```
//!
//! Each row is a mood (idle, working, waiting, error, asleep); on its left
//! the lion on the dark theme's ink in acid yellow, on its right on the light
//! theme's paper in ink, at 16, 20, 32, 64, 128 and 256 pixels. Everything is
//! alive: hover a mark to make it narrow its eyes. The 16 and 20 pixel marks
//! are the fitted geometry and play only the blink and the glare. Scroll for
//! the rows below.
//!
//! Environment:
//!
//! - `LEON_MARK_STILL=1` paints the rest pose and stops the animation.
//! - `LEON_MARK_REDUCED=1` turns reduced motion on.
//! - `LEON_MARK_INTRO=1` plays the intro on every mark when it opens.
//! - `LEON_MARK_TRACE=1` prints every redraw of the view with its time: how
//!   often the lion costs a frame.

use std::time::Instant;

use gpui_kit::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, Pixels, Render, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use leon_mark::{AnimatedMark, Mood};

const SIZES: [f32; 6] = [16., 20., 32., 64., 128., 256.];
const MOODS: [Mood; 5] = [
    Mood::Idle,
    Mood::Working,
    Mood::Waiting,
    Mood::Error,
    Mood::Asleep,
];

// The two surfaces of the Leon themes: ink with the acid yellow mark, paper
// with the ink mark.
const INK: u32 = 0x0a_0a_0a;
const PAPER: u32 = 0xfa_fa_f9;
const ACID: u32 = 0xff_ea_00;

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.is_empty() && value != "0")
}

struct Preview {
    opened: Instant,
    renders: usize,
}

impl Preview {
    fn panel(&self, mood: Mood, dark: bool) -> impl IntoElement {
        let (background, color) = if dark { (INK, ACID) } else { (PAPER, INK) };
        div()
            .flex_1()
            .flex()
            .items_center()
            .gap(px(28.))
            .px(px(24.))
            .py(px(20.))
            .bg(rgb(background))
            .children(SIZES.iter().enumerate().map(|(index, side)| {
                AnimatedMark::new(px(*side))
                    .id(SharedString::from(format!("{mood:?}-{dark}-{side}")))
                    .seed(index as u64 * 7 + u64::from(dark))
                    .color(rgb(color))
                    .mood(mood)
                    .animate(!flag("LEON_MARK_STILL"))
                    .reduced_motion(flag("LEON_MARK_REDUCED"))
                    .intro(flag("LEON_MARK_INTRO"))
                    .play_on_hover(true)
            }))
    }
}

impl Render for Preview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // How often the view is redrawn is what the animation costs.
        self.renders += 1;
        if flag("LEON_MARK_TRACE") {
            let at = self.opened.elapsed().as_secs_f64();
            eprintln!("render {} at {at:.3}", self.renders);
        }
        div()
            .id("rows")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .bg(rgb(INK))
            .children(MOODS.map(|mood| {
                div()
                    .flex_none()
                    .flex()
                    .child(self.panel(mood, true))
                    .child(self.panel(mood, false))
            }))
    }
}

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        let extent: gpui_kit::Size<Pixels> = size(px(1500.), px(900.));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, extent, cx))),
            titlebar: Some(TitlebarOptions {
                title: Some("Leon mark".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.open_window(options, |_, cx| {
            cx.new(|_| Preview {
                opened: Instant::now(),
                renders: 0,
            })
        })
        .expect("the preview window opens");
        cx.activate(true);
    });
}

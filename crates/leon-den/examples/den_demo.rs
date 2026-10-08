//! The Den in a window, with a cast of made-up lions that go through every
//! state, and the happenings that go with them.
//!
//! ```text
//! cargo run -p leon-den --example den_demo
//! ```
//!
//! Keys: `t` switches between the dark and the light theme, `tab` and
//! `shift-tab` (or the arrows) move the selection, `enter` opens it (the
//! title of the window says which), `escape` clears it; `page up`, `page
//! down`, `home` and `end` move the feed, `shift` with `up` or `down` walks
//! its entries and `enter` then opens a long message; `+` and `-` change
//! how many lions there are, `r` turns reduced motion on and off, `p` moves
//! the pride to the next built-in den.
//!
//! Environment:
//!
//! - `DEN_THEME=light` starts in the light theme.
//! - `DEN_PREFAB=library` starts in that built-in den (the office).
//! - `DEN_CUBS=9` sets how many lions there are (6).
//! - `DEN_ROUND_MS=1500` sets how often a lion moves on in its script.
//! - `DEN_REDUCED=1` turns reduced motion on.
//! - `DEN_FONT="Some Mono"` sets the font of the text (JetBrains Mono, which
//!   the app bundles and this example does not).
//! - `DEN_START=7` starts every lion that many steps into its script.
//! - `DEN_WIDTH` and `DEN_HEIGHT` set the size of the window.
//! - `DEN_SELECT=1` selects that lion of the roster at the start.
//! - `DEN_TRACE=1` prints, every round, how many times the view painted, how
//!   many pictures of the room it composited and how many timers it set:
//!   what the Den costs.

#[path = "support/mod.rs"]
mod support;

use std::time::Duration;

use gpui_kit::{
    div, prelude::*, px, size, App, Bounds, Context, Entity, FocusHandle, KeyDownEvent, Pixels,
    Render, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use leon_den::model::Cub;
use leon_den::{DenEvent, DenStyle, DenView};

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.is_empty() && value != "0")
}

fn number(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn style(dark: bool) -> DenStyle {
    DenStyle {
        palette: support::palette(dark),
        font_family: std::env::var("DEN_FONT")
            .unwrap_or_else(|_| "JetBrains Mono".to_owned())
            .into(),
        font_size: px(13.),
    }
}

struct Demo {
    den: Entity<DenView>,
    focus: FocusHandle,
    dark: bool,
    count: usize,
    round: usize,
    start: usize,
    reduced: bool,
    cast: Vec<Cub>,
    opened: Option<u64>,
    prefab: usize,
}

impl Demo {
    fn advance(&mut self, cx: &mut Context<Self>) {
        // One lion moves on at a time, each in its turn, as sessions do.
        let (count, round) = (self.count.max(1), self.round);
        let cast = support::cast(self.dark, self.count, |index| {
            self.start + (round + count - 1 - index) / count
        });
        let happenings = support::happenings(&self.cast, &cast, self.round);
        let speeches = support::speeches(&self.cast, &cast, self.round);
        // The demo reads the clock; the view is told the time.
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs() as i64);
        let first = self.round == 0;
        self.den.update(cx, |den, cx| {
            den.set_wall_time(wall, cx);
            den.set_cubs(&cast, cx);
            if first {
                // What the sessions did before the Den was opened.
                for (index, cub) in cast.iter().filter(|cub| cub.parent.is_none()).enumerate() {
                    den.remember(cub.id, &cub.name, &support::past(index, wall), cx);
                }
            }
            for happening in &happenings {
                den.happen(happening, cx);
            }
            for (cub, name, text) in &speeches {
                den.speak(*cub, name, text, None, cx);
            }
        });
        if flag("DEN_TRACE") {
            let (paints, pictures, timers) = self.den.read(cx).cost();
            eprintln!(
                "round {}: {paints} paints, {pictures} pictures, {timers} timers",
                self.round
            );
        }
        if self.round == 0 {
            if let Some(nth) = std::env::var("DEN_SELECT")
                .ok()
                .and_then(|n| n.parse().ok())
            {
                self.den.update(cx, |den, cx| {
                    let id = den.read(|den| den.order().get::<usize>(nth).copied());
                    den.select(id, cx);
                });
            }
        }
        self.cast = cast;
        self.round += 1;
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let shift = event.keystroke.modifiers.shift;
        match event.keystroke.key.as_str() {
            "t" => {
                self.dark = !self.dark;
                let dark = self.dark;
                self.den
                    .update(cx, |den, cx| den.set_style(style(dark), cx));
                // The manes are theme colours too.
                self.round = self.round.saturating_sub(1);
                self.advance(cx);
            }
            "r" => {
                self.reduced = !self.reduced;
                let reduced = self.reduced;
                self.den
                    .update(cx, |den, cx| den.set_reduced_motion(Some(reduced), cx));
            }
            "pageup" => self.den.update(cx, |den, cx| den.scroll_feed(-1, cx)),
            "pagedown" => self.den.update(cx, |den, cx| den.scroll_feed(1, cx)),
            "home" => self.den.update(cx, |den, cx| den.feed_to(false, cx)),
            "end" => self.den.update(cx, |den, cx| den.feed_to(true, cx)),
            "up" if shift => self.den.update(cx, |den, cx| den.feed_step(true, cx)),
            "down" if shift => self.den.update(cx, |den, cx| den.feed_step(false, cx)),
            "tab" if shift => self.den.update(cx, |den, cx| den.select_previous(cx)),
            "tab" | "down" | "right" => self.den.update(cx, |den, cx| den.select_next(cx)),
            "up" | "left" => self.den.update(cx, |den, cx| den.select_previous(cx)),
            "enter" => {
                self.den.update(cx, |den, cx| {
                    if !den.feed_toggle(cx) {
                        den.open_selection(cx);
                    }
                });
            }
            "escape" => self.den.update(cx, |den, cx| {
                if !den.feed_release(cx) {
                    den.select(None, cx);
                }
            }),
            "p" => {
                let all = leon_den::prefabs::prefabs();
                self.prefab = (self.prefab + 1) % all.len();
                let layout = all[self.prefab].layout.clone();
                self.den.update(cx, |den, cx| den.set_layout(&layout, cx));
            }
            "+" | "=" => self.count = (self.count + 1).min(24),
            "-" => self.count = self.count.saturating_sub(1),
            _ => return,
        }
        window.set_window_title(&match self.opened {
            Some(id) => format!("The Den - opened lion {id}"),
            None => "The Den".to_owned(),
        });
        cx.notify();
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("demo")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .child(self.den.clone())
    }
}

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        let extent: gpui_kit::Size<Pixels> = size(
            px(number("DEN_WIDTH", 1180) as f32),
            px(number("DEN_HEIGHT", 760) as f32),
        );
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, extent, cx))),
            titlebar: Some(TitlebarOptions {
                title: Some("The Den".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let dark = std::env::var("DEN_THEME").map_or(true, |theme| theme != "light");
        let round = Duration::from_millis(number("DEN_ROUND_MS", 1500) as u64);
        cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                let den = cx.new(|cx| DenView::new(style(dark), cx));
                if flag("DEN_REDUCED") {
                    den.update(cx, |den, cx| den.set_reduced_motion(Some(true), cx));
                }
                cx.subscribe(&den, |demo: &mut Demo, _, event, cx| {
                    if let DenEvent::Opened(id) = event {
                        demo.opened = Some(*id);
                        cx.notify();
                    }
                })
                .detach();
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                let all = leon_den::prefabs::prefabs();
                let prefab = std::env::var("DEN_PREFAB")
                    .ok()
                    .and_then(|id| all.iter().position(|prefab| prefab.id == id))
                    .unwrap_or(0);
                let layout = all[prefab].layout.clone();
                den.update(cx, |den, cx| den.set_layout(&layout, cx));
                let mut demo = Demo {
                    den,
                    focus,
                    dark,
                    count: number("DEN_CUBS", 6),
                    round: 0,
                    start: number("DEN_START", 0),
                    reduced: flag("DEN_REDUCED"),
                    cast: Vec::new(),
                    opened: None,
                    prefab,
                };
                demo.advance(cx);
                cx.spawn(async move |demo, cx| loop {
                    cx.background_executor().timer(round).await;
                    if demo.update(cx, |demo, cx| demo.advance(cx)).is_err() {
                        break;
                    }
                })
                .detach();
                demo
            })
        })
        .expect("the window opens");
        cx.activate(true);
    });
}

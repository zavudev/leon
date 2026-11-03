//! The sidebar as a view of its own, so that the window can draw it again
//! only when something in it changed.
//!
//! The toolkit draws a view again whenever one of the views inside it changes,
//! and the shell holds every view of the window: the terminal on screen
//! changes at every byte its program prints, which used to draw the whole
//! sidebar again (its tree, its rows, their text) up to the refresh rate of
//! the screen while an agent streams. The sidebar is kept as it was drawn
//! until it is told otherwise:
//!
//! * every time the shell says it changed (a click, the store, the engine,
//!   an agent's light, the spinners), because the sidebar draws the shell's
//!   state;
//! * when the settings change or the tree is built again, announced or not (see
//!   [`Shell::sync_sidebar`]), since a change made while drawing is not
//!   announced;
//! * every [`AGES`], for the ages written in the rows;
//! * by itself: a row hovered, the list scrolled, the lion's gestures and the
//!   filter's field are the sidebar's own.

use super::shell::Shell;
use crate::theme::palette;
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Entity, Subscription, Task, WeakEntity, Window};
use std::time::Duration;

/// How often the sidebar is drawn again when nothing else asks: the ages of
/// the rows ("now", "7m") are written to the minute.
pub(super) const AGES: Duration = Duration::from_secs(30);

pub(super) struct SidebarView {
    shell: WeakEntity<Shell>,
    _shell_changed: Subscription,
    _ages: Task<()>,
}

impl SidebarView {
    pub(super) fn new(shell: &Entity<Shell>, cx: &mut Context<Self>) -> Self {
        let shell_changed = cx.observe(shell, |_, _, cx| cx.notify());
        let ages = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(AGES).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                return;
            }
        });
        Self {
            shell: shell.downgrade(),
            _shell_changed: shell_changed,
            _ages: ages,
        }
    }
}

impl Render for SidebarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colours = palette(cx);
        self.shell
            .update(cx, |shell, cx| shell.render_sidebar(&colours, cx))
            .unwrap_or_else(|_| div())
    }
}

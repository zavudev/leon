//! Project logos on screen.
//!
//! The snapshot says which logo each project has (its content hash and
//! format, never its bytes). The first time a hash is seen, its bytes are read
//! from the store on a background thread and kept as a toolkit `Image`; the
//! toolkit then decodes it off the UI thread and caches the decoded picture by
//! the image's own content id, so drawing a row again, scrolling included,
//! never decodes anything. Until the bytes are there, and for a project with
//! no image, the row shows the folder glyph.

use super::shell::Shell;
use crate::icons::{icon, IconName};
use crate::theme::{metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{div, img, AnyElement, Context, Image, ImageFormat, ObjectFit, Pixels};
use leon_core::{IconFormat, ProjectId};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The images read so far, by content hash.
#[derive(Default)]
pub struct Logos {
    images: HashMap<String, Arc<Image>>,
    pending: HashSet<String>,
}

impl Logos {
    /// How many images are held.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.images.len()
    }
}

/// The toolkit's name for a format.
fn toolkit_format(format: IconFormat) -> ImageFormat {
    match format {
        IconFormat::Png => ImageFormat::Png,
        IconFormat::Webp => ImageFormat::Webp,
        IconFormat::Ico => ImageFormat::Ico,
        IconFormat::Jpeg => ImageFormat::Jpeg,
        IconFormat::Svg => ImageFormat::Svg,
    }
}

impl Shell {
    /// Starts reading the bytes of every logo of the snapshot that has not
    /// been read.
    pub(super) fn load_logos(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<String> = self
            .snapshot
            .icons
            .values()
            .filter_map(|icon| icon.hash.clone())
            .filter(|hash| {
                !self.logos.images.contains_key(hash) && !self.logos.pending.contains(hash)
            })
            .collect();
        for hash in wanted {
            self.logos.pending.insert(hash.clone());
            let store = self.engine.store().clone();
            let wanted = hash.clone();
            cx.spawn(async move |this, cx| {
                let read = cx
                    .background_spawn(async move { store.icon_image(&wanted) })
                    .await;
                this.update(cx, |this, cx| {
                    this.logos.pending.remove(&hash);
                    if let Ok(Some(image)) = read {
                        let image = Image::from_bytes(toolkit_format(image.format), image.bytes);
                        this.logos.images.insert(hash, Arc::new(image));
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    /// A project's logo, `size` square: its image inside a hairline and the
    /// control radius, scaled to fit, or the folder glyph. `place` names the
    /// spot for tests (`tree-4-logo-image`, `tree-4-logo-folder`).
    pub(super) fn logo(
        &self,
        project: &ProjectId,
        size: Pixels,
        place: &str,
        colours: &Palette,
    ) -> AnyElement {
        let image = self
            .snapshot
            .icons
            .get(project)
            .and_then(|icon| icon.hash.as_ref())
            .and_then(|hash| self.logos.images.get(hash));
        match image {
            Some(image) => {
                let selector = format!("{place}-logo-image");
                div()
                    .debug_selector(move || selector.clone())
                    .flex_none()
                    .size(size)
                    .overflow_hidden()
                    .rounded(metrics::RADIUS())
                    .border_1()
                    .border_color(colours.border)
                    .child(
                        img(image.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .into_any_element()
            }
            None => {
                let selector = format!("{place}-logo-folder");
                div()
                    .debug_selector(move || selector.clone())
                    .flex_none()
                    .size(size)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(IconName::Folder, size - px(2.), colours.text_muted))
                    .into_any_element()
            }
        }
    }
}

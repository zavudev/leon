//! The icons the UI uses, embedded in the binary.
//!
//! Only the icons listed here are compiled in (from the Lucide set bundled
//! with the component library), on top of the few the component library
//! itself needs.

use crate::brand;
pub use gpui_kit::assets::IconName;
use gpui_kit::assets::{icon_assets, Assets as ComponentAssets};
use gpui_kit::component::Icon;
use gpui_kit::{AssetSource, Hsla, Pixels, Result, SharedString, Styled};
use std::borrow::Cow;

icon_assets!(
    AppIcons,
    [
        Bot,
        Check,
        ChevronDown,
        ChevronRight,
        Circle,
        CircleAlert,
        CircleDot,
        Command,
        Folder,
        FolderOpen,
        GitBranch,
        Hash,
        Info,
        Keyboard,
        Laptop,
        Moon,
        PanelLeft,
        PanelLeftClose,
        Plus,
        RefreshCw,
        Search,
        Server,
        Settings,
        Slash,
        Sun,
        Terminal,
        Trash,
        TriangleAlert,
        X,
    ]
);

/// The application's asset source: the brand assets and its own icons first,
/// then the component library's defaults.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = brand::load(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        match AppIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => ComponentAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut names: Vec<SharedString> = brand::paths()
            .filter(|name| name.starts_with(path))
            .map(Into::into)
            .collect();
        names.extend(AppIcons.list(path)?);
        names.extend(ComponentAssets.list(path)?);
        Ok(names)
    }
}

/// An icon of the given size and colour.
pub fn icon(name: IconName, size: Pixels, colour: Hsla) -> Icon {
    Icon::new(name).size(size).text_color(colour)
}

/// The logo of an agent at `size`, in the agent's colour token of `palette`.
pub fn agent_icon(
    agent: leon_core::AgentKind,
    size: Pixels,
    palette: &crate::theme::Palette,
) -> impl gpui_kit::IntoElement {
    gpui_kit::svg()
        .path(crate::brand::agent_mark(agent))
        .flex_none()
        .size(size)
        .text_color(palette.agent(agent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_logo_loads_through_the_asset_source() {
        for agent in leon_core::AgentKind::ALL {
            let loaded = Assets.load(crate::brand::agent_mark(agent)).unwrap();
            assert!(loaded.is_some_and(|bytes| bytes.starts_with(b"<svg")));
        }
    }
}

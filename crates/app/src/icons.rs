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
        ExternalLink,
        File,
        Folder,
        FolderOpen,
        GitBranch,
        GitCommitHorizontal,
        GitMerge,
        Github,
        Hash,
        House,
        Info,
        Keyboard,
        Laptop,
        Moon,
        PanelLeft,
        PanelLeftClose,
        Pin,
        PawPrint,
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
/// An agent without a bundled mark gets a letter-mark tile: its initials on a
/// hairline square, drawn from the theme's tokens (generated, so it needs no
/// asset and no licence).
pub fn agent_icon(
    agent: leon_core::AgentId,
    size: Pixels,
    palette: &crate::theme::Palette,
) -> gpui_kit::AnyElement {
    agent_icon_in(agent, size, palette, Tone::Full)
}

/// How strongly a logo is drawn: the state of the row it leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The agent's own colour: something runs.
    Full,
    /// Dimmed: it was put to sleep and waits to be resumed.
    Asleep,
    /// Nearly grey: only its history is kept.
    Faded,
}

impl Tone {
    /// `colour` as this tone draws it.
    pub fn of(self, colour: Hsla) -> Hsla {
        match self {
            Self::Full => colour,
            Self::Asleep => Hsla {
                s: colour.s * 0.6,
                a: colour.a * 0.55,
                ..colour
            },
            Self::Faded => Hsla {
                s: colour.s * 0.1,
                a: colour.a * 0.6,
                ..colour
            },
        }
    }
}

/// [`agent_icon`] in a tone.
pub fn agent_icon_in(
    agent: leon_core::AgentId,
    size: Pixels,
    palette: &crate::theme::Palette,
    tone: Tone,
) -> gpui_kit::AnyElement {
    use gpui_kit::prelude::*;
    let colour = tone.of(palette.agent(agent));
    match crate::brand::agent_mark(agent) {
        Some(path) => gpui_kit::svg()
            .path(path)
            .flex_none()
            .size(size)
            .text_color(colour)
            .into_any_element(),
        None => {
            let letters = agent.spec().map_or_else(
                || leon_core::agent::initials(agent.name()),
                leon_core::AgentSpec::initials,
            );
            gpui_kit::div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(size)
                .rounded(size * 0.2)
                .border_1()
                .border_color(palette.elevated_border)
                .text_color(colour)
                .font_family(crate::theme::fonts::mono())
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_size(size * 0.42)
                .child(letters)
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_agent_logo_loads_through_the_asset_source() {
        for spec in leon_core::agent::builtin() {
            let Some(path) = crate::brand::agent_mark(spec.id) else {
                continue;
            };
            let loaded = Assets.load(&path).unwrap();
            assert!(
                loaded.is_some_and(|bytes| bytes.starts_with(b"<svg")),
                "{path}"
            );
        }
    }

    #[test]
    fn the_sidebar_icons_load_through_the_asset_source() {
        for name in [
            IconName::GitBranch,
            IconName::GitCommitHorizontal,
            IconName::GitMerge,
            IconName::Github,
            IconName::Check,
            IconName::Pin,
            IconName::House,
            IconName::PawPrint,
        ] {
            let path = gpui_kit::assets::IconNamed::path(name);
            let loaded = Assets.load(&path).unwrap();
            assert!(
                loaded.is_some_and(|bytes| bytes.starts_with(b"<svg")),
                "{path}"
            );
        }
    }

    #[test]
    fn an_agent_without_a_mark_gets_letters_and_every_agent_gets_something() {
        let without: Vec<&str> = leon_core::agent::builtin()
            .iter()
            .filter(|spec| crate::brand::agent_mark(spec.id).is_none())
            .map(|spec| spec.id.as_str())
            .collect();
        assert!(
            without.contains(&"amp") && !without.contains(&"grok") && without.len() > 10,
            "{without:?}"
        );
        for spec in leon_core::agent::builtin() {
            let letters = spec.initials();
            assert!((1..=2).contains(&letters.chars().count()), "{}", spec.id);
        }
    }
}

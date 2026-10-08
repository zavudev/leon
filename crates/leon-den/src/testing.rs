//! What the tests share: the tokens of the Leon themes and a few lions.

use std::time::Duration;

use gpui_kit::{rgb, Hsla};

use crate::model::{Cub, CubState, Species};
use crate::palette::{DenPalette, Tokens};

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// The tokens of the Leon theme, dark or light (`brand/tokens.md`).
pub fn tokens(dark: bool) -> Tokens {
    let pick = |dark_value: u32, light_value: u32| hex(if dark { dark_value } else { light_value });
    Tokens {
        background: pick(0x0a0a0a, 0xfafaf9),
        surface: pick(0x141414, 0xffffff),
        surface_2: pick(0x1c1c1c, 0xf5f5f4),
        border: pick(0x262626, 0xe7e5e4),
        guide: pick(0x333333, 0xd3d1d0),
        grid_mark: pick(0x555555, 0xaaa8a7),
        text: pick(0xfafaf9, 0x0c0a09),
        text_muted: pick(0xa8a29e, 0x57534e),
        text_faint: pick(0x8c8580, 0x78716c),
        signal: pick(0xffea00, 0x756600),
        accent_fill: hex(0xffea00),
        on_accent_fill: hex(0x0a0a0a),
        success: pick(0x4df688, 0x047857),
        warning: pick(0xff9500, 0xa06000),
        error: pick(0xff5e5e, 0xb91c1c),
        info: pick(0x6efaff, 0x0e7490),
    }
}

/// The palette of the den in the Leon theme.
pub fn palette(dark: bool) -> DenPalette {
    DenPalette::from_tokens(&tokens(dark))
}

/// A lion with this id, called `name`, in this state.
pub fn cub(id: u64, name: &str, state: CubState) -> Cub {
    Cub {
        id,
        name: name.to_owned(),
        species: Species {
            tint: hex(0xd97757),
            seed: id,
        },
        state,
        level: 3,
        detail: None,
        parent: None,
        mystery: false,
    }
}

/// A little one of `parent`.
pub fn little(id: u64, parent: u64, state: CubState) -> Cub {
    Cub {
        parent: Some(parent),
        ..cub(id, "explore", state)
    }
}

/// A time in tenths of a second: in ticks.
pub fn at(ticks: u64) -> Duration {
    Duration::from_millis(ticks * 100)
}

/// A den in the prefab with this id, with nobody in it.
pub fn den_in(prefab: &str) -> crate::sim::Den {
    let mut den = crate::sim::Den::new();
    let room = crate::prefabs::prefab(prefab).expect("a prefab").layout;
    den.set_layout(&room, false, Duration::ZERO);
    den
}

/// What the feed of a den says, the oldest first, each entry on one line.
pub fn told(den: &crate::sim::Den) -> Vec<String> {
    den.feed()
        .entries()
        .map(|entry| entry.item.text.replace('\n', " "))
        .collect()
}

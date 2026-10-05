//! The brand assets bundled with the application: the marks of the agents and
//! the font families of every theme.
//!
//! Everything is compiled into the binary from `crates/app/assets`, whose
//! `ASSETS.md` records where each file comes from and its licence.
//!
//! The Leon glare is not bundled as a picture: it is always the animated mark
//! of the `leon-mark` crate, painted from vector shapes. `assets/brand/` keeps
//! the owner's SVGs of it as the source those shapes are tested against.

use std::borrow::Cow;

/// The logo of an agent, one single-path SVG drawn with `currentColor`.
pub fn agent_mark(agent: leon_core::AgentKind) -> &'static str {
    match agent {
        leon_core::AgentKind::Claude => "agents/claude.svg",
        leon_core::AgentKind::Codex => "agents/codex.svg",
        leon_core::AgentKind::Opencode => "agents/opencode.svg",
    }
}

const FILES: [(&str, &[u8]); 3] = [
    (
        "agents/claude.svg",
        include_bytes!("../assets/agents/claude.svg"),
    ),
    (
        "agents/codex.svg",
        include_bytes!("../assets/agents/codex.svg"),
    ),
    (
        "agents/opencode.svg",
        include_bytes!("../assets/agents/opencode.svg"),
    ),
];

/// The bytes of a bundled brand asset.
pub fn load(path: &str) -> Option<&'static [u8]> {
    FILES
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, bytes)| *bytes)
}

/// The paths of the bundled brand assets.
pub fn paths() -> impl Iterator<Item = &'static str> {
    FILES.iter().map(|(name, _)| *name)
}

/// The application icon for the window, where the platform takes one from the
/// application: only X11 does. Wayland takes it from the desktop entry
/// (`assets/linux`), macOS from the bundle (`packaging/macos`) and Windows
/// from the executable's icon resource (`build.rs`).
pub fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    let png = include_bytes!("../assets/icons/app-icon-256.png");
    image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()
        .map(|icon| std::sync::Arc::new(icon.into_rgba8()))
}

/// Puts the mark in the Dock and the application switcher of a run that is
/// not in a bundle (a `target/debug/leon` started by hand; `cargo run` is
/// bundled by its runner, `scripts/cargo-runner-macos.sh`): a bundle has its icon from `Info.plist`, and
/// AppKit shows a bare executable with the generic one. Call it on the main
/// thread once the application is running. `true` when AppKit took the image.
#[cfg(target_os = "macos")]
pub fn set_dock_icon() -> bool {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let bundled = std::env::current_exe()
        .map(|path| path.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false);
    if bundled {
        return true;
    }
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let data = NSData::with_bytes(include_bytes!("../assets/icons/app-icon-macos-512.png"));
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return false;
    };
    // SAFETY: on the main thread (the marker), with an image that is a valid
    // NSImage; AppKit retains it.
    unsafe {
        NSApplication::sharedApplication(main_thread).setApplicationIconImage(Some(&image));
    }
    true
}

/// Names the process after the product, for what reads `NSProcessInfo`. It does
/// not change the Dock label, Activity Monitor or the menu-bar title: macOS
/// takes those from the bundle (`Leon.app`, also the development one the cargo
/// runner makes) or, with none, from the executable's file name. It stays for
/// an unbundled run, where it is the one thing that can still say `Leon`. Call
/// it first in `main`, before the application exists.
#[cfg(target_os = "macos")]
pub fn set_process_name() {
    use objc2_foundation::{NSProcessInfo, NSString};
    NSProcessInfo::processInfo().setProcessName(&NSString::from_str(crate::product::PRODUCT_NAME));
}

/// Nothing to do off macOS.
#[cfg(not(target_os = "macos"))]
pub fn set_process_name() {}

/// Where the platform does not need it, there is nothing to do.
#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon() -> bool {
    true
}

/// The font files of every theme, all registered at start so that switching
/// theme loads nothing: Space Grotesk (regular, medium, bold), Geist Mono
/// (regular, medium, semibold), Inter (regular, italic, medium, semibold) and
/// JetBrains Mono (regular, medium).
pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    [
        &include_bytes!("../assets/fonts/SpaceGrotesk-Regular.ttf")[..],
        include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf"),
        include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf"),
        include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
        include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
        include_bytes!("../assets/fonts/GeistMono-SemiBold.ttf"),
        include_bytes!("../assets/fonts/Inter-Regular.ttf"),
        include_bytes!("../assets/fonts/Inter-Italic.ttf"),
        include_bytes!("../assets/fonts/Inter-Medium.ttf"),
        include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
        include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
        include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
    ]
    .into_iter()
    .map(Cow::Borrowed)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// The side of the PNG the window is given on platforms that take its icon from
    /// the application (X11).
    const WINDOW_ICON_SIZE: u32 = 256;

    /// The sides of the full-bleed PNGs: the freedesktop hicolor sizes, the
    /// window icon and the pictures inside the Windows icon.
    const ICON_SIZES: [u32; 7] = [16, 32, 48, 64, 128, 256, 512];

    /// The sides of the macOS PNGs, the artwork inset in the rounded-square
    /// template: what an `.iconset` is cut from.
    const MACOS_SIZES: [u32; 7] = [16, 32, 64, 128, 256, 512, 1024];

    /// The sizes inside the Windows `.ico`.
    const WINDOWS_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

    fn app_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn png_size(path: &Path) -> (u32, u32) {
        let bytes = std::fs::read(path).unwrap_or_else(|_| panic!("{} is missing", path.display()));
        let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .unwrap_or_else(|_| panic!("{} is not a PNG", path.display()));
        (image.width(), image.height())
    }

    #[test]
    fn the_window_icon_decodes_to_a_square_rgba_image_of_the_expected_size() {
        let icon = window_icon().expect("the embedded icon decodes");
        assert_eq!(icon.dimensions(), (WINDOW_ICON_SIZE, WINDOW_ICON_SIZE));
        assert!(
            icon.pixels().any(|pixel| pixel.0[3] == 255),
            "the icon is not empty"
        );
    }

    #[test]
    fn every_size_the_platforms_need_is_generated_and_is_the_size_its_name_says() {
        let icons = app_dir().join("assets/icons");
        for size in ICON_SIZES {
            assert_eq!(
                png_size(&icons.join(format!("app-icon-{size}.png"))),
                (size, size)
            );
        }
        for size in MACOS_SIZES {
            assert_eq!(
                png_size(&icons.join(format!("app-icon-macos-{size}.png"))),
                (size, size)
            );
        }
    }

    #[test]
    fn the_windows_icon_holds_every_size_and_is_the_resource_the_window_loads() {
        let ico = std::fs::read(app_dir().join("assets/icons/app-icon.ico")).expect("the .ico");
        assert_eq!(&ico[..4], &[0, 0, 1, 0], "an icon file");
        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
        let sizes: Vec<u32> = (0..count)
            .map(|entry| match ico[6 + entry * 16] {
                0 => 256,
                size => u32::from(size),
            })
            .collect();
        assert_eq!(sizes, WINDOWS_SIZES);
    }

    #[test]
    fn the_macos_icon_and_the_bundle_script_are_there() {
        let icns = std::fs::read(app_dir().join("assets/icons/app-icon.icns")).expect("the .icns");
        assert_eq!(&icns[..4], b"icns");
        let script = std::fs::read_to_string(app_dir().join("../../packaging/macos/bundle.sh"))
            .expect("the bundle script");
        assert!(script.contains("dev.zavu.leon"));
        assert!(script.contains("AppIcon.icns"));
    }

    #[test]
    fn the_linux_desktop_entry_is_named_after_the_application_id() {
        let id = crate::product::APP_ID;
        let entry = std::fs::read_to_string(app_dir().join(format!("assets/linux/{id}.desktop")))
            .expect("the desktop entry is named after the application id");
        assert!(entry.contains(&format!("Icon={id}")));
        assert!(entry.contains(&format!("StartupWMClass={id}")));
        assert!(entry.contains("Exec=leon"));
    }

    #[test]
    fn both_drawings_of_the_mark_stay_one_cut_layer_the_animated_mark_is_tested_against() {
        for file in ["leon-mark.svg", "leon-mark-16.svg"] {
            let svg = std::fs::read_to_string(app_dir().join("assets/brand").join(file)).unwrap();
            assert_eq!(svg.matches("<path").count(), 1, "{file} is one layer");
            assert!(svg.contains("fill-rule=\"evenodd\""), "{file}");
            assert!(svg.contains("fill=\"currentColor\""), "{file}");
        }
        // The glare is painted by the animated mark; no picture of it is bundled.
        assert!(paths().all(|name| !name.starts_with("brand/")));
        for retired in ["tile", "face", "features"] {
            assert!(load(&format!("brand/leon-mark-{retired}.svg")).is_none());
        }
        assert!(load("brand/zavu-mark.svg").is_none(), "the product is Leon");
    }

    #[test]
    fn the_icon_source_is_the_glare_and_its_accent_is_one_variable_of_the_generator() {
        let source = std::fs::read_to_string(app_dir().join("assets/brand/app-icon.svg")).unwrap();
        assert!(
            source.contains("fill-rule=\"evenodd\""),
            "the glare is a cut silhouette"
        );
        let script = std::fs::read_to_string(app_dir().join("../../scripts/generate-icons.sh"))
            .expect("the generator");
        assert!(script.contains("ICON_ACCENT="));
    }

    #[test]
    fn every_agent_has_a_bundled_single_path_mark_in_the_current_colour() {
        for agent in leon_core::AgentKind::ALL {
            let path = agent_mark(agent);
            let bytes = load(path).unwrap_or_else(|| panic!("{path} is not bundled"));
            let svg = std::str::from_utf8(bytes).unwrap();
            assert!(svg.contains("fill=\"currentColor\""), "{path}");
            assert!(!svg.contains('#'), "{path} carries a colour of its own");
            assert_eq!(svg.matches("<path").count(), 1, "{path} is one path");
            assert!(paths().any(|name| name == path));
        }
    }

    #[test]
    fn an_unknown_asset_is_not_found() {
        assert!(load("brand/other.svg").is_none());
    }

    #[test]
    fn every_font_file_is_a_truetype_font() {
        let fonts = fonts();
        assert_eq!(fonts.len(), 12);
        for bytes in fonts {
            // 0x00010000 is the TrueType sfnt version.
            assert_eq!(&bytes[..4], &[0, 1, 0, 0]);
        }
    }

    #[test]
    fn the_licences_ship_beside_the_fonts() {
        for licence in [
            include_str!("../assets/fonts/SpaceGrotesk-OFL.txt"),
            include_str!("../assets/fonts/GeistMono-OFL.txt"),
            include_str!("../assets/fonts/Inter-LICENSE.txt"),
            include_str!("../assets/fonts/JetBrainsMono-OFL.txt"),
        ] {
            assert!(licence.contains("SIL OPEN FONT LICENSE"));
        }
    }
}

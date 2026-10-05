//! Writes the animated SVG files of the brand.
//!
//! ```text
//! cargo run -p leon-mark --example export-svg [OUT_DIR]
//! ```
//!
//! The files are `mark-animated.svg` (transparent, in the accent) and
//! `app-icon-animated.svg` (on the ink tile), written to `OUT_DIR`, by
//! default `brand/logo/final` of this repository. The accent and the ink are
//! the ones of the `app-icon.svg` that sits there: the brand's colours stay in
//! one place, and a test of this crate fails when a committed file differs
//! from what this program writes.

use std::path::PathBuf;

use leon_mark::svg::{icon_animated, mark_animated};

/// The value of the `fill` attribute of the first `<tag ...>` of an SVG text.
fn fill_of(svg: &str, tag: &str) -> String {
    let start = svg
        .find(&format!("<{tag} "))
        .unwrap_or_else(|| panic!("no <{tag}> in app-icon.svg"));
    let element = &svg[start..svg[start..].find('>').map_or(svg.len(), |end| start + end)];
    let at = element.find("fill=\"").expect("the element has a fill") + 6;
    element[at..]
        .split('"')
        .next()
        .expect("the fill is quoted")
        .to_owned()
}

fn main() -> std::io::Result<()> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../brand/logo/final")
        });
    let icon = std::fs::read_to_string(out.join("app-icon.svg"))?;
    let (accent, ink) = (fill_of(&icon, "path"), fill_of(&icon, "rect"));

    std::fs::write(out.join("mark-animated.svg"), mark_animated(&accent))?;
    std::fs::write(
        out.join("app-icon-animated.svg"),
        icon_animated(&accent, &ink),
    )?;
    println!(
        "wrote mark-animated.svg and app-icon-animated.svg to {}",
        out.display()
    );
    Ok(())
}

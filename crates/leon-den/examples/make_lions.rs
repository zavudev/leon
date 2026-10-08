//! Makes the lion sheets from the character sheets.
//!
//! ```text
//! cargo run -p leon-den --example make_lions [-- --preview DIR]
//! ```
//!
//! It reads `assets/source/characters/char_N.png` and writes
//! `assets/lions/lion_B_S.png` (body `B` in style `S`), `assets/lions/cub.png` and the props of
//! `assets/props/`, all drawn by [`leon_den::atelier`]. The files are
//! committed: run this again only after changing a drawing of the workshop,
//! and a test will say so if you forget. With `--preview DIR` it also writes
//! every sheet ten times larger into `DIR`, to look at.

use std::path::{Path, PathBuf};

use leon_den::atelier;
use leon_den::bitmap::Bitmap;

fn main() -> std::io::Result<()> {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let mut args = std::env::args().skip(1);
    let preview: Option<PathBuf> = match (args.next().as_deref(), args.next()) {
        (Some("--preview"), Some(dir)) => Some(dir.into()),
        _ => None,
    };
    let write = |path: PathBuf, bitmap: &Bitmap| -> std::io::Result<()> {
        std::fs::write(&path, bitmap.to_png())?;
        if let Some(dir) = &preview {
            let name = path.file_name().expect("a file name");
            let mut large = Bitmap::new(bitmap.w, bitmap.h);
            large.fill(0, 0, bitmap.w, bitmap.h, [70, 70, 90, 255]);
            large.draw(bitmap, 0, 0);
            std::fs::write(dir.join(name), large.scaled(10).to_png())?;
        }
        println!("wrote {}", path.display());
        Ok(())
    };
    for (name, bitmap) in atelier::everything(|index| {
        let path = assets.join(format!("source/characters/char_{index}.png"));
        Bitmap::from_png(&std::fs::read(path).expect("the source sheet is in the repository"))
    }) {
        write(assets.join(name), &bitmap)?;
    }
    Ok(())
}

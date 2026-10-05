//! Build script: on Windows, embeds the application icon in the executable.
//!
//! The window class takes its icon from the executable's icon resource number
//! 1, which is what `winresource` writes the icon as. Elsewhere it does
//! nothing: macOS takes the icon from the bundle (`packaging/macos`) and Linux
//! from the desktop entry (`assets/linux`). The resource compiler runs on a
//! Windows host, which is where Windows builds are made.

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/app-icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    embed_windows_icon();
}

#[cfg(windows)]
fn embed_windows_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/icons/app-icon.ico");
    resource.set("ProductName", "Leon");
    resource.set("FileDescription", "Leon");
    if let Err(error) = resource.compile() {
        println!("cargo:warning=the icon could not be embedded: {error}");
    }
}

#[cfg(not(windows))]
fn embed_windows_icon() {}

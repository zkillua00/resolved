use std::{env, error::Error, path::Path};

fn main() {
    // The macOS .app bundle provides Info.plist, icons, and signing through
    // scripts/bundle-macos.sh, so only Windows needs build-time resources.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_resources().expect("embedding Windows resources must succeed");
    }
}

fn embed_windows_resources() -> Result<(), Box<dyn Error>> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")?;
    let icon = Path::new(&manifest_dir)
        .join("assets")
        .join("windows")
        .join("resolved.ico");

    let mut resources = winresource::WindowsResource::new();
    if icon.is_file() {
        resources.set_icon(icon.to_str().expect("icon path is valid UTF-8"));
    }
    // Version and product metadata for Explorer and installers.
    resources.set("FileDescription", "Resolved API workbench");
    resources.set("ProductName", "Resolved");

    resources.compile()?;
    Ok(())
}

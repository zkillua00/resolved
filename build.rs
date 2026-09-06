#[cfg(target_os = "windows")]
use std::{env, error::Error, path::Path};

fn main() {
    println!("cargo:rerun-if-env-changed=RESOLVED_BUILD_VERSION");
    let version = std::env::var("RESOLVED_BUILD_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=RESOLVED_BUILD_VERSION={version}");
    // The macOS .app bundle provides Info.plist, icons, and signing through
    // scripts/bundle-macos.sh, so only Windows needs build-time resources.
    #[cfg(target_os = "windows")]
    embed_windows_resources().expect("embedding Windows resources must succeed");
}

#[cfg(target_os = "windows")]
fn embed_windows_resources() -> Result<(), Box<dyn Error>> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")?;
    let icon = Path::new(&manifest_dir)
        .join("assets")
        .join("windows")
        .join("resolved.ico");

    let mut resources = winresource::WindowsResource::new();
    println!("cargo:rerun-if-changed={}", icon.display());
    if icon.is_file() {
        resources.set_icon(icon.to_str().expect("icon path is valid UTF-8"));
    }
    // Version and product metadata for Explorer and installers.
    resources.set("FileDescription", "Resolved API workbench");
    resources.set("ProductName", "Resolved");
    if let Ok(version) = env::var("RESOLVED_BUILD_VERSION") {
        resources.set("FileVersion", &version);
        resources.set("ProductVersion", &version);
    }

    resources.compile()?;
    Ok(())
}

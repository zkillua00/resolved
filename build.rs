#[cfg(target_os = "windows")]
use std::{env, error::Error, path::Path};

fn main() {
    println!("cargo:rerun-if-env-changed=RESOLVED_BUILD_VERSION");
    let version = std::env::var("RESOLVED_BUILD_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=RESOLVED_BUILD_VERSION={version}");
    println!("cargo:rerun-if-env-changed=RESOLVED_BUILD_COMMIT");
    // Ask Git for paths: linked worktrees keep HEAD and shared refs separately.
    // Watching refs also catches loose refs created from previously packed refs.
    for name in ["HEAD", "refs", "packed-refs"] {
        if let Some(path) = git_output(&["rev-parse", "--git-path", name]) {
            if std::path::Path::new(&path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
    let commit = std::env::var("RESOLVED_BUILD_COMMIT")
        .ok()
        .or_else(|| git_output(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "Unknown".to_owned());
    println!("cargo:rustc-env=RESOLVED_BUILD_COMMIT={commit}");
    // The macOS .app bundle provides Info.plist, icons, and signing through
    // scripts/bundle-macos.sh, so only Windows needs build-time resources.
    #[cfg(target_os = "windows")]
    embed_windows_resources().expect("embedding Windows resources must succeed");
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
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

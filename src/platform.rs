//! Runtime platform decisions used across Resolved.
//!
//! macOS keeps the bundle-gated, Keychain-backed model. Windows requires MSIX
//! package identity at launch, the source of app identity for WebView2 data
//! directories and packaged fail-fast behavior; title-bar decoration lives in
//! `src/app/windows_controls.rs`.

#[cfg(target_os = "macos")]
use std::path::Path;

#[cfg(target_os = "windows")]
mod windows_probe {
    use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, WIN32_ERROR};
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

    /// GetCurrentPackageFullName reports the required buffer length when the
    /// process has package identity, and APPMODEL_ERROR_NO_PACKAGE when it
    /// does not — so a lone size query is a complete identity probe.
    pub(super) fn has_package_identity() -> bool {
        let mut length: u32 = 0;
        // SAFETY: `length` is a valid, writable u32 for the duration of the
        // call; no package-name buffer is required for the size probe.
        let status: WIN32_ERROR = unsafe { GetCurrentPackageFullName(&mut length, None) };
        status == WIN32_ERROR(0) || status == ERROR_INSUFFICIENT_BUFFER
    }
}

/// Returns the human-readable reason the current launch is unsupported, if
/// it is.
///
/// * macOS builds must run from an `.app` bundle so Info.plist, the icon, and
///   provisioning metadata are present.
/// * Windows builds require MSIX package identity; a bare exe refuses to
///   start and `scripts/package-msix.ps1` produces a launchable package.
#[cfg(target_os = "macos")]
pub fn launch_blocker() -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve the executable path: {error}"))?;
    if containing_app_bundle(&executable).is_some() {
        Ok(())
    } else {
        Err(
            "Resolved must run from its macOS application bundle. Use scripts/cargo.sh run."
                .to_owned(),
        )
    }
}

#[cfg(target_os = "windows")]
pub fn launch_blocker() -> Result<(), String> {
    if windows_probe::has_package_identity() {
        Ok(())
    } else {
        Err(
            "Resolved must run as a packaged (MSIX) application on Windows. Use \
             scripts/cargo.ps1 run to build and launch the packaged app."
                .to_owned(),
        )
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn launch_blocker() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn containing_app_bundle(executable: &Path) -> Option<&Path> {
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    (bundle.extension()? == "app").then_some(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn recognizes_only_executables_inside_macos_app_bundles() {
        let bundled = Path::new("/tmp/Resolved.app/Contents/MacOS/api-tester");
        assert_eq!(
            containing_app_bundle(bundled),
            Some(Path::new("/tmp/Resolved.app"))
        );
        assert!(containing_app_bundle(Path::new("/tmp/target/debug/api-tester")).is_none());
        assert!(
            containing_app_bundle(Path::new("/tmp/Resolved/Contents/MacOS/api-tester")).is_none()
        );
    }
}

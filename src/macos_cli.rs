//! User-initiated macOS CLI setup. Never replace an existing destination.

use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
enum InstallOutcome {
    Installed,
    AlreadyInstalled,
}

pub(crate) fn install_cli() -> Result<String, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the running Resolved executable: {error}"))?;
    let home = dirs::home_dir().ok_or("Could not locate your home directory.")?;
    let destination = home.join(".local/bin/resolved");
    let outcome = install_symlink(&executable, &destination)?;
    Ok(match outcome {
        InstallOutcome::Installed => format!(
            "Installed {} → {}. Add ~/.local/bin to your shell's PATH if needed.",
            destination.display(),
            executable.display()
        ),
        InstallOutcome::AlreadyInstalled => format!(
            "{} already points to this Resolved executable.",
            destination.display()
        ),
    })
}

pub(crate) fn mcp_config() -> Result<String, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the running Resolved executable: {error}"))?;
    config_for_executable(&executable)
}

fn config_for_executable(executable: &Path) -> Result<String, String> {
    if !executable.is_absolute() {
        return Err("The Resolved executable path must be absolute.".to_owned());
    }
    let command = executable
        .to_str()
        .ok_or("The Resolved executable path is not valid UTF-8 and cannot be used in MCP JSON.")?;
    serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "resolved": {
                "command": command,
                "args": ["--mcp"]
            }
        }
    }))
    .map_err(|error| format!("Could not generate MCP configuration: {error}"))
}

fn install_symlink(executable: &Path, destination: &Path) -> Result<InstallOutcome, String> {
    if !executable.is_absolute() || !destination.is_absolute() {
        return Err(
            "The Resolved executable and CLI installation paths must be absolute.".to_owned(),
        );
    }
    let parent = destination
        .parent()
        .ok_or("The CLI installation path has no parent directory.")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;

    // Creating the link itself is atomic and refuses any existing directory entry,
    // including dangling symlinks. Do not use exists(), remove_file(), or rename().
    match std::os::unix::fs::symlink(executable, destination) {
        Ok(()) => Ok(InstallOutcome::Installed),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::read_link(destination).ok().as_deref() == Some(executable) {
                Ok(InstallOutcome::AlreadyInstalled)
            } else {
                Err(format!(
                    "Refusing to replace {}: a different file, directory, or symlink already exists. Move it yourself before installing the Resolved CLI.",
                    destination.display()
                ))
            }
        }
        Err(error) => Err(format!(
            "Could not install the Resolved CLI at {}: {error}",
            destination.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_absolute_symlink_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("Resolved.app/Contents/MacOS/Resolved");
        let destination = temp.path().join("home/.local/bin/resolved");

        assert_eq!(
            install_symlink(&executable, &destination).unwrap(),
            InstallOutcome::Installed
        );
        assert_eq!(std::fs::read_link(&destination).unwrap(), executable);
        assert_eq!(
            install_symlink(&executable, &destination).unwrap(),
            InstallOutcome::AlreadyInstalled
        );
    }

    #[test]
    fn preserves_existing_file_directory_and_dangling_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("Resolved");
        let file = temp.path().join("file");
        let directory = temp.path().join("directory");
        let link = temp.path().join("link");
        let other_target = temp.path().join("missing-other-executable");
        std::fs::write(&file, "keep me").unwrap();
        std::fs::create_dir(&directory).unwrap();
        std::os::unix::fs::symlink(&other_target, &link).unwrap();

        for destination in [&file, &directory, &link] {
            let error = install_symlink(&executable, destination).unwrap_err();
            assert!(error.contains("Refusing to replace"), "{error}");
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep me");
        assert!(directory.is_dir());
        assert_eq!(std::fs::read_link(&link).unwrap(), other_target);
    }

    #[test]
    fn reports_parent_creation_failure() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("not-a-directory");
        std::fs::write(&parent, "keep me").unwrap();
        let error =
            install_symlink(&temp.path().join("Resolved"), &parent.join("resolved")).unwrap_err();
        assert!(error.contains("Could not create"), "{error}");
        assert_eq!(std::fs::read_to_string(parent).unwrap(), "keep me");
    }

    #[test]
    fn config_uses_absolute_executable_and_escapes_json() {
        let executable = Path::new("/Applications/Resolved \"Dev\".app/Contents/MacOS/Resolved");
        let config: serde_json::Value =
            serde_json::from_str(&config_for_executable(executable).unwrap()).unwrap();
        assert_eq!(
            config["mcpServers"]["resolved"]["command"],
            executable.to_str().unwrap()
        );
        assert_eq!(
            config["mcpServers"]["resolved"]["args"],
            serde_json::json!(["--mcp"])
        );
    }

    #[test]
    fn rejects_relative_paths_before_installation() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("not-created/bin/resolved");
        assert!(install_symlink(Path::new("Resolved"), &destination).is_err());
        assert!(!destination.parent().unwrap().exists());
        assert!(config_for_executable(Path::new("Resolved")).is_err());
    }

    #[test]
    fn config_rejects_non_utf8_instead_of_corrupting_command() {
        use std::os::unix::ffi::OsStrExt;
        let executable = Path::new(std::ffi::OsStr::from_bytes(b"/Applications/\xff/Resolved"));
        assert!(
            config_for_executable(executable)
                .unwrap_err()
                .contains("UTF-8")
        );
    }
}

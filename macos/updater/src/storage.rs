use crate::download::{Failure, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};
use tempfile::TempDir;

fn failure() -> Failure {
    Failure::new("storage", "Cannot use a safe private updater cache.")
}

fn safe_metadata(owner: u32, mode: u32, uid: u32, user_only: bool) -> bool {
    (owner == uid || (!user_only && owner == 0)) && mode & 0o022 == 0
}

fn validate_directory(path: &Path, user_only: bool) -> Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| failure())?;
    let metadata = file.metadata().map_err(|_| failure())?;
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir() || !safe_metadata(metadata.uid(), metadata.mode(), uid, user_only) {
        return Err(failure());
    }
    Ok(())
}

fn cache(home: &Path) -> Result<PathBuf> {
    if !home.is_absolute()
        || home
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(failure());
    }
    let mut path = PathBuf::new();
    for component in home.components() {
        path.push(component.as_os_str());
        validate_directory(&path, path == home)?;
    }
    for name in ["Library", "Caches", "dev.apitester.desktop", "updates"] {
        path.push(name);
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(failure()),
        }
        validate_directory(&path, true)?;
    }
    if fs::metadata(&path).map_err(|_| failure())?.mode() & 0o077 != 0 {
        return Err(failure());
    }
    Ok(path)
}

pub struct Download {
    directory: Option<TempDir>,
    file: Option<File>,
    path: PathBuf,
}

impl Download {
    pub fn create() -> Result<Self> {
        let home = std::env::var_os("HOME").ok_or_else(failure)?;
        Self::in_cache(&cache(Path::new(&home))?)
    }
    pub(crate) fn in_cache(root: &Path) -> Result<Self> {
        validate_directory(root, true)?;
        let directory = tempfile::Builder::new()
            .prefix("download-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(root)
            .map_err(|_| failure())?;
        let path = directory.path().join("archive.zip.partial");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|_| failure())?;
        Ok(Self {
            directory: Some(directory),
            file: Some(file),
            path,
        })
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.file
            .as_mut()
            .ok_or_else(failure)?
            .write_all(bytes)
            .map_err(|_| failure())
    }
    pub fn finish(&mut self, name: &str) -> Result<()> {
        // Shared policy selects an exact release ZIP name. Keep the storage
        // primitive independently incapable of writing outside its directory.
        if !name.ends_with(".zip")
            || name.len() > 255
            || name.contains(['/', '\\', '\0'])
            || Path::new(name).file_name() != Some(std::ffi::OsStr::new(name))
        {
            return Err(failure());
        }
        let mut file = self.file.take().ok_or_else(failure)?;
        file.flush()
            .and_then(|_| file.sync_all())
            .map_err(|_| failure())?;
        drop(file);
        let complete = self.path.with_file_name(name);
        fs::rename(&self.path, &complete).map_err(|_| failure())?;
        self.path = complete;
        Ok(())
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn retain(mut self) {
        if let Some(directory) = self.directory.take() {
            let _ = directory.keep();
        }
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        self.file.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn foreign_owners_and_writable_directories_are_rejected() {
        assert!(safe_metadata(501, 0o700, 501, true));
        assert!(safe_metadata(0, 0o755, 501, false));
        assert!(!safe_metadata(0, 0o700, 501, true));
        assert!(!safe_metadata(502, 0o700, 501, false));
        assert!(!safe_metadata(501, 0o722, 501, true));
        assert!(!safe_metadata(501, 0o770, 501, true));
    }

    #[test]
    fn cleanup_permissions_and_explicit_retention() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut file = Download::in_cache(root.path()).unwrap();
        let directory = file.path().parent().unwrap().to_owned();
        assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(file.path()).unwrap().mode() & 0o777, 0o600);
        file.write(b"partial").unwrap();
        drop(file);
        assert!(!directory.exists());
        let mut file = Download::in_cache(root.path()).unwrap();
        file.write(b"complete").unwrap();
        file.finish("Resolved.zip").unwrap();
        let path = file.path().to_owned();
        file.retain();
        assert_eq!(fs::read(path).unwrap(), b"complete");
    }
    #[test]
    fn unsafe_directories_and_relative_home_rejected() {
        let root = tempfile::tempdir().unwrap();
        assert!(cache(Path::new("relative")).is_err());
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(Download::in_cache(root.path()).is_err());
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let link = root.path().join("link");
        symlink(root.path(), &link).unwrap();
        assert!(Download::in_cache(&link).is_err());
    }
}

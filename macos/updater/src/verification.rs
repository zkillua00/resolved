//! A verification marker is never authority: every install rehashes a fresh
//! snapshot, extracts into private destination-volume staging, and checks trust.
use crate::{
    download::{Failure, Result},
    native::{self, BundleIdentity, Host},
};
use resolved_release::UpdateArtifact;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};
use tokio::sync::watch;

pub struct Prepared {
    pub host: Host,
    pub bundle: PathBuf,
    pub identity: BundleIdentity,
}

fn failure() -> Failure {
    Failure::new(
        "verification",
        "The cached update is unsafe, changed, or incompatible. Download it again.",
    )
}

fn cancelled(cancel: &watch::Receiver<bool>) -> Result<()> {
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Err(Failure::new("cancelled", "Update verification cancelled."));
    }
    Ok(())
}

fn validate_offer(artifact: &UpdateArtifact, installed: &str) -> Result<()> {
    // Reuse the one feed/architecture/version rule, even for a cached approval.
    let feed = serde_json::json!({
        "version": artifact.version,
        "resolved": { "macos": { (artifact.arch.clone()): [{
            "name": artifact.name, "url": artifact.url,
            "size": artifact.size, "sha256": artifact.sha256
        }] } }
    });
    let check = resolved_release::check_feed(
        &serde_json::to_vec(&feed).map_err(|_| failure())?,
        installed,
        "macos",
        env!("RESOLVED_UPDATER_ARCH"),
    )
    .map_err(|_| failure())?;
    if check
        .macos_update(env!("RESOLVED_UPDATER_ARCH"))
        .map_err(|_| failure())?
        .as_ref()
        != Some(artifact)
    {
        return Err(failure());
    }
    Ok(())
}

fn snapshot(
    root: &Path,
    artifact: &UpdateArtifact,
    archive: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<tempfile::NamedTempFile> {
    if !archive.is_absolute()
        || archive
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        || archive.file_name().and_then(|v| v.to_str()) != Some(artifact.name.as_str())
        || archive.parent().and_then(Path::parent) != Some(root)
    {
        return Err(failure());
    }
    let directory = archive.parent().ok_or_else(failure)?;
    if !directory
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.starts_with("download-"))
    {
        return Err(failure());
    }
    let metadata = fs::symlink_metadata(directory).map_err(|_| failure())?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(failure());
    }
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(archive)
        .map_err(|_| failure())?;
    let before = source.metadata().map_err(|_| failure())?;
    if !before.is_file()
        || before.uid() != unsafe { libc::geteuid() }
        || before.mode() & 0o7777 != 0o600
        || before.nlink() != 1
        || before.len() != artifact.size
    {
        return Err(failure());
    }
    let mut copy = tempfile::Builder::new()
        .prefix(".verified-input-")
        .tempfile_in(root)
        .map_err(|_| failure())?;
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        cancelled(cancel)?;
        let count = source.read(&mut buffer).map_err(|_| failure())?;
        if count == 0 {
            break;
        }
        bytes = bytes.checked_add(count as u64).ok_or_else(failure)?;
        if bytes > artifact.size {
            return Err(failure());
        }
        hash.update(&buffer[..count]);
        copy.write_all(&buffer[..count]).map_err(|_| failure())?;
    }
    let after = source.metadata().map_err(|_| failure())?;
    if bytes != artifact.size
        || format!("{:x}", hash.finalize()) != artifact.sha256
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
    {
        return Err(failure());
    }
    copy.flush()
        .and_then(|_| copy.as_file().sync_all())
        .map_err(|_| failure())?;
    Ok(copy)
}

pub async fn prepare(
    artifact: &UpdateArtifact,
    archive: &Path,
    destination: &Path,
    cancel: &watch::Receiver<bool>,
) -> Result<Prepared> {
    cancelled(cancel)?;
    let host = native::host().await?;
    crate::install::eligible_target(&host.path)?;
    validate_offer(artifact, &host.identity.version)?;
    let root = crate::storage::cache_root()?;
    let copy = snapshot(&root, artifact, archive, cancel)?;
    cancelled(cancel)?;
    // Synchronous/cooperative extraction: dropping this future never leaves a
    // detached blocking worker writing into a directory being recovered.
    let bundle = crate::archive::extract(copy.path(), destination, || {
        *cancel.borrow() || cancel.has_changed().is_err()
    })?;
    cancelled(cancel)?;
    let identity = native::candidate(&bundle, &host, artifact).await?;
    cancelled(cancel)?;
    Ok(Prepared {
        host,
        bundle,
        identity,
    })
}

pub async fn run(
    artifact: &UpdateArtifact,
    archive: &Path,
    cancel: &watch::Receiver<bool>,
    emit: &mut impl FnMut(serde_json::Value) -> Result<()>,
) -> Result<()> {
    emit(
        serde_json::json!({"kind":"progress","phase":"checking","downloaded":0,"total":artifact.size}),
    )?;
    let root = crate::storage::cache_root()?;
    let stage = tempfile::Builder::new()
        .prefix("verification-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(root)
        .map_err(|_| failure())?;
    emit(
        serde_json::json!({"kind":"progress","phase":"verifying","downloaded":artifact.size,"total":artifact.size}),
    )?;
    let prepared = prepare(artifact, archive, stage.path(), cancel).await?;
    emit(serde_json::json!({
        "kind":"verified","artifact":artifact,"archive":archive,
        "team_id":prepared.identity.team_id, "bundle_version":prepared.identity.build,
        "installation_enabled":true
    }))?;
    // Deliberately discard this staging. Restart preparation verifies a fresh
    // snapshot on the destination filesystem; no persisted 'verified' shortcut.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact() -> UpdateArtifact {
        UpdateArtifact {
            version: "0.13.0".into(),
            arch: env!("RESOLVED_UPDATER_ARCH").into(),
            name: format!(
                "Resolved-0.13.0-macos-{}.zip",
                env!("RESOLVED_UPDATER_ARCH")
            ),
            url: format!(
                "https://github.com/zkillua00/resolved/releases/download/v0.13.0/Resolved-0.13.0-macos-{}.zip",
                env!("RESOLVED_UPDATER_ARCH")
            ),
            size: 3,
            sha256: format!("{:x}", Sha256::digest(b"abc")),
        }
    }

    #[test]
    fn cached_offer_must_be_a_strict_upgrade_and_canonical_artifact() {
        let mut value = artifact();
        assert!(validate_offer(&value, "0.12.4").is_ok());
        assert!(validate_offer(&value, "0.13.0").is_err());
        value.url = "https://example.com/update.zip".into();
        assert!(validate_offer(&value, "0.12.4").is_err());
    }

    #[test]
    fn snapshot_is_private_bounded_and_independent_of_later_cache_mutation() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let directory = root.join("download-test");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let artifact = artifact();
        let archive = directory.join(&artifact.name);
        fs::write(&archive, b"abc").unwrap();
        fs::set_permissions(&archive, fs::Permissions::from_mode(0o600)).unwrap();
        let (_sender, cancel) = watch::channel(false);
        let copy = snapshot(&root, &artifact, &archive, &cancel).unwrap();
        fs::write(&archive, b"bad").unwrap();
        assert_eq!(fs::read(copy.path()).unwrap(), b"abc");
        assert!(snapshot(&root, &artifact, &archive, &cancel).is_err());
        assert!(snapshot(&root, &artifact, Path::new("/elsewhere/file.zip"), &cancel).is_err());
    }
}

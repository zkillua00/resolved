use std::{
    fs::{self, File, OpenOptions},
    io::{self, Seek as _, SeekFrom, Write as _},
    path::Path,
};

/// Holds an advisory, process-wide lock for the application's SQLite workspace.
///
/// SQLite protects individual transactions, but this app saves domain
/// aggregates by replacement. Restricting the MVP to one process prevents two
/// stale in-memory snapshots from deleting each other's rows.
pub struct InstanceGuard {
    // The open descriptor holds the advisory lock for the guard's lifetime;
    // kept unnamed so no code path can write through it accidentally. The
    // byte contents (the owning process id) are diagnostic only.
    _file: File,
}

impl InstanceGuard {
    pub fn acquire(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }

        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;

        // Lock while the file is still empty, then record the owning process
        // id. `File::try_lock` places an advisory lock that the OS releases
        // when this handle closes or the process exits, matching the previous
        // exclusive `flock` semantics on every platform this crate supports.
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => {
                io::Error::new(io::ErrorKind::AlreadyExists, "Resolved is already running")
            }
            std::fs::TryLockError::Error(io_error) => io::Error::new(
                io_error.kind(),
                format!("could not lock the Resolved workspace: {io_error}"),
            ),
        })?;

        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_guard_can_hold_a_workspace_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("api-tester.lock");
        let first = InstanceGuard::acquire(&path).unwrap();

        let error = InstanceGuard::acquire(&path)
            .err()
            .expect("a second process guard must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);

        drop(first);
        InstanceGuard::acquire(&path).expect("dropping the guard must release the lock");
    }
}

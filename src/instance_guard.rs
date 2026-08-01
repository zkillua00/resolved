use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek as _, SeekFrom, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

/// Holds an advisory, process-wide lock for the application's SQLite workspace.
///
/// SQLite protects individual transactions, but this app saves domain
/// aggregates by replacement. Restricting the MVP to one process prevents two
/// stale in-memory snapshots from deleting each other's rows.
pub struct InstanceGuard {
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
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)?;

        // SAFETY: `file` owns a live descriptor for the duration of the call
        // and remains owned by `InstanceGuard` until the process releases it.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::WouldBlock {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Resolved is already running",
                ))
            } else {
                Err(error)
            };
        }

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

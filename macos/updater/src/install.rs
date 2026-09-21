//! Unprivileged, single-installation transaction. There is deliberately no rollback:
//! once the replacement can have executed, restoring its predecessor could corrupt
//! migrated user data. The old bundle lives until a verified new-app acknowledgement.
//!
//! A crash between mkdir and the first durable journal can leave an inert private
//! directory. We never sweep unknown staging directories to recover that tiny window.
use crate::{
    download::{Failure, Result},
    native::{self, BundleIdentity},
};
use resolved_release::UpdateArtifact;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::sync::{mpsc, watch};

const JOURNAL_LIMIT: u64 = 32 * 1024;
const READY_LIMIT: Duration = Duration::from_secs(60);
const EXIT_LIMIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug)]
pub enum Control {
    Commit,
    Cancel,
    Closed,
}

fn failure() -> Failure {
    Failure::new(
        "installation",
        "Installation could not be completed safely. Run update recovery.",
    )
}
fn cancelled() -> Failure {
    Failure::new("cancelled", "Installation cancelled before replacement.")
}
fn io<T>(value: std::io::Result<T>) -> Result<T> {
    value.map_err(|_| failure())
}
fn cstr(value: &OsStr) -> Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| failure())
}
fn uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    dev: u64,
    ino: u64,
}
impl Anchor {
    fn of(path: &Path) -> Result<Self> {
        let m = io(fs::symlink_metadata(path))?;
        if !m.is_dir() || m.uid() != uid() || m.mode() & 0o022 != 0 {
            return Err(failure());
        }
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
        })
    }
    fn value(self) -> Value {
        json!({"dev": self.dev, "ino": self.ino})
    }
    fn parse(v: &Value) -> Result<Self> {
        Ok(Self {
            dev: v["dev"].as_u64().ok_or_else(failure)?,
            ino: v["ino"].as_u64().filter(|n| *n != 0).ok_or_else(failure)?,
        })
    }
    fn check(self, path: &Path) -> Result<()> {
        if Self::of(path)? != self {
            return Err(failure());
        }
        Ok(())
    }
}

fn directory(path: &Path) -> Result<File> {
    io(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(path))
}

fn owner_policy(owner: u32, mode: u32, current: u32, leaf: bool) -> bool {
    (owner == current || (!leaf && owner == 0)) && mode & 0o022 == 0
}

fn applications_policy(path: &Path, owner: u32, group: u32, mode: u32) -> bool {
    // macOS's standard admin group is GID 80. This exception is deliberately
    // confined to the real /Applications directory, never arbitrary shared paths.
    path == Path::new("/Applications") && owner == 0 && group == 80 && mode & 0o7777 == 0o775
}

/// Every ancestor is checked, not just canonicalized (which would hide symlinks).
fn safe_path(path: &Path, leaf_owned: bool) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(failure());
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        if component.as_os_str().as_bytes() == b"AppTranslocation" {
            return Err(failure());
        }
        prefix.push(component);
        let m = io(fs::symlink_metadata(&prefix))?;
        if !m.is_dir()
            || !(owner_policy(m.uid(), m.mode(), uid(), leaf_owned && prefix == path)
                || (prefix != path && applications_policy(&prefix, m.uid(), m.gid(), m.mode())))
        {
            return Err(failure());
        }
    }
    Ok(())
}

fn mount_writable(flags: u32) -> bool {
    flags & libc::MNT_RDONLY as u32 == 0
}

fn writable_mount(parent: &Path) -> Result<()> {
    let file = directory(parent)?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(failure());
    }
    let stat = unsafe { stat.assume_init() };
    if !mount_writable(stat.f_flags) {
        return Err(failure());
    }
    // A real create is stronger than access(2), including ACL and mount policy.
    let probe = io(tempfile::Builder::new()
        .prefix(".resolved-write-")
        .tempfile_in(parent))?;
    io(probe.as_file().sync_all())?;
    Ok(())
}

pub fn eligible_target(target: &Path) -> Result<()> {
    safe_path(target, true)?;
    writable_mount(target.parent().ok_or_else(failure)?)
}

fn identity_value(identity: &BundleIdentity) -> Value {
    json!({"version": identity.version, "build": identity.build,
        "team_id": identity.team_id, "fingerprint": identity.fingerprint})
}
fn text(v: &Value, key: &str) -> Result<String> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or_else(failure)
}
fn identity(v: &Value) -> Result<BundleIdentity> {
    Ok(BundleIdentity {
        version: text(v, "version")?,
        build: text(v, "build")?,
        team_id: text(v, "team_id")?,
        fingerprint: text(v, "fingerprint")?,
    })
}

struct Store {
    _lock: File,
    root: PathBuf,
    journal: PathBuf,
    target: PathBuf,
    prefix: String,
}

impl Drop for Store {
    fn drop(&mut self) {
        // A concurrent fork can inherit the descriptor before CLOEXEC runs.
        // Release the scoped lock explicitly instead of waiting for that child.
        unsafe { libc::flock(self._lock.as_raw_fd(), libc::LOCK_UN) };
    }
}

impl Store {
    fn open(root: PathBuf, target: PathBuf) -> Result<Self> {
        if target.to_str().is_none() || !target.is_absolute() {
            return Err(failure());
        }
        let m = io(fs::symlink_metadata(&root))?;
        if !m.is_dir() || m.uid() != uid() || m.mode() & 0o077 != 0 {
            return Err(failure());
        }
        let key = format!("{:x}", Sha256::digest(target.as_os_str().as_bytes()));
        let lock = io(OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(root.join(format!("install-{key}.lock"))))?;
        let m = io(lock.metadata())?;
        if !m.is_file() || m.uid() != uid() || m.mode() & 0o077 != 0 || m.nlink() != 1 {
            return Err(failure());
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(Failure::new(
                "installation_busy",
                "Another installer is using this application.",
            ));
        }
        Ok(Self {
            _lock: lock,
            journal: root.join(format!("install-{key}.json")),
            root,
            target,
            prefix: format!(".resolved-update-{}-", &key[..16]),
        })
    }
    fn read(&self) -> Result<Option<Value>> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&self.journal)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(failure()),
        };
        let m = io(file.metadata())?;
        if !m.is_file()
            || m.uid() != uid()
            || m.mode() & 0o077 != 0
            || m.nlink() != 1
            || m.len() > JOURNAL_LIMIT
        {
            return Err(failure());
        }
        let mut data = Vec::new();
        io(file.take(JOURNAL_LIMIT + 1).read_to_end(&mut data))?;
        if data.len() as u64 > JOURNAL_LIMIT {
            return Err(failure());
        }
        let value: Value = serde_json::from_slice(&data).map_err(|_| failure())?;
        if value["schema"] != 1
            || value["target"].as_str().map(Path::new) != Some(self.target.as_path())
        {
            return Err(failure());
        }
        Ok(Some(value))
    }
    fn write(&self, journal: &Value) -> Result<()> {
        let bytes = serde_json::to_vec(journal).map_err(|_| failure())?;
        if bytes.len() as u64 > JOURNAL_LIMIT {
            return Err(failure());
        }
        let mut file = io(tempfile::Builder::new()
            .prefix(".journal-")
            .tempfile_in(&self.root))?;
        io(file.write_all(&bytes))?;
        io(file.as_file().sync_all())?;
        file.persist(&self.journal).map_err(|_| failure())?;
        io(directory(&self.root)?.sync_all())
    }
    fn phase(&self, journal: &mut Value, phase: &str) -> Result<()> {
        journal["phase"] = json!(phase);
        self.write(journal)
    }
    fn forget(&self) -> Result<()> {
        io(fs::remove_file(&self.journal))?;
        io(directory(&self.root)?.sync_all())
    }
    fn stage(&self, journal: &Value) -> Result<PathBuf> {
        let stage = PathBuf::from(text(journal, "stage")?);
        if stage.parent() != self.target.parent()
            || !stage
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|s| s.starts_with(&self.prefix) && s.len() > self.prefix.len())
            || stage
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(failure());
        }
        Anchor::parse(&journal["stage_anchor"])?.check(&stage)?;
        if io(fs::symlink_metadata(&stage))?.mode() & 0o077 != 0 {
            return Err(failure());
        }
        Ok(stage)
    }
    fn candidate(&self, journal: &Value) -> Result<PathBuf> {
        let stage = self.stage(journal)?;
        let name = text(journal, "candidate")?;
        if Path::new(&name).components().count() != 1
            || !matches!(
                Path::new(&name).components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err(failure());
        }
        Ok(stage.join(name))
    }
    fn cleanup(&self, journal: &Value) -> Result<()> {
        let stage = self.stage(journal)?;
        // remove_dir_all does not follow contained symlinks. The directory itself
        // is constrained by parent, generated prefix, owner, mode and dev/inode.
        io(fs::remove_dir_all(stage))?;
        io(directory(self.target.parent().ok_or_else(failure)?)?.sync_all())?;
        self.forget()
    }
}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidpath(pid: libc::c_int, buffer: *mut libc::c_void, buffersize: u32) -> libc::c_int;
}
fn caller_matches(pid: libc::pid_t, target: &Path) -> Result<()> {
    let mut bytes = vec![0u8; 4096];
    let len = unsafe { proc_pidpath(pid, bytes.as_mut_ptr().cast(), bytes.len() as u32) };
    if len <= 0 {
        return Err(failure());
    }
    let end = bytes.iter().position(|b| *b == 0).ok_or_else(failure)?;
    if &bytes[..end]
        != target
            .join("Contents/MacOS/api-tester")
            .as_os_str()
            .as_bytes()
    {
        return Err(failure());
    }
    Ok(())
}

/// Registration captures a process object. Never replace it with kill(pid, 0)
/// or a later registration: the numeric PID could have been recycled.
struct ParentExit {
    queue: OwnedFd,
    pid: libc::pid_t,
}
impl ParentExit {
    fn register(pid: libc::pid_t) -> Result<Self> {
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(failure());
        }
        let queue = unsafe { OwnedFd::from_raw_fd(fd) };
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(failure());
        }
        let event = libc::kevent {
            ident: pid as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        if unsafe { libc::kevent(fd, &event, 1, std::ptr::null_mut(), 0, std::ptr::null()) } != 0 {
            return Err(failure());
        }
        Ok(Self { queue, pid })
    }
    fn exited(&self) -> Result<bool> {
        let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
        let zero = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let n = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                std::ptr::null(),
                0,
                event.as_mut_ptr(),
                1,
                &zero,
            )
        };
        if n < 0 {
            return Err(failure());
        }
        if n == 0 {
            return Ok(false);
        }
        let event = unsafe { event.assume_init() };
        if event.ident != self.pid as libc::uintptr_t
            || event.filter != libc::EVFILT_PROC
            || event.flags & libc::EV_ERROR != 0
            || event.fflags & libc::NOTE_EXIT == 0
        {
            return Err(failure());
        }
        Ok(true)
    }
}

fn preparation_control(control: Option<Control>) -> Failure {
    match control {
        Some(Control::Commit) => {
            Failure::new("protocol", "Commit arrived before installation readiness.")
        }
        _ => cancelled(),
    }
}
async fn wait_commit(controls: &mut mpsc::Receiver<Control>) -> Result<()> {
    wait_commit_for(controls, READY_LIMIT).await
}
async fn wait_commit_for(controls: &mut mpsc::Receiver<Control>, limit: Duration) -> Result<()> {
    match tokio::time::timeout(limit, controls.recv()).await {
        Ok(Some(Control::Commit)) => Ok(()),
        _ => Err(cancelled()),
    }
}
#[derive(Default)]
struct CommittedState {
    clean_closed: bool,
    parent_exited: bool,
    cancelled: bool,
    swapped: bool,
}
impl CommittedState {
    fn control(&mut self, control: Option<Control>) {
        match control {
            Some(Control::Closed) if !self.clean_closed => self.clean_closed = true,
            Some(_) => self.cancelled = true,
            None if !self.clean_closed => self.cancelled = true,
            None => {}
        }
    }
    fn check(
        &mut self,
        controls: &mut mpsc::Receiver<Control>,
        cancel: &watch::Receiver<bool>,
    ) -> Result<()> {
        self.cancelled |= *cancel.borrow();
        loop {
            if self.cancelled {
                return Err(cancelled());
            }
            match controls.try_recv() {
                Ok(control) => self.control(Some(control)),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.control(None);
                    break;
                }
            }
        }
        self.cancelled |= *cancel.borrow();
        if self.cancelled {
            Err(cancelled())
        } else {
            Ok(())
        }
    }
}

async fn wait_exit(
    watcher: &ParentExit,
    controls: &mut mpsc::Receiver<Control>,
    cancel: &watch::Receiver<bool>,
    state: &mut CommittedState,
) -> Result<()> {
    wait_exit_for(watcher, controls, cancel, state, EXIT_LIMIT).await
}
async fn wait_exit_for(
    watcher: &ParentExit,
    controls: &mut mpsc::Receiver<Control>,
    cancel: &watch::Receiver<bool>,
    state: &mut CommittedState,
    limit: Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        // NOTE_EXIT is one-shot. Remember it independently of stdin framing and
        // cancellation so post-exit failures can safely restart the old app.
        if !state.parent_exited {
            state.parent_exited = watcher.exited()?;
        }
        state.check(controls, cancel)?;
        if state.parent_exited && state.clean_closed {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(cancelled());
        }
        tokio::select! {
            biased;
            control = controls.recv(), if !state.clean_closed => state.control(control),
            _ = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
    }
}

fn sync_tree(path: &Path) -> Result<()> {
    let m = io(fs::symlink_metadata(path))?;
    if m.file_type().is_symlink() {
        return Ok(());
    }
    if m.is_dir() {
        for entry in io(fs::read_dir(path))? {
            sync_tree(&io(entry)?.path())?;
        }
        io(directory(path)?.sync_all())
    } else if m.is_file() {
        let file = io(OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path))?;
        io(file.sync_all())
    } else {
        Err(failure())
    }
}

unsafe extern "C" {
    fn renameatx_np(
        fromfd: libc::c_int,
        from: *const libc::c_char,
        tofd: libc::c_int,
        to: *const libc::c_char,
        flags: libc::c_uint,
    ) -> libc::c_int;
}
fn exchange(parent: &File, target: &OsStr, stage: &File, candidate: &OsStr) -> Result<()> {
    let target = cstr(target)?;
    let candidate = cstr(candidate)?;
    // RENAME_SWAP is Darwin's 0x00000002; no non-atomic fallback is permitted.
    if unsafe {
        renameatx_np(
            parent.as_raw_fd(),
            target.as_ptr(),
            stage.as_raw_fd(),
            candidate.as_ptr(),
            0x00000002,
        )
    } != 0
    {
        return Err(failure());
    }
    Ok(())
}

fn entry_anchor(directory: &File, name: &OsStr, expected: Anchor) -> Result<()> {
    let name = cstr(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(failure());
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_dev as u64 != expected.dev
        || stat.st_ino != expected.ino
        || stat.st_uid != uid()
        || stat.st_mode & libc::S_IFMT != libc::S_IFDIR
    {
        return Err(failure());
    }
    Ok(())
}

async fn launch(target: &Path) -> Result<()> {
    let mut child = tokio::process::Command::new("/usr/bin/open")
        .arg("-n")
        .arg(target)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| failure())?;
    match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        _ => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(failure())
        }
    }
}

async fn validate(path: &Path, expected: &BundleIdentity, team: &str) -> Result<()> {
    if expected.team_id != team {
        return Err(failure());
    }
    native::validate_identity(path, expected).await?;
    if native::fingerprint(path)? != expected.fingerprint {
        return Err(failure());
    }
    Ok(())
}

async fn transaction(
    store: &Store,
    journal: &mut Value,
    watcher: &ParentExit,
    controls: &mut mpsc::Receiver<Control>,
    cancel: &watch::Receiver<bool>,
    state: &mut CommittedState,
    old: &BundleIdentity,
    new: &BundleIdentity,
) -> Result<()> {
    store.phase(journal, "committed")?;
    wait_exit(watcher, controls, cancel, state).await?;
    store.phase(journal, "parent_exited")?;
    let parent_path = store.target.parent().ok_or_else(failure)?;
    safe_path(&store.target, true)?;
    writable_mount(parent_path)?;
    let stage = store.stage(journal)?;
    let candidate = store.candidate(journal)?;
    let parent = directory(parent_path)?;
    let stage_fd = directory(&stage)?;
    validate(&store.target, old, &old.team_id).await?;
    validate(&candidate, new, &old.team_id).await?;
    Anchor::parse(&journal["old_anchor"])?.check(&store.target)?;
    Anchor::parse(&journal["new_anchor"])?.check(&candidate)?;
    Anchor::parse(&journal["stage_anchor"])?.check(&stage)?;
    let pm = io(parent.metadata())?;
    if pm.dev()
        != journal["parent_anchor"]["dev"]
            .as_u64()
            .ok_or_else(failure)?
        || pm.ino()
            != journal["parent_anchor"]["ino"]
                .as_u64()
                .ok_or_else(failure)?
    {
        return Err(failure());
    }
    // Revalidate cancellation after slow signature checks and before the boundary.
    state.check(controls, cancel)?;
    store.phase(journal, "swap_intent")?;
    let sm = io(stage_fd.metadata())?;
    let expected_stage = Anchor::parse(&journal["stage_anchor"])?;
    if sm.dev() != expected_stage.dev || sm.ino() != expected_stage.ino {
        return Err(failure());
    }
    entry_anchor(
        &parent,
        store.target.file_name().ok_or_else(failure)?,
        Anchor::parse(&journal["old_anchor"])?,
    )?;
    entry_anchor(
        &stage_fd,
        candidate.file_name().ok_or_else(failure)?,
        Anchor::parse(&journal["new_anchor"])?,
    )?;
    state.check(controls, cancel)?;
    if exchange(
        &parent,
        store.target.file_name().ok_or_else(failure)?,
        &stage_fd,
        candidate.file_name().ok_or_else(failure)?,
    )
    .is_err()
    {
        store.phase(journal, "swap_failed")?;
        return Err(failure());
    }
    state.swapped = true;
    // From here onward EVERY failure retains the old backup. Recovery determines
    // actual identities even if the next durable journal write never happens.
    io(parent.sync_all())?;
    io(stage_fd.sync_all())?;
    store.phase(journal, "swapped")?;
    store.phase(journal, "launch_intent")?;
    let launched = launch(&store.target).await.is_ok();
    store.phase(
        journal,
        if launched {
            "launched"
        } else {
            "launch_failed"
        },
    )?;
    Ok(())
}

fn old_launch_intent(store: &Store, journal: &mut Value, state: &CommittedState) -> Result<()> {
    if !state.parent_exited || state.swapped {
        return Err(failure());
    }
    Anchor::parse(&journal["old_anchor"])?.check(&store.target)?;
    store.phase(journal, "old_launch_intent")?;
    Anchor::parse(&journal["old_anchor"])?.check(&store.target)
}

async fn post_commit_failure(
    store: &Store,
    journal: &mut Value,
    state: &CommittedState,
    old: &BundleIdentity,
) -> Result<()> {
    journal["outcome"] = json!("recovery_required");
    store.write(journal)?;
    if state.swapped {
        return Ok(()); // New code could have run: never launch or restore old code.
    }
    if state.parent_exited {
        // All post-exit pre-swap errors share this path, including cancellation,
        // final trust/anchor failures and swap errors. Never launch on JSON flags.
        store.phase(journal, "post_exit_failed")?;
        safe_path(&store.target, true)?;
        Anchor::parse(&journal["old_anchor"])?.check(&store.target)?;
        validate(&store.target, old, &old.team_id).await?;
        old_launch_intent(store, journal, state)?;
        let launched = launch(&store.target).await.is_ok();
        store.phase(
            journal,
            if launched {
                "old_launched"
            } else {
                "old_launch_failed"
            },
        )?;
        // Retain journal/stage so old-app recovery can report cancellation.
    } else {
        Anchor::parse(&journal["old_anchor"])?.check(&store.target)?;
        validate(&store.target, old, &old.team_id).await?;
        store.cleanup(journal)?;
    }
    Ok(())
}

pub async fn run(
    artifact: &UpdateArtifact,
    archive: &Path,
    mut controls: mpsc::Receiver<Control>,
    mut cancel: watch::Receiver<bool>,
    emit: &mut impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    let target = native::host_path()?;
    safe_path(&target, true)?;
    let parent = target.parent().ok_or_else(failure)?;
    writable_mount(parent)?;
    let store = Store::open(crate::storage::cache_root()?, target.clone())?;
    if store.read()?.is_some() {
        return Err(Failure::new(
            "recovery_required",
            "A previous update needs recovery first.",
        ));
    }
    let pid = unsafe { libc::getppid() };
    // Register first, then prove the *current direct parent* path. A process that
    // exits in this interval cannot be mistaken for a recycled PID.
    let watcher = ParentExit::register(pid)?;
    caller_matches(pid, &target)?;
    if unsafe { libc::getppid() } != pid || watcher.exited()? {
        return Err(failure());
    }
    let host = native::host().await?;
    if host.path != target {
        return Err(failure());
    }
    native::validate_running_host(pid as u32, &host).await?;
    if unsafe { libc::getppid() } != pid || watcher.exited()? {
        return Err(failure());
    }
    let old_anchor = Anchor::of(&target)?;
    let parent_metadata = io(directory(parent)?.metadata())?;
    let stage = io(tempfile::Builder::new()
        .prefix(&store.prefix)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent))?;
    let stage_anchor = Anchor::of(stage.path())?;
    if stage_anchor.dev != old_anchor.dev {
        return Err(failure());
    }
    let mut journal = json!({
        "schema": 1, "target": target, "stage": stage.path(),
        "stage_anchor": stage_anchor.value(), "old_anchor": old_anchor.value(),
        "parent_anchor": {"dev": parent_metadata.dev(), "ino": parent_metadata.ino()},
        "old": identity_value(&host.identity), "artifact": artifact, "phase": "preparing",
    });
    store.write(&journal)?;
    // Journal now owns cleanup. RAII must not unlink an old running helper after swap.
    let stage = stage.keep();
    let prepared = async {
        if *cancel.borrow() { return Err(cancelled()); }
        let verification_cancel = cancel.clone();
        let cancellation = async {
            loop {
                if *cancel.borrow_and_update() || cancel.changed().await.is_err() { break; }
            }
        };
        tokio::select! {
            biased;
            control = controls.recv() => Err(preparation_control(control)),
            _ = cancellation => Err(cancelled()),
            result = crate::verification::prepare(artifact, archive, &stage, &verification_cancel) => result,
        }
    }.await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = store.cleanup(&journal);
            return Err(error);
        }
    };
    let ready = async {
        if prepared.host.path != target
            || prepared.host.identity.fingerprint != host.identity.fingerprint
            || prepared.identity.team_id != host.identity.team_id
            || prepared.bundle.parent() != Some(stage.as_path())
        {
            return Err(failure());
        }
        journal["candidate"] = json!(
            prepared
                .bundle
                .file_name()
                .and_then(OsStr::to_str)
                .ok_or_else(failure)?
        );
        journal["new"] = identity_value(&prepared.identity);
        journal["new_anchor"] = Anchor::of(&prepared.bundle)?.value();
        sync_tree(&stage)?;
        old_anchor.check(&target)?;
        stage_anchor.check(&stage)?;
        validate(&target, &host.identity, &host.identity.team_id).await?;
        validate(&prepared.bundle, &prepared.identity, &host.identity.team_id).await?;
        if *cancel.borrow() {
            return Err(cancelled());
        }
        match controls.try_recv() {
            Ok(control) => return Err(preparation_control(Some(control))),
            Err(mpsc::error::TryRecvError::Disconnected) => return Err(cancelled()),
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
        if watcher.exited()? {
            return Err(cancelled());
        }
        store.phase(&mut journal, "ready")?;
        emit(json!({"kind": "install_ready", "artifact": artifact, "installation_enabled": true}))?;
        wait_commit(&mut controls).await
    }
    .await;
    if let Err(error) = ready {
        let _ = store.cleanup(&journal);
        return Err(error);
    }
    // No output/error is required after accepting Commit. Even journal I/O failure
    // is recovered by inspecting the preexisting intent and actual on-disk anchors.
    let mut state = CommittedState::default();
    if transaction(
        &store,
        &mut journal,
        &watcher,
        &mut controls,
        &cancel,
        &mut state,
        &host.identity,
        &prepared.identity,
    )
    .await
    .is_err()
    {
        // Observe an exit coincident with a failing journal/control operation,
        // without waiting for an unrelated later application shutdown.
        if !state.parent_exited {
            state.parent_exited = watcher.exited().unwrap_or(false);
        }
        let _ = post_commit_failure(&store, &mut journal, &state, &host.identity).await;
    }
    Ok(())
}

fn recovery_event(status: &str, message: &'static str) -> Value {
    json!({"kind": "recovery", "status": status, "message": message, "installation_enabled": false})
}

/// Called only after validating both bundles and, for deletion, the live caller.
/// There is no operation in this function that can exchange the bundles back.
fn finish_updated(store: &Store, journal: &Value, acknowledged: bool) -> Result<&'static str> {
    if acknowledged {
        let mut intent = journal.clone();
        store.phase(&mut intent, "acknowledged_cleanup")?;
        store.cleanup(&intent)?;
    } else if journal["phase"] == "launch_failed" {
        return Ok("manual");
    }
    Ok("updated")
}

async fn inspect_recovery(
    store: &Store,
    journal: &Value,
    acknowledge: bool,
) -> Result<&'static str> {
    safe_path(&store.target, true)?;
    let host = native::host().await?;
    if host.path != store.target {
        return Err(failure());
    }
    let old = identity(&journal["old"])?;
    // Never take a publisher trust anchor from a writable JSON journal.
    if old.team_id != host.identity.team_id {
        return Err(failure());
    }
    let old_anchor = Anchor::parse(&journal["old_anchor"])?;
    store.stage(journal)?;
    if old_anchor.check(&store.target).is_ok() {
        validate(&store.target, &old, &host.identity.team_id).await?;
        // Preparing can contain incomplete unsigned extraction. Nothing from it
        // will execute; ownership/inode and the unchanged signed old target suffice.
        store.cleanup(journal)?;
        return Ok("cancelled");
    }
    let new = identity(&journal["new"])?;
    Anchor::parse(&journal["new_anchor"])?.check(&store.target)?;
    validate(&store.target, &new, &host.identity.team_id).await?;
    let backup = store.candidate(journal)?;
    old_anchor.check(&backup)?;
    validate(&backup, &old, &host.identity.team_id).await?;
    if acknowledge {
        let pid = unsafe { libc::getppid() };
        let watcher = ParentExit::register(pid)?;
        caller_matches(pid, &store.target)?;
        native::validate_running_host(pid as u32, &host).await?;
        if unsafe { libc::getppid() } != pid || watcher.exited()? {
            return Err(failure());
        }
        // Acknowledgement itself is the startup proof, not an old launch flag.
        // The exclusive lock also proves the old installer is no longer active.
    }
    finish_updated(store, journal, acknowledge)
}

pub async fn recover(acknowledge: bool, emit: &mut impl FnMut(Value) -> Result<()>) -> Result<()> {
    let target = native::host_path()?;
    let root = crate::storage::cache_root()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let store = loop {
        match Store::open(root.clone(), target.clone()) {
            Ok(store) => break store,
            Err(error)
                if error.code == "installation_busy" && tokio::time::Instant::now() < deadline =>
            {
                // New-app startup may beat the old installer's return from open.
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(_) => {
                return emit(recovery_event(
                    "manual",
                    "Another installer is active or recovery storage is unsafe.",
                ));
            }
        }
    };
    let journal = match store.read() {
        Ok(None) => {
            return emit(
                json!({"kind": "recovery", "status": "none", "installation_enabled": false}),
            );
        }
        Ok(Some(journal)) => journal,
        Err(_) => {
            return emit(recovery_event(
                "manual",
                "Recovery metadata is unsafe. No application files were changed.",
            ));
        }
    };
    let (status, message) = match inspect_recovery(&store, &journal, acknowledge).await {
        Ok("cancelled") => (
            "cancelled",
            "The original application is unchanged; unused staging was removed.",
        ),
        Ok("updated") if acknowledge => (
            "updated",
            "The updated application acknowledged startup; its recovery copy was removed.",
        ),
        Ok("updated") => (
            "updated",
            "The updated application is installed. Recovery copies are retained until startup acknowledgement.",
        ),
        _ => (
            "manual",
            "Recovery requires manual attention. No rollback was attempted; recovery copies were retained.",
        ),
    };
    emit(recovery_event(status, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn fixture() -> (tempfile::TempDir, Store, Value) {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let target = root.path().join("Resolved.app");
        fs::create_dir(&target).unwrap();
        let store = Store::open(root.path().to_owned(), target.clone()).unwrap();
        let stage = tempfile::Builder::new()
            .prefix(&store.prefix)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(root.path())
            .unwrap()
            .keep();
        let journal = json!({"schema":1,"target":target,"stage":stage,
            "stage_anchor":Anchor::of(&stage).unwrap().value(),
            "old_anchor":Anchor::of(&target).unwrap().value(),"phase":"preparing"});
        (root, store, journal)
    }
    #[test]
    fn journal_is_bounded_and_substitutions_fail_closed() {
        let (_root, store, mut journal) = fixture();
        store.write(&journal).unwrap();
        assert_eq!(store.read().unwrap().unwrap(), journal);
        journal["padding"] = json!("x".repeat(JOURNAL_LIMIT as usize));
        assert!(store.write(&journal).is_err());
        journal.as_object_mut().unwrap().remove("padding");
        journal["target"] = json!("/another/Resolved.app");
        store.write(&journal).unwrap();
        assert!(store.read().is_err());
        fs::remove_file(&store.journal).unwrap();
        symlink(&store.target, &store.journal).unwrap();
        assert!(store.read().is_err());
    }
    #[test]
    fn lock_is_exclusive_and_released_by_drop() {
        let (root, store, _) = fixture();
        let target = store.target.clone();
        assert!(Store::open(root.path().to_owned(), target.clone()).is_err());
        drop(store);
        assert!(Store::open(root.path().to_owned(), target).is_ok());
    }
    #[test]
    fn cleanup_cannot_follow_json_to_unrelated_or_replaced_paths() {
        let (root, store, mut journal) = fixture();
        let original = store.stage(&journal).unwrap();
        let unrelated = root.path().join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        journal["stage"] = json!(unrelated);
        journal["stage_anchor"] = Anchor::of(&unrelated).unwrap().value();
        assert!(store.cleanup(&journal).is_err());
        assert!(unrelated.exists());
        journal["stage"] = json!(original);
        assert!(store.stage(&journal).is_err());
        journal["stage_anchor"] = Anchor::of(&original).unwrap().value();
        fs::rename(&original, root.path().join("moved")).unwrap();
        symlink(&unrelated, &original).unwrap();
        assert!(store.cleanup(&journal).is_err());
        assert!(unrelated.exists());
    }
    #[test]
    fn ownership_policy_rejects_shared_write_and_foreign_owner() {
        assert!(mount_writable(0));
        assert!(!mount_writable(libc::MNT_RDONLY as u32));
        assert!(owner_policy(501, 0o755, 501, true));
        assert!(owner_policy(0, 0o755, 501, false));
        assert!(!owner_policy(0, 0o755, 501, true));
        assert!(!owner_policy(502, 0o755, 501, false));
        for mode in [0o777, 0o775, 0o757] {
            assert!(!owner_policy(501, mode, 501, true));
        }
        assert!(applications_policy(
            Path::new("/Applications"),
            0,
            80,
            0o40775
        ));
        assert!(!applications_policy(
            Path::new("/Applications"),
            501,
            80,
            0o40775
        ));
        assert!(!applications_policy(
            Path::new("/Applications"),
            0,
            20,
            0o40775
        ));
        assert!(!applications_policy(
            Path::new("/Applications"),
            0,
            80,
            0o40777
        ));
        assert!(!applications_policy(Path::new("/other"), 0, 80, 0o40775));
    }
    #[tokio::test]
    async fn ordered_controls_and_preparation_reject_early_commit() {
        assert_eq!(preparation_control(Some(Control::Commit)).code, "protocol");
        assert_eq!(preparation_control(None).code, "cancelled");
        for control in [Control::Cancel, Control::Closed] {
            let (tx, mut rx) = mpsc::channel(2);
            tx.send(control).await.unwrap();
            assert!(wait_commit(&mut rx).await.is_err());
        }
        let (tx, mut rx) = mpsc::channel(2);
        tx.send(Control::Commit).await.unwrap();
        tx.send(Control::Closed).await.unwrap();
        drop(tx);
        assert!(wait_commit(&mut rx).await.is_ok());
        assert!(matches!(rx.recv().await, Some(Control::Closed)));
        let (_tx, mut rx) = mpsc::channel(1);
        assert!(
            wait_commit_for(&mut rx, Duration::from_millis(1))
                .await
                .is_err()
        );
    }
    #[test]
    fn atomic_exchange_keeps_both_directory_objects() {
        let (root, store, mut journal) = fixture();
        let stage = store.stage(&journal).unwrap();
        let candidate = stage.join("Resolved.app");
        fs::create_dir(&candidate).unwrap();
        fs::write(store.target.join("old"), b"old").unwrap();
        fs::write(candidate.join("new"), b"new").unwrap();
        let old = Anchor::of(&store.target).unwrap();
        let new = Anchor::of(&candidate).unwrap();
        journal["phase"] = json!("swap_intent");
        store.write(&journal).unwrap();
        exchange(
            &directory(root.path()).unwrap(),
            store.target.file_name().unwrap(),
            &directory(&stage).unwrap(),
            candidate.file_name().unwrap(),
        )
        .unwrap();
        new.check(&store.target).unwrap();
        old.check(&candidate).unwrap();
        assert!(store.target.join("new").exists());
        assert!(candidate.join("old").exists());
        // Simulate a crash before persisting swapped: anchors, not phase, win.
        assert_eq!(store.read().unwrap().unwrap()["phase"], "swap_intent");
        assert!(old.check(&store.target).is_err());
    }
    #[test]
    fn interrupted_phases_and_launch_failure_never_roll_back_or_discard_backup() {
        for phase in [
            "swap_intent",
            "swapped",
            "launch_intent",
            "launched",
            "launch_failed",
        ] {
            let (root, store, mut journal) = fixture();
            let stage = store.stage(&journal).unwrap();
            let candidate = stage.join("Resolved.app");
            fs::create_dir(&candidate).unwrap();
            fs::write(store.target.join("old"), b"old").unwrap();
            fs::write(candidate.join("new"), b"new").unwrap();
            exchange(
                &directory(root.path()).unwrap(),
                store.target.file_name().unwrap(),
                &directory(&stage).unwrap(),
                candidate.file_name().unwrap(),
            )
            .unwrap();
            journal["phase"] = json!(phase);
            store.write(&journal).unwrap();
            // Test-private trust fixture: real signed identity/caller validation
            // happens before this finalization boundary in inspect_recovery.
            assert_eq!(
                finish_updated(&store, &journal, false).unwrap(),
                if phase == "launch_failed" {
                    "manual"
                } else {
                    "updated"
                }
            );
            assert!(candidate.join("old").exists());
            assert!(store.target.join("new").exists());
            assert_eq!(finish_updated(&store, &journal, true).unwrap(), "updated");
            assert!(!stage.exists());
            assert!(!store.journal.exists());
            assert!(store.target.join("new").exists());
        }
    }
    #[test]
    fn post_exit_failure_records_old_launch_intent_without_removing_recovery_state() {
        let (root, store, mut journal) = fixture();
        let stage = store.stage(&journal).unwrap();
        for phase in ["committed", "parent_exited", "swap_intent", "swap_failed"] {
            journal["phase"] = json!(phase);
            store.write(&journal).unwrap();
            assert!(old_launch_intent(&store, &mut journal, &CommittedState::default()).is_err());
            let state = CommittedState {
                parent_exited: true,
                ..Default::default()
            };
            old_launch_intent(&store, &mut journal, &state).unwrap();
            assert_eq!(store.read().unwrap().unwrap()["phase"], "old_launch_intent");
            assert!(stage.exists());
            assert!(store.target.exists());
            let swapped = CommittedState {
                swapped: true,
                ..state
            };
            assert!(old_launch_intent(&store, &mut journal, &swapped).is_err());
        }
        let state = CommittedState {
            parent_exited: true,
            ..Default::default()
        };
        fs::rename(&store.target, root.path().join("original")).unwrap();
        fs::create_dir(&store.target).unwrap();
        assert!(old_launch_intent(&store, &mut journal, &state).is_err());
        assert!(stage.exists());
    }

    #[test]
    fn late_cancellation_and_unclean_disconnect_are_sticky() {
        let (watch_tx, cancel) = watch::channel(false);
        let (tx, mut controls) = mpsc::channel(3);
        let mut state = CommittedState {
            parent_exited: true,
            ..Default::default()
        };
        tx.try_send(Control::Closed).unwrap();
        state.check(&mut controls, &cancel).unwrap();
        assert!(state.clean_closed);
        watch_tx.send(true).unwrap();
        assert!(state.check(&mut controls, &cancel).is_err());
        watch_tx.send(false).unwrap();
        assert!(state.check(&mut controls, &cancel).is_err());

        let mut state = CommittedState::default();
        tx.try_send(Control::Closed).unwrap();
        tx.try_send(Control::Cancel).unwrap();
        assert!(state.check(&mut controls, &cancel).is_err());
        assert!(state.check(&mut controls, &cancel).is_err());
        let (tx, mut disconnected) = mpsc::channel(1);
        drop(tx);
        assert!(
            CommittedState::default()
                .check(&mut disconnected, &cancel)
                .is_err()
        );
        let mut clean = CommittedState {
            clean_closed: true,
            ..Default::default()
        };
        clean.check(&mut disconnected, &cancel).unwrap();
    }

    #[tokio::test]
    async fn exact_process_watcher_notices_exit_and_cancel_wins() {
        let (_watch_tx, cancel) = watch::channel(false);
        let mut state = CommittedState::default();
        let mut child = std::process::Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let watcher = ParentExit::register(child.id() as libc::pid_t).unwrap();
        assert!(!watcher.exited().unwrap());
        drop(child.stdin.take());
        child.wait().unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        // Exit alone is insufficient; retain the one-shot event while waiting
        // for the parser's explicit clean-EOF marker.
        assert!(
            wait_exit_for(
                &watcher,
                &mut rx,
                &cancel,
                &mut state,
                Duration::from_millis(1)
            )
            .await
            .is_err()
        );
        assert!(state.parent_exited);
        assert!(!state.clean_closed);
        tx.send(Control::Closed).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(2),
            wait_exit(&watcher, &mut rx, &cancel, &mut state),
        )
        .await
        .unwrap()
        .unwrap();
        let mut child = std::process::Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let watcher = ParentExit::register(child.id() as libc::pid_t).unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        let mut state = CommittedState::default();
        assert!(
            wait_exit_for(
                &watcher,
                &mut rx,
                &cancel,
                &mut state,
                Duration::from_millis(1)
            )
            .await
            .is_err()
        );
        assert!(child.try_wait().unwrap().is_none());
        tx.send(Control::Cancel).await.unwrap();
        assert!(
            wait_exit(&watcher, &mut rx, &cancel, &mut state)
                .await
                .is_err()
        );
        drop(child.stdin.take());
        child.wait().unwrap();
    }
}

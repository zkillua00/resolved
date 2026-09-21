//! A prepared installer is supervised on a dedicated thread. The final GUI
//! commit is one nonblocking pipe write, with no await before cx.quit().
use std::{
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, OwnedFd},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::{oneshot, watch};

use crate::update_install::{InstallControl, InstallTicket, RestartRequest};

const LIMIT: usize = 16 * 1024;
const PREPARE_LIMIT: Duration = Duration::from_secs(15 * 60);
const CANCEL_GRACE: Duration = Duration::from_secs(5);

struct State {
    input: File,
    ready: bool,
    committed: bool,
    cancelled: bool,
    exited: bool,
    cancel: watch::Receiver<bool>,
    ticket_cancelled: Arc<AtomicBool>,
}

impl State {
    fn cancellation_requested(&self) -> bool {
        self.cancelled
            || self.ticket_cancelled.load(Ordering::Acquire)
            || *self.cancel.borrow()
            || self.cancel.has_changed().is_err()
    }

    fn write(&mut self, command: &[u8]) -> Result<(), String> {
        for _ in 0..2 {
            match self.input.write(command) {
                Ok(count) if count == command.len() => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                _ => break,
            }
        }
        Err("The installer did not accept the handoff. Resolved was not closed.".to_owned())
    }

    fn cancel(&mut self) {
        if !self.committed && !self.exited {
            self.cancelled = true;
            let _ = self.write(b"cancel\n");
        }
    }
}

struct Ticket {
    state: Arc<Mutex<State>>,
    cancel: Arc<AtomicBool>,
}

impl InstallControl for Ticket {
    fn commit(&mut self) -> Result<(), String> {
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| "The installer handoff is busy. Resolved was not closed.".to_owned())?;
        if !state.ready || state.exited || state.cancellation_requested() {
            return Err("Update preparation was cancelled or the installer exited.".to_owned());
        }
        state.write(b"commit\n")?;
        state.committed = true;
        Ok(())
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        // Destruction also runs on the foreground error path; it must not wait
        // for a descheduled supervisor holding the control mutex.
        self.cancel.store(true, Ordering::Release);
    }
}

struct Process {
    child: Child,
    state: Option<Arc<Mutex<State>>>,
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(state) = &self.state {
            let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
            if state.committed {
                // Intentional handoff: the helper waits for our exact process
                // exit, installs, and is reaped by launchd after we are gone.
                return;
            }
            state.cancel();
        }
        let deadline = Instant::now() + CANCEL_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Ok(None) => break,
            }
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn nonblocking(fd: libc::c_int) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err("Cannot establish a nonblocking installer channel.".to_owned());
    }
    Ok(())
}

pub(super) async fn prepare(
    executable: PathBuf,
    request: RestartRequest,
    cancel: watch::Receiver<bool>,
    finished: watch::Sender<bool>,
) -> Result<InstallTicket, String> {
    let approval =
        serde_json::to_string(&request.artifact).map_err(|_| "Invalid update approval.")?;
    if approval.len() > LIMIT || !request.archive.is_absolute() {
        return Err("Invalid update preparation request.".to_owned());
    }
    let mut command = Command::new(executable);
    command
        .args([
            "--protocol-version",
            "1",
            "install",
            "--artifact-json",
            &approval,
            "--archive",
        ])
        .arg(&request.archive);
    spawn(command, request, cancel, finished).await
}

async fn spawn(
    mut command: Command,
    request: RestartRequest,
    cancel: watch::Receiver<bool>,
    finished: watch::Sender<bool>,
) -> Result<InstallTicket, String> {
    let (ready, receiver) = oneshot::channel();
    std::thread::Builder::new()
        .name("updater-install-handoff".into())
        .spawn(move || {
            let mut ready = Some(ready);
            let result = drive(&mut command, &request, cancel, &mut ready);
            if let Some(ready) = ready {
                let _ = ready.send(Err(result
                    .err()
                    .unwrap_or_else(|| "Installer exited before readiness.".to_owned())));
            }
            let _ = finished.send(true);
        })
        .map_err(|_| "Could not supervise the installer.".to_owned())?;
    receiver
        .await
        .map_err(|_| "The installer supervisor stopped.".to_owned())?
}

fn ready_event(line: &[u8], request: &RestartRequest) -> Result<bool, String> {
    let value: serde_json::Value = serde_json::from_slice(line)
        .map_err(|_| "The installer returned an invalid response.".to_owned())?;
    if value["protocol_version"] != 1 {
        return Err("The installer protocol is incompatible.".to_owned());
    }
    match value["kind"].as_str() {
        Some("install_ready") if value["installation_enabled"] == true => {
            let artifact: resolved_release::UpdateArtifact =
                serde_json::from_value(value["artifact"].clone())
                    .map_err(|_| "The installer returned an invalid approval.".to_owned())?;
            if artifact != request.artifact {
                return Err("The prepared update differs from the approved release.".to_owned());
            }
            Ok(true)
        }
        Some("progress") if value["installation_enabled"] == false => Ok(false),
        Some("cancelled") => Err("Update preparation cancelled.".to_owned()),
        Some("error") => Err(
            "Update preparation failed. Recheck verification and installation permissions."
                .to_owned(),
        ),
        _ => Err("The installer returned an unexpected response.".to_owned()),
    }
}

fn drive(
    command: &mut Command,
    request: &RestartRequest,
    cancel: watch::Receiver<bool>,
    ready: &mut Option<oneshot::Sender<Result<InstallTicket, String>>>,
) -> Result<(), String> {
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Err("Update preparation cancelled.".to_owned());
    }
    let child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start the bundled installer.".to_owned())?;
    let mut process = Process { child, state: None };
    let input: OwnedFd = process
        .child
        .stdin
        .take()
        .ok_or("Missing installer input.")?
        .into();
    let mut output = process
        .child
        .stdout
        .take()
        .ok_or("Missing installer output.")?;
    nonblocking(input.as_raw_fd())?;
    nonblocking(output.as_raw_fd())?;
    let ticket_cancelled = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new(State {
        input: File::from(input),
        ready: false,
        committed: false,
        cancelled: false,
        exited: false,
        cancel,
        ticket_cancelled: ticket_cancelled.clone(),
    }));
    process.state = Some(state.clone());
    let mut buffer = Vec::new();
    let mut bytes = [0_u8; 4096];
    let deadline = Instant::now() + PREPARE_LIMIT;
    loop {
        {
            let mut value = state.lock().unwrap_or_else(|poison| poison.into_inner());
            if value.committed {
                return Ok(());
            }
            if value.cancellation_requested() {
                value.cancel();
                return Err("Update preparation cancelled.".to_owned());
            }
        }
        if Instant::now() >= deadline {
            return Err("Update preparation timed out.".to_owned());
        }
        if let Some(_) = process
            .child
            .try_wait()
            .map_err(|_| "Could not observe installer exit.")?
        {
            state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .exited = true;
            return Err("The installer exited before the restart handoff.".to_owned());
        }
        match output.read(&mut bytes) {
            Ok(0) => return Err("The installer closed its preparation channel.".to_owned()),
            Ok(count) => {
                for &byte in &bytes[..count] {
                    if buffer.len() >= LIMIT {
                        return Err("Installer response exceeded its size limit.".to_owned());
                    }
                    buffer.push(byte);
                    if byte == b'\n' {
                        if ready_event(&buffer, request)? {
                            let sender = ready
                                .take()
                                .ok_or("Duplicate installer readiness response.")?;
                            state
                                .lock()
                                .unwrap_or_else(|poison| poison.into_inner())
                                .ready = true;
                            let _ = sender.send(Ok(InstallTicket::new(Ticket {
                                state: state.clone(),
                                cancel: ticket_cancelled.clone(),
                            })));
                        }
                        buffer.clear();
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("Could not read installer preparation.".to_owned()),
        }
        let mut descriptor = libc::pollfd {
            fd: output.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut descriptor, 1, 20) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_and_drop_do_not_wait_for_the_supervisor_mutex() {
        let cancel = Arc::new(AtomicBool::new(false));
        let (_sender, receiver) = watch::channel(false);
        let state = Arc::new(Mutex::new(State {
            input: tempfile::tempfile().unwrap(),
            ready: true,
            committed: false,
            cancelled: false,
            exited: false,
            cancel: receiver,
            ticket_cancelled: cancel.clone(),
        }));
        let held = state.lock().unwrap();
        let mut ticket = Ticket {
            state: state.clone(),
            cancel: cancel.clone(),
        };
        assert!(ticket.commit().is_err());
        drop(ticket);
        assert!(cancel.load(Ordering::Acquire));
        drop(held);
    }

    fn request() -> RestartRequest {
        RestartRequest {
            archive: "/private/cache/update.zip".into(),
            artifact: resolved_release::UpdateArtifact {
                version:"0.13.0".into(), name:"Resolved-0.13.0-macos-arm64.zip".into(),
                url:"https://github.com/zkillua00/resolved/releases/download/v0.13.0/Resolved-0.13.0-macos-arm64.zip".into(),
                size:3, sha256:"a".repeat(64), arch:"arm64".into(),
            },
        }
    }

    #[tokio::test]
    async fn dropping_uncommitted_ticket_cancels_and_reaps() {
        let request = request();
        let ready=serde_json::json!({"protocol_version":1,"kind":"install_ready","installation_enabled":true,"artifact":request.artifact}).to_string();
        let directory = tempfile::tempdir().unwrap();
        let result = directory.path().join("result");
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "printf '%s\\n' \"$1\"; read line; printf '%s' \"$line\" > \"$2\"",
                "test",
                &ready,
            ])
            .arg(&result);
        let (_cancel, receiver) = watch::channel(false);
        let (finished, mut completion) = watch::channel(false);
        let ticket = spawn(command, request, receiver, finished).await.unwrap();
        drop(ticket);
        tokio::time::timeout(Duration::from_secs(10), completion.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read_to_string(result).unwrap(), "cancel");
    }

    #[tokio::test]
    async fn commit_is_one_command_and_drop_does_not_cancel_it() {
        let request = request();
        let ready=serde_json::json!({"protocol_version":1,"kind":"install_ready","installation_enabled":true,"artifact":request.artifact}).to_string();
        let directory = tempfile::tempdir().unwrap();
        let result = directory.path().join("result");
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "printf '%s\\n' \"$1\"; read line; printf '%s' \"$line\" > \"$2\"",
                "test",
                &ready,
            ])
            .arg(&result);
        let (_cancel, receiver) = watch::channel(false);
        let (finished, mut completion) = watch::channel(false);
        let ticket = spawn(command, request, receiver, finished).await.unwrap();
        ticket.commit().unwrap();
        tokio::time::timeout(Duration::from_secs(10), completion.changed())
            .await
            .unwrap()
            .unwrap();
        // The committed process deliberately outlives supervisor ownership.
        for _ in 0..100 {
            if result.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(std::fs::read_to_string(result).unwrap(), "commit");
    }

    #[test]
    fn readiness_must_bind_approval_and_trust() {
        let request = request();
        let mut value = serde_json::json!({"protocol_version":1,"kind":"install_ready","installation_enabled":true,"artifact":request.artifact});
        assert!(ready_event(value.to_string().as_bytes(), &request).unwrap());
        value["artifact"]["size"] = serde_json::json!(4);
        assert!(ready_event(value.to_string().as_bytes(), &request).is_err());
    }
}

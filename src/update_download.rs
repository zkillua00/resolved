//! Desktop side of the bundled updater protocol. Downloaded bytes are not yet
//! publisher-verified, extracted, or eligible to install.
use std::{path::PathBuf, process::Stdio, time::Duration};

use resolved_release::UpdateArtifact;
use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{mpsc, watch},
};

const MAX_EVENT_BYTES: u64 = 16 * 1024;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(16 * 60);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Checking,
    Downloading,
    Verifying,
}

#[derive(Clone, Debug)]
pub(crate) struct Progress {
    pub phase: Phase,
    pub downloaded: u64,
    pub total: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Downloaded(PathBuf),
    Cancelled,
}

#[derive(Deserialize)]
struct Envelope {
    protocol_version: u32,
    installation_enabled: bool,
    #[serde(flatten)]
    event: Event,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Event {
    Progress {
        phase: Phase,
        downloaded: u64,
        total: u64,
    },
    Downloaded {
        artifact: UpdateArtifact,
        path: PathBuf,
    },
    Cancelled,
    Error {
        error: ErrorMessage,
    },
}

#[derive(Deserialize)]
struct ErrorMessage {
    code: String,
    #[serde(default)]
    message: Option<String>,
}

enum Validated {
    Progress(Progress),
    Finished(Result<Outcome, String>),
}

fn decode(line: &[u8], expected: &UpdateArtifact) -> Result<Validated, String> {
    if line.len() as u64 > MAX_EVENT_BYTES || !line.ends_with(b"\n") {
        return Err("The updater returned an oversized or incomplete response.".to_owned());
    }
    let message: Envelope = serde_json::from_slice(line).map_err(|_| {
        "The updater returned an invalid response. Rebuild or reinstall Resolved.".to_owned()
    })?;
    if message.protocol_version != 1 || message.installation_enabled {
        return Err("The updater protocol or capabilities do not match this app.".to_owned());
    }
    Ok(match message.event {
        Event::Progress {
            phase,
            downloaded,
            total,
        } => {
            let valid = match phase {
                Phase::Checking => downloaded == 0 && (total == 0 || total == expected.size),
                Phase::Downloading => total == expected.size && downloaded <= total,
                Phase::Verifying => total == expected.size && downloaded == total,
            };
            if !valid {
                return Err("The updater reported invalid download progress.".to_owned());
            }
            Validated::Progress(Progress {
                phase,
                downloaded,
                total,
            })
        }
        Event::Downloaded { artifact, path } => {
            if artifact != *expected
                || !path.is_absolute()
                || path.file_name().and_then(|name| name.to_str()) != Some(expected.name.as_str())
            {
                return Err(
                    "The downloaded artifact does not match the approved update.".to_owned(),
                );
            }
            Validated::Finished(Ok(Outcome::Downloaded(path)))
        }
        Event::Cancelled => Validated::Finished(Ok(Outcome::Cancelled)),
        Event::Error { error } => {
            let message = error
                .message
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    format!(
                        "Updater error: {}. Recheck the release or rebuild the app.",
                        error.code
                    )
                });
            Validated::Finished(Err(message))
        }
    })
}

pub(crate) async fn download(
    artifact: UpdateArtifact,
    progress: mpsc::Sender<Progress>,
    cancel: watch::Receiver<bool>,
    finished: watch::Sender<bool>,
) -> Result<Outcome, String> {
    let executable = crate::platform::updater_executable()?;
    let approval = serde_json::to_string(&artifact)
        .map_err(|_| "Could not encode the selected update.".to_owned())?;
    if approval.len() as u64 > MAX_EVENT_BYTES {
        return Err("The selected update metadata is too large.".to_owned());
    }
    let mut command = Command::new(executable);
    command.args([
        "--protocol-version",
        "1",
        "download",
        "--artifact-json",
        &approval,
    ]);
    run_command(command, artifact, progress, cancel, finished).await
}

async fn run_command(
    command: Command,
    artifact: UpdateArtifact,
    progress: mpsc::Sender<Progress>,
    cancel: watch::Receiver<bool>,
    finished: watch::Sender<bool>,
) -> Result<Outcome, String> {
    // Dropping a caller cancels ownership, not the child. This independent task
    // survives caller cancellation; normal app exit waits for its finished signal.
    let (owner, gone) = watch::channel(());
    let supervisor = tokio::spawn(async move {
        let result = own_command(command, artifact, progress, cancel, gone).await;
        let _ = finished.send(true);
        result
    });
    let result = supervisor
        .await
        .map_err(|error| format!("Updater supervisor failed: {error}"));
    drop(owner);
    result?
}

#[derive(Default)]
struct Session {
    // read_until is cancellation-safe only if its partially filled buffer survives.
    line: Vec<u8>,
    result: Option<Result<Outcome, String>>,
    last_downloaded: u64,
}

impl Session {
    async fn receive(
        &mut self,
        output: &mut (impl tokio::io::AsyncBufRead + Unpin),
        artifact: &UpdateArtifact,
        progress: Option<&mpsc::Sender<Progress>>,
    ) -> Result<(), String> {
        loop {
            let limit = (MAX_EVENT_BYTES + 1).saturating_sub(self.line.len() as u64);
            let length = (&mut *output)
                .take(limit)
                .read_until(b'\n', &mut self.line)
                .await
                .map_err(|error| format!("Could not read updater progress: {error}"))?;
            if length == 0 && self.line.is_empty() {
                return Ok(());
            }
            if self.result.is_some() {
                return Err("The updater returned data after its terminal response.".to_owned());
            }
            let event = decode(&self.line, artifact)?;
            self.line.clear();
            match event {
                Validated::Progress(update) => {
                    if update.downloaded < self.last_downloaded {
                        return Err("The updater's download progress moved backwards.".to_owned());
                    }
                    self.last_downloaded = update.downloaded;
                    if let Some(progress) = progress {
                        progress
                            .send(update)
                            .await
                            .map_err(|_| "The download view was closed.".to_owned())?;
                    }
                }
                Validated::Finished(finished) => self.result = Some(finished),
            }
        }
    }
}

async fn own_command(
    mut command: Command,
    artifact: UpdateArtifact,
    progress: mpsc::Sender<Progress>,
    mut cancel: watch::Receiver<bool>,
    mut owner_gone: watch::Receiver<()>,
) -> Result<Outcome, String> {
    if *cancel.borrow() {
        return Ok(Outcome::Cancelled);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            format!("Could not start the bundled updater. Reinstall or rebundle Resolved: {error}")
        })?;
    let mut input = child.stdin.take().expect("piped updater stdin");
    let mut output = BufReader::new(child.stdout.take().expect("piped updater stdout"));
    let mut session = Session::default();
    let mut cancelled = false;
    let mut received = tokio::select! {
        biased;
        _ = owner_gone.changed() => { cancelled = true; Ok(()) }
        _ = cancel.changed() => { cancelled = true; Ok(()) }
        result = tokio::time::timeout(
            OPERATION_TIMEOUT, session.receive(&mut output, &artifact, Some(&progress))
        ) => {
            result.unwrap_or_else(|_| Err("The updater download timed out.".to_owned()))
        }
    };
    // EOF/cancel is also the helper's parent-death signal. Give it time to drop
    // partial files, then kill/reap as a bounded fallback; never detach a child.
    if cancelled || received.is_err() {
        let _ = tokio::time::timeout(EXIT_TIMEOUT, input.write_all(b"cancel\n")).await;
    }
    drop(input);
    if cancelled {
        // Completion wins if the helper already committed it. Drain with the
        // persistent frame buffer, without blocking on UI progress delivery.
        received =
            tokio::time::timeout(EXIT_TIMEOUT, session.receive(&mut output, &artifact, None))
                .await
                .unwrap_or_else(|_| Err("The updater did not finish cancelling.".to_owned()));
    }
    let status = match tokio::time::timeout(EXIT_TIMEOUT, child.wait()).await {
        Ok(status) => status.map_err(|error| format!("Could not wait for the updater: {error}"))?,
        Err(_) => {
            let _ = child.kill().await;
            return Err("The updater did not exit after the operation.".to_owned());
        }
    };
    received?;
    if matches!(session.result, Some(Ok(Outcome::Downloaded(_)))) && !status.success() {
        return Err("The updater failed after reporting a download.".to_owned());
    }
    if let Some(Ok(Outcome::Downloaded(path))) = session.result {
        return Ok(Outcome::Downloaded(path));
    }
    if cancelled {
        return Ok(Outcome::Cancelled);
    }
    session
        .result
        .ok_or_else(|| "The updater exited without a completed download.".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn artifact() -> UpdateArtifact {
        UpdateArtifact {
            version: "0.13.0".into(), name: "Resolved-0.13.0-macos-arm64.zip".into(),
            url: "https://github.com/zkillua00/resolved/releases/download/v0.13.0/Resolved-0.13.0-macos-arm64.zip".into(),
            size: 100, sha256: "a".repeat(64), arch: "arm64".into(),
        }
    }

    fn line(payload: serde_json::Value) -> Vec<u8> {
        let mut payload = payload;
        payload["protocol_version"] = json!(1);
        payload["installation_enabled"] = json!(false);
        format!("{payload}\n").into_bytes()
    }

    #[test]
    fn validates_download_metadata_and_absolute_path() {
        let expected = artifact();
        let good = line(json!({
            "kind":"downloaded", "artifact":expected,
            "path":format!("/cache/{}", expected.name)
        }));
        assert!(matches!(
            decode(&good, &expected),
            Ok(Validated::Finished(Ok(Outcome::Downloaded(_))))
        ));
        let mut wrong = expected.clone();
        wrong.sha256 = "b".repeat(64);
        assert!(
            decode(
                &line(json!({"kind":"downloaded","artifact":wrong,"path":"/cache/a.zip"})),
                &expected
            )
            .is_err()
        );
        assert!(
            decode(
                &line(json!({"kind":"downloaded","artifact":expected,"path":expected.name})),
                &expected
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_invalid_progress_and_protocol() {
        let expected = artifact();
        for payload in [
            json!({"kind":"progress","phase":"downloading","downloaded":101,"total":100}),
            json!({"kind":"progress","phase":"downloading","downloaded":0,"total":99}),
            json!({"kind":"progress","phase":"verifying","downloaded":50,"total":100}),
            json!({"kind":"progress","phase":"installing","downloaded":100,"total":100}),
        ] {
            assert!(decode(&line(payload), &expected).is_err());
        }
        assert!(
            decode(
                b"{\"protocol_version\":2,\"installation_enabled\":false,\"kind\":\"cancelled\"}\n",
                &expected
            )
            .is_err()
        );
        assert!(
            decode(
                b"{\"protocol_version\":1,\"installation_enabled\":true,\"kind\":\"cancelled\"}\n",
                &expected
            )
            .is_err()
        );
        assert!(decode(&vec![b'a'; MAX_EVENT_BYTES as usize + 1], &expected).is_err());
        assert!(decode(b"{}", &expected).is_err());
    }

    #[tokio::test]
    async fn cancelled_before_start_does_not_spawn() {
        let (progress, _) = mpsc::channel(1);
        let (_, cancel) = watch::channel(true);
        assert_eq!(
            run_command(
                Command::new("/does-not-exist"),
                artifact(),
                progress,
                cancel,
                watch::channel(false).0,
            )
            .await
            .unwrap(),
            Outcome::Cancelled
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn truncated_child_output_is_not_success() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '{}'; exit 0"]);
        let (progress, _receiver) = mpsc::channel(1);
        let (_sender, cancel) = watch::channel(false);
        assert!(
            run_command(
                command,
                artifact(),
                progress,
                cancel,
                watch::channel(false).0
            )
            .await
            .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn completed_message_requires_successful_exit_and_no_extra_output() {
        let expected = artifact();
        let payload = String::from_utf8(line(json!({
            "kind": "downloaded", "artifact": expected,
            "path": format!("/cache/{}", expected.name)
        })))
        .unwrap();
        for (script, success) in [
            ("printf '%s' \"$1\"; exit 0", true),
            ("printf '%s' \"$1\"; exit 1", false),
            ("printf '%s%s' \"$1\" \"$1\"; exit 0", false),
        ] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script, "updater-test", &payload]);
            let (progress, _receiver) = mpsc::channel(1);
            let (_sender, cancel) = watch::channel(false);
            let result = run_command(
                command,
                expected.clone(),
                progress,
                cancel,
                watch::channel(false).0,
            )
            .await;
            assert_eq!(result.is_ok(), success, "{script}: {result:?}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_reaps_a_stalled_child() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "printf '%s\\n' '{\"protocol_version\":1,\"installation_enabled\":false,\"kind\":\"progress\",\"phase\":\"checking\",\"downloaded\":0,\"total\":0}'; read line; exit 1",
        ]);
        let (progress, mut receiver) = mpsc::channel(1);
        let (sender, cancel) = watch::channel(false);
        let task = tokio::spawn(run_command(
            command,
            artifact(),
            progress,
            cancel,
            watch::channel(false).0,
        ));
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        sender.send(true).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            Outcome::Cancelled
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_caller_allows_helper_cleanup_before_reap() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("cleaned");
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "printf '%s\\n' '{\"protocol_version\":1,\"installation_enabled\":false,\"kind\":\"progress\",\"phase\":\"checking\",\"downloaded\":0,\"total\":0}'; read line; printf cleaned > \"$1\"",
            "updater-test",
        ]).arg(&marker);
        let (progress, mut receiver) = mpsc::channel(1);
        let (_sender, cancel) = watch::channel(false);
        let (finished, mut completion) = watch::channel(false);
        let task = tokio::spawn(run_command(command, artifact(), progress, cancel, finished));
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        task.abort();
        tokio::time::timeout(Duration::from_secs(5), completion.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(*completion.borrow());
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "cleaned");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_preserves_a_concurrently_completed_partial_frame() {
        let expected = artifact();
        let payload = String::from_utf8(line(json!({
            "kind": "downloaded", "artifact": expected,
            "path": format!("/cache/{}", expected.name)
        })))
        .unwrap();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "printf '%s\\n' '{\"protocol_version\":1,\"installation_enabled\":false,\"kind\":\"progress\",\"phase\":\"checking\",\"downloaded\":0,\"total\":0}'; printf '%s' \"$1\"; read line; printf '%s' \"$2\"; exit 0",
            "updater-test", &payload[..20], &payload[20..],
        ]);
        let (progress, mut receiver) = mpsc::channel(1);
        let (sender, cancel) = watch::channel(false);
        let task = tokio::spawn(run_command(
            command,
            expected.clone(),
            progress,
            cancel,
            watch::channel(false).0,
        ));
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        sender.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            result,
            Outcome::Downloaded(PathBuf::from(format!("/cache/{}", expected.name)))
        );
    }
}

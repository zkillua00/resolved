//! Installation handoff. The foreground commits only after persistence is
//! durable; a committed installer must survive the desktop process exiting.
use std::{path::PathBuf, process::Stdio, time::Duration};

use resolved_release::UpdateArtifact;
use tokio::{io::AsyncReadExt, sync::watch};

#[derive(Clone, Debug)]
pub(crate) struct RestartRequest {
    pub artifact: UpdateArtifact,
    pub archive: PathBuf,
}

pub(crate) trait InstallControl: Send {
    /// Must be a bounded, nonblocking write. Success transfers ownership to the
    /// helper; dropping an uncommitted control cancels its preparation instead.
    fn commit(&mut self) -> Result<(), String>;
}

pub(crate) struct InstallTicket(Box<dyn InstallControl>);

impl InstallTicket {
    pub(crate) fn new(control: impl InstallControl + 'static) -> Self {
        Self(Box::new(control))
    }

    pub(crate) fn commit(mut self) -> Result<(), String> {
        self.0.commit()
    }
}

pub(crate) async fn prepare(
    request: RestartRequest,
    cancel: watch::Receiver<bool>,
    finished: watch::Sender<bool>,
) -> Result<InstallTicket, String> {
    crate::platform::prepare_update_install(request, cancel, finished).await
}

pub(crate) async fn recovery() -> Result<Option<String>, String> {
    if !crate::platform::supports_update_downloads() {
        return Ok(None);
    }
    let executable = crate::platform::updater_executable()?;
    let mut child = tokio::process::Command::new(executable)
        .args(["--protocol-version", "1", "recover", "--ack"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Could not check pending update recovery.".to_owned())?;
    let mut output = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        child
            .stdout
            .take()
            .expect("piped recovery output")
            .take(16 * 1024 + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|_| "Could not read update recovery status.".to_owned())?;
        if output.len() > 16 * 1024 {
            return Err("Update recovery returned an oversized response.".to_owned());
        }
        let status = child
            .wait()
            .await
            .map_err(|_| "Could not wait for update recovery.".to_owned())?;
        parse_recovery(&output, status.success())
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            let _ = child.kill().await;
            Err("Update recovery timed out; any recovery copy was retained.".to_owned())
        }
    }
}

fn parse_recovery(bytes: &[u8], success: bool) -> Result<Option<String>, String> {
    let response: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| "Invalid update recovery response.".to_owned())?;
    if response["protocol_version"] != 1 || response["installation_enabled"] != false {
        return Err("Incompatible update recovery protocol.".to_owned());
    }
    if !success || response["kind"] != "recovery" {
        return Err(
            "A pending update needs attention; no automatic rollback was performed.".to_owned(),
        );
    }
    match response["status"].as_str() {
        Some("none") => Ok(None),
        Some("updated") => Ok(Some("Update installed and startup acknowledged.".to_owned())),
        Some("cancelled") => Ok(Some("An interrupted update was cancelled; the installed app was preserved.".to_owned())),
        Some("manual") => Ok(Some("An interrupted update needs manual attention. The installed app and recovery copy were left unchanged.".to_owned())),
        _ => Err("Unknown update recovery state.".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_never_trusts_arbitrary_messages_or_install_permissions() {
        assert_eq!(parse_recovery(br#"{"protocol_version":1,"installation_enabled":false,"kind":"recovery","status":"none"}"#, true).unwrap(), None);
        assert!(parse_recovery(br#"{"protocol_version":1,"installation_enabled":true,"kind":"recovery","status":"updated"}"#, true).is_err());
        assert!(parse_recovery(br#"{"protocol_version":1,"installation_enabled":false,"kind":"recovery","status":"updated"}"#, false).is_err());
        assert!(parse_recovery(br#"{"protocol_version":1,"installation_enabled":false,"kind":"recovery","status":"arbitrary"}"#, true).is_err());
    }
}

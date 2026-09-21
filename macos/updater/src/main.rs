mod archive;
mod download;
mod install;
mod native;
mod storage;
mod transport;
mod verification;

use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
    process::ExitCode,
};
use tokio::sync::{mpsc, watch};

#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Health,
    VerifyHost,
    Download {
        artifact: resolved_release::UpdateArtifact,
    },
    Verify {
        artifact: resolved_release::UpdateArtifact,
        archive: PathBuf,
    },
    Install {
        artifact: resolved_release::UpdateArtifact,
        archive: PathBuf,
    },
    Recover {
        acknowledge: bool,
    },
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, &'static str> {
    let args = args
        .into_iter()
        .map(|arg| arg.into_string().map_err(|_| "invalid_utf8"))
        .collect::<Result<Vec<_>, _>>()?;
    if args.first().map(String::as_str) != Some("--protocol-version") {
        return Err("missing_protocol_version");
    }
    if args.get(1).map(String::as_str) != Some("1") {
        return Err("unsupported_protocol_version");
    }
    match args.get(2).map(String::as_str) {
        Some("health") if args.len() == 3 => Ok(Command::Health),
        Some("verify-host") if args.len() == 3 => Ok(Command::VerifyHost),
        Some("download")
            if args.len() == 5 && args[3] == "--artifact-json" && args[4].len() <= 16 * 1024 =>
        {
            Ok(Command::Download {
                artifact: serde_json::from_str(&args[4]).map_err(|_| "invalid_arguments")?,
            })
        }
        Some(operation @ ("verify" | "install"))
            if args.len() == 7
                && args[3] == "--artifact-json"
                && args[4].len() <= 16 * 1024
                && args[5] == "--archive"
                && args[6].len() <= 4096
                && std::path::Path::new(&args[6]).is_absolute() =>
        {
            let artifact = serde_json::from_str(&args[4]).map_err(|_| "invalid_arguments")?;
            let archive = PathBuf::from(&args[6]);
            if operation == "verify" {
                Ok(Command::Verify { artifact, archive })
            } else {
                Ok(Command::Install { artifact, archive })
            }
        }
        Some("recover") if args.len() == 3 => Ok(Command::Recover { acknowledge: false }),
        Some("recover") if args.len() == 4 && args[3] == "--ack" => {
            Ok(Command::Recover { acknowledge: true })
        }
        None | Some("health" | "download" | "verify" | "verify-host" | "install" | "recover") => {
            Err("invalid_arguments")
        }
        _ => Err("unsupported_command"),
    }
}

fn event(mut value: Value) -> Value {
    value["protocol_version"] = json!(1);
    if value.get("installation_enabled").is_none() {
        value["installation_enabled"] = json!(false);
    }
    value
}

fn emit(value: Value) -> download::Result<()> {
    let mut line = serde_json::to_vec(&event(value))
        .map_err(|_| download::Failure::new("output", "Cannot encode updater event."))?;
    // Parent's record cap includes the final NDJSON newline.
    if line.len() >= 16 * 1024 {
        return Err(download::Failure::new(
            "output",
            "Updater event is too large.",
        ));
    }
    line.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&line)
        .map_err(|_| download::Failure::new("output", "Updater output is unavailable."))
}

fn health() -> Value {
    json!({
        "kind": "health", "name": "resolved-updater",
        "app_version": env!("RESOLVED_UPDATER_APP_VERSION"),
        "build_version": env!("RESOLVED_BUILD_VERSION"),
        "build_number": env!("API_TESTER_BUILD_NUMBER"),
        "os": "macos", "arch": env!("RESOLVED_UPDATER_ARCH"),
        "feed_url": resolved_release::FEED_URL,
        "capabilities": ["health", "download", "verify", "verify-host", "install", "recover"]
    })
}

// A detached std thread cannot keep runtime shutdown waiting for an open pipe.
fn monitor(mut input: impl Read, cancel: watch::Sender<bool>) {
    let mut line = Vec::with_capacity(7);
    let mut overflow = false;
    let mut byte = [0];
    loop {
        match input.read(&mut byte) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Ok(0) | Err(_) => break,
            Ok(_) if byte[0] == b'\n' => {
                if !overflow && line == b"cancel" {
                    break;
                }
                line.clear();
                overflow = false;
            }
            Ok(_) => {
                if line.len() < 6 {
                    line.push(byte[0]);
                } else {
                    overflow = true;
                }
            }
        }
    }
    let _ = cancel.send(true);
}

fn monitor_install(
    mut input: impl Read,
    controls: mpsc::Sender<install::Control>,
    cancel: watch::Sender<bool>,
) {
    let mut line = Vec::new();
    let mut committed = false;
    let mut byte = [0];
    loop {
        match input.read(&mut byte) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Ok(0) => {
                if !committed || !line.is_empty() {
                    let _ = cancel.send(true);
                    let _ = controls.try_send(install::Control::Cancel);
                }
                let _ = controls.try_send(install::Control::Closed);
                return;
            }
            Err(_) => {
                let _ = cancel.send(true);
                let _ = controls.try_send(install::Control::Cancel);
                return;
            }
            Ok(_) if byte[0] == b'\n' => {
                let control = if line == b"commit" && !committed {
                    committed = true;
                    install::Control::Commit
                } else {
                    let _ = cancel.send(true);
                    install::Control::Cancel
                };
                if controls.try_send(control).is_err() {
                    let _ = cancel.send(true);
                    return;
                }
                line.clear();
            }
            Ok(_) => {
                if line.len() >= 6 {
                    let _ = cancel.send(true);
                    let _ = controls.try_send(install::Control::Cancel);
                    return;
                }
                line.push(byte[0]);
            }
        }
    }
}

struct NonblockingOutput {
    fd: libc::c_int,
    flags: libc::c_int,
}

impl NonblockingOutput {
    fn new(fd: libc::c_int) -> download::Result<Self> {
        // A slow/full pipe must fail, not block cancellation. Restore inherited
        // flags on normal exit because an open file description may be shared.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(download::Failure::new(
                    "output",
                    "Cannot configure updater output.",
                ));
            }
            Ok(Self { fd, flags })
        }
    }
}

impl Drop for NonblockingOutput {
    fn drop(&mut self) {
        unsafe { libc::fcntl(self.fd, libc::F_SETFL, self.flags) };
    }
}

fn run(command: Command) -> download::Result<()> {
    if matches!(command, Command::Health) {
        return emit(health());
    }
    let (sender, mut receiver) = watch::channel(false);
    let (controls, control_receiver) = mpsc::channel(8);
    let installing = matches!(command, Command::Install { .. });
    let recovering = matches!(command, Command::Recover { .. } | Command::VerifyHost);
    if !recovering {
        std::thread::Builder::new()
            .name("updater-cancel".into())
            .spawn(move || {
                if installing {
                    monitor_install(io::stdin().lock(), controls, sender);
                } else {
                    monitor(io::stdin().lock(), sender);
                }
            })
            .map_err(|_| download::Failure::new("runtime", "Cannot monitor cancellation."))?;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| download::Failure::new("runtime", "Cannot start updater runtime."))?;
    let result = runtime.block_on(async {
        match command {
            Command::Download { artifact } => {
                download::interruptible(&mut receiver, download::run(&artifact, &mut emit)).await
            }
            Command::Verify { artifact, archive } => {
                let cooperative_cancel = receiver.clone();
                download::interruptible(
                    &mut receiver,
                    verification::run(&artifact, &archive, &cooperative_cancel, &mut emit),
                )
                .await
            }
            Command::Install { artifact, archive } => {
                install::run(&artifact, &archive, control_receiver, receiver, &mut emit).await
            }
            Command::Recover { acknowledge } => tokio::time::timeout(
                std::time::Duration::from_secs(120),
                install::recover(acknowledge, &mut emit),
            )
            .await
            .map_err(|_| download::Failure::new("timeout", "Update recovery timed out."))?,
            Command::VerifyHost => {
                let host = native::host().await?;
                emit(
                    json!({"kind":"host_verified","team_id":host.identity.team_id,
                    "app_version":host.identity.version,"build_number":host.identity.build}),
                )
            }
            Command::Health => unreachable!(),
        }
    });
    // Transport guards have already synchronously killed/reaped their child,
    // including when interruptible dropped a stalled network future.
    runtime.shutdown_timeout(std::time::Duration::from_millis(100));
    result
}

fn main() -> ExitCode {
    let mut output_mode = None;
    let result = parse(std::env::args_os().skip(1))
        .map_err(|code| download::Failure::new(code, "Invalid updater arguments."))
        .and_then(|command| {
            if !matches!(command, Command::Health) {
                output_mode = Some(NonblockingOutput::new(libc::STDOUT_FILENO)?);
            }
            run(command)
        });
    let code = match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            let value = if failure.code == "cancelled" {
                json!({"kind": "cancelled"})
            } else {
                json!({"kind": "error", "error": {"code": failure.code, "message": failure.message}})
            };
            let _ = emit(value);
            ExitCode::FAILURE
        }
    };
    drop(output_mode);
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_eof_requires_a_clean_frame_and_read_errors_cancel() {
        let (tx, mut controls) = mpsc::channel(8);
        let (cancel, cancelled) = watch::channel(false);
        monitor_install(b"commit\n".as_slice(), tx, cancel);
        assert!(matches!(controls.try_recv(), Ok(install::Control::Commit)));
        assert!(matches!(controls.try_recv(), Ok(install::Control::Closed)));
        assert!(!*cancelled.borrow());
        for input in [
            b"commit\ncancel".as_slice(),
            b"commit\nx".as_slice(),
            b"commit\ncancel\n".as_slice(),
        ] {
            let (tx, mut controls) = mpsc::channel(8);
            let (cancel, cancelled) = watch::channel(false);
            monitor_install(input, tx, cancel);
            assert!(matches!(controls.try_recv(), Ok(install::Control::Commit)));
            assert!(matches!(controls.try_recv(), Ok(install::Control::Cancel)));
            assert!(*cancelled.borrow());
        }
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("broken input"))
            }
        }
        let (tx, mut controls) = mpsc::channel(8);
        let (cancel, cancelled) = watch::channel(false);
        monitor_install(b"commit\n".as_slice().chain(Broken), tx, cancel);
        assert!(matches!(controls.try_recv(), Ok(install::Control::Commit)));
        assert!(matches!(controls.try_recv(), Ok(install::Control::Cancel)));
        assert!(*cancelled.borrow());
    }

    #[test]
    fn restores_inherited_output_flags() {
        use std::os::fd::AsRawFd;
        let output = tempfile::tempfile().unwrap();
        let copy = output.try_clone().unwrap();
        let original = unsafe { libc::fcntl(copy.as_raw_fd(), libc::F_GETFL) };
        let mode = NonblockingOutput::new(output.as_raw_fd()).unwrap();
        assert_ne!(
            unsafe { libc::fcntl(copy.as_raw_fd(), libc::F_GETFL) } & libc::O_NONBLOCK,
            0
        );
        drop(mode);
        assert_eq!(
            unsafe { libc::fcntl(copy.as_raw_fd(), libc::F_GETFL) },
            original
        );
    }

    #[test]
    fn exact_arguments_and_messages() {
        let hash = "a".repeat(64);
        let artifact = json!({"version":"2.0","sha256":hash,"size":3,
            "name":"Resolved.zip","arch":"arm64","url":"https://example.invalid/a"})
        .to_string();
        assert!(matches!(
            parse(
                [
                    "--protocol-version",
                    "1",
                    "download",
                    "--artifact-json",
                    &artifact
                ]
                .map(OsString::from)
            ),
            Ok(Command::Download { .. })
        ));
        let padded = format!("{artifact}{}", " ".repeat(16 * 1024 - artifact.len()));
        assert!(
            parse(
                [
                    "--protocol-version",
                    "1",
                    "download",
                    "--artifact-json",
                    &padded
                ]
                .map(OsString::from)
            )
            .is_ok()
        );
        let oversized = format!("{padded} ");
        assert!(
            parse(
                [
                    "--protocol-version",
                    "1",
                    "download",
                    "--artifact-json",
                    &oversized
                ]
                .map(OsString::from)
            )
            .is_err()
        );
        for args in [
            vec![],
            vec!["health"],
            vec!["--protocol-version", "2", "health"],
            vec!["--protocol-version", "1", "health", "extra"],
            vec!["--protocol-version", "1", "download"],
            vec!["--protocol-version", "1", "install"],
            vec![
                "--protocol-version",
                "1",
                "download",
                "--sha256",
                &hash,
                "--version",
                "2.0",
            ],
            vec![
                "--protocol-version",
                "1",
                "download",
                "--version",
                "2.0",
                "--sha256",
                "bad",
            ],
        ] {
            assert!(parse(args.into_iter().map(OsString::from)).is_err());
        }
        assert_eq!(
            event(health())["capabilities"],
            json!([
                "health",
                "download",
                "verify",
                "verify-host",
                "install",
                "recover"
            ])
        );
        assert_eq!(event(health())["installation_enabled"], false);
    }

    #[test]
    fn cancellation_input_and_eof() {
        for mut input in [
            b"".as_slice(),
            b"cancel\r\njunk".as_slice(),
            b"cancelx\njunk".as_slice(),
            b" cancel\njunk".as_slice(),
            b"cancel\0\njunk".as_slice(),
        ] {
            let (tx, rx) = watch::channel(false);
            monitor(&mut input, tx);
            assert!(input.is_empty(), "only EOF may cancel malformed lines");
            assert!(*rx.borrow());
        }
        let (tx, rx) = watch::channel(false);
        let mut input = b"cancel\nignored".as_slice();
        monitor(&mut input, tx);
        assert!(*rx.borrow());
        assert_eq!(input, b"ignored");
    }

    #[test]
    fn metadata_is_bounded_and_cannot_inject_directives() {
        for value in [
            "",
            "\n",
            "1\ncargo:rustc-env=X=Y",
            "1\r2",
            "1\0",
            " 1",
            "💥",
        ] {
            assert!(build_support::validate_version(value).is_err());
        }
        assert!(build_support::validate_version(&"a".repeat(129)).is_err());
        for value in ["1.2.3", "1.2.3-nightly.abc+1", "nightly-abc123"] {
            assert!(build_support::validate_version(value).is_ok());
        }
        for value in ["", "0", "-1", "+1", "1\n", "1.0", "18446744073709551616"] {
            assert!(build_support::validate_build_number(value).is_err());
        }
        assert!(build_support::validate_build_number("1").is_ok());
        assert!(build_support::validate_build_number("18446744073709551615").is_ok());
    }

    #[test]
    fn build_contract() {
        assert!(build_support::validate_target("", "macos", "aarch64").is_err());
        assert_eq!(
            build_support::validate_target("1", "macos", "aarch64"),
            Ok("arm64")
        );
        assert_eq!(
            build_support::validate_target("1", "macos", "x86_64"),
            Ok("x64")
        );
        assert!(build_support::validate_target("1", "linux", "aarch64").is_err());
        assert!(build_support::validate_version("1\ncargo:X=Y").is_err());
        assert!(build_support::validate_build_number("0").is_err());
    }
}

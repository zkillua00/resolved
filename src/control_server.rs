use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use ring::rand::{SecureRandom as _, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc as tokio_mpsc;

#[cfg(not(unix))]
use std::net::TcpStream as ControlStream;
#[cfg(unix)]
use std::os::unix::net::UnixStream as ControlStream;

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
const CONTROL_DESCRIPTOR_NAME: &str = "resolved-control.json";
const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(unix)]
const CONTROL_SOCKET_NAME: &str = "resolved-control.sock";

#[derive(Debug)]
pub struct ControlCall {
    pub method: String,
    pub params: Value,
    response: mpsc::Sender<ControlResponse>,
}

impl ControlCall {
    pub fn respond(self, response: ControlResponse) {
        let _ = self.response.send(response);
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ControlResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ControlResponse {
    pub fn success(result: Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Deserialize)]
struct WireRequest {
    token: String,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct ControlDescriptor<'a> {
    protocol_version: u32,
    transport: &'a str,
    endpoint: String,
    token: &'a str,
}

pub struct ControlServer {
    descriptor_path: PathBuf,
    stopping: Arc<AtomicBool>,
    listener: Option<thread::JoinHandle<()>>,
}

impl ControlServer {
    pub fn remove_stale_files(data_directory: &Path) -> io::Result<()> {
        remove_if_exists(&data_directory.join(CONTROL_DESCRIPTOR_NAME))?;
        #[cfg(unix)]
        remove_if_exists(&data_directory.join(CONTROL_SOCKET_NAME))?;
        Ok(())
    }

    pub fn start(
        data_directory: &Path,
    ) -> io::Result<(Self, tokio_mpsc::UnboundedReceiver<ControlCall>)> {
        fs::create_dir_all(data_directory)?;
        let token = random_token()?;
        let descriptor_path = data_directory.join(CONTROL_DESCRIPTOR_NAME);
        let (sender, receiver) = tokio_mpsc::unbounded_channel();
        let stopping = Arc::new(AtomicBool::new(false));

        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
            use std::os::unix::net::UnixListener;

            let socket_path = data_directory.join(CONTROL_SOCKET_NAME);
            if socket_path.exists() {
                fs::remove_file(&socket_path)?;
            }
            let listener = UnixListener::bind(&socket_path)?;
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            write_descriptor(
                &descriptor_path,
                &ControlDescriptor {
                    protocol_version: CONTROL_PROTOCOL_VERSION,
                    transport: "unix",
                    endpoint: socket_path.to_string_lossy().into_owned(),
                    token: &token,
                },
                |options| {
                    options.mode(0o600);
                },
            )?;

            let listener_stopping = Arc::clone(&stopping);
            let listener_token = token.clone();
            let thread = thread::Builder::new()
                .name("resolved-control".to_owned())
                .spawn(move || {
                    while !listener_stopping.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                if let Err(error) = spawn_connection(
                                    stream, listener_token.clone(), sender.clone(),
                                ) {
                                    tracing::warn!(%error, "could not start local control connection");
                                }
                            }
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(25));
                            }
                            Err(error) => {
                                tracing::warn!(%error, "local control listener stopped");
                                break;
                            }
                        }
                    }
                })?;

            Ok((
                Self {
                    descriptor_path,
                    stopping,
                    listener: Some(thread),
                },
                receiver,
            ))
        }

        #[cfg(not(unix))]
        {
            use std::net::TcpListener;

            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
            let endpoint = listener.local_addr()?.to_string();
            listener.set_nonblocking(true)?;
            write_descriptor(
                &descriptor_path,
                &ControlDescriptor {
                    protocol_version: CONTROL_PROTOCOL_VERSION,
                    transport: "tcp",
                    endpoint,
                    token: &token,
                },
                |_| {},
            )?;

            let listener_stopping = Arc::clone(&stopping);
            let listener_token = token.clone();
            let thread = thread::Builder::new()
                .name("resolved-control".to_owned())
                .spawn(move || {
                    while !listener_stopping.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                if let Err(error) = spawn_connection(
                                    stream, listener_token.clone(), sender.clone(),
                                ) {
                                    tracing::warn!(%error, "could not start local control connection");
                                }
                            }
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(25));
                            }
                            Err(error) => {
                                tracing::warn!(%error, "local control listener stopped");
                                break;
                            }
                        }
                    }
                })?;

            Ok((
                Self {
                    descriptor_path,
                    stopping,
                    listener: Some(thread),
                },
                receiver,
            ))
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        let data_directory = self
            .descriptor_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let _ = Self::remove_stale_files(data_directory);
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| io::Error::other("could not generate a local control token"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn write_descriptor(
    path: &Path,
    descriptor: &ControlDescriptor<'_>,
    configure: impl FnOnce(&mut OpenOptions),
) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    configure(&mut options);
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, descriptor).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()
}

fn spawn_connection(
    stream: ControlStream,
    token: String,
    sender: tokio_mpsc::UnboundedSender<ControlCall>,
) -> io::Result<()> {
    // Accepted sockets can inherit the listener's nonblocking mode (e.g. macOS).
    // Only accept is polled: each worker reads a complete frame synchronously.
    // Configure both transports before handing the stream to that worker.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(CONTROL_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_IO_TIMEOUT))?;
    thread::Builder::new()
        .name("resolved-control-client".to_owned())
        .spawn(move || handle_connection(stream, &token, sender))
        .map(|_| ())
}

fn handle_connection<S>(
    mut stream: S,
    token: &str,
    sender: tokio_mpsc::UnboundedSender<ControlCall>,
) where
    S: io::Read + io::Write,
{
    let response = read_request(&mut stream, token, sender)
        .unwrap_or_else(|error| ControlResponse::error(error.to_string()));
    let _ = serde_json::to_writer(&mut stream, &response);
    let _ = stream.write_all(b"\n");
    let _ = stream.flush();
}

fn read_request<S>(
    stream: &mut S,
    token: &str,
    sender: tokio_mpsc::UnboundedSender<ControlCall>,
) -> io::Result<ControlResponse>
where
    S: io::Read + io::Write,
{
    let mut line = String::new();
    let mut reader = std::io::Read::take(BufReader::new(&mut *stream), 1024 * 1024 + 1);
    reader.read_line(&mut line)?;
    if line.len() > 1024 * 1024 || !line.ends_with('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local control request must be one newline-terminated JSON message of at most 1 MiB",
        ));
    }
    let request: WireRequest = serde_json::from_str(&line)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if !constant_time_eq(request.token.as_bytes(), token.as_bytes()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid local control token",
        ));
    }
    let (response_sender, response_receiver) = mpsc::channel();
    sender
        .send(ControlCall {
            method: request.method,
            params: request.params,
            response: response_sender,
        })
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Resolved is shutting down"))?;
    response_receiver
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "local control request timed out"))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_chunked_request_below_limit_reaches_the_app_channel() {
        let directory = tempfile::tempdir().unwrap();
        let (_server, receiver) = ControlServer::start(directory.path()).unwrap();
        let descriptor: Value = serde_json::from_slice(
            &fs::read(directory.path().join(CONTROL_DESCRIPTOR_NAME)).unwrap(),
        )
        .unwrap();
        let stream = ControlStream::connect(descriptor["endpoint"].as_str().unwrap()).unwrap();
        assert_delayed_chunked_request(stream, receiver, descriptor["token"].as_str().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn worker_resets_nonblocking_mode_and_sets_io_timeouts() {
        let (client, server) = ControlStream::pair().unwrap();
        // Force the inherited state even on systems whose accept returns a
        // blocking socket, so this regression is also detectable on Linux.
        server.set_nonblocking(true).unwrap();
        let socket_options = server.try_clone().unwrap();
        let (sender, receiver) = tokio_mpsc::unbounded_channel();
        spawn_connection(server, "test-token".to_owned(), sender).unwrap();
        assert_eq!(
            socket_options.read_timeout().unwrap(),
            Some(CONTROL_IO_TIMEOUT)
        );
        assert_eq!(
            socket_options.write_timeout().unwrap(),
            Some(CONTROL_IO_TIMEOUT)
        );
        drop(socket_options);
        assert_delayed_chunked_request(client, receiver, "test-token");
    }

    fn assert_delayed_chunked_request(
        mut stream: ControlStream,
        mut receiver: tokio_mpsc::UnboundedReceiver<ControlCall>,
        token: &str,
    ) {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let params = serde_json::json!({ "replay": "x".repeat(45 * 1024) });
        let request = serde_json::to_vec(&serde_json::json!({
            "token": token,
            "method": "replay",
            "params": params,
        }))
        .unwrap();
        assert!(request.len() < 1024 * 1024);
        // Give the polling listener time to accept before the first byte arrives.
        thread::sleep(Duration::from_millis(100));
        for chunk in request.chunks(512) {
            stream
                .write_all(chunk)
                .expect("connection must survive gaps between chunks");
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            receiver.try_recv().is_err(),
            "dispatch requires the newline"
        );
        stream.write_all(b"\n").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let call = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                .await
                .expect("complete request dispatched")
                .expect("server remains available")
        });
        assert_eq!(call.method, "replay");
        assert_eq!(call.params, params);
        call.respond(ControlResponse::success(
            serde_json::json!({ "received": true }),
        ));
        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap()["result"]["received"],
            true
        );
        assert!(
            receiver.try_recv().is_err(),
            "request is dispatched exactly once"
        );
    }

    #[test]
    fn incomplete_and_oversized_frames_are_rejected_before_dispatch() {
        let valid = serde_json::json!({
            "token": "test-token",
            "method": "status",
            "params": {},
        })
        .to_string();
        for frame in [valid.into_bytes(), vec![b' '; 1024 * 1024 + 1]] {
            let (sender, mut receiver) = tokio_mpsc::unbounded_channel();
            let error =
                read_request(&mut io::Cursor::new(frame), "test-token", sender).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(error.to_string().contains("at most 1 MiB"));
            assert!(receiver.try_recv().is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn authenticated_unix_request_reaches_the_app_channel() {
        use std::os::unix::net::UnixStream;

        let directory = tempfile::tempdir().unwrap();
        let (server, mut receiver) = ControlServer::start(directory.path()).unwrap();
        let descriptor: Value = serde_json::from_slice(
            &fs::read(directory.path().join(CONTROL_DESCRIPTOR_NAME)).unwrap(),
        )
        .unwrap();
        let socket = descriptor["endpoint"].as_str().unwrap().to_owned();
        let token = descriptor["token"].as_str().unwrap().to_owned();
        let client = thread::spawn(move || {
            let mut stream = UnixStream::connect(socket).unwrap();
            serde_json::to_writer(
                &mut stream,
                &serde_json::json!({
                    "token": token,
                    "method": "status",
                    "params": {}
                }),
            )
            .unwrap();
            stream.write_all(b"\n").unwrap();
            stream.flush().unwrap();
            let mut response = String::new();
            BufReader::new(stream).read_line(&mut response).unwrap();
            serde_json::from_str::<Value>(&response).unwrap()
        });

        let call = receiver.blocking_recv().unwrap();
        assert_eq!(call.method, "status");
        call.respond(ControlResponse::success(
            serde_json::json!({ "ready": true }),
        ));
        assert_eq!(client.join().unwrap()["result"]["ready"], true);

        drop(server);
        assert!(!directory.path().join(CONTROL_DESCRIPTOR_NAME).exists());
    }

    #[cfg(unix)]
    #[test]
    fn invalid_token_is_rejected_before_dispatch() {
        use std::os::unix::net::UnixStream;

        let directory = tempfile::tempdir().unwrap();
        let (_server, mut receiver) = ControlServer::start(directory.path()).unwrap();
        let descriptor: Value = serde_json::from_slice(
            &fs::read(directory.path().join(CONTROL_DESCRIPTOR_NAME)).unwrap(),
        )
        .unwrap();
        let mut stream = UnixStream::connect(descriptor["endpoint"].as_str().unwrap()).unwrap();
        serde_json::to_writer(
            &mut stream,
            &serde_json::json!({
                "token": "wrong",
                "method": "status",
                "params": {}
            }),
        )
        .unwrap();
        stream.write_all(b"\n").unwrap();
        stream.flush().unwrap();
        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response).unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["ok"], false);
        assert!(
            response["error"]
                .as_str()
                .unwrap()
                .contains("invalid local control token")
        );
        assert!(receiver.try_recv().is_err());
    }
}

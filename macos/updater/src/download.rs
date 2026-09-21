use crate::storage::Download;
use crate::transport::{Client, Response, chunk, request};
use resolved_release::{
    FEED_URL, MAX_ARCHIVE_BYTES, MAX_FEED_BYTES, UpdateArtifact, check_feed,
    validate_asset_redirect,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    time::{Duration, Instant},
};
use tokio::sync::watch;

pub type Result<T> = std::result::Result<T, Failure>;

#[derive(Debug)]
pub struct Failure {
    pub code: &'static str,
    pub message: &'static str,
}

impl Failure {
    pub const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

const OVERALL: Duration = Duration::from_secs(15 * 60);
const MAX_REDIRECTS: usize = 5;

pub async fn interruptible<T>(
    cancel: &mut watch::Receiver<bool>,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = async {
            loop {
                if *cancel.borrow_and_update() { break; }
                if cancel.changed().await.is_err() { break; }
            }
        } => Err(Failure::new("cancelled", "Download cancelled.")),
        result = tokio::time::timeout(OVERALL, work) =>
            result.map_err(|_| Failure::new("timeout", "Updater operation timed out."))?,
    }
}

fn client() -> Result<Client> {
    Ok(Client::production())
}

fn headers(response: &Response, maximum: u64) -> Result<()> {
    if response.status != 200 || response.partial {
        return Err(Failure::new(
            "http_status",
            "Expected a complete HTTP 200 response.",
        ));
    }
    if response.encoded {
        return Err(Failure::new(
            "content_encoding",
            "Encoded HTTP responses are not accepted.",
        ));
    }
    if response.length.is_some_and(|size| size > maximum) {
        return Err(Failure::new(
            "size",
            "HTTP response exceeds the permitted size.",
        ));
    }
    Ok(())
}

fn redirect_target(_current: &str, location: &str, redirects: usize) -> Result<String> {
    if redirects >= MAX_REDIRECTS {
        return Err(Failure::new("redirect", "Too many asset redirects."));
    }
    // Only absolute HTTPS locations are supported; never guess relative forms.
    if !location.starts_with("https://") {
        return Err(Failure::new("redirect", "Invalid asset redirect."));
    }
    validate_asset_redirect(location)
        .map_err(|_| Failure::new("redirect", "Untrusted asset redirect."))?;
    Ok(location.to_owned())
}

async fn asset_response(client: &Client, artifact: &UpdateArtifact) -> Result<Response> {
    let mut url = artifact.url.clone();
    for redirects in 0..=MAX_REDIRECTS {
        let response = request(client, &url).await?;
        if response.partial || response.encoded {
            return Err(Failure::new(
                "headers",
                "Partial or encoded asset response.",
            ));
        }
        if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
            let location = response.location.as_deref().ok_or(Failure::new(
                "redirect",
                "Missing or invalid asset redirect.",
            ))?;
            url = redirect_target(&url, location, redirects)?;
        } else {
            headers(&response, artifact.size)?;
            return Ok(response);
        }
    }
    Err(Failure::new("redirect", "Too many asset redirects."))
}

fn approve(artifact: &UpdateArtifact, approved: &UpdateArtifact) -> Result<()> {
    if artifact != approved {
        return Err(Failure::new(
            "offer_changed",
            "The offered update changed. Check for updates again.",
        ));
    }
    if artifact.size == 0 || artifact.size > MAX_ARCHIVE_BYTES as u64 {
        return Err(Failure::new("size", "Invalid archive size."));
    }
    Ok(())
}

struct Integrity {
    expected: u64,
    downloaded: u64,
    hash: Sha256,
}

impl Integrity {
    fn new(expected: u64) -> Self {
        Self {
            expected,
            downloaded: 0,
            hash: Sha256::new(),
        }
    }
    fn push(&mut self, bytes: &[u8]) -> Result<()> {
        let size = self
            .downloaded
            .checked_add(bytes.len() as u64)
            .ok_or(Failure::new("size", "Archive is too large."))?;
        if size > self.expected || size > MAX_ARCHIVE_BYTES as u64 {
            return Err(Failure::new("size", "Archive exceeds its declared size."));
        }
        self.hash.update(bytes);
        self.downloaded = size;
        Ok(())
    }
    fn finish(self, expected_hash: &str) -> Result<()> {
        if self.downloaded != self.expected {
            return Err(Failure::new(
                "size",
                "Archive does not match its declared size.",
            ));
        }
        if format!("{:x}", self.hash.finalize()) != expected_hash {
            return Err(Failure::new(
                "checksum",
                "Archive SHA-256 does not match the feed.",
            ));
        }
        Ok(())
    }
}

fn progress(phase: &str, downloaded: u64, total: u64) -> Value {
    json!({"kind": "progress", "phase": phase, "downloaded": downloaded, "total": total})
}

pub async fn run(
    approved: &UpdateArtifact,
    emit: &mut impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    emit(progress("checking", 0, 0))?;
    let client = client()?;
    let response = request(&client, FEED_URL).await?;
    let feed = read_feed(response).await?;
    let artifact = check_feed(
        &feed,
        env!("RESOLVED_BUILD_VERSION"),
        "macos",
        env!("RESOLVED_UPDATER_ARCH"),
    )
    .and_then(|check| check.macos_update(env!("RESOLVED_UPDATER_ARCH")))
    .map_err(|_| Failure::new("feed", "Cannot authorize an update from the current feed."))?
    .ok_or(Failure::new(
        "no_update",
        "No applicable macOS update is available.",
    ))?;
    approve(&artifact, approved)?;
    if serde_json::to_vec(&artifact).map_or(true, |json| json.len() > 8 * 1024) {
        return Err(Failure::new(
            "feed",
            "Update artifact metadata is too large.",
        ));
    }
    let response = asset_response(&client, &artifact).await?;
    receive(response, artifact, Download::create()?, emit).await
}

async fn read_feed(mut response: Response) -> Result<Vec<u8>> {
    headers(&response, MAX_FEED_BYTES as u64)?;
    let mut feed = Vec::new();
    while let Some(bytes) = chunk(&mut response).await? {
        if bytes.len() > (MAX_FEED_BYTES as usize).saturating_sub(feed.len()) {
            return Err(Failure::new("feed", "Update feed exceeds its size limit."));
        }
        feed.extend_from_slice(&bytes);
    }
    Ok(feed)
}

async fn receive(
    mut response: Response,
    artifact: UpdateArtifact,
    mut file: Download,
    emit: &mut impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    headers(&response, artifact.size)?;
    let mut integrity = Integrity::new(artifact.size);
    let mut last_progress = Instant::now();
    emit(progress("downloading", 0, artifact.size))?;
    while let Some(bytes) = chunk(&mut response).await? {
        integrity.push(&bytes)?;
        file.write(&bytes)?;
        if last_progress.elapsed() >= Duration::from_millis(100) {
            emit(progress("downloading", integrity.downloaded, artifact.size))?;
            last_progress = Instant::now();
        }
        // Even a fully buffered response must yield to pending cancellation.
        tokio::task::yield_now().await;
    }
    integrity.finish(&artifact.sha256)?;
    emit(progress("verifying", artifact.size, artifact.size))?;
    file.finish(&artifact.name)?;
    tokio::task::yield_now().await;
    let path = file
        .path()
        .to_str()
        .ok_or(Failure::new("storage", "Download path is not valid UTF-8."))?;
    // RAII remains armed until the terminal event has been delivered.
    emit(json!({"kind": "downloaded", "artifact": artifact, "path": path}))?;
    file.retain();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    async fn response(wire: Vec<u8>) -> Response {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                if stream.read(&mut byte).unwrap() == 0 {
                    return;
                }
                request.push(byte[0]);
            }
            let _ = stream.write_all(&wire);
        });
        request(&Client::loopback(), &url).await.unwrap()
    }

    #[tokio::test]
    async fn http_status_encoding_and_size_fail_closed() {
        for wire in [
            "HTTP/1.1 302 Found\r\nLocation: https://apiworkbench.dev/downloads.json\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Range: bytes 0-2/3\r\nContent-Length: 3\r\n\r\nabc",
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 3\r\n\r\nabc",
            "HTTP/1.1 200 OK\r\nContent-Encoding: identity\r\nContent-Length: 3\r\n\r\nabc",
            "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabcd",
        ] {
            assert!(headers(&response(wire.as_bytes().to_vec()).await, 3).is_err());
        }
        let body = vec![b'x'; MAX_FEED_BYTES as usize + 1];
        let mut wire = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        wire.extend(body);
        assert!(read_feed(response(wire).await).await.is_err());
    }

    #[tokio::test]
    async fn receive_preserves_only_verified_success_and_cleans_output_failures() {
        for (body, success) in [
            ("abc", true),
            ("ab", false),
            ("abcd", false),
            ("xyz", false),
        ] {
            let root = tempfile::tempdir().unwrap();
            let file = Download::in_cache(root.path()).unwrap();
            let directory = file.path().parent().unwrap().to_owned();
            let wire = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}");
            let mut events = Vec::new();
            let result = receive(
                response(wire.into_bytes()).await,
                artifact(),
                file,
                &mut |event| {
                    events.push(event);
                    Ok(())
                },
            )
            .await;
            assert_eq!(result.is_ok(), success);
            if success {
                let terminal = events.last().unwrap();
                assert_eq!(terminal["kind"], "downloaded");
                assert_eq!(
                    terminal["artifact"],
                    serde_json::to_value(artifact()).unwrap()
                );
                assert_eq!(
                    std::fs::read(terminal["path"].as_str().unwrap()).unwrap(),
                    b"abc"
                );
                assert!(directory.exists());
            } else {
                assert!(!directory.exists());
                assert!(events.iter().all(|event| event["kind"] != "downloaded"));
            }
        }
        let root = tempfile::tempdir().unwrap();
        let file = Download::in_cache(root.path()).unwrap();
        let directory = file.path().parent().unwrap().to_owned();
        let response = response(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc".to_vec()).await;
        let result = receive(response, artifact(), file, &mut |event| {
            if event["kind"] == "downloaded" {
                Err(Failure::new("output", "Output closed."))
            } else {
                Ok(())
            }
        })
        .await;
        assert!(result.is_err());
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn cancellation_drops_stalled_response_and_partial_directory() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = [0; 4096];
            stream.read(&mut bytes).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\na")
                .unwrap();
            let _ = wait.recv_timeout(Duration::from_secs(5));
        });
        let response = request(&Client::loopback(), &url).await.unwrap();
        let pid = response.pid();
        let root = tempfile::tempdir().unwrap();
        let file = Download::in_cache(root.path()).unwrap();
        let directory = file.path().parent().unwrap().to_owned();
        let (tx, mut rx) = watch::channel(false);
        let cancel = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            tx.send(true).unwrap();
        });
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            interruptible(
                &mut rx,
                receive(response, artifact(), file, &mut |_| Ok(())),
            ),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "cancelled");
        assert!(!directory.exists());
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        cancel.await.unwrap();
        release.send(()).unwrap();
        server.join().unwrap();
    }

    fn artifact() -> UpdateArtifact {
        UpdateArtifact {
            version: "2.0".into(),
            name: "Resolved.zip".into(),
            url: "https://github.com/zkillua00/resolved/releases/download/v2/Resolved.zip".into(),
            sha256: format!("{:x}", Sha256::digest(b"abc")),
            arch: "arm64".into(),
            size: 3,
        }
    }
    #[test]
    fn approval_and_stream_integrity() {
        let artifact = artifact();
        assert!(approve(&artifact, &artifact).is_ok());
        for field in ["version", "sha256", "url", "size", "name", "arch"] {
            let mut changed = serde_json::to_value(&artifact).unwrap();
            changed[field] = if field == "size" {
                json!(4)
            } else {
                json!("changed")
            };
            let changed = serde_json::from_value(changed).unwrap();
            assert_eq!(
                approve(&artifact, &changed).unwrap_err().code,
                "offer_changed"
            );
        }
        let mut valid = Integrity::new(3);
        valid.push(b"a").unwrap();
        valid.push(b"bc").unwrap();
        valid.finish(&artifact.sha256).unwrap();
        let mut short = Integrity::new(3);
        short.push(b"a").unwrap();
        assert!(short.finish(&artifact.sha256).is_err());
        assert!(Integrity::new(2).push(b"abc").is_err());
        let mut corrupt = Integrity::new(3);
        corrupt.push(b"xyz").unwrap();
        assert_eq!(
            corrupt.finish(&artifact.sha256).unwrap_err().code,
            "checksum"
        );
    }
    #[test]
    fn redirects_fail_closed() {
        let base = artifact().url;
        for target in [
            "/relative.zip",
            "//release-assets.githubusercontent.com/file",
            "http://github.com/zkillua00/resolved/releases/download/v2/file.zip",
            "https://evil.example/file",
            "https://github.com/other/repo/releases/download/v2/file",
            "https://user:secret@release-assets.githubusercontent.com/file",
            "https://release-assets.githubusercontent.com:444/file",
            "https://objects.githubusercontent.com/file#fragment",
        ] {
            assert!(redirect_target(&base, target, 0).is_err());
        }
        assert!(
            redirect_target(
                &base,
                "https://release-assets.githubusercontent.com/file?sig=secret",
                4
            )
            .is_ok()
        );
        assert!(redirect_target(&base, &base, 5).is_err());
    }
    #[tokio::test]
    async fn cancellation_interrupts_stalled_io_and_closed_control() {
        let (tx, mut rx) = watch::channel(false);
        let work = async {
            tx.send(true).unwrap();
            std::future::pending::<Result<()>>().await
        };
        let result = tokio::time::timeout(Duration::from_secs(1), interruptible(&mut rx, work))
            .await
            .unwrap();
        assert_eq!(result.unwrap_err().code, "cancelled");
        let (tx, mut rx) = watch::channel(false);
        drop(tx);
        assert_eq!(
            interruptible(&mut rx, std::future::pending::<Result<()>>())
                .await
                .unwrap_err()
                .code,
            "cancelled"
        );
    }
}

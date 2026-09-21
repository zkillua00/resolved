//! The system curl is an OS facility, not a bundled dependency. Rust owns
//! redirects, framing validation, limits, cancellation, and subprocess lifetime.
use crate::download::{Failure, Result};
use std::{
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{Child, ChildStdout, Command},
};

const STALL: Duration = Duration::from_secs(30);
const HEADER_LIMIT: usize = 32 * 1024;
const REAP_LIMIT: Duration = Duration::from_secs(2);

pub struct Client {
    protocol: &'static str,
}

impl Client {
    pub fn production() -> Self {
        Self { protocol: "=https" }
    }

    #[cfg(test)]
    pub fn loopback() -> Self {
        Self { protocol: "=http" }
    }

    fn command(&self, url: &str) -> Command {
        let mut command = Command::new("/usr/bin/curl");
        command
            .env_clear()
            .current_dir("/")
            .args([
                "-q",
                "--silent",
                "--no-buffer",
                "--globoff",
                "--http1.1",
                "--include",
                "--proxy",
                "",
                "--noproxy",
                "*",
                "--retry",
                "0",
                "--proto",
                self.protocol,
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "10",
                "--max-time",
                "900",
                "--speed-limit",
                "1",
                "--speed-time",
                "30",
                "--header",
                "Accept-Encoding: identity",
                "--url",
                url,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }
}

// Synchronous bounded cleanup is intentional: Drop also runs when an outer
// future is cancelled, and cannot depend on a runtime that is shutting down.
// start_kill + try_wait explicitly kills and reaps instead of assuming that
// kill_on_drop reaps. SIGKILL/power loss cannot run this guard.
struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
        let deadline = Instant::now() + REAP_LIMIT;
        loop {
            match self.0.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(None) => break,
            }
        }
    }
}

pub struct Response {
    child: OwnedChild,
    body: BufReader<ChildStdout>,
    pub status: u16,
    pub length: Option<u64>,
    pub location: Option<String>,
    pub partial: bool,
    pub encoded: bool,
    read: u64,
}

fn malformed() -> Failure {
    Failure::new("headers", "Malformed or oversized HTTP response headers.")
}

async fn line(body: &mut BufReader<ChildStdout>, remaining: &mut usize) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        if *remaining == 0 {
            return Err(malformed());
        }
        let byte = tokio::time::timeout(STALL, body.read_u8())
            .await
            .map_err(|_| Failure::new("timeout", "HTTP headers stalled."))?
            .map_err(|_| malformed())?;
        *remaining -= 1;
        line.push(byte);
        if byte == b'\n' {
            if !line.ends_with(b"\r\n") {
                return Err(malformed());
            }
            line.truncate(line.len() - 2);
            return Ok(line);
        }
    }
}

impl Response {
    #[cfg(test)]
    pub fn pid(&self) -> u32 {
        self.child.0.id().unwrap()
    }

    async fn headers(&mut self) -> Result<()> {
        let mut remaining = HEADER_LIMIT;
        for interim in 0..=8 {
            let status_line = line(&mut self.body, &mut remaining).await?;
            let text = std::str::from_utf8(&status_line).map_err(|_| malformed())?;
            let parts = text.splitn(3, ' ').collect::<Vec<_>>();
            if parts.len() != 3
                || !matches!(parts[0], "HTTP/1.0" | "HTTP/1.1")
                || parts[1].len() != 3
                || !parts[1].bytes().all(|b| b.is_ascii_digit())
                || parts[2].bytes().any(|b| b < 32 || b == 127)
            {
                return Err(malformed());
            }
            self.status = parts[1].parse().map_err(|_| malformed())?;
            self.length = None;
            self.location = None;
            self.partial = false;
            self.encoded = false;
            let mut transfer = false;
            loop {
                let bytes = line(&mut self.body, &mut remaining).await?;
                if bytes.is_empty() {
                    break;
                }
                let text = std::str::from_utf8(&bytes).map_err(|_| malformed())?;
                let (name, value) = text.split_once(':').ok_or_else(malformed)?;
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                    || value.bytes().any(|b| (b < 32 && b != b'\t') || b == 127)
                {
                    return Err(malformed());
                }
                let value = value.trim_matches([' ', '\t']);
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => {
                        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                            return Err(malformed());
                        }
                        let length = value.parse::<u64>().map_err(|_| malformed())?;
                        if self.length.is_some_and(|old| old != length) {
                            return Err(malformed());
                        }
                        self.length = Some(length);
                    }
                    "location" => {
                        if self.location.is_some() || value.is_empty() {
                            return Err(malformed());
                        }
                        self.location = Some(value.to_owned());
                    }
                    "content-range" => self.partial = true,
                    "content-encoding" => self.encoded = true,
                    "transfer-encoding" => {
                        if transfer || !value.eq_ignore_ascii_case("chunked") {
                            return Err(malformed());
                        }
                        transfer = true;
                    }
                    _ => {}
                }
            }
            if transfer && self.length.is_some() {
                return Err(malformed());
            }
            if (100..200).contains(&self.status) {
                if self.status == 101
                    || interim == 8
                    || transfer
                    || self.length.is_some()
                    || self.partial
                    || self.encoded
                {
                    return Err(malformed());
                }
                continue;
            }
            if !(200..600).contains(&self.status) {
                return Err(malformed());
            }
            return Ok(());
        }
        Err(malformed())
    }
}

pub async fn request(client: &Client, url: &str) -> Result<Response> {
    #[cfg(test)]
    if client.protocol == "=http" && !url.starts_with("http://127.0.0.1:") {
        return Err(Failure::new("network", "Test transport requires loopback."));
    }
    let mut child = OwnedChild(
        client
            .command(url)
            .spawn()
            .map_err(|_| Failure::new("network", "Cannot start system HTTP transport."))?,
    );
    let stdout = child.0.stdout.take().ok_or(Failure::new(
        "network",
        "Cannot open HTTP transport output.",
    ))?;
    let mut response = Response {
        child,
        body: BufReader::new(stdout),
        status: 0,
        length: None,
        location: None,
        partial: false,
        encoded: false,
        read: 0,
    };
    response.headers().await?;
    Ok(response)
}

pub async fn chunk(response: &mut Response) -> Result<Option<Vec<u8>>> {
    let mut bytes = vec![0; 16 * 1024];
    let count = tokio::time::timeout(STALL, response.body.read(&mut bytes))
        .await
        .map_err(|_| Failure::new("timeout", "HTTP response stalled."))?
        .map_err(|_| Failure::new("network", "Cannot read HTTP response."))?;
    if count == 0 {
        let status = tokio::time::timeout(REAP_LIMIT, response.child.0.wait())
            .await
            .map_err(|_| Failure::new("timeout", "HTTP transport did not exit."))?
            .map_err(|_| Failure::new("network", "Cannot wait for HTTP transport."))?;
        if !status.success()
            || response
                .length
                .is_some_and(|length| length != response.read)
        {
            return Err(Failure::new("network", "Incomplete HTTP response."));
        }
        return Ok(None);
    }
    response.read = response
        .read
        .checked_add(count as u64)
        .ok_or_else(malformed)?;
    if response.length.is_some_and(|length| response.read > length) {
        return Err(malformed());
    }
    bytes.truncate(count);
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn server(wire: Vec<u8>) -> String {
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
            while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                let mut byte = [0];
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    return;
                }
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.contains("Accept-Encoding: identity\r\n"));
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            assert!(!request.to_ascii_lowercase().contains("cookie:"));
            let _ = stream.write_all(&wire);
        });
        url
    }

    #[tokio::test]
    async fn bounded_strict_headers_and_framing() {
        for header in [
            "Content-Length: 3\r\nContent-Length: 4\r\n",
            "Content-Length: +3\r\n",
            "Content-Length: 3, 3\r\n",
            "Content-Length: 18446744073709551616\r\n",
            "Content-Length: 3\r\nTransfer-Encoding: chunked\r\n",
            "Transfer-Encoding: gzip\r\n",
            "Location: https://a\r\nLocation: https://b\r\n",
            " folded: header\r\n",
            "Bad Name: x\r\n",
            "X: bad\0value\r\n",
        ] {
            let url = server(format!("HTTP/1.1 200 OK\r\n{header}\r\nabc").into_bytes());
            assert!(request(&Client::loopback(), &url).await.is_err());
        }
        for wire in [
            format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "x".repeat(HEADER_LIMIT)),
            format!(
                "{}HTTP/1.1 200 OK\r\n\r\n",
                "HTTP/1.1 100 Continue\r\n\r\n".repeat(9)
            ),
            "HTTP/1.1 101 Switching Protocols\r\n\r\n".into(),
            "HTTP/1.1 200 OK\n\n".into(),
        ] {
            assert!(
                request(&Client::loopback(), &server(wire.into_bytes()))
                    .await
                    .is_err()
            );
        }
        for wire in [
            "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 3\r\n\r\nabc",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        ] {
            let mut response = request(&Client::loopback(), &server(wire.as_bytes().to_vec()))
                .await
                .unwrap();
            let mut body = Vec::new();
            while let Some(bytes) = chunk(&mut response).await.unwrap() {
                body.extend(bytes);
            }
            assert_eq!(body, b"abc");
        }
        let url = server(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabc".to_vec());
        let mut response = request(&Client::loopback(), &url).await.unwrap();
        loop {
            match chunk(&mut response).await {
                Err(_) => break,
                Ok(Some(_)) => {}
                Ok(None) => panic!("truncated body accepted"),
            }
        }
    }

    #[tokio::test]
    async fn outer_future_drop_kills_and_reaps_transport() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        // No server response: curl blocks waiting for headers.
        let mut child = OwnedChild(Client::loopback().command(&url).spawn().unwrap());
        let pid = child.0.id().unwrap();
        let stdout = child.0.stdout.take().unwrap();
        let future = async move {
            let mut response = Response {
                child,
                body: BufReader::new(stdout),
                status: 0,
                length: None,
                location: None,
                partial: false,
                encoded: false,
                read: 0,
            };
            response.headers().await
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(30), future)
                .await
                .is_err()
        );
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn hostile_environment_is_ignored() {
        if std::env::var_os("UPDATER_TEST_ENV_CHILD").is_some() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let url = server(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc".to_vec());
                let mut response = request(&Client::loopback(), &url).await.unwrap();
                let mut body = Vec::new();
                while let Some(bytes) = chunk(&mut response).await.unwrap() {
                    body.extend(bytes);
                }
                assert_eq!(body, b"abc");
                // The production command has no inherited or explicitly set env.
                let command = Client::production().command("https://example.invalid/");
                assert_eq!(command.as_std().get_program(), "/usr/bin/curl");
                assert_eq!(command.as_std().get_args().next().unwrap(), "-q");
                assert_eq!(command.as_std().get_envs().count(), 0);
            });
            return;
        }
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".curlrc"),
            "url = \"file:///must-not-be-read\"\nproxy = \"http://127.0.0.1:1\"\n",
        )
        .unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "transport::tests::hostile_environment_is_ignored",
            ])
            .env("UPDATER_TEST_ENV_CHILD", "1")
            .env("HOME", home.path())
            .env("CURL_HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("PATH", "/nonexistent");
        for name in [
            "http_proxy",
            "https_proxy",
            "ALL_PROXY",
            "HTTP_PROXY",
            "HTTPS_PROXY",
        ] {
            child.env(name, "http://127.0.0.1:1");
        }
        for name in [
            "CURL_CA_BUNDLE",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "CURL_SSL_BACKEND",
            "NETRC",
        ] {
            child.env(name, "/nonexistent");
        }
        assert!(child.status().unwrap().success());
    }
}

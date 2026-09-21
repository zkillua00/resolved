//! Characterization, not a gateway implementation or a claim of gateway fidelity.
use super::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug)]
struct Capture {
    line: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Capture {
    fn values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect()
    }
}

async fn line(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        bytes.push(socket.read_u8().await.unwrap());
        assert!(bytes.len() < 64 * 1024, "fixture header too large");
        if bytes.ends_with(b"\r\n") {
            bytes.truncate(bytes.len() - 2);
            return bytes;
        }
    }
}

async fn fixture(response: Vec<u8>) -> (String, tokio::task::JoinHandle<Capture>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request_line = String::from_utf8(line(&mut socket).await).unwrap();
            let mut headers = Vec::new();
            loop {
                let bytes = line(&mut socket).await;
                if bytes.is_empty() {
                    break;
                }
                let text = String::from_utf8(bytes).unwrap();
                let (name, value) = text.split_once(':').unwrap();
                headers.push((name.to_owned(), value.trim().to_owned()));
            }
            let mut capture = Capture {
                line: request_line,
                headers,
                body: Vec::new(),
            };
            if capture.values("transfer-encoding").contains(&"chunked") {
                loop {
                    let size = String::from_utf8(line(&mut socket).await).unwrap();
                    let size = usize::from_str_radix(size.split(';').next().unwrap(), 16).unwrap();
                    assert!(size < 1024 * 1024);
                    if size == 0 {
                        assert!(line(&mut socket).await.is_empty());
                        break;
                    }
                    let offset = capture.body.len();
                    assert!(offset + size < 1024 * 1024);
                    capture.body.resize(offset + size, 0);
                    socket
                        .read_exact(&mut capture.body[offset..])
                        .await
                        .unwrap();
                    assert!(line(&mut socket).await.is_empty());
                }
            } else if let Some(size) = capture.values("content-length").first() {
                let size: usize = size.parse().unwrap();
                assert!(size < 1024 * 1024);
                capture.body.resize(size, 0);
                socket.read_exact(&mut capture.body).await.unwrap();
            }
            socket.write_all(&response).await.unwrap();
            socket.shutdown().await.unwrap();
            capture
        })
        .await
        .expect("fixture timed out")
    });
    (origin, task)
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

async fn execute(request: RequestDraft) -> ResponseData {
    let limits = execution_limits::ExecutionLimits::default();
    let directory = tempfile::tempdir().unwrap();
    // Disabled isolated jar: characterize response fields, not cookie persistence.
    let vault = CredentialVault::new(DatabaseStore::new(directory.path().join("cookies.sqlite3")));
    let jar = Arc::new(CookieJar::uninitialized(vault, "fidelity"));
    let client = build_http_client_with_limits(jar, &limits).unwrap();
    send_request_with_limits(&client, request, &limits)
        .await
        .unwrap()
}

#[tokio::test]
async fn gateway_fidelity_native_wire_preserved_but_history_normalizes_query_replay() {
    let reply = b"\0\xff\x80binary";
    let (origin, capture) = fixture(response(
        "200 OK",
        "Set-Cookie: a=1\r\nSet-Cookie: b=2\r\nX-Repeat: one\r\nX-Repeat: two\r\n",
        reply,
    ))
    .await;
    let target = "/a%2Fb/%20?q=a%20b&q=a+b&empty=&bare&&=value";
    let mut request = RequestDraft::new("mIxEd", format!("{origin}{target}"));
    request.body = "text\0body".into();
    request.headers = vec![
        HeaderEntry::new("X-Repeat", "one"),
        HeaderEntry::new("X-Repeat", "two"),
    ];
    let result = execute(request.clone()).await;
    let captured = capture.await.unwrap();
    assert_eq!(captured.line, format!("MIXED {target} HTTP/1.1"));
    assert_eq!(captured.values("x-repeat"), ["one", "two"]);
    assert_eq!(captured.body, b"text\0body");
    assert_eq!(result.body.as_ref(), reply);
    let cookies: Vec<_> = result
        .headers
        .iter()
        .filter(|h| h.name == "set-cookie")
        .map(|h| h.value.as_str())
        .collect();
    assert_eq!(cookies, ["a=1", "b=2"]);
    let repeated: Vec<_> = result
        .headers
        .iter()
        .filter(|h| h.name == "x-repeat")
        .map(|h| h.value.as_str())
        .collect();
    assert_eq!(repeated, ["one", "two"]);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite3");
    let mut history = RequestHistory::default();
    history.push(HistoryEntry::completed(&request, &result));
    let normalized_target = "/a%2Fb/%20?q=a+b&q=a+b&empty=&bare=&=value";
    let mut expected_history_request = request.clone();
    expected_history_request.url = format!("{origin}{normalized_target}");
    // Redaction rebuilds every query, even when no secret needs replacing.
    assert_eq!(history.entries()[0].request, expected_history_request);
    DatabaseStore::new(&path).save_history(&history).unwrap();
    let loaded = DatabaseStore::new(&path).load_history().unwrap();
    let entry = &loaded.entries()[0];
    assert_eq!(entry.request, expected_history_request);
    assert_eq!(entry.response.as_ref().unwrap().size_bytes, reply.len());
    let persisted = serde_json::to_value(entry).unwrap();
    assert!(persisted["response"].get("body").is_none());
    assert!(persisted["response"].get("headers").is_none());
    // Replay uses the persisted draft, not a byte-level wire snapshot.
    let (replay_origin, capture) = fixture(response("204 No Content", "", b"")).await;
    let mut replay = entry.request.clone();
    replay.url = format!(
        "{replay_origin}{}",
        replay.url.strip_prefix(&origin).unwrap()
    );
    assert!(execute(replay).await.body.is_empty());
    let replayed = capture.await.unwrap();
    assert_eq!(replayed.line, format!("MIXED {normalized_target} HTTP/1.1"));
    assert_ne!(replayed.line, captured.line);
    assert_eq!(replayed.body, captured.body);
    assert_eq!(replayed.values("x-repeat"), captured.values("x-repeat"));
}

#[test]
fn gateway_fidelity_editor_conversion_limits_are_explicit() {
    let binary = vec![0, 0xff, 0x80, b'a'];
    assert!(String::from_utf8(binary.clone()).is_err());
    assert_ne!(String::from_utf8_lossy(&binary).as_bytes(), binary);
    let url = "http://example.test/a%2Fb?q=a%20b&q=a+b&empty=&bare&&=value";
    let draft = RequestDraft::new("GET", url);
    assert_eq!(draft.url, url, "construction does not rebuild the URL");
    assert_eq!(
        url_with_query_params(url, &draft.query_params),
        "http://example.test/a%2Fb?q=a+b&q=a+b&empty=&bare=&=value"
    );
}

#[tokio::test]
async fn gateway_fidelity_multipart_file_preserves_bytes_but_regenerates_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("upload.bin");
    let binary = b"\0\xff\x80\r\nbinary";
    std::fs::write(&path, binary).unwrap();
    let (origin, capture) = fixture(response("200 OK", "", b"")).await;
    let mut request = RequestDraft::new("POST", origin);
    request.body_mode = BodyMode::MultipartFormData;
    request.headers.push(HeaderEntry::new(
        "Content-Type",
        "multipart/form-data; boundary=original",
    ));
    request
        .body_fields
        .push(BodyField::file("upload", path.to_str().unwrap()));
    execute(request).await;
    let captured = capture.await.unwrap();
    let content_type = captured.values("content-type")[0];
    let boundary = content_type.split("boundary=").nth(1).unwrap();
    assert_ne!(boundary, "original");
    assert!(
        captured
            .body
            .starts_with(format!("--{boundary}\r\n").as_bytes())
    );
    assert!(
        captured
            .body
            .ends_with(format!("\r\n--{boundary}--\r\n").as_bytes())
    );
    assert!(
        captured
            .body
            .windows(binary.len())
            .any(|part| part == binary)
    );
}

#[tokio::test]
async fn gateway_fidelity_gzip_is_decoded_and_opaque_response_header_is_lossy() {
    // gzip member for "hello", generated once; fixture has no compression dependency.
    let gzip = [
        31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 203, 72, 205, 201, 201, 7, 0, 134, 166, 16, 54, 5, 0, 0, 0,
    ];
    let mut wire = response("200 OK", "Content-Encoding: gzip\r\n", &gzip);
    let position = wire.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 2;
    wire.splice(position..position, b"X-Opaque: \xff\r\n".iter().copied());
    let (origin, capture) = fixture(wire).await;
    let result = execute(RequestDraft::new("GET", origin)).await;
    capture.await.unwrap();
    assert_eq!(result.body.as_ref(), b"hello");
    assert!(
        !result
            .headers
            .iter()
            .any(|h| h.name == "content-encoding" || h.name == "content-length")
    );
    assert_eq!(
        result
            .headers
            .iter()
            .find(|h| h.name == "x-opaque")
            .unwrap()
            .value,
        "\u{fffd}"
    );
}

#[tokio::test]
async fn gateway_fidelity_url_dot_segments_are_normalized_and_head_has_no_body() {
    let (origin, capture) = fixture(response("200 OK", "", b"not a HEAD body")).await;
    let result = execute(RequestDraft::new("HEAD", format!("{origin}/a/%2e%2e/b"))).await;
    assert_eq!(capture.await.unwrap().line, "HEAD /b HTTP/1.1");
    assert!(result.body.is_empty());
}

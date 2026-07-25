use std::fmt;
use std::str::FromStr;
use std::time::{Duration, Instant};

use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::task::{AbortHandle, JoinHandle};
use url::Url;

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_USER_AGENT: &str = concat!("api-tester/", env!("CARGO_PKG_VERSION"));

/// One editable request header.
///
/// Disabled entries remain in the editor but are not added to the outgoing
/// request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeaderEntry {
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    pub name: String,
    pub value: String,
}

impl HeaderEntry {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            name: name.into(),
            value: value.into(),
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

/// The complete request editor state needed by the network layer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestDraft {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<HeaderEntry>,
    #[serde(default)]
    pub body: String,
}

impl Default for RequestDraft {
    fn default() -> Self {
        Self {
            method: "GET".to_owned(),
            url: String::new(),
            headers: Vec::new(),
            body: String::new(),
        }
    }
}

impl RequestDraft {
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            ..Self::default()
        }
    }

    /// Validate and convert the editable fields into reqwest-native values.
    fn prepared(&self) -> Result<PreparedRequest, RequestError> {
        let method_text = self.method.trim().to_ascii_uppercase();
        let method = Method::from_str(&method_text)
            .map_err(|_| RequestError::InvalidMethod(self.method.clone()))?;

        let url_text = self.url.trim();
        let url =
            Url::parse(url_text).map_err(|error| RequestError::InvalidUrl(error.to_string()))?;

        match url.scheme() {
            "http" | "https" => {}
            scheme => return Err(RequestError::UnsupportedScheme(scheme.to_owned())),
        }

        let mut headers = HeaderMap::new();
        for header in self.headers.iter().filter(|header| header.enabled) {
            let name_text = header.name.trim();
            if name_text.is_empty() {
                continue;
            }

            let name = HeaderName::from_str(name_text).map_err(|error| {
                RequestError::InvalidHeaderName {
                    name: header.name.clone(),
                    reason: error.to_string(),
                }
            })?;
            let value = HeaderValue::from_str(&header.value).map_err(|error| {
                RequestError::InvalidHeaderValue {
                    name: header.name.clone(),
                    reason: error.to_string(),
                }
            })?;

            // append (rather than insert) intentionally preserves repeated
            // headers such as Accept and Cookie.
            headers.append(name, value);
        }

        Ok(PreparedRequest {
            method,
            url,
            headers,
        })
    }
}

struct PreparedRequest {
    method: Method,
    url: Url,
    headers: HeaderMap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseHeader {
    pub name: String,
    pub value: String,
}

/// The fully buffered response returned to the UI.
///
/// Buffering is deliberate for the MVP: it makes raw, pretty, and HTML views
/// deterministic. A later streaming/download path can bypass this type.
#[derive(Clone, Debug)]
pub struct ResponseData {
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    pub final_url: String,
    pub headers: Vec<ResponseHeader>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub duration: Duration,
}

impl ResponseData {
    pub fn size_bytes(&self) -> usize {
        self.body.len()
    }

    pub fn body_text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Debug)]
pub enum RequestError {
    InvalidMethod(String),
    InvalidUrl(String),
    UnsupportedScheme(String),
    InvalidHeaderName { name: String, reason: String },
    InvalidHeaderValue { name: String, reason: String },
    Transport(reqwest::Error),
    Cancelled,
    TaskFailed(String),
}

impl fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMethod(method) => write!(formatter, "invalid HTTP method: {method}"),
            Self::InvalidUrl(reason) => write!(formatter, "invalid URL: {reason}"),
            Self::UnsupportedScheme(scheme) => {
                write!(
                    formatter,
                    "unsupported URL scheme '{scheme}'; use http or https"
                )
            }
            Self::InvalidHeaderName { name, reason } => {
                write!(formatter, "invalid header name '{name}': {reason}")
            }
            Self::InvalidHeaderValue { name, reason } => {
                write!(formatter, "invalid value for header '{name}': {reason}")
            }
            Self::Transport(error) => write!(formatter, "{error}"),
            Self::Cancelled => formatter.write_str("request cancelled"),
            Self::TaskFailed(reason) => write!(formatter, "request task failed: {reason}"),
        }
    }
}

impl std::error::Error for RequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for RequestError {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(error)
    }
}

/// Construct the shared client used by the application.
pub fn build_client() -> Result<Client, RequestError> {
    Client::builder()
        .user_agent(DEFAULT_USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(DEFAULT_REQUEST_TIMEOUT)
        .build()
        .map_err(RequestError::Transport)
}

/// Send one request and buffer its response.
///
/// This future is cancellation-safe: it has no application-visible side
/// effects before returning, and dropping/aborting it drops reqwest's pending
/// response future.
pub async fn send_request(
    client: &Client,
    request: RequestDraft,
) -> Result<ResponseData, RequestError> {
    let prepared = request.prepared()?;
    let mut builder = client
        .request(prepared.method, prepared.url)
        .headers(prepared.headers);

    if !request.body.is_empty() {
        builder = builder.body(request.body);
    }

    let started_at = Instant::now();
    let response = builder.send().await?;

    let status = response.status();
    let final_url = response.url().to_string();
    let http_version = format!("{:?}", response.version());
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| ResponseHeader {
            name: name.as_str().to_owned(),
            value: String::from_utf8_lossy(value.as_bytes()).into_owned(),
        })
        .collect();
    let body = response.bytes().await?.to_vec();

    Ok(ResponseData {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or_default().to_owned(),
        http_version,
        final_url,
        headers,
        content_type,
        body,
        duration: started_at.elapsed(),
    })
}

/// A request running on a Tokio runtime.
///
/// Store a clone of [`abort_handle`](Self::abort_handle) in the GPUI model,
/// then move this value into a foreground task and await [`wait`](Self::wait).
pub struct RequestTask {
    join_handle: JoinHandle<Result<ResponseData, RequestError>>,
}

impl RequestTask {
    pub fn abort_handle(&self) -> AbortHandle {
        self.join_handle.abort_handle()
    }

    pub async fn wait(self) -> Result<ResponseData, RequestError> {
        match self.join_handle.await {
            Ok(result) => result,
            Err(error) if error.is_cancelled() => Err(RequestError::Cancelled),
            Err(error) => Err(RequestError::TaskFailed(error.to_string())),
        }
    }
}

pub fn spawn_request(runtime: &Handle, client: Client, request: RequestDraft) -> RequestTask {
    let join_handle = runtime.spawn(async move { send_request(&client, request).await });
    RequestTask { join_handle }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn request_draft_defaults_to_get() {
        let request = RequestDraft::default();
        assert_eq!(request.method, "GET");
        assert!(request.headers.is_empty());
        assert!(request.body.is_empty());
    }

    #[test]
    fn prepared_request_ignores_disabled_and_blank_header_rows() {
        let request = RequestDraft {
            method: " post ".to_owned(),
            url: " https://example.com/path ".to_owned(),
            headers: vec![
                HeaderEntry {
                    enabled: false,
                    name: "Authorization".to_owned(),
                    value: "secret".to_owned(),
                },
                HeaderEntry::new("", ""),
                HeaderEntry::new("Accept", "application/json"),
            ],
            body: "{}".to_owned(),
        };

        let prepared = request.prepared().expect("request should be valid");
        assert_eq!(prepared.method, Method::POST);
        assert_eq!(prepared.url.as_str(), "https://example.com/path");
        assert_eq!(prepared.headers.len(), 1);
        assert_eq!(prepared.headers["accept"], "application/json");
    }

    #[test]
    fn prepared_request_rejects_non_http_schemes() {
        let error = RequestDraft::new("GET", "file:///tmp/test")
            .prepared()
            .err()
            .expect("file URLs must be rejected");

        assert!(matches!(error, RequestError::UnsupportedScheme(_)));
    }

    #[test]
    fn sends_a_real_request_and_captures_the_response() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = stream.read(&mut buffer).expect("read test request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);

                let Some(headers_end) = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|position| position + 4)
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..headers_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or_default();
                if request.len() >= headers_end + content_length {
                    break;
                }
            }

            request_tx.send(request).unwrap();
            let body = br#"{"received":true}"#;
            write!(
                stream,
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nX-Test-Response: yes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });

        let request = RequestDraft {
            method: "POST".to_owned(),
            url: format!("http://{address}/echo"),
            headers: vec![HeaderEntry::new("X-Test-Request", "yes")],
            body: "hello from the client".to_owned(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let response = runtime
            .block_on(send_request(&build_client().unwrap(), request))
            .expect("request should succeed");

        server.join().unwrap();
        let received = String::from_utf8_lossy(&request_rx.recv().unwrap()).to_ascii_lowercase();
        assert!(received.starts_with("post /echo http/1.1\r\n"));
        assert!(received.contains("x-test-request: yes\r\n"));
        assert!(received.ends_with("hello from the client"));
        assert_eq!(response.status, 201);
        assert_eq!(response.status_text, "Created");
        assert_eq!(response.content_type.as_deref(), Some("application/json"));
        assert_eq!(response.body, br#"{"received":true}"#);
        assert!(
            response
                .headers
                .iter()
                .any(|header| header.name == "x-test-response" && header.value == "yes")
        );
    }

    #[test]
    fn abort_handle_cancels_a_slow_request() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = listener.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer).expect("read request headers");
            accepted_tx.send(()).unwrap();
            thread::sleep(Duration::from_millis(100));
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        });

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let task = spawn_request(
            runtime.handle(),
            build_client().unwrap(),
            RequestDraft::new("GET", format!("http://{address}/slow")),
        );
        accepted_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("request should reach the server");

        task.abort_handle().abort();
        let result = runtime.block_on(task.wait());
        assert!(matches!(result, Err(RequestError::Cancelled)));
        server.join().unwrap();
    }
}

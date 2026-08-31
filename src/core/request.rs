use std::fmt;
use std::future::Future;
use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::task::{AbortHandle, JoinHandle};
use url::Url;

use super::DbStringEnum;

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_USER_AGENT: &str = concat!("resolved/", env!("CARGO_PKG_VERSION"));
const MAX_BUFFERED_RESPONSE_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Common HTTP methods offered by editable method controls and script
/// completions. Custom extension methods remain valid when entered manually.
pub const STANDARD_HTTP_METHODS: &[&str] =
    &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// One editable request header.
///
/// Disabled entries remain in the editor but are not added to the outgoing
/// request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeaderEntry {
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default = "shared_by_default")]
    pub shared: bool,
    pub name: String,
    pub value: String,
}

impl HeaderEntry {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            shared: true,
            name: name.into(),
            value: value.into(),
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

fn shared_by_default() -> bool {
    true
}

/// The persisted request-body mode selected in the editor.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BodyMode {
    None,
    #[default]
    Raw,
    FormUrlEncoded,
    MultipartFormData,
}

impl BodyMode {
    pub const fn all() -> &'static [Self] {
        &[
            Self::None,
            Self::Raw,
            Self::FormUrlEncoded,
            Self::MultipartFormData,
        ]
    }

    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Raw => "raw",
            Self::FormUrlEncoded => "form_url_encoded",
            Self::MultipartFormData => "multipart_form_data",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|mode| mode.as_db_str() == value)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Raw => "Raw",
            Self::FormUrlEncoded => "x-www-form-urlencoded",
            Self::MultipartFormData => "form-data",
        }
    }
}

impl fmt::Display for BodyMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Syntax highlighting and default media type for a raw request body.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RawBodyLanguage {
    Text,
    #[default]
    Json,
    Xml,
    Html,
    #[serde(rename = "javascript")]
    JavaScript,
    #[serde(rename = "typescript")]
    TypeScript,
    Css,
    Markdown,
    #[serde(rename = "graphql")]
    GraphQl,
    Yaml,
    Toml,
    Sql,
    Shell,
    Rust,
    Python,
}

impl RawBodyLanguage {
    pub const fn all() -> &'static [Self] {
        &[
            Self::Text,
            Self::Json,
            Self::Xml,
            Self::Html,
            Self::JavaScript,
            Self::TypeScript,
            Self::Css,
            Self::Markdown,
            Self::GraphQl,
            Self::Yaml,
            Self::Toml,
            Self::Sql,
            Self::Shell,
            Self::Rust,
            Self::Python,
        ]
    }

    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::Xml => "xml",
            Self::Html => "html",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Css => "css",
            Self::Markdown => "markdown",
            Self::GraphQl => "graphql",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Sql => "sql",
            Self::Shell => "shell",
            Self::Rust => "rust",
            Self::Python => "python",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|language| language.as_db_str() == value)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Json => "JSON",
            Self::Xml => "XML",
            Self::Html => "HTML",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Css => "CSS",
            Self::Markdown => "Markdown",
            Self::GraphQl => "GraphQL",
            Self::Yaml => "YAML",
            Self::Toml => "TOML",
            Self::Sql => "SQL",
            Self::Shell => "Shell",
            Self::Rust => "Rust",
            Self::Python => "Python",
        }
    }

    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Text => "text/plain; charset=utf-8",
            Self::Json => "application/json",
            Self::Xml => "application/xml",
            Self::Html => "text/html; charset=utf-8",
            Self::JavaScript => "application/javascript",
            Self::TypeScript => "text/typescript; charset=utf-8",
            Self::Css => "text/css; charset=utf-8",
            Self::Markdown => "text/markdown; charset=utf-8",
            Self::GraphQl => "application/graphql",
            Self::Yaml => "application/yaml",
            Self::Toml => "application/toml",
            Self::Sql => "application/sql",
            Self::Shell => "application/x-sh",
            Self::Rust => "text/x-rust; charset=utf-8",
            Self::Python => "text/x-python; charset=utf-8",
        }
    }
}

impl fmt::Display for RawBodyLanguage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Whether a structured body row is sent as text or read from a local file.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BodyFieldKind {
    #[default]
    Text,
    File,
}

impl BodyFieldKind {
    #[allow(dead_code)]
    pub const fn all() -> &'static [Self] {
        &[Self::Text, Self::File]
    }

    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::File => "file",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|kind| kind.as_db_str() == value)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::File => "File",
        }
    }
}

impl fmt::Display for BodyFieldKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// One persisted URL-encoded or multipart body row.
///
/// For file rows, `value` stores the optional local path. An enabled file row
/// with an empty path is omitted from multipart serialization.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BodyField {
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub kind: BodyFieldKind,
}

impl Default for BodyField {
    fn default() -> Self {
        Self {
            enabled: true,
            name: String::new(),
            value: String::new(),
            kind: BodyFieldKind::Text,
        }
    }
}

impl BodyField {
    #[allow(dead_code)]
    pub fn text(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            name: name.into(),
            value: value.into(),
            kind: BodyFieldKind::Text,
        }
    }

    #[allow(dead_code)]
    pub fn file(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            enabled: true,
            name: name.into(),
            value: path.into(),
            kind: BodyFieldKind::File,
        }
    }

    pub fn file_path(&self) -> Option<&Path> {
        (self.kind == BodyFieldKind::File && !self.value.is_empty()).then(|| Path::new(&self.value))
    }
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
    #[serde(default)]
    pub body_mode: BodyMode,
    #[serde(default)]
    pub raw_body_language: RawBodyLanguage,
    #[serde(default)]
    pub body_fields: Vec<BodyField>,
}

impl Default for RequestDraft {
    fn default() -> Self {
        Self {
            method: "GET".to_owned(),
            url: String::new(),
            headers: Vec::new(),
            body: String::new(),
            body_mode: BodyMode::Raw,
            raw_body_language: RawBodyLanguage::Json,
            body_fields: Vec::new(),
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
    /// Immutable shared storage keeps UI, script, and tab snapshots from
    /// duplicating a response that may be as large as the buffering limit.
    pub body: Bytes,
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
    InvalidHeaderName {
        name: String,
        reason: String,
    },
    InvalidHeaderValue {
        name: String,
        reason: String,
    },
    MultipartFileRead {
        field_name: String,
        path: String,
        reason: std::io::Error,
    },
    ResponseBodyTooLarge {
        limit_bytes: usize,
    },
    ResponseBodyAllocationFailed(String),
    Transport(reqwest::Error),
    Upstream(String),
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
            Self::MultipartFileRead {
                field_name,
                path,
                reason,
            } => {
                write!(
                    formatter,
                    "could not read multipart file '{path}' for field '{field_name}': {reason}"
                )
            }
            Self::ResponseBodyTooLarge { limit_bytes } => {
                write!(
                    formatter,
                    "response body exceeds the {limit_bytes}-byte buffering limit"
                )
            }
            Self::ResponseBodyAllocationFailed(reason) => {
                write!(
                    formatter,
                    "could not allocate the response body buffer: {reason}"
                )
            }
            Self::Transport(error) => write!(formatter, "{error}"),
            Self::Upstream(message) => formatter.write_str(message),
            Self::Cancelled => formatter.write_str("request cancelled"),
            Self::TaskFailed(reason) => write!(formatter, "request task failed: {reason}"),
        }
    }
}

impl std::error::Error for RequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::MultipartFileRead { reason, .. } => Some(reason),
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
    crate::tls::install_crypto_provider()
        .map_err(|error| RequestError::TaskFailed(error.to_owned()))?;
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
    let PreparedRequest {
        method,
        url,
        mut headers,
    } = prepared;
    let has_user_content_type = headers.contains_key(CONTENT_TYPE);

    if request.body_mode == BodyMode::MultipartFormData {
        // reqwest must provide the boundary-bearing Content-Type and the
        // matching length for the generated multipart stream.
        headers.remove(CONTENT_TYPE);
        headers.remove(CONTENT_LENGTH);
    }

    let mut builder = client.request(method, url).headers(headers);
    builder = apply_request_body(builder, &request, has_user_content_type).await?;

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
    let body = read_response_body(response, MAX_BUFFERED_RESPONSE_BODY_BYTES).await?;

    Ok(ResponseData {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or_default().to_owned(),
        http_version,
        final_url,
        headers,
        content_type,
        body: body.into(),
        duration: started_at.elapsed(),
    })
}

async fn apply_request_body(
    mut builder: reqwest::RequestBuilder,
    request: &RequestDraft,
    has_user_content_type: bool,
) -> Result<reqwest::RequestBuilder, RequestError> {
    match request.body_mode {
        BodyMode::None => {}
        BodyMode::Raw => {
            if request.body.is_empty() {
                return Ok(builder);
            }
            if !has_user_content_type {
                builder = builder.header(CONTENT_TYPE, request.raw_body_language.content_type());
            }
            builder = builder.body(request.body.clone());
        }
        BodyMode::FormUrlEncoded => {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for field in &request.body_fields {
                if !field.enabled || field.name.trim().is_empty() {
                    continue;
                }
                // URL-encoded fields are text on the wire. Retaining the
                // editor's multipart kind lets users switch modes without
                // losing which rows were file uploads.
                serializer.append_pair(&field.name, &field.value);
            }
            if !has_user_content_type {
                builder = builder.header(CONTENT_TYPE, "application/x-www-form-urlencoded");
            }
            builder = builder.body(serializer.finish());
        }
        BodyMode::MultipartFormData => {
            let mut form = reqwest::multipart::Form::new();
            for field in request
                .body_fields
                .iter()
                .filter(|field| field.enabled && !field.name.trim().is_empty())
            {
                match field.kind {
                    BodyFieldKind::Text => {
                        form = form.text(field.name.clone(), field.value.clone());
                    }
                    BodyFieldKind::File => {
                        let Some(path) = field.file_path() else {
                            continue;
                        };
                        form = form
                            .file(field.name.clone(), path)
                            .await
                            .map_err(|reason| RequestError::MultipartFileRead {
                                field_name: field.name.clone(),
                                path: path.display().to_string(),
                                reason,
                            })?;
                    }
                }
            }
            builder = builder.multipart(form);
        }
    }
    Ok(builder)
}

async fn read_response_body(
    mut response: reqwest::Response,
    limit_bytes: usize,
) -> Result<Vec<u8>, RequestError> {
    let limit_u64 = u64::try_from(limit_bytes).unwrap_or(u64::MAX);
    if response
        .content_length()
        .is_some_and(|length| length > limit_u64)
    {
        return Err(RequestError::ResponseBodyTooLarge { limit_bytes });
    }

    let mut body = BoundedResponseBody::new(limit_bytes);
    while let Some(chunk) = response.chunk().await? {
        body.extend(&chunk)?;
    }
    Ok(body.into_bytes())
}

struct BoundedResponseBody {
    bytes: Vec<u8>,
    limit_bytes: usize,
}

impl BoundedResponseBody {
    fn new(limit_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit_bytes,
        }
    }

    fn extend(&mut self, chunk: &[u8]) -> Result<(), RequestError> {
        let next_len = self
            .bytes
            .len()
            .checked_add(chunk.len())
            .filter(|next_len| *next_len <= self.limit_bytes)
            .ok_or(RequestError::ResponseBodyTooLarge {
                limit_bytes: self.limit_bytes,
            })?;

        if next_len > self.bytes.capacity() {
            let doubled_capacity = self.bytes.capacity().max(16 * 1024).saturating_mul(2);
            let target_capacity = next_len.max(doubled_capacity).min(self.limit_bytes);
            self.bytes
                .try_reserve_exact(target_capacity.saturating_sub(self.bytes.len()))
                .map_err(|error| RequestError::ResponseBodyAllocationFailed(error.to_string()))?;
        }

        self.bytes.extend_from_slice(chunk);
        Ok(())
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A request running on a Tokio runtime.
///
/// Store a clone of [`abort_handle`](Self::abort_handle) in the GPUI model,
/// then move this value into a foreground task and await [`wait`](Self::wait).
pub struct RequestTask {
    join_handle: JoinHandle<Result<ResponseData, RequestError>>,
}

impl RequestTask {
    pub fn spawn<F>(runtime: &Handle, future: F) -> Self
    where
        F: Future<Output = Result<ResponseData, RequestError>> + Send + 'static,
    {
        Self {
            join_handle: runtime.spawn(future),
        }
    }

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
    RequestTask::spawn(runtime, async move { send_request(&client, request).await })
}

impl DbStringEnum for BodyMode {
    fn from_db_str(value: &str) -> Option<Self> {
        Self::from_db_str(value)
    }
}

impl DbStringEnum for RawBodyLanguage {
    fn from_db_str(value: &str) -> Option<Self> {
        Self::from_db_str(value)
    }
}

impl DbStringEnum for BodyFieldKind {
    fn from_db_str(value: &str) -> Option<Self> {
        Self::from_db_str(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    fn send_and_capture(mut request: RequestDraft) -> Vec<u8> {
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
                let content_length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
                let chunked = headers.lines().any(|line| {
                    let Some((name, value)) = line.split_once(':') else {
                        return false;
                    };
                    name.eq_ignore_ascii_case("transfer-encoding")
                        && value
                            .split(',')
                            .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
                });
                let complete = if let Some(content_length) = content_length {
                    request.len() >= headers_end + content_length
                } else if chunked {
                    request[headers_end..]
                        .windows(5)
                        .any(|window| window == b"0\r\n\r\n")
                } else {
                    true
                };
                if complete {
                    break;
                }
            }

            request_tx.send(request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .expect("write test response");
        });

        request.url = format!("http://{address}/capture");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime
            .block_on(send_request(&build_client().unwrap(), request))
            .expect("request should succeed");
        server.join().unwrap();
        request_rx.recv().unwrap()
    }

    fn captured_request_parts(request: &[u8]) -> (&str, &[u8]) {
        let headers_end = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4)
            .expect("captured request should contain headers");
        (
            std::str::from_utf8(&request[..headers_end]).expect("headers should be UTF-8"),
            &request[headers_end..],
        )
    }

    #[test]
    fn request_draft_defaults_to_get() {
        let request = RequestDraft::default();
        assert_eq!(request.method, "GET");
        assert!(request.headers.is_empty());
        assert!(request.body.is_empty());
        assert_eq!(request.body_mode, BodyMode::Raw);
        assert_eq!(request.raw_body_language, RawBodyLanguage::Json);
        assert!(request.body_fields.is_empty());
    }

    #[test]
    fn legacy_request_json_defaults_to_raw_json() {
        let request: RequestDraft = serde_json::from_value(serde_json::json!({
            "method": "POST",
            "url": "https://example.test/items",
            "headers": [],
            "body": "{\"legacy\":true}"
        }))
        .expect("legacy request should deserialize");

        assert_eq!(request.body_mode, BodyMode::Raw);
        assert_eq!(request.raw_body_language, RawBodyLanguage::Json);
        assert!(request.body_fields.is_empty());
        assert_eq!(request.body, "{\"legacy\":true}");

        let persisted = serde_json::to_value(request).expect("request should serialize");
        assert_eq!(persisted["body_mode"], "raw");
        assert_eq!(persisted["raw_body_language"], "json");
        assert_eq!(persisted["body_fields"], serde_json::json!([]));
    }

    #[test]
    fn body_mode_and_language_helpers_have_stable_values() {
        for mode in BodyMode::all() {
            assert_eq!(BodyMode::from_db_str(mode.as_db_str()), Some(*mode));
            assert!(!mode.label().is_empty());
        }
        assert_eq!(
            serde_json::to_string(&BodyMode::FormUrlEncoded).unwrap(),
            "\"form_url_encoded\""
        );
        assert_eq!(
            serde_json::to_string(&BodyMode::MultipartFormData).unwrap(),
            "\"multipart_form_data\""
        );

        for language in RawBodyLanguage::all() {
            assert_eq!(
                RawBodyLanguage::from_db_str(language.as_db_str()),
                Some(*language)
            );
            assert!(!language.label().is_empty());
            assert!(!language.content_type().is_empty());
        }
        assert_eq!(
            serde_json::to_string(&RawBodyLanguage::JavaScript).unwrap(),
            "\"javascript\""
        );
        assert_eq!(
            serde_json::to_string(&RawBodyLanguage::GraphQl).unwrap(),
            "\"graphql\""
        );

        let field: BodyField =
            serde_json::from_value(serde_json::json!({"name": "key", "value": "value"}))
                .expect("body field defaults should deserialize");
        assert!(field.enabled);
        assert_eq!(field.kind, BodyFieldKind::Text);
        assert!(BodyField::default().enabled);
        for kind in BodyFieldKind::all() {
            assert_eq!(BodyFieldKind::from_db_str(kind.as_db_str()), Some(*kind));
            assert!(!kind.label().is_empty());
        }
    }

    #[test]
    fn prepared_request_ignores_disabled_and_blank_header_rows() {
        let request = RequestDraft {
            method: " post ".to_owned(),
            url: " https://example.com/path ".to_owned(),
            headers: vec![
                HeaderEntry {
                    enabled: false,
                    shared: true,
                    name: "Authorization".to_owned(),
                    value: "secret".to_owned(),
                },
                HeaderEntry::new("", ""),
                HeaderEntry::new("Accept", "application/json"),
            ],
            body: "{}".to_owned(),
            ..RequestDraft::default()
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
    fn prepared_request_accepts_custom_http_methods() {
        let prepared = RequestDraft::new(" purge ", "https://example.com/cache")
            .prepared()
            .expect("extension methods should be accepted");

        assert_eq!(prepared.method.as_str(), "PURGE");
    }

    #[test]
    fn prepared_request_rejects_a_blank_method() {
        let error = RequestDraft::new("   ", "https://example.com")
            .prepared()
            .err()
            .expect("a blank method must not silently become GET");

        assert!(matches!(error, RequestError::InvalidMethod(_)));
    }

    #[test]
    fn raw_body_infers_content_type_without_overriding_a_user_header() {
        let mut inferred = RequestDraft::new("POST", "http://placeholder.invalid");
        inferred.body = "console.log('hello')".to_owned();
        inferred.raw_body_language = RawBodyLanguage::JavaScript;
        let captured = send_and_capture(inferred);
        let (headers, body) = captured_request_parts(&captured);
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("content-type: application/javascript\r\n")
        );
        assert_eq!(body, b"console.log('hello')");

        let mut custom = RequestDraft::new("POST", "http://placeholder.invalid");
        custom.body = "answer: 42".to_owned();
        custom.raw_body_language = RawBodyLanguage::Yaml;
        custom.headers.push(HeaderEntry::new(
            "Content-Type",
            "application/vnd.example+yaml",
        ));
        let captured = send_and_capture(custom);
        let (headers, body) = captured_request_parts(&captured);
        let headers = headers.to_ascii_lowercase();
        assert!(headers.contains("content-type: application/vnd.example+yaml\r\n"));
        assert!(!headers.contains("content-type: application/yaml\r\n"));
        assert_eq!(body, b"answer: 42");
    }

    #[test]
    fn none_body_mode_ignores_stale_body_content() {
        let mut request = RequestDraft::new("POST", "http://placeholder.invalid");
        request.body_mode = BodyMode::None;
        request.body = "must not be sent".to_owned();

        let captured = send_and_capture(request);
        let (headers, body) = captured_request_parts(&captured);
        assert!(!headers.to_ascii_lowercase().contains("content-type:"));
        assert!(body.is_empty());
    }

    #[test]
    fn urlencoded_body_encodes_enabled_text_fields_in_order() {
        let mut disabled_file = BodyField::file("ignored", "/does/not/exist");
        disabled_file.enabled = false;
        let mut request = RequestDraft::new("POST", "http://placeholder.invalid");
        request.body_mode = BodyMode::FormUrlEncoded;
        request.body_fields = vec![
            BodyField::text("space", "a b"),
            BodyField::text("", "blank keys are ignored"),
            BodyField::text("plus", "+"),
            disabled_file,
        ];

        let captured = send_and_capture(request);
        let (headers, body) = captured_request_parts(&captured);
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded\r\n")
        );
        assert_eq!(body, b"space=a+b&plus=%2B");
    }

    #[test]
    fn urlencoded_body_treats_retained_file_fields_as_text() {
        let mut request = RequestDraft::new("POST", "http://placeholder.invalid");
        request.body_mode = BodyMode::FormUrlEncoded;
        request.body_fields = vec![BodyField::file("attachment", "/tmp/file name.txt")];

        let captured = send_and_capture(request);
        let (_, body) = captured_request_parts(&captured);
        assert_eq!(body, b"attachment=%2Ftmp%2Ffile+name.txt");
    }

    #[test]
    fn multipart_body_sends_text_and_file_parts_with_a_generated_boundary() {
        let mut file = tempfile::NamedTempFile::new().expect("create multipart fixture");
        file.write_all(b"file payload")
            .expect("write multipart fixture");
        let file_name = file
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let mut empty_file = BodyField::file("empty", "");
        empty_file.enabled = true;
        let mut disabled_file = BodyField::file("disabled", "/does/not/exist");
        disabled_file.enabled = false;
        let mut request = RequestDraft::new("POST", "http://placeholder.invalid");
        request.body_mode = BodyMode::MultipartFormData;
        request.headers = vec![
            HeaderEntry::new("Content-Type", "text/plain"),
            HeaderEntry::new("X-Custom", "preserved"),
        ];
        request.body_fields = vec![
            BodyField::text("note", "hello"),
            BodyField::file("attachment", file.path().to_string_lossy()),
            empty_file,
            disabled_file,
        ];

        let captured = send_and_capture(request);
        let (headers, body) = captured_request_parts(&captured);
        let headers_lower = headers.to_ascii_lowercase();
        let content_type_lines = headers_lower
            .lines()
            .filter(|line| line.starts_with("content-type:"))
            .collect::<Vec<_>>();
        assert_eq!(content_type_lines.len(), 1);
        assert!(content_type_lines[0].contains("multipart/form-data; boundary="));
        assert!(!content_type_lines[0].contains("text/plain"));
        assert!(headers_lower.contains("x-custom: preserved\r\n"));

        let boundary = content_type_lines[0]
            .split_once("boundary=")
            .map(|(_, boundary)| boundary)
            .expect("multipart boundary");
        let body = String::from_utf8_lossy(body);
        assert!(body.starts_with(&format!("--{boundary}\r\n")));
        assert!(body.contains("name=\"note\"\r\n\r\nhello\r\n"));
        assert!(body.contains(&format!("name=\"attachment\"; filename=\"{file_name}\"")));
        assert!(body.contains("file payload"));
        assert!(!body.contains("name=\"empty\""));
        assert!(!body.contains("name=\"disabled\""));
    }

    #[test]
    fn multipart_file_errors_include_field_and_path() {
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let missing_path = temporary.path().join("missing.txt");
        let mut request = RequestDraft::new("POST", "http://127.0.0.1:9");
        request.body_mode = BodyMode::MultipartFormData;
        request.body_fields = vec![BodyField::file(
            "attachment",
            missing_path.to_string_lossy(),
        )];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let error = runtime
            .block_on(send_request(&build_client().unwrap(), request))
            .expect_err("missing multipart file should fail");
        let message = error.to_string();
        assert!(matches!(error, RequestError::MultipartFileRead { .. }));
        assert!(message.contains("attachment"));
        assert!(message.contains("missing.txt"));
    }

    #[test]
    fn bounded_response_body_accepts_the_exact_limit() {
        let mut body = BoundedResponseBody::new(5);
        body.extend(b"he").expect("first chunk should fit");
        body.extend(b"llo").expect("exact limit should fit");

        assert_eq!(body.into_bytes(), b"hello");
    }

    #[test]
    fn bounded_response_body_rejects_before_growing_or_appending() {
        let mut body = BoundedResponseBody::new(5);
        body.extend(b"four").expect("first chunk should fit");
        let len_before = body.bytes.len();
        let capacity_before = body.bytes.capacity();

        let error = body
            .extend(b"!!")
            .expect_err("chunk crossing the limit must fail");

        assert!(matches!(
            error,
            RequestError::ResponseBodyTooLarge { limit_bytes: 5 }
        ));
        assert_eq!(body.bytes.len(), len_before);
        assert_eq!(body.bytes.capacity(), capacity_before);
        assert_eq!(body.bytes, b"four");
    }

    #[test]
    fn bounded_reader_rejects_an_oversized_declared_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).expect("read request headers");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef",
                )
                .expect("write test response");
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_client().unwrap();
        let response = runtime
            .block_on(async { client.get(format!("http://{address}")).send().await })
            .expect("receive response headers");
        let error = runtime
            .block_on(read_response_body(response, 5))
            .expect_err("declared oversized response must fail");

        assert!(matches!(
            error,
            RequestError::ResponseBodyTooLarge { limit_bytes: 5 }
        ));
        server.join().unwrap();
    }

    #[test]
    fn bounded_reader_rejects_an_oversized_chunked_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).expect("read request headers");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n",
                )
                .expect("write test response");
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_client().unwrap();
        let response = runtime
            .block_on(async { client.get(format!("http://{address}")).send().await })
            .expect("receive response headers");
        assert_eq!(response.content_length(), None);
        let error = runtime
            .block_on(read_response_body(response, 5))
            .expect_err("streamed oversized response must fail");

        assert!(matches!(
            error,
            RequestError::ResponseBodyTooLarge { limit_bytes: 5 }
        ));
        server.join().unwrap();
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
            ..RequestDraft::default()
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
        assert!(received.contains(concat!(
            "user-agent: resolved/",
            env!("CARGO_PKG_VERSION"),
            "\r\n"
        )));
        assert!(received.contains("x-test-request: yes\r\n"));
        assert!(received.ends_with("hello from the client"));
        assert_eq!(response.status, 201);
        assert_eq!(response.status_text, "Created");
        assert_eq!(response.content_type.as_deref(), Some("application/json"));
        assert_eq!(response.body.as_ref(), br#"{"received":true}"#);
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

    #[test]
    fn cloned_responses_share_buffered_body_storage() {
        const BODY_LEN: usize = 8 * 1024 * 1024;
        let mut buffered_body = Vec::with_capacity(BODY_LEN + 4096);
        buffered_body.resize(BODY_LEN, 0x5a);
        assert!(buffered_body.capacity() > buffered_body.len());
        let buffered_body_ptr = buffered_body.as_ptr();
        let shared_body = Bytes::from(buffered_body);
        assert_eq!(buffered_body_ptr, shared_body.as_ptr());

        let response = ResponseData {
            status: 200,
            status_text: "OK".to_owned(),
            http_version: "HTTP/2".to_owned(),
            final_url: "https://example.test/large".to_owned(),
            headers: Vec::new(),
            content_type: Some("application/octet-stream".to_owned()),
            body: shared_body,
            duration: Duration::from_millis(1),
        };

        let first = response.clone();
        let second = response.clone();

        assert_eq!(response.body.len(), BODY_LEN);
        assert_eq!(response.body.as_ptr(), first.body.as_ptr());
        assert_eq!(response.body.as_ptr(), second.body.as_ptr());
    }
}

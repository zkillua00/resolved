use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use bytes::Bytes;

use super::{
    BodyMode, RequestDraft, ResponseData,
    request::ResponseHeader,
    template::{redact_secret_bytes, redact_secret_values},
};

pub const DEFAULT_HISTORY_LIMIT: usize = 100;
pub const REDACTED_VALUE: &str = "[REDACTED]";
#[cfg(test)]
const HISTORY_FILE_VERSION: u32 = 1;

#[cfg(test)]
use anyhow::{Context, Result};
#[cfg(test)]
use std::fs::{self, File};
#[cfg(test)]
use std::io::{self, Write};
#[cfg(test)]
use std::path::{Path, PathBuf};

static NEXT_HISTORY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResponseSummary {
    pub status: u16,
    pub status_text: String,
    pub duration_ms: u64,
    pub size_bytes: usize,
    pub content_type: Option<String>,
}

impl From<&ResponseData> for ResponseSummary {
    fn from(response: &ResponseData) -> Self {
        Self {
            status: response.status,
            status_text: response.status_text.clone(),
            duration_ms: response.duration.as_millis().min(u128::from(u64::MAX)) as u64,
            size_bytes: response.size_bytes(),
            content_type: response.content_type.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub request: RequestDraft,
    pub response: Option<ResponseSummary>,
    pub error: Option<String>,
}

impl HistoryEntry {
    #[cfg(test)]
    pub fn completed(request: &RequestDraft, response: &ResponseData) -> Self {
        Self::completed_with_secrets(request, response, &[])
    }

    pub fn completed_with_secrets(
        request: &RequestDraft,
        response: &ResponseData,
        sensitive_values: &[String],
    ) -> Self {
        Self::new(
            request,
            Some(ResponseSummary::from(response)),
            None,
            sensitive_values,
        )
    }

    pub fn failed_with_secrets(
        request: &RequestDraft,
        error: impl Into<String>,
        sensitive_values: &[String],
    ) -> Self {
        Self::new(request, None, Some(error.into()), sensitive_values)
    }

    fn new(
        request: &RequestDraft,
        response: Option<ResponseSummary>,
        error: Option<String>,
        sensitive_values: &[String],
    ) -> Self {
        let created_at = Utc::now();
        let sequence = NEXT_HISTORY_ID.fetch_add(1, Ordering::Relaxed);
        let id = format!(
            "{}-{}-{sequence}",
            created_at.timestamp_micros(),
            std::process::id()
        );

        Self {
            id,
            created_at,
            request: redact_request(request, sensitive_values),
            response,
            error: error.map(|error| redact_secret_values(&error, sensitive_values)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RequestHistory {
    entries: Vec<HistoryEntry>,
    max_entries: usize,
}

impl Default for RequestHistory {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_LIMIT)
    }
}

impl RequestHistory {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries: max_entries.max(1),
        }
    }

    pub fn from_entries(entries: Vec<HistoryEntry>, max_entries: usize) -> Self {
        let mut history = Self {
            entries,
            max_entries: max_entries.max(1),
        };
        history.entries.truncate(history.max_entries);
        history
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Insert newest-first and enforce the configured bound.
    pub fn push(&mut self, entry: HistoryEntry) {
        self.entries.insert(0, entry);
        self.entries.truncate(self.max_entries);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
#[derive(Debug, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    #[serde(default)]
    entries: Vec<HistoryEntry>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct HistoryStore {
    path: PathBuf,
    max_entries: usize,
}

#[cfg(test)]
impl Default for HistoryStore {
    fn default() -> Self {
        Self::new(default_history_path(), DEFAULT_HISTORY_LIMIT)
    }
}

#[cfg(test)]
impl HistoryStore {
    pub fn new(path: impl Into<PathBuf>, max_entries: usize) -> Self {
        Self {
            path: path.into(),
            max_entries: max_entries.max(1),
        }
    }

    pub fn load(&self) -> Result<RequestHistory> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(RequestHistory::new(self.max_entries));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read history from {}", self.path.display()));
            }
        };

        let file: HistoryFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse history from {}", self.path.display()))?;
        anyhow::ensure!(
            file.version == HISTORY_FILE_VERSION,
            "unsupported history file version {} in {}",
            file.version,
            self.path.display()
        );

        Ok(RequestHistory::from_entries(file.entries, self.max_entries))
    }

    pub fn save(&self, history: &RequestHistory) -> Result<()> {
        let parent = self
            .path
            .parent()
            .context("history path must have a parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create history directory {}", parent.display()))?;

        let file = HistoryFile {
            version: HISTORY_FILE_VERSION,
            entries: history
                .entries()
                .iter()
                .take(self.max_entries)
                .cloned()
                .collect(),
        };
        let json = serde_json::to_vec_pretty(&file).context("serialize request history")?;
        let temporary_path = temporary_path_for(&self.path);

        let mut temporary_file = File::create(&temporary_path)
            .with_context(|| format!("create {}", temporary_path.display()))?;
        temporary_file
            .write_all(&json)
            .with_context(|| format!("write {}", temporary_path.display()))?;
        temporary_file
            .sync_all()
            .with_context(|| format!("sync {}", temporary_path.display()))?;
        drop(temporary_file);

        fs::rename(&temporary_path, &self.path).with_context(|| {
            format!(
                "replace history file {} with {}",
                self.path.display(),
                temporary_path.display()
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
fn default_history_path() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("API Tester")
        .join("history.json")
}

#[cfg(test)]
fn temporary_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("history.json");
    path.with_file_name(format!(".{file_name}.tmp"))
}

fn redact_request(request: &RequestDraft, sensitive_values: &[String]) -> RequestDraft {
    let known_secrets = request_sensitive_values(request, sensitive_values, false);
    redact_request_with_known_secrets(request, &known_secrets)
}

pub(crate) fn request_for_shared_history(
    request: &RequestDraft,
    sensitive_values: &[String],
) -> (RequestDraft, Vec<String>) {
    let known_secrets = request_sensitive_values(request, sensitive_values, true);
    let mut redacted = redact_request_with_known_secrets(request, &known_secrets);
    redacted
        .headers
        .retain(|header| header.enabled && header.shared);
    match redacted.body_mode {
        BodyMode::None => {
            redacted.body.clear();
            redacted.body_fields.clear();
        }
        BodyMode::Raw => redacted.body_fields.clear(),
        BodyMode::FormUrlEncoded | BodyMode::MultipartFormData => {
            redacted.body.clear();
            redacted.body_fields.retain(|field| field.enabled);
            for field in &mut redacted.body_fields {
                if field.kind == super::BodyFieldKind::File {
                    field.value.clear();
                }
            }
        }
    }
    (redacted, known_secrets)
}

pub(crate) fn response_for_shared_history(
    response: &ResponseData,
    sensitive_values: &[String],
) -> ResponseData {
    let mut known_secrets = sensitive_values.to_vec();
    for header in &response.headers {
        if is_sensitive_header(&header.name) && !header.value.is_empty() {
            push_header_secrets(&mut known_secrets, &header.value);
        }
    }
    let headers = response
        .headers
        .iter()
        .map(|header| ResponseHeader {
            name: redact_secret_values(&header.name, &known_secrets),
            value: if is_sensitive_header(&header.name) {
                REDACTED_VALUE.to_owned()
            } else {
                redact_secret_values(&header.value, &known_secrets)
            },
        })
        .collect();
    let body = Bytes::from(redact_secret_bytes(&response.body, &known_secrets));
    ResponseData {
        status: response.status,
        status_text: redact_secret_values(&response.status_text, &known_secrets),
        http_version: redact_secret_values(&response.http_version, &known_secrets),
        final_url: redact_url(&response.final_url, &known_secrets),
        headers,
        content_type: response
            .content_type
            .as_deref()
            .map(|value| redact_secret_values(value, &known_secrets)),
        body,
        duration: response.duration,
    }
}

fn request_sensitive_values(
    request: &RequestDraft,
    sensitive_values: &[String],
    include_unshared: bool,
) -> Vec<String> {
    let mut known_secrets = sensitive_values.to_vec();
    for header in &request.headers {
        if (is_sensitive_header(&header.name) || include_unshared && !header.shared)
            && !header.value.is_empty()
        {
            push_header_secrets(&mut known_secrets, &header.value);
        }
    }
    known_secrets
}

fn push_header_secrets(known_secrets: &mut Vec<String>, value: &str) {
    known_secrets.push(value.to_owned());
    if let Some((scheme, credential)) = value.split_once(' ')
        && matches!(
            scheme.to_ascii_lowercase().as_str(),
            "bearer" | "basic" | "token"
        )
        && !credential.is_empty()
    {
        known_secrets.push(credential.to_owned());
    }
}

fn redact_request_with_known_secrets(
    request: &RequestDraft,
    known_secrets: &[String],
) -> RequestDraft {
    let mut redacted = request.clone();

    redacted.headers = redacted
        .headers
        .into_iter()
        .map(|mut header| {
            if is_sensitive_header(&header.name) {
                header.value = REDACTED_VALUE.to_owned();
            } else {
                header.value = redact_secret_values(&header.value, known_secrets);
            }
            header
        })
        .collect();
    redacted.body_fields = redacted
        .body_fields
        .into_iter()
        .map(|mut field| {
            let sensitive_name = is_sensitive_field(&field.name);
            field.name = redact_secret_values(&field.name, known_secrets);
            field.value = if sensitive_name {
                REDACTED_VALUE.to_owned()
            } else {
                redact_secret_values(&field.value, known_secrets)
            };
            field
        })
        .collect();
    redacted.url = redact_url(&request.url, known_secrets);
    redacted.body = redact_body(request, known_secrets);
    redacted
}

fn redact_url(value: &str, sensitive_values: &[String]) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return redact_secret_values(value, sensitive_values);
    };

    if !url.username().is_empty() {
        let _ = url.set_username(REDACTED_VALUE);
    }
    if url.password().is_some() {
        let _ = url.set_password(Some(REDACTED_VALUE));
    }
    if url.query().is_some() {
        let pairs = url
            .query_pairs()
            .map(|(key, value)| {
                let value = if is_sensitive_field(&key) {
                    REDACTED_VALUE.to_owned()
                } else {
                    redact_secret_values(&value, sensitive_values)
                };
                (key.into_owned(), value)
            })
            .collect::<Vec<_>>();
        url.set_query(None);
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(&key, &value);
        }
    }

    redact_secret_values(url.as_str(), sensitive_values)
}

fn redact_body(request: &RequestDraft, sensitive_values: &[String]) -> String {
    if request.body.is_empty() {
        return String::new();
    }

    if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&request.body)
        && redact_json_value(&mut json)
    {
        let serialized = if request.body.contains('\n') {
            serde_json::to_string_pretty(&json)
        } else {
            serde_json::to_string(&json)
        };
        if let Ok(serialized) = serialized {
            return redact_secret_values(&serialized, sensitive_values);
        }
    }

    let is_form = request.headers.iter().any(|header| {
        header.enabled
            && header.name.eq_ignore_ascii_case("content-type")
            && header
                .value
                .to_ascii_lowercase()
                .contains("application/x-www-form-urlencoded")
    });
    if is_form {
        let pairs = url::form_urlencoded::parse(request.body.as_bytes())
            .map(|(key, value)| {
                let value = if is_sensitive_field(&key) {
                    REDACTED_VALUE.to_owned()
                } else {
                    redact_secret_values(&value, sensitive_values)
                };
                (key.into_owned(), value)
            })
            .collect::<Vec<_>>();
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(&key, &value);
        }
        return serializer.finish();
    }

    redact_secret_values(&request.body, sensitive_values)
}

fn redact_json_value(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => {
            let mut changed = false;
            for (key, value) in object {
                if is_sensitive_field(key) {
                    *value = serde_json::Value::String(REDACTED_VALUE.to_owned());
                    changed = true;
                } else {
                    changed |= redact_json_value(value);
                }
            }
            changed
        }
        serde_json::Value::Array(values) => {
            let mut changed = false;
            for value in values {
                changed |= redact_json_value(value);
            }
            changed
        }
        _ => false,
    }
}

fn is_sensitive_header(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "x-api-key"
            | "api-key"
            | "x-auth-token"
            | "set-cookie"
            | "www-authenticate"
            | "proxy-authenticate"
    ) || normalized.contains("token")
        || normalized.contains("secret")
        || normalized.ends_with("-api-key")
}

fn is_sensitive_field(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase().replace('-', "_");
    matches!(
        normalized.as_str(),
        "authorization"
            | "auth"
            | "password"
            | "passwd"
            | "pwd"
            | "api_key"
            | "apikey"
            | "access_key"
            | "private_key"
            | "client_secret"
    ) || normalized.contains("token")
        || normalized.contains("secret")
        || normalized.ends_with("_password")
        || normalized.ends_with("_api_key")
}

#[cfg(test)]
mod tests {
    use super::super::{BodyField, BodyMode, HeaderEntry};
    use super::*;
    use std::time::Duration;

    fn response(status: u16) -> ResponseData {
        ResponseData {
            status,
            status_text: "OK".to_owned(),
            http_version: "HTTP/2".to_owned(),
            final_url: "https://example.com".to_owned(),
            headers: Vec::new(),
            content_type: Some("application/json".to_owned()),
            body: br#"{"ok":true}"#.to_vec().into(),
            duration: Duration::from_millis(42),
        }
    }

    #[test]
    fn history_redacts_sensitive_header_values() {
        let request = RequestDraft {
            method: "GET".to_owned(),
            url: "https://example.com".to_owned(),
            headers: vec![
                HeaderEntry::new("Authorization", "Bearer secret"),
                HeaderEntry::new("X-Custom-Token", "secret"),
                HeaderEntry::new("Accept", "application/json"),
            ],
            body: String::new(),
            ..RequestDraft::default()
        };

        let entry = HistoryEntry::completed(&request, &response(200));
        assert_eq!(entry.request.headers[0].value, REDACTED_VALUE);
        assert_eq!(entry.request.headers[1].value, REDACTED_VALUE);
        assert_eq!(entry.request.headers[2].value, "application/json");
        assert_eq!(
            request.headers[0].value, "Bearer secret",
            "redaction must not mutate the active request"
        );
    }

    #[test]
    fn history_redacts_known_secrets_and_sensitive_url_and_json_fields() {
        let request = RequestDraft {
            method: "POST".to_owned(),
            url: "https://user:url-pass@example.com/items?api_key=literal-key&value=rotated%20secret"
                .to_owned(),
            headers: vec![HeaderEntry::new("Authorization", "Bearer header-token")],
            body: r#"{"token":"literal-body","nested":{"password":"body-pass"},"echo":"rotated secret"}"#
                .to_owned(),
            ..RequestDraft::default()
        };

        let entry = HistoryEntry::failed_with_secrets(
            &request,
            "request failed for rotated%20secret",
            &["rotated secret".to_owned()],
        );
        let serialized = serde_json::to_string(&entry).unwrap();

        for secret in [
            "literal-key",
            "literal-body",
            "url-pass",
            "body-pass",
            "header-token",
            "rotated secret",
            "rotated%20secret",
        ] {
            assert!(
                !serialized.contains(secret),
                "history leaked secret spelling {secret:?}: {serialized}"
            );
        }
        assert_eq!(entry.request.headers[0].value, REDACTED_VALUE);
        assert!(entry.request.body.contains(REDACTED_VALUE));
    }

    #[test]
    fn history_redacts_sensitive_form_fields() {
        let request = RequestDraft {
            method: "POST".to_owned(),
            url: "https://example.com/login".to_owned(),
            headers: vec![HeaderEntry::new(
                "Content-Type",
                "application/x-www-form-urlencoded",
            )],
            body: "username=alice&password=hunter2&access_token=abc".to_owned(),
            ..RequestDraft::default()
        };

        let entry = HistoryEntry::completed(&request, &response(200));
        assert!(!entry.request.body.contains("hunter2"));
        assert!(!entry.request.body.contains("abc"));
        assert!(entry.request.body.contains("%5BREDACTED%5D"));
    }

    #[test]
    fn history_redacts_structured_body_fields() {
        let mut request = RequestDraft::new("POST", "https://example.com/login");
        request.body_mode = BodyMode::FormUrlEncoded;
        request.body_fields = vec![
            BodyField::text("username", "alice"),
            BodyField::text("password", "hunter2"),
            BodyField::text("note", "rotated secret"),
        ];

        let entry = HistoryEntry::completed_with_secrets(
            &request,
            &response(200),
            &["rotated secret".to_owned()],
        );
        let serialized = serde_json::to_string(&entry.request).unwrap();

        assert!(serialized.contains("alice"));
        assert!(!serialized.contains("hunter2"));
        assert!(!serialized.contains("rotated secret"));
        assert_eq!(entry.request.body_fields[1].value, REDACTED_VALUE);
        assert_eq!(entry.request.body_fields[2].value, REDACTED_VALUE);
    }

    #[test]
    fn shared_history_omits_explicitly_private_headers_and_scrubs_their_values() {
        let private_value = "partner-credential-value";
        let mut disabled_field = BodyField::text("disabled", "not-sent");
        disabled_field.enabled = false;
        let request = RequestDraft {
            method: "POST".to_owned(),
            url: format!("https://example.com/items?echo={private_value}"),
            headers: vec![
                HeaderEntry {
                    enabled: true,
                    shared: false,
                    name: "X-Partner-Credential".to_owned(),
                    value: private_value.to_owned(),
                },
                HeaderEntry::new("Authorization", "Bearer standard-token"),
                HeaderEntry::new("Accept", "application/json"),
                HeaderEntry {
                    enabled: false,
                    shared: true,
                    name: "X-Disabled".to_owned(),
                    value: "not-sent".to_owned(),
                },
            ],
            body: format!(r#"{{"echo":"{private_value}"}}"#),
            body_mode: BodyMode::MultipartFormData,
            body_fields: vec![
                BodyField::text("echo", private_value),
                BodyField::file("upload", "/private/customer-contract.pdf"),
                disabled_field,
            ],
            ..RequestDraft::default()
        };
        let mut response = response(200);
        response.final_url = format!("https://example.com/items?echo={private_value}");
        response.headers = vec![ResponseHeader {
            name: "X-Echo".to_owned(),
            value: private_value.to_owned(),
        }];
        let mut binary_body = vec![0xff, 0x00];
        binary_body.extend_from_slice(private_value.as_bytes());
        response.body = Bytes::from(binary_body);

        let (shared_request, secrets) = request_for_shared_history(&request, &[]);
        let shared_response = response_for_shared_history(&response, &secrets);
        let serialized = serde_json::to_string(&shared_request).unwrap();
        let shared_response_text = String::from_utf8_lossy(&shared_response.body);

        assert!(!serialized.contains("X-Partner-Credential"));
        assert!(!serialized.contains(private_value));
        assert!(!serialized.contains("standard-token"));
        assert!(!serialized.contains("/private/customer-contract.pdf"));
        assert!(!shared_response.final_url.contains(private_value));
        assert!(!shared_response.headers[0].value.contains(private_value));
        assert!(!shared_response_text.contains(private_value));
        assert_eq!(shared_request.headers[0].value, REDACTED_VALUE);
        assert_eq!(shared_request.headers[1].value, "application/json");
        assert_eq!(shared_request.headers.len(), 2);
        assert_eq!(shared_request.body_fields[1].value, "");
        assert_eq!(shared_request.body_fields.len(), 2);
        assert!(shared_response_text.contains(REDACTED_VALUE));
    }

    #[test]
    fn history_keeps_newest_entries_within_bound() {
        let request = RequestDraft::new("GET", "https://example.com");
        let mut history = RequestHistory::new(2);
        let first = HistoryEntry::completed(&request, &response(200));
        let first_id = first.id.clone();
        history.push(first);
        history.push(HistoryEntry::completed(&request, &response(201)));
        history.push(HistoryEntry::completed(&request, &response(202)));

        assert_eq!(history.len(), 2);
        assert!(!history.entries().iter().any(|entry| entry.id == first_id));
        assert_eq!(history.entries()[0].response.as_ref().unwrap().status, 202);
    }

    #[test]
    fn history_store_round_trips() {
        let directory = std::env::temp_dir().join(format!(
            "api-tester-history-test-{}-{}",
            std::process::id(),
            NEXT_HISTORY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let path = directory.join("history.json");
        let store = HistoryStore::new(&path, 5);
        let request = RequestDraft::new("POST", "https://example.com/items");
        let mut history = RequestHistory::new(5);
        history.push(HistoryEntry::completed(&request, &response(201)));

        store.save(&history).expect("history should save");
        let loaded = store.load().expect("history should load");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.entries()[0].request.method, "POST");
        assert_eq!(
            loaded.entries()[0].response.as_ref().unwrap().duration_ms,
            42
        );

        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(directory);
    }
}

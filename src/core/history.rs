use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{RequestDraft, ResponseData};

pub const DEFAULT_HISTORY_LIMIT: usize = 100;
pub const REDACTED_VALUE: &str = "[REDACTED]";
const HISTORY_FILE_VERSION: u32 = 1;

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
    pub fn completed(request: &RequestDraft, response: &ResponseData) -> Self {
        Self::new(request, Some(ResponseSummary::from(response)), None)
    }

    pub fn failed(request: &RequestDraft, error: impl Into<String>) -> Self {
        Self::new(request, None, Some(error.into()))
    }

    fn new(
        request: &RequestDraft,
        response: Option<ResponseSummary>,
        error: Option<String>,
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
            request: redact_request(request),
            response,
            error,
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

#[derive(Debug, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    #[serde(default)]
    entries: Vec<HistoryEntry>,
}

#[derive(Clone, Debug)]
pub struct HistoryStore {
    path: PathBuf,
    max_entries: usize,
}

impl Default for HistoryStore {
    fn default() -> Self {
        Self::new(default_history_path(), DEFAULT_HISTORY_LIMIT)
    }
}

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

fn default_history_path() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("API Tester")
        .join("history.json")
}

fn temporary_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("history.json");
    path.with_file_name(format!(".{file_name}.tmp"))
}

fn redact_request(request: &RequestDraft) -> RequestDraft {
    let mut redacted = request.clone();
    redacted.headers = redacted
        .headers
        .into_iter()
        .map(|mut header| {
            if is_sensitive_header(&header.name) {
                header.value = REDACTED_VALUE.to_owned();
            }
            header
        })
        .collect();
    redacted
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
    ) || normalized.contains("token")
        || normalized.contains("secret")
        || normalized.ends_with("-api-key")
}

#[cfg(test)]
mod tests {
    use super::super::HeaderEntry;
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
            body: br#"{"ok":true}"#.to_vec(),
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

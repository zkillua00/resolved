//! Workspace-isolated cookie storage. Only encrypted snapshots reach disk.
use super::CredentialVault;
use cookie_store::CookieStore as RfcCookieStore;
use reqwest::{cookie::CookieStore as ReqwestCookieStore, header::HeaderValue};
use serde::{Deserialize, Serialize};
use std::{io::Cursor, sync::RwLock};
use url::Url;
use zeroize::Zeroizing;

pub struct CookieJar {
    id: String,
    vault: CredentialVault,
    state: RwLock<State>,
    remote: Option<RemoteStorage>,
}
struct State {
    store: RfcCookieStore,
    enabled: bool,
    blocked: bool,
    warning: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    enabled: bool,
    cookies: serde_json::Value,
}
#[derive(Clone)]
pub struct CookieEntry {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub origin: String,
    pub header: String,
}
impl CookieJar {
    pub fn load(vault: CredentialVault, id: impl Into<String>) -> Result<Self, String> {
        let id = id.into();
        let saved = vault.load_cookie_jar(&id).map_err(|e| e.to_string())?;
        let (store, enabled) = match saved {
            Some(bytes) => {
                // Read the original array format as well as snapshots with preferences.
                let value: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                let (cookies, enabled) = if value.is_array() {
                    (value, true)
                } else {
                    let snapshot: Snapshot =
                        serde_json::from_value(value).map_err(|e| e.to_string())?;
                    (snapshot.cookies, snapshot.enabled)
                };
                let bytes =
                    Zeroizing::new(serde_json::to_vec(&cookies).map_err(|e| e.to_string())?);
                (
                    cookie_store::serde::json::load(Cursor::new(bytes.as_slice()))
                        .map_err(|e| e.to_string())?,
                    enabled,
                )
            }
            None => (RfcCookieStore::default(), true),
        };
        Ok(Self {
            id,
            vault,
            remote: None,
            state: RwLock::new(State {
                store,
                enabled,
                blocked: false,
                warning: None,
            }),
        })
    }
    /// Preserve unreadable storage until the user explicitly resets it. The app
    /// remains usable, but never sends or silently overwrites an unreadable jar.
    pub fn open(vault: CredentialVault, id: impl Into<String>) -> Self {
        let id = id.into();
        Self::load(vault.clone(), id.clone()).unwrap_or_else(|error| Self {
            id,
            vault,
            remote: None,
            state: RwLock::new(State {
                store: RfcCookieStore::default(),
                enabled: false,
                blocked: true,
                warning: Some(format!(
                    "Cookie storage could not be opened: {error}. Reset the jar to recover."
                )),
            }),
        })
    }
    fn persist(&self, store: &RfcCookieStore, enabled: bool) -> Result<(), String> {
        if let Some(remote) = &self.remote {
            return remote.enqueue(RemoteJob::Save(remote_snapshot(store, enabled), false));
        }
        let mut cookies = Zeroizing::new(Vec::new());
        cookie_store::serde::json::save_incl_expired_and_nonpersistent(store, &mut *cookies)
            .map_err(|e| e.to_string())?;
        let snapshot = Snapshot {
            enabled,
            cookies: serde_json::from_slice(&cookies).map_err(|e| e.to_string())?,
        };
        let bytes = Zeroizing::new(serde_json::to_vec(&snapshot).map_err(|e| e.to_string())?);
        self.vault
            .store_cookie_jar(&self.id, &bytes)
            .map_err(|e| e.to_string())
    }
    pub fn enabled(&self) -> bool {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .enabled
    }
    pub fn has_unsaved_changes(&self) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|remote| remote.status.borrow().dirty)
    }
    pub fn syncing(&self) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|remote| remote.status.borrow().pending > 0)
    }
    pub fn warning(&self) -> Option<String> {
        if let Some(remote) = &self.remote {
            let status = remote.status.borrow();
            if let Some(error) = &status.error {
                return Some(error.clone());
            }
        }
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .warning
            .clone()
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.blocked {
            return Err("Reset the unreadable jar before enabling cookies.".into());
        }
        self.persist(&state.store, enabled)?;
        state.enabled = enabled;
        state.warning = None;
        Ok(())
    }
    pub fn clear(&self) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let store = RfcCookieStore::default();
        if let Some(remote) = &self.remote {
            remote.enqueue(RemoteJob::Save(
                remote_snapshot(&store, state.enabled),
                true,
            ))?;
        } else {
            self.persist(&store, state.enabled)?;
        }
        state.store = store;
        state.blocked = false;
        state.warning = None;
        Ok(())
    }
    pub fn entries(&self) -> Vec<CookieEntry> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut entries = state
            .store
            .iter_unexpired()
            .map(|cookie| {
                let domain = cookie.domain.as_cow().unwrap_or_default();
                let domain = domain.as_ref();
                let path: &str = cookie.path.as_ref();
                CookieEntry {
                    domain: domain.into(),
                    path: path.into(),
                    name: cookie.name().into(),
                    origin: format!(
                        "{}://{}{}",
                        if cookie.secure().unwrap_or(false) {
                            "https"
                        } else {
                            "http"
                        },
                        domain,
                        path
                    ),
                    header: normalized_cookie(cookie),
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
        entries
    }
    pub fn edit(
        &self,
        old: Option<&CookieEntry>,
        origin: &str,
        header: &str,
    ) -> Result<(), String> {
        let url = Url::parse(origin).map_err(|_| "Enter a valid HTTP or HTTPS origin URL.")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("Enter a credential-free HTTP or HTTPS origin URL.".into());
        }
        if header.len() > 4096 || HeaderValue::from_str(header).is_err() {
            return Err("Enter a valid Set-Cookie value of at most 4096 bytes.".into());
        }
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.blocked {
            return Err("Reset the unreadable jar before editing cookies.".into());
        }
        let mut store = state.store.clone();
        if let Some(old) = old {
            store.remove(&old.domain, &old.path, &old.name);
        }
        store
            .parse(header, &url)
            .map_err(|_| "The cookie is invalid, expired, or does not match its origin.")?;
        self.persist(&store, state.enabled)?;
        state.store = store;
        state.warning = None;
        Ok(())
    }
    pub fn delete(&self, entry: &CookieEntry) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut store = state.store.clone();
        store.remove(&entry.domain, &entry.path, &entry.name);
        self.persist(&store, state.enabled)?;
        state.store = store;
        state.warning = None;
        Ok(())
    }
    #[cfg(test)]
    fn request_cookie_header(&self, url: &Url) -> Option<HeaderValue> {
        ReqwestCookieStore::cookies(self, url)
    }
}
// Preserve the effective path and absolute expiry when editing; replaying a
// stored Max-Age would otherwise extend the cookie's lifetime on every save.
fn normalized_cookie(cookie: &cookie_store::Cookie<'_>) -> String {
    let mut raw = std::ops::Deref::deref(cookie).clone();
    raw.set_path(cookie.path.as_ref().to_owned());
    raw.set_max_age(None);
    match cookie.expires {
        cookie_store::CookieExpiration::AtUtc(time) => raw.set_expires(time),
        cookie_store::CookieExpiration::SessionEnd => raw.set_expires(None),
    }
    raw.to_string()
}
impl ReqwestCookieStore for CookieJar {
    fn set_cookies(&self, headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.enabled || state.blocked {
            return;
        }
        let mut changed = false;
        for value in headers {
            if let Ok(value) = value.to_str() {
                changed |= state.store.parse(value, url).is_ok();
            }
        }
        if changed {
            state.warning = self
                .persist(&state.store, state.enabled)
                .err()
                .map(|e| format!("Cookies changed in memory but could not be saved: {e}"));
        }
    }
    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.enabled || state.blocked {
            return None;
        }
        let mut matching = state
            .store
            .iter_unexpired()
            .filter(|c| c.matches(url))
            .collect::<Vec<_>>();
        matching.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.as_ref().len()));
        let value = matching
            .into_iter()
            .map(|c| format!("{}={}", c.name(), c.value()))
            .collect::<Vec<_>>()
            .join("; ");
        if value.is_empty() {
            None
        } else {
            HeaderValue::from_str(&value).ok()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        DatabaseStore, build_client_with_cookie_jar, secure_store::COOKIE_JAR_NAMESPACE,
    };
    use reqwest::header::COOKIE;
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        sync::{Arc, mpsc},
        thread,
    };

    fn test_vault() -> (tempfile::TempDir, DatabaseStore, CredentialVault) {
        let directory = tempfile::tempdir().unwrap();
        let database = DatabaseStore::new(directory.path().join("cookies.sqlite3"));
        database.initialize().unwrap();
        let vault = CredentialVault::new(database.clone());
        (directory, database, vault)
    }

    fn set_cookie(jar: &CookieJar, value: &'static str, url: &Url) {
        let header = HeaderValue::from_static(value);
        let mut headers = std::iter::once(&header);
        ReqwestCookieStore::set_cookies(jar, &mut headers, url);
    }

    #[test]
    fn persists_encrypted_cookies_and_isolates_workspace_jars() {
        let (_directory, database, vault) = test_vault();
        let url = Url::parse("https://api.example.com/accounts").unwrap();
        let first = CookieJar::load(vault.clone(), "local:workspace-a").unwrap();
        set_cookie(&first, "session=plain-secret; Path=/; HttpOnly", &url);
        assert_eq!(
            first.request_cookie_header(&url).unwrap(),
            HeaderValue::from_static("session=plain-secret")
        );
        drop(first);

        let reloaded = CookieJar::load(vault.clone(), "local:workspace-a").unwrap();
        assert_eq!(
            reloaded.request_cookie_header(&url).unwrap(),
            HeaderValue::from_static("session=plain-secret")
        );
        let other = CookieJar::load(vault, "local:workspace-b").unwrap();
        assert!(other.request_cookie_header(&url).is_none());

        let encrypted = database
            .load_secure_value(COOKIE_JAR_NAMESPACE, "local:workspace-a")
            .unwrap()
            .unwrap();
        assert!(!String::from_utf8_lossy(&encrypted.ciphertext).contains("plain-secret"));
    }

    #[test]
    fn applies_domain_path_secure_and_expiration_rules() {
        let (_directory, _database, vault) = test_vault();
        let jar = CookieJar::load(vault, "local:workspace-a").unwrap();
        let origin = Url::parse("https://api.example.com/private/login").unwrap();
        set_cookie(&jar, "scoped=yes; Path=/private; Secure", &origin);

        assert_eq!(
            jar.request_cookie_header(
                &Url::parse("https://api.example.com/private/profile").unwrap()
            )
            .unwrap(),
            HeaderValue::from_static("scoped=yes")
        );
        assert!(
            jar.request_cookie_header(&Url::parse("http://api.example.com/private").unwrap())
                .is_none()
        );
        assert!(
            jar.request_cookie_header(&Url::parse("https://api.example.com/public").unwrap())
                .is_none()
        );

        set_cookie(&jar, "scoped=gone; Path=/private; Max-Age=0", &origin);
        assert!(jar.request_cookie_header(&origin).is_none());
    }

    #[test]
    fn reqwest_stores_response_cookies_and_explicit_headers_take_precedence() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind cookie test server");
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().expect("accept cookie test request");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 2048];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).expect("read cookie test request");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                request_tx.send(request).unwrap();
                let set_cookie = if index == 0 {
                    "Set-Cookie: session=from-jar; Path=/; HttpOnly\r\n"
                } else {
                    ""
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\n{set_cookie}Content-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });

        let (_directory, _database, vault) = test_vault();
        let jar = Arc::new(CookieJar::load(vault, "local:workspace-a").unwrap());
        let client = build_client_with_cookie_jar(jar).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let url = format!("http://{address}/cookies");
        runtime.block_on(async {
            client.get(&url).send().await.unwrap();
            client.get(&url).send().await.unwrap();
            client
                .get(&url)
                .header(COOKIE, "manual=explicit")
                .send()
                .await
                .unwrap();
        });

        let requests = (0..3)
            .map(|_| String::from_utf8_lossy(&request_rx.recv().unwrap()).into_owned())
            .collect::<Vec<_>>();
        server.join().unwrap();
        let requests = requests
            .iter()
            .map(|request| request.to_ascii_lowercase())
            .collect::<Vec<_>>();
        assert!(!requests[0].contains("cookie:"));
        assert!(requests[1].contains("cookie: session=from-jar\r\n"));
        assert!(requests[2].contains("cookie: manual=explicit\r\n"));
        assert!(!requests[2].contains("session=from-jar"));
    }
    #[test]
    fn management_and_disabled_state_survive_reload() {
        let (_directory, _database, vault) = test_vault();
        let url = Url::parse("https://api.example.com/private/resource").unwrap();
        let jar = CookieJar::load(vault.clone(), "local:manage").unwrap();
        jar.edit(
            None,
            url.as_str(),
            "session=one; Path=/private; Secure; HttpOnly",
        )
        .unwrap();
        let old = jar.entries().remove(0);
        jar.edit(
            Some(&old),
            url.as_str(),
            "session=two; Path=/private; Secure; HttpOnly",
        )
        .unwrap();
        assert_eq!(jar.request_cookie_header(&url).unwrap(), "session=two");
        assert!(jar.edit(Some(&old), url.as_str(), "invalid").is_err());
        assert_eq!(jar.request_cookie_header(&url).unwrap(), "session=two");
        jar.set_enabled(false).unwrap();
        set_cookie(&jar, "ignored=yes; Path=/", &url);
        assert!(jar.request_cookie_header(&url).is_none());
        let jar = CookieJar::load(vault.clone(), "local:manage").unwrap();
        assert!(!jar.enabled());
        assert_eq!(jar.entries().len(), 1);
        jar.set_enabled(true).unwrap();
        assert_eq!(jar.request_cookie_header(&url).unwrap(), "session=two");
        jar.delete(&jar.entries().remove(0)).unwrap();
        assert!(
            CookieJar::load(vault.clone(), "local:manage")
                .unwrap()
                .entries()
                .is_empty()
        );
        jar.edit(None, url.as_str(), "session=three; Path=/")
            .unwrap();
        jar.clear().unwrap();
        assert!(
            CookieJar::load(vault, "local:manage")
                .unwrap()
                .entries()
                .is_empty()
        );
    }

    #[test]
    fn unreadable_storage_is_preserved_until_explicit_reset() {
        let (_directory, _database, vault) = test_vault();
        vault.store_cookie_jar("broken", b"not json").unwrap();
        let jar = CookieJar::open(vault.clone(), "broken");
        assert!(!jar.enabled());
        assert!(jar.warning().is_some());
        assert!(jar.set_enabled(true).is_err());
        let url = Url::parse("https://example.com/").unwrap();
        set_cookie(&jar, "session=new", &url);
        assert_eq!(
            vault.load_cookie_jar("broken").unwrap().unwrap().as_slice(),
            b"not json"
        );
        jar.clear().unwrap();
        assert!(jar.warning().is_none());
        jar.set_enabled(true).unwrap();
        assert!(
            CookieJar::load(vault, "broken")
                .unwrap()
                .entries()
                .is_empty()
        );
    }

    #[test]
    fn persistence_failure_is_visible_and_management_is_transactional() {
        let (directory, _database, vault) = test_vault();
        let jar = CookieJar::load(vault, "failure").unwrap();
        let url = Url::parse("https://example.com/").unwrap();
        jar.edit(None, url.as_str(), "session=old; Path=/").unwrap();
        let connection =
            rusqlite::Connection::open(directory.path().join("cookies.sqlite3")).unwrap();
        connection.execute_batch("CREATE TRIGGER fail_cookie_save BEFORE INSERT ON secure_values BEGIN SELECT RAISE(ABORT, 'test storage unavailable'); END;").unwrap();
        assert!(jar.clear().is_err());
        assert_eq!(jar.request_cookie_header(&url).unwrap(), "session=old");
        assert!(jar.set_enabled(false).is_err());
        assert!(jar.enabled());
        set_cookie(&jar, "session=new; Path=/", &url);
        assert!(jar.warning().unwrap().contains("could not be saved"));
        assert_eq!(jar.request_cookie_header(&url).unwrap(), "session=new");
    }

    #[test]
    fn legacy_snapshots_and_editor_preserve_effective_expiry_and_path() {
        let (_directory, _database, vault) = test_vault();
        let url = Url::parse("https://example.com/private/login").unwrap();
        let mut legacy = RfcCookieStore::default();
        legacy
            .parse("session=old; Max-Age=60; Secure", &url)
            .unwrap();
        let mut bytes = Vec::new();
        cookie_store::serde::json::save_incl_expired_and_nonpersistent(&legacy, &mut bytes)
            .unwrap();
        vault.store_cookie_jar("legacy", &bytes).unwrap();
        let jar = CookieJar::load(vault.clone(), "legacy").unwrap();
        let entry = jar.entries().remove(0);
        assert_eq!(entry.path, "/private");
        assert!(!entry.header.contains("Max-Age"));
        assert!(entry.header.contains("Expires="));
        assert!(entry.header.contains("Path=/private"));
        jar.edit(Some(&entry), &entry.origin, &entry.header)
            .unwrap();
        assert_eq!(jar.entries()[0].header, entry.header);
        assert!(
            jar.request_cookie_header(
                &Url::parse("https://other.example.com/private/login").unwrap()
            )
            .is_none()
        );
        assert!(
            jar.request_cookie_header(&Url::parse("https://example.com/public").unwrap())
                .is_none()
        );
        set_cookie(&jar, "session=deleted; Path=/private; Max-Age=0", &url);
        assert!(
            CookieJar::load(vault, "legacy")
                .unwrap()
                .request_cookie_header(&url)
                .is_none()
        );
    }
    #[test]
    fn remote_jar_uses_server_storage_and_preserves_sync_errors() {
        use crate::core::{AppSettings, UpstreamCredential};
        let (_directory, database, vault) = test_vault();
        vault
            .store_upstream_with_settings(
                &AppSettings::default(),
                "test-server",
                &UpstreamCredential::new(
                    Zeroizing::new("test-bearer".into()),
                    chrono::Utc::now() + chrono::Duration::hours(1),
                ),
            )
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for index in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 2048];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .map(|value| value.parse::<usize>().unwrap())
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8_lossy(&bytes);
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer test-bearer")
                );
                assert!(request.starts_with(if index == 1 || index == 2 {
                    "PUT "
                } else {
                    "GET "
                }));
                let (status, body) = if index == 2 {
                    (
                        "409 Conflict",
                        serde_json::json!({"success":false,"error":{"message":"Cookie jar changed on another client. Reload it before saving."}}),
                    )
                } else {
                    (
                        "200 OK",
                        serde_json::json!({"success":true,"data":{"enabled":true,"revision":index+1,"cookies":[{"url":"https://example.com/","cookie":"session=server-secret; Path=/; Secure"}]}}),
                    )
                };
                let body = body.to_string();
                write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let jar = CookieJar::open_remote(
            vault,
            "test-server".into(),
            "workspace".into(),
            Url::parse(&format!("http://{address}/")).unwrap(),
            crate::core::build_upstream_client().unwrap(),
            runtime.handle(),
        );
        runtime.block_on(async {
            jar.synchronized().await.unwrap();
            assert_eq!(
                jar.request_cookie_header(&Url::parse("https://example.com/").unwrap())
                    .unwrap(),
                "session=server-secret"
            );
            jar.edit(None, "https://example.com/", "manual=edited; Path=/")
                .unwrap();
            jar.synchronized().await.unwrap();
            assert!(!jar.has_unsaved_changes());
            jar.set_enabled(false).unwrap();
            assert!(jar.has_unsaved_changes());
            assert!(
                jar.synchronized()
                    .await
                    .unwrap_err()
                    .contains("another client")
            );
            assert!(jar.warning().unwrap().contains("another client"));
            assert!(jar.has_unsaved_changes());
            jar.refresh().await.unwrap();
            assert!(!jar.has_unsaved_changes());
            assert!(jar.enabled());
            assert!(jar.warning().is_none());
        });
        assert!(
            database
                .load_secure_value(COOKIE_JAR_NAMESPACE, "upstream:test-server:workspace")
                .unwrap()
                .is_none()
        );
        server.join().unwrap();
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct RemoteCookie {
    url: String,
    cookie: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct RemoteSnapshot {
    enabled: bool,
    cookies: Vec<RemoteCookie>,
    #[serde(default)]
    revision: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    reset: bool,
}
#[derive(Clone, Default)]
pub struct CookieSyncStatus {
    dirty: bool,
    pending: usize,
    error: Option<String>,
}
enum RemoteJob {
    Load,
    Save(RemoteSnapshot, bool),
}
struct RemoteStorage {
    jobs: tokio::sync::mpsc::UnboundedSender<RemoteJob>,
    status: tokio::sync::watch::Sender<CookieSyncStatus>,
}
impl RemoteStorage {
    fn enqueue(&self, job: RemoteJob) -> Result<(), String> {
        self.status.send_modify(|status| {
            status.pending += 1;
            status.dirty |= matches!(&job, RemoteJob::Save(..));
        });
        if self.jobs.send(job).is_err() {
            self.status.send_modify(|status| {
                status.pending -= 1;
                status.error =
                    Some("Cookie synchronization stopped. Reopen this workspace.".into());
            });
            return Err("Cookie synchronization stopped. Reopen this workspace.".into());
        }
        Ok(())
    }
}
fn remote_snapshot(store: &RfcCookieStore, enabled: bool) -> RemoteSnapshot {
    RemoteSnapshot {
        enabled,
        revision: 0,
        reset: false,
        cookies: store
            .iter_unexpired()
            .map(|cookie| {
                let domain = cookie.domain.as_cow().unwrap_or_default();
                RemoteCookie {
                    url: format!(
                        "{}://{}{}",
                        if cookie.secure().unwrap_or(false) {
                            "https"
                        } else {
                            "http"
                        },
                        domain,
                        cookie.path.as_ref()
                    ),
                    cookie: normalized_cookie(cookie),
                }
            })
            .collect(),
    }
}
impl CookieJar {
    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }
    pub fn subscribe(&self) -> Option<tokio::sync::watch::Receiver<CookieSyncStatus>> {
        self.remote.as_ref().map(|remote| remote.status.subscribe())
    }
    pub fn reload(&self) -> Result<(), String> {
        self.remote
            .as_ref()
            .ok_or("This is a local cookie jar.")?
            .enqueue(RemoteJob::Load)
    }
    pub async fn synchronized(&self) -> Result<(), String> {
        let Some(mut status) = self.subscribe() else {
            return Ok(());
        };
        loop {
            let current = status.borrow().clone();
            if current.pending == 0 {
                return current.error.map_or(Ok(()), Err);
            }
            status
                .changed()
                .await
                .map_err(|_| "Cookie synchronization stopped.".to_string())?;
        }
    }
    pub async fn refresh(&self) -> Result<(), String> {
        if self.is_remote() {
            self.reload()?;
            self.synchronized().await?;
        }
        Ok(())
    }

    /// Remote jars never use CredentialVault for cookie payloads. It supplies
    /// only the existing bearer session; password-derived keys stay on the server.
    pub fn open_remote(
        vault: CredentialVault,
        upstream_id: String,
        workspace_id: String,
        base_url: Url,
        client: reqwest::Client,
        runtime: &tokio::runtime::Handle,
    ) -> std::sync::Arc<Self> {
        let (jobs, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let (status, _) = tokio::sync::watch::channel(CookieSyncStatus::default());
        let jar = std::sync::Arc::new(Self {
            id: format!("upstream:{upstream_id}:{workspace_id}"),
            vault: vault.clone(),
            state: RwLock::new(State {
                store: RfcCookieStore::default(),
                enabled: false,
                blocked: true,
                warning: None,
            }),
            remote: Some(RemoteStorage {
                jobs,
                status: status.clone(),
            }),
        });
        let weak = std::sync::Arc::downgrade(&jar);
        jar.reload().expect("new cookie worker is connected");
        let runtime_handle = runtime.clone();
        runtime.spawn(async move {
            let mut revision = 0;
            let mut pinned_credential = None;
            while let Some(job) = receiver.recv().await {
                let fetch = matches!(&job, RemoteJob::Load);
                let snapshot = match job {
                    RemoteJob::Load => None,
                    RemoteJob::Save(mut snapshot, reset) => {
                        snapshot.revision = revision;
                        snapshot.reset = reset;
                        Some(snapshot)
                    }
                };
                if pinned_credential.is_none() {
                    let credentials = vault.clone();
                    let upstream = upstream_id.clone();
                    if let Ok(Ok(Some(credential))) = runtime_handle
                        .spawn_blocking(move || credentials.load_upstream(&upstream))
                        .await
                    {
                        pinned_credential = Some(credential);
                    }
                }
                // Pin this jar to its opening session. A later login on the same
                // profile must never upload one user's cookies as another user.
                let result = match pinned_credential.as_ref() {
                    Some(credential) if credential.expires_at > chrono::Utc::now() => {
                        remote_request(
                            &client,
                            &base_url,
                            &workspace_id,
                            credential.bearer_token(),
                            snapshot.as_ref(),
                        )
                        .await
                    }
                    _ => Err(
                        "Log in again and reopen this workspace to unlock its cookie jar.".into(),
                    ),
                };
                let Some(jar) = weak.upgrade() else {
                    break;
                };
                let result = result.and_then(|snapshot| {
                    revision = snapshot.revision;
                    if fetch && status.borrow().pending == 1 {
                        let mut store = RfcCookieStore::default();
                        for cookie in snapshot.cookies {
                            let origin = Url::parse(&cookie.url)
                                .map_err(|_| "Server cookie origin is invalid.")?;
                            // Expired cookies are deliberately discarded on reload.
                            let _ = store.parse(&cookie.cookie, &origin);
                        }
                        let mut state = jar
                            .state
                            .write()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        *state = State {
                            store,
                            enabled: snapshot.enabled,
                            blocked: false,
                            warning: None,
                        };
                    }
                    Ok(())
                });
                status.send_modify(|status| {
                    status.pending = status.pending.saturating_sub(1);
                    if result.is_ok() && status.pending == 0 {
                        status.dirty = false;
                    }
                    status.error = result.err();
                });
            }
        });
        jar
    }
}
async fn remote_request(
    client: &reqwest::Client,
    base: &Url,
    workspace: &str,
    token: &str,
    snapshot: Option<&RemoteSnapshot>,
) -> Result<RemoteSnapshot, String> {
    let endpoint = base
        .join(&format!("api/v1/workspaces/{workspace}/cookie-jar"))
        .map_err(|e| e.to_string())?;
    let builder = if let Some(snapshot) = snapshot {
        client.put(endpoint).json(snapshot)
    } else {
        client.get(endpoint)
    };
    let mut response = builder
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if status.is_redirection() {
        return Err("The server redirected cookie storage; request refused.".into());
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(
            "This server does not support encrypted cookie jars. Update the server.".into(),
        );
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
            return Err("The server cookie response is too large.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    #[derive(Deserialize)]
    struct Envelope {
        success: bool,
        data: Option<RemoteSnapshot>,
        error: Option<RemoteError>,
    }
    #[derive(Deserialize)]
    struct RemoteError {
        message: String,
    }
    let envelope: Envelope =
        serde_json::from_slice(&bytes).map_err(|_| "The server cookie response is invalid.")?;
    if !status.is_success() || !envelope.success {
        return Err(envelope
            .error
            .map(|e| e.message)
            .unwrap_or_else(|| "Could not synchronize encrypted cookie storage.".into()));
    }
    envelope
        .data
        .ok_or_else(|| "The server omitted its cookie jar.".into())
}

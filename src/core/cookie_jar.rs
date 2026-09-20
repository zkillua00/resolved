//! Workspace-isolated cookie storage. Only encrypted snapshots reach disk.
use super::CredentialVault;
use cookie_store::CookieStore as RfcCookieStore;
use reqwest::{cookie::CookieStore as ReqwestCookieStore, header::HeaderValue};
use serde::{Deserialize, Serialize};
use std::{
    io::Cursor,
    sync::{Arc, RwLock},
};
use url::Url;
use zeroize::Zeroizing;

pub struct CookieJar {
    id: String,
    vault: CredentialVault,
    state: Arc<RwLock<State>>,
    local_status: tokio::sync::watch::Sender<CookieSyncStatus>,
    remote: Option<RemoteStorage>,
}
#[derive(Clone)]
struct State {
    store: RfcCookieStore,
    enabled: bool,
    blocked: bool,
    warning: Option<String>,
    generation: u64,
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
    pub value: String,
    pub origin: String,
    pub header: String,
}
impl CookieJar {
    /// A blocked, in-memory placeholder for isolated runners. No storage is read
    /// or written; the owner must supply its loaded jar before execution.
    pub fn uninitialized(vault: CredentialVault, id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            vault,
            remote: None,
            local_status: tokio::sync::watch::channel(CookieSyncStatus::default()).0,
            state: Arc::new(RwLock::new(State {
                store: RfcCookieStore::default(),
                enabled: false,
                blocked: true,
                warning: Some("Cookie storage has not been initialized.".to_owned()),
                generation: 0,
            })),
        }
    }

    /// Performs synchronous vault I/O. Production callers must open local jars
    /// on the shared I/O worker, after previously enqueued cookie saves.
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
            local_status: tokio::sync::watch::channel(CookieSyncStatus::default()).0,
            state: Arc::new(RwLock::new(State {
                store,
                enabled,
                blocked: false,
                warning: None,
                generation: 0,
            })),
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
            local_status: tokio::sync::watch::channel(CookieSyncStatus::default()).0,
            state: Arc::new(RwLock::new(State {
                store: RfcCookieStore::default(),
                enabled: false,
                blocked: true,
                warning: Some(format!(
                    "Cookie storage could not be opened: {error}. Reset the jar to recover."
                )),
                generation: 0,
            })),
        })
    }
    fn persist(&self, store: &RfcCookieStore, enabled: bool) -> Result<(), String> {
        if let Some(remote) = &self.remote {
            return remote.enqueue(RemoteJob::Save(remote_snapshot(store, enabled), false));
        }
        Self::persist_local(&self.vault, &self.id, store, enabled)
    }
    fn persist_local(
        vault: &CredentialVault,
        id: &str,
        store: &RfcCookieStore,
        enabled: bool,
    ) -> Result<(), String> {
        let mut cookies = Zeroizing::new(Vec::new());
        cookie_store::serde::json::save_incl_expired_and_nonpersistent(store, &mut *cookies)
            .map_err(|e| e.to_string())?;
        let snapshot = Snapshot {
            enabled,
            cookies: serde_json::from_slice(&cookies).map_err(|e| e.to_string())?,
        };
        let bytes = Zeroizing::new(serde_json::to_vec(&snapshot).map_err(|e| e.to_string())?);
        vault
            .store_cookie_jar(id, &bytes)
            .map_err(|e| e.to_string())
    }
    pub fn enabled(&self) -> bool {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .enabled
    }
    pub fn has_unsaved_changes(&self) -> bool {
        self.status().borrow().dirty
    }
    pub fn syncing(&self) -> bool {
        self.status().borrow().pending > 0
    }
    pub fn warning(&self) -> Option<String> {
        {
            let status = self.status().borrow();
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
    /// Account for an accepted management save before enqueueing it, so close
    /// protection includes jobs which have not started yet. A failure remains
    /// visible until a successful retry (or explicit reset) saves this jar.
    /// This includes validation errors: the editor stays open for correction.
    pub fn save_on_worker(
        self: &Arc<Self>,
        change: impl FnOnce(&Self) -> Result<(), String> + Send + 'static,
    ) -> crate::io::IoTask<Result<(), String>> {
        let jar = self.clone();
        if self.is_remote() {
            // RemoteStorage owns acknowledgment accounting. A worker enqueue
            // reply must not clear a server-side failure or claim durability.
            return crate::io::run(move || change(&jar));
        }
        let mut completion = LocalSaveCompletion::new(self.local_status.clone());
        crate::io::run(move || {
            let result = change(&jar);
            completion.finish(result.clone());
            result
        })
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.change(false, |state| {
            if state.blocked {
                return Err("Reset the unreadable jar before enabling cookies.".into());
            }
            state.enabled = enabled;
            Ok(())
        })
    }
    pub fn clear(&self) -> Result<(), String> {
        self.change(true, |state| {
            state.store = RfcCookieStore::default();
            state.blocked = false;
            Ok(())
        })
    }
    /// Local callers run management on the shared I/O worker. Never hold the
    /// state lock during disk access: render and network readers need it too.
    /// Retry if a response updated cookies while the candidate was saved.
    fn change(
        &self,
        reset: bool,
        edit: impl Fn(&mut State) -> Result<(), String>,
    ) -> Result<(), String> {
        loop {
            let mut candidate = self
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let generation = candidate.generation;
            edit(&mut candidate)?;
            candidate.warning = None;
            if let Some(remote) = &self.remote {
                let mut state = self
                    .state
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.generation != generation {
                    continue;
                }
                remote.enqueue(RemoteJob::Save(
                    remote_snapshot(&candidate.store, candidate.enabled),
                    reset,
                ))?;
                candidate.generation += 1;
                *state = candidate;
                return Ok(());
            }
            self.persist(&candidate.store, candidate.enabled)?;
            let mut state = self
                .state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.generation != generation {
                continue;
            }
            candidate.generation += 1;
            *state = candidate;
            self.local_status.send_modify(|status| {
                if status.pending == 0 {
                    status.dirty = false;
                }
                status.error = None;
            });
            return Ok(());
        }
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
                    value: cookie.value().into(),
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
        self.change(false, |state| {
            if state.blocked {
                return Err("Reset the unreadable jar before editing cookies.".into());
            }
            if let Some(old) = old {
                state.store.remove(&old.domain, &old.path, &old.name);
            }
            state
                .store
                .parse(header, &url)
                .map_err(|_| "The cookie is invalid, expired, or does not match its origin.")?;
            Ok(())
        })
    }
    pub fn delete(&self, entry: &CookieEntry) -> Result<(), String> {
        self.change(false, |state| {
            state.store.remove(&entry.domain, &entry.path, &entry.name);
            Ok(())
        })
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
            state.generation += 1;
            if self.remote.is_some() {
                state.warning = self.persist(&state.store, state.enabled).err();
            } else {
                // Enqueue while the mutation lock is held to preserve ordering.
                // Read the latest state on execution, not an old snapshot that
                // could overwrite a later management transaction.
                let shared = self.state.clone();
                let mut completion = LocalSaveCompletion::new(self.local_status.clone());
                let vault = self.vault.clone();
                let id = self.id.clone();
                drop(crate::io::run(move || {
                    let snapshot = shared
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    let result =
                        Self::persist_local(&vault, &id, &snapshot.store, snapshot.enabled);
                    completion.finish(result);
                }));
            }
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
        futures::executor::block_on(crate::io::flush()).unwrap();
    }

    #[test]
    fn abandoned_cookie_save_reports_failure_instead_of_staying_pending() {
        let (status, _) = tokio::sync::watch::channel(CookieSyncStatus::default());
        {
            let _completion = LocalSaveCompletion::new(status.clone());
            assert_eq!(status.borrow().pending, 1);
        }
        let status = status.borrow();
        assert_eq!(status.pending, 0);
        assert!(status.dirty);
        assert!(
            status
                .error
                .as_ref()
                .unwrap()
                .contains("could not be saved")
        );
    }

    #[test]
    fn management_submission_tracks_queued_failure_and_successful_recovery() {
        let (_directory, _database, vault) = test_vault();
        let jar = Arc::new(CookieJar::load(vault, "managed-status").unwrap());
        let (release, blocked) = mpsc::channel();
        let blocker = crate::io::run(move || blocked.recv().unwrap());
        let failed = jar.save_on_worker(|_| Err("test save failure".into()));
        assert!(jar.syncing());
        assert!(jar.has_unsaved_changes());
        release.send(()).unwrap();
        futures::executor::block_on(blocker).unwrap();
        assert!(futures::executor::block_on(failed).unwrap().is_err());
        assert!(!jar.syncing());
        assert!(jar.has_unsaved_changes());
        assert!(jar.warning().unwrap().contains("test save failure"));

        // A harmless successful save also acknowledges a failed validation
        // attempt; recovery does not require deleting/resetting cookies.
        futures::executor::block_on(jar.save_on_worker(|jar| jar.set_enabled(jar.enabled())))
            .unwrap()
            .unwrap();
        assert!(!jar.syncing());
        assert!(!jar.has_unsaved_changes());
        assert!(jar.warning().is_none());
    }

    #[test]
    fn panicking_management_job_reports_unsaved_status_and_allows_retry() {
        let (_directory, _database, vault) = test_vault();
        let jar = Arc::new(CookieJar::load(vault, "managed-panic").unwrap());
        let task = jar.save_on_worker(|_| panic!("test management panic"));
        assert!(futures::executor::block_on(task).is_err());
        assert!(!jar.syncing());
        assert!(jar.has_unsaved_changes());
        assert!(jar.warning().unwrap().contains("did not complete"));
        futures::executor::block_on(jar.save_on_worker(|jar| jar.clear()))
            .unwrap()
            .unwrap();
        assert!(!jar.has_unsaved_changes());
        assert!(jar.warning().is_none());
    }

    #[test]
    fn response_cookies_are_immediate_but_persistence_is_ordered_on_worker() {
        let (_directory, _database, vault) = test_vault();
        let jar = Arc::new(CookieJar::load(vault.clone(), "queued").unwrap());
        let url = Url::parse("https://example.com/").unwrap();
        let (release, blocked) = mpsc::channel();
        let blocker = crate::io::run(move || blocked.recv().unwrap());
        let header = HeaderValue::from_static("session=response; Path=/");
        ReqwestCookieStore::set_cookies(&*jar, &mut std::iter::once(&header), &url);
        assert_eq!(jar.request_cookie_header(&url).unwrap(), "session=response");
        assert!(jar.syncing());
        assert!(jar.has_unsaved_changes());
        assert!(vault.load_cookie_jar("queued").unwrap().is_none());
        let edited = jar.clone();
        let management = crate::io::run(move || {
            edited.edit(None, "https://example.com/", "session=managed; Path=/")
        });
        release.send(()).unwrap();
        futures::executor::block_on(blocker).unwrap();
        futures::executor::block_on(management).unwrap().unwrap();
        futures::executor::block_on(crate::io::flush()).unwrap();
        assert!(!jar.syncing());
        assert!(!jar.has_unsaved_changes());
        assert_eq!(
            CookieJar::load(vault, "queued")
                .unwrap()
                .request_cookie_header(&url)
                .unwrap(),
            "session=managed",
        );
    }

    #[test]
    fn management_does_not_hold_state_lock_during_storage_access() {
        use super::super::secure_store::{CredentialVaultError, MasterKeyProvider};
        use std::sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        };
        struct BlockingKey {
            entered: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
            blocked: AtomicBool,
        }
        impl MasterKeyProvider for BlockingKey {
            fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
                if !self.blocked.swap(true, Ordering::SeqCst) {
                    self.entered.send(()).unwrap();
                    self.release.lock().unwrap().recv().unwrap();
                }
                Ok(Zeroizing::new(vec![7; 32]))
            }
        }
        let (_directory, database, _) = test_vault();
        let (entered, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let vault = CredentialVault::with_key_provider(
            database,
            Arc::new(BlockingKey {
                entered,
                release: Mutex::new(wait),
                blocked: AtomicBool::new(false),
            }),
        );
        let jar = Arc::new(CookieJar::load(vault.clone(), "nonblocking").unwrap());
        let writer = jar.clone();
        let task = crate::io::run(move || {
            writer.edit(None, "https://example.com/", "manual=edited; Path=/")
        });
        ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let (read_done, read_result) = mpsc::channel();
        let reader = thread::spawn(move || {
            let entries = jar.entries();
            let header = HeaderValue::from_static("network=response; Path=/");
            ReqwestCookieStore::set_cookies(
                &*jar,
                &mut std::iter::once(&header),
                &Url::parse("https://example.com/").unwrap(),
            );
            let _ = read_done.send(entries);
        });
        let readable = read_result
            .recv_timeout(std::time::Duration::from_secs(1))
            .is_ok();
        release.send(()).unwrap();
        reader.join().unwrap();
        futures::executor::block_on(task).unwrap().unwrap();
        futures::executor::block_on(crate::io::flush()).unwrap();
        assert!(
            readable,
            "cookie readers and response callbacks must not wait for storage"
        );
        let entries = CookieJar::load(vault, "nonblocking").unwrap().entries();
        assert_eq!(
            entries.len(),
            2,
            "the transaction must preserve concurrent response cookies"
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "manual" && entry.value == "edited")
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "network" && entry.value == "response")
        );
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
        futures::executor::block_on(crate::io::flush()).unwrap();
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
// A callback cannot await its I/O reply. Account for both normal completion
// and a rejected/panicking job, so synchronization never remains pending forever.
struct LocalSaveCompletion {
    status: tokio::sync::watch::Sender<CookieSyncStatus>,
    finished: bool,
}
impl LocalSaveCompletion {
    fn new(status: tokio::sync::watch::Sender<CookieSyncStatus>) -> Self {
        status.send_modify(|status| {
            status.pending += 1;
            status.dirty = true;
        });
        Self {
            status,
            finished: false,
        }
    }
    fn finish(&mut self, result: Result<(), String>) {
        self.status.send_modify(|status| {
            status.pending = status.pending.saturating_sub(1);
            if result.is_ok() && status.pending == 0 {
                status.dirty = false;
            }
            status.error = result
                .err()
                .map(|error| format!("Cookie changes could not be saved: {error}"));
        });
        self.finished = true;
    }
}
impl Drop for LocalSaveCompletion {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(Err(
                "The cookie storage worker did not complete its save.".into()
            ));
        }
    }
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
    fn status(&self) -> &tokio::sync::watch::Sender<CookieSyncStatus> {
        self.remote
            .as_ref()
            .map_or(&self.local_status, |remote| &remote.status)
    }
    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }
    pub fn subscribe(&self) -> Option<tokio::sync::watch::Receiver<CookieSyncStatus>> {
        Some(self.status().subscribe())
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
            local_status: tokio::sync::watch::channel(CookieSyncStatus::default()).0,
            state: Arc::new(RwLock::new(State {
                store: RfcCookieStore::default(),
                enabled: false,
                blocked: true,
                warning: None,
                generation: 0,
            })),
            remote: Some(RemoteStorage {
                jobs,
                status: status.clone(),
            }),
        });
        let weak = std::sync::Arc::downgrade(&jar);
        jar.reload().expect("new cookie worker is connected");
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
                    if let Ok(Ok(Some(credential))) =
                        crate::io::run(move || credentials.load_upstream(&upstream)).await
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
                            generation: state.generation + 1,
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

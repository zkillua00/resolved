//! Workspace-isolated, encrypted cookie-jar persistence for request execution.

use std::{io::Cursor, sync::RwLock};

use cookie_store::CookieStore as RfcCookieStore;
use reqwest::{cookie::CookieStore as ReqwestCookieStore, header::HeaderValue};
use thiserror::Error;
use url::Url;
use zeroize::Zeroizing;

use super::{CredentialVault, CredentialVaultError};

/// A reqwest cookie provider backed by an RFC 6265 store.
///
/// Every instance belongs to exactly one workspace-provider identity. Changes
/// are encrypted and persisted immediately so session cookies survive app
/// restarts and a client created for another workspace cannot observe them.
pub struct CookieJar {
    id: String,
    store: RwLock<RfcCookieStore>,
    vault: CredentialVault,
}

#[derive(Debug, Error)]
pub enum CookieJarError {
    #[error("secure cookie-jar storage failed: {0}")]
    Vault(#[from] CredentialVaultError),
    #[error("the saved cookie jar is invalid: {0}")]
    Decode(cookie_store::Error),
    #[error("the cookie jar could not be encoded: {0}")]
    Encode(cookie_store::Error),
}

impl CookieJar {
    pub fn load(vault: CredentialVault, id: impl Into<String>) -> Result<Self, CookieJarError> {
        let id = id.into();
        let store = match vault.load_cookie_jar(&id)? {
            Some(state) => cookie_store::serde::json::load(Cursor::new(state.as_slice()))
                .map_err(CookieJarError::Decode)?,
            None => RfcCookieStore::default(),
        };
        Ok(Self {
            id,
            store: RwLock::new(store),
            vault,
        })
    }

    fn persist(&self, store: &RfcCookieStore) -> Result<(), CookieJarError> {
        let mut state = Zeroizing::new(Vec::new());
        cookie_store::serde::json::save_incl_expired_and_nonpersistent(store, &mut *state)
            .map_err(CookieJarError::Encode)?;
        self.vault.store_cookie_jar(&self.id, &state)?;
        Ok(())
    }

    #[cfg(test)]
    fn request_cookie_header(&self, url: &Url) -> Option<HeaderValue> {
        ReqwestCookieStore::cookies(self, url)
    }
}

impl ReqwestCookieStore for CookieJar {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        let mut store = self
            .store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut changed = false;
        for value in cookie_headers {
            let Ok(value) = value.to_str() else {
                continue;
            };
            if store.parse(value, url).is_ok() {
                changed = true;
            }
        }
        if changed && let Err(error) = self.persist(&store) {
            tracing::warn!(jar_id = %self.id, %error, "could not persist cookie jar");
        }
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        let store = self
            .store
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = store
            .get_request_values(url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if value.is_empty() {
            return None;
        }
        HeaderValue::from_str(&value).ok()
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
}

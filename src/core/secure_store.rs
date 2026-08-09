use std::{fmt, sync::Arc};

use chrono::{DateTime, Utc};
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom as _, SystemRandom},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use super::{AppSettings, DatabaseStore, database::EncryptedValueRecord};

const ALGORITHM: &str = "aes-256-gcm";
const KEY_VERSION: u32 = 1;
const KEY_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 12;
const UPSTREAM_SESSION_NAMESPACE: &str = "upstream-session";

/// A decrypted upstream session. Its token is zeroed when dropped and is
/// intentionally omitted from debug output.
pub struct UpstreamCredential {
    token: Zeroizing<String>,
    pub expires_at: DateTime<Utc>,
}

impl UpstreamCredential {
    pub fn new(token: Zeroizing<String>, expires_at: DateTime<Utc>) -> Self {
        Self { token, expires_at }
    }

    pub fn bearer_token(&self) -> &str {
        self.token.as_str()
    }
}

impl fmt::Debug for UpstreamCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UpstreamCredential")
            .field("token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum CredentialVaultError {
    #[error("secure storage is unavailable: {0}")]
    Keychain(String),
    #[error("the secure-storage master key has an invalid length")]
    InvalidMasterKey,
    #[error("could not generate secure random data")]
    Random,
    #[error("could not encrypt the credential")]
    Encrypt,
    #[error("the credential could not be decrypted; sign in again")]
    Decrypt,
    #[error("the credential uses unsupported encryption metadata")]
    UnsupportedEncryption,
    #[error("could not encode the credential: {0}")]
    Encode(serde_json::Error),
    #[error("could not decode the credential: {0}")]
    Decode(serde_json::Error),
    #[error("secure storage failed: {0}")]
    Database(#[from] super::database::DatabaseError),
}

trait MasterKeyProvider: Send + Sync {
    fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError>;
}

/// Encrypted credential storage backed by SQLite, with only the master key in
/// the operating-system keychain.
#[derive(Clone)]
pub struct CredentialVault {
    database: DatabaseStore,
    key_provider: Arc<dyn MasterKeyProvider>,
}

impl CredentialVault {
    pub fn new(database: DatabaseStore) -> Self {
        Self {
            database,
            key_provider: Arc::new(OsKeychainMasterKeyProvider),
        }
    }

    #[cfg(test)]
    fn with_key_provider(
        database: DatabaseStore,
        key_provider: Arc<dyn MasterKeyProvider>,
    ) -> Self {
        Self {
            database,
            key_provider,
        }
    }

    /// Atomically persist connection metadata and its encrypted session.
    pub fn store_upstream_with_settings(
        &self,
        settings: &AppSettings,
        upstream_id: &str,
        credential: &UpstreamCredential,
    ) -> Result<(), CredentialVaultError> {
        let plaintext = Zeroizing::new(
            serde_json::to_vec(&StoredCredentialRef {
                token: credential.bearer_token(),
                expires_at: credential.expires_at,
            })
            .map_err(CredentialVaultError::Encode)?,
        );
        let encrypted = self.seal(UPSTREAM_SESSION_NAMESPACE, upstream_id, &plaintext)?;
        self.database.save_app_settings_and_secure_value(
            settings,
            UPSTREAM_SESSION_NAMESPACE,
            upstream_id,
            &encrypted,
        )?;
        Ok(())
    }

    /// Atomically remove connection metadata and its encrypted session.
    pub fn delete_upstream_with_settings(
        &self,
        settings: &AppSettings,
        upstream_id: &str,
    ) -> Result<(), CredentialVaultError> {
        self.database.save_app_settings_and_delete_secure_value(
            settings,
            UPSTREAM_SESSION_NAMESPACE,
            upstream_id,
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn load_upstream(
        &self,
        upstream_id: &str,
    ) -> Result<Option<UpstreamCredential>, CredentialVaultError> {
        let Some(encrypted) = self
            .database
            .load_secure_value(UPSTREAM_SESSION_NAMESPACE, upstream_id)?
        else {
            return Ok(None);
        };
        let plaintext = self.open(UPSTREAM_SESSION_NAMESPACE, upstream_id, encrypted)?;
        let stored: StoredCredential =
            serde_json::from_slice(&plaintext).map_err(CredentialVaultError::Decode)?;
        Ok(Some(UpstreamCredential {
            token: Zeroizing::new(stored.token),
            expires_at: stored.expires_at,
        }))
    }

    fn seal(
        &self,
        namespace: &str,
        name: &str,
        plaintext: &[u8],
    ) -> Result<EncryptedValueRecord, CredentialVaultError> {
        let key = self.key_provider.load_or_create()?;
        let key = encryption_key(&key)?;
        let mut nonce_bytes = [0_u8; NONCE_LENGTH];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| CredentialVaultError::Random)?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut ciphertext = plaintext.to_vec();
        key.seal_in_place_append_tag(
            nonce,
            Aad::from(aad(namespace, name).as_bytes()),
            &mut ciphertext,
        )
        .map_err(|_| CredentialVaultError::Encrypt)?;
        Ok(EncryptedValueRecord {
            algorithm: ALGORITHM.to_owned(),
            key_version: KEY_VERSION,
            nonce: nonce_bytes.to_vec(),
            ciphertext,
        })
    }

    #[allow(dead_code)]
    fn open(
        &self,
        namespace: &str,
        name: &str,
        encrypted: EncryptedValueRecord,
    ) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
        if encrypted.algorithm != ALGORITHM || encrypted.key_version != KEY_VERSION {
            return Err(CredentialVaultError::UnsupportedEncryption);
        }
        let nonce = Nonce::try_assume_unique_for_key(&encrypted.nonce)
            .map_err(|_| CredentialVaultError::UnsupportedEncryption)?;
        let key_bytes = self.key_provider.load_or_create()?;
        let key = encryption_key(&key_bytes)?;
        let mut plaintext = Zeroizing::new(encrypted.ciphertext);
        let plaintext_len = key
            .open_in_place(
                nonce,
                Aad::from(aad(namespace, name).as_bytes()),
                &mut plaintext,
            )
            .map_err(|_| CredentialVaultError::Decrypt)?
            .len();
        plaintext.truncate(plaintext_len);
        Ok(plaintext)
    }
}

#[derive(Serialize)]
struct StoredCredentialRef<'a> {
    token: &'a str,
    expires_at: DateTime<Utc>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct StoredCredential {
    token: String,
    expires_at: DateTime<Utc>,
}

fn encryption_key(bytes: &[u8]) -> Result<LessSafeKey, CredentialVaultError> {
    if bytes.len() != KEY_LENGTH {
        return Err(CredentialVaultError::InvalidMasterKey);
    }
    let key =
        UnboundKey::new(&AES_256_GCM, bytes).map_err(|_| CredentialVaultError::InvalidMasterKey)?;
    Ok(LessSafeKey::new(key))
}

fn aad(namespace: &str, name: &str) -> String {
    format!("resolved-secure-value:{KEY_VERSION}:{namespace}:{name}")
}

struct OsKeychainMasterKeyProvider;

#[cfg(target_os = "macos")]
impl MasterKeyProvider for OsKeychainMasterKeyProvider {
    fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
        use security_framework::passwords::{
            PasswordOptions, generic_password, set_generic_password_options,
        };

        const SERVICE: &str = "dev.apitester.desktop.secure-vault";
        const ACCOUNT: &str = "master-key-v1";
        const ERR_SEC_ITEM_NOT_FOUND: i32 = -25_300;

        let options = || {
            let mut options = PasswordOptions::new_generic_password(SERVICE, ACCOUNT);
            // The database ciphertext is device-local, so the key must not be
            // synchronized through iCloud Keychain.
            options.set_access_synchronized(Some(false));
            options
        };

        match generic_password(options()) {
            Ok(key) => validate_master_key(key),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => {
                let mut key = vec![0_u8; KEY_LENGTH];
                SystemRandom::new()
                    .fill(&mut key)
                    .map_err(|_| CredentialVaultError::Random)?;
                set_generic_password_options(&key, options())
                    .map_err(|error| CredentialVaultError::Keychain(error.to_string()))?;
                Ok(Zeroizing::new(key))
            }
            Err(error) => Err(CredentialVaultError::Keychain(error.to_string())),
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl MasterKeyProvider for OsKeychainMasterKeyProvider {
    fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
        Err(CredentialVaultError::Keychain(
            "this build has no supported OS keychain integration".to_owned(),
        ))
    }
}

fn validate_master_key(key: Vec<u8>) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
    if key.len() != KEY_LENGTH {
        return Err(CredentialVaultError::InvalidMasterKey);
    }
    Ok(Zeroizing::new(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StaticKeyProvider(Vec<u8>);

    impl MasterKeyProvider for StaticKeyProvider {
        fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
            Ok(Zeroizing::new(self.0.clone()))
        }
    }

    fn vault() -> (tempfile::TempDir, DatabaseStore, CredentialVault) {
        let directory = tempfile::tempdir().unwrap();
        let database = DatabaseStore::new(directory.path().join("vault.sqlite3"));
        database.initialize().unwrap();
        let vault = CredentialVault::with_key_provider(
            database.clone(),
            Arc::new(StaticKeyProvider(vec![7_u8; KEY_LENGTH])),
        );
        (directory, database, vault)
    }

    #[test]
    fn stores_only_authenticated_ciphertext_and_loads_the_session() {
        let (_directory, database, vault) = vault();
        let mut settings = AppSettings::default();
        settings.upstreams.active_upstream_id = Some("server-a".to_owned());
        let credential = UpstreamCredential::new(
            Zeroizing::new("plain-session-token".to_owned()),
            Utc::now() + chrono::Duration::hours(1),
        );

        vault
            .store_upstream_with_settings(&settings, "server-a", &credential)
            .unwrap();

        assert_eq!(database.load_app_settings().unwrap(), settings);
        let encrypted = database
            .load_secure_value(UPSTREAM_SESSION_NAMESPACE, "server-a")
            .unwrap()
            .unwrap();
        assert!(!String::from_utf8_lossy(&encrypted.ciphertext).contains("plain-session-token"));
        let loaded = vault.load_upstream("server-a").unwrap().unwrap();
        assert_eq!(loaded.bearer_token(), "plain-session-token");
        assert_eq!(loaded.expires_at, credential.expires_at);
        assert!(!format!("{loaded:?}").contains("plain-session-token"));
    }

    #[test]
    fn rejects_tampered_ciphertext() {
        let (_directory, database, vault) = vault();
        let credential = UpstreamCredential::new(
            Zeroizing::new("plain-session-token".to_owned()),
            Utc::now() + chrono::Duration::hours(1),
        );
        vault
            .store_upstream_with_settings(&AppSettings::default(), "server-a", &credential)
            .unwrap();
        let mut encrypted = database
            .load_secure_value(UPSTREAM_SESSION_NAMESPACE, "server-a")
            .unwrap()
            .unwrap();
        encrypted.ciphertext[0] ^= 0x80;
        database
            .save_secure_value(UPSTREAM_SESSION_NAMESPACE, "server-a", &encrypted)
            .unwrap();

        assert!(matches!(
            vault.load_upstream("server-a"),
            Err(CredentialVaultError::Decrypt)
        ));
    }

    #[test]
    fn deletes_metadata_and_ciphertext_together() {
        let (_directory, database, vault) = vault();
        let credential = UpstreamCredential::new(
            Zeroizing::new("plain-session-token".to_owned()),
            Utc::now() + chrono::Duration::hours(1),
        );
        vault
            .store_upstream_with_settings(&AppSettings::default(), "server-a", &credential)
            .unwrap();
        let settings = AppSettings::default();

        vault
            .delete_upstream_with_settings(&settings, "server-a")
            .unwrap();

        assert_eq!(database.load_app_settings().unwrap(), settings);
        assert!(
            database
                .load_secure_value(UPSTREAM_SESSION_NAMESPACE, "server-a")
                .unwrap()
                .is_none()
        );
    }
}

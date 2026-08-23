use std::{
    collections::HashMap,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

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
const LOCAL_MASTER_KEY_EXTENSION: &str = "secure-vault.key";

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
    #[error("could not access the local credential key at {path}: {source}")]
    LocalKey {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
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

struct CachedMasterKeyProvider {
    delegate: Arc<dyn MasterKeyProvider>,
    key: Mutex<Option<Zeroizing<Vec<u8>>>>,
}

impl CachedMasterKeyProvider {
    fn new(delegate: Arc<dyn MasterKeyProvider>) -> Self {
        Self {
            delegate,
            key: Mutex::new(None),
        }
    }
}

impl MasterKeyProvider for CachedMasterKeyProvider {
    fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
        let mut cached = self
            .key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(key) = cached.as_ref() {
            return Ok(Zeroizing::new(key.as_slice().to_vec()));
        }

        let key = self.delegate.load_or_create()?;
        *cached = Some(Zeroizing::new(key.as_slice().to_vec()));
        Ok(key)
    }
}

/// Encrypted credential storage backed by SQLite. Provisioned builds keep the
/// master key in the operating-system keychain; other builds use a restricted
/// local key file so sessions still survive application restarts.
#[derive(Clone)]
pub struct CredentialVault {
    database: DatabaseStore,
    key_provider: Arc<dyn MasterKeyProvider>,
}

impl CredentialVault {
    pub fn new(database: DatabaseStore) -> Self {
        let key_provider = process_master_key_provider(&database);
        Self {
            database,
            key_provider,
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
        self.database.save_app_settings_and_delete_upstream(
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
        // Wrap the working buffer so a failed encryption leaves no plaintext
        // copy behind: `Zeroizing` wipes the buffer on drop (success and error
        // paths alike). `seal_in_place_separate_tag` encrypts in place and
        // returns the tag separately; the tag is appended afterwards.
        let mut ciphertext = Zeroizing::new(plaintext.to_vec());
        let tag = key
            .seal_in_place_separate_tag(
                nonce,
                Aad::from(aad(namespace, name).as_bytes()),
                &mut ciphertext,
            )
            .map_err(|_| CredentialVaultError::Encrypt)?;
        ciphertext.extend_from_slice(tag.as_ref());
        // The plaintext has been overwritten in place, so this copy holds only
        // ciphertext (no longer secret); the `Zeroizing` wrapper still wipes it.
        let ciphertext = ciphertext.to_vec();
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum MasterKeySource {
    LocalFile,
    #[cfg(target_os = "macos")]
    DataProtectionKeychain,
}

fn process_master_key_provider(database: &DatabaseStore) -> Arc<dyn MasterKeyProvider> {
    type ProviderKey = (MasterKeySource, PathBuf);
    static PROVIDERS: OnceLock<Mutex<HashMap<ProviderKey, Arc<dyn MasterKeyProvider>>>> =
        OnceLock::new();

    let source = selected_master_key_source();
    let local_key_path = local_master_key_path(database.path());
    let provider_key = (source, local_key_path.clone());
    let providers = PROVIDERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut providers = providers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(provider) = providers.get(&provider_key) {
        return Arc::clone(provider);
    }

    let local_provider = FileMasterKeyProvider::new(local_key_path);
    let delegate: Arc<dyn MasterKeyProvider> = match source {
        MasterKeySource::LocalFile => Arc::new(local_provider),
        #[cfg(target_os = "macos")]
        MasterKeySource::DataProtectionKeychain => {
            Arc::new(OsKeychainMasterKeyProvider { local_provider })
        }
    };
    let provider: Arc<dyn MasterKeyProvider> = Arc::new(CachedMasterKeyProvider::new(delegate));
    providers.insert(provider_key, Arc::clone(&provider));
    provider
}

#[cfg(target_os = "macos")]
fn selected_master_key_source() -> MasterKeySource {
    master_key_source_for_entitlement(has_data_protection_keychain_entitlement())
}

#[cfg(target_os = "macos")]
fn master_key_source_for_entitlement(has_entitlement: bool) -> MasterKeySource {
    if has_entitlement {
        MasterKeySource::DataProtectionKeychain
    } else {
        MasterKeySource::LocalFile
    }
}

#[cfg(not(target_os = "macos"))]
fn selected_master_key_source() -> MasterKeySource {
    MasterKeySource::LocalFile
}

fn local_master_key_path(database_path: &Path) -> PathBuf {
    let mut path = database_path.to_owned();
    path.set_extension(LOCAL_MASTER_KEY_EXTENSION);
    path
}

#[derive(Clone)]
struct FileMasterKeyProvider {
    path: PathBuf,
}

impl FileMasterKeyProvider {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn load_existing(&self) -> Result<Option<Zeroizing<Vec<u8>>>, CredentialVaultError> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = match options.open(&self.path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(self.error(source)),
        };
        if !file
            .metadata()
            .map_err(|source| self.error(source))?
            .is_file()
        {
            return Err(self.error(io::Error::new(
                io::ErrorKind::InvalidData,
                "credential key path is not a regular file",
            )));
        }
        restrict_local_key_permissions(&file, &self.path)?;

        let mut key = Vec::with_capacity(KEY_LENGTH + 1);
        file.read_to_end(&mut key)
            .map_err(|source| self.error(source))?;
        validate_master_key(key).map(Some)
    }

    fn persist_if_absent(&self, key: &[u8]) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
        let key = validate_master_key(key.to_vec())?;
        prepare_local_key_parent(&self.path)?;

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = match options.open(&self.path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                return self.load_existing()?.ok_or_else(|| {
                    self.error(io::Error::new(
                        io::ErrorKind::NotFound,
                        "credential key disappeared while it was being opened",
                    ))
                });
            }
            Err(source) => return Err(self.error(source)),
        };
        restrict_local_key_permissions(&file, &self.path)?;
        if let Err(source) = file.write_all(&key).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&self.path);
            return Err(self.error(source));
        }
        Ok(key)
    }

    fn remove(&self) -> Result<(), CredentialVaultError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(self.error(source)),
        }
    }

    fn error(&self, source: io::Error) -> CredentialVaultError {
        CredentialVaultError::LocalKey {
            path: self.path.clone(),
            source,
        }
    }
}

impl MasterKeyProvider for FileMasterKeyProvider {
    fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
        if let Some(key) = self.load_existing()? {
            return Ok(key);
        }
        let key = generate_master_key()?;
        self.persist_if_absent(&key)
    }
}

fn prepare_local_key_parent(path: &Path) -> Result<(), CredentialVaultError> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    fs::create_dir_all(parent).map_err(|source| CredentialVaultError::LocalKey {
        path: parent.to_owned(),
        source,
    })?;
    restrict_local_key_directory_permissions(parent)
}

#[cfg(unix)]
fn restrict_local_key_directory_permissions(path: &Path) -> Result<(), CredentialVaultError> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = fs::metadata(path)
        .map_err(|source| CredentialVaultError::LocalKey {
            path: path.to_owned(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).map_err(|source| CredentialVaultError::LocalKey {
        path: path.to_owned(),
        source,
    })
}

#[cfg(not(unix))]
fn restrict_local_key_directory_permissions(_: &Path) -> Result<(), CredentialVaultError> {
    Ok(())
}

#[cfg(unix)]
fn restrict_local_key_permissions(file: &File, path: &Path) -> Result<(), CredentialVaultError> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = file
        .metadata()
        .map_err(|source| CredentialVaultError::LocalKey {
            path: path.to_owned(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o600);
    file.set_permissions(permissions)
        .map_err(|source| CredentialVaultError::LocalKey {
            path: path.to_owned(),
            source,
        })
}

#[cfg(not(unix))]
fn restrict_local_key_permissions(_: &File, _: &Path) -> Result<(), CredentialVaultError> {
    Ok(())
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

#[cfg(target_os = "macos")]
struct OsKeychainMasterKeyProvider {
    local_provider: FileMasterKeyProvider,
}

#[cfg(target_os = "macos")]
impl MasterKeyProvider for OsKeychainMasterKeyProvider {
    fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
        use security_framework::passwords::generic_password;

        if !has_data_protection_keychain_entitlement() {
            return self.local_provider.load_or_create();
        }

        match generic_password(biometric_keychain_lookup_options()) {
            Ok(key) => validate_master_key(key),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => {
                migrate_or_create_biometric_master_key(&self.local_provider)
            }
            Err(error) if biometric_keychain_unavailable(error.code()) => {
                self.local_provider.load_or_create()
            }
            Err(error) => Err(CredentialVaultError::Keychain(error.to_string())),
        }
    }
}

#[cfg(target_os = "macos")]
fn has_data_protection_keychain_entitlement() -> bool {
    use std::ffi::c_void;

    use core_foundation::{
        base::{CFAllocatorRef, CFRelease, CFTypeRef, TCFType},
        error::CFErrorRef,
        string::{CFString, CFStringRef},
    };

    type SecTaskRef = *const c_void;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecTaskCreateFromSelf(allocator: CFAllocatorRef) -> SecTaskRef;
        fn SecTaskCopyValueForEntitlement(
            task: SecTaskRef,
            entitlement: CFStringRef,
            error: *mut CFErrorRef,
        ) -> CFTypeRef;
    }

    let entitlement = CFString::new("keychain-access-groups");
    unsafe {
        let task = SecTaskCreateFromSelf(std::ptr::null());
        if task.is_null() {
            return false;
        }
        let value = SecTaskCopyValueForEntitlement(
            task,
            entitlement.as_concrete_TypeRef(),
            std::ptr::null_mut(),
        );
        CFRelease(task);
        if value.is_null() {
            return false;
        }
        CFRelease(value);
        true
    }
}

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "dev.apitester.desktop.secure-vault";
#[cfg(target_os = "macos")]
const BIOMETRIC_KEYCHAIN_ACCOUNT: &str = "master-key-v2";
#[cfg(target_os = "macos")]
const LEGACY_KEYCHAIN_ACCOUNT: &str = "master-key-v1";
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25_300;
#[cfg(target_os = "macos")]
const ERR_SEC_NOT_AVAILABLE: i32 = -25_291;
#[cfg(target_os = "macos")]
const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34_018;

#[cfg(target_os = "macos")]
fn biometric_keychain_unavailable(code: i32) -> bool {
    matches!(code, ERR_SEC_NOT_AVAILABLE | ERR_SEC_MISSING_ENTITLEMENT)
}

#[cfg(target_os = "macos")]
fn biometric_keychain_lookup_options() -> security_framework::passwords::PasswordOptions {
    use security_framework::passwords::PasswordOptions;

    let mut options =
        PasswordOptions::new_generic_password(KEYCHAIN_SERVICE, BIOMETRIC_KEYCHAIN_ACCOUNT);
    options.use_protected_keychain();
    options.set_access_synchronized(Some(false));
    options
}

#[cfg(target_os = "macos")]
fn biometric_keychain_create_options()
-> Result<security_framework::passwords::PasswordOptions, CredentialVaultError> {
    use security_framework::{
        access_control::{ProtectionMode, SecAccessControl},
        passwords::{AccessControlOptions, PasswordOptions},
    };

    let access_control = SecAccessControl::create_with_protection(
        Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
        AccessControlOptions::USER_PRESENCE.bits(),
    )
    .map_err(|error| CredentialVaultError::Keychain(error.to_string()))?;
    let mut options =
        PasswordOptions::new_generic_password(KEYCHAIN_SERVICE, BIOMETRIC_KEYCHAIN_ACCOUNT);
    options.use_protected_keychain();
    options.set_access_synchronized(Some(false));
    options.set_access_control(access_control);
    options.set_label("Resolved secure vault");
    options.set_description("Master key for saved server sessions");
    Ok(options)
}

#[cfg(target_os = "macos")]
fn legacy_keychain_options() -> security_framework::passwords::PasswordOptions {
    use security_framework::passwords::PasswordOptions;

    let mut options =
        PasswordOptions::new_generic_password(KEYCHAIN_SERVICE, LEGACY_KEYCHAIN_ACCOUNT);
    options.set_access_synchronized(Some(false));
    options
}

#[cfg(target_os = "macos")]
fn migrate_or_create_biometric_master_key(
    local_provider: &FileMasterKeyProvider,
) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
    use security_framework::passwords::{
        delete_generic_password_options, generic_password, set_generic_password_options,
    };

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum KeyOrigin {
        LocalFile,
        LegacyKeychain,
        Generated,
    }

    let (key, origin) = if let Some(key) = local_provider.load_existing()? {
        (key, KeyOrigin::LocalFile)
    } else {
        match generic_password(legacy_keychain_options()) {
            Ok(key) => (validate_master_key(key)?, KeyOrigin::LegacyKeychain),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => {
                (generate_master_key()?, KeyOrigin::Generated)
            }
            Err(error) => return Err(CredentialVaultError::Keychain(error.to_string())),
        }
    };

    match set_generic_password_options(&key, biometric_keychain_create_options()?) {
        Ok(()) => {}
        Err(error) if biometric_keychain_unavailable(error.code()) => {
            if origin == KeyOrigin::LocalFile {
                return Ok(key);
            }
            return local_provider.persist_if_absent(&key);
        }
        Err(error) => return Err(CredentialVaultError::Keychain(error.to_string())),
    }

    if origin == KeyOrigin::LocalFile
        && let Err(error) = local_provider.remove()
    {
        tracing::warn!(error = %error, "could not remove the migrated local secure-vault key");
    }
    if origin == KeyOrigin::LegacyKeychain {
        match delete_generic_password_options(legacy_keychain_options()) {
            Ok(()) => {}
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => {}
            Err(error) => tracing::warn!(
                error_code = error.code(),
                "could not remove the migrated legacy secure-vault key"
            ),
        }
    }
    Ok(key)
}

fn generate_master_key() -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
    let mut key = Zeroizing::new(vec![0_u8; KEY_LENGTH]);
    SystemRandom::new()
        .fill(key.as_mut_slice())
        .map_err(|_| CredentialVaultError::Random)?;
    Ok(key)
}

fn validate_master_key(key: Vec<u8>) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
    let key = Zeroizing::new(key);
    if key.len() != KEY_LENGTH {
        return Err(CredentialVaultError::InvalidMasterKey);
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct StaticKeyProvider(Vec<u8>);

    impl MasterKeyProvider for StaticKeyProvider {
        fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
            Ok(Zeroizing::new(self.0.clone()))
        }
    }

    struct CountingKeyProvider {
        loads: Arc<AtomicUsize>,
    }

    impl MasterKeyProvider for CountingKeyProvider {
        fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>, CredentialVaultError> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(Zeroizing::new(vec![11_u8; KEY_LENGTH]))
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
    fn unlocks_the_master_key_once_across_vault_instances() {
        let directory = tempfile::tempdir().unwrap();
        let database = DatabaseStore::new(directory.path().join("vault.sqlite3"));
        database.initialize().unwrap();
        let loads = Arc::new(AtomicUsize::new(0));
        let key_provider: Arc<dyn MasterKeyProvider> = Arc::new(CachedMasterKeyProvider::new(
            Arc::new(CountingKeyProvider {
                loads: Arc::clone(&loads),
            }),
        ));
        let first_vault =
            CredentialVault::with_key_provider(database.clone(), Arc::clone(&key_provider));
        let second_vault = CredentialVault::with_key_provider(database, key_provider);
        let credential = UpstreamCredential::new(
            Zeroizing::new("plain-session-token".to_owned()),
            Utc::now() + chrono::Duration::hours(1),
        );

        first_vault
            .store_upstream_with_settings(&AppSettings::default(), "server-a", &credential)
            .unwrap();
        assert!(second_vault.load_upstream("server-a").unwrap().is_some());
        assert!(first_vault.load_upstream("server-a").unwrap().is_some());

        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn default_vaults_share_the_process_master_key_provider() {
        let directory = tempfile::tempdir().unwrap();
        let database = DatabaseStore::new(directory.path().join("vault.sqlite3"));
        let first = CredentialVault::new(database.clone());
        let second = CredentialVault::new(database);

        assert!(Arc::ptr_eq(&first.key_provider, &second.key_provider));
    }

    #[test]
    fn local_master_key_survives_provider_recreation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.secure-vault.key");
        let first = FileMasterKeyProvider::new(path.clone())
            .load_or_create()
            .unwrap();
        let second = FileMasterKeyProvider::new(path.clone())
            .load_or_create()
            .unwrap();

        assert_eq!(first.as_slice(), second.as_slice());
        assert_eq!(first.len(), KEY_LENGTH);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn local_master_key_path_is_specific_to_the_database() {
        assert_eq!(
            local_master_key_path(Path::new("/tmp/api-tester.sqlite3")),
            PathBuf::from("/tmp/api-tester.secure-vault.key")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unprovisioned_builds_select_the_local_key_file() {
        assert_eq!(
            master_key_source_for_entitlement(false),
            MasterKeySource::LocalFile
        );
        assert_eq!(
            master_key_source_for_entitlement(true),
            MasterKeySource::DataProtectionKeychain
        );
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
        let mut request_tabs = crate::core::RequestTabs::new();
        request_tabs.active_mut().set_title("Server draft");
        database
            .save_upstream_request_tabs("server-a", "workspace-a", &request_tabs)
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
        let restored = database
            .load_upstream_request_tabs("server-a", "workspace-a")
            .unwrap();
        assert_eq!(restored.active().display_title(), "Untitled Request");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn biometric_vault_falls_back_only_when_the_platform_is_unavailable() {
        assert!(biometric_keychain_unavailable(ERR_SEC_NOT_AVAILABLE));
        assert!(biometric_keychain_unavailable(ERR_SEC_MISSING_ENTITLEMENT));
        assert!(!biometric_keychain_unavailable(-25_293)); // errSecAuthFailed
        assert!(!biometric_keychain_unavailable(-25_308)); // errSecInteractionNotAllowed
        assert!(!biometric_keychain_unavailable(-128)); // errSecUserCanceled
    }
}

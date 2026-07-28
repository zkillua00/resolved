//! SQLite persistence for the complete local API Tester workspace.
//!
//! The database is intentionally an aggregate store: callers load or save the
//! domain-level [`Workspace`] and [`RequestHistory`] values, while this module
//! owns normalization, transaction boundaries, migrations, and the one-time
//! import of the legacy JSON files.

use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    AppSettings,
    history::{DEFAULT_HISTORY_LIMIT, HistoryEntry, RequestHistory, ResponseSummary},
    request::{BodyField, BodyFieldKind, BodyMode, HeaderEntry, RawBodyLanguage, RequestDraft},
    request_tabs::RequestTabs,
    template::RequestTemplate,
    workspace::{
        Collection, CollectionFolder, Environment, EnvironmentVariable, RequestScripts,
        SavedRequest, WORKSPACE_FILE_VERSION, Workspace, WorkspaceValidationError,
    },
};

#[cfg(test)]
use super::request_tabs::RequestTabGroupColor;

const CURRENT_SCHEMA_VERSION: i64 = 5;
const LEGACY_HISTORY_FILE_VERSION: u32 = 1;
const LEGACY_HISTORY_IMPORT_MARKER: &str = "history-json-v1";
const LEGACY_WORKSPACE_IMPORT_MARKER: &str = "workspace-json-v1";
// UI callbacks currently commit small local aggregates synchronously. Keep
// lock contention bounded so another SQLite reader/writer cannot stall GPUI
// for multiple seconds.
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_millis(250);
const SQLITE_APPLICATION_ID: i32 = 0x4150_4954; // "APIT"

const MIGRATION_1: &str = r#"
CREATE TABLE collections (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL CHECK (length(trim(name)) > 0),
    position    INTEGER NOT NULL CHECK (position >= 0),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    version     INTEGER NOT NULL CHECK (version >= 1)
);

CREATE INDEX collections_position_idx
    ON collections(position, id);

CREATE TABLE saved_requests (
    id              TEXT PRIMARY KEY NOT NULL,
    collection_id   TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    name            TEXT NOT NULL CHECK (length(trim(name)) > 0),
    position        INTEGER NOT NULL CHECK (position >= 0),
    method          TEXT NOT NULL,
    url             TEXT NOT NULL,
    body            TEXT NOT NULL,
    pre_request     TEXT NOT NULL,
    post_response   TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    version         INTEGER NOT NULL CHECK (version >= 1)
);

CREATE INDEX saved_requests_collection_position_idx
    ON saved_requests(collection_id, position, id);

CREATE TABLE saved_request_headers (
    id                  INTEGER PRIMARY KEY,
    saved_request_id    TEXT NOT NULL REFERENCES saved_requests(id) ON DELETE CASCADE,
    position            INTEGER NOT NULL CHECK (position >= 0),
    enabled             INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    name                TEXT NOT NULL,
    value               TEXT NOT NULL,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    version             INTEGER NOT NULL CHECK (version >= 1),
    UNIQUE(saved_request_id, position)
);

CREATE TABLE environments (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL CHECK (length(trim(name)) > 0),
    position    INTEGER NOT NULL CHECK (position >= 0),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    version     INTEGER NOT NULL CHECK (version >= 1)
);

CREATE INDEX environments_position_idx
    ON environments(position, id);

CREATE TABLE environment_variables (
    id              TEXT PRIMARY KEY NOT NULL,
    environment_id  TEXT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    position        INTEGER NOT NULL CHECK (position >= 0),
    key             TEXT NOT NULL CHECK (length(trim(key)) > 0),
    value           TEXT NOT NULL,
    enabled         INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    secret          INTEGER NOT NULL CHECK (secret IN (0, 1)),
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    version         INTEGER NOT NULL CHECK (version >= 1)
);

CREATE INDEX environment_variables_environment_position_idx
    ON environment_variables(environment_id, position, id);

CREATE TABLE metadata (
    singleton               INTEGER PRIMARY KEY CHECK (singleton = 1),
    active_environment_id   TEXT REFERENCES environments(id) ON DELETE SET NULL,
    updated_at              INTEGER NOT NULL,
    version                 INTEGER NOT NULL CHECK (version >= 1)
);

CREATE TABLE history_entries (
    id                      TEXT PRIMARY KEY NOT NULL,
    position                INTEGER NOT NULL CHECK (position >= 0),
    created_at              INTEGER NOT NULL,
    method                  TEXT NOT NULL,
    url                     TEXT NOT NULL,
    body                    TEXT NOT NULL,
    error                   TEXT,
    response_status         INTEGER,
    response_status_text    TEXT,
    response_duration_ms    INTEGER,
    response_size_bytes     INTEGER,
    response_content_type   TEXT,
    updated_at              INTEGER NOT NULL,
    version                 INTEGER NOT NULL CHECK (version >= 1),
    CHECK (response_status IS NULL OR response_status BETWEEN 100 AND 999)
);

CREATE INDEX history_entries_position_idx
    ON history_entries(position, id);

CREATE TABLE history_headers (
    id                  INTEGER PRIMARY KEY,
    history_entry_id    TEXT NOT NULL REFERENCES history_entries(id) ON DELETE CASCADE,
    position            INTEGER NOT NULL CHECK (position >= 0),
    enabled             INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    name                TEXT NOT NULL,
    value               TEXT NOT NULL,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    version             INTEGER NOT NULL CHECK (version >= 1),
    UNIQUE(history_entry_id, position)
);

CREATE TABLE migration_markers (
    marker          TEXT PRIMARY KEY NOT NULL,
    completed_at    INTEGER NOT NULL
);

INSERT INTO metadata(singleton, active_environment_id, updated_at, version)
VALUES (1, NULL, 0, 1);
"#;

const MIGRATION_2: &str = r#"
ALTER TABLE saved_requests
ADD COLUMN body_mode TEXT NOT NULL DEFAULT 'raw'
CHECK (body_mode IN ('none', 'raw', 'form_url_encoded', 'multipart_form_data'));

ALTER TABLE saved_requests
ADD COLUMN raw_body_language TEXT NOT NULL DEFAULT 'json'
CHECK (
    raw_body_language IN (
        'text', 'json', 'xml', 'html', 'javascript', 'typescript', 'css',
        'markdown', 'graphql', 'yaml', 'toml', 'sql', 'shell', 'rust', 'python'
    )
);

ALTER TABLE history_entries
ADD COLUMN body_mode TEXT NOT NULL DEFAULT 'raw'
CHECK (body_mode IN ('none', 'raw', 'form_url_encoded', 'multipart_form_data'));

ALTER TABLE history_entries
ADD COLUMN raw_body_language TEXT NOT NULL DEFAULT 'json'
CHECK (
    raw_body_language IN (
        'text', 'json', 'xml', 'html', 'javascript', 'typescript', 'css',
        'markdown', 'graphql', 'yaml', 'toml', 'sql', 'shell', 'rust', 'python'
    )
);

CREATE TABLE saved_request_body_fields (
    id                  INTEGER PRIMARY KEY,
    saved_request_id    TEXT NOT NULL REFERENCES saved_requests(id) ON DELETE CASCADE,
    position            INTEGER NOT NULL CHECK (position >= 0),
    enabled             INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    name                TEXT NOT NULL,
    value               TEXT NOT NULL,
    kind                TEXT NOT NULL CHECK (kind IN ('text', 'file')),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    version             INTEGER NOT NULL CHECK (version >= 1),
    UNIQUE(saved_request_id, position)
);

CREATE TABLE history_body_fields (
    id                  INTEGER PRIMARY KEY,
    history_entry_id    TEXT NOT NULL REFERENCES history_entries(id) ON DELETE CASCADE,
    position            INTEGER NOT NULL CHECK (position >= 0),
    enabled             INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    name                TEXT NOT NULL,
    value               TEXT NOT NULL,
    kind                TEXT NOT NULL CHECK (kind IN ('text', 'file')),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    version             INTEGER NOT NULL CHECK (version >= 1),
    UNIQUE(history_entry_id, position)
);
"#;

const MIGRATION_3: &str = r#"
CREATE TABLE collection_folders (
    id                  TEXT PRIMARY KEY NOT NULL,
    collection_id       TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    parent_folder_id    TEXT REFERENCES collection_folders(id) ON DELETE CASCADE,
    name                TEXT NOT NULL CHECK (length(trim(name)) > 0),
    position            INTEGER NOT NULL CHECK (position >= 0),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    version             INTEGER NOT NULL CHECK (version >= 1),
    CHECK (parent_folder_id IS NULL OR parent_folder_id <> id)
);

CREATE INDEX collection_folders_collection_parent_position_idx
    ON collection_folders(collection_id, parent_folder_id, position, id);

ALTER TABLE saved_requests
ADD COLUMN folder_id TEXT REFERENCES collection_folders(id) ON DELETE CASCADE;

CREATE INDEX saved_requests_collection_folder_position_idx
    ON saved_requests(collection_id, folder_id, position, id);
"#;

const MIGRATION_4: &str = r#"
CREATE TABLE request_tab_state (
    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
    state_json  TEXT NOT NULL,
    updated_at  INTEGER NOT NULL,
    version     INTEGER NOT NULL CHECK (version >= 1)
);
"#;

const MIGRATION_5: &str = r#"
CREATE TABLE app_settings (
    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
    state_json  TEXT NOT NULL,
    updated_at  INTEGER NOT NULL,
    version     INTEGER NOT NULL CHECK (version >= 1)
);
"#;

/// The application aggregates persisted in one consistent SQLite snapshot.
#[derive(Debug)]
#[allow(dead_code)]
pub struct DatabaseState {
    pub workspace: Workspace,
    pub history: RequestHistory,
}

/// Result of checking for and importing the former JSON persistence files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyImportOutcome {
    /// Neither legacy file exists. No marker was written, so a later retry is
    /// still possible.
    NoFiles,
    /// Every existing source already has its own durable import marker.
    AlreadyImported,
    /// Existing empty aggregates were populated and their markers committed.
    ///
    /// An aggregate is skipped rather than overwritten when its database
    /// tables already contain data.
    Imported {
        workspace_imported: bool,
        history_imported: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegacySourceOutcome {
    Missing,
    AlreadyImported,
    Processed { imported: bool },
}

impl LegacySourceOutcome {
    fn imported(self) -> bool {
        matches!(self, Self::Processed { imported: true })
    }
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("could not create database directory {path}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not open SQLite database {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    #[error("SQLite operation failed")]
    Sql(#[from] rusqlite::Error),

    #[error("database schema version {found} is newer than the supported version {supported}")]
    UnsupportedSchemaVersion { found: i64, supported: i64 },

    #[error("workspace data is invalid")]
    InvalidWorkspace(#[from] WorkspaceValidationError),

    #[error("could not read legacy {kind} file {path}")]
    LegacyRead {
        kind: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not parse legacy {kind} file {path}")]
    LegacyParse {
        kind: &'static str,
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "legacy {kind} file {path} has version {found}; this build supports version {supported}"
    )]
    UnsupportedLegacyVersion {
        kind: &'static str,
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    #[error("{field} cannot be represented by SQLite: {value}")]
    IntegerOutOfRange { field: &'static str, value: String },

    #[error("database contains invalid {field}: {value}")]
    CorruptData { field: &'static str, value: String },

    #[error("SQLite quick_check failed: {details}")]
    IntegrityCheckFailed { details: String },

    #[error("database has application_id {found:#010x}; expected API Tester's {expected:#010x}")]
    UnexpectedApplicationId { found: i64, expected: i32 },

    #[error("could not restrict database storage permissions for {path}")]
    SetPermissions {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Path-backed SQLite aggregate store.
///
/// Connections are short-lived and independently configured, which keeps this
/// type cheap to clone and safe to move between the UI and background threads.
#[derive(Clone, Debug)]
pub struct DatabaseStore {
    path: PathBuf,
    history_limit: usize,
}

impl Default for DatabaseStore {
    fn default() -> Self {
        Self::new(default_database_path())
    }
}

#[allow(dead_code)]
impl DatabaseStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            history_limit: DEFAULT_HISTORY_LIMIT,
        }
    }

    pub fn default_path() -> PathBuf {
        default_database_path()
    }

    pub fn with_history_limit(mut self, history_limit: usize) -> Self {
        self.history_limit = history_limit.max(1);
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Create/configure the database and apply all known migrations.
    pub fn initialize(&self) -> Result<(), DatabaseError> {
        let connection = self.open_connection()?;
        quick_check_connection(&connection)
    }

    /// Run SQLite's lightweight full-database consistency check.
    pub fn quick_check(&self) -> Result<(), DatabaseError> {
        let connection = self.open_connection()?;
        quick_check_connection(&connection)
    }

    /// Load the workspace and request history from one read transaction.
    pub fn load_state(&self) -> Result<DatabaseState, DatabaseError> {
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let workspace = load_workspace_tx(&transaction)?;
        let history = load_history_tx(&transaction, self.history_limit)?;
        transaction.commit()?;
        Ok(DatabaseState { workspace, history })
    }

    /// Replace both aggregates atomically.
    pub fn save_state(
        &self,
        workspace: &Workspace,
        history: &RequestHistory,
    ) -> Result<(), DatabaseError> {
        workspace.validate()?;
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        save_workspace_tx(&transaction, workspace)?;
        save_history_tx(&transaction, history, self.history_limit)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_workspace(&self) -> Result<Workspace, DatabaseError> {
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let workspace = load_workspace_tx(&transaction)?;
        transaction.commit()?;
        Ok(workspace)
    }

    pub fn save_workspace(&self, workspace: &Workspace) -> Result<(), DatabaseError> {
        workspace.validate()?;
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        save_workspace_tx(&transaction, workspace)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn save_workspace_and_request_tabs(
        &self,
        workspace: &Workspace,
        request_tabs: &RequestTabs,
    ) -> Result<(), DatabaseError> {
        workspace.validate()?;
        let state_json = serialize_request_tabs(request_tabs)?;
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        save_workspace_tx(&transaction, workspace)?;
        save_request_tabs_tx(&transaction, &state_json)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_request_tabs(&self) -> Result<RequestTabs, DatabaseError> {
        let connection = self.open_connection()?;
        let state_json = connection
            .query_row(
                "SELECT state_json FROM request_tab_state WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match state_json {
            Some(state_json) => {
                serde_json::from_str(&state_json).map_err(|error| DatabaseError::CorruptData {
                    field: "request tab state",
                    value: error.to_string(),
                })
            }
            None => Ok(RequestTabs::default()),
        }
    }

    pub fn save_request_tabs(&self, request_tabs: &RequestTabs) -> Result<(), DatabaseError> {
        let state_json = serialize_request_tabs(request_tabs)?;
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        save_request_tabs_tx(&transaction, &state_json)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_app_settings(&self) -> Result<AppSettings, DatabaseError> {
        let connection = self.open_connection()?;
        let state_json = connection
            .query_row(
                "SELECT state_json FROM app_settings WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match state_json {
            Some(state_json) => {
                serde_json::from_str(&state_json).map_err(|error| DatabaseError::CorruptData {
                    field: "app settings",
                    value: error.to_string(),
                })
            }
            None => Ok(AppSettings::default()),
        }
    }

    pub fn save_app_settings(&self, settings: &AppSettings) -> Result<(), DatabaseError> {
        let state_json = serialize_app_settings(settings)?;
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        save_app_settings_tx(&transaction, &state_json)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_history(&self) -> Result<RequestHistory, DatabaseError> {
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let history = load_history_tx(&transaction, self.history_limit)?;
        transaction.commit()?;
        Ok(history)
    }

    pub fn save_history(&self, history: &RequestHistory) -> Result<(), DatabaseError> {
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        save_history_tx(&transaction, history, self.history_limit)?;
        transaction.commit()?;
        Ok(())
    }

    /// Import `history.json` and `workspace.json` next to this database.
    pub fn import_legacy_if_needed(&self) -> Result<LegacyImportOutcome, DatabaseError> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        self.import_legacy_json(parent.join("history.json"), parent.join("workspace.json"))
    }

    /// Import the former JSON stores once without modifying or deleting them.
    ///
    /// Each source has its own transaction and durable marker, so a malformed
    /// source cannot block the other valid source. An aggregate is imported
    /// only when its corresponding SQLite tables are empty. Its marker and
    /// rows commit together.
    pub fn import_legacy_json(
        &self,
        history_path: impl AsRef<Path>,
        workspace_path: impl AsRef<Path>,
    ) -> Result<LegacyImportOutcome, DatabaseError> {
        let history_path = history_path.as_ref();
        let workspace_path = workspace_path.as_ref();
        let mut connection = self.open_connection()?;

        let history_result = self.import_legacy_history_source(&mut connection, history_path);
        // Always attempt workspace even when history parsing or import failed.
        let workspace_result = self.import_legacy_workspace_source(&mut connection, workspace_path);

        match (history_result, workspace_result) {
            (Err(error), _) | (_, Err(error)) => Err(error),
            (Ok(history), Ok(workspace)) => {
                if history == LegacySourceOutcome::Missing
                    && workspace == LegacySourceOutcome::Missing
                {
                    return Ok(LegacyImportOutcome::NoFiles);
                }
                if matches!(
                    history,
                    LegacySourceOutcome::Missing | LegacySourceOutcome::AlreadyImported
                ) && matches!(
                    workspace,
                    LegacySourceOutcome::Missing | LegacySourceOutcome::AlreadyImported
                ) {
                    return Ok(LegacyImportOutcome::AlreadyImported);
                }
                Ok(LegacyImportOutcome::Imported {
                    workspace_imported: workspace.imported(),
                    history_imported: history.imported(),
                })
            }
        }
    }

    fn import_legacy_history_source(
        &self,
        connection: &mut Connection,
        path: &Path,
    ) -> Result<LegacySourceOutcome, DatabaseError> {
        if legacy_import_completed(connection, LEGACY_HISTORY_IMPORT_MARKER)? {
            return Ok(LegacySourceOutcome::AlreadyImported);
        }
        let Some(history) = read_legacy_history(path)? else {
            return Ok(LegacySourceOutcome::Missing);
        };

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if legacy_import_completed(&transaction, LEGACY_HISTORY_IMPORT_MARKER)? {
            transaction.commit()?;
            return Ok(LegacySourceOutcome::AlreadyImported);
        }
        let imported = aggregate_is_empty(
            &transaction,
            "SELECT NOT EXISTS(SELECT 1 FROM history_entries)",
        )?;
        if imported {
            save_history_tx(&transaction, &history, self.history_limit)?;
        }
        write_legacy_import_marker(&transaction, LEGACY_HISTORY_IMPORT_MARKER)?;
        transaction.commit()?;
        Ok(LegacySourceOutcome::Processed { imported })
    }

    fn import_legacy_workspace_source(
        &self,
        connection: &mut Connection,
        path: &Path,
    ) -> Result<LegacySourceOutcome, DatabaseError> {
        if legacy_import_completed(connection, LEGACY_WORKSPACE_IMPORT_MARKER)? {
            return Ok(LegacySourceOutcome::AlreadyImported);
        }
        let Some(workspace) = read_legacy_workspace(path)? else {
            return Ok(LegacySourceOutcome::Missing);
        };

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if legacy_import_completed(&transaction, LEGACY_WORKSPACE_IMPORT_MARKER)? {
            transaction.commit()?;
            return Ok(LegacySourceOutcome::AlreadyImported);
        }
        let imported = aggregate_is_empty(
            &transaction,
            "SELECT NOT EXISTS(
                SELECT 1 FROM collections
                UNION ALL
                SELECT 1 FROM environments
            )",
        )?;
        if imported {
            save_workspace_tx(&transaction, &workspace)?;
        }
        write_legacy_import_marker(&transaction, LEGACY_WORKSPACE_IMPORT_MARKER)?;
        transaction.commit()?;
        Ok(LegacySourceOutcome::Processed { imported })
    }

    fn open_connection(&self) -> Result<Connection, DatabaseError> {
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| DatabaseError::CreateDirectory {
                path: parent.to_owned(),
                source,
            })?;
            restrict_directory_permissions(parent)?;
        }

        let mut connection =
            Connection::open(&self.path).map_err(|source| DatabaseError::Open {
                path: self.path.clone(),
                source,
            })?;
        configure_connection(&connection)?;
        migrate(&mut connection)?;
        ensure_application_id(&connection)?;
        restrict_database_permissions(&self.path)?;
        Ok(connection)
    }
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> Result<(), DatabaseError> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = fs::metadata(path)
        .map_err(|source| DatabaseError::SetPermissions {
            path: path.to_owned(),
            source,
        })?
        .permissions();
    if permissions.mode() & 0o777 != 0o700 {
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).map_err(|source| DatabaseError::SetPermissions {
            path: path.to_owned(),
            source,
        })?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_: &Path) -> Result<(), DatabaseError> {
    Ok(())
}

pub fn default_database_path() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("API Tester")
        .join("api-tester.sqlite3")
}

fn configure_connection(connection: &Connection) -> Result<(), DatabaseError> {
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.busy_timeout(DEFAULT_BUSY_TIMEOUT)?;
    let application_id: i64 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if application_id != 0 && application_id != i64::from(SQLITE_APPLICATION_ID) {
        return Err(DatabaseError::UnexpectedApplicationId {
            found: application_id,
            expected: SQLITE_APPLICATION_ID,
        });
    }
    Ok(())
}

fn ensure_application_id(connection: &Connection) -> Result<(), DatabaseError> {
    let application_id: i64 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if application_id == 0 {
        connection.pragma_update(None, "application_id", SQLITE_APPLICATION_ID)?;
        return Ok(());
    }
    if application_id != i64::from(SQLITE_APPLICATION_ID) {
        return Err(DatabaseError::UnexpectedApplicationId {
            found: application_id,
            expected: SQLITE_APPLICATION_ID,
        });
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_database_permissions(path: &Path) -> Result<(), DatabaseError> {
    use std::{ffi::OsString, os::unix::fs::PermissionsExt as _};

    let sidecar = |suffix: &str| {
        let mut value = OsString::from(path.as_os_str());
        value.push(suffix);
        PathBuf::from(value)
    };
    for candidate in [path.to_owned(), sidecar("-wal"), sidecar("-shm")] {
        let metadata = match fs::metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(DatabaseError::SetPermissions {
                    path: candidate,
                    source,
                });
            }
        };
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&candidate, permissions).map_err(|source| {
            DatabaseError::SetPermissions {
                path: candidate,
                source,
            }
        })?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_database_permissions(_: &Path) -> Result<(), DatabaseError> {
    Ok(())
}

fn quick_check_connection(connection: &Connection) -> Result<(), DatabaseError> {
    let result: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if result.eq_ignore_ascii_case("ok") {
        Ok(())
    } else {
        Err(DatabaseError::IntegrityCheckFailed { details: result })
    }
}

fn migrate(connection: &mut Connection) -> Result<(), DatabaseError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
            version     INTEGER NOT NULL CHECK (version >= 0),
            updated_at  INTEGER NOT NULL
        );",
    )?;
    transaction.execute(
        "INSERT INTO schema_version(singleton, version, updated_at)
         VALUES (1, 0, ?1)
         ON CONFLICT(singleton) DO NOTHING",
        params![now_micros()],
    )?;

    let mut version: i64 = transaction.query_row(
        "SELECT version FROM schema_version WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if version > CURRENT_SCHEMA_VERSION {
        return Err(DatabaseError::UnsupportedSchemaVersion {
            found: version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    while version < CURRENT_SCHEMA_VERSION {
        let next = version + 1;
        match next {
            1 => transaction.execute_batch(MIGRATION_1)?,
            2 => transaction.execute_batch(MIGRATION_2)?,
            3 => transaction.execute_batch(MIGRATION_3)?,
            4 => transaction.execute_batch(MIGRATION_4)?,
            5 => transaction.execute_batch(MIGRATION_5)?,
            _ => {
                return Err(DatabaseError::UnsupportedSchemaVersion {
                    found: next,
                    supported: CURRENT_SCHEMA_VERSION,
                });
            }
        }
        transaction.execute(
            "UPDATE schema_version SET version = ?1, updated_at = ?2 WHERE singleton = 1",
            params![next, now_micros()],
        )?;
        version = next;
    }

    transaction.commit()?;
    Ok(())
}

fn save_workspace_tx(
    transaction: &Transaction<'_>,
    workspace: &Workspace,
) -> Result<(), DatabaseError> {
    workspace.validate()?;
    let saved_at = now_micros();
    let mut collection_ids = HashSet::new();
    let mut folder_ids = HashSet::new();
    let mut saved_request_ids = HashSet::new();

    for (collection_position, collection) in workspace.collections.iter().enumerate() {
        collection_ids.insert(collection.id.clone());
        transaction.execute(
            "INSERT INTO collections(
                id, name, position, created_at, updated_at, version
             ) VALUES (?1, ?2, ?3, ?4, ?4, 1)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                position = excluded.position,
                updated_at = excluded.updated_at,
                version = collections.version + 1",
            params![
                &collection.id,
                &collection.name,
                to_i64(collection_position, "collection position")?,
                saved_at,
            ],
        )?;

        // A folder may appear after its children in the flat domain vector,
        // while SQLite's immediate self-referential foreign key requires the
        // parent row to exist first. Persist topologically but retain the
        // vector index as the durable display position.
        let mut pending_folders = collection.folders.iter().enumerate().collect::<Vec<_>>();
        let mut persisted_folder_ids = HashSet::new();
        while !pending_folders.is_empty() {
            let pending_count = pending_folders.len();
            let mut deferred_folders = Vec::new();

            for (folder_position, folder) in pending_folders {
                let parent_is_ready = folder
                    .parent_folder_id
                    .as_deref()
                    .map(|parent_id| persisted_folder_ids.contains(parent_id))
                    .unwrap_or(true);
                if !parent_is_ready {
                    deferred_folders.push((folder_position, folder));
                    continue;
                }

                transaction.execute(
                    "INSERT INTO collection_folders(
                        id, collection_id, parent_folder_id, name, position,
                        created_at, updated_at, version
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 1)
                     ON CONFLICT(id) DO UPDATE SET
                        collection_id = excluded.collection_id,
                        parent_folder_id = excluded.parent_folder_id,
                        name = excluded.name,
                        position = excluded.position,
                        updated_at = excluded.updated_at,
                        version = collection_folders.version + 1",
                    params![
                        &folder.id,
                        &collection.id,
                        folder.parent_folder_id.as_deref(),
                        &folder.name,
                        to_i64(folder_position, "collection folder position")?,
                        saved_at,
                    ],
                )?;
                persisted_folder_ids.insert(folder.id.as_str());
                folder_ids.insert(folder.id.clone());
            }

            if deferred_folders.len() == pending_count {
                // `Workspace::validate` rejects dangling parents and cycles,
                // so reaching this branch means the validated domain and the
                // persistence contract have drifted apart.
                return Err(DatabaseError::CorruptData {
                    field: "collection folder hierarchy",
                    value: collection.id.clone(),
                });
            }
            pending_folders = deferred_folders;
        }

        for (request_position, saved_request) in collection.requests.iter().enumerate() {
            saved_request_ids.insert(saved_request.id.clone());
            let request = &saved_request.definition.request;
            let scripts = &saved_request.definition.scripts;
            transaction.execute(
                "INSERT INTO saved_requests(
                    id, collection_id, folder_id, name, position, method, url, body,
                    body_mode, raw_body_language, pre_request, post_response,
                    created_at, updated_at, version
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 1
                 )
                 ON CONFLICT(id) DO UPDATE SET
                    collection_id = excluded.collection_id,
                    folder_id = excluded.folder_id,
                    name = excluded.name,
                    position = excluded.position,
                    method = excluded.method,
                    url = excluded.url,
                    body = excluded.body,
                    body_mode = excluded.body_mode,
                    raw_body_language = excluded.raw_body_language,
                    pre_request = excluded.pre_request,
                    post_response = excluded.post_response,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    version = saved_requests.version + 1",
                params![
                    &saved_request.id,
                    &collection.id,
                    saved_request.folder_id.as_deref(),
                    &saved_request.name,
                    to_i64(request_position, "saved request position")?,
                    &request.method,
                    &request.url,
                    &request.body,
                    request.body_mode.as_db_str(),
                    request.raw_body_language.as_db_str(),
                    &scripts.pre_request,
                    &scripts.post_response,
                    saved_request.created_at.timestamp_micros(),
                    saved_request.updated_at.timestamp_micros(),
                ],
            )?;
            sync_saved_request_headers(transaction, &saved_request.id, &request.headers, saved_at)?;
            sync_saved_request_body_fields(
                transaction,
                &saved_request.id,
                &request.body_fields,
                saved_at,
            )?;
        }
    }

    delete_missing_ids(
        transaction,
        "SELECT id FROM saved_requests",
        "DELETE FROM saved_requests WHERE id = ?1",
        &saved_request_ids,
    )?;
    delete_missing_ids(
        transaction,
        "SELECT id FROM collection_folders",
        "DELETE FROM collection_folders WHERE id = ?1",
        &folder_ids,
    )?;
    delete_missing_ids(
        transaction,
        "SELECT id FROM collections",
        "DELETE FROM collections WHERE id = ?1",
        &collection_ids,
    )?;

    let mut environment_ids = HashSet::new();
    let mut variable_ids = HashSet::new();
    for (environment_position, environment) in workspace.environments.iter().enumerate() {
        environment_ids.insert(environment.id.clone());
        transaction.execute(
            "INSERT INTO environments(
                id, name, position, created_at, updated_at, version
             ) VALUES (?1, ?2, ?3, ?4, ?4, 1)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                position = excluded.position,
                updated_at = excluded.updated_at,
                version = environments.version + 1",
            params![
                &environment.id,
                &environment.name,
                to_i64(environment_position, "environment position")?,
                saved_at,
            ],
        )?;

        for (variable_position, variable) in environment.variables.iter().enumerate() {
            variable_ids.insert(variable.id.clone());
            transaction.execute(
                "INSERT INTO environment_variables(
                    id, environment_id, position, key, value, enabled, secret,
                    created_at, updated_at, version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, 1)
                 ON CONFLICT(id) DO UPDATE SET
                    environment_id = excluded.environment_id,
                    position = excluded.position,
                    key = excluded.key,
                    value = excluded.value,
                    enabled = excluded.enabled,
                    secret = excluded.secret,
                    updated_at = excluded.updated_at,
                    version = environment_variables.version + 1",
                params![
                    &variable.id,
                    &environment.id,
                    to_i64(variable_position, "environment variable position")?,
                    &variable.key,
                    &variable.value,
                    bool_to_i64(variable.enabled),
                    bool_to_i64(variable.secret),
                    saved_at,
                ],
            )?;
        }
    }

    delete_missing_ids(
        transaction,
        "SELECT id FROM environment_variables",
        "DELETE FROM environment_variables WHERE id = ?1",
        &variable_ids,
    )?;
    delete_missing_ids(
        transaction,
        "SELECT id FROM environments",
        "DELETE FROM environments WHERE id = ?1",
        &environment_ids,
    )?;

    transaction.execute(
        "INSERT INTO metadata(singleton, active_environment_id, updated_at, version)
         VALUES (1, ?1, ?2, 1)
         ON CONFLICT(singleton) DO UPDATE SET
            active_environment_id = excluded.active_environment_id,
            updated_at = excluded.updated_at,
            version = metadata.version + 1",
        params![workspace.active_environment_id.as_deref(), saved_at],
    )?;
    Ok(())
}

fn serialize_request_tabs(request_tabs: &RequestTabs) -> Result<String, DatabaseError> {
    serde_json::to_string(request_tabs).map_err(|error| DatabaseError::CorruptData {
        field: "request tab state",
        value: error.to_string(),
    })
}

fn save_request_tabs_tx(
    transaction: &Transaction<'_>,
    state_json: &str,
) -> Result<(), DatabaseError> {
    transaction.execute(
        "INSERT INTO request_tab_state(singleton, state_json, updated_at, version)
         VALUES (1, ?1, ?2, 1)
         ON CONFLICT(singleton) DO UPDATE SET
            state_json = excluded.state_json,
            updated_at = excluded.updated_at,
            version = request_tab_state.version + 1",
        params![state_json, now_micros()],
    )?;
    Ok(())
}

fn serialize_app_settings(settings: &AppSettings) -> Result<String, DatabaseError> {
    serde_json::to_string(settings).map_err(|error| DatabaseError::CorruptData {
        field: "app settings",
        value: error.to_string(),
    })
}

fn save_app_settings_tx(
    transaction: &Transaction<'_>,
    state_json: &str,
) -> Result<(), DatabaseError> {
    transaction.execute(
        "INSERT INTO app_settings(singleton, state_json, updated_at, version)
         VALUES (1, ?1, ?2, 1)
         ON CONFLICT(singleton) DO UPDATE SET
            state_json = excluded.state_json,
            updated_at = excluded.updated_at,
            version = app_settings.version + 1",
        params![state_json, now_micros()],
    )?;
    Ok(())
}

fn sync_saved_request_headers(
    transaction: &Transaction<'_>,
    saved_request_id: &str,
    headers: &[HeaderEntry],
    saved_at: i64,
) -> Result<(), DatabaseError> {
    for (position, header) in headers.iter().enumerate() {
        transaction.execute(
            "INSERT INTO saved_request_headers(
                saved_request_id, position, enabled, name, value,
                created_at, updated_at, version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 1)
             ON CONFLICT(saved_request_id, position) DO UPDATE SET
                enabled = excluded.enabled,
                name = excluded.name,
                value = excluded.value,
                updated_at = excluded.updated_at,
                version = saved_request_headers.version + 1",
            params![
                saved_request_id,
                to_i64(position, "saved request header position")?,
                bool_to_i64(header.enabled),
                &header.name,
                &header.value,
                saved_at,
            ],
        )?;
    }
    transaction.execute(
        "DELETE FROM saved_request_headers
         WHERE saved_request_id = ?1 AND position >= ?2",
        params![
            saved_request_id,
            to_i64(headers.len(), "saved request header count")?
        ],
    )?;
    Ok(())
}

fn sync_saved_request_body_fields(
    transaction: &Transaction<'_>,
    saved_request_id: &str,
    body_fields: &[BodyField],
    saved_at: i64,
) -> Result<(), DatabaseError> {
    for (position, field) in body_fields.iter().enumerate() {
        transaction.execute(
            "INSERT INTO saved_request_body_fields(
                saved_request_id, position, enabled, name, value, kind,
                created_at, updated_at, version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 1)
             ON CONFLICT(saved_request_id, position) DO UPDATE SET
                enabled = excluded.enabled,
                name = excluded.name,
                value = excluded.value,
                kind = excluded.kind,
                updated_at = excluded.updated_at,
                version = saved_request_body_fields.version + 1",
            params![
                saved_request_id,
                to_i64(position, "saved request body field position")?,
                bool_to_i64(field.enabled),
                &field.name,
                &field.value,
                field.kind.as_db_str(),
                saved_at,
            ],
        )?;
    }
    transaction.execute(
        "DELETE FROM saved_request_body_fields
         WHERE saved_request_id = ?1 AND position >= ?2",
        params![
            saved_request_id,
            to_i64(body_fields.len(), "saved request body field count")?
        ],
    )?;
    Ok(())
}

fn load_workspace_tx(transaction: &Transaction<'_>) -> Result<Workspace, DatabaseError> {
    let collection_rows = {
        let mut statement = transaction.prepare(
            "SELECT id, name
             FROM collections
             ORDER BY position ASC, id ASC",
        )?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut collections = Vec::with_capacity(collection_rows.len());
    for (collection_id, name) in collection_rows {
        let folder_rows = {
            let mut statement = transaction.prepare(
                "SELECT id, name, parent_folder_id
                 FROM collection_folders
                 WHERE collection_id = ?1
                 ORDER BY position ASC, id ASC",
            )?;
            statement
                .query_map(params![&collection_id], |row| {
                    Ok(CollectionFolder {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        parent_folder_id: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };

        let request_rows = {
            let mut statement = transaction.prepare(
                "SELECT
                    id, folder_id, name, method, url, body, body_mode,
                    raw_body_language, pre_request, post_response,
                    created_at, updated_at
                 FROM saved_requests
                 WHERE collection_id = ?1
                 ORDER BY position ASC, id ASC",
            )?;
            statement
                .query_map(params![&collection_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, i64>(11)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };

        let mut requests = Vec::with_capacity(request_rows.len());
        for (
            request_id,
            folder_id,
            request_name,
            method,
            url,
            body,
            body_mode,
            raw_body_language,
            pre_request,
            post_response,
            created_at,
            updated_at,
        ) in request_rows
        {
            let headers = load_headers(
                transaction,
                "SELECT enabled, name, value
                 FROM saved_request_headers
                 WHERE saved_request_id = ?1
                 ORDER BY position ASC",
                &request_id,
                "saved request header enabled",
            )?;
            let body_fields = load_body_fields(
                transaction,
                "SELECT enabled, name, value, kind
                 FROM saved_request_body_fields
                 WHERE saved_request_id = ?1
                 ORDER BY position ASC",
                &request_id,
                "saved request body field enabled",
                "saved request body field kind",
            )?;
            requests.push(SavedRequest {
                id: request_id,
                folder_id,
                name: request_name,
                definition: RequestTemplate {
                    request: RequestDraft {
                        method,
                        url,
                        headers,
                        body,
                        body_mode: body_mode_from_db(&body_mode, "saved request body_mode")?,
                        raw_body_language: raw_body_language_from_db(
                            &raw_body_language,
                            "saved request raw_body_language",
                        )?,
                        body_fields,
                    },
                    scripts: RequestScripts {
                        pre_request,
                        post_response,
                    },
                },
                created_at: datetime_from_micros(created_at, "saved request created_at")?,
                updated_at: datetime_from_micros(updated_at, "saved request updated_at")?,
            });
        }

        collections.push(Collection {
            id: collection_id,
            name,
            folders: folder_rows,
            requests,
        });
    }

    let environment_rows = {
        let mut statement = transaction.prepare(
            "SELECT id, name
             FROM environments
             ORDER BY position ASC, id ASC",
        )?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut environments = Vec::with_capacity(environment_rows.len());
    for (environment_id, name) in environment_rows {
        let variable_rows = {
            let mut statement = transaction.prepare(
                "SELECT id, key, value, enabled, secret
                 FROM environment_variables
                 WHERE environment_id = ?1
                 ORDER BY position ASC, id ASC",
            )?;
            statement
                .query_map(params![&environment_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };

        let mut variables = Vec::with_capacity(variable_rows.len());
        for (id, key, value, enabled, secret) in variable_rows {
            variables.push(EnvironmentVariable {
                id,
                key,
                value,
                enabled: bool_from_i64(enabled, "environment variable enabled")?,
                secret: bool_from_i64(secret, "environment variable secret")?,
            });
        }
        environments.push(Environment {
            id: environment_id,
            name,
            variables,
        });
    }

    let active_environment_id = transaction
        .query_row(
            "SELECT active_environment_id FROM metadata WHERE singleton = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let workspace = Workspace {
        collections,
        environments,
        active_environment_id,
    };
    workspace.validate()?;
    Ok(workspace)
}

fn save_history_tx(
    transaction: &Transaction<'_>,
    history: &RequestHistory,
    history_limit: usize,
) -> Result<(), DatabaseError> {
    let saved_at = now_micros();
    let mut history_ids = HashSet::new();
    for (position, entry) in history.entries().iter().take(history_limit).enumerate() {
        history_ids.insert(entry.id.clone());
        let (
            response_status,
            response_status_text,
            response_duration_ms,
            response_size_bytes,
            response_content_type,
        ) = match entry.response.as_ref() {
            Some(response) => (
                Some(i64::from(response.status)),
                Some(response.status_text.as_str()),
                Some(to_i64(
                    response.duration_ms,
                    "history response duration_ms",
                )?),
                Some(to_i64(response.size_bytes, "history response size_bytes")?),
                response.content_type.as_deref(),
            ),
            None => (None, None, None, None, None),
        };

        transaction.execute(
            "INSERT INTO history_entries(
                id, position, created_at, method, url, body,
                body_mode, raw_body_language, error,
                response_status, response_status_text, response_duration_ms,
                response_size_bytes, response_content_type, updated_at, version
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 1
             )
             ON CONFLICT(id) DO UPDATE SET
                position = excluded.position,
                created_at = excluded.created_at,
                method = excluded.method,
                url = excluded.url,
                body = excluded.body,
                body_mode = excluded.body_mode,
                raw_body_language = excluded.raw_body_language,
                error = excluded.error,
                response_status = excluded.response_status,
                response_status_text = excluded.response_status_text,
                response_duration_ms = excluded.response_duration_ms,
                response_size_bytes = excluded.response_size_bytes,
                response_content_type = excluded.response_content_type,
                updated_at = excluded.updated_at,
                version = history_entries.version + 1",
            params![
                &entry.id,
                to_i64(position, "history position")?,
                entry.created_at.timestamp_micros(),
                &entry.request.method,
                &entry.request.url,
                &entry.request.body,
                entry.request.body_mode.as_db_str(),
                entry.request.raw_body_language.as_db_str(),
                entry.error.as_deref(),
                response_status,
                response_status_text,
                response_duration_ms,
                response_size_bytes,
                response_content_type,
                saved_at,
            ],
        )?;
        sync_history_headers(transaction, &entry.id, &entry.request.headers, saved_at)?;
        sync_history_body_fields(transaction, &entry.id, &entry.request.body_fields, saved_at)?;
    }

    delete_missing_ids(
        transaction,
        "SELECT id FROM history_entries",
        "DELETE FROM history_entries WHERE id = ?1",
        &history_ids,
    )?;
    Ok(())
}

fn sync_history_headers(
    transaction: &Transaction<'_>,
    history_entry_id: &str,
    headers: &[HeaderEntry],
    saved_at: i64,
) -> Result<(), DatabaseError> {
    for (position, header) in headers.iter().enumerate() {
        transaction.execute(
            "INSERT INTO history_headers(
                history_entry_id, position, enabled, name, value,
                created_at, updated_at, version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 1)
             ON CONFLICT(history_entry_id, position) DO UPDATE SET
                enabled = excluded.enabled,
                name = excluded.name,
                value = excluded.value,
                updated_at = excluded.updated_at,
                version = history_headers.version + 1",
            params![
                history_entry_id,
                to_i64(position, "history header position")?,
                bool_to_i64(header.enabled),
                &header.name,
                &header.value,
                saved_at,
            ],
        )?;
    }
    transaction.execute(
        "DELETE FROM history_headers
         WHERE history_entry_id = ?1 AND position >= ?2",
        params![
            history_entry_id,
            to_i64(headers.len(), "history header count")?
        ],
    )?;
    Ok(())
}

fn sync_history_body_fields(
    transaction: &Transaction<'_>,
    history_entry_id: &str,
    body_fields: &[BodyField],
    saved_at: i64,
) -> Result<(), DatabaseError> {
    for (position, field) in body_fields.iter().enumerate() {
        transaction.execute(
            "INSERT INTO history_body_fields(
                history_entry_id, position, enabled, name, value, kind,
                created_at, updated_at, version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 1)
             ON CONFLICT(history_entry_id, position) DO UPDATE SET
                enabled = excluded.enabled,
                name = excluded.name,
                value = excluded.value,
                kind = excluded.kind,
                updated_at = excluded.updated_at,
                version = history_body_fields.version + 1",
            params![
                history_entry_id,
                to_i64(position, "history body field position")?,
                bool_to_i64(field.enabled),
                &field.name,
                &field.value,
                field.kind.as_db_str(),
                saved_at,
            ],
        )?;
    }
    transaction.execute(
        "DELETE FROM history_body_fields
         WHERE history_entry_id = ?1 AND position >= ?2",
        params![
            history_entry_id,
            to_i64(body_fields.len(), "history body field count")?
        ],
    )?;
    Ok(())
}

fn load_history_tx(
    transaction: &Transaction<'_>,
    history_limit: usize,
) -> Result<RequestHistory, DatabaseError> {
    let limit = to_i64(history_limit, "history limit")?;
    let raw_entries = {
        let mut statement = transaction.prepare(
            "SELECT
                id, created_at, method, url, body, body_mode, raw_body_language, error,
                response_status, response_status_text, response_duration_ms,
                response_size_bytes, response_content_type
             FROM history_entries
             ORDER BY position ASC, id ASC
             LIMIT ?1",
        )?;
        statement
            .query_map(params![limit], |row| {
                Ok(RawHistoryEntry {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    method: row.get(2)?,
                    url: row.get(3)?,
                    body: row.get(4)?,
                    body_mode: row.get(5)?,
                    raw_body_language: row.get(6)?,
                    error: row.get(7)?,
                    response_status: row.get(8)?,
                    response_status_text: row.get(9)?,
                    response_duration_ms: row.get(10)?,
                    response_size_bytes: row.get(11)?,
                    response_content_type: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut entries = Vec::with_capacity(raw_entries.len());
    for raw in raw_entries {
        let headers = load_headers(
            transaction,
            "SELECT enabled, name, value
             FROM history_headers
             WHERE history_entry_id = ?1
             ORDER BY position ASC",
            &raw.id,
            "history header enabled",
        )?;
        let body_fields = load_body_fields(
            transaction,
            "SELECT enabled, name, value, kind
             FROM history_body_fields
             WHERE history_entry_id = ?1
             ORDER BY position ASC",
            &raw.id,
            "history body field enabled",
            "history body field kind",
        )?;
        let response = match raw.response_status {
            Some(status) => Some(ResponseSummary {
                status: u16_from_i64(status, "history response status")?,
                status_text: required_when_response(
                    raw.response_status_text,
                    "history response status_text",
                )?,
                duration_ms: u64_from_i64(
                    required_when_response(
                        raw.response_duration_ms,
                        "history response duration_ms",
                    )?,
                    "history response duration_ms",
                )?,
                size_bytes: usize_from_i64(
                    required_when_response(raw.response_size_bytes, "history response size_bytes")?,
                    "history response size_bytes",
                )?,
                content_type: raw.response_content_type,
            }),
            None => {
                if raw.response_status_text.is_some()
                    || raw.response_duration_ms.is_some()
                    || raw.response_size_bytes.is_some()
                    || raw.response_content_type.is_some()
                {
                    return Err(DatabaseError::CorruptData {
                        field: "history response",
                        value: "response columns exist without a status".to_owned(),
                    });
                }
                None
            }
        };

        entries.push(HistoryEntry {
            id: raw.id,
            created_at: datetime_from_micros(raw.created_at, "history created_at")?,
            request: RequestDraft {
                method: raw.method,
                url: raw.url,
                headers,
                body: raw.body,
                body_mode: body_mode_from_db(&raw.body_mode, "history body_mode")?,
                raw_body_language: raw_body_language_from_db(
                    &raw.raw_body_language,
                    "history raw_body_language",
                )?,
                body_fields,
            },
            response,
            error: raw.error,
        });
    }

    Ok(RequestHistory::from_entries(entries, history_limit))
}

struct RawHistoryEntry {
    id: String,
    created_at: i64,
    method: String,
    url: String,
    body: String,
    body_mode: String,
    raw_body_language: String,
    error: Option<String>,
    response_status: Option<i64>,
    response_status_text: Option<String>,
    response_duration_ms: Option<i64>,
    response_size_bytes: Option<i64>,
    response_content_type: Option<String>,
}

fn load_headers(
    transaction: &Transaction<'_>,
    sql: &str,
    parent_id: &str,
    bool_field: &'static str,
) -> Result<Vec<HeaderEntry>, DatabaseError> {
    let raw_headers = {
        let mut statement = transaction.prepare(sql)?;
        statement
            .query_map(params![parent_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    raw_headers
        .into_iter()
        .map(|(enabled, name, value)| {
            Ok(HeaderEntry {
                enabled: bool_from_i64(enabled, bool_field)?,
                name,
                value,
            })
        })
        .collect()
}

fn load_body_fields(
    transaction: &Transaction<'_>,
    sql: &str,
    parent_id: &str,
    bool_field: &'static str,
    kind_field: &'static str,
) -> Result<Vec<BodyField>, DatabaseError> {
    let raw_fields = {
        let mut statement = transaction.prepare(sql)?;
        statement
            .query_map(params![parent_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    raw_fields
        .into_iter()
        .map(|(enabled, name, value, kind)| {
            Ok(BodyField {
                enabled: bool_from_i64(enabled, bool_field)?,
                name,
                value,
                kind: body_field_kind_from_db(&kind, kind_field)?,
            })
        })
        .collect()
}

fn body_mode_from_db(value: &str, field: &'static str) -> Result<BodyMode, DatabaseError> {
    BodyMode::from_db_str(value).ok_or_else(|| DatabaseError::CorruptData {
        field,
        value: value.to_owned(),
    })
}

fn raw_body_language_from_db(
    value: &str,
    field: &'static str,
) -> Result<RawBodyLanguage, DatabaseError> {
    RawBodyLanguage::from_db_str(value).ok_or_else(|| DatabaseError::CorruptData {
        field,
        value: value.to_owned(),
    })
}

fn body_field_kind_from_db(
    value: &str,
    field: &'static str,
) -> Result<BodyFieldKind, DatabaseError> {
    BodyFieldKind::from_db_str(value).ok_or_else(|| DatabaseError::CorruptData {
        field,
        value: value.to_owned(),
    })
}

fn delete_missing_ids(
    transaction: &Transaction<'_>,
    select_sql: &str,
    delete_sql: &str,
    retained_ids: &HashSet<String>,
) -> Result<(), DatabaseError> {
    let existing_ids = {
        let mut statement = transaction.prepare(select_sql)?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for id in existing_ids {
        if !retained_ids.contains(&id) {
            transaction.execute(delete_sql, params![id])?;
        }
    }
    Ok(())
}

fn legacy_import_completed(connection: &Connection, marker: &str) -> Result<bool, DatabaseError> {
    connection
        .query_row(
            "SELECT 1 FROM migration_markers WHERE marker = ?1",
            params![marker],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
        .map_err(DatabaseError::from)
}

fn write_legacy_import_marker(
    transaction: &Transaction<'_>,
    marker: &str,
) -> Result<(), DatabaseError> {
    transaction.execute(
        "INSERT INTO migration_markers(marker, completed_at) VALUES (?1, ?2)",
        params![marker, now_micros()],
    )?;
    Ok(())
}

fn aggregate_is_empty(transaction: &Transaction<'_>, sql: &str) -> Result<bool, DatabaseError> {
    transaction
        .query_row(sql, [], |row| row.get::<_, bool>(0))
        .map_err(DatabaseError::from)
}

#[derive(Debug, Deserialize, Serialize)]
struct LegacyHistoryFile {
    version: u32,
    #[serde(default)]
    entries: Vec<HistoryEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LegacyWorkspaceFile {
    version: u32,
    #[serde(default)]
    collections: Vec<Collection>,
    #[serde(default)]
    environments: Vec<Environment>,
    #[serde(default)]
    active_environment_id: Option<String>,
}

fn read_legacy_history(path: &Path) -> Result<Option<RequestHistory>, DatabaseError> {
    let Some(bytes) = read_optional_legacy_file("history", path)? else {
        return Ok(None);
    };
    let file: LegacyHistoryFile =
        serde_json::from_slice(&bytes).map_err(|source| DatabaseError::LegacyParse {
            kind: "history",
            path: path.to_owned(),
            source,
        })?;
    if file.version != LEGACY_HISTORY_FILE_VERSION {
        return Err(DatabaseError::UnsupportedLegacyVersion {
            kind: "history",
            path: path.to_owned(),
            found: file.version,
            supported: LEGACY_HISTORY_FILE_VERSION,
        });
    }
    Ok(Some(RequestHistory::from_entries(
        file.entries,
        DEFAULT_HISTORY_LIMIT,
    )))
}

fn read_legacy_workspace(path: &Path) -> Result<Option<Workspace>, DatabaseError> {
    let Some(bytes) = read_optional_legacy_file("workspace", path)? else {
        return Ok(None);
    };
    let file: LegacyWorkspaceFile =
        serde_json::from_slice(&bytes).map_err(|source| DatabaseError::LegacyParse {
            kind: "workspace",
            path: path.to_owned(),
            source,
        })?;
    if file.version != WORKSPACE_FILE_VERSION {
        return Err(DatabaseError::UnsupportedLegacyVersion {
            kind: "workspace",
            path: path.to_owned(),
            found: file.version,
            supported: WORKSPACE_FILE_VERSION,
        });
    }
    let workspace = Workspace {
        collections: file.collections,
        environments: file.environments,
        active_environment_id: file.active_environment_id,
    };
    workspace.validate()?;
    Ok(Some(workspace))
}

fn read_optional_legacy_file(
    kind: &'static str,
    path: &Path,
) -> Result<Option<Vec<u8>>, DatabaseError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(DatabaseError::LegacyRead {
            kind,
            path: path.to_owned(),
            source,
        }),
    }
}

fn now_micros() -> i64 {
    Utc::now().timestamp_micros()
}

fn bool_to_i64(value: bool) -> i64 {
    i64::from(value)
}

fn bool_from_i64(value: i64, field: &'static str) -> Result<bool, DatabaseError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DatabaseError::CorruptData {
            field,
            value: value.to_string(),
        }),
    }
}

fn to_i64<T>(value: T, field: &'static str) -> Result<i64, DatabaseError>
where
    T: TryInto<i64> + Copy + ToString,
{
    value
        .try_into()
        .map_err(|_| DatabaseError::IntegerOutOfRange {
            field,
            value: value.to_string(),
        })
}

fn u16_from_i64(value: i64, field: &'static str) -> Result<u16, DatabaseError> {
    u16::try_from(value).map_err(|_| DatabaseError::CorruptData {
        field,
        value: value.to_string(),
    })
}

fn u64_from_i64(value: i64, field: &'static str) -> Result<u64, DatabaseError> {
    u64::try_from(value).map_err(|_| DatabaseError::CorruptData {
        field,
        value: value.to_string(),
    })
}

fn usize_from_i64(value: i64, field: &'static str) -> Result<usize, DatabaseError> {
    usize::try_from(value).map_err(|_| DatabaseError::CorruptData {
        field,
        value: value.to_string(),
    })
}

fn datetime_from_micros(value: i64, field: &'static str) -> Result<DateTime<Utc>, DatabaseError> {
    DateTime::from_timestamp_micros(value).ok_or_else(|| DatabaseError::CorruptData {
        field,
        value: value.to_string(),
    })
}

fn required_when_response<T>(value: Option<T>, field: &'static str) -> Result<T, DatabaseError> {
    value.ok_or_else(|| DatabaseError::CorruptData {
        field,
        value: "NULL".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{SavedTheme, ShortcutOverride, ThemeSettings};

    fn database() -> (tempfile::TempDir, DatabaseStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        (directory, store)
    }

    fn timestamp(micros: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_micros(micros).unwrap()
    }

    fn sample_workspace() -> Workspace {
        Workspace {
            collections: vec![Collection {
                id: "collection-1".to_owned(),
                name: "Development".to_owned(),
                folders: vec![
                    CollectionFolder {
                        id: "folder-1".to_owned(),
                        name: "Users".to_owned(),
                        parent_folder_id: None,
                    },
                    CollectionFolder {
                        id: "folder-2".to_owned(),
                        name: "Administration".to_owned(),
                        parent_folder_id: Some("folder-1".to_owned()),
                    },
                ],
                requests: vec![SavedRequest {
                    id: "request-1".to_owned(),
                    folder_id: Some("folder-2".to_owned()),
                    name: "Create user".to_owned(),
                    definition: RequestTemplate {
                        request: RequestDraft {
                            method: "POST".to_owned(),
                            url: "{{base_url}}/users".to_owned(),
                            headers: vec![
                                HeaderEntry::new("Content-Type", "application/json"),
                                HeaderEntry {
                                    enabled: false,
                                    name: "X-Debug".to_owned(),
                                    value: "off".to_owned(),
                                },
                            ],
                            body: r#"{"name":"Ada"}"#.to_owned(),
                            body_mode: BodyMode::MultipartFormData,
                            raw_body_language: RawBodyLanguage::Json,
                            body_fields: vec![
                                BodyField::text("name", "Ada"),
                                BodyField {
                                    enabled: false,
                                    name: "avatar".to_owned(),
                                    value: "/tmp/avatar.png".to_owned(),
                                    kind: BodyFieldKind::File,
                                },
                            ],
                        },
                        scripts: RequestScripts {
                            pre_request:
                                "api.request.headers.set('X-Token', api.environment.get('token'));"
                                    .to_owned(),
                            post_response:
                                "api.test('created', () => api.assert(api.response.status === 201));"
                                    .to_owned(),
                        },
                    },
                    created_at: timestamp(1_700_000_000_000_001),
                    updated_at: timestamp(1_700_000_000_000_002),
                }],
            }],
            environments: vec![Environment {
                id: "environment-1".to_owned(),
                name: "Local".to_owned(),
                variables: vec![
                    EnvironmentVariable {
                        id: "variable-1".to_owned(),
                        key: "base_url".to_owned(),
                        value: "http://127.0.0.1:8080".to_owned(),
                        enabled: true,
                        secret: false,
                    },
                    EnvironmentVariable {
                        id: "variable-2".to_owned(),
                        key: "token".to_owned(),
                        value: "top-secret".to_owned(),
                        enabled: true,
                        secret: true,
                    },
                ],
            }],
            active_environment_id: Some("environment-1".to_owned()),
        }
    }

    fn sample_history() -> RequestHistory {
        RequestHistory::from_entries(
            vec![
                HistoryEntry {
                    id: "history-2".to_owned(),
                    created_at: timestamp(1_700_000_000_000_020),
                    request: RequestDraft {
                        method: "POST".to_owned(),
                        url: "https://example.test/users".to_owned(),
                        headers: vec![HeaderEntry::new("Content-Type", "application/json")],
                        body: r#"{"name":"Ada"}"#.to_owned(),
                        body_mode: BodyMode::FormUrlEncoded,
                        raw_body_language: RawBodyLanguage::Yaml,
                        body_fields: vec![BodyField::text("name", "Ada")],
                    },
                    response: Some(ResponseSummary {
                        status: 201,
                        status_text: "Created".to_owned(),
                        duration_ms: 42,
                        size_bytes: 18,
                        content_type: Some("application/json".to_owned()),
                    }),
                    error: None,
                },
                HistoryEntry {
                    id: "history-1".to_owned(),
                    created_at: timestamp(1_700_000_000_000_010),
                    request: RequestDraft {
                        method: "GET".to_owned(),
                        url: "https://example.test/failed".to_owned(),
                        headers: vec![],
                        body: String::new(),
                        body_mode: BodyMode::None,
                        raw_body_language: RawBodyLanguage::Text,
                        body_fields: Vec::new(),
                    },
                    response: None,
                    error: Some("connection refused".to_owned()),
                },
            ],
            DEFAULT_HISTORY_LIMIT,
        )
    }

    fn assert_history_eq(left: &RequestHistory, right: &RequestHistory) {
        assert_eq!(
            serde_json::to_value(left.entries()).unwrap(),
            serde_json::to_value(right.entries()).unwrap()
        );
    }

    fn assert_corrupt<T>(
        result: Result<T, DatabaseError>,
        expected_field: &'static str,
        expected_value: &str,
    ) {
        match result {
            Err(DatabaseError::CorruptData { field, value }) => {
                assert_eq!(field, expected_field);
                assert_eq!(value, expected_value);
            }
            Err(error) => panic!("expected corrupt data for {expected_field}, got {error}"),
            Ok(_) => panic!("expected corrupt data for {expected_field}, got success"),
        }
    }

    #[test]
    fn initializes_versioned_schema_and_connection_pragmas() {
        let (_directory, store) = database();
        store.initialize().unwrap();
        store.quick_check().unwrap();
        let connection = store.open_connection().unwrap();

        let schema_version: i64 = connection
            .query_row(
                "SELECT version FROM schema_version WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, CURRENT_SCHEMA_VERSION);

        let foreign_keys: i64 = connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        let journal_mode: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        let busy_timeout: i64 = connection
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        let synchronous: i64 = connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap();
        let application_id: i64 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(busy_timeout, 250);
        assert_eq!(synchronous, 1, "SQLite NORMAL synchronous mode is 1");
        assert_eq!(application_id, i64::from(SQLITE_APPLICATION_ID));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let mode = fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let directory_mode = fs::metadata(store.path().parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(directory_mode, 0o700);
        }

        let tables = {
            let mut statement = connection
                .prepare(
                    "SELECT name
                     FROM sqlite_master
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                     ORDER BY name",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<HashSet<_>, _>>()
                .unwrap()
        };
        for table in [
            "app_settings",
            "collection_folders",
            "collections",
            "environment_variables",
            "environments",
            "history_body_fields",
            "history_entries",
            "history_headers",
            "metadata",
            "migration_markers",
            "request_tab_state",
            "saved_request_body_fields",
            "saved_request_headers",
            "saved_requests",
            "schema_version",
        ] {
            assert!(tables.contains(table), "missing table {table}");
        }

        let assert_cascade = |table: &str, column: &str, target: &str| {
            let mut statement = connection
                .prepare(&format!("PRAGMA foreign_key_list({table})"))
                .unwrap();
            let foreign_keys = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(6)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(
                foreign_keys
                    .iter()
                    .any(
                        |(foreign_table, foreign_column, on_delete)| foreign_table == target
                            && foreign_column == column
                            && on_delete == "CASCADE"
                    ),
                "{table}.{column} must cascade deletion from {target}; found {foreign_keys:?}"
            );
        };
        assert_cascade("saved_requests", "collection_id", "collections");
        assert_cascade("saved_requests", "folder_id", "collection_folders");
        assert_cascade("collection_folders", "collection_id", "collections");
        assert_cascade(
            "collection_folders",
            "parent_folder_id",
            "collection_folders",
        );
        assert_cascade(
            "saved_request_headers",
            "saved_request_id",
            "saved_requests",
        );
        assert_cascade(
            "saved_request_body_fields",
            "saved_request_id",
            "saved_requests",
        );
        assert_cascade("history_body_fields", "history_entry_id", "history_entries");
    }

    #[test]
    fn migrates_handcrafted_v1_rows_with_legacy_body_defaults() {
        let (_directory, store) = database();
        let connection = Connection::open(store.path()).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_version (
                    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
                    version     INTEGER NOT NULL CHECK (version >= 0),
                    updated_at  INTEGER NOT NULL
                 );
                 INSERT INTO schema_version(singleton, version, updated_at)
                 VALUES (1, 1, 1700000000000000);",
            )
            .unwrap();
        connection.execute_batch(MIGRATION_1).unwrap();
        connection
            .execute(
                "INSERT INTO collections(
                    id, name, position, created_at, updated_at, version
                 ) VALUES (?1, ?2, 0, ?3, ?3, 1)",
                params!["v1-collection", "V1 collection", 1_700_000_000_000_001_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO saved_requests(
                    id, collection_id, name, position, method, url, body,
                    pre_request, post_response, created_at, updated_at, version
                 ) VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?9, 1)",
                params![
                    "v1-request",
                    "v1-collection",
                    "V1 request",
                    "POST",
                    "https://example.test/v1",
                    r#"{"legacy":true}"#,
                    "console.log('v1 pre');",
                    "console.log('v1 post');",
                    1_700_000_000_000_002_i64,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO history_entries(
                    id, position, created_at, method, url, body, error,
                    response_status, response_status_text, response_duration_ms,
                    response_size_bytes, response_content_type, updated_at, version
                 ) VALUES (
                    ?1, 0, ?2, ?3, ?4, ?5, NULL,
                    200, 'OK', 12, 15, 'application/json', ?2, 1
                 )",
                params![
                    "v1-history",
                    1_700_000_000_000_003_i64,
                    "POST",
                    "https://example.test/v1",
                    r#"{"legacy":true}"#,
                ],
            )
            .unwrap();
        drop(connection);

        store.initialize().unwrap();
        let migrated = store.load_state().unwrap();
        let migrated_request = &migrated
            .workspace
            .saved_request("v1-request")
            .expect("v1 saved request survives migration")
            .1
            .definition
            .request;
        assert_eq!(migrated_request.method, "POST");
        assert_eq!(migrated_request.body, r#"{"legacy":true}"#);
        assert_eq!(migrated_request.body_mode, BodyMode::Raw);
        assert_eq!(migrated_request.raw_body_language, RawBodyLanguage::Json);
        assert!(migrated_request.body_fields.is_empty());
        let migrated_saved_request = migrated
            .workspace
            .saved_request("v1-request")
            .expect("v1 saved request survives migration")
            .1;
        assert_eq!(migrated_saved_request.folder_id, None);
        assert!(migrated.workspace.collections[0].folders.is_empty());

        let migrated_history = migrated
            .history
            .entries()
            .first()
            .expect("v1 history row survives migration");
        assert_eq!(migrated_history.id, "v1-history");
        assert_eq!(migrated_history.request.body, r#"{"legacy":true}"#);
        assert_eq!(migrated_history.request.body_mode, BodyMode::Raw);
        assert_eq!(
            migrated_history.request.raw_body_language,
            RawBodyLanguage::Json
        );
        assert!(migrated_history.request.body_fields.is_empty());
        assert_eq!(
            migrated_history
                .response
                .as_ref()
                .map(|response| response.status),
            Some(200)
        );

        let connection = store.open_connection().unwrap();
        let schema_version: i64 = connection
            .query_row(
                "SELECT version FROM schema_version WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, CURRENT_SCHEMA_VERSION);
        let saved_defaults: (String, String, Option<String>) = connection
            .query_row(
                "SELECT body_mode, raw_body_language, folder_id
                 FROM saved_requests WHERE id = 'v1-request'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(saved_defaults, ("raw".to_owned(), "json".to_owned(), None));
        let history_defaults: (String, String) = connection
            .query_row(
                "SELECT body_mode, raw_body_language
                 FROM history_entries WHERE id = 'v1-history'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(history_defaults, ("raw".to_owned(), "json".to_owned()));
    }

    #[test]
    fn migrates_handcrafted_v2_requests_to_the_collection_root() {
        let (_directory, store) = database();
        let connection = Connection::open(store.path()).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_version (
                    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
                    version     INTEGER NOT NULL CHECK (version >= 0),
                    updated_at  INTEGER NOT NULL
                 );
                 INSERT INTO schema_version(singleton, version, updated_at)
                 VALUES (1, 2, 1700000000000000);",
            )
            .unwrap();
        connection.execute_batch(MIGRATION_1).unwrap();
        connection.execute_batch(MIGRATION_2).unwrap();
        connection
            .execute(
                "INSERT INTO collections(
                    id, name, position, created_at, updated_at, version
                 ) VALUES (?1, ?2, 0, ?3, ?3, 1)",
                params!["v2-collection", "V2 collection", 1_700_000_000_000_001_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO saved_requests(
                    id, collection_id, name, position, method, url, body,
                    pre_request, post_response, created_at, updated_at, version,
                    body_mode, raw_body_language
                 ) VALUES (
                    ?1, ?2, ?3, 0, ?4, ?5, ?6,
                    '', '', ?7, ?7, 1, 'raw', 'json'
                 )",
                params![
                    "v2-request",
                    "v2-collection",
                    "V2 request",
                    "GET",
                    "https://example.test/v2",
                    "",
                    1_700_000_000_000_002_i64,
                ],
            )
            .unwrap();
        drop(connection);

        store.initialize().unwrap();
        let migrated = store.load_workspace().unwrap();
        let collection = migrated.collection("v2-collection").unwrap();
        assert!(collection.folders.is_empty());
        assert_eq!(collection.requests.len(), 1);
        assert_eq!(collection.requests[0].id, "v2-request");
        assert_eq!(collection.requests[0].folder_id, None);
        assert_eq!(
            collection.requests[0].definition.request.url,
            "https://example.test/v2"
        );

        let connection = store.open_connection().unwrap();
        let schema_version: i64 = connection
            .query_row(
                "SELECT version FROM schema_version WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, CURRENT_SCHEMA_VERSION);
        let folder_id: Option<String> = connection
            .query_row(
                "SELECT folder_id FROM saved_requests WHERE id = 'v2-request'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(folder_id, None);
    }

    #[test]
    fn migrates_handcrafted_v3_database_with_empty_request_tabs() {
        let (_directory, store) = database();
        let connection = Connection::open(store.path()).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_version (
                    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
                    version     INTEGER NOT NULL CHECK (version >= 0),
                    updated_at  INTEGER NOT NULL
                 );
                 INSERT INTO schema_version(singleton, version, updated_at)
                 VALUES (1, 3, 1700000000000000);",
            )
            .unwrap();
        connection.execute_batch(MIGRATION_1).unwrap();
        connection.execute_batch(MIGRATION_2).unwrap();
        connection.execute_batch(MIGRATION_3).unwrap();
        drop(connection);

        store.initialize().unwrap();
        let request_tabs = store.load_request_tabs().unwrap();
        assert_eq!(request_tabs.tabs().len(), 1);
        assert!(!request_tabs.active().is_dirty());

        let connection = store.open_connection().unwrap();
        let schema_version: i64 = connection
            .query_row(
                "SELECT version FROM schema_version WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, CURRENT_SCHEMA_VERSION);
        let request_tab_table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'request_tab_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(request_tab_table_count, 1);
    }

    #[test]
    fn migrates_handcrafted_v4_database_with_default_app_settings() {
        let (_directory, store) = database();
        let connection = Connection::open(store.path()).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_version (
                    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
                    version     INTEGER NOT NULL CHECK (version >= 0),
                    updated_at  INTEGER NOT NULL
                 );
                 INSERT INTO schema_version(singleton, version, updated_at)
                 VALUES (1, 4, 1700000000000000);",
            )
            .unwrap();
        connection.execute_batch(MIGRATION_1).unwrap();
        connection.execute_batch(MIGRATION_2).unwrap();
        connection.execute_batch(MIGRATION_3).unwrap();
        connection.execute_batch(MIGRATION_4).unwrap();
        let mut legacy_request_tabs = RequestTabs::new();
        legacy_request_tabs.open_new();
        legacy_request_tabs
            .active_mut()
            .set_title("Pre-group request tab");
        let legacy_state_json = serde_json::to_string(&legacy_request_tabs).unwrap();
        assert!(
            !legacy_state_json.contains("\"groups\""),
            "empty group metadata should match the pre-group JSON shape"
        );
        connection
            .execute(
                "INSERT INTO request_tab_state(singleton, state_json, updated_at, version)
                 VALUES (1, ?1, 1700000000000000, 1)",
                params![legacy_state_json],
            )
            .unwrap();
        drop(connection);

        store.initialize().unwrap();
        assert_eq!(store.load_app_settings().unwrap(), AppSettings::default());
        assert_eq!(store.load_request_tabs().unwrap(), legacy_request_tabs);

        let connection = store.open_connection().unwrap();
        let schema_version: i64 = connection
            .query_row(
                "SELECT version FROM schema_version WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, CURRENT_SCHEMA_VERSION);
        let app_settings_table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'app_settings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(app_settings_table_count, 1);
        let app_settings_row_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM app_settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(app_settings_row_count, 0);
    }

    #[test]
    fn rejects_a_database_from_a_newer_schema_version() {
        let (_directory, store) = database();
        let connection = Connection::open(store.path()).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_version (
                    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
                    version     INTEGER NOT NULL,
                    updated_at  INTEGER NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO schema_version(singleton, version, updated_at)
                 VALUES (1, ?1, ?2)",
                params![CURRENT_SCHEMA_VERSION + 1, now_micros()],
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            store.initialize(),
            Err(DatabaseError::UnsupportedSchemaVersion {
                found,
                supported: CURRENT_SCHEMA_VERSION,
            }) if found == CURRENT_SCHEMA_VERSION + 1
        ));
    }

    #[test]
    fn rejects_corrupt_body_modes_languages_and_field_kinds() {
        let (_directory, store) = database();
        store
            .save_state(&sample_workspace(), &sample_history())
            .unwrap();
        let connection = store.open_connection().unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", true)
            .unwrap();

        connection
            .execute(
                "UPDATE saved_requests SET body_mode = 'broken_mode'
                 WHERE id = 'request-1'",
                [],
            )
            .unwrap();
        assert_corrupt(
            store.load_workspace(),
            "saved request body_mode",
            "broken_mode",
        );
        connection
            .execute(
                "UPDATE saved_requests SET body_mode = 'raw',
                    raw_body_language = 'broken_language'
                 WHERE id = 'request-1'",
                [],
            )
            .unwrap();
        assert_corrupt(
            store.load_workspace(),
            "saved request raw_body_language",
            "broken_language",
        );
        connection
            .execute(
                "UPDATE saved_requests SET raw_body_language = 'json'
                 WHERE id = 'request-1'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE saved_request_body_fields SET kind = 'broken_kind'
                 WHERE saved_request_id = 'request-1' AND position = 0",
                [],
            )
            .unwrap();
        assert_corrupt(
            store.load_workspace(),
            "saved request body field kind",
            "broken_kind",
        );

        connection
            .execute(
                "UPDATE history_entries SET body_mode = 'broken_mode'
                 WHERE id = 'history-2'",
                [],
            )
            .unwrap();
        assert_corrupt(store.load_history(), "history body_mode", "broken_mode");
        connection
            .execute(
                "UPDATE history_entries SET body_mode = 'raw',
                    raw_body_language = 'broken_language'
                 WHERE id = 'history-2'",
                [],
            )
            .unwrap();
        assert_corrupt(
            store.load_history(),
            "history raw_body_language",
            "broken_language",
        );
        connection
            .execute(
                "UPDATE history_entries SET raw_body_language = 'json'
                 WHERE id = 'history-2'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE history_body_fields SET kind = 'broken_kind'
                 WHERE history_entry_id = 'history-2' AND position = 0",
                [],
            )
            .unwrap();
        assert_corrupt(
            store.load_history(),
            "history body field kind",
            "broken_kind",
        );
    }

    #[test]
    fn workspace_and_history_round_trip_as_one_aggregate() {
        let (_directory, store) = database();
        let workspace = sample_workspace();
        let history = sample_history();

        store.save_state(&workspace, &history).unwrap();
        let loaded = store.load_state().unwrap();
        assert_eq!(loaded.workspace, workspace);
        assert_history_eq(&loaded.history, &history);

        store.save_state(&workspace, &history).unwrap();
        let connection = store.open_connection().unwrap();
        let collection_version: i64 = connection
            .query_row(
                "SELECT version FROM collections WHERE id = ?1",
                params!["collection-1"],
                |row| row.get(0),
            )
            .unwrap();
        let request_version: i64 = connection
            .query_row(
                "SELECT version FROM saved_requests WHERE id = ?1",
                params!["request-1"],
                |row| row.get(0),
            )
            .unwrap();
        let folder_version: i64 = connection
            .query_row(
                "SELECT version FROM collection_folders WHERE id = ?1",
                params!["folder-1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(collection_version, 2);
        assert_eq!(folder_version, 2);
        assert_eq!(request_version, 2);

        let foreign_key_violation: Option<String> = connection
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
            .optional()
            .unwrap();
        assert_eq!(foreign_key_violation, None);
    }

    #[test]
    fn folder_round_trip_preserves_flat_order_when_a_child_precedes_its_parent() {
        let (_directory, store) = database();
        let mut workspace = sample_workspace();
        workspace.collections[0].folders.reverse();

        store.save_workspace(&workspace).unwrap();

        let loaded = store.load_workspace().unwrap();
        assert_eq!(loaded, workspace);
        assert_eq!(
            loaded.collections[0]
                .folders
                .iter()
                .map(|folder| folder.id.as_str())
                .collect::<Vec<_>>(),
            ["folder-2", "folder-1"]
        );
    }

    #[test]
    fn folder_deletion_cascades_descendants_and_nested_request_rows() {
        let (_directory, store) = database();
        let mut workspace = sample_workspace();
        let mut root_request = workspace.collections[0].requests[0].clone();
        root_request.id = "request-root".to_owned();
        root_request.name = "Root request".to_owned();
        root_request.folder_id = None;
        workspace.collections[0].requests.push(root_request);
        store.save_workspace(&workspace).unwrap();

        let connection = store.open_connection().unwrap();
        connection
            .execute("DELETE FROM collection_folders WHERE id = 'folder-1'", [])
            .unwrap();

        let count = |sql: &str| {
            connection
                .query_row(sql, [], |row| row.get::<_, i64>(0))
                .unwrap()
        };
        assert_eq!(
            count("SELECT count(*) FROM collection_folders"),
            0,
            "deleting a parent folder must delete every descendant"
        );
        assert_eq!(
            count("SELECT count(*) FROM saved_requests WHERE id = 'request-1'"),
            0,
            "the nested request must follow its deleted folder"
        );
        assert_eq!(
            count(
                "SELECT count(*) FROM saved_request_headers
                 WHERE saved_request_id = 'request-1'"
            ),
            0
        );
        assert_eq!(
            count(
                "SELECT count(*) FROM saved_request_body_fields
                 WHERE saved_request_id = 'request-1'"
            ),
            0
        );
        assert_eq!(
            count("SELECT count(*) FROM saved_requests WHERE id = 'request-root'"),
            1,
            "a root request in the same collection must survive folder deletion"
        );
        let foreign_key_violation: Option<String> = connection
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
            .optional()
            .unwrap();
        assert_eq!(foreign_key_violation, None);
        drop(connection);

        let loaded = store.load_workspace().unwrap();
        let collection = loaded.collection("collection-1").unwrap();
        assert!(collection.folders.is_empty());
        assert_eq!(
            collection
                .requests
                .iter()
                .map(|request| request.id.as_str())
                .collect::<Vec<_>>(),
            ["request-root"]
        );
        assert_eq!(collection.requests[0].folder_id, None);

        let connection = store.open_connection().unwrap();
        connection
            .execute("DELETE FROM collections WHERE id = 'collection-1'", [])
            .unwrap();
        for table in [
            "collection_folders",
            "saved_requests",
            "saved_request_headers",
            "saved_request_body_fields",
        ] {
            let remaining: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(
                remaining, 0,
                "deleting a collection must cascade through {table}"
            );
        }
    }

    #[test]
    fn moving_a_request_to_root_before_removing_its_folder_preserves_it() {
        let (_directory, store) = database();
        let mut workspace = sample_workspace();
        store.save_workspace(&workspace).unwrap();

        workspace.collections[0].requests[0].folder_id = None;
        workspace.collections[0].folders.clear();
        store.save_workspace(&workspace).unwrap();

        let loaded = store.load_workspace().unwrap();
        assert_eq!(loaded, workspace);
        let connection = store.open_connection().unwrap();
        let persisted: (Option<String>, i64, i64) = connection
            .query_row(
                "SELECT
                    folder_id,
                    (SELECT count(*) FROM saved_request_headers
                     WHERE saved_request_id = saved_requests.id),
                    (SELECT count(*) FROM saved_request_body_fields
                     WHERE saved_request_id = saved_requests.id)
                 FROM saved_requests
                 WHERE id = 'request-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(persisted, (None, 2, 2));
    }

    #[test]
    fn foreign_keys_reject_dangling_folder_relationships() {
        let (_directory, store) = database();
        store.save_workspace(&sample_workspace()).unwrap();
        let connection = store.open_connection().unwrap();

        assert!(
            connection
                .execute(
                    "UPDATE saved_requests
                     SET folder_id = 'missing-folder'
                     WHERE id = 'request-1'",
                    [],
                )
                .is_err(),
            "a request cannot reference a missing folder"
        );
        assert!(
            connection
                .execute(
                    "UPDATE collection_folders
                     SET parent_folder_id = 'missing-folder'
                     WHERE id = 'folder-2'",
                    [],
                )
                .is_err(),
            "a folder cannot reference a missing parent"
        );

        let stored_relationships: (Option<String>, Option<String>) = connection
            .query_row(
                "SELECT
                    (SELECT folder_id FROM saved_requests WHERE id = 'request-1'),
                    (SELECT parent_folder_id FROM collection_folders WHERE id = 'folder-2')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            stored_relationships,
            (Some("folder-2".to_owned()), Some("folder-1".to_owned()))
        );
    }

    #[test]
    fn load_rejects_cross_collection_folder_relationships() {
        let (_directory, store) = database();
        let mut workspace = sample_workspace();
        workspace.collections.push(Collection {
            id: "collection-2".to_owned(),
            name: "Other".to_owned(),
            folders: Vec::new(),
            requests: Vec::new(),
        });
        store.save_workspace(&workspace).unwrap();
        let connection = store.open_connection().unwrap();

        // Both referenced rows exist, so ordinary foreign keys alone cannot
        // express that a request and its folder must have the same owner.
        connection
            .execute(
                "UPDATE saved_requests
                 SET collection_id = 'collection-2'
                 WHERE id = 'request-1'",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.load_workspace(),
            Err(DatabaseError::InvalidWorkspace(
                WorkspaceValidationError::RequestFolderOutsideCollection {
                    collection_id,
                    request_id,
                    folder_id,
                    folder_collection_id,
                }
            )) if collection_id == "collection-2"
                && request_id == "request-1"
                && folder_id == "folder-2"
                && folder_collection_id == "collection-1"
        ));

        connection
            .execute(
                "UPDATE saved_requests
                 SET collection_id = 'collection-1',
                     folder_id = 'folder-1'
                 WHERE id = 'request-1'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE collection_folders
                 SET collection_id = 'collection-2'
                 WHERE id = 'folder-2'",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.load_workspace(),
            Err(DatabaseError::InvalidWorkspace(
                WorkspaceValidationError::FolderParentOutsideCollection {
                    collection_id,
                    folder_id,
                    parent_folder_id,
                    parent_collection_id,
                }
            )) if collection_id == "collection-2"
                && folder_id == "folder-2"
                && parent_folder_id == "folder-1"
                && parent_collection_id == "collection-1"
        ));
    }

    #[test]
    fn aggregate_save_rolls_back_and_foreign_keys_reject_dangling_metadata() {
        let (_directory, store) = database();
        let baseline_workspace = sample_workspace();
        let baseline_history = sample_history();
        store
            .save_state(&baseline_workspace, &baseline_history)
            .unwrap();

        let mut changed_workspace = baseline_workspace.clone();
        changed_workspace.collections[0].name = "Must roll back".to_owned();
        let overflowing_history = RequestHistory::from_entries(
            vec![HistoryEntry {
                id: "overflow".to_owned(),
                created_at: timestamp(1_700_000_000_000_030),
                request: RequestDraft::new("GET", "https://example.test"),
                response: Some(ResponseSummary {
                    status: 200,
                    status_text: "OK".to_owned(),
                    duration_ms: u64::MAX,
                    size_bytes: 0,
                    content_type: None,
                }),
                error: None,
            }],
            DEFAULT_HISTORY_LIMIT,
        );

        assert!(matches!(
            store.save_state(&changed_workspace, &overflowing_history),
            Err(DatabaseError::IntegerOutOfRange {
                field: "history response duration_ms",
                ..
            })
        ));
        let after_failure = store.load_state().unwrap();
        assert_eq!(after_failure.workspace, baseline_workspace);
        assert_history_eq(&after_failure.history, &baseline_history);

        let connection = store.open_connection().unwrap();
        let constraint_error = connection
            .execute(
                "UPDATE metadata
                 SET active_environment_id = ?1
                 WHERE singleton = 1",
                params!["missing-environment"],
            )
            .unwrap_err();
        assert!(matches!(
            constraint_error,
            rusqlite::Error::SqliteFailure(_, _)
        ));
    }

    #[test]
    fn legacy_json_import_is_atomic_non_destructive_and_idempotent() {
        let (directory, store) = database();
        let history_path = directory.path().join("history.json");
        let workspace_path = directory.path().join("workspace.json");

        assert_eq!(
            store
                .import_legacy_json(&history_path, &workspace_path)
                .unwrap(),
            LegacyImportOutcome::NoFiles
        );

        let workspace = sample_workspace();
        let history = sample_history();
        let history_file = LegacyHistoryFile {
            version: LEGACY_HISTORY_FILE_VERSION,
            entries: history.entries().to_vec(),
        };
        let workspace_file = LegacyWorkspaceFile {
            version: WORKSPACE_FILE_VERSION,
            collections: workspace.collections.clone(),
            environments: workspace.environments.clone(),
            active_environment_id: workspace.active_environment_id.clone(),
        };
        fs::write(
            &history_path,
            serde_json::to_vec_pretty(&history_file).unwrap(),
        )
        .unwrap();
        fs::write(
            &workspace_path,
            serde_json::to_vec_pretty(&workspace_file).unwrap(),
        )
        .unwrap();

        assert_eq!(
            store
                .import_legacy_json(&history_path, &workspace_path)
                .unwrap(),
            LegacyImportOutcome::Imported {
                workspace_imported: true,
                history_imported: true,
            }
        );
        assert!(history_path.exists());
        assert!(workspace_path.exists());
        let imported = store.load_state().unwrap();
        assert_eq!(imported.workspace, workspace);
        assert_history_eq(&imported.history, &history);

        // The marker is checked before either legacy file is parsed.
        fs::write(&workspace_path, b"not valid JSON").unwrap();
        assert_eq!(
            store
                .import_legacy_json(&history_path, &workspace_path)
                .unwrap(),
            LegacyImportOutcome::AlreadyImported
        );
        let connection = store.open_connection().unwrap();
        let marker_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM migration_markers
                 WHERE marker IN (?1, ?2)",
                params![LEGACY_HISTORY_IMPORT_MARKER, LEGACY_WORKSPACE_IMPORT_MARKER],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker_count, 2);
    }

    #[test]
    fn corrupt_legacy_history_does_not_block_workspace_import() {
        let (directory, store) = database();
        let history_path = directory.path().join("history.json");
        let workspace_path = directory.path().join("workspace.json");
        let workspace = sample_workspace();
        let workspace_file = LegacyWorkspaceFile {
            version: WORKSPACE_FILE_VERSION,
            collections: workspace.collections.clone(),
            environments: workspace.environments.clone(),
            active_environment_id: workspace.active_environment_id.clone(),
        };
        fs::write(&history_path, b"{broken").unwrap();
        fs::write(
            &workspace_path,
            serde_json::to_vec_pretty(&workspace_file).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            store.import_legacy_json(&history_path, &workspace_path),
            Err(DatabaseError::LegacyParse {
                kind: "history",
                ..
            })
        ));
        assert_eq!(store.load_workspace().unwrap(), workspace);
        assert!(store.load_history().unwrap().is_empty());

        let connection = store.open_connection().unwrap();
        assert!(!legacy_import_completed(&connection, LEGACY_HISTORY_IMPORT_MARKER).unwrap());
        assert!(legacy_import_completed(&connection, LEGACY_WORKSPACE_IMPORT_MARKER).unwrap());
        drop(connection);

        let history = sample_history();
        let history_file = LegacyHistoryFile {
            version: LEGACY_HISTORY_FILE_VERSION,
            entries: history.entries().to_vec(),
        };
        fs::write(
            &history_path,
            serde_json::to_vec_pretty(&history_file).unwrap(),
        )
        .unwrap();
        assert_eq!(
            store
                .import_legacy_json(&history_path, &workspace_path)
                .unwrap(),
            LegacyImportOutcome::Imported {
                workspace_imported: false,
                history_imported: true,
            }
        );
        assert_history_eq(&store.load_history().unwrap(), &history);
    }

    #[test]
    fn request_tabs_round_trip_and_corrupt_state_is_rejected() {
        let (_directory, store) = database();
        let mut request_tabs = RequestTabs::new();
        request_tabs.open_new();
        request_tabs.active_mut().set_title("Second request");
        request_tabs
            .active_mut()
            .set_template(RequestTemplate::new(RequestDraft::new(
                "GET",
                "https://tabs.example.test",
            )));
        let grouped_tab_id = request_tabs.active_tab_id().clone();
        let group_id = request_tabs
            .create_group_for_tab(&grouped_tab_id, "Integration", RequestTabGroupColor::Blue)
            .expect("the open tab should be groupable");
        assert!(request_tabs.set_group_collapsed(&group_id, true));

        store.save_request_tabs(&request_tabs).unwrap();
        assert_eq!(store.load_request_tabs().unwrap(), request_tabs);

        let connection = store.open_connection().unwrap();
        connection
            .execute(
                "UPDATE request_tab_state SET state_json = '{broken' WHERE singleton = 1",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.load_request_tabs(),
            Err(DatabaseError::CorruptData {
                field: "request tab state",
                ..
            })
        ));
    }

    #[test]
    fn app_settings_round_trip_increments_version_and_rejects_corrupt_state() {
        let (_directory, store) = database();
        let saved_theme = SavedTheme::new(
            "SQLite Ocean",
            ":root { --api-background: #14121a; }",
            Some(PathBuf::from("/tmp/theme.css")),
        );
        let settings = AppSettings {
            shortcuts: std::collections::BTreeMap::from([
                (
                    "request.send".to_owned(),
                    ShortcutOverride::Custom("cmd-enter".to_owned()),
                ),
                ("request.close_tab".to_owned(), ShortcutOverride::Disabled),
            ]),
            theme: ThemeSettings {
                saved_themes: vec![saved_theme.clone()],
                active_theme_id: Some(saved_theme.id),
                source_path: Some(PathBuf::from("/tmp/theme.css")),
                css_source: Some(":root { --api-background: #14121a; }".to_owned()),
                draft_source: Some(":root { --api-background: #20202a; }".to_owned()),
                draft_path: Some(PathBuf::from("/tmp/theme-draft.css")),
                draft_disk_source: Some(":root { --api-background: #14121a; }".to_owned()),
                ..Default::default()
            },
            navigation_compact: true,
            ..Default::default()
        };

        store.save_app_settings(&settings).unwrap();
        assert_eq!(store.load_app_settings().unwrap(), settings);
        store.save_app_settings(&settings).unwrap();

        let connection = store.open_connection().unwrap();
        let version: i64 = connection
            .query_row(
                "SELECT version FROM app_settings WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 2);
        connection
            .execute(
                "UPDATE app_settings SET state_json = '{broken' WHERE singleton = 1",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.load_app_settings(),
            Err(DatabaseError::CorruptData {
                field: "app settings",
                ..
            })
        ));
    }

    #[test]
    fn workspace_and_request_tabs_commit_atomically() {
        let (_directory, store) = database();
        let workspace = sample_workspace();
        let mut request_tabs = RequestTabs::new();
        request_tabs.active_mut().set_title("Atomic tab");

        store
            .save_workspace_and_request_tabs(&workspace, &request_tabs)
            .unwrap();
        assert_eq!(store.load_workspace().unwrap(), workspace);
        assert_eq!(store.load_request_tabs().unwrap(), request_tabs);

        let mut changed_workspace = workspace.clone();
        changed_workspace.collections[0].name = "Must not commit".to_owned();
        let mut rejected_tabs = request_tabs.clone();
        rejected_tabs.active_mut().set_title("Must not commit");

        let connection = store.open_connection().unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_request_tab_state_update
                 BEFORE UPDATE ON request_tab_state
                 BEGIN
                    SELECT RAISE(ABORT, 'forced request tab state failure');
                 END;",
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            store.save_workspace_and_request_tabs(&changed_workspace, &rejected_tabs),
            Err(DatabaseError::Sql(rusqlite::Error::SqliteFailure(_, _)))
        ));
        assert_eq!(store.load_workspace().unwrap(), workspace);
        assert_eq!(store.load_request_tabs().unwrap(), request_tabs);
    }
}

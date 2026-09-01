mod chain;
mod database;
mod format;
mod history;
mod interchange;
mod realtime;
mod request;
mod request_namespace;
mod request_tabs;
mod script;
mod secure_store;
mod settings;
mod snippet;
mod template;
mod upstream;
mod upstream_management;
mod websocket;
mod workspace;
mod workspace_provider;

#[cfg(test)]
mod mvp_smoke_test;

pub use chain::{ChainFailure, ChainLimits, ChainRun, run_chain};
pub use database::{DatabaseStore, LocalWorkspace};
pub use format::{
    format_body, format_raw_source, format_script_source, is_probably_text, parse_json_lines,
};
pub use history::{HistoryEntry, REDACTED_VALUE, RequestHistory};
pub use interchange::{
    ImportBundle, InterchangeFormat, MAX_INTERCHANGE_BYTES, export_request, import_requests,
};
pub use realtime::{RealtimeResourceChange, RealtimeSignal, watch_upstream_changes};
pub use request::{
    BodyField, BodyFieldKind, BodyMode, HeaderEntry, QueryParamEntry, RawBodyLanguage,
    RequestDraft, RequestError, RequestTask, ResponseData, STANDARD_HTTP_METHODS, build_client,
    query_params_from_url, send_request, spawn_request, url_with_query_params,
};
#[allow(unused_imports)]
pub use request_namespace::{
    AccessStep, CHAIN_MAX_DEPTH, CHAIN_MAX_TOTAL, NAMESPACE_REF_MARKER, NodeKind, NodeStatus,
    REQUEST_REF_MARKER, RequestNamespaceCatalog, RequestNamespaceNode, RequestRefInfo,
    RuntimeNamespaceSpec, RuntimeNodeKind, is_valid_js_identifier,
};
pub use request_tabs::{
    DEFAULT_REQUEST_TAB_TITLE, RequestTabAssociation, RequestTabCloseScope, RequestTabGroup,
    RequestTabGroupColor, RequestTabGroupId, RequestTabId, RequestTabRecord, RequestTabs,
};
pub use script::{
    ChainedRequest, EnvironmentMutation, InlineChainer, MAX_SCRIPT_SOURCE_BYTES,
    PostResponseResult, PreRequestResult, ScriptCancellation, ScriptDiagnostic, ScriptEnvironment,
    ScriptError, ScriptErrorKind, ScriptLogLevel, ScriptPhase, ScriptReport, ScriptScope,
    execute_post_response_with_chain, execute_pre_request_with_chain,
};
#[cfg(test)]
pub use script::{ScriptLog, ScriptTestResult};
#[allow(unused_imports)]
pub use secure_store::{CredentialVault, CredentialVaultError, UpstreamCredential};
#[allow(unused_imports)]
pub use settings::{
    AppSettings, DEFAULT_ZOOM_PERCENT, EditorSettings, FormatterQuoteStyle, FormatterSemicolons,
    FormatterSettings, FormatterTrailingCommas, MAX_ZOOM_PERCENT, MIN_ZOOM_PERCENT, MetricsPosition,
    SavedTheme, ShortcutOverride, ThemeSettings, ZOOM_STEP_PERCENT, ZoomSettings,
};
pub(crate) use snippet::{GENERATOR_WRAPPER_PREFIX, GENERATOR_WRAPPER_SUFFIX};
pub use snippet::{
    MAX_SNIPPET_NAME_BYTES, Snippet, SnippetCancellation, SnippetCategory,
    SnippetInvocationContext, SnippetKind, SnippetLog, SnippetLogLevel, SnippetRequirement,
    SnippetSelection, SnippetSelectionArea, SnippetSelectionSource, SnippetTextRange,
    generate_snippet,
};
pub use template::{RequestTemplate, ResolvedRequest, resolve_request};
#[allow(unused_imports)]
pub use upstream::{
    LoginPermission, LoginRole, LoginUser, UpstreamCollectionView, UpstreamEnvironmentVariableView,
    UpstreamEnvironmentView, UpstreamLoginError, UpstreamLoginResult, UpstreamProfile,
    UpstreamSavedRequestView, UpstreamSettings, UpstreamUrlError, UpstreamUserSummary,
    UpstreamWorkspaceError, UpstreamWorkspaceSummary, UpstreamWorkspaceView,
    add_upstream_proxy_allowlist_entry, build_upstream_client, build_upstream_execution_client,
    create_upstream_collection, create_upstream_environment, create_upstream_environment_variable,
    create_upstream_saved_request, create_upstream_workspace, delete_upstream_collection,
    delete_upstream_environment, delete_upstream_environment_variable,
    delete_upstream_saved_request, delete_upstream_workspace, execute_upstream_request,
    get_upstream_execution_policy, get_upstream_user, get_upstream_workspace,
    list_upstream_environments, list_upstream_workspaces, login_upstream, move_upstream_collection,
    move_upstream_saved_request, normalize_upstream_url, put_upstream_environment_variable_value,
    save_upstream_environment, send_request_for_upstream_workspace, update_upstream_collection,
    update_upstream_environment, update_upstream_environment_variable,
    update_upstream_saved_request, update_upstream_workspace, upstream_url_label,
};
#[allow(unused_imports)]
pub use upstream_management::{
    AUDIT_READ, ActivityLogDiff, ActivityLogEntry, ActivityLogPage, COLLECTIONS_ASSIGN_USERS,
    COLLECTIONS_CREATE, COLLECTIONS_DELETE, COLLECTIONS_READ, COLLECTIONS_UPDATE,
    ENVIRONMENT_VALUES_UPDATE, ENVIRONMENTS_CREATE, ENVIRONMENTS_DELETE, ENVIRONMENTS_READ,
    ENVIRONMENTS_UPDATE, HISTORY_READ_OTHERS, HostnameOverride, ManagementPermission,
    ManagementRole, ManagementUser, PERMISSIONS_READ, ProfileView, REQUESTS_CREATE,
    REQUESTS_DELETE, REQUESTS_READ, REQUESTS_UPDATE, ROLES_ASSIGN_PERMISSIONS, ROLES_CREATE,
    ROLES_READ, ROLES_UPDATE, RequestExecutionMode, RequestExecutionSettings, SERVER_SETTINGS_READ,
    SERVER_SETTINGS_UPDATE, SharedHistoryBodyField, SharedHistoryEntry, SharedHistoryHeader,
    SharedHistoryRequest, SharedHistoryResponse, SharedHistoryUpload, USERS_ASSIGN_ROLES,
    USERS_CREATE, USERS_READ, USERS_UPDATE, UpstreamManagementError, UpstreamManagementSnapshot,
    WORKSPACES_ASSIGN_USERS, WORKSPACES_CREATE, WORKSPACES_DELETE, WORKSPACES_READ,
    WORKSPACES_UPDATE, create_management_role, create_management_user, delete_shared_history,
    list_audit_activity, list_shared_history, list_workspace_activity, load_upstream_management,
    replace_management_collection_users, replace_management_role_permissions,
    replace_management_user_roles, replace_management_workspace_users, update_management_role,
    update_management_user, update_request_execution_settings, upload_shared_history,
};
pub use websocket::{
    MAX_WEBSOCKET_TIMELINE_ENTRIES, WebSocketAutomationEvent, WebSocketCommand,
    WebSocketMessageTemplate, WebSocketReplay, WebSocketSavedMessage, WebSocketSignal,
    WebSocketWorkspace, binary_preview, execute_websocket_automation, render_message_template,
    replay_frames, replay_websocket_frames, run_websocket_connection, template_variable_names,
};
pub use workspace::{
    Collection, CollectionFolder, Environment, RequestScripts, ResourceCreator, SavedRequest,
    Workspace, WorkspaceMutationError, apply_environment_mutations_to_workspace,
};
#[allow(unused_imports)]
pub use workspace_provider::{
    LocalWorkspaceProvider, RemoteWorkspaceProvider, WorkspaceProvider, WorkspaceProviderError,
    WorkspaceProviderId, WorkspaceProviderRegistry,
};

/// Persisted enums that can be read back from their on-disk string via
/// `from_db_str`. `crate::core::database::enum_from_db` wraps those reads into a
/// `DatabaseError::CorruptData` for unknown values instead of each call site
/// repeating the same boilerplate.
pub(crate) trait DbStringEnum: Sized {
    fn from_db_str(value: &str) -> Option<Self>;
}

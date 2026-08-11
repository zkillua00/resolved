mod database;
mod format;
mod history;
mod interchange;
mod request;
mod request_tabs;
mod script;
mod secure_store;
mod settings;
mod snippet;
mod template;
mod upstream;
mod upstream_management;
mod workspace;
mod workspace_provider;

#[cfg(test)]
mod mvp_smoke_test;

pub use database::{DatabaseStore, LocalWorkspace};
pub use format::{format_body, format_raw_source, format_script_source, is_probably_text};
pub use history::{HistoryEntry, REDACTED_VALUE, RequestHistory};
pub use interchange::{
    ImportBundle, InterchangeFormat, MAX_INTERCHANGE_BYTES, export_request, import_requests,
};
pub use request::{
    BodyField, BodyFieldKind, BodyMode, HeaderEntry, RawBodyLanguage, RequestDraft, RequestError,
    RequestTask, ResponseData, STANDARD_HTTP_METHODS, build_client, spawn_request,
};
pub use request_tabs::{
    DEFAULT_REQUEST_TAB_TITLE, RequestTabAssociation, RequestTabCloseScope, RequestTabGroup,
    RequestTabGroupColor, RequestTabGroupId, RequestTabId, RequestTabRecord, RequestTabs,
};
pub use script::{
    EnvironmentMutation, MAX_SCRIPT_SOURCE_BYTES, PostResponseResult, PreRequestResult,
    ScriptCancellation, ScriptDiagnostic, ScriptEnvironment, ScriptError, ScriptErrorKind,
    ScriptLogLevel, ScriptPhase, ScriptReport, ScriptScope, execute_post_response,
    execute_pre_request,
};
#[cfg(test)]
pub use script::{ScriptLog, ScriptTestResult};
#[allow(unused_imports)]
pub use secure_store::{CredentialVault, CredentialVaultError, UpstreamCredential};
#[allow(unused_imports)]
pub use settings::{
    AppSettings, EditorSettings, FormatterQuoteStyle, FormatterSemicolons, FormatterSettings,
    FormatterTrailingCommas, MetricsPosition, SavedTheme, ShortcutOverride, ThemeSettings,
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
    UpstreamWorkspaceError, UpstreamWorkspaceSummary, UpstreamWorkspaceView, build_upstream_client,
    create_upstream_collection, create_upstream_environment, create_upstream_environment_variable,
    create_upstream_saved_request, create_upstream_workspace, delete_upstream_collection,
    delete_upstream_environment, delete_upstream_environment_variable,
    delete_upstream_saved_request, delete_upstream_workspace, get_upstream_user,
    get_upstream_workspace, list_upstream_environments, list_upstream_workspaces, login_upstream,
    move_upstream_collection, move_upstream_saved_request, normalize_upstream_url,
    put_upstream_environment_variable_value, save_upstream_environment, update_upstream_collection,
    update_upstream_environment, update_upstream_environment_variable,
    update_upstream_saved_request, update_upstream_workspace, upstream_url_label,
};
#[allow(unused_imports)]
pub use upstream_management::{
    COLLECTIONS_ASSIGN_USERS, COLLECTIONS_CREATE, COLLECTIONS_DELETE, COLLECTIONS_READ,
    COLLECTIONS_UPDATE, ENVIRONMENT_VALUES_UPDATE, ENVIRONMENTS_CREATE, ENVIRONMENTS_DELETE,
    ENVIRONMENTS_READ, ENVIRONMENTS_UPDATE, ManagementPermission, ManagementRole, ManagementUser,
    PERMISSIONS_READ, REQUESTS_CREATE, REQUESTS_DELETE, REQUESTS_READ, REQUESTS_UPDATE,
    ROLES_ASSIGN_PERMISSIONS, ROLES_CREATE, ROLES_READ, ROLES_UPDATE, USERS_ASSIGN_ROLES,
    USERS_CREATE, USERS_READ, USERS_UPDATE, UpstreamManagementError, UpstreamManagementSnapshot,
    WORKSPACES_ASSIGN_USERS, WORKSPACES_CREATE, WORKSPACES_DELETE, WORKSPACES_READ,
    WORKSPACES_UPDATE, create_management_role, create_management_user, load_upstream_management,
    replace_management_collection_users, replace_management_role_permissions,
    replace_management_user_roles, replace_management_workspace_users, update_management_role,
    update_management_user,
};
pub use workspace::{
    Collection, CollectionFolder, Environment, RequestScripts, SavedRequest, Workspace,
    WorkspaceMutationError,
};
#[allow(unused_imports)]
pub use workspace_provider::{
    LocalWorkspaceProvider, RemoteWorkspaceProvider, WorkspaceProvider, WorkspaceProviderError,
    WorkspaceProviderId, WorkspaceProviderRegistry,
};

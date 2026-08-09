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
mod workspace;
mod workspace_provider;

#[cfg(test)]
mod mvp_smoke_test;

pub use database::DatabaseStore;
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
    LoginUser, UpstreamLoginError, UpstreamLoginResult, UpstreamProfile, UpstreamSettings,
    UpstreamUrlError, build_upstream_client, login_upstream, normalize_upstream_url,
    upstream_url_label,
};
pub use workspace::{
    Collection, Environment, RequestScripts, SavedRequest, Workspace, WorkspaceMutationError,
};
#[allow(unused_imports)]
pub use workspace_provider::{
    LocalWorkspaceProvider, WorkspaceProvider, WorkspaceProviderError, WorkspaceProviderId,
    WorkspaceProviderRegistry,
};

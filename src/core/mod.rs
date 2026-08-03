mod database;
mod format;
mod history;
mod request;
mod request_tabs;
mod script;
mod settings;
mod snippet;
mod template;
mod workspace;

#[cfg(test)]
mod mvp_smoke_test;

pub use database::DatabaseStore;
pub use format::{format_body, is_probably_text};
pub use history::{HistoryEntry, REDACTED_VALUE, RequestHistory};
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
pub use settings::{AppSettings, MetricsPosition, SavedTheme, ShortcutOverride, ThemeSettings};
pub(crate) use snippet::{GENERATOR_WRAPPER_PREFIX, GENERATOR_WRAPPER_SUFFIX};
pub use snippet::{
    MAX_SNIPPET_NAME_BYTES, Snippet, SnippetCancellation, SnippetCategory,
    SnippetInvocationContext, SnippetKind, SnippetLog, SnippetLogLevel, SnippetRequirement,
    SnippetSelection, SnippetSelectionArea, SnippetSelectionSource, SnippetTextRange,
    generate_snippet,
};
pub use template::{RequestTemplate, ResolvedRequest, resolve_request};
pub use workspace::{
    Collection, Environment, RequestScripts, SavedRequest, Workspace, WorkspaceMutationError,
};

mod database;
mod format;
mod history;
mod request;
mod request_tabs;
mod script;
mod settings;
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
    EnvironmentMutation, PostResponseResult, PreRequestResult, ScriptCancellation,
    ScriptDiagnostic, ScriptEnvironment, ScriptError, ScriptErrorKind, ScriptLogLevel, ScriptPhase,
    ScriptReport, ScriptScope, execute_post_response, execute_pre_request,
};
#[cfg(test)]
pub use script::{ScriptLog, ScriptTestResult};
#[allow(unused_imports)]
pub use settings::{AppSettings, MetricsPosition, SavedTheme, ShortcutOverride, ThemeSettings};
pub use template::{RequestTemplate, ResolvedRequest, resolve_request};
pub use workspace::{
    Collection, Environment, RequestScripts, SavedRequest, Workspace, WorkspaceMutationError,
};

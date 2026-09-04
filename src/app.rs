use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, OnceLock, atomic::AtomicUsize},
    time::Duration,
};

use chrono::{Local, Utc};
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, Corner, Entity, EntityId,
    EntityInputHandler, ExternalPaths, Focusable as _, Hsla, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _,
    PathPromptOptions, Pixels, Point, Rems, Render, ScrollHandle, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Timer, WeakEntity, Window,
    anchored, deferred, div, img, point, prelude::FluentBuilder as _, px, rems,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Root, RopeExt as _, Selectable as _,
    Sizable as _, StyledExt as _, WindowExt as _,
    accordion::{Accordion, AccordionItem},
    button::{Button, ButtonVariant, ButtonVariants as _},
    checkbox::Checkbox,
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputEvent, InputInlineAction, InputInlineActionPlacement, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    popover::Popover,
    resizable::{h_resizable, resizable_panel, v_resizable},
    scroll::ScrollableElement as _,
    tab::TabBar,
    tooltip::Tooltip,
    v_flex,
};
use reqwest::Client;
use tokio::{runtime::Runtime, task::AbortHandle};

use crate::{
    brand::{BRAND_EXPLANATION, BRAND_HEADLINE, ICON_ASSET_PATH, PRODUCT_NAME},
    code_editor::{
        CodeEditor, CodeEditorConfig, CodeEditorContextMenuBuilder, CodeEditorContextMenuContext,
        CodeEditorEvent, CodeLanguage, apply_template_pair_edit,
    },
    core::{
        AppSettings, BodyField, BodyFieldKind, BodyMode, COLLECTIONS_CREATE, COLLECTIONS_DELETE,
        COLLECTIONS_UPDATE, Collection, CollectionFolder, CredentialVault,
        DEFAULT_REQUEST_TAB_TITLE, DatabaseStore, ENVIRONMENT_VALUES_UPDATE, ENVIRONMENTS_CREATE,
        ENVIRONMENTS_DELETE, ENVIRONMENTS_READ, ENVIRONMENTS_UPDATE, EditorInlineActionPlacement,
        Environment, EnvironmentMutation, EnvironmentVariable, FormatterSettings, HeaderEntry,
        HistoryEntry, HostnameOverride, ImportBundle, InterchangeFormat, LocalWorkspace,
        LocalWorkspaceProvider, MAX_INTERCHANGE_BYTES, MAX_SNIPPET_NAME_BYTES,
        MAX_WEBSOCKET_TIMELINE_ENTRIES, PostResponseResult, PreRequestResult, QueryParamEntry,
        REDACTED_VALUE, REQUESTS_CREATE, REQUESTS_DELETE, REQUESTS_UPDATE, RawBodyLanguage,
        RealtimeResourceChange, RealtimeSignal, RemoteWorkspaceProvider, RequestDraft,
        RequestError, RequestExecutionMode, RequestExecutionSettings, RequestHistory,
        RequestScripts, RequestTabAssociation, RequestTabCloseScope, RequestTabGroup,
        RequestTabGroupColor, RequestTabGroupId, RequestTabId, RequestTabRecord, RequestTabs,
        RequestTask, RequestTemplate, ResourceCreator, ResponseData, SERVER_SETTINGS_UPDATE,
        STANDARD_HTTP_METHODS, SavedRequest, SavedTheme, ScriptCancellation, ScriptDiagnostic,
        ScriptEnvironment, ScriptError, ScriptErrorKind, ScriptLog, ScriptLogLevel, ScriptPhase,
        ScriptReport, ScriptScope, SharedHistoryUpload, ShortcutOverride, Snippet,
        SnippetCancellation, SnippetCategory, SnippetKind, SnippetLog, SnippetRequirement,
        SnippetSelection, SnippetSelectionArea, SnippetSelectionSource, SnippetTextRange,
        UpstreamCollectionView, UpstreamCredential, UpstreamEnvironmentView, UpstreamProfile,
        UpstreamSavedRequestView, UpstreamWorkspaceError, UpstreamWorkspaceSummary,
        UpstreamWorkspaceView, WORKSPACES_CREATE, WORKSPACES_DELETE, WORKSPACES_READ,
        WORKSPACES_UPDATE, WebSocketAutomationEvent, WebSocketCommand, WebSocketMessageTemplate,
        WebSocketReplay, WebSocketSavedMessage, WebSocketSignal, WebSocketWorkspace, Workspace,
        WorkspaceMutationError, WorkspaceProvider, WorkspaceProviderId, WorkspaceProviderRegistry,
        add_upstream_proxy_allowlist_entry, binary_preview, build_client, build_upstream_client,
        build_upstream_execution_client, create_upstream_collection, create_upstream_environment,
        create_upstream_environment_variable, create_upstream_saved_request,
        create_upstream_workspace, delete_shared_history, delete_upstream_collection,
        delete_upstream_environment, delete_upstream_environment_variable,
        delete_upstream_saved_request, delete_upstream_workspace, execute_websocket_automation,
        export_request, format_body, generate_snippet, get_upstream_execution_policy,
        get_upstream_user, get_upstream_workspace, import_requests, is_probably_text,
        list_upstream_environments, list_upstream_workspaces, login_upstream,
        move_upstream_collection, move_upstream_saved_request, normalize_upstream_url,
        put_upstream_environment_variable_value, query_params_from_url, render_message_template,
        replay_frames, replay_websocket_frames, resolve_request, run_upstream_websocket_connection,
        run_websocket_connection, save_upstream_environment, send_request_for_upstream_workspace,
        spawn_request, template_variable_names, update_request_execution_settings,
        update_upstream_collection, update_upstream_environment,
        update_upstream_environment_variable, update_upstream_saved_request,
        update_upstream_workspace, upload_shared_history, url_with_query_params,
        watch_upstream_changes,
    },
    debug_overlay::DebugOverlay,
    request_dirty::{RequestDirtyPart, RequestDirtyState},
    script_intelligence::{ScriptCompletionProvider, ScriptEditorPhase, ScriptVariableCatalog},
    shortcuts::{self, ShortcutId},
    snippet_intelligence::{SnippetEditorContext, SnippetIntelligenceProvider, SnippetTargetPhase},
    template_intelligence::{
        TemplateClassification, TemplateCompletionProvider, TemplateHighlightColors,
        TemplateVariableCatalog, TemplateVariableCatalogHandle, scan_template_spans,
        semantic_style_spans,
    },
    theme::ApiThemeExt as _,
    typescript_service::TypeScriptServiceHandle,
    web_preview::{HtmlPreview, can_preview},
};

// Composition root only. Page/workspace behavior and every concrete render
// component live in their dedicated child modules under `src/app/`.
mod bootstrap;
mod collection_folder_actions;
mod collections_actions;
mod collections_page;
mod control;
mod drag_drop;
mod editor_settings;
mod environment_browser;
mod environment_detail;
mod environment_selector;
mod environment_variable_grid;
mod environments_actions;
mod environments_page;
mod execution;
mod execution_stage;
mod headers_editor;
mod history_page;
mod navigation_rail;
mod pane_editor;
mod pane_tree;
mod pending_delete;
mod persistence;
mod query_params_editor;
mod realtime;
mod request_actions;
mod request_body_editor;
mod request_interchange;
mod request_pane;
mod request_tab_group_actions;
mod request_tab_reconciliation;
mod request_tab_runtime;
mod request_tab_strip;
mod request_tabs_actions;
mod request_url_bar;
mod request_workspace;
mod response_actions;
mod response_body;
mod response_headers;
mod response_preview;
mod response_tab;
mod response_workspace;
mod script_console;
mod script_console_model;
mod server_management;
mod settings_actions;
mod settings_page;
mod shell;
mod shortcut_actions;
mod sidebar;
mod sidebar_tab;
mod snippets;
mod template_variable_popover;
mod template_variable_popover_model;
mod template_variables;
mod theme_css_actions;
mod theme_css_editor;
mod title_bar;
mod ui_utils;
mod upstream_connections;
mod upstream_workspace_actions;
mod websocket_workspace;
mod welcome_page;
mod window_chrome;
mod workspace_connections;
mod workspace_panes;
mod workspace_tab;
mod workspace_tab_actions;
mod zoom_settings;

use environment_variable_grid::EnvironmentVariableRow;
use execution_stage::*;
use headers_editor::HeaderRow;
use pane_editor::*;
use pane_tree::*;
use pending_delete::*;
use query_params_editor::QueryParamRow;
use request_interchange::RequestInterchangeState;
use request_pane::*;
use request_tab_runtime::*;
use response_tab::*;
use script_console_model::*;
use sidebar_tab::*;
use snippets::*;
use template_variable_popover_model::*;
use template_variables::*;
use ui_utils::*;
use upstream_connections::*;
use websocket_workspace::*;
use workspace_connections::*;
use workspace_tab::*;

const TEMPLATE_HIGHLIGHT_DEBOUNCE: Duration = Duration::from_millis(90);
const TEMPLATE_HOVER_DEBOUNCE: Duration = Duration::from_millis(120);
const REQUEST_TABS_PERSIST_DEBOUNCE: Duration = Duration::from_millis(450);
const THEME_EDITOR_VALIDATION_DEBOUNCE: Duration = Duration::from_millis(100);
const THEME_EDITOR_PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);
/// Height of every in-app title bar, in rems so interface zoom scales it.
const APP_TITLE_BAR_HEIGHT: Rems = Rems(3.25);

struct ThemeEditorSession {
    theme_id: Option<String>,
    theme_name: String,
    editor: Entity<CodeEditor>,
    path: Option<PathBuf>,
    baseline: String,
    dirty: bool,
    disk_source: Option<String>,
    /// Cached parse of the editor source, refreshed by the debounced validation
    /// task (not on every render/keystroke).
    validation: Result<crate::theme::ApiTheme, crate::theme::ThemeError>,
    validation_task: Option<Task<()>>,
    persist_task: Option<Task<()>>,
    _subscription: Subscription,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SnippetDraftSnapshot {
    name: String,
    description: String,
    category: SnippetCategory,
    kind: SnippetKind,
    source: String,
    requirements: Vec<SnippetRequirement>,
}

struct SnippetEditorSession {
    search: Entity<InputState>,
    name: Entity<InputState>,
    description: Entity<InputState>,
    editor: Entity<CodeEditor>,
    preview_editor: Entity<CodeEditor>,
    selected_id: Option<String>,
    category: SnippetCategory,
    kind: SnippetKind,
    requirements: Vec<SnippetRequirement>,
    baseline: SnippetDraftSnapshot,
    notice: Option<String>,
    notice_is_error: bool,
    preview_status: Option<String>,
    preview_logs: Vec<SnippetLog>,
    preview_running: bool,
    preview_valid: bool,
    preview_generation: u64,
    preview_cancellation: Option<SnippetCancellation>,
    _input_subscriptions: Vec<Subscription>,
    _editor_subscription: Subscription,
    _editor_format_subscription: Subscription,
}

pub struct ApiTester {
    method: Entity<InputState>,
    url: Entity<InputState>,
    body: Entity<CodeEditor>,
    pre_request_script: Entity<CodeEditor>,
    post_response_script: Entity<CodeEditor>,
    response_editor: Entity<CodeEditor>,
    query_params: Vec<QueryParamRow>,
    next_query_param_id: usize,
    syncing_query_params: bool,
    headers: Vec<HeaderRow>,
    next_header_id: usize,
    body_mode: BodyMode,
    raw_body_language: RawBodyLanguage,
    body_fields: Vec<request_body_editor::BodyFieldRow>,
    next_body_field_id: usize,
    request_pane: RequestPane,
    response_tab: ResponseTab,
    pretty_body: bool,
    sending: bool,
    execution_stage: Option<ExecutionStage>,
    request_generation: u64,
    mcp_http_operation_id: Option<u64>,
    mcp_http_request_id: Option<String>,
    mcp_http_exchange: Option<control::McpHttpExchangeSnapshot>,
    mcp_request_sequence_generation: u64,
    mcp_request_sequence: Option<control::McpRequestSequence>,
    abort_handle: Option<AbortHandle>,
    script_cancellation: Option<ScriptCancellation>,
    request_namespace: crate::core::RequestNamespaceCatalog,
    chain_budget: Arc<AtomicUsize>,
    response: Option<ResponseData>,
    response_request: Option<RequestDraft>,
    response_sensitive_values: Vec<String>,
    request_error: Option<String>,
    script_diagnostic: Option<ScriptDiagnostic>,
    pre_script_report: Option<ScriptReport>,
    post_script_report: Option<ScriptReport>,
    script_console_input: Entity<CodeEditor>,
    script_console_running: bool,
    mcp_script_console_generation: u64,
    mcp_script_console_operation_id: Option<u64>,
    mcp_script_console_request_id: Option<String>,
    mcp_script_console_owned: bool,
    script_console_history: Vec<String>,
    script_console_history_cursor: Option<usize>,
    script_console_history_draft: String,
    script_console_scroll: ScrollHandle,
    script_console_expanded_rows: HashSet<String>,
    script_console_cleared_key: Option<u64>,
    preview_error: Option<String>,
    copied: bool,
    client: Client,
    runtime: Arc<Runtime>,
    history: RequestHistory,
    history_warning: Option<String>,
    history_writable: bool,
    snippets: Vec<Snippet>,
    workspace: Workspace,
    database_store: DatabaseStore,
    _control_server: Option<crate::control_server::ControlServer>,
    mcp_websocket_generation: u64,
    mcp_websocket: Option<control::ControlWebSocketConnection>,
    workspace_providers: WorkspaceProviderRegistry,
    local_workspaces: Vec<LocalWorkspace>,
    credential_vault: CredentialVault,
    workspace_warning: Option<String>,
    workspace_writable: bool,
    workspace_switch_status: WorkspaceSwitchStatus,
    workspace_switch_generation: u64,
    workspace_switch_abort_handle: Option<AbortHandle>,
    realtime_generation: u64,
    realtime_abort_handle: Option<AbortHandle>,
    realtime_status: RealtimeConnectionStatus,
    realtime_refresh_generation: u64,
    realtime_refresh_abort_handle: Option<AbortHandle>,
    websocket_workspace: WebSocketWorkspaceState,
    workspace_name: Entity<InputState>,
    sidebar_tab: SidebarTab,
    navigation_sidebar_open: bool,
    navigation_compact: bool,
    selected_collection_id: Option<String>,
    selected_folder_id: Option<String>,
    active_saved_request_id: Option<String>,
    detached_request_dirty: bool,
    request_dirty: RequestDirtyState,
    loaded_request_baseline: RequestTemplate,
    request_notice: Option<String>,
    request_interchange: RequestInterchangeState,
    request_tabs: RequestTabs,
    last_persisted_request_tabs: RequestTabs,
    request_tab_runtime: HashMap<String, RequestTabRuntime>,
    request_tabs_persist_task: Option<Task<()>>,
    request_tabs_warning: Option<String>,
    request_tabs_writable: bool,
    request_tab_context_target: Option<request_tab_strip::RequestTabContextTarget>,
    workspace_tabs: WorkspaceTabs,
    panes: PaneRoot,
    pane_editors: HashMap<PaneId, PaneEditorState>,
    settings: AppSettings,
    settings_warning: Option<String>,
    settings_writable: bool,
    base_key_bindings: Vec<gpui::KeyBinding>,
    recording_shortcut_id: Option<ShortcutId>,
    settings_notice: Option<String>,
    mcp_open_tool_groups: HashSet<String>,
    server_management: server_management::ServerManagementState,
    server_management_generation: u64,
    server_management_abort_handle: Option<AbortHandle>,
    profile_history_generation: u64,
    profile_history_abort_handle: Option<AbortHandle>,
    request_history_target: Option<workspace_connections::ActiveUpstreamWorkspace>,
    upstream_client: Client,
    upstream_execution_client: Client,
    upstream_login_open: bool,
    upstream_login_url: Entity<InputState>,
    upstream_login_email: Entity<InputState>,
    upstream_login_password: Entity<InputState>,
    upstream_login_status: UpstreamLoginStatus,
    upstream_login_generation: u64,
    upstream_login_abort_handle: Option<AbortHandle>,
    theme_editors: HashMap<String, ThemeEditorSession>,
    snippet_editor: SnippetEditorSession,
    snippet_apply_generation: u64,
    snippet_apply_cancellation: Option<SnippetCancellation>,
    selected_environment_id: Option<String>,
    collection_search: Entity<InputState>,
    environment_search: Entity<InputState>,
    expanded_collection_ids: BTreeSet<String>,
    expanded_folder_ids: BTreeSet<String>,
    renaming_collection_id: Option<String>,
    renaming_folder_id: Option<String>,
    collection_name: Entity<InputState>,
    folder_name: Entity<InputState>,
    collection_delete_confirmation: Entity<InputState>,
    saved_request_name: Entity<InputState>,
    environment_name: Entity<InputState>,
    environment_variables: Vec<EnvironmentVariableRow>,
    next_variable_row_id: usize,
    pending_delete: Option<PendingDelete>,
    script_variable_catalog: Rc<RefCell<ScriptVariableCatalog>>,
    script_request_namespace: Rc<RefCell<crate::core::RequestNamespaceCatalog>>,
    typescript_service: Option<TypeScriptServiceHandle>,
    template_variable_catalog: TemplateVariableCatalogHandle,
    template_highlight_tasks: HashMap<EntityId, Task<()>>,
    template_variable_hover_task: Option<Task<()>>,
    template_variable_source_hovered: Option<EntityId>,
    template_variable_popover_hovered: bool,
    template_variable_popover: Option<TemplateVariablePopover>,
    focused_template_input: Option<Entity<InputState>>,
    debug_overlay: Entity<DebugOverlay>,
    preview: Option<Entity<HtmlPreview>>,
    /// Bumped whenever `self.workspace` is replaced so derived per-workspace
    /// caches can invalidate.
    workspace_version: u64,
    /// Memoized folder render indexes for the collections sidebar, keyed by
    /// (workspace version, collection id, query) so expanded collections aren't
    /// re-indexed on every frame.
    collection_folder_index_cache:
        RefCell<HashMap<(u64, String, String), Rc<collections_page::CollectionFolderRenderIndex>>>,
    /// Memoized `parse_css` results for the Settings Appearance page, keyed by
    /// exact source text so per-frame theme rendering doesn't re-parse the full
    /// CSS document for every saved theme on every repaint.
    theme_parse_cache:
        RefCell<HashMap<String, Rc<Result<crate::theme::ApiTheme, crate::theme::ThemeError>>>>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RealtimeConnectionStatus {
    #[default]
    Inactive,
    Connecting,
    Connected,
    Reconnecting,
    Unavailable,
}

#[cfg(test)]
mod tests;

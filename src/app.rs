use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use chrono::Local;
use gpui::{
    AnyElement, App, AppContext as _, Axis, ClickEvent, ClipboardItem, Context, Corner, Entity,
    EntityId, EntityInputHandler, Focusable as _, Hsla, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Timer, Window,
    anchored, deferred, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Root, RopeExt as _, Selectable as _,
    Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    checkbox::Checkbox,
    clipboard::Clipboard,
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem},
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
    code_editor::{
        CodeEditor, CodeEditorConfig, CodeEditorEvent, CodeLanguage, apply_template_pair_edit,
    },
    core::{
        AppSettings, BodyField, BodyFieldKind, BodyMode, Collection, DEFAULT_REQUEST_TAB_TITLE,
        DatabaseStore, Environment, EnvironmentMutation, HeaderEntry, HistoryEntry,
        PostResponseResult, PreRequestResult, REDACTED_VALUE, RawBodyLanguage, RequestDraft,
        RequestError, RequestHistory, RequestScripts, RequestTabAssociation, RequestTabCloseScope,
        RequestTabGroup, RequestTabGroupColor, RequestTabGroupId, RequestTabId, RequestTabRecord,
        RequestTabs, RequestTask, RequestTemplate, ResponseData, STANDARD_HTTP_METHODS,
        SavedRequest, SavedTheme, ScriptCancellation, ScriptDiagnostic, ScriptEnvironment,
        ScriptError, ScriptErrorKind, ScriptLogLevel, ScriptPhase, ScriptReport, ScriptScope,
        ShortcutOverride, Workspace, build_client, execute_post_response, execute_pre_request,
        format_body, is_probably_text, resolve_request, spawn_request,
    },
    debug_overlay::DebugOverlay,
    request_dirty::{RequestDirtyPart, RequestDirtyState},
    script_intelligence::{
        ScriptCompletionProvider, ScriptEditorPhase, ScriptVariableCatalog, diagnostics_for_source,
    },
    shortcuts::{self, ShortcutId},
    template_intelligence::{
        TemplateClassification, TemplateCompletionProvider, TemplateHighlightColors,
        TemplateHoverProvider, TemplateVariableCatalog, TemplateVariableCatalogHandle,
        scan_template_spans, semantic_style_spans,
    },
    theme::ApiThemeExt as _,
    web_preview::{HtmlPreview, can_preview},
};

// Composition root only. Page/workspace behavior and every concrete render
// component live in their dedicated child modules under `src/app/`.
mod bootstrap;
mod collection_folder_actions;
mod collections_actions;
mod collections_page;
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
mod pending_delete;
mod persistence;
mod request_actions;
mod request_body_editor;
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
mod settings_actions;
mod settings_page;
mod shell;
mod shortcut_actions;
mod sidebar;
mod sidebar_tab;
mod template_variable_popover;
mod template_variable_popover_model;
mod template_variables;
mod theme_css_actions;
mod theme_css_editor;
mod title_bar;
mod ui_utils;

use environment_variable_grid::EnvironmentVariableRow;
use execution_stage::*;
use headers_editor::HeaderRow;
use pending_delete::*;
use request_pane::*;
use request_tab_runtime::*;
use response_tab::*;
use script_console_model::*;
use sidebar_tab::*;
use template_variable_popover_model::*;
use template_variables::*;
use ui_utils::*;

const TEMPLATE_HIGHLIGHT_DEBOUNCE: Duration = Duration::from_millis(90);
const REQUEST_TABS_PERSIST_DEBOUNCE: Duration = Duration::from_millis(450);
const THEME_EDITOR_VALIDATION_DEBOUNCE: Duration = Duration::from_millis(100);
const THEME_EDITOR_PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);

pub struct ApiTester {
    method: Entity<InputState>,
    url: Entity<InputState>,
    body: Entity<CodeEditor>,
    pre_request_script: Entity<CodeEditor>,
    post_response_script: Entity<CodeEditor>,
    response_editor: Entity<CodeEditor>,
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
    abort_handle: Option<AbortHandle>,
    script_cancellation: Option<ScriptCancellation>,
    response: Option<ResponseData>,
    request_error: Option<String>,
    script_diagnostic: Option<ScriptDiagnostic>,
    pre_script_report: Option<ScriptReport>,
    post_script_report: Option<ScriptReport>,
    preview_error: Option<String>,
    copied: bool,
    client: Client,
    runtime: Arc<Runtime>,
    history: RequestHistory,
    history_warning: Option<String>,
    history_writable: bool,
    workspace: Workspace,
    database_store: DatabaseStore,
    workspace_warning: Option<String>,
    workspace_writable: bool,
    sidebar_tab: SidebarTab,
    navigation_compact: bool,
    selected_collection_id: Option<String>,
    selected_folder_id: Option<String>,
    active_saved_request_id: Option<String>,
    detached_request_dirty: bool,
    request_dirty: RequestDirtyState,
    loaded_request_baseline: RequestTemplate,
    request_notice: Option<String>,
    request_tabs: RequestTabs,
    last_persisted_request_tabs: RequestTabs,
    request_tab_runtime: HashMap<String, RequestTabRuntime>,
    request_tabs_persist_task: Option<Task<()>>,
    request_tabs_warning: Option<String>,
    request_tabs_writable: bool,
    request_tab_context_target: Option<request_tab_strip::RequestTabContextTarget>,
    settings: AppSettings,
    settings_warning: Option<String>,
    settings_writable: bool,
    base_key_bindings: Vec<gpui::KeyBinding>,
    recording_shortcut_id: Option<ShortcutId>,
    settings_notice: Option<String>,
    theme_editor: Option<Entity<CodeEditor>>,
    theme_editor_path: Option<PathBuf>,
    theme_editor_baseline: String,
    theme_editor_dirty: bool,
    theme_editor_disk_source: Option<String>,
    theme_editor_validation_task: Option<Task<()>>,
    theme_editor_persist_task: Option<Task<()>>,
    theme_editor_subscription: Option<Subscription>,
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
    template_variable_catalog: TemplateVariableCatalogHandle,
    template_highlight_tasks: HashMap<EntityId, Task<()>>,
    template_variable_popover: Option<TemplateVariablePopover>,
    focused_template_input: Option<Entity<InputState>>,
    debug_overlay: Entity<DebugOverlay>,
    preview: Option<Entity<HtmlPreview>>,
    _subscriptions: Vec<Subscription>,
}

#[cfg(test)]
mod tests;

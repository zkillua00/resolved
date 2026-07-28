use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use chrono::Local;
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, Corner, Entity, EntityId,
    EntityInputHandler, Focusable as _, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Timer, Window, anchored,
    deferred, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Root, RopeExt as _, Selectable as _,
    Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    checkbox::Checkbox,
    clipboard::Clipboard,
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
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
        BodyField, BodyFieldKind, BodyMode, DatabaseStore, Environment, EnvironmentMutation,
        HeaderEntry, HistoryEntry, PostResponseResult, PreRequestResult, REDACTED_VALUE,
        RawBodyLanguage, RequestDraft, RequestError, RequestHistory, RequestScripts, RequestTask,
        RequestTemplate, ResponseData, STANDARD_HTTP_METHODS, ScriptCancellation, ScriptDiagnostic,
        ScriptEnvironment, ScriptError, ScriptErrorKind, ScriptLogLevel, ScriptPhase, ScriptReport,
        ScriptScope, Workspace, build_client, execute_post_response, execute_pre_request,
        format_body, is_probably_text, resolve_request, spawn_request,
    },
    debug_overlay::DebugOverlay,
    request_dirty::{RequestDirtyPart, RequestDirtyState},
    script_intelligence::{
        ScriptCompletionProvider, ScriptEditorPhase, ScriptVariableCatalog, diagnostics_for_source,
    },
    template_intelligence::{
        TemplateClassification, TemplateCompletionProvider, TemplateHighlightColors,
        TemplateHoverProvider, TemplateVariableCatalog, TemplateVariableCatalogHandle,
        scan_template_spans, semantic_style_spans,
    },
    theme::{
        outline_variant, primary_bright, primary_lavender, surface, surface_container, surface_low,
        surface_lowest,
    },
    web_preview::{HtmlPreview, can_preview},
};

// Composition root only. Page/workspace behavior and every concrete render
// component live in their dedicated child modules under `src/app/`.
mod bootstrap;
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
mod request_tab;
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
mod shell;
mod sidebar;
mod sidebar_tab;
mod template_variable_popover;
mod template_variable_popover_model;
mod template_variables;
mod title_bar;
mod ui_utils;

use environment_variable_grid::EnvironmentVariableRow;
use execution_stage::*;
use headers_editor::HeaderRow;
use pending_delete::*;
use request_tab::*;
use response_tab::*;
use script_console_model::*;
use sidebar_tab::*;
use template_variable_popover_model::*;
use template_variables::*;
use ui_utils::*;

const TEMPLATE_HIGHLIGHT_DEBOUNCE: Duration = Duration::from_millis(90);

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
    request_tab: RequestTab,
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
    active_saved_request_id: Option<String>,
    detached_request_dirty: bool,
    request_dirty: RequestDirtyState,
    loaded_request_baseline: RequestTemplate,
    pending_request_load_key: Option<String>,
    request_notice: Option<String>,
    selected_environment_id: Option<String>,
    collection_search: Entity<InputState>,
    environment_search: Entity<InputState>,
    expanded_collection_ids: BTreeSet<String>,
    renaming_collection_id: Option<String>,
    collection_name: Entity<InputState>,
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
